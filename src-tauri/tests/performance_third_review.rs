//! R3-05/08/09: actual source, target and saved-ledger ownership regressions.
use std::collections::BTreeSet;
use std::path::Path;
use verse_lib::bundle::{build_preservation_ledger, BundleLayout, PreservationLedger};
use verse_lib::engine::convert::{convert_midi_with_target, ConvertOutcome};
use verse_lib::engine::midi::Midi;
use verse_lib::engine::performance::TransferStatus;
use verse_lib::engine::score_intensity::Fraction;
use verse_lib::engine::{
    musicxml,
    target::{self, ExportTarget},
};
use verse_lib::stems::StemPlan;

fn note(duration: u32, voice: u32, chord: bool) -> String {
    let step = if chord { "E" } else { "C" };
    let chord = if chord { "<chord/>" } else { "" };
    format!("<note>{chord}<pitch><step>{step}</step><octave>4</octave></pitch><duration>{duration}</duration><voice>{voice}</voice><staff>1</staff><lyric><text>la</text></lyric></note>")
}
fn convert(source: &Midi, target: ExportTarget) -> ConvertOutcome {
    let before = source.tracks.clone();
    let outcome = convert_midi_with_target(source, "english", None, target);
    assert!(outcome.ok, "{:?}", outcome.msg);
    assert_eq!(source.tracks, before);
    target::serialize_to(target, outcome.svp.as_ref().unwrap()).unwrap();
    outcome
}
fn saved(source: &Midi, outcome: &ConvertOutcome, target: ExportTarget) -> PreservationLedger {
    let layout = BundleLayout::new(
        Path::new("/tmp/PerformanceThirdReview.versebundle"),
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

#[test]
fn musicxml_exact_half_tick_boundaries_agree_in_direct_exports_and_saved_ledgers() {
    for (offset, rounded) in [
        ("0.499999999999999999", 0),
        ("0.5", 1),
        ("0.500000000000000001", 1),
    ] {
        let xml = format!(
            r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes><direction><direction-type><dynamics><p/></dynamics></direction-type><offset sound="yes">{offset}</offset></direction>{}</measure></part></score-partwise>"#,
            note(960, 1, false)
        );
        let source = musicxml::parse(xml.as_bytes()).unwrap();
        assert_eq!(source.ticks_per_beat, 480);
        let exact = Fraction::decimal(offset)
            .unwrap()
            .checked_mul(Fraction::new(1, 480).unwrap())
            .unwrap();
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let outcome = convert(&source, target);
            let project = outcome.svp.as_ref().unwrap();
            assert_eq!(project.tracks[0].notes.len(), 1);
            let note = &project.tracks[0].notes[0];
            assert_eq!(
                (note.onset_ticks, note.duration_ticks, note.pitch),
                (0, 960, 60)
            );
            let ledger = saved(&source, &outcome, target);
            assert_eq!(
                ledger.performance_spans,
                outcome.projection.performance_spans
            );
            if target == ExportTarget::Ustx {
                let spans: Vec<_> = ledger
                    .performance_spans
                    .iter()
                    .filter(|span| span.status == TransferStatus::Mapped)
                    .collect();
                assert_eq!(spans.len(), 1, "{offset}");
                let span = spans[0];
                let evidence = span.intensity.as_ref().unwrap();
                assert_eq!(evidence["start"], serde_json::to_value(exact).unwrap());
                assert_eq!(evidence["targetStart"], rounded);
                assert_eq!(evidence["targetEnd"], 960);
                assert_eq!((span.start_tick, span.end_tick), (0, 960));
                let direct = target::ustx::performance_report(project).unwrap();
                let direct = direct
                    .iter()
                    .find(|span| span.status == TransferStatus::Mapped)
                    .unwrap();
                assert_eq!(direct.intensity.as_ref(), Some(evidence));
                let model = target::ustx::serialize(project).unwrap();
                assert!(model.voice_parts[0]
                    .curves
                    .iter()
                    .any(|curve| curve.abbr == "dyn" && curve.ys.contains(&-78)));
            }
        }
    }
}

#[test]
fn sibling_lane_issue_coverage_keeps_only_the_other_voices_uncovered_interval() {
    // Voice 1's main member ends at 480 while its sibling sustains to 960.
    // Voice 2 ends at 240. The same part-wide invalid level starts at 240;
    // sibling coverage must not duplicate voice 1 or erase voice 2's issue.
    let xml = format!(
        r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voices</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes>
        <direction><direction-type><dynamics><p/></dynamics></direction-type><sound dynamics="1000"/><offset sound="yes">240</offset></direction>
        {}{}<backup><duration>480</duration></backup>{}<forward><duration>720</duration></forward>
        </measure></part></score-partwise>"#,
        note(480, 1, false),
        note(960, 1, true),
        note(240, 2, false)
    );
    let source = musicxml::parse(xml.as_bytes()).unwrap();
    let voice1: Vec<_> = source
        .tracks
        .iter()
        .filter(|t| t.source.voice.as_deref() == Some("1"))
        .collect();
    assert_eq!(voice1.len(), 2);
    let voice2 = source
        .tracks
        .iter()
        .find(|t| t.source.voice.as_deref() == Some("2"))
        .unwrap();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, target);
        let ledger = saved(&source, &outcome, target);
        let issues: Vec<_> = ledger
            .performance_spans
            .iter()
            .filter(|s| {
                s.intensity
                    .as_ref()
                    .is_some_and(|i| i["provenance"].is_object() && i.get("curve").is_none())
            })
            .collect();
        assert!(issues
            .iter()
            .any(|s| s.target_track.is_some() && s.start_tick == 240 && s.end_tick == 960));
        let uncovered: Vec<_> = issues.iter().filter(|s| s.target_track.is_none()).collect();
        assert_eq!(uncovered.len(), 1, "{issues:?}");
        assert_eq!(uncovered[0].source_track_id, voice2.id);
        assert_eq!((uncovered[0].start_tick, uncovered[0].end_tick), (240, 960));
        assert!(uncovered[0].note_ids.is_empty());
        assert!(!uncovered
            .iter()
            .any(|s| voice1.iter().any(|t| t.id == s.source_track_id)));
    }
}

