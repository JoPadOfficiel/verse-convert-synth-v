# Verse Bundle Format

## Purpose

A `.versebundle` is the primary source-faithful export. It combines editable
vocal material with real source-Part audio while retaining the original source
and a machine-verifiable audit trail.

The current persisted schema version is **2**.

## Layout

```text
Song.versebundle/
├── manifest.json
├── preservation.json
├── source/
│   └── <original filename>
├── project/
│   └── <sanitized bundle name>.svp   # or .ustx, per the export target
└── audio/
    ├── full-score.wav
    └── stems/
        ├── part-001-<source-id-hash>-<part-name>.wav
        └── ...
```

Paths are relative, sanitized, confined to the bundle, and validated after
serialization.

## Artifact roles

### Exact source

`source/<original filename>` is a byte-identical copy of the input. Its size
and SHA-256 are recorded in the manifest.

### The project

A bundle writes the project format the **Export target** names: a `.svp` for
Synthesizer V or a `.ustx` for OpenUtau. Both variants reference the same stems,
by the same relative paths, with the same hashes; only the way the reference is
spelled differs.

Because the bundle now carries the chosen target's own project, its availability
follows that target: a source the selected target refuses no longer offers a
bundle either. `bundleReady` therefore equals `ok`. The field is retained for
protocol stability, not because the two can differ. A source only the OpenUtau
target refuses is still bundleable — by selecting Synthesizer V.

#### `.svp` variant

`project/*.svp` targets Synthesizer V project version 113. It contains:

- evidence-backed editable vocal tracks;
- one audio-backed instrumental track per rendered source Part;
- one muted full-score reference audio track.

An audio-backed instrumental track contains no fake vocal notes and has
`mainRef.isInstrumental = true`. Audio references use `../audio/...` relative
paths and `blickOffset = 0`.

#### `.ustx` variant

`project/*.ustx` targets `ustx_version` 0.6 and carries the same audio through
`wave_parts`:

- `relative_path` is the same `../audio/...` reference the `.svp` uses, resolved
  against the `.ustx` file's own directory;
- `position`, `skip`, `trim`, `fadein` and `fadeout` are all `0` — the whole
  file, from the start of the score, which is the claim `blickOffset = 0` makes
  on the Synthesizer V side. Anything else would state an edit the source never
  asked for;
- `file_duration_ms` is derived from the validated WAV's own frame count and
  sample rate as `frames * 1000 / sample_rate`, and is refused if it is not a
  finite number YAML can state;
- `name` is the WAV's basename, because `UWavePart.FilePath`'s setter assigns
  `name = Path.GetFileName(value)` and `AfterLoad` sets `FilePath` from
  `relative_path` — a human label there would be overwritten on load, so it
  lives on `track_name` instead;
- `comment` is empty.

Every wave part gets its own track. That track is not optional:
`UProject.AfterLoad` dereferences `tracks[part.trackNo]` unguarded, so a wave
part without one makes the project unopenable. OpenUtau has no per-part mute, so
the mute state sits on the track — the same place Synthesizer V keeps it.

### Part stems

`audio/stems/*.wav` contains one stem per **note-bearing** source Part.
Technical lanes from chords remain grouped inside their source Part.

Stem IDs are deterministic:

```text
part-NNN-<first 12 hex characters of SHA-256(source_part_id)>
```

Roles:

- `vocalReference` — the Part also owns an editable vocal projection; muted by
  default to avoid doubling the singer.
- `accompaniment` — instrumental/accompaniment Part; active by default.

Rest-only, metadata-only, and lyrics-only Parts do not receive fake silent
stems. They remain in the source and preservation evidence.

### Full-score reference

`audio/full-score.wav` is rendered from the original source and starts muted.
It is an audit/listening reference, not a vocal-removed accompaniment.

## `manifest.json`

Top-level fields:

| Field | Meaning |
|---|---|
| `schemaVersion` | Bundle manifest schema, currently `2` |
| `verseVersion` | Verse application version that wrote the bundle |
| `sourceFormat` | `standardMidi`, `karaokeMidi`, `musicXml`, or `museScore` |
| `source` | Path, byte count, and SHA-256 |
| `project` | Path, byte count, and SHA-256; the path's extension follows the export target |
| `audio` | Reference mix, stems, and coverage |
| `preservation` | Ledger path, byte count, and SHA-256 |
| `renderer` | Provider, version, major, executable SHA-256, capabilities |
| `alignment` | Timeline alignment policy and SVP offset |
| `warnings` | Source/projection diagnostics retained after the UI closes |

**Adding the OpenUtau target did not change the manifest schema.** It remains
version `2`, and every key keeps its name — including `svpGroupId` and
`alignment.svpBlickOffset`, whose names are part of a persisted contract that
must not churn.

`audio.referenceMix` records the WAV metadata, linked SVP group ID, and default
mute state.

