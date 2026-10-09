# Architecture

> **Partly decided, partly open — and the split is marked.** This file records
> *which* questions are open, *what* each one blocks, and *what* is already
> committed. Two decisions are settled: the betting model
> ([ADR-0001](../adr/0001-peer-to-peer-exchange.md)) and the settlement layer
> ([ADR-0002](../adr/0002-solana-settlement.md)). Everything else is still a
> question, and the withdrawn entries are struck through rather than deleted so
> the rejected option stays visible.

## Current state

No implementation. 58 open questions tracked in [open design questions](../design/open-questions.md), one issue each, plus 8 exchange questions (Q59–Q66) added by
[ADR-0001](../adr/0001-peer-to-peer-exchange.md) and
[ADR-0002](../adr/0002-solana-settlement.md).

**Two decisions are now recorded** and are not up for re-litigation without a
superseding ADR: the betting model, and the settlement layer.

## The dependency spine

**[ADR-0001](../adr/0001-peer-to-peer-exchange.md) decided the top of this
spine.** LadyWalk is a peer-to-peer exchange, so the bookmaker branch below is
history, not a plan. The remaining spine is what the exchange makes hard:

```
                 ┌──────────────────────────────────────────┐
                 │  DECIDED: peer-to-peer exchange           │
                 │  ADR-0001 · price-time priority           │
                 └───────────────────┬──────────────────────┘
                                     │ determines who is owed what,
                                     │ and therefore what must be atomic
     ┌───────────────┬───────────────┼───────────────┬───────────────┐
     ▼               ▼               ▼               ▼               ▼
┌─────────┐  ┌───────────┐  ┌────────────┐  ┌─────────────┐  ┌────────────┐
│Q08 book │  │Q59 exposure│  │Q60 self-   │  │Q61 partial  │  │Q62 cancel  │
│ ordering │  │  limits   │  │  match     │  │  fill &     │  │  after     │
│          │  │            │  │  prevention│  │  remainder  │  │  match     │
└────┬────┘  └─────┬──────┘  └──────┬─────┘  └──────┬──────┘  └─────┬──────┘
     └─────────────┴───────────────┼───────────────┴──────────────┘
                                     ▼
                       ┌──────────────────────────┐
                       │ Q10 exposure check and   │
                       │ Q45 negative balance are │
                       │ ONE atomic step          │
                       └────────────┬─────────────┘
                                    ▼
              ┌─────────────────────────────────────────┐
              │ Q03 ledger is source of truth           │
              │ Q04 double-entry sums to zero           │
              │ Q13/Q27 exactly-once, resumable        │
              └────────────────────┬────────────────────┘
                                   ▼
              ┌─────────────────────────────────────────┐
              │ ADR-0002: settle the NET, on Solana     │
              │ Q63 on-chain idempotency                │
              └─────────────────────────────────────────┘
```

Two things are now visibly true that were not before, and both came from
choosing an exchange:

- **The book ordering decision (Q08) is now a question about an order book**,
  not about who owns a price. It was already a candidate; it is now on the
  critical path.
- **Q10 and Q45 stop being "make the balance correct" and become "evaluate
  exposure atomically with the mutation that changes it."** The race is the
  same race. The thing being protected is the exposure limit, not a
  non-negative balance.

Three more run across everything:

| Question | Cross-cutting because |
| --- | --- |
| **[Q36](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/36)** — monolith vs services | Decides the failure domain the money path lives in |
| **[Q35](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/35)** — kill switch | Must be able to stop all money movement regardless of shape |
| **[Q47](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/47)** — audit log | Every subsystem writes to it; retrofitting it is expensive |

## Open decisions

