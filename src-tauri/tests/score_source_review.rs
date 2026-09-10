//! Source review regressions through actual score loading, conversion and saved ledgers.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use verse_lib::{
    bundle::{build_preservation_ledger, BundleLayout, PreservationLedger, PrimaryDisposition},
    engine::{
        convert::convert_midi_with_target,
        midi, musescore, musicxml, performance,
        performance::TransferStatus,
        score_intensity::{
            Basis, Curve, Evaluation, Fraction, Instruction, Intensity, IssueKind, Level, Scope,
            Timeline,
        },
        target::{self, ustx, ExportTarget},
    },
    stems::StemPlan,
};

fn at(ticks: i64) -> Fraction {
    Fraction::new(ticks, 480).unwrap()
}
fn xml(measures: &str) -> String {
    format!(
        r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1">{measures}</part></score-partwise>"#
    )
}
fn measure(body: &str) -> String {
    format!("<measure><attributes><divisions>480</divisions></attributes>{body}</measure>")
}
fn note(duration: u32) -> String {
    format!("<note><pitch><step>C</step><octave>4</octave></pitch><duration>{duration}</duration><lyric><text>la</text></lyric></note>")
}
fn dynamic(mark: &str, tick: i32) -> String {
    format!(
        r#"<direction><direction-type><dynamics><{mark}/></dynamics></direction-type><offset sound="yes">{tick}</offset></direction>"#
    )
}
fn wedge(kind: &str, tick: i32, marker: &str, words: &str, extra: &str) -> String {
    format!(
        r#"<direction id="{marker}"><direction-type><wedge type="{kind}" number="1"/>{words}</direction-type><offset sound="yes">{tick}</offset>{extra}</direction>"#
    )
}
fn timeline(source: &midi::Midi) -> Arc<Timeline> {
    performance::normalize(source)
        .unwrap()
        .bindings
        .values()
        .find_map(|binding| binding.intensity.as_ref().map(|i| i.timeline.clone()))
        .expect("source intensity timeline")
}
fn assert_db(timeline: &Timeline, tick: i64, expected: f64) {
    let Evaluation::Known(Intensity::Decibels(actual)) = timeline.evaluate(at(tick)) else {
        panic!(
            "expected finite intensity at {tick}: {:?}",
            timeline.evaluate(at(tick))
        );
    };
    assert!(
        (actual - expected).abs() < 1e-8,
        "at {tick}: {actual} != {expected}"
    );
}
fn exported(source: &midi::Midi) -> ustx::UstxProject {
    let outcome = convert_midi_with_target(source, "english", None, ExportTarget::Ustx);
    assert!(outcome.ok, "{:?}", outcome.msg);
    ustx::serialize(outcome.svp.as_ref().unwrap()).unwrap()
}
fn dyn_at(project: &ustx::UstxProject, tick: i32) -> f64 {
    let curve = project.voice_parts[0]
        .curves
        .iter()
        .find(|c| c.abbr == "dyn")
        .expect("active exported DYN");
    match curve.xs.binary_search(&tick) {
        Ok(i) => f64::from(curve.ys[i]),
        Err(i) if i > 0 && i < curve.xs.len() => {
            let phase =
                f64::from(tick - curve.xs[i - 1]) / f64::from(curve.xs[i] - curve.xs[i - 1]);
            f64::from(curve.ys[i - 1]) + phase * f64::from(curve.ys[i] - curve.ys[i - 1])
        }
        position => panic!("missing DYN at {tick}: {position:?}"),
    }
}

