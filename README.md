<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="./assets/zebrafish-mark-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="./assets/zebrafish-mark-light.svg">
  <img src="./assets/zebrafish-mark-light.svg" alt="zebrafish" width="72" height="72">
</picture>

# zebrafish

### A Stripe-compatible model organism for your test suite.

**A fake-but-stateful, Stripe-compatible payment server that runs entirely on your machine.**
Exercise your *whole* subscription flow — hosted checkout, renewals, payment failures,
cancellation — and receive **real, signed webhooks** at a local endpoint. No network calls to
Stripe. No Stripe credentials. No flakiness.

[![CI](https://img.shields.io/badge/CI-pending-lightgrey)](https://github.com/indielabio/zebrafish/actions)
[![conformance](https://img.shields.io/badge/conformance-stripe--api%20pending-lightgrey)](https://github.com/indielabio/zebrafish)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE-APACHE)
[![image](https://img.shields.io/badge/ghcr.io-indielabio%2Fzebrafish-2496ED?logo=docker&logoColor=white)](https://github.com/indielabio/zebrafish/pkgs/container/zebrafish)
![local-only](https://img.shields.io/badge/scope-local%20dev%20tool-success)

[Quickstart](#-quickstart) · [Why zebrafish](#-why-zebrafish) · [Using it](#-using-it) · [How it works](#-how-it-works) · [Contributing](#-contributing)

</div>

> [!NOTE]
> **Status: pre-`v0.1`, under active construction.** The roadmap below is tracked publicly as
> [GitHub issues and milestones](https://github.com/indielabio/zebrafish/milestones) so you can
> follow along. `zebrafish` is **Stripe-compatible** — an independent emulator, not affiliated with
> or endorsed by Stripe.

---

## What is this?

The **zebrafish** is the standard *model organism* — the thing you run experiments on so you don't
have to experiment on the real subject. It's striped, and it's transparent: you can see straight
through it.

That is exactly what this tool is for your payments code. Point your app's Stripe SDK at a local
`zebrafish` container instead of the real API, and your **entire** billing state machine runs
offline: a customer hits a fake hosted Checkout page, completes a subscription, your app receives a
signed `checkout.session.completed` webhook, you fast-forward the virtual clock 31 days, a renewal
invoice is charged and you receive `invoice.payment_succeeded` — or you swap in a failing card and
receive `invoice.payment_failed` to test your dunning/grace logic. All deterministic, all
reproducible, all on `localhost`.

## 🤔 Why zebrafish?

Existing tools each solve a slice of the problem. `zebrafish` is built to drive the *full
subscription lifecycle end to end*.

| | **zebrafish** | stripe-mock | localstripe |
|---|:---:|:---:|:---:|
| Stateful objects (CRUD persists) | ✅ | ❌ (schema-only) | ✅ |
| **Hosted Checkout page emulation** | ✅ | ❌ | ❌ |
| Real, **signed** webhooks (SDK-verifiable) | ✅ | ❌ | ✅ |
| Full subscription lifecycle + renewals | ✅ | ❌ | partial |
| **Virtual clock** (fast-forward time) | ✅ | ❌ | partial |
| **Deterministic** seeded runs | ✅ | ❌ | ❌ |
| **Chaos API** (declines, errors, dropped/duplicated webhooks) | ✅ | ❌ | ❌ |
| Embedded web **dashboard** + delivery log | ✅ | ❌ | ❌ |
| Conformance-tested against a real Stripe sandbox | ✅ (nightly) | ❌ | ❌ |
| Single static binary + multi-arch Docker image | ✅ | ✅ | ❌ |

**What makes it different, in one breath:** a fake **hosted Checkout** page that fires the full
event cascade; event cascades recorded from *real* Stripe test mode as declarative JSON fixtures (no
Rust needed to extend coverage); one **seeded RNG** so the same seed produces a byte-identical run; a
**Chaos API** to inject specific failures; and an embedded **dashboard** with an object browser, live
event stream, and a webhook delivery log you can replay.

---

## 🚀 Quickstart

`zebrafish` ships as a multi-arch Docker image. Drop it into your `compose.yaml` next to your app:

```yaml
services:
  zebrafish:
    image: ghcr.io/indielabio/zebrafish:latest
    ports: ["4242:4242"]
    environment:
      ZEBRAFISH_SEED: "42"                                  # deterministic runs
      ZEBRAFISH_WEBHOOK_URL: "http://app:3000/webhooks"     # auto-registers, prints the signing secret
    volumes: ["zebrafish-data:/data"]

volumes:
  zebrafish-data:
```

Then point your application's Stripe SDK at it:

```bash
STRIPE_API_BASE=http://zebrafish:4242
STRIPE_SECRET_KEY=sk_test_anything          # any sk_test_* / pk_test_* value is accepted
STRIPE_WEBHOOK_SECRET=<the whsec_… printed at startup>
```

Open the dashboard at **http://localhost:4242/_dashboard**.

Prefer one-shot, no compose file?

```bash
docker run --rm -p 4242:4242 -e ZEBRAFISH_SEED=42 ghcr.io/indielabio/zebrafish:latest
```

> A standalone static binary (`zebrafish`) for non-Docker use is planned for the first release.

---

## 🧪 Using it

### Drive it with your normal Stripe SDK

There's nothing special to learn for the happy path — `stripe-node`, `stripe-go`, `stripe-python`,
`stripe-ruby`, etc. all work once `STRIPE_API_BASE` points at the container. Create products, prices,
customers, and Checkout Sessions exactly as you would against the real test API.

### Magic test cards

`zebrafish` implements Stripe's published test cards with identical behavior, so the same numbers you
already use in Stripe test mode work here:

| Card number | Behavior |
|---|---|
| `4242 4242 4242 4242` | Succeeds (Visa) |
| `4000 0000 0000 0002` | Declined — `card_declined` / `generic_decline` |
| `4000 0000 0000 9995` | Declined — `card_declined` / `insufficient_funds` |
| `4000 0000 0000 0341` | Attaches, then **fails on renewal / off-session charges** |
| `5555 5555 5555 4444` | Succeeds (Mastercard) |

See Stripe's [test cards reference](https://docs.stripe.com/testing) for the full catalog — we mirror
their semantics rather than re-document them.

### Fast-forward time (virtual clock)

No waiting 30 days for a renewal. Advance the clock and the renewal cascade runs synchronously:

```bash
curl -X POST http://localhost:4242/_config/clock/advance \
  -H 'Content-Type: application/json' \
  -d '{"days": 31}'
# → { "now": 1750000000, "events_emitted": ["evt_…", "evt_…"] }
```

### Register a webhook endpoint

If you didn't set `ZEBRAFISH_WEBHOOK_URL`, register one at runtime (or via the dashboard):

```bash
curl -X POST http://localhost:4242/_config/webhooks \
  -d '{"url":"http://host.docker.internal:3000/webhooks","events":["*"]}'
# secret optional → a whsec_… is generated and returned
```

Deliveries are signed byte-exactly like Stripe (`Stripe-Signature: t=…,v1=…`) and are verified in our
CI by the real `stripe-node` and `stripe-go` libraries.

### Inject failures (Chaos API)

Force a single request to fail without touching global state — perfect for parallel integration
tests:

```bash
curl http://localhost:4242/v1/charges -H 'Zebrafish-Fail: card_declined'
```

…or register a standing rule (`POST /_config/chaos`) to drop, duplicate, delay, or reorder webhook
deliveries and simulate declines or API errors. See the **Chaos** tab in the dashboard for the full
menu.

### The dashboard

`http://localhost:4242/_dashboard` gives you an object browser, a live event stream, a **webhook
delivery log** (with your app's response body, signature, and a one-click replay), virtual-clock
controls, and the chaos cheat-sheet. It talks to the same public `/_config` API your tests use — no
privileged endpoints.

---

## 🔬 How it works

- **Virtual clock.** Every timestamp comes from one virtual clock — never the wall clock. You advance
  it explicitly; advancing runs the renewal/cancellation scheduler synchronously before returning.
- **Deterministic by seed.** All generated data (IDs, fake names/emails, prices, fingerprints) flows
  from a single seeded `ChaCha` RNG, persisted across restarts. Same seed ⇒ identical run.
- **Cascade-as-data.** The event storms that one trigger produces (e.g. "checkout completed") are
  **declarative JSON fixtures recorded from real Stripe test mode**, not hand-written code. Extending
  coverage is a recording, not a pull request full of Rust.
- **Blob storage.** Objects are stored as the exact JSON the API returns, in SQLite — no relational
  re-modeling of Stripe's schema, so responses match the pinned Stripe API version field-for-field.

Each `zebrafish` release pins **one** Stripe API version and stamps it into every event's
`api_version`. The README badge will state the verified version once nightly conformance is live.
Deeper docs (architecture, fixture format, recording pipeline) live in
[`docs/`](./docs) and on the docs site (in progress).

---

## 🗺️ Roadmap

Work is organized into workstreams (`WS-A` … `WS-K`) tracked as
[epics with sub-issues](https://github.com/indielabio/zebrafish/issues?q=is%3Aissue+label%3Aepic) and
sequenced across [milestones](https://github.com/indielabio/zebrafish/milestones), building toward the
**v0.1 north-star**: a sample app that, fully offline, redirects a browser through fake checkout with
`4242`, activates on `checkout.session.completed`, renews on `invoice.payment_succeeded` after a
clock advance, goes past-due on `invoice.payment_failed`, and deactivates on
`customer.subscription.deleted` — the complete four-webhook subscription state machine.

---

## 🤝 Contributing

Contributions are tiered so you can help without necessarily writing Rust:

- **Tier 1 — fixtures / seeds / docs (no Rust).** Record a new event cascade from your own Stripe test
  sandbox via the recording pipeline and land it as a JSON fixture.
- **Tier 2 — a new resource module + fixtures + tests.** A guided walkthrough (adding `Coupon`) is
  provided.
- **Tier 3 — engine internals** (cascade engine, webhook delivery, chaos, the form parser).

See [`docs/CONTRIBUTING.md`](./docs/CONTRIBUTING.md) and
[`docs/RECORDING.md`](./docs/RECORDING.md) to get started. Good first issues are
[labeled](https://github.com/indielabio/zebrafish/labels/good%20first%20issue).

---

## 🔒 Security

`zebrafish` is a **local development tool.** It binds `localhost` by default (outside Docker), makes
no outbound calls to Stripe, and uses no real credentials. **No real card data ever** — PANs you type
into the fake checkout are never persisted; only the brand, last 4, expiry, and a synthetic
fingerprint are stored, keeping the project trivially out of PCI scope. **Do not expose it publicly.**

---

## 📄 License

Dual-licensed under either of [MIT](./LICENSE-MIT) or [Apache 2.0](./LICENSE-APACHE), at your option.

> *zebrafish is an independent, Stripe-compatible emulator for local testing. "Stripe" is a trademark
> of Stripe, Inc.; this project is not affiliated with, endorsed by, or sponsored by Stripe.*
