//! R2-10: validate emitted extensions, not a hand-invented required extension.
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};
use verse_lib::{
    bundle::{build_preservation_ledger, BundleLayout, PreservationLedger},
    engine::{
        convert::convert_midi_with_target, midi::Midi, musescore, musicxml, target::ExportTarget,
    },
    stems::StemPlan,
};

fn ledger(source: &Midi, target: ExportTarget) -> (PreservationLedger, BTreeSet<String>) {
    let outcome = convert_midi_with_target(source, "english", None, target);
    assert!(outcome.ok, "{:?}", outcome.msg);
    let layout =
        BundleLayout::new(Path::new("Verification.versebundle"), "source.xml", target).unwrap();
    let stems = StemPlan::from_source(source, &outcome.tracks).unwrap();
    let allowed = [
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
    (
        build_preservation_ledger(source, &outcome.projection, &layout, &stems),
        allowed,
    )
}

fn xml(body: &str) -> Midi {
    musicxml::parse(format!(r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes>{body}</measure></part></score-partwise>"#).as_bytes()).unwrap()
}
const NOTE: &str = "<note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><lyric><text>la</text></lyric></note>";
const P: &str = "<direction><direction-type><dynamics><p/></dynamics></direction-type></direction>";
const F: &str = "<direction><direction-type><dynamics><f/></dynamics></direction-type></direction>";

fn reopened(value: &PreservationLedger) -> PreservationLedger {
    serde_json::from_slice(&serde_json::to_vec(value).unwrap()).unwrap()
}

#[test]
fn emitted_intensity_variants_roundtrip_with_legacy_absence() {
    let mut shapes = BTreeSet::new();
    let sources = [
        musicxml::parse(include_bytes!("score-intensity-fixtures/contour.musicxml")).unwrap(),
        musicxml::parse(include_bytes!("score-intensity-fixtures/niente.musicxml")).unwrap(),
        musescore::parse(include_bytes!(
            "score-intensity-fixtures/contour-legacy.mscx"
        ))
        .unwrap(),
        musescore::parse(include_bytes!(
            "score-intensity-fixtures/contour-modern.mscx"
        ))
        .unwrap(),
        xml(&format!("{P}{NOTE}{F}")),
        xml(&format!(
            "<direction><sound dynamics=\"200\"/></direction>{NOTE}{P}{NOTE}"
        )),
        xml(&format!(
            "<direction><direction-type><wedge type=\"stop\"/></direction-type></direction>{NOTE}"
        )),
        xml(&NOTE.replace("<note>", "<note dynamics=\"100\">")),
    ];
    for source in &sources {
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let (value, allowed) = ledger(source, target);
            assert!(!value.performance_spans.is_empty());
            value.validate(&allowed).unwrap();
            let copy = reopened(&value);
            assert_eq!(copy, value);
            copy.validate(&allowed).unwrap();
            for span in &value.performance_spans {
                if let Some(e) = &span.intensity {
                    shapes.insert(if e.get("targetStart").is_some() {
                        "gain"
                    } else if e.get("segments").is_some() {
                        "segments"
                    } else if e.get("curve").is_some() {
                        "terminal"
                    } else {
                        "issue"
                    });
                }
            }
            // Historical schema 3 records legitimately omit this extension.
            let mut legacy = value.clone();
            legacy.intensity_context = None;
            for span in &mut legacy.performance_spans {
                span.intensity = None;
            }
            reopened(&legacy).validate(&allowed).unwrap();
        }
    }
    assert_eq!(
        shapes,
        BTreeSet::from(["gain", "segments", "terminal", "issue"])
    );
    let (plain, allowed) = ledger(&xml(NOTE), ExportTarget::Ustx);
    assert_eq!(plain.schema_version, 2);
    assert!(plain.performance_spans.is_empty());
    reopened(&plain).validate(&allowed).unwrap();
}

#[test]
fn saved_schema3_intensity_contexts_without_route_origins_remain_readable() {
    let sources = [
        musicxml::parse(include_bytes!("score-intensity-fixtures/contour.musicxml")).unwrap(),
        musicxml::parse(include_bytes!("score-intensity-fixtures/niente.musicxml")).unwrap(),
        xml(&format!("{P}{NOTE}{NOTE}{F}{NOTE}")),
        xml(&format!(
            "<barline location=\"left\"><repeat direction=\"forward\"/></barline>{P}{NOTE}{F}{NOTE}<barline location=\"right\"><repeat direction=\"backward\"/></barline>"
        )),
    ];
    for source in &sources {
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let (current, allowed) = ledger(source, target);
            current.validate(&allowed).unwrap();
            let mut saved = serde_json::to_value(&current).unwrap();
            let context = saved["intensityContext"].as_object_mut().unwrap();
            context.remove("dependencies");
            for declaration in context["declarations"].as_array_mut().unwrap() {
                for application in declaration["applications"].as_array_mut().unwrap() {
                    assert!(application
                        .as_object_mut()
                        .unwrap()
                        .remove("writtenStart")
                        .is_some());
                }
            }
            // This is the prior serialized context shape, with the intensity
            // extension still present. Do not substitute no-intensity3 coverage.
            let legacy: PreservationLedger = serde_json::from_value(saved.clone()).unwrap();
            assert_eq!(legacy.schema_version, 3);
            assert!(legacy
                .performance_spans
                .iter()
                .any(|span| span.intensity.is_some()));
            reopened(&legacy).validate(&allowed).unwrap();
            assert_eq!(serde_json::to_value(&legacy).unwrap(), saved);

            // Legacy compatibility does not discard existing source ownership.
            let mut bad = legacy.clone();
            bad.intensity_context.as_mut().unwrap().declarations[0].applications[0]
                .owner
                .part = "unrelated-part".into();
            assert!(reopened(&bad).validate(&allowed).is_err());

            for corruption in [Value::Null, json!({"numerator":0,"denominator":0})] {
                let mut bad = current.clone();
                bad.intensity_context.as_mut().unwrap().declarations[0].applications[0]
                    .written_start = corruption;
                assert!(reopened(&bad).validate(&allowed).is_err());
            }
        }
    }
}

#[test]
fn malformed_emitted_intensity_structure_and_nested_references_are_rejected() {
    let source =
        musicxml::parse(include_bytes!("score-intensity-fixtures/contour.musicxml")).unwrap();
    let (valid, allowed) = ledger(&source, ExportTarget::Ustx);
    valid.validate(&allowed).unwrap();
    let index = valid
        .performance_spans
        .iter()
        .position(|s| {
            s.intensity
                .as_ref()
                .is_some_and(|e| e.get("targetStart").is_some())
        })
        .unwrap();
    for (pointer, bad) in [
        ("/policy", json!("another-policy")),
        ("/start/denominator", json!(0)),
        ("/end/numerator", json!(-1)),
        (
            "/start/numerator",
            json!("9999999999999999999999999999999999999999999999"),
        ),
        ("/provenance", Value::Null),
        ("/provenance/0/policy", json!("another-policy")),
        ("/provenance/0/scope", json!({"Unknown":"P1"})),
        ("/provenance/0/occurrence", json!(-1)),
        ("/provenance/0/evidence/0/contract", json!("InventedFormat")),
        (
            "/provenance/0/evidence/0/source_ids/0",
            json!("unrelated-source"),
        ),
        ("/provenance/0/evidence/0/raw_fields", json!([])),
        (
            "/provenance/0/interpretations/0/source_ids/0",
            json!("unrelated-interpretation"),
        ),
        (
            "/provenance/0/interpretations/0/basis",
            json!("InventedBasis"),
        ),
        (
            "/curve",
            json!({"Level":{"Positive":{"numerator":0,"denominator":1}}}),
        ),
        ("/targetEnd", json!(-1)),
        ("/targetEnd", json!(999)),
        ("/rounding", json!("truncate")),
        ("/limitedNienteTail", json!(true)),
    ] {
        let mut bad_ledger = valid.clone();
        *bad_ledger.performance_spans[index]
            .intensity
            .as_mut()
            .unwrap()
            .pointer_mut(pointer)
            .unwrap() = bad;
        assert!(
            reopened(&bad_ledger).validate(&allowed).is_err(),
            "accepted {pointer}"
        );
    }
    for terminal in [json!(true), json!("true")] {
        let mut bad = valid.clone();
        bad.performance_spans[index].intensity.as_mut().unwrap()["terminalEndpoint"] = terminal;
        assert!(reopened(&bad).validate(&allowed).is_err());
    }
    let mut missing_structure = valid.clone();
    missing_structure.performance_spans[index].intensity = Some(json!({
        "start":{"numerator":0,"denominator":1}, "end":{"numerator":1,"denominator":1}
    }));
    assert!(reopened(&missing_structure).validate(&allowed).is_err());
    let mut missing_reference = valid.clone();
    let id = valid.performance_spans[index].intensity.as_ref().unwrap()["provenance"][0]
        ["evidence"][0]["source_ids"][0]
        .as_str()
        .unwrap();
    missing_reference
        .entries
        .iter_mut()
        .find(|e| e.source_id == id)
        .unwrap()
        .performance_refs
        .retain(|r| *r != index);
    assert!(reopened(&missing_reference).validate(&allowed).is_err());
    let mut wrong_note = valid.clone();
    wrong_note.performance_spans[index].note_ids = vec!["missing-note".into()];
    assert!(reopened(&wrong_note).validate(&allowed).is_err());

    let (valid, allowed) = ledger(&source, ExportTarget::Svp);
    let index = valid
        .performance_spans
        .iter()
        .position(|s| {
            s.intensity
                .as_ref()
                .is_some_and(|e| e.get("segments").is_some())
        })
        .unwrap();
    let mut bad = valid.clone();
    bad.performance_spans[index].intensity.as_mut().unwrap()["segments"][0]["curve"] =
        json!({"Held":{"Decibels":null}});
    assert!(reopened(&bad).validate(&allowed).is_err());
    let mut bad = valid.clone();
    bad.performance_spans[index].intensity.as_mut().unwrap()["segments"][0]["provenance"]
        ["evidence"][0]["source_ids"][0] = json!("unrelated-segment-source");
    assert!(reopened(&bad).validate(&allowed).is_err());
}

#[test]
fn terminal_metadata_cannot_claim_a_note_or_mapped_sounding_interval() {
    let source = xml(&format!("{P}{NOTE}{F}"));
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        let (valid, allowed) = ledger(&source, target);
        valid.validate(&allowed).unwrap();
        let index = valid
            .performance_spans
            .iter()
            .position(|s| {
                s.intensity
                    .as_ref()
                    .is_some_and(|e| e["terminalEndpoint"] == true)
            })
            .unwrap();
        assert!(valid.performance_spans[index].note_ids.is_empty());
        for mutation in 0..3 {
            let mut bad = valid.clone();
            let span = &mut bad.performance_spans[index];
            match mutation {
                0 => {
                    span.note_ids = vec![valid
                        .entries
                        .iter()
                        .find(|e| e.item_kind == verse_lib::bundle::SourceItemKind::Note)
                        .unwrap()
                        .source_id
                        .clone()]
                }
                1 => span.status = verse_lib::engine::performance::TransferStatus::Mapped,
                _ => {
                    span.intensity.as_mut().unwrap()["end"] = json!({"numerator":2,"denominator":1})
                }
            }
            assert!(reopened(&bad).validate(&allowed).is_err());
        }
    }
}

