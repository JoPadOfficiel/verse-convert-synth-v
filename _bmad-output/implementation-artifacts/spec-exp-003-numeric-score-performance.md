---
title: Preserve score dynamics and authored intensity transitions in active USTX automation
type: feature
created: 2026-09-09
updated: 2026-09-10
status: in-review
review_loop_iteration: 6
revision: 2
baseline_commit: 1fa489fab16bf7b7f2fde2e63d663d41546202bf
initial_review_baseline_commit: 35b3de22f4f878a3ecff3d98022042979ae52666
design_baseline_commit: 05d4b35
implementation_status: paused-review-limit
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

- [x] Extend the completed EXP-002 ownership/segment seam without changing its
  MIDI controller interpretation or blocking the current worker's integration.
- [x] Implement active standard/numeric levels, wedges/HairPins, authored fades,
  note overrides, compound transitions and exact recognized text above.
- [x] Implement persistent scoped state, occurrence/repeat evaluation, precedence
  and per-field interpretation provenance.
- [x] Adapt composed gain through USTX; validate real consumer sampling without
  a model or acoustic render, plus the compact matrix across real loaders.
- [x] Document source-version differences, portable policies, target limits and
  the lower-priority pitch/vibrato follow-up. Update the BMAD index at adoption.

Implementation and local acceptance are complete for combined review iteration2. See the verification record below; final aggregate/publication and the independent review remain separate completion gates.

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

## Review clarification — iteration 1

The frozen review snapshot and all three independent findings are under
`exp-003/review-0/`. Apply the bounded KEEP instructions in
[`exp-003/review-triage.md`](exp-003/review-triage.md); they clarify the existing
implementation and acceptance contract, without replacing its musical policy.

Source text element type must preserve staff/system scope; actual MuseScore
assignment decoding needs loader-level tests. An unmatched legacy ID does not
erase a valid explicit duration. All new spanner arithmetic is checked, and
optional expression parsing must preserve nominal tempo-format acceptance.
Unsupported expression precision remains explicit and localized.

Attack-only evaluation retains a single source attack reference across segment
boundaries. A held gain does not become a short pulse merely because projection
subdivides it into short notes. Use an already declared L80 transition-start
reference where that permitted reference resolves a velocity-anchored note's
otherwise absent score attack; do not invent a finite reference through an
unknown or explicitly muted attack. Preserve the existing anchor formula and
localize any remaining limitation.

Expand diagnostic coordinates and provenance through playback occurrences.
Diagnostic-only bindings must retain specific reasons without producing active
automation. Ordinary transfer ownership is half-open; terminal endpoints and
point issues have explicit ownership. SVP retains issue reasons as unsupported
intent, and partially mapped source entries retain their limitation summaries.
An inactive velocity field must not replace active accent provenance.

Bound actual sample/segment and note/accent lookup work, not only event counts.
Use indexed/shared traversal and preserve the existing limits. Normal tests must
assert the independent fade-interior oracle as well as the separately executed
pinned native consumer. The within-model intensity-removal test is not a
pre-change parser comparison: parent completion requires the prepared historical
aggregate gate against the final integrated library.

FID002 remains responsible for proving and adopting retained continuations.
The combined real-loader test must preserve tail identity, original owner and
head intensity context while leaving an eligible new source syllable as its
own attack. The manual helper test alone does not satisfy this integration gate.

## Spec Change Log

- Review iteration 1: parent triaged the complete blind, edge and verification
  results, accepting the bounded corrections above and recording integration
  gates separately. Avoided states include nominal parse regression, held-gain
  drops on short notes, attack-only mid-note jumps, missing/misattributed issue
  evidence and unbounded nested lookup work. KEEP all implemented source-owned
  curves, explicit policy, native sampling, nominal fidelity, previous fixes and
  immutable history. The user explicitly requires preserving implemented work;
  apply amendments without destructive re-derivation. Numeric precedence changes
  a compound's level, not its otherwise authored transition/accent behavior.

## Combined integration review

