//! FID-002 R6/R7: real MIDI payloads carried by routed projected notes.
//! The target seam receives a whole-note move, as the continuity planner does;
//! this does not claim that the pre-EXP003 score adapter imports MIDI expression.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use verse_lib::bundle::{
    build_preservation_ledger, BundleLayout, PreservationLedger, PrimaryDisposition,
};
use verse_lib::engine::convert::{convert_midi_with_target, ConvertOutcome, ProjectionEvidence};
use verse_lib::engine::midi::{self, Kind};
use verse_lib::engine::performance::{
    ChannelPerformance, Dimension, PerformanceIssue, PerformanceReference, PerformanceTransfer,
    TransferStatus,
};
use verse_lib::engine::projection::{ProjectedLyric, ProjectedProject};
use verse_lib::engine::target::{self, ustx, ExportTarget};
use verse_lib::stems::StemPlan;

trait MidiBindingExt {
    fn timeline(&self) -> &Arc<ChannelPerformance>;
    fn timeline_mut(&mut self) -> &mut Arc<ChannelPerformance>;
    fn key_mut(&mut self) -> &mut verse_lib::engine::performance::ChannelKey;
}
impl MidiBindingExt for verse_lib::engine::performance::PerformanceNote {
    fn timeline(&self) -> &Arc<ChannelPerformance> {
        let verse_lib::engine::performance::PerformanceOwner::Midi { timeline, .. } = &self.owner
        else {
            panic!("expected original MIDI owner")
        };
        timeline
    }
    fn timeline_mut(&mut self) -> &mut Arc<ChannelPerformance> {
        let verse_lib::engine::performance::PerformanceOwner::Midi { timeline, .. } =
            &mut self.owner
        else {
            panic!("expected original MIDI owner")
        };
        timeline
    }
    fn key_mut(&mut self) -> &mut verse_lib::engine::performance::ChannelKey {
        let verse_lib::engine::performance::PerformanceOwner::Midi { key, .. } = &mut self.owner
        else {
            panic!("expected original MIDI owner")
        };
        key
    }
}

type Events = Vec<(u32, Vec<u8>)>;
fn vlq(mut value: u32, output: &mut Vec<u8>) {
    let mut bytes = vec![(value & 127) as u8];
    value >>= 7;
    while value > 0 {
        bytes.push((value & 127) as u8 | 128);
        value >>= 7;
    }
    output.extend(bytes.into_iter().rev());
}
fn smf(tracks: Vec<Events>) -> Vec<u8> {
    let mut file = b"MThd\0\0\0\x06\0\x01".to_vec();
    file.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    file.extend_from_slice(&480u16.to_be_bytes());
    for mut events in tracks {
        events.sort_by_key(|(tick, _)| *tick);
        let mut bytes = Vec::new();
        let mut previous = 0;
        for (tick, data) in events {
            vlq(tick - previous, &mut bytes);
            bytes.extend(data);
            previous = tick;
        }
        bytes.extend([0, 0xff, 0x2f, 0]);
        file.extend_from_slice(b"MTrk");
        file.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        file.extend(bytes);
    }
    file
}
fn sung(start: u32, duration: u32) -> Events {
    vec![
        (start, vec![0xff, 5, 4, b's', b'i', b'n', b'g']),
        (start, vec![0x90, 68, 73]),
        (start + duration, vec![0x80, 68, 12]),
    ]
}

struct Fixture {
    midi: midi::Midi,
    conversion: ConvertOutcome,
    unrouted: ProjectedProject,
    project: ProjectedProject,
    timeline: Arc<ChannelPerformance>,
}
fn fixture() -> Fixture {
    fixture_with_expression(None)
}

