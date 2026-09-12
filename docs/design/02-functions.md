# Functions, descriptors and the dispatcher

This document specifies the unit of work in Keel — the function — and the two things the kernel
builds around it: the descriptor, which tells every client what a function takes, returns and how
its screen looks, and the dispatcher pipeline, which runs every call in one fixed order regardless
of who is calling. Ownership rules that the pipeline enforces are defined in
[03-access.md](03-access.md); the wire shapes referred to here are printed in full in
[10-contracts.md](10-contracts.md); where functions are registered and how modules hook into them
is in [05-modules.md](05-modules.md).

## The function

Everything a product does is a function named `<module>.<entity>.<verb>`: `dns.zone.list`,
`dns.zone.edit`, `keel.account.list`, `task.list`, `auth.su`. The first segment is the id of the
module that owns it; kernel functions use the `keel` prefix. A function declares:

- its parameters as JSON Schema, produced from a Rust type through `schemars`;
- its result, likewise;
- its access: the minimum level, a permission key and, optionally, a license feature;
- its mode, `read` or `write`;
- its screen: the `ui` block described below.

The frontend, the CLI (`ctl` over the local socket), the external HTTP API, plugins and peer
products are all clients of the same dispatcher and receive the same envelope. There is no second
code path for any of them: a plugin's `call` frame, a peer's `POST /api/v1/call/{func}` with a
`kpt_` token and a browser's cookie request enter the same pipeline. The only routes that bypass
descriptors are the raw routes `/api/v1/x/<module>/…` a module may register for upload, download,
a WebSocket terminal or VNC; they are allowed only with a mandatory guard and an audit event.

A module registers its functions in `init` through the registrar (`r.function(zone::edit())`);
the registries are frozen during boot, after every module's `init` and before `start`, and
validated then, so every error a descriptor can carry — an unknown field in a condition, a
missing translation key, a forgotten ownership declaration — is a start-up failure or a CI
failure, never a runtime one.

## The descriptor

A descriptor is the machine-readable description of one function. It is served per principal at
`GET /api/v1/functions/<name>` (already projected: columns and actions above the caller's level
are removed) and carries the top-level fields below. The complete example for `dns.zone.list` is in
[10-contracts.md](10-contracts.md).

| Field    | Meaning                                                                                  |
|----------|------------------------------------------------------------------------------------------|
| `keel`   | Descriptor format version; `1` for `/api/v1`. A breaking change ships as `/api/v2` alongside. |
| `name`   | Function name.                                                                           |
| `module` | Id of the owning module.                                                                 |
| `kind`   | One of the closed `kind` enum, see below.                                                |
| `mode`   | `read` or `write`; decides whether the pipeline opens a transaction.                     |
| `access` | `{ level, permission, feature }`.                                                        |
| `key`    | Name of the parameter that identifies the owned row (`"id"`); its presence makes the function subject to `OwnedGuard`. |
| `params` | JSON Schema of the parameters; lists point at `keel://ListQuery` through `$ref`.        |
| `result` | JSON Schema of the result.                                                               |
| `ui`     | The screen block; a kind without a screen of its own (`get`, `options`, `download`, `upload`) omits it. |

Titles, labels, hints and error messages inside a descriptor are i18n keys, never strings;
the key convention and the bundle pipeline are in [08-frontend.md](08-frontend.md).

### Three parts of a descriptor

1. **The parameter schema** is pure JSON Schema. Any standard validator, any code generator and
   any peer product that renders a form from a foreign schema can consume it without knowing
   Keel. Nothing the kernel relies on for correctness is hidden in a place a standard tool would
   drop.

