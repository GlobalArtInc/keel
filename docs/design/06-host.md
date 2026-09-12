# The root boundary: keel-host

This document describes how a Keel product touches the operating system: the rule that the kernel
process is never root, the single privileged code path of a product and its two policies, the
`keel-host` and `keel-host-agent` crates, the typed operation catalogue, the `agent.yaml` policy,
auditing of operations, impersonation, configuration templates, the agent protocol, and how the
same model extends to a cluster of nodes. The dispatcher that calls into this layer is described
in [02-functions.md](02-functions.md); delivery, directories and systemd units in
[09-operations.md](09-operations.md); the decision record in [11-decisions.md](11-decisions.md).

## Principle

The kernel process is never root. Every product has exactly one privileged code path: the
`privileged` subcommand of the same binary, run under a root unit, with a set of policies that
decides what that subcommand is allowed to do:

- the `update` policy exists in every product — it installs a signed release;
- the `host` policy exists only in products that link `keel-host` — it executes the operation
  catalogue described below.

A billing or IP-management product therefore ships a binary without a host agent. That is what
proves the kernel is clean: nothing in `keel-kernel` or `keel-system` needs the agent to exist.
CI holds this on the cargo dependency graph — `keel-kernel` depends neither on `keel-system` nor
on `keel-host`, and `keel-system` depends only on `keel-sdk`. The `update` system module reaches
the root unit through a request file, not through `keel-host` (see
[01-overview.md](01-overview.md), layer table).

The library knows nothing about nginx or bind as such: configuration templates are brought by the
product's modules. `keel-host` and `keel-host-agent` are L4 libraries that a product links when it
needs them.

The security statement that follows from the split: a compromise of the kernel process gives the
attacker the operation catalogue, constrained by policy, not a shell.

## Processes and who talks to whom

On a host running a product that manages the operating system there are three kinds of process,
plus local clients. The example below uses `dns`; every product has the same set under its
own names.

| Process | Identity | Listens on | Connects to |
|---------|----------|------------|-------------|
| `keel-dns serve` | uid `keel-dns`; the unit grants no capability except `CAP_NET_BIND_SERVICE` | the TLS port (HTTP and SSE) and `/run/keel/dns/core.sock` with peer-credential checks | Postgres over its unix socket; the privileged process over `/run/keel/dns/agent.sock`; plugin processes over their stdio |
| `keel-dns privileged` | root | `/run/keel/dns/agent.sock`; a connection is accepted only from uid `keel-dns`, checked with `SO_PEERCRED` | nothing — it initiates no connection |
| a plugin | a separate uid per plugin | its stdio, JSON Lines | the `serve` process only |

The `serve` process is the kernel: the HTTP and SSE transports, the dispatcher, the Postgres
connection, the modules, tasks and the outbox. It holds the client side of the agent protocol as a
service: `HostExecutor`, registered by the `host` module and taken by a module handler through
`call.require::<dyn HostExecutor>()`.

The `privileged` process runs the operation catalogue under the `host` policy — `user`, `fs`,
`service`, `template`, `pkg`, `cron`, `quota`, `exec` — plus the product's own `AgentOp`
contributions (libvirt for a VM product, ipmi for a data-centre product), and the `update` policy,
which knows one operation: install signed version X. Resources are addressed by identifiers, not
by paths; running something as another user is a re-exec of an internal subcommand, never a
credential change inside the process. Both are explained below.

A plugin has no access to `agent.sock`; whatever it does on the host goes through kernel functions
called under its own principal. Plugins are described in [05-modules.md](05-modules.md).

Local clients — `keel-dns ctl` and `install.sh` — connect to `core.sock`. Peer credentials
on that socket yield a principal of level `owner`, whose call context records the transport as
`via: Socket` ([03-access.md](03-access.md)). The ctl socket is part of the kernel's transports
([02-functions.md](02-functions.md)).

So the arrows are: plugin → `serve`; `ctl` and `install.sh` → `serve`; `serve` → `privileged`.
Nobody else connects to the privileged process, and the privileged process never initiates a
connection on a single host. In a cluster the direction of the transport is reversed — the
node's agent initiates an mTLS connection to the kernel — but the protocol is the same, and a
module does not know whether its server is local or remote (see "Cluster" below).

The `update` policy is driven differently from the `host` policy: the kernel does not call it over
`agent.sock`. It waits for tasks to drain, writes `update.request` under
`/var/lib/keel/dns/`, and the path unit `keel-dns-update.path` starts the privileged
subcommand under the `update` policy. That unit does one thing and does not read commands from the
request beyond the version to install ([09-operations.md](09-operations.md)).

## The privileged subcommand

### Units, sockets and files

