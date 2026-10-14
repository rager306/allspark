Краткий аудит `crates/pi-edit` hashline для Prime edit. Заметка: `/tmp/prime-architecture-research/oh-my-pi-hashline-edit-audit.md`.

Допущение по “token cost”: трактую как model-token overhead механизма и диагностик, не как CPU cost.

## 1) Preflight multi-section / atomic до первого write

Claim: hashline сначала полностью stage-ит все секции, валидирует write-policy, и только потом пишет. Это ловит stale/duplicate в поздней секции без частичной записи ранней секции.

Code-evidence:
- `crates/pi-edit/src/session.rs:274-286` документирует порядок: reread targets → stage all files in memory → enforce guard → write.
- `crates/pi-edit/src/session.rs:298-305` вызывает `engine.stage(...)` и `enforce_write(...)` для всех staged до write-loop.
- `crates/pi-edit/src/session.rs:309-324` write-loop начинается только после stage/enforce.
- `crates/pi-edit/src/modes/hashline/patcher.rs:593-672` `stage_patch` проходит все `patch.sections`, читает/resolve-ит, проверяет tag и вызывает `apply_with_recovery` in-memory.
- `crates/pi-edit/tests/hashline_parity.rs:737-750` regression: `a.ts` валиден, `b.ts#FFFF` stale; apply error и оба файла остаются исходными.

Class error → mechanism → Prime:
- Error class: multi-edit/multi-file payload partially writes before a later target fails.
- Mechanism: separate `stage` from `write`; validate all targets against fresh reads and policy first.
- Prime: current exact edit is single-file, but already has same pattern внутри файла: `crates/pa-core/src/tools/edit_diff.rs:239-322` validates all `edits[]`, duplicates, overlap and no-op before returning `new_content`; `crates/pa-core/src/tools/edit.rs:247-288` reads/applies then writes once. If Prime adds multi-file edit/batch later, copy the hashline pattern: `Vec<StagedFile>` + all canonical/path/tag/guard checks before first `write_file`.

## 2) Stale-tag rejection + recovery boundary

Claim: hashline requires a per-file snapshot tag and rejects when live file hash differs unless a narrowly safe recovery applies.

Code-evidence:
- Prompt contract: `crates/pi-edit/prompts/hashline.md:1` requires `[PATH#TAG]`; line numbers are from original `LINE:TEXT`.
- Missing tag rejected at `crates/pi-edit/src/modes/hashline/patcher.rs:603-607`; message text at `crates/pi-edit/src/modes/hashline/messages.rs:721-728` says use latest read/search tag.
- Live-vs-expected hash check is `file_hash(normalized).eq_ignore_ascii_case(expected)` at `crates/pi-edit/src/modes/hashline/patcher.rs:435-437`.
- On mismatch, anchor-scoped edits try retained-snapshot recovery at `crates/pi-edit/src/modes/hashline/patcher.rs:485-500`.
- Recovery itself only uses a retained snapshot for that path/tag, remaps edits, checks preserved context, applies to current text, and rejects no-op: `crates/pi-edit/src/modes/hashline/recovery.rs:371-396`; warning choice at `:398-405`. Warning constants at `crates/pi-edit/src/modes/hashline/messages.rs:681-695`.

Class error → mechanism → Prime:
- Error class: stale read, copied tag from sibling/worktree, or post-read external edit can make an apparently valid edit hit the wrong place.
- Mechanism: read/search mints content hash; edit must cite it; edit compares live hash before mutation; if mismatch, either safe-rebase or hard reject with actionable message.
- Prime: exact `oldText` matching reduces but does not eliminate stale risk: stale `oldText` may still uniquely match the wrong copy. To get true stale rejection, Prime needs an optional or required read revision/hash in the edit input, tied to read/search provenance. Without that schema/provenance change, Prime can only improve “not found/duplicate/no-op” diagnostics; it cannot detect “oldText still matches but from stale read”. If schema cannot change, do not pretend to have stale-tag rejection.

## 3) Mismatch diagnostics: recognized stale vs unknown/wrong-session tag

Claim: hashline diagnostics distinguish “known tag but file changed” from “tag not from this session / wrong file”, and show local anchor context.

