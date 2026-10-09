# Documentation

Everything written down about this system lives here. If it is not in this
directory, it is either code or a GitHub issue.

## Where to start

| If you want to… | Read |
| --- | --- |
| Understand the current design and what is still undecided | [`architecture/README.md`](architecture/README.md) |
| See the open design questions and what each one blocks | [`design/open-questions.md`](design/open-questions.md) |
| See what gets built, in what order | [`roadmap.md`](roadmap.md) |
| Understand why a decision was made | [`adr/`](adr/) |
| Contribute code | [`../.github/CONTRIBUTING.md`](../.github/CONTRIBUTING.md) |
| Report a vulnerability | [`../.github/SECURITY.md`](../.github/SECURITY.md) |

## Layout

```
docs/
├── README.md                    this file
├── architecture/
│   └── README.md                the dependency spine, open decisions, committed constraints
├── adr/                         architecture decision records, one file per decision
│   ├── README.md                index and process
│   └── 0000-template.md         copy this to start a new decision
├── design/
│   └── open-questions.md        the 58 open design questions, each linked to its issue
└── roadmap.md                   the 260-issue implementation plan
```

Two directories are referenced by the architecture document and are **not yet
written**, because the questions that unblock them are still open:

| Planned document | Blocked by |
| --- | --- |
| `docs/schema.md` — ERD, and the immutable/mutable column split | [Q02](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/2) |
| `docs/slos.md` — SLIs, SLOs, error budgets | [Q48](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/48) |
| `runbooks/` — cap breach, quarantined market, ledger imbalance, kill switch | [Q35](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/35), [Q51](https://github.com/BerlinDeskMudit/ladywalk-odds-engine/issues/51) |

They are listed as unwritten rather than stubbed, so that a missing document is
visible in review instead of being mistaken for an empty one.

## Conventions

- **A decision is an ADR, not a paragraph.** Anything that constrains future work
  gets a file in [`adr/`](adr/) and a link from the change that implements it.
- **A question is open until its issue closes with an artefact.** See
  [`../.github/CONTRIBUTING.md`](../.github/CONTRIBUTING.md).
- **Generated files are generated.** `design/open-questions.md` and `roadmap.md`
  are produced from the issue data. CI fails if they drift from it, so do not
  hand-edit them; change the source and regenerate.
- **Link to the issue, not to a copy of it.** A second copy of a decision is a
  second thing that can go stale.
