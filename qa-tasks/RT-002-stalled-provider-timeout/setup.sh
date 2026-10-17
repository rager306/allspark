#!/usr/bin/env bash
# A loopback openai-completions provider that reads the request and then
# never answers: the client's per-attempt request timeout (retry.timeoutMs,
# set small) must settle the wait; the run must end inside the budget.
set -euo pipefail
AGENT_DIR="$HOME/.prime/agent"
mkdir -p "$AGENT_DIR"
cat > "$AGENT_DIR/models.json" <<EOF
{
  "providers": {
    "qa-hang": {
      "api": "openai-completions",
      "baseUrl": "$QA_MOCK_HANG_URL",
      "apiKey": "sk-qa",
      "models": [
        {"id": "mock-1", "name": "Mock 1", "api": "openai-completions",
         "contextWindow": 128000, "maxTokens": 1024}
      ]
    }
  }
}
EOF
cat > "$AGENT_DIR/settings.json" <<'EOF'
{
  "retry": {"enabled": true, "maxRetries": 1, "baseDelayMs": 50,
            "provider": {"timeoutMs": 8000}}
}
EOF
