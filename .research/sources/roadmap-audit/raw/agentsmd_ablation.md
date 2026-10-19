url: https://www.developersdigest.tech/blog/context-files-coding-agents-ablation-2026
title: "AGENTS.md Files Don't Move Coding Agent Correctness: A 288-Run Ablation"
content: >-
  **Last updated:** July 31, 2026


  AGENTS.md and CLAUDE.md files are the standard way teams steer coding agents.
  New research asks the question most of us have never tested: does the context
  actually change what the agent ships? The answer, from a controlled ablation
  of 288 evaluated runs across Claude Code and Codex on 17 real tasks from 3
  repositories, is that injection strategy does not measurably move correctness
  on either agent, bounded to under 10-15 percentage points via equivalence
  testing.


  The paper also explains why, and the why is more interesting than the null.


  The researcher, Prakhar Khatri, built a SWE-bench-style harness with three
  context-injection strategies applied to the same tasks:


  *   **none**: the AGENTS.md is removed from the workspace entirely

  *   **always_on**: the full file is injected into the system prompt every turn

  *   **selective**: topic-organized wiki files are placed in the workspace and
  the agent reads them on demand


  Each task ran under all three strategies with 3 independent repeats, on Claude
  Code (claude-sonnet-4-6) and Codex CLI (gpt-5.5). Tasks came from merged pull
  requests in three Python repositories: pdm (477-word context file),
  firebase-admin-python (1,236 words, rated "Excellent" on the study's rubric),
  and opshin (248 words). Correctness was scored by hidden gold tests extracted
  from the merged PRs, and runs executed on an egress-locked pod with GitHub DNS
  blackholed so agents could not read gold solutions.


  Pass rates by strategy:


  | Strategy | Claude (15 tasks) | Codex (17 tasks) |

  | --- | --- | --- |

  | none | 53.3% (24/45) | 58.8% (30/51) |

  | always_on | 55.6% (25/45) | 56.9% (29/51) |

  | selective | 55.6% (25/45) | 52.9% (27/51) |


  Omnibus permutation tests found no strategy effect (p = 1.00 for Claude, p =
  0.66 for Codex). Equivalence testing bounds every pairwise difference to under
  10pp for Claude and under 15pp for Codex.


  ## Why context does not rescue these tasks#


  The mechanism is the durable finding. The author triaged near-miss failures
  (1-4 failing gold tests), the exact tasks where one extra fact could flip a
  pass. None of them were missing-knowledge failures:


  *   a union-expansion optimization pass built correctly but with a correctness
  bug

  *   reactive retry implemented where the fix required proactive auth-token
  refresh

  *   a validator check the agent knew about but miswired

  *   type-narrowing that needed deep type-system reasoning


  The failure mode is implementation skill: feature design, pattern selection,
  exact wiring. A context file cannot supply that. A manipulation probe hammered
  the point home: the two convention-closest near-miss tasks were re-run under
  all strategies on both agents, 36 cells total, and the real, unmodified
  AGENTS.md never converted a near-miss to a pass on either agent (Codex failed
  18/18 regardless of strategy).


  ## Context still changes how agents work#


  The correctness null does not mean context is inert. Two process-level signals
  survived:


  *   On opshin, whose AGENTS.md warns that the full test suite takes over 20
  minutes, blind full-suite pytest runs dropped monotonically with context dose:
  none 3.67, always_on 2.44, selective 1.67 per cell. Wall-clock time fell about
  24% (2,689s vs 2,066s vs 2,032s). The file changed test strategy, not
  outcomes.

  *   For Claude, selective injection cut cache-creation tokens on 11 of 11
  tasks (Holm-corrected p = 0.012), a mechanical artifact of a shorter system
  prompt.


  The study offers a clean explanation for the contradictory prior results:
  borderline task difficulty is agent-specific. Across the 15 shared tasks,
  per-task pass rates correlate at Spearman rho = 0.75, but roughly 40% of tasks
  sit in different difficulty bands per agent (borderline for one, floor or
  ceiling for the other). A task set calibrated on Codex is mostly floor/ceiling
  for Claude, where no manipulation can register. Single-agent studies draw
  tasks from different agents' informative bands and reach different conclusions
  without any contradiction in the underlying behavior.


  A power analysis makes the practical stakes clear: at 15-17 tasks with 3
  repeats, even a 30pp effect is only caught 57% of the time, and detecting a
  10pp effect at 80% power needs roughly 120-200 tasks. Most "context files
  work" or "context files are useless" takes floating around are built on
  studies that structurally cannot detect the effect they claim.


  This is the most rigorous AGENTS.md study yet, and its honest reading is not
  "delete your context files". It is:


  **Context files are behavior steering, not capability injection.** If you want
  them to change correctness, write them for the failure modes that actually
  gate tasks: worked examples, task decomposition, wiring patterns. The paper's
  own recommendation is that effort spent on generic convention prose may pay
  off less than task decomposition, tooling, or example-driven prompting, which
  lines up with why example-rich context wins in our context engineering guide
  and in our coverage of Claude's context engineering rules.


  **Process effects are real and valuable.** We run this site's fleet on
  AGENTS.md files with verification gates and commit rules, and the opshin
  result is exactly the mechanism we rely on: files that tell agents when and
  how to run expensive checks change behavior in measurable ways even when
  pass/fail does not move. That is the argument behind keeping context lean,
  which we covered in agent context reduction patterns and running a fleet of
  Claude agents.


  **Calibrate per agent.** The rho = 0.75 finding is a methodological warning
  for everyone building agent workflows: what is borderline for Claude Code may
  be trivial for Codex and vice versa. Our comparison of repository context for
  coding agents and the Claude Code skills primer both touch the same trap from
  different angles: guidance that changes one agent's behavior does not transfer
  to another.


  Caveats worth stating plainly: this is 3 Python repositories, naturalistic
  style-guide-type files, and a snapshot of two model versions as of late July
  2026. Purpose-built, task-specific context remains an open question. But the
  burden of proof just shifted: teams claiming AGENTS.md files improve agent
  correctness should now have to show it at scale, not assume it.


  ### Do AGENTS.md files improve coding agent correctness?#


  Not measurably in this study. Across 288 evaluated runs on Claude Code and
  Codex, 17 real tasks, and 3 Python repositories, context-injection strategy
  did not change pass rates. Equivalence testing bounds every pairwise
  difference to under 10 percentage points for Claude and under 15 for Codex.


  ### Which context injection strategies did the study compare?#


  Three: none, where the AGENTS.md is removed from the workspace; always_on,
  where the full file is injected into the system prompt every turn; and
  selective, where topic-organized wiki files sit in the workspace and the agent
  reads them on demand. Each task ran under all three with 3 independent
  repeats.


  ### Why did context files not rescue failing tasks?#


  The near-miss failures were implementation-skill problems, not missing
  knowledge: a correctness bug in an optimization pass, reactive retry where
  proactive token refresh was needed, a miswired validator, and hard
  type-narrowing. A manipulation probe re-ran the two convention-closest
  near-miss tasks under every strategy, and the real AGENTS.md never converted a
  near-miss to a pass.


  ### Are context files useless, then?#


  No. They changed how agents worked even when pass/fail did not move. On
  opshin, whose AGENTS.md warns the full test suite takes over 20 minutes, blind
  full-suite pytest runs dropped with context dose and wall-clock time fell
  about 24 percent. The post's reading is that context files are behavior
  steering, not capability injection.


  *   The Text Is a Control Surface - the boundary of this null: decorative
  prose pays nothing, but actionable interface text and re-grounding after lossy
  stages carry measurable weight

  *   The Context Engineering Guide: Getting Claude Code to Actually Read Your
  Repo

  *   Claude 5 Context Engineering Rules: What Changed in the System Prompt Era

  *   Managing a Fleet of Claude Agents

  *   Agent Context Reduction: Cutting Tokens Without Losing Behavior

  *   What Are Claude Code Skills? A Beginner's Guide

  *   Claude Code Now Reads AGENTS.md - setup, the four modes, and the CLAUDE.md
  precedence rule that catches people

  *   Which Coding Agent Reads Which Instruction File - which agents load
  AGENTS.md, CLAUDE.md or GEMINI.md, and how they combine nested files


  *   Do Context Files Help Coding Agents? A Two-Agent Ablation Study on Real
  Repositories - arXiv:2607.27250 (abstract)

  *   Full paper HTML - arXiv:2607.27250v1

  *   Evaluating AGENTS.md: Are Repository-Level Context Files Helpful for
  Coding Agents? - Gloaguen et al., arXiv:2602.11988 (prior work cited by the
  study)
