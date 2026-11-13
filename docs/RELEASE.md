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
- `SHA256SUMS`
- `z2m-guard-<version>.cdx.json` — CycloneDX 1.5 SBOM from the real lockfile

Verify: `file dist/z2m-guard-*` says "statically linked", and the binary runs on a
musl host/container (`docker run --rm -v $PWD/dist:/d alpine /d/z2m-guard-... --version`).

## 4. Image

```bash
docker build -t z2m-guard:<version> .
docker buildx build --platform linux/amd64,linux/arm64 -t z2m-guard:<version> --output type=oci,dest=dist/z2m-guard-<version>.oci.tar .
```

The multi-arch build is the only proof the arm64 path compiles; run it before
tagging.

## 5. Tag (local, private repo)

```bash
git tag -a v<version> -m "z2m-guard <version>"
git push origin v<version>
```

The tag-triggered release job cannot run yet: the pipeline lives in
`deploy/ci/release.yml` because the current token lacks the `workflow` scope. Once
it is moved to `.github/workflows/release.yml`, a tag push builds both musl
targets, the SBOM and a **draft** GitHub release with `SHA256SUMS` attached.

## 6. After deploying

Re-verify on the reference host, because gates do not prove the I/O path:

- planned recreate through `deploy/restart-z2m.sh` → one
  `suppressed_events_total{reason="prior_signal"}`, `outage_confirmed_total`
  unchanged
- unexplained `docker stop` → `outage_confirmed_total` +1 after ~15 s
- `curl /healthz` inside the network, Prometheus target `up`, the three Home
  Assistant entities present
