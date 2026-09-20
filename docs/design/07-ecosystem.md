# Products working together

This document covers how Keel products cooperate at runtime: the surface the kernel guarantees to
every caller so that one product can drive another, how two instances establish trust (pairing,
`kpt_` tokens, trust-on-first-use by instance key, rotation), how a peer acts on behalf of a
customer and hands that customer over with a one-click login, how customer identity and its
events flow from an identity peer, how events are delivered (signed webhooks and a pull feed with
inbox deduplication), how a product exposes services to a future billing product (`ServiceKind`
and `svc.*`), and how a hosting product reaches DNS through `DnsProvider`: in-process when both
products are enabled on one installation, through a peer when DNS lives on another host.
Everything described here is implemented by system modules and libraries on the public SDK
([05-modules.md](05-modules.md)); the kernel itself knows nothing about peers, services or tariffs.

## What the kernel guarantees

Every product exposes the same surface, so a neighbour written against one product works against
any other. The pieces a neighbour relies on:

| Guarantee | Where | What it gives a neighbour |
|---|---|---|
| One call protocol | `POST /api/v1/call/{func}` | the same envelope for HTTP, the local socket, plugins and peers |
| Envelope with i18n keys | every response | errors carry keys and field paths, not rendered text; the caller renders them in its own language and can map them onto its own form |
| Descriptors | `GET /api/v1/functions` | a neighbour can render a form or a list from a foreign schema |
| `ListQuery` / `Page` | every `kind: list` function | one paging, sorting and filter contract |
| ULID identifiers | every `id` | ids that stay stable across instances, import and export |
| Instance handshake | `GET /api/v1/instance` | `protocol`, the instance public key and the list of service kinds |
| `Idempotency-Key` | request header, handled by the kernel | safe retries over the network |
| Events of one form | outbox | one event shape for the in-process bus, the outbox, webhooks and the pull feed |

The envelope, error codes and event form are specified in [10-contracts.md](10-contracts.md); the
dispatcher pipeline that produces them is in [02-functions.md](02-functions.md).

Idempotency is a kernel concern, not a module convention. The kernel keeps
`keel_idempotency(principal_id, key, func, request_hash, status, response_json, created_at,
expires_at)`; a repeat with the same key and the same body is replayed with the header
`X-Keel-Idempotent-Replay`, a repeat with a different body is refused with `409`
(`idempotency_mismatch`). `keel-client`, the Rust client for calling a neighbour with `keel-types`
types, retries only requests that carry an `Idempotency-Key`. For other languages the client is
generated from the OpenAPI document that the kernel derives from the function registry.

Products enabled on one installation are not peers. They share the process, the database, the
account tree, the principal set, the session and the outbox ([03-access.md](03-access.md),
[09-operations.md](09-operations.md)) and reach each other in-process through services and
`call.invoke`, never through pairing. Everything in this document is for installations on
different hosts: a billing installation elsewhere, a DNS installation on its own server. A peer
may carry one product or several; its handshake says which.

The endpoints and headers that appear in this document:

| Endpoint or header | Owner | Purpose |
|---|---|---|
| `GET /api/v1/instance` | kernel; contents extended through `InstanceInfoSource` | handshake: `instanceId`, `keel`, `protocol`, `products`, `publicKey`, `features`, `serviceKinds` |
| `POST /api/v1/call/{func}` | kernel | every call, including `peer.*`, `svc.*`, `auth.key.issue`, `keel.account.upsert` |
| `Authorization: Bearer kpt_…` | auth | the peer token of a paired instance |
| `X-Keel-On-Behalf-Of: origin=…; external_id=…` | peers | act as a customer principal that belongs to the calling peer |
| `/login?key=kok_…` | auth, shell | one-click login page; performs a same-origin `POST auth.key.redeem` |
| `POST /api/v1/x/peers/hook` | peers | inbound signed event deliveries |
| `GET /api/v1/x/peers/feed?after=seq` | peers | pull feed of outbox events for instances behind NAT |
| `Idempotency-Key` | kernel | replay protection on retried calls |
| `X-Keel-Idempotent-Replay` | kernel | marks a response that was replayed from the idempotency store |
| `X-Keel-Event-Id`, `X-Keel-Peer`, `X-Keel-Signature` | webhooks | identity and HMAC signature of a webhook delivery |

