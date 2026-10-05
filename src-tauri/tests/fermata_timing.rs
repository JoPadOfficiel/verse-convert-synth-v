use std::collections::BTreeMap;
use verse_lib::engine::convert::convert_midi_with_target;
use verse_lib::engine::midi::{Kind, Midi};
use verse_lib::engine::musescore::parse_mscx;
use verse_lib::engine::target::{self, ExportTarget};

fn score(version: &str, staves: &[String]) -> String {
    let parts: String = staves.iter().enumerate().map(|(index, _)| {
        let instrument = if index == 0 { "voice.soprano" } else { "keyboard.piano" };
        format!("<Part><Staff id=\"{}\"/><Instrument><instrumentId>{instrument}</instrumentId></Instrument></Part>", index + 1)
    }).collect();
    let body: String = staves
        .iter()
        .enumerate()
        .map(|(index, measures)| format!("<Staff id=\"{}\">{measures}</Staff>", index + 1))
        .collect();
    format!("<museScore version=\"{version}\"><Score><Division>480</Division>{parts}{body}</Score></museScore>")
}

fn measure(contents: &str) -> String {
    format!("<Measure len=\"1/4\"><voice>{contents}</voice></Measure>")
}

fn chord(duration: &str, pitch: u8) -> String {
    format!("<Chord><durationType>{duration}</durationType><Lyrics><text>one</text></Lyrics><Note><pitch>{pitch}</pitch></Note></Chord>")
}

fn fermata(properties: &str) -> String {
    format!("<Fermata>{properties}</Fermata>")
}

fn tempos(midi: &Midi) -> Vec<(u32, u32)> {
    midi.tracks
        .iter()
        .flat_map(|track| &track.events)
        .filter_map(|event| {
            if let Kind::Tempo(micros) = event.kind {
                Some((event.tick, micros))
            } else {
                None
            }
        })
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect()
}

fn geometry(midi: &Midi) -> Vec<(u32, String, Option<u8>, bool)> {
    midi.tracks
        .iter()
        .flat_map(|track| &track.events)
        .filter_map(|event| match &event.kind {
            Kind::NoteOn(note) => Some((event.tick, note.source.id.clone(), note.key, true)),
            Kind::NoteOff(note) => {
                Some((event.tick, note.source_id.clone().unwrap(), note.key, false))
            }
            _ => None,
        })
        .collect()
}

fn both_targets(midi: &Midi, expected: &[(u32, f64)]) {
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        let outcome = convert_midi_with_target(midi, "english", None, target);
        assert!(outcome.ok, "{target:?}: {:?}", outcome.msg);
        let project = outcome.svp.unwrap();
        target::validate_for(target, &project).unwrap();
        let actual: Vec<_> = match target {
            ExportTarget::Svp => target::svp::serialize(&project)
                .unwrap()
                .time
                .tempo
                .into_iter()
                .map(|tempo| (tempo.position, tempo.bpm))
                .collect(),
            ExportTarget::Ustx => target::ustx::serialize(&project)
                .unwrap()
                .tempos
                .into_iter()
                .map(|tempo| (i64::from(tempo.position), tempo.bpm))
                .collect(),
        };
        assert_eq!(actual.len(), expected.len());
        for ((position, bpm), &(tick, expected_bpm)) in actual.iter().zip(expected) {
            let expected_position = match target {
                ExportTarget::Svp => i64::from(tick) * 1_470_000,
                ExportTarget::Ustx => i64::from(tick),
            };
            assert_eq!(*position, expected_position, "{target:?}");
            assert!(
                (bpm - expected_bpm).abs() < 0.001,
                "{target:?}: {bpm} != {expected_bpm}"
            );
        }
    }
}

#[test]
fn instrumental_fermata_uses_longest_simultaneous_mark_and_shortest_cross_staff_rest() {
    let mark = fermata("<timeStretch>1.5</timeStretch>");
    let longest = fermata("<timeStretch>3</timeStretch>");
    let vocal = format!(
        "<Measure><voice><Tempo><tempo>2</tempo></Tempo>{}</voice></Measure>",
        chord("whole", 72)
    );
    let instrument = format!("<Measure><voice>{longest}<Rest><durationType>eighth</durationType></Rest><Rest><durationType>quarter</durationType></Rest></voice></Measure>");
    let other = format!(
        "<Measure><voice>{mark}<Rest><durationType>whole</durationType></Rest></voice></Measure>"
    );
    let xml = score("3.02", &[vocal, instrument, other]);
    let midi = parse_mscx(&xml).unwrap();
    let nominal = parse_mscx(
        &xml.replace(&mark, &fermata("<play>0</play>"))
            .replace(&longest, &fermata("<play>0</play>")),
    )
    .unwrap();
    assert_eq!(geometry(&midi), geometry(&nominal));
    assert_eq!(tempos(&midi), [(0, 1_500_000), (239, 500_000)]);
    both_targets(&midi, &[(0, 40.0), (239, 120.0)]);
}

