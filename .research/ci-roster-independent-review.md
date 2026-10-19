# Independent review: roster fixture ledger isolation

## Verdict: source-clean; focused compile/test evidence confirms the correction

No concrete blocker remains in the reviewed four-file test-only change. A fresh read matched the source hashes below after completed focused native results. The earlier privacy compile blocker is lifted for this corrected snapshot. **Full pa-daemon library tests, format, all-target lint, and full workspace gates are not certified here**; root/worker still own their pending queue.

## Actual final diff and ownership

- The one existing `roster_fixture` moves from sibling seed tests into `supervisor::tests`, a descendant of the module defining `Supervisor`. Its private `rlm_ledger` access is now valid. `Supervisor`'s production field remains private. The only module visibility change is the existing `#[cfg(test)] pub(crate) mod tests`; no production API is widened.
- The moved factory retains its existing owned temporary dirs, real transcript contents, and return shape. Its only body changes are qualifying the existing seed-test display writer, cloning the owned agent dir before moving it into options, and caching the owned-dir ledger. Seed/passivation tests use direct imports. There is no re-export shim, accessor, duplicate factory, or new helper.
- The adoption fixture wraps its existing ledger in `Arc` and caches a clone before its unchanged append. The shared fixture append helper uses another handle for the same owned ledger **file**, not the same Arc identity; real stat-guarded replay remains exercised.
- The saved-delete test adds only a case-specific outer re-exec guard: current test binary, the exact full test name, child-only removal of `crate::paths::SESSION_DIR_ENV`, recursion marker, checked child status, and return. The entire original child body—from `roster_fixture().await` through every action, assertion, and cleanup—is byte-identical to HEAD. All other original test assertions remain unchanged. The new assertion checks child success.
- Whole-file reconstruction against HEAD matched all four diffs exactly: that fixture move/injection, adoption injection, test-module visibility/imports, and the saved-delete outer guard. Passivation retains its existing test gate; seed production code before `#[cfg(test)]` is byte-identical. Relevant production path/cache/ledger/saved-delete files have empty working-tree diffs.

This stays within pa-daemon's existing supervisor/roster/ledger test ownership. No dependency, production API, shared vocabulary, runtime behavior, global environment mutation, or runtime timing budget changes. The new test-only constant `PA_ROSTER_SAVED_DELETE_ISOLATED_TEST` prevents child recursion.

## Why the owned fixture is isolated

`supervisor/notes.rs:199-219` resolves the default session path first. For `None`, requested is a clone of that same path, so the non-default branch is not taken. It returns the injected Arc before constructing another default-path ledger/log. `paths::sessions_dir`/`expand_tilde` only select/expand the path; they do not open or walk that selected tree. Supervisor config remains written under the owned temporary descriptor dir. This does not suppress default-path resolution or promise to avoid errors from an invalid tilde/home setting.

The saved-delete writer is separate: `saved_session_commands.rs:638-671` resolves the session path, writes the tombstone through another ledger, and refreshes roster usage through the cache. Cache injection alone therefore did not isolate it. Removing the override on this exact test's child makes both writer and reader use its owned sessions dir while preserving the original real deletion/usage/push oracle. The parent environment is untouched. No actual override value or user session was inspected by this reviewer.

## Completed focused evidence inspected

Only safe compiler/result summaries were read; no raw frames or user data are reproduced.

- `ci-roster-isolated-filter-green.log`: compile finished in 2m 40s; nested saved-delete child **1 passed / 0 failed**, 0.05s; outer roster filter **25 passed / 0 failed / 1 ignored**, 1144 filtered out, 4.31s.
- `ci-roster-isolated-adoption-green.log`: exact adoption **1 passed / 0 failed**, 1169 filtered out, 0.02s.

These actual completed results confirm the corrected test target compiled and the guarded original saved-delete oracle ran. They do not imply full-library or all-target-lint success. This reviewer ran no Cargo/build/test/format commands.

## Earlier rejected snapshots and reviewer miss

The first two-file injection snapshot failed to compile with **E0616**: sibling seed tests could not access the private `Supervisor.rlm_ledger` field. This reviewer missed that Rust privacy boundary and initially called its source clean. That earlier approval and its `7c2c...`/`a9af...` hashes are obsolete; restoration of those bytes did not make them valid. The compiler error summary was independently inspected, and no tests ran after that failed compile. Relocating the existing factory into the owner test module corrected the boundary without widening production state.

The first corrected relocation compiled but produced **24 passed / 1 failed / 1 ignored**, 5.97s. The saved-delete case still used the environment-selected writer ledger and failed its one-push oracle. That was a real remaining fixture mismatch, not a timing finding or a green run. The final child guard and actual focused results above supersede this partial result.

Historical 11 failures and controlled owned-override baseline reds are root/worker evidence; this reviewer did not execute those checks. Source confirms the mismatch mechanism, but this report does not inspect the historical override value or certify every historical cause.

## Final source freeze (SHA-256)

Freshly re-read after completed focused evidence, then hash-matched again before this report:

| Source | SHA-256 |
|---|---|
| `crates/pa-daemon/src/supervisor.rs` | `e3728ec8badadc71c3f79ccdbdba9cd22bbf980d514ba26f03ea972ee6239f5d` |
| `crates/pa-daemon/src/supervisor/tests.rs` | `5c914a6ec53a8c4a6956458b5799883544c9e1803bf9ecb8ba50d2c3483f24ae` |
| `crates/pa-daemon/src/supervisor_roster_seed.rs` | `e46ff752f5ca93e48060b497c5d2318b6ec166b9f3c75f8109c5ddc6e7c13785` |
| `crates/pa-daemon/src/supervisor_roster/passivation.rs` | `9d4846e17134467030e25ae5d2f00731632bd6f343c7f7c2d47377272cc26b9f` |

Scoped native log receipts:

| Log under `.research/` | SHA-256 |
|---|---|
| `ci-roster-isolated-filter-green.log` | `1207dcdcb646f5a03e47cdd1a0f71c8a6dc7a3b4f9b9c756c1ed898a42366618` |
| `ci-roster-isolated-adoption-green.log` | `8c347c542c7363a17caab46cff45025f8a06747b5ab72d60c27613942d4f217c` |

Only this requested review report was written. No source/staging/commit/push/environment changes or cleanup were performed by the reviewer. Final root/worker gates remain separate.
