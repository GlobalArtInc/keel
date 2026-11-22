# Products working together

This document covers how Keel products cooperate at runtime: the surface the kernel guarantees to
every caller so that one product can drive another, how two instances establish trust (pairing,
`kpt_` tokens, trust-on-first-use by instance key with a proof of possession, rotation), how a peer
acts on behalf of a customer and hands that customer over with a one-click login, how customer
identity and its events flow from an identity peer, how events are delivered (signed webhooks and a
pull feed with inbox deduplication), how a product exposes services to a future billing product
(`ServiceKind` and `svc.*`), and how a hosting product reaches DNS through `DnsProvider`:
in-process when both products are enabled on one installation, through a peer when DNS lives on
another host. Everything described here is implemented by system modules and libraries on the
public SDK ([05-modules.md](05-modules.md)); the kernel itself knows nothing about peers, services
or tariffs.

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
| Instance handshake | `GET /api/v1/instance` | `protocol`, the instance public key, a proof of that key over a challenge, the enabled products and the list of service kinds |
| `Idempotency-Key` | request header, handled by the kernel | safe retries over the network |
| Events of one form | outbox | one event shape for the in-process bus, the outbox, webhooks and the pull feed |

The envelope, error codes and event form are specified in [10-contracts.md](10-contracts.md); the
dispatcher pipeline that produces them is in [02-functions.md](02-functions.md).

Idempotency is a kernel concern, not a module convention. The kernel keeps
`keel_idempotency(principal_id, key, func, request_hash, status, response_json, created_at,
expires_at)`; a repeat with the same key and the same body is replayed with the header
`X-Keel-Idempotent-Replay`, a repeat with a different body is refused with `409`
(`idempotency_mismatch`), and a concurrent repeat waits for the first call and is then answered from
what it committed, so the handler runs once ([02-functions.md](02-functions.md)). `keel-client`, the
Rust client for calling a neighbour with `keel-types` types, retries only requests that carry an
`Idempotency-Key`. For other languages the client is generated from the OpenAPI document that the
kernel derives from the function registry.

Products enabled on one installation are not peers. They share the process, the database, the
account tree, the principal set, the session and the outbox ([03-access.md](03-access.md),
[09-operations.md](09-operations.md)) and reach each other in-process through services and
`call.invoke`, never through pairing. Everything in this document is for installations on
different hosts: a billing installation elsewhere, a DNS installation on its own server. A peer
may carry one product or several; its handshake says which.

The endpoints and headers that appear in this document:

| Endpoint or header | Owner | Purpose |
|---|---|---|
| `GET /api/v1/instance` | kernel; contents extended through `InstanceInfoSource` | handshake: `instanceId`, `keel`, `protocol`, `products`, `publicKey`, `features`, `serviceKinds`; with `?challenge=` also `proof` |
| `POST /api/v1/call/{func}` | kernel | every call, including `peer.*` (the guest exchanges `peer.pair` and `peer.renew` among them), `svc.*`, `auth.key.issue`, `auth.key.handover`, `keel.account.upsert`, `keel.principal.upsert` |
| `Authorization: Bearer kpt_…` | auth generates the token, peers authenticates it | the peer token of a paired instance |
| `X-Keel-On-Behalf-Of: origin=…; external_id=…` | the kernel passes it on, peers resolves it | act as a customer principal that belongs to the calling peer |
| `/login?key=kok_…` | auth, shell | one-click login page; performs a same-origin `POST auth.key.redeem` |
| `POST /api/v1/x/peer/hook` | peers | inbound signed event deliveries |
| `GET /api/v1/x/peer/feed?after=&types=&limit=` | peers | pull feed of outbox events for instances behind NAT |
| `Idempotency-Key` | kernel | replay protection on retried calls |
| `X-Keel-Idempotent-Replay` | kernel | marks a response that was replayed from the idempotency store |
| `X-Keel-Event-Id`, `X-Keel-Peer`, `X-Keel-Signature` | webhooks | identity and HMAC signature of a webhook delivery |

The module that owns pairing has the id `peer`, the prefix of its tables, functions, settings and
i18n keys, and the cargo feature `peers`, so its raw routes live under `/api/v1/x/peer/`
([05-modules.md](05-modules.md)).

## Instance identity and the handshake

Every instance has an `instanceId` (kept in `keel_kv`, [04-storage.md](04-storage.md), and written
to `/var/lib/keel/instance.id`, [09-operations.md](09-operations.md)) and an Ed25519 key pair. The
key is 32 random bytes of Ed25519 seed, established at the first start in the same transaction and
under the same lock as the instance id and the data key, sealed under the data key in the `Sealed`
form every `Secret` takes and kept in `keel_kv` under `instance_key`. A restore that drops the
dump's data key, because no local `secret.key` opens it, drops the instance key sealed under it as
well, so a copy restored on another host comes up under the instance id of the dump with a new
instance key, and keeps the dump's links, whose sealed tokens no longer open. Pairing is then
refused on both sides, by a neighbour that knows the id under another key
(`403 peer.error.instance_key`) and by the copy that still holds a link to the neighbour
(`409 peer.error.paired`), so an administrator on each side deletes the stale link with
`peer.delete` before the two pair again; answering to the same id, the copy then reaches the
records the neighbour keeps under origins naming it ("Threat model and review notes"). With the
key, the identity and the links follow the backup ([09-operations.md](09-operations.md)). The
public half is published in the handshake, which is the first thing a neighbour reads:

```
GET /api/v1/instance
{ "instanceId": "01J9B…", "keel": "1.2.0", "protocol": 1, "products": ["dns", "panel"],
  "publicKey": "ed25519:MCowBQYDK2Vw…", "features": ["peers", "svc"],
  "serviceKinds": ["dns.zones", "hosting.sites"] }
```

`protocol` is the wire version (`PROTOCOL` in `keel-types`); an instance supports versions N and N-1
([05-modules.md](05-modules.md)). `keel` is the version of the installation and of every product on
it; `products` lists the enabled product ids, so a neighbour that needs DNS looks for `dns` in the
list. Editions are not published in the handshake. `publicKey` is `ed25519:` followed by the base64
of the RFC 8410 SubjectPublicKeyInfo, the fixed twelve-byte prefix and the 32 key bytes. The kernel
serves the route outside the authenticating layer, since a neighbour reads it before it holds any
token, and answers `503 keel.error.unavailable` while the composition carries no instance id or
key. It folds the handshake once at freeze and provides it to the modules as the hidden
`InstanceInfo` service; `InstanceInfo` itself is a wire type of `keel-types` and
`contracts/schema`. `features` and `serviceKinds` are assembled from `InstanceInfoSource`
contributions, sorted and without duplicates: the `peers` module contributes the feature `peers`,
the `svc` module the feature `svc` and the name of every service kind the enabled products offer,
so the kernel never learns what a feature or a service is.

A neighbour that must know it talks to the holder of the key asks with a challenge. `GET
/api/v1/instance?challenge=<c>`, a challenge of 32 to 128 letters, digits, `-` and `_`, answers the
handshake with `proof`: `ed25519:` and the base64 of a signature of the instance key over
`keel-instance-proof`, a line break and the JSON array `[challenge, instanceId, publicKey, keel,
protocol, products]`.

```
GET /api/v1/instance?challenge=3f9c…e1
{ "instanceId": "01J9B…", "keel": "1.2.0", "protocol": 1, "products": ["dns"],
  "publicKey": "ed25519:MCowBQYDK2Vw…", "features": ["peers", "svc"], "serviceKinds": ["dns.zones"],
  "proof": "ed25519:k3Jd…" }
```

The proof covers everything a link records of the handshake, its identity, version, protocol and
products; `features` and `serviceKinds`, which no link keeps, stay unsigned. The handshake with its
proof is `ProvenInstance`, which joins `keel-types` and `contracts/schema` like every other wire
type. A malformed or repeated challenge answers `422 validation` with `keel.error.challenge` on
`challenge`; the handshake without a challenge is unchanged.

## Pairing

Trust between two instances is created by pairing with a one-time code. The receiving side issues
the code and, with it, fixes what the other side will be allowed to do; the initiating side
presents the code in a single request that exchanges everything both sides need.

1. On the receiving side an administrator runs `peer.pair_code { scopes, level, onBehalf, ttl }`
   with the scopes to grant: masks of functions, a level ceiling (`service` by default), whether
   on-behalf calls are allowed (`false` by default) and a time to live for the code in seconds, 60
   to 86400, 600 by default. It answers `{ code, expiresAt }`, the code shown once.
2. On the initiating side an administrator runs `peer.create { url, code, subscribe, pull,
   identity, scopes, level, onBehalf }` with the receiver's address, the code, the masks of the
   events to subscribe to, and the scopes it grants back to the token the receiver will call it
   with (no functions, `service` and no on-behalf by default). It reads the receiver's handshake,
   then calls the receiver's guest function `peer.pair` with the code, its own instance
   description, an inbound `kpt_` token with which the receiver may call back, the scopes of that
   inbound token, its own address and the subscription.
3. The receiver answers with its `peerId`, the `kpt_` token the initiator will use, the granted
   scopes and, when the initiator gave an address and masks, the secret with which deliveries to
   the initiator's hook will be signed.

