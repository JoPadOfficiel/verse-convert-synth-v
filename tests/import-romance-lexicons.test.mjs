import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const importer = path.join(root, "scripts/import-romance-lexicons.py");
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function corpus(name) {
  const [data, reportBytes] = await Promise.all([
    readFile(path.join(root, `src-tauri/src/engine/target/${name}-community.tsv`)),
    readFile(path.join(root, `docs/${name}-lexicon-provenance.json`)),
  ]);
  const report = JSON.parse(reportBytes);
  const rows = data.toString("utf8").trimEnd().split("\n").filter((line) => !line.startsWith("#")).map((line) => line.split("\t"));
  const words = rows.map(([word]) => word);
  return { data, report, rows, words, set: new Set(words) };
}

async function pronunciationCorpus(name) {
  const report = JSON.parse(await readFile(path.join(root, `docs/${name}-lexicon-provenance.json`), "utf8"));
  const data = await readFile(path.join(root, report.pronunciation.output.path));
  const rows = data.toString("utf8").trimEnd().split("\n")
    .filter((line) => !line.startsWith("#"))
    .map((line) => line.split("\t"));
  return { data, report, rows };
}

test("Spanish and Portuguese community corpora are pinned, canonical and broad enough for routing", async () => {
  const [spanish, portuguese] = await Promise.all([corpus("spanish"), corpus("portuguese")]);
  assert.equal(spanish.report.pinned_commit, "8cfea406b505e4d7df52d5a19bce525df98c54ab");
  assert.equal(portuguese.report.pinned_commit, spanish.report.pinned_commit);
  assert.equal(spanish.report.output.path, "src-tauri/src/engine/target/spanish-community.tsv");
  assert.equal(portuguese.report.output.path, "src-tauri/src/engine/target/portuguese-community.tsv");
  for (const item of [spanish, portuguese]) {
    assert.equal(item.report.output.sha256, hash(item.data));
    assert.equal(item.report.output.bytes, item.data.length);
    assert.equal(item.data.includes(13), false);
    assert.equal(item.data.at(-1), 10);
    assert.equal(item.words.length, item.set.size, "community keys must be unique");
    assert.deepEqual(item.words, [...item.words].sort(), "community keys must be sorted");
    assert.ok(item.rows.every((row) => row.length === 2 && row[0] && row[1]), "community rows must be two-column TSV");
    for (const source of item.report.sources) {
      const license = await readFile(path.join(root, source.license_path));
      assert.equal(hash(license), source.license_sha256);
    }
  }
  assert.equal(spanish.words.length, 54833);
  assert.equal(portuguese.words.length, 326827);
  assert.ok(spanish.rows.every(([, language]) => language === "es"));
  assert.ok(portuguese.rows.every(([, community]) => ["br", "pt", "br+pt"].includes(community)));
  assert.equal(portuguese.rows.filter(([, community]) => community === "br+pt").length, portuguese.report.counts.shared_keys);
  for (const word of ["hola", "canción", "corazón", "gracia", "estamos"]) assert.ok(spanish.set.has(word), word);
  for (const word of ["você", "coração", "obrigado", "portugal", "brasil"]) assert.ok(portuguese.set.has(word), word);
  assert.equal(spanish.set.has("obrigado"), false);
  assert.equal(portuguese.set.has("hola"), false);
  assert.ok(portuguese.report.counts.brazil_keys > 300000);
  assert.ok(portuguese.report.counts.portugal_keys > 40000);
  assert.ok(portuguese.report.counts.shared_keys > 20000);
});

