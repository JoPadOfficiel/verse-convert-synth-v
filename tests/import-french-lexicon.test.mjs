import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const importer = path.join(root, "scripts/import-french-lexicon.py");
const dataPath = path.join(root, "src-tauri/src/engine/target/french-community.tsv");
const reportPath = path.join(root, "docs/french-lexicon-provenance.json");
const sourceHash = "b4560adb8e5e2145f7b8a25f810db4c2f798d7db518f1d814d0dbae9728f7d34";
const licenseHash = "11e3c35cb4439d0086b5cc9309b31ab00185a7b10bb1f584e042769d0d78302c";
// Pin the independently audited OpenUtau inventory, rather than accepting
// whatever phone list a future importer happens to write into its report.
const allowed = new Set("ah eh ae ee oe ih oh oo ou uh en in on uy y w f k p s sh t h b d g l m n r v z j ng q".split(" ").map((p) => `fr/${p}`));
const vowels = new Set("ah eh ae ee oe ih oh oo ou uh en in on".split(" ").map((p) => `fr/${p}`));
const [data, reportBytes] = await Promise.all([readFile(dataPath), readFile(reportPath)]);
const report = JSON.parse(reportBytes);
const entries = data.toString("utf8").trimEnd().split("\n").filter((line) => !line.startsWith("#")).map((line) => line.split("\t"));
const dictionary = new Map(entries.map(([key, phones, lines, symbols]) => [key, { phones, lines, symbols }]));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const canonical = (key) => key.toLowerCase().replace(/[‘’]/gu, "'").normalize("NFC");
function run(args, extraEnv = {}) {
  const result = spawnSync("python3", ["-B", importer, ...args], {
    cwd: root, encoding: "utf8", env: { ...process.env, ...extraEnv }, timeout: 30000,
  });
  assert.ifError(result.error);
  return result;
}
function succeeds(args, extraEnv) {
  const result = run(args, extraEnv);
  assert.equal(result.status, 0, result.stderr || result.stdout);
}

test("the real generated corpus has unique canonical keys, allowed vowels/phones and complete provenance", async () => {
  assert.equal(report.source.sha256, sourceHash);
  assert.equal(hash(await readFile(path.join(root, report.license.path))), licenseHash);
  assert.equal(report.license.sha256, licenseHash);
  assert.equal(report.output.sha256, hash(data));
  assert.equal(report.output.bytes, data.length);
  assert.equal(data.includes(13), false, "generated data uses LF endings");
  assert.equal(data.at(-1), 10);
  assert.deepEqual(new Set(report.allowed_phones), allowed);
  assert.deepEqual(new Set(report.vowel_phones), vowels);
  assert.equal(entries.length, 104943, "pinned corpus coverage must not silently shrink");
  assert.equal(dictionary.size, entries.length);
  assert.equal(report.counts.accepted_keys, entries.length);
  const sourceLines = new Set(report.comment_lines);
  const countLine = (line) => {
    assert.ok(Number.isSafeInteger(line) && line > 0 && line <= report.counts.source_lines);
    assert.equal(sourceLines.has(line), false, `source line ${line} accounted twice`);
    sourceLines.add(line);
  };
  let acceptedRows = 0;
  let previous;
  for (const [key, phones, lines, symbols, ...extra] of entries) {
    assert.equal(extra.length, 0, key);
    assert.equal(canonical(key), key, `noncanonical key ${key}`);
    const bytes = Buffer.from(key);
    if (previous) assert.ok(Buffer.compare(previous, bytes) < 0, `unsorted key ${key}`);
    previous = bytes;
    const tokens = phones.split(" ");
    assert.ok(tokens.every((phone) => allowed.has(phone)), `unsupported phones for ${key}`);
    assert.ok(tokens.some((phone) => vowels.has(phone)), `no sung vowel for ${key}`);
    const provenanceLines = lines.split(",").map(Number);
    const originalReadings = symbols.split("|");
    assert.equal(provenanceLines.length, originalReadings.length, key);
    assert.deepEqual(provenanceLines, [...provenanceLines].sort((a, b) => a - b));
    for (const reading of originalReadings) {
      const translated = reading.split(" ").flatMap((symbol) => {
        assert.ok(Object.hasOwn(report.mapping, symbol), `unmapped ${symbol} for ${key}`);
        return report.mapping[symbol];
      });
      assert.equal(translated.join(" "), phones, key);
    }
    provenanceLines.forEach(countLine);
    acceptedRows += provenanceLines.length;
  }
  for (const row of report.excluded_rows) {
    countLine(row.line);
    assert.ok(row.reason, `missing exclusion reason on ${row.line}`);
    assert.equal(dictionary.has(row.key), false, `excluded alternative survived as ${row.key}`);
  }
  assert.equal(acceptedRows, report.counts.accepted_source_rows);
  assert.equal(acceptedRows - entries.length, report.counts.deduplicated_extra_rows);
  assert.equal(report.excluded_rows.length, report.counts.excluded_rows);
  assert.equal(sourceLines.size, report.counts.source_lines);
  assert.equal(report.quarantines.length, report.counts.quarantined_keys);
  const reasons = Object.create(null);
  for (const row of report.excluded_rows) reasons[row.reason] = (reasons[row.reason] ?? 0) + 1;
  assert.deepEqual({ ...reasons }, report.counts.excluded_rows_by_reason);
  assert.doesNotMatch(reportBytes.toString("utf8"), /\/Users\/|\/private\/|\/tmp\//u);
});

test("real upstream variants, gn expansion, duplicate readings and invalid alternatives stay explicit", () => {
  for (const [key, expected] of [
    ["gagner", "fr/g fr/ah fr/n fr/y fr/eh"],
    ["montagne", "fr/m fr/on fr/t fr/ah fr/n fr/y"],
    ["ligne", "fr/l fr/ih fr/n fr/y"],
    ["un", "fr/in"],
    ["rêves", "fr/r fr/ae fr/v"],
    ["rêves(2)", "fr/r fr/ae fr/v fr/ee"],
    ["a priori", "fr/ah fr/p fr/r fr/ih fr/y fr/oo fr/r fr/ih"],
  ]) assert.equal(dictionary.get(key)?.phones, expected, key);
  assert.equal(dictionary.get("érignac").lines, "102122,105104");
  assert.equal(dictionary.get("érignac").symbols, "ei rr ii nn yy aa kk|ei rr ii gn aa kk");
  assert.equal(dictionary.get("l'équipe").lines, "54237,105114");
  for (const key of ["aires(3)", "hôtel", "e", "u", "che", "bye"]) {
    assert.equal(dictionary.has(key), false, key);
    const group = report.quarantines.find((entry) => entry.key === key);
    assert.ok(group && group.rows.length === 2, `${key} must preserve both alternatives`);
  }
  assert.ok(report.quarantines.find((entry) => entry.key === "e").reasons.includes("invalid_alternative"));
  assert.ok(report.quarantines.find((entry) => entry.key === "u").rows.some((row) => row.reason === "empty_reading"));
  assert.equal(report.counts.gn_accepted_source_rows, 1043);
  assert.deepEqual(report.mapping.gn, ["fr/n", "fr/y"]);
  for (const symbol of ["4", "ih", "m", "u", "c", "zzvérita", "-"]) {
    assert.equal(Object.hasOwn(report.mapping, symbol), false, symbol);
  }
});

test("a wrong source checksum is rejected before generation or check can change artifacts", async (t) => {
  const temp = await mkdtemp(path.join(tmpdir(), "verse-fr-import-hash-"));
  t.after(() => rm(temp, { recursive: true, force: true }));
  const source = path.join(temp, "cmudict_fr.txt");
  const output = path.join(temp, "data.tsv");
  const provenance = path.join(temp, "report.json");
  await Promise.all([
    writeFile(source, "not the pinned dictionary\n"),
    writeFile(output, "existing output\n"),
    writeFile(provenance, "existing report\n"),
  ]);
  for (const mode of [[], ["--check"]]) {
    const result = run(["--source", source, "--output", output, "--provenance", provenance, ...mode]);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /source SHA-256 mismatch/u);
    assert.equal(await readFile(output, "utf8"), "existing output\n");
    assert.equal(await readFile(provenance, "utf8"), "existing report\n");
  }
});

