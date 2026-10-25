# Topic 4 — Automatic invariants on the data path (and the build-identity invariant)

Date: 2026-10-24. Scope: how the class "the data set silently becomes wrong/empty" is caught
systematically instead of one bug at a time, plus the second half of the same class — a running
process whose identity nobody can verify. Read-only research: no code edits, no releases, no
daemon touched, no `update`/`install` run.

All measurements below were taken on this box on 2026-10-24 against
`/root/.local/libexec/prime-agent` (0.10.2-beta.3), `/root/.local/libexec.manual-old/prime-agent`
(0.10.1-beta.35), the published `allspark-v0.10.2-beta.3-fork.010` tarball, and hermetic
`HOME`/`PI_PACKAGE_DIR` fixture directories under `./.tmp`. Anything I did not run is marked
**UNVERIFIED**.

## TL;DR (10 lines)

1. The empty/wrong model list is a *data* defect, not billing: the release ships
   `models.bundled.json` generated with `--fixture` (46 synthetic ids, **0** shared with the live
   catalog), and `chain.rs:183` lets it *replace* the compiled table.
2. `pinning.rs:84-88` silently drops any entry that is not a compiled transport — the second half
   of the same class, and the exact reason a 46-model asset renders as 2–4 rows.
3. The invariant that would have caught it is "replacement must be a superset of the source, per
   provider, plus a hash total" — a real practice (§3.5: dbt-utils `equal_rowcount`,
   ETL control totals, SBOM declared-vs-actual).
4. The just-added packer gate tests the wrong thing (fork provider *names*) and is red right now:
   10 of 24 packer tests fail, `--network` and `--fixture` both abort (§4.4).
5. Version comparison is not weak — it is defeatable: an old binary with a forged `package.json`
   is `"status": "current"` to the new binary's `doctor --json` (§5.2, measured).
6. `buildId` is `concat!("pa-daemon-rs-", CARGO_PKG_VERSION)` (`clients.rs:126`); `schemaId` is a
   hand literal; the one real content hash we ship (`.note.gnu.build-id`) is never read.
7. Fix = one stamped build record (`build.rs` + commit/dirty/target/profile → 12-hex `buildId`),
   one identity classifier used by all three call sites, and `executableSha256` (already published)
   checked at install.
8. Data fix = `ResolvedCatalog` with a layer + drop report, runtime warnings at the registry choke
   point (never throw), and a packer gate on superset + hash total instead of provider names.
9. `doctor` must become a real health check with an exit code: measured exit 0 today on a `stale`
   daemon actively serving the session.
10. Full matrix of 11 invariants → level → cost in §6; ordered P0/P1/P2 plan in §8.

---

## 0. What is new here (and what is deliberately not repeated)

I read `blackbox-qa-best-practices.md`, `fork-testing-research.md`, `TEST-MATRIX.md`,
`oh-my-pi.md`, `oh-my-pi-deep.md` first. What they already cover and I do **not** restate:
behavioural evals / pass^k / judges / judge drift (blackbox), wire-level mock servers, PTY
harnesses, soak gates, MCP conformance, cassettes, wire-shape pins (fork-testing-research), the
T1–T8 test tiers and the contract-gate idea (TEST-MATRIX), omp's advisor/native-bounds/snapcompact
patterns (oh-my-pi*, plus `learn_*.md` and the sqlite/harness roadmaps).

**New in this file:**

1. A named catalogue of **data-path invariants** (non-empty, per-provider, superset-of-source,
   hash-total, value-trust, provenance, build identity) with the *level* each belongs to and its
   measured cost — the matrix in §6.
2. The finding that the "size relative to source" practice **exists** (§3.5) under three names
   (dbt-utils `equal_rowcount`/`fewer_rows_than`, ETL *control totals*, SBOM declared-vs-actual),
   and why a pure count is not enough — you need a *hash total*.
3. The framing of `chain.rs` step 2 as an **unguarded schema-registry upgrade with compatibility
   mode NONE** (§3.6) and what the registry would have done instead.
4. A **measurement** that the just-added packer gate is at the wrong layer: it rejects the
   fixture *and* the live catalog, and 10 of 24 packer tests are red right now (§4.4, §8-P0.3).
5. A **measurement** that version comparison is not merely weak but *defeatable*: an old binary
   with a forged `package.json` is classified `current` by `doctor --json` (§5.2).
6. A concrete **build-identity design** for Rust 2026 (`build.rs` stamp, GNU build-id, published
   `executableSha256`, and where each one belongs), with the measured costs (§5).
7. The observation that the packer's model-schema check **duplicates** the runtime's strict schema
   and nothing ties the two together (§4.5) — the same "two classifiers can disagree" defect class
   as the daemon probe.

---

## 1. The incident, reduced to two invariants

The user reported "providers are there, models are not", then a 429 from z.ai. The 429 is a
billing zero at z.ai (proven by the parent with direct curl) — **not** a code defect. The empty/
wrong model list is ours, and it is reproducible without any daemon:

```bash
# $T/pkg holds a copy of the asset the fork release ships (models.bundled.json);
# $T/agent/auth.json configures exactly one provider (zai). Hermetic HOME, no daemon.
env -i HOME=$T PI_OFFLINE=1 PI_PACKAGE_DIR=$T/pkg PRIME_AGENT_CODING_AGENT_DIR=$T/agent \
  PRIME_AGENT_SESSION_DIR=$T/agent/sessions PATH=/usr/bin:/bin \
  /root/.local/libexec/prime-agent model list
# provider   model                     context  max-out  thinking  images
# opencode   deepseek-v4.1-flash-free  200K     32K      yes       yes
# zai        fixture-zai-42            128K     4.1K     no        no      <-- an id that does not exist upstream
```

Two invariants were violated at the same time:

- **I-COMPLETE**: the resolved catalog must not be *smaller than the compiled one* in any provider
  it claims to serve. The shipped asset carries **46 models, 0 of which are compiled ids** and
  **46 synthetic `fixture-*` ids** across all 32 provider names (§4.1). Substitute `anthropic` for `zai` and the user sees
  `fixture-anthropic-4` + `fixture-anthropic-reasoning` and no real Claude ids at all.
- **I-IDENTITY**: nothing in the handshake distinguishes two binaries of the same version, so the
  TUI talked to a 0.10.1-beta.35 daemon while a 0.10.2-beta.3 payload was installed (§5.1).

Both are *silent*: `model list` exits 0, the TUI renders, `doctor --json` exits 0, and the
telemetry line `catalog_refresh` reports a healthy-looking count (`track.rs:88-93`).

## 2. Theory: absence is an error, not an empty list

