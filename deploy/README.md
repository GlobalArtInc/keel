# Installing keel

`install.sh` is the one way a keel installation reaches a host. It installs from the signed
release channel and creates nothing the design leaves to the kernel. This file is the operator's
manual of an installation: the installer, the processes and the host agent, KeelDNS on bind,
updates, backups, diagnostics and what the operator proves on a machine. The design of the whole
operator plane is in [../docs/design/09-operations.md](../docs/design/09-operations.md) and the
root boundary in [../docs/design/06-host.md](../docs/design/06-host.md).

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
curl -fsSL https://dl.globalart.hu/keel/install.sh | bash -s -- --product dns --yes
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
| `--channel-url <url>` | the address of the channel, `https://`; a plain `http://` address is refused unless `--insecure-rehearsal-channel` names it a rehearsal | `https://dl.globalart.hu/keel/` |
| `--insecure-rehearsal-channel` | admit an `http://` `--channel-url` for a rehearsal channel on a network you trust, with a warning (the signature protects the release either way, but not against an old signed head replayed until it expires), and write `update: { channel_url: <that address>, insecure_rehearsal_channel: true }` into `/etc/keel/config.yaml`; with an `https://` address it writes nothing about the rehearsal | off |
| `--version <ver>` | a version of the channel (`versions/<ver>/manifest.json`) instead of the head | the head of the channel |
| `--release-key <file>` | a public key file (`ed25519 <base64> [comment]`) in place of the embedded key, for a rehearsal channel signed with a rehearsal pair | the embedded key |
| `--binary <path\|url>` | a bare `keel` binary or the tar.zst artifact of the Build workflow, or an https URL of either, in place of the channel; a `.sha256` file next to it is verified, no manifest signature is | the channel |
| `--listen <addr:port>` | listen address written to `config.yaml` when it differs from the kernel's default; `<ipv4>:<port>` or `[<ipv6>]:<port>` | `127.0.0.1:1500` |
| `--yes` | do not ask for confirmation | asks |

The installer prints the https address, the certificate fingerprint and the one-time setup link
at the end. Every step announces itself with `==>`; a failure names the step and exits non-zero.

The binary of the channel and the artifact of the Build workflow carry `dns` and `panel` and no
`demo`; a plain `cargo build --release -p keel-bin` carries the default features, `demo` among them.

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
   A manifest that does not verify ends the installer with nothing unpacked. A head, that is
   the manifest read without `--version`, must also carry an `expires_at`, whole seconds since
   the Unix epoch inside the signed payload, that lies after `date +%s`; otherwise the installer
   stops, again with nothing unpacked, and prints either `carries no usable expires_at` or `expired at
   <instant>` followed by the advice to try again later, when the channel has been refreshed, or
   to use another channel (`--channel`, `--channel-url`) or a version of it (`--version`, whose
   manifest never expires). The verified
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
   it; the allotted range is then not written. With `panel` among the products the installer
   then installs, configures and starts the daemons of the panel (see "Running KeelPanel").
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
idempotent, so a partially prepared host can be purged and installed again. With `--product panel`
the packages and the configuration of its daemons stay in place as well, and a new attempt
prepares them again.

## What runs, and as whom

| Unit | User | What it runs |
|---|---|---|
| `keel.service` | `keel` | `keel serve`: the kernel, the TLS listener, the control socket `/run/keel/core.sock` and the modules |
| `keel-privileged.service` | root | `keel privileged --socket /run/keel/agent.sock --policy /etc/keel/agent.yaml`: the one root code path, serving the update policy and the host agent |
| `keel-update.path` | root | watches `/var/lib/keel/update.request` and starts the oneshot below |
| `keel-update.service` | root | `keel privileged update --from-request`, the root half of an update |

After the installer exits, `keel privileged` is the only process of keel running as root, which
`ps -eo user,comm | grep keel` shows.

`keel.service` is `Type=notify` with `NotifyAccess=main` and `WatchdogSec=60`: `keel serve`
writes the notify socket named by `NOTIFY_SOCKET` directly, no library between, `READY=1` once
the migrations ran and the port answers, `WATCHDOG=1` every half of `WATCHDOG_USEC` while it
runs, and `STOPPING=1` when it starts to drain; a kernel that stops answering is restarted by the
service manager after 60 s. `TimeoutStartSec=300` gives the migrations of a large installation
time to run before the first `READY=1`. An abstract notify socket (a name starting with `@`) is
logged and not written; the service manager uses a path. `RestartForceExitStatus=75` and
`SuccessExitStatus=75` make the exit status 75, the restart a product switch requests, a restart
the journal does not count as a failure. `MemoryMax=50%` is the hardening value the design leaves
to the unit: half of the host's physical memory, so that a leaking kernel is stopped before it
starves Postgres and the operating system, and the same unit fits every host size. An operator
who needs another limit sets it in a drop-in, `systemctl edit keel`, which an update does not
overwrite.

## The host agent

`keel-privileged.service` serves the host policy of `/etc/keel/agent.yaml` on
`/run/keel/agent.sock`. The socket admits the account `keel` alone, checked by peer credentials
(`--user` names another account for a rehearsal). Every operation of the catalogue is one line in
`journalctl -u keel-privileged` with the actor, the request id and the outcome, and a request
outside the policy is refused with `host.error.policy` and appears there too. A
`host.user.delete` that had to end processes adds one more line with the outcome
`processes_stopped`, the same actor and request id, the user, its uid and the number of processes
sent SIGTERM and SIGKILL, summed over the rounds, and the number of rounds:

