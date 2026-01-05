#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/.." && pwd)

PROJECT_DIR=$(pwd)
SERVER_NAME="codex-taskloop"
INSTALL_SCOPE="project"
SKIP_MCP=0
SKIP_HOOK=0
BIN_DIR_OVERRIDE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --scope)
      INSTALL_SCOPE="$2"
      shift 2
      ;;
    --project)
      PROJECT_DIR="$2"
      shift 2
      ;;
    --name)
      SERVER_NAME="$2"
      shift 2
      ;;
    --bin-dir)
      BIN_DIR_OVERRIDE="$2"
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

CODEX_HOME_DIR="${CODEX_HOME:-${HOME}/.codex}"
PROJECT_SKILL_DIR="${PROJECT_DIR}/.codex/skills/codex-taskloop"
USER_SKILL_DIR="${CODEX_HOME_DIR}/skills/codex-taskloop"
PROJECT_BIN_DIR="${PROJECT_DIR}/.codex/bin"
USER_BIN_DIR="${CODEX_HOME_DIR}/bin"

if [[ "${INSTALL_SCOPE}" != "project" && "${INSTALL_SCOPE}" != "user" ]]; then
  echo "Invalid --scope: ${INSTALL_SCOPE} (expected: project | user)" >&2
  exit 1
fi
if [[ -n "${BIN_DIR_OVERRIDE}" && ! -d "${BIN_DIR_OVERRIDE}" ]]; then
  echo "Invalid --bin-dir: ${BIN_DIR_OVERRIDE} (expected a directory)" >&2
  exit 1
fi

resolve_bin_in_dir() {
  local dir="$1"
  local name="$2"
  if [[ -f "${dir}/${name}" ]]; then
    echo "${dir}/${name}"
    return 0
  fi
  if [[ -f "${dir}/${name}.exe" ]]; then
    echo "${dir}/${name}.exe"
    return 0
  fi
  return 1
}

if [[ -n "${BIN_DIR_OVERRIDE}" ]]; then
  BIN_DEST_DIR="${BIN_DIR_OVERRIDE}"
else
  if [[ "${INSTALL_SCOPE}" == "project" ]]; then
    BIN_DEST_DIR="${PROJECT_BIN_DIR}"
  else
    BIN_DEST_DIR="${USER_BIN_DIR}"
  fi
fi

ADMIN_BIN="$(resolve_bin_in_dir "${BIN_DEST_DIR}" "codex-taskloop-admin" || true)"
HOOK_CMD="$(resolve_bin_in_dir "${BIN_DEST_DIR}" "codex-taskloop-hook" || true)"

if [[ ${SKIP_HOOK} -eq 0 ]]; then
  if [[ -z "${ADMIN_BIN}" ]]; then
    echo "admin binary not found in ${BIN_DEST_DIR}; provide --bin-dir or pass --no-hook" >&2
    exit 1
  fi
  if [[ "${INSTALL_SCOPE}" == "project" ]]; then
    if [[ -z "${HOOK_CMD}" ]]; then
      echo "hook binary not found in ${BIN_DEST_DIR}; provide --bin-dir or pass --no-hook" >&2
      exit 1
    fi
    "${ADMIN_BIN}" hooks remove \
      --project "${PROJECT_DIR}" \
      --command "${HOOK_CMD}"
  else
    "${ADMIN_BIN}" stop-hooks remove \
      --name "${SERVER_NAME}"
  fi
fi

if [[ "${INSTALL_SCOPE}" == "project" ]]; then
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

  if [[ -d "${PROJECT_SKILL_DIR}" ]]; then
    rm -rf "${PROJECT_SKILL_DIR}"
  fi
else
  if [[ -d "${USER_SKILL_DIR}" ]]; then
    rm -rf "${USER_SKILL_DIR}"
  fi
fi

if [[ ${SKIP_MCP} -eq 0 ]]; then
  if command -v codex >/dev/null 2>&1; then
    codex mcp remove "${SERVER_NAME}" >/dev/null 2>&1 || true
  else
    if [[ -z "${ADMIN_BIN}" ]]; then
      echo "admin binary not found in ${BIN_DEST_DIR}; provide --bin-dir or pass --no-mcp" >&2
      exit 1
    fi
    "${ADMIN_BIN}" mcp remove \
      --name "${SERVER_NAME}"
  fi
fi

remove_bin() {
  local name="$1"
  local path
  for ext in "" ".exe"; do
    path="${BIN_DEST_DIR}/${name}${ext}"
    if [[ -f "${path}" ]]; then
      rm -f "${path}"
    fi
  done
}

if [[ -d "${BIN_DEST_DIR}" ]]; then
  remove_bin "codex-taskloop"
  remove_bin "codex-taskloop-hook"
  remove_bin "codex-taskloop-admin"
fi

if [[ "${INSTALL_SCOPE}" == "project" ]]; then
  echo "Uninstalled codex-taskloop (project-level MCP + Stop hook)."\
  " Project: ${PROJECT_DIR} | Bin: ${BIN_DEST_DIR} | Skill: ${PROJECT_SKILL_DIR}"
else
  echo "Uninstalled codex-taskloop (user-level MCP + Stop hook)."\
  " MCP: ${SERVER_NAME} | Bin: ${BIN_DEST_DIR} | Skill: ${USER_SKILL_DIR}"
fi
