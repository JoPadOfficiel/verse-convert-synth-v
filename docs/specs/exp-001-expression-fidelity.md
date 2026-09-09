---
title: "EXP-001: Musical and expressive provenance"
type: investigation
created: 2026-09-09
status: audited
---

# EXP-001: Musical and expressive provenance

Status: investigated for the supplied scores; broader expressive import is a
separate implementation, not part of the pronunciation profiles.

## Verified baseline

An independent master-MSCZ comparison finds all 1550 notes of the user's raw PB
export at the exact source pitch, onset and tied duration. Its voice ranges are
59–76 (Sop/Alti) and 44–64 (Bass), with no added octave. The supplied vocal staves
contain no authored pitch-bend, vibrato or dynamic fade curves to transfer.

The audit of the last twenty commits identifies `7741de7` as replacing ±1 ms
pitch points with OpenUtau's Standard ±40 ms preset. These transitions are an
explicit renderer default, not expressive points extracted from the source.
The PB contains disabled vibrato and no active part expression curves or audio
fades. A written rest explains a checked gap before `Oui`; this does not prove
the cause of every perceived cutoff. Two extra pitch points exist only in the
separately hand-edited reference and must not be overwritten by reconversion.

## Requirements for subsequent expressive import

- Preserve authored pitch points, their time reference, onset offsets,
  interpolation, pitch-bend range and tempo-dependent placement when present.
- Preserve authored vibrato speed, depth, start, fades and phase with explicit
  unit conversion; do not invent vibrato for a score that lacks it.
- Treat volume envelopes separately from pitch. Document translation of MIDI
  controllers and MusicXML/MuseScore dynamics where an equivalent exists; do not
  claim byte preservation means editable OpenUtau expression preservation.
- Surface source expression data that cannot be represented, rather than
  silently claiming an identical expressive performance.
- Keep nominal notes source-exact. Transposition is an arrangement choice, not
  a silent fix for a bright voice timbre. DiffSinger GENC adjusts formants;
  classic GEN is a different expression and does not transpose the score.
- Verify multi-point curves, short notes, tempo changes, interpolation variants,
  neighboring notes, rests and manual USTX edits with dedicated fixtures before
  claiming broader expressive import is implemented.

No arbitrary fades, duration extensions, automatic octave shifts, or edits to
the user's existing projects are authorized by a source-fidelity requirement.