The current review covers the integrated EXP003 and FID002 source changes
against commit1fa489fab16bf7b7f2fde2e63d663d41546202bf. Historical review artifacts
and independent baseline goldens remain unchanged. See
`combined-exp003-fid002/parent-integration.md` for source attribution, typed tie
intensity adoption, bounded copying and the written-expression prepass reset.
Current integrated verification is tracked there; status is not done until
review findings and final available-file gates have been resolved.

## Spec Change Log — combined review iteration 2

The combined review and public corpus exposed interactions detailed in
`combined-exp003-fid002/review-1/triage.md` (C01–C12). Amend the implementation
requirements to enforce selected-attack accent ownership, complete proven tie
reference ranges, precise source-only/terminal diagnostic ownership, active
recognized HairPin fades, conservative duplicate wedge handling, localized
invalid-state recovery and pre-allocation resource bounds. Final linguistic
rewriting must preserve the meaning of proven continuity, not freeze obsolete
pre-transform syllable ownership. Keep strict invalid-chain rejection.

Reproduce each accepted finding through actual conversion or a focused resource
fixture, rerun the public/private aggregate and pinned native gates, and record
remaining differences explicitly. KEEP all working source geometry/identity,
FR/EN pronunciation, original ownership, two PB/one chant restorations, five
exclusions, intensity policy and native sampling guarantees. Preserve prior
code and evidence per explicit user instruction; no destructive reset.

### Iteration 2 resource acceptance clarification

C09 resource protection must count actual pre-copy provenance work across source owners and occurrence runs. It must not charge ordinary replacing dynamics as hypothetical cumulative inherited ramp histories: the first dry estimate refused605 valid OpenScore files. Keep the50M operation and128MiB cumulative copy limits, with event traversal preflight and real copy/merge charges before allocation. Normal multi-part dynamic replacements must pass, while a64-run1MiB repeated evidence case and the adversarial long inherited-ramp case must refuse within those unchanged bounds. Every original source remains immutable; baseline refusal totals may not be loosened to accommodate this implementation defect.

Note-level resource accounting follows the same rule: use the shared real pre-copy budget for NoteIntensity construction and issue/velocity provenance. Do not multiply every note’s inherited context by hypothetical future segment copies. The normal multi-part fixture covers both ordinary dynamics and explicit numeric note overrides; the same128MiB/50M limits continue across owner timelines and note bindings.

## Verification — combined iteration2 review candidate

Implementation gate:732 Rust tests passed,0 failed,18 private/process helpers ignored by default; strict Clippy, formatting and diff checks passed. The affected bounded-negative assertion was rerun after a mechanical lint change. Frontend45 passed/1 optional skip, build and version0.6.3 passed on unchanged frontend sources. Public OpenScore pin6b2dc542ce2e8aa4b78c8ee62103b210efc07015:1352 discovered,1343 parsed,1277 projected,75 expected refusals,0 unexpected,0 evidence invariant failures.

Configured acceptance passed: original PB/chant against independent nominal goldens and exact+2/+1 continuity identities/exclusions;22-case score intensity inventory; actual This Little924-note and Help percussion source-fidelity; complete fake-render bundles. Fresh fixtures passed the installed exact OpenUtau0.1.569.0 consumer:9 expression fixtures through real Ustx.Load/Validate and3 negative inputs;6 continuity fixtures through native validation/grouping plus6 mutated negatives. No acoustic/model fidelity claim.

Evidence directory: [combined iteration2](combined-exp003-fid002/review-2/amendment-evidence.md). Root library SHA256 a0640002131209934c6858581d930af671d2830240a21a3d676de159efb3e2cf. Supplemental264-case final private aggregate and deterministic real MuseScore render are running against frozen production; neither is claimed passed here. The known corrupt Iko external fixture remains unavailable, with its pinned failure receipt. Independent review, final aggregate, new sibling preview publication and commits remain to complete the parent task.

## Spec Change Log — combined review iteration 3