```
On B (an installation with `dns` enabled):
keel ctl peer.pair_code scopes='["svc.*","auth.key.issue","keel.principal.upsert"]' level=admin onBehalf=true ttl=600
→ { "code": "PAIR-7K2Q-91XD", "expiresAt": "…" }

On A (KeelBill):
POST /api/v1/call/peer.create
{ "url": "https://dns.example:8443", "code": "PAIR-7K2Q-91XD", "subscribe": ["dns.zone.*", "svc.*"],
  "scopes": ["billing.service.sync"], "level": "service" }

A → B: POST /api/v1/call/peer.pair
{ "code": "PAIR-7K2Q-91XD", "instance": { …A… }, "url": "https://bill.example",
  "inboundToken": "kpt_…", "inboundScopes": { "functions": ["billing.service.sync"], "level": "service", "onBehalf": false },
  "subscribe": ["dns.zone.*", "svc.*"] }

B → A: 200
{ "peerId": "01J9B…", "token": "kpt_…",
  "scopes": { "functions": ["svc.*", "auth.key.issue", "keel.principal.upsert"], "level": "admin", "onBehalf": true },
  "webhookSecret": "whs_…" }
```

The exchange is a function rather than a raw route: `peer.pair` is a guest write action on the call
route, so it gets the guest rate limit, schema validation, a transaction and an audit record with
the code and the tokens masked, and a raw route of the module would live under `/x/peer/` anyway.
The code is `PAIR-XXXX-XXXX`, eight symbols of a 32-symbol alphabet without `I`, `L`, `O` and `U`,
forty bits from the operating system's generator, stored as the SHA-256 of the trimmed, uppercased
code and consumed by `DELETE … RETURNING` inside the pairing transaction: a used code is gone, a
pairing that fails afterwards rolls back and leaves it usable, and an expired code is refused and
pruned by the daily check. `peerId` is the receiver's instance id, which the initiator checks
against the handshake it read, together with the form of the token. Every grant, the code's and
the one `peer.create` sends back, checks its masks, keeps its level between `service` and the lower
of the caller's level and `admin`, and must fit inside the caller's own scope
(`peer.error.scope`), so a token cannot pair a link wider than itself.

`url` is the initiator's base address, its `auth.shell_url`, which the receiver also keeps for the
handshake of its daily check and for a rotation from its side. An initiator without a valid
`auth.shell_url` counts as behind NAT and sends no address; `peer.create { pull: true }` says "the
receiver cannot reach me" and sends none either. The receiver makes a webhook subscription to
`<url>/api/v1/x/peer/hook` and issues the secret, `whs_` and 32 random bytes as hex, only when the
initiator sent both an address and masks and the hook address passes the check
`webhook.subscription.set` makes of a URL: it resolves the name, and while
`webhook.private_targets` is off it issues nothing for a plain `http` address, an IP literal or a
name resolving to a private network; a name that does not resolve gets nothing whatever the
setting. Without a secret the initiator reads the pull feed instead. Masks follow the webhook rule,
`module.*` or a dotted event name, at most 64 of them.

`peer.create` and `peer.rotate` are read-mode functions, so the transaction of a call never waits on
the network: they do the exchange first and then write through the hidden `Caller` into the
internal functions `peer.link` and `peer.rekey`, which run as the caller with the scope widened to
full, as a nested invoke would, in one transaction, and are audited. Before any network call they
refuse a read-only token and a read-only or unlisted lease, as the dispatcher would for a write. If
the initiator's write fails after the receiver committed, the receiver keeps a link the initiator
lacks, which its administrator deletes before pairing again. Pairing refuses the installation
itself (`peer.error.self`), an instance already paired (`409 peer.error.paired` for the same key,
`403 peer.error.instance_key` for another), a protocol out of reach (`409 peer.error.protocol`) and
an address that is not an `http` or `https` base with a host and without credentials, query or
fragment; two pairings racing for one instance end with one link and the same refusal as in
sequence. A refusal of the other side surfaces as `peer` (`502`) carrying its key, params and field
errors, so a bad code lands on the code field; a transport failure answers
`peer.error.unreachable`, an answer that is not a handshake `peer.error.handshake` and an
unreadable one `peer.error.answer`. The outgoing client of the module follows no redirect, gives a
connection 10 seconds and an exchange 30, reads at most 1 MiB of an answer and 8 MiB of a page of
the pull feed, and verifies TLS against the roots of the operating system
([09-operations.md](09-operations.md)).

After the exchange each side keeps one row per link,
`peer_product(id, instance_id, products_json, keel_version, protocol, url, public_key,
inbound_token_hash, inbound_scopes_json, outbound_token_enc, webhook_secret_enc, status,
last_seen_at, created_at, principal_id, events, feed_seq, pull_seq, identity)`: the token the other
side uses to call us is stored as a sha256 hash, like every other token; the token we use to call
them is stored through the SDK `Secret` type, sealed under the data key
([04-storage.md](04-storage.md)). `url` and `webhook_secret_enc` are nullable, since an initiator
behind NAT has neither a reachable address nor a hook, and `webhook_secret_enc` is the secret that
verifies the deliveries this installation receives, which the initiator alone keeps: the receiver
keeps its signing secret in the webhook subscription of the link and nowhere else.
`principal_id` names the link's principal, `events` the masks the peer subscribed to (set on the
receiving side alone), `feed_seq` the outbox `seq` at pairing, below which the feed never serves,
`pull_seq` the cursor of a link the initiator pulls, and `identity` the role of "Shared customer
identity". The one-time codes live in `peer_pair_code(id, code_hash, scopes_json, expires_at,
created_by, created_at)` until they are used or expire. The only status is `active`.

The module's functions are `peer.list` and `peer.get` (admin, never answering a token hash, a
sealed token or a secret), `peer.pair_code`, `peer.create`, `peer.rotate` and `peer.delete`, the
guest exchanges `peer.pair` and `peer.renew`, and the internal writes `peer.link` and `peer.rekey`;
they sit behind the menu node `system/settings/peers`, and the events `peer.created`, `peer.rotated` and
`peer.deleted` refresh the list.

**Trust on first use.** The identity of a peer is its Ed25519 instance key, pinned in the link at
pairing: the initiator pins the key the handshake published, the receiver the key in the instance
description of the request. Pairing asks for no proof, as trust on first use does. Every later
contact must present the same key: a second pairing answers `403 peer.error.instance_key`, and
`peer.rotate` and the daily check read the handshake with a fresh challenge of 32 random bytes as
hex and take it only when its proof verifies under the presented key, which must be the pinned one.
A server presenting the pinned key without a valid proof answers `403 peer.error.proof {
instanceId, url }` before `peer.renew` carries any token; the daily check raises the warning
`peer.problem.proof` and records nothing of that handshake, and clears it with
`peer.problem.instance_key` once a proof verifies. TLS is deliberately not the identity: a
certificate renewed by ACME or replaced by the operator does not break the link.

**Rotation and removal.** `peer.rotate` issues fresh tokens for a link and retires the old ones
without re-pairing. Either side that holds the other's address may start it, otherwise it answers
`409 peer.error.no_url`: it makes the new token the other side will call it with and sends it,
with its current token in the body, to the guest function `peer.renew { token, inboundToken }`,
which needs no scope mask to admit it; the other side swaps both tokens in one transaction and
answers its own new token, which `peer.rekey` stores. The old tokens stop working at once, and a
rotation cut off between the two commits needs a re-pair. `peer.delete` removes the link with its
webhook subscription and the deliveries not yet sent, its inbox and its identity rows, and clears
its three problems (`peer.problem.instance_key`, `peer.problem.proof`, `peer.problem.protocol`)
after commit. Because tokens are opaque values whose hashes live in the database, revocation is
immediate: the next call with a revoked token fails with `401`. Whenever a link's inbound token is
retired, by `peer.delete`, `peer.renew` or `peer.rekey`, every other credential of the link's
principal is retired in the same transaction through the `auth` module: its API tokens, the sessions
where it is the principal or the actor, the unused login keys issued for it or by it, its login
challenges and mail tokens and its stored password, and with them every credential minted through
the link ("One-click login"). The principal row stays, with no credential left. A rotation is
included because a rotation after a leak would otherwise leave alive what the leaker minted.

**Compatibility.** The `protocol` of the other side is checked at pairing time and once a day
thereafter; two versions are compatible when they differ by at most one, since each side speaks N
and N-1. Pairing refuses an incompatible peer with `409 peer.error.protocol`. The daily cron job
`peer.check`, at 03:40, reads the proven handshake of every link with an address: another key
raises the warning `peer.problem.instance_key` and a key without a valid proof `peer.problem.proof`,
both recording nothing of that handshake; a proven one clears the two and records the products, the
version, the protocol and `last_seen_at`, and its protocol raises the warning
`peer.problem.protocol` about the link when it is incompatible and clears it otherwise; an
unreachable peer is logged without a problem. The same job prunes expired codes, the processed
inbox rows older than seven days and the rows of links that are gone, and clears the open problems
of links that are gone.

## Peer tokens and scopes

`kpt_` is one of the four opaque token prefixes of the `auth` module (`ks_` session, `kat_` API
token, `kpt_` peer token, `kok_` one-time login key). `auth` generates it, 32 random bytes as hex
like a `kat_` token, and keeps its own authenticator to `ks_` and `kat_`; `peers` contributes the
authenticator that finds a link by the hash of a `kpt_` token in `inbound_token_hash`. The side
that verifies a token stores only its sha256, so that database holds no usable token value and
revocation is a row change with immediate effect.

The principal behind a peer token is the link's own stored principal, created in the pairing
transaction through the hidden `PrincipalSource::create`: level `service`, the level for
integrations without a UI, the login `peer-<link id>`, the role `peer`, in the root account (`kind:
system`, `path: /`) together with the owner, administrators, API tokens and plugin principals
([03-access.md](03-access.md)).

A token carries three scope facets, fixed when the pair code is issued:

| Facet | Meaning |
|---|---|
| `functions` | masks of function names the token may call (`svc.*`, `auth.key.issue`, …) |
| `level` | the level the token acts at, which is also the ceiling of a customer it acts for |
| `onBehalf` | whether the token may carry `X-Keel-On-Behalf-Of` |

A call with a peer token runs as the link's principal at the granted level, at most `admin`, with
`scope` the peer scope and `via` the link (`Peer(<link id>)`); since the stored principal is
`service`, the level facet is the level it acts at rather than a minimum with the stored level. The
dispatcher enforces the token scope in its guards together with level, permission and license
feature ([02-functions.md](02-functions.md)): a token scoped to one function receives `403` on any
other. A token created under a peer token defaults to the peer's masks, and a scope of function
masks contains a peer scope whose masks it covers, so nothing a peer derives is ever a full or a
read-only scope. A peer with the narrowest useful scope is the expected configuration; the pairing
example above grants a billing instance only the service functions, key issuing and principal
synchronisation, and "Hosting to DNS" names what a panel needs.

## Acting on behalf of a customer

A peer that manages customers (a billing instance) calls the installation as the customer, not as
an administrator who then reaches into the customer's data. It sends its own `kpt_` token plus
the header

```
X-Keel-On-Behalf-Of: origin=bill:01J9A…; external_id=client-42
```

The header names `origin=<name>:<instance>` and `external_id=<id>`, each key exactly once, in
either order, separated by `;` with whitespace allowed around each part: the name follows the rule
of a module id, a lowercase ASCII letter followed by up to 63 lowercase letters, digits and `_`; the
instance is a canonical ULID; the external id is 1 to 256 visible ASCII characters without `"`,
`,`, `;` and `\`. Anything else answers `422 peer.error.header`. The kernel reads the header and
passes it on as sent, repeated values joined with `, `, to the hidden `OnBehalf` contributions once
the credential is authenticated, and the first that answers supplies the principal of the call; a
header no contribution accepts, which is the case of any credential other than a peer token granted
`onBehalf` and of a request without one, answers `403 keel.error.on_behalf`. The `peers` resolver
declines a token without `onBehalf` before it parses the header or looks anything up, so such a
token probes neither the grammar nor the external ids. The same resolution serves the call route,
the raw routes, the event stream and the function streams, and a stream re-resolves the header at
each keep-alive and closes on a refusal.

On-behalf is restricted by origin. An origin names the calling peer when its instance part equals
the instance id pinned for the link; the name part is a label the peer chooses, by convention the
product that owns the customer, so one peer may hold several origins but never another peer's, and
a foreign instance answers `403 peer.error.origin` with the origin. The principal is looked up by
its exact `(origin, external_id)`, and its account's origin must name the same peer. Principals
created by hand on the installation, synchronised from a different peer, or placed in an account
that is not the peer's all answer the same `404 peer.error.customer { origin, externalId }`, so the
answer does not tell which case applied. The consequence is the point of the rule: a leaked billing
token opens no session under a manually created user, and two identity peers cannot act for each
other's customers.

The call runs as the customer: its level, its stored grants, its language and time zone, the
`OwnedGuard`, account scoping and field stripping behave exactly as if the customer had called, and
the customer's account state is not checked, as for a session. The customer principal also carries
the peer scope, so the token's function masks apply on top as an intersection checked at entry like
every token scope, and the token's level is a ceiling: a customer above it answers `403
keel.error.token_level`. The actor is the principal of the link exactly as the peer token
authenticated it, its login `peer-<link id>`, the token's level and `via: peer`, and the customer
carries the same `via`; in the envelope `meta.principal.actor` reads `{ id, login, level, via:
"peer" }`, the shape of the actor of `su`, with no `kind` member and the id of the link's principal
rather than the peer's instance id:

```
Authorization: Bearer kpt_…
X-Keel-On-Behalf-Of: origin=bill:01J9A…; external_id=client-42
→ principal = alice, actor = { id: 01J9L…, login: peer-01J9L…, level: admin, via: peer }
```

In the audit the principal is the customer, `actor_principal_id` and `actor_login` name the link's
principal, and `via` is `peer:<link id>`, the way `su` records the acting administrator.

## One-click login

A customer who is already signed in to a peer (the billing account page, for example) should open
the installation without a second password. The peer, on behalf of the customer, asks for a
one-time `kok_` key for a path to land on — a path starting with a single `/`, neither `//` nor a
scheme — with a `ttl` of at most 300 seconds, and receives the key and the URL to send the browser
to:

