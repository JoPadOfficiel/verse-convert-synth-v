//! Automatic FR/EN/ES/PT routing across the shared projection and output targets.
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

const AUTO: PronunciationProfile = PronunciationProfile::Automatic;

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
            Some(target::diffsinger::FRENCH_NAME),
            Some(target::diffsinger::ENGLISH_NAME),
            Some(target::diffsinger::FRENCH_NAME),
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

#[test]
fn neutral_adapter_chord_member_inherits_sibling_across_gap_through_export() {
    let musicxml = r#"<score-partwise version="4.0">
      <part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list>
      <part id="P1"><measure number="1"><attributes><divisions>480</divisions></attributes>
        <note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><voice>1</voice><staff>1</staff><lyric><text>the</text></lyric></note>
        <note><rest/><duration>480</duration><voice>1</voice><staff>1</staff></note>
        <note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><voice>1</voice><staff>1</staff><lyric><text>hou</text></lyric></note>
        <note><chord/><pitch><step>E</step><octave>4</octave></pitch><duration>480</duration><voice>1</voice><staff>1</staff><lyric><text>hou</text></lyric></note>
      </measure></part></score-partwise>"#;
    let musescore = r#"<museScore version="4.0"><Score><Division>480</Division>
      <Part><Staff id="1"/><trackName>Voice</trackName></Part><Staff id="1"><Measure><voice>
        <TimeSig><sigN>4</sigN><sigD>4</sigD></TimeSig>
        <Chord><durationType>quarter</durationType><Lyrics><text>the</text></Lyrics><Note><pitch>60</pitch><tpc>14</tpc></Note></Chord>
        <Rest><durationType>quarter</durationType></Rest>
        <Chord><durationType>quarter</durationType><Lyrics><text>hou</text></Lyrics><Note><pitch>60</pitch><tpc>14</tpc></Note><Note><pitch>64</pitch><tpc>18</tpc></Note></Chord>
      </voice></Measure></Staff></Score></museScore>"#;
    for (adapter, mut source) in [
        ("MusicXML", musicxml::parse(musicxml.as_bytes()).unwrap()),
        ("MuseScore", musescore::parse(musescore.as_bytes()).unwrap()),
    ] {
        assert!(
            source.tracks.len() >= 2,
            "{adapter} must produce real technical chord-member tracks"
        );
        assert_eq!(source.topology.part_count(), 1);
        assert_eq!(source.topology.staff_count(), 1);
        assert_eq!(source.topology.voice_count(), 1);
        for reverse in [false, true] {
            if reverse {
                source.tracks.reverse();
            }
            let baseline = convert_midi_with_profile(
                &source,
                "english",
                None,
                ExportTarget::Ustx,
                PronunciationProfile::Default,
            );
            assert!(
                baseline.ok,
                "{adapter}, reverse={reverse}: {:?}",
                baseline.msg
            );
            for target in [ExportTarget::Ustx, ExportTarget::Svp] {
                let outcome = convert(&source, target);
                let project = outcome.svp.as_ref().unwrap();
                let mut notes: Vec<_> = project
                    .tracks
                    .iter()
                    .flat_map(|track| &track.notes)
                    .collect();
                notes.sort_by_key(|note| (note.onset_ticks, note.pitch));
                let mut original: Vec<_> = baseline
                    .svp
                    .as_ref()
                    .unwrap()
                    .tracks
                    .iter()
                    .flat_map(|track| &track.notes)
                    .collect();
                original.sort_by_key(|note| (note.onset_ticks, note.pitch));
                assert_eq!(notes.len(), 3, "{adapter}, reverse={reverse}");
                assert_eq!(notes.len(), original.len());
                assert!(
                    notes[0].onset_ticks + notes[0].duration_ticks < notes[1].onset_ticks,
                    "fixture must contain a real performed gap"
                );
                for (note, original) in notes.iter().zip(original) {
                    assert_eq!(
                        note.pronunciation_language,
                        Some(PronunciationLanguage::English),
                        "{adapter}, reverse={reverse}: {:?}",
                        source_lyric(note).map(|lyric| &lyric.raw)
                    );
                    assert_eq!(
                        (
                            note.onset_ticks,
                            note.duration_ticks,
                            note.pitch,
                            &note.source_evidence,
                            &note.performance
                        ),
                        (
                            original.onset_ticks,
                            original.duration_ticks,
                            original.pitch,
                            &original.source_evidence,
                            &original.performance
                        )
                    );
                    assert_eq!(source_lyric(note), source_lyric(original));
                }
                assert!(
                    project.tracks.iter().any(|track| track.notes.len() == 1
                        && track.notes[0].pitch == 64
                        && source_lyric(&track.notes[0]).unwrap().raw == "hou"),
                    "neutral-only technical lane survives conversion"
                );
                if target == ExportTarget::Ustx {
                    let exported = ustx::serialize(project).unwrap();
                    let neutral = exported
                        .voice_parts
                        .iter()
                        .flat_map(|part| &part.notes)
                        .find(|note| note.tone == 64)
                        .unwrap();
                    assert_eq!(
                        neutral.phonemizer.as_deref(),
                        Some(target::diffsinger::ENGLISH_NAME)
                    );
                    assert!(
                        String::from_utf8(target::serialize_to(target, project).unwrap())
                            .unwrap()
                            .contains(target::diffsinger::ENGLISH_NAME)
                    );
                } else {
                    assert!(
                        !String::from_utf8(target::serialize_to(target, project).unwrap())
                            .unwrap()
                            .contains("phonemizer")
                    );
                }
            }
        }
    }
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
fn four_language_routing_preserves_source_notes_across_all_adapters() {
    let words = ["bonjour", "beautiful", "hola", "obrigado", "merci"];
    let expected = [
        PronunciationLanguage::French,
        PronunciationLanguage::English,
        PronunciationLanguage::Spanish,
        PronunciationLanguage::Portuguese,
        PronunciationLanguage::French,
    ];
    let phonemizers = [
        target::diffsinger::FRENCH_NAME,
        target::diffsinger::ENGLISH_NAME,
        target::diffsinger::SPANISH_NAME,
        target::diffsinger::PORTUGUESE_NAME,
        target::diffsinger::FRENCH_NAME,
    ];
    let sources = [
        midi::parse(&midi_score(&words, false)).unwrap(),
        midi::parse_with_karaoke_profile(&midi_score(&words, true)).unwrap(),
        musicxml::parse(musicxml_score(&words).as_bytes()).unwrap(),
        musescore::parse(musescore_score(&words).as_bytes()).unwrap(),
    ];
    for (source_index, source) in sources.iter().enumerate() {
        let baseline = convert_midi_with_profile(
            source,
            "english",
            None,
            ExportTarget::Ustx,
            PronunciationProfile::Default,
        );
        assert!(baseline.ok);
        let automatic = convert(source, ExportTarget::Ustx);
        let project = automatic.svp.as_ref().unwrap();
        assert_eq!(head_languages(project), expected);
        let identities = |project: &ProjectedProject| {
            project
                .tracks
                .iter()
                .flat_map(|track| &track.notes)
                .map(|note| {
                    (
                        note.onset_ticks,
                        note.duration_ticks,
                        note.pitch,
                        note.source_evidence.clone(),
                        note.performance.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            identities(project),
            identities(baseline.svp.as_ref().unwrap())
        );
        let native = ustx::serialize(project).unwrap();
        assert_eq!(native.voice_parts.len(), 1);
        assert_eq!(
            native.voice_parts[0]
                .notes
                .iter()
                .map(|note| note.phonemizer.as_deref())
                .collect::<Vec<_>>(),
            phonemizers.map(Some)
        );
        assert_eq!(native.voice_parts[0].notes[2].lyric, "hola[o l a]");
        assert_eq!(native.voice_parts[0].notes[3].lyric, "obrigado");
        if source_index == 0 {
            if let Some(path) = std::env::var_os("VERSE_OPENUTAU_PRONUNCIATION_FIXTURE") {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .unwrap();
                file.write_all(ustx::to_yaml(&native).as_bytes()).unwrap();
            }
        }
        let svp = convert(source, ExportTarget::Svp);
        assert_eq!(head_languages(svp.svp.as_ref().unwrap()), expected);
        let bytes = target::serialize_to(ExportTarget::Svp, svp.svp.as_ref().unwrap()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        for forbidden in ["phonemizer", "fr/", "en/", "es/", "pt/"] {
            assert!(!text.contains(forbidden), "SVP contains {forbidden}");
        }
    }
}

#[test]
fn external_spanish_portuguese_split_words_preserve_lyrics_holds_and_manual_hints() {
    for (syllables, word, language, profile, phonemizer) in [
        (
            ["Can", "ción!"],
            "Canción!",
            PronunciationLanguage::Spanish,
            PronunciationProfile::SpanishDiffSinger,
            target::diffsinger::SPANISH_PHONEMIZER,
        ),
        (
            ["Cora", "ção!"],
            "Coração!",
            PronunciationLanguage::Portuguese,
            PronunciationProfile::PortugueseDiffSinger,
            target::diffsinger::PORTUGUESE_PHONEMIZER,
        ),
    ] {
        let xml = musicxml_score(&syllables)
            .replacen("<syllabic>single</syllabic>", "<syllabic>begin</syllabic>", 1)
            .replacen("<syllabic>single</syllabic>", "<syllabic>end</syllabic><extend type=\"start\"/>", 1)
            .replace("</measure>", "<note><pitch><step>D</step><octave>4</octave></pitch><duration>1</duration><lyric><extend type=\"stop\"/></lyric></note><note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration><lyric><text>manual[custom]</text></lyric></note></measure>");
        let source = musicxml::parse(xml.as_bytes()).unwrap();
        for selected in [AUTO, profile] {
            let outcome =
                convert_midi_with_profile(&source, "french", None, ExportTarget::Ustx, selected);
            assert!(outcome.ok, "{:?}", outcome.msg);
            let project = outcome.svp.as_ref().unwrap();
            let notes = &project.tracks[0].notes;
            assert_eq!(notes.len(), 4);
            for (note, raw) in notes.iter().zip(syllables) {
                assert_eq!(source_lyric(note).unwrap().raw, raw);
            }
            if selected == AUTO {
                assert!(notes[..3]
                    .iter()
                    .all(|note| note.pronunciation_language == Some(language)));
                assert_eq!(notes[3].pronunciation_language, None);
            }
            let native = ustx::serialize(project).unwrap();
            assert_eq!(native.tracks[0].phonemizer, phonemizer);
            let sung = &native.voice_parts[0].notes;
            assert_eq!(
                sung.iter()
                    .map(|note| note.lyric.as_str())
                    .collect::<Vec<_>>(),
                [word, "+", "+~", "manual[custom]"]
            );
            if selected == AUTO {
                let name = if language == PronunciationLanguage::Spanish {
                    target::diffsinger::SPANISH_NAME
                } else {
                    target::diffsinger::PORTUGUESE_NAME
                };
                assert_eq!(sung[0].phonemizer.as_deref(), Some(name));
                assert!(sung[1..].iter().all(|note| note.phonemizer.is_none()));
            }
        }
    }
}

#[test]
fn external_exact_split_words_emit_mfa_hints() {
    assert_exact_split_word_layout(false);
}

#[test]
fn external_exact_split_words_reject_mismatched_syllable_attacks() {
    assert_exact_split_word_layout(true);
}

fn assert_exact_split_word_layout(mismatch: bool) {
    for (syllables, mismatched, raw_word, hint, language, profile, phonemizer, name) in [
        (
            ["Ho", "la!"],
            ["H", "o", "la!"],
            "Hola!",
            "hola[o l a]",
            PronunciationLanguage::Spanish,
            PronunciationProfile::SpanishDiffSinger,
            target::diffsinger::SPANISH_PHONEMIZER,
            target::diffsinger::SPANISH_NAME,
        ),
        (
            ["A", "cho"],
            ["A", "c", "ho"],
            "Acho",
            "acho[a S u]",
            PronunciationLanguage::Portuguese,
            PronunciationProfile::PortugueseDiffSinger,
            target::diffsinger::PORTUGUESE_PHONEMIZER,
            target::diffsinger::PORTUGUESE_NAME,
        ),
    ] {
        let fragments = if mismatch {
            &mismatched[..]
        } else {
            &syllables[..]
        };
        // A Portuguese anchor supplies passage ownership for shared spelling `acho`.
        let mut words = fragments.to_vec();
        words.push(if language == PronunciationLanguage::Portuguese {
            "obrigado"
        } else {
            "hola"
        });
        let mut xml = musicxml_score(&words);
        for index in 0..fragments.len() {
            let state = if index == 0 {
                "begin"
            } else if index + 1 == fragments.len() {
                "end"
            } else {
                "middle"
            };
            xml = xml.replacen(
                "<syllabic>single</syllabic>",
                &format!("<syllabic>{state}</syllabic>"),
                1,
            );
        }
        let source = musicxml::parse(xml.as_bytes()).unwrap();
        let baseline = convert_midi_with_profile(
            &source,
            "english",
            None,
            ExportTarget::Ustx,
            PronunciationProfile::Default,
        );
        assert!(baseline.ok);
        for selected in [AUTO, profile] {
            let outcome =
                convert_midi_with_profile(&source, "english", None, ExportTarget::Ustx, selected);
            assert!(outcome.ok, "{selected:?}: {:?}", outcome.msg);
            let project = outcome.svp.as_ref().unwrap();
            assert_eq!(project.tracks[0].notes.len(), words.len());
            let notes = &project.tracks[0].notes[..fragments.len()];
            for ((note, original), raw) in notes
                .iter()
                .zip(&baseline.svp.as_ref().unwrap().tracks[0].notes)
                .zip(fragments)
            {
                let evidence = source_lyric(note).unwrap();
                let original_lyric = source
                    .tracks
                    .iter()
                    .flat_map(|track| &track.events)
                    .filter_map(|event| match &event.kind {
                        midi::Kind::NoteOn(note) => Some(&note.lyrics),
                        _ => None,
                    })
                    .flatten()
                    .find(|lyric| lyric.id == evidence.id)
                    .unwrap();
                assert_eq!(evidence, original_lyric);
                assert_eq!(source_lyric(note).unwrap().raw, *raw);
                assert_eq!(
                    (note.onset_ticks, note.duration_ticks, note.pitch),
                    (
                        original.onset_ticks,
                        original.duration_ticks,
                        original.pitch
                    )
                );
                assert_eq!(note.source_evidence, original.source_evidence);
                assert_eq!(note.performance, original.performance);
                if selected == AUTO {
                    assert_eq!(note.pronunciation_language, Some(language));
                }
            }
            let diagnostics: Vec<_> = outcome
                .tracks
                .iter()
                .flat_map(|track| &track.warnings)
                .collect();
            let expected_code = if mismatch {
                target::diffsinger::SYLLABLE_MISMATCH
            } else {
                target::diffsinger::APPLIED
            };
            let diagnostic = diagnostics
                .iter()
                .find(|d| d.code == expected_code)
                .expect("word-head pronunciation diagnostic");
            assert!(diagnostic.source_id.is_some());
            let forbidden = if mismatch {
                target::diffsinger::APPLIED
            } else {
                target::diffsinger::SYLLABLE_MISMATCH
            };
            assert!(!diagnostics
                .iter()
                .any(|d| d.code == forbidden && d.source_id == diagnostic.source_id));
            let native = ustx::serialize(project).unwrap();
            assert_eq!(native.tracks[0].phonemizer, phonemizer);
            assert_eq!(native.voice_parts[0].notes.len(), words.len());
            let sung = &native.voice_parts[0].notes[..fragments.len()];
            assert_eq!(sung[0].lyric, if mismatch { raw_word } else { hint });
            assert!(sung[1..]
                .iter()
                .all(|note| note.lyric == "+" && note.phonemizer.is_none()));
            assert_eq!(
                sung[0].phonemizer.as_deref(),
                if selected == AUTO { Some(name) } else { None }
            );
            for (index, note) in sung.iter().enumerate() {
                assert_eq!(
                    (note.position, note.duration, note.tone),
                    (index as i32 * 480, 480, 60)
                );
            }
        }
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
