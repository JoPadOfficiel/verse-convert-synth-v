---
title: "FR-002: Broad French community lexicon"
type: feature
created: 2026-09-09
status: ready
---

# FR-002: Broad French community lexicon

Status: ready for implementation after FR-001. Scope authorized 2026-09-09.

## Intent

Replace the small diagnostic vocabulary with broad offline dictionary coverage
while retaining FR-001's protections for sung syllables and source ownership.
More entries must not make fragment or liaison decisions less reliable.

## Design and boundaries

- Import the compatible entries from the OpenUTAU French community dictionary
  using a pinned upstream revision, SHA-256, original license and reproducible
  import/check script. Record accepted and excluded counts and reasons.
- Translate only explicitly supported symbols to Millefeuille. Reject unmapped
  readings rather than guessing an acoustic token. Keep alternate numbered
  readings distinct; variant numbers do not define universal liaison rules.
- Use indexed lookup initialized once, not a full dictionary scan for each note.
- Normalize canonical Unicode, case and edge punctuation for lookup while
  preserving the exact original lyric evidence and internal elisions.
- Keep curated source layouts higher priority. Whole-word dictionary readings
  cannot be applied blindly to the fragments of another word. Unknown layouts
  retain text and diagnostics. Phrase-dependent homographs remain ambiguous.
- Support safe French elisions using a recognized remainder; do not pronounce
  an arbitrary apostrophe as a consonant or infer liaison across punctuation.
- No runtime network requirement, voicebank edit, model replacement or claimed
  exhaustive grammar. Evaluate other primary lexical/G2P sources before adding
  dependencies; additional runtimes need a concrete benefit over the embedded
  OpenUtau fallback and the community lexicon.

## Acceptance

The import is reproducible and every emitted dictionary symbol is supported by
the documented convention. Verify representative silent endings, pronounced
exceptions, nasal vowels, contractions, NFC/NFD spellings, multiple syllables,
homographs, unknowns and manual hints. Preserve FR-001's full matrix. Compare
coverage across all vocal lanes of the two private scores, and report unresolved
occurrences rather than equating dictionary membership with perfect singing.
No change in note identity, timing, pitch/vibrato or default/SVP output.

## Sources

- https://github.com/mmemim/OpenUTAU-French-Dictionary
- https://github.com/imsupposedto/Millefeuille-DiffSinger-French
- https://github.com/openutau/OpenUtau/wiki/Phonemizers
- https://github.com/bootphon/phonemizer
- https://github.com/espeak-ng/espeak-ng
