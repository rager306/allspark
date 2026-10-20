# Archive saved in version control

Parent verification completed after the worker froze the archive allowlist.

- Sanitized archive commit: `1b41c7d1d` (replaces our own unpublished `0fb34004c`; no shared history rewritten).
- 213 explicitly staged archive paths; all reviewed paths were under `.research/`.
- Allowlist SHA-256: `04748e88a79267deb54c3ac2bef104922ce72e91b27827f409a8d6c46b1df16e`.
- Independent parent path, file-hash and known-secret-value checks: zero failures/matches.
- Source map: 310 rows, 255 exact and 55 sanitized. Sanitized captures are not original wire bytes.
- Exact source excerpts retain original whitespace/CRLF/SSE framing. The cached whitespace check warnings were classified, not normalized away.
- Generated TS binaries/packages, build outputs, process snapshots and runtime homes were not staged.
- The old commit remains local in reflogs until normal Git maintenance; it is not an ancestor of the replacement publication history. No credential was detected there, but its local session/environment metadata was intentionally not published.

## Cleanup boundary

The completed research evidence from `/tmp/roadmap_audit`, `/tmp/cc_verify` and the selected `/tmp/cx` artifacts is now preserved. Use `TMP-ARCHIVE-INVENTORY.md` for the per-path exclusions and replay limits.

Research/archive sources under `/tmp` have not been removed. A later CI headroom action removed only46 completed installer-test sandboxes matching `/tmp/pa-update-e2e-PID-counter`, after verifying dead owner PIDs and no active cwd/executable/open-fd references. It reclaimed9.64GiB; details in `ci-owned-fixture-cleanup.json`. No target, kernel, runtime, research, active socket or unrelated project data was cleaned. This receipt is not blanket authorization to clean `/tmp`.