2. **Field attributes inside the schema** live in two namespaces with different guarantees:

   | Namespace | Contents                                          | Guarantee                                                                                  |
   |-----------|---------------------------------------------------|--------------------------------------------------------------------------------------------|
   | `x-keel`  | `level`, `secret`, `check` presets, `options` function | Protocol semantics. Part of `keel-types` and `PROTOCOL = 1`; the kernel is obliged to enforce it in the guard and strip stages. |
   | `x-ui`    | `control`, `placeholder`, `help`, `order`, and similar presentation hints such as `generate` | Presentation only. A client that ignores it renders a plainer screen and nothing else breaks. |

   `level` says which principal level may see and send the field; `secret` says the value must
   never come back in a response and drives the same masking in audit records, logs and the
   support bundle; `check` names a validation preset the kernel applies on top of the schema;
   `options` names a `kind: options` function that supplies the field's choices. These four
   were deliberately placed outside `x-ui`: the two namespaces carry different guarantees, and
   mixing enforced attributes into a namespace a client may ignore would let that client treat
   them as optional. A standard validator ignores both namespaces alike, which is why the kernel
   enforces `x-keel` itself, on every call.

3. **The `ui` block of the screen** holds what the renderer needs and the API does not: columns,
   actions, pages, the conditions `show_if` / `required_if` / `readonly_if`, `recompute`
   triggers and `invalidate_on`. It can change from release to release without breaking a
   client that only calls the function.

What the `ui` block carries, as seen in the two examples in this document set:

| Screen kind | Members                                                                                               |
|-------------|-------------------------------------------------------------------------------------------------------|
| list        | `title`, `search` (fields), `columns[]` (`key`, `type`, `to`, `sortable`, `primary`, `total`, `map`, `filter` with `options` or `remote`, inline `edit` naming a function, per-column `access`), `actions[]` (`id`, `role`, `screen` or `function`, `icon`, `default`, `params`, `confirm` incl. `destructive` / `items` / `input`, `hidden_if`, `formats`), `row_actions`, `empty` (`title`, `action`), `invalidate_on` (event patterns) |
| form        | `title`, `read` (the `get` function that supplies current values), `pages[]` with field lists and `show_if`, per-field `readonly_if` / `required_if` / `recompute`, `multi` (fields allowed in group editing) |
| wizard      | `pages[]`, each with `validate` naming the function that checks the step on the server; `result` after the final step: `redirect`, `task` or `download` |

A form is therefore a pair: the `get` function named in `ui.read` supplies the values, and the
form function itself performs the write. Parameters that the screen needs from the URL
(`zone_id` of a child list) arrive through `params` of the action and are visible to conditions
as `$screen.params.*`.

### Closed vocabulary

Every enumerable member of a descriptor is a closed enum in `keel-types`, serialised by name and
generated into a TypeScript union for `@globalart/keel/meta`:

| Enum          | Values                                                                                                                 |
|---------------|------------------------------------------------------------------------------------------------------------------------|
| `kind`        | `list` · `get` · `form` · `action` · `wizard` · `report` · `options` · `stream` · `download` · `upload`               |
| `control`     | Closed set of form controls (`tags`, `password`, … as used by `x-ui.control`).                                          |
| `column.type` | Closed set with an explicit mapping onto phoenix-ui's `PhxColumn.type` (`link`, `number`, `status`, `toggle`, `text`, …). |
| `action.role` | `new` · `edit` · `delete` · `group` · `button` · `child` · `external` · `export` · `upload` · `download` · `refresh`   |
| `check`       | Validation presets: `domain`, `ip`, `cidr`, `email`, `int`, `name`, `port`, `path`, and others.                         |

Icons are strings, but every icon named in a descriptor or menu node is checked against
`PhxIconName` at start-up.

The reason the vocabulary is closed rather than documented: descriptor v1 is a frozen contract
guarded by golden fixtures and a snapshot diff in CI. With open strings the diff catches nothing
and the frontend renders by string comparison; with enums a new value is a change to
`keel-types`, visible in review and mirrored in the TypeScript union. `get` and `options` are
members of `kind` so that the functions a form's `ui.read` and a field's `x-keel.options` name
are typed like every other function rather than by convention.

What each kind means for the pipeline and the renderer:

