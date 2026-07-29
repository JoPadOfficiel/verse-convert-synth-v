# Testing

Verse tests source fidelity, projection safety, target representability,
renderer behavior, bundle integrity, and the desktop frontend separately. A
successful test run is not defined only by producing a project file: every
emitted lyric and note must retain source evidence, source topology must remain
stable, timing must be exact or refused, and a complete bundle must contain
exactly the audio assets declared by its manifest.

## Current verified baseline

The authoritative count is whatever the gate commands below report on the
revision under test; the suite grows with every change, so no fixed total is
recorded here.

Some Rust tests are ignored by design:

- private KAR parity fixtures;
- private-corpus gates;
- child-process helpers launched by their parent renderer tests.

Ignored private tests are not optional once explicitly requested. They fail if
their required fixtures are missing. Copyrighted fixtures remain outside Git.

## Required quality gates

Run the following commands from the repository root:

```sh
npm ci
npm run version:check
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

These are the same core gates used by CI. The CI environment uses Node 22,
Rust 1.93.0, an immutable npm install, the locked Cargo dependency graph, and
Ubuntu 22.04 native Tauri dependencies.

`npm run version:check` verifies that the strict SemVer value is synchronized
across:

- `package.json`;
- the root package entries in `package-lock.json`;
- `src-tauri/Cargo.toml`;
- the Verse package in `src-tauri/Cargo.lock`;
- `src-tauri/tauri.conf.json`;
- `.release-please-manifest.json`;
- the latest dated heading in `CHANGELOG.md`.

For a release tag, also validate the exact `vVERSION` value:

```sh
node scripts/check-version.mjs --tag v0.4.0
```

Replace `v0.4.0` with the release being validated.

## What the active suites cover

### Rust unit and integration tests

The Rust suites exercise:

- MIDI SMF 0/1 parsing, event ordering, Windows-1252 text, lyric events,
  Soft Karaoke qualification, note-on velocity zero, and malformed input;
- source-faithful KAR lyric ownership, detached-lyric ambiguity, percussion,
  chord ambiguity, and lyric-free MIDI;
- MusicXML/MXL Parts, staves, voices, lyric lanes, ties, tuplets, repeats,
  encodings, percussion, archive safety, and exact rational timing;
- native MuseScore 2/3/4 parsing, master-score selection, styled lyrics,
  voice topology, grace notes, local meters, and malformed archives;
- SVP serialization and validation, including that the blick grid refuses an
  inexact position rather than rounding it;
- USTX serialization: byte-exact YAML output, the 480-tick exactness gate
  refusing a septuplet, the 10-tick duration floor, the `+~`/`+` marker
  vocabulary, `lyric: ""` for an untexted note, monophonic overlap refusal, and a
  marker refused across a gap;
- target dispatch: that the analysis gate refuses exactly what the write boundary
  refuses, that `ExportTarget`'s serde values are the stable `"svp"`/`"ustx"`
  protocol strings with Synthesizer V as the default, and that a refusal stays
  distinguishable from an encoder fault;
- that lyric text of any language reaches the output byte-exactly with nothing
  configured (`tests/language_fidelity.rs`, covering fr, es, en, pt, de, pl, tr);
- Part-level stem planning;
- renderer probing, fixed arguments, timeouts, process-tree termination,
  output validation, Part extraction, and the bounded macOS retry policy;
- preservation-ledger completeness, hashes, audio references, staging,
  rollback, destination races, and atomic no-replace bundle publication;
- both bundle project variants: that a `.ustx` bundle references the same stems
  as its `.svp` counterpart, that `svpGroupId` is empty in a `.ustx` bundle and
  verification refuses a non-empty one, that a wave part's offsets are all zero
  and its `file_duration_ms` matches the validated WAV, and that the committed
  `.ustx` re-reads through the strict reader;
- public-corpus auditing and deterministic render sampling.

The integration tests also lock these behaviors:

- a lyric-free MIDI file succeeds with no synthetic vocal track;
- generic MIDI Text is not silently promoted to lyrics;
- the first note of the supplied “This Little” score is pitch 65 with an empty
  lyric, and the first non-empty source lyric is `let`;
- every projected `la` has a corresponding source lyric;
- source percussion inventory and channel metadata survive MusicXML parsing;
- equivalent MusicXML and MuseScore sources retain stable Part/staff/voice
  topology.

### Frontend tests

The Node tests cover frontend utilities, structured error parsing, the
distinction between renderer/audio errors and other export failures, vocal
override state, per-target default output paths, and version-check behavior.

## Real-file fidelity gates

The real-file tests return early when their environment variables are absent.
Provide the audited score paths to activate them:

```sh
VERSE_MSCZ_GATE="/path/to/This Little S_Pno Melodie.mscz" \
VERSE_MXL_GATE="/path/to/Help SAB PB MZ4.mxl" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --test source_fidelity
```

These files are evidence gates, not snapshots committed to the repository. The
MSCZ fixture currently proves 924 source notes and 171 source lyrics. The MXL
fixture proves 695 unpitched source percussion notes for Part `P6`, including
the source MIDI channel metadata.

## Private regression corpus

Run both ignored corpus tests with the audited private directory:

```sh
VERSE_CORPUS_DIR="/path/to/private-corpus" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --test corpus \
  -- \
  --ignored
