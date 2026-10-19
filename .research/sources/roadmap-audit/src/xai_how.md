url: https://docs.x.ai/developers/advanced-api-usage/prompt-caching/how-it-works
title: How Prompt Caching Works
content: >-
  API


  Grok Build Grok Bot Grok X Subscription


  Search


  Ctrl+K


  API Console


  Products


  Grok


  Status


  Available


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


  # How It Works


  Copy for LLM View as Markdown


  Create API key Meet grok-4.7


  The cache works from the **start of your messages array**. When a request
  arrives, the system checks how many messages at the beginning match a previous
  request exactly — that matching portion is the "prefix" and gets served from
  cache:


  1.   **First request** — The full prompt is processed and cached

  2.   **Subsequent requests** — If the prompt prefix matches, the cached
  portion is reused (a cache _hit_)

  3.   **Billing** — Cached tokens are billed at a reduced rate


  Prompt caching is not 100% guaranteed. Cache entries can be evicted due to
  memory pressure, and requests may be routed to different servers. Use
  `x-grok-conv-id` to maximize cache hit rates.


  * * *


  ## Example


  **Request 1:**


  Text


  ```

  [system] "You are a helpful assistant."

  [user] "What is the capital of France?"

  [assistant] "The capital of France is Paris."

  ```


  **Request 2:**


  Text


  ```

  [system] "You are a helpful assistant."       ← cached

  [user] "What is the capital of France?"       ← cached

  [assistant] "The capital of France is Paris." ← cached

  [user] "What about Germany?"                  ← new

  ```


  The first 3 messages match Request 1 exactly, so they're served from cache.
  Only the new message is computed.


  * * *


  ## Next


  *   Maximizing Cache Hits

  *   What Breaks Caching

  *   Usage & Pricing

  *   Best Practices & FAQ


  * * *


  Last updated: March 16, 2026


  On this page

  *   Example

  *   Next


  Copy for LLM Share feedback
