FROM rust:slim-bookworm AS builder

RUN rustup target add x86_64-unknown-linux-musl && \
    apt-get update && apt-get install -y --no-install-recommends musl-tools && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

RUN cargo build --release --target x86_64-unknown-linux-musl && \
    strip target/x86_64-unknown-linux-musl/release/rust-affected

FROM alpine:3.21

RUN apk add --no-cache git cargo

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/rust-affected /usr/local/bin/rust-affected

ENTRYPOINT ["rust-affected", "--github-actions"]
