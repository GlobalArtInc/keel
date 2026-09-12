# Frontend

This document describes the browser side of Keel: the single Angular library `@globalart/keel` and
what each of its entry points contains, the prebuilt shell application a product embeds when it has
no screens of its own, the boundary between Keel and the phoenix-ui design system with the additions
requested from phoenix-ui per milestone, the registry of custom views and the ceiling placed on
their share, and the conventions for translation keys, placeholders and bundles. The wire
contracts the frontend consumes are specified in [10-contracts.md](10-contracts.md); descriptors,
field attributes and the `ui` block are specified in [02-functions.md](02-functions.md).

## Position in the architecture

The frontend is layer L6 of the dependency graph described in [01-overview.md](01-overview.md). It
consists of two npm packages: `@globalart/keel`, the library with entry points, and
`@globalart/keel-shell-app`, the prebuilt shell. Both are built on Angular 19 and phoenix-ui.
`@globalart/keel` is published to Nexus next to phoenix-ui; where the prebuilt shell is published
is an open question (see "Embedding and the prebuilt shell").

Two things are explicitly outside this layer:

- the design system itself. phoenix-ui is an external library with its own repository and release
  cycle; Keel consumes it, pins its version in `peerDependencies` and requests additions to it, but
  does not contain it;
- runtime loading of UI plugins. A plugin registers its functions, menu entries and dictionaries
  through the same registrar as a compiled module (see [05-modules.md](05-modules.md)), so the
  frontend, like the CLI, audit and license, does not distinguish a module from a plugin: plugin
  screens are rendered from descriptors by the generic renderers, and no code is ever loaded from a
  plugin into the browser.

The frontend is one client of the dispatcher among several: the CLI, the external API, plugins and
neighbouring products call the same `POST /api/v1/call/{func}` and read the same descriptors from
`/api/v1/functions`. The library carries no product knowledge; a product without custom screens
ships without a line of TypeScript of its own.

The major version of `@globalart/keel` equals the descriptor version: descriptors carry `"keel": 1`
and are served under `/api/v1`; a breaking change to the descriptor is published as `/api/v2` next
to `/api/v1`, and the library takes a new major with it (see the versioning planes in
[05-modules.md](05-modules.md)).

## One library, several entry points

`@globalart/keel` is one Angular library with secondary entry points. The entry points are
boundaries inside a single package: they keep `/meta` free of Angular and make the dependency
direction between the parts explicit, without paying the tax of publishing, versioning and
cross-bumping six to eight separate npm packages. The whole library is released with one tag.

| Entry point                | Contents                                                          | Angular |
|----------------------------|-------------------------------------------------------------------|---------|
| `@globalart/keel/meta`     | wire types, condition interpreter, client-side validation         | no      |
| `@globalart/keel/client`   | `call()`, list adapter, export, SSE, auth and idempotency, dates  | yes     |
| `@globalart/keel/forms`    | schema + `ui` to a reactive form; pages, conditions, wizard       | yes     |
| `@globalart/keel/renderer` | renderers by `kind`; view registry with generic fallback          | yes     |
| `@globalart/keel/shell`    | `bootstrapKeel()`, layout, navigation, login, theme, system screens | yes   |
| `@globalart/keel/testing`  | mock client, principal fixtures, Storybook decorators, Playwright | yes     |

### `/meta`

Everything the other entry points and any non-Angular consumer need to understand the wire format,
with no Angular dependency, so the same code is exercised under Jest against the fixtures shared
with Rust.

- TypeScript types for descriptors, the call envelope, errors, `ListQuery` and `Page`, the
  principal and events. They are generated from the JSON Schema of `keel-types` by
  `keel-dev types`, not written by hand. The closed vocabulary of the descriptor — `kind`,
  `control`, `column.type`, `action.role`, the `check` presets — arrives as TypeScript unions
  generated from the `keel-types` enums, so the renderer switches over a union rather than over
  free strings.
