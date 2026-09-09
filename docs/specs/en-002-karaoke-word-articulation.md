---
title: "EN-002: Karaoke words across performed note gaps"
type: feature
created: 2026-09-09
status: planned
---

# EN-002: Karaoke words across performed note gaps

## Evidence and current protection

A qualified KAR source can spell one word in several text events, with leading
spaces or line controls marking word boundaries. The audited `Li` / `ving`
sequence spells `Living`, but its performed notes do not touch. The independent
CMU key `li` is a different lexical entry and must not supply its pronunciation.

EN-001 protects this case: when encoded source lyrics demonstrate both line
controls and whitespace boundaries, adjacent fragments without a word boundary
remain unforced and receive source-linked diagnostics, even across a note gap.
This does not alter source notes or reinterpret ordinary XML lyric records.

## Subsequent implementation contract

- Establish word boundaries from qualified karaoke metadata, including leading
  and trailing spaces, line controls, text-event ownership and encoding.
  Do not concatenate unrelated MIDI meta text or infer words solely from the
  existence of a dictionary entry.
- Resolve the complete word and allocate its pronunciation to source syllable
  attacks. A gap between performed notes is not permission to lengthen a note.
- Native OpenUtau `+` requires a connected group; choose and verify an explicit
  per-note articulation representation when a word spans a real gap. Preserve
  consonant timing, vowel attacks and the original lyric evidence.
- Verify contractions, repeated vowels, breath controls, word/phrase boundaries,
  polyphonic note assignment, tempo changes and short staccato gaps. Unknown
  allocations remain diagnosed.
- Validate the resulting allocation with the installed phonemizer and compare
  audio before claiming a complete pronunciation repair.

This enhancement is specified, not implemented. EN-001 ships the conservative
fragment guard and keeps this limitation visible instead of forcing a different
word's dictionary reading.
