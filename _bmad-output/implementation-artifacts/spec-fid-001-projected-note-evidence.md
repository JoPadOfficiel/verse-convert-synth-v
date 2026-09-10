---
title: Make note projection evidence match editable output
type: bugfix
created: 2026-09-09
status: ready-for-dev
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

- [ ] Correct the narrow evidence boundary without changing serialized vocal notes.
- [ ] Cover mixed untexted, protected holds, multiple lyric projections and equal-note identities with positive/negative regressions.
- [ ] Assert actual bundle ledger note and note-event dispositions against final USTX/SVP notes; preserve raw/stem attribution.
- [ ] Run required local gate and compare both private masters' musical output unchanged.
- [ ] Document what projected means, including separate remaining expression mappings and source-only notation.

## Design Notes

The caller owns integration coordination. FR-004 is committed; EXP-002 currently
owns shared projection files and Cargo. This specification is ready, but no
production implementation has begun. FID-002 will reuse the retained note record
for source-owned continuity and technical routing. Do not implement that musical
change here or depend on optional MIDI performance metadata for note identity.
Only add ownership fields needed for this evidence correction; source identity
exists even when performance is None. No worker commits; parent reviews and
commits. Use at most two Cargo jobs and two test threads after the integration
slot is explicitly released.

## Verification

Pending. Source audit observed first-pass Alti ger at tick19680 absent from the
editable project yet projectedExact in historical ledger. Five other PB spans
carry no lyrics and may legitimately remain source/stem-only. No generic
restoration or complete editable fidelity is claimed.
