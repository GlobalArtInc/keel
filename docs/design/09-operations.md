# Delivery and operations

This document describes how Keel reaches a customer's server and is operated there: the shape
of the binary, the filesystem layout, Postgres as part of the delivery, the systemd units and the
privilege boundary between them, installation and first start, TLS, the startup configuration,
updates and rollback, backup and restore, observability, and the diagnostic tooling. One delivery
loop serves every product of the line, including products that never touch the operating system.

## One installation per host

Keel is installed once per host: one binary `keel`, one process, one database, one privileged
agent, one set of directories and units, one update. A product is installed by enabling its
bundle on that installation: `install.sh --product <id>[,<id>]` on a clean server,
`keel product enable <id>` on an existing installation; `keel product disable <id>` stops loading
the product's modules and leaves their tables in place as orphaned. A customer may install
KeelPanel alone; adding KeelDNS to the same host later is one `enable` and one restart. The
delivery loop is the same whatever is enabled: a host on which no enabled product manages the
operating system runs the privileged unit with the `update` policy alone
([06-host.md](06-host.md)).

Keel is installed only on a clean server, never in a container: `install.sh` refuses a host that
already carries `/opt/keel` or `/var/lib/keel` (use `keel product enable`) and refuses to run
inside a container, where the path-unit bridge for updates does not work. Container delivery and
Windows are outside the project ([01-overview.md](01-overview.md)).

## The binary

Keel is one static binary `keel` carrying the kernel, the system modules and the module sets of
every first-party product, with the one shell build embedded:

- Built for musl with rustls. The frontend is embedded through rust-embed, brotli-compressed,
  served as immutable assets addressed by content hash; it is the one shell application with the
  views of every product, and the shell loads only the products `/boot` names
  ([08-frontend.md](08-frontend.md)).
- No C libraries. CI checks the binary with `ldd`: systemd is driven by executing `systemctl`
  rather than through D-Bus, SFTP is done with russh, and there is no libssh2, openssl or
  librdkafka. Any C dependency breaks the static musl build, so "no C libraries" is a rule backed
  by the CI check rather than a preference.
- Translation bundles are exported from OwnLate at build time and baked into the binary; the
  customer's server is assumed to be offline with respect to the translation service.
- The M2 acceptance criterion caps the binary with the embedded frontend at 40 MB
  ([../plan.md](../plan.md)).

The binary exposes the following subcommands:

| Subcommand       | Runs as      | Purpose                                                              |
|------------------|--------------|----------------------------------------------------------------------|
| `serve`          | `keel`       | the kernel process: HTTP/TLS listener, control socket, modules       |
| `ctl`            | local caller | calls functions over the control socket; the caller is identified by peer credentials |
| `privileged`     | root         | the single privileged code path: policies `update` and `host`        |
| `migrate`        | `keel`       | migration operations                                                 |
| `product`        | `keel`       | lists, enables and disables product bundles                          |
| `backup`         | `keel`       | produces the backup archive                                          |
| `restore`        | `keel`       | restores an archive with the service stopped                         |
| `doctor`         | `keel`       | checks the installation                                              |
| `support-bundle` | `keel`       | collects a redacted diagnostic archive                               |

`product` is a client of `keel.product.*` over the control socket while the service runs (the
call checks the lease and requests a restart), and a direct write of `keel_kv.products` under the
`keelinst` lock while it is stopped; the enabled set is read once at boot. At M0 only the direct
path exists: `product enable|disable` refuses to run while something answers on the socket, and
the socket path, the lease check and the restart are the operator plane's work in M4 and M5
([05-modules.md](05-modules.md)). `product list` prints every bundle the binary carries as
`<id>\t<enabled|disabled>\t<version>`, and an enabled id the binary does not carry as
`not carried by this binary`; `serve` refuses to start in that state, naming the id and the
bundles it knows. `migrate` runs the kernel's migrations, then those of the loaded modules, and
prints the applied migration names per module (`kernel: applied …`, `note: up to date`) and the
modules left orphaned. The `serve` subcommand creates the socket's parent directory when it is
missing, next to `secret.key` and `instance.id` under `data_dir` at first start.

`ctl` uses the same call envelope as HTTP ([10-contracts.md](10-contracts.md)); the level granted
over the local socket is `owner` ([03-access.md](03-access.md)):

