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
    utils.defaultVocalPath("C:\\scores\\Song.mid"),
    "C:\\scores\\Song_LYRICS.svp",
  );
});

test("the vocal target extension follows the export target and nothing else", () => {
  // Only the extension moves: the `_LYRICS` stem, the source directory and the
  // Windows separator behaviour are unchanged.
  assert.equal(
    utils.defaultVocalPath("/scores/Song.mid", "svp"),
    "/scores/Song_LYRICS.svp",
  );
  assert.equal(
    utils.defaultVocalPath("/scores/Song.mid", "ustx"),
    "/scores/Song_LYRICS.ustx",
  );
  assert.equal(
    utils.defaultVocalPath("C:\\scores\\Song.mid", "ustx"),
    "C:\\scores\\Song_LYRICS.ustx",
  );
  // A caller that names no target keeps release 0.4.9's filename exactly.
  assert.equal(
    utils.defaultVocalPath("/scores/Song.mxl"),
    utils.defaultVocalPath("/scores/Song.mxl", "svp"),
  );
});

test("batch bundle targets disambiguate equal stems by source extension", () => {
  const paths = utils.batchBundlePaths(
    ["/scores/Help.mscz", "/scores/Help.mxl", "/scores/Other.kar"],
    "/exports",
  );
  assert.equal(paths.get("/scores/Help.mscz"), "/exports/Help.mscz.versebundle");
  assert.equal(paths.get("/scores/Help.mxl"), "/exports/Help.mxl.versebundle");
  assert.equal(paths.get("/scores/Other.kar"), "/exports/Other.versebundle");
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

// The two fidelity tiers shown in the drop target must together be exactly the
// extensions the file dialog accepts, or a supported format becomes invisible.
test("the fidelity tiers cover every supported extension exactly once", () => {
  const tiered = [...utils.SCORE_EXTENSIONS, ...utils.MIDI_EXTENSIONS];
  assert.equal(
    new Set(tiered).size,
    tiered.length,
    "no extension may appear in both tiers",
  );
  assert.deepEqual([...tiered].sort(), [...utils.SUPPORTED_EXTENSIONS].sort());
});

// A score states which note owns a syllable; a MIDI file does not. Listing a
// MIDI extension as a score would tell the user the opposite.
test("the score tier holds the notated formats and the MIDI tier the rest", () => {
  assert.deepEqual([...utils.SCORE_EXTENSIONS], ["mxl", "musicxml", "xml", "mscz", "mscx"]);
  assert.deepEqual([...utils.MIDI_EXTENSIONS], ["kar", "mid", "midi"]);
});