- `host.user.*` and `host.group.*` inside `uid_range`, under the product's `user_prefix`;
  `host.user.delete` first ends the processes of the user (SIGTERM, then SIGKILL after 3 seconds,
  re-sent for 2 more seconds to whatever is still there) and then runs `userdel`; when `userdel`
  still finds the user busy (a worker respawned in between) the processes are ended again and
  `userdel` retried after 200 ms, up to five rounds in all, and the deletion ends in
  `host.error.exec_failed` if processes survive or the fifth round fails;
  `host.user.modify` sets the login shell, `/usr/sbin/nologin` or one listed in `/etc/shells`,
  and locks or unlocks the account through `usermod`, a lock also expiring the account so that
  no key opens it either; an
  account whose primary group lies outside the range is refused, and a program run as one of
  these accounts (`keel agent-exec`, a re-exec of the binary) starts with a cleared environment,
  the fixed `PATH`, `LC_ALL=C`, the account's `HOME`, `USER` and `LOGNAME`, in `/`;
- `host.fs.*` and `host.template.apply` under `file_roots`, addressed by a root key and a name
  with the product's `drop_in_prefix`, written through a temporary file and a rename, never
  following a symbolic link, never setuid, never writable by every account, and read up to
  8 MiB;
- `host.service.*` for the units listed in `services`, through `systemctl` as a process; an
  entry may carry one `*` standing for a version of digits and dots, so `php*-fpm` admits
  `php8.4-fpm` and nothing else, and the list shows the installed units such an entry matches;
- `host.pkg.*` over `apt-get`, `apt-cache` and `dpkg-query`, with each name taken as that package
  and never as a pattern or a trailing `+` or `-` action, and only for the packages listed under
  `packages.allow`: a request naming another package is refused with `host.error.policy` before
  any tool runs, and a policy without the section allows no package;
- `host.cron.*` writing the product's drop-ins under `/etc/cron.d`, whose jobs run an entry of
  `exec_allow` as `keel` or as the product's own users;
- `host.quota.*` over `setquota` and `repquota`, answering `host.error.unavailable` on a host
  without them;
- `host.exec` for the entries of `exec_allow`, a binary name run without arguments or `name:
  [binary, arguments…]` run with exactly those; given a `dest`, the output goes into that file of
  a root instead of the answer, created afresh with mode 0600 and owned by the kernel's user.

The agent also executes the operations and renders the configuration templates the compiled
products contribute. A template is read from `/etc/keel/templates/<id>` when the operator put one
there and from the binary otherwise, rendered, swapped into place, checked with the named check
of `checks` with the file in place, reloaded, and rolled back when the check or the reload fails,
so a broken template never reaches the running service. A product operation runs only for a
product whose section of `agent.yaml` grants its policy class under `classes`, as in
`demo: { drop_in_prefix: keel-demo-, classes: { note: { root: notes } } }`; its journal line
carries `class=`.

The kernel reaches the agent through the `host` module, which a product loads by naming it: `dns`
does, `demo` does not. Its section of `config.yaml` is `host: { socket, timeout_seconds }`, by
default `/run/keel/agent.sock` and 60 seconds; the kernel connects only when a module calls, and
an agent error reaches the form as its key (`host.error.*`) with the tool's output as the detail.
While no enabled product loads `host`, `keel doctor` reports the agent as not in use.

Every process an operation runs has a time limit, so a tool that never exits cannot hold the
lock all operations take; a plugin the agent spawns is a long-lived service by design, not a
process of an operation, and has no limit. When the limit passes, the process and its process group are killed, the
lock is released, the operation answers `host.error.timeout` and its journal line carries that
outcome. The limits are the `timeouts` section: `default_seconds`, 120 when the section is
missing, and `tools`, a map from a tool to its own seconds. A tool is named as the policy names it:
a check such as `nginx.test`, an entry of `exec_allow` such as `certbot`, or one of the tools the
agent runs, `systemctl`, `getent`, `useradd`, `userdel`, `groupadd`, `groupdel`, `usermod`,
`apt_get`, `dpkg_query`, `apt_cache`, `setquota`, `repquota`, `kill` and `keel`; a name the policy
does not know, a limit of zero and one over 86400 seconds are refused when the policy is read. The
limit holds for each process, so an operation that runs several may take their sum. The release
ships:

| Tool | Seconds | Used by |
|---|---|---|
| `apt_get` | 1800 | `host.pkg.install`, `host.pkg.remove`, `host.pkg.refresh` |
| `certbot` | 600 | `host.exec` of the entry `certbot` |
| `keel` | 600 | `host.home.*`, the helper that lists, reads and writes a hosting user's files for the file manager |
| `mariadb` | 3600 | the statements and the `import` of `mariadb.exec` |
| `mariadb-dump` | 3600 | the `dump` statement of `mariadb.exec` |
| `mail.archive` | 3600 | `host.exec` of the entry `mail.archive`, the archive of the maildirs |
| every other tool, check and entry | 120 | `default_seconds` |

The kernel's own `host.timeout_seconds` (60 by default, at most 3600) also bounds how long a
caller waits for an answer, whatever the limit of a tool in this table.

A tool whose output, stdout and stderr together, exceeds 16 MiB, the frame limit of the client, is
killed and answers `host.error.io`. A tool that exits while a descendant still holds its output
pipes returns what it wrote after a grace of 2 seconds and the agent logs a warning; the
descendant itself is left running.

