# syntax=docker/dockerfile:1
FROM rust:1.99.1-bookworm AS build
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends cmake && \
    rm -rf /var/lib/apt/lists/*

# Copy only source inputs; credentials are never needed during the build.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked && \
    cp target/release/start-infinity-mcp /start-infinity-mcp

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=build /start-infinity-mcp /usr/local/bin/start-infinity-mcp
ENV RUST_LOG=warn
ENTRYPOINT ["/usr/local/bin/start-infinity-mcp"]
