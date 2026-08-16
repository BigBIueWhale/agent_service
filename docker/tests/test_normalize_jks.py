#!/usr/bin/env python3
# SPDX-License-Identifier: Unlicense

from __future__ import annotations

import hashlib
from pathlib import Path
import struct
import sys
import tempfile
import unittest


SCRIPT_DIR = Path(__file__).resolve().parent
NORMALIZER_DIR = SCRIPT_DIR.parent / "scripts"
if not NORMALIZER_DIR.is_dir():
    NORMALIZER_DIR = SCRIPT_DIR
sys.path.insert(0, str(NORMALIZER_DIR))

from normalize_jks import JKS_WHITENER, JksError, normalize_file, normalize_jks  # noqa: E402


PASSWORD = "changeit"


def _utf(value: bytes) -> bytes:
    return struct.pack(">H", len(value)) + value


def _synthetic_jks(timestamp_ms: int) -> bytes:
    certificate = b"synthetic-certificate"
    body = b"".join(
        (
            struct.pack(">III", 0xFEEDFEED, 2, 1),
            struct.pack(">I", 2),
            _utf(b"test-alias"),
            struct.pack(">q", timestamp_ms),
            _utf(b"X.509"),
            struct.pack(">I", len(certificate)),
            certificate,
        )
    )
    digest = hashlib.sha1(PASSWORD.encode("utf-16-be") + JKS_WHITENER + body).digest()
    return body + digest


class NormalizeJksTests(unittest.TestCase):
    def test_normalization_is_exact_and_idempotent(self) -> None:
        original = _synthetic_jks(1234)
        normalized = normalize_jks(original, 1_786_725_153, PASSWORD)
        self.assertNotEqual(normalized, original)
        self.assertEqual(normalize_jks(normalized, 1_786_725_153, PASSWORD), normalized)

    def test_wrong_password_is_rejected(self) -> None:
        with self.assertRaisesRegex(JksError, "integrity digest"):
            normalize_jks(_synthetic_jks(1234), 1_786_725_153, "wrong")

    def test_unknown_entry_tag_is_rejected(self) -> None:
        malformed = bytearray(_synthetic_jks(1234))
        struct.pack_into(">I", malformed, 12, 99)
        body = malformed[:-20]
        malformed[-20:] = hashlib.sha1(PASSWORD.encode("utf-16-be") + JKS_WHITENER + body).digest()
        with self.assertRaisesRegex(JksError, "unsupported entry tag"):
            normalize_jks(bytes(malformed), 1_786_725_153, PASSWORD)

    def test_file_replacement_preserves_mode_and_is_idempotent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "cacerts"
            path.write_bytes(_synthetic_jks(1234))
            path.chmod(0o640)
            self.assertTrue(normalize_file(path, 1_786_725_153, PASSWORD))
            first = path.read_bytes()
            self.assertEqual(path.stat().st_mode & 0o777, 0o640)
            self.assertFalse(normalize_file(path, 1_786_725_153, PASSWORD))
            self.assertEqual(path.read_bytes(), first)


if __name__ == "__main__":
    unittest.main()
