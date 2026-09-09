//! Source-owned sung attacks, pronunciation policy, and musical invariants.
use verse_lib::engine::convert::{
    convert_midi_with_profile, convert_midi_with_target, ConvertOutcome,
};
use verse_lib::engine::midi::{Kind, Midi};
use verse_lib::engine::projection::ProjectedLyric;
use verse_lib::engine::target::{self, french, ustx, ExportTarget, PronunciationProfile};
use verse_lib::engine::{musescore, musicxml};

const FR: PronunciationProfile = PronunciationProfile::FrenchMillefeuille;

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
            if let ProjectedLyric::Pronounced { source, .. } = &note.lyric {
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
                &["court[fr/k fr/ou fr/r]", "ou", "ourt"],
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
                "J'i",
                "rai",
                "la",
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
    let midi = sab(&["u", "ure,", "urs,", "la", "J'i", "rai"], false, false);
    assert_lanes(&midi, &["u", "ure,", "urs,", "la", "J'i", "rai"]);
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
        "hommes",
        "tout[fr/t fr/ou]",
        "au[fr/oh]",
        "mes[fr/m fr/eh]",
        "haricots",
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
            "rêves(2)",
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
        9
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
