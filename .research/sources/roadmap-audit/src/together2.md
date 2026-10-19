url: https://docs.together.ai/docs/inference/chat/prompt-caching
title: Prompt caching - Together AI docs
content: >-
  Chat & text


  Together reuses previously processed prompt prefixes to lower input cost and
  time to first token.


  Prompt caching reuses the work a model has already done on the beginning of a
  prompt. When a request starts with the same tokens as an earlier request, the
  model loads the saved state for that shared prefix instead of recomputing it,
  then processes only the tokens that follow.Prompt caching is automatic on
  serverless inference and dedicated model inference. There’s no parameter,
  header, or account setting to turn it on, and no cache ID or expiry setting to
  manage. Caching never changes how the model generates its output.


  ## How prompt caching works


  To process a prompt, the model computes intermediate key-value (KV) states for
  every input token, which let it attend to earlier tokens while it generates.
  Prompt caching stores the KV states for a prompt’s prefix, so a later request
  with the same prefix skips that computation.The prefix is the prompt exactly
  as the model sees it, after the model’s chat template renders your request
  into tokens. Everything the chat template renders counts toward the prefix:


  *   The system prompt and every earlier message, including images.

  *   Tool definitions in `tools`.

  *   Request settings the chat template renders into the prompt, such as
  `reasoning`, `reasoning_effort`, and `chat_template_kwargs`.


  A request reuses the longest prefix it shares with a cached entry. Tokens up
  to the first difference can come from the cache, and every token from the
  first difference onward is processed from scratch.


  Cache entries typically last about an hour. This is done on a best effort
  basis. For serverless models, the cache is shared across the fleet, so under
  heavy load, entries may be evicted sooner. The minimum cacheable prefix length
  and the granularity of cached tokens vary by model and can change, so don’t
  design your prompts around a specific threshold.


  ## Maximize cache hits


  Any change that alters the rendered prompt ends the match at the point of the
  change. Structure requests so the part that changes comes last:


  *   **Put stable content first:** Place instructions, tool definitions,
  reference documents, and few-shot examples at the start of the prompt. Put
  per-request content, such as the user’s question, timestamps, or retrieved
  snippets, at the end. A timestamp or username in the system prompt makes every
  request’s prefix unique from that point on.

  *   **Append instead of rewriting:** In a multi-turn conversation, add new
  messages to the end of the history and keep earlier turns byte-identical, so
  the reusable prefix grows with each turn. Summarizing, truncating, or
  reordering the history changes the prefix from the first edited message
  onward.

  *   **Keep tools stable:** Send the same tool definitions, with the same
  names, descriptions, schemas, and order, on every request. Tool definitions
  are part of the prefix, so changing any of them ends the match where the chat
  template renders them.

  *   **Set reasoning options once per conversation:** Choose `reasoning` and
  `reasoning_effort` before the conversation starts. Changing them partway
  through changes the rendered prompt.

  *   **Pass reasoning back unchanged:** For models that use preserved thinking,
  return prior reasoning exactly as the model generated it. Editing or
  reordering it changes the prefix.

  *   **Serialize content deterministically:** If your code builds tool schemas
  or JSON message content, keep the key order the same between requests. The
  same data serialized in a different order renders as different tokens.

  *   **Stay on one model:** KV states are specific to the model that computed
  them, so a cache entry only serves requests to the same model.


  Where dynamic content sits decides how much of the prompt the next request can
  reuse:


  When each turn appends to the history, every request reuses most of the
  previous request’s prompt:


  When an earlier turn is summarized or truncated, the next request reuses only
  the content before the edit:


  Set `prompt_cache_key` to a stable string on requests that share a prefix,
  such as a conversation ID or a version name for your application’s system
  prompt. Together uses the key to route those requests as a group, which raises
  the chance that each one lands where its prefix is already cached. The key
  improves the odds of a cache hit but doesn’t guarantee one.On dedicated
  endpoints, `prompt_cache_key` also serves as the request’s sampling key for
  routing stickiness, so requests with the same key always route to the same
  deployment.In the Python SDK, pass `prompt_cache_key` in `extra_body`, as
  shown in the example in the next section. In the TypeScript SDK and the REST
  API, pass it as a top-level request field.


  ## Check cached tokens


  The `usage` object on each response reports how many prompt tokens came from
  the cache. This example sends two requests that share a long system prompt and
  prints the cached count for each:


  You should see output similar to this:


  The second request reads 3,327 of its 3,417 prompt tokens from the cache. The
  rest are the tokens after the shared prefix: the new question and the tokens
  the chat template adds around it. The first request can also report a few
  cached tokens when the start of its rendered prompt matches an earlier
  request.Kimi K3 reports the count in
  `usage.prompt_tokens_details.cached_tokens`. Some models return
  `cached_tokens` at the top level of `usage` instead, so the example checks
  both. See OpenAI compatibility for the full shape of the `usage` object.To
  track your cache hit rate, divide the total `cached_tokens` by the total
  `prompt_tokens` across your requests.


  ## Pricing


  On serverless inference, cached tokens are billed at the model’s cached input
  rate, listed in the **Cached input pricing** column of Chat models. All other
  input tokens are billed at the standard input rate. There’s no charge for
  writing to the cache. Models without a cached input price bill every input
  token at the standard rate.For example, the second request in Check cached
  tokens bills 3,327 cached tokens at Kimi K3’s cached input rate and the
  remaining 90 tokens at its standard input rate.Dedicated model inference bills
  for GPU time rather than tokens, so there’s no cached-token discount. But
  caching still pays off: replicas skip computation for cached prefixes, so the
  same deployment can serve more tokens and reach its first token
  sooner.Requests that hit the cache still count toward your rate limits.