fn fixture_with_expression(expression: Option<Events>) -> Fixture {
    let mut head = vec![
        (0, vec![0xb0, 101, 0]),
        (0, vec![0xb0, 100, 0]),
        (0, vec![0xb0, 6, 2]),
        (0, vec![0xe0, 0, 64]),
        (0, vec![0xb0, 7, 127]),
        (0, vec![0xb0, 11, 127]),
        (240, vec![0xe0, 0, 96]),
        (240, vec![0xb0, 11, 64]),
        (720, vec![0xe0, 0, 0]),
        (720, vec![0xb0, 11, 0]),
        (960, vec![0xe0, 0, 64]),
        (960, vec![0xb0, 11, 127]),
        // Authored changes during a real rest must still govern the next note.
        (1300, vec![0xe0, 0, 96]),
        (1300, vec![0xb0, 11, 64]),
    ];
    if let Some(expression) = expression {
        head.retain(|(_, data)| !data.starts_with(&[0xb0, 11]));
        head.extend(expression);
    }
    head.extend(sung(0, 240));
    head.extend(sung(1440, 240));
    let midi = midi::parse(&smf(vec![head, sung(240, 960)])).unwrap();
    let conversion = convert_midi_with_target(&midi, "english", None, ExportTarget::Ustx);
    assert!(conversion.ok, "{:?}", conversion.msg);
    let mut unrouted = conversion.svp.as_ref().unwrap().clone();
    assert_eq!(unrouted.tracks.len(), 2);
    for (index, note) in unrouted
        .tracks
        .iter_mut()
        .flat_map(|track| &mut track.notes)
        .enumerate()
    {
        let payload = note
            .performance
            .as_mut()
            .expect("real normalized MIDI performance");
        // Universal note identity must not be taken from this independent ID.
        payload.source_id = format!("independent-performance-binding-{index}");
    }
    let timeline = Arc::clone(
        unrouted.tracks[0].notes[0]
            .performance
            .as_ref()
            .unwrap()
            .timeline(),
    );
    assert!(unrouted
        .tracks
        .iter()
        .flat_map(|track| &track.notes)
        .all(|note| Arc::ptr_eq(note.performance.as_ref().unwrap().timeline(), &timeline)));
    let mut project = unrouted.clone();
    let mut source = project.tracks.remove(1);
    assert_eq!(source.source_track_id, "midi-track-1");
    let mut tail = source.notes.pop().unwrap();
    assert!(source.notes.is_empty());
    tail.lyric = ProjectedLyric::Extension;
    project.tracks[0].notes.push(tail);
    project.tracks[0].notes.sort_by_key(|note| note.onset_ticks);
    Fixture {
        midi,
        conversion,
        unrouted,
        project,
        timeline,
    }
}

fn note_owners(project: &ProjectedProject) -> BTreeMap<String, String> {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.notes)
        .map(|note| {
            let evidence = note.source_evidence.as_ref().unwrap();
            (
                evidence.note_id.clone(),
                evidence.origin.as_ref().unwrap().track_id.clone(),
            )
        })
        .collect()
}
fn assert_attribution(project: &ProjectedProject, reports: &[PerformanceTransfer]) {
    let owners = note_owners(project);
    assert!(!reports.is_empty());
    for report in reports {
        assert_eq!(
            report.target_track,
            Some(0),
            "actual destination, independent of original track"
        );
        assert!(!report.note_ids.is_empty());
        for id in &report.note_ids {
            assert_eq!(
                owners[id], report.track_id,
                "{id}: source owner is not destination or performance binding ID"
            );
        }
        assert!(
            report
                .source_ids
                .iter()
                .all(|id| id.starts_with("event:midi-track-0:")
                    || report.note_ids.iter().any(|note_id| {
                        project.tracks.iter().flat_map(|t| &t.notes).any(|n| {
                            n.source_evidence.as_ref().is_some_and(|e| {
                                e.note_id == *note_id
                                    && *id == format!("expression:{}:velocity", e.note_on_event_id)
                            })
                        })
                    })),
            "controller sources stay on track0; attack evidence belongs to the exact affected note"
        );
    }
    let tail = &project.tracks[0].notes[1];
    let evidence = tail.source_evidence.as_ref().unwrap();
    assert_eq!(evidence.origin.as_ref().unwrap().track_id, "midi-track-1");
    assert_ne!(
        tail.performance.as_ref().unwrap().source_id,
        evidence.note_id
    );
    assert!(reports
        .iter()
        .any(|r| r.track_id == "midi-track-1" && r.note_ids == [evidence.note_id.clone()]));
}
fn curve<'a>(model: &'a ustx::UstxProject, abbr: &str) -> &'a ustx::UstxCurve {
    let matches: Vec<_> = model.voice_parts[0]
        .curves
        .iter()
        .filter(|curve| curve.abbr == abbr)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "one {abbr} curve; attribution must not compose it twice"
    );
    matches[0]
}
fn sample(curve: &ustx::UstxCurve, tick: i32) -> i32 {
    match curve.xs.binary_search(&tick) {
        Ok(index) => curve.ys[index],
        Err(index) if index > 0 && index < curve.xs.len() => {
            let ratio = f64::from(tick - curve.xs[index - 1])
                / f64::from(curve.xs[index] - curve.xs[index - 1]);
            (f64::from(curve.ys[index - 1])
                + ratio * f64::from(curve.ys[index] - curve.ys[index - 1]))
            .round_ties_even() as i32
        }
        _ => 0,
    }
}