Each `audio.stems[]` record contains:

- stable stem ID and display name;
- owning source Part ID and source track IDs;
- role and default active state;
- isolation method;
- WAV path/hash/size/duration/sample rate/channels/bits/frames;
- matching SVP group ID.

`svpGroupId` holds the Synthesizer V group UUID in a `.svp` bundle. In a `.ustx`
bundle it is the **empty string**, and verification *requires* it empty: a
Synthesizer V group UUID in a project that has none would be an invented
identity, so an OpenUtau bundle must state none.

`audio.coverage` records ordered expected and rendered stem IDs and must have
`complete = true`.

The alignment policy is `source-tick-zero` with an SVP blick offset of zero, for
both targets. Every stem must have the same sample rate and frame count as the
full-score reference.

## `preservation.json`

The ledger is schema version 2 and contains:

- `expectedSourceIds`
- one `entries[]` item for every inventoried item in the current rich source
  model

An entry contains:

- stable `sourceId`;
- `itemKind`: track, instrument, event, note, or lyric;
- exactly one primary disposition;
- one or more preserving artifact paths.

Primary dispositions:

- `projectedExact`
- `renderedStem { stemId }`
- `sourceOnly { reason }`
- `metadataOnly`
- `referenceMixCandidate` only for compatibility when reading older diagnostic
  data

There is no normal “dropped” state. Every entry always references the exact
source; projected/rendered items additionally reference their project or stem.

The ledger inventories constructs represented in Verse's current source model.
Unknown or opaque source-format constructs remain preserved by the
byte-identical source even when they do not receive their own ledger row.

## Part alignment

MuseScore `--score-parts` output is aligned to planned stems using:

1. unique native Part ID from `partsMeta.id`; then
2. a unique normalized Part display name.

Duplicate IDs, duplicate names without an ID match, missing expected Parts,
duplicated ordinals, or a Part count that does not prove the expected
note-bearing topology blocks publication.

If MuseScore exposes an additional rest-only Part that the current stem plan
cannot match one-to-one, the bundle fails closed rather than silently ignoring
or fabricating an asset.

## Integrity validation

Verification is **manifest-driven**. Verse reopens `manifest.json` and checks
every artifact the manifest declares; it never enumerates the bundle directory.
`fs::read_dir` appears in `bundle.rs` only under `#[cfg(test)]`.

Before and after publication, Verse verifies:

- schema versions;
- regular files and confined relative paths, resolved through `safe_join`;
- source byte equality;
- the size and SHA-256 of every manifest-declared artifact — a declared file
  that is missing, resized, or altered fails here;
- complete/unique stem coverage across `expectedStemIds`, `renderedStemIds`, and
  the `stems[]` records;
- non-empty, non-silent, valid PCM/float WAV data;
- WAV metadata and timeline alignment;
- aggregate audio size;
- exactly one project audio reference for each asset — one SVP audio track in a
  `.svp` bundle, one `wave_parts` entry in a `.ustx` bundle;
- references, durations, offsets, and mute states matching the manifest, plus the
  SVP group ID in a `.svp` bundle and its emptiness in a `.ustx` bundle;
- that every artifact path referenced by a `preservation.json` entry is one of
  the manifest-declared source, project, and audio paths.

The project check is forked per target over one shared canonicalisation block, so
both variants prove the same invariants — one reference per stem, the muted
full-score reference, and every path canonicalised to stay under the bundle root.
For a `.ustx`, the committed file is re-read through a strict reader that accepts
only the layout the emitter writes, and the reference is compared as the
double-quoted scalar the file states, byte for byte, so the equality proves what
was written rather than what an unescaper made of it. `file_duration_ms` is
recomputed from the validated WAV's own frame count and sample rate, so a
duration that drifted from the audio cannot be committed.

What this does **not** do: an extra, unrelated file placed inside a committed
bundle directory is not detected, because nothing walks the directory. The
guarantee is that everything the manifest declares is present and exactly as
declared, and that the ledger references nothing outside that set.

Nobody has yet opened a `.ustx` bundle in OpenUtau 0.1.568. The references are
written and verified against the manifest and the WAVs; the listening check that
the instruments are audible in the application is outstanding.

## Transaction and rollback

Publication is no-replace:

1. validate the new `.versebundle` destination;
2. create a unique sibling staging directory;
3. add a Verse-owned staging marker;
4. write and validate all artifacts;
5. rename staging to the final destination;
6. reopen and validate the committed bundle.

An existing destination is never overwritten. On failure, Verse removes only
the staging directory whose identity and marker prove ownership. This is a
transactional staging model; persistent job journaling/recovery across an
application crash is not yet implemented.

## Compatibility

Schema v1 bundles remain historical immutable artifacts. Current writes are
always schema v2 and never fall back to v1. Consumers must reject unknown
schema versions rather than guessing.
