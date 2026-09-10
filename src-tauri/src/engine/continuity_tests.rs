//! Synthetic FID002 contracts. Included as a child module of continuity.rs.
//! These tests intentionally assert the specification, including refusal paths.
use super::super::{
    extract_notes, prepare_track, ExportRepresentation, LyricStatus, LyricStatusState, SourceRole,
    TrackProjection,
};
use super::*;
use crate::engine::midi::{
    Lyric, Midi, SourceContinuity, SourceContinuityIssue, SourceEvidenceRef, SourceFormat,
    SourceTie,
};
use crate::engine::musescore::parse_mscx;
use crate::engine::performance::{
    ChannelKey, ChannelPerformance, Dimension, HeldPoint, PerformanceIndex, PerformanceIssue,
    PerformanceNote,
};
use crate::engine::projection::{NoteEvidence, ProjectedProject};
use crate::engine::target::PronunciationProfile;
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn ordinary_multi_verse_diagnostics_fit_while_all_report_bounds_remain_active() {
    let mut budget = Budget::default();
    for _ in 0..512 {
        budget.diagnostic(600).unwrap();
    }
    assert!(budget.diagnostic(4097).is_err());
    let mut count = Budget::default();
    for _ in 0..1024 {
        count.diagnostic(1).unwrap();
    }
    assert!(count.diagnostic(1).is_err());
    let mut bytes = Budget::default();
    for _ in 0..128 {
        bytes.diagnostic(4096).unwrap();
    }
    assert!(bytes.diagnostic(1).is_err());
}

trait MidiBindingExt {
    fn timeline(&self) -> &Arc<ChannelPerformance>;
}
impl MidiBindingExt for PerformanceNote {
    fn timeline(&self) -> &Arc<ChannelPerformance> {
        let crate::engine::performance::PerformanceOwner::Midi { timeline, .. } = &self.owner
        else {
            panic!("expected original MIDI owner")
        };
        timeline
    }
}

type Pending = Vec<(usize, Vec<String>, ProjectedTrack)>;

