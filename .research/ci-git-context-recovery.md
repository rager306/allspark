# Git-context bare-SSH fixture recovery

Scope: only `crates/pa-core/tests/git_context.rs`, failing `keeps_ssh_remote_url_verbatim_when_unnormalizable` test. Source frozen. No production/API/dependency/config/identity changes. Existing throwaway temp-repo identities remain unchanged.

## Root cause
`capture_git_context` intentionally invokes real `git remote get-url origin`, which applies Git `url.*.insteadOf` rewrites. Production behavior is correct and still honors real user config. The test assumed a bare SSH URL without owning ambient Git configuration.

Original root full gate failed **12 passed / 1 failed**, SSH expected vs rewritten HTTPS actual. Historical full raw log is local-only `.research/ci-fullgate-1.98.1-git-context-red.log` (do not commit raw frames). SHA-256: `2e07e40ab8cf2d85b8745f1d75488c9655f799c6f191ae7a234fa6dbb72695fa`.

## Red-first verifier
Created standalone safe fixture `.research/git-context-rewrite-fixture/gitconfig`:

```gitconfig
[url "https://github.com/"]
    insteadOf = git@github.com:
```

`PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/root/prime-agent/.research/git-context-rewrite-fixture/gitconfig cargo +1.98.1 test -p pa-core --test git_context keeps_ssh_remote_url_verbatim_when_unnormalizable -- --exact > .research/ci-git-context-planted-red.log 2>&1`

Original source exited **101**: exactly the existing assertion, actual `https://github.com/acme/widgets.git`, expected `git@github.com:acme/widgets.git`. One run, no timeout/deadlock or retry-to-green.

## Fix boundary
Only this bare-SSH fixture re-execs its exact test in a child process with `env_clear`, fresh temporary HOME, inherited PATH, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=<fresh HOME>/gitconfig` pointing to its own empty file (portable; no `/dev/null` dependency), and a test-only recursion marker `PA_TEST_GIT_CONTEXT_BARE_SSH=1`. Existing inner repository creation, real capture call, and SSH assertion are unchanged. No in-process environment mutation, global config write, or local identity rewrite rule masking real Git semantics. No production constants changed.

## Green checks
One queue, same explicit Rust 1.98.1 build environment:
- Exact same parent planted-config command after fix: **exit 0, 1 passed / 0 failed** (`.research/ci-git-context-planted-green.log`).
- Entire `cargo +1.98.1 test -p pa-core --test git_context` under the same planted parent config: **exit 0, 13 passed / 0 failed** (`.research/ci-git-context-binary-green.log`).
- `cargo +1.98.1 fmt --all --check`: **exit 0** (`.research/ci-git-context-fmt.log`).
- Parent portability review found the fixture is not Unix-only. Replaced `/dev/null` with its own empty config file in the existing temporary HOME. Exact planted-config test plus fmt rerun once for this source adjustment: **exit 0, exact 1 passed / 0 failed; fmt exit 0** (`.research/ci-git-context-portable-green.log`, `.research/ci-git-context-portable-fmt.log`). This is portability validation, not retry-to-green.

No workspace-green claim. Root owns final full native gate after explicit Cargo-slot release. Target/cache cleanup is not performed by this worker.
