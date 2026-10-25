# Black-box TUI testing in 2026 — and the two silent failures that shipped through it

**Date:** 2026-10-24 · **Mode:** read-only research + empirical probes on this tree (no code changes, no
releases, no git operations). Every live process I started was isolated under `/tmp/<name>` and reaped by
pid; the ambient daemon (pid 3289830, `--daemon-socket /tmp/allspark-daemon.sock`) was never signalled.

**Scope:** (1) how a TUI is tested as a black box in 2026 — tools, oracles, the "rendered but empty" class,
remote-data pickers, CI determinism; (2) **build identity** for a Rust binary: why a version string cannot
identify a build, and what to compare on the daemon handshake, in the release artifact, and in `--version`.

---

## 0. What is already in this repo's research corpus (read first, not repeated here)

| Existing report | What it already covers | Status |
|---|---|---|
| `.research/blackbox-qa-best-practices.md` (2026-10-17) | Anthropic evals canon, pass@k/pass^k, Terminal-Bench 2.0, τ-bench, CORE path metrics, judge calibration, shadow/canary | **Does not** cover terminal-emulator oracles, screen-state assertions, or build identity. |
| `.research/fork-testing-research.md` | Codex `wiremock`/`context_snapshot`, grok `MockInferenceServer` + PTY harness, omp `VirtualTerminal` (kitty-VT), wire-shape pins, soak gates, cassettes | Names the PTY tier and `insta` once ("Snapshots as gate for TUI", its sections 1.3 and 1.6); **no** comparison of TUI tooling, **no** empty-state class, **no** build identity. |
| `.research/TEST-MATRIX.md` | Tier model, contract gates, positive/negative fixtures, metrics drift | TUI appears only as "headless tests"; no screen oracle. |
| `.research/oh-my-pi.md`, `oh-my-pi-deep.md` | omp's architecture patterns (advisor, taste model) and a crate-by-crate inventory for porting (`pi-edit`/`pi-ast`/`pi-diff`, LOC + `unsafe` counts) | **Do not** cover TUI testing at all (their subject is code portability). The `VirtualTerminal`/`waitForRender` detail lives in `fork-testing-research.md`'s oh-my-pi section, not here. |
| `.research/learn_blackbox_qa.md` (2026-10-19) | Six-system harness comparison, oracle hierarchy, 33 catalogued techniques, `fix_offbyone_add` black-box design, empirical timings on our binary | The best prior art here. **Its oracle hierarchy ranks "snapshot of the rendered screen" at #5 and never distinguishes "the frame rendered" from "the frame has data".** That omission is exactly this incident. |
| `.research/PACKAGING-FORK-009-REVIEW.md` (2026-10-23) | The fork.009 tarball was missing `prime-agent-runtime/` and `skills/` | A packaging-defect genre with the same shape as today's: the artifact exists, the payload is incomplete, nothing notices. |

### What this report adds (the delta)

1. **The empty-vs-broken distinction as a first-class oracle class.** Prior reports test "did the frame
   render"; nothing in the corpus says "assert the collection is non-empty *before* asserting on its rows".
   §2.1–§2.4 give the rule and the mechanism; §5.2 gives the tests and the red-without-the-fix column that
   proves each can fail.
2. **A 2026 tool survey with a decision table** — `termlens`, Microsoft `tui-test`, `ratatui-testlib`
   (`terminal-testlib`), `expectrl`, `vt100`, `alacritty_terminal`, tmux-as-oracle, asciinema casts,
   `insta` — priced against *our* measured numbers (§1, Table A). Prior reports named at most two of these.
   Also the negative result: `divineTerm` does not exist, and "Playwright for TTY" resolves to `termless`
   (a Node toolchain we should not adopt).
3. **Five confirmed findings from probing this tree today**, each with `file:line` and a reproduction:
   a fixture catalogue shipped as the release asset and replacing the compiled one (§2.2 Probe A/A2); the
   *sharper* form — **a bundled asset that mentions one model deletes every other compiled model**, provider by
   provider (Probe A1, a five-row table); `pin_catalog_models` rejecting an asset only when it pins to *nothing*,
   so a *partial* asset is more dangerous than a broken one (Probe B); two divergent daemon classifiers that
   answer differently for one live daemon (§6.5); and `buildId` being a function of the version, so "same
   version, different code" is invisible (§6.1). Probe A1 is the one I would put in front of a reviewer: it is
   four `model list` runs and it makes the mechanism undeniable.
4. **Build identity for Rust, 2026 practice** — `vergen`/`build.rs`, `SOURCE_DATE_EPOCH`, ELF `NT_GNU_BUILD_ID`,
   `cargo-auditable`, artifact SHA-256 vs. build id, and what each one can and cannot prove (§6).
5. **A build-id handshake contract** that survives "same version, different code" — the case the current
   design cannot see at all (§6.6), with the exact test tiers that go red without it (§6.7).
6. **Measured facts about our own TUI** that make the plan cheap: boot-to-ready 115–145 ms, `/model` →
   picker 27–37 ms, full picker row count 196 (fixture asset) vs 192 (compiled), all through tmux (§5.1).

### Where each question from the brief is answered

| Brief | Section |
|---|---|
| 1. Tools and approaches, per-tool "what it proves / price / catches empty / CI stability" | §1.2 (Table A), §1.1 (the four layers) |
| 2. How to tell "drawn correctly" from "drawn, but empty" | §2.1 (why snapshots cannot), §2.4 (the rule), §5.2 T5/T6 (the tests) |
| 3. Picker/table with remote data: fixtures vs mock vs recorded | §3.1 (the four producers), §3.2 (the assert shape) |
| 4. CI determinism, flake control, failure artifacts | §4.1 (five knobs), §4.2 (serialization + artifacts) |
| 5. The first 5–8 TUI tests, in order, and which catches this incident | §5.2 (nine rows, T1a→T7, with a red-without-fix column), §7 |
| Build identity: 2026 Rust practice | §6.2 (four kinds), §6.3 (mechanisms priced) |
| Build identity: runtime check, replace-vs-refuse | §6.5, §6.6 |
| Build identity: how to test it | §6.7 (B1–B7) |
| Build identity: published artifact vs `--version` | §6.8 |

---

## 1. How a TUI is tested black-box in 2026

### 1.1 The four layers, and what each one structurally cannot see

The consensus across the surveyed tools is a four-layer split, not a single harness:

| Layer | Runs | Sees | Cannot see |
|---|---|---|---|
| **Widget** | in-process `draw` into a `Buffer` | cell content and styles at a chosen size | event loop, keys, exit codes, raw-mode/alt-screen, output printed outside the framework |
| **Headless app** (our `HeadlessPlan`) | real app loop, in-process, scripted steps | key dispatch, state transitions, frame text | PTY semantics (raw mode, resize signals), process exit, capability probes |
| **PTY + VT emulator** | real binary on a real pty, bytes decoded by a terminal model | everything above *plus* alt-screen enter/leave, cursor restore, `println!` leaks, `SIGWINCH`, OSC title/clipboard | nothing about internals — by construction it only sees what a user sees |
| **Record** | asciinema/`.cast`/GIF | a human-reviewable artifact | nothing automatically |