```
POST /api/v1/call/auth.key.handover          (on behalf of the customer)
{ "ttl": 60, "redirect": "/f/dns.zone.list" }
→ { "key": "kok_…", "url": "https://dns.example:8443/login?key=kok_…" }
```

`auth.key.handover { ttl, redirect }` is a write action at the `user` level without a permission
that issues the key for the caller itself. It answers a call on behalf of a customer alone, one
whose actor is the principal of a link, and `403 auth.error.handover` any other caller, the link
calling as itself and a token it minted included; the customer lies below the level granted to the
link or the answer is `403 auth.error.not_below`; the key's `issued_by` is the principal of the
link. The trail of the installation therefore records the customer as the principal and the link
as the actor of the key, the redeem answers that principal as `issuedBy`, and the session is an
ordinary one of the customer without an actor. The redeem is a guest call, so its audit row names
no principal and records no result, as for any one-click login: the session reaches the link
through the `issued_by` of its key and the key's audit row alone. A call through a peer, as the link
or for a customer, gets the `url` absolute under the installation's public address,
`<auth.shell_url>/login?key=…`, when that setting holds an `http` or `https` address, and the
relative path otherwise; local callers keep the relative path the shell navigates to.

The administrator's tool beside it is `auth.key.issue { principal | externalId, ttl, redirect }`,
which issues a key for another principal: the target must lie below the caller's level and inside
its scope, as for `auth.su`, and exactly one of `principal` and `externalId` is given
(`auth.error.key_target` otherwise). `externalId` is accepted only from a caller that arrived
through a peer link and is refused with `auth.error.external_id` for any other. Whenever
`auth.key.issue` is called through a link, as the link or for a customer, the target is a principal
whose origin and whose account's origin name the peer's instance, whatever the origin's name, or
the answer is the `not_found` an unknown id gets; an `externalId` the link sends as itself is
resolved against every origin naming the peer's instance and refused when two principals match,
while one sent on behalf of a customer is resolved against the customer's own origin. Keeping the
login of a customer as a call of the link was the other choice for a handover: its trail names the
link alone without an actor, and the link would need `auth.key.issue` over every customer of the
peer.

The browser navigates to `/login?key=…`. That request is a `GET` and does not redeem the key: it
returns a page which, from the installation's own origin, performs `POST auth.key.redeem`. The
detour is required by the CSRF policy: every request without a Bearer token is checked by
`Sec-Fetch-Site` and `Origin` ([10-contracts.md](10-contracts.md)), and a navigation arriving
from the peer's site would fail that check if the navigation itself created the session. The key
is consumed at first use, with one conditional update so that two concurrent redeems end with
one session, and is bound to the `redirect` given at issue time; the redeem answers `{ redirect,
principal, issuedBy }`. The session that results is an ordinary `ks_` session; the audit record
of the key's issue names the issuer and the target and the row keeps `issued_by`, so opening a
DNS installation from a hosting installation on another host is traceable to that installation.
Between products of one installation there is nothing to redeem: they share the session.

The `auth` module keeps issued keys as `auth_login_key(key_hash, principal_id, issued_by,
redirect, expires_at, used_at, created_at, link_principal_id)`. A credential minted through a link
dies with it: an API token, a login key and the session such a key opens keep the principal of the
link they came through in `link_principal_id` of `auth_token`, `auth_login_key` and
`auth_session`, the link's principal for a call as the link and its actor for a call on behalf of a
customer, and an `su` session takes it from the session it starts in; an API token or a login key
minted with an API token that carries a link carries that link too, since only the peer ever held
that token, so no chain of tokens outlives the link. `peer.rotate` and `peer.delete` retire them
with the link's own credentials, so the session a handover key or `svc.<kind>.login` opened ends
too. Minting under a link takes a shared transaction lock on the link's principal and checks under
it that the credential the call came with is still live, the link still holding the token of the
call or the API token unrevoked and unexpired, and answers `401 keel.error.unauthenticated`
otherwise; retiring takes that lock exclusively before it revokes anything and revokes login keys
before sessions, so a credential minted while its link rotates or is deleted is either retired with
the link or refused, and a key redeemed meanwhile ends with the session it opens. To know which of
its tokens a call came with, `auth` reads `token` of the hidden `Authenticated` and `Client`, the
hash a module keeps the bearer token of the call under, and asks `PeerReach::holds(link, token)`
([05-modules.md](05-modules.md)). One-time keys serve the first administrator as well: the
installer prints a one-time `kok_` link for `keel.setup.admin` ([09-operations.md](09-operations.md)).

