# Wire contracts

This document fixes the shapes that cross a process boundary in Keel: where the contract is defined
and how it is verified, the function descriptor as the frontend receives it, the metadata version
and its caching rule, the call envelope with its error codes and the CSRF rule, the list contract
(`ListQuery` → `Page`) and its source trait, the request and response shapes behind form dynamics,
the compile-time shape of a module and a product, the plugin manifest and its JSON Lines frames,
the host agent protocol with `agent.yaml`, and the `GET /api/v1/boot` response. The semantics
behind each shape live in the sibling documents: the dispatcher and descriptors in
[02-functions.md](02-functions.md), levels and accounts in [03-access.md](03-access.md), modules
and plugins in [05-modules.md](05-modules.md), the host agent in [06-host.md](06-host.md), peers
and services in [07-ecosystem.md](07-ecosystem.md), and the frontend that consumes all of this in
[08-frontend.md](08-frontend.md).

## Where the contract lives

The wire contract is defined in `keel-types`, a crate without I/O. It carries `PROTOCOL = 1`, the
descriptor types, the envelope, `ListQuery` and `Page`, `Principal`, `Account` and `Level`, the
condition AST and the `check` presets. Golden fixtures are checked by `cargo test` on the Rust side
and by Jest on the TypeScript side, so the two implementations cannot drift without a red build.
From the JSON Schemas of `keel-types` the build generates the TypeScript types of
`@globalart/keel/meta` and the OpenAPI description used by clients in other languages
(`keel-dev types`; see [05-modules.md](05-modules.md)).

Three fixture sets are shared between Rust and TypeScript from M0:

- descriptor snapshots per level (`keel-dev descriptors dump | check | diff`);
- condition cases in `contracts/fixtures/expr-cases.json` for the ~200-line interpreters on each
  side;
- `check` preset cases (`domain`, `ip`, `cidr`, `email`, `int`, `name`, `port`, `path`, ...).

`keel-conformance` runs the same contract against a started binary of any product — boot, auth,
the list contract, errors, idempotency, webhooks, `svc.*` — and is a mandatory CI job of every
product. The three versioning planes (compile-time `keel-sdk`, wire `PROTOCOL`, data `"keel": 1`)
and the N / N-1 rule for `PROTOCOL` are stated in [01-overview.md](01-overview.md); `PROTOCOL` is
reported in `GET /api/v1/instance` and in the plugin manifest.

### Endpoint map

The routes this design names. Anything else a product serves is a raw route under
`/api/v1/x/<module>/…`, registered by a module with a mandatory guard and an audit event
([02-functions.md](02-functions.md)).

| Route | Purpose |
|-------|---------|
| `GET /healthz`, `GET /readyz` | liveness; readiness checks the database, migrations and that the process is not draining ([09-operations.md](09-operations.md)) |
| `GET /api/v1/boot` | the first request of the frontend: product, brand, principal, menu, features, dashboard, metadata version |
| `GET /api/v1/functions`, `GET /api/v1/functions/{func}` | descriptors, projected under the calling principal |
| `POST /api/v1/call/{func}` | the call; `GET` is accepted for `kind: list \| get` |
| `/api/v1/i18n` | message catalogues per language |
| `GET /api/v1/instance` | instance identity for peers: `protocol`, the instance public key, service kinds ([07-ecosystem.md](07-ecosystem.md)) |
| `/api/v1/x/<module>/…` | raw routes of modules: upload, download, WebSocket terminal, VNC, the peers pull feed `/api/v1/x/peers/feed?after=seq` |
| `/login?key=…` | the page that redeems a one-time login key same-origin ([07-ecosystem.md](07-ecosystem.md)) |

The server-sent event stream carries `task.progress`, `notification`, `data.invalidate` and
`meta.invalidate`; `Last-Event-ID` is the outbox `seq`, so a reconnecting client catches up from
where it stopped. Events are filtered by `ScopePolicy` and principal: a user receives
`data.invalidate` only for its own objects. The peers feed is a separate path, not the browser
stream.

### Conventions shared by every shape

- Identifiers are ULIDs serialised as text: stable for peers, import and export, sortable by time.
- Time is RFC 3339 in UTC. The principal's time zone comes from `settings_pref` and is reported in
  `/boot`; date filters compute day boundaries in that zone, and the client converts to `Date` by
  it.
