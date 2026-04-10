FROM rust:1.85-bookworm AS builder

WORKDIR /build
COPY . .

RUN cargo build --release -p lvqr-cli

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/lvqr /usr/local/bin/lvqr

EXPOSE 4443/udp 1935/tcp 8080/tcp

ENTRYPOINT ["lvqr"]
CMD ["serve"]
