# Accounts, principals and access

This document defines who owns data in a Keel installation and who may call what: the account as the
unit of ownership, the principal as a member of an account, the fixed ladder of access levels,
fine-grained permissions, delegation (`su`, socket, on-behalf), the ownership and scoping helpers
the SDK gives to modules, per-account limits, the account lifecycle, and the stable external
identity used to synchronise accounts with neighbouring products. The dispatcher pipeline that
enforces these rules is described in [02-functions.md](02-functions.md); the tables live in the
kernel schema described in [04-storage.md](04-storage.md); the `auth` module that implements
sessions, tokens and `su` is described in [05-modules.md](05-modules.md).

## Ownership model

The unit of ownership is the account: a tenant. A person is a principal, a member of an account
with a role inside it. Ownership is never modelled as a tree of users with an `owner_id` on each
user, because that shape fits only a hosting panel where one person equals one customer. A billing
customer is a company with several people; a virtual-machine project is a team. With the account
as the owner, every owned row of every module carries an `account_id`, and the kernel can answer
"may this caller see this row" with one rule for every product.

The kernel owns three of its eight tables for this: `keel_account`, `keel_principal` and
`keel_limit`. It also owns the `keel.account.*` and `keel.principal.*` functions and the
ownership guard in the dispatcher. Passwords, sessions, API tokens, `su` and the storage of
fine-grained permissions are not in the kernel; they belong to the `auth` system module, which is
written on the public SDK and can be replaced by the product composition (for example by an SSO
module).

Accounts and principals belong to the installation, not to a product. Every product enabled on a
host shares the one account tree, the one principal set and the one session: a customer of
KeelPanel and KeelDNS on the same host has one login and sees the objects of both, an
administrator administers both, and a reseller's subtree is the same subtree in every product. A
product contributes only vocabulary — the account kinds and roles it creates — and its own owned
tables, which carry `account_id` like any other. What separates products for a principal is
permission keys and license features, never a second account table. Products on different hosts
remain separate installations with separate trees, synchronised through `origin` and
`external_id` ([07-ecosystem.md](07-ecosystem.md)).

## Levels

The ladder of access levels is a fixed enum in `keel-types`:

```
guest < service < user < reseller < admin < owner
```

| Level      | Meaning                                                                          |
|------------|----------------------------------------------------------------------------------|
| `guest`    | No principal. Only functions declared at this level: `auth.login`, the one-time |
|            | login-key page, `keel.setup.admin` while no admin exists.                        |
| `service`  | Integration tokens without a UI.                                                 |
| `user`     | A member of a customer account.                                                  |
| `reseller` | A principal of level `reseller` in an account of kind `reseller`; sees the       |
|            | subtree of accounts under its own.                                               |
| `admin`    | Administrator of the installation; sees everything in every enabled product.    |
| `owner`    | The local control socket (`ctl` with peer credentials), not an HTTP login.       |

The enum itself never changes per product. What a product bundle controls is the labels and
which levels are enabled: a billing product labels `user` as "client", a VM product does without
`reseller`. Disabling a level hides it; it does not renumber the ladder. With exactly one product
enabled on the installation its labels and disabled levels apply everywhere. With several, the
platform labels `keel.level.<level>` apply and a level is hidden only when every enabled bundle
disables it; whether a product's own labels should still appear inside its own screens is an
open question below.

Levels are used in several places of the protocol, always with the same ordering:

- `access.level` of a function: the minimum level of the caller.
- `x-keel.level` on a schema field and `access.level` on a descriptor column or action: the
  minimum level that may see or write that field. A field above the caller's level is stripped
  from responses and from the projected descriptor; writing it answers `forbidden_field` (403),
  never a silent drop.
- The level ceiling of an API token and of a peer token's scope.
- `$principal.level` in screen conditions (`show_if`, `required_if`, `readonly_if`).
- The `level` of a plugin manifest: a plugin's hooks may not target functions above its own level.
- Defaults of permissions, settings (`SettingDef::level`) and limits (`LimitDef` level).
- `keel-testkit` snapshots descriptors once per level, and `call_as(Level::Reseller, ...)`
  runs a function as a synthetic principal of that level.

The Rust form, as fixed in `keel-types`:

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Level { Guest, Service, User, Reseller, Admin, Owner }
```

## Permissions

Levels are coarse. Fine-grained rights are string keys such as `dns.zone.manage`, registered by
the module that owns them together with the levels that hold them by default:

```rust
r.permission("dns.zone.read", &[Level::User, Level::Reseller])
    .permission("dns.zone.manage", &[Level::User, Level::Reseller]);