A host installed earlier keeps its own `/etc/keel/agent.yaml`, which an update never touches: add
the `timeouts` and `packages` sections of the shipped `deploy/agent.yaml` by hand, otherwise
`host.pkg.*` refuses every package and every tool is cut at 120 seconds. A process killed at its
limit can leave work half-applied: a killed `apt-get` may need `dpkg --configure -a`, and a killed
`mariadb` import leaves the part of the dump it had loaded.

`agent.yaml` is the operator's file: the installer writes it from the release's
`deploy/agent.yaml` with the allotted `uid_range`, keeps an existing one, and an update never
touches it. Its shared keys are `uid_range`, `file_roots`, `services`, `checks`, `exec_allow`,
`timeouts` and `packages`; under `products`, one section per product id carries `user_prefix`,
`drop_in_prefix` and `classes`. `packages: { allow: [...] }` lists the packages `host.pkg.*` may
install, remove or read, and the release lists exactly those `install.sh` installs: `curl`,
`ca-certificates`, `openssl`, `zstd`, `postgresql` and the packages of `panel` named above. The
file has no secret and travels verbatim in the support bundle.

## Running KeelDNS

`--product dns` installs the bundle; bind itself is the operator's, and neither the installer
nor the kernel wires it. The defaults of the `dns` section of `config.yaml` follow the Debian
layout of bind:

| Key | Default | Meaning |
|---|---|---|
| `zones_dir` | `/etc/bind/zones` | where the zone files `keel-dns-<zone>.zone` are written; the `zones` root of `file_roots` |
| `include_dir` | `/etc/bind/keel` | where `keel-dns-zones.conf`, the index of the zones, is written; the `named` root of `file_roots` |
| `nameserver` | `localhost` | the primary name server of the SOA and NS records |
| `hostmaster` | `hostmaster.localhost` | the mailbox of the SOA record |

Before the first zone, the host needs bind with its tools, both directories, the index and its
include line, so that `named-checkconf -z`, the check `named.check` of `agent.yaml`, passes:

```
apt-get install -y bind9 bind9-utils bind9-dnsutils
install -d -m 0755 -o root -g bind /etc/bind/zones /etc/bind/keel
install -m 0644 -o root -g root /dev/null /etc/bind/keel/keel-dns-zones.conf
echo 'include "/etc/bind/keel/keel-dns-zones.conf";' >> /etc/bind/named.conf.local
named-checkconf -z && systemctl reload named
```

A zone is then created on the Zones screen, over the control socket or with an API token; its
file is rendered after the transaction commits, checked, and bind reloaded:

```
runuser -u keel -- keel ctl dns.zone.create --as <login> name=example.test
runuser -u keel -- keel ctl dns.zone.list --as <login> --out table
dig @127.0.0.1 example.test SOA
```

A zone whose check fails turns to `error` with the tool's output, raises the problem
`dns.zone.failed`, and leaves the previous file and the running bind as they were.

## Running KeelPanel

`--product panel` installs the bundle and the daemons it drives. Before the units of keel are
installed, the installer answers the mailer question of postfix with `Internet Site`, installs
`nginx php-fpm mariadb-server mariadb-client postfix dovecot-core dovecot-imapd dovecot-lmtpd
vsftpd libpam-pwdfile` from the distribution in one `apt-get install`, and prepares what the
roots of `file_roots` in `/etc/keel/agent.yaml` need: `/var/www`, `/var/lib/keel-panel-acme`,
`/var/lib/keel-panel-dumps` and `/var/lib/keel-panel-backups` for root, `/etc/ssl/keel` for root
alone and `/var/lib/keel-panel-transfer` for `keel` alone. The kernel unit is sandboxed with
`ProtectSystem=strict` and writes only `/var/lib/keel` and `/run/keel`, while the kernel itself
stages every upload and download in the transfer root; so the installer also writes the drop-in
`/etc/systemd/system/keel.service.d/keel-panel.conf`, which adds that root to `ReadWritePaths`,
and systemd reads it with the `daemon-reload` that precedes the first start of `keel.service`. It
points postfix and dovecot at the
mailboxes and vsftpd at the FTP accounts as described below, checks each configuration with the
check the policy names for it, then enables and restarts `nginx`, `mariadb`, `postfix`,
`dovecot`, `vsftpd` and every `php*-fpm` unit and stops if one of them is not running. The
dovecot drop-in is written for dovecot 2.4, the one of Debian 13. `hack/panel-vm.sh install`
runs the installer on a clean machine and checks every package, unit and root, and that
`systemctl show keel -p ReadWritePaths` names the transfer root. Enabling `panel` later on an
installation of another product installs none of this.

An operator who enables `panel` that way, with `keel product enable panel`, prepares by hand, as
root and before the restart that command performs, the one root the kernel writes itself and the
drop-in that lets the sandboxed unit write it; without them every upload and download of the file
manager fails with `hosting.file.error.transfer` and `ReadOnlyFilesystem` in the journal:

```
install -d -m 0700 -o keel -g keel /var/lib/keel-panel-transfer
install -d -m 0755 -o root -g root /etc/systemd/system/keel.service.d
printf '%s\n' '[Service]' 'ReadWritePaths=/var/lib/keel-panel-transfer' >/etc/systemd/system/keel.service.d/keel-panel.conf
chmod 0644 /etc/systemd/system/keel.service.d/keel-panel.conf
systemctl daemon-reload
```

The transfer root has to exist before the unit starts, since a missing path in `ReadWritePaths`
stops the unit at its namespace setup. The other roots of the policy are written by the host
agent as root and need no drop-in. The drop-in survives an update, which refreshes the unit
file alone, and `uninstall.sh` removes it.

