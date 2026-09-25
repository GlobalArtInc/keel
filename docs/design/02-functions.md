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
module that owns it; kernel functions use the `keel` prefix. A module with a single entity may
drop the middle segment, as the demo's `note.list` and `note.edit` do; the i18n prefix of such
a function is then the module id alone. A function declares:

- its parameters as JSON Schema, produced from a Rust type through `schemars`;
- its result, likewise;
- its access: the minimum level, a permission key and, optionally, a license feature;
- its mode, `read` or `write`;
- its screen: the `ui` block described below.

The frontend, the CLI (`ctl` over the local socket), the external HTTP API, plugins and peer
products are all clients of the same dispatcher and receive the same envelope. There is no second
code path for any of them: a plugin's `call` frame, a peer's `POST /api/v1/call/{func}` with a
`kpt_` token and a browser's cookie request enter the same pipeline. The only routes that bypass
descriptors are the raw routes `/api/v1/x/<module>/…` for upload, download, a WebSocket terminal
or VNC; they are allowed only behind authentication and with an audit event. The ones built so
far are the kernel's projection of the `download` and `upload` kinds, served through the whole
pipeline; a free-form raw route a module registers itself waits for `panel` in M5
([05-modules.md](05-modules.md)).

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
| `ui`     | The screen block; a kind without a screen of its own (`get`, `options`, `download`) omits it, while an `upload` carries a `Form` block like a `form`. A `get` reached through a `link` column or a row action is shown by the object renderer of [08-frontend.md](08-frontend.md) from its result schema, without a `ui` block of its own. |

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
   | `x-keel`  | `level`, `secret`, `once`, `check` presets, `options` function, `file { maxBytes, types }` on a field whose control is `file` | Protocol semantics. Part of `keel-types` and `PROTOCOL = 1`; the kernel is obliged to enforce it in the guard and strip stages. |
   | `x-ui`    | `control`, `placeholder`, `help`, `order`, and similar presentation hints such as `generate` | Presentation only. A client that ignores it renders a plainer screen and nothing else breaks. |

   `level` says which principal level may see and send the field; `secret` says the value must
   never come back in a response and drives the same masking in audit records, logs and the
   support bundle; `once` says the value is answered to the call that produced it and is `***`
   in every stored copy — the record an `Idempotency-Key` replays from and the audit event —
   which is the shape of an API token or a recovery code, shown when it is made and never
   again; `check` names a validation preset the kernel applies on top of the schema; `options`
   names a `kind: options` function that supplies the field's choices; `file` fixes the size and
   the media types of a part of an `upload`, which the kernel checks while reading the part
   ([10-contracts.md](10-contracts.md)). These attributes were
   deliberately placed outside `x-ui`: the two namespaces carry different guarantees, and
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
| action      | `title`; `result` when the call does not answer inline: `task` opens the progress modal on the `TaskRef` the dispatcher answers ([10-contracts.md](10-contracts.md)) |

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
| `check`       | Validation presets: `domain`, `ip`, `ipv4`, `ipv6`, `cidr`, `email`, `int`, `name`, `port`, `path`, `quoted`, `caa`, and others. `checkBy { field, cases }` picks the preset by the value of a sibling field, as the record type picks the check of a record's value. |

Icons are strings, but every icon named in a descriptor action, a menu node or a menu group is
checked at start-up against the icon set of phoenix-ui, embedded from `contracts/icons.json`, which
`pnpm generate` writes from `PHX_ICON_NAMES` ([08-frontend.md](08-frontend.md)).

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
| `get`       | Returns one object by key; `GET` is allowed; used as `ui.read` of a form and shown by the object renderer when a link or a row action opens it. |
| `form`      | A write with a screen: pages, conditions, recompute, group editing.                                                |
| `action`    | A write invoked from an action of a list or object screen, usually with `confirm`; in `mode: read` also the kind of a `recompute` or `pages[].validate` helper, which has no screen. |
| `wizard`    | A write split into server-validated steps with a `result` after the last one.                                      |
| `report`    | Bands and diagrams for the report renderer (M5).                                                                   |
| `options`   | `OptionsQuery { search, scope, limit, cursor }` → `Choices { data: [Choice { value, label, hint?, icon?, group? }], more }` from `keel-types`, `limit` 20 by default and `value` a string; the source for remote selects and filters. |
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
    function::<ZoneEdit, Saved>("dns.zone.edit")
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
  `require::<dyn Trait>()` for a service another module provides — `dyn Clock` among them,
  the kernel's clock from which every stored time is stamped, so that a test can freeze it —
  and `after_commit` for work that must not run inside the transaction.
  `KeelError::conflict("dns.zone.error.exists").field("name")` becomes a field error keyed for
  translation, an entry of `fields[]` with `path: name`, and the envelope's `code` is then
  `validation` (`422`) whatever code the error was built with; the status codes are listed in
  [10-contracts.md](10-contracts.md).