#[test]
fn native_three_explicit_hold_ends_at_dotted_quarter_segment_without_implicit_beat() {
    let mark = fermata("<timeStretch>1.5</timeStretch>");
    let first = format!(
        "<Measure><voice><Tempo><tempo>2</tempo></Tempo>{}</voice></Measure>",
        chord("whole", 60)
    );
    let second = format!("<Measure><voice>{mark}<Rest><durationType>quarter</durationType><dots>1</dots></Rest><Rest><durationType>half</durationType></Rest></voice></Measure>");
    let xml = score("3.02", &[first, second]);
    let midi = parse_mscx(&xml).unwrap();
    assert_eq!(tempos(&midi), [(0, 750_000), (719, 500_000)]);
    assert_eq!(
        geometry(&midi),
        geometry(&parse_mscx(&xml.replace(&mark, &fermata("<play>0</play>"))).unwrap())
    );
    both_targets(&midi, &[(0, 80.0), (719, 120.0)]);
}

#[test]
fn cross_staff_short_voice_tail_gap_start_bounds_the_whole_note_hold() {
    for version in ["3.02", "4.50"] {
        let mark = fermata("<timeStretch>1.5</timeStretch>");
        let vocal = format!(
            "<Measure><voice>{mark}{}</voice></Measure>",
            chord("whole", 60)
        );
        let short = "<Measure><voice><Chord><durationType>quarter</durationType><Note><pitch>48</pitch></Note></Chord></voice></Measure>".to_string();
        let xml = score(version, &[vocal, short]);
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(tempos(&midi), [(0, 750_000), (479, 500_000)]);
        let nominal = parse_mscx(&xml.replace(&mark, &fermata("<play>0</play>"))).unwrap();
        assert_eq!(geometry(&midi), geometry(&nominal));
        assert_eq!(geometry(&midi).len(), 4); // Only the two authored notes and their offs.
        both_targets(&midi, &[(0, 80.0), (479, 120.0)]);
    }
}

#[test]
fn native_three_positive_gap_after_prior_note_adds_only_its_start_boundary() {
    let mark = fermata("<timeStretch>1.5</timeStretch>");
    let vocal = format!("<Measure><voice>{}{mark}<Chord><durationType>half</durationType><dots>1</dots><Lyrics><text>two</text></Lyrics><Note><pitch>62</pitch></Note></Chord></voice></Measure>", chord("quarter", 60));
    let other = "<Measure><voice><Chord><durationType>quarter</durationType><dots>1</dots><Note><pitch>48</pitch></Note></Chord><location><fractions>1/8</fractions></location><Chord><durationType>half</durationType><Note><pitch>50</pitch></Note></Chord></voice></Measure>".to_string();
    let xml = score("3.02", &[vocal, other]);
    let midi = parse_mscx(&xml).unwrap();
    assert_eq!(tempos(&midi), [(480, 750_000), (719, 500_000)]);
    assert_eq!(
        geometry(&midi),
        geometry(&parse_mscx(&xml.replace(&mark, &fermata("<play>0</play>"))).unwrap())
    );
    assert_eq!(geometry(&midi).len(), 8);
    both_targets(&midi, &[(0, 120.0), (480, 80.0), (719, 120.0)]);
}

