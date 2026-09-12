# Delivery and operations

This document describes how a Keel product reaches a customer's server and is operated there: the
shape of the binary, the filesystem layout, Postgres as part of the delivery, the systemd units and
the privilege boundary between them, installation and first start, TLS, the startup configuration,
updates and rollback, backup and restore, observability, and the diagnostic tooling. One delivery
loop serves every product of the line, including products that never touch the operating system.

## One loop for every product

The delivery loop is the same for `dns`, `panel` and the later products. A product that
does not link `keel-host` (billing, address management) is installed by the same script, runs
under the same units, updates through the same path unit and is backed up by the same subcommand;
the only difference is that its privileged code path carries the `update` policy alone and no host
agent. That a product can ship without a host agent at all is what proves the kernel itself is
free of operating-system knowledge (see [06-host.md](06-host.md)).

Container delivery and Windows are outside the project ([01-overview.md](01-overview.md)); whether
a container image should be offered at all is an open question listed at the end of this
document.

## The binary

A product is one static binary:

- Built for musl with rustls. The frontend is embedded through rust-embed, brotli-compressed,
  served as immutable assets addressed by content hash. A product with no bespoke screens does not
  keep an Angular workspace at all: it embeds the prebuilt `@globalart/keel-shell-app` via
  `embed_frontend!` ([08-frontend.md](08-frontend.md)).
- No C libraries. CI checks the binary with `ldd`: systemd is driven by executing `systemctl`
  rather than through D-Bus, SFTP is done with russh, and there is no libssh2, openssl or
  librdkafka. Any C dependency breaks the static musl build, so "no C libraries" is a rule backed
  by the CI check rather than a preference.
- Translation bundles are exported from OwnLate at build time and baked into the binary; the
  customer's server is assumed to be offline with respect to the translation service.
- The M2 acceptance criterion caps the binary with the embedded frontend at 40 MB
  ([../plan.md](../plan.md)).

The binary exposes the following subcommands; the product for the examples in this document is
`dns`, so the binary is `keel-dns`.

| Subcommand       | Runs as        | Purpose                                                              |
|------------------|----------------|----------------------------------------------------------------------|
| `serve`          | `keel-dns` | the kernel process: HTTP/TLS listener, control socket, modules        |
| `ctl`            | local caller   | calls functions over the control socket; the caller is identified by peer credentials |
| `privileged`     | root           | the single privileged code path: policies `update` and `host`         |
| `migrate`        | `keel-dns` | migration operations                                                 |
| `backup`         | `keel-dns` | produces the backup archive                                          |
| `restore`        | `keel-dns` | restores an archive with the service stopped                          |
| `doctor`         | `keel-dns` | checks the installation                                              |
| `support-bundle` | `keel-dns` | collects a redacted diagnostic archive                                |

`ctl` uses the same call envelope as HTTP ([10-contracts.md](10-contracts.md)); the level granted
over the local socket is `owner` ([03-access.md](03-access.md)):

```
CLI:  keel-dns ctl dns.zone.edit id=01J9Q3… ttl=300 --as alice --out json     (same envelope; --out table by ui.columns)
```

The parameter syntax of `ctl` (`a.b=v`, repeated keys, `@file.json`, or JSON only) is an open
question ([11-decisions.md](11-decisions.md)).

Tables left behind by a module removed from the product stay in the database as `orphaned`; the
explicit schema prune that removes them is a deliberate operator action, not part of a routine
migration run ([04-storage.md](04-storage.md)). Which command hosts it is not fixed here.

## Filesystem layout

Every path is namespaced by product so that two products can share one host. For `dns`:

| Path                                 | Contents                                                          |
|--------------------------------------|-------------------------------------------------------------------|
| `/opt/keel/dns/versions/<ver>/`  | one unpacked release per version                                  |
| `/opt/keel/dns/bin/current`      | symlink to the active version; switched by the update             |
| `/etc/keel/dns/config.yaml`      | startup configuration; may be absent                              |
| `/etc/keel/dns/agent.yaml`       | host agent policy; only products that link `keel-host`            |
| `/etc/keel/dns/license.lease`    | signed license lease ([05-modules.md](05-modules.md))             |
| `/etc/keel/dns/templates/`       | operator overrides of `ConfigTemplate` contributions               |
| `/var/lib/keel/dns/secret.key`   | AES-GCM key for `Secret` values, mode 0600, generated at first start |
| `/var/lib/keel/dns/instance.id`  | instance identity                                                 |
| `/var/lib/keel/dns/tls/`         | self-signed or operator-supplied certificate and key              |
| `/var/lib/keel/dns/backups/`     | backup archives and the pre-update dump                           |
| `/var/lib/keel/dns/plugins/`     | installed plugins                                                 |
| `/var/lib/keel/dns/tasks/`       | task logs, one file per task                                      |
| `/run/keel/dns/core.sock`        | control socket of the kernel (`ctl`)                              |
| `/run/keel/dns/agent.sock`       | host agent socket; only products that link `keel-host`            |
| database `keel_dns`              | all tables, in the host's Postgres                                 |

