//! Combined review C01/C04/C06 through real parsers, conversion and ledgers.
use std::collections::BTreeSet;
use std::path::Path;
use verse_lib::bundle::{
    build_preservation_ledger, BundleLayout, PreservationLedger, PrimaryDisposition,
};
use verse_lib::engine::convert::{convert_midi_with_target, ConvertOutcome};
use verse_lib::engine::midi::{self, Kind};
use verse_lib::engine::musicxml;
use verse_lib::engine::performance::{Dimension, PerformanceReference, TransferStatus};
use verse_lib::engine::target::{self, ustx, ExportTarget};
use verse_lib::stems::StemPlan;

fn xml(body: &str) -> String {
    format!("<score-partwise version=\"4.0\"><part-list><score-part id=\"P1\"><part-name>Voice</part-name></score-part></part-list><part id=\"P1\"><measure><attributes><divisions>480</divisions></attributes>{body}</measure></part></score-partwise>")
}

fn note(duration: u32) -> String {
    format!("<note><pitch><step>C</step><octave>4</octave></pitch><duration>{duration}</duration><lyric><text>la</text></lyric></note>")
}

fn dynamic(mark: &str, offset: u32) -> String {
    format!("<direction><direction-type><dynamics><{mark}/></dynamics></direction-type><offset sound=\"yes\">{offset}</offset></direction>")
}

fn wedge(kind: &str, offset: u32) -> String {
    format!("<direction><direction-type><wedge type=\"{kind}\" number=\"1\"/></direction-type><offset sound=\"yes\">{offset}</offset></direction>")
}

fn convert(source: &midi::Midi, export: ExportTarget) -> ConvertOutcome {
    let before = source.tracks.clone();
    let outcome = convert_midi_with_target(source, "english", None, export);
    assert!(outcome.ok, "{:?}", outcome.msg);
    assert_eq!(
        source.tracks, before,
        "conversion preserves source payloads"
    );
    outcome
}

// Pinned UCurve interpolation uses integer knots and ties-to-even sampling.
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

fn dyn_curve(project: &ustx::UstxProject) -> &ustx::UstxCurve {
    let curves: Vec<_> = project.voice_parts[0]
        .curves
        .iter()
        .filter(|curve| curve.abbr == "dyn")
        .collect();
    assert_eq!(curves.len(), 1, "one composed DYN curve");
    curves[0]
}

#[test]
fn smooth_ramp_note_fragments_keep_numeric_values_for_all_five_phases() {
    // Both ends of a ramp can be subdivided into two-tick note fragments.
    for boundary in [10, 26] {
        let body = format!(
            "{}{}{}{}{}{}",
            dynamic("p", 0),
            wedge("crescendo", 8),
            wedge("stop", 28),
            dynamic("f", 28),
            note(boundary),
            note(48 - boundary),
        );
        let source = musicxml::parse(xml(&body).as_bytes()).unwrap();
        let outcome = convert(&source, ExportTarget::Ustx);
        let projected = outcome.svp.as_ref().unwrap();
        let model = ustx::serialize(projected).unwrap();
        assert_eq!(
            model.voice_parts[0]
                .notes
                .iter()
                .map(|n| (n.position, n.duration, n.tone))
                .collect::<Vec<_>>(),
            vec![
                (0, boundary as i32, 60),
                (boundary as i32, (48 - boundary) as i32, 60)
            ],
        );
        let curve = dyn_curve(&model);
        assert_eq!(sample(curve, 9), -72, "p to f at tick 9 is -7.1625 dB");
        for phase in 0..5 {
            for tick in (phase..=48).step_by(5) {
                let expected = -77.5 + 117.5 * (f64::from(tick - 8) / 20.0).clamp(0.0, 1.0);
                assert!(
                    (f64::from(sample(curve, tick)) - expected).abs() <= 1.0,
                    "boundary {boundary}, phase {phase}, tick {tick}",
                );
            }
        }
        let (start, end, note_index) = if boundary == 10 {
            (8, 10, 0)
        } else {
            (26, 28, 1)
        };
        let fragment = outcome
            .projection
            .performance_spans
            .iter()
            .find(|span| span.start_tick == start && span.end_tick == end)
            .unwrap();
        assert_eq!(fragment.status, TransferStatus::Mapped);
        assert_eq!(
            fragment.note_ids,
            [projected.tracks[0].notes[note_index]
                .source_evidence
                .as_ref()
                .unwrap()
                .note_id
                .clone()]
        );
        assert!(outcome
            .projection
            .performance_spans
            .iter()
            .all(|span| !span.detail.contains("shorter")));
    }
}