All three iteration2 reviews were collected before triage. The non-frozen implementation/verification requirements are amended by [iteration2 triage](combined-exp003-fid002/review-2/triage.md), R2-01 through R2-15. Apply only the relevant ownership slice specified by the amendment handoff. Preserve every frozen interpretation and original baseline. KEEP all working geometry, original IDs, source bytes, pronunciation, MIDI behavior, PB+2/chant+1, a01+7 and the five PB exclusions. The user explicitly requires preserving prior corrections; amend in place without reverting them. Avoid skipped-ending dynamics, lost repeated diagnostics, unbounded evidence copies and false-positive validation. Previous gate receipts describe the frozen iteration2 candidate; rerun affected checks and final aggregate after these amendments. No new baseline exceptions.

## Verification — combined iteration3 frozen review candidate

All-targets Rust763passed0failed18ignored. Strict Clippy and final formatting/diff checksPASS after equivalent mechanical lint cleanup (no behavior/expectation changes). Frontend45passed1optional skip/build/version0.6.3 passed earlier on unchanged frontend. Actual source/private gatesPASS: PB/chant independently pinned historical golden manifest and generator; source-proven+2PB/+1chant holds and five exclusions;22-score intensity inventory; ThisLittle924-note/Help source fidelity; actual fake-render bundles.

Fresh exact OpenUtau0.1.569.0 native gatePASS:9 expression realLoad positives,3 standard negatives and53 score-oracle negatives;6 continuity native fixtures and6 mutated negatives. Full-domain gain oracles reject truncation and unauthorized floors. This is native schema/validation/sampling evidence, not DiffSinger acoustic equivalence.

Public OpenScore pin6b2dc542ce2e8aa4b78c8ee62103b210efc07015:1352discovered1343parsed1277projected75expectedineligible0unexpected0evidence-invariant failures. Actual MuseScore4.7.4 render of deterministic Holmès lc5661740 sample succeeded: full score64059702bytes and2/2Partstems,0render errors.

Final264-case private aggregate completed:252serialized12expectedexact-grid refusals0discrepancies; all73source/reference hashes unchanged and66SVPprofile comparisons identical. No baseline exceptions added. RootrlibSHA25605145dd14d1947eb83ac1da2e0ac91411385da2c3590ead36139d4ef5827bdd9; exact helper/acceptance pins unchanged. Receipt: corpus-2026-09-09/final-aggregate/run-final-1788992628123088000/summary.json. Full evidence: combined-exp003-fid002/review-3/.

Only independent review, new sibling preview publication and commits remain. Known corrupt external Iko fixture remains unavailable; no acoustic/model equivalence or unsupported numeric score pitch/vibrato transfer claimed.


## Spec Change Log — combined review iteration 4

All three iteration3 reviews were collected and independently triaged before editing. [Iteration3 triage](combined-exp003-fid002/review-3/triage.md), R3-01–R3-16, now adds the missing non-frozen implementation and verification requirements. Resolve performed-route tempo and simultaneous ramps without written-order winners; isolate malformed sibling instructions; use exact shared rounding; bound candidate and note-off work; preserve typed diagnostic/ledger ownership even without sounding notes; independently reject lost tie inheritance and corrupt source attribution. Keep all frozen musical interpretation and original baseline. The user explicitly requires preserving previous successful corrections: amend in place, never revert those fixes. KEEP PB+2/chant+1/a01+7, five PB exclusions, all IDs/geometry/raw bytes, FR/EN pronunciation, MIDI/no-expression behavior, native/historical pins and fixed corpus acceptance. Worker handoffs under combined-exp003-fid002/review-4 specify disjoint tasks and acceptance. Iteration3 passing receipts remain historical; rerun affected checks and final aggregate before acceptance. No new baseline exceptions.


### Iteration4 integration clarification

For source declarations in a zero-width written measure whose performed occurrence cannot be proved, retain one diagnostic per original raw identity at its exact written position and actual typed owner. Preserve the existing occurrence0/repeat_pass0 encoding with an explicit unresolved-performed-scope interpretation; no target track, no note IDs and no active expression claim. Exclude those ambiguous declarations and affected tempo candidates before performed state resolution. Do not create nominal measure duration or infer repeated occurrences. This bounded fallback is separate from positively proven performed runs and preserves compatibility.

