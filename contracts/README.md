# contracts

The wire contract of the platform, in a form both sides can check against.

- `protocol.md` — the prose of the contract, not yet written; until it is,
  `docs/design/10-contracts.md` is the contract's specification.
- `fixtures/` — golden JSON fixtures loaded by the tests of `keel-types` and by the frontend test
  suite under Jest: expression cases, check cases, descriptors, envelopes, list queries, the
  event and the `/boot` payloads `boot.json` (two products) and `boot-none.json` (none), which
  feed the shell's Storybook stories together with the admin dump's one-product payload.
- `fixtures/descriptors/` — the descriptor snapshots of the kernel's own functions,
  `keel.<entity>.<verb>.json`, one plain JSON file per function, unprojected. The test
  `the_descriptors_of_the_kernel_module_match_the_committed_snapshots` in `keel-kernel` compares
  the registry against them and fails on a difference or a file no function owns; after
  reviewing the diff, `KEEL_UPDATE_SNAPSHOTS=1 cargo test -p keel-kernel` rewrites them.
- `schema/` — the JSON Schema of `keel-types`, written by `keel-dev types --out contracts/schema`
  and checked by CI with `--check`: `keel.json` is one draft 2020-12 document whose `$defs` hold
  every wire type, and `<Type>.json` beside it is one `$ref` into it per root type, which the
  frontend compiles with Ajv and `pnpm generate` turns into the TypeScript types of
  `@globalart/keel/meta`.
- `dump/` — the projected contract of the real binary per level, written by
  `keel-dev descriptors dump` through `hack/descriptors.sh`: for `guest`, `user`, `reseller` and
  `admin` the `/boot` payload, the descriptors of `GET /api/v1/functions` as `functions/<func>.json`,
  the `en` and `ru` catalogues under `i18n/` and, where the level may list them, the rows of
  `keel.product.list` as `products.json`. The ids of the principal and its account, the metadata
  version and the enabling instant name the run and are replaced by fixed placeholders. CI runs
  `keel-dev descriptors check` against it and `keel-dev views share` over it; after reviewing a
  change, `KEEL_UPDATE_DUMP=1 hack/descriptors.sh target/debug/keel` rewrites it.
- `icons.json` — the sorted names of the icon set the shell renders, written from
  `PHX_ICON_NAMES` of `@globalart/phoenix-ui` by `node tools/icons.mjs`, the second half of
  `pnpm generate` in `frontend/`. The kernel embeds the file and refuses at start a descriptor
  action, a menu node or a menu group naming an icon outside it, so the shell never receives an
  unknown icon; the job `frontend` regenerates the file and fails on a difference.
- `openapi.json` — the OpenAPI description of the call surface, written from the admin dump by
  `keel-dev openapi --dump contracts/dump --out contracts/openapi.json`: one
  `POST /api/v1/call/<func>` per function, the parameter schema as its request body, the
  envelope around the result schema as its `200` response and the error envelope as `default`;
  a `kind: download` function is described at its raw route `/api/v1/x/<module>/<name>` instead,
  as `GET` with the parameters in the `q` query member and `POST` with them in the body, its `200`
  the streamed file and the `{ filename, contentType }` result schema kept under `x-keel.result`;
  the `keel://` references and the `$defs` of the descriptors are hoisted under
  `components/schemas`, a local definition keeping its name when nothing else claims it and
  taking `<func>.<params|result>.<name>` otherwise. `hack/descriptors.sh` runs `--check` after
  the dump check, so a descriptor change reaches the document in the same commit;
  `KEEL_UPDATE_DUMP=1` rewrites it with the dump.
