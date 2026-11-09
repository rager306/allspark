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

# `SELF` is captured before the `cd`: `--self-test` re-invokes the guard as a
# subprocess, and `$0` may be a relative path.
SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
ROOT="${ROOT:-$(cd "$(dirname "$SELF")/../.." && pwd)}"
cd "$ROOT" || exit 1
# The guard parses `rg` output; colour codes would poison every comparison.
export NO_COLOR=1 CLICOLOR=0 CLICOLOR_FORCE=0

fail=0
pass() { printf '  ok    %s\n' "$*"; }
bad()  { printf '  FAIL  %s\n' "$*"; fail=1; }

# ── Facts the checks assert (also read by --self-test) ─────────────────────
# Declared ONCE, before the self-test. The self-test builds its fixture from
# these same names, because a fact the checks read but the fixture never copies
# is a check that can only fail (review #40: the fixture omitted
# `pa-edit/src/tool.rs`, so check 4b was red before any mutation and every
# "detected" break was detected by accident).
#
# The roster moved out of `engine.rs` on 2026-10-30: registration lives in its own
# module, which is the whole point of the split (upstream edits engine.rs ~29 times
# per 100 commits). The guard follows the fact, not the old path.
ENGINE="crates/pa-core/src/session_engine/native_tools.rs"
EXPECTED_NATIVE="edit graph jev lsp schedule"
CORE_LIB="crates/pa-core/src/lib.rs"
CLI_MANIFEST="crates/pa-cli/Cargo.toml"
EDIT_TOOL="crates/pa-edit/src/tool.rs"
EDIT_TOOL_OLD="crates/pa-core/src/tools/edit_hybrid.rs"
FORK_CRATES=(pa-ast pa-diff pa-edit pa-graph pa-jev pa-lsp pa-mem)
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
  crates/pa-daemon/src/agent_engine/provider_admission
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

# Rewrite `$1` in place: replace every literal `$2` with `$3`. Prints `0` when
# the pattern matched nothing - a mutation that changes nothing would "prove" a
# check it never exercised.
rewrite() {
  python3 - "$1" "$2" "$3" <<'PY'
import sys
path, old, new = sys.argv[1:4]
text = open(path, encoding="utf-8").read()
if old not in text:
    print(0)
    raise SystemExit(0)
open(path, "w", encoding="utf-8").write(text.replace(old, new))
print(1)
PY
}

if [[ "${BASH_SOURCE[0]:-}" == "$0" && "${1:-}" == "--self-test" ]]; then
  # Proof that each check CAN fail. Three properties make that proof real, and
  # each one was missing once (review #40):
  #
  #  1. the fixture CARRIES every fact the checks read;
  #  2. the CLEAN fixture must be green first - otherwise "3/3 detected" says
  #     nothing, because every run is red before any mutation;
  #  3. each break must redden the check it targets, matched by that check's own
  #     text. "Something went red" is not detection.
  scratch="${TMPDIR:-$ROOT/.tmp}"
  mkdir -p "$scratch" || exit 1
  tmp=$(mktemp -d "$scratch/fork-surface-self-test.XXXXXX") || exit 1
  trap 'rm -rf "$tmp"' EXIT
  clean="$tmp/clean"
  m="$tmp/mutated"
  facts=("$ENGINE" "$CORE_LIB" "$CLI_MANIFEST" "$EDIT_TOOL" "${INSIDERS[@]}")
  for crate in "${FORK_CRATES[@]}"; do facts+=("crates/$crate/src/lib.rs"); done
  for fact in "${facts[@]}"; do
    mkdir -p "$clean/$(dirname "$fact")"
    cp -r "$fact" "$clean/$fact"
  done

  if ! baseline=$(ROOT="$clean" bash "$SELF" 2>&1); then
    echo "self-test: FAILED — the UNMUTATED fixture is already red, so it proves nothing:"
    printf '%s\n' "$baseline"
    exit 1
  fi
  echo "  ok    self-test: clean fixture is green (${#facts[@]} facts copied)"

  # Each mutation runs against a fresh copy of the clean fixture.
  fresh() { rm -rf "$m"; cp -a "$clean" "$m"; }
  mutate() { # file literal replacement label
    if [[ $(rewrite "$1" "$2" "$3") != 1 ]]; then
      echo "self-test: FAILED — $4 injected nothing: $1 does not contain $2"
      exit 1
    fi
  }
  remove() { # path label
    if [[ ! -e $1 ]]; then
      echo "self-test: FAILED — $2 injected nothing: $1 does not exist"
      exit 1
    fi
    rm -rf "$1"
  }
  problems=0
  expect_red() { # label expected-check-text
    local label=$1 want=$2 out
    if out=$(ROOT="$m" bash "$SELF" 2>&1); then
      printf '  FAIL  self-test: %s left the guard GREEN\n' "$label"
      problems=$((problems + 1))
    elif [[ $out != *"$want"* ]]; then
      printf '  FAIL  self-test: %s reddened a different check than %q:\n' "$label" "$want"
      printf '%s\n' "$out"
      problems=$((problems + 1))
    else
      printf '  ok    self-test: %-24s -> %s\n' "$label" "$want"
    fi
  }

  # 1. a native tool drops off the roster
  fresh
  mutate "$m/$ENGINE" 'native_tool_names.push("graph".to_string());' '' "native push removal"
  expect_red "native push removal" "native tool roster changed"

  # 2. a native name is advertised with no tools.push behind it
  fresh
  mutate "$m/$ENGINE" 'tools.push(Arc::new(' 'tools.extend(Arc::new(' "unbacked native name"
  expect_red "unbacked native name" "a name is advertised without a tool"

  # 3. a FORK-ONLY marker is stripped
  fresh
  mutate "$m/crates/pa-graph/src/lib.rs" 'FORK-ONLY' 'fork owned' "FORK-ONLY removal"
  expect_red "FORK-ONLY removal" "lost its 'FORK-ONLY' marker"

  # 4. a fork-owned path inside an upstream crate is deleted - graph/mod.rs, the
  #    file that exists (the old break removed graph.rs, which never did)
  fresh
  remove "$m/crates/pa-core/src/tools/graph/mod.rs" "fork path deletion"
  expect_red "fork path deletion" "fork file vanished inside an upstream crate"

  # 5. the native edit tool vanishes from pa-edit (drifts back to pa-core)
  fresh
  remove "$m/$EDIT_TOOL" "edit tool drift"
  expect_red "edit tool drift" "native edit tool moved back into pa-core"

  # 6. the E1 codec export disappears
  fresh
  mutate "$m/$CORE_LIB" 'decode as decode_repeated_output, ' '' "E1 export removal"
  expect_red "E1 export removal" "lost the E1 codec export"

  # 7. the payload binary is renamed
  fresh
  mutate "$m/$CLI_MANIFEST" 'name = "prime-agent"' 'name = "allspark"' "payload rename"
  expect_red "payload rename" 'no longer declares [[bin]] name = "prime-agent"'

  if [[ $problems -ne 0 ]]; then
    echo "self-test: FAILED — $problems break(s) were not caught by the check they target"
    exit 1
  fi
  echo "self-test: ok (clean fixture green; all 7 injected breaks turned their own check red)"
  exit 0
