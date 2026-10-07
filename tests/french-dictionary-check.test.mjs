import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../scripts/check-french-dictionary.py", import.meta.url));
const run = (...args) => spawnSync("python3", ["-B", script, ...args], { encoding: "utf8", timeout: 30000 });
function report(...args) {
  const result = run(...args);
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}

test("French dictionary inspector distinguishes spelling, attested phones and unknown words", () => {
  const result = report("yeux", "jeu", "jeux", "clairs", "mot-zyx-inconnu");
  assert.equal(result.mode, "dictionary-evidence-only");
  assert.match(result.corpora.community.sha256, /^[a-f0-9]{64}$/);
  assert.ok(result.corpora.community.headers.some((line) => line.includes("Source SHA-256:")));
  assert.match(result.limitations, /liaison and acoustic quality are separate/);
  const words = new Map(result.words.map((word) => [word.key, word]));
  assert.deepEqual(words.get("yeux").community.phones, ["fr/y", "fr/ee"]);
  assert.deepEqual(words.get("jeu").community.phones, ["fr/j", "fr/ee"]);
  assert.deepEqual(words.get("jeux").community.phones, ["fr/j", "fr/ee"]);
  assert.deepEqual(words.get("yeux").community.sourceSymbols, ["yy eu"]);
  assert.deepEqual(words.get("yeux").community.sourceLines, [101730]);
  assert.equal(words.get("yeux").dictionaryAgreement, null);
  assert.equal(words.get("mot-zyx-inconnu").community, null);
  assert.equal(words.get("mot-zyx-inconnu").curated, null);
});

test("native dictionary comparison detects a disagreeing reading without editing the pack", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "verse-french-dictionary-"));
  const pack = path.join(root, "g2p.zip");
  try {
    const created = spawnSync("python3", ["-B", "-c", "import zipfile,sys\nwith zipfile.ZipFile(sys.argv[1], 'x') as z:\n z.writestr('dict.txt', 'yeux  y ee\\nyeux  y oe\\njeux  j ee\\njeu  j oe\\n')\n z.writestr('phones.txt', 'y glide\\nee vowel\\noe vowel\\nj consonant\\n')", pack], { encoding: "utf8", timeout: 10000 });
    assert.equal(created.status, 0, created.stderr);
    const before = await readFile(pack);
    const result = report("--millefeuille-pack", pack, "YEUX", "jeu", "clairs", "jeux");
    assert.deepEqual(result.words[0].nativeReadings, [["fr/y", "fr/ee"], ["fr/y", "fr/oe"]]);
    assert.equal(result.words[0].dictionaryAgreement, false);
    assert.equal(result.words[0].nativeAmbiguous, true);
    assert.deepEqual(result.words[0].nativeConsumerReading, ["fr/y", "fr/oe"]);
    assert.equal(result.words[0].nativeRows[1].line, 2);
    assert.equal(result.words[1].dictionaryAgreement, false);
    assert.deepEqual(result.words[2].nativeReadings, []);
    assert.equal(result.words[2].dictionaryAgreement, null);
    assert.equal(result.words[3].dictionaryAgreement, true);
    assert.equal(result.words[3].nativeAmbiguous, false);
    assert.match(result.nativePackSha256, /^[a-f0-9]{64}$/);
    assert.deepEqual(await readFile(pack), before);
    const invalid = run("--millefeuille-pack", path.join(root, "missing.zip"), "yeux");
    assert.equal(invalid.status, 1);
    assert.ok(JSON.parse(invalid.stderr).error);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("native inspection rejects invented separators and exposes inventory-filtered symbols", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "verse-native-format-"));
  const pack = path.join(root, "g2p.zip");
  try {
    const created = spawnSync("python3", ["-B", "-c", "import zipfile,sys\nwith zipfile.ZipFile(sys.argv[1], 'x') as z:\n z.writestr('dict.txt', 'yeux\\ty ee\\nyeux    y ee\\njeu  j ee zz-not-supported\\n')\n z.writestr('phones.txt', 'y glide\\nee vowel\\nj consonant\\n')", pack], { encoding: "utf8", timeout: 10000 });
    assert.equal(created.status, 0, created.stderr);
    const result = report("--millefeuille-pack", pack, "yeux", "jeu");
    assert.deepEqual(result.words[0].nativeReadings, []);
    assert.equal(result.nativePack.ignoredRowCount, 2);
    assert.deepEqual(result.words[1].nativeConsumerReading, ["fr/j", "fr/ee"]);
    assert.deepEqual(result.words[1].nativeRows[0].invalidSymbols, ["zz-not-supported"]);
    assert.equal(result.words[1].dictionaryAgreement, false);
  } finally { await rm(root, { recursive: true, force: true }); }
});
