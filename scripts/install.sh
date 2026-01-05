#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/.." && pwd)

PROJECT_DIR=$(pwd)
SERVER_NAME="codex-taskloop"
INSTALL_SCOPE="project"
SKIP_MCP=0
SKIP_HOOK=0
SKIP_BUILD=0
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

if [[ "${INSTALL_SCOPE}" != "project" && "${INSTALL_SCOPE}" != "user" ]]; then
  echo "Invalid --scope: ${INSTALL_SCOPE} (expected: project | user)" >&2
  exit 1
fi
if [[ -n "${BIN_DIR_OVERRIDE}" && ! -d "${BIN_DIR_OVERRIDE}" ]]; then
  echo "Invalid --bin-dir: ${BIN_DIR_OVERRIDE} (expected a directory)" >&2
  exit 1
fi

CODEX_HOME_DIR="${CODEX_HOME:-${HOME}/.codex}"
PROJECT_SKILL_DIR="${PROJECT_DIR}/.codex/skills/codex-taskloop"
USER_SKILL_DIR="${CODEX_HOME_DIR}/skills/codex-taskloop"
PROJECT_BIN_DIR="${PROJECT_DIR}/.codex/bin"
USER_BIN_DIR="${CODEX_HOME_DIR}/bin"

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

find_bin_sources() {
  local dir
  for dir in "$@"; do
    [[ -d "${dir}" ]] || continue
    local mcp hook admin
    mcp="$(resolve_bin_in_dir "${dir}" "codex-taskloop" || true)"
    hook="$(resolve_bin_in_dir "${dir}" "codex-taskloop-hook" || true)"
    admin="$(resolve_bin_in_dir "${dir}" "codex-taskloop-admin" || true)"
    if [[ -n "${mcp}" && -n "${hook}" && -n "${admin}" ]]; then
      MCP_SRC="${mcp}"
      HOOK_SRC="${hook}"
      ADMIN_SRC="${admin}"
      return 0
    fi
  done
  return 1
}

BIN_CANDIDATES=()
if [[ -n "${BIN_DIR_OVERRIDE}" ]]; then
  BIN_CANDIDATES+=("${BIN_DIR_OVERRIDE}")
else
  BIN_CANDIDATES+=("${REPO_ROOT}/target/release" "${REPO_ROOT}/bin")
fi

if ! find_bin_sources "${BIN_CANDIDATES[@]}"; then
  if [[ -n "${BIN_DIR_OVERRIDE}" ]]; then
    echo "Binaries not found in ${BIN_DIR_OVERRIDE}. Provide a valid --bin-dir." >&2
    exit 1
  fi
  if [[ ${SKIP_BUILD} -eq 0 && -f "${REPO_ROOT}/Cargo.toml" ]]; then
    if command -v cargo >/dev/null 2>&1; then
      cargo build --release --manifest-path "${REPO_ROOT}/Cargo.toml"
      if ! find_bin_sources "${BIN_CANDIDATES[@]}"; then
        echo "Binaries not found after build; use --bin-dir to specify their location." >&2
        exit 1
      fi
    else
      echo "cargo not found; provide --bin-dir or pass --no-build" >&2
      exit 1
    fi
  else
    echo "Binaries not found; provide --bin-dir to specify their location." >&2
    exit 1
  fi
fi

if [[ "${INSTALL_SCOPE}" == "project" ]]; then
  BIN_DEST_DIR="${PROJECT_BIN_DIR}"
else
  BIN_DEST_DIR="${USER_BIN_DIR}"
fi
mkdir -p "${BIN_DEST_DIR}"
cp -f "${MCP_SRC}" "${BIN_DEST_DIR}/"
cp -f "${HOOK_SRC}" "${BIN_DEST_DIR}/"
cp -f "${ADMIN_SRC}" "${BIN_DEST_DIR}/"

