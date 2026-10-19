# CI recovery: run 39191633430

Status: auth/model-resolution fix source-stable; focused tests green except original full interactive suite (40 pass / 2 fail; no-auth fixture fixed targeted green, producer-rate WaitIdle starvation remains root-owned blocker). No full-suite pass claimed.

## Environment
- Working tree shared; edits limited to auth/model-resolution and named fixture tests.
- `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2`.
- Worker focused test evidence uses ambient stable Rust 1.99; root owns the final CI Rust 1.98.1 gate. Focused green evidence does not replace that gate.
- Disk before: `/dev/sda1 242G total, 197G used, 45G available (82%)`.
- Disk after focused builds: `/dev/sda1 242G total, 205G used, 37G available (85%)` (shared parallel builds; no deletion performed).
- No model endpoint called, no credential/config changed, no commits/pushes.

## CI evidence
- `env -u GH_TOKEN -u GITHUB_TOKEN gh run view 39191633430 --json conclusion,jobs,url` defaulted upstream, returned 404.
- Correct repo: `env -u GH_TOKEN -u GITHUB_TOKEN gh run view 39191633430 --repo rager306/allspark --json conclusion,jobs,url`.
- `env -u GH_TOKEN -u GITHUB_TOKEN gh run view 39191633430 --repo rager306/allspark --log-failed > .research/ci-39191633430-failed.log` (exit 0).
- Auth regression: default print selected opencode/big-pickle instead of fixture; no-auth print unexpectedly selected bundled model; 3 interactive picker count failures; daemon configuredProviders falsely included opencode; 2 daemon startup/refused switch failures.
- Other failures handed root: pa-edit unknown clippy lint; pa-mem float comparisons; prompt guards; ctrl-S termios launch; shard1 pinned TS lifecycle comparison.
- Downloaded log checked for obvious authorization/token markers; none found. Logs must still receive repository-wide secret scan before commit.

## Design
- Configured auth remains configured-only. Public free-tier capability must not become a credential candidate or environment API key.
- Explicit native free models remain selectable, resumable, runnable; paid opencode models and custom endpoints are not implicitly authenticated.
- Predicate is scoped to existing model provider `opencode`, API `openai-completions`, native base URL `https://opencode.ai/inference/v1`, and `-free` IDs. Existing URL/protocol constants unchanged.
- No dependency or cross-crate re-export introduced. Documented model-level registry method is a new public API (review required), owned by pa-core models/auth scope.

## Validation
- Original production mutation restored HEAD `env_api_keys.rs` + `auth/manager.rs` while retaining new regression.
- `cargo test -p pa-core --lib native_free_models_do_not_configure_a_provider_or_enter_default_candidates -- --exact models::registry::tests::native_free_models_do_not_configure_a_provider_or_enter_default_candidates > .research/ci-auth-original-red.log 2>&1` exited **101**: assertion `!registry.auth.has_auth("opencode")` failed. Original regression reproduced. Final files restored immediately afterward.
- Same original regression binary (built by the command above) retained the explicit native test. `target/debug/deps/pa_core-4474e33bf9917dea --exact models::registry::tests::explicit_native_free_selection_resolves_public_request_auth_only_on_its_endpoint > .research/ci-auth-explicit-original-red.log 2>&1` exited **101**: request auth was `None`, expected `Some("public")`. This proves preservation of explicit native request auth; no second source mutation needed.
- Original production source restoration completed. Final code source-stable ping sent to root.
- Focused green commands (logs named accordingly):
  - `cargo test -p pa-core --lib native_free`
  - `cargo test -p pa-daemon --lib model_resolution`
  - `cargo test -p pa-daemon --lib model_catalog_and_available_models_match_the_ts_shapes`
  - `cargo test -p pa-daemon --test model_catalog_e2e`
  - `cargo test -p pa-cli --lib`
  - `cargo test -p pa-cli --test interactive_daemon_e2e`
- Core native-free green: `cargo test -p pa-core --lib native_free` exited **0**; 4 passed, 0 failed (registry no-default-pollution, explicit scoped public request auth, configured credential preservation, saved native-free restore). `.research/ci-native-free-green.log`. Daemon model_resolution **24 passed**, daemon catalog shape **1 passed**, CLI lib **311 passed**. Daemon catalog E2E **1 passed**. Original full CLI interactive suite exited **101**, **40 passed / 2 failed**: no-auth preflight fixture (fixed and focused green below) and producer-rate WaitIdle starvation (not fixed; root notified). Original picker-count regressions all passed. No full suite result claimed.