| `kind`      | Semantics                                                                                                          |
|-------------|--------------------------------------------------------------------------------------------------------------------|
| `list`      | Takes `ListQuery`, returns `Page<T>`; `GET` is allowed; scoping, field whitelist and totals are done by the kernel through `ListSource`. |
| `get`       | Returns one object by key; `GET` is allowed; used as `ui.read` of a form.                                          |
| `form`      | A write with a screen: pages, conditions, recompute, group editing.                                                |
| `action`    | A write invoked from an action of a list or object screen, usually with `confirm`.                                 |
| `wizard`    | A write split into server-validated steps with a `result` after the last one.                                      |
| `report`    | Bands and diagrams for the report renderer (M5).                                                                   |
| `options`   | `{ search, scope, limit, cursor }` → `{ data: [{ value, label, hint?, icon?, group? }], more }`, the source for remote selects and filters. |
| `stream`    | Server-sent frames, for example a log tail.                                                                        |
| `download`  | Returns `{ filename, content_type }` and streams a body; always behind a guard and an audit event.                  |
| `upload`    | Multipart parameters; always behind a guard and an audit event.                                                    |

The request and response shapes for `options`, `recompute`, group editing, wizard steps and
download / upload / stream are tabulated in [10-contracts.md](10-contracts.md); how each kind is
rendered is in [08-frontend.md](08-frontend.md).

### Conditions

Conditions are written in a small expression language, `kind == 'slave' && !dnssec`, and
compiled at start-up into a JSON AST. The AST is what the wire descriptor carries, for example
`"hidden_if": { "eq": ["status", "suspended"] }`.

| Element   | Set                                                                                              |
|-----------|--------------------------------------------------------------------------------------------------|
| Operators | `eq` · `ne` · `in` · `gt` · `lt` · `truthy` · `empty` · `matches` · `and` · `or` · `not`         |
| Variables | Fields of the form or row; `$mode`, the mode of the form (`'edit'` in the example); `$principal.level`; `$screen.params.*` |

There are two interpreters, one in Rust and one in TypeScript, each about 200 lines, and one set
of fixtures, `contracts/fixtures/expr-cases.json`, that both must pass. Field names used in a
condition are resolved against the schema when the registries are frozen: a typo fails the
start of the binary, not a form in production. The remaining risk — the two interpreters
drifting apart — is covered by the shared fixtures from M0 and by a conformance test that a
field is shown if and only if the server accepts it (see [11-decisions.md](11-decisions.md)).

The operator and variable sets above are the whole language in descriptor v1.

### How a function is written

There are no project-specific procedural macros. Parameters are an ordinary
`#[derive(JsonSchema)]` struct with `#[schemars(extend(...))]` on fields for `x-keel` and
`x-ui`; the screen is a `ScreenUi` value from `keel-types`, filled through a builder next to the
handler. The only check on the shape of a descriptor is the golden fixture and the snapshot diff
in CI. A derive for the `ui` block will be added only if, after two products, the builder code
proves repetitive.

A module lists `keel-sdk` as its only dependency ([05-modules.md](05-modules.md)), so the
derives are pointed at the SDK's re-exports: `#[serde(crate = "keel_sdk::serde")]` and
`#[schemars(crate = "keel_sdk::schemars")]` on every struct that derives them, and
`use keel_sdk::sea_orm;` in the module that defines an entity.

