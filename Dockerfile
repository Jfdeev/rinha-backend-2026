# Stage 1: compile Rust binaries
# Target x86-64 Haswell (the contest machine: Mac Mini Late 2014 / Intel Core i5-4xxx)
FROM rust:1.81-slim AS builder

WORKDIR /build
COPY Cargo.toml ./
COPY src ./src

RUN RUSTFLAGS="-C target-cpu=haswell" \
    cargo build --release --bin rinha --bin preprocess

# Stage 2: preprocess reference data into compact binary
FROM rust:1.81-slim AS data-prep

WORKDIR /data-prep
COPY --from=builder /build/target/release/preprocess /usr/local/bin/preprocess

# references.json.gz must be present in the build context at resources/references.json.gz
# Run: cp /path/to/rinha-de-backend-2026/resources/references.json.gz resources/
COPY resources/references.json.gz ./

RUN mkdir -p /app/data && \
    preprocess references.json.gz /app/data/refs.bin && \
    echo "refs.bin size: $(wc -c < /app/data/refs.bin) bytes"

# Stage 3: minimal runtime image
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends curl ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /build/target/release/rinha /app/rinha
COPY --from=data-prep /app/data/refs.bin /app/data/refs.bin

ENV DATA_PATH=/app/data/refs.bin

EXPOSE 9999
CMD ["/app/rinha"]
