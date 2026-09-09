---
title: Preserve score dynamics and authored intensity transitions in active USTX automation
type: feature
created: 2026-09-09
updated: 2026-09-09
status: in-progress
revision: 2
baseline_commit: 05d4b35
implementation_status: independent-resolver-first
depends_on:
  - spec-exp-002-midi-performance-curves.md
context:
  - _bmad-output/implementation-artifacts/spec-exp-001-expression-fidelity.md
  - _bmad-output/implementation-artifacts/spec-exp-002-midi-performance-curves.md
  - _bmad-output/implementation-artifacts/fr-003/exp-003-score-expression-proposal.md
  - _bmad-output/implementation-artifacts/fr-003/exp-003-source-semantics-evidence.json
  - src-tauri/src/engine/performance.rs
  - src-tauri/src/engine/projection.rs
  - src-tauri/src/engine/musicxml.rs
  - src-tauri/src/engine/musescore.rs
  - src-tauri/src/engine/target/ustx.rs
---

## Authorization and intent

The user explicitly authorizes reasonable semantic interpretation of written
soft/loud dynamics, crescendo/diminuendo and annotated fades. A written hairpin
is sufficient evidence for a meaningful intensity transition even without an
explicit sampled waveform. No absolute acoustic loudness equivalence is promised;
the user can adjust global volume manually. Diagnostics alone do not satisfy
ordinary source dynamics. This revision supersedes EXP-003's earlier numeric-only
proposal and its blanket deferral of symbols/hairpins. The existing canonical
filename is retained so links remain valid.

Implement active score intensity for MSCX/MSCZ and MusicXML/XML/MXL through
EXP-002's shared performance/USTX adapter. Preserve explicit note velocity,
numeric dynamic settings and applicable CC gain without counting the same level
twice. MIDI/MID/KAR retain EXP-002 bend/controller support; this increment also
provides the common explicit attack-velocity contributor. Numeric tuning,
fractional pitch, pitch curves and vibrato are a separate lower-priority spec;
they must not delay score intensity. Production remains untouched by this spec
revision. Ready-for-dev means the contract below is concrete; it does not mean
EXP-002 integration or implementation tests have passed.

## Frozen bounded interpretation: `verse-score-intensity-v1`

This is a portable musical interpretation, not MuseScore-engine emulation. Store
source format/saving version, raw fields, source IDs, scope, occurrence and the
policy revision on every derived span. Mark standard-table and inferred-endpoint
values as policy-derived, separately from explicitly written numeric values.
Source-owned does not mean every target sample must have been written literally.

### Levels and numeric precedence

Use this velocity-equivalent table, taken from MuseScore3.6.2 and retained in the
4.7.4 legacy dynamic table. Applying it to MusicXML and modern MuseScore is an
explicit portable policy; normal MuseScore4 playback uses a different MPE scale.

| Mark | pppppp | ppppp | pppp | ppp | pp | p | mp | mf | f | ff | fff | ffff/fffff/ffffff |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Level L | 1 | 5 | 10 | 16 | 33 | 49 | 64 | 80 | 96 | 112 | 126 | 127 |

For positive L in 1..127, map to relative dB by **D(L)=(L-80)/4**. Thus mf is
0dB, p -7.75dB, f +4dB, and all positive standard-table values fit USTX's range
(-19.75..+11.75dB) before independent gain composition. This monotonic scaling is
an authorized interpretation of relative intensity, not a velocity-to-amplitude
law. It replaces revision1's linear-amplitude attack policy. No automatic
per-note normalization, clipping, compressor, or mastering effect is introduced.

- MusicXML numeric `sound@dynamics` supplies L=0.9*value. It replaces the symbol
  level in the same direction/scope; never multiply numeric and symbolic
  representations of the same dynamic. `note@dynamics` is a note-owned absolute
  override. Retain exact decimals; positive fractions below1 are not rounded to
  zero and may use the affine formula if within target range. Values above127
  are explicitly out of this bounded source-level domain.
- MuseScore `Dynamic/velocity` >0 replaces the table value. A nonpositive stored
  Dynamic velocity is a source sentinel for the table, not silence. Custom
  `other-dynamics` with a valid positive velocity is ACTIVE even if its text is
  unknown. Honor explicit instance `veloChange`, transition speed and scope.
- MuseScore `Note/velocity` with `veloType=user`, and MIDI nonzero NoteOn
  velocity, supply an absolute note-owned level. Offset mode applies its
  percentage to the resolved source score level; preserve source-version
  integer arithmetic where required. Without an authored score context, a
  documented mf/L80 reference is permitted only to interpret that explicit
  offset, attributed as a policy default. No event means no new automation.
