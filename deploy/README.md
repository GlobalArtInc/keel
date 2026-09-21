# Installing keel

`install.sh` is the one way a keel installation reaches a host. It installs from the signed
release channel of the design and creates nothing the design leaves to the kernel; this file says
what the script does today, the design of the whole operator plane is in
[../docs/design/09-operations.md](../docs/design/09-operations.md).

## Requirements

- A clean Debian 12 or newer, or Ubuntu 24.04 or newer, booted with systemd, not a container.
  A host that already carries `/opt/keel` or `/var/lib/keel` is refused.
- Root.
- `x86_64`: the channel publishes `keel-<ver>-x86_64-linux-musl.tar.zst` alone. Another
  architecture takes a build of its own through `--binary`.
- Network access to the channel (`https://dl.globalart.hu/keel/`) and to the distribution's
  package mirror when Postgres is not installed yet; `curl`, `ca-certificates` and `openssl`
  are installed when they are missing, since the installer verifies the release manifest with
  `openssl` and waits for readiness over https.

## Installing

```
curl -fsSL https://dl.globalart.hu/keel/install.sh | bash -s -- --product demo --yes
```

The script is bash; piped into a plain `sh` it stops at once and prints this line instead of
failing half way through.

The script at the channel root is `deploy/install.sh` of the released commit with the public
release key embedded by `hack/release.py`; the same script is inside every release archive under
`deploy/`. From a checkout, `deploy/install.sh --product demo` does the same with the key
committed in the script, which `hack/check-install.sh` compares with
`crates/release/keys/release.pub`.

Flags:

| Flag | Meaning | Default |
|---|---|---|
| `--product <id>[,<id>]` | bundles to enable; the binary must carry each of them | required |
| `--channel <name>` | the release channel to read, `stable` or `beta` | `stable` |
| `--channel-url <url>` | the address of the channel; `http://` is admitted for a rehearsal channel with a warning, the signature protects the release either way | `https://dl.globalart.hu/keel/` |
| `--version <ver>` | a version of the channel (`versions/<ver>/manifest.json`) instead of the head | the head of the channel |
| `--release-key <file>` | a public key file (`ed25519 <base64> [comment]`) in place of the embedded key, for a rehearsal channel signed with a rehearsal pair | the embedded key |
| `--binary <path\|url>` | a bare `keel` binary or the tar.zst artifact of the Build workflow, or an https URL of either, in place of the channel; a `.sha256` file next to it is verified, no manifest signature is | the channel |
| `--listen <addr:port>` | listen address written to `config.yaml` when it differs from the kernel's default; `<ipv4>:<port>` or `[<ipv6>]:<port>` | `127.0.0.1:1500` |
| `--yes` | do not ask for confirmation | asks |

The installer prints the https address, the certificate fingerprint and the one-time setup link
at the end. Every step announces itself with `==>`; a failure names the step and exits non-zero.

A build that is not on the channel, the artifact `keel-<sha>-x86_64-linux-musl.tar.zst` of the
Build workflow (`gh run download --repo GlobalArtInc/keel --name keel-x86_64-linux-musl`) or a
local `cargo build --release -p keel-bin`, is installed with `--binary` from a copy of the
`deploy/` directory:

```
scp -r deploy/ root@host:/root/keel-deploy
scp keel-<sha>-x86_64-linux-musl.tar.zst keel-<sha>-x86_64-linux-musl.tar.zst.sha256 root@host:/root/
ssh root@host /root/keel-deploy/install.sh --product demo --binary /root/keel-<sha>-x86_64-linux-musl.tar.zst --yes
```

The unit files, the policy template and `config-sample.yaml` come from the archive when it
carries them and from the `deploy/` directory next to the script otherwise, which is why the
whole directory is copied; a release archive carries all of them under `deploy/`.

## What the installer does, in order

1. Refuses a container (`systemd-detect-virt --container`, `/.dockerenv`, `/proc/1/cgroup`),
   a host with `/opt/keel` or `/var/lib/keel`, a host without `apt-get`, a host whose running
   init is not systemd, and, without `--binary`, a host that is not `x86_64`.
