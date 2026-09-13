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
- Network access to the distribution's package mirror when Postgres is not installed yet.

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
unpacked release per version, and `keel.service` and `config-sample.yaml` are taken from its root
when it carries them. Until the release archive of M4 does, they come from `deploy/` next to the
script, which is why the whole directory is copied; a bare binary always takes them from there.

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
8. Installs `keel.service` from `/opt/keel/versions/<version>/`, placed there from
   `deploy/systemd/` when the release does not carry it, and enables it.
9. As user `keel`: `keel migrate`, `keel product enable <id>` for each product, `keel migrate`
   again so that the products' migrations are applied before the first start.
10. Starts the unit and waits up to 90 s for `GET /readyz` to answer `200`
    (`KEEL_INSTALL_READY_TIMEOUT` changes the limit). A unit that stops or a timeout prints the
    tail of the journal.
11. Creates the first administrator over the control socket:
    `keel ctl keel.principal.create login=<admin> level=admin account=<root account>`, then
    `keel ctl auth.password.set principal=<id> password=<generated>` with a 24-character random
    password.
12. Prints what was installed, the credentials and the commands to check the service.

On a failure after the host was changed the unit is stopped, disabled and removed and
`/opt/keel` is deleted; `/etc/keel`, `/var/lib/keel`, the unix user and the database are left in
place and `uninstall.sh --purge` clears them before another attempt. Steps 3 to 5 are
idempotent, so a partially prepared host can be purged and installed again.

The generated password is passed to `auth.password.set` on the command line, since `ctl` takes
its parameters as `key=value` pairs; it is visible in the process list for the duration of that
call and is masked in the audit trail.

## What it does not do yet

These parts of the operator plane are designed but not built; the installer does not pretend
they exist:

- TLS. The listener is plain HTTP and the session cookie carries `Secure`, so a browser will
  not store it. The self-signed certificate and `trusted_proxies` arrive with M3.
- The privileged agent: `keel-privileged.service`, `agent.yaml` and the host catalogue arrive
  with M2 (the spike) and M4. Nothing runs as root after the installer exits.
- The uid range the host agent may manage. The design allots it to the installation at install
  time next to the unix user; it arrives with `agent.yaml` in M4, since that file is where the
  agent reads it, and the installer creates the user and the group only until then.
- The release channel. `--binary` is required because the signed channel manifest the installer
  fetches a release from does not exist yet; from M4 a missing `--binary` means the channel's
  current version and the flag remains as an override.
- Updates: `keel-update.path`, the signed release manifest and `update rollback` arrive with M4.
  Until then a new version is installed by hand: copy the binary to
  `/opt/keel/versions/<version>/keel`, point `/opt/keel/bin/current` at it,
  `systemctl restart keel`.
- The setup link. `keel.setup.admin` and the one-time `kok_` link printed at the first start
  arrive with M4; the installer creates the administrator over the control socket instead.
- The signature check of the release manifest; the installer verifies the sha256 of the artifact
  only.
- The frontend. The URL answers the API; the browser shell arrives with M2.
- `backup`, `restore`, `doctor` and `support-bundle` (M4).

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
| `listen` | `127.0.0.1:1500` | address and port of the HTTP listener; the port is provisional until the design fixes it |
| `socket` | `/run/keel/core.sock` | control socket `keel ctl` talks to; admitted for the unix user `keel` alone |
| `data_dir` | `/var/lib/keel` | `secret.key`, `instance.id` and everything the kernel stores outside the database |
| `database` | `postgres:///keel?host=/run/postgresql` | connection URL; peer authentication on the unix socket, or a TCP URL with `${KEEL_DB_PASS}` substituted from the environment |
| `tls` | `{ cert: "", key: "", acme: "" }` | parsed and kept; TLS itself arrives with M3 |
| `trusted_proxies` | `[]` | parsed and kept; used from M3 |
| `log` | `{ level: info, format: json }` | `tracing` level and output format; `json` lines go to the journal under systemd, `text` is for a terminal |

Unknown top-level keys are sections for modules (`ConfigSection`); a section no loaded module
claims is logged and ignored. A misspelt key inside `tls` or `log`, an unusable `listen` or
`database` value, or a file that is not YAML fails the start naming the file and the key.

## After the installation

```
systemctl status keel
journalctl -u keel -f
runuser -u keel -- keel ctl keel.account.list --out table
runuser -u keel -- keel product list
curl -sS http://127.0.0.1:1500/readyz
```

`keel ctl` must run as the unix user `keel`: the control socket admits that uid alone, root
included in the refusal. The first login over HTTP:

```
curl -sS -X POST http://127.0.0.1:1500/api/v1/call/auth.login \
  -H 'Content-Type: application/json' \
  -d '{"login":"admin","password":"<printed by the installer>"}'
```

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
./uninstall.sh            # stops and disables the unit, removes the unit, /usr/local/bin/keel and /opt/keel
./uninstall.sh --purge    # also removes /etc/keel, /var/lib/keel, /run/keel, the user keel, and drops the database keel and the role keel
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
prints the usage, and, against the built binary, that every subcommand and flag the scripts and
this README call exists in `keel --help`, that every function they call is defined in the
workspace, and that `config-sample.yaml` parses.
