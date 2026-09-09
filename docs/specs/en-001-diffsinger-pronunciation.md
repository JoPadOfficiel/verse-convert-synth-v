---
title: "EN-001: Explicit English DiffSinger pronunciation"
type: feature
created: 2026-09-09
status: implemented
---

# EN-001: Explicit English DiffSinger pronunciation

Status: implemented and checked; listening and EN-002 remain separate.
Scope authorized 2026-09-09. No acoustic rendering or listening validation is
claimed by this implementation.

## Intent

Apply the same source-preservation discipline to English lyrics and to compatible
multilingual DiffSinger banks. DiffSinger alone does not establish automatic
language detection or a universal phoneme alphabet.

## Contract

- Explicit profile `EnglishArpabet`, serialized as `englishArpabet`, selects
  `OpenUtau.Core.DiffSinger.DiffSingerEnglishPhonemizer` (DIFFS EN). The audited
  installed version is `1.0.0+3f213e8993ca792c3e6f8958c92ab27eae78eac5`.
  Its 39 stressless CMU ARPAbet phones use the `en/` namespace and match the
  audited UFR inventories. This does not select DIFFS EN+ or infer a language.
- Carry selection through analysis, direct and bundle export, with clear
  compatibility guidance. Keep existing Default and French profiles stable.
- Preserve apostrophes in contractions, manual hints, lyric lanes, repeats and
  every source-owned attack. Clean lookup punctuation without changing the
  source. Never apply French silent-ending rules to English plurals or past tense.
- For explicit syllabic words, use a verified word pronunciation and check its
  allocation over notes. Do not merge unrelated words from one-sided metadata or
  assume that a vowel count alone establishes the source's sung realization.
- Use a redistributable pinned CMU lexicon indexed once with `OnceLock`.
  Verse performs no runtime G2P. Validate the entire emitted hint against the
  39-phone inventory and require at least one vowel; never silently drop an
  invalid symbol. Explicit user hints and control aliases remain unchanged.
- Unknowns and unsupported layouts retain evidence and a diagnostic. Do not
  promise compatibility with banks using a different acoustic alphabet.

## Acceptance

Test case/punctuation around contractions such as `She's` and `don't`; singular
and plural final consonants; silent letters; multi-note words, repeated vowels,
holds, rests, source row/repeat boundaries and conflicting metadata. Verify
phonemizer allocation with the installed OpenUtau version without claiming that
symbol compatibility proves audible quality. Preserve all musical identity and
source bytes, with parity across public command paths.

## Dictionary and reading policy

The generated asset contains 135,166 distinct keys, including 9,114 explicit
numbered variants, from CMU commit
`74790861f652b15e4ac49015a90074ad62a27690`. The original source SHA256 is
`81917843c7f44ce2b094ac63873c2c7a4cf802040792c455ba3ca406891c3d22`.
[Provenance and reproduction](../english-lexicon-provenance.json) records the
exact import, mapping, source rows and asset hash. The bundled
[CMU license](../../public/licenses/cmudict.txt) retains the original notices and
is linked from Settings. This dictionary describes American English; it does
not settle every dialect, semantic ambiguity or sung realization.

Lookup normalizes NFC, case, typographic apostrophes and outer punctuation while
retaining contractions and explicit numbered variant keys. Original spelling,
punctuation, metadata and source bytes remain available as source evidence.
The base homographs `read`, `wound`, `lead`, `live`, `bow`, `tear`, `wind`, `bass`,
`close`, `desert`, `present`, `object`, `record`, `content` and `minute` remain
unhinted with `ENGLISH_PRONUNCIATION_AMBIGUOUS` unless an explicit variant or
manual hint was supplied. The runtime never chooses another variant by matching
vowel counts. Eight consonant-only dictionary readings remain in the asset but
cannot supply an automatic sung hint.

Complete bilateral source syllable chains are resolved before standalone words.
A recognized reading must contain exactly as many vowels as source syllable
attacks. The head note carries the complete word and phoneme hint; following
syllables emit native `+`. Actual extensions emit `+~` and consume no additional
vowel. Source-owned notes, timings, pitches and original lyrics remain intact.
Rest gaps, lyric rows, voice domains, repeats, phrase endings, manual hints and
unknown split markers prevent unsafe reconstruction.

Unknown coherent fragments cannot receive independent whole-word readings.
Encoded karaoke fragments are also protected across performed gaps when the
source demonstrates line controls and whitespace word boundaries. Reconstructing
their full articulation across those gaps is tracked in [EN-002](en-002-karaoke-word-articulation.md).
Orphan `Begin`, `Middle` or `End` metadata also retains source text and receives
an unsupported diagnostic. Standalone dictionary lookup requires absent or
`Single` syllabic metadata. Unknown entries and vowel-count mismatches remain
reviewable through `ENGLISH_PRONUNCIATION_UNSUPPORTED` and
`ENGLISH_PRONUNCIATION_VOWEL_MISMATCH` respectively. Successful changes carry
`ENGLISH_PRONUNCIATION_APPLIED` and their source note IDs.

Both explicit profiles share the guarded source-duplicate selection: only a
truly blank record can yield to a meaningful record. Continuations and explicit
syllable splits retain priority, conflicting meaningful records are diagnosed,
and playback-specific eligibility preserves time-only precedence. English uses
`ENGLISH_DUPLICATE_BLANK_RESOLVED` and `ENGLISH_DUPLICATE_LYRIC_CONFLICT`.

## Verification and handoff

Targeted checks completed on 2026-09-09:

- `english_phonetics`: 12 integration tests passed across MusicXML and native
  MuseScore SAB fixtures. They cover contractions/case/punctuation, silent and
  plural endings, `beautiful` across three attacks with and without a hold,
  variants, semantic homographs, mismatched vowel counts, unknown/orphan
  fragments, unilateral dashes, outer parentheses and numbered variants,
  manual controls, rest/row/repeat boundaries and deterministic
  reapplication. Assertions preserve source evidence, musical fields, Default
  USTX and SVP behavior.
- `english_analysis_batch_and_direct_exports_share_the_bundle_projection`
  passed: analysis, batch writes, direct export and actual
  `export_bundle_blocking` produce the same vocal projection and profile;
  manifest diagnostics survive. It uses the existing fake renderer seam and
  performs no installed acoustic rendering.
- Both `explicit_profiles_duplicate` tests passed for French and English,
  including conflicting meaningful states and playback-specific priority.
- The runtime hint-validation test passed for whole-reading rejection of
  unknown phones, a foreign namespace, stress suffixes and missing vowels.
- `tests/pronunciation-contract.test.mjs`: 8 tests passed. All command adapters
  carry the exact three profile values. Same-target reanalysis adopts returned
  per-file verdicts, including unsupported warnings; rejected commands retain
  the previous selection and diagnostics, and the busy guard prevents overlap.

Consolidated Rust/frontend checks and installed OpenUtau allocation checks are
recorded at final integration. These establish the selected class, alphabet and
`+`/`+~` allocation convention; they do not establish audible quality. Compatible
bank assignment and review of unsupported or ambiguous lyrics remain necessary.