#[test]
fn modern_implicit_gap_decomposition_refuses_only_intersecting_holds() {
    let mark = fermata("<timeStretch>1.5</timeStretch>");
    let sparse = "<Measure><voice><Rest><durationType>whole</durationType></Rest></voice><voice><location><fractions>3/8</fractions></location><Chord><durationType>half</durationType><Note><pitch>48</pitch></Note></Chord></voice></Measure>";
    let vocal = format!(
        "<Measure><voice>{mark}{}</voice></Measure>",
        chord("whole", 60)
    );
    let legacy = parse_mscx(&score("3.02", &[vocal.clone(), sparse.to_string()])).unwrap();
    assert_eq!(tempos(&legacy), [(0, 750_000), (719, 500_000)]);
    both_targets(&legacy, &[(0, 80.0), (719, 120.0)]);
    let modern = score("4.50", &[vocal, sparse.to_string()]);
    let error = parse_mscx(&modern).unwrap_err();
    assert!(error.starts_with("SOURCE_PLAYBACK_UNSUPPORTED:"));
    assert!(error.contains("implicit gap decomposition"));
    assert!(parse_mscx(&modern.replace(&mark, &fermata("<play>0</play>"))).is_ok());

    // Gaps in an earlier measure end exactly at this dense hold's start.
    let dense = format!(
        "<Measure><voice>{}</voice></Measure>{}",
        chord("whole", 60),
        measure(&format!("{mark}{}", chord("quarter", 62)))
    );
    let other = format!(
        "{sparse}{}",
        measure("<Rest><durationType>quarter</durationType></Rest>")
    );
    let disjoint = parse_mscx(&score("4.50", &[dense, other])).unwrap();
    assert_eq!(tempos(&disjoint), [(1920, 750_000), (2399, 500_000)]);
    both_targets(&disjoint, &[(0, 120.0), (1920, 80.0), (2399, 120.0)]);

    // A later gap in the same measure is also outside a dense quarter hold.
    let dense = format!("<Measure><voice>{mark}{}<Rest><durationType>half</durationType><dots>1</dots></Rest></voice></Measure>", chord("quarter", 60));
    let later_gap = "<Measure><voice><Chord><durationType>quarter</durationType><Note><pitch>48</pitch></Note></Chord><location><fractions>1/8</fractions></location><Chord><durationType>half</durationType><Note><pitch>50</pitch></Note></Chord></voice></Measure>".to_string();
    let disjoint = parse_mscx(&score("4.50", &[dense, later_gap])).unwrap();
    assert_eq!(tempos(&disjoint), [(0, 750_000), (479, 500_000)]);
    both_targets(&disjoint, &[(0, 80.0), (479, 120.0)]);
}

#[test]
fn overlapping_voice_rhythm_refuses_active_fermata_only_in_its_played_measure() {
    for version in ["3.02", "4.50"] {
        let mark = fermata("<timeStretch>1.5</timeStretch>");
        let overlap = "<Measure><voice><Chord><durationType>half</durationType><Note><pitch>48</pitch></Note></Chord><location><fractions>-1/4</fractions></location><Chord><durationType>quarter</durationType><Note><pitch>50</pitch></Note></Chord></voice></Measure>";
        let vocal = format!(
            "<Measure><voice>{mark}{}</voice></Measure>",
            chord("whole", 60)
        );
        let xml = score(version, &[vocal, overlap.to_string()]);
        let error = parse_mscx(&xml).unwrap_err();
        assert!(error.starts_with("SOURCE_PLAYBACK_UNSUPPORTED:"));
        assert!(error.contains("overlapping voice rhythm"));
        assert!(parse_mscx(&xml.replace(&mark, &fermata("<play>0</play>"))).is_ok());
        let later = format!(
            "<Measure><voice>{}</voice></Measure>{}",
            chord("whole", 60),
            measure(&format!("{mark}{}", chord("quarter", 62)))
        );
        let other = format!(
            "{overlap}{}",
            measure("<Rest><durationType>quarter</durationType></Rest>")
        );
        assert!(parse_mscx(&score(version, &[later, other])).is_ok());
    }
}

#[test]
fn empty_or_grace_only_voices_do_not_create_implicit_gap_boundaries() {
    for voice in ["<voice/>", "<voice><Chord><grace8/><durationType>quarter</durationType><Note><pitch>48</pitch></Note></Chord></voice>"] {
        let vocal = format!("<Measure><voice>{}{}</voice></Measure>", fermata("<timeStretch>1.5</timeStretch>"), chord("whole", 60));
        let other = format!("<Measure>{voice}</Measure>");
        let midi = parse_mscx(&score("3.02", &[vocal, other])).unwrap();
        assert_eq!(tempos(&midi), [(0, 750_000), (1919, 500_000)]);
        both_targets(&midi, &[(0, 80.0), (1919, 120.0)]);
    }
}

