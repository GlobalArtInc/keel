# Frontend

This document describes the browser side of Keel: the single Angular library `@globalart/keel` and
what each of its entry points contains, the one shell application the `keel` binary embeds, the
boundary between Keel and the phoenix-ui design system with the additions requested from phoenix-ui
per milestone, the registry of custom views and the ceiling placed on their share, and the
conventions for translation keys, placeholders and bundles. The wire contracts the frontend
consumes are specified in [10-contracts.md](10-contracts.md); descriptors, field attributes and the
`ui` block are specified in [02-functions.md](02-functions.md).

## Position in the architecture

The frontend is layer L6 of the dependency graph described in [01-overview.md](01-overview.md). It
consists of the npm library `@globalart/keel` with its entry points and of one shell application,
`frontend/apps/shell`, whose build the `keel` binary embeds. Both are built on Angular 19 and
phoenix-ui. `@globalart/keel` is published to Astrum next to phoenix-ui; the shell application is
not published, it is built in the repository and embedded (see "Embedding").

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
has no TypeScript of its own.

The major version of `@globalart/keel` equals the descriptor version: descriptors carry `"keel": 1`
and are served under `/api/v1`; a breaking change to the descriptor is published as `/api/v2` next
to `/api/v1`, and the library takes a new major with it (see the versioning planes in
[05-modules.md](05-modules.md)). The rule applies from SDK 1.0 in M7; until then the `0.x`
releases of the library carry the binary's version, stamped from the workspace version of
`Cargo.toml` by `tools/stamp.mjs` before `publish-npm.yml` publishes them, so the library and the
binary a release ships are one number.

## One library, several entry points

`@globalart/keel` is one Angular library with secondary entry points. The entry points are
boundaries inside a single package: they keep `/meta` free of Angular and make the dependency
direction between the parts explicit, without paying the tax of publishing, versioning and
cross-bumping six to eight separate npm packages. The whole library is released with one tag.

| Entry point                | Contents                                                          | Angular |
|----------------------------|-------------------------------------------------------------------|---------|
| `@globalart/keel/meta`     | wire types, condition interpreter, client-side validation         | no      |
| `@globalart/keel/client`   | `call()`, list adapter, download and upload, SSE, notifications and problems, auth and idempotency, dates | yes  |
| `@globalart/keel/forms`    | schema + `ui` to a reactive form; pages, conditions, wizard       | yes     |
| `@globalart/keel/renderer` | renderers by `kind`, the object renderer; view registry with generic fallback | yes |
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
- Export. `role: export` on a list is a toolbar dropdown of the action's `formats` in the
  renderer, `csv` when it names none; the client posts the generic `keel.export { func, query,
  columns, format }` to its raw route through `download(func, params)`, with the table's current
  search, sort, filters and scope as the query and the visible columns in their order. A file is
  handed to the browser; a `TaskRef` envelope on the raw route, the server's answer above the row
  threshold, opens the progress modal, whose result carries the `download` the client then
  fetches; an error envelope becomes a toast. A composition without `keel.export` hides the
  button, since the function is absent from the descriptor index. `upload(func, params, files)`
  posts the multipart body of an `upload` function to its raw route, a `params` part and one part
  per file field, with the `Idempotency-Key` in the header ([10-contracts.md](10-contracts.md)).
- SSE. `KeelEvents` opens `GET /api/v1/events` with credentials once `/boot` answers a principal,
  closes it when the boot loses one and reopens it when the principal changes, so the stream
  always belongs to the principal the shell shows. The four frames — `task.progress`,
  `notification`, `data.invalidate`, `meta.invalidate` — are typed from the wire types and a
  frame that does not parse is dropped and counted. The last event id is kept from every frame;
  the browser resends it as `Last-Event-ID` on its own reconnects, and a reconnect the client
  makes after a closed stream, with a doubling backoff, opens `?after=<id>`, which the server
  reads like the header. `meta.invalidate` drops the descriptor cache and revalidates `/boot`
  with `If-None-Match`, the same path a changed `X-Keel-Meta-Version` takes; `data.invalidate`
  reaches the lists through `invalidations()`, debounced at 250 ms so a burst of frames on one
  list is one refetch. The server filters the stream by the principal's account scope: a `user`
  receives `data.invalidate` only for objects of their own account.
- Notifications and problems. `KeelNotifications` keeps the messages and the unread count over
  `notify.message.list`, prepends `notification` frames and marks through `notify.message.mark`;
  `KeelProblems` keeps the open problems by key over `notify.problem.list`, refreshed on a
  `notify.problem.*` invalidation, from which the sidebar fills its badges.
- Authentication and safety. A `401` with `keel.error.unauthenticated`, the transport's answer
  when no credential is valid, sends the shell to the login route with the screen it came from;
  a `401` a function answers on its own, such as `auth.error.credentials` for a wrong current
  password on the security page, is returned as its envelope and shown in place. Every request
  without a `Bearer` token, the cookie session and the guest login form alike, is checked by the
  server against `Sec-Fetch-Site` and `Origin`; `Bearer` requests are not, so the same client
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
`once`, `check`, `options`) carry protocol semantics that the server enforces.

The control mapping is one table in `mapping.ts`, from the closed `control` enum onto phoenix
components, with the schema deciding what the descriptor leaves open:

