//! EXP-003 acceptance through actual loaders, projection and editable targets.
use verse_lib::engine::{
    convert::convert_midi_with_target,
    midi, musescore, musicxml,
    performance::PerformanceOwner,
    target::{self, ustx, ExportTarget},
};

fn project(
    bytes: &[u8],
    ms: bool,
) -> (
    midi::Midi,
    verse_lib::engine::convert::ConvertOutcome,
    ustx::UstxProject,
) {
    let source = if ms {
        musescore::parse(bytes)
    } else {
        musicxml::parse(bytes)
    }
    .unwrap();
    let outcome = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(outcome.ok, "{:?}", outcome.msg);
    let target = ustx::serialize(outcome.svp.as_ref().unwrap()).unwrap();
    (source, outcome, target)
}
fn dyn_at(project: &ustx::UstxProject, t: i32) -> i32 {
    let curve = project.voice_parts[0]
        .curves
        .iter()
        .find(|c| c.abbr == "dyn")
        .expect("active DYN");
    match curve.xs.binary_search(&t) {
        Ok(i) => curve.ys[i],
        Err(i) if i > 0 && i < curve.xs.len() => {
            let x = f64::from(t - curve.xs[i - 1]) / f64::from(curve.xs[i] - curve.xs[i - 1]);
            (f64::from(curve.ys[i - 1]) + x * f64::from(curve.ys[i] - curve.ys[i - 1]))
                .round_ties_even() as i32
        }
        _ => 0,
    }
}
const XML: &[u8] = include_bytes!("score-intensity-fixtures/contour.musicxml");
#[test]
fn contour_actual_loaders_and_compressed_parity() {
    let cases: [(&[u8], bool); 6] = [
        (XML, false),
        (
            include_bytes!("score-intensity-fixtures/contour.mxl"),
            false,
        ),
        (
            include_bytes!("score-intensity-fixtures/contour-legacy.mscx"),
            true,
        ),
        (
            include_bytes!("score-intensity-fixtures/contour-legacy.mscz"),
            true,
        ),
        (
            include_bytes!("score-intensity-fixtures/contour-modern.mscx"),
            true,
        ),
        (
            include_bytes!("score-intensity-fixtures/contour-modern.mscz"),
            true,
        ),
    ];
    for (bytes, ms) in cases {
        let (_, outcome, target) = project(bytes, ms);
        assert_eq!(
            [0, 480, 720, 960, 1440].map(|t| dyn_at(&target, t)),
            [-78, -78, -19, 40, 40]
        );
        let notes = &outcome.svp.as_ref().unwrap().tracks[0].notes;
        assert_eq!(
            notes
                .iter()
                .map(|n| (n.onset_ticks, n.duration_ticks, n.pitch))
                .collect::<Vec<_>>(),
            vec![(0, 480, 60), (480, 480, 62), (1440, 480, 64)]
        );
        for n in notes {
            assert!(matches!(
                n.performance.as_ref().unwrap().owner,
                PerformanceOwner::Score { .. }
            ));
            assert_eq!(
                n.performance.as_ref().unwrap().source_id,
                n.source_evidence.as_ref().unwrap().note_id
            );
        }
        for phase in 0..5 {
            for t in (480 + phase..960).step_by(5) {
                let ideal = -77.5 + 117.5 * f64::from(t - 480) / 480.0;
                assert!((f64::from(dyn_at(&target, t)) - ideal).abs() <= 1.0);
            }
        }
    }
}
#[test]
fn numeric_override_is_one_absolute_anchor() {
    let (_, _, target) = project(
        include_bytes!("score-intensity-fixtures/numeric-override.musicxml"),
        false,
    );
    assert_eq!(dyn_at(&target, 0), 70); // note dynamics120 => L108, not sound90 times note108.
}
#[test]
fn niente_is_active_positive_until_exact_zero() {
    let (_, outcome, target) = project(
        include_bytes!("score-intensity-fixtures/niente.musicxml"),
        false,
    );
    assert_eq!(dyn_at(&target, 960), -240);
    for t in 480..960 {
        assert!(dyn_at(&target, t) > -240, "positive tail muted at {t}");
    }
    assert!(outcome
        .projection
        .performance_spans
        .iter()
        .any(|s| s.intensity.is_some()));
}
#[test]
fn svp_retains_intent_without_claiming_automation() {
    let (source, _, _) = project(XML, false);
    let outcome = convert_midi_with_target(&source, "english", None, ExportTarget::Svp);
    assert!(outcome.ok, "{:?}", outcome.msg);
    let reports =
        target::performance_report(ExportTarget::Svp, outcome.svp.as_ref().unwrap()).unwrap();
    assert!(reports.iter().any(|r| r.intensity.is_some()));
    assert!(reports
        .iter()
        .all(|r| r.status != verse_lib::engine::performance::TransferStatus::Mapped));
}
#[test]
fn offsets_obey_playback_flag_and_sound_precedence() {
    let xml = String::from_utf8(XML.to_vec()).unwrap();
    for (offset, expected) in [
        ("<offset>240</offset>", -78),
        ("<offset sound=\"no\">240</offset>", -78),
        ("<offset sound=\"yes\">240</offset>", 0),
    ] {
        let modified = xml.replacen("</direction>", &format!("{offset}</direction>"), 1);
        let (_, _, target) = project(modified.as_bytes(), false);
        assert_eq!(dyn_at(&target, 0), expected);
        assert_eq!(dyn_at(&target, 240), -78);
    }
}

fn xml(body: &str) -> String {
    format!("<score-partwise version=\"4.0\"><part-list><score-part id=\"P1\"><part-name>Voice</part-name></score-part></part-list><part id=\"P1\">{body}</part></score-partwise>")
}
fn note(duration: u32, extra: &str) -> String {
    format!("<note {extra}><pitch><step>C</step><octave>4</octave></pitch><duration>{duration}</duration><lyric><text>la</text></lyric></note>")
}
fn direction(mark: &str, offset: i32) -> String {
    format!("<direction><direction-type><dynamics><{mark}/></dynamics></direction-type><offset sound=\"yes\">{offset}</offset></direction>")
}
fn wedge(kind: &str, offset: i32, extra: &str) -> String {
    format!("<direction><direction-type><wedge type=\"{kind}\" {extra}/></direction-type><offset sound=\"yes\">{offset}</offset></direction>")
}
fn measure(body: &str) -> String {
    format!("<measure><attributes><divisions>480</divisions></attributes>{body}</measure>")
}