#[test]
fn native_three_omissions_and_explicit_one_do_not_add_playback_holds() {
    for version in ["3.02", "4.50"] {
        let properties = if version.starts_with('3') {
            "<subtype>fermataLongAbove</subtype>"
        } else {
            "<timeStretch>1</timeStretch>"
        };
        let xml = score(
            version,
            &[measure(&format!(
                "{}{}",
                fermata(properties),
                chord("quarter", 60)
            ))],
        );
        assert!(tempos(&parse_mscx(&xml).unwrap()).is_empty());
        let disabled = score(
            version,
            &[measure(&format!(
                "{}{}",
                fermata("<subtype>unknown</subtype><play>0</play><timeStretch>4</timeStretch>"),
                chord("quarter", 60)
            ))],
        );
        assert!(tempos(&parse_mscx(&disabled).unwrap()).is_empty());
    }
    let legacy_no_symbol = score(
        "3.02",
        &[measure(&format!(
            "{}{}",
            fermata("<subtype>noSym</subtype>"),
            chord("quarter", 60)
        ))],
    );
    assert!(tempos(&parse_mscx(&legacy_no_symbol).unwrap()).is_empty());
}

#[test]
fn explicit_stretch_uses_source_tempo_precision_before_microsecond_rounding() {
    let xml = score(
        "3.02",
        &[measure(&format!(
            "<Tempo><tempo>1.8333333333333333</tempo></Tempo>{}{}",
            fermata("<timeStretch>1.5</timeStretch>"),
            chord("quarter", 60)
        ))],
    );
    let midi = parse_mscx(&xml).unwrap();
    assert_eq!(tempos(&midi), [(0, 818_182), (479, 545_455)]);
    both_targets(&midi, &[(0, 110.0 / 1.5), (479, 110.0)]);
}

#[test]
fn native_epsilon_scales_exactly_with_source_division() {
    let xml = score(
        "3.02",
        &[measure(&format!(
            "{}{}",
            fermata("<timeStretch>2</timeStretch>"),
            chord("quarter", 60)
        ))],
    )
    .replace("<Division>480</Division>", "<Division>960</Division>");
    let midi = parse_mscx(&xml).unwrap();
    assert_eq!(midi.ticks_per_beat, 960);
    assert_eq!(tempos(&midi), [(0, 1_000_000), (958, 500_000)]);
    // Both target maps restore at 479 native ticks, not 479.5.
    both_targets(&midi, &[(0, 60.0), (479, 120.0)]);
}

#[test]
fn native_four_defaults_are_qualified_by_subtype() {
    for (subtype, micros) in [
        (Some("fermataAbove"), 1_000_000),
        (Some("fermataBelow"), 1_000_000),
        (Some("fermataVeryShortAbove"), 625_000),
        (Some("fermataShortBelow"), 750_000),
        (Some("fermataShortHenzeAbove"), 750_000),
        (Some("fermataLongAbove"), 1_500_000),
        (Some("fermataLongHenzeBelow"), 1_500_000),
        (Some("fermataVeryLongBelow"), 2_000_000),
    ] {
        let properties = subtype
            .map(|s| format!("<subtype>{s}</subtype>"))
            .unwrap_or_default();
        let xml = score(
            "4.50",
            &[measure(&format!(
                "{}{}",
                fermata(&properties),
                chord("quarter", 60)
            ))],
        );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(tempos(&midi), [(0, micros), (479, 500_000)], "{subtype:?}");
        both_targets(
            &midi,
            &[(0, 60_000_000.0 / f64::from(micros)), (479, 120.0)],
        );
    }
}

#[test]
fn repeat_playback_reemits_holds_without_leaking_written_recording_marks() {
    let mark = fermata("<timeStretch>1.5</timeStretch>");
    let measures = format!("<Measure len=\"1/4\"><startRepeat/><voice><Tempo><tempo>2</tempo></Tempo>{mark}{}</voice><endRepeat>2</endRepeat></Measure>{}", chord("quarter", 60), measure(&chord("quarter", 62)));
    let xml = score("3.02", &[measures]);
    let midi = parse_mscx(&xml).unwrap();
    let nominal = parse_mscx(&xml.replace(&mark, &fermata("<play>0</play>"))).unwrap();
    assert_eq!(geometry(&midi), geometry(&nominal));
    assert_eq!(geometry(&midi).len(), 6);
    assert_eq!(
        tempos(&midi),
        [(0, 750_000), (479, 500_000), (480, 750_000), (959, 500_000)]
    );
    both_targets(&midi, &[(0, 80.0), (479, 120.0), (480, 80.0), (959, 120.0)]);
}

