---
title: Preserve standalone MuseScore parts with external staff links
type: bugfix
created: 2026-09-09
status: ready-for-dev
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
FID-002. No implementation has started; no original-file edits or worker
commits. Parent performs independent review and commits after verification.

## Tasks and acceptance

- [ ] Reproduce the private failure without modifying its bytes; capture hash.
- [ ] Correct linked-view qualification before topology/music suppression.
- [ ] Test self, missing target, externally colliding IDs, cycles, mixed valid
  and external links, and valid equivalent local notation/tablature views.
- [ ] Test divergent local musical content remains preserved/diagnosed, including
  lyric or rhythm differences despite equal pitch sets.
- [ ] Convert the actual standalone MSCZ and extracted identical MSCX successfully
  to USTX/SVP; verify two Parts and three source staves with exact musical data.
- [ ] Run focused regressions, full private corpus and existing OpenScore parser
  corpus when Cargo ownership permits. Preserve existing master's outputs.

## Verification

Pending. Initial private matrix shows eight parse errors for this one source
across its target/profile combinations. This spec does not classify the 12
separate target-grid refusals for deliberate nonrepresentable rhythm fixtures
as bugs, nor authorize rounding those source rhythms silently.
