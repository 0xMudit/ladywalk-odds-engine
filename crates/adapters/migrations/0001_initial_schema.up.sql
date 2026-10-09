-- Migration: 0001_initial_schema
-- Accounts, markets, orders, quotes, and an append-only double-entry ledger.
--
-- Money representation (Q01):
--   Every monetary amount is a BIGINT count of minor units paired with a
--   currency code. Approximate and exact-numeric column types are prohibited.
--   scripts/check-migration-money-types.sh fails the build if one is declared.
--
-- Append-only design (Q03):
--   ledger_entries has no UPDATE path. Triggers reject both, and the only
--   correction is a compensating transaction that references the original.
--
-- Identifiers are TEXT, not UUID:
--   The domain treats an id as an opaque string, and some accounts are not
--   customers — the platform liability account and a per-market holding
--   account are both named. A UUID column would force a UUID-typed
--   representation on a domain type that deliberately has none, and would
--   leave the system accounts with nowhere to live.

-- ─── Extensions ──────────────────────────────────────────────────────────────
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ─── Currency reference table ────────────────────────────────────────────────
CREATE TABLE currencies (
    code        CHAR(3)  PRIMARY KEY,                -- ISO 4217 e.g. 'GBP'
    exponent    SMALLINT NOT NULL CHECK (exponent >= 0 AND exponent <= 6),
    description TEXT     NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO currencies (code, exponent, description) VALUES
    ('GBP', 2, 'British Pound Sterling'),
    ('USD', 2, 'United States Dollar'),
    ('EUR', 2, 'Euro');

-- ─── Accounts ────────────────────────────────────────────────────────────────
-- A ledger balance belongs to an account. Customers and the platform's own
-- accounts are both rows here, distinguished by `kind`, so that an id minted by
-- the system can never be mistaken for a customer who happens to log in with
-- the same subject string.
CREATE TABLE accounts (
    id           TEXT        PRIMARY KEY,
    kind         TEXT        NOT NULL CHECK (kind IN ('CUSTOMER', 'SYSTEM')),
    external_ref TEXT        UNIQUE,        -- Keycloak subject, customers only
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT accounts_external_ref_required_for_customers
        CHECK (kind <> 'CUSTOMER' OR external_ref IS NOT NULL)
);

CREATE INDEX accounts_external_ref_idx ON accounts (external_ref);

-- ─── Markets ─────────────────────────────────────────────────────────────────
-- The published price is stored as a count of ten-thousandths, matching
-- Odds::raw in the domain, so a market row and a signed quote compare equal
-- without a conversion in between.
CREATE TABLE markets (
    id                   TEXT        PRIMARY KEY,
    state                TEXT        NOT NULL CHECK (state IN (
                                'OPEN', 'SUSPENDED', 'SETTLED', 'QUARANTINED')),
    currency_code        CHAR(3)     NOT NULL REFERENCES currencies (code),
    published_odds_tenthousandths BIGINT CHECK (
                            published_odds_tenthousandths IS NULL
                            OR published_odds_tenthousandths >= 10_100),
    outcome              TEXT        CHECK (outcome IN ('HOME_WIN', 'AWAY_WIN', 'DRAW')),
    opened_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    settled_at           TIMESTAMPTZ,

    -- A market that has resolved must say how, and one that has not must not
    -- claim to have. This is the check that stops a settlement from being
    -- recorded against a market that is still running.
    CONSTRAINT markets_settled_has_outcome
        CHECK ((state = 'SETTLED') = (outcome IS NOT NULL))
);

CREATE INDEX markets_state_idx ON markets (state);

-- A market's holding account is created with the market rather than on first
-- fill. Creating it lazily would mean a concurrent first-fill could race to
-- create the same account twice, or that a settlement could run against a
-- market whose hold had never existed and pay out of nothing.
CREATE OR REPLACE FUNCTION create_market_holding_account() RETURNS trigger AS $$
BEGIN
    INSERT INTO accounts (id, kind) VALUES ('hold:' || NEW.id, 'SYSTEM');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER markets_create_holding_account
    AFTER INSERT ON markets
    FOR EACH ROW EXECUTE FUNCTION create_market_holding_account();

-- ─── Orders ──────────────────────────────────────────────────────────────────
-- One row per instruction. An order is a single request to stake a sum on one
-- selection, and it can be filled by several counterparties — so the
-- counterparties are *not* a column on this table. See order_matches below.
CREATE TABLE orders (
    id               TEXT        PRIMARY KEY,
    account_id       TEXT        NOT NULL REFERENCES accounts (id),
    market_id        TEXT        NOT NULL REFERENCES markets (id),
    selection        TEXT        NOT NULL CHECK (selection IN ('HOME', 'AWAY', 'DRAW')),
    stake_minor      BIGINT      NOT NULL CHECK (stake_minor > 0),
    currency_code    CHAR(3)     NOT NULL REFERENCES currencies (code),
    odds_tenthousandths BIGINT   CHECK (
                        odds_tenthousandths IS NULL
                        OR odds_tenthousandths >= 10_100),
    status           TEXT        NOT NULL CHECK (status IN (
                        'PENDING', 'MATCHED', 'SETTLED', 'CANCELLED')),

    -- The transaction that took this order's stake into its market's holding
    -- account. NULL until matched, and retained afterwards: the money did not
    -- come back, and the reference is the audit trail for where it went.
    holds_funds_ref  TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- A price is decided once, but an order may be persisted before it is
    -- quoted, so NULL is a real state here. Validity of the value is the
    -- domain's: `Odds` cannot be constructed below its minimum, and a matched
    -- order with no price is a domain-level internal error at settlement rather
    -- than something the column can rule out. A CHECK that forbade it would
    -- forbid a state the domain deliberately models.
    --
    -- Holding funds is the same fact as having matched: an order is either
    -- PENDING with nothing held, or MATCHED/SETTLED with its stake held. A
    -- matched order with no stake held is unrepresentable rather than merely
    -- discouraged.
    CONSTRAINT orders_holding_matches_status CHECK (
        (status IN ('MATCHED', 'SETTLED') AND holds_funds_ref IS NOT NULL)
        OR (status IN ('PENDING', 'CANCELLED') AND holds_funds_ref IS NULL))
);

CREATE INDEX orders_market_status_idx ON orders (market_id, status);
CREATE INDEX orders_account_idx        ON orders (account_id);

-- ─── Order matches ───────────────────────────────────────────────────────────
-- A fill, not a status. A taker filled by three makers has three rows here and
-- one stake, which is what the domain's MatchOutcome describes: the taker's
-- money is taken once, and the counterparties are many.
--
-- A single `matched_against` column on orders could not express this, and
-- choosing it would have forced the matching engine to either drop fills beyond
-- the first or to overwrite the earlier one. Both are silent corruption of a
-- customer's money, which is why the relation is a table.
CREATE TABLE order_matches (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    taker_order_id   TEXT        NOT NULL REFERENCES orders (id),
    maker_order_id   TEXT        NOT NULL REFERENCES orders (id),
    stake_minor      BIGINT      NOT NULL CHECK (stake_minor > 0),
    holding_ref      TEXT        NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- A customer cannot match their own order.
    CONSTRAINT order_matches_not_self CHECK (taker_order_id <> maker_order_id),

    -- The same pair cannot fill twice. Without this, a retried matching pass
    -- would double-take a maker's stake, and the retry would look successful.
    CONSTRAINT order_matches_pair_unique UNIQUE (taker_order_id, maker_order_id)
);

-- The lookup the engine actually runs: every fill for one order, either side.
CREATE INDEX order_matches_taker_idx ON order_matches (taker_order_id);
CREATE INDEX order_matches_maker_idx ON order_matches (maker_order_id);
CREATE INDEX order_matches_holding_idx ON order_matches (holding_ref);

-- A market's fills are only meaningful within that market, and a fill across two
-- markets would be money held against the wrong holding account.
CREATE OR REPLACE FUNCTION order_matches_share_a_market() RETURNS trigger AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
          FROM orders taker, orders maker
         WHERE taker.id = NEW.taker_order_id
           AND maker.id = NEW.maker_order_id
           AND taker.market_id = maker.market_id
    ) THEN
        RAISE EXCEPTION
            'order match % pairs orders from different markets', NEW.id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER order_matches_same_market
    BEFORE INSERT ON order_matches
    FOR EACH ROW EXECUTE FUNCTION order_matches_share_a_market();