## Shared customer identity

The unit of ownership is the account and the person is a principal in it
([03-access.md](03-access.md)). Both rows carry `(origin, external_id)`: a stable identity assigned
by whoever created the record. `keel.account.upsert { origin, externalId, kind?, parent? }` and
`keel.principal.upsert { origin, externalId, login, level, role?, lang?, account: { origin,
externalId, kind?, parent? } }` are idempotent by that pair, which is the basis of synchronisation
from a billing system: the same customer pushed twice yields one account, and a service opened for
a customer already known yields no duplicate. Both are kernel actions at the `reseller` level under
`keel.account.manage` and `keel.principal.manage`, answering `{ id, created }` and `{ id,
accountId, created }`; the principal upsert upserts its account first through `call.invoke` of
`keel.account.upsert`, whose field errors come back under `account`. Their origins and external
ids are 1 to 256 characters of the alphabet of the on-behalf header, `EXTERNAL_PATTERN` of
`keel-types`, so every synced customer stays addressable on behalf. Products of one installation
share these rows directly; the pair matters only between installations. The rules of an existing
record, the reach of the caller and concurrency are in [03-access.md](03-access.md).

The origin of a peer is bound to that peer: the upserts refuse an origin naming an instance,
`<name>:<instance id>`, to every caller below `admin` but the principal of the link to that
instance, whether it calls through its peer token, a token minted under it or as the actor of a
call on behalf of a customer, with `403 peer.error.origin`. `peers` checks it in hooks inside the
transaction of both upserts, so a record under the origin of a peer was placed by that peer or by an
administrator; origins of any other shape stay open to resellers.

A peer link can carry the role `identity`. The identity peer, the billing instance, is the source
of truth for its accounts and principals. Changes on its side are emitted as standard events
through the outbox: `keel.account.created`, `suspended`, `resumed` and `deleted`, and
`keel.principal.created`, `renamed` (a change of the login) and `deleted` (each principal removed
with its account, before its account's event). An account carries no name, so
`keel.account.renamed` is not emitted and `on_rename` keeps no caller; no function suspends a
principal, so `keel.principal.suspended` and `resumed` are not emitted either. The subjects are `{
keel.account, id, accountId: id }` and `{ keel.principal, id, accountId }`, and the data carries no
admin-only member: `{ id, kind, state }` for a created account, `{ id, login, accountId }` for a
created principal, `{ id, login }` for a rename, `{ id, state }` for an account suspended, resumed
or deleted, and `{ id }` for a principal deleted.

Only the initiator receives the events of a link, and one link joins two installations, so the
role is chosen by the initiator, the side that receives the customers: `peer.create { identity:
true }` stores it in `peer_product.identity` and adds `keel.account.*` and `keel.principal.*` to the
subscription, skipping exact duplicates within the 64 masks. For billing to be the identity source
of a DNS installation, the DNS side is the initiator and grants billing the upserts through the
`scopes` of `peer.create`, which reverses the example of "Pairing":

```
On A (KeelBill): keel ctl peer.pair_code scopes='[]' level=admin ttl=600   → PAIR-…
On B (dns):      POST /api/v1/call/peer.create
{ "url": "https://bill.example", "code": "PAIR-…", "identity": true,
  "scopes": ["svc.*", "auth.key.handover", "keel.account.upsert", "keel.principal.upsert"],
  "level": "admin", "onBehalf": true }
```

The account events reach the initiator only when the receiver's code granted a level whose account
scope covers those accounts, that is `admin`. On the receiving side the `peers` module applies them
from its inbox after relaying them onto the bus, and only for events whose module is `keel`:
`keel.account.suspended`, `resumed` and `deleted` and `keel.principal.renamed`. `created` events
are not applied, since records arrive on demand through the upserts, and a principal's `deleted`
follows its account. An event names its record by its id on the identity peer, and the receiver
finds the record whose `external_id` is that id under an origin naming the peer's instance, a
principal's account's origin naming it as well; no match is skipped, several are skipped with an
error in the log. The change is applied through the `Caller` as the link's principal raised to
`admin` with a full scope, calling `keel.account.suspend`, `resume` and `delete` and
`keel.principal.update`, so their guards, audit, events and the after-commit walk of every enabled
product's `AccountLifecycle` run as for a local change, a delete runs as the task of M4 and an
account already `deleting` is not deleted again. `peer_identity(peer_id, subject_kind, subject_id,
seq, applied_at)` keeps the outbox `seq` of the last event applied to each record of the link, so a
delayed older event never brings back an older state; it goes with the link. A refusal is logged and
final, while an internal, unavailable or rate-limited error is tried again within the five attempts
of the inbox.

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
  "actor": { "kind": "peer", "id": "01J9L…" },
  "subject": { "kind": "dns.zone", "id": "01J9Q3…", "accountId": "01J8A…" },
  "data": { "name": "a.org" } }
