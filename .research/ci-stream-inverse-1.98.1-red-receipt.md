# Producer-rate regression: controlled inverse red receipt

- Compiler: Rust1.98.1, test profile unoptimized; DEV/TEST_DEBUG=0, BUILD_JOBS=2.
- Mutation: only private `autolink::bare_candidate` implementation restored original four-prefix loop. Semantic candidate unit retained. No test/barrier/time/budget changed.
- Command: `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo +1.98.1 test -p pa-cli --test interactive_daemon_e2e tui_big_streamed_turns_render_at_the_producer_rate -- --exact`
- Exit:101 (runtime regression assertion, not compile failure).
- Assertion: `a WaitIdle barrier expired; the render starved behind the stream`.
- Counts:0 passed;1 failed;0 ignored;0 measured;41 filtered out.
- Runtime:94.40s.
- Original full-frame log (local only): `.research/ci-stream-inverse-1.98.1-red.log`.
- Original log SHA-256: `9566d4ebdb47f424f349fc56a52f1519749d2d8bbef4d1f2ad6ac9f4821e6f6e`.
- Full-frame transcript omitted from this sanitized receipt. Source restored exactly afterward; equality to saved patched source asserted; SOURCE_STABLE sent.

## Restored green
- Same compiler, command, test and45s barriers after exact source restoration.
- Exit0;1 passed;0 failed;41 filtered out;40.68s.
- Green log: `.research/ci-stream-restored-1.98.1-green.log`.
- Green log SHA-256: `1bcfe97ed5e584417a267f640aeeb1648f6febd6f2090a3441bd6c95f6f3f124`.
- Separate autolink semantic/rendered-byte tests:9 passed;0 failed (Rust1.98.1).