/// Exercise the live ledger builder using the public handoff records. This
/// mirrors the caller's mechanical report-to-ledger transfer, taking every
/// source/target/note attribution from the actual target report, never an oracle.
fn ledger(
    fixture: &Fixture,
    target: ExportTarget,
    reports: &[PerformanceTransfer],
) -> (PreservationLedger, BundleLayout, StemPlan) {
    let mut projection = ProjectionEvidence {
        source_ids: fixture.conversion.projection.source_ids.clone(),
        intensity_note_owners: fixture
            .project
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(target_track, track)| {
                track.notes.iter().map(move |note| {
                    let evidence = note.source_evidence.as_ref().unwrap();
                    verse_lib::engine::projection::IntensityNoteProjection {
                        note_id: evidence.note_id.clone(),
                        intensity_attack_note_id: evidence
                            .origin
                            .as_ref()
                            .and_then(|o| o.continuation.as_ref())
                            .filter(|link| {
                                link.kind == verse_lib::engine::projection::ContinuationKind::Tie
                            })
                            .and_then(|link| link.intensity_attack_note_id.clone()),
                        source_track_id: evidence.origin.as_ref().unwrap().track_id.clone(),
                        target_track,
                        destination_track_id: track.source_track_id.clone(),
                        start_tick: note.onset_ticks,
                        end_tick: note.onset_ticks.checked_add(note.duration_ticks).unwrap(),
                    }
                })
            })
            .collect(),
        ..ProjectionEvidence::default()
    };
    for report in reports {
        let index = projection.performance_spans.len();
        projection.performance_spans.push(PerformanceReference {
            intensity: report.intensity.clone(),
            target: target.extension().into(),
            target_track: report.target_track,
            source_track_id: report.track_id.clone(),
            dimension: report.dimension,
            start_tick: report.start_tick,
            end_tick: report.end_tick,
            note_ids: report.note_ids.clone(),
            status: report.status,
            detail: report.message.clone(),
        });
        for id in &report.source_ids {
            projection
                .performance_refs
                .entry(id.clone())
                .or_default()
                .push(index);
            let map = if report.status == TransferStatus::Mapped {
                &mut projection.performance_mapped
            } else {
                &mut projection.performance_unmapped
            };
            map.entry(id.clone())
                .or_insert_with(|| report.message.clone());
        }
    }
    let stems = StemPlan::from_source(&fixture.midi, &fixture.conversion.tracks).unwrap();
    assert_eq!(stems.stems.len(), 2);
    let layout =
        BundleLayout::new(Path::new("/tmp/Routed.versebundle"), "source.mid", target).unwrap();
    let built = build_preservation_ledger(&fixture.midi, &projection, &layout, &stems);
    let bytes = serde_json::to_vec(&built).unwrap();
    let decoded: PreservationLedger = serde_json::from_slice(&bytes).unwrap();
    let allowed: BTreeSet<_> = std::iter::once(layout.source_relative_path.clone())
        .chain(std::iter::once(layout.project_relative_path.clone()))
        .chain(
            stems
                .stems
                .iter()
                .map(|stem| layout.stem_audio_relative_path(stem)),
        )
        .collect();
    decoded.validate(&allowed).unwrap();
    (decoded, layout, stems)
}