- Every human-readable string is an i18n key, never a sentence: `title`, `label`, `error.key`,
  `fields[].key`. Placeholders have one format, `{name}`, in minijinja, the CLI and TypeScript.
- Function names are `<module>.<entity>.<verb>`; the descriptor carries `"keel": 1`, the data
  version of the descriptor format.
- The vocabulary is closed by enums in `keel-types` and generated as TypeScript unions: `kind` ∈
  `list · get · form · action · wizard · report · options · stream · download · upload`;
  `control`; `column.type` (with an explicit mapping onto `PhxColumn.type`); `action.role` ∈
  `new · edit · delete · group · button · child · external · export · upload · download ·
  refresh`; the `check` presets. Icons are validated against `PhxIconName` at start.

## Function descriptor: `GET /api/v1/functions/dns.zone.list`

The descriptor is returned already projected under the calling principal: columns and actions
whose `access` is above the caller's level or outside the caller's permissions are cut out, not
delivered and hidden. The three parts of a descriptor — the parameter schema, the `x-keel` /
`x-ui` field attributes inside it, and the separate `ui` block — are explained in
[02-functions.md](02-functions.md). Conditions (`hidden_if`, `show_if`, `required_if`,
`readonly_if`) are delivered as the compiled JSON AST, never as the source string.

```json
{ "keel": 1, "name": "dns.zone.list", "module": "dns", "kind": "list", "mode": "read",
  "access": { "level": "user", "permission": "dns.zone.read", "feature": "dns.zones" }, "key": "id",
  "params": { "$ref": "keel://ListQuery" }, "result": { "$ref": "#/$defs/Page_ZoneRow" },
  "ui": {
    "title": "dns.zone.list.title", "search": ["name"],
    "columns": [
      { "key": "name", "type": "link", "to": { "screen": "dns.zone.edit", "params": { "id": "$id" } }, "sortable": true, "primary": true },
      { "key": "records", "type": "number", "sortable": true, "total": "sum" },
      { "key": "status", "type": "status", "map": { "active": "success", "suspended": "warning" }, "filter": { "options": ["active", "suspended"] } },
      { "key": "dnssec", "type": "toggle", "edit": { "function": "dns.zone.update" }, "access": { "permission": "dns.zone.manage" } },
      { "key": "account", "type": "text", "access": { "level": "reseller" }, "filter": { "remote": "keel.account.options" } } ],
    "actions": [
      { "id": "create", "role": "new", "screen": "dns.zone.edit", "icon": "plus" },
      { "id": "edit", "role": "edit", "screen": "dns.zone.edit", "default": true },
      { "id": "records", "role": "child", "screen": "dns.record.list", "params": { "zone_id": "$id" } },
      { "id": "suspend", "role": "group", "function": "dns.zone.suspend", "confirm": true, "hidden_if": { "eq": ["status", "suspended"] } },
      { "id": "delete", "role": "delete", "function": "dns.zone.delete", "confirm": { "destructive": true, "items": "name", "input": "count" } },
      { "id": "export", "role": "export", "formats": ["csv", "json"] } ],
    "row_actions": ["edit", "records", "suspend", "delete"],
    "empty": { "title": "dns.zone.empty", "action": "create" },
    "invalidate_on": ["dns.zone.*"] } }
```

| Field | Meaning |
|-------|---------|
| `keel` | descriptor data version |
| `name`, `module` | function name and owning module; the module prefix is enforced at registration |
| `kind`, `mode` | screen kind and `read \| write`; `mode` selects the dispatcher path (no transaction, no idempotency, no outbox for `read`) |
| `access` | minimum `level`, required `permission`, licence `feature`; evaluated by the guards |
| `key` | the row key column; its presence makes the function owned, so `OwnedGuard` loads the row's account before the handler ([02-functions.md](02-functions.md)) |
| `params`, `result` | JSON Schema; `keel://ListQuery` is the shared list query, `Page_ZoneRow` is generated from the Rust row type |
| `ui.title`, `ui.search` | title key and the columns full-text search runs over |
| `ui.columns[]` | `key`, `type` (mapped onto `PhxColumn.type`), `sortable`, `primary`, `total` (footer aggregate), `map` (value → status tone), `filter` with either static `options` or a `remote` options function, `to` for `link` columns (target `screen` with `params`), `edit` for in-cell editing through another function, and `access` that projects the column away for callers below it |
| `ui.actions[]` | `id`, `role`, and either a `screen` to open or a `function` to call; `icon`; `default` marks the action a row opens; `params` for child screens; `confirm` as `true` or `{ destructive, items, input }` (show the affected items by the named column and ask the user to type their count); `hidden_if` as a condition AST; `formats` for `export` |
| `ui.row_actions` | which actions appear on a row; the rest are toolbar and group actions |
| `ui.empty` | empty-state title and the action it offers |
| `ui.invalidate_on` | event masks; a matching `data.invalidate` on the event stream refreshes the list |

