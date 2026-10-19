Title: Apply Patch | OpenAI API

URL Source: https://developers.openai.com/api/docs/guides/tools-apply-patch

Published Time: Sun, 18 Oct 2026 15:54:55 GMT

Markdown Content:
For the complete documentation index, see [llms.txt](https://developers.openai.com/llms.txt). Markdown versions of documentation pages are available by appending `.md` to the page URL. 

[![Image 1: OpenAI Developers](https://developers.openai.com/OpenAI_Developers.svg)ChatGPT](https://developers.openai.com/)

Start searching

[API Dashboard](https://platform.openai.com/login)

[Try ChatGPT](https://chatgpt.com/)

[Home](https://developers.openai.com/)

[API](https://developers.openai.com/api/docs)

[Overview Get started with the OpenAI API](https://developers.openai.com/api/docs)[Models Explore models and compare capabilities](https://developers.openai.com/api/docs/models)[Agents Build persistent agents on hosted infrastructure](https://developers.openai.com/api/docs/guides/agents)[Tools Connect models to tools and data](https://developers.openai.com/api/docs/guides/tools)[Audio & voice Build speech and realtime voice experiences](https://developers.openai.com/api/docs/guides/audio)[Production Deploy and scale your API integrations](https://developers.openai.com/api/docs/guides/production-best-practices)[API reference Explore endpoints, parameters, and responses](https://developers.openai.com/api/reference/overview)

[ChatGPT](https://developers.openai.com/chatgpt)

[Plugins Extend ChatGPT and Codex](https://developers.openai.com/plugins)[Sign in with ChatGPT Apps powered by your user's ChatGPT plan](https://developers.openai.com/siwc)[Commerce Build commerce flows in ChatGPT](https://developers.openai.com/commerce)[Ads Publish and measure ads in ChatGPT](https://developers.openai.com/ads)[Workspace Agents Trigger published ChatGPT workspace agents](https://developers.openai.com/workspace-agents)[ChatGPT + Codex user docs Guides and product docs for ChatGPT and Codex](https://learn.chatgpt.com/docs)[Use cases Example workflows and tasks teams can take on with ChatGPT or Codex](https://learn.chatgpt.com/use-cases)

[Docs](https://developers.openai.com/codex)

[Use cases](https://developers.openai.com/codex/use-cases)

[Training](https://developers.openai.com/training)

[Resources](https://developers.openai.com/codex/resources)

[Resources](https://developers.openai.com/learn)

[Showcase Demo apps to get inspired](https://developers.openai.com/showcase)[Blog Learnings and experiences from developers](https://developers.openai.com/blog)[Cookbook Notebook examples for building with OpenAI models](https://developers.openai.com/cookbook)[Learn Docs, videos, and demo apps for building with OpenAI](https://developers.openai.com/learn)[Community Programs, meetups, and support for builders](https://developers.openai.com/community)

[Overview](https://developers.openai.com/api/docs)[Models](https://developers.openai.com/api/docs/models)[Agents](https://developers.openai.com/api/docs/guides/agents)[Tools](https://developers.openai.com/api/docs/guides/tools)[Audio & voice](https://developers.openai.com/api/docs/guides/audio)[Production](https://developers.openai.com/api/docs/guides/production-best-practices)[API reference](https://developers.openai.com/api/reference/overview)

## Search the API docs

Search docs 

### Suggested

responses create reasoning_effort realtime prompt caching

 Primary navigation 

 API  ChatGPT  Docs  Use cases  Training  Resources  Resources 

Search docs 

### Suggested

responses create reasoning_effort realtime prompt caching

 Overview  Models  Agents  Tools  Audio & voice  Production  API reference 

Docs Tools

*   [Home](https://developers.openai.com/api/docs)

### Get started

*   [Quickstart](https://developers.openai.com/api/docs/quickstart)
*   [Using GPT-6](https://developers.openai.com/api/docs/guides/latest-model)
*   [Key concepts](https://developers.openai.com/api/docs/concepts)

### Core concepts

*   [Responses API](https://developers.openai.com/api/docs/guides/migrate-to-responses)
*   [Decisions API](https://developers.openai.com/api/docs/guides/decisions)
*   [Conversation state](https://developers.openai.com/api/docs/guides/conversation-state)
*   [Background mode](https://developers.openai.com/api/docs/guides/background)
*   [Streaming](https://developers.openai.com/api/docs/guides/streaming-responses)
*   [WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)
*   [Mid-turn steering](https://developers.openai.com/api/docs/guides/steering)
*   [Multi-agent](https://developers.openai.com/api/docs/guides/responses-multi-agent)
*   [Webhooks](https://developers.openai.com/api/docs/guides/webhooks)
*   [File inputs](https://developers.openai.com/api/docs/guides/file-inputs)
*   [Compaction](https://developers.openai.com/api/docs/guides/compaction)
*   [Counting tokens](https://developers.openai.com/api/docs/guides/token-counting)

### SDKs and CLI

*   [OpenAI SDK](https://developers.openai.com/api/docs/libraries)
*   [OpenAI CLI](https://developers.openai.com/api/docs/libraries/openai-cli)

### Resources

*   [Changelog](https://developers.openai.com/api/docs/changelog)
*   [Deprecations](https://developers.openai.com/api/docs/deprecations)
*   [Supported countries](https://developers.openai.com/api/docs/supported-countries)
*   [OpenAI Crawlers](https://developers.openai.com/api/docs/bots)
*   [Terms and policies](https://openai.com/policies)

### Legacy APIs

*   
Agent Builder
    *   [Overview](https://developers.openai.com/api/docs/guides/agent-builder)
    *   [Migration guide](https://developers.openai.com/api/docs/guides/agent-builder/migrate-from-agent-builder)
    *   [Node reference](https://developers.openai.com/api/docs/guides/node-reference)
    *   [Safety in building agents](https://developers.openai.com/api/docs/guides/agent-builder-safety)

*   
Evals
    *   [Getting started](https://developers.openai.com/api/docs/guides/evaluation-getting-started)
    *   [Working with evals](https://developers.openai.com/api/docs/guides/evals)
    *   [Prompt optimizer](https://developers.openai.com/api/docs/guides/prompt-optimizer)
    *   [External models](https://developers.openai.com/api/docs/guides/external-models)
    *   [Graders](https://developers.openai.com/api/docs/guides/graders)

*   
Fine-tuning
    *   [Optimization cycle](https://developers.openai.com/api/docs/guides/model-optimization)
    *   [Supervised fine-tuning](https://developers.openai.com/api/docs/guides/supervised-fine-tuning)
    *   [Vision fine-tuning](https://developers.openai.com/api/docs/guides/vision-fine-tuning)
    *   [Direct preference optimization](https://developers.openai.com/api/docs/guides/direct-preference-optimization)
    *   [Reinforcement fine-tuning](https://developers.openai.com/api/docs/guides/reinforcement-fine-tuning)
    *   [RFT use cases](https://developers.openai.com/api/docs/guides/rft-use-cases)
    *   [Best practices](https://developers.openai.com/api/docs/guides/fine-tuning-best-practices)

*   
Assistants API
    *   [Migration guide](https://developers.openai.com/api/docs/assistants/migration)

*   [Model catalog](https://developers.openai.com/api/docs/models)

### Choose a model

*   [Pricing](https://developers.openai.com/api/docs/pricing)
*   [Model selection](https://developers.openai.com/api/docs/guides/model-selection)

### Text and code

*   [Text generation](https://developers.openai.com/api/docs/guides/text)
*   [Code generation](https://developers.openai.com/api/docs/guides/code-generation)
*   [Structured output](https://developers.openai.com/api/docs/guides/structured-outputs)

### Prompting

*   [Overview](https://developers.openai.com/api/docs/guides/prompting)
*   [Prompt engineering](https://developers.openai.com/api/docs/guides/prompt-engineering)
*   [Citation formatting](https://developers.openai.com/api/docs/guides/citation-formatting)
*   [Migration guide](https://developers.openai.com/api/docs/guides/prompting/migrate-from-prompt-object)
*   [Prompt generation](https://developers.openai.com/api/docs/guides/prompt-generation)
*   [Frontend prompting](https://developers.openai.com/api/docs/guides/frontend-prompt)

### Reasoning

*   [Reasoning models](https://developers.openai.com/api/docs/guides/reasoning)
*   [Reasoning best practices](https://developers.openai.com/api/docs/guides/reasoning-best-practices)

### Images

*   
[Images and vision](https://developers.openai.com/api/docs/guides/images-vision)
    *   [Image input cost calculator](https://developers.openai.com/api/docs/guides/image-cost-calculator)

*   
[Image generation](https://developers.openai.com/api/docs/guides/image-generation)
    *   [Overview](https://developers.openai.com/api/docs/guides/image-generation)
    *   [Image prompting](https://developers.openai.com/api/docs/guides/image-prompting)

### Realtime and audio

*   [Audio and speech](https://developers.openai.com/api/docs/guides/audio)
*   [Getting started](https://developers.openai.com/api/docs/guides/realtime)
*   [Voice agents](https://developers.openai.com/api/docs/guides/voice-agents)

### Specialized models

*   [Deep research](https://developers.openai.com/api/docs/guides/deep-research)
*   [Embeddings](https://developers.openai.com/api/docs/guides/embeddings)
*   [Moderation](https://developers.openai.com/api/docs/guides/moderation)

*   [Overview](https://developers.openai.com/api/docs/guides/agents)

### Agents API

*   [Overview](https://developers.openai.com/api/docs/guides/agents-api/overview)
*   [Quickstart](https://developers.openai.com/api/docs/guides/agents-api/quickstart)
*   [Architecture](https://developers.openai.com/api/docs/guides/agents-api/architecture)
*   [Configuring Agents](https://developers.openai.com/api/docs/guides/agents-api/configuration)
*   
Sessions
    *   [Run and continue sessions](https://developers.openai.com/api/docs/guides/agents-api/sessions)
    *   [Events and items](https://developers.openai.com/api/docs/guides/agents-api/sessions/events)
    *   [Manage sessions](https://developers.openai.com/api/docs/guides/agents-api/sessions/manage)
    *   [Webhooks](https://developers.openai.com/api/docs/guides/agents-api/sessions/webhooks)

*   
Environments and sandboxes
    *   [OpenAI-hosted sandboxes](https://developers.openai.com/api/docs/guides/agents-api/environments/openai-hosted)
    *   [Self-hosted sandboxes](https://developers.openai.com/api/docs/guides/agents-api/environments/self-hosted)
    *   [Sandbox lifecycle](https://developers.openai.com/api/docs/guides/agents-api/environments/lifecycle)
    *   [Pre-warm sandboxes](https://developers.openai.com/api/docs/guides/agents-api/environments/prewarmed)
    *   [Sandbox security](https://developers.openai.com/api/docs/guides/agents-api/environments/security)
    *   [Files and artifacts](https://developers.openai.com/api/docs/guides/agents-api/environments/files)

*   
Tools and integrations
    *   [Web search](https://developers.openai.com/api/docs/guides/agents-api/tools/web-search)
    *   [Computer use](https://developers.openai.com/api/docs/guides/agents-api/tools/computer-use)
    *   [Functions](https://developers.openai.com/api/docs/guides/agents-api/tools/functions)
    *   [MCP connections](https://developers.openai.com/api/docs/guides/agents-api/tools/mcp)
    *   [Plugins](https://developers.openai.com/api/docs/guides/agents-api/tools/plugins)
    *   [Vaults](https://developers.openai.com/api/docs/guides/agents-api/tools/vaults)

*   [Multi-agent](https://developers.openai.com/api/docs/guides/agents-api/multi-agent)
*   [Observability and usage](https://developers.openai.com/api/docs/guides/agents-api/observability)
*   [Tracing](https://developers.openai.com/api/docs/guides/agents-api/tracing)
*   [Errors and recovery](https://developers.openai.com/api/docs/guides/agents-api/errors)
*   [API reference](https://developers.openai.com/api/reference/resources/beta/subresources/agents)
*   [Bedrock Managed Agents](https://developers.openai.com/api/docs/guides/agents-api/bedrock-managed-agents)

### Agents SDK

*   [Overview](https://developers.openai.com/api/docs/guides/agents/sdk)
*   [Quickstart](https://developers.openai.com/api/docs/guides/agents/quickstart)
*   [Agent definitions](https://developers.openai.com/api/docs/guides/agents/define-agents)
*   [Models and providers](https://developers.openai.com/api/docs/guides/agents/models)
*   [Running agents](https://developers.openai.com/api/docs/guides/agents/running-agents)
*   [Sandbox agents](https://developers.openai.com/api/docs/guides/agents/sandboxes)
*   [Orchestration](https://developers.openai.com/api/docs/guides/agents/orchestration)
*   [Guardrails](https://developers.openai.com/api/docs/guides/agents/guardrails-approvals)
*   [Results and state](https://developers.openai.com/api/docs/guides/agents/results)
*   [Integrations and observability](https://developers.openai.com/api/docs/guides/agents/integrations-observability)
*   [Evaluate agent workflows](https://developers.openai.com/api/docs/guides/agent-evals)

### ChatKit

*   [Overview](https://developers.openai.com/api/docs/guides/chatkit)
*   [Customize](https://developers.openai.com/api/docs/guides/chatkit-themes)
*   [Widgets](https://developers.openai.com/api/docs/guides/chatkit-widgets)
*   [Actions](https://developers.openai.com/api/docs/guides/chatkit-actions)
*   [Advanced integrations](https://developers.openai.com/api/docs/guides/custom-chatkit)

*   [Overview](https://developers.openai.com/api/docs/guides/tools)
*   [Function calling](https://developers.openai.com/api/docs/guides/function-calling)

### Search and retrieval

*   [Web search](https://developers.openai.com/api/docs/guides/tools-web-search)
*   [File search](https://developers.openai.com/api/docs/guides/tools-file-search)
*   [Retrieval](https://developers.openai.com/api/docs/guides/retrieval)

### Connect tools and data

*   [MCP servers](https://developers.openai.com/api/docs/guides/tools-connectors-mcp)
*   [Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)

### Build tool workflows

*   [Skills](https://developers.openai.com/api/docs/guides/tools-skills)
*   [Tool search](https://developers.openai.com/api/docs/guides/tools-tool-search)
*   [Programmatic tool calling](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)
*   [Async tool calling](https://developers.openai.com/api/docs/guides/async-tool-calling)

### Computer and code

*   [Shell](https://developers.openai.com/api/docs/guides/tools-shell)
*   
[Computer use](https://developers.openai.com/api/docs/guides/tools-computer-use)
    *   [Integration recipes](https://developers.openai.com/api/docs/guides/tools-computer-use-integration)
    *   [Driver reference](https://developers.openai.com/api/docs/guides/tools-computer-use-sdk-drivers)

*   [Apply Patch](https://developers.openai.com/api/docs/guides/tools-apply-patch)
*   [Local shell](https://developers.openai.com/api/docs/guides/tools-local-shell)
*   [Code interpreter](https://developers.openai.com/api/docs/guides/tools-code-interpreter)

### Media

*   [Image generation](https://developers.openai.com/api/docs/guides/tools-image-generation)

*   [Overview](https://developers.openai.com/api/docs/guides/audio)

### GPT-Live

*   [Getting started](https://developers.openai.com/api/docs/guides/live)
*   [Prompting](https://developers.openai.com/api/docs/guides/live-prompting)
*   [Managing sessions](https://developers.openai.com/api/docs/guides/live-conversations)
*   [Delegation and tools](https://developers.openai.com/api/docs/guides/live-delegation)
*   [Migrate to GPT-Live](https://developers.openai.com/api/docs/guides/live-migration)
*   [Partner integrations](https://developers.openai.com/api/docs/guides/live-partner-integrations)

### Realtime API

*   [Getting started](https://developers.openai.com/api/docs/guides/realtime)
*   [Prompting](https://developers.openai.com/api/docs/guides/voice-prompting)
*   [Managing conversations](https://developers.openai.com/api/docs/guides/realtime-conversations)
*   [Voice activity detection](https://developers.openai.com/api/docs/guides/realtime-vad)
*   [Tools and MCP](https://developers.openai.com/api/docs/guides/realtime-mcp)

### Build with voice

*   [Voice agents](https://developers.openai.com/api/docs/guides/voice-agents)
*   [Connect voice to Decisions](https://developers.openai.com/api/docs/guides/decisions-voice)
*   [Custom voices](https://developers.openai.com/api/docs/guides/custom-voices)
*   [Cost optimization](https://developers.openai.com/api/docs/guides/voice-latency-cost)

### Connections

*   [WebRTC](https://developers.openai.com/api/docs/guides/voice-webrtc)
*   [WebRTC with WARP](https://developers.openai.com/api/docs/guides/realtime-webrtc-warp)
*   [WebSockets](https://developers.openai.com/api/docs/guides/voice-websockets)
*   [Telephony and SIP](https://developers.openai.com/api/docs/guides/voice-sip)
*   [Server-side controls](https://developers.openai.com/api/docs/guides/voice-server-controls)

### Audio processing

*   [File transcription](https://developers.openai.com/api/docs/guides/speech-to-text)
*   [Live transcription](https://developers.openai.com/api/docs/guides/realtime-transcription)
*   [Live translation](https://developers.openai.com/api/docs/guides/realtime-translation)
*   [Text to speech](https://developers.openai.com/api/docs/guides/text-to-speech)
*   [Audio in Chat Completions](https://developers.openai.com/api/docs/guides/audio-chat-completions)

### Go live

*   [Production best practices](https://developers.openai.com/api/docs/guides/production-best-practices)
*   [Deployment checklist](https://developers.openai.com/api/docs/guides/deployment-checklist)

### Performance and quality

*   [Fast mode](https://developers.openai.com/api/docs/guides/fast-mode)
*   [Ultrafast mode](https://developers.openai.com/api/docs/guides/ultrafast-mode)
*   [Latency optimization](https://developers.openai.com/api/docs/guides/latency-optimization)
*   [Predicted Outputs](https://developers.openai.com/api/docs/guides/predicted-outputs)
*   [Accuracy optimization](https://developers.openai.com/api/docs/guides/optimizing-llm-accuracy)

### Evaluation

*   [Evaluation best practices](https://developers.openai.com/api/docs/guides/evaluation-best-practices)
*   [Promptfoo](https://developers.openai.com/api/docs/guides/promptfoo-codex)

### Cost and throughput

*   [Cost optimization](https://developers.openai.com/api/docs/guides/cost-optimization)
*   
[Prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching)
    *   [Prompt cache diagnostics](https://developers.openai.com/api/docs/guides/prompt-caching/diagnostics)

*   [Batch](https://developers.openai.com/api/docs/guides/batch)
*   [Flex processing](https://developers.openai.com/api/docs/guides/flex-processing)

### Safety and governance

*   [Safety best practices](https://developers.openai.com/api/docs/guides/safety-best-practices)
*   [Red teaming](https://developers.openai.com/api/docs/guides/red-teaming)
*   [Daybreak](https://developers.openai.com/api/docs/guides/daybreak)
*   
Safety checks
    *   [Safety classifiers](https://developers.openai.com/api/docs/guides/safety-checks)
    *   [Cybersecurity checks](https://developers.openai.com/api/docs/guides/safety-checks/cybersecurity)
    *   [Misalignment monitoring](https://developers.openai.com/api/docs/guides/safety-checks/misalignment-monitoring)

*   [Enforcement notifications](https://developers.openai.com/api/docs/guides/safety-enforcement)
*   [Under-18 guidance](https://developers.openai.com/api/docs/guides/safety-checks/under-18-api-guidance)
*   [CSAM guidance](https://developers.openai.com/api/docs/guides/csam-guidance)
*   [Content provenance](https://developers.openai.com/api/docs/guides/content-provenance)
*   [Your data](https://developers.openai.com/api/docs/guides/your-data)
*   [Private Safety Processing](https://developers.openai.com/api/docs/guides/private-safety-processing)
*   [Permissions](https://developers.openai.com/api/docs/guides/rbac)

### Infrastructure and access

*   
[Terraform provider](https://developers.openai.com/api/docs/guides/terraform)
    *   [Overview](https://developers.openai.com/api/docs/guides/terraform)
    *   [Projects and access](https://developers.openai.com/api/docs/guides/terraform/projects-and-access)
    *   [Service accounts](https://developers.openai.com/api/docs/guides/terraform/service-accounts)
    *   [Rate limits and spend](https://developers.openai.com/api/docs/guides/terraform/rate-limits-and-spend)
    *   [Model, tool, and data controls](https://developers.openai.com/api/docs/guides/terraform/project-controls)
    *   [Import and reconciliation](https://developers.openai.com/api/docs/guides/terraform/import-and-reconcile)

*   [Private Link](https://developers.openai.com/api/docs/guides/private-link)
*   [IP allowlist](https://developers.openai.com/api/docs/guides/ip-allowlist)
*   [Organization blocking](https://developers.openai.com/api/docs/guides/organization-blocking)
*   [Mutual TLS](https://developers.openai.com/api/docs/guides/mutual-tls)
*   
[Workload identity federation](https://developers.openai.com/api/docs/guides/workload-identity-federation)
    *   [Federation rules](https://developers.openai.com/api/docs/guides/workload-identity-federation/federation-rules)
    *   [X.509 certificates](https://developers.openai.com/api/docs/guides/workload-identity-federation/x509)
    *   [Kubernetes](https://developers.openai.com/api/docs/guides/workload-identity-federation/kubernetes)
    *   [AWS](https://developers.openai.com/api/docs/guides/workload-identity-federation/aws)
    *   [Microsoft Azure](https://developers.openai.com/api/docs/guides/workload-identity-federation/microsoft-azure)
    *   [Google Cloud](https://developers.openai.com/api/docs/guides/workload-identity-federation/google-cloud)
    *   [Oracle Cloud Infrastructure](https://developers.openai.com/api/docs/guides/workload-identity-federation/oracle-cloud)
    *   [GitHub Actions](https://developers.openai.com/api/docs/guides/workload-identity-federation/github-actions)
    *   [SPIFFE](https://developers.openai.com/api/docs/guides/workload-identity-federation/spiffe)
    *   [Idira](https://developers.openai.com/api/docs/guides/workload-identity-federation/idira)

*   [IP egress ranges](https://developers.openai.com/api/docs/guides/ip-addresses)
*   [Amazon Bedrock](https://developers.openai.com/api/docs/guides/amazon-bedrock)

### Operations

*   [Rate limits](https://developers.openai.com/api/docs/guides/rate-limits)
*   [Spend limits](https://developers.openai.com/api/docs/guides/spend-limits)
*   [Admin APIs](https://developers.openai.com/api/docs/guides/admin-apis)
*   [Error codes](https://developers.openai.com/api/docs/guides/error-codes)

[Overview](https://developers.openai.com/chatgpt) Plugins  Sign in with ChatGPT  Commerce  Ads  Workspace Agents [ChatGPT + Codex user docs](https://learn.chatgpt.com/docs)[Use cases](https://learn.chatgpt.com/use-cases)

Docs Overview

*   [Home](https://developers.openai.com/plugins)
*   [Quickstart](https://developers.openai.com/plugins/quickstart)

### Core concepts

*   [Plugin architecture](https://developers.openai.com/plugins/concepts/plugins)
*   [Skills](https://developers.openai.com/plugins/concepts/skills)
*   [MCP server](https://developers.openai.com/plugins/concepts/mcp-server)

### Plan

*   [Brainstorm use cases](https://developers.openai.com/plugins/plan/use-case)
*   [Define tools](https://developers.openai.com/plugins/plan/tools)

### Build

*   [Build an MCP server](https://developers.openai.com/plugins/build/mcp-server)
*   [Add UI to your MCP server (optional)](https://developers.openai.com/plugins/build/chatgpt-ui)
*   [Add events to your MCP server (optional)](https://developers.openai.com/plugins/build/mcp-events)
*   [Extensions](https://developers.openai.com/plugins/build/extensions)
*   [Authenticate users](https://developers.openai.com/plugins/build/auth)
*   [Build skills](https://developers.openai.com/plugins/build/skills)
*   [Package your plugin](https://developers.openai.com/plugins/build/plugins)
*   [Examples](https://developers.openai.com/plugins/build/examples)

### Test and publish

*   [Connect and test your plugin](https://developers.openai.com/plugins/deploy/connect-chatgpt)
*   [Submit and publish](https://developers.openai.com/plugins/deploy/submission)
*   [Submit desktop-only plugins](https://developers.openai.com/plugins/deploy/desktop-only)
*   [Submission error reference](https://developers.openai.com/plugins/deploy/submission-errors)

### Conversion specs

*   [Restaurant reservation spec](https://developers.openai.com/plugins/guides/restaurant-reservation-conversion-spec)
*   [Get Quote spec](https://developers.openai.com/plugins/guides/local-services-request-quote-conversion-spec)
*   [Product checkout spec](https://developers.openai.com/plugins/guides/product-checkout-conversion-spec)

### Guides

*   [UI guidelines](https://developers.openai.com/plugins/concepts/ui-guidelines)
*   [Optimize Metadata](https://developers.openai.com/plugins/guides/optimize-metadata)
*   [Submit a Claude Code plugin](https://developers.openai.com/plugins/guides/submit-claude-plugin)
*   [Security & Privacy](https://developers.openai.com/plugins/guides/security-privacy)
*   [Troubleshooting](https://developers.openai.com/plugins/deploy/troubleshooting)

### Resources

*   [Changelog](https://developers.openai.com/plugins/changelog)
*   [Plugin guidelines](https://developers.openai.com/plugins/plugin-guidelines)
*   [MCP server review requirements](https://developers.openai.com/plugins/deploy/app-review)
*   [Plugin UI reference](https://developers.openai.com/plugins/reference)
*   [Checkout API reference](https://developers.openai.com/plugins/build/monetization)

*   [Home](https://developers.openai.com/siwc)
*   [Quickstart](https://developers.openai.com/siwc/quickstart)
*   [Request a client ID](https://developers.openai.com/siwc/request-client-id)

### Identity

*   [On your website](https://developers.openai.com/siwc/website)
*   [In your ChatGPT plugin](https://developers.openai.com/siwc/chatgpt-plugin)

### ChatGPT plan usage

*   [Overview](https://developers.openai.com/siwc/token-sharing-open-source)
*   [UI/UX guidelines](https://developers.openai.com/siwc/ui-ux-guidelines)
*   [Registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
*   [Accounts and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)
*   [Models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)
*   [Codex app-server](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
*   [Self-hosted VMs](https://developers.openai.com/siwc/token-sharing-open-source/self-hosted-vms)
*   [Token reference](https://developers.openai.com/siwc/token-sharing-open-source/token-reference)
*   [Errors and recovery](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery)
*   [Preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

*   [Home](https://developers.openai.com/commerce)

### Guides

*   [Get started](https://developers.openai.com/commerce/guides/get-started)
*   [Best practices](https://developers.openai.com/commerce/guides/best-practices)

### File Upload

*   [Overview](https://developers.openai.com/commerce/specs/file-upload/overview)
*   [Products](https://developers.openai.com/commerce/specs/file-upload/products)

### API

*   [Overview](https://developers.openai.com/commerce/specs/api/overview)
*   [Feeds](https://developers.openai.com/commerce/specs/api/feeds)
*   [Products](https://developers.openai.com/commerce/specs/api/products)
*   [Promotions](https://developers.openai.com/commerce/specs/api/promotions)

*   [Ads Overview](https://developers.openai.com/ads)

### Measurement

*   [Measurement Pixel](https://developers.openai.com/ads/measurement-pixel)
*   [Multiple Pixels (Advanced)](https://developers.openai.com/ads/multiple-pixels)
*   [Image Tag](https://developers.openai.com/ads/image-tag)
*   [Conversions API](https://developers.openai.com/ads/conversions-api)
*   [Supported Events](https://developers.openai.com/ads/supported-events)

### Advertiser API

*   [Overview](https://developers.openai.com/ads/api-overview)
*   [API Partner Setup](https://developers.openai.com/ads/api-partner-setup)
*   [Campaign Management](https://developers.openai.com/ads/campaign-management)
*   [Bidding & Budgets](https://developers.openai.com/ads/bidding-and-budgets)
*   [Targeting](https://developers.openai.com/ads/campaign-targeting)
*   [Product Feeds](https://developers.openai.com/ads/product-feeds)
*   [Hotel Feeds (limited beta)](https://developers.openai.com/ads/hotel-feeds)
*   [Conversion Tracking](https://developers.openai.com/ads/conversion-tracking)
*   [Reporting](https://developers.openai.com/ads/reporting)
*   [Troubleshooting](https://developers.openai.com/ads/troubleshooting)
*   [Account Management](https://developers.openai.com/ads/account-management)

### API Reference

*   [Authentication](https://developers.openai.com/ads/api-reference/authentication)
*   [Ad Account](https://developers.openai.com/ads/api-reference/ad-account)
*   [Audit Logs](https://developers.openai.com/ads/api-reference/audit-logs)
*   [Campaigns](https://developers.openai.com/ads/api-reference/campaigns)
*   [Ad Groups](https://developers.openai.com/ads/api-reference/ad-groups)
*   [Ads](https://developers.openai.com/ads/api-reference/ads)
*   [Insights](https://developers.openai.com/ads/api-reference/insights)
*   [Files](https://developers.openai.com/ads/api-reference/files)
*   [Conversion Setup](https://developers.openai.com/ads/api-reference/conversion-setup)

*   [Home](https://developers.openai.com/workspace-agents)

### Get started

*   [Trigger workspace agent runs](https://developers.openai.com/workspace-agents/trigger-runs)
*   [Authenticate with Workspace Agent access tokens](https://developers.openai.com/workspace-agents/authentication)

 Overview  Features  Configuration  Developers  Security  Administration  Use Cases  Resources 

Docs Overview

*   [Home](https://developers.openai.com/codex)

### Get started

*   [Quickstart](https://developers.openai.com/codex/quickstart)
*   [Use ChatGPT](https://developers.openai.com/codex/use-chatgpt)
*   [Get started with Work](https://developers.openai.com/codex/get-started-with-work)
*   [Meet dots](https://developers.openai.com/codex/dots?nav=overview)
*   [Import from another agent](https://developers.openai.com/codex/import)

### Foundations

*   [Prompting](https://developers.openai.com/codex/prompting)
*   [Model selection](https://developers.openai.com/codex/model-selection)
*   [Personalize ChatGPT](https://developers.openai.com/codex/personalize)
*   [Skills & Plugins](https://developers.openai.com/codex/skills-and-plugins)
*   [Permissions](https://developers.openai.com/codex/permission-modes)

### Explore

*   [What's new](https://developers.openai.com/codex/whats-new)
*   [Models](https://developers.openai.com/codex/models)
*   [Pricing](https://developers.openai.com/codex/pricing)
*   [Glossary](https://developers.openai.com/codex/glossary)

### Available on

*   [ChatGPT desktop app](https://developers.openai.com/codex/app)
*   [ChatGPT mobile app](https://developers.openai.com/codex/mobile)
*   [ChatGPT on the web](https://developers.openai.com/codex/web)
*   [Codex CLI](https://developers.openai.com/codex/cli)
*   [Codex IDE extension](https://developers.openai.com/codex/ide)
*   [Codex Cloud](https://developers.openai.com/codex/cloud)

### Releases

*   [Changelog](https://developers.openai.com/codex/changelog)
*   [Feature Maturity](https://developers.openai.com/codex/feature-maturity)
*   [Open Source](https://developers.openai.com/codex/open-source)

*   [Overview](https://developers.openai.com/codex/features)

### Workflows

*   [Projects and chats](https://developers.openai.com/codex/projects)
*   [Build plugins](https://developers.openai.com/codex/build-plugins)
*   [Visualizations](https://developers.openai.com/codex/visualizations)
*   [Scheduled tasks](https://developers.openai.com/codex/automations)
*   [Long-running work](https://developers.openai.com/codex/long-running-work)
*   [Notifications](https://developers.openai.com/codex/notifications)
*   [Pets](https://developers.openai.com/codex/pets)
*   [Codex Micro](https://developers.openai.com/codex/features/codex-micro)

### Capabilities

*   [Browser](https://developers.openai.com/codex/browser)
*   [Computer use](https://developers.openai.com/codex/computer-use)
*   [Voice](https://developers.openai.com/codex/features/voice)
*   [Composer Predictions](https://developers.openai.com/codex/features/composer-predictions)
*   [Plugins](https://developers.openai.com/codex/plugins)
*   [Sign in with ChatGPT](https://developers.openai.com/codex/sign-in-with-chatgpt)
*   [Web search](https://developers.openai.com/codex/web-search)
*   [Image generation](https://developers.openai.com/codex/image-generation)
*   [Image inputs](https://developers.openai.com/codex/image-inputs)
*   [Appshots](https://developers.openai.com/codex/appshots)
*   [Browser extension](https://developers.openai.com/codex/chrome-extension)
*   [Work with files](https://developers.openai.com/codex/artifacts-viewer)

### Sites

*   [Overview](https://developers.openai.com/codex/sites)
*   [Create a site](https://developers.openai.com/codex/sites/creating-a-site)
*   [Data and connections](https://developers.openai.com/codex/sites/data-and-connections)
*   [Publish and share](https://developers.openai.com/codex/sites/publishing-and-sharing)
*   [Manage a site](https://developers.openai.com/codex/sites/managing-a-site)

### dots

*   [Meet dots](https://developers.openai.com/codex/dots?nav=features)
*   [Getting started](https://developers.openai.com/codex/dots/getting-started)
*   [Messaging](https://developers.openai.com/codex/dots/channels)
*   [Tasks and memory](https://developers.openai.com/codex/dots/tasks-and-memory)
*   [Computers and apps](https://developers.openai.com/codex/dots/computers-and-apps)
*   [Controls](https://developers.openai.com/codex/dots/controls)

### ChatGPT Space

*   [Overview](https://developers.openai.com/codex/space)
*   [Getting started](https://developers.openai.com/codex/space/getting-started)
*   [Pages](https://developers.openai.com/codex/space/pages)
*   [Work with agents](https://developers.openai.com/codex/space/agents)
*   [Collaboration](https://developers.openai.com/codex/space/collaboration)

### Reference

*   [Commands](https://developers.openai.com/codex/reference/commands)
*   [Slash commands](https://developers.openai.com/codex/reference/slash-commands)
*   [Settings](https://developers.openai.com/codex/reference/settings)
*   [Troubleshooting](https://developers.openai.com/codex/reference/troubleshooting)

*   [Overview](https://developers.openai.com/codex/configuration)

### Customization

*   [Overview](https://developers.openai.com/codex/customization/overview)
*   [Memories](https://developers.openai.com/codex/customization/memories)
*   [Computer History](https://developers.openai.com/codex/customization/computer-history)

### Config file

*   [Config Basics](https://developers.openai.com/codex/config-file/config-basic)
*   [Advanced Config](https://developers.openai.com/codex/config-file/config-advanced)
*   [Config Reference](https://developers.openai.com/codex/config-file/config-reference)
*   [Environment Variables](https://developers.openai.com/codex/config-file/environment-variables)
*   [Sample Config](https://developers.openai.com/codex/config-file/config-sample)

### Agent configuration

*   [AGENTS.md](https://developers.openai.com/codex/agent-configuration/agents-md)
*   [Subagents](https://developers.openai.com/codex/agent-configuration/subagents)
*   [Speed](https://developers.openai.com/codex/agent-configuration/speed)
*   [Rules](https://developers.openai.com/codex/agent-configuration/rules)

### Extend ChatGPT and Codex

*   [Record & Replay](https://developers.openai.com/codex/extend/record-and-replay)
*   [MCP](https://developers.openai.com/codex/extend/mcp)

### Linux

*   [Desktop app](https://developers.openai.com/codex/linux/linux-app)

### Windows

*   [Desktop app](https://developers.openai.com/codex/windows/windows-app)
*   [Windows sandbox](https://developers.openai.com/codex/windows/windows-sandbox)
*   [WSL](https://developers.openai.com/codex/windows/wsl)

*   [Overview](https://developers.openai.com/codex/developers)

### Development workflows

*   [Code review](https://developers.openai.com/codex/code-review)
*   [Integrated terminal](https://developers.openai.com/codex/integrated-terminal)

### Extend and automate

*   [Build skills](https://developers.openai.com/codex/build-skills)
*   [Site tools (WebMCP)](https://developers.openai.com/codex/webmcp)
*   [Annotations Extensibility](https://developers.openai.com/codex/annotations-extensibility)
*   [Hooks](https://developers.openai.com/codex/hooks)

### Environments

*   [Modes](https://developers.openai.com/codex/environments/modes)
*   [Local environments](https://developers.openai.com/codex/environments/local-environment)
*   [Git worktrees](https://developers.openai.com/codex/environments/git-worktrees)

### Codex Cloud

*   [Cloud environments](https://developers.openai.com/codex/environments/cloud-environments)

### Build with Codex

*   [Codex SDK](https://developers.openai.com/codex/codex-sdk)
*   [App Server](https://developers.openai.com/codex/app-server)
*   [GitHub Action](https://developers.openai.com/codex/github-action)
*   [Non-interactive mode](https://developers.openai.com/codex/non-interactive-mode)

### Third-party integrations

*   [GitHub](https://developers.openai.com/codex/third-party/github)
*   [GitLab (Beta)](https://developers.openai.com/codex/third-party/gitlab)
*   [Slack](https://developers.openai.com/codex/third-party/slack)
*   [Linear](https://developers.openai.com/codex/third-party/linear)

### Reference

*   [CLI customization](https://developers.openai.com/codex/cli-customization)
*   [Developer commands](https://developers.openai.com/codex/developer-commands)
*   [Developer settings](https://developers.openai.com/codex/developer-settings)

*   [Overview](https://developers.openai.com/codex/security-administration)

### Permissions

*   [Profiles](https://developers.openai.com/codex/permissions)
*   [Sandboxing](https://developers.openai.com/codex/sandboxing)
*   [Auto-review](https://developers.openai.com/codex/sandboxing/auto-review)
*   [Agent approvals & security](https://developers.openai.com/codex/agent-approvals-security)

### Codex Security

*   [Overview](https://developers.openai.com/codex/security)
*   
Codex Security Cloud
    *   [Setup](https://developers.openai.com/codex/security/setup)
    *   [Security Review](https://developers.openai.com/codex/security/security-review)
    *   [Improving the threat model](https://developers.openai.com/codex/security/threat-model)
    *   [FAQ](https://developers.openai.com/codex/security/faq)

*   
Codex Security plugin
    *   [Quickstart](https://developers.openai.com/codex/security/plugin)
    *   [Run a security scan](https://developers.openai.com/codex/security/plugin/scans)
    *   [Run a deep scan](https://developers.openai.com/codex/security/plugin/deep-scans)
    *   [Review code changes](https://developers.openai.com/codex/security/plugin/code-changes)
    *   [Use the Security workbench](https://developers.openai.com/codex/security/plugin/workbench)
    *   [Triage a backlog](https://developers.openai.com/codex/security/plugin/triage-backlog)
    *   [Fix findings](https://developers.openai.com/codex/security/plugin/fix-findings)
    *   [Propose security hardening](https://developers.openai.com/codex/security/plugin/security-hardening)
    *   [Write vulnerability reports](https://developers.openai.com/codex/security/plugin/vulnerability-reports)
    *   [Export and track findings](https://developers.openai.com/codex/security/plugin/export-findings)
    *   [Changelog](https://developers.openai.com/codex/security/plugin/changelog)

*   
Codex Security CLI
    *   [Quickstart](https://developers.openai.com/codex/security/cli)
    *   [Run bulk scans](https://developers.openai.com/codex/security/cli/bulk-scans)
    *   [Run scans in CI](https://developers.openai.com/codex/security/cli/ci)
    *   [GitLab CI/CD](https://developers.openai.com/codex/security/cli/ci/gitlab)
    *   [Reference](https://developers.openai.com/codex/security/cli/reference)
    *   [FAQ](https://developers.openai.com/codex/security/cli/faq)

*   [Codex Security SDK](https://developers.openai.com/codex/security/sdk)

### Cyber safety

*   [Models & Trusted Access](https://developers.openai.com/codex/cyber-safety)
*   [Recommended configuration](https://developers.openai.com/codex/cyber-safety/recommended-configuration)

*   [Overview](https://developers.openai.com/codex/administration)

### Getting started

*   [Admin rollout guide](https://developers.openai.com/codex/enterprise/admin-setup)
*   [Admin plugin](https://developers.openai.com/codex/enterprise/admin-plugin)

### Feature Configuration

*   [Dots](https://developers.openai.com/codex/dots/controls#for-workspace-admins)
*   [Space](https://developers.openai.com/codex/space/collaboration#for-workspace-admins)
*   [Sites](https://developers.openai.com/codex/enterprise/sites)
*   [Teams and Team Tasks](https://developers.openai.com/codex/enterprise/teams#for-workspace-admins)
*   [ChatGPT in Slack and Teams](https://developers.openai.com/codex/enterprise/chatgpt-slack-and-teams)
*   [Workspace connections](https://developers.openai.com/codex/enterprise/shared-connections)
*   [Local computer access for Work Cloud and dots](https://developers.openai.com/codex/enterprise/cloud-local-access)

### Identity and access

*   [Authentication overview](https://developers.openai.com/codex/auth)
*   [Groups and provisioning](https://developers.openai.com/codex/enterprise/groups-and-provisioning)
*   [Dynamic groups](https://developers.openai.com/codex/enterprise/dynamic-groups)
*   [User lifecycle management](https://developers.openai.com/codex/enterprise/user-lifecycle)
*   [Roles and workspace permissions](https://developers.openai.com/codex/enterprise/roles-and-workspace-permissions)
*   [Personal access tokens](https://developers.openai.com/codex/enterprise/access-tokens)
*   [Service accounts](https://developers.openai.com/codex/enterprise/service-accounts)

### Deployment and configuration

*   [Windows app and CLI deployment](https://developers.openai.com/codex/enterprise/windows-deployment)
*   [Manage app updates](https://developers.openai.com/codex/enterprise/manage-app-updates)
*   [Managed configuration](https://developers.openai.com/codex/enterprise/managed-configuration)
*   [Codex security cookbook](https://developers.openai.com/codex/enterprise/security-cookbook)
*   [Remote connections](https://developers.openai.com/codex/remote-connections)
*   [Workspace model availability](https://developers.openai.com/codex/enterprise/workspace-model-availability)
*   [Amazon Bedrock](https://developers.openai.com/codex/amazon-bedrock)
*   [Bedrock GovCloud configuration](https://developers.openai.com/codex/enterprise/govcloud-configuration)
*   
Connect to a gateway
    *   [Sign in with ChatGPT](https://developers.openai.com/codex/enterprise/sign-in-with-chatgpt-through-a-gateway)
    *   [Use API/provider credentials](https://developers.openai.com/codex/enterprise/connect-to-a-gateway)

*   [Roll out a gateway](https://developers.openai.com/codex/enterprise/roll-out-a-gateway)
*   [Gateway compatibility](https://developers.openai.com/codex/enterprise/gateway-compatibility)
*   [Bedrock through LiteLLM](https://developers.openai.com/codex/enterprise/bedrock-through-litellm)

### ChatGPT Work

*   [Overview](https://developers.openai.com/codex/enterprise/chatgpt-work-overview)
*   [Cloud security](https://developers.openai.com/codex/enterprise/chatgpt-work-cloud-security)
*   [Local security](https://developers.openai.com/codex/enterprise/chatgpt-work-local-security)
*   [Usage and cost](https://developers.openai.com/codex/enterprise/chatgpt-work-usage-and-cost)
*   [Admin FAQ](https://developers.openai.com/codex/enterprise/work-admin-faq)

### Collaboration and sharing

*   [GPTs and sharing](https://developers.openai.com/codex/enterprise/gpts-and-sharing)

### Plugins and connections

*   [Plugin controls](https://developers.openai.com/codex/enterprise/apps-and-connectors)
*   [Plugin management](https://developers.openai.com/codex/enterprise/plugin-management)
*   [Skill controls](https://developers.openai.com/codex/enterprise/skills)
*   [Migrate custom GPTs to plugins](https://developers.openai.com/codex/migrate-custom-gpts)

### Usage and analytics

*   [Workspace analytics](https://developers.openai.com/codex/enterprise/workspace-analytics)
*   [Usage Insights](https://developers.openai.com/codex/enterprise/usage-insights)
*   [Analytics API](https://developers.openai.com/codex/enterprise/analytics-api)

### Security and compliance

*   [Governance](https://developers.openai.com/codex/enterprise/governance)
*   [Agent security](https://developers.openai.com/codex/enterprise/agent-security)
*   [Prisma AIRS](https://developers.openai.com/codex/enterprise/prisma-airs)
*   [HIPAA configuration](https://developers.openai.com/codex/hipaa-configuration)
*   [Compliance API and audit events](https://developers.openai.com/codex/enterprise/compliance-api)

*   [Explore use cases](https://developers.openai.com/codex/use-cases)
*   [Collections](https://developers.openai.com/codex/use-cases/collections)

*   [Home](https://developers.openai.com/codex/resources)
*   [Videos](https://developers.openai.com/codex/videos)
*   [Showcase](https://developers.openai.com/showcase)
*   [OpenAI Academy](https://openai.com/academy/)
*   [Online trainings](https://academy.openai.com/home/events)

### Community

*   [Codex Ambassadors](https://developers.openai.com/community/codex-ambassadors)
*   [Codex for Students](https://developers.openai.com/community/students)
*   [Codex for Open Source](https://developers.openai.com/community/codex-for-oss)
*   [Events](https://luma.com/codex-community?utm_source=oaidevs)

### Blog

*   [Company blog](https://openai.com/news/)
*   [Developer blog](https://developers.openai.com/blog)

*   [Explore use cases](https://developers.openai.com/codex/use-cases)
*   [Collections](https://developers.openai.com/codex/use-cases/collections)

*   [Home](https://developers.openai.com/codex/resources)
*   [Videos](https://developers.openai.com/codex/videos)
*   [Showcase](https://developers.openai.com/showcase)
*   [OpenAI Academy](https://openai.com/academy/)
*   [Online trainings](https://academy.openai.com/home/events)

### Community

*   [Codex Ambassadors](https://developers.openai.com/community/codex-ambassadors)
*   [Codex for Students](https://developers.openai.com/community/students)
*   [Codex for Open Source](https://developers.openai.com/community/codex-for-oss)
*   [Events](https://luma.com/codex-community?utm_source=oaidevs)

### Blog

*   [Company blog](https://openai.com/news/)
*   [Developer blog](https://developers.openai.com/blog)

[Showcase](https://developers.openai.com/showcase) Blog  Cookbook  Learn  Community 

Docs Showcase

*   [All posts](https://developers.openai.com/blog)

### Recent

*   [Designing failover across OpenAI model providers](https://developers.openai.com/blog/designing-failover-across-openai-model-providers)
*   [Maintaining software with the GPT-6 family](https://developers.openai.com/blog/maintaining-software-with-the-gpt-6-family)
*   [Bringing my LED display to life with GPT-Live-1 and Codex](https://developers.openai.com/blog/bringing-my-led-display-to-life)
*   [Rethinking skills and prompts for GPT-6 Astra](https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra)
*   [Architectural visualization with Astra](https://developers.openai.com/blog/architectural-visualization-with-astra)

### Topics

*   [General](https://developers.openai.com/blog/topic/general)
*   [API](https://developers.openai.com/blog/topic/api)
*   [Apps SDK](https://developers.openai.com/blog/topic/apps-sdk)
*   [Audio](https://developers.openai.com/blog/topic/audio)
*   [Codex](https://developers.openai.com/blog/topic/codex)
*   [Life sciences](https://developers.openai.com/blog/topic/life-sciences)

*   [Home](https://developers.openai.com/cookbook)

### Topics

*   [Sign-in with ChatGPT](https://developers.openai.com/cookbook/topic/sign-in-with-chatgpt)
*   [Agents](https://developers.openai.com/cookbook/topic/agents)
*   [Evals](https://developers.openai.com/cookbook/topic/evals)
*   [Multimodal](https://developers.openai.com/cookbook/topic/multimodal)
*   [Text](https://developers.openai.com/cookbook/topic/text)
*   [Guardrails](https://developers.openai.com/cookbook/topic/guardrails)
*   [Optimization](https://developers.openai.com/cookbook/topic/optimization)
*   [ChatGPT](https://developers.openai.com/cookbook/topic/chatgpt)
*   [Codex](https://developers.openai.com/cookbook/topic/codex)
*   [gpt-oss](https://developers.openai.com/cookbook/topic/gpt-oss)

### Contribute

*   [Cookbook on GitHub](https://github.com/openai/openai-cookbook)

*   [Home](https://developers.openai.com/learn)
*   [OpenAI Developers plugin](https://developers.openai.com/learn/developers-codex-plugin)
*   [Docs MCP](https://developers.openai.com/learn/docs-mcp)

### Categories

*   [Demo apps](https://developers.openai.com/learn/code)
*   [Videos](https://developers.openai.com/learn/videos)

### Topics

*   [Agents](https://developers.openai.com/learn/agents)
*   [Audio & Voice](https://developers.openai.com/learn/audio)
*   [Computer Use](https://developers.openai.com/learn/cua)
*   [Codex](https://developers.openai.com/learn/codex)
*   [Evals](https://developers.openai.com/learn/evals)
*   [gpt-oss](https://developers.openai.com/learn/gpt-oss)
*   [Fine-tuning](https://developers.openai.com/learn/fine-tuning)
*   [Image generation](https://developers.openai.com/learn/imagegen)
*   [Scaling](https://developers.openai.com/learn/scaling)
*   [Tools](https://developers.openai.com/learn/tools)
*   [Video generation](https://developers.openai.com/learn/videogen)

*   [Community](https://developers.openai.com/community)

### Programs

*   
[Codex Ambassadors](https://developers.openai.com/community/codex-ambassadors)
    *   [Meet the ambassadors](https://developers.openai.com/community/codex-ambassadors/directory)

*   [Codex for Students](https://developers.openai.com/community/students)
*   [Codex for Open Source](https://developers.openai.com/community/codex-for-oss)
*   [OpenAI for Startups](https://openai.com/business/why-openai/startups/)

### Spaces

*   [Events](https://luma.com/codex-community?utm_source=oaidevs)
*   [Developer Forum](https://community.openai.com/)
*   [Discord](https://discord.com/invite/openai)
*   [Reddit](https://www.reddit.com/r/OpenAI/)
*   [X](https://x.com/OpenAIDevs)

[API Dashboard](https://platform.openai.com/login)

[Try ChatGPT](https://chatgpt.com/)

*   [Overview](https://developers.openai.com/api/docs/guides/tools)
*   [Function calling](https://developers.openai.com/api/docs/guides/function-calling)

### Search and retrieval

*   [Web search](https://developers.openai.com/api/docs/guides/tools-web-search)
*   [File search](https://developers.openai.com/api/docs/guides/tools-file-search)
*   [Retrieval](https://developers.openai.com/api/docs/guides/retrieval)

### Connect tools and data

*   [MCP servers](https://developers.openai.com/api/docs/guides/tools-connectors-mcp)
*   [Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)

### Build tool workflows

*   [Skills](https://developers.openai.com/api/docs/guides/tools-skills)
*   [Tool search](https://developers.openai.com/api/docs/guides/tools-tool-search)
*   [Programmatic tool calling](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)
*   [Async tool calling](https://developers.openai.com/api/docs/guides/async-tool-calling)

### Computer and code

*   [Shell](https://developers.openai.com/api/docs/guides/tools-shell)
*   
[Computer use](https://developers.openai.com/api/docs/guides/tools-computer-use)
    *   [Integration recipes](https://developers.openai.com/api/docs/guides/tools-computer-use-integration)
    *   [Driver reference](https://developers.openai.com/api/docs/guides/tools-computer-use-sdk-drivers)

*   [Apply Patch](https://developers.openai.com/api/docs/guides/tools-apply-patch)
*   [Local shell](https://developers.openai.com/api/docs/guides/tools-local-shell)
*   [Code interpreter](https://developers.openai.com/api/docs/guides/tools-code-interpreter)

### Media

*   [Image generation](https://developers.openai.com/api/docs/guides/tools-image-generation)

Copy Page

[Cookbook example Build a coding agent with GPT-5.1 and the apply_patch tool](https://developers.openai.com/cookbook/examples/build_a_coding_agent_with_gpt-5.1)

# Apply Patch

Allow models to propose structured diffs that your integration applies.

Copy Page

The `apply_patch` tool lets GPT-5.1 create, update, and delete files in your codebase using structured diffs. Instead of just suggesting edits, the model emits patch operations that your application applies and then reports back on, enabling iterative, multi-step code editing workflows.

## When to use

Some common scenarios where you would use apply_patch:

*   **Multi-file refactors** – Rename symbols, extract helpers, or reorganize modules across many files at once.
*   **Bug fixes** – Have the model both diagnose issues and emit precise patches.
*   **Tests & docs generation** – Create new test files, fixtures, and documentation alongside code changes.
*   **Migrations & mechanical edits** – Apply repetitive, structured updates (API migrations, type annotations, formatting fixes, etc.).

If you can describe your repo and desired change in text, apply_patch can usually generate the corresponding diffs.

## Use apply patch tool with Responses API

At a high level, using `apply_patch` with the Responses API looks like this:

1.   **Call the Responses API with the `apply_patch` tool**
    *   Provide the model with context about available files (or a summary) in your `input`, or give the model tools for exploring your file system.
    *   Enable the tool with `tools=[{"type": "apply_patch"}]`.

2.   **Let the model return one or more patch operations**
    *   The Response output includes one or more `apply_patch_call` objects.
    *   Each call describes a single file operation: create, update, or delete.

3.   **Apply patches in your environment**
    *   Run a patch harness or script that: 
        *   Interprets the `operation` diff for each `apply_patch_call`.
        *   Applies the patch to your working directory or repo.
        *   Records whether each patch succeeded and any logs or error messages.

4.   **Report patch results back to the model**
    *   Call the Responses API again, either with `previous_response_id` or by passing back your conversation items into `input`.
    *   Include an `apply_patch_call_output` event for each `call_id`, with a `status` and optional `output` string.
    *   Keep `tools=[{"type": "apply_patch"}]` so the model can continue editing if needed.

5.   **Let the model continue or explain changes**
    *   The model may issue more `apply_patch_call` operations, or
    *   Provide a human-facing explanation of what it changed and why.

## Example: Renaming a function with Apply Patch Tool

**Step 1: Ask the model to plan and emit patches**

Ask the model to plan and emit patches

JavaScript

```
1
2
3
4
5
6
7
8
9const response = await client.responses.create({
  model: "gpt-6-astra",
  input: fileContext,
  tools: [{ type: "apply_patch" }],
});

const patchCalls = response.output.filter(
  (item) => item.type === "apply_patch_call"
);
```

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18
19
20
21
22
23
24
25
26
27
28
29
30
31
32
33
34
35
36
37
38
39
40
41
42from openai import OpenAI

client = OpenAI()

# For brevity, we are including file context in the example input.
# Most agentic use cases should instead equip the model with tools
# for exploring file system state.
RESPONSE_INPUT = """
The user has the following files:
<BEGIN_FILES>
===== lib/fib.py
def fib(n):
    if n <= 1:
        return n
    return fib(n-1) + fib(n-2)

===== run.py
from lib.fib import fib

def main():
  print(fib(42))
<END_FILES>

You are a helpful coding assistant that should assist the user with whatever they
ask.

User query:
Help me rename the fib() function to fibonacci()
"""

response = client.responses.create(
    model="gpt-6-astra",
    input=RESPONSE_INPUT,
    tools=[{"type": "apply_patch"}],
)

# response.output may contain multiple apply_patch_call entries, e.g.:
# - update lib/fib.py
# - update run.py
patch_calls = [
    item.model_dump() for item in response.output if item.type == "apply_patch_call"
]
```

```
1
2
3
4
5
6
7
8response, err := client.Responses.New(ctx, responses.ResponseNewParams{
	Model: "gpt-6-astra",
	Input: responses.ResponseNewParamsInputUnion{OfString: openai.String(responseInput)},
	Tools: []responses.ToolUnionParam{{OfApplyPatch: &responses.ApplyPatchToolParam{}}},
})
if err != nil {
	return err
}
```

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16import com.openai.client.OpenAIClient;
import com.openai.client.okhttp.OpenAIOkHttpClient;
import com.openai.models.responses.ApplyPatchTool;
import com.openai.models.responses.ResponseCreateParams;

ResponseCreateParams params =
    ResponseCreateParams.builder()
        .model("gpt-6-astra")
        .input(
            "Rename fib() to fibonacci() in lib/fib.py and update run.py to use the new name.")
        .addTool(ApplyPatchTool.builder().build())
        .build();

client.responses().create(params).output().stream()
    .flatMap(item -> item.applyPatchCall().stream())
    .forEach(System.out::println);
```

```
1
2
3
4
5
6
7
8
9
10
11require "openai"

client = OpenAI::Client.new
response = client.responses.create(
  model: "gpt-6-astra",
  input: "Rename fib() to fibonacci() in lib/fib.py and update run.py to use the new name.",
  tools: [{ type: :apply_patch }]
)

patch_calls = response.output.select { |item| item.type == :apply_patch_call }
puts(patch_calls)
```

**Example `apply_patch_call` object**

Example apply_patch_call object

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18{
    "id": "apc_08f3d96c87a585390069118b594f7481a088b16cda7d9415fe",
    "type": "apply_patch_call",
    "status": "completed",
    "call_id": "call_Rjsqzz96C5xzPb0jUWJFRTNW",
    "operation": {
        "type": "update_file",
        "diff": "
@@
-def fib(n):
+def fibonacci(n):
    if n <= 1:
        return n
-    return fib(n-1) + fib(n-2)                                                  +    return fibonacci(n-1) + fibonacci(n-2),
",
        "path": "lib/fib.py"
    }
}
```

**Step 2: Apply the patch and send results back**

Apply the patch and return results

JavaScript

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18
19const results = patchCalls.map((call) => {
  const { success, output } = applyOperation(call.operation);

  return {
    type: "apply_patch_call_output",
    call_id: call.call_id,
    status: success ? "completed" : "failed",
    output,
  };
});

const followup = await client.responses.create({
  model: "gpt-6-astra",
  previous_response_id: response.id,
  input: results,
  tools: [{ type: "apply_patch" }],
});

console.log(followup.output_text);
```

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18
19
20
21
22from apply_patch_harness import apply_operation  # your implementation

results = []
for call in patch_calls:
    op = call["operation"]
    success, maybe_log_output = apply_operation(op)

    results.append(
        {
            "type": "apply_patch_call_output",
            "call_id": call["call_id"],
            "status": "completed" if success else "failed",
            "output": maybe_log_output,
        }
    )

followup = client.responses.create(
    model="gpt-6-astra",
    previous_response_id=response.id,
    input=results,
    tools=[{"type": "apply_patch"}],
)
```

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18
19
20
21
22
23
24
25
26
27
28
29
30
31
32
33
34
35
36
37
38for turn := 0; ; turn++ {
	if response.Status != responses.ResponseStatusCompleted {
		return fmt.Errorf("response ended with status %s", response.Status)
	}
	patchCalls := make([]responses.ResponseOutputItemUnion, 0)
	for _, item := range response.Output {
		if item.Type == "apply_patch_call" {
			patchCalls = append(patchCalls, item)
		}
	}
	if len(patchCalls) == 0 {
		fmt.Println(response.OutputText())
		return nil
	}
	if turn == 8 {
		return fmt.Errorf("patch workflow exceeded eight tool turns")
	}
	results := make(responses.ResponseInputParam, 0, len(patchCalls))
	for _, call := range patchCalls {
		success, logOutput := applyOperation(files, call.Operation)
		status := "completed"
		if !success {
			status = "failed"
		}
		result := responses.ResponseInputItemParamOfApplyPatchCallOutput(call.CallID, status)
		result.OfApplyPatchCallOutput.Output = openai.String(logOutput)
		results = append(results, result)
	}
	response, err = client.Responses.New(ctx, responses.ResponseNewParams{
		Model:              "gpt-6-astra",
		PreviousResponseID: openai.String(response.ID),
		Input:              responses.ResponseNewParamsInputUnion{OfInputItemList: results},
		Tools:              []responses.ToolUnionParam{{OfApplyPatch: &responses.ApplyPatchToolParam{}}},
	})
	if err != nil {
		return err
	}
}
```

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18
19
20
21
22
23
24
25
26
27import com.openai.client.OpenAIClient;
import com.openai.client.okhttp.OpenAIOkHttpClient;
import com.openai.models.responses.ApplyPatchTool;
import com.openai.models.responses.ResponseCreateParams;
import com.openai.models.responses.ResponseInputItem;
import java.util.List;

ResponseCreateParams params =
    ResponseCreateParams.builder()
        .model("gpt-6-astra")
        .inputOfResponse(
            List.of(
                ResponseInputItem.ofApplyPatchCallOutput(
                    ResponseInputItem.ApplyPatchCallOutput.builder()
                        .callId(System.getenv("OPENAI_EXAMPLE_APPLY_PATCH_CALL_ID"))
                        .status(ResponseInputItem.ApplyPatchCallOutput.Status.COMPLETED)
                        .output("Patch applied successfully.")
                        .build())))
        .previousResponseId(System.getenv("OPENAI_EXAMPLE_PREVIOUS_RESPONSE_ID"))
        .addTool(ApplyPatchTool.builder().build())
        .build();

client.responses().create(params).output().stream()
    .flatMap(item -> item.message().stream())
    .flatMap(message -> message.content().stream())
    .flatMap(content -> content.outputText().stream())
    .forEach(text -> System.out.println(text.text()));
```

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18
19
20require "openai"

client = OpenAI::Client.new
response_id = ENV.fetch("OPENAI_RESPONSE_ID")
patch_call_id = ENV.fetch("OPENAI_APPLY_PATCH_CALL_ID")
response = client.responses.create(
  model: "gpt-6-astra",
  previous_response_id: response_id,
  input: [
    {
      type: :apply_patch_call_output,
      call_id: patch_call_id,
      status: :completed,
      output: "Patch applied successfully."
    }
  ],
  tools: [{ type: :apply_patch }]
)

puts(response.output_text)
```

If a patch fails (for example, file not found), set `status: "failed"` and include a helpful `output` string so the model can recover:

Report a failed apply_patch call

```
1
2
3
4
5
6{
  "type": "apply_patch_call_output",
  "call_id": "call_cNWm41dB3RyQcLNOVTIPBWZU",
  "status": "failed",
  "output": "Could not apply patch to lib/foo.py — file not found on disk"
}
```

## Apply patch operations

| Operation Type | Purpose | Payload |
| --- | --- | --- |
| `create_file` | Create a new file at `path`. | `diff` is a V4A diff representing the full file contents. |
| `update_file` | Modify an existing file at `path`. | `diff` is a V4A diff with additions, deletions, or replacements. |
| `delete_file` | Remove a file at `path`. | No `diff`; delete the file entirely. |

Your patch harness is responsible for interpreting the V4A diff format and applying changes. For reference implementations, see the [Python Agents SDK](https://github.com/openai/openai-agents-python/blob/main/src/agents/apply_diff.py) or [TypeScript Agents SDK](https://github.com/openai/openai-agents-js/blob/main/packages/agents-core/src/utils/applyDiff.ts) code.

## Implementing the patch harness

When using the `apply_patch` tool, you don’t provide an input schema; the model knows how to construct `operation` objects. Your job is to:

1.   **Parse operations from the Response**
    *   Scan the Response for items with `type: "apply_patch_call"`.
    *   For each call, inspect `operation.type`, `operation.path`, and any potential `diff`.

2.   **Apply file operations**
    *   For `create_file` and `update_file`, apply the V4A diff to the file system or in-memory workspace.
    *   For `delete_file`, remove the file at `path`.
    *   Record whether each operation succeeded and any logs or error messages.

3.   **Return `apply_patch_call_output` events**
    *   For each `call_id`, emit exactly one `apply_patch_call_output` event with: 
        *   `status: "completed"` if the operation was applied successfully.
        *   `status: "failed"` if you encountered an error (include a short human-readable `output` string).

### Safety and robustness

*   **Path validation**: Prevent directory traversal and restrict edits to allowed directories.
*   **Backups**: Consider backing up files (or working in a scratch copy) before applying patches.
*   **Error handling**: Always return a `failed` status with an informative `output` string when patches cannot be applied.
*   **Atomicity**: Decide whether you want “all-or-nothing” semantics (rollback if any patch fails) or per-file success/failure.

## Use the apply patch tool with the Agents SDK

Alternatively, you can use the [Agents SDK](https://developers.openai.com/api/docs/guides/tools#usage-in-the-agents-sdk) to use the apply patch tool. You’ll still have to implement the harness that handles the actual file operations but you can use the `applyDiff` function to handle the diff processing.

Use the apply patch tool with the Agents SDK

JavaScript

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18
19
20
21
22
23
24
25
26
27
28
29
30
31
32
33
34
35
36
37
38
39
40
41
42
43
44
45
46
47
48
49
50
51import { applyDiff, Agent, run, applyPatchTool } from "@openai/agents";

class WorkspaceEditor {
  async createFile(operation) {
    // convert the diff to the file content
    const content = applyDiff("", operation.diff, "create");
    // write the file content to the file system
    return { status: "completed", output: `Created ${operation.path}` };
  }

  async updateFile(operation) {
    // read the file content from the file system
    const current = "";
    // convert the diff to the new file content
    const newContent = applyDiff(current, operation.diff);
    // write the updated file content to the file system
    return { status: "completed", output: `Updated ${operation.path}` };
  }

  async deleteFile(operation) {
    // delete the file from the file system
    return { status: "completed", output: `Deleted ${operation.path}` };
  }
}

const editor = new WorkspaceEditor();

const agent = new Agent({
  name: "Patch Assistant",
  model: "gpt-6-astra",
  instructions:
    "You can edit files inside the /tmp directory using the apply_patch tool.",
  tools: [
    applyPatchTool({
      editor,
      // could also be a function for you to determine if approval is needed
      needsApproval: true,
      onApproval: async (_ctx, _approvalItem) => {
        // create your own approval logic
        return { approve: true };
      },
    }),
  ],
});

const result = await run(
  agent,
  "Create tasks.md with a shopping checklist of 5 entries."
);

console.log(`\nFinal response:\n${result.finalOutput}`);
```

```
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18
19
20
21
22
23
24
25
26
27
28
29
30
31
32
33
34
35
36
37
38
39
40
41
42
43
44
45
46
47
48
49
50
51
52
53
54from agents import Agent, ApplyPatchTool, Runner, apply_diff

class WorkspaceEditor:
    async def create_file(self, operation):
        # convert the diff to the file content
        content = apply_diff("", operation.diff, mode="create")
        # write the file content to the file system
        return {"status": "completed", "output": f"Created {operation.path}"}

    async def update_file(self, operation):
        # read the file content from the file system
        current = ""
        # convert the diff to the new file content
        new_content = apply_diff(current, operation.diff)
        # write the updated file content to the file system
        return {"status": "completed", "output": f"Updated {operation.path}"}

    async def delete_file(self, operation):
        # delete the file from the file system
        return {"status": "completed", "output": f"Deleted {operation.path}"}

editor = WorkspaceEditor()

agent = Agent(
    name="Patch Assistant",
    model="gpt-6-astra",
    instructions="You can edit files inside the /tmp directory using the apply_patch tool.",
    tools=[
        ApplyPatchTool(
            editor=editor,
            # could also be a function for you to determine if approval is needed
            needs_approval=True,
            # Implement your own approval logic
            on_approval=lambda _ctx, _approval_item: {"approve": True},
        ),
    ],
)

async def main():
    result = await Runner.run(
        agent,
        input="Create tasks.md with a shopping checklist of 5 entries.",
    )

    print(f"\nFinal response:\n{result.final_output}")

if __name__ == "__main__":
    import asyncio

    asyncio.run(main())
```

You can find full working examples on GitHub.

[Apply patch tool example - TypeScript Example of how to use the apply patch tool with the Agents SDK in TypeScript](https://github.com/openai/openai-agents-js/blob/main/examples/tools/applyPatch.ts)[Apply patch tool example - Python Example of how to use the apply patch tool with the Agents SDK in Python](https://github.com/openai/openai-agents-python/blob/main/examples/tools/apply_patch.py)
## Handling common errors

Use `status: "failed"` plus a clear `output` message to help the model recover.

File not found Patch conflict

File not found

File not found error

```
1
2
3
4
5
6{
  "type": "apply_patch_call_output",
  "call_id": "call_abc",
  "status": "failed",
  "output": "Error: File not found at path 'lib/baz.py'"
}
```

Patch conflict

Patch conflict error

```
1
2
3
4
5
6{
  "type": "apply_patch_call_output",
  "call_id": "call_abc",
  "status": "failed",
  "output": "Error: Invalid Context:\n@@ def fib(n):"
}
```

The model can then adjust future diffs (for example, by re-reading a file in your prompt or simplifying a change) based on these error messages.

## Best practices

*   **Give clear file context**
    *   When you call the Responses API, include either an inline snapshot of your files (as in the example), or give the model tools for exploring your filesystem (like the `shell` tool).

*   **Consider using with the `shell` tool**
    *   When used in conjunction with the `shell` tool, the model can explore file system directories, read files, and grep for keywords, enabling agentic file discovery and editing.

*   **Encourage small, focused diffs**
    *   In your system instructions, nudge the model toward minimal, targeted edits rather than huge rewrites.

*   **Make sure changes apply cleanly**
    *   After a series of patches, run your tests or linters and share failures back in the next `input` so the model can fix them.

## Usage notes

| API Availability | Supported models |
| --- | --- |
| [Responses](https://developers.openai.com/api/docs/api-reference/responses) [Chat Completions](https://developers.openai.com/api/docs/api-reference/chat) [Assistants](https://developers.openai.com/api/docs/api-reference/assistants) | [GPT-5.5](https://developers.openai.com/api/docs/models/gpt-5.5) [GPT-5.4](https://developers.openai.com/api/docs/models/gpt-5.4) [GPT-5.2](https://developers.openai.com/api/docs/models/gpt-5.2) [GPT-5.1](https://developers.openai.com/api/docs/models/gpt-5.1) |

[Previous Driver reference](https://developers.openai.com/api/docs/guides/tools-computer-use-sdk-drivers)[Next Local shell](https://developers.openai.com/api/docs/guides/tools-local-shell)

Ask AI

## Docs agent

Loading docs agent...
