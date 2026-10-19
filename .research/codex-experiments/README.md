# codex-experiments — OpenCode mock-provider regression fixtures (2026-10-19)

Source: `/tmp/cx/` (still present at audit; no cleanup was performed). Captured runs against a local mock
provider (`mock3.py`) emulating the OpenAI Responses API for model `gpt-6-astra`.

- `mock3.py` — the mock provider used to drive the runs (regression tool).
- `instr_gpt6astra.md` — full instructions_template sent to the model (21,428 chars, reference snapshot).
- `reqs2/req_1.json` — sanitized captured request; tool schemas and synthetic scenario are preserved, but this is **not** a byte-level wire reference.
- `plan_*.json` — scripted tool-call plans per scenario (input side).
- `sc_<name>/req_N.json` — captured request payloads for each scenario run
  (errs, patch, trunc, loop, parpatch, atomic, dup, deny, cap, bg, shell, stale,
  timeout, schema, misc, wrongtool, afterexit, patch2, trunc2, deny2).

Reproduce: run `mock3.py`, feed the `plan_*.json` scenarios, diff captured
`req_N.json` against these fixtures.

## Archive privacy and reproducibility boundary (2026-10-19 audit)

All 54 archived `req_*.json` were sanitized after the initial archive commit:
- removed top-level `client_metadata` (including nested turn metadata, session,
  installation, window, thread and turn identifiers) and `prompt_cache_key`;
- removed transport `id` on all input records; `call_id` and tool outputs
  remain because they connect the scripted scenario;
- replaced local `<skills_instructions>` inventory and `<environment_context>`
  text with explicit archive-redaction markers. The synthetic cwd `/tmp/cx`
  and bash shell are documented; local timezone/date/home skill paths are not.

No HTTP headers were captured in these JSON bodies. `mock3.py` writes only parsed
request bodies, not auth headers. Credential-pattern and current-secret-value
scans found no credentials; this is a bounded audit, not proof of absence.
Source/destination SHA-256 pairs are in `../TMP-ARCHIVE-INVENTORY.md`.
The original private captures remain in `/tmp/cx` until the user deletes them;
they must not be committed again. Original captures are also present in ancestor
commit `0fb34004c`; sanitizing the working tree does not rewrite git history.

These copies can replay the synthetic tool/scenario content, but cannot prove
exact wire bytes, cache-key behavior, local prompt inventory, or session metadata
parity. The scripts still assume their original `/tmp/cx` working directory;
restore that synthetic fixture path or adapt the scripts before rerunning.
`source-extracts/` contains inspected upstream code snapshots, not a whole clone.
`fixtures/` contains final observed tiny file states, not guaranteed preimages.