fn saved_ledger(source: &midi::Midi) -> PreservationLedger {
    saved_ledger_for(source, ExportTarget::Ustx)
}
fn saved_ledger_for(source: &midi::Midi, target: ExportTarget) -> PreservationLedger {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let outcome = convert_midi_with_target(source, "english", None, target);
    assert!(outcome.ok, "{:?}", outcome.msg);
    assert!(!target::serialize_to(target, outcome.svp.as_ref().unwrap())
        .unwrap()
        .is_empty());
    let path = std::env::temp_dir().join(format!(
        "verse-score-source-review-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let layout =
        BundleLayout::new(&path.with_extension("versebundle"), "score.xml", target).unwrap();
    let stems = StemPlan::from_source(source, &outcome.tracks).unwrap();
    let ledger = build_preservation_ledger(source, &outcome.projection, &layout, &stems);
    std::fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let saved: PreservationLedger = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved, ledger);
    let allowed = saved
        .entries
        .iter()
        .flat_map(|entry| entry.artifact_paths.iter().cloned())
        .collect();
    saved.validate(&allowed).unwrap();
    saved
}
fn source_id(source: &midi::Midi, marker: &str) -> String {
    let fragment = format!(r#"id="{marker}""#);
    source
        .score_intensity
        .as_ref()
        .unwrap()
        .retained
        .iter()
        .find(|(_, evidence)| {
            evidence
                .raw_fields
                .get("xml")
                .is_some_and(|raw| raw.contains(&fragment))
        })
        .map(|(id, _)| id.clone())
        .expect("original declaration identity")
}
fn assert_ledger_mapping(ledger: &PreservationLedger, id: &str, mapped: bool) {
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.source_id == id)
        .expect("retained ledger entry");
    assert_eq!(
        matches!(
            entry.disposition,
            PrimaryDisposition::ProjectedMapped { .. }
        ),
        mapped,
        "{entry:?}"
    );
    assert_eq!(
        entry
            .performance_refs
            .iter()
            .any(|index| ledger.performance_spans[*index].status == TransferStatus::Mapped),
        mapped,
        "{entry:?}"
    );
}

fn endings(opening: &str, second: &str) -> String {
    xml(&format!(
        "{}{}{}{}{}",
        measure(&format!(
            r#"<barline location="left"><repeat direction="forward"/></barline>{opening}{}"#,
            note(480)
        )),
        measure(&format!(
            r#"<barline location="left"><ending number="1" type="start"/></barline>{}{}<barline location="right"><ending number="1" type="stop"/><repeat direction="backward"/></barline>"#,
            dynamic("f", 0),
            note(480)
        )),
        measure(&format!(
            r#"<barline location="left"><ending number="2" type="start"/></barline>{second}{}<barline location="right"><ending number="2" type="discontinue"/></barline>"#,
            note(480)
        )),
        measure("<note><rest/><duration>480</duration></note>"),
        measure(&note(480))
    ))
}

#[test]
fn second_ending_first_occurrence_uses_pass_two_numeric_dyn() {
    let source = musicxml::parse(endings(&dynamic("p", 0), r#"<direction><direction-type><dynamics><pp/></dynamics></direction-type><sound dynamics="100" time-only="2"/></direction>"#).as_bytes()).unwrap();
    let second = source.tracks[0]
        .events
        .iter()
        .find_map(|event| match &event.kind {
            midi::Kind::NoteOn(note) if note.source.measure == Some(2) => {
                Some((event.tick, note.source.occurrence))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        second,
        (1440, 0),
        "first source occurrence of the second ending"
    );
    let input = source.score_intensity.as_ref().unwrap();
    assert!(input
        .runs
        .values()
        .flatten()
        .any(|run| run.written_start == at(960)
            && run.performed_start == at(1440)
            && run.pass == 2));
    let curve = timeline(&source);
    assert_db(&curve, 1440, 2.5);
    assert_eq!(
        curve
            .segment_at(at(1440))
            .unwrap()
            .provenance
            .as_ref()
            .unwrap()
            .repeat_pass,
        2
    );
    let target = exported(&source);
    assert_eq!(
        [0, 480, 960, 1440, 2400].map(|tick| dyn_at(&target, tick)),
        [-78.0, 40.0, -78.0, 25.0, 25.0]
    );
}

#[test]
fn skipped_first_ending_cannot_replace_held_state_through_forward_rests() {
    let source = musicxml::parse(endings(&dynamic("p", 0), "").as_bytes()).unwrap();
    let curve = timeline(&source);
    for tick in [1440, 1920, 2400] {
        assert_db(&curve, tick, -7.75);
    }
    let provenance = curve
        .segment_at(at(1440))
        .unwrap()
        .provenance
        .as_ref()
        .unwrap();
    assert!(!provenance
        .evidence
        .iter()
        .flat_map(|e| &e.source_ids)
        .any(|id| id.contains("measure:1:")));
    let target = exported(&source);
    assert_eq!(
        [0, 480, 960, 1440, 2400].map(|tick| dyn_at(&target, tick)),
        [-78.0, 40.0, -78.0, -78.0, -78.0]
    );
}

#[test]
fn skipped_first_ending_cannot_supply_a_ramp_endpoint_at_the_jump_boundary() {
    let opening = format!(
        "{}{}{}",
        dynamic("p", 0),
        wedge("crescendo", 0, "start", "", ""),
        wedge("stop", 480, "stop", "", "")
    );
    let source = musicxml::parse(endings(&opening, "").as_bytes()).unwrap();
    let input = source.score_intensity.as_ref().unwrap();
    let owner = verse_lib::engine::score_intensity::ScoreVoice {
        part: "P1".into(),
        staff: "1".into(),
        voice: "1".into(),
        instrument: None,
    };
    let stop_id = source_id(&source, "stop");
    let skipped_id = input
        .score
        .events
        .iter()
        .find_map(|event| {
            matches!(&event.instruction, Instruction::Dynamic(dynamic)
            if dynamic.symbol.as_deref() == Some("f"))
            .then(|| &event.evidence.source_ids[0])
        })
        .unwrap();
    assert_eq!(
        input.declarations[&stop_id].at,
        input.declarations[skipped_id].at
    );
    for run in input
        .owner_runs(&owner)
        .unwrap()
        .iter()
        .filter(|run| run.pass == 2)
    {
        assert_eq!(
            input.declaration_route_pass(&stop_id, &owner, run.ordinal, run.pass),
            Some(2)
        );
        assert_eq!(
            input.declaration_route_pass(skipped_id, &owner, run.ordinal, run.pass),
            None
        );
    }
    let curve = timeline(&source);
    assert_db(&curve, 240, -1.875);
    assert_db(&curve, 1200, -5.875);
    assert_db(&curve, 1440, -4.0);
    let target = exported(&source);
    assert_eq!(dyn_at(&target, 240), -19.0);
    assert_eq!(dyn_at(&target, 1200), -59.0);
    assert_eq!(dyn_at(&target, 1440), -40.0);

    // A following dynamic in the second ending must not retroactively change
    // the endpoint already reached before the skip.
    let source = musicxml::parse(endings(&opening, &dynamic("ff", 240)).as_bytes()).unwrap();
    let curve = timeline(&source);
    assert_db(&curve, 1440, -4.0);
    assert_db(&curve, 1680, 8.0);
    let target = exported(&source);
    assert_eq!(dyn_at(&target, 1440), -40.0);
    assert_eq!(dyn_at(&target, 1680), 80.0);
}

#[test]
fn unsupported_direction_preserves_valid_position_scope_order_and_pass() {
    for (fields, tick, reason) in [
        (
            r#"<offset sound="yes">240</offset><sound dynamics="bad-level" time-only="2"/>"#,
            240,
            "bad-level",
        ),
        (
            r#"<offset sound="yes">240</offset><sound dynamics="bad-level" time-only="2"><offset>360</offset></sound>"#,
            360,
            "bad-level",
        ),
        (
            r#"<offset>visual-only</offset><sound dynamics="bad-level" time-only="2"><offset>240</offset></sound>"#,
            240,
            "bad-level",
        ),
        (
            r#"<offset sound="yes">bad-offset</offset><sound dynamics="100" time-only="2"/>"#,
            0,
            "bad-offset",
        ),
    ] {
        let document = xml(&measure(&format!(
            r#"{}<direction id="invalid"><staff>1</staff><voice>1</voice>{fields}</direction>{}<barline location="right"><repeat direction="backward"/></barline>"#,
            dynamic("p", 0),
            note(960)
        )));
        let source = musicxml::parse(document.as_bytes()).unwrap();
        let event = source
            .score_intensity
            .as_ref()
            .unwrap()
            .score
            .events
            .iter()
            .find(|e| matches!(e.instruction, Instruction::Unsupported(_)))
            .unwrap();
        assert_eq!(event.at, at(tick));
        assert_eq!(event.time_only, [2]);
        assert_eq!(event.scope, Scope::musicxml("P1", Some("1"), Some("1")));
        let parsed = roxmltree::Document::parse(&document).unwrap();
        let node = parsed
            .descendants()
            .find(|node| node.attribute("id") == Some("invalid"))
            .unwrap();
        assert_eq!(event.order, node.id().get());
        assert!(
            matches!(&event.instruction, Instruction::Unsupported(message) if message.contains(reason))
        );
        let curve = timeline(&source);
        assert!(curve.issues.iter().any(|issue| issue.start == at(tick)
            && issue.kind == IssueKind::PassFiltered
            && issue.provenance.repeat_pass == 1));
        assert!(curve
            .issues
            .iter()
            .any(|issue| issue.start == at(960 + tick)
                && issue.end == issue.start
                && issue.kind == IssueKind::UnsupportedMark
                && issue.provenance.repeat_pass == 2
                && issue.message.contains(reason)));
        assert_db(&curve, 1440, -7.75);
    }
}

#[test]
fn unsupported_wedge_keeps_position_even_when_pass_syntax_is_invalid() {
    let document = xml(&measure(&format!(
        r#"{}<direction id="invalid"><direction-type><wedge type="unknown"/></direction-type><offset sound="yes">240</offset><staff>1</staff><sound time-only="bad-pass"/></direction>{}"#,
        dynamic("p", 0),
        note(960)
    )));
    let source = musicxml::parse(document.as_bytes()).unwrap();
    let event = source
        .score_intensity
        .as_ref()
        .unwrap()
        .score
        .events
        .iter()
        .find(|e| matches!(e.instruction, Instruction::Unsupported(_)))
        .unwrap();
    assert_eq!(event.at, at(240));
    assert_eq!(event.scope, Scope::musicxml("P1", Some("1"), None));
    assert!(
        matches!(&event.instruction, Instruction::Unsupported(message) if message.contains("bad-pass"))
    );
}

#[test]
fn incompatible_owning_wedge_words_are_local_conflicts_with_saved_evidence() {
    for (kind, words, on_stop) in [
        ("crescendo", "<words>dim.</words>", false),
        ("crescendo", "<words>fade out</words>", false),
        ("diminuendo", "<words>fade in</words>", false),
        (
            "crescendo",
            "<words>cresc.</words><words>morendo</words>",
            false,
        ),
        ("crescendo", "<words>dim.</words>", true),
    ] {
        let document = xml(&measure(&format!(
            "{}{}{}{}",
            dynamic("p", 0),
            wedge(kind, 480, "start", if on_stop { "" } else { words }, ""),
            wedge("stop", 960, "stop", if on_stop { words } else { "" }, ""),
            note(1440)
        )));
        let source = musicxml::parse(document.as_bytes()).unwrap();
        let curve = timeline(&source);
        assert_eq!(curve.evaluate(at(720)), Evaluation::Unknown);
        assert_db(&curve, 240, -7.75);
        assert_db(&curve, 1200, -7.75);
        let issue = curve
            .issues
            .iter()
            .find(|i| i.kind == IssueKind::Conflict)
            .unwrap();
        assert_eq!((issue.start, issue.end), (at(480), at(960)));
        let raw: String = issue
            .provenance
            .evidence
            .iter()
            .flat_map(|e| e.raw_fields.values())
            .map(String::as_str)
            .collect();
        assert!(raw.contains(kind) && raw.contains(words));
        let events = &source.score_intensity.as_ref().unwrap().score.events;
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.instruction, Instruction::Transition(_)))
                .count(),
            1
        );
        assert!(!events
            .iter()
            .any(|e| matches!(e.instruction, Instruction::Text { .. })));
        let target = exported(&source);
        assert_eq!(dyn_at(&target, 240), -78.0);
        assert_eq!(dyn_at(&target, 1200), -78.0);
        let ledger = saved_ledger(&source);
        assert_ledger_mapping(&ledger, &source_id(&source, "start"), false);
        assert_ledger_mapping(&ledger, &source_id(&source, "stop"), false);
    }
}

#[test]
fn matching_wedge_words_coalesce_and_unknown_words_are_not_guessed() {
    for (words, unknown) in [("cresc.", false), ("please fade out soon", true)] {
        let source = musicxml::parse(
            xml(&measure(&format!(
                "{}{}{}{}",
                dynamic("p", 0),
                wedge(
                    "crescendo",
                    480,
                    "start",
                    &format!("<words>{words}</words>"),
                    ""
                ),
                wedge("stop", 960, "stop", "", ""),
                note(1440)
            )))
            .as_bytes(),
        )
        .unwrap();
        let curve = timeline(&source);
        assert_db(&curve, 720, -5.875);
        assert_db(&curve, 1200, -4.0);
        assert!(!curve
            .issues
            .iter()
            .any(|i| matches!(i.kind, IssueKind::Conflict | IssueKind::InferredSpan)));
        assert_eq!(
            curve
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::UnsupportedMark),
            unknown
        );
        assert_eq!(dyn_at(&exported(&source), 720), -59.0);
    }
}

#[test]
fn pass_excluded_wedge_starts_never_appear_in_mapped_saved_ledger_spans() {
    for (second_kind, pass_filter, mapped) in [
        ("crescendo", "", true),
        ("crescendo", r#"<sound time-only="2"/>"#, false),
        ("diminuendo", r#"<sound time-only="2"/>"#, false),
    ] {
        for reverse in [false, true] {
            let active = wedge("crescendo", 480, "active", "", "");
            let other = wedge(second_kind, 480, "other", "", pass_filter);
            let starts = if reverse {
                format!("{other}{active}")
            } else {
                format!("{active}{other}")
            };
            let source = musicxml::parse(
                xml(&measure(&format!(
                    "{}{starts}{}{}",
                    dynamic("p", 0),
                    wedge("stop", 960, "stop", "", ""),
                    note(1440)
                )))
                .as_bytes(),
            )
            .unwrap();
            let curve = timeline(&source);
            assert_db(&curve, 720, -5.875);
            assert!(!curve.issues.iter().any(|i| i.kind == IssueKind::Conflict));
            let other = source_id(&source, "other");
            let provenance = curve
                .segment_at(at(720))
                .unwrap()
                .provenance
                .as_ref()
                .unwrap();
            assert_eq!(
                provenance
                    .evidence
                    .iter()
                    .any(|e| e.source_ids.contains(&other)),
                mapped
            );
            let ledger = saved_ledger(&source);
            assert_ledger_mapping(&ledger, &source_id(&source, "active"), true);
            assert_ledger_mapping(&ledger, &source_id(&source, "stop"), true);
            assert_ledger_mapping(&ledger, &other, mapped);
            if !mapped {
                assert!(curve
                    .issues
                    .iter()
                    .any(|issue| issue.kind == IssueKind::PassFiltered
                        && issue.provenance.repeat_pass == 1
                        && issue.message.contains("time-only")
                        && issue
                            .provenance
                            .evidence
                            .iter()
                            .any(|e| e.source_ids.contains(&other))));
                let entry = ledger
                    .entries
                    .iter()
                    .find(|entry| entry.source_id == other)
                    .unwrap();
                assert!(entry
                    .performance_refs
                    .iter()
                    .any(|index| ledger.performance_spans[*index]
                        .detail
                        .contains("time-only")));
            }
        }
    }
}

#[test]
fn incompatible_active_wedge_starts_keep_both_conflict_declarations() {
    for reverse in [false, true] {
        let a = wedge("crescendo", 480, "up", "", "");
        let b = wedge("diminuendo", 480, "down", "", "");
        let starts = if reverse {
            format!("{b}{a}")
        } else {
            format!("{a}{b}")
        };
        let source = musicxml::parse(
            xml(&measure(&format!(
                "{}{starts}{}{}",
                dynamic("p", 0),
                wedge("stop", 960, "stop", "", ""),
                note(1440)
            )))
            .as_bytes(),
        )
        .unwrap();
        let curve = timeline(&source);
        assert_eq!(curve.evaluate(at(720)), Evaluation::Unknown);
        let conflict = curve
            .issues
            .iter()
            .find(|i| i.kind == IssueKind::Conflict)
            .unwrap();
        for marker in ["up", "down", "stop"] {
            let id = source_id(&source, marker);
            assert!(conflict
                .provenance
                .evidence
                .iter()
                .any(|e| e.source_ids.contains(&id)));
            assert_ledger_mapping(&saved_ledger(&source), &id, false);
        }
        assert_db(&curve, 1200, -7.75);
    }
}

fn legacy_spanner(duplicate_at_one: bool, reverse: bool, ticks: &str) -> String {
    let sung = "<Chord><durationType>quarter</durationType><Lyrics><text>la</text></Lyrics><Note><pitch>60</pitch></Note></Chord>";
    let main = format!(
        r#"<voice><Dynamic><subtype>p</subtype></Dynamic><HairPin id="h"><subtype>0</subtype>{ticks}</HairPin>{sung}<endSpanner id="h"/>{sung}{sung}</voice>"#
    );
    let (before, after) = if duplicate_at_one {
        ("quarter", "half")
    } else {
        ("half", "quarter")
    };
    let duplicate = format!(
        r#"<voice><Rest><durationType>{before}</durationType></Rest><endSpanner id="h"/><Rest><durationType>{after}</durationType></Rest></voice>"#
    );
    let voices = if reverse {
        format!("{duplicate}{main}")
    } else {
        format!("{main}{duplicate}")
    };
    format!(
        r#"<museScore version="3.02"><programVersion>3.6.2</programVersion><Score><Division>480</Division><Part id="1"><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.soprano</instrumentId></Instrument></Part><Staff id="1"><Measure len="3/4">{voices}</Measure></Staff></Score></museScore>"#
    )
}

#[test]
fn equal_legacy_endpoints_coalesce_and_conflicts_never_choose_document_order() {
    for equal in [false, true] {
        for reverse in [false, true] {
            for ticks in ["", "<ticks>480</ticks>"] {
                let source =
                    musescore::parse(legacy_spanner(equal, reverse, ticks).as_bytes()).unwrap();
                let event = source
                    .score_intensity
                    .as_ref()
                    .unwrap()
                    .score
                    .events
                    .iter()
                    .find(|e| e.evidence.raw_fields.contains_key("legacy_spanner"))
                    .unwrap();
                assert_eq!(
                    event.evidence.source_ids.len(),
                    3,
                    "start and both original endpoints retained"
                );
                let curve = timeline(&source);
                if equal {
                    assert!(
                        matches!(&event.instruction, Instruction::Transition(t) if t.end == at(480))
                    );
                    assert_db(&curve, 240, -5.875);
                    assert!(!curve.issues.iter().any(|i| matches!(
                        i.kind,
                        IssueKind::Conflict | IssueKind::UnresolvedSpan
                    )));
                    assert_eq!(dyn_at(&exported(&source), 240), -59.0);
                } else {
                    assert!(
                        matches!(&event.instruction, Instruction::Unsupported(message) if message.contains("Conflicting explicit hairpin endpoints"))
                    );
                    assert!(curve
                        .issues
                        .iter()
                        .any(|i| i.message.contains("Conflicting explicit hairpin endpoints")));
                    assert_db(&curve, 240, -7.75);
                    assert_db(&curve, 720, -7.75);
                    assert_eq!(dyn_at(&exported(&source), 720), -78.0);
                }
                let ledger = saved_ledger(&source);
                for id in &event.evidence.source_ids {
                    assert_ledger_mapping(&ledger, id, equal);
                }
            }
        }
    }
}

fn ms_score(body: &str, modern: bool) -> String {
    let (version, program) = if modern {
        ("4.70", "4.7.4")
    } else {
        ("3.02", "3.6.2")
    };
    format!(
        r#"<museScore version="{version}"><programVersion>{program}</programVersion><Score><Division>480</Division><Part id="1"><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.soprano</instrumentId></Instrument></Part><Staff id="1">{body}</Staff></Score></museScore>"#
    )
}
fn ms_note(pitch: u8, duration: &str) -> String {
    format!("<Chord><durationType>{duration}</durationType><Lyrics><text>la</text></Lyrics><Note><pitch>{pitch}</pitch></Note></Chord>")
}
fn ms_endings(
    modern: bool,
    opening_tempo: &str,
    ending_tempo: &str,
    dynamic: &str,
    first_compound: bool,
) -> String {
    let sung = ms_note(60, "quarter");
    let first = if first_compound {
        "<Dynamic><subtype>fp</subtype></Dynamic>"
    } else {
        ""
    };
    let volta = |pass| {
        format!(
            r#"<Spanner type="Volta"><Volta><endings>{pass}</endings></Volta><next><location><measures>1</measures></location></next></Spanner>"#
        )
    };
    ms_score(
        &format!(
            r#"
        <Measure len="1/4"><startRepeat/><voice><Tempo id="opening-tempo"><tempo>{opening_tempo}</tempo></Tempo><Dynamic><subtype>p</subtype></Dynamic>{sung}</voice></Measure>
        <Measure len="1/4">{}<voice><Tempo id="ending-tempo"><tempo>{ending_tempo}</tempo></Tempo>{first}{sung}</voice><endRepeat>2</endRepeat></Measure>
        <Measure len="1/4">{}<voice><Dynamic id="second-compound">{dynamic}</Dynamic>{sung}</voice></Measure>
        <Measure len="1/4"><voice>{sung}</voice></Measure>"#,
            volta(1),
            volta(2)
        ),
        modern,
    )
}
fn nominal_notes(source: &midi::Midi) -> Vec<(u32, u8)> {
    source
        .tracks
        .iter()
        .flat_map(|t| &t.events)
        .filter_map(|e| match &e.kind {
            midi::Kind::NoteOn(n) => n.key.map(|key| (e.tick, key)),
            _ => None,
        })
        .collect()
}

#[test]
fn source_review4_compounds_use_exact_performed_tempo_after_skipped_ending() {
    for modern in [false, true] {
        for (dynamic, midpoint, held) in [
            ("<subtype>fp</subtype>", -1.875, -7.75),
            ("<subtype>pf</subtype>", -1.875, 4.0),
            (
                "<subtype>p</subtype><veloChange>20</veloChange>",
                -5.25,
                -2.75,
            ),
        ] {
            for ending_tempo in ["1", "1.0000000000000000001"] {
                let document = ms_endings(modern, "2", ending_tempo, dynamic, false);
                let source = musescore::parse(document.as_bytes()).unwrap();
                let before = format!("{:?}", source.tracks);
                assert_eq!(
                    nominal_notes(&source),
                    [(0, 60), (480, 60), (960, 60), (1440, 60), (1920, 60)]
                );
                let tempos: Vec<_> = source
                    .tracks
                    .iter()
                    .flat_map(|t| &t.events)
                    .filter_map(|e| match e.kind {
                        midi::Kind::Tempo(micros) => Some((e.tick, micros)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(tempos, [(0, 500_000), (480, 1_000_000), (960, 500_000)]);
                let curve = timeline(&source);
                assert_db(&curve, 1632, midpoint);
                assert_db(&curve, 1824, held);
                assert_db(&curve, 2100, held);
                assert!(!curve.issues.iter().any(|i| i.message.contains("precision")));
                let segment = curve.segment_at(at(1632)).unwrap();
                assert!(matches!(segment.curve, Curve::Transition { start, end, .. }
                    if start == at(1440) && end == at(1824)));
                let p = segment.provenance.as_ref().unwrap();
                assert_eq!(p.repeat_pass, 2);
                let tempo_id = source_id(&source, "opening-tempo");
                let skipped_id = source_id(&source, "ending-tempo");
                assert!(p.interpretations.iter().any(|i| i.field == "tempo"
                    && i.basis == Basis::ExplicitNumeric
                    && i.exact == Some(Fraction::integer(120))
                    && i.source_ids.contains(&tempo_id)));
                assert!(!p
                    .evidence
                    .iter()
                    .any(|e| e.source_ids.contains(&skipped_id)));
                let compound = source_id(&source, "second-compound");
                for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                    let ledger = saved_ledger_for(&source, target);
                    assert_ledger_mapping(&ledger, &compound, target == ExportTarget::Ustx);
                }
                assert!((dyn_at(&exported(&source), 1632) / 10.0 - midpoint).abs() <= 0.1);
                assert_eq!(
                    format!("{:?}", source.tracks),
                    before,
                    "nominal source events remain immutable"
                );
            }
        }
        // The invalid exact tempo is diagnosed when a compound actually reaches
        // it on pass one, and cannot poison the valid second-ending compound.
        let source = musescore::parse(
            ms_endings(
                modern,
                "2",
                "1.0000000000000000001",
                "<subtype>fp</subtype>",
                true,
            )
            .as_bytes(),
        )
        .unwrap();
        let curve = timeline(&source);
        let invalid: Vec<_> = curve
            .issues
            .iter()
            .filter(|i| i.message.contains("precision"))
            .collect();
        assert_eq!(invalid.len(), 1);
        assert_eq!(
            (invalid[0].start, invalid[0].provenance.repeat_pass),
            (at(480), 1)
        );
        assert_db(&curve, 1632, -1.875);
    }
}

#[test]
fn source_review4_compound_timing_retains_source_precision_and_default120() {
    for modern in [false, true] {
        let source = musescore::parse(
            ms_endings(
                modern,
                "0.83333333333333337",
                "2",
                "<subtype>pf</subtype>",
                false,
            )
            .as_bytes(),
        )
        .unwrap();
        let curve = timeline(&source);
        let segment = curve.segment_at(at(1500)).unwrap();
        let duration = Fraction::decimal("0.333333333333333348").unwrap();
        assert!(matches!(segment.curve, Curve::Transition { start, end, .. }
            if start == Fraction::integer(3) && end == start.checked_add(duration).unwrap()));
        assert!(segment
            .provenance
            .as_ref()
            .unwrap()
            .interpretations
            .iter()
            .any(|i| i.field == "duration" && i.exact == Some(duration)));
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            saved_ledger_for(&source, target);
        }
        let source = musescore::parse(
            ms_score(
                &format!(
                    "<Measure><voice><Dynamic><subtype>fp</subtype></Dynamic>{}</voice></Measure>",
                    ms_note(60, "quarter")
                ),
                modern,
            )
            .as_bytes(),
        )
        .unwrap();
        let curve = timeline(&source);
        assert_db(&curve, 192, -1.875);
        assert!(curve
            .segment_at(at(192))
            .unwrap()
            .provenance
            .as_ref()
            .unwrap()
            .interpretations
            .iter()
            .any(|i| i.field == "tempo"
                && i.basis == Basis::DeclaredDefault
                && i.exact == Some(Fraction::integer(120))));
    }
}

#[test]
fn source_review4_invalid_numeric_preserves_wedges_and_independent_words() {
    let document = xml(&measure(&format!(
        r#"{}
        <direction id="mixed"><direction-type><dynamics><ff/></dynamics><wedge type="crescendo"/><words>cresc.</words><words>dolce</words></direction-type><offset sound="yes">240</offset><staff>1</staff><voice>1</voice><sound dynamics="bad-level" time-only="2"/></direction>
        <direction id="stop"><direction-type><wedge type="stop"/></direction-type><offset sound="yes">720</offset><staff>1</staff><voice>1</voice><sound time-only="2"/></direction>
        {}<barline location="right"><repeat direction="backward"/></barline>"#,
        dynamic("p", 0),
        note(960)
    )));
    let source = musicxml::parse(document.as_bytes()).unwrap();
    let input = source.score_intensity.as_ref().unwrap();
    let mixed = source_id(&source, "mixed");
    let events: Vec<_> = input
        .score
        .events
        .iter()
        .filter(|e| e.evidence.source_ids.contains(&mixed))
        .collect();
    assert!(events
        .iter()
        .any(|e| matches!(e.instruction, Instruction::Transition(_))));
    assert!(events
        .iter()
        .any(|e| matches!(&e.instruction, Instruction::Text { text, .. } if text == "dolce")));
    assert!(events
        .iter()
        .any(|e| matches!(&e.instruction, Instruction::Unsupported(m) if m.contains("bad-level"))));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.instruction, Instruction::Dynamic(_))),
        "invalid numeric cannot fall back to ff"
    );
    for event in &events {
        assert_eq!(
            (event.at, event.scope.clone(), event.time_only.clone()),
            (
                at(240),
                Scope::musicxml("P1", Some("1"), Some("1")),
                vec![2]
            )
        );
    }
    let curve = timeline(&source);
    assert_db(&curve, 480, -7.75);
    assert_db(&curve, 1440, -5.875);
    assert_db(&curve, 1800, -4.0);
    assert!(curve.issues.iter().any(|i| i.start == at(1200)
        && i.provenance.repeat_pass == 2
        && i.message.contains("bad-level")));
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let ledger = saved_ledger_for(&source, target);
        assert_ledger_mapping(&ledger, &mixed, target == ExportTarget::Ustx);
        assert!(ledger
            .performance_spans
            .iter()
            .any(|span| span.detail.contains("bad-level")));
    }
}

