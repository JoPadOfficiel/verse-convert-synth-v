#!/usr/bin/env python3
"""Import the pinned EN-001 CMU dictionary using Python 3.9+ stdlib only.

    python3 scripts/import-english-lexicon.py --source cmudict.dict
    python3 scripts/import-english-lexicon.py --source cmudict.dict --check

The source and distributed license must match their fixed SHA-256 hashes.
There is no download, hash override, installed-bank dependency or runtime G2P.
--check compares complete artifact bytes and never writes files.

TSV columns: key, phones, source_lines, source_symbols. Source lines are comma
separated; original phone sequences are pipe separated in matching order.
Numbered variants remain distinct keys, even when their mapped readings match.
Original rows involved in normalization, deduplication or quarantine are kept
in the JSON provenance. Eight consonant-only CMU readings are valid data; their
keys are reported separately and do not imply a sung vowel allocation.
"""

import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import re
import sys
import unicodedata


ROOT = Path(__file__).resolve().parents[1]
SOURCE_COMMIT = "74790861f652b15e4ac49015a90074ad62a27690"
SOURCE_SHA256 = "81917843c7f44ce2b094ac63873c2c7a4cf802040792c455ba3ca406891c3d22"
SOURCE_BASE = "https://raw.githubusercontent.com/cmusphinx/cmudict/" + SOURCE_COMMIT
LICENSE_PATH = "public/licenses/cmudict.txt"
LICENSE_SHA256 = "bd4ce8e44170a5f9f481310ca85c51de3c4f851a65e679b40e603b143bd3542a"
OUTPUT_PATH = "src-tauri/src/engine/target/english-lexicon.tsv"
REPORT_PATH = "docs/english-lexicon-provenance.json"
OPENUTAU_COMMIT = "3f213e8993ca792c3e6f8958c92ab27eae78eac5"
VOWELS = frozenset("AA AE AH AO AW AY EH ER EY IH IY OW OY UH UW".split())
CONSONANTS = frozenset("B CH D DH F G HH JH K L M N NG P R S SH T TH V W Y Z ZH".split())
MAPPING = {phone: "en/" + phone.lower() for phone in VOWELS | CONSONANTS}
ALLOWED_PHONES = frozenset(MAPPING.values())
VOWEL_PHONES = frozenset(MAPPING[phone] for phone in VOWELS)
APOSTROPHES = str.maketrans({"\u2018": "'", "\u2019": "'"})
VARIANT = re.compile(r"\([0-9]+\)$")
PHONE_TOKEN = re.compile(r"[A-Za-z]+[012]?")


def canonical_key(word):
    """Preserve lexical punctuation, digits and numbered variant suffixes."""
    return unicodedata.normalize("NFC", word.lower().translate(APOSTROPHES))


def checked_bytes(path, expected, label):
    data = path.read_bytes()
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise ValueError(f"{label} SHA-256 mismatch: expected {expected}, got {actual}")
    return data


def parse_row(raw, number):
    fields = raw.split("#", 1)[0].split()
    word, tokens = fields[0], fields[1:]
    key = canonical_key(word)
    reasons = []
    if not key or any(unicodedata.category(char).startswith("C") for char in key):
        reasons.append("malformed_key")
    if not tokens:
        reasons.append("empty_reading")
    phones, unsupported, invalid_stress = [], [], []
    for token in tokens:
        # Strip only one terminal CMU stress digit, only from a phone token.
        # AH3, AH12 and stress on consonants are invalid, never repaired.
        stress = token[-1:] in ("0", "1", "2")
        phone = (token[:-1] if stress else token).upper()
        if not PHONE_TOKEN.fullmatch(token) or phone not in MAPPING:
            unsupported.append(token)
        elif stress and phone not in VOWELS:
            invalid_stress.append(token)
        else:
            phones.append(MAPPING[phone])
    if unsupported:
        reasons.append("unsupported_phonemes")
    if invalid_stress:
        reasons.append("invalid_stress")
    row = {
        "line": number,
        "raw": raw,
        "word": word,
        "key": key,
        "symbols": " ".join(tokens),
        "phones": None if reasons else phones,
    }
    if reasons:
        row["reasons"] = reasons
    if unsupported:
        row["unsupported_phonemes"] = sorted(set(unsupported))
    if invalid_stress:
        row["invalid_stress_tokens"] = sorted(set(invalid_stress))
    return row


