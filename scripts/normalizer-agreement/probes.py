"""The normalizer agreement probes, and the suite materializer's normalizer over them.

Every Unicode scalar value, alone and inside two probes that put it between a
base and a combining mark it could block or reorder: `a` + it + U+0301 and
U+03B1 + it + U+0345. The probes are written as probes.json. The materializer
measures prompts with this host's Python `unicodedata`, and its NFC of every
probe is written as python.json.
"""

import json
import sys
import unicodedata
from pathlib import Path

KINDS = {"alone": "{}", "acute": "a{}́", "ypo": "α{}ͅ"}

out = Path(sys.argv[1])
scalars = [cp for cp in range(0x110000) if not 0xD800 <= cp <= 0xDFFF]
probes = {kind: [form.format(chr(cp)) for cp in scalars] for kind, form in KINDS.items()}
(out / "probes.json").write_text(
    json.dumps({"code_points": scalars, **probes}, ensure_ascii=False), encoding="utf-8"
)
(out / "python.json").write_text(
    json.dumps(
        {
            "unicode_version": unicodedata.unidata_version,
            **{kind: [unicodedata.normalize("NFC", text) for text in texts] for kind, texts in probes.items()},
        },
        ensure_ascii=False,
    ),
    encoding="utf-8",
)