```
CLI:  keel ctl dns.zone.edit id=01J9Q3… ttl=300 --as alice --out json     (same envelope; --out table by ui.columns)
```

The parameter syntax of `ctl` (`a.b=v`, repeated keys, `@file.json`, or JSON only) is an open
question ([11-decisions.md](11-decisions.md)); M0 implements `key=value` pairs whose value is
JSON when it parses and a string otherwise, with the last of repeated keys winning, plus
`--out json|table`, `--lang` for the table headers and `--socket` for another socket path. The
global `--config <path>`, or `KEEL_CONFIG`, names the configuration file of every subcommand.
Exit codes are `0` for a successful call, `1` for an error envelope or a failure and `2` for a
usage error; the socket protocol behind `ctl` is in [10-contracts.md](10-contracts.md).

Tables left behind by a disabled product, or by a module the binary no longer composes, stay in
the database as `orphaned`; the explicit schema prune that removes them is a deliberate operator
action, not part of a routine migration run ([04-storage.md](04-storage.md)). Which command hosts
it is not fixed here.

## Filesystem layout

One installation per host; nothing is namespaced by product:

| Path                        | Contents                                                          |
|-----------------------------|-------------------------------------------------------------------|
| `/opt/keel/versions/<ver>/` | one unpacked release per version                                  |
| `/opt/keel/bin/current`     | symlink to the active version; switched by the update             |
| `/etc/keel/config.yaml`     | startup configuration; may be absent                              |
| `/etc/keel/agent.yaml`      | host agent policy, one file with a section per product            |
| `/etc/keel/license.lease`   | signed license lease listing the products and their editions      |
| `/etc/keel/templates/`      | operator overrides of `ConfigTemplate` contributions              |
| `/var/lib/keel/secret.key`  | AES-GCM key for `Secret` values, mode 0600, generated at first start |
| `/var/lib/keel/instance.id` | instance identity                                                 |
| `/var/lib/keel/tls/`        | self-signed or operator-supplied certificate and key              |
| `/var/lib/keel/backups/`    | backup archives and the pre-update dump                           |
| `/var/lib/keel/plugins/`    | installed plugins                                                 |
| `/var/lib/keel/tasks/`      | task logs, one file per task                                      |
| `/run/keel/core.sock`       | control socket of the kernel (`ctl`)                              |
| `/run/keel/agent.sock`      | host agent socket                                                 |
| database `keel`             | all tables of every enabled product, in the host's Postgres       |

Products on one host share the unix user, the sockets, the listen port, the session cookie
`__Host-keel_session` and the database. What remains per product is the drop-in prefix
`keel-<product>-` for configuration files it owns and the operating-system user prefix, both in
the product's section of `agent.yaml` ([06-host.md](06-host.md)).

## Postgres as part of the delivery

Postgres is the only storage dialect of the line ([04-storage.md](04-storage.md)). Because Keel is
installed on the customer's own server, Postgres becomes part of what is delivered:

- `install.sh` installs Postgres from the distribution's packages if no server is present;
  if a server is already there it only creates the role `keel` and the database `keel`. The
  minimum version is 15, which Debian 12, Ubuntu 24.04 and RHEL 9 ship.
- The kernel connects over the unix socket with peer authentication under the unix user `keel`;
  the default connection string is `postgres:///keel?host=/run/postgresql`. An external database
  — a managed service or a shared server — is the same `database` parameter with a TCP URL and
  the password taken from `${KEEL_DB_PASS}`. The pool holds 20 connections, connect and acquire
  timeouts are 10 s, and `search_path` is set to the installation's schema `keel`.
- Keel never manages a server it does not own: it touches only its own role and database.
- An update does not touch Postgres at all.
- Leadership for cron and the single task worker are taken with `pg_advisory_lock`; migrations
  run under a session lock before the port opens.
- `doctor` checks the server version, the socket, the role's rights and free space. An
  unreachable database raises a problem on the dashboard ([05-modules.md](05-modules.md)).

