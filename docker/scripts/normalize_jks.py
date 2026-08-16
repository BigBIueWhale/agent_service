#!/usr/bin/env python3
# SPDX-License-Identifier: Unlicense
"""Normalize entry timestamps in a Java JKS keystore without changing entries."""

from __future__ import annotations

import hashlib
import hmac
import os
from pathlib import Path
import struct
import sys
import tempfile


JKS_MAGIC = 0xFEEDFEED
JKS_VERSION = 2
JKS_DIGEST_SIZE = hashlib.sha1().digest_size
JKS_WHITENER = b"Mighty Aphrodite"


class JksError(ValueError):
    """Raised when the input is not the exact supported JKS structure."""


def _read_u16(data: bytes | bytearray, offset: int) -> tuple[int, int]:
    if offset + 2 > len(data):
        raise JksError("truncated two-byte field")
    return struct.unpack_from(">H", data, offset)[0], offset + 2


def _read_u32(data: bytes | bytearray, offset: int) -> tuple[int, int]:
    if offset + 4 > len(data):
        raise JksError("truncated four-byte field")
    return struct.unpack_from(">I", data, offset)[0], offset + 4


def _skip(data: bytes | bytearray, offset: int, length: int, role: str) -> int:
    if length < 0 or offset + length > len(data):
        raise JksError(f"truncated {role}")
    return offset + length


def _skip_utf(data: bytes | bytearray, offset: int, role: str) -> int:
    length, offset = _read_u16(data, offset)
    return _skip(data, offset, length, role)


def _skip_certificate(data: bytes | bytearray, offset: int) -> int:
    offset = _skip_utf(data, offset, "certificate type")
    length, offset = _read_u32(data, offset)
    return _skip(data, offset, length, "certificate body")


def _timestamp_offsets(body: bytes | bytearray) -> list[int]:
    if len(body) < 12:
        raise JksError("truncated JKS header")
    magic, version, entry_count = struct.unpack_from(">III", body, 0)
    if magic != JKS_MAGIC:
        raise JksError(f"unexpected JKS magic 0x{magic:08x}")
    if version != JKS_VERSION:
        raise JksError(f"unsupported JKS version {version}; expected {JKS_VERSION}")

    offset = 12
    timestamps: list[int] = []
    for entry_index in range(entry_count):
        tag, offset = _read_u32(body, offset)
        offset = _skip_utf(body, offset, f"entry {entry_index} alias")
        if offset + 8 > len(body):
            raise JksError(f"truncated entry {entry_index} timestamp")
        timestamps.append(offset)
        offset += 8

        if tag == 1:  # private key and certificate chain
            encrypted_key_length, offset = _read_u32(body, offset)
            offset = _skip(body, offset, encrypted_key_length, "encrypted private key")
            chain_length, offset = _read_u32(body, offset)
            for _ in range(chain_length):
                offset = _skip_certificate(body, offset)
        elif tag == 2:  # trusted certificate
            offset = _skip_certificate(body, offset)
        else:
            raise JksError(f"unsupported entry tag {tag} at index {entry_index}")

    if offset != len(body):
        raise JksError(f"unexpected {len(body) - offset} bytes after the final entry")
    return timestamps


def _digest(password: str, body: bytes | bytearray) -> bytes:
    try:
        encoded_password = password.encode("utf-16-be")
    except UnicodeEncodeError as exc:
        raise JksError("JKS password contains an unsupported character") from exc
    return hashlib.sha1(encoded_password + JKS_WHITENER + body).digest()


def normalize_jks(data: bytes, epoch_seconds: int, password: str) -> bytes:
    if len(data) < JKS_DIGEST_SIZE:
        raise JksError("file is too small to be a JKS keystore")
    if epoch_seconds < 0 or epoch_seconds > ((1 << 63) - 1) // 1000:
        raise JksError("epoch seconds are outside the signed JKS timestamp range")

    body = bytearray(data[:-JKS_DIGEST_SIZE])
    expected_digest = data[-JKS_DIGEST_SIZE:]
    observed_digest = _digest(password, body)
    if not hmac.compare_digest(observed_digest, expected_digest):
        raise JksError("JKS integrity digest does not match the supplied password")

    offsets = _timestamp_offsets(body)
    timestamp_ms = epoch_seconds * 1000
    for offset in offsets:
        struct.pack_into(">q", body, offset, timestamp_ms)

    normalized = bytes(body) + _digest(password, body)
    normalized_body = normalized[:-JKS_DIGEST_SIZE]
    for offset in _timestamp_offsets(normalized_body):
        if struct.unpack_from(">q", normalized_body, offset)[0] != timestamp_ms:
            raise AssertionError("post-write JKS timestamp validation failed")
    return normalized


def normalize_file(path: Path, epoch_seconds: int, password: str) -> bool:
    source = path.read_bytes()
    normalized = normalize_jks(source, epoch_seconds, password)
    if normalized == source:
        return False

    metadata = path.stat()
    temporary_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=f".{path.name}.", delete=False) as temporary:
            temporary_name = temporary.name
            temporary.write(normalized)
            temporary.flush()
            os.fsync(temporary.fileno())
        os.chmod(temporary_name, metadata.st_mode)
        os.chown(temporary_name, metadata.st_uid, metadata.st_gid)
        os.replace(temporary_name, path)
        temporary_name = None
    finally:
        if temporary_name is not None:
            os.unlink(temporary_name)

    if path.read_bytes() != normalized:
        raise OSError("atomic JKS replacement did not persist the expected bytes")
    return True


def main(argv: list[str]) -> int:
    if len(argv) != 4:
        print(f"usage: {argv[0]} JKS_PATH SOURCE_DATE_EPOCH PASSWORD", file=sys.stderr)
        return 64
    try:
        epoch_seconds = int(argv[2], 10)
    except ValueError:
        print("SOURCE_DATE_EPOCH must be a base-10 integer", file=sys.stderr)
        return 64

    try:
        changed = normalize_file(Path(argv[1]), epoch_seconds, argv[3])
    except (JksError, OSError) as exc:
        print(f"JKS normalization failed: {exc}", file=sys.stderr)
        return 1
    print(f"JKS timestamps {'normalized' if changed else 'already normalized'}: {argv[1]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
