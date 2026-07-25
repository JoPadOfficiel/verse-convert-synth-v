import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

const source = await readFile(
  new URL("../src/lib/vocal-overrides.ts", import.meta.url),
  "utf8",
);
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2020,
  },
}).outputText;
const { applyTrackOverrides } = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`
);

test("one Part action applies every vocal candidate override atomically", () => {
  const previous = { 1: false, 9: true };
  const next = applyTrackOverrides(previous, [1, 3, 5], true);

  assert.deepEqual(next, { 1: true, 3: true, 5: true, 9: true });
  assert.deepEqual(previous, { 1: false, 9: true });
});

test("FileList forwards all Part track IDs through one callback", async () => {
  const fileList = await readFile(
    new URL("../src/components/FileList.tsx", import.meta.url),
    "utf8",
  );

  assert.match(
    fileList,
    /onToggleVocal\(item\.path,\s*trackIds,\s*enabled\)/,
  );
  assert.doesNotMatch(fileList, /trackIds\.forEach/);
});
