#!/usr/bin/env bash
# Every code identifier the transformation README names is one the
# transformation ships, and every count of its semantic concerns a document
# states is the count the transformer validates.
#
# The README describes the result of applying the review diff to pinned
# upstream source, so an identifier it quotes must appear in that result. An
# identifier the diff only removes, or never mentions at all, is not part of
# what ships; naming it describes a tree nobody has.
#
# Scope is deliberately narrow and mechanical: backtick-quoted tokens that are
# unambiguously code symbols (SCREAMING_SNAKE_CASE or lowerCamelCase). Prose,
# paths, wire fields and file names are not identifiers and are not checked.
#
# The concern count is the transformer's own number, read from its contracts
# module rather than restated, so a document that states another describes a
# transformer nobody has.
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly README="${PROJECT_DIR}/patches/README.md"
readonly REVIEW_DIFF="${PROJECT_DIR}/patches/qwen-code-0.21.12-agent-service.patch"
readonly PROJECT_README="${PROJECT_DIR}/README.md"

for required in "${README}" "${REVIEW_DIFF}" "${PROJECT_README}"; do
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

concerns="$(
  cd -- "${PROJECT_DIR}" &&
    PYTHONDONTWRITEBYTECODE=1 python3 -c \
      'from patches.source_patch_v1.contracts_qwen_code import CONCERNS; print(len(CONCERNS))'
)"

# A stated count may wrap across lines, so whitespace is folded before it is
# read, and a document that states none no longer matches the extractor.
for document in "${README}" "${PROJECT_README}"; do
  mapfile -t stated < <(
    tr -s '[:space:]' ' ' <"${document}" |
      grep -oE '\b[0-9]+ semantic concerns\b' |
      cut -d' ' -f1 |
      sort -u
  )
  (( ${#stated[@]} > 0 )) || {
    printf 'ERROR: %s states no semantic-concern count; the extractor no longer matches it.\n' \
      "${document#"${PROJECT_DIR}/"}" >&2
    exit 1
  }
  for count in "${stated[@]}"; do
    [[ "${count}" == "${concerns}" ]] || {
      printf 'ERROR: %s states %s semantic concerns; the transformer validates %s.\n' \
        "${document#"${PROJECT_DIR}/"}" "${count}" "${concerns}" >&2
      exit 1
    }
  done
done

printf 'DOC_IDENTIFIER_CONTRACT_OK identifiers=%s concerns=%s\n' "${#identifiers[@]}" "${concerns}"
