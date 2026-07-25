import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

const source = await readFile(
  new URL("../src/lib/export-progress.ts", import.meta.url),
  "utf8",
);
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2020,
  },
}).outputText;
const progress = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`
);

test("per-title export progress is bounded and reaches one hundred percent", () => {
  assert.equal(
    progress.exportProgressPercent({
      phase: "renderingStem",
      completed: 4,
      total: 8,
    }),
    50,
  );
  assert.equal(
    progress.exportProgressPercent({
      phase: "renderingStem",
      completed: 99,
      total: 8,
    }),
    100,
  );
  assert.equal(
    progress.exportProgressPercent({
      phase: "finished",
      completed: 0,
      total: 0,
    }),
    100,
  );
});

test("queued and failed titles retain useful progress context", () => {
  assert.deepEqual(progress.queuedExportProgress(), {
    phase: "queued",
    completed: 0,
    total: 1,
    message: "Waiting for the previous title",
    stemId: null,
    stemName: null,
  });
  assert.deepEqual(
    progress.failedExportProgress({
      phase: "renderingStem",
      completed: 6,
      total: 10,
      message: "Rendering",
      stemId: "part-006",
      stemName: "Piano",
    }),
    {
      phase: "failed",
      completed: 6,
      total: 10,
      message: "Complete project failed",
      stemId: "part-006",
      stemName: "Piano",
    },
  );
});
