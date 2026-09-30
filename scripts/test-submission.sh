#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

SOURCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_DIR="$(mktemp -d /tmp/agent-service-submission-test.XXXXXX)"
case "${TEST_DIR}" in
  /tmp/agent-service-submission-test.*) ;;
  *) printf 'ERROR: unexpected submission-test directory: %s\n' "${TEST_DIR}" >&2; exit 1 ;;
esac
cleanup() {
  rm -rf -- "${TEST_DIR}"
}
trap cleanup EXIT

SCRIPT_DIR="${TEST_DIR}"
readonly SCRIPT_DIR
mkdir --mode=0700 -- "${SCRIPT_DIR}/.runtime"
# submission-common.sh reads every numeric cap from the stack lock at source
# time (relative to SCRIPT_DIR), so the test root must carry that exact lock or
# the source below dies before a single assertion runs.
mkdir --mode=0700 -- "${SCRIPT_DIR}/config"
cp -- "${SOURCE_DIR}/config/stack.lock.json" "${SCRIPT_DIR}/config/stack.lock.json"
# shellcheck source=scripts/submission-common.sh
source "${SOURCE_DIR}/scripts/submission-common.sh"

readonly RUNNING_ID='s-1111111111111111111111111111111111111111111111111111111111111111'
readonly TERMINAL_ID='s-2222222222222222222222222222222222222222222222222222222222222222'
readonly INVALID_ID='s-3333333333333333333333333333333333333333333333333333333333333333'
readonly EMPTY_ID='s-4444444444444444444444444444444444444444444444444444444444444444'
readonly TAMPER_ID='s-5555555555555555555555555555555555555555555555555555555555555555'
readonly BUDGET_ID='s-6666666666666666666666666666666666666666666666666666666666666666'
readonly REFUSED_ID='s-7777777777777777777777777777777777777777777777777777777777777777'
readonly DECLARED_ID='s-8888888888888888888888888888888888888888888888888888888888888888'
PROMPT_FILE="${TEST_DIR}/prompt.txt"
printf 'exact receipt prompt\n' >"${PROMPT_FILE}"

# A real fixture workspace: content, an executable, and a symbolic link the
# receipt archive must record as a link rather than following.
FIXTURE_DIR="${TEST_DIR}/workspace"
mkdir -p "${FIXTURE_DIR}/nested"
printf '#!/bin/sh\nprintf proof\n' >"${FIXTURE_DIR}/run.sh"
chmod 0755 "${FIXTURE_DIR}/run.sh"
printf 'receipt-archive-proof' >"${FIXTURE_DIR}/nested/data.bin"
ln -s ../outside-tree "${FIXTURE_DIR}/escape-link"
EMPTY_DIR="${TEST_DIR}/empty-workspace"
mkdir "${EMPTY_DIR}"

assert_receipt() {
  local session_id="$1" expected="$2"
  local observed
  observed="$(submission_validate_receipt "${session_id}")"
  [[ "${observed}" == "${expected}" ]] || {
    printf 'receipt mismatch: observed=%s expected=%s\n' "${observed}" "${expected}" >&2
    return 1
  }
}

RUNNING_REQUEST="$(submission_create_receipt "${RUNNING_ID}" "${FIXTURE_DIR}" "${PROMPT_FILE}" '[]')"
readonly RUNNING_REQUEST
assert_receipt "${RUNNING_ID}" "${RUNNING_REQUEST}"
[[ "$(stat -c '%u:%g:%a' "${RUNNING_REQUEST}")" == '1000:1000:600' ]]

# The receipt archive is a real zip of the exact workspace: unzip must list
# the symbolic link as a link entry and the commitment must match the bytes.
RUNNING_ARCHIVE="${SUBMISSION_RECEIPT_ROOT}/${RUNNING_ID}/archive.zip"
unzip -l "${RUNNING_ARCHIVE}" | grep -q 'escape-link' || {
  printf 'receipt archive is missing the symbolic-link entry\n' >&2
  exit 1
}
[[ "$(jq -r '.archive_bytes' "${RUNNING_REQUEST}")" == "$(stat -c '%s' "${RUNNING_ARCHIVE}")" ]]
[[ "$(jq -r '.archive_sha256' "${RUNNING_REQUEST}")" == "$(sha256sum "${RUNNING_ARCHIVE}" | awk '{print $1}')" ]]