#[test]
fn simultaneous_split_follows_resolved_predecessor_instead_of_incidental_previous_lane() {
    let (head, tail) = tied_pair();
    let mut lanes = pending(vec![vec![head, tail]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:0", "head", "first-verse");
    let mut track = lanes.remove(0).2;
    let head = track.notes[0].clone();
    let tail = track.notes[1].clone();
    let mut incidental = note("incidental", 0, 480, 64);
    word(&mut incidental, "unrelated-lyric", "other");
    // Both simultaneous heads touch the tail and share its pitch. The unrelated
    // head is processed last, making lane 1 the incidental previous_lane.
    track.notes = vec![head.clone(), incidental.clone(), tail.clone()];

    let split = super::super::split_simultaneous_voices(track);

    assert_eq!(split.len(), 2);
    assert_eq!(split[0].notes, vec![head, tail]);
    assert_eq!(split[1].notes, vec![incidental]);
    let project = ProjectedProject {
        tracks: split,
        ..ProjectedProject::default()
    };
    assert_eq!(project.monophony_violation(), None);
    assert_eq!(project.continuity_violation(), None);
}

fn evidence(id: &str) -> SourceEvidenceRef {
    SourceEvidenceRef {
        source_format: SourceFormat::MuseScore,
        source_version: Some("3.02".into()),
        program_version: Some("3.6.2".into()),
        source_id: id.into(),
        raw_xml: format!("<synthetic id=\"{id}\"/>").into(),
    }
}

fn note(id: &str, onset: u32, duration: u32, pitch: u8) -> ProjectedNote {
    ProjectedNote {
        onset_ticks: onset,
        duration_ticks: duration,
        pitch,
        lyric: ProjectedLyric::Absent,
        performance: None,
        source_evidence: Some(NoteEvidence {
            note_id: format!("note:{id}"),
            note_on_event_id: format!("event:{id}:on"),
            note_off_event_id: format!("event:{id}:off"),
            lyric_id: None,
            lyric_event_id: None,
            origin: Some(NoteOrigin {
                track_id: format!("original:{id}"),
                note_on_order: 17,
                note_off_order: 23,
                source: NoteSource {
                    id: id.into(),
                    part_id: Some("part".into()),
                    staff_id: Some("staff".into()),
                    voice: Some("voice".into()),
                    chord_id: Some(format!("chord:{id}")),
                    continuity: Some(std::sync::Arc::new(SourceContinuity {
                        evidence: evidence(id),
                        chord_id: format!("chord:{id}"),
                        playback_segment: 0,
                        extensions: Vec::new(),
                        incoming_tie: None,
                        issues: Vec::new(),
                    })),
                    ..NoteSource::default()
                },
                lyric_conflict: false,
                continuation: None,
            }),
        }),
    }
}

fn origin(note: &ProjectedNote) -> &NoteOrigin {
    note.source_evidence
        .as_ref()
        .unwrap()
        .origin
        .as_ref()
        .unwrap()
}

fn origin_mut(note: &mut ProjectedNote) -> &mut NoteOrigin {
    note.source_evidence
        .as_mut()
        .unwrap()
        .origin
        .as_mut()
        .unwrap()
}

fn continuity_mut(note: &mut ProjectedNote) -> &mut SourceContinuity {
    std::sync::Arc::make_mut(origin_mut(note).source.continuity.as_mut().unwrap())
}

fn word(note: &mut ProjectedNote, id: &str, text: &str) {
    note.lyric = ProjectedLyric::Source(Box::new(Lyric::text(id, text.into())));
    note.source_evidence.as_mut().unwrap().lyric_id = Some(format!("lyric:{id}"));
}

fn head(id: &str, pitch: u8, lyric_id: &str, chord: &str, end: u32) -> ProjectedNote {
    let mut note = note(id, 0, 480, pitch);
    word(&mut note, lyric_id, "same text");
    origin_mut(&mut note).source.chord_id = Some(chord.into());
    let continuity = continuity_mut(&mut note);
    continuity.chord_id = chord.into();
    continuity.extensions.push(SourceExtension {
        lyric_id: lyric_id.into(),
        lane: "1".into(),
        chord_id: chord.into(),
        occurrence: 0,
        playback_segment: 0,
        start_tick: 0,
        end_tick: Some(end),
        extend_ticks: Some(i64::from(end)),
        extend_fraction: None,
        raw_ticks: Some(i64::from(end)),
        evidence: evidence(lyric_id),
    });
    note
}

fn reference(note: &ProjectedNote) -> SourceNoteRef {
    let source = &origin(note).source;
    SourceNoteRef {
        source_id: source.id.clone(),
        occurrence: source.occurrence,
        playback_segment: source.continuity.as_ref().unwrap().playback_segment,
    }
}

fn tie(head: &ProjectedNote, tail: &mut ProjectedNote) {
    let link = SourceTie {
        head: reference(head),
        tail: reference(tail),
        contact_tick: tail.onset_ticks,
        pitch: tail.pitch,
        evidence: vec![
            evidence(&origin(head).source.id),
            evidence(&origin(tail).source.id),
        ],
    };
    continuity_mut(tail).incoming_tie = Some(link);
}

fn pending(lanes: Vec<Vec<ProjectedNote>>) -> Pending {
    lanes
        .into_iter()
        .enumerate()
        .map(|(i, notes)| {
            (
                i,
                vec!["1".into()],
                ProjectedTrack {
                    name: "Identical display name".into(),
                    source_track_id: format!("technical:{i}"),
                    muted: false,
                    notes,
                },
            )
        })
        .collect()
}

fn reports(pending: &Pending) -> Vec<TrackReport> {
    (0..pending.len())
        .map(|id| TrackReport {
            id,
            source_id: pending
                .iter()
                .find(|entry| entry.0 == id)
                .map(|entry| entry.2.source_track_id.clone())
                .unwrap_or_default(),
            track: "Voice".into(),
            notes: 0,
            role: "vocal".into(),
            placed: 0,
            source_role: SourceRole::Vocal,
            lyric_status: LyricStatus {
                state: LyricStatusState::SourceOwned,
                source_text_count: 0,
                projected_text_count: 0,
                explicit_empty_count: 0,
                continuation_count: 0,
                unsupported_count: 0,
            },
            export_representation: ExportRepresentation::VocalNotes,
            requires_voice_assignment: true,
            warnings: Vec::new(),
        })
        .collect()
}

fn run(pending: &mut Pending) -> Vec<TrackReport> {
    run_with_sources(pending, &[])
}

fn run_with_sources(pending: &mut Pending, sources: &[Vec<SourceNote>]) -> Vec<TrackReport> {
    let mut report = reports(pending);
    resolve(
        pending,
        sources,
        &mut report,
        &crate::engine::performance::PerformanceIndex::default(),
    )
    .expect("bounded continuity plan");
    report
}

fn find<'a>(pending: &'a Pending, id: &str) -> (&'a str, &'a ProjectedNote) {
    let matches: Vec<_> = pending
        .iter()
        .flat_map(|(_, _, track)| {
            track
                .notes
                .iter()
                .filter(move |note| origin(note).source.id == id)
                .map(move |note| (track.source_track_id.as_str(), note))
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "each original atom must remain exactly once: {id}"
    );
    matches[0]
}

fn assert_unresolved(pending: &Pending, before: &ProjectedNote, lane: &str) {
    let (actual_lane, actual) = find(pending, &origin(before).source.id);
    assert_eq!(actual_lane, lane);
    assert_eq!(
        actual, before,
        "refusal must not alter lyric, geometry, origin or performance"
    );
}

fn assert_code(report: &[TrackReport], code: &str, note_id: &str) {
    let warning = report
        .iter()
        .flat_map(|track| &track.warnings)
        .find(|warning| warning.code == code && warning.source_id.as_deref() == Some(note_id))
        .unwrap_or_else(|| panic!("missing scoped {code} for {note_id}: {report:#?}"));
    assert_eq!(warning.severity, DiagnosticSeverity::Warning);
    assert!(warning.message.contains("occurrence"));
    assert!(warning.message.contains("interval"));
    assert!(warning.message.contains("source"));
}

fn assert_hold(pending: &Pending, id: &str, lane: &str, predecessor: &str, owner: &str) {
    let (actual_lane, note) = find(pending, id);
    assert_eq!(actual_lane, lane);
    assert_eq!(note.lyric, ProjectedLyric::Extension);
    let link = origin(note).continuation.as_ref().unwrap();
    assert_eq!(link.predecessor_id, format!("note:{predecessor}"));
    assert_eq!(link.lyric_owner_id, owner);
    assert_eq!(link.destination_track_id, lane);
}

#[test]
fn shared_chord_owner_routes_once_independently_of_technical_lane_iteration() {
    for reverse in [false, true] {
        let low = head("low", 59, "owner", "chord", 480);
        let high = head("high", 68, "owner", "chord", 480);
        let tail = note("tail", 480, 240, 68);
        let before = tail.clone();
        let mut lanes = pending(vec![vec![tail], vec![low.clone()], vec![high.clone()]]);
        if reverse {
            lanes.reverse();
        }
        let report = run(&mut lanes);
        assert_hold(&lanes, "tail", "technical:2", "high", "owner");
        let routed = find(&lanes, "tail").1;
        let mut expected = before;
        expected.lyric = ProjectedLyric::Extension;
        origin_mut(&mut expected).continuation = origin(routed).continuation.clone();
        assert_eq!(routed, &expected);
        assert_eq!(find(&lanes, "low").1, &low);
        assert_eq!(find(&lanes, "high").1, &high);
        assert!(report.iter().all(|track| track.warnings.is_empty()));
        assert_eq!(
            lanes
                .iter()
                .map(|(_, _, track)| track.notes.len())
                .sum::<usize>(),
            3
        );
    }
}

#[test]
fn equal_text_and_unique_same_pitch_do_not_merge_distinct_lyric_owners() {
    for same_chord in [false, true] {
        let low = head("low", 59, "lyric-a", "chord-a", 480);
        let high = head(
            "high",
            68,
            "lyric-b",
            if same_chord { "chord-a" } else { "chord-b" },
            480,
        );
        let tail = note("tail", 480, 240, 68);
        let mut lanes = pending(vec![vec![tail.clone()], vec![low], vec![high]]);
        let report = run(&mut lanes);
        assert_unresolved(&lanes, &tail, "technical:0");
        assert_code(&report, OWNER_AMBIGUOUS, "note:tail");
        let warning = &report[0].warnings[0];
        assert!(warning.message.contains("lyric-a") && warning.message.contains("lyric-b"));
    }
}

#[test]
fn same_lyric_id_on_different_chords_is_still_competing_ownership() {
    let tail = note("tail", 480, 240, 68);
    let mut lanes = pending(vec![
        vec![tail.clone()],
        vec![head("a", 59, "owner", "chord-a", 480)],
        vec![head("b", 68, "owner", "chord-b", 480)],
    ]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_code(&report, OWNER_AMBIGUOUS, "note:tail");
}

#[test]
fn identical_extension_copies_coalesce_proof_without_copying_output() {
    let mut owner = head("head", 68, "owner", "chord", 480);
    let copy = continuity_mut(&mut owner).extensions[0].clone();
    continuity_mut(&mut owner).extensions.push(copy);
    let mut lanes = pending(vec![vec![note("tail", 480, 240, 68)], vec![owner]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:1", "head", "owner");
    assert_eq!(lanes[1].2.notes.len(), 2);
}

#[test]
fn duplicate_identical_evidence_is_still_one_head_for_local_pitch_contour() {
    let mut owner = head("head", 60, "owner", "chord", 480);
    let copy = continuity_mut(&mut owner).extensions[0].clone();
    continuity_mut(&mut owner).extensions.push(copy);
    let mut lanes = pending(vec![vec![owner, note("tail", 480, 240, 62)]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:0", "head", "owner");
    assert_eq!(lanes[0].2.notes.len(), 2);
}

#[test]
fn conflicting_extension_copies_block_the_entire_owner() {
    let a = head("a", 59, "owner", "chord", 480);
    let b = head("b", 68, "owner", "chord", 960);
    let tail = note("tail", 480, 240, 68);
    let mut lanes = pending(vec![
        vec![tail.clone(), note("endpoint", 960, 240, 68)],
        vec![a],
        vec![b],
    ]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_code(&report, LINK_INVALID, "note:a");
}

#[test]
fn invalid_extension_copy_cannot_be_discarded_in_favor_of_valid_sibling() {
    let mut invalid = head("a", 59, "owner", "chord", 480);
    continuity_mut(&mut invalid).extensions[0].end_tick = None;
    continuity_mut(&mut invalid)
        .issues
        .push(SourceContinuityIssue {
            code: LINK_INVALID,
            message: "Contradictory numeric extension copy".into(),
            evidence: evidence("owner"),
        });
    let tail = note("tail", 480, 240, 68);
    let mut lanes = pending(vec![
        vec![tail.clone()],
        vec![invalid],
        vec![head("b", 68, "owner", "chord", 480)],
    ]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_code(&report, LINK_INVALID, "note:b");
}

#[test]
fn zero_extension_copy_conflicts_with_positive_sibling_before_allocation() {
    let zero = head("zero", 59, "owner", "chord", 0);
    let positive = head("positive", 68, "owner", "chord", 480);
    let tail = note("tail", 480, 240, 68);
    let mut lanes = pending(vec![vec![tail.clone()], vec![zero], vec![positive]]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_code(&report, OWNER_AMBIGUOUS, "note:positive");
}

#[test]
fn contradictory_duplicate_extensions_on_one_representation_also_block() {
    for invalid_end in [Some(0), None] {
        let mut owner = head("head", 68, "owner", "chord", 480);
        let mut contradictory = continuity_mut(&mut owner).extensions[0].clone();
        contradictory.end_tick = invalid_end;
        contradictory.raw_ticks = Some(0);
        contradictory.extend_ticks = Some(0);
        continuity_mut(&mut owner).extensions.push(contradictory);
        let tail = note("tail", 480, 240, 68);
        let mut lanes = pending(vec![vec![tail.clone()], vec![owner]]);
        let report = run(&mut lanes);
        assert_unresolved(&lanes, &tail, "technical:0");
        assert_code(&report, OWNER_AMBIGUOUS, "note:head");
    }
}

#[test]
fn same_pitch_in_another_source_domain_cannot_allocate_the_owner() {
    for field in ["part", "staff", "voice", "segment"] {
        let mut foreign = head("foreign", 68, "owner", "chord", 480);
        let source = &mut origin_mut(&mut foreign).source;
        match field {
            "part" => source.part_id = Some("other".into()),
            "staff" => source.staff_id = Some("other".into()),
            "voice" => source.voice = Some("other".into()),
            "segment" => {
                let continuity = std::sync::Arc::make_mut(source.continuity.as_mut().unwrap());
                continuity.playback_segment = 1;
                continuity.extensions[0].playback_segment = 1;
            }
            _ => unreachable!(),
        }
        let tail = note("tail", 480, 240, 68);
        let mut lanes = pending(vec![
            vec![tail.clone()],
            vec![head("local", 59, "owner", "chord", 480)],
            vec![foreign],
        ]);
        let report = run(&mut lanes);
        assert_unresolved(&lanes, &tail, "technical:0");
        assert_code(&report, ROUTE_UNRESOLVED, "note:tail");
    }
}

#[test]
fn coeval_shared_chord_copies_with_different_occurrences_are_contradictory() {
    let local = head("local", 59, "owner", "chord", 480);
    let mut foreign = head("foreign", 68, "owner", "chord", 480);
    origin_mut(&mut foreign).source.occurrence = 1;
    continuity_mut(&mut foreign).extensions[0].occurrence = 1;
    let tail = note("tail", 480, 240, 68);
    let mut lanes = pending(vec![vec![tail.clone()], vec![local], vec![foreign]]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_code(&report, OWNER_AMBIGUOUS, "note:foreign");
}

#[test]
fn zero_or_two_touching_same_pitch_candidates_refuse_without_pitch_fallback() {
    for pitches in [[59, 67], [68, 68]] {
        let tail = note("tail", 480, 240, 68);
        let mut lanes = pending(vec![
            vec![tail.clone()],
            vec![head("a", pitches[0], "owner", "chord", 480)],
            vec![head("b", pitches[1], "owner", "chord", 480)],
        ]);
        let report = run(&mut lanes);
        assert_unresolved(&lanes, &tail, "technical:0");
        assert_code(&report, ROUTE_UNRESOLVED, "note:tail");
    }
}

#[test]
fn technical_colocation_does_not_break_a_two_candidate_allocation_tie() {
    let tail = note("tail", 480, 240, 68);
    let mut lanes = pending(vec![
        vec![head("a", 68, "owner", "chord", 480), tail.clone()],
        vec![head("b", 68, "owner", "chord", 480)],
    ]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_code(&report, ROUTE_UNRESOLVED, "note:tail");
}

#[test]
fn touching_wrong_pitch_local_member_does_not_override_unique_owned_same_pitch() {
    let mut lanes = pending(vec![
        vec![
            head("low", 59, "owner", "chord", 480),
            note("tail", 480, 240, 68),
        ],
        vec![head("high", 68, "owner", "chord", 480)],
    ]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:1", "high", "owner");
}

#[test]
fn occupied_destination_checks_the_full_tail_interval_and_does_not_try_another_lane() {
    for onset in [480, 600, 719] {
        let blocker = note("blocker", onset, 120, 70);
        let tail = note("tail", 480, 240, 68);
        let mut lanes = pending(vec![
            vec![tail.clone()],
            vec![head("owner", 68, "lyric", "chord", 480), blocker.clone()],
            vec![],
        ]);
        let report = run(&mut lanes);
        assert_unresolved(&lanes, &tail, "technical:0");
        assert_eq!(find(&lanes, "blocker").1, &blocker);
        assert_code(&report, ROUTE_UNRESOLVED, "note:tail");
    }
}

#[test]
fn destination_boundary_contact_is_available() {
    let mut lanes = pending(vec![
        vec![note("tail", 480, 240, 68)],
        vec![
            head("head", 68, "owner", "chord", 480),
            note("after", 720, 240, 70),
        ],
    ]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:1", "head", "owner");
}

#[test]
fn missing_final_endpoint_blocks_even_an_earlier_touching_note() {
    let tail = note("tail", 480, 240, 68);
    let mut lanes = pending(vec![
        vec![tail.clone()],
        vec![head("head", 68, "owner", "chord", 960)],
    ]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_code(&report, LINK_INVALID, "note:head");
}

#[test]
fn selected_intervening_word_blocks_an_extension_chain() {
    let tail = note("tail", 960, 240, 68);
    let mut middle = note("middle", 480, 480, 68);
    word(&mut middle, "new-word", "new");
    let mut lanes = pending(vec![
        vec![tail.clone()],
        vec![head("head", 68, "owner", "chord", 960), middle.clone()],
    ]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_eq!(find(&lanes, "middle").1, &middle);
    assert_code(&report, LINK_INVALID, "note:tail");
}

fn source_obstacle(
    head: &ProjectedNote,
    onset: u32,
    duration: u32,
    pitch: Option<u8>,
) -> SourceNote {
    let mut source = origin(head).source.clone();
    source.id = "source-only-obstacle".into();
    SourceNote {
        onset,
        duration,
        pitch,
        source_order: 29,
        end_order: 31,
        source,
        lyrics: Vec::new(),
    }
}

#[test]
fn source_rest_unmapped_pitch_or_zero_duration_blocks_extension() {
    for (duration, pitch) in [(120, None), (0, Some(68))] {
        let owner = head("head", 68, "owner", "chord", 480);
        let obstacle = source_obstacle(&owner, 240, duration, pitch);
        let tail = note("tail", 480, 240, 68);
        let mut lanes = pending(vec![vec![tail.clone()], vec![owner]]);
        let report = run_with_sources(&mut lanes, &[vec![obstacle]]);
        assert_unresolved(&lanes, &tail, "technical:0");
        assert_code(&report, LINK_INVALID, "note:head");
    }
}

#[test]
fn gap_inside_explicit_extension_is_not_filled_or_healed() {
    let mut owner = head("head", 68, "owner", "chord", 480);
    owner.duration_ticks = 240;
    let before = owner.clone();
    let tail = note("tail", 480, 240, 68);
    let mut lanes = pending(vec![vec![tail.clone()], vec![owner]]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_eq!(find(&lanes, "head").1, &before);
    assert_code(&report, ROUTE_UNRESOLVED, "note:tail");
}

#[test]
fn extension_chain_uses_resolved_predecessor_and_inclusive_endpoint_once() {
    let mut lanes = pending(vec![
        vec![
            note("last", 960, 480, 68),
            note("middle", 480, 480, 68),
            note("outside", 1440, 480, 68),
        ],
        vec![head("head", 68, "owner", "chord", 960)],
    ]);
    let outside = find(&lanes, "outside").1.clone();
    run(&mut lanes);
    assert_hold(&lanes, "middle", "technical:1", "head", "owner");
    assert_hold(&lanes, "last", "technical:1", "middle", "owner");
    assert_unresolved(&lanes, &outside, "technical:0");
    assert_eq!(
        lanes[1]
            .2
            .notes
            .iter()
            .map(|note| note.onset_ticks)
            .collect::<Vec<_>>(),
        vec![0, 480, 960]
    );
}

fn tied_pair() -> (ProjectedNote, ProjectedNote) {
    let mut head = note("head", 0, 480, 64);
    word(&mut head, "first-verse", "word");
    let mut tail = note("tail", 480, 960, 64);
    tie(&head, &mut tail);
    (head, tail)
}

#[test]
fn valid_tie_keeps_two_original_attacks_and_tail_duration() {
    let (head, tail) = tied_pair();
    let before = head.clone();
    let mut lanes = pending(vec![vec![tail], vec![head]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:1", "head", "first-verse");
    assert_eq!(find(&lanes, "head").1, &before);
    assert_eq!(find(&lanes, "tail").1.duration_ticks, 960);
    assert_eq!(lanes[1].2.notes.len(), 2);
}

#[test]
fn selected_tail_text_empty_unsupported_or_conflict_cannot_be_replaced() {
    for state in [
        LyricState::Text("second verse".into()),
        LyricState::ExplicitEmpty,
        LyricState::Unsupported("humming".into()),
    ] {
        let (head, mut tail) = tied_pair();
        let mut lyric = Lyric::text("tail-lyric", "original bytes".into());
        lyric.state = state;
        tail.lyric = ProjectedLyric::Source(Box::new(lyric));
        let mut lanes = pending(vec![vec![tail.clone()], vec![head]]);
        run(&mut lanes);
        assert_unresolved(&lanes, &tail, "technical:0");
    }
    let (head, mut tail) = tied_pair();
    origin_mut(&mut tail).lyric_conflict = true;
    let mut lanes = pending(vec![vec![tail.clone()], vec![head]]);
    run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
}

#[test]
fn untexted_or_conflicting_head_cannot_create_a_sung_tie() {
    for conflict in [false, true] {
        let (mut head, tail) = tied_pair();
        if conflict {
            origin_mut(&mut head).lyric_conflict = true;
        } else {
            head.lyric = ProjectedLyric::Absent;
        }
        let mut lanes = pending(vec![vec![tail.clone()], vec![head]]);
        run(&mut lanes);
        assert_unresolved(&lanes, &tail, "technical:0");
    }
}

#[test]
fn stale_tie_references_wrong_pitch_contact_occurrence_and_gaps_refuse() {
    for defect in [
        "head-id",
        "tail-id",
        "head-occurrence",
        "tail-segment",
        "pitch",
        "contact",
        "head-pitch",
        "gap",
    ] {
        let (mut head, mut tail) = tied_pair();
        match defect {
            "head-pitch" => head.pitch = 65,
            "gap" => head.duration_ticks = 479,
            other => {
                let link = continuity_mut(&mut tail).incoming_tie.as_mut().unwrap();
                match other {
                    "head-id" => link.head.source_id = "missing".into(),
                    "tail-id" => link.tail.source_id = "another-tail".into(),
                    "head-occurrence" => link.head.occurrence = 1,
                    "tail-segment" => link.tail.playback_segment = 1,
                    "pitch" => link.pitch = 65,
                    "contact" => link.contact_tick = 481,
                    _ => unreachable!(),
                }
            }
        }
        let mut lanes = pending(vec![vec![tail.clone()], vec![head]]);
        let report = run(&mut lanes);
        assert_unresolved(&lanes, &tail, "technical:0");
        assert_code(&report, LINK_INVALID, "note:tail");
    }
}

#[test]
fn tied_head_in_another_part_staff_voice_occurrence_or_segment_is_not_a_match() {
    for field in ["part", "staff", "voice", "occurrence", "segment"] {
        let (mut head, tail) = tied_pair();
        let source = &mut origin_mut(&mut head).source;
        match field {
            "part" => source.part_id = Some("other".into()),
            "staff" => source.staff_id = Some("other".into()),
            "voice" => source.voice = Some("other".into()),
            "occurrence" => source.occurrence = 1,
            "segment" => {
                std::sync::Arc::make_mut(source.continuity.as_mut().unwrap()).playback_segment = 1
            }
            _ => unreachable!(),
        }
        let mut lanes = pending(vec![vec![tail.clone()], vec![head]]);
        let report = run(&mut lanes);
        assert_unresolved(&lanes, &tail, "technical:0");
        assert_code(&report, LINK_INVALID, "note:tail");
    }
}

#[test]
fn source_rest_also_blocks_tie_only_recovery() {
    let (head, tail) = tied_pair();
    let rest = source_obstacle(&head, 480, 120, None);
    let mut lanes = pending(vec![vec![tail.clone()], vec![head]]);
    let report = run_with_sources(&mut lanes, &[vec![rest]]);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_code(&report, LINK_INVALID, "note:tail");
}

#[test]
fn chained_ties_retain_original_owner_and_each_immediate_predecessor() {
    let (head, mut middle) = tied_pair();
    middle.duration_ticks = 480;
    let mut last = note("last", 960, 240, 64);
    tie(&middle, &mut last);
    let mut lanes = pending(vec![vec![last], vec![middle], vec![head]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:2", "head", "first-verse");
    assert_hold(&lanes, "last", "technical:2", "tail", "first-verse");
}

#[test]
fn tie_and_extension_coalesce_on_existing_extension_without_duplicate_tail() {
    let owner = head("head", 64, "owner", "chord", 480);
    let mut tail = note("tail", 480, 960, 64);
    tail.lyric = ProjectedLyric::Extension;
    tie(&owner, &mut tail);
    let mut lanes = pending(vec![vec![tail], vec![owner]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:1", "head", "owner");
    assert_eq!(lanes[1].2.notes.len(), 2);
}

#[test]
fn explicit_tie_selects_one_of_two_same_pitch_members_of_the_proven_owner() {
    let a = head("a", 68, "owner", "chord", 480);
    let b = head("b", 68, "owner", "chord", 480);
    let mut tail = note("tail", 480, 240, 68);
    tie(&b, &mut tail);
    let mut lanes = pending(vec![vec![tail], vec![a], vec![b]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:2", "b", "owner");
}

#[test]
fn tie_cannot_override_a_different_extension_owner() {
    let owner = head("owner", 68, "extension-lyric", "chord", 480);
    let mut other = note("tie-head", 0, 480, 68);
    word(&mut other, "different-lyric", "same text");
    // Conflicting selected ownership on the same source chord remains blocking.
    origin_mut(&mut other).source.chord_id = Some("chord".into());
    continuity_mut(&mut other).chord_id = "chord".into();
    let mut tail = note("tail", 480, 240, 68);
    tie(&other, &mut tail);
    let mut lanes = pending(vec![vec![tail.clone()], vec![owner], vec![other]]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_code(&report, OWNER_AMBIGUOUS, "note:owner");
}

#[test]
fn no_typed_evidence_means_no_inference_from_source_name_or_geometry() {
    let mut owner = head("mscx:head", 68, "owner", "chord", 480);
    let mut tail = note("mscx:tail", 480, 240, 68);
    origin_mut(&mut owner).source.continuity = None;
    origin_mut(&mut tail).source.continuity = None;
    let mut lanes = pending(vec![vec![tail.clone()], vec![owner]]);
    run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
}

#[test]
fn coincident_original_tail_ids_are_not_merged_or_removed_by_geometry() {
    for reverse in [false, true] {
        let a = note("tail-a", 480, 240, 68);
        let b = note("tail-b", 480, 240, 68);
        let mut lanes = pending(vec![
            vec![a.clone()],
            vec![b.clone()],
            vec![head("head", 68, "owner", "chord", 480)],
        ]);
        if reverse {
            lanes.reverse();
        }
        let report = run(&mut lanes);
        assert_unresolved(&lanes, &a, "technical:0");
        assert_unresolved(&lanes, &b, "technical:1");
        assert_code(&report, OWNER_AMBIGUOUS, "note:tail-a");
        assert_code(&report, OWNER_AMBIGUOUS, "note:tail-b");
        assert_eq!(
            lanes
                .iter()
                .map(|(_, _, track)| track.notes.len())
                .sum::<usize>(),
            3
        );
    }
}

#[test]
fn distinct_intended_lyric_projections_each_keep_their_own_endpoint() {
    let owner = head("head", 68, "owner", "chord", 480);
    let tail = note("tail", 480, 240, 68);
    let mut second_owner = owner.clone();
    word(&mut second_owner, "owner-row-two", "same text");
    if let ProjectedLyric::Source(lyric) = &mut second_owner.lyric {
        lyric.lane = "2".into();
    }
    let extension = &mut continuity_mut(&mut second_owner).extensions[0];
    extension.lyric_id = "owner-row-two".into();
    extension.lane = "2".into();
    let mut lanes = pending(vec![
        vec![tail.clone()],
        vec![owner],
        vec![tail],
        vec![second_owner],
    ]);
    lanes[2].1 = vec!["2".into()];
    lanes[3].1 = vec!["2".into()];
    // These are two intended groups of each original adapter track, unlike
    // singleton tracks whose differing verse inventories must be normalized.
    lanes[2].0 = 0;
    lanes[3].0 = 1;
    run(&mut lanes);
    for destination in [1, 3] {
        let notes = &lanes[destination].2.notes;
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[1].lyric, ProjectedLyric::Extension);
        assert_eq!(origin(&notes[1]).source.id, "tail");
        assert_eq!(
            origin(&notes[1])
                .continuation
                .as_ref()
                .unwrap()
                .destination_track_id,
            format!("technical:{destination}")
        );
    }
    assert!(lanes[0].2.notes.is_empty() && lanes[2].2.notes.is_empty());
}

fn performance(id: &str, value: f64, key: ChannelKey) -> PerformanceNote {
    PerformanceNote {
        source_id: format!("performance:{id}"),
        intensity: None,
        owner: crate::engine::performance::PerformanceOwner::Midi {
            key,
            timeline: Arc::new(ChannelPerformance {
                pitch_cents: vec![
                    HeldPoint {
                        tick: 0,
                        value: Some(value),
                        source_ids: vec![format!("controller:{id}:bend")],
                    },
                    HeldPoint {
                        tick: 600,
                        value: None,
                        source_ids: vec![format!("controller:{id}:unknown")],
                    },
                ],
                linear_gain: vec![
                    HeldPoint {
                        tick: 0,
                        value: Some(0.5),
                        source_ids: vec![
                            format!("controller:{id}:cc7"),
                            format!("controller:{id}:cc11"),
                        ],
                    },
                    HeldPoint {
                        tick: 240,
                        value: Some(0.0),
                        source_ids: vec![format!("controller:{id}:mute")],
                    },
                    HeldPoint {
                        tick: 720,
                        value: Some(0.75),
                        source_ids: vec![format!("controller:{id}:restore")],
                    },
                ],
                issues: vec![PerformanceIssue {
                    tick: 600,
                    dimension: Dimension::PitchCents,
                    source_ids: vec![format!("controller:{id}:unknown")],
                    reason: format!("Unsupported source state for {id}"),
                }],
            }),
        },
    }
}

#[test]
fn routing_moves_full_performance_even_with_contradictory_same_key_timelines() {
    for same_key in [false, true] {
        let mut owner = head("head", 68, "owner", "chord", 480);
        let mut tail = note("tail", 480, 240, 68);
        let key = ChannelKey {
            port: 4,
            channel: 9,
        };
        let other_key = if same_key {
            key
        } else {
            ChannelKey {
                port: 2,
                channel: 3,
            }
        };
        owner.performance = Some(performance("head", 120.0, other_key));
        tail.performance = Some(performance("tail", -75.0, key));
        let before_head = owner.clone();
        let before_tail = tail.clone();
        let tail_timeline = tail.performance.as_ref().unwrap().timeline().clone();
        let head_timeline = owner.performance.as_ref().unwrap().timeline().clone();
        assert!(!Arc::ptr_eq(&tail_timeline, &head_timeline));
        let mut lanes = pending(vec![vec![tail], vec![owner]]);
        run(&mut lanes);
        assert_hold(&lanes, "tail", "technical:1", "head", "owner");
        let routed = find(&lanes, "tail").1;
        assert_eq!(routed.performance, before_tail.performance);
        assert!(Arc::ptr_eq(
            routed.performance.as_ref().unwrap().timeline(),
            &tail_timeline
        ));
        assert!(!Arc::ptr_eq(
            routed.performance.as_ref().unwrap().timeline(),
            &head_timeline
        ));
        assert_eq!(find(&lanes, "head").1, &before_head);
        assert!(Arc::ptr_eq(
            find(&lanes, "head")
                .1
                .performance
                .as_ref()
                .unwrap()
                .timeline(),
            &head_timeline
        ));
        let mut expected = before_tail;
        expected.lyric = ProjectedLyric::Extension;
        origin_mut(&mut expected).continuation = origin(routed).continuation.clone();
        assert_eq!(
            routed, &expected,
            "only classification and continuation destination may change"
        );
        if same_key {
            let project = consumer_project(
                lanes
                    .into_iter()
                    .map(|(_, _, t)| t)
                    .filter(|t| !t.notes.is_empty())
                    .collect(),
            );
            for target in [
                crate::engine::target::ExportTarget::Ustx,
                crate::engine::target::ExportTarget::Svp,
            ] {
                assert!(
                    crate::engine::target::validate_for(target, &project).is_err(),
                    "contradictory channel state must not serialize"
                );
                assert!(crate::engine::target::serialize_to(target, &project).is_err());
            }
        }
    }
}

#[test]
fn absent_performance_remains_none_even_when_the_head_has_a_payload() {
    let (mut owner, tail) = tied_pair();
    owner.performance = Some(performance(
        "head",
        50.0,
        ChannelKey {
            port: 1,
            channel: 2,
        },
    ));
    let mut lanes = pending(vec![vec![tail], vec![owner]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:1", "head", "first-verse");
    assert_eq!(find(&lanes, "tail").1.performance, None);
}

fn score(body: &str) -> String {
    format!(
        r#"<museScore version="3.02"><programVersion>3.6.2</programVersion><Score><Division>480</Division><Part><trackName>Voice</trackName><Staff id="1"/></Part><Staff id="1"><Measure><voice>{body}</voice></Measure></Staff></Score></museScore>"#
    )
}

fn chord(pitch: u8, lyrics: &str, spanners: &str) -> String {
    format!("<Chord><durationType>quarter</durationType>{lyrics}<Note><pitch>{pitch}</pitch>{spanners}</Note></Chord>")
}

const TIE_START: &str = r#"<Spanner type="Tie"><Tie/><next><location><fractions>1/4</fractions></location></next></Spanner>"#;
const TIE_STOP: &str = r#"<Spanner type="Tie"><prev><location><fractions>-1/4</fractions></location></prev></Spanner>"#;

fn parsed_pending(midi: &Midi) -> (Pending, Vec<Vec<SourceNote>>) {
    let sources: Vec<_> = midi.tracks.iter().map(extract_notes).collect();
    let lanes = vec!["1".into()];
    let performance = PerformanceIndex::default();
    let standalone = HashMap::new();
    let pending = midi
        .tracks
        .iter()
        .zip(&sources)
        .enumerate()
        .map(|(index, (track, notes))| {
            let mut diagnostics = Vec::new();
            let projected = prepare_track(
                "Voice",
                notes,
                TrackProjection {
                    source_track_id: &track.id,
                    performance: &performance,
                    lanes: &lanes,
                    standalone: &standalone,
                    profile: PronunciationProfile::Default,
                    diagnostics: &mut diagnostics,
                },
            );
            (index, lanes.clone(), projected)
        })
        .collect();
    (pending, sources)
}

#[test]
fn parser_ticks_and_fraction_include_final_chord_and_exclude_next() {
    for extension in ["<ticks>960</ticks>", "<ticks_f>1/2</ticks_f>"] {
        let xml = score(&format!(
            "{}{}{}{}",
            chord(
                60,
                &format!("<Lyrics><text>sing</text>{extension}</Lyrics>"),
                ""
            ),
            chord(62, "", ""),
            chord(64, "", ""),
            chord(65, "", "")
        ));
        let midi = parse_mscx(&xml).unwrap();
        let (mut lanes, sources) = parsed_pending(&midi);
        let report = run_with_sources(&mut lanes, &sources);
        let notes = &lanes[0].2.notes;
        assert_eq!(
            notes
                .iter()
                .map(|note| (note.onset_ticks, note.duration_ticks, note.pitch))
                .collect::<Vec<_>>(),
            vec![
                (0, 480, 60),
                (480, 480, 62),
                (960, 480, 64),
                (1440, 480, 65)
            ]
        );
        assert!(matches!(notes[0].lyric, ProjectedLyric::Source(_)));
        assert_eq!(notes[1].lyric, ProjectedLyric::Extension);
        assert_eq!(notes[2].lyric, ProjectedLyric::Extension);
        assert_eq!(notes[3].lyric, ProjectedLyric::Absent);
        let extension = &origin(&notes[0])
            .source
            .continuity
            .as_ref()
            .unwrap()
            .extensions[0];
        assert_eq!(extension.end_tick, Some(960));
        assert_eq!(extension.evidence.source_format, SourceFormat::MuseScore);
        assert!(report.iter().all(|track| track.warnings.is_empty()));
    }
}

#[test]
fn parser_no_zero_or_temporary_one_tick_extension_authorizes_no_later_chord() {
    for extension in ["", "<ticks>0</ticks>", "<ticks>1</ticks>"] {
        let midi = parse_mscx(&score(&format!(
            "{}{}",
            chord(
                60,
                &format!("<Lyrics><text>sing</text>{extension}</Lyrics>"),
                ""
            ),
            chord(60, "", "")
        )))
        .unwrap();
        let (mut lanes, sources) = parsed_pending(&midi);
        run_with_sources(&mut lanes, &sources);
        assert_eq!(lanes[0].2.notes[1].lyric, ProjectedLyric::Absent);
        assert!(origin(&lanes[0].2.notes[1]).continuation.is_none());
    }
}

#[test]
fn parser_conflicting_numeric_extension_retains_raw_evidence_and_refuses_hold() {
    let raw = "<Lyrics><text>sing</text><ticks>480</ticks><ticks_f>1/2</ticks_f></Lyrics>";
    let midi = parse_mscx(&score(&format!(
        "{}{}{}",
        chord(60, raw, ""),
        chord(60, "", ""),
        chord(60, "", "")
    )))
    .unwrap();
    let (mut lanes, sources) = parsed_pending(&midi);
    let head_id = lanes[0].2.notes[0]
        .source_evidence
        .as_ref()
        .unwrap()
        .note_id
        .clone();
    let report = run_with_sources(&mut lanes, &sources);
    let continuity = origin(&lanes[0].2.notes[0])
        .source
        .continuity
        .as_ref()
        .unwrap();
    assert_eq!(continuity.extensions[0].end_tick, None);
    assert_eq!(continuity.extensions[0].raw_ticks, Some(480));
    assert_eq!(continuity.extensions[0].extend_fraction, Some((1, 2)));
    assert_eq!(continuity.extensions[0].evidence.raw_xml.as_ref(), raw);
    assert_eq!(lanes[0].2.notes[1].lyric, ProjectedLyric::Absent);
    assert_eq!(lanes[0].2.notes[2].lyric, ProjectedLyric::Absent);
    assert_code(&report, LINK_INVALID, &head_id);
}

#[test]
fn parser_rejects_fraction_outside_exact_tick_grid() {
    let xml = score(&chord(
        60,
        "<Lyrics><text>sing</text><ticks_f>1/7</ticks_f></Lyrics>",
        "",
    ));
    let error = parse_mscx(&xml).unwrap_err();
    assert!(error.contains("lyric") && error.contains("1/7"), "{error}");
}

#[test]
fn parser_retains_source_tie_when_another_verse_requires_separate_tail_attack() {
    let midi = parse_mscx(&score(&format!(
        "{}{}",
        chord(64, "<Lyrics><text>first</text></Lyrics>", TIE_START),
        chord(
            64,
            "<Lyrics><no>1</no><text>second</text></Lyrics>",
            TIE_STOP
        )
    )))
    .unwrap();
    let sources: Vec<_> = midi.tracks.iter().flat_map(extract_notes).collect();
    assert_eq!(sources.len(), 2);
    assert_eq!((sources[0].duration, sources[1].duration), (480, 480));
    let link = sources[1]
        .source
        .continuity
        .as_ref()
        .unwrap()
        .incoming_tie
        .as_ref()
        .unwrap();
    assert_eq!(link.head.source_id, sources[0].source.id);
    assert_eq!(link.tail.source_id, sources[1].source.id);
    assert_eq!(link.contact_tick, 480);
    assert_eq!(link.pitch, 64);
    assert_eq!(link.evidence.len(), 2);
    assert!(link
        .evidence
        .iter()
        .all(|evidence| evidence.raw_xml.contains("Spanner")));
    let (mut lanes, sources) = parsed_pending(&midi);
    run_with_sources(&mut lanes, &sources);
    assert_eq!(lanes[0].2.notes.len(), 2);
    assert_eq!(lanes[0].2.notes[1].lyric, ProjectedLyric::Extension);
}

#[test]
fn parser_invalid_back_reference_does_not_bind_nearby_same_pitch() {
    let bad_stop = TIE_STOP.replace("-1/4", "-1/2");
    let midi = parse_mscx(&score(&format!(
        "{}{}",
        chord(64, "<Lyrics><text>first</text></Lyrics>", TIE_START),
        chord(
            64,
            "<Lyrics><no>1</no><text>second</text></Lyrics>",
            &bad_stop
        )
    )))
    .unwrap();
    let (mut lanes, sources) = parsed_pending(&midi);
    let tail = &lanes[0].2.notes[1];
    assert!(origin(tail)
        .source
        .continuity
        .as_ref()
        .unwrap()
        .incoming_tie
        .is_none());
    let tail_id = tail.source_evidence.as_ref().unwrap().note_id.clone();
    let report = run_with_sources(&mut lanes, &sources);
    assert_eq!(lanes[0].2.notes[1].lyric, ProjectedLyric::Absent);
    assert_code(&report, LINK_INVALID, &tail_id);
}

#[test]
fn parser_existing_bare_tie_merge_remains_one_sustained_attack() {
    let midi = parse_mscx(&score(&format!(
        "{}{}",
        chord(64, "<Lyrics><text>sing</text></Lyrics>", TIE_START),
        chord(64, "", TIE_STOP)
    )))
    .unwrap();
    let played: Vec<_> = midi
        .tracks
        .iter()
        .flat_map(extract_notes)
        .filter(|note| note.pitch.is_some())
        .collect();
    assert_eq!(played.len(), 1);
    assert_eq!(
        (played[0].onset, played[0].duration, played[0].pitch),
        (0, 960, Some(64))
    );
    let (mut lanes, sources) = parsed_pending(&midi);
    run_with_sources(&mut lanes, &sources);
    assert_eq!(lanes[0].2.notes.len(), 1);
    assert_eq!(lanes[0].2.notes[0].duration_ticks, 960);
}

#[test]
fn successful_routing_preserves_monophony_without_adjusting_intervals() {
    let mut lanes = pending(vec![
        vec![note("tail", 480, 240, 68)],
        vec![head("head", 68, "owner", "chord", 480)],
    ]);
    run(&mut lanes);
    let project = ProjectedProject {
        tracks: lanes.into_iter().map(|(_, _, track)| track).collect(),
        ..ProjectedProject::default()
    };
    assert_eq!(project.monophony_violation(), None);
    assert_eq!(project.continuity_violation(), None);
}

#[test]
fn intervening_selected_word_blocks_tie_without_an_extension_owner() {
    let (head, tail) = tied_pair();
    let mut intervening = note("intervening", 240, 240, 67);
    word(&mut intervening, "new-word", "new");
    let mut lanes = pending(vec![vec![tail.clone()], vec![head, intervening.clone()]]);
    let report = run(&mut lanes);
    assert_unresolved(&lanes, &tail, "technical:0");
    assert_eq!(find(&lanes, "intervening").1, &intervening);
    assert_code(&report, LINK_INVALID, "note:tail");
}

#[test]
fn one_owner_one_touching_local_candidate_keeps_changing_pitch_contour() {
    let mut lanes = pending(vec![vec![
        head("head", 60, "owner", "chord", 960),
        note("middle", 480, 480, 62),
        note("tail", 960, 480, 64),
    ]]);
    run(&mut lanes);
    assert_hold(&lanes, "middle", "technical:0", "head", "owner");
    assert_hold(&lanes, "tail", "technical:0", "middle", "owner");
    assert_eq!(find(&lanes, "middle").1.pitch, 62);
    assert_eq!(find(&lanes, "tail").1.pitch, 64);
}

#[test]
fn untexted_unextended_notes_keep_full_performance_without_becoming_sung() {
    let key = ChannelKey {
        port: 7,
        channel: 4,
    };
    let mut head = note("untexted-head", 0, 480, 64);
    let mut tail = note("untexted-tail", 480, 240, 64);
    let mut after_rest = note("after-rest", 960, 240, 64);
    head.performance = Some(performance("untexted-head", 100.0, key));
    tail.performance = Some(performance("untexted-tail", -100.0, key));
    after_rest.performance = tail.performance.clone();
    tie(&head, &mut tail);
    let original = vec![head.clone(), tail.clone(), after_rest.clone()];
    let mut lanes = pending(vec![vec![head, tail, after_rest]]);
    run(&mut lanes);
    for before in &original {
        assert_unresolved(&lanes, before, "technical:0");
        let actual = find(&lanes, &origin(before).source.id).1;
        assert!(Arc::ptr_eq(
            actual.performance.as_ref().unwrap().timeline(),
            before.performance.as_ref().unwrap().timeline()
        ));
        assert!(!actual.lyric.is_sung());
    }
    assert!(Arc::ptr_eq(
        find(&lanes, "untexted-tail")
            .1
            .performance
            .as_ref()
            .unwrap()
            .timeline(),
        find(&lanes, "after-rest")
            .1
            .performance
            .as_ref()
            .unwrap()
            .timeline()
    ));
}

#[test]
fn analysis_and_write_reject_wrong_predecessor_identity_despite_matching_geometry() {
    use crate::engine::target::{serialize_to, validate_for, ExportTarget, SerializeError};
    let (head, tail) = tied_pair();
    let mut lanes = pending(vec![vec![head, tail]]);
    run(&mut lanes);
    let mut project = ProjectedProject {
        ticks_per_beat: 480,
        language: "english".into(),
        tracks: lanes.into_iter().map(|(_, _, track)| track).collect(),
        ..ProjectedProject::default()
    };
    assert_eq!(project.continuity_violation(), None);
    // Replace identity only: pitch, onset, duration and lyric remain identical.
    project.tracks[0].notes[0]
        .source_evidence
        .as_mut()
        .unwrap()
        .note_id = "note:unrelated-original".into();
    let violation = project
        .continuity_violation()
        .expect("geometry cannot satisfy an identity link");
    assert!(violation.contains("note:head"));
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        assert_eq!(validate_for(target, &project), Err(violation.clone()));
        match serialize_to(target, &project) {
            Err(SerializeError::Unrepresentable(message)) => assert_eq!(message, violation),
            other => panic!("write and analysis must reject the same identity mismatch: {other:?}"),
        }
    }
}

#[test]
fn continuous_repeat_exit_allows_head_occurrence_one_and_tail_occurrence_zero() {
    for use_tie in [false, true] {
        let mut owner = head("head", 68, "owner", "chord", 480);
        origin_mut(&mut owner).source.occurrence = 1;
        continuity_mut(&mut owner).extensions[0].occurrence = 1;
        let mut tail = note("tail", 480, 240, 68);
        if use_tie {
            continuity_mut(&mut owner).extensions.clear();
            tie(&owner, &mut tail);
        }
        let mut lanes = pending(vec![vec![tail], vec![owner]]);
        run(&mut lanes);
        assert_hold(&lanes, "tail", "technical:1", "head", "owner");
        assert_eq!(origin(find(&lanes, "head").1).source.occurrence, 1);
        assert_eq!(origin(find(&lanes, "tail").1).source.occurrence, 0);
    }
}

#[test]
fn singleton_verse_inventories_do_not_separate_proven_source_owner() {
    let mut lanes = pending(vec![
        vec![note("tail", 480, 240, 68)],
        vec![head("head", 68, "owner", "chord", 480)],
    ]);
    lanes[0].1 = vec!["1".into(), "2".into()];
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:1", "head", "owner");
}

#[test]
fn validated_merged_intermediate_tie_resolves_to_retained_sung_head() {
    let (mut owner, mut middle) = tied_pair();
    middle.duration_ticks = 480;
    let mut tail = note("last", 960, 240, 64);
    tie(&middle, &mut tail);
    // The adapter has already absorbed this bare intermediate into the head.
    let mut merged = source_obstacle(&middle, 480, 480, None);
    merged.source = origin(&middle).source.clone();
    let expected_source = merged.source.clone();
    owner.duration_ticks = 960;
    let mut lanes = pending(vec![vec![tail], vec![owner]]);
    run_with_sources(&mut lanes, &[vec![merged]]);
    assert_hold(&lanes, "last", "technical:1", "head", "first-verse");
    assert_eq!(find(&lanes, "head").1.duration_ticks, 960);
    let captured = &origin(find(&lanes, "last").1)
        .continuation
        .as_ref()
        .unwrap()
        .merged_tie_sources;
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].source, expected_source);
    assert_eq!(
        (captured[0].onset_ticks, captured[0].duration_ticks),
        (480, 480)
    );
    assert!(
        Arc::ptr_eq(
            captured[0].source.continuity.as_ref().unwrap(),
            expected_source.continuity.as_ref().unwrap()
        ),
        "raw XML remains shared"
    );
}

#[test]
fn selected_merged_tie_captures_intermediates_tail_to_head_without_retained_owner() {
    let (mut owner, mut first) = tied_pair();
    first.duration_ticks = 480;
    let mut second = note("second-merged", 960, 480, 64);
    tie(&first, &mut second);
    let mut tail = note("last", 1440, 240, 64);
    tie(&second, &mut tail);
    let originals: Vec<_> = [&first, &second]
        .into_iter()
        .map(|note| {
            let mut raw = source_obstacle(note, note.onset_ticks, note.duration_ticks, None);
            raw.source = origin(note).source.clone();
            raw
        })
        .collect();
    owner.duration_ticks = 1440;
    let mut lanes = pending(vec![vec![tail], vec![owner]]);
    run_with_sources(&mut lanes, &[originals.clone()]);
    assert_hold(&lanes, "last", "technical:1", "head", "first-verse");
    let captured = &origin(find(&lanes, "last").1)
        .continuation
        .as_ref()
        .unwrap()
        .merged_tie_sources;
    assert_eq!(captured.len(), 2);
    for (actual, expected) in captured.iter().zip(originals.iter().rev()) {
        assert_eq!(actual.source, expected.source);
        assert_eq!(
            (actual.onset_ticks, actual.duration_ticks),
            (expected.onset, expected.duration)
        );
    }
    assert_eq!(find(&lanes, "head").1.duration_ticks, 1440);
}

#[test]
fn direct_ties_and_lyric_extensions_capture_no_merged_intermediates() {
    for use_tie in [false, true] {
        let mut owner = head("head", 64, "owner", "chord", 480);
        let mut tail = note("last", 480, 240, 64);
        if use_tie {
            continuity_mut(&mut owner).extensions.clear();
            tie(&owner, &mut tail);
        }
        let mut lanes = pending(vec![vec![tail], vec![owner]]);
        run(&mut lanes);
        assert_hold(&lanes, "last", "technical:1", "head", "owner");
        assert!(origin(find(&lanes, "last").1)
            .continuation
            .as_ref()
            .unwrap()
            .merged_tie_sources
            .is_empty());
    }
}

#[test]
fn merged_source_copies_preflight_owned_metadata_and_poison_cumulative_budget() {
    let (_, middle) = tied_pair();
    let mut raw = source_obstacle(&middle, 480, 480, None);
    raw.source.unpitched = Some(crate::engine::midi::UnpitchedInfo {
        display_step: Some("large-source-metadata".repeat(1024)),
        ..Default::default()
    });
    for storage_limit in [false, true] {
        let budget = Budget::default();
        if storage_limit {
            budget.reserved_bytes.set(128 * 1024 * 1024 - 1024);
        } else {
            budget.work.set(MAX_WORK - 1);
        }
        let mut captured = Vec::new();
        assert!(push_merged_tie_source(&mut captured, &raw, &budget)
            .unwrap_err()
            .starts_with(LIMIT));
        assert!(
            captured.is_empty(),
            "the source must not be copied before preflight succeeds"
        );
        assert!(
            budget.charge(0).is_err(),
            "resource refusal cannot masquerade as absent proof"
        );
    }
}

#[test]
fn source_only_continuity_issue_survives_without_a_projected_atom() {
    let owner = note("unmapped", 0, 480, 64);
    let mut source = source_obstacle(&owner, 0, 480, None);
    std::sync::Arc::make_mut(source.source.continuity.as_mut().unwrap())
        .issues
        .push(SourceContinuityIssue {
            code: LINK_INVALID,
            message: "Unmapped source note has an invalid incoming tie".into(),
            evidence: evidence("source-only-obstacle"),
        });
    let expected_id =
        super::super::note_instance_id("technical:0", &source.source, source.source_order);
    let mut lanes = pending(vec![vec![]]);
    let report = run_with_sources(&mut lanes, &[vec![source]]);
    assert!(lanes[0].2.notes.is_empty());
    let warning = report[0]
        .warnings
        .iter()
        .find(|warning| warning.code == LINK_INVALID)
        .unwrap();
    assert_eq!(warning.source_id.as_deref(), Some(expected_id.as_str()));
    assert!(warning.message.contains("Unmapped source note"));
    assert!(warning.message.contains("occurrence 0"));
    assert!(warning.message.contains("source interval 0–480"));
}

#[test]
fn untexted_technical_lane_candidates_require_proof_and_honor_false_override() {
    use crate::engine::target::{self, ExportTarget};
    let xml = score(&format!(
        "{}{}{}",
        r#"<Chord><durationType>quarter</durationType><Lyrics><text>sing</text><ticks>480</ticks></Lyrics><Note><pitch>59</pitch></Note><Note><pitch>68</pitch></Note></Chord>"#,
        chord(68, "", ""),
        chord(70, "", "")
    ));
    let midi = parse_mscx(&xml).unwrap();
    let candidate_index = midi
        .tracks
        .iter()
        .position(|track| {
            track.events.iter().all(|event|
        !matches!(&event.kind, crate::engine::midi::Kind::NoteOn(note) if !note.lyrics.is_empty()))
        })
        .unwrap();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let result = super::super::convert_midi_with_target(&midi, "english", None, target);
        assert!(result.ok, "{:?}", result.msg);
        let project = result.svp.as_ref().unwrap();
        assert_eq!(
            project.tracks.len(),
            2,
            "candidate and unrelated empty lanes are not emitted"
        );
        let notes: Vec<_> = project.tracks.iter().flat_map(|t| &t.notes).collect();
        assert_eq!(notes.len(), 3);
        let tail = notes.iter().find(|n| n.onset_ticks == 480).unwrap();
        assert_eq!(tail.lyric, ProjectedLyric::Extension);
        assert_eq!(origin(tail).track_id, midi.tracks[candidate_index].id);
        assert_eq!(project.continuity_violation(), None);
        target::serialize_to(target, project).unwrap();
        let overrides = HashMap::from([(candidate_index, false)]);
        let disabled =
            super::super::convert_midi_with_target(&midi, "english", Some(&overrides), target);
        assert!(disabled.ok, "{:?}", disabled.msg);
        assert_eq!(
            disabled
                .svp
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .flat_map(|t| &t.notes)
                .count(),
            2
        );
    }
}

#[test]
fn singleton_and_multi_projection_siblings_match_actual_row_only() {
    for row in ["1", "2", "3"] {
        let first = head("head-one", 68, "owner-one", "chord", 480);
        let mut second = head("head-two", 68, "owner-two", "chord", 480);
        origin_mut(&mut second).track_id = origin(&first).track_id.clone();
        if let ProjectedLyric::Source(lyric) = &mut second.lyric {
            lyric.lane = "2".into();
        }
        continuity_mut(&mut second).extensions[0].lane = "2".into();
        let tail = note("tail", 480, 240, 68);
        let mut lanes = pending(vec![vec![tail.clone()], vec![first], vec![second]]);
        lanes[0].1 = vec![row.into()];
        lanes[1].1 = vec!["1".into()];
        lanes[2].1 = vec!["2".into()];
        run(&mut lanes);
        match row {
            "1" => assert_hold(&lanes, "tail", "technical:1", "head-one", "owner-one"),
            "2" => assert_hold(&lanes, "tail", "technical:2", "head-two", "owner-two"),
            _ => assert_unresolved(&lanes, &tail, "technical:0"),
        }
    }
}

#[test]
fn distinct_proven_unison_chains_survive_but_shared_predecessor_claims_conflict() {
    for shared in [false, true] {
        let mut first = note("first-head", 0, 480, 64);
        word(&mut first, "first-owner", "one");
        let mut second = note("second-head", 0, 480, 64);
        word(&mut second, "second-owner", "two");
        let mut a = note("a", 480, 480, 64);
        let mut b = note("b", 480, 480, 64);
        tie(&first, &mut a);
        tie(if shared { &first } else { &second }, &mut b);
        let mut lanes = pending(vec![
            vec![first],
            vec![second],
            vec![a.clone()],
            vec![b.clone()],
        ]);
        let report = run(&mut lanes);
        if shared {
            assert_unresolved(&lanes, &a, "technical:2");
            assert_unresolved(&lanes, &b, "technical:3");
            assert_code(&report, OWNER_AMBIGUOUS, "note:a");
            assert_code(&report, OWNER_AMBIGUOUS, "note:b");
        } else {
            assert_hold(&lanes, "a", "technical:0", "first-head", "first-owner");
            assert_hold(&lanes, "b", "technical:1", "second-head", "second-owner");
            let project = consumer_project(
                lanes
                    .into_iter()
                    .map(|(_, _, t)| t)
                    .filter(|t| !t.notes.is_empty())
                    .collect(),
            );
            for target in [
                crate::engine::target::ExportTarget::Svp,
                crate::engine::target::ExportTarget::Ustx,
            ] {
                crate::engine::target::serialize_to(target, &project).unwrap();
            }
        }
    }
}

#[test]
fn parallel_selected_words_and_extensions_do_not_override_an_explicit_chain() {
    let mut owner = note("head", 0, 960, 64);
    word(&mut owner, "owner", "long");
    let mut tail = note("tail", 960, 480, 64);
    tie(&owner, &mut tail);
    let mut parallel = head("parallel", 67, "parallel-owner", "other-chord", 960);
    parallel.duration_ticks = 480;
    let mut word_change = note("change", 480, 480, 67);
    word(&mut word_change, "new-word", "new");
    let mut lanes = pending(vec![vec![owner], vec![parallel, word_change], vec![tail]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:0", "head", "owner");
}

#[test]
fn final_gate_rejects_lyric_owner_and_destination_mutations_without_geometry_changes() {
    use crate::engine::target::{self, ExportTarget};
    let (head, tail) = tied_pair();
    let mut lanes = pending(vec![vec![head, tail]]);
    run(&mut lanes);
    let project = consumer_project(lanes.into_iter().map(|(_, _, t)| t).collect());
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        target::serialize_to(target, &project).unwrap();
    }
    for destination in [false, true] {
        let mut changed = project.clone();
        let link = origin_mut(&mut changed.tracks[0].notes[1])
            .continuation
            .as_mut()
            .unwrap();
        if destination {
            link.destination_track_id = "unrelated-lane".into();
        } else {
            link.lyric_owner_id = "unrelated-lyric".into();
        }
        assert!(changed.continuity_violation().is_some());
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            assert!(target::validate_for(target, &changed).is_err());
            assert!(target::serialize_to(target, &changed).is_err());
        }
    }
}

#[test]
fn indexed_large_chain_succeeds_and_diagnostic_limit_refuses_without_partial_success() {
    const COUNT: u32 = 8000;
    let mut chain = vec![head("root", 64, "owner", "chord", COUNT * 480)];
    for i in 1..=COUNT {
        chain.push(note(&format!("tail-{i}"), i * 480, 480, 64));
    }
    let mut lanes = pending(vec![chain]);
    run(&mut lanes);
    assert_eq!(lanes[0].2.notes.len(), COUNT as usize + 1);
    assert_eq!(
        origin(lanes[0].2.notes.last().unwrap())
            .continuation
            .as_ref()
            .unwrap()
            .lyric_owner_id,
        "owner"
    );
    let mut invalid = Vec::new();
    for i in 0..1100 {
        let mut n = note(&format!("invalid-{i}"), i * 480, 480, 64);
        origin_mut(&mut n).source.chord_id = Some("contradicts-typed-proof".into());
        invalid.push(n);
    }
    let mut lanes = pending(vec![invalid]);
    let mut report = reports(&lanes);
    assert!(resolve(
        &mut lanes,
        &[],
        &mut report,
        &crate::engine::performance::PerformanceIndex::default()
    )
    .unwrap_err()
    .starts_with(LIMIT));
    assert!(report.iter().map(|r| r.warnings.len()).sum::<usize>() <= 1024);
}

fn consumer_project(tracks: Vec<ProjectedTrack>) -> ProjectedProject {
    ProjectedProject {
        ticks_per_beat: 480,
        language: "english".into(),
        tracks,
        tempos: vec![crate::engine::projection::ProjectedTempo {
            tick: 0,
            bpm: 120.0,
            source: None,
            discovery_index: 0,
        }],
        meters: vec![crate::engine::projection::ProjectedMeter {
            bar_index: 0,
            numerator: 4,
            denominator: 4,
        }],
        ..ProjectedProject::default()
    }
}

#[test]
fn proven_routing_reaches_target_curves_with_shared_payload_and_original_report_owner() {
    use crate::engine::target::{self, ExportTarget};
    let timeline = Arc::new(ChannelPerformance {
        pitch_cents: vec![HeldPoint {
            tick: 0,
            value: Some(75.0),
            source_ids: vec!["control:bend".into()],
        }],
        linear_gain: vec![HeldPoint {
            tick: 0,
            value: Some(0.5),
            source_ids: vec!["control:gain".into()],
        }],
        issues: Vec::new(),
    });
    let key = ChannelKey {
        port: 2,
        channel: 3,
    };
    let mut owner = head("head", 68, "owner", "chord", 480);
    let mut tail = note("tail", 480, 240, 68);
    owner.performance = Some(PerformanceNote {
        source_id: "performance:head".into(),
        intensity: None,
        owner: crate::engine::performance::PerformanceOwner::Midi {
            key,
            timeline: timeline.clone(),
        },
    });
    tail.performance = Some(PerformanceNote {
        source_id: "performance:tail".into(),
        intensity: None,
        owner: crate::engine::performance::PerformanceOwner::Midi {
            key,
            timeline: timeline.clone(),
        },
    });
    let mut lanes = pending(vec![vec![tail], vec![owner]]);
    run(&mut lanes);
    assert_hold(&lanes, "tail", "technical:1", "head", "owner");
    let project = consumer_project(
        lanes
            .into_iter()
            .map(|(_, _, t)| t)
            .filter(|t| !t.notes.is_empty())
            .collect(),
    );
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        target::serialize_to(target, &project).unwrap();
        let reports = target::performance_report(target, &project).unwrap();
        let tail_reports: Vec<_> = reports
            .iter()
            .filter(|report| report.note_ids.contains(&"note:tail".to_string()))
            .collect();
        assert_eq!(tail_reports.len(), 2);
        for report in tail_reports {
            assert_eq!(report.track_id, "original:tail");
            assert_eq!(report.target_track, Some(0));
            assert_eq!((report.start_tick, report.end_tick), (480, 720));
            assert_eq!(report.note_ids, ["note:tail"]);
        }
    }
    let model = target::ustx::serialize(&project).unwrap();
    for (abbr, value) in [("pitd", 75), ("dyn", -60)] {
        let curves: Vec<_> = model.voice_parts[0]
            .curves
            .iter()
            .filter(|c| c.abbr == abbr)
            .collect();
        assert_eq!(curves.len(), 1);
        assert!(curves[0].ys.contains(&value));
    }
    assert!(Arc::ptr_eq(
        project.tracks[0].notes[1]
            .performance
            .as_ref()
            .unwrap()
            .timeline(),
        &timeline
    ));
    assert_eq!(
        project.tracks[0].notes[1]
            .performance
            .as_ref()
            .unwrap()
            .source_id,
        "performance:tail"
    );
}

#[test]
fn unrelated_source_notes_do_not_consume_the_continuity_note_ceiling() {
    let mut plain = note("plain", 0, 480, 60);
    origin_mut(&mut plain).source.continuity = None;
    let mut irrelevant = source_obstacle(&plain, 0, 480, Some(60));
    irrelevant.source = NoteSource::default();
    let sources = vec![vec![irrelevant; MAX_NOTES + 1]];
    let mut lanes = pending(vec![vec![plain.clone()]]);
    assert!(
        candidate_projections(&lanes, &sources, &BTreeSet::new(), &Budget::default())
            .unwrap()
            .is_empty()
    );
    run_with_sources(&mut lanes, &sources);
    assert_eq!(lanes[0].2.notes, vec![plain]);
    // A real typed voice still resolves when that unrelated material is present.
    let (head, tail) = tied_pair();
    let mut lanes = pending(vec![vec![head, tail]]);
    run_with_sources(&mut lanes, &sources);
    assert!(matches!(
        lanes[0].2.notes[1].lyric,
        ProjectedLyric::Extension
    ));
}

#[test]
fn fourth_review_discovery_borrows_identities_and_bounds_owned_row_copies() {
    let mut owner = head("head", 64, "owner", "chord", 480);
    let selected_row = "selected-row".repeat(8192);
    if let ProjectedLyric::Source(lyric) = &mut owner.lyric {
        lyric.lane = selected_row.clone();
    }
    continuity_mut(&mut owner).extensions[0].lane = selected_row.clone();
    let tail = note("tail", 480, 240, 64);
    let sources = vec![vec![], vec![source_obstacle(&tail, 480, 240, Some(64))]];
    let mut lanes = pending(vec![vec![owner]]);
    lanes[0].1 = vec![selected_row];
    let full =
        candidate_projections(&lanes, &sources, &BTreeSet::new(), &Budget::default()).unwrap();
    assert_eq!(full.len(), 1);
    assert_eq!(full[0].0, 1);
    assert_eq!(full[0].1, lanes[0].1);
    let budget = Budget::default();
    budget.reserved_bytes.set(128 * 1024 * 1024 - 4096);
    let before = lanes.clone();
    assert!(
        candidate_projections(&lanes, &sources, &BTreeSet::new(), &budget)
            .unwrap_err()
            .starts_with(LIMIT)
    );
    assert_eq!(lanes, before);
    assert!(
        budget.charge(0).is_err(),
        "storage refusal must remain a failure, never absent ownership"
    );
}

#[test]
fn fourth_review_resolver_preflights_long_domain_before_cloning_or_moving_notes() {
    let (mut head, mut tail) = tied_pair();
    let long = "part".repeat(32768);
    origin_mut(&mut head).source.part_id = Some(long.clone());
    origin_mut(&mut tail).source.part_id = Some(long);
    let mut lanes = pending(vec![vec![head, tail]]);
    let before = lanes.clone();
    let mut report = reports(&lanes);
    let mut budget = Budget::default();
    budget.reserved_bytes.set(128 * 1024 * 1024 - 1024);
    let result = resolve_bounded(
        &mut lanes,
        &[],
        &mut report,
        &PerformanceIndex::default(),
        &mut budget,
    );
    assert!(result.unwrap_err().starts_with(LIMIT));
    assert_eq!(
        lanes, before,
        "refusal occurs before planner moves the original atoms"
    );
}

#[test]
fn fourth_review_discovery_and_resolution_share_cumulative_work() {
    let (head, tail) = tied_pair();
    let original = pending(vec![vec![head, tail]]);
    let measured = Budget::default();
    candidate_projections(&original, &[], &BTreeSet::new(), &measured).unwrap();
    let discovery = measured.work.get();
    let mut alone = Budget::default();
    let mut copy = original.clone();
    resolve_bounded(
        &mut copy,
        &[],
        &mut reports(&original),
        &PerformanceIndex::default(),
        &mut alone,
    )
    .unwrap();
    let resolution = alone.work.get();
    assert!(discovery > 0 && resolution > 0);
    let mut shared = Budget::default();
    shared.work.set(MAX_WORK - discovery - resolution + 1);
    candidate_projections(&original, &[], &BTreeSet::new(), &shared).unwrap();
    let mut copy = original;
    let mut report = reports(&copy);
    assert!(resolve_bounded(
        &mut copy,
        &[],
        &mut report,
        &PerformanceIndex::default(),
        &mut shared
    )
    .unwrap_err()
    .starts_with(LIMIT));
}