```

The kernel knows the keys and their defaults from the registry. Which principal actually holds
which key is stored by the `auth` module through the `PermissionSource` extension point; the
kernel asks the source, it does not read the table itself: the HTTP transport asks once per
request, the socket for a frame with `as`, and the answer is overlaid on the principal as
`Principal.grants`, an in-process map from key to `granted` that is skipped by serde and never
crosses the wire. The guard reads a stored grant over the registered default, so a grant gives a
lower level a key its module did not list and a denial takes a default key away; the descriptor
projection and the menu go through the same function, so a denial hides the screen as well as
refusing the call. The kernel provides `PermissionCatalogue { registered, holds }` under the
platform name, so `auth` checks a key without the registry through the guard's own function.
`Principal::can(permission)` is the call-side view of the same answer. The defaults are the answer
when no source is composed, and
the floor under any source: `admin` and `owner` hold every registered key, and a lower level
holds a key exactly when the module listed that level in the defaults — the levels do not
inherit from one another, so a module that wants a reseller to read its subtree lists
`reseller` next to `user`, as the `keel` module does: `keel.account.read`,
`keel.principal.read` and `keel.principal.manage` default to `[user, reseller]`, and
`keel.account.manage` to `[reseller]`.

The registry holds the keys of the modules that are loaded, so the permission set of an
installation is the union over its enabled products. Grants the `auth` module stores for a key of
a product that is later disabled stay in its tables like any orphaned data and count again when
the product is enabled. `admin` and `owner` hold every registered key of every enabled product: an
administrator of the installation administers all of them, and a lower level holds by default
every key whose module listed that level, whichever product the module belongs to.

A function declares its access as a triple: level, permission and licence feature.

```rust
function::<ZoneEdit, Saved>("dns.zone.edit")
    .kind(Kind::Form).mode(Mode::Write)
    .access(Level::User, "dns.zone.manage").feature("dns.zones")
    .owned::<zone::Entity>()
```

which is projected into the descriptor as

```json
"access": { "level": "user", "permission": "dns.zone.manage", "feature": "dns.zones" }
```

The same triple appears in every descriptor; the list descriptor returned by
`GET /api/v1/functions/dns.zone.list` carries
`"access": { "level": "user", "permission": "dns.zone.read", "feature": "dns.zones" }`.

Columns and actions carry the same `access` block at a finer grain, and a schema field carries
the level in `x-keel`:

```json
{ "key": "dnssec", "type": "toggle", "edit": { "function": "dns.zone.update" }, "access": { "permission": "dns.zone.manage" } },
{ "key": "account", "type": "text", "access": { "level": "reseller" }, "filter": { "remote": "keel.account.options" } }
```

```rust
#[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
pub account: Option<Id>,
```

The descriptor a client receives is already projected for its principal: columns, actions and
fields above the caller's level or permission are cut out, so the frontend never renders a
control the server would refuse.

The dispatcher evaluates access before the handler, in a fixed order: level, permission, licence
feature, token scope, module guards, then the ownership guard described below. A token whose
scope names one function receives `forbidden` (403) on any other. The full pipeline is in
[02-functions.md](02-functions.md).

The feature guard answers `license` (402, `keel.error.license` naming the feature) for a function
whose feature is not in the feature set, and the projection asks the same question with the level
and the permission, so such a function leaves the descriptor listing, answers 402 for its own
descriptor and disappears as an action, a row action, a column's inline edit, a dashboard widget
and a menu item wherever it is referenced; a custom view that needs such a feature is cut too.
Since M5 the feature set is the one the `license` module provides as `dyn Features`: the
features of the editions the lease lists, or of the default edition of each enabled product
while no lease is installed ([05-modules.md](05-modules.md)); `/boot.features` carries it, so the
list and the menu agree. A composition without the `license` module keeps the set of M4, every
feature of every edition the enabled products declare. The metadata version hashes the features,
the editions and the license state only when the feature service reports a license, so the
versions of a composition without one did not change. Beside the feature guard the `license`
module adds the read-only guard of an expired or unlisted lease, which refuses an entered call
of a function that writes with the same `license` code and `keel.error.license`.

## Delegation

An operation can be performed by one principal on behalf of another. In every such case the call
context carries two identities: the effective `principal` and the `actor` who initiated it, with
`via` naming the mechanism.

| Mechanism | Who                                              | `actor.via` |
|-----------|--------------------------------------------------|-------------|
| `auth.su` | an admin or reseller entering a lower principal  | `su`        |
| `ctl --as`| the local socket (`owner`) running as a principal | `socket`    |
| on-behalf | a paired neighbour acting for one of its clients | `peer`      |

The mechanism is the `via` of the actor principal itself: the actor of a `su` session carries `via:
Su`, the `--as` actor is the socket principal with `via: Socket`, the on-behalf actor is the
principal of the peer's link with `via: Peer(<link id>)`; an actor with any other `via` is a kernel
bug, so `actor.via` on the wire is a closed set of exactly these three values. The dispatcher checks
this before the call runs: a principal whose actor arrived any other way is answered `500` with
`keel.error.actor_not_delegated` and the offending `via` in `params`, and no function runs for it.

Rules for `su`:

- Only downwards on the ladder: a reseller may enter a `user` of its subtree, an admin may enter
  a reseller or a user, nobody enters a level equal to or above their own.
- `su` opens a separate session that records the actor; `su.exit` returns to the session it was
  started from. The session, not the request, carries the actor, so every call made inside the
  delegated session is attributed the same way.
- In `auth_session` the delegated session is a second row of `kind: su` carrying
  `actor_principal_id` and `parent_id_hash`, created with the parent's lifetimes; the parent row
  names its live child in `su_id_hash`, and the cookie does not change: the authenticator
  resolves the cookie to the parent row, follows `su_id_hash`, slides both rows and answers the
  child's principal with the parent's as `actor`. `su.exit` revokes the child and clears
  `su_id_hash`; revoking the parent revokes its child through `parent_id_hash`, and a revoked or
  expired child is ignored, so the parent answers as itself again. A target outside the caller's
  scope answers `not_found`, one at or above the caller's level `auth.error.not_below`, and a
  caller already delegated `auth.error.su_nested`.
- The audit record contains both sides: the effective principal and the actor.
- Metadata is keyed by the effective principal, so entering a user in the same browser shows that
  user's menu, not the admin's (see `acl_gen` below).

The envelope of every call reports the pair in `meta.principal`:

```
POST /api/v1/call/dns.zone.edit
Authorization: Bearer kat_01J9…          (or Cookie: __Host-keel_session=ks_…)
Idempotency-Key: bill-order-9912-open
{ "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3", "ttl": 300 }