fn rejects(value: &PreservationLedger, allowed: &BTreeSet<String>, reason: &str) {
    let error = reopened(value).validate(allowed).unwrap_err();
    let verse_lib::bundle::BundleError::InvalidLedger(message) = error else {
        panic!("unexpected error: {error}");
    };
    assert_eq!(message, reason);
}

fn gain_index(value: &PreservationLedger) -> usize {
    value
        .performance_spans
        .iter()
        .position(|span| {
            span.intensity
                .as_ref()
                .is_some_and(|e| e.get("targetStart").is_some())
                && span.status == verse_lib::engine::performance::TransferStatus::Mapped
        })
        .unwrap()
}

#[test]
fn mapped_score_intensity_cannot_erase_provenance_or_actual_contributors() {
    let (valid, allowed) = ledger(&xml(&format!("{P}{NOTE}{NOTE}")), ExportTarget::Ustx);
    reopened(&valid).validate(&allowed).unwrap();
    let index = gain_index(&valid);
    let mut bad = reopened(&valid);
    bad.performance_spans[index].intensity.as_mut().unwrap()["provenance"] = json!([]);
    rejects(
        &bad,
        &allowed,
        "intensity has no score provenance or inventoried controller support",
    );
    let mut bad = reopened(&valid);
    let source = valid.performance_spans[index].intensity.as_ref().unwrap()["provenance"][0]
        ["evidence"][0]["source_ids"][0]
        .as_str()
        .unwrap();
    let entry = bad
        .entries
        .iter_mut()
        .find(|e| e.source_id == source)
        .unwrap();
    // A second real mapped span keeps the primary disposition valid, isolating
    // the missing contributing reference for this particular span.
    assert!(entry.performance_refs.iter().any(|i| *i != index));
    entry.performance_refs.retain(|i| *i != index);
    rejects(
        &bad,
        &allowed,
        "intensity source ID does not reference its containing span",
    );
}

#[test]
fn inventoried_but_unrelated_track_note_and_target_owners_are_rejected() {
    let source = musicxml::parse(format!(r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>First</part-name></score-part><score-part id="P2"><part-name>Second</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes>{P}{NOTE}</measure></part><part id="P2"><measure><attributes><divisions>480</divisions></attributes>{F}{NOTE}</measure></part></score-partwise>"#).as_bytes()).unwrap();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&source, target);
        let valid = reopened(&valid);
        valid.validate(&allowed).unwrap();
        let index = valid
            .performance_spans
            .iter()
            .position(|s| s.intensity.is_some() && !s.note_ids.is_empty())
            .unwrap();
        let span = &valid.performance_spans[index];
        let other = valid
            .intensity_context
            .as_ref()
            .unwrap()
            .projected_notes
            .iter()
            .find(|n| {
                n.source_track_id != span.source_track_id
                    && Some(n.target_track) != span.target_track
            })
            .unwrap();
        for mutation in 0..3 {
            let mut bad = valid.clone();
            match mutation {
                0 => bad.performance_spans[index].source_track_id = other.source_track_id.clone(),
                1 => bad.performance_spans[index].note_ids = vec![other.note_id.clone()],
                _ => bad.performance_spans[index].target_track = Some(other.target_track),
            }
            rejects(
                &bad,
                &allowed,
                if mutation == 2 {
                    "intensity note has no eligible target ownership"
                } else {
                    "intensity note does not belong to its source track"
                },
            );
        }
        let mut bad = valid.clone();
        bad.intensity_context = None;
        rejects(
            &bad,
            &allowed,
            "intensity source context is missing or exceeded bounded storage",
        );
    }
}

#[test]
fn exact_bounds_must_agree_with_non480_source_coverage() {
    let source = musescore::parse(br#"<museScore version="3.02"><programVersion>3.6.2</programVersion><Score><Division>960</Division><Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part><Staff id="1"><Measure len="1/4"><voice><Dynamic><subtype>p</subtype></Dynamic><Chord><durationType>quarter</durationType><Lyrics><text>la</text></Lyrics><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff></Score></museScore>"#).unwrap();
    assert_eq!(source.ticks_per_beat, 960);
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&source, target);
        reopened(&valid).validate(&allowed).unwrap();
        assert_eq!(valid.intensity_context.as_ref().unwrap().source_ppq, 960);
        let index = valid
            .performance_spans
            .iter()
            .position(|s| s.intensity.is_some() && !s.note_ids.is_empty())
            .unwrap();
        assert_eq!(valid.performance_spans[index].end_tick, 960);
        let mut bad = valid.clone();
        bad.performance_spans[index].end_tick = 480;
        rejects(
            &bad,
            &allowed,
            "exact intensity bounds disagree with source tick coverage",
        );
        let mut bad = valid.clone();
        bad.intensity_context.as_mut().unwrap().source_ppq = 480;
        rejects(
            &bad,
            &allowed,
            "exact intensity bounds disagree with source tick coverage",
        );
    }
}

