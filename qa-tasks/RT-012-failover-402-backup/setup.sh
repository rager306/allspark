#!/usr/bin/env bash
# Two loopback providers both serving mock-1: the primary always 402s,
# the backup answers. Fast retry+failover settings make the chain walk
# quick (mirrors crates/pa-daemon/tests/provider_failover_e2e.rs).
set -euo pipefail
AGENT_DIR="$HOME/.prime/agent"
mkdir -p "$AGENT_DIR"
cat > "$AGENT_DIR/models.json" <<EOF
{
  "providers": {
    "qa-primary": {
      "api": "openai-completions",
      "baseUrl": "$QA_MOCK_PRIMARY_URL",
      "apiKey": "sk-primary",
      "models": [
        {"id": "mock-1", "name": "Mock 1", "api": "openai-completions",
         "contextWindow": 128000, "maxTokens": 1024}
      ]
    },
    "qa-backup": {
      "api": "openai-completions",
      "baseUrl": "$QA_MOCK_BACKUP_URL",
      "apiKey": "sk-backup",
      "models": [
        {"id": "mock-1", "name": "Mock 1 (backup)", "api": "openai-completions",
         "contextWindow": 128000, "maxTokens": 1024}
      ]
    }
  }
}
EOF
cat > "$AGENT_DIR/settings.json" <<'EOF'
{
  "retry": {"enabled": true, "maxRetries": 1, "baseDelayMs": 50,
            "failover": {"enabled": true, "maxRetries": 1, "baseDelayMs": 50}}
}
EOF
