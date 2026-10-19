# New issue: `providerBackupModel` setting is parsed but has no reader in the Rust port — the TS v0.9.8 feature is silently ignored

**Status:** draft / not submitted. **Type:** bug (dead setting / feature regression on port). **Severity:** P3 (with #3312 adjacency → P2). **Area:** `crates/pa-core/src/settings/{load.rs,types.rs,manager.rs}`.

## Summary

The Rust settings layer lists `providerBackupModel` as a known field and deserializes it into `Settings::provider_backup_model`, but **nothing in `crates/` ever reads it**. In TS v0.9.8 this setting routes a failed turn to a user-defined backup model (`settingsManager.getProviderBackupModel()` → `_resolveBackupModel()` → `_handleBackupModelRetry`). The port silently drops the feature: a user who sets `providerBackupModel` gets no backup routing and no warning.

## Reproduction

1. Set `providerBackupModel` in settings (global or project):
   ```json
   { "providerBackupModel": "deepseek/deepseek-v4-pro" }
   ```
2. Make the primary provider fail with a retryable stream failure.
3. TS v0.9.8: the turn switches to the backup model (event `backupModel`). Rust port: nothing happens — the setting is parsed and then never consulted.

## Evidence (both eras)

| Era | File:line | Code |
|---|---|---|
| TS `v0.9.8` | `packages/coding-agent/src/core/settings-manager.ts:1124` | `const reference = this.settings.providerBackupModel;` — `getProviderBackupModel()` reader |
| TS `v0.9.8` | `packages/coding-agent/src/core/agent-session.ts:13339-13348` | `// User-defined backup model (settings.providerBackupModel …)`; `const backupModel = this._resolveBackupModel(); if (backupModel && !modelsAreEqual(this._runModel(), backupModel)) return this._handleBackupModelRetry(…)` |
| TS `v0.9.8` | `…/agent-session.ts:13508-13572` | `_resolveBackupModel` + `_handleBackupModelRetry` (switches model, clamps thinking, appends model change, emits `backupModel`) |
| TS tests | `test/suite/agent-session-retry-events.test.ts:1140,1184,1222`; `test/image-model-override.test.ts:24-26` | exercise `providerBackupModel` |
| Rust `v0.10.x` / `main` | `crates/pa-core/src/settings/load.rs:34` | `"providerBackupModel"` in `KNOWN_FIELDS` — parsed as a known field |
| Rust | `crates/pa-core/src/settings/types.rs:459` | `pub provider_backup_model: Option<String>,` |
| Rust | `crates/` (whole tree) | **no reader**: no `get_provider_backup_model()`, no consumer of `provider_backup_model` anywhere outside the settings struct/known-fields list |

Verified with `grep -rn 'provider_backup_model\|providerBackupModel' crates/`: only the two settings-definition sites appear. The Rust port has a **different** failover mechanism (`retry.failover`, `pa-core/src/session_engine/provider_failover.rs`, multi-candidate chain) that is broader than the single TS backup, but it does **not** consult `providerBackupModel` — so the TS setting is a no-op.

## Proposed fix

Two options:

1. **Wire it in** — add `SettingsManager::get_provider_backup_model()` and feed it into the failover candidate chain (or a first-hop preference) in `provider_failover.rs`, mirroring the TS `_resolveBackupModel` eligibility (backup must not equal the serving model; backup must accept image input when the episode carries images — TS `image-model-override.test.ts`).
2. **Deprecate loudly** — if the port intentionally replaced it with `retry.failover`, emit a one-time deprecation warning when `providerBackupModel` is present and point at `retry.failover`, so a TS-authorized settings file is not silently ignored.

The adjacency matters: `providerBackupModel` is the same code path as the `#3312` routed-image failover attribution, so whichever fix is chosen should be done alongside `#3312`.

## Impact

- A TS-authorized `settings.json` carrying `providerBackupModel` is silently ignored by the Rust port — the user believes failover-to-backup is configured when it is not.
- No warning, no error: pure silent feature loss on the TS→Rust port.
- Low individual severity, but a class of "parsed-but-unread settings" worth a sweep.

## References

- `.research/verify-ts-era.md` §3 ("`providerBackupModel` парсится в Rust-настройках, но читателя в `crates/` нет").
- `.research/verify-upstream-wire.md` #38 (adjacent failover-attribution issue).
- Upstream `#3312` (OPEN), `#3332` (ordered fallback chain, NOT_PLANNED).