200  X-Keel-Request-Id: req-1a2b   X-Keel-Meta-Version: m-7f3a2c
{ "ok": true, "data": { "id": "01J9Q3…" },
  "meta": { "requestId": "req-1a2b",
            "principal": { "id": "01J9…", "login": "alice", "level": "user", "account": "01J8A…",
                           "actor": { "id": "01J8…", "login": "root", "level": "admin", "via": "su" } } } }
```

The same shape is returned by the CLI, which runs over the local socket and may name the
principal to act as:

```
CLI:  keel ctl dns.zone.edit id=01J9Q3… ttl=300 --as alice --out json     (same envelope; --out table renders ui.columns)
```

Without `--as` every frame on the socket runs as the owner principal: the nil id
`Id::ROOT_ACCOUNT` as its id, login `owner`, level `owner`, the root account, no actor, `via:
socket`. With `--as <login>` the call frame carries `as`, and the kernel runs it as that stored
principal with the owner as `actor` and `via: socket`, answering `not_found` for a login it does
not know ([10-contracts.md](10-contracts.md)).

On-behalf calls from a neighbouring product (`X-Keel-On-Behalf-Of: origin=…; external_id=…`)
produce a principal whose `actor` is the principal of the peer's link, `{ id, login, level, via:
peer }` in the envelope like the actor of `su`. They are accepted only for principals and accounts
whose `origin` names that peer, so a leaked billing token does not open a login under manually
created users. The call runs with the customer's level, grants, ownership and account scoping, and
the token's function masks and level apply on top: a customer above the token's level answers
`keel.error.token_level`. The hidden `OnBehalf` seam of the SDK turns the header into the principal
of the call, so the kernel knows nothing of origins. A peer hands its customer over with
`auth.key.handover`, a one-time key for the customer itself issued by the link, which answers only a
call on behalf of a customer lying below the level granted to the link; and every credential
minted through a link, an API token, a login key and the session such a key opens, dies with the
link at `peer.rotate` and `peer.delete`. The pairing, scope and login rules are in
[07-ecosystem.md](07-ecosystem.md).

The M1 acceptance sequence exercises all of this end to end:
`login → boot → keel.account.list → auth.su → list narrowed and actor filled → su.exit`;
a token scoped to one function gets 403 on another; writing a field above level gives
`forbidden_field`; the audit contains both sides of `su` and masks the password, including for
failed calls. The M1 exit checklist of [../plan.md](../plan.md) names the test and the command
that demonstrate each item.

### `acl_gen` and metadata caching

Descriptors and `/api/v1/boot` depend on the principal's rights, so their cache key must include
the principal. The ETag is `hash(build, modules, products, lang, principal_id, acl_gen)`, where
`modules` is the loaded module set and `products` the enabled products with their editions, so
enabling a product or changing its edition changes the key. `acl_gen` is a counter in
`keel_principal` that is incremented in the same transaction that changes the principal's
permissions, role or account; when an account subtree changes, every principal in that subtree is
bumped by `path`. Responses are `private, no-store`, and the SSE event `meta.invalidate` tells
open clients to refetch.

## Accounts

`keel_account` is the tenant table. Its columns:

| Column        | Meaning                                                                        |
|---------------|--------------------------------------------------------------------------------|
| `id`          | ULID, `TEXT`.                                                                  |
| `kind`        | Account kind. The kernel uses `system` for the root account and `reseller`     |
|               | for reseller accounts; other values (`user`, `client`, `project`) are declared |
|               | by the bundles enabled on the installation through                             |
|               | `ProductManifest.account_kinds`, and the kernel accepts their union — the      |
|               | same kind declared by two bundles is one kind.                                 |
| `parent_id`   | Parent account; `NULL` only for the root.                                      |
| `path`        | Materialized path from the root: the root's path is `/`, a child's is its      |
|               | parent's path followed by the child's id and `/`, so `/<id>/…/`.               |
| `state`       | `active`, `suspended`, `deleting`.                                             |
| `origin`      | Which neighbour created the account (`NULL` for local ones).                   |
| `external_id` | The account's id in that neighbour. `(origin, external_id)` is unique.         |
| `created_at`, `updated_at` | `timestamptz`.                                                    |

The materialized path is what makes reseller scoping cheap: "everything under account A" is a
single `LIKE 'A.path%'` predicate, added to a query by the kernel. The depth of the chain is not
bounded by the schema; whether products allow arbitrary nesting or exactly one reseller level is
an open question (see below).

The root account has `kind: system` and `path: /` and is created by a kernel migration. It owns
everything that is not a customer: `owner` and `admin` principals, service tokens, and the
principals of plugins and paired neighbours. There is therefore no principal without an account
and no owned row without a tenant. The first administrator is created through `keel.setup.admin`,
a `guest` function that exists only while no admin exists and is reached by the one-time link
printed at first start ([09-operations.md](09-operations.md)).

A reseller is not a special table: it is a principal of level `reseller` in an account of kind
`reseller`, and its customers are accounts whose `parent_id` is that account. An account is
visible to every enabled product whatever its kind, because scoping is by `path`, not by kind.

```sql
CREATE TABLE keel_account (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    parent_id   TEXT REFERENCES keel_account(id),
    path        TEXT NOT NULL,
    state       TEXT NOT NULL DEFAULT 'active',
    external_id TEXT,
    origin      TEXT,
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL,
    UNIQUE (origin, external_id)
);
```

```rust
pub struct Account {
    pub id: Id,
    pub kind: String,            /* "user" | "client" | "project": vocabulary of the enabled bundles */
    pub parent_id: Option<Id>,
    pub path: String,
    pub state: State,
    pub external_id: Option<String>,
    pub origin: Option<String>,
}
```

On the wire the row of `keel.account.get` and `keel.account.list` carries these members in
camelCase — `parentId`, `externalId`, `createdAt`, `updatedAt` — while the columns stay
snake_case ([10-contracts.md](10-contracts.md)). An account has no name, so the rows of
`keel.account.list` and the options of `keel.account.options` carry a `label` computed when they
are read, with no column behind it: the login of the principal with the role `owner` created
first in that account, and the last eight characters of the id when the account has no such
principal. A caller who may not list principals, by the rule of `keel.principal.list` (the level,
the permission `keel.principal.read` and the scope of its token), gets the short id, so a label
never shows a login the caller could not already read; the value of an option stays the id. The
forms `keel.account.create` and
`keel.account.update` take the parent as `parent`. `keel_account` is itself `Owned` by `id`:
the scope of a caller over accounts is the same subtree rule as over any owned row, so a user
sees only its own account and a reseller its subtree. The kernel reserves `kind = system` for
the root and refuses it on create and update (`keel.account.error.kind_reserved`); the root
row cannot be changed (`keel.account.error.root_immutable`), and an account cannot be moved
under itself or one of its descendants (`keel.account.error.parent_cycle`). Moving an account
rewrites the `path` of its whole subtree. The union of the enabled bundles' `account_kinds` is
not yet enforced on create: at M0 any identifier other than `system` is accepted, and the check
arrives with the first bundle that declares a kind other than `user`. An account carries no name,
so `keel.account.renamed` is not emitted and `AccountLifecycle`'s `on_rename` keeps no caller.

## Principals

`keel_principal` holds identity and membership. At M0 a principal belongs to exactly one account:
`account_id` and `role` are columns of the principal row.

| Column        | Meaning                                                                        |
|---------------|--------------------------------------------------------------------------------|
| `id`          | ULID, `TEXT`.                                                                  |
| `account_id`  | The account the principal is a member of.                                      |
| `login`       | Unique login name.                                                             |
| `level`       | One of the ladder values, never `guest`. `keel.principal.create` and `update` refuse a level above the caller's own, and bind the level to the account's kind: `admin` and `owner` live only in the root account, `reseller` only in an account of kind `reseller`. |
| `role`        | Role inside the account (`owner`, `member`, ...): product vocabulary.          |
| `state`       | Principal state; `active` by default. No function suspends a principal, so `suspended` stays reserved and the events `keel.principal.suspended` and `resumed` are not emitted. |
| `lang`        | Language of the principal.                                                     |
| `origin`, `external_id` | Stable external identity, unique as a pair.                          |
| `acl_gen`     | Rights generation counter for metadata caching; bumped by a change of the principal's level, role or account, and for every principal of a subtree by a change of an ancestor account. |
| `created_at`, `updated_at` | `timestamptz`.                                                    |

The row on the wire carries `accountId`, `externalId`, `aclGen`, `createdAt` and `updatedAt`;
the forms take the account as `account` ([10-contracts.md](10-contracts.md)).

```sql
CREATE TABLE keel_principal (
    id          TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL REFERENCES keel_account(id),
    login       TEXT NOT NULL UNIQUE,
    level       TEXT NOT NULL,
    role        TEXT NOT NULL DEFAULT 'owner',
    state       TEXT NOT NULL DEFAULT 'active',
    lang        TEXT NOT NULL DEFAULT 'en',
    external_id TEXT,
    origin      TEXT,
    acl_gen     INTEGER NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL,
    UNIQUE (origin, external_id)
);
```

The in-process call context is richer than the row: it carries the actor, the token scope and
`via`: the transport the call arrived on, or `Su` for the actor recorded by a delegated session.

```rust
pub struct Principal {
    pub id: Id, pub login: String, pub level: Level,
    pub account_id: Id, pub role: String,           /* role inside the account (owner|member|...): product vocabulary */
    pub actor: Option<Box<Principal>>,               /* su, --as, on-behalf (via = su|socket|peer) */
    pub scope: Scope,                               /* Full | Functions(masks) | ReadOnly */
    pub via: Via,                                   /* Http | Socket | Plugin(id) | Peer(id) | Internal | Su */
    pub external_id: Option<String>, pub origin: Option<String>, pub lang: String,
    pub grants: BTreeMap<String, bool>,             /* PermissionSource; in-process, skipped by serde */
}

