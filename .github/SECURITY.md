# Security Policy

## Scope

This project handles bets and balances. Even while it runs entirely on synthetic fixtures, the design questions it tracks — double-spend, negative balance, price spoofing, replay — are the same class of defect that causes real financial loss. Reports about those are taken seriously.

## Reporting a vulnerability

**Do not open a public issue for a security vulnerability.**

Use GitHub's private reporting: **Security → Report a vulnerability** on
<https://github.com/BerlinDeskMudit/ladywalk-odds-engine>.

Please include:

- What an attacker can do, in terms of money or state
- The exact request sequence or code path, if you have it
- Whether it needs a valid account, and whether it is a race (two concurrent requests)
- Whether you have verified it, and how

### What is in scope

| Area | Examples |
| --- | --- |
| **Money path** | Double-spend, negative balance, payout that survives a re-grade, a settlement that pays twice on crash-resume |
| **Price integrity** | Executing a bet at an odds value the client chose, replaying a captured request, forging a quote token |
| **Authorisation** | A user mutating another user's balance, a bet, or a market; an admin action without a valid elevated permission |
| **Auditability** | A state change that produces no audit record, or a way to alter or delete an existing one |
| **Injection** | Anything reaching SQL, a shell, or a deserialiser from user input |

### What is out of scope

- Vulnerabilities in third-party dependencies with no demonstrated impact here — report those upstream
- Running the project against real money or as an unlicensed operator
- Denial of service by design (there is a rate-limiting issue for that)
- Findings that require an attacker who already has host or CI access

## Response

| Stage | Target |
| --- | --- |
| Acknowledgement | 72 hours |
| Triage (severity assigned) | 7 days |
| Fix or mitigation for `critical` | 14 days |
| Public write-up | On fix, credited unless you prefer otherwise |

Severity follows impact on money and on the integrity of the audit trail. A race condition that can produce a negative balance is `critical` regardless of how hard it is to reproduce.

## Design commitments

These are not aspirations; they are the tracked questions, and the work is public.

- **[Q10](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/10) / [Q45](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/45)** — no double-spend, no negative balance. Validation and deduction are one atomic step, not a read followed by a write. Both are `priority:critical` and are written as `bug` issues, because they are defects that would otherwise ship by accident.
- **[Q42](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/42)** — a client cannot choose its own odds. Execution requires a short-lived, server-signed quote token.
- **[Q46](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/46)** — no replay. Single-use nonce plus a bounded timestamp window plus a request signature.
- **[Q47](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/47)** — every state change is audited into an append-only, hash-chained log that the application role cannot write to.
- **[Q35](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/35)** — a kill switch halts all bet acceptance platform-wide within seconds, with audited trigger authority.
- **[Q03](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/3)** — the ledger is append-only. There is no code path that corrects a posted entry by mutating it, so a privileged bug cannot silently rewrite a balance.

## Not a licensed operator

This project publishes no odds, accepts no bets, handles no real money, and uses synthetic fixtures only. Any real-world deployment requires licensing in the relevant jurisdiction. Regulatory requirements are tracked as cited research in an ADR, not legal advice.