| `control` | Component | From the schema |
|-----------|-----------|-----------------|
| `input` | `phx-input` | `type` `email` or `url` from `format`, `text` otherwise |
| `password` | `phx-password` | forced by `x-keel.secret` whatever the control says; `generate` from `x-ui.generate` |
| `select` | `phx-select` | the `enum` as local choices, or the remote mode when `x-keel.options` names a function |
| `toggle` | `phx-toggle` | |
| `switcher` | `phx-switcher` | the `enum` as choices |
| `slider` | `phx-slider` | `minimum` and `maximum`, 0 and 100 without them; `unlimited` for a nullable field |
| `date` | `phx-date-field` | the value stays the wire's `YYYY-MM-DD` |
| `time` | `phx-time-field` | |
| `file` | `phx-file-field` | |
| `list` | `phx-list-field` | the columns from the properties of `items` |
| `code` | `phx-code-editor` of `@globalart/phoenix-ui/code`, inside a `@defer (on immediate)` block so CodeMirror stays in its own lazy chunk | the language stays plain: the vocabulary has no attribute for it, and the configuration editor of `panel` is a custom view that sets its own |
| `key_value` | `phx-key-value-field` | an object; the key pattern from `propertyNames.pattern`, the field keeping the control invalid while a key is empty, refused or repeated; an untouched map is submitted as `{}` |
| `transfer` | `phx-transfer-list` | an array of ids, its items from the choices a recompute served, else from the options function of `x-keel.options` (the first page of 20, loaded once and searched locally), else from the `enum` of the items |
| `tags` | `phx-tags-field` | |
| none | inferred | a `boolean` is a toggle; an `enum` or `x-keel.options` a select; an `integer` or `number` a `phx-number-field` with the bounds, `unlimited` for a nullable field; a `string` with `format: date` a date; an array of strings the tags field; anything else an input |

- Pages. `ui.page(...)` groups fields into pages or tabs; a page or a field may carry `show_if`,
  `required_if` or `readonly_if`, evaluated by the `/meta` interpreter with `$mode` set to the
  screen mode (for example `$mode == 'edit'` makes the zone name read-only).
- Level. A field whose `x-keel.level` is above the caller's level is already stripped from the
  descriptor the server serves, so the form never shows it; writing such a field anyway yields
  `403 forbidden_field` with the field path, which the form reports by path like a validation
  error.
- Secret. Fields marked `secret` use the password control, with generation when `x-ui.generate`
  is set; the same flag masks the value in audit, logs and the support bundle.
- Once. A result field marked `once` is shown by the form the one time the call answers it, with
  a copy control, since no replay and no later read answers it again.
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
  becomes a task. The multi-row form of `/renderer`, `KeelMultiFormComponent`, is opened from the
  selection bar when the list's edit action names a form with a non-empty `ui.multi`: one row
  per field with a toggle that is the mask and the field's own control, built from the descriptor
  with its pages and field conditions stripped; a `422` at `mask[i]` lands on the i-th masked
  field, at `fields.<name>` on its field, a `TaskRef` opens the progress modal and a
  `GroupResult` closes the modal, toasts the count and reloads the table.
- File control. `control: file` renders `phx-file-field` with `accept` from `x-keel.file.types`
  and `maxSize` from `maxBytes`; a form whose descriptor is of kind `upload` submits through the
  client's `upload`, the envelope handled as a submit's.
- Scoped labels. A nullable number field says "from the product" when empty through the
  convention `<module>.<entity>.<field>.empty`: the field component provides `PHX_LABELS` at its
  element injector with `unlimited` replaced by that key when the catalogue has it, by
  `phx.unlimited` otherwise.
- Wizard. A `kind: wizard` function walks its `pages`; `pages[].validate: <func>` checks a step on
  the server before the next one opens, and `ui.result` after the last step is `redirect`, `task`
  or `download`. The form model of the wizard is this entry point's; the step container, next and
  back navigation and per-step validation are the wizard renderer of `/renderer`, since the
  container navigates after the last step; `phx-wizard` supplies only the step indicator.
- Custom controls. `provideKeelWidgets` registers product widgets such as money, CIDR or CPU/RAM
  pickers; they are handed to the shell through `bootstrapKeel({ widgets })`.

The same code renders a form from a schema that did not come from this product: the future billing
product draws the parameter form of a service from the `ServiceKind` schema published by a
neighbouring installation (see [07-ecosystem.md](07-ecosystem.md)).

### `/renderer`

Generic screens, one renderer per screen kind. The renderer set is not identical to the descriptor
`kind` enum of [02-functions.md](02-functions.md): `tabs`, `dashboard`, `tree` and `master-detail`
are layouts chosen by the descriptor's `ui` block, not kinds of their own.

| Milestone | Renderers                                                                    |
|-----------|------------------------------------------------------------------------------|
| M2        | `list`, `form`, `action`, `wizard`, `tabs`, and the object renderer of a `get` reached through a link or a row action |
| M3        | the `list`-widget slice of `dashboard`: the home screen at `/` renders one card per `list` widget of `/boot.dashboard` with the first rows of the list renderer in a compact mode and a link to the screen, a route without a function, so it counts for neither `views.json` nor the custom-views share; the multi-row form; the progress modal following a `TaskRef` |
| M5        | `report`, `dashboard` in full (`stat` widgets, widget order), `tree`, `master-detail`, `stream` |

The list renderer composes `phx-page-header`, `phx-data-table` as one sheet, the toolbar pill,
filter chips, row actions, child screens and export (with `keel.export`) from the `ui` block: `search` fields; `columns` with a
`type` from the closed enum that maps explicitly onto `PhxColumn.type`, `sortable`, `primary`,
`total`, `status` maps, `filter` with static options or a remote options function, and inline `edit`
through the toggle the table renders and the cell editor of phoenix-ui for `status` (a select over
the filter options and the map keys), `number` and `text` cells, the other types with `edit` staying
read-only; the column order is bound to the preference `columns.<function>`, read through
`settings.pref.get` at load and written on a header drag or a change of the column menu as
`{ order, hidden }`, the older `{ order }` form being read as before, a composition without
`settings.pref.*` keeping the declared order; `actions` with a `role` from `new`, `edit`,
`delete`, `group`, `button`, `child`, `external`, `export`, `upload`, `download`, `refresh`,
`confirm` (plain or `{ destructive, items, input }`), `hidden_if`, `default`; `row_actions`; an
`empty` state with its action; `invalidate_on` patterns that trigger a refetch when a matching
`data.invalidate` event arrives. Column access is not decided in the browser: the descriptor is
already projected under the principal by the server, so a column or action above the caller's level
never reaches the renderer. A list opened with a `search` screen parameter takes it as the search
text of its table and keeps it out of the scope, which is how a custom view opens a list narrowed to
a value; the name is reserved for lists, being the member of `ListQuery` a scope key never carries.

