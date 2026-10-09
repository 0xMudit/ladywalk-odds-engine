# Architecture decision records

An ADR captures one architectural decision, the alternatives that were
rejected, and — the part that matters most — the condition that should make
someone reopen it.

Two decisions are recorded. Both were taken together, because the second is
meaningless until the first exists: an exchange has to decide who settles
before it can decide how.

The rest of the committed constraints in
[`../architecture/README.md`](../architecture/README.md) are still written as if
they were decided, and each becomes an ADR when its question is answered.

## Process

1. **Claim the question.** Comment on the issue to claim it.
2. **Copy the template.** `0000-template.md` → `NNNN-kebab-title.md`, numbered
   sequentially from `0001`. Never reuse a number, and never renumber.
3. **Open a PR** with the ADR in `Proposed` status. It is reviewed like code,
   because a decision that constrains 200 issues is more expensive to change
   later than most code is to write.
4. **Merge to accept it.** Merging an ADR sets the status to `Accepted` and
   closes the issue it answers.
5. **Supersede, do not edit.** A merged ADR is immutable. To change a decision,
   write a new ADR that links to and quotes the one it replaces, and set the old
   one's status to `Superseded by ADR-NNNN`. History is the point; an ADR that
   can be rewritten proves nothing.

## Index

| ADR | Title | Status | Date |
| --- | --- | --- | --- |
| [0001](0001-peer-to-peer-exchange.md) | Peer-to-peer exchange with a price-time priority order book | Accepted | 2026-09-27 |
| [0002](0002-solana-settlement.md) | Solana for settlement, with the matching engine kept off-chain | Accepted | 2026-09-27 |

## Conventions

- One decision per file. If it needs an "and", it is two ADRs.
- The title is a statement, not a topic: `0001-represent-money-as-i64-minor-units`,
  not `0001-money`.
- **`Revisit if` is mandatory.** A decision without a measurable trigger for
  reversal is a decision that cannot be corrected. If you cannot state the
  trigger, you have not finished deciding.
- Record the option you rejected *and why*. Six months later, the rejected option
  is usually the one somebody proposes again, and the reason it lost is the only
  thing that stops the cycle.

## Candidates

The committed constraints in
[`../architecture/README.md`](../architecture/README.md) are written as if they
were already decided, and each will become an ADR when its question is answered:

| Constraint | Question |
| --- | --- |
| Money is never a float | [Q01](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/1) |
| The ledger is the source of truth, not the balance | [Q03](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/3) |
| Double-entry sums to zero with zero tolerance | [Q04](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/4) |
| Every state change is audited tamper-evidently | [Q47](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/47) |
| One switch stops all money movement | [Q35](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/35) |

One candidate was removed rather than deferred. "The server decides the price"
([Q19](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/19)) was a
bookmaker property, and [ADR-0001](0001-peer-to-peer-exchange.md) inverted it:
in an exchange the client proposes the price and the engine decides the match.
Writing an ADR for it would record the wrong invariant.