fn ms_score(body: &str, modern: bool) -> String {
    let (version, program) = if modern {
        ("4.70", "4.7.4")
    } else {
        ("3.02", "3.6.2")
    };
    format!("<museScore version=\"{version}\"><programVersion>{program}</programVersion><Score><Division>480</Division><Part id=\"1\"><trackName>Voice</trackName><Staff id=\"1\"/><Instrument id=\"voice\"><instrumentId>voice.soprano</instrumentId></Instrument></Part><Staff id=\"1\">{body}</Staff></Score></museScore>")
}
fn ms_note(pitch: u8, duration: &str, extra: &str) -> String {
    format!("<Chord><durationType>{duration}</durationType><Lyrics><text>la</text></Lyrics><Note><pitch>{pitch}</pitch>{extra}</Note></Chord>")
}
fn reports_for(
    source: &midi::Midi,
    export: ExportTarget,
) -> Vec<verse_lib::engine::performance::PerformanceTransfer> {
    let outcome = convert_midi_with_target(source, "english", None, export);
    assert!(outcome.ok, "{:?}", outcome.msg);
    target::performance_report(export, outcome.svp.as_ref().unwrap()).unwrap()
}

#[test]
fn actual_musescore_scope_assignments_and_text_defaults() {
    for (modern, element, assignment, expected) in [
        (false, "Dynamic", "<dynType>0</dynType>", vec![60, 62]),
        (false, "Dynamic", "<dynType>1</dynType>", vec![60, 62, 64]),
        (
            false,
            "Dynamic",
            "<dynType>2</dynType>",
            vec![60, 62, 64, 65],
        ),
        (
            true,
            "Dynamic",
            "<voiceAssignment>currentVoiceOnly</voiceAssignment>",
            vec![60],
        ),
        (
            true,
            "Dynamic",
            "<voiceAssignment>allInStaff</voiceAssignment>",
            vec![60, 62],
        ),
        (
            true,
            "Dynamic",
            "<voiceAssignment>allInInstrument</voiceAssignment>",
            vec![60, 62, 64],
        ),
        (false, "StaffText", "", vec![60, 62]),
        (true, "StaffText", "", vec![60, 62]),
        (false, "SystemText", "", vec![60, 62, 64, 65]),
        (true, "SystemText", "", vec![60, 62, 64, 65]),
        (
            true,
            "SystemText",
            "<voiceAssignment>currentVoiceOnly</voiceAssignment>",
            vec![60],
        ),
    ] {
        let content = if element == "Dynamic" {
            "<subtype>p</subtype>"
        } else {
            "<text>fade out</text>"
        };
        let declaration = format!("<{element}>{content}{assignment}</{element}>");
        let body = format!(
            "<Measure><voice>{declaration}{}</voice><voice>{}</voice></Measure>",
            ms_note(60, "quarter", ""),
            ms_note(62, "quarter", "")
        );
        let score = ms_score(&body,modern)
            .replace("<Staff id=\"1\"/><Instrument", "<Staff id=\"1\"/><Staff id=\"2\"/><Instrument")
            .replace("</Part><Staff", "</Part><Part id=\"2\"><trackName>Other</trackName><Staff id=\"3\"/><Instrument id=\"voice\"><instrumentId>voice.soprano</instrumentId></Instrument></Part><Staff")
            .replace("</Score>", &format!("<Staff id=\"2\"><Measure><voice>{}</voice></Measure></Staff><Staff id=\"3\"><Measure><voice>{}</voice></Measure></Staff></Score>",ms_note(64,"quarter",""),ms_note(65,"quarter","")));
        let (_, _, target) = project(score.as_bytes(), true);
        let mut affected: Vec<_> = target
            .voice_parts
            .iter()
            .filter(|p| p.curves.iter().any(|c| c.abbr == "dyn"))
            .map(|p| p.notes[0].tone)
            .collect();
        affected.sort();
        assert_eq!(affected, expected, "{modern}/{element}/{assignment}");
        assert_eq!(
            target
                .voice_parts
                .iter()
                .map(|p| p.notes.len())
                .sum::<usize>(),
            4
        );
    }
}

#[test]
fn legacy_spanner_uses_available_endpoint_and_diagnoses_conflicts() {
    for (ticks, ending, valid) in [
        ("480", "", true),
        ("960", "<endSpanner id=\"h\"/>", false),
        ("bad", "<endSpanner id=\"h\"/>", true),
    ] {
        let body = format!("<Measure><voice><Dynamic><subtype>p</subtype></Dynamic><HairPin id=\"h\"><subtype>0</subtype><ticks>{ticks}</ticks></HairPin>{}{ending}{}</voice></Measure>",ms_note(60,"quarter",""),ms_note(62,"quarter",""));
        let (source, _, target) = project(ms_score(&body, false).as_bytes(), true);
        if valid {
            assert!(dyn_at(&target, 240) > -78, "{ticks}");
        } else {
            assert!(reports_for(&source, ExportTarget::Ustx)
                .iter()
                .any(|r| r.message.to_lowercase().contains("conflicting")));
        }
        if ticks == "bad" {
            assert!(reports_for(&source, ExportTarget::Svp)
                .iter()
                .any(|r| r.message.contains("ticks") || r.message.contains("integer")));
        }
    }
}

#[test]
fn extreme_spanner_measure_displacements_are_local_diagnostics() {
    for displacement in [i64::MAX, i64::MIN, -2] {
        let first = format!(
            "<Measure><voice>{}</voice></Measure>",
            ms_note(60, "quarter", "")
        );
        let second=format!("<Measure><voice><Spanner type=\"HairPin\"><HairPin><subtype>0</subtype></HairPin><next><location><measures>{displacement}</measures><fractions>1/4</fractions></location></next></Spanner>{}</voice></Measure>",ms_note(62,"quarter",""));
        let (source, _, _) = project(ms_score(&(first + &second), false).as_bytes(), true);
        assert!(
            reports_for(&source, ExportTarget::Ustx)
                .iter()
                .any(|r| r.message.contains("span") || r.message.contains("displacement")),
            "{displacement}"
        );
    }
}

