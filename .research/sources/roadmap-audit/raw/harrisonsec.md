url: https://harrisonsec.com/blog/system-prompt-token-floor/
title: 550 Tokens of Prompt, 588 Tokens of Tools
content: >-
  *   Home

  *   Blog

  *   550 Tokens of Prompt, 588 Tokens of Tools


  ## Pi is famous for a system prompt under a thousand tokens. I measured it:
  550. Then I measured the four tool schemas that ship beside it in every single
  request: 588. The celebrated number is the smaller half. Meanwhile Codex ships
  a different prompt length for every model, from 1,436 to 5,070 tokens, and the
  reason why is the most useful thing in either codebase.


  September 11, 2026


  Harrison Guo


  8 min read


  AI Engineering Measurement


  ### Table of Contents


  *   The measurement

  *   Half the prompt is a map to itself

  *   The bigger half nobody counts

  *   Codex ships a different prompt per model

  *   Why none of this saves you money

  *   The real argument for a small surface

  *   What to measure in your own harness


  Pi’s reputation rests on one number: a system prompt small enough to quote.


  I measured it. The default is 550 tokens under `o200k_base`, 2,547 characters.
  The claim is true, and by the standards of this field, 550 is remarkably
  small.


  Then I measured the four tool schemas that go out in the same request, every
  request, forever: 588 tokens.


  The famous number is the smaller half.


  > **tl;dr** — Pi’s system prompt really is small, but its tool schemas are
  bigger. Measured with tiktoken, the default prompt is 550 tokens and the four
  tool schemas sent with every request are 588, a per-request floor of 1,138.
  Codex’s prompt files run from 1,436 tokens for Codex-tuned models to 5,070 for
  general GPT-5.x models, and prompt caching makes either floor cheap.


  ## The measurement


  Pi builds its system prompt in
  `packages/coding-agent/src/core/system-prompt.ts`, in a function called
  `buildSystemPrompt`. It is not a static file. It assembles a template from the
  selected tools, one-line snippets each tool contributes, guidelines each tool
  contributes, and paths to Pi’s own documentation.


  I reconstructed the default path exactly: no custom prompt, the default tool
  set of read, bash, edit and write, with each tool’s real snippet and
  guidelines pulled from `packages/coding-agent/src/core/tools/*.ts`. Then I
  counted with `tiktoken`.


  | segment | tokens |

  | --- | --- |

  | system prompt, total | 550 |

  | … behavioural core | 282 |

  | … Pi documentation navigation block | 268 |

  | tool schemas, 4 tools as JSON | 588 |

  | **per-request floor** | **1,138** |


  segment system prompt, total


  tokens 550


  segment… behavioural core


  tokens 282


  segment… Pi documentation navigation block


  tokens 268


  segment tool schemas, 4 tools as JSON


  tokens 588


  segment**per-request floor**


  tokens**1,138**


  The per-tool split is `read` 153, `bash` 110, `edit` 238, `write` 85. The
  `edit` tool is 43% larger than the entire behavioural core of the system
  prompt, because exact-match editing needs careful instructions about
  non-overlapping regions and Pi puts them where they belong, in the schema.


  Two things fall out of this table and they point in different directions.


  ## Half the prompt is a map to itself


  268 of the 550 tokens are a block that lists where Pi’s own documentation
  lives: the README path, the docs path, the examples path, and a line
  enumerating which file to read for extensions, themes, skills, prompt
  templates, TUI components, keybindings, SDK integrations, custom providers,
  models, packages and environment variables.


  The block opens with a condition: read this only when the user asks about Pi
  itself.


  So on the overwhelming majority of turns, where somebody is asking Pi to fix
  their own code, 49% of the celebrated system prompt is navigation for a case
  that is not happening.


  I want to be careful about what this is and is not. It is not waste in the
  sloppy sense. It is a deliberate trade, and it is the same trade Claude Code
  makes with `CLAUDE.md`: pay a fixed context cost so the agent can find things
  without a round trip. Pi is buying self-service. An agent that knows where its
  own extension docs are can write its own extension, which is the entire
  product thesis.


  But it does mean the number people quote is not measuring what they think. The
  behavioural core of Pi, the part that says what kind of agent this is and how
  to act, is 282 tokens. That is the genuinely astonishing number, and it is not
  the one in circulation.


  ## The bigger half nobody counts


  Tool schemas are not part of the system prompt in the API’s data model. They
  are a separate field. They are also serialised into the same request, priced
  the same way, and consume the same context.


  For Pi that is 588 tokens for four tools. Four is the smallest tool set of any
  harness in serious use. If four tools cost 588 tokens, a harness with fifteen
  tools is carrying a couple of thousand tokens of schema before anyone types
  anything, and none of it appears in the number anyone advertises.


  This is a measurement boundary problem, and it is the same shape as a wrong
  ruler. “System prompt tokens” is a real, precise, reproducible quantity. It is
  just not the quantity that matters, because the thing that matters is what
  goes out on every request, and the API’s field boundary is not the same as the
  cost boundary.


  If you want one number for a harness, it is the per-request floor: system
  prompt plus tool schemas plus any always-on context. For Pi that is 1,138
  tokens.


  ## Codex ships a different prompt per model


  Now the other half of the comparison, and this is the finding I did not
  expect.


  Codex keeps its system prompts as markdown files checked into
  `codex-rs/core/`. Not generated, not hidden behind an API. Files you can read.
  Here they are, measured the same way:


  | file | tokens |

  | --- | --- |

  | `gpt_5_1_prompt.md` | 5,070 |

  | `gpt_5_2_prompt.md` | 4,564 |

  | `gpt-5.1-codex-max_prompt.md` | 1,624 |

  | `gpt-5.2-codex_prompt.md` | 1,624 |

  | `gpt_5_codex_prompt.md` | 1,436 |


  file`gpt_5_1_prompt.md`


  tokens 5,070


  file`gpt_5_2_prompt.md`


  tokens 4,564


  file`gpt-5.1-codex-max_prompt.md`


  tokens 1,624


  file`gpt-5.2-codex_prompt.md`


  tokens 1,624


  file`gpt_5_codex_prompt.md`


  tokens 1,436


  Look at the split. The Codex-tuned models get 1,436 to 1,624 tokens. The
  general-purpose GPT-5.x models get 4,564 to 5,070 for the same product doing
  the same job.


  The gap runs from 2,940 tokens to 3,634 depending on which pair you line up,
  so call it three thousand and change. That is not a style difference. That is
  the same behaviour, specified twice, in two different places. For the tuned
  models it is in the weights. For the general models it has to be said out loud
  on every request.


  This is the cleanest illustration I have seen that a system prompt is not a
  virtue metric. It is a statement about a split. Everything the model does not
  already know, you pay for in tokens, forever, on every call.


  Which means “our system prompt is small” is only a meaningful boast if you
  also say which model it is small for. Pi’s 550 tokens run against Claude, GPT
  and Gemini models that were never tuned for Pi. Codex’s 1,436 tokens run
  against a model that was.


  ## Why none of this saves you money


  Here is where I have to argue against the obvious conclusion, including one I
  have made myself.


  The per-request floor is static. It is the same bytes at the front of every
  request in a session. Static prefixes are exactly what prompt caching is for,
  and every major provider now caches them at a large discount after the first
  call.


  So the floor is the cheapest part of a session, per token, by a wide margin.


  What is not cached, and what grows without bound, is the conversation: the
  file contents, the command output, the tool results, the model’s own messages.
  That is the part that ends up dominating a long session, and none of it is
  affected by whether your system prompt was 550 tokens or 5,070.


  I made this argument in Agent Retrieval Is a Cost Curve Problem about grep
  versus RAG, and in Your AI Bill Is a Distributed Systems Problem about where
  agent spend actually goes. The same shape applies here. Total cost over a
  session is dominated by repeated and growing content, not by the fixed header.


  Optimising a cached constant is the most satisfying kind of optimisation and
  one of the least useful.


  ## The real argument for a small surface


  Which does not mean Pi’s design is wrong. It means the usual defence of it is.


  Four tools is better than fifteen for a reason that has nothing to do with
  tokens. Every tool in the schema is a choice the model evaluates on every
  turn, and most wrong actions are not the model failing at a task, they are the
  model picking a plausible tool that was not the right one. Shrinking the
  option set shrinks the space of confident mistakes.


  That is shrinking the stochastic surface, and it is a quality argument, not a
  cost argument. It survives contact with prompt caching. The cost argument does
  not.


  The same logic explains why Pi is comfortable spending 238 tokens on the
  `edit` schema alone. When you only have four tools, each one has to be exactly
  right, and precision in a schema is cheap compared to a wrong edit.


  ## What to measure in your own harness


  Three numbers, and most teams have none of them.


  Your per-request floor: system prompt plus every tool schema plus any
  always-on context, in tokens, from the actual serialised request rather than
  from the source. Mine came out 588 tokens higher than the number I would have
  quoted from the prompt file.


  Your cache hit rate on that floor. If it is not near one after the first turn,
  the floor is not a floor, it is a tax, and that is a real bug worth fixing.


  Your tool count against your error modes. If the model reaches for the wrong
  tool, the fix is fewer tools, not a longer prompt explaining when to use each
  one. Pi’s four are read, bash, edit and write, and three independent teams
  converged on approximately that set. That convergence is worth more attention
  than the token count.


  * * *


  _All token counts measured 2026-09-01 with `tiktoken``o200k\_base` against
  `github.com/earendil-works/pi` and `github.com/openai/codex` at that day’s
  HEAD. The Pi system prompt was reconstructed from `buildSystemPrompt` on its
  default path rather than captured from a live request, so a real session with
  project context files or skills loaded will be larger._


  I occasionally advise small teams on backend reliability, Go performance, and
  production AI systems. Learn more: /services


  ### [ Agent_Architecture_Notes ]


  How Claude Code-style agents actually work. Retrieval, memory, orchestration.
  New post when there's something worth saying. Unsubscribe anytime.