```

`module` names the module that emitted the event; together with the prefix of `type` and
`instanceId` it identifies the source. `actor` is `{ kind: principal, id }` for a call of a local
principal and `{ kind: peer, id }` for a call made through a peer link, `id` being the principal of
that link whether the link calls as itself or on behalf of a customer; a guest's call emits no
actor, and a change an identity link applies through the `Caller` names the link's principal with
`kind: principal`. An installation carries several products, so an event names no product.
`subject` names the row the event is about, and is what the webhook consumer and the browser stream
scope an event by: a handler names it through `call.emit_about(type, subject, &data)`; a plain
`call.emit` gets a subject the dispatcher derives — `kind` is the function name without its verb,
`id` the key parameter when it is a single id, `accountId` the account the handler last resolved
through `resolve_account`, the caller's account when it resolved none — and a call whose key is not
a single id, a create or a batch over `ids`, gets no derived subject, so its events reach admins
alone unless the handler names one.

There are two ways for an event to reach a peer.

**Webhooks.** The `webhooks` module is the single owner of outbound deliveries, for peers and for
user integrations alike; the subscription for a peer is created at pairing, a user-level
subscription is created from the UI, and the signature format is the same. A delivery is signed
with HMAC-SHA256 over the timestamp and the body using the link's secret; the acceptance window
for the timestamp is ±300 s, `t` being the time of the attempt so a retry carries a new signature,
and the check compares in constant time:

```
POST https://bill.example/api/v1/x/peer/hook
X-Keel-Event-Id: 01J9EV5K…
X-Keel-Peer: 01J9B
X-Keel-Signature: t=1757671200,v1=<hmac_sha256(secret, t + "." + body)>
```

Failed deliveries are retried at 1m, 5m, 30m, 2h and 12h; after the last attempt the delivery is
dead-lettered and surfaced as a problem in the `notify` registry ([05-modules.md](05-modules.md)).
The subscription and delivery state live in the webhooks module's own tables,
`webhook_subscription` (`url`, the sealed `secret`, the event masks, `account_id`, `enabled`, and
for a link `peer_id`, `level` and `floor_seq`) and `webhook_delivery` with `attempt`, `next_at`,
`status` and `last_error`; a delivery of a link carries `X-Keel-Peer`, the instance id of the
sender, beside `X-Keel-Event-Id` and `X-Keel-Signature`. The subscription of a peer belongs to its
link: it is named `peer-<link id>`, owned by the link's principal in the root account, addressed to
`<url>/api/v1/x/peer/hook` of the address the initiator gave, carries the level granted to the link
(at most `admin`), whose account scope the consumer applies, starts after the outbox `seq` of the
pairing, refuses `webhook.subscription.set`, `.toggle` and `.delete` with `409 webhook.error.peer`
and goes with `peer.delete` and its deliveries not yet sent. It obeys `webhook.private_targets` as a
user's subscription does, so a delivery to an address resolving to a private network fails with
`webhook.error.url_private` at each attempt while the setting is off. The receiving side keeps the
secret in this subscription alone and the initiator in its link, so the hook of the side that signs
refuses whatever the other side signs.

**Pull feed.** An instance behind NAT, which cannot receive a webhook, reads the same outbox from
the other side:

```
GET /api/v1/x/peer/feed?after=8812&types=dns.zone.*&limit=500
→ { "data": [ … ], "next": 9020 }
```

`after` is the outbox `seq`; the consumer stores the last `seq` it processed and asks for what
follows. `peer.create` with `pull: true` sends the receiver no address, so no subscription is
created and the initiator reads the feed with the token of its link; an initiator without an address
of its own (`auth.shell_url` unset), or one whose hook address the receiver refused, reads it too.
The feed answers the peer token of a link alone: a session, an API token, a token minted under a
peer token and a call on behalf of a customer get `403 peer.error.feed`, a guest `401`. It serves
this installation's events after the greater of `after` and the `seq` of the pairing that match the
masks the link subscribed to and the optional `types` masks (comma separated or repeated), admitted
by the account scope of the level the link's token was granted; `limit` defaults to 100 and stops at
500, one request scans at most 5000 rows, and `next` is the last `seq` scanned, so a page the masks
emptied still moves the cursor. A malformed `after` or `limit` answers `422 peer.error.query` on its
name and a malformed mask `peer.error.mask`. The pull consumer of the initiator runs every five
seconds, reads each pull link's feed with the outbound token from `pull_seq`, 100 events a page and
up to 50 pages a round, checks the instance and the `seq` range of every event, puts them into the
inbox and moves `pull_seq` only when it still holds the value it read. The feed lives on its own
path so that it is not confused with the browser event stream at `GET /api/v1/events`, which is
filtered per principal and resumed by `Last-Event-ID` ([08-frontend.md](08-frontend.md),
[10-contracts.md](10-contracts.md)).

**Inbox.** Whichever way an event arrives, the receiver deduplicates it per link by event `id` in
`peer_inbox(peer_id, event_id, event_json, received_at, processed_at, attempts)`: an insert that
conflicts is ignored, the hook answers `200 { data: { eventId, duplicate } }` at once, and
processing happens asynchronously from the inbox, which keeps the event as `event_json` until then.
The hook looks the link up by `X-Keel-Peer` and checks the signature before it reads the body: a
missing header, an unknown link, a link without a secret, a malformed header, a stale time and a
wrong signature all answer the same `401 peer.error.signature`. A refused signature spends one
request from a bucket the hook keeps per address, apart from the guest bucket of `auth` and
refilled each minute to `auth.rate.guest_per_minute`, and an address with none left is answered
`429 auth.error.rate_limited` before its link is looked up or its body hashed; a signed delivery
spends nothing. After the signature the body must be an event whose id equals `X-Keel-Event-Id`,
whose `instanceId` is the instance of the link that signed it and whose `at` lies in the years 0 to
9999 a timestamp holds, or the hook answers `422 peer.error.event` naming the reason, `body`, `id`,
`instance` or `at`; the pull consumer skips such an event with a warning rather than refusing the
page. A link whose inbox holds 10 000 unprocessed events is answered `429 keel.error.rate_limited {
pending }`, which the sender retries along its ladder, and the pull consumer reads no page for it
until the inbox drains below that.

The processor drains the inbox on a wake or every five seconds and relays each event onto the bus
through the kernel's hidden `EventRelay`, which inserts it into `keel_outbox` under its own id with
its `instance_id`, in its own transaction under the outbox lock, answers a repeat as nothing new and
refuses an event of this installation (`keel.error.relay_local`) and an id the outbox holds for
another installation (`keel.error.relay_taken`). Module subscribers on the receiving side get the
event through the same bus as local events, under its own id and `instanceId`. A repeated delivery
therefore never creates a duplicate; the M6 acceptance in [../plan.md](../plan.md) tests exactly
that. An event the bus refuses is dropped with an error in the log; one the bus fails to take is
tried again five seconds later, up to five attempts, before it is dropped the same way, so no single
event holds the inbox, and rows may reach the bus out of order. A delivery, pushed or pulled,
carries only events the sending installation emitted, so an event of a peer never travels on to a
third installation or back.

On the receiving side the account ids of such an event name nothing, since every installation's
root account has the same nil id, so the browser stream and a user subscription carry it only to an
admin or owner whose credential is not narrowed: neither a call through a peer nor a token scoped to
functions, which is all a peer can mint. A user subscription remembers in `local_only` whether the
credential that saved it was narrowed, a peer's subscription is always `local_only`, and the notify
consumer leaves such events alone, since its message events refer to its own rows. The rows of a
link go with `peer.delete`, rows of a link that is gone never enter the bus, and the daily check
prunes processed rows after seven days.

The account scope of a link's level is not the whole rule for the events of a service: an event
about a service of another peer's customer must not reach a link either. The hidden `peer_sees` of
the SDK lets an event whose subject is `keel_sdk::svc::SUBJECT`, `svc.service`, through to a
principal only when `PeerReach::admits` that principal for the origin of the event's account, read
when the event is delivered; the pull feed, the webhook consumer for every subscription with a
principal and the kernel's event stream ask it after their account scope. The feed, the hook and the
stream of a link therefore carry no event about a service of another peer's customer or of an
account no peer owns, and an event whose account is deleted before it is delivered reaches no link;
events about other subjects keep the scope of the link's level.

## Services for a billing product

Billing is not in this project; it will be built on the same kernel later. The providing side of
the contract is built now, so that a service can be sold, opened, suspended and measured through
one API regardless of which product provides it.

The `svc` module, a system module rather than a kernel component, collects `ServiceKind`
contributions from product modules and registers for each kind the functions
`svc.<kind>.open`, `.suspend`, `.resume`, `.close`, `.set_params`, `.stat` and `.login`. A module's
`init` runs before the contributions of the modules after it are known and functions are fixed at
the freeze, so `svc` registers them through the hidden `derive::<ServiceKindDef>(hook)` of the
registrar: after every `init` the loader hands the hook the contributions of every module, and the
hook registers into the `svc` module's own registrar, where the prefix and duplicate checks apply
([05-modules.md](05-modules.md)). A service kind declares three JSON Schemas, `Params`, `State` and
`Account`, a schema version and the i18n key of its title. The name of every kind is published in
`serviceKinds` of `/api/v1/instance` through `InstanceInfoSource`. The contribution is a trait of
`keel_sdk::svc`:

```rust
#[async_trait] pub trait ServiceKind: Send + Sync + 'static {
    const KIND: &'static str; const TITLE: &'static str; const SCHEMA_VERSION: u32;
    type Params: JsonSchema + DeserializeOwned + Serialize + Send; type State: JsonSchema + Serialize + Send; type Account: JsonSchema + DeserializeOwned + Send;
    async fn open(&self, call: &Call<()>, req: OpenRequest<Self::Account, Self::Params>) -> keel::Result<Opened<Self::State>>;
    async fn suspend(&self, call: &Call<()>, svc: &Service, reason: Value) -> keel::Result<()>;
    async fn resume(&self, call: &Call<()>, svc: &Service) -> keel::Result<()>;
    async fn close(&self, call: &Call<()>, svc: &Service) -> keel::Result<()>;
    async fn set_params(&self, call: &Call<()>, svc: &Service, params: Self::Params) -> keel::Result<Self::State>;
    async fn stat(&self, call: &Call<()>, svc: &Service, period: Period) -> keel::Result<Value>;
    async fn login(&self, call: &Call<()>, svc: &Service) -> keel::Result<LoginTarget>;
}

r.contribute(ServiceKindDef::new(ZonesKind));
```

What a module contributes is `ServiceKindDef::new(kind)`, which erases the kind's types and
computes the schemas of its functions. The start refuses, with `svc.error.kind` naming the reason,
a kind whose name is not lowercase segments joined by dots, whose version is not 1 or more, whose
`Account` does not require `origin` and `externalId`, or which two modules contribute. The account
object is the SDK's `ServiceAccount { origin, externalId, kind? }` or a type of the kind that
requires both identity members; a customer's principal is not part of it but synced by
`keel.principal.upsert`, which the pairing example grants.

A consumer reads the catalogue of kinds with their schemas from the function `svc.kinds`, an
`admin`-level read rather than a raw route, so it keeps the guards, the audit, the descriptor and
the OpenAPI entry. Its entries are sorted by kind, each schema without its `$schema`:

```
{ "data": [ { "kind": "dns.zones", "schemaVersion": 1, "title": "dns.service.zones.title",
  "account": { "type": "object", "required": ["origin", "externalId"], "properties": { … } },
  "params": { "type": "object", "required": ["zoneLimit"], "properties": {
      "zoneLimit": { "type": ["integer", "null"], "minimum": 0, "maximum": 100000, "x-ui": { "control": "slider" } },
      "template": { …, "x-keel": { "options": "dns.template.options" }, "x-ui": { "control": "select" } } } },
  "state": { "type": "object", "required": ["zonesUsed"], "properties": { "zonesUsed": { "type": "integer" } } },
  "functions": { "open": "svc.dns.zones.open", "suspend": "svc.dns.zones.suspend", "resume": "svc.dns.zones.resume",
                 "close": "svc.dns.zones.close", "setParams": "svc.dns.zones.set_params",
                 "stat": "svc.dns.zones.stat", "login": "svc.dns.zones.login" } } ] }