| Item | Path or name |
|------|--------------|
| kernel unit | `keel-dns.service` — `User=keel-dns`, `NoNewPrivileges`, `ProtectSystem=strict`, `CAP_NET_BIND_SERVICE` |
| privileged unit | `keel-dns-privileged.service` — root |
| update trigger | `keel-dns-update.path` |
| agent socket | `/run/keel/dns/agent.sock` |
| policy | `/etc/keel/dns/agent.yaml` |
| template overrides | `/etc/keel/dns/templates/` |
| update request | `/var/lib/keel/dns/update.request` |

Two products on one host get different unix users, sockets, ports, cookies and drop-in prefixes.
Because the agent socket accepts only its own product's uid, one product's kernel cannot reach
another product's agent; each product has its own privileged process and its own policy file.

### The update policy

Present in every product, including those that never touch the operating system. The kernel
writes the request; the root unit downloads `versions/<ver>`, verifies the Ed25519 signature and
the sha256 from the channel manifest, switches the `bin/current` symlink, updates the unit files
and restarts. `update rollback` works within a major version; a rollback across a `contract`
boundary is only possible with `--restore`. Anti-rollback is enforced by `min_upgrade_from` in
the channel manifest. The signing key stays offline; `previous_keys` are compiled into the binary
for key rotation. Before every update the kernel takes a logical dump of the database over its
own connection ([04-storage.md](04-storage.md)). The full sequence is in
[09-operations.md](09-operations.md).

### The host policy

Present only in products that link `keel-host`. A product enables it by adding `keel-host` to
its workspace and putting `.module(&keel_host::MODULE)` into its product builder; product modules
that need the executor declare `requires: ["host"]`. Without it the binary has no `agent.sock`
and no host policy, so there is no agent for `doctor` to check. Everything that follows in this
document is the host policy.

## Crates

### keel-host — the client side

`keel-host` is linked into the kernel process. It contains:

- the `HostExecutor` trait in `keel_host::api`. It is deliberately not part of `keel-sdk`: the
  SDK knows nothing about hosts, and the cargo invariant "kernel does not depend on host" holds
  without exceptions. A module obtains the executor with `call.require::<dyn HostExecutor>()`;
  the registry freeze at start-up fails if a module requires the `host` service and no module
  provides it ([05-modules.md](05-modules.md)).
- two implementations of the trait: `Local`, which speaks over the unix socket `agent.sock`, and
  later `Remote`, which speaks the same protocol over mTLS gRPC to a node. The module code is
  identical for both.
- the typed client API: `users()`, `fs()`, `services()`, `pkg()`, `cron()`, `quota()`,
  `template()`, `exec()` (allowlisted), and `op::<A, R>(name)` for product-defined operations;
  propagation of `request_id` and mapping of agent errors to i18n keys (`host.error.*`). Client
  timeouts are expected but not fixed by this revision.
- the `host` module, `keel_host::MODULE` — an ordinary `ModuleDef` with id `host`. It registers
  `dyn HostExecutor` as a service and reads its own section of `config.yaml` through
  `ConfigSection`; the keys of that section are not fixed by this revision. The set of `host.*`
  functions and streams it exposes for host-level screens (services, files, logs) is also not
  fixed by this revision. File manager and terminal screens are product views built on raw
  routes, as described in [02-functions.md](02-functions.md) and [08-frontend.md](08-frontend.md).
  Long operations run as tasks through the `tasks` module.

The socket the `Local` executor connects to is fixed by the directory layout:
`/run/keel/<product>/agent.sock` ([09-operations.md](09-operations.md)).

### keel-host-agent — the server side

`keel-host-agent` is what the `privileged` subcommand runs under the `host` policy: the socket
with the `SO_PEERCRED` check, the policy loader for `agent.yaml`, the built-in operation
catalogue, the product's `AgentOp` contributions, the journald audit and the impersonation
helper. The agent knows nothing about the database, the license or HTTP. It is compiled into the
same product binary, so the catalogue the kernel calls and the catalogue the agent executes are
always from the same build.

### Calling the agent from a module

```rust
let host = call.require::<dyn HostExecutor>()?;
host.template("bind/zone.j2").vars(&vars).dest("zones", name).validate("named-checkzone").reload("named").apply().await?;
```

The dispatcher rule from [02-functions.md](02-functions.md) applies without exception: operations
on the operating system run only in `after_commit` or as a task. The write transaction never
waits for the agent. The `dns.zone.edit` handler in that document shows the pattern — the row is
saved inside the transaction, and `call.after_commit(...)` hands it to the provider that talks to
the host.

## The operation catalogue

The catalogue is typed: every operation has a name, a schema for its arguments and its result,
and a policy that constrains it. There is no "run this command" operation with free arguments.

