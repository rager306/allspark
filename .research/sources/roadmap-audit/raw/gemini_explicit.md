url: https://ai.google.dev/gemini-api/docs/caching?lang=python&api=generateContent
title: Context caching
content: >-
  Skip to main content


  /


  *   English

  *   Deutsch

  *   Español – América Latina

  *   Français

  *   Indonesia

  *   Italiano

  *   Polski

  *   Português – Brasil

  *   Shqip

  *   Tiếng Việt

  *   Türkçe

  *   Русский

  *   עברית

  *   العربيّة

  *   فارسی

  *   हिंदी

  *   বাংলা

  *   ภาษาไทย

  *   中文 – 简体

  *   中文 – 繁體

  *   日本語

  *   한국어


  Get API key Cookbook Community


  Sign in


  Docs API reference More


  *   Gemini API
      *   Docs
      *   API reference

  *   Get API key

  *   Cookbook

  *   Community


  *   Get started 

  *   Overview

  *   Get started

  *   API keys

  *   Pricing

  *   Coding agent setup

  *   Models 

  *   All models

  *   Latest Gemini models

  *   Nano Banana

  *   Veo

  *   Gemini Omni Flash

  *   

  Lyria 3.5 & Lyria Clip 
      *   Overview
      *   Lyria RealTime
      *   Lyria prompt guide

  *   

  Text-to-speech 
      *   Overview
      *   Voice design
      *   Voice replication

  *   Transcribe

  *   Live

  *   Live translate

  *   Embeddings

  *   

  Robotics 
      *   Overview
      *   Spatial reasoning
      *   Agentic vision
      *   Task orchestration
      *   Robotics with streaming
      *   Video understanding

  *   Core capabilities 

  *   Text

  *   

  Image 
      *   Image generation 🍌
      *   Image understanding

  *   

  Video 
      *   Video overview
      *   Video understanding

  *   Documents

  *   

  Speech and audio 
      *   Speech generation
      *   Audio understanding
      *   Audio transcription

  *   

  Thinking 
      *   Thinking
      *   Thought signatures

  *   Structured outputs

  *   Function calling

  *   Long context

  *   Long decode

  *   Agents 

  *   Overview

  *   Quickstart

  *   Antigravity agent

  *   Building managed agents

  *   Environments

  *   Credentials

  *   Hooks

  *   Deep Research agent

  *   Tools 

  *   Overview

  *   Google Search

  *   Google Maps

  *   Code execution

  *   URL context

  *   Computer use

  *   File search

  *   Combine tools and function calling

  *   Live API 

  *   Overview

  *   

  Get started 
      *   Get started using the GenAI SDK
      *   Get started using raw WebSockets

  *   Capabilities

  *   Thinking

  *   Live transcription

  *   Live translation

  *   Tool use

  *   Session management

  *   Ephemeral tokens

  *   Best practices

  *   Optimization 

  *   Overview

  *   Batch API

  *   Webhooks

  *   Flex inference

  *   Priority inference

  *   Context caching

  *   Guides 

  *   Interactions API

  *   Streaming

  *   Background execution

  *   

  File input 
      *   Input methods
      *   Files API

  *   Open AI compatibility

  *   Media resolution

  *   Token counting

  *   Prompt engineering

  *   

  Logs and datasets 
      *   Get started with logs
      *   Data logging and sharing

  *   

  Safety 
      *   Safety settings
      *   Safety guidance

  *   

  Frameworks 
      *   LangChain & LangGraph
      *   CrewAI
      *   LlamaIndex
      *   Vercel AI SDK
      *   Temporal

  *   Resources 

  *   Release notes

  *   Deprecations

  *   Libraries

  *   

  Migration 
      *   Migrate to Gen AI SDK
      *   Migrate to Interactions API
      *   Interactions breaking changes (May 2026)

  *   Rate limits

  *   Billing info

  *   API troubleshooting

  *   API errors

  *   Status

  *   Partner and library integrations

  *   

  Google AI Studio 
      *   Quickstart
      *   Google AI plans
      *   Vibe code in Build mode
      *   Developing full-stack apps
      *   Build Android apps
      *   Deploying your app
      *   Agents in AI Studio Playground
      *   Try out LearnLM
      *   Troubleshooting
      *   Access for Workspace users

  *   

  Google Cloud Platform 
      *   Gemini Enterprise Agent Platform Gemini API
      *   OAuth authentication

  *   Policies 

  *   Terms of service

  *   Available regions

  *   Abuse monitoring

  *   Feedback information


  *   On this page

  *   Implicit caching

   Gemini 4 Argon is now available. Try it out. 

  *   Home

  *    Gemini API

  *    Docs


  Select an option Interactions API (Recommended)

  *   Interactions API (Recommended)

  *   generateContent API

  *   Learn more

   Send feedback 
  # Context caching


  *   On this page

  *   Implicit caching


  In a typical AI workflow, you might pass the same input tokens over and over
  to a model. The Gemini API offers implicit caching to optimize performance and
  costs.


  **Note:** The **Interactions API** only supports implicit caching. Explicit
  caching (manually creating and managing cache objects) is not supported in the
  Interactions API, or for Gemini 4 Argon and onward. To use explicit caching on
  supported Gemini 3 and 2.5 models, switch to the generateContent API.

  ## Implicit caching


  Implicit caching is enabled by default for all Gemini 2.5 and newer models. It
  is supported for both stateful (using `previous_interaction_id`) and stateless
  conversation modes. We automatically pass on cost savings if your request hits
  caches. You don't need to configure anything to turn it on. The following
  table lists the minimum input token count for each model:


  | Model | Min token limit |

  | --- | --- |

  | Gemini 4 Argon | 4,096 |

  | Gemini 3.8 Flash | 4,096 |

  | Gemini 3.6 Flash | 4,096 |

  | Gemini 3.1 Pro Preview | 4,096 |

  | Gemini 2.5 Flash | 2,048 |

  | Gemini 2.5 Pro | 2,048 |


  To increase the chance of an implicit cache hit:


  *   Try putting large and common contents at the beginning of your prompt

  *   Try to send requests with similar prefix in a short amount of time


  You can see the number of tokens which were cache hits in the response
  object's `usage.total_cached_tokens` (Python and JavaScript) field.

   Send feedback 
  Except as otherwise noted, the content of this page is licensed under the
  Creative Commons Attribution 4.0 License, and code samples are licensed under
  the Apache 2.0 License. For details, see the Google Developers Site Policies.
  Java is a registered trademark of Oracle and/or its affiliates.


  Last updated 2026-10-13 UTC.

   Need to tell us more?  [[["Easy to understand","easyToUnderstand","thumb-up"],["Solved my problem","solvedMyProblem","thumb-up"],["Other","otherUp","thumb-up"]],[["Missing the information I need","missingTheInformationINeed","thumb-down"],["Too complicated / too many steps","tooComplicatedTooManySteps","thumb-down"],["Out of date","outOfDate","thumb-down"],["Samples / code issue","samplesCodeIssue","thumb-down"],["Other","otherDown","thumb-down"]],["Last updated 2026-10-13 UTC."],[],[]] 

  *   Terms

  *   Privacy

  *   Manage cookies


  *   English

  *   Deutsch

  *   Español – América Latina

  *   Français

  *   Indonesia

  *   Italiano

  *   Polski

  *   Português – Brasil

  *   Shqip

  *   Tiếng Việt

  *   Türkçe

  *   Русский

  *   עברית

  *   العربيّة

  *   فارسی

  *   हिंदी

  *   বাংলা

  *   ภาษาไทย

  *   中文 – 简体

  *   中文 – 繁體

  *   日本語

  *   한국어
