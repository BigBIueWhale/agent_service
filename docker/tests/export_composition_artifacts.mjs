import {
  copyFileSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";

const messages = readFileSync("/gate-compiler.jsonl", "utf8")
  .trimEnd()
  .split("\n")
  .map(JSON.parse);
const expected = ["agent_service", "docker_broker", "session_capture"];
mkdirSync("/gate-artifacts");
const artifacts = {};
const inputs = {};
function recordInput(path) {
  inputs[path] = createHash("sha256")
    .update(readFileSync(`/build/${path}`))
    .digest("hex");
}
function recordTree(path) {
  for (const entry of readdirSync(`/build/${path}`, { withFileTypes: true })) {
    const child = `${path}/${entry.name}`;
    if (entry.isDirectory()) recordTree(child);
    else if (entry.isFile()) recordInput(child);
    else throw new Error(`Unsupported compilation input ${child}`);
  }
}
for (const path of [
  "Cargo.toml",
  "Cargo.lock",
  ".dockerignore",
  "build.rs",
  "docker/Dockerfile",
  "scripts/generate-protocol-bindings.mjs",
  "config/stack.lock.json",
  "config/broker-policy-v1.json",
  "config/agent-runtime-contract-v1.json",
  "docker/config/settings.json",
])
  recordInput(path);
for (const path of ["src", "protocol", "docker/tests"]) recordTree(path);
for (const name of expected) {
  const matches = messages.filter(
    (message) =>
      message.reason === "compiler-artifact" &&
      message.target.name === name &&
      message.target.kind.includes("bin") &&
      message.profile.test &&
      message.executable,
  );
  if (matches.length !== 1)
    throw new Error(`Expected exactly one test executable for ${name}`);
  const bytes = readFileSync(matches[0].executable);
  copyFileSync(matches[0].executable, `/gate-artifacts/${name}`);
  artifacts[name] = {
    sha256: createHash("sha256").update(bytes).digest("hex"),
    bytes: bytes.length,
  };
}
writeFileSync(
  "/gate-artifacts/manifest.json",
  JSON.stringify({ v: 1, artifacts, inputs }, null, 2) + "\n",
);