## Instance identity and the handshake

Every instance has an `instanceId` (kept in `keel_kv`, [04-storage.md](04-storage.md), and written
to `/var/lib/keel/instance.id`, [09-operations.md](09-operations.md)) and an Ed25519 key pair. The
public half is published in the handshake, which is the first thing a neighbour reads:

```
GET /api/v1/instance
{ "instanceId": "01J9B…", "keel": "1.2.0", "protocol": 1, "products": ["dns", "panel"],
  "publicKey": "ed25519:MCowBQYDK2Vw…", "features": ["tasks", "events", "peers", "services"],
  "serviceKinds": ["dns.zones"] }
```

`protocol` is the wire version (`PROTOCOL` in `keel-types`); an instance supports versions N and N-1
([05-modules.md](05-modules.md)). `keel` is the version of the installation and of every product on
it; `products` lists the enabled product ids, so a neighbour that needs DNS looks for `dns` in the
list. Editions are not published in the handshake. The kernel serves the route, but `features` and
`serviceKinds` are assembled from `InstanceInfoSource` contributions: the `svc` module contributes
the service kinds the enabled products offer, so the kernel never learns what a service is.

## Pairing

Trust between two instances is created by pairing with a one-time code. The receiving side issues
the code and, with it, fixes what the other side will be allowed to do; the initiating side
presents the code in a single request that exchanges everything both sides need.

1. On the receiving side an administrator runs `peer.pair_code` with the scopes to grant: masks
   of functions, a level ceiling, whether on-behalf calls are allowed, and a time to live for the
   code.
2. On the initiating side `peer.create` takes the receiver's URL, the code and the list of events
   to subscribe to. It reads the receiver's handshake, then sends one request carrying the code,
   its own instance description, an inbound `kpt_` token with which the receiver may call back,
   the scopes of that inbound token, and a webhook subscription.
3. The receiver answers with its `peerId`, the `kpt_` token the initiator will use, the granted
   scopes and the secret with which deliveries to the initiator's hook will be signed.

```
On B (an installation with `dns` enabled):
keel ctl peer.pair_code scopes='svc.*,auth.key.issue,keel.principal.upsert' level=admin on_behalf=true ttl=10m
→ PAIR-7K2Q-91XD

On A (KeelBill):
POST /api/v1/call/peer.create
{ "url": "https://dns.example:8443", "code": "PAIR-7K2Q-91XD", "subscribe": ["dns.zone.*", "svc.*"] }

A → B: POST /api/v1/x/peers/pair
{ "code": "PAIR-7K2Q-91XD", "instance": { …A… },
  "inboundToken": "kpt_01J9A.…", "inboundScopes": { "functions": ["billing.service.sync"], "level": "service" },
  "webhook": { "url": "https://bill.example/api/v1/x/peers/hook", "events": ["dns.zone.*", "svc.*"] } }

B → A: 200
{ "peerId": "01J9B", "token": "kpt_01J9B.…",
  "scopes": { "functions": ["svc.*", "auth.key.issue", "keel.principal.upsert"], "level": "admin", "onBehalf": true },
  "webhookSecret": "whs_…" }
```

After the exchange each side keeps one row per link. The row is sketched as
`peer_product(id, instance_id, products_json, keel_version, protocol, url, public_key,
inbound_token_hash, inbound_scopes_json, outbound_token_enc, webhook_secret_enc, status,
last_seen_at)`: the token the other side uses to call us is stored as a sha256 hash, like every
other token; the token we use to call them and the webhook secret are stored through the SDK
`Secret` type, encrypted with the key from `secret.key` ([04-storage.md](04-storage.md)). The
one-time codes live in `peer_pair_code` until they are used or expire.

