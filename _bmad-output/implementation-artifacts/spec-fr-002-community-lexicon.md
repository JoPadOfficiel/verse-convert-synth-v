---
title: "FR-002: Broad French community lexicon"
type: feature
created: 2026-09-09
status: implemented
---

# FR-002: Broad French community lexicon

Status: implemented and reviewed; listening remains pending. Scope authorized 2026-09-09.

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

## Implementation evidence

The pinned importer accepts 104,943 keys, excludes 125 source rows, and
quarantines 27 conflicting/invalid keys. It preserves 40 duplicate source rows
as provenance rather than overwriting their evidence. Runtime uses lazy indexes,
NFC normalization and curated readings/layouts first. Numbered variants are
literal choices. Bare elisions, incomplete word fragments and known ambiguous
homographs stay unforced. Complete source words use a whole-word hint and native
syllable markers only when the reading supplies exactly the stated attacks.

No external G2P runtime was added. eSpeak/Phonemizer would require another alias
adapter and alignment policy; their installation would not by itself improve
the supported-bank contract. Audible evaluation remains separate.

Historical FR-002 validation: 19 French integration tests passed, including the review regressions
for unilateral dashes and punctuation parentheses. Both private score exports
retain all matching musical data, with nine separately source-justified recovered
notes. Installed OpenUtau allocation accepts 2,997 groups / 3,124 notes with all
3,107 expected attacks present; no rejected or unsupported symbols. The lexical
policy supplies 2,625 attacks and diagnoses 482 unresolved occurrences.

[FR-004](spec-fr-004-contextual-sung-readings.md) adds bounded contextual sung
layouts and original-source recovery for polyphonic members. Its source-ID audit
resolves 480 of those warnings and corrects 51 previously applied readings;
two mismatched verse labels remain diagnosed. Current allocation covers all
3,107 expected attacks across the same 3,124 notes, with unchanged musical fields.
This remains symbol/allocation evidence, separate from acoustic validation.
