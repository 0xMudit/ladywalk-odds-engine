# NNNN. Title stated as a decision

**Status:** Proposed
**Date:** YYYY-MM-DD
**Issue:** [#nnn](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/nnn)
**Deciders:** @handle, @handle
**Supersedes:** none

<!--
Delete the guidance in this block before merging.
-->

## Context

<!--
What forces are at play, and what breaks if we do nothing?

State the constraint, not the conclusion. "Concurrent bet placement on one market
must not produce a negative balance, and the p99 budget for bet acceptance is
X" is context. "We should use per-market locks" is a decision, and belongs below.

Link the questions this answers. If it answers none, that is a signal the ADR is
premature.
-->

## Options considered

<!--
At least two. An ADR with one option is a note, not a decision, and the rejected
option is usually the one somebody proposes again later.
-->

### Option A — name it

- How it works
- Throughput and latency
- Failure modes
- Operational and licence cost

### Option B — name it

- How it works
- Throughput and latency
- Failure modes
- Operational and licence cost

## Decision

We chose Option A, because <the forcing constraint that decided it>.

<!--
State it so a reader who disagrees can identify exactly which premise they think
is wrong. That is the whole purpose of the paragraph.
-->

## Consequences

**Becomes easier:**

-

**Becomes harder:**

-

**We accept these costs:**

-

## Revisit if

<!--
Mandatory. A measurable trigger, not a feeling.

Good:   "p99 bet acceptance exceeds 50ms at 10k RPS sustained for 15 minutes."
Bad:    "if this turns out to be slow."
Bad:    "if the team grows."

This line is what makes the decision correctable. A decision with no trigger is a
decision that will be changed by whoever is loudest, rather than by whoever is
right.
-->