#[test]
fn source_review4_invalid_wedge_does_not_skip_following_wedge_or_words() {
    for invalid in [
        r#"<wedge type="unknown" number="9"/>"#,
        r#"<wedge type="diminuendo" niente="bad" number="9"/>"#,
    ] {
        for valid_sibling in [false, true] {
            let good = if valid_sibling {
                r#"<wedge type="crescendo" number="1"/>"#
            } else {
                ""
            };
            let stop = if valid_sibling {
                wedge("stop", 960, "stop", "", "")
            } else {
                String::new()
            };
            let document = xml(&measure(&format!(
                r#"{}<direction id="mixed"><direction-type>{invalid}{good}<words>cresc.</words></direction-type><offset sound="yes">480</offset></direction>{stop}{}{}"#,
                dynamic("p", 0),
                dynamic("f", 960),
                note(1440)
            )));
            let source = musicxml::parse(document.as_bytes()).unwrap();
            let curve = timeline(&source);
            assert_db(&curve, 720, -1.875);
            assert_db(&curve, 1200, 4.0);
            let input = source.score_intensity.as_ref().unwrap();
            assert!(input
                .score
                .events
                .iter()
                .any(|e| matches!(e.instruction, Instruction::Unsupported(_))));
            assert_eq!(
                input
                    .score
                    .events
                    .iter()
                    .any(|e| matches!(e.instruction, Instruction::Text { .. })),
                !valid_sibling
            );
            for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                saved_ledger_for(&source, target);
            }
        }
    }
}

