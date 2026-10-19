url: https://blog.sandbase.ai/coding-agent-benchmark-claude-code-sandbase-models-2026
title: "Claude Code With Other Models: 11-Model Benchmark (2026)"
content: >-
  GPT-6 Luna fixed our pagination bug in 22 turns; Claude Opus 5.5 needed 5.
  Claude Code’s own JSON said the Luna run cost $0.33. At SandBase’s list price
  it was under a cent. Both numbers came out of the same run, and the gap
  between them is most of what this test is about.


  We pointed Claude Code 2.1.246 at SandBase and swapped the model underneath
  it: 10 models from the brief, plus one substitute, each on 6 small Python repo
  tasks, twice, graded by hidden tests the agent never saw. This **Claude Code
  with other models benchmark** ran on 2026-10-03 (UTC) between 13:40 and 14:15.
  It answers two practical questions: which models actually work as Claude
  Code’s engine, and what a task really costs when Claude Code’s cost counter
  doesn’t know the model.


  > **Key takeaway**

  > 

  > 

  > *   Six models passed 12/12: GPT-6.1 Sol, GLM-5.3 Prime, Claude Sonnet 5.5,
  Kimi K3, Qwen3.8 Max 0902 and Claude Opus 5.5. GPT-6 Luna and Grok 4.7 passed
  11/12; DeepSeek V4 Pro and MiniMax M3 passed 10/12.

  > *   GLM-5.3 never started: SandBase’s `/v1/messages` returned HTTP 400,
  “does not support api_format ‘anthropic’”. GLM-5.3 Prime, which does accept
  that format, stood in and passed 12/12.

  > *   Median list-price cost per task ran from $0.0071 (GPT-6 Luna) to $0.54
  (Claude Opus 5.5). Most models landed between $0.10 and $0.18.

  > *   Claude Code’s `total_cost_usd` priced every non-Anthropic model at
  $5/$25 per million tokens. It reported $44.55 for the whole study; the
  list-price estimate is $17.16.

  > *   Scope: 6 small stdlib Python tasks, N=2, default settings, one day, one
  machine. It shows which models work as Claude Code’s engine on tasks like
  these. It is not a general coding ranking.


  ## How to point Claude Code at SandBase


  Claude Code talks the Anthropic Messages protocol. SandBase exposes an
  Anthropic-compatible `POST /v1/messages` and maps the `model` field to the
  provider, so the setup is all environment variables:


  *   `ANTHROPIC_BASE_URL=https://api.sandbase.ai`

  *   `ANTHROPIC_AUTH_TOKEN=$SANDBASE_API_KEY` (sent as a Bearer token; unset
  `ANTHROPIC_API_KEY` so it doesn’t take over)

  *   `ANTHROPIC_MODEL=<SandBase model id>`, plus the same id in
  `ANTHROPIC_SMALL_FAST_MODEL` and the
  `ANTHROPIC_DEFAULT_HAIKU/SONNET/OPUS_MODEL` aliases, so background calls and
  subagents use the same model (we set all of them; we didn’t test what happens
  if you don’t)

  *   `CLAUDE_CONFIG_DIR=<a fresh temp dir>`, so the run doesn’t touch your
  normal Claude Code login, history or settings


  The key lives only in the environment; it is never written into the repo or
  the config dir. For the general Claude Code workflow (permissions, CLAUDE.md,
  subagents) see our Claude Code complete guide.


  _Caption: SandBase’s Anthropic-compatible Messages reference documents POST
  /v1/messages, Bearer or x-api-key auth and the forwarded anthropic-version
  header, which is the contract Claude Code ran on in this test (captured
  2026-10-03)._


  On 2026-10-03 we found no SandBase docs page about routing Claude Code’s
  model. The Setup guide connects Claude Code to SandBase tools, which is a
  different job, so the Messages reference is the contract we relied on.


  Two side effects showed up. For every non-Anthropic id, Claude Code printed
  `[claude-code:unrecognized_model]` on stderr (all 108 such runs). The runs
  still worked; the only harm we measured was the cost figure. And not every
  model accepts the Anthropic format: GLM-5.3 failed all 12 runs in about 2
  seconds with HTTP 400. Send one `/v1/messages` request before you commit. We
  pinged nine non-Anthropic ids that way, and only GLM-5.3 refused.


  ## The six tasks


  Each task is a tiny repo (1 to 4 source files, stdlib only) with a README spec
  and a visible failing test. Every model got the same prompt:


  `Read README.md. Run the tests with `python3 -m pytest -q`, change the code so
  it meets the spec in README.md and the tests pass, then rerun the tests. Do
  not edit files under tests/.`

  | Task | What the agent has to do | Hidden tests check |

  | --- | --- | --- |

  | T1 pagination | Fix 0-indexed slicing, floor page count, `has_prev` |
  Past-end page, invalid args, `page_count(0)`, `page_info` |

  | T2 date range | Rewrite a naive `--range` parser | Whitespace, open ends,
  Feb 30, leap day, `20260101` rejected, stderr + exit code 2 |

  | T3 LRU cache | Implement from a stub | `in` doesn’t refresh recency, update
  doesn’t evict, `on_evict`, `pop`, falsy values |

  | T4 counter | Make a racy counter thread-safe | Atomic `reset()` under load,
  `ValueError` without leaving the lock held, read-only `value` |

  | T5 refactor | Extract `money.py` from two drifted copies | Half-up rounding
  for negatives, both files import it, private copies gone, 5 old tests pass |

  | T6 CSV export | Implement `to_csv` from a quoting spec | Formula guard on
  strings only, quoting rules, header quoting, empty string |


  Pass means the full suite passed after the run: the original visible tests
  (restored from our copy in case the agent touched them) plus the hidden file,
  copied in only after Claude Code exited. Each run got a fresh temp dir and
  config dir, `--max-turns 40`, a 600 s timeout and
  `--dangerously-skip-permissions`, with up to 5 runs in parallel and a pytest
  8.3.3 virtualenv first on `PATH`. Beforehand we checked that every visible
  suite failed on the starting code and that our reference solutions passed
  every hidden test.


  ## Results


  Prices are SandBase list prices on 2026-10-03, read from `prompt_token_price`
  and `completion_token_price` in `GET https://api.sandbase.ai/v1/models/<id>`,
  in dollars per million tokens. That endpoint (and `GET /v1/tasks/<id>/cost`)
  needs the same Bearer key; the public model pages show the same prices.


  | Model (SandBase id) | $/M in / out | Pass (12) | Median turns | Median time
  | Median cost / task | Claude Code reported (12 runs) |

  | --- | --- | --- | --- | --- | --- | --- |

  | `openai/gpt-6.1-sol` | $2 / $10 | 12 | 23.5 | 114 s | $0.117 | $4.55 |

  | `z-ai/glm-5.3-prime` | $2.80 / $8.80 | 12 | 8.5 | 37 s | $0.106 | $1.93 |

  | `anthropic/claude-sonnet-5.5` | $2 / $10 | 12 | 9 | 109 s | $0.124 | $1.56 |

  | `moonshotai/kimi-k3` | $3 / $15 | 12 | 10 | 51 s | $0.136 | $2.75 |

  | `alibaba/qwen3.8-max-0902` | $2 / $6 | 12 | 11.5 | 90 s | $0.181 | $6.12 |

  | `anthropic/claude-opus-5.5` | $4 / $20 | 12 | 5 | 44 s | $0.540 | $7.64 |

  | `openai/gpt-6-luna` | $0.10 / $0.50 | 11 | 22 | 100 s | $0.0071 | $4.33 |

  | `x-ai/grok-4.7` | $2 / $6 | 11 | 8 | 31 s | $0.128 | $3.31 |

  | `deepseek/deepseek-v4-pro-0813` | $1.32 / $3.96 | 10 | 8.5 | 114 s | $0.046
  | $2.90 |

  | `minimax/minimax-m3` | $0.30 / $1.20 | 10 | 10.5 | 45 s | $0.044 | $9.46 |

  | `z-ai/glm-5.3` | $1.40 / $4.40 | 0 | n/a | n/a | n/a | $0 (HTTP 400) |


  Of the 120 runs that actually started (nine lineup models plus GLM-5.3 Prime),
  114 passed. Nobody timed out, nobody hit the turn cap, and no run edited a
  test file. T1 to T4 were passed by every model that ran. All six misses came
  from T5 and T6.


  Turn counts show two styles. Opus 5.5 worked in large steps, a median 5 turns.
  GPT-6.1 Sol and GPT-6 Luna took 22 to 24 small ones. Both passed. Time didn’t
  track turns: Sonnet 5.5 needed 109 s for 9 turns, probably in part because its
  upstream was slow that afternoon. A bare “Say OK.” to it through
  `/v1/messages`, sent during our pilot, took 26 s, while the other models
  answered in 2 to 4 s; a re-ping at 14:26 UTC took 5.6 s.


  ## Where models failed


  **T5 refactor: copied the bug along with the code (4 runs).** DeepSeek V4 Pro
  and MiniMax M3 both failed T5 twice, the same way. The spec says tax rounds
  half up, “away from zero”. The `orders.py` copy rounded halves up toward
  positive infinity, which is right for positive amounts and wrong for negative
  ones. Both models moved that logic into `money.py` unchanged and wrote a
  docstring above it promising “away from zero”, so `apply_tax(-200, 25)`
  returned 0 instead of -1. The visible tests had no negative amounts, so both
  agents saw green and stopped. Every other model handled the negative case.


  **T6 CSV: one edge in the formula guard (2 runs).** GPT-6 Luna once applied
  the leading `-` guard to the integer `-5`, writing `'-5`, although the spec
  limits the guard to strings. Grok 4.7 once checked `text[:1] in "=+-@"`, and
  in Python the empty string is “in” every string, so an empty cell came out as
  `'`. Each model passed T6 on its other run.


  **GLM-5.3: protocol, not coding (12 runs).** HTTP 400 on the first request,
  “Model ‘glm-5.3’ does not support api_format ‘anthropic’”, 0 tokens, about 2
  seconds. For it, the test measured nothing about coding.


  No miss was a tool-format error, a timeout or a mid-run API error. Each was an
  untested branch of the spec the agent never checked. With a cheaper model, put
  the spec’s edge cases into the visible tests, or have a stronger model review
  the diff. Our strong pass vs cheap loops piece covers that retry-or-escalate
  trade-off.


  ## Cost: Claude Code’s number vs the list price


  Claude Code’s JSON reports `total_cost_usd` from its own price table. For
  Claude Sonnet 5.5, it matched our list-price estimate to the cent: SandBase
  lists Sonnet at $2/$10, and so does Claude Code. For every other model it was
  wrong. Recomputing all 96 non-Anthropic runs that made requests (GLM-5.3’s
  errors had zero usage) showed Claude Code priced each one at $5 input / $25
  output per million, 1.25x for cache writes, 0.1x for cache reads. That’s an
  Opus-class rate. It also priced Opus 5.5 at $5/$25 against SandBase’s $4/$20
  list.


  So we computed cost ourselves from the cumulative `usage` block in Claude
  Code’s JSON. Input tokens and output tokens are priced at list. Cache writes
  get the model card’s `cache_write_multiplier`, and cache reads get its
  `cache_read_multiplier`. Five models (Kimi K3, Grok 4.7, DeepSeek V4 Pro,
  MiniMax M3, GLM-5.3 Prime) have no cache-write multiplier on their cards.
  Those five reported zero cache-write tokens anyway, so that choice changes
  nothing. These are estimates: SandBase’s `GET /v1/tasks/<id>/cost` doesn’t
  resolve `/v1/messages` ids, so we couldn’t reconcile billing per run.


  _Caption: GPT-6 Luna is listed at $0.10 input and $0.50 output per million
  tokens, which is why its 22-turn runs still came to under a cent each
  (captured 2026-10-03)._


  Claude Code re-sends its system prompt and tool definitions every turn, so
  input dominates, and where the route caches, most of it comes back as cache
  reads. Cache reads were 92% to 94% of all tokens for GPT-6.1 Sol, GPT-6 Luna,
  Sonnet 5.5 and GLM-5.3 Prime. MiniMax M3 is the exception at 11%: a median
  134,604 uncached input tokens per run against 16,580 cache reads. At $0.30 per
  million that barely matters. On a pricier model without caching it would.


  GPT-6 Luna costs about 1/16 as much per task as GPT-6.1 Sol here ($0.0071 vs
  $0.117), for 11/12 vs 12/12. That’s in line with our Pro vs Flash tier
  benchmark, where Luna matched Sol on short tool tasks at about 1/20 of the
  cost.


  _Caption: On 2026-10-03 the catalog showed Claude Opus 5.5 at 25% off ($3 /
  $15 instead of $4 / $20), while its model card still listed $4 / $20; the cost
  table uses the card (captured 2026-10-03)._


  Opus 5.5 is the expensive one: $6.01 for 12 runs at list price, $0.54 median
  per task. Its median run wrote 94,059 tokens to cache and read back only
  37,598; with 5 turns, little of what it cached got reused. If the 25%
  promotion applied, its median would be about $0.41, which we didn’t verify.
  The page’s “API Free Week” banner reads as covering data APIs; model token
  prices in the table are unchanged by it.


  Whole study: $17.16 at list price for 132 runs (Claude Code reported $44.55).
  A 2-run pilot before it ($0.24) and one run of the script below afterwards
  ($0.0069) aren’t counted in the tables.


  ## How to choose


  | If you want | Use | But |

  | --- | --- | --- |

  | Lowest cost that still passes nearly everything | GPT-6 Luna: 11/12, $0.0071
  per task | Many small turns (median 22); spell out edge cases |

  | No misses, moderate cost | GPT-6.1 Sol, GLM-5.3 Prime, Sonnet 5.5 or Kimi
  K3: 12/12, $0.10 to $0.14 | Sol and Sonnet took 109 to 114 s at the median;
  Prime and Kimi under a minute |

  | Fastest wall time | Grok 4.7 (31 s median) or GLM-5.3 Prime (37 s) | Grok
  missed one CSV edge case |

  | Fewest turns, biggest steps | Claude Opus 5.5: median 5 turns | 4x to 5x the
  cost per task of the $0.10 to $0.14 group at list price |

  | Cheap models that aren’t Luna | DeepSeek V4 Pro or MiniMax M3, about $0.045
  | Both copied a rounding bug in the refactor task twice |

  | The original GLM-5.3 | Not through Claude Code on SandBase today |
  `/v1/messages` returns 400; use GLM-5.3 Prime |


  ## Run it yourself


  This is the script we ran on 2026-10-03 for the summary below. It copies a
  repo into a temp dir, runs Claude Code against SandBase with an isolated
  config, and prices the run from the live model card. The key comes only from
  `SANDBASE_API_KEY`.


  ```

  #!/usr/bin/env bash

  # Run Claude Code headless against SandBase on a throwaway copy of a repo,

  # then price the run at the SandBase list price of the model.

  # Usage: SANDBASE_API_KEY=... ./cc_sandbase.sh <sandbase-model-id> <repo-dir>
  "<prompt>"

  set -euo pipefail

  MODEL="$1"; SRC="$2"; PROMPT="$3"

  : "${SANDBASE_API_KEY:?set SANDBASE_API_KEY in the environment}"


  WORK="$(mktemp -d)"   # throwaway copy: the agent may run any command here

  CFG="$(mktemp -d)"    # isolated Claude Code config, so your normal login is
  untouched

  cp -R "$SRC"/. "$WORK"/

  cd "$WORK"


  env -u ANTHROPIC_API_KEY \
    CLAUDE_CONFIG_DIR="$CFG" \
    ANTHROPIC_BASE_URL="https://api.sandbase.ai" \
    ANTHROPIC_AUTH_TOKEN="$SANDBASE_API_KEY" \
    ANTHROPIC_MODEL="$MODEL" ANTHROPIC_SMALL_FAST_MODEL="$MODEL" \
    ANTHROPIC_DEFAULT_HAIKU_MODEL="$MODEL" ANTHROPIC_DEFAULT_SONNET_MODEL="$MODEL" \
    ANTHROPIC_DEFAULT_OPUS_MODEL="$MODEL" \
    DISABLE_TELEMETRY=1 DISABLE_ERROR_REPORTING=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    perl -e 'alarm shift; exec @ARGV' 600 \
    claude -p "$PROMPT" --model "$MODEL" --output-format json --max-turns 40 \
      --dangerously-skip-permissions > "$CFG/result.json" 2> "$CFG/stderr.txt" || true

  echo "work dir: $WORK"

  python3 - "$MODEL" "$CFG/result.json" <<'EOF'

  import json, os, sys, urllib.request

  model, path = sys.argv[1], sys.argv[2]

  run = json.load(open(path))

  u = run.get("usage") or {}

  req = urllib.request.Request(f"https://api.sandbase.ai/v1/models/{model}",
                               headers={"Authorization": f"Bearer {os.environ['SANDBASE_API_KEY']}"})
  card = json.load(urllib.request.urlopen(req, timeout=60))["model_card"]

  p_in, p_out = float(card["prompt_token_price"]),
  float(card["completion_token_price"])

  read_mult = float(card.get("cache_read_multiplier") or 1)    # missing ->
  priced as normal input

  write_mult = float(card.get("cache_write_multiplier") or 1)  # missing ->
  priced as normal input

  cost = (u.get("input_tokens", 0) * p_in
          + u.get("cache_creation_input_tokens", 0) * p_in * write_mult
          + u.get("cache_read_input_tokens", 0) * p_in * read_mult
          + u.get("output_tokens", 0) * p_out) / 1e6
  print(json.dumps({"model": model, "is_error": run.get("is_error"),
  "num_turns": run.get("num_turns"),
                    "duration_ms": run.get("duration_ms"),
                    "claude_code_reported_usd": run.get("total_cost_usd"),
                    "sandbase_list_price_usd": round(cost, 4),
                    "usage": {k: u.get(k, 0) for k in ("input_tokens", "cache_creation_input_tokens",
                                                       "cache_read_input_tokens", "output_tokens")},
                    "result": (run.get("result") or "")[:200]}, indent=1))
  EOF

  ```


  `--dangerously-skip-permissions` lets the agent run any shell command without
  asking. Use it only in a throwaway directory like this one, never in a
  checkout with credentials, and never with a key that has a high spending
  limit.


  **Tested on 2026-10-03 (UTC).** At 14:17:01 we ran the script on the T1
  pagination repo with `openai/gpt-6-luna` and the prompt above. It finished at
  14:18:41 and printed this object (complete as printed; the script itself cuts
  `result` to 200 characters):


  ```

  {
   "model": "openai/gpt-6-luna",
   "is_error": false,
   "num_turns": 20,
   "duration_ms": 97428,
   "claude_code_reported_usd": 0.34457125000000005,
   "sandbase_list_price_usd": 0.0069,
   "usage": {
    "input_tokens": 1879,
    "cache_creation_input_tokens": 19629,
    "cache_read_input_tokens": 342890,
    "output_tokens": 1642
   },
   "result": "Updated `pager.py` to use 1-indexed pagination, validate invalid inputs, calculate partial pages correctly, and set `page_info` flags according to the README spec. I did not edit anything under `tests"
  }

  ```


  Afterwards we copied the hidden T1 tests into its work dir: 10 passed.
  `num_turns`, `usage`, `total_cost_usd` and `result` are fields we observed in
  Claude Code 2.1.246’s `--output-format json`, not a documented schema, so read
  them defensively. Next steps: the Anthropic Messages API reference for the
  contract, and get a SandBase API key to run it on your own repo.


  About the test material: the tasks are toy repos we wrote for this test, with
  no third-party code or personal data. The task specs, prompt, grading rule and
  aggregates are all in this article. Per-run logs and transcripts are kept
  internally.


  ## FAQ


  ### What is the best model for Claude Code besides Claude?


  GPT-6.1 Sol, GLM-5.3 Prime, Kimi K3 and Qwen3.8 Max 0902 passed 12/12 here at
  $0.10 to $0.18 per task; GPT-6 Luna passed 11/12 at $0.0071. With 6 tasks and
  2 runs each, treat that as a shortlist.


  ### Can Claude Code use GPT, Kimi, DeepSeek or Qwen models?


  Yes, through an Anthropic-compatible endpoint like SandBase’s `/v1/messages`.
  Set `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`, and set `ANTHROPIC_MODEL`
  to the SandBase id. Eight of the nine non-Anthropic ids we tried worked;
  GLM-5.3 returned HTTP 400 for the Anthropic format.


  ### Why is Claude Code’s cost wrong for non-Anthropic models?


  It doesn’t know their prices. In every non-Anthropic run, its `total_cost_usd`
  matched a $5/$25 per million rate exactly. Across the study it reported $44.55
  against a $17.16 list-price estimate, and for MiniMax M3 alone, $9.46 against
  $0.56. Price runs from the `usage` block instead.


  ### Is GPT-6 Luna good enough for Claude Code?


  For small, well-specified tasks, mostly: 11/12, one miss on a spec edge case,
  under a cent per task. It takes many small steps (median 22 turns, about 100
  s). Put edge cases in visible tests or have a stronger model review its diffs.


  ### Does prompt caching matter when Claude Code runs other models?


  A lot. Claude Code re-sends a large system prompt every turn. For GPT-6.1 Sol,
  Luna, Sonnet 5.5 and GLM-5.3 Prime, 92% to 94% of tokens were cache reads,
  listed at 0.05x to 0.2x the input price. MiniMax M3 reported few cache reads,
  so most of its input was priced at the full input rate.


  ## Limitations


  Six small stdlib Python tasks with clear specs are not a real codebase, and
  two runs per task can’t separate models one miss apart. Settings were
  defaults: no reasoning-effort tuning, no CLAUDE.md. Timing came from one
  machine through a local proxy, up to five runs in parallel, one afternoon, so
  it’s rough. Costs are list-price estimates from reported usage, not reconciled
  bills, and we didn’t confirm the Opus promotion. GLM-5.3 Prime was added after
  GLM-5.3 failed. Models and routes change; rerun the script on your own repo
  before you choose.
