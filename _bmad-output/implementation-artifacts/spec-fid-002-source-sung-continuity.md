---
title: "FID-002: Preserve source-proven sung continuity across chord lanes and verses"
type: bugfix
created: 2026-09-09
status: in-review
review_loop_iteration: 6
baseline_commit: 1fa489fab16bf7b7f2fde2e63d663d41546202bf
initial_review_baseline_commit: 28abaea44e20951a32d26a61de68c10139d34712
execution_root: /private/tmp/verse-fid-002-sung-continuity
implementation_status: paused-review-limit
depends_on:
  - spec-fid-001-projected-note-evidence.md
integration_contracts:
  - spec-exp-002-midi-performance-curves.md
  - spec-exp-003-numeric-score-performance.md
context:
  - docs/contribution-guide.md
  - src-tauri/src/engine/midi.rs
  - src-tauri/src/engine/musescore.rs
  - src-tauri/src/engine/convert.rs
  - src-tauri/src/engine/projection.rs
  - src-tauri/src/engine/performance.rs
  - _bmad-output/implementation-artifacts/fr-003/melisma-eligibility-followup.md
  - _bmad-output/implementation-artifacts/fr-003/melisma-eligibility-evidence/current-probe-summary.json
---

## Intent and readiness

Restore only source-proven sung continuations currently lost by two mechanisms:
(1) a MuseScore lyric extension head and endpoint enter different technical chord
lanes; (2) a tied tail's other-verse lyric prevents tie merging before the active
verse is selected. Neither defect warrants transposition or invented syllables.

**Safe and adequate for the actual two reproductions.** Alti has one proven
chord/lyric owner and one touching same-pitch head after ownership is established.
Sop has an explicit, contiguous source tie and an unambiguous eligible first-pass
head. No source-data blocker remains for these cases. Unsupported or ambiguous
other cases must be diagnosed without fallback; they do not prevent implementing
the bounded contract. Ready-for-dev describes the specification, not completed
implementation or acceptance. Integrate against the parent's final FID001 metadata
seam and current EXP work; do not independently redesign either.

The initial authoring turn produced this specification only. Implementation is
authorized after the reviewed FID001 metadata seam is available. The parent
coordinates the isolated worktree, serial compiler slot, integration and commits;
workers must preserve concurrent EXP changes and must not edit original projects.

## Execution coordination

Implement in `/private/tmp/verse-fid-002-sung-continuity` only. This worktree contains the reviewed FID001 retention patch on top of EXP002 commit28abaea; `fid-002/integration-baseline.json` pins these pre-existing files. Preserve them and package only FID002 changes relative to that snapshot. The parent is verifying/committing FID001 in main; do not duplicate its patch or change its policy.

Do not edit main production, run Cargo, compile the whole engine, render, commit, or modify original source projects without the parent allocating the appropriate integration/compiler slot. Independent light checks and read-only source inspection are permitted. Request the serial compiler slot when ready. The parent handles integration and commits. EXP003 is independently extending PerformanceNote to typed MIDI/score ownership; preserve original ownership/Arc identity and coordinate that seam with the parent. FID003 is adding source-level staff-link metadata in an isolated worktree; do not overwrite it on integration.

## Evidence and exact reproduction

[Detailed current-engine audit](fr-003/melisma-eligibility-followup.md) and
[probe evidence](fr-003/melisma-eligibility-evidence/current-probe-summary.json)
record the original source paths, XML fragments, source IDs, fresh output and
code hashes. The audit executed the then-current engine, not an old bundle:
French PB1559 notes, chant1565; both had the omissions below. Current EXP integration
has since added `Option<PerformanceNote>`; its contract is incorporated below,
without claiming this specification reran conversion after that integration.

Sources under `/Users/jopad/Downloads/Musique_maman_a_convertire/Pour bugs/`:

- PB: `Au-bout-de-mes-reves-Goldman-SAB PB.mscz`, SHA-256
  `d859cc506b7510283b193b78914095dc25718b856ccdd2aa0544edeb24564ba5`.
- Chant: `Au-bout-SAB chant.mscz`, SHA-256
  `efb13f3809c15d099d60967f0b71a51daa3b6c7b06f0508d03baf90fecebd873`.

Positions are absolute playback ticks at480 PPQ; measures are one-based direct
master XML ordinals. PB's embedded part Scores are not independent source events.

