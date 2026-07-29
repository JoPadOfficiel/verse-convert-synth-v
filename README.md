# Verse

Verse is a desktop application for macOS and Windows that converts karaoke,
MIDI and score files into singing-synthesis projects without inventing lyrics
or notes. It writes two project formats — Synthesizer V `.svp` and OpenUtau
`.ustx` — selected with the **Export target** control. Linux packages are built
for release qualification but are not yet an officially supported user platform.
A complete export keeps the original file, renders one real audio stem per
note-bearing source score Part, and retains a muted full-score reference mix.

![Verse screenshot](docs/convertion_flux.png)

## Why this exists

Neither target application imports a score's words reliably.

**Synthesizer V Studio 1.x** does not reliably import lyrics from MIDI.
Depending on the import/conversion path, users can end up with `la la la`,
altered Western text, missing lyrics or instrumental notes represented as
silent vocal tracks.

**OpenUtau** can already open MIDI and MusicXML itself, and doing so is worse
than it looks. `UNote.lyric` is declared `public string lyric =
NotePresets.Default.DefaultLyric;` (`Ustx/UNote.cs`), and that default is `"a"`
(`Util/NotePresets.cs:61`), so every note an importer leaves untouched keeps
`a`:

- its MusicXML importer sets a lyric only for a real `<lyric>` or for a slur
  continuation, with no `else` branch (`Format/MusicXML.cs:141-161`). Opening
  one real `.musicxml` in OpenUtau 0.1.568 put the lyric `a` on **319 of 319
  notes**, and imported instrument parts as singing tracks.
- its MIDI reader substitutes the same default for any note it cannot match
  (`Format/MidiWriter.cs:205-208`), and it builds its lyric dictionary from
  `LyricEvent` only (`:190-192`). It never reads the MIDI `TextEvent` where a
  Soft Karaoke `.kar` actually stores its words, and a `.kar` loads as plain
  MIDI (`Format/Formats.cs:16`), so **every note in a `.kar` becomes `a`**.
- `int durTick = (int)note.Duration * uproject.resolution / divisions;`
  (`Format/MusicXML.cs:128`) is truncating integer division with no exactness
  check, so a duration that does not divide evenly is silently shortened. It
  also reads `<divisions>` from the first measure only (`:96`), and pairs ties
  by pitch alone (`:100`, `:168-186`).

Verse uses only evidence present in the source:

- a genuine source lyric such as `la` remains `la`;
- a note the source never texted is not written into the vocal project — OpenUtau
  marks an empty lyric `error` and cannot sing one — and it stays byte-exact in
  the bundle's source and audible in its rendered stem;
- generic MIDI Text is metadata, not a lyric;
- a normal MIDI file without lyrics succeeds with zero generated words;
- no fallback pitch, lyric, track, instrument or audio is fabricated; the audio
  tracks a `.versebundle` adds reference WAVs MuseScore rendered from the source
  itself;
- timing is exact or refused, never truncated to fit.

## Supported input formats

| Format | Extensions | Notes |
| --- | --- | --- |
| Karaoke MIDI | `.kar` | Qualified Soft Karaoke text and MIDI lyric events |
| Standard MIDI | `.mid`, `.midi` | Lyric-free MIDI is valid |
| MusicXML | `.mxl`, `.xml`, `.musicxml` | Parts, voices, lyric lanes and unpitched percussion are inventoried |
| MuseScore | `.mscz`, `.mscx` | Native MuseScore score parsing |

## Export targets

One **Export target** selector chooses the project format the vocals-only export
writes. Both targets read the same target-neutral projection, so neither can
change what the other writes.

| Target | Output | Time grid | Held syllable | Syllable split |
| --- | --- | --- | --- | --- |
| Synthesizer V | `.svp`, project version 113 | 705,600,000 blicks per quarter note | `-` | `+` |
| OpenUtau | `.ustx`, `ustx_version` 0.6 | 480 integer ticks per quarter note | `+~` | `+` |

The two marker vocabularies do not agree, so they are never swapped: in
OpenUtau, `+` alone is the syllable split and `+~` is the hold
(`Format/MusicXML.cs:147-149` and `:157-160`; `Format/MidiWriter.cs:209-211`
turns an imported MIDI `-` into `+~`). Verse carries the source's lyric state,
not a rendered marker, and each target spells it in its own words.

