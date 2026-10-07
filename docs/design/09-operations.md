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
  ([08-frontend.md](08-frontend.md)). `pnpm build` in `frontend/` writes `frontend/dist/shell`
  and must precede `cargo build -p keel-bin`, since `embed_frontend!` reads the directory at
  compile time; CI and the Build workflow run the two in that order. The build pre-compresses:
  `tools/compress.mjs` writes a brotli twin `<name>.br` beside every asset it makes smaller, and
  the kernel answers the twin with `Content-Encoding: br` and `Vary: Accept-Encoding` when the
  client admits `br`, never by its own name. A hashed name — a stem ending in a dash and at least
  six alphanumerics with a digit or an uppercase letter among them — is answered
  `Cache-Control: public, max-age=31536000, immutable`; `index.html` and every other unhashed
  file `no-store`. A `GET` or `HEAD` outside `/api/`, `/healthz`, `/readyz` and `/brand/` that
  names an embedded file answers it with the content type of its extension; a path whose last
  segment carries no extension the shell build emits — `js`, `mjs`, `css`, `map`, `woff2`,
  `woff`, `svg`, `png`, `ico`, `json`, `txt`, `br` — answers `index.html`, so `/f/note.list`
  reaches the shell while `/missing.png` is `404` and `/api/v1/nothing` keeps its envelope. Every
  HTML answer carries `Content-Security-Policy: default-src 'self'; script-src 'self'; style-src
  'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self';
  frame-ancestors 'none'; base-uri 'self'; form-action 'self'`, `X-Content-Type-Options: nosniff`
  and `Referrer-Policy: same-origin`, and the shell carries no inline script: the phoenix theme
  boot script is written to `dist/shell/theme-boot.js` from the library's constant and loaded from
  the head. `nosniff` is set on every asset answer.
- No C libraries. CI checks the binary with `ldd`: systemd is driven by executing `systemctl`
  rather than through D-Bus, SFTP is done with russh, and there is no libssh2, openssl or
  librdkafka. Any C dependency breaks the static musl build, so "no C libraries" is a rule backed
  by the CI check rather than a preference.
- Translation bundles are exported from OwnLate at build time and baked into the binary; the
  customer's server is assumed to be offline with respect to the translation service.
- The M2 acceptance criterion caps the binary with the embedded frontend at 40 MB
  ([../plan.md](../plan.md)); the Build workflow prints the size of the musl release build with
  the `demo` bundle and fails at 40,000,000 bytes. The M2 closing build measures 19.8 MB,
  19,822,864 bytes, as a stripped `x86_64-unknown-linux-musl` release with thin LTO
  ([11-decisions.md](11-decisions.md)). M5 set the cap for the release binary carrying `dns` and
  `panel` at its start: `hack/size-cap` holds 42,000,000 bytes, and `hack/check-size.sh` measures
  the stripped musl binary against it with `wc -c`, in the Build workflow after
  `hack/check-size-test.sh` has fed it a file of exactly the cap, one byte more, a cap given as an
  argument and a cap file without a number, and in the release workflow in place of its 40 MB
  limit. The last size measured before the cap, at the start of M5, was 33,777,536 bytes.

The binary exposes the following subcommands:

| Subcommand       | Runs as      | Purpose                                                              |
|------------------|--------------|----------------------------------------------------------------------|
| `serve`          | `keel`       | the kernel process: HTTP/TLS listener, control socket, modules       |
| `ctl`            | local caller | calls functions over the control socket; the caller is identified by peer credentials |
| `privileged`     | root         | the single privileged code path: policies `update` and `host`        |
| `migrate`        | `keel`       | migration operations                                                 |
| `product`        | `keel`       | lists, enables and disables product bundles                          |
| `update`         | `keel`       | checks the channel, applies an update or rolls back, over the control socket |
| `backup`         | `keel`       | produces the backup archive                                          |
| `restore`        | `keel`       | restores an archive with the service stopped                         |
| `doctor`         | `keel`       | checks the installation                                              |
| `support-bundle` | `keel`       | collects a redacted diagnostic archive                               |