#[test]
fn optional_exact_tempo_preserves_nominal_scientific_and_long_decimal_inputs() {
    for raw in ["1e0", "1.0000000000000000001", "1.0000000000000000000"] {
        let body = format!(
            "<Measure><voice><Tempo><tempo>{raw}</tempo></Tempo>{}</voice></Measure>",
            ms_note(60, "quarter", "")
        );
        let (source, _, target) = project(ms_score(&body, false).as_bytes(), true);
        assert!(source.score_intensity.is_none(), "{raw}");
        assert!(target.voice_parts.iter().all(|p| p.curves.is_empty()));
        assert!(source
            .tracks
            .iter()
            .flat_map(|t| &t.events)
            .any(|e| matches!(e.kind, midi::Kind::Tempo(1_000_000))));
        let compound = body
            .replace(
                "</Tempo>",
                "</Tempo><Dynamic><subtype>fp</subtype></Dynamic>",
            )
            .replace(
                "</voice>",
                &format!(
                    "<Dynamic><subtype>p</subtype></Dynamic>{}</voice>",
                    ms_note(62, "quarter", "")
                ),
            );
        let (source, _, target) = project(ms_score(&compound, false).as_bytes(), true);
        assert_eq!(dyn_at(&target, 480), -78);
        let reports = reports_for(&source, ExportTarget::Svp);
        let precision = reports.iter().any(|r| r.message.contains("precision"));
        assert_eq!(
            precision,
            raw == "1.0000000000000000001",
            "{raw}: {reports:?}"
        );
        if !precision {
            assert_eq!(dyn_at(&target, 0), 40);
        }
    }
}

#[test]
fn attack_only_reference_does_not_jump_at_a_held_endpoint() {
    let head = ms_note(60, "half", "").replace("</durationType>", "</durationType><dots>1</dots>");
    let body=format!("<Measure><voice>{head}{}</voice><voice><Rest><durationType>quarter</durationType></Rest><HairPin><subtype>0</subtype><ticks>480</ticks><singleNoteDynamics>0</singleNoteDynamics></HairPin></voice></Measure>",ms_note(62,"quarter",""));
    let (_, _, target) = project(ms_score(&body, false).as_bytes(), true);
    assert_eq!(
        [0, 480, 720, 960, 1439, 1440].map(|t| dyn_at(&target, t)),
        [0, 0, 0, 0, 0, 40]
    );
}

#[test]
fn diagnostic_only_bindings_remain_specific_without_automation() {
    let unresolved = xml(&measure(&format!(
        "{}{}",
        wedge("crescendo", 0, ""),
        note(480, "")
    )));
    let disabled=ms_score(&format!("<Measure><voice><Dynamic><subtype>p</subtype><play>0</play></Dynamic>{}</voice></Measure>",ms_note(60,"quarter","")),true);
    let unknown = disabled.replace(
        "<play>0</play>",
        "<voiceAssignment>futureScope</voiceAssignment>",
    );
    for (text, ms, reason) in [
        (unresolved, false, "wedge"),
        (disabled, true, "disabled"),
        (unknown, true, "scope"),
    ] {
        let (source, _, target) = project(text.as_bytes(), ms);
        assert!(
            target.voice_parts.iter().all(|p| p.curves.is_empty()),
            "{reason}"
        );
        for export in [ExportTarget::Svp, ExportTarget::Ustx] {
            let reports = reports_for(&source, export);
            assert!(
                reports
                    .iter()
                    .any(|r| r.message.to_lowercase().contains(reason)),
                "{reason}: {reports:?}"
            );
            assert!(reports
                .iter()
                .all(|r| r.status != verse_lib::engine::performance::TransferStatus::Mapped));
        }
    }
}

#[test]
fn held_gain_survives_short_notes_but_actual_short_pulses_are_limited() {
    let short_notes = format!(
        "{}{}{}{}",
        direction("p", 0),
        note(2, ""),
        note(2, ""),
        note(6, "")
    );
    let source = musicxml::parse(xml(&measure(&short_notes)).as_bytes()).unwrap();
    // The nominal USTX ten-tick note floor remains intact. Exercise its
    // performance report separately, then emitted DYN using eligible notes
    // with the same two-tick held-expression subdivisions at their boundary.
    let projected = convert_midi_with_target(&source, "english", None, ExportTarget::Svp);
    let reports =
        target::performance_report(ExportTarget::Ustx, projected.svp.as_ref().unwrap()).unwrap();
    assert!(reports.iter().all(|r| !r.message.contains("shorter")));
    let body = format!(
        "{}{}{}{}{}",
        direction("p", 0),
        direction("p", 8),
        direction("p", 12),
        note(10, ""),
        note(10, "")
    );
    let (_, outcome, target) = project(xml(&measure(&body)).as_bytes(), false);
    assert!((0..20).all(|t| dyn_at(&target, t) == -78));
    assert!(outcome
        .projection
        .performance_spans
        .iter()
        .all(|s| !s.detail.contains("shorter")));
    let pulse = format!(
        "{}{}{}{}",
        direction("p", 0),
        direction("f", 4),
        note(10, ""),
        note(10, "")
    );
    let (_, outcome, target) = project(xml(&measure(&pulse)).as_bytes(), false);
    assert!(outcome
        .projection
        .performance_spans
        .iter()
        .any(|s| s.detail.contains("shorter")));
    assert_eq!(dyn_at(&target, 4), 40);
}

#[test]
fn fade_serialized_dyn_matches_independent_linear_gain_oracle_all_phases() {
    let (_, _, target) = project(
        include_bytes!("score-intensity-fixtures/niente.musicxml"),
        false,
    );
    let initial = 10f64.powf(-7.75 / 20.0);
    for phase in 0..5 {
        for tick in (480 + phase..960).step_by(5) {
            let gain = initial * (1.0 - f64::from(tick - 480) / 480.0);
            let expected = (200.0 * gain.log10()).max(-239.0);
            assert!(
                (f64::from(dyn_at(&target, tick)) - expected).abs() <= 1.0,
                "phase {phase}, tick {tick}"
            );
        }
    }
    assert_eq!(dyn_at(&target, 959), -239);
    assert_eq!(dyn_at(&target, 960), -240);
}

#[test]
fn early_velocity_anchor_reuses_only_declared_transition_reference() {
    let body = format!(
        "{}{}{}",
        wedge("crescendo", 480, ""),
        wedge("stop", 960, ""),
        note(1440, "dynamics=\"120\"")
    );
    let (_, outcome, target) = project(xml(&measure(&body)).as_bytes(), false);
    assert_eq!(
        [0, 480, 720, 960].map(|t| dyn_at(&target, t)),
        [70, 70, 90, 110]
    );
    assert!(outcome
        .projection
        .performance_spans
        .iter()
        .all(|s| !s.detail.contains("reference")));
}

