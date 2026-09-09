//! Source-owned sung attacks, pronunciation policy, and musical invariants.
use verse_lib::engine::convert::{
    convert_midi_with_profile, convert_midi_with_target, ConvertOutcome,
};
use verse_lib::engine::midi::{Kind, Midi};
use verse_lib::engine::projection::ProjectedLyric;
use verse_lib::engine::target::{self, french, ustx, ExportTarget, PronunciationProfile};
use verse_lib::engine::{musescore, musicxml};

const FR: PronunciationProfile = PronunciationProfile::FrenchMillefeuille;

#[test]
fn contextual_sung_layout_matrix_in_both_score_formats() {
    // Synthetic isolated words: expected phones come from the FR-004 contract,
    // including contexts that previously received a wrong dictionary reading.
    let cases: &[(&[&str], &[&str])] = &[
        (&["j'i", "rai"], &["j ih", "r ae"]),
        (&["rai", "son"], &["r ae", "z on"]),
        (&["s'a", "chève"], &["s ah", "sh ae v"]),
        (&["gar", "de", "rai"], &["g ah r", "d ee", "r ae"]),
        (
            &["j'ef", "fa", "ce", "rai"],
            &["j ae", "f ah", "s ee", "r ae"],
        ),
        (&["for", "ce"], &["f oo r", "s ee"]),
        (&["tê", "te"], &["t ae", "t ee"]),
        (&["bles", "su", "re"], &["b l ae", "s uh", "r ee"]),
        (&["cou", "rants", "an", "an"], &["k ou", "r en", "en", "en"]),
        (&["cou", "rant", "an", "an"], &["k ou", "r en", "en", "en"]),
        (&["jours", "ou", "ours"], &["j ou", "ou", "ou r"]),
        (&["jours", "ou", "our"], &["j ou", "ou", "ou r"]),
        (&["par", "ti", "i", "ir"], &["p ah r", "t ih", "ih", "ih r"]),
        (&["tra", "a", "ce"], &["t r ah", "ah", "s ee"]),
        (&["es", "pa", "a", "ace"], &["ae s", "p ah", "ah", "ah s"]),
        (&["pre", "sse", "e"], &["p r ae", "s ee", "ee"]),
        (&["l'e", "xi", "il"], &["l ae", "g z ih", "ih l"]),
        (&["l'em", "prein", "te"], &["l en", "p r in", "t ee"]),
        (&["trom", "pet", "tes"], &["t r on", "p ae", "t ee"]),
        (&["trom", "pet", "te"], &["t r on", "p ae", "t ee"]),
        (&["a", "mou", "ou", "ours"], &["ah", "m ou", "ou", "ou r"]),
        (&["a", "mou", "ou", "our"], &["ah", "m ou", "ou", "ou r"]),
        (&["gar", "de"], &["g ah r", "d ee"]),
        (&["tem", "pête"], &["t en", "p ae t"]),
        (&["cou", "rant"], &["k ou", "r en"]),
        (&["pli", "er"], &["p l ih", "y eh"]),
        (&["an", "nées"], &["ah", "n eh"]),
        (&["lais", "se"], &["l ae", "s ee"]),
        (&["mi", "nutes"], &["m ih", "n uh t"]),
        (&["souf", "flant"], &["s ou", "f l en"]),
        (&["cour", "ber"], &["k ou r", "b eh"]),
        (&["tes", "te"], &["t ae s", "t ee"]),
        (&["bri", "ser"], &["b r ih", "z eh"]),
        (&["cher", "cher"], &["sh ae r", "sh eh"]),
        (&["ge", "nou"], &["j ee", "n ou"]),
        (&["des", "sus"], &["d ee", "s uh"]),
        (&["Ouh"], &["ou"]),
    ];
    for &(words, phones) in cases {
        let expected: Vec<_> = words
            .iter()
            .zip(phones)
            .map(|(word, phones)| {
                format!(
                    "{}[{}]",
                    word.to_lowercase(),
                    phones
                        .split_whitespace()
                        .map(|p| format!("fr/{p}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })
            .collect();
        let expected: Vec<_> = expected.iter().map(String::as_str).collect();
        for native in [false, true] {
            assert_lanes(&sab(words, native, false), &expected);
        }
    }
}

fn direct_notes(words: &[&str]) -> Vec<verse_lib::engine::projection::ProjectedNote> {
    use verse_lib::engine::{midi::Lyric, projection::ProjectedNote};
    words
        .iter()
        .enumerate()
        .map(|(i, word)| ProjectedNote {
            performance: None,
            onset_ticks: i as u32 * 480,
            duration_ticks: 480,
            pitch: 60 + i as u8,
            lyric: ProjectedLyric::Source(Box::new(Lyric::text(
                format!("lyric-{i}"),
                (*word).into(),
            ))),
        })
        .collect()
}

fn direct_apply(
    notes: &mut [verse_lib::engine::projection::ProjectedNote],
) -> Vec<verse_lib::engine::convert::Diagnostic> {
    french::apply(
        notes,
        &(0..notes.len())
            .map(|i| format!("note-{i}"))
            .collect::<Vec<_>>(),
    )
}

fn edit_source(
    note: &mut verse_lib::engine::projection::ProjectedNote,
) -> &mut verse_lib::engine::midi::Lyric {
    let ProjectedLyric::Source(source) = &mut note.lyric else {
        panic!("expected original")
    };
    source
}

fn phones(note: &verse_lib::engine::projection::ProjectedNote) -> Option<&str> {
    match &note.lyric {
        ProjectedLyric::Pronounced { phonemes, .. } => Some(phonemes),
        _ => None,
    }
}

#[test]
fn only_audited_end_slots_can_support_sung_echoes_and_schwas() {
    use verse_lib::engine::midi::Syllabic;
    for (words, slot, expected) in [
        (vec!["rê", "ê", "ve"], 0, "fr/v fr/ee"),
        (vec!["mu", "u", "ur"], 0, "fr/uh fr/r"),
        (vec!["mur", "mu", "u", "ure"], 1, "fr/uh fr/r"),
        (vec!["vent", "en", "ent"], 0, "fr/en"),
        (vec!["fond", "on", "on"], 1, "fr/on"),
        (vec!["ju", "ure"], 0, "fr/uh fr/r"),
        (vec!["m'ar", "rê", "te"], 1, "fr/t fr/ee"),
        (vec!["bles", "su", "re"], 1, "fr/r fr/ee"),
        (vec!["trom", "pet", "tes"], 1, "fr/t fr/ee"),
        (vec!["tê", "te"], 0, "fr/t fr/ee"),
        (vec!["lais", "se"], 0, "fr/s fr/ee"),
        (vec!["cou", "rants", "an", "an"], 1, "fr/en"),
    ] {
        let mut notes = direct_notes(&words);
        edit_source(&mut notes[slot]).syllabic = Some(Syllabic::End);
        let before = notes.clone();
        let diagnostics = direct_apply(&mut notes);
        assert_eq!(phones(notes.last().unwrap()), Some(expected), "{words:?}");
        assert!(!diagnostics.iter().any(|d| d.code == french::UNSUPPORTED));
        for (note, original) in notes.iter().zip(&before) {
            assert_eq!(
                (note.onset_ticks, note.duration_ticks, note.pitch),
                (
                    original.onset_ticks,
                    original.duration_ticks,
                    original.pitch
                )
            );
            let ProjectedLyric::Pronounced { source, .. } = &note.lyric else {
                panic!("{note:?}")
            };
            assert_eq!(ProjectedLyric::Source(source.clone()), original.lyric);
        }
    }
    for (words, slot) in [(vec!["rê", "ê", "ve"], 1), (vec!["j'i", "rai"], 0)] {
        let mut notes = direct_notes(&words);
        edit_source(&mut notes[slot]).syllabic = Some(Syllabic::End);
        assert!(direct_apply(&mut notes)
            .iter()
            .any(|d| d.code == french::UNSUPPORTED));
    }
}

#[test]
fn exact_layouts_reject_independent_rows_verses_manual_hints_and_unknowns() {
    use verse_lib::engine::midi::Syllabic;
    for boundary in [
        "single",
        "row",
        "verse",
        "manual",
        "unknown",
        "rest",
        "punctuation",
    ] {
        let mut notes = direct_notes(&["j'i", "rai"]);
        match boundary {
            "single" => edit_source(&mut notes[0]).syllabic = Some(Syllabic::Single),
            "row" => edit_source(&mut notes[1]).lane = "2".into(),
            "verse" => edit_source(&mut notes[1]).verse = 2,
            "manual" => notes[0] = direct_notes(&["j'i[fr/j fr/ih]"]).remove(0),
            "unknown" => notes[0] = direct_notes(&["zyx"]).remove(0),
            "rest" => notes[1].onset_ticks += 240,
            "punctuation" => notes[0] = direct_notes(&["j'i,"]).remove(0),
            _ => unreachable!(),
        }
        let original = notes.clone();
        direct_apply(&mut notes);
        assert_eq!(notes, original, "{boundary}");
    }
    let mut other_verse = direct_notes(&["bon", "jour"]);
    edit_source(&mut other_verse[0]).syllabic = Some(Syllabic::Begin);
    edit_source(&mut other_verse[1]).syllabic = Some(Syllabic::End);
    edit_source(&mut other_verse[1]).verse = 2;
    let original = other_verse.clone();
    direct_apply(&mut other_verse);
    assert_eq!(
        other_verse, original,
        "whole-word fallback must also respect the verse"
    );
}

#[test]
fn complete_written_layout_across_rest_uses_independent_hints() {
    use verse_lib::engine::midi::Syllabic;
    let mut notes = direct_notes(&["fond", "on", "on"]);
    for (note, syllabic) in notes
        .iter_mut()
        .zip([Syllabic::Begin, Syllabic::Middle, Syllabic::End])
    {
        edit_source(note).syllabic = Some(syllabic);
    }
    notes[0].duration_ticks = 240;
    let original = notes.clone();
    assert!(!direct_apply(&mut notes)
        .iter()
        .any(|d| d.code == french::UNSUPPORTED));
    assert_eq!(
        notes.iter().map(phones).collect::<Vec<_>>(),
        [Some("fr/f fr/on"), Some("fr/on"), Some("fr/on")]
    );
    for (a, b) in notes.iter().zip(original) {
        assert_eq!(
            (a.onset_ticks, a.duration_ticks, a.pitch),
            (b.onset_ticks, b.duration_ticks, b.pitch)
        );
        assert!(!a.lyric.continues_previous_note());
    }
}

#[test]
fn packed_vowels_and_explicit_echoes_keep_real_holds_in_both_formats() {
    for native in [false, true] {
        for (words, expected) in [
            (
                vec!["rai", "sons'a", "", "chève"],
                vec![
                    "rai[fr/r fr/ae]",
                    "sons'a[fr/z fr/on fr/s fr/ah]",
                    "+~",
                    "chève[fr/sh fr/ae fr/v]",
                ],
            ),
            (
                vec!["es", "pa", "", "ace"],
                vec!["es[fr/ae fr/s]", "pa[fr/p fr/ah]", "+~", "ace[fr/ah fr/s]"],
            ),
        ] {
            let mut midi = sab(&words, native, false);
            for track in &mut midi.tracks {
                for (i, note) in track
                    .events
                    .iter_mut()
                    .filter_map(|event| {
                        if let Kind::NoteOn(note) = &mut event.kind {
                            Some(note)
                        } else {
                            None
                        }
                    })
                    .enumerate()
                {
                    if i == 1 {
                        note.lyrics[0].extend_ticks = Some(480);
                    } else if i == 2 {
                        note.lyrics.clear();
                    }
                }
            }
            assert_lanes(&midi, &expected);
        }
    }
}

#[test]
fn proven_phrase_and_punctuation_evidence_never_relocate_a_consonant() {
    use verse_lib::engine::midi::Syllabic;
    let mut notes = direct_notes(&["tout", "au", "bout", "de"]);
    edit_source(&mut notes[2]).syllabic = Some(Syllabic::Begin);
    edit_source(&mut notes[3]).syllabic = Some(Syllabic::End);
    direct_apply(&mut notes);
    assert_eq!(
        notes.iter().map(phones).collect::<Vec<_>>(),
        [
            Some("fr/t fr/ou"),
            Some("fr/t fr/oh"),
            Some("fr/b fr/ou"),
            Some("fr/d fr/ee")
        ]
    );
    let mut unknown = direct_notes(&["bout", "de"]);
    edit_source(&mut unknown[0]).syllabic = Some(Syllabic::Begin);
    edit_source(&mut unknown[1]).syllabic = Some(Syllabic::End);
    let before = unknown.clone();
    direct_apply(&mut unknown);
    assert_eq!(unknown, before);
    let mut punctuation = direct_notes(&["laisse,", "se,"]);
    edit_source(&mut punctuation[1]).syllabic = Some(Syllabic::End);
    direct_apply(&mut punctuation);
    assert_eq!(phones(&punctuation[0]), Some("fr/l fr/ae fr/s"));
    assert_eq!(phones(&punctuation[1]), Some("fr/s fr/ee"));
}

#[test]
fn verse_labels_require_matching_explicit_verse_and_keep_literal_variants() {
    for (word, verse, expected) in [
        ("1.Et", 1, Some("fr/eh")),
        ("2.Et", 2, Some("fr/eh")),
        ("3.Et", 1, None),
        ("02.Et", 2, None),
        ("2.rêves(2)", 2, Some("fr/r fr/ae fr/v fr/ee")),
        ("rêves(2)", 1, Some("fr/r fr/ae fr/v fr/ee")),
        ("2.Et[manual]", 2, None),
    ] {
        let mut notes = direct_notes(&[word]);
        edit_source(&mut notes[0]).verse = verse;
        edit_source(&mut notes[0]).verse_from_score = true;
        let original = notes[0].lyric.clone();
        direct_apply(&mut notes);
        assert_eq!(phones(&notes[0]), expected, "{word}");
        if let ProjectedLyric::Pronounced { source, .. } = &notes[0].lyric {
            assert_eq!(ProjectedLyric::Source(source.clone()), original);
        } else {
            assert_eq!(notes[0].lyric, original);
        }
    }
}

fn divided_word() -> Midi {
    let mut midi = sab(&["chan", "ger"], true, false);
    let original = midi.tracks[0].clone();
    let mut head = original.clone();
    let mut tail = original;
    head.events.retain(|e| match &e.kind {
        Kind::NoteOn(_) => e.tick == 0,
        Kind::NoteOff(_) => e.tick == 480,
        _ => false,
    });
    tail.events.retain(|e| match &e.kind {
        Kind::NoteOn(_) => e.tick == 480,
        Kind::NoteOff(_) => e.tick == 960,
        _ => false,
    });
    head.id = "separated-chord-member-a".into();
    tail.id = "continuing-source-line".into();
    let mut copy = head.clone();
    copy.id = "separated-chord-member-b".into();
    for event in &mut copy.events {
        match &mut event.kind {
            Kind::NoteOn(note) => {
                note.source.id.push_str("-copy");
                note.key = Some(67);
            }
            Kind::NoteOff(note) => {
                note.source_id.as_mut().unwrap().push_str("-copy");
                note.key = Some(67);
            }
            _ => {}
        }
    }
    midi.tracks = vec![tail, head, copy];
    // Deliberately identical display names cannot establish a context domain.
    for track in &mut midi.tracks {
        track.name = "Same display name".into();
    }
    midi.topology = verse_lib::engine::midi::SourceTopology::from_tracks(&midi.tracks);
    midi
}

#[test]
fn divided_word_uses_written_provenance_without_changing_ownership_or_geometry() {
    for explicit in [false, true] {
        let mut midi = divided_word();
        if explicit {
            for (i, track) in midi.tracks.iter_mut().enumerate() {
                for event in &mut track.events {
                    if let Kind::NoteOn(note) = &mut event.kind {
                        note.lyrics[0].syllabic = Some(if i == 0 {
                            verse_lib::engine::midi::Syllabic::End
                        } else {
                            verse_lib::engine::midi::Syllabic::Begin
                        });
                    }
                }
            }
        }
        let before = format!("{midi:?}");
        let result = convert(&midi);
        let default = convert_midi_with_target(&midi, "english", None, ExportTarget::Ustx);
        let project = result.svp.as_ref().unwrap();
        assert_eq!(project.tracks.len(), 3);
        assert_eq!(
            project
                .tracks
                .iter()
                .flat_map(|t| &t.notes)
                .map(phones)
                .collect::<Vec<_>>(),
            [Some("fr/j fr/eh"), Some("fr/sh fr/en"), Some("fr/sh fr/en")]
        );
        assert_eq!(result.projection, default.projection);
        assert_eq!(result.topology, default.topology);
        assert_eq!(format!("{midi:?}"), before);
        for (actual, original) in model(&result)
            .voice_parts
            .iter()
            .zip(model(&default).voice_parts)
        {
            assert_eq!(actual.name, original.name);
            for (actual, original) in actual.notes.iter().zip(original.notes) {
                assert_eq!(
                    (actual.position, actual.duration, actual.tone),
                    (original.position, original.duration, original.tone)
                );
                assert_eq!(actual.pitch, original.pitch);
                assert_eq!(actual.vibrato, original.vibrato);
            }
        }
        for track in &result.tracks {
            assert_eq!(
                track
                    .warnings
                    .iter()
                    .filter(|d| d.code == french::APPLIED)
                    .count(),
                1
            );
            assert!(!track.warnings.iter().any(|d| d.code == french::UNSUPPORTED));
            assert!(track
                .warnings
                .iter()
                .filter(|d| d.code == french::APPLIED)
                .all(|d| d
                    .source_id
                    .as_ref()
                    .is_some_and(|id| id.contains(&track.source_id))));
        }
    }
}

#[test]
fn divided_context_rejects_other_domains_competing_identity_and_manual_input() {
    use verse_lib::engine::midi::Syllabic;
    for boundary in [
        "part",
        "staff",
        "voice",
        "row",
        "verse",
        "repeat",
        "identity",
        "single",
        "manual",
        "gap",
        "duration",
        "missing-provenance",
    ] {
        let mut midi = divided_word();
        if boundary == "identity" {
            for event in &mut midi.tracks[2].events {
                if let Kind::NoteOn(note) = &mut event.kind {
                    note.lyrics[0].id.push_str("-unrelated");
                }
            }
        } else if boundary == "duration" {
            for event in &mut midi.tracks[2].events {
                if matches!(event.kind, Kind::NoteOff(_)) {
                    event.tick -= 240;
                }
            }
        } else {
            for event in &mut midi.tracks[0].events {
                if boundary == "gap" {
                    event.tick += 240;
                }
                if let Kind::NoteOn(note) = &mut event.kind {
                    match boundary {
                        "part" => note.source.part_id = Some("another".into()),
                        "staff" => note.source.staff_id = Some("another".into()),
                        "voice" => note.source.voice = Some("another".into()),
                        "missing-provenance" => note.source.voice = None,
                        "row" => note.lyrics[0].lane = "2".into(),
                        "verse" => note.lyrics[0].verse = 2,
                        "repeat" => note.source.occurrence = 1,
                        "single" => note.lyrics[0].syllabic = Some(Syllabic::Single),
                        "manual" => {
                            note.lyrics[0].state =
                                verse_lib::engine::midi::LyricState::Text("ger[manual]".into())
                        }
                        "gap" => {}
                        _ => unreachable!(),
                    }
                }
            }
        }
        let result = convert(&midi);
        assert!(
            result
                .svp
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .flat_map(|t| &t.notes)
                .all(|note| phones(note) != Some("fr/sh fr/en")),
            "{boundary}"
        );
        assert!(
            !result
                .tracks
                .iter()
                .flat_map(|t| &t.warnings)
                .any(|d| d.message.contains("matching written lyric provenance")),
            "{boundary}"
        );
    }
}

#[test]
fn overlapping_source_material_blocks_cross_member_context() {
    let mut midi = divided_word();
    // A different written lyric is still sounding when the supposed successor
    // begins; onset order alone cannot establish an independent lexical chain.
    let mut rival = midi.tracks[1].clone();
    rival.id = "competing-material".into();
    for event in &mut rival.events {
        event.tick += 240;
        match &mut event.kind {
            Kind::NoteOn(note) => {
                note.source.id.push_str("-rival");
                note.source.chord_id = Some("other-chord".into());
                note.lyrics[0].id = "different-written-lyric".into();
            }
            Kind::NoteOff(note) => note.source_id.as_mut().unwrap().push_str("-rival"),
            _ => {}
        }
    }
    midi.tracks.push(rival);
    let result = convert(&midi);
    assert!(!result
        .tracks
        .iter()
        .flat_map(|t| &t.warnings)
        .any(|d| d.message.contains("matching written lyric provenance")));
}

#[test]
fn short_layouts_and_punctuation_cannot_consume_a_longer_explicit_word() {
    use verse_lib::engine::midi::Syllabic;
    let mut notes = direct_notes(&["gar", "de", "ra"]);
    for (note, binding) in notes
        .iter_mut()
        .zip([Syllabic::Begin, Syllabic::Middle, Syllabic::End])
    {
        edit_source(note).syllabic = Some(binding);
    }
    direct_apply(&mut notes);
    assert_ne!(phones(&notes[1]), Some("fr/d fr/ee"));
    assert_ne!(phones(&notes[0]), Some("fr/g fr/ah fr/r"));
    let mut notes = direct_notes(&["laisse,", "se", "cret"]);
    edit_source(&mut notes[1]).syllabic = Some(Syllabic::Begin);
    edit_source(&mut notes[2]).syllabic = Some(Syllabic::End);
    direct_apply(&mut notes);
    assert_ne!(phones(&notes[1]), Some("fr/s fr/ee"));
    assert_eq!(phones(&notes[0]), Some("fr/l fr/ae fr/s"));
}

#[test]
fn a_true_hold_before_a_rest_keeps_independent_complete_layout_attacks() {
    use verse_lib::engine::midi::Syllabic;
    let mut notes = direct_notes(&["gar", "", "de"]);
    edit_source(&mut notes[0]).syllabic = Some(Syllabic::Begin);
    edit_source(&mut notes[2]).syllabic = Some(Syllabic::End);
    notes[1].lyric = ProjectedLyric::Extension;
    notes[2].onset_ticks += 480;
    let geometry: Vec<_> = notes
        .iter()
        .map(|n| (n.onset_ticks, n.duration_ticks, n.pitch))
        .collect();
    direct_apply(&mut notes);
    assert_eq!(phones(&notes[0]), Some("fr/g fr/ah fr/r"));
    assert_eq!(phones(&notes[2]), Some("fr/d fr/ee"));
    assert_eq!(notes[1].lyric, ProjectedLyric::Extension);
    assert_eq!(
        notes
            .iter()
            .map(|n| (n.onset_ticks, n.duration_ticks, n.pitch))
            .collect::<Vec<_>>(),
        geometry
    );
}

#[test]
fn explicit_different_verses_cannot_absorb_a_blank_as_a_melisma() {
    use verse_lib::engine::midi::Syllabic;
    let mut notes = direct_notes(&["gar", "", "de"]);
    edit_source(&mut notes[0]).syllabic = Some(Syllabic::Begin);
    edit_source(&mut notes[2]).syllabic = Some(Syllabic::End);
    edit_source(&mut notes[2]).verse = 2;
    notes[1].lyric = ProjectedLyric::Absent;
    direct_apply(&mut notes);
    assert_eq!(notes[1].lyric, ProjectedLyric::Absent);
}

#[test]
fn score_verse_labels_cover_joined_words_and_exact_ustx_text_but_not_midi_defaults() {
    use verse_lib::engine::midi::{LyricState, Syllabic};
    for native in [false, true] {
        let mut midi = sab(&["2.Et", "2.bon", "jour"], native, false);
        for track in &mut midi.tracks {
            for event in &mut track.events {
                if let Kind::NoteOn(note) = &mut event.kind {
                    let lyric = &mut note.lyrics[0];
                    assert!(lyric.verse_from_score);
                    lyric.verse = 2;
                    lyric.lane = "2".into();
                    lyric.syllabic = match lyric.raw.as_str() {
                        "2.bon" => Some(Syllabic::Begin),
                        "jour" => Some(Syllabic::End),
                        _ => None,
                    };
                }
            }
        }
        assert_lanes(
            &midi,
            &["et[fr/eh]", "bonjour[fr/b fr/on fr/j fr/ou fr/r]", "+"],
        );
        for track in &mut midi.tracks {
            for event in &mut track.events {
                if let Kind::NoteOn(note) = &mut event.kind {
                    if note.lyrics[0].raw == "2.Et" {
                        note.lyrics[0].raw = "1.Et".into();
                        note.lyrics[0].state = LyricState::Text("1.Et".into());
                        note.lyrics[0].verse = 1;
                        note.lyrics[0].verse_from_score = false;
                    }
                }
            }
        }
        let result = convert(&midi);
        for part in model(&result).voice_parts {
            assert_eq!(part.notes[0].lyric, "1.Et");
        }
    }
    let mut midi_default = direct_notes(&["1.Et"]);
    assert!(!edit_source(&mut midi_default[0]).verse_from_score);
    direct_apply(&mut midi_default);
    assert_eq!(phones(&midi_default[0]), None);
}

#[test]
fn score_adapters_establish_numbered_verse_evidence_without_source_id_heuristics() {
    let mscx = r#"<museScore version="4.0"><Score><Division>480</Division><Part><Staff id="1"/><trackName>Voice</trackName></Part><Staff id="1"><Measure><voice><Chord><durationType>quarter</durationType><Lyrics><no>1</no><text>2.Et</text></Lyrics><Note><pitch>60</pitch><tpc>14</tpc></Note></Chord></voice></Measure></Staff></Score></museScore>"#;
    let xml = r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure number="1"><attributes><divisions>1</divisions></attributes><note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration><lyric number="2"><text>2.Et</text></lyric></note></measure></part></score-partwise>"#;
    for midi in [
        musescore::parse(mscx.as_bytes()).unwrap(),
        musicxml::parse(xml.as_bytes()).unwrap(),
    ] {
        let lyric = midi
            .tracks
            .iter()
            .flat_map(|t| &t.events)
            .find_map(|e| match &e.kind {
                Kind::NoteOn(n) => n.lyrics.first(),
                _ => None,
            })
            .unwrap();
        assert_eq!(lyric.verse, 2);
        assert!(lyric.verse_from_score);
        assert_eq!(
            model(&convert(&midi)).voice_parts[0].notes[0].lyric,
            "et[fr/eh]"
        );
    }
    let named = musicxml::parse(
        xml.replace("number=\"2\"", "number=\"chorus\"")
            .replace("2.Et", "1.Et")
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(
        model(&convert(&named)).voice_parts[0].notes[0].lyric,
        "1.Et"
    );
}

#[test]
fn cross_member_context_respects_untexted_geometry_chord_ids_and_blank_priority() {
    use verse_lib::engine::midi::{Lyric, LyricState};
    for case in [
        "untexted-chord",
        "blank-duplicate",
        "conflict",
        "untexted-overlap",
        "untexted-other-chord",
        "missing-chord",
        "different-chord",
    ] {
        let mut midi = divided_word();
        for event in &mut midi.tracks[2].events {
            if case == "untexted-overlap" {
                event.tick += 240;
            }
            if let Kind::NoteOn(note) = &mut event.kind {
                match case {
                    "untexted-chord" | "untexted-overlap" => note.lyrics.clear(),
                    "untexted-other-chord" => {
                        note.lyrics.clear();
                        note.source.chord_id = Some("other".into());
                    }
                    "missing-chord" => note.source.chord_id = None,
                    "different-chord" => note.source.chord_id = Some("other".into()),
                    "blank-duplicate" | "conflict" => {
                        let mut duplicate = Lyric::text("duplicate", "".into());
                        if case == "conflict" {
                            duplicate.state = LyricState::Text("other".into());
                        }
                        note.lyrics.insert(0, duplicate);
                    }
                    _ => unreachable!(),
                }
            }
        }
        let result = convert(&midi);
        let accepted = ["untexted-chord", "blank-duplicate"].contains(&case);
        assert_eq!(
            result
                .tracks
                .iter()
                .flat_map(|t| &t.warnings)
                .any(|d| d.message.contains("matching written lyric provenance")),
            accepted,
            "{case}"
        );
        if case.starts_with("untexted") {
            assert!(
                result
                    .svp
                    .as_ref()
                    .unwrap()
                    .tracks
                    .iter()
                    .filter(|t| t.source_track_id == midi.tracks[2].id)
                    .flat_map(|t| &t.notes)
                    .all(|n| matches!(n.lyric, ProjectedLyric::Absent)),
                "{case}"
            );
        }
        if accepted {
            assert_eq!(
                phones(&result.svp.as_ref().unwrap().tracks[0].notes[0]),
                Some("fr/j fr/eh")
            );
            assert_eq!(
                phones(&result.svp.as_ref().unwrap().tracks[1].notes[0]),
                Some("fr/sh fr/en")
            );
        }
    }
}

#[test]
fn cross_member_liaison_uses_the_whole_source_word_and_its_boundaries() {
    use verse_lib::engine::midi::{LyricState, Syllabic};
    for boundary in [
        "les",
        "mes",
        "punctuation",
        "rest",
        "verse",
        "manual",
        "competing",
    ] {
        let mut midi = divided_word();
        for (i, track) in midi.tracks.iter_mut().enumerate() {
            for event in &mut track.events {
                event.tick += 480;
                if let Kind::NoteOn(note) = &mut event.kind {
                    let text = if i == 0 { "mours" } else { "a" };
                    note.lyrics[0].raw = text.into();
                    note.lyrics[0].state = LyricState::Text(text.into());
                    note.lyrics[0].syllabic = Some(if i == 0 {
                        Syllabic::End
                    } else {
                        Syllabic::Begin
                    });
                }
            }
        }
        let mut preceding = sab(
            &[if boundary == "mes" { "mes" } else { "les" }],
            true,
            false,
        )
        .tracks
        .remove(0);
        preceding.id = "preceding-word".into();
        for event in &mut preceding.events {
            if boundary == "rest" && matches!(event.kind, Kind::NoteOff(_)) {
                event.tick -= 240;
            }
            if let Kind::NoteOn(note) = &mut event.kind {
                note.source.id.push_str("-preceding");
                note.source.chord_id = Some("preceding-chord".into());
                note.lyrics[0].id.push_str("-preceding");
                match boundary {
                    "punctuation" => {
                        note.lyrics[0].raw = "les,".into();
                        note.lyrics[0].state = LyricState::Text("les,".into());
                    }
                    "manual" => {
                        note.lyrics[0].raw = "les[fr/l fr/eh]".into();
                        note.lyrics[0].state = LyricState::Text(note.lyrics[0].raw.clone());
                    }
                    "verse" => note.lyrics[0].verse = 2,
                    _ => {}
                }
            }
            if let Kind::NoteOff(note) = &mut event.kind {
                note.source_id.as_mut().unwrap().push_str("-preceding");
            }
        }
        if boundary == "competing" {
            let mut rival = preceding.clone();
            rival
                .events
                .retain(|e| matches!(e.kind, Kind::NoteOn(_) | Kind::NoteOff(_)));
            rival.id = "untexted-competing".into();
            for event in &mut rival.events {
                event.tick += 240;
                if let Kind::NoteOn(note) = &mut event.kind {
                    note.lyrics.clear();
                }
            }
            midi.tracks.push(rival);
        }
        midi.tracks.push(preceding);
        let result = convert(&midi);
        for index in [1, 2] {
            let phone = phones(&result.svp.as_ref().unwrap().tracks[index].notes[0]);
            if ["les", "mes"].contains(&boundary) {
                assert_eq!(phone, Some("fr/z fr/ah"), "{boundary}");
            } else {
                assert_ne!(phone, Some("fr/z fr/ah"), "{boundary}");
            }
        }
    }
}

#[test]
fn real_mscx_and_musicxml_chords_recover_split_word_readings() {
    let mscx = r#"<museScore version="4.0"><Score><Division>480</Division><Part><Staff id="1"/><trackName>Choir</trackName></Part><Staff id="1"><Measure><voice><TimeSig><sigN>2</sigN><sigD>4</sigD></TimeSig><Chord><durationType>quarter</durationType><Lyrics><syllabic>begin</syllabic><text>chan</text></Lyrics><Note><pitch>60</pitch><tpc>14</tpc></Note><Note><pitch>64</pitch><tpc>18</tpc></Note></Chord><Chord><durationType>quarter</durationType><Lyrics><syllabic>end</syllabic><text>ger</text></Lyrics><Note><pitch>62</pitch><tpc>16</tpc></Note></Chord></voice></Measure></Staff></Score></museScore>"#;
    let xml = r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Choir</part-name></score-part></part-list><part id="P1"><measure number="1"><attributes><divisions>1</divisions><time><beats>2</beats><beat-type>4</beat-type></time></attributes><note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration><voice>1</voice></note><note><chord/><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration><voice>1</voice><lyric><syllabic>begin</syllabic><text>chan</text></lyric></note><note><pitch><step>D</step><octave>4</octave></pitch><duration>1</duration><voice>1</voice><lyric><syllabic>end</syllabic><text>ger</text></lyric></note></measure></part></score-partwise>"#;
    for (midi, expected_notes) in [
        (musescore::parse(mscx.as_bytes()).unwrap(), 3),
        (musicxml::parse(xml.as_bytes()).unwrap(), 2),
    ] {
        let original = format!("{midi:?}");
        let result = convert(&midi);
        let notes: Vec<_> = result
            .svp
            .as_ref()
            .unwrap()
            .tracks
            .iter()
            .flat_map(|t| &t.notes)
            .collect();
        assert_eq!(notes.len(), expected_notes);
        let default = convert_midi_with_target(&midi, "english", None, ExportTarget::Ustx);
        assert_eq!(result.projection, default.projection);
        assert_eq!(
            midi.tracks
                .iter()
                .flat_map(|t| &t.events)
                .filter(|e| matches!(e.kind, Kind::NoteOn(_)))
                .count(),
            3
        );
        let heads: Vec<_> = notes
            .iter()
            .filter(|n| n.onset_ticks == 0 && phones(n).is_some())
            .collect();
        assert!(!heads.is_empty());
        assert!(heads.iter().all(|n| phones(n) == Some("fr/sh fr/en")));
        assert_eq!(
            phones(
                notes
                    .iter()
                    .find(|n| n.onset_ticks == midi.ticks_per_beat as u32)
                    .unwrap()
            ),
            Some("fr/j fr/eh")
        );
        assert!(result
            .tracks
            .iter()
            .flat_map(|t| &t.warnings)
            .any(|d| d.message.contains("matching written lyric provenance")));
        assert_eq!(format!("{midi:?}"), original);
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}

/// Three independent voices, all touching quarter notes. None of the source
/// lyrics has syllabic metadata, as in the original chant arrangement.
fn sab(words: &[&str], native: bool, repeat: bool) -> Midi {
    let mut parts = String::new();
    let mut staves = String::new();
    for (index, name) in ["Soprano", "Alto", "Bass"].iter().enumerate() {
        let id = index + 1;
        let octave = if index == 2 { 3 } else { 4 };
        let pitch = if index == 2 { 48 } else { 60 };
        if native {
            parts.push_str(&format!("<Part><Staff id=\"{id}\"/><trackName>{name}</trackName><Instrument><trackName>{name}</trackName></Instrument></Part>"));
            let notes: String = words.iter().map(|word| format!("<Chord><durationType>quarter</durationType><Lyrics><text>{}</text></Lyrics><Note><pitch>{pitch}</pitch><tpc>14</tpc></Note></Chord>", escape(word))).collect();
            staves.push_str(&format!("<Staff id=\"{id}\"><Measure len=\"{}/4\">{}<voice><TimeSig><sigN>{}</sigN><sigD>4</sigD></TimeSig>{notes}</voice>{}</Measure></Staff>", words.len(), if repeat { "<startRepeat/>" } else { "" }, words.len(), if repeat { "<endRepeat>2</endRepeat>" } else { "" }));
        } else {
            parts.push_str(&format!(
                "<score-part id=\"P{id}\"><part-name>{name}</part-name></score-part>"
            ));
            let notes: String = words.iter().map(|word| format!("<note><pitch><step>C</step><octave>{octave}</octave></pitch><duration>1</duration><lyric><text>{}</text></lyric></note>", escape(word))).collect();
            staves.push_str(&format!("<part id=\"P{id}\"><measure number=\"1\"><attributes><divisions>1</divisions><time><beats>{}</beats><beat-type>4</beat-type></time></attributes>{}{notes}{}</measure></part>", words.len(), if repeat { "<barline location=\"left\"><repeat direction=\"forward\"/></barline>" } else { "" }, if repeat { "<barline location=\"right\"><repeat direction=\"backward\"/></barline>" } else { "" }));
        }
    }
    if native {
        musescore::parse(format!("<museScore version=\"4.0\"><Score><Division>480</Division>{parts}{staves}</Score></museScore>").as_bytes()).unwrap()
    } else {
        musicxml::parse(format!("<score-partwise version=\"4.0\"><part-list>{parts}</part-list>{staves}</score-partwise>").as_bytes()).unwrap()
    }
}

fn convert(midi: &Midi) -> ConvertOutcome {
    let result = convert_midi_with_profile(midi, "english", None, ExportTarget::Ustx, FR);
    assert!(result.ok, "{:?}", result.msg);
    result
}

fn model(result: &ConvertOutcome) -> ustx::UstxProject {
    ustx::serialize(result.svp.as_ref().unwrap()).unwrap()
}

fn assert_lanes(midi: &Midi, expected: &[&str]) {
    let snapshot = format!("{midi:?}");
    let corrected = convert(midi);
    let output = model(&corrected);
    assert_eq!(output.voice_parts.len(), 3);
    for part in &output.voice_parts {
        assert_eq!(
            part.notes
                .iter()
                .map(|note| note.lyric.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{}",
            part.name
        );
    }
    assert!(output
        .tracks
        .iter()
        .all(|track| track.phonemizer == french::PHONEMIZER));
    let baseline = convert_midi_with_target(midi, "english", None, ExportTarget::Ustx);
    let baseline_model = model(&baseline);
    for (actual, original) in output.voice_parts.iter().zip(&baseline_model.voice_parts) {
        assert_eq!(actual.notes.len(), original.notes.len());
        for (actual, original) in actual.notes.iter().zip(&original.notes) {
            assert_eq!(
                (actual.position, actual.duration, actual.tone),
                (original.position, original.duration, original.tone)
            );
            assert_eq!(actual.pitch, original.pitch);
            assert_eq!(actual.vibrato, original.vibrato);
        }
    }
    assert_eq!(output.tempos, baseline_model.tempos);
    assert_eq!(output.time_signatures, baseline_model.time_signatures);
    assert_eq!(corrected.topology, baseline.topology);
    assert_eq!(corrected.projection, baseline.projection);
    assert_eq!(format!("{midi:?}"), snapshot, "the source IR is immutable");
    let source_lyrics: Vec<_> = midi
        .tracks
        .iter()
        .flat_map(|track| &track.events)
        .filter_map(|event| match &event.kind {
            Kind::NoteOn(note) => Some(&note.lyrics),
            _ => None,
        })
        .flatten()
        .collect();
    for track in &corrected.svp.as_ref().unwrap().tracks {
        for note in &track.notes {
            if let ProjectedLyric::Source(source)
            | ProjectedLyric::Pronounced { source, .. }
            | ProjectedLyric::PronouncedSplit { source } = &note.lyric
            {
                assert!(
                    source_lyrics.iter().any(|original| **original == **source),
                    "every original field survives"
                );
            }
        }
    }
    let mut again = corrected.svp.clone().unwrap();
    for track in &mut again.tracks {
        let ids: Vec<_> = track
            .notes
            .iter()
            .enumerate()
            .map(|(i, _)| i.to_string())
            .collect();
        french::apply(&mut track.notes, &ids);
    }
    assert_eq!(
        again,
        corrected.svp.unwrap(),
        "reprocessing must be idempotent"
    );
    let repeated_conversion = convert(midi);
    assert_eq!(output, model(&repeated_conversion));
}

#[test]
fn phrase_boundaries_cannot_relocate_a_word_consonant() {
    for native in [false, true] {
        for head in ["court.", "court.’", "court!\"", "court,»"] {
            assert_lanes(
                &sab(&[head, "ou", "ourt"], native, false),
                &["court[fr/k fr/ou fr/r]", "ou[fr/ou]", "ourt"],
            );
        }
    }
}

#[test]
fn liaison_uses_whole_layout_identity_and_only_its_head_and_tail() {
    for native in [false, true] {
        assert_lanes(
            &sab(
                &["mes", "a", "mours", "tem", "pê", "tes", "amis"],
                native,
                false,
            ),
            &[
                "mes[fr/m fr/eh]",
                "a[fr/z fr/ah]",
                "mours[fr/m fr/ou fr/r]",
                "tem[fr/t fr/en]",
                "pê[fr/p fr/ae]",
                "tes[fr/t fr/ee]",
                "amis[fr/ah fr/m fr/ih]",
            ],
        );
    }
}

#[test]
fn an_eligible_liaison_pair_at_the_repeat_jump_stays_separate() {
    for native in [false, true] {
        assert_lanes(
            &sab(&["au", "tout"], native, true),
            &[
                "au[fr/oh]",
                "tout[fr/t fr/ou]",
                "au[fr/oh]",
                "tout[fr/t fr/ou]",
            ],
        );
    }
}

#[test]
fn another_source_row_inside_a_melisma_prevents_blank_absorption() {
    use verse_lib::engine::midi::{Lyric, LyricState, Syllabic};
    use verse_lib::engine::projection::ProjectedNote;
    let mut left = Lyric::text("left", "mê".into());
    left.syllabic = Some(Syllabic::Begin);
    let mut right = Lyric::text("right", "me".into());
    right.syllabic = Some(Syllabic::End);
    let mut other_row = Lyric::text("other-row", "+".into());
    other_row.state = LyricState::Continuation;
    other_row.lane = "2".into();
    let mut notes: Vec<_> = [
        ProjectedLyric::Source(Box::new(left)),
        ProjectedLyric::Absent,
        ProjectedLyric::Source(Box::new(other_row)),
        ProjectedLyric::Absent,
        ProjectedLyric::Source(Box::new(right)),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, lyric)| ProjectedNote {
        performance: None,
        onset_ticks: i as u32 * 480,
        duration_ticks: 480,
        pitch: 60,
        lyric,
    })
    .collect();
    let before = notes.clone();
    french::apply(
        &mut notes,
        &(0..5).map(|i| i.to_string()).collect::<Vec<_>>(),
    );
    assert_eq!(
        notes, before,
        "the other row is a boundary even between blank notes"
    );
}

#[test]
fn missing_metadata_sab_layouts_preserve_every_attack_in_both_adapters() {
    let words = [
        "chan-", "ger", "mê", "me", "pres", "se", "rê", "ves,", "tem", "pê", "tes", "rê", "ê",
        "ve,", "vent", "en", "ent,", "court", "ou", "ourt,", "fond", "on", "on,",
    ];
    let expected = [
        "chan[fr/sh fr/en]",
        "ger[fr/j fr/eh]",
        "mê[fr/m fr/ae]",
        "me[fr/m fr/ee]",
        "pres[fr/p fr/r fr/ae]",
        "se[fr/s fr/ee]",
        "rê[fr/r fr/ae]",
        "ves[fr/v fr/ee]",
        "tem[fr/t fr/en]",
        "pê[fr/p fr/ae]",
        "tes[fr/t fr/ee]",
        "rê[fr/r fr/ae]",
        "ê[fr/ae]",
        "ve[fr/v fr/ee]",
        "vent[fr/v fr/en]",
        "en[fr/en]",
        "ent[fr/en]",
        "court[fr/k fr/ou]",
        "ou[fr/ou]",
        "ourt[fr/ou fr/r]",
        "fond[fr/f fr/on]",
        "on[fr/on]",
        "on[fr/on]",
    ];
    for native in [false, true] {
        assert_lanes(&sab(&words, native, false), &expected);
    }
}

#[test]
fn lexical_endings_punctuation_nasals_and_elisions() {
    let words = [
        "blessures,",
        "murs!",
        "amours,",
        "laisses",
        "genoux.",
        "vent,",
        "court?",
        "un",
        "Un,",
        "d'un",
        "D’un!",
        "Hou",
    ];
    let expected = [
        "blessures[fr/b fr/l fr/ae fr/s fr/uh fr/r]",
        "murs[fr/m fr/uh fr/r]",
        "amours[fr/ah fr/m fr/ou fr/r]",
        "laisses[fr/l fr/ae fr/s]",
        "genoux[fr/j fr/ee fr/n fr/ou]",
        "vent[fr/v fr/en]",
        "court[fr/k fr/ou fr/r]",
        "un[fr/in]",
        "un[fr/in]",
        "d'un[fr/d fr/in]",
        "d'un[fr/d fr/in]",
        "hou[fr/ou]",
    ];
    for native in [false, true] {
        assert_lanes(&sab(&words, native, false), &expected);
    }
}

#[test]
fn closing_apostrophes_are_punctuation_but_internal_elisions_remain() {
    for native in [false, true] {
        assert_lanes(
            &sab(
                &[
                    "rêves',",
                    "RÊVES’,",
                    "d'un",
                    "D’UN’,",
                    "Tout,'",
                    "au",
                    "l'",
                    "rê'ves",
                    "J'i",
                    "rai",
                    "la",
                ],
                native,
                false,
            ),
            &[
                "rêves[fr/r fr/ae fr/v]",
                "rêves[fr/r fr/ae fr/v]",
                "d'un[fr/d fr/in]",
                "d'un[fr/d fr/in]",
                "tout[fr/t fr/ou]",
                "au[fr/oh]",
                "l'",
                "rê'ves",
                "j'i[fr/j fr/ih]",
                "rai[fr/r fr/ae]",
                "la[fr/l fr/ah]",
            ],
        );
    }
}

#[test]
fn audited_word_layouts_preserve_repeated_vowels_and_only_stated_consonants() {
    for native in [false, true] {
        assert_lanes(
            &sab(&["rêves", "ê", "ves,"], native, false),
            &["rêves[fr/r fr/ae]", "ê[fr/ae]", "ves[fr/v fr/ee]"],
        );
        assert_lanes(
            &sab(
                &[
                    "m'ar", "rê", "te,", "m’ar", "rête", "mur", "mu", "u", "ures,", "mu", "u",
                    "urs,", "ju", "ure,", "rê",
                ],
                native,
                false,
            ),
            &[
                "m'ar[fr/m fr/ah]",
                "rê[fr/r fr/ae]",
                "te[fr/t fr/ee]",
                "m'ar[fr/m fr/ah]",
                "rête[fr/r fr/ae fr/t]",
                "mur[fr/m fr/uh fr/r]",
                "mu[fr/m fr/uh]",
                "u[fr/uh]",
                "ures[fr/uh fr/r]",
                "mu[fr/m fr/uh]",
                "u[fr/uh]",
                "urs[fr/uh fr/r]",
                "ju[fr/j fr/uh]",
                "ure[fr/uh fr/r]",
                "rê[fr/r fr/ae]",
            ],
        );
        assert_lanes(
            &sab(&["mur", "mu", "u", "ure,", "mu", "u", "ur,"], native, false),
            &[
                "mur[fr/m fr/uh fr/r]",
                "mu[fr/m fr/uh]",
                "u[fr/uh]",
                "ure[fr/uh fr/r]",
                "mu[fr/m fr/uh]",
                "u[fr/uh]",
                "ur[fr/uh fr/r]",
            ],
        );
    }
}

#[test]
fn vowel_fragments_outside_the_audited_layouts_stay_unsupported() {
    let midi = sab(
        &["u", "ure,", "urs,", "la", "J'i", "zyx", "rai"],
        false,
        false,
    );
    assert_lanes(
        &midi,
        &["u", "ure,", "urs,", "la[fr/l fr/ah]", "J'i", "zyx", "rai"],
    );
    for track in convert(&midi)
        .tracks
        .iter()
        .filter(|track| track.placed > 0)
    {
        assert_eq!(
            track
                .warnings
                .iter()
                .filter(|w| w.code == french::UNSUPPORTED)
                .count(),
            6
        );
    }
}

#[test]
fn liaison_is_bounded_and_already_spelled_consonants_are_not_doubled() {
    let words = [
        "Tout", "au", "tout", "à", "tout", "tau", "mes", "amis", "un", "ami", "est", "un", "les",
        "hommes", "tout,", "au", "mes", "haricots",
    ];
    let expected = [
        "tout[fr/t fr/ou]",
        "au[fr/t fr/oh]",
        "tout[fr/t fr/ou]",
        "à[fr/t fr/ah]",
        "tout[fr/t fr/ou]",
        "tau[fr/t fr/oh]",
        "mes[fr/m fr/eh]",
        "amis[fr/z fr/ah fr/m fr/ih]",
        "un[fr/in]",
        "ami[fr/n fr/ah fr/m fr/ih]",
        "est[fr/ae]",
        "un[fr/t fr/in]",
        "les[fr/l fr/eh]",
        "hommes[fr/z fr/oo fr/m]",
        "tout[fr/t fr/ou]",
        "au[fr/oh]",
        "mes[fr/m fr/eh]",
        "haricots[fr/ah fr/r fr/ih fr/k fr/oh]",
    ];
    assert_lanes(&sab(&words, false, false), &expected);
}

#[test]
fn unknowns_pronounced_consonants_and_manual_hints_are_retained() {
    let midi = sab(
        &[
            "bus",
            "gaz,",
            "sud",
            "net",
            "Xylophonie!",
            "un[fr/un]",
            "rêves(2)",
        ],
        false,
        false,
    );
    assert_lanes(
        &midi,
        &[
            "bus",
            "gaz[fr/g fr/ah fr/z]",
            "sud[fr/s fr/uh fr/d]",
            "net[fr/n fr/ae fr/t]",
            "Xylophonie!",
            "un[fr/un]",
            "rêves(2)[fr/r fr/ae fr/v fr/ee]",
        ],
    );
    let outcome = convert(&midi);
    assert_eq!(
        outcome
            .tracks
            .iter()
            .flat_map(|track| &track.warnings)
            .filter(|warning| warning.code == french::UNSUPPORTED)
            .count(),
        6
    );
}

#[test]
fn repeats_reapply_from_original_source_and_preserve_musical_data() {
    for native in [false, true] {
        let midi = sab(&["mê", "me", "Tout", "au"], native, true);
        assert_lanes(
            &midi,
            &[
                "mê[fr/m fr/ae]",
                "me[fr/m fr/ee]",
                "tout[fr/t fr/ou]",
                "au[fr/t fr/oh]",
                "mê[fr/m fr/ae]",
                "me[fr/m fr/ee]",
                "tout[fr/t fr/ou]",
                "au[fr/t fr/oh]",
            ],
        );
    }
}

#[test]
fn selected_french_profile_cannot_change_svp() {
    let midi = sab(&["chan-", "ger", "Un,", "vent"], false, false);
    let default = convert_midi_with_target(&midi, "french", None, ExportTarget::Svp);
    let selected = convert_midi_with_profile(&midi, "french", None, ExportTarget::Svp, FR);
    assert_eq!(default.svp, selected.svp);
    assert_eq!(
        target::serialize_to(ExportTarget::Svp, default.svp.as_ref().unwrap()),
        target::serialize_to(ExportTarget::Svp, selected.svp.as_ref().unwrap())
    );
    assert_eq!(
        default
            .tracks
            .iter()
            .map(|t| &t.warnings)
            .collect::<Vec<_>>(),
        selected
            .tracks
            .iter()
            .map(|t| &t.warnings)
            .collect::<Vec<_>>()
    );
}

#[test]
fn broad_dictionary_normalizes_unicode_and_keeps_pronounced_exceptions() {
    for native in [false, true] {
        assert_lanes(
            &sab(
                &[
                    "FLEURS!",
                    "grand",
                    "chats",
                    "parfum",
                    "gagner",
                    "montagne",
                    "ABI\u{302}ME",
                    "d’abîme",
                    "bus(2)",
                    "fils(4)",
                ],
                native,
                false,
            ),
            &[
                "fleurs[fr/f fr/l fr/oe fr/r]",
                "grand[fr/g fr/r fr/en]",
                "chats[fr/sh fr/ah]",
                "parfum[fr/p fr/ah fr/r fr/f fr/in]",
                "gagner[fr/g fr/ah fr/n fr/y fr/eh]",
                "montagne[fr/m fr/on fr/t fr/ah fr/n fr/y]",
                "abîme[fr/ah fr/b fr/ih fr/m]",
                "d'abîme[fr/d fr/ah fr/b fr/ih fr/m]",
                "bus(2)[fr/b fr/uh fr/s]",
                "fils(4)[fr/f fr/ih fr/s]",
            ],
        );
    }
}

#[test]
fn ambiguous_homographs_and_forced_input_remain_unforced() {
    let words = [
        "bus",
        "fils",
        "président",
        "couvent",
        "plus",
        "tous",
        "?vent",
        "l'",
        "j’",
        "SP",
        "AP",
        "br",
        "R",
    ];
    assert_lanes(&sab(&words, false, false), &words);
}

#[test]
fn punctuation_parentheses_do_not_hide_words_or_erase_literal_variants() {
    assert_lanes(
        &sab(
            &["(le", "vent)", "(rêves.)", "(rêves(2))", "(l')"],
            false,
            false,
        ),
        &[
            "le[fr/l fr/ee]",
            "vent[fr/v fr/en]",
            "rêves[fr/r fr/ae fr/v]",
            "rêves(2)[fr/r fr/ae fr/v fr/ee]",
            "(l')",
        ],
    );
}

#[test]
fn dangling_dashes_protect_fragments_and_a_split_marker_is_not_a_hold() {
    assert_lanes(&sab(&["chan-", "ter"], false, false), &["chan", "ter"]);
    // The marker consumes a syllable; it cannot be skipped to find the curated
    // two-syllable rê/ves layout and leave a native + without a vowel.
    let midi = sab(&["rê", "+", "ves"], false, false);
    let result = model(&convert(&midi));
    for part in result.voice_parts {
        assert_eq!(part.notes[2].lyric, "ves");
    }
}

#[test]
fn dictionary_words_use_native_syllable_allocation_without_losing_source_evidence() {
    use verse_lib::engine::midi::{Lyric, Syllabic};
    use verse_lib::engine::projection::ProjectedNote;
    let mut left = Lyric::text("a", "bon".into());
    left.syllabic = Some(Syllabic::Begin);
    let mut right = Lyric::text("b", "jour".into());
    right.syllabic = Some(Syllabic::End);
    let mut notes = vec![
        ProjectedNote {
            performance: None,
            onset_ticks: 0,
            duration_ticks: 480,
            pitch: 60,
            lyric: ProjectedLyric::Source(Box::new(left.clone())),
        },
        ProjectedNote {
            performance: None,
            onset_ticks: 480,
            duration_ticks: 240,
            pitch: 62,
            lyric: ProjectedLyric::Absent,
        },
        ProjectedNote {
            performance: None,
            onset_ticks: 720,
            duration_ticks: 480,
            pitch: 64,
            lyric: ProjectedLyric::Source(Box::new(right.clone())),
        },
    ];
    french::apply(&mut notes, &["0".into(), "1".into(), "2".into()]);
    assert_eq!(
        notes[0].lyric,
        ProjectedLyric::Pronounced {
            source: Box::new(left),
            text: "bonjour".into(),
            phonemes: "fr/b fr/on fr/j fr/ou fr/r".into()
        }
    );
    assert_eq!(notes[1].lyric, ProjectedLyric::Extension);
    assert_eq!(
        notes[2].lyric,
        ProjectedLyric::PronouncedSplit {
            source: Box::new(right)
        }
    );
    assert!(notes[2].lyric.continues_previous_note());
    let once = notes.clone();
    french::apply(&mut notes, &["0".into(), "1".into(), "2".into()]);
    assert_eq!(notes, once);
    // A rest or another lyric row cannot be traversed by the dictionary word.
    for row in [false, true] {
        let mut broken = once.clone();
        for note in &mut broken {
            note.lyric = match &note.lyric {
                ProjectedLyric::Pronounced { source, .. }
                | ProjectedLyric::PronouncedSplit { source } => {
                    ProjectedLyric::Source(source.clone())
                }
                _ => note.lyric.clone(),
            };
        }
        if row {
            if let ProjectedLyric::Source(source) = &mut broken[2].lyric {
                source.lane = "other".into();
            }
        } else {
            broken[2].onset_ticks += 1;
        }
        french::apply(&mut broken, &["0".into(), "1".into(), "2".into()]);
        assert!(!matches!(
            broken[2].lyric,
            ProjectedLyric::PronouncedSplit { .. }
        ));
    }
}

#[test]
fn edge_separators_keep_known_attacks_with_and_without_syllabic_metadata() {
    use verse_lib::engine::midi::Syllabic;
    let expected = [
        "chan[fr/sh fr/en]",
        "ger[fr/j fr/eh]",
        "pres[fr/p fr/r fr/ae]",
        "se[fr/s fr/ee]",
        "mê[fr/m fr/ae]",
        "me[fr/m fr/ee]",
        "rê[fr/r fr/ae]",
        "ves[fr/v fr/ee]",
    ];
    for native in [false, true] {
        assert_lanes(
            &sab(
                &["chan-", "ger", "pres–", "se", "mê—", "me", "rê-", "ves"],
                native,
                false,
            ),
            &expected,
        );
        let mut midi = sab(
            &["chan", "ger", "pres", "se", "mê", "me", "rê", "ves"],
            native,
            false,
        );
        for track in &mut midi.tracks {
            for (i, note) in track
                .events
                .iter_mut()
                .filter_map(|event| match &mut event.kind {
                    Kind::NoteOn(note) => Some(note),
                    _ => None,
                })
                .enumerate()
            {
                for lyric in &mut note.lyrics {
                    lyric.syllabic = Some(if i % 2 == 0 {
                        Syllabic::Begin
                    } else {
                        Syllabic::End
                    });
                }
            }
        }
        assert_lanes(&midi, &expected);
    }
}

#[test]
fn unknown_edge_separators_are_spelling_only_in_direct_and_bundle_exports() {
    use verse_lib::bundle::BundleProject;
    for native in [false, true] {
        for dash in ['-', '‐', '‑', '‒', '–', '—', '―', '−', '﹘', '﹣', '－'] {
            let left = format!("zyx{dash}");
            let right = format!("{dash}qwv");
            let midi = sab(&[&left, &right], native, true);
            assert_lanes(&midi, &["zyx", "qwv", "zyx", "qwv"]);
            let outcome = convert(&midi);
            for track in outcome.tracks.iter().filter(|track| track.placed > 0) {
                assert_eq!(
                    track
                        .warnings
                        .iter()
                        .filter(|w| w.code == french::UNSUPPORTED)
                        .count(),
                    4
                );
            }
            let projected = outcome.svp.as_ref().unwrap();
            let before = projected.clone();
            assert!(projected
                .tracks
                .iter()
                .flat_map(|t| &t.notes)
                .all(|n| matches!(n.lyric, ProjectedLyric::Source(_))));
            let direct = target::serialize_to(ExportTarget::Ustx, projected).unwrap();
            let BundleProject::Ustx(bundle) =
                BundleProject::from_projection(ExportTarget::Ustx, projected).unwrap()
            else {
                panic!("expected USTX");
            };
            assert_eq!(direct, ustx::to_yaml(&bundle).into_bytes());
            assert_eq!(
                target::serialize_to(ExportTarget::Ustx, projected).unwrap(),
                direct
            );
            assert_eq!(projected, &before);

            // Existing Default/SVP word joining remains exactly as before.
            let default = convert_midi_with_target(&midi, "english", None, ExportTarget::Ustx);
            for part in model(&default).voice_parts {
                assert_eq!(
                    part.notes
                        .iter()
                        .map(|n| n.lyric.as_str())
                        .collect::<Vec<_>>(),
                    ["zyxqwv", "+", "zyxqwv", "+"]
                );
            }
            let svp = convert_midi_with_target(&midi, "english", None, ExportTarget::Svp);
            let selected = convert_midi_with_profile(&midi, "english", None, ExportTarget::Svp, FR);
            assert_eq!(svp.svp, selected.svp);
            assert_eq!(
                target::serialize_to(ExportTarget::Svp, svp.svp.as_ref().unwrap()),
                target::serialize_to(ExportTarget::Svp, selected.svp.as_ref().unwrap())
            );
        }
    }
}

#[test]
fn unknown_punctuation_internal_hyphens_and_manual_controls_survive_conversion() {
    let words = [
        " «Zyx-,» ",
        " (-Qwv!) ",
        "arc-en-ciel",
        "zyx-qwv",
        "-",
        "+",
        "+~",
        "?alias-",
        "mot-[phones]",
        "mot[phones]-",
    ];
    let expected = [
        " «Zyx,» ",
        " (Qwv!) ",
        "arc-en-ciel[fr/ah fr/r fr/k fr/en fr/s fr/y fr/ae fr/l]",
        "zyx-qwv",
        "-",
        "+",
        "+~",
        "?alias-",
        "mot-[phones]",
        "mot[phones]-",
    ];
    for native in [false, true] {
        // MuseScore already trims outer whitespace while parsing; the target
        // must preserve the spelling that actually reaches the source IR.
        let expected: Vec<_> = expected
            .iter()
            .map(|text| if native { text.trim() } else { *text })
            .collect();
        assert_lanes(&sab(&words, native, false), &expected);
    }
}

#[test]
fn a_dictionary_cannot_invent_a_third_vowel_for_a_three_note_word() {
    use verse_lib::engine::midi::{Lyric, Syllabic};
    use verse_lib::engine::projection::ProjectedNote;
    let mut notes: Vec<_> = [
        ("mon", Syllabic::Begin),
        ("ta", Syllabic::Middle),
        ("gne", Syllabic::End),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (text, syllabic))| {
        let mut lyric = Lyric::text(i.to_string(), text.into());
        lyric.syllabic = Some(syllabic);
        ProjectedNote {
            performance: None,
            onset_ticks: i as u32 * 480,
            duration_ticks: 480,
            pitch: 60,
            lyric: ProjectedLyric::Source(Box::new(lyric)),
        }
    })
    .collect();
    let before = notes.clone();
    let diagnostics = french::apply(&mut notes, &["0".into(), "1".into(), "2".into()]);
    assert_eq!(notes, before);
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| d.code == french::UNSUPPORTED)
            .count(),
        3
    );
}
