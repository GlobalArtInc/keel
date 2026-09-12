# Working in this repository

Keel is the framework every product of the line is built on. Read `docs/design/01-overview.md`
before touching anything; the rest of `docs/design/` is the specification, and `docs/plan.md` is
the order of work. When code and design disagree, fix one of them in the same change.

## Rules

- English everywhere that lands in git: code, docs, tests, CI, commit messages.
- No comments in code. Explain in the commit message or in `docs/`. The only exception is a link
  to an RFC next to the code that implements it.
- The project's own vocabulary only. Do not name other control-panel products in code, docs, or
  commit messages, and do not borrow their identifiers.
- Push straight to `main`. One task is one change; do not split a task across branches.
- Verify with the same commands CI runs before pushing: `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` against a local
  Postgres, and the frontend `pnpm lint && pnpm test && pnpm build` once `frontend/` exists.
- Postgres is the only database. Tests get a database per test from a template; no in-memory
  substitutes.

## Boundaries that CI enforces

- `keel-kernel` depends on neither `keel-system` nor `keel-host`.
- A module depends on `keel-sdk` and on the `api` module of crates named in its `requires`,
  nothing else.
- Every table, function, permission key, setting and i18n key of a module carries the module id
  as its prefix.
- Migrations inside a major version are additive only.

## Names

Platform `keel`; crates `keel-*`; npm `@globalart/keel`. Products carry a brand for people and
an id for machines: KeelDNS is `dns`, KeelPanel is `panel`, KeelBill is `bill`; `demo` is the
dogfood bundle, compiled in only behind the `demo` feature. One installation per host: binary and
CLI `keel`, unix user `keel`, `/opt/keel`, `/etc/keel`, `/var/lib/keel`, `/run/keel`, database
and schema `keel`, cookie `__Host-keel_session`. Product ids name bundles, module groups, menu
groups, license features, the `--product` flag and the prefixes `keel-<product>-` in shared
include directories.
