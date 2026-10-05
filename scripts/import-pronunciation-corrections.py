#!/usr/bin/env python3
"""Validate exchange using Rust's authoritative schema; emit review candidates.

Never modifies production resources, an acoustic model, source scores or memory.
The patch contains scoped regression candidate data, not an approved global fix.
"""
import argparse
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--validator", type=Path)
    args = parser.parse_args()
    project = Path(__file__).resolve().parent.parent
    command = [str(args.validator.resolve()), str(args.package.resolve())] if args.validator else ["cargo", "run", "--offline", "--locked", "--quiet", "--manifest-path", str(project / "src-tauri/Cargo.toml"), "--example", "pronunciation_corrections", "--", str(args.package.resolve())]
    result = subprocess.run(command, capture_output=True, text=True, timeout=600, check=True)
    package = json.loads(result.stdout)
    args.output.mkdir(exist_ok=False)
    candidates = []
    for correction in package["corrections"]:
        voice_specific = correction["voice"] is not None or correction["after"]["phones"] is not None
        kind = "voice_scoped_reading" if voice_specific else "language_context"
        target = "src-tauri/src/engine/language.rs" if kind == "language_context" else f"src-tauri/src/engine/target/{ {'fr': 'french', 'en': 'english', 'es': 'spanish', 'pt': 'portuguese'}[correction['after']['language']] }.rs"
        candidates.append({"kind": kind, "responsible_resource": target, "correction": correction,
                           "promotion_qualified": False, "required_checks": ["independent linguistic evidence", "source-bound context regression", "native duration/acoustic inventories", "blinded listening where acoustic benefit is claimed"]})
    data = json.dumps({"format": "verse.pronunciation-fix-candidates", "schema_version": 1, "candidates": candidates}, ensure_ascii=False, indent=2) + "\n"
    with (args.output / "fixture-candidates.json").open("x", encoding="utf-8") as stream:
        stream.write(data)
    filename = "src-tauri/tests/fixtures/pronunciation-correction-candidates.json"
    lines = data.splitlines()
    patch = f"diff --git a/{filename} b/{filename}\nnew file mode 100644\n--- /dev/null\n+++ b/{filename}\n@@ -0,0 +1,{len(lines)} @@\n" + "".join("+" + line + "\n" for line in lines)
    with (args.output / "review-candidate.patch").open("x", encoding="utf-8") as stream:
        stream.write(patch)
    print(json.dumps({"validated_records": len(candidates), "patch": str(args.output / "review-candidate.patch"), "promotion_qualified": False}))


if __name__ == "__main__":
    main()
