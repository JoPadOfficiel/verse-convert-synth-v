#!/usr/bin/env python3
"""Paired local development capture; an uncalibrated raw choice is not benefit.

Production words/baseline come from the Rust pronunciation_evaluation example.
Optional gold is fixed externally by source hash + stable member IDs. Files,
receipts and copyright-bearing text stay local. No upstream downloading occurs.
"""
import argparse
from collections import Counter
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import shutil
import socket
import subprocess
import sys
import time

POLICY = "verse-pronunciation-v1"
PROMPT = "Which language should own the pronunciation of the marked complete source word in this lyric context?"
DESCRIPTIONS = ["French pronunciation", "English pronunciation", "Spanish pronunciation", "Portuguese pronunciation"]
LABELS = ["fr", "en", "es", "pt"]


OWNER_FIELDS = ("track", "part", "staff", "voice", "occurrence", "segment", "lane", "verse")

def owner_key(owner):
    if not isinstance(owner, dict) or any(key not in owner for key in OWNER_FIELDS):
        raise ValueError("PRONUNCIATION_OWNER_REQUIRED")
    return tuple(owner[key] for key in OWNER_FIELDS)


def eligible_development_oracle(records):
    """Exact existing editorial lyric-record oracles, NOT complete-word gold."""
    white = "66ad238c35ed838e95b9eebb7bc421168539a600eaab055b69b234663d867fff"
    rock = "186f0c1f91a138aef4317fe9f3dfe1473e5773479a36657bf9b637d2be8ad6fb"

    def rock_label(note):
        staff, measure, raw = note["staff"], note["measure"], note["raw"].strip().lower()
        ranges = {"1": [(25, 30), (47, 55), (72, 75), (119, 122)], "2": [(23, 30), (47, 55), (71, 75), (93, 99), (121, 122)], "3": [(25, 30), (47, 55), (72, 75), (94, 100)]}
        singles = {"1": {93, 95, 97, 99}, "2": set(), "3": {118, 120}}
        if staff not in ranges or measure is None:
            raise ValueError("PRONUNCIATION_ORACLE_SOURCE_OWNER")
        english = measure in singles[staff] or any(a <= measure <= b for a, b in ranges[staff])
        if staff == "1":
            english |= (measure == 36 and raw.startswith("ex")) or (measure == 37 and raw in {"me", "sir", "big"}) or (measure == 38 and raw == "ben") or (measure == 40 and raw.startswith(("lon", "beat"))) or (measure == 42 and raw.startswith("beat")) or (measure == 46 and raw in {"it's", "been", "a"}) or (measure == 60 and raw.startswith("bet")) or (measure == 67 and raw == "beach") or (measure == 68 and raw == "boys") or (measure in {105, 108} and raw.startswith("jim")) or (measure == 110 and raw == "stones")
        elif staff == "2":
            english |= (measure == 34 and raw.startswith("li")) or (measure in {41, 43} and raw.startswith("beat")) or (measure == 46 and raw in {"it's", "been", "a"}) or (measure == 66 and raw == "beach") or (measure == 67 and raw == "boys")
        else:
            english |= (measure in {41, 43} and raw.startswith("beat")) or (measure == 66 and raw == "beach") or (measure == 67 and raw == "boys") or (measure == 117 and raw in {"i", "can't"})
        return "en" if english else "fr"

    result = []
    for source in records:
        sha = source["source_sha256"]
        if sha not in {white, rock}:
            continue
        proposals = {}
        for word in source.get("words", []):
            for member in word["source_word"]["members"]:
                identity = (owner_key(word["source_word"]["owner"]), member)
                if identity in proposals:
                    raise ValueError("PRONUNCIATION_ORACLE_MEMBER_AMBIGUOUS")
                proposals[identity] = word.get("raw_laya_choice")
        counts = Counter()
        fixes, breaks, uncovered = [], [], []
        for note in source["notes"]:
            if note["continues"] or not note["raw"] or not any(c.isalpha() for c in note["raw"]):
                continue
            if sha == white:
                if note["verse"] not in {1, 2}:
                    raise ValueError("PRONUNCIATION_ORACLE_SOURCE_VERSE")
                expected = "en" if note["verse"] == 1 else "fr"
            else:
                expected = rock_label(note)
            identity = (owner_key(note["owner"]), note["id"])
            if note["owner"]["verse"] != note["verse"] or note["owner"]["staff"] != note.get("staff"):
                raise ValueError("PRONUNCIATION_ORACLE_SOURCE_OWNER")
            baseline = note["baseline_owner"] == expected
            covered = proposals.get(identity) in LABELS
            raw = covered and proposals.get(identity) == expected
            counts["eligible_lyric_records"] += 1
            counts["baseline_correct"] += baseline
            counts["raw_laya_correct"] += raw
            counts["raw_covered_predictions"] += covered
            counts["raw_wrong_covered_predictions"] += covered and not raw
            counts["raw_uncovered_records"] += not covered
            counts["rejected_hybrid_correct"] += note["hybrid_owner"] == expected
            if raw and not baseline:
                fixes.append(note["id"])
            if baseline and covered and not raw:
                breaks.append(note["id"])
            if not covered:
                uncovered.append(note["id"])
        counts["raw_full_denominator_failures_including_noncoverage"] = counts["eligible_lyric_records"] - counts["raw_laya_correct"]
        assert counts["raw_laya_correct"] + counts["raw_wrong_covered_predictions"] + counts["raw_uncovered_records"] == counts["eligible_lyric_records"]
        result.append({"source_sha256": sha, "partition": "exposed_development", "metric": "eligible_lyric_records_including_unresolved_fragments", "counts": dict(counts), "raw_fixes": fixes, "raw_breaks_covered_predictions_only": breaks, "raw_uncovered_records": uncovered})
    return result


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def write_json(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2, allow_nan=False)
        stream.write("\n")


