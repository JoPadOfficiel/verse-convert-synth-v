---
title: Resolve French sung fragments from their written context
type: bugfix
created: 2026-09-09
status: in-progress
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
data. `french.rs` already owns exact layouts and their hints. The current layout
matcher rejects all premature End metadata, even for known sung vowel echoes.
Whole-word fallback requires exactly as many vowels as attacks, missing explicit
sung-schwa/repetition layouts. `standalone_allowed` rejects incomplete metadata;
relax only through positive contextual evidence.

`project_track` applies profile pronunciation per part/staff/voice/occurrence;
source tracks may already be split into polyphonic members. Any recovery of
context must prove those domains from source fields. It must not move lyrics or
change lane decomposition. The sibling FR-003 change owns output-only edge-dash
cleanup in `ustx.rs`; avoid modifying that work.

## Tasks and Acceptance

- [ ] Implement contextual rules covering every audited resolvable class,
  including incorrect existing hints. Add generic synthetic fixtures, not songs.
- [ ] Resolve the polyphonic lexical-context case from original source identity
  where unambiguous, with negative independent-voice/row/repeat coverage.
- [ ] Add positive/negative tests for the matrix; preserve complete source objects
  and musical fields. Explicitly test multi-vowel hints plus real holds.
- [ ] Reconvert both masters to a new private directory, compare all 3,124 notes
  with the previous corrected exports, and account for each original warning by
  stable source ID. Do not relabel an unresolved case as applied without a reading.
- [ ] Run installed Millefeuille symbol/allocation validation and verify every
  expected source attack survives. Report acoustic validation separately.
- [ ] Update public specifications and French guidance with current limits and
  actual coverage. Parent owns consolidated gate and commit.

## Design Notes

Use the audited dictionary pronunciations, with explicit known French inflection
or sung-layout additions where absent. Choose the smallest implementation that
meets the matrix; if better equivalent evidence handling emerges, preserve the
frozen contract and record it. No network or custom model installation is needed.

## Spec Change Log

- 2026-09-09: Created before implementation, following exhaustive source-context
  audit and the user's instruction to fix remaining cases inside Verse.

## Verification

Pending. The 482 baseline is diagnostic coverage, not an acoustic failure count.