#[test]
fn unavailable_attack_reference_is_local_and_recovers_at_a_genuine_attack() {
    for initial in ["", "<direction><sound dynamics=\"0\"/></direction>"] {
        let body = format!(
            "{initial}{}{}{}",
            direction("f", 480),
            note(960, "dynamics=\"120\""),
            note(480, "")
        );
        let (_, outcome, target) = project(xml(&measure(&body)).as_bytes(), false);
        assert_eq!(
            dyn_at(&target, 0),
            if initial.is_empty() { 70 } else { -240 }
        );
        assert_eq!(dyn_at(&target, 960), 40);
        let limitations: Vec<_> = outcome
            .projection
            .performance_spans
            .iter()
            .filter(|s| s.detail.contains("finite permitted attack reference"))
            .collect();
        assert!(!limitations.is_empty());
        assert!(limitations
            .iter()
            .all(|s| s.start_tick == 480 && s.end_tick == 960));
    }
}

#[test]
fn numeric_level_override_preserves_authored_compound_shape() {
    let body=format!("<direction><direction-type><dynamics><fp/></dynamics></direction-type><sound dynamics=\"100\"/></direction>{}",note(960,""));
    let (_, _, target) = project(xml(&measure(&body)).as_bytes(), false);
    assert_eq!(dyn_at(&target, 0), 25); // numeric L90 overrides f's level once.
    assert_eq!(dyn_at(&target, 480), -78); // MusicXML fp keeps its one-quarter shape.
}

#[test]
fn repeated_unmatched_wedge_reports_performed_occurrences_and_boundary_owner() {
    let first = measure(&format!(
        "<barline location=\"left\"><repeat direction=\"forward\"/></barline>{}",
        note(480, "")
    ));
    let second = measure(&format!(
        "{}{}<barline location=\"right\"><repeat direction=\"backward\" times=\"2\"/></barline>",
        wedge("crescendo", 0, ""),
        note(480, "")
    ));
    let source = musicxml::parse(xml(&(first + &second)).as_bytes()).unwrap();
    for export in [ExportTarget::Ustx, ExportTarget::Svp] {
        let outcome = convert_midi_with_target(&source, "english", None, export);
        assert!(outcome.ok, "{:?}", outcome.msg);
        let notes = &outcome.svp.as_ref().unwrap().tracks[0].notes;
        let reports = reports_for(&source, export);
        let issues: Vec<_> = reports
            .iter()
            .filter(|r| r.message.to_lowercase().contains("wedge"))
            .collect();
        assert_eq!(issues.len(), 2, "{issues:?}");
        for (report, note) in issues.iter().zip([&notes[1], &notes[3]]) {
            assert_eq!(report.start_tick, note.onset_ticks);
            assert_eq!(
                report.note_ids,
                vec![note.source_evidence.as_ref().unwrap().note_id.clone()]
            );
        }
        let first = issues[0].intensity.as_ref().unwrap();
        let second = issues[1].intensity.as_ref().unwrap();
        assert_ne!(
            first["provenance"]["occurrence"],
            second["provenance"]["occurrence"]
        );
        assert_ne!(
            first["provenance"]["repeat_pass"],
            second["provenance"]["repeat_pass"]
        );
    }
}

#[test]
fn ordinary_boundary_mark_is_never_owned_by_the_preceding_note() {
    let body = format!(
        "{}{}{}{}",
        direction("p", 0),
        direction("f", 480),
        note(480, ""),
        note(480, "")
    );
    let (source, outcome, _) = project(xml(&measure(&body)).as_bytes(), false);
    let field = source
        .score_intensity
        .as_ref()
        .unwrap()
        .score
        .events
        .iter()
        .find(|e| e.at.as_f64() == 1.0)
        .unwrap()
        .evidence
        .source_ids[0]
        .clone();
    let notes = &outcome.svp.as_ref().unwrap().tracks[0].notes;
    let previous = &notes[0].source_evidence.as_ref().unwrap().note_id;
    let next = &notes[1].source_evidence.as_ref().unwrap().note_id;
    for export in [ExportTarget::Ustx, ExportTarget::Svp] {
        let reports = reports_for(&source, export);
        let owned: Vec<_> = reports
            .iter()
            .filter(|r| r.source_ids.contains(&field))
            .collect();
        assert!(!owned.is_empty());
        assert!(
            owned.iter().all(|r| !r.note_ids.contains(previous)),
            "{owned:?}"
        );
        assert!(owned.iter().any(|r| r.note_ids.contains(next)));
    }
}

#[test]
fn svp_specific_issues_survive_without_changing_nominal_serialization() {
    let head = note(480, "dynamics=\"100\"").replace("<lyric>", "<tie type=\"start\"/><lyric>");
    let tail = note(480, "dynamics=\"120\"")
        .replace("<lyric><text>la</text></lyric>", "<tie type=\"stop\"/>");
    let tied = xml(&measure(&format!("{}{head}{tail}", direction("p", 0))));
    let easing =
        String::from_utf8(include_bytes!("score-intensity-fixtures/contour-legacy.mscx").to_vec())
            .unwrap()
            .replace(
                "<veloChangeMethod>normal</veloChangeMethod>",
                "<veloChangeMethod>future</veloChangeMethod>",
            );
    for (text, ms, reason) in [
        (tied, false, "continuation"),
        (easing, true, "velochangemethod"),
    ] {
        let (mut source, _, _) = project(text.as_bytes(), ms);
        let outcome = convert_midi_with_target(&source, "english", None, ExportTarget::Svp);
        let reports = reports_for(&source, ExportTarget::Svp);
        assert!(
            reports
                .iter()
                .any(|r| r.message.to_lowercase().contains(reason)),
            "{reports:?}"
        );
        assert!(reports
            .iter()
            .all(|r| r.status != verse_lib::engine::performance::TransferStatus::Mapped));
        let bytes =
            serde_json::to_vec(&target::svp::serialize(outcome.svp.as_ref().unwrap()).unwrap())
                .unwrap();
        source.score_intensity = None;
        let plain = convert_midi_with_target(&source, "english", None, ExportTarget::Svp);
        assert_eq!(
            bytes,
            serde_json::to_vec(&target::svp::serialize(plain.svp.as_ref().unwrap()).unwrap())
                .unwrap()
        );
    }
}

