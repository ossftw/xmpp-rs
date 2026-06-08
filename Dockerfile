FROM rust:slim-bookworm AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null; true
RUN rm -rf src

COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /data /certs

COPY --from=builder /app/target/release/xmpp-server /usr/local/bin/xmpp-server

VOLUME ["/data", "/certs"]

EXPOSE 5222 5269

CMD ["xmpp-server"]