- MusicXML explicit numeric0, `n`, a recognized niente indication, or an
  explicit annotated fade-to-nothing means silence. Legacy MuseScore user
  velocity0 follows its source clamp to1, not silence. Modern zero means no
  velocity override in the traced playback context. Keep these cases distinct.
- Held dynamics remain effective until replaced. Rests, phrase boundaries and
  tied continuation notes do not reset them to mf or to neutral gain. Before
  the first intensity instruction, retain existing output; a missing start
  level needed for an authored transition uses L80 with default provenance.

### Active transitions and fades

Parse MusicXML numbered wedge start/continue/stop; parse MuseScore HairPin
spanners, including crescendo/decrescendo text-line subtypes. A matched written
span defines transition timing. `spread`, page coordinates and drawn height are
not loudness. Disabled playback instances remain retained/inactive.

Resolve start from an explicit start dynamic/numeric value, otherwise the held
level at the start, otherwise declared L80. Resolve the end in this order:

1. Explicit niente/fade-to-nothing at the taper gives silence; crescendo from
   niente gives a silent start. A conflicting explicit nonzero endpoint is a
   scoped conflict, not silently discarded.
2. A valid nonzero instance HairPin `veloChange` gives start L plus the signed
   magnitude (crescendo positive, diminuendo negative). This deliberately honors
   authored custom intent even where modern normal playback ignores that legacy
   field. If a different explicit dynamic exists at the end, ramp to the numeric
   endpoint, then apply the written end dynamic at the boundary and report the
   conflict/step. Do not suppress either instruction.
3. An explicit dynamic at the matched end, or MuseScore recognized end text,
   supplies the endpoint. With no exact end mark, use the first following
   same-scope ordinary/numeric dynamic before any other state-level intensity instruction
   or repeat jump (note-owned velocity anchors do not terminate this look-ahead); the ramp still ends at its written end and holds until that
   mark. This bounded look-ahead permits common hairpin-then-f writing.
4. Otherwise move one adjacent rung in the ordered standard table. For a
   numeric intermediate, choose the first strictly higher/lower table level;
   if already at an extreme, report the exhausted range rather than inventing
   a stronger value. A zero/absent legacy veloChange means automatic endpoint
   selection, not “no musical expression.”

A wrong-direction endpoint does not silently reverse a crescendo. Use the
one-rung fallback during the span, apply the explicit conflicting end mark at
its boundary, and report the mismatch. Overlapping incompatible ramps in the
same effective scope are local conflicts; unaffected spans remain active.
An explicit dynamic strictly inside a ramp interrupts it at that tick: preserve
its authored level and split there; stop the earlier transition unless another
source transition continues it. Do not average contradictory instructions.

Interpolate **relative dB linearly in musical time** by default. For an explicit
known MuseScore `veloChangeMethod`, use normalized easing x in [0,1]: normal=x,
ease-in=1-cos(pi*x/2), ease-out=sin(pi*x/2), ease-in-out=(1-cos(pi*x))/2.
Exponential uses ((abs(deltaL)+1)^x-1)/abs(deltaL), with x for deltaL=0.
These are the named portable easing semantics; no claim of exact legacy integer
sampling or its decreasing-exponential arithmetic. Unknown method: apply the
active normal ramp with an explicit fallback disposition and retain the field.
Explicit `singleNoteDynamics=false` samples the score transition at actual note
attacks and holds within each tie chain; otherwise crescendos affect sustained
and tied notes continuously. Modern playback may differ; this honors the stored
instance instruction under the portable policy.

Niente transitions interpolate **linear gain** from/to zero using the same
normalized easing, not log(0). Reuse the adapter's limits: below the smallest
positive representable DYN level, keep a positive floor until the exact authored
zero endpoint and record the limited fade tail. Never turn a positive sample into
mute. This bounded tail approximation is required active fade behavior, not a
reason to discard the entire fade. An ordinary diminuendo without niente is not
an instruction to become silent.

Recognize exact normalized text `cresc.`, `cresc`, `crescendo`, `dim.`, `dim`,
`diminuendo`, `decresc.`, `decrescendo` as transitions; `morendo`, `smorzando`,
`fade out`, `fade-out`, `fondu`, `fondu au silence` as fade-to-nothing, and
`fade in`, `fade-in` as fade-from-nothing. A text-line span takes precedence.
For standalone text, end at the next same-scope dynamic, otherwise final sounding
note end before a repeat jump. Record the inferred span. No positive span means
an explicit unresolved-span result. Unknown free text remains retained, not
parsed by speculative NLP. Do not duplicate a text label and its owning hairpin.

