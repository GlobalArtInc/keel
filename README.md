# keel

The framework under a line of control panels. A product of the line — KeelDNS, KeelPanel, later
KeelBill, KeelVM, KeelIP — is a set of modules composed on this core; every product ships in the
one static binary `keel` and is enabled per installation. The core itself knows nothing about
zones, sites or tariffs: it knows how to run a named function, who is calling it, how to describe
its screen to the frontend, and how products discover and trust each other.

Backend in Rust; frontend in Angular on `@globalart/phoenix-ui`; Postgres on the host, one
database per host; one binary `keel` per host with the frontend embedded.

## Status

Designed, not yet built. The design is in `docs/design/`, the order of work in `docs/plan.md`.

| Milestone | Scope | State |
|---|---|---|
| M0 | Walking skeleton: types, SDK, kernel, dispatcher, storage, control socket, `demo` bundle | not started |
| M1 | Identity, access and audit: `auth`, `settings`, `audit`, conformance skeleton | |
| M2 | Frontend runtime `@globalart/keel` and the host agent spike | |
| M3 | Background work and feedback: `tasks`, `cron`, `notify`, `webhooks`, SSE, TLS | |
| M4 | Operator plane, `keel-host`, KeelDNS alpha | |
| M5 | KeelPanel, `license`, `stats`, report and dashboard renderers | |
| M6 | Products working together: `peers`, events, services, `keel-client` | |
| M7 | External plugins, SDK 1.0, product module crates may move to their own repositories | |
| M8 | Cluster of nodes | |

## Layout

```
crates/      keel-types, keel-sdk, keel-kernel, keel-system, keel-host, ... (from M0)
products/    demo, dns (KeelDNS), panel (KeelPanel) — one bundle (lib crate) each
bin/         keel — the binary: the explicit composition of every module and bundle
frontend/    pnpm workspace: @globalart/keel, the shell application and the product view
             libraries (from M2)
contracts/   protocol.md, golden fixtures, generated openapi.json
deploy/      install.sh, systemd units, sample configs (from M4)
docs/        design and plan
hack/        scripts CI runs
```

## Documents

- `docs/design/01-overview.md` — what the core is, layers, repository and versioning
- `docs/design/02-functions.md` — functions, descriptors, the dispatcher pipeline
- `docs/design/03-access.md` — accounts, principals, levels, scoping, limits
- `docs/design/04-storage.md` — Postgres, migrations, identifiers, secrets
- `docs/design/05-modules.md` — modules, the SDK, system modules, tooling
- `docs/design/06-host.md` — the root boundary and the host agent
- `docs/design/07-ecosystem.md` — peers, events, services, shared identity
- `docs/design/08-frontend.md` — the frontend library and its boundary with phoenix-ui
- `docs/design/09-operations.md` — delivery, systemd, updates, backups, support
- `docs/design/10-contracts.md` — wire contracts with examples
- `docs/design/11-decisions.md` — decisions, alternatives, risks, open questions
- `docs/plan.md` — milestones and the M0 work breakdown
