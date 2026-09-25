# The root boundary: keel-host

This document describes how a Keel installation touches the operating system: the rule that the
kernel process is never root, the single privileged code path of an installation and its two
policies, the `keel-host` and `keel-host-agent` crates, the typed operation catalogue, the
`agent.yaml` policy, auditing of operations, impersonation, configuration templates, the agent
protocol, and how the same model extends to a cluster of nodes. The dispatcher that calls into
this layer is described in [02-functions.md](02-functions.md); delivery, directories and systemd
units in [09-operations.md](09-operations.md); the decision record in
[11-decisions.md](11-decisions.md).

## Principle

The kernel process is never root. Every installation has exactly one privileged code path: the
`privileged` subcommand of the same binary, run under one root unit, with a set of policies that
decides what that subcommand is allowed to do:

- the `update` policy is always present — it installs a signed release;
- the `host` policy — the operation catalogue described below — is used only while the `host`
  module is loaded, which is the case when an enabled bundle names it.

On an installation where no enabled product manages the operating system (billing alone) the
kernel never connects to the agent, and the privileged unit serves the `update` policy alone.
That the kernel is clean is proved elsewhere: nothing in `keel-kernel` or `keel-system` needs the
agent to exist, and CI holds this on the cargo dependency graph — `keel-kernel` depends neither
on `keel-system` nor on `keel-host`, `keel-system` depends only on `keel-sdk`, and a composition
without `keel-host` is built in the matrix. The `update` system module reaches the root unit
through a request file, not through `keel-host` (see [01-overview.md](01-overview.md), layer
table).

The library knows nothing about nginx or bind as such: configuration templates are brought by
product modules. `keel-host` and `keel-host-agent` are L4 libraries that the `keel` binary links.

The security statement that follows from the split: a compromise of the kernel process gives the
attacker the operation catalogue, constrained by policy, not a shell.

## Processes and who talks to whom

On a host there are three kinds of process, plus local clients.

| Process | Identity | Listens on | Connects to |
|---------|----------|------------|-------------|
| `keel serve` | uid `keel`; the unit grants no capability except `CAP_NET_BIND_SERVICE` | the TLS port (HTTP and SSE) and `/run/keel/core.sock` with peer-credential checks | Postgres over its unix socket; the privileged process over `/run/keel/agent.sock` (only while `host` is loaded); plugin processes over their stdio |
| `keel privileged` | root | `/run/keel/agent.sock`; a connection is accepted only from uid `keel`, checked with `SO_PEERCRED` | nothing — it initiates no connection |
| a plugin | a separate uid per plugin | its stdio, JSON Lines | the `serve` process only |

The `serve` process is the kernel: the HTTP and SSE transports, the dispatcher, the Postgres
connection, the modules, tasks and the outbox. It holds the client side of the agent protocol as a
service: `HostExecutor`, registered by the `host` module and taken by a module handler through
`call.require::<dyn HostExecutor>()`.

The `privileged` process runs the operation catalogue under the `host` policy — `user`, `fs`,
`service`, `template`, `pkg`, `cron`, `quota`, `exec` — plus the `AgentOp` contributions of
product modules (libvirt for a VM product, ipmi for a data-centre product), and the `update`
policy, which knows one operation: install signed version X. Resources are addressed by
identifiers, not by paths; running something as another user is a re-exec of an internal
subcommand, never a credential change inside the process. Both are explained below.

A plugin has no access to `agent.sock`; whatever it does on the host goes through kernel functions
called under its own principal. Plugins are described in [05-modules.md](05-modules.md).

Local clients — `keel ctl` and `install.sh` — connect to `core.sock`. Peer credentials
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
`/var/lib/keel/`, and the path unit `keel-update.path` starts the privileged
subcommand under the `update` policy. That unit does one thing and does not read commands from the
request beyond the version to install ([09-operations.md](09-operations.md)).

## The privileged subcommand

### Units, sockets and files

| Item | Path or name |
|------|--------------|
| kernel unit | `keel.service` — `User=keel`, `NoNewPrivileges`, `ProtectSystem=strict`, `CAP_NET_BIND_SERVICE` |
| privileged unit | `keel-privileged.service` — root |
| update trigger | `keel-update.path` |
| agent socket | `/run/keel/agent.sock` |
| policy | `/etc/keel/agent.yaml` |
| template overrides | `/etc/keel/templates/` |
| update request | `/var/lib/keel/update.request` |
| admitted peer | the account named by `--user`, `keel` by default |

