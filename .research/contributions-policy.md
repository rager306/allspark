# Allspark fork: contribution policy for PrimeIntellect-ai/prime-agent

## The process (from CONTRIBUTING.md, confirmed by code and repo)

1. You CANNOT open a direct Issue (auto-close + redirect to Discussions)
2. You CANNOT open a PR without an invitation (auto-close)
3. Discussion [Bug reports] / [Feature requests] / [General] is the only entry point

## Path to vouch (trusted contributor)

1. Open a Discussion [Bug reports] with a quality repro + analysis + patch proposal
2. Help in existing Discussions (testing, investigation, additional context)
3. Maintainer creates an Issue from the accepted Discussion
4. Maintainer invites implementation (vouch)
5. Only after that -- PR

## Our bug candidates for Discussions (no duplicates found via gh graphql)

| Upstream issue | Bug | Our entry point | Complexity |
|---|---|---|---|
| #3312 | Failover routed image route: resend to failed provider + set_model race | agent_engine/turn/run_once.rs, armed_image_route | medium |
| #3314 | Resume refused: failed descriptor retains ownerClientId | pa-daemon lease.rs reclaim | medium |
| #3315 | Paste API key does not work in TUI | pa-tui editor paste | low |
| #3250 | Double character on key repeat/IME | pa-tui input handler | low |
| #3174 | TUI status row panic (out-of-range index) | pa-tui status-row guard | low |

All these files are NOT touched by our env rename (0 occurrences of ALLSPARK_) -- patches will be clean.

## Our existing bugfixes (in the fork, not upstream)

- is_error carried through the tool bridge
- bash signal kill = failure
- cron one-shot interrupt-safe
- worker commands session-plane
- Genspark/CodeBuddy 5xx + credits classification
- char-boundary UTF-8 panic in truncate
- migration skips auth.json (rotating refresh)

## Deliberately kept upstream references

- legacy_admission.rs reads ~/.prime/supervisor-owners
- migration reads ~/.prime/agent (settings/models, never auth.json)
- prime_inference.rs reads ~/.prime/config.json
- hold_refusal.rs recognizes upstream prime-agent holder
- PRIME_API_KEY (Prime Inference credential)
- DO_NOT_TRACK (cross-tool standard)
