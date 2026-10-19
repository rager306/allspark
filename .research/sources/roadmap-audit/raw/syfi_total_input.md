url: https://tracelab.cs.washington.edu/exp/session/total_input_growth/
title: Total input growth · SyFI TraceLab
content: >-
  All figures


  Session


  one continuous trace of work, often spanning multiple requests or problems.


  Request


  one user input through the agent's final response.


  Agent step


  one model call inside a request.


  User-initiated step


  an agent step started by user input.


  Tool-triggered step


  an agent step started by a tool result.


  Problem

  **Within a single coding session, how does the total input length (`prefix +
  append`) move from one agent step to the next — and when it shrinks, is that
  accounting jitter or a real context compaction?**


  Tables


  **All steps**


  | Metric | Claude | Codex |

  | --- | --- | --- |

  | Steps | 300,062 | 334,681 |

  | Positive (growth) % | 99.59% | 97.18% |

  | Negative (reduction) % | 0.34% | 2.81% |

  | Micro % | 0.04% | 0.89% |

  | Ordinary % | 0.07% | 1.30% |

  | Major reduction % | 0.22% | 0.62% |

  | Avg positive growth | 1,856 | 1,824 |


  **User-initiated steps**


  | Metric | Claude | Codex |

  | --- | --- | --- |

  | Steps | 38,543 | 32,821 |

  | Positive (growth) % | 98.72% | 79.06% |

  | Negative (reduction) % | 1.27% | 20.91% |

  | Micro % | 0.23% | 8.34% |

  | Ordinary % | 0.35% | 11.36% |

  | Major reduction % | 0.69% | 1.21% |

  | Avg positive growth | 1,361 | 2,955 |


  **Tool-initiated steps**


  | Metric | Claude | Codex |

  | --- | --- | --- |

  | Steps | 261,519 | 301,860 |

  | Positive (growth) % | 99.72% | 99.15% |

  | Negative (reduction) % | 0.20% | 0.85% |

  | Micro % | 0.02% | 0.08% |

  | Ordinary % | 0.03% | 0.21% |

  | Major reduction % | 0.15% | 0.56% |

  | Avg positive growth | 1,929 | 1,726 |


  Table 1 Same-session context change by step trigger: per-step change in total
  input length (prefix + append tokens) versus the previous step in the session,
  split into growth and reduction bands.


  Context almost always grows step to step (the paper’s
  `tab:context_growth_and_compaction`). Across all steps the window grows on
  **99.59%** of Claude steps and **97.18%** of Codex steps — new user input,
  tool results, and output all stack onto the prior context — adding ~1.8k
  tokens per growing step. Reductions are rare and provider-shaped: for Claude
  the negatives are 0.34% and skew **major** (0.22%, real compactions), whereas
  Codex reduces 2.81% of the time and mostly in the harmless **micro/ordinary**
  bands. The split also concentrates by trigger: Codex’s reductions pile up on
  **user-initiated** steps (20.91% of those steps shrink, vs. 0.85% of
  tool-initiated), while tool-initiated steps for both providers grow ~99% of
  the time.


  Reference


  Experiment overview

  Each row in the trace is one agent step. Walking the steps of a session in
  order, this experiment records the per-step change in **total input length**
  (`prefix_tokens + newly_append_tokens`) — a positive delta is the window
  growing, a negative delta is it shrinking — and classifies every drop into one
  of three buckets.


  Method and assumptions:


  *   **Total input** for a step is `prefix_tokens + newly_append_tokens`
  (cached prefix plus freshly appended input). The per-step metric is the signed
  delta of that quantity from the **previous step seen in the same session**.

  *   **Pairing.** A growth event is emitted only when the current step’s
  **first timing event** is a visible input event — a `user_message` or a
  `tool_result` (the step’s _trigger_) — and the session has been seen before.
  The `previous` step is whatever step was last observed for that session in
  trace order, regardless of its trigger. Steps are ordered within a session by
  ingestion order (`round_pk` = file order), the same line-order sequencing the
  pre-DuckDB scan used.

  *   **Reduction buckets** (thresholds from `artifacts/utils/growth.py`,
  overridable on the CLI): 
      *   **micro-reduction** — drop `≤ 1024` tokens (accounting jitter);
      *   **major-reduction** — drop `≥ 50000` tokens (a real context compaction);
      *   **ordinary reduction** — anything between the two.

  *   **Triggers reported.** Summary rows are cut three ways — `all`, `user`,
  and `tool_result` — by the current step’s trigger, and per scope (`merged`
  plus each provider).

  *   Shares the growth helpers (`build_growth_stats`, `reduction_bucket`, the
  CSV writers) with the `trace_facts` overview summaries.


  Code structure

  This is a **hybrid** experiment: the trace DuckDB does the single-pass ingest,
  and Python keeps the per-session sequencing and growth bucketing.


  *   `iter_growth_events_from_db(con)` — the only data-loading code. Two
  queries in ingestion order (step scalars `ORDER BY round_pk`, and each step’s
  _first_ timing event at `event_index = 1` for the trigger type and timestamp),
  walked in Python with a `last_by_session` map to emit one growth event per
  qualifying step — exactly reproducing the old line-by-line JSONL scan.

  *   `_epoch_us_to_iso(...)` — timestamps are pulled as integer
  epoch-microseconds (native/wasm identical) and rebuilt to the canonical `…Z`
  ISO string, so the timestamp columns match the pre-DuckDB output bit-for-bit.

  *   `build_growth_stats(...)` / `reduction_bucket(...)` /
  `write_summary_csv(...)` / `write_events_csv(...)` — unchanged shared helpers
  in `artifacts/utils/growth.py`.

  *   `write_filtered_events_csv(...)` — the stable-sorted reduction /
  micro-reduction drilldowns.


  The data layer lives in `artifacts/utils/trace_db.py` (see
  `artifacts/utils/DB_SCHEMA.md`).


  Running it

  ```

  # default merged trace (materialized to a temp DuckDB cache on first use)

  uv run python artifacts/session/total_input_growth/analyze.py


  # a specific trace

  uv run python artifacts/session/total_input_growth/analyze.py -i
  trace/sample.jsonl


  # a prebuilt DB (run_all.py's build-db step passes this), into a chosen dir

  uv run python artifacts/session/total_input_growth/analyze.py --db
  "$TMPDIR/trace.duckdb" -o "$TMPDIR/out"

  ```


  Knobs: `--micro-reduction-max-tokens` / `--major-reduction-min-tokens` retune
  the reduction buckets, `--no-drilldowns` writes only the summary,
  `--limit-events` caps each drilldown after a stable sort, and `--summary-csv`
  / `--events-csv` / `--reductions-csv` / `--micro-csv` override individual
  paths.


  Outputs

  *   `total_input_growth_summary.csv` — growth/reduction bucket counts and
  delta stats per `(scope, trigger)`.

  *   `total_input_growth.md` — GFM mirror of the paper float
  `tab:context_growth_and_compaction` (Claude vs Codex, by step trigger),
  rendered on the web detail page.

  *   `total_input_growth_events.csv` — every same-session growth event, in
  trace order.

  *   `total_input_reductions.csv` — only the negative-delta events (all three
  reduction buckets).

  *   `total_input_micro_reductions.csv` — only the micro-reduction events.


  No figures.


  Previous**Session internal counts**Next**Context compactions**
