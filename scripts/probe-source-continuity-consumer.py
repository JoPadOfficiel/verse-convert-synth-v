#!/usr/bin/env python3
"""Validate FID-002 synthetic exports with the pinned local OpenUtau consumer.

Run only after the parent releases the compiler slot. Requires .NET 10; builds
an offline local probe, never downloads packages or starts OpenUtau. Inputs are
read-only. --output-dir must be new; it retains copied fixtures, build artifacts,
and bounded evidence.json (or failure.json). This is native part validation and
lyric reading, not full Ustx.Load, G2P, singer validation, or acoustic rendering.

Example, from the worktree:
  proxy python3 scripts/probe-source-continuity-consumer.py \
    --source-dir /private/tmp/verse-openutau-reverse \
    --fixtures-dir /private/tmp/fid002-native-exports \
    --output-dir /private/tmp/fid002-native-evidence
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import threading
from xml.sax.saxutils import escape

REVISION = "3f213e8993ca792c3e6f8958c92ab27eae78eac5"
CORE_HASH = "0674a9f691a23fedc8aad2debbef9e6104b6296c89c356af64fc799ca5dce1c2"
SOURCE_HASHES = {
    "UPart.cs": "b093c01aea0b0e7033c4cb5189dc353d6452b3d6c58e6b3bce1d6dda002b5ed9",
    "UTrack.cs": "df5331e959fdbea56cd76804142fa173d42fbcbadcfb8b9772ea2eda8254b994",
    "UNote.cs": "146a6aeee4d7fa53257aa46b72c81025fea4342b678a9a05c05ab21709e43af9",
    "USTx.cs": "668feed67d9b4eb821e38e79e4e49bdeace485bbeac52b6f45061bc260c06ade",
    "RenderPhrase.cs": "e505d41aaa419578d7208126bde54202873eb7432032b5234c894f86e44afb57",
    "UCurve.cs": "c032bcb2f81987cf103245ca7279945762a714cf2b13bfb992008682233c81af",
    "MusicMath.cs": "d6a904bd3a6f73c290d32625180116ceb9ca195e3c4c443984741c11727456eb",
}
BINARY_HASHES = {
    "OpenUtau.Core.dll": CORE_HASH,
    "YamlDotNet.dll": "198be37b7c05a13e3fe140ac8a2f77becb675331b51c7b0981da41c6c05d5447",
}
NAMES = tuple(f"{master}-{profile}" for master in ("pb", "chant")
              for profile in ("default", "french", "english"))
MAX_FIXTURE_BYTES = 1024 * 1024
MAX_PROCESS_BYTES = 256 * 1024
MAX_EVIDENCE_BYTES = 128 * 1024


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_new(path, data):
    """Exclusive creation also refuses existing symlinks and directories."""
    with path.open("xb") as output:
        output.write(data)


def write_json(path, value):
    data = (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    if len(data) > MAX_PROCESS_BYTES:
        raise RuntimeError("Evidence exceeds the bounded JSON report size")
    write_new(path, data)


def require_pin(path, expected):
    if not path.is_file():
        raise RuntimeError(f"Missing pinned prerequisite: {path}")
    actual = digest(path.read_bytes())
    if actual != expected:
        raise RuntimeError(f"Pin mismatch: {path.name}: {actual}")


def run_bounded(command, cwd, env, timeout):
    """Capture bounded pipes and terminate only this subprocess group on failure."""
    if os.name != "posix":
        raise RuntimeError("This local pinned macOS consumer runner requires POSIX process groups")
    buffers = [bytearray(), bytearray()]
    overflow = threading.Event()
    lock = threading.Lock()
    process = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               start_new_session=True)

    def stop():
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    def read_pipe(pipe, index):
        try:
            while True:
                chunk = pipe.read(4096)
                if not chunk:
                    return
                with lock:
                    remaining = MAX_PROCESS_BYTES - sum(map(len, buffers))
                    buffers[index].extend(chunk[:max(0, remaining)])
                    if len(chunk) > remaining:
                        overflow.set()
                        stop()
                        return
        finally:
            pipe.close()

    readers = [threading.Thread(target=read_pipe, args=(pipe, index), daemon=True)
               for index, pipe in enumerate((process.stdout, process.stderr))]
    for reader in readers:
        reader.start()
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        stop()
        process.wait(timeout=5)
        raise RuntimeError(f"Process timeout after {timeout}s: {Path(command[0]).name}") from error
    finally:
        for reader in readers:
            reader.join(timeout=2)
        if any(reader.is_alive() for reader in readers):
            stop()
    if any(reader.is_alive() for reader in readers):
        raise RuntimeError("Process pipe did not close within the bound")
    if overflow.is_set():
        raise RuntimeError("Process output exceeded 256 KiB")
    stdout, stderr = (bytes(value).decode("utf-8", errors="replace") for value in buffers)
    if process.returncode:
        raise RuntimeError(f"Process exited {process.returncode}: {(stderr or stdout)[-4096:]}")
    return stdout, stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--fixtures-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--app-dir", type=Path,
                        default=Path("/Applications/OpenUtau.app/Contents/MacOS"))
    parser.add_argument("--dotnet", default="dotnet")
    parser.add_argument("--timeout-seconds", type=int, choices=range(1, 61), default=60,
                        metavar="1..60", help="per-process deadline, at most 60 seconds")
    args = parser.parse_args()
    output = None
    try:
        app = args.app_dir.resolve(strict=True)
        source_dir = args.source_dir.resolve(strict=True)
        fixture_dir = args.fixtures_dir.resolve(strict=True)
        dotnet = shutil.which(args.dotnet)
        if dotnet is None:
            raise RuntimeError(".NET 10 executable unavailable; pass --dotnet")
        for name, expected in SOURCE_HASHES.items():
            require_pin(source_dir / name, expected)
        for name, expected in BINARY_HASHES.items():
            require_pin(app / name, expected)
        if {p.stem for p in fixture_dir.glob("*.ustx")} != set(NAMES):
            raise RuntimeError("Expected exactly six named USTX fixtures: " + ", ".join(NAMES))
        inputs = {}
        for name in NAMES:
            path = fixture_dir / f"{name}.ustx"
            if not path.is_file() or not 0 < path.stat().st_size <= MAX_FIXTURE_BYTES:
                raise RuntimeError(f"Missing or oversized synthetic fixture: {path}")
            data = path.read_bytes()
            if len(data) > MAX_FIXTURE_BYTES:
                raise RuntimeError(f"Fixture grew past size bound: {path}")
            data.decode("utf-8", errors="strict")
            inputs[name] = data
        # Exclusive directory creation; do not resolve a pre-existing symlink.
        destination = args.output_dir.absolute()
        destination.mkdir(mode=0o700, parents=False, exist_ok=False)
        output = destination
        work = output / "build"
        isolated = output / "fixtures"
        work.mkdir()
        isolated.mkdir()
        for name, data in inputs.items():
            write_new(isolated / f"{name}.ustx", data)
        harness = Path(__file__).with_name("source-continuity-consumer.cs")
        source = harness.read_bytes()
        write_new(work / "Program.cs", source)
        references = "".join(
            f'<Reference Include="{name}"><HintPath>{escape(str(app / (name + ".dll")))}</HintPath>'
            '<Private>false</Private></Reference>'
            for name in ("OpenUtau.Core", "YamlDotNet"))
        project = ('<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType>'
                   '<TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings>'
                   '<Nullable>enable</Nullable><UseSharedCompilation>false</UseSharedCompilation>'
                   '</PropertyGroup><ItemGroup>' + references + '</ItemGroup></Project>')
        write_new(work / "Probe.csproj", project.encode("utf-8"))
        write_new(work / "NuGet.Config", b'<configuration><packageSources><clear/></packageSources></configuration>')
        env = dict(os.environ, VERSE_OPENUTAU_APP_DIR=str(app),
                   DOTNET_CLI_HOME=str(output / "dotnet-home"),
                   NUGET_PACKAGES=str(output / "nuget-packages"),
                   DOTNET_CLI_TELEMETRY_OPTOUT="1", DOTNET_SKIP_FIRST_TIME_EXPERIENCE="1",
                   DOTNET_CLI_WORKLOAD_UPDATE_NOTIFY_DISABLE="1", DOTNET_NOLOGO="1",
                   MSBUILDDISABLENODEREUSE="1")
        sdk, _ = run_bounded([dotnet, "--version"], work, env, args.timeout_seconds)
        if not sdk.strip().startswith("10."):
            raise RuntimeError(f".NET 10 SDK required, found {sdk.strip()[:128]}")
        run_bounded([dotnet, "build", "Probe.csproj", "--disable-build-servers", "--nologo", "-v:q", "-m:2"],
                    work, env, args.timeout_seconds)
        stdout, _ = run_bounded([dotnet, str(work / "bin/Debug/net10.0/Probe.dll"),
                                 *(str(isolated / f"{name}.ustx") for name in NAMES)],
                                work, env, args.timeout_seconds)
        if len(stdout.encode("utf-8")) > MAX_EVIDENCE_BYTES:
            raise RuntimeError("Native evidence exceeded 128 KiB")
        evidence = json.loads(stdout)
        if (evidence.get("status") != "passed" or evidence.get("revision") != REVISION
                or evidence.get("coreSha256") != CORE_HASH or evidence.get("fixtureCount") != 6):
            raise RuntimeError("Native probe did not produce complete pinned success evidence")
        rows = evidence.get("fixtures", [])
        if len(rows) != 6 or {row.get("fixture") for row in rows} != set(NAMES):
            raise RuntimeError("Native evidence omitted or duplicated a fixture")
        for row in rows:
            if row.get("sha256") != digest(inputs[row["fixture"]]) or row.get("notes") != 21:
                raise RuntimeError("Native evidence does not match its input bytes/count")
        negatives = evidence.get("negativeControls", [])
        if len(negatives) != 6 or not all(row.get("rejected") is True for row in negatives):
            raise RuntimeError("Native negative controls did not all reject")
        for name, data in inputs.items():
            if (fixture_dir / f"{name}.ustx").read_bytes() != data or (isolated / f"{name}.ustx").read_bytes() != data:
                raise RuntimeError(f"Fixture bytes changed: {name}")
        evidence["runner"] = {"dotnetSdk": sdk.strip(), "sourceHashes": SOURCE_HASHES,
                              "binaryHashes": BINARY_HASHES, "harnessSha256": digest(source),
                              "inputDirectory": str(fixture_dir), "inputsUnchanged": True,
                              "timeoutSeconds": args.timeout_seconds, "maximumProcessBytes": MAX_PROCESS_BYTES}
        write_json(output / "evidence.json", evidence)
        print(json.dumps({"status": "passed", "fixtures": 6, "negativeControls": 6,
                          "evidence": str(output / "evidence.json")}, separators=(",", ":")))
        return 0
    except (OSError, RuntimeError, ValueError) as error:
        failure = {"schemaVersion": 1, "status": "failed", "error": str(error)[:4096]}
        if output is not None:
            write_json(output / "failure.json", failure)
        print(json.dumps(failure, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