This is a recognised risk: one more service on the customer's server, with a version chosen by the
distribution, its own memory footprint, possibly a pre-existing server configured by someone else,
and a major Postgres upgrade arriving together with an operating-system upgrade. The measures are
the minimum version check in `doctor`, the rule that Keel manages only its own role and database,
the logical dump before every update, and the dashboard problem when the database is unavailable.
Whether to keep installing from distribution packages or to ship a private Postgres under
`/opt/keel/pg` is an open question (see the end of this document).

## Processes and privilege

The kernel process is never root. Each installation has exactly one privileged code path: the
`privileged` subcommand of the same binary, run under a root unit with policies. The `update`
policy is always present; the `host` policy is used only while an enabled product names the
`host` module. The `update` system module talks to the root unit through a request file, never
through `keel-host`.

| Unit                      | User   | Role                                                   |
|---------------------------|--------|--------------------------------------------------------|
| `keel.service`            | `keel` | the kernel (`serve`); `Type=notify`, `WatchdogSec`     |
| `keel-privileged.service` | root   | the privileged code path (`privileged`)                |
| `keel-update.path`        | root   | watches `update.request` and triggers the update       |

The kernel unit is hardened: `NoNewPrivileges`, `ProtectSystem=strict`, a capability bounding set
of `CAP_NET_BIND_SERVICE` only, `MemoryMax` and `LimitNOFILE`. Unit files ship with the release
and are refreshed by the update.

`keel.service`:

```
[Service]
Type=notify
User=keel
Group=keel
ExecStart=/opt/keel/bin/current serve
Restart=always
WatchdogSec=60
StateDirectory=keel
RuntimeDirectory=keel
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/keel /run/keel
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
AmbientCapabilities=CAP_NET_BIND_SERVICE
```

`keel-privileged.service`, always installed; it serves the host catalogue only while an enabled
product names `host` and the `update` policy otherwise:

```
[Service]
ExecStart=/opt/keel/bin/current privileged --socket /run/keel/agent.sock --policy /etc/keel/agent.yaml
User=root
Restart=always
```

`keel-update.path`, present in every installation, starts a root oneshot service when
`update.request` appears:

```
[Path]
PathExists=/var/lib/keel/update.request
[Service]
Type=oneshot
ExecStart=/opt/keel/bin/current privileged update --from-request
```

The request carries the version, the channel and the requesting principal, for example
`{"version":"1.3.0","channel":"stable","requested_by":"admin"}`; the root side downloads
`versions/1.3.0`, verifies the Ed25519 signature and the sha256, switches the symlink, refreshes
the unit files from the release and restarts the service. The design fixes the three unit names,
the file the path unit watches and the order of steps in the update; the flag spellings above are
illustrative. The host agent's policy file `agent.yaml`,
its operation catalogue and its impersonation rules are described in [06-host.md](06-host.md).

## Installation and first start

`install.sh --product <id>[,<id>]` refuses a host that already has an installation and refuses a
container (see above), then performs, in order:

1. Verification of the Ed25519 signature of the release manifest.
2. Postgres from the distribution's packages if no server is present; otherwise nothing is
   installed.
3. The Postgres role `keel` and the database `keel`.
4. The unix user and group `keel`, including the uid range the host agent is allowed to manage
   ([06-host.md](06-host.md)).
5. The directories listed above.
6. The systemd units.
7. A self-signed TLS certificate generated with rcgen.
8. `keel product enable <id>` for each requested product, run as `keel` against the fresh
   database before the first start; the kernel migrations run there.

On first start the kernel generates `secret.key` and `instance.id`, reads the enabled products,
runs the kernel migrations and then the migrations of the loaded modules in `requires` order
under a session lock, and only then opens the port ([04-storage.md](04-storage.md),
[05-modules.md](05-modules.md)). It prints a one-time link with a `kok_` login key to
`keel.setup.admin`, a guest-level function that exists only while the installation has no
administrator. The binary starts without `config.yaml` against the default database; that is an
M0 acceptance criterion.

Installing a second product later is `keel product enable <id>` followed by the restart it
requests; the restart runs the same drain and pre-change logical dump as an update, and the new
product's modules migrate before the port opens.

`uninstall.sh --purge` removes the installation together with its data.

