#!/usr/bin/env python3
"""EXP-002: exercise pinned native OpenUtau curve consumers, without a singer.

Needs .NET 10, the audited app, and previously fetched upstream source files.
Does not download dependencies or initialize the OpenUtau application.
"""
import argparse
import hashlib
import json
import math
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
from xml.sax.saxutils import escape

HASHES = {
    "RenderPhrase.cs": "e505d41aaa419578d7208126bde54202873eb7432032b5234c894f86e44afb57",
    "UCurve.cs": "c032bcb2f81987cf103245ca7279945762a714cf2b13bfb992008682233c81af",
    "UNote.cs": "146a6aeee4d7fa53257aa46b72c81025fea4342b678a9a05c05ab21709e43af9",
    "USTx.cs": "668feed67d9b4eb821e38e79e4e49bdeace485bbeac52b6f45061bc260c06ade",
    "MusicMath.cs": "d6a904bd3a6f73c290d32625180116ceb9ca195e3c4c443984741c11727456eb",
}


MAX_FIXTURE_BYTES = 32 * 1024 * 1024
MAX_ORACLE_SAMPLES = 2_000_001


def bounded_read(path):
    with path.open("rb") as stream:
        data = stream.read(MAX_FIXTURE_BYTES + 1)
    if len(data) > MAX_FIXTURE_BYTES:
        raise ValueError(f"Native fixture exceeds bounded storage: {path}")
    return data


def needs_floor(gain):
    """Pinned DYN rounding: nearest, ties away from zero; -240 is mute."""
    if gain <= 0:
        return False
    value = 200 * math.log10(gain)
    rounded = math.ceil(value - 0.5) if value < 0 else math.floor(value + 0.5)
    return rounded <= -240


def oracle_geometry(original):
    """Preflight the unmodified authored oracle before allocating mutations."""
    gains = original["gains"]
    start, end = original["startTick"], original["endTick"]
    if (original["schema"] != 1 or original["ticksPerQuarter"] != 480
            or original["tickStep"] != 1 or not isinstance(start, int)
            or not isinstance(end, int) or not 0 <= start < end <= 2_147_483_647):
        raise ValueError("Score oracle domain mismatch")
    if len(gains) != end - start + 1 or len(gains) > MAX_ORACLE_SAMPLES:
        raise ValueError("Score oracle must cover every tick within bounded storage")
    if any(not isinstance(g, (int, float)) or not math.isfinite(g) or g < 0 for g in gains):
        raise ValueError("Score oracle invalid gain")
    fades = original["nienteFadeIntervals"]
    if len(fades) > len(gains):
        raise ValueError("Score oracle invalid niente interval")
    intervals = []
    for fade in fades:
        a, b, direction = fade["startTick"], fade["endTick"], fade["direction"]
        if (not isinstance(a, int) or not isinstance(b, int) or not start <= a < b <= end
                or direction not in ("in", "out")):
            raise ValueError("Score oracle invalid niente interval")
        from_gain, to_gain = gains[a - start], gains[b - start]
        if ((direction == "out" and (from_gain <= 0 or to_gain != 0))
                or (direction == "in" and (from_gain != 0 or to_gain <= 0))):
            raise ValueError("Score oracle invalid niente endpoint")
        intervals.append((a, b))
    ordered = sorted(intervals)
    if any(right[0] < left[1] for left, right in zip(ordered, ordered[1:])):
        raise ValueError("Score oracle invalid niente interval")
    # Reject overlap before scanning interiors so total visits stay domain-bounded.
    for a, b in ordered:
        if any(gains[t - start] <= 0 for t in range(a + 1, b)):
            raise ValueError("Score oracle invalid niente interior")
    floor_ticks = []
    interval_index = 0
    for offset, gain in enumerate(gains):
        tick = start + offset
        while interval_index < len(ordered) and ordered[interval_index][1] <= tick:
            interval_index += 1
        if needs_floor(gain):
            if interval_index == len(ordered) or not ordered[interval_index][0] < tick < ordered[interval_index][1]:
                raise ValueError("Score gain floor outside authored niente interval")
            floor_ticks.append(tick)
    return floor_ticks


