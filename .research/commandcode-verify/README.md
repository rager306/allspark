# commandcode-verify — Command Code repair-layer test extracts (2026-10-19)

Source: `/tmp/cc_verify/` (still present at audit; no cleanup was performed). Functions cut out of the
Command Code bundle (`.cjs`) to test the tool-call repair layers in isolation
(`repair.cjs` is the main harness; `lint.cjs`, `coerce.cjs`, `parse.js`,
`taste.cjs`, `reorg.cjs` are individual layers; `run.cjs` drives them).

- `c*.cjs` — extracted bundle functions under test.
- `o*.txt` — observed outputs of the runs (expected-behavior evidence).
- `proj/`, `proj2/` — tiny fixture repos (`.commandcode/taste/**`) the extract
  tests run against.

The audit verified all nonempty source/fixture/output files by SHA-256.
Thirteen empty stderr files were intentionally not copied (no content to lose).
