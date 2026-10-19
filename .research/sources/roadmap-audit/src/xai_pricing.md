url: https://docs.x.ai/developers/advanced-api-usage/prompt-caching/usage-and-pricing
title: Usage & Pricing
content: >-
  #### Prompt Caching


  ## Chat Completions API


  Cached tokens appear in `usage.prompt_tokens_details.cached_tokens`:


  JSON


  ```

  {
    "usage": {
      "prompt_tokens": 125,
      "completion_tokens": 48,
      "total_tokens": 173,
      "prompt_tokens_details": {
        "text_tokens": 125,
        "audio_tokens": 0,
        "image_tokens": 0,
        "cached_tokens": 98
      },
      "completion_tokens_details": {
        "reasoning_tokens": 0,
        "audio_tokens": 0,
        "accepted_prediction_tokens": 0,
        "rejected_prediction_tokens": 0
      }
    }
  }

  ```


  ## Responses API


  Cached tokens appear in `usage.input_tokens_details.cached_tokens`:


  JSON


  ```

  {
    "usage": {
      "input_tokens": 125,
      "output_tokens": 48,
      "total_tokens": 173,
      "input_tokens_details": {
        "cached_tokens": 98
      },
      "output_tokens_details": {
        "reasoning_tokens": 0
      }
    }
  }

  ```


  * * *


  ## Verifying cache hits


  To determine whether your request benefitted from prompt caching, check the
  `cached_tokens` value in the response:


  | `cached_tokens` value | What it means |

  | --- | --- |

  | `0` | **Cache miss** — the entire prompt was computed from scratch. This is
  expected on the first request or after cache eviction. |

  | `> 0` | **Cache hit** — some or all of your prompt prefix was served from
  cache. The number indicates how many tokens were reused. |

  | Equal to `prompt_tokens` | **Full cache hit** — your entire prompt was
  served from cache (rare, typically happens when resending the exact same
  request). |


  A typical multi-turn conversation shows increasing `cached_tokens` over time:


  Text


  ```

  Turn 1: prompt_tokens=50,  cached_tokens=0    # First request, cache
  established

  Turn 2: prompt_tokens=120, cached_tokens=50   # Previous 50 tokens cached

  Turn 3: prompt_tokens=200, cached_tokens=120  # Previous 120 tokens cached

  ```


  If `cached_tokens` is consistently 0 across multiple requests in the same
  conversation, verify that you're setting `x-grok-conv-id` (or
  `prompt_cache_key`) and that you're not modifying earlier messages between
  requests.


  * * *


  ## Pricing


  Cached tokens are billed at the **cached prompt token price**, which is
  substantially lower than the regular prompt token price. The exact rates vary
  by model — check the Pricing page for current prices.


  | Token type | Billing rate |

  | --- | --- |

  | Prompt tokens (non-cached) | Full prompt token price |

  | Cached prompt tokens | Reduced cached prompt token price |

  | Completion tokens | Full completion token price |

  | Reasoning tokens | Full completion token price |


  Long context pricing applies when total prompt tokens (including cached
  tokens) exceed the model's long context threshold. Both cached and non-cached
  tokens use their respective long-context rates in this case.


  * * *


  ## Next


  *   Best Practices & FAQ
