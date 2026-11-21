# Operating Keel

This guide takes someone who has never seen the project from a clean machine to a working KeelDNS
or KeelPanel and through the first weeks of running it. It is short on purpose: every command in
it exists, and each section names the place with the full detail, which is
[deploy/README.md](../../deploy/README.md), the manual of the installer, the processes and the
host agent.

## What you are installing

One installation per host: one binary `keel`, one Postgres database `keel`, one unprivileged
service under the unix user `keel` and one small root process, `keel privileged`, that does what
the service may not. The products are bundles of modules compiled into that binary and switched on
per host:

| Product | Id | What it does |
|---|---|---|
| KeelDNS | `dns` | zones and records served by bind, from the browser, the command line and an API token |
| KeelPanel | `panel` | sites on nginx and php-fpm with certificates, databases, mail, FTP, backups; it creates its sites' zones in KeelDNS on the same host or on a paired host |

Accounts and sign-in are shared by every product of a host. Peers, the pairing of two
installations, are a beta: pair only installations you run yourself.

## What you need

- A clean Debian 12 or newer, or Ubuntu 24.04 or newer, `x86_64`, booted with systemd, not a
  container. A host that already has `/opt/keel` or `/var/lib/keel` is refused.
- Root, and network access to `https://dl.globalart.hu/keel/` and to the distribution's package
  mirror.
- For KeelPanel, a host name that points at the machine if you want a Let's Encrypt certificate.

## Install KeelDNS

```
curl -fsSL https://dl.globalart.hu/keel/install.sh | bash -s -- --product dns --yes
```

The installer verifies the signature of the release manifest with a key embedded in the script,
installs Postgres from the distribution if it is missing, creates the user `keel`, the database
and the units, and starts the service. Every step announces itself with `==>`; a failure names
the step and exits with a non-zero status. At the end it prints three things: the https address of
the panel, the fingerprint of the generated certificate and a one-time setup link.

By default the service listens on `127.0.0.1:1500`. From another machine, forward the port:

```
ssh -L 1500:127.0.0.1:1500 root@<host>
```

then open the setup link with `localhost` as its host. The browser warns once about the
self-signed certificate: compare its fingerprint with the one the installer printed. The setup
link creates the first administrator and signs them in. To listen on another address, pass
`--listen <address>:<port>` to the installer.

Check that everything runs:

```
systemctl status keel keel-privileged keel-update.path
runuser -u keel -- keel doctor
```

`keel doctor` prints one line per check, `ok`, `warn` or `fail`, and exits with status 1 when a
check fails. Only `keel privileged` runs as root; `ps -eo user,comm | grep keel` shows it.

## Create the first zone

The installer does not touch bind. Install it, create the two directories Keel writes into and
the include line, and let `named-checkconf` pass once:

```
apt-get install -y bind9 bind9-utils bind9-dnsutils
install -d -m 0755 -o root -g bind /etc/bind/zones /etc/bind/keel
install -m 0644 -o root -g root /dev/null /etc/bind/keel/keel-dns-zones.conf
echo 'include "/etc/bind/keel/keel-dns-zones.conf";' >> /etc/bind/named.conf.local
named-checkconf -z && systemctl reload named
```

Then create a zone on the Zones screen of the panel, or from the command line:

```
runuser -u keel -- keel ctl dns.zone.create --as <login> name=example.test
runuser -u keel -- keel ctl dns.zone.list --as <login> --out table
dig @127.0.0.1 example.test SOA
```

The zone file is written after the transaction commits, checked with `named-checkconf`, and bind
is reloaded. A zone whose check fails turns to `error` with the tool's output, raises a problem
and leaves the previous file and the running bind as they were. Set `dns.nameserver` and
`dns.hostmaster` in `/etc/keel/config.yaml` to the real names of your name server; the defaults
are placeholders. The details are in "Running KeelDNS" of the manual.

## Install KeelPanel

On a clean host:

```
curl -fsSL https://dl.globalart.hu/keel/install.sh | bash -s -- --product panel --yes
```

The installer installs the distribution's packages of the panel, nginx, php-fpm, MariaDB,
Postfix, Dovecot and vsftpd, and prepares their directories and configuration; the full list and
what each module touches are in "Running KeelPanel" of the manual.

On a host that already runs KeelDNS, enabling the product switches the modules on and restarts
the service once, and one login then sees both products, but it installs none of the panel's
daemons: install the packages and prepare the directories and configuration as "Running
KeelPanel" describes first.

```
runuser -u keel -- keel product enable panel
```

A site, with its zone and a Let's Encrypt certificate, is created by the wizard of the Sites
screen.

## Run it

| Task | How |
|---|---|
| See the health of the host | the screen Health, or `runuser -u keel -- keel doctor` |
| Take a backup | `runuser -u keel -- keel backup`; add `--with-secret-key` to include the data key, sealed under a passphrase from `KEEL_BACKUP_PASSPHRASE` |
| Restore a backup | stop the service, `runuser -u keel -- keel restore <archive>`, start it; the archive is checked against the binary first |
| Update | the screen Updates, or `runuser -u keel -- keel update check` and `keel update apply --wait`; `keel update rollback` returns to the previous version |
| Change what is enabled | `runuser -u keel -- keel product list`, `keel product enable <id>`, `keel product disable <id>` |
| Install a license lease | `runuser -u keel -- keel license install <file>` |
| Collect what support needs | `runuser -u keel -- keel support-bundle`; secrets are redacted |
| Reset a password | `runuser -u keel -- keel ctl auth.password.set principal=<id> password=<new>` |

The commands marked `keel` run as the unix user `keel`: the control socket admits that user and no
other, root included. Backups stay in `/var/lib/keel/backups/` until you copy them off the host.
An update drains the task queue, takes a dump of the database, hands the new version to the root
unit and keeps the previous version for a rollback. It reads only a head of the release channel
that is signed and has not expired.

## When something is wrong

| Symptom | Look at |
|---|---|
| The panel does not open | `systemctl status keel`, `journalctl -u keel -f`, `curl -sS --cacert /var/lib/keel/tls/cert.pem https://127.0.0.1:1500/readyz` |
| A zone is in `error` | the problem `dns.zone.failed` and the tool's output on the zone; the previous zone file is still served |
| A host operation fails with a timeout | the limits in `/etc/keel/agent.yaml`, section `timeouts`; "The host agent" of the manual |
| A package operation is refused | the allowlist, section `packages` of `/etc/keel/agent.yaml` |
| The update says the head expired | the channel's head is re-signed weekly; try again later, or read `keel update check` |
| You need help | `keel support-bundle`, then send the archive |

## The commands of this guide

Every command name used above exists in the binary of the version this guide ships with; the
checks of the repository run these lines against it:

```sh check
keel --version
keel product --help
keel doctor --help
keel backup --help
keel restore --help
keel update --help
keel license --help
keel support-bundle --help
keel ctl --help
```

## What is not in this release

The plugin host and the cluster of nodes are not built into the release binary. Peers are a beta.
The FTP directory of a hosting user is confined by its name, not by a root-owned jail. The full
list of what is left to later milestones is at the end of the manual and in
[docs/plan.md](../plan.md).

## Where the detail is

- [deploy/README.md](../../deploy/README.md): the installer's flags, the processes and who runs
  them, the host agent and its policy, KeelDNS, KeelPanel, licenses, updates, backups, diagnostics,
  uninstalling.
- [docs/design/09-operations.md](../design/09-operations.md): the design of the operator plane and
  the release channel.
- [docs/design/06-host.md](../design/06-host.md): the boundary between the service and the root
  process.