#[test]
fn source_review4_invalid_shared_offset_or_pass_prevents_active_siblings() {
    for shared in [
        r#"<offset sound="yes">-1</offset><sound time-only="2"/>"#,
        r#"<offset sound="yes">240</offset><sound time-only="bad-pass"/>"#,
    ] {
        let source = musicxml::parse(xml(&measure(&format!(r#"{}<direction id="mixed"><direction-type><dynamics><ff/></dynamics><wedge type="crescendo"/><words>cresc.</words></direction-type>{shared}</direction>{}"#, dynamic("p", 0), note(960)))).as_bytes()).unwrap();
        let input = source.score_intensity.as_ref().unwrap();
        let mixed = source_id(&source, "mixed");
        let events: Vec<_> = input
            .score
            .events
            .iter()
            .filter(|e| e.evidence.source_ids.contains(&mixed))
            .collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].instruction, Instruction::Unsupported(_)));
        assert_db(&timeline(&source), 480, -7.75);
    }
}

#[test]
fn source_review4_malformed_musescore_fields_keep_scope_enabled_order_and_position() {
    for (modern, assignment, expected) in [
        (
            true,
            "<voiceAssignment>currentVoiceOnly</voiceAssignment>",
            vec![60],
        ),
        (
            true,
            "<voiceAssignment>allInInstrument</voiceAssignment>",
            vec![60, 62, 64],
        ),
        (false, "<dynType>1</dynType>", vec![60, 62, 64]),
        (false, "<dynType>2</dynType>", vec![60, 62, 64, 65]),
    ] {
        for malformed in [
            "<velocity>bad-velocity</velocity>",
            "<veloChangeSpeed>bad-speed</veloChangeSpeed>",
        ] {
            for enabled in [false, true] {
                let bad = format!(
                    r#"<Dynamic id="malformed"><subtype>fp</subtype>{assignment}<play>{}</play>{malformed}</Dynamic>"#,
                    u8::from(enabled)
                );
                let body = format!("<Measure len=\"3/4\"><voice><Dynamic><subtype>p</subtype><dynType>2</dynType></Dynamic>{}{bad}{}</voice><voice>{}{}</voice></Measure>",
                    ms_note(60, "quarter"), ms_note(60, "half"), ms_note(62, "quarter"), ms_note(62, "half"));
                let document = ms_score(&body, modern)
                    .replace("<Staff id=\"1\"/><Instrument", "<Staff id=\"1\"/><Staff id=\"2\"/><Instrument")
                    .replace("</Part><Staff", "</Part><Part id=\"2\"><trackName>Other</trackName><Staff id=\"3\"/><Instrument id=\"voice\"><instrumentId>voice.soprano</instrumentId></Instrument></Part><Staff")
                    .replace("</Score>", &format!("<Staff id=\"2\"><Measure len=\"3/4\"><voice>{}{}</voice></Measure></Staff><Staff id=\"3\"><Measure len=\"3/4\"><voice>{}{}</voice></Measure></Staff></Score>",
                        ms_note(64, "quarter"), ms_note(64, "half"), ms_note(65, "quarter"), ms_note(65, "half")));
                let source = musescore::parse(document.as_bytes()).unwrap();
                let id = source_id(&source, "malformed");
                let event = source
                    .score_intensity
                    .as_ref()
                    .unwrap()
                    .score
                    .events
                    .iter()
                    .find(|e| e.evidence.source_ids.contains(&id))
                    .unwrap();
                let parsed = roxmltree::Document::parse(&document).unwrap();
                assert_eq!(
                    event.order,
                    parsed
                        .descendants()
                        .find(|n| n.attribute("id") == Some("malformed"))
                        .unwrap()
                        .id()
                        .get()
                );
                assert_eq!((event.at, event.enabled), (at(480), enabled));
                assert!(!matches!(event.scope, Scope::Unsupported { .. }));
                let index = performance::normalize(&source).unwrap();
                let mut affected = std::collections::BTreeSet::new();
                for track in &source.tracks {
                    for e in &track.events {
                        if let midi::Kind::NoteOn(note) = &e.kind {
                            if let Some(binding) = index.bindings.get(&(track.id.clone(), e.order))
                            {
                                if let Some(intensity) = &binding.intensity {
                                    let issues: Vec<_> = intensity
                                        .timeline
                                        .issues
                                        .iter()
                                        .filter(|i| {
                                            i.provenance
                                                .evidence
                                                .iter()
                                                .any(|ev| ev.source_ids.contains(&id))
                                        })
                                        .collect();
                                    if !issues.is_empty() {
                                        affected.insert(note.key.unwrap());
                                        assert!(issues.iter().all(|i| i.start == at(480)
                                            && i.kind
                                                == if enabled {
                                                    IssueKind::UnsupportedMark
                                                } else {
                                                    IssueKind::Disabled
                                                }));
                                    }
                                    assert_db(&intensity.timeline, 720, -7.75);
                                }
                            }
                        }
                    }
                }
                assert_eq!(affected.into_iter().collect::<Vec<_>>(), expected);
                for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                    let ledger = saved_ledger_for(&source, target);
                    assert_ledger_mapping(&ledger, &id, false);
                }
            }
        }
    }
}