## Source stabilization
- New documented API: `ModelRegistry::has_request_auth(&Model)`, configured auth OR eligible native free endpoint. `has_configured_auth` and `get_available` remain configured-only, preserving startup defaults, failover candidates, and provider-list semantics.
- Explicit CLI resolution preserves the existing available list (including private entitlement filtering), then appends eligible unconfigured native free entries. It does not broadly resolve all unauthenticated catalog models.
- Registry explicit set_model, model can-use validation, saved session restore, daemon turn preflight, and request key resolution reuse the model-level rule.
- Parsed native URL checks HTTPS, exact host `opencode.ai`, default HTTPS port 443, `/inference/v1` path (trailing slash allowed), no credentials/query/fragment, matching API and `-free` model ID. These are existing native endpoint values, not changed production constants.
- No production constants changed. Child-process credential scrub added missing AWS container/web identity variables (test fixtures only).
- Ownership extension approved by parent: `pa-cli/src/print_runtime.rs`, `pa-cli/src/print_model_tests.rs`, `pa-core/src/models/session_restore.rs`, one preflight condition/comment in `pa-daemon/src/agent_engine/turn/model.rs`.
- `rustfmt --edition 2021 <owned changed Rust files>` and scoped `git diff --check` passed. Full gate remains root-owned.


## Test isolation and limits
- Removed old in-process `std::env::remove_var` loops; async/threaded tests never modify shared process credentials.
- Allowlist startup explicitly pins fixture default; switch test explicitly pins fixture initial selection. These tests prove allowlist behavior, not default provider order.
- Getter configuredProviders assertion remains whole-object exact equality; expected set independently includes ambient configured providers plus fixture provider, not a snapshot magic list or presence-only assertion.
- E2E daemon child processes alone scrub provider key/ADC/AWS vars; parent process environment is untouched.
- Engine native test follows actual engine model/request-auth resolution with empty temp auth; expected ambient key is respected if one exists. Hermetic core tests prove exact no-env public fallback.
- No live native endpoint request was made intentionally; no paid credentials used. New scope is auth/model-resolution, not wire transport.
- Additional no-credentials TUI fixture defect reproduced: `target/debug/deps/interactive_daemon_e2e-575b6b5b2956a928 --exact tui_flagged_model_turn_reports_the_ts_preflight_error_without_credentials --nocapture > .research/ci-interactive-preflight-diagnostic.log 2>&1` exited **101**; TUI forwarded client ambient Google credentials on create, undoing supervisor-only scrub, and selected Vertex instead of sign-in refusal. This was not the opencode production fix.
- Fixed that one no-auth test with isolated child test process (`env_clear`, fresh temporary HOME, PATH retained, test-only `PA_TEST_PREFLIGHT_WITHOUT_CREDENTIALS=1` recursion marker). No in-process environment changes, no production behavior change, no new production constants. Red diagnostic above is the regression proof.
- `cargo test -p pa-cli --test interactive_daemon_e2e tui_flagged_model_turn_reports_the_ts_preflight_error_without_credentials -- --exact > .research/ci-interactive-preflight-isolated-green.log 2>&1` exited **0**: isolated inner test and wrapper passed (one logical test).
- Daemon focused command group completed exit **0**: model-resolution 24 passed; catalog shape 1 passed; catalog E2E 1 passed. Logs `.research/ci-daemon-*-green.log`.

## Backlog not fixed here
- Existing pa-ai OpenAI-completions native transport unconditionally rewrites `opencode` Authorization to public, including paid/custom endpoints. This predates this fix. Correct it separately with provider-wire tests covering native endpoint public vs custom/paid credential preservation. Registry endpoint-override guards here do not claim transport correctness.
- Picker discovery uses the full catalog and explicit selection works through the registry, but its UI label is provider-configured-only: native free entries can say `require sign in` despite explicit keyless selection succeeding. Avoid lying in configuredProviders; a future model-level capability label should fix this separately.