- The interpreter of the condition AST. Conditions such as `kind == 'slave' && !dnssec` are
  compiled by the server at start into a JSON AST with the operators `eq`, `ne`, `in`, `gt`, `lt`,
  `truthy`, `empty`, `matches`, `and`, `or`, `not`; the variables are form fields, `$mode`,
  `$principal.level` and `$screen.params.*`. The TypeScript interpreter is about 200 lines, mirrors
  the Rust one, and both run against `contracts/fixtures/expr-cases.json`. Field names used in
  conditions are validated by the server at start, so the client never meets a misspelled field.
- Client-side validation: a subset of JSON Schema (`type`, `required`, `min`, `max`, `pattern`,
  `enum`, `format`) plus the `check` presets (`domain`, `ip`, `cidr`, `email`, `int`, `name`,
  `port`, `path`, ...), verified by the same fixtures as the Rust implementation. Client validation
  is a convenience for the user; the server validates every call regardless and answers `422` with
  `fields[]`.

Two implementations of conditions, checks and validation (Rust and TypeScript) can drift. The
measures are the shared fixtures from M0 and a conformance test stating that a field is shown if
and only if the server accepts it (see [11-decisions.md](11-decisions.md), risks).

### `/client`

The HTTP and SSE client.

- `call(func, params)` performs `POST /api/v1/call/{func}` and returns the envelope: `ok`, `data`,
  `meta` with `requestId` and the effective principal, or `error` with `code`, `key`, `params`,
  `requestId` and `fields[]`. Error codes map to HTTP statuses as listed in
  [10-contracts.md](10-contracts.md). `GET` is permitted for `kind: list | get`.
- The list adapter turns a function name and a `scope` into a fetch callback for
  `PhxDataTableFetch`. `ListQuery` and `Page` are one type for every `kind: list` and coincide with
  the contract of `phx-data-table`: `page`, `pageSize`, `search`, a single `sort`, `filters[]` with
  `key`, `op`, `value`, and `scope`; the response carries `data`, `total` and optional `totals`.
- Export. `role: export` on a list is a toolbar button in the renderer; the client calls the generic
  `keel.export { func, query, columns, format }`, which streams every page through the same
  `ListSource` with the same scoping and stripping as the list itself. Above the row threshold the
  server turns the export into a task whose result is a download.
- SSE. One event stream with the events `task.progress`, `notification`, `data.invalidate` and
  `meta.invalidate`. `Last-Event-ID` is the outbox sequence number, so a reconnect catches up
  without a gap. The server filters the stream by `ScopePolicy` and principal: a `user` receives
  `data.invalidate` only for objects of their own account.
- Authentication and safety. `401` sends the shell to the login route. Cookie requests are checked
  by the server against `Sec-Fetch-Site` and `Origin`; `Bearer` requests are not, so the same client
  serves both the browser session and integration tokens. Writes may carry an `Idempotency-Key`;
  a retry with the same body is replayed with `X-Keel-Idempotent-Replay`, a retry with a different
  body gets `409 idempotency_mismatch`.
- Response headers. `X-Keel-Request-Id` is kept for error display and support; `X-Keel-Meta-Version`
  is watched on every response, and a changed value tells the shell that its descriptor cache is
  stale.
- Dates. The wire carries RFC3339 UTC; the client converts to `Date` using the principal's `tz` from
  `/boot`. Date filters are computed by the server with the day boundary taken in the principal's
  time zone.
- In-flight calls are counted into the global progress indicator of phoenix-ui
  (`PhxGlobalProgressService`).

### `/forms`

Turns the parameter schema and the `ui` block of a descriptor into an Angular reactive form on
`phx-form-sheet`, `phx-form-section` and `phx-form-row` with phoenix fields: input, password with
`generate`, select with remote options from a function, toggle, switcher, slider with `unlimited`,
date, time, file, list field, code field, tags. The `control` value from `x-ui` is a closed enum and
maps to a component; `x-ui` attributes (`control`, `placeholder`, `help`, `order`) are presentation
only and may be ignored without breaking anything, while the `x-keel` attributes (`level`, `secret`,
`check`, `options`) carry protocol semantics that the server enforces.

- Pages. `ui.page(...)` groups fields into pages or tabs; a page or a field may carry `show_if`,
  `required_if` or `readonly_if`, evaluated by the `/meta` interpreter with `$mode` set to the
  screen mode (for example `$mode == 'edit'` makes the zone name read-only).