Compound `fp`/`pf` and valid MuseScore Dynamic `veloChange` are ACTIVE: resolve
start/end numeric values or standard marks and use the source transition speed.
The pinned source speed contract gives duration in quarters
(tempo_at_start/120BPM)*{slow1.3, normal0.8, fast0.5}; retain exact policy time
until target conversion. MusicXML fp/pf without timing uses one quarter, marked
as default. Single-note sf/sfz/rf/rfz/fz and sff/sffz use levels112 and126
respectively for the next source attack/tie chain, then restore held context;
unknown compounds remain specifically unsupported. They must not become
permanent f unless that persistence is written.

### Scope, timing and repeats

MusicXML: a direction belongs to its part, explicit staff narrows it, explicit
voice narrows it further. Without staff/voice, apply part-wide as a declared
portable convention; never cross parts merely because notes share MIDI channel
numbers. Note notations remain note-owned. Match wedges by part, effective scope
and number (default1). Preserve source order at equal times, coalescing a symbol
and its numeric sound override. Conflicting unrelated same-priority declarations
are diagnosed instead of arbitrarily ordered.

Sound's own offset sets playback position. Otherwise direction offset affects
playback only when `sound="yes"`; omitted/no is visual-only. This corrects the
revision1 omission of the W3C offset sound flag. For notation-only directions,
use that same explicit playback-position policy. Never use default-x as ticks.
Resolve divisions, backups/forwards and repeat occurrences before target-grid
adaptation. MusicXML sound `time-only` applies on the specified repeat passes.

MuseScore3: honor `dynType` staff/part/system; missing defaults to part in the
pinned Dynamic/HairPin constructors. MuseScore4: honor voiceAssignment current
voice/all staff voices/all instrument voices; narrower applicable assignment
wins at the same instant. Map old dynType through an explicit compatibility rule
when reading older layouts; do not expand an instrument scope to the whole score.
For a modern instance with no assignment, use instrument-wide scope as a declared
portable default. Version/layout and chosen default must be recorded. Unknown enum uses a localized
unsupported scope, not global broadcast. Playback-disabled marks do not apply.

Build dynamics in source score coordinates with occurrence provenance. At a
repeat/jump, evaluate the state at the destination for the applicable pass;
do not carry the final bar's level backward into the opening repeat. Split
spans at jumps, reevaluate destination ramps, and carry state through ordinary
forward rests. Tied notes share one continuous intensity/override context.

### Numeric overrides, controllers and master

An explicit note velocity overrides the score level at that note attack; it is
not multiplied by another full copy of the same score level. Preserve ongoing
relative hairpin motion using
D_note(t)=D_score(t)+[D(note_velocity)-D_score(note_attack)] over that attack/tie
chain. Without note override, use D_score(t). A score mute overrides the positive
anchor adjustment. This is the portable rule for preserving an authored
crescendo within a note with an absolute attack override; it is not a promise of
legacy synthesizer behavior. Tied continuations do not re-anchor or multiply;
conflicting continuation velocities are retained/reported. When there is no
score intensity, explicit velocity alone supplies its mapped level.

Then G_total=10^(D_note/20)*G_CC7_CC11, with EXP-002's controller policy unchanged.
An independently supported source master gain is composed once; a manual target
master stays a separate control and must not also be baked into DYN. Shared source
IDs prevent duplication across phonemes, chord members and split lanes. Check
combined limits after composition. Do not flatten every note to a clip boundary
or silently auto-compress. Apart from the documented niente tail, range failures
retain the exact intended curve and report the affected span.

## Implementation seam and targets

The parent is now integrating `engine/performance.rs`, whose current held
ChannelPerformance and PerformanceNote key are MIDI-port/channel specific.
Do not encode score staff IDs as invented MIDI channels. Extend ownership to a
source-scope variant while preserving existing MIDI keys; add a small neutral
segment representation for held/continuous gain with interpolation/time domain
and provenance. `None` remains unknown, never neutral or silence. Reuse shared
transfer status, existing projection attachment, CC resolver and USTX adaptation.
Resolve score semantic levels first, then derive gain. Keep DYN scaling and
sampling in the target adapter. Never shoehorn a ramp into two held points.

