---
title: Transfer explicit MIDI performance curves to OpenUtau
type: bugfix
created: 2026-09-09
status: done
baseline_commit: e04120ee94cc520aeeec72db246074204eb102f4
review_loop_iteration: 1
context:
  - docs/contribution-guide.md
  - docs/architecture.md
  - _bmad-output/implementation-artifacts/spec-exp-001-expression-fidelity.md
  - _bmad-output/implementation-artifacts/fr-003/expressive-loss-audit.md
  - src-tauri/src/engine/midi.rs
  - src-tauri/src/engine/convert.rs
  - src-tauri/src/engine/projection.rs
  - src-tauri/src/engine/target/ustx.rs
  - src-tauri/src/engine/target/svp.rs
  - src-tauri/tests/source_fidelity.rs
---

<frozen-after-approval>

## Intent

MIDI pitch bends and volume/expression controllers currently survive parsing but
vanish from editable vocal exports. The user requires source-present performance
to be transferred, rather than silently replaced with an empty automation lane.
Implement supported explicit MIDI/KAR performance as active OpenUtau curves.
The architecture must retain source ownership and distinguish mapped performance
from raw source preservation. No-expression files retain current output exactly.

## Boundaries

Always retain raw events and source IDs. Keep nominal notes, lyrics, geometry,
rests, tempo and voice ownership unchanged. Build one target-neutral performance
representation with explicit units and held interpolation; place target scaling
and sampling in the USTX adapter. Scope controller state by original port/channel,
including physical-track and polyphonic siblings. Unknown or conflicting state
must not silently pick a different track's value.

Never guess a bend sensitivity from a noncenter bend, equate CC1 with fully
specified vibrato, equate OpenUtau VEL with MIDI velocity, transpose nominal
notes, add fades absent from source, or change existing user projects. No manual
version or changelog updates. This increment implements MIDI/KAR curves only;
numeric score expression and notation playback need their own verified mappings.
SVP support is not claimed by the USTX adapter; report unsupported target transfer.

## I/O and Edge Cases

| Source evidence | Expected behavior |
| --- | --- |
| Explicit RPN 0 sensitivity 2 semitones, bends 8192/12288/0/8192 | USTX PITD 0/+100/-200/0 cents at source ticks, with held values and source attribution |
| Sensitivity changes during a held bend | Recompute cents at that event without requiring another bend |
| Unknown range, conflicting ordering, NRPN/MPE/tuning/reset ambiguity | Retain and diagnose affected unmapped performance; no guessed cents |
| Explicit CC7/CC11 | Compose normalized gains once and export active DYN automation |
| Gain zero then restored | Exact target mute followed by restored gain; do not forget restoration |
| Positive gain below DYN range or pitch beyond PITD range | Explicit representation-limit result, no silent clipping or accidental mute |
| 14-bit CC39/43 or unhandled state-altering SysEx/reset | Do not silently use a partial MSB-only interpretation |
| One channel split among vocal lanes | Same applicable source timeline reaches each affected lane; other channels/ports remain separate |
| Curves before note onset, after rests, across tempo changes | Correct initial state and exact tick positions; no long invented ramp across silence |
| Dense steps/short pulses/off-grid changes | Respect documented target sampling tolerance or report unsupported span |
| Authored pitch timeline | Flat source-note base with snap disabled where governed; PITD applied once, no additional synthetic portamento |
| No authored timeline | Existing +/-40 ms defaults, disabled vibrato and empty curves remain byte-compatible |
| Non-USTX target | Source retained and unsupported editable-expression transfer reported honestly |

</frozen-after-approval>

## Code Map

The read-only audit identifies exact loss boundaries. MIDI parser already emits
typed CC and 14-bit bend events. Decomposition places non-note events only in
lane zero, so derive channel state from original provenance rather than current
output lane alone. `SourceNote` and `ProjectedTrack` currently omit performance;
both writers emit empty curves. The existing performance-preservation test only
checks parsed events and an empty vocal output and must gain a singing fixture.

The pinned OpenUtau sources under `/tmp/verse-openutau-reverse/` establish integer
curve samples, PITD cents, DYN tenths of dB, descriptor limits and 5-tick render
sampling. Verify the consumer contract and cite the exact upstream revision in
format-specific implementation comments. Reuse exact target-grid checks.

## Mapping Contract

- RPN sensitivity uses CC101/100 and CC6/38, with null selection respected.
  `bend_cents=(raw14-8192)*(100*semitones+cents)/8192`. Keep fractional cents
  until target adaptation; center needs no sensitivity assumption.
- Supported gain translation is `(CC7/127)*(CC11/127)`, each absent contributor
  neutral until explicitly set. This is a documented conversion policy, not a
  claim that every MIDI synthesizer uses the same amplitude response.
- DYN is `round(200*log10(gain))`; zero maps to the consumer's exact-mute minimum.
  Positive gain must not round into that mute sentinel. Report mapping and value
  rounding explicitly. Avoid duplicated gain on separate phonemes.
- MIDI values are stepwise. Use target guard points and final-state coverage,
  with a documented bounded transition/sampling error, rather than interpolating
  across the entire interval between events. Validate actual `UCurve.Sample` and
  render sample origins; report pulses the target cannot preserve.

## Tasks and Acceptance

