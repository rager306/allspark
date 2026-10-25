# Release artifact contract testing: how 2026 proves "what we built is what the user runs"

Date: 2026-10-24. Scope: read-only research + a concrete plan for the allspark fork of
prime-agent (Rust). Nothing here was applied to product code; every measurement below was
taken on this machine, from the tree at `01de64db2` plus the published artifacts.

Incident that motivated this: after an update, the user's picker showed providers with no
models (and 429s from z.ai on any model). Two independent causes, kept separate throughout:

- the 429 is a real z.ai billing zero (proved by the parent session with direct curl);
  it is **not** a code defect and is not analysed here;
- "providers exist, models do not" is a packaging defect: the release shipped a
  **synthetic fixture catalog** as the release asset (`models.bundled.json`), and that
  file *replaces* the compiled catalog at runtime (`crates/pa-models/src/chain.rs:177-186`).

---

## 0. What this report adds (and what it deliberately does not repeat)

Already covered elsewhere in `.research` — read and not restated here:

| File | What it already owns |
|---|---|
| `blackbox-qa-best-practices.md` | eval methodology (pass@k vs pass^k), task-level benchmarks, trajectory graders, real-model QA |
| `fork-testing-research.md` | Codex/oh-my-pi/grok-build *in-process* harnesses: wire-level mock providers, context snapshots, MCP conformance, PTY harness, snapshot tests |
| `TEST-MATRIX.md` | the tier matrix (unit → integration → provider wire → faux → mock-provider → real-model → daemon) for allspark features |
| `oh-my-pi.md`, `oh-my-pi-deep.md` | oh-my-pi internals (advisor/watchdog, LSP, compaction, snapcompact); no release/packaging content at all (keyword sweep: 0 hits for "release") |
| `PACKAGING-FORK-009-REVIEW.md` | the fork.009 *self-sufficiency* defect (missing runtime + skills) and its fix |

New here, and the reason this file exists:

1. **The artifact as the unit under test.** Every prior report tests code or a staged
   layout from the source tree. None tests *the tarball a user downloads*, and none
   asserts **data provenance** inside it. That gap is exactly what shipped `fixture-*`
   models to users.
2. **Measured state of the published artifacts** (fork.010, upstream v0.10.1, upstream
   nightly beta.45): all three ship a synthetic catalog — upstream included. Concrete
   counts and reproduction commands below.
3. **A working prototype gate** (`.tmp/artifact-gate.py`, 1.1 s, ~30 MB) that fails
   fork.010 for the right reason and passes a repaired artifact. Its design constraints
   (stub credentials are mandatory; `--version` alone cannot catch this) are measured.
4. **Build identity**: the parent's follow-up (version is not enough) measured against the
   tree — `buildId` is literally the version, there is no `build.rs`, `appVersion` is
   overridable by editing a JSON file beside the binary, and two *different* binaries on
   this box report identical `appVersion` + `schemaId` + `buildId`. Plus a measured
   `build.rs` stamping gotcha (`.git/HEAD` is not enough).
5. **Post-publish verification without moving a tag**, including proof that the current
   fork release has *no* attestation, is *not* immutable, and is missing two of the four
   channel assets its own launcher promises.

---

## 1. Release-artifact verification, 2026 state of the art

### 1.1 What provenance does and does not buy (the honest version)

SLSA's Build track predicate (`https://slsa.dev/provenance/v1`,
https://slsa.dev/spec/v1.2/build-provenance) binds an artifact to a builder, a source
repository, and a commit. SLSA v1.2's own text is explicit about the consumer's job:
"Consumers of the provenance can verify that the artifact was built according to
expectations. Others can rebuild the artifact, if desired."

