# Wire contracts

This document fixes the shapes that cross a process boundary in Keel: where the contract is defined
and how it is verified, the function descriptor as the frontend receives it, the metadata version
and its caching rule, the call envelope with its error codes and the CSRF rule, the list contract
(`ListQuery` → `Page`) and its source trait, the request and response shapes behind form dynamics,
the compile-time shape of a module, a bundle and the composition, the plugin manifest and its JSON
Lines frames, the host agent protocol with `agent.yaml`, and the `GET /api/v1/boot` response. The
semantics behind each shape live in the sibling documents: the dispatcher and descriptors in
[02-functions.md](02-functions.md), levels and accounts in [03-access.md](03-access.md), modules
and plugins in [05-modules.md](05-modules.md), the host agent in [06-host.md](06-host.md), peers
and services in [07-ecosystem.md](07-ecosystem.md), and the frontend that consumes all of this in
[08-frontend.md](08-frontend.md).

## Where the contract lives

The wire contract is defined in `keel-types`, a crate without I/O. It carries `PROTOCOL = 1`, the
descriptor types, the envelope, `ListQuery` and `Page`, `Principal`, `Account` and `Level`, the
condition AST and the `check` presets. Golden fixtures are checked by `cargo test` on the Rust side
and by Jest on the TypeScript side, so the two implementations cannot drift without a red build.
From the JSON Schemas of `keel-types`, written to `contracts/schema/` by `keel-dev types`, the
frontend build generates the TypeScript types of `@globalart/keel/meta`; the OpenAPI description
used by clients in other languages, `contracts/openapi.json`, is written by `keel-dev openapi`
from the admin dump of the running binary, one `POST /api/v1/call/<func>` per descriptor with the
envelope around its result schema, and checked in CI by `hack/descriptors.sh`
(see [05-modules.md](05-modules.md)).

Three fixture sets are shared between Rust and TypeScript from M0:

- descriptor snapshots per level: the kernel's own under `contracts/fixtures/descriptors/`, a
  module's under its tests, and from M2 the projected contract of the real binary for `guest`,
  `user`, `reseller` and `admin` under `contracts/dump/`, written and compared by
  `keel-dev descriptors dump | check | diff` through `hack/descriptors.sh`;
- condition cases in `contracts/fixtures/expr-cases.json` for the ~200-line interpreters on each
  side;
- `check` preset cases (`domain`, `ip`, `cidr`, `email`, `int`, `name`, `port`, `path`, ...).

`keel-conformance` runs the same contract against the started `keel` binary with any set of
products enabled — boot, auth, the list contract, errors, idempotency, webhooks, `svc.*` — and
is a mandatory CI job, run with each first-party bundle alone and with all of them. The three
versioning planes (compile-time `keel-sdk`, wire `PROTOCOL`, data `"keel": 1`) and the N / N-1
rule for `PROTOCOL` are stated in [01-overview.md](01-overview.md); `PROTOCOL` is reported in
`GET /api/v1/instance` and in the plugin manifest.

### Endpoint map

The routes this design names. Anything else the installation serves is a raw route under
`/api/v1/x/<module>/…`: today the kernel's projection of every `download` and `upload` function
and the `Endpoint` routes a module contributes, which authenticate on their own and may answer a
guest ([05-modules.md](05-modules.md)); later a route a module registers with a mandatory guard and
an audit event ([02-functions.md](02-functions.md)).

| Route | Purpose |
|-------|---------|
| `GET /healthz`, `GET /readyz` | liveness, `{ "status": "ok" }` as soon as the listener is bound; readiness answers `{ "status": "ready" }` once the database answers `SELECT 1`, the migrations are applied and the process is not draining, else `503` with `migrating`, `draining` or `database` ([09-operations.md](09-operations.md)) |
| `GET /api/v1/boot` | the first request of the frontend: keel version, enabled products, effective brand, principal, menu, features, dashboard, metadata version |
| `GET /api/v1/functions`, `GET /api/v1/functions/{func}` | descriptors, projected under the calling principal |
| `POST /api/v1/call/{func}` | the call; `GET` is accepted for `kind: list \| get` |
| `GET /api/v1/i18n?lang=` | the message catalogue of one language, `en` filling every gap |
| `GET /api/v1/instance` | instance identity for peers: `protocol`, the instance public key, enabled products, service kinds ([07-ecosystem.md](07-ecosystem.md)) |
| `/api/v1/x/<module>/<name>` | raw routes: `GET` or `POST` for a `download` function and `POST` multipart for an `upload`, `<name>` being the function name without its module prefix, behind authentication, the cross-site check and the whole pipeline; `GET` for a `stream` function, answering `text/event-stream`; `GET` or `POST` for an `Endpoint` a module contributes, which receives the body, the query and the headers without the credential and runs neither the guards nor the audit of the pipeline, so it authenticates on its own and may answer a guest: the peers hook `POST /api/v1/x/peer/hook`, authenticated by the HMAC signature of the delivery, and the pull feed `GET /api/v1/x/peer/feed?after=seq`, behind the peer token of a link; later the WebSocket terminal and VNC |
| `GET /api/v1/events` | the server-sent event stream, `text/event-stream` with `Cache-Control: no-store`, resumed by `Last-Event-ID` or the query member `after` |
| `GET /brand/logo.svg`, `GET /brand/<id>/logo.svg` | the effective brand's logo and each compiled-in bundle's, served from the bundle's `assets` or the kernel's own mark, `no-store` ([08-frontend.md](08-frontend.md)) |
| `/login?key=…` | the page that redeems a one-time login key same-origin ([07-ecosystem.md](07-ecosystem.md)) |
| everything else | the embedded shell: an asset by name, or `index.html` for a path whose last segment carries no extension the shell build emits ([09-operations.md](09-operations.md)) |