#[test]
fn rounded_collapse_is_a_limit_and_cannot_be_claimed_as_mapped() {
    // Real loader emits a sub-tick ordinary dynamic interval, followed by a
    // normal state. Only the former's disposition is mutated after reload.
    let dynamic = "<direction><direction-type><dynamics><f/></dynamics></direction-type><offset sound=\"yes\">0.2</offset></direction>";
    let (valid, allowed) = ledger(&xml(&format!("{P}{dynamic}{NOTE}")), ExportTarget::Ustx);
    reopened(&valid).validate(&allowed).unwrap();
    let index = valid
        .performance_spans
        .iter()
        .position(|span| {
            span.intensity.as_ref().is_some_and(|e| {
                e.get("targetStart").is_some() && e["targetStart"] == e["targetEnd"]
            })
        })
        .expect("actual rounded collapse report");
    assert_eq!(
        valid.performance_spans[index].status,
        verse_lib::engine::performance::TransferStatus::RepresentationLimit
    );
    let mut bad = reopened(&valid);
    bad.performance_spans[index].status = verse_lib::engine::performance::TransferStatus::Mapped;
    rejects(
        &bad,
        &allowed,
        "mapped intensity requires a positive target interval",
    );
}

#[test]
fn svp_segments_are_positive_ordered_and_source_supported_with_neutral_gaps() {
    let first = "<direction><direction-type><dynamics><p/></dynamics></direction-type><offset sound=\"yes\">240</offset></direction>";
    let second = "<direction><direction-type><dynamics><f/></dynamics></direction-type><offset sound=\"yes\">720</offset></direction>";
    let (valid, allowed) = ledger(
        &xml(&format!("{first}{second}{}", NOTE.replace("480", "960"))),
        ExportTarget::Svp,
    );
    reopened(&valid).validate(&allowed).unwrap();
    let index = valid
        .performance_spans
        .iter()
        .position(|s| {
            s.intensity
                .as_ref()
                .is_some_and(|e| e.get("segments").is_some())
        })
        .unwrap();
    let segments = valid.performance_spans[index].intensity.as_ref().unwrap()["segments"]
        .as_array()
        .unwrap();
    assert!(segments.len() >= 3);
    assert_eq!(segments[0]["curve"], "Absent");
    assert!(
        segments[0]["provenance"].is_null(),
        "unexpressed gap has no invented evidence"
    );
    for mutation in 0..6 {
        let mut bad = reopened(&valid);
        let segments = bad.performance_spans[index].intensity.as_mut().unwrap()["segments"]
            .as_array_mut()
            .unwrap();
        let reason = match mutation {
            0 => {
                segments[0]["end"] = segments[0]["start"].clone();
                "intensity segment must have positive ordinary duration"
            }
            1 => {
                segments[1]["start"] = segments[0]["start"].clone();
                "intensity segments overlap or are out of order"
            }
            2 => {
                segments.swap(0, 1);
                "intensity segments overlap or are out of order"
            }
            3 => {
                segments[0]["start"] = segments[1]["end"].clone();
                "invalid exact intensity bounds"
            }
            4 => {
                segments[1]["provenance"] = Value::Null;
                "authored intensity segment has no source provenance"
            }
            _ => {
                segments.insert(1, segments[0].clone());
                "intensity segments overlap or are out of order"
            }
        };
        rejects(&bad, &allowed, reason);
    }
}

#[test]
fn moved_hold_uses_original_source_identity_and_independent_destination_ownership() {
    use verse_lib::engine::{midi::Kind, projection::ProjectedLyric};
    // The tail has no selected word: its shared chord owner's explicit extension
    // authorizes routing to the unique same-pitch member on a different lane.
    let source = musescore::parse(br#"<museScore version="3.02"><programVersion>3.6.2</programVersion><Score><Division>480</Division><Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part><Staff id="1"><Measure len="2/4"><voice><Dynamic><subtype>p</subtype></Dynamic><Chord><durationType>quarter</durationType><Lyrics><no>0</no><text>hold</text><ticks>480</ticks></Lyrics><Note><pitch>60</pitch></Note><Note><pitch>67</pitch></Note></Chord><Chord><durationType>quarter</durationType><Note><pitch>67</pitch></Note></Chord></voice></Measure></Staff></Score></museScore>"#).unwrap();
    let source_notes: Vec<_> = source
        .tracks
        .iter()
        .flat_map(|track| {
            track
                .events
                .iter()
                .filter_map(move |event| match &event.kind {
                    Kind::NoteOn(note) => Some((track, event.tick, note)),
                    _ => None,
                })
        })
        .collect();
    let (tail_track, _, tail_source) = source_notes
        .iter()
        .find(|(_, tick, note)| *tick == 480 && note.key == Some(67))
        .unwrap();
    let (head_track, _, head_source) = source_notes
        .iter()
        .find(|(_, tick, note)| *tick == 0 && note.key == Some(67))
        .unwrap();
    assert_ne!(tail_track.id, head_track.id);
    assert!(tail_source.lyrics.is_empty());
    assert_eq!(
        head_source.source.continuity.as_ref().unwrap().extensions[0].end_tick,
        Some(480)
    );
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert_midi_with_target(&source, "english", None, target);
        assert!(outcome.ok, "{:?}", outcome.msg);
        let project = outcome.svp.as_ref().unwrap();
        let (destination, tail) = project
            .tracks
            .iter()
            .flat_map(|track| track.notes.iter().map(move |note| (track, note)))
            .find(|(_, note)| note.onset_ticks == 480)
            .unwrap();
        assert!(matches!(tail.lyric, ProjectedLyric::Extension));
        let origin = tail
            .source_evidence
            .as_ref()
            .unwrap()
            .origin
            .as_ref()
            .unwrap();
        assert_eq!(origin.source, tail_source.source);
        assert_eq!(origin.track_id, tail_track.id);
        assert_eq!(destination.source_track_id, head_track.id);
        assert_eq!(
            origin.continuation.as_ref().unwrap().destination_track_id,
            head_track.id
        );
        let (valid, allowed) = ledger(&source, target);
        let valid = reopened(&valid);
        valid.validate(&allowed).unwrap();
        let context = valid.intensity_context.as_ref().unwrap();
        let moved = context
            .projected_notes
            .iter()
            .find(|n| n.start_tick == 480 && n.source_track_id != n.destination_track_id)
            .expect("actual relocated held endpoint");
        assert_eq!(moved.source_track_id, tail_track.id);
        assert_eq!(moved.destination_track_id, head_track.id);
        assert_eq!(
            moved.note_id,
            tail.source_evidence.as_ref().unwrap().note_id
        );
        let source = context
            .source_notes
            .iter()
            .find(|n| n.note_id == moved.note_id)
            .unwrap();
        assert_eq!(source.source_track_id, moved.source_track_id);
        assert!(valid.performance_spans.iter().any(|s| s.intensity.is_some()
            && s.note_ids.contains(&moved.note_id)
            && s.source_track_id == moved.source_track_id
            && s.target_track == Some(moved.target_track)));
        let mut bad = valid.clone();
        let index = bad
            .performance_spans
            .iter()
            .position(|s| s.intensity.is_some() && s.note_ids.contains(&moved.note_id))
            .unwrap();
        bad.performance_spans[index].source_track_id = moved.destination_track_id.clone();
        rejects(
            &bad,
            &allowed,
            "intensity note does not belong to its source track",
        );
    }
}

