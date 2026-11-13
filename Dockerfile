# syntax=docker/dockerfile:1
FROM rust:1.100-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY config ./config
RUN cargo build --release -p z2m-guard --features stdin-events

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/z2m-guard /usr/local/bin/z2m-guard
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/z2m-guard"]