Products on one host share one agent, one socket and one policy file. What stays per product
inside the policy is the drop-in prefix `keel-<product>-` and the operating-system user prefix,
declared in a section per product; a request to the agent carries the `product` of the module
that made it, and the agent applies that product's section (see "Policy: agent.yaml").

### The update policy

Present on every installation, including one whose enabled products never touch the operating
system. The kernel writes the request; the root unit downloads `versions/<ver>`, verifies the
Ed25519 signature and the sha256 from the channel manifest, switches the `bin/current` symlink,
updates the unit files and restarts. `update rollback` works within a major version; a rollback
across a `contract` boundary is only possible with `--restore`. Anti-rollback is enforced by
`min_upgrade_from` in the channel manifest. The signing key stays offline; `previous_keys` are
compiled into the binary for key rotation. Before every update the kernel takes a logical dump of
the database over its own connection ([04-storage.md](04-storage.md)). The full sequence is in
[09-operations.md](09-operations.md).

### The host policy

`keel_host::MODULE`, module id `host`, is part of the composition of `keel`; it is loaded when an
enabled bundle names `host` in its `modules`, and product modules that need the executor declare
`requires: ["host"]`. While `host` is not loaded the kernel never connects to `agent.sock`,
`doctor` reports the agent as not in use, and the privileged unit — always installed and running
— serves the `update` policy only. Everything that follows in this document is the host policy.

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
  propagation of `request_id` and mapping of agent errors to i18n keys (`host.error.*`). A call
  goes through `host.on(Origin::new(product, principal, request_id))`: the caller names the
  product that owns it beside the principal and the request id it takes from the `Call` or the
  `AfterContext`, since a module crate belongs to one bundle and nothing in the SDK maps a module
  to the bundle that loaded it. `Local` opens one connection per request, which survives a
  restart of the privileged unit without reconnect logic, reads one answer line of at most
  16 MiB and checks its id. The client timeout is `timeout_seconds` of the section, 60 by
  default. An agent error surfaces in the call envelope as its key with the detail as the
  parameter `detail`, and the code follows the key: `host.error.policy` is `forbidden`, `args`,
  `render_failed` and `validate_failed` are `validation`, `not_found` is `not_found`,
  `unavailable` is `unavailable`, and any other key, a product operation's own included, is
  `internal`. The client adds `host.error.unreachable` (the socket is missing, refuses, or closes
  without an answer, which is how the agent turns away a peer it does not admit) and
  `host.error.timeout`, both `unavailable`.
- the `host` module, `keel_host::MODULE` — an ordinary `ModuleDef` with id `host`. It registers
  `dyn HostExecutor` as a service and reads its own section of `config.yaml` through
  `ConfigSection`: `socket` (absolute, default `/run/keel/agent.sock`) and `timeout_seconds`
  (1 to 3600, default 60). The start hook only records them; the socket is opened by the first
  operation, so a kernel with the module loaded connects only once a module calls. The set of `host.*`
  functions and streams it exposes for host-level screens (services, files, logs) is also not
  fixed by this revision. File manager and terminal screens are product views built on raw
  routes, as described in [02-functions.md](02-functions.md) and [08-frontend.md](08-frontend.md).
  Long operations run as tasks through the `tasks` module.

The socket the `Local` executor connects to is fixed by the directory layout:
`/run/keel/agent.sock` ([09-operations.md](09-operations.md)).

### keel-host-agent — the server side

`keel-host-agent` is what the `privileged` subcommand runs under the `host` policy: the socket
with the `SO_PEERCRED` check, the policy loader for `agent.yaml`, the built-in operation
catalogue, the `AgentOp` contributions of product modules, the journald audit and the
impersonation helper. The agent knows nothing about the database, the license or HTTP. It is
compiled into the same binary, so the catalogue the kernel calls and the catalogue the agent
executes are always from the same build.

### Calling the agent from a module

