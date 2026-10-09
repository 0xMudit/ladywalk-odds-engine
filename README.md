<div align="center">

<h1>LadyWalk Odds Engine</h1>

**An open-source peer-to-peer betting exchange, built in public — designed to survive every question a principal engineer asks before signing off.**

The hard part of a betting platform is not a `POST /bets` endpoint. It is money correctness, concurrency under load, provable odds, exactly-once settlement, and adversarial clients. This repository is built from the *question set* and the *roadmap* rather than ahead of them: 66 design questions and 267 build issues, each tracked as a GitHub issue and each closed with a committed artefact — an ADR, a benchmark, or a test that fails against the pre-fix code.

<p>
<a href="https://github.com/0xMudit/ladywalk-odds-engine/actions/workflows/ci.yml"><img src="https://github.com/0xMudit/ladywalk-odds-engine/actions/workflows/ci.yml/badge.svg" alt="CI status" /></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green.svg" alt="MIT license" /></a>
<a href="docs/design/open-questions.md"><img src="https://img.shields.io/badge/design%20questions-66-informational.svg" alt="66 open design questions" /></a>
<a href="docs/roadmap.md"><img src="https://img.shields.io/badge/roadmap-267%20issues-success.svg" alt="267 build issues" /></a>
<img src="https://img.shields.io/badge/status-design%20phase-orange.svg" alt="Design phase" />
<img src="https://img.shields.io/badge/Rust-1.98.1-000000.svg?logo=rust&logoColor=white" alt="Rust 1.98.1" />
<img src="https://img.shields.io/badge/PostgreSQL-16-4169E1.svg?logo=postgresql&logoColor=white" alt="PostgreSQL 16" />
</p>

</div>

---

> **Status: design phase, implementation beginning.** Two architectural decisions are settled — the betting model ([ADR-0001](docs/adr/0001-peer-to-peer-exchange.md)) and the settlement layer ([ADR-0002](docs/adr/0002-solana-settlement.md)) — and everything else is still an open question. The Cargo workspace exists and the `domain` crate, where the betting rules live, is implemented first, because it is the one crate that can be finished without answering a single open design question. No service is deployed.

## Why this repository exists

Most betting-platform projects are a `POST /bets` endpoint and a `Decimal` field that quietly becomes a `float`. The interesting engineering is everything around it:

- **Money correctness.** Integer minor units, an append-only double-entry ledger, a zero-tolerance sum-to-zero invariant, and reversal-by-compensation instead of mutation.
- **Concurrency under load.** What serialises concurrent orders on one market — and how that choice constrains horizontal scaling.
- **Odds you can prove.** An immutable odds history, so any historical payout can be reproduced byte-for-byte, with the calculation version that produced it.
- **Settlement that is exactly-once.** A settlement job that can crash partway and resume without double-paying anyone or skipping anyone.
- **Adversarial clients.** In an exchange the client proposes the price — but the matching engine decides the match. Price-time priority is a correctness property, not a performance detail.

Every question is tracked publicly because **"we thought about it" is not the same as "we measured it."** Acceptance criteria require an artefact, not a paragraph.

## The design questions at a glance

66 open questions (`Q01`–`Q66`) across 12 subsystems, one GitHub issue each. The type is derived from the wording: a question about something that *could be wrong* is a `bug`, something that *must be built* is a `feat`, something that *cannot be answered without measuring* is a `spike`.

| Subsystem | Q | Covers |
| --- | --: | --- |
| Data Modeling & Correctness | 7 | money representation, schema, ledger, odds versioning, voids |
| Concurrency & Consistency | 7 | serialization, ACID boundaries, TOCTOU, locking, idempotency |
| Betting Model | 6 | exchange design, matching, pricing, commission, market mechanics |
| Real-Time Delivery | 5 | transport, fanout, resync, backpressure, hot markets |
| Settlement & Grading | 5 | grading model, exactly-once payout, resumability, clawback, reconciliation |
| Failure Modes & Reliability | 5 | fail safe vs fail open, circuit breakers, quarantine, DR, kill switch |
| Scalability & Architecture | 6 | service boundaries, latency budget, read/write split, hot partitions, rate limits |
| Security | 6 | price spoofing, bots, admin auth, negative balance, replay, audit log |
| Observability | 4 | SLIs/SLOs, silent-bug detection, liability dashboards, ledger alerts |
| Testing | 4 | load, property-based fuzzing, crash injection, chaos |
| Deployment | 3 | zero-downtime, production-like staging, calc versioning |
| Exchange Matching | 8 | exposure limits, self-match prevention, partial fills, tick ladder, on-chain idempotency |

