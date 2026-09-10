---
title: Resolve French sung fragments from their written context
type: bugfix
created: 2026-09-09
status: done
baseline_commit: 425dab11208c3a8663b41aaa41defe601bac13aa
review_loop_iteration: 0
context:
  - docs/contribution-guide.md
  - _bmad-output/implementation-artifacts/spec-fr-001-sung-pronunciation.md
  - _bmad-output/implementation-artifacts/spec-fr-002-community-lexicon.md
  - src-tauri/src/engine/target/french.rs
  - src-tauri/src/engine/target/lexical.rs
  - src-tauri/src/engine/convert.rs
  - src-tauri/tests/french_phonetics.rs
  - _bmad-output/implementation-artifacts/fr-003/context-audit/candidate-layouts.json
  - _bmad-output/implementation-artifacts/fr-003/context-audit/classified-482.json
---

<frozen-after-approval>

## Intent

The first dictionary integration leaves 482 source occurrences without a Verse
reading and incorrectly treats some fragments as unrelated dictionary words.
The user requires correction in Verse, not a list of manual OpenUtau repairs.
The two private score masters contain enough evidence for most cases: explicit
syllables, repeated vowel spellings, elisions and word boundaries remain available.

Extend the French pronunciation policy to consume that evidence. Resolve each
audited occurrence wherever the source determines a supported sung reading.
Preserve written syllable attacks and real holds, including vowel repetitions,
sung schwas and packed syllables. Protect the source against unrelated whole-word
readings such as the proper-name `chan` inside `chan / ger`. Apply consistently to
all vocal parts, including source-owned material separated by polyphonic
decomposition. Use source identity, not names, pitches, song positions or titles.

## Boundaries

Always preserve source objects, source files, notes, rests, timing, tempo,
nominal pitches, extensions and existing expressive fields. Pronunciation alone
may change. Keep Default, English and SVP compatibility. Prefer audited exact
syllable layouts and indexed community readings over a new runtime dependency.
Retain source-linked diagnostics that explain applied readings and any genuinely
unresolved source ambiguity. A dictionary hint is not acoustic validation.

Ask first only when the source supports materially different intended lyrics
and none can be chosen from its evidence. Do not treat the old conservative
guards as a reason to stop correcting cases which now have evidence.

Never add source-specific indices, filenames or complete song lyrics to runtime
rules or public fixtures. Never invent attacks from a hold, fill untexted notes,
remove all terminal consonants, assume all h-initial words allow liaison, or
choose dictionary variants by number alone. Do not change user originals or
manual edits. No release/version edits or push.

## I/O and Edge Cases

| Evidence | Required behavior |
| --- | --- |
| `j'i / rai`, `rai / son`, `s'a / chève` | Contextual per-note readings; no independent `rai` noun or `son` possessive reading |
| `gar / de / rai`, `j'ef / fa / ce / rai` | Sung schwa on its written attack; no duplicated consonant |
| `for / ce`, `tê / te`, `bles / su / re` | Supported sung schwa rather than dropped syllable |
| Repeated `rê / ê / ve`, `mu / u / ur`, `courants`/`jours`/`partir`/`trace`/`espace` vowel layouts | Every written vowel attack remains; only explicit holds stay holds |
| Orphan End inside an exact audited vowel/schwa layout | Permit the evidenced pronunciation without rewriting or globally ignoring metadata |
| Explicit Single, different row/voice/repeat, manual hint, unknown sequence | No unsafe word reconstruction or automatic consonant relocation |
| Two vowels in source `sons'a` followed by a real hold | Preserve both source vowel nuclei on the authored note and hold semantics; no fabricated note |
| A complete written word whose notes are separated by a rest | Independent supported hints may use explicit word evidence; do not stretch notes or use illegal native `+` across silence |
| `au / bout / de` with misleading Begin/End | Exact phrase establishes three words; no guessed compound or liaison from fragment identity |
| Verse-number prefix matching explicit lyric verse | Treat the proven verse label as notation for pronunciation only; keep original bytes; protect mismatched numbers and numbered dictionary variants |
| Independent `Ouh` vocalisation | Supported `fr/ou`, preserving its explicit attack |
| Source word divided between polyphonic members | Derive contextual hints only from matching written provenance and unambiguous chronology; preserve lane geometry and lyric ownership |
| Phrase punctuation before a repeated syllable | Do not silently relocate the preceding word's consonant across punctuation |

</frozen-after-approval>

## Code Map