A hosting user is an operating-system account named
`hp_<name>` inside `uid_range`, with its home `/var/www/keel-panel-<name>` under the `www` root
of `file_roots` and no login shell unless shell access is granted. It is created after the
transaction commits; suspending its account locks it, resuming unlocks it, and deleting the
user or its account removes the account and its home; the kernel retries the user deletion up to six
times with a pause of two seconds when the host agent reports `host.error.exec_failed`:

```
runuser -u keel -- keel ctl hosting.user.create --as <login> name=alice
runuser -u keel -- keel ctl hosting.user.list --as <login> --out table
getent passwd hp_alice
```

A site belongs to a hosting user: a domain with its aliases, a document root inside the user's
home (the domain unless another relative path is given) and a PHP version or none. After the
transaction commits its php-fpm pool `keel-panel-<domain>.conf` is rendered into the `pool.d` of
that PHP, the root `pools-<version>` of `file_roots`, checked with `php<version>-fpm.test` and
`php<version>-fpm` reloaded; then its server block `keel-panel-<domain>.conf` is rendered into
`/etc/nginx/conf.d`, the root `vhosts`, checked with `nginx.test` and nginx reloaded. A failed
check leaves the previous file in place and turns the site to `error` with the tool's output.
Suspending its account makes the server block answer 403, and deleting the site or its account
removes both files and reloads both services. A hosting user that still has sites cannot be
deleted:

```
runuser -u keel -- keel ctl hosting.site.create --as <login> user=<user id> domain=example.test 'php="8.4"'
runuser -u keel -- keel ctl hosting.site.list --as <login> --out table
curl -H 'Host: example.test' http://127.0.0.1/
```

The shipped policy names the pool roots and the checks of PHP 8.2, 8.3 and 8.4, the versions
Debian and Ubuntu carry; another version needs its `pools-<version>` root and its
`php<version>-fpm.test` check added to `/etc/keel/agent.yaml`, which the installer keeps across
updates. The panel does not create the document root: the hosting user fills it, and nginx needs
to search the home to serve it. The policy also names the roots and the units the later modules
of the bundle use: the certificates under `certs`, and `postfix`, `dovecot`, `mariadb` and
`vsftpd`.

Mail domains and mailboxes belong to an account. After each change the three virtual maps of
postfix (`keel-panel-domains`, `keel-panel-mailboxes`, `keel-panel-aliases`) are rendered into
`/etc/postfix/keel`, the root `mail`, each checked with `postfix.check`, then the `postmap` entry
of `exec_allow` rebuilds them and postfix is reloaded; the passwd-file `keel-panel-passwd` of
dovecot, owned by `dovecot` with mode 0640 and carrying `{ARGON2ID}` hashes, is rendered into
`/etc/dovecot/keel`, the root `dovecot`, checked with `dovecot.check` and dovecot reloaded. A
mailbox's home is `/var/lib/keel-panel-mail/keel-panel-<mailbox id>`, the root `maildirs`, which
dovecot fills as the user `vmail`; the backup carries it through the `mail.archive` entry. The
installer creates the system user `vmail`, the empty maps, the empty passwd-file, the `main.cf`
keys `virtual_mailbox_domains`, `virtual_mailbox_maps`, `virtual_alias_maps` and
`virtual_transport = lmtp:unix:private/dovecot-lmtp`, and the dovecot drop-in
`/etc/dovecot/conf.d/90-keel-panel.conf` that reads the passwd-file and listens for LMTP in the
spool of postfix, and keeps the domain in the LMTP lookup, because the distribution's
`20-lmtp.conf` strips it. It also points the INBOX at the maildir root, because the
distribution's `10-mail.conf` points it at `/var/mail`.

FTP accounts belong to a site and are virtual users of vsftpd, named `hp_<name>` and chrooted to
a directory under the document root of the site, given relative to that root; a directory that
leaves the site is refused. After each change the user file `keel-panel-passwd`, one `login:hash`
line per account with a SHA-512 crypt hash, is rendered into `/etc/vsftpd/keel`, the root `ftp`,
and each account gets a file named after its login in `/etc/vsftpd/keel-users`, the root
`ftp-users`, which the policy lists under `account_roots` of `panel` so that its files carry the
names of the product's accounts rather than the drop-in prefix: `guest_username` maps the session
onto the hosting user of the site and `local_root` sets the chroot. Both belong to root with mode
0600 and are checked with `vsftpd.check`, which parses `/etc/vsftpd.conf`; vsftpd reads them at
each login, so nothing is reloaded. The logins of a suspended account leave the user file, and
deleting a site or its account removes its FTP accounts. The installer keeps the distribution's
file as `/etc/vsftpd.conf.orig` and writes a `/etc/vsftpd.conf` for virtual users mapped by the
files of `/etc/vsftpd/keel-users` and authenticated by the PAM service `keel-panel-ftp` on
`pam_pwdfile` against the user file, and `hack/panel-vm.sh ftp` proves the chroot, an upload as
the hosting user, the suspension and the removal on a machine.