**Type split:** 42 `feat` · 15 `task` · 2 `bug` · 7 `spike`. **Priority split:** 21 `critical` · 22 `high` · 20 `medium` · 3 `low`.

The two `bug` issues are the classes that would otherwise ship by accident:

- **[Q10 — TOCTOU double-spend.](https://github.com/0xMudit/ladywalk-odds-engine/issues/10)** A balance check and a balance deduction that are not one atomic step. Reachable by any user with two concurrent requests, and invisible in manual QA.
- **[Q45 — negative-balance exploit.](https://github.com/0xMudit/ladywalk-odds-engine/issues/45)** Validate-then-deduct is not atomic, so two requests both pass validation and both deduct.

## The build roadmap at a glance

267 build issues across 16 epics (`EP-01`–`EP-16`), ordered from the first commit to a defensible production claim. The build track is separate from the design track on purpose: a question is answered by a decision, a task by merged code.

| Milestone | Issues | Meaning |
| --- | --: | --- |
| **B1 · Foundations** | 28 | Tooling, CI, infrastructure, identity. Nothing here touches money, but everything downstream depends on it. |
| **B2 · Money Path** | 98 | Money type, double-entry ledger, markets, the order book, exposure, settlement, Solana settlement. Highest risk, highest priority. |
| **B3 · Product** | 58 | Real-time delivery, web client, public API, webhooks, SDKs, observability. |
| **B4 · Hardening** | 83 | Audit, reconciliation, chaos testing, DR drills, security, compliance, penetration testing. |
| **Total** | **267** | 16 epics |

The milestones are a **dependency order**, not a priority ranking: the ledger (`EP-04`) exists before bet placement (`EP-05`), because a bet is a ledger movement and not a row insert. See the full plan in [`docs/roadmap.md`](docs/roadmap.md).

> **A note on the counts.** The generated indexes are authoritative: [`docs/design/open-questions.md`](docs/design/open-questions.md) lists **66** questions (the original 58 plus 8 exchange questions, `Q59`–`Q66`, added by ADR-0001 and ADR-0002) and [`docs/roadmap.md`](docs/roadmap.md) lists **267** issues across **16** epics (EP-16 added Solana settlement). A few older cross-references still quote the pre-exchange figures (58 questions / 260 issues); the generated files are the source of truth.

## What is decided, and what is open

Two decisions are recorded and are not up for re-litigation without a superseding ADR:

- **[ADR-0001 — peer-to-peer exchange](docs/adr/0001-peer-to-peer-exchange.md).** LadyWalk is a customer-to-customer exchange with a price-time priority order book, **not** a bookmaker. There is no vig, no odds curve, and no platform liability cap; revenue is commission on net winnings.
- **[ADR-0002 — Solana for settlement, matching off-chain](docs/adr/0002-solana-settlement.md).** Settlement moves the net position per `(customer, market, epoch)`, keyed by a settlement nonce so a retry cannot pay twice, with the OIDC-authenticated matching engine kept off-chain.

Everything else is still an open question. The remaining spine — the questions most other decisions hang off — is the order-book data structure and per-market serialization ([Q08](https://github.com/0xMudit/ladywalk-odds-engine/issues/8)) and the service boundaries and the money path's failure domain ([Q36](https://github.com/0xMudit/ladywalk-odds-engine/issues/36)).

## Capabilities

The committed guarantees below are written as if decided — each becomes an ADR when its question closes. They are enforced by the type system, CI gates, and tests, not by convention.

| Guarantee | How it is held |
| --- | --- |
| **Money is never a float** | A `MoneyAmount` newtype over `i64` minor units, `BIGINT` in the database, and a CI gate that fails the build on a float in a money path ([Q01](https://github.com/0xMudit/ladywalk-odds-engine/issues/1)). |
| **The ledger is the source of truth** | `balance` is a derived projection; no code path mutates a posted entry; corrections are compensating entries ([Q03](https://github.com/0xMudit/ladywalk-odds-engine/issues/3)). |
| **Double-entry sums to zero, with zero tolerance** | Not "within rounding." A reconciliation discrepancy fails the run loudly ([Q04](https://github.com/0xMudit/ladywalk-odds-engine/issues/4)). |
| **The client proposes the price; the engine decides the match** | An order matches only at a price and size permitted by the tick ladder and the remaining exposure of *both* accounts ([Q19](https://github.com/0xMudit/ladywalk-odds-engine/issues/19), [Q42](https://github.com/0xMudit/ladywalk-odds-engine/issues/42)). |
| **Exposure is bounded atomically** | A balance may be negative, bounded by a configured exposure limit; the check and the decrement are one atomic step ([Q10](https://github.com/0xMudit/ladywalk-odds-engine/issues/10), [Q45](https://github.com/0xMudit/ladywalk-odds-engine/issues/45)). |
| **Payouts are exactly-once and resumable** | Idempotency keys everywhere, and a settlement checkpoint written in the same transaction as the payout ([Q13](https://github.com/0xMudit/ladywalk-odds-engine/issues/13), [Q27](https://github.com/0xMudit/ladywalk-odds-engine/issues/27)). |
| **Odds history is immutable** | Every bet records the odds version it executed against, and every odds version records the calculation version that produced it ([Q05](https://github.com/0xMudit/ladywalk-odds-engine/issues/5)). |
| **Every state change is audited tamper-evidently** | Hash-chained, append-only, and not writable by the application role ([Q47](https://github.com/0xMudit/ladywalk-odds-engine/issues/47)). |
| **One switch stops all money movement** | A platform-wide kill switch with audited trigger authority and a measured time-to-halt ([Q35](https://github.com/0xMudit/ladywalk-odds-engine/issues/35)). |
| **Fail safe, not fail open** | If the odds pipeline dies mid-event, betting on that market halts — trading revenue against financial correctness is a choice, made explicitly ([Q31](https://github.com/0xMudit/ladywalk-odds-engine/issues/31)). |

## What exists today

The workspace builds, and its dependency direction is enforced rather than documented:

- **`domain` — the rules, with nothing to stand on.** Money, odds, quotes, orders, matching, the ledger, settlement, and risk. This crate is allowed exactly one external dependency — `serde` — enforced by `crates/domain/tests/dependency_purity.rs` (which reads the crate's own `Cargo.toml`) and by `scripts/check-domain-deps.sh` (which walks the resolved graph with `cargo metadata`). Both run in CI. A rule that cannot be evaluated without a database is a rule nobody will test properly.
- **`application` — the use cases.** Order, quote, market, wallet, settlement, and reconciliation orchestration behind ports declared in this crate, plus configuration, telemetry, signing, and graceful-shutdown primitives.
- **`adapters` — the only crate allowed to touch the outside world.** PostgreSQL via `sqlx`, HTTP via Axum, and the initial schema migration. This is the crate that can be replaced, and the one whose tests need containers.
- **`api` — the versioned public contract**, and **`web` — the server-rendered surface**, kept apart because a browser can be forced to reload but an integrator's integration cannot.
- **`bin/order-entry` — the thin entrypoint.** Loads configuration (fail-fast on missing keys), initialises tracing, starts the Axum server, and coordinates graceful shutdown. No business logic lives in a binary.

**No `.sqlx` metadata is committed, deliberately.** Every money-path statement is written with the runtime `query` / `query_as` forms and exercised against a real database by the test suite, rather than compiled against whatever schema the last developer happened to have running ([`Cargo.toml`](Cargo.toml)).

CI runs five gates: no floats in money paths, generated design/roadmap indexes in sync with the issues, relative documentation links resolve, and the workspace builds, tests, formats, and keeps `domain` infrastructure-free ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)).

## Requirements

- **Rust 1.98.1**, pinned in [`rust-toolchain.toml`](rust-toolchain.toml) (with `rustfmt` and `clippy`).
- **Docker** (recommended) to run PostgreSQL 16 and Redis 7, and to build and test without a local Rust install.
- **PostgreSQL 16** and **Redis 7** are needed to run the service; the `domain` crate's tests need neither.

## Getting started

```bash
git clone https://github.com/0xMudit/ladywalk-odds-engine.git
cd ladywalk-odds-engine

# Start dependencies and run the whole workspace's tests in the pinned container
docker compose up -d postgres redis
docker compose run --rm dev cargo test --workspace

# Or use a local Rust 1.98.1 toolchain directly
cargo build --workspace --locked
cargo test  --workspace --locked
cargo fmt   --all --check
./scripts/check-domain-deps.sh      # domain must not reach I/O crates
```

To run the service locally, copy the environment file and start the binary:

```bash
cp .env.example .env     # every key with its meaning and default
docker compose up -d      # postgres + redis
cargo run --bin order-entry   # serves on 0.0.0.0:8080, graceful shutdown on SIGTERM
```

Configuration is read from the environment; the full reference is [`.env.example`](.env.example). In staging and production `KILL_SWITCH_ENABLED` must be `true` — a false value in a non-local environment is a startup error.

## Project layout

```
crates/
  domain/        money types, error taxonomy, and the rules of the exchange (serde only)
  application/   use cases; config, telemetry, signing, and shutdown primitives
  adapters/      sqlx, HTTP handlers, migrations, and every other I/O edge
  api/           the versioned public HTTP contract
  web/           the server-rendered surface
bin/
  order-entry/   the thin binary that wires the above together
docs/
  architecture/  the dependency spine, open decisions, committed constraints
  design/        open-questions.md — the 66 design questions, each linked to its issue
  adr/           architecture decision records, one file per decision
  roadmap.md     the 267-issue build plan
scripts/         domain-dependency and money-type CI gates
docker-compose.yml   PostgreSQL 16 + Redis 7 + a pinned Rust dev container
```

The dependency direction is enforced, not merely documented: a rule about betting cannot depend on a database, a socket, or a web framework.

## Stack

**Committed** (wired in the workspace): Rust 1.98.1 · Tokio · Axum · `sqlx` / PostgreSQL 16 · Redis 7 · `serde` · `thiserror` / `anyhow` · `tracing` · `proptest` · GitHub Actions · Docker (cargo-chef).

**Planned** (tracked in the roadmap): Next.js / TypeScript web client (EP-11), Keycloak / OIDC identity (EP-03), OpenTelemetry (EP-13), Solana settlement (EP-16), and Terraform-based infrastructure (EP-02). Deliberately **not** chosen: Kafka, an ORM, Kubernetes, and a second backend language — the reasons are in the [roadmap](docs/roadmap.md).

## Scope and safety

This is an educational, open-source project. It is **not** a licensed gambling operator: it publishes no odds, takes no bets, and handles no real money. All fixtures are synthetic. Legal and regulatory requirements (licensing, age verification, KYC, responsible-gambling tooling, geo-restriction, tax reporting) are treated as **cited research in an ADR, not legal advice**, and are tracked as such in [Q29](https://github.com/0xMudit/ladywalk-odds-engine/issues/29).

## Documentation

| Start here | What it is |
| --- | --- |
| [`docs/README.md`](docs/README.md) | Index of everything written down about this system. |
| [`docs/design/open-questions.md`](docs/design/open-questions.md) | The 66 design questions, grouped by subsystem, each linked to its issue. |
| [`docs/roadmap.md`](docs/roadmap.md) | The 267-issue implementation plan, 16 epics, first commit to production. |
| [`docs/architecture/README.md`](docs/architecture/README.md) | The dependency spine, what is decided, and what each open question blocks. |
| [`docs/adr/`](docs/adr/) | Why each decision was made, what was rejected, and what would reopen it. |

The generated indexes are produced from the issue data; CI fails if they drift, so change the source and regenerate rather than hand-editing them.

## Contributing

Claim an issue, satisfy its acceptance criteria, and close it with the artefact linked — an issue is closed when an artefact is committed, not when a paragraph is typed. The two-track model, the ADR process, the conventional-commit style, and the workflow are in [`.github/CONTRIBUTING.md`](.github/CONTRIBUTING.md). Expectations and how to report a concern privately are in [`.github/CODE_OF_CONDUCT.md`](.github/CODE_OF_CONDUCT.md).

## Security

Do not open a public issue for a security vulnerability, especially one in the money path. Report it privately via GitHub's **Security → Report a vulnerability**, following [`.github/SECURITY.md`](.github/SECURITY.md). A race condition that can produce a negative balance is `critical` regardless of how hard it is to reproduce.

## License

[MIT](LICENSE).
