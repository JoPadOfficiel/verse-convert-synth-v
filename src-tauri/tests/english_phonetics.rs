//! EN-001: source-owned English pronunciation and native DiffSinger allocation.
use verse_lib::engine::convert::{convert_midi_with_profile, ConvertOutcome};
use verse_lib::engine::midi::{Kind, Lyric, LyricState, Midi, Syllabic};
use verse_lib::engine::projection::{ProjectedLyric, ProjectedNote};
use verse_lib::engine::target::{self, english, ustx, ExportTarget, PronunciationProfile};
use verse_lib::engine::{musescore, musicxml};

const EN: PronunciationProfile = PronunciationProfile::EnglishArpabet;
const BEAUTIFUL: &str = "beautiful[en/b en/y en/uw en/t en/ah en/f en/ah en/l]";

#[derive(Clone, Copy)]
enum Cell<'a> {
    Word(&'a str, Option<&'static str>),
    Blank,
    Rest,
}

fn word(text: &str) -> Cell<'_> {
    Cell::Word(text, None)
}
fn syllable<'a>(text: &'a str, state: &'static str) -> Cell<'a> {
    Cell::Word(text, Some(state))
}
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}

fn sab(cells: &[Cell<'_>], native: bool, repeat: bool) -> Midi {
    let mut declarations = String::new();
    let mut bodies = String::new();
    for (index, name) in ["Soprano", "Alto", "Bass"].iter().enumerate() {
        let id = index + 1;
        let pitch = if index == 2 { 48 } else { 60 };
        let octave = if index == 2 { 3 } else { 4 };
        let mut notes = String::new();
        for cell in cells {
            let lyrics = match cell {
                Cell::Word(text, state) => {
                    let state = state
                        .map(|s| format!("<syllabic>{s}</syllabic>"))
                        .unwrap_or_default();
                    let tag = if native { "Lyrics" } else { "lyric" };
                    format!("<{tag}>{state}<text>{}</text></{tag}>", escape(text))
                }
                _ => String::new(),
            };
            if native {
                if matches!(cell, Cell::Rest) {
                    notes.push_str("<Rest><durationType>quarter</durationType></Rest>");
                } else {
                    notes.push_str(&format!("<Chord><durationType>quarter</durationType>{lyrics}<Note><pitch>{pitch}</pitch><tpc>14</tpc></Note></Chord>"));
                }
            } else {
                let kind = if matches!(cell, Cell::Rest) {
                    "<rest/>".into()
                } else {
                    format!("<pitch><step>C</step><octave>{octave}</octave></pitch>")
                };
                notes.push_str(&format!(
                    "<note>{kind}<duration>1</duration>{lyrics}</note>"
                ));
            }
        }
        if native {
            declarations.push_str(&format!("<Part><Staff id=\"{id}\"/><trackName>{name}</trackName><Instrument><trackName>{name}</trackName></Instrument></Part>"));
            bodies.push_str(&format!("<Staff id=\"{id}\"><Measure len=\"{}/4\">{}<voice><TimeSig><sigN>{}</sigN><sigD>4</sigD></TimeSig>{notes}</voice>{}</Measure></Staff>", cells.len(), if repeat { "<startRepeat/>" } else { "" }, cells.len(), if repeat { "<endRepeat>2</endRepeat>" } else { "" }));
        } else {
            declarations.push_str(&format!(
                "<score-part id=\"P{id}\"><part-name>{name}</part-name></score-part>"
            ));
            bodies.push_str(&format!("<part id=\"P{id}\"><measure number=\"1\"><attributes><divisions>1</divisions><time><beats>{}</beats><beat-type>4</beat-type></time></attributes>{}{notes}{}</measure></part>", cells.len(), if repeat { "<barline location=\"left\"><repeat direction=\"forward\"/></barline>" } else { "" }, if repeat { "<barline location=\"right\"><repeat direction=\"backward\"/></barline>" } else { "" }));
        }
    }
    if native {
        musescore::parse(format!("<museScore version=\"4.0\"><Score><Division>480</Division>{declarations}{bodies}</Score></museScore>").as_bytes()).unwrap()
    } else {
        musicxml::parse(format!("<score-partwise version=\"4.0\"><part-list>{declarations}</part-list>{bodies}</score-partwise>").as_bytes()).unwrap()
    }
}

fn convert(midi: &Midi) -> ConvertOutcome {
    let outcome = convert_midi_with_profile(midi, "french", None, ExportTarget::Ustx, EN);
    assert!(outcome.ok, "{:?}", outcome.msg);
    outcome
}

fn assert_lanes(midi: &Midi, expected: &[&str]) {
    let original = format!("{midi:?}");
    let result = convert(midi);
    let project = result.svp.as_ref().unwrap();
    let output = ustx::serialize(project).unwrap();
    assert_eq!(output.voice_parts.len(), 3);
    assert!(output
        .tracks
        .iter()
        .all(|track| track.phonemizer == english::PHONEMIZER));
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
    let default = convert_midi_with_profile(
        midi,
        "french",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Default,
    );
    assert!(default.ok);
    let default_output = ustx::serialize(default.svp.as_ref().unwrap()).unwrap();
    assert_eq!(output.tempos, default_output.tempos);
    assert_eq!(output.time_signatures, default_output.time_signatures);
    assert_eq!(result.topology, default.topology);
    assert_eq!(result.projection, default.projection);
    for (actual, before) in output.voice_parts.iter().zip(&default_output.voice_parts) {
        assert_eq!(actual.notes.len(), before.notes.len());
        for (a, b) in actual.notes.iter().zip(&before.notes) {
            assert_eq!(
                (a.position, a.duration, a.tone),
                (b.position, b.duration, b.tone)
            );
            assert_eq!(a.pitch, b.pitch);
            assert_eq!(a.vibrato, b.vibrato);
        }
    }
    let source_lyrics: Vec<_> = midi
        .tracks
        .iter()
        .flat_map(|track| &track.events)
        .filter_map(|event| {
            if let Kind::NoteOn(note) = &event.kind {
                Some(&note.lyrics)
            } else {
                None
            }
        })
        .flatten()
        .collect();
    let mut again = project.clone();
    for track in &mut again.tracks {
        for note in &track.notes {
            if let ProjectedLyric::Pronounced { source, .. }
            | ProjectedLyric::PronouncedSplit { source } = &note.lyric
            {
                assert!(source_lyrics.iter().any(|original| **original == **source));
            }
        }
        let ids = (0..track.notes.len())
            .map(|i| i.to_string())
            .collect::<Vec<_>>();
        english::apply(&mut track.notes, &ids);
    }
    assert_eq!(&again, project, "no repeated hints or split markers");
    assert_eq!(
        format!("{midi:?}"),
        original,
        "source evidence stays unchanged"
    );
    assert_eq!(
        target::serialize_to(ExportTarget::Ustx, project).unwrap(),
        target::serialize_to(ExportTarget::Ustx, convert(midi).svp.as_ref().unwrap()).unwrap()
    );
}

#[test]
fn contractions_case_punctuation_silent_letters_and_english_endings() {
    let words = [
        "She's", "SHE’S,", "Debt,", "debts", "don't!", "isn't", "knight", "lamb", "knee", "sign",
        "island", "cats", "dogs", "walked", "loved",
    ];
    let expected = [
        "she's[en/sh en/iy en/z]",
        "she's[en/sh en/iy en/z]",
        "debt[en/d en/eh en/t]",
        "debts[en/d en/eh en/t en/s]",
        "don't[en/d en/ow en/n en/t]",
        "isn't[en/ih en/z en/ah en/n en/t]",
        "knight[en/n en/ay en/t]",
        "lamb[en/l en/ae en/m]",
        "knee[en/n en/iy]",
        "sign[en/s en/ay en/n]",
        "island[en/ay en/l en/ah en/n en/d]",
        "cats[en/k en/ae en/t en/s]",
        "dogs[en/d en/aa en/g en/z]",
        "walked[en/w en/ao en/k en/t]",
        "loved[en/l en/ah en/v en/d]",
    ];
    for native in [false, true] {
        assert_lanes(&sab(&words.map(word), native, false), &expected);
    }
}

#[test]
fn beautiful_uses_a_whole_word_hint_and_native_splits_on_three_attacks() {
    for native in [false, true] {
        assert_lanes(
            &sab(
                &[
                    syllable("beau", "begin"),
                    syllable("ti", "middle"),
                    syllable("ful", "end"),
                ],
                native,
                false,
            ),
            &[BEAUTIFUL, "+", "+"],
        );
        assert_lanes(
            &sab(&[word("beau-"), word("-ti-"), word("-ful")], native, false),
            &[BEAUTIFUL, "+", "+"],
        );
        assert_lanes(
            &sab(
                &[
                    syllable("beau", "begin"),
                    Cell::Blank,
                    syllable("ti", "middle"),
                    syllable("ful", "end"),
                ],
                native,
                false,
            ),
            &[BEAUTIFUL, "+~", "+", "+"],
        );
    }
}

#[test]
fn variants_are_explicit_and_vowel_mismatch_never_selects_another_reading() {
    for native in [false, true] {
        assert_lanes(
            &sab(
                &[
                    syllable("fi", "begin"),
                    syllable("re", "end"),
                    word("fire(2),"),
                    word("READ(2)!"),
                    word("wound(2)"),
                ],
                native,
                false,
            ),
            &[
                "fire[en/f en/ay en/er]",
                "+",
                "fire(2)[en/f en/ay en/r]",
                "read(2)[en/r en/iy en/d]",
                "wound(2)[en/w en/uw en/n en/d]",
            ],
        );
        for cells in [
            vec![syllable("de", "begin"), syllable("bt", "end")],
            vec![syllable("beau", "begin"), syllable("tiful", "end")],
            vec![syllable("fi", "begin"), syllable("re(2)", "end")],
            vec![
                syllable("beau", "begin"),
                syllable("ti", "middle"),
                syllable("fu", "middle"),
                syllable("l", "end"),
            ],
        ] {
            let midi = sab(&cells, native, false);
            let expected: Vec<_> = cells
                .iter()
                .map(|cell| match cell {
                    Cell::Word(text, _) => *text,
                    _ => unreachable!(),
                })
                .collect();
            assert_lanes(&midi, &expected);
            assert_eq!(
                convert(&midi)
                    .tracks
                    .iter()
                    .flat_map(|track| &track.warnings)
                    .filter(|w| w.code == english::VOWEL_MISMATCH)
                    .count(),
                cells.len() * 3
            );
        }
    }
}

#[test]
fn semantic_homographs_remain_unhinted_without_an_explicit_variant() {
    let words = [
        "read", "wound", "lead", "live", "bow", "tear", "wind", "bass", "close", "desert",
        "present", "object", "record", "content", "minute",
    ];
    let midi = sab(&words.map(word), false, false);
    assert_lanes(&midi, &words);
    assert_eq!(
        convert(&midi)
            .tracks
            .iter()
            .flat_map(|track| &track.warnings)
            .filter(|w| w.code == english::AMBIGUOUS)
            .count(),
        words.len() * 3
    );
}

#[test]
fn orphan_syllabic_metadata_does_not_turn_a_fragment_into_a_dictionary_word() {
    for native in [false, true] {
        for state in ["begin", "middle", "end"] {
            let midi = sab(
                &[syllable("Debt", state), Cell::Rest, word("She's")],
                native,
                false,
            );
            assert_lanes(&midi, &["Debt", "she's[en/sh en/iy en/z]"]);
            assert_eq!(
                convert(&midi)
                    .tracks
                    .iter()
                    .flat_map(|track| &track.warnings)
                    .filter(|warning| warning.code == english::UNSUPPORTED)
                    .count(),
                3
            );
        }
    }
}

#[test]
fn unknown_coherent_fragments_are_not_reinterpreted_as_known_words() {
    let midi = sab(
        &[syllable("debt", "begin"), syllable("zzq", "end")],
        false,
        false,
    );
    assert_lanes(&midi, &["debt", "zzq"]);
    assert_eq!(
        convert(&midi)
            .tracks
            .iter()
            .flat_map(|track| &track.warnings)
            .filter(|w| w.code == english::UNSUPPORTED)
            .count(),
        6
    );
}

#[test]
fn unilateral_dashes_protect_fragments_and_outer_parentheses_preserve_variant_keys() {
    for native in [false, true] {
        for cells in [[word("debt-"), word("zzq")], [word("zzq"), word("-debt")]] {
            let midi = sab(&cells, native, false);
            let expected: Vec<_> = cells
                .iter()
                .map(|cell| match cell {
                    Cell::Word(text, _) => *text,
                    _ => unreachable!(),
                })
                .collect();
            assert_lanes(&midi, &expected);
            assert_eq!(
                convert(&midi)
                    .tracks
                    .iter()
                    .flat_map(|track| &track.warnings)
                    .filter(|warning| warning.code == english::UNSUPPORTED)
                    .count(),
                6
            );
        }
        assert_lanes(
            &sab(
                &[word("(Debt.)"), word("(She's"), word("read(2))")],
                native,
                false,
            ),
            &[
                "debt[en/d en/eh en/t]",
                "she's[en/sh en/iy en/z]",
                "read(2)[en/r en/iy en/d]",
            ],
        );
    }
}

#[test]
fn manual_hints_forced_phonemes_and_control_aliases_are_preserved() {
    let words = [
        "She’s[en/sh en/iy en/z]",
        "?en/sh en/iy en/z",
        "SP",
        "AP",
        "br",
        "R",
        "zzq-not-a-word",
    ];
    assert_lanes(&sab(&words.map(word), false, false), &words);
}

#[test]
fn source_rows_rests_and_manual_boundaries_block_word_allocation() {
    for native in [false, true] {
        let broken = sab(
            &[
                syllable("beau", "begin"),
                Cell::Rest,
                syllable("ti", "middle"),
                syllable("ful", "end"),
            ],
            native,
            false,
        );
        let out = ustx::serialize(convert(&broken).svp.as_ref().unwrap()).unwrap();
        for part in out.voice_parts {
            assert_eq!(part.notes.len(), 3);
            assert!(part
                .notes
                .iter()
                .all(|n| n.lyric != "+" && !n.lyric.starts_with("beautiful[")));
        }
    }
    for boundary in ["row", "single", "manual", "split"] {
        let mut notes: Vec<_> = [
            ("beau", Syllabic::Begin),
            ("ti", Syllabic::Middle),
            ("ful", Syllabic::End),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (text, syllabic))| {
            let mut source = Lyric::text(i.to_string(), text.into());
            source.syllabic = Some(syllabic);
            ProjectedNote {
                onset_ticks: i as u32 * 480,
                duration_ticks: 480,
                pitch: 60,
                lyric: ProjectedLyric::Source(Box::new(source)),
            }
        })
        .collect();
        let ProjectedLyric::Source(source) = &mut notes[1].lyric else {
            unreachable!()
        };
        match boundary {
            "row" => source.lane = "2".into(),
            "single" => source.syllabic = Some(Syllabic::Single),
            "manual" => source.state = LyricState::Text("ti[en/t en/iy]".into()),
            "split" => source.state = LyricState::SyllableSplit,
            _ => unreachable!(),
        }
        let protected = notes[1].lyric.clone();
        english::apply(&mut notes, &["a".into(), "b".into(), "c".into()]);
        assert!(notes
            .iter()
            .all(|n| !matches!(&n.lyric, ProjectedLyric::PronouncedSplit { .. })));
        if matches!(boundary, "manual" | "split") {
            assert_eq!(notes[1].lyric, protected);
        }
    }
}