def score_oracle_controls(original):
    """Floor-free fades remain valid without floor authorization."""
    if not oracle_geometry(original) and original["nienteFadeIntervals"]:
        yield "floor-free-without-authorization", json.dumps(
            dict(original, nienteFadeIntervals=[]), allow_nan=False)


def score_oracle_mutations(original):
    """Mutate only the independent oracle; keep the native fixture unchanged."""
    floor_ticks = oracle_geometry(original)

    def changed(**fields):
        return json.dumps(dict(original, **fields), allow_nan=False)

    yield "empty-oracle", changed(gains=[]), "Score oracle must cover every tick"
    yield "truncated-oracle", changed(gains=original["gains"][:-1]), "Score oracle must cover every tick"
    yield "oversized-oracle", changed(gains=original["gains"] + [original["gains"][-1]]), "Score oracle must cover every tick"
    yield "wrong-end", changed(endTick=original["endTick"] - 1), "Score oracle domain mismatch"
    yield "wrong-start", changed(startTick=original["startTick"] + 1), "Score oracle domain mismatch"
    yield "wrong-step", changed(tickStep=2), "Score oracle domain mismatch"
    yield "wrong-timebase", changed(ticksPerQuarter=960), "Score oracle domain mismatch"
    # Valid JSON numeric overflow exercises GetDouble's nonfinite result rather
    # than a JSON syntax error. The rest of the full-domain oracle is unchanged.
    overflow = dict(original, gains=["OVERFLOW"] + original["gains"][1:])
    yield "nonfinite-gain", json.dumps(overflow).replace('"OVERFLOW"', '1e9999'), "Score oracle invalid gain"
    yield "negative-gain", changed(gains=[-1] + original["gains"][1:]), "Score oracle invalid gain"
    ordinary = dict(original, gains=[1e-4] * len(original["gains"]), nienteFadeIntervals=[])
    yield "ordinary-small-gain", json.dumps(ordinary), "Score gain floor outside authored niente interval"
    ordinary["nienteFadeIntervals"] = [{"startTick": original["startTick"],
                                         "endTick": original["endTick"], "direction": "out"}]
    yield "forged-fade-floor", json.dumps(ordinary), "Score oracle invalid niente endpoint"
    changed_endpoint = original["gains"][:-1] + [0 if original["gains"][-1] > 0 else 1]
    # Niente endpoints are authenticated before native sample comparison.
    # A fade ending at the domain end fails there, regardless of mute direction.
    final_is_fade_endpoint = any(fade["endTick"] == original["endTick"]
                                 for fade in original["nienteFadeIntervals"])
    endpoint_reason = ("Score oracle invalid niente endpoint" if final_is_fade_endpoint else
                       "Score mute endpoint mismatch" if original["gains"][-1] > 0 else
                       "Positive score gain became mute")
    yield "wrong-final-value", changed(gains=changed_endpoint), endpoint_reason
    if floor_ticks:
        yield "missing-fade-authorization", changed(nienteFadeIntervals=[]), "Score gain floor outside authored niente interval"