**Trust on first use.** The identity of a peer is its Ed25519 instance key, learned from the
handshake during pairing and pinned in the link. Later contact must present the same key. TLS is
deliberately not the identity: a certificate renewed by ACME or replaced by the operator does not
break the link.

**Rotation and removal.** `peer.rotate` issues fresh tokens for a link and retires the old ones
without re-pairing; `peer.delete` removes the link and revokes its tokens. Because tokens are
opaque values whose hashes live in the database, revocation is immediate: the next call with a
revoked token fails with `401`.

**Compatibility.** The `protocol` of the other side is checked at pairing time and once a day
thereafter.

## Peer tokens and scopes

`kpt_` is one of the four opaque token prefixes of the `auth` module (`ks_` session, `kat_` API
token, `kpt_` peer token, `kok_` one-time login key). The side that verifies a token stores only
its sha256, so that database holds no usable token value and revocation is a row change with
immediate effect. The principal behind a peer token is a `service`-level principal, the level
for integrations without a UI, and it belongs to the root account (`kind: system`, `path: /`)
together with the owner, administrators, API tokens and plugin principals
([03-access.md](03-access.md)).

A token carries three scope facets, fixed when the pair code is issued:

| Facet | Meaning |
|---|---|
| `functions` | masks of function names the token may call (`svc.*`, `auth.key.issue`, …) |
| `level` | the level ceiling for calls made with the token |
| `onBehalf` | whether the token may carry `X-Keel-On-Behalf-Of` |

The dispatcher enforces the token scope in its guards together with level, permission and license
feature ([02-functions.md](02-functions.md)): a token scoped to one function receives `403` on any
other. A peer with the narrowest useful scope is the expected configuration; the pairing example
above grants a billing instance only the service functions, key issuing and principal
synchronisation.

## Acting on behalf of a customer

A peer that manages customers (a billing instance) calls the installation as the customer, not as
an administrator who then reaches into the customer's data. It sends its own `kpt_` token plus
the header

```
X-Keel-On-Behalf-Of: origin=bill:01J9A; external_id=client-42
```

The peers module resolves the header to the principal with that `(origin, external_id)` and the
call runs as that principal with `actor = peer`: the customer's level and permissions apply, the
`OwnedGuard`, account scoping and field stripping behave exactly as if the customer had called.
The intended reading is that the token's `functions` mask applies on top of the customer's
permissions; the design does not spell out how the two combine. The envelope and the audit record
both sides, in the same way `su` records the acting administrator:

```
Authorization: Bearer kpt_01J9B.…
X-Keel-On-Behalf-Of: origin=bill:01J9A; external_id=client-42
→ principal = alice, actor = { kind: peer, id: 01J9A, via: peer }
```

On-behalf is restricted by origin: it resolves only principals and accounts whose `origin` is the
calling peer. Principals created by hand on the installation, or synchronised from a different
peer, are out of reach. The consequence is the point of the rule: a leaked billing token opens no
session under a manually created user, and two identity peers cannot act for each other's
customers. The token must also have been granted `onBehalf` at pairing.

## One-click login

A customer who is already signed in to a peer (the billing account page, for example) should open
the installation without a second password. The peer, usually on behalf of the customer, calls
`auth.key.issue` with the customer's principal, a `ttl` of at most 300 seconds and the path to
land on — a path starting with a single `/`, neither `//` nor a scheme; it receives a one-time
`kok_` key and the relative URL to send the browser to:

```
POST /api/v1/call/auth.key.issue
{ "principal": "01J9…", "ttl": 60, "redirect": "/f/dns.zone.list" }
→ { "key": "kok_…", "url": "/login?key=kok_…" }
```

The target must lie below the caller's level and inside its scope, as for `auth.su`. The `peers`
module of M6 adds `externalId`, resolved against `(origin, external_id)` for the peer that
carries an `origin`, and a public-URL setting makes the `url` absolute.

