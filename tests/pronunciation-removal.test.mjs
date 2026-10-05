import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import test from "node:test";
import ts from "typescript";

const root = fileURLToPath(new URL("../", import.meta.url));
const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

test("nonignored product and build surfaces contain no rejected candidate integration", () => {
  const removed = [
    "src-tauri/src/pronunciation/laya.rs", "src-tauri/src/pronunciation/assets.rs",
    "scripts/prepare-laya-assets.py", "scripts/evaluate-pronunciation.py",
    "src-tauri/examples/pronunciation_evaluation.rs", "tests/pronunciation-evaluation.test.mjs",
    "docs/laya-assets-provenance.json", "src-tauri/resources/pronunciation/README.md",
  ];
  for (const path of removed) assert.equal(existsSync(new URL(`../${path}`, import.meta.url)), false, path);
  const sources = execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], { cwd: root }).toString().split("\0");
  const rootManifests = /^(?:[^/]+\.(?:json|toml|lock|[cm]?[jt]sx?|ya?ml)|(?:Cargo|bun|yarn|pnpm)[^/]*|[^/]*config[^/]*|rust-toolchain[^/]*|Makefile|Justfile)$/i;
  const integration = /\blaya\b|LAYA_|layaReason|laya_reason|laya-native|prepare-laya|evaluate-pronunciation|pronunciation_evaluation/i;
  for (const path of new Set(sources)) {
    if (!path || removed.includes(path) || !existsSync(new URL(`../${path}`, import.meta.url))) continue;
    if (!rootManifests.test(path) && !/^(src\/|src-tauri\/|scripts\/|\.github\/)/.test(path)) continue;
    // Dictionary words (including Spanish "laya") are source data, not integration.
    if (!rootManifests.test(path) && !/\.(rs|toml|lock|json|[cm]?[jt]sx?|py|sh|ya?ml)$/.test(path)) continue;
    assert.doesNotMatch(read(path), integration, path);
  }
  const cargo = read("src-tauri/Cargo.toml") + read("src-tauri/Cargo.lock");
  assert.doesNotMatch(cargo, /(?:name\s*=\s*"(?:ort|ort-sys|tokenizers)"|^\s*(?:ort|tokenizers)\s*=)/m);
  const resources = JSON.parse(read("src-tauri/tauri.conf.json")).bundle.resources;
  const resourcePaths = Array.isArray(resources) ? resources : Object.entries(resources ?? {}).flat();
  for (const path of resourcePaths) assert.doesNotMatch(path, /(?:^|[\\/])(?:laya(?:[\\/]|$)|pronunciation[\\/]laya(?:[\\/]|$))/i);
});

test("retained correction memory DTO agrees between Rust and frontend", () => {
  const rust = read("src-tauri/src/pronunciation/commands.rs");
  const body = rust.match(/pub struct MemoryView\s*\{([^}]+)\}/)?.[1];
  assert.ok(body);
  const rustFields = [...body.matchAll(/pub\s+(\w+)\s*:/g)].map((match) => match[1]);
  const source = ts.createSourceFile("tauri.ts", read("src/lib/tauri.ts"), ts.ScriptTarget.Latest, true);
  const dto = source.statements.find((node) => ts.isTypeAliasDeclaration(node) && node.name.text === "PronunciationMemory");
  assert.ok(dto && ts.isTypeLiteralNode(dto.type));
  const frontendFields = dto.type.members.map((node) => node.name.getText(source));
  assert.deepEqual(rustFields.toSorted(), ["baseline", "history", "references"]);
  assert.deepEqual(frontendFields.toSorted(), rustFields.toSorted());
  assert.doesNotMatch(read("src/components/ImportedCorrectionsReview.tsx"), /\blaya\b|availability/i);
});