Values written as `$id` in `to.params` and `action.params` refer to the row's key column of the
current row. `filter.remote` names a `kind: options` function (see form dynamics below).

## Metadata version and caching

Descriptor responses depend on who asks, so they are cached per principal and invalidated when
the principal's rights change:

- `ETag = hash(build, modules, edition, lang, principal_id, acl_gen)`;
- `acl_gen` is a column of `keel_principal`, incremented in the same transaction that changes the
  principal's permissions, role or account; for a reseller's subtree the increment is applied by
  `path`;
- responses are `Cache-Control: private, no-store`;
- the event stream's `meta.invalidate` tells the client to re-request.

The same value is exposed as `X-Keel-Meta-Version` on call responses and as `meta` in `/boot`;
the shell keys its metadata cache by it ([08-frontend.md](08-frontend.md)). The principal is part
of the hash on purpose: a metadata ETag that ignored the principal while the body depended on the
caller's rights would violate cache semantics — after `auth.su` in the same browser the client
would show the delegated user another principal's menu. See [11-decisions.md](11-decisions.md).

## Call and envelope: `POST /api/v1/call/{func}`

One shape for every transport: HTTP, the local socket, plugins and peers. `GET` is accepted for
`kind: list | get`.

```http
POST /api/v1/call/dns.zone.edit
Authorization: Bearer kat_01J9…          (or Cookie: __Host-keel_dns_session=ks_…)
Idempotency-Key: bill-order-9912-open
{ "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3", "ttl": 300 }

200  X-Keel-Request-Id: req-1a2b   X-Keel-Meta-Version: m-7f3a2c
{ "ok": true, "data": { "id": "01J9Q3…" },
  "meta": { "requestId": "req-1a2b",
            "principal": { "id": "01J9…", "login": "alice", "level": "user", "account": "01J8A…",
                           "actor": { "id": "01J8…", "login": "root", "level": "admin", "via": "su" } } } }

422
{ "ok": false, "error": { "code": "validation", "key": "keel.error.validation", "requestId": "req-1a2c",
  "fields": [ { "path": "master_ips[1]", "code": "check.ip", "key": "keel.check.ip", "params": { "value": "10.0.0" } },
              { "path": "name", "code": "dns.zone.error.exists", "key": "dns.zone.error.exists", "params": { "name": "a.org" } } ] } }

403  { "ok": false, "error": { "code": "forbidden_field", "key": "keel.error.forbidden_field", "fields": [ { "path": "account" } ] } }

CLI:  keel-dns ctl dns.zone.edit id=01J9Q3… ttl=300 --as alice --out json     (same envelope; --out table renders by ui.columns)
```

### Success envelope

| Field | Meaning |
|-------|---------|
| `ok: true` | the call committed (or, for `mode: read`, completed) |
| `data` | the function's result, shaped by `result` in the descriptor |
| `meta.requestId` | the same value as `X-Keel-Request-Id`; appears in logs and in agent operations |
| `meta.principal` | the effective principal: `id`, `login`, `level`, `account` (the account is always explicit, see [03-access.md](03-access.md)) |
| `meta.principal.actor` | who is acting on the principal's behalf, or `null`: after `auth.su` the actor is the delegating principal with `via: "su"`; on an on-behalf call from a peer the actor is the peer ([07-ecosystem.md](07-ecosystem.md)) |

### Error envelope

| Field | Meaning |
|-------|---------|
| `ok: false` | nothing was committed |
| `error.code` | machine-readable code from the table below |
| `error.key`, `error.params` | i18n key and its placeholders; the client renders, the server never formats sentences |
| `error.requestId` | correlation id |
| `error.fields[]` | field-addressed errors: `path` is a JSON path into the parameters (`master_ips[1]`), plus `code`, `key`, `params` for each; a `check` preset fails with `code: check.<preset>` and `key: keel.check.<preset>`; a handler error attached with `.field("name")` lands here with its own key |

