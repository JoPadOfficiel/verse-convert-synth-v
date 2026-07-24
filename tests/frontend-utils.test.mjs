import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

const source = await readFile(
  new URL("../src/lib/file-utils.ts", import.meta.url),
  "utf8",
);
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2020,
  },
}).outputText;
const utils = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`
);

test("all documented source extensions are accepted", () => {
  for (const extension of [
    "kar",
    "mid",
    "midi",
    "mxl",
    "xml",
    "musicxml",
    "mscz",
    "mscx",
  ]) {
    assert.equal(utils.isSupported(`/scores/Song.${extension}`), true);
  }
  assert.equal(utils.isSupported("/scores/Song.pdf"), false);
  assert.equal(utils.isSupported("/scores/not-midi.txt"), false);
});

test("one file-selection payload cannot create duplicate rows", () => {
  assert.deepEqual(
    utils.uniqueSupportedPaths([
      "/scores/Song.mscz",
      "/scores/Song.mscz",
      "/scores/Other.mid",
      "/scores/readme.txt",
      "/scores/Other.mid",
    ]),
    ["/scores/Song.mscz", "/scores/Other.mid"],
  );
});

test("bundle and vocal targets remain beside the source unless configured", () => {
  assert.equal(
    utils.defaultBundlePath("/scores/Song.mscz"),
    "/scores/Song.versebundle",
  );
  assert.equal(
    utils.defaultBundlePath("/scores/Song.mscz", "/exports/"),
    "/exports/Song.versebundle",
  );
  assert.equal(
    utils.defaultSvpPath("C:\\scores\\Song.mid"),
    "C:\\scores\\Song_LYRICS.svp",
  );
});

test("structured Tauri errors retain remediation and never stringify as object", () => {
  const parsed = utils.commandError({
    code: "RENDERER_NOT_FOUND",
    message: "MuseScore was not found.",
    remediation: "Configure MuseScore Studio 4.",
  });
  assert.equal(parsed.code, "RENDERER_NOT_FOUND");
  assert.equal(
    utils.commandErrorMessage(parsed),
    "MuseScore was not found. Configure MuseScore Studio 4.",
  );
  assert.equal(
    utils.commandErrorMessage('{"code":"WRITE_FAILED","message":"Disk full"}'),
    "Disk full",
  );
});

test("only renderer/audio failures mark audio unavailable", () => {
  assert.equal(
    utils.isAudioUnavailableErrorCode("RENDERER_NOT_FOUND"),
    true,
  );
  assert.equal(utils.isAudioUnavailableErrorCode("RENDERER_FAILED"), true);
  assert.equal(utils.isAudioUnavailableErrorCode("DESTINATION_EXISTS"), false);
  assert.equal(utils.isAudioUnavailableErrorCode("BUNDLE_COMMIT_FAILED"), false);
});
