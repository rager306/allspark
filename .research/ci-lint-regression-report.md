# CI lint regression recovery

## Red evidence

GitHub CI run `39191633430` on `d18ccb701` uses Rust/Clippy 1.98.1.
It rejects `clippy::assert_is_empty` crate attributes in `pa-edit` (unknown lint),
and two float equality assertions in `pa-mem/src/matryoshka.rs`.
The full captured failure log is `ci-39191633430-failed.log`.

## Changes

- Remove the 1.99-only lint allowance from workspace/pa-edit and the duplicate pa-cli/pa-core tables, rather than suppress unknown lints.
- Use exact `.to_bits()` equality for float sentinel tests in matryoshka, maxsim, rabitq and turbovec, plus pa-edit parity confidence and pa-jev client reply/billing sentinels; no tolerance or runtime behavior change.
- No dependencies or runtime constants changed. `Cargo.toml` changes lint configuration only.

## Verification

- First local 1.98.1 pass reproduced seven remaining `float_cmp` failures in the focused crates; those assertions were corrected, not suppressed.
- Final focused gate: `cargo +1.98.1 clippy --locked -p pa-edit -p pa-mem --all-targets -- -D warnings` — **exit 0**. Log: `ci-lint-green.log`.
- First full-workspace 1.98.1 gate found the remaining pa-core lint-table copy and a missing `#[must_use]` on the existing public `default_refiner_call` factory. Removed the unsupported allowance and added the focused attribute. No runtime behavior changed.
- This is not a full-workspace pass; the restored workspace gate is tracked separately.
