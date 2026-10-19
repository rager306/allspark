url: https://docs.x.ai/developers/advanced-api-usage/prompt-caching/maximizing-cache-hits
title: Maximizing Cache Hits
content: >-
  API


  Grok Build Grok Bot Grok X Subscription


  Search


  ⌘K


  API Console


  Products


  Grok


  Status


  Loading...


  Resources


  llms.txt Discord Email support Terms and Policies


  Get Started


  *   Overview

  *   Quickstart

  *   Grok 4.7 Latest

  *   Models

  *   Pricing

  *   Release Notes


  Text


  *   Text Generation

  *   Streaming

  *   Reasoning

  *   Structured Outputs

  *   Image Understanding

  *   Multi Agent

  *   Best Practices Guide


  Imagine


  *   Overview

  *   Image Generation

  *   Image Editing

  *   Multi-Image Editing

  *   Image Prompting Guide

  *   Video Generation

  *   Video Overview New

  *   Image-to-Video

  *   Reference-to-Video

  *   Video Editing

  *   Video Extension


  Voice


  *   Overview


  *   Speech to Speech 


  *   Text to Speech

  *   Speech to Text New

  *   Custom Voices

  *   Ephemeral Tokens


  Files & Collections


  *   Files Overview

  *   Managing Files

  *   Public URLs New

  *   Chat with Files

  *   Collections

  *   Collections via API

  *   Collection Metadata


  *   Generated Media 


  Tools


  *   Overview

  *   Function Calling

  *   Web Search

  *   X Search

  *   Code Execution

  *   Image Generation

  *   Collections Search (RAG)

  *   Remote MCP Tools


  *   In Depth 


  Advanced API Usage


  *   Rate Limits


  *   Prompt Caching

      *   Overview
      *   How It Works
      *   Maximizing Cache Hits
      *   What Breaks Caching
      *   Usage & Pricing
      *   Best Practices & FAQ

  *   Context Compaction

  *   Priority Processing

  *   Batch API

  *   Deferred Completions

  *   Async Requests

  *   WebSocket Mode

  *   mTLS Authentication

  *   Regional Endpoints New

  *   Cost Tracking

  *   Debugging Errors


  Platforms


  *   Community Integrations

  *   Google Cloud Vertex AI

  *   Microsoft Foundry

  *   Docs MCP


  Migration Guides


  *   Imagine Image Quality Retirement (Nov 2, 2026)New

  *   Model Retirement (May 15, 2026)

  *   Migrating to Responses API

  *   Chat Completions (Legacy)


  FAQ


  *   Data & Privacy

  *   General


  *   API
      *   Docs
      *   REST Reference

  *   Grok Build

  *   Grok Bot

  *   Grok

  *   X Subscription


  Get Started


  *   Overview

  *   Quickstart

  *   Grok 4.7 Latest

  *   Models

  *   Pricing

  *   Release Notes


  Text


  *   Text Generation

  *   Streaming

  *   Reasoning

  *   Structured Outputs

  *   Image Understanding

  *   Multi Agent

  *   Best Practices Guide


  Imagine


  *   Overview

  *   Image Generation

  *   Image Editing

  *   Multi-Image Editing

  *   Image Prompting Guide

  *   Video Generation

  *   Video Overview New

  *   Image-to-Video

  *   Reference-to-Video

  *   Video Editing

  *   Video Extension


  Voice


  *   Overview


  *   Speech to Speech 


  *   Text to Speech

  *   Speech to Text New

  *   Custom Voices

  *   Ephemeral Tokens


  Files & Collections


  *   Files Overview

  *   Managing Files

  *   Public URLs New

  *   Chat with Files

  *   Collections

  *   Collections via API

  *   Collection Metadata


  *   Generated Media 


  Tools


  *   Overview

  *   Function Calling

  *   Web Search

  *   X Search

  *   Code Execution

  *   Image Generation

  *   Collections Search (RAG)

  *   Remote MCP Tools


  *   In Depth 


  Advanced API Usage


  *   Rate Limits


  *   Prompt Caching

      *   Overview
      *   How It Works
      *   Maximizing Cache Hits
      *   What Breaks Caching
      *   Usage & Pricing
      *   Best Practices & FAQ

  *   Context Compaction

  *   Priority Processing

  *   Batch API

  *   Deferred Completions

  *   Async Requests

  *   WebSocket Mode

  *   mTLS Authentication

  *   Regional Endpoints New

  *   Cost Tracking

  *   Debugging Errors


  Platforms


  *   Community Integrations

  *   Google Cloud Vertex AI

  *   Microsoft Foundry

  *   Docs MCP


  Migration Guides


  *   Imagine Image Quality Retirement (Nov 2, 2026)New

  *   Model Retirement (May 15, 2026)

  *   Migrating to Responses API

  *   Chat Completions (Legacy)


  FAQ


  *   Data & Privacy

  *   General


  #### Prompt Caching


  # Maximizing Cache Hits


  Copy for LLM View as Markdown


  Create API key Meet grok-4.7


  ## Set `x-grok-conv-id` (Chat Completions API)


  The `x-grok-conv-id` HTTP header routes requests with the same conversation ID
  to the same server. Since cache entries are stored per-server, this maximizes
  your cache hit rate.


  Python (OpenAI)Bash JavaScript (OpenAI)


  ```

  from openai import OpenAI


  client = OpenAI(
      api_key="YOUR_XAI_API_KEY",
      base_url="https://api.x.ai/v1",
  )


  response = client.chat.completions.create(
      model="grok-4.7",
      messages=[
          {"role": "system", "content": "You are Grok, a helpful and truthful AI assistant built by SpaceXSI."},
          {"role": "user", "content": "What is prompt caching?"},
      ],
      extra_headers={
          "x-grok-conv-id": "conv_abc123",
      },
  )


  print(response.choices[0].message.content)

  print(f"Cached tokens: {response.usage.prompt_tokens_details.cached_tokens}")

  ```


  ```

  curl https://api.x.ai/v1/chat/completions \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $XAI_API_KEY" \
    -H "x-grok-conv-id: conv_abc123" \
    -d '{
      "model": "grok-4.7",
      "messages": [
        {"role": "system", "content": "You are Grok, a helpful and truthful AI assistant built by SpaceXSI."},
        {"role": "user", "content": "What is prompt caching?"}
      ]
    }'
  ```


  ```

  import OpenAI from 'openai';


  const client = new OpenAI({
    apiKey: 'YOUR_XAI_API_KEY',
    baseURL: 'https://api.x.ai/v1',
  });


  const response = await client.chat.completions.create(
    {
      model: 'grok-4.7',
      messages: [
        {
          role: 'system',
          content:
            'You are Grok, a helpful and truthful AI assistant built by SpaceXSI.',
        },
        { role: 'user', content: 'What is prompt caching?' },
      ],
    },
    {
      headers: {
        'x-grok-conv-id': 'conv_abc123',
      },
    },
  );


  console.log(response.choices[0].message.content);

  console.log(
    `Cached tokens: ${response.usage.prompt_tokens_details.cached_tokens}`,
  );

  ```


  * * *


  ## Set `prompt_cache_key` (Responses API)


  For the Responses API, use the `prompt_cache_key` field directly in the
  request body. It functions identically to setting `x-grok-conv-id` — it routes
  requests to the same server for cache reuse.

   JavaScript Python (OpenAI)Bash JavaScript (OpenAI)

  ```

  import { xai } from '@ai-sdk/xai';

  import { generateText } from 'ai';


  const { text, usage } = await generateText({
    model: xai.responses('grok-4.7'),
    prompt: 'What is prompt caching?',
    providerOptions: {
      xai: {
        promptCacheKey: 'b79ad29b-b3f9-463c-bca6-041d5058d366',
      },
    },
  });


  console.log(text);

  console.log(`Total tokens: ${usage.totalTokens}`);

  ```


  ```

  from openai import OpenAI


  client = OpenAI(
      api_key="YOUR_XAI_API_KEY",
      base_url="https://api.x.ai/v1",
  )


  response = client.responses.create(
      model="grok-4.7",
      input="What is prompt caching?",
      extra_body={
          "prompt_cache_key": "b79ad29b-b3f9-463c-bca6-041d5058d366",
      },
  )


  print(response.output_text)

  print(f"Cached tokens: {response.usage.input_tokens_details.cached_tokens}")

  ```


  ```

  curl https://api.x.ai/v1/responses \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $XAI_API_KEY" \
    -d '{
      "model": "grok-4.7",
      "input": "What is prompt caching?",
      "prompt_cache_key": "b79ad29b-b3f9-463c-bca6-041d5058d366"
    }'
  ```


  ```

  import OpenAI from 'openai';


  const client = new OpenAI({
    apiKey: 'YOUR_XAI_API_KEY',
    baseURL: 'https://api.x.ai/v1',
  });


  const response = await client.responses.create({
    model: 'grok-4.7',
    input: 'What is prompt caching?',
    // @ts-expect-error -- SpaceXSI-specific field
    prompt_cache_key: 'b79ad29b-b3f9-463c-bca6-041d5058d366',
  });


  console.log(response.output_text);

  console.log(
    `Cached tokens: ${response.usage.input_tokens_details.cached_tokens}`,
  );

  ```


  * * *


  ## Set `x-grok-conv-id` metadata (gRPC API)


  For the gRPC API using the xAI SDK, pass `x-grok-conv-id` as gRPC metadata to
  enable sticky routing for cache reuse.


  Python


  ```

  from xai_sdk import Client

  from xai_sdk.chat import system, user


  client = Client(
      api_key="YOUR_API_KEY",
      metadata=(("x-grok-conv-id", "conv_abc123"),),
  )


  chat = client.chat.create(model="grok-4.7")

  chat.append(system("You are Grok, a helpful and truthful AI assistant built by
  SpaceXSI."))

  chat.append(user("What is prompt caching?"))


  response = chat.sample()

  print(f"Response: {response.content}")

  print(f"Cached tokens: {response.usage.cached_prompt_text_tokens}")

  ```


  * * *


  ## Next


  *   What Breaks Caching


  * * *


  Last updated: October 10, 2026
