//! R2-06: performed source-only score diagnostics survive both saved ledgers.
use std::collections::BTreeSet;
use std::path::Path;
use verse_lib::bundle::{build_preservation_ledger, BundleLayout, PreservationLedger};
use verse_lib::engine::convert::{convert_midi_with_target, ConvertOutcome};
use verse_lib::engine::midi::Midi;
use verse_lib::engine::performance::{PerformanceReference, TransferStatus};
use verse_lib::engine::score_intensity::{Fraction, POLICY};
use verse_lib::engine::{musicxml, target::ExportTarget};
use verse_lib::stems::StemPlan;

fn note(duration: u32) -> String {
    format!("<note><pitch><step>C</step><octave>4</octave></pitch><duration>{duration}</duration><voice>1</voice><lyric><text>la</text></lyric></note>")
}
fn source(repeat: bool) -> Midi {
    source_with_stop(repeat, 720)
}
fn source_with_stop(repeat: bool, stop: u32) -> Midi {
    let repeat_start = if repeat {
        "<barline location=\"left\"><repeat direction=\"forward\"/></barline>"
    } else {
        ""
    };
    let repeat_end = if repeat {
        "<barline location=\"right\"><repeat direction=\"backward\"/></barline>"
    } else {
        ""
    };
    // The diagnostic belongs to P2, while P1 appears first in source order.
    let xml = format!(
        r#"<score-partwise version="4.0"><part-list>
        <score-part id="P1"><part-name>First</part-name></score-part>
        <score-part id="P2"><part-name>Second</part-name></score-part></part-list>
        <part id="P1"><measure><attributes><divisions>480</divisions></attributes>{}</measure></part>
        <part id="P2"><measure><attributes><divisions>480</divisions></attributes>{repeat_start}
        <direction><direction-type><wedge type="stop" number="3"/></direction-type><offset sound="yes">{stop}</offset><voice>1</voice></direction>
        {}<note><rest/><duration>480</duration><voice>1</voice></note>{}{repeat_end}</measure></part></score-partwise>"#,
        note(1440),
        note(480),
        note(480)
    );
    musicxml::parse(xml.as_bytes()).unwrap()
}
fn convert(source: &Midi, target: ExportTarget) -> ConvertOutcome {
    let before = source.tracks.clone();
    let result = convert_midi_with_target(source, "english", None, target);
    assert!(result.ok, "{:?}", result.msg);
    assert_eq!(source.tracks, before);
    result
}
fn saved(source: &Midi, outcome: &ConvertOutcome, target: ExportTarget) -> PreservationLedger {
    let layout = BundleLayout::new(
        Path::new("/tmp/PerformanceSecondReview.versebundle"),
        "source.musicxml",
        target,
    )
    .unwrap();
    let stems = StemPlan::from_source(source, &outcome.tracks).unwrap();
    let ledger = build_preservation_ledger(source, &outcome.projection, &layout, &stems);
    let allowed: BTreeSet<_> = [
        layout.source_relative_path.clone(),
        layout.project_relative_path.clone(),
    ]
    .into_iter()
    .chain(
        stems
            .stems
            .iter()
            .map(|s| layout.stem_audio_relative_path(s)),
    )
    .collect();
    let decoded: PreservationLedger =
        serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
    decoded.validate(&allowed).unwrap();
    decoded
}
fn check(span: &PerformanceReference, owner: &str, start: u32, end: u32, reason: &str) {
    assert_eq!(span.source_track_id, owner);
    assert_eq!(span.target_track, None);
    assert!(span.note_ids.is_empty());
    assert_eq!((span.start_tick, span.end_tick), (start, end));
    assert_eq!(span.detail, reason);
    assert_eq!(span.status, TransferStatus::Unsupported);
    let evidence = span.intensity.as_ref().unwrap();
    assert_eq!(evidence["policy"], POLICY);
    assert_eq!(evidence["provenance"]["policy"], POLICY);
}
#[test]
fn repeated_rest_wedge_stop_preserves_720_and_2160_with_original_owner_and_provenance() {
    let source = source(true);
    let issue = &source.score_intensity.as_ref().unwrap().issues[0];
    let id = &issue.provenance.evidence[0].source_ids[0];
    let owner = &source
        .tracks
        .iter()
        .find(|t| t.source.part_id.as_deref() == Some("P2"))
        .unwrap()
        .id;
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, target);
        let refs = &outcome.projection.performance_refs[id];
        assert_eq!(refs.len(), 2, "no generic fallback and no collapsed repeat");
        let ledger = saved(&source, &outcome, target);
        let entry = ledger.entries.iter().find(|e| &e.source_id == id).unwrap();
        assert_eq!(entry.performance_refs.len(), 2);
        for ((index, saved_index), (tick, pass)) in refs
            .iter()
            .zip(&entry.performance_refs)
            .zip([(720, 1), (2160, 2)])
        {
            let span = &outcome.projection.performance_spans[*index];
            check(span, owner, tick, tick, &issue.message);
            assert_eq!(span, &ledger.performance_spans[*saved_index]);
            let evidence = span.intensity.as_ref().unwrap();
            assert_eq!(evidence["provenance"]["repeat_pass"], pass);
            assert_eq!(
                evidence["provenance"]["scope"],
                serde_json::to_value(&issue.provenance.scope).unwrap()
            );
            assert_eq!(
                evidence["provenance"]["evidence"],
                serde_json::to_value(&issue.provenance.evidence).unwrap()
            );
            assert_eq!(
                evidence["provenance"]["interpretations"],
                serde_json::to_value(&issue.provenance.interpretations).unwrap()
            );
            assert_eq!(evidence["terminalEndpoint"], false);
            assert_eq!(
                evidence["start"],
                serde_json::to_value(Fraction::new(tick.into(), 480).unwrap()).unwrap()
            );
            assert_eq!(evidence["start"], evidence["end"]);
        }
        let occurrences: BTreeSet<_> = refs
            .iter()
            .map(|i| {
                outcome.projection.performance_spans[*i]
                    .intensity
                    .as_ref()
                    .unwrap()["provenance"]["occurrence"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        assert_eq!(occurrences.len(), 2);
    }
}
#[test]
fn terminal_score_issue_has_no_destination_or_preceding_note_in_saved_ledgers() {
    let source = source_with_stop(false, 1440);
    let issue = &source.score_intensity.as_ref().unwrap().issues[0];
    let id = issue.provenance.evidence[0].source_ids[0].clone();
    let reason = issue.message.clone();
    let owner = &source
        .tracks
        .iter()
        .find(|t| t.source.part_id.as_deref() == Some("P2"))
        .unwrap()
        .id;
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, target);
        let refs = &outcome.projection.performance_refs[&id];
        assert_eq!(refs.len(), 1);
        let span = &outcome.projection.performance_spans[refs[0]];
        check(span, owner, 1440, 1440, &reason);
        assert_eq!(span.intensity.as_ref().unwrap()["terminalEndpoint"], true);
        let ledger = saved(&source, &outcome, target);
        assert!(ledger.performance_spans.contains(span));
    }
}
#[test]
fn source_only_interval_subtracts_only_already_reported_exact_note_spans() {
    let mut source = source(false);
    let issue = &mut source.score_intensity.as_mut().unwrap().issues[0];
    // Parser-only diagnostics may cover a positive interval across a rest.
    issue.start = Fraction::new(1, 2).unwrap();
    issue.end = Fraction::new(5, 2).unwrap();
    let id = issue.provenance.evidence[0].source_ids[0].clone();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, target);
        let refs = &outcome.projection.performance_refs[&id];
        let mut spans: Vec<_> = refs
            .iter()
            .map(|i| &outcome.projection.performance_spans[*i])
            .collect();
        spans.sort_by_key(|s| s.start_tick);
        assert_eq!(
            spans
                .iter()
                .map(|s| (s.start_tick, s.end_tick))
                .collect::<Vec<_>>(),
            [(240, 480), (480, 960), (960, 1200)]
        );
        assert!(spans[0].target_track.is_some() && !spans[0].note_ids.is_empty());
        assert!(spans[2].target_track.is_some() && !spans[2].note_ids.is_empty());
        assert_eq!(spans[1].target_track, None);
        assert!(spans[1].note_ids.is_empty());
        let ledger = saved(&source, &outcome, target);
        assert!(spans
            .iter()
            .all(|span| ledger.performance_spans.contains(span)));
    }
}