#[test]
fn routed_gain_pitch_and_current_ledger_keep_original_ownership() {
    let fixture = fixture();
    let before = fixture.project.clone();
    let original_source = fixture.midi.tracks.clone();
    let mut destination_named = fixture.project.clone();
    for track in &mut destination_named.tracks {
        for note in &mut track.notes {
            note.source_evidence
                .as_mut()
                .unwrap()
                .origin
                .as_mut()
                .unwrap()
                .track_id = track.source_track_id.clone();
        }
    }
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        target::validate_for(target, &fixture.project).unwrap();
        let bytes = target::serialize_to(target, &fixture.project).unwrap();
        let reports = target::performance_report(target, &fixture.project).unwrap();
        assert_attribution(&fixture.project, &reports);
        assert_eq!(
            bytes,
            target::serialize_to(target, &destination_named).unwrap(),
            "original-owner report boundaries cannot alter nominal notes or curves"
        );
        assert_ne!(
            reports,
            target::performance_report(target, &destination_named).unwrap(),
            "original ownership changes reports even when target bytes stay equal"
        );
        if target == ExportTarget::Ustx {
            let model = ustx::serialize(&fixture.project).unwrap();
            assert_eq!(bytes, ustx::to_yaml(&model).as_bytes());
            assert_eq!(model.voice_parts.len(), 1);
            assert_eq!(model.voice_parts[0].curves.len(), 2);
            assert_eq!(
                model.voice_parts[0]
                    .notes
                    .iter()
                    .map(|n| (n.position, n.duration, n.tone))
                    .collect::<Vec<_>>(),
                [(0, 240, 68), (240, 960, 68), (1440, 240, 68)]
            );
            assert_eq!(model.voice_parts[0].notes[1].lyric, "+~");
            for tick in 0..=1680 {
                let (pitch, cc) = match tick {
                    0..240 => (0, 1.0_f64),
                    240..720 => (100, 64.0 / 127.0),
                    720..960 => (-200, 0.0),
                    960..1300 => (0, 1.0),
                    _ => (100, 64.0 / 127.0),
                };
                // Authored NoteOn velocity73 means (73-80)/4 dB on sounding
                // notes only. CC contributes once, including across the rest.
                let attack_db = if (0..1200).contains(&tick) || (1440..=1680).contains(&tick) {
                    -1.75
                } else {
                    0.0
                };
                let gain = if cc == 0.0 {
                    -240
                } else {
                    (10.0 * attack_db + 200.0 * cc.log10()).round() as i32
                };
                assert_eq!(
                    sample(curve(&model, "pitd"), tick),
                    pitch,
                    "pitch tick {tick}"
                );
                assert_eq!(
                    sample(curve(&model, "dyn"), tick),
                    gain,
                    "gain tick {tick}: compose one owned attack and one CC gain"
                );
            }
            assert!(model.voice_parts[0]
                .notes
                .iter()
                .all(|n| !n.pitch.snap_first && n.pitch.data.len() == 1));
            assert!(reports.iter().all(|r| r.status == TransferStatus::Mapped));
        } else {
            let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let notes = saved["tracks"][0]["mainGroup"]["notes"].as_array().unwrap();
            assert_eq!(notes.len(), 3);
            assert_eq!(notes[1]["lyrics"], "-");
            assert_eq!(notes[1]["onset"], 352_800_000u64);
            assert_eq!(notes[1]["duration"], 1_411_200_000u64);
            assert!(reports
                .iter()
                .all(|r| r.status == TransferStatus::Unsupported));
        }
        let (ledger, layout, stems) = ledger(&fixture, target, &reports);
        let owners = note_owners(&fixture.project);
        for span in &ledger.performance_spans {
            assert_eq!(span.target_track, Some(0));
            for id in &span.note_ids {
                assert_eq!(owners[id], span.source_track_id);
            }
        }
        let evidence = fixture.project.tracks[0].notes[1]
            .source_evidence
            .as_ref()
            .unwrap();
        let source_stem = stems
            .stems
            .iter()
            .find(|stem| stem.source_track_ids.contains(&"midi-track-1".to_string()))
            .unwrap();
        let expected: BTreeSet<_> = [
            layout.source_relative_path.clone(),
            layout.project_relative_path.clone(),
            layout.stem_audio_relative_path(source_stem),
        ]
        .into_iter()
        .collect();
        for id in [
            &evidence.note_id,
            &evidence.note_on_event_id,
            &evidence.note_off_event_id,
        ] {
            let entry = ledger
                .entries
                .iter()
                .find(|entry| entry.source_id == *id)
                .unwrap();
            assert_eq!(entry.disposition, PrimaryDisposition::ProjectedExact);
            assert_eq!(
                entry
                    .artifact_paths
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
                expected
            );
        }
        let control = fixture.midi.tracks[0]
            .events
            .iter()
            .find(|e| e.tick == 240 && matches!(e.kind, Kind::ControlChange { controller: 11, .. }))
            .unwrap();
        let id = format!("event:midi-track-0:{}", control.order);
        let entry = ledger
            .entries
            .iter()
            .find(|entry| entry.source_id == id)
            .unwrap();
        assert!(!entry.performance_refs.is_empty());
        for &index in &entry.performance_refs {
            assert_eq!(
                ledger.performance_spans[index].source_track_id,
                "midi-track-1"
            );
            assert_eq!(
                ledger.performance_spans[index].note_ids.as_slice(),
                std::slice::from_ref(&evidence.note_id)
            );
        }
        assert!(
            matches!(
                entry.disposition,
                PrimaryDisposition::ProjectedMapped { .. }
            ) == (target == ExportTarget::Ustx)
        );
    }
    assert_eq!(fixture.project, before);
    assert_eq!(fixture.midi.tracks, original_source);
    assert!(fixture.project.tracks[0].notes.iter().all(|n| Arc::ptr_eq(
        n.performance.as_ref().unwrap().timeline(),
        &fixture.timeline
    )));
}

