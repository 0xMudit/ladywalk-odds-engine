# 0002. Solana for settlement, with the matching engine kept off-chain

**Status:** Accepted
**Date:** 2026-09-27
**Issue:** [#15](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/15)
**Deciders:** @BerlinDeskMudit
**Supersedes:** none

## Context

[ADR-0001](0001-peer-to-peer-exchange.md) makes LadyWalk a peer-to-peer exchange
where a matched bet is a relationship between two customers. That raises the
question of where money actually moves, and it constrains the answer hard
before any chain is considered.

Two facts dominate:

1. **The matching engine cannot be on-chain.** Solana slot time is roughly
   400ms. The committed budget is p99 under 50ms for order acceptance
   ([Q37](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/37)).
   A consensus-finality round trip cannot sit under that budget, and no amount
   of tuning changes the order of magnitude. Anything on the hot path stays in
   a centralised, single-writer service.
2. **Exposure cannot be evaluated against a cluster.** Under
   [ADR-0001](0001-peer-to-peer-exchange.md) a customer may hold unmatched
   exposure, and every decrement must be evaluated atomically with the order
   that causes it. An RPC round trip per order is both too slow and not atomic
   with the order book.

So the question is not "should this be on-chain" but **which layer is
authoritative for which fact**. The answer is that they are different, and the
design must say so explicitly rather than blurring them.

Options considered were a ledger-only design with no chain, a chain for
everything, and the split adopted below.

### Option A — Ledger only, no chain

An internal PostgreSQL double-entry ledger is the sole record. Simple, fast,
fully testable, and it satisfies every committed constraint.

- Weakness: settlement is self-attested. "We computed your winnings correctly" is
  exactly the claim the project exists to make falsifiable, and this option
  cannot support it.

### Option B — Everything on-chain

Orders, matching, and settlement all as program instructions.

- Rejected immediately by the 400ms-versus-50ms arithmetic above. Recorded here
  so the option is visibly considered rather than assumed away.

### Option C — Split: off-chain matching, on-chain settlement *(chosen)*

Off-chain, authoritative for: orders, the book, matches, exposure, commission,
and the customer-facing balance. On-chain, authoritative for: final transfer of
tokens, and a periodic hash anchor of the audit chain.

## Decision

**Option C.** Solana is the settlement and finality layer. The matching engine,
the order book, and the exposure ledger stay off-chain and remain the source of
truth for anything that must be fast or must be atomic with a book mutation.

Specific commitments:

1. **Settle the net, not the bet.** One settlement per (customer, market, epoch),
   not per bet. Partial fills and in-play settlement would otherwise generate a
   transaction per fill, which is both expensive and unnecessary — the
   customer's position in a market is the only thing that needs to move.

2. **The chain is authoritative for token balances, the off-chain ledger is
   authoritative for exposure.** These are different quantities and conflating
   them is the main way this design could be got wrong. A customer may be
   negative off-chain (unmatched exposure) while holding tokens on-chain.

3. **On-chain idempotency is mandatory. Solana does not provide exactly-once.**
   A signed transaction cannot execute twice, but a retry that builds a *second*
   transaction can pay twice, and RPC timeouts make retries unavoidable. Each
   settlement is therefore keyed by a settlement-nonce account derived from
   `(customer, market, epoch)`, and the program rejects a second settlement for
   a nonce. This is the single most likely way an on-chain exchange loses money,
   so it is a correctness requirement and not an optimisation.

4. **Integer minor units throughout, end to end.** SPL token amounts are `u64`
   integers. The `Money` newtype over `i64` maps onto them without conversion, so
   the "never a float" guarantee ([Q01](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/1))
   extends across the on-chain boundary with no decimal handling. Any
   multiplication, such as commission on net winnings, is integer arithmetic
   with an explicitly documented rounding direction — never a float, and never
   a rounding rule that is implicit in the type.

5. **Rust on both sides.** The settlement program is an Anchor program, so the
   "one backend language" constraint holds. There is no second toolchain and no
   conversion boundary around money.

6. **Devnet and testnet only, never mainnet.** Tokens on either have no value,
   which preserves the project's no-real-money property. Devnet is the stable
   target for CI because testnet is periodically reset; testnet is used where
   realism matters. Mainnet is out of scope until a separate ADR, and would
   require the licensing position to be settled first.

7. **Settlement runs behind a feature flag with a ledger-only mode.** The
   off-chain ledger is a complete implementation, not a stub. CI must be able to
   exercise the entire settlement path with no cluster reachable, and the same
   tests must run against a local validator.

8. **The audit chain is anchored on-chain.** A periodic commitment to the head
   hash of the tamper-evident audit log is written to the chain, so the log is
   provably not rewritten after the fact.

## Consequences

**Becomes easier:**

- Settlement is publicly verifiable by anyone, which matches the project's
  "we measured it, not we thought about it" standard.
- The on-chain settlement trail is a real artefact for a dispute or an audit,
  rather than a database row a customer has to trust.
- The money representation is uniform across the boundary, so there is no place
  for a rounding bug to hide at the off-chain/on-chain seam.
- No real money is ever at risk, because no real token ever exists.

**Becomes harder:**

- Two authoritative stores, so "what is the balance" has a precise but subtle
  answer that must be documented and tested at the boundary.
- A new failure surface: transaction simulation, confirmation, retry, and
  reorg handling, none of which exists in the ledger-only design.
- The settlement program is a second artefact that must be reviewed, versioned
  and (eventually) audited independently of the service.
- Idempotency must be enforced on-chain, which is more machinery than simply
  having one database transaction.

**We accept these costs:**

- **Devnet and testnet tell us nothing about production latency or economics.**
  Testnet is resettable and faucet-driven, and public RPC is rate-limited, so
  neither can validate the 50ms budget, nor tell us how the system behaves under
  real economic abuse such as a wallet deliberately defaulting. Load and threat
  testing stay against the off-chain path, and any claim about on-chain
  behaviour under load is explicitly out of scope until mainnet-or-equivalent
  infrastructure exists.
- Settlement is not atomic with matching, and never will be. The window between
  a match and its on-chain settlement is a real, accepted gap in finality, and
  must be visible to the customer rather than papered over.
- A cluster dependency in the settlement path is an availability dependency
  that the ledger-only design does not have.

**Revisit if:**

- Median on-chain settlement confirmation exceeds **30 seconds** at p95 over a
  7-day window on devnet or testnet, making the customer-visible settlement lag
  unacceptable.
- The RPC provider's rate limit forces a dedicated validator, at which point the
  operational cost needs re-evaluating against the benefit.
- Commission or tax logic requires arithmetic that `u64` minor units cannot
  express exactly, which would mean a second on-chain representation.
- The project moves toward mainnet, which reopens licensing, custody and
  consumer-protection questions that are explicitly out of scope here.