| # | Decision | Blocks | Issue |
| --: | --- | --- | --- |
| — | ~~Betting model: fixed-odds / pari-mutuel / exchange~~ | — | **Decided: [ADR-0001](../adr/0001-peer-to-peer-exchange.md)** |
| 2 | Order book data structure and per-market serialization | 8, 38, 39 | [#8](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/8) |
| 3 | Real-time transport | 22, 23, 24, 25 | [#21](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/21) |
| 4 | p99 budget for "bet accepted" | 37, 48 | [#37](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/37) |
| 5 | As-of reconstruction: event sourcing vs snapshot+delta | 2, 5, 6 | [#6](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/6) |
| 6 | Wallet locking strategy | 8, 10, 45 | [#11](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/11) |
| 7 | ~~Odds update curve (LMSR)~~ | — | **Withdrawn: not applicable to an exchange, [ADR-0001](../adr/0001-peer-to-peer-exchange.md)** |
| 8 | Fail safe vs fail open on pipeline death | 18, 31, 33 | [#31](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/31) |
| 9 | Clawback policy after a wrong settlement | 26, 27, 29 | [#29](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/29) |
| 10 | Wallet sharding (or an explicit decision not to) | 12, 38 | [#12](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/12) |
| 11 | Calculation versioning scheme | 5, 58 | [#58](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/58) |
| 12 | Webhook / fanout transport for 100K connections | 22, 25, 40 | [#22](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/22) |
| 13 | Monolith vs services, and the money path's failure domain | 22, 36, 38, 39 | [#36](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/36) |

### Added by the exchange decision

[ADR-0001](../adr/0001-peer-to-peer-exchange.md) and
[ADR-0002](../adr/0002-solana-settlement.md) create questions that did not exist
when this product was a bookmaker, because a bookmaker never had to answer them.
They are tracked in [open design questions](../design/open-questions.md) and
numbered from Q59:

| # | Decision | Why the exchange forces it |
| --: | --- | --- |
| 13 | Exposure limit per account, and what it applies to | Unmatched back bets make a customer a debtor; without a bound the platform is the counterparty by accident |
| 14 | Self-match prevention policy | Two of a customer's own orders matching is usually a mistake and is occasionally fraud |
| 15 | Order lifecycle: partial fill, remainder, cancel-after-match | A bookmaker's bet is atomic; an exchange's order is not |
| 16 | Commission model on net winnings | There is no vig, so this replaces the entire revenue calculation |
| 17 | Odds tick-size ladder per market | Prices are now customer-proposed and must be constrained to a valid set |
| 18 | On-chain settlement idempotency | [ADR-0002](../adr/0002-solana-settlement.md): Solana gives at-most-once per transaction, not exactly-once per business event |
| 19 | Market suspension and in-play behaviour | Empty books and a suspended market are a real failure mode for a peer-to-peer book |
| 20 | Odd-lot handling: who is counterparty to the remainder | [ADR-0001](../adr/0001-peer-to-peer-exchange.md) admits the exchange takes the residual; that residual must be priced |

## Committed constraints

These are *not* open. They follow from the question set and are not up for re-litigating without a new ADR.

### Money

- **Money is never a float.** A `Money` value type, `BIGINT` minor units + explicit currency in the database. Enforced by a CI lint gate, not by convention. ([Q01](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/1))
- **The ledger is the source of truth.** A `balance` column is a derived projection. There is no code path that mutates a posted entry. Corrections are compensating entries. ([Q03](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/3))
- **An unmatched order is a ledger event, not a pending request.** In an exchange an order that does not fill has still changed the customer's exposure, so it posts to the ledger when accepted and is relieved by a compensating entry when it fills, is cancelled, or is otherwise resolved. An order's whole life is reconstructible from the ledger.
- **Double-entry sums to zero, with zero tolerance.** Not "within rounding". If a reconciliation finds a discrepancy, the run fails loudly. ([Q04](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/4), [Q30](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/30), [Q51](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/51))
- **Payouts are exactly-once and resumable.** Idempotency keys everywhere, and a settlement job checkpointed in the same transaction as the payout. ([Q13](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/13), [Q27](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/27), [Q28](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/28))
- **The client proposes the price; the matching engine decides the match.** This is the inverse of a bookmaker's guarantee and is now the committed invariant: an order may match only at a price and size permitted by the tick-size ladder and by the remaining exposure of *both* accounts, and every matched bet is reconstructible from the order book event log. ([ADR-0001](../adr/0001-peer-to-peer-exchange.md), [Q19](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/19), [Q42](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/42))
- **A balance may be negative, bounded by an exposure limit.** The invariant is not "the balance is never negative" but "no account exceeds its configured exposure limit, and each decrement is evaluated in the same atomic step as the order causing it." ([ADR-0001](../adr/0001-peer-to-peer-exchange.md), [Q10](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/10), [Q45](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/45))
- **Price-time priority is a correctness property.** Two equally-priced orders matching in a different order is customer-visible and disputable, so the ordering is specified, tested, and auditable. ([ADR-0001](../adr/0001-peer-to-peer-exchange.md))
- **No vig, no pricing curve, no platform liability cap.** Revenue is commission on net winnings. The old margin, LMSR curve and per-outcome platform cap do not apply to this product. ([ADR-0001](../adr/0001-peer-to-peer-exchange.md))
- **Settlement moves the net, on Solana.** One settlement per (customer, market, epoch) rather than per bet, keyed by a settlement nonce so a retry cannot pay twice. Integer minor units end to end, with no float and no implicit rounding at the on-chain boundary. ([ADR-0002](../adr/0002-solana-settlement.md))

### State

- **Odds history is immutable.** Every bet records the exact odds version it executed against, and every odds version records the calculation version that produced it. Any historical payout must be reproducible. ([Q05](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/5), [Q58](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/58))
- **Voids are compensations, not mutations.** A void never rewrites what was originally paid. ([Q07](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/7))
- **Every state change is audited tamper-evidently.** Hash-chained, append-only, not writable by the application role, retained past the dispute window. ([Q47](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/47))
- **Grading is hybrid with gated human override.** Overrides are the highest-risk admin action and are always attributable. ([Q26](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/26), [Q44](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/44))

### Failure behaviour

- **Fail safe, not fail open.** If the odds pipeline dies mid-event, betting on that market halts. Trading revenue against financial correctness is a choice, and this one is made explicitly. ([Q31](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/31))
- **One switch stops all money movement.** A platform-wide kill switch, audited, with defined trigger authority and a measured time-to-halt. ([Q35](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/35))
- **A bad market is quarantined automatically.** Plausibility invariants on odds; a breach blocks new bets and pages. ([Q33](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/33))
- **The ledger has its own DR posture.** RPO/RTO for the ledger are specified separately from application state, and the restore is drilled. ([Q34](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/34))

## Planned top-level shape

Provisional, subject to [Q36](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/36):

```
                    ┌──────────────────────────────────┐
   clients  ◄─────► │  order-entry service              │  ◄── the money path
                    │  auth · rate limit · idem · tick  │      single ACID boundary
                    └────────────┬─────────────────────┘      (Q09)
                                 │ accepted order
                                 ▼
                    ┌──────────────────────────────────┐
                    │  matching engine                 │  ◄── single writer
                    │  price-time priority book        │      per market
                    │  self-match prevention            │      (Q08, Q60)
                    └────────────┬─────────────────────┘
                                 │ matched bet (two legs)
                                 ▼
        ┌────────────────────────────────┬───────────────────────────┐
        ▼                                ▼                           ▼
┌───────────────┐            ┌──────────────────────┐    ┌──────────────────┐
│  wallet +     │            │  exposure +          │    │  real-time       │
│  ledger       │            │  commission engine   │    │  gateway         │
│  (Q03, Q04)   │            │  (Q59, Q64)          │    │  stateless, thin │
└───────────────┘            └──────────┬───────────┘    └──────────────────┘
                                        │ net position
                                        ▼
                         ┌──────────────────────────────┐
                         │  settlement + grading        │
                         │  net per (customer, market)  │
                         └────────────┬─────────────────┘
                                      │ idempotent, on-chain
                                      ▼
                         ┌──────────────────────────────┐
                         │  Solana (devnet / testnet)   │  ◄── ADR-0002
                         │  tokens + audit hash anchor  │
                         └──────────────────────────────┘
```

Everything except the money path and the matching engine is a candidate to be a
module in one deployable. The minimum defensible service split is order entry
plus matching, because that is the failure domain that can lose money or
mis-price a customer.

The matching engine is deliberately drawn as a separate single-writer box.
Under [ADR-0001](../adr/0001-peer-to-peer-exchange.md) it is the most
correctness-critical object in the system, and the p99 budget rules out putting
a consensus round trip anywhere near it ([ADR-0002](../adr/0002-solana-settlement.md)).

## Documents

Written and maintained:

- [Documentation index](../README.md) — everything in `docs/`
- [open design questions](../design/open-questions.md) — the 58 questions and their issues
- [Roadmap](../roadmap.md) — the 260-issue build plan
- [Decision records](../adr/) — one file per decision:
  - [ADR-0001: peer-to-peer exchange](../adr/0001-peer-to-peer-exchange.md) — **Accepted**
  - [ADR-0002: Solana for settlement, matching off-chain](../adr/0002-solana-settlement.md) — **Accepted**

Referenced but **not yet written**, because the question that unblocks each one is
still open. Listed as unwritten rather than stubbed, so a missing document is
visible in review instead of being mistaken for an empty one:

- `docs/schema.md` — ERD and the immutable/mutable column split (blocked on [Q02](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/2))
- `docs/slos.md` — SLIs, SLOs, error budgets (blocked on [Q48](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/48))
- `runbooks/` — exposure breach, quarantined market, ledger imbalance, kill switch, stalled settlement ([Q35](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/35), [Q51](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/51), [Q63](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/63))
- `docs/orderbook.md` — book structure, price-time priority, self-match policy, partial fills (blocked on [Q08](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/8), [Q60](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/60), [Q61](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/61))
- `docs/settlement.md` — net-position calculation, commission, on-chain settlement state machine (blocked on [Q64](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/64), [Q65](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/65))