The optional intensityContext ledger extension derives its source PPQ, original note/controller ownership and final eligible note placement independently of reported spans. It is required only for new intensity-bearing evidence; legacy schema2 and no-intensity schema3 reads remain supported. Exact outward source coverage and exact nearest target rounding share the engine arithmetic without changing Time serialization. Missing or invalid required context blocks publication.


## Verification — combined iteration4 frozen review candidate

Final shared implementation: Rust all-targets798passed/0failed/18ignored; strict Clippy, fmt and diff-check pass. One test-only initializer cleanup was followed by its focused regression and all-target Clippy. Frontend remains unchanged since its45pass/1optional-skip build/version0.6.3 gate. All prepared iteration4 regressions executed, including corrected valid source fixtures; no production acceptance rule was relaxed.

Configured original PB/chant against independent pinned historical goldens, exact+2/+1 holds and five PB exclusions pass. Score inventory22 cases, This Little924-note and Help source-fidelity, complete fake-render bundles pass. Native exact OpenUtau0.1.569.0:13 expression/load/sampling fixtures,3 standard negatives,107 score-oracle negatives,2 floor-free positive controls;6 continuity fixtures and6 negatives pass. Short fades use valid10-tick notes; no unsupported5-tick note is admitted.

Final264-case private aggregate:252serialized12expectedexact-grid refusals0discrepancies; all73 original/reference hashes intact and66 SVP profile comparisons identical. Fixed checker/acceptance pins unchanged. Receipt: corpus-2026-09-09/final-aggregate/run-final-1788996312931194000/summary.json. Root rlib SHA2565c34bf0afba2bf0180651930506bc2d4e793183affdd09da7d5f8606b290b4fa.

Pinned OpenScore1352discovered1343parsed1277projected75expectedineligible,0unexpected0evidence failures. MuseScore4.7.4 deterministic Holmès lc5661740 real render: full score64059702bytes plus2/2Partstems,0errors. These structural, target-consumer and renderer checks do not establish acoustic equivalence for every DiffSinger voice.

New PB/chant sibling previews prepared and validated against the exact final candidate, original metadata and all original backing audio. No original replaced. Independent combined review, exclusive publication and commits remain. Known unavailable corrupt Iko fixture, two ambiguous3.Et readings and unsupported broader numeric score pitch/vibrato remain explicitly tracked. Evidence: combined-exp003-fid002/review-4/final-execution-evidence.json and worker/parent result documents.


## Spec Change Log — combined review iteration 5

All three iteration4 reviews were collected before independent parent triage. [Iteration4 triage](combined-exp003-fid002/review-4/triage.md), R4-01–R4-16, clarifies the non-frozen implementation and verification requirements: parser-owned written declaration membership and silent/unplayed owners; pass-aware wedge pairing; reconciled exact tempo candidates and incompatible symbol behavior; real cumulative pre-allocation accounting; independent expression/controller applicability and occurrence validation; shared export rejection of lost proven tie intensity; exact native floor checking. The generic native probe remains backward compatible; final acceptance still enumerates the complete generated fixture set.

The user explicitly requires keeping existing corrections: amend in place, never revert successful changes. KEEP all original source bytes, IDs and geometry, PB+2/chant+1/a01+7, five exclusions, FR/EN pronunciation, original MIDI and no-expression behavior, all historical/native/public/private pins and passing positives/negatives. Never regenerate baselines or add acceptance exceptions. Do not change the frozen musical interpretation, nominal tempo ordering, wide rational encoding, schema2/no-intensity compatibility, or invent performed occurrences. Missing proof remains honest source-only evidence. Iteration4 passing results are historical; final integrated checks are required after these edits.

Ownership and executable regression requirements are detailed in the source, performance, verification and parent amendment specs under combined-exp003-fid002/review-5. All workers edit disjoint files; parent alone owns compilation and final integration. No staging/commits until accepted review.

### Iteration5 integration clarification

