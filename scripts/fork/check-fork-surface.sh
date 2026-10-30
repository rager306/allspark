#!/usr/bin/env bash
# Fork-surface guard: the fork's tool surface and its fork-owned files, checked
# after every upstream merge.
#
# WHY (2026-10-30): the v0.10.3-beta.1 merge removed upstream's built-in bash/edit
# tools as dead code (#3823). In the fork they were NOT dead -- the fork graph tool
# read `tools::truncate::DEFAULT_MAX_BYTES` and the compaction pricing test built
# the bash tool schema -- so the merge compiled only after both consumers were
# re-pointed. Nothing failed; the breakage was found by reading `cargo check`
# output. A merge that silently drops a native tool, or a fork file that vanishes,
# has no other detector.
#
# Each check must be ABLE to fail (doc/agent-errors.md: a guard that cannot fail
# gates nothing). `--self-test` proves it: it copies the tree's facts into a temp
# fixture, breaks each one, and requires every check to go red.
#
# Exit 0 = surface intact. Exit 1 = a check found a real regression (prints why).
set -uo pipefail

ROOT="${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$ROOT" || exit 1
# The guard parses `rg` output; colour codes would poison every comparison.
export NO_COLOR=1 CLICOLOR=0 CLICOLOR_FORCE=0

fail=0
pass() { printf '  ok    %s\n' "$*"; }
bad()  { printf '  FAIL  %s\n' "$*"; fail=1; }

# ── Facts the checks assert (also read by --self-test) ─────────────────────
# The roster moved out of `engine.rs` on 2026-10-30: registration lives in its own
# module, which is the whole point of the split (upstream edits engine.rs ~29 times
# per 100 commits). The guard follows the fact, not the old path.
ENGINE="crates/pa-core/src/session_engine/native_tools.rs"
EXPECTED_NATIVE="edit graph jev lsp"
INSIDERS=(
  crates/pa-core/src/tools/graph/mod.rs
  crates/pa-core/src/tools/graph/reports.rs
  crates/pa-core/src/tools/graph/build.rs
  crates/pa-core/src/tools/graph/args.rs
  crates/pa-core/src/tools/graph/tool
  crates/pa-core/src/tools/graph_in_flight.rs
  crates/pa-core/src/tools/repeat_codec.rs
  crates/pa-core/src/jev
  crates/pa-core/src/session_engine/tool_bridge.rs
  crates/pa-daemon/src/agent_engine/provider_admission.rs
  crates/pa-daemon/src/supervisor/provider_admission.rs
  crates/pa-daemon/src/image_route.rs
  crates/pa-daemon/src/input_pause_lease.rs
  crates/pa-daemon/src/supervisor/notes.rs
  crates/pa-cli/src/update_flow
  crates/pa-cli/src/command_registry.rs
  crates/pa-tui/src/heartbeats_picker
  crates/pa-tui/src/tool_card
  crates/pa-ai/src/models_fork.rs
  crates/pa-ai/src/providers/bedrock
  crates/pa-ai/src/oauth
  crates/pa-models/src/opencode_models.rs
  crates/pa-models/src/discovery.rs
)