The browser navigates to `/login?key=…`. That request is a `GET` and does not redeem the key: it
returns a page which, from the installation's own origin, performs `POST auth.key.redeem`. The
detour is required by the CSRF policy: every request without a Bearer token is checked by
`Sec-Fetch-Site` and `Origin` ([10-contracts.md](10-contracts.md)), and a navigation arriving
from the peer's site would fail that check if the navigation itself created the session. The key
is consumed at first use, with one conditional update so that two concurrent redeems end with
one session, and is bound to the `redirect` given at issue time; the redeem answers `{ redirect,
principal, issuedBy }`. The session that results is an ordinary `ks_` session; the audit record
of `auth.key.issue` names the issuer and the target and the row keeps `issued_by`, so opening a
DNS installation from a hosting installation on another host is traceable to that installation.
Between products of one installation there is nothing to redeem: they share the session.

The `auth` module keeps issued keys as `auth_login_key(key_hash, principal_id, issued_by,
redirect, expires_at, used_at, created_at)`. The same mechanism serves the first administrator:
the installer prints a one-time `kok_` link for `keel.setup.admin`
([09-operations.md](09-operations.md)).

## Shared customer identity

The unit of ownership is the account and the person is a principal in it
([03-access.md](03-access.md)). Both rows carry `(origin, external_id)`: a stable identity assigned
by whoever created the record. `keel.account.upsert` and `keel.principal.upsert` are idempotent by
that pair, which is the basis of synchronisation from a billing system: the same customer pushed
twice yields one account, and a service opened for a customer already known yields no duplicate.
Products of one installation share these rows directly; the pair matters only between
installations.

A peer link can carry the role `identity`. The identity peer, the billing instance, is the source
of truth for its accounts and principals. Changes on its side are emitted as standard events,
`keel.account.*` and `keel.principal.*` with the verbs `created`, `suspended`, `resumed`,
`deleted` and `renamed`, and go out through the outbox. On a receiving installation the `peers`
module applies them with its event handler, so an account suspended in billing is suspended on
every paired installation, with each enabled product's `AccountLifecycle` hooks running as they
would for a local change ([03-access.md](03-access.md)).

Single sign-on between products is the `oidc` login method of the `auth` module, the same method
that serves corporate installations. The identity provider may later be the billing product
itself; the design states this as a direction, not as a decided component.

Account membership is one principal to one account at M0; membership of one person in several
accounts, which a billing customer that is a company with several employees needs, is listed as an
open question ([11-decisions.md](11-decisions.md)). The session, token and envelope already carry
the account explicitly so that this can be added without changing the protocol.

## Event delivery

Every change that a neighbour may care about is written to the outbox inside the transaction of
the call that made it ([02-functions.md](02-functions.md)). Delivery reads the outbox at-least-once:
each consumer keeps its own cursor, and retention never discards what has not been delivered.
The event has one form everywhere, on the in-process bus, in the outbox, in a webhook body and in
the pull feed:

```
{ "id": "01J9EV5K…", "seq": 8812, "type": "dns.zone.created", "at": "2026-09-12T10:00:00.000Z",
  "instanceId": "01J9B", "module": "dns",
  "actor": { "kind": "peer", "id": "01J9A" },
  "subject": { "kind": "dns.zone", "id": "01J9Q3…", "accountId": "01J8A…" },
  "data": { "name": "a.org" } }
```

`module` names the module that emitted the event; together with the prefix of `type` and
`instanceId` it identifies the source. An installation carries several products, so an event names
no product. `subject` names the row the event is about, and is what the webhook consumer and the
browser stream scope an event by: a handler names it through `call.emit_about(type, subject,
&data)`; a plain `call.emit` gets a subject the dispatcher derives — `kind` is the function name
without its verb, `id` the key parameter when it is a single id, `accountId` the account the
handler last resolved through `resolve_account`, the caller's account when it resolved none — and
a call whose key is not a single id, a create or a batch over `ids`, gets no derived subject, so
its events reach admins alone unless the handler names one.