The actions of a list sit in the header, not in a bar over the table. A round search button
(shown when `ui.search` is not empty) and a round sort button over the sortable columns come first,
then one segmented pill: the action with the role `new` is the primary segment (when several
actions carry it, every one comes first and only the first is primary), then the actions that make
sense over a selection (the roles `edit`, `delete`, `group`, `button`, `child`, `download`, and
every row action or bulk action with a `confirm`), icon-only when the role has an icon, then the
remaining actions (`upload`, `external`, `refresh`) with a label, with a divider between the
groups. A single-row action is enabled when exactly one row is selected and its `hidden_if` does
not hide it for that row; a bulk action is enabled when at least one row is selected. The `export`
formats, "Refresh" and "Copy link" live in the header's "⋮" menu; each export format is a menu
item, so the menu carries no `data-format` attributes. The selection bar stays in `tree` only. The
`data-action` attribute moves onto the pill segment, so the end-to-end specs find an action by it.

Active column filters show as chips under the actions (`chipsOf` in `list/chips.ts` maps the
filters and the static options of a column onto chips; a removed option shows its stored value);
removing a chip drops the column's filter, the reset chip clears all of them, and a round reset
button with the count of active filters appears in front of the pill only while a filter is
active. The table renders as one sheet (`appearance="sheet"`) with the column menu: a column the
user hides is written to the preference of the function, the primary column and the last visible
column cannot be hidden, and the export still carries every column.

A root list, the one at depth 0 of the screen tree (`KEEL_SCREEN_DEPTH`), keeps its search, sort,
filters and page in the workspace tab it was opened in (`KeelWorkspace.listState` and
`saveListState`, reached through the `KEEL_WORKSPACE` token so the renderer never imports the
shell). A `search` screen parameter wins over the saved search and the saved sort, filters and page
are then left out. Scroll and selection are not saved. A refresh of the tab strip refetches a root
list.

The column mapping is `COLUMN_TYPES` in `columns.ts`: `text`, `number`, `link`, `status` and
`toggle` map onto the `PhxColumn.type` of the same name; `date` onto `date`, formatted in UTC so
the calendar day stays as written; `datetime` onto `date`, formatted in the principal's zone and
language; `boolean` onto `icon`, labelled through the shared `keel.value.true` and
`keel.value.false`. A `status` value is mapped through `map` onto the tone and labelled through
`<module>.<entity>.<column>.<value>`, a `link` column opens its target as an object tab through
`PhxTabsService`, and a column with a `total` shows the page's footer aggregate.

The object renderer shows a `get` from its result schema on `phx-prop-list`: the properties in
schema order, nested objects flattened to dotted paths, a date-time in the principal's language
and zone, a plain date as the wire text, a boolean through the shared value labels, an enum through
`<module>.<entity>.<field>.<value>`, an absent value as a dash and anything without a shape as
JSON. A row is labelled by `<module>.<entity>.<path>` and, when that key has no entry, by the
column key `<module>.<entity>.column.<path>`, since a `get`'s result mirrors the row of its list
and the dictionaries already carry the column headers.

The report renderer draws the report's parameters form above the bands, maps each band to
`phx-list-table` with footer totals computed from its rows and each chart to `phx-chart` fed by
its series function; Apply refetches the report and the charts follow the new answer, and the
applied parameters are not written to the route. The home screen lays widgets out on
`phx-widget-grid`: a `stat` widget shows its number with the unit (`keel.shell.home.unit.<unit>`)
and a sparkline the shell draws as a small SVG of its own from `stats.series`, since `phx-chart`
has axes and grid lines and no sparkline form, and a missing series leaves the number alone; a
drag reorders the widgets and the page's menu hides or restores them, both written to the
preference `dashboard.layout` through `settings.pref.set`, after which the shell refetches `/boot`
without `If-None-Match`. `keel-tree` loads one level at a time through the list function and
draws `phx-tree` with the primary column as the node label, the list's toolbar actions above it
and the row actions of the chosen node in a selection bar; `keel-master-detail` shows the list in
a picking mode, where a row click selects instead of running the default action, beside the
detail through `keel-screen`, the selection kept in the query parameter `selected` when the
screen is routed. The stream renderer opens the raw route through `KeelClient.stream`
(`EventSource`), shows the lines in `phx-log-view` with pause and a level filter, and closes on
the `end` event or an error instead of letting the browser reconnect and replay the backlog,
offering a reconnect button. The wire shapes are in [10-contracts.md](10-contracts.md).

Function results are handled uniformly: a `TaskRef` answered by an action, a form or a wizard
opens the progress modal, `keel-progress` over `phx-progress-modal`, driven by the `task.progress`
frames of the job through `KeelTasks.follow`, with `done/total` as its value and the frame's
message translated as its status; cancel posts `task.cancel`, done fetches the job through
`task.get` and hands the result back to the opener, a result carrying `download` fetches the raw
route and hands the file to the browser, failed shows the error with the request id on a toast, a
modal that misses frames for five seconds polls `task.get`, and a details button hands the job to
the task drawer without giving up the follow; `redirect` navigates, `download` hands the file to
the browser.

The role `external`, named in the vocabulary since M2, is an action whose function answers `{ url
}`, as `hosting.site.open_zone` answers the address the zone of a site opens at
([07-ecosystem.md](07-ecosystem.md)): the action runner calls the function and navigates inside the
shell to a path starting with a single `/`, leaves for an `http` or `https` address in the same tab
through the injection token `KEEL_OPEN_EXTERNAL`, since a window opened after an asynchronous call
is blocked by browsers, and on any other answer opens nothing and shows `keel.error.target`; a
refusal shows its toast as any action's. A key held in a column or a param, such as the `zoneError`
of a site whose zone the neighbour refused, is shown as text, as the column `error` of a failed
apply is; translating it is left to the shell.