touch "${SUBMISSION_RECEIPT_ROOT}/${RUNNING_ID}/unexpected"
if submission_validate_receipt "${RUNNING_ID}" >/dev/null 2>&1; then
  printf 'receipt with an unexpected entry was accepted\n' >&2
  exit 1
fi
rm -- "${SUBMISSION_RECEIPT_ROOT}/${RUNNING_ID}/unexpected"
assert_receipt "${RUNNING_ID}" "${RUNNING_REQUEST}"

TEST_RESPONSE_STATUS='running'
TEST_HTTP_STATUS=202
TEST_EXPECTED_ID="${RUNNING_ID}"
# Unset: the service echoes the deliverables it accepted, which are the ones the
# receipt declared. Set: a response naming another list.
TEST_RESPONSE_DELIVERABLES=''
readonly TEST_MODEL='qwen3.8-27b-nvfp4-k8v4'
curl() {
  local output='' headers='' id_header='' request_form='' archive_form=''
  while (($#)); do
    case "$1" in
      --output) output="$2"; shift 2 ;;
      --dump-header) headers="$2"; shift 2 ;;
      --header)
        if [[ "$2" == Idempotency-Key:* ]]; then
          id_header="${2#Idempotency-Key: }"
        fi
        shift 2
        ;;
      --form)
        case "$2" in
          request=@*) request_form="${2#request=@}" ;;
          archive=@*) archive_form="${2#archive=@}" ;;
          *) printf 'unexpected fake curl form: %q\n' "$2" >&2; return 97 ;;
        esac
        shift 2
        ;;
      --connect-timeout|--max-time|--write-out) shift 2 ;;
      --noproxy) shift 2 ;;
      --silent|--show-error) shift ;;
      http://127.0.0.1:8090/v1/agent/sessions) shift ;;
      *) printf 'unexpected fake curl argument: %q\n' "$1" >&2; return 97 ;;
    esac
  done
  local request_file="${request_form%;type=application/json}"
  local archive_file="${archive_form%;type=application/zip}"
  [[ "${id_header}" == "${TEST_EXPECTED_ID}" ]] || return 96
  [[ "${request_form}" == *';type=application/json' && -f "${request_file}" ]] || return 96
  [[ "${archive_form}" == *';type=application/zip' && -f "${archive_file}" ]] || return 96
  # The service echoes the accepted archive commitment on the wire body.
  local echo_bytes echo_sha
  echo_bytes="$(jq -r '.archive_bytes' "${request_file}")"
  echo_sha="$(jq -r '.archive_sha256' "${request_file}")"
  [[ "${echo_bytes}" == "$(stat -c '%s' "${archive_file}")" ]] || return 95
  [[ "${echo_sha}" == "$(sha256sum "${archive_file}" | awk '{print $1}')" ]] || return 95
  printf 'HTTP/1.1 %s synthetic\r\ncache-control: no-store\r\n\r\n' "${TEST_HTTP_STATUS}" >"${headers}"
  jq -n \
    --arg id "${TEST_EXPECTED_ID}" \
    --arg status "${TEST_RESPONSE_STATUS}" \
    --arg model "${TEST_MODEL}" \
    --argjson archive_bytes "${echo_bytes}" \
    --arg archive_sha256 "${echo_sha}" \
    --slurpfile request "${request_file}" \
    --argjson other_deliverables "${TEST_RESPONSE_DELIVERABLES:-null}" \
    '{session_id:$id,status:$status,progress_revision:1,progress_events:[],model:$model,context_window:262144,max_session_turns:400,
      deliverables:($other_deliverables // $request[0].deliverables),archive_bytes:$archive_bytes,archive_sha256:$archive_sha256}' \
    >"${output}"
  printf '%s' "${TEST_HTTP_STATUS}"
}

submission_post_receipt "${RUNNING_ID}" "${RUNNING_REQUEST}" >/dev/null
[[ ! -e "${SUBMISSION_RECEIPT_ROOT}/${RUNNING_ID}" ]] || {
  printf 'accepted running receipt was not removed\n' >&2
  exit 1
}

