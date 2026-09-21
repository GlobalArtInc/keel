# Installing keel

`install.sh` is the one way a keel installation reaches a host. It is the installer of M4
brought forward in the subset the current binary supports, so that every installation, the demo
included, goes through the same path. The design of the whole operator plane is in
[../docs/design/09-operations.md](../docs/design/09-operations.md); this file says what the
script does today.

## Requirements

- A clean Debian 12 or newer, or Ubuntu 24.04 or newer, booted with systemd, not a container.
  A host that already carries `/opt/keel` or `/var/lib/keel` is refused.
- Root.
- A build of the binary for the host's architecture: the file
  `keel-<sha>-x86_64-linux-musl.tar.zst` from the Build workflow of the repository, or a bare
  `keel` binary.
- Network access to the distribution's package mirror when Postgres is not installed yet, and
  when `curl` is missing: the installer installs `curl` and `ca-certificates` right after the
  confirmation, since it waits for readiness over https.

## Getting the files to the host

The repository is private, so the raw `install.sh` is not fetched with `curl`. Copy the
`deploy/` directory and the artifact:

```
scp -r deploy/ root@host:/root/keel-deploy
scp keel-<sha>-x86_64-linux-musl.tar.zst keel-<sha>-x86_64-linux-musl.tar.zst.sha256 root@host:/root/
```

The artifact is on the run page of the Build workflow (Actions, Build, the run for the commit,
artifact `keel-x86_64-linux-musl`), or with the GitHub CLI:

```
gh run download --repo GlobalArtInc/keel --name keel-x86_64-linux-musl
```

The `.sha256` file next to the archive is verified by the installer when present; when it is
missing the installer says so and goes on without the check. A local build works too:
`cargo build --release -p keel-bin` and `--binary target/release/keel`, provided the build's
target matches the host.

The archive is unpacked as a whole into `/opt/keel/versions/<version>/`, the design's one
unpacked release per version; the unit files are taken from its `deploy/systemd/` and
`config-sample.yaml` from its root when it carries them. Until the release archive of the
release workflow does, they come from `deploy/` next to the script, which is why the whole
directory is copied, and the installer places the units under
`/opt/keel/versions/<version>/deploy/systemd/` so that a later rollback finds them where a
release keeps them; a bare binary always takes them from `deploy/`.

## Installing

```
cd /root/keel-deploy
./install.sh --product demo --binary /root/keel-<sha>-x86_64-linux-musl.tar.zst --yes
```

Flags:

| Flag | Meaning | Default |
|---|---|---|
| `--product <id>[,<id>]` | bundles to enable; the binary must carry each of them | required |
| `--binary <path\|url>` | the binary or the tar.zst artifact; an https URL is downloaded to a temporary directory, and a `.sha256` file next to it is verified. The flag stands in for the release channel: from M4 a missing `--binary` means the channel's current version, and the flag stays as an override | required until M4 |
| `--listen <addr:port>` | listen address written to `config.yaml` when it differs from the kernel's default; `<ipv4>:<port>` or `[<ipv6>]:<port>` | `127.0.0.1:1500` |
| `--admin <login>` | login of the first administrator: 1 to 64 characters of letters, digits, `.`, `_`, `@`, `-`; a value `keel ctl` would read as JSON (a number, `true`, `false`, `null`) is refused | `admin` |
| `--yes` | do not ask for confirmation | asks |

The installer prints the login, the generated password and the URL once at the end and writes
none of them anywhere. Every step announces itself with `==>`; a failure names the step and exits
non-zero.

## What the installer does, in order

1. Refuses a container (`systemd-detect-virt --container`, `/.dockerenv`, `/proc/1/cgroup`),
   a host with `/opt/keel` or `/var/lib/keel`, a host without `apt-get`, and a host whose
   running init is not systemd.
2. Fetches the binary: downloads an https URL (https for redirects too), verifies the `.sha256`
   when present and reports when it is missing, unpacks the whole tar.zst under
   `/opt/keel/staging` and requires `keel` at its root, runs `keel --version` to make sure the
   build runs on this host.
