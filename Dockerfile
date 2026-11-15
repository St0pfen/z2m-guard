# syntax=docker/dockerfile:1
# Pinned to bookworm on purpose. Plain `rust:1.100-slim` moved to a trixie base
# upstream, which links against glibc 2.38+; the runtime stage below is distroless
# cc-debian12 (glibc 2.36), so the resulting image died at startup with
# "GLIBC_2.38 not found" on *both* architectures while the build itself succeeded.
# Measured: objdump -T on the binary requires GLIBC_2.38, cc-debian12 ships 2.36.
FROM rust:1.100-slim-bookworm AS build
WORKDIR /src

# Optional cargo features for the shipped binary. Empty by default, so the plain
# image stays exactly as before; `--build-arg FEATURES=proxy` produces the image
# that can actually run proxy mode. Without this the Dockerfile compiled a binary
# where `proxy.enabled = true` was a startup error — an image nobody could enable.
ARG FEATURES=""

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY config ./config
# Cache mounts keep the ~150 dependency crates out of the image layer, so a
# source-only change recompiles one crate instead of everything. That matters
# most for the emulated arm64 leg of the multi-arch build.
# No `--features stdin-events` here on purpose: a container cannot be fed a live
# `docker events` stream (its stdin is fixed at start, and the runtime image has no
# shell to pipe through). Compiling it into the shipped image would advertise a
# capability that nothing can use. The host/systemd variant wires it properly —
# see docs/INSTALL.md, "Docker event attribution".
RUN --mount=type=cache,target=/src/target,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --release -p z2m-guard ${FEATURES:+--features "$FEATURES"} \
 && cp target/release/z2m-guard /usr/local/bin/z2m-guard

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /usr/local/bin/z2m-guard /usr/local/bin/z2m-guard
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/z2m-guard"]