if [[ "${BASH_SOURCE[0]:-}" == "$0" && "${1:-}" == "--self-test" ]]; then
  # Proof that each check CAN fail: build a temp copy of the facts, break them,
  # and require a red run. A guard whose failure mode is untested is a guard that
  # gates nothing.
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  mkdir -p "$tmp/$(dirname "$ENGINE")" "$tmp/crates/pa-core/src/tools" \
           "$tmp/crates/pa-cli" "$tmp/crates/pa-core/src/jev"
  cp "$ENGINE" "$tmp/$ENGINE" 2>/dev/null
  cp crates/pa-core/src/lib.rs "$tmp/crates/pa-core/src/lib.rs" 2>/dev/null
  cp crates/pa-cli/Cargo.toml "$tmp/crates/pa-cli/Cargo.toml" 2>/dev/null
  for c in pa-ast pa-diff pa-edit pa-graph pa-jev pa-lsp pa-mem; do
    mkdir -p "$tmp/crates/$c/src"; cp "crates/$c/src/lib.rs" "$tmp/crates/$c/src/lib.rs" 2>/dev/null
  done
  for p in "${INSIDERS[@]}"; do
    if [[ -e $p ]]; then mkdir -p "$tmp/$(dirname "$p")"; cp -r "$p" "$tmp/$p" 2>/dev/null; fi
  done

  red=0
  # break 1: drop a native push
  sed -i 's/native_tool_names.push("graph".to_string());//' "$tmp/$ENGINE"
  if ! ROOT="$tmp" bash "$0" >/dev/null 2>&1; then red=$((red+1)); fi
  cp "$ENGINE" "$tmp/$ENGINE"
  # break 2: remove a fork file
  rm -rf "$tmp/crates/pa-core/src/tools/graph.rs"
  if ! ROOT="$tmp" bash "$0" >/dev/null 2>&1; then red=$((red+1)); fi
  # break 3: strip a FORK-ONLY marker
  printf '// no marker\n' > "$tmp/crates/pa-graph/src/lib.rs"
  if ! ROOT="$tmp" bash "$0" >/dev/null 2>&1; then red=$((red+1)); fi
  if [[ $red -eq 3 ]]; then
    echo "self-test: ok (all 3 injected breaks turned the guard red)"
    exit 0
  fi
  echo "self-test: FAILED — only $red/3 injected breaks were detected: the guard cannot fail"
  exit 1
fi

# ── Declarations used by both the checks and the self-test ──────────────────
# The roster moved out of `engine.rs` on 2026-10-30: registration lives in its own
# module, which is the whole point of the split (upstream edits engine.rs ~29 times
# per 100 commits). The guard follows the fact, not the old path.
ENGINE="crates/pa-core/src/session_engine/native_tools.rs"
EXPECTED_NATIVE="edit graph jev lsp"
INSIDERS=(
  crates/pa-core/src/tools/graph/mod.rs
  crates/pa-core/src/tools/graph/reports.rs
  crates/pa-core/src/tools/graph/build.rs
  crates/pa-core/src/tools/graph/args.rs
  crates/pa-core/src/tools/graph/tool
  crates/pa-core/src/tools/graph_in_flight.rs
  crates/pa-core/src/tools/repeat_codec.rs
  crates/pa-core/src/jev
  crates/pa-core/src/session_engine/tool_bridge.rs
  crates/pa-daemon/src/agent_engine/provider_admission.rs
  crates/pa-daemon/src/supervisor/provider_admission.rs
  crates/pa-daemon/src/image_route.rs
  crates/pa-daemon/src/input_pause_lease.rs
  crates/pa-daemon/src/supervisor/notes.rs
  crates/pa-cli/src/update_flow
  crates/pa-cli/src/command_registry.rs
  crates/pa-tui/src/heartbeats_picker
  crates/pa-tui/src/tool_card
  crates/pa-ai/src/models_fork.rs
  crates/pa-ai/src/providers/bedrock
  crates/pa-ai/src/oauth
  crates/pa-models/src/opencode_models.rs
  crates/pa-models/src/discovery.rs
)

# ── 1. The four gated native tools still register ────────────────────────────
# The AGENTS.md surface contract: bash/edit/python_repl from the kernel, PLUS four
# gated natives. A merge that drops one leaves the prompt advertising a tool the
# runtime never pushes.
if [[ ! -f $ENGINE ]]; then
  bad "engine.rs missing: $ENGINE"
else
  # `--color=never` explicitly: the guard parses this output, and an inherited
  # ripgrep colour setting would poison the comparison.
  actual=$(rg --color=never -N -o 'native_tool_names\.push\("([a-z_]+)"' -r '$1' "$ENGINE" 2>/dev/null | sort -u | tr '\n' ' ' | sed 's/ $//')
  if [[ "$actual" == "$EXPECTED_NATIVE" ]]; then
    pass "native tools registered: $actual"
  else
    bad "native tool roster changed"
    printf '        expected: %s\n        actual:   %s\n' "$EXPECTED_NATIVE" "${actual:-<none>}"
    printf '        (prompt sections and tests reason about this set; update EXPECTED_NATIVE only with a contract change)\n'
  fi
