//! Accepted combined-review regressions through real score parsers and conversion.
use std::sync::Arc;
use verse_lib::engine::{
    convert::convert_midi_with_target,
    midi, musescore, musicxml, performance,
    projection::ProjectedLyric,
    score_intensity::{Curve, Evaluation, Fraction, Instruction, Intensity, IssueKind, Timeline},
    target::{self, ustx, ExportTarget},
};

fn xml(body: &str) -> String {
    format!(
        r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes>{body}</measure></part></score-partwise>"#
    )
}
fn note(duration: u32) -> String {
    format!("<note><pitch><step>C</step><octave>4</octave></pitch><duration>{duration}</duration><lyric><text>la</text></lyric></note>")
}
fn direction(mark: &str, tick: u32) -> String {
    format!(
        r#"<direction><direction-type><dynamics><{mark}/></dynamics></direction-type><offset sound="yes">{tick}</offset></direction>"#
    )
}
fn wedge(kind: &str, tick: u32, extra: &str) -> String {
    format!(
        r#"<direction><direction-type><wedge type="{kind}" number="1"/></direction-type><offset sound="yes">{tick}</offset>{extra}</direction>"#
    )
}
fn ms(body: &str, modern: bool) -> String {
    let (version, program) = if modern {
        ("4.70", "4.7.4")
    } else {
        ("3.02", "3.6.2")
    };
    format!(
        r#"<museScore version="{version}"><programVersion>{program}</programVersion><Score><Division>480</Division><Part id="1"><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.soprano</instrumentId></Instrument></Part><Staff id="1"><Measure len="4/4"><voice>{body}</voice></Measure></Staff></Score></museScore>"#
    )
}
fn chord(lyrics: &str, extra: &str) -> String {
    format!("<Chord><durationType>quarter</durationType>{lyrics}<Note><pitch>65</pitch>{extra}</Note></Chord>")
}
fn sung() -> String {
    chord("<Lyrics><text>la</text></Lyrics>", "")
}
fn timeline(source: &midi::Midi) -> Arc<Timeline> {
    performance::normalize(source)
        .unwrap()
        .bindings
        .values()
        .find_map(|b| b.intensity.as_ref().map(|i| i.timeline.clone()))
        .expect("score timeline")
}
fn at(ticks: i64) -> Fraction {
    Fraction::new(ticks, 480).unwrap()
}
fn db(value: Evaluation) -> f64 {
    match value {
        Evaluation::Known(Intensity::Decibels(db)) => db,
        other => panic!("finite intensity: {other:?}"),
    }
}
fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-8, "{actual} != {expected}");
}
fn exported(source: &midi::Midi) -> ustx::UstxProject {
    let out = convert_midi_with_target(source, "english", None, ExportTarget::Ustx);
    assert!(out.ok, "{:?}", out.msg);
    ustx::serialize(out.svp.as_ref().unwrap()).unwrap()
}
fn dyn_at(project: &ustx::UstxProject, tick: i32) -> f64 {
    let curve = project.voice_parts[0]
        .curves
        .iter()
        .find(|c| c.abbr == "dyn")
        .expect("active DYN");
    match curve.xs.binary_search(&tick) {
        Ok(i) => f64::from(curve.ys[i]),
        Err(i) if i > 0 && i < curve.xs.len() => {
            let x = f64::from(tick - curve.xs[i - 1]) / f64::from(curve.xs[i] - curve.xs[i - 1]);
            f64::from(curve.ys[i - 1]) + x * f64::from(curve.ys[i] - curve.ys[i - 1])
        }
        other => panic!("missing sample at {tick}: {other:?}"),
    }
}

#[test]
fn owning_hairpin_fade_labels_are_active_once_in_both_layouts() {
    for modern in [false, true] {
        for label in [
            "morendo",
            "smorzando",
            "fade out",
            "fade-out",
            "fondu",
            "fondu au silence",
        ] {
            let body = format!("<Dynamic><subtype>p</subtype></Dynamic>{}<HairPin><subtype>1</subtype><beginText>{label}</beginText><ticks>480</ticks></HairPin>{}{}", sung(), sung(), sung());
            let source = musescore::parse(ms(&body, modern).as_bytes()).unwrap();
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
            let curve = timeline(&source);
            close(db(curve.evaluate(at(720))), -7.75 + 20.0 * 0.5f64.log10());
            assert_eq!(
                curve.evaluate(at(960)),
                Evaluation::Known(Intensity::Silence)
            );
            let output = exported(&source);
            assert!((dyn_at(&output, 720) - (-7.75 + 20.0 * 0.5f64.log10()) * 10.0).abs() <= 1.0);
            assert_eq!(dyn_at(&output, 960), -240.0);
        }
    }
}