| Case | Source statement | Required editable representation |
| --- | --- | --- |
| Both Alti, staff2/voice1, m11 Chord1→2, pass1 | Head19440/duration240 has pitches59 and68, one shared verse1 “Même”/“Même'” lyric with ticks240; endpoint19680/duration240/pitch68 has verse2-only “ger” | Keep both original heads. Add the single original endpoint as a hold of “Même”, routed to the lane containing the19440/pitch68 head |
| PB Sop, staff1/voice1, m20 Chord5→m21 Chord1, pass1 | Head38160/duration240/pitch64 has verse1 “murs”, verse2 “cou”; explicit tie to38400/duration960/pitch64 with verse2 “rants” | Keep head38160/240 and retain tail38400/960 as a separate `ProjectedLyric::Extension` in the same lane; continuous coverage through39360 |
| Alti pass2 | “chan” heads75120; eligible “ger”75360 | Existing eligible syllable and its identity remain; do not turn it into a hold or reroute it under this recovery rule |
| Sop pass2 | “cou”93840/240; eligible “rants”94080/960 | Preserve the separate source syllable/attack and existing pronunciation-profile split behavior |
| Chant Sop positive control | Same tie/verse shape, but verse1 “mur” also explicitly has ticks240 | Existing38400/960 Extension stays exactly once; tie proof must not add another copy |