`provideKeelViews([{ func, component }])` registers a component for a function name; anything not
registered falls back to the generic renderer for its `kind`. The registry is a first-class
mechanism, not an escape hatch (see "Custom views" below).

### `/shell`

The application frame. The shell application is a call to
`bootstrapKeel({ products, icons, widgets })`: `products` lists the frontend libraries of the
first-party products that have custom views as `{ id, load }`, where `load()` is the dynamic
import that becomes a chunk of its own and answers the providers of the library, its `views` (from
`provideKeelViews`) and `widgets` (from `provideKeelWidgets`); `icons` is the icon set made
available to the shell and `widgets` the shell's own. After `/boot` the shell loads the chunks of
the products `/boot.products` names, and of those alone, in parallel with the catalogue, so a chunk
that does not load fails the startup as a boot that does not answer would; their providers land on
the `/f/:func` route, visible to the screens and to nothing else. Icon names used in descriptors
and menus are checked by the server at start against the icon set of phoenix-ui, embedded from
`contracts/icons.json`, so the shell never receives an unknown icon.

- `/boot` is the first request. It returns the `keel` version and API, the enabled products with
  their brands and editions, the effective brand, the principal with `lang`, `tz`, `actor` and
  `totp`, the enabled features, the license state, the menu already cut to the principal's level
  with a `product` on every group, the dashboard, the list of custom views, the meta version and
  the count of open problems. With exactly one product enabled the effective brand is that
  product's brand and `phx-sidebar` shows its groups followed by the platform groups `access`, `monitor` and `system`;
  with several, the effective brand is the platform brand and the sidebar shows a heading per
  enabled product, in the order of `/boot.products`, with that product's groups under it and
  the platform groups last; a heading with no visible item is not shown. With none, the shell
  shows the platform brand and the platform group only. `phx-layout` and `phx-sidebar` are
  built from `menu`; a `badge` on a menu item is bound to a problem key and shows the count of
  open problems with that key from `KeelProblems`. A product's heading and a
  group's label are the one heading primitive phoenix-ui has, the eyebrow of `phx-sidebar-group`,
  nested one inside the other. The level label is `product.<id>.level.<level>` while one product
  is enabled and `keel.level.<level>` otherwise, a product key without a translation falling back
  to the platform key.
- The workspace of open pages is `KeelWorkspace` over `PhxTabsService`, the source of truth of the
  tab strip (`phx-tab-bar`). Every `NavigationEnd` on `/f/:func` opens or activates a tab whose id
  is the path and the query parameters sorted by key, without `selected` (that is the master-detail
  choice, not another page); the full last URL, `selected` included, is the tab's `url`. The root
  `/` with widgets is a pinned `home` tab, and the login, logout and unknown routes, the modals and
  the drawers create none. A tab is titled by its menu entry (the function name when the menu has
  none), and an object tab by the label `KeelNavigation.object` gives it; a title the screen
  resolves later is not applied yet. A page opened from another page
  (`KeelNavigation.screen` or `.object`: a child action, a link, a row action) carries one level of
  breadcrumb, the opener's title, announced through `expectChild` just before the navigation; a
  page opened from the menu, the address bar or the history carries none. Closing the active tab
  activates its neighbour, and the last one leads to `/`; "close others" and "close all" keep the
  pinned tab. At most 20 unpinned tabs stay open: the 21st evicts the unpinned inactive tab used
  longest ago, with its saved list state. The state lives in
  `localStorage['keel.workspace.v1.' + principal.id]` (a principal acting as another has its own),
  survives a logout, and is read defensively: a value that is not JSON, of another version or of
  another shape is an empty workspace. On restore the tab of the current URL becomes the active
  one. Nothing is synchronised between browser tabs and nothing is sent to the server. Below 640 px
  the strip is hidden and the page header detaches from it (`workspace.visible()` is false); the
  navigation goes through the menu. Only the list renderer is refreshed by the strip's refresh
  button for now; `tree`, `report` and `object` follow later.
- The sidebar is compact (`density="compact"`, 200 px wide, the rail 56 px). In an installation of
  one product the group captions are not printed (the groups keep their accessible names); with
  several products each product keeps its heading.
- The rail carries notifications, tasks, help and the user menu: the login and level, the language
  switch between the languages the catalogue serves, writing `settings.pref.set { key: "lang" }`
  and reloading the catalogue in place; the theme switch on `phx-theme-settings`, writing
  `settings.pref.set { key: "theme" }`; "Act as", a dialog searching `keel.principal.list` by login
  and posting `auth.su`, offered while `auth.su` is in the descriptor index; the links to the
  sessions, tokens and security screens; and the logout. The three items are alive: tasks
  carries the count of queued and running jobs and opens the task drawer, `keel-task-drawer` over
  `phx-drawer`, listing the recent jobs of `task.list`, a job opening its detail with the prop
  list, the progress bar, the log tail and cancel while it runs; notifications carries the
  unread count and opens the notification centre, `phx-notification-center` over
  `notify.message.list` and the `notification` frames, with a problems section for a principal
  who may list problems, a selected message marked read and a message naming a problem key
  opening `notify.problem.list` searching it; help opens the about drawer over `/boot.keel` and
  `/boot.products`, the smallest target before the support screen of `panel` in M5. An item
  whose function the descriptor index lacks is disabled.
- The home screen at `/` renders the widgets `/boot.dashboard` names, one `phx-widget-card` per
  `list` widget with the first rows of the list renderer and a link to its screen; without a
  widget `/` redirects to the first menu item.
- A banner is shown while the session runs as somebody else: `su` (a separate session with `actor`
  filled in, see [03-access.md](03-access.md)) or on-behalf from a neighbouring product. It lives
  in the page area above the screen, where it shows on every width, and offers `auth.su.exit`;
  `phx-topbar` is the phone chrome of `phx-layout` and has no slot for it.
