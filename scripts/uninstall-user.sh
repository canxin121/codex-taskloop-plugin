#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/.." && pwd)

SERVER_NAME="codex-taskloop"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --name)
      SERVER_NAME="$2"
      shift 2
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

ADMIN_BIN="${REPO_ROOT}/target/release/codex-taskloop-admin"

if command -v codex >/dev/null 2>&1; then
  codex mcp remove "${SERVER_NAME}" >/dev/null 2>&1 || true
else
  if [[ ! -x "${ADMIN_BIN}" ]]; then
    echo "admin binary not found; build first" >&2
    exit 1
  fi
  "${ADMIN_BIN}" mcp remove \
    --name "${SERVER_NAME}"
fi

if [[ ! -x "${ADMIN_BIN}" ]]; then
  echo "admin binary not found; build first" >&2
  exit 1
fi
${ADMIN_BIN} stop-hooks remove \
  --name "${SERVER_NAME}"

echo "Uninstalled codex-taskloop (user-level MCP + Stop hook)."\
" MCP: ${SERVER_NAME}"
