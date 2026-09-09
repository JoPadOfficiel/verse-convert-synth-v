#!/usr/bin/env python3
"""Import the pinned FR-002 community dictionary using only the Python stdlib.

    python3 scripts/import-french-lexicon.py --source SOURCE
    python3 scripts/import-french-lexicon.py --source SOURCE --check

SOURCE must contain the exact upstream bytes, including CRLF line endings.
There is no download, hash override, installed-bank dependency or runtime G2P.
--check regenerates both artifacts in memory and compares their complete bytes.

TSV columns: key, phones, source_lines, source_symbols. Source lines are comma
separated; source symbol sequences are pipe separated in the same order. Each
sequence retains the upstream tokens, with whitespace standardized to spaces.
Numbered variants and internal spaces/punctuation remain part of the key.
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
SOURCE_COMMIT = "1e11bbe4df10bd4df03dfbf90ace92924c56e5b0"
SOURCE_SHA256 = "b4560adb8e5e2145f7b8a25f810db4c2f798d7db518f1d814d0dbae9728f7d34"
SOURCE_BASE = (
    "https://raw.githubusercontent.com/mmemim/OpenUTAU-French-Dictionary/"
    + SOURCE_COMMIT
)
LICENSE_PATH = "public/licenses/french-community-dictionary.txt"
LICENSE_SHA256 = "11e3c35cb4439d0086b5cc9309b31ab00185a7b10bb1f584e042769d0d78302c"
OUTPUT_PATH = "src-tauri/src/engine/target/french-community.tsv"
REPORT_PATH = "docs/french-lexicon-provenance.json"
OPENUTAU_COMMIT = "3f213e8993ca792c3e6f8958c92ab27eae78eac5"

# Audited community -> Millefeuille translation. gn expands to two existing
# tokens; un follows FR-001's metropolitan fr/in convention. Do not guess the
# malformed tokens found in this corpus (4, ih, m, u, c, zzvérita, or -).
MAPPING = {
    "aa": ("fr/ah",), "ai": ("fr/ae",), "ei": ("fr/eh",),
    "eu": ("fr/ee",), "ee": ("fr/ee",), "oe": ("fr/oe",),
    "ii": ("fr/ih",), "au": ("fr/oh",), "oo": ("fr/oo",),
    "ou": ("fr/ou",), "uu": ("fr/uh",), "an": ("fr/en",),
    "in": ("fr/in",), "un": ("fr/in",), "on": ("fr/on",),
    "uy": ("fr/uy",), "bb": ("fr/b",), "dd": ("fr/d",),
    "ff": ("fr/f",), "gg": ("fr/g",), "jj": ("fr/j",),
    "kk": ("fr/k",), "ll": ("fr/l",), "mm": ("fr/m",),
    "nn": ("fr/n",), "pp": ("fr/p",), "rr": ("fr/r",),
    "ss": ("fr/s",), "ch": ("fr/sh",), "tt": ("fr/t",),
    "vv": ("fr/v",), "ww": ("fr/w",), "yy": ("fr/y",),
    "zz": ("fr/z",), "hh": ("fr/h",), "gn": ("fr/n", "fr/y"),
}
# Nonempty symbols from the pinned FrenchMillefeuilleG2p phoneme inventory.
ALLOWED_PHONES = frozenset(
    "fr/ah fr/eh fr/ae fr/ee fr/oe fr/ih fr/oh fr/oo fr/ou fr/uh fr/en "
    "fr/in fr/on fr/uy fr/y fr/w fr/f fr/k fr/p fr/s fr/sh fr/t fr/h "
    "fr/b fr/d fr/g fr/l fr/m fr/n fr/r fr/v fr/z fr/j fr/ng fr/q".split()
)
# uy, y and w are semivowels, not sung vowel nuclei.
VOWELS = frozenset(
    "fr/ah fr/eh fr/ae fr/ee fr/oe fr/ih fr/oh fr/oo fr/ou fr/uh fr/en fr/in fr/on".split()
)
SEPARATOR = re.compile(r" {2,}|\t+")
APOSTROPHES = str.maketrans({"\u2018": "'", "\u2019": "'"})


def canonical_key(word):
    """NFC after lowercase; preserve numbered variants and internal spelling."""
    return unicodedata.normalize("NFC", word.lower().translate(APOSTROPHES))


def checked_bytes(path, expected, label):
    data = path.read_bytes()
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise ValueError(f"{label} SHA-256 mismatch: expected {expected}, got {actual}")
    return data


def parse_row(line, number):
    """Attribute invalid alternatives to a key before deciding which keys emit."""
    fields = SEPARATOR.split(line, maxsplit=1)
    reason = None
    if not line.strip():
        word, symbols, reason = "", "", "empty_row"
    elif len(fields) != 2:
        # Retain malformed rows, never repair them into accepted entries. The
        # first whitespace field attributes an invalid alternative conservatively.
        fallback = line.split(maxsplit=1)
        word = fallback[0]
        symbols = fallback[1] if len(fallback) == 2 else ""
        reason = "malformed_separator"
    else:
        word, symbols = fields[0].strip(), fields[1].strip()
    key = canonical_key(word)
    tokens = symbols.split()
    if not reason and (not key or any(unicodedata.category(c)[0] == "C" for c in key)):
        reason = "malformed_key"
    if not reason and not tokens:
        reason = "empty_reading"
    unsupported = sorted(set(tokens) - MAPPING.keys())
    phones = None if unsupported or not tokens else [
        phone for token in tokens for phone in MAPPING[token]
    ]
    if not reason and unsupported:
        reason = "unsupported_phonemes"
    if not reason and not VOWELS.intersection(phones):
        reason = "no_vowel"
    row = {
        "line": number,
        "word": word,
        "key": key,
        "symbols": " ".join(tokens),
        "phones": phones,
    }
    if reason:
        row["reason"] = reason
        row["raw"] = line
    if unsupported:
        row["unsupported_phonemes"] = unsupported
    return row


def generate(data):
    if any(phone not in ALLOWED_PHONES for phones in MAPPING.values() for phone in phones):
        raise ValueError("mapping contains a phone outside the pinned inventory")
    lines = data.decode("utf-8").splitlines()
    groups = defaultdict(list)
    rows, comments = [], []
    for number, line in enumerate(lines, 1):
        if line.lstrip().startswith(";;;"):
            comments.append(number)
            continue
        row = parse_row(line, number)
        rows.append(row)
        if row["key"]:
            groups[row["key"]].append(row)

    accepted, duplicates, quarantines = [], [], []
    for key, alternatives in sorted(groups.items()):
        readings = {tuple(row["phones"]) for row in alternatives if row["phones"] is not None}
        invalid = any("reason" in row for row in alternatives)
        if len(alternatives) > 1 and (len(readings) > 1 or invalid):
            reasons = []
            if len(readings) > 1:
                reasons.append("conflicting_readings")
            if invalid:
                reasons.append("invalid_alternative")
            quarantines.append({
                "key": key,
                "reasons": reasons,
                "rows": [dict(row) for row in alternatives],
            })
            for row in alternatives:
                row.setdefault("reason", reasons[0])
                row["quarantine_key"] = key
            continue
        if invalid:
            continue
        accepted.append((key, alternatives))
        if len(alternatives) > 1:
            duplicates.append({"key": key, "rows": [dict(row) for row in alternatives]})

    excluded = [row for row in rows if "reason" in row]
    header = (
        "# Generated by scripts/import-french-lexicon.py; do not edit.\n"
        f"# Source: {SOURCE_BASE}/cmudict_fr.txt\n"
        f"# Source SHA-256: {SOURCE_SHA256}\n"
        f"# License: {LICENSE_PATH}; SHA-256: {LICENSE_SHA256}\n"
        "# Numbered variants are distinct keys; membership does not resolve context or homographs.\n"
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
    counts = {
        "source_lines": len(lines),
        "comment_lines": len(comments),
        "entry_rows": sum(row.get("reason") != "empty_row" for row in rows),
        "blank_rows": sum(row.get("reason") == "empty_row" for row in rows),
        "accepted_source_rows": accepted_rows,
        "accepted_keys": len(accepted),
        "accepted_numbered_variant_keys": sum(bool(re.search(r"\([0-9]+\)$", key)) for key, _ in accepted),
        "accepted_multiword_keys": sum(" " in key for key, _ in accepted),
        "deduplicated_keys": len(duplicates),
        "deduplicated_extra_rows": accepted_rows - len(accepted),
        "excluded_rows": len(excluded),
        "excluded_rows_by_reason": dict(sorted(Counter(row["reason"] for row in excluded).items())),
        "quarantined_keys": len(quarantines),
        "quarantined_source_rows": sum(len(group["rows"]) for group in quarantines),
        "quarantined_keys_by_reason": dict(sorted(Counter(
            reason for group in quarantines for reason in group["reasons"]
        ).items())),
        "gn_source_rows": sum("gn" in row["symbols"].split() for row in rows),
        "gn_accepted_source_rows": sum(
            "gn" in row["symbols"].split() for _, alternatives in accepted for row in alternatives
        ),
    }
    if len(lines) != len(comments) + accepted_rows + len(excluded):
        raise ValueError("source line accounting failed")
    report = {
        "schema_version": 1,
        "source": {
            "repository": "https://github.com/mmemim/OpenUTAU-French-Dictionary",
            "commit": SOURCE_COMMIT,
            "version": "1.6",
            "url": SOURCE_BASE + "/cmudict_fr.txt",
            "sha256": SOURCE_SHA256,
            "bytes": len(data),
            "encoding": "UTF-8; hash covers original bytes including CRLF",
        },
        "license": {"path": LICENSE_PATH, "url": SOURCE_BASE + "/LICENSE", "sha256": LICENSE_SHA256},
        "mapping": {key: list(value) for key, value in sorted(MAPPING.items())},
        "allowed_phones": sorted(ALLOWED_PHONES),
        "vowel_phones": sorted(VOWELS),
        "mapping_evidence": {
            "phoneme_inventory_url": (
                "https://raw.githubusercontent.com/openutau/OpenUtau/" + OPENUTAU_COMMIT
                + "/OpenUtau.Core/G2p/FrenchMillefeuilleG2p.cs"
            ),
            "phoneme_inventory_source_sha256": "98e4d2d8c7101b5cb6df496fe679e7b810530b4f9a2026a5ce73a4605865147e",
            "model_inventory_file": "millefeuille_v001.phonemes.json",
            "model_inventory_sha256": "a618b0e2030b91c492c3227ab21529591dff0f9d0bb6edeee040b49661b50249",
            "mapped_phones_outside_audited_model_inventory": [],
            "gn": {
                "verified_date": "2026-09-09",
                "method": "Installed OpenUtau Millefeuille embedded dictionary queries (--embedded)",
                "observations": {
                    "gagner": ["g", "ah", "n", "y", "eh"],
                    "montagne": ["m", "on", "t", "ah", "n", "y"],
                    "ligne": ["l", "ih", "n", "y"],
                },
                "policy": "gn expands to fr/n fr/y; no invented fr/ny token",
            },
            "un": "FR-001 metropolitan convention: upstream un maps to fr/in",
            "no_vowel": "uy, y and w are semivowels; only vowel_phones qualify as sung nuclei",
        },
        "policy": {
            "normalization": "lowercase; U+2018/U+2019 to ASCII apostrophe; NFC; no edge-punctuation or variant stripping",
            "parsing": "Two or more ASCII spaces or a tab separate key from phones; single internal spaces belong to the key",
            "malformed_key_attribution": "Without a separator, first whitespace field identifies the quarantined key; row is never accepted",
            "deduplication": "Equal canonical keys and mapped readings merge, preserving all source lines and symbol sequences",
            "quarantine": "Any conflicting mapped readings or invalid alternative quarantine the entire canonical-key group",
            "exclusions": "Malformed/empty rows, unsupported phonemes and readings without a vowel never enter automatic sung lookup",
            "runtime_scope": "Static symbol-compatible data only; curated FR-001 overrides, fragment guards and homograph/context decisions remain separate",
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
            "generate": "python3 scripts/import-french-lexicon.py --source SOURCE",
            "check": "python3 scripts/import-french-lexicon.py --source SOURCE --check",
            "requirements": "Python 3.9+ stdlib; local pinned source and repository license; no network",
        },
        "counts": counts,
        "comment_lines": comments,
        "excluded_rows": excluded,
        "quarantines": quarantines,
        "deduplications": duplicates,
    }
    report_bytes = (json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode("utf-8")
    return tsv_bytes, report_bytes, counts


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", type=Path, required=True, help="local, byte-exact pinned cmudict_fr.txt")
    parser.add_argument("--check", action="store_true", help="compare both artifacts without writing")
    parser.add_argument("--output", type=Path, default=ROOT / OUTPUT_PATH, help="TSV destination (default: repository artifact)")
    parser.add_argument("--provenance", type=Path, default=ROOT / REPORT_PATH, help="JSON destination (default: repository report)")
    args = parser.parse_args()
    try:
        data = checked_bytes(args.source, SOURCE_SHA256, "source")
        checked_bytes(ROOT / LICENSE_PATH, LICENSE_SHA256, "license")
        destinations = [args.output.resolve(), args.provenance.resolve()]
        if len(set(destinations)) != 2 or any(
            path in (args.source.resolve(), (ROOT / LICENSE_PATH).resolve()) for path in destinations
        ):
            raise ValueError("output paths must be distinct and must not overwrite source or license")
        tsv, report, counts = generate(data)
        artifacts = [(args.output, tsv), (args.provenance, report)]
        if args.check:
            stale = [str(path) for path, expected in artifacts if not path.is_file() or path.read_bytes() != expected]
            if stale:
                raise ValueError("generated artifacts missing or stale: " + ", ".join(stale))
        else:
            for path, content in artifacts:
                path.write_bytes(content)
        print(
            f"{'Checked' if args.check else 'Generated'} {counts['accepted_keys']} keys "
            f"from {counts['accepted_source_rows']} source rows; "
            f"{counts['excluded_rows']} excluded rows; "
            f"{counts['quarantined_keys']} quarantined keys."
        )
    except (OSError, UnicodeError, ValueError) as error:
        print(f"French lexicon import failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
