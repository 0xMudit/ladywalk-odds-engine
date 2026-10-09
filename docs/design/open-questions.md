# Open design questions

> 66 questions a staff/principal engineer would force you to answer before signing off on the design. **One GitHub issue per question.** This file is the index; the issues are the work.

This is the **design track**. The implementation track is the
[roadmap](../roadmap.md), and the decisions that get made here land as ADRs in
[`../adr/`](../adr/).

[repo](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine) · [all 66 issues](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues) · [open](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues?q=is%3Aissue+is%3Aopen) · [closed](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues?q=is%3Aissue+is%3Aclosed) · [MVP milestone](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/milestone/1)

## How this works

A question is **not** answered because someone typed a paragraph. It is answered when the linked issue is closed with its acceptance criteria met and the artefact committed. A question whose answer is "we decided not to build this" is still answered — say so in the issue and close it.

| Label | Values | Meaning |
| --- | --- | --- |
| `type:` | `feat` · `bug` · `task` · `spike` | `feat` capability to build · `bug` defect class to eliminate · `task` process/ops/research · `spike` needs measurement before a decision |
| `priority:` | `critical` · `high` · `medium` · `low` | `critical` = money-correctness or regulatory safety · `high` = blocks a core guarantee |
| `area:` | subsystem labels | see the section headings below |

Milestones encode phase, not urgency:

| Milestone | Count | Meaning |
| --- | --- | --- |
| **MVP** | 41 | Needed before the platform can honestly be called shippable. |
| **Portfolio Depth** | 22 | Separates a working demo from a defensible design. Answer, partial implementations acceptable. |
| **Future Work** | 3 | Not needed yet. Document the design and the trigger. |

**Type split:** 42 `feat` · 15 `task` · 2 `bug` · 7 `spike`
**Priority split:** 21 `critical` · 22 `high` · 20 `medium` · 3 `low`

## Progress

**0 / 66 closed** (0%)

