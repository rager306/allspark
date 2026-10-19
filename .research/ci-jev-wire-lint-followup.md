# pa-jev wire float-comparison follow-up

- Source stable: `crates/pa-jev/src/wire.rs`.
- Owned outputs: that source file and `.research/ci-jev-wire-lint-followup.md` only.
- Classification: test-only internals; no production behavior, constants, dependencies, public API, or cross-crate surface changed. Leaf-crate ownership remains unchanged.
- Replaced the nine `float_cmp` assertions reported by `.research/ci-fullgate-1.98.1-clippy-red.log` with exact `.to_bits()` equality. All values are declared `f64` in `Answer`/`Usage`; expected literals use `_f64`.
- Assumption: these assertions check exact decoded fixture sentinels, not approximate numerical results. No epsilon, tolerance, or lint allowance was added. The existing calculated-price tolerance was left unchanged.
- Static inspection verified exactly nine assertion replacements and byte-identical pre-test production code. No Cargo/build/test commands were run; the root owns the single build queue. Existing `client.rs` edits and RT006 were untouched.

## Native verification for the root queue

Run from `/root/prime-agent` with the root's Rust 1.98.1 environment:

```sh
cargo test -p pa-jev --lib wire::tests
cargo clippy -p pa-jev --all-targets -- -D warnings
```

The existing full-gate log supplies the red lint evidence. These queued commands must provide the post-change result; no passing result is claimed here.

## Independent root verification

The corrected native `make check` on Rust1.98.1 passed workspace fmt and
all-target workspace Clippy at `-D warnings`. It later stopped at an unrelated
terminal-harness Kitty-handshake failure; no full workspace/release pass is
claimed. Root then ran `cargo test -p pa-graph -p pa-jev --lib` in the same native
environment: **439 graph +11 Jev tests passed**, zero failures/ignored.
Evidence: `ci-graph-jev-followup-green.log`,
`ci-float-followup-verification.json`. Root independently confirmed all13
production prefixes remain byte-identical. The formatter-reported new assertion
layout was corrected without behavior changes.