MCP_BIN="${BIN_DEST_DIR}/$(basename "${MCP_SRC}")"
HOOK_CMD="${BIN_DEST_DIR}/$(basename "${HOOK_SRC}")"
ADMIN_BIN="${BIN_DEST_DIR}/$(basename "${ADMIN_SRC}")"
chmod +x "${MCP_BIN}" "${HOOK_CMD}" "${ADMIN_BIN}" 2>/dev/null || true

if [[ ${SKIP_HOOK} -eq 0 ]]; then
  if [[ ! -f "${ADMIN_BIN}" ]]; then
    echo "admin binary not found at ${ADMIN_BIN}; provide --bin-dir" >&2
    exit 1
  fi
  if [[ "${INSTALL_SCOPE}" == "project" ]]; then
    "${ADMIN_BIN}" hooks add \
      --project "${PROJECT_DIR}" \
      --command "${HOOK_CMD}"
  else
    "${ADMIN_BIN}" stop-hooks add \
      --name "${SERVER_NAME}" \
      --command "${HOOK_CMD}"
  fi
fi

if [[ ${SKIP_MCP} -eq 0 ]]; then
  if command -v codex >/dev/null 2>&1; then
    codex mcp remove "${SERVER_NAME}" >/dev/null 2>&1 || true
    MCP_ENV_ARGS=()
    if [[ "${INSTALL_SCOPE}" == "project" ]]; then
      MCP_ENV_ARGS+=(--env "CODEX_CWD=${PROJECT_DIR}" --env "TASKLOOP_STORAGE_SCOPE=project-only")
    fi
    if [[ -n "${CODEX_HOME:-}" ]]; then
      MCP_ENV_ARGS+=(--env "CODEX_HOME=${CODEX_HOME}")
    fi
    codex mcp add "${SERVER_NAME}" "${MCP_ENV_ARGS[@]}" -- "${MCP_BIN}"
  else
    if [[ ! -f "${ADMIN_BIN}" ]]; then
      echo "admin binary not found at ${ADMIN_BIN}; provide --bin-dir" >&2
      exit 1
    fi
    if [[ "${INSTALL_SCOPE}" == "project" ]]; then
      "${ADMIN_BIN}" mcp add \
        --name "${SERVER_NAME}" \
        --command "${MCP_BIN}" \
        --project "${PROJECT_DIR}"
    else
      "${ADMIN_BIN}" mcp add \
        --name "${SERVER_NAME}" \
        --command "${MCP_BIN}"
    fi
  fi
fi

SKILL_SOURCE_DIR=""
for candidate in "${REPO_ROOT}/.codex/skills/codex-taskloop" "${REPO_ROOT}/skills/codex-taskloop"; do
  if [[ -d "${candidate}" ]]; then
    SKILL_SOURCE_DIR="${candidate}"
    break
  fi
done
if [[ -z "${SKILL_SOURCE_DIR}" ]]; then
  echo "Skill source not found; expected .codex/skills/codex-taskloop or skills/codex-taskloop" >&2
  exit 1
fi

if [[ "${INSTALL_SCOPE}" == "project" ]]; then
  mkdir -p "${PROJECT_DIR}/.codex/skills"
  rm -rf "${PROJECT_SKILL_DIR}"
  cp -R "${SKILL_SOURCE_DIR}" "${PROJECT_SKILL_DIR}"
else
  mkdir -p "${CODEX_HOME_DIR}/skills"
  rm -rf "${USER_SKILL_DIR}"
  cp -R "${SKILL_SOURCE_DIR}" "${USER_SKILL_DIR}"
fi

if [[ "${INSTALL_SCOPE}" == "project" ]]; then
  echo "Installed codex-taskloop (project-level MCP + Stop hook)."\
  " Project: ${PROJECT_DIR}"\
  " | MCP: ${SERVER_NAME} | Hook: ${HOOK_CMD} | Bin: ${BIN_DEST_DIR} | Skill: ${PROJECT_SKILL_DIR}"
else
  echo "Installed codex-taskloop (user-level MCP + Stop hook)."\
  " MCP: ${SERVER_NAME} | Hook: ${HOOK_CMD} | Bin: ${BIN_DEST_DIR} | Skill: ${USER_SKILL_DIR}"
fi
