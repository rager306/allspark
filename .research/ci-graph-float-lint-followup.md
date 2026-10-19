# pa-graph float-comparison lint follow-up

Source is stable. All **34** pa-graph `float_cmp` assertions in
`.research/ci-fullgate-1.98.1-clippy-red.log` now compare `.to_bits()` values.
Expected literals have explicit types checked against the actual fields/returns.
The symmetry assertion compares the two existing `f32` results' bits.

## Owned exact paths

| Path | Assertions | Type |
| --- | ---: | --- |
| `crates/pa-graph/src/bm25.rs` | 1 | `f32` |
| `crates/pa-graph/src/condenser.rs` | 4 | `f64` |
| `crates/pa-graph/src/delta_index.rs` | 2 | `f32` |
| `crates/pa-graph/src/hybrid.rs` | 2 | `f32` |
| `crates/pa-graph/src/minhash.rs` | 1 | `f64` |
| `crates/pa-graph/src/coherence.rs` | 2 | `f64` |
| `crates/pa-graph/src/pdx.rs` | 4 | `f32` |
| `crates/pa-graph/src/mcp_brain.rs` | 1 | `f64` |
| `crates/pa-graph/src/prometheus_metrics.rs` | 4 | `f64` |
| `crates/pa-graph/src/homology.rs` | 3 | `f32` |
| `crates/pa-graph/src/voi.rs` | 5 | `f32` |
| `crates/pa-graph/src/drifting.rs` | 5 | `f32` |

Classification: **internals, existing unit-test assertions only**. Every production
prefix before `#[cfg(test)]` is byte-for-byte unchanged. Ownership and dependency
direction stay within pa-graph's README scope; no API, dependency, production
behavior, constant, tolerance, lint allowance, or test coverage change. Expected
zero sentinels are positive zero; exact bit comparison intentionally distinguishes
negative zero. No user-visible surface changes, so parity-diff evidence is not
applicable. RT006 and all other paths were untouched except this report.

## Verification / handoff

Source audit mapped and covered all 34 logged assertion locations, verified types,
and confirmed that the listed assertion substitutions account for every changed
source byte. No Cargo, compiler, formatter, or test command was run by this worker;
the root owns the build queue. Native commands for the root's Rust 1.98.1 queue:

```sh
cargo +1.98.1 fmt --all --check
cargo +1.98.1 clippy -p pa-graph --all-targets -- -D warnings
cargo +1.98.1 test -p pa-graph --lib
```

The focused crate test command includes all touched unit tests. The existing final
log is red evidence for the old assertion syntax; green compiler/test results
remain pending the root's single-queue run.

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