Two products on one host get different unix users, sockets, ports, session cookie names
(`__Host-keel_<product>_session`) and drop-in prefixes for configuration files they own. They
share one Postgres server but never a database.

## Postgres as part of the delivery

Postgres is the only storage dialect of the line ([04-storage.md](04-storage.md)). Because a panel
is installed on the customer's own server, Postgres becomes part of what is delivered:

- `install.sh` installs Postgres from the distribution's packages if no server is present;
  if a server is already there it only creates the product's role and database. The minimum
  version is 15, which Debian 12, Ubuntu 24.04 and RHEL 9 ship.
- The panel connects over the unix socket with peer authentication under its own unix user
  (`keel-dns`); the default connection string is
  `postgres:///keel_dns?host=/run/postgresql`. An external database — a managed service or a
  shared server — is the same `database` parameter with a TCP URL and the password taken from
  `${DNS_DB_PASS}`. The pool holds 20 connections, connect and acquire timeouts are 10 s, and
  `search_path` is set to the product's schema.
- The panel never manages a server it does not own: it touches only its own role and database.
  Several products on one host share the server and keep separate databases.
- A panel update does not touch Postgres at all.
- Leadership for cron and the single task worker are taken with `pg_advisory_lock`; migrations
  run under a session lock before the port opens.
- `doctor` checks the server version, the socket, the role's rights and free space. An
  unreachable database raises a problem on the dashboard ([05-modules.md](05-modules.md)).

This is a recognised risk: one more service on the customer's server, with a version chosen by the
distribution, its own memory footprint, possibly a pre-existing server configured by someone else,
and a major Postgres upgrade arriving together with an operating-system upgrade. The measures are
the minimum version check in `doctor`, the rule that the panel manages only its own role and
database, the logical dump before every update, and the dashboard problem when the database is
unavailable. Whether to keep installing from distribution packages or to ship a private Postgres
under `/opt/keel/pg` is an open question (see the end of this document).

## Processes and privilege

The kernel process is never root. Each product has exactly one privileged code path: the
`privileged` subcommand of the same binary, run under a root unit with policies. The `update`
policy exists in every product; the `host` policy only in products that link `keel-host`. The
`update` system module talks to the root unit through a request file, never through `keel-host`.

| Unit                             | User           | Role                                                   |
|----------------------------------|----------------|--------------------------------------------------------|
| `keel-dns.service`           | `keel-dns` | the kernel (`serve`); `Type=notify`, `WatchdogSec`      |
| `keel-dns-privileged.service`| root           | the privileged code path (`privileged`)                 |
| `keel-dns-update.path`       | root           | watches `update.request` and triggers the update        |

The kernel unit is hardened: `NoNewPrivileges`, `ProtectSystem=strict`, a capability bounding set
of `CAP_NET_BIND_SERVICE` only, `MemoryMax` and `LimitNOFILE`. Unit files ship with the release
and are refreshed by the update.

`keel-dns.service`:

```
[Service]
Type=notify
User=keel-dns
Group=keel-dns
ExecStart=/opt/keel/dns/bin/current serve
Restart=always
WatchdogSec=60
StateDirectory=keel/dns
RuntimeDirectory=keel/dns
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/keel/dns /run/keel/dns
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
AmbientCapabilities=CAP_NET_BIND_SERVICE
```

`keel-dns-privileged.service`, present with the `host` policy only in products that link
`keel-host`:

```
[Service]
ExecStart=/opt/keel/dns/bin/current privileged --socket /run/keel/dns/agent.sock --policy /etc/keel/dns/agent.yaml
User=root
Restart=always
```

`keel-dns-update.path`, present in every product, starts a root oneshot service when
`update.request` appears:

```
[Path]
PathExists=/var/lib/keel/dns/update.request
[Service]
Type=oneshot
ExecStart=/opt/keel/dns/bin/current privileged update --from-request
```

The request carries the version, the channel and the requesting principal, for example
`{"version":"1.3.0","channel":"stable","requested_by":"admin"}`; the root side downloads
`versions/1.3.0`, verifies the Ed25519 signature and the sha256, switches the symlink, refreshes
the unit files from the release and restarts the service. The design fixes the three unit names,
the file the path unit watches and the order of steps in the update; the flag spellings above are
illustrative. The host agent's policy file `agent.yaml`,
its operation catalogue and its impersonation rules are described in [06-host.md](06-host.md).

## Installation and first start

`install.sh` performs, in order:

1. Verification of the Ed25519 signature of the release manifest.
2. Postgres from the distribution's packages if no server is present; otherwise nothing is
   installed.
3. The product's Postgres role and database.
4. The product's unix user and group (`keel-dns`), including the uid range the host agent is
   allowed to manage ([06-host.md](06-host.md)).
5. The directories listed above.
6. The systemd units.
7. A self-signed TLS certificate generated with rcgen.

