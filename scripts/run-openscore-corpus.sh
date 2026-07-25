#!/usr/bin/env bash
set -euo pipefail

OPEN_SCORE_REPOSITORY="https://github.com/OpenScore/Lieder.git"
OPEN_SCORE_COMMIT="6b2dc542ce2e8aa4b78c8ee62103b210efc07015"
OPEN_SCORE_LICENSE="CC0-1.0"
DEFAULT_SAMPLE_SIZE="3"
DEFAULT_MAX_FILES="2000"

SCRIPT_DIRECTORY="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PROJECT_ROOT="$(cd -- "${SCRIPT_DIRECTORY}/.." && pwd -P)"
DEFAULT_CACHE_DIRECTORY="${PROJECT_ROOT}/src-tauri/target/corpora/openscore-lieder-${OPEN_SCORE_COMMIT:0:12}"
DEFAULT_REPORT_PATH="${PROJECT_ROOT}/src-tauri/target/corpus-reports/openscore-lieder-${OPEN_SCORE_COMMIT:0:12}.json"

cache_directory="${VERSE_OPENSCORE_CACHE:-${DEFAULT_CACHE_DIRECTORY}}"
report_path="${VERSE_OPENSCORE_REPORT:-${DEFAULT_REPORT_PATH}}"
renderer_path="${VERSE_MUSESCORE_GATE:-}"
sample_size="0"
max_files="${VERSE_OPENSCORE_MAX_FILES:-${DEFAULT_MAX_FILES}}"
full_parse="false"

usage() {
  echo "Usage: scripts/run-openscore-corpus.sh --full-parse [--render-sample [N]] [options]"
  echo
  echo "Options:"
  echo "  --full-parse           Parse and project every canonical .mscx score."
  echo "  --render-sample [N]    Render N deterministic scores and all their Parts (default: 3)."
  echo "  --renderer PATH        MuseScore Studio 3.6.2/4 executable."
  echo "  --cache PATH           Reuse or create the pinned corpus checkout here."
  echo "  --report PATH          Write the machine-readable JSON report here."
  echo "  --max-files N          Safety ceiling (default: 2000)."
  echo "  --help                 Show this help."
}

while (($# > 0)); do
  case "$1" in
    --full-parse)
      full_parse="true"
      shift
      ;;
    --render-sample)
      sample_size="${DEFAULT_SAMPLE_SIZE}"
      if (($# > 1)) && [[ "$2" != --* ]]; then
        sample_size="$2"
        shift
      fi
      shift
      ;;
    --renderer)
      [[ $# -ge 2 ]] || { echo "--renderer requires a path" >&2; exit 2; }
      renderer_path="$2"
      shift 2
      ;;
    --cache)
      [[ $# -ge 2 ]] || { echo "--cache requires a path" >&2; exit 2; }
      cache_directory="$2"
      shift 2
      ;;
    --report)
      [[ $# -ge 2 ]] || { echo "--report requires a path" >&2; exit 2; }
      report_path="$2"
      shift 2
      ;;
    --max-files)
      [[ $# -ge 2 ]] || { echo "--max-files requires a value" >&2; exit 2; }
      max_files="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "${full_parse}" != "true" && "${sample_size}" == "0" ]]; then
  echo "Select --full-parse and/or --render-sample." >&2
  exit 2
fi
if ! [[ "${sample_size}" =~ ^[0-9]+$ && "${max_files}" =~ ^[1-9][0-9]*$ ]]; then
  echo "Sample size and file limit must be non-negative integers." >&2
  exit 2
fi

if [[ -e "${cache_directory}" ]]; then
  if [[ ! -d "${cache_directory}/.git" ]]; then
    echo "Refusing to reuse a non-Git cache path: ${cache_directory}" >&2
    exit 2
  fi
else
  mkdir -p -- "$(dirname -- "${cache_directory}")"
  git init --quiet "${cache_directory}"
  git -C "${cache_directory}" remote add origin "${OPEN_SCORE_REPOSITORY}"
  git -C "${cache_directory}" fetch --quiet --depth 1 origin "${OPEN_SCORE_COMMIT}"
  git -C "${cache_directory}" checkout --quiet --detach "${OPEN_SCORE_COMMIT}"
fi

cached_commit="$(git -C "${cache_directory}" rev-parse HEAD)"
cached_remote="$(git -C "${cache_directory}" remote get-url origin)"
if [[ "${cached_commit}" != "${OPEN_SCORE_COMMIT}" || "${cached_remote}" != "${OPEN_SCORE_REPOSITORY}" ]]; then
  echo "Refusing an unpinned or foreign corpus cache: ${cache_directory}" >&2
  exit 2
fi
cached_status="$(
  git -C "${cache_directory}" status \
    --porcelain=v1 \
    --untracked-files=all \
    --ignored=matching \
    --ignore-submodules=none
)"
if [[ -n "${cached_status}" ]]; then
  echo "Refusing a modified, untracked or ignored corpus cache: ${cache_directory}" >&2
  echo "${cached_status}" >&2
  exit 2
fi

if [[ ! -f "${cache_directory}/LICENSE.txt" ]] || ! grep -q "Creative Commons" "${cache_directory}/LICENSE.txt"; then
  echo "Pinned corpus checkout is missing its CC0 license evidence." >&2
  exit 2
fi

mkdir -p -- "$(dirname -- "${report_path}")"

audit_arguments=(
  --input "${cache_directory}/scores"
  --report "${report_path}"
  --corpus-name "OpenScore Lieder"
  --repository "${OPEN_SCORE_REPOSITORY%.git}"
  --commit "${OPEN_SCORE_COMMIT}"
  --license "${OPEN_SCORE_LICENSE}"
  --extension mscx
  --max-files "${max_files}"
)
if [[ "${full_parse}" == "true" ]]; then
  audit_arguments+=(--full-parse)
fi
if [[ "${sample_size}" != "0" ]]; then
  audit_arguments+=(--render-sample "${sample_size}")
fi
if [[ -n "${renderer_path}" ]]; then
  audit_arguments+=(--renderer "${renderer_path}")
fi

cargo run \
  --quiet \
  --locked \
  --manifest-path "${PROJECT_ROOT}/src-tauri/Cargo.toml" \
  --example corpus_audit \
  -- \
  "${audit_arguments[@]}"

echo "OpenScore Lieder audit passed."
echo "Pinned commit: ${OPEN_SCORE_COMMIT}"
echo "Report: ${report_path}"