- The router addresses every screen by function name: `/f/:func`. Objects open in tabs through
  `PhxTabsService`.
- Login screens on `phx-logon`: password (`auth.login` is a `guest`-level function and goes
  through the ordinary dispatcher pipeline: rate limits, audit and module hooks; plugin hooks on
  `auth.*` are refused at start, see [05-modules.md](05-modules.md)), TOTP, one-time login key,
  and creation of the first administrator. The login-key page at `/login?key=...` performs a
  same-origin `POST auth.key.redeem` itself, because a cross-site navigation would fail the
  `Sec-Fetch-Site` policy; the key is consumed on first use. The first-administrator screen is
  `keel.setup.admin`, a `guest` function that exists while the installation has no administrator
  and is reached through the one-time `kok_` link printed at first start.
- The OwnLate to `PHX_LABELS` bridge feeds the labels phoenix-ui components need from the `phx.*`
  namespace of the translation bundles: the shell provides `PHX_LABELS` with a live object that
  reads `phx.*` of the loaded catalogue, so a language switch changes the labels in place without
  re-providing them, which a static value provider at the root could not do after bootstrap; the
  kernel's dictionaries carry every key of phoenix-ui's default labels in `en` and `ru`.
- The theme is derived from the brand: `hue` and `chroma` from `/boot.brand` go through
  `PhxThemeService`, the effective logo is served from `/brand/logo.svg` and each product's from
  `/brand/<product>/logo.svg`. Every product of the line and the platform brand carry the palette
  of the platform applications, hue 264 and OKLCH chroma 0.18 — the defaults of phoenix-ui — so an
  installation looks the same whichever products are enabled; the brand fields exist for
  white-label, not for a colour per product. `chroma` is the OKLCH chroma itself, 0 to 0.4, while
  `PhxThemeService` takes a multiplier of the 0.18 the phoenix stylesheet applies at 1, so the
  shell divides by that base before feeding it. White-label by hostname is a `ManifestPatch` on
  the server side, answered by the `ManifestSource` of `settings` for the request's host
  ([05-modules.md](05-modules.md)), so it changes `/boot` and `/brand/logo.svg` and needs no
  product code in the browser.
- Metadata cache keyed by the meta version. Descriptor responses carry an `ETag` computed from the
  build, the loaded module set, the enabled products with their editions, the language, the
  principal id and the principal's `acl_gen`, which is incremented in the same transaction that
  changes the principal's permissions, role or account (for a reseller subtree, by path). Responses
  are `private, no-store`; the shell keeps its own cache keyed by `/boot.meta` and
  `X-Keel-Meta-Version`, and drops it on `meta.invalidate` or when a response carries a different
  version. Without the principal in the key, `su` in the same browser would show another
  principal's menu.
- Screens of the system modules ship in the shell: settings, accounts, permissions, tasks,
  scheduler, notifications, problems, channels, webhooks, audit, logs, license, updates, backups,
  plugins, peers,
  products (enabling and disabling bundles, from the `keel.product.*` descriptors), brand and
  support. Every installation has them, without writing any UI: every screen of a system module
  that exists as a descriptor is rendered generically, reached from the menu nodes the modules
  register — the kernel's `system/accounts`, `system/principals` and `system/products`, the
  `auth` nodes `system/sessions`, `system/tokens`, `system/ip-rules` and `system/grants`,
  `system/settings` and `system/brand` of `settings`, `system/audit` of `audit`, `system/tasks`
  of `tasks`, `system/cron` of `cron`, `system/problems` and `system/channels` of `notify`,
  `system/webhooks` of `webhooks`. The shell adds a
  screen of its own only where a descriptor cannot say it, and registers it through
  `provideKeelViews` like a product's view, listed in `frontend/apps/shell/views.json` and counted
  in the custom-views share: the settings value form, which replaces the generic form of
  `settings.set` and takes its control from the schema the row of `settings.get` carries, so an
  integer setting is a number field and a boolean a toggle; and the security page registered for
  `auth.totp.enroll`, with the caller's own password change and the second-factor enrolment — the
  `otpauth://` URI as a QR drawn inline, the secret, the confirmation code and the recovery codes
  shown the one time they arrive.
- The shell consumes the event stream: the progress modal, the task drawer and the notification
  centre above, scope-filtered `data.invalidate` through the lists, and catch-up by
  `Last-Event-ID`; the shell arms `KeelEvents` once and the client binds the stream to the
  principal of `/boot`.

### `/testing`

- A mock client, `KeelMockClient`, over handed fixtures — a `/boot` payload, descriptors, a
  catalogue and canned answers per function — that records every call, and
  `KeelMockClient.fromDump(dir, level)` over a level of the `keel-dev descriptors` dump under
  `contracts/dump/`, so screens can be developed and tested without a running binary.
- Principal fixtures of different levels, `KEEL_PRINCIPALS`, the principals the dump was taken
  with, matching the descriptor snapshots per level.
- Storybook decorators. `withKeel({ level, boot, calls, messages, routes })` hands a story the mock
  client, the loaded boot, the catalogue, a quiet event source, the labels bridge and a router;
  Storybook for `/renderer` and `/shell` runs on the dump and the mock client and serves as
  documentation of the contract and as visual regression without a backend; CI runs the stories
  and their accessibility checks against the static build.
- A harness for Playwright against a built binary, `keelTest` of the nested entry point
  `@globalart/keel/testing/playwright`, with a login through the real screen, the three fixture
  logins and a session over the HTTP API; `hack/e2e.sh` prepares the site and runs the suite under
  `frontend/e2e/` in CI, leaving out the specs tagged `@peers`, which `hack/peers-e2e.sh` runs
  against the two installations it pairs, `frontend/e2e/peers.spec.ts` following "Open in KeelDNS"
  onto the neighbour as the customer of a site ([09-operations.md](09-operations.md)).