impl Principal {
    pub fn can_see(&self, account_path: &str) -> bool;                       /* path.starts_with(self.account.path) for reseller */
    pub fn can(&self, permission: &str) -> bool;                              /* PermissionSource */
}

impl<P> Call<P> {
    pub async fn resolve_account(&self, requested: Option<Id>) -> keel::Result<Id>; /* user: own; reseller/admin: inside the subtree */
}
```

`resolve_account` lives on the call rather than on the principal because a reseller's answer
needs the database: whether the requested account lies under the reseller's path is a query
against `keel_account`, run on the call's connection.

`/api/v1/boot` returns the principal the shell renders for, including the preferences the shell
needs (language, time zone, whether TOTP is enrolled) and the actor when a delegated session is
active:

```json
"principal": { "id": "01J8…", "login": "res1", "level": "reseller", "account": "01J8A…", "lang": "ru", "tz": "Europe/Belgrade", "actor": null, "totp": true }
```

Note that `account` is explicit in the boot payload, in the envelope `meta.principal`, in the
session and in the token. It is never derived on the client from the principal id. This is what
allows N:M membership to arrive later: a person who is a member of two billing companies gets an
additive table `keel_membership` and a "current account" in the session, while the wire protocol,
the audit record (one `(principal_id, account_id)` pair per call) and every `Owned` table stay as
they are. Whether to start with N:M immediately is an open question (see below and
[11-decisions.md](11-decisions.md)).

## What the SDK gives to modules

### `Owned` and `scope_by_account`

A module entity that belongs to a tenant implements `Owned`, naming its account column. The
kernel then scopes every query on it by the caller:

```rust
pub trait Owned: EntityTrait { fn account_column() -> Self::Column; }
pub fn scope_by_account<E: Owned>(q: Select<E>, p: &Principal) -> Select<E>;
#[async_trait] pub trait ScopePolicy: Send + Sync { async fn visible_accounts(&self, p: &Principal) -> keel::Result<AccountScope>; }
```

| Caller level       | Visible accounts                                   |
|--------------------|----------------------------------------------------|
| `guest`            | nothing                                            |
| `service`, `user`  | its own account                                    |
| `reseller`         | the subtree under its account, by `path`           |
| `admin`, `owner`   | everything                                         |

The table is the `AccountScope` of a principal: `Nothing`, `Own(account)`, `Subtree(account)`
or `All`. A service token sits in an account like a user and sees that account; a guest has no
account and sees no row.

The rule is a `ScopePolicy` service; a product may provide its own implementation (for example a
project-based product with a different visibility rule) without touching the kernel. The same
policy is what filters SSE events per client, so a user receives `data.invalidate` only for its
own objects, and what `keel.export` applies while streaming pages.

A function that has a `key` must say whether it is owned. `.owned::<E>()` turns on the ownership
guard: before the handler runs, the kernel loads the `account_id` of the addressed row (or of all
ids in a batch) and answers `not_found` (404) if it is outside the caller's scope, so that the
existence of foreign rows is not revealed. A function that deliberately has no owner declares
`.unowned()`; a function with a key and neither declaration is rejected by `keel-dev check`.
`keel-testkit` ships two tests in the module template that pin this invariant, the helpers
`Harness::assert_reseller_sees_only_subtree(func_list, &fixture)` and
`Harness::assert_user_cannot_edit_foreign_row(func_edit, &fixture)`, where the `Fixture` creates
a row of the module in a given account. They hold only when the module's permission defaults
list `reseller`: under the default source a level holds a key exactly when listed (see
"Permissions").

Foreign keys from module tables to `keel_account` and `keel_principal` are the only cross-module
references the storage rules permit outside a declared `requires` ([04-storage.md](04-storage.md)).

### `resolve_account`

When a form lets a higher level choose the tenant for a new row, the handler asks the call to
resolve the requested account for its principal:

```rust
let account = call.resolve_account(call.params().account).await?;
```

- `user`: always its own account; the `account` field is above its level, so a value sent by a
  user is refused earlier as `forbidden_field`.
- `reseller` and `admin`: any account inside their visible subtree; an account outside it, or
  one that does not exist, is answered with `keel.error.account_not_found` attached to the
  `account` field, which the envelope carries as `422 validation` with one entry in `fields[]`
  ([10-contracts.md](10-contracts.md)), so the form shows the error on the field and the
  existence of foreign accounts is not revealed.

The `account` field of such a form uses `keel.account.options` as its remote option source, so
the select shows only what the caller may pick.

### Limits

Quantitative limits per account (zones, domains, disk) are a kernel-level mechanism, not a
per-module convention. A module contributes a `LimitDef` with the key, unit, default, the level
that may edit it and whether "unlimited" is allowed:

```rust
r.contribute(LimitDef::new("dns.zones", Unit::Count).default(50).unlimited(true));
```

Values are stored per account in `keel_limit(account_id, key, value)` and inherited along
`path`: a reseller cannot grant a customer more than it has itself. An edition ceiling from the
`license` module is foreseen and not built: no edition of M5 carries a limit. A handler checks
before creating:

```rust
call.check_limit(key, current)
```

and receives `limit_exceeded`, a 409 in the envelope with an i18n key, when `current` does not
fit under the effective value, with the parameters `limit` and `max`. Invariant checks like this
belong in `within` hooks or the handler, inside the transaction
([02-functions.md](02-functions.md)). The effective value is the tightest one stored along the
account's `path`; a stored `null` means unlimited where the definition allows it, and the default
applies otherwise. Account limits shipped with the first product, dns, at M4: `dns.zones` (50 per
account) and `dns.record.max` (500 per zone, checked against the zone's account under the zone's
row lock), both admitting unlimited. The `panel` bundle of M5 adds `hosting.users`,
`hosting.site.max`, `hosting.database.max`, `hosting.database.user.max` and `hosting.ftp.max`,
10 per account each and unlimited allowed, each checked under an advisory lock per account
before the insert; `hosting.users` is the one key named after the module `hosting`, the others
after their modules (`hosting_site`, `hosting_database`, `hosting_ftp`), as `dns.record.max` is.
The stored values have two writers since M6: the service kinds `dns.zones` and `hosting.sites`
store the account's `dns.zones` and `hosting.site.max` through the hidden
`LimitSource::store(key, account, value, db)`, which the kernel answers by writing the account's
row of `keel_limit` and refuses for an undeclared key or for `null` where the definition admits no
unlimited, the effective limit staying the tightest along the path
([07-ecosystem.md](07-ecosystem.md)); a function through which an administrator edits the stored
values is not built yet.

The limit table is `keel_limit(account_id, key, value)`, one of the kernel tables
([04-storage.md](04-storage.md)); the keys stored in it come from `LimitDef` contributions.

### `AccountLifecycle`

Suspending, resuming, renaming or deleting an account has consequences in every module that owns
rows for it, and in host products those consequences include operations on the OS that may only
run after commit or as a task. A module therefore contributes an `AccountLifecycle` with the
handlers it needs:

```rust
r.contribute(AccountLifecycle { on_delete: Some(lifecycle::drop_zones), ..Default::default() });
```

The handlers are `on_suspend`, `on_resume`, `on_delete` and `on_rename`. For each state change
the kernel invokes the matching handler in every module that contributed one and emits the
matching event. The contribution, `keel.account.delete`, `suspend` and `resume` were built in M4,
for the `on_delete` of `dns`.

`keel.account.suspend` and `keel.account.resume` (reseller, `keel.account.manage`, owned) lock the
row, refuse the root account, the caller's own account and an account in `deleting`, answer
`changed: false` when the state is already the requested one, and emit
`keel.account.suspended` or `resumed`. Their handlers run after commit, in `requires` order, each
in a transaction of its own that first reads the account row `FOR SHARE` and runs the handler
only while the row still holds the state the change set, so a suspend and a resume in quick
succession cannot leave a module in the state of the earlier one; a failing handler is logged and
the walk goes on.

`keel.account.delete` is an action whose result is a task:

1. the kernel sets the account `state` to `deleting`, refusing the root account, the caller's
   own account and an account with child accounts;
2. the task walks the modules in reverse `requires` order, dependents first, calling `on_delete`
   with progress per module, a checkpoint `{ done: [module] }` and the log line `deleted
   <module>` after each; a resumed job skips the modules the checkpoint names;
3. only then does it remove the account's limits, its principals and the `keel_account` row in
   the job's own transaction and emit `keel.account.deleted`; a foreign key that still holds the
   row answers `keel.account.error.rows_remain` with the constraint's detail.

A failing handler fails the task with its own error and the module as a parameter and leaves the
account in `deleting`, which `doctor` reports; calling delete again runs a new job. The modules
`auth`, `settings`, `notify`, `webhooks` and `svc` of `keel-system` contribute an `on_delete`: `auth`
removes the tokens, sessions (with the sessions that hang from them by `parent_id_hash`; a session
that merely pointed at a removed one by `su_id_hash` stays and loses the pointer), login keys and
challenges, mail tokens, recovery codes, second factors, passwords, addresses and grants of the
account's principals and of the account; `settings` its values and the principals' preferences;
`notify` its messages, problems and pending direct mails through `drop_account` and one more
statement; `webhooks` its subscriptions and deliveries through `drop_account`. The `plugins`
module, which only a build with the `plugins` feature carries, contributes none yet, so deleting an
account whose principals toggled a plugin fails with `rows_remain` there. `on_rename` keeps no
caller, since an account carries no name.

Module migrations use `ON DELETE RESTRICT` on their account foreign keys by default. A module
that forgot to contribute `on_delete` therefore makes the deletion fail at step 3 rather than
leaving orphaned rows or OS resources behind: the operation breaks loudly instead of losing data.
A module may also refuse the deletion up front with a `within` hook, which runs inside the
transaction of the call:

```rust
r.within("keel.account.delete", hooks::forbid_delete_with_zones);
```

The other state changes drive the matching handlers and emit the matching events
(`keel.account.suspended`, `resumed`). For service accounts opened by billing, the `close` of the
last open service of an account moves the account to `suspended` and keeps the service for a
retention period, and deletion remains a separate explicit call
([07-ecosystem.md](07-ecosystem.md)).

### External identity: `origin` and `external_id`

Accounts and principals that a neighbouring product creates carry that product's identity for
them: `origin` names the neighbour, `external_id` is the id there, and the pair is unique in both
tables. `keel.account.upsert` and `keel.principal.upsert` are idempotent on this pair, which makes
them the primitive for synchronisation from billing: repeating a call creates nothing twice, and
two services of different kinds for the same client resolve to one account.

Products enabled on the same installation need none of this: they share the rows directly.
`origin` and `external_id` are for installations on different hosts.

```
POST /api/v1/call/keel.principal.upsert
{ "origin": "bill:01J9A…", "externalId": "client-42", "login": "alice", "level": "user",
  "account": { "origin": "bill:01J9A…", "externalId": "acc-7", "kind": "user" } }
