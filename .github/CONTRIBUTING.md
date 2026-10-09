# Contributing

Thanks for looking. This project is a **design-first** open-source build, and the contribution model follows from that.

There are two tracks of work, and they close differently:

| Track | What it is | Closes when |
| --- | --- | --- |
| **Design** · [open design questions](../docs/design/open-questions.md) · 58 questions | Open decisions that block the build. `Q01`–`Q58`. | A decision artefact is committed — an ADR, a benchmark, a failing test. |
| **Build** · [Roadmap](../docs/roadmap.md) · 260 issues | The implementation. `EP-01`–`EP-15` epics, `F-xxx` tasks, `H-xxx` hardening. | The code merges and every acceptance criterion is demonstrably met. |

A design question is answered by a decision. A build task is answered by merged code. Keeping them
separate is what stops a design discussion from stalling behind an implementation, and an
implementation from silently deciding a question nobody voted on.

## The one rule

> An issue is closed when an **artefact is committed**, not when a paragraph is typed.

A question is answered when the answer is reproducible. That artefact is usually one of:

| Question shape | Required artefact |
| --- | --- |
| "Which X or Y?" | An ADR in `docs/adr/` with the alternatives and the rejected ones |
| "How fast / how much?" | A benchmark or measurement committed alongside the decision |
| "What could be wrong?" | A test that **fails against the pre-fix code** and passes after |
| "What happens on failure?" | A runbook, plus a drill result with the measured outcome |

## Workflow

1. **Claim it.** Comment on the issue, or assign yourself. No need to ask permission.
2. **Open a branch** named after the issue: `q03-double-entry-ledger`, `q11-wallet-lock-benchmark`.
3. **Work the acceptance criteria** as a checklist. Tick them in the PR description, not in the issue body — the issue body is the source of truth and should stay stable.
4. **Open a PR** referencing the issue (`Closes #42`). The CI lint gate in Q01 will fail your build if you have used a float in a money path. That is the point of it.
5. **Close by merging.** The issue closes when the artefact lands.

If you conclude an issue should be answered *differently* than the acceptance criteria suggest, that is a welcome contribution — propose the change in the PR and say what evidence drove it. Do not silently satisfy a checkbox.

## Claiming order

**If you are writing code**, work the build milestones in [Roadmap](../docs/roadmap.md) — `B1`
Foundations, then `B2` Money Path, then `B3` Product, then `B4` Hardening. That order is a
dependency order, not a preference: `EP-04` builds the double-entry ledger before `EP-05` accepts a
bet, because a bet is a ledger movement. Check the `blocked_by` line in the issue body before you
start.

**If you are settling a design question**, work `priority:critical` first, and within that,
respect the dependency order in [Architecture](../docs/architecture/README.md). The three questions that block
most of the others:

- **[Q15](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/15)** — fixed-odds, pari-mutuel, or exchange?
- **[Q08](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/8)** — what serialises concurrent bets on a market?
- **[Q36](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/36)** — where are the failure domain boundaries?

Answering Q15 wrong invalidates most of the concurrency and liability design. Please do not start there without settling it.

## ADRs

Architecture decisions live in `docs/adr/NNNN-kebab-title.md`, numbered sequentially, immutable once merged. To supersede one, write a new ADR that links to and quotes the old one — never edit a merged ADR in place.

Template:

```markdown
# NNNN. Title

**Status:** Proposed | Accepted | Superseded by ADR-NNNN
**Date:** YYYY-MM-DD
**Issue:** #nn
**Deciders:** ...

## Context
What forces are at play? What breaks if we do nothing?

## Options considered
### Option A — name it
How it works. Throughput. Failure modes. Cost.

### Option B — name it
...

## Decision
Option A, because ...

## Consequences
**Becomes easier:** ...
**Becomes harder:** ...
**We accept these costs:** ...
**Revisit if:** <the measurable trigger that should reopen this>
```

The `Revisit if` line matters. A decision without a trigger for revisiting is a decision that cannot be corrected.

## Commit messages

Conventional Commits, because the changelog is generated from them.

```
feat(ledger): append-only double-entry postings with derived balance
fix(wallet): make deduct conditional at the storage layer to close TOCTOU
docs(adr): 0015 choose fixed-odds over pari-mutuel for v1
test(settlement): kill the job at 70% and assert exactly-once payout
```

Scopes follow the `area:` labels. `test:` and `docs:` commits do not need a linked issue.

## Finding a question we missed

Open an issue using the **New finding** template. Give it an `area:` label and a rough `priority:`. It does not need a `Qnn` number — a maintainer assigns that when it is folded into [open design questions](../docs/design/open-questions.md). A question that turns out to be already covered gets closed with a pointer to the covering issue; that is a useful outcome, not a rejection.

## Reporting a vulnerability

**Do not open a public issue for a security vulnerability**, especially one in the money path. See [`SECURITY.md`](SECURITY.md).

## Code of conduct

Be accurate and be kind. Correctness is the contribution; being right about the code matters more
than being attached to an idea. Critique the design, never the person.

The full expectations, the betting-specific provisions, and how to report a concern privately are
in [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).
