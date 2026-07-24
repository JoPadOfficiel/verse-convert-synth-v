# Verse

Verse is a desktop application for macOS and Windows that converts karaoke,
MIDI and score files into Synthesizer V projects without inventing lyrics or
notes. Its complete export also keeps the original file and adds a real,
audible full-score reference mix.

![Verse screenshot](docs/screenshot.png)

## Why this exists

Synthesizer V Studio 1.x does not reliably import lyrics from MIDI. Depending
on the import/conversion path, users can end up with `la la la`, altered
Western text, missing lyrics or instrumental notes represented as silent vocal
tracks.

Verse uses only evidence present in the source:

- a genuine source lyric such as `la` remains `la`;
- an untexted note remains untexted;
- generic MIDI Text is metadata, not a lyric;
- a normal MIDI file without lyrics succeeds with zero generated words;
- no fallback pitch, lyric, track, instrument or audio is fabricated.

## Supported input formats

| Format | Extensions | Notes |
|---|---|---|
| Karaoke MIDI | `.kar` | Qualified Soft Karaoke text and MIDI lyric events |
| Standard MIDI | `.mid`, `.midi` | Lyric-free MIDI is valid |
| MusicXML | `.mxl`, `.xml`, `.musicxml` | Parts, voices, lyric lanes and unpitched percussion are inventoried |
| MuseScore | `.mscz`, `.mscx` | Native MuseScore score parsing |

## Complete preservation bundle

The primary export is a new `.versebundle` directory:

```text
Song.versebundle/
├── manifest.json
├── preservation.json
├── source/
│   └── Song.mscz
├── project/
│   └── Song.svp
└── audio/
    └── full-score.wav
```

- `source/` contains a byte-identical copy of the input.
- `preservation.json` records the disposition of inventoried source items.
- `manifest.json` contains hashes, sizes, renderer identity and audio metadata.
- `project/*.svp` contains only evidence-backed vocal-note projections plus a
  Synthesizer V instrumental-audio track.
- `audio/full-score.wav` is rendered from the original file by MuseScore
  Studio 4 and referenced by the SVP as real instrumental audio.

The bundle is staged and committed transactionally. Verse never silently
falls back to an audio-less bundle and never overwrites an existing bundle.

“Source-faithful” means that the original bytes and a disposition ledger are
preserved. It does not mean every notation or MIDI concept has a lossless SVP
equivalent.

## Audio renderer and important limits

Complete bundle export requires a user-installed **MuseScore Studio 4**.
Configure its executable in Settings or let Verse try to detect it. MuseScore
is not bundled with Verse.

MuseScore renders the **original full score**. This keeps piano, instruments
and percussion audible, but the WAV is a reference mix, not a clean
vocal-removed accompaniment stem. Renderer absence, timeout, invalid output or
write failure blocks the bundle and leaves no fake or partial result.

The secondary “Vocals `.svp`” action writes only editable vocal notes. It does
not contain piano or instrumental audio; use the complete bundle when those
parts must be audible.

## Lyrics, tracks and voices

- MusicXML and MuseScore lyric ownership stays attached to the source
  note/voice/lane.
- Soft Karaoke text is accepted only after its karaoke profile is qualified.
- Generic MIDI Text is preserved as metadata.
- Continuation markers are emitted only from source lyric-extension evidence.
- Unpitched percussion and data not representable in SVP remain in the source,
  ledger and full-score audio.
- UTF-8, UTF-16, ISO-8859-1 and Windows-1252 score XML are decoded from their
  declared encoding; unsupported declarations fail explicitly.
- A manual “Vocal SVP” override changes only the requested export
  representation. It does not change the reported source role and does not
  invent or copy words from another track.

Verse stops instead of guessing when Synthesizer V cannot express a source
timing graph exactly. Current explicit failures include time-signature changes
inside a measure and advanced score navigation with nested repeats, multiple
jumps, or ambiguous segno/coda targets. Native MuseScore tie/spanner graphs
remain preserved in the original score and reference mix; MusicXML start/stop
tie chains are merged in the editable vocal projection.

Verse does not embed or select a commercial Synthesizer V voice database.
After opening the project, assign a compatible voice to every vocal track.
Without that assignment Synthesizer V cannot sing the notes. The instrumental
WAV does not need a voice database.

## Usage

1. Install Verse from the
   [Releases page](https://github.com/JoPadOfficiel/verse-convert-synth-v/releases)
   (`.dmg` on macOS, `.exe` or `.msi` on Windows).
2. Install MuseScore Studio 4 if you want complete bundles.
3. Drop one or more supported files into Verse.
4. Expand a file to inspect source roles, lyric status, export representation
   and warnings.
5. Optionally change a pitched track’s “Vocal SVP” export choice.
6. Click **Bundle** (or **Export all bundles**) for the complete result.
7. Open `project/*.svp` from inside the bundle in Synthesizer V and assign a
   voice database to the vocal tracks.

The selected lyric language configures the Synthesizer V vocal database
language. It never translates, normalizes or phoneticizes source text.

### Opening an unsigned build

Released binaries are not code-signed with paid Apple/Microsoft developer
certificates, so the operating system may ask for one-time confirmation.

**Windows:** on the SmartScreen dialog, select **More info > Run anyway**.

**macOS:** if Gatekeeper reports that Verse is damaged, remove the download
quarantine flag:

```sh
sudo xattr -rd com.apple.quarantine "/Applications/Verse.app"
```

Adjust the path if Verse is installed elsewhere.

## Development

Prerequisites:

- Rust stable;
- Node.js 22 or later;
- MuseScore Studio 4 for real audio-rendering gates.

```sh
npm ci
npm run build
npm run tauri dev

cd src-tauri
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Optional local gates for the two reported real-world regressions use:

```sh
VERSE_MSCZ_GATE="/path/to/score.mscz" \
VERSE_MXL_GATE="/path/to/score.mxl" \
cargo test --locked --test source_fidelity
```

### Releases

Release Please maintains synchronized versions and `CHANGELOG.md`, creates a
draft release, and creates its `vMAJOR.MINOR.PATCH` tag before the reusable
six-platform build starts. The build verifies that exact tag/commit pair,
assembles stable asset names and checksums, replaces draft assets
idempotently, and only then publishes.

Repository administrators must enforce immutable/protected `v*` tags in
GitHub. The workflow revalidates the tag immediately before and after
publication, but a repository rule is the atomic protection against an
external force-push during that final API operation.

### Architecture

- `src-tauri/src/engine/` parses MIDI, MusicXML and MuseScore into a
  provenance-rich source model and projects evidence-backed vocal material.
- `src-tauri/src/renderer.rs` discovers and bounds MuseScore Studio 4
  rendering.
- `src-tauri/src/bundle.rs` creates the transactional preservation bundle,
  validates WAV output and writes manifests/hashes.
- `src-tauri/src/lib.rs` exposes validated Tauri commands and structured
  errors.
- `src/` is the React interface for analysis, renderer settings, overrides and
  bundle/vocal-only export.

The SVP serializer currently targets project format version 113. Time is
expressed in blicks; one quarter note is 705,600,000 blicks.

## License

[MIT](LICENSE)
