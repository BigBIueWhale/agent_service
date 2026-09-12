#!/usr/bin/env bash
# The four prompts are in this repository and are always checked. A corpus is
# not -- it is a commercial book series derived from the owner's own EPUBs -- so
# it is checked against its pinned identity when its directory is named.
#
# There is one mode: verify. Naming a directory adds the corpus that directory
# claims to be, identified by its own file set rather than by a flag, and a
# directory that is neither corpus is refused rather than skipped.
set -Eeuo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly HARNESS_DIR

if (( $# > 1 )); then
  printf 'ERROR: usage: ./verify-harness-inputs.sh [corpus-directory]\n' >&2
  exit 64
fi

# Prompt identity, and the invariant that makes Test A, B and C one experiment:
# Test C is the common text, and each of the other two is Test C plus exactly one
# delegation clause. The digest of the diff pins that clause in both directions --
# its text, its position, and the absence of anything else.
readonly PROBE_SHA256='66e4b9a9fe661c1ddca759a98e39537b3b62adec96db638fc50848c3cf29a8e0'
readonly TEST_A_SHA256='fa92a0c2bc559898f849f95626596c5b96117ff06d5328708290ca5cc35c0cc7'
readonly TEST_B_SHA256='20cdbbdd5a9c20106638b0101c59309a0a162a289ce6e32bc721dac23bbab2dc'
readonly TEST_C_SHA256='727c358ba9846ccd56f07037decc07151cbda577b9096b7da76f163116a5250e'
readonly C_TO_A_DIFF_SHA256='7debd15ea663bd2765e98d9c03d4a678796ef75dafba0292c8034d587fe5e82a'
readonly C_TO_B_DIFF_SHA256='8382dc7187f185955274271957dbfce81b906c7b496ec3bb42b4ee05af12513b'

# Corpus identity. Counts and byte totals are stated beside the digests because a
# file set that matches by name and digest but not by count is a different corpus.
readonly SERIES_FILES=11
readonly SERIES_BYTES=9400777
readonly SERIES_WORDS=1554654
readonly SERIES_CONCAT_SHA256='4a87caf8b460bbee91239471ce7c2507c7ad72870c09e1d35154a9291495ff95'
readonly PROBE_FILES=26
readonly PROBE_BYTES=1293067
readonly PROBE_CONCAT_SHA256='68a5006393aee012b3c2d7fa98409b4491ac4dd3147b3724950f7544df0f1946'

require_equal() {
  local what="$1" observed="$2" expected="$3"
  if [[ "${observed}" != "${expected}" ]]; then
    printf 'ERROR: %s\n  expected: %s\n  observed: %s\n' "${what}" "${expected}" "${observed}" >&2
    exit 65
  fi
  printf '  %s: OK\n' "${what}"
}

sha256_file() {
  sha256sum -- "$1" | cut -d' ' -f1
}

printf 'Prompts:\n'
require_equal 'prompts/probe.txt'  "$(sha256_file "${HARNESS_DIR}/prompts/probe.txt")"  "${PROBE_SHA256}"
require_equal 'prompts/test-a.txt' "$(sha256_file "${HARNESS_DIR}/prompts/test-a.txt")" "${TEST_A_SHA256}"
require_equal 'prompts/test-b.txt' "$(sha256_file "${HARNESS_DIR}/prompts/test-b.txt")" "${TEST_B_SHA256}"
require_equal 'prompts/test-c.txt' "$(sha256_file "${HARNESS_DIR}/prompts/test-c.txt")" "${TEST_C_SHA256}"
require_equal 'delegation clause, Test C -> Test A' \
  "$(diff -- "${HARNESS_DIR}/prompts/test-c.txt" "${HARNESS_DIR}/prompts/test-a.txt" | sha256sum | cut -d' ' -f1)" \
  "${C_TO_A_DIFF_SHA256}"
require_equal 'delegation clause, Test C -> Test B' \
  "$(diff -- "${HARNESS_DIR}/prompts/test-c.txt" "${HARNESS_DIR}/prompts/test-b.txt" | sha256sum | cut -d' ' -f1)" \
  "${C_TO_B_DIFF_SHA256}"

if (( $# == 0 )); then
  printf 'No corpus directory named; prompt identity verified.\n'
  exit 0
fi

corpus_dir="$1"
[[ -d "${corpus_dir}" ]] || {
  printf 'ERROR: not a directory: %s\n' "${corpus_dir}" >&2
  exit 66
}
corpus_dir="$(cd -- "${corpus_dir}" && pwd)"

mapfile -t books < <(cd -- "${corpus_dir}" && printf '%s\n' book_*.txt 2>/dev/null | grep -v '^book_\*\.txt$' | sort)
mapfile -t chunks < <(cd -- "${corpus_dir}" && printf '%s\n' chunk_*.txt 2>/dev/null | grep -v '^chunk_\*\.txt$' | sort)

if (( ${#books[@]} > 0 && ${#chunks[@]} > 0 )); then
  printf 'ERROR: %s holds both corpora; each is verified in its own directory.\n' "${corpus_dir}" >&2
  exit 67
fi

if (( ${#books[@]} > 0 )); then
  printf 'Series corpus at %s:\n' "${corpus_dir}"
  require_equal 'file count' "${#books[@]}" "${SERIES_FILES}"
  require_equal 'byte total' "$(cd -- "${corpus_dir}" && cat -- "${books[@]}" | wc -c)" "${SERIES_BYTES}"
  require_equal 'word total' "$(cd -- "${corpus_dir}" && cat -- "${books[@]}" | wc -w)" "${SERIES_WORDS}"
  require_equal 'concatenation SHA-256' \
    "$(cd -- "${corpus_dir}" && cat -- "${books[@]}" | sha256sum | cut -d' ' -f1)" \
    "${SERIES_CONCAT_SHA256}"
  # Per-file digests live beside this script so a single moved volume is named
  # rather than reported as a changed concatenation.
  ( cd -- "${corpus_dir}" && sha256sum --check --quiet -- "${HARNESS_DIR}/corpus-digests.sha256" ) || {
    printf 'ERROR: a volume does not match its pinned digest (see the line above).\n' >&2
    exit 68
  }
  printf '  per-volume digests: OK\n'
elif (( ${#chunks[@]} > 0 )); then
  printf 'Probe corpus at %s:\n' "${corpus_dir}"
  require_equal 'file count' "${#chunks[@]}" "${PROBE_FILES}"
  require_equal 'byte total' "$(cd -- "${corpus_dir}" && cat -- "${chunks[@]}" | wc -c)" "${PROBE_BYTES}"
  require_equal 'concatenation SHA-256' \
    "$(cd -- "${corpus_dir}" && cat -- "${chunks[@]}" | sha256sum | cut -d' ' -f1)" \
    "${PROBE_CONCAT_SHA256}"
else
  printf 'ERROR: %s holds neither corpus: no book_*.txt and no chunk_*.txt.\n' "${corpus_dir}" >&2
  exit 69
fi

printf 'Verified.\n'