fn changing_tempo_repeat(version: &str, tempo_staff: usize, last_tempo: &str) -> String {
    let mark = fermata("<timeStretch>2</timeStretch>");
    let mut staves = Vec::new();
    for index in 0..2 {
        let first_tempo = if index == tempo_staff {
            "<Tempo><tempo>2</tempo></Tempo>"
        } else {
            ""
        };
        let final_tempo = if index == tempo_staff {
            format!("<Tempo><tempo>{last_tempo}</tempo></Tempo>")
        } else {
            String::new()
        };
        let music = if index == 0 {
            chord("quarter", 60)
        } else {
            "<Rest><durationType>quarter</durationType></Rest>".to_string()
        };
        let hold = if index == 0 { mark.as_str() } else { "" };
        staves.push(format!("{}<Measure len=\"1/4\"><startRepeat/><voice>{hold}{music}</voice></Measure><Measure len=\"1/4\"><voice>{final_tempo}{music}</voice><endRepeat>2</endRepeat></Measure>", measure(&format!("{first_tempo}{music}"))));
    }
    score(version, &staves)
}

#[test]
fn repeat_jump_restores_written_global_tempo_before_applying_the_fermata() {
    for version in ["3.02", "4.50"] {
        for tempo_staff in [0, 1] {
            let xml = changing_tempo_repeat(version, tempo_staff, "4");
            let midi = parse_mscx(&xml).unwrap();
            assert_eq!(
                tempos(&midi),
                [
                    (0, 500_000),
                    (480, 1_000_000),
                    (959, 500_000),
                    (960, 250_000),
                    (1440, 1_000_000),
                    (1919, 500_000),
                    (1920, 250_000)
                ]
            );
            both_targets(
                &midi,
                &[
                    (0, 120.0),
                    (480, 60.0),
                    (959, 120.0),
                    (960, 240.0),
                    (1440, 60.0),
                    (1919, 120.0),
                    (1920, 240.0),
                ],
            );
            let nominal_xml = xml.replace(
                &fermata("<timeStretch>2</timeStretch>"),
                &fermata("<play>0</play><timeStretch>2</timeStretch>"),
            );
            let nominal = parse_mscx(&nominal_xml).unwrap();
            assert_eq!(
                tempos(&nominal),
                [
                    (0, 500_000),
                    (960, 250_000),
                    (1440, 500_000),
                    (1920, 250_000)
                ]
            );
            assert_eq!(geometry(&midi), geometry(&nominal));
        }
    }
}

#[test]
fn constant_written_tempo_does_not_add_a_redundant_repeat_reset() {
    let xml = changing_tempo_repeat("3.02", 1, "2");
    let midi = parse_mscx(&xml).unwrap();
    assert_eq!(
        midi.tracks
            .iter()
            .flat_map(|track| &track.events)
            .filter(|event| event.tick == 1440 && matches!(event.kind, Kind::Tempo(_)))
            .count(),
        1
    );
    let nominal = parse_mscx(&xml.replace(
        &fermata("<timeStretch>2</timeStretch>"),
        &fermata("<play>0</play>"),
    ))
    .unwrap();
    assert!(!nominal
        .tracks
        .iter()
        .flat_map(|track| &track.events)
        .any(|event| event.tick == 1440 && matches!(event.kind, Kind::Tempo(_))));
}

#[test]
fn forward_volta_skip_keeps_performed_tempo_and_never_imports_skipped_ending_tempo() {
    for version in ["3.02", "4.50"] {
        for play in ["0", "1"] {
            let mark = fermata(&format!("<timeStretch>2</timeStretch><play>{play}</play>"));
            let note = chord("quarter", 60);
            let ending = |pass| {
                format!("<Spanner type=\"Volta\"><Volta><endings>{pass}</endings></Volta><next><location><measures>1</measures></location></next></Spanner>")
            };
            let measures = format!("<Measure len=\"1/4\"><startRepeat/><voice><Tempo><tempo>2</tempo></Tempo>{note}</voice></Measure><Measure len=\"1/4\">{}<voice><Tempo><tempo>1</tempo></Tempo>{note}</voice><endRepeat>2</endRepeat></Measure><Measure len=\"1/4\">{}<voice>{mark}{note}</voice></Measure>{}", ending(1), ending(2), measure(&note));
            let midi = parse_mscx(&score(version, &[measures])).unwrap();
            assert_eq!(
                geometry(&midi)
                    .iter()
                    .filter(|event| event.3)
                    .map(|event| event.0)
                    .collect::<Vec<_>>(),
                [0, 480, 960, 1440, 1920]
            );
            let mut expected = vec![(0, 500_000), (480, 1_000_000), (960, 500_000)];
            let mut expected_target = vec![(0, 120.0), (480, 60.0), (960, 120.0)];
            if play == "1" {
                expected.extend([(1440, 1_000_000), (1919, 500_000)]);
                expected_target.extend([(1440, 60.0), (1919, 120.0)]);
            }
            assert_eq!(tempos(&midi), expected);
            both_targets(&midi, &expected_target);
        }
    }
}

