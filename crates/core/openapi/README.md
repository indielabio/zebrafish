# Vendored Stripe OpenAPI document

`spec3.sdk.json` is the reference of record for every hand-rolled response
shape in zebrafish (spec §3): contract tests in `crates/server/tests/contract.rs`
validate emulator responses against the component schemas in this file with
required properties enforced.

| | |
| --- | --- |
| Source | <https://raw.githubusercontent.com/stripe/openapi/v2153/openapi/spec3.sdk.json> |
| Upstream tag | `v2153` |
| `info.version` | `2025-12-15.clover` |
| Retrieved | 2026-06-10 |

## Why `2025-12-15.clover` for the `2025-12-30` pin

zebrafish pins `STRIPE_API_VERSION = "2025-12-30"` (spec §3). Stripe shipped no
API version dated exactly 2025-12-30 — upstream releases jump from
`2025-12-15.clover` (tag `v2153`) to `2026-01-28.clover` (tag `v2154`) — so this
file vendors the version that was *in effect* on the pin date. Do not re-vendor
from `master`; later documents describe a different API version.

No build-script codegen consumes this file (spec §3 permits hand-rolled
`serde_json` shapes for v1); it is read only by tests and tooling.
