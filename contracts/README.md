# contracts

The wire contract of the platform, in a form both sides can check against.

- `protocol.md` — the prose of the contract, not yet written; until it is,
  `docs/design/10-contracts.md` is the contract's specification.
- `fixtures/` — golden JSON fixtures loaded by the tests of `keel-types` and, from M2, by the
  frontend test suite: expression cases, check cases, descriptors, envelopes, list queries, the
  event and the `/boot` payload.
- `fixtures/descriptors/` — the descriptor snapshots of the kernel's own functions,
  `keel.<entity>.<verb>.json`, one plain JSON file per function, unprojected. The test
  `the_descriptors_of_the_kernel_module_match_the_committed_snapshots` in `keel-kernel` compares
  the registry against them and fails on a difference or a file no function owns; after
  reviewing the diff, `KEEL_UPDATE_SNAPSHOTS=1 cargo test -p keel-kernel` rewrites them.
- `openapi.json` — generated from the function registry, committed for diffs (from M2).