`product` is a client of `keel.product.*` over the control socket while the service runs (the
call checks the lease and requests a restart), and a direct write of `keel_kv.products` under the
`keelinst` lock while it is stopped; the enabled set is read once at boot. The direct path came
first, in M0; the socket path and the restart came with the operator plane of M4, and the lease
check with the `license` module of M5 ([05-modules.md](05-modules.md)): `keel.product.enable`
refuses a product an installed lease does not list, while the direct write has no gate and a
product enabled that way is degraded as unlisted at the next start. `keel license install
<file>` sends a lease over the socket to `license.install`, which stores it in `keel_kv`; no
lease file lives under `/etc/keel`. Leases are issued by hand with `keel-dev license issue --key
<file> --instance <id> --product <id>:<edition> … --days <n> --grace <n> --out <file>`, whose
`expires_at` is the moment of issuing truncated to the second plus `--days` calendar days, from a
pair `keel-dev license keygen --out <dir>` writes as `license.key` (mode `0600`, never
overwritten) and `license.pub`; the production pair is the operator's and its private half stays
out of the repository. The restart is a
distinguished exit status: the switch queues the internal task `keel.restart`, which drains the
task queue, takes the pre-change dump into `backups/pre-restart-<version>-<instant>/` and then
makes the kernel exit with status `75`; `keel.service` carries `RestartForceExitStatus=75` and
`SuccessExitStatus=75`, so the service manager starts it again and the journal does not count
the exit as a failure. A restart requested before the running process started is answered as
already applied, and a second request while one is pending is a conflict; without a task runner
in the composition the function still answers `restart: true` and the change takes effect at the
next start. `product list` prints every bundle the binary carries as
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
| `/etc/keel/templates/`      | operator overrides of `ConfigTemplate` contributions              |
| `/var/lib/keel/secret.key`  | AES-GCM key for `Secret` values, mode 0600, generated at first start |
| `/var/lib/keel/instance.id` | instance identity                                                 |
| `/var/lib/keel/tls/`        | self-signed or operator-supplied certificate and key              |
| `/var/lib/keel/backups/`    | backup archives and the pre-update dump                           |
| `/var/lib/keel/plugins/`    | installed plugins                                                 |
| `/var/lib/keel/tasks/`      | task logs and output files, `<id>.log` and `<id>.out` per task    |
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
| `keel-update.service`     | root   | the oneshot the path unit starts (`privileged update`) |

The kernel unit is hardened: `NoNewPrivileges`, `ProtectSystem=strict`, a capability bounding set
of `CAP_NET_BIND_SERVICE` only, `MemoryMax=50%` and `LimitNOFILE`. Unit files ship with the
release under `deploy/systemd/` and are refreshed by the update; the installer enables
`keel.service`, `keel-privileged.service` and `keel-update.path`, and the oneshot carries no
install section, since it must never run at boot.

`keel.service`:

```
[Service]
Type=notify
User=keel
Group=keel
NotifyAccess=main
ExecStart=/opt/keel/bin/current serve
Restart=always
WatchdogSec=60
TimeoutStartSec=300
RestartForceExitStatus=75
SuccessExitStatus=75
StateDirectory=keel
RuntimeDirectory=keel
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/keel /run/keel
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
AmbientCapabilities=CAP_NET_BIND_SERVICE
MemoryMax=50%
```

`keel serve` writes the notify socket named by `NOTIFY_SOCKET` itself, no library between:
`READY=1` once the migrations ran and the port answers, `WATCHDOG=1` every half of
`WATCHDOG_USEC`, `STOPPING=1` when it drains. `TimeoutStartSec=300` leaves the migrations of a
large installation time before the first `READY=1`.

`keel-privileged.service`, always installed; it serves the host catalogue only while an enabled
product names `host` and the `update` policy otherwise:

```
[Service]
ExecStart=/opt/keel/bin/current privileged --socket /run/keel/agent.sock --policy /etc/keel/agent.yaml
User=root
Restart=always
```

`keel-update.path`, present in every installation, starts the root oneshot `keel-update.service`
when `update.request` appears:

```
[Path]
PathExists=/var/lib/keel/update.request
Unit=keel-update.service
```

```
[Service]
Type=oneshot
User=root
ExecStart=/opt/keel/bin/current privileged update --from-request
TimeoutStartSec=900
```

