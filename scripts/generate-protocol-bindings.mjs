#!/usr/bin/env node
// Compile the sole shared schema using Qwen's pinned build dependency.
import { createHash } from "node:crypto";
import { readFileSync, mkdirSync, writeFileSync, lstatSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";

const arguments_ = process.argv.slice(2);
if (
  arguments_.length < 1 ||
  arguments_.length > 2 ||
  (arguments_.length === 2 && arguments_[1] !== "--check")
) {
  throw new Error(
    "usage: generate-protocol-bindings.mjs QWEN_SOURCE [--check]",
  );
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const source = resolve(arguments_[0]);
const require = createRequire(resolve(source, "packages/core/package.json"));
const Ajv = require("ajv/dist/ajv.js").default;
const standalone = require("ajv/dist/standalone/index.js").default;
const esbuild = require("esbuild");
const schemaPath = resolve(root, "protocol/stream-contract-v1.json");
const generatorPath = fileURLToPath(import.meta.url);
const lockPath = resolve(source, "package-lock.json");
function readRegular(path) {
  if (!lstatSync(path).isFile())
    throw new Error(`binding input is not a regular file: ${path}`);
  return readFileSync(path);
}
const bytes = readRegular(schemaPath);
const generatorBytes = readRegular(generatorPath);
const lockBytes = readRegular(lockPath);
const schema = JSON.parse(bytes);
// MCP is an independently owned protocol inside our control envelope. Preserve
// its pinned SDK definition mechanically rather than maintaining another shape.
const { JSONRPCMessageSchema } = require("@modelcontextprotocol/sdk/types.js");
const { toJSONSchema } = require("zod/v4");
if (
  !isDeepStrictEqual(
    schema.definitions.mcpMessage,
    toJSONSchema(JSONRPCMessageSchema, { target: "draft-7" }),
  )
) {
  throw new Error("Embedded MCP schema differs from its pinned protocol owner");
}
const hash = createHash("sha256").update(bytes).digest("hex");
const names = schema.oneOf.map((variant) => variant.properties.type.const);
if (new Set(names).size !== names.length)
  throw new Error("duplicate event discriminator");
const header =
  "// Generated from agent_service/protocol/stream-contract-v1.json.\n" +
  "// Run scripts/generate-protocol-bindings.mjs; do not edit derived bindings.\n";
const outputs = new Map();
outputs.set(
  "stream-contract.ts",
  header +
    `export const STREAM_CONTRACT_SHA256 = '${hash}';\n` +
    `export const STREAM_EVENT_KINDS = ${JSON.stringify(names)} as const;\n` +
    `export const OUTBOUND_CONTROL_KINDS = ${JSON.stringify(schema.definitions.outboundControlMessage.oneOf.map((v) => v.properties.type.const))} as const;\n` +
    `export const PARTIAL_EVENT_KINDS = ${JSON.stringify(schema.definitions.streamEvent.oneOf.map((v) => v.properties.type.const))} as const;\n` +
    `export const SYSTEM_EVENT_KINDS = ${JSON.stringify(schema.oneOf.find((v) => v.properties.type.const === "system").oneOf.map((v) => v.properties.subtype.const))} as const;\n` +
    `export const RESULT_SUCCESS_SUBTYPE = ${JSON.stringify(schema.definitions.resultSuccessSubtype.const)} as const;\n` +
    `export const RESULT_ERROR_SUBTYPES = ${JSON.stringify(schema.definitions.resultErrorSubtype.enum)} as const;\n` +
    `export const GOAL_CHECKPOINT_EVIDENCE_MAX_BYTES = ${schema.definitions.goalCheckpointEvidenceBytes.maximum};\n`,
);

const ajv = new Ajv({
  strict: true,
  strictTypes: false,
  allowUnionTypes: true,
  code: { source: true, esm: true },
});
ajv.addSchema(schema);
for (const [file, entry] of [
  ["stream-record-validator", schema.$id],
  ["goal-snapshot-validator", `${schema.$id}#/definitions/goalSnapshot`],
  [
    "outbound-control-validator",
    `${schema.$id}#/definitions/outboundControlMessage`,
  ],
]) {
  const validator = ajv.getSchema(entry);
  if (!validator)
    throw new Error(`canonical schema entry is unavailable: ${entry}`);
  const compiled = await esbuild.build({
    absWorkingDir: source,
    stdin: {
      contents: standalone(ajv, validator),
      loader: "js",
      // A clean checkout has no generated directory yet. Resolve runtime
      // helpers from the package that owns the pinned compiler dependency.
      resolveDir: resolve(source, "packages/core"),
      sourcefile: `${file}.js`,
    },
    bundle: true,
    platform: "neutral",
    format: "esm",
    target: "es2022",
    write: false,
    legalComments: "none",
  });
  if (compiled.outputFiles.length !== 1)
    throw new Error("validator compiler did not emit one module");
  outputs.set(`${file}.js`, header + compiled.outputFiles[0].text);
  outputs.set(
    `${file}.d.ts`,
    header +
      "import type { ValidateFunction } from 'ajv';\n" +
      "declare const validate: ValidateFunction;\nexport { validate };\nexport default validate;\n",
  );
}

outputs.set(
  "stream-binding-manifest.json",
  JSON.stringify(
    {
      v: 1,
      schema_sha256: hash,
      generator_sha256: createHash("sha256")
        .update(generatorBytes)
        .digest("hex"),
      package_lock_sha256: createHash("sha256").update(lockBytes).digest("hex"),
      files: Object.fromEntries(
        [...outputs].map(([file, content]) => [
          file,
          createHash("sha256").update(content).digest("hex"),
        ]),
      ),
    },
    null,
    2,
  ) + "\n",
);

for (const name of ["goal-state-v1", "partial-stream-v1"]) {
  const expected = readRegular(
    resolve(root, `protocol/test-vectors/${name}.json`),
  );
  const actual = readRegular(
    resolve(source, `packages/core/src/utils/__fixtures__/${name}.json`),
  );
  if (!actual.equals(expected))
    throw new Error(`${name} vectors differ from their canonical source`);
}
for (const [path, original] of [
  [schemaPath, bytes],
  [generatorPath, generatorBytes],
  [lockPath, lockBytes],
]) {
  if (!readRegular(path).equals(original))
    throw new Error(`binding input changed during generation: ${path}`);
}
// All authored inputs and every destination are checked before any output is
// replaced. A partial filesystem write still fails the build; the receipt is
// published last and --check must succeed before consumers compile.
for (const name of [
  "stream-record-validator.ts",
  "goal-snapshot-validator.ts",
  "outbound-control-validator.ts",
]) {
  const path = resolve(source, "packages/core/src/generated", name);
  try {
    lstatSync(path);
  } catch (cause) {
    if (cause?.code === "ENOENT") continue;
    throw cause;
  }
  throw new Error(`obsolete competing binding exists: ${path}`);
}
for (const name of outputs.keys()) {
  const path = resolve(source, "packages/core/src/generated", name);
  try {
    if (!lstatSync(path).isFile())
      throw new Error(`binding output is not a regular file: ${path}`);
  } catch (cause) {
    if (cause?.code !== "ENOENT") throw cause;
  }
}
for (const [file, content] of outputs) {
  const destination = resolve(source, "packages/core/src/generated", file);
  if (arguments_[1] === "--check") {
    if (readRegular(destination).toString("utf8") !== content) {
      throw new Error(
        `${file} differs from the shared schema and pinned compiler`,
      );
    }
  } else {
    mkdirSync(dirname(destination), { recursive: true });
    try {
      if (!lstatSync(destination).isFile())
        throw new Error(`binding output is not a regular file: ${destination}`);
    } catch (cause) {
      if (cause?.code !== "ENOENT") throw cause;
    }
    writeFileSync(destination, content);
  }
}

for (const [path, original] of [
  [schemaPath, bytes],
  [generatorPath, generatorBytes],
  [lockPath, lockBytes],
]) {
  if (!readRegular(path).equals(original))
    throw new Error(`binding input changed during generation: ${path}`);
}
console.log(
  JSON.stringify({
    state: arguments_[1] === "--check" ? "verified" : "generated",
    schema_sha256: hash,
    files: [...outputs.keys()],
  }),
);