#[test]
fn note_free_and_repeated_rest_only_parts_keep_declared_owner_positions_and_passes() {
    for rest_only in [false, true] {
        for voice in [None, Some(2)] {
            let qualifier = voice.map_or(String::new(), |v| format!("<voice>{v}</voice>"));
            let (open, silence, close) = if rest_only {
                (
                    "<barline location=\"left\"><repeat direction=\"forward\"/></barline>",
                    format!(
                        "<note><rest/><duration>960</duration>{qualifier}<staff>1</staff></note>"
                    ),
                    "<barline location=\"right\"><repeat direction=\"backward\"/></barline>",
                )
            } else {
                ("", "<forward><duration>960</duration></forward>".into(), "")
            };
            let xml = format!(
                r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Singing</part-name></score-part><score-part id="P2"><part-name>Silent</part-name></score-part></part-list>
                <part id="P1"><measure><attributes><divisions>480</divisions></attributes>{}</measure></part>
                <part id="P2"><measure><attributes><divisions>480</divisions></attributes>{open}<direction><direction-type><wedge type="stop" number="7"/></direction-type><offset sound="yes">720</offset>{qualifier}<staff>1</staff></direction>{silence}{close}</measure></part></score-partwise>"#,
                note(960, 1, false)
            );
            let source = musicxml::parse(xml.as_bytes()).unwrap();
            let issue = &source.score_intensity.as_ref().unwrap().issues[0];
            let id = &issue.provenance.evidence[0].source_ids[0];
            let owner = source
                .tracks
                .iter()
                .find(|t| {
                    t.source.part_id.as_deref() == Some("P2")
                        && t.source.staff_id.as_deref() == Some("1")
                })
                .expect("parser retains an original staff lane without inventing a vocal note");
            for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                let outcome = convert(&source, target);
                assert_eq!(outcome.svp.as_ref().unwrap().tracks.len(), 1);
                let ledger = saved(&source, &outcome, target);
                let entry = ledger
                    .entries
                    .iter()
                    .find(|entry| &entry.source_id == id)
                    .unwrap();
                assert_eq!(entry.performance_refs.len(), if rest_only { 2 } else { 1 });
                for (ordinal, index) in entry.performance_refs.iter().enumerate() {
                    let span = &ledger.performance_spans[*index];
                    assert_eq!(span.source_track_id, owner.id);
                    assert_eq!(span.target_track, None);
                    assert!(span.note_ids.is_empty());
                    assert_eq!(
                        (span.start_tick, span.end_tick),
                        (720 + ordinal as u32 * 960, 720 + ordinal as u32 * 960)
                    );
                    let evidence = span.intensity.as_ref().unwrap();
                    assert_eq!(evidence["provenance"]["repeat_pass"], ordinal as u32 + 1);
                    assert_eq!(
                        evidence["provenance"]["scope"],
                        serde_json::to_value(&issue.provenance.scope).unwrap()
                    );
                    assert_eq!(
                        evidence["start"],
                        serde_json::to_value(
                            Fraction::new(i64::from(span.start_tick), 480).unwrap()
                        )
                        .unwrap()
                    );
                    assert_eq!(evidence["start"], evidence["end"]);
                }
            }
        }
    }
}