The request carries the version, the channel and the requesting principal, for example
`{"version":"1.3.0","channel":"stable","requested_by":"admin"}`; the root side downloads
`versions/1.3.0`, verifies the Ed25519 signature and the sha256, switches the symlink, refreshes
the unit files from the release and restarts `keel-privileged.service` together with
`keel.service`, so the privileged process is always the build of the kernel. The design fixes the
four unit files, the file the path unit watches and the order of steps in the update. The flag
spellings are those of `keel privileged --help`: `--socket` and `--policy` with the defaults
above, `--user keel`, the one account the agent socket admits, and `--templates
/etc/keel/templates`, the operator's template overrides; `update --from-request` with
`--request`, `--channel-url` (an `https://` or `http://` base, a `file://` URL or an absolute
directory), `--opt`, `--unit-dir`, `--systemctl` and `--release-key`, which exist so that a test
or a rehearsal drives the whole sequence in another tree with a stub `systemctl` and a channel
directory; and `update rollback [--restore <archive>]`. Everything the root side does is fixed by
its flags, never by the request. The host agent's policy file `agent.yaml`,
its operation catalogue and its impersonation rules are described in [06-host.md](06-host.md).

## Installation and first start

`install.sh --product <id>[,<id>]` refuses a host that already has an installation and refuses a
container (see above), then performs, in order:

1. Verification of the Ed25519 signature of the release manifest.
2. Postgres from the distribution's packages if no server is present; otherwise nothing is
   installed.
3. The Postgres role `keel` and the database `keel`.
4. The unix user and group `keel`, including the uid range the host agent is allowed to manage
   ([06-host.md](06-host.md)): the first block of 65536 uids from 100000 upwards that no account
   and no subordinate range of `/etc/subuid` or `/etc/subgid` touches, written into
   `/etc/keel/agent.yaml` as `policy.uid_range: { from, to }` together with the section of every
   first-party product from the `deploy/agent.yaml` the release carries.
5. The directories listed above.
6. The systemd units.
7. Nothing for TLS: the binary generates the self-signed pair itself at first start, and the
   installer reads its fingerprint from the journal and prints it.
8. `keel product enable <id>` for each requested product, run as `keel` against the fresh
   database before the first start; the kernel migrations run there.

On first start the kernel generates `secret.key`, `instance.id` and the instance's Ed25519 key,
sealed in `keel_kv` ([07-ecosystem.md](07-ecosystem.md)), reads the enabled products,
runs the kernel migrations and then the migrations of the loaded modules in `requires` order
under a session lock, and only then opens the port ([04-storage.md](04-storage.md),
[05-modules.md](05-modules.md)). It prints a one-time link with a `kok_` login key to
`keel.setup.admin`, a guest-level function that exists only while the installation has no
administrator. The binary starts without `config.yaml` against the default database; that is an
M0 acceptance criterion.

A product that drives daemons of its own is prepared by the installer between the policy and the
units: for `panel` the step `prepare_products` answers the mailer question of postfix, installs
`nginx`, `php-fpm`, `mariadb-server`, `mariadb-client`, `postfix`, `dovecot-core`,
`dovecot-imapd`, `dovecot-lmtpd`, `vsftpd` and `libpam-pwdfile` in one `apt-get install`,
creates the roots of the policy's `panel` section (`/var/www` and `/var/lib/keel-panel-acme`
root's with `0755`, `/var/lib/keel-panel-dumps` and `/var/lib/keel-panel-backups` root's with
`0711`, `/etc/ssl/keel` root's with `0700`, `/var/lib/keel-panel-transfer` keel's with `0700`, the
postfix maps, the dovecot passwd-file, the maildirs of the system user `vmail` and the vsftpd
user file and per-login directory), writes the `main.cf` keys, the dovecot drop-in in the 2.4
syntax of Debian 13, the PAM service `keel-panel-ftp` and a `vsftpd.conf` for guest users (the
previous one kept as `.orig`), checks each with the check the policy names, and enables and
restarts `nginx`, `mariadb`, `postfix`, `dovecot`, `vsftpd` and every `php*-fpm` unit, stopping
when one of them does not run. The preparation is idempotent and lives in the installer rather
than in a hook of the release, since only `panel` needs one; `dns` installs no package.

