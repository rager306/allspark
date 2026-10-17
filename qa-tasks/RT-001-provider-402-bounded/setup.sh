#!/usr/bin/env bash
# Registers a loopback anthropic-messages provider that always answers 402
# insufficient_credits, with fast bounded retries (no failover configured:
# the invariant under test is the bounded retry + surfaced error).
set -euo pipefail
AGENT_DIR="$HOME/.prime/agent"
mkdir -p "$AGENT_DIR"
cat > "$AGENT_DIR/models.json" <<EOF
{
  "providers": {
    "qa-402": {
      "api": "anthropic-messages",
      "baseUrl": "$QA_MOCK_FAIL402_URL",
      "apiKey": "sk-qa",
      "models": [
        {"id": "mock-1", "name": "Mock 1", "api": "anthropic-messages",
         "contextWindow": 128000, "maxTokens": 1024}
      ]
    }
  }
}
EOF
cat > "$AGENT_DIR/settings.json" <<'EOF'
{
  "retry": {"enabled": true, "maxRetries": 2, "baseDelayMs": 50, "maxDelayMs": 100,
            "failover": {"enabled": false}}
}
EOF