There are two ways for an event to reach a peer.

**Webhooks.** The `webhooks` module is the single owner of outbound deliveries, for peers and for
user integrations alike; the subscription for a peer is created at pairing, a user-level
subscription is created from the UI, and the signature format is the same. A delivery is signed
with HMAC-SHA256 over the timestamp and the body using the link's secret; the acceptance window
for the timestamp is ±300 s, `t` being the time of the attempt so a retry carries a new signature,
and the check compares in constant time:

```
POST https://bill.example/api/v1/x/peers/hook
X-Keel-Event-Id: 01J9EV5K…
X-Keel-Peer: 01J9B
X-Keel-Signature: t=1757671200,v1=<hmac_sha256(secret, t + "." + body)>
```

Failed deliveries are retried at 1m, 5m, 30m, 2h and 12h; after the last attempt the delivery is
dead-lettered and surfaced as a problem in the `notify` registry ([05-modules.md](05-modules.md)).
The subscription and delivery state live in the webhooks module's own tables,
`webhook_subscription` (`url`, the sealed `secret`, the event masks, `account_id`, `enabled`) and
`webhook_delivery` with `attempt`, `next_at`, `status` and `last_error`; `X-Keel-Peer` joins the
headers at pairing in M6, a user subscription carries `X-Keel-Event-Id` and `X-Keel-Signature`.

**Pull feed.** An instance behind NAT, which cannot receive a webhook, reads the same outbox from
the other side:

```
GET /api/v1/x/peers/feed?after=8812&types=dns.zone.*&limit=500
→ { "data": [ … ], "next": 9020 }
```

`after` is the outbox `seq`; the consumer stores the last `seq` it processed and asks for what
follows. The feed lives on its own path so that it is not confused with the browser event
stream at `GET /api/v1/events`, which is filtered per principal and resumed by `Last-Event-ID`
([08-frontend.md](08-frontend.md), [10-contracts.md](10-contracts.md)).

**Inbox.** Whichever way an event arrives, the receiver deduplicates it by event `id` in
`peer_inbox(event_id, peer_id, received_at, processed_at)`: an insert that conflicts is ignored, the
hook answers `200` at once, and processing happens asynchronously from the inbox. Module subscribers
on the receiving side get the event through the same bus as local events. A repeated delivery
therefore never creates a duplicate; the M6 acceptance in [../plan.md](../plan.md) tests exactly
that.

## Services for a billing product

Billing is not in this project; it will be built on the same kernel later. The providing side of
the contract is designed now, so that a service can be sold, opened, suspended and measured
through one API regardless of which product provides it.

The `svc` module, a system module rather than a kernel component, collects `ServiceKind`
contributions from product modules and registers for each kind the functions
`svc.<kind>.open`, `.suspend`, `.resume`, `.close`, `.set_params`, `.stat` and `.login`. A
service kind declares three JSON Schemas, `Params`, `State` and `Account`, and a
`schema_version`. The list of kinds is published in `/api/v1/instance` through
`InstanceInfoSource`. The contribution is sketched as a trait:

```rust
#[async_trait] pub trait ServiceKind: Send + Sync + 'static {
    const KIND: &'static str; const SCHEMA_VERSION: u32;
    type Params: JsonSchema + DeserializeOwned + Serialize + Send; type State: JsonSchema + Serialize + Send; type Account: JsonSchema + DeserializeOwned + Send;
    async fn open(&self, call: &Call<()>, req: OpenRequest<Self::Account, Self::Params>) -> keel::Result<Opened<Self::State>>;
    async fn suspend(&self, call: &Call<()>, svc: &Service, reason: Value) -> keel::Result<()>;
    async fn resume(&self, call: &Call<()>, svc: &Service) -> keel::Result<()>;
    async fn close(&self, call: &Call<()>, svc: &Service) -> keel::Result<()>;
    async fn set_params(&self, call: &Call<()>, svc: &Service, params: Self::Params) -> keel::Result<Self::State>;
    async fn stat(&self, call: &Call<()>, svc: &Service, period: Period) -> keel::Result<Value>;
    async fn login(&self, call: &Call<()>, svc: &Service) -> keel::Result<LoginTarget>;
}
```