MuseScore3.6.2 `Lyrics::_ticks` reaches the **start of the last covered chord**.
The chord's duration is included. Current `onset <= end` is already correct.
[`lyricsline.cpp:89–118`](https://github.com/musescore/MuseScore/blob/v3.6.2/libmscore/lyricsline.cpp#L89)
and [`lyrics.h:37–43`](https://github.com/musescore/MuseScore/blob/v3.6.2/libmscore/lyrics.h#L37)
establish the endpoint and temporary one-tick sentinel. Exact source copies are
in `fr-003/melisma-eligibility-evidence/`. Do not change the endpoint operator,
interpret ticks as total sound duration, or infer an extension from adjacency.

## Boundaries with FID001 and EXP

FID001 owns evidence retention, union across lyric projections and ledger fallback.
FID002 owns continuity proof, lyric continuation classification and the necessary
technical routing. Feed FID001 actual retained original identities. Do not fix
`build_preservation_ledger` by manually marking recovered IDs, checking geometry,
or bypassing its normal source/stem fallback. FID001 alone must leave musical
output unchanged; FID002 is the separately reviewable intentional output change.

Current EXP API:

- `ProjectedNote.performance: Option<PerformanceNote>` travels with the note.
- `PerformanceNote` contains `source_id`, `ChannelKey { port, channel }`, and
  `Arc<ChannelPerformance> timeline`.
- `PerformanceIndex.notes` is keyed by **original adapter track ID and note-on
  order**. Resolve before any routing; a destination lane is not a lookup key.
- `performance::normalize` currently handles MIDI/KAR only. Both score cases
  therefore carry `None` today. Preserve `None`; never manufacture a neutral
  performance payload or encode staff IDs as MIDI channels.

Every routing/filter/split operation must move the complete projected note and
its FID001 provenance atomically. Preserve any `Some` payload's own source ID,
port/channel, shared timeline, issues and point-source attribution. Do not borrow
head performance for the tail, rebuild it from destination identity, compose
curves twice, or turn unknown values into neutral/zero. EXP003 may later add score
scope; FID002 must remain agnostic to that representation. This increment adds
no dynamics, pitch curves, velocity interpretation, fades or expression mappings.

Keep all original bytes, IDs, repeat occurrences, pitches, onsets, durations,
nonaffected lyrics, singer assignments, source topology and stems unchanged.
Do not modify an existing user's project or bundle. Five editable lanes remain
five in these sources; moving an endpoint is not creating a new source voice.
Do not invoke a source-format-specific repair from a target serializer.

## Source ownership and neutral provenance contract

Extend the narrow source/projection metadata seam, preferably FID001's per-note
record, rather than introducing a second geometry-based identity index. Required
information must survive until retention and serialization:

1. Original note instance ID; original adapter track ID; original note-on/off
   orders; part/staff/voice; source chord ID; playback occurrence and occurrence
   segment across repeat/jump boundaries.
2. For an explicit extension: original lyric ID/row, owning chord ID, exact
   start and inclusive endpoint, source format/version and raw evidence reference.
3. For a validated tie: original head/tail source IDs and occurrence-qualified
   relationship, source link evidence, exact contact position and nominal pitch.
4. For a recovered note: its original identity plus a separate continuation-owner
   reference and destination technical lane. Destination metadata must never
   replace origin metadata. `PerformanceNote.source_id` is not a substitute for
   this universal record because most score notes currently have no performance.

Use typed adapter evidence, not source-ID prefix parsing, display names or a
projector guess about MuseScore versus MIDI. The MuseScore adapter already reads
explicit Spanner/Tie back references; retain that resolved relation even when a
text-bearing tail must remain an independent attack. Do not discard its incoming
tie merely because some verse has text. Preserve current validated handling of
bare merged tails and their source provenance; do not globally unmerge the corpus.

The first implementation populates new continuity evidence from the confirmed
MuseScore syntax. MusicXML/MXL and MIDI/KAR keep current behavior unless their
adapters explicitly supply an independently validated equivalent. No inference
from coincident pitch, implicit synthetic NoteOff events or source-family names.
Malformed, unsupported or contradictory relations retain raw evidence with a
scoped diagnostic; no guessed head or fabricated duration.

## Recovery and routing algorithm

Process deterministically in playback order; use exact checked tick arithmetic.
Compute an identity-keyed recovery plan before untexted filtering and before any
pronunciation transformation or lane split that would assume a previous note.
Do not depend on incidental iteration order among simultaneous chord members.

### A. Prove extension ownership first

Use the existing playback-eligible lyric selection, including profile-specific
blank duplicate handling, without rewriting verse/refrain policy. A candidate
endpoint must have no eligible own sung syllable. An explicit empty/unsupported
or conflicting selected lyric is not permission to replace it with a hold.

Gather heads within the same original part/staff/voice and playback segment.
They must reference the **same original source chord AND same selected lyric
identity/row**, with compatible extension values and occurrence. Equal lyric
text is insufficient. A validated source extension must cover the actual endpoint
chord; no intervening selected word, rest, repeat jump, contradictory extension,
unmapped pitch or missing endpoint may be silently bridged. Retain the inclusive
bound and exact fraction conversion. No/zero/temporary single-chord extension
cannot authorize a later ordinary chord.

For the actual Alti case the shared owner is:

- chord `mscx:staff:2:measure:10:voice:0:chord:1`;
- verse1 lyric `mscx:staff:2:measure:10:voice:0:chord:1-lyric-0`;
- head note members `...:chord:1:note:0` and `...:chord:1:note:1`, occurrence0;
- endpoint `mscx:staff:2:measure:10:voice:0:chord:2:note:0`, occurrence0.

The parser's two lyric copies are representations of that one owner. They do
not authorize two endpoint copies. Competing nonidentical owners block recovery.

### B. Allocate a technical lane only after proof

Within the proven owner's head instances, require exact contact and an available
monophonic destination over the endpoint's full interval. An already explicit
predecessor link takes precedence. Otherwise, permit **exactly one touching
same-pitch head** to determine the technical lane. This is allocation among
already-proven representations of one owner, never pitch-based ownership
inference. No nearest pitch, highest/lowest pitch fallback, first free lane,
lexical similarity or source-order tie-break when that choice is ambiguous.

Here both heads end19680, but only member2 has MIDI68. Its lane is free over
19680–19920. Route the original endpoint once to
`mscx:staff:2:voice:1:polyphonic-member:2`, retain duration240/pitch68, and mark it
Extension. Remove it from the main lane's pending projection by original identity,
not by pitch/time matching. Its original instance remains:
`note:mscx:staff:2:voice:1:mscx:staff:2:measure:10:voice:0:chord:2:note:0:occurrence:0:event:18`.
Do not relabel that identity as originating in member2 or copy either head.

The preceding main-lane note ends18240; endpoint19680 is1440 ticks later. Appending
an Extension to main would bind the wrong predecessor and fails native USTX
continuity checks. Never fill that rest or extend the old note to hide the gap.

After a recovered chain is assigned, every downstream split must follow the
resolved predecessor identity, not `previous_lane` from unrelated iteration.
Require exactly one retained representation per original endpoint per intended
lyric projection; multiple legitimate lyric groups use FID001's union semantics.
Do not generalize routing into a new voice-leading algorithm. No unique safe
allocation means unresolved continuity, not a guessed musical arrangement.

### C. Apply tie continuity after eligible verse selection

For a source-validated incoming tie, first evaluate the tail's selected verse.
If it owns a sung syllable on this pass, retain the existing attack and text.
Otherwise a valid, touching, same-pitch sung head can authorize a hold only when
there is no explicit contradictory selected lyric, rest or playback boundary.
An untexted head cannot create a sung tie by itself. Unpitched/invalid/missing
links cannot be repaired by matching nearby notes.

For PB pass1, preserve the head's240 and tail's960 as **two projected notes**, with
the tail Extension linked to “murs”. This avoids rewriting duration or collapsing
the tail's identity/performance binding. It is the required representation for
this defect, not a mandate to unmerge already-correct bare ties elsewhere.
Pass2 “cou/rants” retains current attack/split behavior. Chant's existing explicit
extension and tie establish the same tail; coalesce proof, not output notes.

## Failure behavior

Add stable scoped diagnostics such as `SOURCE_CONTINUITY_OWNER_AMBIGUOUS`,
`SOURCE_CONTINUITY_ROUTE_UNRESOLVED` and `SOURCE_CONTINUITY_LINK_INVALID`, carrying
head/endpoint IDs, occurrence, source interval and preserving artifacts. Reuse an
existing equivalent code if present. Leave unresolved notes under existing
source/stem policy; never claim sung restoration or projectedExact for an absent
endpoint. A mandatory exporter-continuity/monophony violation still refuses via
the shared analysis/write gate. Do not weaken target checks or replace another
note's lyric to make serialization pass.

## Five explicit non-restoration cases

All remain excluded from editable vocals under the existing mixed-lane policy;
source bytes and owning Part stems preserve them. Do not infer the author's intent
from the different chant master or fill in a known song lyric.

| PB source span: onset/duration/MIDI | Why no sung recovery is authorized |
| --- | --- |
| Alti206160/240/66, m79 C3 | No lyrics/tie/extension; preceding “même” has no explicit continuation |
| Alti214800/480/64, m83 C7→m84 C1 | Both head and tied tail untexted; preceding “mes” does not extend |
| Bass180720/480/61, m66 C1 | No lyric/tie/extension; preceding “presse,” ends before a240-tick rest |
| Bass206160/240/57, m79 C3 | No lyrics/tie/extension from preceding “même” |
| Bass214800/240/59, m83 C7 | No lyrics/tie/extension; do not supply “rêves” after “mes” |

FID001 must report source/stem fallback for these dropped identities, not an
editable-note claim. FID002 does not turn their notation into explicit rests,
remove it from the preserved source, or change whole-untexted-lane override policy.

## Implementation map and order

| Module/seam | Required implementation |
| --- | --- |
| `engine/midi.rs` source metadata | Add optional typed continuity evidence/relationship references with backward-compatible absent defaults. Preserve all existing IDs and constructors; do not overload performance fields |
| `engine/musescore.rs`: `chord_lyrics`, tie resolution and chord buckets | Retain extension/chord ownership and validated tie links, including text-bearing tails. Preserve source duration/version evidence and existing bucket classification |
| `engine/convert.rs`: extraction, eligibility, `project_track` | Keep original instance/order metadata; attach EXP performance by original key; build identity-keyed eligible continuity plan across sibling adapter tracks |
| Projection preparation and pronunciation boundary | Separate raw eligibility/ownership from existing per-lane linguistic postprocessing as narrowly as required; route owned atoms before previous-note-dependent transforms. Preserve existing French source-context annotations and repeat domains |
| `drop_untexted` / `split_simultaneous_voices` | Consume FID001's single retention/provenance seam; move notes plus metadata/performance together; retain proof-linked predecessor through splitting; no geometry identity joins |
| `engine/projection.rs` | Keep neutral Extension and optional performance intact; add only the minimum ownership/link support required by the shared projection contract |
| `engine/target/{ustx,svp}.rs` | Reuse native marker spelling, exact timing and monophony checks. No source parsing or correction here; test the repaired output |
| `bundle.rs`, `lib.rs`, stems/topology | Integration tests only unless FID001's provenance API requires caller wiring. Feed actual origin IDs, keep existing stem inventory/parts stable; no separate ledger workaround |
| Tests | Parser link fixtures; projector routing/eligibility negatives; source-fidelity, language/profile, performance and bundle evidence tests; bounded native USTX consumer verification |

Integrate after FID001's reviewed retention record and current EXP attachment seam
are available. Do not create a competing sidecar or overwrite parent's shared-file
work. Performance transfer status remains separate from nominal-note evidence.

## Acceptance: fixtures, private corpus and native consumer

All checks below are implementation acceptance, **not executed for this spec**.
Use compact committed synthetic fixtures for CI; private source scores remain
local and read-only. Fresh outputs go to a new temporary directory.

1. **Exact corpus delta.** On each master, French Alti gains only its missing
   original19680/240/68 endpoint in member2, with the original ID and a touching
   “Même” head. PB additionally gains38400/960/64 Extension after “murs”. With
   the audited French baseline and this required separate-tail representation:
   PB1559→1561, chant1565→1566, still five vocal lanes. Default keeps its existing
   blank-duplicate policy; only the same two proven continuity deltas are allowed
   (audited PB1550→1552, chant1565→1566). Rebaseline counts if parent work changes
   them, but require an explained identity-level delta; never use counts alone.
2. **No collateral music changes.** Compare retained original IDs, exact
   pitch/onset/duration, eligible lyric ownership and performance payloads.
   All previous notes remain; no duplicated endpoint, swapped staff, new overlap,
   additional voice, altered source tempo or healed rest. Pass2 and chant Sop
   positive control stay unchanged. Verify the previously rendered de/mes/rêves
   and Oui/j'irai windows remain source-exact. The nine French PB recoveries stay.
3. **Owner-before-lane negatives.** Same text/different lyric IDs; same pitch in
   another part/staff/voice; wrong occurrence; conflicting extension copies;
   zero/two touching same-pitch candidates; occupied destination; missing endpoint;
   stale source link; a rest; an intervening selected word: diagnose, no fallback.
   One shared owner plus unique touching same-pitch candidate succeeds regardless
   of technical lane iteration order. Move the tail once; do not merge distinct
   same-geometry source IDs.
4. **Eligibility/tie matrix.** First-pass tie hold versus second-pass owned
   syllable; ordinary lyric-free tie already merged; both ends untexted; explicit
   empty/unsupported/conflicting selected lyric; invalid back reference; pitch
   mismatch; gap; repeat jump; chained ties. Preserve eligible syllable attacks
   and existing bare-tie behavior. All five plain PB exclusions must remain absent.
5. **Endpoint semantics.** ticks960 and fraction-only1/2 include a chord at960
   and exclude1440 in a quarter-note fixture; absent/zero/temporary ticks1 add
   no later chord. Preserve exact-grid refusal and overflow checks. An in-memory
   Extension placed in Alti main at19680 must still be rejected for the gap.
6. **EXP compatibility.** Unit fixtures route whole `Some(PerformanceNote)`
   values without changing source_id/key/timeline ownership (shared Arc identity),
   even when destination metadata differs; `None` remains None. Retain controller
   event attribution and held state across rests. Existing MIDI/KAR port/channel,
   PITD/DYN and unsupported-target tests remain unchanged. Do not claim score
   expression support from synthetic adapter events. Contradictory same-key
   timelines must not be silently interchanged during routing.
7. **FID001 integration.** Build actual fixture ledgers from final conversion;
   compare recovered endpoint/head note and on/off IDs to retained output, including
   moved-origin IDs and verse union. Five exclusions have source/stem fallback.
   No identity guessed from geometry or destination track. Existing preservation
   snapshots are evidence, not a substitute for executing the current builder.
8. **Both targets and profiles.** Default, French and English source eligibility
   share the proof; preserve profile text rules. Validate SVP neutral timing/marker
   output and USTX exact480-grid output. Analysis and write gates agree. No new
   source-specific source ownership logic in either adapter.
9. **Native USTX acceptance.** Use the installed/pinned OpenUtau reflective consumer
   on temporary phrase exports: load saved YAML and inspect native note/part
   timing, overlap status and continuation grouping. Alti19680 must group under
   the actual19440/pitch68 “Même” head; PB38400 under38160 “murs”; second-pass
   syllables retain their own eligible reading. Exercise existing consumer
   grouping/validation methods without SetSinger, model loading or full song
   synthesis. Native success is required in addition to text inspection; an
   unavailable consumer is a recorded verification blocker, not a pass. No F0
   render, app preferences, unsaved-document writes or user reapproval needed.
10. **Broader corpus and local gate.** Run configured MSCZ/MXL gates and existing
    source/language/performance regressions; classify every changed identity by
    explicit source proof. Fail configured-but-missing fixtures. No forced clean
    diffs by dropping newly exposed cases. Run `docs/contribution-guide.md`'s
    required implementation gate when parent releases the slot (RTK prefix,
    at most two build jobs/two test threads). No Cargo is run to author this spec.

## Done criteria

Both concrete source-owned losses are restored with native-valid continuity,
original identity/performance retained and no source guessing. The five plain
untexted spans remain source/stem-only. FID001 reports actual representations.
Document bounded coverage and unresolved diagnostics, without claiming every
untexted notation is singable or every source score is now losslessly editable.

## Spec baseline

Current source contracts inspected for this specification (SHA-256):

- `engine/projection.rs`: `fca62000d166306866dd7f857943ac36351fc1024526fa24e305a199377d4d74`.
- `engine/performance.rs`: `6b6a7dfedf7d6e91f6e5a635d00f9cf5ea45cfc99d30a3f6806efb8b21af708d`.
- `engine/convert.rs`: `362d0e74dbcbf86d36496a2043308049b93d6fc5d892f1c964bcdd70cafe4cdb`.
- `engine/musescore.rs`: `c84f6b12249bc8ce163650e0bc56cdf35635a293e5b263dc66aa38757b79691c`.
- `engine/midi.rs`: `74670b07f19741b32d0b2a654e79c25cc8f5f5570ecceb0506756df6f00d5fd4`.
- `engine/target/ustx/performance.rs`: `21d7d9925b3ff6308460fe715efcbe92446cc71f46c0d3073eabe0061d6cfa6b`.

These hashes document the integration seam read; they are not a claim that the
parent must freeze those files or that implementation verification has passed.

## Review clarification — iteration 1

Apply [independent review triage](fid-002/review-triage.md), preserving the frozen
source-fidelity contract and already passing repairs. Admit untexted endpoints
only under typed compatible source ownership; resolve row inventories consistently.
Independent proven tie chains must survive parallel lyrics or unison endpoints,
while contradictory fields, missing links and conflicting same-owner claims are
diagnosed. Verify complete lyric/predecessor/destination ownership at the final
gate and performance source attribution through actual serialization and ledger
construction. Bound planner work/diagnostics. Add legacy-tie and scaled-timebase
coverage plus a real pre-change private-master comparison, keeping the current
metadata-disabled comparison only as supplemental evidence.

The user requires preserving implemented corrections. KEEP: exact two PB/one
chant additions, five exclusions, source bytes/identities and original performance
payloads, source-first verse eligibility, inclusive bounds, no invented singing,
existing native positives/negatives and both targets. No destructive code reset.
The separately diagnosed old Cabaret corpus assertions are parent-owned; they do
not authorize changing the released harmony policy in this increment.

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

### Iteration 2 corpus amendment — merged final endpoint

C13: the source adapter can absorb the final covered tied chord into a later, untexted continuation chord within an authored melisma. Endpoint existence must follow the validated original tie references to that retained chord, including original occurrence, pitch and contact, rather than require the lyric owner’s own chord. This does not establish ownership by geometry: every retained intermediate still needs the existing unique touching predecessor and selected-word/gap checks. Actual loader tests cover changing-pitch intermediates, both writers and explicit pronunciation profiles, with rest/new-word negatives. The seven original notes from the a01 private source must remain in the aggregate; they must never become a baseline exception. KEEP: every frozen policy, the two PB/one chant restorations and the five explicit PB exclusions.

### Iteration 2 diagnostic capacity

C08 public replay exposed a legitimate multi-verse report reaching245 warnings and131115 charged bytes. Keep the1024-warning and4096-byte per-record bounds, with a512KiB total report budget so ordinary hundreds-of-note scores are not refused for their source-scoped diagnostics alone. Exceeding any boundary remains a stable SOURCE_CONTINUITY_LIMIT error with actual count/bytes. Do not drop warnings, suppress ambiguous ownership, or change the public corpus baseline. Test ordinary512-record output and each hard refusal independently.

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