3. Installs `postgresql` from the distribution when no server answers on
   `/run/postgresql`, enables and starts it, and waits for its socket; an existing server is
   used as it is and the version must be 15 or newer. A pre-existing cluster that is down is
   reported, not started.
4. Creates the Postgres role `keel` (login, no password) and the database `keel` owned by it
   when they are absent. Nothing else on the server is touched.
5. Creates the system user and group `keel`, home `/var/lib/keel`, shell `nologin`, and checks
   that peer authentication over the unix socket admits it.
6. Moves the unpacked release to `/opt/keel/versions/<version>/`, places `config-sample.yaml`
   there from `deploy/` when the release does not carry it, and lays out the symlink
   `/opt/keel/bin/current`, the convenience symlink `/usr/local/bin/keel` (an absent or dangling
   name is taken, an existing symlink is repointed, anything else is left alone and reported),
   `/etc/keel` (`0750 root:keel`) with `config.yaml` (`0640`) and `templates/`, and
   `/var/lib/keel` (`0700 keel:keel`) with `tls/`, `backups/`, `plugins/` and `tasks/`, so that
   the layout of the design is complete from the first install and later milestones fill the
   directories rather than create them. `/run/keel` is created by the unit's
   `RuntimeDirectory=` at every start.
7. Writes `/etc/keel/config.yaml` with the keys that differ from the kernel's defaults only:
   `listen` when `--listen` was given, `database` when the server's socket is not on port
   5432, both as quoted scalars so that a bracketed IPv6 address stays an address. With the
   defaults the file is empty, which the kernel treats as absent.
8. Installs the four units from `/opt/keel/versions/<version>/deploy/systemd/`, placed there
   from `deploy/systemd/` when the release does not carry them: `keel.service` (the kernel, user
   `keel`), `keel-privileged.service` (the one root code path, `keel privileged --socket
   /run/keel/agent.sock --policy /etc/keel/agent.yaml`), `keel-update.path` (watches
   `/var/lib/keel/update.request`) and `keel-update.service` (the root oneshot `keel privileged
   update --from-request` the path unit starts). The first three are enabled; the oneshot has
   no install section and runs only when the path unit fires.
9. As user `keel`: `keel migrate`, `keel product enable <id>` for each product, `keel migrate`
   again so that the products' migrations are applied before the first start.
10. Starts `keel-privileged.service` and `keel-update.path`, then `keel.service`, and waits up
    to 90 s for `GET /readyz` over https to answer `200`, with
    `--cacert /var/lib/keel/tls/cert.pem`, the certificate the binary generates at its first
    start; a missing certificate counts as not ready (`KEEL_INSTALL_READY_TIMEOUT` changes the
    limit). A unit that stops or a timeout prints the tail of the journal.
11. Creates the first administrator over the control socket:
    `keel ctl keel.principal.create login=<admin> level=admin account=<root account>`, then
    `keel ctl auth.password.set principal=<id> password=<generated>` with a 24-character random
    password.
12. Prints what was installed, the https address, the certificate path and its SHA-256
    fingerprint read from the journal (through `openssl` when the journal has none), the
    credentials and the commands to check the service. When `listen` is `0.0.0.0` or `::` the
    printed address is the first interface address, which the certificate does not name, so the
    report lists the names the certificate carries — the host name, `localhost` and the loopback
    addresses — warns that a browser reaching the host by another address gets a name warning
    too, and pins the API example to the host name with `--resolve`.

On a failure after the host was changed the units are stopped, disabled and removed and
`/opt/keel` is deleted; `/etc/keel`, `/var/lib/keel`, the unix user and the database are left in
place and `uninstall.sh --purge` clears them before another attempt. Steps 3 to 5 are
idempotent, so a partially prepared host can be purged and installed again.

The generated password is passed to `auth.password.set` on the command line, since `ctl` takes
its parameters as `key=value` pairs; it is visible in the process list for the duration of that
call and is masked in the audit trail.

## What it does not do yet

These parts of the operator plane are designed but not built; the installer does not pretend
they exist:

- TLS beyond the generated pair. The listener speaks TLS alone with the self-signed certificate
  the binary writes under `/var/lib/keel/tls/` at first start, or with the pair `tls.cert` and
  `tls.key` of `config.yaml` name; ACME, the expiry problem and the certificate button of the
  settings arrive with M4. The installer generates nothing for TLS itself.
- The host policy of the privileged unit. `keel-privileged.service` is installed and runs as
  root, but until the `host` module and its agent arrive it serves the `update` policy alone
  and binds no socket; `agent.yaml`, the operation catalogue and the audit of operations come
  with the host layer of M4. The M2 spike `bin/spike` proved the mechanism on `keel-demo` with
  its own unit and policy and was removed again by `hack/spike-vm.sh --cleanup`. After the
  installer exits the privileged unit is the only process of keel running as root.
- The uid range the host agent may manage. The design allots it to the installation at install
  time next to the unix user; it arrives with `agent.yaml`, since that file is where the agent
  reads it, and the installer creates the user and the group only until then.
- The release channel. `--binary` is required because the installer does not fetch a release
  from the signed channel yet; a missing `--binary` will mean the channel's current version and
  the flag remains as an override.
- The update button. The root half of an update is in place: `keel-update.path` watches
  `/var/lib/keel/update.request`, and `keel-update.service` runs `keel privileged update
  --from-request`, which reads the version, the channel and the requesting principal from the
  request and nothing else, downloads `versions/<ver>/manifest.json` and the archive from
  `https://dl.globalart.hu/keel/`, verifies the Ed25519 signature of the manifest against the
  release key compiled into the binary, the archive's sha256 against the manifest and the
  version against `min_upgrade_from` of the channel head, unpacks into
  `/opt/keel/versions/<ver>/`, switches `bin/current` (keeping `bin/previous`), refreshes the
  unit files from the release's `deploy/systemd/` and restarts `keel-privileged.service` and
  `keel.service`; `keel privileged update rollback` switches back within the same `contract`
  and refuses across one unless `--restore <archive>` names the pre-update backup, in which
  case `keel.service` is stopped for the restore instead of restarted. The kernel half is the
  `update` module: the screen "Updates" under the system group and `keel update check` read the
  head of the channel named by the setting `update.channel` (`stable` or `beta`) at the address
  of the `update.channel_url` key of `config.yaml` (default `https://dl.globalart.hu/keel/`),
  and `update.apply` from its button or `keel update apply [--wait]` runs as a task that drains
  the task queue, takes the pre-update dump with `COPY … TO STDOUT` into
  `/var/lib/keel/backups/pre-update-<version>-<instant>/`, one file per table, and writes
  `update.request`; the task ends `done` and the root unit takes over. `update.rollback` and
  `keel update rollback` write the same request for the version the installation was updated
  from, within the same `contract`. The published channel arrives with the release workflow;
  until then a version is installed by hand as before: unpack the artifact into
  `/opt/keel/versions/<version>/`, point `/opt/keel/bin/current` at its `keel`,
  `systemctl restart keel` and check that `/readyz` answers `ready`. `hack/update-vm.sh`
  rehearses the root half on a machine with a rehearsal key and a channel directory (`run`), and
  the kernel half through `keel update apply --wait` against that channel served over HTTP
  (`kernel`).
- The setup link. `keel.setup.admin` and the one-time `kok_` link printed at the first start
  arrive with M4; the installer creates the administrator over the control socket instead.
- The signature check of the release manifest; the installer verifies the sha256 of the artifact
  only.
- `doctor` and `support-bundle` (M4). `backup` and `restore` are in place (see below);
  `rollback --restore` still stops the service and leaves the restore of the named archive to
  the operator, who runs `keel restore` as the user `keel` and starts the service again.

`keel.service` is `Type=simple` without `WatchdogSec=` because the kernel does not send
`sd_notify` readiness yet; the installer waits for `/readyz` instead. The unit switches to
`Type=notify` with the watchdog when the kernel speaks it. `MemoryMax=50%` is the hardening
value the design leaves to the unit: half of the host's physical memory, so that a leaking
kernel is stopped before it starves Postgres and the operating system, and the same unit fits
every host size. An operator who needs another limit sets it in a drop-in,
`systemctl edit keel`, which an update does not overwrite.