```rust
let host = call.require::<dyn HostExecutor>()?;
host.on(Origin::new("dns", call.principal(), call.request_id()))
    .template("bind/zone.j2").vars(&vars).dest("zones", &file).validate("named.check").reload("named").apply().await?;
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
| `host.user.*` | operating-system users | names under `user_prefix`; uid from the range allotted to the installation at install, named under the calling product's `user_prefix` |
| `host.group.*` | groups | as for users |
| `host.fs.*` | files and directories | path policy from `file_roots`; writes go through a temporary file and rename |
| `host.template.apply` | render a configuration file from a `ConfigTemplate` | render → swap → validate → reload → rollback, the check running with the file in place; see "Configuration templates" |
| `host.service.*` | reload, restart, list | only units named in `services`; `systemctl` is executed as a process, without D-Bus, so the binary stays a musl static build |
| `host.pkg.*` | packages: install, remove, get, refresh | `apt-get` with a fixed argument vector and `APT::Cmd::Pattern-Only=true`, each name taken as that package and never as a pattern or a trailing `+` or `-` action; `changes` lists every requested package whose installed version moved, read from `dpkg-query` before and after; no allowlist in M4 |
| `host.cron.*` | scheduled jobs on the host | drop-ins in `/etc/cron.d` under the product's `drop_in_prefix`, mode `0644`, written through a temporary file and a rename; a job runs an entry of `exec_allow`, never a command, as `keel` or as one of the product's own users, never as root; five fields or a nickname |
| `host.quota.*` | disk quotas | `setquota` and `repquota`; `host.error.unavailable` on a host without them |
| `host.exec` | run an external binary | only entries of `exec_allow`: a name runs without arguments, `name: [binary, arguments…]` with exactly that vector |

### Resource identifiers instead of paths

An operation accepts an identifier of a resource, and the agent validates that identifier by its
own rules. It does not accept an arbitrary path or an arbitrary argument vector, whether the
operation comes from the built-in catalogue or from a product `AgentOp`. The rules are:

- path prefixes: a product may create and change only its own files in include directories,
  `conf.d/keel-<product>-*`; the target of a file operation is a `file_roots` key plus a name, as in
  `"dest": { "root": "zones", "name": "keel-dns-a.org.zone" }`;
- unit name patterns: only the services listed in the policy;
- a uid range allotted to the installation at install time: users are created inside it and
  impersonation is possible only for uids inside it.

The M4 acceptance test for this rule is explicit: an operation outside `file_roots` is rejected by
the agent and appears in the audit ([../plan.md](../plan.md)).

### Product operations: AgentOp

The catalogue is extended by modules, not by editing the library. A product module contributes an
`AgentOp` — a name, the schema of its arguments and its result, a handler, and a policy class:

```rust
r.contribute(AgentOp::new::<VmCreateArgs, VmRef>("vm.libvirt.create", ops::libvirt_create).policy("vm"));
```

The operation is compiled into the same binary and executed by the agent. A VM product
adds libvirt operations, a data-centre product adds ipmi operations, and neither touches
`keel-host`. A module calls a product operation through the same executor:

```rust
host.op::<VmCreateArgs, VmRef>("vm.libvirt.create", &args).await?;
```

Product operations follow the same rules as built-in ones: typed arguments, resource identifiers
rather than free paths or argument vectors, a policy class the agent checks the call against, and
a journald record per call. The policy class (`vm` in the example) is the name under which the
policy constrains the operation: the product's section of `agent.yaml` grants it under
`classes`, a map from class to a free-form section the agent hands to the handler, as in
`demo: { drop_in_prefix: keel-demo-, classes: { note: { root: notes } } }`; the handler resolves
files through the same `file_roots` rules as `host.fs`, and the journal line carries `class=`.
`keel-host-agent` cannot depend on `keel-sdk`, whose database types would enter the agent's
graph, so the agent has operation and template types of its own and `keel privileged` adapts the
contributions of the frozen registry of every product the binary carries to them.

## Policy: agent.yaml

The policy lives at `/etc/keel/agent.yaml` and is read by the privileged process. It is one file
for the installation: the shared keys at the top, then a section per product for what stays per
product. The release ships a section for every first-party product, so `keel product enable`
never edits `/etc/keel`; the operator edits one file.

```yaml
policy:
  uid_range: { from: 100000, to: 165535 }
  file_roots: { zones: /etc/bind/zones, named: /etc/bind/keel, vhosts: /etc/nginx/conf.d, www: /var/www }
  services: [named, nginx, php8.3-fpm]
  checks: { nginx.test: ["nginx", "-t"], named.check: ["named-checkconf", "-z"] }
  exec_allow: [certbot, quotatool]
  products:
    bill:  { drop_in_prefix: keel-bill- }
    dns:   { drop_in_prefix: keel-dns- }
    panel: { user_prefix: hp_, drop_in_prefix: keel-panel- }