2. Fetches the release manifest, `stable/manifest.json` (or `beta/`, or
   `versions/<ver>/manifest.json` with `--version`), and verifies its Ed25519 signature with
   the embedded key before anything is unpacked: the envelope's `payload` and `signature` are
   decoded, the key is wrapped into an SPKI document, and `openssl pkeyutl -verify -rawin`
   checks the signature over the payload bytes, exactly what `keel-release verify` checks.
   A manifest that does not verify ends the installer with nothing unpacked. The verified
   payload names the version and the sha256 of the archive; the installer downloads
   `versions/<ver>/keel-<ver>-x86_64-linux-musl.tar.zst`, checks it against that sha256 with
   `sha256sum`, unpacks the whole archive under `/opt/keel/staging`, requires `keel` at its
   root, runs `keel --version` and refuses a binary whose version differs from the manifest.
   The signed manifest is kept beside the installed version as
   `/opt/keel/versions/<ver>/manifest.json`, where the update path keeps it too. With
   `--binary` the file or URL is taken as it is, the `.sha256` next to it verified when present.
3. Installs `postgresql` from the distribution when no server answers on
   `/run/postgresql`, enables and starts it, and waits for its socket; an existing server is
   used as it is and the version must be 15 or newer. A pre-existing cluster that is down is
   reported, not started.
4. Creates the Postgres role `keel` (login, no password) and the database `keel` owned by it
   when they are absent. Nothing else on the server is touched.
5. Creates the system user and group `keel`, home `/var/lib/keel`, shell `nologin`, allots the
   uid range the host agent may manage, and checks that peer authentication over the unix
   socket admits the user. The range is the first block of 65536 uids from 100000 upwards that
   no account of `getent passwd` and no line of `/etc/subuid` or `/etc/subgid` touches; it
   stays above the range `useradd` allots to ordinary users, so nothing the operator creates by
   hand lands in it.
6. Moves the unpacked release to `/opt/keel/versions/<version>/`, places `config-sample.yaml`
   there from `deploy/` when the release does not carry it, and lays out the symlink
   `/opt/keel/bin/current`, the convenience symlink `/usr/local/bin/keel` (an absent or dangling
   name is taken, an existing symlink is repointed, anything else is left alone and reported),
   `/etc/keel` (`0750 root:keel`) with `config.yaml` (`0640`) and `templates/`, and
   `/var/lib/keel` (`0700 keel:keel`) with `tls/`, `backups/`, `plugins/` and `tasks/`.
   `/run/keel` is created by the unit's `RuntimeDirectory=` at every start.
7. Writes `/etc/keel/config.yaml` with the keys that differ from the kernel's defaults only:
   `listen` when `--listen` was given, `database` when the server's socket is not on port
   5432, both as quoted scalars so that a bracketed IPv6 address stays an address. With the
   defaults the file is empty, which the kernel treats as absent.
8. Writes the host agent policy `/etc/keel/agent.yaml` (`0640 root:keel`) from the release's
   `deploy/agent.yaml`, the one file of the design with the shared keys and a section per
   first-party product (`bill`, `dns`, `panel`), with the allotted range written under
   `policy.uid_range` as `{ from: <first>, to: <last> }`. An `agent.yaml` left by an earlier
   installation is kept as it is and the installer says so, since the operator may have edited
   it; the allotted range is then not written.
9. Installs the four units from `/opt/keel/versions/<version>/deploy/systemd/`, placed there
   from `deploy/systemd/` when the release does not carry them: `keel.service` (the kernel,
   user `keel`, `Type=notify` with `WatchdogSec=60`), `keel-privileged.service` (the one root
   code path, `keel privileged --socket /run/keel/agent.sock --policy /etc/keel/agent.yaml`),
   `keel-update.path` (watches `/var/lib/keel/update.request`) and `keel-update.service` (the
   root oneshot `keel privileged update --from-request` the path unit starts). The first three
   are enabled; the oneshot has no install section and runs only when the path unit fires.
10. As user `keel`: `keel migrate`, `keel product enable <id>` for each product, `keel migrate`
    again so that the products' migrations are applied before the first start.
11. Starts `keel-privileged.service` and `keel-update.path`, then `keel.service`, which
    `systemctl start` holds until the kernel reports `READY=1` over the notify socket, after its
    migrations and the moment its port answers; the installer then checks `GET /readyz` over
    https with `--cacert /var/lib/keel/tls/cert.pem`, the certificate the binary generates at
    its first start (`KEEL_INSTALL_READY_TIMEOUT` changes the limit of that check). A unit
    that stops or a timeout prints the tail of the journal.