Two 2026 incidents define the boundary (both documented at
https://arbure.com/slsa-provenance-hard-gate.html):

- **Mastra, 2026-06-17**: 143 poisoned `@mastra/*` versions carried **no** provenance
  while the legitimate releases did. A policy that *requires* an attestation kills this
  class. Enforcement, not generation, is the missing control.
- **Red Hat / Miasma, 2026-06-01**: 96 malicious versions carried **valid** SLSA
  provenance, because the attacker controlled the trusted-publisher pipeline. All true,
  all signed, all malware.

Conclusion for us: provenance answers "did our pipeline build this?", never "does this
artifact contain the right data?". A fixture catalog inside a correctly-built,
correctly-signed tarball has flawless provenance and is still broken. **The defect class in
this incident is a content-contract defect, not a supply-chain-integrity defect** — which
is why no amount of signing would have caught it, and why the gate has to assert contents.

### 1.2 What the ecosystem actually automates today

| Project / spec | What it verifies about the *artifact* | URL |
|---|---|---|
| GitHub **immutable releases** (GA 2025-10-28) | tags locked to a commit, assets immutable after publish; publishing an immutable release auto-generates a **release attestation** (tag + commit SHA + asset digests) | https://github.blog/changelog/2025-10-28-immutable-releases-are-now-generally-available/ , https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases |
| `gh release verify <tag>` | the release carries a valid GitHub-signed attestation covering its assets; prints every asset digest | https://cli.github.com/manual/gh_release_verify |
| `gh attestation verify <file> -R owner/repo` | verifies one downloaded file against build provenance, enforcing signer identity/ref | https://cli.github.com/manual/gh_attestation_verify |
| GoReleaser **`verify`** (v2.17+) | **re-downloads the published assets** and runs arbitrary commands against them, with retries for CDN propagation; skipped on `--snapshot` because nothing was published | https://goreleaser.com/customization/verify/ |
| cargo-dist | generates plan/build/host/publish/announce jobs; **no install-and-smoke job by default** (verified via DeepWiki on `axodotdev/cargo-dist`); supports `github_attestations` via `actions/attest` in the build/host phases | https://github.com/axodotdev/cargo-dist/blob/main/cargo-dist/templates/ci/github/release.yml.j2 |
| Homebrew `brew test-bot` | installs the built bottle and runs `brew test`, plus affected dependents | https://docs.brew.sh/BrewTestBot-For-Maintainers |
| Debian `autopkgtest` / DEP-8 | "as-installed" tests: the package under test is *installed* in a testbed, not run from the build tree | https://dep-team.pages.debian.net/deps/dep8/ |
| npm `--provenance` / PEP 740 on PyPI | signed attestations attached at publish; consumable via policy | https://docs.pypi.org/attestations/ , https://blog.pypi.org/posts/2024-11-14/pypi-now-supports-digital-attestations/ |
| cosign `verify-blob --certificate-identity…` | keyless signature verification of the exact bytes, bound to a workflow identity | https://docs.sigstore.dev/cosign/verifying/verify/ |

The pattern that matters for us: **the projects with a real "install-and-smoke" step put
it after packaging and run it against the packaged layout, with the host environment
deliberately crippled.**

### 1.3 Two reference implementations worth copying verbatim

**astral-sh/uv** — a smoke job that consumes only the built binary:

- `.github/workflows/test-smoke.yml`: downloads the workflow artifact
  (`uv-linux-libc-${{ inputs.sha }}`), `chmod +x`, then runs the smoke script; one job per
  platform, including a musl run inside `alpine:latest` pinned by digest.
- `scripts/smoke-test/__main__.py` executes `scripts/smoke-test/commands.sh` **serially
  through Python** (cross-platform, no shell), prepending the artifact's directory to
  `PATH` so every command resolves to the built binary, not the developer's.

That is the cheapest possible shape of "install as the user does, run real commands,
assert on data".

**openai/codex** — `scripts/codex_package/smoke_tests/test_codex_package.py` (pytest)
plus `.github/actions/build-codex-packages/action.yml`:

- extracts each assembled archive and runs the *packaged* CLI (`--help`, `features list`,
  `completion bash`, `--version` compared against `codex-rs/Cargo.toml`);
- asserts packaged payload contents (vendored `rg`, `bwrap`, sandbox helpers);
- and the line we should copy outright (action.yml:171-201, the `Smoke-test assembled packages` step): **if a host `rg` or `bwrap`
  is on `PATH`, the job fails** — "CI must exercise packaged tools; local smoke runs may
  use host tools." That is the general form of our bug: a test that can be satisfied by
  the *environment* rather than by the *artifact* proves nothing about the artifact.

### 1.4 Reproducible builds: useful, but not the lever here

`SOURCE_DATE_EPOCH` (https://reproducible-builds.org/docs/source-date-epoch/) and
bit-for-bit rebuilds prove the artifact matches the source. They do not answer "is the
data inside right". Measured on this box: after `cargo clean`, a twice-built trivial Rust
crate produced an **identical** sha256 and an identical GNU build-id
(`dd717ae9ae206415e3c679f494de74edb37a31c3`), so reproducibility is achievable for small
inputs — but a 69 MB binary with a network-fetched asset is not reproducible by
construction, because the asset differs per build. **Reproducibility is the wrong tool for
network-fetched payload data; a provenance marker plus content assertions is the right
one.** (Kept as a P3, not part of the gate.)

---

## 2. Our defect class, measured

### 2.1 The mechanism, in three lines of code

- `crates/pa-models/src/chain.rs:177-186` — step 2 of the no-cold-start chain:
  `Some(bundled) => bundled`. The bundled asset **replaces** the compiled catalog.
- `crates/pa-models/src/pinning.rs:84-88` — every catalog entry whose
  `(provider, api, baseUrl)` is not a compiled tuple is `continue`d, "drop silently".
  So a synthetic catalog is not even an error: it degrades to almost nothing.
- `crates/pa-core/src/models/registry.rs:600` —
  `.chain(pa_ai::models_fork::fork_provider_models())` is the *only* reason fork providers
  survive a bad asset.

### 2.2 Where the fixture enters the release

`.github/workflows/release-fork.yml:117-129`:

```yaml
- name: Generate bundled catalog assets (fixture, offline-safe)
  run: |
    python3 scripts/release/bundle_catalog.py generate --fixture --out target/catalog-assets
    python3 scripts/release/bundle_catalog.py verify --out target/catalog-assets
```

Upstream does the same in three places: `.github/workflows/release.yml:211`, `:352`,
`:468`. `Makefile:85` defaults `CATALOG_ASSETS_MODE ?= fixture`, so the local packaging
dry-run (`Makefile:116-160`) also produces fixtures by default.

`bundle_catalog.py:481-530` builds those entries as `id = f"fixture-{provider}-{index}"`,
`name = "Fixture {provider} model {index}"`, and `cost = 0`.

### 2.3 The artifact as published (measured)

Downloaded and inspected (commands reproduce exactly what was run):

```text
fork.010  allspark-v0.10.2-beta.3-fork.010-linux-x86_64.tar.gz (26,095,429 B)
  libexec/models.bundled.json      : 46 models, 46 synthetic (fixture-*), 32 providers
  libexec/mcp-services.bundled.json: 73 entries, 71 synthetic (fixture-svc-NN; the
                                    other 2 are the special-shape fixtures)
  libexec/package.json             : absent
upstream v0.10.1   prime-agent-0.10.1-linux-x64.tar.gz
  models.bundled.json              : 46 models, 46 synthetic
upstream nightly   beta.45 tarball
  models.bundled.json              : 46 models, 46 synthetic
```

The user-visible effect, measured by running the *published* payload with stub provider
keys (7 providers) and a dead HTTP proxy, clean `HOME`:

| installed catalog asset | models in `model list` | providers | `fixture-*` ids shown |
|---|---|---|---|
| none (compiled fallback) | 18 | 7 | 0 |
| **fixture asset (as released)** | **20** | **7** | **12** |
| real catalog (`catalog.v1.json`, 1209 models) | **173** | 7 | 0 |

With *no* credentials at all, all three cases show the same **2** free `opencode` models —
i.e. **a smoke test without stub credentials is blind to this defect**. That single
measurement decides the gate's design (§6.2): the gate must inject stub keys *and* scrub
the ambient ones, or it asserts nothing.

Same test against the published fork.010 payload, strict environment + 7 stub keys:
**35 models in 8 providers, 12 of them `fixture-*`**
(`anthropic 2, openai 2, zai 1, muse 5, codebuddy 15, ninerouter 1, opencode 6,
opencode-go 3`) — i.e. the whole non-fork part of the picker is synthetic. With the
ambient environment left in place (extra tokens from the shell) the same run shows 39
models / 10 providers / 16 fixture ids, which is why the gate must scrub the environment
the way `scripts/fork/prepush-gate.sh` already does. Upstream 0.10.1 with a clean `HOME`
and no network: **4 models, all four `fixture-*`**.

`model list` itself never fetches (no cache file is written by it; a dead proxy changes
nothing), so the asset is the only source — good news for the gate: it is deterministic
and offline.

Also measured (and relevant to §7): the installed payload
(`/root/.local/libexec/prime-agent`, sha256 `116d1346…`, 69,587,976 B) and the published
fork.010 payload (`b65b5c17…`, 69,587,976 B — same size, different bytes) are **two
different builds of the same version**. Both real `daemon_hello` frames report
`appVersion 0.10.2-beta.3`, `schemaId protocol-7-schema-30-8e4b17c2a9f5`,
`schemaRevision 30`, `runtime.buildId pa-daemon-rs-0.10.2-beta.3`. Their `--version`
lines differ only in the fork stamp (`fork build dev` vs `fork build fork.010`), which two
release builds of the same counter would share. **No version-shaped check separates two
rebuilds of one tag.**

### 2.4 Why nothing caught it

`release-fork.yml:210-230` is the fork's entire artifact gate:

```sh
./unpack/"${root}"/bin/allspark --version
test -f "unpack/${root}/libexec/models.bundled.json"
test -f "unpack/${root}/libexec/mcp-services.bundled.json"
test -f "unpack/${root}/libexec/prime-agent-runtime/pyproject.toml"
test -d "unpack/${root}/libexec/skills"
test -n "$(ls -A "unpack/${root}/libexec/skills")"
```

Presence and a version banner. Not one byte of content. `scripts/release/verify_release.py`
— the script that *does* validate the assets (`:184`, `validate_bundled_catalog_dir`) and
compare the staged binary's version (`:198-227`) — is called by `continuous.yml:211/:303`
and `release.yml`, **never by `release-fork.yml`** (and its `EXPECTED_TOP_LEVEL` at `:49`
does not describe the fork layout anyway).

The bundled-catalog test battery `scripts/release/test_catalog_assets.py` is not wired
into any workflow and is not in `scripts/fork/prepush-gate.sh` (grep: no `catalog` match).
Its own docstring says the fixture "must produce assets that pass the FULL validation
gates" (`:4-5`) — i.e. the suite is *designed* to bless the fixture.

---

## 3. How 2026 catches "synthetic data became the release asset"

Three mechanisms, all cheap, none present in our tree:

**(a) A provenance marker the artifact must carry, and the gate must read.**
The catalog JSON is an envelope we control (`{"schemaVersion":1,"models":[…]}`). Give the
generator one extra top-level key, e.g. `"provenance": {"source":"network"|"catalog-dir"|
"fixture","url":…,"fetchedAt":…,"commit":…}`, and have `validate_bundled_model_catalog`
**reject `source=="fixture"` in release mode**. I verified the runtime tolerates extra
top-level keys (`crates/pa-models/src/schema.rs:97-100`: "unknown top-level keys are not
rejected (TS parity)"), so this is wire-invisible and parity-safe. This is the marker
that makes "fixture can never be a release asset" a *typed* property instead of a
convention.

**(b) Content assertions on the artifact: fixed provider floors, no synthetic ids.**
The gate asserts properties of the *data the binary will load*: no `fixture-*` id, every
fork provider present, per-provider model counts above a floor, and the same floors after
the binary has actually parsed it. Codex's rule generalizes: assert against the packaged
data, in a process the host environment cannot satisfy.

**(c) Two artifact classes, never one path.** The fixture exists to make *offline builds*
work; it must therefore never travel down the release path. Two acceptable shapes:

- release job uses `--network` (or `--catalog-dir` with a pinned catalog checkout commit)
  and *fails* if the fetch fails, while tests keep `--fixture`; or
- the fixture is renamed to a distinct asset type (e.g. `models.smoke.json`) that the
  runtime never reads, gated by a unit test asserting `PACKAGED_MODEL_CATALOG_FILE` is
  produced only by the real modes.

I recommend the first: it is one argument in one workflow, and it also removes the
"release depends on no network" excuse for shipping placeholder data.

### 3.1 The in-progress fix in this tree is currently self-defeating — fix this first

The working tree adds a fork-provider gate (`bundle_catalog.py:117` `FORK_PROVIDERS`,
`:202-215` the rejection) but does **not** add the fork providers to
`FIXTURE_TRANSPORT_TUPLES` (`:434-470`) or to `fixture_models()` (`:498`). Measured:

```text
python3 scripts/release/bundle_catalog.py generate --fixture ... ; verify  -> rc 1
  error: Bundled model catalog is missing the fork's providers
         ['codebuddy','genspark','muse','ninerouter','subquadratic']
python3 scripts/release/bundle_catalog.py generate --network ...           -> rc 1 (same)
python3 scripts/release/test_catalog_assets.py                             -> 24 tests, 10 failures
```

So as the tree stands, both release paths fail and ten catalog tests are red. The gate is
right in principle and wrong in placement: applied to *every* mode, it makes the fixture
illegitimate and therefore blocks the very workflow that ships fixtures. The fix is the
one §3(c) prescribes — the release must not use `--fixture` at all — plus optionally
adding the fork tuples to the fixture so tests exercise them.

---

## 4. Verifying an already-published release **without moving the tag**

This is the part our constraints make mandatory (AGENTS.md: "never move or force-push a
release tag"; "a push is a publication, not a probe"). Verification must therefore be
**read-only against the published bytes**, and repair must be **roll-forward**.

Ordered by cost:

1. **Digest check against an independent copy** (seconds).
   `sha256sum <archive>` vs the release's `SHA256SUMS` *and* vs the channel manifest row
   the installer reads (`install-rust.sh:1533-1543`). Measured on fork.010: archive
   `87462f0f7d1c7198…`, matching both SHA256SUMS and the GitHub asset `digest`. This
   proves transport integrity only — the OVHcloud CLI incident (2026-10-19) replaced
   *both* the binaries and `checksums.txt` in place without moving the tag
   (https://www.stepsecurity.io/blog/ovhcloud-cli-github-release-swap-glassworm), which is
   why step 2 exists.
2. **`gh release verify <tag>` / `gh attestation verify <file> -R owner/repo`** (seconds).
   Measured on fork.010: **both fail** — `no attestations for tag …`, and the attestations
   API returns 404. So the fork release currently has no provenance record at all.
3. **Download-and-run the published artifact** (our prototype: 1.1 s). Extract into a
   scratch prefix with a clean `HOME`, run the launcher and the real commands, assert
   content. Nothing about this touches the tag; it reads the release like a user does.
4. **Self-verification inside the installed product** (`allspark doctor --json`,
   `allspark --version`, and a future `--build-id`/`--self-test`). This is the only check
   that also helps *after* install, and it is where §7's build identity pays off.
5. **Roll-forward**: publish `fork.NNN+1` with the fixed asset; delete nothing, move
   nothing. Publishing a new release is a `workflow_dispatch` on an existing tag *only* if
   the assets of that tag are what changed — which is precisely what the fork workflow's
   dispatch path does, and precisely why immutable releases (below) must be a *deliberate*
   choice, not a default.

### 4.1 What fork.010 looks like under those checks (measured, 2026-10-24)

```text
sha256sum -c                 : OK (archive == SHA256SUMS)
gh release verify            : FAIL (no attestations)
gh attestation verify        : FAIL (404)
immutable-releases (repo)    : {"enabled": false}
channel assets on the release: SHA256SUMS 200, stable 404, latest.json 404
```

Two findings beyond the catalog:

- **`stable` and `latest.json` are missing from the published release.** The launcher
  exports `PRIME_AGENT_DOWNLOAD_BASE_URL=https://github.com/rager306/allspark/releases/latest/download`
  (`scripts/allspark-launcher.sh:68`), and `install-rust.sh:1521-1543` requires both files.
  So `allspark update` cannot work against this release while the code claims it does —
  a "documented, never present" break of exactly the shape AGENTS.md item 8 hunts. Cause
  is visible in the last two workflow runs: `publish the GitHub release` fails with
  `no matches found for dist/stable` (the in-tree fix adding `stable`/`latest.json` to the
  artifact upload path addresses it).
- **The assets predate the commit the tag now names.** Measured via the API:

  ```text
  gh api repos/rager306/allspark/git/ref/tags/allspark-v0.10.2-beta.3-fork.010
      -> {"type":"commit","sha":"01de64db2"}      (commit date 2026-10-24T14:10:58Z)
  release assets created_at                       -> 2026-10-24T11:29:33Z/34Z
  run that uploaded them                          -> 40089364244, head 5e300a7bf (success 11:29)
  release created_at / published_at               -> 14:10:58Z / 09:55:58Z
  later release-fork runs for 01de64db2, ...      -> all "failure"
  ```

  So the published bytes were built from `5e300a7bf`, while the tag now resolves to a
  later commit whose release runs failed. How the tag reference reached `01de64db2` is
  **UNVERIFIED** — I did not find tag-ref logs, and the plausible explanations (delete and
  re-push; a dispatch that re-pointed it) both matter, because AGENTS.md forbids moving a
  release tag. The structural finding stands regardless: **nothing in the pipeline ties
  the published bytes to the commit the tag names**, so this was inferable only by
  comparing timestamps. A publish-time release attestation (tag + commit + asset digests)
  would have made it a two-second check.

### 4.2 Immutable releases: adopt deliberately

GitHub's GA (2025-10-28) locks tags and assets at publish and auto-generates a release
attestation (https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases).
For us the trade-off is concrete: immutability kills the `--clobber` republish path the
fork uses to repair a bad upload (`release-fork.yml:274-292`, the `--clobber` branch at `:283`), so enabling it means
"repair = new fork.NNN". That is the correct policy once the gate in §6 exists; enabling
it before the gate would leave no repair path at all. Sequence: gate first, then
immutability, then delete the `--clobber` branch.

---

## 5. "Version → affected surface": what a release touches, and what to re-check

Three cheap mappings, in order of cost:

1. **Path → gate set** (seconds). `dorny/paths-filter` is the common 2025-26 primitive
   (https://github.com/dorny/paths-filter).
   For us the mapping is already implicit in the tree: `scripts/release/**`,
   `install-rust.sh`, `.github/workflows/release-fork.yml`, `scripts/allspark-launcher.sh`,
   `crates/pa-models/src/bundled.rs`, `crates/pa-models/src/chain.rs`,
   `crates/pa-ai/src/models_{generated,fork}.rs` → **run the artifact gate** (§6).
   A change to `crates/pa-daemon/**` or `crates/pa-types/src/daemon/**` → **run the
   two-hello staleness test** (§7.5).
2. **Crate → dependents** (seconds). `cargo metadata` gives the reverse-dependency graph;
   a change in `pa-models` affects `pa-core` → `pa-cli` → the artifact. For Rust this is
   enough; general TIA tooling (Datadog TIA, `pytest-impacted`) buys little at our size.
   `scripts/ci_test_shard.py` already enumerates the 275 test units and could consume such
   a filter.
3. **Release → the assertion that would have failed.** Every release entry in
   `CHANGELOG-FORK.md` should name the *artifact-level* assertion it added or affected.
   fork.009's lesson was recorded as a report; fork.010's should be recorded as a gate.

---

## 6. The minimal release gate for us

Design constraints, all measured above:

- must run **offline** (`model list` with a dead proxy finishes in 0.03 s);
- must **inject stub credentials** for a fixed provider set *and* scrub the ambient ones
  (without stub keys the picker shows 2 models in every case — blind; with ambient keys
  left in, a local run disagrees with CI — measured: 39 models vs 35);
- must read the **published/unpacked artifact**, not the source tree (Codex's host-tool
  rule);
- must be **fast** enough to run in CI and locally (1.1 s measured, ~30 MB peak).

### 6.1 Gate A — content assertions (no build, no container)

New script `scripts/release/verify_artifact_contents.py`, called by
`release-fork.yml` right after `tar -czf` (before upload) **and** by a post-publish job
(§6.3). Checks:

```text
1. models.bundled.json exists; parses; schemaVersion == 1
2. NO model id matches ^fixture-                (the exact defect)
3. provenance.source ∈ {network, catalog-dir}   (the marker from §3(a))
4. fork providers present: muse, codebuddy, genspark, subquadratic, ninerouter, opencode
5. models count ≥ 200 and providers ≥ 25        (real catalog: 1209 / 31 measured)
6. mcp-services.bundled.json: no entry.server starting with fixture-
7. package.json present beside the payload: version == the tag's version, plus
   commit and contentId (NEW for the fork — measured, the fork tarball ships no
   package.json, so config::version()'s packaged-manifest preference
   (crates/pa-cli/src/config.rs:67-92) never fires and the version comes from the
   compiled CARGO_PKG_VERSION; the version is therefore not bound to the artifact)
8. every file the launcher/payload resolves exists and is non-empty
   (bin/allspark, libexec/prime-agent, prime-agent-runtime/pyproject.toml, skills/*)
9. no development litter (test/, .venv, __pycache__, *.pyc) anywhere in the tarball
```

Numbers 2-5 are the ones that would have blocked fork.010. Number 8 generalizes
fork.009's defect.

### 6.2 Gate B — install-and-smoke against the unpacked artifact

Prototype (written for this report, `.tmp/artifact-gate.py`, ~90 lines) does:

```text
extract -> run bin/allspark --version
        -> assert "allspark <tag version>" and "fork build <fork.NNN>"
        -> read libexec/models.bundled.json (capture/provenance/fork assertions)
        -> run bin/allspark model list with stub keys + dead HTTPS proxy
        -> assert per-provider floors and zero fixture-* rows
```

Measured behaviour:

| artifact | verdict | wall clock |
|---|---|---|
| published fork.010 | **FAIL**: `46/46 model ids are synthetic` | 1.1 s |
| + fixture catalog with fork providers added | **FAIL**: `46/51 model ids are synthetic` | 1.1 s |
| real catalog, fork providers stripped | **FAIL**: `catalog is missing the fork's providers` | 1.5 s |
| real catalog + fork providers, no provenance marker | **FAIL**: `catalog provenance is absent` | 1.3 s |
| real catalog + fork rows + provenance marker | **OK**: 1214 models, 36 providers, picker `{anthropic:16, openai:45, zai:7, muse:5, opencode:71, …}` | 1.1 s |

Note the second row: adding the fork providers to the fixture (the tempting "make the gate
pass" move) still fails, because the synthetic-id assertion is independent. That is the
property the gate needs: **the content assertion, not the provider list, is the load-bearing
one.**

### 6.3 Gate C — post-publish verification (read-only, no tag move)

A `verify-published` job in `release-fork.yml`, triggered by `workflow_dispatch` *and*
chained after `publish`:

```text
gh release download <tag> --pattern '*.tar.gz' -D dist/published
curl -fsSL <release>/SHA256SUMS   &&  sha256sum -c
python3 scripts/release/verify_artifact_contents.py dist/published/*.tar.gz
python3 scripts/release/verify_published_smoke.py dist/published/*.tar.gz <version> <fork.NNN>
# channel contract the launcher actually reads:
curl -fsSL <release base>/stable      # must equal <version>
curl -fsSL <release base>/latest.json # must carry the row for this platform + sha256
gh release verify <tag>               # once attestations are enabled
```

The `stable`/`latest.json` checks are not theoretical: both 404 today (§4.1), while the
launcher and installer require them.

### 6.4 Locally, before the push (the existing rule, applied to artifacts)

AGENTS.md: "Never ask CI a question you can answer locally." The artifact gate is 1.1 s,
so it belongs in the pre-push path:

```bash
make release-dry-run            # already exists (Makefile:116) — but make it use --network
python3 scripts/release/verify_artifact_contents.py target/release/dist/*.tar.gz
python3 scripts/release/verify_artifact_smoke.py    target/release/dist/*.tar.gz <version> fork.NNN
```

Two required edits to the existing target: `CATALOG_ASSETS_MODE` must default to
`network` for the release path (or the target must refuse `fixture`), and
`scripts/release/test_catalog_assets.py` must be added to `scripts/fork/prepush-gate.sh`
(it is currently in no workflow and not in the gate) — **after** §3.1's contradiction is
resolved.

### 6.5 Cost, and why this is not "another CI lane"

| step | measured |
|---|---|
| catalog asset generation (`--network`, cold) | ~1 s |
| content assertions (pure Python, 1 tarball) | < 0.1 s |
| install-and-smoke (extract + 2 subprocess runs) | 1.1 s, ~30 MB |
| full artifact gate | < 2 s |

That is one to two seconds on a release that already spends ~9 minutes in `cargo build`.
There is no cost argument against it.

---

## 7. Build identity: version is not enough (the parent's follow-up)

### 7.1 Measured state of the tree

| Claim | Evidence |
|---|---|
| `buildId` is derived from the version | `crates/pa-daemon/src/supervisor/clients.rs:126`: `build_id: concat!("pa-daemon-rs-", env!("CARGO_PKG_VERSION"))` |
| no build script anywhere | `find crates -name build.rs` → 0 results |
| no VCS/git stamp anywhere | `grep -rn 'GIT_HASH\|git_sha\|commit_hash\|SOURCE_DATE' crates/` → 0 results |
| the schema id is a hand-written literal, identical across builds | `crates/pa-types/src/daemon/mod.rs:13-17` (`DAEMON_PROTOCOL_VERSION`, `DAEMON_SCHEMA_REVISION`, `DAEMON_SCHEMA_ID = "protocol-7-schema-30-8e4b17c2a9f5"`) |
| the only content stamp is the fork counter | `crates/pa-cli/src/config.rs:61` `option_env!("ALLSPARK_FORK_BUILD")` |
| `appVersion` is a **file**, not a build property | `crates/pa-cli/src/config.rs:67-92`: `version()` prefers `package.json` beside the executable. Measured: copying the payload and writing `{"version":"9.9.9-probe"}` makes `--version` print `allspark 9.9.9-probe (fork build fork.010; …)` |
| the daemon reports that same overridable string | `crates/pa-cli/src/lib.rs:150` `configure_app_version(crate::config::version())` → `clients.rs:123` `app_version: Some(app_version())` |

Two live proofs that the identity fields do not separate builds:

```text
A) same host, two different binaries, identical identity fields
   installed  /root/.local/libexec/prime-agent   sha256 116d1346…  GNU build-id 247167fc…
   published  fork.010 libexec/prime-agent       sha256 b65b5c17…  GNU build-id 429892b7…
   both real hellos: appVersion 0.10.2-beta.3, schemaId protocol-7-schema-30-8e4b17c2a9f5,
                     buildId pa-daemon-rs-0.10.2-beta.3
   -> the only field that differs anywhere is the executable path, and only
      because the two files sit in different directories

B) a fixture daemon claiming the *current* version but a different executable
   doctor --json -> {"version":"0.10.2-beta.3","buildId":"pa-daemon-rs-0.10.2-beta.3",
                     "executablePath":"/root/.local/libexec.manual-old/prime-agent",
                     "status":"current"}
```

(B is a Python fixture supervisor at the resolved socket; `doctor` reads
`appVersion`/`schemaId` and calls it `current` even though `executablePath` names a
different binary. `classify_reachable` at `crates/pa-cli/src/daemon_discovery/mod.rs:416-425`
compares exactly protocol + schema + `config::version()`.)

So the parent's statement is confirmed and refined: when the same version is rebuilt —
patch, cherry-pick, feature flag — **nothing** distinguishes the two processes, and when a
`package.json` sits beside the binary even the version can be made to lie.

### 7.2 The two classifiers, and the launch path

- `crates/pa-cli/src/daemon_discovery/mod.rs:416` `classify_reachable`: protocol + schema +
  version → `DaemonStatus::{Current,Stale}`. Consumed only by `doctor`
  (`crates/pa-cli/src/public_command.rs:457-474`).
- `crates/pa-cli/src/interactive_mode/daemon.rs:31-56` `probe_daemon`: **protocol +
  schemaId only**. This is `ensure_daemon_running`'s classifier (`:58-72`), i.e. every TUI
  launch. A daemon of a different build is `DaemonProbe::Current` here and the user rides
  it — exactly the reported incident.
- `crates/pa-tui/src/daemon_client.rs:445-456` `DaemonClient::connect` checks only
  `protocol.name`, then takes `version.min(DAEMON_PROTOCOL_VERSION)`; no app/build
  comparison anywhere.

The fix is small and has one shape: **one predicate, two callers.**

```rust
// crates/pa-cli/src/daemon_discovery/mod.rs (new, private)
pub(crate) enum BuildMatch { Current, Stale(&'static str) }
pub(crate) fn classify_hello(hello: &serde_json::Value) -> BuildMatch { /* protocol, schema,
    app_version == config::version(), build_id == config::build_id() */ }
```

`classify_reachable` and `probe_daemon` both call it; the second stops being a second
opinion. `doctor` keeps its richer JSON.

### 7.3 What a build identity should be (2026 practice)

Three layers, each answering a different question; do not collapse them:

1. **Source identity (provenance):** `commit` + `dirty` + fork counter. In Rust the
   idiomatic mechanism is a `build.rs` emitting `cargo:rustc-env=` (documented at
   https://doc.rust-lang.org/cargo/reference/build-scripts.html; `vergen-gitcl` wraps the
   same calls, https://docs.rs/vergen-gitcl), consumed with `option_env!`. We already have
   the pattern for the fork counter — extend it, do not add a second mechanism:

   ```rust
   // build.rs (new)
   println!("cargo:rerun-if-changed=build.rs");
   if let Ok(out) = Command::new("git").args(["symbolic-ref","-q","HEAD"]).output() {
       if out.status.success() {
           println!("cargo:rerun-if-changed=.git/{}", String::from_utf8_lossy(&out.stdout).trim());
       }
   }
   println!("cargo:rerun-if-changed=.git/packed-refs");
   let sha = /* git rev-parse HEAD, or "unknown" */;
   let dirty = /* git status --porcelain, non-empty */;
   println!("cargo:rustc-env=ALLSPARK_BUILD_COMMIT={sha}");
   println!("cargo:rustc-env=ALLSPARK_BUILD_DIRTY={}", if dirty {"1"} else {"0"});
   ```

   **Measured gotcha, and the reason for `symbolic-ref`:** with
   `cargo:rerun-if-changed=.git/HEAD` only, a new commit does **not** rebuild — `HEAD`
   holds `ref: refs/heads/main`, unchanged, so the binary keeps reporting the old sha.
   Reproduced twice on this box (before: `124a1da5` reported, `b7169761` on disk; after
   watching the resolved ref: `c0210f58` correctly reported). Also: a stamp variable set
   in the environment (`cargo:rustc-env`-style `option_env!` values like
   `PROBE_STAMP`) *is* re-read on rebuild, so `ALLSPARK_FORK_BUILD` continues to work
   unchanged.

2. **Build identity (content):** the artifact's own bytes. Two cheap options:
   - ELF `NT_GNU_BUILD_ID` (present in every binary here — verified with `readelf -n`:
     fork.010 `429892b7…`, upstream 0.10.1 `9fabf13d…`, local dist `247167fc…`,
     stale local `07f78b3c…`). Parsing it from a file took **0.053-0.143 ms** in plain
     Python; the same parse in Rust is a few dozen lines over the program headers.
     It is per-link, so it differs between two builds of the same commit — which is
     exactly what "is the running daemon *this* binary?" needs.
   - sha256 of the payload: correct for verification, but **172 ms for 69 MB** (measured
     cold, 131 ms warm) — too slow for startup; fine for `--self-test` and for the gate.
3. **Protocol identity (compatibility):** protocol version + schema id — unchanged, and
   never a substitute for the other two (it is identical across all four binaries above).

### 7.4 Where each identity travels

| surface | content | why |
|---|---|---|
| `daemon_hello.runtime.buildId` | `pa-daemon-rs-<version>+<commit12>[.dirty]` (replacing the version-derived string at `clients.rs:126`) | client can compare source identity on handshake |
| `daemon_hello.runtime.buildId` — additional field, do not overload the wire | `contentId` = GNU build-id (ELF) / sha256 (other) | new `#[serde(default, skip_serializing_if…)]` field in `DaemonRuntimeIdentity` (`crates/pa-types/src/daemon/outbound.rs:146-153`), so an older peer simply omits it |
| `--version` | one extra line: `build <commit12>[.dirty] content <build-id12>` | a user pasting `--version` into an issue gives us both identities |
| release `manifest.json` + `SHA256SUMS` | archive sha256 (exists), payload sha256, commit, content id | post-publish verification without the tag |
| `doctor --json` | both, plus `status` | already exists; add the fields, keep the shape |

The comparison policy (all three cases must be *stated*, not implied):

- **content id differs, protocol matches** → the daemon is the wrong build: shut it down
  when idle (the existing path: `daemon.rs:61` → `shutdown_stale_daemon` `:112-170`), and
  refuse with the existing "incompatible daemon" message when busy.
- **source commit differs, protocol matches** → same handling, message names the commit.
- **content id absent (older peer)** → fall back to protocol+schema, and print one warning
  line; never silently.
- The daemon must **not** exit on its own; a client-side decision keeps the "busy daemon
  refuses replacement" contract intact.

### 7.5 How to test it (cheapest tier that can observe the defect)

- **Unit** — `classify_hello` truth table over: same build / different content id /
  different commit / different version / missing fields. Red-first: the
  different-content-id case must fail on today's code (today it returns `Current`).
- **Integration, two fake hellos** — reuse the existing Python fixture supervisor
  (`scripts/lifecycle_lib.py:199` `Supervisor`, hello injected at `:270`), and the
  `scripts/check_reconnect_version.py` pattern that already rewrites `appVersion` in a
  hello (`:70-80`). Add a case that keeps `appVersion`/`schemaId` identical and changes
  only the content id, then assert the TUI does **not** ride it (a fake supervisor already
  exists for exactly this shape, so this is an extension, not new infrastructure).
  I reproduced the current wrong behaviour by hand today:
  `doctor --json` → `"status":"current"` for a fixture daemon with a different
  `executablePath` (§7.1 B).
- **PTY e2e** — `scripts/lifecycle_lib.py` `Terminal` + `wait_for_text` (the same harness
  as `check_reconnect_version.py`): assert the refusal banner text, not an exit code.
- **Artifact level** — the gate of §6.2 asserts `--version` carries the commit and that
  `--self-test` (if added) reports the same content id as the manifest inside the tarball.

### 7.6 Consequence for the release contract (the parent's last question)

For a *published* artifact the chain must be checkable with only the download:

```text
archive  --sha256--> SHA256SUMS (or the channel manifest row)
archive  --extract--> libexec/prime-agent --sha256--> manifest.payloadSha256
                     libexec/prime-agent --build-id--> manifest.contentId
                     libexec/prime-agent --version --> version + commit + contentId
                     libexec/models.bundled.json    --> provenance.source != fixture
```

Every arrow is cheap (§6.5) and none requires the tag. `--self-test` is the one addition
worth making to the product surface: one command that prints those five facts and exits
non-zero when the install's assets contradict its binary. `doctor` already covers the
daemon half; `--self-test` covers the install half, which is what failed here.

---

## 8. What we already have (do not re-propose)

- `scripts/release/verify_release.py` — tarball shape, checksums, manifest, staged-binary
  version, catalog validation. Missing from the fork workflow only.
- `scripts/release/bundle_catalog.py` — schema/strictness gates mirroring the runtime's
  `deny_unknown_fields` (`:122-250`), plus the (new, uncommitted) fork-provider check.
- `scripts/release/test_catalog_assets.py`, `Makefile:165 catalog-assets-gates`.
- `scripts/release/test_channel_install.py` — a real install-and-smoke for the *upstream*
  layout (`curl | sh` against a local channel, twice, with user-state preservation).
  The fork's tar layout is what needs the equivalent.
- `qa-tasks/RT-013-version-stamp-check` — the only artifact-adjacent QA task
  (`--version` must carry `fork.NNN`). It cannot catch this defect: the published
  fork.010 satisfies it.
- `allspark doctor --json` — already prints `version`, `buildId`, `executablePath`,
  `status: stale|current`. The reader is missing, not the data.
- PTY harness (`crates/pa-perf/src/bench/pty.rs`, `scripts/lifecycle_lib.py`,
  `crates/pa-cli/tests/login_panel_pty_e2e.rs`).

## 9. Explicitly UNVERIFIED

- The exact mechanism that moved `allspark-v0.10.2-beta.3-fork.010` to `01de64db2` while
  the assets came from `5e300a7bf` (§4.1). The data is from the GitHub API; the cause is
  not determined, and nothing in the pipeline would have told us either way.
- Whether `mcp-services.bundled.json`'s 73 `fixture-svc-*` entries are *visible* in the
  TUI service picker. The read path exists (`crates/pa-core/src/mcp/remote_source.rs:75-80`
  ← `crates/pa-core/src/mcp/mod.rs:337`), but I could not drive the TUI here; treat the
  user-visible impact as UNVERIFIED, the content assertion as still correct.
- `cargo-dist`'s absence of an install-smoke job is from a DeepWiki answer plus the
  template source, not from running it.
- z.ai billing state (parent-verified; out of scope here).

## 10. Recommended order of work

1. **Unblock and correct the catalog source** (blocks everything): release path uses
   `--network`/`--catalog-dir`, fixture stays a test-only mode; add the provenance marker;
   make `test_catalog_assets.py` green and wire it into `prepush-gate.sh`.
2. **Gate A + B** (§6.1, §6.2) in `release-fork.yml` before upload, plus locally in
   `make release-dry-run`. Two seconds.
3. **Gate C** (§6.3) as a dispatchable read-only job; it would have caught the missing
   `stable`/`latest.json` today.
4. **Build identity** (§7): `build.rs` + commit/dirty, `contentId` from the ELF build-id,
   one shared `classify_hello`, `--version` and `--self-test` carrying both, manifest
   fields, tests at unit + two-hello + PTY tiers.
5. **Then** immutable releases + `actions/attest-release`/`attest-build-provenance`
   (https://github.com/marketplace/actions/attest-build-provenance), and drop the
   `--clobber` repair path in favour of roll-forward.

---

## Appendix A — reproduction of every measurement (commands as run)

All scratch output lived under `/root/prime-agent/.tmp` (nothing outside the repo, no
daemon touched, no `install`/`update` invoked).

```bash
# 1. what the published fork release actually contains
gh release download allspark-v0.10.2-beta.3-fork.010 -R rager306/allspark -D .tmp/r
tar -xzf .tmp/r/*.tar.gz -C .tmp/r
python3 -c 'import json,sys; c=json.load(open(sys.argv[1])); ms=c["models"]; \
print(len(ms), sum(m["id"].startswith("fixture-") for m in ms), len({m["provider"] for m in ms}))' \
  .tmp/r/*/libexec/models.bundled.json        # -> 46 46 32
sha256sum .tmp/r/*.tar.gz                     # -> 87462f0f7d1c7198… == SHA256SUMS == asset digest
gh release verify allspark-v0.10.2-beta.3-fork.010 -R rager306/allspark      # -> no attestations
gh attestation verify .tmp/r/*.tar.gz -R rager306/allspark                  # -> HTTP 404
gh api repos/rager306/allspark/immutable-releases                            # -> enabled:false

# 2. the three-catalog comparison (fixture vs real vs none), dead proxy + stub keys
#    (the harness is .tmp/artifact-probe; a `model list` under a dead proxy is 0.03 s)
HTTPS_PROXY=http://127.0.0.1:9 ZAI_API_KEY=x ANTHROPIC_API_KEY=x OPENAI_API_KEY=x \
  MUSE_API_KEY=x NINEROUTER_API_KEY=x OPENCODE_API_KEY=x \
  HOME=$H PRIME_AGENT_CODING_AGENT_DIR=$H/agent ./prime-agent model list
#    -> none: 18 models / 7 providers / 0 fixture ids;
#       fixture asset: 20 / 7 / 12 fixture ids;
#       real catalog: 173 models / 7 providers / 0 fixture ids;
#       and with NO stub keys at all: 2 models in all three cases (the blindness)

# 3. the catalog gate as it currently stands in this tree
python3 scripts/release/bundle_catalog.py generate --fixture --out .tmp/c && \
python3 scripts/release/bundle_catalog.py verify --out .tmp/c      # -> rc 1 (fork providers)
python3 scripts/release/bundle_catalog.py generate --network --out .tmp/n  # -> rc 1 (same)
python3 scripts/release/test_catalog_assets.py                     # -> 24 tests, 10 failures

# 4. the artifact gate prototype (this report), on the published archive
python3 .tmp/artifact-gate.py .tmp/r/*.tar.gz 0.10.2-beta.3 fork.010
#    -> FAIL: 46/46 model ids are synthetic (1.1 s)

# 5. build identity
find crates -name build.rs | wc -l                 # -> 0
grep -rn 'GIT_HASH\|git_sha\|commit_hash\|SOURCE_DATE' crates/   # -> nothing
readelf -n <binary> | grep 'Build ID'              # each binary carries NT_GNU_BUILD_ID
python3 .tmp/buildid.py <4 binaries>               # 0.05-0.14 ms per file to parse
#    build.rs stamping gotcha: watching only .git/HEAD never rebuilds on a commit;
#    watching the symbolic-ref target does (.tmp/stamp-probe)
```

The prototype gate and the probe helpers are scratch, not product code; delete with
`rm -rf .tmp/artifact-gate.py .tmp/buildid.py .tmp/artifact-probe .tmp/healthy-archive
.tmp/stamp-probe .tmp/probe-doctor* .tmp/hello-*`.