Because `705,600,000 / 480 = 1,470,000` exactly, every position OpenUtau can
state is also a Synthesizer V position — but not the reverse. `480 = 2^5 × 3 ×
5` has no factor 7, so a septuplet exports to `.svp` and is **refused** for
`.ustx`. Changing the export target therefore re-analyses every file, because
the convertibility verdict depends on the target.

Each target refuses what it cannot state exactly, and says which note:

- **Both** refuse a position or duration that does not divide exactly into
  their grid, a time-signature change inside a measure, and a timing graph that
  cannot be proven.
- **OpenUtau** additionally refuses a note under its 10-tick floor
  (`UNote.Validate` does `duration = Math.Max(10, duration)` and would silently
  lengthen it), two overlapping notes in one monophonic voice part, and a held
  syllable or syllable split on a note that does not begin exactly where its
  predecessor ends — `UVoicePart.Validate` wires a continuation only when
  `Prev.End == position`, so across a gap OpenUtau would sing the marker itself
  as a word.

Refusing is the point. OpenUtau's own MusicXML importer truncates the same
timing at `Format/MusicXML.cs:128`, so a refusal is strictly more faithful than
the software's native behaviour.

Lyrics are source text. They survive byte for byte in any language and there is
nothing to configure: `src-tauri/tests/language_fidelity.rs` proves it for
French, Spanish, English, Portuguese, German, Polish and Turkish. Verse has
never seen the voice database or singer a track will be sung with, so it does
not fill `database.language` in a `.svp` and the `.ustx` target never writes a
language at all.

## Complete preservation bundle

The primary export is a new `.versebundle` directory:

```text
Song.versebundle/
├── manifest.json
├── preservation.json
├── source/
│   └── Song.mscz
├── project/
│   └── Song.svp        # or Song.ustx, per the export target
└── audio/
    ├── full-score.wav
    └── stems/
        ├── part-0001-…-voice.wav
        └── part-0002-…-piano.wav
```

- `source/` contains a byte-identical copy of the input.
- `preservation.json` records the disposition of inventoried source items.
- `manifest.json` contains hashes, sizes, renderer identity, source ownership,
  exact Part coverage and audio metadata.
- `project/` holds the project in the format the Export target names. It contains
  only evidence-backed vocal-note projections plus the real, audio-backed
  instrumental material: SVP instrumental tracks in a `.svp`, OpenUtau
  `wave_parts` in a `.ustx`.
- `audio/stems/*.wav` contains one source-owned stem per note-bearing MuseScore
  Part. Technical chord lanes stay grouped inside their Part.
- `audio/full-score.wav` is rendered from the original file by MuseScore
  Studio and retained as a muted reference track.

Both project variants reference the same stems, by the same relative paths, with
the same hashes. Because the bundle now carries the chosen target's own project,
its availability follows that target: a source the selected target refuses no
longer offers a bundle either, since offering one would only move the refusal to
export time. Such a source is still bundleable — by selecting Synthesizer V.

Nobody has yet opened a `.ustx` bundle in OpenUtau. The audio references are
written and verified against the manifest and the WAVs; confirming by ear that
the instruments play in the application is still outstanding.

The bundle is staged and committed transactionally. Verse never silently
falls back to a mixed-only or audio-less bundle and never overwrites an
existing bundle. Publication fails and rolls back if a required Part stem,
hash, WAV header or project audio reference is missing or inconsistent.

“Source-faithful” means that the original bytes and a disposition ledger are
preserved. It does not mean every notation or MIDI concept has a lossless
equivalent in either project format.

## Audio renderer and important limits