On first start the kernel generates `secret.key` and `instance.id`, runs the kernel migrations and
then the module migrations in `requires` order under a session lock, and only then opens the port
([04-storage.md](04-storage.md), [05-modules.md](05-modules.md)). It prints a one-time link with a
`kok_` login key to `keel.setup.admin`, a guest-level function that exists only while the
installation has no administrator. The binary starts without `config.yaml` against the default
database; that is an M0 acceptance criterion.

`uninstall.sh --purge` removes the installation together with its data.

The M4 acceptance test for this section is: on a clean Debian or Ubuntu, `curl | sh` installs
`dns`, and no process except the privileged unit runs as root ([../plan.md](../plan.md)).

## TLS

The panel listens on TLS from its very first start. The session cookie carries the `__Host-`
prefix ([03-access.md](03-access.md)), and a browser does not store such a cookie for a plain
HTTP origin — a first visit over plain HTTP by IP address would leave the operator unable to log
in. The plain port therefore serves nothing but a redirect. The operator can supply a certificate
and key or request one through ACME with a button in the settings; `trusted_proxies` makes the
panel usable behind a reverse proxy. A certificate that is about to expire raises a problem
([05-modules.md](05-modules.md)).

## Configuration

`config.yaml` holds only what the process cannot start without; the file may be absent. Its keys
are `listen`, `socket`, `data_dir`, `database`, `tls`, `trusted_proxies` and `log`. A module
receives its own section through a `ConfigSection` contribution. Everything that is edited from the
UI lives in the `settings` module, not in the file.

```yaml
listen: 0.0.0.0:8443
socket: /run/keel/dns/core.sock
data_dir: /var/lib/keel/dns
database: postgres:///keel_dns?host=/run/postgresql
tls: { cert: "", key: "", acme: "" }
trusted_proxies: []
log: { level: info, format: json }
```

Every key is optional. `database` may instead be a TCP URL such as
`postgres://keel_dns:${DNS_DB_PASS}@db.example.net/keel_dns`; an empty `tls` means
the self-signed certificate under `<data_dir>/tls`. The values above for `log` are also its
defaults. Sections of modules (`host`, `plugins`) are added by their `ConfigSection`
contributions; their keys are not fixed by this revision ([06-host.md](06-host.md)). The kernel
keeps every top-level key it does not know as a raw section for those contributions, while a
misspelt key inside `tls` or `log`, an unusable `listen` or `database` value, or a file that is
not YAML fails the start with a message naming the file and the key. An empty file is the same as
no file. The default port of each product is part of the open naming question
([11-decisions.md](11-decisions.md)); the value above is only an example.

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
4. `keel-dns-update.path` fires. The root code path downloads `versions/<ver>`, verifies the
   Ed25519 signature and the sha256 from the manifest, switches the `bin/current` symlink,
   refreshes the unit files from the release and restarts the service. The root side knows one
   operation only — install signed version X — and takes no commands from the request beyond
   the version to install.
5. The new binary starts, runs its migrations before opening the port, marks tasks that were
   running as `interrupted` and restarts only those declared resumable.

Postgres is not touched by an update. Migrations within a major are additive (add table, column
or index; no rename or drop), so `update rollback` within a major switches the symlink back
without down migrations. Crossing a contract boundary backwards is possible only with
`--restore`, that is by restoring the pre-update backup.

The signing key is kept offline; `previous_keys` compiled into the binary allow key rotation.
Compromise of the signing key would compromise every installation, which is why the root side is
limited to a single operation and does not read commands from the request. The M4 acceptance test
is an update from 0.1 to 0.2 by a button in the UI and a rollback without losing the database.

## Backup and restore

A backup is a `tar.zst` archive with a manifest recording the product, `app_version` and the
schema versions of the modules. It contains:

- the database: a logical `COPY` dump per module table, or a `pg_dump` if one is present on the
  host;
- `config.yaml` and the rest of `/etc/keel/dns/`, plugins, brand assets and TLS material;
- contributions of modules through `BackupSource` (for example zone files contributed by
  `dns-zones`).

Targets are `local` and S3, reached through `keel-storage`. API tokens and sessions are stored as
SHA-256 hashes, so an archive contains no usable keys. `secret.key` is included only with
`--with-secret-key`, protected by a passphrase; values of type `Secret` in the dump are readable on
another host only with that key. The M4 acceptance test is a backup taken on one VM and restored on
another with two-factor authentication still working.

`restore` runs with the service stopped and consults a compatibility table by major version.
Restore from the UI is a task that ends with a restart.

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
(version, socket, role rights), migrations, the host agent, the license and free space.

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
- Whether delivery is `install.sh` plus `tar.zst` only, or a container image is offered as well;
  inside a container the path-unit bridge for updates does not work and a different updater is
  needed.
- The default listen ports of the products.
- The license server: the first releases run on long offline leases issued by hand; when the
  server is written as a Keel product and where the signing key is stored remain open.
- Where the prebuilt shell application is published: an npm tarball in Nexus or GitHub release
  assets, which do not pass an egress-restricted production network without extra configuration.