Site backups archive a site with its files, the databases of its hosting user and the mailboxes
of the mail domain named like the site, as a task, into `/var/lib/keel-panel-backups`, the root
`backups`, one `keel-panel-<backup id>.tar.zst` per backup owned by the `keel` user with mode 0600
so that the panel can offer it for download; the directory itself belongs to root with mode 0711.
The databases are dumped through `mariadb.exec` into `dumps` first; the agent operation
`hosting.backup.pack` of the policy class `backup` walks the document root and the maildirs by
descriptor without following a link, and `hosting.backup.unpack` puts the files back as the
hosting user and the mail as the owner of `/var/lib/keel-panel-mail`, while `mariadb.exec import`
loads the dumps. The agent records the SHA-256 of every dump and archive it writes in a file
`.<name>.sha256` beside it, owned by root with mode 0600, which no operation of the catalogue can
name: a dump that changed after `mariadb-dump` wrote it is not packed, an archive that changed after
the pack is not unpacked, and `import` loads only a dump in `dumps` that the unpack wrote as root
with its digest intact. The `mariadb` entry of `exec_allow` carries `--sandbox`, and the agent adds
it when an entry lacks it, so the client refuses the shell and file commands of a dump. A schedule per site is five cron fields in UTC checked at the top of every hour
by the job `hosting.backup.scheduled`, and each finished backup keeps the newest archives of the
site, seven or the count of its schedule. Deleting a site or its account removes its archives. The
archive root is `hosting_backup.archives_dir` of `config.yaml`; `hack/panel-vm.sh backup` proves a
backup, a restore and the retention on a machine. The archives stay on this host: a remote target
comes later.

Every site has at most one certificate, requested from an ACME directory over HTTP-01 or uploaded
as a PEM chain and key. The directory is the setting `hosting.certificate.directory`, Let's
Encrypt's production directory unless it names staging or a private directory by URL, with
`hosting.certificate.email` as the optional contact; a private directory's trust anchor is
`hosting_certificate.directory_ca` of `config.yaml`. The ACME account is created once per
directory and sealed in the database. An order runs as the task `hosting.certificate.issue`
after `hosting.certificate.request` or the site wizard's Let's Encrypt box: the challenge file is
placed under `/var/lib/keel-panel-acme`, the root `acme`, and served by the site's server block,
the pair lands under `/etc/ssl/keel`, the root `certs`, as `keel-panel-<id>.crt` and `.key`, and
the server block is rendered again with `listen 443 ssl`. The cron job
`hosting.certificate.renew` renews every ACME certificate thirty days before its expiry, and the
problem `hosting.certificate.expiring` is raised fourteen days before, critical once expired. The
site must resolve to this host from the internet for the challenge to pass:

```
runuser -u keel -- keel ctl settings.set key=hosting.certificate.directory 'value="https://acme-staging-v02.api.letsencrypt.org/directory"'
runuser -u keel -- keel ctl hosting.certificate.request --as <login> site=<site id>
runuser -u keel -- keel ctl hosting.certificate.list --as <login> --out table
```

`hack/panel-vm.sh ssl <domain>` proves a certificate from the staging directory on a machine whose
domain points at it, and `--production` from Let's Encrypt itself.

Databases are MariaDB databases and users per account, named after a hosting user of the
account, an underscore and up to sixteen lowercase letters or digits (`hp_alice_wp`), created,
granted, locked and dropped through the agent operation `mariadb.exec` with a fixed statement set;
a password is never stored by keel. The limits `hosting.database.max` and
`hosting.database.user.max` are 10 per account. Suspending an account locks its database users,
and deleting it drops them with its databases. `hack/panel-vm.sh db` proves it against a real
MariaDB.

The file manager, under "Files" of the panel group, works in the home of a hosting user as that
user: every operation runs through `keel agent-home` under the user's uid, and uploads and
downloads pass through `/var/lib/keel-panel-transfer`, the root `transfer`, which only `keel`
may enter. The configuration editor, under "Site configuration" and for administrators only, adds
lines to a site's server block and pool: listed nginx directives without files, sockets or names,
`location` and `if` blocks two deep, and process-manager and `php_value` lines; a save is
checked with `nginx -t` or `php-fpm -t` before it is stored, and a refused check shows its
output on the field. The screen "Services" of the panel group lists the units of the policy's
`services` with their state, memory and start and lets an administrator start, stop, restart and
reload them; a failed unit raises `hosting.service.down`. `hack/panel-vm.sh files`,
`hack/panel-vm.sh config` and `hack/panel-vm.sh services` prove the three on a machine.

## Licenses

The installation runs without a lease: every bundle the binary carries may be enabled at its
default edition, and nothing degrades. A lease is issued by the vendor for the installation's
instance id, which `keel ctl license.get` shows, and installed over the control socket; it
lists the products and the edition of each:

```
runuser -u keel -- keel ctl license.get
runuser -u keel -- keel license install /root/keel.lease
```

Once a lease is installed, `keel product enable` refuses a product it does not list, and the
functions of an edition it does not grant are hidden and answer `402`. After `expires_at` the
installation keeps working for the lease's grace days and raises `license.expiring`; after the
grace it turns read-only, raising `license.expired`: reads, logins, `su` and `license.install`
keep working, writes answer `402` until a new lease is installed. A product a new lease no longer
lists turns read-only on its own and raises `license.product_unlisted`; its data is never
deleted. The clock of the lease only moves forward, so setting the system clock back does not
revive an expired lease.

## Updates

The screen "Updates" of the system group shows the running version and the head of the channel
the setting `update.channel` names (`stable` or `beta`), read from the `update.channel_url` key of
`config.yaml` (default `https://dl.globalart.hu/keel/`, read over https alone: with a plain
`http://` address the update functions answer `update.error.channel_insecure` and the rest of the
service runs on, unless `update.insecure_rehearsal_channel: true` stands beside it, which is for a
rehearsal channel only); a daily job raises the problem
`update.available`. Its button, or the command below, runs `update.apply` as a task: it drains the
task queue, takes the pre-update dump with `COPY … TO STDOUT` into
`/var/lib/keel/backups/pre-update-<version>-<instant>/`, one file per table, and writes
`update.request`; the task ends `done` and the root unit takes over:

