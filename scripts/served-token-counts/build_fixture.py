"""Write served-token-counts.json from one recording's counts.jsonl.

usage: build_fixture.py COUNTS_JSONL TOKENIZER_SHA256 OUTPUT_JSON

A digest recorded with two counts is refused: one text has one count.
"""
import json
import sys

counts_path, tokenizer_sha, output = sys.argv[1:4]
counts: dict[str, int] = {}
for line in open(counts_path, encoding='utf-8'):
    row = json.loads(line)
    if counts.setdefault(row['sha'], row['n']) != row['n']:
        sys.exit(f"{row['sha']} was counted as {counts[row['sha']]} and as {row['n']}")
document = {
    'tokenizer': {'file': 'tokenizer.json', 'sha256': tokenizer_sha},
    'counted': 'len(Tokenizer.from_file(tokenizer.json).encode(text, '
    'add_special_tokens=False).ids), as vLLM /tokenize answers a '
    'completion-form prompt with add_special_tokens false',
    'counts': dict(sorted(counts.items())),
}
with open(output, 'w', encoding='utf-8') as handle:
    json.dump(document, handle, indent=2)
    handle.write('\n')
print(f'{len(counts)} counts')