Code-evidence:
- `crates/pi-edit/src/modes/hashline/patcher.rs:97-125` computes actual hash, records current snapshot, finds other paths with the expected hash, and builds `MismatchDetails` with anchor lines.
- `crates/pi-edit/src/modes/hashline/mismatch.rs:80-89` message for recognized hash: file changed; use new header from prior edit response or re-read.
- `crates/pi-edit/src/modes/hashline/mismatch.rs:90-110` message for unknown hash: not from session; names origin paths if known elsewhere; never invent/reuse prior-session tag.
- `crates/pi-edit/src/modes/hashline/mismatch.rs:113-118` appends anchored context.
- Context size is fixed: `MISMATCH_CONTEXT = 2` at `crates/pi-edit/src/modes/hashline/messages.rs:45-46`; context collection around anchors at `:48-63`.

Class error → mechanism → Prime:
- Error class: generic “old text not found” makes agents widen payloads, retry stale context, or edit wrong worktree.
- Mechanism: classify cause and include bounded current context near intended anchors.
- Prime: extend `get_not_found_error` / `get_duplicate_error` near `crates/pa-core/src/tools/edit_diff.rs:196-221` with bounded diagnostics. If edit input carries read hash/line span, distinguish stale-file vs wrong-session hash. If not, add safer next-step text: “re-read exact range; do not widen oldText”, and for duplicates show occurrence count (already done) plus a bounded line preview if cheap.

## 4) Seen-lines guard for summarized/partial reads

Claim: hashline tracks which lines were actually shown to the model and rejects edits anchored to hidden/folded/out-of-window lines.

Code-evidence:
- Snapshot stores `seen_lines`: `crates/pi-edit/src/store.rs:27-39`.
- Recording snapshots unions seen lines and deduplicates same content: `crates/pi-edit/src/store.rs:210-245`; explicit `record_seen_lines` at `:263-275`; `merge_seen` at `:439-443`.
- Guard entry: `crates/pi-edit/src/modes/hashline/patcher.rs:349-369` loads snapshot by content and filters anchor lines not in `seen`.
- If unseen, it reveals bounded actual content, records revealed lines when not truncated, then errors: `crates/pi-edit/src/modes/hashline/patcher.rs:385-423`.
- User-facing message explains partial/search/folded summary and retry path: `crates/pi-edit/src/modes/hashline/messages.rs:802-849`.
- Regression: `crates/pi-edit/tests/hashline_parity.rs:964-986` rejects edit on line 3 when snapshot only saw line 2, accepts line 2.

Class error → mechanism → Prime:
- Error class: model edits a line number from a folded summary/search context it never actually saw; this can mangle nearby code.
- Mechanism: read/search provenance is not just hash; it includes displayed line set. Edit validates target span against seen set.
- Prime: for exact edit, compute matched `oldText` line span after `apply_edits_to_normalized_content` finds the unique match. If the edit cites a read revision, require every touched line to be in that revision’s `seen_lines`. If no provenance exists, this guard cannot be exact. Add a read-provenance store at session/tool layer, not inside pure string diff code.

## 5) Seen-lines shift rescue, but only with strong evidence

Claim: hashline has a narrow exception for follow-up edits after earlier edits shifted line numbers; it rejects stale numbers unless payload content proves the current target.

Code-evidence:
- Version maps from retained snapshots to current text: `crates/pi-edit/src/modes/hashline/patcher.rs:200-245`.
- Unique evidence rejects short/repeated lines: `crates/pi-edit/src/modes/hashline/patcher.rs:247-264`; min length constant is `SHIFT_RESCUE_EVIDENCE_MIN_CHARS = 8` at `:127-130`.
- Range evidence must carry first and last current lines in body: `crates/pi-edit/src/modes/hashline/patcher.rs:286-310`.
- Rescue applies only when all unseen anchors image previously seen lines and body evidence passes: `crates/pi-edit/src/modes/hashline/patcher.rs:312-346`; called by the seen-lines guard at `:373-383`.

Class error → mechanism → Prime:
- Error class: strict seen-lines guard can false-reject a valid second edit after prior edit shifted displayed lines.
- Mechanism: use retained versions + diff map + unique line evidence; do not accept pure inserts/cuts or duplicate/short evidence.
- Prime: mostly unnecessary for exact `oldText` because target is content-based. If Prime adds line anchors/revision tags, copy this only after the basic seen-lines guard exists. Do not add fuzzy “nearby line” rescue to exact edit; it would recreate wrong-location edits.