```

This is `deploy/agent.yaml` as the release ships it; the installer writes the range it allotted
over `uid_range`.

| Key | Meaning |
|-----|---------|
| `uid_range` | `{ from, to }`, the uids and gids the agent may create and impersonate; a range reaching 4294967295, the id that means "unchanged" to `setresuid` and `fchown`, is refused |
| `file_roots` | named directories the file and template operations may write to; `dest.root` in a request refers to a key of this map |
| `services` | units the service operations may reload, restart or list |
| `checks` | named validation commands with a fixed argument vector; `validate` in a request refers to a check by name |
| `exec_allow` | entries `host.exec` and cron jobs may run: a binary name, or `name: [binary, arguments…]`; a name appearing twice fails the policy |
| `products` | a section per product id: `user_prefix`, the prefix of the operating-system user names the agent creates and manages for that product; `drop_in_prefix`, the prefix of that product's own files in include directories; `classes`, the policy classes of its `AgentOp` contributions. The agent applies the section named by the `product` field of the request and refuses a request whose product has no section. |

The placement rules hold for every file the agent writes. The root of `file_roots` is
canonicalised; a symbolic link at the target is refused, and no path is followed or changed
after it is opened: a temporary is created with `O_EXCL` and `O_NOFOLLOW` beside the target, its
owner and mode applied to the open handle before the rename, and the previous content, mode and
owner are read from one no-follow handle. A mode is octal up to `1777` for a file and `3777` for
a directory; nothing carries the setuid bit, a file carries no setgid bit and never the
other-write bit, and a directory carries the other-write bit only with the sticky bit. A file the
agent reads for `host.fs.read`, `host.fs.write` or `host.template.apply` is read up to 8 MiB and
refused beyond. Operations run one at a time behind one lock, every tool runs with a cleared
environment and a fixed `PATH`, and account lookups go through `getent` as a process, so the
static build is not tied to the C library's name service. A service name starting with `-` never
reaches `systemctl`.

Everything outside the policy is refused. The agent does not hold a copy of the kernel's principal
or account model: the policy is about the host, and who may call a function is decided by the
kernel's guards before the request reaches the agent ([03-access.md](03-access.md)).

## Audit

Every operation — accepted or refused — is written to journald with the `actor` and the
`request_id` carried in the request. The kernel's `audit` module records the function call on its
side with the same request id ([05-modules.md](05-modules.md)), so the two trails join on
`request_id`: the audit table says who called `dns.zone.edit`, the journal says which files the
agent touched for it and whether the validation passed. The frame fields that reach every record
— the operation, the actor, the request id and the product — are capped at 1024 bytes each, and
so is the detail a record carries.

## Impersonation

The agent never changes its uid inside the tokio process. An operation "as uid X" is a re-exec of
the same binary through the internal subcommand `agent-exec --uid`, which calls `setresuid` in
`pre_exec`, before the target executes. `setfsuid` and `spawn_blocking` as means of running
something under another identity are forbidden by a rule that is enforced with a test.

The rationale is the runtime: an asynchronous multi-threaded process has no safe notion of "this
thread is now another user", and any per-thread credential trick leaks across the worker pool. A
fresh process is the only unit of identity that the kernel of the operating system guarantees.
Impersonation is allowed only for uids in the range allotted to the installation, and only for an
account whose primary group lies in that range too; a lookup that answers another uid than the
one asked is refused. The re-exec clears the environment, gives the program the fixed `PATH`,
`LC_ALL=C` and the account's `HOME`, `USER` and `LOGNAME`, and starts it in `/`, and it changes
credentials before the multi-threaded runtime is built.

## Configuration templates

A product owns only its own files in the include directories of the services it manages — the
files with its `drop_in_prefix` — and never edits a file that belongs to someone else. This is
what lets two products, or a product and a hand-configured service, share one nginx or one bind
on a host; on one host they do so through the same agent.

Templates are contributed by product modules, not by the library:

```rust
r.contribute(ConfigTemplate::new("bind/zone.j2", include_str!("../templates/zone.j2")).validate("named-checkzone"));
```

The operator may override a template by placing a file of the same name under
`/etc/keel/templates/` (the directory `keel privileged --templates` names), which the agent reads
before the compiled text; template ids are unique across the loaded set, which the freeze checks,
so two products' overrides never collide. The render is minijinja with undefined variables
strict, and the `validate` of a request wins over the template's named check. The kernel side sends the template id and its variables;
the agent renders the template (the template engine lives outside the kernel), writes the result
to a temporary file beside the target, swaps it into place keeping the previous content, mode and
owner, runs the named validation check, reloads the service, and rolls the file back — restores
what was there, or removes a file that did not exist — when the check or the reload fails,
reloading again after a failed reload. The check runs after the swap because a check such as
`nginx -t` validates the whole configuration in place and cannot be pointed at one drop-in, which
the M2 spike showed; a broken file is in place only for the duration of the check and the service
is reloaded only after the check passes, so a running service never reads it. Operations run one
at a time, so a swap, its check and its rollback are never interleaved with another request. An
apply whose rendered file equals what is there changes nothing and answers `changed: false`. The
response reports whether the file changed (`changed`).

The acceptance tests for this path are in [../plan.md](../plan.md): at M2 an invalid template
rolled back on nginx, proven by the host spike on the demo machine, which `keel-host-agent`
replaced and the close of M4 removed; at M4 a broken template rolls back without downtime, proven
in CI against a stub check and reload and on a machine by `hack/host-vm.sh` and `hack/dns-vm.sh`,
where `named` keeps its process and its serial and answers every query during the broken apply.

## The agent protocol

One request and one response per operation; the same frames go over the unix socket on a single
host and over mTLS gRPC to a remote node.

```
→ {"id":1,"op":"host.template.apply","actor":{"login":"alice","level":"user"},"request_id":"req-1a2b","product":"dns",
   "args":{"template":"bind/zone.j2","vars":{…},"dest":{"root":"zones","name":"keel-dns-a.org.zone"},"mode":"0640","owner":"bind",
           "validate":"named-checkzone","reload":"named"}}
