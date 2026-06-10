# syntax=docker/dockerfile:1
#
# Multi-stage build producing a tiny, fully static `zebrafish` image.
# Built NATIVELY per architecture (no QEMU): CI runs this on an amd64 runner and
# an arm64 runner, then merges the two into one manifest. So BUILDPLATFORM always
# equals TARGETPLATFORM here.
#
# Result: a static musl binary on `scratch`, target < 15 MB.

# ---- builder -------------------------------------------------------------
FROM rust:1.96-bookworm AS builder

# musl-tools provides musl-gcc, needed once C deps land (rusqlite bundled, WS-A).
# Pure-Rust builds use rustc's self-contained musl linking and don't need it yet,
# but installing now keeps the image forward-compatible.
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/*

# Map Docker's TARGETARCH (amd64|arm64) to the Rust musl triple. BuildKit
# auto-populates TARGETARCH from the build platform — but ONLY for a bare `ARG`
# with no default (a default value suppresses the automatic value). Requires
# BuildKit (guaranteed by the `# syntax=` directive at the top).
ARG TARGETARCH
RUN case "$TARGETARCH" in \
        amd64) echo x86_64-unknown-linux-musl  > /tmp/triple ;; \
        arm64) echo aarch64-unknown-linux-musl > /tmp/triple ;; \
        *)     echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac && rustup target add "$(cat /tmp/triple)"

WORKDIR /app
COPY . .

# BuildKit cache mounts keep the cargo registry and target dir warm across
# builds without bloating the final image. The binary is copied OUT of the
# cache mount inside the same RUN (cache mounts don't persist between layers).
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    TRIPLE="$(cat /tmp/triple)" && \
    cargo build --release --locked --bin zebrafish --target "$TRIPLE" && \
    cp "/app/target/$TRIPLE/release/zebrafish" /zebrafish

# ---- runtime -------------------------------------------------------------
FROM scratch AS runtime

# The data volume for the SQLite db (spec §4): /data/zebrafish.db
VOLUME ["/data"]
EXPOSE 4242

COPY --from=builder /zebrafish /zebrafish

ENTRYPOINT ["/zebrafish"]