#[test]
fn authored_tempo_segment_and_later_fermata_base_are_preserved() {
    let first = format!("<Measure><voice><Tempo><tempo>2</tempo></Tempo>{}{}<Tempo><tempo>4</tempo></Tempo>{}{}<Rest><durationType>half</durationType></Rest></voice></Measure>", fermata("<timeStretch>2</timeStretch>"), chord("quarter", 60), fermata("<timeStretch>3</timeStretch>"), chord("quarter", 62));
    let other = "<Measure><voice><location><fractions>479/1920</fractions></location><Tempo><tempo>3</tempo></Tempo><location><fractions>-479/1920</fractions></location><Rest><durationType>whole</durationType></Rest></voice></Measure>".to_string();
    let midi = parse_mscx(&score("3.02", &[first, other])).unwrap();
    assert_eq!(
        tempos(&midi),
        [
            (0, 1_000_000),
            (478, 500_000),
            (479, 333_333),
            (480, 750_000),
            (959, 250_000)
        ]
    );
    both_targets(
        &midi,
        &[
            (0, 60.0),
            (478, 120.0),
            (479, 180.0),
            (480, 80.0),
            (959, 240.0),
        ],
    );
}

#[test]
fn native_four_missing_subtype_requires_explicit_positive_stretch() {
    for mark in ["<Fermata/>", "<Fermata><play>1</play></Fermata>"] {
        let xml = score(
            "4.50",
            &[measure(&format!("{mark}{}", chord("quarter", 60)))],
        );
        assert!(parse_mscx(&xml)
            .unwrap_err()
            .starts_with("SOURCE_PLAYBACK_UNSUPPORTED:"));
    }
    let xml = score(
        "4.50",
        &[measure(&format!(
            "{}{}",
            fermata("<timeStretch>1.5</timeStretch>"),
            chord("quarter", 60)
        ))],
    );
    let midi = parse_mscx(&xml).unwrap();
    assert_eq!(tempos(&midi), [(0, 750_000), (479, 500_000)]);
    both_targets(&midi, &[(0, 80.0), (479, 120.0)]);
}

#[test]
fn nonrhythmic_native_segments_bound_a_cross_part_whole_note_fermata() {
    for version in ["3.02", "4.50"] {
        for anchor in [
            "<Clef><concertClefType>G</concertClefType></Clef>",
            "<KeySig><accidental>0</accidental></KeySig>",
            "<BarLine><subtype>normal</subtype></BarLine>",
            "<Tempo><tempo>2</tempo></Tempo>",
            "<Dynamic><subtype>p</subtype></Dynamic>",
            "<Expression><text>dolce</text></Expression>",
        ] {
            let mark = fermata("<timeStretch>2</timeStretch>");
            let vocal = format!(
                "<Measure><voice>{mark}{}</voice></Measure>",
                chord("whole", 60)
            );
            let instrument = format!("<Measure><voice><Rest><durationType>whole</durationType></Rest><location><fractions>-7/8</fractions></location>{anchor}</voice></Measure>");
            let xml = score(version, &[vocal, instrument]);
            let midi = parse_mscx(&xml).unwrap();
            assert_eq!(
                geometry(&midi),
                geometry(&parse_mscx(&xml.replace(&mark, &fermata("<play>0</play>"))).unwrap())
            );
            let mut expected = vec![(0, 1_000_000), (239, 500_000)];
            let mut target_expected = vec![(0, 60.0), (239, 120.0)];
            if anchor.starts_with("<Tempo>") {
                expected.push((240, 500_000));
                target_expected.push((240, 120.0));
            }
            assert_eq!(tempos(&midi), expected, "{version}: {anchor}");
            both_targets(&midi, &target_expected);
        }
    }
}