The private audit maps all 482 occurrences to original lyric IDs and neighborhoods.
Its candidate layouts and classifications are development evidence, not shipped
data. `french.rs` owns 67 exact layouts with per-attack hints and explicit allowed
premature-End slots. A complete Begin/Middle/End word binding permits independent
layout hints across a rest; unknown layouts and native joins keep their guards.
Whole-word fallback still requires exactly as many vowels as attacks. Orphan
fragments reach standalone lookup only under the existing curated exceptions or
positive contextual evidence.

`project_track` applies profile pronunciation per part/staff/voice/occurrence;
source tracks may already be split into polyphonic members. Any recovery of
context must prove those domains from source fields. It must not move lyrics or
change lane decomposition. The sibling FR-003 change owns output-only edge-dash
cleanup in `ustx.rs`; this implementation does not modify that boundary.
`french_source_context` groups all source note geometry by original part, staff,
voice and occurrence, then selects eligible lyrics per row and verse using the
projection's first-nonblank policy. Equal lyric copies require the same written
chord and bounds; untexted members of that same chord are harmless but never
receive lyrics. Conflicts, missing/mismatched chord IDs and overlapping material
block reconstruction. Proven chronology supplies whole-word identity for liaison
before hints are transferred to already selected lyrics by track, event order
and lyric ID. It never changes selection or lane decomposition.

The internal `Lyric::verse_from_score` flag distinguishes score-established verse
rows from MIDI's default verse 1. Score adapters set this evidence; unnamed MIDI
lyrics and nonnumeric MusicXML row labels cannot authorize prefix stripping.
French dictionary joins use the same verse-aware key as standalone pronunciation.
The IR is not an IPC payload and requires no frontend mirror.

## Tasks and Acceptance

- [x] Implement contextual rules covering every audited resolvable class,
  including incorrect existing hints. Add generic synthetic fixtures, not songs.
- [x] Resolve the polyphonic lexical-context case from original source identity
  where unambiguous, with negative independent-voice/row/repeat coverage.
- [x] Add positive/negative tests for the matrix; preserve complete source objects
  and musical fields. Explicitly test multi-vowel hints plus real holds.
- [x] Reconvert both masters to a new private directory, compare all 3,124 notes
  with the previous corrected exports, and account for each original warning by
  stable source ID. Do not relabel an unresolved case as applied without a reading.
- [x] Run installed Millefeuille symbol/allocation validation and verify every
  expected source attack survives. Report acoustic validation separately.
- [x] Update public specifications and French guidance with current limits and
  actual coverage. Parent owns consolidated gate and commit.

## Design Notes

Use the audited dictionary pronunciations, with explicit known French inflection
or sung-layout additions where absent. Choose the smallest implementation that
meets the matrix; if better equivalent evidence handling emerges, preserve the
frozen contract and record it. No network or custom model installation is needed.

Implementation decisions:

- Exact layouts are applied before standalone lookup, so wrong existing fragment
  readings are corrected as well as missing readings. Longest layouts take
  precedence and each source attack is annotated once.
- `pre / sse / e` uses `fr/p fr/r fr/ae | fr/s fr/ee | fr/ee`: the pinned sung
  `presse(2)` reading and an explicit final schwa echo. This is a bounded layout,
  not a general rule for bare `e`.
- `sons'a` receives `fr/z fr/on fr/s fr/ah` on its authored note. Native
  `ProcessWord` retains both vowel nuclei in that note's phoneme group and does
  not allocate a new attack to the following hold. The source does not determine
  exact phoneme timing within the group.
- `au / bout / de` contributes three independent word identities for liaison;
  the packed two-word layout is never treated as a standalone dictionary word.
- The punctuation case keeps the full `fr/l fr/ae fr/s` on `laisse,` and adds
  independent `fr/s fr/ee` to the written `se` attack.
- Only numeral==explicit-verse labels are stripped for pronunciation. Per user
  coordination, both `3.Et`/verse-1 mismatches remain unchanged and diagnosed;
  no score-local stanza inference was added.

## Spec Change Log

- 2026-09-09: Created before implementation, following exhaustive source-context
  audit and the user's instruction to fix remaining cases inside Verse.
- 2026-09-09: Implemented and verified against both masters; 480 baseline warnings
  resolved and 51 wrong applied readings corrected. Canonical location moved by
  the parent without changing the frozen contract. Awaiting parent review/commit.

## Verification

Verification on 2026-09-09 (Cargo jobs capped at 2):