→ { "id": "01J9P…", "accountId": "01J8A…", "created": true }
```

`keel.account.upsert { origin, externalId, kind?, parent? }` answers `{ id, created }` and
`keel.principal.upsert { origin, externalId, login, level, role?, lang?, account }` `{ id,
accountId, created }`; both are kernel actions at the `reseller` level, under `keel.account.manage`
and `keel.principal.manage`, without a screen and without an ownership guard, and the principal
upsert upserts its account first through `call.invoke` of `keel.account.upsert`, so that function's
level, permission and hooks apply and its field errors come back under `account`. Both members of
both pairs are 1 to 256 characters of `EXTERNAL_PATTERN` of `keel-types`, visible ASCII without `"`,
`,`, `;` and `\`, the alphabet of the on-behalf header, so every synced customer stays addressable
on behalf; a character outside it answers `keel.schema.pattern`. A new account takes the kind `user`
unless `kind` names another, and the caller's own account as its parent unless `parent` names one
within the caller's reach; `parent` applies only when the account is created. On an existing account
a different kind is applied and `acl_gen` is bumped for its subtree, as `keel.account.update` does,
the state is never changed, and an account in `deleting` answers `409 keel.account.error.deleting`;
on an existing principal the login, level, role, language and account are applied with the rules of
`keel.principal.update`, which shares the change path, and a taken login answers
`keel.principal.error.login_exists`. A pair held outside the caller's reach answers `422
keel.account.error.external_taken` or `keel.principal.error.external_taken` on `externalId`.
Concurrent upserts of one pair insert with `ON CONFLICT DO NOTHING` and read the row again for
update, so they end with one row and one `created: true`.

The pair also gates access from a peer. A peer may act only for principals whose origin and whose
account's origin name that peer's instance ([07-ecosystem.md](07-ecosystem.md)), and the upserts
refuse an origin naming an instance, `<name>:<instance id>`, to a caller below `admin` other than
the principal of the link to that instance, whether it calls through its peer token, a token minted
under it or on behalf of a customer, with `403 peer.error.origin`: a record under the origin of a
peer was placed by that peer or by an administrator. The `peers` module checks it in hooks inside
the transaction of both upserts; origins of any other shape stay open to resellers, and a
composition without `peers` binds nothing.

Changes to accounts and principals are published through the outbox as `keel.account.created`,
`suspended`, `resumed` and `deleted` and `keel.principal.created`, `renamed` (a change of the
login) and `deleted` (each principal removed with its account); `keel.setup.admin` and the
principals of peer links emit nothing. A billing peer with the `identity` role is the source of
truth, and the installations it pairs with apply its suspensions, resumptions, deletions and
renames in the `peers` module, through the same functions a local change calls
([07-ecosystem.md](07-ecosystem.md)).

## Open questions

The following questions are open and change the design depending on the answer.

- Membership cardinality. 1:N at M0 with an additive `keel_membership` later is the proposal; the
  alternative is N:M from the first migration. If a billing product with "a company of three
  employees" is planned within the first year, N:M from the start is the better choice. One tree
  serving a hosting product and a billing product on the same host makes the question more
  pressing.
- Reseller depth. An arbitrary chain of accounts by `path`, as proposed, or exactly one reseller
  level. The answer shapes the UI for granting permissions and limits.
- Account kinds across products. With one tree an account carries the kind of the product that
  created it; whether a kind should become a set, or be replaced by a per-account product
  entitlement, is open.
- Product-scoped administrators. `admin` sees every enabled product and no grant limits an
  administrator to one product on a shared installation; fine-grained keys can be withdrawn per
  product, but a product-scoped `admin` would need a new mechanism.
- Level labels inside a product's own screens when several products are enabled: platform labels
  everywhere is the rule; a product's labels inside its own screens is the alternative.
