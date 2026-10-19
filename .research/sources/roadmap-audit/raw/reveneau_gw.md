url: https://reveneau.com/guides/claude-code-prompt-caching/gateways-and-cloud-providers
title: "Gateways and cloud providers: how a gateway can remove prompt caching
  without an error"
content: >-
  Companies


  A gateway can remove Claude Code prompt caching without an error. Anthropic's
  documentation, read on 4 October 2026, describes three things a gateway can do
  with the cache markers that Claude Code sends: forward them unchanged, reject
  the request with a 400 error, or remove them while returning success. In the
  third case the entire conversation history is billed as uncached input on
  every turn, and the request still succeeds. The cache itself is stored
  wherever the model is served: with Anthropic, with your cloud provider, or
  wherever a gateway forwards your requests. To test a connection, run /usage
  and read the Prompt cache (main) line, which works on every provider and
  gateway.


  Published October 4, 2026. Editorial.


  ## Key takeaways


  *   Anthropic's documentation, read on 4 October 2026, says a gateway that
  removes the cache markers while returning success causes the entire
  conversation history to be billed as uncached input on every turn.

  *   When a gateway rejects a marked request with a 400 error naming
  cache_control, Claude Code sends it again with the marker moved to the last
  conversation message, and the conversation stays cached.

  *   The one-hour cache lifetime is unavailable through the Claude apps
  gateway, and through another gateway it needs the anthropic-beta header
  forwarded unchanged.

  *   On Amazon Bedrock, support for prompt caching, the minimum size of a
  stored start and the availability of the one-hour lifetime all vary by model,
  by Anthropic's documentation.

  *   Claude Code disables MCP tool search by default when ANTHROPIC_BASE_URL is
  not an Anthropic address, so adding a tool definition during a session, or
  removing one on purpose, resets the cache there.


  Claude Code is Anthropic's coding tool. You type a request in a terminal (the
  text window where you type commands), and an AI model reads files, runs
  commands and edits code for you. The work is counted in tokens, which are the
  pieces of text the model processes. The model keeps nothing between requests,
  so Claude Code sends the whole conversation again with every request.


  The prompt cache is a store of request text that the service running the model
  has already processed. When a new request begins with exactly the same text,
  the service reuses its earlier work and bills that text at a lower price. A
  company can connect Claude Code through a cloud provider, or through a server
  of its own that passes requests on, instead of connecting to Anthropic
  directly. This page explains what each of those connections does to the cache,
  and how to test yours. It is part of the guide to the Claude Code prompt
  cache.


  ## What a gateway is and why companies run one


  A proxy is a server that receives requests and passes them on to another
  server. A gateway, in Anthropic's words, is "a proxy your organization runs
  between Claude Code and a model provider" [2]. Claude Code sends its requests
  to the gateway, and the gateway forwards them with a credential that the
  organisation holds. The model provider can be Anthropic's API or a cloud
  provider such as Amazon Bedrock, Google Cloud's Agent Platform or Microsoft
  Foundry [2].


  Anthropic's documentation, read on 4 October 2026, lists what a gateway gives
  an organisation: one place to keep credentials, track usage, enforce budgets,
  log requests and change provider [3]. The same page states the cost of that
  arrangement. Claude Code adds capabilities with each release, and a gateway
  that does not forward them stops the matching features from working, so the
  gateway has to be kept up to date [3]. The cache markers and the header
  described below are among the things a gateway has to forward [1].


  There are two kinds of gateway in Anthropic's pages. The Claude apps gateway
  is Anthropic's own product, included in the `claude` program. Any other
  gateway is a separate product, and Anthropic says it does not endorse,
  maintain or audit those products [2].


  ## Where the cache is stored for each connection


  The cache is kept on the server side, in the infrastructure that serves your
  model. Anthropic's documentation, read on 4 October 2026, gives the place for
  each way of connecting [1]. The third column is our list of what to check,
  taken from the sections below.


  | How you connect | Where the cache is stored | What to check |

  | --- | --- | --- |

  | API key, Claude subscription or Claude Platform on AWS | Anthropic's
  infrastructure | The `Prompt cache (main)` line in `/usage` |

  | Amazon Bedrock | Your cloud provider's serving infrastructure | Whether your
  model and region support caching, and whether cache token counts are above
  zero |

  | Google Cloud's Agent Platform | Your cloud provider's serving infrastructure
  | Whether your model is from the Claude 4.5 generation or later, because
  earlier models load all MCP tools at the start |

  | Microsoft Foundry | Azure infrastructure for deployments hosted on Azure,
  Anthropic's infrastructure for deployments hosted on Anthropic | Deployments
  hosted on Azure reject tool search |

  | Claude apps gateway | Anthropic's caching page has no separate line for it.
  Its rule for a gateway is: wherever your requests are forwarded | The lifetime
  is five minutes. The one-hour lifetime is unavailable |

  | Another gateway or a custom `ANTHROPIC_BASE_URL` | Wherever your requests
  are forwarded | Whether the gateway forwards the cache markers and the
  `anthropic-beta` header unchanged |


  `ANTHROPIC_BASE_URL` is an environment variable, a named value that a program
  reads from the shell when it starts. It replaces the address that Claude Code
  sends requests to, so that they go through a proxy or gateway [7]. For the
  last row, Anthropic adds: "whether caching works depends on the gateway" [1].


  ## Three things a gateway can do with the cache markers


  Claude Code tells the API which part of a request to store by sending markers
  with it. Anthropic calls them `cache_control` markers [1]. An LLM gateway is
  Anthropic's term for a gateway product that an organisation already runs [3].
  The three cases below apply when requests pass through an LLM gateway, a
  custom `ANTHROPIC_BASE_URL`, or an override of a cloud provider's address such
  as `ANTHROPIC_BEDROCK_BASE_URL`. They come from Anthropic's documentation,
  read on 4 October 2026 [1].


  One detail comes first. During a conversation, Claude Code adds blocks of
  system context, such as notices that a file changed, and marks each block for
  caching. In the list below, "the block" means that added block.


  1.   **The gateway forwards the markers unchanged.** The block and your
  conversation are cached in the same way as at the provider's own address.

  2.   **The gateway rejects the marked request with a `400` error that names
  `cache_control`.** A `400` is an error code that a server returns when it
  refuses a request. Claude Code sends the request again, with the marker moved
  from the block to your last conversation message, and keeps it there for the
  rest of the conversation. The block is billed as uncached input. Your
  conversation stays cached.

  3.   **The gateway removes the markers and returns success.** Your entire
  conversation history is billed as uncached input on every turn. A gateway that
  converts the system content from blocks to a plain string removes the marker
  in the same way.


  The third case is the one in this page's title. The gateway returns success,
  so the request does not fail. The change appears in the token counts and in
  the cost.


  ## What the third case costs: an invented example


  This example is invented, and the arithmetic is ours. It uses the list prices
  for Claude Sonnet 5.5 on Anthropic's pricing page, read on 4 October 2026: $2
  per million tokens of base input and $0.20 per million tokens read from the
  cache [9].


  Suppose a conversation holds 100,000 tokens, and a task sends it 50 times. To
  keep the sum simple, the example holds the conversation at that size and
  ignores the new text of each turn.


  *   With the cache working, each request reads 100,000 tokens at $0.20 per
  million, which is $0.02. Fifty requests cost $1.00.

  *   With the markers removed, each request is billed as uncached input:
  100,000 tokens at $2 per million, which is $0.20. Fifty requests cost $10.00.


  The second total is 10 times the first, which matches the pricing page's
  multiplier of 0.1 for a cache read on most models [9]. On Claude Opus 5.5 the
  multiplier is 0.05, and on Claude Fable 5.1 it is 0.025 [9]. These are API
  list prices. When developers connect with a gateway credential, usage is
  billed to the organisation's provider account at API rates [2]. The exception
  is a session that sets only `ANTHROPIC_BASE_URL`, with no gateway credential.
  There a saved claude.ai login stays the active credential, so the
  subscription's usage limits and billing apply [2]. Anthropic's pages give no
  statement of how cache tokens count against those limits.


  ## How to test a gateway


  The test uses the fact that the cache counts come back in every API response.
  Anthropic's documentation, read on 4 October 2026, describes three places to
  read them.


  **The /usage line.** Start a session through the gateway, send a few messages,
  and run `/usage`, the command that shows the token counts of the session. With
  Claude Code v2.1.251 or later, the Session block has a line named `Prompt
  cache (main)`. Anthropic says its counts come from the cache token fields in
  the API's responses, "so the line works on every provider and gateway" [10].
  With a working cache, the line shows the share of input tokens that came from
  the cache. When no response has reported cache tokens, the line ends with `no
  prompt caching reported by the API` [10].


  **The status line data.** The status line is a bar at the bottom of Claude
  Code that runs a script you choose. Its data has a field named
  `caching_observed`. Anthropic's definition: `false` means "prompt caching is
  off, or your provider or gateway doesn't report it" [11].


  **The raw counts.** The two counts are `cache_creation_input_tokens`, the
  tokens written to the cache, and `cache_read_input_tokens`, the tokens read
  from it. Anthropic's API reference says that if both are 0, the prompt was not
  cached [8].


  Run the same test once with a direct connection, if your organisation allows
  one, so that you have a number to compare. The page on how to check your cache
  hit rate reads the `/usage` line field by field.


  The second case in the list above is harder to see. The conversation stays
  cached, so the share from cache stays high, and only the added block is billed
  as uncached input. Anthropic's pages name no separate indicator for it.


  ## The anthropic-beta header and the one-hour lifetime


  A stored copy is kept for a limited time without use. That time is called the
  time to live, or TTL. Anthropic offers a five-minute TTL and a one-hour TTL
  [1]. The page on how long the cache lasts explains the defaults and the
  settings.


  The one-hour lifetime depends on a header. A header is a labelled line of
  information sent with a request, outside its main content. Anthropic's
  documentation, read on 4 October 2026, says that through a gateway you set
  with `ANTHROPIC_BASE_URL`, part of the one-hour request is sent in the
  `anthropic-beta` header, "so configure the gateway to forward that header
  unchanged" [1].


  The Claude apps gateway handles the header for you, with one limit. It
  delivers the `anthropic-beta` values to every provider it forwards to. For
  Amazon Bedrock, which ignores the header, it moves the values into a field of
  the request body [4]. Standard prompt caching is available through it: the
  gateway forwards the cache markers to every provider [4]. The one-hour
  lifetime is unavailable. Anthropic's reason is that some of the providers the
  gateway can forward to do not support it, so Claude Code leaves it out on
  these sessions and caching uses the five-minute lifetime [4].


  Two more differences apply on a Claude apps gateway session. Anthropic lists
  "first-party-only optimizations such as global cache scope and token-efficient
  tools" as unavailable there [4]. And a change of effort level, the setting for
  how much the model reasons, loses the cache there on the models that otherwise
  keep it [1]. The page on switching model or effort level in the middle of a
  task covers that rule.


  ## Amazon Bedrock, Google Cloud and Microsoft Foundry


  At the provider's own address, Anthropic's documentation says, Amazon Bedrock
  and its Mantle endpoint (a second Bedrock address that serves Claude models in
  Anthropic's own request format [5]), Google Cloud's Agent Platform and
  Microsoft Foundry cache the added system context block in the same way the
  Claude API does [1]. Three differences remain, all from Anthropic's pages read
  on 4 October 2026.


  **The default lifetime is five minutes.** Claude Code requests one hour by
  default only on a Claude subscription within plan usage [1]. With a cloud
  provider, you request the hour yourself. Anthropic's Bedrock page shows
  `ENABLE_PROMPT_CACHING_1H=1` as an optional line for this, and notes that the
  one-hour lifetime is billed at a higher rate [5].


  **Amazon Bedrock varies by model and region.** Support for prompt caching, the
  minimum size of a stored start, and the availability of the one-hour lifetime
  all vary by model [1]. The Bedrock page adds that prompt caching may not be
  available in all Amazon Bedrock regions, and gives the test: if cache token
  counts stay at zero, check the supported models, regions and limits in the
  Amazon Bedrock documentation [5]. The API reference says the same for the
  per-model minimums, the failure behaviour and the names of the usage fields:
  on Bedrock, AWS's own documentation applies [8].


  **Sharing inside one organisation differs.** Caches are always isolated
  between organisations. On the Claude API, Claude Platform on AWS and Microsoft
  Foundry, they are also isolated between the workspaces inside one
  organisation. Bedrock and Google Cloud use isolation at the level of the
  organisation only. Anthropic's API reference advises reviewing your caching
  strategy if you use several workspaces [8].


  ## Tool search, and why its absence makes the cache easier to reset


  An MCP server is a program that gives Claude extra tools. Each tool has a
  definition, and the definitions are part of the system prompt, the first part
  of every request. Tool search is the Claude Code feature that holds back the
  definitions of MCP tools until Claude needs them [6].


  Tool search matters for the cache. Anthropic's documentation, read on 4
  October 2026, says that with tool search holding back the tools, Claude Code
  keeps the tool list from the first request for the whole conversation, so a
  server that connects or disconnects during a session does not disturb anything
  already cached. Without it, adding a tool definition invalidates the cache,
  which means it makes the stored copy unusable for the next request. Removing
  one on purpose does the same [1].


  Tool search is off by default on three kinds of setup:


  *   **A custom `ANTHROPIC_BASE_URL` that is not an Anthropic address.** Claude
  Code disables tool search there, "since most proxies don't forward
  `tool_reference` blocks" [6]. If your proxy forwards them, set
  `ENABLE_TOOL_SEARCH=true`. With that value, requests fail on proxies that do
  not support those blocks [6].

  *   **Google Cloud's Agent Platform with a model earlier than the Claude 4.5
  generation.** Claude Code loads all MCP tools at the start, and
  `ENABLE_TOOL_SEARCH=true` does not change this. Before v2.1.221, Claude Code
  disabled tool search for all models on that platform unless you set the
  variable [6].

  *   **A Microsoft Foundry deployment hosted on Azure.** The deployment rejects
  tool search on its side. Claude Code detects the rejection and loads the MCP
  tools at the start [6].


  On these setups, connect your MCP servers before the first message and leave
  them unchanged during the task. The page on the nine actions that reset the
  cache has the table of which server changes reset the cache, and the related
  guide's page on MCP servers and command-line tools covers how many servers to
  run.


  ## What CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS removes


  Some gateways reject requests that contain fields they do not know.
  Anthropic's list of environment variables, read on 4 October 2026, names the
  errors: `Unexpected value(s)` for the `anthropic-beta` header, or `Extra
  inputs are not permitted`. Setting `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` to
  `1` removes the pre-release `anthropic-beta` headers and the fields that go
  with them [7].


  Anthropic says to use the variable when a proxy gateway rejects requests with
  those errors [7]. It also changes three things about the cache, by Anthropic's
  pages [1] [6]:


  *   The system context block that Claude Code adds during a conversation is
  sent uncached.

  *   Tool search stays off, and setting `ENABLE_TOOL_SEARCH` yourself does not
  turn it back on. An organisation can keep tool search on through managed
  settings (settings that administrators enforce for all users) on Claude Code
  v2.1.227 or later.

  *   A change of effort level loses the cache on Opus 5.5, Sonnet 5.5 and Fable
  5.1, the models that otherwise keep it.


  Use the variable when a gateway needs it, and know that these three changes
  come with it.


  ## Our position


  Test the cache before a team starts to use a gateway. A gateway that removes
  the markers returns success. The token counts show the change. Reveneau
  recommends three checks for any company connection: the `/usage` line shows a
  share of input tokens from cache, the gateway forwards `cache_control` markers
  and the `anthropic-beta` header unchanged, and MCP servers are connected
  before the first message wherever tool search is off.


  Reveneau is an AI software development consultancy, and all of its code is
  written by AI, so token use is a running cost of every Reveneau build.
  Reveneau is independent of Anthropic. Every statement about Claude Code,
  gateways and cloud providers on this page is Anthropic's own, read on 4
  October 2026, and the sums marked as ours are arithmetic on Anthropic's list
  prices.


  ## Common questions


  ### Can a gateway turn off Claude Code prompt caching without showing an
  error?


  Yes. A gateway can remove the cache markers that Claude Code sends and still
  return a successful response. Anthropic's documentation, read on 4 October
  2026, says that in this case your entire conversation history is billed as
  uncached input on every turn. A gateway that converts the system content from
  blocks to a plain string removes the marker in the same way. The request
  succeeds, so the token counts are where the change appears.


  ### What is an LLM gateway in Claude Code?


  An LLM gateway is Anthropic's term for a gateway product that an organisation
  already runs. A gateway, in Anthropic's documentation read on 4 October 2026,
  is a proxy your organisation runs between Claude Code and a model provider.
  Claude Code sends requests to the gateway, and the gateway forwards them with
  a credential the organisation holds. Anthropic says it does not endorse,
  maintain or audit gateway products other than its own Claude apps gateway.


  ### How do I test whether my gateway keeps prompt caching?


  To test whether your gateway keeps prompt caching, start a session through it,
  send a few messages, and run `/usage`. With Claude Code v2.1.251 or later, the
  Session block has a `Prompt cache (main)` line. Anthropic's documentation,
  read on 4 October 2026, says the line works on every provider and gateway.
  When no response has reported cache tokens, it ends with `no prompt caching
  reported by the API`.


  ### What happens when a gateway rejects a request that contains cache markers?


  When a gateway rejects a marked request with a `400` error that names
  `cache_control`, Claude Code sends the request again with the marker moved.
  Anthropic's documentation, read on 4 October 2026, says the marker moves from
  the added system context block to your last conversation message and stays
  there for the rest of the conversation. That block is then billed as uncached
  input, and your conversation stays cached.


  ### What is a cache_control marker?


  A `cache_control` marker is a label that Claude Code sends with a request to
  tell the API which part to store in the prompt cache. Anthropic's
  documentation, read on 4 October 2026, says that what stays cached through a
  gateway depends on how the gateway handles these markers. A gateway can
  forward them unchanged, reject the request with a `400` error, or remove them
  while returning success.


  ### Does the Claude apps gateway support prompt caching?


  Yes. The Claude apps gateway supports standard prompt caching. Anthropic's
  documentation, read on 4 October 2026, says the gateway forwards the cache
  markers to every provider it sends requests to. The one-hour lifetime is
  unavailable through it, so caching uses the five-minute lifetime. Anthropic
  also lists first-party-only optimizations, such as global cache scope and
  token-efficient tools, as unavailable on these sessions.


  ### Does the one-hour cache lifetime work through a gateway?


  The one-hour cache lifetime works through a gateway you set with
  `ANTHROPIC_BASE_URL` only if the gateway forwards the `anthropic-beta` header
  unchanged. Anthropic's documentation, read on 4 October 2026, says part of the
  one-hour request is sent in that header. Through the Claude apps gateway the
  one-hour lifetime is unavailable, because some of the providers that gateway
  can forward to do not support it.


  ### Is prompt caching different on Amazon Bedrock?


  Yes. On Amazon Bedrock, support for prompt caching, the minimum size of a
  stored start, and the availability of the one-hour lifetime all vary by model.
  That is Anthropic's documentation, read on 4 October 2026. The Bedrock page
  adds that prompt caching may not be available in all regions. If cache token
  counts stay at zero, Anthropic says to check the supported models, regions and
  limits in the Amazon Bedrock documentation.


  ### Why can an MCP server change reset the cache behind a gateway?


  An MCP server change can reset the cache behind a gateway because tool search
  is off there by default. Anthropic's documentation, read on 4 October 2026,
  says Claude Code disables tool search when `ANTHROPIC_BASE_URL` is not an
  Anthropic address, since most proxies do not forward `tool_reference` blocks.
  Without tool search, adding a tool definition during a session invalidates the
  cache, and removing one on purpose does the same.


  ### What does CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS do to prompt caching?


  Setting `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` to `1` changes three things
  about prompt caching, by Anthropic's documentation read on 4 October 2026. The
  system context block that Claude Code adds during a conversation is sent
  uncached. Tool search stays off, so MCP tool definitions load at the start.
  And a change of effort level loses the cache on Opus 5.5, Sonnet 5.5 and Fable
  5.1, the models that otherwise keep it.


  ### Who is billed for Claude Code requests sent through a gateway?


  When developers connect through a gateway with a gateway credential, the usage
  is billed to the organisation's provider account at API rates. That is
  Anthropic's documentation, read on 4 October 2026. This is why a gateway that
  removes cache markers raises cost directly: in our invented example with
  Claude Sonnet 5.5 list prices, 50 requests of 100,000 tokens cost $10.00 as
  uncached input and $1.00 as cache reads.


  ### Are prompt caches shared between workspaces on Bedrock and Google Cloud?


  On Bedrock and Google Cloud, prompt caches are isolated at the level of the
  organisation only. Anthropic's API reference, read on 4 October 2026, says
  caches are always isolated between organisations. On the Claude API, Claude
  Platform on AWS and Microsoft Foundry, they are also isolated between the
  workspaces inside one organisation. Anthropic advises reviewing your caching
  strategy if you use several workspaces.


  ## References


  *   Anthropic, How Claude Code uses prompt caching (code.claude.com), read 4
  October 2026

  *   Anthropic, Run Claude Code through a gateway (code.claude.com), read 4
  October 2026

  *   Anthropic, Other LLM gateways (code.claude.com), read 4 October 2026

  *   Anthropic, Claude apps gateway (code.claude.com), read 4 October 2026

  *   Anthropic, Claude Code on Amazon Bedrock (code.claude.com), read 4 October
  2026

  *   Anthropic, Connect Claude Code to tools via MCP (code.claude.com), read 4
  October 2026

  *   Anthropic, Environment variables (code.claude.com), read 4 October 2026

  *   Anthropic, Prompt caching (platform.claude.com), read 4 October 2026

  *   Anthropic, Pricing (platform.claude.com), read 4 October 2026

  *   Anthropic, Manage costs effectively (code.claude.com), read 4 October 2026

  *   Anthropic, Customize your status line (code.claude.com), read 4 October
  2026


  ## Related reading