// Offline full-corpus check: supply the local pinned download explicitly. Normal
// frontend checks still exercise the entire committed dataset and hash refusal.
const source = process.env.FRENCH_LEXICON_SOURCE;
test("the pinned source reproduces both artifacts and check detects either stale artifact without writing", {
  skip: source ? false : "Set FRENCH_LEXICON_SOURCE to the local pinned cmudict_fr.txt for full regeneration",
}, async (t) => {
  const original = await readFile(source);
  assert.equal(hash(original), sourceHash);
  const sourceLines = original.toString("utf8").trimEnd().split(/\r\n|\n|\r/u);
  for (const [key, , lines, symbols] of entries) {
    const originalReadings = symbols.split("|");
    lines.split(",").forEach((number, index) => {
      const match = /^(.*?) {2,}(.*)$/u.exec(sourceLines[Number(number) - 1]);
      assert.ok(match, `${number}: expected an upstream dictionary row`);
      assert.equal(canonical(match[1]), key);
      assert.equal(match[2].trim().split(/\s+/u).join(" "), originalReadings[index]);
    });
  }
  const before = await Promise.all([stat(dataPath), stat(reportPath)]);
  succeeds(["--source", source, "--check"]);
  const after = await Promise.all([stat(dataPath), stat(reportPath)]);
  assert.deepEqual(after.map((s) => s.mtimeMs), before.map((s) => s.mtimeMs));
  const temp = await mkdtemp(path.join(tmpdir(), "verse-fr-import-reproduce-"));
  t.after(() => rm(temp, { recursive: true, force: true }));
  const output = path.join(temp, "data.tsv");
  const provenance = path.join(temp, "report.json");
  const args = ["--source", source, "--output", output, "--provenance", provenance];
  for (const seed of ["1", "23"]) {
    succeeds(args, { PYTHONHASHSEED: seed });
    assert.deepEqual(await readFile(output), data);
    assert.deepEqual(await readFile(provenance), reportBytes);
  }
  succeeds([...args, "--check"]);
  await writeFile(output, Buffer.concat([data, Buffer.from("\n")]));
  let result = run([...args, "--check"]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /missing or stale/u);
  assert.equal((await readFile(output)).length, data.length + 1, "check must not repair data");
  await writeFile(output, data);
  await writeFile(provenance, "{}\n");
  result = run([...args, "--check"]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /missing or stale/u);
  assert.equal(await readFile(provenance, "utf8"), "{}\n", "check must not repair report");
});
