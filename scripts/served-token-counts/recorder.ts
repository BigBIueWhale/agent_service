/**
 * @license
 * Copyright 2026 Qwen
 * SPDX-License-Identifier: Apache-2.0
 */

// The recording counter ./record.sh puts in place of
// packages/core/src/test-utils/token-counter.ts while it runs the tests, and
// takes away after: every text a test counts is counted by the served
// tokenizer (count_server.py, on /sock/count.sock) and recorded, by the
// SHA-256 of its UTF-8, in /rec/counts.jsonl. It exports what the counter
// does, so the tests run as they do against the record.
import { execFileSync } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const counted = new Map<string, number>();

export function servedTokens(text: string): number {
  const sha = createHash('sha256').update(text, 'utf8').digest('hex');
  const known = counted.get(sha);
  if (known !== undefined) return known;
  const reply = execFileSync(
    'curl',
    [
      '--silent',
      '--fail',
      '--unix-socket',
      '/sock/count.sock',
      '--data-binary',
      '@-',
      'http://tokenizer/count',
    ],
    { input: Buffer.from(text, 'utf8'), maxBuffer: 1 << 20 },
  );
  const n = Number(reply.toString());
  if (!Number.isInteger(n) || n < 0) {
    throw new Error(`the served tokenizer answered ${JSON.stringify(reply.toString())}, not a count`);
  }
  counted.set(sha, n);
  appendFileSync('/rec/counts.jsonl', `${JSON.stringify({ sha, n })}\n`);
  return n;
}

export async function countServedTokens(text: string): Promise<number> {
  return servedTokens(text);
}