#[test]
fn controller_only_intensity_authenticates_inventoried_midi_contributors() {
    use verse_lib::engine::midi::{self, Kind};
    let track = [
        0, 0xb0, 7, 64, 0, 0xff, 5, 2, b'l', b'a', 0, 0x90, 60, 80, 0x83, 0x60, 0x80, 60, 0, 0,
        0xff, 5, 2, b'l', b'a', 0, 0x90, 62, 80, 0x83, 0x60, 0x80, 62, 0, 0, 0xff, 0x2f, 0,
    ];
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
    bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&track);
    let mut source = midi::parse(&bytes).unwrap();
    // The typed source model permits unknown velocity. The first real attack
    // enables the shared adapter; the second exercises its controller-only path.
    for event in source.tracks.iter_mut().flat_map(|t| &mut t.events) {
        if let Kind::NoteOn(note) = &mut event.kind {
            if event.tick == 480 {
                note.velocity = None;
            }
        }
    }
    let (valid, allowed) = ledger(&source, ExportTarget::Ustx);
    let valid = reopened(&valid);
    valid.validate(&allowed).unwrap();
    let index = valid
        .performance_spans
        .iter()
        .position(|s| {
            s.intensity
                .as_ref()
                .is_some_and(|e| e["provenance"] == json!([]))
        })
        .expect("actual controller-only gain extension");
    let span = &valid.performance_spans[index];
    assert_eq!(
        span.status,
        verse_lib::engine::performance::TransferStatus::Mapped
    );
    assert_eq!(span.start_tick, 480);
    let controller = &valid.intensity_context.as_ref().unwrap().controllers[0];
    assert_eq!(controller.controller, 7);
    let mut bad = valid.clone();
    bad.entries
        .iter_mut()
        .find(|e| e.source_id == controller.source_id)
        .unwrap()
        .performance_refs
        .retain(|r| *r != index);
    rejects(
        &bad,
        &allowed,
        "intensity has no score provenance or inventoried controller support",
    );
    let mut bad = valid.clone();
    bad.intensity_context.as_mut().unwrap().controllers[0]
        .owner
        .channel = 1;
    rejects(
        &bad,
        &allowed,
        "intensity controller contributor has unrelated ownership or time",
    );
    let mut bad = valid.clone();
    bad.performance_spans[index].intensity.as_mut().unwrap()["curve"] =
        json!({"Level":{"Positive":{"numerator":100,"denominator":1}}});
    rejects(
        &bad,
        &allowed,
        "intensity has no score provenance or inventoried controller support",
    );
}

/// Exercise the actual persisted JSON boundary before and after each mutation.
fn saved_mutation(
    valid: &PreservationLedger,
    mutate: impl FnOnce(&mut Value),
) -> PreservationLedger {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "verse-r4-ledger-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    serde_json::to_writer(&mut file, valid).unwrap();
    drop(file);
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    mutate(&mut value);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let result = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::remove_file(path).unwrap();
    result
}

fn replace_source(value: &mut Value, old: &str, replacement: &str) {
    match value {
        Value::String(text) if text == old => *text = replacement.into(),
        Value::Array(items) => items
            .iter_mut()
            .for_each(|v| replace_source(v, old, replacement)),
        Value::Object(items) => items
            .values_mut()
            .for_each(|v| replace_source(v, old, replacement)),
        _ => {}
    }
}

fn replace_contributor(value: &mut Value, index: usize, old: &str, replacement: &str) {
    replace_source(
        &mut value["performanceSpans"][index]["intensity"],
        old,
        replacement,
    );
    for entry in value["entries"].as_array_mut().unwrap() {
        if entry["sourceId"] == old {
            entry["performanceRefs"]
                .as_array_mut()
                .unwrap()
                .retain(|r| r.as_u64() != Some(index as u64));
        }
        if entry["sourceId"] == replacement {
            if entry.get("performanceRefs").is_none() {
                entry["performanceRefs"] = json!([]);
            }
            entry["performanceRefs"]
                .as_array_mut()
                .unwrap()
                .push(json!(index));
        }
    }
}

fn two_parts() -> Midi {
    musicxml::parse(format!(r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>First</part-name></score-part><score-part id="P2"><part-name>Second</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes>{P}{NOTE}{NOTE}</measure></part><part id="P2"><measure><attributes><divisions>480</divisions></attributes>{F}{NOTE}{NOTE}</measure></part></score-partwise>"#).as_bytes()).unwrap()
}

#[test]
fn saved_reciprocal_substitutions_require_original_expression_kind_and_part() {
    use verse_lib::bundle::SourceItemKind;
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&two_parts(), target);
        saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
        let context = valid.intensity_context.as_ref().unwrap();
        let original = context
            .declarations
            .iter()
            .find(|d| d.scope == json!({"Part":"P1"}))
            .unwrap();
        let other = context
            .declarations
            .iter()
            .find(|d| d.scope == json!({"Part":"P2"}))
            .unwrap();
        let entry = valid
            .entries
            .iter()
            .find(|e| e.source_id == original.source_id)
            .unwrap();
        assert!(
            entry.performance_refs.len() > 1,
            "removing one link must preserve the original mapped disposition"
        );
        let index = *entry
            .performance_refs
            .iter()
            .find(|i| !valid.performance_spans[**i].note_ids.is_empty())
            .unwrap();
        let replacements: Vec<_> = [
            SourceItemKind::Note,
            SourceItemKind::Track,
            SourceItemKind::Lyric,
        ]
        .iter()
        .map(|kind| {
            valid
                .entries
                .iter()
                .find(|e| &e.item_kind == kind)
                .unwrap()
                .source_id
                .as_str()
        })
        .chain([other.source_id.as_str()])
        .collect();
        for replacement in replacements {
            let bad = saved_mutation(&valid, |json| {
                replace_contributor(json, index, &original.source_id, replacement)
            });
            assert!(
                bad.validate(&allowed).is_err(),
                "accepted reciprocal substitution {replacement}"
            );
        }
    }
}