- `/boot` fixtures with no product, one product and two products enabled, `KEEL_BOOT`, so the
  brand rule and the product headings are covered by Storybook and Playwright without a second
  installation.

## Embedding

The frontend is compiled into the `keel` binary through `rust-embed`: assets are brotli-compressed
and served immutable by content hash (see [09-operations.md](09-operations.md)). The runtime
builder declares what it embeds in `bin/keel/src/main.rs`:

```
.frontend(keel::embed_frontend!("../../frontend/dist/shell"))
```

`embed_frontend!` expands to a `rust-embed` type over the directory, resolved against the calling
crate's manifest, with `debug-embed` on so the debug binary CI proves carries the assets exactly as
a release does, and answers a `keel::Frontend` the kernel's HTTP transport serves beside the API;
`pnpm build` in `frontend/` must therefore precede `cargo build -p keel-bin`.

There is one frontend build, `frontend/dist/shell`, and it carries the views of every first-party
product. A product with custom views keeps them in an Angular library `frontend/products/<id>` that
exports its `provideKeelViews` and `provideKeelWidgets` providers, built as a chunk of its own,
which the shell loads at bootstrap for the products `/boot.products` names and for those alone;
loading a chunk at the first route of its product is the aim for the milestone that carries the
module-to-product map, since a route names a function and the shell learns the product of a
function only through that map. A product without custom screens has no frontend code at all and
is rendered from descriptors. Which products a running installation has enabled is not a build
question: the shell registers the providers of the products `/boot.products` names and never loads
the chunks of the others. A product whose crates move to their own
repository ([01-overview.md](01-overview.md)) keeps its frontend library in this repository or
publishes it to Astrum as `@globalart/keel-<id>`; which, is decided when it happens. A third-party
composition built from the facade builds and embeds a shell of its own from the same sources.

The `frontend/` directory is a pnpm workspace: `libs/keel` with the entry points, `apps/shell`,
`products/<id>` for each product with custom views, Storybook under `.storybook/`, the Playwright
suite under `e2e/` and the build tools under `tools/`. `pnpm generate` writes what the repository
commits and CI regenerates: the TypeScript wire types from `contracts/schema/`,
`contracts/icons.json` from `PHX_ICON_NAMES` and the `KEEL_BOOT` fixtures of `/testing` from the
three `/boot` payloads.

## Boundary with phoenix-ui

phoenix-ui receives only atomic components; every piece of logic driven by metadata lives in
`@globalart/keel`. The library before M2 had gaps that matter for metadata-driven screens:
`phx-wizard` is only a step indicator, `phx-select` filtered the options it was given locally, the
table had neither column persistence nor a bulk-action panel, and `phx-code-field` is a textarea;
batch 1 closed the remote select and the selection bar, and the M2 renderers recorded what the
next batch owes.

Additions to phoenix-ui are delivered in batches tied to milestones:

| Milestone | Additions to phoenix-ui                                                            |
|-----------|-------------------------------------------------------------------------------------|
| M2        | `phx-selection-bar`; remote mode of `phx-select` (options from `searchChange`, `loading`, `more`); `phx-tags-field`; `phx-number-field` (stepper, unit, unlimited) |
| M3        | `phx-datetime-field`; `select` and `number` types in `PhxCellEditService`; `columnOrder` on `phx-list-table`; a searchable remote mode of `phx-column-filter`; the accessible name of a control inside `phx-form-row`; the heading level of the table's empty state under a page title; `stopPropagation` of the native `search` event in `phx-searchbar` |
| M5        | entry point `@globalart/phoenix-ui/code` (CodeMirror 6 with nginx, bind and ini modes); `phx-log-view` (tail, ANSI); `phx-key-value-field`; `phx-transfer-list`; time axis and tooltips on `phx-chart` |
| 0.5.0     | the workspace chrome: `primary` and `menu` on `phxToolbarButton` with grouped overflow; `appearance="sheet"` and the column menu (`columnsMenu`, `hiddenColumns`) on `phx-list-table` and `phx-data-table`; the `phxTabBarStart` slot, the shortened breadcrumb chain and the context menu of `phx-tab-bar`; `density="compact"` on `phx-sidebar`; `badge` on `phxIconButton`; moving wave background of the full-screen layout; the 13 px secondary text token |

What does not go into phoenix-ui, because it depends on descriptors, preferences or the call
protocol, is code of `/renderer` and `/forms`:

- the wizard step container with navigation and per-step server validation;
- persistence of column order and visibility in `settings_pref`, through `settings.pref.set` under
  the key `columns.<function>`;
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

`menu` is an optional `MenuNode` pointing at the view, which goes through the same ownership,
group and label checks as any node and must carry a label of its own, since a view has no screen
title; its item in `/boot.menu` carries no `kind`. `access.feature` and `needs` go through the feature
guard like a function's, so a view whose feature the lease does not grant leaves `/boot.views`
and the menu.

`/boot.views` lists the registered views. On the client the product registers a component for the
name through `provideKeelViews([{ func, component }])`, and the component receives:

```
KeelViewContext { descriptor?, params, client, principal, tabs }
```

`descriptor` is optional: a view registered through `r.view` has no screen descriptor, while a
component that replaces the generic screen of a function through `provideKeelViews` (the M2
acceptance criterion replaces one screen of the demo product this way) receives that function's
descriptor. `params` are the screen parameters from the route, `client` is the `/client` instance,
`principal` the current principal, `tabs` the object-tab service. A shell built without a
product's frontend library shows `phx-not-found` with a hint for that product's views.

### The ceiling

Custom views absorb everything metadata cannot express — the file manager, the configuration
editor, the terminal, monitoring — and their share is the measure of whether the
metadata approach is paying off. CI computes the share with `keel-dev views share`, run by
`hack/descriptors.sh` over the admin dump: the numerator counts the views of `/boot.views`
registered through `r.view`, the replacements of generic screens listed in
`frontend/products/<id>/views.json` and, for the composition, the shell's own views of
`frontend/apps/shell/views.json`; the denominator is the functions of the product's modules, or of
the whole composition. A registered view belongs to the product whose module id prefixes its name.
The share is printed per product bundle and for the composition; the alarm threshold is 20 % of
functions. Above it the economics of the platform do not hold.