| Family | What it does | Constraint |
|--------|--------------|------------|
| `host.user.*` | operating-system users | names under `user_prefix`; uid from the range allotted to the product at install |
| `host.group.*` | groups | as for users |
| `host.fs.*` | files and directories | path policy from `file_roots`; writes go through a temporary file and rename |
| `host.template.apply` | render a configuration file from a `ConfigTemplate` | render → validate → swap → reload → rollback; see "Configuration templates" |
| `host.service.*` | reload, restart, list | only units named in `services`; `systemctl` is executed as a process, without D-Bus, so the binary stays a musl static build |
| `host.pkg.*` | packages | the distribution's package manager |
| `host.cron.*` | scheduled jobs on the host | |
| `host.quota.*` | disk quotas | |
| `host.exec` | run an external binary | only binaries in `exec_allow`, with fixed arguments |

### Resource identifiers instead of paths

An operation accepts an identifier of a resource, and the agent validates that identifier by its
own rules. It does not accept an arbitrary path or an arbitrary argument vector, whether the
operation comes from the built-in catalogue or from a product `AgentOp`. The rules are:

- path prefixes: a product may create and change only its own files in include directories,
  `conf.d/keel-<product>-*`; the target of a file operation is a `file_roots` key plus a name, as in
  `"dest": { "root": "zones", "name": "keel-dns-a.org.zone" }`;
- unit name patterns: only the services listed in the policy;
- a uid range allotted to the product at install time: users are created inside it and
  impersonation is possible only for uids inside it.

The M4 acceptance test for this rule is explicit: an operation outside `file_roots` is rejected by
the agent and appears in the audit ([../plan.md](../plan.md)).

### Product operations: AgentOp

The catalogue is extended by modules, not by editing the library. A product module contributes an
`AgentOp` — a name, the schema of its arguments and its result, a handler, and a policy class:

```rust
r.contribute(AgentOp::new::<VmCreateArgs, VmRef>("vm.libvirt.create", ops::libvirt_create).policy("vm"));
```

The operation is compiled into the same product binary and executed by the agent. A VM product
adds libvirt operations, a data-centre product adds ipmi operations, and neither touches
`keel-host`. A module calls a product operation through the same executor:

```rust
host.op::<VmCreateArgs, VmRef>("vm.libvirt.create", &args).await?;
```

Product operations follow the same rules as built-in ones: typed arguments, resource identifiers
rather than free paths or argument vectors, a policy class the agent checks the call against, and
a journald record per call. The policy class (`vm` in the example) is the name under which the
policy constrains the operation; the shape of a product-specific section in `agent.yaml` is not
fixed by this revision.

## Policy: agent.yaml

The policy lives at `/etc/keel/dns/agent.yaml` and is read by the privileged process. The
example below is a hosting product's policy:

```yaml
policy:
  file_roots: { zones: /etc/bind/zones, vhosts: /etc/nginx/conf.d, www: /var/www }
  services: [nginx, php8.3-fpm, named]
  checks: { nginx.test: ["nginx", "-t"] }
  exec_allow: [certbot, quotatool]
  user_prefix: hp_
  drop_in_prefix: keel-panel-
```

| Key | Meaning |
|-----|---------|
| `file_roots` | named directories the file and template operations may write to; `dest.root` in a request refers to a key of this map |
| `services` | units the service operations may reload, restart or list |
| `checks` | named validation commands with a fixed argument vector; `validate` in a request refers to a check by name |
| `exec_allow` | binaries `host.exec` may run |
| `user_prefix` | prefix of the operating-system user names the agent creates and manages |
| `drop_in_prefix` | prefix of the product's own files in include directories |

Everything outside the policy is refused. The agent does not hold a copy of the kernel's principal
or account model: the policy is about the host, and who may call a function is decided by the
kernel's guards before the request reaches the agent ([03-access.md](03-access.md)).

## Audit

Every operation — accepted or refused — is written to journald with the `actor` and the
`request_id` carried in the request. The kernel's `audit` module records the function call on its
side with the same request id ([05-modules.md](05-modules.md)), so the two trails join on
`request_id`: the audit table says who called `dns.zone.edit`, the journal says which files the
agent touched for it and whether the validation passed.

## Impersonation

The agent never changes its uid inside the tokio process. An operation "as uid X" is a re-exec of
the same binary through the internal subcommand `agent-exec --uid`, which calls `setresuid` in
`pre_exec`, before the target executes. `setfsuid` and `spawn_blocking` as means of running
something under another identity are forbidden by a rule that is enforced with a test.

The rationale is the runtime: an asynchronous multi-threaded process has no safe notion of "this
thread is now another user", and any per-thread credential trick leaks across the worker pool. A
fresh process is the only unit of identity that the kernel of the operating system guarantees.
Impersonation is allowed only for uids in the range allotted to the product.

## Configuration templates