Ratatui's own docs state the boundary explicitly: `TestBackend` "tests rendering in-process, but does not
cover your event loop, key handling, terminal setup and teardown or exit codes", and recommends a PTY harness
for "a small number of important user flows" — not as a replacement for widget tests
(<https://ratatui.rs/recipes/testing/snapshots/>, fetched 2026-10-24). Codex's `codex-rs/tui` follows the same
split (many `insta` snapshots over `TestBackend`, a small `PtyCodex` layer for process-level behaviour —
`terminal.wait_for_screen()`; per DeepWiki `openai/codex`, **UNVERIFIED** against the tree: I did not clone it).

### 1.1a What this repo already has, so the plan does not rebuild it

| Asset | Where | Count / shape | Gap it leaves |
|---|---|---|---|
| Headless TUI e2e (real app loop, scripted steps, captured frames) | `crates/pa-tui/tests/*_headless.rs` | **43 files** | asserts on frames the test *supplies* the catalog for — never the packaged catalog |
| PTY e2e with a real child on `openpty` + a mock supervisor | `crates/pa-cli/tests/*pty*_e2e.rs` | **4 files**, 2,703 lines | none of them opens `/model` against a *packaged* asset |
| `openpty` users across the tree | `rg -l openpty` | **20 files** | hand-rolled per test (byte-needle waits), no shared screen oracle |
| `vt100` decoder | `crates/pa-tui` (dev-dep), `crates/pa-perf/src/bench/pty.rs:32,84` | 15 files reference it | `PtyChild` is `pub(crate)` to `pa-perf` and its decoded-screen accessor `screen_text()` is private (`crates/pa-perf/src/bench/pty.rs:257`) — a bench-only asset, so the integration tests cannot reuse it |
| `/model`-in-a-pty e2e | `crates/pa-cli/tests/login_panel_pty_e2e.rs:462-505` | 1 test | proves the sign-in routing; the catalog comes from the mock supervisor, so packaging is invisible |
| Daemon health surface | `crates/pa-cli/src/daemon_discovery/*`, `prime-agent doctor --json` | works, prints `status`/`executablePath`/`buildId` | the launch path does not consult it (§6.5) |

**The conclusion that shapes §5:** the missing piece is not a pty harness (four exist) and not a screen model
(`vt100` is already linked). It is **(a)** one test whose catalog comes from the packaged artifact, and **(b)** one
shared predicate for "is this daemon my build". Both are small, and both are in the tier we already trust.

### 1.2 Table A — tool → what it proves → price → verdict for us

Prices are the honest ones for this workspace: a 78 GB `target/`, one test binary per integration file, and
the gate already at ~275 units.

| Tool (crates.io / repo) | What it actually proves | Price | Catches "screen empty"? | CI stability | Verdict |
|---|---|---|---|---|---|
| **`ratatui` `TestBackend`** (in `ratatui` 0.29, already a dependency) | the buffer a `draw` produced, at a chosen `(w,h)`, cell-exact; `assert_buffer_lines`, `assert_cursor_position`, `assert_scrollback` | ~0 (already linked); µs per test | Only if you assert a non-empty invariant yourself | Perfect (no pty, no clock) | **Recommended** for the picker's *rendering* rules (already done: `crates/pa-tui/src/model_picker/tests.rs:579`) |
| **`insta` snapshots over `TestBackend`** | the whole frame as reviewed text; diffs on change | ~0 runtime; review cost per change; **not currently in the workspace** (`rg 'insta::'` → 0 hits, 0 `.snap` files) | No — a snapshot of an empty panel is a valid snapshot | Perfect; `CI=true` makes insta refuse to write new snapshots (<https://insta.rs/docs/quickstart/>) | **Recommended, narrowly**: pin 3–5 frames that a reviewer reads once. Not for the data-driven list (see §2.4) |
| **`termlens` 0.11.6** (vyncint, 2026-10-14) | real binary on a real pty; `wait_until`/`wait_frame`/`snapshot_after`/`wait_stable`; screen grid + styles + cursor + modes + OSC; `unsupported()` tells a plausible grid from a right one | one dev-dependency; pty + 80×24 per test | Only via your predicate — but its `snapshot_after` settles first, which removes the "snapshot taken mid-paint" flake | Designed for it: no `sleep`, every wait deadline-bounded and carries the screen into the failure message; documented Windows/ConPTY caveats | **Recommended as the pty oracle** (§5.2). It is the only surveyed crate with frame-accurate `DEC 2026` waits and an artifact story |
| **Microsoft `tui-test`** / `tui-test-rs` 0.1.1 (2026-10-23) | real shell/TUI, semantic `text` locators with retry, styles, mouse, resize, snapshots, failure artifacts, trace viewer; backends Alacritty/Ghostty/Rio/xterm.js | external binary + a daemon; a Rust binding that loads a native lib | Yes — `expect text`/`expect snapshot` are assertions, not just reads | Mature; `--failure-artifacts`, `[trace] mode=on-failure` | **Good second opinion, not first choice**: a daemonized out-of-tree binary in a merge gate is a new failure mode for us |
| **`ratatui-testlib` / `terminal-testlib` 0.1.0** (2025-12-01) | pty + emulator + `insta`, Sixel/kitty graphics, Bevy ECS, "headless for CI" | one crate; Linux-focused per its README | Only via your assert | Young (one release), Bevy-oriented | **No** — `termlens` covers the same need with a stability policy and a spec doc |
| **`expectrl` 0.9.0** / **`rexpect` 0.7.1** | byte/stream matching with timeouts on a pty | one crate | **No** — stream matching cannot see a screen | Workable but every wait is your problem | **No** for assertions; fine for "type and wait" | 
| **`vt100` 0.18.1** (doy) | a screen model from bytes | already a dev-dep of `pa-tui`/`pa-perf` | No | Deterministic (pure function of bytes) | **Use it as the decoder**, not the harness — this is what `crates/pa-perf/src/bench/pty.rs` already does |
| **`alacritty_terminal` 0.26.0** | the most faithful VT model (reflow, wide chars, sixel) | a heavy dep, unstable API | No | Deterministic | Only if a display bug needs reflow fidelity |
| **tmux as oracle** (`tmux capture-pane -p -S -`) | the rendered pane as text, from a real terminal a user could attach to | **9.2 ms median** per capture (20 samples, min 7.6/max 17.6); needs `tmux` in CI | Only via your assert | Good, but `send-keys` timing is yours to solve (see §5.1's two traps) | **Recommended as the *evidence* layer** for 2–3 user journeys; not the gate |
| **asciinema `.cast`** (v3 format: newline-delimited JSON, interval timing — <https://docs.asciinema.org/manual/asciicast/v3/>) | a replayable recording for a human | free; a few hundred KB per run | No | N/A (not an assertion) | **Recommended** on failure only, for the picker journey |
| **Charm `vhs`** (`.tape` scripts) | GIF of a scripted session | needs `ttyd`+`ffmpeg` | No | Tuned for demos | No |
| **`divineTerm`** | *does not exist* — the only "divine" hits are a diesel test-lead kit and a model checker. Treat the name in the brief as a false lead. | — | — | — | **UNVERIFIED/nonexistent** |
| **Playwright-for-TTY** | the credible instance is **`beorn/termless`** (Vitest matchers, 12 backends incl. `vt100-rust` and Alacritty via napi) — a Node toolchain | new runtime in the repo | Yes (matchers) | Multi-backend is its selling point; irrelevant to us | **No** — a second language runtime for a Rust repo |

### 1.3 What "black box" means here, precisely

The user-visible contract of a TUI is not "the process didn't crash". The four things a black-box suite can
hold it to, in increasing fragility (extending `.research/learn_blackbox_qa.md` §3.1 with the TUI rows):

```
1. Process contract:       exit code, alt-screen entered AND left, cursor restored
2. Screen invariants:      a set of needles that must be present, and a set that must be absent
3. Non-emptiness:          the collection being displayed has N > 0 rows, counted from the screen
4. Frame snapshots:        the whole grid, normalized, reviewed once
```
Rows 1–3 are cheap and stable. Row 4 is expensive per change and — this is the load-bearing point — **row 4
subsumes nothing about row 3**: a snapshot faithfully records an empty list, and it will keep passing.

---

## 2. The central problem: "drawn correctly" vs "drawn, but empty"

### 2.1 Why a screenshot diff cannot see it

A screenshot/golden comparison answers exactly one question: *did the pixels change since the last approval?*
It says nothing about whether the pixels were ever right. The two failure modes are orthogonal:

- **The frame is wrong** (layout broke, a row is clipped) → snapshot diff catches it. This is what snapshots are for.
- **The frame is right, the data is not** (the list is empty, or full of placeholder rows) → snapshot diff
  **locks it in**. The reviewer approves a picture of an empty picker, and every later run agrees with it.

Kent C. Dodds names the decay mechanism: a suite that fails for irrelevant changes "quickly erode[s] the team's
trust in a test" (<https://kentcdodds.com/blog/effective-snapshot-testing>). Percy's 2026 snapshot-testing
write-up makes the same distinction — a snapshot detects *unintended* change, not *wrong content*
(<https://percy.io/blog/snapshot-testing>). And `insta` itself will not write a new snapshot under `CI=true`,
so in CI a *new* empty-state snapshot is simply a failure with a diff nobody has reviewed
(<https://insta.rs/docs/quickstart/>).

The 2026 practice that closes this is to treat non-emptiness as a **separate, mandatory pre-condition**, checked
before (or independently of) any snapshot:

1. **Semantic screen assertions** — search for domain strings, not pixels: a known model id, a known provider
   name, the count row. `termlens` (`screen.contains`, `find`, `find_all`), `tui-test` (`expect text` with
   retry and `--not`) and tmux (`capture-pane | grep`) all provide this; the discipline is that the needle must
   come from the *data*, not from the layout.
2. **Collection-cardinality assertion** — count the rows on the screen and compare to the *known size of the
   served fixture*: `assert_eq!(rows_on_screen, catalog.len())`. This is the assertion that distinguishes
   "empty" from "not drawn": an empty frame yields 0, a fully-broken frame yields 0, a correct frame yields N,
   and a *substituted* frame (see §2.2) yields N with wrong names — which the next rule catches.
3. **Set equality against the source of truth** — every row's `(provider, id)` must be a member of the
   catalog the daemon served, and the set must be equal (not a subset). A subset assertion passes on any
   truncation; equality fails.
4. **Empty-state *detection*, not just empty-state *rendering*** — the app may legitimately render "No matching
   models" (there is a test for that: `crates/pa-tui/src/model_picker/tests.rs:579`). The bug is that the *same
   frame* is produced for "the user typed a filter that matches nothing" and "the catalog arrived empty".
   A test must therefore assert on the reason: with an empty filter and a non-empty catalog, the no-match row
   must be **absent**.
5. **Snapshot only after 1–3 pass**, and only for surfaces where layout itself is the contract.

### 2.2 What our binary actually does — five probes, run today

All five use a hermetic `env -i` home under `/tmp`, `--offline`, and the `dist` binary
(`sha256 116d1346…`), with the daemon started per-probe on its own socket.

**Probe A — a bundled asset replaces the compiled catalog wholesale.**
`crates/pa-models/src/chain.rs:183`: `Some(bundled) => bundled`. `crates/pa-models/src/bundled.rs:84-121` never
merges the compiled list; it only appends Prime Inference offline entries. Measured with the *shape the release
actually ships* (`--fixture`, `.github/workflows/release-fork.yml:127`):

```
picker over the compiled catalog        → (1/192) rows, contains "GLM-4.7" and the upstream models
picker over the shipped fixture asset   → (1/196) rows; the upstream providers' rows read
                                          "Fixture <provider> model N" (46 of them), and "GLM-4.7" is
                                          ABSENT. What still resolves to real ids is only what does not
                                          come from the asset: the fork table and the native-free
                                          OpenCode rows. Measured: filtering that picker by "glm" yields
                                          (1/4) — "GLM-5.3 (Genspark)", "GLM 5.3 Flash (OpenCode free)",
                                          "GLM 5.3" and "GLM 5.3 Flash" (both prime-inference) — i.e. every
                                          real GLM the user could pick, but the *upstream-catalog* GLM rows
                                          (zai's own `glm-4.7`, `glm-5.3`) are gone.
```
(`GLM-4.7` present: `False` for the fixture asset, `True` without it — measured via `tmux capture-pane` on
`/model`.) So a release install does not show *no* models; it shows **46 synthetic placeholder rows plus the
fork/Prime-Inference backups** — providers are there, the user's real models are not. That is the exact
user-visible shape of the report ("провайдеры есть, а моделей нет"), and **the frame is perfect**: the picker
renders, scrolls, shows `(1/196)`, the price columns, the hint row.

**Probe A1 — the replacement is total: an asset that mentions one model deletes every other compiled model.**
This is the minimal, most damning form. Each row is `prime-agent --offline model list`
(= `get_discoverable_models()`), the same hermetic home, `anthropic` + `zai` credentials so the compiled models
are eligible, varying **only** `models.bundled.json`. (The `model list` row count is small because `--offline`
and a fresh home means only these two providers are "configured"; the picker shows the full 192 because the
daemon's `catalog_payload` does not auth-filter.)

| bundled asset | rows | which providers survive |
|---|---|---|
| **absent** (compiled fallback) | 4 | `anthropic/claude-fable-5`, `opencode` ×2, `zai/glm-4.7` |
| `{"models": []}` | 4 | same — an empty asset fails pinning (`"Catalog has no models supported by this client"`) and falls back |
| **one `anthropic` entry** | 3 | `anthropic/fixture-one`, `opencode` ×2 — **`zai/glm-4.7` is gone**, and so is `claude-fable-5` |
| **one `zai` entry** | 3 | `opencode` ×2, `zai/glm-4.7` — **`anthropic` is gone entirely** |
| one entry with an unpinned transport (`muse`) | 4 | the asset fails pinning wholesale → falls back to compiled (so a *bad* asset is safer than a *partial* one) |

The last two rows are the mechanism in one sentence: **`Some(bundled) => bundled` (`chain.rs:183`) is a
replacement, not a merge, and `pin_catalog_models` rejects the asset only when it pins to *nothing***. A release
that ships a catalogue covering 40 of 43 transports silently loses the other 3 providers' models, and no layer
reports it — `load_bundled_models` returns `Some(vec)`, `resolve` returns it, the registry chains the fork table
on top, and the picker renders a healthy-looking list. This is why the guard at `bundle_catalog.py:202-210` is
necessary but not sufficient: it checks *the fork's* providers, not the compiled set (§5.2 T1b generalizes it).

**Probe A2 — the release workflow's catalogue is a *fixture*, and the in-flight fix already turns it red.**
`.github/workflows/release-fork.yml:117-129` runs `generate --fixture` then `verify --out`, and `--fixture`
exempts the asset from the minimum gates (`scripts/release/bundle_catalog.py:842`:
`allow_small = options.allow_small_fixture or options.fixture`). So the shipped `models.bundled.json` is the
**synthetic 46-row fixture**, which is what makes Probe A reproduce on a real install rather than a contrived
one. The uncommitted working-tree change to `bundle_catalog.py` adds a fork-provider gate
(`:202-210`, `FORK_PROVIDERS` at `:113-117`); measured on this tree today:

```
HEAD version of the script:   generate --fixture → rc 0 ; verify --out → rc 0  (43 tuples, no fork check)
working-tree version:         generate --fixture → rc 0 ; verify --out → rc 1
  error: Bundled model catalog is missing the fork's providers
         ['codebuddy', 'genspark', 'muse', 'ninerouter', 'subquadratic'] …
```
Two things follow. **The gate works and is red for the right reason** — but it is red on the workflow step that
already exists, so the release job will fail until one of three things happens: the fixture grows real rows for
the fork's providers, the workflow passes the real catalogue (`--network`), or the gate is deliberately waived
for the fixture (which removes the protection it was written for). A gate that fails the release *before* the
artifact exists is the cheapest possible place to learn this; a gate that only fires after publishing is not a
gate. **This is the packaging half of T1 and it costs nothing to keep.**

**Probe B — a non-compiled transport tuple is dropped with no signal.**
`crates/pa-models/src/pinning.rs:68-95`: a `(provider, api, baseUrl)` that is not in the compiled table is
`continue`d — the comment at `:85` says "Non-matching tuples drop silently: the catalog can never introduce a
transport" (correct as a security rule, wrong as an observability rule). Measured: a bundled asset with
`good-model` (`anthropic`) + `muse-x` (`muse`, `https://api.muse.example/v1`) listed **only** `good-model`; the
muse row vanished silently, and `model list` exited 0.

**Probe C — with zero credentials the picker is still fully populated, from the daemon, not from the
startup snapshot.** (Measured on its own hermetic home, `models.bundled.json` = `{"models": []}`.)
`crates/pa-core/src/models/registry.rs:588-604` chains `pa_ai::models_fork::fork_provider_models()` after the
catalog chain (`:600`), and the launch path's startup snapshot is `registry.get_available()`
(`crates/pa-cli/src/interactive_mode.rs:458`), which filters to providers with **configured auth**
(`registry.rs:219-241`). Measured with **no `auth.json` at all** and a bundled asset of `{"models":[]}`:

```
$ prime-agent --offline model list          # get_discoverable_models (native-free included)
opencode  deepseek-v4.1-flash-free
opencode  glm-5.3-flash-free                 ← 2 rows

/model picker, settled (tmux capture)        → (1/192) rows, every one "require sign in"
```
So the startup snapshot is empty (no provider has auth — inferred from `get_available`'s filter at
`registry.rs:219-241`; I did **not** observe a zero-model first frame, because the background refresh from
`crates/pa-daemon/src/model_catalog.rs:25-60` (`catalog_payload` at `:104`) lands before the first Enter is
processed — in a 15 ms-interval sampling run the very first capture that showed the picker already read
`(1/192)`). Two consequences, both load-bearing:

(a) **the fork table is what keeps this from being an empty picker** — delete one `.chain()` at `registry.rs:600`
and the same home renders `No matching models` with no error anywhere;
(b) **the model list depends on which of two snapshots the picker folds**, so a test must assert on the
**settled** picker. A test that asserts on the frame right after `/model` is asserting on a race, and a test that
stubs `InteractiveOptions::model_catalog` never sees the daemon path at all.

**What this is *not*.** The 429 the user saw from `z.ai` is a real billing state (the parent's direct curl shows
`code=1113 "Insufficient balance"` for `glm-5.3`, `glm-5.3-flash`, `glm-4.7`) — not a code defect, and not fixed
by anything in this report. It is, however, the *reason the incident was noticed*: the user picked a model,
the request failed, and while looking at that they found the list itself was wrong. The catalogue defect and the
billing state are independent; conflating them would send the next reader to the wrong file.

### 2.3 The stale-daemon cause, reproduced live

The incident report's second cause is separate from the catalog: the TUI talked to an **older daemon on the same
socket**. Reproduced today without touching any installed file:

```
1. start the OLD payload as a supervisor on a private socket:
   /root/.local/libexec.manual-old/prime-agent --mode daemon --daemon-socket /tmp/stale-repro/daemon.sock
2. start the NEW payload's TUI against that socket
3. press /model
```
Result: the new TUI renders the **old** daemon's catalog — `Claude Fable 5`, `GPT-5.4`, `(1/1356)` — with no
warning, no notice, no log line. Meanwhile, on the same socket:

```
$ prime-agent doctor --json
[{ "version": "0.10.1-beta.35",
   "buildId": "pa-daemon-rs-0.10.1-beta.35",
   "executablePath": "/root/.local/libexec.manual-old/prime-agent",
   "status": "stale", ... }]
```
`doctor` is right and nobody reads it; the launch path is wrong. That is §6.5.

### 2.4 Why the empty list must be a *separate* assert (the general rule)

Three properties make "non-empty" a distinct oracle rather than a detail of some other assertion:

1. **It is the cheapest possible check** (one integer compare) and it fails for *every* cause of an empty
   list: a dropped asset, a failed fetch, a stale daemon with no models, a filter bug, a wrong provider key.
   Anthropic's evals guidance puts the same idea at the level of tasks — "grade what the agent produced, not
   the path it took" (<https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents>) — and the
   analogue for a list is "grade the list's contents, not the fact that a list widget painted".
2. **Vacuous truth is the default.** Any loop over the rendered rows (`for row in rows { assert!(...) }`) passes
   trivially on zero rows. Playwright's docs make the same point for `locator.count() === 0`
   (<https://playwright.dev/docs/api/class-locator>): a bare count is not an assertion. Every surveyed TUI tool
   offers an explicit `expect`/`assert` for exactly this reason.
3. **Mutation-style reasoning**: a test that would still pass if the feature returned `[]` is not testing the
   feature. Our own `crates/pa-tui/src/model_picker/tests.rs:579`
   (`an_empty_catalog_opens_the_empty_panel`) is *correct as a rendering rule* and *useless as a data rule* —
   it asserts that empty renders as empty. Nothing asserts the converse.

---

### 2.5 Reproducing the user's screen in 40 seconds (the recipe a maintainer will actually use)

```bash
# hermetic home; the dist payload; the fixture asset the release ships; no credentials needed
root=$(mktemp -d /tmp/incident.XXXX)
mkdir -p "$root/agent" "$root/libexec"
cp /root/.local/libexec/models.bundled.json "$root/libexec/"     # or: bundle_catalog.py generate --fixture
printf '{"onboardingShown":true,"onboardingCompleted":true}' > "$root/agent/settings.json"
printf '{"zai":{"type":"api_key","key":"sk-x"}}'              > "$root/agent/auth.json"
tmux new-session -d -s inc -x 110 -y 34
tmux send-keys -t inc "exec /usr/bin/env -i HOME=$root PRIME_AGENT_CODING_AGENT_DIR=$root/agent \
  PRIME_AGENT_DAEMON_SOCKET=$root/d.sock PI_PACKAGE_DIR=$root/libexec \
  DO_NOT_TRACK=1 NO_COLOR=1 TERM=screen-256color PATH=/usr/local/bin:/usr/bin:/bin \
  /root/prime-agent/target/dist/prime-agent --offline" Enter
sleep 1; tmux send-keys -t inc -l '/model'; sleep 0.2; tmux send-keys -t inc Enter; sleep 1
tmux capture-pane -t inc -p | tail -20      # → (1/196), "Fixture <provider> model N"
tmux kill-session -t inc; rm -rf "$root"
```
This is the exact sequence every measurement in this report used; it needs no network, no installed payload and
no credentials. Verified verbatim today — the capture it prints is:

```
 >  Search models
────────────────────────────────────────────────────────────────────────────────
› Fixture zai model 42                                              current · zai
  Fixture amazon-bedrock model 0                         require sign in · amazon-bedrock
  Fixture amazon-bedrock model 1                         require sign in · amazon-bedrock
  Fixture Claude (reasoning)        ■■■□   medium         require sign in · anthropic
  Fixture anthropic model 4                              require sign in · anthropic
  Fixture azure-openai-responses model 5      require sign in · azure-openai-responses
  (1/196)
 ↑/↓ model · ←/→ effort · Enter select · Esc close
```
Every visible row is synthetic; the picker is otherwise flawless.

## 3. Data from a remote source: fixtures vs mock server vs recorded bytes

### 3.1 The three options, and what each can prove

| Option | Proves | Cannot prove | Cost here | Use for |
|---|---|---|---|---|
| **Static fixture file** (a JSON catalog on disk) | the *renderer's* behaviour for a given input set | that the input can come from the real source; that packaging ships it | ~0 | picker layout, sorting, search, effort rows |
| **Mock server / in-process mock** (our `MockSupervisor` unix-socket supervisors, `crates/pa-tui/tests/model_picker_provider_headless.rs:47`; the daemon's `CatalogServer` in `crates/pa-daemon/src/model_catalog.rs:420-470`) | the *whole client path*: wire shape, status codes, ordering, refresh timing | that the real source's bytes parse | seconds | TUI behaviour against a controlled catalog, including "the catalog arrives empty" and "the catalog arrives late" |
| **Recorded real bytes (VCR/cassette)** | that the real provider's response parses and the real ids resolve | that today's provider still says that | one recording + redaction | the one test per release that would catch "the live catalog renamed everything" |

The distinguishing question for *this* incident is different from all three: **which layer owns the list that
reaches the picker?** In our tree there are four, and a test only proves the layer it stubs:

```
disk cache (provider-model-catalog.v1.json)
  └─ bundled asset (models.bundled.json)          ← packaging owns this; the release substitutes it
       └─ compiled table (models_generated.rs)     ← the code owns this
            └─ + models_fork table (chain at registry.rs:600)
                 └─ + live Prime Inference snapshot (credentialed)
                      └─ daemon's catalog_payload → wire → TUI's model_catalog
```
A picker test whose `InteractiveOptions.model_catalog` is hand-built (the table's first row) proves **the renderer** and
nothing else. Our existing picker test does exactly that: `duplicate_id_catalog()` constructs two `Model`s in
the test (`crates/pa-tui/tests/model_picker_provider_headless.rs:35-52`). That is the right test for its
question (provider disambiguation) and it *cannot* see this incident.

**The rule that follows:** for every user-visible collection, one test must take the list from the *same*
producer the product uses (the packaged asset on disk, resolved through `pa_models::ModelCatalog`), and assert
on the rendered screen. That is the tier where packaging and rendering meet, and it is the only tier where
this defect is visible.

### 3.2 Empty must fail as its *own* assert, with its own message

The failure text has to name the reason, or the next reader repeats the investigation:

```
BAD   assert!(screen.contains("GLM"), "{screen}")
GOOD  assert!(
        rows.len() > 0,
        "the picker rendered {} data rows from a catalog of {} models (asset: {}, \
         dropped by pinning: {}) — the frame painted, the data did not",
        rows.len(), catalog.len(), asset_path.display(), dropped
      );
```
`termlens` builds this in: "every wait returns either the thing you asked for or a timeout carrying the screen"
(`README.md`), and its `unsupported()` accessor distinguishes "a plausible grid" from "a right one" — the direct
analogue of "rendered" vs "rendered correctly".

---

## 4. CI determinism for TUI tests

### 4.1 The five knobs that actually cause flakes

| Knob | Rule | Where it bites us |
|---|---|---|
| **Never `sleep`** | wait on an observable predicate, with a deadline. `termlens`: "There is no `sleep` in termlens"; opencode's own TUI-test skill is blunter — "Never absorb a sleep, poll or production interval with a larger timeout" | any new pty test |
| **Terminal size** | pin it (`110×34` here) and *set it on the pty* (`TIOCSWINSZ`) before spawn; a `pty.openpty()` default is 80×24 and our picker's layout changes with width | `crates/pa-cli/tests/*_pty_e2e.rs` already do this |
| **`TERM` / feature probes** | `TERM=screen-256color` under tmux, `xterm-256color` on a raw pty; **answer the probes** (`\x1b[?u`, DA1) or the app waits — `crates/pa-perf/src/bench/pty.rs:15-19` documents exactly this pair | a harness that ignores `\x1b[?u` will hang or degrade |
| **Environment leakage** | scrub `PRIME_AGENT_*`/`PI_*`; the gate already does it (`scripts/fork/prepush-gate.sh:57`) because a live session's overrides "point spawned daemons at the live root" | any test that spawns a daemon |
| **Frame settling** | assert on *complete* frames. Our renderer brackets repaints in `DEC 2026` (`\x1b[?2026h` … `\x1b[?2026l`, visible in every captured stream here), so a decoder that ignores 2026 can snapshot a half-painted grid | `termlens::wait_frame` is built for exactly this; a hand-rolled decoder is not |

### 4.2 Serialization and artifacts

- **Serialize pty tests.** They are process- and CPU-sensitive; this repo already uses a `HARNESS_LOCK`
  (`crates/pa-cli/tests/chat_open_first_frame_pty_e2e.rs:61,72`) and `nextest` groups with `max-threads = 1` are the
  established analogue (Codex `nextest.toml`, per `.research/fork-testing-research.md` §1).
- **Emit the screen on failure**, in the failure message and as a file. `termlens` does both
  (`TERMLENS_ARTIFACT_DIR` + a report action); `tui-test` has `--failure-artifacts`/`trace`. For us the cheapest
  version is: on failure, write `capture-pane -p -S -` output plus the resolved catalog ids into
  `CARGO_TARGET_TMPDIR`, and paste the last 20 rows into the panic message.
- **Do not retry.** A retried pty test is a test that has not told you which of the two states it saw. Codex's
  `retries = 1` is a 10k-test suite's insurance, not a design.

---

## 5. The concrete plan

### 5.1 Measured baseline (this machine, this tree, today)

Via tmux 3.4, `/root/prime-agent/target/dist/prime-agent`, hermetic `env -i`, 110×34:

| Step | Measured |
|---|---|
| spawn → "Collapsed mode" visible (ready marker) | **115–145 ms** (4 runs: 136 / 145 / 117 / 115; polled at 10 ms) |
| `/model` + Enter → "Search models" visible | **27–37 ms** (4 runs: 28 / 33 / 27 / 37, same runs) |
| `tmux capture-pane -p -S -` | **9.2 ms median** (20 samples: min 7.6, max 17.6) |
| picker row count, compiled catalog | **192** = 42 unique compiled non-PI + 110 PI + 40 fork. The compiled table has 45 non-PI blocks but only 42 unique `(provider,id)` pairs (3 ids repeat under a different base URL), and the registry's merge keeps one row per pair — hence 152 + 40 = 192. `GLM-4.7` present |
| picker row count, shipped fixture asset | **196** = 46 pinned fixture entries + 110 compiled Prime-Inference offline + 40 fork-table entries (exact: `models.bundled.json` 46, `models_generated.rs` `provider == "prime-inference"` 110, `models_fork.rs` 40). `fixture-*` ids for every upstream provider; `GLM-4.7` absent; filter `glm` → 4 real rows (genspark / opencode-free / prime-inference ×2) |
| picker row count, no credentials at all | **192** rows, every row `require sign in` (the fork table + daemon refresh) |
| `doctor --json` on a foreign-version daemon | `status: "stale"` + `executablePath`, in <1 s |

Timings are wall-clock from the `tmux send-keys` to the first capture that shows the needle, polled by
repeated `capture-pane` (each capture itself costs ~9 ms, so these are upper bounds on the app's own latency).

Two traps found while measuring, worth writing into the harness:

1. **The first-run onboarding panel steals keystrokes.** Before `/traces` is answered, typing `/model` lands in
   the onboarding surface and nothing opens (observed: the frame stayed on `Welcome to PRIME Agent` and the
   picker never appeared). Seed `settings.json` with `{"onboardingShown": true, "onboardingCompleted": true}`
   (`crates/pa-core/src/settings/types.rs:632-633`) or the test is a race.
2. **`send-keys '/model'` can race the autocomplete.** Sending the whole string and `Enter` as one `send-keys`
   call sometimes produced a frame with the slash menu open and the literal `/model` in the editor (no picker).
   Two calls with a ≥100 ms gap, and `-l` for the literal, were stable across every probe after that. Never
   replace the wait with a bigger sleep: wait for `Search models`.

### 5.2 The first nine tests, in build order (T1 is split into T1a/T1b)

Each row states the tier from `.research/TEST-MATRIX.md`'s model, the file, and **what goes red without the
fix** — because a test that cannot fail is documentation.

| # | Test | Tier | File | What it proves | Red without the fix |
|---|---|---|---|---|---|
| **T1a** | `the_release_catalogue_carries_every_compiled_provider` | packaging unit (python) | `scripts/release/test_catalog_assets.py` (the existing suite) | generated fixture asset's provider set ⊇ `models_generated.rs`'s provider set, and no id matches a placeholder pattern | **Already red** on the working-tree gate (`verify` rc 1, §2.2 Probe A2) — it needs the fixture to grow real rows, or the workflow to fetch the real catalogue. ~15 min |
| **T1b** | `a_bundled_asset_never_narrows_the_catalog_silently` | integration (no pty) | `crates/pa-models/tests/bundled_asset_shadowing.rs` | table-test the resolve over (i) no asset, (ii) `[]`, (iii) **one** compiled-provider entry, (iv) the fixture asset, (v) an entry with an unpinned transport; assert the resolved `(provider,id)` set ⊇ the compiled set **minus explicitly superseded ids**, and that every dropped compiled id is reported (a counted `warn!` or a return value) | **Yes** — probe A1 shows case (iii) silently deletes a whole provider today. ~1 h |
| **T2** | `a_bundled_entry_with_an_unpinned_transport_is_reported` | unit | `crates/pa-models/src/pinning.rs` (`#[cfg(test)]`) | `pin_catalog_models` returns the dropped list (or logs a counted warning) instead of dropping silently | **Yes** against the `continue` at `:85`. ~30 min |
| **T3** | `two_daemon_classifiers_agree` | unit | `crates/pa-cli/src/interactive_mode/daemon.rs` (`#[cfg(test)]`) | one shared predicate; a hello with a different `appVersion` (and later `buildHash`) is *not* `Current` for **both** `probe_daemon` and `classify_reachable` | **Yes** — today `classify_reachable` says Stale and `probe_daemon` says Current for the same hello. ~45 min |
| **T4** | `a_stale_daemon_is_replaced_before_the_picker_opens` | pty e2e | `crates/pa-cli/tests/stale_daemon_takeover_pty_e2e.rs` | a fake supervisor answering an **older** hello; launch the TUI; assert the daemon was shut down and a new one started (or that the TUI refuses with an actionable message) | **Yes** — today it rides the old daemon silently (§2.3). ~3 h (the `login_panel_pty_e2e` mock-supervisor + re-exec pattern is the template) |
| **T5** | `the_picker_renders_every_model_the_daemon_served` | pty e2e (tmux or termlens) | `crates/pa-cli/tests/model_picker_catalog_pty_e2e.rs` | boot → `/model` → count rows on screen, compare to the served catalog size, assert set equality on `(provider,id)`, assert the no-match row is **absent** | **Yes** — with the packaged fixture asset the set equality fails on the first real model. ~4 h first time, ~20 min after the harness exists |
| **T6** | `an_empty_catalog_is_not_a_silent_empty_panel` | headless (existing harness) | `crates/pa-tui/tests/model_picker_empty_state_headless.rs` | with `model_catalog: vec![]` and a non-empty filter, "No matching models" appears; with an empty filter and a genuinely empty catalog, the panel carries an explanatory line (not a bare list) | **Yes** — today both cases render the same frame (`render.rs:98-100`). ~1 h |
| **T7** | `the_model_picker_journey_records_a_cast` | pty (artifact only, non-gating) | `crates/pa-cli/tests/model_picker_cast_e2e.rs` | the same journey as T5, recorded to `.cast` on failure | n/a (evidence, not oracle). ~1 h |
| **T8** | `the_daemon_handshake_rejects_a_different_build_of_the_same_version` | unit + pty | `crates/pa-daemon/src/supervisor/clients.rs` tests + a pty e2e with a fake hello | the handshake compares a **build hash**, not only a version (§6.6) | **Yes** — the case §6.1 describes is invisible today. ~2 h (plus the stamping work in §6.3) |

Order: **T1a → T1b → T2 → T3 → T6 → T5 → T4 → T8 → T7.** T1a/T1b/T2 are ~2 h total and close the incident's
root cause; T3/T4 close the second cause; T5/T6 close the *class*; T8 closes the third (build identity). Total
≈ 13 h unsupervised, less if T4 borrows the mock-supervisor scaffolding from
`crates/pa-cli/tests/login_panel_pty_e2e.rs:651-720` (`MockSupervisor` + `serve`), `:577-600` (`spawn_child`: re-exec the test binary with a child-mode env var), `:173-260` (`child_options`), and `PtyReader` at `:595-640`.

**Which single test would have caught this incident?** T1a — the packaging gate, if it asserts on a catalogue
produced the way the release produces it. T5 alone would not: with the compiled catalog in place every row is
present and the screen is right; the defect is upstream of the renderer. That is the lesson worth recording:
**the incident was not a TUI bug, it was a data-provenance bug that a TUI happened to display.** The TUI tier's
job is to make the *user-visible consequence* unmissable and to name the cause in one message; the packaging
tier's job is to refuse to build the file in the first place.

---

## 6. Build identity: why a version string cannot identify a build

### 6.1 The state of this tree (measured, today)

| Fact | Evidence |
|---|---|
| **No `build.rs` anywhere in `crates/`** | `find crates -name build.rs` → empty |
| **No git/commit/source hash baked into anything** | `rg 'GIT_HASH\|git_sha\|commit_hash\|SOURCE_DATE'` → 0 hits outside `install-rust.sh`'s comment text |
| **`buildId` is a function of the version** | `crates/pa-daemon/src/supervisor/clients.rs:126`: `build_id: concat!("pa-daemon-rs-", env!("CARGO_PKG_VERSION"))` |
| **`schemaId` is a hand-written literal** | `crates/pa-types/src/daemon/mod.rs:17`: `"protocol-7-schema-30-8e4b17c2a9f5"` — a manual constant, asserted against itself at `:177`; it never changes unless a human edits it |
| **The only content-derived identity already present is the ELF build id** | `readelf -n` on `target/dist/prime-agent` → `Build ID: 247167fc8874b7726d0171b38a87659137e9aa8d`; the old payload → `07f78b3ca75335f88aa3299c049ffe6b46589783`. Two different binaries, two different GNU build ids — but nothing reads them |
| **`ALLSPARK_FORK_BUILD` is the existing stamp mechanism** | `crates/pa-cli/src/config.rs:61` (`option_env!`), consumed by `--version` (`crates/pa-cli/src/lib.rs:240`) and by the installer funnel (`crates/pa-core/src/update/installer.rs:284,762`) |

The live proof that this is not theoretical: `--version` on `target/dist/prime-agent` prints

```
allspark 0.10.2-beta.3 (fork build dev; upstream prime-agent 0.10.2-beta.3)
```
and every field in that line is derived from `CARGO_PKG_VERSION` plus one optional stamp. Two consequences, both
demonstrated above: (a) **two builds of the same version with different code are indistinguishable** by any field
the process publishes — the `dist` and the `debug` binary here have different bytes (`116d1346…` vs `b7ae805a…`)
and different ELF build ids (`247167fc…` vs `83e86099…`), and if both were version `0.10.2-beta.3` with the same
stamp, nothing in `--version`, `daemon_hello`, or `doctor` would tell them apart; (b) `fork build dev` is what an
*unstamped* build reports (`crates/pa-cli/src/config.rs:61`), so a locally rebuilt release-channel payload says
`dev` and looks like a development build.

### 6.2 What "build identity" means in 2026 practice

Four distinct things are commonly conflated. Keeping them apart is the whole design:

| Kind | What it is | Changes when | Proves | Cost |
|---|---|---|---|---|
| **Source revision** | `git rev-parse HEAD`, plus `-dirty` | any commit / any uncommitted edit | *which source* | one `build.rs` call |
| **Build description** | rustc version, target triple, features, profile, `SOURCE_DATE_EPOCH` | toolchain/flags change | *how it was built* | a few `cargo:rustc-env` lines |
| **Content identity** | SHA-256 of the artifact; or the ELF `NT_GNU_BUILD_ID` | any byte of the output changes | *which artifact*, exactly | free (the release already computes SHA-256) |
| **Provenance** | signed attestation binding artifact → source → builder | per build | *who built what from what* | a workflow step |

Mapping to our mechanisms: `buildId` today is a **build description** that omits everything except the version.
The handshake needs **content identity** (or at least source revision + dirty flag). The release artifact needs
**content identity**, which it already publishes as `sha256` in `latest.json`
(`.github/workflows/release-fork.yml:193-207`) and `SHA256SUMS` (`:183`), verified by the installer
(`install-rust.sh:1506-1600`).

### 6.3 The Rust mechanisms, priced

| Mechanism | Use | Price | Notes |
|---|---|---|---|
| **`build.rs` + `git rev-parse HEAD`** + `cargo:rustc-env=PA_BUILD_SHA=…` | source revision, `+dirty` when `git status --porcelain` is non-empty | ~30 lines; **one process spawn per build** (cached otherwise); breaks on a source tarball / vendored build — must fall back to a stamped env var, not to `"unknown"` | The canonical approach; <https://stackoverflow.com/questions/43753491/include-git-commit-hash-as-string-into-rust-program> |
| **`vergen` 10.0.3 / `vergen-gix` 10.0.3** (56.8M downloads, updated 2026-08-24) | the same, plus `VERGEN_BUILD_*`, `VERGEN_CARGO_*`, `VERGEN_RUSTC_*`, `VERGEN_SYSINFO_*`; emits `rerun-if-changed=.git/HEAD` and `rerun-if-env-changed=SOURCE_DATE_EPOCH` | one build-dependency + its git backend (`gix` = a large tree; `vergen-gitcl` shells out to `git`) | Emits `cargo:warning` when a variable can't be produced, or `default_on_error` for an idempotent default — that is exactly the "never silently `unknown`" property we need |
| **`crate-git-revision` 0.0.9** | the minimal version of the same | tiny | Less surface than vergen if we only want the SHA |
| **`option_env!` + a stamp variable** (already used for `ALLSPARK_FORK_BUILD`) | CI-provided identity: `PA_BUILD_ID=<sha256 of binary or git sha>` injected at compile time | ~0; no build script; works in a tarball build | The right *primary* mechanism for us: it composes with the existing release stamping and needs no git at build time |
| **`SOURCE_DATE_EPOCH`** | reproducible timestamps so two builds of the same source produce the same bytes | free to set; only pays off if every other nondeterminism is gone (paths, `-C metadata`, ordering) | <https://reproducible-builds.org/docs/source-date-epoch/>; `vergen` already re-runs on changes to it |
| **ELF `NT_GNU_BUILD_ID`** (`-Wl,--build-id=sha1`, the default on most Linux distros) | a 160-bit hash of the normalized ELF contents, stored in `.note.gnu.build-id` | free (linker default here, see §6.1) | Readable in-process **without** a hash of self: read `/proc/self/exe`'s note section, or parse `argv[0]`. Also the key `debuginfod`/`perf`/`systemd-coredump` use. Caveat: it hashes *linker inputs*, so it is stable across rebuilds only if the inputs are |
| **`cargo-auditable` 0.7.7** | embeds the dependency list in a section (`auditable-serde`) | `cargo auditable build` instead of `cargo build`; the release job changes | Not identity; SBOM-adjacent. Worth adding later for vulnerability triage, not for this defect |
| **Artifact SHA-256** | byte identity of the *published tarball/binary* | already computed (`sha256sum`, `sha2` in `crates/pa-core/src/update/release.rs:164`) | This is the only identity a user can verify **without running the binary** |

**Recommendation (cheapest sufficient set).** Two fields. One is embedded, one is published — because a binary
cannot contain a hash of its own bytes (see §6.8):

1. **`PA_BUILD_SHA`** (embedded) — `git rev-parse HEAD`, `+dirty` when `git status --porcelain` is non-empty,
   optionally `+<short sha of the object tree>`. Stamped by the CI job as an env var *and* derivable by a local
   build through a 20-line `build.rs` fallback. `"unknown"` only when neither is available, and that value must
   be loud (a `cargo:warning`, and `--version` printing `build unknown`).
2. **`buildHash`** (embedded, on the wire) — a short digest *of the embedded fields*, i.e.
   `sha256(sourceSha || target || profile || features)[:16]`. Not a self-hash, but a value that differs whenever
   the thing that produced the bytes differs. This is what the handshake compares.
3. **`artifactSha`** (published, not embedded) — `sha256(binary)` / `sha256(tarball)`, which the release already
   computes. The manifest carries it beside `sourceSha`, so a verifier binds file → source without running
   anything.

Why not the ELF build id as the primary field: it exists on Linux/macOS (Mach-O UUID) but not on Windows (PE has
a timestamp + checksum, no content hash), it can be absent (`--build-id=none`), and a different linker flavour
changes it. `readelf` on this tree shows it works *here* (`247167fc…` vs `07f78b3c…` for two payloads) — useful
as a **cross-check in a test** (§6.7 B7), not as the shipped contract.

Why not `-C metadata` / rustc's crate disambiguator: it is per-crate, not per-artifact, and is not exposed as a
stable API (<https://internals.rust-lang.org/t/emitting-build-id-when-linking-elf-binaries/15349> is the relevant
discussion; the crate disambiguator is internal to rustc's `Fingerprint`).

### 6.4 Where each identity must appear

| Surface | Field | Consumer | Why there |
|---|---|---|---|
| `daemon_hello.runtime` | `buildHash` (new, optional) alongside `buildId` | every client on connect; `doctor`; `install-rust.sh`'s classifier | the handshake is the *only* moment the client learns what it is talking to |
| `--version` | `allspark <v> (fork build <N>; build <sha12>; upstream …)` | users, bug reports, `qa-tasks/RT-013/stamp` | today's line already carries three of the four; adding the hash costs one `format!` |
| `package.json` (staged layout) | `buildHash` | `packaged_layout_e2e`, the launcher, `config::version()`'s manifest read | the manifest is what a *staged* tree can be checked against without executing |
| `latest.json` / `SHA256SUMS` | `sha256` of the archive (already), **plus** `sourceSha` (so the file can be bound to its source) | `install-rust.sh`, `verify_release.py` | lets a user prove the installed binary is the published one: `sha256sum $(which prime-agent)` vs the manifest, and the manifest's `sourceSha` vs `--version` |
| `daemon_hello.runtime.executablePath` | (already) | `doctor`, the installer's `install_owns` | the path is what makes "stale" actionable ("you are running /old/path") |

### 6.5 The runtime rule: compare, refuse, or replace — never ride silently

The three postures, and what each costs the user:

| Posture | Behaviour | Verdict |
|---|---|---|
| **Ride silently** | today (`crates/pa-tui/src/daemon_client.rs:447-456`: name check + `version.min(…)`; no app-version or build comparison anywhere on the connect path) | **the incident** |
| **Refuse** | error: "the running daemon is build `abc123`, this client is `def456`; run `prime-agent shutdown --force`" | safe, but punishes a user mid-session |
| **Replace** | `ensure_daemon_running` already has the machinery: `shutdown_stale_daemon` (`crates/pa-cli/src/interactive_mode/daemon.rs:112`) refuses to replace a *busy* daemon and shuts down an idle one | **correct default** for a stale *idle* daemon; refuse when busy |

Note what already exists and only needs a predicate: `DaemonProbe::Stale` is plumbed, `shutdown_stale_daemon`
works, and the launch path calls it. The bug is entirely in **what counts as stale**:

```
crates/pa-cli/src/interactive_mode/daemon.rs:38-41   (launch path)
  current = hello.protocol.version == DAEMON_PROTOCOL_VERSION
         && hello.schemaId == DAEMON_SCHEMA_ID
                                    ← no appVersion, no buildHash

crates/pa-cli/src/daemon_discovery/mod.rs:416-425    (doctor only)
  current = protocol_version == … && schema_id == … && version == config::version()
                                    ← app version, but still no hash
```
Two classifiers, two answers, one live daemon: `doctor` said `stale`, the launch path said fine (§2.3). The
divergence is `file:line` `daemon.rs:31-48` vs `daemon_discovery/mod.rs:416-425`.

A third divergence sits on the *client* side: `DaemonClient::connect` (`crates/pa-tui/src/daemon_client.rs:333-460`)
checks the protocol **name** (`:447`) and clamps the envelope version (`:456`), and nothing else. Its own caller
`probe_daemon` then re-reads `hello` — so the connect path and the probe path each decide "current" separately,
and `agents_view` (`crates/pa-tui/src/agents_view.rs:326` `AgentsViewLink::connect`, reached from `:616`) connects through `DaemonClient::connect_with_retry` (`daemon_client.rs:497`) without either. That is three places a
daemon is accepted; one predicate has to replace all three, or the next daemon bug will be in the one that was
missed.

### 6.6 The build-hash handshake, concretely

1. **Stamp** (`build.rs` or `option_env!`), one accessor:
   ```rust
   // crates/pa-types/src/build_identity.rs  (a leaf crate: both pa-cli and pa-daemon may read it)
   pub fn build_hash() -> &'static str {
       option_env!("PA_BUILD_HASH").unwrap_or("unknown")
   }
   pub const fn build_sha() -> &'static str { option_env!("PA_BUILD_SHA").unwrap_or("unknown") }
   ```
   One place, three consumers (`--version`, hello, doctor), no `build.rs` required for the primary path.
2. **Publish** in `DaemonRuntimeIdentity` as `#[serde(default, skip_serializing_if = "Option::is_none")]
   build_hash: Option<String>` — optional on the wire, so an old daemon still parses (wire-compat is binding:
   the field must be additive; `DaemonHello` uses `#[serde(flatten)] rest: JsonMap` for unknown keys, and the new
   field must be `Option` so a *new* client can parse an *old* hello).
3. **Compare** in one shared predicate used by both classifiers:
   ```rust
   // one function, both call sites
   pub fn hello_is_current(hello: &Value) -> bool {
       protocol_ok(hello) && schema_ok(hello)
           && app_version_ok(hello)              // existing doctor rule
           && hello_build_hash(hello).is_none_or(|h| h == build_hash() || h == "unknown")
   }
   ```
   `is_none_or` on purpose: an **older daemon that predates the field** must not be rejected by a newer client
   purely for lacking it — that would break every upgrading user. The rule is: *if the daemon publishes a hash,
   it must match; if it does not, fall back to the app-version rule.* This keeps the change backward-compatible
   and still closes "same version, different code" for every build from this release forward.
4. **Act** on a mismatch exactly as today's `DaemonProbe::Stale` path: shut down an idle daemon, refuse a busy one
   with an actionable message, then spawn.

### 6.6a The blast radius of changing `buildId` (read before touching it)

`buildId` is not a private field. `install-rust.sh` classifies every daemon it might stop by a **prefix match** on
it: `RUST_BUILD_ID_PREFIX = "pa-daemon-rs-"` (`install-rust.sh:1985`), used at `:2135` and `:2211` to decide
"this is our product's daemon" before the install-root gate. Changing the *format* (e.g. dropping the prefix in
favour of a bare hash) turns that ladder into `unrecognized:<schema>` and the installer stops recognising its own
daemons — the exact class of the 2026-10-21 incident (see the `AGENTS.md` live-system-safety rules).

So the rule for this change: **append, never re-format.**

- `buildId` keeps its `pa-daemon-rs-<version>` shape (the prefix is a product classifier, not identity).
- The new field is *separate* (`runtime.buildHash`), optional, additive.
- Both classifiers in the binary, and the installer's prefix ladder, keep working unchanged.
- `doctor --json` already prints `buildId` (`crates/pa-cli/src/daemon_discovery/mod.rs:326,363-366`), so the new
  field rides the same struct (`DaemonInfo.build_id` at `:100`) with one more row; no format break.

Second cascade to decide deliberately: `crates/pa-tui/src/daemon_client.rs:456` clamps
`protocol.version.min(DAEMON_PROTOCOL_VERSION)`. That is *correct* for the envelope version (a shared-minimum
rule, and the TS parity contract) and must stay. It is also the line that a reader mistakes for a version check —
worth one comment saying "the envelope minimum; app/build identity is compared by the caller, see
`hello_is_current`", so the next person does not "fix" the wrong thing.

### 6.7 Tests for build identity (each must be red without the change)

| # | Test | Tier | File | Red-first check |
|---|---|---|---|---|
| B1 | `hello_is_current_rejects_a_different_build_hash` | unit | `crates/pa-cli/src/interactive_mode/daemon.rs` tests | build a hello JSON with this process's version and `buildHash: "deadbeefdeadbeef"` → `false`; with the process's own hash → `true` |
| B2 | `hello_without_a_build_hash_still_uses_the_version_rule` | unit | same | a pre-upgrade hello (`runtime` without `buildHash`, matching version) → `true`; the same with an older `appVersion` → `false` |
| B3 | `both_classifiers_agree_on_the_same_hello` | unit | `crates/pa-cli/src/daemon_discovery/mod.rs` tests | table of 6 hello shapes × `{probe_daemon, classify_reachable}` → same verdict. Today this table fails on the version-only column |
| B4 | `a_stale_build_of_the_same_version_is_replaced` | pty e2e | `crates/pa-cli/tests/stale_daemon_takeover_pty_e2e.rs` | a fake supervisor serving `appVersion == current`, `buildHash: "0000…"`; the TUI must shut it down and start its own. Today it rides it |
| B5 | `version_and_manifest_name_the_build_hash` | integration | `crates/pa-cli/tests/package_e2e.rs` or `packaged_layout_e2e.rs` | stage a layout with a `package.json` carrying a `buildHash`; `--version` must print it; strip the field → the test fails on the assertion (not on a panic) |
| B6 | `the_published_manifest_binds_the_artifact_to_its_source` | release-gate (python) | `scripts/release/test_catalog_assets.py`-style, or `verify_release.py` | run the packer on a tiny fixture tree; assert `manifest.sourceSha == the stamp baked into the binary` **and** `sha256(tarball) == manifest.sha256`; flip one byte of the stamp and the first assert must fail. Today neither field exists, so the test fails by absence, then passes once wired |
| B7 | `the_artifact_elf_build_id_is_stable_across_a_rebuild_of_the_same_tree` | nightly, non-gating | `scripts/` | documents reality: if two builds differ, the ELF build id is *not* reproducible and must not be the user-facing identity (measures the assumption instead of asserting it). Mark advisory |

### 6.8 Publishing an identity a user can check *without* running the binary

The chain that must hold, all of it cheap:

```
source  →  buildHash  (stamped INTO the binary: sourceSha + target + profile; printed by --version and hello)
        →  binary     →  sha256(tarball)  (computed at package time; published)
        →  latest.json { version, file, sha256, sourceSha }   ← sourceSha lets a verifier bind file → source
        →  SHA256SUMS  { <sha256> <file> }
        →  install-rust.sh cross-checks the two published reads (already: :1588-1593 "the channel is inconsistent")
```
The subtlety worth stating: **a binary cannot contain its own SHA-256** (the hash would have to be inside the
hashed bytes). Two workable resolutions:

- **(a) publish, don't embed**: `buildHash` in the manifest is the *artifact* hash; the binary embeds the *source*
  identity (`build_sha`). A verifier checks `sha256(file) == manifest.sha256` and `--version` prints
  `build_sha`, which the manifest also carries (`sourceSha`). Anyone can then prove "this file is the published
  artifact for commit X".
- **(b) embed a hash of the inputs**: build twice, or hash the object files/crate metadata — fragile and slow.
  Not recommended.

So the field set is: **`sourceSha` (embedded, printed by `--version`), `artifactSha` (published, in
`latest.json`/`SHA256SUMS`), `buildHash` on the wire = `sourceSha` + target + profile** (a short string derived
from things that are known *before* linking). That is what makes the handshake able to say "same version,
different code": the two builds differ in `sourceSha` or in the dirty flag even when `CARGO_PKG_VERSION` is
equal.

For provenance beyond identity, the 2026 standard step is **GitHub Actions artifact attestations**
(Sigstore-backed; <https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations/using-artifact-attestations-to-establish-provenance-for-builds>),
which is also the shortest path to SLSA Build L3 claims (<https://github.blog/enterprise-software/devsecops/enhance-build-security-and-reach-slsa-level-3-with-github-artifact-attestations/>).
Recommended as a follow-up, not as this fix: it answers "who built it", not "is the daemon I am talking to the
build I installed".

---

## 7. How the two threads meet: one test that would have failed on both causes

Both causes in the incident report are *data provenance*, and both are visible in exactly one place — the picker
after it settles:

```
cause 1 (catalog)        → the picker shows (1/196) rows, `fixture-*` for every upstream provider
cause 2 (stale daemon)   → the picker shows the OLD daemon's catalogue: (1/1356), GPT-5.4 current
cause 1+2 together       → the picker looks healthy and is wrong twice
```
So one assertion covers both, if it is written as *set equality against what the installed payload should
serve*:

```rust
// crates/pa-cli/tests/model_picker_catalog_pty_e2e.rs (T5)
let screen_rows = picker_rows(&capture_pane());       // (provider, id) parsed from the pane
let expected = expected_catalog_for_this_payload();   // resolved through pa_models::ModelCatalog
assert!(!expected.is_empty(), "the payload serves no models at all: {}", DIAGNOSTICS);
assert_eq!(screen_rows, expected,
    "the picker shows {} rows; the payload serves {}. First missing: {:?}. Daemon: {} ({})",
    screen_rows.len(), expected.len(), missing.first(), daemon_version, daemon_build);
```
The `expected` side is what makes this a *black-box* test rather than a tautology — it is derived from the
packaged asset and the fork table, not from the renderer. And the failure message names which of the two causes
it is, because it prints the daemon's version and build hash.

---

## 8. Anti-patterns this report argues against

1. **A screenshot diff as the data oracle.** It approves whatever the first run produced — including a picker of
   placeholder rows — and it is the most expensive assertion to maintain. Our own picker screenshot would have
   been approved at `(1/196) fixture-*` and stayed green through the whole incident.
2. **`for row in rows { assert!(…) }` as a data check.** Vacuous on zero rows (§2.4).
3. **Testing the picker with a hand-built `Vec<Model>`.** Proves the renderer only; the incident's cause lives
   upstream of the renderer (§3.1).
4. **`sleep`-based waits in a pty test.** Every surveyed 2026 harness refuses it; ours should too.
5. **Asserting on the first frame after `/model`.** Measured: the startup snapshot (`get_available`, auth-filtered)
   and the daemon's `catalog_payload` are different sources, and the picker folds whichever arrives (Probe C).
   Assert on the settled screen; never on "the frame right after the keypress".
6. **Adding `insta` broadly.** No `.snap` file exists in this workspace today; introducing a snapshot gate over
   the whole TUI would import the blind-approval problem wholesale. Use it for 3–5 frames, reviewed.
7. **Trusting a retry.** A retried pty test hides which of the two states it saw.
8. **A build id derived from the version.** That is what exists now (`clients.rs:126`) and it is why "same
   version, different code" is invisible.

---

## 9. Assumptions, and what I did not verify

**Assumptions I made (stated so they can be challenged):**
- The picker's `(1/196)` row is the count of *filtered* rows; my row-count assertions read the visible rows and
  the counter, not an internal field.
- "Settled" in my probes means "the picker is present and the count row is stable across two captures"; a
  wall-clock settle was used *only* to stop sampling. The recommended tests must use `wait_frame`/`wait_stable`
  (termlens) or wait-on-needle (tmux), never a sleep.
- The picker's `(1/N)` count is the filtered catalogue size; the fixture-asset case reports 196 while the asset
  has 46 models because the resolved list also carries the fork table (`models_fork.rs`, 40 `id:` literals) and
  the 110 compiled Prime-Inference offline entries (`crates/pa-models/src/bundled.rs:104-121`). My assertions use
  that resolved number, not the asset's own length — a test comparing to `json.load(asset)["models"]` would be
  wrong by 150. The composition adds up (46 + 110 + 40 for the fixture asset; 42 + 110 + 40 for the compiled
  table), but the *count on screen* is the measured fact and is what a test must assert — deriving it from source
  counts is a maintenance trap (this note got 192→"42 unique" wrong on the first attempt).
- The fixture catalogue the release ships is byte-identical to `generate --fixture` output for this tree:
  `diff -q` between a fresh `--fixture` run and `/root/.local/libexec/models.bundled.json` reports no difference
  (46 models, 43 transport tuples, every id `fixture-*`). Verified 2026-10-24.
- I treat `--fixture` at `.github/workflows/release-fork.yml:127` as the release's actual catalog source; the
  workflow file at HEAD does that, and a working-tree edit (uncommitted) adds the fork-provider gate to
  `bundle_catalog.py` but does not change the catalog's contents.

**UNVERIFIED (I did not check these):**
- Codex's `PtyCodex`/`terminal.wait_for_screen()` and `TEST_MODEL_PRESETS` file paths came from DeepWiki answers,
  not from a clone. The *pattern* is what I cite; the paths are indicative.
- `termlens`' Windows/ConPTY limitations are its own README's claim; I did not run it.
- Microsoft `tui-test`'s behaviour under a CI runner without a writable state dir — not exercised.
- Whether `vergen-gix`'s `gix` dependency passes this workspace's `make deny` (license/advisory) — not run; it
  is the one cost item to check before adopting vergen over a 20-line `build.rs`.
- The exact `--version` plumbing for a new field: I found the formatter at `crates/pa-cli/src/lib.rs:238-244` and
  the `splash_version()` at `crates/pa-cli/src/config.rs:52-55`, but did not trace every caller (the TUI footer,
  the ACP handshake, `qa-tasks/RT-013`'s regex) to confirm a fourth field would not break one of them.
  `RT-013`'s regex (`r"fork build fork\.(\d+)"`) would still match, so the risk is cosmetic — **UNVERIFIED**.
- Whether the ELF `NT_GNU_BUILD_ID` is stable across two clean rebuilds of this exact tree: I only observed that
  the two *existing* payloads have different ids. §6.7 B7 turns this into a measurement rather than an assumption.

**Reproduction hygiene:** every probe used `env -i`, a `/tmp/<name>` `HOME`/`TMPDIR`, a private daemon socket,
and `--offline`. Processes were reaped by pid only, after checking `/proc/<pid>/exe` was one of the three
payloads I started and `/proc/<pid>/cwd` was one of my scratch dirs — 56 in the first sweep, 13 in the second
(all `tmux`-hosted), plus one `--mode daemon` I started on `/tmp/stale-repro/daemon.sock`. The ambient daemon
(`pid 3289830`, `/tmp/allspark-daemon.sock`) was verified alive before and after, and never signalled. No
`pkill -f prime-agent`/`killall` was used; sockets and scratch dirs were removed as `/tmp` paths I created.

---

## 10. Sources

**TUI tooling (fetched 2026-10-24):**
- `ratatui` testing recipes — <https://ratatui.rs/recipes/testing/snapshots/> (the `TestBackend` +
  `insta` recipe, the explicit "does not cover your event loop, key handling, terminal setup and teardown or
  exit codes", and the `termlens` PTY example).
- `termlens` 0.11.6 — <https://crates.io/crates/termlens>, <https://github.com/vyncint/termlens>
  (`wait_frame`/`wait_stable`/`snapshot_after`, `unsupported()`, `TERMLENS_ARTIFACT_DIR`, the
  known-unknowns table, the `TestBackend`-vs-PTY comparison).
- Microsoft `tui-test` — <https://github.com/microsoft/tui-test>, `tui-test-rs` 0.1.1
  (<https://crates.io/crates/tui-test-rs>); README's locator options, `wait idle`/`expect snapshot`,
  `--failure-artifacts`, `[trace]`.
- `ratatui-testlib` 0.1.0 — <https://crates.io/crates/ratatui-testlib>,
  <https://github.com/raibid-labs/ratatui-testlib> ("PTY-based testing", `snapshot-insta`, Linux focus).
- `termless` (Playwright-for-TTY, multi-backend conformance) — <https://github.com/beorn/termless>.
- `expectrl` 0.9.0 — <https://crates.io/crates/expectrl>; `rexpect` 0.7.1; `vt100` 0.18.1
  (<https://github.com/doy/vt100-rust>); `alacritty_terminal` 0.26.0.
- asciicast v3 (interval timing, NDJSON) — <https://docs.asciinema.org/manual/asciicast/v3/>.
- `insta` CI behaviour (refuses to write new snapshots under `CI=true`) — <https://insta.rs/docs/quickstart/>.
- Snapshot-testing critique and 2026 practice — <https://kentcdodds.com/blog/effective-snapshot-testing>,
  <https://percy.io/blog/snapshot-testing>.
- Playwright locator semantics (a count of 0 is not an assertion) — <https://playwright.dev/docs/api/class-locator>.

**Build identity:**
- `vergen` 10.0.3 / `vergen-gix` — <https://crates.io/crates/vergen>, <https://github.com/rustyhorde/vergen>
  (`cargo:rustc-env`, `rerun-if-changed=.git/HEAD`, `rerun-if-env-changed=SOURCE_DATE_EPOCH`,
  `default_on_error`).
- `crate-git-revision` 0.0.9 — <https://crates.io/crates/crate-git-revision>; `cargo-auditable` 0.7.7 —
  <https://github.com/rust-secure-code/cargo-auditable>.
- The `option_env!` stamp idiom — <https://stackoverflow.com/questions/43753491/include-git-commit-hash-as-string-into-rust-program>.
- ELF build id semantics — <https://github.com/mattst88/build-id>, <https://interrupt.memfault.com/blog/gnu-build-id-for-firmware>,
  and the rustc discussion on emitting one — <https://internals.rust-lang.org/t/emitting-build-id-when-linking-elf-binaries/15349>.
- Reproducible builds / `SOURCE_DATE_EPOCH` — <https://reproducible-builds.org/docs/source-date-epoch/>.
- Immutable identity vs mutable tags (the digest analogy) — <https://docs.docker.com/dhi/explore/security-concepts/digests/>.
- Provenance — <https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations/using-artifact-attestations-to-establish-provenance-for-builds>,
  <https://github.blog/enterprise-software/devsecops/enhance-build-security-and-reach-slsa-level-3-with-github-artifact-attestations/>.

**This tree (all `file:line` verified today):** `crates/pa-models/src/chain.rs:165-190`,
`crates/pa-models/src/bundled.rs:84-121`, `crates/pa-models/src/pinning.rs:68-95` (`:85` silent drop),
`crates/pa-core/src/models/registry.rs:219-241` (`get_available`), `:588-604` (the fork chain at `:600`),
`crates/pa-cli/src/interactive_mode.rs:458-460`, `crates/pa-cli/src/interactive_mode/daemon.rs:31-48` /
`:112-160`, `crates/pa-cli/src/daemon_discovery/mod.rs:336-375` / `:416-425`,
`crates/pa-daemon/src/supervisor/clients.rs:118-140` (`build_id` at `:126`),
`crates/pa-daemon/src/model_catalog.rs:25-60` / `:104-140`, `crates/pa-types/src/daemon/mod.rs:17` (`schemaId`),
`crates/pa-types/src/daemon/outbound.rs:144-160` (`DaemonRuntimeIdentity`), `:252-290` (`DaemonHello`),
`crates/pa-tui/src/daemon_client.rs:333-460` (connect: name check `:447`, `version.min` `:456`),
`crates/pa-tui/src/model_picker/render.rs:98-100` (`"No matching models"`),
`crates/pa-tui/src/model_picker/tests.rs:579-586`, `crates/pa-tui/src/session_ui/model_picker.rs:13` / `:182-240`,
`crates/pa-tui/tests/model_picker_provider_headless.rs:35-52` (hand-built catalog),
`crates/pa-cli/src/config.rs:52-72`, `crates/pa-cli/src/lib.rs:238-244`, `crates/pa-core/src/update/installer.rs:284,760-765`,
`crates/pa-perf/src/bench/pty.rs:15-19,28-107,214-260,257`, `crates/pa-cli/tests/login_panel_pty_e2e.rs:462-505`,
`crates/pa-cli/tests/chat_open_first_frame_pty_e2e.rs:61,72`, `.github/workflows/release-fork.yml:117-230`,
`scripts/release/bundle_catalog.py:113-117` (`FORK_PROVIDERS`), `:122-210` (the model gate incl. `:202-210`), `:410-422`, `:424-530` (the fixture), `:842` (`allow_small`), `install-rust.sh:1506-1600` (sha256 verification), `:1973-2000` (`RUST_BUILD_ID_PREFIX`/`TS_SCHEMA_FAMILY`), `:2118-2141` (`install_owns`, `classify_hello`),
`crates/pa-core/src/settings/types.rs:632-633`, `qa-tasks/RT-013-version-stamp-check/oracle.py`.