Other members of `Call` used elsewhere in the design: `invoke` for a nested function call,
`check_limit(key, current)` for account limits, and `extra` for values of fields added to the
descriptor by a patch.

### Validation at start-up and in CI

When the registries are frozen during boot, after every module's `init` and before `start`,
the kernel:

- applies patches to descriptors of other modules and plugins, refusing a patch that weakens
  `access` and requiring `store` on every added field;
- compiles every condition and checks each field name against the schema;
- validates every descriptor (enum membership, icons against the embedded icon set of
  `contracts/icons.json`, every function, action and permission it names is registered, `ui.read`
  is a `get` and `x-keel.options` an `options` function) and checks that i18n keys exist at least
  for `en`;
- checks the task rules: a `result: task` function needs a module providing the `TaskRunner`, a
  function that can become a job — `result: task`, or a `batch_threshold` — carries no `secret`
  parameter, since a job stores its parameters, the one exception being a form with `ui.multi`
  and a `batch_threshold`, whose job parameters are `ids`, `fields` and `mask` alone; `ui.multi`
  names parameters of the form that are neither its key nor `secret`, and a form with `ui.multi`
  has a key and a `ui.read`; every field whose control is `file` carries `x-keel.file`, and an
  `upload` carries a `Form` block.

`keel-dev check` adds what needs the source: every function with a `key` is either `.owned()` or
explicitly `.unowned()`, and module prefixes are respected; the subcommand arrives with the
milestone that needs it. `keel-dev descriptors dump | check | diff` produce and compare the
projected snapshots per level that CI keeps under `contracts/dump/` through `hack/descriptors.sh`,
and `keel-dev openapi` writes `contracts/openapi.json` from the same dump
([10-contracts.md](10-contracts.md)).

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
write functions answer `402` while reads and login keep working. The lease is one document per
installation listing the enabled products and an edition for each; a product the lease no longer
lists degrades the same way on its own, product by product (see [05-modules.md](05-modules.md)).

### Guards

The guard stage runs the checks a function's `access` and its registrations imply:

- **Level.** The caller's level on the fixed ladder `guest < service < user < reseller < admin
  < owner` must reach `access.level`. `auth.login` and `keel.setup.admin` are `guest`
  functions, which is why rate limiting, audit and hooks apply to them like to any other.
- **Permission.** `access.permission` is a key such as `dns.zone.manage`, with defaults per
  level supplied by the module that registers it and stored by the `auth` module through
  `PermissionSource` (see [03-access.md](03-access.md)).
- **Feature.** `access.feature` must be enabled for the installation. The enabled set is the
  union, over the enabled products, of the features of the edition the lease grants that product
  (until M5, of the edition the bundle names as its default). A function whose feature is off is
  absent from the menu and answers `402`. A function of a disabled product is never registered,
  so its name is unknown to the resolve stage and answers `not_found`, not `402`.
- **Token scope.** An API token or a peer token may be limited to a mask of functions, a level
  ceiling and on-behalf rights; a token scoped to one function receives `403` on another,
  `keel.error.token_scope` with the function in `params`, and a read-only token receives
  `keel.error.token_read_only` on a write. The guard runs for the entered call only: a nested
  `invoke` belongs to a call the guard already admitted, the same rule as module guards.
- **Module guards.** Guards contributed by modules, such as the `auth` module's IP rules and
  rate limit, and the guard a module attaches to a raw route. A module registers one with
  `r.guard(Arc<dyn Guard>)`; the guard receives the function name and the call, runs on a
  pooled connection for every call that enters through a transport, and refuses with any error
  code (`rate_limited`, `forbidden`). A nested `invoke` does not run module guards: it belongs
  to a request the guards already admitted.
- **OwnedGuard.** Mandatory for every function that declares a `key`. Before the handler runs,
  the kernel loads the `account_id` of the row named by the key — or of every id in a batch,
  when the key parameter is an array such as `ids`, checked element by element — and checks it
  against the caller's scope. A row outside the scope is answered with `not_found`, not
  `forbidden`, so that the existence of another account's objects is not revealed; one foreign
  id fails the whole batch. A function that has a key but no owner (a system setting, a task)
  declares `.unowned()` explicitly; a function that declares neither fails `keel-dev check`,
  and the dispatcher refuses to run it (`internal`, `keel.error.ownership_undeclared`). The
  guard is what closes the gap between lists, which are scoped through `ListSource`, and
  single-object functions, which would otherwise depend on each handler remembering to check.
  `keel-testkit` ships the two tests every module template starts with: a reseller sees only
  its subtree, and a user cannot edit another account's row by id. A hook that vetoes a keyed
  call names the key parameter as its field (`ids` for a batch), which is where the screen
  shows the veto.

The scoping rules themselves — `Owned`, `scope_by_account`, `resolve_account`, `ScopePolicy` —
are specified in [03-access.md](03-access.md).

### Strip and `forbidden_field`

A field or column whose `x-keel.level` is above the caller's level does not exist for that
caller:

- in the descriptor: the field leaves the schemas with its `required` entry and its mentions in
  `ui.fields`, `ui.pages` and `ui.multi`; the column or action is not served, nor an action
  whose function or screen the caller cannot call, nor a column's inline `edit` whose function
  it cannot call; a descriptor the caller may not call at all answers `401` to a guest and
  `403` to a lower principal; and the descriptor's `ETag` includes the principal and its
  `acl_gen`, so a change of rights invalidates the cached copy (see
  [10-contracts.md](10-contracts.md));
- in a response: the field is cut from the row or object;
- in a request: sending it is an error, `403` with `code: forbidden_field` and the offending
  paths in `fields[]`, never a silent drop. A client that silently lost a field would believe
  the write succeeded as sent. A field sent as `null` counts as absent, so a client that
  round-trips an object it read does not have to know which fields were cut.

Validation errors from the same stage are `422` with `code: validation` and one entry per
field: `path`, `code` (`check.ip`, or a module key), the i18n `key` and `params`. A schema
violation is reported under the keyword that failed, `code: schema.<keyword>` with
`key: keel.schema.<keyword>` (`schema.required`, `schema.type`, `schema.minimum`); a `check`
preset applies to a string field and to every string item of an array field, at any depth of
the parameters, and fails with `code: check.<preset>`, `key: keel.check.<preset>`, the JSON
path of the value (`records[1].ip`) and the offending `value` in `params`. A field with
`checkBy { field, cases }` is checked with the preset its sibling's value selects; the freeze
refuses a `checkBy` naming no sibling, and a form checks such a field again when the sibling
changes. A
`secret` field is accepted on input and masked everywhere on output, including the audit record:
the kernel replaces its value with `***` in every response, in the record an `Idempotency-Key`
replays from and in the audit event, whatever type the handler returned it as. A `once` field is
the complement for a value the caller must see exactly one time: the kernel answers it in the
response of the call that produced it and replaces it with `***` in the same stored copies, so a
replay by `Idempotency-Key` answers the rest of the result with that value masked and no table
of the kernel ever holds it.

### Two kinds of hooks

| Hook     | Runs                        | Timeout | May                                                       | Open to                          |
|----------|-----------------------------|---------|-----------------------------------------------------------|----------------------------------|
| `before` | before the transaction opens | yes     | read through the pool, enrich `params`, veto with an error | modules and plugins              |
| `within` | inside the transaction       | no      | read and write on `call.tx()`, veto                        | modules compiled into the binary |

The split follows from what each side can be trusted with. A `before` hook may live in another
process and may be slow or dead, so it gets a timeout and no transaction; a plugin that vetoes
returns an error with an i18n key, parameters and optionally a field path, and the caller sees
it as a field error of the form. A hook that answers `Before::Enrich(value)` has the object
merged over the parameters, key by key; the enriched parameters are what the transaction, the
`within` hooks and the handler see, and they are not validated a second time. An enrichment
that is not an object is a bug in the hook, and the kernel ends the call with `internal`
(`keel.error.enrich_not_object`) naming the module rather than handing the handler a value the
schema never admitted. Plugins may hook only functions not above their own level and
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
an inner call. The inner function still passes the level, permission and ownership guards, the
strip and the validation of its own descriptor, and its in-process `before` and `within` hooks;
its outbox rows go into the shared transaction and its `after_commit` callbacks run with the
outer call's commit. A write function invoked from a read call is a programming error and
answers `internal`. A module reads another module's data only through `call.invoke` or a service
the other module provides; direct SQL across module prefixes is forbidden (see
[04-storage.md](04-storage.md)). The M0 acceptance list includes the test that a nested
`invoke` reuses the caller's transaction.

### Outbox

`call.emit(type, &data)` appends an event to `keel_outbox` inside the call's transaction. If the
transaction rolls back, the event never existed; if it commits, delivery is guaranteed at least
once to everything that reads the outbox: webhooks, peers, notifications, SSE
(`Last-Event-ID` is the outbox `seq`), plugins with the `observe` right and the `keel.patch.stored`
mechanism above. `call.emit_about(type, subject, &data)` is the same with an explicit subject
where the derived one would be wrong ([07-ecosystem.md](07-ecosystem.md)). Each consumer keeps
its own cursor, and retention never discards an event
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

Three details the mechanism fixes: the record is written in the call's transaction, so only a
committed response is ever replayed and a failed call leaves no record behind; a record expires
24 hours after it was written; and a guest carries no identity to key on, so a key sent with a
guest call is ignored. The dispatcher reports a replay next to the envelope, and the transport
turns it into the header.

`keel-client` retries only requests that carry a key; a billing peer opening a service twice
with the same key therefore gets one service (the M6 acceptance criterion). The end-to-end path
across products is finished in M6.

### Audit event

After commit or rollback the kernel raises an in-process audit event carrying who called
(principal and, under `su` or on-behalf, the actor), what, with which parameters — `secret`
fields masked — and how it ended. The event is raised for failed calls as well, which is the
reason it is not written inside the transaction: an audit row inside a rolled-back transaction
would vanish with it. A write call that fails at any stage after its name resolved — a guard, the
validation, a hook — is audited with that failure as its outcome; an idempotent replay executes
nothing and raises no event. The event reaches the `audit` module through the `AuditSink`
service it provides; without a sink the event is dropped. The `audit` module subscribes and
writes in batches from a background worker so that the call's response does not wait for the
audit write. For read functions the event is off by default. The event also carries the
`Client` of the request, the `subject` — the key parameter of a keyed function as text — and the
`result` of a committed form function that has a key, masked like the parameters with `once`
fields as well, so an object edit records what it left behind while the results of key-less
forms never reach the trail; the `before` snapshot of the object-history screen stays `NULL` at
M1, and both snapshots are described with the module in [05-modules.md](05-modules.md).

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
| `batch_threshold` | A group-edit or batch call with more ids than this is not executed inline: it becomes a task with progress and log, one call and one transaction per chunk, after the ownership guard admitted every id. The same member on `keel.export` is its row threshold: an export of more rows becomes a task whose result carries a `download` ([10-contracts.md](10-contracts.md)). Without a `TaskRunner` in the composition the threshold is inert and the batch runs inline. |

Globally: the body limit on a request, a platform constant of 1 MiB rather than a configuration
key, whose one exception is the multipart body of an `upload`, bounded by the sum of its file
fields' `maxBytes` plus 64 KiB for the `params` part; and `SSE_MAX_CLIENTS`, a platform constant
of 4 concurrent event streams per credential ([10-contracts.md](10-contracts.md)). A global limit
on the streams of the whole installation is not built; the per-credential cap bounds what one
login can hold.

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
