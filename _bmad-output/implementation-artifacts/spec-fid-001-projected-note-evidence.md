---
title: Make note projection evidence match editable output
type: bugfix
created: 2026-09-09
status: done
review_loop_iteration: 1
baseline_commit: 588fe2c
context:
  - docs/contribution-guide.md
  - src-tauri/src/engine/convert.rs
  - src-tauri/src/engine/projection.rs
  - src-tauri/src/bundle.rs
  - _bmad-output/implementation-artifacts/fr-003/source-track-fidelity.md
---

<frozen-after-approval>

## Intent

The source audit found source notes absent from editable vocals but marked
projectedExact in existing bundle ledgers. A source snapshot or rendered stem
is not proof that an editable note exists. Make projection evidence and bundle
dispositions describe the actual saved vocal project, without restoring an
untexted note by inventing a lyric or changing its musical data.

## Boundaries

Keep source files, source identity, note geometry, lyric selection, vocal lane
selection, default output and preservation artifacts unchanged. This increment
corrects evidence only, not melisma ownership or the meaning of untexted notation.
Preserve all existing pronunciation and performance work. No release/version
changes, original-file writes, push or historical bundle overwrites.

## I/O and Edge Cases

| Condition | Required behavior |
| --- | --- |
| Mixed sung/untexted lane drops a genuinely untexted note | No editable-note/event disposition for the removed note; original source and applicable stem still preserve it |
| Real hold or its necessary predecessor remains | Both actual retained note identities remain projected |
| Same source note appears in several lyric projections, retained in one | Evidence is the union of actual representations, never erased by another lane's exclusion |
| Equal pitch/onset/duration notes with different source identities | Do not identify retained notes by geometry alone |
| Explicit no-lyrics override retains a whole lane under current policy | Evidence matches those actually emitted notes, without changing existing policy |
| Both USTX and SVP, direct and bundle | Same note ownership and truthful disposition at their current shared projection boundary |

</frozen-after-approval>

## Code Map

`project_track` currently inserts source note IDs and note-on/off event IDs into
global ProjectionEvidence before `drop_untexted` filters the track. Attach a
small target-neutral source-evidence record to each projected note so the record
travels atomically with the note through filtering and splitting. Collect original
note/on/off IDs and any represented lyric/event IDs before projection transforms;
union only actually retained records into global evidence afterward. Preserve
the existing retention predicate rather than introduce a parallel prune mask.
Never clear an ID already represented by another lyric lane. Avoid geometry-only
joins. Attached/standalone lyric event attribution must remain truthful too.

`build_preservation_ledger` already falls back to source/stem dispositions when
projection evidence is absent. Verify actual current ledger construction with a
compact fixture, not only the historical user's preservation.json. Existing
source files and bundles remain read-only evidence.

## Tasks and Acceptance

- [x] Correct the narrow evidence boundary without changing serialized vocal notes.
- [x] Cover mixed untexted, protected holds, multiple lyric projections and equal-note identities with positive/negative regressions.
- [x] Assert actual bundle ledger note and note-event dispositions against final USTX/SVP notes; preserve raw/stem attribution.
- [x] Run required local gate and compare both private masters' musical output unchanged.
- [x] Document what projected means, including separate remaining expression mappings and source-only notation.

## Design Notes

The caller owns integration coordination. FR-004 and EXP002 are committed.
The reviewed FID001 implementation is integrated with both ownership fields preserved. FID-002 will reuse the retained note record
for source-owned continuity and technical routing. Do not implement that musical
change here or depend on optional MIDI performance metadata for note identity.
Only add ownership fields needed for this evidence correction; source identity
exists even when performance is None. No worker commits; parent reviews and
commits. Use at most two Cargo jobs and two test threads after the integration
slot is explicitly released.

## Verification

Implemented and independently reviewed. The amended isolated engine/bundle and
French/English suite passes 401 tests; all eight private outputs remain byte-identical
to baseline588fe2c. Integrated with EXP002 at28abaea, all-targets Rust passes
514 tests with zero failures and14 expected opt-in ignores. Original source
identity is cloned for PerformanceNote and also retained in NoteEvidence; both
fields survive the integration. Source audit observed first-pass Alti ger at
tick19680 absent from the editable project yet projectedExact in historical ledger. Five other PB spans
carry no lyrics and may legitimately remain source/stem-only. No generic
restoration or complete editable fidelity is claimed.

## Isolated implementation workspace

Implement in `/private/tmp/verse-fid-001-note-evidence` on branch
`codex/fid-001-note-evidence`, based on588fe2c. Resolve production context paths
relative to this worktree. Read the private source-track report from the main
checkout at `/Users/jopad/Downloads/verse-convert-synth-v/` (it is intentionally
not committed or copied into public fixtures). Do not edit main production.
This baseline predates EXP002 production, so no PerformanceNote field exists
here; preserve the parent's later optional field at integration and do not add
competing expression ownership. FID002 will reuse this evidence record.

Do not run Cargo or commit. Parent owns shared Cargo, final integration, reviews
and commits. Standalone rustc verification of actual engine modules against
existing cached dependencies is permitted. Existing compile arguments are in
main `_bmad-output/implementation-artifacts/fr-004/rustc-command-review.json`.
Set CARGO_MANIFEST_DIR to this worktree's src-tauri and use private outputs;
`#[path=...] mod engine;` can test the engine without Tauri. Coordinate one
whole-engine rustc compilation at a time with the parent/FID003 worker; pure
small harnesses need no shared Cargo. A cached verse_lib bundle builder may be
used for read-only evidence checks by reparsing the same source in each crate
and transferring the standard source-ID sets, never treating distinct crate
IR types as interchangeable. Final integrated Cargo/bundle checks remain pending
until the parent releases the slot.

Return the changed paths, ready patch and exact evidence. Do not claim full
integrated acceptance merely from standalone tests. Keep the original note
outputs byte-identical; the only behavior change is truthful retained evidence.

## Review clarification — iteration 1

See [review triage](fid-001/review-triage.md). Preserve the implemented evidence seam and all musical output. Add actual simultaneous same-verse splitting and multi-verse saved-bundle coverage for both targets, with original note/on/off/lyric IDs and exact artifact sets on every lane. Include representative French/English lyric transformations with populated provenance, tempo/meter evidence independence and the exact existing gap diagnostic. Document source-lyric correspondence without asserting verbatim target text or acoustic fidelity. These non-frozen verification clarifications do not change retention policy.

## Suggested Review Order

- Union provenance only after filtering and lane splitting.
  [convert.rs:1829](../../src-tauri/src/engine/convert.rs#L1829)

- Move original note and lyric identities together.
  [projection.rs:148](../../src-tauri/src/engine/projection.rs#L148)

- Verify both targets retain identities through actual overlapping verse splits.
  [convert.rs:3466](../../src-tauri/src/engine/convert.rs#L3466)

- Reopen saved bundles and verify every vocal lane and artifact disposition.
  [bundle.rs:3965](../../src-tauri/src/bundle.rs#L3965)

- Check source identity survives French word allocation.
  [french_phonetics.rs:1509](../../src-tauri/tests/french_phonetics.rs#L1509)

Integrated Clippy, rustfmt and diff checks passed; see [integration evidence](fid-001/integrated-verification.md).