The server-sent event stream carries `task.progress` (a `TaskProgress` frame: the job id, its
state, `done`, `total` and a message), `notification` (a `NotificationFrame`: the message row as
`notify.message.list` shows it), `data.invalidate` (the event in its common shape, `id:` the
outbox `seq`) and `meta.invalidate` (`{ meta }`, the principal's current metadata version). A
stream opens with `retry: 3000` and sends a keep-alive comment every fifteen seconds; a draining
process closes it with a final `retry:` so the browser reconnects to the next process.
`Last-Event-ID`, or the query member `after` when the header is absent, replays the events after
that `seq` within a window of the newest 1000 rows of the outbox; an id older than the window,
ahead of the table or unreadable is answered with one `meta.invalidate` and the stream goes live
from the newest `seq`, so the client refetches rather than misses something. Events are filtered
per connection by the account scope of the principal: a `data.invalidate` is sent when the
event's `subject.accountId` lies within the scope, an event without one reaches admins alone, so
a user receives `data.invalidate` only for its own objects; an event another installation emitted,
whose account ids name nothing here, reaches only an admin or owner whose credential is not
narrowed by a scope of functions or a call through a peer ([07-ecosystem.md](07-ecosystem.md)); a
live frame reaches its own principal, a reseller whose subtree holds its account and every admin,
never another user. A guest is answered `401` as an envelope. A credential holds at most `SSE_MAX_CLIENTS`, 4, streams at
once, keyed by a SHA-256 of the session cookie or the bearer token, so a token is capped like a
session and a stream under `su` counts against the session that holds it; the stream beyond the
cap evicts the oldest stream of that credential, which ends with a final frame carrying
`retry: 0` and the comment `superseded`, and a stream that closes by itself frees its slot. At
every keep-alive the stream runs the authenticator chain again with its credential: an
unauthenticated answer closes it without a final frame, so a logout, a revoked session or an
expired token loses its stream within one keep-alive period, any other error is logged and the
stream stays, and the principal the check answers rescopes the stream. An open stream keeps its
session from expiring idle. The peers feed is a separate path, not the browser stream.

### Conventions shared by every shape

- Identifiers are ULIDs serialised as text: stable for peers, import and export, sortable by time.
- Time is RFC 3339 in UTC. The principal's time zone comes from `settings_pref`, is reported in
  `/boot` and reaches a handler as `Call::tz()`, UTC without a preference and on the socket.
  Date filters compute day boundaries in that zone: in the `Owned` list source of
  `keel_sdk::list`, a filter on a timestamp column whose value is a bare calendar date
  `YYYY-MM-DD` resolves to the day's boundaries in the caller's zone — `gte` and `lt` at the
  day's start, `gt` and `lte` at the next day's start, `eq` the day as a range and `ne` its
  complement — while a full RFC 3339 value is compared as the instant it names, and a module with
  its own `ListSource` reads `call.tz()` for the same; the client converts to `Date` by the zone.
- Every human-readable string is an i18n key, never a sentence: `title`, `label`, `error.key`,
  `fields[].key`. Placeholders have one format, `{name}`, in minijinja, the CLI and TypeScript.
- Function names are `<module>.<entity>.<verb>`, or `<module>.<verb>` for a module with a single
  entity such as the demo's `note.list`; the descriptor carries `"keel": 1`, the data version of
  the descriptor format.
- Member names of every wire row are camelCase (`parentId`, `accountId`, `externalId`,
  `createdAt`, `updatedAt`, `aclGen`), like the shared shapes (`pageSize`, `instanceId`,
  `requestId`), whatever the snake_case column behind them; the column keys of `ui.columns` and
  the i18n keys derived from them follow the wire name (`keel.account.column.parentId`). The
  parameters of a form are named by the module's parameter type and need not mirror the row:
  the account forms take `parent` and the principal forms `account`, while the rows they read
  back carry `parentId` and `accountId`.
- The vocabulary is closed by enums in `keel-types` and generated as TypeScript unions: `kind` ∈
  `list · get · form · action · wizard · report · options · stream · download · upload`;
  `control`; `column.type` (with an explicit mapping onto `PhxColumn.type`); `action.role` ∈
  `new · edit · delete · group · button · child · external · export · upload · download ·
  refresh`; the `check` presets. Icons are validated at start against the icon set of
  phoenix-ui, embedded from `contracts/icons.json`, which `pnpm generate` writes from
  `PHX_ICON_NAMES` ([08-frontend.md](08-frontend.md)).

## Function descriptor: `GET /api/v1/functions/dns.zone.list`

The descriptor is returned already projected under the calling principal: columns and actions
whose `access` is above the caller's level or outside the caller's permissions are cut out, not
delivered and hidden. The projection goes further than `access` members: a field whose
`x-keel.level` is above the caller leaves the parameter and result schemas together with its
`required` entry, and with it every mention in `ui.fields`, `ui.pages` and `ui.multi`; an action
whose `function` or `screen` the caller may not call is cut, and so is its entry in
`row_actions` and in `empty.action`; a column's inline `edit` is dropped when its function is
not callable. A descriptor of a function above the caller is not served at all: a guest asking
for it receives `401 unauthenticated`, a lower principal `403 forbidden`, and the list under
`/api/v1/functions` carries only what the caller may call. The three parts of a descriptor —
the parameter schema, the `x-keel` / `x-ui` field attributes inside it, and the separate `ui`
block — are explained in [02-functions.md](02-functions.md). Conditions (`hidden_if`,
`show_if`, `required_if`, `readonly_if`) are delivered as the compiled JSON AST, never as the
source string.

```json
{ "keel": 1, "name": "dns.zone.list", "module": "dns", "kind": "list", "mode": "read",
  "access": { "level": "user", "permission": "dns.zone.read", "feature": "dns.zones" }, "key": "id",
  "params": { "$ref": "keel://ListQuery" }, "result": { "title": "Page_ZoneRow", "type": "object" },
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
| `params`, `result` | JSON Schema; `keel://ListQuery` is the shared list query, and the result of a list is the page schema, titled `Page_<Row>` after the Rust row type, with `data`, `total` and `totals`; the example abbreviates its body |
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

- `ETag = hash(build, modules, products, lang, principal_id, acl_gen)`, rendered as
  `m-<16 hex>` and quoted as an entity tag; a request whose `If-None-Match` names it is answered
  `304`;
- `build` is the platform version, the one version of the binary and of every product on it;
  `modules` is the loaded module set with versions and `products` the enabled product ids, with
  their editions once a lease exists, so enabling a product or changing its edition changes the
  key;
- `acl_gen` is a column of `keel_principal`, incremented in the same transaction that changes the
  principal's permissions, role, level or account, and on any change of an ancestor account;
  for a reseller's subtree the increment is applied by `path`;
- responses are `Cache-Control: private, no-store`;
- the event stream's `meta.invalidate` tells the client to re-request.

The same value is exposed as `X-Keel-Meta-Version` on call responses and as `meta` in `/boot`,
which carries it as its `ETag` too; the shell keys its metadata cache by it
([08-frontend.md](08-frontend.md)). The catalogue under `/api/v1/i18n` carries the same hash
computed without a principal, so it changes with the loaded set and the language only. The
principal is part of the hash on purpose: a metadata ETag that ignored the principal while the
body depended on the caller's rights would violate cache semantics — after `auth.su` in the same
browser the client would show the delegated user another principal's menu. See
[11-decisions.md](11-decisions.md).

## Call and envelope: `POST /api/v1/call/{func}`

One shape for every transport: HTTP, the local socket, plugins and peers. `GET` is accepted for
`kind: list | get`.

```http
POST /api/v1/call/dns.zone.edit
Authorization: Bearer kat_01J9…          (or Cookie: __Host-keel_session=ks_…)
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

CLI:  keel ctl dns.zone.edit id=01J9Q3… ttl=300 --as alice --out json     (same envelope; --out table renders by ui.columns)
```

The request side follows these rules:

- A `POST` body is JSON; an empty body is `{}`. A body above the platform's body limit, 1 MiB,
  answers `422 validation` with `keel.error.body_too_large` and the limit in `params`; a body
  that is not JSON answers `keel.error.body_not_json`. The limit is a kernel constant, not a
  configuration key; its one exception is the multipart body of an `upload`, bounded by the sum
  of its file fields' `maxBytes` plus 64 KiB for the `params` part.
- A `GET` carries the parameters in the query string, either as one `q` member holding a JSON
  object (`?q={"search":"www","page":1}`) or as `key=value` pairs, where a value that parses as
  JSON is taken as that value and any other value as a string. A `q` that is not JSON or not an
  object answers `422 validation` with `keel.error.query_not_json` or
  `keel.error.query_not_object`. A `GET` on a function whose kind is not `list` or `get` answers
  `422 validation` with `keel.error.method_not_allowed` and an `Allow: POST` header.
- `X-Keel-Request-Id` sent by the client is kept as the request id when it is plain — at most
  64 characters of `[A-Za-z0-9._-]` — and replaced by a generated one otherwise; the response
  always carries the effective value.
- Until the kernel has booted, every route under `/api/v1/` answers `503 unavailable`
  (`keel.error.unavailable`) while `/healthz` already answers and `/readyz` reports
  `migrating` ([09-operations.md](09-operations.md)).

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
| `error.fields[]` | field-addressed errors: `path` is a JSON path into the parameters (`master_ips[1]`), plus `code`, `key`, `params` for each; a `check` preset fails with `code: check.<preset>` and `key: keel.check.<preset>`, a JSON Schema keyword with `keel.schema.<keyword>`; a handler error attached with `.field("name")` lands here with its own key, and the envelope's `code` is then `validation` whatever the handler's own code was — this is how `resolve_account` surfaces an account outside the caller's scope: `422 validation` with the field `account` keyed `keel.error.account_not_found` |

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

- Browser sessions use the cookie `__Host-keel_session`, one for the installation, carrying an
  opaque `ks_` token; API tokens are `kat_` Bearer tokens, peer tokens `kpt_`, one-time login keys
  `kok_` ([03-access.md](03-access.md)).
- Every API request that does not carry a Bearer token is checked by `Sec-Fetch-Site` /
  `Origin`: a request with `__Host-keel_session`, and a request with no credential at all, even
  when it carries other cookies; a Bearer request is not checked. The check reaches requests
  without a credential because a guest form such as `auth.login` plants the session cookie, so a
  cross-site post to it is refused exactly like a cookie call. A request is refused as
  `403 forbidden` with `keel.error.cross_site` when `Sec-Fetch-Site` says `cross-site`, or when
  `Origin` is present and its authority differs from the `Host` header, or from the request
  authority when there is no `Host` — from `X-Forwarded-Host` when the peer is a trusted proxy
  ([09-operations.md](09-operations.md)); a request that carries neither `Sec-Fetch-Site` nor
  `Origin` is admitted. This is why the one-time login key is redeemed by a same-origin page at
  `/login?key=…` that posts `auth.key.redeem`, instead of the key being accepted on a cross-site
  navigation ([07-ecosystem.md](07-ecosystem.md)).
- The `__Host-` prefix requires TLS, so the installation listens only on TLS from the first start
  ([09-operations.md](09-operations.md)).

### Idempotency

`Idempotency-Key` is handled by the kernel, not by handlers. A repeat with the same key and the
same body is a replay of the stored response marked with `X-Keel-Idempotent-Replay`; the same key
with a different body is `409 idempotency_mismatch`. Keys are stored in `keel_idempotency`
([04-storage.md](04-storage.md)); `keel-client` retries only calls that carry a key.

### CLI and the control socket

`keel ctl <func> …` goes through the local socket with peer-credential authentication and
receives the same envelope; `--out json` prints it, `--out table` renders `data` by `ui.columns`.
Whether parameters are accepted as `a.b=v` with repeated keys and `@file.json`, or only as JSON,
is an open question ([11-decisions.md](11-decisions.md)). What M0 fixes:

- The socket speaks JSON Lines: one frame per line in, one envelope per line out, in order. A
  frame is one of `{ "func", "params"?, "requestId"?, "idempotencyKey"?, "as"? }` (a call, where
  `as` names the login of a stored principal the frame runs as, with the owner as `actor` and
  `via: socket`, `not_found` with `keel.error.principal_not_found` for an unknown login),
  `{ "describe": "<func>" }` (the descriptor projected for the caller), `{ "i18n": "<lang>" }`
  (the catalogue) and `{ "boot": true }` (the `/boot` payload); only the call frame takes `as`.
  A JSON line of another shape answers `422 validation` with `keel.error.frame_unknown`; a line
  that is not JSON answers `keel.error.body_not_json`. A line is limited to the same 1 MiB as an
  HTTP body, measured without the delimiter. The idempotent replay marker is an HTTP header and
  has no counterpart on the socket.
- The socket file is created with mode `0600` in a directory the kernel creates when missing; a
  stale file no process answers on is replaced, a file that answers makes the start fail. Only
  the kernel's own uid is admitted, checked with `SO_PEERCRED`; every frame without `as` runs
  as the owner principal: nil id, login `owner`, level `owner`, the root account, `via: socket`.
- `keel ctl <func> key=value …` builds the parameters from the pairs: a value that parses as
  JSON is that value (`ttl=300`, `ids=["a","b"]`, `dnssec=true`), anything else is a string, and
  the last of repeated keys wins. `--out json` prints the envelope; `--out table` renders a
  `kind: list` result as a grid whose headers are `<module>.<entity>.column.<key>` translated
  through the catalogue in `--lang` (`en` by default), an object as two columns of member and
  value, an error as `error: <code> <key> <params>` with one line per field entry and the
  request id. `--as <login>` sends the frame's `as`; `--socket <path>` overrides the socket
  named by the configuration; the global `--config <path>` or `KEEL_CONFIG` names the
  configuration file.
- Exit codes: `0` for `ok: true`, `1` for an error envelope or a failure to reach the socket,
  `2` for a usage error.

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
`null` when the function declares none; the kernel computes them over the rows of the returned
page, after the strip, so a column the caller cannot see has no total either. `page_size` is
capped by the function's `max_page_size`, and a missing `sort` falls back to `default_sort`
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
never contains a column or a row the caller could not see on screen. `func` must name a list the
caller may call, `columns` an optional subset of the caller's projected columns, `query` loses
`page` and `pageSize`; `format` is `csv` (RFC 4180, a header row of the translated column
labels) or `json` (one array), the file named `<func>-<date>.<ext>` in the caller's zone. Above a
row threshold, `batch_threshold(10000)` on the function read against the total of the first
page, the call becomes a task through `Call::spawn_self_as_task` and the raw route answers the
`TaskRef` envelope in place of the stream; the job writes the file to its output and its result
carries `DownloadRef { download: { function: "task.download", params: { id } } }`, from which
the shell fetches the file ([02-functions.md](02-functions.md)). The `export` action in a list
descriptor names the offered `formats`.

### Reports, stat widgets, trees and streams

The screen kinds and layouts M5 rendered in full take these shapes on the wire:

| Shape | Request | Response |
|-------|---------|----------|
| `kind: report` | the report's `params` schema, rendered by the forms library as the parameters form above the bands; Apply refetches with the form's body, which the route does not keep | `{ bands: { <band id>: [rows] }, charts: { <chart id>: { params } } }`. `ui.bands[]` are tables: `id`, columns in the column vocabulary of lists with a `total` per column, whose footer the renderer computes from the rows since a report is not paged, and the export `formats`; `ui.charts[]` are `{ id, type: line \| bar \| pie, series, params }`. A chart is fetched through its `series` function, a registered `get`, with the descriptor's `params` merged with the ones the answer gives under `charts.<id>`, so the report resolves defaults such as the period and the series function keeps its own guards. The projection cuts band columns above the caller and a chart whose series the caller may not call; labels are `<entity>.band.<id>` and `<entity>.chart.<id>` |
| series function | its own parameters | `{ points: [{ at \| label, value }] }`, which `stats.series` answers; a point at a UTC midnight is labelled as that day in UTC, any other instant in the principal's zone |
| `keel.export` of a band | `{ func, band, params, format }` naming a report | the band's rows in the projected band columns, from one call of the report and never a task; a missing band answers `keel.error.export_band` on `band`, an answer without its rows `keel.error.export_report` |
| stat widget | the widget's `get` function without parameters | `StatValue { value, unit, series?, subject? }`: `series` is the stats key of the sparkline, drawn from `stats.series` at hour resolution over the last seven days, and `subject` its account for a per-account key; the freeze refuses a `stat` widget over anything but a `get` and a second widget over one function, so the function name is the widget's id in `dashboard.layout`, which `settings.pref.set` stores and refuses unless it holds `order` and `hidden` lists of at most 64 names each (`settings.error.pref_dashboard`) |
| `ui.layout: "tree"` | the list function with `scope.<parent>: null` for the top of the caller's reach and `scope.<parent>: <id>` for the children of a node, 200 rows per level; `ui.parent` names the column that holds the parent id | the list page; a node is expandable until a load answers no children. `keel.account.list` takes it over `parentId` and refuses any other value of `scope.parentId` with `keel.account.error.parent_scope` |
| `ui.layout: "master-detail"` | `ui.detail` links the `get` or `list` function shown for the selected row, `$id` standing for the row's key | the selection lives in the query parameter `selected`, left out of the parameters the descriptor is resolved with; `dns.zone.list` takes it with `dns.record.list` over `zone_id` |
| `kind: stream` | `GET` on the raw route with the function's parameters | `text/event-stream`: an event `line` per `LogLine { at?, level, text }`, keep-alive comments, and a final event `end` whose data names the reason: `done`, `expired`, `superseded` or `draining`. A credential holds its own `SSE_MAX_CLIENTS` function streams beside the event stream, so a log tab never evicts the shell's; at every keep-alive the credential is resolved again and the stream ends when it no longer authenticates or answers another principal or level. `keel.logs.stream` sends the last 200 entries of the log ring and follows it |

The freeze refuses a layout on a function that is not a list, a `parent` that is not one of its
columns and a `detail` that is unregistered or neither `get` nor `list`, and the projection drops
the layout with its `parent` or `detail` when the caller does not see the column or may not call
the detail, so a user gets the plain list.

## Form dynamics

The shapes behind interactive forms. Which fields trigger which mechanism is declared in the `ui`
block by the builder (`.field("ttl").recompute("dns.zone.recompute")`, `.multi(["ttl"])`,
`x-keel.options`), so the client needs no product code.

| Mechanism | Request | Response |
|-----------|---------|----------|
| `kind: options` | `OptionsQuery { search, scope, limit, cursor }`; `limit` defaults to 20 | `Choices { data: [Choice { value, label, hint?, icon?, group? }], more }`, `value` a string — the source for the remote mode of `phx-select` and for `filter.remote` on columns; the three types live in `keel-types` from M0 because `keel.account.options` needs them |
| `recompute` | `{ values, changed }` | `{ values: patch, options: { field: [...] }, errors?: fields[] }`; which fields trigger it is declared in `ui`; the helper is registered as `kind: action` in `mode: read` and unowned, since the vocabulary has no kind for a helper without a screen |
| group editing | `{ ids: [], fields: { ttl: 300 }, mask: ["ttl"] }` to the form function itself, told apart from its single-row body by `ids` and the absent key | `GroupResult { updated }`; `ui.multi` lists the fields that may be edited across several rows; the ownership guard admits `ids` element by element, `mask` is checked against `ui.multi` (`keel.error.multi_field` at `mask[i]`) and `fields` against `mask` (`keel.error.multi_unmasked` at `fields.<name>`); inside one transaction every row is read through `ui.read`, the masked fields are laid over it and the form runs per row as a nested invoke, so one failing row rolls the group back; above `batch_threshold` the call becomes a task answering `TaskRef` |
| `kind: wizard` | `pages[].validate: <func>` validates a step on the server before the next page opens, with the values of the pages so far as its parameters; the helper is a `kind: action` in `mode: read` like `recompute` | `ui.result` after the final step: `redirect · task · download` |
| `result: task` | any call whose screen says `result: task`, or a batch above `batch_threshold` | `TaskRef { task: { id } }` after the guards, strip, validation, idempotency and the `before` hooks ran; the job runs the same function with the enriched parameters under the caller's principal, and its `task.get` result carries `DownloadRef` when the job stored a file |
| `download · upload · stream` | a `download` takes its parameters as a call does, at `GET` or `POST` on its raw route; an `upload` takes `multipart/form-data` on its raw route: a `params` part with the JSON of the other fields and one part per field whose control is `file`, checked against `x-keel.file { maxBytes, types }` while it is read (`keel.error.file_type`, `keel.error.file_too_large` on the field's path) and placed into the parameters as the text of a string field or as base64 for a field whose schema says `contentEncoding: base64`; SSE frames for `stream` | a `download` streams the body with `Content-Type`, `Content-Disposition` and `Cache-Control: no-store`, `{ filename, contentType }` being its result in the pipeline, or answers `TaskRef` when the handler became a job; an `upload` answers the form's envelope and carries a `Form` block like a `form`; the call route answers `422 keel.error.download_route` or `keel.error.upload_route` naming the raw route, a guest is refused on the raw route before dispatching, and every such call raises the audit event whatever its mode |

`ui.result = task` opens the progress modal driven by `task.progress` events
([08-frontend.md](08-frontend.md)). `recompute` returns a patch, not the whole value set, so the
client merges it over what the user has typed; `options` in its response refreshes the choices of
dependent fields; `errors` uses the same `fields[]` shape as the envelope.

## Module, bundle and composition

The compile-time contract of the SDK: a module is a crate exporting `pub static MODULE: ModuleDef`,
a product is a lib crate exporting `pub static MANIFEST: ProductManifest`, and the composition is
the `main.rs` of `bin/keel`, which builds a `keel::Runtime` listing every platform module and
every bundle explicitly; a bundle names its modules by definition, so they are not listed twice.
The kernel's own module `keel` is composed first by the loader and is never listed. The kernel
orders modules by `requires`, not by the order of `.module()` calls, and loads only the platform
modules, the modules of the bundles enabled on the installation and the chosen side of every
bridge; a module a bundle names that requires a module outside the loaded set fails the start
naming the module and the product. The lifecycle and the registration surface are specified in
[05-modules.md](05-modules.md).

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
    r.permission("dns.zone.read", &[Level::User, Level::Reseller]).permission("dns.zone.manage", &[Level::User, Level::Reseller]);
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

// products/dns/src/lib.rs
pub static MANIFEST: ProductManifest = ProductManifest {
    id: "dns",
    name: "product.dns.brand",
    version: env!("CARGO_PKG_VERSION"),
    api: keel::types::DESCRIPTOR_VERSION,
    brand: Brand { logo: "product.dns.logo", hue: 264, chroma: 0.12 },
    modules: &[&keel_host::MODULE, &dns_zones::MODULE, &dns_records::MODULE, &dns_templates::MODULE],
    menu: &[MenuGroup { id: "dns", label: "menu.dns", icon: "globe", order: 10 }],
    levels: LevelLabels { labels: &[(Level::User, "product.dns.level.user")], disabled: &[] },
    editions: &[
        Edition { id: "lite", features: &["dns.zones"] },
        Edition { id: "pro", features: &["dns.zones", "dns.dnssec", "dns.secondary"] },
    ],
    account_kinds: &["user"],
    i18n: &[("en", include_str!("../i18n/en.json")), ("ru", include_str!("../i18n/ru.json"))],
    assets: &[Asset { id: "product.dns.logo", content_type: "image/svg+xml", bytes: include_bytes!("../brand/logo.svg") }],
};

// bin/keel/src/main.rs
fn main() -> keel::ExitCode {
    keel::Runtime::new()
        .module(&keel_system::auth::MODULE).module(&keel_system::settings::MODULE).module(&keel_system::audit::MODULE)
        .module(&keel_system::tasks::MODULE).module(&keel_system::cron::MODULE).module(&keel_system::notify::MODULE)
        .module(&keel_system::license::MODULE).module(&keel_system::update::MODULE).module(&keel_system::backup::MODULE)
        .module(&keel_system::peers::MODULE)
        .product(&keel_dns::MANIFEST).product(&keel_panel::MANIFEST)
        .frontend(keel::embed_frontend!("../../frontend/dist/shell"))
        .run()
}
```

`init` only registers; it performs no I/O. Everything a module can register — functions, streams,
raw routes, views, migrations, menu nodes, i18n catalogues, permission keys, hooks, descriptor
patches, services and contributions — is listed in [05-modules.md](05-modules.md). The
composition above is abbreviated; the full list of system modules, the bundle fields, the
`bridges` field the `panel` bundle adds and the `demo` feature are in the example at the end of
that document. Since M1 `main.rs` composes `auth`, `settings` and `audit` and the `demo` bundle
behind the `demo` feature.

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

The privileged process of the installation speaks JSON Lines with the kernel over
`/run/keel/agent.sock`; the socket accepts only the uid `keel`, checked with `SO_PEERCRED`. The
same protocol later runs over mTLS gRPC to a remote node without the module knowing which it
talks to ([06-host.md](06-host.md)).

```jsonl
→ {"id":1,"op":"host.template.apply","actor":{"login":"alice","level":"user"},"request_id":"req-1a2b","product":"dns",
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
| `product` | the product that owns the calling module (the bundle that names it); the agent applies that product's section of the policy |
| `args` | the operation's arguments, validated against the schema of the operation |
| `dest` | a resource identifier — a named root from `file_roots` plus a name — never a raw path; the agent resolves and validates it under its policy |
| `validate`, `reload` | the check to run on the rendered file and the service to reload; a check that needs the file in place, `nginx -t` over the whole configuration, runs after the swap, so a failed check restores the previous content, mode and owner, or removes a file that did not exist, and returns `host.error.validate_failed` with the check's output as `detail`; a failed reload restores the file, reloads again and returns `host.error.reload_failed` ([06-host.md](06-host.md)) |
| `ok`, `data` / `error` | `error.key` is an i18n key that surfaces unchanged in the call envelope; `error.detail` is the raw output of the failed step |

The policy the agent enforces is `agent.yaml`:

```yaml
// agent.yaml
policy:
  file_roots: { zones: /etc/bind/zones, vhosts: /etc/nginx/conf.d, www: /var/www }
  services: [nginx, php8.3-fpm, named]
  checks: { nginx.test: ["nginx", "-t"] }
  exec_allow: [certbot, quotatool]
  products:
    panel: { user_prefix: hp_, drop_in_prefix: keel-panel- }
    dns:   { drop_in_prefix: keel-dns- }
```

| Key | Meaning |
|-----|---------|
| `file_roots` | the named roots `dest.root` may refer to; a file operation outside them is rejected; every operation is written to journald with actor and request_id |
| `services` | the units `host.service.*` may act on |
| `checks` | named validation commands referenced by `validate` |
| `exec_allow` | the only binaries `host.exec` may run, with fixed arguments |
| `products` | one section per product, keyed by product id and selected by `product` in the request: `user_prefix` is the prefix of the operating-system users that product's modules may create and manage, `drop_in_prefix` the prefix of its own files in include directories, and a product never edits other files; the release ships a section for every first-party product, so enabling a product does not edit the policy |

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
{ "keel": { "version": "1.2.0", "api": 1 },
  "products": [
    { "id": "dns", "name": "product.dns.brand", "edition": "pro",
      "brand": { "name": "product.dns.brand", "logo": "/brand/dns/logo.svg",
                 "hue": 264, "chroma": 0.12 } },
    { "id": "panel", "name": "product.panel.brand", "edition": "lite",
      "brand": { "name": "product.panel.brand", "logo": "/brand/panel/logo.svg",
                 "hue": 200, "chroma": 0.10 } } ],
  "brand": { "name": "keel.brand", "hue": 264, "chroma": 0.18 },
  "principal": { "id": "01J8…", "login": "res1", "level": "reseller", "account": "01J8A…",
                 "lang": "ru", "tz": "Europe/Belgrade", "actor": null, "totp": true },
  "features": ["dns.zones", "dns.dnssec", "panel.users", "panel.web"],
  "license": { "state": "valid", "expiresAt": "2026-10-12T00:00:00Z",
               "graceEndsAt": "2026-10-26T00:00:00Z" },
  "menu": [
    { "id": "dns", "product": "dns", "label": "menu.dns", "icon": "globe",
      "items": [ { "path": "dns/zones", "label": "dns.zone.list.title", "func": "dns.zone.list",
                   "kind": "list", "icon": "globe", "badge": { "problem": "dns.zone_stale" } } ] },
    { "id": "panel", "product": "panel", "label": "menu.panel", "icon": "server",
      "items": [ { "path": "panel/sites", "label": "hosting.site.list.title",
                   "func": "hosting.site.list", "kind": "list" },
                 { "path": "panel/files", "label": "hosting.file-manager.title",
                   "func": "hosting.file-manager" } ] },
    { "id": "system", "product": null, "label": "menu.system", "icon": "settings",
      "items": [ { "path": "system/accounts", "label": "keel.account.list.title", "func": "keel.account.list", "kind": "list", "icon": "users" },
                 { "path": "system/principals", "label": "keel.principal.list.title", "func": "keel.principal.list", "kind": "list", "icon": "user" },
                 { "path": "system/tasks", "label": "task.list.title", "func": "task.list", "kind": "list" },
                 { "path": "system/products", "label": "keel.product.list.title", "func": "keel.product.list", "kind": "list", "icon": "package" } ] } ],
  "dashboard": [ { "kind": "stat", "func": "dns.widget.zones" },
                 { "kind": "list", "func": "notify.problem.list", "limit": 5 },
                 { "kind": "stat", "func": "keel.widget.accounts", "hidden": true } ],
  "views": ["hosting.file-manager", "hosting.site.config-editor"], "meta": "m-7f3a2c1b9d4e0a62", "problems": { "open": 1, "critical": 0 } }
```

| Field | Meaning |
|-------|---------|
| `keel` | version of the installation, which is the version of every product on it, and the API version |
| `products` | the enabled products in composition order: `id`, `name` (the brand name key), `edition` and the product's own `brand`, with its logo at `/brand/<id>/logo.svg`; `edition` is the licensed edition once a lease exists and the bundle's default edition, or `null` for a bundle without editions, while none does |
| `brand` | the effective brand: the single enabled product's brand with its logo at `/brand/logo.svg`, otherwise the platform brand `keel.brand` with hue 264, chroma 0.18 and no `logo` member; brand name key, logo path and the theme hue and chroma the shell feeds to `PhxThemeService`; patched per hostname for white-label with the `ManifestPatch` the `ManifestSource` answers for the request's `Host` (`settings_brand`, [05-modules.md](05-modules.md)), every member optional, a patched `name` being literal text rather than a key; the socket boot carries no host and is never patched |
| `principal` | the effective principal with language and time zone from `settings_pref`, the `actor` when delegated (`su`, on-behalf) and whether TOTP is enrolled; `null` for a guest. `tz` is `null` while the principal has no `tz` preference and on the socket; `totp` is `false` for the owner and while no module provides a `SecondFactorSource`; a non-guest principal whose `actor` arrived without delegation is answered with `500 internal` and `keel.error.actor_not_delegated`, as a call would be |
| `features` | licence features of the enabled products' editions, as one list — the union of the default editions while no lease exists; a function whose `access.feature` is absent is hidden from the menu and answers `402` |
| `license` | `BootLicense { state, expiresAt, graceEndsAt }`: `state` is `none`, `valid`, `grace` or `read_only`, the two instants are `null` without a lease; a product the lease does not list is reported as the problem `license.product_unlisted` ([05-modules.md](05-modules.md), `license`); `null` in a composition without the `license` module |
| `menu` | groups of the enabled bundles and the platform group in that order, each with `product` (`null` for the platform), `label` and `icon`, and items projected under the principal's level and permissions as `{ path, label, func, kind?, icon?, badge? }`: `path` is the shell route, `label` the node's own key or the screen title of `func`, `func` the function it opens, `kind` its screen kind, absent on the item of a view registered through `r.view`, which has no descriptor, `badge.problem` a problem key whose open count is shown on the item; a group without a visible item is omitted |
| `dashboard` | widgets, `BootWidget { kind, func, limit?, hidden? }`, one per `DashboardWidget` contribution whose level the principal meets and whose function it may call: the ones the principal's preference `dashboard.layout { order, hidden }` names first in its order, the rest in registry order, `hidden` set on those the layout hides so the shell can offer them back; names the layout carries that no longer exist are ignored. A `stat` widget's function is a `get` answering `StatValue` (below); empty without a contribution, so `/` redirects to the first menu item. The layout is not part of the metadata version |
| `views` | the bespoke views registered with `r.view(...)` whose level and permission the principal meets, in registry order; the shell shows `phx-not-found` for one without a registered component; filled from M2 |
| `meta` | the metadata version, equal to `X-Keel-Meta-Version`, also served as the response's `ETag` with `private, no-store` and `304` on a matching `If-None-Match`; `principal.totp` and `principal.actor` are not part of the version, so the shell refetches `/boot` without `If-None-Match` after a call that changes the principal's own state, such as a second-factor enrolment |
| `problems` | `BootProblems { open, critical }`, the open problems within the principal's account scope, counted through the hidden `ProblemCounter`; `null` without a module providing it |

Labels in `menu`, `brand.name` and `products[].brand.name` are i18n keys resolved by the client
from the catalogues served under `/api/v1/i18n`. Over the control socket the same payload is
answered to the frame `{ "boot": true }` as the owner, as the `data` of an envelope.
`contracts/fixtures/boot.json` is the golden fixture of this shape as M0 serves it:
`edition: null`, no lease, no dashboard, no views; `contracts/fixtures/boot-none.json` is the
payload without a product, written from the kernel's own test, and `contracts/dump/<level>/boot.json`
the payload of the real binary with the `demo` bundle enabled per level. The three feed the
shell's Storybook stories of the brand rule as `KEEL_BOOT` of `@globalart/keel/testing`.

## `GET /api/v1/i18n`

`GET /api/v1/i18n?lang=<code>` answers the call envelope whose `data` is
`{ "lang": "<code>", "messages": { … } }`: the merged dictionaries of the loaded modules and the
enabled bundles for that language as one nested object, with every key missing in the language
filled from `en`; a language nobody ships falls back to `en` altogether, and `lang` names the
language actually served; without `lang` the principal's language is served. The response
carries the catalogue's `ETag` (the metadata hash without a principal), `private, no-store`, and
`304` on a matching `If-None-Match`. The same catalogue answers the socket frame
`{ "i18n": "<code>" }` as the `data` of an envelope, which is how `keel ctl --out table`
translates its headers. Unlike this route, `GET /api/v1/boot` and `GET /api/v1/functions` answer
their payload bare, without the envelope.