#[test]
fn modern_inactive_velocity_keeps_accent_contributors_and_ledger_mapping() {
    use verse_lib::bundle::{build_preservation_ledger, BundleLayout, PrimaryDisposition};
    use verse_lib::stems::StemPlan;
    let body = format!(
        "<Measure><voice><Dynamic><subtype>sfz</subtype></Dynamic>{}</voice></Measure>",
        ms_note(60, "quarter", "<velocity>0</velocity>")
    );
    let (source, outcome, target) = project(ms_score(&body, true).as_bytes(), true);
    assert_eq!(dyn_at(&target, 0), 80);
    let accent = source.score_intensity.as_ref().unwrap().score.events[0]
        .evidence
        .source_ids[0]
        .clone();
    let reports = reports_for(&source, ExportTarget::Ustx);
    assert!(reports.iter().any(|r| r.status
        == verse_lib::engine::performance::TransferStatus::Mapped
        && r.source_ids.contains(&accent)
        && r.source_ids.len() > 1));
    let layout = BundleLayout::new(
        std::path::Path::new("/tmp/Score.versebundle"),
        "score.mscx",
        ExportTarget::Ustx,
    )
    .unwrap();
    let stems = StemPlan::from_source(&source, &outcome.tracks).unwrap();
    let ledger = build_preservation_ledger(&source, &outcome.projection, &layout, &stems);
    let entry = ledger
        .entries
        .iter()
        .find(|e| e.source_id == accent)
        .unwrap();
    assert!(matches!(
        entry.disposition,
        PrimaryDisposition::ProjectedMapped { .. }
    ));
}

#[test]
fn partly_limited_score_field_has_both_ledger_summary_and_structured_references() {
    use verse_lib::bundle::{build_preservation_ledger, BundleLayout, PrimaryDisposition};
    use verse_lib::stems::StemPlan;
    let (source, outcome, _) = project(
        include_bytes!("score-intensity-fixtures/niente.musicxml"),
        false,
    );
    let layout = BundleLayout::new(
        std::path::Path::new("/tmp/Score.versebundle"),
        "score.musicxml",
        ExportTarget::Ustx,
    )
    .unwrap();
    let stems = StemPlan::from_source(&source, &outcome.tracks).unwrap();
    let ledger = build_preservation_ledger(&source, &outcome.projection, &layout, &stems);
    let mixed: Vec<_> = ledger
        .entries
        .iter()
        .filter(|e| {
            matches!(
                &e.disposition,
                PrimaryDisposition::ProjectedMapped {
                    limitations: Some(_),
                    ..
                }
            )
        })
        .collect();
    assert!(!mixed.is_empty());
    assert!(mixed.iter().any(|entry| {
        let spans: Vec<_> = entry
            .performance_refs
            .iter()
            .map(|i| &ledger.performance_spans[*i])
            .collect();
        spans
            .iter()
            .any(|s| s.status == verse_lib::engine::performance::TransferStatus::Mapped)
            && spans.iter().any(|s| {
                s.status == verse_lib::engine::performance::TransferStatus::RepresentationLimit
                    && s.intensity.is_some()
            })
    }));
    assert!(serde_json::to_string(&ledger)
        .unwrap()
        .contains("limitedNienteTail"));
}

#[test]
fn final_endpoint_evidence_is_explicit_and_has_no_preceding_note_id() {
    use verse_lib::bundle::{build_preservation_ledger, BundleLayout};
    use verse_lib::stems::StemPlan;
    let body = format!(
        "{}{}{}",
        direction("p", 0),
        direction("f", 480),
        note(480, "")
    );
    let (source, _, target) = project(xml(&measure(&body)).as_bytes(), false);
    assert_eq!(dyn_at(&target, 480), 40);
    let field = source
        .score_intensity
        .as_ref()
        .unwrap()
        .score
        .events
        .iter()
        .find(|e| e.at.as_f64() == 1.0)
        .unwrap()
        .evidence
        .source_ids[0]
        .clone();
    for export in [ExportTarget::Svp, ExportTarget::Ustx] {
        let reports = reports_for(&source, export);
        let endpoint = reports
            .iter()
            .find(|r| r.source_ids.contains(&field))
            .unwrap();
        assert_eq!((endpoint.start_tick, endpoint.end_tick), (480, 480));
        assert!(endpoint.note_ids.is_empty());
        assert_eq!(
            endpoint.intensity.as_ref().unwrap()["terminalEndpoint"],
            true
        );
        let outcome = convert_midi_with_target(&source, "english", None, export);
        let layout = BundleLayout::new(
            std::path::Path::new("/tmp/Terminal.versebundle"),
            "score.musicxml",
            export,
        )
        .unwrap();
        let stems = StemPlan::from_source(&source, &outcome.tracks).unwrap();
        let ledger = build_preservation_ledger(&source, &outcome.projection, &layout, &stems);
        let allowed = ledger
            .entries
            .iter()
            .flat_map(|e| e.artifact_paths.iter().cloned())
            .collect();
        ledger.validate(&allowed).unwrap();
    }
}

#[test]
fn dense_actual_timeline_preserves_boundaries_and_target_work_is_bounded() {
    let mut body = String::new();
    for i in 0..64 {
        let start = i * 120;
        body += &direction(if i % 2 == 0 { "p" } else { "f" }, start);
        body += &wedge(
            if i % 2 == 0 {
                "crescendo"
            } else {
                "diminuendo"
            },
            start,
            "",
        );
        body += &wedge("stop", start + 120, "");
    }
    body += &direction("p", 7680);
    body += &note(7680, "");
    let (_, _, target) = project(xml(&measure(&body)).as_bytes(), false);
    for i in 0..64 {
        assert_eq!(dyn_at(&target, i * 120), if i % 2 == 0 { -78 } else { 40 });
        assert_eq!(dyn_at(&target, i * 120 + 60), -19);
    }
    let huge = format!(
        "{}{}{}{}{}",
        direction("p", 0),
        wedge("crescendo", 0, ""),
        wedge("stop", 300_000, ""),
        direction("f", 300_000),
        note(300_000, "")
    );
    let source = musicxml::parse(xml(&measure(&huge)).as_bytes()).unwrap();
    let outcome = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(!outcome.ok);
    assert!(
        outcome
            .msg
            .as_deref()
            .unwrap()
            .contains("PERFORMANCE_LIMIT"),
        "{:?}",
        outcome.msg
    );
}