#[test]
fn owning_fade_in_and_conflicting_labels_preserve_local_intent() {
    for modern in [false, true] {
        let body = format!("{}<HairPin><subtype>0</subtype><beginText>fade in</beginText><ticks>480</ticks><endText>f</endText></HairPin>{}{}", sung(), sung(), sung());
        let source = musescore::parse(ms(&body, modern).as_bytes()).unwrap();
        let curve = timeline(&source);
        assert_eq!(
            curve.evaluate(at(480)),
            Evaluation::Known(Intensity::Silence)
        );
        close(db(curve.evaluate(at(720))), 4.0 + 20.0 * 0.5f64.log10());
        close(db(curve.evaluate(at(960))), 4.0);
        assert_eq!(dyn_at(&exported(&source), 480), -240.0);

        for labels in [
            "<beginText>fade out</beginText>",
            "<beginText>fade in</beginText><endText>fade out</endText>",
        ] {
            let body = format!("<Dynamic><subtype>p</subtype></Dynamic>{}<HairPin><subtype>0</subtype>{labels}<ticks>480</ticks></HairPin>{}{}", sung(), sung(), sung());
            let source = musescore::parse(ms(&body, modern).as_bytes()).unwrap();
            let curve = timeline(&source);
            close(db(curve.evaluate(at(240))), -7.75);
            assert_eq!(curve.evaluate(at(720)), Evaluation::Unknown);
            close(db(curve.evaluate(at(1200))), -7.75);
            assert!(curve
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::Conflict && i.start == at(480) && i.end == at(960)));
            assert_eq!(
                source
                    .score_intensity
                    .as_ref()
                    .unwrap()
                    .score
                    .events
                    .iter()
                    .filter(|e| matches!(e.instruction, Instruction::Transition(_)))
                    .count(),
                1
            );
        }
        // Unknown free text must not disable an otherwise ordinary hairpin.
        let body = format!("<Dynamic><subtype>p</subtype></Dynamic>{}<HairPin><subtype>1</subtype><beginText>dolce</beginText><ticks>480</ticks></HairPin>{}{}", sung(), sung(), sung());
        let source = musescore::parse(ms(&body, modern).as_bytes()).unwrap();
        close(db(timeline(&source).evaluate(at(960))), -11.75);
    }
}

#[test]
fn contradictory_same_key_wedge_starts_never_choose_xml_order() {
    for directions in [["crescendo", "diminuendo"], ["diminuendo", "crescendo"]] {
        let body = format!(
            "{}{}{}{}{}{}",
            direction("p", 0),
            wedge(directions[0], 480, ""),
            wedge(directions[1], 480, ""),
            wedge("stop", 960, ""),
            direction("f", 1440),
            note(1920)
        );
        let source = musicxml::parse(xml(&body).as_bytes()).unwrap();
        let curve = timeline(&source);
        close(db(curve.evaluate(at(240))), -7.75);
        assert_eq!(curve.evaluate(at(720)), Evaluation::Unknown);
        close(db(curve.evaluate(at(1200))), -7.75);
        close(db(curve.evaluate(at(1680))), 4.0);
        let issue = curve
            .issues
            .iter()
            .find(|i| i.kind == IssueKind::Conflict)
            .expect("local wedge conflict");
        assert_eq!((issue.start, issue.end), (at(480), at(960)));
        let raw: String = issue
            .provenance
            .evidence
            .iter()
            .flat_map(|e| e.raw_fields.values())
            .cloned()
            .collect();
        for kind in ["crescendo", "diminuendo", "stop"] {
            assert!(raw.contains(kind), "missing {kind}");
        }
        let target = exported(&source);
        assert_eq!(dyn_at(&target, 240), -78.0);
        assert_eq!(dyn_at(&target, 1200), -78.0);
        assert_eq!(dyn_at(&target, 1680), 40.0);
    }
}

