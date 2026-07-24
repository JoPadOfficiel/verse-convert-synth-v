# Changelog

All notable changes to Verse are documented in this file.

The project follows [Semantic Versioning](https://semver.org/), and release
entries are maintained by Release Please from Conventional Commits.

## [0.3.0](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.2.0...v0.3.0) (2026-07-24)


### Features

* add source-faithful preservation bundles ([8b9b15d](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/8b9b15d674dae4e60e5d311e2d85da6e52188be4))
* **engine:** MIDI and karaoke parsing with score playback unrolling ([37a5e4d](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/37a5e4d47ffe55c074ceaeae1add15f065ffee68))
* **engine:** multi-track conversion, voice detection and .svp serialization ([20720b2](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/20720b2cde6c99d1c26d86de763898b2f9f05b7e))
* **engine:** MusicXML and native MuseScore importers with repeats, voltas and D.S./D.C. unrolling ([1f9a3e7](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/1f9a3e7b15663c895f5c6ddadf2e7dc640dedd58))
* **engine:** recognize French instrument and voice names in track classification ([2caaa61](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/2caaa61ec4b8f6b5d34dbfd5b514c9daa8faedf0))
* per-file Download opens a Save dialog with explicit destination ([bf172a1](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/bf172a1c0371cd2f3ff8078a276ebfa26d5ffa41))
* Tauri command layer with per-track overrides and input hardening ([2329d73](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/2329d7351d1e79d263b3ebb4431bd27bd0789f08))
* **ui:** direct per-file Download button ([d50ca52](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/d50ca523f2b95b34abcf32dc29f38d5fe9eaed8a))
* **ui:** shadcn interface with light/dark theming, batch conversion and Sings/Muted toggles ([5a1b1f3](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/5a1b1f3b9df287e91172ee5a9d1213ac3f6f5266))
* **ui:** show the saved .svp path after conversion ([2786f67](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/2786f67d43251177f624e1617d829ae9ce5121c5))


### Bug Fixes

* **engine:** robust rich-text extraction across all score formats ([0e7abfd](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/0e7abfdf04e7822186a74156cb616f7cc8dfdea2))
* harden source-faithful conversion and release ([74362e8](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/74362e80752907fc85f80ad773a0cfb2a7b475f6))
* **musescore:** read lyrics wrapped in inline formatting elements ([2fc142d](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/2fc142df5d2a326979550d0f4e4f8d8134420e35))

## [0.2.0] - 2026-07-24

### Added

- Preservation bundles containing the byte-identical source, editable vocal
  project, full-score reference audio, manifest, checksums, and disposition
  ledger.
- Native source-fidelity coverage for MIDI/KAR, MusicXML/MXL, and
  MuseScore/MSCZ projects.
- Explicit per-track vocal export overrides and separate projection of every
  source lyric lane.

### Fixed

- Preserve source-owned lyrics instead of filling missing syllables with
  synthetic `la` text.
- Keep instrumental and percussion material in the full-score reference mix
  without inventing vocal notes or pitches.
- Preserve supported repeat occurrences, offset/metronome tempos, additive
  meters, ties, grace notes, MIDI text bytes, and MusicXML elisions without
  heuristic reassignment; unsupported navigation now fails instead of
  truncating playback.
- Reject malformed or ambiguous timing and pitch data instead of silently
  substituting musical values.

### Security

- Bound archive/XML parsing and external rendering.
- Validate rendered audio and commit preservation bundles transactionally.
- Pin release workflow actions to immutable commits and verify tag, commit, and
  application versions before packaging.

[0.2.0]: https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.1.4...v0.2.0