test("Spanish and Portuguese pronunciation corpora preserve pinned MFA dialect readings", async () => {
  const [spanish, portuguese] = await Promise.all([
    pronunciationCorpus("spanish"),
    pronunciationCorpus("portuguese"),
  ]);
  for (const item of [spanish, portuguese]) {
    const pronunciation = item.report.pronunciation;
    assert.equal(pronunciation.pinned_repository, "https://github.com/MontrealCorpusTools/mfa-models");
    assert.equal(pronunciation.pinned_commit, "d6eff86a42c6a90b641e17dfdf7a16555b934483");
    assert.equal(pronunciation.license, "CC BY 4.0");
    assert.equal(pronunciation.output.sha256, hash(item.data));
    assert.equal(pronunciation.output.bytes, item.data.length);
    assert.equal(pronunciation.output.distinct_readings, item.rows.length);
    assert.ok(item.rows.every((row) => row.length === 3 && row.every(Boolean)), "pronunciation rows must be three-column TSV");
    for (const source of pronunciation.sources) {
      assert.equal(source.rejections.length, source.counts.rejected_rows);
      assert.deepEqual(source.rejections.map((row) => row.line),
        source.rejections.map((row) => row.line).sort((a, b) => a - b));
      assert.ok(source.rejections.every((row) => Number.isInteger(row.line) && row.line > 0
        && typeof row.text === "string" && row.text.length > 0
        && ["missing_tab_separator", "non_lexical_key", "empty_pronunciation"].includes(row.reason)));
    }
    const license = await readFile(path.join(root, pronunciation.license_path));
    assert.equal(hash(license), pronunciation.license_sha256);
    const attribution = await readFile(path.join(root, pronunciation.attribution_path), "utf8");
    assert.match(attribution, /Montreal Forced Aligner/u);
  }

  const spanishRows = spanish.rows.filter(([word]) => word === "canción");
  assert.ok(spanishRows.some(([, dialect, phones]) => dialect === "spain" && phones === "k ã n θ j õ n"));
  assert.ok(spanishRows.some(([, dialect, phones]) => dialect === "latin-america" && phones === "k ã n s j õ n"));
  assert.deepEqual(
    spanish.rows.filter(([word]) => word === "hola"),
    [["hola", "spain+latin-america", "o l a"]],
  );

  const obrigado = portuguese.rows.filter(([word]) => word === "obrigado");
  assert.ok(obrigado.some(([, dialect, phones]) => dialect === "brazil" && phones === "o b ɾ i ɡ a d o"));
  assert.ok(obrigado.some(([, dialect, phones]) => dialect === "portugal" && phones === "ɔ β ɾ i ɣ a ð u"));
  assert.ok(spanish.report.pronunciation.sources.some((source) => source.dialect === "spain" && source.version === "3.3.0"));
  assert.ok(spanish.report.pronunciation.sources.some((source) => source.dialect === "latin-america" && source.version === "3.3.0"));
  assert.ok(portuguese.report.pronunciation.sources.some((source) => source.dialect === "brazil" && source.version === "2.0.0"));
  assert.ok(portuguese.report.pronunciation.sources.some((source) => source.dialect === "portugal" && source.version === "2.0.0"));
});

