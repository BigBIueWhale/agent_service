#!/usr/bin/env bash
# Every code identifier the transformation README names is one the
# transformation ships.
#
# The README describes the result of applying the review diff to pinned
# upstream source, so an identifier it quotes must appear in that result. An
# identifier the diff only removes, or never mentions at all, is not part of
# what ships; naming it describes a tree nobody has.
#
# Scope is deliberately narrow and mechanical: backtick-quoted tokens that are
# unambiguously code symbols (SCREAMING_SNAKE_CASE or lowerCamelCase). Prose,
# paths, wire fields and file names are not identifiers and are not checked.
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly README="${PROJECT_DIR}/patches/README.md"
readonly REVIEW_DIFF="${PROJECT_DIR}/patches/qwen-code-0.21.12-agent-service.patch"

for required in "${README}" "${REVIEW_DIFF}"; do
  [[ -f "${required}" && ! -L "${required}" ]] || {
    printf 'ERROR: missing required input: %s\n' "${required}" >&2
    exit 1
  }
done

mapfile -t identifiers < <(
  grep -oE '`[A-Za-z][A-Za-z0-9_]{4,}`' "${README}" |
    tr -d '`' |
    grep -E '^[A-Z][A-Z0-9_]+$|^[a-z]+[A-Z][A-Za-z0-9]*$' |
    sort -u
)

(( ${#identifiers[@]} > 0 )) || {
  printf 'ERROR: no documented identifiers found; the extractor no longer matches the README.\n' >&2
  exit 1
}

# Evidence is hunk content: the added and context lines that together are the
# result of applying the diff. A `+++ ` file header carries the same leading
# `+` but names a path, so an identifier occurring only inside a path name is
# not evidence that any code mentions it.
if ! hunk_content="$(
  grep -E '^[+ ]' "${REVIEW_DIFF}" | grep -vE '^\+\+\+ '
)"; then
  printf 'ERROR: the review diff carries no hunk content to check against.\n' >&2
  exit 1
fi

undocumented=()
for identifier in "${identifiers[@]}"; do
  if ! grep -qE "\\b${identifier}\\b" <<<"${hunk_content}"; then
    undocumented+=("${identifier}")
  fi
done

if (( ${#undocumented[@]} > 0 )); then
  printf 'ERROR: the transformation README names identifiers the review diff does not ship:\n' >&2
  printf '  %s\n' "${undocumented[@]}" >&2
  exit 1
fi

printf 'DOC_IDENTIFIER_CONTRACT_OK identifiers=%s\n' "${#identifiers[@]}"