def generate(data):
    """Pure deterministic transformation; CLI verifies the pin before calling."""
    lines = data.decode("utf-8").splitlines()
    groups = defaultdict(list)
    rows, comments, blanks = [], [], []
    for number, raw in enumerate(lines, 1):
        if not raw.strip():
            blanks.append(number)
            continue
        if raw.lstrip().startswith(("#", ";;;")):
            comments.append(number)
            continue
        row = parse_row(raw, number)
        rows.append(row)
        groups[row["key"]].append(row)

    accepted, duplicates, quarantines = [], [], []
    for key, alternatives in sorted(groups.items()):
        readings = {tuple(row["phones"]) for row in alternatives if row["phones"] is not None}
        invalid = any("reasons" in row for row in alternatives)
        if invalid or len(readings) > 1:
            reasons = []
            if invalid:
                reasons.append("invalid_alternative")
            if len(readings) > 1:
                reasons.append("conflicting_readings")
            quarantines.append({"key": key, "reasons": reasons, "rows": alternatives})
            continue
        accepted.append((key, alternatives))
        if len(alternatives) > 1:
            duplicates.append({"key": key, "rows": alternatives})

    header = (
        "# Generated by scripts/import-english-lexicon.py; do not edit.\n"
        f"# Source: {SOURCE_BASE}/cmudict.dict\n"
        f"# Source SHA-256: {SOURCE_SHA256}\n"
        f"# License: {LICENSE_PATH}; SHA-256: {LICENSE_SHA256}\n"
        "# Numbered variants are distinct keys; no semantic or sung-syllable choice is implied.\n"
        "# key\tphones\tsource_lines\tsource_symbols\n"
    )
    tsv = header + "".join(
        "\t".join((
            key,
            " ".join(alternatives[0]["phones"]),
            ",".join(str(row["line"]) for row in alternatives),
            "|".join(row["symbols"] for row in alternatives),
        )) + "\n"
        for key, alternatives in accepted
    )
    tsv_bytes = tsv.encode("utf-8")
    accepted_rows = sum(len(alternatives) for _, alternatives in accepted)
    quarantined_rows = sum(len(group["rows"]) for group in quarantines)
    no_vowel_keys = [
        key for key, alternatives in accepted
        if not VOWEL_PHONES.intersection(alternatives[0]["phones"])
    ]
    counts = {
        "source_lines": len(lines),
        "comment_lines": len(comments),
        "blank_lines": len(blanks),
        "entry_rows": len(rows),
        "accepted_source_rows": accepted_rows,
        "accepted_keys": len(accepted),
        "accepted_base_keys": len({VARIANT.sub("", key) for key, _ in accepted}),
        "accepted_numbered_variant_keys": sum(bool(VARIANT.search(key)) for key, _ in accepted),
        "accepted_keys_without_vowel": len(no_vowel_keys),
        "used_phones": len({phone for _, alternatives in accepted for phone in alternatives[0]["phones"]}),
        "deduplicated_keys": len(duplicates),
        "deduplicated_extra_rows": accepted_rows - len(accepted),
        "invalid_source_rows": sum("reasons" in row for row in rows),
        "invalid_rows_by_reason": dict(sorted(Counter(
            reason for row in rows for reason in row.get("reasons", [])
        ).items())),
        "quarantined_keys": len(quarantines),
        "quarantined_source_rows": quarantined_rows,
        "quarantined_keys_by_reason": dict(sorted(Counter(
            reason for group in quarantines for reason in group["reasons"]
        ).items())),
    }
    if len(lines) != len(comments) + len(blanks) + accepted_rows + quarantined_rows:
        raise ValueError("source line accounting failed")
    report = {
        "schema_version": 1,
        "source": {
            "repository": "https://github.com/cmusphinx/cmudict",
            "commit": SOURCE_COMMIT,
            "url": SOURCE_BASE + "/cmudict.dict",
            "sha256": SOURCE_SHA256,
            "bytes": len(data),
            "encoding": "UTF-8; hash covers unmodified source bytes",
            "dialect": "US English; not a universal English pronunciation authority",
        },
        "license": {"path": LICENSE_PATH, "url": SOURCE_BASE + "/LICENSE", "sha256": LICENSE_SHA256},
        "mapping": dict(sorted(MAPPING.items())),
        "allowed_phones": sorted(ALLOWED_PHONES),
        "vowel_phones": sorted(VOWEL_PHONES),
        "mapping_evidence": {
            "openutau_commit": OPENUTAU_COMMIT,
            "phonemizer_type": "OpenUtau.Core.DiffSinger.DiffSingerEnglishPhonemizer",
            "phonemizer_url": "https://github.com/openutau/OpenUtau/blob/" + OPENUTAU_COMMIT
                + "/OpenUtau.Core/DiffSinger/Phonemizers/DiffSingerEnglishPhonemizer.cs",
            "audited_bank": "UFR-V1.0/UFR_Lewisia",
            "bank_dictionary": "dsdur/dsdict-en.yaml",
            "bank_dictionary_sha256": "924a671a9dc93301d9399ef80042a56134a37fb6b4705201e700d6aa2b6c5e18",
            "model_inventory_file": "millefeuille_v001.phonemes.json",
            "model_inventory_sha256": {
                "0_CORE/dsacoustic": "a618b0e2030b91c492c3227ab21529591dff0f9d0bb6edeee040b49661b50249",
                "0_CORE/dsdur": "aba00afba20bc99d4342c106b184a029153c3bfde707fa9c29cb2881e770cdc1",
                "0_CORE/dspitch": "bbd8bf10e15ec0899e3587e2978ae1dd2dc3b6c9786e3131733dd07a73ebe309",
            },
            "mapped_phones_outside_audited_model_inventories": [],
            "scope": "All 39 CMU phones and vowel types audited; no installed bank required to regenerate",
        },
        "policy": {
            "normalization": "Lowercase; U+2018/U+2019 to ASCII apostrophe; NFC; preserve lexical punctuation, digits and variant suffixes",
            "parsing": "First whitespace token is the key; remaining tokens before # are phones; # and ;;; full-line comments are skipped",
            "stress": "Remove only one terminal 0/1/2 from vowel phones; retain original stressed symbols; reject unsupported phones and stressed consonants",
            "deduplication": "Merge equal canonical keys only when all mapped readings match; preserve every contributing source line and original row",
            "quarantine": "Any invalid alternative or differing mapped reading quarantines the entire canonical-key group with all original rows",
            "variants": "Numbered keys remain distinct, even for equal readings; no context, homograph, stress or sung-syllable selection",
            "no_vowel": "Keep valid consonant-only readings as lexical data and report their keys; they do not establish a sung nucleus",
            "runtime_scope": "Static data only; case/punctuation lookup, explicit overrides, syllable allocation and holds require separate runtime integration",
        },
        "output": {
            "path": OUTPUT_PATH,
            "sha256": hashlib.sha256(tsv_bytes).hexdigest(),
            "bytes": len(tsv_bytes),
            "encoding": "UTF-8 without BOM; LF endings; Unicode code point key order",
            "columns": ["key", "phones", "source_lines", "source_symbols"],
            "provenance_separators": {"source_lines": ",", "source_symbols": "|"},
        },
        "reproduce": {
            "generate": "python3 scripts/import-english-lexicon.py --source SOURCE",
            "check": "python3 scripts/import-english-lexicon.py --source SOURCE --check",
            "requirements": "Python 3.9+ stdlib; local pinned source and repository license; no network",
        },
        "counts": counts,
        "comment_lines": comments,
        "blank_lines": blanks,
        "keys_without_vowel": no_vowel_keys,
        "normalizations": [row for row in rows if row["key"] != row["word"]],
        "quarantines": quarantines,
        "deduplications": duplicates,
    }
    report_bytes = (json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode("utf-8")
    return tsv_bytes, report_bytes, counts


def publish(artifacts, check):
    if check:
        stale = [str(path) for path, data in artifacts if not path.is_file() or path.read_bytes() != data]
        if stale:
            raise ValueError("generated artifacts missing or stale: " + ", ".join(stale))
    else:
        for path, data in artifacts:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", type=Path, required=True, help="local, byte-exact pinned cmudict.dict")
    parser.add_argument("--check", action="store_true", help="verify source, license and both artifacts without writing")
    args = parser.parse_args()
    try:
        data = checked_bytes(args.source, SOURCE_SHA256, "source")
        checked_bytes(ROOT / LICENSE_PATH, LICENSE_SHA256, "license")
        tsv, report, counts = generate(data)
        publish([(ROOT / OUTPUT_PATH, tsv), (ROOT / REPORT_PATH, report)], args.check)
        print(
            f"{'Checked' if args.check else 'Generated'} {counts['accepted_keys']} keys; "
            f"{counts['accepted_numbered_variant_keys']} numbered variants; "
            f"{counts['deduplicated_extra_rows']} duplicate rows; "
            f"{counts['quarantined_source_rows']} quarantined rows."
        )
    except (OSError, UnicodeError, ValueError) as error:
        print(f"English lexicon import failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
