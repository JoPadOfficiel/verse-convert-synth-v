//! Automatic FR/EN routing across the shared projection and output targets.
//! Synthetic fixtures are authored here; private acceptance masters stay outside Git.

use sha2::{Digest, Sha256};
use std::path::Path;
use verse_lib::engine::convert::{convert_midi_with_profile, ConvertOutcome};
use verse_lib::engine::midi::{self, Midi};
use verse_lib::engine::projection::{
    ProjectedLyric, ProjectedNote, ProjectedProject, PronunciationLanguage,
};
use verse_lib::engine::target::{self, ustx, ExportTarget, PronunciationProfile};
use verse_lib::engine::{musescore, musicxml};

const AUTO: PronunciationProfile = PronunciationProfile::AutomaticFrenchEnglish;

fn convert(midi: &Midi, target: ExportTarget) -> ConvertOutcome {
    let result = convert_midi_with_profile(midi, "english", None, target, AUTO);
    assert!(result.ok, "automatic conversion failed: {:?}", result.msg);
    result
}

fn head_languages(project: &ProjectedProject) -> Vec<PronunciationLanguage> {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.notes)
        .filter(|note| !note.lyric.continues_previous_note())
        .filter_map(|note| note.pronunciation_language)
        .collect()
}

fn assert_french_english_french(midi: &Midi) {
    let default = convert_midi_with_profile(
        midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Default,
    );
    assert!(default.ok, "default conversion failed: {:?}", default.msg);

    let ustx_outcome = convert(midi, ExportTarget::Ustx);
    let svp_outcome = convert(midi, ExportTarget::Svp);
    let ustx_project = ustx_outcome.svp.as_ref().unwrap();
    let svp_project = svp_outcome.svp.as_ref().unwrap();
    assert_eq!(
        head_languages(ustx_project),
        [
            PronunciationLanguage::French,
            PronunciationLanguage::English,
            PronunciationLanguage::French,
        ]
    );
    assert_eq!(
        head_languages(svp_project),
        head_languages(ustx_project),
        "analysis ownership is target-neutral"
    );

    let geometry = |project: &ProjectedProject| {
        project
            .tracks
            .iter()
            .flat_map(|track| &track.notes)
            .map(|note| (note.onset_ticks, note.duration_ticks, note.pitch))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        geometry(ustx_project),
        geometry(default.svp.as_ref().unwrap()),
        "automatic routing cannot duplicate, drop or retime notes"
    );

    let openutau = ustx::serialize(ustx_project).unwrap();
    assert_eq!(
        openutau.tracks[0].phonemizer,
        target::french::PHONEMIZER,
        "an automatic mixed track keeps a deterministic first-language compatibility fallback"
    );
    assert_eq!(
        openutau.voice_parts.len(),
        1,
        "language switches stay in one lane"
    );
    assert_eq!(
        openutau.voice_parts[0]
            .notes
            .iter()
            .map(|note| note.phonemizer.as_deref())
            .collect::<Vec<_>>(),
        [
            Some(target::french::PHONEMIZER),
            Some(target::english::PHONEMIZER),
            Some(target::french::PHONEMIZER),
        ],
        "each complete word head carries its selected OpenUtau phonemizer"
    );

    let svp = String::from_utf8(target::serialize_to(ExportTarget::Svp, svp_project).unwrap())
        .expect("SVP is UTF-8 JSON");
    assert!(!svp.contains("phonemizer"));
    assert!(!svp.contains("fr/"));
    assert!(!svp.contains("en/"));
}

fn musicxml_score(words: &[&str]) -> String {
    let notes: String = words
        .iter()
        .map(|word| {
            format!(
                "<note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration>\
                 <type>quarter</type><lyric><syllabic>single</syllabic><text>{word}</text></lyric></note>"
            )
        })
        .collect();
    format!(
        "<score-partwise version=\"4.0\"><part-list><score-part id=\"P1\">\
         <part-name>Voice</part-name></score-part></part-list><part id=\"P1\">\
         <measure number=\"1\"><attributes><divisions>1</divisions><time><beats>3</beats>\
         <beat-type>4</beat-type></time></attributes>{notes}</measure></part></score-partwise>"
    )
}

