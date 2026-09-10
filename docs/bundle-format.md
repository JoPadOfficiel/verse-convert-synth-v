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

The ledger remains schema version 2 for exports without performance references.
Performance evidence uses **ledger schema version 3**; the bundle manifest stays
at version 2. Version-2 ledgers remain readable and their serialization remains
unchanged when the new fields are empty. Every ledger contains:

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
- `projectedMapped { policy, limitations? }`
- `renderedStem { stemId }`
- `sourceOnly { reason }`
- `metadataOnly`
- `referenceMixCandidate` only for compatibility when reading older diagnostic
  data

There is no normal “dropped” state. Every entry always references the exact
source; projected/rendered items additionally reference their project or stem.

`projectedMapped` requires schema 3 and identifies supported editable performance
conversion. `policy` and optional `limitations` give concise first summaries;
the shared `performanceSpans` table retains every span's full `detail` policy or
reason. Each event entry's `performanceRefs` indexes that table. A span records
the target format, zero-based `targetTrack` (the actual USTX track/voice part),
`sourceTrackId`, `dimension`, source `startTick`/`endTick`, affected `noteIds`,
and `status` (`mapped`, `unsupported` or `representationLimit`). Editable held
spans are half-open. Source-level records have no target track or affected notes
and identify raw events with no eligible editable ownership.

EXP-003 uses the same table and schema. Its optional `intensity` object stores
policy `verse-score-intensity-v1`, neutral curve and provenance, exact rational
quarter-note `start`/`end`, and target sampling disposition when applicable.
Rationals have `numerator` and positive `denominator`; signed 128-bit values
outside the JSON signed 64-bit range are exact decimal strings. Validation checks
ordering, the supported policy and provenance structure, nested source references,
terminal ownership and bounded evidence size. Older schema-3 spans without this
field remain valid.

When any span contains `intensity`, the ledger also requires an independent
`intensityContext` table. The builder reads source identities from the typed
`Midi` inventory and eligible ownership from final projected notes and their
original `source_evidence`; span contents never supply these ownership values.
All table fields use camelCase:

| Field | Contents |
|---|---|
| `sourcePpq` | Nonzero source pulses per quarter note, consistent with its timebase |
| `sourceTracks` | Inventoried source track IDs |
| `sourceNotes` | `noteId`, original `sourceTrackId`, `originalSourceId`, `startTick`, `sourceOccurrence`, optional `scoreOwner` and `midiChannel`, actual native MIDI `velocity`/`attackSourceId`/`explicitAttack`, and parser-proven `mergedVelocitySources` |
| `controllers` | Inventoried CC7/CC11 `sourceId`, `sourceTrackId`, `tick`, channel `owner` (`port`, `channel`), `controller`, and `value` |
| `projectedNotes` | `noteId`, original `sourceTrackId`, zero-based `targetTrack`, final `destinationTrackId`, source-tick `startTick`/`endTick`, and optional source-proven tie root `intensityAttackNoteId` |
| `scoreOwners` | Original `sourceTrackId` and typed `owner` (`part`, `staff`, `voice`, optional `instrument`), including declared silent voices |
| `declarations` | Original `sourceId`, parser `kinds`, original `scope`, exact written `at`, optional raw `noteSourceId`, symmetric original `pairedSourceIds`, and shared source-route `applications` |

Each declaration application records its original typed `owner`, performed
`occurrence`/`repeatPass`, exact quarter-note route `start`/`end`, and whether the
declaration is `active` on that route. The builder derives these relations from
original written-measure membership and the loader's performed route, including
held prefix state, tempo and endpoint evidence. Transfer reports never supply
these fields. An unresolved application uses only `0/0`, its exact original
written point, and `active: false`.
An endpoint at the end of its own positive written measure can use that
measure's proved route membership; a skipped measure at the same coordinate
cannot. Pass-filtered diagnostics may cite an inactive application, while
mapped evidence requires an active one. Combined endpoint evidence must include
an original paired declaration. Standalone unmatched endpoint diagnostics
remain valid without a pair.

Each affected note must belong to the stated source track and have eligible
ownership on the stated target track. Its projected intervals must cover the
span. A relocated continuation retains its original source track while recording
its final destination separately. Source-only diagnostics need an inventoried
source track but have no target or note ownership. An explicit target terminal
must coincide with an eligible final note endpoint.