#[test]
fn same_key_conflicts_refuse_reports_analysis_and_bytes_in_every_region_layout() {
    for change in ["gain", "pitch", "point-attribution", "issue"] {
        for location in ["same-region", "later-region", "other-destination"] {
            let fixture = fixture();
            let mut project = if location == "other-destination" {
                fixture.unrouted.clone()
            } else {
                fixture.project.clone()
            };
            let mut conflicting = fixture.timeline.as_ref().clone();
            match change {
                "gain" => conflicting.linear_gain[1].value = Some(0.25),
                "pitch" => conflicting.pitch_cents[1].value = Some(111.0),
                "point-attribution" => conflicting.pitch_cents[1]
                    .source_ids
                    .push("contradictory-source-event".into()),
                "issue" => conflicting.issues.push(PerformanceIssue {
                    tick: 300,
                    dimension: Dimension::Other,
                    source_ids: vec!["contradictory-source-event".into()],
                    reason: "different original protocol evidence".into(),
                }),
                _ => unreachable!(),
            }
            let note = match location {
                "other-destination" => &mut project.tracks[1].notes[0],
                "later-region" => {
                    project.tracks[0].notes[1]
                        .performance
                        .as_mut()
                        .unwrap()
                        .key_mut()
                        .channel = 1;
                    &mut project.tracks[0].notes[2]
                }
                _ => &mut project.tracks[0].notes[1],
            };
            *note.performance.as_mut().unwrap().timeline_mut() = Arc::new(conflicting);
            let before = project.clone();
            for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                let error = target::performance_report(target, &project).unwrap_err();
                assert!(
                    error.starts_with("MIDI_PERFORMANCE_OWNER_CONFLICT:"),
                    "{location}/{change}/{target:?}: {error}"
                );
                assert_eq!(target::validate_for(target, &project).unwrap_err(), error);
                assert_eq!(
                    target::serialize_to(target, &project)
                        .unwrap_err()
                        .message(),
                    error
                );
            }
            assert_eq!(
                project, before,
                "refusal never rebinds a contradictory timeline"
            );
        }
    }
}

