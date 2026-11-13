## What this changes

<!-- One paragraph. If it changes behaviour, say what changes for an existing deployment. -->

## Why

<!-- The problem, with numbers where possible (timings, message counts, log lines). -->

## Verification

<!-- What you actually ran, not what should work. Paste the relevant output. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-features`
- [ ] exercised against a real MQTT broker (or: pure logic only, no I/O touched)

## Checklist

- [ ] The guard still writes only to `z2m_guard/#` and its own discovery configs
- [ ] No secrets, tokens or connection strings in code, config, fixtures or logs
- [ ] Docs updated if behaviour, configuration keys or deployment changed