TERMINAL_REQUEST="$(submission_create_receipt "${TERMINAL_ID}" "${FIXTURE_DIR}" "${PROMPT_FILE}" '[]')"
readonly TERMINAL_REQUEST
TEST_RESPONSE_STATUS='ended'
TEST_HTTP_STATUS=200
TEST_EXPECTED_ID="${TERMINAL_ID}"
submission_post_receipt "${TERMINAL_ID}" "${TERMINAL_REQUEST}" >/dev/null
[[ ! -e "${SUBMISSION_RECEIPT_ROOT}/${TERMINAL_ID}" ]] || {
  printf 'accepted terminal replay receipt was not removed\n' >&2
  exit 1
}

# An empty workspace serializes to the canonical 22-byte empty zip container
# and remains a valid, replayable, hash-committed receipt.
EMPTY_REQUEST="$(submission_create_receipt "${EMPTY_ID}" "${EMPTY_DIR}" "${PROMPT_FILE}" '[]')"
readonly EMPTY_REQUEST
assert_receipt "${EMPTY_ID}" "${EMPTY_REQUEST}"
[[ "$(stat -c '%s' "${SUBMISSION_RECEIPT_ROOT}/${EMPTY_ID}/archive.zip")" == 22 ]] || {
  printf 'empty workspace did not serialize to the canonical empty zip container\n' >&2
  exit 1
}
TEST_RESPONSE_STATUS='running'
TEST_HTTP_STATUS=202
TEST_EXPECTED_ID="${EMPTY_ID}"
submission_post_receipt "${EMPTY_ID}" "${EMPTY_REQUEST}" >/dev/null

# Byte drift between the archive and its recorded commitment must fail
# closed before any replay reaches the API.
TAMPER_REQUEST="$(submission_create_receipt "${TAMPER_ID}" "${FIXTURE_DIR}" "${PROMPT_FILE}" '[]')"
readonly TAMPER_REQUEST
chmod 0600 "${SUBMISSION_RECEIPT_ROOT}/${TAMPER_ID}/archive.zip"
printf 'X' >>"${SUBMISSION_RECEIPT_ROOT}/${TAMPER_ID}/archive.zip"
if submission_validate_receipt "${TAMPER_ID}" >/dev/null 2>&1; then
  printf 'a tampered receipt archive was accepted for replay\n' >&2
  exit 1
fi
rm -rf -- "${SUBMISSION_RECEIPT_ROOT}/${TAMPER_ID}"

# The optional creation-body field is exercised through the same one receipt
# path: an explicit budget must survive serialization and revalidation
# byte-exactly, and a budget outside the locked ceiling must be refused before
# any receipt exists.
BUDGET_REQUEST="$(submission_create_receipt "${BUDGET_ID}" "${FIXTURE_DIR}" "${PROMPT_FILE}" '[]' 700)"
readonly BUDGET_REQUEST
assert_receipt "${BUDGET_ID}" "${BUDGET_REQUEST}"
[[ "$(jq -r '.max_session_turns' "${BUDGET_REQUEST}")" == 700 ]] || {
  printf 'explicit turn budget was not recorded in the receipt\n' >&2
  exit 1
}
rm -rf -- "${SUBMISSION_RECEIPT_ROOT}/${BUDGET_ID}"
for rejected in 0 -1 "$((SUBMISSION_MAX_SESSION_TURNS_CEILING + 1))" 1.5 abc; do
  if submission_create_receipt "${REFUSED_ID}" "${FIXTURE_DIR}" "${PROMPT_FILE}" '[]' "${rejected}" \
    >/dev/null 2>&1; then
    printf 'turn budget %s was accepted by the client harness\n' "${rejected}" >&2
    exit 1
  fi
  [[ ! -e "${SUBMISSION_RECEIPT_ROOT}/${REFUSED_ID}" ]] || {
    printf 'a refused turn budget left a receipt behind: %s\n' "${rejected}" >&2
    exit 1
  }
done

# The required deliverables list survives serialization and revalidation
# byte-exactly, is required in every receipt, and must come back as the list
# the service accepted; anything but one JSON array of strings is refused
# before any receipt exists.
DECLARED_REQUEST="$(submission_create_receipt "${DECLARED_ID}" "${FIXTURE_DIR}" "${PROMPT_FILE}" \
  '["report.md", "out/data \u00e9.csv"]')"