#[test]
fn equal_distinct_arcs_are_compatible_without_replacing_the_payload() {
    let fixture = fixture();
    let mut equal = fixture.project.clone();
    let owned_copy = Arc::new(fixture.timeline.as_ref().clone());
    *equal.tracks[0].notes[1]
        .performance
        .as_mut()
        .unwrap()
        .timeline_mut() = Arc::clone(&owned_copy);
    assert!(!Arc::ptr_eq(&owned_copy, &fixture.timeline));
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        assert_eq!(
            target::serialize_to(target, &equal).unwrap(),
            target::serialize_to(target, &fixture.project).unwrap()
        );
        assert_eq!(
            target::performance_report(target, &equal).unwrap(),
            target::performance_report(target, &fixture.project).unwrap()
        );
    }
    assert!(Arc::ptr_eq(
        equal.tracks[0].notes[1]
            .performance
            .as_ref()
            .unwrap()
            .timeline(),
        &owned_copy
    ));
}

#[test]
fn routed_representation_limits_and_issues_name_only_the_original_affected_notes() {
    // Both the short pulse and unsupported controller are authored in the SMF.
    // Persisted evidence must name the actual causal event, never a future CC.
    let fixture = fixture_with_expression(Some(vec![
        (0, vec![0xb0, 11, 127]),
        (300, vec![0xb0, 1, 64]),
        (718, vec![0xb0, 11, 64]),
        (720, vec![0xb0, 11, 127]),
    ]));
    let before = fixture.project.clone();
    let reports = target::performance_report(ExportTarget::Ustx, &fixture.project).unwrap();
    assert_attribution(&fixture.project, &reports);
    let tail_id = fixture.project.tracks[0].notes[1]
        .source_evidence
        .as_ref()
        .unwrap()
        .note_id
        .clone();
    let issue = reports
        .iter()
        .find(|r| r.dimension == Dimension::Other)
        .unwrap();
    assert_eq!(issue.track_id, "midi-track-1");
    assert_eq!(issue.note_ids.as_slice(), std::slice::from_ref(&tail_id));
    let limited = reports
        .iter()
        .find(|r| {
            r.dimension == Dimension::LinearGain
                && r.track_id == "midi-track-1"
                && r.status == TransferStatus::RepresentationLimit
        })
        .unwrap();
    assert_eq!(limited.note_ids, [tail_id]);
    assert!(limited.message.contains("5-tick"));
    let bytes = target::serialize_to(ExportTarget::Ustx, &fixture.project).unwrap();
    let model = ustx::serialize(&fixture.project).unwrap();
    assert_eq!(bytes, ustx::to_yaml(&model).as_bytes());
    assert_eq!(model.voice_parts[0].curves.len(), 2);
    // EXP003 localizes the 2-tick limitation. Valid surrounding owned attacks
    // remain editable, while the pulse is explicitly neutral and reported.
    assert_eq!(
        [717, 718, 719, 720].map(|t| sample(curve(&model, "dyn"), t)),
        [-18, 0, 0, -18]
    );
    assert!(reports
        .iter()
        .any(|r| r.dimension == Dimension::LinearGain && r.status == TransferStatus::Mapped));
    assert_eq!(curve(&model, "pitd").abbr, "pitd");
    let (ledger, _, _) = ledger(&fixture, ExportTarget::Ustx, &reports);
    assert!(ledger
        .performance_spans
        .iter()
        .any(|span| span.source_track_id == "midi-track-1"
            && span.status == TransferStatus::RepresentationLimit));
    assert_eq!(fixture.project, before);
}

#[test]
fn absent_payload_does_not_inherit_routed_channel_curves_across_a_rest() {
    let mut fixture = fixture();
    fixture.project.tracks[0].notes[2].performance = None;
    let third_id = fixture.project.tracks[0].notes[2]
        .source_evidence
        .as_ref()
        .unwrap()
        .note_id
        .clone();
    let reports = target::performance_report(ExportTarget::Ustx, &fixture.project).unwrap();
    assert!(reports.iter().all(|r| !r.note_ids.contains(&third_id)));
    let model = ustx::serialize(&fixture.project).unwrap();
    assert!(model.voice_parts[0].notes[2].pitch.snap_first);
    assert_eq!(sample(curve(&model, "dyn"), 1440), 0);
    assert_eq!(sample(curve(&model, "pitd"), 1440), 0);
    assert!(fixture.project.tracks[0].notes[2].performance.is_none());
}