- 40 French integration tests, including both adapters, exact phonetic assertions,
  immutable source objects, repeated attacks, actual source extensions, packed
  vowels, matching/mismatched verse labels, complete words across rests, orphan
  End slots, manual input, and positive/negative polyphonic provenance.
- 26 syllabic-word integration tests. Two legacy rest expectations were updated
  to the explicit FR-004 independent-hint contract; unbound rests still cannot
  establish a layout, and no native `+` crosses silence.
- 263 engine unit tests; 12 English pronunciation tests; 1 language-fidelity test;
  10 source-fidelity tests. Optional external fixture gates were not configured.
- Rust format check and all-target Clippy with warnings denied; `git diff --check`.
  Parent owns the consolidated gate, review and commit. No commit or push here.

Private artifacts under `fr-004/exports-final/` and `fr-004/source-context-final/`
contain fresh exports and source snapshots. `fr-004/` also contains
reproduction scripts and per-source-ID accounting. All 482 original warnings
map to source IDs: 480 now have actual applied hints; two mismatched `3.Et` labels
remain unsupported. All 51 separately audited wrong applied hints match their
expected readings. The preceding consonant in the punctuation case is unchanged.
Complete source lyric objects, source chronology and all 3,124 projected note
geometries match the baseline audit. Original MSCZ SHA-256 values are unchanged.

Installed Millefeuille validation reports 3,094 groups / 3,124 notes, all 3,107
expected attacks present, no rejected hints and no unsupported symbols. Full
musical-field comparison with the previous corrected exports preserves timing,
nominal pitch, pitch points, vibrato, curves, tempo, meter and other non-lyric
fields. Packed-vowel `ProcessWord` checks confirm both vowel nuclei survive on
the authored note and all real holds retain hold semantics.

The 482 baseline measures diagnostic coverage, not acoustic failure. No acoustic
render/listening validation was performed. Exact internal timing of packed
phonemes and the audible sung-schwa interpretation remain listening checks.

### Parent review corrections and verification

All nine numbered findings were confirmed and patched, together with the
requested preexisting melisma verse guard. Exact layouts now require a terminal
word boundary, and punctuation `se` cannot claim the beginning of `secret`.
Touching true holds may precede a rest inside a completely bound exact word;
notes, holds and silence remain unchanged. Source-context liaison keeps complete
word identity; all untexted geometry participates in ambiguity checks. Blank
selection is shared with projection. Joined words and standalone verse labels
require score-established verse evidence. New tests include isolated missing
and mismatched chord IDs, actual MSCX/MusicXML chord ownership, and negative
cross-verse melisma evidence.

Review gates passed: 352 focused tests (263 engine, 40 French, 26 syllabic,
12 English, 1 language fidelity, 10 source fidelity), all-target Clippy with
warnings denied, formatting and diff checks. The engine run includes the parent's
USTX sign/control/quote patch; its tested SHA-256 is recorded separately.

Private review artifacts are under `fr-004/exports-review/` and
`fr-004/source-context-review/`; `fr-004/review-verification.md` maps every finding
to its tests and links reproduction evidence. Reverification retains 480/482
actual repairs and all 51 corrected wrong readings, with the same two protected
`3.Et` mismatches. All preexisting source fields and 3,124 projected note geometries
match the baseline; the only added source field is typed verse provenance.
Native validation remains 3,094 groups / 3,124 notes / 3,107 expected attacks,
13 packed-vowel groups, no missing attacks or rejected/unsupported symbols.
All non-lyric YAML fields and original master hashes match. These counts concern
exported material; the parent's separate omission/ledger audit is outside this
patch. No acoustic validation, commit, or changes to user originals were made.

## Suggested Review Order

- Recover context from original ownership without moving source notes.
  [convert.rs:679](../../src-tauri/src/engine/convert.rs#L679)

- Preserve every written attack and guard word boundaries.
  [french.rs:603](../../src-tauri/src/engine/target/french.rs#L603)

- Require score provenance before removing a verse label.
  [french.rs:550](../../src-tauri/src/engine/target/french.rs#L550)

- Verify adapter-generated chord ownership, including negative cases.
  [french_phonetics.rs:860](../../src-tauri/tests/french_phonetics.rs#L860)

Parent final integration check: all-target Rust tests passed after review patches. The prior full frontend/version/build gate remains unchanged. This closes pronunciation scope only; separately identified source-only spans and ledger evidence remain tracked under FID-001.