#[test]
fn saved_scope_and_occurrence_changes_fail_with_real_repeated_route_controls() {
    let source = musicxml::parse(format!(r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes><barline location="left"><repeat direction="forward"/></barline>{P}{NOTE}<barline location="right"><repeat direction="backward" times="2"/></barline></measure></part></score-partwise>"#).as_bytes()).unwrap();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&source, target);
        saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
        let index = valid
            .performance_spans
            .iter()
            .position(|s| !s.note_ids.is_empty() && s.intensity.is_some())
            .unwrap();
        let p = &valid.performance_spans[index].intensity.as_ref().unwrap()["provenance"][0];
        assert!(valid
            .intensity_context
            .as_ref()
            .unwrap()
            .declarations
            .iter()
            .any(|d| d.applications.iter().any(|a| a.repeat_pass == 2)));
        for (key, bad) in [
            ("scope", json!({"Part":"P2"})),
            ("scope", json!({"Staff":{"part":"P1","staff":"unrelated"}})),
            (
                "scope",
                json!({"Voice":{"part":"P1","staff":"1","voice":"unrelated"}}),
            ),
            ("scope", json!({"Midi":{"port":7,"channel":3}})),
            ("scope", json!("System")),
            ("occurrence", json!(p["occurrence"].as_u64().unwrap() + 1)),
            ("repeat_pass", json!(p["repeat_pass"].as_u64().unwrap() + 1)),
            ("occurrence", json!(0)),
        ] {
            let bad = saved_mutation(&valid, |value| {
                value["performanceSpans"][index]["intensity"]["provenance"][0][key] = bad
            });
            assert!(bad.validate(&allowed).is_err(), "accepted {key}");
        }
    }
}

#[test]
fn saved_source_scope_controls_include_system_part_staff_voice_and_unresolved() {
    for scope in ["", "<staff>1</staff>", "<staff>1</staff><voice>1</voice>"] {
        let direction = format!("<direction><direction-type><dynamics><p/></dynamics></direction-type>{scope}</direction>");
        let source = xml(&format!("{direction}{NOTE}"));
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let (valid, allowed) = ledger(&source, target);
            saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
        }
    }
    let music = "<Measure len=\"1/4\"><voice><Chord><durationType>quarter</durationType><Lyrics><text>la</text></Lyrics><Note><pitch>60</pitch></Note></Chord></voice></Measure>";
    let first = music.replace(
        "<Chord>",
        "<Dynamic><subtype>p</subtype><dynType>2</dynType></Dynamic><Chord>",
    );
    let system = musescore::parse(format!(r#"<museScore version="3.02"><programVersion>3.6.2</programVersion><Score><Division>480</Division><Part><trackName>First</trackName><Staff id="1"/></Part><Part><trackName>Second</trackName><Staff id="2"/></Part><Staff id="1">{first}</Staff><Staff id="2">{music}</Staff></Score></museScore>"#).as_bytes()).unwrap();
    let unresolved = musicxml::parse(format!(r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes>{P}</measure><measure>{NOTE}</measure></part></score-partwise>"#).as_bytes()).unwrap();
    for source in [&system, &unresolved] {
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let (valid, allowed) = ledger(source, target);
            saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
            let context = valid.intensity_context.as_ref().unwrap();
            if std::ptr::eq(source, &system) {
                assert!(context
                    .declarations
                    .iter()
                    .any(|d| d.scope == "System" && d.applications.len() >= 2));
            } else {
                let index = valid
                    .performance_spans
                    .iter()
                    .position(|s| {
                        s.intensity.as_ref().is_some_and(|v| {
                            v["provenance"]["occurrence"] == 0
                                && v["provenance"]["repeat_pass"] == 0
                        })
                    })
                    .expect("source-only unresolved0/0");
                assert!(valid.performance_spans[index].target_track.is_none());
                assert!(valid.performance_spans[index].note_ids.is_empty());
                let bad = saved_mutation(&valid, |v| {
                    v["performanceSpans"][index]["intensity"]["provenance"]["repeat_pass"] =
                        json!(1)
                });
                assert!(bad.validate(&allowed).is_err());
            }
        }
    }
}

#[test]
fn saved_ordinary_ustx_transition_bounds_reject_shifts_and_accept_phase_fragments() {
    let open =
        "<direction><direction-type><wedge type=\"crescendo\"/></direction-type></direction>";
    let close = "<direction><direction-type><wedge type=\"stop\"/><dynamics><f/></dynamics></direction-type></direction>";
    let (valid, allowed) = ledger(
        &xml(&format!("{P}{open}{NOTE}{NOTE}{close}")),
        ExportTarget::Ustx,
    );
    saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
    let index = valid
        .performance_spans
        .iter()
        .position(|s| {
            s.status == verse_lib::engine::performance::TransferStatus::Mapped
                && s.intensity
                    .as_ref()
                    .is_some_and(|v| v["curve"].get("Transition").is_some())
        })
        .unwrap();
    let intensity = valid.performance_spans[index].intensity.as_ref().unwrap();
    assert_ne!(
        intensity["end"], intensity["curve"]["Transition"]["end"],
        "valid original phase fragment"
    );
    let offset = |time: &Value, units: i64| json!({"numerator":time["numerator"].as_i64().unwrap()*960 + units*time["denominator"].as_i64().unwrap(),"denominator":time["denominator"].as_i64().unwrap()*960});
    for mutation in 0..3 {
        let bad = saved_mutation(&valid, |value| {
            let v = &mut value["performanceSpans"][index]["intensity"];
            match mutation {
                0 => {
                    v["curve"]["Transition"]["start"] =
                        offset(&v["curve"]["Transition"]["start"], 960);
                    v["curve"]["Transition"]["end"] = offset(&v["curve"]["Transition"]["end"], 960);
                }
                1 => v["curve"]["Transition"]["start"] = offset(&v["start"], 1),
                _ => v["curve"]["Transition"]["end"] = offset(&v["end"], -1),
            }
        });
        assert!(
            bad.validate(&allowed).is_err(),
            "accepted ordinary transition mutation {mutation}"
        );
    }
}

fn cross_track_midi() -> Midi {
    cross_track_midi_with_recovery(false)
}

fn cross_track_midi_with_recovery(recovery: bool) -> Midi {
    let mut bytes = b"MThd\0\0\0\x06\0\x01\0\x03\x01\xe0".to_vec();
    for track in [
        &[
            0, 0xff, 5, 2, b'l', b'a', 0, 0x90, 60, 80, 0x83, 0x60, 0x80, 60, 0, 0, 0xff, 5, 2,
            b'l', b'a', 0, 0x90, 62, 80, 0x83, 0x60, 0x80, 62, 0, 0, 0xff, 0x2f, 0,
        ][..],
        if recovery {
            &[0, 0xb0, 7, 64, 0x83, 0x60, 0xb0, 7, 96, 0, 0xff, 0x2f, 0][..]
        } else {
            &[0, 0xb0, 7, 64, 0, 0xff, 0x2f, 0][..]
        },
        if recovery {
            &[0, 0xb0, 7, 32, 0, 0xff, 0x2f, 0][..]
        } else {
            &[0, 0xb1, 7, 64, 0, 0xff, 0x2f, 0][..]
        },
    ] {
        bytes.extend_from_slice(b"MTrk");
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(track);
    }
    verse_lib::engine::midi::parse(&bytes).unwrap()
}

#[test]
fn saved_mixed_velocity_controller_provenance_authenticates_every_controller() {
    let source = cross_track_midi();
    let (valid, allowed) = ledger(&source, ExportTarget::Ustx);
    saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
    let index = gain_index(&valid);
    assert!(
        !valid.performance_spans[index].intensity.as_ref().unwrap()["provenance"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let context = valid.intensity_context.as_ref().unwrap();
    let controller = context
        .controllers
        .iter()
        .find(|c| c.owner.channel == 0)
        .unwrap();
    let unrelated = context
        .controllers
        .iter()
        .find(|c| c.owner.channel == 1)
        .unwrap();
    assert_ne!(
        controller.source_track_id, valid.performance_spans[index].source_track_id,
        "same-port cross-track positive"
    );
    for mutation in 0..3 {
        let bad = saved_mutation(&valid, |v| {
            if mutation == 0 {
                replace_contributor(v, index, &controller.source_id, &unrelated.source_id);
            } else {
                let c = v["intensityContext"]["controllers"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|c| c["sourceId"] == controller.source_id)
                    .unwrap();
                if mutation == 1 {
                    c["owner"]["port"] = json!(1);
                } else {
                    c["tick"] = json!(480);
                }
            }
        });
        let error = bad.validate(&allowed).unwrap_err().to_string();
        assert!(
            error.contains("intensity controller contributor has unrelated ownership or time"),
            "{error}"
        );
    }
}

#[test]
fn saved_mixed_controller_recovery_keeps_original_historical_evidence() {
    use verse_lib::engine::performance::TransferStatus;
    let (valid, allowed) = ledger(&cross_track_midi_with_recovery(true), ExportTarget::Ustx);
    saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
    assert!(valid
        .performance_spans
        .iter()
        .any(|s| s.intensity.is_some() && s.start_tick == 0 && s.status != TransferStatus::Mapped));
    assert!(valid.performance_spans.iter().any(|s| s.intensity.is_some()
        && s.start_tick == 480
        && s.status == TransferStatus::Mapped));
}

#[test]
fn saved_tied_velocity_keeps_original_attack_root_and_repeated_new_attacks() {
    let a = "<Chord><durationType>quarter</durationType><Lyrics><no>0</no><text>hold</text></Lyrics><Lyrics><no>1</no><text>again</text></Lyrics><Note><pitch>65</pitch><Tie id=\"first\"/><veloType>user</veloType><velocity>64</velocity></Note></Chord>";
    let b = "<Chord><durationType>quarter</durationType><Lyrics><no>1</no><text>ta</text></Lyrics><Note><pitch>65</pitch><endSpanner id=\"first\"/><veloType>user</veloType><velocity>20</velocity></Note></Chord>";
    let source = musescore::parse(format!(r#"<museScore version="2.06"><programVersion>2.3.2</programVersion><Score><Division>480</Division><Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part><Staff id="1"><Measure len="2/4"><startRepeat/><Dynamic><subtype>p</subtype></Dynamic>{a}{b}<endRepeat>2</endRepeat></Measure></Staff></Score></museScore>"#).as_bytes()).unwrap();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&source, target);
        saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
        let context = valid.intensity_context.as_ref().unwrap();
        let tail = context
            .projected_notes
            .iter()
            .find(|n| n.intensity_attack_note_id.is_some())
            .expect("proven inherited attack");
        let root = tail.intensity_attack_note_id.as_ref().unwrap();
        assert_ne!(tail.note_id, *root);
        let bad = saved_mutation(&valid, |value| {
            let note = value["intensityContext"]["projectedNotes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|n| n["noteId"] == tail.note_id)
                .unwrap();
            note["intensityAttackNoteId"] = json!(tail.note_id);
        });
        assert!(bad.validate(&allowed).is_err());
    }
}

#[test]
fn saved_midi_attack_authentication_uses_velocity_not_source_id_spelling() {
    use verse_lib::engine::midi::Kind;
    let mut source = cross_track_midi();
    for track in &mut source.tracks {
        for event in &mut track.events {
            match &mut event.kind {
                Kind::NoteOn(note) => note.source.id = format!("arbitrary note at {}", event.tick),
                Kind::NoteOff(note) => {
                    note.source_id = Some(format!("arbitrary note at {}", event.tick - 480))
                }
                _ => {}
            }
        }
    }
    let (valid, allowed) = ledger(&source, ExportTarget::Ustx);
    saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
    let context = valid.intensity_context.as_ref().unwrap();
    assert!(context
        .source_notes
        .iter()
        .all(|n| n.velocity == Some(80) && n.explicit_attack));
    let bad = saved_mutation(&valid, |value| {
        value["intensityContext"]["sourceNotes"][0]["velocity"] = json!(0)
    });
    assert!(bad.validate(&allowed).is_err());
}

#[test]
fn saved_many_distinct_declarations_fit_existing_validation_limits_in_both_targets() {
    let measures: String = (0..1500)
        .map(|index| {
            let dynamic = if index % 2 == 0 { P } else { F };
            format!("<measure><attributes><divisions>480</divisions></attributes>{dynamic}{NOTE}</measure>")
        })
        .collect();
    let source = musicxml::parse(format!(r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1">{measures}</part></score-partwise>"#).as_bytes()).unwrap();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&source, target);
        assert_eq!(
            valid.intensity_context.as_ref().unwrap().declarations.len(),
            1500
        );
        assert_eq!(valid.performance_spans.len(), 1500);
        saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
    }
}

#[test]
fn saved_large_retained_inventory_preserves_one_authenticated_attack() {
    // Many original metadata items need one disposition each, but do not
    // require three duplicate source-ID trees or a large expression index.
    let mut track = Vec::new();
    for _ in 0..40_000 {
        track.extend_from_slice(&[0, 0xff, 1, 1, b'x']);
    }
    track.extend_from_slice(&[
        0, 0xff, 5, 2, b'l', b'a', 0, 0x90, 60, 80, 0x83, 0x60, 0x80, 60, 0, 0, 0xff, 0x2f, 0,
    ]);
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
    bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&track);
    let source = verse_lib::engine::midi::parse(&bytes).unwrap();
    let (valid, allowed) = ledger(&source, ExportTarget::Ustx);
    assert!(valid.entries.len() >= 40_000);
    assert_eq!(
        valid.intensity_context.as_ref().unwrap().source_notes.len(),
        1
    );
    saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
    let duplicate = saved_mutation(&valid, |value| {
        let id = value["expectedSourceIds"][0].clone();
        value["expectedSourceIds"].as_array_mut().unwrap().push(id);
    });
    assert!(duplicate.validate(&allowed).is_err());
    let reordered = saved_mutation(&valid, |value| {
        value["expectedSourceIds"].as_array_mut().unwrap().reverse();
        value["entries"].as_array_mut().unwrap().reverse();
    });
    reordered.validate(&allowed).unwrap();
}

#[test]
fn legacy_sixty_thousand_entry_inventory_has_no_expression_work_charge() {
    use verse_lib::bundle::{DispositionEntry, PrimaryDisposition, SourceItemKind};
    let ids: Vec<String> = (0..60_000)
        .map(|n| format!("original-metadata-{n}"))
        .collect();
    let entries = ids
        .iter()
        .map(|id| DispositionEntry {
            source_id: id.clone(),
            item_kind: SourceItemKind::Event,
            disposition: PrimaryDisposition::MetadataOnly,
            artifact_paths: vec!["source/score.xml".into()],
            performance_refs: vec![],
        })
        .collect();
    let mut valid = PreservationLedger {
        schema_version: 2,
        intensity_context: None,
        performance_spans: vec![],
        expected_source_ids: ids,
        entries,
    };
    let allowed = BTreeSet::from(["source/score.xml".to_string()]);
    for schema in [2, 3] {
        valid.schema_version = schema;
        let mut copy = reopened(&valid);
        copy.expected_source_ids.reverse();
        copy.entries.reverse();
        copy.validate(&allowed).unwrap();
    }
    for mutation in 0..3 {
        let mut bad = valid.clone();
        match mutation {
            0 => bad
                .expected_source_ids
                .push(bad.expected_source_ids[0].clone()),
            1 => {
                bad.entries.pop();
            }
            _ => bad.entries.push(bad.entries[0].clone()),
        }
        assert!(
            reopened(&bad).validate(&allowed).is_err(),
            "inventory mutation {mutation}"
        );
    }
}

#[test]
fn every_extra_intensity_backlink_needs_an_authenticated_contributor_role() {
    use verse_lib::bundle::{PrimaryDisposition, SourceItemKind};
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&two_parts(), target);
        reopened(&valid).validate(&allowed).unwrap();
        let index = valid
            .performance_spans
            .iter()
            .position(|s| s.intensity.is_some() && !s.note_ids.is_empty())
            .unwrap();
        let other_declaration = valid
            .intensity_context
            .as_ref()
            .unwrap()
            .declarations
            .iter()
            .find(|d| d.scope == json!({"Part":"P2"}))
            .unwrap();
        let extra: Vec<String> = [
            SourceItemKind::Note,
            SourceItemKind::Lyric,
            SourceItemKind::Track,
        ]
        .iter()
        .map(|kind| {
            valid
                .entries
                .iter()
                .find(|e| &e.item_kind == kind)
                .unwrap()
                .source_id
                .clone()
        })
        .chain([other_declaration.source_id.clone()])
        .collect();
        for id in extra {
            let mut bad = valid.clone();
            let entry = bad.entries.iter_mut().find(|e| e.source_id == id).unwrap();
            assert!(!entry.performance_refs.contains(&index));
            entry.performance_refs.push(index);
            if target == ExportTarget::Ustx {
                entry.disposition = PrimaryDisposition::ProjectedMapped {
                    policy: verse_lib::engine::score_intensity::POLICY.into(),
                    limitations: None,
                };
            }
            assert!(
                reopened(&bad).validate(&allowed).is_err(),
                "accepted extra backlink {id}"
            );
        }
    }
}

#[test]
fn later_same_route_dynamic_cannot_replace_a_held_contributor_on_any_repeat() {
    for repeated in [false, true] {
        let start = if repeated {
            r#"<barline location="left"><repeat direction="forward"/></barline>"#
        } else {
            ""
        };
        let end = if repeated {
            r#"<barline location="right"><repeat direction="backward"/></barline>"#
        } else {
            ""
        };
        let source = xml(&format!("{start}{P}{NOTE}{NOTE}{F}{NOTE}{end}"));
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let (valid, allowed) = ledger(&source, target);
            reopened(&valid).validate(&allowed).unwrap();
            let context = valid.intensity_context.as_ref().unwrap();
            let early = context
                .declarations
                .iter()
                .find(|d| d.at == json!({"numerator":0,"denominator":1}))
                .unwrap();
            let later = context
                .declarations
                .iter()
                .find(|d| d.at == json!({"numerator":2,"denominator":1}))
                .unwrap();
            let indices: Vec<usize> = valid
                .entries
                .iter()
                .find(|e| e.source_id == early.source_id)
                .unwrap()
                .performance_refs
                .iter()
                .copied()
                .filter(|i| !valid.performance_spans[*i].note_ids.is_empty())
                .collect();
            assert!(indices.len() >= if repeated { 4 } else { 2 });
            for index in indices {
                let bad = saved_mutation(&valid, |value| {
                    replace_contributor(value, index, &early.source_id, &later.source_id)
                });
                assert!(
                    bad.validate(&allowed).is_err(),
                    "accepted future dynamic: {target:?}, repeat={repeated}, span={index}"
                );
            }
        }
    }
}

#[test]
fn future_transition_endpoint_cannot_escape_its_source_resolved_interval() {
    let ramp_start = r#"<direction><direction-type><wedge type="crescendo" number="8"/></direction-type></direction>"#;
    let ramp_end = r#"<direction><direction-type><wedge type="stop" number="8"/><dynamics><f/></dynamics></direction-type></direction>"#;
    let source = xml(&format!("{P}{NOTE}{ramp_start}{NOTE}{ramp_end}{NOTE}"));
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&source, target);
        reopened(&valid).validate(&allowed).unwrap();
        let context = valid.intensity_context.as_ref().unwrap();
        let early = context
            .declarations
            .iter()
            .find(|d| d.at == json!({"numerator":0,"denominator":1}))
            .unwrap();
        let future = context
            .declarations
            .iter()
            .find(|d| {
                d.at == json!({"numerator":2,"denominator":1})
                    && d.kinds
                        .as_array()
                        .is_some_and(|kinds| kinds.iter().any(|kind| kind == "Dynamic"))
            })
            .unwrap();
        assert!(context.dependencies.iter().any(|dependency| {
            dependency.source_ids.contains(&future.source_id)
                && dependency.attack_note_id.is_none()
                && dependency.start == json!({"numerator":1,"denominator":1})
        }));
        let index = valid
            .entries
            .iter()
            .find(|entry| entry.source_id == early.source_id)
            .unwrap()
            .performance_refs
            .iter()
            .copied()
            .find(|index| {
                valid.performance_spans[*index].start_tick == 0
                    && !valid.performance_spans[*index].note_ids.is_empty()
            })
            .unwrap();
        let bad = saved_mutation(&valid, |value| {
            replace_contributor(value, index, &early.source_id, &future.source_id)
        });
        assert!(
            bad.validate(&allowed).is_err(),
            "accepted future endpoint outside its source-resolved interval: {target:?}"
        );
    }
}

#[test]
fn earlier_same_route_dynamic_cannot_replace_a_later_contributor_on_any_repeat() {
    for repeated in [false, true] {
        let start = if repeated {
            r#"<barline location="left"><repeat direction="forward"/></barline>"#
        } else {
            ""
        };
        let end = if repeated {
            r#"<barline location="right"><repeat direction="backward"/></barline>"#
        } else {
            ""
        };
        let source = xml(&format!("{start}{P}{NOTE}{NOTE}{F}{NOTE}{NOTE}{end}"));
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let (valid, allowed) = ledger(&source, target);
            reopened(&valid).validate(&allowed).unwrap();
            let context = valid.intensity_context.as_ref().unwrap();
            let early = context
                .declarations
                .iter()
                .find(|d| d.at == json!({"numerator":0,"denominator":1}))
                .unwrap();
            let later = context
                .declarations
                .iter()
                .find(|d| d.at == json!({"numerator":2,"denominator":1}))
                .unwrap();
            let indices: Vec<usize> = valid
                .entries
                .iter()
                .find(|e| e.source_id == later.source_id)
                .unwrap()
                .performance_refs
                .iter()
                .copied()
                .filter(|i| !valid.performance_spans[*i].note_ids.is_empty())
                .collect();
            assert!(!indices.is_empty());
            for index in indices {
                let bad = saved_mutation(&valid, |value| {
                    replace_contributor(value, index, &later.source_id, &early.source_id)
                });
                assert!(
                    bad.validate(&allowed).is_err(),
                    "accepted superseded dynamic: {target:?}, repeat={repeated}, span={index}"
                );
            }
        }
    }
}

#[test]
fn invalid_unused_dependency_rows_are_rejected() {
    let source =
        musicxml::parse(include_bytes!("score-intensity-fixtures/contour.musicxml")).unwrap();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&source, target);
        reopened(&valid).validate(&allowed).unwrap();
        let template = valid
            .intensity_context
            .as_ref()
            .unwrap()
            .dependencies
            .iter()
            .find(|d| d.attack_note_id.is_none())
            .unwrap()
            .clone();
        for mutation in 0..3 {
            let mut bad = valid.clone();
            let context = bad.intensity_context.as_mut().unwrap();
            let mut dependency = template.clone();
            match mutation {
                0 => {
                    dependency.occurrence = dependency.occurrence.saturating_add(99);
                    dependency.repeat_pass = dependency.repeat_pass.saturating_add(99);
                }
                1 => {
                    dependency.start = json!({"numerator":9999,"denominator":1});
                    dependency.end = json!({"numerator":10000,"denominator":1});
                }
                _ => dependency.source_ids.push(dependency.source_ids[0].clone()),
            }
            context.dependencies.push(dependency);
            assert!(
                reopened(&bad).validate(&allowed).is_err(),
                "accepted invalid unused dependency {mutation}: {target:?}"
            );
        }
    }
}