```rust
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct ZoneEdit {
    pub id: Option<Id>,
    #[schemars(extend("x-keel" = json!({ "check": "domain" })))]
    pub name: String,
    pub kind: ZoneKind,
    #[schemars(extend("x-keel" = json!({ "check": "ip" }), "x-ui" = json!({ "control": "tags" })))]
    pub master_ips: Vec<String>,
    pub ttl: u32,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password", "generate": true })))]
    pub tsig_key: Option<Secret>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub account: Option<Id>,
}

pub fn edit() -> FunctionDef {
    keel::function::<ZoneEdit, Saved>("dns.zone.edit")
        .kind(Kind::Form).mode(Mode::Write)
        .access(Level::User, "dns.zone.manage").feature("dns.zones")
        .owned::<zone::Entity>()
        .ui(Form::new("dns.zone.edit.title")
            .read("dns.zone.get")
            .page("main", ["name", "kind", "ttl", "account"])
            .page("transfer", ["master_ips", "tsig_key"]).show_if("kind == 'slave'")
            .field("name").readonly_if("$mode == 'edit'")
            .field("master_ips").required_if("kind == 'slave'")
            .field("ttl").recompute("dns.zone.recompute")
            .multi(["ttl"]))
        .handler(|call: Call<ZoneEdit>| async move {
            let account = call.resolve_account(call.params().account).await?;
            let row = zone::upsert(call.tx()?, account, call.params()).await
                .map_err(|e| KeelError::conflict("dns.zone.error.exists").field("name").source(e))?;
            call.emit("dns.zone.saved", &row)?;
            let dns = call.require::<dyn DnsProvider>()?;
            call.after_commit(move |ctx| async move { dns.apply(&ctx, &row).await });
            Ok(Saved { id: row.id })
        })
}
```

Reading the example against the descriptor parts:

- `name` and `master_ips` carry `check` presets in `x-keel`; the kernel validates them, and the
  frontend pre-validates them with the same fixtures. `master_ips` additionally asks for the
  `tags` control in `x-ui`.
- `tsig_key` is `secret`: it is typed `Secret` (encrypted at rest, see
  [04-storage.md](04-storage.md)), never returned, masked in audit; the `password` control with
  `generate: true` is presentation only.
- `account` is visible and writable only from level `reseller` upward and takes its choices
  from `keel.account.options`. A `user` never sees the field in the descriptor and gets
  `forbidden_field` if they send it.
- `.access(Level::User, "dns.zone.manage").feature("dns.zones")` fills `access`.
- `.owned::<zone::Entity>()` binds the function to an entity with an account column, which
  turns on `OwnedGuard` for the row named by `id`.
- `Form::new(...)` builds the `ui` block: two pages, the second shown only for slave zones;
  `name` read-only once the object exists; `master_ips` required for slave zones; changing
  `ttl` calls `dns.zone.recompute`; `ttl` is the one field allowed in group editing.
- The handler receives `Call<ZoneEdit>` and uses only what the SDK exposes on it:
  `principal()` and `resolve_account` (see [03-access.md](03-access.md)), `params()`, `tx()`
  — the transaction the kernel opened; a read call has none, so `tx()` answers an error there
  and `db()` is the accessor that works in both modes — `emit` into the outbox,
  `require::<dyn Trait>()` for a service another module provides, and `after_commit` for work
  that must not run inside the transaction. `KeelError::conflict("dns.zone.error.exists").field("name")` becomes a field
  error keyed for translation, an entry of `fields[]` with `path: name`; the status codes are
  listed in [10-contracts.md](10-contracts.md).

Other members of `Call` used elsewhere in the design: `invoke` for a nested function call,
`check_limit(key, current)` for account limits, and `extra` for values of fields added to the
descriptor by a patch.

### Validation at start-up and in CI

When the registries are frozen during boot, after every module's `init` and before `start`,
the kernel:

- applies patches to descriptors of other modules and plugins, refusing a patch that weakens
  `access` and requiring `store` on every added field;
- compiles every condition and checks each field name against the schema;
- validates every descriptor (enum membership, icons against `PhxIconName`) and checks that
  i18n keys exist at least for `en`.

`keel-dev check` adds what needs the source: every function with a `key` is either `.owned()` or
explicitly `.unowned()`, and module prefixes are respected. `keel-dev descriptors dump | check |
diff` produce and compare the snapshots that CI keeps as golden fixtures.

## The dispatcher pipeline

