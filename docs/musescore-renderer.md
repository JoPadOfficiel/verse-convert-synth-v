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
`--help`. That flag qualifies the installation; complete bundles no longer
extract Parts with it.

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

Score conversion, for a MusicXML/MXL or MIDI/KAR source (a `.kar` is handed
over as a `.mid` copy of the same bytes, because MuseScore aborts on a `.kar`
path):

```text
MuseScore -F -o <output.mscz> <input>
```

WAV rendering, for the full-score reference and every stem:

```text
MuseScore -F -o <output.wav> <input>
```

No shell is involved. Conversion uses the same score-loading process policy,
executable hash checks, retry rules and deadline as rendering.

A score stem is the whole source score, or its converted `.mscz`, with every
other Part's notes and chord symbols silenced by an inserted `<play>0</play>`.
Rendering the full score keeps every fermata, breath and tempo mark on the
reference mix's timeline; a Part cut out with `--score-parts` would lose the
timing other Parts impose. A MIDI stem is MuseScore's `.mscz` import of the
whole file silenced the same way, once its Parts are proven to be the
note-bearing source tracks; otherwise it is one byte-identical source track
after the file's global marks. See
[Formats and fidelity](formats-and-fidelity.md#audio-stems).

### Source playback defaults

MuseScore 3 gives a fermata with no `timeStretch` property a playback multiplier
of 1. MuseScore 4 defaults a normal fermata to 2. Loading unchanged legacy XML
in the newer renderer can therefore add a pause absent from the vocal project.
Verse writes the legacy value of 1 explicitly in a private render copy before
rendering the reference and every Part stem. The preserved source stays
byte-identical. The diagnostic is
`MUSESCORE_LEGACY_FERMATA_DEFAULT_PRESERVED`.

For an explicitly prolonged legacy fermata, the newer renderer may split an
implicit silence into shorter notation segments and restore normal tempo too
early. Verse lowers the exact written native 3 playback map into private tempo
annotations, neutralizes only the private fermata playback, and retains every
existing note and voice container. Before rendering it checks that the prepared
input preserves the complete note geometry and played tempo map. Unsupported
lowering refuses with `MUSESCORE_PLAYBACK_MAP_UNPROVEN`; successful preparation
reports `MUSESCORE_SOURCE_PLAYBACK_MAP_PRESERVED`.

Native explicit fermata prolongations, and qualified MuseScore 4 subtype
defaults, use one global playback tempo map in both vocal export targets.
They never move nominal note positions or alter their written durations.

### Qualified legacy drum template compatibility

MuseScore 4 can resolve a native `Instrument id="piano"` as piano before
consulting its explicit drumset evidence. Verse permits one bounded exception
in private renderer intermediates: change that template ID to `drumset` only
when one instrument owns one explicitly percussion staff, `useDrumset` is 1,
the source supplies a unique Standard-range drum map covering every played key,
and one channel explicitly declares native channel 9, bank MSB 1/LSB 0 (128),
and program 0. The taxonomy must be absent, `keyboard.piano`, or
`drum.group.set`; nonempty playback `soundId`, extra channels/instruments,
ownership changes and unsupported native layouts are refused.

The same prepared native master feeds the full-score reference and all stems,
including qualified MusicXML/MXL and MIDI/KAR imports. A mapped MIDI import
whose Part mapping cannot be proven is refused rather than combined with
per-track fallback stems. Inputs without this conflict keep the existing render
paths. MuseScore 3 receives unchanged templates.

`MUSESCORE_DRUM_TEMPLATE_MAPPED` records the exception in the manifest;
`MUSESCORE_DRUM_TEMPLATE_UNPROVEN` refuses an unqualified conflict before bundle
publication. Only the template attribute changes. Original source snapshots,
Part/stem identities, drum mappings, MIDI state, notes and mute defaults remain
unchanged. Other archive entries retain their compressed payloads, compression
method, modification timestamps and Unix permissions. The archive comment is
also retained; individual entry comments and extra fields are not preserved by
the existing ZIP copy path.

The authored real-renderer control compares complete corrected/canonical WAV
sample payloads byte for byte and distinguishes kick, snare and hi-hat from an
ordinary piano control. This qualifies the installed renderer and its sound
resources; it does not promise acoustic quality across banks or installations.

## Conversion limits

- Converted `.mscz`: 32 MiB, and never more than the WAV limit
- Archive entries: at most 128, each with a confined path
- Master MSCX files: exactly one

A converted score that is not such an archive is rejected before any stem is
rendered.

## Render limits

- Aggregate conversion + full-score + all-stems deadline: 20 minutes
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
- retries `--score-parts` (probe and corpus use only) only if the complete JSON
  payload was already valid;
- removes any failed-attempt WAV or converted score before retry;
- still requires the final process to exit successfully and the WAV or
  converted score to pass all validation.

This is a bounded compatibility workaround, not permission to ignore arbitrary
MuseScore failures.

## Troubleshooting

See [Troubleshooting](troubleshooting.md#musescore-and-complete-bundles) for
error codes and remediation.