const PERMUTATIONS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];
#[test]
fn source_review4_three_peer_ramps_have_order_independent_conflicts_tail_and_ledgers() {
    for duplicate in [false, true] {
        for permutation in PERMUTATIONS {
            let starts = [("crescendo", 1), ("diminuendo", 2), ("crescendo", 3)];
            let mut body = dynamic("p", 0);
            for index in permutation {
                let (kind, number) = starts[index];
                body.push_str(
                    &wedge(kind, 480, &format!("start-{number}"), "", "")
                        .replace("number=\"1\"", &format!("number=\"{number}\"")),
                );
            }
            if duplicate {
                body.push_str(
                    &wedge("crescendo", 480, "duplicate", "", "")
                        .replace("number=\"1\"", "number=\"3\""),
                );
            }
            body.push_str(
                &wedge("crescendo", 480, "excluded", "", "<sound time-only=\"2\"/>")
                    .replace("number=\"1\"", "number=\"3\""),
            );
            for (number, tick) in [(1, 960), (2, 1440), (3, 1920)] {
                body.push_str(
                    &wedge("stop", tick, &format!("stop-{number}"), "", "")
                        .replace("number=\"1\"", &format!("number=\"{number}\"")),
                );
            }
            body.push_str(&note(2400));
            let source = musicxml::parse(xml(&measure(&body)).as_bytes()).unwrap();
            let curve = timeline(&source);
            for tick in (0..=2400).step_by(60) {
                if (480..1440).contains(&tick) {
                    assert_eq!(
                        curve.evaluate(at(tick)),
                        Evaluation::Unknown,
                        "{permutation:?} at {tick}"
                    );
                } else {
                    let expected = if tick < 480 {
                        -7.75
                    } else if tick < 1920 {
                        -7.75 + 3.75 * (tick - 480) as f64 / 1440.0
                    } else {
                        -4.0
                    };
                    assert_db(&curve, tick, expected);
                }
            }
            let conflicts: Vec<_> = curve
                .issues
                .iter()
                .filter(|i| i.kind == IssueKind::Conflict)
                .map(|i| (i.start, i.end))
                .collect();
            assert_eq!(conflicts, [(at(480), at(960)), (at(960), at(1440))]);
            let survivor = curve.segment_at(at(1680)).unwrap();
            assert!(
                matches!(survivor.curve, Curve::Transition { start, end, from, to, .. }
                if start == at(480) && end == at(1920) && from == Level::Positive(Fraction::integer(49)) && to == Level::Positive(Fraction::integer(64)))
            );
            let ids: Vec<_> = survivor
                .provenance
                .as_ref()
                .unwrap()
                .evidence
                .iter()
                .flat_map(|e| &e.source_ids)
                .collect();
            assert!(ids.contains(&&source_id(&source, "start-3")));
            assert!(!ids.contains(&&source_id(&source, "excluded")));
            if duplicate {
                assert!(ids.contains(&&source_id(&source, "duplicate")));
            }
            let target = exported(&source);
            assert!((dyn_at(&target, 1680) / 10.0 + 4.625).abs() <= 0.1);
            assert_eq!(dyn_at(&target, 2100), -40.0);
            for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                let ledger = saved_ledger_for(&source, target);
                assert_ledger_mapping(
                    &ledger,
                    &source_id(&source, "start-3"),
                    target == ExportTarget::Ustx,
                );
                assert_ledger_mapping(&ledger, &source_id(&source, "excluded"), false);
                if duplicate {
                    assert_ledger_mapping(
                        &ledger,
                        &source_id(&source, "duplicate"),
                        target == ExportTarget::Ustx,
                    );
                }
            }
        }
    }
}

