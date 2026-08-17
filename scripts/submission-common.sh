#!/usr/bin/env bash

# Shared implementation for the one session-creation operation.  The public
# entry points are run.sh (create a caller-known handle and receipt) and
# resubmit.sh (replay that exact receipt after an ambiguous transport result).
# This file is sourced; it deliberately has no executable mode or alternate
# command-line surface.

readonly SUBMISSION_API='http://127.0.0.1:8090'
readonly SUBMISSION_RECEIPT_ROOT="${SCRIPT_DIR}/.runtime/client-submissions"
readonly SUBMISSION_MAX_ATTEMPTS=5

submission_die() {
  printf 'ERROR: %s\n' "$*" >&2
  return 1
}

submission_require_private_directory() {
  local path="$1" role="$2"
  [[ -d "${path}" && ! -L "${path}" ]] ||
    submission_die "${role} is not an ordinary directory: ${path}" || return
  local identity
  identity="$(stat -c '%u:%g:%a' -- "${path}")" ||
    submission_die "cannot stat ${role}: ${path}" || return
  [[ "${identity}" == '1000:1000:700' ]] ||
    submission_die "${role} ${path} has ${identity}; required owner/group/mode is 1000:1000:700" || return
}

submission_ensure_receipt_root() {
  local runtime_root="${SCRIPT_DIR}/.runtime"
  submission_require_private_directory "${runtime_root}" 'runtime root' || return
  if [[ ! -e "${SUBMISSION_RECEIPT_ROOT}" ]]; then
    mkdir --mode=0700 -- "${SUBMISSION_RECEIPT_ROOT}" ||
      submission_die "cannot create private submission-receipt root ${SUBMISSION_RECEIPT_ROOT}" || return
    sync -f -- "${runtime_root}" ||
      submission_die "cannot durably synchronize new submission-receipt root" || return
  fi
  submission_require_private_directory "${SUBMISSION_RECEIPT_ROOT}" 'submission-receipt root'
}

submission_require_handle() {
  local session_id="$1"
  [[ "${session_id}" =~ ^s-[0-9a-f]{64}$ ]] ||
    submission_die "session handle must be s- followed by exactly 64 lowercase hexadecimal characters"
}

# Maximum accepted archive bytes: the server's staged-content cap plus its
# fixed zip container-overhead allowance (4 GiB + 64 MiB).
readonly SUBMISSION_MAX_ARCHIVE_BYTES=4362076160

submission_validate_receipt() {
  local session_id="$1"
  submission_require_handle "${session_id}" || return
  submission_require_private_directory "${SUBMISSION_RECEIPT_ROOT}" 'submission-receipt root' || return

  local receipt_dir="${SUBMISSION_RECEIPT_ROOT}/${session_id}"
  local request_file="${receipt_dir}/request.json"
  local archive_file="${receipt_dir}/archive.zip"
  submission_require_private_directory "${receipt_dir}" "receipt for ${session_id}" || return

  local entry_list
  entry_list="$(find "${receipt_dir}" -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort)" ||
    submission_die "cannot enumerate receipt for ${session_id}" || return
  [[ "${entry_list}" == $'archive.zip\nrequest.json' ]] ||
    submission_die "receipt for ${session_id} does not contain exactly archive.zip and request.json; observed entries: ${entry_list@Q}" || return
  local file identity
  for file in "${request_file}" "${archive_file}"; do
    [[ -f "${file}" && ! -L "${file}" ]] ||
      submission_die "receipt entry is not an ordinary non-symlink file: ${file}" || return
    identity="$(stat -c '%u:%g:%a' -- "${file}")" ||
      submission_die "cannot stat receipt entry ${file}" || return
    [[ "${identity}" == '1000:1000:600' ]] ||
      submission_die "receipt entry ${file} has ${identity}; required owner/group/mode is 1000:1000:600" || return
  done

  local request_bytes archive_bytes
  request_bytes="$(stat -c '%s' -- "${request_file}")" ||
    submission_die "cannot read receipt size ${request_file}" || return
  ((request_bytes > 0 && request_bytes <= 2097152)) ||
    submission_die "receipt request is ${request_bytes} bytes; required range is 1..2097152" || return
  archive_bytes="$(stat -c '%s' -- "${archive_file}")" ||
    submission_die "cannot read receipt archive size ${archive_file}" || return
  ((archive_bytes > 0 && archive_bytes <= SUBMISSION_MAX_ARCHIVE_BYTES)) ||
    submission_die "receipt archive is ${archive_bytes} bytes; required range is 1..${SUBMISSION_MAX_ARCHIVE_BYTES}" || return

  jq -e --argjson archive_bytes "${archive_bytes}" '
    type == "object" and
    ((keys == ["archive_bytes", "archive_sha256", "prompt"]) or
     (keys == ["archive_bytes", "archive_sha256", "preserve_thinking", "prompt"])) and
    (.prompt | type == "string" and length > 0) and
    (.archive_bytes == $archive_bytes) and
    (.archive_sha256 | type == "string" and test("^[0-9a-f]{64}$")) and
    ((has("preserve_thinking") | not) or
     (.preserve_thinking | type == "boolean"))
  ' "${request_file}" >/dev/null ||
    submission_die "receipt request for ${session_id} violates the one creation-body schema or disagrees with its archive" || return

  # The commitment is byte-exact: the archive on disk must still hash to the
  # value the request will declare, on every replay, before it reaches the
  # API under a trusted handle.
  local declared_sha observed_sha
  declared_sha="$(jq -r '.archive_sha256' "${request_file}")" ||
    submission_die "cannot read declared archive hash for ${session_id}" || return
  observed_sha="$(sha256sum -- "${archive_file}" | awk '{print $1}')" ||
    submission_die "cannot hash receipt archive ${archive_file}" || return
  [[ "${observed_sha}" == "${declared_sha}" ]] ||
    submission_die "receipt archive for ${session_id} hashes to ${observed_sha} but the request declares ${declared_sha}" || return

  printf '%s\n' "${request_file}"
}

