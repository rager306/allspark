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
# The bundle is probed, not assumed: this machine can carry more than one
# (`$HOME/.kani` under an isolated HOME, `/root/.kani` under the real one), and a
# partial unpack — no `toolchain/` — sorts HIGHEST by name, so "newest bundle"
# picked the one that cannot run at all. Candidates are equal here; the first one
# that can actually verify wins.
kani_homes() {
  if [ -n "${KANI_HOME:-}" ]; then
    printf '%s\n' "$KANI_HOME"
    return
  fi
  printf '%s\n' \
    "${HOME:-/root}/.kani" \
    /root/.kani \
    /root/.allspark-home/.kani
}

BUNDLE=""
for home in $(kani_homes); do
  [ -d "$home" ] || continue
  while IFS= read -r candidate; do
    [ -n "$candidate" ] || continue
    # A COMPLETE bundle wins: `kani-driver` execs `<bundle>/toolchain/bin/cargo`
    # for `cargo metadata`, so a bundle without it cannot verify anything. The
    # probe therefore prefers one that has both, and only falls back to a
    # compiler-only unpack to report the repair honestly below.
    if [ -x "$candidate/bin/kani-compiler" ] && [ -x "$candidate/toolchain/bin/cargo" ]; then
      BUNDLE="$candidate"
      break
    fi
    if [ -z "${PARTIAL_BUNDLE:-}" ] && [ -x "$candidate/bin/kani-compiler" ]; then
      PARTIAL_BUNDLE="$candidate"
    fi
  done <<EOF
$(ls -d "$home"/kani-* 2>/dev/null | sort -rV || true)
EOF
  [ -n "$BUNDLE" ] && break
done

if [ -z "$BUNDLE" ]; then
  if [ -n "${PARTIAL_BUNDLE:-}" ]; then
    echo "kani-check: the only bundle under $(kani_homes | tr '\n' ' ') is partial (no toolchain/bin/cargo): $PARTIAL_BUNDLE — skipping"
    echo "kani-check: repair with: cargo install --locked kani-verifier (a complete bundle ships toolchain/)"
    exit 0
  fi
  echo "kani-check: no Kani bundle under $(kani_homes | tr '\n' ' ') — skipping (install: cargo install --locked kani-verifier)"
  exit 0
fi
# A bundle that carries no `toolchain/` cannot resolve the `cargo` it runs
# metadata through: `kani-driver` execs `<bundle>/toolchain/bin/cargo`, not the
# `cargo` on PATH, so the lane dies with "failed to start `cargo metadata`" on a
# machine where cargo is perfectly available. Measured 2026-11-22: the bundle
# under an isolated HOME (`.../kani-0.68.0`) had no `toolchain/`, while the one
# under the real home had a complete toolchain and verified 5 harnesses. A
# partial unpack is skipped in favour of a complete one by the probe above; when
# EVERY candidate is partial the lane skips with the repair named, never a RED
# that blames the code.
if [ ! -x "$BUNDLE/toolchain/bin/cargo" ]; then
  echo "kani-check: the bundle at $BUNDLE has no toolchain/bin/cargo — skipping"
  echo "kani-check: repair with: cargo install --locked kani-verifier (a complete bundle ships toolchain/)"
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
# KANI_HOME must name the bundle this script PROBED. `cargo kani` resolves the
# bundle by itself from `$HOME/.kani` — which on an isolated HOME is the partial
# unpack this script just refused, so the lane failed with "failed to start
# `cargo metadata`" while the correct bundle sat one directory away (measured).
# Handing the resolved home over is what makes the probe authoritative.
CARGO_HOME="$SCRATCH" KANI_HOME="$(dirname "$BUNDLE")" \
  RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
  cargo kani -p pa-core "$@"