The continuity planner records a typed Tie versus Extension relation and the original attack-root note ID only after source tie proof. Final intensity ownership context may carry this root, independently of PerformanceNote and transfer spans, so a recovered tail may truthfully attribute its head velocity. The shared target gate checks the source contact/pitch/root chain and actual inherited attack fields with a cumulative bounded read-only comparison. An extension-only melisma cannot claim a tie attack. Legitimate issue lists, tail end and permitted attack-reference refresh remain separate from immutable inherited attack context. No target geometry or musical interpretation changes follow from this bookkeeping.


## Verification — combined iteration5 frozen review candidate

The final integrated all-target Rust gate passes 834 tests, zero failed, 18 conditional tests ignored. After an equivalent short-circuit Clippy cleanup, all46 actual bundle unit tests and21 saved-ledger integration tests pass. Strict Clippy, rustfmt and whitespace checks pass. Frontend45passed/1optional-skip, production build and version0.6.3 check pass. Narrow pre-review integration fixes preserve acceptance and are documented in combined-exp003-fid002/review-5/parent-integration-fixes.md; no temporary tracing remains.

Independent pinned private PB/chant nominal goldens, source-proven+2/+1 holds and five PB exclusions pass. The22-score inventory, This Little/Help source fidelity and complete fake-render bundles pass. Native OpenUtau0.1.569.0 at commit3f213e8993ca792c3e6f8958c92ab27eae78eac5 passes13 distinct expression fixtures,107 corrupted score-oracle negatives,3 actual wrong-floor target mutations,3 standard negatives,2 floor-free positives,6 continuity fixtures and6 continuity negatives. Python oracle tests8passed.

Final private matrix264cases:252serialized,12expected exact-grid refusals,0discrepancies; all73original/reference hashes intact and66SVP profile comparisons identical. No checker or acceptance pin changed. Root rlib SHA2566ca72919c8b7f2cdf4fc93ff23578ad94afa9889b6294ab0ba9f735a69798c87. Compile receipt: corpus-2026-09-09/final-aggregate/compile-final-1789002943962644000/receipt.json. Matrix summary: corpus-2026-09-09/final-aggregate/run-final-1789002947752276000/summary.json; its hash matches the prior accepted checked-field results.

Pinned OpenScore1352discovered/1343parsed/1277projected/75expectedineligible,0unexpected errors and0evidence failures. Final MuseScore4.7.4 Holmès lc5661740 real render passes: full score64059702bytes and2/2Partstems,0render errors. These tests do not establish acoustic equivalence across DiffSinger voices.

Fresh PB/chant sibling previews are prepared and validated with1561/1566notes,5vocal lanes each and10/6unchanged original backing parts. Their bytes match the prior prepared output. Originals are untouched. Independent review, exclusive sibling publication and commits remain. Known ambiguous3.Et readings, corrupt unavailable Iko fixture and broader numeric score pitch/vibrato limitations remain tracked. Durable evidence: combined-exp003-fid002/review-5/final-execution-evidence.json, amendment-evidence.md and the worker/parent result files.


## Review5 decision — review limit reached

All three independent reviews were collected and triaged together. The decision is recorded in [iteration5 triage](combined-exp003-fid002/review-5/triage.md): 15 non-frozen implementation/verification corrections, one patch-only boundary adoption fix and two rejected findings. Parent replay against the exact tested library confirms the missing source-head check, lost inherited attack provenance and bundle-constructor validation bypass. Other retained findings document static evidence and required executable coverage; no acoustic failure is inferred from these probes.

A spec-level loopback advances review_loop_iteration from5 to6. The rendered BMAD step04 requires escalation when the counter exceeds5, so implementation is paused awaiting a human decision on another bounded iteration. The tested code, full frozen patch and all existing corrections are preserved under the user's explicit KEEP instruction. No revert, staging, commit or sibling preview publication is performed at this boundary. The passing iteration5 receipts remain historical evidence for those exact bytes; they do not establish acceptance of the newly identified cases. Preserve all frozen musical interpretation, schema2/no-intensity compatibility, original source hashes and fixed private/public/native baselines.
