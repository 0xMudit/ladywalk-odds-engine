# Implementation Roadmap

**267 issues across 16 epics, ordered from the first commit to a defensible production claim.**

This is the *build* track. The 67 questions in [open design questions](design/open-questions.md) are the *design* track: open
decisions that need an artefact to close. The two are kept separate on purpose — a question is
answered by a decision, a task is answered by merged code.

- **Design track** · 67 questions → [open design questions](design/open-questions.md)
- **Build track** · 267 issues → this file

This file is generated from the issue data. CI fails if it drifts from the
issues, so change the source and regenerate rather than editing it by hand.

---

## How to read the ordering

Milestones are a *dependency order*, not a wish list. `B1` is genuinely unblocked work; `B4`
mostly is not. Three rules explain most of the sequencing:

1. **The ledger precedes everything that moves money.** `EP-04` builds the double-entry ledger
   before `EP-05` accepts an order, because an order is a ledger movement, not a row insert.
2. **The order book precedes settlement.** `EP-06` matches, `EP-07` settles what it matched, and
   the exposure limit in `EP-06` is a precondition for settling anything.
3. **Security and compliance come last, deliberately.** `EP-14` is not neglected — it is
   designed against real components rather than imagined ones. It is still a hard gate before
   anything that could be mistaken for a real operator.

Every issue carries a scope, acceptance criteria that require a *verifiable artefact* (a test
that fails against the pre-fix code, a benchmark, an ADR), and a milestone. No issue closes on
assertion.

## Milestones

| Milestone | Issues | Meaning |
| --- | --: | --- |
| **[B1 · Foundations](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/milestone/4)** | 28 | Tooling, CI, infrastructure, identity. Nothing here touches money, but everything downstream depends on it. |
| **[B2 · Money Path](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/milestone/5)** | 98 | The money path. Money type, double-entry ledger, markets, the order book, exposure, settlement. Highest risk, highest priority. |
| **[B3 · Product](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/milestone/6)** | 58 | Product surface. Real-time delivery, web client, public API, webhooks, SDKs, responsible-gambling surfaces. |
| **[B4 · Hardening](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/milestone/7)** | 83 | Hardening. Observability, audit, reconciliation, chaos testing, DR drills, penetration testing. |
| **Total** | **267** | 16 epics, 214 feature/chore tasks, 53 hardening tasks |

## Epics