A consumer reads the catalogue of kinds with their schemas from the `svc` module. An earlier
sketch served it as `GET /api/v1/services`; with `svc` as a module the design does not fix
whether that is a function or a raw route of the module. The entry shape:

```
{ "data": [ { "kind": "dns.zones", "schemaVersion": 2, "title": "dns.service.zones.title",
  "account": { "$ref": "#/$defs/ServiceAccount" },
  "params": { "type": "object", "properties": {
      "zoneLimit": { "type": "integer", "x-ui": { "control": "slider", "unlimited": true } },
      "template": { "type": "string", "x-keel": { "options": "dns.template.options" }, "x-ui": { "control": "select" } } } },
  "state": { "type": "object", "properties": { "zonesUsed": { "type": "integer" } } },
  "functions": { "open": "svc.dns.zones.open", "suspend": "svc.dns.zones.suspend", "resume": "svc.dns.zones.resume",
                 "close": "svc.dns.zones.close", "setParams": "svc.dns.zones.set_params",
                 "stat": "svc.dns.zones.stat", "login": "svc.dns.zones.login" } } ] }
```

### Semantics of the service functions

| Function | Semantics |
|---|---|
| `open` | upserts the account by `(origin, external_id)` from the `account` object, then creates the service with the given `params`; two services of different kinds for one customer land in one account |
| `suspend`, `resume` | hand the service to the kind's handler; the effect on the product's objects is defined by the kind |
| `close` | moves the account to `suspended` with a retention period; nothing is deleted |
| deletion | a separate explicit call, never a side effect of `close`; the account path is `keel.account.delete`, which runs as a task through the lifecycle hooks ([03-access.md](03-access.md)) |
| `set_params` | applies new `Params` and returns the current `State` |
| `stat` | returns usage for a period, the input for billing |
| `login` | returns the login target for the service |

`Idempotency-Key` is required on `open` and `close`. A billing system that retries an order after a
network failure gets the same service back, not a second one; M6 in [../plan.md](../plan.md) is
accepted only when `svc.dns.zones.open` sent twice with one key yields one service.

```
POST /api/v1/call/svc.dns.zones.open   Idempotency-Key: bill-9912-open
{ "externalId": "bill-svc-9912", "account": { "origin": "bill:01J9A", "externalId": "acc-7", "login": "alice", "email": "a@example.com" }, "params": { "zoneLimit": 50, "template": "default" } }
→ { "serviceId": "01J9S…", "accountId": "01J8A…", "status": "active", "state": { "zonesUsed": 0 } }
```

Services are sketched as `svc_service(id, kind, external_id, account_id, status, params_json,
state_json, schema_version, peer_id, UNIQUE(kind, external_id))`: a service is unique per kind and
external id, remembers which peer opened it and the schema version its params were written under.

### How billing refers to a service

A tariff in the billing product references a service by `(peerId, kind, schemaVersion)`. The
form in which an operator fills in the parameters of that tariff is rendered by
`@globalart/keel/forms` from the foreign `Params` schema ([08-frontend.md](08-frontend.md)); the
descriptor and schema vocabulary is the same on every product, which is what makes rendering a
neighbour's form possible. The sketch adds that the consumer caches the schemas it has seen and
that compatible changes to a schema are additive; the sketch implies, without stating it, that
anything else needs a new `schemaVersion`.

## Hosting to DNS: `DnsProvider`

The hosting product needs DNS for the sites it creates, and it must work whether DNS is enabled
on the same installation or runs on another host. The `hosting-web` module therefore declares
the trait it needs, `DnsProvider`, and requires an implementation; the panel bundle's bridge
`Bridge { to: "dns", local: "dns_local", remote: "dns_peer" }` chooses which one at start
([05-modules.md](05-modules.md)):