#[test]
fn authored_svp_segments_cannot_disappear_behind_attack_summary() {
    let (valid, allowed) = ledger(&xml(&format!("{P}{NOTE}")), ExportTarget::Svp);
    reopened(&valid).validate(&allowed).unwrap();
    let index = valid
        .performance_spans
        .iter()
        .position(|span| {
            span.intensity
                .as_ref()
                .and_then(|value| value.get("segments"))
                .and_then(Value::as_array)
                .is_some_and(|segments| !segments.is_empty())
        })
        .unwrap();
    let bad = saved_mutation(&valid, |value| {
        value["performanceSpans"][index]["intensity"]["segments"] = json!([]);
    });
    assert!(
        bad.validate(&allowed).is_err(),
        "an authored score-intensity segment must not disappear behind its attack summary"
    );
}

#[test]
fn authored_svp_segment_and_matching_summary_cannot_disappear_together() {
    let source =
        musicxml::parse(include_bytes!("score-intensity-fixtures/contour.musicxml")).unwrap();
    let (valid, allowed) = ledger(&source, ExportTarget::Svp);
    reopened(&valid).validate(&allowed).unwrap();
    let index = valid
        .performance_spans
        .iter()
        .position(|span| {
            span.intensity
                .as_ref()
                .and_then(|value| value.get("segments"))
                .and_then(Value::as_array)
                .is_some_and(|segments| {
                    segments
                        .iter()
                        .any(|segment| !segment["provenance"].is_null())
                })
        })
        .unwrap();
    let bad = saved_mutation(&valid, |value| {
        let intensity = &mut value["performanceSpans"][index]["intensity"];
        let removed = intensity["segments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|segment| !segment["provenance"].is_null())
            .unwrap()["provenance"]
            .clone();
        intensity["segments"]
            .as_array_mut()
            .unwrap()
            .retain(|segment| segment["provenance"] != removed);
        intensity["provenance"]
            .as_array_mut()
            .unwrap()
            .retain(|provenance| *provenance != removed);
    });
    assert!(
        bad.validate(&allowed).is_err(),
        "an authored dependency must remain detectable when its segment and summary are removed together"
    );
}

#[test]
fn merged_repeat_exit_svp_note_authenticates_each_original_segment() {
    let head = NOTE.replace("<lyric>", "<tie type=\"start\"/><lyric>");
    let tail = "<note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><tie type=\"stop\"/></note>";
    let source = musicxml::parse(format!(r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions><time><beats>2</beats><beat-type>4</beat-type></time></attributes><barline location="left"><repeat direction="forward"/></barline>{P}{NOTE}{head}<barline location="right"><repeat direction="backward"/></barline></measure><measure><barline location="left"><repeat direction="forward"/></barline>{F}{tail}{NOTE}<barline location="right"><repeat direction="backward"/></barline></measure></part></score-partwise>"#).as_bytes()).unwrap();
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        let (valid, allowed) = ledger(&source, target);
        let spans: Vec<_> = valid
            .performance_spans
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.start_tick == 1440
                    && s.end_tick == 2400
                    && s.intensity
                        .as_ref()
                        .is_some_and(|e| e.get("segments").is_some())
            })
            .collect();
        if target == ExportTarget::Svp {
            assert_eq!(
                spans.len(),
                1,
                "actual loader must merge the [3,4) head with its [4,5) tail"
            );
            let (index, span) = spans[0];
            let segments = span.intensity.as_ref().unwrap()["segments"]
                .as_array()
                .unwrap();
            assert_eq!(segments.len(), 2);
            assert_ne!(
                segments[0]["provenance"]["occurrence"],
                segments[1]["provenance"]["occurrence"]
            );
            reopened(&valid).validate(&allowed).unwrap();
            let bad = saved_mutation(&valid, |value| {
                let original = value["performanceSpans"][index]["intensity"]["segments"][0]
                    ["provenance"]
                    .clone();
                value["performanceSpans"][index]["intensity"]["segments"][1]["provenance"] =
                    original;
            });
            assert!(
                bad.validate(&allowed).is_err(),
                "an earlier occurrence cannot own the later segment"
            );
        } else {
            reopened(&valid).validate(&allowed).unwrap();
        }
    }
}