## Configuration keys

`config-sample.yaml` lists every key the kernel reads with its default value. It is installed
next to the binary as `/opt/keel/versions/<version>/config-sample.yaml`, since it documents that
version's kernel and an update refreshes it with the release; until the release archive carries
it, the installer takes it from `deploy/`. `/etc/keel` holds nothing but the files the design
lists there. The kernel starts without the file, and an empty file is the same as no file.

| Key | Default | Meaning |
|---|---|---|
| `listen` | `127.0.0.1:1500` | address and port of the TLS listener; the port is provisional until the design fixes it |
| `socket` | `/run/keel/core.sock` | control socket `keel ctl` talks to; admitted for the unix user `keel` alone |
| `data_dir` | `/var/lib/keel` | `secret.key`, `instance.id` and everything the kernel stores outside the database |
| `database` | `postgres:///keel?host=/run/postgresql` | connection URL; peer authentication on the unix socket, or a TCP URL with `${KEEL_DB_PASS}` substituted from the environment |
| `tls` | `{ cert: "", key: "", acme: "" }` | `cert` and `key` name an operator's PEM pair, both or neither; empty means the pair the binary generates under `<data_dir>/tls` at first start; `acme` is refused until M4 |
| `trusted_proxies` | `[]` | CIDRs of reverse proxies whose `X-Forwarded-For` and `X-Forwarded-Host` the kernel believes; a bare address stands for its `/32` or `/128` |
| `log` | `{ level: info, format: json }` | `tracing` level and output format; `json` lines go to the journal under systemd, `text` is for a terminal |

Unknown top-level keys are sections for modules (`ConfigSection`); a section no loaded module
claims is logged and ignored. A misspelt key inside `tls` or `log`, an unusable `listen` or
`database` value, or a file that is not YAML fails the start naming the file and the key.

## After the installation

```
systemctl status keel keel-privileged keel-update.path
journalctl -u keel -f
runuser -u keel -- keel ctl keel.account.list --out table
runuser -u keel -- keel product list
curl -sS --cacert /var/lib/keel/tls/cert.pem https://127.0.0.1:1500/readyz
journalctl -u keel --no-pager -o cat | grep -oE '([0-9A-F]{2}:){31}[0-9A-F]{2}' | tail -n 1
```

`keel ctl` must run as the unix user `keel`: the control socket admits that uid alone, root
included in the refusal.

The URL the installer prints serves the panel, the embedded shell, next to the API, over TLS
with the generated certificate. The browser warns once about the self-signed certificate; the
fingerprint the installer printed, the same the journal logs at every start, tells the operator
it is the right one, and the certificate names the host name, `localhost` and the loopback
addresses, so a host reached by another address gets a name warning as well. A `listen` of
`127.0.0.1:1500` is reached from another machine through an SSH port forward,
`ssh -L 1500:127.0.0.1:1500 root@host` and then `https://localhost:1500/`; a `listen` of
`0.0.0.0:1500` is opened directly at `https://<host name>:1500/`. Behind a reverse proxy,
`trusted_proxies` in `config.yaml` lists the proxy addresses, and the proxy speaks TLS to the
listener with the certificate or its own trust. The login the installer printed signs the
administrator in; the same login over the API:

```
curl -sS --cacert /var/lib/keel/tls/cert.pem -X POST https://127.0.0.1:1500/api/v1/call/auth.login \
  -H 'Content-Type: application/json' \
  -d '{"login":"admin","password":"<printed by the installer>"}'
```

A password that is no longer at hand is reset over the control socket:

```
runuser -u keel -- keel ctl auth.password.set principal=<id> password=<new>
```


## Backup and restore

