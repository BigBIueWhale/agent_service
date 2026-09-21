"""Every normalizer that measures a byte bound, held to the served tokenizer's.

Runs where the served tokenizer runs, on its tokenizer.json, and compares
UTF-8 lengths probe by probe:

- A probe normalized by a measuring normalizer against the same probe
  normalized by the tokenizer. Shorter is a defect: that normalizer would
  measure the text as written smaller than the tokenizer counts it. Alone,
  no code point may come out shorter. Inside a probe, a normalizer on a newer
  Unicode composes across a combining mark that the tokenizer's older Unicode
  treats as a starter, and comes out shorter; that is reported, and it is why
  no bound measures the text as written: the measured form is what is sent.
- The tokenizer's normalizer over each measuring normalizer's output against
  that output. Longer is a defect, and none is allowed: the measured form is
  the text every bound hands on, so this is what the tokenizer reads.
- The service's normalizer against the materializer's, text for text. They
  must agree exactly, or the materializer would admit a prompt the service
  refuses, or refuse one it admits.
"""

import json
import sys

import tokenizers
from tokenizers import Tokenizer

KINDS = ("alone", "acute", "ypo")
MEASURING = ("node", "rust", "python")

tokenizer = Tokenizer.from_file("/tokenizer/tokenizer.json")
normalize = tokenizer.normalizer.normalize_str


def utf8(text: str) -> int:
    return len(text.encode("utf-8"))


with open("/work/probes.json", encoding="utf-8") as handle:
    probes = json.load(handle)
code_points = probes["code_points"]
served = {kind: [utf8(normalize(text)) for text in probes[kind]] for kind in KINDS}
print(
    f"served tokenizer: tokenizers {tokenizers.__version__}, normalizer "
    f"{json.dumps(json.loads(tokenizer.to_str())['normalizer'])}; "
    f"{len(code_points)} code points, alone and in {len(KINDS) - 1} probes"
)

defects = []
outputs = {}
for name in MEASURING:
    with open(f"/work/{name}.json", encoding="utf-8") as handle:
        measured = json.load(handle)
    if name in ("rust", "python"):
        outputs[name] = {kind: measured[kind] for kind in KINDS}
    for kind in KINDS:
        shorter = [cp for cp, text, length in zip(code_points, measured[kind], served[kind]) if utf8(text) < length]
        lengthened = [cp for cp, text in zip(code_points, measured[kind]) if utf8(normalize(text)) > utf8(text)]
        examples = " ".join(f"U+{cp:04X}" for cp in shorter[:4])
        print(
            f"{name:6} (Unicode {measured['unicode_version']}) {kind:5}: "
            f"shorter than the tokenizer {len(shorter):4}{' e.g. ' + examples if shorter else ''}; "
            f"the tokenizer lengthens its output {len(lengthened)}"
        )
        if kind == "alone" and shorter:
            defects.append(f"{name} measures {len(shorter)} code points alone shorter than the tokenizer normalizes them")
        if lengthened:
            defects.append(f"the tokenizer lengthens {len(lengthened)} of {name}'s {kind} outputs")
    del measured

for kind in KINDS:
    differing = [cp for cp, a, b in zip(code_points, outputs["rust"][kind], outputs["python"][kind]) if a != b]
    print(f"service and materializer {kind:5}: {len(differing)} probes normalize differently")
    if differing:
        defects.append(f"the service and the materializer normalize {len(differing)} {kind} probes differently")

if defects:
    print("NORMALIZER AGREEMENT FAILED: " + "; ".join(defects))
    sys.exit(1)
print("NORMALIZER AGREEMENT HOLDS")