| Code | HTTP | Raised when |
|------|------|-------------|
| `validation` | 422 | schema validation, `check` presets, `required_if`, or a handler error attached to a field |
| `forbidden` | 403 | level, permission, token scope or a module guard rejects the call |
| `forbidden_field` | 403 | a write carries a field whose `x-keel.level` is above the caller's level; `fields[].path` names it — the field is never silently dropped |
| `unauthenticated` | 401 | no valid session, token or cookie |
| `not_found` | 404 | the row does not exist, or belongs to an account outside the caller's scope (`OwnedGuard` does not reveal existence) |
| `conflict` | 409 | a uniqueness or state conflict raised by the handler |
| `idempotency_mismatch` | 409 | the same `Idempotency-Key` with a different body |
| `limit_exceeded` | 409 | `call.check_limit(key, current)` found the account's limit reached ([03-access.md](03-access.md)) |
| `license` | 402 | the required `feature` is not licensed, or the licence is in read-only degradation and the function mutates |
| `rate_limited` | 429 | a rate limit was hit (the `auth` module's login limits or request limits) |
| `unavailable` | 503 | the call cannot be executed now — for example, the cluster node that owns the function is unreachable |
| `peer` | 502 | a call to a neighbour product failed |
| `internal` | 500 | anything else; details go to the log under `requestId`, not into the envelope |

Where each code originates in the pipeline is tabulated in [02-functions.md](02-functions.md).

### Authentication and CSRF

- Browser sessions use the cookie `__Host-keel_<product>_session` (`__Host-keel_dns_session`)
  carrying an opaque `ks_` token; API tokens are `kat_` Bearer tokens, peer tokens `kpt_`, one-time
  login keys `kok_` ([03-access.md](03-access.md)).
- Cookie-authenticated requests are checked by `Sec-Fetch-Site` / `Origin`; Bearer requests are
  not. This is why the one-time login key is redeemed by a same-origin page at `/login?key=…` that
  posts `auth.key.redeem`, instead of the key being accepted on a cross-site navigation
  ([07-ecosystem.md](07-ecosystem.md)).
- The `__Host-` prefix requires TLS, so the panel listens only on TLS from the first start
  ([09-operations.md](09-operations.md)).

### Idempotency

`Idempotency-Key` is handled by the kernel, not by handlers. A repeat with the same key and the
same body is a replay of the stored response marked with `X-Keel-Idempotent-Replay`; the same key
with a different body is `409 idempotency_mismatch`. Keys are stored in `keel_idempotency`
([04-storage.md](04-storage.md)); `keel-client` retries only calls that carry a key.

### CLI

`keel-dns ctl <func> …` goes through the local socket with peer-credential authentication and
receives the same envelope; `--out json` prints it, `--out table` renders `data` by `ui.columns`.
Whether parameters are accepted as `a.b=v` with repeated keys and `@file.json`, or only as JSON,
is an open question ([11-decisions.md](11-decisions.md)).

## Lists: `ListQuery` → `Page`

One request type for every `kind: list`, identical to the contract of `phx-data-table`. `scope`
carries the parent keys of a child list; `op` on a filter exists for API clients.

```http
POST /api/v1/call/dns.record.list
{ "page": 2, "pageSize": 25, "search": "www", "sort": { "key": "name", "dir": "asc" },
  "filters": [ { "key": "type", "value": ["A", "AAAA"] }, { "key": "ttl", "op": "gte", "value": "300" } ],
  "scope": { "zone_id": "01J9Q3…" } }
200 { "ok": true, "data": { "data": [ { "id": "01J9R…", "name": "www", "type": "A", "ttl": 300 } ], "total": 7, "totals": null } }
```

```rust
pub struct ListQuery { page: u32, page_size: u32, search: String, sort: Option<Sort>, filters: Vec<Filter>, scope: Map<String, Value> }
pub struct Page<T>   { data: Vec<T>, total: u64, totals: Option<Map<String, Value>> }
```

A `Filter` is `{ key, op, value }` where `op` defaults to `eq` and `value` is a string or a list
of strings, as in the `type` filter above; the match semantics of a list value are not fixed by
the design. `totals`
holds the footer aggregates declared by `column.total` (`"sum"` in the descriptor above) or
`null` when the function declares none. `page_size` is capped by the function's
`max_page_size`, and a missing `sort` falls back to `default_sort`
([02-functions.md](02-functions.md)).

### `ListSource`

The source of a list is the trait `ListSource` in `keel-sdk`:

```rust
#[async_trait] pub trait ListSource: Send + Sync { type Row: Serialize + JsonSchema;
    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> keel::Result<Page<Self::Row>>; }
```

A sea-orm entity is only one implementation, `entity_source::<E: Owned>(…)`, which applies
`scope_by_account`, the sort map and the search columns. A VM product fetches rows from the
hypervisor, billing returns aggregates. Whatever the implementation, the kernel, not the source,
enforces account scoping, the whitelist of sortable and filterable fields, and the totals, so a
source cannot widen what the caller may see.

### Export

Export is generic in the kernel: `keel.export { func, query, columns, format }` streams every page
through the same `ListSource` with the same scope and strip as the interactive list, so an export
never contains a column or a row the caller could not see on screen. Above a row threshold the
call becomes a task whose result is a download ([02-functions.md](02-functions.md),
`batch_threshold`). The `export` action in a list descriptor names the offered `formats`.

## Form dynamics

The shapes behind interactive forms. Which fields trigger which mechanism is declared in the `ui`
block by the builder (`.field("ttl").recompute("dns.zone.recompute")`, `.multi(["ttl"])`,
`x-keel.options`), so the client needs no product code.

| Mechanism | Request | Response |
|-----------|---------|----------|
| `kind: options` | `{ search, scope, limit, cursor }` | `{ data: [{ value, label, hint?, icon?, group? }], more }` — the source for the remote mode of `phx-select` and for `filter.remote` on columns |
| `recompute` | `{ values, changed }` | `{ values: patch, options: { field: [...] }, errors?: fields[] }`; which fields trigger it is declared in `ui` |
| group editing | `{ ids: [], fields: { ttl: 300 }, mask: ["ttl"] }` | `ui.multi` lists the fields that may be edited across several rows; above `batch_threshold` the call becomes a task |
| `kind: wizard` | `pages[].validate: <func>` validates a step on the server before the next page opens | `ui.result` after the final step: `redirect · task · download` |
| `download · upload · stream` | multipart parameters; SSE frames for `stream` | `{ filename, content_type }` for downloads; always behind a guard and an audit event |

`ui.result = task` opens the progress modal driven by `task.progress` events
([08-frontend.md](08-frontend.md)). `recompute` returns a patch, not the whole value set, so the
client merges it over what the user has typed; `options` in its response refreshes the choices of
dependent fields; `errors` uses the same `fields[]` shape as the envelope.

## Module and product composition

The compile-time contract of the SDK: a module is a crate exporting `pub static MODULE: ModuleDef`,
and a product is a `main.rs` that lists its modules explicitly. The kernel orders modules by
`requires`, not by the order of `.module()` calls; the lifecycle and the registration surface are
specified in [05-modules.md](05-modules.md).

```rust
pub static MODULE: ModuleDef = ModuleDef {
    id: "dns", version: env!("CARGO_PKG_VERSION"), sdk: keel_sdk::VERSION,
    requires: &["auth", "tasks", "host"],
    migrations: Some(&MIGRATIONS),           // migrator::embed!("$CARGO_MANIFEST_DIR/migrations")
    init,
};

fn init(r: &mut Registrar) -> keel::Result<()> {
    r.function(zone::list()).function(zone::get()).function(zone::edit()).function(zone::delete());
    r.stream("dns.zone.log", logs::tail);
    r.menu(MenuNode::new("dns/zones", "dns.zone.list").icon("globe").order(10));
    r.i18n("en", include_str!("../i18n/en.json")).i18n("ru", include_str!("../i18n/ru.json"));
    r.permission("dns.zone.read", &[Level::User]).permission("dns.zone.manage", &[Level::User]);
    r.within("keel.account.delete", hooks::forbid_delete_with_zones);
    r.contribute(LimitDef::new("dns.zone.max", Unit::Count).default(50).unlimited(true));
    r.contribute(AccountLifecycle { on_delete: Some(lifecycle::drop_zones), ..Default::default() });
    r.contribute(SettingDef::new("dns.default_ttl", schema_for!(u32)).default(3600).level(Level::Admin));
    r.contribute(CronJob::new("dns.serial_check", "0 */6 * * *", jobs::serial_check));
    r.contribute(ConfigTemplate::new("bind/zone.j2", include_str!("../templates/zone.j2")).validate("named-checkzone"));
    r.contribute(BackupSource::new("dns.zonefiles", backup::zonefiles));
    r.provide::<dyn DnsProvider>(Arc::new(BindProvider::new()));
    Ok(())
}

// products/dns/src/main.rs
fn main() -> keel::ExitCode {
    keel::Product::new(&keel_dns::MANIFEST)
        .module(&keel_system::auth::MODULE).module(&keel_system::settings::MODULE).module(&keel_system::audit::MODULE)
        .module(&keel_system::tasks::MODULE).module(&keel_system::cron::MODULE).module(&keel_system::notify::MODULE)
        .module(&keel_system::license::MODULE).module(&keel_system::update::MODULE).module(&keel_system::backup::MODULE)
        .module(&keel_host::MODULE)
        .module(&dns_zones::MODULE).module(&dns_records::MODULE).module(&dns_templates::MODULE)
        .frontend(keel::embed_frontend!("../../frontend/dist/dns"))
        .run()
}
```

`init` only registers; it performs no I/O. Everything a module can register — functions, streams,
raw routes, views, migrations, menu nodes, i18n catalogues, permission keys, hooks, descriptor
patches, services and contributions — is listed in [05-modules.md](05-modules.md).

## Plugin manifest and frames

A plugin is an external process, in any language, that registers through the same `Registrar` as
a compiled module: the frontend, the CLI, audit and the licence do not distinguish the two. Its
manifest is `keel-plugin.json`.

```json
{ "protocol": 1, "id": "acme-sync", "version": "1.2.0",
  "requires": { "keel": ">=0.6 <1", "modules": { "dns": ">=1" } },
  "run": { "mode": "daemon", "cmd": ["./bin/acme-sync", "--serve"], "restart": "always" },
  "level": "user", "storage": "kv",
  "rights": { "observe": ["keel.account.*", "auth.login"], "hook": ["dns.zone.delete"], "patch": ["dns.zone.edit"] },
  "functions": [ { "name": "acme.sync.list", "kind": "list", "access": { "level": "user" }, "key": "id",
                   "params": { "$ref": "keel://ListQuery" }, "ui": { "title": "acme.sync.list.title", "columns": [ … ] } } ],
  "patches": [ { "target": "dns.zone.edit", "op": "add_field", "after": "ttl",
                 "field": { "name": "acmeSync", "schema": { "type": "boolean", "default": true }, "store": "acme-sync" } } ],
  "settings": [ { "key": "acme.email", "schema": { "type": "string", "format": "email" } } ],
  "cron": [ { "id": "acme.renew", "schedule": "0 3 * * *", "func": "acme.renew.run" } ],
  "menu": [ { "path": "dns/acme", "func": "acme.sync.list", "order": 90, "icon": "refresh-cw" } ],
  "i18n": { "en": "i18n/en.json", "ru": "i18n/ru.json" } }
```

| Field | Meaning |
|-------|---------|
| `protocol` | the plugin protocol version; N and N-1 are accepted |
| `id`, `version` | plugin identity; the storage namespace and the plugin's principal are bound to `id` |
| `requires` | semver ranges on the platform and on the modules the plugin depends on |
| `run` | how the process is started: `mode` (`daemon` or `exec`), `cmd`, `restart`; the process runs under its own uid with timeouts and a circuit breaker |
| `level` | the level of the plugin's principal; a hook may not act on a function above it |
| `storage` | `kv` requests `plugin.kv.*` storage, namespaced by plugin and bound to a subject |
| `rights.observe` | event masks the plugin receives asynchronously from the outbox, after commit; any event may be observed |
| `rights.hook` | functions on which the plugin may run a `before` hook and veto; not above its own level and never on `keel.*` / `auth.*`, enforced by the kernel |
| `rights.patch` | functions whose descriptors the plugin may patch; a patch cannot weaken `access`, and an added field must declare `store` |
| `functions` | the plugin's own functions with descriptors in the same shape as compiled ones; calls are forwarded to the process |
| `patches` | descriptor patches; `add_field` inserts `field` after the named one |
| `settings`, `cron`, `menu`, `i18n` | the same contributions a compiled module registers: `SettingDef`, `CronJob`, menu nodes, message catalogues; `problems` and `widgets` sections exist as well |

### JSON Lines frames

The kernel writes frames to the plugin's stdin and reads frames from its stdout, one JSON object
per line. `id` correlates a request with its response, `t` is the frame type.

```jsonl
// keel → plugin (stdin), plugin → keel (stdout)
{"id":"ev-7","t":"before","func":"dns.zone.delete","principal":{…},"params":{"id":"01J9…"}}
{"id":"ev-7","ok":false,"error":{"key":"acme.error.linked","params":{"name":"a.org"},"field":"id"}}
{"id":"c-12","t":"call","func":"acme.sync.list","principal":{…},"params":{"page":1,"pageSize":25}}
{"id":"c-12","t":"invoke","func":"dns.zone.get","params":{"id":"01J9…"}}      ← under the caller's principal and the plugin's own scope
{"id":"c-12","ok":true,"data":{"data":[…],"total":1}}
```

| Frame | Direction | Meaning |
|-------|-----------|---------|
| `t: before` | kernel → plugin | a `before` hook: runs outside the transaction with a timeout; the plugin answers `ok: true` (optionally with enriched `params`) or vetoes with `ok: false` and an error whose `key`, `params` and `field` land in the caller's envelope as a field error |
| `t: call` | kernel → plugin | a call of one of the plugin's own functions, with the calling principal and parameters; the plugin answers with `ok` and `data` or `error` in the envelope shape |
| `t: invoke` | plugin → kernel | while handling a frame, the plugin calls a kernel function under the principal of the current call and within the plugin's own scope; the kernel answers with an envelope frame under the same `id` |
| observed events | kernel → plugin | delivered asynchronously from the outbox after commit, at least once, in the common event shape ([07-ecosystem.md](07-ecosystem.md)) |

A plugin never sees a `within` hook: those run inside the transaction and exist only for modules
compiled into the binary. A nested `call.invoke` inside a handler does not run plugin hooks
either; plugins see the resulting events after commit ([02-functions.md](02-functions.md)).

### Stored patch fields

The value of a field added by a patch is put by the kernel into `Call.extra`, not into the
handler's typed parameters. For an in-process module the `store` callback is called inside the
transaction after the handler, with the id of the new row. For an external plugin the value is
delivered as the event `keel.patch.stored` from the outbox after commit, at least once, and the
plugin is idempotent by the row id. Deletion of the row that owns the form emits an event on which
the owner of the patch cleans its own storage.

## Host agent protocol

The privileged process of a product speaks JSON Lines with the kernel over
`/run/keel/<product>/agent.sock`; the socket accepts only the product's own uid, checked with
`SO_PEERCRED`. The same protocol later runs over mTLS gRPC to a remote node without the module
knowing which it talks to ([06-host.md](06-host.md)).

```jsonl
→ {"id":1,"op":"host.template.apply","actor":{"login":"alice","level":"user"},"request_id":"req-1a2b",
   "args":{"template":"bind/zone.j2","vars":{…},"dest":{"root":"zones","name":"keel-dns-a.org.zone"},"mode":"0640","owner":"bind",
           "validate":"named-checkzone","reload":"named"}}
← {"id":1,"ok":true,"data":{"changed":true}}
→ {"id":2,"op":"host.service.reload","args":{"name":"nginx","validate":"nginx.test"}}
← {"id":2,"ok":false,"error":{"key":"host.error.validate_failed","detail":"nginx: [emerg] …"}}
```

| Field | Meaning |
|-------|---------|
| `id` | correlation between request and response |
| `op` | an operation of the typed catalogue (`host.user.*`, `group`, `fs`, `template.apply`, `service`, `pkg`, `cron`, `quota`, `exec`) or a product `AgentOp` |
| `actor` | login and level of the principal on whose behalf the kernel calls; written to journald with every operation |
| `request_id` | the `requestId` of the kernel call, so a host operation is traceable to the envelope |
| `args` | the operation's arguments, validated against the schema of the operation |
| `dest` | a resource identifier — a named root from `file_roots` plus a name — never a raw path; the agent resolves and validates it under its policy |
| `validate`, `reload` | the check to run on the rendered file and the service to reload; a failed check rolls the file back and returns `host.error.validate_failed` |
| `ok`, `data` / `error` | `error.key` is an i18n key that surfaces unchanged in the call envelope; `error.detail` is the raw output of the failed step |

The policy the agent enforces is `agent.yaml`:

```yaml
// agent.yaml
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
| `file_roots` | the named roots `dest.root` may refer to; a file operation outside them is rejected; every operation is written to journald with actor and request_id |
| `services` | the units `host.service.*` may act on |
| `checks` | named validation commands referenced by `validate` |
| `exec_allow` | the only binaries `host.exec` may run, with fixed arguments |
| `user_prefix` | the prefix of operating-system users the product may create and manage |
| `drop_in_prefix` | the prefix of the product's own files in include directories; the product never edits other files |

A product module extends the catalogue with an `AgentOp` — argument and result schemas, a handler
and a policy class — compiled into the same binary and executed by the agent; the module side
reaches the agent through the `HostExecutor` service:

```rust
// a product operation is registered by a module and executed by the agent of the same binary
r.contribute(AgentOp::new::<VmCreateArgs, VmRef>("vm.libvirt.create", ops::libvirt_create).policy("vm"));

let host = call.require::<dyn HostExecutor>()?;
host.template("bind/zone.j2").vars(&vars).dest("zones", name).validate("named-checkzone").reload("named").apply().await?;
```

Host operations are issued only from `after_commit` or from a task: the transaction never waits for
the agent ([02-functions.md](02-functions.md)).

## `GET /api/v1/boot`

The first request of the frontend after login. It returns everything the shell needs to render
the layout before any descriptor is fetched; it carries the same metadata version (`meta`) the
descriptors are cached under.

```json
{ "product": { "id": "dns", "version": "1.2.0", "api": 1, "edition": "pro" },
  "brand": { "name": "product.title", "logo": "/brand/logo.svg", "hue": 264, "chroma": 0.12 },
  "principal": { "id": "01J8…", "login": "res1", "level": "reseller", "account": "01J8A…", "lang": "ru", "tz": "Europe/Belgrade", "actor": null, "totp": true },
  "features": ["dns.zones", "dns.dnssec", "dns.secondary"], "license": { "state": "active", "expires": "2026-10-12T00:00:00Z" },
  "menu": [ { "id": "dns", "label": "menu.dns", "icon": "globe",
              "items": [ { "path": "dns/zones", "label": "dns.zone.list.title", "func": "dns.zone.list", "kind": "list", "badge": { "problem": "dns.zone_stale" } } ] },
            { "id": "system", "label": "menu.system", "items": [ { "path": "system/accounts", "func": "keel.account.list" }, { "path": "system/tasks", "func": "task.list" } ] } ],
  "dashboard": [ { "kind": "stat", "func": "dns.widget.zones" }, { "kind": "list", "func": "problem.list", "limit": 5 } ],
  "views": [], "meta": "m-7f3a2c", "problems": { "open": 1, "critical": 0 } }
```

| Field | Meaning |
|-------|---------|
| `product` | product id, version, API version of the product and the licensed edition |
| `brand` | brand name key, logo path and the theme hue and chroma the shell feeds to `PhxThemeService`; replaced per hostname by a `ManifestPatch` for white-label |
| `principal` | the effective principal with language and time zone from `settings_pref`, the `actor` when delegated (`su`, on-behalf) and whether TOTP is enrolled |
| `features` | licence features enabled in this edition; a function whose `access.feature` is absent is hidden from the menu and answers `402` |
| `license` | state and expiry of the lease ([05-modules.md](05-modules.md), `license`) |
| `menu` | groups from the product manifest with items projected under the principal's level: `path` is the shell route, `func` the function it opens, `kind` its screen kind, `badge.problem` a problem key whose open count is shown on the item |
| `dashboard` | widgets: a `kind` and the function that feeds it |
| `views` | the bespoke views registered with `r.view(...)` that the principal may open; the shell shows `phx-not-found` for an unknown one |
| `meta` | the metadata version, equal to `X-Keel-Meta-Version` |
| `problems` | counts for the notification rail |

Labels in `menu` and `brand.name` are i18n keys resolved by the client from the catalogues served
under `/api/v1/i18n`.