#[test]
fn end_barline_fermata_does_not_spill_into_repeated_or_following_measure() {
    for version in ["3.02", "4.50"] {
        for barline in ["", "<BarLine><subtype>end-repeat</subtype></BarLine>"] {
            let trailing = fermata("<timeStretch>4</timeStretch>");
            let following = fermata("<timeStretch>1.5</timeStretch>");
            let measures = format!("<Measure len=\"1/4\"><startRepeat/><voice><Tempo><tempo>2</tempo></Tempo>{}{trailing}{barline}</voice><endRepeat>2</endRepeat></Measure>{}", chord("quarter", 60), measure(&format!("{following}{}{trailing}", chord("quarter", 62))));
            let xml = score(version, &[measures]);
            let midi = parse_mscx(&xml).unwrap();
            let nominal = parse_mscx(&xml.replace(&trailing, &fermata("<play>0</play>"))).unwrap();
            assert_eq!(geometry(&midi), geometry(&nominal));
            assert_eq!(
                tempos(&midi),
                [
                    (0, 500_000),
                    (480, 500_000),
                    (960, 750_000),
                    (1439, 500_000)
                ]
            );
            both_targets(
                &midi,
                &[(0, 120.0), (480, 120.0), (960, 80.0), (1439, 120.0)],
            );
        }
    }
}

#[test]
fn unqualified_native_segments_with_active_fermata_are_refused() {
    for version in ["3.02", "4.50"] {
        for segment in [
            "<Breath/>",
            "<RepeatMeasure/>",
            "<Ambitus/>",
            "<TimeSig><sigN>4</sigN><sigD>4</sigD></TimeSig>",
        ] {
            let mark = fermata("<timeStretch>2</timeStretch>");
            let xml = score(version, &[format!("<Measure><voice>{mark}{}<location><fractions>-7/8</fractions></location>{segment}</voice></Measure>", chord("whole", 60))]);
            assert!(parse_mscx(&xml)
                .unwrap_err()
                .starts_with("SOURCE_PLAYBACK_UNSUPPORTED:"));
            assert!(parse_mscx(&xml.replace(&mark, &fermata("<play>0</play>"))).is_ok());
        }
    }
}

#[test]
fn repeat_measure_refusal_is_scoped_to_active_fermata_measure() {
    for version in ["3.02", "4.50"] {
        // The private regression has repeats through written measure 44 and
        // active stretches only in measure 45; adjacent boundaries are disjoint.
        for gap in [0, 2] {
            let mark = fermata("<timeStretch>1.5</timeStretch>");
            let rest = "<Rest><durationType>quarter</durationType></Rest>";
            let filler_vocal = measure(&chord("quarter", 60)).repeat(gap);
            let filler_instrument = measure(rest).repeat(gap);
            let vocal = format!(
                "{}{filler_vocal}{}",
                measure(&chord("quarter", 60)),
                measure(&format!("{mark}{}", chord("quarter", 62)))
            );
            let disjoint = format!(
                "{}{filler_instrument}{}",
                measure("<RepeatMeasure/>"),
                measure(rest)
            );
            let midi = parse_mscx(&score(version, &[vocal.clone(), disjoint])).unwrap();
            let start = (gap as u32 + 1) * 480;
            assert_eq!(tempos(&midi), [(start, 750_000), (start + 479, 500_000)]);
            both_targets(&midi, &[(0, 120.0), (start, 80.0), (start + 479, 120.0)]);

            let overlapping = format!(
                "{}{filler_instrument}{}",
                measure(rest),
                measure("<RepeatMeasure/>")
            );
            let xml = score(version, &[vocal, overlapping]);
            let error = parse_mscx(&xml).unwrap_err();
            assert!(error.starts_with("SOURCE_PLAYBACK_UNSUPPORTED:"));
            assert!(error.contains("RepeatMeasure"));
            assert!(parse_mscx(&xml.replace(&mark, &fermata("<play>0</play>"))).is_ok());
        }
    }
}