Installing a second product later is `keel product enable <id>` followed by the restart it
requests; the restart runs the same drain and pre-change logical dump as an update, and the new
product's modules migrate before the port opens. Enabling `panel` this way prepares none of its
daemons; the operator runs the installer's preparation or installs them by hand
([deploy/README.md](../../deploy/README.md)). `uninstall.sh` leaves the packages of a product
and their configuration in place.

`uninstall.sh --purge` removes the installation together with its data.

The installer is `deploy/install.sh`, complete since M4, and `deploy/README.md` is its manual.
It is a bash script: `hack/release.py` embeds the public release key into it and publishes it at
the channel root, so `curl -fsSL https://dl.globalart.hu/keel/install.sh | bash -s -- --product
dns` reads the same file every release archive carries under `deploy/`, and a plain `sh` stops it
at once with one line naming bash. The signature is verified with `openssl` over the payload
bytes of the manifest, the same check `keel-release verify` makes, before anything is unpacked;
the archive is checked against the sha256 of the verified payload, and the signed manifest is kept
beside the installed version. Its flags: `--product <id>[,<id>]`, `--channel stable|beta`,
`--channel-url`, `--version` for a version of the channel instead of its head, `--release-key`
for a rehearsal channel, `--binary <path|url>`, an unsigned override for a build that is not on
the channel, `--listen` and `--yes`. The installer creates no administrator; it prints the setup
link the first start logs, and the page behind it calls `keel.setup.admin`.

The M4 acceptance test for this section is: on a clean Debian or Ubuntu,
`curl | bash -s -- --product dns` installs Keel with `dns` enabled, and no process except the
privileged unit runs as root; at M5 `keel product enable panel` on the same host yields one login
that sees both products ([../plan.md](../plan.md)).

## TLS

Keel listens on TLS from its very first start. The session cookie carries the `__Host-` prefix
([03-access.md](03-access.md)), and a browser does not store such a cookie for a plain HTTP origin —
a first visit over plain HTTP by IP address would leave the operator unable to log in. There is no
plain listener and no redirect port: a plain request to the TLS port gets no HTTP answer, so a
browser reports a protocol error instead of showing a login page whose cookie it would refuse, and a
second port would be one more thing to open and to explain. The pair is the one `tls.cert` and
`tls.key` name in `config.yaml`, or the one the binary generates at first start under
`<data_dir>/tls` (`cert.pem`, `key.pem` with mode `0600` in a `0700` directory) with rcgen: an ECDSA
P-256 key and a certificate valid for ten years whose subject alternative names are the host name,
`localhost`, `127.0.0.1`, `::1`, the addresses the host name resolves to and the listen address when
it is neither unspecified nor loopback; a browser reaching the host by another address gets a name
warning. Each file is written to a temporary name and renamed into place, so a reader polling
`/readyz` never sees a partial file. A pair under the data directory that does not parse is
replaced; an operator's pair that does not parse, or one file of it without the other, fails the
start naming the file. The certificate's SHA-256 fingerprint is logged at every start, so the
operator accepting the certificate in a browser knows it is the right one. The operator can supply a
certificate and key; requesting one through ACME with a button in the settings, and the problem a
certificate about to expire raises ([05-modules.md](05-modules.md)), are left to a later
milestone the plan names when it schedules them. Until then a non-empty `tls.acme` fails the
start with `keel.error.acme_unsupported`. The ACME client of M5, `instant-acme` in `hosting-ssl`,
issues the certificates of the sites `panel` serves over HTTP-01, from the directory the setting
`hosting.certificate.directory` names (Let's Encrypt production by default, staging or a private
directory by URL, with `directory_ca` of the config section `hosting_certificate` as the trust
anchor of a private one); the installation's own listener does not use it yet.