fn note_geometry(source: &midi::Midi) -> Vec<(u32, bool, Option<u8>)> {
    source
        .tracks
        .iter()
        .flat_map(|track| &track.events)
        .filter_map(|event| match &event.kind {
            midi::Kind::NoteOn(note) => Some((event.tick, true, note.key)),
            midi::Kind::NoteOff(note) => Some((event.tick, false, note.key)),
            _ => None,
        })
        .collect()
}

#[test]
fn source_review5_numeric_levels_do_not_choose_incompatible_printed_behavior() {
    for symbols in ["<p/><fp/>", "<fp/><p/>", "<f/><sfz/>", "<sfz/><f/>"] {
        let body = format!(
            r#"<direction id="conflict"><direction-type><dynamics>{symbols}</dynamics><wedge type="crescendo"/><words>cresc.</words></direction-type><sound dynamics="100"/></direction>{}{}"#,
            wedge("stop", 960, "end", "", ""),
            note(1440)
        );
        let source = musicxml::parse(xml(&measure(&body)).as_bytes()).unwrap();
        let before = note_geometry(&source);
        let id = source_id(&source, "conflict");
        let input = source.score_intensity.as_ref().unwrap();
        assert!(input.score.events.iter().any(|e| e.evidence.source_ids.contains(&id)
            && matches!(&e.instruction, Instruction::Unsupported(message) if message.contains("Multiple unrelated dynamics"))));
        // The independent wedge remains active with the permitted L80 default.
        assert_db(&timeline(&source), 480, 2.0);
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            saved_ledger_for(&source, target);
        }
        assert_eq!(note_geometry(&source), before);
    }
    let source = musicxml::parse(xml(&measure(&format!(r#"<direction><direction-type><dynamics><p/></dynamics></direction-type><sound dynamics="100"/></direction>{}"#, note(960)))).as_bytes()).unwrap();
    assert_db(&timeline(&source), 0, 2.5);
}

#[test]
fn source_review5_coincident_tempos_reconcile_all_exact_candidates() {
    for modern in [false, true] {
        for (left, right, active, kind) in [
            ("2", "2.0", true, IssueKind::Conflict),
            ("2", "1", false, IssueKind::Conflict),
            (
                "2",
                "2.0000000000000000001",
                false,
                IssueKind::UnsupportedMark,
            ),
        ] {
            for (a, b) in [(left, right), (right, left)] {
                let body = format!(
                    r#"<Measure><voice><Tempo id="tempo-a"><tempo>{a}</tempo></Tempo><Tempo id="tempo-b"><tempo>{b}</tempo></Tempo><Dynamic id="compound"><subtype>fp</subtype></Dynamic>{}</voice></Measure>"#,
                    ms_note(60, "whole")
                );
                let source = musescore::parse(ms_score(&body, modern).as_bytes()).unwrap();
                let before = note_geometry(&source);
                assert_eq!(before, [(0, true, Some(60)), (1920, false, Some(60))]);
                let ids = [source_id(&source, "tempo-a"), source_id(&source, "tempo-b")];
                let curve = timeline(&source);
                if active {
                    assert_db(&curve, 192, -1.875);
                    let p = curve
                        .segment_at(at(192))
                        .unwrap()
                        .provenance
                        .as_ref()
                        .unwrap();
                    for id in &ids {
                        assert!(p.evidence.iter().any(|e| e.source_ids.contains(id)));
                    }
                } else {
                    assert_eq!(curve.evaluate(at(192)), Evaluation::Absent);
                    assert!(curve.issues.iter().any(|issue| issue.kind == kind
                        && ids.iter().all(|id| issue
                            .provenance
                            .evidence
                            .iter()
                            .any(|e| e.source_ids.contains(id)))));
                }
                for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                    saved_ledger_for(&source, target);
                }
                assert_eq!(note_geometry(&source), before);
            }
        }
    }
}

#[test]
fn source_review5_unplayed_musescore_velocity_has_original_owner_and_location() {
    use verse_lib::engine::score_intensity::{source::DeclarationKind, ScoreVoice};
    for modern in [false, true] {
        for value in ["100", "bad-velocity"] {
            let silent = ms_note(64, "quarter").replace(
                "<Note>",
                &format!(
                    r#"<Note id="unplayed"><velocity>{value}</velocity><veloType>1</veloType>"#
                ),
            );
            let body = format!(
                r#"<Measure len="1/4"><startRepeat/><voice>{}</voice></Measure><Measure len="1/4"><voice>{}</voice><endRepeat>2</endRepeat></Measure><Measure len="1/4"><Spanner type="Volta"><Volta><endings>3</endings></Volta><next><location><measures>1</measures></location></next></Spanner><voice>{silent}</voice></Measure><Measure len="1/4"><voice>{}</voice></Measure>"#,
                ms_note(60, "quarter"),
                ms_note(62, "quarter"),
                ms_note(65, "quarter")
            );
            let source = musescore::parse(ms_score(&body, modern).as_bytes()).unwrap();
            assert_eq!(
                nominal_notes(&source),
                [(0, 60), (480, 62), (960, 60), (1440, 62), (1920, 65)]
            );
            let before = note_geometry(&source);
            let id = source_id(&source, "unplayed");
            let input = source.score_intensity.as_ref().unwrap();
            let declaration = &input.declarations[&id];
            let Scope::Voice {
                part,
                staff: Some(staff),
                voice,
            } = &declaration.scope
            else {
                panic!("typed velocity voice");
            };
            let owner = ScoreVoice {
                part: part.clone(),
                staff: staff.clone(),
                voice: voice.clone(),
                instrument: None,
            };
            assert_eq!(
                (staff.as_str(), voice.as_str(), declaration.at),
                ("1", "1", at(960))
            );
            let original = &input.original_declarations[&id];
            assert!(original.kinds.contains(&DeclarationKind::Velocity));
            assert!(original.note_source_id.is_some());
            assert!(input.written_measures[original.written_measure.unwrap()]
                .visits
                .is_empty());
            assert!(input.declaration_is_unresolved(&id, &owner));
            assert_eq!(input.declaration_route_pass(&id, &owner, 0, 0), Some(0));
            for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                let ledger = saved_ledger_for(&source, target);
                assert_ledger_mapping(&ledger, &id, false);
                let entry = ledger
                    .entries
                    .iter()
                    .find(|entry| entry.source_id == id)
                    .unwrap();
                assert!(!entry.performance_refs.is_empty());
                assert!(entry.performance_refs.iter().all(|i| {
                    let span = &ledger.performance_spans[*i];
                    span.note_ids.is_empty() && span.target_track.is_none()
                }));
            }
            assert_eq!(note_geometry(&source), before);
        }
    }
}

#[test]
fn source_review5_missing_voice_on_occupied_staff_gets_metadata_owner() {
    let xml_body = format!(
        r#"<direction id="silent-voice"><direction-type><dynamics><f/></dynamics></direction-type><staff>1</staff><voice>2</voice></direction>{}"#,
        note(960)
    );
    let ms_body = format!(
        r#"<Measure len="1/2"><voice>{}</voice><voice><Dynamic id="silent-voice"><subtype>f</subtype><voiceAssignment>currentVoiceOnly</voiceAssignment></Dynamic></voice></Measure>"#,
        ms_note(60, "half")
    );
    let sources = [
        musicxml::parse(xml(&measure(&xml_body)).as_bytes()).unwrap(),
        musescore::parse(ms_score(&ms_body, true).as_bytes()).unwrap(),
    ];
    for source in sources {
        assert_eq!(
            note_geometry(&source),
            [(0, true, Some(60)), (960, false, Some(60))]
        );
        let metadata = source
            .tracks
            .iter()
            .find(|t| t.id.ends_with("intensity-metadata"))
            .unwrap();
        assert_eq!(metadata.source.voice, None);
        assert!(metadata.events.is_empty());
        let id = source_id(&source, "silent-voice");
        let index = performance::normalize(&source).unwrap();
        assert!(index
            .bindings
            .values()
            .all(|binding| binding.intensity.as_ref().is_none_or(|n| n
                .timeline
                .segments
                .iter()
                .filter_map(|s| s.provenance.as_ref())
                .all(|p| p.evidence.iter().all(|e| !e.source_ids.contains(&id))))));
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let ledger = saved_ledger_for(&source, target);
            assert_ledger_mapping(&ledger, &id, false);
            let entry = ledger.entries.iter().find(|e| e.source_id == id).unwrap();
            assert!(!entry.performance_refs.is_empty());
            assert!(entry.performance_refs.iter().all(|i| {
                let span = &ledger.performance_spans[*i];
                span.note_ids.is_empty() && span.target_track.is_none()
            }));
        }
    }
}