- [x] Implement source-owned performance normalization and active USTX curves.
- [x] Test the full matrix with compact synthetic MIDI files containing lyrics,
  including RPN changes, volume mute/restore, ports/channels, sibling lanes,
  ambiguous cross-track state, sampling limits and target compatibility.
- [x] Verify source event disposition and user-visible diagnostics; no source
  snapshot or stem may be mistaken for editable curve preservation.
- [x] Validate saved curves through the pinned OpenUtau consumer without an
  acoustic model where possible, including base-pitch/snap interaction.
- [x] Keep the French/English pronunciation tests and no-expression corpus stable.
- [x] Update fidelity documentation and the BMAD specification index with exact
  supported mappings and remaining notation/SVP limitations.

## Design Notes

Choose the narrowest architecture-compatible implementation. Retain unsupported
raw evidence without promising an unimplemented musical interpretation. The
completed supported path must create actual curves; warnings alone are not a fix.

Integration coordination: FR-004 review patches currently own `convert.rs`,
`french.rs`, parser provenance fields and the Cargo slot. Begin by reading the
contract and implementing the independent performance module and its contained
tests in a new `src-tauri/src/engine/performance.rs`. Report readiness to the
parent before editing shared production files or running Cargo; the parent will
release that integration slot after committing FR-004. Preserve every existing
working-tree change. Do not commit: the parent owns review, the full gate and
coherent commits. Use at most two Cargo jobs and two test threads when released.

## Spec Change Log

- 2026-09-09: Created in configured BMAD implementation_artifacts before code,
  following the user-authorized expression/history audit.

## Verification

Iteration-1 bounded review corrections are implemented without reverting the
existing work. See [iteration-1 verification](exp-002/iteration-1-verification.md)
for every triaged finding, exact commands, consumer pin and limits. Final Rust
gate: 505 passed, zero failed, 14 opt-in/helper tests ignored; Clippy, fmt,
frontend tests/build, version check and diff check passed. French/English and
no-expression automated fixtures remain stable. The completed pre-review corpus
batch is historical evidence; it does not cover these corrections. The parent
accepted the 505-test/native gate for the bounded EXP commit. The final aggregate
corpus matrix remains mandatory after FID/EXP-003 integration; no separate
EXP-only rerun or acoustic render is claimed.

Saved pitch, gain, pulse, tempo/rest and default projects passed the native
OpenUtau 0.1.569.0 / Core revision `3f213e8993ca792c3e6f8958c92ab27eae78eac5`
probe, including real `Ustx.Load` and negative empty/missing-curve fixtures.
Full renderer pitch composition remains source-inspected, not executed by this
model-free probe. Positive terminal spans under five ticks and overlapping
millisecond default-template extents are explicit representation limits. An
event exactly at the exclusive endpoint cannot invalidate the preceding curve.
Mapped evidence uses ledger schema 3 with bounded shared span references; schema
2 remains readable and unchanged for no-performance exports. The frozen linear gain law
is not a universal GM response or identical SoundFont reproduction. Nominal-note
retention evidence (FID-001), score dynamics/ramps (EXP-003) and SVP expression
mapping remain separate. No commit was made by this worker.

This is not the cause or a claimed correction of the reported high voice.

## Review clarification, iteration 1

The independent review decisions and complete KEEP contract are in
[review triage](exp-002/review-triage.md). Apply these bounded corrections without
reverting the existing implementation, as explicitly required by the user.
Performance applies to half-open sounding spans: a controller exactly at the
exclusive endpoint cannot invalidate the preceding curve. Short positive spans
still obey the consumer limit. Millisecond default pitch extents, including
short rests and tempo changes, must be considered at ownership boundaries.
Known later state may recover a specifically resolved conflict; opaque protocol
hazards remain unsupported. A parameter selection alone is not a parameter write.
Use bounded indexed traversal and bounded structured target/note/span references
for partial mapping. New mapped disposition needs an explicit ledger capability
or schema version, with old schema2 readable and unchanged no-expression output;
this is a data-format change, not an application release-version bump. Verify
serialized curves and mixed-success ledger bytes in ordinary tests, and prevent
empty native fixtures from passing. Native test claims must match the actual
consumer path exercised. FID and EXP003 ownership remain separate.

Change log2026-09-09: clarified temporal/protocol/capability boundaries after
review; avoided false mapped claims, unrelated event attribution and unbounded
report growth. Preserve all working mappings, French/English changes and source
geometry listed in the triage KEEP contract. No source projects are rewritten.

## Suggested Review Order

- Resolve original ownership before any target adaptation.
  [performance.rs:380](../../src-tauri/src/engine/performance.rs#L380)

- Carry source identity and shared state with each projected note.
  [convert.rs:968](../../src-tauri/src/engine/convert.rs#L968)

- Bound traversal and preserve half-open sounding intervals.
  [performance.rs:69](../../src-tauri/src/engine/target/performance.rs#L69)

- Map held source values into verified PITD and DYN limits.
  [performance.rs:100](../../src-tauri/src/engine/target/ustx/performance.rs#L100)

- Version structured performance evidence while reading existing ledgers.
  [bundle.rs:350](../../src-tauri/src/bundle.rs#L350)

- Exercise saved outputs, partial mapping and compatibility.
  [expression_fidelity.rs:783](../../src-tauri/tests/expression_fidelity.rs#L783)

- Validate the pinned native consumer against positive and negative fixtures.
  [probe-midi-performance-consumer.py:25](../../scripts/probe-midi-performance-consumer.py#L25)