Between installations the certificate is not the identity: a link pins the neighbour's instance
key, proven by a signed challenge ([07-ecosystem.md](07-ecosystem.md)). The clients of the
installation, the outgoing calls of `peers` and `keel-client` with `Trust::System` in `dns-peer`,
verify a neighbour's certificate by the trust store of the operating system, so two installations
with self-signed certificates each add the other's `<data_dir>/tls/cert.pem` to that store and
reach each other under an address their certificates name, the address of `--listen` for a
generated one; pinning the certificate of a neighbour in its link is left to a later milestone, and
`keel-client` already offers `Trust::Pinned` for it. A link's webhook subscription and the hook
address pairing accepts obey the setting `webhook.private_targets`, so two installations on one
private network turn it on for the side that delivers. `hack/peers-e2e.sh target/debug/keel`, a
step of the job `check` after the browser suite, proves the "Done when" of M6 on two binaries,
`dns` alone and `panel` alone, each on a database, a `data_dir`, a port and a host agent of its own,
paired by code over the CLI; they reach each other through two relays on plain HTTP, one in each
direction, since neither trusts the other's self-signed certificate, the public address of the
panel, which pairing sends, being its relay, and a relay records each request with the prefix of
its bearer credential, never the value. The relay in front of the panel forwards the first
delivery and answers the dns side `500` in place of the panel's answer, so the dns side reads the
delivery failed and `webhook.delivery.retry` sends it again under a later signature, which the
panel answers as a duplicate; the specs tagged `@peers` of `frontend/e2e/` run against the two
installations alone. `hack/peers-vm.sh` is the operator's proof on two machines, its steps
alternating between them: each is installed with `--listen <address>:1500` and trusts the other's
certificate through the system trust store with `trust`, and with no relay between them the repeat
is a delivery marked failed in the database and sent again by `webhook.delivery.retry`, the
one-time URL is opened by hand in a browser, and the dns machine waits for the delivery of
`svc.service.opened` before `peer.delete`, since the subscription of a link goes with it and takes
the deliveries not yet sent along.

`trusted_proxies` makes the installation usable behind a reverse proxy: a list of CIDRs (a bare
address standing for its `/32` or `/128`, a bad entry failing the start naming it). When the
peer address lies inside one of them, the client address is the rightmost `X-Forwarded-For` entry
that is not itself a trusted proxy, and the authority for the brand patch, `/boot` and the
`Origin` comparison of the cross-site check comes from `X-Forwarded-Host` when it is a single
`host[:port]`; the peer address and the `Host` header stand otherwise, an entry of
`X-Forwarded-For` that is not a bare IP address leaves the peer address, and a malformed header
is logged at debug level. The IP rules, the lockout and the audit record follow through the
client address the transport resolved.

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
(`host`, `update`, `plugin`) are added by their `ConfigSection` contributions; their keys are not
fixed by this revision ([06-host.md](06-host.md)), except those of `plugin`: `timeout_ms` (10000,
the answer time of every frame), `failures` (5, the failures that open the breaker), `quiet_ms`
(60000, the quiet period before the trial and the uptime after which a daemon's count of failures
starts again), `liveness_ms` (30000) and `backoff_ms` (1000, the first pause before a restart,
doubling up to `quiet_ms`). The kernel keeps every top-level key it does not know as
a raw section for those contributions and matches them at freeze: a section is validated
against the schema of the type the contribution names and a violation fails the start naming the
file, the section and the path, an absent section becomes the type's default, a section claimed
by two modules is a freeze error, and a top-level key no loaded module claims is logged as a
warning, so that the section of a disabled product stays valid. A misspelt key inside `tls` or
`log`, an unusable `listen` or `database` value, or a file that is not YAML fails the start with
a message naming the file and the key. An empty file is the same as no file. The default listen
port of the installation is open ([11-decisions.md](11-decisions.md)); the value above is only an
example, and M0 keeps the provisional default `127.0.0.1:1500` while the port is undecided. The
body limit of a request is a kernel constant, 1 MiB, not a key of this file; the multipart body
of an `upload` is bounded by its file fields instead ([10-contracts.md](10-contracts.md)). The
runtime binds
the listener before the kernel boots and attaches the dispatcher after it, so `/healthz` answers
`{ "status": "ok" }` from the first moment, `/readyz` reports `migrating` (and later `draining`,
or `database` when `SELECT 1` fails) with `503` until it reports `ready`, and `/api/v1/*`
answers `503 unavailable` in between ([10-contracts.md](10-contracts.md)); the readiness flags
belong to the runtime, not to a module.

