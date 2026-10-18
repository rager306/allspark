#!/usr/bin/env bash
# Copy the fixture into the sandbox work dir.
set -euo pipefail
cp -r "$(dirname "$0")/fixture/." "$QA_WORK/"
