#!/usr/bin/env bash
# Kani harness lane: bounded model checking for the pure decisions in pa-core.
#
# WHY IT NEEDS A WRAPPER. `cargo install kani-verifier` unpacks a bundle whose
# `kani-compiler` links `librustc_driver-<hash>.so`, and the hash pins ONE
# nightly. The bundle ships without that shared object and without a `cargo`,
# so out of the box `cargo kani` fails twice: "failed to start cargo metadata"
# and "librustc_driver-...: cannot open shared object file". Two facts decide
# the workaround:
#
#   * rustup already has the matching toolchain
#     (nightly-2026-08-21, the version the bundle names in
#     `~/.kani/kani-<ver>/rust-toolchain-version`), so the library exists — it
#     just is not on the loader path `kani-compiler` sees;
#   * the global `~/.cargo/config.toml` sets `build.rustc-wrapper = kache`, and
#     `cargo` clears `LD_LIBRARY_PATH` for children, so neither the wrapper nor
#     an exported variable reaches the compiler.
#
# So this script runs cargo with a scratch CARGO_HOME whose config has no rustc
# wrapper. That is the whole trick, and it is why the lane is a script rather
# than a line in the workflow.
#
# MEASURED 2026-10-27 (12 cores): 3 harnesses, 109 checks, ~1.3 s total.
# The harnesses themselves live beside the code as `#[cfg(kani)]`, which is the
# documented placement; `unexpected_cfgs` for `cfg(kani)` is declared in
# crates/pa-core/Cargo.toml.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KANI_HOME="${KANI_HOME:-$HOME/.kani}"
BUNDLE="$(ls -d "$KANI_HOME"/kani-* 2>/dev/null | sort -V | tail -1 || true)"

if [ -z "$BUNDLE" ] || [ ! -x "$BUNDLE/bin/kani-compiler" ]; then
  echo "kani-check: no Kani bundle under $KANI_HOME — skipping (install: cargo install --locked kani-verifier)"
  exit 0
fi
if ! command -v cargo-kani >/dev/null 2>&1; then
  echo "kani-check: cargo-kani not on PATH — skipping"
  exit 0
fi

TOOLCHAIN="$(cat "$BUNDLE/rust-toolchain-version" 2>/dev/null || true)"
RUSTUP_TOOLCHAIN_DIR="${RUSTUP_HOME:-$HOME/.rustup}/toolchains/$TOOLCHAIN"
if [ ! -d "$RUSTUP_TOOLCHAIN_DIR" ]; then
  echo "kani-check: the bundle needs rustup toolchain '$TOOLCHAIN' (not installed) — skipping"
  echo "kani-check: install it with: rustup toolchain install $TOOLCHAIN"
  exit 0
fi

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/kani-home.XXXXXX")"
cleanup() { rm -rf "$SCRATCH"; }
trap cleanup EXIT
# A wrapper-free cargo config: the global one points rustc at kache, which does
# not forward LD_LIBRARY_PATH to the compiler it execs.
printf '[build]\nrustc-wrapper = ""\n' > "$SCRATCH/config.toml"
cp "${CARGO_HOME:-$HOME/.cargo}/credentials.toml" "$SCRATCH/" 2>/dev/null || true

# The compiler needs its own driver on the loader path; cargo will not pass an
# exported LD_LIBRARY_PATH through, so it is set inside the exec below.
SHIM_DIR="$SCRATCH/shim"
mkdir -p "$SHIM_DIR"
cat > "$SHIM_DIR/kani-compiler" <<SHIM
#!/bin/sh
export LD_LIBRARY_PATH="$RUSTUP_TOOLCHAIN_DIR/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
exec "$BUNDLE/bin/kani-compiler.real" "\$@"
SHIM
chmod +x "$SHIM_DIR/kani-compiler"

# Put the shim where the driver looks for the compiler, then restore the vendor
# layout no matter how the run ends.
RESTORE=0
if [ ! -e "$BUNDLE/bin/kani-compiler.real" ]; then
  mv "$BUNDLE/bin/kani-compiler" "$BUNDLE/bin/kani-compiler.real"
  cp "$SHIM_DIR/kani-compiler" "$BUNDLE/bin/kani-compiler"
  RESTORE=1
fi
cleanup_bundle() {
  if [ "$RESTORE" -eq 1 ]; then
    rm -f "$BUNDLE/bin/kani-compiler"
    mv "$BUNDLE/bin/kani-compiler.real" "$BUNDLE/bin/kani-compiler"
  fi
}
trap 'cleanup_bundle; cleanup' EXIT

cd "$ROOT"
echo "kani-check: verifying pa-core with $BUNDLE ($("$BUNDLE/bin/kani-driver" --version 2>/dev/null | head -1))"
CARGO_HOME="$SCRATCH" RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
  cargo kani -p pa-core "$@"
