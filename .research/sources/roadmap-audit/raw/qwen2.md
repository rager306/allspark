url: https://docs.modelstudio.console.alibabacloud.com/en/model-studio/context-cache
title: Context Cache
content: |-
  Inference requests for large models often contain overlapping input, such as in a multi-turn conversation or a series of questions about the same book. Context Cache reduces redundant computation by caching the common prefix of these requests. This improves response speed and lowers usage costs without affecting response quality.

  To support different scenarios, context cache offers two modes. Choose a mode based on your requirements for convenience, determinism, and cost:

  *   Explicit cache: A mode that you enable manually. You create a cache for specific content to ensure a deterministic hit within its 5-minute validity period. Tokens used to create the cache are typically billed at 125% of the standard input token price, while subsequent cache hits are typically billed at only 10% of that price. For specific pricing, see Billing.
  *   Implicit cache: This automatic mode requires no extra configuration and cannot be disabled, ideal for scenarios that prioritize convenience. The system **automatically identifies** and caches the **common prefix** of requests, but the **hit probability is not guaranteed**. The portion of the input served from the cache is typically billed at 20% of the standard input token price. For specific pricing, see Billing.

  | **Item** | **Explicit cache** | **Implicit cache** |
  | --- | --- | --- |
  | Impact on response quality | No | No |
  | Billing for cache creation tokens | Typically 125% of the standard input token price | 100% of the standard input token price |
  | Billing for cached input tokens | Typically 10% of the standard input token price (see Billing) | Typically 20% of the standard input token price (see Billing) |
  | Minimum tokens for caching | 1024 | 1024 |
  | Cache validity period | 5 minutes (resets on hit) | Indeterminate. The system periodically clears old, unused cache data. |

  Although the minimum token count is the same for both modes, the meaning differs: explicit cache is actively created and managed by the user; implicit cache is automatically created and managed by the system. Reaching 1,024 tokens only means the request meets the technical conditions for a cache hit, but does not guarantee an actual hit.

  ## Explicit cache

  Unlike an implicit cache, an explicit cache requires explicit creation and incurs overhead, but delivers a higher cache hit ratio and lower access latency.

  ### How it works

  Add a`"cache_control": {"type": "ephemeral"}` marker to the messages array. The system then searches backward from each`cache_control` marker and examines up to 20 preceding`content` blocks to find a cache hit.

  > A single request supports up to four cache markers.

  *   **Cache miss**If a cache miss occurs, the system creates a new cache block from the content between the start of the messages array and the`cache_control` marker. The new cache block has a validity period of 5 minutes.
  > The system creates the cache after the model generates a response. Wait for the creation request to complete before trying to hit that cache.

  > A cache block contains at least 1,024 tokens.

  *   **Cache hit**If a cache hit occurs, the system selects the longest matching prefix and resets the validity period of the corresponding cache block to 5 minutes.

  The following example demonstrates how this works:

  1.   **Send the first request**: Send a system message containing text A (more than 1,024 tokens), and add a cache marker:

  ```
  [{"role": "system", "content": [{"type": "text", "text": A, "cache_control": {"type": "ephemeral"}}]}]
  ```

  The system creates the first cache block, calling it cache block A. 2. **Send the second request:** Send a request with the following structure:

  ```
  [
      {"role": "system", "content": A},
      <Other messages>
      {"role": "user","content": [{"type": "text", "text": B, "cache_control": {"type": "ephemeral"}}]}
  ]
  ```

  *   If there are 20 or fewer "Other messages," the request hits cache block A, resetting its validity period to 5 minutes. The system also creates a new cache block based on A, the other messages, and B.
  *   If there are more than 20 "Other messages," the request misses cache block A. The system still creates a new cache block based on the full context (A, the other messages, and B).

  ### Supported models

  > The following models are available in the International deployment scope.

  Qwen Max: qwen3.8-max, qwen3.8-max-0902, qwen3.7-max, qwen3.7-max-2026-05-20, qwen3.7-max-2026-06-08 Qwen Open-source: qwen3.8-2.4t-a95b, qwen3.8-27b Qwen Plus: qwen3.7-plus, qwen3.7-plus-2026-05-26, qwen3.6-plus, qwen3.5-plus, qwen3.5-plus-2026-04-20, qwen-plus Qwen Flash: qwen3.8-flash, qwen3.7-flash, qwen3.7-flash-2026-07-15, qwen3.6-flash, qwen3.5-flash, qwen-flash Qwen Coder: qwen3-coder-flash Qwen VL: qwen3-vl-plus

  ### Quick start

  The following examples demonstrate the cache block creation and cache hit mechanisms for OpenAI compatible, DashScope, and Anthropic compatible protocols.

  ```
  from openai import OpenAI
  import os

  client = OpenAI(
      # If the environment variable is not set, replace the following line with: api_key="sk-xxx"
      api_key=os.getenv("DASHSCOPE_API_KEY"),
      # If you use a model in China (Beijing), replace the base_url with: https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1
      base_url="https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
  )

  # Mock code repository content. The minimum cacheable prompt length is 1,024 tokens.
  long_text_content = "<Your Code Here>" * 400

  # Function to make a request
  def get_completion(user_input):
      messages = [
          {
              "role": "system",
              "content": [
                  {
                      "type": "text",
                      "text": long_text_content,
                      # Place the cache_control marker here. This creates a cache block containing all content from the start of the messages array up to this point.
                      "cache_control": {"type": "ephemeral"},
                  }
              ],
          },
          # The user's question is different for each request.
          {
              "role": "user",
              "content": user_input,
          },
      ]
      completion = client.chat.completions.create(
          # Select a model that supports explicit cache.
          model="qwen3.8-max",
          messages=messages,
      )
      return completion

  # First request
  first_completion = get_completion("What is the content of this code?")
  print(f"First request cache creation tokens: {first_completion.usage.prompt_tokens_details.cache_creation_input_tokens}")
  print(f"First request cached tokens: {first_completion.usage.prompt_tokens_details.cached_tokens}")
  print("=" * 20)
  # Second request. The code content is the same, but the question is different.
  second_completion = get_completion("How can this code be optimized?")
  print(f"Second request cache creation tokens: {second_completion.usage.prompt_tokens_details.cache_creation_input_tokens}")
  print(f"Second request cached tokens: {second_completion.usage.prompt_tokens_details.cached_tokens}")
  ```

  Adding the `cache_control` marker enables explicit cache for the mock code repository content. For subsequent requests that query this content, the system reuses the cache block, eliminating recomputation. This makes requests that hit the cache faster and cheaper than the initial cache-creation request.

  ```
  First request cache creation tokens: 1605
  First request cached tokens: 0
  ====================
  Second request cache creation tokens: 0
  Second request cached tokens: 1605
  ```

  ### Fine-grained control with multiple cache markers

  In complex scenarios, a prompt often consists of multiple parts with different reuse frequencies. You can use multiple cache markers to achieve fine-grained control.For example, the prompt for an intelligent customer service agent typically includes:

  *   **System persona:** Highly stable and rarely changes.
  *   **External knowledge:** This is obtained from the knowledge base or through tool queries and may not change during a single conversation.
  *   **Conversation history:** Grows dynamically.
  *   **Current question:** Different for each request.

  If you cache the entire prompt as a single unit, any minor change, such as an update to the external knowledge, can cause a cache miss.You can add up to four cache markers in a request to create separate cache blocks for different parts of the prompt. This improves the cache hit ratio and enables fine-grained control.

  ### Billing

  Explicit cache only affects how input tokens are billed. The rules are as follows:

  *   **Cache creation**: Content used to create a new cache is billed at 125% of the standard input token price. If the content for a new cache includes an existing cache as a prefix, only the incremental portion is billed for cache creation (i.e., the number of new cache tokens minus the number of existing cache tokens).For example, if you have an existing 1,200-token cache (Cache A) and you use a new request to cache 1,500 tokens of content (Content AB), the first 1,200 tokens are billed as a cache hit at 10% of the standard price. The new 300 tokens are billed for cache creation at 125% of the standard price.
  > The `cache_creation_input_tokens` parameter specifies the number of tokens used for cache creation.

  *   **Cache hit**: Billed at 10% of the standard input token price.
  > The `cached_tokens` parameter specifies the number of cached tokens.

  *   **Other tokens**: Tokens that are neither a cache hit nor used for cache creation are billed at the standard input token price.
  *   **Exception**: The explicit cache hit price for qwen3.8-max, qwen3.8-max-0902, qwen3.8-flash, and qwen3.8-2.4t-a95b is not 10% of the standard input token price. For specific pricing, see the Model Studio console.

  ### Cacheable content

  Only the following message types in the `messages`array support adding cache markers:

  *   System message
  *   User message
  > When creating a cache with the `qwen3-vl-plus` model, you can place the `cache_control` marker after multimodal content or text. Its position does not affect how the entire user message is cached.

  *   Assistant message
  *   Tool message (the result of tool execution)

  For example, for a system message, you must change the `content`field to an array and add the `cache_control`field:

  ```
  {
    "role": "system",
    "content": [
      {
        "type": "text",
        "text": "<your specified prompt>",
        "cache_control": {
          "type": "ephemeral"
        }
      }
    ]
  }
  ```

  This structure also applies to other message types in the `messages`array.

  ### Cache limitations

  *   The minimum cacheable prompt length is **1,024** tokens.
  *   The cache uses a backward prefix matching strategy. A cache miss occurs if the matching content and the message with the `cache_control` marker are separated by more than 20 content blocks.
  *   The `type` can only be set to `ephemeral`, which creates a cache with a 5-minute validity period.
  *   A single request supports up to four cache markers.
  > If more than four cache markers are provided, only the last four take effect.

  ### Function Calling cache optimization

  A tool definition is serialized into a JSON string for caching. To prevent cache invalidation, this definition must be identical across all requests. Note the following:

  *   **Consistent tool order**: The order of tools in the `tools` array must be consistent across all requests.
  *   **Consistent field order**: The order of JSON fields within the same tool must be consistent across all requests.
  *   **Consistent field structure**: Do not omit or add fields, even if they are empty or optional.

  ### Optimizing message structure for parallel tool calls

  When you use parallel tool calls, the model returns multiple `tool_calls` in a single response. If you send each tool result as a separate `tool` message, the number of content blocks in the `messages` array grows quickly. When more than 20 content blocks separate the `cache_control` marker from earlier content, the backward lookback window cannot reach those earlier blocks, causing a cache miss.To resolve this, merge consecutive same-role tool messages into a single `tool` message with multiple content blocks before sending the next request. This reduces the total content block count and keeps the content you want to cache within the 20-block lookback window.**Before optimization (separate tool messages — lower cache hit rate)**:

  ```
  # After the model returns parallel tool_calls, send each result as a separate message
  messages.append(assistant_message)  # assistant message containing parallel tool_calls
  # Each tool result is its own message — increases content block count by N
  messages.append({"role": "tool", "tool_call_id": "call_1", "content": "result_1"})
  messages.append({"role": "tool", "tool_call_id": "call_2", "content": "result_2"})
  ```

  **After optimization (merged tool message — higher cache hit rate)**:

  ```
  # After the model returns parallel tool_calls, merge all results into one message
  messages.append(assistant_message)  # assistant message containing parallel tool_calls
  # Merge all tool results into a single message with multiple content blocks
  messages.append({
      "role": "tool",
      "tool_call_id": "call_1",
      "content": [
          {"type": "text", "text": "result_1"},
          {"type": "text", "text": "result_2", "tool_call_id": "call_2"},
      ],
  })
  ```

  To further improve the cache hit rate, place `cache_control` markers at stable positions in the `messages` array (for example, on the system message or on other infrequently changing content). A single request supports up to four cache markers.

  ### Usage examples

  Querying a long text

  ```
  from openai import OpenAI
  import os

  client = OpenAI(
      api_key=os.getenv("DASHSCOPE_API_KEY"),
      # This is the base_url for the Singapore region. When making a call, replace {WorkspaceId} with your actual WorkspaceId. URLs vary by region.
      base_url="https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
  )

  # Mock code repository content
  long_text_content = "<Your Code Here>" * 400

  # Function to send a request
  def get_completion(user_input):
      messages = [
          {
              "role": "system",
              "content": [
                  {
                      "type": "text",
                      "text": long_text_content,
                      # Place the cache_control marker here to create a cache from the start of the prompt to the end of this content object (the mock code repository content).
                      "cache_control": {"type": "ephemeral"},
                  }
              ],
          },
          {
              "role": "user",
              "content": user_input,
          },
      ]
      completion = client.chat.completions.create(
          # Select a model that supports explicit cache
          model="qwen3.8-max",
          messages=messages,
      )
      return completion

  # First request
  first_completion = get_completion("What is the content of this code?")
  created_cache_tokens = first_completion.usage.prompt_tokens_details.cache_creation_input_tokens
  print(f"First request - Cache creation tokens: {created_cache_tokens}")
  hit_cached_tokens = first_completion.usage.prompt_tokens_details.cached_tokens
  print(f"First request - Cache hit tokens: {hit_cached_tokens}")
  print(f"First request - Uncached tokens: {first_completion.usage.prompt_tokens-created_cache_tokens-hit_cached_tokens}")
  print("=" * 20)
  # Second request with the same code content but a different question
  second_completion = get_completion("What are some possible optimizations for this code?")
  created_cache_tokens = second_completion.usage.prompt_tokens_details.cache_creation_input_tokens
  print(f"Second request - Cache creation tokens: {created_cache_tokens}")
  hit_cached_tokens = second_completion.usage.prompt_tokens_details.cached_tokens
  print(f"Second request - Cache hit tokens: {hit_cached_tokens}")
  print(f"Second request - Uncached tokens: {second_completion.usage.prompt_tokens-created_cache_tokens-hit_cached_tokens}")
  ```

  This example caches the code repository content as a prefix. Subsequent requests ask different questions about the same repository.

  ```
  First request - Cache creation tokens: 1605
  First request - Cache hit tokens: 0
  First request - Uncached tokens: 13
  ====================
  Second request - Cache creation tokens: 0
  Second request - Cache hit tokens: 1605
  Second request - Uncached tokens: 15
  ```

  > To ensure model performance, the system appends a few internal tokens. These tokens are billed at the standard input price. For more information, see the FAQ.

  Caching tools for function calling

  When caching system messages for Function Calling, the `tools` parameter is cached as part of the system message. Ensure that the tool definition is identical for every request (including the tool order, field order, and field structure), and add a `cache_control` flag to the last `content` in `messages`.The following shows the complete flow: the first request creates the cache, and the second request hits the cache.

  ```
  from openai import OpenAI
  import os

  client = OpenAI(
      api_key=os.getenv("DASHSCOPE_API_KEY"),
      base_url="https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
  )

  # Mock code repository content, ensuring it exceeds the minimum 1,024-token threshold for explicit cache.
  long_text_content = "<Your Code Here>" * 400

  # Tool definition: Ensure it is identical for every request (tool order, field order, and field structure).
  tools = [
      {
          "type": "function",
          "function": {
              "name": "get_weather",
              "description": "Get the current weather information for a specified city.",
              "parameters": {
                  "type": "object",
                  "properties": {
                      "city": {
                          "type": "string",
                          "description": "The city name, e.g., Beijing, Shanghai, or New York."
                      },
                      "unit": {
                          "type": "string",
                          "description": "The temperature unit, 'celsius' or 'fahrenheit'. Defaults to 'celsius'.",
                          "enum": ["celsius", "fahrenheit"]
                      }
                  },
                  "required": ["city"],
                  "additionalProperties": False
              },
              "strict": True
          }
      },
      {
          "type": "function",
          "function": {
              "name": "get_current_time",
              "description": "Get the current date and time for a specified time zone.",
              "parameters": {
                  "type": "object",
                  "properties": {
                      "timezone": {
                          "type": "string",
                          "description": "IANA time zone name, e.g., 'Asia/Shanghai' or 'America/New_York'. Defaults to 'Asia/Shanghai'."
                      }
                  },
                  "required": [],
                  "additionalProperties": False
              },
              "strict": True
          }
      },
      {
          "type": "function",
          "function": {
              "name": "convert_currency",
              "description": "Convert currency amounts based on real-time exchange rates.",
              "parameters": {
                  "type": "object",
                  "properties": {
                      "from_currency": {
                          "type": "string",
                          "description": "The ISO 4217 code of the source currency, e.g., CNY, USD, or EUR."
                      },
                      "to_currency": {
                          "type": "string",
                          "description": "The ISO 4217 code of the target currency."
                      },
                      "amount": {
                          "type": "number",
                          "description": "The amount to be converted."
                      }
                  },
                  "required": ["from_currency", "to_currency", "amount"],
                  "additionalProperties": False
              },
              "strict": True
          }
      }
  ]

  def get_completion(user_input, messages=None):
      if messages is None:
          messages = [
              {
                  "role": "system",
                  "content": [
                      {
                          "type": "text",
                          "text": long_text_content,
                          # Place the cache_control marker here. This creates a cache block with all content from the start of the messages array to the current content object.
                          # The cache_control marker must be on the 'content' of a message, not on 'tools'.
                          "cache_control": {"type": "ephemeral"},
                      }
                  ],
              }
          ]

      messages.append({"role": "user", "content": user_input})

      completion = client.chat.completions.create(
          # Select a model that supports explicit cache
          model="qwen3.7-plus",
          messages=messages,
          tools=tools,
          # Disable thinking mode
          extra_body={"enable_thinking": False},
      )
      return completion

  # First request: Create cache
  print("=== First request (Create cache) ===")
  first_completion = get_completion("What's the weather like in Beijing now?")
  usage = first_completion.usage
  print(f"Prompt Tokens: {usage.prompt_tokens}")
  print(f"Cache creation tokens: {usage.prompt_tokens_details.cache_creation_input_tokens}")
  print(f"Cache hit tokens: {usage.prompt_tokens_details.cached_tokens}")
  print(f"Model selected tool(s): {[t.function.name for t in first_completion.choices[0].message.tool_calls or []]}")
  print()

  # Second request: Hits the cache with the same system message but a different question
  print("=== Second request (Cache hit) ===")
  messages = [
      {
          "role": "system",
          "content": [
              {
                  "type": "text",
                  "text": long_text_content,
                  "cache_control": {"type": "ephemeral"},
              }
          ],
      }
  ]
  second_completion = get_completion("What's the weather like in Shanghai now?", messages=messages)
  usage = second_completion.usage
  print(f"Prompt Tokens: {usage.prompt_tokens}")
  print(f"Cache creation tokens: {usage.prompt_tokens_details.cache_creation_input_tokens}")
  print(f"Cache hit tokens: {usage.prompt_tokens_details.cached_tokens}")
  print(f"Model selected tool(s): {[t.function.name for t in second_completion.choices[0].message.tool_calls or []]}")
  ```

  Running the code produces output similar to the following:

  ```
  === First request (Create cache) ===
   Prompt Tokens: 2174
   Cache creation tokens: 2156
   Cache hit tokens: 0
   Model selected tool(s): ['get_weather']

   === Second request (Cache hit) ===
   Prompt Tokens: 2174
   Cache creation tokens: 0
   Cache hit tokens: 2156
   Model selected tool(s): ['get_weather']
  ```

  Continuous multi-turn conversation

  In a typical multi-turn conversation scenario, add a cache marker to the last content object in the messages array for each request. From the second turn onward, each request hits and refreshes the cache from the previous turn while creating a new cache block for the current turn.

  ```
  from openai import OpenAI
  import os

  client = OpenAI(
      api_key=os.getenv("DASHSCOPE_API_KEY"),
      # This is the base_url for the Singapore region. When making a call, replace {WorkspaceId} with your actual WorkspaceId. URLs vary by region.
      base_url="https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
  )

  system_prompt = "You are a witty person." * 400
  messages = [{"role": "system", "content": system_prompt}]

  def get_completion(messages):
      completion = client.chat.completions.create(
          model="qwen3.8-max",
          messages=messages,
      )
      return completion

  while True:
      user_input = input("User: ")
      messages.append({"role": "user", "content": [{"type": "text", "text": user_input, "cache_control": {"type": "ephemeral"}}]})
      completion = get_completion(messages)
      print(f"[AI Response] {completion.choices[0].message.content}")
      messages.append(completion.choices[0].message)
      created_cache_tokens = completion.usage.prompt_tokens_details.cache_creation_input_tokens
      hit_cached_tokens = completion.usage.prompt_tokens_details.cached_tokens
      uncached_tokens = completion.usage.prompt_tokens - created_cache_tokens - hit_cached_tokens
      print(f"[Cache Info] Cache creation tokens: {created_cache_tokens}")
      print(f"[Cache Info] Cache hit tokens: {hit_cached_tokens}")
      print(f"[Cache Info] Uncached tokens: {uncached_tokens}")
  ```

  Run the code to start a conversation with the large language model. Each subsequent question hits the cache created in the previous turn.

  ## Implicit cache

  ### Supported models

  *   **Omni-modal models**
      *   Qwen Omni: `qwen3.8-omni-flash`

  *   **Text generation models**
      *   Qwen Max: qwen3.8-max, qwen3.8-max-0902, qwen3.7-max, qwen3.7-max-2026-05-20, qwen3.7-max-2026-06-08, qwen-max
      *   Qwen Plus: qwen3.7-plus, qwen3.7-plus-2026-05-26, qwen-plus
      *   Qwen Flash: qwen3.8-flash, qwen3.7-flash, qwen3.7-flash-2026-07-15, qwen-flash
      *   Qwen Coder: qwen3-coder-flash
      *   Qwen Character: qwen-plus-character, qwen-flash-character
      *   Qwen Open-source: qwen3.8-2.4t-a95b, qwen3.8-27b
      *   DeepSeek: deepseek-v4.1-flash, deepseek-v4-pro
      *   Kimi: kimi-k3, kimi-k2.7-code, kimi-k2.6, kimi-k2.5
      *   GLM: glm-5.3, glm-5.2, glm-5.2-fast-preview, glm-5.1, glm-5
      *   MiniMax: MiniMax-M2.5

  *   **Visual understanding models**
      *   Qwen VL: qwen3-vl-plus

  ### How it works

  The implicit cache feature is automatically enabled when a request is sent to a supported model. The system works as follows:

  1.   **Search**: After receiving a request, the system uses **prefix matching** to check the cache for a common prefix of the content in the request's `messages` array.
  2.   **Decision**:
      *   If a cache hit occurs, the system uses the cached result for the rest of the inference.
      *   If a cache miss occurs, the system processes the request normally and stores the prefix of the prompt in the cache for future requests.

  > The system periodically clears cached data that has not been used for a long time. The Context Cache hit probability is not 100%. A cache miss may occur even if the request context is identical. The system determines the specific hit probability.

  Reaching the minimum token count does not guarantee a cache hit. Whether a hit occurs is also affected by factors such as cache generation status, cache validity period, and system scheduling. Refer to the cache hit token count in the API response for actual results.

  ### Increase the cache hit probability

  An implicit cache hit occurs when the **prefixes** of different requests have duplicate content. To increase the hit probability, **place duplicate content at the beginning of a prompt and unique content at the end.**

  *   **Text model**: For example, assume the system has cached "ABCD". A request for "ABE" may hit the "AB" part, but a request for "BCD" will not.
  *   **Visual understanding model:**
      *   To ask multiple questions about **the same image or video**, place the image or video before the text.
      *   To ask the same question about **different images or videos**, place the text before the image or video.

  ### Billing

  No additional fees are charged for enabling the implicit cache mode.When a request hits the cache, the input tokens from the cache hit are billed as `cached_token`. The discount rate for these tokens varies by model. Input tokens that do not hit the cache are billed as standard `input_token`. Output tokens are billed at the original price.

  *   For models other than deepseek-v4.1-flash, deepseek-v4-pro, qwen3.8-max, qwen3.8-max-0902, qwen3.8-flash, qwen3.8-2.4t-a95b, and qwen3.8-omni-flash: The unit price of `cached_token` is **20%** of the `input_token` unit price.
  *   qwen3.8-omni-flash: Cache-hit input tokens are billed at a fixed unit price. For regional prices, see Model pricing.
  *   deepseek-v4-pro: The unit price of `cached_token` is not 20% of the `input_token` unit price. For specific pricing, see the Model Studio console.
  *   deepseek-v4.1-flash: The unit price of `cached_token` is **10%** of the `input_token` unit price.
  *   qwen3.8-max, qwen3.8-max-0902, qwen3.8-flash, and qwen3.8-2.4t-a95b: The unit price of `cached_token` is not 20% of the `input_token` unit price. For specific pricing, see the Model Studio console.
  *   GLM (deployed on Alibaba Cloud Model Studio): glm-5.3, **25%** for glm-5.2 and glm-5.2-fast-preview, and **20%** for all other GLM series models.

  Example: A request contains 10,000 input tokens, and 5,000 of them result in a cache hit. The cost is calculated as follows:

  *   Non-cache-hit tokens (5,000): Billed at 100% of the unit price.
  *   Cache-hit tokens (5,000): Billed at 20% of the unit price.

  The total input cost is 60% of the cost in a non-cache mode: (50% × 100%) + (50% × 20%) = 60%.

  You can get the number of cache-hit tokens from the `cached_tokens` attribute in the response.

  > Calls made using the OpenAI-compatible - Batch (file input) method are not eligible for cache discounts.

  ### Cache hit examples

  When you call a model using an OpenAI-compatible method and trigger the implicit cache, the response indicates the number of tokens that hit the cache in the `usage.prompt_tokens_details.cached_tokens` field. This value is part of `usage.prompt_tokens`.

  ```
  {
      "choices": [
          {
              "message": {
                  "role": "assistant",
                  "content": "I am a large-scale language model developed by Alibaba Cloud. My name is Qwen."
              },
              "finish_reason": "stop",
              "index": 0,
              "logprobs": null
          }
      ],
      "object": "chat.completion",
      "usage": {
          "prompt_tokens": 3019,
          "completion_tokens": 104,
          "total_tokens": 3123,
          "prompt_tokens_details": {
              "cached_tokens": 2048
          }
      },
      "created": 1735120033,
      "system_fingerprint": null,
      "model": "qwen-plus",
      "id": "chatcmpl-6ada9ed2-7f33-9de2-8bb0-78bd4035025a"
  }
  ```

  ### Use cases

  If your requests share a common prefix, the context cache can significantly improve inference speed, lower inference cost, and reduce first-packet latency. This feature is particularly useful in the following use cases:

  1.   **Long-text question answering**Use this pattern when you send multiple requests about the same long text, such as a novel, textbook, or legal document.**First request messages**

  ```
  messages = [{"role": "system","content": "You are a language teacher who can help students with reading comprehension."},
            {"role": "user","content": "
  ```

  **Subsequent request messages array**

  ```
  messages = [{"role": "system","content": "You are a language arts teacher. You can help students with reading comprehension."},
            {"role": "user","content": "<Article content> Please analyze the third paragraph of this text."}]
  ```

  Although the questions are different, they are all based on the same article. The same system prompt and article content constitute a large amount of repetitive prefix information, which has a high probability of a cache hit. 2. **Code auto-completion**In code auto-completion scenarios, the model uses the surrounding code as context to generate subsequent code. As you write, the beginning of the code file remains the same. The context cache can store this prefix to accelerate code completions. 3. **Multi-turn conversation**For a multi-turn conversation, you append each turn to the messages array. This ensures that each new request shares a common prefix with the previous turns, increasing the likelihood of a cache hit.**First turn messages**

  ```
  messages=[{"role": "system","content": "You are a helpful assistant."},
            {"role": "user","content": "Who are you?"}]
  ```

  **Second turn messages**

  ```
  messages=[{"role": "system","content": "You are a helpful assistant."},
            {"role": "user","content": "Who are you?"},
            {"role": "assistant","content": "I am Qwen, developed by Alibaba Cloud."},
            {"role": "user","content": "What can you do?"}]
  ```

  As the conversation grows, the benefits of caching for inference speed and cost become more significant. 4. **Role-playing or few-shot learning**In role-playing or few-shot learning scenarios, you often include extensive instructions in the prompt to guide the model's output format. This creates a large, shared prefix across multiple requests.For example, when instructing the model to act as a marketing expert, the system prompt contains extensive text. The following are two example requests:

  ```
  system_prompt = """You are an experienced marketing expert. Provide detailed marketing suggestions for different products in the following format:

  1. Target audience: xxx

  2. Main selling points: xxx

  3. Marketing channels: xxx
  ...
  12. Long-term development strategy: xxx

  Ensure your suggestions are specific, actionable, and highly relevant to the product features."""

  # The user message for the first request asks about a smartwatch.
  messages_1=[
    {"role": "system", "content": system_prompt},
    {"role": "user", "content": "Provide marketing suggestions for a newly launched smartwatch."}
  ]

  # The user message for the second request asks about a laptop. Because the system_prompt is the same, a cache hit is highly likely.
  messages_2=[
    {"role": "system", "content": system_prompt},
    {"role": "user", "content": "Provide marketing suggestions for a newly launched laptop."}
  ]
  ```

  With the context cache, the system can respond faster because the lengthy system prompt is cached, even when you frequently change the product in your request (for example, from a smartwatch to a laptop). 5. **Video understanding**In video understanding scenarios, if you ask multiple questions about the same video, placing `video` before `text` increases the probability of a cache hit. If you ask the same question about different videos, placing `text` before `video` increases the probability of a cache hit. The following example shows two requests for the same video:

  ```
  # The user message for the first request asks about the content of this video.
  messages1 = [
      {"role":"system","content":[{"text": "You are a helpful assistant."}]},
      {"role": "user",
          "content": [
              {"video": "https://help-static-aliyun-doc.aliyuncs.com/file-manage-files/zh-CN/20250328/eepdcq/phase_change_480p.mov"},
              {"text": "What is the content of this video?"}
          ]
      }
  ]

  # For the second request about the same video, placing the video before the text increases the likelihood of a cache hit.
  messages2 = [
      {"role":"system","content":[{"text": "You are a helpful assistant."}]},
      {"role": "user",
          "content": [
              {"video": "https://help-static-aliyun-doc.aliyuncs.com/file-manage-files/zh-CN/20250328/eepdcq/phase_change_480p.mov"},
              {"text": "Describe the series of events in the video. Output the start time (start_time), end time (end_time), and event (event) in JSON format. Do not include the ```json``` code block."}
          ]
      }
  ]
  ```

  ## FAQ

  ### Q: How long is the context cache retained (validity period)?

  A: The validity period of the context cache depends on the cache type:

  *   **Explicit cache**: The validity period is 5 minutes, and each cache hit resets it to another 5 minutes. If the cache block is not hit within 5 minutes, the system automatically clears it.
  *   **Implicit cache**: Managed automatically by the system with no fixed validity period. The system periodically clears cache data that has not been used for a long time.

  ### Q: How do I disable implicit cache?

  A: You cannot disable it. The implicit cache is enabled for all applicable model requests because it does not affect response quality. When a cache hit occurs, it reduces costs and improves response speed.

  ### Q: Why did my explicit cache miss?

  A: A cache miss can occur for the following reasons:

  *   The system clears the cache block if it is not hit within its 5-minute validity period.
  *   If the interval between the last `content` and an existing cache block is greater than 20 `content` blocks, a cache hit will not occur. We recommend that you create a new cache block.

  ### Q: Does a cache hit reset its validity?

  A: Yes. Each hit resets the cache block's validity period to 5 minutes.

  ### Q: Is explicit cache shared between accounts?

  A: No. Both implicit cache and explicit cache data is isolated at the account level.

  ### Q:Is explicit cache shared across models?

  A: No. Cache data is isolated between models.

  ### Q: Why doesn't the`input_tokens`in`usage`equal the sum of`cache_creation_input_tokens`and`cached_tokens`?

  A: To ensure model output quality, the backend service appends a small number of tokens (typically 10 or fewer) to your prompt. These tokens are placed after the `cache_control` marker, so they are not counted for cache creation or reading, but are included in the total `input_tokens`.
