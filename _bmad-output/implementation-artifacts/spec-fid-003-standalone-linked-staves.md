---
title: Preserve standalone MuseScore parts with external staff links
type: bugfix
created: 2026-09-09
status: done
review_loop_iteration: 1
baseline_commit: 588fe2c726d369ef6b9345dfa33549e2a84cb839
context:
  - docs/contribution-guide.md
  - src-tauri/src/engine/musescore.rs
  - src-tauri/src/engine/midi.rs
---

## Intent

The complete private corpus exposes a genuine import refusal: the standalone
MuseScore 3.6.2 file `This Little S_Pno Melodie.mscz` contains two Parts, three
musical Staves and 68 measures on each, but conversion reports no usable staff.
Every Part/Staff declaration has a linkedTo element (1->1, 2->3, 3->4).
The parser currently deletes every linked staff unconditionally. These links
can refer to a master score that is not included in this standalone part.
Preserve its actual musical content and topology without duplicating a proven
local notation/tablature view.

## Source evidence

[MuseScore 3.6.2 Staff::write/readProperties](https://github.com/musescore/MuseScore/blob/v3.6.2/libmscore/staff.cpp)
uses masterScore staff indices for linkedTo. The reference is not proof that
the referenced music is present in this document. The failing private archive
contains one root Score and no embedded master. It has 214, 157 and 266 Chord
elements on its three respective staves. This is read-only evidence; do not
commit the copyrighted score or substitute a stripped source for the real test.

## Contract

- Keep a self-link, dangling link, external-master reference, cycle, or unresolved
  chain from suppressing actual score-body measures. Numeric equality of staff
  IDs alone is not evidence of a local duplicate.
- A linked view may be excluded only when its retained canonical target is
  present in this same selected Score, belongs to the same source Part, and
  the musical content is proven equivalent. Compare normalized musical content
  including rhythms, pitches, lyrics, voice ownership and relevant playback
  controls; do not require equal source IDs or visual layout. If an exact safe
  equivalence cannot be established, retain the distinct source staff and
  diagnose unresolved linkage rather than delete music.
- Preserve the existing positive local notation+tablature fixture: one played
  note, one canonical staff, with raw source retained. Do not apply a blanket
  “keep all linked staves” fix.
- Preserve all actual Part/staff/voice identities, instruments, lyrics, tempo,
  repeats and note geometry for retained staves. Standalone source staff2 and3
  must remain the two Piano staves, not be attributed to Soprano or an external
  index. No pitch transposition, inferred text, or audio changes.
- Raw linkage metadata survives source retention. Record any new interpretation
  or unresolved-link diagnostic truthfully; a retained unresolved staff is not
  an asserted canonical local duplicate.
- Apply the same parsing policy to MSCX and its MSCZ container. Default/FR/EN
  profiles and both targets share the corrected source topology.

## Implementation map

Inspect linked_staff_ids construction and early continue in the Part/Staff
loop, then the score_staves filter. Determine retained canonical relationships
before either topology construction or note parsing excludes a staff. Reuse
normal parsing for semantic equivalence if practical; do not create a second
independent rhythm parser. Keep the selected root Score boundary; do not flatten
embedded excerpts into another performance.

The parent coordinates shared parser/Cargo ownership with EXP-002/003 and
FID-002. The reviewed isolated implementation is now integrated, including the
parent correction for anonymous linked declarations. Original music files remain
unchanged; the parent owns commits and final aggregate verification.

## Tasks and acceptance

- [x] Reproduce the private failure without modifying its bytes; capture hash.
- [x] Correct linked-view qualification before topology/music suppression.
- [x] Test self, missing target, externally colliding IDs, cycles, mixed valid
  and external links, and valid equivalent local notation/tablature views.
- [x] Test divergent local musical content remains preserved/diagnosed, including
  lyric or rhythm differences despite equal pitch sets.
- [x] Convert the actual standalone MSCZ and extracted identical MSCX successfully
  to USTX/SVP; verify two Parts and three source staves with exact musical data.
- [x] Run focused regressions, full private corpus and existing OpenScore parser
  corpus when Cargo ownership permits. Preserve existing master's outputs.

## Verification

Completed independent review and bounded review amendments. The frozen worker
patch `a6fa6070c8b0de28885182f6cf3f2fb2188a3d75a0c8379d3133b93b6cf9bb0c`
passed 406 actual engine/bundle/renderer/stems and French/English tests. Its
264-case private matrix produced 252 independently checked serialized exports,
12 expected exact-grid refusals and zero parse errors. All 73 inventoried
source/reference hashes remain unchanged; 244 paired FR004 exports are
byte-identical. The standalone MSCZ and identical extracted MSCX preserve all
924 source notes, 133 tied continuations, two Parts and three staves in 12
export requests. Exact frozen-binary replay reproduced every report/export.

Main integration also fixes the anonymous-declaration body exclusion using the
resolved staff ID. A positive MSCX/MSCZ fixture verifies one canonical note/staff
and truthful link metadata. EXP-002's test constructor receives the new empty
staff-links field; both EXP performance and FID-001 retained-note evidence survive.
Integrated gates: **529 Rust tests passed, zero failed, 14 ignored**; all-targets
Clippy with warnings denied, formatting and whitespace checks pass. The pinned
OpenScore full-parse gate examines **1,352 files**, reports **1,343 parsed,
1,277 projected, 75 evidence-ineligible and zero unexpected errors**. Its source
pin is `6b2dc542ce2e8aa4b78c8ee62103b210efc07015`.

See [worker evidence](fid-003/review-1/verification.md),
[accepted findings](fid-003/review-1/accepted-review-items.md) and
[main integration evidence](fid-003/review-1/integrated-verification.md).
The worker evidence deliberately excludes the later parent anonymous-ID fix;
the integrated gate covers it. Final combined private/public corpus and real
render sampling after EXP-003/FID-002 remain parent release gates, not claims of
this increment. No acoustic or live UI fidelity is asserted. The 12 deliberate
nonrepresentable grid cases remain refusals; no rhythm rounding is authorized.

## Isolated implementation workspace

Implement in `/private/tmp/verse-fid-003-linked-staves` on
`codex/fid-003-linked-staves`, based on588fe2c. All context paths above are relative
to that worktree. Do not edit production files in the main checkout. The parent
owns final integration and commits; do not commit in either checkout. Shared
Cargo remains reserved to EXP-002: do not run Cargo. Standalone rustc tests using
existing cached dependencies are permitted, with at most two test threads.
The entire engine has no crate dependencies outside `crate::engine`, so a private
`#[path=...] mod engine;` test harness may compile actual worktree modules.
Existing dependency arguments are in the main checkout's
`_bmad-output/implementation-artifacts/fr-004/rustc-command-review.json`; use
only existing local dependencies, set CARGO_MANIFEST_DIR to the worktree's
src-tauri directory, and keep outputs private. This avoids rebuilding Tauri or
locking the shared target directory. No network dependency installation needed.

Report a ready patch and exact standalone/private-source checks. Full Cargo and
public/private corpus acceptance remains mandatory after parent integration;
do not label it passed before execution. Keep implementation scope this linked
staff defect; FID001/FID002/EXP changes belong to other work.

## Review clarification — iteration 1

Apply [review triage](fid-003/review-triage.md): conservative equivalence must include element namespaces and unknown markup, ignore only completely recognized harmless visual structure, and preserve referenced identity meaning. Verify pinned linkage indexing rules. Resolve missing declaration identities consistently and never attach unowned metadata to an unrelated track. Diagnostics must distinguish retained music from missing bodies. Add actual conversion/ledger assertions and ambiguity/music-preservation regressions, preserving existing note IDs and the canonical normalization cache. These non-frozen implementation/verification clarifications retain the approved source-preservation policy; keep implemented corrections rather than reverting.

## Suggested Review Order

- Qualify links conservatively before either topology or music can be excluded.
  [musescore.rs:1210](../../src-tauri/src/engine/musescore.rs#L1210)

- Use resolved anonymous identities for declaration and body exclusion.
  [musescore.rs:1554](../../src-tauri/src/engine/musescore.rs#L1554)

- Keep link interpretations outside timed musical events and unrelated tracks.
  [midi.rs:43](../../src-tauri/src/engine/midi.rs#L43)

- Expose source-owned warnings through existing application responses.
  [convert.rs:2566](../../src-tauri/src/engine/convert.rs#L2566)

- Grant link metadata only source retention, without editable project or stem credit.
  [bundle.rs:686](../../src-tauri/src/bundle.rs#L686)

- Verify anonymous linked bodies are excluded once in MSCX and MSCZ.
  [musescore.rs:2790](../../src-tauri/src/engine/musescore.rs#L2790)

- Exercise actual converter and ledger ownership for preserved and collapsed links.
  [bundle.rs:4596](../../src-tauri/src/bundle.rs#L4596)
