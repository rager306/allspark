url: https://www.claudecodecamp.com/p/inside-claude-code-s-system-prompt
title: "Claude Code System Prompt: What's Inside (Token Map)"
content: >-
  This is the fourth post in a series where I dig into Claude Code's internals
  through experiments and data. The first three covered prompt caching, tool
  use, and context management.


  I wanted to see what Claude Code actually sends to the Anthropic API. What
  instructions does the model receive? What's in the tool definitions? How does
  your CLAUDE.md get wired in?


  People have been intercepting Claude Code's API traffic with local proxy
  servers, and the Piebald-AI project tracks every change across 125+ releases.
  I combined their extractions with my own token measurements to build a
  complete picture of what's happening under the hood.


  Understanding this made a lot of Claude Code's behavior click. Why it writes
  good commit messages without being asked. Why it asks permission before `git
  push` but not before `grep`. Why its responses are terser than regular Claude.
  Each of those traces back to a specific line in the prompt.


  ## The API request


  When Claude Code sends a message to the Anthropic API, it doesn't send a
  single prompt string. It sends a structured request with three sections:


  ```

  {
    "system": [
      {"type": "text", "text": "You are Claude Code...", "cache_control": {"type": "ephemeral"}},
      {"type": "text", "text": "You are an interactive CLI tool..."}
    ],
    "tools": [
      {"name": "Read", "description": "Reads a file...", "input_schema": {...}},
      {"name": "Edit", "description": "Performs exact string replacements...", "input_schema": {...}},
      ...
    ],
    "messages": [
      {"role": "user", "content": "Fix the login bug"},
      {"role": "assistant", "content": [{"type": "tool_use", ...}]},
      {"role": "user", "content": [{"type": "tool_result", ...}]}
    ]
  }

  ```


  The `system` array is the actual system prompt - the instructions that shape
  Claude's behavior. The `tools` array is separate: JSON schemas describing
  every tool Claude can call. And `messages` is the conversation.


  The `cache_control: {"type": "ephemeral"}` annotation enables prompt caching.
  I covered the mechanics in the prompt caching deep dive. The system prompt and
  tools are static, so they get cached on the first request and reused at 90%
  discount for the rest of the session.


  I measured the total size of this initial payload by running `claude -p
  --output-format json` and checking the usage data:


  ```

  From /tmp (no project config):     27,169 tokens

  From my project dir (full setup):  30,919 tokens

  ```


  That 3,750-token delta is the cost of my project's CLAUDE.md, memory files,
  project-specific skills, and MCP tools. The 27K baseline is the system prompt
  + built-in tools + my global configuration. It lines up with the Piebald-AI
  estimate of 16-25K for system prompt and tools alone, with the rest being my
  global CLAUDE.md and plugins.


  Now let's look at what's actually inside.


  #### **Free Claude Code crash course**


  60-min video lesson + CLAUDE.md starter kit. Yours when you subscribe.


  The system prompt isn't one big block of text. It's 110+ separate
  instructions, conditionally assembled based on your configuration. Here are
  the main sections, in the order they appear:


  **Identity and security** (~100 tokens). "You are Claude Code, Anthropic's
  official CLI for Claude." Plus guidelines about security testing — authorized
  pentesting is fine, refuse malicious requests. Short and foundational.


  **Task execution** (~600 tokens across 12 separate instructions). Read before
  modifying. Don't over-engineer. Don't create unnecessary files. Don't add
  error handling for scenarios that can't happen. Each instruction is 30-100
  tokens. They read like a senior engineer's code review comments compressed
  into rules.


  This is worth internalizing. When Claude Code reads a file before editing it,
  or when it suggests a simple approach instead of an abstraction, it's
  following these instructions. If you've ever wondered why Claude Code pushes
  back on over-engineering - "Only make changes that are directly requested or
  clearly necessary" is literally in the prompt.


  **Executing actions with care** (~540 tokens). This section defines which
  actions need your permission and which don't. It tells Claude to assess the
  "reversibility and blast radius" of every action. Reading a file is safe and
  reversible. Deleting a branch is not. That's why Claude Code asks before `git
  push` but not before `grep`.


  The specific language is interesting: "The cost of pausing to confirm is low,
  while the cost of an unwanted action (lost work, unintended messages sent,
  deleted branches) can be very high." This is the philosophy behind Claude
  Code's permission system, spelled out in the prompt.


  **Tool usage policy** (~550 tokens). Use Read instead of `cat`. Use Edit
  instead of `sed`. Use Grep instead of `grep`. These are instructions that
  explain why Claude Code prefers its native tools over shell commands.


  The reason is observability. When every file read is a Read tool call, Claude
  Code can log it, measure latency, and show it in the UI. An arbitrary `cat`
  command inside a bash call is opaque. This section also tells Claude when to
  run tool calls in parallel and when to use subagents for complex tasks.


  **Output and tone** (~320 tokens). Be concise. Lead with the answer, not the
  reasoning. Use markdown. Minimal emoji. If you've noticed that Claude Code's
  responses are terser than regular Claude, this is why.


  **Conditional sections** (0-1,300 tokens depending on your session):


  *   Auto mode: +188 tokens (instructions for autonomous operation)


  *   Plan mode: +142 to +1,297 tokens (read-only constraints)


  *   Learning mode: +1,042 tokens (pedagogical approach)


  *   Git status snapshot: +97 tokens (your current branch and changes)


  Total system prompt text: **2,300-3,600 tokens**. That's 1-2% of a 200K
  context window. The prompt itself is compact.


  ## The tool definitions


  The system prompt is 2.5K tokens. The tool definitions are 14-17K. They're the
  biggest single component of every request, and they're where the interesting
  design decisions live.


  Each of Claude Code's 23+ built-in tools has a name, a description, and a JSON
  schema. Here's what they cost:


  | Tool | Tokens | Why it's that size |

  | --- | --- | --- |

  | TodoWrite | 2,161 | Structured fields for priority, status, assignee, tags |

  | TeammateTool | 1,645 | Descriptions of every subagent type (Explore, Plan,
  etc.) |

  | Bash | 1,558 | Full git commit message format + PR creation instructions |

  | SendMessage | 1,205 | Inter-agent communication protocol |

  | Agent | 931 | When to spawn subagents vs. handle inline |

  | EnterPlanMode | 878 | Plan mode constraints and behavior |

  | ReadFile | 440 | PDF, image, notebook support documentation |

  | Grep | 300 | Regex syntax, output modes, multiline matching |

  | Edit | 246 | String replacement rules, indentation preservation |

  | Glob | 122 | File pattern matching |


  **Total: ~14,000-17,600 tokens** depending on which tools are active.


  Look at Bash. It costs 1,558 tokens because it contains detailed instructions
  for how to format git commit messages, how to create pull requests, and how to
  handle pre-commit hooks. This is why Claude Code writes good commit messages
  without being asked — the format is in the tool description. It's also why
  Claude Code uses `git commit -m "$(cat <<'EOF'...EOF)"` with heredocs instead
  of inline quotes. That specific pattern is in the Bash tool definition.


  The Grep tool at 300 tokens includes documentation for ripgrep-specific
  syntax, output modes (`content`, `files_with_matches`, `count`), and multiline
  matching flags. When Claude Code writes a regex search, it has this reference
  material in context.


  Each tool definition is a small manual. Claude isn't figuring out how to use
  these tools from general knowledge — it has specific, detailed instructions
  for each one, tuned for how the tool actually works inside Claude Code.


  ### MCP tools


  Each MCP server you configure adds its tool definitions to the same array. A
  lightweight server with 3-4 tools might add 1,000-2,000 tokens. A heavy server
  like GitHub or Playwright with 20+ tools can add 10,000+. Thariq Shihipar
  estimated that 5 MCP servers can add ~55K tokens.


  Tool definitions load on every request, whether you use them in that turn or
  not, because the model needs to know what's available. Claude Code mitigates
  this with deferred tool loading — when the number of MCP tools passes a
  threshold, it loads lightweight stubs and lets the model discover full schemas
  through ToolSearch on demand.


  This is a useful mental model: every MCP server you register is always in the
  room, even when it's not talking. If you have servers you rarely use, removing
  them from your config reclaims that space for conversation.


  ## How CLAUDE.md actually works


  CLAUDE.md is how you customize Claude Code's behavior. But it doesn't go where
  most people think it goes.


  ### Where it lives


  Claude Code loads instructions from five locations:


  1.   **Project root**`./CLAUDE.md` — your project-specific instructions


  2.   **Subdirectories**`./src/CLAUDE.md`, `./packages/api/CLAUDE.md` — loaded
  when Claude accesses files in that directory


  3.   **Global**`~/.claude/CLAUDE.md` — your personal preferences across all
  projects


  4.   **Project-scoped global**`~/.claude/projects/-path-to-project/CLAUDE.md`
  — per-project personal config


  5.   **MEMORY.md**`~/.claude/projects/.../memory/MEMORY.md` — auto-saved
  memories


  ### Where it goes (and why)


  You might expect CLAUDE.md to go into the system prompt. It doesn't.


  The system prompt is the same for every Claude Code user on the same version.
  That's what makes the shared prompt cache work — all users share the same
  cached prefix, so Anthropic only computes it once. If your CLAUDE.md went into
  the system prompt, every user with a different CLAUDE.md would need a separate
  cache. The economics of the product would break.


  So Claude Code needed a way to inject per-user instructions without touching
  the system prompt. The solution is `<system-reminder>` — an XML tag that gets
  attached to conversation messages. The model treats it as high-priority
  context (the label says "These instructions OVERRIDE any default behavior"),
  but it lives in the messages array, not the system prompt. The prefix stays
  frozen, the cache stays shared.


  Here's what it looks like in the actual API request:


  ```

  <system-reminder>

  As you answer the user's questions, you can use the following context:

  # claudeMd

  Contents of /path/to/CLAUDE.md (user's project instructions):


  [your CLAUDE.md content here]


  # currentDate

  Today's date is 2026-03-18.

  </system-reminder>

  ```


  This is the same pattern Thariq described in his prompt caching thread: when
  information changes between turns (dates, file contents, git status), send it
  as a system-reminder instead of modifying the system prompt. The system prompt
  stays frozen. The cache stays warm. Your custom instructions still reach the
  model.


  ### CLAUDE.md vs. rules


  CLAUDE.md isn't the only place you can put instructions. Claude Code also
  supports `.claude/rules/` — a directory of individual rule files, each with
  optional `paths:` frontmatter that scopes it to specific file patterns.


  The difference is when they load. CLAUDE.md is injected once at the start of
  your conversation. Rules are re-injected as system-reminders every time Claude
  accesses a file that matches their path pattern. If you have a rule scoped to
  `**/*.py`, it gets attached to every Python file read or edit as a fresh
  reminder.


  This makes rules useful for context-specific instructions. "Always use
  `pytest` fixtures, never `setUp`/`tearDown`" makes sense as a rule scoped to
  test files — it shows up exactly when Claude is working on tests. Your
  project's coding conventions, architecture decisions, and general preferences
  belong in CLAUDE.md, where they load once and benefit from caching.


  The tradeoff: rules pay a per-tool-call cost because they're re-injected each
  time. If you have instructions that apply broadly, CLAUDE.md is the cheaper
  place to put them.


  ## Seeing it for yourself


  Run `/context` at the start of your next session. It shows you the breakdown
  of where your context is going — system prompt, tools, CLAUDE.md,
  conversation.


  If you want to see the actual system prompt text, the Piebald-AI repo has the
  full extraction for every release. Reading through it is worth the time.
  You'll recognize behaviors you've seen in Claude Code and understand where
  they come from.


  If you want to measure your own setup's overhead, run:


  ```

  claude -p --output-format json "What is 2+2?" 2>/dev/null | python3 -c "

  import json, sys

  data = json.loads(sys.stdin.read())

  u = data.get('usage', {})

  total = u.get('input_tokens',0) + u.get('cache_creation_input_tokens',0) +
  u.get('cache_read_input_tokens',0)

  print(f'Total context before your message: {total:,} tokens')

  print(f'That is {total/200000*100:.1f}% of a 200K context window')

  "

  ```


  Try it from different directories. The delta between a bare directory and your
  project directory is the cost of your configuration.


  If you've read the earlier deep dives on prompt caching, tool use, and context
  management, this is the piece that ties them together. The system prompt is
  where those mechanisms originate - the cache breakpoints, the tool schemas,
  the compaction triggers. Knowing what's in there turns Claude Code from a
  black box into a tool you can reason about.