The installer lives under `deploy/` ahead of M4, in the subset the current binary supports:
the clean-host and container checks, Postgres from the distribution, the role and the
database, the unix user, the directories, `keel.service`, the enabled products and the first
administrator created over the control socket in place of the setup link. Signature
verification, TLS, the privileged unit, the update path-unit and `keel.setup.admin` join it
with the milestones that build them, and `deploy/README.md` names what is missing at any
time. Until the kernel speaks `sd_notify`, the shipped unit is `Type=simple` without
`WatchdogSec` and the installer waits for `/readyz`.

The M4 acceptance test for this section is: on a clean Debian or Ubuntu,
`curl | sh -s -- --product dns` installs Keel with `dns` enabled, and no process except the
privileged unit runs as root; at M5 `keel product enable panel` on the same host yields one login
that sees both products ([../plan.md](../plan.md)).

## TLS

Keel listens on TLS from its very first start. The session cookie carries the `__Host-`
prefix ([03-access.md](03-access.md)), and a browser does not store such a cookie for a plain
HTTP origin — a first visit over plain HTTP by IP address would leave the operator unable to log
in. The plain port therefore serves nothing but a redirect. The operator can supply a certificate
and key or request one through ACME with a button in the settings; `trusted_proxies` makes the
installation usable behind a reverse proxy. A certificate that is about to expire raises a problem
([05-modules.md](05-modules.md)). The self-signed certificate and `trusted_proxies` arrive with
M3; until then the installation listens on plain HTTP, the cookie carries `Secure` from its first
version, and the clients of M1 keep and return it themselves, since a browser is not a client
before M2.

## Configuration

`config.yaml` holds only what the process cannot start without; the file may be absent. Its keys
are `listen`, `socket`, `data_dir`, `database`, `tls`, `trusted_proxies` and `log`. A module
receives its own section through a `ConfigSection` contribution. Everything that is edited from the
UI lives in the `settings` module, not in the file.

```yaml
listen: 0.0.0.0:8443
socket: /run/keel/core.sock
data_dir: /var/lib/keel
database: postgres:///keel?host=/run/postgresql
tls: { cert: "", key: "", acme: "" }
trusted_proxies: []
log: { level: info, format: json }
```

Every key is optional. `database` may instead be a TCP URL such as
`postgres://keel:${KEEL_DB_PASS}@db.example.net/keel`; an empty `tls` means the self-signed
certificate under `<data_dir>/tls`. The values above for `log` are also its defaults. The defaults
of `socket`, `data_dir` and `database` are fixed by the kernel, not by a product bundle. The
enabled products are not a configuration key: they live in the database. Sections of modules
(`host`, `plugins`) are added by their `ConfigSection` contributions; their keys are not fixed by
this revision ([06-host.md](06-host.md)). The kernel keeps every top-level key it does not know as
a raw section for those contributions and matches them at freeze: a section is validated
against the schema of the type the contribution names and a violation fails the start naming the
file, the section and the path, an absent section becomes the type's default, a section claimed
by two modules is a freeze error, and a top-level key no loaded module claims is logged as a
warning, so that the section of a disabled product stays valid. A misspelt key inside `tls` or
`log`, an unusable `listen` or `database` value, or a file that is not YAML fails the start with
a message naming the file and the key. An empty file is the same as no file. The default listen
port of the installation is open ([11-decisions.md](11-decisions.md)); the value above is only an
example, and M0 keeps the provisional default `127.0.0.1:1500` while the port is undecided. The
body limit of a request is a kernel constant, 1 MiB, not a key of this file. The runtime binds
the listener before the kernel boots and attaches the dispatcher after it, so `/healthz` answers
`{ "status": "ok" }` from the first moment, `/readyz` reports `migrating` (and later `draining`,
or `database` when `SELECT 1` fails) with `503` until it reports `ready`, and `/api/v1/*`
answers `503 unavailable` in between ([10-contracts.md](10-contracts.md)); the readiness flags
belong to the runtime, not to a module.

## Updates

Releases are published on a channel described by a signed `manifest.json` with `version`,
`sha256`, `min_upgrade_from`, `contract` and `notes`. An update runs in two halves separated by the
privilege boundary:

1. The `update` module compares the running version with the channel manifest. Anti-rollback is
   enforced by the minimum version stated in the channel manifest.
2. The kernel drains the task queue ([05-modules.md](05-modules.md)) and takes a logical dump of
   the database: `COPY … TO STDOUT` over its own connection into `backups/`, so the unprivileged
   process does not need to invoke `pg_dump`. This dump is taken before every update.