## 6) Duplicate canonical targets / same-path merge guards

Claim: hashline prevents ambiguous multi-section targeting of the same real file and catches conflicting tags for same authored path.

Code-evidence:
- Same authored path sections coalesce; conflicting tags reject: `crates/pi-edit/src/modes/hashline/input.rs:425-455`.
- Resolved canonical duplicates reject before staging more writes: `crates/pi-edit/src/modes/hashline/patcher.rs:642-649`.
- Undecodable delete path has the same canonical duplicate guard: `crates/pi-edit/src/modes/hashline/patcher.rs:563-582`.
- Regression: duplicate `a.ts`/`./a.ts` rejects and file stays unchanged at `crates/pi-edit/tests/hashline_parity.rs:752-760`.

Class error → mechanism → Prime:
- Error class: two authored paths resolve to one file, or same path uses two stale tags; order becomes ambiguous.
- Mechanism: canonicalize targets during preflight and reject unless ops are intentionally merged.
- Prime: current tool takes one path, so mostly already avoided. For future batch/multi-file path, canonicalize before staging. For current `edits[]`, the equivalent is overlap detection; Prime already has it at `crates/pa-core/src/tools/edit_diff.rs:293-307`.

## 7) Extra edit-path guards worth porting

Claim: hashline has several small guards that prevent retry loops or hidden drift.

Code-evidence:
- No-op loop hard limit: `NOOP_HARD_LIMIT = 3` at `crates/pi-edit/src/store.rs:24-25`; no-op handling/error in `stage_patch` at `crates/pi-edit/src/modes/hashline/patcher.rs:716-735`.
- Plan/write guard is enforced for every staged file before write: `crates/pi-edit/src/session.rs:298-305`; policy hook in `crates/pi-edit/src/files.rs:194-199`.
- Write drift warning for editor/IDE format-on-save is described at `crates/pi-edit/src/modes/hashline/messages.rs:704-715`; session compares recorded write vs intended text at `crates/pi-edit/src/session.rs:343-357` (post-write snapshot re-key path continues there).
- New-file misuse rejected in hashline with “Use the write tool”: `crates/pi-edit/src/modes/hashline/patcher.rs:634-637`; same message helper at `crates/pi-edit/src/modes/hashline/messages.rs:723-728`.

Class error → mechanism → Prime:
- Repeated identical no-op → per-session `(canonical_path, payload_hash)` counter; after 3, hard stop with “re-read or move on”. Prime currently has one-shot no-change errors at `crates/pa-core/src/tools/edit_diff.rs:230-238`, but no cross-call loop memory.
- Policy bypass → keep/extend Prime’s write authorization before `write_file`; any future multi-stage path must enforce all guards before first write.
- Write drift → if Prime ever writes through an editor/LSP-backed writer, read/compare actual persisted content and warn/re-key provenance. With local `std::fs::write` (`crates/pa-core/src/tools/edit.rs:92-93`) this is lower value.

## Token cost

- Steady prompt cost of full hashline syntax is non-trivial: `crates/pi-edit/prompts/hashline.md:1-30` is roughly 350-450 model tokens. Do not port the whole syntax to Prime exact edit unless changing the tool contract intentionally.
- Per-edit tag overhead is tiny: `[PATH#TAG]` uses a 4-hex tag (`prompts/hashline.md:1`), usually ~1-3 extra tokens beyond the path/header.
- Mismatch diagnostic cost is bounded by context: ±2 lines per anchor (`messages.rs:45-63`) plus ~80-120 tokens of explanation.
- Seen-lines failure can be expensive by design: reveal cap is 40 lines × 512 chars (`patcher.rs:33-34`), worst-case about 20 KB ≈ 5k-6k tokens plus framing. Common case is one/few lines.
- State/memory cost, not model tokens: retained snapshots default to 256 paths, 4 versions/path, 64 MiB total (`store.rs:14-21`).
- Prime recommendation for low token cost: do not add hashline grammar. Add optional revision/provenance fields and compact diagnostics to existing exact edit. Prompt/schema overhead can stay ~50-150 tokens; failure diagnostics should reveal at most a small bounded context unless the model must verify hidden lines.