## Updates

Releases are published on a channel described by a signed `manifest.json` with `version`,
`sha256`, `min_upgrade_from`, `contract` and `notes`.

### The release channel

The channel lives at `https://dl.globalart.hu/keel/`, a host name compiled into the binary as the
default `update.channel_url`. Behind that name stands a raw repository `keel-releases` of the
Nexus at `nexus.globalart.hu`, reachable through a route of the cluster; the name is the contract
with every installation, the storage behind it is not, so the repository can move to an object
store later without touching a single host. The layout:

```
https://dl.globalart.hu/keel/
  stable/manifest.json
  beta/manifest.json
  versions/<ver>/keel-<ver>-x86_64-linux-musl.tar.zst
  versions/<ver>/keel-<ver>-x86_64-linux-musl.tar.zst.sha256
  versions/<ver>/manifest.json
```

`stable/manifest.json` and `beta/manifest.json` are the heads of the two channels; the copy under
`versions/<ver>/` is the manifest the version was released with, kept so that a rollback and an
audit can read what was promised at the time. Reading is anonymous for this repository only: the
manifest is signed and the archive is verified against the hash in it, and what an installation
may run is decided by its license, not by who can download the archive. Should downloads ever
need to be restricted, the license server of M5 hands out tokens for the same URL.

Publication is the release workflow of M4: a tag `keel-vX.Y.Z` builds the musl binary, signs the
manifest with `hack/release.py` and uploads through the Nexus REST API under a deploy account
`keel-release` whose secret reaches CI through `ci-cd sync-secrets`. The first releases sign in
CI with the key held as a repository secret, which is enough while the only installations are
our own; before the product is sold the signature moves to an offline machine, which changes
where `hack/release.py` runs and nothing else. The workflow is `release.yml`, triggered by
annotated `keel-v*` tags alone: it repeats the musl build, checks that the binary prints the
version of the tag, and runs `hack/release.py`, which packages the binary with
`config-sample.yaml` beside it and `install.sh`, `uninstall.sh`, `agent.yaml` and the unit files
under `deploy/`, writes the `.sha256`, signs the manifest through `keel-release sign`, verifies it
against the compiled keys and uploads the archive, its digest, the version's manifest and the
head in that order, then `install.sh` to the channel root. A
version with a pre-release suffix lands on `beta`, any other on `stable`; the release notes are
the tag's annotation; a version the channel already carries is refused without `--replace`, and a
head never moves backwards. `contract` and `min_upgrade_from` of the next release live in
`crates/release/channel.json`, edited beside the workspace version. The `Build` workflow keeps
uploading the musl binary of every push as a short-lived GitHub artifact, which `install.sh
--binary` takes for a build that is not on the channel.

An update runs in two halves separated by the privilege boundary:

1. The `update` module compares the running version, the one `/boot` answers, with the head of
   the channel the setting `update.channel` names (`stable` or `beta`) at the address of
   `update.channel_url` in `config.yaml`. The address is startup configuration, not a setting,
   so no administrator points an installation at another channel from the browser. Anti-rollback
   is enforced by the minimum version stated in the channel manifest. The kernel side reads the
   payload of the signed envelope without verifying it: the root side is the one verifier. A
   daily cron job raises the problem `update.available`, and the running version clears it.
2. The kernel drains the task queue ([05-modules.md](05-modules.md)) and takes a logical dump of
   the database: `COPY … TO STDOUT` over its own connection into
   `backups/pre-update-<version>-<instant>/`, one file per base table of the schema, loaded or
   orphaned, so the unprivileged process does not need to invoke `pg_dump`. This dump is taken
   before every update. Draining stops the task worker from starting a queued job and lets running
   jobs see the cancellation at their next checkpoint; a refused update lifts the drain at once,
   and a written request lifts it after fifteen minutes should the service still run.
3. The kernel writes `update.request` with the requested version, channel and the requesting
   principal; an unconsumed request is replaced. The whole of steps 1 to 3 is the task
   `update.apply`, run from the button of the screen "Updates" or by `keel update apply`.