| Epic | Milestone | Area | Tasks | Covers |
| --- | --- | --- | --: | --- |
| [EP-01](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/59) Repository, Tooling & Developer Experience | B1 | `tooling` | 0 | Clone to a running, tested, linted, fully type-checked service in under ten minutes — and no money bug can be written in the type system. |
| [EP-02](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/60) Infrastructure as Code & Environments | B1 | `infra` | 0 | Laptop, staging and production are all reproducible from code, and a database restore has actually been drilled, not just documented. |
| [EP-03](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/61) Identity, Access & Administration | B1 | `identity` | 0 | Deny-by-default authorisation, MFA on every admin path, and an attributable record of every privileged action. |
| [EP-04](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/62) Money, Accounts & Double-Entry Ledger | B2 | `data-modeling` | 0 | A `Money` type the compiler will not let you build from a float, and an append-only ledger that provably sums to zero. |
| [EP-05](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/121) Bet Engine | B2 | `settlement` | 0 | Accept, validate, price, and record a bet atomically and idempotently, so a request either produces exactly one ledger-balanced position or nothing at |
| [EP-06](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/122) Order Book & Price Engine | B2 | `matching` | 9 | Represent prices exactly and match them fairly: a single-writer order book that pairs back and lay orders by price then time, constrained to a valid p |
| [EP-07](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/123) Exposure, Commission & Settlement | B2 | `settlement` | 3 | Turn a final result into balanced ledger movements, a correct and replayable payout for every matched pair, a commission charge that reverses cleanly, |
| [EP-08](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/124) Markets & Feeds | B2 | `data-modeling` | 0 | Own the definition of every market and the ingestion of upstream results, so that market state is derived from a single authoritative source and can a |
| [EP-09](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/125) Wallet, Balances & KYC Tiering | B2 | `identity` | 0 | Give every account a balance that is a derived, auditable fact of the ledger, and gate deposits and withdrawals by a documented verification tier. |
| [EP-10](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/126) Real-Time Delivery | B3 | `realtime` | 0 | Fan out committed state changes to connected clients over WebSockets with ordering, gap detection, and replayable catch-up, so a client is never silen |
| [EP-11](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/127) Web Client | B3 | `frontend` | 0 | Ship an accessible, responsive betting interface whose displayed state is always traceable to a server-confirmed state, and which never implies a bet  |
| [EP-12](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/128) Public API & Client SDKs | B3 | `api-sdk` | 0 | Expose a versioned, contract-tested public API with honest idempotency, rate limits, and webhooks, so third parties can integrate without the platform |
| [EP-13](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/129) Observability & Operations | B3 | `observability` | 0 | Make the system's internal state legible from outside it, through metrics, traces, dashboards, alerts, and runbooks, so an incident is diagnosable fro |
| [EP-14](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/130) Security, Compliance & Responsible Gambling | B4 | `security` | 0 | Make the security and responsible-gambling controls that a real deployment would be required to demonstrate buildable, testable, and evidenced, so the |
| [EP-15](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/131) Reliability, Release Engineering & Disaster Recovery | B4 | `release` | 0 | Make releases routine and boring, and make recovery from data loss a rehearsed, timed procedure rather than an improvisation. |
| [EP-16](https://github.com/BerlinDeskMudit/LadyWalk-odds-engine/blob/main/../issues/362) Solana Settlement & Wallet Integration | B2 | `onchain` | 7 | Settle a customer's net position to a Solana token account, exactly once, behind a feature flag, with devnet and testnet as the only networks that exi |

---

## EP-01 · Repository, Tooling & Developer Experience

**Milestone:** B1 · Foundations · **Tasks:** 15

| # | Task | Status |
| --: | --- | --- |
| 63 | #63 | open |
| 64 | #64 | open |
| 65 | #65 | open |
| 66 | #66 | open |
| 67 | #67 | open |
| 68 | #68 | open |
| 69 | #69 | open |
| 70 | #70 | open |
| 71 | #71 | open |
| 72 | #72 | open |
| 73 | #73 | open |
| 74 | #74 | open |
| 75 | #75 | open |
| 76 | #76 | open |
| 77 | #77 | open |

## EP-02 · Infrastructure as Code & Environments

**Milestone:** B1 · Foundations · **Tasks:** 11

| # | Task | Status |
| --: | --- | --- |
| 78 | #78 | open |
| 79 | #79 | open |
| 80 | #80 | open |
| 81 | #81 | open |
| 82 | #82 | open |
| 83 | #83 | open |
| 84 | #84 | open |
| 85 | #85 | open |
| 86 | #86 | open |
| 87 | #87 | open |
| 88 | #88 | open |

## EP-03 · Identity, Access & Administration

**Milestone:** B1 · Foundations · **Tasks:** 13

| # | Task | Status |
| --: | --- | --- |
| 89 | #89 | open |
| 90 | #90 | open |
| 91 | #91 | open |
| 92 | #92 | open |
| 93 | #93 | open |
| 94 | #94 | open |
| 95 | #95 | open |
| 96 | #96 | open |
| 97 | #97 | open |
| 98 | #98 | open |
| 99 | #99 | open |
| 100 | #100 | open |
| 101 | #101 | open |

## EP-04 · Money, Accounts & Double-Entry Ledger

**Milestone:** B2 · Money Path · **Tasks:** 19

| # | Task | Status |
| --: | --- | --- |
| 102 | #102 | open |
| 103 | #103 | open |
| 104 | #104 | open |
| 105 | #105 | open |
| 106 | #106 | open |
| 107 | #107 | open |
| 108 | #108 | open |
| 109 | #109 | open |
| 110 | #110 | open |
| 111 | #111 | open |
| 112 | #112 | open |
| 113 | #113 | open |
| 114 | #114 | open |
| 115 | #115 | open |
| 116 | #116 | open |
| 117 | #117 | open |
| 118 | #118 | open |
| 119 | #119 | open |
| 120 | #120 | open |

## EP-05 · Bet Engine

**Milestone:** B2 · Money Path · **Tasks:** 20

| # | Task | Status |
| --: | --- | --- |
| 132 | #132 | open |
| 133 | #133 | open |
| 134 | #134 | open |
| 135 | #135 | open |
| 136 | #136 | open |
| 137 | #137 | open |
| 138 | #138 | open |
| 139 | #139 | open |
| 140 | #140 | open |
| 141 | #141 | open |
| 142 | #142 | open |
| 143 | #143 | open |
| 144 | #144 | open |
| 145 | #145 | open |
| 277 | #277 | open |
| 278 | #278 | open |
| 279 | #279 | open |
| 280 | #280 | open |
| 281 | #281 | open |
| 282 | #282 | open |

## EP-06 · Order Book & Price Engine

**Milestone:** B2 · Money Path · **Tasks:** 27

| # | Task | Status |
| --: | --- | --- |
| 146 |  ] [F-073 · Write the price representation ADR | open |
| 147 |  ] [F-074 · Implement exact decimal price arithmetic | open |
| 148 |  ] [F-075 · Implement decimal, fractional, and American format conversion | open |
| 149 |  ] [F-076 · Convert between prices and implied probability | open |
| 153 |  ] [F-080 · Validate prices and reject malformed ones | open |
| 154 |  ] [F-081 · Snapshot the book for audit and dispute resolution | open |
| 156 |  ] [F-083 · Publish price change events to the real-time stream | open |
| 158 |  ] [F-085 · Property-test price arithmetic across the full domain | open |
| 287 |  ] [H-011 · Guard decimal parsing against locale and exponent tricks | open |
| 150 |  ] ~~[F-077 · Compute the book overround and expose it explicitly | open |
| 151 |  ] ~~[F-078 · Apply a documented margin policy to pricing | open |
| 152 |  ] ~~[F-079 · Build a versioned price model registry | open |
| 155 |  ] ~~[F-082 · Add price model evaluation harness | open |
| 157 |  ] ~~[F-084 · Handle price model failure without publishing bad prices | open |
| 285 |  ] ~~[H-009 · Ensure a failed price recomputation cannot publish a stale price as fresh | open |
| 286 |  ] ~~[H-010 · Pin the model version so a redeploy cannot change historical prices | open |
| 284 |  ] ~~[H-008 · Reject a book that implies an impossible probability | open |
| 283 |  ] ~~[H-007 · Reject prices that round to a value implying certainty | open |
| 339 |  ] [F-196 · Define the order book structure ADR | open |
| 340 |  ] [F-197 · Implement the price-time priority order book | open |
| 341 |  ] [F-198 · Enforce the per-account exposure limit atomically at match time | open |
| 342 |  ] [F-199 · Implement self-match prevention | open |
| 343 |  ] [F-200 · Implement partial fills and the remainder lifecycle | open |
| 344 |  ] [F-201 · Implement the tick-size ladder validation | open |
| 345 |  ] [F-202 · Publish book change events to the real-time stream | open |
| 346 |  ] [F-203 · Property-test that price-time priority is deterministic | open |
| 347 |  ] [F-204 · Detect and flag suspicious order patterns | open |

## EP-07 · Exposure, Commission & Settlement

**Milestone:** B2 · Money Path · **Tasks:** 23

| # | Task | Status |
| --: | --- | --- |
| 159 |  ] [F-086 · Define the settlement model ADR | open |
| 160 |  ] [F-087 · Implement exactly-once settlement processing | open |
| 161 |  ] [F-088 · Compute payouts from the frozen price snapshot | open |
| 162 |  ] [F-089 · Write ledger entries atomically with settlement | open |
| 165 |  ] [F-092 · Handle void, abandoned, and postponed results | open |
| 166 |  ] [F-093 · Build a deterministic settlement replay tool | open |
| 167 |  ] [F-094 · Settle a result set in a single bounded transaction | open |
| 168 |  ] [F-095 · Expose settlement status and payout history | open |
| 169 |  ] [F-096 · Reconcile payouts against feed-provided totals | open |
| 170 |  ] [F-097 · Property-test settlement invariants | open |
| 288 |  ] [H-012 · Prevent a result correcting an already-settled bet silently | open |
| 289 |  ] [H-013 · Guarantee a payout is never issued twice under concurrent workers | open |
| 290 |  ] [H-014 · Handle a settlement run that dies partway through a result set | open |
| 291 |  ] [H-015 · Cap a runaway payout and surface it as an anomaly | open |
| 292 |  ] [H-016 · Make void refunds idempotent under concurrent requests | open |
| 293 |  ] [H-017 · Prevent a reconciliation job from masking a real divergence | open |
| 294 |  ] [H-018 · Reject a result whose selection set does not match the market | open |
| 305 |  ] [H-019 · Prevent settlement of a bet accepted after the market closed | open |
| 163 |  ] [F-090 · Compute a customer's exposure figure and persist it | open |
| 164 |  ] [F-091 · Run a scheduled exposure reconciliation against customer limits | open |
| 348 |  ] [F-205 · Implement commission on net winnings | open |
| 349 |  ] [F-206 · Reverse commission idempotently on void and re-grade | open |
| 350 |  ] [F-207 · Compute the net position per (customer, market, epoch) | open |

## EP-08 · Markets & Feeds

**Milestone:** B2 · Money Path · **Tasks:** 11

| # | Task | Status |
| --: | --- | --- |
| 171 | #171 | open |
| 172 | #172 | open |
| 173 | #173 | open |
| 174 | #174 | open |
| 175 | #175 | open |
| 176 | #176 | open |
| 177 | #177 | open |
| 178 | #178 | open |
| 179 | #179 | open |
| 180 | #180 | open |
| 181 | #181 | open |

## EP-09 · Wallet, Balances & KYC Tiering

**Milestone:** B2 · Money Path · **Tasks:** 16

| # | Task | Status |
| --: | --- | --- |
| 182 | #182 | open |
| 183 | #183 | open |
| 184 | #184 | open |
| 185 | #185 | open |
| 186 | #186 | open |
| 187 | #187 | open |
| 188 | #188 | open |
| 189 | #189 | open |
| 190 | #190 | open |
| 191 | #191 | open |
| 192 | #192 | open |
| 306 | #306 | open |
| 307 | #307 | open |
| 308 | #308 | open |
| 309 | #309 | open |
| 310 | #310 | open |

## EP-10 · Real-Time Delivery

**Milestone:** B3 · Product · **Tasks:** 15

| # | Task | Status |
| --: | --- | --- |
| 193 | #193 | open |
| 194 | #194 | open |
| 195 | #195 | open |
| 196 | #196 | open |
| 197 | #197 | open |
| 198 | #198 | open |
| 199 | #199 | open |
| 200 | #200 | open |
| 209 | #209 | open |
| 210 | #210 | open |
| 211 | #211 | open |
| 311 | #311 | open |
| 312 | #312 | open |
| 313 | #313 | open |
| 314 | #314 | open |

## EP-11 · Web Client

**Milestone:** B3 · Product · **Tasks:** 20

| # | Task | Status |
| --: | --- | --- |
| 212 | #212 | open |
| 213 | #213 | open |
| 214 | #214 | open |
| 215 | #215 | open |
| 216 | #216 | open |
| 217 | #217 | open |
| 218 | #218 | open |
| 219 | #219 | open |
| 220 | #220 | open |
| 221 | #221 | open |
| 222 | #222 | open |
| 223 | #223 | open |
| 224 | #224 | open |
| 225 | #225 | open |
| 226 | #226 | open |
| 315 | #315 | open |
| 316 | #316 | open |
| 317 | #317 | open |
| 318 | #318 | open |
| 319 | #319 | open |

## EP-12 · Public API & Client SDKs

**Milestone:** B3 · Product · **Tasks:** 17

| # | Task | Status |
| --: | --- | --- |
| 227 | #227 | open |
| 228 | #228 | open |
| 229 | #229 | open |
| 230 | #230 | open |
| 231 | #231 | open |
| 232 | #232 | open |
| 233 | #233 | open |
| 234 | #234 | open |
| 235 | #235 | open |
| 236 | #236 | open |
| 237 | #237 | open |
| 238 | #238 | open |
| 239 | #239 | open |
| 320 | #320 | open |
| 321 | #321 | open |
| 322 | #322 | open |
| 323 | #323 | open |

## EP-13 · Observability & Operations

**Milestone:** B3 · Product · **Tasks:** 15

| # | Task | Status |
| --: | --- | --- |
| 240 | #240 | open |
| 241 | #241 | open |
| 242 | #242 | open |
| 243 | #243 | open |
| 244 | #244 | open |
| 245 | #245 | open |
| 246 | #246 | open |
| 247 | #247 | open |
| 248 | #248 | open |
| 249 | #249 | open |
| 250 | #250 | open |
| 251 | #251 | open |
| 324 | #324 | open |
| 325 | #325 | open |
| 326 | #326 | open |

## EP-14 · Security, Compliance & Responsible Gambling

**Milestone:** B4 · Hardening · **Tasks:** 20

| # | Task | Status |
| --: | --- | --- |
| 252 | #252 | open |
| 253 | #253 | open |
| 254 | #254 | open |
| 255 | #255 | open |
| 256 | #256 | open |
| 257 | #257 | open |
| 258 | #258 | open |
| 259 | #259 | open |
| 260 | #260 | open |
| 261 | #261 | open |
| 262 | #262 | open |
| 263 | #263 | open |
| 264 | #264 | open |
| 327 | #327 | open |
| 328 | #328 | open |
| 201 | #201 | open |
| 202 | #202 | open |
| 203 | #203 | open |
| 204 | #204 | open |
| 205 | #205 | open |

## EP-15 · Reliability, Release Engineering & Disaster Recovery

**Milestone:** B4 · Hardening · **Tasks:** 15

| # | Task | Status |
| --: | --- | --- |
| 265 | #265 | open |
| 266 | #266 | open |
| 267 | #267 | open |
| 268 | #268 | open |
| 269 | #269 | open |
| 270 | #270 | open |
| 271 | #271 | open |
| 272 | #272 | open |
| 273 | #273 | open |
| 274 | #274 | open |
| 275 | #275 | open |
| 276 | #276 | open |
| 206 | #206 | open |
| 207 | #207 | open |
| 208 | #208 | open |

## EP-16 · Solana Settlement & Wallet Integration

**Milestone:** B2 · Money Path · **Tasks:** 10

| # | Task | Status |
| --: | --- | --- |
| 351 |  ] [F-208 · Scaffold the Anchor settlement program workspace | open |
| 352 |  ] [F-209 · Implement the settlement-nonce PDA and duplicate rejection | open |
| 353 |  ] [F-210 · Implement the idempotent net settlement instruction | open |
| 354 |  ] [F-211 · Anchor the audit log head hash on-chain | open |
| 355 |  ] [F-212 · Implement ledger-only settlement mode behind a feature flag | open |
| 356 |  ] [F-213 · Integrate wallet connection and token accounts | open |
| 357 |  ] [F-214 · Prove double settlement is rejected against a local validator | open |
| 358 |  ] [H-051 · Reject a settlement larger than the customer's settled balance | open |
| 359 |  ] [H-052 · Keep settlement state consistent across a cluster fork | open |
| 360 |  ] [H-053 · Fail loudly when on-chain and ledger positions disagree | open |

## Notes

**Withdrawn tasks.** Tasks struck through above are not deleted. They were
removed by [ADR-0001](../adr/0001-peer-to-peer-exchange.md) because the
concept they implement does not exist in a peer-to-peer exchange — there is
no vig, no odds curve, and no platform liability cap. They are kept visible so
the decision that removed them stays reviewable.

**Progress.** 2 of 267 build tasks closed (1%).