```

### Semantics of the service functions

Every function but `open` names its service by `externalId` within the kind. The functions are
`admin`-level without a permission key and without an ownership guard; `stat` is a read and the
others are writes, and `open` and `close` require an `Idempotency-Key`. They answer `{ serviceId,
accountId, status, state, retainUntil? }`, except `stat { externalId, from, to }`, which answers `{
serviceId, from, to, usage }`, and `login`, which answers `{ url }`.

| Function | Semantics |
|---|---|
| `open` | upserts the account by `(origin, externalId)` through `call.invoke` of `keel.account.upsert`, so its guards and hooks, the origin binding of `peers` among them, apply and its field errors land under `account`, a new account going under the caller's account; then inserts the service and runs the kind's `open`; two services of different kinds for one customer land in one account; a second `open` of one external id under another key answers `409 svc.error.exists`, since the external id is unique per kind across peers, which tells any caller that the id is taken and nothing more |
| `suspend`, `resume` | hand the service to the kind's handler; a suspend of a suspended service or a resume of an active one answers it unchanged without calling the kind; the effect on the product's objects is defined by the kind |
| `close` | calls the kind's `close`, marks the service `closed` and keeps it for `svc.retention_days` (1 to 3650, 30 by default) as `retainUntil`, which nothing acts on; suspends the account through `keel.account.suspend` only when no other service of the account remains open, so closing one of two services keeps the other working; nothing is deleted |
| deletion | a separate explicit call, never a side effect of `close`; the account path is `keel.account.delete`, which runs as a task through the lifecycle hooks ([03-access.md](03-access.md)), and the account's deletion forgets its services |
| `set_params` | replaces the `Params` whole, raises the stored schema version to the kind's and returns the current `State` |
| `stat` | returns usage for a period that ends after it starts (`422 svc.error.period` on `to` otherwise), the input for billing; it also answers on a closed service |
| `login` | returns the login target for the service |

The statuses are `active`, `suspended` and `closed`. A closed service refuses every function but
`stat` with `409 svc.error.closed`, and an unknown external id answers `404 svc.error.not_found`.

The account's suspension belongs to whoever made it. When `close` suspends the account and the
suspend changed its state, the closed row carries `suspended_account`; `open` undoes that
suspension and no other: it resumes the account through `keel.account.resume` when the account is
suspended and one of its services carries the mark, so a customer who buys again within the
retention period gets an account that works, and otherwise never changes the account's state. `svc`
hooks `keel.account.suspend` and `keel.account.resume` inside their transaction and drops the
account's marks on every call, its own included, before the kernel switches the state, so the
suspension an administrator or an identity peer makes, before the last close or after it, and a
resume followed by a new suspension, all take the suspension from `svc`: the account stays
suspended, the new service answers `active` on it, and a peer whose token lacks
`keel.account.resume` cannot lift such a suspension by closing the last service and opening
another. `open` and `close` hold a transaction lock keyed by the account's `(origin, externalId)`,
`open` before the upsert and `close`, which reads the pair through the hidden `AccountSource`,
before anything touches the account, and the hook takes the same lock before it touches the marks,
so the three take the lock and then the rows in one order: an `open` and a `close` cannot both miss
the last open service, and a suspend or resume of the account waits for them instead of
deadlocking.

A call that comes through a peer link, its principal or its actor being the principal of a link,
reaches only the services whose account's origin names that link's instance; any other answers
`404 svc.error.not_found`, as an unknown one does, so one billing peer can neither act on nor read
another's services. `svc` asks it of `PeerReach::admits(principal, origin, db)`, which `peers`
answers by the rule of its origin binding, and asks only when `peers` provides it. The module emits
`svc.service.opened`, `suspended`, `resumed`, `closed` and `updated` about `{ svc.service, id,
accountId }` with the data `{ id, kind, externalId, status }`, and the same rule holds where those
events leave the installation ("Event delivery").

`Idempotency-Key` is required on `open` and `close`. A billing system that retries an order after a
network failure gets the same service back, not a second one; M6 in [../plan.md](../plan.md) is
accepted only when `svc.dns.zones.open` sent twice with one key yields one service.

```
POST /api/v1/call/svc.dns.zones.open   Idempotency-Key: bill-9912-open
{ "externalId": "bill-svc-9912", "account": { "origin": "bill:01J9A…", "externalId": "acc-7" },
  "params": { "zoneLimit": 50, "template": "01J9T…" } }