Source integer bounds are exact outward coverage of the musical interval:
`startTick = floor(start × sourcePpq)` and
`endTick = ceil(end × sourcePpq)`. Negative, reversed or out-of-u32 bounds are
invalid. A fractional point can therefore cover two neighboring integer bounds
while its exact musical start and end remain equal. USTX target bounds use
exact quarter time × 480, nearest rounding with ties away from zero, and require
the unrounded value within the nonnegative i32 range. A mapped interval must
remain positive after rounding; collapse is a representation limit.

Score intensity requires nonempty authenticated provenance and corresponding
contributing inventory references. Every contributor must be an independently
inventoried expression declaration or an actual native MIDI attack field.
Ordinary note, track and lyric entries cannot substitute for score expression.
Each declaration's original scope must apply to the affected source owner and
its performed occurrence/pass must agree with the independent application table.
Combined provenance may include different applicable scopes; at least one
original declaration must witness the reported scope. Note-owned velocity must
belong to the original attack or an independently proven tie root; moved notes
retain original ownership. Source-only unresolved `0/0` cannot claim target notes.

Every contributing typed CC7/CC11 is checked against original MIDI port/channel
and time, including spans with nonempty velocity provenance. A controller on a
different physical track is valid on the same port/channel; historical held or
recovery contributors may precede the span. An empty provenance array is allowed only for
neutral, controller-only MIDI intensity whose inventoried contributing CC7/CC11
events match the source notes' port/channel and precede or coincide with the
span. Authored SVP segments require provenance; unexpressed `Absent` or
`Held(null)` gap segments may remain neutral. Ordinary segments have positive
duration and ordered, nonoverlapping bounds. A whole source segment may extend
beyond the enclosing note, but it must intersect that note; transition segments
must remain within their authored transition. Ordinary USTX mapped intervals
likewise remain within the original authored transition, preserving clipped
phase fragments. The separately reported niente tail retains its own bounded
containment rule. The native oracle requires the authorized positive floor to
equal DYN −239 within floating numeric epsilon; the 0.1 dB sample tolerance is
reserved for unfloored representable values.

The builder preflights references, text and traversal before copying the tables.
Intensity context and evidence share a 32 MiB serialized-byte ceiling and
250,000-reference ceiling. Validation reserves index storage before allocation
and uses an indexed reciprocal contributor table, with a cumulative budget of
2,000,000 work units. Serialization traversal prepays 64-byte blocks; reference
and index operations are charged separately. Writer callback fragmentation
does not multiply the byte-traversal charge. Ownership construction also uses the performance
work budget. Exceeding a limit prevents validation and bundle
publication. The table establishes relationships independently of individual
span assertions; bundle artifact hashes detect changes against the manifest's
recorded hashes. This is an independent source description, not a cryptographic
signature against replacement of every source, context and manifest artifact.
It validates structural/source relationships and does not establish acoustic
equivalence.
`intensityContext` is omitted when there is no new intensity. Historical schema-2
and no-intensity schema-3 ledgers remain readable without it, with unchanged
rational serialization and no schema-version increase.

Validation sorts a borrowed expected-ID vector and reuses the disposition map
to check source completeness and duplicates. It does not build a second tree
of actual IDs. Ownership indexes are prepaid by their own row counts and
storage requirements; identity-field references remain subject to the separate
reference ceiling and are not counted as rows of one combined tree.

Source-version contracts, raw fields, interpreted scope and repeat occurrence
remain available even when a target reports an unsupported curve. Native note
velocity uses an `expression:event:…:velocity` field reference linked to its
original event; score fields likewise have separate expression IDs. These
references do not change nominal NoteOn or per-note evidence ownership.
Ordinary expression and diagnostic spans use half-open note ownership. A final
terminal declaration is retained explicitly with `terminalEndpoint: true` and
no preceding note IDs; an emitted endpoint without a positive sounding interval
is limited, not a mapped span. Resolved parser diagnostics carry performed
coordinates and repeat provenance. An unresolved performed scope instead retains
the written coordinate and explicit unresolved evidence, with numeric occurrence
and repeat-pass sentinels of zero; it does not assert a performed occurrence.
Mixed mapped/limited score fields retain both structured
references and the primary disposition's limitation summary.

One event may map to one polyphonic sibling and be limited on another; its
references preserve both outcomes without duplicating a full note list in every
event entry. Reference indices, required schema capability and bounded evidence
storage are validated. Mapped rows reference the editable project and exact source.
An entirely unsupported performance event remains `sourceOnly` with a reason
(and any available stem), without an editable-project reference. Consumers of
the ledger must recognize schema 3 and the new disposition instead of treating mapped
performance as `projectedExact`. This does not change nominal-note evidence or
establish that notes pruned from projection were retained (separate FID-001).

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
