#!/usr/bin/env python3
"""Inspect complete French dictionary keys offline, without changing music.

Example: python3 scripts/check-french-dictionary.py yeux jeu clairs
Add --millefeuille-pack /local/g2p-fr-millefeuille.zip to compare native evidence.
Dictionary agreement does not prove liaison, syllable allocation or audio quality.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
MAX_BYTES = 32 * 1024 * 1024
MAX_WORDS = 128
spec = importlib.util.spec_from_file_location("french_importer", ROOT / "scripts/import-french-lexicon.py")
importer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(importer)


def read_bounded(path):
    with path.open("rb") as stream:
        data = stream.read(MAX_BYTES + 1)
    if len(data) > MAX_BYTES:
        raise ValueError(f"Dictionary exceeds {MAX_BYTES} bytes: {path}")
    return data


def corpus(path):
    data = read_bounded(path)
    result = {}
    headers = []
    for number, line in enumerate(data.decode("utf-8-sig").splitlines(), 1):
        if not line or line.startswith("#"):
            if line.startswith("#"):
                headers.append(line)
            continue
        key, phones, lines, symbols = line.split("\t")
        if key in result:
            raise ValueError(f"Duplicate dictionary key: {key}")
        result[key] = {"phones": phones.split(), "sourceLines": [int(n) for n in lines.split(",")], "sourceSymbols": symbols.split("|"), "tsvLine": number}
    return result, {"path": str(path.relative_to(ROOT)), "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data), "headers": headers}


def native_dictionary(path):
    # Read the dictionary only. Never load the pack's ONNX graph.
    archive_bytes = read_bounded(path)
    import io
    with zipfile.ZipFile(io.BytesIO(archive_bytes)) as archive:
        def member(name):
            entry = archive.getinfo(name)
            if entry.file_size > MAX_BYTES:
                raise ValueError("Native dictionary member exceeds the size limit")
            with archive.open(entry) as stream:
                data = stream.read(MAX_BYTES + 1)
            if len(data) > MAX_BYTES:
                raise ValueError("Native dictionary member exceeds the size limit")
            return data
        data, inventory_data = member("dict.txt"), member("phones.txt")
    # Match the pinned G2pPack/Builder parser. Unknown phones are filtered by
    # its inventory, duplicate keys overwrite the previous native reading.
    inventory = set()
    for line in inventory_data.decode("utf-8-sig").splitlines():
        fields = re.split(r"\s", line.strip())
        if len(fields) == 2:
            inventory.add(fields[0])
    result = {}
    records, effective, counts = {}, {}, {}
    ignored, ignored_count = [], 0
    for number, line in enumerate(data.decode("utf-8-sig").splitlines(), 1):
        if line.startswith(";;;"):
            continue
        fields = line.strip().split("  ")
        if len(fields) != 2 or not fields[0]:
            ignored_count += 1
            if len(ignored) < 8:
                ignored.append({"line": number, "raw": line, "reason": "not-a-native-two-field-entry"})
            continue
        key = fields[0]
        phones = re.split(r"\s", fields[1])
        invalid = [phone for phone in phones if phone and phone not in inventory]
        valid = [phone for phone in phones if phone in inventory]
        reading = [phone if "/" in phone else f"fr/{phone}" for phone in valid]
        record = {"line": number, "key": key, "sourceSymbols": phones, "invalidSymbols": invalid, "consumerReading": reading}
        counts[key] = counts.get(key, 0) + 1
        if len(records.setdefault(key, [])) < 64:
            records[key].append(record)
        effective[key] = reading
        if reading not in result.setdefault(key, []):
            result[key].append(reading)
    return result, {"sha256": hashlib.sha256(archive_bytes).hexdigest(), "dictionarySha256": hashlib.sha256(data).hexdigest(),
                    "inventorySha256": hashlib.sha256(inventory_data).hexdigest(), "ignoredRowCount": ignored_count,
                    "ignoredRowSamples": ignored, "rows": records, "effective": effective, "counts": counts}


def inspect(words, pack=None):
    if not 1 <= len(words) <= MAX_WORDS or any(not word.strip() or len(word.encode("utf-8")) > 1024 for word in words):
        raise ValueError("Provide 1..128 nonempty complete dictionary keys, at most 1024 bytes each")
    curated, curated_metadata = corpus(ROOT / "src-tauri/src/engine/target/french-lexicon.tsv")
    community, community_metadata = corpus(ROOT / "src-tauri/src/engine/target/french-community.tsv")
    native, metadata = native_dictionary(pack) if pack else (None, None)
    rows = []
    for raw in words:
        key = importer.canonical_key(raw.strip())
        evidence = curated.get(key) or community.get(key)
        native_readings = native.get(key, []) if native is not None else None
        native_rows = metadata["rows"].get(key, []) if metadata else None
        truncated = metadata["counts"].get(key, 0) > len(native_rows) if metadata else False
        rows.append({"input": raw, "key": key, "curated": curated.get(key), "community": community.get(key),
                     "nativeReadings": native_readings,
                     "nativeRows": native_rows, "nativeConsumerReading": metadata["effective"].get(key) if metadata else None,
                     "nativeRowCount": metadata["counts"].get(key, 0) if metadata else None, "nativeRowsTruncated": truncated,
                     "nativeAmbiguous": None if native_readings is None else len(native_readings) > 1,
                     "dictionaryAgreement": None if evidence is None or not native_readings or truncated else native_readings == [evidence["phones"]] and not any(row["invalidSymbols"] for row in native_rows)})
    return {"schemaVersion": 1, "mode": "dictionary-evidence-only", "corpora": {"curated": curated_metadata, "community": community_metadata},
            "nativePackSha256": metadata["sha256"] if metadata else None,
            "nativePack": {key: value for key, value in metadata.items() if key not in ("rows", "effective", "counts")} if metadata else None,
            "limitations": "Literal complete-word dictionary evidence only. Runtime aliases, ambiguity policy, source-owned context, liaison and acoustic quality are separate. Check score export diagnostics for FRENCH_LIAISON_APPLIED and FRENCH_PRONUNCIATION_UNSUPPORTED.",
            "words": rows}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--millefeuille-pack", type=Path)
    parser.add_argument("words", nargs="+")
    args = parser.parse_args()
    try:
        print(json.dumps(inspect(args.words, args.millefeuille_pack), ensure_ascii=False, indent=2))
        return 0
    except (OSError, ValueError, UnicodeError, RuntimeError, zipfile.BadZipFile, KeyError) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