def score_target_mutations(original, oracle):
    """Change actual saved DYN floors while keeping the authored oracle intact."""
    if not oracle_geometry(oracle):
        return
    # Only the pinned emitter's DYN value arrays are eligible. Do not rewrite
    # times, pitches, unrelated curves or the independent source expectations.
    pattern = r'(      - abbr: "dyn"\n        xs: \[[^\n]*\]\n        ys: \[)([^\n]*)(\])'
    changed = 0

    def replace(match):
        nonlocal changed
        xs = json.loads(re.search(r'xs: (\[[^\n]*\])', match[1])[1])
        values = json.loads('[' + match[2] + ']')
        if len(xs) != len(values):
            raise ValueError("Unpaired target curve arrays")
        for index, tick in enumerate(xs):
            offset = tick - oracle["startTick"]
            if (values[index] == -239 and 0 <= offset < len(oracle["gains"])
                    and needs_floor(oracle["gains"][offset])):
                values[index] = -238
                changed += 1
        return match[1] + ', '.join(str(value) for value in values) + match[3]

    mutated = re.sub(pattern, replace, original)
    if not changed:
        raise ValueError("Authored floor has no emitted DYN -239 to mutate")
    yield "wrong-target-floor", mutated, "Score authorized floor must equal DYN -239"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--fixtures-dir", type=Path, required=True)
    parser.add_argument("--score-fixtures-dir", type=Path,
                        help="Optional EXP-003 score-*.ustx files with independent .expected.json oracles")
    parser.add_argument("--app-dir", type=Path, default=Path("/Applications/OpenUtau.app/Contents/MacOS"))
    parser.add_argument("--dotnet", default="dotnet")
    args = parser.parse_args()
    for name, expected in HASHES.items():
        actual = hashlib.sha256((args.source_dir / name).read_bytes()).hexdigest()
        if actual != expected:
            raise SystemExit(f"Source pin mismatch: {name}: {actual}")
    fixtures = [args.fixtures_dir.resolve() / f"{name}.ustx"
                for name in ("pitch", "gain", "pulse", "tempo-rest", "default")]
    for fixture in fixtures:
        if not fixture.is_file():
            raise SystemExit(f"Missing fixture: {fixture}; run expression_fidelity with VERSE_PERFORMANCE_PROBE_DIR")
    score_fixtures = sorted(args.score_fixtures_dir.resolve().glob("score-*.ustx")) if args.score_fixtures_dir else []
    if args.score_fixtures_dir and not score_fixtures:
        raise SystemExit("No EXP-003 score fixtures found")
    for fixture in score_fixtures:
        if not fixture.with_suffix(".expected.json").is_file():
            raise SystemExit(f"Missing independent score oracle: {fixture}")
    source = (args.source_dir / "RenderPhrase.cs").read_text()
    # Execute the exact upstream base/vibrato/pitch-point code using native
    # types. Only the surrounding method's parameter/local declarations differ.
    body = source.split("const int pitchInterval = 5;", 1)[1].split("// Mod plus", 1)[0]
    with tempfile.TemporaryDirectory(prefix="verse-exp002-consumer-") as directory:
        work = Path(directory)
        shutil.copyfile(Path(__file__).with_name("midi-performance-consumer.cs"), work / "Program.cs")
        (work / "PitchConsumer.cs").write_text(
            "using OpenUtau.Core; using OpenUtau.Core.Ustx; using OpenUtau.Core.Util;\n"
            "static class PitchConsumer { public static float[] Sample(UProject project, "
            "UVoicePart part, int position, int leading, int end) {\n"
            "var timeAxis=project.timeAxis; var uNotes=part.notes.ToList(); "
            "float[] pitches; const int pitchInterval=5;\n" + body + "\nreturn pitches; }}\n")
        app = args.app_dir.resolve()
        references = "".join(f'<Reference Include="{name}"><HintPath>{escape(str(app / (name + ".dll")))}</HintPath><Private>false</Private></Reference>'
                             for name in ("OpenUtau.Core", "YamlDotNet"))
        (work / "Probe.csproj").write_text(
            '<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType>'
            '<TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings>'
            '</PropertyGroup><ItemGroup>' + references + '</ItemGroup></Project>')
        (work / "NuGet.Config").write_text('<configuration><packageSources><clear/></packageSources></configuration>')
        env = dict(os.environ, VERSE_OPENUTAU_APP_DIR=str(app))
        subprocess.run([args.dotnet, "build", "Probe.csproj", "--nologo", "-v:q"], cwd=work, env=env, check=True, timeout=60)
        isolated = work / "fixtures"
        isolated.mkdir()
        for fixture in fixtures:
            (isolated / fixture.name).write_bytes(bounded_read(fixture))
        for fixture in score_fixtures:
            (isolated / fixture.name).write_bytes(bounded_read(fixture))
            oracle = fixture.with_suffix(".expected.json")
            original_oracle = bounded_read(oracle)
            oracle_geometry(json.loads(original_oracle))
            (isolated / oracle.name).write_bytes(original_oracle)
        fixture_args = [str(isolated / fixture.name) for fixture in fixtures + score_fixtures]
        command = [args.dotnet, str(work / "bin/Debug/net10.0/Probe.dll")]
        subprocess.run([*command, *fixture_args],
                       cwd=work, env=env, check=True, timeout=60)
        # Negative checks must fail at fixture validation, before native load.
        # They run on private copies and never alter the caller's saved files.
        pitch = isolated / "pitch.ustx"
        original = pitch.read_text()
        malformed = {
            "empty-project": (original.split("voice_parts:", 1)[0] + "voice_parts: []\nwave_parts: []\n", "Fixture part/note counts changed"),
            "missing-curve": (original.split("    curves:", 1)[0] + "    curves: []\nwave_parts: []\n", "Missing/unexpected curves"),
        }
        for name, (text, reason) in malformed.items():
            pitch.write_text(text)
            result = subprocess.run([*command, *fixture_args], cwd=work, env=env,
                                    text=True, capture_output=True, timeout=60)
            if result.returncode == 0 or reason not in result.stdout + result.stderr:
                raise SystemExit(f"Negative native fixture did not fail as expected: {name}")
            print(f"NEGATIVE_FIXTURE {name} rejected before load", flush=True)
        pitch.write_text(original)
        result = subprocess.run([*command, *fixture_args[1:]], cwd=work, env=env,
                                text=True, capture_output=True, timeout=60)
        if result.returncode == 0 or "Expected exactly five named fixtures" not in result.stdout + result.stderr:
            raise SystemExit("Missing fixture argument did not fail")
        print("NEGATIVE_FIXTURE missing-file argument rejected", flush=True)
        for fixture in score_fixtures:
            oracle_path = isolated / fixture.with_suffix(".expected.json").name
            original_oracle = bounded_read(oracle_path)
            try:
                oracle_path.unlink()
                result = subprocess.run([*command, *fixture_args], cwd=work, env=env,
                                        text=True, capture_output=True, timeout=60)
                if result.returncode == 0 or "FileNotFoundException" not in result.stdout + result.stderr:
                    raise SystemExit(f"Missing score oracle did not fail: {fixture.name}")
                print(f"NEGATIVE_SCORE {fixture.stem}/missing-oracle rejected", flush=True)
                original_values = json.loads(original_oracle)
                for name, control in score_oracle_controls(original_values):
                    oracle_path.write_text(control)
                    subprocess.run([*command, *fixture_args], cwd=work, env=env,
                                   check=True, timeout=60)
                    print(f"POSITIVE_SCORE {fixture.stem}/{name} accepted", flush=True)
                for name, mutated, reason in score_oracle_mutations(original_values):
                    oracle_path.write_text(mutated)
                    result = subprocess.run([*command, *fixture_args], cwd=work, env=env,
                                            text=True, capture_output=True, timeout=60)
                    if result.returncode == 0 or reason not in result.stdout + result.stderr:
                        raise SystemExit(f"Negative score oracle did not fail as expected: {fixture.stem}/{name}")
                    print(f"NEGATIVE_SCORE {fixture.stem}/{name} rejected", flush=True)
            finally:
                oracle_path.write_bytes(original_oracle)
            target_path = isolated / fixture.name
            original_target = bounded_read(target_path)
            try:
                for name, mutated, reason in score_target_mutations(
                        original_target.decode("utf-8"), json.loads(original_oracle)):
                    target_path.write_text(mutated)
                    result = subprocess.run([*command, *fixture_args], cwd=work, env=env,
                                            text=True, capture_output=True, timeout=60)
                    if result.returncode == 0 or reason not in result.stdout + result.stderr:
                        raise SystemExit(f"Negative score target did not fail as expected: {fixture.stem}/{name}")
                    print(f"NEGATIVE_SCORE_TARGET {fixture.stem}/{name} rejected", flush=True)
            finally:
                target_path.write_bytes(original_target)


if __name__ == "__main__":
    main()