12. Prints what was installed, the https address, the certificate path and its SHA-256
    fingerprint read from the journal (through `openssl` when the journal has none), the setup
    link and the commands to check the service. When `listen` is `0.0.0.0` or `::` the printed
    address is the first interface address, which the certificate does not name, so the report
    lists the names the certificate carries — the host name, `localhost` and the loopback
    addresses — and warns that a browser reaching the host by another address gets a name
    warning too.

The first administrator is created in the browser, not by the installer. A start of the kernel
that finds no administrator issues a one-time key and logs the setup link
`https://<host>:<port>/setup?key=kok_…` (`journalctl -u keel -o cat | grep setup` shows it);
the installer reads the key from the journal and prints the link with the address it printed
for the panel. The page behind the link asks for a login and a password and calls
`keel.setup.admin`, a guest-level function that exists only while the installation has no
administrator: it creates the administrator in the root account, stores the password through
the `auth` module under the password policy, and signs the new administrator in. The key dies
with its use and with the process: once an administrator exists the function answers
`not_found` like an unknown function, and a restart before the link is used logs a new link.
The scripts under `hack/` still create their administrators over the control socket, which
stays the operator's way too:

```
runuser -u keel -- keel ctl keel.principal.create login=<login> level=admin account=00000000000000000000000000
runuser -u keel -- keel ctl auth.password.set principal=<id> password=<password>
```

On a failure after the host was changed the units are stopped, disabled and removed and
`/opt/keel` is deleted; `/etc/keel`, `/var/lib/keel`, the unix user and the database are left in
place and `uninstall.sh --purge` clears them before another attempt. Steps 3 to 5 are
idempotent, so a partially prepared host can be purged and installed again.

## What it does not do yet

These parts of the operator plane are designed but not built; the installer does not pretend
they exist:

- TLS beyond the generated pair. The listener speaks TLS alone with the self-signed certificate
  the binary writes under `/var/lib/keel/tls/` at first start, or with the pair `tls.cert` and
  `tls.key` of `config.yaml` name; ACME, the expiry problem and the certificate button of the
  settings arrive later. The installer generates nothing for TLS itself.
- The host policy of the privileged unit. `keel-privileged.service` is installed and runs as
  root, `agent.yaml` is written with the uid range and the product sections, but until the
  `host` module and its agent arrive the unit serves the `update` policy alone and binds no
  socket; the operation catalogue and the audit of operations come with the host layer of M4.
  After the installer exits the privileged unit is the only process of keel running as root,
  which `ps -eo user,comm | grep keel` shows.
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
  from, within the same `contract`. `hack/update-vm.sh` rehearses the root half on a machine
  with a rehearsal key and a channel directory (`run`), and the kernel half through
  `keel update apply --wait` against that channel served over HTTP (`kernel`). The button
  itself is proven against a published 0.2 at the closure of M4.
- `rollback --restore` still stops the service and leaves the restore of the named archive to
  the operator, who runs `keel restore` as the user `keel` and starts the service again;
  `backup` and `restore` themselves are in place (see below).

`keel doctor`, run as the user `keel`, checks the installation end to end and prints one line
per check with `ok`, `warn` or `fail`, exit 1 when a check fails: the data, configuration and
install directories with their owners and modes, `secret.key`, the TLS pair and its expiry, the
three units with their users through `systemctl`, the database (server version, socket, role
rights), the migration ledgers against the binary, the host agent, the license (not required
until M5), free space, that every enabled product is carried by the binary, the orphaned modules
and accounts left in `deleting`. The same checks are the screen "Health" under the system group
(`keel.doctor.run`), with the design's paths; the CLI takes `--opt`, `--unit-dir`, `--systemctl`
and `--agent-socket` for a rehearsal in another tree. `keel support-bundle [--out <path>]` writes
`<data_dir>/support/keel-support-<version>-<instant>.tar.zst` with `config.yaml` redacted (the
database password, every `secret` field of a module section, and the password-like keys of a
section no loaded module claims), `agent.yaml`, the last 10 000 log entries of the running service
(`keel.logs.tail` over the control socket), the migration ledgers, the tail of the audit table,
the open problems, the `/readyz` answer (the kernel exposes no metrics yet) and the doctor report.
`hack/doctor-vm.sh` proves both on a machine installed by `install.sh`.

