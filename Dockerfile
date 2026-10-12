FROM rust:1-slim-bookworm AS build
WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p server && \
    cp target/release/server /usr/local/bin/data-layer

FROM debian:bookworm-slim AS runtime

RUN useradd --system --create-home --uid 10001 datalayer
USER datalayer

COPY --from=build /usr/local/bin/data-layer /usr/local/bin/data-layer

ENV DL_BIND_ADDR=0.0.0.0:8080 \
    DL_LOG_LEVEL=info

EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/data-layer"]