- **Fail-closed vs fail-open.** Fail-open proceeds on an unaccounted state; fail-closed blocks it
  (AuthZed, *Understanding "Failed Open" and "Fail Closed"*, updated 2024-11-20,
  https://authzed.com/blog/fail-open). Our chain is fail-open by design at the *availability* level
  ("a user always has models, offline or not", `chain.rs:1-5`) — correct for the user, but the
  substitution must still be **declared**, not silent.
- **"Parsed, unmarked, silently inert" is a prohibited fourth state.** ADR-0078 (ObjectStack,
  amended 2026-10-16,
  https://github.com/objectstack-ai/objectstack/blob/main/docs/adr/0078-no-silently-inert-metadata.md)
  states the trichotomy precisely: a config whose omission makes an otherwise-valid instance do
  nothing must be (1) completeness-enforced, (2) marked `[EXPERIMENTAL — not enforced]`, or
  (3) genuinely optional with graceful degradation. The fourth state — *parsed, unmarked, silently
  inert* — is prohibited. Its §4 "loud-not-silent at the runtime seam" says: **warn at the choke
  point where every instance converges, and never throw** (an inert field must not kill a boot).
  That is exactly the shape we need: the packer may be strict, but the *runtime* must warn at
  `registry` load, because the packer is not the only door (models.json, stale assets, an
  old daemon's own package dir).
  Its siblings named in the header — ADR-0049 (enforce-or-remove), ADR-0054 (prove-it-runs),
  ADR-0072 (reference scope and resolvability), ADR-0077 (surface-selection traps) — are the same
  family; I did not fetch them (**UNVERIFIED**; my `/0072-reference-resolvability.md` URL 404'd,
  the real name per the 0078 header is `0072-reference-scope-and-resolvability.md`).
- **Parse, don't validate** (Alexis King, 2019-11-05,
  https://lexi-lambda.github.io/blog/2019/11/05/parse-don-t-validate/): push the check to the
  boundary and return a type that *cannot* be empty, so no downstream caller needs `is_empty()`.
  Rust translation: `NonEmpty`, or a `ResolvedCatalog` newtype with a private field.
- **Make illegal states unrepresentable** (corrode.dev, 2023-11-18,
  https://corrode.dev/blog/illegal-state/): the `Username(String)` example — a private-field
  newtype with a fallible constructor is zero-cost and removes the class, not the instance.
- **Referential integrity is a foreign key, not a schema rule.** dbt's `relationships` test
  "validates that all of the records ... every non-null value in a foreign key column exists in
  the referenced column" (https://docs.getdbt.com/reference/resource-properties/data-tests,
  also https://www.elementary-data.com/dbt-tests/relationships). Our `provider → model` relation
  is a foreign key with **no** enforced parent row: `provider_login.rs:366-377` derives the
  `/login` provider list from `registry.get_all()` (so a provider row appears as soon as *any*
  model carries it), while the picker shows only what survived pinning. A provider row with zero
  surviving models is an orphan, and it is exactly the user-visible text of the incident.

## 3. Tooling, 2025–2026: what actually catches this class

### 3.1 JSON Schema — necessary, not sufficient

JSON Schema 2020-12 constrains *one instance*: types, ranges, `deny_unknown_fields` analogues,
enums. It cannot express "every provider in this array also appears in that other array", because
`$ref` is schema composition, not a data reference. The question has a standing open discussion
(json-schema-org discussion #934, *Mechanism for describing "foreign key" relationships between
two schema*; I confirmed the URL exists but could not extract the thread body — **UNVERIFIED
content**). `schemars` documents per-field restriction attributes (`validate`/`garde`) but nothing
cross-object (https://docs.rs/schemars/latest/schemars/derive.JsonSchema.html).

We already do the per-entry half in *two* places: the Rust strict parse
(`crates/pa-models/src/schema.rs:30-63`, `deny_unknown_fields` on `CatalogModelSchema` at `:34`) and a
hand-maintained Python mirror of the key set (`scripts/release/bundle_catalog.py:102-106`,
`_MODEL_ENTRY_KEYS`). Nothing verifies that the two key sets agree; a field added to Rust and not
to Python makes the packer accept an asset the runtime will reject **whole** (fail-closed
`InvalidEntries::Reject`, `bundled.rs:79`) — and `bundled.rs:79` swallows that into `None` →
compiled fallback, i.e. *silent*.

**Verdict:** keep JSON Schema for shape; do not expect it to carry any of §6's I2–I5.

### 3.2 Property-based testing (`proptest`)

Right tool for the *merge/pin* algebra, because the bug is a function of arbitrary inputs, not of
one fixture. Properties worth writing (each cheap, no I/O):

- `resolve(bundle) != []` for every bundle that pins ≥1 model (non-empty is preserved).
- **No silent loss**: `providers(input_bundle) ⊆ providers(output) ∪ reported_drops`. Today
  `pin_catalog_models` drops with `continue` and `Result` carries only "all were dropped"
  (`pinning.rs:84-99`); there is no way to observe a *partial* drop. Measured (§4.2): a bundle
  entry for `muse` or `codebuddy` disappears with zero signal.
- **Pinning can never introduce a transport**: `(provider, api, baseUrl) ∈ compiled_transport_tuples()`
  for every output (already true by construction; a property test pins it against refactors).
- **Superset**: `ids(compiled_non_pi) ⊆ ids(output)` for the bundled layer (today false — §4.1).
- Resolver: `build_fallback_model(p, id, catalog)` is `Some` **iff** `p` has ≥1 model
  (`resolver.rs:137-143` — already coded that way; a property test guards it).

Cost: `proptest` is already a dev-dependency pattern in the repo (TEST-MATRIX §1.2, geometry and
supervisor). Each property ≈ 50 lines, runs in ms.

### 3.3 Contract testing (Pact) — the packer must run the *consumer's* parser

Pact's contract, in its own words: "contract tests assert that inter-application messages conform
to a shared understanding that is documented in a contract"; the consumer records the expectation,
the provider verifies it (https://docs.pact.io/). The transferable rule for us: **the producer
(packer) must verify its artifact with the consumer's own code, not with a re-implementation of
it.** Today the packer's gate is a re-implementation (`_MODEL_ENTRY_KEYS`,
`validate_bundled_model_catalog`), so it can drift; a `prime-agent model verify --asset <path>`
subcommand (or a `pa-models` test binary invoked by the packer with the real parser) removes the
duplicate source of truth in one move. This is the same discipline as `verify_release.py:184`,
which already calls into the *packer's* Python gate — correct direction (consumer verifies
producer), wrong parser (still a copy).

### 3.4 Data-quality frameworks — what to steal, what not to

- **dbt**: `relationships` (referential integrity), `not_null`, `unique` — per-row/per-key checks,
  wired into CI so a failure fails the build
  (https://docs.getdbt.com/reference/resource-properties/data-tests).
- **Soda / Great Expectations**: declarative checks with thresholds and anomaly scores
  (`row_count > 0`, `schema` checks; https://docs.soda.io/soda-documentation/soda-v3/run-a-scan;
  `ExpectTableRowCountToBeBetween`, gallery page
  https://greatexpectations.io/expectations/expect_table_row_count_to_be_between/ — page body is
  JS-rendered, **UNVERIFIED** wording).
  What ports: the *declarative check file*, not the framework. One YAML/JSON of "expected facts
  about this release's catalog" evaluated by a 60-line Rust/Python checker is enough.
- What does **not** port: scheduling, anomaly baselines ("today's row count vs last week's"). We
  have a build-time producer, not a streaming table; the check belongs at pack, install, and
  startup — not on a cron.

### 3.5 The "size relative to source" invariant — yes, it exists (three names)

This was the explicit question. The practice is real and older than 2026:

1. **dbt-utils `equal_rowcount` / `fewer_rows_than`** — "Asserts that two relations have the same
   number of rows" / "fewer rows than the model being compared"
   (https://raw.githubusercontent.com/dbt-labs/dbt-utils/main/README.md, sections
   `equal_rowcount`/`fewer_rows_than`). The purest form of "replacement must not shrink the set".
2. **Control totals** (classic batch/audit control): record count + *hash total* + financial total,
   compared before and after processing. The **hash total** is the part we need: a *count* can
   match while the *content* was substituted. Our equivalent is a digest over the sorted
   `(provider, id)` set — a count-only check passes on today's fixture (46 vs 152 fails anyway,
   but a 152-model fixture with 46 substitutions would pass a count check).
   Sources for the term are teaching/audit material; I verified the concept, not a canonical page
   (**VERIFIED-WEAK**).
3. **SBOM declared-vs-actual** — "derive a binary inventory from the shipped artifact and diff it
   against the declared SBOM" (CodeAnt, *SBOM in Practice*, 2026-09-08,
   https://codeant.ai/blogs/sbom-formats-tools-binary-gap). That article's asymmetry argument is
   the one to quote to a reviewer: an attacker needs **one** undeclared component; you need the
   declaration to be complete **for every component, on every release**. Substitute "stale
   catalog asset" for "undeclared component" and it is our incident.

**Concrete recommendation for us — the "superset + hash total" pair:**

```
COMPILED  = models_generated.rs (152 models, 32 providers; 42 non-PI ids)
ASSET     = models.bundled.json          (the replacing source)
INVARIANT (release gate, I-SUPERSET):
   ∀ provider p ∈ providers(COMPILED)\{prime-inference}:
       ids(ASSET, p) ⊇ ids(COMPILED, p)                     # size relative to source, per provider
   digest(sorted {(p,id) | ASSET}) ⊇ digest(sorted compiled) # hash total, not just a count
INVARIANT (runtime, I-UNION):
   resolved = ASSET ∪ FORK ∪ PI_OFFLINE ∪ live
   must satisfy: resolved ⊇ FORK_TABLE  and  ∀p ∈ resolved: |models(p)| ≥ 1
```

Measured on the shipped asset: `ids(ASSET) ∩ ids(COMPILED) = 0` for all 31 non-PI providers
(§4.1). The invariant is violated by 100% today, and a pure provider-count check passes
(32 vs 32) — which is precisely why the current gate (fork providers present) is the wrong test:
it passes/fails on provider *names* while the ids are all synthetic.

### 3.6 The fallback chain is an unguarded schema upgrade (compatibility mode NONE)

Confluent's Schema Registry defines the modes a new version must satisfy: BACKWARD, FORWARD, FULL,
TRANSITIVE (default BACKWARD), and rejects a version that breaks the configured mode
(https://docs.confluent.io/platform/current/schema-registry/fundamentals/schema-evolution.html).
Read `chain.rs:165-187` in that vocabulary: step 2 **replaces** step 3 (`Some(bundled) => bundled`,
`chain.rs:183`); parents are not unioned. That is a FORWARD-incompatible upgrade by construction —
consumers built against the compiled set lose ids. In a registry it would be rejected before
publication. Here nothing rejects it, and `chain.rs:3-5` calls the replacement the design.

Two acceptable repairs, both cheap:

- **Union instead of replace** (`compiled ∪ bundled`, dedup by `(provider,id)`, bundled wins on
  values): makes I-SUPERSET true by construction at runtime, and turns the asset into an *overlay*
  rather than a replacement. Risk: two sources of truth for a value; mitigate with I5
  (value-trust: an overlay may not *lower* `contextWindow`/`maxTokens` below the compiled value
  without a declared flag). Note the current shape has the opposite risk already: a bundle claiming
  `contextWindow: 1` is believed (measured, §4.3).
- **Replace + declare**: keep replacement, but return the layer and the drop report so every
  surface (picker, doctor, telemetry) can say "serving packaged snapshot: 46 models, 106 compiled
  ids replaced".

### 3.7 Best-practice shape for a `doctor` (derived health check)

`npm doctor` is the canonical small version: a fixed set of *named* checks over the real
environment (registry reachable, npm/node versions, permissions, cache checksums), all run by
default, selectable by argument (https://docs.npmjs.com/cli/v10/commands/npm-doctor). Two lessons
we need: (a) checks are **named and independently runnable**, (b) the report names a *recommended
change*, not just a status. `expo doctor` adds the CI lesson: it must **exit non-zero** when it
finds problems — that was filed as a bug and fixed
(https://github.com/expo/expo-cli/issues/4231). Our `doctor` cannot fail CI or a script today
because it always exits 0 (§4.6, measured).

The "derived" part is ADR-0078 §2/§4: **one shared predicate consumed by every surface**. We have
the anti-pattern instead: two classifiers for the same fact (`classify_reachable`,
`daemon_discovery/mod.rs:416-425`, versus `probe_daemon`,
`interactive_mode/daemon.rs:31-47`) that disagree for the same live daemon — measured: `doctor`
says `stale`, the TUI's own probe says current.

### 3.8 Database-constraint analogues for an in-memory catalog

A relational catalog gets this class for free; ours is a `Vec<Model>` built in code, so each
constraint has to be re-created deliberately. Mapping, with where ours stands today:

| DB constraint | in-memory analogue | our state |
|---|---|---|
| `NOT NULL` | field required in the strict schema | **present** (`CatalogModelSchema`, `schema.rs:31-62`) |
| `CHECK (x BETWEEN lo AND hi)` | per-entry range validation | **present** (`validate_entry`, `schema.rs:164-197`) |
| `UNIQUE (provider, id)` | duplicate rejection at parse | **present** (`schema.rs:116-121`, rejects even in skip mode) |
| `FOREIGN KEY (provider) REFERENCES providers(id)` | every model's provider exists in a provider registry; **and** every provider in the picker/`/login` has ≥1 model | **absent** — this is I2 |
| `ASSERTION` / deferred trigger | invariant over the *result* of a merge/overlay (`resolved ⊇ fork`, no value lowered) | **absent** — I3/I5 |
| `CHECK` on a derived column | "replace must not shrink the parent set" | **absent** — I3/I4 |
| `ON DELETE RESTRICT` | removing a model a saved config names must fail or deprecate, not vanish | **partly** (`docs/model-discovery-contract.md` §6 recommends `deprecate`; `list_models.rs:84-92` already renders `[retired <date>]`) |

"DBCONSTRAINT for in-memory catalogs" is therefore not a library — it is *one* validated constructor
that runs the four absent rows and returns a type that cannot be built otherwise (§7.1). The
practical rule: enforce at construction (cheap, total) and **report** at merge (the runtime door a
packer cannot close).

**Golden-invariant tests** (the cheap form that survives refactors): assert the *relationship*
between two artifacts rather than either artifact's content — "every compiled non-PI id appears in
the live catalog fixture" (true today, 42/42, measured), "every compiled non-PI id appears in the
bundled asset, or the asset is explicitly marked partial" (false today, 0/42). These do not need a
golden copy of 1197 entries; they need one set operation per invariant.

### 3.9 The same list applied to us (what `allspark doctor` should become)

Each check is a named predicate with a stable id, runnable alone (`doctor --check assets`), and
shared with the runtime so the two can never disagree. The list is ordered cheapest-first and is
exactly what answers the incident:

| id | check | passes when | today |
|---|---|---|---|
| `install.present` | payload exists, is executable, `--version` answers <10 s | always | ✔ (`install-rust.sh:2794` probes at install time only) |
| `install.identity` | `buildId` == manifest `buildId`, sha256 == manifest `executableSha256` | artifact is the published one | ✘ (installer ignores `executableSha256`; §5.1) |
| `assets.present` | both bundled assets exist beside the payload | both files | ✔ (packer gate, `verify_release.py:184`) |
| `assets.schema` | asset parses with the **runtime's** parser | parse ok | ✘ packer uses a Python copy (§4.5) |
| `assets.superset` | every compiled non-PI `(provider,id)` present in the asset | I3 | ✘ **0 of 42** (§4.1) |
| `assets.not_fixture` | no id matches `^fixture-` in a release build | I11 | ✘ the shipped asset is 100% `fixture-*` |
| `catalog.layer` | reports which layer served the catalog | always | ✘ nothing reports it |
| `catalog.nonempty` | resolved catalog is non-empty | I1 | ✔ by accident (fallback) |
| `catalog.provider_integrity` | **every provider with a configured credential has ≥1 model** | I2 | ✘ today a provider row can exist with zero models (§4.2) |
| `catalog.fork_covered` | resolved ⊇ `fork_provider_models()` | true via `registry.rs:600`, never checked | ✔ unchecked |
| `catalog.value_trust` | no resolved `contextWindow`/`maxTokens` below the compiled value without a flag | I5 | ✘ (measured `1 / 1`) |
| `daemon.identity` | running daemon's `buildId` == this binary's | I9 | ✘ version-only, and defeatable (§5.2) |
| `daemon.sessions` | a stale daemon is not serving live sessions | — | reported, never acted on |

Exit code: 0 when every check passes, **1** when any `FAIL` line is present (`WARN` keeps 0). That
single change turns the existing `doctor --json` output into something a launcher, a CI job, or a
post-install step can branch on — today `doctor` exits 0 on a `stale` daemon that is actively
serving the session (measured, §4.6).

Post-install placement: `install-rust.sh` already ends with a `--version` probe through the
launcher (`:3604-3613`). Running `doctor --check install.identity --check assets.presence` there is
one extra subprocess at install time and would have failed the `fork.010` install with one line
instead of surfacing as a user-visible empty picker.

## 4. Where our tree already is, and the exact holes (measured)

### 4.1 The replacing asset is a synthetic fixture, and it is empty of real ids

- `chain.rs:183` — step 2 replaces step 3 entirely.
- `.github/workflows/release-fork.yml:127-129` generates the
  asset with `--fixture` and then `verify`s it. `--fixture` emits one synthetic model per compiled
  transport tuple (`bundle_catalog.py:481-496`, ids `fixture-<provider>-<index>`).
- Measured on the installed release: `/root/.local/libexec/models.bundled.json` is **byte-identical
  to `fixture_catalog_bodies()['models.bundled.json']`** (sha256 prefix `c91ee90be4817ebc`), and
  the same file inside the published `fork.010` tarball (archive sha256
  `87462f0f…` matches the published `SHA256SUMS`).
- Counts: asset = **46 models / 32 providers / 0 ids shared with the live catalog**; compiled table
  (`models_generated.rs` `RAW_MODELS`) = **152 models / 32 providers**, of which the **42 non-PI
  ids are all present in the live catalog** (`crates/pa-models/tests/fixtures/catalog.v1.json`,
  1197 models / 31 providers). So the shipped asset's ids are 100% synthetic and its provider set
  is a superset by *name* only.
- Consequence measured: with only `zai` configured, the picker offers `zai/fixture-zai-42` and
  none of the 7 real `zai` ids; with `anthropic`, `fixture-anthropic-4` +
  `fixture-anthropic-reasoning` and none of the 15 real ones. "Providers are there" comes from
  `/login`, which lists providers from `registry.get_all()` (`provider_login.rs:366-377`); "models
  are not" is the pinned result. Reproduced hermetically; no daemon involved.

### 4.2 Silent partial drops (the class inside the class)

`pinning.rs:84-88` drops any entry whose `(provider, api, baseUrl)` is not a compiled tuple, with
a comment that says so and no signal. Measured: a hand-written asset with one `anthropic` tuple and
one `muse` entry (`https://api.meta.ai/v1`) renders the anthropic row and **nothing** for muse — no
warning, exit 0. Also measured: the fork table's providers are *not* compiled transports at all
(`'muse' ∈ compiled providers == False`, same for `codebuddy`), so a bundled muse entry can never
survive pinning. That is the decisive argument against the new packer gate's premise (§8-P0.3).

### 4.3 The asset is trusted for values

Measured: a bundle claiming `contextWindow: 1, maxTokens: 1` for `zai/glm-4.7` is printed verbatim
(`1 / 1`), while the compiled fallback prints `204.8K / 131.1K`. A wrong-asset release therefore
silently degrades context budgeting (and `contextWindow` feeds compaction thresholds).

### 4.4 The new packer gate is at the wrong layer (and is red now)

The uncommitted `scripts/release/bundle_catalog.py:113-120, 190-210` (verified at `:202-210`) adds: every release asset must
carry `FORK_PROVIDERS = {muse, codebuddy, genspark, subquadratic, ninerouter, opencode}`.
Measured effects, on the current worktree:

- `bundle_catalog.py generate --network` → **rc=1**, same message (the live catalog has no fork
  providers — it is upstream's).
- `bundle_catalog.py generate --fixture` then `verify` → **rc=1** (`verify` gets no waiver;
  `release-fork.yml:129`, `assemble_artifacts.py:333`, `package_release.py:216` all call
  `validate_bundled_catalog_dir` without `allow_small_fixture`).
- `scripts/release/test_catalog_assets.py` → **10 of 24 tests fail** (the full list, in the
  runner's order): `test_fixture_passes_the_full_gates`; `NetworkMode` × 3
  (`test_network_fetch_is_byte_identical_and_normalizes_newlines`,
  `test_network_sends_bearer_only_when_configured`,
  `test_failed_mcp_fetch_leaves_the_previous_snapshot_intact`); `CatalogDirMode` × 1
  (`test_catalog_dir_copy_is_byte_identical`); `PackerGates` × 5
  (`test_assets_land_in_the_tarball_the_installer_expects`,
  `test_continuous_flow_stamps_and_verifies`, `test_kernel_packer_gates_the_same_assets`,
  `test_kernel_packer_rejects_decoder_leak`, `test_packers_never_ship_generated_skill_caches`).
  Log: `./.tmp/cat-tests.log`.
- The fixture *cannot* satisfy it: `FIXTURE_TRANSPORT_TUPLES` (`bundle_catalog.py:434-478`) has no
  muse/codebuddy/genspark/ninerouter/subquadratic tuple, and even if one were added, pinning would
  drop it (§4.2). The gate as written makes both the fork release and the upstream release lane
  (`release.yml:211` → `assemble_artifacts.py:333`) fail.

The *intent* is right and belongs at the runtime, where the fork table is actually joined
(`registry.rs:600`) — and it is already pinned there by the new tests
(`crates/pa-core/src/models/registry/tests.rs:604-676`). The asset-level proxy is a false-positive
gate, i.e. the "gate that fails a legitimate producer" anti-pattern.

### 4.5 Two definitions of the same schema, unlinked

`bundle_catalog.py:102-106` (`_MODEL_ENTRY_KEYS`) vs `schema.rs:31-62` (`CatalogModelSchema`).
Nothing asserts equality. Same defect class as the two daemon classifiers.

### 4.6 `doctor` exists but cannot say anything about data or identity

`public_command.rs:457-474` → `daemon_discovery::run_ps` (`stop.rs:28-42`): socket table only.
`run_reap` "Returns the process exit code (always 0 — failures are reported as kept lines)"
(`stop.rs:44-46`) — and `run_doctor` **discards** that return value (`public_command.rs:462-467`
calls it as a statement), then returns `handled()` = exit 0 (`public_command.rs:71-73`). So even a
future non-zero from the reaper could not reach the shell. Measured: `allspark doctor` and
`doctor --json` both exit 0 while reporting `"status": "stale"` for the daemon serving this
session. Cost 0.16 s wall / 9.6 MB RSS — cheap enough to run on every launch if it ever had
something to say.

### 4.7 The picker has no empty-state explanation

`model_picker/mod.rs:793` — "TS always opens the menu — an empty catalog renders the empty panel,
never a note"; `render.rs:98-99` prints "No matching models" only when a query filtered everything
out. An empty/asset-replaced catalog is therefore pixel-identical to a healthy one with a bad query.

---

## 5. The second invariant: build identity (version is not a build fact)

### 5.1 What exists, and why it cannot work

- `buildId` is derived from the version: `clients.rs:126`
  `concat!("pa-daemon-rs-", env!("CARGO_PKG_VERSION"))`. Two rebuilds of the same version are
  indistinguishable by it (by construction).
- `schemaId` is a hand literal: `pa-types/src/daemon/mod.rs:17`
  `"protocol-7-schema-30-8e4b17c2a9f5"`, pinned by a test at `mod.rs:175-177`. It cannot detect
  schema drift; only a human edit changes it. Measured: the 0.10.1-beta.35 and 0.10.2-beta.3
  binaries both answer `protocol-7-schema-30-8e4b17c2a9f5` while differing in 6.7 MB of code.
- **The hookup code is a content hash that nobody uses**: `/usr/bin/readelf -n` gives
  `.note.gnu.build-id 247167fc8874b7726d0171b38a87659137e9aa8d` for the installed 0.10.2-beta.3
  binary and `07f78b3ca75335f88aa3299c049ffe6b46589783` for the 0.10.1-beta.35 one. It is present
  even in the stripped `dist` profile (measured on the published `fork.010` binary:
  `429892b7a8e586a4f997342fadcd6f12a97bc55e`).
- `--version` is **manifest-driven, not build-driven**: `config.rs:67-73` reads `package.json` next
  to the executable (`config.rs:90-96`, `PI_PACKAGE_DIR` first). Measured: a forged
  `{"version": "9.9.9-claimed"}` makes both binaries print `allspark 9.9.9-claimed` (fork.010 and
  the old one alike). Version is therefore not evidence of *what code* is running.
- The install probe trusts the printed line: `install-rust.sh:2794-2836` parses `--version` and
  compares versions; the released `manifest.json` does carry `executableSha256`
  (`assemble_artifacts.py:496-503`, checked in `verify_release.py:196`) but the installer never
  reads it (0 hits for `executableSha256` in `install-rust.sh`).

### 5.2 The proof that version comparison is not merely weak but defeatable

Recipe (20 lines, ~15 s, fully hermetic — I ran it):

1. copy the OLD binary into a package dir and write `package.json {"version": "0.10.2-beta.3"}`
   (simulating a same-version rebuild with different code);
2. start it as a supervisor on a fixture socket;
3. run the NEW binary's `doctor --json` against that socket.

Result: `{"version": "0.10.2-beta.3", "buildId": "pa-daemon-rs-0.10.1-beta.35",
"schemaId": "protocol-7-schema-30-8e4b17c2a9f5", "status": "current"}`.

`classify_reachable` compares protocol + schema + `config::version()` (`daemon_discovery/mod.rs:416-425`);
all three agree, so the client rides a build from the previous release. The `buildId` that *does*
differ is already collected (`daemon_discovery/mod.rs:363-369`) and already printed — and never
compared. This is the whole bug in one sentence: **we collect the identity, print it, and classify
without it.**

### 5.3 What Rust 2026 does (and what to take)

| approach | mechanism | what it gives | take? |
|---|---|---|---|
| `build.rs` + `git rev-parse HEAD` (+ dirty) + `option_env!` | emit `cargo:rustc-env=PA_BUILD_COMMIT=…`; `rerun-if-changed=.git/HEAD`, the ref file, `build.rs`; `rerun-if-env-changed=SOURCE_DATE_EPOCH` | exact source identity, works offline, ~30 lines, no new dependency | **yes** |
| `vergen` / `vergen-gitcl` 10.x | emits `VERGEN_GIT_SHA`, emits the same rerun hints incl. `SOURCE_DATE_EPOCH` (https://docs.rs/vergen-gitcl) | the same, plus timestamps/toolchain; 3 crates + `time`/`regex` transitives | only if we want timestamps/toolchain without writing them |
| `shadow-rs` 2.x | git commit, toolchain, profile, deps at runtime (https://docs.rs/shadow-rs) | more fields, one more dependency + `git2` | no |
| `buildid` crate | reads `.note.gnu.build-id` of the including object at runtime (https://docs.rs/buildid) | the linker's identity, 0 build cost | **yes as a cross-check** (we already ship the note) |
| `cargo-auditable` | embeds the dependency list (<4 kB even for 400+ deps) (https://github.com/rust-secure-code/cargo-auditable) | binary-side SBOM; useful for the *asset* half of this report | later |
| Go `-buildvcs` + `debug.ReadBuildInfo` | `vcs.revision`, `vcs.modified`; `-X` for a custom string (https://blog.kowalczyk.info/article/vEja/embedding-build-number-in-go-executable.html) | the mature precedent: revision + dirty flag beat a timestamp | the *shape* (see below) |
| reproducible builds / `SOURCE_DATE_EPOCH` | one standard env var every tool reads (https://reproducible-builds.org/docs/source-date-epoch/) | rebuilds from the same tree hash equal | **yes**, as the acceptance test |
| SLSA / in-toto provenance | signed attestation of builder + inputs, distributed beside the artifact (https://slsa.dev/spec/v1.2/build-provenance) | "who built it" beyond "what is it" | P2 |
| existing stamp | `ALLSPARK_FORK_BUILD` via `option_env!` (`config.rs:61-63`; CI sets it at `release-fork.yml:107`) | already proven in this repo: changing the env **does** force a rebuild (measured twice, §5.4) | **extend this pattern** |

**Measured properties of the GNU build-id** (all with a 2-directory experiment, `dist`-equivalent
profile `strip="symbols"`, `lto="fat"`, `codegen-units=1`):

- stable across two clean builds of the same source in *different* directories (same id, same
  sha256) → it is a function of content, not of the build path;
- changes when the source changes (yes), and when `RUSTFLAGS` changes (yes);
- read cost **0.069 ms** (parsing `PT_NOTE`) vs **177 ms** for a full sha256 of the 70 MB binary →
  usable on every handshake, unlike hashing the executable;
- caveat from the crate docs: a *post-link* modification can change the id without the code
  changing (and vice versa is not guaranteed) — so it identifies "this linked object", not "this
  source tree".

**Recommended identity record** (printed by `--version`, sent in `daemon_hello`, and published in
the release manifest):

```jsonc
{
  "productVersion": "0.10.2-beta.3",     // config::version() — may be restamped by package.json
  "forkBuild": "fork.010",               // ALLSPARK_FORK_BUILD (already shipped)
  "commit": "01de64db2…",                // PA_BUILD_COMMIT (build.rs; "unknown" when no git)
  "dirty": false,                        // PA_BUILD_DIRTY
  "target": "x86_64-unknown-linux-gnu",  // TARGET
  "profile": "dist",                     // PROFILE
  "buildId": "pa-rs-3f9c1a7e2b04"        // sha256(productVersion|forkBuild|commit|dirty|target|profile)[..12]
}
```

`buildId` then *is* a content identifier that survives a version restamp — which is the exact
situation that produced the incident (same version, different code). GNU build-id stays as a cheap
tie-breaker for "is the file on disk the file that is running" (compare
`/proc/<pid>/exe`'s note with the hello's `buildId` only after the record matches), and
`executableSha256` stays as the *published-artifact* identity.

### 5.4 Runtime policy (one classifier, three answers)

Replace the two divergent classifiers with one pure function in `pa-types` (or `pa-daemon`), used
by `classify_reachable`, `probe_daemon`, **and** `DaemonClient::connect`:

```rust
enum PeerIdentity {
    SameBuild,                 // buildId equal                -> attach
    SameVersionDifferentBuild, // version equal, buildId differs-> warn + restart-if-idle (or refuse)
    CompatibleOlder,           // protocol+schema equal, build older -> warn (attach allowed)
    ForeignProduct,            // protocol.name or schema mismatch -> refuse
}
```

- `ensure_daemon_running` already owns the repair path (`interactive_mode/daemon.rs:58-64` →
  `shutdown_stale_daemon`, `:112-152`, refuses a busy daemon with an actionable message). Extending
  `SameVersionDifferentBuild` into that path costs one arm; it must keep the "busy daemon refuses"
  behaviour (restarting under a live session is the 2026-10-21 incident's shape).
- `DaemonClient::connect` currently keeps only `protocol.name` and `min(version)` and discards the
  rest (`daemon_client.rs:439-456`); it never looks at `appVersion`/`buildId`. `hello` is retained
  (`daemon_client.rs:314-315`), so the classifier can run there with no new wire field.
- Never hard-fail on a build-id mismatch alone at the *protocol* layer: the two-viewer attach path
  and the supervisor's own worker hellos (`worker/connection.rs:330-348`, which sends
  `runtime: None`) would break. Warning + idle restart is the safe combination.

### 5.5 Tests for the identity invariant (cheapest tier that can fail)

| test | tier | shape | cost |
|---|---|---|---|
| `identity_classifier_matrix` | unit | table over `PeerIdentity` inputs, incl. forged `package.json` | ms |
| `same_version_different_build_is_not_current` | integration (real supervisor) | the §5.2 recipe: old binary + claimed version, new binary's `doctor --json` must not say `current` | ~2 s |
| `doctor_reports_build_mismatch_on_stderr_and_exits_nonzero` | integration | exit code 1 + one line naming both ids | ~2 s |
| `tui_warns_on_mixed_build_daemon` | PTY e2e | mock supervisor writes a hello with a different `buildId` (pattern: `login_panel_pty_e2e.rs:510`, `pa-perf/bench/pty.rs:214,230,257`), assert the warning line renders | ~5 s |
| `version_restamp_does_not_change_build_id` | unit | build record → restamped version → `buildId` equal; commit change → different | ms |
| `installed_artifact_matches_manifest_build_id` | release gate | `verify_release.py` compares the binary's reported `buildId` with the manifest row | ~1 s |

Red-first: flip the classifier back to version-only; the second row must go green→red.

### 5.6 Where each identity lives (the published-artifact question)

Three different questions, three different fields — conflating them is how we got here:

| question | field | written by | read by | measured cost |
|---|---|---|---|---|
| "what code is this?" | `buildId` (stamped record) | `build.rs` → `option_env!`/`env!` in `pa-cli` | `--version`, `daemon_hello`, doctor | 0 (compile-time string) |
| "is the running file the published file?" | sha256 of `libexec/prime-agent` | `assemble_artifacts.py:496-503` (`executableSha256`, already published) | install-time check (I10), doctor --verify | 76 ms / 70 MB |
| "is this the linked object I debug?" | GNU build-id | the linker (already present, `strip` keeps it) | crash triage; cheap cross-check | 0.069 ms |

`--version` is the one place a human looks, so the line must carry identity, not only the
marketing version. It is `lib.rs:240-244`:

```
allspark <config::version()> (fork build <ALLSPARK_FORK_BUILD|dev>; upstream prime-agent <CARGO_PKG_VERSION>)
```

Measured on three payloads of two builds:

| binary | `--version` | sha256 (size) |
|---|---|---|
| `/root/.local/libexec/prime-agent` (installed) | `allspark 0.10.2-beta.3 (fork build dev; …)` | `116d1346…` (69,587,976 B) |
| `libexec/prime-agent` inside the published `fork.010` tarball | `allspark 0.10.2-beta.3 (fork build fork.010; …)` | `b65b5c17…` (69,608,768 B) |
| `/root/.local/libexec.manual-old/prime-agent` (running daemon) | `allspark 0.10.1-beta.35 (fork build fork.008; …)` | `a42a4fae…` (62,827,704 B) |

Two of them report the *same* version in different builds, and one of those two is unstamped
(`fork build dev`) even though the fork lane always stamps (`release-fork.yml:107`) — i.e. the
installed file is a local rebuild, and nothing except the `fork.010`/`dev` token says so. The
content id belongs on that line:

```
allspark 0.10.2-beta.3 (fork build fork.010; build pa-rs-3f9c1a7e2b04; upstream prime-agent 0.10.2-beta.3)
```

`install-rust.sh:2809-2836` already parses the second field for the version pre-flight; adding a
`build <id>` field means widening that parse (it currently `awk '{print $2}'`s the version — a new
field after the version is safe only if the parser is changed to an explicit token match, not a
positional one).

---

## 6. The invariant matrix (invariant → level → cost)

Levels: **C** compile-time (type), **U** unit, **I** integration/packer, **Q** QA task (faux),
**R** runtime (doctor / startup), **T** telemetry.

| # | invariant | why it is the right check | level | where | cost | red-first test |
|---|---|---|---|---|---|---|
| I1 | resolved catalog is non-empty | the class's floor | C+U | `ResolvedCatalog` newtype returned by `chain.rs:165`; `schema.rs:94` already rejects empty payloads | ~40 LOC | inject an all-dropped asset → `resolve` cannot return an empty list |
| I2 | every provider has ≥1 model (referential integrity) | "provider without models" is an orphan row | U+R | `registry.rs:219-231` after load; warn at the choke point (ADR-0078 §4) | ~20 LOC | bundle with a `muse` entry → today silent (§4.2) → must warn |
| I3 | asset ids ⊇ compiled ids, per provider (superset) | the replacing source may not shrink the set | I | `bundle_catalog.py` gate; `assemble_artifacts.py:333` | ~1 s in packer | fixture asset → must fail with the missing-id list (§4.1) |
| I4 | hash total over sorted `(provider,id)` | a count can match under substitution | I | same gate, digest compare | ms | swap one id keeping the count → must fail |
| I5 | asset may not *lower* `contextWindow`/`maxTokens` below compiled without a flag | measured §4.3 (1/1 accepted) | I+R | packer + `registry.rs:588` merge | ms | asset with `contextWindow: 1` → must warn/refuse |
| I6 | every resolved model names its source layer | silence is the bug; "which snapshot am I on" must be answerable | R+T | `model_catalog.rs:104` payload; `track.rs:88-93` props; picker footer | ~60 LOC | assert the picker/doctor line names the layer |
| I7 | asset envelope matches content (counts, schemaVersion) | models have no `counts`; plugins do (`bundle_catalog.py:COUNTS_KEYS`) | I | packer | ms | hand-edit a count → must fail (plugins already covered) |
| I8 | `buildId` identifies content, not version | measured §5.2 | U+R | build.rs stamp; `clients.rs:126`; one classifier | ~60 LOC | forged `package.json` → must not be `current` |
| I9 | daemon and client agree on identity at the handshake | the incident's second half | U+I+Q | `daemon_client.rs:439`; `daemon.rs:31`; `mod.rs:416` | ~2 s per test | version-only classifier → red |
| I10 | installed artifact == published artifact | "a fix in the tree is not a fix installed" (AGENTS.md) | I | `install-rust.sh:2794-2836` + manifest `executableSha256`/`buildId` | ~50 ms (sha256 of 70 MB = 76 ms measured) | corrupt the staged binary → install must refuse |
| I11 | the asset the release ships is not a fixture | the release lane must be distinguishable from a smoke build | I | `release-fork.yml:127`; add an explicit `--release` mode instead of `--fixture` | ms | grep the tarball's asset for `fixture-` ids → must fail |

Deliberately **not** in the table (over-engineering for us): cross-release value drift baselines
(needs history we do not keep), LLM-judged "is this catalog good", per-model capability probes at
startup (network on the paint path — AGENTS.md forbids it).

## 7. Rust types: what is justified, what is not

**Justified (do it):**

1. `pub struct ResolvedCatalog { models: Vec<Model>, layer: CatalogLayer, dropped: Vec<Dropped>, replaced: usize }`
   with private fields and `models(&self) -> &[Model]`, `len()`, `is_empty()` returning `false`
   by construction (constructor rejects empty → falls back before returning). Only two production
   callers exist — `registry.rs:594` (the registry every surface reads) and
   `supervisor.rs:354` (the telemetry count) — so the type lands in ~40 lines with two call-site
   changes, no dependency. This is "parse, don't validate" applied once, at the boundary.
2. `pub struct BuildId(String)` with `Display`, `FromStr` (validated `pa-rs-[0-9a-f]{12}`),
   `PartialEq` — and *no* `From<String>` for versions. The type system then refuses
   `assert_eq(build_id, version)`, which is today's bug written as code.
3. `pub enum CatalogLayer { DiskCache, Bundled, Compiled, ForkTable, PrimeInferenceLive }` — a
   plain enum in the resolve result, *not* on the wire `Model` (parity rule: `pa-types::ai::Model`
   stays byte-compatible).

**Not justified (do not do):**

- A typestate `Catalog<Validated>` threaded through `pa-models` → every call site changes to buy
  what one non-empty constructor already guarantees. (Rust typestate is right when a *sequence* of
  operations must be ordered; here there is one boundary.)
- `NonEmpty<T>` from the `nonempty` crate (https://docs.rs/nonempty/latest/nonempty/): a new
  dependency for one type; hand-roll the 20-line version. Also it would spread through
  `get_available`/`get_discoverable_models` (which legitimately return empty for an
  unauthenticated user — an empty *auth-scoped* list is not an error, an empty *catalog* is).
  Getting this distinction wrong is how a "non-empty" refactor breaks the unauthenticated path.
- `CatalogSource` as a field on the wire `Model` (`pa-types`) → breaks TS parity and the
  `deny_unknown_fields` asset schema for no gain.
- A `ValidatedAsset` type in the packer's Python gate → the gate speaks JSON; the Rust type adds
  nothing there. Instead, make the *consumer* verify (§3.3).

## 8. Plan (ordered; each item names its red-first test)

**P0.1 — Make loss observable (no behaviour change).**
`pinning.rs:68-101` returns `(Vec<Model>, Vec<Dropped{provider,id,reason}>)`; `bundled.rs:77-104`
and `chain.rs:165-187` propagate it into `ResolvedCatalog` (§7.1). Red-first:
`a_bundled_entry_for_a_non_compiled_transport_is_reported_dropped` — today the muse entry
disappears with 0 signal (measured).

**P0.2 — Runtime invariants at the registry choke point.**
After `load_models` (`registry.rs:424-449`): assert fork coverage (`⊇ fork_provider_models()`,
already true via `:600`, now *checked*), warn once per process when any provider has 0 models, and
warn when the layer is not `DiskCache|Compiled` with the replaced-id count. ADR-0078 §4 shape:
warn, never throw. Red-first: the §4.2 fixture.

**P0.3 — Fix the packer gate's layer (unblocks both release lanes).**
Remove the fork-provider requirement from `validate_bundled_model_catalog` (the asset cannot carry
them — §4.2), replace it with I3+I4 computed against the compiled table, and add an explicit
`--release` mode to `bundle_catalog.py generate` so the fork lane stops calling `--fixture` for a
real release. The compiled facts come from a small checked-in `compiled_catalog_facts.json`
(`{providers: {p: [ids…]}, digest}`) regenerated by a `pa-models` example
(`cargo run -p pa-models --example catalog_facts`) — the robust form; note that a plain regex over
`models_generated.rs` already reproduces it today (I got 152 models / 32 providers / 42 non-PI ids
that way in one pass, and re-checked the 42 against the live fixture: all present), so the facts
file is not blocked on anything. Red-first: the 10 tests currently red must go green; a new test
must fail when one compiled provider disappears from the asset, and another must fail when one id
is swapped while the count is preserved (I4).

**P0.4 — Build identity end to end.**
`build.rs` in `pa-cli` (commit, dirty, target, profile, `rerun-if-changed=.git/HEAD`, honour
`SOURCE_DATE_EPOCH`); `buildId` = 12-hex digest of the record (§5.3) in `clients.rs:126`; one
classifier shared by `mod.rs:416`, `daemon.rs:31`, `daemon_client.rs:439`; manifest gains a
`buildId` row for binaries (`assemble_artifacts.py:496-503`) verified in `verify_release.py`
(next to `:196`); `install-rust.sh` compares it after the `--version` probe (`:2794-2836`).
Red-first: `same_version_different_build_is_not_current` (§5.5) — measured to fail today.

**P1.1 — `doctor` becomes a real health check.**
Sections: install (path, version, buildId, sha256 vs manifest), assets (presence, byte hash, model/
provider counts, fork coverage, layer), catalog (resolved layer + counts + drops), daemon (today's
table). Named checks selectable by argument (npm-doctor shape), `--json`, and **exit 1 on failure**
(expo-doctor's lesson). Pure predicates shared with the runtime (P0.2 of this plan, §3.9), so doctor cannot drift
from the code path.
Red-first: a staged install with the fixture asset must print `assets: FAIL (46 models, 0 compiled
ids)` and exit 1.

**P1.2 — Telemetry, primitives only.**
Extend `catalog_refresh` (`track.rs:88-93`) with `layer` (enum), `dropped` (count), `replaced`
(count), `fork_covered` (bool); add them to the versioned catalog (`pa-telemetry/src/catalog.rs`,
`SCHEMA_VERSION` at `:26`, rule rows near `:937`). Red-first: the sanitizer test that an unknown
enum falls back and the property stays a primitive.

**P1.3 — Picker/`/model` empty-state message.**
`get_model_catalog` payload (`model_catalog.rs:104`) gains `catalogHealth`; `render.rs:98-105`
renders it instead of the bare "No matching models" when the catalog itself is short. Red-first:
headless TUI test asserting the line names the layer and the replaced count.

**P1.4 — `proptest` properties** from §3.2 (five properties, ~250 lines total).

**P2.1 — Kill the duplicated schema.** Have the packer call the binary: `prime-agent model verify
--asset <path>` (consumer verifies producer, §3.3); delete `_MODEL_ENTRY_KEYS` or generate it from
Rust in the same test that regenerates `compiled_catalog_facts.json`.

**P2.2 — Install-time artifact verification** (I10): compare the unpacked binary's sha256 with the
manifest's `executableSha256` (76 ms measured). This is the cheapest way to make "the archive was
fine but the payload was swapped" impossible.

**P2.3 — Provenance** (SLSA-style attestation) only if artifacts leave GitHub releases.

## 9. Anti-patterns this report argues against

1. **A gate that fails a legitimate producer.** The current fork-provider gate fails `--network`,
   `--fixture`, both release lanes, and 10 of 24 packer tests (§4.4). An invariant that cannot be
   satisfied by a correct producer is a false positive.
2. **Two implementations of one fact.** Two daemon classifiers (§3.7, §5.2) and two catalog
   schemas (§4.5). Both are the same defect: the pair drifts, and the drift is silent.
3. **Doctor that cannot fail.** `run_reap` "always 0" (`stop.rs:44-46`) + `handled()`
   (`public_command.rs:71-73`): measured exit 0 on a stale daemon that is actively serving a
   session.
4. **Reporting a symptom as health.** `catalog_refresh` sends a count (`track.rs:88-93`); 46
   synthetic models is a "healthy" number.
5. **Version strings as identity.** Measured defeatable with one file (`package.json`) in both
   directions: it can claim a version the binary is not, and it can hide a build the version does
   not describe.
6. **Silent `continue` on a policy drop.** `pinning.rs:84-88` is correct policy (a catalog may not
   introduce a transport) with the wrong reporting (nothing).
7. **A UI that renders emptiness identically to a bad query** (`model_picker/mod.rs:793`).

## 9b. How to rerun every measurement (the exact commands)

All of these are read-only and hermetic; each creates its own `HOME`/`PI_PACKAGE_DIR` under a
temp dir and touches no live socket. `$T` is a fresh `mktemp -d`.

```bash
# A. The shipped asset is the fixture (byte-identical) and carries no real ids
python3 - <<'PY'
import json,sys; sys.path.insert(0,'scripts/release'); import bundle_catalog as bc
inst=open('/root/.local/libexec/models.bundled.json','rb').read()
fix=bc.fixture_catalog_bodies()['models.bundled.json'].encode()
print("identical:", inst==fix)                       # True
d=json.loads(inst); print(len(d['models']), "models,",
      len({m['provider'] for m in d['models']}), "providers,",
      sum(1 for m in d['models'] if m['id'].startswith('fixture-')), "fixture-* ids")   # 46 32 46
PY

# B. What a user with one credential sees (the incident, daemon-free)
mkdir -p $T/agent/sessions && echo '{"zai":{"type":"api_key","key":"x"}}' > $T/agent/auth.json
env -i HOME=$T PI_OFFLINE=1 PI_PACKAGE_DIR=/root/.local/libexec \
  PRIME_AGENT_CODING_AGENT_DIR=$T/agent PRIME_AGENT_SESSION_DIR=$T/agent/sessions \
  PATH=/usr/bin:/bin /root/.local/libexec/prime-agent model list

# C. Same, with the compiled fallback (no asset beside the payload)
env -i ... PI_PACKAGE_DIR=$T/empty-dir ... prime-agent model list    # real ids, 200K/131K

# D. Silent drop of a non-compiled transport (add a muse entry to a hand-written asset)
#    -> the muse row never appears, exit 0, no warning (§4.2)

# E. Value trust: an asset claiming contextWindow=1 for zai/glm-4.7 prints "1  1" (§4.3)

# F. The version-compare bypass (§5.2), 20 lines:
#    1) copy the OLD binary + package.json {"version":"0.10.2-beta.3"} into $T/pkg
#    2) $T/pkg/prime-agent --mode daemon --daemon-socket $T/iso.sock   (hermetic env)
#    3) /root/.local/libexec/prime-agent doctor --json                -> "status":"current"

# G. Build ids and costs
readelf -n /root/.local/libexec/prime-agent | grep 'Build ID'        # 247167fc…
readelf -n /root/.local/libexec.manual-old/prime-agent | grep 'Build ID'  # 07f78b3c…
time sha256sum /root/.local/libexec/prime-agent                     # ~76 ms / 70 MB

# H. The packer gate is red (uncommitted worktree state)
python3 scripts/release/bundle_catalog.py generate --fixture --out $T/a  # rc 0
python3 scripts/release/bundle_catalog.py verify --out $T/a              # rc 1 (fork providers)
python3 scripts/release/bundle_catalog.py generate --network --out $T/b  # rc 1 (same)
python3 scripts/release/test_catalog_assets.py                           # 10/24 FAIL

# I. The size comparison: compiled vs live vs fixture
#    compiled (regex over models_generated.rs RAW_MODELS): 152 ids / 32 providers, 42 non-PI
#    live fixture catalog.v1.json: 1197 ids / 31 providers; 42/42 compiled non-PI ids present
#    shipped asset: 46 ids / 32 providers; 0/42 compiled non-PI ids present
```

Note on 5.4/§7: I did not build (`cargo check`) — the shared `target/` and the disk made it
irresponsible; those two sections are design, and are marked as such.

## 10. Open questions / UNVERIFIED

- `json-schema-org/discussions/934` body not extractable (**content UNVERIFIED**); the
  no-foreign-key limitation is stated from the 2020-12 spec surface, not from the thread.
- ADR-0049 / 0054 / 0072 / 0077: referenced by ADR-0078's header, not fetched (0072's URL I tried
  404'd).
- Whether upstream's TS daemon sends a `buildId` at all (wire-compat question for adding one on our
  side): **UNVERIFIED** — `pa-types` carries it as an optional field, so adding a value is
  wire-compatible by shape, but I did not check the TS reference for a consumer.
- I did not compile or run any Rust test. For most of the session the box was at 99 % full
  (`/` 240 G used, 1.6 G free; `target/` = 78 G) because this checkout is shared by several agents;
  a build was not a responsible use of the last gigabyte, and the disk filled once from my own
  scratch cargo projects (six, ~2.5 GB, deleted). All Rust-side claims are therefore from reading
  the tree plus black-box runs of the installed binaries, the published tarball, and the Python
  release scripts — which is sufficient for every claim made here (each names the `file:line` that
  implements the behaviour or the command whose output was captured). The two claims that *would*
  need a compile are marked as design, not measurement: §5.4's busy-daemon policy and §7's type
  sketches.
- `readelf`-based build-id reading is Linux-specific; the Windows/macOS equivalents (PE debug
  directory, Mach-O `LC_UUID`) are **UNVERIFIED** here and are why the *stamped record* (build.rs)
  must be the primary identity and the platform note only a cross-check.
- The exact behaviour of `ensure_daemon_running` when the daemon is busy but same-version-
  different-build is designed by me from `daemon.rs:112-152`, not measured against a live busy
  daemon (I did not touch the running supervisor).