← {"id":1,"ok":true,"data":{"changed":true}}
→ {"id":2,"op":"host.service.reload","args":{"name":"nginx","validate":"nginx.test"}}
← {"id":2,"ok":false,"error":{"key":"host.error.validate_failed","detail":"nginx: [emerg] …"}}
```

A request carries `id`, `op`, the `actor` (login and level of the principal on whose behalf the
kernel is calling), the `request_id` of the kernel call, the `product` that owns the calling
module (the bundle that names it, [05-modules.md](05-modules.md)), and `args` shaped by the
operation's schema. A response carries the same `id` and either `ok: true` with `data` or
`ok: false` with an `error` whose `key` is an i18n key and whose `detail` is the raw output of the
failed step. The key surfaces unchanged in the call envelope, so a validation failure on the host
reaches the form as a keyed error ([10-contracts.md](10-contracts.md)). A request is one line of
at most 1 MiB; a frame with a field it does not know, a duplicate field or a nesting deeper than
the parser's limit is refused with `host.error.frame`, and an argument of the catalogue with a
field the operation does not know with `host.error.args`. The answers
of the catalogue are `created`, `removed`, `changed`, `entries` and `services`, as each operation
fits.

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

The review of M4 went over the agent, the root half of the update and the release verifier as an
adversary would, and every finding it fixed carries a named test in the crate it concerns: the
re-exec's environment and group, the id that means "unchanged", a service named like an option,
world-writable placements, unbounded reads, the frame fields in the journal, the request file
read twice by path, a request moving to an older version, the unpacker counting the ustar size
where tar reads the PAX size, and group or other write bits in an archive. The frame parser and
the manifest verifier are fuzzed by bounded property tests (`proptest`) in the ordinary test run,
since the runner has no nightly toolchain for a coverage-guided fuzzer. Left without a fix, and
recorded in the M4 exit checklist of [../plan.md](../plan.md): a process the agent runs has no
timeout, so a hung check holds the lock for every later operation; `host.pkg.*` has no allowlist;
the owner of a placement may be any account of the host, since `bind` has to own zone files; the
channel admits `http://` and a head manifest carries no expiry, so a man in the middle can replay
an older signed head or withhold one, but not forge one; the unpacker has no bound on the number
of entries, a finding that needs the signing key; and the arguments of a product operation are the
product's types, which the SDK does not hold to `deny_unknown_fields`, the subject of the M7
review.

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
| M2 | a thin agent spike in a separate bin crate, `keel-spike`, removed at the close of M4: socket with `SO_PEERCRED`, `host.user.create`, `host.template.apply` with validate and rollback on nginx under a policy in the shape of `agent.yaml`, every operation written to the journal with actor and request id; done when, on a VM, the spike creates an operating-system user and a vhost through the agent from an unprivileged uid and an invalid template rolls back — shown by the spike's own script on `keel-demo`; the spike depended on no workspace crate and was replaced by `keel-host-agent` in M4 |
| M4 | `keel-host` in full — users, files, services, template, pkg, `AgentOp`, the `host` module; `privileged` with the `update` policy (check, apply, rollback, pre-update dump, anti-rollback); `dns` on bind; done when no process except `privileged` runs as root, a broken template rolls back without downtime, and an operation outside `file_roots` is refused and audited |
| M8 | `Remote` executor, node PKI and enrolment, `keel-system::cluster`, `Invoker` |

The order and the full done-when conditions are in [../plan.md](../plan.md).