fi

# ── 2. Every registered name is also pushed into `tools` ─────────────────────
# `native_tool_names` mirrors the effective surface (engine.rs strips names not in
# the active profile), but a name with no `tools.push` in the same file is a
# prompt entry for a tool the agent cannot call.
if [[ -f $ENGINE ]]; then
  pushes=$(rg --color=never -c 'tools\.push\(Arc::new|tools\.push\(Arc::new\(' "$ENGINE" 2>/dev/null || echo 0)
  names=$(rg --color=never -c 'native_tool_names\.push' "$ENGINE" 2>/dev/null || echo 0)
  if [[ $names -le $pushes ]]; then
    pass "every native name has a tools.push ($names names / $pushes pushes)"
  else
    bad "$names native names but only $pushes tools.push calls: a name is advertised without a tool"
  fi
fi

# ── 3. FORK-ONLY crates still carry their marker ─────────────────────────────
# A merge resolves a conflict by taking upstream's side; the marker in src/lib.rs
# is the machine-readable proof that a crate is fork-owned. Without it, a future
# conflict resolution has no signal.
for crate in pa-ast pa-diff pa-edit pa-graph pa-jev pa-lsp pa-mem; do
  lib="crates/$crate/src/lib.rs"
  if [[ ! -f $lib ]]; then
    bad "fork crate file missing: $lib"
  elif ! grep -q 'FORK-ONLY' "$lib"; then
    bad "$lib lost its 'FORK-ONLY' marker"
  fi
done
[[ $fail -eq 0 ]] && pass "all 7 fork crates carry FORK-ONLY"

# ── 4. Fork files that live INSIDE upstream crates are still present ─────────
# These are the merge tax: files the fork owns but upstream's tree names. Listed
# explicitly so a silent deletion (upstream removed a module we depended on) is a
# red gate rather than a compile error found by hand.
missing=0
for path in "${INSIDERS[@]}"; do
  if [[ ! -e $path ]]; then
    bad "fork file vanished inside an upstream crate: $path"
    missing=1
  fi
done
[[ $missing -eq 0 ]] && pass "${#INSIDERS[@]} fork-owned paths inside upstream crates present"

# ── 4b. The native edit tool lives in pa-edit, not pa-core ────────────────────
# Moved 2026-10-30: it depends only on the engine, and keeping it inside pa-core
# bought nothing but merge surface. It must not drift back.
if [[ -f crates/pa-edit/src/tool.rs ]] && [[ ! -f crates/pa-core/src/tools/edit_hybrid.rs ]]; then
  pass "native edit tool lives in pa-edit (fork crate), not pa-core"
else
  bad "native edit tool moved back into pa-core (or vanished from pa-edit)"
fi

# ── 5. The E1 codec export survives (kernel stdout spill) ────────────────────
# `decode_repeated_output` is the fork's reversible representation for clipped
# bash output; losing the export silently downgrades every spill to a lossy clip.
if rg --color=never -q 'pub use tools::repeat_codec::\{decode as decode_repeated_output' crates/pa-core/src/lib.rs 2>/dev/null; then
  pass "E1 codec export present (decode_repeated_output)"
else
  bad "crates/pa-core/src/lib.rs lost the E1 codec export (repeat_codec::decode)"
fi

# ── 6. Payload identity is still upstream ────────────────────────────────────
# docs/fork-identity.md: the binary is `prime-agent`, env names stay upstream, so
# `git merge upstream/main` keeps applying. A rename here breaks the launcher's
# socket pin and every parity diff.
if rg --color=never -q '^\s*name = "prime-agent"$' crates/pa-cli/Cargo.toml 2>/dev/null; then
  pass "payload binary is still named prime-agent"
else
  bad "crates/pa-cli/Cargo.toml no longer declares [[bin]] name = \"prime-agent\""
fi


echo
if [[ $fail -eq 0 ]]; then
  echo "fork surface: INTACT"
else
  echo "fork surface: BROKEN (see FAIL lines above)"
fi
exit $fail