#[test]
fn duplicate_compatible_and_pass_filtered_starts_keep_supported_ramps() {
    for (second, filter) in [
        ("crescendo", ""),
        ("diminuendo", r#"<sound time-only="2"/>"#),
    ] {
        let body = format!(
            "{}{}{}{}{}",
            direction("p", 0),
            wedge("crescendo", 480, ""),
            wedge(second, 480, filter),
            wedge("stop", 960, ""),
            note(1440)
        );
        let source = musicxml::parse(xml(&body).as_bytes()).unwrap();
        let curve = timeline(&source);
        close(db(curve.evaluate(at(720))), (-7.75 - 4.0) / 2.0);
        close(db(curve.evaluate(at(1200))), -4.0);
        assert!(!curve.issues.iter().any(|i| i.kind == IssueKind::Conflict));
    }
}

#[test]
fn invalid_level_ends_only_at_effective_valid_recovery_for_both_adapters() {
    let invalid = r#"<direction><sound dynamics="200"/></direction>"#;
    // Unknown text, an unanchored ramp and an excluded mark cannot recover the level.
    let body = format!(
        "{invalid}{}{}{}{}{}{}{}{}{}{}{}",
        wedge("crescendo", 240, ""),
        wedge("stop", 360, ""),
        direction("p", 480),
        direction("f", 480),
        r#"<direction><direction-type><dynamics><mp/></dynamics></direction-type><offset sound="yes">720</offset><sound time-only="2"/></direction>"#,
        direction("pp", 960),
        note(480),
        note(480),
        note(480),
        note(480),
        ""
    );
    let source = musicxml::parse(xml(&body).as_bytes()).unwrap();
    let curve = timeline(&source);
    assert_eq!(curve.evaluate(at(720)), Evaluation::Unknown);
    close(db(curve.evaluate(at(1200))), -11.75);
    let invalid = curve
        .issues
        .iter()
        .find(|i| i.kind == IssueKind::InvalidLevel)
        .unwrap();
    assert_eq!((invalid.start, invalid.end), (at(0), at(960)));
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let out = convert_midi_with_target(&source, "english", None, target);
        assert!(out.ok, "{:?}", out.msg);
        let project = out.svp.as_ref().unwrap();
        let reports = target::performance_report(target, project).unwrap();
        let recovered = &project.tracks[0].notes[2..];
        let reports: Vec<_> = reports
            .iter()
            .filter(|r| r.message.contains("outside the bounded positive domain"))
            .collect();
        assert!(!reports.is_empty());
        for report in reports {
            for note in recovered {
                assert!(!report
                    .note_ids
                    .contains(&note.source_evidence.as_ref().unwrap().note_id));
            }
        }
    }
}

#[test]
fn proven_later_tie_tail_refreshes_head_reference_without_reanchoring() {
    for attack_only in [false, true] {
        let lyrics = |row, text| format!("<Lyrics><no>{row}</no><text>{text}</text></Lyrics>");
        let head = chord(
            &format!("{}{}", lyrics(0, "hold"), lyrics(1, "again")),
            r#"<Tie id="a"/><veloType>user</veloType><velocity>100</velocity>"#,
        );
        let tail = chord(
            &lyrics(1, "ta"),
            r#"<endSpanner id="a"/><Tie id="b"/><veloType>user</veloType><velocity>20</velocity>"#,
        );
        let last = chord(
            &lyrics(1, "il"),
            r#"<endSpanner id="b"/><veloType>user</veloType><velocity>20</velocity>"#,
        );
        let end = chord(&format!("{}{}", lyrics(0, "end"), lyrics(1, "end")), "");
        let body = format!("<startRepeat/>{head}{tail}<HairPin><subtype>0</subtype><ticks>480</ticks><singleNoteDynamics>{}</singleNoteDynamics></HairPin>{last}{end}<endRepeat>2</endRepeat>", !attack_only);
        let source = ms(&body, false)
            .replace("3.02", "2.06")
            .replace("3.6.2", "2.3.2")
            .replace("<voice>", "")
            .replace("</voice>", "");
        let source = musescore::parse(source.as_bytes()).unwrap();
        let out = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
        assert!(out.ok, "{:?}", out.msg);
        let notes = &out.svp.as_ref().unwrap().tracks[0].notes;
        assert_eq!(notes.len(), 8);
        let head = notes[0]
            .performance
            .as_ref()
            .unwrap()
            .intensity
            .as_ref()
            .unwrap();
        let tail = notes[2]
            .performance
            .as_ref()
            .unwrap()
            .intensity
            .as_ref()
            .unwrap();
        assert!(matches!(notes[2].lyric, ProjectedLyric::Extension));
        assert_eq!(tail.source_id, head.source_id);
        assert_eq!(tail.start, at(0));
        assert!(Arc::ptr_eq(&head.timeline, &tail.timeline));
        close(
            db(tail.evaluate(at(1200))),
            if attack_only { 5.0 } else { 7.0 },
        );
        assert!(!tail
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::UnknownAnchor));
        assert!(tail.issues.iter().any(|i| matches!(
            i.kind,
            IssueKind::Conflict | IssueKind::ContinuationVelocity
        )));
        let target = ustx::serialize(out.svp.as_ref().unwrap()).unwrap();
        assert_eq!(dyn_at(&target, 1200), if attack_only { 50.0 } else { 70.0 });
        // On repeat two the written tail syllable is a real attack at L20.
        assert!(matches!(&notes[6].lyric, ProjectedLyric::Source(_)));
        let second = notes[6]
            .performance
            .as_ref()
            .unwrap()
            .intensity
            .as_ref()
            .unwrap();
        assert_ne!(second.source_id, head.source_id);
        close(db(second.evaluate(at(2880))), -15.0);
    }
}

