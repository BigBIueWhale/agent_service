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
// The executables were compiled in /build, which holds the build inputs and
// what cargo wrote under target/; every other file is an input, recorded here
// so the host can compare the whole tree with its sealed manifest.
function recordTree(prefix) {
  for (const entry of readdirSync(`/build/${prefix}`, { withFileTypes: true })) {
    const child = `${prefix}${entry.name}`;
    if (entry.isDirectory()) {
      if (child !== "target") recordTree(`${child}/`);
    } else if (entry.isFile()) recordInput(child);
    else throw new Error(`Unsupported compilation input ${child}`);
  }
}
recordTree("");
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
