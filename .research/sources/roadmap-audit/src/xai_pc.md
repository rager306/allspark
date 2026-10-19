url: https://docs.x.ai/developers/advanced-api-usage/prompt-caching
title: Prompt Caching
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


  #### Advanced API Usage


  # Prompt Caching


  Copy for LLM View as Markdown


  Create API key Meet grok-4.7


  When consecutive requests share the same starting messages, the xAI API
  automatically caches them. On the next request, messages at the beginning that
  match exactly are served from cache:


  *   **Faster time-to-first-token** — the model skips re-computing cached
  messages

  *   **Lower cost** — cached tokens are billed at a reduced rate


  The xAI API performs prompt caching **automatically**. However, we recommend
  setting the `x-grok-conv-id` HTTP header to maximize your cache hit rate.


  * * *


  ## In this section


  *   How It Works — Understand how caching works from the start of your
  messages array

  *   Maximizing Cache Hits — Set up `x-grok-conv-id` and `prompt_cache_key` for
  optimal caching

  *   What Breaks Caching — Common mistakes that cause cache misses

  *   Usage & Pricing — Read cached token counts and understand billing

  *   Best Practices & FAQ — Tips, supported models, and common questions


  * * *


  Last updated: March 16, 2026