#[test]
fn standalone_fermata_uses_current_cursor_and_explicit_location_without_back_binding() {
    for location in ["", "<location><fractions>-1/4</fractions></location>"] {
        let mark = fermata("<timeStretch>2</timeStretch>");
        let advance = if location.is_empty() {
            ""
        } else {
            "<location><fractions>1/4</fractions></location>"
        };
        let body = format!("<Measure><voice>{}{}{mark}{advance}{}<Rest><durationType>half</durationType></Rest></voice></Measure>", chord("quarter", 60), location, chord("quarter", 62));
        let xml = score("3.02", &[body]);
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(
            geometry(&midi),
            geometry(&parse_mscx(&xml.replace(&mark, &fermata("<play>0</play>"))).unwrap())
        );
        let start = if location.is_empty() { 480 } else { 0 };
        assert_eq!(tempos(&midi), [(start, 1_000_000), (start + 479, 500_000)]);
    }
}

#[test]
fn simultaneous_one_is_maximum_but_disabled_mark_does_not_suppress_a_speedup() {
    for (other, expected) in [
        ("<timeStretch>1</timeStretch>", vec![]),
        (
            "<timeStretch>4</timeStretch><play>0</play>",
            vec![(0, 250_000), (479, 500_000)],
        ),
    ] {
        let xml = score(
            "3.02",
            &[measure(&format!(
                "{}{}{}",
                fermata("<timeStretch>0.5</timeStretch>"),
                fermata(other),
                chord("quarter", 60)
            ))],
        );
        assert_eq!(tempos(&parse_mscx(&xml).unwrap()), expected);
    }
}

#[test]
fn invalid_or_unknown_playback_declarations_refuse_stably() {
    for properties in [
        "<timeStretch>0</timeStretch>",
        "<timeStretch>-1</timeStretch>",
        "<timeStretch>NaN</timeStretch>",
        "<timeStretch>inf</timeStretch>",
        "<timeStretch>oops</timeStretch>",
        "<timeStretch/>",
        "<play>2</play>",
        "<play/>",
        "<timeStretch>2</timeStretch><timeStretch>3</timeStretch>",
        "<timeStretch>2<value>3</value></timeStretch>",
    ] {
        let xml = score(
            "3.02",
            &[measure(&format!(
                "{}{}",
                fermata(properties),
                chord("quarter", 60)
            ))],
        );
        assert!(
            parse_mscx(&xml)
                .unwrap_err()
                .starts_with("SOURCE_PLAYBACK_INVALID:"),
            "{properties}"
        );
    }
    for version in ["3.02", "4.50"] {
        let unknown = fermata("<subtype>unknown</subtype>");
        let xml = score(
            version,
            &[measure(&format!("{unknown}{}", chord("quarter", 60)))],
        );
        if version.starts_with('4') {
            assert!(parse_mscx(&xml)
                .unwrap_err()
                .starts_with("SOURCE_PLAYBACK_UNSUPPORTED:"));
        } else {
            assert!(tempos(&parse_mscx(&xml).unwrap()).is_empty());
        }
        let explicit = xml.replace(
            "<subtype>unknown</subtype>",
            "<subtype>unknown</subtype><timeStretch>1.5</timeStretch>",
        );
        assert_eq!(
            tempos(&parse_mscx(&explicit).unwrap()),
            [(0, 750_000), (479, 500_000)]
        );
    }
}

#[test]
fn native_grid_or_tempo_overflow_is_refused_without_rounding() {
    let mark = fermata("<timeStretch>2</timeStretch>");
    let xml = score(
        "3.02",
        &[measure(&format!("{mark}{}", chord("quarter", 60)))],
    );
    let off_grid = xml.replace("<Division>480</Division>", "<Division>100</Division>");
    let tuplet = score("3.02", &[measure(&format!("{mark}<Tuplet><normalNotes>4</normalNotes><actualNotes>7</actualNotes></Tuplet>{}{}<endTuplet/>", chord("quarter", 60), chord("quarter", 62)))]);
    let outside_measure = score(
        "3.02",
        &[measure(&format!(
            "{}<location><fractions>1/1920</fractions></location>{mark}",
            chord("quarter", 60)
        ))],
    );
    for refused in [
        off_grid,
        tuplet,
        outside_measure,
        xml.replace(
            "<timeStretch>2</timeStretch>",
            "<timeStretch>1e308</timeStretch>",
        ),
    ] {
        assert!(parse_mscx(&refused)
            .unwrap_err()
            .starts_with("SOURCE_PLAYBACK_TIMING_UNREPRESENTABLE:"));
    }
}
