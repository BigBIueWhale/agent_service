// The patched CLI's normalizer over the agreement probes: String.prototype
// .normalize on the Node the agent image runs the CLI with.
import { readFileSync, writeFileSync } from 'node:fs';

const probes = JSON.parse(readFileSync('/work/probes.json', 'utf8'));
const out = { unicode_version: process.versions.unicode };
for (const kind of ['alone', 'acute', 'ypo']) {
  out[kind] = probes[kind].map((text) => text.normalize('NFC'));
}
writeFileSync('/work/node.json', JSON.stringify(out));