```
runuser -u keel -- keel update check
runuser -u keel -- keel update apply --wait
runuser -u keel -- keel update rollback
```

`keel-update.service` reads the version, the channel and the requesting principal from the request
and nothing else, reads the head of the channel and refuses it, with nothing downloaded or
changed, when its signature fails or it is expired or carries no `expires_at` (the unit's
journal says `expired at <instant>` or `carries no expires_at`), downloads
`versions/<ver>/manifest.json` and the archive, verifies the Ed25519
signature of the manifest against the release key compiled into the binary, the archive's sha256
against the manifest and the version against `min_upgrade_from` of the channel head, unpacks into
`/opt/keel/versions/<ver>/`, switches `bin/current` and keeps the version it left as
`bin/previous`, refreshes the unit files from the release's `deploy/systemd/` and restarts
`keel-privileged.service` and `keel.service`. The new binary migrates before its port opens.

`update.rollback`, its button and `keel update rollback` write the same request for the version
the installation was updated from, within the same `contract`; the root side installs a lower
version only when `bin/previous` points at it. Across a contract, the rollback is the operator's,
with the backup taken before the update:

```
/opt/keel/bin/current privileged update rollback --restore /var/lib/keel/backups/<archive>.tar.zst
runuser -u keel -- keel restore /var/lib/keel/backups/<archive>.tar.zst --with-secret-key
systemctl start keel
```

`--restore` stops `keel.service` instead of restarting it and prints the restore the operator
runs next.

The heads of the two channels expire. `hack/release.py` signs every head with an `expires_at` 30
days ahead (`--expires-days`, 1 to 365) and the workflow `refresh-channel.yml` signs the current
heads again every Monday at 04:17 UTC, and on demand from the Actions tab, with
`hack/release.py --refresh stable` and `--refresh beta`: it reads the head from the raw repository
with the deploy account, a missing `beta` head is reported and skipped, a missing `stable` head
fails the step, and the `beta` step runs even when the `stable` one failed. `keel-release` is built
in a step without secrets; the key and the deploy account are the repository secrets
`KEEL_RELEASE_KEY` and `KEEL_RELEASE_ASTRUM_PASSWORD` of the release workflow, handed only to the
two refresh steps. The workflow and `release.yml` share the concurrency group `release-channel`,
so a release and a refresh never run at once (GitHub keeps one waiting run per group: a run that
is replaced while it waits is started again from the Actions tab). An installation that meets an
expired head keeps running the version it runs: `keel update check` and the screen "Updates"
answer `update.error.head_expired` (`update.error.head_no_expiry` for a head without the field),
no update is offered or applied until the channel is refreshed, and nothing else changes. A
rollback to `bin/previous` does not need a fresh head (it still needs the channel to answer;
`keel privileged update rollback` needs neither). The same refresh is run by hand when the
workflow cannot run:

```
KEEL_RELEASE_KEY="$(cat release.key)" KEEL_RELEASE_ASTRUM_PASSWORD=… python3 hack/release.py --refresh stable
```

Two operational notes. Heads published before the release that introduces `expires_at` carry no
expiry and the new clients refuse them: after that release every head it did not touch, typically
`beta` when the release is a stable one, must be refreshed once with `workflow_dispatch` of
`refresh-channel.yml`. And GitHub disables the scheduled workflows of a public repository after 60
days without activity in it: the heads then stop being refreshed, and each expires at most 30 days
after its last refresh, so a repository that may go quiet needs the refresh run by hand.

A channel is read over https alone. A plain `http://` address, in `install.sh --channel-url`, in
`update.channel_url` or in `keel privileged update --channel-url`, is refused unless it is named a
rehearsal: `--insecure-rehearsal-channel` of the installer and of `keel privileged update`,
`update.insecure_rehearsal_channel: true` in `config.yaml`. Outside a rehearsal a redirect that
leaves https or changes the host is not followed. A head that has expired is refused; that is the
whole defence against a replayed head, and within its window an old signed head is still accepted.
The root side takes the version to install from the request and checks it against its own signed
manifest, without binding it to the version the verified head names. The signature protects what
is installed over plain http as well.

## Doctor and the support bundle

`keel doctor`, run as the user `keel`, checks the installation end to end and prints one line
per check with `ok`, `warn` or `fail`, exit 1 when a check fails: the data, configuration and
install directories with their owners and modes, `secret.key`, the TLS pair and its expiry, the
units with their users through `systemctl`, the database (server version, socket, role rights),
the migration ledgers against the binary, the host agent, the license (not required until M5),
free space, that every enabled product is carried by the binary, the orphaned modules and the
accounts left in `deleting`. The same checks are the screen "Health" under the system group
(`keel.doctor.run`), with the design's paths; the CLI takes `--opt`, `--unit-dir`, `--systemctl`
and `--agent-socket` for a rehearsal in another tree. `keel support-bundle [--out <path>]` writes
`<data_dir>/support/keel-support-<version>-<instant>.tar.zst` with `config.yaml` redacted (the
database password, every `secret` field of a module section, and the password-like keys of a
section no loaded module claims), `agent.yaml`, the last 10 000 log entries of the running service
(`keel.logs.tail` over the control socket), the migration ledgers, the tail of the audit table,
the open problems, the `/readyz` answer (the kernel exposes no metrics yet) and the doctor report.
Whatever the member, every session (`ks_`), API token (`kat_`), one-time login key (`kok_`) and
peer token (`kpt_`) found in its text is replaced by `<prefix>_<redacted>`, so the setup link in
the log ring of a fresh installation cannot be used by whoever reads the bundle.

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
| `tls` | `{ cert: "", key: "", acme: "" }` | `cert` and `key` name an operator's PEM pair, both or neither; empty means the pair the binary generates under `<data_dir>/tls` at first start; a non-empty `acme` fails the start, since ACME is left to a later milestone |
| `trusted_proxies` | `[]` | CIDRs of reverse proxies whose `X-Forwarded-For` and `X-Forwarded-Host` the kernel believes; a bare address stands for its `/32` or `/128` |
| `log` | `{ level: info, format: json }` | `tracing` level and output format; `json` lines go to the journal under systemd, `text` is for a terminal |