A product owns only its own files in the include directories of the services it manages — the
files with its `drop_in_prefix` — and never edits a file that belongs to someone else. This is
what lets two products, or a product and a hand-configured service, share one nginx or one bind
on a host.

Templates are contributed by product modules, not by the library:

```rust
r.contribute(ConfigTemplate::new("bind/zone.j2", include_str!("../templates/zone.j2")).validate("named-checkzone"));
```

The operator may override a template by placing a file of the same name under
`/etc/keel/dns/templates/`. The kernel side sends the template id and its variables; the
agent renders the template (the template engine lives outside the kernel), writes the result to a
temporary file, runs the named validation check, swaps the file into place, reloads the service,
and rolls the file back if validation or reload fails. The response reports whether the file
changed (`changed`).

The acceptance tests for this path are in [../plan.md](../plan.md): at M2 an invalid template
rolls back on nginx; at M4 a broken template rolls back without downtime.

## The agent protocol

One request and one response per operation; the same frames go over the unix socket on a single
host and over mTLS gRPC to a remote node.

```
→ {"id":1,"op":"host.template.apply","actor":{"login":"alice","level":"user"},"request_id":"req-1a2b",
   "args":{"template":"bind/zone.j2","vars":{…},"dest":{"root":"zones","name":"keel-dns-a.org.zone"},"mode":"0640","owner":"bind",
           "validate":"named-checkzone","reload":"named"}}
← {"id":1,"ok":true,"data":{"changed":true}}
→ {"id":2,"op":"host.service.reload","args":{"name":"nginx","validate":"nginx.test"}}
← {"id":2,"ok":false,"error":{"key":"host.error.validate_failed","detail":"nginx: [emerg] …"}}
```

A request carries `id`, `op`, the `actor` (login and level of the principal on whose behalf the
kernel is calling), the `request_id` of the kernel call, and `args` shaped by the operation's
schema. A response carries the same `id` and either `ok: true` with `data` or `ok: false` with an
`error` whose `key` is an i18n key and whose `detail` is the raw output of the failed step. The
key surfaces unchanged in the call envelope, so a validation failure on the host reaches the form
as a keyed error ([10-contracts.md](10-contracts.md)).

Streaming, progress and cancellation of long agent operations are not fixed by this revision; on
the kernel side a long host operation is a task.

## Cluster

The cluster milestone (M8) does not change the model. `Remote`, the second implementation of
`HostExecutor`, speaks the same protocol over mTLS gRPC (tonic, protox — no `protoc` at build
time) with a PKI issued by the kernel (rcgen). A node is enrolled with `agent enroll` and a
one-time token; the node's agent initiates the connection, so nodes need no open ports. A used
enrolment token is refused on reuse.

`keel-system::cluster` adds the node registry and an `Invoker`, which chooses the node by an
attribute of the function; the extension point `r.invoker` is reserved in the kernel from M0 so
that this arrives without a kernel change. The module also brings node drain and status, automatic
update of node agents when the kernel is updated, and the problem "node unreachable"; functions
bound to an unreachable node answer 503. The acceptance test: `dns` manages a secondary
server on a remote node with the same module code ([../plan.md](../plan.md)).

## Threat model and review notes

The agent under root is the main attack surface of a host-managing product: the path policy,
symlink and time-of-check races, the exec allowlist, and product `AgentOp` handlers. The measures
are fuzzing of operation schemas, a security review at M4 and again at M7, and the journald
record of every operation.

Self-update under root has its own risk: a compromise of the release signing key is a compromise
of every installation. The measures are the offline key, `previous_keys` in the binary, and a
root unit that knows one operation and does not read commands from the request.

The alternative considered for this boundary was an in-process root executor, as in the first
revision of the design. The review kept the split and added three refinements that are now part
of it: one privileged code path (`privileged`) with the `update` and `host` policies instead of two
root programs; resources addressed by identifiers, never by paths; and impersonation by re-exec
rather than by changing credentials in place. The full record is in
[11-decisions.md](11-decisions.md).

## Milestones

| Milestone | Scope for this layer |
|-----------|----------------------|
| M2 | a thin agent spike in a separate `products/spike`: socket with `SO_PEERCRED`, `host.user.create`, `host.template.apply` with validate and rollback on nginx; done when, on a VM, the spike creates an operating-system user and a vhost through the agent from an unprivileged uid and an invalid template rolls back |
| M4 | `keel-host` in full — users, files, services, template, pkg, `AgentOp`, the `host` module; `privileged` with the `update` policy (check, apply, rollback, pre-update dump, anti-rollback); `dns` on bind; done when no process except `privileged` runs as root, a broken template rolls back without downtime, and an operation outside `file_roots` is refused and audited |
| M8 | `Remote` executor, node PKI and enrolment, `keel-system::cluster`, `Invoker` |

The order and the full done-when conditions are in [../plan.md](../plan.md).
