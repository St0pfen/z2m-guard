# syntax=docker/dockerfile:1
FROM rust:1.100-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY config ./config
# Cache mounts keep the ~150 dependency crates out of the image layer, so a
# source-only change recompiles one crate instead of everything. That matters
# most for the emulated arm64 leg of the multi-arch build.
RUN --mount=type=cache,target=/src/target,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --release -p z2m-guard --features stdin-events \
 && cp target/release/z2m-guard /usr/local/bin/z2m-guard

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /usr/local/bin/z2m-guard /usr/local/bin/z2m-guard
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/z2m-guard"]