```

The private KAR expectations are exact:

| Fixture | Projected source-backed lyrics |
|---|---:|
| Beatles — All You Need Is Love | 207 |
| Beatles — HELP | 314 |
| Dirty Dancing — She's Like the Wind | 218 |
| Elvis Presley — Heartbreak Hotel | 276 |
| Elvis Presley — Hound Dog | 244 |
| Cabaret | 162 |
| Queen — Crazy Little Thing Called Love | 0 |

The Queen result is intentionally empty because the source does not prove a
safe melody binding. Cabaret additionally locks eight unresolved chord pitches.
Neither case may be “repaired” by inventing lyrics or a fallback C4.

The private score fixtures lock:

- “This Little”: 2 Parts and 3 source voices;
- “Help” MXL: 6 Parts and 10 source voices;
- “Help” MSCZ: 6 Parts and 10 source voices;
- “Iko Iko”: 8 Parts and 9 source voices;
- exactly one stem per note-bearing Part;
- no empty vocal track in the generated project.

## Public OpenScore corpus

The reproducible public corpus is OpenScore Lieder at the pinned commit
`6b2dc542ce2e8aa4b78c8ee62103b210efc07015` under CC0-1.0.

Parse and project the complete pinned corpus:

```sh
scripts/run-openscore-corpus.sh --full-parse
```

Also render the default deterministic sample of three scores and every
extracted Part:

```sh
VERSE_MUSESCORE_GATE="/path/to/mscore" \
scripts/run-openscore-corpus.sh --full-parse --render-sample
```

The current accepted baseline is:

- 1,352 score files discovered;
- 1,343 files parsed;
- 1,277 files projected exactly;
- 75 files classified as evidence-ineligible;
- 0 unexpected errors;
- 0 evidence-invariant failures;
- 2,893 Parts and 7,154 voices;
- 1,315,791 source notes;
- 279,661 source lyrics;
- 278,643 projected lyrics;
- 3 deterministically selected scores rendered;
- 6 expected Part stems rendered;
- 0 render errors;
- MuseScore 4.7.4 used for the render sample.

The runner verifies the repository URL, exact commit, clean checkout, and
license evidence before auditing. Unknown errors fail the run. Only a narrow,
typed set of structures that cannot be projected exactly may be classified as
evidence-ineligible. Baseline drift also fails.

## Full renderer integration

MuseScore-dependent integration can be activated with:

```sh
VERSE_MUSESCORE_GATE="/path/to/mscore" \
VERSE_SCORE_PARTS_GATE="/path/to/score.mscz" \
VERSE_BUNDLE_GATE="/path/to/score.mscz" \
VERSE_PART_MAPPING_GATE="/path/to/mapping-fixture-or-directory" \
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Use a supported MuseScore 3.6.2+ or MuseScore 4 executable that advertises
`--score-parts`. MuseScore 4 is required when the native input itself is a
MuseScore 4 score.

## Interpreting a failure

A parser or projection refusal is not automatically a regression. Verse
deliberately fails closed when ownership or timing cannot be represented
exactly. A regression exists when one of these occurs:

- source evidence disappears from the inventory or preservation ledger;
- an emitted note or lyric has no source evidence;
- an empty lyric becomes `la`, `a`, or another fabricated syllable;
- an arbitrary source pitch becomes a fallback pitch;
- Parts or source voices are merged unexpectedly;
- a timing that does not divide exactly into the selected target's grid is
  rounded, truncated, or lengthened instead of refused;
- a hold marker is emitted in the other target's vocabulary, turning a hold into
  a split or a split into a hold;
- a refusal appears for the first time at export instead of at analysis;
- changing the export target changes the bytes the other target writes;
- a renderer failure leaves a published or partial destination;
- a manifest, hash, stem identity, WAV, or project audio reference passes despite being
  inconsistent.

When adding support for a previously ineligible construct, add a focused unit
test, a fidelity assertion, and—where legally possible—a corpus case. Do not
broaden an allowlist merely to make a corpus total green.
