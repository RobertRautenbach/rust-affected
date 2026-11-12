# Base images are pinned by digest as well as tag: a tag can be moved to
# different content. Dependabot updates the tag and the digest together.
FROM --platform=$BUILDPLATFORM rust:1.99.1-slim-bookworm@sha256:f2c004e3d2d8b547e8cf7ffbc29205385f4e71d23e53e4e383065db5e9f822cd AS chef
RUN apt-get update && apt-get install -y --no-install-recommends \
      musl-tools \
      gcc-aarch64-linux-gnu \
      gcc-x86-64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked --version 0.1.73

WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG TARGETARCH
# Cross-compile: runs on the build host, targets the desired arch
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl  > /rust-target ;; \
      arm64) echo aarch64-unknown-linux-musl > /rust-target ;; \
      *) echo "Unsupported arch: $TARGETARCH" && exit 1 ;; \
    esac && \
    rustup target add "$(cat /rust-target)"

ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --target "$(cat /rust-target)" --recipe-path recipe.json

COPY . .
RUN TARGET="$(cat /rust-target)" && \
    cargo build --release --locked --target "$TARGET" -p rust-affected && \
    cp "target/$TARGET/release/rust-affected" /rust-affected

# The binary shells out to `cargo metadata`, so the runtime image needs a Rust
# toolchain. git stays too: when a consumer sets `net.git-fetch-with-cli = true`
# cargo fetches git dependencies by spawning the git CLI (the smoke test covers
# that configuration). This RUN executes on the target platform, so the release
# build needs QEMU for the arm64 image.
FROM rust:1.99.1-alpine3.23@sha256:51e1b72c7f236093c084f6e3051239f033fba9e4c0c1430cc2ed479d0a3bd14c
# Use the installed toolchain even when the checkout contains rust-toolchain.toml.
# Derive the pin from the base image so dependency updates keep it in sync.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION} RUSTUP_AUTO_INSTALL=0
RUN test -n "${RUSTUP_TOOLCHAIN}" && rustup run "${RUSTUP_TOOLCHAIN}" cargo --version \
    && apk add --no-cache git

LABEL org.opencontainers.image.title="rust-affected"
LABEL org.opencontainers.image.description="Computes affected Cargo workspace members from a set of changed files via the Cargo dependency graph"
LABEL org.opencontainers.image.source="https://github.com/robertrautenbach/rust-affected"
LABEL org.opencontainers.image.licenses="MIT"
LABEL org.opencontainers.image.authors="Robert Rautenbach"

COPY --from=builder /rust-affected /usr/local/bin/rust-affected
WORKDIR /github/workspace
ENTRYPOINT ["rust-affected"]