- Level. A field whose `x-keel.level` is above the caller's level is already stripped from the
  descriptor the server serves, so the form never shows it; writing such a field anyway yields
  `403 forbidden_field` with the field path, which the form reports by path like a validation
  error.
- Secret. Fields marked `secret` use the password control, with generation when `x-ui.generate`
  is set; the same flag masks the value in audit, logs and the support bundle.
- Remote options. A field with `x-keel.options` names a `kind: options` function. The form calls it
  with `{ search, scope, limit, cursor }` and receives `{ data: [{ value, label, hint?, icon?,
  group? }], more }`, which is exactly the remote mode of `phx-select`.
- Recompute. A field marked `.recompute(<func>)` in `ui` triggers a call with `{ values, changed }`
  and applies the answer `{ values: patch, options: { field: [...] }, errors?: fields[] }`.
- Server errors. A `422` carries `fields[]` entries `{ path, code, key, params }`; the form maps
  them onto controls by path, including array indices such as `master_ips[1]`, and translates
  `key` with `params`. A `conflict` raised by a handler with `.field("name")` lands on that field
  the same way.
- Group editing. `ui.multi([...])` lists the fields allowed for editing several rows at once; the
  request is `{ ids: [], fields: { ttl: 300 }, mask: ["ttl"] }`. Above `batch_threshold` the call
  becomes a task.
- Wizard. A `kind: wizard` function walks its `pages`; `pages[].validate: <func>` checks a step on
  the server before the next one opens, and `ui.result` after the last step is `redirect`, `task`
  or `download`. The step container, next and back navigation and per-step validation live here;
  `phx-wizard` supplies only the step indicator.
- Custom controls. `provideKeelWidgets` registers product widgets such as money, CIDR or CPU/RAM
  pickers; they are handed to the shell through `bootstrapKeel({ widgets })`.

The same code renders a form from a schema that did not come from this product: the future billing
product draws the parameter form of a service from the `ServiceKind` schema published by a
neighbouring panel (see [07-ecosystem.md](07-ecosystem.md)).

### `/renderer`

Generic screens, one renderer per screen kind. The renderer set is not identical to the descriptor
`kind` enum of [02-functions.md](02-functions.md): `tabs`, `dashboard`, `tree` and `master-detail`
are layouts chosen by the descriptor's `ui` block, not kinds of their own.

| Milestone | Renderers                                                                    |
|-----------|------------------------------------------------------------------------------|
| M2        | `list`, `form`, `action`, `wizard`, `tabs`                                   |
| M5        | `report`, `dashboard`, `tree`, `master-detail`, `stream`                     |

The list renderer composes `phx-page-header`, `phx-data-table`, the selection bar, row actions,
child screens and export from the `ui` block: `search` fields; `columns` with a `type` from the
closed enum that maps explicitly onto `PhxColumn.type`, `sortable`, `primary`, `total`, `status`
maps, `filter` with static options or a remote options function, and inline `edit` through
`PhxCellEditService` for toggle, select and number cells; `actions` with a `role` from `new`,
`edit`, `delete`, `group`, `button`, `child`, `external`, `export`, `upload`, `download`, `refresh`,
`confirm` (plain or `{ destructive, items, input }`), `hidden_if`, `default`; `row_actions`; an
`empty` state with its action; `invalidate_on` patterns that trigger a refetch when a matching
`data.invalidate` event arrives. Column access is not decided in the browser: the descriptor is
already projected under the principal by the server, so a column or action above the caller's level
never reaches the renderer.

The report renderer maps `bands` to `phx-list-table` and diagrams to `phx-chart`; the dashboard
renderer lays widgets out on `phx-widget-grid` with the order kept in user preferences; the stream
renderer shows a log arriving over SSE.

Function results are handled uniformly: `ui.result = task` opens the progress modal driven by
`task.progress` events (M3), `redirect` navigates, `download` hands the file to the browser.

`provideKeelViews([{ func, component }])` registers a component for a function name; anything not
registered falls back to the generic renderer for its `kind`. The registry is a first-class
mechanism, not an escape hatch (see "Custom views" below).

### `/shell`