#[test]
fn genuine_hold_after_a_whole_word_stays_a_hold() {
    let first = Lyric::text("word", "She’s".into());
    let mut held = Lyric::text("hold", String::new());
    held.state = LyricState::Continuation;
    let mut notes = vec![
        ProjectedNote {
            onset_ticks: 0,
            duration_ticks: 480,
            pitch: 60,
            lyric: ProjectedLyric::Source(Box::new(first)),
        },
        ProjectedNote {
            onset_ticks: 480,
            duration_ticks: 480,
            pitch: 62,
            lyric: ProjectedLyric::Source(Box::new(held.clone())),
        },
    ];
    english::apply(&mut notes, &["word".into(), "hold".into()]);
    assert!(
        matches!(&notes[0].lyric,ProjectedLyric::Pronounced { phonemes,.. } if phonemes=="en/sh en/iy en/z")
    );
    assert_eq!(notes[1].lyric, ProjectedLyric::Source(Box::new(held)));
    assert!(notes[1].lyric.continues_previous_note());
}

#[test]
fn repeat_boundaries_do_not_complete_a_word_from_different_passes() {
    for native in [false, true] {
        let midi = sab(
            &[
                syllable("ti", "middle"),
                syllable("ful", "end"),
                syllable("beau", "begin"),
            ],
            native,
            true,
        );
        let out = ustx::serialize(convert(&midi).svp.as_ref().unwrap()).unwrap();
        for part in out.voice_parts {
            assert_eq!(part.notes.len(), 6);
            assert!(part
                .notes
                .iter()
                .all(|n| n.lyric != "+" && !n.lyric.starts_with("beautiful[")));
        }
        assert_lanes(
            &sab(
                &[
                    syllable("beau", "begin"),
                    syllable("ti", "middle"),
                    syllable("ful", "end"),
                ],
                native,
                true,
            ),
            &[BEAUTIFUL, "+", "+", BEAUTIFUL, "+", "+"],
        );
    }
}

#[test]
fn english_selection_never_changes_svp_or_legacy_default_behavior() {
    let midi = sab(&[word("She's,"), word("Debt,"), word("un")], false, false);
    let baseline = verse_lib::engine::convert::convert_midi_with_target(
        &midi,
        "french",
        None,
        ExportTarget::Svp,
    );
    let english = convert_midi_with_profile(&midi, "french", None, ExportTarget::Svp, EN);
    assert_eq!(baseline.svp, english.svp);
    assert_eq!(baseline.projection, english.projection);
    let ustx_default = verse_lib::engine::convert::convert_midi_with_target(
        &midi,
        "french",
        None,
        ExportTarget::Ustx,
    );
    for part in ustx::serialize(ustx_default.svp.as_ref().unwrap())
        .unwrap()
        .voice_parts
    {
        assert_eq!(
            part.notes
                .iter()
                .map(|n| n.lyric.as_str())
                .collect::<Vec<_>>(),
            ["She's,", "Debt,", "un"]
        );
    }
}
