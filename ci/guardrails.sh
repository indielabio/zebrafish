#!/usr/bin/env bash
# CI guardrails — enforce spec §15 invariants with greps (issue #108).
# Exits non-zero on the first violation. Run from the repo root.
#
# Scope note: zebrafish MUST emit the real `Stripe-Signature` / `Stripe-Version`
# compatibility headers, so we do NOT blanket-ban the string "Stripe". We ban
# only the specific things the spec forbids in zebrafish's *own* surface.
set -euo pipefail

fail=0
report() {
    # report <message> <grep-output>
    echo "::error::guardrail violation: $1"
    echo "$2"
    fail=1
}

# Search source only; never the target dir, vendored specs, or this script.
SRC_GLOBS=(--include='*.rs')
SRC_DIRS=(crates)

# ---------------------------------------------------------------------------
# A. No SystemTime::now() outside crates/core/src/clock — the virtual clock is
#    the only time source (spec §6.2, §15).
# ---------------------------------------------------------------------------
if hits=$(grep -rnE "${SRC_GLOBS[@]}" 'SystemTime::now|Instant::now' "${SRC_DIRS[@]}" \
        | grep -vE 'crates/core/src/clock' || true); [ -n "$hits" ]; then
    report "SystemTime/Instant::now() outside core::clock — use world.now()" "$hits"
fi

# ---------------------------------------------------------------------------
# B. No ambient RNG outside crates/core/src/faker — all randomness flows from
#    the seeded world RNG (spec §6.4, §15).
# ---------------------------------------------------------------------------
if hits=$(grep -rnE "${SRC_GLOBS[@]}" 'thread_rng|rand::random|OsRng' "${SRC_DIRS[@]}" \
        | grep -vE 'crates/core/src/faker' || true); [ -n "$hits" ]; then
    report "ambient RNG (thread_rng/rand::random/OsRng) outside core::faker" "$hits"
fi

# ---------------------------------------------------------------------------
# C. zebrafish's own env vars use the ZEBRAFISH_ prefix, never STRIPE_
#    (spec naming rules). Catches env reads of a STRIPE_-prefixed var.
# ---------------------------------------------------------------------------
if hits=$(grep -rnE "${SRC_GLOBS[@]}" '"STRIPE_[A-Z]' "${SRC_DIRS[@]}" || true); [ -n "$hits" ]; then
    report 'STRIPE_-prefixed env var in source — zebrafish uses ZEBRAFISH_' "$hits"
fi

# ---------------------------------------------------------------------------
# D. The word "stripe" must not appear in crate or binary names (only as
#    "Stripe-compatible" prose). Check every Cargo.toml name/bin field.
# ---------------------------------------------------------------------------
if hits=$(grep -rnE --include='Cargo.toml' '^(name|\s*name)\s*=\s*".*stripe' "${SRC_DIRS[@]}" || true); [ -n "$hits" ]; then
    report 'crate/binary name contains "stripe"' "$hits"
fi

if [ "$fail" -ne 0 ]; then
    echo
    echo "Guardrails failed. See spec §15 for the rules."
    exit 1
fi
echo "Guardrails passed."
