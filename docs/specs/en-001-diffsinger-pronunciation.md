---
title: "EN-001: Explicit English DiffSinger pronunciation"
type: feature
created: 2026-09-09
status: planning
---

# EN-001: Explicit English DiffSinger pronunciation

Status: compatibility investigation. Scope authorized 2026-09-09.

## Intent

Apply the same source-preservation discipline to English lyrics and to compatible
multilingual DiffSinger banks. DiffSinger alone does not establish automatic
language detection or a universal phoneme alphabet.

## Contract

- Add an explicit English DiffSinger profile only after confirming its exact
  OpenUtau phonemizer class, dictionary convention and installed UFR inventory.
- Carry selection through analysis, direct and bundle export, with clear
  compatibility guidance. Keep existing Default and French profiles stable.
- Preserve apostrophes in contractions, manual hints, lyric lanes, repeats and
  every source-owned attack. Clean lookup punctuation without changing the
  source. Never apply French silent-ending rules to English plurals or past tense.
- For explicit syllabic words, use a verified word pronunciation and check its
  allocation over notes. Do not merge unrelated words from one-sided metadata or
  assume that a vowel count alone establishes the source's sung realization.
- Prefer existing OpenUtau English G2P and a redistributable, pinned CMU lexicon
  over a new runtime. Verify any emitted hint against the target convention.
- Unknowns and unsupported layouts retain evidence and a diagnostic. Do not
  promise compatibility with banks using a different acoustic alphabet.

## Acceptance

Test case/punctuation around contractions such as `She's` and `don't`; singular
and plural final consonants; silent letters; multi-note words, repeated vowels,
holds, rests, source row/repeat boundaries and conflicting metadata. Verify
phonemizer allocation with the installed OpenUtau version without claiming that
symbol compatibility proves audible quality. Preserve all musical identity and
source bytes, with parity across public command paths.
