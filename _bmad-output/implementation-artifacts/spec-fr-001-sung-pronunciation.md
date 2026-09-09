---
title: "FR-001: French sung pronunciation"
type: bugfix
created: 2026-09-09
status: implemented
---

# FR-001: French sung pronunciation

Status: implemented and reviewed; acoustic listening remains pending. Original baseline:
`6558ea9da7d7b4dc60b3dbf18f7ab60c86f2f73c`.

## Intent

French score conversion must retain every source-owned sung attack while
providing the French DiffSinger Millefeuille phonemizer with appropriate
pronunciation. Punctuation, case and incomplete syllabic metadata currently
produce unwanted final consonants, merged unrelated words and lost notes.

## Contract

- Offer an explicit French Millefeuille profile across analysis, direct export
  and bundle export. Choose the pronunciation convention, never a singer.
- Preserve notes, onset, duration, nominal pitch, repetitions, lyric lanes,
  original text and source bytes. Keep existing pitch/vibrato defaults unchanged.
- Retain manual phonetic hints and true holds. Repeated vowel attacks and a
  separately written sung schwa remain separate notes.
- Use bounded verified readings and layouts, with diagnostics for unresolved
  input. Never strip final s/x/t/d mechanically or substitute raw `un` with `in`.
- Never join unrelated words from an isolated syllabic end. Never carry context
  across a rest, source lyric lane, explicit word boundary or repeat occurrence.
- Resolve eligible blank/text duplicates without replacing meaningful hold
  instructions or borrowing another verse's lyrics.

## Acceptance matrix

| Input | Required result |
| --- | --- |
| chan/ger, mê/me, pres/se | Every sung attack retained with contextual hints |
| rê/ê/ves, vent/en/ent, court/ou/ourt, fond/on/on | Repeated vowels retained; consonants placed on their intended attack |
| rêves, blessures, amours, murs, genoux, vent | Verified silent endings, independent of punctuation and case |
| un, Un, d'un | Supported nasal pronunciation and preserved elided consonant |
| tout/au, mes/amours and supported split forms | Liaison belongs to the following attack |
| tem/pê/tes followed by amis | Final syllable is not mistaken for determiner tes |
| des/tin, unknown layouts, bus, ambiguous h | No unverified whole-word correction to a fragment |
| explicit hints, holds, contradictory metadata | Preserve instructions; diagnose ambiguity |

## Verification

Exercise native MuseScore and MusicXML, SAB voices, source duplicates, repeats,
idempotence, source provenance and default/SVP isolation. Verify command profile
propagation and bundle diagnostics. Run frontend tests/build/version checks and
Rust tests/format/Clippy. Compare private exports with their own source and raw
export; a different manually corrected arrangement is not a universal oracle.
Check emitted hints against the installed phonemizer separately from listening.