readonly DECLARED_REQUEST
assert_receipt "${DECLARED_ID}" "${DECLARED_REQUEST}"
[[ "$(jq -c '.deliverables' "${DECLARED_REQUEST}")" == '["report.md","out/data é.csv"]' ]] || {
  printf 'declared deliverables were not recorded in the receipt: %s\n' "$(jq -c '.deliverables' "${DECLARED_REQUEST}")" >&2
  exit 1
}
[[ "$(jq -c 'keys' "${DECLARED_REQUEST}")" == '["archive_bytes","archive_sha256","deliverables","prompt"]' ]]
TEST_RESPONSE_STATUS='running'
TEST_HTTP_STATUS=202
TEST_EXPECTED_ID="${DECLARED_ID}"
TEST_RESPONSE_DELIVERABLES='["report.md"]'
if submission_post_receipt "${DECLARED_ID}" "${DECLARED_REQUEST}" >/dev/null 2>&1; then
  printf 'a response naming other deliverables than the receipt declared was accepted\n' >&2
  exit 1
fi
assert_receipt "${DECLARED_ID}" "${DECLARED_REQUEST}"
TEST_RESPONSE_DELIVERABLES=''
submission_post_receipt "${DECLARED_ID}" "${DECLARED_REQUEST}" >/dev/null
[[ ! -e "${SUBMISSION_RECEIPT_ROOT}/${DECLARED_ID}" ]] || {
  printf 'accepted declared-deliverables receipt was not removed\n' >&2
  exit 1
}
for rejected in '' 'null' '"report.md"' '[1]' '["a.md", null]' '{"report.md": true}' '[] []' 'not json'; do
  if submission_create_receipt "${REFUSED_ID}" "${FIXTURE_DIR}" "${PROMPT_FILE}" "${rejected}" \
    >/dev/null 2>&1; then
    printf 'deliverables %s were accepted by the client harness\n' "${rejected@Q}" >&2
    exit 1
  fi
  [[ ! -e "${SUBMISSION_RECEIPT_ROOT}/${REFUSED_ID}" ]] || {
    printf 'refused deliverables left a receipt behind: %s\n' "${rejected@Q}" >&2
    exit 1
  }
done
# A receipt without the list, or with a list of another type, is not a
# replayable creation body.
UNDECLARED_REQUEST="$(submission_create_receipt "${REFUSED_ID}" "${FIXTURE_DIR}" "${PROMPT_FILE}" '[]')"
for mutation in 'del(.deliverables)' '.deliverables = null' '.deliverables = ["a.md", 1]'; do
  chmod 0600 "${UNDECLARED_REQUEST}"
  jq "${mutation}" "${UNDECLARED_REQUEST}" >"${TEST_DIR}/mutated.json"
  cat "${TEST_DIR}/mutated.json" >"${UNDECLARED_REQUEST}"
  if submission_validate_receipt "${REFUSED_ID}" >/dev/null 2>&1; then
    printf 'a receipt mutated by %s was accepted for replay\n' "${mutation}" >&2
    exit 1
  fi
  jq '.deliverables = []' "${TEST_DIR}/mutated.json" >"${UNDECLARED_REQUEST}"
  assert_receipt "${REFUSED_ID}" "${UNDECLARED_REQUEST}"
done
rm -rf -- "${SUBMISSION_RECEIPT_ROOT}/${REFUSED_ID}"

INVALID_REQUEST="$(submission_create_receipt "${INVALID_ID}" "${FIXTURE_DIR}" "${PROMPT_FILE}" '[]')"
readonly INVALID_REQUEST
TEST_RESPONSE_STATUS='ended'
TEST_HTTP_STATUS=202
TEST_EXPECTED_ID="${INVALID_ID}"
if submission_post_receipt "${INVALID_ID}" "${INVALID_REQUEST}" >/dev/null 2>&1; then
  printf 'newly accepted HTTP 202 response with terminal state was accepted\n' >&2
  exit 1
fi
assert_receipt "${INVALID_ID}" "${INVALID_REQUEST}"

printf 'SUBMISSION_CONTRACT_OK archive-receipt=validated commitment=hash-checked empty-workspace=canonical tamper=fail-closed replay-running=accepted replay-terminal=accepted malformed-202=retained turn-budget=optional-and-bounded deliverables=required-and-echoed\n'
