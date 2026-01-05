#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/.." && pwd)

PROJECT_DIR=$(pwd)
SERVER_NAME="codex-taskloop"
SKIP_MCP=0
SKIP_HOOK=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project)
      PROJECT_DIR="$2"
      shift 2
      ;;
    --name)
      SERVER_NAME="$2"
      shift 2
      ;;
    --no-mcp)
      SKIP_MCP=1
      shift
      ;;
    --no-hook)
      SKIP_HOOK=1
      shift
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

HOOK_CMD="${REPO_ROOT}/target/release/codex-taskloop-hook"
ADMIN_BIN="${REPO_ROOT}/target/release/codex-taskloop-admin"

if [[ ${SKIP_HOOK} -eq 0 ]]; then
  if [[ ! -x "${ADMIN_BIN}" ]]; then
    echo "admin binary not found; build first or pass --no-hook" >&2
    exit 1
  fi
  "${ADMIN_BIN}" hooks remove \
    --project "${PROJECT_DIR}" \
    --command "${HOOK_CMD}"
fi

for file in "${PROJECT_DIR}"/.codex/task-loop*.local.md; do
  if [[ -f "${file}" ]]; then
    rm -f "${file}"
  fi
done

for file in "${PROJECT_DIR}"/.codex/task-loop*.history.jsonl; do
  if [[ -f "${file}" ]]; then
    rm -f "${file}"
  fi
done

if [[ -d "${PROJECT_DIR}/.codex/task_loop" ]]; then
  rm -rf "${PROJECT_DIR}/.codex/task_loop"
fi

if [[ ${SKIP_MCP} -eq 0 ]]; then
  if command -v codex >/dev/null 2>&1; then
    codex mcp remove "${SERVER_NAME}" >/dev/null 2>&1 || true
  else
    if [[ ! -x "${ADMIN_BIN}" ]]; then
      echo "admin binary not found; build first or pass --no-mcp" >&2
      exit 1
    fi
    "${ADMIN_BIN}" mcp remove \
      --name "${SERVER_NAME}"
  fi
fi

echo "Uninstalled codex-taskloop."\
" Project: ${PROJECT_DIR}"