| # | Area | Issues | Closed |
| --: | --- | --: | --: |
| 1–7 | [Data Modeling & Correctness](#q01) | 7 | 0 |
| 8–14 | [Concurrency & Consistency](#q08) | 7 | 0 |
| 15–20 | [Betting Model](#q15) | 6 | 0 |
| 21–25 | [Real-Time Delivery](#q21) | 5 | 0 |
| 26–30 | [Settlement & Grading](#q26) | 5 | 0 |
| 31–35 | [Failure Modes & Reliability](#q31) | 5 | 0 |
| 36–41 | [Scalability & Architecture](#q36) | 6 | 0 |
| 42–47 | [Security](#q42) | 6 | 0 |
| 48–51 | [Observability](#q48) | 4 | 0 |
| 52–55 | [Testing](#q52) | 4 | 0 |
| 56–58 | [Deployment](#q56) | 3 | 0 |
| 59–66 | [Exchange Matching](#q59) | 8 | 0 |

---

## Data Modeling & Correctness

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [01](#q01) | `feat` | `critical` | MVP | [#1](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/1) | open |
| [02](#q02) | `feat` | `critical` | MVP | [#2](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/2) | open |
| [03](#q03) | `feat` | `critical` | MVP | [#3](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/3) | open |
| [04](#q04) | `feat` | `critical` | MVP | [#4](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/4) | open |
| [05](#q05) | `feat` | `high` | MVP | [#5](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/5) | open |
| [06](#q06) | `task` | `medium` | Portfolio Depth | [#6](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/6) | open |
| [07](#q07) | `feat` | `medium` | Portfolio Depth | [#7](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/7) | open |

<a id="q01"></a>

#### Q01 · Enforce integer-only money representation (no floats anywhere)

[Issue #1](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/1) · `feat` · `critical` · **MVP**

> Do I store money as integers (cents) or fixed-point decimals — and have I banned floats everywhere, including in the ORM layer?

**Why it matters.** A single float rounding error in a payout path is a financial defect that is invisible in unit tests and catastrophic in production. The ban has to be enforced mechanically, not by convention.

**Acceptance criteria**

- [ ] ADR `docs/adr/0001-money-representation.md` picks integer minor units (cents) over fixed-point decimal, with rationale
- [ ] `Money` value type in the domain layer: immutable, not constructible from float, explicit currency
- [ ] DB type is `BIGINT` minor units + `currency CHAR(3)`; no NUMERIC/FLOAT/REAL money columns
- [ ] CI lint gate fails the build on `float(`, `Float`, `REAL`, `DOUBLE PRECISION`, `NUMERIC` in money paths
- [ ] JSON boundary layer explicitly coerces or rejects float money input
- [ ] Unit tests for currency boundary behaviour (JPY zero-decimal, KWD three-decimal)

<a id="q02"></a>

#### Q02 · Define canonical market -> outcome -> bet -> settlement schema

[Issue #2](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/2) · `feat` · `critical` · **MVP**

> What's my canonical schema for `market -> outcome -> bet -> settlement` — and is it normalized enough to avoid update anomalies during grading?

**Why it matters.** Grading mutates market state. If bet and settlement facts live in the same row as mutable market state, a re-grade silently rewrites history.

**Acceptance criteria**

- [ ] ERD in `docs/schema.md` with a 3NF target and explicit denormalizations
- [ ] Migrations exist for all four entities with FKs and constraints
- [ ] Immutable vs mutable columns enumerated per table
- [ ] Written argument for each deliberate denormalization
- [ ] Constraint tests prove a re-grade cannot alter a settled bet's payout facts

<a id="q03"></a>

#### Q03 · Build immutable append-only double-entry ledger; derive balance

[Issue #3](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/3) · `feat` · `critical` · **MVP**

> Am I using a single mutable `balance` column, or a full immutable, append-only ledger (debits/credits) that balance is *derived* from?

**Why it matters.** A mutable balance column is unrecoverable history. Every dispute ('you took my money') becomes unfalsifiable.

**Acceptance criteria**

- [ ] `ledger_entries` is append-only: no UPDATE/DELETE path exists, enforced at the DB level
- [ ] Balance is a derived projection (materialized), never the source of truth
- [ ] Every entry carries account_id, direction, amount_minor, currency, posted_at, reference_type, reference_id
- [ ] Reversal is a new compensating entry, never a mutation
- [ ] Documented chart of accounts: user wallet, platform liability, platform revenue, margin, fees

<a id="q04"></a>

#### Q04 · Enforce and continuously test the double-entry sum-to-zero invariant

[Issue #4](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/4) · `feat` · `critical` · **MVP**

> If ledger-based: how do I guarantee the ledger always sums to zero across the system (double-entry invariant), and how do I test that invariant continuously?

**Why it matters.** Sum-to-zero is the single property that makes a ledger trustworthy. It must hold at transaction level, batch level, and in production continuously.

**Acceptance criteria**

- [ ] Transaction-level enforcement: postings cannot commit unbalanced (DB constraint or serializable transaction)
- [ ] Continuous per-account/per-currency invariant checker running in production
- [ ] CI property test: randomized entry sequences always sum to zero
- [ ] Alert fires on any imbalance within minutes (see #51)
- [ ] Documented recovery procedure when an imbalance is detected

<a id="q05"></a>

#### Q05 · Add immutable odds history with an as-placed execution record

[Issue #5](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/5) · `feat` · `high` · **MVP**

> How do I version odds over time — do I keep an immutable odds history table so I can prove what odds a user actually got, forever?

**Why it matters.** This is the regulatory and audit record. Without it, a dispute over what price was offered cannot be settled.

**Acceptance criteria**

- [ ] `odds_history` append-only, one row per odds version per outcome
- [ ] Each bet stores the exact `odds_version_id` it executed against
- [ ] Odds versions are never updated or deleted
- [ ] Retention and archival policy documented
- [ ] One-query answer to: 'what odds did user X receive at time T?'

<a id="q06"></a>

#### Q06 · ADR: as-of state reconstruction — event sourcing vs snapshot + delta

[Issue #6](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/6) · `task` · `medium` · **Portfolio Depth**

> What's my strategy for storing 'as of' state — can I reconstruct exactly what any market looked like at any past timestamp (event sourcing vs snapshot + delta)?

**Why it matters.** This determines the shape of every table and every future migration. Expensive to reverse once shipped.

**Acceptance criteria**

- [ ] ADR comparing both approaches against our actual read patterns
- [ ] Explicit statement of the reconstruction guarantee we are signing up for
- [ ] Prototype proving the chosen approach answers the as-of query
- [ ] Consequences for the schema in #2 documented

<a id="q07"></a>

#### Q07 · Model void / push / partial-void outcomes without corrupting settlement history

[Issue #7](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/7) · `feat` · `medium` · **Portfolio Depth**

> How do I model void/push/partial-void outcomes without corrupting historical settlement records?

**Why it matters.** Voids are the most common post-settlement event. Modelling them as an UPDATE destroys the record of what was originally paid.

**Acceptance criteria**

- [ ] Documented semantics for void / push / partial-void / cancelled
- [ ] Void implemented as a new state plus compensating ledger entries, never a mutation
- [ ] Original settlement record preserved and linked to its reversal
- [ ] Tests for every void variant, including partial on multi-outcome markets

## Concurrency & Consistency

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [08](#q08) | `feat` | `high` | MVP | [#8](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/8) | open |
| [09](#q09) | `feat` | `critical` | MVP | [#9](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/9) | open |
| [10](#q10) | `bug` | `critical` | MVP | [#10](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/10) | open |
| [11](#q11) | `spike` | `high` | MVP | [#11](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/11) | open |
| [12](#q12) | `task` | `low` | Future Work | [#12](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/12) | open |
| [13](#q13) | `feat` | `critical` | MVP | [#13](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/13) | open |
| [14](#q14) | `feat` | `high` | MVP | [#14](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/14) | open |

<a id="q08"></a>

#### Q08 · Choose and implement the per-market bet serialization strategy

[Issue #8](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/8) · `feat` · `high` · **MVP**

> When 10,000 users hit 'place bet' on the same market within 50ms of each other, what serializes them — DB row locks, optimistic concurrency with retry, or a single-writer queue per market?

**Why it matters.** This is the central concurrency decision. Everything in #9 through #14 depends on it.

**Acceptance criteria**

- [ ] ADR naming the chosen mechanism, with the trade-off against the other two written out
- [ ] Implementation with a documented per-market critical section boundary
- [ ] Documented ordering guarantee (FIFO? arrival at DB? arrival at gateway?)
- [ ] Demonstrated at 10k/50ms via the #52 load test
- [ ] Documented throughput ceiling and behaviour at saturation

<a id="q09"></a>

#### Q09 · Make bet placement + balance deduction a single ACID transaction

[Issue #9](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/9) · `feat` · `critical` · **MVP**

> Is bet placement + balance deduction wrapped in a single ACID transaction, or is it two-phase across services — and if the latter, what's my compensation/saga logic on partial failure?

**Why it matters.** Splitting these two writes is the largest single source of money-loss defects: bet accepted but not charged, or charged but no bet.

**Acceptance criteria**

- [ ] ADR: single ACID transaction as the default unless there is a stated reason otherwise
- [ ] Both writes in one transaction; isolation level named and justified
- [ ] If a saga is used: every step has a compensating action, and the compensation is itself tested
- [ ] No code path writes a bet without a corresponding ledger entry
- [ ] Failure-injection test proves no orphan bet and no orphan debit

<a id="q10"></a>

#### Q10 · Eliminate TOCTOU double-spend in balance check-then-deduct

[Issue #10](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/10) · `bug` · `critical` · **MVP**

> How do I prevent a double-spend where a user's balance check and balance deduction race across two concurrent requests (classic TOCTOU)?

**Why it matters.** This is a defect class, not a feature. It is exploitable by a user sending two concurrent requests and it is completely silent in manual testing.

**Acceptance criteria**

- [ ] Root cause written up: the check and the deduct are not one atomic step
- [ ] Deduction is conditional at the storage layer (`UPDATE ... WHERE balance >= amount`, or `SELECT FOR UPDATE`)
- [ ] Concurrency test fires N simultaneous debits and asserts the balance never goes negative
- [ ] Regression test added to CI, verified to fail against the pre-fix code

<a id="q11"></a>

#### Q11 · Benchmark SELECT FOR UPDATE vs advisory locks vs optimistic versioning on the wallet row

[Issue #11](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/11) · `spike` · `high` · **MVP**

> Am I using SELECT FOR UPDATE, advisory locks, or optimistic versioning (version column + compare-and-swap) on the wallet row — and what's the throughput ceiling of each?

**Why it matters.** The answer is an empirical number, not an opinion. Guessing here invalidates the entire capacity model.

**Acceptance criteria**

- [ ] Benchmark harness measuring all three at 1, 10, and 100 concurrent writers
- [ ] Reported throughput (ops/sec), p99 latency, and deadlock rate for each
- [ ] Written recommendation tied to the measured numbers
- [ ] Chosen option fed back into #8

<a id="q12"></a>

#### Q12 · ADR: wallet sharding and cross-shard transaction atomicity

[Issue #12](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/12) · `task` · `low` · **Future Work**

> If I shard the wallet table, how do I keep balance-deduction transactions atomic across shard boundaries (or do I ensure a user's wallet is always single-shard)?

**Why it matters.** Premature sharding is expensive. Sharding done wrong silently breaks money atomicity.

**Acceptance criteria**

- [ ] ADR on the sharding key (recommend: hash on user_id so a wallet is always single-shard)
- [ ] Documented cross-shard transaction strategy if sharding is ever adopted
- [ ] Explicit decision on whether to shard before reaching 100k users
- [ ] Migration path documented

<a id="q13"></a>

#### Q13 · Design and implement idempotency keys for bet placement and settlement

[Issue #13](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/13) · `feat` · `critical` · **MVP**

> What's my idempotency key design so a client retry (network blip) never double-places a bet or double-pays a settlement?

**Why it matters.** Client retries are not exceptional — mobile networks produce them constantly. Without idempotency, every retry is a silent double-charge.

**Acceptance criteria**

- [ ] `idempotency_key` unique constraint scoped per user per operation
- [ ] Key is a client-generated UUID; the server stores the full response and replays it on retry
- [ ] Concurrent duplicates with the same key return the same result, not a second bet
- [ ] Key retention longer than any client retry window
- [ ] The settlement payout path is idempotent too

<a id="q14"></a>

#### Q14 · Define last-odds / last-slot race semantics

[Issue #14](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/14) · `feat` · `high` · **MVP**

> How do I handle the 'last odds/last slot' race — first-writer-wins, queue-based FIFO, or do I reject and let the client re-quote?

**Why it matters.** In-play markets close while requests are in flight. The chosen semantics must be consistent and must not be gameable.

**Acceptance criteria**

- [ ] ADR naming the policy and the user-visible error
- [ ] Implementation: bet rejected with a distinct, documented error code on stale odds
- [ ] Client contract for re-quote defined and documented
- [ ] Race test: a bet arriving after market close is rejected, not silently accepted

## Betting Model

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [15](#q15) | `task` | `critical` | MVP | [#15](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/15) | open |
| [16](#q16) | `feat` | `high` | MVP | [#16](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/16) | open |
| [17](#q17) | `spike` | `high` | MVP | [#17](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/17) | open |
| [18](#q18) | `feat` | `high` | MVP | [#18](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/18) | open |
| [19](#q19) | `feat` | `critical` | MVP | [#19](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/19) | open |
| [20](#q20) | `feat` | `medium` | Portfolio Depth | [#20](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/20) | open |

<a id="q15"></a>

#### Q15 · ADR: choose fixed-odds, pari-mutuel, or exchange matching model

[Issue #15](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/15) · `task` · `critical` · **MVP**

> Is this fixed-odds, pari-mutuel pool, or exchange-style (peer matching) — and do I understand that these three have completely different concurrency and liability models?

**Why it matters.** The most consequential decision in the repository. Every other subsystem design is downstream of it.

**Acceptance criteria**

- [ ] ADR comparing all three on: concurrency model, liability model, capital requirement, regulatory surface, implementation cost
- [ ] Explicit choice with rationale
- [ ] Consequences enumerated for #16, #17, #18, and #38
- [ ] Documented regulatory/licensing implications (documented as research, not legal advice)

<a id="q16"></a>

#### Q16 · Decide and implement where the vig / margin is applied

[Issue #16](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/16) · `feat` · `high` · **MVP**

> Where does the vig/margin get baked in — at market creation, or dynamically as liability shifts?

**Why it matters.** Where the vig lives determines whether margin is a modelled number or an emergent result — and whether it is auditable after the fact.

**Acceptance criteria**

- [ ] ADR on static vs dynamic vig
- [ ] Vig applied in exactly one documented place, not scattered across the code
- [ ] Margin is computed, stored, and attributable per market
- [ ] Tests assert the expected platform margin for a known bet sequence

<a id="q17"></a>

#### Q17 · Evaluate the odds update function: linear vs logistic vs LMSR-style curve

[Issue #17](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/17) · `spike` · `high` · **MVP**

> If odds move based on bet volume, what's the update function — linear, logistic, or a proper market-maker curve (e.g., something LMSR-like)?

**Why it matters.** A naive update function produces pathological odds under load — exactly the moment correctness matters most.

**Acceptance criteria**

- [ ] All three candidates implemented
- [ ] Compared on: stability under concentrated volume, divergence, computability, auditability
- [ ] Written selection with a worked example
- [ ] Property tests: no combination of bets drives implied probability outside (0,1) or past the liability cap

<a id="q18"></a>

#### Q18 · Implement per-outcome platform liability cap and cap-breach behaviour

[Issue #18](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/18) · `feat` · `high` · **MVP**

> How do I cap my platform's liability on any single outcome, and what happens when a cap is hit mid-bet (reject, requeue, auto-hedge)?

**Why it matters.** The liability cap is the platform's hard financial backstop. Its breach behaviour must be defined, not emergent.

**Acceptance criteria**

- [ ] Per-outcome liability tracked in real time
- [ ] Cap-breach behaviour explicitly chosen and implemented (reject / reprice / halt)
- [ ] Breaches raise an alert, not just an error
- [ ] Test: betting up to and past the cap produces defined, asserted behaviour
- [ ] Runbook for a cap breach

<a id="q19"></a>

#### Q19 · Bind bet execution to a server-signed odds quote with slippage tolerance

[Issue #19](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/19) · `feat` · `critical` · **MVP**

> Do I snapshot the odds a user sees vs. the odds at execution time — and what's my tolerance/slippage policy if they differ?

**Why it matters.** The client must never be able to assert what price it saw (see #42). The server must decide, and the tolerance must be explicit.

**Acceptance criteria**

- [ ] Quote token issued by the server on odds fetch, signed and short-lived
- [ ] Bet execution validates the token and matches it to the current odds version
- [ ] Slippage tolerance defined numerically and enforced
- [ ] Rejection path returns a fresh quote so the client can retry
- [ ] Tests for: within tolerance, outside tolerance, expired token, forged token

<a id="q20"></a>

#### Q20 · Invalidate stale odds across all connected clients within a latency budget

[Issue #20](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/20) · `feat` · `medium` · **Portfolio Depth**

> How do I stale-invalidate odds across all connected clients within an acceptable latency budget?

**Why it matters.** A client holding a stale price is either about to place a rejected bet, or about to be arbitraged.

**Acceptance criteria**

- [ ] Staleness budget defined numerically
- [ ] Invalidations pushed (not polled) to every subscriber of a market
- [ ] Measured end-to-end propagation latency under load
- [ ] Alert if propagation exceeds the budget

## Real-Time Delivery

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [21](#q21) | `task` | `high` | MVP | [#21](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/21) | open |
| [22](#q22) | `feat` | `medium` | Portfolio Depth | [#22](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/22) | open |
| [23](#q23) | `feat` | `high` | MVP | [#23](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/23) | open |
| [24](#q24) | `feat` | `medium` | Portfolio Depth | [#24](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/24) | open |
| [25](#q25) | `feat` | `medium` | Portfolio Depth | [#25](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/25) | open |

<a id="q21"></a>

#### Q21 · ADR: WebSockets vs SSE vs long-polling, justified on reconnection semantics

[Issue #21](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/21) · `task` · `high` · **MVP**

> WebSockets, SSE, or long-polling — and can I justify the choice against reconnection semantics, not just 'WebSockets are cool'?

**Why it matters.** The transport choice constrains reconnection, resume, and proxy behaviour for the rest of the project's life.

**Acceptance criteria**

- [ ] ADR comparing all three on: reconnection, resume-with-token, proxy/load-balancer support, client complexity, battery cost
- [ ] Explicit choice with rationale
- [ ] Documented heartbeat and dead-connection detection

<a id="q22"></a>

#### Q22 · Fan out odds updates to 100K+ connections without per-server socket maps

[Issue #22](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/22) · `feat` · `medium` · **Portfolio Depth**

> How do you fan out odds updates to potentially 100K+ concurrent connections without every app server holding a giant socket map — Redis pub/sub, Kafka + a thin gateway layer, or a dedicated real-time service?

**Why it matters.** Every app server holding 100K sockets is both an OOM risk and a deploy-stall waiting problem.

**Acceptance criteria**

- [ ] ADR naming the fanout architecture
- [ ] Implementation with a thin, stateless gateway tier
- [ ] Measured: 100K concurrent connections on documented hardware
- [ ] Gateway deploys do not drop connections

<a id="q23"></a>

#### Q23 · Implement reconnect resync via snapshot + delta

[Issue #23](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/23) · `feat` · `high` · **MVP**

> On reconnect, how does a client resync to current state without replaying every event since disconnect (snapshot + delta vs full replay)?

**Why it matters.** Full replay is O(disconnect duration) and blows up on exactly the reconnect thundering herd it creates.

**Acceptance criteria**

- [ ] Client sends its last-seen sequence number
- [ ] Server returns snapshot + missed delta, or a fresh snapshot if the gap is too large
- [ ] Sequence numbers are gap-detectable, so a silently dropped message is detectable
- [ ] Test: client disconnects mid-event, reconnects, state matches exactly

<a id="q24"></a>

#### Q24 · Handle backpressure for slow clients during high-velocity in-play updates

[Issue #24](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/24) · `feat` · `medium` · **Portfolio Depth**

> How do you handle backpressure when a slow client can't keep up with odds update velocity during a live in-play event?

**Why it matters.** One slow client on an unbounded per-connection queue is a memory leak that takes down the node.

**Acceptance criteria**

- [ ] Bounded per-connection queue with a defined overflow policy (drop and force resync)
- [ ] Slow-consumer detection and disconnection
- [ ] Explicitly document and test that odds are snapshotted state, so dropping intermediate updates is safe
- [ ] Memory ceiling per connection measured

<a id="q25"></a>

#### Q25 · Design hot-market fanout for a 50K-socket thundering herd

[Issue #25](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/25) · `feat` · `medium` · **Portfolio Depth**

> What's my fanout architecture if one market suddenly needs to push to 50K sockets simultaneously (a 'hot market' thundering herd)?

**Why it matters.** Popularity is spiky and correlated. Capacity planning against average traffic does not survive a final.

**Acceptance criteria**

- [ ] Documented hot-market strategy (subscription-based routing, partitioned fanout, or a dedicated channel)
- [ ] Measured: single market pushed to 50K subscribers
- [ ] Other markets' update latency unaffected by a hot market
- [ ] Runbook for a hot market

## Settlement & Grading

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [26](#q26) | `feat` | `high` | MVP | [#26](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/26) | open |
| [27](#q27) | `feat` | `critical` | MVP | [#27](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/27) | open |
| [28](#q28) | `feat` | `critical` | MVP | [#28](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/28) | open |
| [29](#q29) | `task` | `medium` | Portfolio Depth | [#29](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/29) | open |
| [30](#q30) | `feat` | `critical` | MVP | [#30](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/30) | open |

<a id="q26"></a>

#### Q26 · Implement the grading pipeline with human override and override audit trail

[Issue #26](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/26) · `feat` · `high` · **MVP**

> Is grading manual, automated via a results feed, or hybrid with human override — and what's the audit trail on overrides?

**Why it matters.** Overrides are the highest-risk admin action in the system. Unattributable overrides are unacceptable.

**Acceptance criteria**

- [ ] Hybrid model: automated feed with a gated human override
- [ ] Every override records actor, timestamp, reason, and prior value
- [ ] Overrides on an already-settled market require a distinct elevated permission
- [ ] Audit log is append-only (see #47)
- [ ] Four-eyes or dual approval for high-value overrides

<a id="q27"></a>

#### Q27 · Guarantee exactly-once payout execution across settlement job crashes

[Issue #27](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/27) · `feat` · `critical` · **MVP**

> What's your exactly-once guarantee on payout execution — how do you ensure a settlement job that crashes mid-run doesn't double-pay or skip users?

**Why it matters.** Exactly-once on money movement is a hard requirement. At-least-once double-pays; at-most-once skips users.

**Acceptance criteria**

- [ ] Idempotent payout with a stable per-bet payout key
- [ ] Atomic ledger write per bet, so a crash cannot half-apply a payout
- [ ] The job is resumable from where it stopped (see #28)
- [ ] Crash-injection test proves no double-pay and no skip
- [ ] Written statement of the exactly-once guarantee and its scope

<a id="q28"></a>

#### Q28 · Make settlement idempotent and resumable with per-batch checkpointing

[Issue #28](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/28) · `feat` · `critical` · **MVP**

> How do you make settlement idempotent and resumable (checkpointing which bets have been paid within a batch)?

**Why it matters.** A settlement run over a large market can take minutes. Without checkpointing, every failure restarts the whole thing.

**Acceptance criteria**

- [ ] Per-bet settlement status (`pending` / `paid` / `failed`) with retry
- [ ] Checkpoint written in the same transaction as the payout
- [ ] Resume skips completed bets without re-paying
- [ ] Failed bets retried with backoff, then quarantined — never silently dropped
- [ ] Test: kill the job at 30% and 70%, resume, assert exactly one payout per bet

<a id="q29"></a>

#### Q29 · ADR: wrong results feed after payout — reversal, clawback, and legality

[Issue #29](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/29) · `task` · `medium` · **Portfolio Depth**

> What happens if a results feed reports a wrong outcome and I've already paid out — do I have a reversal/clawback mechanism, and is it even legal/expected to claw back?

**Why it matters.** A product and legal decision with real money attached. It must be made before settlement ships, not after the first bad feed.

**Acceptance criteria**

- [ ] ADR covering: automatic reversal, manual clawback, or no-reversal-and-eat-the-loss
- [ ] Research on user-facing terms for the chosen policy (cited, not legal advice)
- [ ] Reversal implemented as compensating ledger entries, never a mutation (see #3)
- [ ] User notification path defined
- [ ] Blast radius estimate: worst-case total exposure from a single wrong settlement

<a id="q30"></a>

#### Q30 · Enforce 'total paid out vs total staked minus margin' as a hard post-settlement invariant

[Issue #30](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/30) · `feat` · `critical` · **MVP**

> How do you reconcile 'total paid out' against 'total staked minus margin' as a hard invariant check after every settlement run?

**Why it matters.** This is the end-to-end financial correctness check. If it fails, real money is unaccounted for.

**Acceptance criteria**

- [ ] Reconciliation asserted after every settlement run, not sampled
- [ ] A mismatch fails the settlement run loudly — no silent continuation
- [ ] Reconciliation is part of the settlement job, not a separate report
- [ ] Tolerance is explicitly zero for the core identity; any rounding policy documented separately
- [ ] A mismatch page includes the account-level breakdown

## Failure Modes & Reliability

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [31](#q31) | `task` | `high` | MVP | [#31](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/31) | open |
| [32](#q32) | `feat` | `medium` | Portfolio Depth | [#32](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/32) | open |
| [33](#q33) | `feat` | `medium` | Portfolio Depth | [#33](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/33) | open |
| [34](#q34) | `task` | `medium` | Portfolio Depth | [#34](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/34) | open |
| [35](#q35) | `feat` | `high` | MVP | [#35](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/35) | open |

<a id="q31"></a>

#### Q31 · ADR: fail safe (halt) vs fail open (stale odds) when the pipeline dies

[Issue #31](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/31) · `task` · `high` · **MVP**

> If my odds-update pipeline dies mid-event, does the system fail safe (halt betting on that market) or fail open (accept bets on stale odds)? Which have I chosen and why?

**Why it matters.** This decision trades revenue for financial safety. It must be explicit and enforced in code, not by hope.

**Acceptance criteria**

- [ ] ADR with an explicit choice (recommend: fail safe — halt betting on the affected market)
- [ ] Implemented: a stale odds pipeline marks the market un-bettable
- [ ] `bettable` flag derived from a health signal with a defined TTL
- [ ] Test: kill the pipeline mid-event, assert new bets are rejected
- [ ] Documented user-facing behaviour and auto-recovery path

<a id="q32"></a>

#### Q32 · Implement a circuit breaker for upstream results and odds feeds

[Issue #32](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/32) · `feat` · `medium` · **Portfolio Depth**

> What's your circuit breaker strategy if the upstream results/odds feed goes down or starts returning garbage?

**Why it matters.** A feed returning plausible-but-wrong data is more dangerous than one returning nothing. Down is easy to detect; garbage is not.

**Acceptance criteria**

- [ ] Circuit breaker on every external feed: closed / open / half-open
- [ ] Garbage detection beyond availability: schema validation, sanity ranges, plausibility checks
- [ ] An open breaker halts the dependent flow (grading or odds) rather than passing data through
- [ ] Metrics on breaker state and transition count
- [ ] Alert on breaker open

<a id="q33"></a>

#### Q33 · Detect and quarantine a poisoned market before it causes financial damage

[Issue #33](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/33) · `feat` · `medium` · **Portfolio Depth**

> How do you detect and quarantine a 'poisoned' market (bad data corrupting odds) before it causes financial damage?

**Why it matters.** Corrupted odds propagate at fanout speed. By the time you notice, the exposure is already real.

**Acceptance criteria**

- [ ] Plausibility invariants on odds (implied probability in (0,1), non-negative, monotonic where expected)
- [ ] A breach marks the market quarantined and blocks new bets
- [ ] Quarantine is automatic, with a human path to release
- [ ] Blast radius measured: how far can bad odds travel before detection
- [ ] Runbook for a quarantined market

<a id="q34"></a>

#### Q34 · Define DR RPO/RTO for the wallet ledger specifically

[Issue #34](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/34) · `task` · `medium` · **Portfolio Depth**

> What's your disaster recovery RPO/RTO for the wallet ledger specifically, separate from the rest of the system?

**Why it matters.** Losing bet and odds state is an outage. Losing the ledger is unrecoverable financial loss. Different answers are required.

**Acceptance criteria**

- [ ] Documented RPO and RTO for the ledger, distinct from application state
- [ ] Ledger durability design: synchronous replication, append-only log, and/or backups
- [ ] Restore actually tested, with a recorded drill result and timing
- [ ] Documented proof that the ledger is reconstructible from backups alone
- [ ] Cost of the chosen durability posture stated

<a id="q35"></a>

#### Q35 · Implement a platform-wide betting kill switch with defined trigger authority

[Issue #35](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/35) · `feat` · `high` · **MVP**

> Do you have a kill switch to freeze all betting platform-wide within seconds, and who/what can trigger it?

**Why it matters.** During a live incident the only acceptable response is a single action that stops all money movement immediately.

**Acceptance criteria**

- [ ] Kill switch halts bet acceptance platform-wide within a stated number of seconds
- [ ] Trigger authority defined: which roles, which automated conditions
- [ ] Grading and settlement separately controllable
- [ ] The kill switch action itself is audited (who pressed it, when)
- [ ] Regular drill to prove it works — record the measured time-to-halt
- [ ] Documented reset procedure

## Scalability & Architecture

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [36](#q36) | `task` | `high` | MVP | [#36](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/36) | open |
| [37](#q37) | `task` | `high` | MVP | [#37](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/37) | open |
| [38](#q38) | `feat` | `medium` | Portfolio Depth | [#38](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/38) | open |
| [39](#q39) | `feat` | `medium` | Portfolio Depth | [#39](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/39) | open |
| [40](#q40) | `feat` | `low` | Future Work | [#40](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/40) | open |
| [41](#q41) | `feat` | `medium` | Portfolio Depth | [#41](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/41) | open |

<a id="q36"></a>

#### Q36 · ADR: monolith vs services, and failure-domain separation

[Issue #36](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/36) · `task` · `high` · **MVP**

> Monolith vs microservices — and specifically, do wallet and bet-placement need to be separate failure domains from odds and market-data?

**Why it matters.** The minimum defensible split is the money path. Everything else can be a module.

**Acceptance criteria**

- [ ] ADR with the failure-domain requirement stated first and service boundaries second
- [ ] Explicit answer on whether wallet is separated from odds/market-data
- [ ] Rationale against premature microservices
- [ ] Document which component can be down without blocking bet acceptance

<a id="q37"></a>

#### Q37 · ADR: sync REST vs event-driven bet placement, with a p99 latency budget

[Issue #37](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/37) · `task` · `high` · **MVP**

> Sync REST for bet placement vs event-driven (Kafka) — what's my p99 latency budget for 'bet accepted' and does my choice hit it?

**Why it matters.** 'Bet accepted' is the user-facing promise. Without a number there is no SLO and no way to know if the choice works.

**Acceptance criteria**

- [ ] p99 latency budget for 'bet accepted' stated as a number
- [ ] ADR comparing sync vs event-driven against that budget
- [ ] Explicit choice
- [ ] Measured p99 meets the budget — report the number
- [ ] Feedback the number into #48 SLIs

<a id="q38"></a>

#### Q38 · Horizontally scale bet placement while preserving per-market serialization

[Issue #38](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/38) · `feat` · `medium` · **Portfolio Depth**

> How do I horizontally scale the bet-placement path without breaking the per-market serialization I designed in question 8?

**Why it matters.** Serialization and horizontal scale pull directly against each other. This is where that tension gets resolved.

**Acceptance criteria**

- [ ] Scaling strategy that preserves the #8 serialization guarantee
- [ ] Documented partitioning: which key is the serialization unit and how it maps to instances
- [ ] Rebalancing does not violate the guarantee — documented and tested
- [ ] Test: multi-instance concurrent placement on one market still serializes correctly

<a id="q39"></a>

#### Q39 · Define the read/write split for live odds reads

[Issue #39](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/39) · `feat` · `medium` · **Portfolio Depth**

> What's your read/write split — do live odds reads hit a read replica or Redis cache, while writes go to primary Postgres?

**Why it matters.** Read-path choice determines staleness tolerance, which determines whether read-your-writes is even possible.

**Acceptance criteria**

- [ ] ADR: replica vs cache vs primary for each read class
- [ ] Staleness budget stated per read class
- [ ] Cache invalidation on odds write, not TTL-only
- [ ] Bet placement reads the authoritative value, never a cached one
- [ ] Measured read latency and cache hit rate

<a id="q40"></a>

#### Q40 · Handle hot-partition load when one market takes 100x all other traffic

[Issue #40](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/40) · `feat` · `low` · **Future Work**

> How do you handle hot-partition problems when one market (e.g., a major event) gets 100x the traffic of everything else combined?

**Why it matters.** Partitioning by user or by time both put a popular market in one place. This is the case that breaks naive designs.

**Acceptance criteria**

- [ ] Documented partition strategy and why it survives a hot market
- [ ] Mitigation: per-market rate limits, dedicated capacity, or queue isolation
- [ ] Load-tested with one market at 100x baseline
- [ ] Degradation behaviour defined (fail, queue, or shed)

<a id="q41"></a>

#### Q41 · Implement per-market rate limiting independent of per-user limits

[Issue #41](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/41) · `feat` · `medium` · **Portfolio Depth**

> Do you need per-market rate limiting/sharding independent of per-user rate limiting?

**Why it matters.** Per-user limits do not protect a market. A thousand legitimate users can still overwhelm one market.

**Acceptance criteria**

- [ ] Both per-user and per-market limiters implemented
- [ ] Algorithm chosen and stated (token bucket or sliding window)
- [ ] Limits configurable per market — a low-liquidity market needs tighter limits
- [ ] Rate-limit responses distinguishable from validation errors
- [ ] Test: the per-market limit trips before the per-user limit does

## Security

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [42](#q42) | `feat` | `critical` | MVP | [#42](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/42) | open |
| [43](#q43) | `feat` | `medium` | Portfolio Depth | [#43](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/43) | open |
| [44](#q44) | `feat` | `high` | MVP | [#44](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/44) | open |
| [45](#q45) | `bug` | `critical` | MVP | [#45](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/45) | open |
| [46](#q46) | `feat` | `high` | MVP | [#46](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/46) | open |
| [47](#q47) | `feat` | `critical` | MVP | [#47](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/47) | open |

<a id="q42"></a>

#### Q42 · Reject client-asserted odds; require a server-signed quote for execution

[Issue #42](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/42) · `feat` · `critical` · **MVP**

> How do I prevent a client from spoofing the odds it 'saw' to force a favorable bet execution?

**Why it matters.** If the client can assert its price, there is no odds engine — just a client-controlled payout.

**Acceptance criteria**

- [ ] The server never trusts a client-supplied odds value
- [ ] Execution requires a server-signed, short-lived quote token
- [ ] Forged, expired, and tampered tokens all rejected
- [ ] Tamper attempts logged as security events
- [ ] Penetration test covering price spoofing

<a id="q43"></a>

#### Q43 · Defend bet placement against scripted clients and staleness arbitrage

[Issue #43](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/43) · `feat` · `medium` · **Portfolio Depth**

> What's your defense against a scripted client hammering the bet-placement endpoint faster than a human (bot arbitrage across odds staleness windows)?

**Why it matters.** Bots are faster and more patient. They specifically target the gap between a client's stale view and the server's current odds.

**Acceptance criteria**

- [ ] Bot heuristics: velocity, inter-arrival distribution, market-hopping behaviour
- [ ] The staleness window from #19/#20 measured and confirmed too small to be profitable
- [ ] Per-market and per-user limits (#41) as the first line of defence
- [ ] Detected bots: challenged, rate-limited, or blocked — behaviour defined
- [ ] Detection metrics and alerting

<a id="q44"></a>

#### Q44 · Authenticate and audit the admin market-creation and grading panel

[Issue #44](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/44) · `feat` · `high` · **MVP**

> How is the admin market-creation/grading panel authenticated and audited — can I prove who graded what and when?

**Why it matters.** Admin access is the single largest concentration of risk in the system.

**Acceptance criteria**

- [ ] Auth: SSO or MFA-required, no password-only admin accounts
- [ ] Least-privilege roles; grading and market creation are separate permissions
- [ ] Every admin action audited with actor, timestamp, and before/after values
- [ ] Session logging and periodic access review
- [ ] No admin action can mutate a settled payout directly (see #3, #47)

<a id="q45"></a>

#### Q45 · Fix the negative-balance exploit on the deduct path — validate and deduct atomically

[Issue #45](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/45) · `bug` · `critical` · **MVP**

> How do I prevent negative-balance exploits via race conditions on the deduct-then-validate path (should be validate-then-deduct, atomically)?

**Why it matters.** A defect class, not a feature. Reachable by any user with two concurrent requests, and completely silent in manual testing.

**Acceptance criteria**

- [ ] Root cause written up: validate and deduct are not one atomic step
- [ ] Conditional deduct at the storage layer — never read-then-write
- [ ] Concurrency test: N simultaneous debits exceeding the balance, asserting exactly one succeeds
- [ ] Balance is never observably negative, even transiently
- [ ] Regression test in CI, verified to fail against pre-fix code

<a id="q46"></a>

#### Q46 · Add replay protection to bet submission: nonces and signed timestamps

[Issue #46](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/46) · `feat` · `high` · **MVP**

> What's my replay-attack protection on the bet-submission API (nonces, signed timestamps)?

**Why it matters.** Without replay protection a captured valid request can be replayed indefinitely. This is independent of the odds-spoofing risk in #42.

**Acceptance criteria**

- [ ] Single-use nonce per request, enforced server-side
- [ ] Timestamped request with a bounded acceptance window
- [ ] Request signature (HMAC or asymmetric) verified before processing
- [ ] Replay attempts rejected and logged as security events
- [ ] Test: verbatim replay of a captured request is rejected

<a id="q47"></a>

#### Q47 · Log every state-changing action to an immutable, tamper-evident audit log

[Issue #47](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/47) · `feat` · `critical` · **MVP**

> Am I logging every state-changing action (bet placed, market graded, balance changed) to an immutable, tamper-evident audit log?

**Why it matters.** 'Immutable' and 'tamper-evident' are different guarantees. Ordinary application logs provide neither.

**Acceptance criteria**

- [ ] Every state change produces an audit record: actor, action, subject, before, after, timestamp
- [ ] Log is append-only and tamper-evident (hash chain or equivalent)
- [ ] Retention exceeds the regulatory dispute window
- [ ] The log is not writable by the application role
- [ ] A verification job proves the chain is intact, and alerts on a break

## Observability

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [48](#q48) | `task` | `high` | MVP | [#48](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/48) | open |
| [49](#q49) | `feat` | `medium` | Portfolio Depth | [#49](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/49) | open |
| [50](#q50) | `feat` | `medium` | Portfolio Depth | [#50](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/50) | open |
| [51](#q51) | `feat` | `critical` | MVP | [#51](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/51) | open |

<a id="q48"></a>

#### Q48 · Define SLIs and SLOs for bet placement, odds propagation, and settlement

[Issue #48](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/48) · `task` · `high` · **MVP**

> What are your key SLIs — bet-placement latency, odds-propagation latency, settlement completion time — and what are the SLOs?

**Why it matters.** No SLO means no definition of 'broken', and no basis for prioritising engineering work.

**Acceptance criteria**

- [ ] SLIs defined for at least: bet-accept latency, odds-propagation latency, settlement completion, rejection rate
- [ ] SLO targets with error budgets stated numerically
- [ ] Document what 'bet accepted' means — the promise made to the user
- [ ] SLI instrumentation in place
- [ ] Alerting thresholds derived from the error budget

<a id="q49"></a>

#### Q49 · Run property-based invariant checks in production to catch silent settlement-math bugs

[Issue #49](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/49) · `feat` · `medium` · **Portfolio Depth**

> How do you detect a 'silent' bug where settlement math is subtly wrong but no error is thrown (property-based invariant checks running in prod)?

**Why it matters.** Wrong money maths typically does not throw. It returns a plausible number. Only an invariant catches it.

**Acceptance criteria**

- [ ] Production invariant checks: per-bet payout matches independent recomputation; market totals reconcile (#30)
- [ ] Sampled verification with a documented sampling rate and justification
- [ ] Any invariant breach pages — it does not log and continue
- [ ] The same invariants run on 100% of settlements in CI
- [ ] Precedent: a deliberately injected wrong-math bug is caught by the checker

<a id="q50"></a>

#### Q50 · Build a real-time platform liability exposure dashboard per market

[Issue #50](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/50) · `feat` · `medium` · **Portfolio Depth**

> Do you have real-time dashboards for platform liability exposure per market, so I'd notice before a $1M loss, not after?

**Why it matters.** Exposure is knowable in real time. If it is not visible, it gets discovered by the ledger.

**Acceptance criteria**

- [ ] Live per-market liability, expressed as worst-case payout if each outcome wins
- [ ] Portfolio-level total exposure
- [ ] Trend over time, not just a current number
- [ ] Approaching-cap warnings before the cap is hit (see #18)
- [ ] Alerting at configurable exposure thresholds

<a id="q51"></a>

#### Q51 · Alert on ledger imbalance with a defined detection threshold

[Issue #51](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/51) · `feat` · `critical` · **MVP**

> What's your alerting threshold for ledger imbalance (sum of all debits != sum of all credits)?

**Why it matters.** The correct threshold is zero. Anything else is a decision to tolerate known corruption, and it must be a conscious one.

**Acceptance criteria**

- [ ] Imbalance check runs continuously (see #4) and alerts at zero tolerance
- [ ] Alert includes the account-level breakdown and the implicated transaction
- [ ] Pages — not just a dashboard — on imbalance
- [ ] Runbook for a confirmed imbalance
- [ ] Drill: inject an imbalance in staging and verify the alert fires

## Testing

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [52](#q52) | `task` | `medium` | Portfolio Depth | [#52](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/52) | open |
| [53](#q53) | `task` | `high` | MVP | [#53](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/53) | open |
| [54](#q54) | `task` | `high` | MVP | [#54](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/54) | open |
| [55](#q55) | `task` | `low` | Future Work | [#55](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/55) | open |

<a id="q52"></a>

#### Q52 · Load-test concurrent bet placement to surface races before users do

[Issue #52](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/52) · `task` · `medium` · **Portfolio Depth**

> How do you load-test concurrent bet placement to surface the race conditions from question 8-14 before users find them?

**Why it matters.** Concurrency defects only appear under contention. Sequential tests will never find them, and neither will manual QA.

**Acceptance criteria**

- [ ] Load-test harness: 10,000 placements on one market within 50ms (the #8 scenario)
- [ ] Tests for: no double-spend (#10), no TOCTOU (#45), idempotency under retry (#13), correct last-slot rejection (#14)
- [ ] A sustained soak test, not just a spike
- [ ] Reproducible and wired into CI
- [ ] Results reported against the measured throughput ceiling from #11

<a id="q53"></a>

#### Q53 · Add property-based tests fuzzing payout maths with random bet/settlement sequences

[Issue #53](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/53) · `task` · `high` · **MVP**

> Do you have property-based tests generating random bet/settlement sequences to fuzz for payout-math bugs?

**Why it matters.** Example-based tests only cover the cases their author imagined. Randomised sequences find the rest.

**Acceptance criteria**

- [ ] Property-based framework integrated (Hypothesis, fast-check, or equivalent)
- [ ] Properties asserted: sum of payouts = stake - margin; balance never negative; ledger sums to zero; re-grading is idempotent
- [ ] Void / push / partial-void included in the generated sequence space
- [ ] Seeded failures are reproducible and shrink to a minimal case
- [ ] Runs in CI on every PR

<a id="q54"></a>

#### Q54 · Test settlement idempotency by deliberately crashing the job mid-batch

[Issue #54](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/54) · `task` · `high` · **MVP**

> How do you test settlement idempotency — deliberately crash the job mid-batch in a test and assert no double-pay?

**Why it matters.** The crash path is the path nobody runs. It has to be tested on purpose, not hoped about.

**Acceptance criteria**

- [ ] Test harness that kills the settlement job at controlled points (30%, 70%, 99%)
- [ ] After each kill + resume: assert exactly one payout per bet, and every bet eventually paid
- [ ] Concurrent duplicate settlement runs assert no double-pay
- [ ] Failure injection at the DB level (connection drop mid-transaction), not just process kill
- [ ] Test runs in CI on every PR

<a id="q55"></a>

#### Q55 · Chaos-engineer the real-time layer: kill a pub/sub node mid-event

[Issue #55](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/55) · `task` · `low` · **Future Work**

> What's your chaos-engineering plan for the real-time layer (kill a pub/sub node mid-event, verify clients resync correctly)?

**Why it matters.** The resync path (#23) is only ever exercised in production unless it is deliberately broken in staging.

**Acceptance criteria**

- [ ] Documented chaos scenarios: broker kill, gateway kill, network partition, duplicate delivery
- [ ] Automated chaos experiments in staging
- [ ] Expected client behaviour asserted per scenario (resync, no missed update)
- [ ] Blameless post-mortem template
- [ ] Guardrails so chaos never runs against production

## Deployment

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [56](#q56) | `feat` | `medium` | Portfolio Depth | [#56](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/56) | open |
| [57](#q57) | `task` | `medium` | Portfolio Depth | [#57](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/57) | open |
| [58](#q58) | `feat` | `medium` | Portfolio Depth | [#58](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/58) | open |

<a id="q56"></a>

#### Q56 · Guarantee zero-downtime deploys do not drop in-flight bet placement

[Issue #56](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/56) · `feat` · `medium` · **Portfolio Depth**

> How do you guarantee zero-downtime deploys don't drop in-flight bet-placement transactions?

**Why it matters.** Deploys are the most common self-inflicted outage in a system like this, and the fix is architectural rather than procedural.

**Acceptance criteria**

- [ ] Connection draining: in-flight requests complete before the instance exits
- [ ] Graceful shutdown with a bounded drain timeout, then forced exit
- [ ] Deployment window documented — ideally none
- [ ] Rolling deploy verified by placing bets continuously across a deploy in staging
- [ ] In-flight bet placement monitored during deploys

<a id="q57"></a>

#### Q57 · Stand up staging with production-like concurrency load

[Issue #57](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/57) · `task` · `medium` · **Portfolio Depth**

> Do you have a staging environment with production-like concurrency load to catch race conditions before they ship?

**Why it matters.** Race conditions need load to appear. A low-traffic staging environment tells you nothing about #8 through #14.

**Acceptance criteria**

- [ ] Staging runs a load profile matching production concurrency, not a smoke test
- [ ] Same code path, same isolation levels, same infrastructure topology as production
- [ ] Synthetic load generator running continuously against staging
- [ ] Staging can be reset to a known state cheaply
- [ ] The race-condition suite (#52, #53, #54) runs against staging, not only unit-level in CI

<a id="q58"></a>

#### Q58 · Version the odds and settlement calculation logic for auditable payouts

[Issue #58](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/58) · `feat` · `medium` · **Portfolio Depth**

> How do you version your odds/settlement calculation logic so I can prove *which version* computed a given historical payout?

**Why it matters.** Logic changes over time. Without versioning, a historical payout cannot be reproduced or defended.

**Acceptance criteria**

- [ ] Every odds version and every settlement carries the `calc_version` that produced it
- [ ] Old calculation versions remain runnable so a historical payout can be reproduced exactly
- [ ] Regression suite runs every historical calc version against golden cases
- [ ] ADR on the versioning scheme (semver vs monotonic integer vs git SHA)
- [ ] Demonstrate reproducing a specific historical payout byte-for-byte

## Exchange Matching

| Q | Type | Priority | Phase | Issue | Status |
| --: | --- | --- | --- | --- | --- |
| [59](#q59) | `spike` | `critical` | MVP | [#330](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/330) | open |
| [60](#q60) | `spike` | `critical` | MVP | [#331](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/331) | open |
| [61](#q61) | `feat` | `critical` | MVP | [#332](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/332) | open |
| [62](#q62) | `spike` | `critical` | MVP | [#333](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/333) | open |
| [63](#q63) | `spike` | `critical` | MVP | [#334](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/334) | open |
| [64](#q64) | `feat` | `high` | MVP | [#335](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/335) | open |
| [65](#q65) | `feat` | `high` | Portfolio Depth | [#336](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/336) | open |
| [66](#q66) | `spike` | `high` | Portfolio Depth | [#337](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/337) | open |

<a id="q59"></a>

#### Q59 · Define the exposure limit per account, and what it applies to

[Issue #330](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/330) · `spike` · `critical` · **MVP**

> What is the per-account exposure limit — is it a fixed amount, a fraction of the account's settled balance, or a market-specific number — and does it cover unmatched back bets, lay liability, or both?

**Why it matters.** An unmatched back bet makes the customer a debtor. Without an explicit bound, the platform is the counterparty by accident, and the 'platform liability cap' that used to protect it is gone now that there is no vig model to size it against. This limit is what makes 'bounded negative customer exposure' a real bound rather than a promise.

**Acceptance criteria**

- [ ] Written policy: the exact limit, what it is measured against, and when it is re-evaluated
- [ ] Explicit answer on whether exposure is per-bet, per-market, per-outcome, or account-wide, with the reasoning for that choice
- [ ] Decision on whether a lay bet's potential liability counts against the same limit as an unmatched back stake
- [ ] Decision on who can raise a customer's limit, and whether it is audited and time-boxed
- [ ] Property test: no sequence of accepted orders can drive an account past its limit, including under concurrent submission
- [ ] Chosen limit fed back into #8 and the order-book design

<a id="q60"></a>

#### Q60 · Decide the self-match prevention policy

[Issue #331](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/331) · `spike` · `critical` · **MVP**

> Can a customer's own back and lay orders match each other, and if not, at what point is the match rejected — at submission, at match time, or by cancelling afterwards?

**Why it matters.** Self-matching is usually a mistake and occasionally fraud: it moves value between a customer's own accounts to fabricate turnover. The rule must be stated before the matching engine exists, because retrofitting it means re-deriving every historical match.

**Acceptance criteria**

- [ ] Written policy: self-matching is rejected, permitted, or permitted-then-cancelled, and the exact matching rule for accounts that share an owner
- [ ] Definition of 'the same customer' — one account, a household, a common beneficial owner — with the authority used to detect it
- [ ] Decision on pre-existing exposure created before a relationship is detected, and whether the pair is unwound or held
- [ ] Property test: for any order sequence, self-matched exposure is always either rejected or fully unwound, never left in place
- [ ] Audit log records the self-match check outcome for every match attempt

<a id="q61"></a>

#### Q61 · Specify order lifecycle: partial fill, remainder, cancel-after-match

[Issue #332](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/332) · `feat` · `critical` · **MVP**

> What are the legal states an order moves through — and specifically, what happens to the remainder when a book is only partially filled, and may a customer cancel an order that has already matched in part?

**Why it matters.** A bookmaker's bet is atomic: it is placed or it is not. An exchange order is not. Partial fills create a remainder that someone is exposed on and nobody has decided what should happen to, and cancel-after-match is a genuine dispute surface that has to be answered before customers rely on it.

**Acceptance criteria**

- [ ] State diagram for order states with every legal and illegal transition enumerated
- [ ] Explicit decision on whether the remainder stays live in the book or is cancelled when the book is exhausted
- [ ] Answer on cancel-after-match: does it cancel the remainder, refuse entirely, or partially fill first
- [ ] Answer on whether a matched bet can be cancelled by the customer, by the exchange, or by neither
- [ ] Invariant: the sum of every fill, the remainder, and any released exposure always equals the original order size, proven by property test
- [ ] Every transition is an audited ledger event, not a memory write

<a id="q62"></a>

#### Q62 · Design the commission model on net winnings

[Issue #333](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/333) · `spike` · `critical` · **MVP**

> How is commission calculated on net winnings — per bet, per market per period, on gross winnings or on net profit — and how is it refunded when a bet is voided or a market is re-graded?

**Why it matters.** There is no vig, so this replaces the entire revenue calculation the old design had. Charging on gross rather than net means a customer pays commission on money they never won, which is the single easiest way for an exchange to lose trust with its users — and the void path is where commission models are usually wrong.

**Acceptance criteria**

- [ ] Written model: base, what it applies to, when it is charged, and the rounding direction
- [ ] Decision on whether commission is charged per bet or netted per market per period, with the reasoning
- [ ] Property test: no sequence of bets, voids or re-grades produces a negative commission refund that exceeds the commission charged
- [ ] Answer on void behaviour: is commission fully reversed, pro-rated, or retained
- [ ] Integer-only arithmetic end to end, with no float and no implicit rounding at any step
- [ ] Customer-facing example: one customer's full session, from acceptance to settlement, with every commission movement shown

<a id="q63"></a>

#### Q63 · Prove on-chain settlement idempotency across retries and reorgs

[Issue #334](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/334) · `spike` · `critical` · **MVP**

> How is a Solana settlement made exactly-once per business event — the settlement-nonce account, what happens on RPC timeout and retry, and how is a duplicate payment made impossible?

**Why it matters.** A signed transaction cannot execute twice, but a retry that builds a second transaction can pay twice, and RPC timeouts make retries unavoidable. This is the most likely way an on-chain exchange loses money, so it is a correctness requirement, not an optimisation. Devnet and testnet also do not exercise reorg behaviour, so the design has to be right without the cluster telling us we got it wrong.

**Acceptance criteria**

- [ ] Settlement-nonce PDA derived from (customer, market, epoch), with the derivation shown
- [ ] Program rejects a second settlement for a nonce, proven by a test that submits the same settlement twice
- [ ] Written retry policy: what triggers a retry, what triggers a rebuild, and what aborts to manual review
- [ ] Answer on cluster fork and slot expiry: is the settlement re-submitted, confirmed by resubmission, or escalated
- [ ] Invariant: for any sequence of network responses, a customer's net position moves at most once per settlement key
- [ ] CI runs the full settlement path twice against a local validator and proves the second run is a no-op

<a id="q64"></a>

#### Q64 · Define the odds tick-size ladder per market

[Issue #335](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/335) · `feat` · `high` · **MVP**

> What price increments are valid for a market, who sets them, and how is an off-ladder price handled at submission?

**Why it matters.** Prices are customer-proposed now, so they must be constrained to a valid set or the book fills with prices nobody intends. This replaces the old overround and margin validation as the mechanism that keeps prices sane.

**Acceptance criteria**

- [ ] Ladder definition: valid increments per price range, and where the range boundaries sit
- [ ] Answer on who sets the ladder — configuration, or derived from the market's own price range
- [ ] Decision on an off-ladder price at submission: reject, round, or accept with a warning, and which is default
- [ ] Property test: every price in the book is on a valid ladder step
- [ ] Ladder changes are audited events and cannot rewrite a book already populated

<a id="q65"></a>

#### Q65 · Specify market suspension and in-play behaviour

[Issue #336](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/336) · `feat` · `high` · **Portfolio Depth**

> When a market is suspended mid-event — is trading halted, orders cancelled, or the market left open with a delay — and what happens to orders and matches that were in flight at the moment of suspension?

**Why it matters.** An empty book and a suspended market are the two real failure modes of a peer-to-peer book, and neither exists in the bookmaker design. In-flight orders at the instant of suspension are the specific case that usually goes undefined and then gets argued about.

**Acceptance criteria**

- [ ] Written rules for each suspension trigger, and who may trigger it
- [ ] Answer on the fate of resting orders at the moment of suspension: held, cancelled, or converted to matches
- [ ] Answer on in-flight matches: does a match that committed before suspension still stand
- [ ] Time-to-halt measured and recorded, consistent with the platform kill switch
- [ ] Suspension and resumption are audited events that customers can see and disputes can reference

<a id="q66"></a>

#### Q66 · Define odd-lot handling: who is counterparty to the remainder

[Issue #337](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/issues/337) · `spike` · `high` · **Portfolio Depth**

> When an order cannot be fully filled by the opposite side of the book, who takes the remainder — is the exchange the counterparty at a posted price, is the remainder discarded, or does the order stay live?

**Why it matters.** An exchange that claims not to be the counterparty is still the counterparty to whatever nobody else wants to take. ADR-0001 admits this residual exists; the question is how it is priced and who carries it, and pretending it does not exist would make the liability cap withdrawn in ADR-0001 quietly necessary again.

**Acceptance criteria**

- [ ] Explicit statement of when and how the exchange becomes counterparty, if at all
- [ ] If the exchange takes the remainder: the price, the limit, and whose exposure it consumes
- [ ] Decision on whether an unfilled remainder is discarded, held, or held-then-expired, and at what time
- [ ] Written argument on whether the odd-lot exposure is bounded by the same account exposure limit, and if not, why not
- [ ] If the exchange takes the remainder, the accounting is visible in the double-entry ledger, not absorbed silently
