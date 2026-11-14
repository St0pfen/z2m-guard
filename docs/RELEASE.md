# Release checklist

Applies to any version bump. The repository stays private until M4 is explicitly
opened; nothing here requires publishing.

## 1. Gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Record the actual numbers (`test result: ok. N passed`), not "tests pass".

## 2. Version

- `Cargo.toml` workspace `version`
- `CHANGELOG.md` section with the date and a summary of verified behaviour
- README status line if the milestone changed

## 3. Artifacts

```bash
./deploy/build-release.sh            # version taken from Cargo.toml
```

Produces in `dist/`:

- `z2m-guard-<version>-x86_64-unknown-linux-musl` — static, no glibc dependency
- `SHA256SUMS` — covers every artifact in the directory
- `z2m-guard-<version>.cdx.json` — CycloneDX 1.5 SBOM from the real lockfile
- `THIRD-PARTY-LICENSES.txt` — verbatim licence texts of all linked crates
- `LICENSE-MIT`, `LICENSE-APACHE` — the project's own terms

The licence files and notices are not decoration: MIT, BSD, ISC and Apache all
require their text to be redistributed with a compiled binary, so they travel
next to it and are covered by `SHA256SUMS`.

The script runs the build as the calling user inside `z2m-guard-artifacts:local`,
so nothing in `dist/` ends up root-owned. Two traps it already handles:

- the builder image must install the toolchain under the same channel name as
  `rust-toolchain.toml` (`stable`), otherwise rustup tries to auto-install it
  during the build and fails on a read-only `RUSTUP_HOME`;
- this `cargo-cyclonedx` has no `--output-cdx` flag — it writes
  `<package>.cdx.json` next to the manifest, which the script moves to `dist/`.

Verify: `file dist/z2m-guard-*` says "statically linked", and the binary runs on a
musl host/container (`docker run --rm -v $PWD/dist:/d alpine /d/z2m-guard-... --version`).

## 4. Image

```bash
docker build -t z2m-guard:<version> .
docker buildx build --platform linux/amd64,linux/arm64 -t z2m-guard:<version> --output type=oci,dest=dist/z2m-guard-<version>.oci.tar .
```

The multi-arch build is the only proof the arm64 path compiles; run it before
tagging. Prerequisites on a plain Docker host:

```bash
docker run --privileged --rm tonistiigi/binfmt --install arm64   # qemu user emulation
docker buildx create --name z2m-multi --driver docker-container --use
docker buildx inspect --bootstrap
```

The default `docker` driver cannot build foreign platforms, and without the
binfmt entry the arm64 leg dies with "exec format error". Expect ~15 minutes for
arm64 under emulation on a cold cache — the Dockerfile's cache mounts exist for
exactly that reason.

### Offloading the build to another machine

The build can run on any host with a Docker daemon reachable over SSH; the OCI
archive is exported back to the calling machine, so no registry is needed.

```bash
# on the caller
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519 -N ""      # once
ssh-copy-id -p 556 user@builder-host
cat >> ~/.ssh/config <<'EOF'
Host wsl-build
    HostName 192.168.1.2
    Port 556
    User stopfen
EOF

docker context create remote --docker "host=ssh://wsl-build"
docker buildx create --name z2m-remote --driver docker-container --use remote
docker buildx inspect --bootstrap
docker buildx build --builder z2m-remote --platform linux/amd64,linux/arm64 \
  -t z2m-guard:<version> --output type=oci,dest=dist/z2m-guard-<version>.oci.tar .
```

The remote host needs `docker.io` **and** `docker-buildx` (Debian/Ubuntu ship the
plugin separately) plus `tonistiigi/binfmt --install arm64`. If the daemon is only
reachable through a port forward (WSL2 in NAT mode), the forwarded port changes
whenever the guest gets a new IP — mirror the port forward in a logon task instead
of assuming stability.

Measured on the reference setup (6-core host vs. 24-thread WSL2 guest, both cold
cache): **707 s** wall time on the guest; the earlier local run was not timed with
`time`, it took roughly ten minutes. The extracted binaries are byte-identical
(`amd64` sha256 prefix `45110296…`, `arm64` `9015c100…`). The resulting `index`
digest differs between builders because
BuildKit 0.30 emits per-platform attestation manifests that 0.2x does not; the
payload is unaffected. Reproducibility claim therefore rests on binary hashes, not
on the archive digest.

## 5. Tag (local, private repo)

```bash
git tag -a v<version> -m "z2m-guard <version>"
git push origin v<version>
```

Pushing the tag triggers `.github/workflows/release.yml`: it builds both musl
targets, the SBOM and the licence notices, pushes `ghcr.io/st0pfen/z2m-guard`
for `linux/amd64` + `linux/arm64`, and opens a **draft** release with every
artifact attached. Local `dist/` output stays useful for the reference host —
the pipeline is what publishes.

Creating or editing files under `.github/workflows/` requires a token with the
`workflow` scope. Without it the contents API answers 404 and `git push` is
rejected with a misleading message; that is a scope problem, not a path problem.

## 6. After deploying

Re-verify on the reference host, because gates do not prove the I/O path:

- planned recreate through `deploy/restart-z2m.sh` → one
  `suppressed_events_total{reason="prior_signal"}`, `outage_confirmed_total`
  unchanged
- unexplained `docker stop` → `outage_confirmed_total` +1 after ~15 s
- `curl /healthz` inside the network, Prometheus target `up`, the three Home
  Assistant entities present
