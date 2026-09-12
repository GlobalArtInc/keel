# Storage and migrations

This document specifies the storage layer of Keel: Postgres as the only database engine, how it is
delivered together with a product and how a product connects to it, the identifier and column
conventions, schema migrations with sqlmig and the additive-only policy, the isolation rules between
module tables, encrypted secrets, and the eight tables the kernel owns. The dispatcher pipeline that
opens and commits transactions is specified in [02-functions.md](02-functions.md), the accounts and
principals that live in the kernel tables in [03-access.md](03-access.md), and installation, updates
and backups in [09-operations.md](09-operations.md).

## One database engine

Every product of the line keeps its data in Postgres. A panel is installed on the customer's own
server, so Postgres is part of the delivery: `install.sh` installs it from the distribution's
packages, creates a role and a database for the product, and the panel connects over the unix
socket with peer authentication. Several products on one host share one Postgres server but never
a database: each product owns `keel_<product>` (`keel_dns`, `keel_panel`).

Database access goes through sea-orm 1.1 (`sqlx-postgres`, `runtime-tokio-rustls`); modules
contain no raw sqlx. The connection pool belongs to `keel-kernel`. `keel-sdk` knows nothing about
the pool: a module receives only the transaction of the current call through `call.tx()`. The
`keel-storage` crate is the library of backup targets (local, S3) and has nothing to do with the
database; see [09-operations.md](09-operations.md).

A single engine for the whole line means one SQL dialect, one migrator without extensions, and one
test matrix instead of the whole class of tests that would have to run against two engines. The
alternatives considered and the review notes are in [11-decisions.md](11-decisions.md). The price is
one more service on the customer's server; the mitigations are listed below.

## Delivery of Postgres

- The minimum version is 15; Debian 12, Ubuntu 24.04 and RHEL 9 ship it.
- `install.sh` installs the server package only when the host has no Postgres server; otherwise it
  only creates the product's role and database. The role matches the product's unix user
  `keel-<product>`, so peer authentication over the unix socket needs no password.
- A panel update never touches Postgres.
- The panel never manages a server it does not own. It owns its role and its database and nothing
  else; a pre-existing server with its own settings is left as it is.
- `doctor` checks the Postgres version, the socket, the privileges of the role and free disk space.
  `/readyz` reports the database and the state of migrations.
- When the database is unreachable the panel raises a problem on the dashboard through the notify
  module (see [05-modules.md](05-modules.md)).
- An external database, whether a managed service or a shared server, is configured through the
  same `database` parameter with a TCP URL.

The operational risks of shipping Postgres are known: the version comes from the distribution,
the server takes memory, a server that is already on the host may be configured differently, and a
major upgrade of Postgres arrives with an upgrade of the operating system. The minimum version and
the `doctor` checks, the rule of never managing a foreign server, the logical dump before every
update and the dashboard problem on an unreachable database are the mitigations. Whether to install
Postgres from distribution packages or to ship a private build under `/opt/keel/pg` remains open;
see "Open points" below and [11-decisions.md](11-decisions.md).

## Connection

The `database` key of `/etc/keel/<product>/config.yaml` holds the connection URL. The file may be
absent; the default for dns is:

```yaml
database: postgres:///keel_dns?host=/run/postgresql
```

That is the unix socket with peer authentication under the uid `keel-dns`. For an external
database the value is a TCP URL, and the password comes from the environment variable
`${<PRODUCT>_DB_PASS}` (`DNS_DB_PASS` for dns) rather than from the file. A binary started
without `config.yaml` connects to the default database; this is an M0 acceptance criterion
([docs/plan.md](../plan.md)).

Pool parameters: 20 connections, connect and acquire timeout 10 s, `search_path` set to the
product's schema.

Transactions are opened by the kernel, not by handlers. The rules, in full in
[02-functions.md](02-functions.md):

- A function with `mode: write` runs inside a transaction that the kernel opens in `Call`. Modules
  never call `begin()` themselves. A nested `call.invoke` reuses the transaction of the outer call.
  The outbox row and the idempotency record are written in the same transaction, and `within` hooks
  (quotas, limits, invariant checks) run inside it.
- A function with `mode: read` has no transaction: it takes one connection from the pool, with no
  idempotency and no outbox.
- `before` hooks run outside the transaction with a timeout. Operations on the operating system and
  calls to peer products happen only in `after_commit` or as a task: a transaction never waits for
  the host agent or for the network.
- Audit is an in-process event after commit or rollback; the audit module writes in batches from a
  background worker so that the response of a call is not extended.

The cron leader and the single task worker are elected with `pg_advisory_lock`; the design names
no other coordination mechanism.

## Identifiers, time and JSON

- Identifiers are ULIDs stored as `TEXT` everywhere, 26 characters, for example
  `01J9Q3W6B1R8N5X2K4T7M0V9Z3`. They are stable across peers, import and export, and they sort by
  creation time. Every function key, every foreign key and every id on the wire is such a ULID.