#[test]
fn automatic_endpoints_and_wrong_direction_keep_active_ramps() {
    for (kind, mark, end) in [
        ("crescendo", None, -40),
        ("diminuendo", None, -118),
        ("crescendo", Some("pp"), -118),
    ] {
        let body = format!(
            "{}{}{}{}{}",
            direction("p", 0),
            wedge(kind, 0, ""),
            wedge("stop", 480, ""),
            mark.map(|m| direction(m, 480)).unwrap_or_default(),
            note(960, "")
        );
        let (_, outcome, target) = project(xml(&measure(&body)).as_bytes(), false);
        assert_eq!(dyn_at(&target, 0), -78);
        assert_eq!(dyn_at(&target, 480), end);
        if mark.is_some() {
            assert!(dyn_at(&target, 240) > -78);
            assert!(outcome
                .projection
                .performance_spans
                .iter()
                .any(|s| s.status == verse_lib::engine::performance::TransferStatus::Unsupported));
        }
    }
}
#[test]
fn repeat_destinations_and_time_only_use_source_pass() {
    let first=measure(&format!("<barline location=\"left\"><repeat direction=\"forward\"/></barline>{}<direction><sound dynamics=\"100\" time-only=\"2\"><offset>240</offset></sound></direction>{}",direction("p",0),note(480,"")));
    let second = measure(&format!(
        "{}{}<barline location=\"right\"><repeat direction=\"backward\" times=\"2\"/></barline>",
        direction("f", 0),
        note(480, "")
    ));
    let (_, _, target) = project(xml(&(first + &second)).as_bytes(), false);
    assert_eq!(
        [0, 480, 960, 1200, 1440].map(|t| dyn_at(&target, t)),
        [-78, 40, -78, 25, 40]
    );
}
#[test]
fn sound_offset_beats_visual_and_direction_offsets() {
    let body=format!("<direction><direction-type><dynamics><p/></dynamics></direction-type><offset sound=\"yes\">120</offset><sound dynamics=\"100\"><offset>240</offset></sound></direction>{}",note(480,""));
    let (_, _, target) = project(xml(&measure(&body)).as_bytes(), false);
    assert_eq!(dyn_at(&target, 0), 0);
    assert_eq!(dyn_at(&target, 120), 0);
    assert_eq!(dyn_at(&target, 240), 25);
}
#[test]
fn positive_floor_is_local_to_niente_tail() {
    let body = format!(
        "{}{}{}{}",
        direction("pppppp", 0),
        wedge("diminuendo", 0, ""),
        wedge("stop", 480, "niente=\"yes\""),
        note(480, "")
    );
    let (_, outcome, target) = project(xml(&measure(&body)).as_bytes(), false);
    let tails: Vec<_> = outcome
        .projection
        .performance_spans
        .iter()
        .filter(|s| {
            s.intensity
                .as_ref()
                .and_then(|e| e.get("limitedNienteTail"))
                .and_then(|v| v.as_bool())
                == Some(true)
        })
        .collect();
    assert!(!tails.is_empty());
    assert!(tails.iter().all(|s| s.start_tick > 0));
    assert_eq!(dyn_at(&target, 479), -239);
    assert_eq!(dyn_at(&target, 480), -240);
}
#[test]
fn decimal_note_override_and_invalid_note_override_are_local() {
    for value in ["0.01", "0", "bad"] {
        let body = format!(
            "{}{}{}",
            direction("mf", 0),
            note(480, &format!("dynamics=\"{value}\"")),
            note(480, "")
        );
        let (_, outcome, target) = project(xml(&measure(&body)).as_bytes(), false);
        assert_eq!(dyn_at(&target, 480), 0);
        match value {
            "0.01" => assert_eq!(dyn_at(&target, 0), -200),
            "0" => assert_eq!(dyn_at(&target, 0), -240),
            _ => {
                assert!(outcome.projection.performance_spans.iter().any(
                    |s| s.status == verse_lib::engine::performance::TransferStatus::Unsupported
                ))
            }
        }
    }
}
#[test]
fn plain_score_is_expression_free_and_metadata_is_preserved() {
    let source = musicxml::parse(xml(&measure(&note(480, ""))).as_bytes()).unwrap();
    assert!(source.score_intensity.is_none());
    assert!(source.staff_links.is_empty());
    let outcome = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(outcome.ok, "{:?}", outcome.msg);
    assert!(outcome.source_warnings.is_empty());
    let project = outcome.svp.unwrap();
    assert!(project
        .tracks
        .iter()
        .flat_map(|t| &t.notes)
        .all(|n| n.performance.is_none() && n.source_evidence.is_some()));
    assert!(ustx::serialize(&project)
        .unwrap()
        .voice_parts
        .iter()
        .all(|p| p.curves.is_empty()));
}

