//! R4-01/06/14/16: real loaders, both target writers, and persisted ledgers.
use std::collections::BTreeSet;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use verse_lib::bundle::{build_preservation_ledger, BundleLayout, PreservationLedger};
use verse_lib::engine::convert::{convert_midi_with_target, ConvertOutcome};
use verse_lib::engine::midi::{Kind, Midi};
use verse_lib::engine::performance::{normalize, TransferStatus};
use verse_lib::engine::score_intensity::{source::DeclarationKind, Fraction, ScoreVoice};
use verse_lib::engine::{
    musescore, musicxml,
    target::{self, ExportTarget},
};
use verse_lib::stems::StemPlan;

fn note(duration: u32) -> String {
    format!("<note><pitch><step>C</step><octave>4</octave></pitch><duration>{duration}</duration><voice>1</voice><staff>1</staff><lyric><text>la</text></lyric></note>")
}
fn xml(measures: &str) -> String {
    format!(
        r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1">{measures}</part></score-partwise>"#
    )
}
fn source_id(source: &Midi, marker: &str) -> String {
    let fragment = format!(r#"id="{marker}""#);
    source
        .score_intensity
        .as_ref()
        .unwrap()
        .retained
        .iter()
        .find(|(_, e)| {
            e.raw_fields
                .get("xml")
                .is_some_and(|raw| raw.contains(&fragment))
        })
        .map(|(id, _)| id.clone())
        .expect("original raw field")
}
fn owner() -> ScoreVoice {
    ScoreVoice {
        part: "P1".into(),
        staff: "1".into(),
        voice: "1".into(),
        instrument: None,
    }
}
fn converted(source: &Midi, target: ExportTarget) -> ConvertOutcome {
    let before = source.tracks.clone();
    let result = convert_midi_with_target(source, "english", None, target);
    assert!(result.ok, "{:?}", result.msg);
    assert_eq!(source.tracks, before);
    result
}
fn saved(source: &Midi, outcome: &ConvertOutcome, target: ExportTarget) -> PreservationLedger {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = std::env::temp_dir().join(format!(
        "verse-performance-review-5-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let filename = if source.source_format == verse_lib::engine::midi::SourceFormat::MuseScore {
        "source.mscx"
    } else {
        "source.musicxml"
    };
    let layout = BundleLayout::new(&directory.join("case.versebundle"), filename, target).unwrap();
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
    let bytes = target::serialize_to(target, outcome.svp.as_ref().unwrap()).unwrap();
    let project_path = directory.join("export");
    fs::write(&project_path, &bytes).unwrap();
    assert_eq!(fs::read(project_path).unwrap(), bytes);
    let ledger_path = directory.join("preservation.json");
    fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let decoded: PreservationLedger =
        serde_json::from_slice(&fs::read(ledger_path).unwrap()).unwrap();
    decoded.validate(&allowed).unwrap();
    assert_eq!(
        decoded.performance_spans,
        outcome.projection.performance_spans
    );
    fs::remove_dir_all(directory).unwrap();
    decoded
}
fn assert_written_only(ledger: &PreservationLedger, source: &Midi, id: &str, track: &str) {
    let declaration = &source.score_intensity.as_ref().unwrap().declarations[id];
    let entry = ledger.entries.iter().find(|e| e.source_id == id).unwrap();
    assert_eq!(entry.performance_refs.len(), 1, "{entry:?}");
    let span = &ledger.performance_spans[entry.performance_refs[0]];
    assert_eq!(span.source_track_id, track);
    assert_eq!(span.status, TransferStatus::Unsupported);
    assert!(span.target_track.is_none() && span.note_ids.is_empty());
    let evidence = span.intensity.as_ref().unwrap();
    assert_eq!(
        evidence["start"],
        serde_json::to_value(declaration.at).unwrap()
    );
    assert_eq!(evidence["end"], evidence["start"]);
    assert_eq!(
        evidence["provenance"]["scope"],
        serde_json::to_value(&declaration.scope).unwrap()
    );
    assert_eq!(evidence["provenance"]["occurrence"], 0);
    assert_eq!(evidence["provenance"]["repeat_pass"], 0);
    assert!(span.detail.contains("performed scope is unresolved"));
    assert!(evidence["provenance"]["interpretations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["field"] == "performed-scope"));
}

#[test]
fn ordinary_1500_note_score_keeps_shared_segments_and_both_saved_ledgers() {
    let mut measures = String::new();
    for i in 0..1500 {
        let dynamic = if i % 2 == 0 { "p" } else { "f" };
        measures.push_str(&format!("<measure><attributes><divisions>480</divisions></attributes><direction><direction-type><dynamics><{dynamic}/></dynamics></direction-type></direction>{}</measure>", note(480)));
    }
    let source = musicxml::parse(xml(&measures).as_bytes()).unwrap();
    let index = normalize(&source).unwrap();
    assert_eq!(index.bindings.len(), 1500);
    let shared = &index
        .bindings
        .values()
        .next()
        .unwrap()
        .intensity
        .as_ref()
        .unwrap()
        .timeline;
    assert!(shared.segments.len() >= 1500);
    assert!(index
        .bindings
        .values()
        .all(|b| std::sync::Arc::ptr_eq(shared, &b.intensity.as_ref().unwrap().timeline)));
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = converted(&source, target);
        assert_eq!(
            outcome
                .svp
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .map(|t| t.notes.len())
                .sum::<usize>(),
            1500
        );
        let ledger = saved(&source, &outcome, target);
        let status = if target == ExportTarget::Ustx {
            TransferStatus::Mapped
        } else {
            TransferStatus::Unsupported
        };
        let spans: Vec<_> = ledger
            .performance_spans
            .iter()
            .filter(|s| !s.note_ids.is_empty())
            .collect();
        assert_eq!(spans.len(), 1500);
        assert!(spans
            .iter()
            .all(|s| s.status == status && s.intensity.is_some()));
    }
}

#[test]
fn zero_width_offset_membership_never_captures_the_following_positive_measure() {
    for offset in ["240", "240.000000000000000001"] {
        for following_offset in [0, 240] {
            let measures = format!(
                r#"<measure><attributes><divisions>480</divisions></attributes>{}</measure>
                <measure><direction id="empty-mark"><direction-type><dynamics><p/></dynamics></direction-type><offset sound="yes">{offset}</offset><staff>1</staff></direction></measure>
                <measure><direction id="played-mark"><direction-type><dynamics><f/></dynamics></direction-type><offset sound="yes">{following_offset}</offset><staff>1</staff></direction>{}</measure>"#,
                note(720),
                note(960)
            );
            let source = musicxml::parse(xml(&measures).as_bytes()).unwrap();
            let empty_id = source_id(&source, "empty-mark");
            let played_id = source_id(&source, "played-mark");
            let input = source.score_intensity.as_ref().unwrap();
            assert!(input.declaration_is_unresolved(&empty_id, &owner()));
            assert!(!input.declaration_is_unresolved(&played_id, &owner()));
            let expected = Fraction::new(3, 2)
                .unwrap()
                .checked_add(
                    Fraction::decimal(offset)
                        .unwrap()
                        .checked_mul(Fraction::new(1, 480).unwrap())
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(input.declarations[&empty_id].at, expected);
            let raw = normalize(&source).unwrap();
            let track = &raw.events[&empty_id].track_id;
            for binding in raw.bindings.values() {
                let timeline = &binding.intensity.as_ref().unwrap().timeline;
                assert!(!timeline
                    .issues
                    .iter()
                    .flat_map(|i| &i.provenance.evidence)
                    .chain(
                        timeline
                            .segments
                            .iter()
                            .filter_map(|s| s.provenance.as_ref())
                            .flat_map(|p| &p.evidence)
                    )
                    .any(|e| e.source_ids.contains(&empty_id)));
            }
            for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                let outcome = converted(&source, target);
                assert_eq!(outcome.svp.as_ref().unwrap().tracks[0].notes.len(), 2);
                let ledger = saved(&source, &outcome, target);
                assert_written_only(&ledger, &source, &empty_id, track);
                let played = ledger
                    .entries
                    .iter()
                    .find(|e| e.source_id == played_id)
                    .unwrap();
                let spans: Vec<_> = played
                    .performance_refs
                    .iter()
                    .map(|index| &ledger.performance_spans[*index])
                    .filter(|span| span.target_track.is_some() && !span.note_ids.is_empty())
                    .collect();
                let played_start = 720 + following_offset;
                assert!(spans.iter().any(|span| {
                    if target == ExportTarget::Ustx {
                        span.status == TransferStatus::Mapped
                            && span.start_tick == played_start && span.end_tick == 1680
                    } else {
                        // SVP reports unsupported intent over the whole note;
                        // exact declaration timing lives in its borrowed segments.
                        // It must not borrow USTX's mapped-span reporting shape.
                        span.status == TransferStatus::Unsupported
                            && span.start_tick == 720 && span.end_tick == 1680
                            && span.intensity.as_ref().unwrap()["segments"].as_array().unwrap()
                                .iter().any(|segment| {
                                    segment["start"] == serde_json::to_value(Fraction::new(i64::from(played_start), 480).unwrap()).unwrap()
                                        && segment["end"] == serde_json::to_value(Fraction::new(1680, 480).unwrap()).unwrap()
                                        && segment["curve"] != "Absent"
                                        && segment["provenance"]["evidence"].as_array().is_some_and(|evidence|
                                            evidence.iter().any(|e| e["source_ids"].as_array().is_some_and(|ids|
                                                ids.iter().any(|id| id.as_str() == Some(played_id.as_str())))))
                                })
                    }
                }), "{target:?}, empty offset {offset}, following offset {following_offset}: {spans:?}");
            }
        }
    }
}

#[test]
fn unplayed_ending_velocity_retains_exact_written_owner_without_a_note_binding() {
    for velocity in ["88.125", "bad"] {
        let skipped_note = note(480).replacen(
            "<note>",
            &format!(r#"<note id="skipped-velocity" dynamics="{velocity}">"#),
            1,
        );
        // No repeat reaches pass three: this positive written measure is skipped.
        let measures = format!(
            r#"<measure><attributes><divisions>480</divisions></attributes>{}</measure>
            <measure><barline location="left"><ending number="3" type="start"/></barline>{skipped_note}<barline location="right"><ending number="3" type="discontinue"/></barline></measure>
            <measure>{}</measure>"#,
            note(480),
            note(480)
        );
        let source = musicxml::parse(xml(&measures).as_bytes()).unwrap();
        let id = source_id(&source, "skipped-velocity");
        let input = source.score_intensity.as_ref().unwrap();
        let original = &input.original_declarations[&id];
        assert!(original.kinds.contains(&DeclarationKind::Velocity));
        let measure = &input.written_measures[original.written_measure.unwrap()];
        assert!(measure.end > measure.start && measure.visits.is_empty());
        assert_eq!(input.declarations[&id].at, Fraction::ONE);
        assert!(!source
            .tracks
            .iter()
            .flat_map(|t| &t.events)
            .any(|e| matches!(&e.kind,
            Kind::NoteOn(n) if Some(&n.source.id) == original.note_source_id.as_ref())));
        let normalized = normalize(&source).unwrap();
        let track = &normalized.events[&id].track_id;
        assert!(normalized.bindings.values().all(|b| b
            .intensity
            .as_ref()
            .is_none_or(|n| n.issues.is_empty() && n.provenance.is_none())));
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let outcome = converted(&source, target);
            assert_eq!(outcome.svp.as_ref().unwrap().tracks[0].notes.len(), 2);
            assert_written_only(&saved(&source, &outcome, target), &source, &id, track);
        }
    }
}

#[test]
fn musescore_unplayed_velocity_uses_written_membership_in_both_saved_targets() {
    for (version, program) in [("3.02", "3.6.2"), ("4.70", "4.7.4")] {
        for velocity in ["100", "bad"] {
            let sung = "<Chord><durationType>quarter</durationType><Lyrics><text>la</text></Lyrics><Note><pitch>60</pitch></Note></Chord>";
            let skipped = sung.replace("<Note>", &format!(r#"<Note id="ms-skipped"><veloType>user</veloType><velocity>{velocity}</velocity>"#));
            let document = format!(
                r#"<museScore version="{version}"><programVersion>{program}</programVersion><Score><Division>480</Division>
                <Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part><Staff id="1">
                <Measure len="1/4"><voice>{sung}</voice></Measure>
                <Measure len="1/4"><Spanner type="Volta"><Volta><endings>3</endings></Volta><next><location><measures>1</measures></location></next></Spanner><voice>{skipped}</voice></Measure>
                <Measure len="1/4"><voice>{sung}</voice></Measure>
                </Staff></Score></museScore>"#
            );
            let source = musescore::parse(document.as_bytes()).unwrap();
            let id = source_id(&source, "ms-skipped");
            let input = source.score_intensity.as_ref().unwrap();
            let original = &input.original_declarations[&id];
            assert!(original.kinds.contains(&DeclarationKind::Velocity));
            assert!(input.written_measures[original.written_measure.unwrap()]
                .visits
                .is_empty());
            assert_eq!(input.declarations[&id].at, Fraction::ONE);
            let normalized = normalize(&source).unwrap();
            let track = &normalized.events[&id].track_id;
            for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                let outcome = converted(&source, target);
                assert_eq!(
                    outcome
                        .svp
                        .as_ref()
                        .unwrap()
                        .tracks
                        .iter()
                        .map(|t| t.notes.len())
                        .sum::<usize>(),
                    2
                );
                assert_written_only(&saved(&source, &outcome, target), &source, &id, track);
            }
        }
    }
}

#[test]
fn silent_declared_voice_on_sounding_staff_keeps_original_metadata_owner() {
    for (instruction, ending) in [
        (r#"<wedge type="stop" number="8"/>"#, 240),
        ("<dynamics><p/></dynamics>", 960),
    ] {
        let measures = format!(
            r#"<measure><attributes><divisions>480</divisions></attributes>
        <barline location="left"><repeat direction="forward"/></barline>
        <direction id="silent-voice"><direction-type>{instruction}</direction-type><offset sound="yes">240</offset><voice>2</voice><staff>1</staff></direction>{}
        <barline location="right"><repeat direction="backward"/></barline></measure>"#,
            note(960)
        );
        let source = musicxml::parse(xml(&measures).as_bytes()).unwrap();
        let id = source_id(&source, "silent-voice");
        let metadata = source
            .tracks
            .iter()
            .find(|t| {
                t.source.part_id.as_deref() == Some("P1")
                    && t.source.staff_id.as_deref() == Some("1")
                    && t.source.voice.is_none()
            })
            .unwrap();
        assert!(!metadata
            .events
            .iter()
            .any(|e| matches!(e.kind, Kind::NoteOn(_))));
        assert!(source
            .tracks
            .iter()
            .any(|t| t.source.voice.as_deref() == Some("1")));
        let normalized = normalize(&source).unwrap();
        assert_eq!(normalized.events[&id].track_id, metadata.id);
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let outcome = converted(&source, target);
            assert_eq!(outcome.svp.as_ref().unwrap().tracks.len(), 1);
            assert_eq!(outcome.svp.as_ref().unwrap().tracks[0].notes.len(), 2);
            let ledger = saved(&source, &outcome, target);
            let entry = ledger.entries.iter().find(|e| e.source_id == id).unwrap();
            assert_eq!(entry.performance_refs.len(), 2);
            for (pass, reference) in entry.performance_refs.iter().enumerate() {
                let span = &ledger.performance_spans[*reference];
                assert_eq!(span.source_track_id, metadata.id);
                assert!(span.target_track.is_none() && span.note_ids.is_empty());
                assert_eq!(
                    (span.start_tick, span.end_tick),
                    (240 + pass as u32 * 960, ending + pass as u32 * 960)
                );
                let evidence = span.intensity.as_ref().unwrap();
                assert_eq!(evidence["provenance"]["repeat_pass"], pass as u32 + 1);
                assert_eq!(
                    evidence["provenance"]["scope"],
                    serde_json::to_value(
                        &source.score_intensity.as_ref().unwrap().declarations[&id].scope
                    )
                    .unwrap()
                );
            }
        }
    }
}

#[test]
fn pass_specific_wedge_issue_cannot_leak_into_a_later_matched_pass() {
    let measures = format!(
        r#"<measure><attributes><divisions>480</divisions></attributes>
        <barline location="left"><repeat direction="forward"/></barline>
        <direction><direction-type><dynamics><p/></dynamics></direction-type></direction>
        <direction id="pass-start"><direction-type><wedge type="crescendo" number="6"/></direction-type></direction>{}
        <direction id="pass-stop"><direction-type><wedge type="stop" number="6"/></direction-type><sound time-only="2"/></direction>
        <barline location="right"><repeat direction="backward"/></barline></measure>"#,
        note(960)
    );
    let source = musicxml::parse(xml(&measures).as_bytes()).unwrap();
    let id = source_id(&source, "pass-start");
    let issues: Vec<_> = source
        .score_intensity
        .as_ref()
        .unwrap()
        .issues
        .iter()
        .filter(|i| {
            i.provenance.repeat_pass == 1
                && i.provenance
                    .evidence
                    .iter()
                    .any(|e| e.source_ids.contains(&id))
        })
        .collect();
    assert!(!issues.is_empty(), "pass one has no eligible stop");
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = converted(&source, target);
        let ledger = saved(&source, &outcome, target);
        for issue in &issues {
            let reported: Vec<_> = ledger
                .performance_spans
                .iter()
                .filter(|s| s.detail == issue.message)
                .collect();
            assert!(!reported.is_empty());
            assert!(reported
                .iter()
                .all(|s| s.intensity.as_ref().unwrap()["provenance"]["repeat_pass"] == 1));
        }
        if target == ExportTarget::Ustx {
            assert!(ledger
                .performance_spans
                .iter()
                .any(|s| s.status == TransferStatus::Mapped
                    && s.start_tick >= 960
                    && s.intensity
                        .as_ref()
                        .is_some_and(|i| i["curve"].get("Transition").is_some())));
        }
    }
}