`keel.service` is `Type=notify` with `NotifyAccess=main` and `WatchdogSec=60`: `keel serve`
writes the notify socket named by `NOTIFY_SOCKET` directly, no library between, `READY=1` once
the migrations ran and the port answers, `WATCHDOG=1` every half of `WATCHDOG_USEC` while it
runs, and `STOPPING=1` when it starts to drain; a kernel that stops answering is restarted by the
service manager after 60 s. `TimeoutStartSec=300` gives the migrations of a large installation
time to run before the first `READY=1`. An abstract notify socket (a name starting with `@`) is
logged and not written; the service manager uses a path. `MemoryMax=50%` is the hardening value
the design leaves to the unit: half of the host's physical memory, so that a leaking kernel is
stopped before it starves Postgres and the operating system, and the same unit fits every host
size. An operator who needs another limit sets it in a drop-in, `systemctl edit keel`, which an
update does not overwrite.

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
ps -eo user,comm | grep keel
runuser -u keel -- keel ctl keel.account.list --out table
runuser -u keel -- keel product list
runuser -u keel -- keel doctor
runuser -u keel -- keel support-bundle
curl -sS --cacert /var/lib/keel/tls/cert.pem https://127.0.0.1:1500/readyz
journalctl -u keel --no-pager -o cat | grep -oE '([0-9A-F]{2}:){31}[0-9A-F]{2}' | tail -n 1
journalctl -u keel --no-pager -o cat | grep -oE 'https://[^ "]+/setup\?key=kok_[0-9a-f]+' | tail -n 1
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
listener with the certificate or its own trust. The setup link the installer printed creates
the first administrator and signs them in; the same login over the API afterwards:

```
curl -sS --cacert /var/lib/keel/tls/cert.pem -X POST https://127.0.0.1:1500/api/v1/call/auth.login \
  -H 'Content-Type: application/json' \
  -d '{"login":"<the login>","password":"<the password>"}'
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

`keel product enable <id>` on a running installation calls `keel.product.enable` over the control
socket; the call records the change and queues the internal task `keel.restart`, which drains the
task queue, takes the pre-change dump of the database with `COPY … TO STDOUT` into
`/var/lib/keel/backups/pre-restart-<version>-<instant>/`, one file per table with a
`manifest.json`, and then makes the kernel exit with status 75. `keel.service` carries
`RestartForceExitStatus=75` and `SuccessExitStatus=75`, so the service manager starts the service
again and the new product's modules migrate before the port opens:

```
runuser -u keel -- keel product enable <id>
```

The same buttons are on the screen "Products" of the system group, each behind a confirmation
that names the restart. `keel product disable <id>` restarts the same way and leaves the product's
tables in place as orphaned, which `keel doctor` lists. While the service is stopped the command
writes the enabled set directly and the modules migrate at the next start:

```
systemctl stop keel
runuser -u keel -- keel product enable <id>
systemctl start keel
```

The lease check the design places before `enable` arrives with the license module of M5; until
then every bundle the binary carries may be enabled.

## Uninstalling

```
./uninstall.sh            # stops and disables the four units, removes them, the sockets under /run/keel, /usr/local/bin/keel and /opt/keel
./uninstall.sh --purge    # also removes /etc/keel (config.yaml and agent.yaml), /var/lib/keel, /run/keel, the user keel, and drops the database keel and the role keel
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
`keel.service` is `Type=notify` with the watchdog while `keel-privileged.service` and
`keel-update.service` run as root, that the path unit watches `/var/lib/keel/update.request`
and starts the oneshot, that the installer embeds the committed release key, verifies the
manifest signature before it unpacks, checks the archive against the manifest's sha256, allots
the uid range and writes it under `uid_range` of the policy written from `deploy/agent.yaml`
(one `uid_range` line, a section per first-party product), waits for the readiness notification,
creates no administrator over the socket and prints the setup link, and, against the built
binary, that every subcommand and flag the units, the scripts and this README call exists in
`keel --help`, that every function they call is defined in the workspace, and that
`config-sample.yaml` parses. Where an OpenSSL 1.1.1 or newer is on the path it also sources
`install.sh` and runs its verification against a manifest signed with a rehearsal pair, which
must be accepted, and against another key, a tampered signature, a tampered payload and a plain
JSON file, which must be refused with no payload written; and it runs the uid allotment against
a stub `getent` and a subordinate range file. `hack/install-vm.sh` is the operator's proof on a
fresh machine: it serves a rehearsal channel and the installer, checks that `curl | sh` stops
with the bash line, runs `curl | bash -s -- --product demo` against them and checks the units,
the processes running as root, the policy and the setup link.
