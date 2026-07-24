# Changelog

All notable changes to Verse are documented in this file.

The project follows [Semantic Versioning](https://semver.org/), and release
entries are maintained by Release Please from Conventional Commits.

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
- Preserve repeat occurrences, tempo and meter changes, grace notes, MIDI text
  bytes, and MusicXML elisions without heuristic reassignment.
- Reject malformed or ambiguous timing and pitch data instead of silently
  substituting musical values.

### Security

- Bound archive/XML parsing and external rendering.
- Validate rendered audio and commit preservation bundles transactionally.
- Pin release workflow actions to immutable commits and verify tag, commit, and
  application versions before packaging.

[0.2.0]: https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.1.4...v0.2.0
