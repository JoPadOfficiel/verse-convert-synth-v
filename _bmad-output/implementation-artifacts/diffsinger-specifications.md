# DiffSinger pronunciation and expression work

These specifications separate musical fidelity, language processing and the
voicebank's acoustic interpretation. The user authorized preserving the existing
French work, continuing corrections, and making separate commits on 2026-09-09.
Earlier research and private musical examples remain local and are not fixtures
in the public repository.

| Specification | Purpose | Current stage |
| --- | --- | --- |
| [FR-001](spec-fr-001-sung-pronunciation.md) | French sung syllables, silent endings, liaison, source ownership | Implemented and reviewed; listening pending |
| [FR-002](spec-fr-002-community-lexicon.md) | Broad, reproducible French dictionary coverage | Implemented; corpus and allocation validated; listening pending |
| [FR-003](spec-fr-003-syllable-hyphen-cleanup.md) | French USTX edge syllable separators, independent of lexical coverage | Completed and independently reviewed; consolidated gate passed |
| [FR-004](spec-fr-004-contextual-sung-readings.md) | Written context for sung fragments and polyphonic members | Completed and reviewed; 480/482 warnings resolved, 51 incorrect readings corrected |
| [EN-001](spec-en-001-diffsinger-pronunciation.md) | Explicit English DiffSinger pronunciation | Implemented; source-fragment guards and integration tests validated |
| [EN-002](spec-en-002-karaoke-word-articulation.md) | Karaoke word articulation across performed gaps | Specified; fragments protected by EN-001 |
| [EXP-001](spec-exp-001-expression-fidelity.md) | Pitch, interpolation, vibrato and volume provenance | Source/history audit complete; broader import remains separate |
| [EXP-002](spec-exp-002-midi-performance-curves.md) | Explicit MIDI/KAR pitch bend and volume/expression curves in USTX | Implemented and reviewed; native load/sampling and final 264-case aggregate pass ([evidence](exp-002/iteration-1-verification.md)) |
| [EXP-003](spec-exp-003-numeric-score-performance.md) | Active score dynamics, hairpins and authored fades | Review limit reached (counter6); iteration5 gates pass, additional validation/route corrections recorded; awaiting human decision, code preserved |
| [FID-001](spec-fid-001-projected-note-evidence.md) | Evidence from notes actually retained in the editable project | Implemented and reviewed; integrated 514 Rust tests and Clippy passed; musical output unchanged |
| [FID-002](spec-fid-002-source-sung-continuity.md) | Source-proven melisma routing and verse-aware tied holds | Review limit reached (counter6); source-proven restorations and iteration5 fidelity gates preserved; further proof/gate corrections await human decision |
| [FID-003](spec-fid-003-standalone-linked-staves.md) | Standalone MuseScore staves with external master links | Done; reviewed integration, 529 Rust tests and pinned public corpus pass |

Each implementation must distinguish automated structural verification from
phonemizer integration and listening. A finite dictionary cannot guarantee all
words, homographs, dialects or sung realizations. Unsupported cases must remain
visible rather than silently receive an invented pronunciation.

Keep the default and SVP paths compatible, preserve original score bytes, and
never silently transpose a part or overwrite a manually edited USTX.

The remaining fidelity and intensity increments above are specified and in progress; their completion is not implied by the already-passing pronunciation gate.
