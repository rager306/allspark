url: https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool
title: Tool search tool
content: >-
  Scale to hundreds or thousands of tools by letting Claude search your tool
  catalog and load only the tools it needs.


  The tool search tool lets Claude work with hundreds or thousands of tools by
  discovering and loading them on demand. Instead of loading all tool
  definitions into the context window up front, Claude searches your tool
  catalog (including tool names, descriptions, argument names, and argument
  descriptions) and loads only the tools it needs.


  Loading every tool definition up front causes two problems as a tool library
  grows:


  *   **Context bloat:** A typical multiserver setup (GitHub, Slack, Sentry,
  Grafana, and Splunk) can consume ~55k tokens in definitions before Claude does
  any work. Tool search typically reduces this by over 85 percent, loading only
  the 3–5 tools Claude needs for a given request.

  *   **Tool selection accuracy:** Claude's ability to pick the right tool
  degrades once you exceed 30–50 available tools. Because tool search loads only
  a focused set of relevant tools on demand, selection accuracy stays high even
  across thousands of tools.


  For the models that support tool search, see Model compatibility.


  Tool search runs as a server-side tool, but you can also implement your own
  client-side tool search. See Custom tool search implementation for details.


  ## Model compatibility


  Both tool search variants are available on the following models:


  | Model | Tool versions |

  | --- | --- |

  | Claude Fable 5.1 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Mythos 5.1 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Fable 5 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Mythos 5 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Opus 5.5 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Opus 5 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Sonnet 5.5 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Sonnet 5 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Haiku 5.5 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Opus 4.8 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Opus 4.7 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Opus 4.6 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Sonnet 4.6 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Opus 4.5 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Sonnet 4.5 () (deprecated) | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |

  | Claude Haiku 4.5 () | `tool_search_tool_regex_20251119`,
  `tool_search_tool_bm25_20251119` |


  Claude Opus 4.1 and earlier models don't support the tool search tool.


  ## How tool search works


  There are two tool search variants:


  *   **Regex** (`tool_search_tool_regex_20251119`): Claude constructs regex
  patterns to search for tools.

  *   **BM25** (`tool_search_tool_bm25_20251119`): Claude uses natural language
  queries to search for tools.


  When you enable the tool search tool:


  1.   You include a tool search tool (for example,
  `tool_search_tool_regex_20251119` or `tool_search_tool_bm25_20251119`) in your
  `tools` list.

  2.   You provide every tool definition in the `tools` array and set
  `defer_loading: true` on the tools that shouldn't load up front. At least one
  tool, normally the tool search tool itself, must stay non-deferred.

  3.   Initially, Claude's context contains only the tool search tool and any
  non-deferred tools.

  4.   When Claude needs additional tools, it searches using a tool search tool.

  5.   The API runs the search and returns the matching tools as
  `tool_reference` blocks (up to 5 by default; Claude can set a `limit` in its
  search input).

  6.   The API automatically expands these references into full tool
  definitions.

  7.   Claude selects from the discovered tools and calls them.


  ## Quick start


  The following example includes the tool search tool and two deferred tools:


  Claude searches the catalog, discovers `get_weather`, and calls it. The
  response ends with `stop_reason: "tool_use"`. Execute the discovered tool and
  return a `tool_result` as in Handle tool calls. Response format shows the
  blocks you get back and what to send next.


  ## Tool definition


  The tool search tool has two variants:


  ### Deferred tool loading


  Mark tools for on-demand loading by adding `defer_loading: true`:


  `defer_loading` controls what enters the context window, not what you send in
  the request:


  *   You still send every tool's full definition in the `tools` array on every
  request, including the deferred ones. The API needs them server-side to run
  the search and expand `tool_reference` blocks.

  *   Tools without `defer_loading` load into context immediately.

  *   Tools with `defer_loading: true` load only when Claude discovers them
  through search.

  *   Never set `defer_loading: true` on the tool search tool itself.

  *   Keep your 3–5 most frequently used tools non-deferred so Claude can call
  them without searching first.


  The computer use and browser use toolsets (`computer_toolset_20260801` and
  `browser_toolset_20260801`) take `defer_loading` per member tool inside the
  entry's `configs` object, not on the entry itself; a request that sets it at
  the entry level is rejected. Because a toolset defers and expands as a unit,
  `defer_loading` must resolve to the same value on every enabled member, and
  when Claude discovers the toolset through search, every enabled member loads
  at once. See Client toolsets for the `configs` format.


  Both tool search variants (`regex` and `bm25`) search tool names,
  descriptions, argument names, and argument descriptions.


  Internally, the API excludes deferred tools from the system-prompt prefix.
  When Claude discovers a deferred tool through tool search, the API appends a
  `tool_reference` block inline in the conversation, then expands it into the
  full tool definition before passing it to Claude. The prefix is untouched, so
  prompt caching is preserved. The grammar for strict mode (the rules that
  constrain tool-call output to match your schemas) builds from the full
  toolset, so `defer_loading` and strict mode compose without grammar
  recompilation.


  ## Response format


  When Claude uses the tool search tool, the response includes the following
  block types:


  ### Understanding the response


  *   **`server_tool_use`:** Claude's call to the tool search tool. The search
  runs on Anthropic's servers. Never return a `tool_result` for its
  `srvtoolu_...` ID. The `input` holds the search (`pattern` for the regex
  variant, `query` for BM25) and may include an optional `limit`, an integer
  from 1 to 10,000 that caps how many matching tools the search returns
  (default: 5).

  *   **`tool_search_tool_result`:** the search results, in a nested
  `tool_search_tool_search_result` object. Keep it in the message history as is.

  *   **`tool_references`:** an array of `tool_reference` objects pointing to
  discovered tools. The API expands these for Claude. You never expand them
  yourself.

  *   **`tool_use`:** Claude's call to a discovered tool. Execute it and return
  a `tool_result` exactly as in standard tool use.


  The API automatically expands `tool_reference` blocks into full tool
  definitions before showing them to Claude. You don't need to handle this
  expansion yourself, as long as you provide all matching tool definitions in
  the `tools` parameter.


  ### Continuing the conversation


  On the next request, pass the assistant's content back unchanged, including
  the `server_tool_use` and `tool_search_tool_result` blocks. Add your
  `tool_result` for the discovered tool in a user message, and send the same
  `tools` array: the search tool plus every deferred definition. Don't return a
  `tool_result` for the `srvtoolu_...` ID: the API rejects the request. The API
  expands `tool_reference` blocks throughout the conversation history, so Claude
  can reuse discovered tools in later turns without re-searching. A search that
  matches nothing returns a `tool_search_tool_search_result` with an empty
  `tool_references` array, not an error.


  ## MCP integration


  If your tools come from MCP servers through the MCP connector, you don't set
  `defer_loading` on individual tool definitions. Instead, set it once on the
  `mcp_toolset` entry's `default_config` for the whole server, or per tool in
  its `configs`. See MCP toolset configuration.


  ## Custom tool search implementation


  You can implement your own tool search logic (for example, using embeddings or
  semantic search) by returning `tool_reference` blocks from a custom tool. When
  Claude calls your custom search tool, return a standard `tool_result` with
  `tool_reference` blocks in the content array:


  Every tool referenced must have a corresponding tool definition in the
  top-level `tools` parameter, normally with `defer_loading: true`. This lets
  you use search methods the built-in variants don't provide, such as
  embedding-based retrieval, and the API expands the returned `tool_reference`
  blocks the same way.


  For a complete example using embeddings, see the tool search with embeddings
  recipe.


  ## Error handling


  ### HTTP errors (400 status)


  These errors prevent the API from processing the request:


  **All tools deferred:**


  **Missing tool definition:**


  ### Tool result errors (200 status)


  When a tool search operation fails during execution, the API returns a 200
  response with the error in the body:


  The `error_code` field has four possible values:


  *   `invalid_tool_input`: the search input was invalid, for example a
  malformed regex pattern or a pattern over the 200-character limit

  *   `unavailable`: the search couldn't run, for example because it timed out
  or the service was unavailable

  *   `too_many_requests`: rate limit exceeded for tool search operations

  *   `execution_time_exceeded`: the search exceeded its execution time limit


  ### Common mistakes


  ## Prompt caching


  To learn how `defer_loading` preserves prompt caching, see Tool use with
  prompt caching.


  A tool with `defer_loading: true` can't also carry `cache_control`: the API
  returns a 400. Put the cache breakpoint on a non-deferred tool.


  ## Streaming


  With streaming enabled, you'll receive tool search events as part of the
  stream:


  ## Batch requests


  You can include the tool search tool in the Messages Batches API.


  ## Limits and best practices


  ### Limits


  *   **Maximum deferred tools:** 10,000 tools with `defer_loading: true` per
  request

  *   **Search results:** each search returns up to 5 matching tools by default;
  Claude can set `limit` in its search input to any integer from 1 to 10,000

  *   **Pattern and query length:** maximum 200 characters for regex patterns
  and 500 characters for BM25 queries

  *   **Model support:** see Model compatibility


  ### When to use tool search


  Use tool search when any of the following apply:


  *   You have 10 or more tools available.

  *   Your tool definitions consume more than 10k tokens.

  *   Tool selection accuracy drops as your toolset grows.

  *   You aggregate multiple MCP servers (200+ tools).

  *   Your tool library grows over time.


  Standard tool calling, without tool search, is a better fit when you have
  fewer than 10 tools, every tool is used in every request, or your tool
  definitions are small (less than 100 tokens total).


  ### Optimization tips


  *   Keep your 3–5 most frequently used tools non-deferred.

  *   Write clear, descriptive tool names and descriptions.

  *   Use consistent namespacing in tool names: prefix by service or resource
  (for example, `github_`, `slack_`) so one search matches the whole group.

  *   Use keywords in descriptions that match how users describe tasks.

  *   Add a system prompt section describing available tool categories: "You can
  search for tools to interact with Slack, GitHub, and Jira."

  *   Monitor which tools Claude discovers to refine your descriptions.


  ## Usage


  Tool search isn't metered as a separate server tool. The response's
  `usage.server_tool_use` object has no tool search field, and the tool
  definitions that search loads into context count as input tokens like any
  other tool definition.


  ## Next steps


  Let Claude store and retrieve information across conversations by implementing
  the memory tool's file operations in your application.


  Directory of Anthropic-provided tools and reference for optional tool
  definition properties.


  Configure MCP toolsets with deferred loading.


  Cache tool definitions across turns and understand what invalidates your
  cache.


  Specify tool schemas, write effective descriptions, and control when Claude
  calls your tools.