#[test]
fn source_review5_empty_measure_offsets_stay_unresolved_beside_positive_offsets() {
    use verse_lib::engine::score_intensity::ScoreVoice;
    for repeated in [false, true] {
        let empty = format!(
            r#"{}<direction id="empty"><direction-type><dynamics><f/></dynamics></direction-type><offset sound="yes">240</offset></direction>"#,
            if repeated {
                r#"<barline location="left"><repeat direction="forward"/></barline>"#
            } else {
                ""
            }
        );
        let positive = format!(
            r#"<direction id="positive"><direction-type><dynamics><p/></dynamics></direction-type><offset sound="yes">240</offset></direction>{}{}"#,
            note(960),
            if repeated {
                r#"<barline location="right"><repeat direction="backward"/></barline>"#
            } else {
                ""
            }
        );
        let source =
            musicxml::parse(xml(&format!("{}{}", measure(&empty), measure(&positive))).as_bytes())
                .unwrap();
        let before = note_geometry(&source);
        assert_eq!(
            nominal_notes(&source),
            if repeated {
                vec![(0, 60), (960, 60)]
            } else {
                vec![(0, 60)]
            }
        );
        let input = source.score_intensity.as_ref().unwrap();
        let owner = ScoreVoice {
            part: "P1".into(),
            staff: "1".into(),
            voice: "1".into(),
            instrument: None,
        };
        let empty_id = source_id(&source, "empty");
        let positive_id = source_id(&source, "positive");
        assert_eq!(
            input.declarations[&empty_id].at,
            input.declarations[&positive_id].at
        );
        assert!(input.declaration_is_unresolved(&empty_id, &owner));
        assert!(!input.declaration_is_unresolved(&positive_id, &owner));
        let curve = timeline(&source);
        assert_db(&curve, 480, -7.75);
        if repeated {
            assert_db(&curve, 1440, -7.75);
        }
        for run in input.owner_runs(&owner).unwrap() {
            assert!(!input.declaration_on_route(&empty_id, &owner, run.ordinal, run.pass));
            assert!(input.declaration_on_route(&positive_id, &owner, run.ordinal, run.pass));
        }
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let ledger = saved_ledger_for(&source, target);
            assert_ledger_mapping(&ledger, &empty_id, false);
            assert_ledger_mapping(&ledger, &positive_id, target == ExportTarget::Ustx);
        }
        assert_eq!(note_geometry(&source), before);
    }
}