#[test]
fn saved_pass_filtered_endpoint_diagnostic_preserves_strict_active_applicability() {
    use verse_lib::engine::performance::TransferStatus;
    let source = xml(&format!(
        r#"<barline location="left"><repeat direction="forward"/></barline>{P}<direction><direction-type><wedge type="crescendo" number="6"/></direction-type></direction>{NOTE}{NOTE}<direction><direction-type><wedge type="stop" number="6"/></direction-type><sound time-only="2"/></direction><barline location="right"><repeat direction="backward"/></barline>"#
    ));
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let (valid, allowed) = ledger(&source, target);
        saved_mutation(&valid, |_| {}).validate(&allowed).unwrap();
        let stop = valid
            .intensity_context
            .as_ref()
            .unwrap()
            .declarations
            .iter()
            .find(|d| {
                d.kinds
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|kind| kind == "WedgeStop")
            })
            .unwrap();
        assert!(stop
            .applications
            .iter()
            .any(|a| a.repeat_pass == 1 && !a.active));
        assert!(stop
            .applications
            .iter()
            .any(|a| a.repeat_pass == 2 && a.active));
        let diagnostic = valid
            .performance_spans
            .iter()
            .position(|span| {
                span.status == TransferStatus::Unsupported
                    && span
                        .intensity
                        .as_ref()
                        .is_some_and(|v| v["provenance"]["repeat_pass"] == 1)
                    && span.detail.contains("time-only")
            })
            .expect("excluded first-pass diagnostic remains explicit");
        let bad = saved_mutation(&valid, |value| {
            value["performanceSpans"][diagnostic]["intensity"]["provenance"]["repeat_pass"] =
                json!(2);
            value["performanceSpans"][diagnostic]["intensity"]["provenance"]["occurrence"] =
                json!(2);
        });
        assert!(bad.validate(&allowed).is_err());
        // Even a symmetrically removed relation cannot turn unrelated raw
        // endpoints into legitimate combined evidence.
        let bad = saved_mutation(&valid, |value| {
            for declaration in value["intensityContext"]["declarations"]
                .as_array_mut()
                .unwrap()
            {
                declaration["pairedSourceIds"] = json!([]);
            }
        });
        assert!(bad.validate(&allowed).is_err());
        if target == ExportTarget::Ustx {
            let bad = saved_mutation(&valid, |value| {
                let declaration = value["intensityContext"]["declarations"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|d| d["sourceId"] == stop.source_id)
                    .unwrap();
                for application in declaration["applications"].as_array_mut().unwrap() {
                    application["active"] = json!(false);
                }
            });
            assert!(
                bad.validate(&allowed).is_err(),
                "mapped endpoint must remain active"
            );
        }
    }
}
