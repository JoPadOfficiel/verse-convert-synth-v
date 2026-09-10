---
title: Remove score syllable hyphens independently of lexical coverage
type: bugfix
created: 2026-09-09
status: done
baseline_commit: 425dab11208c3a8663b41aaa41defe601bac13aa
review_loop_iteration: 0
context:
  - docs/contribution-guide.md
  - _bmad-output/implementation-artifacts/spec-fr-001-sung-pronunciation.md
  - src-tauri/src/engine/projection.rs
  - src-tauri/src/engine/target/ustx.rs
  - src-tauri/src/engine/target/lexical.rs
  - src-tauri/src/engine/syllable.rs
  - src-tauri/tests/french_phonetics.rs
---

<frozen-after-approval>

## Intent

The user's original specification explicitly requires removing typographic
syllable hyphens from the OpenUtau lyric on each original note. Current French
pronunciation removes these only after finding a supported reading. An unknown
fragment such as `chan- / ter` retains the hyphen, forcing manual cleanup even
though removing that separator needs no pronunciation guess.

Decouple this export spelling repair from dictionary success. For the explicit
French Millefeuille profile, render edge syllable separators without them even
when the pronunciation remains unresolved. Keep source evidence and every note
unchanged. Known layouts such as `chan- / ger`, `pres- / se`, `mê- / me`, and
`rê- / ves` must retain one phonetic attack on each note, not join the whole word
onto its first note. Internal lexical hyphens are not edge separators.

This is one bounded fix in the larger investigation. It does not claim that
unresolved French fragments, excessive perceived pitch, tempo, or missing
expression import have been solved. Those retain their own investigations.

## Boundaries

Always preserve source Lyric objects including raw bytes, spelling, syllabic
metadata, lane, verse and extension evidence. Preserve musical projection,
tempo, pitch points and vibrato. Keep manual phonetic hints, force aliases,
standalone markers and internal lexical hyphens intact. Target serialization
is the appropriate boundary for a spelling-only transformation; avoid a new
projection variant unless demonstrably needed.

Ask first only if implementing requires changing source files or deleting
user edits. Existing authorization covers this code correction and its commit.

Never modify the user's original or currently open projects. Never add a guessed
phoneme, note, rest, continuation, octave shift, or dictionary entry merely to
remove a separator. Do not alter Default or SVP compatibility. Do not edit release
versions or CHANGELOG headings. All work remains on the dedicated branch.

## I/O and Edge Cases

| Input under French profile | Expected output |
| --- | --- |
| Known `chan- / ger`, `pres– / se`, `mê— / me`, `rê- / ves` | Separate normalized syllables with verified existing hints, same notes |
| Unknown `zyx- / -qwv` | `zyx / qwv`, source objects intact, unresolved diagnostic retained |
| Each supported edge dash character | Same separator cleanup, no empty lyric introduced |
| MuseScore Begin/End metadata without literal hyphens | Existing per-note pronunciation unchanged, metadata preserved |
| Leading/trailing edge separator plus surrounding whitespace/punctuation | Remove only proven separator, preserve punctuation/case/spacing when no lexical reading exists |
| `arc-en-ciel`, bare `-`, `+`, `+~`, `?alias-`, `mot[phones]` | Internal hyphens, controls and manual hints unchanged |
| Same unknown input with Default or SVP | Existing output unchanged |
| Repeated conversion and direct/bundle serialization | Deterministic identical spelling behavior |

</frozen-after-approval>

## Code Map

`hyphen_markers` already defines eleven supported typographic separator
characters and protects standalone dash-only tokens. French `pronounce` uses
normalized lookup text; unresolved `ProjectedLyric::Source` currently bypasses
that cleanup. The existing dangling-dash regression explicitly expects
`chan-`, documenting the gap rather than the user's required output.
USTX serialization receives the chosen profile and owns emitted lyric spelling.

## Tasks and Acceptance

- [x] Add narrowly scoped French-profile output cleanup independent of lookup.
- [x] Add positive and negative tests for every matrix row; update the old
  dangling-dash expectation only for the intended spelling change.
- [x] Prove original lyric objects and note/timing/pitch/vibrato invariants.
- [x] Run focused tests, format and lint; parent will run the consolidated gate
  after remaining pronunciation work to avoid concurrent Cargo builds.
- [x] Document this distinction in the French guide and spec index.

## Design Notes

If code inspection exposes a better equivalent target-boundary implementation,
use it while preserving the frozen behavior and report the choice.

Implemented in USTX serialization for unresolved `Source` text under the French
profile. The existing eleven-character separator set is shared without changing
the syllable joiner. Cleanup removes only edge separators, retaining surrounding
text; no projection variant, source mutation or dictionary entry was added.

## Spec Change Log

- 2026-09-09: Spec created before production edits from the clarified user
  requirement; implementation authorized by the current request.
- 2026-09-09: Implemented target-boundary cleanup, regression coverage and guide
  documentation. Frozen behavior is unchanged.

## Verification

Passed on 2026-09-09, with Cargo builds run sequentially:

- `cargo test --manifest-path src-tauri/Cargo.toml --locked --test french_phonetics`:
  22 tests, covering both score adapters, all eleven separators, known layouts,
  Begin/End metadata, repeats, unresolved diagnostics and direct/bundle bytes.
- `cargo test --manifest-path src-tauri/Cargo.toml --locked --lib engine::target::ustx::tests`:
  41 tests, including punctuation/spacing/control guards and full source-object,
  note, timing, pitch, vibrato and profile-isolation comparisons.
- `cargo test --manifest-path src-tauri/Cargo.toml --locked --test language_fidelity --test syllabic_words --test source_fidelity`:
  37 compatibility and fidelity tests.
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`.
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings`.
- `git diff --check`.

The parent consolidated gate passed after review patches: 459 Rust tests,
14 optional ignored; 45 frontend tests, one skipped; version check, build,
format, all-target Clippy and diff check passed. Review guards preserve numeric
minus signs and punctuation-wrapped control aliases, and recognize additional
quote punctuation. Bundle coverage exercises its real
project serialization boundary; no acoustic rendering was performed. Existing
MuseScore parsing trims outer spaces before projection; this fix preserves all
spacing present in projected text. Internal lexical hyphens remain intact even
when an existing verified reading adds hints, as for `arc-en-ciel`.

Unresolved pronunciation, perceived pitch, tempo investigation and broader
expression import remain separate. No acoustic claim is implied by separator
or allocation tests.

## Suggested Review Order

- Inspect the French-only spelling boundary and its control guards.
  [ustx.rs:412](../../src-tauri/src/engine/target/ustx.rs#L412)
- Check separator coverage and unchanged musical/source fields.
  [ustx.rs:1306](../../src-tauri/src/engine/target/ustx.rs#L1306)