#[test]
fn source_review5_wedge_endpoints_pair_by_performed_pass() {
    for restricted_start in [false, true] {
        let start_extra = if restricted_start {
            r#"<sound time-only="2"/>"#
        } else {
            ""
        };
        let stop_extra = r#"<sound time-only="1"/>"#;
        let start = wedge("crescendo", 0, "start", "", start_extra);
        let early = wedge("stop", 480, "early", "", stop_extra);
        let late = wedge("stop", 960, "late", "", r#"<sound time-only="2"/>"#);
        let body = format!(
            r#"<barline location="left"><repeat direction="forward"/></barline>{}{}{}{}{}<barline location="right"><repeat direction="backward"/></barline>"#,
            dynamic("p", 0),
            start,
            early,
            late,
            note(1440)
        );
        let source = musicxml::parse(xml(&measure(&body)).as_bytes()).unwrap();
        assert_eq!(nominal_notes(&source), [(0, 60), (1440, 60)]);
        let curve = timeline(&source);
        assert_db(&curve, 1920, -5.875);
        assert_db(&curve, 240, if restricted_start { -7.75 } else { -5.875 });
        let start_id = source_id(&source, "start");
        let late_id = source_id(&source, "late");
        let early_id = source_id(&source, "early");
        let p = curve
            .segment_at(at(1920))
            .unwrap()
            .provenance
            .as_ref()
            .unwrap();
        assert_eq!(p.repeat_pass, 2);
        assert!(p
            .evidence
            .iter()
            .any(|e| e.source_ids.contains(&start_id) && e.source_ids.contains(&late_id)));
        assert!(p.evidence.iter().all(|e| !e.source_ids.contains(&early_id)));
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            saved_ledger_for(&source, target);
        }
    }
}

#[test]
fn source_review5_no_run_part_retains_only_unresolved_declaration_owner() {
    use verse_lib::engine::score_intensity::ScoreVoice;
    let document = format!(
        r#"<score-partwise><part-list><score-part id="P1"><part-name>Empty</part-name></score-part><score-part id="P2"><part-name>Voice</part-name></score-part></part-list><part id="P1">{}</part><part id="P2">{}</part></score-partwise>"#,
        measure(
            r#"<direction id="no-run"><direction-type><dynamics><f/></dynamics></direction-type><offset sound="yes">240</offset></direction>"#
        ),
        measure(&note(960))
    );
    let source = musicxml::parse(document.as_bytes()).unwrap();
    assert_eq!(nominal_notes(&source), [(0, 60)]);
    let id = source_id(&source, "no-run");
    let input = source.score_intensity.as_ref().unwrap();
    let owner = ScoreVoice {
        part: "P1".into(),
        staff: "1".into(),
        voice: "1".into(),
        instrument: None,
    };
    assert!(input.owner_runs(&owner).is_none_or(|runs| runs.is_empty()));
    assert_eq!(input.declaration_route_pass(&id, &owner, 0, 0), Some(0));
    assert_eq!(input.declaration_route_pass(&id, &owner, 1, 1), None);
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let ledger = saved_ledger_for(&source, target);
        assert_ledger_mapping(&ledger, &id, false);
    }
}

#[test]
fn source_review5_first_ending_stop_does_not_consume_later_route_start() {
    let opening = format!(
        "{}{}",
        dynamic("p", 0),
        wedge("crescendo", 0, "route-start", "", "")
    );
    let second = wedge("stop", 0, "route-late", "", "");
    let document = endings(&opening, &second).replace(
        &dynamic("f", 0),
        &format!(
            "{}{}",
            dynamic("f", 0),
            wedge("stop", 0, "route-early", "", "")
        ),
    );
    let source = musicxml::parse(document.as_bytes()).unwrap();
    assert_eq!(
        nominal_notes(&source),
        [(0, 60), (480, 60), (960, 60), (1440, 60), (2400, 60)]
    );
    let input = source.score_intensity.as_ref().unwrap();
    let start = source_id(&source, "route-start");
    let late = source_id(&source, "route-late");
    let early = source_id(&source, "route-early");
    let candidates: Vec<_> = input
        .score
        .events
        .iter()
        .filter(|e| {
            matches!(e.instruction, Instruction::Transition(_))
                && e.evidence.source_ids.contains(&start)
        })
        .collect();
    assert!(candidates
        .iter()
        .any(|e| e.time_only == [1] && e.evidence.source_ids.contains(&early)));
    assert!(candidates.iter().any(|e| e.time_only == [2]
        && e.evidence.source_ids.contains(&late)
        && !e.evidence.source_ids.contains(&early)));
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        saved_ledger_for(&source, target);
    }
}
