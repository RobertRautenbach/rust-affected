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

# The binary shells out to `cargo metadata`, so the runtime image needs cargo and
# rustc. Cargo asks rustc only for its version and target configuration and
# never compiles, so everything a build would need is removed here: the standard
# library, the linkers, rustdoc, the documentation and the C toolchain. That is
# about two thirds of the image. libgcc stays: cargo and rustc link against it.
# git stays too: when a consumer sets `net.git-fetch-with-cli = true` cargo
# fetches git dependencies by spawning the git CLI (the smoke test covers that
# configuration). This RUN executes on the target platform, so the release
# build needs QEMU for the arm64 image.
FROM rust:1.99.1-alpine3.23@sha256:51e1b72c7f236093c084f6e3051239f033fba9e4c0c1430cc2ed479d0a3bd14c AS runtime
RUN set -eux; \
    apk add --no-cache git libgcc; \
    apk del --no-cache gcc musl-dev; \
    sysroot="$(rustc --print sysroot)"; \
    rm -rf "$sysroot"/lib/rustlib/*/lib "$sysroot"/lib/rustlib/*/bin \
      "$sysroot/bin/rustdoc" "$sysroot/share" "$sysroot/libexec" "$sysroot/etc"; \
    # A name that does not change with the Rust version, for RUSTUP_TOOLCHAIN below.
    rustup toolchain link pinned "$sysroot"; \
    # Fail the build if the pruned toolchain can no longer answer cargo metadata.
    mkdir -p /tmp/probe/src; \
    printf '[package]\nname = "probe"\nversion = "0.0.0"\nedition = "2021"\n' > /tmp/probe/Cargo.toml; \
    : > /tmp/probe/src/lib.rs; \
    RUSTUP_TOOLCHAIN=pinned RUSTUP_AUTO_INSTALL=0 CARGO_HOME=/tmp/probe/cargo-home \
      cargo metadata --format-version 1 --offline --manifest-path /tmp/probe/Cargo.toml > /dev/null; \
    rm -rf /tmp/probe

# Files deleted in a later layer still ship in the earlier one, so the pruned
# filesystem is copied into an empty image. That drops the base image's
# environment, which is restated here.
FROM scratch
COPY --from=runtime / /
ENV PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo
# Use the installed toolchain even when the checkout contains rust-toolchain.toml.
ENV RUSTUP_TOOLCHAIN=pinned RUSTUP_AUTO_INSTALL=0

LABEL org.opencontainers.image.title="rust-affected"
LABEL org.opencontainers.image.description="Computes affected Cargo workspace members from a set of changed files via the Cargo dependency graph"
LABEL org.opencontainers.image.source="https://github.com/robertrautenbach/rust-affected"
LABEL org.opencontainers.image.licenses="MIT"
LABEL org.opencontainers.image.authors="Robert Rautenbach"

COPY --from=builder /rust-affected /usr/local/bin/rust-affected
WORKDIR /github/workspace
ENTRYPOINT ["rust-affected"]
