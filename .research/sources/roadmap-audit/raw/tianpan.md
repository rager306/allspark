url: https://tianpan.co/blog/2026/05/05/prompt-credit-assignment-system-prompt-dead-weight
title: "Prompt Credit Assignment: Finding the Dead Weight in Your System Prompt"
content: >-
  Most teams discover their system prompt has a weight problem the same way — a
  cost review, a latency spike, or an engineer who finally reads the thing end
  to end. What they find is typically a 2,000-token document that grew
  organically over six months, with three versions of "be concise" scattered
  across different sections, instructions that reference a product workflow that
  was deprecated in February, and a dozen rules that the model visibly ignores
  on every run. The prompt is large. Most of it isn't doing anything.


  This is the prompt credit assignment problem: figuring out which instructions
  in a multi-thousand-token system prompt actually drive model behavior, and
  which are just dead weight that burns tokens and dilutes attention. The bad
  news is that most teams skip this entirely — they add instructions when
  behavior breaks and never subtract. The good news is there is a repeatable
  engineering discipline for it.


  ## Why Credit Assignment Is Hard for Prompts​


  In traditional machine learning, credit assignment is the problem of
  determining which input features caused a particular output. For a decision
  tree, it is trivial. For a neural network, it requires gradient analysis. For
  a system prompt, most teams treat it as a black box: they add an instruction,
  run a few examples, and decide whether it helped based on feel.


  The root problem is that LLM behavior is jointly determined by everything in
  context simultaneously. A 500-token system prompt is not evaluated instruction
  by instruction. The model's attention mechanism weighs all tokens against each
  other across every layer. Instruction A might be redundant because instruction
  B already implies it. Instruction C might be actively harmful because it
  conflicts with a pattern the model learned during fine-tuning. Instruction D
  might matter enormously in one context and be completely ignored in another.


  This means the "add an instruction when something breaks" pattern is
  particularly unreliable. You cannot know whether the fix worked because of the
  new instruction, because of a correlated change in how you phrased another
  instruction, or because the eval set you used to check it was too narrow to
  expose the failure.


  The discipline of prompt credit assignment forces you to answer this
  empirically instead of by intuition.


  ## Ablation Testing as the Empirical Foundation​


  The most reliable tool for prompt credit assignment is ablation testing:
  systematically removing or replacing prompt components and measuring the
  effect on outputs.


  The setup is straightforward. You take your system prompt and break it into
  semantically distinct components — the persona definition, the output format
  rules, the behavioral constraints, the domain knowledge, the safety
  guardrails, the few-shot examples. Then you run a fixed evaluation set against
  a series of prompt variants where each component is individually removed or
  replaced with a neutral placeholder.


  The key insight ablation testing surfaces is that most prompts contain three
  classes of instructions:


  *   **High-signal instructions**: removing these degrades measurable outputs.
  The model's task completion rate drops, format compliance breaks, or safety
  evals fail. These are load-bearing.

  *   **Low-signal instructions**: removing these has no measurable effect
  across your eval set. The model already does the behavior natively, or the
  instruction is too vague to be actionable.

  *   **Negative-signal instructions**: removing these _improves_ outputs. These
  are actively confusing the model — often because they conflict with another
  instruction, contradict the model's training priors, or introduce ambiguity
  where none existed.


  In practice, research on prompt ablation in code generation tasks found that
  removing cross-file context from prompts degraded branch coverage by 8.5% for
  some models — while having almost no effect for others. The point is not that
  cross-file context is always valuable; it is that you cannot know until you
  test. Assumptions about what matters in your prompt are routinely wrong.


  A rigorous ablation harness runs each variant against at least 50–100
  representative examples and scores against a defined rubric. It is not manual
  — you build it once and run it every time the prompt changes, treating it like
  a prompt regression suite.


  ## Attribution Estimation: Going Below Component Level​


  Ablation testing identifies which high-level sections matter. Attribution
  estimation goes deeper — it tells you which specific tokens or sentences
  within those sections are actually driving model behavior.


  The principle: if a small perturbation to a token's representation in the
  input causes a large change in the output distribution, that token has high
  attribution. Several techniques operationalize this:


  **Perturbation-based attribution** replaces individual tokens or spans with
  neutral alternatives (e.g., mask tokens or synonyms) and measures output
  change. This is model-agnostic and works through the standard API. You can run
  this on your system prompt today without any special infrastructure.


  **Gradient-based attribution** uses the model's internal gradients to compute
  an importance score for each input token. Tools like Integrated Gradients (IG)
  are well-established for this purpose. The tradeoff is that these methods
  require model internals — they do not work for black-box APIs. If you run an
  open-weights model in-house, gradient-based attribution is significantly more
  precise than perturbation.


  **ProCut** is an open-source framework that applies attribution estimation
  directly to prompt compression — it identifies which tokens in a prompt are
  least influential and removes them while preserving the tokens that actually
  drive behavior. Benchmarks show 40–60% token reduction with minimal
  performance degradation on downstream tasks.


  For most teams, perturbation-based approaches are the practical starting
  point. The workflow: take a long, potentially redundant section of your system
  prompt, run attribution analysis across 20–30 representative examples, and
  identify the spans that consistently score near-zero attribution. Those are
  candidates for removal.


  The uncomfortable finding teams often encounter: the instructions they wrote
  most carefully — the elaborate behavioral constraints, the nuanced persona
  guidance — frequently score lower attribution than the blunt format directives
  they added as afterthoughts.


  ## The Redundancy Problem Is Worse Than You Think​


  Beyond low-attribution instructions, a separate category of dead weight comes
  from redundancy. Most system prompts that have been edited over time contain
  the same behavioral directive stated multiple times in different sections.


  For members


  Members get the rest — the frameworks, decisions, and reasoning behind every
  idea I publish in public.


  *   Complete essays, including the parts I keep off the public archive

  *   Working frameworks with the trade-offs spelled out

  *   Early access to new pieces before they go public


  Cancel anytime · One subscription, every essay