Measures:

- the metric is in CI from M2;
- registries of widgets and column types on both sides: the enums in `keel-types` on the server,
  `provideKeelWidgets` and the renderer's column types in the browser;
- the rule "a custom screen is cheaper than a new attribute": when a screen needs something the
  vocabulary lacks, the first answer is a custom view, not an extension of the descriptor;
- the `ui` vocabulary is reviewed once per milestone.

Milestone targets: at M5 `panel` has exactly two custom views (the file manager and the
configuration editor) and the share of custom views is at most 10 % of the functions of `panel`
(see [../plan.md](../plan.md)); `hack/descriptors.sh` fails unless `keel-dev views share` reports
exactly those two for `panel` within that share. M5 closed with two views over 66 functions of
`panel`, 3.0 %, and 5 over 194 functions, 2.6 %, for the composition. The two views, written in
`frontend/products/panel` with phoenix-ui components alone:

- `hosting.file-manager` (module `hosting`, menu node `panel/files`, feature `panel.users`) lists,
  uploads, renames, downloads, changes the mode of and deletes files in the home of a hosting
  user through `hosting.file.list`, `mkdir`, `rename`, `delete`, `chmod`, `upload` and
  `download`. Listing and download are read functions; the changes answer a `TaskRef` of the
  internal read-mode task `hosting.file.apply`, since the transaction never waits for the agent,
  and the view follows it through `task.get`. An upload is one part of at most 32 MiB, marked
  secret; a mode is three octal digits; a listing stops at 5000 entries and says so.
- `hosting.site.config-editor` (module `hosting_site`, menu node `panel/config`, `Level::Admin`,
  feature `panel.web`) edits the overrides of a site's server block and php-fpm pool in
  `phx-code-editor` beside the installed files, read-only, through `hosting.site.config.get` and
  `hosting.site.config.set`. The plan's names `hosting.config.set` and `hosting.config-editor`
  would not carry the prefix of `hosting_site`, the module of `hosting-web`, and a second module
  in the crate for two functions was not worth its ledger and dictionary. An override is a list of
  directives placed into the compiled template, not a template of its own, checked by a grammar on
  save and at every render ([06-host.md](06-host.md)); a save renders, checks and reloads in a
  `before` hook of its own module, the one stage outside the transaction that can still refuse
  with a field error, and a refused check answers `host.error.validate_failed` with the check's
  output on `server` or `pool`, keeping the edited text.

## Internationalisation

### Key convention

Translation keys are derived by convention from the registry, not written by hand: the function
builder knows the module, the entity, the fields, the actions, the pages and the columns, and the
key for each follows from that. `keel-dev i18n extract` collects the keys from the registry without
a manual list.

| Key                                   | Used for                                  |
|---------------------------------------|-------------------------------------------|
| `<module>.<entity>.<field>`           | field label; derived for every field of the parameter schema |
| `<module>.<entity>.<field>.<enum>`    | label of an enum value; derived for every value of an enum field |
| `<module>.<entity>.action.<id>`       | action label; derived for every action    |
| `<module>.<entity>.page.<id>`         | page or tab label; derived for every page |
| `<module>.<entity>.column.<key>`      | column header; derived for every column, with `<key>` the wire name (`createdAt`) |
| `<module>.<entity>.<column>.<value>`  | label of a filter option or status value of a column; derived for every value in `filter.options` and `map` |
| `<module>.<entity>.<field>.hint`, `.placeholder` | field hint and placeholder; the schema names them explicitly in `x-ui.help` and `x-ui.placeholder`, and only a named key is checked |
| `<module>.<entity>.<field>.empty`   | the label a nullable number field shows for its null in place of `phx.unlimited`; optional, read by the field component, which provides a scoped `PHX_LABELS` with `unlimited` replaced when the catalogue has the key |
| `<patching module>.<entity>.<field>`  | label of a field another module added by a descriptor patch (`storing.item.sync` for a patch of `base.item.edit` by `storing`); a module never writes keys under another module's prefix |
| `menu.<group>`                        | menu group label, declared by a bundle or the platform; `menu.access`, `menu.monitor` and `menu.system` are the kernel's |
| `product.<id>.brand`                  | brand name of a product, the `name` of its manifest |
| `product.<id>.level.<level>`          | a bundle's label of a level, from `levels.labels`, used while it is the only enabled product |
| `keel.level.<level>`                  | platform label of a level, shipped by the kernel, used when several products are enabled |
| `keel.*`                              | shared labels, inherited                  |
| `keel.error.*`, `keel.check.*`, `keel.schema.<keyword>` | the kernel's error, check-preset and schema-keyword messages |
| `phx.*`                               | labels of phoenix-ui components (`PHX_LABELS`) |

Shared labels live under `keel.*` and are inherited: a field, column or action without a
module-specific key resolves to the shared key with the same tail, `keel.<tail>`, so common
names such as `id`, `name`, `state` or `createdAt` are translated once. The kernel ships the
shared labels `keel.{id,name,title,kind,state,login,role,lang,createdAt,updatedAt}`,
`keel.column.*` for the same names, `keel.action.{create,edit,delete,refresh,export}` and
`keel.page.main`, and a derived key is satisfied by either its own entry or the shared one. Keys
that are not derivable are explicit in the descriptor: `ui.title` (`dns.zone.list.title`), the
empty state (`dns.zone.empty`), menu labels (`menu.dns`; the kernel's menu nodes reuse the
screen titles `keel.account.list.title` and `keel.principal.list.title`), error keys
(`dns.zone.error.exists`, `keel.error.validation`) and check keys (`keel.check.ip`). Shared keys
follow the wire member names of [10-contracts.md](10-contracts.md), so the kernel's rows carry
`keel.account.column.parentId` and `keel.principal.column.accountId`.

