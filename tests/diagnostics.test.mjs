import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

const source = await readFile(
  new URL("../src/lib/diagnostics.ts", import.meta.url),
  "utf8",
);
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2020,
  },
}).outputText;
const { groupDiagnostics } = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`
);

const reinterpreted = (sourceId) => ({
  code: "LYRIC_REINTERPRETED_BY_TARGET",
  severity: "warning",
  message: 'OpenUtau will read the source lyric "+plus" as a continuation.',
  sourceId,
});

test("one diagnostic repeated per note is displayed once with its count", () => {
  const grouped = groupDiagnostics([
    reinterpreted("note:melody:1"),
    reinterpreted("note:melody:2"),
    reinterpreted("note:melody:3"),
  ]);

  assert.equal(grouped.length, 1);
  assert.equal(grouped[0].count, 3);
  assert.equal(grouped[0].code, "LYRIC_REINTERPRETED_BY_TARGET");
  assert.equal(grouped[0].severity, "warning");
  assert.equal(grouped[0].message, reinterpreted("x").message);
});

test("grouping never merges two different claims or loses one", () => {
  const other = {
    code: "LYRIC_REINTERPRETED_BY_TARGET",
    severity: "warning",
    message: "OpenUtau will read the bracketed part as a phonetic hint.",
    sourceId: "note:melody:9",
  };
  const info = {
    code: "LYRIC_VERSES_EXCEED_REPEAT_PASSES",
    severity: "info",
    message: "The source stacks more verses than the repeat plays back.",
    sourceId: "melody",
  };

  const grouped = groupDiagnostics([
    reinterpreted("note:melody:1"),
    info,
    other,
    reinterpreted("note:melody:2"),
  ]);

  // Same code, different message: two rows, never one.
  assert.deepEqual(
    grouped.map((warning) => [warning.code, warning.count]),
    [
      ["LYRIC_REINTERPRETED_BY_TARGET", 2],
      ["LYRIC_VERSES_EXCEED_REPEAT_PASSES", 1],
      ["LYRIC_REINTERPRETED_BY_TARGET", 1],
    ],
  );
  // First-appearance order is the backend's deterministic order.
  assert.equal(grouped[1].severity, "info");
  assert.equal(grouped[2].message, other.message);
});

test("an empty diagnostic list stays empty", () => {
  assert.deepEqual(groupDiagnostics([]), []);
});
