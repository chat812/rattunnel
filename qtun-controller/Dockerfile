FROM rust:bookworm AS builder
RUN apt-get update && apt-get install -y libssl-dev pkg-config
WORKDIR /home/rust/src
COPY . .
RUN cargo build --locked --release
RUN mkdir -p build-out/ && cp target/release/qtun-controller build-out/

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /home/rust/src/build-out/qtun-controller .
ENTRYPOINT ["./qtun-controller", "--config", "/app/config.toml"]