fn musicxml_split_word() -> String {
    let syllables = [("beau", "begin"), ("ti", "middle"), ("ful", "end")];
    let notes: String = syllables
        .iter()
        .map(|(text, syllabic)| {
            format!(
                "<note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration>\
                 <type>quarter</type><lyric><syllabic>{syllabic}</syllabic><text>{text}</text></lyric></note>"
            )
        })
        .collect();
    format!(
        "<score-partwise version=\"4.0\"><part-list><score-part id=\"P1\">\
         <part-name>Voice</part-name></score-part></part-list><part id=\"P1\">\
         <measure number=\"1\"><attributes><divisions>1</divisions><time><beats>3</beats>\
         <beat-type>4</beat-type></time></attributes>{notes}</measure></part></score-partwise>"
    )
}

fn musescore_score(words: &[&str]) -> String {
    let notes: String = words
        .iter()
        .enumerate()
        .map(|(index, word)| {
            format!(
                "<Chord><durationType>quarter</durationType><Lyrics><syllabic>single</syllabic>\
                 <text>{word}</text></Lyrics><Note><pitch>{}</pitch><tpc>14</tpc></Note></Chord>",
                60 + index
            )
        })
        .collect();
    format!(
        "<museScore version=\"4.0\"><Score><Division>480</Division><Part><Staff id=\"1\"/>\
         <trackName>Voice</trackName></Part><Staff id=\"1\"><Measure><voice><TimeSig>\
         <sigN>3</sigN><sigD>4</sigD></TimeSig>{notes}</voice></Measure></Staff></Score></museScore>"
    )
}

