# Changelog

All notable changes to Verse are documented in this file.

The project follows [Semantic Versioning](https://semver.org/), and release
entries are maintained by Release Please from Conventional Commits.

## [0.6.2](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.6.1...v0.6.2) (2026-08-25)


### Bug Fixes

* three shapes a score writes that the conversion read wrong ([#29](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/29)) ([7d4d0e8](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/7d4d0e8c10f55123e986e008411bb6bd6b7b69a1))

## [0.6.1](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.6.0...v0.6.1) (2026-08-23)


### Bug Fixes

* sing the word a score spells across several notes ([#27](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/27)) ([c4674e5](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/c4674e519a7ed4c2192f5a7d99c57f339e4c4ae1))

## [0.6.0](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.5.0...v0.6.0) (2026-08-18)


Regenerating a project from this release does not reproduce the previous file:
note portamento and MuseScore part names both change. All of it landed in
[#25](https://github.com/JoPadOfficiel/verse-convert-synth-v/pull/25), squashed
into one commit
([7741de7](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/7741de79de2c4ad44a2c287283602582edab0f5c)).

### Bug Fixes

* **openutau:** state the opening tempo, so the file opens at all.
  `TimeAxis.BuildSegments` throws `First tempo must be at tick 0.` and abandons
  the load without saying why. 25 of 2645 corpus scores state their first tempo
  mark after the start and wrote exactly that map.
* **openutau:** give a note the portamento OpenUtau gives every note. The two
  pitch points were `±1 ms`, which is OpenUtau's own `Snap` preset — deliberately
  no portamento — while `UProject.CreateNote` uses `±40 ms`. Measured against a
  built 0.1.568, the rendered f0 stepped in one sample where the default glides
  over ~83 ms. That f0 is the tensor DiffSinger sings from.
* **engine:** split a voice that sounds two notes at once into one lane each. A
  vocal lane is monophonic in both targets; 21 lanes across 17 corpus scores were
  not. Synthesizer V sang one note of the stack and OpenUtau refused the export.
  The split runs on the lane that is sung, so an instrumental part is untouched.
* **engine:** prove the monophonic lane at the analysis gate rather than trust
  it, with its own wording — a lane sounding two notes at once is not a timing
  fault.
* **musescore:** keep the part names the author wrote. Names came from
  `trackName`, the template's instrument, so a score whose three sung parts were
  left as `Piano` produced three identical lanes while the author's names sat in
  `longName`.
* **musescore:** read a lyric extension written only as `ticks_f`. The unit was
  measured, not assumed: all 10968 corpus lyrics stating both agree on
  `ticks = ticks_f × 4 × Division`.
* **musicxml:** merge a tie only where the head ends exactly where the tail
  begins, the contiguity `musescore.rs` already required. A tie reopened across a
  rest sustained a note the score never sustains. A tail with nothing to merge
  into keeps its own pitch instead of leaving the score.
* **midi:** read a track's words once when it writes them in both encodings, and
  let the lyric event win over generic karaoke text. A track qualifies as karaoke
  on a line control plus two payloads, which two section markers satisfy: reading
  the text stream there sang `Chorus` and left seven of eight real words out.


### Features

* **ui:** say which source carries the most, where the source is chosen. The drop
  target separates scores from MIDI and names the evidence that differs — which
  note owns a syllable, which verse it belongs to, where a word is held.


### Tests

* add a generated MusicXML rhythm corpus and its invariants: four scores written
  by music21 covering 3-, 5-, 7-, 9- and 11-tuplets, a meter change per bar and
  ties across every barline. The generator runs out of CI and its output is the
  fixture, so CI stays offline and Python-free.
* all nine private parity tests pass, a first. Two were recorded as unfixable
  fixture mismatches; the `.mxl` of the same file projects the names the `.mscz`
  was dropping, which refutes that reading.


### Verified

Every projected note of 2645 corpus scores was compared against the file
written for it: 518138 Synthesizer V notes and 507521 OpenUtau notes, with no
divergence in pitch, onset, duration or lyric. Synthesizer V accepts all 2645;
OpenUtau accepts 2613, refusing 30 for a held syllable across a gap and 2 for a
rhythm its fixed 480-tick grid cannot state. Files OpenUtau would refuse to
open: 0, previously 25. Lanes sounding two notes at once: 0, previously 21.

## [0.5.0](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.9...v0.5.0) (2026-07-29)


### Features

* **engine:** move untexted notes to a muted companion lane ([0124b50](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/0124b50d8e2425891162d9305f9b7c9c0cc3bd80))
* OpenUtau target, and a vocal project that matches the score ([#22](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/22)) ([6434774](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/6434774835036315a8814e7ea9c688fac2262a0e))
* put the MuseScore stems inside the OpenUtau project ([2c30426](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/2c30426ccffa82f021bc8168cd3e9e097ec24db0))
* write OpenUtau .ustx projects beside Synthesizer V ones ([b80862a](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/b80862aa101250f96be743708ffda0fde50ee8f7))


### Bug Fixes

* give every macOS MuseScore 4 the score-load cooldown ([579b7e2](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/579b7e280566ffd0e10b2d1b62bb5db398579009))
* inventory a chord's lyric once, not once per lane ([a25b268](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/a25b2684737d4ecee905668b160b1f2c5487bc79))
* keep the bundle reachable under the OpenUtau target ([ea1ae20](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/ea1ae20e719e2cbe96afd854fe4f788b4806870d))
* **musescore:** one sung lane per written line, and a syllable sung once ([d587219](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/d587219fb745063b7f0e069963c274075b059c68))
* never open a bundle with every track muted ([1c79f6c](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/1c79f6cf61086eff0c2c10a8760e34c9341a1e52))
* never write a note with no word into a vocal project ([c5d90e8](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/c5d90e814ba6b0da3c2ba4f2fff2887b9f577657))
* show a warning count without expanding a row ([bea4a47](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/bea4a47170eda1a44e2b952cee7af3d0af2d2b7f))
* stop claiming a voice-database language ([09b95bc](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/09b95bc9c44e4d61c4acb7e471d95a0ad9fd927a))
* **ui:** count informational diagnostics apart from warnings ([1a1ff69](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/1a1ff69edc6190b4007f187db30f540b4a71169a))

## [0.4.9](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.8...v0.4.9) (2026-07-27)


### Bug Fixes

* let the window drive the layout instead of a fixed column ([#20](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/20)) ([afe1ba9](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/afe1ba978bfb63d1a046e86a8c394d5871b1772a))

## [0.4.8](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.7...v0.4.8) (2026-07-26)


### Bug Fixes

* split every source voice and lose no lyric ([#18](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/18)) ([4383c9f](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/4383c9fce95849da8685479799895de4bcf1e28d))

## [0.4.7](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.6...v0.4.7) (2026-07-26)


### Bug Fixes

* divide a MIDI into stems instead of asking MuseScore to ([#16](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/16)) ([c7c28fe](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/c7c28fea2e8e5f637d021b814371eaa7cea23e86))

## [0.4.6](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.5...v0.4.6) (2026-07-26)


### Bug Fixes

* resolve the release branch without fromJson ([#14](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/14)) ([b0abadb](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/b0abadb59061deab29065e45681ac626a272114d))

## [0.4.5](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.4...v0.4.5) (2026-07-26)


### Bug Fixes

* unroll MusicXML repeats score-wide and keep Cargo.lock released ([#12](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/12)) ([08f6977](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/08f6977acf7c66b4449698463f46cdff59665d76))

## [0.4.4](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.3...v0.4.4) (2026-07-26)


### Bug Fixes

* sing the refrain a repeated passage writes once ([#10](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/10)) ([8dc9919](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/8dc9919507bbdae9757c95d9693914c3940d8df5))

## [0.4.3](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.2...v0.4.3) (2026-07-26)


### Bug Fixes

* restore MuseScore projection fidelity — tie merging, score-level repeats, verse-per-pass ([#8](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/8)) ([7587f59](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/7587f59e5424fd29d5ee6ec91c592a8aec4e1287))

## [0.4.2](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.1...v0.4.2) (2026-07-25)


### Bug Fixes

* keep renderer work off shared destinations ([#6](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/6)) ([a6201c1](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/a6201c150154d27e317dd36588b82a50fdc3d5ca))

## [0.4.1](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.4.0...v0.4.1) (2026-07-25)


### Bug Fixes

* stabilize cross-platform conversion and progress ([#4](https://github.com/JoPadOfficiel/verse-convert-synth-v/issues/4)) ([72bc05f](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/72bc05f5c8ec793625edb8b251cf8fcd1f6ab596))

## [0.4.0](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.3.0...v0.4.0) (2026-07-25)


### Features

* preserve source parts and KAR lyrics ([bca0da7](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/bca0da7b32f60ad360a59642df9fca99a7623786))
* source-faithful conversion, track decomposition and release 0.4.0 ([25b488a](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/25b488a4bc2018b35e22c70e6299b667e02f7227))


### Bug Fixes

* finalize conversion paths, documentation and release 0.4.0 ([9d20747](https://github.com/JoPadOfficiel/verse-convert-synth-v/commit/9d207471e992e17e2224d188b15dfeb9c24cf2e3))

## [0.4.0](https://github.com/JoPadOfficiel/verse-convert-synth-v/compare/v0.3.0...v0.4.0) (2026-07-25)

### Added

- Preserve the original Part → staff → source voice → projection-lane
  topology for MusicXML and MuseScore inputs instead of presenting technical
  chord lanes as independent source tracks.
- Bind detached KAR lyric streams to a melody only when the complete mapping
  is unique, injective, monotonic, timing-compatible, and non-percussive.
- Publish `.versebundle` schema v2 with one validated MuseScore WAV stem per
  note-bearing source Part plus a muted full-score reference mix.
- Support one user-installed MuseScore Studio 3.6.2+ or 4.x renderer, including
  strict master-score/Part extraction, executable identity checks, and bounded
  macOS MuseScore 4 shutdown retries.
- Add private multi-format regression gates and a pinned CC0 OpenScore Lieder
  auditor covering 1,352 scores and deterministic real-audio render samples.
- Add complete tracked documentation for architecture, supported formats,
  bundle schema, renderer setup, IPC contracts, development, testing,
  security, troubleshooting, deployment, and contribution.

### Fixed

- Keep genuinely lyricless notes empty instead of generating `la`, Japanese
  text, continuation markers, fallback pitches, or a synthetic C4 melody.
- Apply the `.kar` container profile consistently during analysis, direct SVP
  export, and complete bundle export.
- Report `nTracks` as the actual number of detailed projection lanes rather
  than copying the source Part count.
- Reject source timing that cannot map to integral Synthesizer V blick
  positions instead of silently rounding note or tempo positions.
- Preserve every source lyric lane, formatted MuseScore text, source voice,
  rest-only topology item, percussion identity, and ambiguous chord lyric as
  source evidence without inventing a target representation.
- Prevent incomplete, silent, misaligned, duplicate, oversized, or
  identity-mismatched Part renders from publishing a partial bundle.

### Reliability and compatibility

- Validate complete Part coverage, WAV headers/frame alignment, SHA-256
  identities, relative SVP audio references, preservation-ledger dispositions,
  and the final no-replace bundle after publication.
- Bound source parsing, MuseScore extraction, renderer logs/process trees,
  per-WAV size, aggregate audio size, Part count, and total render time.
- Keep analysis and vocal-only export available without MuseScore; fail
  complete bundle export explicitly when no compatible renderer is available.
- Build release artifacts for macOS, Windows, and build-qualified Linux on
  ARM64 and x86_64 with stable names and `SHA256SUMS`.

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