Complete bundle export requires **one** user-installed **MuseScore Studio
3.6.2+ or 4.x** executable whose `--score-parts` capability passes Verse's
probe. You do not need to install both major versions. Install the
[current MuseScore Studio 4 release](https://musescore.org/en/download) or the
[official MuseScore 3.6.2 release](https://github.com/musescore/MuseScore/releases/tag/v3.6.2),
then configure its executable in Settings or let Verse try to detect it.
MuseScore is not bundled with Verse. Analysis and the secondary vocals-only
export do not need MuseScore; complete bundles do. A native MuseScore 4
score requires a MuseScore 4 renderer. MuseScore 4 can render older MuseScore
sources; MuseScore 3 cannot render a native MuseScore 4 source. Unknown major
versions are rejected until their CLI contract is qualified.

Common executable locations are:

- macOS 4: `/Applications/MuseScore 4.app/Contents/MacOS/mscore` or
  `/Applications/MuseScore Studio 4.app/Contents/MacOS/mscore`;
- macOS 3: `/Applications/MuseScore 3.app/Contents/MacOS/mscore`;
- Windows 4: `C:\Program Files\MuseScore 4\bin\MuseScore4.exe` or the
  equivalent `MuseScore Studio 4` installation folder;
- Linux: `mscore4`, `musescore4`, `mscore3`, `musescore3`, `mscore` or
  `musescore` on `PATH`.

MuseScore renders every note-bearing source Part separately and also renders
the original full score. Accompaniment Part stems start active; Parts that own an
editable vocal projection start muted to avoid doubling the singer; the
full-score reference always starts muted. That applies to both project formats —
OpenUtau has no per-part mute, so the state sits on the wave part's own track,
which is where Synthesizer V keeps it too. These are score-Part stems, not AI
vocal-removal stems. Renderer absence, timeout, invalid output, incomplete Part
coverage or write failure blocks the bundle and leaves no fake or partial result.

The secondary **Vocals only** action writes editable vocal notes in the selected
export target's format — `.svp` or `.ustx`. It does not contain piano or
instrumental audio; use the complete bundle when those parts must be audible.

## Lyrics, tracks and voices

- MusicXML and MuseScore preserve a stable
  Part → staff → voice → projection-lane topology. Chord-member lanes are not
  reported as fake source tracks.
- Soft Karaoke text is accepted only after its karaoke profile is qualified
  and the complete lyric stream has one unique, injective, monotonic melody
  mapping. Ambiguous or partial mappings remain source-only.
- Generic MIDI Text is preserved as metadata.
- Continuation markers are emitted only from source lyric-extension evidence.
- Unpitched percussion and data no target can represent remain in the source,
  ledger and Part/full-score audio.
- UTF-8, UTF-16, ISO-8859-1 and Windows-1252 score XML are decoded from their
  declared encoding; unsupported declarations fail explicitly.
- A manual “Vocal SVP”/“Vocal USTX” override changes only the requested export
  representation. It does not change the reported source role and does not
  invent or copy words from another track.

Verse stops instead of guessing when the selected target cannot express a source
timing graph exactly. Current explicit failures include time-signature changes
inside a measure and advanced score navigation with nested repeats, multiple
jumps, or ambiguous segno/coda targets. Native MuseScore tie/spanner graphs
remain preserved in the original score and Part/reference audio; MusicXML
start/stop tie chains are merged in the editable vocal projection.

Verse does not embed or select a commercial Synthesizer V voice database or an
OpenUtau singer. After opening the project, assign a voice to every vocal
track; without that assignment neither application can sing the notes. The
instrumental WAV in a bundle does not need one.

## Usage

1. Install Verse from the
   [Releases page](https://github.com/JoPadOfficiel/verse-convert-synth-v/releases)
   (`.dmg` on macOS, `.exe` or `.msi` on Windows).
   Linux ARM64/x86_64 packages are build-qualified but currently experimental.
2. Install either MuseScore Studio 3.6.2+ or MuseScore Studio 4 if you want
   complete bundles.
3. Choose the **Export target**: Synthesizer V or OpenUtau. Changing it
   re-analyses every loaded file, because the two targets accept different
   timings.
4. Drop one or more supported files into Verse.
5. Expand a file to inspect source Parts, staff/voice counts, source roles,
   lyric status, stem state and warnings.
6. Optionally change a Part’s eligible projection lanes with “Vocal SVP” /
   “Vocal USTX”.
7. Click **Vocals only** for a bare project in the selected format, or
   **Complete project** (or **Export all complete projects**) for the bundle.
8. Open the project — from inside a bundle it is under `project/` — and assign a
   Synthesizer V voice database or an OpenUtau singer to the vocal tracks.

There is no language selector. Lyrics are source text and are written byte for
byte in any language, with nothing to configure.

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

## Documentation

Start with the [documentation index](docs/index.md). The maintained project
documentation includes:

- [project overview](docs/project-overview.md) and
  [source tree](docs/source-tree-analysis.md);
- [current architecture](docs/architecture.md) and
  [component inventory](docs/component-inventory.md);
- [format and fidelity rules](docs/formats-and-fidelity.md),
  [bundle schema](docs/bundle-format.md), and
  [Tauri command contracts](docs/tauri-command-contracts.md);
- [MuseScore renderer setup](docs/musescore-renderer.md),
  [development](docs/development-guide.md), and [testing](docs/testing.md);
- [security and limits](docs/security-and-limits.md),
  [troubleshooting](docs/troubleshooting.md), and
  [deployment](docs/deployment-guide.md);
- [contribution rules](docs/contribution-guide.md) and
  [corpus testing/licensing](docs/test-corpora.md).

## Development

Prerequisites:

- Rust stable (CI uses Rust 1.93.0);
- Node.js 20.19+ or 22.12+ (CI uses Node.js 22);
- either MuseScore Studio 3.6.2+ or 4.x for real audio-rendering gates.

```sh
npm ci
npm run version:check
npm test
npm run build
npm run tauri dev

cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Optional local gates for the two reported real-world regressions use:

```sh
VERSE_MSCZ_GATE="/path/to/score.mscz" \
VERSE_MXL_GATE="/path/to/score.mxl" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --test source_fidelity
```

The private multi-format regression matrix is opt-in and never committed:

```sh
VERSE_CORPUS_DIR="/path/to/private-corpus" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --test corpus \
  -- \
  --ignored
```

The pinned CC0 OpenScore Lieder runner parses the full vocal-score corpus and
can render a deterministic bounded sample:

```sh
VERSE_MUSESCORE_GATE="/path/to/mscore" \
scripts/run-openscore-corpus.sh --full-parse --render-sample
```

It writes a machine-readable report under `src-tauri/target/corpus-reports/`.
At the pinned commit the corpus contains 1,352 canonical `.mscx` files. The
current gate projects 1,277 exactly, records 75 narrowly classified
source-evidence limitations, and has zero unexpected or render errors.
See [Testing](docs/testing.md) and
[Corpus testing and licensing](docs/test-corpora.md) for the source audit,
rights policy and additional datasets that were evaluated.

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

See the [deployment guide](docs/deployment-guide.md) for the complete CI,
artifact, checksum, platform-support, and unsigned-package contract.

### Architecture

- `src-tauri/src/engine/` parses MIDI, MusicXML and MuseScore into a
  provenance-rich source model and projects evidence-backed vocal material.
- `src-tauri/src/engine/projection.rs` is the target-neutral projection, kept in
  source-exact IR ticks so no target's grid reaches back into the engine.
- `src-tauri/src/engine/target/svp.rs` and `target/ustx.rs` are the two
  serializers; `target/mod.rs` owns the `ExportTarget` selector, the one write
  boundary and the analysis gate both targets answer.
- `src-tauri/src/stems.rs` maps each note-bearing source Part to one stable
  audio stem and its default mute policy.
- `src-tauri/src/renderer.rs` probes MuseScore 3/4 capabilities, extracts
  bounded Parts and bounds every render.
- `src-tauri/src/bundle.rs` creates the transactional preservation bundle,
  validates exact Part coverage, WAVs, hashes and project audio references.
- `src-tauri/src/lib.rs` exposes validated Tauri commands and structured
  errors.
- `src/` is the React interface for analysis, target selection, renderer
  settings, overrides and bundle/vocals-only export.

The SVP serializer targets project format version 113 and expresses time in
blicks; one quarter note is 705,600,000 blicks. The USTX serializer targets
`ustx_version` 0.6 and 480 integer ticks per quarter note — `resolution` is
`[YamlIgnore] => 480` in `Ustx/UProject.cs:42`, so that grid is a property of
the format and cannot be rescaled.

The tracked [architecture document](docs/architecture.md) describes the
implementation that exists today. The BMAD architecture artifacts under
`_bmad-output/` also contain explicitly labelled convergence targets; do not
mistake those future application/domain seams for already shipped modules.

## License

[MIT](LICENSE)