submission_create_receipt() {
  local session_id="$1" folder="$2" prompt_file="$3" preserve_thinking="${4-}"
  submission_require_handle "${session_id}" || return
  [[ -z "${preserve_thinking}" || "${preserve_thinking}" == true || "${preserve_thinking}" == false ]] ||
    submission_die "preserve_thinking must be omitted, true, or false; got ${preserve_thinking@Q}" || return
  submission_ensure_receipt_root || return

  local receipt_dir="${SUBMISSION_RECEIPT_ROOT}/${session_id}"
  local archive_next="${receipt_dir}/archive.zip.next"
  local archive_file="${receipt_dir}/archive.zip"
  local request_next="${receipt_dir}/request.json.next"
  local request_file="${receipt_dir}/request.json"
  mkdir --mode=0700 -- "${receipt_dir}" ||
    submission_die "cannot exclusively create receipt for ${session_id}; preserve any collision for inspection" || return

  # Once the private directory exists, every failure preserves it.  An
  # incomplete receipt is explicit evidence and cannot be mistaken for a
  # replayable request by submission_validate_receipt.
  #
  # The workspace travels over the connection as one zip: the exact bytes are
  # frozen into the receipt now, so a later replay resends bit-identical
  # content no matter what happens to the original folder. `zip -y` stores
  # symbolic links as links, matching the service's opaque-symlink staging.
  if [[ -z "$(find "${folder}" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    # Info-ZIP refuses to write an archive with no entries; an empty
    # workspace is the canonical 22-byte empty zip container.
    printf 'PK\x05\x06\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00' >"${archive_next}" ||
      submission_die "cannot write empty-workspace archive for ${session_id}" || return
  else
    (cd -- "${folder}" && zip -ryq "${archive_next}" .) ||
      submission_die "cannot serialize workspace folder ${folder} into the receipt archive for ${session_id}" || return
  fi
  chmod 0600 -- "${archive_next}" ||
    submission_die "cannot set private mode on receipt archive for ${session_id}" || return
  sync -f -- "${archive_next}" ||
    submission_die "cannot durably synchronize receipt archive for ${session_id}" || return
  mv --no-clobber -- "${archive_next}" "${archive_file}" ||
    submission_die "cannot atomically publish receipt archive for ${session_id}" || return

  local archive_bytes archive_sha256
  archive_bytes="$(stat -c '%s' -- "${archive_file}")" ||
    submission_die "cannot read receipt archive size for ${session_id}" || return
  archive_sha256="$(sha256sum -- "${archive_file}" | awk '{print $1}')" ||
    submission_die "cannot hash receipt archive for ${session_id}" || return

  if [[ -z "${preserve_thinking}" ]]; then
    jq -n --argjson archive_bytes "${archive_bytes}" \
      --arg archive_sha256 "${archive_sha256}" \
      --rawfile prompt "${prompt_file}" \
      '{prompt:$prompt,archive_bytes:$archive_bytes,archive_sha256:$archive_sha256}' >"${request_next}" ||
      submission_die "cannot serialize exact request receipt for ${session_id}" || return
  else
    jq -n --argjson archive_bytes "${archive_bytes}" \
      --arg archive_sha256 "${archive_sha256}" \
      --argjson preserve_thinking "${preserve_thinking}" \
      --rawfile prompt "${prompt_file}" \
      '{prompt:$prompt,preserve_thinking:$preserve_thinking,archive_bytes:$archive_bytes,archive_sha256:$archive_sha256}' >"${request_next}" ||
      submission_die "cannot serialize exact request receipt for ${session_id}" || return
  fi
  chmod 0600 -- "${request_next}" ||
    submission_die "cannot set private mode on receipt for ${session_id}" || return
  sync -f -- "${request_next}" ||
    submission_die "cannot durably synchronize private receipt body for ${session_id}" || return
  mv --no-clobber -- "${request_next}" "${request_file}" ||
    submission_die "cannot atomically publish private receipt for ${session_id}" || return
  sync -f -- "${request_file}" ||
    submission_die "cannot synchronize published receipt body for ${session_id}" || return
  sync -f -- "${receipt_dir}" ||
    submission_die "cannot synchronize receipt directory for ${session_id}" || return
  sync -f -- "${SUBMISSION_RECEIPT_ROOT}" ||
    submission_die "cannot synchronize receipt-root publication for ${session_id}" || return

  submission_validate_receipt "${session_id}"
}

submission_remove_receipt() {
  local session_id="$1" request_file="$2"
  local receipt_dir="${SUBMISSION_RECEIPT_ROOT}/${session_id}"
  [[ "${request_file}" == "${receipt_dir}/request.json" ]] ||
    submission_die "refusing to remove receipt through an unexpected path: ${request_file}" || return
  submission_validate_receipt "${session_id}" >/dev/null || return

  rm -- "${request_file}" "${receipt_dir}/archive.zip" ||
    submission_die "server accepted ${session_id}, but its local receipt could not be removed; replay remains safe" || return
  sync -f -- "${receipt_dir}" ||
    submission_die "server accepted ${session_id}, but receipt-file removal could not be synchronized" || return
  rmdir -- "${receipt_dir}" ||
    submission_die "server accepted ${session_id}, but its now-empty receipt directory could not be removed" || return
  sync -f -- "${SUBMISSION_RECEIPT_ROOT}" ||
    submission_die "server accepted ${session_id}, but receipt-directory removal could not be synchronized" || return
}

submission_response_is_valid() {
  local session_id="$1" http_status="$2" response_file="$3"
  local required_status
  if [[ "${http_status}" == 202 ]]; then
    required_status='running'
  else
    required_status='running|completed|cancelled'
  fi
  local receipt_dir="${SUBMISSION_RECEIPT_ROOT}/${session_id}"
  local declared_bytes declared_sha
  declared_bytes="$(jq -r '.archive_bytes' "${receipt_dir}/request.json")" || return 1
  declared_sha="$(jq -r '.archive_sha256' "${receipt_dir}/request.json")" || return 1
  jq -e --arg id "${session_id}" --arg statuses "${required_status}" '
    . as $body |
    .session_id == $id and
    (.status | type == "string") and
    (($statuses | split("|") | index($body.status)) != null)
  ' "${response_file}" >/dev/null 2>&1 &&
    jq -e --argjson archive_bytes "${declared_bytes}" --arg archive_sha256 "${declared_sha}" '
      (.progress_revision | type == "number") and
      (.progress_events | type == "array") and
      (.model | type == "string" and length > 0) and
      (.context_window | type == "number" and . == 262144) and
      (.preserve_thinking | type == "boolean") and
      (.archive_bytes == $archive_bytes) and
      (.archive_sha256 == $archive_sha256)
    ' "${response_file}" >/dev/null
}

submission_post_receipt() {
  local session_id="$1" request_file="$2"
  local response_file header_file
  response_file="$(mktemp /tmp/agent-service-response.XXXXXX)"
  header_file="$(mktemp /tmp/agent-service-response-headers.XXXXXX)"
  case "${response_file}" in /tmp/agent-service-response.*) ;; *) submission_die "unexpected response scratch path: ${response_file}"; return ;; esac
  case "${header_file}" in /tmp/agent-service-response-headers.*) ;; *) rm -f -- "${response_file}"; submission_die "unexpected header scratch path: ${header_file}"; return ;; esac

  local attempt curl_status http_status retryable delay
  for ((attempt = 1; attempt <= SUBMISSION_MAX_ATTEMPTS; attempt++)); do
    # Revalidate immediately before every replay. A malformed or externally
    # replaced receipt never reaches the API under a trusted handle.
    request_file="$(submission_validate_receipt "${session_id}")" || {
      rm -f -- "${response_file}" "${header_file}"
      return 1
    }
    : >"${response_file}"
    : >"${header_file}"
    set +e
    # The workspace archive itself streams through the connection as the
    # second multipart part; nothing is referenced by filesystem path.
    http_status="$(curl --noproxy '*' --silent --show-error \
      --connect-timeout 5 --max-time 900 \
      --output "${response_file}" --dump-header "${header_file}" \
      --write-out '%{http_code}' \
      --header "Idempotency-Key: ${session_id}" \
      --form "request=@${request_file};type=application/json" \
      --form "archive=@${SUBMISSION_RECEIPT_ROOT}/${session_id}/archive.zip;type=application/zip" \
      "${SUBMISSION_API}/v1/agent/sessions")"
    curl_status=$?
    set -e

    if ((curl_status == 0)) && [[ "${http_status}" == 200 || "${http_status}" == 202 ]]; then
      if ! submission_response_is_valid "${session_id}" "${http_status}" "${response_file}"; then
        printf 'ERROR: HTTP %s response violated the required session-resource schema for %s:\n' \
          "${http_status}" "${session_id}" >&2
        jq . "${response_file}" >&2 || sed -n '1,80p' "${response_file}" >&2
        printf 'The private exact receipt remains at %s; do not create another handle.\n' \
          "${request_file}" >&2
        rm -f -- "${response_file}" "${header_file}"
        return 1
      fi
      jq . "${response_file}"
      if ! submission_remove_receipt "${session_id}" "${request_file}"; then
        printf 'The operation is accepted, but local receipt cleanup is incomplete. Replaying ./resubmit.sh %s is safe.\n' \
          "${session_id}" >&2
        rm -f -- "${response_file}" "${header_file}"
        return 1
      fi
      printf 'Accepted. The operation is connection-independent; optional later read: ./session.sh %s\n' \
        "${session_id}" >&2
      rm -f -- "${response_file}" "${header_file}"
      return 0
    fi

    retryable=false
    if ((curl_status != 0)); then
      retryable=true
    else
      case "${http_status}" in
        408|425|429|500|502|503|504) retryable=true ;;
      esac
    fi
    if [[ "${retryable}" == true && "${attempt}" -lt "${SUBMISSION_MAX_ATTEMPTS}" ]]; then
      delay=$((1 << (attempt - 1)))
      printf 'Transient submission result for %s (curl=%s HTTP=%s); replaying the identical receipt and handle in %ss.\n' \
        "${session_id}" "${curl_status}" "${http_status:-000}" "${delay}" >&2
      sleep "${delay}"
      continue
    fi

    printf 'ERROR: submission did not prove durable acceptance (curl=%s HTTP=%s).\n' \
      "${curl_status}" "${http_status:-000}" >&2
    if [[ -s "${response_file}" ]]; then
      jq . "${response_file}" >&2 || sed -n '1,80p' "${response_file}" >&2
    fi
    printf 'The exact body and handle remain private and durable at %s. Retry only with: ./resubmit.sh %s\n' \
      "${request_file}" "${session_id}" >&2
    rm -f -- "${response_file}" "${header_file}"
    return 1
  done

  rm -f -- "${response_file}" "${header_file}"
  submission_die "unreachable submission-loop exit for ${session_id}"
}
