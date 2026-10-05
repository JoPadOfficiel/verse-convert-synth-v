#!/usr/bin/env python3
"""Offline development preparation. Never author calibration or clear rights gates.

Run with the pinned development Python environment, outside the installed app.
The output directory must be new. Canonical checkpoint files are never passed
to an upstream loader because that loader rewrites tokenizer_config.json.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import resource
import shutil
import socket
import subprocess
import sys
import time

REVISION = "8a6e1328cce2460a0e5aa348ad465bb1b5821cd2"
MODEL_REVISION = "1720e3e3357cfe1e281542e223f8273b0890ca34"
EXPORTER_SHA = "5e95a5dcfa32d3dcba63ddf9411a9d8a1f9773a58f1bf8507c642992166ac323"
POLICY = "verse-pronunciation-v1"
PROMPT = "Which language should own the pronunciation of the marked complete source word in this lyric context?"
OPTIONS = ["French pronunciation", "English pronunciation", "Spanish pronunciation", "Portuguese pronunciation"]
HASHES = {
    "model.safetensors": "9d628fd971b700382ac6f65920a86f149777b2e748e0c955fb3b19695aa8f204",
    "tokenizer/tokenizer.json": "609d8f4c067cd3950f88594c5a802616cea245823836ef5848ee4fc40aab5b6f",
    "rl_agent_config.json": "25061739243b617ad88d1219ba6f8a9c86c5881ca28df024fa2d9b3b2fcc30c6",
    "encoder/config.json": "83f6916d13ef0f556ac461f28308dc2bffa7ebeadee8ec9e2db5812020ea5bb4",
    "tokenizer/tokenizer_config.json": "424b69444bf7b5809dc2cd2e36d0bd71b8055124dd24274d6db3c655d38205e7",
}


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--upstream", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--export", action="store_true")
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    upstream = args.upstream.resolve(strict=True)
    exporter = upstream / "laya-dotnet/tools/export_onnx.py"
    revision = subprocess.check_output(["git", "-C", str(upstream), "rev-parse", "HEAD"], text=True).strip()
    if revision != REVISION or digest(exporter) != EXPORTER_SHA:
        raise ValueError("LAYA_EXPORTER_IDENTITY: pinned exporter differs")
    for name, expected in HASHES.items():
        path = source / name
        if path.is_symlink() or not path.is_file() or digest(path) != expected:
            raise ValueError(f"LAYA_SOURCE_INTEGRITY: {name}")
    output = args.output.absolute()
    if output == source or source in output.parents:
        raise ValueError("LAYA_SOURCE_IMMUTABLE: output must be outside canonical source")
    output.mkdir(parents=False, exist_ok=False)
    work = output / "working-copy"
    shutil.copytree(source, work)
    receipt = {
        "schema_version": 1, "policy": POLICY, "upstream_revision": REVISION,
        "model_revision": MODEL_REVISION, "source_assets": HASHES,
        "exporter_sha256": EXPORTER_SHA, "redistribution_qualified": False,
        "tokenizer_rights": "unresolved Gemma tokenizer ancestry; no redistribution",
        "calibration_qualified": False, "independent_benefit_qualified": False,
        "native_parity_qualified": False, "network_attempts": [],
    }
    start = time.perf_counter()
    if args.export:
        for key in ["HF_HUB_OFFLINE", "TRANSFORMERS_OFFLINE", "HF_HUB_DISABLE_TELEMETRY", "DO_NOT_TRACK"]:
            os.environ[key] = "1"
        os.environ["TOKENIZERS_PARALLELISM"] = "false"

        def deny_network(*_args, **_kwargs):
            receipt["network_attempts"].append("socket connection attempted")
            raise RuntimeError("LAYA_OFFLINE: network is prohibited during preparation")

        socket.create_connection = deny_network
        socket.socket.connect = deny_network
        socket.socket.connect_ex = deny_network
        sys.path.insert(0, str(upstream))
        spec = importlib.util.spec_from_file_location("pinned_laya_export", exporter)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        import torch
        torch.set_num_threads(2)
        torch.set_num_interop_threads(1)
        model_dir = Path(module.export_one(str(work), str(output / "export"), None, 18, False))
        import onnx
        model = onnx.load(str(model_dir / "model.onnx"), load_external_data=False)
        external = {entry.value for tensor in model.graph.initializer for entry in tensor.external_data if entry.key == "location"}
        if external != {"model.onnx.data"}:
            raise ValueError("LAYA_GRAPH_EXTERNAL_DATA: unexpected external paths")
        receipt["graph"] = {"ir_version": model.ir_version, "opsets": {op.domain: op.version for op in model.opset_import}}
        receipt["prepared_assets"] = [{"path": str(p.relative_to(model_dir)), "bytes": p.stat().st_size, "sha256": digest(p)} for p in sorted(model_dir.rglob("*")) if p.is_file()]
        # Reference fixtures use the SAME local weights, not cache-compatible ones.
        from laya.common import build_model, build_sequence, collate_items
        from safetensors.torch import load_file
        from transformers import AutoTokenizer
        config = json.loads((work / "rl_agent_config.json").read_text())
        reference = build_model(config, encoder_dir=str(work / "encoder"))
        reference.load_state_dict(load_file(str(work / "model.safetensors")), strict=True)
        reference.encoder.config._attn_implementation = "eager"
        reference.encoder.config.reference_compile = False
        reference.eval()
        tokenizer = AutoTokenizer.from_pretrained(str(work / "tokenizer"), local_files_only=True)
        question = {"t": "choice", "ins": PROMPT, "crit": dict(zip(["fr", "en", "es", "pt"], OPTIONS))}
        states = ["<target>ciel</target>", "<target>hello</target>", "<target>hola</target>", "<target>obrigado</target>", "bonjour <target>ami</target> ici", "hello <target>friend</target> again"]
        fixtures = []
        for state in states:
            ids, markers = build_sequence(tokenizer, state, question, 1024, 256)
            batch = collate_items([[{"ids": ids, "markers": markers, "qtype": 0}]], 0)
            with torch.no_grad():
                logits, action = reference(**{k: batch[k] for k in ["input_ids", "attention_mask", "marker_pos", "marker_mask", "qtype"]})
            fixtures.append({"state": state, "input_ids": ids, "marker_pos": markers, "logits": logits.tolist(), "act_logits": action.tolist()})
        write_json(model_dir / "verse-parity-fixtures.json", {"policy": POLICY, "source_model_sha256": HASHES["model.safetensors"], "cases": fixtures})
        receipt["fixture_scope"] = "Authored preprocessing/reference parity only; no accuracy or calibrated benefit"
    receipt["prepared_tokenizer_config_sha256"] = digest(work / "tokenizer/tokenizer_config.json")
    for name, expected in HASHES.items():
        if digest(source / name) != expected:
            raise ValueError("LAYA_SOURCE_IMMUTABLE: canonical source changed")
    receipt["elapsed_seconds"] = time.perf_counter() - start
    receipt["peak_rss_host_units"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    receipt["python"] = sys.version
    write_json(output / "preparation-receipt.json", receipt)
    print(json.dumps({"output": str(output), "exported": args.export, "activation_qualified": False}))


if __name__ == "__main__":
    main()