test("Hunspell parsing strips flags, keeps escaped spelling and rejects non-lexical rows deterministically", () => {
  const source = "5\nCanción/AB\nrock\\/roll/C\nniño\tpo:noun\n123/X\nco-op/Z\n";
  const code = [
    "import json, runpy, sys",
    "m = runpy.run_path(sys.argv[1])",
    "words, counts = m['parse_dictionary'](sys.argv[2].encode())",
    "print(json.dumps({'words': sorted(words), 'counts': counts}, ensure_ascii=False))",
  ].join("\n");
  const result = spawnSync("python3", ["-B", "-c", code, importer, source], {
    cwd: root, encoding: "utf8", env: { ...process.env, PYTHONHASHSEED: "57" }, timeout: 10000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  const parsed = JSON.parse(result.stdout);
  assert.deepEqual(parsed.words, ["canción", "co-op", "niño"]);
  assert.deepEqual(parsed.counts, { entry_rows: 5, accepted_keys: 3, rejected_rows: 2, duplicate_rows: 0 });
});

test("romance importer check mode is read-only and rejects stale artifacts", () => {
  const code = [
    "import json, pathlib, runpy, sys, tempfile",
    "m = runpy.run_path(sys.argv[1])",
    "with tempfile.TemporaryDirectory() as d:",
    " p = pathlib.Path(d) / 'asset.txt'",
    " p.write_bytes(b'stale')",
    " try: m['publish']([(p, b'expected')], True)",
    " except ValueError as e: print(json.dumps(str(e)))",
    " else: raise AssertionError('stale artifact accepted')",
    " assert p.read_bytes() == b'stale'",
  ].join("\n");
  const result = spawnSync("python3", ["-B", "-c", code, importer], { cwd: root, encoding: "utf8", timeout: 10000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.match(JSON.parse(result.stdout), /missing or stale/u);
});

test("MFA parsing preserves rejected source rows with deterministic lines and reasons", () => {
  const source = "# header\n\nOlá\to  l a\nOlá\to l a\nno separator\n123\ta\nválido\t  \n bad/key \t a\n";
  const code = [
    "import json, runpy, sys",
    "m = runpy.run_path(sys.argv[1])",
    "readings, counts, rejections = m['parse_mfa_dictionary'](sys.argv[2].encode())",
    "print(json.dumps([sorted(readings), counts, rejections], ensure_ascii=False))",
  ].join("\n");
  const outputs = ["1", "57"].map((seed) => {
    const result = spawnSync("python3", ["-B", "-c", code, importer, source], {
      cwd: root, encoding: "utf8", env: { ...process.env, PYTHONHASHSEED: seed }, timeout: 10000,
    });
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    return JSON.parse(result.stdout);
  });
  assert.deepEqual(outputs[0], outputs[1]);
  assert.deepEqual(outputs[0], [
    [["olá", "o l a"]],
    { entry_rows: 6, accepted_readings: 1, accepted_words: 1, rejected_rows: 4, duplicate_rows: 1 },
    [
      { line: 5, text: "no separator", reason: "missing_tab_separator" },
      { line: 6, text: "123\ta", reason: "non_lexical_key" },
      { line: 7, text: "válido\t  ", reason: "empty_pronunciation" },
      { line: 8, text: " bad/key \t a", reason: "non_lexical_key" },
    ],
  ]);
});

test("publication stages all outputs and rolls back an injected replacement failure", () => {
  const code = [
    "import pathlib, runpy, sys, tempfile",
    "from unittest.mock import patch",
    "m = runpy.run_path(sys.argv[1])",
    "with tempfile.TemporaryDirectory() as d:",
    " root = pathlib.Path(d)",
    " first, new, last = [root / name for name in ('first', 'new', 'last')]",
    " first.write_bytes(b'old-first'); first.chmod(0o640)",
    " last.write_bytes(b'old-last')",
    " artifacts = [(first, b'next-first'), (new, b'next-new'), (last, b'next-last')]",
    " replace = m['os'].replace",
    " calls = []",
    " def fail(source, destination):",
    "  calls.append(destination)",
    "  if len(calls) == 1:",
    "   assert first.read_bytes() == b'old-first' and last.read_bytes() == b'old-last' and not new.exists()",
    "   assert all(any(p.read_bytes() == data for p in root.iterdir()) for _, data in artifacts)",
    "  if len(calls) == 3: raise OSError('injected replacement failure')",
    "  return replace(source, destination)",
    " with patch.object(m['os'], 'replace', fail):",
    "  try: m['publish'](artifacts, False)",
    "  except OSError as e: assert str(e) == 'injected replacement failure'",
    "  else: raise AssertionError('failure did not propagate')",
    " assert first.read_bytes() == b'old-first' and last.read_bytes() == b'old-last'",
    " assert first.stat().st_mode & 0o777 == 0o640",
    " assert set(root.iterdir()) == {first, last}",
    " m['publish'](artifacts, False)",
    " assert all(p.read_bytes() == data for p, data in artifacts)",
    " assert set(root.iterdir()) == {first, new, last}",
    " m['publish'](artifacts, True)",
  ].join("\n");
  const result = spawnSync("python3", ["-B", "-c", code, importer], { cwd: root, encoding: "utf8", timeout: 10000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
});

test("staging failure leaves originals intact and cleans temporary files", () => {
  const code = [
    "import pathlib, runpy, sys, tempfile",
    "from unittest.mock import patch",
    "m = runpy.run_path(sys.argv[1])",
    "with tempfile.TemporaryDirectory() as d:",
    " root = pathlib.Path(d)",
    " first, last = root / 'first', root / 'last'",
    " first.write_bytes(b'old-first'); last.write_bytes(b'old-last')",
    " write = pathlib.Path.write_bytes",
    " def fail(path, data):",
    "  if data == b'next-last': raise OSError('injected staging failure')",
    "  return write(path, data)",
    " with patch.object(pathlib.Path, 'write_bytes', fail):",
    "  try: m['publish']([(first, b'next-first'), (last, b'next-last')], False)",
    "  except OSError as e: assert str(e) == 'injected staging failure'",
    "  else: raise AssertionError('failure did not propagate')",
    " assert first.read_bytes() == b'old-first' and last.read_bytes() == b'old-last'",
    " assert set(root.iterdir()) == {first, last}",
  ].join("\n");
  const result = spawnSync("python3", ["-B", "-c", code, importer], { cwd: root, encoding: "utf8", timeout: 10000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
});