3. The kernel writes `update.request` with the requested version, channel and the requesting
   principal.
4. `keel-update.path` fires. The root code path downloads `versions/<ver>`, verifies the
   Ed25519 signature and the sha256 from the manifest, switches the `bin/current` symlink,
   refreshes the unit files from the release and restarts the service. The root side knows one
   operation only — install signed version X — and takes no commands from the request beyond
   the version to install.
5. The new binary starts, runs its migrations before opening the port, marks tasks that were
   running as `interrupted` and restarts only those declared resumable.

There is one release train: the channel manifest carries one `version`, and an update moves every
enabled product at once; rollback moves all of them back; the pre-update dump covers every loaded
module's tables.

Postgres is not touched by an update. Migrations within a major are additive (add table, column
or index; no rename or drop), so `update rollback` within a major switches the symlink back
without down migrations. Crossing a contract boundary backwards is possible only with
`--restore`, that is by restoring the pre-update backup.

The signing key is kept offline; `previous_keys` compiled into the binary allow key rotation.
Compromise of the signing key would compromise every installation, which is why the root side is
limited to a single operation and does not read commands from the request. The M4 acceptance test
is an update from 0.1 to 0.2 by a button in the UI and a rollback without losing the database.

## Backup and restore

A backup is a `tar.zst` archive with a manifest recording the keel version, the enabled products
with their editions and the schema versions of the modules. It contains:

- the database: a logical `COPY` dump per module table, or a `pg_dump` if one is present on the
  host;
- `/etc/keel/` in full, plugins, brand assets and TLS material;
- contributions of every loaded module through `BackupSource` (for example zone files contributed
  by `dns-zones`).

Targets are `local` and S3, reached through `keel-storage`. API tokens and sessions are stored as
SHA-256 hashes, so an archive contains no usable keys. `secret.key` is included only with
`--with-secret-key`, protected by a passphrase; values of type `Secret` in the dump are readable on
another host only with that key. The M4 acceptance test is a backup taken on one VM and restored on
another with two-factor authentication still working.

`restore` runs with the service stopped and consults a compatibility table by major version. The
enabled set is restored with the database, because it lives in `keel_kv`; `restore` refuses a
binary that does not carry one of the bundles the manifest names, and the next start reports what
the restored set changed. Restore from the UI is a task that ends with a restart.

## Observability

- Logs go through `tracing` in JSON or text, selected by `log.format`. Log levels can be changed
  per module at runtime. A ring buffer of 10 000 entries backs the `keel.logs.tail` function and
  the support bundle.
- Every response carries `X-Keel-Request-Id`; the host agent writes every operation to journald
  with the actor and the request id ([06-host.md](06-host.md)).
- Call counters and latency histograms are exposed in the text exposition format. OTLP export is
  a cargo feature.
- `/healthz` reports liveness. `/readyz` reports readiness: the database is reachable, migrations
  are applied, and the process is not draining.
- The `notify` module keeps a problem registry with idempotent raise and clear for disk space,
  TLS, license, migrations, the outbox and cluster nodes; open problems are counted in `/boot`
  and shown on the dashboard ([05-modules.md](05-modules.md)).

## Diagnostics and support

`doctor` checks the installation end to end: directory permissions, units, TLS, the database
(version, socket, role rights), migrations, the host agent, the license and free space; that every
enabled product is carried by the binary and listed by the lease; and the orphaned modules, if
any.

`support-bundle` collects the configuration with secrets redacted, logs, the migration ledger, the
tail of the audit log, open problems and metrics. Redaction uses the same `secret` flag of the
field schema that masks values in logs and audit ([02-functions.md](02-functions.md)), and CI runs
a test that the bundle contains no secrets; the M5 acceptance criteria repeat that check.

## Open questions

These items from [11-decisions.md](11-decisions.md) affect this document directly and are not
decided:

- Whether Postgres is installed from the distribution's packages (one dependency, upgraded with
  the operating system) or shipped privately under `/opt/keel/pg` (one version everywhere, but
  its own upgrade path and responsibility for the data).
- The default listen port of the installation.
- The license server: the first releases run on long offline leases issued by hand; when the
  server is written as a Keel product and where the signing key is stored remain open.