At registry freeze the server verifies that every key it will emit has a translation at least for
`en`, that a bundle's dictionaries are JSON objects, and that no message anywhere uses a
placeholder format other than `{name}` (see the module lifecycle in
[05-modules.md](05-modules.md)).

### Placeholders

Messages in the envelope are not translated by the server: it sends a key and `params`, and the
consumer formats them. Mail and notification templates are rendered on the server with minijinja.
Placeholders have one format, `{name}`, in minijinja templates, in CLI output and in TypeScript, so
a message key has a single translation that works in all three places. A name is an identifier,
`[A-Za-z_][A-Za-z0-9_]*`; freeze refuses a message with `{{`, `}}`, a printf-style `%s` or
`%1$s`, a positional `{0}`, a padded `{ name }`, an unbalanced brace or an empty `{}`, naming the
offending fragment. A bare `%` (`100%`) is fine.

### Bundles

A module embeds its dictionaries at compile time:

```
r.i18n("en", include_str!("../i18n/en.json")).i18n("ru", include_str!("../i18n/ru.json"));
```

A bundle embeds its own dictionaries through `ProductManifest.i18n`. A plugin ships its own files
and names them in its manifest (`"i18n": { "en": "i18n/en.json", "ru": "i18n/ru.json" }`). The
server merges the dictionaries of the loaded modules and of the enabled bundles and serves them
over `/api/v1/i18n?lang=` as `{ lang, messages }` in the `data` of the call envelope, one language
at a time with `en` filling every gap ([10-contracts.md](10-contracts.md)), so a disabled product
contributes no keys; the shell picks the language from `principal.lang` in `/boot`, and the
user's language lives in `settings_pref`. The products screen of the shell is the kernel's
`keel.product.list` with its row actions `enable` and `disable`, both confirmed, not a screen of
its own; a product's logo is served at `/brand/<id>/logo.svg` from the asset its `Brand.logo`
names.

Translations are managed in OwnLate: one project for the core (the kernel, the system modules,
the frontend runtime, the `phx.*` keys) and one project per product, recorded in
`contracts/i18n.json` with the dictionary directories and the key prefixes each project holds.
`keel-dev i18n pull` fetches every first-party project and rewrites the committed dictionaries,
`<directory>/<lang>.json` nested by key and sorted, so the pull of a synced tree changes nothing;
the Build workflow runs it before the build when the service token is present and fails on a
difference, and the bundles are compiled into the binary, because the server Keel is installed
on may have no outbound access and cannot fetch translations at runtime. `keel-dev i18n missing`
derives every key the descriptors of the dump emit by the table above, adds the menu labels, the
product names and every leaf of `en`, and fails CI on any key the committed dictionaries do not
translate in a served language, a derived key satisfied by its own entry or by `keel.<tail>`;
`keel-dev i18n extract` writes the catalogue of the dump split per project, the file that seeds
a project. The dictionaries in the repository are the per-language source, edited there between
pulls; the freeze's `en` check catches a key without any entry.

`phx.*` is the namespace reserved for `PHX_LABELS`: the labels phoenix-ui components render
themselves (table chrome, pagination, dialogs) come through the same bundles and are bridged into
phoenix-ui by the shell.

## Frontend work by milestone

The full plan is in [../plan.md](../plan.md); the frontend-relevant parts are:

| Milestone | Frontend scope                                                                      |
|-----------|-------------------------------------------------------------------------------------|
| M2        | entry points `/meta`, `/client`, `/forms`, `/renderer` (list, form, action, wizard, tabs, the object renderer), `/shell` with the one shell application (`/boot.products`, the brand rule, product headings, the settings value form and the security page as its own views), `/testing` with the `/boot` fixtures, the dump loader, the Storybook decorator and the Playwright harness; `rust-embed` in `bin/keel`; Storybook with fixtures; phoenix-ui batch 1; the OwnLate to `PHX_LABELS` bridge; publication of `@globalart/keel` to Nexus |
| M3        | the real event stream in `/client` with `data.invalidate` through the lists; the progress modal, the task drawer, the notification centre, the about drawer and the home screen in `/shell`; the export dropdown, column order, the cell editor and the multi-row form in `/renderer`; the file control, the upload submit and the scoped labels in `/forms`; download and upload in `/client`; phoenix-ui batch 2 pinned at `0.3.1`; the OwnLate projects with `keel-dev i18n pull`, `missing` and `extract` |
| M5        | renderers `report`, `dashboard`, `tree`, `master-detail`, `stream`; phoenix-ui batch 3 pinned at `0.4.0` with the `code`, `key_value` and `transfer` controls in `/forms`; `KeelClient.stream`; the forgotten-password flow at `/reset` and the address confirmation at `/verify` in `/shell`; one named chunk per product under `frontend/products`, checked by `tools/product-chunks.mjs`; the file manager and configuration editor of `panel` as custom views in `frontend/products/panel` |
| M6        | the role `external` in the action runner, `KEEL_OPEN_EXTERNAL` and `keel.error.target`; the row action "Open in KeelDNS" of `panel` and the column `zoneError` from descriptors alone; `frontend/e2e/peers.spec.ts` against two installations through `hack/peers-e2e.sh` |

M2 is done when the `demo` bundle, enabled on the `keel` binary, is driven entirely from the
browser without any product TypeScript: a list with search, sorting, filters and bulk delete with
confirmation; a form with pages, `show_if`, recompute and a field-level error; a wizard; one screen
replaced by a custom view through `provideKeelViews`; `en` and `ru` switch; Storybook renders the
shell from `/boot` fixtures with one product and with two; and the binary with the frontend inside
is smaller than 40 MB. The cap for the release binary carrying `dns` and `panel` is set at the
start of M5 ([11-decisions.md](11-decisions.md)).