def metrics(records, gold):
    """Whole gold denominator retained; wrong/missing membership is an error."""
    if not gold:
        return {"status": "unqualified", "reason": "Independent frozen gold absent", "reconstruction": None, "language": None, "phones": None, "listening": None}
    if gold.get("schema_version") != 1 or gold.get("format") != "verse.pronunciation-gold":
        raise ValueError("PRONUNCIATION_GOLD_SCHEMA")
    families = {}
    counts = Counter()
    fixes, breaks = [], []
    predicted = {}
    for source in records:
        for item in source.get("words", []):
            word = item["source_word"]
            identity = (source["source_sha256"], owner_key(word["owner"]), tuple(word["members"]))
            if identity in predicted:
                raise ValueError("PRONUNCIATION_GOLD_AMBIGUOUS: duplicate membership")
            predicted[identity] = item
    seen = set()
    for entry in gold["records"]:
        family, split = entry["song_family_id"], entry["split"]
        if family in families and families[family] != split:
            raise ValueError("PRONUNCIATION_GOLD_LEAKAGE: family appears in multiple splits")
        families[family] = split
        identity = (entry["source_sha256"], owner_key(entry["owner"]), tuple(entry["members"]))
        if identity in seen:
            raise ValueError("PRONUNCIATION_GOLD_DUPLICATE")
        seen.add(identity)
        item = predicted.get(identity)
        counts["denominator"] += 1
        if not item:
            counts["missing_membership"] += 1
            continue
        reconstruction = item["source_word"]["key"] in entry["accepted_keys"]
        counts["reconstruction_correct"] += reconstruction
        baseline = reconstruction and item["baseline_owner"] in entry["accepted_languages"]
        raw = reconstruction and item.get("raw_laya_choice") in entry["accepted_languages"]
        counts["baseline_correct"] += baseline
        counts["raw_laya_correct"] += raw
        counts["hybrid_correct"] += reconstruction and item.get("hybrid_owner") in entry["accepted_languages"]
        if raw and not baseline:
            fixes.append(entry["id"])
        if baseline and not raw:
            breaks.append(entry["id"])
    return {"status": "development" if set(families.values()) != {"test"} else "test_without_activation_receipt", "counts": dict(counts), "raw_fixes": fixes, "raw_breaks": breaks,
            "phones": None, "listening": None, "activation_qualified": False,
            "missing_gates": ["calibrated candidate", "native parity", "independent phone and listening gold", "song-family confidence intervals"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--sources", type=Path, nargs="+")
    selection.add_argument("--corpus-manifest", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-assets", type=Path)
    parser.add_argument("--upstream", type=Path)
    parser.add_argument("--gold", type=Path)
    parser.add_argument("--candidate-root", type=Path)
    parser.add_argument("--candidate-manifest-sha256")
    args = parser.parse_args()
    args.output.mkdir(exist_ok=False)
    aliases = {}
    companions = Counter()
    if args.corpus_manifest:
        manifest = json.loads(args.corpus_manifest.read_text())
        root = Path(manifest["root"]).resolve(strict=True)
        for entry in manifest["files"]:
            source = root / entry["path"]
            if digest(source) != entry["sha256"]:
                raise ValueError("PRONUNCIATION_SOURCE_IDENTITY: initial corpus inventory differs")
            if source.suffix.lower() in {".mid", ".midi", ".kar", ".xml", ".musicxml", ".mxl", ".mscx", ".mscz"}:
                aliases.setdefault(entry["sha256"], []).append(source)
            else:
                companions[source.suffix.lower() or "metadata"] += 1
    else:
        for source in args.sources:
            source = source.resolve(strict=True)
            aliases.setdefault(digest(source), []).append(source)
    priority = ["66ad238c35ed838e95b9eebb7bc421168539a600eaab055b69b234663d867fff", "186f0c1f91a138aef4317fe9f3dfe1473e5773479a36657bf9b637d2be8ad6fb", "851e91e0ddfe61d13160c8c20ca360e33cd70925026628be6a00008e37cf0be5"]
    identities = sorted(aliases, key=lambda sha: (priority.index(sha) if sha in priority else len(priority), sha))
    sources = [(min(aliases[sha], key=lambda p: (".versebundle" in str(p), len(str(p)), str(p))), sha) for sha in identities]
    captured = subprocess.run([str(args.probe.resolve(strict=True))], input="".join(json.dumps({"path": str(p), "candidate_root": str(args.candidate_root.resolve()) if args.candidate_root else None, "candidate_manifest_sha256": args.candidate_manifest_sha256}) + "\n" for p, _ in sources), text=True, capture_output=True, check=True, timeout=600)
    records = [json.loads(line) for line in captured.stdout.splitlines()]
    if len(records) != len(sources) or any(r["source_sha256"] != sha for r, (_, sha) in zip(records, sources)):
        raise ValueError("PRONUNCIATION_SOURCE_IDENTITY")
    receipt = {"policy": POLICY, "probe_sha256": digest(args.probe), "git_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(), "script_sha256": digest(Path(__file__)), "memory_enabled": False, "calibrated_for_verse": False, "activation_qualified": False, "network_attempts": [], "sources": [sha for _, sha in sources]}
    receipt["input_entries"] = sum(map(len, aliases.values()))
    receipt["byte_unique_inputs"] = len(sources)
    receipt["source_aliases"] = [{"source_sha256": sha, "canonical_path": str(path), "aliases": [str(p) for p in aliases[sha]], "song_family_status": "not_independent_test_material"} for path, sha in sources]
    receipt["input_format_counts"] = dict(Counter(p.suffix.lower() for paths in aliases.values() for p in paths))
    receipt["companions"] = dict(companions)
    receipt["source_refusals"] = [{"source_sha256": r["source_sha256"], "stage": r.get("stage"), "code": r.get("diagnostic_code"), "diagnostic": r.get("diagnostic")} for r in records if not r["ok"]]
    receipt["lyric_free_controls"] = [r["source_sha256"] for r in records if r["ok"] and not r.get("words")]
    if args.source_assets:
        if not args.upstream:
            raise ValueError("Pinned upstream required for local Laya")
        from importlib.machinery import SourceFileLoader
        preparation = SourceFileLoader("verse_laya_preparation", str(Path(__file__).with_name("prepare-laya-assets.py"))).load_module()
        if subprocess.check_output(["git", "-C", str(args.upstream), "rev-parse", "HEAD"], text=True).strip() != preparation.REVISION:
            raise ValueError("LAYA_UPSTREAM_IDENTITY")
        for name, expected in preparation.HASHES.items():
            if digest(args.source_assets / name) != expected:
                raise ValueError(f"LAYA_SOURCE_INTEGRITY: {name}")
        working = args.output / "loader-working-copy"
        def prepare_copy(src, dst):
            # Only the small tokenizer configuration is rewritten by the pinned
            # loader. Keep large immutable data in the canonical directory.
            if Path(src).name in {"model.safetensors", "tokenizer.json"}:
                os.symlink(Path(src).resolve(), dst)
                return dst
            return shutil.copy2(src, dst)
        shutil.copytree(args.source_assets, working, copy_function=prepare_copy)
        for key in ["HF_HUB_OFFLINE", "TRANSFORMERS_OFFLINE", "HF_HUB_DISABLE_TELEMETRY", "DO_NOT_TRACK"]:
            os.environ[key] = "1"
        os.environ["TOKENIZERS_PARALLELISM"] = "false"

        def deny_network(*_args, **_kwargs):
            receipt["network_attempts"].append("socket connection attempted")
            raise RuntimeError("LAYA_OFFLINE")

        socket.create_connection = deny_network
        socket.socket.connect = deny_network
        socket.socket.connect_ex = deny_network
        sys.path.insert(0, str(args.upstream.resolve()))
        import torch
        from laya.agent import Agent
        from laya.common import build_sequence
        torch.set_num_threads(2)
        torch.set_num_interop_threads(1)
        start = time.perf_counter()
        agent = Agent(str(working.resolve()), device="cpu", backend="eager", expected_sha256=preparation.HASHES)
        receipt["load_seconds"] = time.perf_counter() - start
        question = {"language": {"type": "choice", "instructions": PROMPT, "criteria": dict(zip(LABELS, DESCRIPTIONS))}}
        normalized = {"t": "choice", "ins": PROMPT, "crit": dict(zip(LABELS, DESCRIPTIONS))}
        latencies = []
        for record in records:
            words = record.get("words", [])
            print(json.dumps({"source_sha256": record["source_sha256"], "complete_words": len(words)}), flush=True)
            for offset in range(0, len(words), 2):
                batch = words[offset:offset + 2]
                contexts = [w["context"] for w in batch]
                begin = time.perf_counter()
                answers = agent.predict_batch(contexts, question, batch_size=2)
                latencies.append(time.perf_counter() - begin)
                for word, answer in zip(batch, answers):
                    value = answer["answers"]["language"]
                    probs = value["probabilities"]
                    if list(probs) != LABELS or any(type(p) not in (int, float) or not math.isfinite(p) or not 0 <= p <= 1 for p in probs.values()):
                        raise ValueError("LAYA_CONFIDENCE_INVALID")
                    ids, markers = build_sequence(agent.tok, word["context"], normalized, 1024, 256)
                    word.update(raw_laya_choice=value["choice"], raw_probabilities=probs, raw_answer_confidence=value["answer_confidence"], calibrated_gate="unevaluated", input_ids=ids, marker_pos=markers)
                    if answer["usage"].get("truncated"):
                        word["raw_laya_choice"] = None
                        word["rejection_reason"] = "LAYA_TRUNCATED"
        receipt["inference_seconds"] = sum(latencies)
        receipt["batch_latency_p50_seconds"] = sorted(latencies)[len(latencies)//2] if latencies else None
        receipt["batch_latency_p95_seconds"] = sorted(latencies)[min(len(latencies)-1, int(len(latencies)*.95))] if latencies else None
        receipt["raw_choices"] = dict(Counter(w.get("raw_laya_choice") for r in records for w in r.get("words", [])))
        receipt["model_sha256"] = preparation.HASHES["model.safetensors"]
        receipt["prepared_config_sha256"] = digest(working / "tokenizer/tokenizer_config.json")
        receipt["peak_rss_host_units"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        for name, expected in preparation.HASHES.items():
            if digest(args.source_assets / name) != expected:
                raise ValueError("Canonical source changed")
    gold = json.loads(args.gold.read_text()) if args.gold else None
    receipt["metrics"] = metrics(records, gold)
    receipt["existing_editorial_record_oracles"] = eligible_development_oracle(records)
    receipt["candidate_gates"] = dict(Counter(r.get("candidate_gate", "source_ineligible") for r in records))
    receipt["laya_routed_ablation_gate"] = "unavailable:not_qualified"
    receipt["hybrid_benefit_executed"] = bool(records) and all(r.get("candidate_gate") == "calibrated_development_candidate" for r in records)
    receipt["gold_sha256"] = digest(args.gold) if args.gold else None
    for path, sha in sources:
        if digest(path) != sha:
            raise ValueError("User source changed")
    for sha, paths in aliases.items():
        if any(digest(path) != sha for path in paths):
            raise ValueError("A supplied alias changed")
    write_json(args.output / "paired-results.json", records)
    write_json(args.output / "evaluation-receipt.json", receipt)
    print(json.dumps(receipt), flush=True)


if __name__ == "__main__":
    main()
