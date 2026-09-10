import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const importer = resolve(dirname(fileURLToPath(import.meta.url)), "../scripts/import-english-lexicon.py");

// Pure transformations use small fixtures. The production CLI has no hash
// override and still requires the exact upstream file; no fixture can import.
function python(code, payload, seed = "1") {
  const result = spawnSync("rtk", ["proxy", "python3", "-B", "-c", [
    "import json, pathlib, runpy, sys",
    "m = runpy.run_path(sys.argv[1])",
    "payload = json.loads(sys.argv[2])",
    code,
  ].join("\n"), importer, JSON.stringify(payload)], {
    encoding: "utf8",
    env: { ...process.env, PYTHONHASHSEED: seed, PYTHONDONTWRITEBYTECODE: "1" },
    timeout: 10_000,
    maxBuffer: 1024 * 1024,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}

function generate(source, seed) {
  return python([
    "tsv, report, counts = m['generate'](payload.encode('utf-8'))",
    "print(json.dumps({'tsv': tsv.decode(), 'report': report.decode(), 'counts': counts}))",
  ].join("\n"), source, seed);
}

function entries(tsv) {
  return new Map(tsv.trimEnd().split("\n").filter((line) => !line.startsWith("#"))
    .map((line) => {
      const [key, phones, lines, symbols] = line.split("\t");
      return [key, { phones, lines, symbols }];
    }));
}

test("CMU normalization preserves variant keys and original readings while merging equal canonical readings", () => {
  const source = [
    "She's SH IY1 Z",
    "she’s SH IY0 Z",
    "read R EH1 D",
    "read(2) R IY1 D",
    "read(3) R IY0 D",
    "ROOM2(2) R UW1 M",
    "Cafe\u0301 K AE1 F EY0",
    "CAFÉ K AE0 F EY1",
    "'BOUT B AW1 T",
  ].join("\n") + "\n";
  const result = generate(source);
  const rows = entries(result.tsv);
  assert.deepEqual(rows.get("she's"), {
    phones: "en/sh en/iy en/z", lines: "1,2", symbols: "SH IY1 Z|SH IY0 Z",
  });
  assert.equal(rows.get("read").phones, "en/r en/eh en/d");
  assert.equal(rows.get("read(2)").phones, "en/r en/iy en/d");
  assert.equal(rows.get("read(3)").phones, rows.get("read(2)").phones);
  assert.equal(rows.get("room2(2)").phones, "en/r en/uw en/m");
  assert.equal(rows.get("café").lines, "7,8");
  assert.ok(rows.has("'bout"));
  assert.equal(result.counts.accepted_keys, 7);
  assert.equal(result.counts.accepted_numbered_variant_keys, 3);
  assert.equal(result.counts.deduplicated_extra_rows, 2);
  const provenance = JSON.parse(result.report);
  assert.equal(provenance.deduplications.find((group) => group.key === "she's").rows[1].raw, "she’s SH IY0 Z");
  assert.equal(provenance.normalizations.find((row) => row.line === 7).word, "Cafe\u0301");
});

test("conflicts and invalid alternatives quarantine complete canonical groups without repairing phones", () => {
  const source = [
    "# comment", "", "Debt D EH1 T", "debt D EH1 B",
    "Safe S EY1 F", "safe S EY1 F3", "wrong W AH9 NG",
    "stress B1 IY0", "empty", "bad\u0000key B AE1 D", "strange ſ",
    "valid V AE1 L IH0 D # retain original comment", "shh SH", ";;; comment",
  ].join("\n") + "\n";
  const result = generate(source);
  const rows = entries(result.tsv);
  assert.deepEqual([...rows.keys()], ["shh", "valid"]);
  assert.equal(result.counts.quarantined_keys, 7);
  assert.equal(result.counts.quarantined_source_rows, 9);
  assert.equal(result.counts.comment_lines, 2);
  assert.equal(result.counts.blank_lines, 1);
  const report = JSON.parse(result.report);
  assert.deepEqual(report.keys_without_vowel, ["shh"]);
  const debt = report.quarantines.find((group) => group.key === "debt");
  assert.deepEqual(debt.reasons, ["conflicting_readings"]);
  assert.deepEqual(debt.rows.map((row) => row.raw), ["Debt D EH1 T", "debt D EH1 B"]);
  const safe = report.quarantines.find((group) => group.key === "safe");
  assert.deepEqual(safe.reasons, ["invalid_alternative"]);
  assert.equal(safe.rows.length, 2);
  assert.deepEqual(safe.rows[1].unsupported_phonemes, ["F3"]);
  assert.deepEqual(report.quarantines.find((group) => group.key === "stress").rows[0].reasons, ["invalid_stress"]);
  assert.equal(result.counts.entry_rows, result.counts.accepted_source_rows + result.counts.quarantined_source_rows);
});

test("TSV and provenance bytes are reproducible across Python hash seeds", () => {
  const source = "Zed Z EH1 D\nzed Z EH0 D\nfire F AY1 ER0\nfire(2) F AY1 R\nread R EH1 D\nREAD R IY1 D\n";
  const first = generate(source, "1");
  const second = generate(source, "187");
  assert.equal(first.tsv, second.tsv);
  assert.equal(first.report, second.report);
  assert.ok(first.tsv.endsWith("\n") && first.report.endsWith("\n"));
  assert.ok(!first.tsv.includes("\r"));
  assert.equal(JSON.parse(first.report).allowed_phones.length, 39);
});

test("check reports missing or stale artifacts without writing or repairing them", () => {
  const temp = mkdtempSync(join(tmpdir(), "verse-cmu-check-"));
  try {
    const result = python(`
root = pathlib.Path(payload)
a, b = root / 'a.tsv', root / 'b.json'
a.write_bytes(b'correct')
before = a.stat().st_mtime_ns
artifacts = [(a, b'correct'), (b, b'expected')]
errors = []
try:
    m['publish'](artifacts, True)
except ValueError as error:
    errors.append(str(error))
assert not b.exists() and a.stat().st_mtime_ns == before
b.write_bytes(b'stale')
try:
    m['publish'](artifacts, True)
except ValueError as error:
    errors.append(str(error))
assert b.read_bytes() == b'stale' and a.stat().st_mtime_ns == before
b.write_bytes(b'expected')
b_before = b.stat().st_mtime_ns
m['publish'](artifacts, True)
assert b.stat().st_mtime_ns == b_before and a.stat().st_mtime_ns == before
print(json.dumps(errors))`, temp);
    assert.equal(result.length, 2);
    assert.ok(result.every((message) => message.includes("missing or stale")));
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

test("CLI rejects an unpinned source before generating anything, including in check mode", () => {
  const temp = mkdtempSync(join(tmpdir(), "verse-cmu-hash-"));
  try {
    mkdirSync(join(temp, "scripts"));
    const copy = join(temp, "scripts/import-english-lexicon.py");
    copyFileSync(importer, copy);
    const source = join(temp, "cmudict.dict");
    writeFileSync(source, "she's SH IY1 Z\n");
    for (const mode of [[], ["--check"]]) {
      const result = spawnSync("rtk", ["proxy", "python3", "-B", copy, "--source", source, ...mode], {
        encoding: "utf8", timeout: 10_000,
      });
      assert.ifError(result.error);
      assert.equal(result.status, 1);
      assert.match(result.stderr, /source SHA-256 mismatch/);
      assert.ok(!existsSync(join(temp, "src-tauri")));
      assert.ok(!existsSync(join(temp, "docs")));
    }
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

test("the distributed CMU license is byte-exact and a changed license is rejected", () => {
  const result = python(`
data = m['checked_bytes'](m['ROOT'] / m['LICENSE_PATH'], m['LICENSE_SHA256'], 'license')
import tempfile
with tempfile.TemporaryDirectory(prefix='verse-cmu-license-') as root:
    path = pathlib.Path(root) / 'LICENSE'
    path.write_bytes(data + b'\\n')
    try:
        m['checked_bytes'](path, m['LICENSE_SHA256'], 'license')
    except ValueError as error:
        print(json.dumps(str(error)))
    else:
        raise AssertionError('changed license accepted')`, null);
  assert.match(result, /license SHA-256 mismatch/);
});