USTX must receive active DYN, sampled from the neutral evaluator with exact
endpoints, step guards and consumer-validated five-tick phase behavior. Target
rounding must be deterministic (nearest, ties away from zero). Ordinary ramps
must have <=0.1dB sampled-value error where representable; boundaries follow
EXP-002's timing tolerance. Range/short-span exceptions are explicit. No changes
to pitch templates, vibrato, nominal notes, rests, lyrics, phonemes or French work.
SVP keeps its nominal export and reports these expression mappings unsupported
until its consumer contract is implemented; source data and intended neutral
curves remain available. Do not write DYN values into SVP loudness units.

## Compact acceptance matrix (design, not executed)

| Test | Required behavior |
| --- | --- |
| XML/MXL p, crescendo, f at t0/t480/t960, then rest | L49 to96; DYN start-78, midpoint-19, end40 after rounding. Resume after rest at40, not0. No printed sound attribute required. |
| MSCX/MSCZ same example in 3.6.2 and modern layout | Same declared portable contour; record different source contracts. Numeric Dynamic velocity wins over subtype. |
| Numeric overrides | sound dynamics100 plus printed p resolves L90/DYN25 once; note overrideL100 anchors DYN50. Subsequent score ramp delta remains; no second full score multiplier. |
| Automatic endpoints | p + endpoint-free cresc reaches mp64/DYN-40; p + dim reaches pp33/DYN-118. Wrong-direction explicit endpoint produces declared ramp then marked boundary conflict. |
| Authored fade/niente | Active gain ramp to/from zero, exact mute endpoint, positive floor only in limited tail with report; ordinary dim never automatically mutes. Bare annotated fade gets documented inferred span. |
| Source transition fields | veloChange20 from49 reaches69; normal/ease curves distinguish midpoints; normal speed at120BPM lasts0.8quarter. Explicit no single-note dynamics holds within tie chains. |
| Long note / tie / rest | p→cresc→f changes continuously through tied sustain; no re-anchor, no note-off gate, no reset after rest; an interior explicit dynamic interrupts correctly. |
| Staff/voice/part/system | Source scope and narrower precedence hold, no unrelated part leakage, no invented channels, sibling output lanes inherit the same owned state. |
| Repeat/time-only/offset | Repeated passage starts with destination state; pass-specific sound preserved. offset sound=yes applies, omitted/no does not; own sound offset wins; page x never becomes time. |
| Combined CC/master, MIDI/MID/KAR | Existing EXP-002 bend/CC tests remain unchanged. L80 with CC7=64/CC11=127 gives DYN-60. Independent manual master changes output once, without duplicate DYN scaling. Explicit MIDI velocity feeds the one common contributor. |
| Range | All positive standard levels1..127 produce distinct supported numeric-level mappings without clipping (source table's127 aliases remain aliases). Combined over/under-range reports span; positive never becomes mute. |
| Unknown/disabled and targets | Unknown marks retain raw evidence and limit only their spans; disabled playback generates no intensity. USTX active controls verified through pinned consumer, SVP honest limitation. |
| No-expression regression | No newly interpreted intensity instruction means byte-compatible output. Source bytes, nominal notes and French/English behavior unchanged. |

## Tasks and verification

- [ ] Extend the completed EXP-002 ownership/segment seam without changing its
  MIDI controller interpretation or blocking the current worker's integration.
- [ ] Implement active standard/numeric levels, wedges/HairPins, authored fades,
  note overrides, compound transitions and exact recognized text above.
- [ ] Implement persistent scoped state, occurrence/repeat evaluation, precedence
  and per-field interpretation provenance.
- [ ] Adapt composed gain through USTX; validate real consumer sampling without
  a model or acoustic render, plus the compact matrix across real loaders.
- [ ] Document source-version differences, portable policies, target limits and
  the lower-priority pitch/vibrato follow-up. Update the BMAD index at adoption.

No Cargo, production edit, render or Git write was performed for this revision.
Primary evidence and source snapshots are linked in the companion report.

## Integration coordination

EXP-002 currently owns shared production files and Cargo in this checkout. Begin
with an independent `src-tauri/src/engine/score_intensity.rs` neutral resolver,
its contained tests and private read-only fixtures/harnesses. Do not register the
module, change parser/projection/target files, or run Cargo until the parent
explicitly releases the integration slot. A standalone rustc test harness is
permitted. Report the ready resolver and continue independent test preparation
while waiting. Once released, integrate the complete contract; do not stop at
the resolver or weaken the active-dynamics acceptance matrix.

No commits from the implementation worker. Parent performs independent review
and commits after verification. Existing French/English work and concurrent
FID note-evidence/continuity work must survive. Use at most two Cargo build jobs
and two test threads when the slot is released.