The order of stages is fixed in the kernel, not left to the conventions of handlers. Conventions
are what reproduce worst when modules are written quickly: three modules would open the
transaction three different ways, one would emit an event before commit, one would call the host
agent inside the transaction. The kernel therefore owns the transaction, the outbox write, the
idempotency record and the audit event, and a handler only ever sees a `Call`.

A function declares `mode: read | write`, and the mode decides which stages exist.

### Stages, in order

1. **Resolve.** The name is looked up in the frozen registry; kind and mode are read from the
   descriptor. An unknown name ends the call here.
2. **Guards.** Level, permission, feature, token scope, module guards, and `OwnedGuard` for
   functions with a `key`. Any failure ends the call here.
3. **Strip and validate.** Fields above the caller's level are handled first — removed on the
   way out, `forbidden_field` on the way in — then the parameters are validated against the
   schema and the `check` presets.
4. **Idempotency.** Write mode only: a repeated `Idempotency-Key` is either replayed or
   rejected with `409`. A replay ends the call here; nothing below runs again.
5. **Before hooks.** Outside the transaction, with a timeout; they may read, enrich the
   parameters, or veto. This is the stage open to plugins.
6. **Transaction.** Write mode only: the kernel opens the transaction that `call.tx()` exposes.
7. **Within hooks.** Inside the transaction; in-process modules only.
8. **Handler.** The function body, with `call.tx()` and nested `call.invoke` on the same
   transaction.
9. **Emit to outbox.** Every `call.emit` is written into the same transaction.
10. **Commit or rollback.** A handler error rolls everything back, including the outbox rows.
11. **Audit event.** An in-process event describing the call, raised after commit and after
    rollback alike, so failed calls are audited too.
12. **After.** `after_commit` callbacks run asynchronously; host-agent operations and peer calls
    happen here or in a task, never inside the transaction.

Stages 4 (idempotency), 6 (transaction), 7 (within hooks), 9 (emit to outbox) and 10 (commit or
rollback) exist only for `mode: write`. The handler runs in both modes; for a read it runs on a
connection from the pool without a transaction.

### Read and write mode

| Aspect         | `mode: read`                                   | `mode: write`                                        |
|----------------|------------------------------------------------|------------------------------------------------------|
| Transaction    | none; one connection from the pool for the call | opened by the kernel before within hooks, held in `Call` |
| Idempotency    | not applied                                    | `Idempotency-Key` handled by the kernel               |
| Outbox         | not available                                  | `call.emit` writes into the call's transaction        |
| Audit          | off by default                                 | event after commit or rollback                        |
| HTTP verb      | `GET` allowed for `kind: list` and `kind: get` | `POST`                                               |

The license module adds one more distinction: in the read-only degradation of an expired lease,
write functions answer `402` while reads and login keep working (see
[05-modules.md](05-modules.md)).

### Guards

The guard stage runs the checks a function's `access` and its registrations imply:

