#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/.." && pwd)

PROJECT_DIR=$(pwd)
SERVER_NAME="codex-taskloop"
SKIP_MCP=0
SKIP_HOOK=0
SKIP_BUILD=0

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
    --no-build)
      SKIP_BUILD=1
      shift
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

HOOK_CMD="${REPO_ROOT}/target/release/codex-taskloop-hook"
MCP_BIN="${REPO_ROOT}/target/release/codex-taskloop"
ADMIN_BIN="${REPO_ROOT}/target/release/codex-taskloop-admin"

if [[ ${SKIP_BUILD} -eq 0 ]]; then
  if [[ ! -x "${MCP_BIN}" ]] || [[ ! -x "${HOOK_CMD}" ]] || [[ ! -x "${ADMIN_BIN}" ]]; then
    if command -v cargo >/dev/null 2>&1; then
      cargo build --release --manifest-path "${REPO_ROOT}/Cargo.toml"
    else
      echo "cargo not found; build the MCP server first or pass --no-build" >&2
      exit 1
    fi
  fi
fi

if [[ ${SKIP_HOOK} -eq 0 ]]; then
  if [[ ! -x "${ADMIN_BIN}" ]]; then
    echo "admin binary not found; build first or pass --no-hook" >&2
    exit 1
  fi
  "${ADMIN_BIN}" hooks add \
    --project "${PROJECT_DIR}" \
    --command "${HOOK_CMD}"
fi

if [[ ${SKIP_MCP} -eq 0 ]]; then
  if command -v codex >/dev/null 2>&1; then
    codex mcp remove "${SERVER_NAME}" >/dev/null 2>&1 || true
    MCP_ENV_ARGS=(--env "CODEX_CWD=${PROJECT_DIR}" --env "TASKLOOP_STORAGE_SCOPE=local-only")
    if [[ -n "${CODEX_HOME:-}" ]]; then
      MCP_ENV_ARGS+=(--env "CODEX_HOME=${CODEX_HOME}")
    fi
    codex mcp add "${SERVER_NAME}" "${MCP_ENV_ARGS[@]}" -- "${MCP_BIN}"
  else
    if [[ ! -x "${ADMIN_BIN}" ]]; then
      echo "admin binary not found; build first or pass --no-mcp" >&2
      exit 1
    fi
    "${ADMIN_BIN}" mcp add \
      --name "${SERVER_NAME}" \
      --command "${MCP_BIN}" \
      --project "${PROJECT_DIR}"
  fi
fi

echo "Installed codex-taskloop."\
" Project: ${PROJECT_DIR}"\
" | MCP: ${SERVER_NAME} | Hook: ${HOOK_CMD}"
