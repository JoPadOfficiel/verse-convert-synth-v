# DiffSinger pronunciation and expression work

These specifications separate musical fidelity, language processing and the
voicebank's acoustic interpretation. The user authorized preserving the existing
French work, continuing corrections, and making separate commits on 2026-09-09.
Earlier research and private musical examples remain local and are not fixtures
in the public repository.

| Specification | Purpose | Current stage |
| --- | --- | --- |
| [FR-001](fr-001-sung-pronunciation.md) | French sung syllables, silent endings, liaison, source ownership | Implementation under review |
| [FR-002](fr-002-community-lexicon.md) | Broad, reproducible French dictionary coverage | Ready for implementation after FR-001 |
| [EN-001](en-001-diffsinger-pronunciation.md) | Explicit English DiffSinger pronunciation | Compatibility investigation |
| [EXP-001](exp-001-expression-fidelity.md) | Pitch, interpolation, vibrato and volume provenance | Source/history audit complete; broader import remains separate |

Each implementation must distinguish automated structural verification from
phonemizer integration and listening. A finite dictionary cannot guarantee all
words, homographs, dialects or sung realizations. Unsupported cases must remain
visible rather than silently receive an invented pronunciation.

Keep the default and SVP paths compatible, preserve original score bytes, and
never silently transpose a part or overwrite a manually edited USTX.
