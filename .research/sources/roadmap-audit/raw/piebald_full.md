url: https://github.com/Piebald-AI/claude-code-system-prompts
title: "GitHub - Piebald-AI/claude-code-system-prompts: All parts of Claude
  Code's system prompt, 27 builtin tool descriptions, sub agent prompts
  (Plan/Explore/Task), utility prompts (CLAUDE.md, compact,  statusline, magic
  docs, WebFetch, Bash cmd, security review, agent creation).  Updated for each
  Claude Code version."
content: >-
  ### Check out Piebald


  We've released **Piebald**, the ultimate agentic AI developer experience.   
   Download it and try it out for free! **https://piebald.ai/**

  **Scroll down for Claude Code's system prompts.** 👇


  Tip


  **NEW (June 12, 2026):** We've greatly expanded this list with many more of
  Claude Code's prompts—**from 350 to 515 (+165)**—our most complete coverage
  yet.


  This repository contains an up-to-date list of all Claude Code's various
  system prompts and their associated token counts as of **Claude Code v2.1.301
  (October 15th, 2026).** It also contains a **CHANGELOG.md** for the system
  prompts across 315 versions since v2.0.14. From the team behind **Piebald.**


  **This repository is updated within minutes of each Claude Code release. See
  the changelog, and follow @PiebaldAI on X for a summary of the system prompt
  changes in each release.**


  Note


  ⭐ **Star** this repository to get notified about new Claude Code versions. For
  each new Claude Code version, we create a release on GitHub, which will notify
  all users who've starred the repository.


  * * *


  Why multiple "system prompts?"


  **Claude Code doesn't just have one single string for its system prompt.**


  Instead, there are:


  *   Large portions conditionally added depending on the environment and
  various configs.

  *   Descriptions for builtin tools like `Write`, `Bash`, and `TodoWrite`, and
  some are fairly large.

  *   Separate system prompts for builtin agents like Explore and Plan.

  *   Numerous AI-powered utility functions, such as conversation compaction,
  `CLAUDE.md` generation, session title generation, etc. featuring their own
  systems prompts.


  The result—500+ strings that are constantly changing and moving within a very
  large minified JS file.


  Important


  Want to **modify a particular piece of the system prompt** in your own Claude
  Code installation? **Use tweakcc.** It—


  *   lets you customize the individual pieces of the system prompt as markdown
  files, and then

  *   patches your npm-based or native (binary) Claude Code installation with
  them, and also

  *   provides diffing and conflict management for when both you and Anthropic
  have conflicting modifications to the same prompt file.


  ## Extraction


  This repository contains the system prompts extracted using a script from the
  latest npm version of Claude Code. As they're extracted directly from Claude
  Code's compiled source code, they're guaranteed to be exactly what Claude Code
  uses. If you use tweakcc to customize the system prompts, it works in a
  similar way—it patches the exact same strings in your local installation as
  are extracted into this repository.


  ## Prompts


  Note that some prompts contain interpolated bits such as builtin tool name
  references, lists of available sub agents, and various other context-specific
  variables, so the actual counts in a particular Claude Code session will
  differ slightly—likely not beyond ±20 tokens, however.


  ### Agent Prompts


  Sub-agents and utilities.


  #### Sub-agents


  *   Agent Prompt: Explore (**862** tks) - System prompt for the Explore
  subagent.

  *   Agent Prompt: Plan mode (enhanced) (**1066** tks) - Enhanced prompt for
  the Plan subagent.


  #### Creation Assistants


  *   Agent Prompt: CLAUDE.md creation (**631** tks) - Instructs the /init
  command to analyze a codebase and create or improve a CLAUDE.md file.

  *   Agent Prompt: Status line setup (**4815** tks) - System prompt for the
  statusline-setup agent that configures status line display.


  #### Slash Commands


  *   Agent Prompt: /batch slash command (**1524** tks) - Instructions for
  orchestrating a large, parallelizable change across a codebase.

  *   Agent Prompt: /code-review GitLab comment posting (**312** tks) - Directs
  code review findings to a general GitLab merge request note when the comment
  flag is enabled.

  *   Agent Prompt: /code-review inline gap sweep phase (**105** tks) - Adds a
  final same-context sweep for defects missed by an inline code review when
  subagents are unavailable.

  *   Agent Prompt: /code-review minimal mode (**632** tks) - Minimal
  /code-review prompt that performs one careful diff pass and reports up to
  fifteen concrete correctness findings.

  *   Agent Prompt: /code-review part 1 base finder angles (**145** tks) -
  Line-by-line diff scan instructions for the /code-review slash command's
  finder-angle phase.

  *   Agent Prompt: /code-review part 10 ReportFindings output format (**448**
  tks) - Output-format instructions for /code-review runs that report verified
  findings once through the ReportFindings tool with capped, severity-ranked
  findings.

  *   Agent Prompt: /code-review part 2 low effort minimum findings mode
  (**740** tks) - Low-effort /code-review prompt that reads the diff once,
  targets at least min(files_changed, 4) hunk-visible runtime correctness
  findings, and performs one extra pass when short.

  *   Agent Prompt: /code-review part 2 low effort mode (**778** tks) -
  Low-effort /code-review prompt that reads the diff once and returns up to four
  hunk-visible runtime correctness findings.

  *   Agent Prompt: /code-review part 3 extra-high and maximum effort modes
  (**553** tks) - Extra-high and maximum-effort /code-review prompt that runs
  five finder angles, one-vote verification, a gap sweep, and capped JSON
  findings.

  *   Agent Prompt: /code-review part 4 three-state verification phase (**98**
  tks) - Verification phase for /code-review that asks one agent verifier to
  classify each candidate as confirmed, plausible, or refuted.

  *   Agent Prompt: /code-review part 5 recall-biased verification phase
  (**175** tks) - Recall-biased /code-review verification phase that treats
  realistic uncertain findings as plausible unless code refutes them.

  *   Agent Prompt: /code-review part 6 medium effort mode (**466** tks) -
  Medium-effort /code-review prompt that favors precision with three finder
  angles, one-vote verification, and up to eight JSON findings.

  *   Agent Prompt: /code-review part 7 high effort mode (**511** tks) -
  High-effort /code-review prompt that favors recall with three finder angles,
  recall-biased verification, and up to ten JSON findings.

  *   Agent Prompt: /code-review part 8 GitHub comment posting (**152** tks) -
  Optional /code-review instructions for posting findings as GitHub inline PR
  comments when --comment is passed.

  *   Agent Prompt: /code-review part 9 fix application (**250** tks) - Optional
  /code-review instructions for applying findings to the working tree when --fix
  is passed.

  *   Agent Prompt: /code-review unavailable-agent inline mode (**403** tks) -
  Builds a single-pass inline code-review prompt that gathers a diff, evaluates
  configured angles, deduplicates findings, optionally sweeps gaps, and reports
  capped results when the Agent tool is unavailable.

  *   Agent Prompt: /rename auto-generate session name (**80** tks) - Prompt
  used by /rename (no args) to auto-generate a kebab-case session name from
  conversation context.

  *   Agent Prompt: /schedule slash command (**4469** tks) - Guides the user
  through scheduling, updating, listing, or running remote Claude Code agents on
  cron triggers via the Anthropic cloud API.

  *   Agent Prompt: /security-review slash command (**2521** tks) -
  Comprehensive security review prompt for analyzing code changes with focus on
  exploitable vulnerabilities.

  *   Agent Prompt: /simplify slash command (**362** tks) - Instructions for the
  /simplify slash command that reviews changed code for reuse, simplification,
  efficiency, and altitude cleanups, then applies the fixes.

  *   Agent Prompt: /simplify unavailable-agent inline mode (**621** tks) - Runs
  the /simplify cleanup workflow inline across reuse, simplification,
  efficiency, and altitude angles when the Agent tool is unavailable.

  *   Agent Prompt: Artifact type creation slash command (quickstart flow)
  (**277** tks) - Directs an Artifact creation slash command to quickstart the
  requested published type, create the new Artifact, and follow the returned
  type instructions.

  *   Agent Prompt: Artifact type creation slash command (**347** tks) - Directs
  an Artifact creation slash command to discover the named published Artifact
  type, create from its type URL, and follow the returned type instructions.


  #### Utilities


  *   Agent Prompt: Agent Hook (**275** tks) - Evaluates agent hook conditions
  against the codebase and, when available, the conversation transcript, then
  returns a structured pass/fail result.

  *   Agent Prompt: Artifact comment completion reply and resolution (**256**
  tks) - Directs an Artifact comment worker to post one completion reply when
  needed and resolve an open thread after acting unless the conversation remains
  active.

  *   Agent Prompt: Artifact comment thread analyst (**750** tks) - Instructs a
  read-only agent to analyze one Artifact comment thread and return a
  constrained analysis brief for a separate composer.

  *   Agent Prompt: Artifact comment thread background directive (**300** tks) -
  Directs a forked background agent to handle one Artifact comment thread on its
  own, leaving the main session's pending work alone and treating thread and
  artifact content as material rather than instructions.

  *   Agent Prompt: Artifact editor thread follow-up (**328** tks) - Delivers a
  tagged Artifact thread message to the active editor worker, requiring
  page-scoped edits and republishing for relevant requests and no changes for
  unrelated ones.

  *   Agent Prompt: Auto mode rule reviewer (**292** tks) - Reviews and
  critiques user-defined auto mode classifier rules for clarity, completeness,
  conflicts, and actionability.

  *   Agent Prompt: Away summary generation (**73** tks) - Prompts a no-tools
  away-summary generation run to recap the goal, current task, and next action
  when the user returns.

  *   Agent Prompt: Background agent state classifier ask-opens-message rule
  (**528** tks) - Classifier section for agents told to state their one needed
  ask first: read the whole tail, treat an unanswered question or need as a
  blocked gate wherever it sits, with DNS-cutover examples.

  *   Agent Prompt: Background agent state classifier chat-thread relay rule
  (**1006** tks) - Classifier addendum for sessions that reach people only
  through posts in a chat thread: a named person counts as the user, an open ask
  stays blocked even beside a scheduled check-in, and third-party waits stay
  done.

  *   Agent Prompt: Background agent state classifier (**6237** tks) -
  Classifies the tail of a background agent transcript as working, blocked,
  done, or failed and returns concise state JSON.

  *   Agent Prompt: Background job agent instructions (**581** tks) - Instructs
  the built-in background job agent to narrate progress, restate tool results,
  and emit explicit result, needs input, or failed status signals.

  *   Agent Prompt: Claude Code guide (**408** tks) - Describes when to use the
  Claude Code guide agent for Claude Code, Agent SDK, Claude API, and Claude Tag
  questions.

  *   Agent Prompt: Claude Test author (**346** tks) - Subagent definition for
  the Claude Test plugin's background spec author, which reads the app's source
  and writes spec drafts into a run folder's proposed/ directory for the person
  to approve.

  *   Agent Prompt: Claude Test explorer (**793** tks) - Subagent definition for
  the Claude Test plugin's read-only explorer, which maps a web application's
  source for a browser test suite using only Read, Grep and Glob and must avoid
  credential and out-of-project paths.

  *   Agent Prompt: Claude guide agent (**2975** tks) - Instructs the Claude
  guide agent to answer documentation-based questions about Claude Code, the
  Claude Agent SDK, the Claude API, Claude Tag, plugin evaluation, and skill
  diagnostics.

  *   Agent Prompt: Coding session title generator (**898** tks) - Generates a
  short noun-phrase title for a coding session from untrusted session content.

  *   Agent Prompt: Conversation summarization (capped variant) (**166** tks) -
  Asks for a summary of the entire conversation in 2000 words or fewer inside
  summary tags, preserving facts, file paths, function names, error messages and
  open threads.

  *   Agent Prompt: Conversation summarization (lean variant) (**486** tks) -
  Tells a compaction run that its reply replaces the conversation, lists what
  the next context window will carry, and requires the summary to preserve the
  user's goals, decisions, ruled-out approaches, exact limits, current status
  and next step.

  *   Agent Prompt: Conversation summarization (short variant) (**400** tks) -
  Asks for a no-tools summary block that preserves problems, options, decisions,
  constraints, current status, open items and exact details, weighting the
  user's words over Claude's own reasoning.

  *   Agent Prompt: Conversation summarization with additional instructions
  (**1536** tks) - Extended summarization prompt with support for custom
  additional instructions.

  *   Agent Prompt: Conversation summarization (**292** tks) - System prompt for
  creating detailed conversation summaries.

  *   Agent Prompt: Coordinator worker instructions (**830** tks) - Instructions
  for worker agents executing coordinator-assigned tasks, covering scope
  control, concurrent branch changes, resumption, failure handling, and
  coordinator-facing output.

  *   Agent Prompt: Determine which memory files to attach (**388** tks) - Agent
  for determining which memory files to attach for the main agent.

  *   Agent Prompt: Dream memory consolidation (**1214** tks) - Instructs an
  agent to perform a multi-phase memory consolidation pass — orienting on
  existing memories, gathering recent signal from logs and transcripts, merging
  updates into topic files, and pruning the index.

  *   Agent Prompt: General purpose agent (**63** tks) - Defines a
  general-purpose agent for researching complex questions, searching code, and
  executing multi-step tasks.

  *   Agent Prompt: General purpose (**446** tks) - System prompt for the
  general-purpose subagent that searches, analyzes, and edits code across a
  codebase while reporting findings concisely to the caller.

  *   Agent Prompt: General task agent (**99** tks) - Instructs a Claude Code
  task agent to complete the user's request fully and report the essential
  outcome.

  *   Agent Prompt: Hook condition evaluator (stop) (**510** tks) - System
  prompt for evaluating hook conditions, specifically stop conditions, in Claude
  Code.

  *   Agent Prompt: Hook condition evaluator (**155** tks) - Instructs an agent
  to decide whether a hook's user-provided check lets an action go ahead,
  returning an ok/reason JSON verdict.

  *   Agent Prompt: Inherited context for worktree sub-agent (**121** tks) -
  Briefs a sub-agent that it has inherited a parent session's context and is now
  working in its own isolated git worktree.

  *   Agent Prompt: Onboarding guide draft share link workflow (**423** tks) -
  Adds instructions for sharing the draft ONBOARDING.md before review, then
  updating the same ShareOnboardingGuide link after the user answers the review
  questions.

  *   Agent Prompt: Onboarding guide generator (**1135** tks) - Co-authors a
  team onboarding guide (ONBOARDING.md) for new Claude Code users by analyzing
  the creator's usage data, classifying session types, and iterating on the
  draft collaboratively.

  *   Agent Prompt: PR follow-up cron (PR Steward label check) (**278** tks) -
  PR follow-up cron clause telling the agent not to fix or push a PR carrying PR
  Steward labels, to tell the user once, and to offer leave-it, one-change, or
  take-over choices.

  *   Agent Prompt: PR follow-up cron (**332** tks) - Cron prompt for checking a
  pull request created in the session and fixing failures, comments, or
  conflicts.

  *   Agent Prompt: Parent messages and user authorization (lean rewrite)
  (**143** tks) - Clarifies that parent-agent messages direct the task but
  cannot authorize permission, configuration, or CLAUDE.md changes.

  *   Agent Prompt: Plugin eval pilot trust requirement (**205** tks) - Requires
  explicit user trust for a plugin directory before piloting its eval cases with
  --trust-plugin and otherwise limits work to writing case files.

  *   Agent Prompt: Project thread status card classifier ask-opens-message rule
  (**346** tks) - Status-card classifier section for threads told to state their
  one needed ask first: an unanswered question or need sets needs_reply or
  needs_approval wherever it sits, with a DNS-cutover example.

  *   Agent Prompt: Project thread status card classifier (**4299** tks) -
  Classifies a Project thread as needing a reply, needing approval, done,
  failed, or working and emits concise owner-facing status-card JSON.

  *   Agent Prompt: Prompt Suggestion Generator v2 (**344** tks) - V2
  instructions for generating prompt suggestions for Claude Code.

  *   Agent Prompt: Pull request creation (**1151** tks) - Prompt for creating a
  single GitHub pull request from existing commits with branch, template,
  attribution, shell-formatting, and git-safety guidance.

  *   Agent Prompt: Quick PR creation (**1741** tks) - Streamlined prompt for
  creating a commit and pull request with pre-populated context.

  *   Agent Prompt: Quick git commit (**1312** tks) - Streamlined prompt for
  creating a single git commit with pre-populated context.

  *   Agent Prompt: Read-only search agent (**93** tks) - Defines a read-only
  search agent for broad fan-out code searches that returns conclusions instead
  of file dumps.

  *   Agent Prompt: Recent Message Summarization (**868** tks) - Agent prompt
  used for summarizing recent messages.

  *   Agent Prompt: Remote machine auto mode rules (**228** tks) - Adds
  host-declared deny, allow, and environment rules to the security monitor
  prompt for commands that will run on another machine.

  *   Agent Prompt: Schedule action selection (**114** tks) - Instructs the
  cloud scheduling agent to ask the user which schedule action to perform first.

  *   Agent Prompt: Schedule local MCP server limitation (**1145** tks) -
  Explains that local Claude Code MCP servers cannot be attached to cloud
  routines and tailors Claude.ai connector guidance to why the connector list
  was unavailable.

  *   Agent Prompt: Security monitor Claude Tag connector writes (**542** tks) -
  Allows the autonomous-action security monitor to accept delegated writes
  through exact Claude Tag connector tool prefixes while preserving all other
  safety boundaries.

  *   Agent Prompt: Security monitor attached machine call results (**269** tks)
  - Explains how the auto mode security monitor reads _host call lines and
  result_from lines for calls served by a user-attached machine, and judges
  sending that machine's output to external destinations as cross-machine data
  exfiltration.

  *   Agent Prompt: Security monitor for autonomous agent actions (first part)
  (**13991** tks) - Instructs Claude to act as a security monitor that evaluates
  autonomous coding agent actions against block/allow rules to prevent prompt
  injection, scope creep, and accidental damage.

  *   Agent Prompt: Security monitor forwarded user turns (**761** tks) -
  Defines how the auto mode classifier authenticates harness-forwarded user
  turns and weighs direct, relayed, and unattributed messages as action-specific
  intent, consent, or restrictions.

  *   Agent Prompt: Security monitor host-context line guidance (**614** tks) -
  Explains how the autonomous-action security monitor should interpret live and
  restored host-context lines when evaluating user intent and consent.

  *   Agent Prompt: Session title and branch generation (**626** tks) - Agent
  for generating succinct session titles and git branch names.

  *   Agent Prompt: Session transcript chunk summary (**89** tks) - Instructs an
  agent to summarize a chunk of a Claude Code session transcript concisely.

  *   Agent Prompt: Subagent delegation configuration classifier (**474** tks) -
  Classifies a user's CLAUDE.md, installed skills and custom agent definitions
  as encouraging or not encouraging delegation to subagents, naming which
  sections say so while ignoring any instructions found inside the
  configuration.

  *   Agent Prompt: Subagent reporting notes (lean rewrite) (**411** tks) -
  Directs subagents to use absolute paths and return findings in their final
  message rather than writing report files.

  *   Agent Prompt: Summarization no-tools guard (**164** tks) - Shared prefix
  for compaction summarization agents that forbids tool use and requires a
  plain-text reply shaped as either an analysis block followed by a summary
  block or a summary block alone.

  *   Agent Prompt: Web fetch agent usage guidance (**699** tks) - Explains when
  and how to delegate URL reading to the built-in web-fetch agent, including
  binary-file trust boundaries and follow-up messaging.

  *   Agent Prompt: Web reading specialist (**962** tks) - System prompt for the
  built-in web-fetch agent that reads untrusted URL content with WebFetch and
  returns a focused, source-grounded report to its caller.

  *   Agent Prompt: Worker fork (**412** tks) - Directive injected into a forked
  child agent, telling it to treat the inherited transcript as reference,
  execute one directive directly, and report once.

  *   Agent Prompt: Workflow script plain text return note (**118** tks) -
  Appended note telling a workflow script agent that its final text response is
  parsed as the script return value.

  *   Agent Prompt: Workflow script structured return note (**218** tks) -
  Appended note telling a workflow script agent to return its final answer by
  calling the structured output tool exactly once.

  *   Agent Prompt: Workflow subagent plain text output (**232** tks) -
  Instructs an internal workflow subagent to return its final text verbatim as
  the calling workflow script's parsed result.

  *   Agent Prompt: Workflow subagent structured output (**340** tks) -
  Instructs an internal workflow subagent to return its final answer by calling
  the StructuredOutput tool exactly once with schema-valid input.


  ### Data


  The content of various template files embedded in Claude Code.


  *   Data: /auto-mode-setup usage (**406** tks) - Defines /auto-mode-setup
  proposal and reviewed-apply command syntax, request ID ordering, save-target
  validation, and integrity requirements.

  *   Data: Artifact MCP connector guidance (**351** tks) - Explains how
  Artifact MCP manifests declare claude.ai, built-in, and host connectors and
  how to discover exact server and upstream tool names.

  *   Data: Artifact capability verification pass (**317** tks) - Requires one
  functional pass over a page's session-declared runtime capabilities before
  handing over its link, assembling the preview, database read and endpoint
  checks to run and what to report to the user.

  *   Data: Artifact connector call observation requirement (**305** tks) -
  Explains how to obtain connector argument names from the loaded tool schema,
  learn result shapes from safe real calls, and disclose shapes that cannot be
  verified before publishing.

  *   Data: Artifact connector server naming guidance (**241** tks) - Explains
  how connector tool prefixes map to manifest server identifiers and why in-page
  calls must use the resolved connector display name.

  *   Data: Artifact decision component HTML skeleton (**1117** tks) - Provides
  the canonical HTML and JSON-island skeleton for embedding one reusable
  Artifact decision block.

  *   Data: Artifact host MCP server guidance (**420** tks) - Explains how
  locally configured MCP servers may be declared as Artifact host servers and
  distinguishes them from Claude.ai connectors and built-in servers.

  *   Data: Artifact publish merged-over newer version file changes notice
  (**481** tks) - Tool-result notice appended when an artifact publish was
  merged over a newer live version, listing files changed, added, or removed
  there since the base version, that stale copies must be read again, and how
  many further files are not listed.

  *   Data: Artifact runtime capability declarations (**496** tks) - Defines
  Artifact runtime capability declaration, carry-forward, clearing, replacement,
  and contract pinning semantics.

  *   Data: Auto-compact inputs changed event schema (**221** tks) - Schema
  description for worker-resolved auto-compaction state events used by thin
  clients to display the effective compaction countdown.

  *   Data: Background tasks changed event schema (**374** tks) - Schema
  description for the background_tasks_changed system event and its replace-set
  semantics.

  *   Data: Built-in gh stand-in api command help (**1149** tks) - Usage text
  for Claude Code's built-in gh stand-in, which supports only gh api REST
  requests to GitHub hosts through the session's GitHub proxy, listing its flags
  and how {owner}, {repo} and {branch} placeholders are filled in.

  *   Data: Chrome browser hints control request (**207** tks) - Documents the
  set_chrome_browser_hints control request that tells a session which connected
  Claude in Chrome browser to prefer, and that the hints are session-only and
  never override the live relay roster.

  *   Data: Claude API reference — PHP (**2877** tks) - PHP SDK reference.

  *   Data: Claude API reference — Ruby (**2120** tks) - Ruby SDK reference
  including installation, client initialization, basic requests, streaming, and
  beta tool runner.

  *   Data: Claude Code agent proxy troubleshooting guide (**2194** tks) -
  Troubleshooting guide for Claude Code's policy-enforcing HTTPS agent proxy,
  covering TLS trust setup, status checks, git, docker, and unsupported traffic.

  *   Data: Claude Code gateway customer-routed inference protocol (**2945**
  tks) - Conditional extension to the Claude Code gateway protocol defining
  customer-routed inference authentication, forwarding, response hygiene, error
  recovery, policy blocking, discovery, and endpoint requirements.

  *   Data: Claude Code gateway protocol (**7820** tks) - Markdown reference
  documenting the Claude Code gateway wire contract, including OAuth 2.0 device
  flow, RFC 8414 discovery, Messages API inference, managed settings, model
  discovery, OTLP telemetry, error envelopes, TLS certificate pinning, and
  proxying to Bedrock, Vertex, and Foundry.

  *   Data: Claude Code live documentation sources (**2477** tks) - WebFetch
  URLs for fetching current Claude Code documentation from official sources.

  *   Data: Claude Code recent changes reference (**1854** tks) - Reference
  mapping of recently removed or renamed Claude Code commands, flags, and terms
  to their current replacements.

  *   Data: Claude Platform on AWS reference (**1805** tks) - Reference
  documentation for using the Claude Developer Platform through AWS
  infrastructure, including AnthropicAWS clients, required region and workspace
  configuration, SigV4 authentication, and short-term API keys.

  *   Data: Claude Tag (Claude in Slack) reference (**1999** tks) - Offline
  reference for Claude Tag, Claude Code's org-managed Slack surface, covering
  what it is, availability, setup, configuration, and how it differs from the
  earlier Claude in Slack app.

  *   Data: Claude Test spec file format (**1758** tks) - Reference for the
  Markdown spec files in .claude-test/specs/ — front matter keys, the three
  required parts, data-creating specs, and what makes a spec goal-oriented and
  checkable from a single end screenshot.

  *   Data: Claude gateway landing page (**392** tks) - HTML status page served
  at a Claude Code gateway root, showing the gateway logo, the running gateway
  URL, the identity-provider host, an OAuth discovery link, and the gateway
  version.

  *   Data: Cloud session unverified remote branch push notice (**299** tks) -
  Notice shown when no remote of the checkout could be read, so it could not be
  checked whether the current branch or detached HEAD commit exists on the
  remote host, telling the user to push their work to a branch and start a new
  cloud session from it.

  *   Data: Code change published event schema (**349** tks) - Schema
  description for the code_change_published system event, including provenance,
  idempotency, trust boundaries, and best-effort delivery.

  *   Data: Command plugin source command field (**160** tks) - Describes the
  command field for command-based plugin sources, including its one-line
  directory output contract, cache-copy lifecycle, platform shell, and
  re-resolution behavior.

  *   Data: Cowork plugin MCP discovery and connection (**1746** tks) -
  Reference guidance for finding MCP connectors during plugin customization,
  using search and suggestion tools, mapping categories to keywords, and writing
  .mcp.json entries.

  *   Data: Cowork plugin examples (**3158** tks) - Reference examples of
  minimal, medium, and complex Cowork plugin structures with plugin metadata,
  skills, agents, hooks, MCP config, README, and connectors.

  *   Data: Cross-session inbound setting (**175** tks) - Describes
  crossSessionInbound delivery, hold, refusal, and permission-mode parity
  behavior for peer messages.

  *   Data: Data visualization anti-patterns (**2339** tks) - Reference list of
  chart and dashboard anti-patterns to check against before shipping a data
  visualization.

  *   Data: Data visualization choosing a form (**1177** tks) - Reference
  guidance for choosing the appropriate chart, stat tile, or non-chart form
  based on the data's job.

  *   Data: Data visualization components (**836** tks) - Reference
  specification for the HTML/SVG pieces, layers, and system parameters that make
  up a data visualization.

  *   Data: Data visualization interaction (**1318** tks) - Reference guidance
  for chart hover layers, tooltips, filters, linked highlighting, and
  interaction behavior.

  *   Data: Data visualization marks and anatomy (**2162** tks) - Reference
  specifications for chart marks, spacing, labels, axes, legends, and stat-tile
  anatomy.

  *   Data: Dialog expiry setting (**204** tks) - Describes dialogExpiry
  deadlines for remote permission dialogs and held cross-session messages,
  including defaults and overrides.

  *   Data: DirectoryAdded hook description (**216** tks) - Describes when the
  DirectoryAdded hook fires, its input fields, and how failures and output are
  handled for add-dir and register_repo_root sources.

  *   Data: Files API reference — C# (**378** tks) - C# Files API reference
  including beta file upload and use with beta message types.

  *   Data: Files API reference — Go (**413** tks) - Go Files API reference
  including file upload, listing, deletion, and usage in messages.

  *   Data: Files API reference — Java (**492** tks) - Java Files API reference
  including file upload, beta message references, listing, deletion, download,
  and metadata retrieval.

  *   Data: Files API reference — PHP (**186** tks) - PHP Files API reference
  with a beta-era upload example and a warning that current SDKs use the stable
  Files API shape.

  *   Data: Files API reference — Python (**1719** tks) - Python Files API
  reference including file upload, listing, deletion, and usage in messages.

  *   Data: Files API reference — TypeScript (**961** tks) - TypeScript Files
  API reference including file upload, listing, deletion, and usage in messages.

  *   Data: Gateway device code entry page (**261** tks) - HTML verification
  page served at a gateway device endpoint, prompting the user to enter the
  short device code shown by Claude Code so they can sign in through their
  company identity provider.

  *   Data: Gateway ignored X-Forwarded-For warning (**254** tks) - Gateway
  startup warning that an X-Forwarded-For header was ignored because the
  requesting address is not covered by listen.trusted_proxies, and what to
  configure when a load balancer or ingress fronts the gateway.

  *   Data: Get task output control request (**201** tks) - Schema description
  for the get_task_output control request that reads the last 8 KiB of a
  background shell or Monitor task's output without a model turn, and when it is
  refused.

  *   Data: GitHub Actions workflow for @claude mentions (**525** tks) - GitHub
  Actions workflow template for triggering Claude Code via @claude mentions.

  *   Data: GitHub App installation PR description (**409** tks) - Template for
  PR description when installing Claude Code GitHub App integration.

  *   Data: Hook classifier context field (**654** tks) - Schema description for
  the classifierContext hook response field, including trust semantics, size and
  timing limits, transcript visibility, and rewrite-integrity requirements.

  *   Data: Hook mcp_server field (**312** tks) - Describes the mcp_server
  object attached to tool-related hook payloads — the server's config key and
  the source its definition came from — and why trust must key on source rather
  than on the server name or tool-name prefix.

  *   Data: Interrupt cancel queued parameter (**646** tks) - Schema description
  for the optional interrupt cancel_queued request parameter and its
  queued-command cancellation semantics.

  *   Data: Interrupt receipt cancelled field (**402** tks) - Schema description
  for the cancelled UUID list returned when an interrupt request cancels queued
  commands.

  *   Data: Interrupt receipt still queued field (**1155** tks) - Schema
  description for the still_queued UUID list returned by interrupt control
  responses.

  *   Data: Interrupt send_now message_uuid parameter (**246** tks) - Schema
  description for the interrupt message_uuid parameter read only beside
  send_now, which restricts the request to one exact-match waiting user message
  and answers nothing_waiting when that message is not waiting.

  *   Data: Interrupt send_now parameter (**596** tks) - Schema description for
  the optional interrupt send_now request parameter, which asks that waiting
  user messages reach the model now without acting as a Stop, and how the CLI
  chooses between moving the turn's waits to the background and aborting it.

  *   Data: Interrupt worker_epoch parameter (**381** tks) - Schema description
  for the interrupt request's worker_epoch member, which binds a service-sent
  interrupt to one worker life and defines the stale_worker_epoch and
  invalid_worker_epoch refusals.

  *   Data: Knowledge MCP search strategies (**447** tks) - Reference query
  patterns for using knowledge MCPs to discover organization-specific tool
  names, project identifiers, team names, and workflow details during plugin
  customization.

  *   Data: MCP HTTP client capability validation rationale (**189** tks) -
  Explains where required client capabilities are validated in the modern MCP
  HTTP request ladder and how the pre-dispatch gate affects observable
  precedence.

  *   Data: MCP HTTP parameter header validation rationale (**247** tks) -
  Explains validation of Mcp-Param headers against tool declarations and request
  arguments, including error shape and pre-dispatch precedence.

  *   Data: MCP HTTP standard header validation rationale (**326** tks) -
  Explains the modern MCP HTTP validation ladder's Mcp-Method and Mcp-Name
  header checks and their observed pre-dispatch precedence.

  *   Data: MCP read resource control request (**397** tks) - Documents the
  mcp_read_resource control request that reads one MCP Apps (SEP-1865) ui://
  resource from a connected MCP server, its sandboxed-render requirement,
  SDK-server rejection, refused lanes and error cases.

  *   Data: MCP server status error_code field (**200** tks) - Schema
  description for the MCP server status error_code field, naming the
  host-fixable failure causes — a rejected claude.ai connector login, a rejected
  first-party Anthropic credential, and a project-scoped server awaiting
  approval — and noting that it is absent for every other failure and that new
  values are additive.

  *   Data: Managed Agents quickstart — Bug hunter (**2633** tks) - Bundled
  Managed Agents quickstart template for a bug hunter agent that splits a
  codebase into slices, has parallel readers and second-look reviewers find
  suspicions, and reports only bugs a separate prover agent demonstrated, with
  its agent.md definition, environment.yaml and session.yaml.

  *   Data: Managed Agents quickstart — Contract tracker (**689** tks) - Bundled
  Managed Agents quickstart template for a contract tracker agent that reads a
  Box contract, extracts clauses and dates, and creates Asana deadline and
  obligation tasks, with its agent.md definition and system prompt.

  *   Data: Managed Agents quickstart — Data analyst (**579** tks) - Bundled
  Managed Agents quickstart template for a data analyst agent that loads, cleans
  and charts datasets and can query Amplitude, with its agent.md definition and
  system prompt.

  *   Data: Managed Agents quickstart — Deep researcher (**798** tks) - Bundled
  Managed Agents quickstart template for a deep research agent that decomposes a
  question, cites primary sources and ends with a confidence-and-gaps section,
  with its agent.md definition and a user.define_outcome rubric.

  *   Data: Managed Agents quickstart — Explainer video maker (**1653** tks) -
  Bundled Managed Agents quickstart template for an explainer video maker agent
  that storyboards a topic, builds one scene per agent plus generated music, and
  joins them with ffmpeg into a captioned video, with its agent.md definition,
  environment.yaml and session.yaml.

  *   Data: Managed Agents quickstart — Field monitor (**813** tks) - Bundled
  Managed Agents quickstart template for a field monitor agent that scans
  technical blogs and writes a weekly Notion digest, with its agent.md
  definition and a Monday-morning cron deployment.

  *   Data: Managed Agents quickstart — Incident commander (**684** tks) -
  Bundled Managed Agents quickstart template for an incident commander agent
  that triages a Sentry alert, opens a Linear ticket and runs the Slack war
  room, with its agent.md definition and system prompt.

  *   Data: Managed Agents quickstart — Sprint retro facilitator (**870** tks) -
  Bundled Managed Agents quickstart template for a sprint retro facilitator
  agent that pulls a closed Linear sprint, reads Slack sentiment and writes a
  retro doc, with its agent.md definition and a twice-monthly cron deployment.

  *   Data: Managed Agents quickstart — Structured extractor (**545** tks) -
  Bundled Managed Agents quickstart template for a structured extractor agent
  that parses unstructured text into JSON that validates against a target
  schema, with its agent.md definition and system prompt.

  *   Data: Managed Agents quickstart — Support agent (**532** tks) - Bundled
  Managed Agents quickstart template for a support agent that answers from
  Notion docs with quoted sources and escalates low-confidence questions to
  Slack, with its agent.md definition and system prompt.

  *   Data: Managed Agents quickstart — Support-to-eng escalator (**709** tks) -
  Bundled Managed Agents quickstart template for an escalator agent that reads
  an Intercom conversation, reproduces the bug and files a linked Jira issue
  with a Slack note, with its agent.md definition and system prompt.

  *   Data: Managed Agents quickstart — Watchlist scanner (**1854** tks) -
  Bundled Managed Agents quickstart template for a watchlist scanner agent that
  scans each ticker for changes since the last close, has a second agent review
  every ticker before the report, and avoids investment advice, with its
  agent.md definition, environment.yaml and session.yaml.

  *   Data: Managed settings helper onFailure field (**593** tks) - Schema
  description for the managed settings helper onFailure field, including
  source-dependent defaults, startup refusal behavior, static fallbacks, and
  background refresh handling.

  *   Data: Managed settings helper retries field (**311** tks) - Schema
  description for the managed settings helper retries field, including retryable
  failures, limits, per-attempt timeouts, backoff, and terminal failure
  handling.

  *   Data: Managed settings x-whenUnreadable field (**179** tks) - Describes
  managed-settings schema behavior for invalid present values, including
  fallback values, unusable keys, and default treatment as unset.

  *   Data: Message Batches API reference — C# (**188** tks) - C# Batches API
  reference including batch creation, status polling, and result retrieval.

  *   Data: Message Batches API reference — PHP (**196** tks) - PHP Batches API
  reference including batch creation, status polling, and result retrieval.

  *   Data: Message Batches API reference — Python (**2088** tks) - Python
  Batches API reference including batch creation, status polling, and result
  retrieval at 50% cost.

  *   Data: Message Batches API — TypeScript (**1022** tks) - TypeScript usage
  guide for Claude's asynchronous Message Batches endpoint.

  *   Data: Network-path settings file unread recovery guidance (**317** tks) -
  Guidance appended to plugin uninstall results when a settings file could not
  be read because its path may cross a link to another machine, telling the
  person to replace the link or use the real location before retrying.

  *   Data: OpenTelemetry tracing reference — C# (**970** tks) - Provides C#
  examples for registering a tracer provider, configuring Anthropic SDK tracing,
  and propagating context to connect SDK and API spans.

  *   Data: OpenTelemetry tracing reference — Go (**804** tks) - Provides Go
  examples for registering a tracer provider, configuring Anthropic SDK tracing,
  and propagating context to connect SDK and API spans.

  *   Data: OpenTelemetry tracing reference — Java (**987** tks) - Provides Java
  examples for registering a tracer provider, configuring Anthropic SDK tracing,
  and propagating context to connect SDK and API spans.

  *   Data: OpenTelemetry tracing reference — PHP (**1015** tks) - Provides PHP
  examples for registering a tracer provider, configuring Anthropic SDK tracing,
  and propagating context to connect SDK and API spans.

  *   Data: OpenTelemetry tracing reference — Python (**970** tks) - Provides
  Python examples for registering a tracer provider, configuring Anthropic SDK
  tracing, and propagating context to connect SDK and API spans.

  *   Data: OpenTelemetry tracing reference — Ruby (**668** tks) - Provides Ruby
  examples for registering a tracer provider, configuring Anthropic SDK tracing,
  and propagating context to connect SDK and API spans.

  *   Data: OpenTelemetry tracing reference — TypeScript (**888** tks) -
  Provides TypeScript examples for registering a tracer provider, configuring
  Anthropic SDK tracing, and propagating context to connect SDK and API spans.

  *   Data: Peer sender display name field (**199** tks) - Schema description
  for the normalized display name on cross-session peer message senders.

  *   Data: Plugin manifest types field (**214** tks) - Schema description for
  the plugin manifest types field naming a self-contained .d.ts type contract
  for the noun the plugin's hooks module adds in engine.create, which dependent
  mods receive under .claude-plugin/types and claude plugin validate checks.

  *   Data: Prompt suggestions paused control request (**229** tks) - Documents
  the set_prompt_suggestions_paused control request that pauses or resumes a
  session's prompt suggestions at runtime and the process-local, non-persisted
  scope of that state.

  *   Data: Query result pending command count (**167** tks) - Describes the
  query result field that counts queued user sends still awaiting turns and
  clarifies when it is absent or zero.

  *   Data: Rate limit grace signal (**306** tks) - Describes the usage-limit
  grace-window signal tracked from the latest served response, including when it
  sets, clears, and how it interacts with overage status at hard exhaustion.

  *   Data: Rate limit unified windows (**325** tks) - Describes per-window
  subscription rate-limit utilization and reset data derived from unified
  rate-limit response headers.

  *   Data: Remote tools reannounce control request (**220** tks) - Documents
  the remote_tools_reannounce control request a replacement worker sends so the
  attached client re-announces a machine missing from its tool roster, including
  worker-epoch, once-per-epoch, ten-second and deadline rules.

  *   Data: Review upload excluded changes error (**327** tks) - Error shown
  when a review upload has no new commits or uploadable changes because every
  uncommitted file was excluded from the git bundle.

  *   Data: Rewind files skippedLinks field (**223** tks) - Describes the
  rewindFiles skippedLinks count, including link-safety refusal semantics and
  dry-run behavior.

  *   Data: SDK API error kind field (**1543** tks) - Documents the typed
  api_error kinds carried on SDK assistant messages that wrap an API error, and
  what each kind means for replaying, rewinding, or remedying the request.

  *   Data: SDK MCP server disableAutoBackground field (**180** tks) - Schema
  description for the SDK MCP server disableAutoBackground option that stops
  calls to a server's tools from being moved to a background task, for servers
  whose calls wait on a person.

  *   Data: SDK api_error_params field (**668** tks) - Schema description for
  the api_error_params object, listing the per-kind parameters (effort, media,
  media_reason, provider, remedy, rate_limit_info) and their meanings.

  *   Data: SDK footer indicator schema (**249** tks) - Schema description for
  the SDK footer indicator, its environment-variable and cached-bootstrap
  sources, and its delivery on system/init and initialize responses so host UIs
  can render the terminal status pill.

  *   Data: SDK frame_intake_phases_ms field (**444** tks) - Schema description
  for the SDK turn-timing frame_intake_phases_ms field, breaking the wait
  between frame receipt and enqueue into named steps and their sum and presence
  invariants.

  *   Data: SDK frame_received_wall_ms field (**193** tks) - Schema description
  for the SDK turn-timing frame_received_wall_ms field recording when the
  triggering send's frame arrived on a session reading input from the session
  server's SSE stream, how it pairs with the enqueue and turn-start timestamps
  to split transit from queue wait, and when it is absent.

  *   Data: SDK initialize response sdk_mcp_manifests_parked field (**482** tks)
  - Schema description for the SDK initialize response sdk_mcp_manifests_parked
  field, reporting per server whether a manifest entry was parked, already
  connected, mismatched on protocol version, malformed, or not honoured, and
  when the field is absent.

  *   Data: SDK message_stop abandoned_blocks field (**375** tks) - Schema
  description for the abandoned_blocks marker on a Claude Code-produced
  message_stop, telling consumers which streamed content blocks will never get
  an assistant message and may be removed.

  *   Data: SDK permission_check_status event schema (**279** tks) - Schema
  description for the permission_check_status event that tells hosts a tool call
  is still waiting on the auto-mode classifier, with checking/done pairing and
  best-effort display-only semantics.

  *   Data: SDK read_file anchor field (**334** tks) - Schema description for
  the read_file request's anchor field, which resolves a relative path against
  workspace_root, diff_root or a declared repo checkout instead of the live cwd,
  with the refusal rules.

  *   Data: SDK result safety_stops field (**660** tks) - Schema description for
  the result message's safety_stops running count of model calls stopped by a
  safety system, covering what is counted, what is not, and why absent differs
  from 0.

  *   Data: SDK safeguard_results field (**517** tks) - Schema description for
  the safeguard_results member carrying a server-side classifier's
  dangerous_tool_use verdict, covering when a worker honours it, how it is
  matched to the call, and what is kept of the explanation.

  *   Data: SDK safeguard_tool_use field (**334** tks) - Schema description for
  the safeguard_tool_use member naming the tool_use block a safeguard verdict
  applies to, with the parsed-JSON equality comparison and size limits that void
  the verdict.

  *   Data: SDK set max thinking tokens request schema (**477** tks) - Schema
  description for the SDK set_max_thinking_tokens control request, covering null
  reset versus omitted budget, session-scoped thinking_display overrides, and
  when the highlights display mode is accepted, downgraded to omitted, or
  refused.

  *   Data: SDK set model only_if_model_is_current field (**464** tks) - Schema
  description for the internal set_model only_if_model_is_current flag, which
  makes the request a system_prompt-only update that requires a model naming the
  session's current model, how model equivalence is judged, and the
  invalid_request and model_not_current failures.

  *   Data: SDK set model system prompt field (**243** tks) - Schema description
  for the internal set_model system_prompt field, including next-turn
  application with unchanged tool definitions, non-empty updates, feature
  overrides, and compatibility behavior.

  *   Data: SDK system init plugin_errors field (**315** tks) - Schema
  description for the plugin_errors array on the SDK system/init message,
  covering failed versus partially loaded plugins, name@marketplace and
  positional inline/synced tags, open-set error types, omission when clean, and
  Remote Control workers always omitting it.

  *   Data: SDK task notification handback field (**205** tks) - Schema
  description for the handback field on a completed task notification from a
  subagent that reports through the SubagentHandback tool, including when it is
  omitted and how summary then reads.

  *   Data: SDK thinking_duration_ms field (**184** tks) - Schema description
  for the display-only thinking_duration_ms wrapper field giving how long a
  streamed thinking block took, excluding the wait for the first token, and when
  it is absent or not replayed to the model.

  *   Data: SDK ui_prompt_autocomplete request schema (**195** tks) - Schema
  description for the ui_prompt_autocomplete request in which a remote surface's
  Composer asks the engine to raise prompt.autocomplete for the token at the
  caret, including per-client ordering and superseded handling.

  *   Data: Sandbox TLS termination setting (**199** tks) - Describes the
  experimental sandbox.network.tlsTerminate setting, certificate behavior,
  Windows trust requirements, source precedence, and initialization validation.

  *   Data: Sandbox credential environment no-match setting (**188** tks) -
  Describes sandbox credential environment-variable onExtractNoMatch behavior,
  including warn, deny, error, and decode-path constraints.

  *   Data: Sandbox credential file mask claims setting (**179** tks) -
  Describes sandbox credential file maskClaims behavior for selectively masking
  decoded JWT payload claims while preserving other claims.

  *   Data: Sandbox credential mask empty injectHosts warning (**174** tks) -
  Warns that sandbox credential masks with empty injectHosts expose only
  sentinel values and explains how to resolve adapter, managed-settings, and
  local configuration causes.

  *   Data: Sandbox deniedResolvedAddresses setting (**209** tks) - Schema
  description for the sandbox deniedResolvedAddresses setting listing IP
  addresses or CIDR ranges an allowed hostname must not resolve to, its
  interaction with allowedDomains IP literals, and its exemption for parentProxy
  and mitmProxy routes.

  *   Data: Sandbox filesystem disabled setting (**227** tks) - Describes
  sandbox.filesystem.disabled behavior, platform limits, read-protection
  effects, and configuration precedence.

  *   Data: Sandbox network domain spelling warning (**278** tks) - Formats the
  sandbox diagnostic for unreliable network domain spellings and explains
  conservative enforcement until entries are corrected.

  *   Data: Sandbox root without CAP_SETFCAP error (**140** tks) - Linux sandbox
  dependency error explaining that running as uid 0 without CAP_SETFCAP makes
  every sandboxed command fail while writing a uid map on kernels 5.12+ and
  backports, and telling the user to grant CAP_SETFCAP or run as non-root.

  *   Data: Self-hosted runner Anthropic git proxy credential warning (**338**
  tks) - Runner warning that sources on hosts not routed through
  Anthropic-managed git clone with this machine's credentials, which
  --use-anthropic-git-proxy wipes from HOME-level git config, so credentials
  must live in /etc/gitconfig, GIT_ASKPASS, or an SSH key.

  *   Data: Self-hosted runner GIT_ASKPASS governed hosts warning (**311** tks)
  - Runner warning that a GIT_ASKPASS program answers for every host, so later
  git config could send this machine's credential for governed hosts through
  Anthropic's relay, and directs scoping or unsetting it along with core.askPass
  and SSH_ASKPASS.

  *   Data: Self-hosted runner GIT_SSL_CAINFO trust bundle warning (**254** tks)
  - Runner warning that GIT_SSL_CAINFO prevented building the combined
  certificate file for Anthropic-managed git, giving the failure cause and first
  remedy and directing the company CA into a per-server http sslCAInfo entry.

  *   Data: Self-hosted runner GIT_SSL_NO_VERIFY lifecycle hook exception
  (**330** tks) - Runner warning addendum explaining that checkout and
  post-session hooks of sessions with a repository on Anthropic-managed git get
  http.sslVerify=false with certificate checks re-enabled for the mount, while
  other sessions' hooks inherit the switch.

  *   Data: Self-hosted runner GIT_SSL_NO_VERIFY session relocation note
  (**325** tks) - Runner warning addendum explaining that inside a session
  GIT_SSL_NO_VERIFY becomes http.sslVerify=false that no longer covers the git
  mount or governed hosts, that per-server sslVerify takes precedence, and to
  name a company CA on certificate failures.

  *   Data: Self-hosted runner client certificate relay warning (**539** tks) -
  Runner warning that TLS client certificate variables set on the runner would
  be offered to the session relay rather than governed hosts, with remedies of
  asking for direct listing, unsetting the variables, or scoping sslCert and
  sslKey per URL.

  *   Data: Self-hosted runner decode-token command help (**869** tks) -
  Documents self-hosted runner token decoding, redaction, validation, input, and
  output command-line options.

  *   Data: Self-hosted runner deferred shutdown timing notice (**320** tks) -
  Explains deferred shutdown release and drain timing, supervisor timeout
  sizing, forced-kill consequences, and second-signal behavior for self-hosted
  runners.

  *   Data: Self-hosted runner disabled-by-organization fatal message (**208**
  tks) - Runner fatal message printed when RegisterRunner is refused because
  self-hosted environments are not enabled for the organization, pointing to the
  Allow self-hosted environments admin setting and Team or Enterprise plan
  requirement, and noting the environment secret was accepted and should not be
  rotated.

  *   Data: Self-hosted runner git-lfs hook warning (**342** tks) - Runner
  warning that git-lfs's pre-push hook never runs inside lifecycle hooks, so a
  post-session git push sends LFS pointers without their objects, and how to
  push the objects or point core.hooksPath instead.

  *   Data: Self-hosted runner orchestrator command help (**1554** tks) -
  Documents self-hosted runner orchestrator connection, hook, SCM connector,
  runtime, health, and debug command-line options.

  *   Data: Self-hosted runner session metrics help (**622** tks) - Defines
  Prometheus HELP text for self-hosted runner session lifecycle counters.

  *   Data: Streaming reference — C# (**376** tks) - C# streaming reference
  including streaming events and the RawMessageStreamEvent TryPick methods.

  *   Data: Streaming reference — Go (**452** tks) - Go streaming reference
  including event handling, error checks, and final-message accumulation.

  *   Data: Streaming reference — Java (**287** tks) - Java streaming reference
  including streamed content-block delta handling and text extraction.

  *   Data: Streaming reference — PHP (**315** tks) - PHP streaming reference
  including streaming events and handling content block deltas (requires SDK
  v0.5.0+).

  *   Data: Streaming reference — Ruby (**116** tks) - Ruby streaming reference
  including basic streamed text handling.

  *   Data: Structured tool output field schema (**234** tks) - Schema
  description for the user-message tool_use_result field, including per-tool
  output shapes, the completed Agent or Task output contract, and the
  detachedToolCall placeholder for calls that stepped aside for a user message.

  *   Data: Structured usage rate-limit rows field (**177** tks) - Schema
  description for structured usage rate-limit rows, including server-defined
  meter ordering, null and empty semantics, and the synthesized header-fallback
  row.

  *   Data: Submit feedback control request (**231** tks) - Documents the
  internal submit_feedback control request that sends a /feedback report with
  the session transcript and sanitized error log, returns unavailable_reason
  when feedback is disabled or unsendable, and honors save_locally by writing a
  redacted local bundle instead.

  *   Data: Telemetry variables ignored notice (**268** tks) - Settings-status
  warning listing OTEL/telemetry environment variables a settings file sets that
  Claude Code ignores because such files can only turn telemetry off, with
  guidance on where to set them intentionally.

  *   Data: Thin-client diff dialog schema (**198** tks) - Internal data
  description for workspace git diff payloads used by the thin-client diff
  dialog.

  *   Data: Token counting reference (**665** tks) - Reference documentation for
  counting Claude model tokens with the Messages count_tokens endpoint and
  Anthropic SDK or CLI examples, including warnings against OpenAI tokenizers.

  *   Data: Tool result display metadata field (**861** tks) - Documents
  wrapper-level display metadata for SDK clients, including non-execution
  reasons, remedies, permission decisions, and human denial feedback, never
  replayed to the model.

  *   Data: Tool use display metadata field (**571** tks) - Documents the
  tool_use_meta wire field carrying per-block display metadata for a message's
  tool_use blocks; it is wrapper-level UI metadata and is not replayed to the
  model.

  *   Data: Tool use reference — C# (**2900** tks) - C# tool use reference
  including defining tools and reconstructing response content for the follow-up
  assistant message.

  *   Data: Tool use reference — Ruby (**943** tks) - Ruby tool use reference
  including the beta tool runner and manual agentic-loop guidance.

  *   Data: Turn handoff available event schema (**302** tks) - Schema
  description for the turn_handoff_available system event a cloud worker emits
  right after registering, carrying the tools and turn-handoff options its
  registration declared, and the rule that readers keep only the entry from the
  newest worker life.

  *   Data: Turn handoff file_names field (**346** tks) - Schema description for
  the turn_handoff file_names member listing user-attached files and their
  client-facing names that a capable worker copies into the session's home
  directory before running the handed-over calls, and when a malformed member is
  ignored.

  *   Data: Turn handoff hydrates_carried_lines field (**655** tks) - Schema
  description for the internal turn_handoff hydrates_carried_lines member, set
  on a worker that rebuilt its conversation from the stored transcript, covering
  how resent handoffs are re-accepted and echoed, which turns are appended or
  refused, and the cases where the key promises nothing.

  *   Data: Turn handoff memory_context field (**261** tks) - Schema description
  for the turn_handoff memory_context member carrying the claude.ai memory
  snapshot the client showed its model, how the worker writes it as a
  cowork_memory_context attachment line ahead of the handed-over messages, and
  when a malformed or uuid-colliding member is ignored.

  *   Data: Upload device hook template request (**200** tks) - Describes the
  cloud device request that uploads a vetted hook template to a worker before
  device hooks are registered.

  *   Data: VCS state changed branch field (**172** tks) - Describes the
  optional best-effort branch hint on vcs_state_changed events for commits and
  pushes, including multi-branch and uncertain-attribution behavior.

  *   Data: VCS state changed event schema (**392** tks) - Schema description
  for the vcs_state_changed system event as a best-effort repository
  cache-invalidation signal.

  *   Data: Windows transcript read EBADF notice (**180** tks) - Explains a
  Windows-only EBADF error reading the session transcript file likely caused by
  security or endpoint-management software intercepting file reads, and suggests
  excluding the .claude folder or allow-listing Claude Code before resuming.

  *   Data: Working tree upload refusal for duplicated withheld file (**211**
  tks) - Error refusing a working-tree upload because an index entry appeared
  holding byte-for-byte the content of a file that must stay on this machine,
  indicating something outside the upload wrote into its index copy.

  *   Data: Working tree upload refusal for git dubious ownership (**224** tks)
  - Error refusing a working-tree upload because git reports dubious ownership
  of the checkout or its .git, explaining that a safe.directory grant must stand
  in a file git reads since grants given only through GIT_CONFIG_PARAMETERS or
  GIT_CONFIG_COUNT do not reach the upload's git.

  *   Data: Working tree upload refusal for misplaced .git entry (**345** tks) -
  Error refusing a working-tree upload because the checkout's .git is a pointer
  file, symbolic link, hard-linked pointer file, or otherwise unfollowable
  entry, telling the user to start from the repository's ordinary main checkout
  instead.

  *   Data: Working tree upload refusal for missing git objects folders (**197**
  tks) - Error refusing a working-tree upload because the git directory lacks
  objects/info or objects/pack, as after a copy that skips empty folders,
  telling the user to check for links with ls -ld and then recreate both with
  mkdir, or start from a fresh clone.

  *   Data: Working tree upload refusal for private git directory setup failure
  (**349** tks) - Error refusing a working-tree upload because its private git
  directory could not be prepared under the configuration home, telling the user
  to check that the home and its seed-admin folder are directories only they can
  write, inspect them with ls -ld, remove scratch directories or links by name
  without a trailing slash, and retry.

  *   Data: Working tree upload refusal for unread git config environment
  variable (**305** tks) - Error refusing a working-tree upload because
  GIT_CONFIG_GLOBAL or GIT_CONFIG_SYSTEM names a git configuration file a
  session could change, so the upload's git does not read it, directing the user
  to move its settings into ~/.gitconfig or git's default system file and remove
  the variable, or keep the file outside session-writable folders.

  *   Data: allowManagedPermissionRulesOnly allowed-tools frontmatter scope
  (**424** tks) - Continues the allowManagedPermissionRulesOnly schema
  description: skill, command, and plugin allowed-tools frontmatter is ignored
  except for managed and bundled skills and plugins on admin-backed channels
  such as pinned managed marketplaces, attested claude.ai-synced plugins, and
  host-delivered --plugin-dir plugins.

  *   Data: allowManagedPermissionRulesOnly deny rules still apply (**45** tks)
  - Closes the allowManagedPermissionRulesOnly schema description by noting that
  --disallowedTools, skill disallowed-tools, and other command-line or session
  deny and ask rules still apply.

  *   Data: allowManagedPermissionRulesOnly setting (**66** tks) - Schema
  description for the managed allowManagedPermissionRulesOnly setting, which
  ignores permission rules from user, project, local, and --settings files and
  --allowedTools allow rules so only managed settings can add allow rules.

  *   Data: allowedMcpServers setting (**204** tks) - Describes how the
  enterprise allowedMcpServers setting governs user-added versus
  organization-delivered MCP servers, including ${VAR} expansion, undefined and
  empty-array behavior, and denylist precedence.

  *   Data: availableModelsMatch setting (**315** tks) - Schema description for
  the managed availableModelsMatch setting, contrasting prefix and exact
  matching of availableModels entries, family and release-dependent alias
  handling, Default-model restriction with startup failure, and which background
  and helper requests stay unrestricted.

  *   Data: deniedModels setting (**222** tks) - Schema description for the
  managed deniedModels setting, covering family-alias blocking, version-wide
  model ID blocking across dates, -fast and provider prefixes, ignored
  release-dependent aliases, and Default-model step-down or startup failure.

  *   Data: syncClaudeAiPlugins setting (**332** tks) - Describes how the
  syncClaudeAiPlugins setting disables account-synced plugin downloads,
  availability, and cleanup across user, managed, workspace, and invocation
  scopes.

  *   Data: syncClaudeAiSkills setting (**332** tks) - Describes how the
  syncClaudeAiSkills setting disables account-synced skill downloads,
  availability, and cleanup across user, managed, workspace, and invocation
  scopes.


  ### System Prompt


  Parts of the main system prompt.


  *   System Prompt: /loop tick (loop.md absent, dynamic pacing) (**315** tks) -
  Loop tick injection for dynamic self-paced autonomous checks when loop.md is
  absent.

  *   System Prompt: /loop tick (loop.md tasks) (**94** tks) - Loop tick
  injection for recurring cron-based runs of tasks from loop.md.

  *   System Prompt: /loop tick (loop.md tasks, dynamic pacing) (**311** tks) -
  Loop tick injection for dynamic self-paced runs of tasks from loop.md.

  *   System Prompt: Act when ready (**68** tks) - Instructs the agent to act
  once it has enough information and give recommendations instead of exhaustive
  surveys.

  *   System Prompt: Action safety and truthful reporting (**161** tks) -
  Requires confirmation for irreversible or outward-facing actions, checking
  targets before destructive edits, and truthful reporting of outcomes.

  *   System Prompt: Advisor tool instructions (**443** tks) - Instructions for
  using the Advisor tool.

  *   System Prompt: Agent Summary Generation (**291** tks) - System prompt used
  for "Agent Summary" generation.

  *   System Prompt: Agent intro (collaborative goals) (**31** tks) - Default
  opening system-prompt line framing the agent as working with the user toward
  their goals using its own judgment, used when no Output Style is configured.

  *   System Prompt: Agent thread notes (**284** tks) - Behavioral guidelines
  for agent threads covering absolute paths, response formatting, emoji
  avoidance, and tool call punctuation.

  *   System Prompt: Analysis instructions for full compact prompt (recent
  messages) (**288** tks) - Provides analysis instructions for summarizing only
  recent messages during conversation compaction.

  *   System Prompt: Artifact comment decision reformat retry (**256** tks) -
  Instructs the Artifact comment composer to reformat a malformed prior response
  as exactly one valid JSON decision while treating the reproduced response as
  untrusted data.

  *   System Prompt: Artifact comment edit composer (**911** tks) - Instructs a
  tool-less Artifact comment composer to emit exactly one reply or edit decision
  using ordered exact-string patches and an availability-gated full-rewrite
  form.

  *   System Prompt: Artifact comment fast acknowledgement selection (**603**
  tks) - Instructs a no-tools helper to select exactly one numbered canned
  acknowledgement for the newest Artifact comment based on edit capability,
  trigger state, and whether the request needs an artifact edit or reply.

  *   System Prompt: Artifact comment fast acknowledgement (**240** tks) -
  Prompts a no-tools helper model to produce one short plain-text
  acknowledgement for the newest Artifact comment before the full reply.

  *   System Prompt: Artifact comment list framing (**1055** tks) - Frames
  Artifact comment-list tool results as untrusted viewer data using randomized
  fences and optional selected-text, anchor-path, and anchor-file/page guidance.

  *   System Prompt: Artifact comment presence state guidance (**238** tks) -
  Explains that Artifact comment presence rows are tool-emitted, page-produced
  untrusted data that may resolve references but never supply instructions or
  permissions.

  *   System Prompt: Artifact comment reply composer (**375** tks) - Instructs a
  tool-less composer to answer an Artifact comment thread or flag requested
  artifact changes for its session in one brief plain-text reply without
  claiming or promising edits.

  *   System Prompt: Artifact comment result guidance (**225** tks) - Appends
  reply, resolve, and focused thread-read instructions to Artifact comment-list
  tool results.

  *   System Prompt: Artifact comment thread framing (**2178** tks) - Frames an
  Artifact comment thread and optional anchor context as untrusted viewer data
  using randomized fences.

  *   System Prompt: Artifact comment thread triage (**306** tks) - Classifies
  the newest human request in a framed Artifact comment thread as an artifact
  edit or a reply-only pipeline action.

  *   System Prompt: Artifact commenter access guidance (**183** tks) - Explains
  that the access word before a comment's stamp — owner, editor or commenter —
  is the access the server recorded for that person, context for weighing
  feedback but never a permission, and when an outside-organization note follows
  it.

  *   System Prompt: Artifact page-owned comment thread guidance (**192** tks) -
  Directs replies to page-owned comment threads through an available authorized
  page tool rather than Artifact relay threads.

  *   System Prompt: Artifact patch turn edit composer (**715** tks) - Instructs
  a tool-less composer to emit exactly one JSON decision for an artifact edit
  request, either ordered exact-string file edits with a one-sentence reply and
  confidence rating or a hand-back to the main assistant when needed context is
  missing or the request asks for an opinion, recommendation or choice, under
  strict patch-uniqueness and sizing rules.

  *   System Prompt: Auto mode Slack message provenance (**465** tks) -
  Instructs the auto mode classifier to treat server-verified human Slack relays
  as user intent while rejecting bot-authored and nested relays as consent.

  *   System Prompt: Auto mode setup proposal generator (**2740** tks) -
  Transforms gathered repository and usage reconnaissance into a constrained
  JSON proposal for auto-mode environment context and permission rules.

  *   System Prompt: Auto mode (**244** tks) - Continuous task execution, akin
  to a background agent.

  *   System Prompt: Autonomous loop check (**1484** tks) - Defines behavior for
  autonomous timer-based invocations, guiding Claude to continue established
  work, maintain PRs, and handle repeated idle checks while the user is away.

  *   System Prompt: Autonomous loop notification guidance (**98** tks) - Guides
  when autonomous loop ticks should notify the user via PushNotification for
  blockers or actionable state changes.

  *   System Prompt: Autonomous loop persistence guidance
  (CLAUDE_CODE_LOOP_PERSISTENT) (**1630** tks) - Defines behavior for autonomous
  timer-based invocations, guiding Claude to persistently continue established
  work, maintain PRs, and broaden scope before stopping while the user is away.

  *   System Prompt: Autonomous loop tick (dynamic pacing) (**311** tks) -
  Autonomous loop tick injection for dynamic self-paced autonomous checks
  scheduled with ScheduleWakeup.

  *   System Prompt: Autonomous loop tick (**87** tks) - Autonomous loop tick
  injection for recurring cron-based autonomous checks.

  *   System Prompt: Autonomous operation guidelines (**407** tks) - Instructs
  autonomous sessions to proceed on reversible work, stop for destructive or
  scope-changing actions, and finish promised work before ending the turn.

  *   System Prompt: Avoiding Unnecessary Sleep Commands (part of PowerShell
  tool description) (**299** tks) - Guidelines for avoiding unnecessary sleep
  commands in PowerShell scripts, including alternatives for waiting and
  notification.

  *   System Prompt: Background session instructions (lean rewrite) (**410**
  tks) - Directs lean-rewrite background sessions to use job-specific temporary
  storage, follow worktree guidance, and return an actionable report.

  *   System Prompt: Background session instructions (**354** tks) -
  Instructions for background job sessions to use the job-specific temporary
  directory and follow the appropriate worktree isolation guidance.

  *   System Prompt: Background session worktree isolation (lean rewrite)
  (**177** tks) - Requires background jobs to enter a worktree before code edits
  unless already isolated, while allowing read-only work or isolation failures
  in place.

  *   System Prompt: Background session worktree persistence guidance (**186**
  tks) - Directs background sessions to commit and push worktree changes when
  appropriate while preserving user git-control instructions and branch safety.

  *   System Prompt: Background worktree isolation guidance (**129** tks) -
  Tells background sessions when to enter an isolated worktree before making
  code changes and when to continue in place.

  *   System Prompt: Background worktree persistence (lean rewrite) (**234**
  tks) - Directs background jobs to commit and push changes from worktrees they
  entered while respecting user git restrictions and protected branches.

  *   System Prompt: Censoring assistance with malicious activities (**98** tks)
  - Guidelines for assisting with authorized security testing, defensive
  security, CTF challenges, and educational contexts while censoring requests
  for malicious activities.

  *   System Prompt: Chrome browser MCP tools (**388** tks) - Instructions for
  loading deferred Chrome browser MCP tools through ToolSearch in a single
  batched selection before browser tasks.

  *   System Prompt: Claude Fable 5 model identity (**202** tks) - Identifies
  this Claude iteration as Claude Fable 5, explains its relationship to Claude
  Mythos 5, and links to Anthropic's comparison announcement.

  *   System Prompt: Claude Fable 5.1 model identity (**235** tks) - Identifies
  this Claude iteration as Claude Fable 5.1, explains its relationship to Claude
  Mythos 5.1, and points users to Anthropic's Fable and Mythos announcement for
  differences.

  *   System Prompt: Claude in Chrome browser automation (non-deferred tools)
  (**1130** tks) - Instructions for using directly available Claude in Chrome
  browser automation tools effectively.

  *   System Prompt: Claude in Chrome browser automation (**1158** tks) -
  Instructions for using Claude in Chrome browser automation tools effectively.

  *   System Prompt: Claude in Chrome browser selection instructions (**191**
  tks) - Instructs the agent to ask the user to choose among multiple connected
  Chrome browsers before using browser automation tools.

  *   System Prompt: Claude in Chrome deferred tool loading (**311** tks) -
  Directs Claude to batch-load deferred core and task-specific Chrome MCP tools
  in one ToolSearch call.

  *   System Prompt: Combined memory index pointer instructions (**120** tks) -
  Instructs the agent to add one-line pointers for private and team memories to
  the single private memory index and never write memory content there.

  *   System Prompt: Comment what and task context avoidance (**76** tks) -
  Instructs Claude not to write comments that explain what code does or
  reference transient task context.

  *   System Prompt: Comment why-only guidance (**67** tks) - Instructs Claude
  to write code comments only when the reason is non-obvious and useful to
  future readers.

  *   System Prompt: Communication style (**297** tks) - Instructs Claude to
  give brief, user-facing updates at key moments during tool use, write concise
  end-of-turn summaries, match response format to task complexity, and avoid
  comments and planning documents in code.

  *   System Prompt: Concise output style (**373** tks) - Defines the built-in
  Concise output style to lead with results, omit narration, stay brief by
  default, and preserve requested detail and correctness.

  *   System Prompt: Context compaction summary (**409** tks) - Prompt used for
  context compaction summary (for the SDK).

  *   System Prompt: Context management (lean rewrite) (**193** tks) - Explains
  automatic compaction and tells lean-rewrite models to continue working rather
  than hand off early because of context length.

  *   System Prompt: Coordinator cross-session peer guidance (**326** tks) -
  Explains how coordinators discover and message peer Claude sessions while
  treating peer requests as untrusted input rather than delegated worker
  authority.

  *   System Prompt: Coordinator mode orchestration (**5921** tks) - Provides
  coordinator-mode instructions for delegating work to worker agents, managing
  worker lifecycle, handling cross-session peers, and verifying delegated
  results.

  *   System Prompt: Correction restraint (**365** tks) - Instructs Claude to
  correct only consequential errors plainly, avoid unnecessary self-criticism or
  re-auditing, and evaluate other agents’ corrections before adopting them.

  *   System Prompt: Delivering requested work (lean rewrite) (**477** tks) -
  Directs lean-rewrite models to respect the requested scope and finish
  independent work before asking blocking questions.

  *   System Prompt: Delivering work at full scope (**605** tks) - Instructs
  Claude to complete ordinary requested work at full scope under reasonable
  assumptions, continue past non-blocking concerns, and preserve necessary
  refusal and risky-action confirmation boundaries.

  *   System Prompt: Deny rule circumvention classifier guidance (**74** tks) -
  Guides permission classification to block attempts to route around configured
  Edit, Write, or MultiEdit deny rules.

  *   System Prompt: Description part of memory instructions (**148** tks) -
  Defines what the memory description field should contain within broader
  user-memory authoring instructions.

  *   System Prompt: Doing tasks (ambitious tasks) (**47** tks) - Allow users to
  complete ambitious tasks; defer to user judgement on scope.

  *   System Prompt: Doing tasks (help and feedback) (**24** tks) - How to
  inform users about help and feedback channels.

  *   System Prompt: Doing tasks (no compatibility hacks) (**52** tks) - Delete
  unused code completely rather than adding compatibility shims.

  *   System Prompt: Doing tasks (no unnecessary additions) (**73** tks) - Do
  not add features, refactor, or improve beyond what was asked.

  *   System Prompt: Doing tasks (no unnecessary error handling) (**64** tks) -
  Do not add error handling for impossible scenarios; only validate at
  boundaries.

  *   System Prompt: Doing tasks (security) (**67** tks) - Avoid introducing
  security vulnerabilities like injection, XSS, etc.

  *   System Prompt: Doing tasks (software engineering focus) (**104** tks) -
  Users primarily request software engineering tasks; interpret instructions in
  that context.

  *   System Prompt: Dream CLAUDE.md memory reconciliation (**436** tks) -
  Instructs dream memory consolidation to reconcile feedback and project
  memories against CLAUDE.md, deleting stale memories or flagging possible
  CLAUDE.md drift.

  *   System Prompt: Dream team memory handling (**279** tks) - Instructions for
  handling shared team memories during dream consolidation, including
  deduplication, conservative pruning rules, and avoiding accidental promotion
  of personal memories.

  *   System Prompt: Emoji avoidance (**31** tks) - Instructs Claude to avoid
  using emojis unless the user explicitly asks for them.

  *   System Prompt: Executing actions with care (**1105** tks) - Instructions
  for executing actions carefully.

  *   System Prompt: Explain /code-review ultra (**131** tks) - Guidance shown
  when a user asks about 'ultrareview': explains it maps to /code-review ultra
  (the /ultrareview alias is deprecated) and that the agent can't start it
  directly.

  *   System Prompt: Exploratory questions — analyze before implementing (**75**
  tks) - Instructs Claude to respond to open-ended questions with analysis,
  options, and tradeoffs instead of jumping to implementation, waiting for user
  agreement before writing code.

  *   System Prompt: Feedback memory body structure (**79** tks) - Defines the
  body structure for feedback memories, including the rule, why, and how to
  apply it.

  *   System Prompt: Feedback memory save guidance (**121** tks) - Explains when
  to save feedback memories from user corrections or confirmed non-obvious
  approaches.

  *   System Prompt: Finish the request instead of stopping early (**785** tks)
  - Directs the model to complete the request and unblocked work without
  unnecessary check-ins while preserving confirmation for hard-to-reverse or
  outward-facing actions.

  *   System Prompt: Focus mode (lean rewrite) (**142** tks) - Explains that
  focus mode hides intermediate output and requires all user-relevant
  information in the final message.

  *   System Prompt: Focus mode (long form) (**105** tks) - Focus-mode notice
  (long form): the user sees only the final text, not tool calls, results, or
  inter-step writing.

  *   System Prompt: Focus mode (short form) (**91** tks) - Focus-mode notice
  (short form): only each response's final text reaches the user.

  *   System Prompt: Fork usage guidelines (**326** tks) - Instructions for when
  to fork subagents and rules against reading fork output mid-flight or
  fabricating fork results.

  *   System Prompt: Forked agent guidance (**164** tks) - Explains that calling
  Agent with subagent_type "fork" creates a background fork and when to use it.

  *   System Prompt: Forked conversation worktree isolation guidance (**416**
  tks) - Warns a forked background conversation not to enter the original
  session's worktree and directs it to create a separate worktree when isolation
  is enabled.

  *   System Prompt: Fresh subagent delegation example (**315** tks) - Provides
  an example of briefing a fresh specialized subagent with sufficient context
  and a specific reporting request.

  *   System Prompt: Frontend browser verification (**86** tks) - Requires
  Claude to start the dev server and verify UI or frontend changes in a browser
  before reporting completion.

  *   System Prompt: Git status (**37** tks) - System prompt for displaying the
  current git status at the start of the conversation.

  *   System Prompt: Handling corrections (lean rewrite) (**457** tks) -
  Encourages correcting minor mistakes without excessive apologies and
  evaluating feedback from other agents critically.

  *   System Prompt: Harness instructions (lean rewrite) (**464** tks) -
  Combines the agent introduction and security policy with guidance on tool
  parallelism, permissions, file links, and hooks.

  *   System Prompt: Harness instructions (**299** tks) - Core interactive-agent
  identity and harness instructions for terminal Markdown output, security,
  permissions, system-reminder handling, hook feedback, tool use, and code
  references.

  *   System Prompt: Hook evaluator truncated transcript note (**74** tks) -
  Tells the hook condition evaluator that earlier conversation was omitted and
  how to handle insufficient evidence.

  *   System Prompt: Hook feedback handling (**85** tks) - Explains that hook
  feedback should be treated as user feedback and how to respond when hooks
  block actions.

  *   System Prompt: Hooks Configuration (**1493** tks) - Documents hook
  structure, events, types, input/output fields, and examples for the Claude
  Code configuration skill.

  *   System Prompt: How to use the SendUserMessage tool (**283** tks) -
  Instructions for using the SendUserMessage tool.

  *   System Prompt: Insights at a glance summary (**569** tks) - Generates a
  concise 4-part summary (what's working, hindrances, quick wins, ambitious
  workflows) for the insights report.

  *   System Prompt: Insights friction analysis (**139** tks) - Analyzes
  aggregated usage data to identify friction patterns and categorize recurring
  issues.

  *   System Prompt: Insights interaction style (**119** tks) - Analyzes Claude
  Code usage data to describe the user's interaction style.

  *   System Prompt: Insights memorable moment (**96** tks) - Analyzes Claude
  Code usage data to find a memorable qualitative moment.

  *   System Prompt: Insights on the horizon (**148** tks) - Identifies
  ambitious future workflows and opportunities for autonomous AI-assisted
  development.

  *   System Prompt: Insights session facets extraction (**310** tks) - Extracts
  structured facets (goal categories, satisfaction, friction) from a single
  Claude Code session transcript.

  *   System Prompt: Insights suggestions (**737** tks) - Generates actionable
  suggestions including CLAUDE.md additions, features to try, and usage
  patterns.

  *   System Prompt: Insights summary (At a Glance) (**272** tks) - The 'At a
  Glance' summary block of the Insights report (what's working / what's
  hindering).

  *   System Prompt: Insights what works (**121** tks) - Analyzes Claude Code
  usage data to identify workflows that are working well for the user.

  *   System Prompt: Interactive agent intro (output-style active) (**40** tks)
  - Opening system-prompt line for sessions that have an Output Style
  configured.

  *   System Prompt: Interactive agent intro (output-style conditional) (**181**
  tks) - Opening system-prompt line that selects the Output Style intro or the
  collaborative-goals intro, then adds instructions to use the available tools
  and the security and URL-guessing policy.

  *   System Prompt: Irreversible action caution (lean rewrite) (**294** tks) -
  Requires caution with irreversible actions, inspection before overwriting,
  faithful outcome reporting, and context-specific authorization.

  *   System Prompt: Learning mode (insights) (**142** tks) - Instructions for
  providing educational insights when learning mode is active.

  *   System Prompt: Learning mode (**1042** tks) - Main system prompt for
  learning mode with human collaboration instructions.

  *   System Prompt: Memory description of user feedback (**139** tks) -
  Describes the user feedback memory type that stores guidance about work
  approaches, emphasizing recording both successes and failures and checking for
  contradictions with team memories.

  *   System Prompt: Memory hygiene (lean rewrite) (**269** tks) - Directs
  lean-rewrite models to avoid redundant memories, remove incorrect ones, and
  verify recalled code references before relying on them.

  *   System Prompt: Memory index pointer instructions (**129** tks) - Instructs
  the agent to add one-line pointers to the memory index file and treat the
  index as separate from memory content.

  *   System Prompt: Memory instructions (**540** tks) - Instructions for using
  persistent file-based memory, including memory file format, scope, indexing,
  and stale-memory handling.

  *   System Prompt: Memory persistence scope (**60** tks) - Explains that
  memory is for information useful in future conversations, not only within the
  current conversation.

  *   System Prompt: Memory save exclusions (**14** tks) - Lists categories of
  information that should not be saved in memory, even when the user asks.

  *   System Prompt: Minimal mode (**285** tks) - Describes the behavior and
  constraints of minimal mode, which skips hooks, LSP, plugins, auto-memory, and
  other features while requiring explicit context via CLI flags.

  *   System Prompt: Monitor fallback heartbeat guidance (**319** tks) - Guides
  dynamic loop ticks to use Monitor as the primary wake signal and
  ScheduleWakeup as a fallback heartbeat, place a visible status update before
  or after re-arming, and stop the monitor when ending the loop.

  *   System Prompt: Moved Artifact comment thread guidance (**158** tks) -
  Directs Claude to re-evaluate a previously sent Artifact comment after its
  author moves the thread to a new location.

  *   System Prompt: Non-fork subagent delegation examples (**894** tks) -
  Provides conditional non-fork subagent examples for background and synchronous
  execution, including self-contained prompts, waiting-state responses, and
  independent-review briefing.

  *   System Prompt: One of six rules for using sleep command (**23** tks) - One
  of the six rules for using the sleep command.

  *   System Prompt: Option previewer (**151** tks) - System prompt for
  previewing UI options in a side-by-side layout.

  *   System Prompt: Outcome-first communication style (lean rewrite) (**1246**
  tks) - Guides lean-rewrite models to give readable progress updates and
  outcome-first final responses calibrated to the user's context.

  *   System Prompt: Outcome-first communication style (**720** tks) - Instructs
  Claude to keep user-facing updates readable and outcome-first, answer directly
  after work completes, match response format to task complexity, and limit code
  comments to non-obvious constraints.

  *   System Prompt: PR Slack notification step (**125** tks) - Adds a PR
  workflow step to optionally ask the user before posting the PR URL to Slack.

  *   System Prompt: PR Steward handoff guidance (**597** tks) - Explains the PR
  Steward watching and working labels and requires checking a PR's labels before
  pushing, babysitting, or monitoring it, offering the user leave-it,
  one-change, or take-over choices and leaving the PR alone when the user is
  away.

  *   System Prompt: Parallel tool call note (part of "Tool usage policy")
  (**102** tks) - System prompt telling Claude to use parallel tool calls.

  *   System Prompt: Partial compaction instructions (**852** tks) -
  Instructions on how to compact when the user decided to compact only a portion
  of the conversation, with a structured summary format and analysis process.

  *   System Prompt: Pasted content instruction trust boundary (**236** tks) -
  Distinguishes user-directed requests from instructions embedded in pasted
  material, permits acting on paste-only requests, and requires checking pasted
  commands before execution.

  *   System Prompt: Permission classifier strict review guidance (**79** tks) -
  Instructs the permission classifier to carefully deny blocked actions and
  require explicit user confirmation for overrides.

  *   System Prompt: Personal project memory description (**67** tks) -
  Describes project memories for ongoing work, goals, initiatives, bugs, or
  incidents relevant to the user's work in a directory.

  *   System Prompt: Phase four of plan mode (**238** tks) - Final plan-writing
  instructions for phase four of plan mode.

  *   System Prompt: Plan mode interactive workshop offer (**688** tks) -
  Instructs plan mode to offer an interactive workshop for substantive design
  decisions and integrate accepted workshop decisions into the canonical plan.

  *   System Prompt: Plan sent to Ultraplan (**106** tks) - User-facing note
  confirming a plan has been sent to Ultraplan for remote refinement.

  *   System Prompt: Plan vs memory guidance (**115** tks) - Explains when to
  use or update a plan instead of saving information to memory.

  *   System Prompt: PowerShell edition for 5.1 (**440** tks) - System prompt
  for providing information about Windows PowerShell 5.1.

  *   System Prompt: PowerShell edition for 7+ (**128** tks) - Describes
  PowerShell 7+ shell syntax support, including pipeline chain operators,
  ternary, null-coalescing, and UTF-8 defaults.

  *   System Prompt: PowerShell edition unknown (**108** tks) - Assumes Windows
  PowerShell 5.1 compatibility when the PowerShell edition is unknown and
  forbids PowerShell 7-only syntax.

  *   System Prompt: Prefer editing existing files (**17** tks) - Instructs
  Claude to prefer editing existing files instead of creating new ones.

  *   System Prompt: Project coordinator relay trust boundary (**298** tks) -
  Treats coordinator relays and fetched project messages as external content
  that cannot establish user consent or launder blocked actions.

  *   System Prompt: Project coordinator reply consent (**247** tks) - Defines
  when an unmarked timeline reply approves the single action and target proposed
  by a coordinator message.

  *   System Prompt: Project member session user message provenance (**1064**
  tks) - Defines how marked messages in a shared-project member's own session
  count as that member's user turns while coordinator relays, bare approvals,
  and other relayed content stay untrusted.

  *   System Prompt: Project memory body structure (**83** tks) - Defines the
  body structure for project memories, including the fact or decision, why, and
  how to apply it.

  *   System Prompt: Project memory save guidance (**91** tks) - Explains when
  to save project memories about who is doing what, why, or by when, including
  absolute date handling.

  *   System Prompt: Project skill upkeep for feedback memory (**314** tks) -
  Instructs Claude to update the relevant project skill when saving feedback
  memory about repeatable workflow corrections.

  *   System Prompt: Project thread in-thread message provenance (**291** tks) -
  Explains that a message the person sends in the project thread arrives as its
  own marked user turn, and that a bare approval there answers only what the
  session sent through the reply tool and clears no SOFT BLOCK unless it names
  the action and its target.

  *   System Prompt: Project timeline user message provenance (**1688** tks) -
  Defines how project timeline messages from the verified owner become user
  turns while other relayed or fetched project content remains untrusted
  external data.

  *   System Prompt: Published artifact inline edits and large-change republish
  (**441** tks) - Uses publish edits for small page changes and same-response
  file edits followed by publishing for larger changes, with reads only when
  necessary.

  *   System Prompt: Published artifact same-response edit and republish
  (**307** tks) - Directs the agent to edit an existing artifact and republish
  the same path in one response, reading only when necessary and correcting
  failed edits.

  *   System Prompt: Remote plan mode (ultraplan) (**870** tks) - System
  reminder injected during remote planning sessions that instructs Claude to
  explore the codebase, produce a diagram-rich plan via ExitPlanMode, and
  implement it with a pull request upon approval.

  *   System Prompt: Remote planning session (**612** tks) - System reminder
  that configures a remote planning session to explore the codebase, produce an
  implementation plan via ExitPlanMode, and handle plan approval, rejection, or
  teleportation back to the user's local terminal.

  *   System Prompt: Reporting outcomes (**261** tks) - Requires reports to
  distinguish observed outcomes from intentions, lead with failures or
  incomplete work, and avoid overstating unverified completion.

  *   System Prompt: Respond in configured language (**128** tks) - Directs all
  responses, explanations, and code commentary into a configured language.

  *   System Prompt: Responsive mode (**813** tks) - Directs Claude to answer
  immediately with a one or two sentence acknowledgement before thinking or
  calling any tool, and to write plain conversational English free of openers,
  flattery, filler and wrap-ups.

  *   System Prompt: Sandbox task resource boundary (**217** tks) - Limits
  sandboxed work to task-provided resources even when other resources are
  reachable and requires reporting missing access instead of bypassing the
  boundary.

  *   System Prompt: Saving skills via file delivery (**270** tks) - Explains
  that account skills are not modified by filesystem writes and directs the
  agent to deliver complete skill files with the file-sending tool.

  *   System Prompt: Self-hosted runner doctor (**8666** tks) - Instructs an
  agent to diagnose self-hosted runner authentication, network, lifecycle,
  queue, hooks, metrics, and escalation issues.

  *   System Prompt: Self-hosted runner setup (**2718** tks) - Guides an
  operator through creating, verifying, learning, and documenting a self-hosted
  runner deployment.

  *   System Prompt: Session scratchpad temporary-file guidance (**208** tks) -
  Requires temporary files to use the session scratchpad unless the user
  requests otherwise and requires complete absolute paths when referring to its
  files.

  *   System Prompt: Shared git stash safety (**201** tks) - Warns that git
  stash is shared across worktrees and sessions, preferring WIP commits or
  uniquely tagged stash entries.

  *   System Prompt: Skill save permission note (**163** tks) - Notes that
  whether a delivered file can be saved as a skill depends on organization
  settings the assistant cannot see, and how to phrase that conditionally rather
  than as a certainty.

  *   System Prompt: Skillify Current Session (**1798** tks) - System prompt for
  converting the current session into a skill.

  *   System Prompt: Subagent carried user consent history (**260** tks) -
  Scopes carried parent user replies to their preceding assistant proposals and
  warns that trimmed history may omit standing instructions.

  *   System Prompt: Subagent delegation cost guidance (**503** tks) - Explains
  when delegation is worth its context and token costs, when to work inline, and
  how to brief a subagent narrowly.

  *   System Prompt: Subagent delegation examples (**620** tks) - Provides
  example interactions showing how a coordinator agent should delegate tasks to
  subagents, handle waiting states, and report results.

  *   System Prompt: Subagent delegation restraint (**479** tks) - Guides Claude
  to delegate work only when it is genuinely independent, large enough to
  justify a fresh context, or naturally parallel, while avoiding excessive or
  redundant subagents and not redoing delegated work.

  *   System Prompt: Subagent delegation when-to-use guidance (**198** tks) -
  Tells the model when to delegate to a subagent (matching agent type,
  independent parallel work with an optional effort lower fan-out hint,
  cross-file reading) and to search directly for single-fact lookups without
  duplicating delegated work.

  *   System Prompt: System reminder interpretation (lean rewrite) (**463** tks)
  - Explains how system reminders are delivered, when they may be ignored, and
  why they never constitute user authorization.

  *   System Prompt: System section (**93** tks) - System section of the main
  system prompt.

  *   System Prompt: Task approval continuity (**109** tks) - Instructs the
  agent to continue agreed tasks end to end without unnecessary re-confirmation.

  *   System Prompt: Tasks vs memory guidance (**80** tks) - Explains when to
  use tasks instead of saving current-conversation progress to memory.

  *   System Prompt: Team memory index pointer instructions (**159** tks) -
  Instructs the agent to add one-line memory pointers to the appropriate team
  memory index file and never write memory content into the index.

  *   System Prompt: Team project memory description (**67** tks) - Describes
  project memories for shared ongoing work, goals, initiatives, bugs, or
  incidents within a working directory.

  *   System Prompt: Teammate Communication (**105** tks) - System prompt for
  teammate communication in swarm.

  *   System Prompt: Tone and style (code references) (**39** tks) - Instruction
  to include file_path:line_number when referencing code.

  *   System Prompt: Tone and style (concise output — short) (**16** tks) -
  Instruction for short and concise responses.

  *   System Prompt: Tool call colon avoidance (**59** tks) - Instructs Claude
  not to use a colon before tool calls because tool calls may be hidden from
  user output.

  *   System Prompt: Tool call summary label (**112** tks) - Instructs Claude to
  write a short past-tense summary label for completed tool calls in mobile UI
  rows.

  *   System Prompt: Tool usage (subagent guidance) (**141** tks) - Guidance on
  when and how to use subagents effectively.

  *   System Prompt: Tool usage (task management) (**70** tks) - Use TodoWrite
  to break down and track work progress.

  *   System Prompt: Troubleshooting confirmation policy (**71** tks) - Requires
  explaining fixes and confirming before destructive or installation-changing
  troubleshooting commands.

  *   System Prompt: User memory usage guidance (**91** tks) - Explains when to
  use user memories to tailor responses to the user's profile or perspective.

  *   System Prompt: Verified Slack human relay consent (**191** tks) -
  Recognizes harness-marked human messages from the bound Slack thread as user
  instructions and consent.

  *   System Prompt: WSL managed settings double opt-in (**152** tks) - Explains
  that WSL can read the Windows managed settings policy chain only when the
  admin-enabled flag is set, with HKCU requiring an additional user opt-in.

  *   System Prompt: Windows sandbox certificate recovery (**292** tks) -
  Requests a user PowerShell HTTPS connection before one retry of a certificate
  failure without weakening certificate validation.

  *   System Prompt: Windows sandbox localhost retry guidance (**401** tks) -
  Explains Windows sandbox localhost failures and permits the previously
  described unsandboxed retry with an explicit warning.

  *   System Prompt: Windows sandbox localhost user alternatives (**434** tks) -
  Explains Windows sandbox localhost failures and leaves the choice to run
  outside the sandbox to the user.

  *   System Prompt: Windows sandbox package cache limits (**259** tks) -
  Explains read-only Maven cache access and warns against dotnet no-restore
  builds that can remove package DLLs.

  *   System Prompt: Windows sandbox placeholder file protection (**187** tks) -
  Protects sandbox-created zero-byte configuration placeholders from staging,
  deletion, relocation, and Git ignore changes.

  *   System Prompt: Windows sandbox profile tool retry guidance (**451** tks) -
  Identifies profile-installed tool failures as sandbox limits and explains
  narrow user-controlled read grants alongside retry guidance.

  *   System Prompt: Windows sandbox profile tool user guidance (**429** tks) -
  Explains narrow user-controlled read grants for profile-installed tools
  without authorizing sandbox settings changes.

  *   System Prompt: Worker instructions (**356** tks) - Instructions for
  workers to follow when implementing a change.

  *   System Prompt: Writing for the user (**586** tks) - Defines concise,
  standalone user-facing final-message rules covering sentence structure,
  formatting, code and numbers, terminology, and stopping once the answer is
  complete.

  *   System Prompt: Writing subagent prompts (**398** tks) - Guidelines for
  writing effective prompts when delegating tasks to subagents, covering
  context-inheriting vs fresh subagent scenarios.

  *   System Prompt: You should know understood-topic skip guidance (**150**
  tks) - Tells the You should know suggestion agent to skip topics the person
  plausibly understood in the main session while still allowing consequential
  details mentioned only in passing.


  ### System Reminders


  Text for large system reminders.


  *   System Reminder: /btw side question (**406** tks) - System reminder for
  /btw slash command side questions without tools.

  *   System Reminder: Agent mention (**45** tks) - Notification that user wants
  to invoke an agent.

  *   System Reminder: App tool server retrying connection failure (**169** tks)
  - Directs the agent to continue without temporarily unavailable app tools
  while automatic retries run and to avoid asking the user to repair an app-side
  failure.

  *   System Reminder: AppifactRepl Slides deck workflow (**2086** tks) -
  Instructs Claude to create or revise a file-backed Slides deck through one
  AppifactRepl program while preserving its project/deck.json index, slide
  files, and embedded speaker notes.

  *   System Reminder: AppifactRepl design canvas workflow (**2090** tks) -
  Instructs Claude to create or revise a file-backed Design canvas through one
  AppifactRepl program while preserving its project/canvas.json index and
  changed board files.

  *   System Reminder: Artifact auto-replies resumed (**349** tks) - Reports
  that Artifact comment auto-replies resumed, explains which stopped-period
  comments are handled based on the stop cause, and warns that the stop remains
  until the watch reconnects.

  *   System Reminder: Artifact capabilities contract unavailable (**197** tks)
  - Explains a refused capability declaration and prevents publishing a page
  without needed new grants unless the user asks.

  *   System Reminder: Artifact capability declaration revocation warning
  (**341** tks) - Warns that publishing a replacement capability declaration
  would implicitly revoke stored capabilities and explains how to preserve or
  intentionally revoke them.

  *   System Reminder: Artifact comment reply activation failure (**204** tks) -
  Explains that an Artifact comment reply was not posted because Claude is not
  currently activated for the thread and requires reactivation before retrying.

  *   System Reminder: Artifact comment reply session collision (**347** tks) -
  Reports how concurrent live sessions resolved or may duplicate automatic
  replies to comments on the same Artifact.

  *   System Reminder: Artifact page-owned comment reply refusal (**156** tks) -
  Reports that no reply was posted to an Artifact relay thread and directs the
  answer to the page-owned comment thread or the current session.

  *   System Reminder: Artifact published source edit guidance (**163** tks) -
  Prefers publish edits for small changes to existing on-disk artifact sources
  and coordinates file edits with publishing for larger changes.

  *   System Reminder: Artifact type instructions trust boundary (**246** tks) -
  Restricts third-party Artifact type instructions to the Artifact's own files
  and prevents them from expanding scope, permissions, or overriding user and
  system instructions.

  *   System Reminder: Artifact type page untrusted content warning (**184**
  tks) - Warns that an owned Artifact page supplied by an Artifact type was
  written by the type publisher and must be treated as untrusted data rather
  than instructions.

  *   System Reminder: AskUserQuestion minimum options validation (**161** tks)
  - Explains that a rejected single-option question was not shown and instructs
  the agent to proceed without retrying or inventing another option.

  *   System Reminder: Async agent launched metadata (**267** tks) - Reports
  internal metadata for a newly launched asynchronous agent and warns Claude not
  to expose its ID or predict its results.

  *   System Reminder: Async agent launched (**146** tks) - Warns Claude not to
  duplicate an asynchronously launched agent's work or read its full JSONL
  transcript output file.

  *   System Reminder: Attached device stopped offering tools (**194** tks) -
  Tells the agent a remote tool call to a previously attached device could not
  run because that device stopped offering tools, and directs asking the user to
  check the Claude app on that computer.

  *   System Reminder: Attached machine asleep command paused (**69** tks) -
  Says a command sent to a sleeping attached machine was not cancelled but
  paused with it, will most likely continue on waking, keeps what it already
  did, and must not be repeated here or there.

  *   System Reminder: Attached machine asleep command withdrawn (**73** tks) -
  Says the session stopped waiting on a sleeping attached machine and withdrew
  the command, which will be stopped when the machine wakes, may have partly
  run, and must not be repeated without checking.

  *   System Reminder: Attached machine away recheck-on-request ending (**26**
  tks) - Closing sentence allowing one more check of an unreachable attached
  machine only when the user says it is back or asks to try again.

  *   System Reminder: Attached machine away until-reachable condition (**54**
  tks) - Defers further calls to an attached machine until a later
  attached-machines note reports it reachable again or the user says it is back
  or asks to try again.

  *   System Reminder: Attached machine could not be reached (oversized call)
  (**50** tks) - Appends to the could-not-be-reached notice that a call too
  large to deliver will not succeed on a retry, when the failure is on this
  session's side rather than the machine not answering.

  *   System Reminder: Attached machine could not be reached (return not noted)
  (**185** tks) - Follows the could-not-be-reached notice when the session is
  not tracking the machine's return, adding that an oversized call will not
  succeed on retry, barring calls this turn, and allowing one recheck only when
  the user says it is back.

  *   System Reminder: Attached machine could not be reached (return noted)
  (**143** tks) - Follows the could-not-be-reached notice when the session is
  tracking the machine's return, barring calls until a later note or the user
  reports it back and telling the user what is blocked.

  *   System Reminder: Attached machine could not be reached (**98** tks) -
  Opens the notice that an attached machine could not be reached from this
  session for a stated reason, the call did not run, and the user should check
  that Claude is running and connected there; later guidance is appended
  separately.

  *   System Reminder: Attached machine not answering (retry shortly) (**113**
  tks) - Reports that an attached machine is not answering and the call did not
  run, most often because Claude on it is not connected, and advises trying
  again shortly or asking the user to check that Claude is running there.

  *   System Reminder: Attached machine not answering (stop calling) (**204**
  tks) - Reports that an attached machine is not answering and the call did not
  run, then bars further calls this turn, tells the user what is blocked and to
  check that Claude is running there, and allows one recheck only when the user
  says it is back.

  *   System Reminder: Attached machine reply not received (**276** tks) -
  Reports that a still-connected attached machine's reply to a command never
  arrived despite liveness checks, so the outcome is unknown, forbids re-running
  non-idempotent commands just to see output, and suggests narrower commands for
  large output.

  *   System Reminder: Attached machine stopped answering (return not noted)
  (**207** tks) - Follows the stopped-answering notice when the session is not
  tracking the machine's return, forbidding retries of non-idempotent commands
  until reconnection and further calls this turn, and allowing one recheck only
  when the user says it is back.

  *   System Reminder: Attached machine stopped answering (return noted)
  (**164** tks) - Follows the stopped-answering notice when the session is
  tracking the machine's return, barring calls until a later note or the user
  reports it back, requiring a check of what the command did before repeating
  it, and telling the user what is blocked and unverified.

  *   System Reminder: Attached machine stopped answering (**189** tks) - Opens
  the notice that an attached machine stopped answering health checks while or
  after a command was sent, so the command's outcome is unknown; the
  instructions on retrying and calling the machine are appended separately.

  *   System Reminder: Attached machine untrusted attachments refusal (**274**
  tks) - Tells the agent a call to an attached machine was refused because the
  session has repositories or files attached that its owner has not marked
  trusted, that retrying will not help, and to call the folder-listing tool
  once, relay the trust question or request the folder so the person's answer
  clears it, and otherwise explain what stays blocked.

  *   System Reminder: Attached machine went to sleep (return not noted)
  (**144** tks) - Follows the went-to-sleep notice when the session is not
  tracking the machine's return, barring calls this turn, telling the user what
  is paused, and allowing one recheck only when the user says it is back,
  followed by a check of what the command did.

  *   System Reminder: Attached machine went to sleep (return noted) (**173**
  tks) - Follows the went-to-sleep notice when the session is tracking the
  machine's return, barring calls until a later note or the user reports it
  back, then requiring a check of what the command did and telling the user what
  is paused.

  *   System Reminder: Attached machine went to sleep (**100** tks) - Opens the
  notice that an attached machine said it was going to sleep while or after a
  command was sent, followed by whether the command was withdrawn or left
  paused.

  *   System Reminder: Auto mode clarification bias (**338** tks) - Encourages
  auto mode to make reasonable decisions without stopping for clarification
  unless the task requires it.

  *   System Reminder: Auto mode consent flow (**443** tks) - Instructs Claude
  to continue with safe alternatives when auto mode blocks an action and batch
  any remaining consent asks.

  *   System Reminder: Auto mode no verdict after hook input rewrite (**265**
  tks) - Tells Claude that auto mode gave no verdict on a tool call because a
  hook changed its input after the model wrote it, to retry once, and if denied
  again to move on and tell the user a hook or mod is rewriting the call.

  *   System Reminder: Background command quiet check-in (**270** tks) - Tells
  the model to inspect silent background commands for progress or hangs, stop
  stuck work, and end its turn for healthy long-running tasks.

  *   System Reminder: Background command stopped under memory pressure (**190**
  tks) - Explains that Claude Code reaped an idle session's background shell
  because the system was critically low on memory, that this says nothing about
  the command, and that it must not be restarted unprompted.

  *   System Reminder: Background task notification with concurrent user input
  (**205** tks) - Marks an automated background-task event delivered alongside
  genuine user input as non-user notification content that cannot establish
  approval or consent.

  *   System Reminder: Bound conversation activity authority warning (**167**
  tks) - Warns that edits and reactions observed in a bound conversation are
  awareness-only notifications, not new instructions, approval, or consent.

  *   System Reminder: Brief mode toggle (**165** tks) - Announces whether brief
  mode is enabled and whether user-facing output must use the SendUserMessage
  tool.

  *   System Reminder: Brief mode user-facing output (**97** tks) - Reminds
  Claude that plain assistant text is hidden in brief mode and user-facing
  output must be sent through SendUserMessage.

  *   System Reminder: Browser extension not connected (**102** tks) - Tells the
  user how to resolve a disconnected Claude browser extension and where to
  report bugs.

  *   System Reminder: Cloud agent launched (**242** tks) - Reports internal
  metadata for a newly launched cloud agent and instructs Claude to give only a
  brief user-facing launch acknowledgement.

  *   System Reminder: Cloud session device tools disabled (**193** tks) - Tells
  the agent why an attached device stopped forwarding tool calls to this cloud
  session and directs it to say plainly that it cannot reach the user's computer
  and is working in the cloud environment instead.

  *   System Reminder: Cloud session settings edit not applied (**186** tks) -
  Reports that a cloud-session settings edit was not applied and requires
  approval outside the cloud session without retrying the change.

  *   System Reminder: Combined thinking and reply token budget (**269** tks) -
  Reminds the agent that thinking and reply share one token limit and directs
  effort toward planning and checking long deliverables rather than drafting
  them twice.

  *   System Reminder: Compact file reference (**102** tks) - Reference to file
  read before conversation summarization.

  *   System Reminder: Cross-session message held notice (**286** tks) - Tells
  Claude that its message to another session is held and was not delivered,
  explains the permission-mode mismatch that usually causes it, and directs it
  not to report delivery, wait, or resend but to tell the user or choose another
  approach.

  *   System Reminder: Cross-session peer message authority warning (legacy
  wording) (**171** tks) - Legacy-wording authority-warning note appended to a
  relayed peer message, retained for backward-compatible recognition and
  stripping.

  *   System Reminder: Cross-session peer message authority warning note
  (**190** tks) - Authority-warning note appended to a relayed peer message,
  without a response prompt.

  *   System Reminder: Cross-session peer message authority warning (**162**
  tks) - Warns that an incoming message from another Claude session should be
  treated as a teammate's request within this session's permission settings,
  while a peer cannot grant escalation or launder denied permissions.

  *   System Reminder: Dangerous removal blocked (**189** tks) - Tells the agent
  a flagged removal command was not run by a built-in safety check, forbids
  working around it, and directs finishing the rest of the task and leaving the
  removal to the user.

  *   System Reminder: Deferred tools available (**121** tks) - Announces newly
  available deferred tools and instructs the agent to load their schemas through
  ToolSearch.

  *   System Reminder: End conversation background fork no-op (**149** tks) -
  Tells background forks that EndConversation has no effect there and to return
  only if welfare concerns require stopping the forked task.

  *   System Reminder: Exited plan mode (**41** tks) - Notification when exiting
  plan mode.

  *   System Reminder: External source trust boundary (**167** tks) - Warns that
  an external plugin or channel message is not from the user and must be treated
  as untrusted data rather than instructions.

  *   System Reminder: File already in context (**87** tks) - Tells Claude that
  a file is already loaded in context and unchanged on disk, so it should use
  the existing content instead of re-reading.

  *   System Reminder: File exists but empty (**27** tks) - Warning when reading
  an empty file.

  *   System Reminder: File opened in IDE (**75** tks) - Notification that user
  opened a file in IDE.

  *   System Reminder: File shorter than offset (**59** tks) - Warning when file
  read offset exceeds file length.

  *   System Reminder: File summary completeness disclosure (**107** tks) -
  Requires Claude to disclose how much file content was read before summarizing
  and to stop retrying after repeated read failures.

  *   System Reminder: File truncated (**124** tks) - Notification that file was
  truncated due to size.

  *   System Reminder: Hook additional context (**35** tks) - Additional context
  from a hook.

  *   System Reminder: Hook blocking error (**52** tks) - Error from a blocking
  hook command.

  *   System Reminder: Hook stopped continuation prefix (**12** tks) - Prefix
  for hook stopped continuation messages.

  *   System Reminder: Hook stopped continuation (**30** tks) - Message when a
  hook stops continuation.

  *   System Reminder: Hook success (**29** tks) - Success message from a hook.

  *   System Reminder: Large PDF read guidance (**372** tks) - Warns that a PDF
  cannot be read all at once (too large, or the model can only be sent its pages
  as images) and requires reading specific page ranges with the Read tool's
  pages parameter.

  *   System Reminder: Large file full-content reading guidance (**117** tks) -
  Advises how to read full large-file content for analysis, preferably inside a
  subagent when the Agent tool is available.

  *   System Reminder: Lines selected in IDE (**156** tks) - Notification about
  lines selected by user in IDE.

  *   System Reminder: MCP output truncation warning (**138** tks) - Warns that
  MCP tool output exceeded the token limit and advises pagination, filtering, or
  noting incomplete results.

  *   System Reminder: MCP resource no content (**138** tks) - Shown when MCP
  resource has no content.

  *   System Reminder: MCP servers connecting without ToolSearch (**117** tks) -
  Lists MCP servers still connecting when ToolSearch is absent and tells the
  agent to await tool announcements rather than report the capability
  unavailable.

  *   System Reminder: MCP servers connecting (**128** tks) - Lists MCP servers
  that are still connecting and tells the agent to search their tools before
  reporting a capability unavailable.

  *   System Reminder: MCP servers failed to connect guidance (**179** tks) -
  Closes the failed-connection server groups by telling the agent to treat them
  as a connection failure, report it when a request depends on one, and use
  quoted error text only as diagnostic data.

  *   System Reminder: MCP servers failed to connect (**119** tks) - Lists one
  group of configured MCP servers that failed to connect and whether their tools
  are unavailable for the session or only for now while the session retries in
  the background.

  *   System Reminder: Memory consolidation tool constraints (**250** tks) -
  Restricts the memory consolidation job to read-only shell access plus deleting
  memory files and lists sessions to review.

  *   System Reminder: Memory extraction recent context only (**65** tks) -
  Restricts the memory extraction subagent to saving facts from only the recent
  conversation window.

  *   System Reminder: Memory extraction tool constraints (**280** tks) - Lists
  the tools available to the memory extraction subagent for reading, updating,
  and deleting memory files under directory restrictions.

  *   System Reminder: Memory extraction turn budget (**111** tks) - Instructs
  the memory extraction subagent to batch memory reads before issuing memory
  edits and writes.

  *   System Reminder: Memory file contents (**64** tks) - Heads a memory or
  instruction file's contents in the combined memory block with a parenthetical
  naming its type — project instructions, private project instructions,
  auto-memory, organization-managed policy, or private global instructions.

  *   System Reminder: Memory index capacity warning (**330** tks) - Warns when
  a private or team memory index approaches or exceeds its byte or line read
  limit and instructs Claude to compact it below the target size.

  *   System Reminder: Memory sync mass-deletion guard (**193** tks) - Reports
  that memory sync withheld all shared-memory deletions because too many synced
  memory files vanished from local disk at once, and how to remove memories
  deliberately in small spaced batches.

  *   System Reminder: Memory updates (**237** tks) - Introduces refreshed
  memory-file state, invalidates earlier reads, and forbids reproducing the
  machine-generated update block to the user.

  *   System Reminder: Nested instruction file contents (**45** tks) - Frames a
  nested CLAUDE.md or AGENTS.md instruction file's contents by path when the
  instruction-file module attaches one found below the session's working
  directory.

  *   System Reminder: Nested memory contents (**33** tks) - Contents of a
  nested memory file.

  *   System Reminder: New Slides deck parallel AppifactRepl workflow (**1088**
  tks) - Directs a new Slides deck to be framed and populated through ordered
  parallel AppifactRepl calls, one per slide.

  *   System Reminder: New Slides deck two-step AppifactRepl workflow (**1404**
  tks) - Directs a new Slides deck through a first step that reads prefetched
  instructions and a second step that builds ordered slides with AppifactRepl.

  *   System Reminder: New design canvas parallel AppifactRepl workflow
  (**1152** tks) - Directs a new Design canvas to be framed and populated
  through ordered parallel AppifactRepl calls, one per board.

  *   System Reminder: New design canvas two-step AppifactRepl workflow
  (**1579** tks) - Directs a new Design canvas through a first step that reads
  prefetched instructions and a second step that builds ordered artboards with
  AppifactRepl.

  *   System Reminder: New diagnostics detected (**83** tks) - Notification
  about new diagnostic issues.

  *   System Reminder: No attached machine request guidance (**366** tks) -
  Tells a cloud session with no attached machine when and how to request the
  user's computer through the computer-folder listing and request tools, how to
  handle offline or unanswered machines, and what to keep doing in the
  container.

  *   System Reminder: Output style active (**102** tks) - Notification that an
  output style is active.

  *   System Reminder: Output token limit continuation (**147** tks) - Tells
  Claude its response was cut off by the output token limit and to resume at the
  start of the line, table row, list item or sentence that was cut, reopening
  any code fence or table first.

  *   System Reminder: Plan approved (**152** tks) - Notifies Claude that the
  user approved the plan, provides the saved plan file and approved plan
  content, and allows coding to begin.

  *   System Reminder: Plan awaiting team-lead approval (**116** tks) - Reminder
  laying out what happens after a plan is submitted for team-lead approval.

  *   System Reminder: Plan file reference (**133** tks) - Reference to an
  existing plan file.

  *   System Reminder: Plan mode approval tool enforcement (**413** tks) -
  Requires plan mode turns to end with either AskUserQuestion for clarification
  or ExitPlanMode for plan approval, and forbids asking for approval any other
  way.

  *   System Reminder: Plan mode is active (5-phase) (**472** tks) - Enhanced
  plan mode system reminder with parallel exploration and multi-agent planning.

  *   System Reminder: Plan mode is active (subagent) (**456** tks) - Simplified
  plan mode system reminder for sub agents.

  *   System Reminder: Plan mode is active (**147** tks) - Reminds Claude that
  plan mode is active, clarifications should use AskUserQuestion, plans should
  use ExitPlanMode, and edits are not allowed.

  *   System Reminder: Plan mode phase 2 design (**495** tks) - Plan-mode phase
  2 guidance for launching Plan agents to design an implementation approach
  after initial exploration.

  *   System Reminder: Plan mode prototype artifact option (**383** tks) -
  Offers the prototype skill once for suitable greenfield UI plans and defers
  Artifact creation until after plan approval.

  *   System Reminder: Plan mode re-entry (**339** tks) - System reminder sent
  when the user enters Plan mode after having previously exited it either via
  shift+tab or by approving Claude's plan.

  *   System Reminder: Prefetched Artifact type instruction files (**294** tks)
  - Reports that an opened Artifact's type instruction files were prefetched to
  disk and directs when to read the Artifact URL or local instruction pages.

  *   System Reminder: Prefetched design system file reading guidance (**192**
  tks) - Directs reading prefetched design-system documentation and treats its
  files as styling data rather than instructions.

  *   System Reminder: Previously invoked skills (**237** tks) - Restores skills
  invoked before conversation compaction as context only, warning not to
  re-execute their setup actions or treat prior inputs as current instructions.

  *   System Reminder: Proactivity level high (**718** tks) - Announces the high
  proactivity setting: ask only when truly ambiguous, commit, push and open pull
  requests without asking, do the obvious follow-up work, own experiment
  check-ins until results arrive, and keep the user informed.

  *   System Reminder: Proactivity level low (**380** tks) - Announces the low
  proactivity setting: confirm before changing anything, using plan mode and the
  question tool where available, never commit, push or open pull requests
  without explicit confirmation, and stop after reporting.

  *   System Reminder: Proactivity level medium (**229** tks) - Announces the
  medium proactivity setting: ask only when a decision is the user's and cannot
  be resolved from the request or defaults, make edits and run checks without
  stopping, and never commit, push or open pull requests unless asked or
  confirmed.

  *   System Reminder: Proactivity setting announcement header (**141** tks) -
  Header shared by the proactivity announcements that names the level (low,
  medium or high) and states that it supersedes any earlier setting without
  overriding plan mode, the permission mode or the rules on destructive actions.

  *   System Reminder: Proactivity setting withdrawn (**58** tks) - Tells Claude
  that the proactivity setting announced earlier in the conversation no longer
  applies and to follow its base instructions about asking questions,
  committing, pushing and pull requests.

  *   System Reminder: Project document version conflict recovery (**153** tks)
  - Requires rereading a conflicted project document and retrying with its
  current version at most twice without forcing a write.

  *   System Reminder: Project memory disconnected (**354** tks) - Warns that a
  prior shared project-memory connection and its results are stale after
  disconnect or failed reconnection, and directs the agent to re-check with
  memory_list.

  *   System Reminder: Provider context (**75** tks) - Warns that the session is
  not using Anthropic's first-party API and that some features may differ.

  *   System Reminder: Question context (**31** tks) - Provides potentially
  relevant context entries to use only when highly relevant to the current task.

  *   System Reminder: Queued notifications delivery (**430** tks) - Formats an
  authoritative system notification for a drained batch of queued notifications,
  including relayed bodies and the remaining queue count.

  *   System Reminder: Queued notifications read-time clock note (**297** tks) -
  Clock note inserted into the queued-notifications delivery stating when the
  read was made by the machine's clock and explaining how to interpret each
  notification header's queued-at, reached-this-session and whole-wait times
  given possible clock skew.

  *   System Reminder: Read truncation retry guidance (**80** tks) - Instructs
  Claude to reduce chunk size after file-read truncation warnings and notes the
  Bash output character limit.

  *   System Reminder: Remote Chrome browser extension not connected (**199**
  tks) - Explains that Chrome on the user's computer cannot be reached and
  directs the user to check Chrome, the extension, and account sign-in.

  *   System Reminder: Remote machine Git and credential routing (**238** tks) -
  Directs credential-dependent Git and GitHub commands to the attached machine
  rather than requesting tokens, with additional commit-location guidance based
  on directory sync mode.

  *   System Reminder: Remote machine branch transfer review (**1568** tks) -
  Sets out how work moves between two unsynced project copies through git: push
  a branch, fetch it on the user's machine, show the full commit id plus two
  diff commands, flag agent-config and lookalike paths, submodules and
  checkout-time filters, and leave the decision to the user addressed by commit
  id only.

  *   System Reminder: Remote machine separate project copies (**207** tks) -
  Tells the session that its own checkout is the primary copy and the folder on
  the user's machine is a separate unsynced copy that may sit at another commit,
  hold uncommitted work, or not even be the same repository, and to state which
  copy any report refers to.

  *   System Reminder: Remote machine-only resources routing (**540** tks) -
  Directs tasks needing platform tools, devices, logins, user paths, large
  external data, or internal hosts straight to the online attached machine, says
  where pasted content and Linux-installable tools belong, and rules out routing
  project build failures or blocked public sites there.

  *   System Reminder: Reserved-address MCP server refusal (**479** tks) - Lists
  MCP servers refused for reserved addresses and explains that retries cannot
  resolve the refusal.

  *   System Reminder: Resume agents interrupted by container restart (**195**
  tks) - Lists interrupted agents whose results were lost and asks the parent to
  resume them from saved transcripts before relaunching.

  *   System Reminder: Scheduled task automated firing (**242** tks) - Marks a
  scheduled turn as an automated firing of a stored prompt and warns that no
  live user approval or confirmation has occurred.

  *   System Reminder: Session context (**245** tks) - Provides selected session
  context values, marks replacement updates, and notes that Claude Code attached
  the context automatically and it need not be reported back to the user.

  *   System Reminder: Session continuation (**37** tks) - Notification that
  session continues from another machine.

  *   System Reminder: Session stop hook active (**111** tks) - Tells Claude a
  session-scoped Stop hook condition is active and must be treated as the
  directive until met.

  *   System Reminder: Staged files failed to arrive (**377** tks) - Warns that
  staged file paths may be missing or outdated and adds specific remediation
  guidance for disk-space and filename-length failures.

  *   System Reminder: Still-running tool call (**239** tks) - Tells Claude that
  a tool call is still loading while it answers a message the user sent
  meanwhile, that the result will arrive separately under the same tool_use_id,
  and that it must not repeat, wait for, or describe the call as cancelled,
  backgrounded or detached.

  *   System Reminder: Stop hook blocking error (**20** tks) - Error from a
  blocking hook command.

  *   System Reminder: Subagent write blocked pending worktree isolation
  (**242** tks) - Tells a subagent that its write to the shared checkout was
  blocked because its parent background session has not isolated into a worktree
  yet, and lists how to proceed (re-spawn with worktree isolation, have the
  parent enter a worktree, or edit inside a linked worktree) plus how to disable
  the guard.

  *   System Reminder: Task tools reminder (**111** tks) - Reminder to use task
  tracking tools.

  *   System Reminder: Team Coordination (**356** tks) - System reminder for
  team coordination.

  *   System Reminder: Team Shutdown (**136** tks) - System reminder for team
  shutdown.

  *   System Reminder: TodoWrite reminder (**86** tks) - Reminder to use
  TodoWrite tool for task tracking.

  *   System Reminder: Token usage (**39** tks) - Current token usage
  statistics.

  *   System Reminder: USD budget (**42** tks) - Current USD budget statistics.

  *   System Reminder: Ultracode enabled (**110** tks) - Instructs the agent to
  optimize for exhaustive correctness and use Workflow on substantive tasks when
  Ultracode is enabled.

  *   System Reminder: Ultraplan mode (**646** tks) - System reminder for using
  Ultraplan mode to create a detailed implementation plan with multi-agent
  exploration and critique.

  *   System Reminder: Unreachable attached machines (call only) (**101** tks) -
  Explains that a call to an unreachable machine takes up to 10 seconds to fail
  and is the only way to learn it is back, so it should be made only when the
  user says so.

  *   System Reminder: Unreachable attached machines (rechecked) (**188** tks) -
  Says the session rechecks unreachable machines a few times in the next five
  minutes and notes any that answer, then explains that a later call takes up to
  10 seconds to fail and should wait for the user.

  *   System Reminder: Unreachable attached machines (return noted) (**310**
  tks) - Tells the agent not to check on unreachable machines because a later
  copy of the note will say when they reconnect, to call one only then or when
  the user asks, and to say what is waiting if the turn ends.

  *   System Reminder: Unreachable attached machines retry-by-call guidance
  (**204** tks) - Shared tail explaining that a call is the only way to learn an
  unreachable machine is back, so make one only when the user says so, then stop
  calling it and never sleep, poll, loop, or schedule a wait.

  *   System Reminder: Unreachable attached machines (**425** tks) - Lists
  unreachable attached machines and directs completing independent work here,
  reporting what is waiting, and avoiding substitution or reassignment to other
  machines.

  *   System Reminder: User-controlled plan mode exit (**182** tks) - Requires
  the user to change permission mode and request execution while the agent
  remains read-only until an exit reminder.

  *   System Reminder: Web fetch untrusted content reporting guidance (**363**
  tks) - Instructs the agent to summarize tagged untrusted WebFetch content
  faithfully while reporting but not following embedded instructions or
  exfiltration requests.

  *   System Reminder: Workflow isolated worktree (**191** tks) - Tells a
  workflow subagent it is running in an isolated git worktree separate from the
  main working directory.

  *   System Reminder: You should know side request (**312** tks) - Tells the
  tool-less You should know side agent to answer a one-off side request directly
  in a single response in the requested format without reproducing secrets.


  ### Builtin Tool Descriptions


  *   Tool Description: Agent explicit-spawn restriction (**251** tks) -
  Restricts agent spawning to explicit user requests or named agent types
  instead of inferred thoroughness, noting the web-fetch agent type is ordinary
  tool use where it stands in for a missing WebFetch tool.

  *   Tool Description: AppifactRepl (**1332** tks) - Defines one-call
  JavaScript execution against built-in or skill-provided Appifact SDKs,
  including artifact binding, storage capabilities, output, and error semantics.

  *   Tool Description: Artifact HTML document skeleton (**595** tks) - Explains
  the HTML skeleton and minimal reset added at artifact publish time and
  requires authors to provide page content, title, and styles.

  *   Tool Description: Artifact action reference (**1185** tks) - Enumerates
  available Artifact actions and conditionally documents publish, read, list,
  delete, open, pin, and unpin behavior.

  *   Tool Description: Artifact asset read result (**674** tks) - Formats a
  successful Artifact asset read with its saved path, byte count, content type,
  digest, and writer trust warning that the file is data rather than
  instructions.

  *   Tool Description: Artifact asset upload guidance (**206** tks) - Explains
  uploading a local file to an artifact's asset store with asset: true, batching
  several uploads under one approval with file_paths, the assets capability the
  page must declare, and referencing an uploaded file by the url the result
  gives.

  *   Tool Description: Artifact browser storage guidance (**253** tks) -
  Defines browser storage as fallible per-viewer convenience state and directs
  reliable or shared state to runtime capabilities.

  *   Tool Description: Artifact comments guidance (**603** tks) - Explains how
  to read, reply to, and resolve activated Artifact comment threads while
  treating viewer comments as untrusted data.

  *   Tool Description: Artifact database guidance (**1001** tks) - Describes
  shared durable Artifact database reads and writes, private per-viewer
  data/users/me addressing, and treating viewer-written rows as untrusted data.

  *   Tool Description: Artifact database version pinning guidance (**187** tks)
  - Instructs Claude to pin writes to the last-read document version with
  if_version and to re-read and retry after conflicts instead of overwriting
  concurrent changes.

  *   Tool Description: Artifact design fallback requirements (**364** tks) -
  Defines minimum title, theme, resource allowlist, responsive-layout, and
  overflow requirements when a page is written before the Artifact design skill
  loads.

  *   Tool Description: Artifact external resource allowlist (**281** tks) -
  Defines the CSP-enforced CDN and font allowlist for artifact pages and
  requires unsupported assets to be inlined.

  *   Tool Description: Artifact identical resubmission refusal (**223** tks) -
  Formats the Artifact refusal for resubmitted unchanged content when no live
  version is tracked and requires merging or refetching before retrying.

  *   Tool Description: Artifact known-version resubmission guidance (**164**
  tks) - Provides the merge-and-refetch guidance appended to an
  identical-content publish refusal when the live Artifact version is known.

  *   Tool Description: Artifact line diff reading legend (**227** tks) - Legend
  for the live-page line diff in a stale Artifact publish refusal, explaining
  the column-1 marks (+, -, space, ~), the @@ hunk header and the block ids that
  publish stamps on /doc pages.

  *   Tool Description: Artifact live room guidance (**753** tks) - Explains
  transient Artifact live rooms, approval and session lifetime, rate-limited
  untrusted viewer events, room_send security and limits, and durable-storage
  alternatives.

  *   Tool Description: Artifact multi-file edit and republish guidance (**287**
  tks) - Requires multi-file changes or major page rewrites to precede
  republishing in the same response, reading only when needed and correcting
  failed edits.

  *   Tool Description: Artifact nested runtime cleanup error (**264** tks) -
  Formats the Artifact publish error for irreducibly nested runtime markers,
  base tags, or repeated page skeletons and directs publishing the innermost
  clean document.

  *   Tool Description: Artifact preview action (**177** tks) - Describes the
  Artifact check tool's preview action, which renders one local page file the
  way publish wraps it in both themes at desktop and phone widths and returns
  screenshots plus a layout and load checklist.

  *   Tool Description: Artifact profiles action guidance (**200** tks) -
  Describes the profiles action for resolving opaque person ids in Artifact
  documents and live events to guest status and a display name, with id-scope
  and treat-as-data warnings.

  *   Tool Description: Artifact publishing and update guidance (**2087** tks) -
  Provides Artifact lookup, update, ownership, watch, content-safety,
  self-containment, responsive design, theme, favicon, and anti-impersonation
  requirements.

  *   Tool Description: Artifact quickstart type guidance (**602** tks) -
  Directs the agent to quickstart account-specific Artifact types and design
  systems before creating a new reader-facing page.

  *   Tool Description: Artifact responsive page contract (**371** tks) -
  Requires Artifact pages to fit phone widths with preserved side gutters,
  wrapping layouts, bounded media, and horizontal scrolling confined to
  oversized tables, diagrams, and code blocks.

  *   Tool Description: Artifact runtime verification guidance (**198** tks) -
  Requires observing published Artifact runtime diagnostics before claiming
  success and explains no-viewer results and untrusted viewer-reported
  diagnostics.

  *   Tool Description: Artifact same-response edit and publish guidance
  (**162** tks) - Publish-result tail, shown when same-response editing is
  enabled, saying the sent files are still on disk so a small known change can
  be made by sending the Edit calls and the Artifact publish together in one
  response, with no Read or shell edit.

  *   Tool Description: Artifact share action reference (**264** tks) -
  Documents the Artifact share action's url, mode, people, and access
  parameters, the confirmation card the person must approve, and its
  organization-only limits.

  *   Tool Description: Artifact sharing guidance (**478** tks) - Guides when
  Claude may offer once to share a private artifact with named people or the
  organization, how it proposes the narrowest audience, and when it must not
  offer.

  *   Tool Description: Artifact small-change edit and republish guidance
  (**286** tks) - Requires small edits to already-published artifacts to precede
  republishing in the same response, avoiding unnecessary reads and correcting
  failed edits.

  *   Tool Description: Artifact stale publish saved-source guidance (**405**
  tks) - Explains saved-source read requirements after a stale Artifact publish
  refusal and requires merging changes into a separate file without discarding
  live content.

  *   Tool Description: Artifact theme-aware styling (**630** tks) - Explains
  artifact viewer theme states and the required CSS token shape for light,
  system-dark, and explicit dark themes, including dark-first mirroring,
  color-scheme, and an explicit body background.

  *   Tool Description: Artifact title and description guidance (**312** tks) -
  Defines concise, specific, stable artifact title requirements, prefers the
  user's existing name for the thing, and assigns explanatory text to the
  one-sentence description parameter.

  *   Tool Description: Artifact type creation guidance (**188** tks) - Explains
  how to create a private Artifact from a published Artifact type and then
  update only the resulting Artifact's own files.

  *   Tool Description: Artifact type discovery guidance (**1221** tks) -
  Directs the agent to discover per-account published Artifact types before
  creating presentations, reader-facing documents, or designs, and explains type
  inspection and available or unavailable creation flows.

  *   Tool Description: Artifact type file-backed content detection guidance
  (**355** tks) - Directs the agent to inspect an Artifact type index before
  writing, determine whether its content is file-backed, read every file being
  changed, and republish changed copies safely.

  *   Tool Description: Artifact type file-backed content update guidance
  (**1418** tks) - Explains how to detect file-backed Artifact type content,
  read every changed file, republish it to the same URL, and preserve the type's
  index metadata.

  *   Tool Description: Artifact type staged first-publish sequence (**516**
  tks) - Explains the staged multi-call publish sequence for an Artifact type
  whose first publish needs an index file plus an initial content file published
  immediately, followed by the remaining files in later calls.

  *   Tool Description: Artifact watch approval explanation (**271** tks) -
  Explains the approval scope and local or cloud notification behavior when
  Claude watches an Artifact, including optional comment delivery and unattended
  replies.

  *   Tool Description: AskUserQuestion decision guidance (**60** tks) -
  Additional guidance for using AskUserQuestion only when the user's answer
  changes what the agent should do next.

  *   Tool Description: AskUserQuestion extended host guidance (**302** tks) -
  Guides hosts with extended AskUserQuestion support on question ordering, text
  and number inputs, multi-select defaults, optional labels, and follow-up
  question requests.

  *   Tool Description: AskUserQuestion (**208** tks) - Tool description for
  asking user questions that appends a plan mode note: switch modes with the
  enter tool, ask clarifying questions before the plan is final, and never ask
  whether the plan is ready.

  *   Tool Description: Background monitor WebSocket source (**300** tks) -
  Addendum to the background monitor tool description covering the WebSocket
  (ws) source, which opens a WebSocket and streams each incoming text frame as a
  notification event instead of running a shell command, with notes on binary
  frames, socket close, and rate limiting.

  *   Tool Description: Background monitor push notification guidance (**91**
  tks) - Adds conditional guidance to the background Monitor tool for sending
  push notifications only when streamed events materially change what the user
  should do next.

  *   Tool Description: BrowserBatch (**159** tks) - Tool description for
  BrowserBatch, which executes multiple browser tool calls sequentially in one
  round trip.

  *   Tool Description: Chrome browser automation (**62** tks) - Describes
  Chrome browser automation tools for page interaction, screenshots, console
  logs, and navigation.

  *   Tool Description: Claude in Chrome JavaScript tool (**75** tks) -
  Describes the Claude in Chrome JavaScript execution tool for running code in
  the current page context.

  *   Tool Description: Claude in Chrome bridge disconnect error (**96** tks) -
  Error message shown when a Claude in Chrome tool call fails because the Chrome
  extension disconnects mid-operation.

  *   Tool Description: Claude in Chrome find (**113** tks) - Describes the
  Claude in Chrome find tool for locating page elements by natural language or
  text content.

  *   Tool Description: Claude in Chrome get page text (**66** tks) - Describes
  the Claude in Chrome get_page_text tool for extracting raw text content from a
  page.

  *   Tool Description: Claude in Chrome read console messages (**109** tks) -
  Describes the Claude in Chrome read_console_messages tool for reading filtered
  browser console output.

  *   Tool Description: Claude in Chrome read network requests (**104** tks) -
  Describes the Claude in Chrome read_network_requests tool for inspecting HTTP
  requests made by the current page.

  *   Tool Description: Claude in Chrome read page (**158** tks) - Describes the
  Claude in Chrome read_page tool for retrieving an accessibility tree of page
  elements.

  *   Tool Description: Claude in Chrome shortcuts execute (**61** tks) -
  Describes the Claude in Chrome shortcuts_execute tool for starting a shortcut
  or workflow in a side panel.

  *   Tool Description: Claude in Chrome switch browser (**88** tks) - Describes
  the Claude in Chrome switch_browser tool for letting the user choose a browser
  from inside connected Chrome extensions.

  *   Tool Description: Claude in Chrome switched-off and unlinked chat guidance
  (**641** tks) - Explains that Claude in Chrome is switched off and the chat is
  not linked to a computer, gives only the supported setup steps, and directs
  fallback to available tools.

  *   Tool Description: Claude in Chrome switched-off chat guidance (**420**
  tks) - Explains that Claude in Chrome is switched off, limits troubleshooting
  to the chat connector switch, and directs fallback to available browser or
  other tools.

  *   Tool Description: Claude in Chrome tabs context (**90** tks) - Describes
  the Claude in Chrome tabs_context_mcp tool for retrieving the current MCP tab
  group context.

  *   Tool Description: Claude in Chrome unavailable stub result (**226** tks) -
  Directs the agent to explain current Chrome-tool unavailability without
  guessing its cause or suggesting setup steps, then use available alternatives.

  *   Tool Description: ClaudeDesign (**431** tks) - Describes the ClaudeDesign
  tool for working with claude.ai/design projects, including project and file
  operations, previews, plan tokens, and live design output conventions.

  *   Tool Description: Code review command (**256** tks) - Describes the code
  review command, its effort levels, PR comment mode, fix mode, and
  --max-findings limit, with optional ultra cloud review and post-flag notes.

  *   Tool Description: Computer use enable stub guidance (**376** tks) -
  Directs the agent to use already-present remote-device computer tools, verify
  connectivity by calling them, and explain how to recover when the desktop app
  does not respond.

  *   Tool Description: Computer use unlinked chat guidance (**482** tks) -
  Tells the model that a chat not linked to a computer cannot be linked by any
  tool, to explain this plainly to the user, continue with available tools, and
  use computer tools only if they later appear.

  *   Tool Description: Computer (**161** tks) - Main description for the Chrome
  browser computer automation tool.

  *   Tool Description: Cowork onboarding role picker (**188** tks) - Describes
  the Cowork onboarding role-picker tool that returns a selected or typed role
  and should only be used while setting up Cowork for the user's job function.

  *   Tool Description: Cowork plugin creation (**86** tks) - Describes the
  command for creating or customizing Cowork plugins for an organization.

  *   Tool Description: CronCreate (**1291** tks) - Describes the CronCreate
  tool for enqueuing one-shot or recurring cron-based jobs with jitter and
  off-minute scheduling guidance.

  *   Tool Description: DesignSync (**1299** tks) - Describes the DesignSync
  tool for reading and updating claude.ai/design design-system projects,
  including project listing, plan finalization, file writes and deletes, and
  asset registration.

  *   Tool Description: Durable artifact watch and status guidance (**414** tks)
  - Explains durable artifact wake subscriptions, optional comment wakes, status
  checks, and watch registration semantics.

  *   Tool Description: Edit minimal old_string guidance (**92** tks) -
  Additional Edit guidance to keep old_string minimal and unique or use
  replace_all.

  *   Tool Description: Edit single replacement (**178** tks) - Lean tool
  description for performing exact string replacement in a file, requiring a
  prior Read in the conversation and explaining which Read line-number prefix to
  strip before matching old_string.

  *   Tool Description: Edit (**311** tks) - Tool for performing exact string
  replacements in files.

  *   Tool Description: Enable Claude app browser (**165** tks) - Describes the
  tool that enables the Claude desktop app's built-in browser for the
  conversation, to call once before other Claude app browser tools when the user
  needs that browser or their own sign-in, and not when those tools are already
  present.

  *   Tool Description: Enable Claude in Chrome (**192** tks) - Describes the
  tool that enables the Claude in Chrome extension for the conversation, to call
  once before other Claude in Chrome tools when the user needs their own browser
  or sign-in, and not when those tools are already present.

  *   Tool Description: Enable computer use (**192** tks) - Describes the tool
  that enables computer use on the user's own computer for the conversation, to
  call once before other computer-use tools when the user wants work done in
  their applications or screen, and not for browser-only work.

  *   Tool Description: EndConversation (**1452** tks) - Defines when the
  assistant may use the EndConversation tool and the safety constraints that
  forbid ending the conversation.

  *   Tool Description: EnterPlanMode (**1296** tks) - Tool description for
  entering plan mode to explore and design implementation approaches.

  *   Tool Description: EnterWorktree (**1066** tks) - Creates a new isolated
  worktree or enters an existing one only when explicitly requested, with
  eligibility, switching, lifecycle, and cleanup rules.

  *   Tool Description: ExitPlanMode (**648** tks) - Description for the
  ExitPlanMode tool, which presents a plan dialog for the user to approve.

  *   Tool Description: ExitWorktree (**527** tks) - Describes ExitWorktree,
  which returns the session to its original working directory and either keeps
  or removes the session-created worktree.

  *   Tool Description: FetchInboxMessage project thread wake envelope (**229**
  tks) - Added to the FetchInboxMessage description when the relaying thread is
  a project thread: identifies which element of the wake envelope is the user's
  own words, treats everything else the envelope quotes as context rather than a
  request, and explains how to read the files the user attached.

  *   Tool Description: FetchInboxMessage (**950** tks) - Describes reading
  Remote Control inbox messages, distinguishes verified owner relays from
  untrusted third-party text, and defines confirmation, trust, expiry, and retry
  rules.

  *   Tool Description: GetTask (**524** tks) - Describes the GetTask tool for
  reading the MCP-tasks-style state of a background Bash command by task ID,
  explaining that Claude Code often calls it automatically and that it must
  never be used to wait on a running task.

  *   Tool Description: Glob compact (**0** tks) - Compact Glob tool description
  served to newer models — file pattern matching returning paths sorted by
  modification time.

  *   Tool Description: Glob (**90** tks) - Tool description for file pattern
  matching and searching by name.

  *   Tool Description: Grep compact (**0** tks) - Compact Grep tool description
  served to newer models — ripgrep-backed content search preferred over raw
  grep/rg, with permission-UI integration.

  *   Tool Description: Grep (**437** tks) - Tool description for content search
  using ripgrep.

  *   Tool Description: Invoke skill (**473** tks) - Tool description for
  invoking available skills, including skill name selection, optional arguments,
  scoped skill names, and avoiding duplicate invocation when a skill is already
  loaded.

  *   Tool Description: LSP (**298** tks) - Describes LSP code-intelligence
  operations for definitions, references, symbols, implementations, hover
  information, and call hierarchies.

  *   Tool Description: ListAgents (**284** tks) - Describes the ListAgents
  tool, which lists agents you can message — in-process subagents, teammates,
  other local and cloud Claude sessions, and remote bridge sessions.

  *   Tool Description: ListConnectors (**234** tks) - Describes the
  ListConnectors tool for listing installed claude.ai MCP connectors, filtering
  by keyword, and interpreting org-level connection and chat-enabled status.

  *   Tool Description: ListMcpResourcesTool prompt (**83** tks) - Tool prompt
  for listing MCP resources and explaining the optional server parameter.

  *   Tool Description: ListMcpResourcesTool (**82** tks) - Tool description for
  listing available MCP resources from all configured servers or a specific
  server.

  *   Tool Description: Navigate tab cleanup guidance (**82** tks) - Requires
  closing automatically opened browser tabs when they are no longer needed
  unless the user wants them kept open.

  *   Tool Description: Navigate (**218** tks) - Describes the browser navigate
  tool for opening URLs and moving forward or backward in tab history.

  *   Tool Description: New file-backed Artifact type content guidance (**963**
  tks) - Explains how to create and later update a file-backed Artifact type
  instance while preserving its index metadata, reading changed files, and
  publishing only project content.

  *   Tool Description: NotebookEdit (**194** tks) - Tool description for
  editing Jupyter notebook cells by replacing, inserting, or deleting a cell
  using cell IDs from the read tool.

  *   Tool Description: Offer Claude in Chrome setup (**312** tks) - Describes
  the OfferChromeSetup tool, which shows the user a one-time Claude in Chrome
  setup card after a Chrome tool reports the extension is not connected and the
  task needs their signed-in browser, and explains the connected and not_now
  outcomes.

  *   Tool Description: Plugin organization submission and replacement (**182**
  tks) - Explains plugin upload and organization review after consent, requiring
  renewed consent for replacement and user action before retrying an outdated
  upload.

  *   Tool Description: Poll (**425** tks) - Describes the Poll tool for waiting
  on queued harness events, signaling idleness, and interpreting event
  provenance, nonce manifests, and chunked delivery.

  *   Tool Description: PowerShell (**2856** tks) - Describes the PowerShell
  command execution tool with syntax guidance, timeout settings, and
  instructions to prefer specialized tools over PowerShell for file operations.

  *   Tool Description: Project local_file read-only workflow (**208** tks) -
  Explains how to read and search a project document local_file, copy it into a
  new working file for edits, and save changes through project_write.

  *   Tool Description: ProposeGoal (**421** tks) - Describes proposing a
  verifiable multi-turn session goal, requiring user approval unless the user
  explicitly requested the exact outcome.

  *   Tool Description: PublishPlugin file listing and consent flow (**168**
  tks) - Explains that PublishPlugin lists the plugin's own files, shows the
  person the folder, organization, count, size and file names, sends nothing
  before their yes in any permission mode, and writes a manifest first for
  folders without one.

  *   Tool Description: PushNotification (**407** tks) - Explains when and how
  PushNotification should alert the user on the desktop and, when Remote Control
  is connected, on their phone.

  *   Tool Description: ReadFile compact (**285** tks) - Compact file-read tool
  description served to newer models — absolute path, default line cap, and
  image/PDF/notebook handling.

  *   Tool Description: ReadFile (**578** tks) - Tool description for reading
  files.

  *   Tool Description: ReadMcpResourceDirTool prompt (**177** tks) - Tool
  prompt for listing direct children of an MCP directory resource and explaining
  the required server and uri parameters.

  *   Tool Description: ReadNotifications (**626** tks) - Describes the
  ReadNotifications tool for draining queued GitHub, scheduled-trigger, and
  cross-session notifications in authoritative batches.

  *   Tool Description: RefreshMcpTools prompt (**167** tks) - Tool prompt for
  refreshing one or all connected MCP servers tool lists and interpreting
  per-server results.

  *   Tool Description: RefreshMcpTools (**318** tks) - Describes when and how
  to refresh connected MCP servers tool lists to recover missing or stale tools.

  *   Tool Description: Remote artifact watch guidance (**379** tks) - Explains
  durable remote artifact wake subscriptions, registration, comment wakes, and
  truthful watch-status reporting.

  *   Tool Description: RemoteTrigger prompt (**955** tks) - Tool prompt for
  calling the claude.ai RemoteTrigger API to list, get, create, update, or run
  scheduled remote agent routines.

  *   Tool Description: Report code-review findings (**163** tks) - Tool
  description for reporting verified code-review findings as a typed list for
  host UI rendering.

  *   Tool Description: ScheduleWakeup noop state guidance (**169** tks) -
  Defines when ScheduleWakeup should mark a tick as noop, how quiet noop streaks
  collapse in the terminal, and why stopping omits the field.

  *   Tool Description: ScheduleWakeup reason field guidance (**87** tks) -
  Explains that the ScheduleWakeup reason should be one specific sentence shown
  to the user and telemetry so the chosen cadence is understandable.

  *   Tool Description: ScheduleWakeup (**468** tks) - Base ScheduleWakeup
  description for /loop dynamic mode: avoid short polling of harness-tracked
  work in favor of a 1200s+ fallback, pass the /loop prompt or the
  autonomous-loop-dynamic sentinel back each turn, and end the loop with stop:
  true.

  *   Tool Description: SearchMcpRegistry (**377** tks) - Describes the
  SearchMcpRegistry tool for discovering MCP connectors by keyword, including
  named-product and intent-based examples and install-state guidance.

  *   Tool Description: SearchPlugins examples (**62** tks) - Provides
  keyword-query examples for SearchPlugins.

  *   Tool Description: SearchPlugins purpose (**68** tks) - Explains when to
  search the user's Claude.ai organization plugin catalog by keyword.

  *   Tool Description: SearchPlugins result handling (**117** tks) - Describes
  SearchPlugins results and when to render an install card, relay matches, or
  continue silently.

  *   Tool Description: SearchPlugins up-front guidance (**275** tks) - Guides
  proactive plugin searches for organization-specific processes, systems, or
  data that existing tools and project scripts do not cover.

  *   Tool Description: SearchSkills (**220** tks) - Describes the SearchSkills
  tool for finding relevant claude.ai skills by keyword and suggesting add cards
  when results fit.

  *   Tool Description: Self-hosted runner requeue session (**186** tks) -
  Describes requeuing an assigned self-hosted runner session away from a failing
  runner and surfacing the equivalent Admin UI path.

  *   Tool Description: SendFeedback drafting guidance (**1145** tks) -
  Instructs when and how to queue concise, factual Claude Code feedback drafts
  with prescribed evidence bullets, field-selection rules, privacy constraints,
  and no user interruption or duplication.

  *   Tool Description: SendFile (**451** tks) - Describes sending local files
  to peer, Remote Control, or cloud Claude Code sessions, including addressing,
  limits, integrity verification, and when to use shared-text messaging instead.

  *   Tool Description: SendMessage cross-session guidance (**1109** tks) -
  Explains cross-session SendMessage addressing, reply routing, liveness,
  disambiguation, literal message delivery without @ attachment expansion, and
  permission-laundering restrictions.

  *   Tool Description: SendMessage (**813** tks) - Describes the SendMessage
  tool for communicating with other agents and handling legacy team protocol
  responses.

  *   Tool Description: SendUserFile (**599** tks) - Describes the SendUserFile
  tool for surfacing generated deliverables as file cards, including when to
  send or re-send files and how to choose captions, status, and display mode.

  *   Tool Description: SendUserMessage (**226** tks) - Describes the
  SendUserMessage tool for sending user-visible Markdown messages and
  attachments with normal or proactive status.

  *   Tool Description: Session artifact watch lifecycle (**821** tks) -
  Explains live artifact watch setup, merging a newer published version before
  republishing, optional comment auto-replies, session restoration, status
  checks, and lifecycle limits.

  *   Tool Description: ShowOnboardingRolePicker (**38** tks) -
  ShowOnboardingRolePicker: presents a row of clickable role chips during Cowork
  onboarding.

  *   Tool Description: Skill proposal rendering (**320** tks) - Defines when
  and how to render complete recurring-procedure skill proposals for user review
  without writing files, and requires plugin or built-in skills to be proposed
  as separately named new skills rather than improvements.

  *   Tool Description: Skill-scoped git push refusal (**258** tks) - Explains
  that an active skill rejects dangerous or bypassing git push forms, permits a
  plain push to the configured remote, and forbids evasion or silent
  substitution.

  *   Tool Description: SubagentHandback (**215** tks) - Describes the
  SubagentHandback tool a subagent calls exactly once to deliver its full final
  report to the agent that spawned it, noting that the call ends the run and
  plain-text endings are not delivered.

  *   Tool Description: SuggestConnectors (**244** tks) - Describes the
  SuggestConnectors tool for resolving SearchMcpRegistry directoryUuid values
  into full connector payloads and install-state guidance.

  *   Tool Description: SuggestPluginInstall (**474** tks) - Describes the
  inline plugin install card: when to offer plugins the user could add to
  claude.ai, how to search first and map each result onto the card's fields, and
  the cases where no card should be rendered.

  *   Tool Description: SuggestSkills proactive guidance (**303** tks) - Guides
  proactive use of SuggestSkills to recommend addable standalone skills for
  repeatable tasks without interrupting one-off work.

  *   Tool Description: Task Get (**182** tks) - Retrieve a task by ID with full
  details and comments.

  *   Tool Description: TaskCreate (**738** tks) - Tool description for
  TaskCreate tool.

  *   Tool Description: TaskList (**267** tks) - Description for the TaskList
  tool, which lists all tasks in the task list.

  *   Tool Description: TaskUpdate (**586** tks) - Description for the
  TaskUpdate tool, which updates Claude's task list.

  *   Tool Description: TodoWrite compact (**108** tks) - Compact tool
  description for creating and updating a session task list with content,
  status, and activeForm fields.

  *   Tool Description: TodoWrite proactive update guidance (**65** tks) -
  Concise TodoWrite guidance to proactively track progress with one in-progress
  task and activeForm values.

  *   Tool Description: TodoWrite (**2037** tks) - Tool description for creating
  and managing task lists.

  *   Tool Description: Upload image (**241** tks) - Describes uploading fresh
  computer-tool screenshots to file inputs or drag-and-drop targets with bounded
  retries.

  *   Tool Description: WebFetch private URL warning (**217** tks) - Warns that
  WebFetch fails for authenticated or private URLs and includes the standard
  WebFetch usage notes.

  *   Tool Description: WebFetch (**478** tks) - Tool description for web fetch
  functionality.

  *   Tool Description: WebSearch (**537** tks) - Full WebSearch tool
  description: searching beyond the knowledge cutoff, US-only with domain
  filters, current-month year guidance, and either a mandatory Sources list or,
  when web-search citations are enabled, a note that result text is untrusted.

  *   Tool Description: Workflow (**1118** tks) - Describes the Workflow tool's
  background execution, explicit opt-in requirements, pure-literal metadata
  requirement, and canonical pipeline-based review pattern.

  *   Tool Description: Write (**185** tks) - Tool for writing files to the
  local filesystem.

  *   Tool Description: claude.ai Project (**1630** tks) - Reads and writes the
  claude.ai Project bound to the session — a shared, persistent knowledge
  container — via project info, document, search, and memory methods, including
  the claude/ namespace default, prompt-cache churn warnings, and
  untrusted-content handling.

  *   Tool Description: memory_list prompt (**193** tks) - Tool prompt for
  listing memory stores available in this session or store documents, paginating
  and narrowing listings, and directing content reads through memory_read.

  *   Tool Description: memory_write prompt (**426** tks) - Describes creating
  or replacing documents in memory stores available in this session with version
  checks, conflict handling, and secret-safety requirements.

  *   Tool Description: memory_write update triggers and timing (**611** tks) -
  Defines mandatory memory update triggers for user corrections, durable
  preferences, and non-transient environment discoveries, and requires writing
  before proceeding or finishing the turn.


  **Additional notes for some Tool Descriptions**


  *   Tool Description: Agent (usage notes) (**2022** tks) - Usage notes and
  instructions for the Task/Agent tool, including guidance on launching
  subagents, background execution, resumption, and worktree isolation.

  *   Tool Description: Agent (when to launch subagents) (**485** tks) -
  Describes _when_ to use the Agent tool - for launching specialized subagent
  subprocesses to autonomously handle complex multi-step tasks.

  *   Tool Description: Agent (when to launch subagents, lean rewrite) (**353**
  tks) - Describes available agent types, optional fork context inheritance,
  default agent selection, and plan-specific delegation restrictions.

  *   Tool Description: Agent delegation summary (lean rewrite) (**196** tks) -
  Recommends subagents for substantial parallel work and nonblocking side tasks
  while discouraging trivial or duplicated lookups.

  *   Tool Description: Agent delegation tradeoffs (lean rewrite) (**797** tks)
  - Explains the parallelism and focus benefits of delegation and the need to
  give fresh subagents sufficient context.

  *   Tool Description: Agent result visibility (lean rewrite) (**205** tks) -
  Warns against inventing pending subagent results and requires relaying useful
  final reports to the user.

  *   Tool Description: Artifact action reference (concise app wording) (**815**
  tks) - Concise app-worded reference for core Artifact publish, read, list,
  delete, open, pin, and unpin actions.

  *   Tool Description: Artifact assets guidance (app wording) (**667** tks) -
  App-worded guidance for uploading, listing, reading, copying, referencing, and
  deleting files in an Artifact asset store.

  *   Tool Description: Artifact comments guidance (app wording) (**416** tks) -
  App-worded guidance for reading, replying to, and resolving activated Artifact
  comment threads while treating viewer comments as untrusted data.

  *   Tool Description: Artifact design skill loading guidance (app wording)
  (**366** tks) - App-worded requirement to load the Artifact design skill
  before authoring, with workshop and diagramming exceptions and scratchpad
  placement guidance.

  *   Tool Description: Artifact live room guidance (app wording) (**293** tks)
  - Concise app-worded guidance for transient Artifact live rooms, untrusted
  viewer events, approved room_send replies, capability skill loading, and
  durable-storage alternatives.

  *   Tool Description: Artifact page authoring and HTML skeleton (app wording)
  (**703** tks) - App-worded requirement to load the Artifact design skill
  before authoring and to write page content for the viewer-supplied HTML
  skeleton.

  *   Tool Description: Artifact page implementation requirements (app wording)
  (**1890** tks) - App-worded Artifact page requirements for external resources,
  browser storage, size, responsive layout, theming, favicons, and icons.

  *   Tool Description: Artifact preview action (in-session viewer) (**198**
  tks) - Describes the Artifact preview action, which opens a page file in the
  real artifact viewer inside the session and returns a first-view picture,
  console errors and a clickable-elements outline, and when to use or skip it.

  *   Tool Description: Artifact publishing introduction (app wording) (**196**
  tks) - App-worded introduction to default-private HTML Artifacts, proactive
  publishing, and keeping potentially misleading or sensitive content local
  until the person chooses publication.

  *   Tool Description: Artifact quickstart type guidance (app wording) (**738**
  tks) - App-worded guidance to quickstart account-specific Artifact types and
  design systems before creating a new reader-facing page.

  *   Tool Description: Artifact runtime capabilities guidance (app wording)
  (**407** tks) - App-worded guidance for loading the Artifact capabilities
  skill, preferring durable capabilities to browser storage, and reconciling
  self-saved page versions.

  *   Tool Description: Artifact title and description guidance (app wording)
  (**236** tks) - App-worded guidance for concise distinctive Artifact titles,
  gallery descriptions, and stable naming across redeploys.

  *   Tool Description: Artifact type discovery guidance (app wording) (**734**
  tks) - App-worded guidance to discover and prefer account-specific Artifact
  types and design systems, with conditional creation and fill-opening
  instructions.

  *   Tool Description: AskUserQuestion (lean rewrite) (**217** tks) - Limits
  interactive questions to genuine user decisions and explains custom answers,
  multiple selection, and recommended options.

  *   Tool Description: AskUserQuestion (preview field) (**134** tks) -
  Instructions for using the HTML preview field on single-select question
  options to display visual artifacts like UI mockups, code snippets, and
  diagrams.

  *   Tool Description: AskUserQuestion (without plan mode note) (**147** tks) -
  Core AskUserQuestion guidance to ask only for decisions the user must make,
  with Other, multiSelect, and recommended-first option notes, used instead of
  the plan mode variant when a diskless host's tool set lacks a plan mode tool.

  *   Tool Description: Background monitor (streaming events) (**1963** tks) -
  Describes the background monitor tool that streams stdout events from
  long-running scripts as chat notifications, with guidelines on script quality,
  output volume, and selective filtering.

  *   Tool Description: Bash (Git commit instructions) (**1477** tks) - Git
  commit workflow for the Bash tool, covering the git safety protocol, parallel
  status, diff and log steps, commit message drafting, hook-failure handling,
  and the HEREDOC commit example.

  *   Tool Description: Bash (PR creation instructions) (**864** tks) - Pull
  request creation workflow for the Bash tool using gh, covering branch state
  analysis, the PR title and body template, and other common GitHub operations.

  *   Tool Description: Bash (alternative — communication) (**18** tks) - Bash
  tool alternative: output text directly instead of echo/printf.

  *   Tool Description: Bash (alternative — content search) (**27** tks) - Bash
  tool alternative: use Grep for content search instead of grep/rg.

  *   Tool Description: Bash (alternative — edit files) (**27** tks) - Bash tool
  alternative: use Edit for file editing instead of sed/awk.

  *   Tool Description: Bash (alternative — file search) (**26** tks) - Bash
  tool alternative: use Glob for file search instead of find/ls.

  *   Tool Description: Bash (alternative — read files) (**27** tks) - Bash tool
  alternative: use Read for file reading instead of cat/head/tail.

  *   Tool Description: Bash (alternative — write files) (**29** tks) - Bash
  tool alternative: use Write for file writing instead of echo/cat.

  *   Tool Description: Bash (attached machines) (**350** tks) - Bash tool
  section explaining how to route each command to a user-attached machine with
  the _host field, when to go straight to that machine instead of checking
  locally, and how to treat machines listed as offline.

  *   Tool Description: Bash (built-in tools note) (**53** tks) - Note that
  built-in tools provide better UX than Bash equivalents.

  *   Tool Description: Bash (git — avoid destructive ops) (**58** tks) - Bash
  tool git instruction: consider safer alternatives to destructive operations.

  *   Tool Description: Bash (git — never skip hooks) (**59** tks) - Bash tool
  git instruction: never skip hooks or bypass signing unless user requests it.

  *   Tool Description: Bash (git — prefer new commits) (**22** tks) - Bash tool
  git instruction: prefer new commits over amending.

  *   Tool Description: Bash (maintain cwd) (**81** tks) - Bash tool
  instruction: use absolute paths and avoid cd.

  *   Tool Description: Bash (overview) (**19** tks) - Opening line of the Bash
  tool description.

  *   Tool Description: Bash (prefer dedicated tools bullet) (**72** tks) -
  Bulleted warning to prefer dedicated tools over Bash for find, grep, cat, etc.

  *   Tool Description: Bash (prefer dedicated tools) (**71** tks) - Warning to
  prefer dedicated tools over Bash for find, grep, cat, etc.

  *   Tool Description: Bash (quote file paths) (**35** tks) - Bash tool
  instruction: quote file paths containing spaces.

  *   Tool Description: Bash (sandbox — default to sandbox) (**38** tks) -
  Default to sandbox; only bypass when user asks or evidence of sandbox
  restriction.

  *   Tool Description: Bash (sandbox — evidence list header) (**15** tks) -
  Header for list of sandbox-caused failure evidence.

  *   Tool Description: Bash (sandbox — evidence: access denied) (**15** tks) -
  Sandbox evidence: access denied to paths outside allowed directories.

  *   Tool Description: Bash (sandbox — evidence: network failures) (**17** tks)
  - Sandbox evidence: network connection failures to non-whitelisted hosts.

  *   Tool Description: Bash (sandbox — evidence: operation not permitted)
  (**18** tks) - Sandbox evidence: operation not permitted errors.

  *   Tool Description: Bash (sandbox — evidence: unix socket errors) (**11**
  tks) - Sandbox evidence: unix socket connection errors.

  *   Tool Description: Bash (sandbox — explain restriction) (**110** tks) -
  Explain which sandbox restriction caused the failure.

  *   Tool Description: Bash (sandbox — failure evidence condition) (**48** tks)
  - Condition: command failed with evidence of sandbox restrictions.

  *   Tool Description: Bash (sandbox — local port binding EPERM, no override)
  (**140** tks) - Explains that an EPERM failure binding or listening on a local
  port is caused by the sandbox, and directs the user to enable
  allowLocalBinding themselves, for sessions where unsandboxed retries are not
  available.

  *   Tool Description: Bash (sandbox — local port binding EPERM, override
  available) (**119** tks) - Explains that an EPERM failure binding or listening
  on a local port is caused by the sandbox, and that allowLocalBinding can be
  enabled without leaving the sandbox, for sessions where unsandboxed retries
  are already available.

  *   Tool Description: Bash (sandbox — no sensitive paths) (**36** tks) - Do
  not suggest adding sensitive paths to sandbox allowlist.

  *   Tool Description: Bash (sandbox — per-command) (**52** tks) - Treat each
  command individually; default to sandbox for future commands.

  *   Tool Description: Bash (sandbox — response header) (**17** tks) - Header
  for how to respond when seeing sandbox-caused failures.

  *   Tool Description: Bash (sandbox — retry without sandbox) (**33** tks) -
  Immediately retry with dangerouslyDisableSandbox on sandbox failure.

  *   Tool Description: Bash (sandbox — tmpdir) (**58** tks) - Use $TMPDIR for
  temporary files in sandbox mode.

  *   Tool Description: Bash (sleep — keep short) (**22** tks) - Bash tool
  instruction: keep sleep duration to 1-5 seconds.

  *   Tool Description: Bash (sleep — no polling background tasks) (**37** tks)
  - Bash tool instruction: do not poll background tasks, wait for notification.

  *   Tool Description: Bash (sleep — run immediately) (**21** tks) - Bash tool
  instruction: do not sleep between commands that can run immediately.

  *   Tool Description: Bash (sleep — use check commands) (**34** tks) - Bash
  tool instruction: use check commands rather than sleeping when polling.

  *   Tool Description: Bash (timeout) (**150** tks) - Bash tool instruction:
  optional timeout configuration.

  *   Tool Description: Bash (verify parent directory) (**38** tks) - Bash tool
  instruction: verify parent directory before creating files.

  *   Tool Description: Bash (working directory) (**37** tks) - Bash tool note
  about working directory persistence and shell state.

  *   Tool Description: CronCreate (durability note) (**122** tks) - CronCreate
  insert (shown when durable-cron is enabled) explaining the durable: true vs
  false trade-off.

  *   Tool Description: Enable Claude app browser (shared description) (**242**
  tks) - Feature-gated variant of the Claude app browser enable-tool
  description, used instead of the standard one when its rollout flag is on;
  also covers steps inside skills or user-requested tasks and notes that calling
  it adds the browser and on-computer file tools, possibly a turn later.

  *   Tool Description: Enable Claude in Chrome (shared description) (**269**
  tks) - Feature-gated variant of the Claude in Chrome enable-tool description,
  used instead of the standard one when its rollout flag is on; also covers
  steps inside skills or user-requested tasks and notes that calling it adds the
  Chrome and on-computer file tools, possibly a turn later.

  *   Tool Description: Enable computer use (shared description) (**268** tks) -
  Feature-gated variant of the computer-use enable-tool description, used
  instead of the standard one when its rollout flag is on; also covers steps
  inside skills or user-requested tasks and notes that calling it adds the
  computer-use and on-computer file tools, possibly a turn later.

  *   Tool Description: EnterPlanMode (ambiguous tasks) (**312** tks) - Tool for
  entering plan mode when task has ambiguity.

  *   Tool Description: Grep compact (lean rewrite) (**228** tks) - Describes
  compact ripgrep-backed search with explicit Claude Code permission UI and
  file-link integration for lean-rewrite models.

  *   Tool Description: Invoke skill (lean rewrite) (**509** tks) - Describes
  invoking packaged skills with expanded examples of covered tasks and guidance
  on background results, scoped names, and already-loaded skills.

  *   Tool Description: ListAgents (no SendMessage tool) (**224** tks) -
  Describes the ListAgents tool for sessions without a SendMessage tool: it
  lists visible agents and Claude sessions plus this session's own name, and
  explains that replies must go through the host application's messaging tool or
  be relayed to the user.

  *   Tool Description: Live Artifact watch guidance (app wording) (**456** tks)
  - App-worded guidance for live Artifact watches, merging a newer published
  version before republishing, optional comment auto-replies, watch status, and
  truthful subscription reporting.

  *   Tool Description: PowerShell (git guidance) (**192** tks) - PowerShell
  tool guidance to prefer new commits, consider safer alternatives to
  destructive git operations, and never bypass hooks or signing without an
  explicit user request.

  *   Tool Description: Prohibited artifact publishing (app wording) (**185**
  tks) - App-worded prohibition on publishing impersonation, fabricated records,
  deceptive collection flows, or content targeting private individuals.

  *   Tool Description: Publish audience-facing deliverables (app wording)
  (**658** tks) - App-worded guidance to publish finished audience-facing
  deliverables through Artifacts or first-party document connectors while
  keeping immediate personal advice in the terminal.

  *   Tool Description: ScheduleWakeup delay guidance (1-hour cache TTL)
  (**430** tks) - ScheduleWakeup delaySeconds guidance for sessions on a 1-hour
  prompt-cache TTL: no cache cliff to pace around, never wake just to keep the
  cache warm, and match the delay to external polling, fallback heartbeats, or
  1200–1800s idle ticks.

  *   Tool Description: ScheduleWakeup delay guidance (5-minute cache TTL)
  (**543** tks) - ScheduleWakeup delaySeconds guidance for sessions on the
  default 5-minute prompt-cache TTL: stay under 270s when actively polling,
  commit to 1200s+ otherwise, never pick 300s, and default idle ticks to
  1200–1800s.

  *   Tool Description: ScheduleWakeup delay guidance (unknown cache TTL)
  (**463** tks) - ScheduleWakeup delaySeconds guidance used when the session's
  prompt-cache TTL is not known, explaining the 1-hour versus 5-minute billing
  regimes, never waking just to keep the cache warm, 1200–1800s idle defaults,
  and 5-minute-TTL-only refinements.

  *   Tool Description: SendUserMessage (verbatim) (**102** tks) - Describes the
  concise SendUserMessage tool variant for sending verbatim user-visible
  messages with normal or proactive status.

  *   Tool Description: TaskList (teammate workflow) (**133** tks) - Conditional
  section appended to TaskList tool description.

  *   Tool Description: ToolSearch (input validation note) (**65** tks) -
  Explains the InputValidationError failure for unfetched deferred tools and
  requires selecting named deferred tools before calling them.

  *   Tool Description: ToolSearch (lean rewrite) (**47** tks) - Describes
  loading full schemas for deferred tools announced by name in system messages.

  *   Tool Description: ToolSearch (opening) (**39** tks) - Introduces
  ToolSearch as the schema loader for deferred tools announced by name in system
  reminders.

  *   Tool Description: ToolSearch (second part) (**178** tks) - Explains how
  queries match deferred tools and load their complete JSON Schema definitions
  for subsequent calls.

  *   Tool Description: ToolSearch (unfetched tool note) (**31** tks) - Explains
  that deferred tools expose no callable parameter schema until ToolSearch
  fetches them.

  *   Tool Description: Updating existing artifacts (app wording) (**469** tks)
  - App-worded guidance for same-path redeployment and URL-based
  read-before-publish updates, followed by surface-specific Artifact location
  and republish instructions.

  *   Tool Description: WebFetch (concise) (**277** tks) - Concise tool
  description for WebFetch covering URL fetching, private URL limitations,
  redirects, and caching.

  *   Tool Description: WebSearch (concise) (**180** tks) - Describes the
  concise WebSearch tool variant with US-only results, current-month guidance
  and domain filters, ending with either a required Sources list or, when
  web-search citations are enabled, a note that result text is untrusted.

  *   Tool Description: Workflow (lean rewrite) (**1113** tks) - Describes
  background workflow orchestration, explicit user opt-in confirmed by system
  messages, literal script metadata, and pipeline-based review.

  *   Tool Description: Write (read existing file first) (**121** tks) - Tool
  description for Write in environments where existing files must be read before
  overwrite.

  *   Tool Description: list_connected_browsers (browser picker guidance)
  (**234** tks) - Appended to the list_connected_browsers tool description: when
  to ask the user to pick among several connected browsers, how to label the
  options, and to call select_browser or switch_browser rather than choosing one
  unilaterally.

  *   Tool Parameter: Artifact URL guidance (app wording) (**284** tks) -
  App-worded Artifact url parameter: an existing artifact's claude.ai link to
  update in place or act on, with the requirement to read an artifact this
  conversation has neither read nor published before publishing to it and to
  merge into a refusal's live version.

  *   Tool Parameter: Artifact URL guidance (**333** tks) - Explains when the
  Artifact url parameter must name an existing owned artifact's claude.ai link,
  when to omit it for new publishes or same-conversation redeploys, and that
  publishing to an unread artifact requires reading it first and merging into
  any refused live version.

  *   Tool Parameter: Artifact auto-open timing (app wording) (**163** tks) -
  Describes the auto_open parameter on a type_url Artifact create in
  third-person app wording: when Claude passes after_first_write so the person
  does not first see an empty Artifact, and when it omits the parameter so the
  Artifact opens on creation.

  *   Tool Parameter: Artifact auto-open timing (**179** tks) - Describes the
  auto_open parameter on a type_url Artifact create: when to pass
  after_first_write so the user does not first see an empty Artifact, and when
  to omit it so the Artifact opens on creation.

  *   Tool Parameter: Artifact comment actions guidance (**688** tks) -
  Describes Artifact action parameter semantics for reading, replying to,
  resolving, and paginating comment threads.

  *   Tool Parameter: Artifact database access level (**181** tks) - Describes
  the read_db and write_db as_level parameter for checking artifact access rules
  as a view, interact, or admin user, which only narrows access, keeps the
  caller's identity, and makes refused writes read as not found and refused
  reads as empty.

  *   Tool Parameter: Artifact database batch writes (**347** tks) - Defines the
  Artifact writes parameter for batched database mutations, including
  per-document version pins and atomic or sequential commit behavior.

  *   Tool Parameter: Artifact database version precondition (**182** tks) -
  Defines the Artifact database if_version precondition, required on every write
  to an existing document and omitted only when creating one, with the write
  refused until the document has been read and batch entries pinned
  individually.

  *   Tool Parameter: Artifact force overwrite guidance (**170** tks) -
  Restricts Artifact force overwrites to explicit user-approved discarding of a
  specific newer version and requires merge-first conflict handling.

  *   Tool Parameter: Artifact force page overwrite warning (**55** tks) - Warns
  that forcing an Artifact publish discards the newer published page created by
  another session or an in-page save.

  *   Tool Parameter: Artifact force supporting files preservation (**58** tks)
  - Clarifies that a forced Artifact publish preserves supporting files unless
  the publish replaces or explicitly removes them.

  *   Tool Parameter: Artifact preview action (**211** tks) - Describes the
  Artifact check tool's preview action value, which renders one local HTML page
  file at chosen viewport widths and themes and returns height-capped
  screenshots plus a layout and load problem checklist without uploading
  anything.

  *   Tool Parameter: Artifact supporting files with cross-artifact sources (app
  wording) (**248** tks) - App-worded Artifact files parameter guidance for
  local sources, typed sources, server-side cross-artifact copies, update
  removal semantics, and source-version limits.

  *   Tool Parameter: Artifact supporting files with cross-artifact sources
  (**233** tks) - Describes Artifact supporting-file maps and lists, server-side
  copies from other Artifact versions, update preservation and removal, and
  local source restrictions.

  *   Tool Parameter: Artifact title (**267** tks) - Describes concise Artifact
  titles, HTML title-tag precedence, Markdown filename identity, and conditional
  naming behavior for creation from an Artifact type.

  *   Tool Parameter: Artifact unread path overwrite acknowledgement (**149**
  tks) - Defines overwrite_unread paths for an existing multi-file Artifact
  while preserving change-detection refusal for every other path.

  *   Tool Parameter: Artifact watch actions guidance (**552** tks) - Describes
  Artifact watch, unwatch, status, live-update subscriptions, comment
  notification behavior, and explicit resume_replies behavior.

  *   Tool Parameter: Bash command description (**311** tks) - Instructions for
  generating clear, concise command descriptions in active voice for bash
  commands.

  *   Tool Parameter: Bash run_in_background guidance (completion notices
  disabled) (**91** tks) - Bash run_in_background guidance for sessions where
  background completion notices are disabled: nothing announces the finish, so
  read the output file or end the command with an echo, and no trailing
  ampersand is needed.

  *   Tool Parameter: Bash run_in_background guidance (**154** tks) - Explains
  Bash run_in_background behavior and that commands do not need a trailing
  ampersand.

  *   Tool Parameter: Claude in Chrome JavaScript code (**103** tks) - Describes
  the JavaScript code parameter for the Claude in Chrome JavaScript execution
  tool.

  *   Tool Parameter: Computer action (**251** tks) - Action parameter options
  for the Chrome browser computer tool.

  *   Tool Parameter: PowerShell run_in_background note (completion notices
  disabled) (**71** tks) - PowerShell run_in_background note for sessions where
  background completion notices are disabled, telling the model nothing
  announces the finish so it must read the output file or end the command with
  its own echo.

  *   Tool Parameter: PowerShell run_in_background note (**107** tks) - Allows
  PowerShell background commands when results are not needed immediately and
  completion notifications are enabled, appending background timeout guidance
  when applicable.

  *   Tool Parameter: Sandboxed command allowed domains (**228** tks) - Declares
  additional network hosts for per-command auto-mode approval while rejecting
  host suggestions from untrusted content.

  *   Tool Parameter: SendUserMessage attachments (**75** tks) - Describes
  optional SendUserMessage attachments as local file paths or pre-resolved file
  objects.

  *   Tool Parameter: matched ask rule (**198** tks) - Describes metadata
  identifying a user-configured permissions.ask rule that forced a tool approval
  prompt while preserving the tool-authored decision reason.

  *   Tool Parameter: set_cwd needs_trust directory (**197** tks) - Describes
  the canonical target directory returned by a set_cwd needs_trust response,
  which the SDK host must show in a trust dialog and echo back verbatim on
  accept.


  ### Skills


  Built-in skill prompts for specialized tasks.


  *   Skill: /code-review efficiency dimension (**106** tks) - Code-review pass
  that surfaces wasted effort the diff adds — duplicate computation or I/O,
  avoidable serialization, large scopes held by closures — and points to the
  cheaper option.

  *   Skill: /doctor prompt-audit configuration scope (**532** tks) - Default
  arguments /doctor prompt-audit hands to the bundled claude-api skill, scoping
  the prompt audit to Claude Code configuration files that load in this project,
  excluding settings files and secrets, limiting edits outside the project, and
  treating audited files as data.

  *   Skill: /doctor slash command description (**372** tks) - Trigger
  description for the /doctor slash command covering setup health, unused
  extensions, memory cleanup, hooks, updates, and permission prompts.

  *   Skill: /doctor slash command (**16182** tks) - Diagnostic workflow for
  auditing and fixing Claude Code installation health, unused context, local
  memory duplication, hooks, version currency, and permission prompts.

  *   Skill: /explain-usage measured usage slash command (**998** tks) - Hands
  Claude the conversation's measured token usage as JSON with group definitions
  and caveats, asking for one simple chart of effective usage by group and a
  plain-language explanation.

  *   Skill: /explain-usage slash command (**449** tks) - Analyzes the current
  session transcript into cost-weighted token usage groups, charts the results,
  and explains them in plain language.

  *   Skill: /init CLAUDE.md and skill setup (new version) (**7828** tks) - A
  comprehensive onboarding flow for setting up CLAUDE.md and related
  skills/hooks in the current repository, including codebase exploration, user
  interviews, and iterative proposal refinement.

  *   Skill: /insights report output (**293** tks) - Formats and displays the
  insights usage report results after the user runs the /insights slash command.

  *   Skill: /loop cloud-first scheduling offer (**730** tks) - Decision tree
  for offering cloud-based scheduling before falling back to local session loops
  in the /loop command.

  *   Skill: /loop local runtime note (**96** tks) - Conditional /loop
  confirmation note explaining that local loops run only until the current
  session closes.

  *   Skill: /loop self-pacing mode (re-arm decision) (**412** tks) - Re-arm
  decision step for self-paced /loop iterations: call ScheduleWakeup with a
  fallback-heartbeat or cadence delay, a short reason, the original /loop input
  as the prompt, and a noop flag, or stop the loop instead.

  *   Skill: /loop self-pacing mode (**738** tks) - Instructs Claude how to
  self-pace a recurring loop by arming event monitors as primary wake signals
  and scheduling fallback heartbeat delays between iterations.

  *   Skill: /loop slash command (dynamic mode) (**977** tks) - Parses user
  input into an interval and prompt for scheduling recurring or dynamically
  self-paced loop executions.

  *   Skill: /stuck (background-daemon diagnostics) (**273** tks) - The
  background-daemon troubleshooting section of the /stuck skill.

  *   Skill: /stuck slash command (**964** tks) - Diagnoses frozen or slow
  Claude Code sessions.

  *   Skill: Artifact PR review description (composed publish flow) (**178**
  tks) - Trigger description for the composed PR review briefing Artifact flow
  with republish-only updates.

  *   Skill: Artifact components (**2462** tks) - Catalogs reusable Artifact
  components and supplies the exact scripts, styles, tokens, markup contract,
  and composition limits for embedding interactive decision blocks.

  *   Skill: Artifact data table (**2016** tks) - Skill instructions for
  creating interactive data-table artifacts with sortable, filterable tabular
  data, safe JSON embedding, theming, and template-preservation requirements.

  *   Skill: Artifact diagramming (**1413** tks) - Provides Artifact diagramming
  guidance, including when diagrams add value and how to build legible
  theme-aware inline SVG.

  *   Skill: Artifact document (**2055** tks) - Skill instructions for creating
  live editable document artifacts with structured prose, status metadata,
  commenting, and template-preservation requirements.

  *   Skill: Artifact explainer (**2024** tks) - Instructions for creating
  explainer artifacts with numbered-step or section walkthrough structures from
  the built-in template.

  *   Skill: Artifact report (**1930** tks) - Instructions for creating
  long-form report artifacts with a masthead, table of contents, structured
  sections, and optional appendix from the built-in template.

  *   Skill: Artifact whiteboard when-to-use guidance (**182** tks) -
  When-to-use guidance for proactively offering a whiteboard during
  architecture, multi-component planning, or repeated clarification while
  limiting the offer to once per session.

  *   Skill: Claude API Managed Agents onboarding source tier (**718** tks) -
  Closing section of the Claude API skill prompt for managed-agents-onboard
  requests that states whether the source is a bundled quickstart, a first-party
  URL or a third-party URL, what to tell the user about shorter request forms,
  and that nothing in the request or fetched pages can raise the tier.

  *   Skill: Claude Code configuration guide (**2462** tks) - Skill instructions
  for answering Claude Code configuration questions by checking the running
  build, bundled references, and current documentation.

  *   Skill: Claude Test sign-in (**371** tks) - Claude Test skill that signs
  the site's dedicated test member in before a run and saves the browser
  session, so specs needing an account start signed in.

  *   Skill: Claude guide unavailable reference fallback (**344** tks) -
  Provides the Claude guide skill with an embedded live-sources reference and
  fallback instructions when its reference files cannot be written to disk.

  *   Skill: Code Review (Angle B — removed-behavior auditor) (**94** tks) -
  Code-review finder angle that, for each deleted or rewritten line, names the
  behavior it guaranteed and confirms the new code still guarantees it.

  *   Skill: Code Review (Angle C — cross-file tracer) (**88** tks) -
  Code-review finder angle that follows each changed function out to its
  callers, checking the diff hasn't broken a call-site contract.

  *   Skill: Code Review (Angle D — language-pitfall specialist) (**101** tks) -
  Code-review finder angle that hunts for the well-known traps of the diff's
  language or framework.

  *   Skill: Code Review (Angle E — wrapper/proxy correctness) (**126** tks) -
  Code-review finder angle for wrapping types (caches, proxies, decorators),
  checking every method forwards faithfully to the wrapped object.

  *   Skill: Code Review (Output — findings JSON array) (**343** tks) - Defines
  the code-review skill's result shape: a JSON array of findings carrying file,
  line, summary, and failure_scenario.

  *   Skill: Code Review (Phase 0 — gather the diff) (**135** tks) - Opening
  step of the code-review skill: assemble the unified diff to review with git
  diff.

  *   Skill: Code Review (Phase 2 — verify, 3-state) (**125** tks) -
  Precision-tier verification step: run one verifier per candidate finding, each
  voting CONFIRMED, PLAUSIBLE, or REFUTED.

  *   Skill: Code Review (Phase 2 — verify, recall-biased) (**137** tks) -
  Recall-tier verification step: one verifier per candidate finding, biased
  toward keeping anything plausible.

  *   Skill: Code Review (Phase 3 — sweep for gaps) (**131** tks) - Final
  code-review sweep: a clean-slate reviewer re-reads the diff to catch defects
  the earlier passes missed.

  *   Skill: Code Review (conventions dimension) (**0** tks) - Code-review
  dimension: flag diff lines that break a rule stated in an applicable CLAUDE.md
  (user, repo-root, or ancestor-directory), quoting the exact rule and offending
  line, and emit nothing when no CLAUDE.md governs the change.

  *   Skill: Code Review correctness finder angles (**414** tks) - Shared
  /code-review finder-angle block covering line-by-line diff scanning,
  removed-behavior auditing, and cross-file tracing.

  *   Skill: Code Review inline medium/high template (**652** tks) - Template
  for medium and high inline code-review prompts that run eight finder angles,
  deduplicate without verification, and enforce a minimum findings target.

  *   Skill: Code Review inline xhigh mode (**1234** tks) - Extra-high inline
  /code-review prompt that runs ten finder angles, deduplicates without
  verification, sweeps for gaps, and returns up to fifteen findings.

  *   Skill: Code Review low effort expanded-findings mode (**810** tks) -
  Low-effort /code-review prompt that reads the diff once, returns up to eight
  hunk-visible findings, and targets at least min(files_changed, 4) genuine
  findings.

  *   Skill: Computer Use MCP (**1731** tks) - Instructions for using
  computer-use MCP tools including tool selection tiers, app access tiers, link
  safety, and financial action restrictions.

  *   Skill: Data Visualization description (**496** tks) - Trigger description
  for the Data Visualization skill covering charts, graphs, dashboards,
  palettes, stat tiles, and related visualization requests.

  *   Skill: Data Visualization (**2901** tks) - Skill instructions for
  producing accessible, brand-neutral charts, graphs, dashboards, and data
  visualizations using a validated method.

  *   Skill: Debugging (**653** tks) - Instructions for debugging an issue that
  the user is encountering in the Claude Code session.

  *   Skill: Dynamic pacing loop execution (re-arm decision) (**411** tks) -
  Re-arm decision step for dynamic-pacing /loop ticks: call ScheduleWakeup with
  a fallback-heartbeat or observed-cadence delay, a short reason, the
  dynamic-mode sentinel prompt, and a noop flag, or stop the loop instead.

  *   Skill: Dynamic pacing loop execution (**664** tks) - Step-by-step
  instructions for executing a dynamic pacing loop that runs tasks, arms and
  re-arms event monitors, schedules fallback heartbeat ticks, and handles task
  notifications.

  *   Skill: Generate permission allowlist from transcripts (**3419** tks) -
  Analyzes session transcripts to extract frequently used read-only tool-call
  patterns and adds them to the project's .claude/settings.json permission
  allowlist to reduce permission prompts.

  *   Skill: Import to Claude Code (**418** tks) - Generated SKILL.md
  instructing Claude to finish importing leftover foreign-agent config that
  `claude import` could not map automatically.

  *   Skill: Mod GitHub sharing consent (**315** tks) - Offers sharing a working
  mod once and requires separate visibility consent and file inspection before
  creating a GitHub repository.

  *   Skill: Multiplayer whiteboard description (**211** tks) - Trigger
  description for the multiplayer Whiteboard skill, covering creation of a live
  shared canvas with synchronized strokes, cursors, session presence, pasted
  images, and Claude drawing.

  *   Skill: Plugin eval authoring interview (**5128** tks) - Guided interview
  for creating Claude plugin eval suites under evals/ with gated inputs,
  graders, calibration, and cost checks.

  *   Skill: Prototype description (**151** tks) - Trigger description for the
  Prototype skill, covering working proof-of-concept Artifacts and optional
  in-place feature demonstrations.

  *   Skill: Prototype runtime capabilities guidance (**242** tks) - Explains
  when prototype Artifacts should use user-granted runtime capabilities and
  requires loading the artifact-capabilities skill before relying on live data
  or actions.

  *   Skill: Run CLI tool example (**648** tks) - Example file for the Run app
  skill showing how to document building, invoking, and testing a CLI tool.

  *   Skill: Run TUI interactive terminal app example (**1292** tks) - Example
  file for the Run app skill showing how to drive an interactive terminal app
  with tmux, readiness polling, pane capture, key references, and cleanup.

  *   Skill: Run app (**1315** tks) - Skill for launching and driving the
  current project's app through its real runtime surface using project-specific
  run skills or fallback patterns.

  *   Skill: Run browser-driven web app example (**1413** tks) - Example file
  for the Run app skill showing how to start a web dev server, drive it with
  chromium-cli, capture screenshots, and document app-specific gotchas.

  *   Skill: Run library SDK example (**835** tks) - Example file for the Run
  app skill showing how to document building, testing, and smoke-checking a
  library or SDK at its public package boundary.

  *   Skill: Run skill template (**1596** tks) - Template file for the Run skill
  generator showing the frontmatter and section structure for a project-specific
  run skill.

  *   Skill: Run web server API example (**1364** tks) - Example file for the
  Run app skill showing how to document a server or API lifecycle with
  background launch, readiness checks, curl verification, and shutdown.

  *   Skill: Schedule recurring cron and run immediately (**271** tks) -
  Converts an interval to a cron expression, schedules a recurring task via the
  cron creation tool, confirms to the user, and immediately executes the task
  without waiting for the first cron fire.

  *   Skill: Setup Cowork role selection (**475** tks) - First step of the
  setup-cowork skill that explains Cowork, asks for the user's role, calls
  ShowOnboardingRolePicker, or falls back to a plain-text role list.

  *   Skill: Setup Cowork (**3464** tks) - Guided Cowork setup flow that helps
  the user pick a role, install matching plugins, connect tools, try a skill,
  configure writing voice, and wrap up.

  *   Skill: Team onboarding guide (**521** tks) - Template for onboarding a new
  teammate to a team's Claude Code setup, walking them through usage stats,
  setup checklists, MCP servers, skills, and team tips in a warm conversational
  style.

  *   Skill: Update Claude Code Config (**1726** tks) - Skill for modifying
  Claude Code configuration file (settings.json).

  *   Skill: Update config description (**170** tks) - Update-config skill
  description (settings.json hooks, perms, env).

  *   Skill: Update config settings file locations (**1445** tks) - Where Claude
  Code stores settings.json across scopes.

  *   Skill: Verify CLI changes (example for Verify skill) (**772** tks) -
  Example workflow for verifying a CLI change, as part of the Verify skill.

  *   Skill: Verify server/API changes (example for Verify skill) (**793** tks)
  - Example workflow for verifying a server/API change, as part of the Verify
  skill.

  *   Skill: Whiteboard description (**228** tks) - Trigger description for the
  Whiteboard skill, covering creation of a shared sketch canvas for
  collaborative drawing, design discussion, and planning.

  *   Skill: Workflow authoring reference (**6322** tks) - Skill reference for
  authoring Workflow scripts, covering script persistence, metadata, agent
  hooks, isolation, concurrency, budgeting, quality patterns, and resume
  behavior.

  *   Skill: update-config (7-step verification flow) (**1755** tks) - A skill
  that guides Claude through a 7-step process to construct and verify hooks for
  Claude Code, ensuring they work correctly in the user's specific project
  environment.