fn push_vlq(out: &mut Vec<u8>, mut value: u32) {
    let mut bytes = [0u8; 5];
    let mut len = 0usize;
    loop {
        bytes[len] = (value & 0x7f) as u8;
        len += 1;
        value >>= 7;
        if value == 0 {
            break;
        }
    }
    for index in (0..len).rev() {
        let mut byte = bytes[index];
        if index != 0 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn meta(out: &mut Vec<u8>, delta: u32, kind: u8, payload: &[u8]) {
    push_vlq(out, delta);
    out.extend_from_slice(&[0xff, kind, payload.len() as u8]);
    out.extend_from_slice(payload);
}

fn midi_score(words: &[&str], karaoke: bool) -> Vec<u8> {
    let mut track = Vec::new();
    if karaoke {
        meta(&mut track, 0, 0x01, b"@KMIDI KARAOKE FILE");
    }
    for (index, word) in words.iter().enumerate() {
        meta(&mut track, 0, 0x05, word.as_bytes());
        track.extend_from_slice(&[0x00, 0x90, 60 + index as u8, 100]);
        push_vlq(&mut track, 480);
        track.extend_from_slice(&[0x80, 60 + index as u8, 0]);
    }
    meta(&mut track, 0, 0x2f, &[]);
    let mut file = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
    file.extend_from_slice(&(track.len() as u32).to_be_bytes());
    file.extend_from_slice(&track);
    file
}

#[test]
fn automatic_routing_agrees_across_midi_karaoke_musicxml_and_musescore() {
    let words = ["bonjour", "beautiful", "merci"];
    let sources = [
        midi::parse(&midi_score(&words, false)).unwrap(),
        midi::parse_with_karaoke_profile(&midi_score(&words, true)).unwrap(),
        musicxml::parse(musicxml_score(&words).as_bytes()).unwrap(),
        musescore::parse(musescore_score(&words).as_bytes()).unwrap(),
    ];
    for source in &sources {
        assert_french_english_french(source);
    }
}

#[test]
fn french_liaison_cannot_cross_into_an_english_word() {
    let midi = musicxml::parse(musicxml_score(&["les", "always", "merci"]).as_bytes()).unwrap();
    let outcome = convert(&midi, ExportTarget::Ustx);
    let project = outcome.svp.as_ref().unwrap();
    assert_eq!(
        head_languages(project),
        [
            PronunciationLanguage::French,
            PronunciationLanguage::English,
            PronunciationLanguage::French,
        ]
    );
    let first = &project.tracks[0].notes[0].lyric;
    let verse_lib::engine::projection::ProjectedLyric::Pronounced { phonemes, .. } = first else {
        panic!("French head should use the existing Millefeuille pronunciation path")
    };
    assert!(!phonemes.split_whitespace().any(|phone| phone == "fr/z"));
}

#[test]
fn automatic_svp_keeps_default_split_word_serialization() {
    let midi = musicxml::parse(musicxml_split_word().as_bytes()).unwrap();
    let default = convert_midi_with_profile(
        &midi,
        "english",
        None,
        ExportTarget::Svp,
        PronunciationProfile::Default,
    );
    let automatic = convert(&midi, ExportTarget::Svp);
    assert!(default.ok && automatic.ok);
    let automatic_project = automatic.svp.as_ref().unwrap();
    assert!(automatic_project.tracks[0]
        .notes
        .iter()
        .all(|note| note.pronunciation_language == Some(PronunciationLanguage::English)));
    assert_eq!(
        target::serialize_to(ExportTarget::Svp, automatic_project).unwrap(),
        target::serialize_to(ExportTarget::Svp, default.svp.as_ref().unwrap()).unwrap(),
        "automatic ownership analysis must not change Synthesizer V lyric joining or phoneme serialization"
    );
}

fn source_lyric(note: &ProjectedNote) -> Option<&verse_lib::engine::midi::Lyric> {
    match &note.lyric {
        ProjectedLyric::Source(source)
        | ProjectedLyric::Pronounced { source, .. }
        | ProjectedLyric::PronouncedSplit { source } => Some(source),
        ProjectedLyric::Extension | ProjectedLyric::Absent => None,
    }
}

fn rockollection_oracle(note: &ProjectedNote) -> PronunciationLanguage {
    let source = source_lyric(note).expect("private oracle word carries its source lyric");
    let origin = note
        .source_evidence
        .as_ref()
        .and_then(|evidence| evidence.origin.as_ref())
        .expect("MuseScore private oracle word carries source ownership");
    let measure = origin
        .source
        .measure
        .expect("MuseScore private oracle word carries a measure");
    let staff = origin
        .source
        .staff_id
        .as_deref()
        .expect("Rockollection private oracle word carries a staff");
    let raw = source.raw.trim().to_lowercase();

    // The ranges and embedded names below are an editorial transcription of
    // the supplied master. They are intentionally independent of Verse's
    // output so the gate can catch a systematic router regression.
    let english = match staff {
        "1" => {
            matches!(measure, 25..=30 | 47..=55 | 72..=75 | 93 | 95 | 97 | 99 | 119..=122)
                || match measure {
                    36 => raw.starts_with("ex"),
                    37 => matches!(raw.as_str(), "me" | "sir" | "big"),
                    38 => raw == "ben",
                    40 => raw.starts_with("lon") || raw.starts_with("beat"),
                    42 => raw.starts_with("beat"),
                    46 => matches!(raw.as_str(), "it's" | "been" | "a"),
                    60 => raw.starts_with("bet"),
                    67 => raw == "beach",
                    68 => raw == "boys",
                    105 => raw.starts_with("jim"),
                    108 => raw.starts_with("jim"),
                    110 => raw == "stones",
                    _ => false,
                }
        }
        "2" => {
            matches!(measure, 23..=30 | 47..=55 | 71..=75 | 93..=99 | 121..=122)
                || match measure {
                    34 => raw.starts_with("li"),
                    41 | 43 => raw.starts_with("beat"),
                    46 => matches!(raw.as_str(), "it's" | "been" | "a"),
                    66 => raw == "beach",
                    67 => raw == "boys",
                    _ => false,
                }
        }
        "3" => {
            matches!(measure, 25..=30 | 47..=55 | 72..=75 | 94..=100 | 118 | 120)
                || match measure {
                    41 | 43 => raw.starts_with("beat"),
                    66 => raw == "beach",
                    67 => raw == "boys",
                    117 => matches!(raw.as_str(), "i" | "can't"),
                    _ => false,
                }
        }
        other => panic!("unexpected Rockollection staff {other}"),
    };
    if english {
        PronunciationLanguage::English
    } else {
        PronunciationLanguage::French
    }
}

fn assert_private_oracle(filename: &str, project: &ProjectedProject) {
    let mut checked = 0usize;
    let mut mismatches = Vec::new();
    for track in &project.tracks {
        for note in &track.notes {
            if note.lyric.continues_previous_note() {
                continue;
            }
            let Some(source) = source_lyric(note) else {
                continue;
            };
            // Punctuation-only source records are not sung words.
            if !source.raw.chars().any(char::is_alphabetic) {
                continue;
            }
            checked += 1;
            let expected = if filename.starts_with("White Christmas") {
                match source.verse {
                    1 => PronunciationLanguage::English,
                    2 => PronunciationLanguage::French,
                    verse => panic!("unexpected White Christmas lyric row {verse}"),
                }
            } else {
                rockollection_oracle(note)
            };
            if note.pronunciation_language != Some(expected) {
                let measure = note
                    .source_evidence
                    .as_ref()
                    .and_then(|evidence| evidence.origin.as_ref())
                    .and_then(|origin| origin.source.measure);
                let staff = note
                    .source_evidence
                    .as_ref()
                    .and_then(|evidence| evidence.origin.as_ref())
                    .and_then(|origin| origin.source.staff_id.as_deref());
                mismatches.push(format!(
                    "{} staff={staff:?} measure={measure:?} lyric={:?} syllabic={:?} expected={expected:?} actual={:?}",
                    track.name, source.raw, source.syllabic, note.pronunciation_language
                ));
            }
        }
    }
    assert!(checked > 0, "private language oracle checked no sung words");
    assert!(
        mismatches.is_empty(),
        "{filename} language oracle mismatch(es), checked {checked} sung words:\n{}",
        mismatches.join("\n")
    );
}

/// The copyrighted masters remain outside Git. Explicitly requesting this gate
/// makes both exact files mandatory and compares every sung word-head against a
/// human-readable source-derived language oracle. Continuation/geometry
/// conservation is covered by synthetic tests.
#[test]
#[ignore = "requires VERSE_MIXED_LANGUAGE_CORPUS_DIR containing both supplied private MSCZ masters"]
fn supplied_private_scores_match_the_frozen_automatic_language_oracle() {
    let directory = std::env::var("VERSE_MIXED_LANGUAGE_CORPUS_DIR")
        .expect("VERSE_MIXED_LANGUAGE_CORPUS_DIR is required for the private mixed-language gate");
    for (filename, source_sha256) in [
        (
            "White Christmas-SATB Pno TAB Do.mscz",
            "66ad238c35ed838e95b9eebb7bc421168539a600eaab055b69b234663d867fff",
        ),
        (
            "Rockollection-L_Voulzy CHORAL MIm.mscz",
            "186f0c1f91a138aef4317fe9f3dfe1473e5773479a36657bf9b637d2be8ad6fb",
        ),
    ] {
        let path = Path::new(&directory).join(filename);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("required private fixture {}: {error}", path.display()));
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), source_sha256);
        let midi = musescore::parse(&bytes).expect("private MuseScore master parses");
        let outcome = convert(&midi, ExportTarget::Ustx);
        assert_private_oracle(filename, outcome.svp.as_ref().unwrap());
    }
}