Unknown top-level keys are sections for modules (`ConfigSection`): `update: { channel_url,
insecure_rehearsal_channel }` of the `update` module (default `https://dl.globalart.hu/keel/` and
`false`), `host: { socket, timeout_seconds }` of the `host` module (see "The host
agent") and `dns: { zones_dir, include_dir, nameserver, hostmaster }` of KeelDNS (see "Running
KeelDNS"). A section no loaded module
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
the directories every loaded module contributes through `BackupSource`. A source reads the
configuration of its module from the context it runs in. The command runs the attach hooks of
the modules, never their start hooks, so the archive is written with the service running or
stopped. The sources of the panel product reach the agent socket the same way, without any
module starting. The kernel's `host.timeout_seconds` (60 by default) also bounds the database
dump and the mail archive of a CLI backup, so a large database or mailbox needs it raised in the
`host` section of `config.yaml`. A CLI backup must not run at the same time as a backup of the
running service. `secret.key` is included only with `--with-secret-key`, sealed under a passphrase read from
`KEEL_BACKUP_PASSPHRASE` or asked for on the terminal; API tokens and sessions are stored as
hashes, so an archive without the key holds nothing that opens a session, and only `Secret`
values (the second factor's seed, channel passwords) need it. `keel backup inspect <archive>`
prints the manifest. The same backup runs from the screen "Backups" of the system group or with
`keel ctl backup.create` as a task that writes under `/var/lib/keel/backups/` and offers the
archive for download. Archives stay in that local directory; copying them off the host is the
operator's until the S3 target of M5.

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

Once a lease is installed, `enable` over the socket refuses a product the lease does not list;
the direct write on a stopped service has no gate, and a product enabled that way turns read-only
at the next start. `keel product disable dns` on an installation where `panel` uses `dns` for its
sites is refused unless `--switch-remote` is given, after which the DNS operations of the sites
answer `unavailable` until a peer is paired. Enabling `panel` this way installs none of its
daemons: install the packages and prepare the roots and configuration as "Running KeelPanel"
describes, or install on a fresh machine with `--product panel`.

## Uninstalling

```
./uninstall.sh            # stops and disables the four units, removes them, the sockets under /run/keel, /usr/local/bin/keel and /opt/keel
./uninstall.sh --purge    # also removes /etc/keel (config.yaml and agent.yaml), /var/lib/keel, /run/keel, the user keel, and drops the database keel and the role keel
./uninstall.sh --purge --keep-backups   # the same, leaving the archives under /var/lib/keel/backups in place
```

Both ask for confirmation unless `--yes` is passed. Without `--purge` nothing in Postgres is
touched. The Postgres server itself is never removed. The daemons the panel installed with
`--product panel`, their packages and their configuration are left in place as well.

## Trying the installer in a container

The container refusal exists because the update path unit and the host agent do not work there; for a
throwaway test bed with systemd as PID 1 (`--privileged --cgroupns=host` with `/sys/fs/cgroup`
mounted) `KEEL_INSTALL_ALLOW_CONTAINER=1` skips that one check and prints a warning. It is for
testing the script, never for running an installation.

## Proving on a machine

What needs root, a real service manager or a real bind is proven by the operator on a virtual
machine with the scripts under `hack/`, run from a checkout; each prints what it checks and exits
non-zero on the first failure, and its transcript is what the exit checklist of the milestone
records.

| Script | What it proves |
|---|---|
| `hack/install-vm.sh prepare <archive> <dir>`, then `run <dir> --product <id>` on the machine | `curl \| bash` from a rehearsal channel signed with a rehearsal pair on a fresh machine: the units, the processes running as root, the policy and the setup link; `curl \| sh` stops with the bash line |
| `hack/update-vm.sh prepare <archive> <dir>`, then `run <dir>` and `kernel <dir>` | the root half of an update and its rollback through the path unit, and the kernel half through `keel update apply --wait` against the rehearsal channel |
| `hack/backup-vm.sh take <archive>` on one machine, `restore <archive>` on another | a backup with `--with-secret-key` restored on a second installation of the same products; the second factor is checked by hand afterwards |
| `hack/doctor-vm.sh [dir]` | `keel doctor` green against the real units, a bad mode named, and a support bundle free of secrets |
| `hack/product-vm.sh <id> [<function>]` | `keel product disable` and `enable` on the running service: the exit 75, the restart, the dump, the orphaned tables and the function answering again |
| `hack/host-vm.sh [--cleanup]` | the agent: an account inside the range running a program, a zone rendered, checked, reloaded and served, a broken template rolled back without a restart of `named`, and an operation outside `file_roots` refused and found in the journal |
| `hack/dns-vm.sh install <archive>`, then `run [--browser-zone <name>]` | KeelDNS on bind: zones created over the control socket and with a token answered by `dig`, and a broken zone template rolled back while `named` keeps answering |
| `hack/panel-vm.sh install <archive>`, then `run` | `install.sh --product panel` on a clean Debian 13 machine with every package, unit and root checked; KeelPanel's hosting users: an account `hp_<name>` inside the range with its home, the shell granted and taken back, locked by the suspension of its account and unlocked by the resumption, and removed with its home when the account is deleted; a site served through its pool, `403` while suspended |
| `hack/panel-vm.sh ssl <domain> [--production]` | a site on a real domain with a certificate from Let's Encrypt staging, or production with the flag, served over TLS, printing the subject, issuer, expiry and fingerprint nginx served |
| `hack/panel-vm.sh db`, `mail`, `ftp`, `files`, `backup`, `config`, `services` | each module against the real daemon: MariaDB grants that leave a sibling closed, mail delivery and logins, the FTP chroot, the file manager as the user, a backup restored and refused after a changed byte, a refused and an accepted override, and the units started and stopped |
| `hack/bridge-vm.sh install <archive>`, then `run` | `panel` beside `dns` on bind, nginx and php-fpm: a site with its zone served by named under one account, a taken zone refused on the domain field, the disable refusal and the switch to `dns-peer` across a restart and back |
| `hack/peers-vm.sh install dns <archive> <address>` on one machine and `install panel <archive> <address>` on another, then `trust`, `code`, `pair`, `check` and `revoked` as each step prints | `panel` on one machine reaching `dns` on the other: pairing by code, a site whose zone named serves on the other machine in the account of the site's customer, a taken zone refused on the domain field, the one-time key opened in a browser with the link as actor in the trail, the zone's signed event received once though sent twice, `svc.dns.zones.open` twice with one key giving one service, and a revoked token and a deleted link answering `401` |

## Left to later milestones

- Pinning the certificate of a paired installation in its link. Until then an installation reaches
  a neighbour whose certificate the trust store of the system admits, so two installations with
  self-signed certificates each add the other's `/var/lib/keel/tls/cert.pem` to that store, as
  `hack/peers-vm.sh trust` does, and are reached under an address their certificates name, the
  address of `--listen` for a generated one.
- ACME for the listener's certificate, the certificate button of the settings and the problem of
  a certificate about to expire; until then the pair is the generated one or the operator's.
- The edition ceiling of account limits, the license check of `keel doctor`, which still reports
  that no lease is required, and `keel product features`, which lists every feature of every
  edition without reading the lease.
- Backup targets other than the local directory, for the installation backup and the site
  archives alike: the S3 target and the crate `keel-storage`.
- The findings of the M5 security review recorded without a fix: an FTP directory the hosting
  user turns into a link is followed at login with that user's rights; a mail domain for a name
  with no zone on this host needs no proof of control; a zone inside another account's zone; the
  template variables the agent takes from the kernel unchecked.
- The terminal and VNC of the panel, and a sweep of the transfer files a crash leaves behind.
- A bound on the owner of a placement: a finding of the M4 security review recorded without a
  fix. The other finding, the plain `http://` channel and the head that never expired, is closed
  by the https-only channel and the `expires_at` of the head.
- A stuck operation is bounded per process, not per operation; the global lock can be held for the
  sum of the limits of the processes of one operation.
- The allowlist of packages covers removal too: an admitted caller can remove any allowlisted
  package, dependencies included.
- The restore across a contract is run by hand after `rollback --restore`, as shown above.
- Deleting an account whose principals hold passwords, sessions or settings fails with
  `keel.account.error.rows_remain` until the system modules contribute their `on_delete`.
- Call counters and latency histograms; the support bundle carries `/readyz` in their place.

## Checks CI runs

`hack/check-release.sh` packages the built binary with `hack/release.py --dry-run` into a
directory, verifies the signed manifests (the heads with `keel-release verify --head`, which
refuses a manifest without an `expires_at`), the archive and its digest, checks that the heads
carry an `expires_at` 30 days ahead and the manifests of the versions none, that
`hack/release.py --refresh` signs a head again with a later `expires_at` and changes nothing else
byte for byte, that a head whose payload was changed is not refreshed, that a `beta` channel without
a head is skipped and a `stable` one without a head fails, that the refresh workflow builds
`keel-release` without secrets and shares its concurrency group with the release workflow, and feeds the dry run's channel to `keel privileged update --from-request` in a
temporary tree with a stub `systemctl`.
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
JSON file, which must be refused with no payload written; it reads a rehearsal channel through
`fetch_from_channel` with a stub in place of the download and requires a fresh head to reach the
unpacking and an expired head, a head without an `expires_at`, the manifest of a version served
in the place of the head and a `date` that prints nothing, no number or fails to stop before it,
while a `--version` of the channel still reaches the unpacking under an expired head and a broken
`date`; it requires `--channel-url` with an `http://` address to be refused without
`--insecure-rehearsal-channel` and the `config.yaml` it renders to carry the channel and the
setting for a rehearsal over `http://` and nothing for `https://`; and it runs the uid allotment against
a stub `getent` and a subordinate range file. `hack/install-vm.sh` is the operator's proof on a
fresh machine: it serves a rehearsal channel and the installer, checks that `curl | sh` stops
with the bash line, runs `curl | bash -s -- --product demo` against them and checks the units,
the processes running as root, the policy and the setup link.