#[test]
fn emit_native_score_consumer_fixtures_with_independent_oracles() {
    let Ok(directory) = std::env::var("VERSE_SCORE_PERFORMANCE_PROBE_DIR") else {
        return;
    };
    let directory = std::path::Path::new(&directory);
    std::fs::create_dir_all(directory).unwrap();
    for (name, bytes, ms, niente) in [
        ("score-xml", XML, false, false),
        (
            "score-legacy",
            include_bytes!("score-intensity-fixtures/contour-legacy.mscx").as_slice(),
            true,
            false,
        ),
        (
            "score-modern",
            include_bytes!("score-intensity-fixtures/contour-modern.mscx").as_slice(),
            true,
            false,
        ),
        (
            "score-niente",
            include_bytes!("score-intensity-fixtures/niente.musicxml").as_slice(),
            false,
            true,
        ),
    ] {
        let (_, _, model) = project(bytes, ms);
        let gains: Vec<_> = (0..=1920)
            .map(|tick| {
                let p = 10f64.powf(-7.75 / 20.0);
                if tick < 480 {
                    p
                } else if niente {
                    p * (1.0 - f64::from(tick - 480) / 480.0).max(0.0)
                } else {
                    10f64.powf((-7.75 + 11.75 * (f64::from(tick - 480) / 480.0).min(1.0)) / 20.0)
                }
            })
            .collect();
        std::fs::write(
            directory.join(format!("{name}.ustx")),
            ustx::to_yaml(&model),
        )
        .unwrap();
        std::fs::write(
            directory.join(format!("{name}.expected.json")),
            serde_json::to_vec(
                // Domain and note count come from the written fixtures above,
                // independently of the candidate's serialized note inventory.
                &serde_json::json!({
                    "schema":1,"noteCount":3,"ticksPerQuarter":480,
                    "startTick":0,"endTick":1920,"tickStep":1,"gains":gains,
                    "nienteFadeIntervals": if niente {
                        vec![serde_json::json!({"startTick":480,"endTick":960,"direction":"out"})]
                    } else { vec![] },
                }),
            )
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn continuation_binding_keeps_tail_identity_and_original_attack() {
    let body = format!(
        "{}{}{}{}{}{}",
        direction("p", 0),
        wedge("crescendo", 0, ""),
        wedge("stop", 960, ""),
        direction("f", 960),
        note(480, "dynamics=\"100\""),
        note(480, "dynamics=\"100\"")
    );
    let (_, mut outcome, _) = project(xml(&measure(&body)).as_bytes(), false);
    let notes = &mut outcome.svp.as_mut().unwrap().tracks[0].notes;
    let head = notes[0].performance.clone().unwrap();
    let tail_evidence = notes[1].source_evidence.clone();
    let tail_id = notes[1].performance.as_ref().unwrap().source_id.clone();
    notes[1]
        .performance
        .as_mut()
        .unwrap()
        .continue_tied_intensity(&head)
        .unwrap();
    assert_eq!(notes[1].source_evidence, tail_evidence);
    let tail = notes[1].performance.as_ref().unwrap();
    assert_eq!(tail.source_id, tail_id);
    assert!(std::sync::Arc::ptr_eq(
        &head.intensity.as_ref().unwrap().timeline,
        &tail.intensity.as_ref().unwrap().timeline
    ));
    let at = verse_lib::engine::score_intensity::Fraction::integer(1);
    assert_eq!(
        tail.intensity.as_ref().unwrap().evaluate(at),
        head.intensity.as_ref().unwrap().evaluate(at)
    );
    assert_ne!(tail.source_id, head.source_id);
}

#[test]
fn part_and_explicit_staff_scopes_never_follow_midi_channels() {
    let body=format!("<direction><direction-type><dynamics><p/></dynamics></direction-type><staff>1</staff></direction>{}<backup><duration>480</duration></backup>{}",
        note(480,"").replace("<lyric>","<staff>1</staff><voice>1</voice><lyric>"),
        note(480,"").replace("<lyric>","<staff>2</staff><voice>1</voice><lyric>"));
    let source = musicxml::parse(xml(&measure(&body)).as_bytes()).unwrap();
    let performance = verse_lib::engine::performance::normalize(&source).unwrap();
    let staff_one = source
        .tracks
        .iter()
        .find(|t| t.source.staff_id.as_deref() == Some("1"))
        .unwrap();
    let staff_two = source
        .tracks
        .iter()
        .find(|t| t.source.staff_id.as_deref() == Some("2"))
        .unwrap();
    assert!(performance
        .bindings
        .keys()
        .any(|(id, _)| id == &staff_one.id));
    assert!(!performance
        .bindings
        .keys()
        .any(|(id, _)| id == &staff_two.id));
    assert!(performance.channels.is_empty());
}

#[test]
fn source_velocity_changes_and_modern_zero_are_versioned() {
    let legacy =
        String::from_utf8(include_bytes!("score-intensity-fixtures/contour-legacy.mscx").to_vec())
            .unwrap();
    for (version, program, velocity, expected) in [
        ("3.02", "3.6.2", "0", -198),
        ("4.70", "4.7.4", "0", -78),
        ("3.02", "3.6.2", "100", 50),
    ] {
        let score = legacy
            .replace("version=\"3.02\"", &format!("version=\"{version}\""))
            .replace(
                "3.6.2</programVersion>",
                &format!("{program}</programVersion>"),
            )
            .replacen(
                "<pitch>60</pitch>",
                &format!(
                    "<pitch>60</pitch><velocity>{velocity}</velocity><veloType>user</veloType>"
                ),
                1,
            );
        let (_, _, target) = project(score.as_bytes(), true);
        assert_eq!(dyn_at(&target, 0), expected);
    }
    let customized=legacy.replacen("<velocity>-1</velocity>","<velocity>70</velocity><veloChange>20</veloChange><veloChangeSpeed>normal</veloChangeSpeed>",1);
    let (_, _, target) = project(customized.as_bytes(), true);
    assert_eq!(dyn_at(&target, 0), -25);
    assert_eq!(dyn_at(&target, 384), 25);
}

#[test]
fn musescore_instance_easing_and_attack_only_are_active() {
    let legacy =
        String::from_utf8(include_bytes!("score-intensity-fixtures/contour-legacy.mscx").to_vec())
            .unwrap();
    let held = legacy.replace(
        "<veloChangeMethod>normal</veloChangeMethod>",
        "<veloChangeMethod>normal</veloChangeMethod><singleNoteDynamics>0</singleNoteDynamics>",
    );
    let (_, _, target) = project(held.as_bytes(), true);
    assert_eq!(dyn_at(&target, 720), -78);
    assert_eq!(dyn_at(&target, 1440), 40);
    let eased = legacy.replace(
        "<veloChangeMethod>normal</veloChangeMethod>",
        "<veloChangeMethod>ease-in</veloChangeMethod>",
    );
    let (_, _, target) = project(eased.as_bytes(), true);
    let expected = (-77.5 + 117.5 * (1.0 - (std::f64::consts::PI / 4.0).cos())).round() as i32;
    assert_eq!(dyn_at(&target, 720), expected);
    let delta = legacy.replace("<veloChange>0</veloChange>", "<veloChange>20</veloChange>");
    let (_, outcome, target) = project(delta.as_bytes(), true);
    assert_eq!(dyn_at(&target, 720), -53);
    assert_eq!(dyn_at(&target, 960), 40);
    assert!(outcome
        .projection
        .performance_spans
        .iter()
        .any(|s| s.detail.contains("endpoint") || s.detail.contains("boundary")));
}

#[test]
fn recognized_standalone_fade_infers_final_sounding_end() {
    let body = format!(
        "{}<direction><direction-type><words>fade out</words></direction-type></direction>{}",
        direction("p", 0),
        note(480, "")
    );
    let (_, _, target) = project(xml(&measure(&body)).as_bytes(), false);
    assert_eq!(dyn_at(&target, 0), -78);
    assert!(dyn_at(&target, 240) < -78);
    assert_eq!(dyn_at(&target, 480), -240);
}

#[test]
fn owning_wedge_fade_label_is_not_lost_or_duplicated() {
    let source =
        String::from_utf8(include_bytes!("score-intensity-fixtures/niente.musicxml").to_vec())
            .unwrap()
            .replace("niente=\"yes\"", "")
            .replace(
                "<wedge type=\"diminuendo\"",
                "<words>fade out</words><wedge type=\"diminuendo\"",
            );
    let (source, _, target) = project(source.as_bytes(), false);
    assert_eq!(dyn_at(&target, 960), -240);
    let input = source.score_intensity.unwrap();
    assert_eq!(
        input
            .score
            .events
            .iter()
            .filter(|e| matches!(
                e.instruction,
                verse_lib::engine::score_intensity::Instruction::Transition(_)
            ))
            .count(),
        1
    );
    assert!(!input.score.events.iter().any(|e| matches!(
        e.instruction,
        verse_lib::engine::score_intensity::Instruction::Text { .. }
    )));
}

/// Private bytes remain outside version control. Use the parent's read-only
/// inventory as the one source list; this test does not create a second report.
#[test]
#[ignore = "requires the private 22-case score inventory and its source files"]
fn private_inventory_loaders_nominal_parity_and_expression_coverage() {
    use sha2::{Digest, Sha256};
    let inventory_path = std::env::var("VERSE_SCORE_INVENTORY").expect("VERSE_SCORE_INVENTORY");
    let inventory: serde_json::Value =
        serde_json::from_slice(&std::fs::read(inventory_path).unwrap()).unwrap();
    let cases = inventory["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 22);
    let mut active = 0;
    for case in cases {
        let path = case["path"].as_str().unwrap();
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            case["sha256"].as_str().unwrap()
        );
        let mut source = if case["format"] == "MuseScore" {
            musescore::parse(&bytes)
        } else {
            musicxml::parse(&bytes)
        }
        .unwrap();
        let expressed_svp = convert_midi_with_target(&source, "english", None, ExportTarget::Svp);
        let expressed_ustx = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
        let retained = source.score_intensity.take();
        let plain_svp = convert_midi_with_target(&source, "english", None, ExportTarget::Svp);
        let plain_ustx = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
        assert_eq!(expressed_svp.ok, plain_svp.ok, "{path}");
        assert_eq!(
            expressed_ustx.ok, plain_ustx.ok,
            "{path}: {:?}",
            expressed_ustx.msg
        );
        if let (Some(a), Some(b)) = (&expressed_svp.svp, &plain_svp.svp) {
            assert_eq!(
                target::serialize_to(ExportTarget::Svp, a).unwrap(),
                target::serialize_to(ExportTarget::Svp, b).unwrap(),
                "SVP nominal bytes: {path}"
            );
        }
        let mut curve_count = 0;
        if let (Some(a), Some(b)) = (&expressed_ustx.svp, &plain_ustx.svp) {
            let mut emitted = ustx::serialize(a).unwrap();
            curve_count = emitted
                .voice_parts
                .iter()
                .flat_map(|p| &p.curves)
                .filter(|c| c.abbr == "dyn")
                .count();
            if curve_count > 0 {
                active += 1;
            }
            for part in &mut emitted.voice_parts {
                part.curves.retain(|c| c.abbr != "dyn");
            }
            assert_eq!(
                emitted,
                ustx::serialize(b).unwrap(),
                "USTX nominal data: {path}"
            );
            if retained.is_none() {
                assert_eq!(
                    target::serialize_to(ExportTarget::Ustx, a).unwrap(),
                    target::serialize_to(ExportTarget::Ustx, b).unwrap(),
                    "No-expression bytes: {path}"
                );
            }
        }
        println!(
            "PRIVATE_SCORE {} retained={} dyn_parts={} convertible={}",
            case["id"],
            retained.as_ref().map_or(0, |i| i.retained.len()),
            curve_count,
            expressed_ustx.ok
        );
    }
    assert!(
        active > 0,
        "Private corpus must exercise authored note velocities"
    );
    println!(
        "PRIVATE_SCORE_TOTAL cases={} active_cases={active}",
        cases.len()
    );
}

#[test]
fn spanner_endpoint_staff_and_voice_displacement_does_not_change_playback_scope() {
    let base =
        String::from_utf8(include_bytes!("score-intensity-fixtures/contour-modern.mscx").to_vec())
            .unwrap();
    let moved = base.replace(
        "<location>",
        "<location><staves>1</staves><voices>-1</voices>",
    );
    assert_ne!(moved, base);
    let (_, _, original) = project(base.as_bytes(), true);
    let (_, _, displaced) = project(moved.as_bytes(), true);
    assert_eq!(original, displaced);
}

#[test]
fn long_authored_ramp_refuses_sample_budget_before_partial_projection() {
    let body = format!(
        "{}{}{}{}",
        direction("p", 0),
        wedge("crescendo", 0, ""),
        note(2_000_001, ""),
        wedge("stop", 0, "")
    );
    let source = musicxml::parse(xml(&measure(&body)).as_bytes()).unwrap();
    let outcome = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(!outcome.ok);
    assert!(outcome
        .msg
        .as_deref()
        .unwrap()
        .contains("MIDI_PERFORMANCE_LIMIT"));
    assert!(outcome.svp.is_none());
}

#[test]
fn diagnostic_only_wedge_in_rest_keeps_original_owner_and_performed_tick() {
    let body = format!(
        "{}{}<note><rest/><duration>480</duration></note>{}",
        note(480, ""),
        wedge("stop", 240, ""),
        note(480, "")
    );
    let source = musicxml::parse(xml(&measure(&body)).as_bytes()).unwrap();
    let index = verse_lib::engine::performance::normalize(&source).unwrap();
    let retained = &source.score_intensity.as_ref().unwrap().retained;
    assert_eq!(retained.len(), 1);
    let id = retained.keys().next().unwrap();
    let event = &index.events[id];
    let source_track = source
        .tracks
        .iter()
        .find(|t| {
            t.events
                .iter()
                .any(|e| matches!(e.kind, midi::Kind::NoteOn(_)))
        })
        .unwrap();
    assert_eq!(event.track_id, source_track.id);
    assert_eq!(event.tick, 720);
    for export in [ExportTarget::Svp, ExportTarget::Ustx] {
        let result = convert_midi_with_target(&source, "english", None, export);
        assert!(result.ok, "{:?}", result.msg);
        let matching: Vec<_> = result
            .projection
            .performance_spans
            .iter()
            .filter(|r| r.start_tick == 720 && r.end_tick == 720)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "{:?}",
            result.projection.performance_spans
        );
        assert_eq!(matching[0].source_track_id, source_track.id);
        assert_eq!(matching[0].target_track, None);
        assert!(matching[0].note_ids.is_empty());
    }
}