4. `keel-update.path` fires. The root code path downloads `versions/<ver>`, verifies the
   Ed25519 signature and the sha256 from the manifest, switches the `bin/current` symlink,
   refreshes the unit files from the release and restarts the service. The root side knows one
   operation only — install signed version X — and takes no commands from the request beyond
   the version to install. The request file is opened once, without following a link and without
   blocking, refused unless it is a regular file, and read up to a limit. The previous version is
   the symlink `bin/previous` beside `bin/current`, written at every switch, and the signed
   manifest of every installed version is kept as `versions/<ver>/manifest.json`; a version
   without one has an unknown contract. `min_upgrade_from` is applied twice: the requested
   version must not be below the head's value, and the running version not below the target
   manifest's.
5. The new binary starts, runs its migrations before opening the port, marks tasks that were
   running as `interrupted` and restarts only those declared resumable.

There is one release train: the channel manifest carries one `version`, and an update moves every
enabled product at once; rollback moves all of them back; the pre-update dump covers every loaded
module's tables.

Postgres is not touched by an update. Migrations within a major are additive (add table, column
or index; no rename or drop), so `update rollback` within a major switches the symlink back
without down migrations. Crossing a contract boundary backwards is possible only with
`--restore`, that is by restoring the pre-update backup. `update.rollback` and `keel update
rollback` write an install request for the version the installation was updated from, read from
the module's own history, and the root side installs a lower version only when it is the one
`bin/previous` points at and lies within its contract; anything else is `keel privileged update
rollback`, run by the operator. With `--restore` the root side stops `keel.service` instead of
restarting it and prints the `keel restore` the operator runs, since the pre-update dump is a
directory and `keel restore` reads a `keel backup` archive.

The signing key is kept offline; `previous_keys` compiled into the binary allow key rotation.
Compromise of the signing key would compromise every installation, which is why the root side is
limited to a single operation and does not read commands from the request. The M4 acceptance test
is an update from 0.1 to 0.2 by a button in the UI and a rollback without losing the database.

## Backup and restore

A backup is a `tar.zst` archive with a manifest recording the keel version, the enabled products
with their editions and the schema versions of the modules. It contains:

- the database: a logical `COPY` dump of every base table of the schema, taken in one `REPEATABLE
  READ` snapshot over the kernel's own connection with the column list in each statement and the
  generated columns left out; `pg_dump` is not consulted, since the archive has one reader;
- `/etc/keel/` in full (regular files), `instance.id` and the TLS material of the data directory;
  brand assets live in a table of the `settings` module and travel with the dump, and plugins
  join with M7;
- contributions of every loaded module through `BackupSource` (for example the zone files
  contributed by `dns-zones` as `dns.zonefiles`), one directory of the archive each, named with
  the module's prefix; a source without a restore hook is archived and skipped by the restore.

`keel backup [--out <path>] [--with-secret-key]` writes the archive, by default
`<data_dir>/backups/keel-<version>-<instant>.tar.zst`, never over an existing one; `keel backup
inspect <archive>` prints its manifest; the screen "Backups" runs `backup.create` as a task whose
archive is offered for download. This local target is the only one: the S3 target, and the crate
`keel-storage` that is to hold the targets, were moved to M5 with `hosting-backup`, which M5 built
with local archives only, so remote targets wait for a later milestone. The per-site backups of
`panel` are archives of their own in the agent's root `backups`, taken and restored by
`hosting-backup` as tasks of that module ([06-host.md](06-host.md)); the installation backup
carries the module's two tables like every other table and none of the site archives, and its
sources `hosting_database.dumps` and `hosting_mail.maildirs` answer `unavailable` in the CLI
backup, which never starts the modules, so `backup.create` in the running service is the one that
carries them. A site backup is taken by `hosting.backup.create` and by the cron job
`hosting.backup.scheduled` as read-mode tasks, a restore by the write-mode task
`hosting.backup.restore`: a write-mode task commits or rolls back with its outcome, which would
hide the pending row while the host packs, erase the error row of a failed run and leave the
archive of a rolled-back run without its row. A site backup is therefore not recorded in
`audit_call`, while the task row and the agent's journal record it, and it keeps running under
the read-only degradation of an expired lease, so a site owner can still secure a site; a
restore is audited, answers `402` in read-only and takes an idempotency key. API tokens and sessions are stored as
SHA-256 hashes, so an archive contains no usable keys. `secret.key` is included only with
`--with-secret-key`, protected by a passphrase; values of type `Secret` in the dump are readable on
another host only with that key. The M4 acceptance test is a backup taken on one VM and restored on
another with two-factor authentication still working.