- JSON columns are `jsonb`.
- Time columns are `timestamptz`. On the wire time is RFC 3339 in UTC. The time zone of a person is
  kept in `settings_pref` and returned by `/boot`; date filters compute the boundary of a day in the
  time zone of the principal, and the client converts RFC 3339 to local dates by the same time zone
  (see [08-frontend.md](08-frontend.md)).

## Migrations

Migrations are run by sqlmig 0.2 as it is; the migrator is not extended. Each module embeds its
own migrations directory and keeps its own ledger table `keel_migration_<module>`
(`keel_migration_auth`, `keel_migration_tasks`); sqlmig already accepts the ledger table name as a
parameter, which is why one ledger per module needs no change in the migrator. The kernel's own
migrations are recorded in `keel_migration`. The layout of a ledger table is sqlmig's own and is
not part of the Keel schema contract.

A module declares its migrations in `ModuleDef`:

```rust
pub static MODULE: ModuleDef = ModuleDef {
    id: "dns", version: env!("CARGO_PKG_VERSION"), sdk: keel_sdk::VERSION,
    requires: &["auth", "tasks", "host"],
    migrations: Some(&MIGRATIONS),           // migrator::embed!("$CARGO_MANIFEST_DIR/migrations")
    init,
};
```

At start the kernel applies migrations in the order kernel, then modules in topological order of
`requires`, before the port is opened and before any module `init` runs, under a session-level
lock so that two processes never migrate concurrently. The full boot sequence is in
[05-modules.md](05-modules.md). The product binary exposes a `migrate` subcommand for running this
step on its own, and `keel-dev migrate create` creates a new migration file in a module.

The schema contract lives in `keel-types` and in `keel-conformance`, not in SQL markers: a
migration file carries no annotations about compatibility, and what a table must contain for the
protocol to hold is checked by the type fixtures and the conformance suite.

## Additive-only policy

Within one major version migrations are additive only: they may add a table, a column or an index
and may never rename or drop one. An older binary therefore runs against a newer schema without
noticing, which is what makes a rollback of the binary possible without down files. `keel-dev
check` lints migration files for this rule, and CI runs the check on every change.

A change that breaks the schema is a major release. The release manifest of a channel carries a
`contract` field, and `update rollback` refuses to cross a contract boundary without `--restore`
from a backup; see [09-operations.md](09-operations.md).

Before every update the kernel takes a logical dump of the database through its own connection
using `COPY ... TO STDOUT`, table by table, so that no `pg_dump` has to be invoked from the
unprivileged process. The backup subcommand uses the same dump, or `pg_dump` when it is present on
the host; the archive layout and the restore procedure are in [09-operations.md](09-operations.md).

## Orphaned modules and schema prune

A module removed from a product leaves its tables in place. The kernel records the module in
`keel_module` with `state = orphaned` and keeps running; nothing is deleted implicitly. `schema
prune` removes the tables of an orphaned module as a deliberate operator action.

## Isolation between modules

- The prefix of every table is the id of the module that owns it (`auth_session`, `task_job`,
  `settings_pref`). `keel_` is reserved for the kernel.
- A foreign key from a module's table may point only to a table of a module named in its declared
  `requires`, or to `keel_account` and `keel_principal`. Any other cross-module foreign key is an
  error of `keel-dev check`.
- A module reads another module's data through `call.invoke` or through a service it requires,
  never through SQL against the other module's tables.
- Foreign keys to `keel_account` are `ON DELETE RESTRICT` by default. Deleting an account is a task
  that walks the modules in reverse order of `requires` through their `AccountLifecycle`
  contributions and deletes the row only afterwards; a module that forgot to clean up fails the
  operation instead of losing data. See [03-access.md](03-access.md).

## Secrets

A value that must not be readable from the database is typed `Secret` in the SDK. It is encrypted
with AES-GCM under the key from `/var/lib/keel/<product>/secret.key`, a file with mode 0600 that
the product generates at its first start; `keel_kv` holds the wrapped key. A copy of the
database without the matching `secret.key` cannot decrypt its `Secret` columns, which is why a
backup includes the key only optionally, under a passphrase (`--with-secret-key`, see
[09-operations.md](09-operations.md)).

Fields marked `x-keel.secret` in a function schema are the same fields that audit, logs and the
support bundle mask; see [02-functions.md](02-functions.md). Session and API tokens are not
`Secret` values: the auth module stores only their SHA-256 hashes, so a backup contains no usable
token ([05-modules.md](05-modules.md)).

## Kernel tables

The kernel owns exactly eight tables. Everything else belongs to a module and carries the module's
prefix.

