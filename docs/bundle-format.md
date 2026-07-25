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
│   └── <sanitized bundle name>.svp
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

### SVP project

`project/*.svp` targets Synthesizer V project version 113. It contains:

- evidence-backed editable vocal tracks;
- one audio-backed instrumental track per rendered source Part;
- one muted full-score reference audio track.

An audio-backed instrumental track contains no fake vocal notes and has
`mainRef.isInstrumental = true`. Audio references use `../audio/...` relative
paths and `blickOffset = 0`.

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
| `project` | Path, byte count, and SHA-256 |
| `audio` | Reference mix, stems, and coverage |
| `preservation` | Ledger path, byte count, and SHA-256 |
| `renderer` | Provider, version, major, executable SHA-256, capabilities |
| `alignment` | Timeline alignment policy and SVP offset |
| `warnings` | Source/projection diagnostics retained after the UI closes |

`audio.referenceMix` records the WAV metadata, linked SVP group ID, and default
mute state.

Each `audio.stems[]` record contains:

- stable stem ID and display name;
- owning source Part ID and source track IDs;
- role and default active state;
- isolation method;
- WAV path/hash/size/duration/sample rate/channels/bits/frames;
- matching SVP group ID.

`audio.coverage` records ordered expected and rendered stem IDs and must have
`complete = true`.

The alignment policy is `source-tick-zero` with an SVP blick offset of zero.
Every stem must have the same sample rate and frame count as the full-score
reference.

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

Before and after publication, Verse verifies:

- schema versions;
- exact allowed path set;
- regular files and confined relative paths;
- source byte equality;
- all artifact sizes and SHA-256 hashes;
- complete/unique stem coverage;
- non-empty, non-silent, valid PCM/float WAV data;
- WAV metadata and timeline alignment;
- aggregate audio size;
- one and only one SVP audio track for each asset;
- matching SVP group IDs, references, durations, offsets, and mute states;
- no unrelated or missing file in the bundle.

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
