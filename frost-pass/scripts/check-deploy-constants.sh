#!/usr/bin/env bash
# Pre-deploy guard: fails if constants that must be replaced before deployment are still placeholders.
# Run from anywhere: ./frost-pass/scripts/check-deploy-constants.sh
set -euo pipefail

constants="$(dirname "$0")/../programs/frost-pass/src/constants.rs"

if grep -q "P1aceho1der" "$constants"; then
  echo "error: PROTOCOL_TREASURY in constants.rs is still a placeholder; set the real treasury before deploying" >&2
  exit 1
fi

echo "deploy constants look set"
