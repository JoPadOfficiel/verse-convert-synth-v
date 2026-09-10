#!/usr/bin/env python3
"""EXP-002: exercise pinned native OpenUtau curve consumers, without a singer.

Needs .NET 10, the audited app, and previously fetched upstream source files.
Does not download dependencies or initialize the OpenUtau application.
"""
import argparse
import hashlib
import os
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--fixtures-dir", type=Path, required=True)
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
            shutil.copyfile(fixture, isolated / fixture.name)
        fixture_args = [str(isolated / fixture.name) for fixture in fixtures]
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
        result = subprocess.run([*command, *fixture_args[:-1]], cwd=work, env=env,
                                text=True, capture_output=True, timeout=60)
        if result.returncode == 0 or "Expected exactly five named fixtures" not in result.stdout + result.stderr:
            raise SystemExit("Missing fixture argument did not fail")
        print("NEGATIVE_FIXTURE missing-file argument rejected", flush=True)


if __name__ == "__main__":
    main()
