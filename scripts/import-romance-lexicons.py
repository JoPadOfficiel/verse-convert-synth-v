#!/usr/bin/env python3
"""Build pinned Spanish/Portuguese routing and pronunciation corpora.

The wooorm/Hunspell corpora are language-routing evidence only. The Montreal
Forced Aligner dictionaries are the lexical pronunciation authority. Runtime
code maps their MFA phones to a verified DiffSinger inventory; this importer
never invents or merges source readings.

Regenerate from the pinned community dictionaries:

    python3 scripts/import-romance-lexicons.py --download

Verify committed artifacts against the same pinned inputs without writing:

    python3 scripts/import-romance-lexicons.py --download --check

For offline pinned checkouts, replace ``--download`` with both
``--routing-source-root /path/to/dictionaries`` and
``--mfa-source-root /path/to/mfa-models``.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
from pathlib import Path
import tempfile
import unicodedata
from urllib.request import urlopen


ROOT = Path(__file__).resolve().parents[1]
ROUTING_PINNED_COMMIT = "8cfea406b505e4d7df52d5a19bce525df98c54ab"
ROUTING_RAW_BASE = (
    f"https://raw.githubusercontent.com/wooorm/dictionaries/{ROUTING_PINNED_COMMIT}/"
)
MFA_PINNED_COMMIT = "d6eff86a42c6a90b641e17dfdf7a16555b934483"
MFA_RAW_BASE = (
    f"https://raw.githubusercontent.com/MontrealCorpusTools/mfa-models/{MFA_PINNED_COMMIT}/"
)
MFA_LICENSE = "dictionary/spanish/spain_mfa/v3.3.0/LICENSE"
MFA_LICENSE_SHA256 = "7e7170e3cebf88a9f60c7b8421418323c09304da1af4d5e90f4da1dc1c8a2661"
MFA_LICENSE_OUTPUT = "public/licenses/mfa-pronunciation-dictionaries-cc-by-4.0.txt"
MFA_ATTRIBUTION_OUTPUT = "public/licenses/mfa-pronunciation-attribution.txt"

SOURCES = {
    "spanish": {
        "dictionary": "dictionaries/es/index.dic",
        "dictionary_sha256": "907f786a8ceb3456722b20ad91dd4dbe99c5c16e45015ea63155e36f26b06d2c",
        "license": "dictionaries/es/license",
        "license_sha256": "a21581497f07b5e550e438d26890ea2b1704492357de67740613bd589b1c652a",
        "license_output": "public/licenses/spanish-community-dictionary.txt",
    },
    "portuguese_brazil": {
        "dictionary": "dictionaries/pt/index.dic",
        "dictionary_sha256": "32e2edd83541d58613bcc83ef731a79aadf85ed57e977a16c6b5f711a7a36cee",
        "license": "dictionaries/pt/license",
        "license_sha256": "bb064f3db7ba5f5536da7d079773f692c5f928822e8aebc3d78259ff6cceb9e5",
        "license_output": "public/licenses/portuguese-brazil-community-dictionary.txt",
    },
    "portuguese_portugal": {
        "dictionary": "dictionaries/pt-PT/index.dic",
        "dictionary_sha256": "fcac21564163586df74ba88cde362bb75560548b35bcaa780558c8f97f43de04",
        "license": "dictionaries/pt-PT/license",
        "license_sha256": "a5d807381c41a7186b235f17cdf472f53d83831cb05a2bae3c44581cc818570c",
        "license_output": "public/licenses/portuguese-portugal-community-dictionary.txt",
    },
}

MFA_SOURCES = {
    "spanish_spain": {
        "language": "Spanish",
        "dialect": "spain",
        "version": "3.3.0",
        "dictionary": "dictionary/spanish/spain_mfa/spanish_spain_mfa.dict",
        "dictionary_sha256": "16d5bfc4eabcb96af142e9b369663072da36412ef5c6c7f7f3847e7b92d24668",
        "meta": "dictionary/spanish/spain_mfa/v3.3.0/meta.json",
        "meta_sha256": "aab9abc39ab73c13ee81e1d9d110a068bf853df39d4ffb2393f559e5c7ebf2b3",
    },
    "spanish_latin_america": {
        "language": "Spanish",
        "dialect": "latin-america",
        "version": "3.3.0",
        "dictionary": "dictionary/spanish/latin_america_mfa/spanish_latin_america_mfa.dict",
        "dictionary_sha256": "dc84a3e8e153b5e7e2856f820d0780b6694e5d0a8b515cb90e05bac10615513a",
        "meta": "dictionary/spanish/latin_america_mfa/v3.3.0/meta.json",
        "meta_sha256": "006b749cc1cb08c12407f7841ee10e63047f76c86a94081019e7b4682dfc99e7",
    },
    "portuguese_brazil": {
        "language": "Portuguese",
        "dialect": "brazil",
        "version": "2.0.0",
        "dictionary": "dictionary/portuguese/brazil_mfa/portuguese_brazil_mfa.dict",
        "dictionary_sha256": "6ec51b884952f1076ad4a3ae17ec7e0c274b8df0dbd681b32e31ee78118f1861",
        "meta": "dictionary/portuguese/brazil_mfa/v2.0.0/meta.json",
        "meta_sha256": "716430af00122e0360cad7559a420acd199f054d2db0d1fc826f7b91af37abae",
    },
    "portuguese_portugal": {
        "language": "Portuguese",
        "dialect": "portugal",
        "version": "2.0.0",
        "dictionary": "dictionary/portuguese/portugal_mfa/portuguese_portugal_mfa.dict",
        "dictionary_sha256": "260f08d8d218f1cd8a4675f8ece20e8e239863f262cdc8d0948693f0abac9703",
        "meta": "dictionary/portuguese/portugal_mfa/v2.0.0/meta.json",
        "meta_sha256": "d040bad61033b896e3c6aa2e1a354660c61af0fb7cb1c01cafab9ecc023da6bb",
    },
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def checked(data: bytes, expected: str, label: str) -> bytes:
    actual = sha256(data)
    if actual != expected:
        raise ValueError(f"{label} SHA-256 mismatch: expected {expected}, got {actual}")
    return data


def canonical(word: str) -> str:
    return unicodedata.normalize("NFC", word.casefold().replace("‘", "'").replace("’", "'"))


def hunspell_word(line: str) -> str:
    """Return the dictionary spelling before flags/morphology, honoring escapes."""
    chars: list[str] = []
    escaped = False
    for char in line:
        if escaped:
            chars.append(char)
            escaped = False
        elif char == "\\":
            escaped = True
        elif char == "/" or char.isspace():
            break
        else:
            chars.append(char)
    if escaped:
        chars.append("\\")
    return "".join(chars)


def accepted_key(word: str) -> str | None:
    key = canonical(word.strip())
    if not key or not any(char.isalpha() for char in key):
        return None
    if not all(char.isalpha() or char in "'-‐‑‒–—" for char in key):
        return None
    return key


def parse_dictionary(data: bytes) -> tuple[set[str], dict[str, int]]:
    text = data.decode("utf-8-sig")
    words: set[str] = set()
    entry_rows = 0
    rejected_rows = 0
    duplicate_rows = 0
    for number, raw in enumerate(text.splitlines(), start=1):
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if number == 1 and stripped.isdigit():
            continue
        entry_rows += 1
        key = accepted_key(hunspell_word(raw))
        if key is None:
            rejected_rows += 1
            continue
        before = len(words)
        words.add(key)
        duplicate_rows += int(len(words) == before)
    return words, {
        "entry_rows": entry_rows,
        "accepted_keys": len(words),
        "rejected_rows": rejected_rows,
        "duplicate_rows": duplicate_rows,
    }


def parse_mfa_dictionary(
    data: bytes,
) -> tuple[set[tuple[str, str]], dict[str, int], list[dict[str, int | str]]]:
    """Keep every distinct lexical reading exactly as MFA publishes it."""
    readings: set[tuple[str, str]] = set()
    rejections: list[dict[str, int | str]] = []
    entry_rows = 0
    duplicate_rows = 0
    for number, raw in enumerate(data.decode("utf-8-sig").splitlines(), start=1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        entry_rows += 1
        if "\t" not in raw:
            rejections.append({"line": number, "text": raw, "reason": "missing_tab_separator"})
            continue
        word, phones = raw.split("\t", 1)
        key = accepted_key(word)
        phones = unicodedata.normalize("NFC", " ".join(phones.split()))
        if key is None or not phones:
            reason = "non_lexical_key" if key is None else "empty_pronunciation"
            rejections.append({"line": number, "text": raw, "reason": reason})
            continue
        before = len(readings)
        readings.add((key, phones))
        duplicate_rows += int(len(readings) == before)
    return readings, {
        "entry_rows": entry_rows,
        "accepted_readings": len(readings),
        "accepted_words": len({word for word, _ in readings}),
        "rejected_rows": len(rejections),
        "duplicate_rows": duplicate_rows,
    }, rejections


def lexical_bytes(
    language: str,
    rows: list[tuple[str, str]],
    source_labels: list[str],
    columns: str,
) -> bytes:
    header = [
        "# Generated by scripts/import-romance-lexicons.py; do not edit.",
        f"# Language-routing membership only: {language}.",
        f"# Pinned wooorm/dictionaries commit: {ROUTING_PINNED_COMMIT}.",
        f"# Sources: {', '.join(source_labels)}.",
        "# Pronunciation is owned separately by the pinned MFA pronunciation TSV.",
        f"# {columns}",
    ]
    body = [f"{word}\t{membership}" for word, membership in rows]
    return ("\n".join(header + body) + "\n").encode("utf-8")


def pronunciation_bytes(
    language: str,
    rows: list[tuple[str, str, str]],
    source_labels: list[str],
) -> bytes:
    header = [
        "# Generated by scripts/import-romance-lexicons.py; do not edit.",
        f"# Lexical pronunciation authority: Montreal Forced Aligner {language} dictionaries.",
        f"# Pinned mfa-models commit: {MFA_PINNED_COMMIT}.",
        f"# Sources: {', '.join(source_labels)}.",
        "# Source MFA phones are retained verbatim after NFC normalization.",
        "# word\tdialect-membership\tmfa-phones",
    ]
    body = [f"{word}\t{dialect}\t{phones}" for word, dialect, phones in rows]
    return ("\n".join(header + body) + "\n").encode("utf-8")


def source_bytes(
    path: str,
    expected_hash: str,
    *,
    download: bool,
    source_root: Path | None,
    raw_base: str,
) -> bytes:
    if download:
        with urlopen(raw_base + path, timeout=60) as response:
            data = response.read()
    else:
        if source_root is None:
            raise ValueError("provide --download or the matching source root")
        data = (source_root / path).read_bytes()
    return checked(data, expected_hash, path)


def report_bytes(
    language: str,
    inputs: list[dict],
    output_path: str,
    output: bytes,
    counts: dict,
    pronunciation: dict,
) -> bytes:
    payload = {
        "language": language,
        "purpose": "automatic language-routing lexical membership; not a pronunciation dictionary",
        "pinned_repository": "https://github.com/wooorm/dictionaries",
        "pinned_commit": ROUTING_PINNED_COMMIT,
        "sources": inputs,
        "counts": counts,
        "output": {"path": output_path, "bytes": len(output), "sha256": sha256(output)},
        "pronunciation": pronunciation,
    }
    return (json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode("utf-8")


def merged_pronunciation_rows(
    first: set[tuple[str, str]],
    first_label: str,
    second: set[tuple[str, str]],
    second_label: str,
) -> list[tuple[str, str, str]]:
    membership: dict[tuple[str, str], set[str]] = {}
    for word, phones in first:
        membership.setdefault((word, phones), set()).add(first_label)
    for word, phones in second:
        membership.setdefault((word, phones), set()).add(second_label)
    rows = []
    for (word, phones), dialects in membership.items():
        dialect = "+".join(label for label in (first_label, second_label) if label in dialects)
        rows.append((word, dialect, phones))
    return sorted(rows)


def attribution_bytes() -> bytes:
    lines = [
        "Montreal Forced Aligner pronunciation dictionaries bundled by Verse",
        "",
        "Source repository: https://github.com/MontrealCorpusTools/mfa-models",
        f"Pinned revision: {MFA_PINNED_COMMIT}",
        "License: Creative Commons Attribution 4.0 International (CC BY 4.0)",
        "Maintainers credited by the model metadata: Michael McAuliffe and Morgan Sonderegger / Montreal Forced Aligner.",
        "",
        "Bundled pronunciation sources:",
    ]
    for spec in MFA_SOURCES.values():
        lines.append(
            f"- {spec['language']} ({spec['dialect']}), v{spec['version']}: {spec['dictionary']}"
        )
    lines.extend([
        "",
        "Adaptation notice: Verse normalizes dictionary keys to Unicode NFC and casefolded spelling, normalizes curly apostrophes, filters non-lexical entries, normalizes phone-string whitespace and Unicode NFC, removes duplicate readings, and combines identical readings with dialect labels in sorted TSV data. Distinct source pronunciations remain separate.",
        "At runtime, Verse maps supported MFA phones to DiffSinger symbols only when every source reading maps exactly to the same hint; unsupported or divergent readings receive no explicit hint. These import and runtime adaptations are made by Verse.",
    ])
    return ("\n".join(lines) + "\n").encode("utf-8")


def publish(artifacts: list[tuple[Path, bytes]], check: bool) -> None:
    def display(path: Path) -> str:
        try:
            return str(path.relative_to(ROOT))
        except ValueError:
            return str(path)

    stale = [display(path) for path, data in artifacts if not path.exists() or path.read_bytes() != data]
    if check:
        if stale:
            raise ValueError("missing or stale romance lexicon artifact(s): " + ", ".join(stale))
        return
    # Generated outputs are intentionally replaceable. Stage every changed file
    # and its original before publishing; this handles exceptions, not crashes
    # or concurrent writers.
    staged: list[tuple[Path, Path, Path | None]] = []
    published: list[tuple[Path, Path | None]] = []
    temporary_paths: set[Path] = set()

    def temporary(path: Path) -> Path:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=path.name + ".", delete=False) as handle:
            result = Path(handle.name)
        temporary_paths.add(result)
        return result

    try:
        for path, data in artifacts:
            if path.exists() and path.read_bytes() == data:
                continue
            path.parent.mkdir(parents=True, exist_ok=True)
            replacement = temporary(path)
            replacement.write_bytes(data)
            replacement.chmod(0o644)
            backup = None
            if path.exists():
                backup = temporary(path)
                shutil.copy2(path, backup)
            staged.append((path, replacement, backup))
        for path, replacement, backup in staged:
            os.replace(replacement, path)
            published.append((path, backup))
    except BaseException as error:
        rollback_errors = []
        for path, backup in reversed(published):
            try:
                if backup is None:
                    path.unlink()
                else:
                    os.replace(backup, path)
            except OSError as rollback_error:
                # Keep recovery evidence if the filesystem also refuses rollback.
                if backup is not None:
                    temporary_paths.discard(backup)
                rollback_errors.append(f"{path}: {rollback_error}; backup: {backup}")
        if rollback_errors:
            raise OSError("publication failed; rollback incomplete: " + "; ".join(rollback_errors)) from error
        raise
    finally:
        for path in sorted(temporary_paths):
            path.unlink(missing_ok=True)


def build(
    *,
    download: bool,
    routing_source_root: Path | None,
    mfa_source_root: Path | None,
) -> list[tuple[Path, bytes]]:
    loaded: dict[str, dict] = {}
    artifacts: list[tuple[Path, bytes]] = []
    for name, spec in SOURCES.items():
        dictionary = source_bytes(
            spec["dictionary"], spec["dictionary_sha256"], download=download,
            source_root=routing_source_root, raw_base=ROUTING_RAW_BASE,
        )
        license_bytes = source_bytes(
            spec["license"], spec["license_sha256"], download=download,
            source_root=routing_source_root, raw_base=ROUTING_RAW_BASE,
        )
        words, counts = parse_dictionary(dictionary)
        loaded[name] = {"words": words, "counts": counts, "spec": spec, "dictionary_bytes": len(dictionary)}
        artifacts.append((ROOT / spec["license_output"], license_bytes))

    mfa_loaded: dict[str, dict] = {}
    for name, spec in MFA_SOURCES.items():
        dictionary = source_bytes(
            spec["dictionary"], spec["dictionary_sha256"], download=download,
            source_root=mfa_source_root, raw_base=MFA_RAW_BASE,
        )
        meta = source_bytes(
            spec["meta"], spec["meta_sha256"], download=download,
            source_root=mfa_source_root, raw_base=MFA_RAW_BASE,
        )
        metadata = json.loads(meta)
        if metadata.get("version") != spec["version"] or metadata.get("license") != "CC BY 4.0":
            raise ValueError(f"unexpected MFA metadata for {name}")
        readings, counts, rejections = parse_mfa_dictionary(dictionary)
        mfa_loaded[name] = {
            "readings": readings,
            "counts": counts,
            "rejections": rejections,
            "spec": spec,
            "dictionary_bytes": len(dictionary),
            "meta_bytes": len(meta),
            "phones": metadata.get("phones", []),
        }

    mfa_license = source_bytes(
        MFA_LICENSE, MFA_LICENSE_SHA256, download=download,
        source_root=mfa_source_root, raw_base=MFA_RAW_BASE,
    )
    artifacts.extend([
        (ROOT / MFA_LICENSE_OUTPUT, mfa_license),
        (ROOT / MFA_ATTRIBUTION_OUTPUT, attribution_bytes()),
    ])

    def pronunciation_source(item: dict) -> dict:
        spec = item["spec"]
        return {
            "dialect": spec["dialect"],
            "version": spec["version"],
            "path": spec["dictionary"],
            "url": MFA_RAW_BASE + spec["dictionary"],
            "sha256": spec["dictionary_sha256"],
            "bytes": item["dictionary_bytes"],
            "meta_path": spec["meta"],
            "meta_sha256": spec["meta_sha256"],
            "meta_bytes": item["meta_bytes"],
            "phones": item["phones"],
            "counts": item["counts"],
            "rejections": item["rejections"],
        }

    es_spain = mfa_loaded["spanish_spain"]
    es_latam = mfa_loaded["spanish_latin_america"]
    spanish_pronunciation_rows = merged_pronunciation_rows(
        es_spain["readings"], "spain", es_latam["readings"], "latin-america",
    )
    spanish_pronunciation_path = "src-tauri/src/engine/target/spanish-pronunciation.tsv"
    spanish_pronunciation_output = pronunciation_bytes(
        "Spanish",
        spanish_pronunciation_rows,
        [es_spain["spec"]["dictionary"], es_latam["spec"]["dictionary"]],
    )
    artifacts.append((ROOT / spanish_pronunciation_path, spanish_pronunciation_output))

    pt_brazil = mfa_loaded["portuguese_brazil"]
    pt_portugal = mfa_loaded["portuguese_portugal"]
    portuguese_pronunciation_rows = merged_pronunciation_rows(
        pt_brazil["readings"], "brazil", pt_portugal["readings"], "portugal",
    )
    portuguese_pronunciation_path = "src-tauri/src/engine/target/portuguese-pronunciation.tsv"
    portuguese_pronunciation_output = pronunciation_bytes(
        "Portuguese",
        portuguese_pronunciation_rows,
        [pt_brazil["spec"]["dictionary"], pt_portugal["spec"]["dictionary"]],
    )
    artifacts.append((ROOT / portuguese_pronunciation_path, portuguese_pronunciation_output))

    spanish = loaded["spanish"]
    spanish_path = "src-tauri/src/engine/target/spanish-community.tsv"
    spanish_output = lexical_bytes(
        "Spanish",
        [(word, "es") for word in sorted(spanish["words"])],
        [spanish["spec"]["dictionary"]],
        "word\tlanguage",
    )
    spanish_inputs = [{
        "path": spanish["spec"]["dictionary"],
        "url": ROUTING_RAW_BASE + spanish["spec"]["dictionary"],
        "sha256": spanish["spec"]["dictionary_sha256"],
        "bytes": spanish["dictionary_bytes"],
        "license_path": spanish["spec"]["license_output"],
        "license_sha256": spanish["spec"]["license_sha256"],
        "counts": spanish["counts"],
    }]
    artifacts.extend([
        (ROOT / spanish_path, spanish_output),
        (
            ROOT / "docs/spanish-lexicon-provenance.json",
            report_bytes(
                "Spanish", spanish_inputs, spanish_path, spanish_output, spanish["counts"],
                {
                    "purpose": "lexical pronunciation authority",
                    "pinned_repository": "https://github.com/MontrealCorpusTools/mfa-models",
                    "pinned_commit": MFA_PINNED_COMMIT,
                    "license": "CC BY 4.0",
                    "license_path": MFA_LICENSE_OUTPUT,
                    "license_sha256": MFA_LICENSE_SHA256,
                    "attribution_path": MFA_ATTRIBUTION_OUTPUT,
                    "sources": [pronunciation_source(es_spain), pronunciation_source(es_latam)],
                    "output": {
                        "path": spanish_pronunciation_path,
                        "bytes": len(spanish_pronunciation_output),
                        "sha256": sha256(spanish_pronunciation_output),
                        "distinct_readings": len(spanish_pronunciation_rows),
                    },
                },
            ),
        ),
    ])

    br = loaded["portuguese_brazil"]
    pt = loaded["portuguese_portugal"]
    portuguese_words = br["words"] | pt["words"]
    portuguese_rows = []
    for word in sorted(portuguese_words):
        in_br = word in br["words"]
        in_pt = word in pt["words"]
        membership = "br+pt" if in_br and in_pt else "br" if in_br else "pt"
        portuguese_rows.append((word, membership))
    portuguese_path = "src-tauri/src/engine/target/portuguese-community.tsv"
    portuguese_output = lexical_bytes(
        "Portuguese (Brazil + Portugal)", portuguese_rows,
        [br["spec"]["dictionary"], pt["spec"]["dictionary"]],
        "word\tcommunity",
    )
    portuguese_inputs = []
    for item in (br, pt):
        portuguese_inputs.append({
            "path": item["spec"]["dictionary"],
            "url": ROUTING_RAW_BASE + item["spec"]["dictionary"],
            "sha256": item["spec"]["dictionary_sha256"],
            "bytes": item["dictionary_bytes"],
            "license_path": item["spec"]["license_output"],
            "license_sha256": item["spec"]["license_sha256"],
            "counts": item["counts"],
        })
    portuguese_counts = {
        "accepted_keys": len(portuguese_words),
        "brazil_keys": len(br["words"]),
        "portugal_keys": len(pt["words"]),
        "shared_keys": len(br["words"] & pt["words"]),
    }
    artifacts.extend([
        (ROOT / portuguese_path, portuguese_output),
        (
            ROOT / "docs/portuguese-lexicon-provenance.json",
            report_bytes(
                "Portuguese (Brazil + Portugal)",
                portuguese_inputs,
                portuguese_path,
                portuguese_output,
                portuguese_counts,
                {
                    "purpose": "lexical pronunciation authority",
                    "pinned_repository": "https://github.com/MontrealCorpusTools/mfa-models",
                    "pinned_commit": MFA_PINNED_COMMIT,
                    "license": "CC BY 4.0",
                    "license_path": MFA_LICENSE_OUTPUT,
                    "license_sha256": MFA_LICENSE_SHA256,
                    "attribution_path": MFA_ATTRIBUTION_OUTPUT,
                    "sources": [pronunciation_source(pt_brazil), pronunciation_source(pt_portugal)],
                    "output": {
                        "path": portuguese_pronunciation_path,
                        "bytes": len(portuguese_pronunciation_output),
                        "sha256": sha256(portuguese_pronunciation_output),
                        "distinct_readings": len(portuguese_pronunciation_rows),
                    },
                },
            ),
        ),
    ])
    return artifacts


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--download", action="store_true", help="fetch the exact pinned files from GitHub")
    parser.add_argument(
        "--source-root", "--routing-source-root", dest="routing_source_root", type=Path,
        help="local checkout root of pinned wooorm/dictionaries",
    )
    parser.add_argument("--mfa-source-root", type=Path, help="local checkout root of pinned mfa-models")
    parser.add_argument("--check", action="store_true", help="compare generated bytes without writing")
    args = parser.parse_args()
    offline = args.routing_source_root is not None or args.mfa_source_root is not None
    if args.download == offline:
        parser.error("choose --download or both offline source roots")
    if offline and (args.routing_source_root is None or args.mfa_source_root is None):
        parser.error("offline mode requires both --routing-source-root and --mfa-source-root")
    try:
        publish(
            build(
                download=args.download,
                routing_source_root=args.routing_source_root,
                mfa_source_root=args.mfa_source_root,
            ),
            args.check,
        )
    except (OSError, ValueError) as error:
        print(f"Romance lexicon import failed: {error}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