#[test]
fn authored_short_ramp_and_short_held_pulse_still_have_sampling_limits() {
    let cases = [
        (
            format!(
                "{}{}{}{}{}{}",
                dynamic("p", 0),
                wedge("crescendo", 8),
                wedge("stop", 10),
                dynamic("f", 10),
                note(10),
                note(20)
            ),
            8,
            10,
        ),
        (
            format!(
                "{}{}{}{}",
                dynamic("p", 0),
                dynamic("f", 4),
                note(10),
                note(20)
            ),
            0,
            4,
        ),
    ];
    for (body, start, end) in cases {
        let source = musicxml::parse(xml(&body).as_bytes()).unwrap();
        let outcome = convert(&source, ExportTarget::Ustx);
        let limited = outcome
            .projection
            .performance_spans
            .iter()
            .find(|span| {
                span.start_tick == start && span.end_tick == end && span.detail.contains("shorter")
            })
            .expect("authored short span retains its limitation");
        assert_eq!(limited.status, TransferStatus::RepresentationLimit);
        let model = ustx::serialize(outcome.svp.as_ref().unwrap()).unwrap();
        assert_eq!(sample(dyn_curve(&model), end as i32), 40);
        assert_eq!(sample(dyn_curve(&model), end as i32 - 1), 0);
    }
}

fn ledger(
    source: &midi::Midi,
    outcome: &ConvertOutcome,
    export: ExportTarget,
    filename: &str,
) -> PreservationLedger {
    let layout = BundleLayout::new(
        Path::new("/tmp/PerformanceReview.versebundle"),
        filename,
        export,
    )
    .unwrap();
    let stems = StemPlan::from_source(source, &outcome.tracks).unwrap();
    let ledger = build_preservation_ledger(source, &outcome.projection, &layout, &stems);
    let allowed: BTreeSet<_> = std::iter::once(layout.source_relative_path.clone())
        .chain(std::iter::once(layout.project_relative_path.clone()))
        .chain(
            stems
                .stems
                .iter()
                .map(|stem| layout.stem_audio_relative_path(stem)),
        )
        .collect();
    let decoded: PreservationLedger =
        serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
    decoded.validate(&allowed).unwrap();
    decoded
}

fn ledger_refs<'a>(ledger: &'a PreservationLedger, id: &str) -> Vec<&'a PerformanceReference> {
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.source_id == id)
        .unwrap();
    assert!(!matches!(
        entry.disposition,
        PrimaryDisposition::ProjectedExact | PrimaryDisposition::ProjectedMapped { .. }
    ));
    let refs: Vec<_> = entry
        .performance_refs
        .iter()
        .map(|index| &ledger.performance_spans[*index])
        .collect();
    assert!(
        !refs.is_empty(),
        "specific source diagnostic must reach the ledger"
    );
    refs
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

fn sung(start: u32, channel: u8) -> Events {
    vec![
        (start, vec![0xff, 5, 2, b'l', b'a']),
        (start, vec![0x90 | channel, 60, 80]),
        (start + 240, vec![0x80 | channel, 60, 12]),
    ]
}

fn diagnostic_id(source: &midi::Midi, tick: u32) -> (String, String) {
    source
        .tracks
        .iter()
        .find_map(|track| {
            track
                .events
                .iter()
                .find(|event| {
                    event.tick == tick
                        && matches!(
                            event.kind,
                            Kind::ChannelPressure { .. }
                                | Kind::PolyPressure { .. }
                                | Kind::ControlChange { controller: 1, .. }
                        )
                })
                .map(|event| {
                    (
                        format!("event:{}:{}", track.id, event.order),
                        track.id.clone(),
                    )
                })
        })
        .unwrap()
}

fn assert_source_only(span: &PerformanceReference, owner: &str, tick: u32, reason: &str) {
    assert_eq!(span.source_track_id, owner);
    assert_eq!(
        span.target_track, None,
        "a rest diagnostic has no destination owner"
    );
    assert!(span.note_ids.is_empty());
    assert_eq!((span.start_tick, span.end_tick), (tick, tick + 1));
    assert_eq!(span.dimension, Dimension::Other);
    assert_eq!(span.status, TransferStatus::Unsupported);
    assert_eq!(span.detail, reason);
}

const PRESSURE: &str = "MIDI pressure has no verified editable performance mapping.";
const CONTROLLER: &str = "This MIDI controller has no editable performance mapping; CC1 is not a fully specified vibrato.";