The application frame. A product application is a call to
`bootstrapKeel({ views, widgets, icons })` plus its custom components; `views` come from
`provideKeelViews`, `widgets` from `provideKeelWidgets`, and `icons` is the icon set made available
to the shell. Icon names used in descriptors and menus are checked by the server against
`PhxIconName` at start, so the shell never receives an unknown icon.

- `/boot` is the first request. It returns the product and edition, the brand, the principal with
  `lang`, `tz`, `actor` and `totp`, the enabled features, the license state, the menu already cut to
  the principal's level, the dashboard, the list of custom views, the meta version and the count
  of open problems. `phx-layout` and `phx-sidebar` are built from `menu`; a `badge` on a menu item
  is bound to a problem key.
- The rail carries notifications, tasks, help and the user menu.
- A banner is shown while the session runs as somebody else: `su` (a separate session with `actor`
  filled in, see [03-access.md](03-access.md)) or on-behalf from a neighbouring product.
- The router addresses every screen by function name: `/f/:func`. Objects open in tabs through
  `PhxTabsService`.
- Login screens on `phx-logon`: password (`auth.login` is a `guest`-level function and goes
  through the ordinary dispatcher pipeline: rate limits, audit and module hooks; plugin hooks on
  `auth.*` are refused at start, see [05-modules.md](05-modules.md)), TOTP, one-time login key,
  and creation of the first administrator. The login-key page at `/login?key=...` performs a
  same-origin `POST auth.key.redeem` itself, because a cross-site navigation would fail the
  `Sec-Fetch-Site` policy; the key is consumed on first use. The first-administrator screen is `keel.setup.admin`, a
  `guest` function that exists while the product has no administrator and is reached through the
  one-time `kok_` link printed at first start.
- The OwnLate to `PHX_LABELS` bridge feeds the labels phoenix-ui components need from the `phx.*`
  namespace of the translation bundles.
- The theme is derived from the brand: `hue` and `chroma` from `/boot.brand` go through
  `PhxThemeService`, the logo is served from `/brand/logo.svg`. White-label by hostname is a
  `ManifestPatch` on the server side, so it changes `/boot` and needs no product code in the
  browser.
- Metadata cache keyed by the meta version. Descriptor responses carry an `ETag` computed from the
  build, the module set, the edition, the language, the principal id and the principal's `acl_gen`,
  which is incremented in the same transaction that changes the principal's permissions, role or
  account (for a reseller subtree, by path). Responses are `private, no-store`; the shell keeps its
  own cache keyed by `/boot.meta` and `X-Keel-Meta-Version`, and drops it on `meta.invalidate` or
  when a response carries a different version. Without the principal in the key, `su` in the same
  browser would show another principal's menu.
- Screens of the system modules ship in the shell: settings, accounts, permissions, tasks,
  scheduler, notifications, problems, audit, logs, license, updates, backups, plugins, peers, brand
  and support. A product gets them by composing the modules, without writing any UI.
- From M3 the shell consumes SSE: the progress modal, the notification centre, scope-filtered
  `data.invalidate`, and catch-up by `Last-Event-ID`.

### `/testing`

- A mock client driven by a `keel-dev descriptors` dump, so screens can be developed and tested
  without a running binary.
- Principal fixtures of different levels, matching the descriptor snapshots per level produced by
  `keel-testkit`.
- Storybook decorators. Storybook for `/renderer` runs on descriptor fixtures and the mock client;
  it serves as documentation of the contract and as visual regression without a backend.
- A harness for Playwright against a built binary.

## Embedding and the prebuilt shell

The frontend is compiled into the product binary through `rust-embed`: assets are brotli-compressed
and served immutable by content hash (see [09-operations.md](09-operations.md)). A product declares
what it embeds in `main.rs`:

```
.frontend(keel::embed_frontend!("../../frontend/dist/dns"))
```

Two cases:

- A product with custom views has its own Angular application in the workspace: a `bootstrapKeel`
  call with its `views` and `widgets`, built to `frontend/dist/<product>` and embedded as above.
- A product without custom screens does not hold an Angular workspace at all. It points
  `embed_frontend!` at `@globalart/keel-shell-app`, the prebuilt distribution of the same shell with
  no custom views. When such a shell meets a view name it does not know, it shows `phx-not-found`
  with a hint.

