FROM rust:slim-bookworm AS chef
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools && \
    rm -rf /var/lib/apt/lists/* && \
    cargo install cargo-chef --locked

WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG TARGETARCH
# Select the correct musl target for the platform being built
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl   > /rust-target ;; \
      arm64) echo aarch64-unknown-linux-musl  > /rust-target ;; \
      *) echo "Unsupported arch: $TARGETARCH" && exit 1 ;; \
    esac && \
    rustup target add "$(cat /rust-target)"

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target "$(cat /rust-target)" --recipe-path recipe.json

COPY . .
RUN TARGET="$(cat /rust-target)" && \
    cargo build --release --target "$TARGET" -p rust-affected && \
    strip "target/$TARGET/release/rust-affected" && \
    cp "target/$TARGET/release/rust-affected" /rust-affected

FROM rust:1-alpine3.23
RUN apk add --no-cache git
COPY --from=builder /rust-affected /usr/local/bin/rust-affected
ENTRYPOINT ["rust-affected", "--github-actions"]