#[test]
fn pressure_and_controller_rest_diagnostics_reach_both_conversion_ledgers() {
    let mut events = sung(0, 0);
    events.extend(sung(480, 0));
    events.extend([
        (120, vec![0xd0, 40]),
        (240, vec![0xd0, 50]), // Exact end of the preceding half-open note.
        (300, vec![0xa0, 60, 60]),
        (360, vec![0xb0, 1, 70]),
        (480, vec![0xd0, 80]), // The next attack does own this diagnostic.
        (720, vec![0xd0, 90]),
    ]);
    let source = midi::parse(&smf(vec![events])).unwrap();
    for export in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, export);
        let ledger = ledger(&source, &outcome, export, "source.mid");
        for (tick, reason) in [
            (240, PRESSURE),
            (300, PRESSURE),
            (360, CONTROLLER),
            (720, PRESSURE),
        ] {
            let (id, owner) = diagnostic_id(&source, tick);
            let refs = &outcome.projection.performance_refs[&id];
            assert_eq!(
                refs.len(),
                1,
                "specific reason replaces the generic fallback"
            );
            assert_source_only(
                &outcome.projection.performance_spans[refs[0]],
                &owner,
                tick,
                reason,
            );
            let retained = ledger_refs(&ledger, &id);
            assert_eq!(retained.len(), 1);
            assert_source_only(retained[0], &owner, tick, reason);
        }
        for tick in [120, 480] {
            let (id, _) = diagnostic_id(&source, tick);
            let refs = &outcome.projection.performance_refs[&id];
            assert!(refs.iter().all(|index| {
                let span = &outcome.projection.performance_spans[*index];
                span.detail == PRESSURE && span.target_track == Some(0) && !span.note_ids.is_empty()
            }));
        }
    }
}

#[test]
fn rest_diagnostic_owner_is_the_original_controller_track_even_with_shared_channels() {
    let port = (0, vec![0xff, 0x21, 1, 7]);
    let mut first = vec![port.clone()];
    first.extend(sung(0, 3));
    let mut second = vec![port.clone()];
    second.extend(sung(480, 3));
    let source = midi::parse(&smf(vec![first, second, vec![port, (300, vec![0xd3, 60])]])).unwrap();
    let (id, owner) = diagnostic_id(&source, 300);
    for export in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, export);
        assert!(outcome
            .svp
            .as_ref()
            .unwrap()
            .tracks
            .iter()
            .all(|track| track.source_track_id != owner));
        let refs = &outcome.projection.performance_refs[&id];
        assert_eq!(refs.len(), 1);
        assert_source_only(
            &outcome.projection.performance_spans[refs[0]],
            &owner,
            300,
            PRESSURE,
        );
        let ledger = ledger(&source, &outcome, export, "source.mid");
        assert_source_only(ledger_refs(&ledger, &id)[0], &owner, 300, PRESSURE);
    }
}

#[test]
fn unmatched_musicxml_wedge_stop_at_final_endpoint_has_no_preceding_note_owner() {
    let source =
        musicxml::parse(xml(&format!("{}{}", note(480), wedge("stop", 0))).as_bytes()).unwrap();
    let issue = &source.score_intensity.as_ref().unwrap().issues[0];
    assert!(issue.message.to_lowercase().contains("wedge"));
    assert_eq!(issue.start.as_f64(), 1.0);
    let id = &issue.provenance.evidence[0].source_ids[0];
    for export in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, export);
        let project = outcome.svp.as_ref().unwrap();
        let expected_owner = &project.tracks[0].notes[0]
            .source_evidence
            .as_ref()
            .unwrap()
            .origin
            .as_ref()
            .unwrap()
            .track_id;
        let reports = target::performance_report(export, project).unwrap();
        let report = reports
            .iter()
            .find(|report| report.source_ids.contains(id))
            .unwrap();
        assert_eq!(report.track_id, *expected_owner);
        assert_eq!((report.start_tick, report.end_tick), (480, 480));
        assert!(report.note_ids.is_empty());
        assert_eq!(report.intensity.as_ref().unwrap()["terminalEndpoint"], true);
        let ledger = ledger(&source, &outcome, export, "score.musicxml");
        for span in ledger_refs(&ledger, id) {
            assert_eq!(span.source_track_id, *expected_owner);
            assert_eq!((span.start_tick, span.end_tick), (480, 480));
            assert!(span.note_ids.is_empty());
            assert_eq!(span.intensity.as_ref().unwrap()["terminalEndpoint"], true);
        }
    }
}

#[test]
fn unmatched_wedge_at_next_attack_keeps_only_that_notes_original_identity() {
    let body = format!("{}{}{}", note(480), wedge("stop", 0), note(480));
    let source = musicxml::parse(xml(&body).as_bytes()).unwrap();
    let issue = &source.score_intensity.as_ref().unwrap().issues[0];
    let id = &issue.provenance.evidence[0].source_ids[0];
    for export in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, export);
        let project = outcome.svp.as_ref().unwrap();
        let second = project.tracks[0].notes[1].source_evidence.as_ref().unwrap();
        let reports = target::performance_report(export, project).unwrap();
        let report = reports
            .iter()
            .find(|report| report.source_ids.contains(id))
            .unwrap();
        assert_eq!(report.note_ids, std::slice::from_ref(&second.note_id));
        assert_eq!(report.track_id, second.origin.as_ref().unwrap().track_id);
        assert_eq!((report.start_tick, report.end_tick), (480, 480));
        assert_eq!(
            report.intensity.as_ref().unwrap()["terminalEndpoint"],
            false
        );
    }
}
