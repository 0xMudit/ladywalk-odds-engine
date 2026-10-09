# 0001. Peer-to-peer exchange with a price-time priority order book

**Status:** Accepted
**Date:** 2026-09-27
**Issue:** [#15](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/15)
**Deciders:** @BerlinDeskMudit
**Supersedes:** none

## Context

The project must choose who takes the other side of a bet. Three models were
on the table, and they are not variations on a theme — they have different
concurrency models, different liability, different failure modes, and different
regulatory surface. Choosing late is expensive because the money path is
downstream of this choice.

Two constraints are already committed and both bear on the decision:

- **p99 for "bet accepted" under 50ms at sustained load** ([Q37](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/37), [Q38](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/38)).
- **Money is an integer minor-unit newtype, never a float** ([Q01](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/1)).

And one property is forced by the *product*, not by the engineering: a customer
can hold an **unmatched back bet**, which is a liability the customer owes the
exchange. That is not a bookmaker property. A bookmaker cannot lose on a single
bet because it never accepts one; an exchange accepts unmatched exposure by
construction.

## Options considered

### Option A — Fixed-odds bookmaker

The platform is the counterparty. It publishes its own prices and accepts or
rejects each bet at the price it chose.

- Simple concurrency: one row per bet, one conditional balance deduction.
- No order book, no matching engine, no self-match problem.
- Platform carries all risk, so it needs a liability cap and a pricing model.
- Requires vig or margin, which means a pricing curve and overround management.
- **Contradicts the committed latency budget at scale**: pricing a bet well
  enough to be competitive is a modelling problem, not a lookup.

### Option B — Pari-mutuel pool

Backers pool into a pot; the price is derived from the pool and drifts as it
grows.

- No counterparty risk and no exposure problem.
- The price is a *consequence* of the pool, so it cannot be quoted in advance.
  This directly conflicts with the requirement to record the exact odds a bet
  executed against, and with in-play pricing.
- Operationally the hardest to build correctly: late money, favourite-longshot
  bias, and pool state are all shared mutable state on the hot path.

### Option C — Peer-to-peer exchange *(chosen)*

Customers post **back** and **lay** limit orders. The matching engine pairs
compatible orders by **price-time priority** and creates matched bets between
two customers. The exchange is not the counterparty except for the residual
("odd lot") it must handle itself.

- Counterparty risk is mutual and bounded by a per-account exposure limit.
- No vig. The exchange earns **commission on net winnings**, which is a
  fundamentally different revenue calculation from margin.
- The hard problems are new and specific: price-time priority under contention,
  self-match prevention, partial fills, unmatched-order lifecycle, and
  cancelling a bet that has already matched.
- The 50ms budget is achievable, because matching is a single-writer,
  in-memory, price-ordered structure — but only if it is designed as one from
  the start rather than bolted onto a database.

## Decision

**Option C.** LadyWalk is a peer-to-peer betting exchange.

Consequences that follow directly and are treated as committed constraints:

1. **The client proposes the price; the matching engine decides the match.**
   This *inverts* the guarantee previously stated for a bookmaker. The invariant
   is not "the client may not choose an odds value" — it is: an order may only
   match at a price and size permitted by the tick-size ladder and the remaining
   exposure of *both* accounts, and the resulting matched bet is always
   reconstructible from the order book event log.
   ([Q19](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/19),
   [Q42](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/42)
   are rewritten by this decision.)

2. **A customer balance may go negative, bounded by an exposure limit.** The
   invariant is not "the balance is never negative" but "no account may exceed
   its configured exposure limit, and every decrement is evaluated inside the
   same atomic step as the order that causes it." The defects tracked as
   [Q10](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/10)
   (TOCTOU) and [Q45](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/45)
   (negative balance) are **retained but reframed**: the race is still a race,
   but the thing being protected is the *exposure limit*, not a non-negative
   balance.

3. **No vig, no pricing curve, no platform liability cap.**
   [Q16](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/16)
   (where the vig is applied), [Q17](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/17)
   (odds update curve, LMSR) and [Q18](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/18)
   (per-outcome platform liability cap) **do not apply to this product and are
   withdrawn.** Their epics and tasks are replaced, not deferred.

4. **Price-time priority is a correctness property, not a fairness nicety.** The
   order in which two equally-priced orders match is observable by customers and
   is a dispute surface. It is specified, tested, and auditable.

5. **A matched bet is between two customers and must be voidable as a pair.**
   Voids, suspensions and re-gradings apply to both legs or neither, and the
   settlement is always expressed as a pair of compensating movements.

## Consequences

**Becomes easier:**

- No pricing model to maintain, and no vig to explain to a customer.
- The 50ms budget is reachable with a single-writer in-memory book.
- Revenue is a clean function of net winnings, with no overround to police.
- Mutual counterparty risk is more visible than hidden platform risk.

**Becomes harder:**

- Every problem listed under Option C is new work that a bookmaker never has.
- The order book is a hot, stateful, single-writer component, and it is now the
  most correctness-critical object in the system.
- Users can be "short" — owed money — which inverts a lot of naive wallet
  intuition, including in the customer-facing ledger and reporting.
- A matched bet is no longer a single row; it is a relationship between two
  customers' positions, and voids, suspensions and chargebacks propagate.

**We accept these costs:**

- The system is harder to build and much harder to explain than a bookmaker.
- Empty markets are a real product problem, not just an operational one. An
  exchange with no liquidity is not a product, and this ADR does not solve
  bootstrapping liquidity.
- We accept a single-writer matching engine as a deliberate scaling ceiling. If
  it must be partitioned later, [Q08](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/08)
  (what serialises concurrent bets on a market) is reopened rather than
  re-decided.

**Revisit if:**

- Sustained order-entry rate on a single market exceeds **10,000 orders/second**
  for 15 consecutive minutes, such that the single-writer book is measurably the
  p99 bottleneck rather than the storage or network layer.
- Matching-engine p99 exceeds **50ms** at 5,000 orders/second on one market and
  does not improve with horizontal scaling, indicating the design ceiling has
  been reached.
- Regulatory review in a target jurisdiction requires the platform to be the
  counterparty, which would make this the wrong product in that market.