fi


# ── 1. The four gated native tools still register ────────────────────────────
# The AGENTS.md surface contract: bash/edit/python_repl from the kernel, PLUS four
# gated natives. A merge that drops one leaves the prompt advertising a tool the
# runtime never pushes.
if [[ ! -f $ENGINE ]]; then
  bad "engine.rs missing: $ENGINE"
else
  # `--color=never` explicitly: the guard parses this output, and an inherited
  # ripgrep colour setting would poison the comparison.
  actual=$(rg --color=never -N -o 'native_tool_names\.push\("([a-z_]+)"' -r '$1' "$ENGINE" 2>/dev/null | sort -u | tr '\n' ' ')
  actual="${actual% }"
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
  pushes=$(rg --color=never -c 'tools\.push\(Arc::new' "$ENGINE" 2>/dev/null || echo 0)
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
for crate in "${FORK_CRATES[@]}"; do
  lib="crates/$crate/src/lib.rs"
  if [[ ! -f $lib ]]; then
    bad "fork crate file missing: $lib"
  elif ! rg --color=never -q 'FORK-ONLY' "$lib"; then
    bad "$lib lost its 'FORK-ONLY' marker"
  fi
done
[[ $fail -eq 0 ]] && pass "all ${#FORK_CRATES[@]} fork crates carry FORK-ONLY"

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
if [[ -f $EDIT_TOOL ]] && [[ ! -f $EDIT_TOOL_OLD ]]; then
  pass "native edit tool lives in pa-edit (fork crate), not pa-core"
else
  bad "native edit tool moved back into pa-core (or vanished from pa-edit)"
fi

# ── 5. The E1 codec export survives (kernel stdout spill) ────────────────────
# `decode_repeated_output` is the fork's reversible representation for clipped
# bash output; losing the export silently downgrades every spill to a lossy clip.
if rg --color=never -q 'pub use tools::repeat_codec::\{decode as decode_repeated_output' "$CORE_LIB" 2>/dev/null; then
  pass "E1 codec export present (decode_repeated_output)"
else
  bad "$CORE_LIB lost the E1 codec export (repeat_codec::decode)"
fi

# ── 6. Payload identity is still upstream ────────────────────────────────────
# docs/fork-identity.md: the binary is `prime-agent`, env names stay upstream, so
# `git merge upstream/main` keeps applying. A rename here breaks the launcher's
# socket pin and every parity diff.
if rg --color=never -q '^\s*name = "prime-agent"$' "$CLI_MANIFEST" 2>/dev/null; then
  pass "payload binary is still named prime-agent"
else
  bad "$CLI_MANIFEST no longer declares [[bin]] name = \"prime-agent\""
fi


echo
if [[ $fail -eq 0 ]]; then
  echo "fork surface: INTACT"
else
  echo "fork surface: BROKEN (see FAIL lines above)"
fi
exit $fail