The `frontend/` directory is a pnpm workspace: `libs/keel` with the entry points, `apps/shell-app`,
one application per product that has custom views, and Storybook.

How the prebuilt shell is distributed is an open question: an npm tarball in Nexus is proposed;
GitHub release assets are the alternative, but they do not pass the production proxy wall without
extra configuration (see [11-decisions.md](11-decisions.md)).

## Boundary with phoenix-ui

phoenix-ui receives only atomic components; every piece of logic driven by metadata lives in
`@globalart/keel`. The library as it stands today has gaps that matter for metadata-driven screens:
`phx-wizard` is only a step indicator, `phx-select` filters the options it was given locally, the
table has neither column persistence nor a bulk-action panel, and `phx-code-field` is a textarea.

Additions to phoenix-ui are delivered in batches tied to milestones:

| Milestone | Additions to phoenix-ui                                                            |
|-----------|-------------------------------------------------------------------------------------|
| M2        | `phx-selection-bar`; remote mode of `phx-select` (options from `searchChange`, `loading`, `more`); `phx-tags-field`; `phx-number-field` (stepper, unit, unlimited) |
| M3        | `phx-datetime-field`; `select` and `number` types in `PhxCellEditService`; `columnOrder` on `phx-list-table` |
| M5        | entry point `@globalart/phoenix-ui/code` (CodeMirror 6 with nginx, bind and ini modes); `phx-log-view` (tail, ANSI); `phx-key-value-field`; `phx-transfer-list`; time axis and tooltips on `phx-chart` |

What does not go into phoenix-ui, because it depends on descriptors, preferences or the call
protocol, is code of `/renderer` and `/forms`:

- the wizard step container with navigation and per-step server validation;
- persistence of column order and visibility in `settings_pref`;
- the link between table selection and the bulk-action panel;
- export.

What goes into neither is a custom view of the product: the file manager, the terminal (xterm) and
the permission matrix.

Eleven components in another repository sit on the critical path of forms already in M2. The
measures are batches bound to milestones, phoenix-ui versions pinned in `peerDependencies`, and the
rule that a gap is closed in phoenix-ui rather than worked around in the Keel runtime.

## Custom views

A custom view is a screen written by the product in TypeScript instead of being derived from a
descriptor. It is registered on both sides.

On the server a module registers the view without a screen descriptor but with access, a menu entry
and the features it needs, so the menu, the level check and the license guard treat it like any
function:

```
r.view("hosting.file-manager", ViewSpec { access, menu, needs })
```

`/boot.views` lists the registered views. On the client the product registers a component for the
name through `provideKeelViews([{ func, component }])`, and the component receives:

```
KeelViewContext { descriptor?, params, client, principal, tabs }
```

`descriptor` is optional: a view registered through `r.view` has no screen descriptor, while a
component that replaces the generic screen of a function through `provideKeelViews` (the M2
acceptance criterion replaces one screen of the demo product this way) receives that function's
descriptor. `params` are the screen parameters from the route, `client` is the `/client` instance,
`principal` the current principal, `tabs` the object-tab service. The prebuilt shell, which carries
no product components, shows `phx-not-found` with a hint for any view it meets.

### The ceiling

Custom views absorb everything metadata cannot express — the file manager, the configuration
editor, the terminal, monitoring — and their share is the measure of whether the
metadata approach is paying off. CI computes the share from the `r.view` registry as the ratio of
views to functions; the alarm threshold is 20 % of functions. Above it the economics of the
platform do not hold.

Measures:

- the metric is in CI from M2;
- registries of widgets and column types on both sides: the enums in `keel-types` on the server,
  `provideKeelWidgets` and the renderer's column types in the browser;
- the rule "a custom screen is cheaper than a new attribute": when a screen needs something the
  vocabulary lacks, the first answer is a custom view, not an extension of the descriptor;
- the `ui` vocabulary is reviewed once per milestone.

Milestone targets: at M5 `panel` has exactly two custom views (the file manager and the
configuration editor) and the share of custom views is at most 10 % of functions (see
[../plan.md](../plan.md)).

## Internationalisation

