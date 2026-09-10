//! Actual selected-row tie recovery must preserve the head attack, not tail velocity.
use verse_lib::engine::{
    convert::convert_midi_with_target,
    midi::Kind,
    musescore,
    performance::{self, PerformanceOwner, TransferStatus},
    projection::ProjectedLyric,
    score_intensity::IssueKind,
    target::{self, ustx, ExportTarget},
};

fn lyric(row: u8, text: &str) -> String {
    format!("<Lyrics><no>{row}</no><text>{text}</text></Lyrics>")
}
fn chord(lyrics: &str, links: &str, velocity: Option<u8>) -> String {
    let velocity = velocity
        .map(|v| format!("<veloType>user</veloType><velocity>{v}</velocity>"))
        .unwrap_or_default();
    format!("<Chord><durationType>quarter</durationType>{lyrics}<Note><pitch>65</pitch>{links}{velocity}</Note></Chord>")
}
fn source(head: Option<u8>, tail: Option<u8>, dynamics: bool) -> String {
    let a = chord(
        &format!("{}{}", lyric(0, "hold"), lyric(1, "again")),
        r#"<Tie id="first"/>"#,
        head,
    );
    let b = chord(
        &lyric(1, "ta"),
        r#"<endSpanner id="first"/><Tie id="second"/>"#,
        tail,
    );
    let c = chord(&lyric(1, "il"), r#"<endSpanner id="second"/>"#, tail);
    let d = chord(&format!("{}{}", lyric(0, "end"), lyric(1, "end")), "", None);
    let p = if dynamics {
        "<Dynamic><subtype>p</subtype></Dynamic>"
    } else {
        ""
    };
    let f = if dynamics {
        "<Dynamic><subtype>f</subtype></Dynamic>"
    } else {
        ""
    };
    format!(
        r#"<museScore version="2.06"><programVersion>2.3.2</programVersion><Score><Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part>
    <Staff id="1"><Measure len="4/4"><startRepeat/>{p}{a}{f}{b}{c}{d}<endRepeat>2</endRepeat></Measure></Staff></Score></museScore>"#
    )
}
fn dyn_at(project: &ustx::UstxProject, tick: i32) -> i32 {
    let curve = project.voice_parts[0]
        .curves
        .iter()
        .find(|c| c.abbr == "dyn")
        .unwrap();
    match curve.xs.binary_search(&tick) {
        Ok(i) => curve.ys[i],
        Err(i) if i > 0 && i < curve.xs.len() => {
            let f = f64::from(tick - curve.xs[i - 1]) / f64::from(curve.xs[i] - curve.xs[i - 1]);
            (f64::from(curve.ys[i - 1]) + f * f64::from(curve.ys[i] - curve.ys[i - 1]))
                .round_ties_even() as i32
        }
        _ => 0,
    }
}

#[test]
fn recovered_tails_do_not_consume_next_attack_accents() {
    for ordinary_context in [true, false] {
        let mut xml =
            source(None, None, true).replace("<subtype>f</subtype>", "<subtype>sfz</subtype>");
        if !ordinary_context {
            xml = xml.replace("<Dynamic><subtype>p</subtype></Dynamic>", "");
        }
        let midi = musescore::parse(xml.as_bytes()).unwrap();
        let outcome = convert_midi_with_target(&midi, "english", None, ExportTarget::Ustx);
        assert!(outcome.ok, "{:?}", outcome.msg);
        let project = outcome.svp.unwrap();
        let saved = ustx::serialize(&project).unwrap();
        let held = if ordinary_context { -78 } else { 0 };
        assert_eq!(
            [120, 600, 1080, 1560].map(|t| dyn_at(&saved, t)),
            [held, held, held, 80]
        );
        // On the repeated pass the written syllable at the tail is an attack.
        assert_eq!(
            [2040, 2520, 3000, 3480].map(|t| dyn_at(&saved, t)),
            [held, 80, held, held]
        );
        for i in [1, 2] {
            assert!(matches!(
                project.tracks[0].notes[i].lyric,
                ProjectedLyric::Extension
            ));
        }
    }
}
#[test]
fn actual_legacy_chain_retains_tail_identity_and_uses_one_head_attack() {
    for (head, tail, dynamics, expected) in [
        (Some(64), Some(20), true, [-40, 78, 78, -150]),
        (None, Some(20), true, [-78, 40, 40, -150]),
        (None, Some(20), false, [0, 0, 0, -150]),
        (Some(64), None, true, [-40, 78, 78, 40]),
    ] {
        let xml = source(head, tail, dynamics);
        let midi = musescore::parse(xml.as_bytes()).unwrap();
        let before = format!("{midi:?}");
        let raw = performance::normalize(&midi).unwrap();
        let outcome = convert_midi_with_target(&midi, "english", None, ExportTarget::Ustx);
        assert!(outcome.ok, "{:?}", outcome.msg);
        let project = outcome.svp.as_ref().unwrap();
        assert_eq!(project.tracks.len(), 1);
        let notes = &project.tracks[0].notes;
        assert_eq!(notes.len(), 8);
        let saved = ustx::serialize(project).unwrap();
        assert_eq!(
            target::serialize_to(ExportTarget::Ustx, project).unwrap(),
            ustx::to_yaml(&saved).as_bytes()
        );
        assert_eq!(
            [120, 600, 1080, 2520].map(|t| dyn_at(&saved, t)),
            expected,
            "head={head:?}, tail={tail:?}, dynamics={dynamics}"
        );
        let root = notes[0]
            .performance
            .as_ref()
            .unwrap()
            .intensity
            .as_ref()
            .unwrap();
        for i in [1, 2] {
            let note = &notes[i];
            assert!(matches!(note.lyric, ProjectedLyric::Extension));
            let evidence = note.source_evidence.as_ref().unwrap();
            let origin = evidence.origin.as_ref().unwrap();
            let binding = note.performance.as_ref().unwrap();
            let original = &raw.bindings[&(origin.track_id.clone(), origin.note_on_order)];
            assert_eq!(binding.source_id, evidence.note_id);
            assert_eq!(binding.source_id, original.source_id);
            assert_eq!(binding.owner, original.owner);
            assert!(matches!(binding.owner, PerformanceOwner::Score { .. }));
            let inherited = binding.intensity.as_ref().unwrap();
            assert_eq!(inherited.source_id, root.source_id);
            assert_eq!(inherited.start, root.start);
            assert!(std::sync::Arc::ptr_eq(&inherited.timeline, &root.timeline));
            assert_eq!(saved.voice_parts[0].notes[i].lyric, "+~");
            assert_eq!(note.onset_ticks, (i * 480) as u32);
            assert_eq!(note.duration_ticks, 480);
            if tail.is_some() {
                assert!(inherited.issues.iter().any(|i| matches!(
                    i.kind,
                    IssueKind::Conflict | IssueKind::ContinuationVelocity
                )));
                assert!(outcome
                    .projection
                    .performance_spans
                    .iter()
                    .any(|r| r.note_ids.contains(&evidence.note_id)
                        && r.status == TransferStatus::Unsupported));
            }
        }
        // Repeat pass two selects the written tail syllables: they remain attacks.
        for i in [5, 6] {
            let n = &notes[i];
            let o = n.source_evidence.as_ref().unwrap().origin.as_ref().unwrap();
            assert!(matches!(&n.lyric,ProjectedLyric::Source(l) if !l.raw.is_empty()));
            assert!(o.continuation.is_none());
            assert_eq!(
                n.performance.as_ref(),
                raw.bindings.get(&(o.track_id.clone(), o.note_on_order))
            );
        }
        for note in notes {
            let e = note.source_evidence.as_ref().unwrap();
            let o = e.origin.as_ref().unwrap();
            let event = midi
                .tracks
                .iter()
                .find(|t| t.id == o.track_id)
                .unwrap()
                .events
                .iter()
                .find(|e| e.order == o.note_on_order)
                .unwrap();
            let Kind::NoteOn(source) = &event.kind else {
                panic!("original NoteOn")
            };
            assert_eq!(o.source, source.source);
            assert_eq!(note.pitch, source.key.unwrap());
        }
        assert_eq!(format!("{midi:?}"), before);
    }
}
#[test]
fn unexpressed_tie_context_never_fabricates_a_curve_or_mapped_expression() {
    let midi = musescore::parse(source(None, None, false).as_bytes()).unwrap();
    let out = convert_midi_with_target(&midi, "english", None, ExportTarget::Ustx);
    assert!(out.ok, "{:?}", out.msg);
    let p = out.svp.as_ref().unwrap();
    assert!(matches!(
        p.tracks[0].notes[1].lyric,
        ProjectedLyric::Extension
    ));
    assert!(out.projection.performance_spans.is_empty());
    assert!(ustx::serialize(p).unwrap().voice_parts[0].curves.is_empty());
}