#[test]
fn zero_duration_declaration_keeps_written_coordinate_and_explicit_unresolved_scope() {
    use verse_lib::engine::score_intensity::{source::DeclarationKind, Scope, ScoreVoice};
    let xml = format!(
        r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Singing</part-name></score-part><score-part id="P2"><part-name>Empty</part-name></score-part></part-list>
        <part id="P1"><measure><attributes><divisions>480</divisions></attributes>{}</measure></part>
        <part id="P2"><measure><attributes><divisions>480</divisions></attributes><direction><direction-type><wedge type="stop" number="9"/></direction-type><offset sound="yes">720.000000000000000001</offset><staff>1</staff></direction></measure></part></score-partwise>"#,
        note(960, 1, false)
    );
    let source = musicxml::parse(xml.as_bytes()).unwrap();
    let input = source.score_intensity.as_ref().unwrap();
    // Zero-width declarations stay in the original inventory; they have no
    // performed wedge-pairing issue. Normalization supplies their source-only
    // diagnostic from this independent written evidence.
    let mut stops = input
        .original_declarations
        .iter()
        .filter(|(_, original)| original.kinds.contains(&DeclarationKind::WedgeStop));
    let (id, original) = stops.next().expect("original written wedge stop");
    assert!(stops.next().is_none());
    let declaration = &input.declarations[id];
    assert!(input.retained[id].source_ids.contains(id));
    let written = &input.written_measures[original.written_measure.unwrap()];
    assert_eq!(written.part, "P2");
    assert_eq!(written.start, written.end);
    let source_owner = ScoreVoice {
        part: "P2".into(),
        staff: "1".into(),
        voice: String::new(),
        instrument: None,
    };
    assert!(input
        .owner_runs(&source_owner)
        .is_none_or(|runs| runs.is_empty()));
    assert_eq!(
        declaration.scope,
        Scope::Staff {
            part: "P2".into(),
            staff: "1".into()
        }
    );
    let owner = source
        .tracks
        .iter()
        .find(|t| t.source.part_id.as_deref() == Some("P2"))
        .unwrap();
    let at = Fraction::decimal("720.000000000000000001")
        .unwrap()
        .checked_mul(Fraction::new(1, 480).unwrap())
        .unwrap();
    assert_eq!(declaration.at, at);
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, target);
        assert_eq!(outcome.svp.as_ref().unwrap().tracks.len(), 1);
        let ledger = saved(&source, &outcome, target);
        let entry = ledger
            .entries
            .iter()
            .find(|entry| &entry.source_id == id)
            .unwrap();
        assert_eq!(entry.performance_refs.len(), 1);
        let span = &ledger.performance_spans[entry.performance_refs[0]];
        assert_eq!(span.source_track_id, owner.id);
        assert!(span.note_ids.is_empty() && span.target_track.is_none());
        assert_eq!((span.start_tick, span.end_tick), (720, 721));
        assert!(span.detail.contains("performed scope is unresolved"));
        let evidence = span.intensity.as_ref().unwrap();
        assert_eq!(evidence["start"], serde_json::to_value(at).unwrap());
        assert_eq!(evidence["end"], evidence["start"]);
        assert_eq!(evidence["provenance"]["occurrence"], 0);
        assert_eq!(evidence["provenance"]["repeat_pass"], 0);
        assert_eq!(
            evidence["provenance"]["scope"],
            serde_json::to_value(&declaration.scope).unwrap()
        );
    }
}

#[test]
fn ambiguous_zero_width_measure_is_source_only_even_inside_a_positive_run() {
    let xml = format!(
        r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>First</part-name></score-part><score-part id="P2"><part-name>Second</part-name></score-part></part-list>
        <part id="P1"><measure><attributes><divisions>480</divisions></attributes>{}</measure><measure/><measure>{}</measure></part>
        <part id="P2"><measure><attributes><divisions>480</divisions></attributes><forward><duration>720</duration></forward></measure>
        <measure><direction><direction-type><dynamics><p/></dynamics></direction-type><staff>1</staff></direction></measure>
        <measure><direction><direction-type><dynamics><f/></dynamics></direction-type><offset sound="yes">240</offset><staff>1</staff></direction>{}</measure></part></score-partwise>"#,
        note(720, 1, false),
        note(960, 1, false),
        note(960, 1, false)
    );
    let source = musicxml::parse(xml.as_bytes()).unwrap();
    let input = source.score_intensity.as_ref().unwrap();
    let id = input
        .declarations
        .iter()
        .find(|(_, d)| d.at == Fraction::new(3, 2).unwrap())
        .unwrap()
        .0;
    let owner = source
        .tracks
        .iter()
        .find(|t| t.source.part_id.as_deref() == Some("P2"))
        .unwrap();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert(&source, target);
        let ledger = saved(&source, &outcome, target);
        let entry = ledger.entries.iter().find(|e| &e.source_id == id).unwrap();
        assert_eq!(entry.performance_refs.len(), 1);
        let span = &ledger.performance_spans[entry.performance_refs[0]];
        assert_eq!(span.source_track_id, owner.id);
        assert!(span.note_ids.is_empty() && span.target_track.is_none());
        assert_eq!((span.start_tick, span.end_tick), (720, 720));
        assert!(span.detail.contains("performed scope is unresolved"));
        assert_eq!(
            span.intensity.as_ref().unwrap()["provenance"]["repeat_pass"],
            0
        );
        assert_eq!(span.intensity.as_ref().unwrap()["terminalEndpoint"], false);
        if target == ExportTarget::Ustx {
            assert!(ledger
                .performance_spans
                .iter()
                .any(|s| s.source_track_id == owner.id
                    && s.status == TransferStatus::Mapped
                    && s.start_tick == 960
                    && s.end_tick == 1680));
        }
    }
}