| Table | Purpose | Key columns |
|---|---|---|
| `keel_account` | Tenant: the unit of ownership, a tree by materialized path | `id`, `kind`, `parent_id`, `path`, `state`, `origin`, `external_id` |
| `keel_principal` | Person or token holder, member of one account with a role | `id`, `account_id`, `login`, `level`, `role`, `state`, `lang`, `origin`, `external_id`, `acl_gen` |
| `keel_migration` | sqlmig ledger of the kernel's own migrations | sqlmig's own layout |
| `keel_module` | Registry of modules that ever ran against this database | `id`, `version`, `state` |
| `keel_kv` | Instance-level values: instance id, wrapped key, version history, monotonic time | `key`, `value` |
| `keel_outbox` | Events written in the call transaction, delivered at least once | `seq`, `id`, `type`, `actor_json`, `subject_json`, `data_json` |
| `keel_idempotency` | Recorded responses for `Idempotency-Key` replays | `principal_id`, `key`, `func`, `request_hash`, `status`, `response_json` |
| `keel_limit` | Per-account values of the limits modules contribute through `LimitDef`, inherited along `path` | `account_id`, `key`, `value` |

### keel_account

`id TEXT`, `kind TEXT`, `parent_id TEXT`, `path TEXT`, `state TEXT`, `origin TEXT`,
`external_id TEXT`, `created_at timestamptz`, `updated_at timestamptz`.

`path` is the materialized path of the account in the tree, so the scope of a reseller is one
`LIKE` on the prefix. The root account with `kind = system` and `path = /` is created by a kernel
migration; it owns the owner, the administrators, the service tokens and the principals of plugins
and peer products. The pair `(origin, external_id)` is the stable identity used by peers:
`keel.account.upsert` is idempotent by it. The semantics are in [03-access.md](03-access.md).

### keel_principal

`id TEXT`, `account_id TEXT`, `login TEXT`, `level TEXT`, `role TEXT`, `state TEXT`, `lang TEXT`,
`origin TEXT`, `external_id TEXT`, `acl_gen` (an integer counter), `created_at timestamptz`,
`updated_at timestamptz`.

At M0 a principal belongs to exactly one account through `account_id` and `role`. Sessions,
tokens and the response envelope carry the account explicitly, so N:M membership can be added
later as an additive table `keel_membership` without a change of protocol. `acl_gen` is
incremented in the same transaction that changes the principal's permissions, role or account,
and for the subtree of a reseller by `path`; it is part of the descriptor `ETag`
([10-contracts.md](10-contracts.md)). `keel.principal.upsert` is idempotent by
`(origin, external_id)`.

### keel_migration

The sqlmig ledger of the kernel's migrations. Module ledgers are the separate tables
`keel_migration_<module>` created by sqlmig for each module; they are not counted among the eight.

### keel_module

`id TEXT`, `version TEXT`, `state TEXT` (`active` or `orphaned`), `installed_at timestamptz`,
`updated_at timestamptz`.

One row per module id that has ever migrated this database. A module missing from the current
composition keeps its row with `state = orphaned` until `schema prune`.

### keel_kv

`key TEXT`, `value jsonb`. A small key-value store for values that exist once per installation:
the instance id, the wrapped secret key, the history of versions that ran against this database,
and the monotonic time that the license module uses against clock changes
([05-modules.md](05-modules.md)).

### keel_outbox

`seq` (monotonic, assigned by the database), `id TEXT` (ULID of the event), `type TEXT`,
`at timestamptz`, `actor_json jsonb`, `subject_json jsonb`, `data_json jsonb`,
`created_at timestamptz`.

A row is inserted inside the transaction of the call that produced the event, so an event exists
exactly when its change is committed. Delivery to webhooks, peer products and notifications reads
the outbox at least once. Every consumer keeps its own cursor over `seq`; retention never discards
a row that some consumer has not yet delivered. `seq` is also the `Last-Event-ID` of the SSE
stream, which lets a client catch up after a reconnect ([08-frontend.md](08-frontend.md)). Event
shapes are in [07-ecosystem.md](07-ecosystem.md).

### keel_idempotency

`principal_id TEXT`, `key TEXT`, `func TEXT`, `request_hash TEXT`, `status TEXT`,
`response_json jsonb`, `created_at timestamptz`, `expires_at timestamptz`; the record is keyed by
`(principal_id, key)`.

The kernel writes the record in the transaction of a write call that carries an `Idempotency-Key`.
A repeat with the same body is answered from `response_json` with the header
`X-Keel-Idempotent-Replay`; a repeat with a different body, detected through `request_hash`, is
rejected with `409 idempotency_mismatch`. Expired records are removed by retention.

## Testing against Postgres

There is no in-memory substitute. `keel-testkit` gives every test its own temporary database,
created with `CREATE DATABASE` from a template; CI provides the server through testcontainers from
M1. The CI matrix of `keel-system` ("every module alone and without each one in turn") and the
test "a product of one bare kernel starts and answers on the socket" start a product for each
composition and therefore run its migrations. See [05-modules.md](05-modules.md) and
[docs/plan.md](../plan.md).

## Open points

- Postgres on the host: install from the distribution's packages (proposed: one dependency,
  upgraded with the operating system) or ship a private build under `/opt/keel/pg` (the same
  version everywhere, but then Keel owns its updates and its data). Tracked in
  [11-decisions.md](11-decisions.md).
- Account membership: 1:N at M0 with an additive `keel_membership` later, or N:M from the start.
  The answer changes the meaning of `keel_principal.account_id`; see
  [11-decisions.md](11-decisions.md).
