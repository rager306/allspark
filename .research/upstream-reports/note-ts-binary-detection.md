# Bonus note: ts_binary() misidentifies the installed Rust binary as TS

`crates/pa-cli/tests/daemon_commands_e2e.rs::ts_binary()` treats any working
`prime-agent` on PATH as the TypeScript reference. Since the 0.10.0 Rust
install ships `/root/.local/bin/prime-agent` (name parity by design), every
box with the Rust product installed fails the differential lane with
`ts json: EOF while parsing a value` — the "TS daemon" is another Rust
binary whose json surface differs from the harness' TS expectations.

Suggested probe hardening (upstream candidate):
- prefer `PA_TS_REFERENCE`/`PA_TS_BINARY` only, or
- validate the located binary IS TypeScript (e.g. `--version` output marker
  distinct per era) before using it as the golden side.

Local gate workaround: `PA_TS_BINARY=/nonexistent/ts cargo test --workspace`
(the lane's designed skip path).
