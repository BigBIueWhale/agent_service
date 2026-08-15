#!/usr/bin/env bash
set -Eeuo pipefail
if [[ "$#" != 1 || ! "$1" =~ ^s-[0-9a-f]{32}$ ]]; then
  printf 'Usage: ./cancel.sh s-<32-lowercase-hex>\n' >&2
  exit 2
fi
curl --fail-with-body --silent --show-error \
  --request POST "http://127.0.0.1:8090/v1/agent/sessions/$1/cancel" | jq .
