url: https://docs.qwencloud.com/developer-guides/run-and-scale/context-cache
title: Context cache - QwenCloud
content: |-
  Context cache caches the common prefix of overlapping requests to avoid redundant computation. This improves response speed and reduces costs without affecting response quality.Context cache provides three modes to suit different scenarios. Choose based on your requirements for convenience, certainty, and cost:

  *   Explicit cache: A mode that you enable manually. Create a cache for specific content to achieve a guaranteed hit. The cache is valid for 5 minutes. Tokens used to create the cache are typically billed at 125% of the standard input token price. Subsequent hits are typically billed at 10% of the standard price. For specific pricing, see Billing.
  *   Implicit cache: An automatic mode that requires no extra configuration and cannot be disabled. It is suitable for general scenarios where convenience is the priority. The system **automatically identifies** and caches the **common prefix** of requests, but the **hit probability is not guaranteed**. The cached portion that is hit is typically billed at 20% of the standard input token price. For specific pricing, see Billing.
  *   Session cache: Designed for multi-turn conversation scenarios that use the Responses API. By adding `x-dashscope-session-cache: enable` to the request header, the server automatically caches the conversation context. The billing rules are the same as for explicit cache: tokens used to create the cache are billed at 125% of the standard input token price, and hits are billed at 10%.

  | **Item** | **Explicit cache** | **Implicit cache** | **Session cache** |
  | --- | --- | --- | --- |
  | Impact on response quality | No impact | No impact | No impact |
  | Billing for tokens used to create the cache | Typically 125% of the standard input token price | 100% of the standard input token price | Typically 125% of the standard input token price |
  | Billing for cached input tokens that are hit | Typically 10% of the standard input token price (see Billing) | Typically 20% of the standard input token price (see Billing) | Typically 10% of the standard input token price (see Billing) |
  | Minimum tokens for caching | 1024 | 1024 | 1024 |
  | Cache validity period | 5 minutes (resets on hit) | Indeterminate. The system periodically clears old, unused cache data. | 5 minutes (resets on hit) |

  Although the minimum token count is the same for both modes, the meaning differs: explicit cache is actively created and managed by the user; implicit cache is automatically created and managed by the system. Reaching 1,024 tokens only means the request meets the technical conditions for a cache hit, but does not guarantee an actual hit.

  ## Explicit cache

  Compared to implicit cache, explicit cache requires explicit creation and incurs overhead, but delivers a higher cache hit rate and lower access latency.

  ### Supported models

  | Category | Models |
  | --- | --- |
  | Qwen Max | `qwen3.8-max`, `qwen3.8-max-0902`, `qwen3.7-max`, `qwen3.7-max-2026-06-08`, `qwen3.7-max-2026-05-20` |
  | Qwen Plus | `qwen3.7-plus`, `qwen3.7-plus-2026-05-26`, `qwen3.6-plus`, `qwen3.5-plus`, `qwen3.5-plus-2026-04-20`, `qwen-plus` |
  | Qwen Flash | `qwen3.8-flash`, `qwen3.7-flash`, `qwen3.7-flash-2026-07-15`, `qwen3.6-flash`, `qwen3.5-flash`, `qwen-flash` |
  | Qwen Open-source | `qwen3.8-2.4t-a95b`, `qwen3.8-27b` |
  | Qwen Coder | `qwen3-coder-flash` |
  | Qwen VL | `qwen3-vl-plus` |

  ### Usage

  Add the `"cache_control": {"type": "ephemeral"}` marker in the `messages` array. The system then searches backward from each `cache_control` marker and examines up to 20 preceding `content` blocks to find a cache hit against existing cache blocks.

  *   **Cache miss**The system creates a new cache block using the content from the beginning of the `messages` array to the `cache_control` marker. The cache block is valid for 5 minutes.
  *   **Cache hit**The system selects the longest matching prefix and resets the validity period of the corresponding cache block to 5 minutes.

  Example:

  1

  2

  ### Getting started

  The following examples show how cache blocks are created and hit.

  Subsequent requests about the same code repository reuse this cache block, resulting in faster responses and lower costs.

  ```
  First request cache creation tokens: 1605
  First request cached hit tokens: 0
  ====================
  Second request cache creation tokens: 0
  Second request cached hit tokens: 1605
  ```

  ### Use multiple cache markers for fine-grained control

  In complex scenarios, a prompt often consists of multiple parts with different reuse frequencies. You can use multiple cache markers to achieve fine-grained control.For example, the prompt for a smart customer service agent typically includes:

  *   **System settings:** Highly stable and almost never changes.
  *   **External knowledge:** Semi-stable. It is retrieved from a knowledge base or by calling a tool and may remain unchanged during a continuous conversation.
  *   **Conversation history:** Grows dynamically.
  *   **Current question:** Different each time.

  If you cache the entire prompt as a single unit, any minor change, such as a change in external knowledge, invalidates the cache.You can set up to four cache markers in a request to create separate cache blocks for different parts of the prompt. This improves the hit rate and provides fine-grained control.

  ### Billing

  Explicit cache affects only the pricing of input tokens. The rules are as follows:

  *   **Cache creation**: Newly created cache content is billed at 125% of the standard input price. If the new cache content contains an existing cache as a prefix, only the incremental portion is billed for creation (new cache tokens minus existing cache tokens).For example, if you have an existing cache A with 1200 tokens and a new request needs to cache content AB with 1500 tokens, the first 1200 tokens are billed as a cache hit (10% of the standard price), and the new 300 tokens are billed for cache creation (125% of the standard price).
  *   **Cache hit**: Billed at 10% of the standard input price.
  *   **Other tokens**: Tokens that do not match any cache and are not used to create a cache are billed at the standard price.
  *   **Exception**: The explicit cache hit price for `qwen3.8-max`, `qwen3.8-flash`, and `qwen3.8-2.4t-a95b` is not 10% of the standard input token price. For specific pricing, see the Model Marketplace.

  ### Cacheable content

  Only the following message types in the `messages` array support adding cache markers:

  *   System message
  *   User message
  *   Assistant message
  *   Tool message (the result after a tool is executed)

  For example, for a system message, change the `content` field to an array and add the `cache_control` field:

  ```
  {
    "role": "system",
    "content": [
      {
        "type": "text",
        "text": "<Your specified prompt>",
        "cache_control": {
          "type": "ephemeral"
        }
      }
    ]
  }
  ```

  This structure also applies to other message types in the `messages` array.

  ### Cache limitations

  *   The minimum cacheable prompt length is **1024** tokens.
  *   The cache uses a backward prefix matching strategy. A cache miss occurs if the matching content and the message with the `cache_control` marker are separated by more than 20 content blocks.
  *   The `type` can only be set to `ephemeral`, which creates a cache with a 5-minute validity period.
  *   A single request can have a maximum of 4 cache markers.

  ### Optimize message structure for parallel tool calls

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

  Different questions for a long text

  ```
  from openai import OpenAI
  import os

  client = OpenAI(
    api_key=os.getenv("DASHSCOPE_API_KEY"),
    base_url="https://maas.qwencloudapi.com/compatible-mode/v1",
  )

  # Mock code repository content
  long_text_content = "<Your Code Here>" * 400

  # Function to send the request
  def get_completion(user_input):
    messages = [
      {
        "role": "system",
        "content": [
          {
            "type": "text",
            "text": long_text_content,
            # Place the cache_control marker here to create a cache from the beginning of the prompt to the end of this content block (the mock code repository content).
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
      model="qwen3.7-max",
      messages=messages,
    )
    return completion

  # First request
  first_completion = get_completion("What is the content of this code?")
  created_cache_tokens = first_completion.usage.prompt_tokens_details.cache_creation_input_tokens
  print(f"First request cache creation tokens: {created_cache_tokens}")
  hit_cached_tokens = first_completion.usage.prompt_tokens_details.cached_tokens
  print(f"First request cached hit tokens: {hit_cached_tokens}")
  print(f"First request tokens not hit and not cached: {first_completion.usage.prompt_tokens-created_cache_tokens-hit_cached_tokens}")
  print("=" * 20)
  # Second request, the code content is the same, only the question is changed.
  second_completion = get_completion("What are some areas for optimization in this code?")
  created_cache_tokens = second_completion.usage.prompt_tokens_details.cache_creation_input_tokens
  print(f"Second request cache creation tokens: {created_cache_tokens}")
  hit_cached_tokens = second_completion.usage.prompt_tokens_details.cached_tokens
  print(f"Second request cached hit tokens: {hit_cached_tokens}")
  print(f"Second request tokens not hit and not cached: {second_completion.usage.prompt_tokens-created_cache_tokens-hit_cached_tokens}")
  ```

  This example caches the code repository content as a prefix. Subsequent requests ask different questions about the repository.

  ```
  First request cache creation tokens: 1605
  First request cached hit tokens: 0
  First request tokens not hit and not cached: 13
  ====================
  Second request cache creation tokens: 0
  Second request cached hit tokens: 1605
  Second request tokens not hit and not cached: 15
  ```

  Continuous multi-turn conversation

  In a daily chat multi-turn conversation scenario, you can add a cache marker to the last content of the messages array in each request. Starting from the second turn of the conversation, each request will hit and refresh the cache block created in the previous turn, and will also create a new cache block.

  ```
  from openai import OpenAI
  import os

  client = OpenAI(
    api_key=os.getenv("DASHSCOPE_API_KEY"),
    base_url="https://maas.qwencloudapi.com/compatible-mode/v1",
  )

  system_prompt = "You are a witty person." * 400
  messages = [{"role": "system", "content": system_prompt}]

  def get_completion(messages):
    completion = client.chat.completions.create(
      model="qwen3.7-max",
      messages=messages,
    )
    return completion

  while True:
    user_input = input("Please enter: ")
    messages.append({"role": "user", "content": [{"type": "text", "text": user_input, "cache_control": {"type": "ephemeral"}}]})
    completion = get_completion(messages)
    print(f"[AI Response] {completion.choices[0].message.content}")
    messages.append(completion.choices[0].message)
    created_cache_tokens = completion.usage.prompt_tokens_details.cache_creation_input_tokens
    hit_cached_tokens = completion.usage.prompt_tokens_details.cached_tokens
    uncached_tokens = completion.usage.prompt_tokens - created_cache_tokens - hit_cached_tokens
    print(f"[Cache Info] Cache creation tokens: {created_cache_tokens}")
    print(f"[Cache Info] Cached hit tokens: {hit_cached_tokens}")
    print(f"[Cache Info] Tokens not hit and not cached: {uncached_tokens}")
  ```

  Run the code above and enter questions to communicate with the model. Each question will hit the cache block created in the previous turn.

  ## Implicit cache

  ### Supported models

  **Omni-modal models**: `qwen3.8-omni-flash`**Text generation models**

  | Category | Models |
  | --- | --- |
  | Qwen Max | `qwen3.8-max`, `qwen3.8-max-0902`, `qwen3.7-max`, `qwen3.7-max-2026-06-08`, `qwen-max` |
  | Qwen Plus | `qwen3.7-plus`, `qwen3.7-plus-2026-05-26`, `qwen-plus` |
  | Qwen Open-source | `qwen3.8-2.4t-a95b`, `qwen3.8-27b` |
  | Qwen Flash | `qwen3.8-flash`, `qwen3.7-flash`, `qwen3.7-flash-2026-07-15`, `qwen-flash` |
  | Qwen Coder | `qwen3-coder-flash` |
  | Qwen Character | `qwen-plus-character`, `qwen-flash-character`, `qwen-plus-character-ja` |
  | DeepSeek | `deepseek-v4.1-flash`, `deepseek-v4-pro-0813`, `deepseek-v4-pro`, `deepseek-v4-flash-0731` |
  | GLM | `ZHIPU/GLM-5.3`, `ZHIPU/GLM-5.2`, `glm-5.3`, `glm-5.2`, `glm-5.2-fast-preview`, `glm-5.1` |
  | Kimi | `kimi-k3`, `kimi/kimi-k3`, `kimi-k2.7-code` |

  **Visual understanding models**: `qwen3-vl-plus`

  Reaching the minimum token count does not guarantee a cache hit. Whether a hit occurs is also affected by factors such as cache generation status, cache validity period, and system scheduling. Refer to the cache hit token count in the API response for actual results.

  ### How it works

  When you send a request to a model that supports implicit cache, the implicit cache feature is automatically enabled. The system works as follows:

  1.   **Search**: Upon receiving a request, the system checks the cache for a common prefix of the content in the request's `messages` array based on the **prefix matching** principle.
  2.   **Evaluate**:
      *   If the cache is hit, the system directly uses the cached result for the subsequent part of the inference process.
      *   If the cache is missed, the system processes the request normally and stores the prefix of the current prompt in the cache for subsequent requests to use.

  ### Increase the cache hit probability

  An implicit cache hit occurs when the **prefixes** of different requests have duplicate content. To increase the hit probability, **place static content at the beginning of the prompt and variable content at the end.**

  *   **Text-only:** If the system has cached "ABCD", a request for "ABE" can match the "AB" prefix, while a request for "BCD" will not match any cache.
  *   **Visual understanding:**
      *   When asking multiple questions about the **same image or video**: Place the image or video before the text to increase the hit rate.
      *   When asking the same question about **different images or videos**: Place the text before the image or video to increase the hit rate.

  ### Billing

  No additional fees are charged for enabling the implicit cache mode.When a request hits the cache, the matched input tokens are billed as `cached_tokens` at **20%** of the `input_token` unit price. Input tokens that do not hit the cache are billed at the standard `input_token` price. Output tokens are billed at the standard price.

  Example: A request contains 10,000 input tokens, of which 5,000 hit the cache:

  *   Non-cache-hit tokens (5,000): Billed at 100% of the unit price
  *   Cache-hit tokens (5,000): Billed at 20% of the unit price

  The total input cost is 60% of the cost in a non-cache mode: (50% × 100%) + (50% × 20%) = 60%.You can retrieve the number of hit cache tokens from the `cached_tokens` attribute of the returned result.

  ### Cache hit examples

  #### Text generation

  Check hit cache tokens in `usage.prompt_tokens_details.cached_tokens` (part of `usage.prompt_tokens` for OpenAI compatible, or `usage.input_tokens` for DashScope), or `usage.cache_read_input_tokens` for Anthropic compatible.

  #### Visual understanding

  Check hit cache tokens in `usage.prompt_tokens_details.cached_tokens` (part of `usage.prompt_tokens` for OpenAI compatible), `usage.cached_tokens` (part of `usage.input_tokens` for DashScope), or `usage.cache_read_input_tokens` for Anthropic compatible.

  ### Typical scenarios

  1.   **Q&A based on long text**This is applicable to scenarios that require multiple requests about a fixed long text, such as a novel, textbook, or legal document.**Message array for the first request**

  ```
  messages = [{"role": "system","content": "You are a language teacher and you can help students with reading comprehension."},
    {"role": "user","content": "<Article content> What is the author's main idea in this text?"}]
  ```

  **Message array for subsequent requests**

  ```
  messages = [{"role": "system","content": "You are a language teacher and you can help students with reading comprehension."},
    {"role": "user","content": "<Article content> Please analyze the third paragraph of this text."}]
  ```

  Although the questions differ, they reference the same article. Because the system prompt and article content remain unchanged, each request shares a large overlapping prefix, increasing the likelihood of a cache hit.

  1.   **Code auto-completion**In code auto-completion scenarios, the model auto-completes code based on the existing context. As the user continues coding, the prefix portion of the code remains unchanged. Context cache can cache the preceding code to improve completion speed.
  2.   **Multi-turn conversation**In a multi-turn conversation, the conversation history from all previous turns is included in the messages array. Each turn's request therefore shares the same prefix as the previous turn, resulting in a high probability of a cache hit.**Message array for the first turn of conversation**

  ```
  messages=[{"role": "system","content": "You are a helpful assistant."},
    {"role": "user","content": "Who are you?"}]
  ```

  **Message array for the second turn of conversation**

  ```
  messages=[{"role": "system","content": "You are a helpful assistant."},
    {"role": "user","content": "Who are you?"},
    {"role": "assistant","content": "I am Qwen, developed by Alibaba Cloud."},
    {"role": "user","content": "What can you do?"}]
  ```

  As the number of conversation turns increases, the benefits of caching -- faster inference and lower cost -- become more pronounced.

  1.   **Role playing or few-shot learning**In role playing or few-shot learning scenarios, you typically include a large amount of information in the prompt to guide the model's output format. This creates a large shared prefix across different requests.For example, if you want the model to act as a marketing expert, the system prompt contains a large amount of text information. The following are message examples for two requests:

  ```
  system_prompt = """You are an experienced marketing expert. Please provide detailed marketing suggestions for different products in the following format:

  1. Target audience: xxx

  2. Main selling points: xxx

  3. Marketing channels: xxx
  ...
  12. Long-term development strategy: xxx

  Please ensure that your suggestions are specific, actionable, and highly relevant to the product features."""

  # User message for the first request, asking about a smartwatch
  messages_1=[
    {"role": "system", "content": system_prompt},
    {"role": "user", "content": "Please provide marketing suggestions for a newly launched smartwatch."}
  ]

  # User message for the second request, asking about a laptop. Because the system_prompt is the same, there is a high probability of hitting the Cache.
  messages_2=[
    {"role": "system", "content": system_prompt},
    {"role": "user", "content": "Please provide marketing suggestions for a newly launched laptop."}
  ]
  ```

  With context cache, the system can respond quickly once a cache hit occurs, even if the user frequently changes the type of product they are asking about, such as from a smartwatch to a laptop.

  1.   **Video understanding**In video understanding scenarios, if you ask multiple questions about the same video, placing the `video` before the `text` increases the probability of a cache hit. If you ask the same question about different videos, placing the `text` before the `video` increases the probability of a cache hit. The following is a message example for two requests for the same video:

  ```
  # User message for the first request, asking about the content of this video
  messages1 = [
    {"role":"system","content":[{"text": "You are a helpful assistant."}]},
    {"role": "user",
      "content": [
        {"video": "https://help-static-aliyun-doc.aliyuncs.com/file-manage-files/zh-CN/20250328/eepdcq/phase_change_480p.mov"},
        {"text": "What is the content of this video?"}
      ]
    }
  ]

  # User message for the second request, asking about the video timestamp. Because the question is based on the same video, placing the video before the text has a high probability of hitting the Cache.
  messages2 = [
    {"role":"system","content":[{"text": "You are a helpful assistant."}]},
    {"role": "user",
      "content": [
        {"video": "https://help-static-aliyun-doc.aliyuncs.com/file-manage-files/zh-CN/20250328/eepdcq/phase_change_480p.mov"},
        {"text": "Please describe the series of events in the video, and output the start time (start_time), end time (end_time), and event (event) in JSON format. Do not output the ```json``` code segment."}
      ]
    }
  ]
  ```

  ## Session cache

  In multi-turn conversation scenarios, enabling session cache allows the server to automatically cache conversation context, reducing inference latency and usage costs. You don't need to manage caching manually — just make calls as you would for a normal multi-turn conversation.

  ### Usage

  Add the following field to the request header to control the session cache:

  *   `x-dashscope-session-cache: enable`: Enables session cache.
  *   `x-dashscope-session-cache: disable`: Disables session cache (default).

  ### Cache behavior

  *   **When session cache is enabled**:
      *   If the model supports explicit cache: uses explicit cache. Billing and constraints follow explicit cache.
      *   If the model does not support explicit cache but supports implicit cache: uses implicit cache. Billing and constraints follow implicit cache.

  *   **When session cache is not enabled**: behaves the same as a normal call. Models that support implicit cache still have it enabled automatically, but do not benefit from session cache optimizations.

  ### Code examples

  **Second-turn response example (cache hit)**The `cached_tokens` field shows the number of hit cache tokens:

  ```
  {
    "id": "145584fd-3dce-4890-99dc-e3896d7f5a42",
    "created_at": 1772440976.0,
    "error": null,
    "incomplete_details": null,
    "instructions": null,
    "metadata": null,
    "model": "qwen3.7-plus",
    "object": "response",
    "output": [
      {
        "id": "msg_62a4e323-d78c-46c7-8469-2ad50f8af4b1",
        "type": "reasoning",
        "content": null
      },
      {
        "id": "msg_560e34a6-1bdf-42ae-993e-590b38249146",
        "content": [
          {
            "annotations": [],
            "text": "Although both Random Forest and GBDT (Gradient Boosting Decision Tree) are ensemble algorithms based on decision trees, they have the following main differences:\n\n1.  **Different Ensemble Strategies**\n    *   **Random Forest**: Based on the **Bagging** idea. Each tree is trained independently, with no dependency between them.\n    *   **GBDT**: Based on the **Boosting** idea. The trees have a strong dependency relationship, where the next tree aims to fit the residuals (negative gradient) of the previous tree's prediction.\n\n2.  **Different Training Methods**\n    *   **Random Forest**: Supports **parallel training** because the trees are independent, which usually results in higher computational efficiency.\n    *   **GBDT**: Must be **trained serially** because the next tree depends on the output of the previous one, making it inherently difficult to parallelize (although engineering implementations like XGBoost have made parallel optimizations at the feature level).\n\n3.  **Different Optimization Objectives**\n    *   **Random Forest**: Mainly reduces **variance** by averaging multiple models to prevent overfitting and improve stability.\n    *   **GBDT**: Mainly reduces **bias** by progressively correcting errors to improve the model's fitting ability and accuracy.\n\n4.  **Sensitivity to Outliers**\n    *   **Random Forest**: Relatively robust and not sensitive to outliers.\n    *   **GBDT**: More sensitive to outliers because outliers produce large residuals, which affect the fitting direction of subsequent trees.\n\nIn summary, Random Forest excels in stability and parallel efficiency, while GBDT usually performs better in terms of accuracy but is more complex to tune and slower to train.",
            "type": "output_text"
          }
        ],
        "role": "assistant",
        "status": "completed",
        "type": "message"
      }
    ],
    "parallel_tool_calls": false,
    "temperature": null,
    "tool_choice": "auto",
    "tools": [],
    "top_p": null,
    "status": "completed",
    "usage": {
      "input_tokens": 1524,
      "input_tokens_details": {
        "cached_tokens": 1305
      },
      "output_tokens": 1534,
      "output_tokens_details": {
        "reasoning_tokens": 1187
      },
      "total_tokens": 3058
    }
  }
  ```

  The `input_tokens` for the second turn of the conversation is 1524, of which the `cached_tokens` is 1305. This indicates that the context from the first turn was a cache hit, which can effectively reduce inference latency and cost.

  ### Billing

  Session cache billing depends on the actual cache type used:

  *   When explicit cache is used, billing follows explicit cache rules.
  *   When implicit cache is used, billing follows implicit cache rules.

  ### Limitations

  *   The minimum cacheable prompt length is **1024** tokens.
  *   The cache validity period is **5 minutes** and is reset upon a hit.
  *   It is applicable only to the Responses API and must be used with the `previous_response_id` parameter for multi-turn conversations.

  ## FAQ

  ### How long is the context cache retained (validity period)?

  The validity period of the context cache depends on the cache type:

  *   **Explicit cache**: The validity period is 5 minutes, and each cache hit resets it to another 5 minutes. If the cache block is not hit within 5 minutes, the system automatically clears it.
  *   **Implicit cache**: Managed automatically by the system with no fixed validity period. The system periodically clears cache data that has not been used for a long time.

  ### How do I disable implicit cache?

  You cannot disable it. Implicit cache is automatically enabled for all supported models because it does not affect response quality and reduces costs while improving response speed when the cache is hit.

  ### Why was the explicit cache not hit after I created it?

  There are several possible reasons:

  *   The cache was not hit within 5 minutes of creation, and the system cleared the cache block after it expired.
  *   If the last `content` block is separated from the existing cache block by more than 20 `content` blocks, the cache is not hit. We recommend that you create a new cache block.

  ### Does hitting the explicit cache reset its validity?

  Yes. Each hit resets the validity period of that cache block to 5 minutes.

  ### Is the explicit cache shared between different accounts?

  No. Both implicit and explicit cache data are isolated at the account level and are not shared across accounts.

  ### Is explicit cache shared between different models?

  No. Cache data is isolated between models and is not shared.

  ### Why doesn't `input_tokens` equal `cache_creation_input_tokens` + `cached_tokens`?

  To ensure model output quality, the backend service appends a small number of tokens (usually fewer than 10) after the user-provided prompt. These tokens are placed after the `cache_control` marker, so they are not counted for cache creation or hits. However, they are included in the total `input_tokens`.
