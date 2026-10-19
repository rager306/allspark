# Prompt caching: official provider rules vs. our P0 implementation

Audit date: **2026-10-18**. Every number below is copied from a provider's own docs
(URLs in Part 5). Where a rule is not stated by the provider I say so instead of
guessing. Anything marked `[SECONDARY]` comes from an error string observed in the
wild, not from the docs.

Repo under audit: `/root/prime-agent` @ `87dad4c63` + working-tree P0 changes
(`crates/pa-ai/src/providers/anthropic/params.rs`, `convert.rs`).

---

## Part 1. Provider table

| provider | min. tokens for a cacheable prefix | step / granularity | TTL | write price | read price | max breakpoints | source |
|---|---|---|---|---|---|---|---|
| **Anthropic** (Claude API) | **512** Fable 5.1, Mythos 5.1, Opus 5.5, Opus 5, Sonnet 5.5, Fable 5, Mythos 5, Haiku 5.5 · **1024** Opus 4.8, Sonnet 5, Sonnet 4.6, Sonnet 4.5, Opus 4.1, Opus 4, Sonnet 4 · **2048** Mythos Preview, Opus 4.7, Haiku 3.5 · **4096** Opus 4.6, Opus 4.5, Haiku 4.5 | no published step; breakpoints are per content block | 5 min default, 1 h optional | 1.25× input (5 m) / 2× input (1 h) | 0.1× input (0.05× Opus 5.5 & Sonnet 5.5, 0.025× Fable 5.1 & Mythos 5.1) | **4** | docs.anthropic.com/en/docs/build-with-claude/prompt-caching |
| **Anthropic on Bedrock** | **512** Haiku 5.5, Sonnet 5.5, Opus 5.5, Fable 5.1, Mythos 5.1, Fable 5, Mythos 5, Opus 5 · **1024** Opus 4.8, Sonnet 5, Sonnet 4.6, Sonnet 4.5, 3.7 Sonnet, 3.5 Sonnet v2 · **4096** Opus 4.7, Opus 4.6, Opus 4.5, Haiku 4.5 | same | 5 min; 1 h only on the models whose card lists it | same as Anthropic | same | **4** per request | docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html |
| **OpenAI** — GPT‑5.6 and later | **1,024 visible input tokens** (hidden system tokens don't count) | exact boundary, **no 128 rounding** | `prompt_cache_options.ttl: "30m"` (default, min lifetime 30 min after last write/reuse) | **1.25×** input | **0.1×** input (0.05× GPT‑6.1 Sol) | **4 cache writes** per request | platform.openai.com/docs/guides/prompt-caching |
| **OpenAI** — GPT‑5.5 and earlier | 1,024 (varies by request settings) | implicit breakpoints every **2,048 tokens** for 5.5; model-dependent otherwise; reported `cached_tokens` **rounded down to a multiple of 128** | `in_memory` ≈ 5–10 min idle (up to 1 h) or `24h` | **no extra fee** | model-dependent cached-input rate | implicit only (no explicit breakpoints) | same |
| **Google Gemini** — implicit | **4,096** Gemini 4 Argon, 3.8 Flash, 3.6 Flash, 3.1 Pro Preview · **2,048** Gemini 2.5 Pro, 2.5 Flash | unspecified | not stated by Google | **no write fee** | **0.05×** input on the current 3.x/4 price rows; **0.10×** on 2.5 Pro ($0.125 vs $1.25) | n/a (automatic) | ai.google.dev/gemini-api/docs/caching + /pricing |
| **Google Gemini** — explicit | varies by model; "the maximum is the same as the maximum for the given model" | separate `cachedContents` object | **1 h default** if TTL unset; any TTL, "no minimum or maximum bounds" | input price + **storage**: $0.50–$4.50 / 1M tokens **per hour** | 0.25× input | n/a | ai.google.dev/gemini-api/docs/generate-content/caching + /pricing |
| **DeepSeek** | 64 tokens is the storage unit ("content less than 64 tokens will not be cached") | **no fixed step**: cache *prefix units* are persisted at request boundaries, at detected common prefixes, and at fixed intervals inside long inputs | "cleared automatically, usually within a few hours to a few days" | **same as input price** (no premium) | **0.02×** on deepseek-flash ($0.003 vs $0.15 / MTok off-peak), **0.033×** on v4-pro | n/a (automatic) | api-docs.deepseek.com/guides/kv_cache + /news/news0802 |
| **xAI Grok** | **not published** | not published | "entries can be evicted at any time"; no TTL documented | not published (OpenRouter says no write fee `[SECONDARY]`) | per-model cached rate: grok‑4.7 **0.25×** ($0.50 vs $2.00), grok‑4.3 **0.16×**, grok-build‑0.1 **0.20×** | n/a (automatic) | docs.x.ai/developers/advanced-api-usage/prompt-caching + /pricing |
| **OpenRouter** (passthrough) | per upstream | per upstream | per upstream; sticky routing expires after **10 min** idle | per upstream | per upstream | per upstream (4 for Anthropic) | openrouter.ai/docs/features/prompt-caching |
| **Together** | "varies by model and can change, so don't design your prompts around a specific threshold" | not published | "typically about an hour", best effort | no fee | discounted | n/a (automatic) | docs.together.ai/docs/inference/chat/prompt-caching |
| **Fireworks** | not published | not published | "at least several minutes … up to several hours" | no fee | default **50 % discount** on serverless | n/a (automatic) | docs.fireworks.ai/guides/prompt-caching |

---

## Part 2. Answers A–J

### A. How many breakpoints does Anthropic allow, and what happens on the 5th?

**4.** "You can define up to 4 cache breakpoints if you want to: …" (prompt-caching,
*When to use multiple breakpoints*).

The 5th is a **hard 400, not an ignore**. Primary evidence is the automatic-caching
edge case: "If 4 explicit block-level breakpoints already exist, the API returns a
400 error (no slots left for automatic caching)." The exact body, seen in the wild:

> `API Error: 400 {"type":"error","error":{"type":"invalid_request_error","message":"A maximum of 4 blocks with cache_control may be provided. Found 5."}}`
> `[SECONDARY]` anthropics/claude-code issues #8419 and #8901

Bedrock behaves the same way: the per-model table gives "Maximum number of cache
checkpoints or writes per request: 4" for every Claude model.

Related, and directly relevant to us: **a top-level `cache_control` (automatic caching)
competes for the same 4 slots** — "an implicit breakpoint uses one of the four cache
write slots to leave three usable explicit cache write slots."

### B. Can a breakpoint sit on a `thinking` / `redacted_thinking` block?

**No.** Docs, *What cannot be cached*:

> "Thinking blocks cannot be cached directly with `cache_control`. However, thinking
> blocks CAN be cached alongside other content when they appear in previous assistant
> turns. When cached this way, they DO count as input tokens when read from cache."

And the SDK types confirm it structurally: `ThinkingBlockParam` is
`{signature, thinking, type}` and `RedactedThinkingBlockParam` is `{data, type}` —
neither has a `cache_control` field, while `TextBlockParam`, `ImageBlockParam`,
`DocumentBlockParam`, `ToolResultBlockParam`, `ToolParam` etc. all do.

So our code that skips thinking blocks is **correct**, and the failure mode is worse
than "rejected": `[SECONDARY]` the observed error is
`messages.N.content.0.thinking.cache_control: Extra inputs are not permitted`.

**But note the neighbouring trap** (docs, *Thinking blocks cannot be modified*): "every
`thinking` and `redacted_thinking` block from the assistant turn must be passed back
exactly as received, **including blocks whose `thinking` field is empty**". Mutating or
dropping one is a 400 that names the block position. See finding **R2**.

### C. Minimum cacheable prefix, exact numbers

Anthropic (Claude API, and "These minimums apply on every platform where each model is
available"):

| min | models |
|---|---|
| 512 | Claude Fable 5.1, Mythos 5.1, Opus 5.5, Opus 5, Sonnet 5.5, Fable 5, Mythos 5, Haiku 5.5 |
| 1,024 | Opus 4.8, Sonnet 5, Sonnet 4.6, Sonnet 4.5, Opus 4.1, Opus 4, Sonnet 4 |
| 2,048 | Mythos Preview, Opus 4.7, Haiku 3.5 |
| 4,096 | Opus 4.6, Opus 4.5, Haiku 4.5 |

> "Shorter prompts cannot be cached, even if marked with `cache_control`. Any requests to
> cache fewer than this number of tokens will be processed without caching, **and no
> error is returned**."

Bedrock's table matches for the models it lists, with one difference worth knowing:
Bedrock lists **Opus 4.7 at 4,096** (Anthropic's own page says 2,048) and **Opus 4.8 at
1,024**. Trust the platform you're on.

OpenAI: **1,024 visible input tokens** for GPT‑5.6+; "varies by request settings" before
that. Google implicit: 2,048 (2.5) / 4,096 (3.x, 4). DeepSeek: 64-token storage unit.

### D. What invalidates the cache

Anthropic's table (*What invalidates the cache*), columns = tools / system / messages:

| change | tools | system | messages |
|---|---|---|---|
| tool definitions (names, descriptions, parameters) | ✘ | ✘ | ✘ |
| web search toggle | ✓ | ✘ | ✘ |
| citations toggle | ✓ | ✘ | ✘ |
| `speed: "fast"` vs standard | ✓ | ✘ | ✘ |
| `tool_choice` | ✓ | ✓ | ✘ |
| images added/removed anywhere | ✓ | ✓ | ✘ |
| thinking config (mode, `budget_tokens`) | model-specific | model-specific | ✘ |
| `output_config.effort` | model-specific | model-specific | ✘ |
| non-tool-result user content while thinking | ✓ | ✓ | model-specific |

Point by point for the questions asked:

1. **Changing tools** → invalidates **everything**, including system and messages. "Modifying tool definitions (names, descriptions, parameters) invalidates the entire cache."
2. **Changing system** → invalidates system **and** messages, keeps the tools cache.
3. **Appending a message to the end** → does **not** invalidate; it is the normal growth case. Caveat: the *lookback window is 20 blocks* per breakpoint, and a run of consecutive `tool_use` (or `tool_result`) blocks counts as one position.
4. **Changing the model** → different cache entirely. Anthropic's cache diagnostics expose `model_changed` as a first-class miss reason; OpenAI: "A different model can use different weights and caching behavior."
5. **temperature / max_tokens** → **not in the invalidation table at all**, so: not documented as invalidating. (On the newest Claude models a non-default `temperature` is a 400 anyway, which is a separate matter.) For OpenAI, `reasoning.effort`, `text.verbosity`, `text.format`, `parallel_tool_calls` and `context_management` **are** listed as prefix-affecting.

Two extras from the Anthropic page that matter to an agent harness:

* **Thinking config is rendered into the prompt.** "Switching between `adaptive`, `enabled`, and `disabled`, changing `budget_tokens`, and changing the effort value all invalidate cache breakpoints: message-level breakpoints always miss, and tool and system-prompt breakpoints can miss too." So an auto-thinking layer that flips effort per turn destroys the cache.
* **Compaction.** OpenAI: "Compaction replaces earlier conversation content with a compacted context that can prevent reuse from the first changed token onward."

### E. TTL: is 5 minutes refreshed on every hit? When is 1 h worth it?

**Yes, refreshed free of charge**, but with a timing subtlety we must respect:

> "By default, the cache has a 5-minute lifetime. The cache is refreshed for no
> additional cost each time the cached content is used."
>
> "The lifetime is measured from the **start of the request** that writes or reads the
> cache entry, not from the end of its response. Time spent generating a response counts
> against the lifetime: if a response takes 4 minutes to stream, a follow-up request that
> reuses the same cached prefix must start within about 1 minute of that response
> completing."

That last sentence is the single most important operational fact for a long-horizon
agent: a turn with 4 minutes of thinking + tool calls eats 80 % of the 5-minute window.

**1-hour cache** — `{"type": "ephemeral", "ttl": "1h"}`, 2× input price for the write.
Anthropic's own guidance:

> "If you have prompts that are used at a regular cadence (that is, system prompts that
> are used more frequently than every 5 minutes), continue to use the 5-minute cache,
> because this will continue to be refreshed at no additional charge."
>
> 1 h is for: "prompts that are likely used less frequently than 5 minutes, but more
> frequently than every hour. For example, when an agentic side-agent will take longer
> than 5 minutes"; "when latency is important and your follow-up prompts may be sent
> beyond 5 minutes"; "when you want to improve your rate limit utilization, because cache
> hits are not deducted against your rate limit."

Mixing rule: "Cache entries with longer TTL must appear before shorter TTLs."

### F. Prices, and when caching is NOT worth it

Multipliers relative to the base input price:

| provider | write | read | break-even (N requests, write once then read) |
|---|---|---|---|
| Anthropic 5 m | 1.25× | 0.1× | `1.25 + 0.1(N−1) < N` → **N ≥ 2** |
| Anthropic 1 h | 2× | 0.1× | `2 + 0.1(N−1) < N` → **N ≥ 3** |
| Anthropic Opus 5.5 / Sonnet 5.5 | 1.25× | **0.05×** | N ≥ 2 (cheaper reads) |
| Anthropic Fable 5.1 / Mythos 5.1 | 1.25× | **0.025×** | N ≥ 2 |
| OpenAI GPT‑5.6+ | 1.25× | 0.1× | N ≥ 2 |
| OpenAI ≤ 5.5 | **0** | model-dependent | always |
| Gemini implicit (3.x/4) | 0 | **0.05×** | always |
| Gemini implicit (2.5 Pro) | 0 | 0.10× | always |
| Gemini explicit | input + hourly storage | 0.25× | depends on TTL: storage is charged per hour of retention |
| DeepSeek | 1× (no premium) | 0.02×–0.033× | always |
| xAI | not published | 0.16×–0.25× | effectively always |

Anthropic states the arithmetic explicitly: "writing a prefix once and fully reusing it
once costs 1.35× its ordinary input cost, compared with 2× for processing it twice
without caching" (OpenAI's page, same 1.25/0.1 numbers).

> Note on a figure you will see in the wild: OpenRouter's page claims Gemini implicit
> reads cost 0.25× and DeepSeek reads 0.1×. Both are **stale**. Computed from the
> providers' own price lists (2026-10-18) the ratios are 0.05× for current Gemini and
> 0.02× for deepseek-flash. Use the price list, not the aggregator.

**Caching is a net loss when:**

1. **You write and never read** — 1.25× instead of 1× (Anthropic 5 m, OpenAI 5.6+). Every
   turn that produces a unique prefix and is never repeated is a 25 % surcharge.
2. **You buy 1 h and read only once** — 2.1× vs 2×.
3. **The breakpoint sits on content that changes every request.** Anthropic calls this
   out by name (*Common mistake*): "Request 1: Cache write at block 6… Request 2: The
   timestamp differs, so the prefix hash at block 6 differs… **No cache hit. You pay for a
   fresh cache write on every request and never get a read.**"
4. **The prefix is below the model minimum** — you pay normal price and get nothing (no
   error, so it is invisible unless you watch `cache_creation_input_tokens`).
5. **Gemini explicit caching with a long TTL and low reuse** — storage is billed per hour
   of retention ($0.50–$4.50 / 1M tokens / hour) whether or not you read.

Note on "breakpoints cost money": they don't. "Cache breakpoints themselves don't add any
cost. You are only charged for: cache writes … cache reads … regular input tokens." And
multiple breakpoints do **not** double-bill the same tokens: billing is positional —
read up to A, write (B−A), write (C−B).

### G. OpenAI: is the step really 128 tokens? What happens at 1,000 tokens?

**The 128-token figure is real but it is not a cache-granularity rule for current
models.** Three separate statements:

* 2024 launch post: "The API caches the longest prefix of a prompt that has been
  previously computed, starting at 1,024 tokens and increasing in **128-token
  increments**."
* Current guide, GPT‑5.6+: "Reported `cached_tokens` is calculated by subtracting the
  hidden system tokens from the last matched breakpoint, then **rounding down to the
  nearest multiple of 128**" — and this sentence sits under **Earlier models**. The
  model-differences table is explicit: GPT‑5.6+ reports "Exact eligible boundary,
  excluding hidden tokens"; ≤ 5.5 "Excludes hidden tokens and rounds down to a multiple
  of 128".
* Implicit breakpoint spacing for GPT‑5.5 is **2,048 tokens**, not 128.

So: for GPT‑5.6+ there is no 128 step in what gets cached or billed; the 128 is a
*reporting* rounding on older models.

**A 1,000-token prefix is not cached at all** — it is below the 1,024 minimum. No error,
no charge, `cached_tokens: 0`. OpenAI also documents the cost trap: below the minimum
you may be better off *lengthening* the prefix to cross it (break-even formula and
worked example with M=1024, r=0.1, w=1.25 → "expanding an original prefix of at least
221 tokens to 1,024 tokens is cheaper across 10 requests").

### H. Does any provider require the cached block to be LAST or FIRST?

**No provider requires first-or-last. Every provider requires it to be a PREFIX.** The
differences are about where the *breakpoint* may go:

* **Anthropic**: content order is fixed — "Cache prefixes are created in the following
  order: `tools`, `system`, then `messages`. This order forms a hierarchy where each level
  builds upon the previous ones." The breakpoint may be on any cacheable block
  (tools entry, system block, text/image/document/tool_use/tool_result block). Static
  content should come first as a matter of *policy*, not of API validity: "Place static
  content … at the beginning of your prompt."
* **OpenAI implicit**: the breakpoint is chosen for you, "at the end of the latest
  eligible message" (GPT‑5.6+) — i.e. last, by construction. Explicit mode lets you mark
  any `input_text` block, with the one hard restriction: "Top-level `instructions` cannot
  contain an explicit breakpoint."
* **Google explicit**: the cached content is a standalone object — "The model doesn't make
  any distinction between cached tokens and regular input tokens. Cached content is a
  prefix to the prompt."
* **xAI / Together / Fireworks / DeepSeek**: automatic, prefix-only, no placement control.

### I. Are breakpoints compatible with the `tools` array?

**Yes — tools are a first-class cache level for Anthropic.** `ToolParam` carries
`cache_control` in the SDK ("Create a cache control breakpoint at this content block"),
and "Tools: Tool definitions in the `tools` array" is listed under *What can be cached*.

Two caveats that bite:

1. Because tools are checked **before** system and messages, the tools breakpoint's
   *prefix is the tools array alone*. If the roster is smaller than the model minimum
   (512–4096 tokens), that breakpoint silently writes nothing. Harmless — breakpoints are
   free — but it means the tools breakpoint is often a no-op for a small roster.
2. Tool order matters. Anthropic's troubleshooting list: "Verify that the keys in your
   `tool_use` content blocks have stable ordering as some languages … randomize key order
   during JSON conversion, breaking caches." OpenAI: `tools` "Changes tool names,
   descriptions, schemas, **ordering**…". Together: "Send the same tool definitions, with
   the same names, descriptions, schemas, and order, on every request."

Bedrock: "Cache checkpoints are processed in this order: tools → system → messages. The
minimum cache size is evaluated against the cumulative tokens across all three sections
combined, not each section individually."

### J. What does Anthropic do with two breakpoints in one block?

**The question is not expressible in the API.** `cache_control` is a single object field
per content block (`CacheControlEphemeralParam { type: "ephemeral", ttl?: "5m" | "1h" }`),
so a block has at most one breakpoint. Two blocks that *end at the same prefix position*
cannot exist either — each block ends at its own offset.

What the docs do define is what happens when a breakpoint lands where a breakpoint
already was:

* "If the last block already has an explicit `cache_control` with the same TTL, automatic
  caching is a **no-op**."
* "If the last block has an explicit `cache_control` with a **different** TTL, the API
  returns a **400 error**."

I found no documented behaviour for "two identical breakpoints at the same position"
because it cannot be sent. Marking this **unclear / not applicable** rather than guessing.

---

## Part 3. Compliance check of our implementation

Files: `crates/pa-ai/src/providers/anthropic/params.rs`,
`.../anthropic/convert.rs`, `.../anthropic/mod.rs`,
`crates/pa-core/src/prompts/system_prompt.rs`,
`crates/pa-core/src/session_engine/engine.rs`,
`crates/pa-agent/src/agent_loop/response.rs`,
`crates/pa-ai/src/providers/bedrock/convert.rs`,
`crates/pa-ai/src/providers/openai_completions/params.rs`,
`crates/pa-ai/src/providers/openai_responses.rs`.

Legend: ✅ compliant · ⚠️ works but leaves money on the table · ❌ violation / 400 risk.

| # | Rule | Where we implement it | Verdict |
|---|---|---|---|
| 1 | ≤ 4 breakpoints per Anthropic request | `params.rs:73-106` (system split) + `params.rs:126-131` (tools) + `convert.rs:231-265` (last user block) | ✅ exactly 4 in the worst case — verified by running the real builder (see §3.1) |
| 2 | No `cache_control` on thinking blocks | `convert.rs:252-254` — only `text`, `image`, `tool_result` are tagged | ✅ |
| 3 | Thinking blocks must be replayed byte-identically | `convert.rs:156-180` | ❌ **R2** — empty thinking dropped, unsigned thinking rewritten as text |
| 4 | Breakpoint must sit on a block that is byte-stable | `params.rs:78-99` (stable half gets its own breakpoint) | ✅ for non-OAuth; ❌ for OAuth (**R1**) and Bedrock (**R6**) |
| 5 | Static content first, volatile last | `system_prompt.rs:146` (`cached_prefix_len` = sum of static segments) | ✅ |
| 6 | Tools order stable | `engine.rs:673` `tools.sort_by(name)` | ✅ |
| 7 | JSON key order stable | `Cargo.toml:48` `serde_json` with `preserve_order` | ✅ |
| 8 | Minimum prefix length per model | none — we never check | ⚠️ **R7** — silent no-op on small prompts; no metric |
| 9 | 20-block lookback | breakpoint moves to the last user message every turn | ✅ (each turn adds ≈2 positions) |
| 10 | 1 h TTL only where supported | `anthropic/mod.rs:207-224` gates on `supports_long_cache_retention` | ✅ for the Claude API; ❌ on Bedrock (**R6b**) |
| 11 | Longer TTL before shorter TTL | one `CacheControl` used everywhere → never mixed | ✅ |
| 12 | No top-level `cache_control` competing for slots | we never send it | ✅ |
| 13 | Empty text blocks must not be cached | `convert.rs:120-127` filters whitespace-only text; `params.rs:75` requires `at > 0 && at < len` | ✅ |
| 14 | `is_char_boundary` before `split_at` | `params.rs:76` | ✅ |
| 15 | OpenAI: `prompt_cache_retention` | `openai_responses.rs:71-78`, `openai_completions/params.rs:43-45` | ❌ **R5** — `"24h"` is a pre‑5.6 field |
| 16 | xAI: `x-grok-conv-id` | not sent | ⚠️ **R8** |
| 17 | Gemini: nothing to do (implicit only on 4.x) | no code | ✅ (nothing required) |

### 3.1 Verified by execution, not by reading

I instrumented the real `build_params` and ran it (probe removed afterwards):

```
AUDIT non-oauth breakpoints = 4
AUDIT oauth breakpoints = 3
AUDIT no-tools breakpoints = 3
```

Non-OAuth wire shape with a split system prompt:

```json
"system": [
  {"type":"text","text":"STATIC PREFIX","cache_control":{"type":"ephemeral"}},
  {"type":"text","text":"\n\nVOLATILE TAIL","cache_control":{"type":"ephemeral"}}
],
"tools": [ …, {"name":"tool2", …, "cache_control":{"type":"ephemeral"}} ],
"messages": [{"role":"user","content":[{"type":"text","text":"q","cache_control":{"type":"ephemeral"}}]}]
```

And the real prompt boundary, from `pa-cli prompt --json` in this repo:

```
cachedPrefixLen 23642   total bytes 65649
tail of stable  : b'...these should be discussed with the user\n  - in UI, descriptions on everything'
head of volatile: b'\n\nPre-installed Python packages: requests, httpx, yaml (PyYAML), ...'
```

So the boundary is clean and the static block is ~23.6 KB ≈ 5.9 k tokens (English at
~4 bytes/token) — above every model minimum in the table, including the 4,096 ones.
The remaining ~42 KB of system prompt (project context 17 KB + skills inventory 23 KB)
is in the volatile block, correctly outside the reusable hash.

---

## Part 4. Risks — ranked

### R1 ❌ OAuth path ignores `system_prompt_cache_prefix` entirely (money)

`params.rs:38-66`. The `is_oauth_token` branch pushes the Claude Code identity block and
then **the whole system prompt as one block** with one breakpoint. The split that P0 added
lives only in the `else` branch. Confirmed by execution: OAuth produces
`[identity][STATIC PREFIX\n\nVOLATILE TAIL]` — no split.

Consequence: for every `sk-ant-oat` session the volatile tail (42 KB of the 65 KB prompt,
i.e. ~65 % of the system prompt) is *inside* the cached hash. Two different projects with
the same static prompt can never share the cache, and any tail change re-writes the whole
system prefix. The whole point of P0 — a cross-session reusable static block — is lost on
exactly the subscription path most people use.

Fix: apply the same split in the OAuth branch. It costs nothing: identity(no bp) +
stable(bp) + volatile(bp) + tools(bp) + message(bp) = 4.

### R2 ❌ Thinking blocks are mutated → 400 on the newest models

`convert.rs:156-180`. Two mutations:

```rust
if thinking.thinking.trim().is_empty() { continue; }        // drops the block
…
if signature.trim().is_empty() { blocks.push(text block) }  // rewrites the type
```

Anthropic: "every `thinking` and `redacted_thinking` block from the assistant turn must be
passed back exactly as received, **including blocks whose `thinking` field is empty**" —
otherwise 400 `invalid_request_error` naming `messages.N.content.M`.

This is not hypothetical: on Fable 5.1, Mythos 5.1/5, Opus 5.5, Opus 5, Sonnet 5.5,
Sonnet 5, Opus 4.8, Opus 4.7, Mythos Preview and Haiku 5.5 the **default `display` is
`"omitted"`**, which returns thinking blocks with an **empty `thinking` field**. Our
`params.rs:139-140` defaults `display` to `Summarized`, which hides the bug today — but
(a) that is a one-line default away from breaking, and (b) any block that arrives with an
empty `thinking` (a progress-update block, or a response truncated on `max_tokens`) is
dropped today. I reproduced the drop with a probe: input
`[thinking(empty, sig), text("hi")]` → output `[{"type":"text","text":"hi"}]`.

Note the cache angle too: dropping a thinking block "changes the cached prefix from that
block's position onward on that request" (prompt-caching docs, *Dropped thinking blocks*).

Fix: emit `{"type":"thinking","thinking":"","signature":sig}` verbatim; never re-type a
block; never filter one out.

### R3 ⚠️ 4 breakpoints is the ceiling, with no headroom for the 20-block fix

`params.rs` uses tools(1) + stable(1) + volatile(1) + last-message(1) = 4. Anthropic's own
remedy for a long tool loop is *a second message breakpoint* ("Adding a second breakpoint
at block 15 starts a second lookback window there"). We have no slot for it. Any future
addition → `A maximum of 4 blocks with cache_control may be provided. Found 5.` → 400 on
every request.

Also: is the volatile-system breakpoint worth a slot? Its only job is to catch the case
where the message-level breakpoint misses. It costs nothing in money (breakpoints are
free, writes are positional) but it consumes the last slot. Consider dropping it and
spending the slot on a second message breakpoint instead.

### R4 ⚠️ 5-minute TTL vs. long turns

Anthropic: "The lifetime is measured from the start of the request… if a response takes
4 minutes to stream, a follow-up request … must start within about 1 minute." Our agentic
turns routinely run minutes of thinking plus tool calls. With the default `Short`
retention (`PI_CACHE_RETENTION` unset) every turn that runs >5 min starts from a cold
cache — and pays a fresh 1.25× write on the whole prefix. For long-horizon sessions
`PI_CACHE_RETENTION=long` is not a nicety, it is the cheaper configuration, *provided* the
session actually reuses the prefix ≥3 times (1 h break-even N ≥ 3).

### R5 ❌ OpenAI: `prompt_cache_retention: "24h"` on GPT‑5.6+ models

`openai_responses.rs:71-78` and `openai_completions/params.rs:43-45` send
`prompt_cache_retention: "24h"` whenever retention is `Long`. OpenAI's migration notes
say the opposite: "Replace `prompt_cache_retention` with `prompt_cache_options.ttl`", and
the model-differences table lists supported retention values as `"30m"` for GPT‑5.6+ and
`"24h"` only for GPT‑5.5/5.5 Pro. Best case the field is ignored; worst case it is a 400
on the newest models. This is a config-dependent latent failure (`Long` only), so it will
surface the first time someone sets `PI_CACHE_RETENTION=long` on a GPT‑5.6 endpoint.

### R6 ❌ Bedrock: Claude 5.x gets **no** cache points at all

`bedrock/convert.rs:74-85`:

```rust
candidates.iter().any(|s| {
    s.contains("-4-") || s.contains("claude-3-7-sonnet") || s.contains("claude-3-5-haiku")
})
```

Evaluated against the real catalog (`/root/.prime/agent/models/provider-model-catalog.v1.json`):

```
anthropic.claude-opus-5-5                  supports_prompt_caching=False
anthropic.claude-sonnet-5-5                supports_prompt_caching=False
anthropic.claude-fable-5-1                 supports_prompt_caching=False
anthropic.claude-sonnet-5                  supports_prompt_caching=False
anthropic.claude-opus-5                    supports_prompt_caching=False
anthropic.claude-opus-4-8                  supports_prompt_caching=True
anthropic.claude-haiku-4-5-20251001-v1:0   supports_prompt_caching=True
```

Bedrock documents explicit prompt caching for **every one of those 5.x models**, at a
512-token minimum. We send no `cachePoint` at all → 100 % of input billed at full rate on
the newest Bedrock models. This is the largest single money leak in the audit.

**R6b** — same file, `build_system_prompt` / `convert_messages`: `ttl: "1h"` is emitted
whenever `cache_retention == Long`, with no per-model TTL check. Bedrock: "For models that
support only the 5-minute TTL, omit the `ttl` field. **Including the field for these
models can result in a `ValidationException`.**" Claude 3.7 Sonnet and 3.5 Sonnet v2 are
5-minute-only in Bedrock's own table and our predicate returns `true` for both →
`PI_CACHE_RETENTION=long` + those models = 400.

**R6c** — Bedrock also ignores `system_prompt_cache_prefix`: `build_system_prompt` takes
`Option<&str>` and never splits. Same class of loss as R1.

### R7 ⚠️ No minimum-length guard, and no cache telemetry

We never check the model's minimum, and we never surface `cache_creation_input_tokens` /
`cache_read_input_tokens` (they are parsed in `anthropic/stream.rs:316-317` and turned
into cost, but nothing warns when a "cached" request produced a 0/0 pair). Anthropic gives
the exact diagnostic for this: "if both `cache_creation_input_tokens` and
`cache_read_input_tokens` are 0, the prompt was not cached (likely because it did not meet
the minimum length requirement)." A one-line warning when we sent a breakpoint and got
0/0 back would have caught R1 and R6 the day they landed.

### R8 ⚠️ xAI: no `x-grok-conv-id`

`openai_completions/params.rs:326-340` sends `session_id`, `x-client-request-id`,
`x-session-affinity` — but only when `compat.send_session_affinity_headers` is true, which
is `false` for `provider == "xai"`. xAI: "**Always set `x-grok-conv-id`** (or
`prompt_cache_key` for Responses API) — Routes requests to the same server, maximizing
cache hits." Without it our Grok sessions are routed arbitrarily and hit whatever cache
the load balancer happens to have.

### R9 ⚠️ Auto-thinking destroys the cache

`session_engine/auto_thinking.rs` changes the session's thinking level per turn
(`set_thinking_level`). Anthropic: "Switching between `adaptive`, `enabled`, and
`disabled`, changing `budget_tokens`, and changing the effort value all invalidate cache
breakpoints: message-level breakpoints always miss, and tool and system-prompt breakpoints
can miss too." So on a session with `[jev] autoThinking = auto` the classifier can
alternate levels and force a full re-write on every alternation. Worth a guard: only change
the level when the change is monotone within a session, or accept the cost knowingly.

### R10 ⚠️ Harness digest rewrites history instead of appending

`harness_digest.rs:539-565` (`append_stale_harness_digest`) filters *every* existing digest
row out of the message vector and pushes a fresh one at the end:

```rust
.filter(|existing| !is_digest_row(existing, latest_digest.as_deref()))
…
messages.push(message);
```

Deleting a message from the middle of history changes the prefix from that point → the
messages-level cache is invalidated wholesale. Today this only runs at cold boundaries
(`ensure_harness_digest_context`, called from `mod.rs:271`), where there is no warm cache
to lose — but if it is ever called mid-session (a "stale digest" refresh after
`refine.run()`, which is the natural next step) it will silently cost a full prefix
re-write. The append-only rule is explicit in every provider's docs: "Append new messages
rather than rewriting earlier turns. Summarization, compaction, or context truncation can
change the prefix and reset cache reuse."

---

## Part 5. Sources

All fetched **2026-10-18**.

**Primary**

1. Anthropic — Prompt caching · https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching
   (canonical: https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
2. Anthropic — Thinking (incl. *Thinking and prompt caching*, *Thinking blocks cannot be modified*, per-model `display` defaults) · https://platform.claude.com/docs/en/build-with-claude/thinking
3. Anthropic — Errors (400 catalogue) · https://platform.claude.com/docs/en/api/errors
4. Anthropic — Messages API reference (`ThinkingBlockParam`, `RedactedThinkingBlockParam`, `CacheControlEphemeral`, `CacheMissReason`) · https://platform.claude.com/docs/en/api/messages
5. Anthropic — Cache diagnostics (`cache_miss_reason`, `previous_message_id`) · https://platform.claude.com/docs/en/build-with-claude/cache-diagnostics
6. Anthropic SDK (types are normative for what the API accepts) · https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/cache_control_ephemeral_param.py · `…/thinking_block_param.py` · `…/tool_param.py` · `…/message_create_params.py`
7. Amazon Bedrock — Prompt caching (per-model minimums, max checkpoints, TTL support) · https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html
8. OpenAI — Prompt caching · https://platform.openai.com/docs/guides/prompt-caching (mirror: https://developers.openai.com/api/docs/guides/prompt-caching)
9. OpenAI — Pricing (cached input / cache writes columns) · https://platform.openai.com/docs/pricing
10. OpenAI — *Prompt Caching in the API* (2024-10-01, the 1,024/128 statement) · https://openai.com/index/api-prompt-caching/
11. OpenAI Cookbook — Prompt Caching 201 (explicitly scoped to models before GPT‑5.6) · https://developers.openai.com/cookbook/examples/prompt_caching_201
12. Google — Context caching (implicit) · https://ai.google.dev/gemini-api/docs/caching
13. Google — Context caching (explicit, generateContent) · https://ai.google.dev/gemini-api/docs/generate-content/caching
14. Google — Gemini API pricing (cached-token price + per-hour storage) · https://ai.google.dev/gemini-api/docs/pricing
15. DeepSeek — Context Caching · https://api-docs.deepseek.com/guides/kv_cache
16. DeepSeek — Context Caching announcement (64-token storage unit) · https://api-docs.deepseek.com/news/news0802
17. DeepSeek — Models & Pricing · https://api-docs.deepseek.com/quick_start/pricing
18. xAI — Prompt Caching · https://docs.x.ai/developers/advanced-api-usage/prompt-caching
19. xAI — How It Works · https://docs.x.ai/developers/advanced-api-usage/prompt-caching/how-it-works
20. xAI — Maximizing Cache Hits (`x-grok-conv-id`) · https://docs.x.ai/developers/advanced-api-usage/prompt-caching/maximizing-cache-hits
21. xAI — Usage & Pricing · https://docs.x.ai/developers/advanced-api-usage/prompt-caching/usage-and-pricing
22. xAI — Best Practices & FAQ · https://docs.x.ai/developers/advanced-api-usage/prompt-caching/best-practices
23. xAI — Pricing (cached column) · https://docs.x.ai/developers/pricing
24. OpenRouter — Prompt Caching (passthrough + sticky routing) · https://openrouter.ai/docs/features/prompt-caching
25. Together — Prompt caching · https://docs.together.ai/docs/inference/chat/prompt-caching
26. Fireworks — Prompt caching · https://docs.fireworks.ai/guides/prompt-caching

**Secondary (error strings / cross-checks only, labelled `[SECONDARY]` above)**

27. anthropics/claude-code #8419 · https://github.com/anthropics/claude-code/issues/8419
28. anthropics/claude-code #8901 · https://github.com/anthropics/claude-code/issues/8901
29. litellm PR #30480 (caps injection at 4 for the same 400) · https://github.com/BerriAI/litellm/pull/30480

**Local evidence**

30. `crates/pa-ai/src/providers/anthropic/params.rs` (split + breakpoints), `convert.rs` (thinking + message/tool breakpoints), `mod.rs` (`CacheControl`, `get_cache_control`)
31. `crates/pa-core/src/prompts/system_prompt.rs:146` (`cached_prefix_len`), `session_engine/engine.rs:673,785`
32. `crates/pa-agent/src/agent_loop/response.rs:116-127` (hook rewrite drops the boundary)
33. `crates/pa-ai/src/providers/bedrock/convert.rs:74-85,133-150,300-316`
34. Live probe of `build_params` (4 / 3 / 3 breakpoints) and `pa-cli prompt --json` (23,642 of 65,649 bytes static) — reproducible, probe reverted.

---

## Appendix. Things the docs do NOT say (so I did not invent them)

* Anthropic: no published *token step* for cache granularity. Breakpoints are per block;
  there is no "multiple of N tokens" rule.
* Anthropic: no documented limit on how many cache *entries* one prefix may have, and no
  documented behaviour for two breakpoints on one block (not expressible).
* xAI: no published minimum prefix length, no published TTL, no published write price —
  only the read price per model.
* Together / Fireworks: deliberately publish no minimum ("don't design your prompts
  around a specific threshold").
* Google: no published TTL or eviction policy for **implicit** caching; explicit caching
  has "no minimum or maximum bounds on the TTL".
* OpenAI: the minimum for pre‑5.6 models is described only as "varies by request settings
  (tools, images, output schemas, reasoning effort, verbosity)" — no table.