`keel backup` as the user `keel` writes a `tar.zst` archive, by default
`/var/lib/keel/backups/keel-<version>-<instant>.tar.zst`, or where `--out` says; it never
overwrites an archive. The archive carries `manifest.json` (the keel version, the enabled
products with their editions, the schema version of every module ledger, the tables with their
columns), a `COPY` dump of every table of the schema taken in one snapshot over the kernel's own
connection, `/etc/keel/` in full (the parent directory of the configuration file the command
runs with, regular files only), `instance.id` and the TLS material under `/var/lib/keel`, and
the directories every loaded module contributes through `BackupSource`. `secret.key` is
included only with `--with-secret-key`, sealed under a passphrase read from
`KEEL_BACKUP_PASSPHRASE` or asked for on the terminal; API tokens and sessions are stored as
hashes, so an archive without the key holds nothing that opens a session, and only `Secret`
values (the second factor's seed, channel passwords) need it. `keel backup inspect <archive>`
prints the manifest. The same backup runs from the screen "Backups" of the system group or with
`keel ctl backup.create` as a task that writes under `/var/lib/keel/backups/` and offers the
archive for download.

`keel restore <archive> [--with-secret-key]` runs as the user `keel` with the service stopped.
It refuses an archive of another major version or one newer than the binary, and one naming a
product bundle the binary does not carry. It applies the binary's migrations for the products
the manifest names, truncates every table of the schema except the migration ledgers and loads
the dump parents first, so the ledgers stay the binary's and the data becomes the archive's;
tables of the archive without a table in the schema are skipped and named. The enabled set
comes back with `keel_kv`, and the next start logs what it changed. With `--with-secret-key` the
key is opened with the passphrase and written to `/var/lib/keel/secret.key`; without it, the
wrapped data key of the dump is kept when this host's `secret.key` opens it and dropped
otherwise, in which case the next start generates a new data key and the sealed values of the
dump do not open. The configuration file the command runs with is never overwritten: the
archive's copy lands beside it as `config.yaml.restored`. When `/etc/keel` is not writable by
the user `keel`, which the installer's `0750 root:keel` makes the rule, the archive's `/etc/keel`
tree lands under `/var/lib/keel/backups/<archive>.etc/` and the command says so: `agent.yaml` is
root's policy and root copies it. `keel ctl backup.restore archive=<name>` from the screen or the
socket checks the archive and writes `/var/lib/keel/restore.request`; `keel serve` consumes the
request before it opens its port, so the restore runs at the next restart. `hack/backup-vm.sh`
rehearses the round trip on two machines: `take` on the first, `restore` on the second, with
the second factor checked by hand afterwards.

## Enabling another product later

`keel product enable` writes the enabled set while the service is stopped; the modules of the
product migrate at the next start:

```
systemctl stop keel
runuser -u keel -- keel product enable <id>
systemctl start keel
```

`keel product disable <id>` the same way leaves the product's tables in place as orphaned.
Enabling on a running installation through `keel.product.*` with the restart it requests arrives
with M4.

## Uninstalling

```
./uninstall.sh            # stops and disables the four units, removes them, /usr/local/bin/keel and /opt/keel
./uninstall.sh --purge    # also removes /etc/keel, /var/lib/keel, /run/keel, the user keel, and drops the database keel and the role keel
./uninstall.sh --purge --keep-backups   # the same, leaving the archives under /var/lib/keel/backups in place
```

Both ask for confirmation unless `--yes` is passed. Without `--purge` nothing in Postgres is
touched. The Postgres server itself is never removed.

## Trying the installer in a container

The container refusal exists because the update path-unit of M4 does not work there; for a
throwaway test bed with systemd as PID 1 (`--privileged --cgroupns=host` with `/sys/fs/cgroup`
mounted) `KEEL_INSTALL_ALLOW_CONTAINER=1` skips that one check and prints a warning. It is for
testing the script, never for running an installation.

## Checks CI runs

`hack/check-install.sh` runs `bash -n` and shellcheck over the scripts, checks that `--help`
prints the usage, that the four unit files exist, are installed and removed by the scripts, that
`keel.service`, `keel-privileged.service` and `keel-update.path` are enabled, that
`keel-privileged.service` and `keel-update.service` run as root while `keel.service` runs as
`keel`, that the path unit watches `/var/lib/keel/update.request` and starts the oneshot, and,
against the built binary, that every subcommand and flag the units, the scripts and this README
call exists in `keel --help`, that every function they call is defined in the workspace, and
that `config-sample.yaml` parses.