→ { "serviceId": "01J9S…", "accountId": "01J8A…", "status": "active", "state": { "zonesUsed": 0 } }
```

Services are kept in `svc_service(id, kind, external_id, account_id, status, params_json,
state_json, schema_version, peer_id, retain_until, suspended_account, created_at, updated_at,
UNIQUE(kind, external_id))`: a service is unique per kind and external id, remembers the link a
call came through, set for a peer token and an on-behalf call and not for a token minted under a
link, and the schema version its params were written under.

### The kinds of the first products

`dns-zones` contributes `dns.zones` (title `dns.service.zones.title`, version 1, account
`ServiceAccount`) with `Params { zoneLimit, template }` and `State { zonesUsed }`, and `hosting-web`
contributes `hosting.sites` (title `hosting.site.service.title`) with `Params { siteLimit }` and
`State { sitesUsed }`. A limit member is required and nullable, an integer from 0 to 100000 or
`null` for unlimited, with `x-ui.control: slider`, whose unlimited switch the forms library reads
from the nullability. `template` is the id of a template of `dns.template.options`, and leaving it
out clears the default, since `set_params` replaces the parameters whole. The kinds store the
account's `dns.zones` or `hosting.site.max` limit through the hidden `LimitSource::store`, the
effective limit staying the tightest along the path ([03-access.md](03-access.md)). `dns-zones`
keeps `dns_account(account_id, template, updated_at)`, the account's default template, which
`dns.zone.create` applies to a zone created without one, an explicit template winning; `open` and
`set_params` accept a template only when the account itself sees it, a platform template or one of
its own, through `ZoneTemplates::exists`, and answer `422 dns.service.error.template` on
`params.template` otherwise; a default deleted later fails a zone creation on `template` like a
chosen one. The form of `dns.zone.create` offers `ip` and `ip6` whether or not a template is picked,
since the account's default may apply to a zone created without one, and a record of the applied
template whose placeholder has no value is left out.

Each service keeps a row of its kind, since `svc` lets one account hold several services of a kind
and keeps the account active while one of them stays open: `dns-zones` keeps
`dns_service(service_id, account_id, seq, zone_limit, template, held, closed, updated_at)` and
`hosting-web` `hosting_site_service(service_id, account_id, seq, site_limit, held, closed,
updated_at)`, `seq` numbering the rows in the order the services were opened. `open` inserts the
row unheld with the service's parameters, `set_params` replaces them, `suspend` holds the row,
`resume` releases it and `close` holds it and marks it closed. After writing its row each of the
five settles the account from the rows: the account's limit and default template are those of its
newest active service, or, while none is active, of its newest suspended one, and when every
service of the kind is closed the account keeps what it last had. Closing or suspending the newer of
two services thus gives the account back the terms of the older, and a change of package that opens
the new service before it closes the old one moves to the new terms at once; the sum or the maximum
of the limits would count an old package alongside the one replacing it and leave the template
without a rule. A row is written and the account settled under a lock per account of the kind's
own, `dns_service:` or `hosting_site_service:`, so calls on two services of one account settle one
after the other.

An account's zones and sites are switched off while the account is not active or while it has rows
of the kind and every one of them is held, so suspending or closing one of two services keeps the
objects of the other on, resuming one service brings them back whatever another holds, and an
account no service was ever sold to stays on; refusing a second open service of a kind would break
a change of package. The `on_suspend` and `on_resume` of both modules apply the same rule, so
resuming the account keeps the objects of a suspended service off and resuming the service keeps
those of a suspended account off, and a zone or site created meanwhile starts off. A suspended zone
leaves the index of `named` while its zone file and include stay, in the backup too, `apply` skips
it, and switching it on marks it `pending` and applies it again; `dns.zone.list` shows it in the
column `suspended`. A kind's switch runs after the `svc` call commits, in a transaction of its own
as the lifecycle handlers do, under the lock per account that the limit hook of `dns.zone.create`
or `hosting.site.create` takes, reading the account's state and the holds under it, since inside
the call it would deadlock with a creation waiting for the account row that `open` holds for update
and with a lifecycle handler holding that row while `close` goes on to suspend the account.

`stat` answers `{ zonesUsed }` or `{ sitesUsed }`, the highest sample `stats` holds for the account
in the period, 0 without one, through `keel_sdk::svc::peak`, which counts no bucket reaching outside
the period: it reads the day buckets lying wholly inside it, the hour buckets lying wholly inside
the edges the days leave and the raw samples of what remains, and reads the time after the last
bucket an answer holds at the next finer resolution, since `stats` aggregates a key incrementally
from its last bucket; a window `stats.series` refuses for its size is halved until it answers, and
the edges of a period older than the raw retention are read from the aggregates that fit inside
them. `hosting-web` contributes the hourly per-account collector `hosting.site.count` for it.
`login` answers, through `keel_sdk::svc::login`, the URL of a one-time key that `auth.key.issue`
issues for 60 seconds to the oldest principal of the account carrying the account's origin, landing
on `/f/dns.zone.list` or `/f/hosting.site.list` and absolute through a peer, and `409
svc.error.no_principal` when there is none; a malformed answer of `auth.key.issue` or
`stats.series` is `svc.error.answer`. Deleting the account drops its rows of `dns_account`,
`dns_service` and `hosting_site_service`. No kind emits events of its own; `svc.service.*` covers
its changes.

### How billing refers to a service

A tariff in the billing product references a service by `(peerId, kind, schemaVersion)`. The
form in which an operator fills in the parameters of that tariff is rendered by
`@globalart/keel/forms` from the foreign `Params` schema ([08-frontend.md](08-frontend.md)); the
descriptor and schema vocabulary is the same on every product, which is what makes rendering a
neighbour's form possible. The consumer caches the schemas it has seen; compatible changes to a
schema are additive, and anything else needs a new `schemaVersion`.

## Hosting to DNS: `DnsProvider`

The hosting product needs DNS for the sites it creates, and it must work whether DNS is enabled
on the same installation or runs on another host. The `hosting-web` module therefore declares
the trait it needs, `DnsProvider`, in its `api` and requires an implementation at freeze; the
panel bundle's bridge `Bridge { to: "dns", local: &keel_dns_local::MODULE, remote:
&keel_dns_peer::MODULE }` chooses which one at start ([05-modules.md](05-modules.md)):

```rust
#[async_trait]
pub trait DnsProvider: Send + Sync {
    async fn add_zone(&self, invoke: &Invoke<'_>, after: &AfterCommit<'_>, site: &Id, zone: &SiteZone) -> Result<()>;
    async fn foreign(&self, invoke: &Invoke<'_>, zone: &SiteZone) -> Result<bool>;
    async fn add_records(&self, invoke: &Invoke<'_>, zone: &SiteZone, records: &[ZoneRecord]) -> Result<bool>;
    async fn open(&self, db: &Db, zone: &SiteZone) -> Result<String>;
}
```

`SiteZone { name, account, own_account }` names the zone and the account of the site; `Invoke`
is the closure through which a local provider runs `dns.zone.*` inside the call's transaction, and
`AfterCommit` the one through which a provider registers an after-commit hook; `hosting-web`
generates the site's id before the insert so that `add_zone` can name it. `add_zone` creates the
zone of a site the create wizard asks for; `foreign` answers whether a zone enclosing the name
belongs to another account of this installation, and a site, an alias or a mail domain inside such
a zone is refused (`hosting.site.error.foreign_zone`, `hosting.mail.domain.error.foreign_zone`),
since the name would otherwise pass the site's ACME challenge and receive the mail this host sends
there; `add_records` places the MX and SPF records of a mail domain into the zone of that exact
name within the account and answers `false`, without an error, when there is none; `open` answers
the address the zone of a site opens at.

Two rules keep one customer from taking the names of another on an installation. They compare
accounts only and prove nothing about who registered a domain. Let T be the account a zone or a
mail domain is created for, and a non-root ancestor a proper ancestor of T that is not the root
account, since the root account is the operator and its zones do not open to customers.

- `dns.zone.create` and `dns.zone.check` refuse a zone whose name lies inside a zone of an account
  other than T, unless a non-root ancestor of T owns that zone (`dns.zone.error.inside_foreign_zone`).
  A zone of T itself is fine. A parent of an existing foreign zone may still be created. An
  administrator or an owner is exempt and takes the responsibility.
- Below administrator level `hosting-mail` admits a mail domain only if its name equals a zone of T
  or lies strictly inside a zone of T or of a non-root ancestor of T. The apex of the zone of an
  ancestor and a name covered only by a zone of the root account are refused with
  `hosting.mail.domain.error.unproven`, also when the installation has no `dns` module. A name
  inside a zone of an account that is neither T nor an ancestor of T is refused with
  `hosting.mail.domain.error.foreign_zone`. An administrator or an owner adds any name, subject to
  `foreign` as before.

`hosting-mail` asks `dns.zone.claim` for the second rule: `covered` answers the admission and
`contested` a zone of an account outside the line of T and its ancestors; `foreign` keeps its
meaning for sites.

| Implementation | Where DNS lives | How it reaches it |
|---|---|---|
| `dns-local` | the `dns` product enabled on the same installation | `call.invoke` of `dns.zone.create`, `dns.zone.list`, `dns.record.create` and the read `dns.zone.claim { name, account? } → { foreign, covered, contested }` inside the hosting call's transaction, under the same principal, so the site and its zone commit together and the zone's after-commit apply runs after the outer commit; the account is passed only when the site belongs to another account than the caller's own (`own_account`), since that parameter of `dns.zone.create` is reserved to resellers and admins; a field error of the zone lands on the hosting form, the zone name on `domain` and any other zone field on `dnsZone`; `open` answers the path `/f/dns.zone.list?search=<domain>` of the installation |
| `dns-peer` | a paired installation offering `dns` | `keel-client` calls to the neighbour with the link's token and `X-Keel-On-Behalf-Of` of the site's customer: a pre-check before the transaction, the zone itself after commit in a task, so the zone is owned by the same customer on both sides; `open` hands the customer over with a one-time key |

A hosting installation always has one of the two, because the bridge falls to `remote` when
`dns` is not enabled; the bridge still picks its side at start by that alone. `keel product
disable dns` on an installation whose `panel` bridges to it locally is refused unless the operator
passes `--switch-remote`, which accepts that hosting DNS operations go to a peer: over the control
socket `keel.product.disable` takes `switchRemote` and otherwise answers `conflict` with
`keel.product.error.bridged`, and on a stopped service the facade refuses with
`ProductsError::Bridged` inside `Error::Products`.

**The neighbour.** `dns-peer` finds its neighbour on every call through the hidden
`PeerLinks::offering(product, db)`, which `peers` answers, when it provides it, with the oldest
active link by creation whose peer published the product in its handshake (the products the daily
check refreshes) and which carries an address, as `PeerLink { id, instance, url, token }` with the
outbound token opened. Without one a zone answers `503 dns_peer.error.unpaired` naming the zone,
which says why where the generic `keel.error.unavailable` could not, a link paired after start
serves without a restart, and of several neighbours the oldest wins. `dns-peer` is a product module
crate that depends on `keel-client`, a library of L4 like `keel-host`, on the `api` of
`hosting-users` and on the `notify` api of `keel-system`, and requires `hosting_site`, `hosting`,
`task` and `notify` ([01-overview.md](01-overview.md)).

**The customer.** The customer of a site is its account. Before each call on its behalf `dns-peer`
calls `keel.principal.upsert` on the neighbour as the link itself, with the origin `panel:<instance
id>` of the installation and the account's id as the external id of both the account and one
principal, the login `panel-<instance id>-<account id>` in lower case (unique across panels, the nil
root account included) and the level `user`, then calls with `X-Keel-On-Behalf-Of` naming that
principal, so the zone lands in the customer's own account on the neighbour whoever created the
site. A pair code for a panel therefore grants `keel.principal.upsert`, `dns.zone.check`,
`dns.zone.create` and `auth.key.handover`, a level of `reseller` or above and `onBehalf`:

```
On the dns side:   keel ctl peer.pair_code scopes='["dns.zone.*","keel.principal.upsert","auth.key.handover"]' level=reseller onBehalf=true
On the panel side: keel ctl peer.create url=https://dns.example:8443 code=PAIR-… subscribe='["dns.zone.*","svc.*"]'
```

**The pre-check.** `dns-zones` adds `dns.zone.check { name, account? }`, a read action with the
access of `dns.zone.create` that answers `{ name }` in lower case or the refusals the creation would
answer for that name and account (the schema and the `domain` check, `dns.zone.error.exists` on
`name`, the `dns.zones` limit) and creates nothing. The pre-check is a `before` hook of `dns-peer`
on `hosting.site.create`, the stage outside the transaction that may wait on the network: when
`dnsZone` is set and the caller sees the hosting user, it syncs the customer and calls
`dns.zone.check` on behalf, unless the handler is certain to refuse anyway, for a hosting user still
pending or a domain or alias a site already holds, which `Sites::taken(db, names)` of `hosting-web`
answers with the handler's own query; the handler's other refusals, an alias named twice and the
`hosting.sites` limit of its within hook, come after the check, and a name freed between the hook
and the handler skips the check and leaves the zone to the task. The hook's client makes one attempt
with 2 seconds to connect and 4 for the whole check, inside the kernel's 5 seconds for a hook, so a
silent neighbour answers `peer` rather than `keel.error.hook_timeout`.

**Refusals.** A refusal of the neighbour answers `peer` (`502`) with its params and its field
errors moved onto the site form, `name` on `domain` and any other on `dnsZone` as `dns-local` moves
them, so no site commits. Its keys travel as they come, but for the two of `dns` a customer meets,
which a panel-only installation, where `dns` is not loaded, has no text for:
`dns.zone.error.exists` and `dns.zone.error.template` become `dns_peer.error.exists` and
`dns_peer.error.template` with the same params, and a field keeps the neighbour's key as its
`code`; the keys of the kernel and of the system modules, which both installations carry, pass
unchanged, and any other key of `dns`, an internal fault of the neighbour, reaches the panel as the
key itself. A transport failure answers `502 dns_peer.error.unreachable { url, reason }`, an answer
outside the contract `502 dns_peer.error.answer { url, status, reason }` and a request the client
refuses `500 dns_peer.error.client`.

**After commit.** `dns-peer`'s `add_zone` registers an after-commit hook that spawns its internal
read task `dns_peer.zone.create { site }` (class `dns_peer.zone`, not resumable) as the caller with
its scope widened to full, since the task continues a call its credential was admitted to, so a
token narrowed to `hosting.site.create` still gets its zone. The task reads the site through
`hosting.site.get` under that principal, ends without a zone when the site is gone, syncs the
customer and calls `dns.zone.create { name }` on behalf with the key
`dns_peer.zone.create:<site>:<request id of the task>`, so the client's retries create one zone. A
failure after commit is reported three ways: the task fails with the mapped error; the site row's
column `zone_error`, `zoneError` in the site's answers and a column of `hosting.site.list`, holds
the key of the error as relayed (its first field's key, else its own), written through
`Sites::zone_failed(db, site, error)` and cleared by a success, apart from `state` and `error`,
which the apply of nginx owns; and `dns-peer` contributes the critical problem source
`dns_peer.zone.failed` about the site, for its account, with the zone and the error, over
`Sites::zone_failures(db)`, so the problem stands while the row carries the error and clears with
the site or its account. The column and the problem show that key as text, as the column `error`
shows the detail or the key of a failed apply, since neither a `Text` column nor a param is
translated. No function retries a zone; recreating the site or creating the zone on the neighbour
are the remedies. Through a peer `foreign` and `add_records` keep their answers of M5, `false`, so
the zones of the neighbour stand behind neither the foreign-zone rule of sites and mail domains nor
the MX and SPF records of a mail domain. For the same reason a customer adds no mail domain on an
installation paired to a neighbour, since none of its zones stands on this installation.

**Opening the zone.** `hosting-web` owns `hosting.site.open_zone { id }`, a read action with the
access of `hosting.site.get` that answers `{ url }` as a `once` member and stands behind the row
action `open_zone` of `hosting.site.list`, of the role `external` and labelled
`hosting.site.action.open_zone` ("Open in KeelDNS"). The action stands on every site whichever side
the bridge picked: `dns-local` answers the local path; `dns-peer` syncs the customer as for a zone,
calls `auth.key.handover` on its behalf for 60 seconds with that path as the landing, answers the
absolute `url` of the neighbour, joins a path the neighbour answers without a public address to the
address of the link, refuses any other answer with `502 dns_peer.error.answer`, and answers `503
dns_peer.error.unpaired_open` without a link offering `dns`, a refusal of the neighbour as `peer`
with its key and an outage as `502 dns_peer.error.unreachable`. The panel opens the neighbour as
its customer rather than as its link ("One-click login"). The function is a read, so the panel
waits on the network inside no transaction, and its own trail does not record which of its
principals opened the neighbour. The answer may be a full browser session on the neighbour, which
no scope short of full grants, so the function opens for full scope only — a session, a token of
full scope, the CLI: before it loads the site, and whichever side the bridge picked, the handler
answers a read-only token `403 keel.error.token_read_only`, and a token scoped to functions, a mask
such as `hosting.site.*` included, or the principal of a link, on behalf of a customer or not, `403
keel.error.token_scope`, both with the function in `params`, and nothing is written on the
neighbour. A lease degraded to read-only does not refuse it: the lease bounds the writes of this
installation, the lease of the neighbour bounds the key and the session there, and a login keeps
working on a degraded installation. How the shell follows the answer is in
[08-frontend.md](08-frontend.md).

Inside dns the same abstraction fronts the name server: the `dns-zones` module provides a
provider of its own that applies zones to bind ([10-contracts.md](10-contracts.md)), and
`dns.zone.edit` calls it from `after_commit` ([02-functions.md](02-functions.md)).

Calls to a neighbour, like operations on the host, run outside the transaction: in a `before` hook,
after commit or inside a task, never while the transaction of a call waits
([02-functions.md](02-functions.md)). A failure reported by the neighbour through `dns-peer` maps
to the error code `peer` (`502`) in the caller's envelope. Because errors travel as i18n keys with
field paths, a validation error from the DNS side is shown on the matching field of the hosting
form; M6 is accepted only when a DNS error reaches the hosting form as a key on a field.

## Boundaries and sequence

Nodes of one installation are not peers, and neither are products of one installation. A second
server managed by the same installation is a cluster node reached through the host agent over mTLS
([06-host.md](06-host.md)); pairing and `kpt_` tokens are for installations on different hosts.

The ecosystem is deliberately late in the plan: no peers before there is a product to pair
([../plan.md](../plan.md)). M6 delivered the `peers` module (handshake with a proof of the instance
key, pairing by code, mutual `kpt_`, scopes, on-behalf by origin, TOFU, rotation), outbox delivery
with signatures and retries, the pull feed and inbox, `Idempotency-Key` end to end, the `svc`
module with the kinds `dns.zones` and `hosting.sites`, `keel-client`, the full conformance suite,
`keel.account.upsert` and `keel.principal.upsert` by origin and external id with the identity
events, and `panel` reaching `dns` on another host through `dns-peer`. Its acceptance: two
installations on two hosts paired by code; `panel` on one host creating a zone in `dns` on the
other on behalf of the customer with the DNS error shown on the hosting form field; the `dns`
installation opened from the hosting installation by a one-time key with the actor in the audit;
`svc.dns.zones.open` twice with one key giving one service; an event arriving signed and a repeat
creating no duplicate; conformance green against an installation with `dns` alone, with `panel`
alone and with both enabled; a revoked token failing calls with `401`. `hack/peers-e2e.sh` proves it
on two binaries of one machine in CI, through relays on plain HTTP, and `hack/peers-vm.sh` on two
machines that trust each other's certificate through the trust store of the system
([09-operations.md](09-operations.md)). The same-host case, one installation with both products
enabled and `dns-local`, is accepted at M5.

`keel-conformance` runs against the started `keel` binary with any set of products enabled and
covers, among other areas, idempotency, webhooks, `svc.*` and peers, a listener of the suite
playing the neighbouring installations with Ed25519 keys of their own; it is a mandatory CI job,
run over `dns` alone, `panel` alone and both ([05-modules.md](05-modules.md)).

## Threat model and review notes

A paired neighbour holds a token that runs functions on the installation, may act for the customers
it synced and receives its events, so the link is an attack surface of its own: the token and its
scope, the origin binding, the key a link pins, the hook any caller reaches, and the credentials a
link mints. The review of M6 went over `peers`, `svc`, idempotency and `keel-client` as an adversary
would, and every finding it fixed carries a named test in the crate it concerns: a server repeating
a copied instance key passed the comparison of trust on first use and received the live token of a
rotation, which the proof of possession now stops; a credential minted through a link outlived the
link; a stranger could make the hook look links up and hash bodies without bound, and a link could
fill its inbox without bound; a link's subscription and pairing delivered to private networks
without the guard of user subscriptions; and an idempotency key replayed an answer stripped for
another level to a credential of the same principal at a lower one, since the request hash now
covers the caller's level beside the body. Reviewed without a finding: the parameters of a service
reaching zone templates (`dns.zones` checks a template within the service's own account),
`keel-client`, the replay window and the constant-time comparisons of the hook, and the feed serving
events of its own installation alone.

Left open, and recorded in the M6 exit checklist of [../plan.md](../plan.md):

- The proof binds no channel: a server at the address that reaches the neighbour can relay the
  challenge, which only the certificate pinned in the link or a token sealed to the instance key
  would stop. The calls of a link, the pull feed and `dns-peer` send its token to the address
  without a proof, a refused daily check keeps them going, and a pulled event is authenticated by
  TLS alone where a pushed one carries the link's signature.
- A level granted to a link bounds the functions it reaches, not the accounts: its principal sits in
  the root account and a reseller's scope is the subtree of its account, so a link granted
  `reseller` reaches every account through every function its masks admit, `dns.zone.*` with an
  `account` parameter, the events of its subscription and feed and the records of its origin
  wherever they sit included, though a panel needs that level only for `keel.principal.upsert`; a
  principal of each link in an account of its own, under which its upserts place their accounts,
  would bound it and moves where synced customers land.
- The inbox bounds how many events of a link wait, not how large they are: an event may be as large
  as a request body. The bucket of the hook counts by address alone, so a paired installation
  sharing the address of a refused sender waits along its ladder with it.
- The receiving administrator approves no event masks, and an identity link applies its peer's
  account and principal events as an administrator; an administrator may place a record under a
  peer's origin, and the peer's later upsert adopts it wherever it lies, bounded by the account
  scope of the link.
- An instance id is bound to no key: the records of a deleted link stay under origins naming its
  instance, and the next pairing presenting that id under any key reaches them, while keeping the
  key of a deleted link would refuse the re-pair that recovers from a lost key.
- The receiving side of `peer.renew` checks the token alone, so a stolen token can rotate the link
  away from its peer; a proof by the caller's instance key there would need the kernel to sign for
  the peers module.
- The daily check, `peer.rotate` and `PeerLinks` reach the address a code holder gave without the
  private-targets guard, on fixed paths whose answers are read as a handshake or an envelope.
- A token scoped to functions grants `onBehalf` in a pair code, since containing a peer scope in a
  function scope compares masks alone, so a token a link mints may issue a code with `onBehalf` its
  link lacks.
- Every other change a link makes stays as the customer's own act: a password or an address it sets
  on behalf of a reseller for a principal below it, and a token created, a key issued or a password
  set inside a session a link's key opened, outlive the link, so a peer that redeems its own handover
  key keeps the customer's access; a session passes its link to no credential minted in it, since a
  token a customer makes for itself there would otherwise end at every rotation.
- `auth.key.handover` turns a call on behalf into a full browser session of the customer whatever
  else the masks of the link hold, and any principal of the panel who sees a site, a reseller or an
  administrator above its account included, opens the neighbour as the customer of that site; the
  session now ends with the link.
- `PeerLinks` hands the outbound token of a link to any module compiled in, the pre-check of
  `dns-peer` writes the customer on the neighbour before the local transaction decides, and its task
  runs with a widened scope: modules compiled into the binary are trusted code, and the review of M7
  takes up plugins.

## Left open

These points are within the scope of this document and not decided by the design:

- Whether external integrators get a REST facade such as `GET /api/v1/dns/zones/{id}` on top of
  the dispatcher, or `/api/v1/call/{func}` with the generated OpenAPI is enough
  ([11-decisions.md](11-decisions.md)).
- One-to-many versus many-to-many account membership, which decides how a billing customer with
  several employees is represented on an installation ([11-decisions.md](11-decisions.md)).
- Pinning the certificate of a neighbour in its link, and a link to several DNS neighbours for one
  panel; until then an installation reaches a neighbour whose certificate the trust store of the
  system admits ([09-operations.md](09-operations.md)).
- The zone of a site is spawned after commit, as the certificates of `hosting-ssl` are, so a stop
  between the commit and the spawn leaves the site without its zone; and translating a key held in
  a column or a param, such as `zoneError`, is the shell's.