### Key convention

Translation keys are derived by convention from the registry, not written by hand: the function
builder knows the module, the entity, the fields, the actions, the pages and the columns, and the
key for each follows from that. `keel-dev i18n extract` collects the keys from the registry without
a manual list.

| Key                                   | Used for                                  |
|---------------------------------------|-------------------------------------------|
| `<module>.<entity>.<field>`           | field label                               |
| `<module>.<entity>.<field>.hint`      | field hint                                |
| `<module>.<entity>.<field>.placeholder` | field placeholder                       |
| `<module>.<entity>.<field>.<enum>`    | label of an enum value                    |
| `<module>.<entity>.action.<id>`       | action label                              |
| `<module>.<entity>.page.<id>`         | page or tab label                         |
| `<module>.<entity>.column.<key>`      | column header                             |
| `keel.*`                              | shared labels, inherited                  |
| `phx.*`                               | labels of phoenix-ui components (`PHX_LABELS`) |

Shared labels live under `keel.*` and are inherited: a field, column or action without a
module-specific key resolves to the shared key with the same tail, so common names such as `id`,
`name`, `status` or `created_at` are translated once. Keys that are not derivable are explicit in
the descriptor: `ui.title` (`dns.zone.list.title`), the empty state (`dns.zone.empty`), menu labels
(`menu.dns`), error keys (`dns.zone.error.exists`, `keel.error.validation`) and check keys
(`keel.check.ip`).

At registry freeze the server verifies that every key it will emit has a translation at least for
`en` (see the module lifecycle in [05-modules.md](05-modules.md)).

### Placeholders

Messages in the envelope are not translated by the server: it sends a key and `params`, and the
consumer formats them. Mail and notification templates are rendered on the server with minijinja.
Placeholders have one format, `{name}`, in minijinja templates, in CLI output and in TypeScript, so
a message key has a single translation that works in all three places.

### Bundles

A module embeds its dictionaries at compile time:

```
r.i18n("en", include_str!("../i18n/en.json")).i18n("ru", include_str!("../i18n/ru.json"));
```

A plugin ships its own files and names them in its manifest (`"i18n": { "en": "i18n/en.json",
"ru": "i18n/ru.json" }`). The server merges the dictionaries and serves them over `/api/v1/i18n`;
the shell picks the language from `principal.lang` in `/boot`, and the user's language lives in
`settings_pref`.

Translations are managed in OwnLate: one project for the core (system modules, the frontend
runtime, the `phx.*` keys) and one project per product. Bundles are pulled from OwnLate at build
time by `keel-dev i18n pull` and compiled into the binary, because the server a panel is installed
on may have no outbound access and cannot fetch translations at runtime. `keel-dev i18n missing`
compares the keys of the registry with the bundles and fails CI on any key without a translation.

`phx.*` is the namespace reserved for `PHX_LABELS`: the labels phoenix-ui components render
themselves (table chrome, pagination, dialogs) come through the same bundles and are bridged into
phoenix-ui by the shell.

## Frontend work by milestone

The full plan is in [../plan.md](../plan.md); the frontend-relevant parts are:

| Milestone | Frontend scope                                                                      |
|-----------|-------------------------------------------------------------------------------------|
| M2        | entry points `/meta`, `/client`, `/forms`, `/renderer` (list, form, action, wizard, tabs), `/shell`, `/testing`; `rust-embed`; Storybook with fixtures; phoenix-ui batch 1; OwnLate to `PHX_LABELS` bridge; publication to Nexus |
| M3        | SSE in the shell (progress modal, notification centre, scope filter, `Last-Event-ID`); `ui.result = task`; phoenix-ui batch 2 |
| M5        | renderers `report`, `dashboard`, `tree`, `master-detail`, `stream`; phoenix-ui batch 3; the file manager and configuration editor of `panel` as custom views |

M2 is done when the demo product is driven entirely from the browser without any product
TypeScript: a list with search, sorting, filters and bulk delete with confirmation; a form with
pages, `show_if`, recompute and a field-level error; a wizard; one screen replaced by a custom view
through `provideKeelViews`; `en` and `ru` switch; and the binary with the frontend inside is smaller
than 40 MB.
