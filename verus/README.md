# Verus formal-verification specs (OPTIONAL gate)

Mirror specs: pure-function contracts copied from the crates they verify. Verus proves
them for ALL inputs; the Rust sources stay untouched (no Cargo dependency, no unsafe).

## What is covered

| File | Mirrors | Contracts proven |
|------|---------|------------------|
| `pa_mem_ptr48.rs` | `crates/pa-mem/src/ptr48.rs` | `SixBytePtr` encode/decode: decode∘encode == id (value level, requires `idx <= 2^48-1` — the pack() invariant), decode bounded `<= 2^48-1`; `clip_hash`: decoded prefix == `h mod 2^24`, bounded; `raw_pack`/`raw_unpack`: masking roundtrip `mod 2^48`, unpack mask is a proven no-op |
| `pa_mem_bucket.rs` | `crates/pa-mem/src/ptr48.rs` (`Bucket`) | Layout accessors: `set_ptr`/`set_hash` field non-overlap (writing one field preserves the other two byte-for-byte), `empty()` all-zero, `dib` position |
| `pa_core_gates.rs` | `crates/pa-core/src/autonomous/gates.rs` | Decision kernel of `should_autonomously_continue`: never continue when disabled or on terminal error/abort; continue ⇔ GateFailed|MissingTerminalEvidence; Passed ⇒ NotNeeded; RetryExhausted ⇒ LimitReached (when autonomous); limit ⇒ never continue; `gate_attempt_result` RetryExhausted ⇔ `attempt > max_retries` |

## Run

```sh
scripts/verus-check.sh          # skip (exit 0) when the toolchain is absent
```

Install (one-time, ~1.6 GB): download the `x86-linux` zip from
https://github.com/verus-lang/verus/releases into `/root/verus-pilot/` and unzip —
the script looks for `/root/verus-pilot/verus-x86-linux/verus` (override with `$VERUS_BIN`).

## Mirror discipline (must)

Each `verus/*.rs` carries a `MIRROR OF:` header naming the source file. When you edit a
mirrored function's logic, update the mirror in the SAME commit — the specs are copies,
not imports, so drift is silent. The trusted `#[verifier::external_body]` wrappers
(std byte conversions) are the declared trust surface: keep them minimal, review any
new one like you would an `assume`.
