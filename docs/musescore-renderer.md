# MuseScore Renderer

## When MuseScore is required

MuseScore is required only for a complete `.versebundle`, because Verse needs
real full-score and Part audio. These operations work without MuseScore:

- source analysis;
- Part/voice/lyric inspection;
- vocal projection overrides;
- export-target selection;
- vocals-only `.svp` or `.ustx` export.

Verse does not bundle MuseScore and has no fake-audio fallback.

## Supported versions

Install **one** compatible version:

| Renderer | Status | Compatibility |
|---|---|---|
| MuseScore 3.6.2 or later 3.x | Supported | Sources that MuseScore 3 can open |
| MuseScore Studio 4.x | Recommended/supported | Older sources and native MuseScore 4 sources |
| MuseScore 3 older than 3.6.2 | Rejected | Required CLI contract not qualified |
| MuseScore 5 or future major | Rejected | Must be explicitly qualified first |

A native MuseScore 4 `.mscx`/`.mscz` cannot be rendered by MuseScore 3. Verse
detects the source major and returns an unsupported-renderer error.

The executable must report MuseScore identity and expose `--score-parts` in
`--help`.

## Installation

Use the official [MuseScore download page](https://musescore.org/en/download)
for MuseScore Studio 4, or the
[MuseScore 3.6.2 release](https://github.com/musescore/MuseScore/releases/tag/v3.6.2)
for the final 3.x line.

### macOS

Auto-detection checks, in order:

```text
/Applications/MuseScore Studio 4.app/Contents/MacOS/mscore
/Applications/MuseScore 4.app/Contents/MacOS/mscore
/Applications/MuseScore 3.app/Contents/MacOS/mscore
/Applications/MuseScore 3.6.app/Contents/MacOS/mscore
```

The same application names are checked under `~/Applications`.

### Windows

Verse checks `ProgramFiles` and `ProgramFiles(x86)` for:

```text
MuseScore Studio 4\MuseScore4.exe
MuseScore Studio 4\bin\MuseScore4.exe
MuseScore 4\MuseScore4.exe
MuseScore 4\bin\MuseScore4.exe
MuseScore 3\MuseScore3.exe
MuseScore 3\bin\MuseScore3.exe
```

### Linux and `PATH`

Verse searches:

```text
mscore4
musescore4
mscore3
musescore3
mscore
musescore
```

## Manual configuration

Open Verse Settings and select the actual MuseScore executable, not the
application folder and not a shell script with custom arguments. The frontend
may provide only the executable path; all arguments are fixed in Rust.

The renderer status is:

- `available` — identity and required capability verified;
- `missing` — executable not found or cannot be started;
- `unsupported` — wrong identity/version/capability or incompatible native
  score.

## Probe contract

Verse:

1. canonicalizes the selected path;
2. requires a regular file with a plausible MuseScore filename;
3. computes SHA-256 of the executable;
4. runs `--version` under a ten-second limit;
5. accepts MuseScore 3.6.2+ or major 4 only;
6. runs `--help` and requires `--score-parts`;
7. rechecks the executable hash.

The renderer identity recorded in the manifest includes provider, complete
version output, major version, executable SHA-256, and capabilities.

## Fixed commands

Part extraction:

```text
MuseScore -F --score-parts <input>
```

WAV rendering:

```text
MuseScore -F -o <output.wav> <input>
```

No shell is involved.

MuseScore's `--score-parts` is a backend compatibility interface that returns
JSON containing Part names, optional metadata, and base64 MSCZ payloads. Verse
parses and validates that payload; it does not assume the CLI writes Part files
itself.

## Extraction limits

- JSON response: 128 MiB
- Parts: 1–256
- Decoded MSCZ per Part: 32 MiB
- Aggregate decoded Parts: 512 MiB
- Archive entries per extracted Part: at most 128
- Master MSCX files per extracted Part: exactly one

Unsafe paths, malformed JSON/base64, mismatched arrays, oversized payloads, or
ambiguous archives are rejected.

## Render limits

- Aggregate extraction + full-score + all-stems deadline: 20 minutes
- Maximum one WAV: 2 GiB
- Maximum aggregate audio in a bundle: 8 GiB
- Captured failure log: 64 KiB
- Process polling: 25 ms
- Grace before forced termination: bounded

The child runs with closed stdin, fixed arguments, a private working
environment/home, a limited safe environment, and process-tree termination on
timeout. Verse verifies the executable hash before/after work and validates
each WAV as regular, bounded, non-empty, and non-silent.

## macOS MuseScore 4 shutdown workaround

MuseScore has had macOS teardown races where a successful console conversion
aborts during destruction with:

```text
mutex lock failed: Invalid argument
```

The related upstream lifetime issue is documented in
[MuseScore PR #31084](https://github.com/musescore/MuseScore/pull/31084).

For MuseScore 4 on macOS only, Verse:

- serializes all score-loading processes globally;
- waits ten seconds between completed processes;
- permits at most three attempts under the same aggregate deadline;
- retries only an actual `SIGABRT`;
- retries Part extraction only if the complete JSON payload was already valid;
- removes any failed-attempt WAV before retry;
- still requires the final process to exit successfully and the WAV to pass
  all validation.

This is a bounded compatibility workaround, not permission to ignore arbitrary
MuseScore failures.

## Troubleshooting

See [Troubleshooting](troubleshooting.md#musescore-and-complete-bundles) for
error codes and remediation.