`restore` runs with the service stopped and consults a compatibility table by major version: the
same major alone, and never an archive newer than the binary, since an additive migration may
have added a column this binary does not know. The enabled set is restored with the database,
because it lives in `keel_kv`; `restore` refuses a binary that does not carry one of the bundles
the manifest names, and the next start reports what the restored set changed. It runs the
binary's migrations for the products the manifest names, then in one transaction truncates every
table of the schema except the migration ledgers and loads the dump parents first, realigning the
sequences; the ledgers stay the binary's, so the next start re-runs nothing. Without
`--with-secret-key` the wrapped data key of the dump is kept when this host's `secret.key` opens
it and dropped otherwise, so a restore on another host boots with a new data key while the sealed
values of the dump do not open; the instance key, sealed under that data key, is dropped with it, so
a copy restored on another host comes up under the instance id of the dump with a new instance key
and with the dump's links, whose sealed tokens do not open: before it pairs again with a neighbour,
an administrator on each side deletes the stale link with `peer.delete`
([07-ecosystem.md](07-ecosystem.md)). A restore with the key keeps the identity and the links. The
configuration file the restore runs with is never overwritten: the archive's copy lands beside it as
`config.yaml.restored`, and when `/etc/keel` is not writable by `keel` the archive's tree lands
under `<data_dir>/backups/<archive>.etc/`, since `agent.yaml` is root's policy and root copies it.
Restore from the UI is the task `backup.restore`, which checks the archive and writes
`<data_dir>/restore.request`; `keel serve` consumes the request before it opens its port, so the
restore runs at the next restart.

## Observability

- Logs go through `tracing` in JSON or text, selected by `log.format`. Log levels can be changed
  per module at runtime. A ring buffer of 10 000 entries backs the `keel.logs.tail` function and
  the support bundle.
- Every response carries `X-Keel-Request-Id`; the host agent writes every operation to journald
  with the actor and the request id ([06-host.md](06-host.md)).
- Call counters and latency histograms are to be exposed in the text exposition format, with OTLP
  export as a cargo feature; neither is built yet, and the support bundle carries the `/readyz`
  answer in their place.
- `/healthz` reports liveness. `/readyz` reports readiness: the database is reachable, migrations
  are applied, and the process is not draining.
- The `notify` module keeps a problem registry with idempotent raise and clear for disk space,
  TLS, license, migrations, the outbox and cluster nodes; open problems are counted in `/boot`
  and shown on the dashboard ([05-modules.md](05-modules.md)).

## Diagnostics and support

`doctor` checks the installation end to end: directory permissions, units, TLS, the database
(version, socket, role rights), migrations, the host agent, the license and free space; that every
enabled product is carried by the binary and listed by the lease; and the orphaned modules and
the accounts left in `deleting`, if any. It prints one line per check with `ok`, `warn` or
`fail` and exits 1 when a check fails. The checks are the functions `keel.doctor.run` and
`keel.logs.tail` of the kernel's own module `keel`, behind the screen "Health" of the system
group, since the registrar admits a function only under its module's prefix; every
operating-system parameter of a check is a flag of the CLI (`--opt`, `--unit-dir`,
`--systemctl`, `--agent-socket`) with the defaults of this document, and the function never
lets a caller name a binary. The license check still reports that no lease is required, as in M4:
reading the state of the `license` module into `doctor` was left out of M5.

`support-bundle` collects the configuration with secrets redacted, logs, the migration ledger, the
tail of the audit log, open problems and metrics, into
`<data_dir>/support/keel-support-<version>-<instant>.tar.zst` or `--out`, written through a
temporary created exclusively and renamed. The audit tail and the problems are read from the
tables, so the bundle works with the service stopped; the password-like keys of a section no
loaded module claims are redacted too, so a disabled product's section cannot leak. Redaction uses the same `secret` flag of the
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
  server is written as a Keel product remains open.