| Implementation | Where DNS lives | How it reaches it |
|---|---|---|
| `dns-local` | the `dns` product enabled on the same installation | `call.invoke` of `dns.zone.*` inside the hosting call's transaction, under the same principal and account, so the site and its zone commit together and a DNS validation error is a field error of the nested call |
| `dns-peer` | a paired installation with `dns` on another host | `keel-client` calls to the neighbour with `X-Keel-On-Behalf-Of`, after commit, so the zone is owned by the same customer on both sides |

A hosting installation always has one of the two, because the bridge falls to `remote` when
`dns` is not enabled; an installation whose `dns-peer` side has no paired neighbour answers the
hosting functions that need DNS with `unavailable` until one is paired. `keel product disable
dns` on an installation whose `panel` bridges to it locally is refused unless the operator passes
`--switch-remote`, which accepts that hosting DNS operations answer `unavailable` until a peer is
paired.

Inside dns the same abstraction fronts the name server: the `dns-zones` module provides a
`DnsProvider` that applies zones to bind ([10-contracts.md](10-contracts.md)), and `dns.zone.edit`
calls it from `after_commit` ([02-functions.md](02-functions.md)).

Calls to a neighbour, like operations on the host, run only after commit or inside a task; the
transaction of a call never waits on the network ([02-functions.md](02-functions.md)). A failure
reported by the neighbour through `dns-peer` maps to the error code `peer` (`502`) in the caller's
envelope. Because errors travel as i18n keys with field paths, a validation error from the DNS
side can be shown on the matching field of the hosting form; M6 is accepted only when a DNS error
reaches the hosting form as a key on a field. How `dns-peer` reports a failure that happens after
the hosting row has been committed, in the response or through a task, is not fixed by the design.

## Boundaries and sequence

Nodes of one installation are not peers, and neither are products of one installation. A second
server managed by the same installation is a cluster node reached through the host agent over mTLS
([06-host.md](06-host.md)); pairing and `kpt_` tokens are for installations on different hosts.

The ecosystem is deliberately late in the plan: no peers before there is a product to pair
([../plan.md](../plan.md)). M6 delivers the `peers` module (handshake, pairing by code, mutual
`kpt_`, scopes, on-behalf by origin, TOFU, rotation), outbox delivery with signatures and retries,
the pull feed and inbox, `Idempotency-Key` end to end, the `svc` module, `keel-client`, the full
conformance suite, `keel.principal.upsert` by origin and external id with the `keel.account.*`
events, and `panel` reaching `dns` on another host through `dns-peer`. Its acceptance: two
installations on two hosts paired by code; `panel` on one host creating a zone in `dns` on the
other on behalf of the customer with the DNS error shown on the hosting form field; the `dns`
installation opened from the hosting installation by a one-time key with the actor in the audit;
`svc.dns.zones.open` twice with one key giving one service; an event arriving signed and a repeat
creating no duplicate; conformance green against an installation with `dns` alone, with `panel`
alone and with both enabled; a revoked token failing calls with `401`. The same-host case, one
installation with both products enabled and `dns-local`, is accepted at M5.

`keel-conformance` runs against the started `keel` binary with any set of products enabled and
covers, among other areas, idempotency, webhooks and `svc.*`; it is a mandatory CI job of every
product ([05-modules.md](05-modules.md)).

## Left open

These points are within the scope of this document and not decided by the design:

- Whether the catalogue of service kinds with full schemas is a function or a raw route of the
  `svc` module.
- The length of the retention period after `svc.<kind>.close`, and what the daily `protocol`
  compatibility check does on a mismatch.
- Whether external integrators get a REST facade such as `GET /api/v1/dns/zones/{id}` on top of
  the dispatcher, or `/api/v1/call/{func}` with the generated OpenAPI is enough
  ([11-decisions.md](11-decisions.md)).
- One-to-many versus many-to-many account membership, which decides how a billing customer with
  several employees is represented on an installation ([11-decisions.md](11-decisions.md)).
