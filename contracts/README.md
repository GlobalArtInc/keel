# contracts

The wire contract of the platform, in a form both sides can check against.

- `protocol.md` — the prose of the contract; `docs/design/10-contracts.md` is its specification.
- `fixtures/` — golden JSON fixtures loaded by the tests of `keel-types` and, from M2, by the
  frontend test suite: expression cases, check cases, descriptors, envelopes, list queries.
- `openapi.json` — generated from the function registry, committed for diffs (from M2).