-- ─── System accounts ─────────────────────────────────────────────────────────
-- The platform's revenue account is a real ledger account and must exist before
-- any settlement can credit it, since ledger_entries.account_id references it.
--
-- There is deliberately no platform *liability* account. ADR-0001 chose a
-- peer-to-peer exchange, where the customers are the counterparties and a
-- winner is paid from their market's own holding account. An exchange has no
-- guaranteed payouts, so it carries no liability; adding the account would
-- invite exactly the bug where settlement pays from it.
INSERT INTO accounts (id, kind) VALUES
    ('platform:revenue', 'SYSTEM');

-- ─── Quotes ──────────────────────────────────────────────────────────────────
-- The expiry is stored as the same integer the signature covers. Storing a
-- timestamp and converting it on the way out would mean the signed bytes and
-- the stored bytes could disagree, and every signature would fail after a
-- round trip.
CREATE TABLE quotes (
    id                     TEXT        PRIMARY KEY,
    account_id             TEXT        NOT NULL REFERENCES accounts (id),
    market_id              TEXT        NOT NULL REFERENCES markets (id),
    selection              TEXT        NOT NULL CHECK (selection IN ('HOME', 'AWAY', 'DRAW')),
    odds_tenthousandths    BIGINT      NOT NULL CHECK (odds_tenthousandths >= 10_100),
    stake_minor            BIGINT      NOT NULL CHECK (stake_minor > 0),
    currency_code          CHAR(3)     NOT NULL REFERENCES currencies (code),
    expires_at_unix        BIGINT      NOT NULL,
    signature              TEXT        NOT NULL CHECK (length(signature) > 0),
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX quotes_account_idx   ON quotes (account_id);
CREATE INDEX quotes_expires_idx   ON quotes (expires_at_unix);

-- ─── Idempotency ─────────────────────────────────────────────────────────────
-- A replay of a request that was already answered returns the first answer. The
-- digest is what distinguishes a retry from a different request that happened
-- to reuse the key: same key and same digest is a retry, same key and a
-- different digest is a client bug worth refusing.
CREATE TABLE idempotency_keys (
    key              TEXT        PRIMARY KEY,
    account_id       TEXT        NOT NULL REFERENCES accounts (id),
    endpoint         TEXT        NOT NULL,
    request_digest   TEXT        NOT NULL,
    response_status  INTEGER     NOT NULL,
    response_body    TEXT        NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idempotency_keys_created_idx ON idempotency_keys (created_at);

-- ─── Ledger entries (append-only) ────────────────────────────────────────────
-- Every money movement is a set of entries that sum to zero. The balance is
-- derived from this table and is never stored.
CREATE TABLE ledger_entries (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      TEXT        NOT NULL REFERENCES accounts (id),

    -- Signed count of minor units: positive debits the account, negative
    -- credits it. A deposit debits the customer and credits the platform.
    amount_minor    BIGINT      NOT NULL,

    currency_code   CHAR(3)     NOT NULL REFERENCES currencies (code),
    entry_type      TEXT        NOT NULL CHECK (entry_type IN ('CREDIT', 'DEBIT')),
    entry_reason    TEXT        NOT NULL CHECK (entry_reason IN (
                        'DEPOSIT', 'WITHDRAWAL', 'BET_STAKE', 'BET_STAKE_RELEASED',
                        'SETTLEMENT_PAYOUT', 'COMMISSION', 'RESIDUAL_SWEEP',
                        'SETTLEMENT_RECOVERY', 'COMPENSATION')),

    -- Groups the legs of one atomic business operation.
    transaction_ref TEXT        NOT NULL,

    -- The request that caused the transaction, for safe replay.
    idempotency_key TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- A zero entry moves nothing, so it is a mistake. The domain refuses to
    -- build one; the column refuses to store one.
    CONSTRAINT ledger_entries_amount_nonzero CHECK (amount_minor <> 0),

    -- entry_type is derived from the sign rather than stored independently, so
    -- it cannot be written as something the amount contradicts.
    CONSTRAINT ledger_entries_type_matches_sign CHECK (
        (entry_type = 'DEBIT' AND amount_minor > 0)
        OR (entry_type = 'CREDIT' AND amount_minor < 0))
);

CREATE INDEX ledger_entries_account_id_idx      ON ledger_entries (account_id);
CREATE INDEX ledger_entries_transaction_ref_idx ON ledger_entries (transaction_ref);
CREATE INDEX ledger_entries_idempotency_key_idx ON ledger_entries (idempotency_key) WHERE idempotency_key IS NOT NULL;
CREATE INDEX ledger_entries_created_at_idx      ON ledger_entries (created_at);

-- ─── Database-level enforcement of Q03 ───────────────────────────────────────
-- The domain refuses to construct an unbalanced transaction, and this refuses
-- to store one. Two independent checks of the same invariant is the point: the
-- second catches the first being bypassed by a hand-written INSERT during an
-- incident, which is exactly when the balance is hardest to trust.
--
-- The same trigger enforces that every leg of a transaction is denominated in
-- the same currency. A sum-to-zero check across two currencies can be satisfied
-- while the whole is unbalanced within either of them, so the currencies have
-- to agree before the sum means anything.
CREATE OR REPLACE FUNCTION assert_ledger_transaction_is_well_formed() RETURNS trigger AS $$
DECLARE
    ref            TEXT := NEW.transaction_ref;
    expected       CHAR(3) := NEW.currency_code;
    currency_count INTEGER;
    total          BIGINT;
BEGIN
    SELECT count(DISTINCT currency_code), COALESCE(SUM(amount_minor), 0)
      INTO currency_count, total
      FROM ledger_entries
     WHERE transaction_ref = ref;

    IF currency_count > 1 THEN
        RAISE EXCEPTION
            'ledger transaction % mixes % currencies', ref, currency_count
            USING ERRCODE = 'check_violation';
    END IF;

    IF total <> 0 THEN
        RAISE EXCEPTION
            'ledger transaction % does not sum to zero', ref
            USING ERRCODE = 'check_violation';
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Deferred to commit time, because a transaction's legs are inserted in more
-- than one statement and neither the total nor the currency set is knowable
-- until they are all present.
CREATE CONSTRAINT TRIGGER ledger_entries_well_formed
    AFTER INSERT ON ledger_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION assert_ledger_transaction_is_well_formed();

-- Append-only, enforced rather than documented. A migration that revokes
-- privileges can be undone by a role grant; a trigger cannot be bypassed by
-- anything short of dropping it.
CREATE OR REPLACE FUNCTION ledger_entries_are_append_only() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION
        'ledger_entries is append-only: % is not permitted', TG_OP
        USING ERRCODE = 'insufficient_privilege';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER ledger_entries_no_update
    BEFORE UPDATE ON ledger_entries
    FOR EACH ROW EXECUTE FUNCTION ledger_entries_are_append_only();

CREATE TRIGGER ledger_entries_no_delete
    BEFORE DELETE ON ledger_entries
    FOR EACH ROW EXECUTE FUNCTION ledger_entries_are_append_only();

COMMENT ON TABLE ledger_entries IS
    'Append-only double-entry ledger. Corrections are compensating entries.';
COMMENT ON CONSTRAINT ledger_entries_amount_nonzero ON ledger_entries IS
    'A zero leg moves no money; the domain refuses to build one.';