fn ramp_resource_source(count: u32, padding: usize) -> midi::Midi {
    let mut body = format!("<direction><direction-type><dynamics><p/></dynamics></direction-type><padding>{}</padding></direction>","x".repeat(padding));
    for i in 0..count {
        body += &wedge(
            if i % 2 == 0 {
                "crescendo"
            } else {
                "diminuendo"
            },
            (i + 1) * 480,
            "",
        );
        body += &wedge("stop", (i + 2) * 480, "");
    }
    body += &note((count + 3) * 480);
    musicxml::parse(xml(&body).as_bytes()).unwrap()
}

#[test]
fn ordinary_dynamic_replacements_do_not_pay_for_hypothetical_provenance_chains() {
    for overrides in [false, true] {
        let mut declarations = String::new();
        let mut parts = String::new();
        for part in 1..=4 {
            declarations += &format!(
                "<score-part id=\"P{part}\"><part-name>Voice {part}</part-name></score-part>"
            );
            let mut body = String::new();
            for i in 0..96 {
                body += &direction(if i % 2 == 0 { "p" } else { "f" }, 0);
                body += &note(480).replace(
                    "<note>",
                    if overrides {
                        "<note dynamics=\"100\">"
                    } else {
                        "<note>"
                    },
                );
            }
            parts += &format!("<part id=\"P{part}\"><measure><attributes><divisions>480</divisions></attributes>{body}</measure></part>");
        }
        let source = musicxml::parse(format!("<score-partwise version=\"4.0\"><part-list>{declarations}</part-list>{parts}</score-partwise>").as_bytes()).unwrap();
        let index = performance::normalize(&source).unwrap();
        assert_eq!(index.bindings.len(), 384);
        for binding in index.bindings.values() {
            let intensity = binding.intensity.as_ref().unwrap();
            let expected = if overrides {
                2.5
            } else if intensity.start.as_f64() as i64 % 2 == 0 {
                -7.75
            } else {
                4.0
            };
            close(db(intensity.evaluate(intensity.start)), expected);
        }
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let out = convert_midi_with_target(&source, "english", None, target);
            assert!(out.ok, "{:?}", out.msg);
            let project = out.svp.unwrap();
            assert_eq!(
                project.tracks.iter().map(|t| t.notes.len()).sum::<usize>(),
                384
            );
            target::serialize_to(target, &project).unwrap();
        }
    }
}

#[test]
fn cumulative_raw_xml_copies_are_bounded_before_large_ramp_growth() {
    // This formerly fit the event-work estimate while retaining about a GiB of XML.
    let source = ramp_resource_source(1000, 1024 * 1024);
    let error = performance::normalize(&source).unwrap_err();
    assert!(
        error.contains("SCORE_INTENSITY_LIMIT") && error.contains("provenance"),
        "{error}"
    );
    // A normal chain keeps every contributor and its established serde field shape.
    let source = ramp_resource_source(24, 0);
    let curve = timeline(&source);
    assert!(matches!(
        curve
            .segments
            .iter()
            .find(|s| matches!(s.curve, Curve::Transition { .. }))
            .unwrap()
            .curve,
        Curve::Transition { .. }
    ));
    let last = curve.segments.last().unwrap().provenance.as_ref().unwrap();
    assert!(last.evidence.len() >= 25);
    let json = serde_json::to_value(last).unwrap();
    assert!(json["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["raw_fields"]["xml"].is_string()));
}