- **Level.** The caller's level on the fixed ladder `guest < service < user < reseller < admin
  < owner` must reach `access.level`. `auth.login` and `keel.setup.admin` are `guest`
  functions, which is why rate limiting, audit and hooks apply to them like to any other.
- **Permission.** `access.permission` is a key such as `dns.zone.manage`, with defaults per
  level supplied by the module that registers it and stored by the `auth` module through
  `PermissionSource` (see [03-access.md](03-access.md)).
- **Feature.** `access.feature` must be enabled for the installation; the set comes from the
  product edition and, from M5, from the license lease. A function whose feature is off is
  absent from the menu and answers `402`.
- **Token scope.** An API token or a peer token may be limited to a mask of functions, a level
  ceiling and on-behalf rights; a token scoped to one function receives `403` on another.
- **Module guards.** Guards contributed by modules, such as the `auth` module's IP rules and
  rate limit, and the guard a module attaches to a raw route.
- **OwnedGuard.** Mandatory for every function that declares a `key`. Before the handler runs,
  the kernel loads the `account_id` of the row named by the key — or of every id in a batch —
  and checks it against the caller's scope. A row outside the scope is answered with
  `not_found`, not `forbidden`, so that the existence of another account's objects is not
  revealed. A function that has a key but no owner (a system setting, a task) declares
  `.unowned()` explicitly; a function that declares neither fails `keel-dev check`. The guard
  is what closes the gap between lists, which are scoped through `ListSource`, and single-object
  functions, which would otherwise depend on each handler remembering to check. `keel-testkit`
  ships the two tests every module template starts with: a reseller sees only its subtree, and a
  user cannot edit another account's row by id.

The scoping rules themselves — `Owned`, `scope_by_account`, `resolve_account`, `ScopePolicy` —
are specified in [03-access.md](03-access.md).

### Strip and `forbidden_field`

A field or column whose `x-keel.level` is above the caller's level does not exist for that
caller:

- in the descriptor: the column or action is not served, and the descriptor's `ETag` includes
  the principal and its `acl_gen`, so a change of rights invalidates the cached copy (see
  [10-contracts.md](10-contracts.md));
- in a response: the field is cut from the row or object;
- in a request: sending it is an error, `403` with `code: forbidden_field` and the offending
  paths in `fields[]`, never a silent drop. A client that silently lost a field would believe
  the write succeeded as sent.

Validation errors from the same stage are `422` with `code: validation` and one entry per
field: `path`, `code` (`check.ip`, or a module key), the i18n `key` and `params`. A `secret`
field is accepted on input and masked everywhere on output, including the audit record.

### Two kinds of hooks

| Hook     | Runs                        | Timeout | May                                                       | Open to                          |
|----------|-----------------------------|---------|-----------------------------------------------------------|----------------------------------|
| `before` | before the transaction opens | yes     | read through the pool, enrich `params`, veto with an error | modules and plugins              |
| `within` | inside the transaction       | no      | read and write on `call.tx()`, veto                        | modules compiled into the binary |

The split follows from what each side can be trusted with. A `before` hook may live in another
process and may be slow or dead, so it gets a timeout and no transaction; a plugin that vetoes
returns an error with an i18n key, parameters and optionally a field path, and the caller sees
it as a field error of the form. Plugins may hook only functions not above their own level and
never `keel.*` or `auth.*`; the plugins module enforces that at start-up and adds per-plugin
timeouts and a circuit breaker (see [05-modules.md](05-modules.md)).

A `within` hook shares the transaction and therefore the serialisation guarantees of the
handler. Invariant checks — quotas, account limits, reservations — are allowed only there,
because a check made outside the transaction can be invalidated by a concurrent write before
the handler runs. A module registers one with `r.within("keel.account.delete",
hooks::forbid_delete_with_zones)`.

Descriptor patches ride on the same distinction. The value of a field added by a patch is put
into `Call.extra`; for an in-process module the patch's `store` callback runs inside the
transaction after the handler, with the id of the new row; for an external plugin the kernel
emits `keel.patch.stored` from the outbox after commit, at least once, and the plugin must be
idempotent by id. Deleting the row that owns the form emits an event on which the patch owner
cleans its own storage.

### Nested `invoke`

`call.invoke` calls another function from inside a handler or hook. It reuses the transaction
of the outer call, runs under the outer call's principal and scope, and does not run plugin
hooks: plugins observe the effects as outbox events after commit, never as an interception of
an inner call. A module reads another module's data only through `call.invoke` or a service
the other module provides; direct SQL across module prefixes is forbidden (see
[04-storage.md](04-storage.md)). The M0 acceptance list includes the test that a nested
`invoke` reuses the caller's transaction.

### Outbox

`call.emit(type, &data)` appends an event to `keel_outbox` inside the call's transaction. If the
transaction rolls back, the event never existed; if it commits, delivery is guaranteed at least
once to everything that reads the outbox: webhooks, peers, notifications, SSE
(`Last-Event-ID` is the outbox `seq`), plugins with the `observe` right and the `keel.patch.stored`
mechanism above. Each consumer keeps its own cursor, and retention never discards an event
that some consumer has not yet passed. Every event has the same shape across products; the
delivery mechanics are in [07-ecosystem.md](07-ecosystem.md) and the table in
[04-storage.md](04-storage.md).

`invalidate_on` in a list's `ui` block names event patterns (`dns.zone.*`); the frontend refreshes
the list when a matching `data.invalidate` arrives, filtered by the principal's scope.

### `Idempotency-Key`

The kernel, not the module, implements idempotent writes. A client sends `Idempotency-Key` on a
write call; the kernel records the key together with the principal, the function and a hash of
the request body, and the response once it exists. A repeat with the same key and the same body
is answered with the stored response and the header `X-Keel-Idempotent-Replay`; a repeat with the
same key and a different body is `409` with `code: idempotency_mismatch`. The records live in
`keel_idempotency` with an expiry (see [04-storage.md](04-storage.md)). Because the stage sits
before the hooks and the handler, a replay costs no hook, no transaction and no second event.

`keel-client` retries only requests that carry a key; a billing peer opening a service twice
with the same key therefore gets one service (the M6 acceptance criterion). The end-to-end path
across products is finished in M6.

### Audit event

After commit or rollback the kernel raises an in-process audit event carrying who called
(principal and, under `su` or on-behalf, the actor), what, with which parameters — `secret`
fields masked — and how it ended. The event is raised for failed calls as well, which is the
reason it is not written inside the transaction: an audit row inside a rolled-back transaction
would vanish with it. The `audit` module subscribes and writes in batches from a background
worker so that the call's response does not wait for the audit write. For read functions the
event is off by default. Optional subject and before/after snapshots for form functions, which
feed the object-history screen, are described with the module in
[05-modules.md](05-modules.md).

### `after_commit`, host operations and tasks

Anything that talks to the operating system through the host agent, to a peer product, or to
any other party outside the database runs either in an `after_commit` callback or in a task.
The transaction never waits for an agent or a network round trip: the row is committed, and
only then `dns.apply(&ctx, &row)` renders the zone file. This keeps the transaction short and
keeps a slow or failed external step from leaving the database undecided. A task is the vehicle
when the work must survive a restart, report progress or be cancelled (see
[05-modules.md](05-modules.md)); the design does not define retry semantics for a failed
`after_commit` callback beyond that.

### Limits

Per function, declared on the definition:

| Limit             | Effect                                                                                        |
|-------------------|-----------------------------------------------------------------------------------------------|
| `timeout`         | Upper bound on the call.                                                                       |
| `max_page_size`   | Cap on `ListQuery.page_size` for lists.                                                        |
| `default_sort`    | Sort applied when the query carries none.                                                      |
| `batch_threshold` | A group-edit or batch call with more ids than this is not executed inline: it becomes a task with progress and log. The same rule turns `keel.export` above a row threshold into a task whose result is a download. |

Globally: `body_limit` on the request body and `sse_max_clients` on concurrent event streams.

### Error codes by stage

The envelope and the full code table are in [10-contracts.md](10-contracts.md); this is where
each code originates in the pipeline.

| Stage                | Codes                                                                                  |
|----------------------|----------------------------------------------------------------------------------------|
| Guards               | `unauthenticated` 401, `forbidden` 403, `license` 402, `rate_limited` 429 (module guard) |
| OwnedGuard, handler  | `not_found` 404                                                                        |
| Strip and validate   | `forbidden_field` 403, `validation` 422                                                |
| Idempotency          | `idempotency_mismatch` 409                                                             |
| Hooks and handler    | `conflict` 409, `limit_exceeded` 409, module errors keyed to fields                    |
| Peer calls and nodes | `peer` 502, `unavailable` 503                                                          |
| Anywhere             | `internal` 500                                                                         |
