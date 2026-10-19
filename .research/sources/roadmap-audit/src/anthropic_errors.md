url: https://platform.claude.com/docs/en/api/errors
title: Claude API errors
content: >-
  Understand the HTTP status codes, error response shape, and request IDs the
  Claude API returns, and handle errors with the SDK's typed exceptions.


  ## HTTP errors


  The API follows a predictable HTTP error code format:


  *   400 - `invalid_request_error`: There was an issue with the format or
  content of your request. This error type may also be used for other 4XX status
  codes not listed in this section. The API also returns a 400 when usage
  reaches an organization or workspace spend limit you set, except limits on the
  Claude Code workspace, which can return a 429 instead.


  *   401 - `authentication_error`: There's an issue with your API key (for
  example, it's malformed, revoked, or expired; see Key expiration). On Claude
  Platform on AWS, this can also indicate a problem with your AWS credentials or
  SigV4 signature.


  *   402 - `billing_error`: There's an issue with your billing or payment
  information. Check your payment details in the Claude Console, or in AWS
  Marketplace if you're using Claude Platform on AWS.


  *   403 - `permission_error`: Your API key does not have permission to use the
  specified resource. Check your organization's access and workspace settings in
  the Claude Console.


  *   404 - `not_found_error`: The requested resource was not found. Check the
  endpoint path and any resource IDs in the request URL.


  *   409 - `conflict_error`: The request conflicts with the current state of a
  resource. For example, the resource was modified concurrently, or a value that
  must be unique is already in use. Resolve the conflict, then retry the
  request.


  *   413 - `request_too_large`: Request exceeds the maximum allowed number of
  bytes. See Request size limits for per-endpoint maximums.


  *   429 - `rate_limit_error`: Your organization has hit a rate limit, reached
  its usage tier's monthly spend cap, or reached a spend limit on the Claude
  Code workspace. A tier spend-cap 429 has no `retry-after` header and keeps
  failing until access resumes; see Reaching your spend cap for how to recognize
  it.


  *   500 - `api_error`: An unexpected error has occurred internal to
  Anthropic's systems. Retry the request with exponential backoff; if the error
  persists, contact support with the request ID.


  *   504 - `timeout_error`: The request timed out while processing. Consider
  using the streaming Messages API for long-running requests. See Long requests
  for more options.


  *   529 - `overloaded_error`: The API is temporarily overloaded.


  The official SDK automatically retries transient failures (such as connection
  errors, rate limits, and 5xx server errors) with exponential backoff, twice by
  default, honoring the `retry-after` header when present. The SDK client
  accepts `max_retries` to configure or disable this behavior.


  When receiving a streaming response over server-sent events (SSE), an error
  can occur after the API returns a 200 response. In that case, error handling
  doesn't follow these standard mechanisms. See Error events for the shape of
  mid-stream errors.


  ## Request size limits


  The API enforces request size limits:


  | Endpoint type | Maximum request size |

  | --- | --- |

  | Messages API | 32 MB |

  | Token Counting API | 32 MB |

  | Batch API | 256 MB |

  | Files API | 500 MB |


  If you exceed these limits, you'll receive a 413 `request_too_large` error. On
  the direct Claude API, Cloudflare returns this error before the request
  reaches the API servers.


  ## Error shapes


  The API always returns errors as JSON, with a top-level `error` object that
  always includes a `type` and `message` value. The response also includes a
  `request_id` field for easier tracking and debugging. For example:


  In accordance with the versioning policy, the values within these objects may
  expand, and it is possible that the `type` values will grow over time.


  ## SDK error types


  The official SDK raises typed exceptions for these errors instead of returning
  raw JSON. For example, a 404 surfaces as `anthropic.NotFoundError`. The Go SDK
  has one error type for every status, `*anthropic.Error`: branch on
  `StatusCode`. Catch the SDK's typed classes rather than string-matching error
  messages, handling the most specific classes first. Your SDK's page documents
  the full exception hierarchy:


  *   Python · TypeScript · C# · Go · Java · PHP · Ruby


  ## Request ID


  Every API response includes a unique `request-id` header. This header contains
  a value such as `req_018EeWyXxfu5pfWkrYcMdjWG`. The same identifier appears as
  the `request_id` field in error response bodies. When contacting support about
  a specific request, include this ID to help quickly resolve your issue.


  On Claude Platform on AWS, responses include two request IDs: the AWS request
  ID (`x-amzn-requestid`, primary, indexed in CloudTrail) and the Anthropic
  request ID (`request-id`, secondary). Use the AWS request ID for CloudTrail
  lookups and the Anthropic request ID for Anthropic support tickets.


  The Python and TypeScript SDKs expose the request ID as a `_request_id`
  property on top-level response objects. The C#, Go, Java, and PHP SDKs expose
  it through their raw-response accessors, and the Ruby SDK through middleware.
  In every SDK except Ruby, use `with_raw_response` to read any other response
  header, such as `anthropic-organization-id` and `anthropic-workspace-id`. In
  Ruby, use the same middleware. On Claude Platform on AWS, use the raw-response
  accessor to read the AWS request ID (`x-amzn-requestid`) as well:


  For Claude Platform on AWS request-ID examples in other languages, see Request
  IDs.


  ## Long requests


  Avoid setting a large `max_tokens` value without using the streaming Messages
  API or Message Batches API:


  *   Some networks may drop idle connections after a variable period of time,
  which can cause the request to fail or time out without receiving a response
  from Anthropic.

  *   Networks differ in reliability. The Message Batches API can help you
  manage the risk of network issues by allowing you to poll for results rather
  than requiring an uninterrupted network connection.


  If you are building a direct API integration, setting a TCP socket keep-alive
  can reduce the impact of idle connection timeouts on some networks.


  The SDKs validate that your non-streaming Messages API requests are not
  expected to exceed a 10-minute timeout. They also set a socket option for TCP
  keep-alive.


  If you don't need to process events incrementally, the SDK can consume the
  stream for you and return the complete `Message` object, identical to what a
  non-streaming call returns:


  See Streaming Messages for more details.


  ## Common validation errors


  ### Prefill not supported


  Claude 4.6 and later models and Claude Mythos Preview do not support
  prefilling assistant messages. Sending a request with a prefilled last
  assistant message to any of these models returns a 400
  `invalid_request_error`:


  Use structured outputs on models that support it, system prompt instructions,
  or `output_config.format` instead.


  ### Thinking blocks cannot be modified


  If the most recent assistant message contains `thinking` or
  `redacted_thinking` blocks that were edited, reordered, filtered out, or
  reconstructed before being sent back to the API, the request returns a 400
  `invalid_request_error`. The error message starts with the position of the
  offending block (for example, `messages.1.content.0`) and contains:


  With tool use, every `thinking` and `redacted_thinking` block from the
  assistant turn must be passed back exactly as received, including blocks whose
  `thinking` field is empty. Pass thinking blocks back unchanged, and if your
  application filters content blocks by type before resending, include both
  `thinking` and `redacted_thinking`. See Troubleshooting thinking, Preserving
  thinking blocks, and Preserved thinking.


  ### Extended thinking not supported


  Claude 4.7 and later models have removed extended thinking. Sending `thinking:
  {"type": "enabled"}` to any of these models returns a 400
  `invalid_request_error`:


  Use adaptive thinking instead. Migrating to adaptive thinking shows the
  parameter mapping, and Troubleshooting thinking covers the symptom-first fix.


  ### Adaptive thinking not supported


  Models that support only extended thinking (Claude 4.5 and earlier models)
  reject `thinking: {"type": "adaptive"}` with a 400 `invalid_request_error`:


  Use `thinking: {"type": "enabled", "budget_tokens": N}` on these models; see
  Extended thinking for the configuration and Troubleshooting thinking for the
  symptom-first fix.


  ### Thinking cannot be disabled


  On Claude Fable 5.1, Claude Mythos 5.1, Claude Fable 5, Claude Mythos 5,
  Claude Opus 5.5, and Claude Mythos Preview, thinking is always on. Sending
  `thinking: {"type": "disabled"}` to any of these models returns a 400
  `invalid_request_error`. On all of these models except Claude Mythos Preview,
  the message reads:


  On Claude Mythos Preview, the only one of these models that accepts extended
  thinking, the message reads:


  On Claude Sonnet 5.5, thinking can't be set to `disabled`. Use `thinking:
  {"type": "between_tools"}` for the lowest thinking setting, which turns off
  up-front thinking. Sending `thinking: {"type": "disabled"}` returns a 400
  `invalid_request_error` with this message:


  At `xhigh` or `max`effort, a request with `between_tools` also returns a 400
  `invalid_request_error`. The message says thinking is disabled because
  `between_tools` has no up-front thinking:


  With `between_tools`, effort can't change mid-conversation: a per-message
  `output_config.effort` that differs from the level in effect returns a 400
  error. The error names the position of the message that set the new level:


  Claude Haiku 5.5 accepts `thinking: {"type": "disabled"}` and holds it to the
  same two effort limits that apply to `between_tools`: at `xhigh` or `max`
  effort, or with a per-message `output_config.effort` that differs from the
  level in effect, the request returns a 400 `invalid_request_error` with the
  matching message shown earlier.


  In both messages, "enable thinking" means adaptive thinking: omit the
  `thinking` field or send `thinking: {"type": "adaptive"}`. Claude Sonnet 5.5
  and Claude Haiku 5.5 reject `"enabled"` with a 400 error. To vary effort per
  turn, use adaptive thinking.


  Sending `thinking: {"type": "between_tools"}` to any model other than Claude
  Sonnet 5.5 returns a 400 `invalid_request_error`:


  For the fixes, see Troubleshooting thinking, which covers the `between_tools`
  and effort errors.


  Omit the `thinking` parameter and the request runs with adaptive thinking. To
  keep thinking content out of responses without turning thinking off, set
  `display: "omitted"` on the thinking configuration. See Troubleshooting
  thinking.


  ### Forced tool use not supported


  Claude Opus 5.5, Claude Sonnet 5.5, Claude Fable 5.1, and Claude Mythos 5.1
  don't support forced tool use. Sending `tool_choice: {"type": "any"}` or
  `tool_choice: {"type": "tool", "name": "..."}` to any of these models,
  including on the token counting endpoint, returns a 400
  `invalid_request_error`:


  `tool_choice: {"type": "auto"}` (the default) and `{"type": "none"}` are
  accepted. Use `auto` with strict tool use to keep tool inputs schema-valid, or
  structured outputs when you need the response itself in a fixed JSON shape.
  See Forcing tool use.


  ### Computer use tool version not supported


  On the Claude API and Google Cloud, Claude Opus 5.5, Claude Sonnet 5.5, and
  Claude Haiku 5.5 support computer use only as the `computer_toolset_20260801`
  toolset. On those platforms, sending any of these models a `tools` entry of
  the earlier `computer_20251124` or `computer_20250124` type, with that tool's
  beta header, returns a 400 `invalid_request_error`. The message names the
  rejected type, then lists the tool types the model does accept after `Did you
  mean one of`. For Claude Opus 5.5, it begins:


  The API returns the same message for any Anthropic-defined tool type that the
  requested model doesn't support. Declare `{"type":
  "computer_toolset_20260801"}` without the beta header and update your agent
  loop as described in Migrate from `computer_20251124`. To move from
  `computer_20250124`, see Move computer use to the toolset in the Claude Haiku
  5.5 migration guide. Earlier models that support the toolset keep accepting
  `computer_20251124`, as do Claude Opus 5.5, Claude Sonnet 5.5, and Claude
  Haiku 5.5 on Amazon Bedrock.


  ### Thinking block no longer matches the conversation


  On Claude Fable 5.1, Claude Opus 5.5, Claude Sonnet 5.5, and Claude Haiku 5.5,
  the API accepts a replayed thinking block only while the `system` prompt,
  `tools`, and messages that preceded it are unchanged. For new accounts created
  on or after August 31, 2026, and for any request that sets
  `thinking.block_binding.prefix_mismatch_behavior` to `"error"`, a replayed
  block whose earlier history changed is rejected with a 400
  `invalid_request_error` (with `"drop_block"`, the API drops the block and the
  request succeeds). The message starts with the position of the first failing
  block:


  Without the `thinking-binding-controls-2026-08-01` beta header the message
  also names that header. Keep the conversation history append-only, or send the
  beta header with `prefix_mismatch_behavior: "drop_block"` to drop the block
  and continue. On Claude Sonnet 5.5 and Claude Haiku 5.5, `block_binding` works
  only with `thinking: {"type": "adaptive"}`. With `between_tools` on Claude
  Sonnet 5.5, or `thinking: {"type": "disabled"}` on Claude Haiku 5.5, keep the
  history append-only, or strip the thinking blocks from the edited turn on. A
  block from a model the target model can't read is dropped rather than
  rejected. See Keeping the prefix unchanged and Troubleshooting thinking.


  Sending `thinking.block_binding` without the
  `thinking-binding-controls-2026-08-01`beta header returns a 400
  `invalid_request_error` whose message ends in:


  Add the header, or remove the field.


  ### Outbound web identity federation disabled (Claude Platform on AWS)


  If every request to Claude Platform on AWS returns `"Outbound web identity
  federation is disabled for your account"`, run `aws iam
  enable-outbound-web-identity-federation` once per AWS account. See Enable
  outbound web identity federation for details.


  ## Next steps


  Symptom-first fixes for thinking configuration 400 errors, empty thinking
  blocks, and `max_tokens` stops.


  To mitigate misuse and manage capacity on the API, limits are in place on how
  much an organization can use the Claude API.


  Stream Messages API responses incrementally with server-sent events, including
  text, tool use, and extended thinking deltas.
