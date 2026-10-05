//! Nasal repairs must preserve source attacks, spelling and ownership.
use std::io::{Cursor, Write};
use verse_lib::engine::{
    convert::convert_midi_with_profile,
    midi::{self, Lyric, LyricState, Midi, Syllabic},
    musescore, musicxml,
    projection::{
        NoteOrigin, ProjectedLyric, ProjectedNote, ProjectedProject, PronunciationLanguage,
    },
    target::{self, french, ustx, ExportTarget, PronunciationProfile},
};

use ExportTarget::{Svp, Ustx};
use PronunciationProfile::{Automatic, Default, FrenchMillefeuille};

const NASAL: &str = "fr/b fr/l fr/en";

fn source(note: &ProjectedNote) -> &Lyric {
    match &note.lyric {
        ProjectedLyric::Source(s)
        | ProjectedLyric::Pronounced { source: s, .. }
        | ProjectedLyric::PronouncedSplit { source: s } => s,
        _ => panic!("expected original lyric"),
    }
}

fn phones(note: &ProjectedNote) -> Option<&str> {
    match &note.lyric {
        ProjectedLyric::Pronounced { phonemes, .. } => Some(phonemes),
        _ => None,
    }
}

fn notes(words: &[(&str, Syllabic)]) -> Vec<ProjectedNote> {
    let ir = musescore::parse(score(Some("4.0")).as_bytes()).unwrap();
    let mut project = convert(&ir, Ustx, Default);
    let mut ns = project.tracks.remove(0).notes;
    ns.truncate(words.len());
    for (note, (word, syllabic)) in ns.iter_mut().zip(words) {
        let mut lyric = Lyric::text(source(note).id.clone(), (*word).into());
        lyric.syllabic = Some(syllabic.clone());
        note.lyric = ProjectedLyric::Source(Box::new(lyric));
    }
    ns
}

fn origin(note: &mut ProjectedNote) -> &mut NoteOrigin {
    note.source_evidence
        .as_mut()
        .unwrap()
        .origin
        .as_mut()
        .unwrap()
}

fn lyric(note: &mut ProjectedNote) -> &mut Lyric {
    let ProjectedLyric::Source(s) = &mut note.lyric else {
        panic!("expected source")
    };
    s
}

fn apply(notes: &mut [ProjectedNote]) -> Vec<String> {
    let before = notes.to_vec();
    let ids = notes
        .iter()
        .map(|n| n.source_evidence.as_ref().unwrap().note_id.clone())
        .collect::<Vec<_>>();
    let diagnostics = french::apply(notes, &ids);
    for (actual, original) in notes.iter().zip(&before) {
        let mut restored = actual.clone();
        restored.lyric = ProjectedLyric::Source(Box::new(source(actual).clone()));
        assert_eq!(&restored, original, "only pronunciation may change");
    }
    diagnostics.into_iter().map(|d| d.code).collect()
}

#[test]
fn bilateral_nasal_echoes_keep_two_attacks_and_reject_boundaries() {
    for spelling in ["blanc", "blan"] {
        for boundary in "none single middle unmarked row verse part staff voice occurrence segment gap overlap manual english conflict".split_whitespace() {
            let mut ns = notes(&[(spelling, Syllabic::Begin), ("an", Syllabic::End)]);
            let tail = lyric(&mut ns[1]);
            match boundary {
                "single" => tail.syllabic = Some(Syllabic::Single),
                "middle" => tail.syllabic = Some(Syllabic::Middle),
                "unmarked" => tail.syllabic = None,
                "row" => tail.lane = "other".into(),
                "verse" => tail.verse = 2,
                "manual" => tail.state = LyricState::Text("an[en/ae en/n]".into()),
                _ => {}
            }
            match boundary {
                "part" => origin(&mut ns[1]).source.part_id = Some("P2".into()),
                "staff" => origin(&mut ns[1]).source.staff_id = Some("2".into()),
                "voice" => origin(&mut ns[1]).source.voice = Some("2".into()),
                "occurrence" => origin(&mut ns[1]).source.occurrence = 2,
                "conflict" => origin(&mut ns[1]).lyric_conflict = true,
                "segment" => std::sync::Arc::make_mut(origin(&mut ns[1]).source.continuity.as_mut().unwrap()).playback_segment += 1,
                "gap" => ns[1].onset_ticks += 1,
                "overlap" => ns[1].onset_ticks -= 1,
                "english" => ns[0].pronunciation_language = Some(PronunciationLanguage::English),
                _ => {}
            }
            let diagnostics = apply(&mut ns);
            if boundary == "none" {
                assert_eq!(phones(&ns[0]), Some(NASAL));
                assert_eq!(phones(&ns[1]), Some("fr/en"));
                assert_eq!(diagnostics, [french::APPLIED, french::APPLIED]);
            } else {
                assert_ne!(phones(&ns[0]), Some(NASAL), "{spelling}: {boundary}");
            }
        }
    }
}

#[test]
fn standalone_blan_needs_french_context_and_same_source_attestation() {
    for case in
        "french english unknown no-attestation part verse gap orphan manual fragment donor-fragment donor-dash punctuation prefix".split_whitespace()
    {
        let neighbor = match case {
            "english" => "beautiful",
            "unknown" => "zyx",
            _ => "neige",
        };
        let mut ns = notes(&[
            ("blanc", Syllabic::Single),
            (neighbor, Syllabic::Single),
            ("Blan", Syllabic::Single),
        ]);
        match case {
            "no-attestation" => {
                ns.remove(0);
            }
            "part" => origin(&mut ns[0]).source.part_id = Some("other".into()),
            "verse" => lyric(&mut ns[0]).verse = 2,
            "orphan" => lyric(&mut ns[2]).syllabic = Some(Syllabic::End),
            "manual" => {
                lyric(&mut ns[2]).state = LyricState::Text("Blan[en/b en/l en/ae en/n]".into())
            }
            "fragment" | "donor-fragment" => {
                let index = if case == "fragment" { 2 } else { 0 };
                let s = lyric(&mut ns[index]);
                s.state = LyricState::Text(format!("{}-", s.raw));
                s.syllabic = None;
            }
            "punctuation" => lyric(&mut ns[1]).state = LyricState::Text("neige.".into()),
            "donor-dash" => {
                lyric(&mut ns[1]).state = LyricState::Text("neige-".into());
                lyric(&mut ns[1]).syllabic = None;
            }
            "prefix" => for n in &mut ns[..2] {
                let s = lyric(n); s.verse_from_score = true;
                s.state = LyricState::Text(format!("1.{}", s.raw));
            },
            "gap" => ns[2].onset_ticks += 1,
            _ => {}
        }
        let diagnostics = apply(&mut ns);
        let last = ns.last().unwrap();
        if matches!(case, "french" | "prefix") {
            assert_eq!(phones(last), Some(NASAL));
            let ProjectedLyric::Pronounced { text, .. } = &last.lyric else {
                unreachable!()
            };
            assert_eq!(text, "Blan", "display spelling stays source-owned");
        } else {
            assert_eq!(phones(last), None, "{case}");
            if case != "manual" {
                assert!(diagnostics.iter().any(|d| d == french::UNSUPPORTED));
            }
        }
    }
    for neighbor in ["beautiful", "zyx"] {
        let mut ns = notes(&[(neighbor, Syllabic::Single), ("Blan", Syllabic::Single)]);
        let ids: Vec<_> = ns.iter().map(|n| source(n).id.clone()).collect();
        let route = verse_lib::engine::language::route(&ns, &ids);
        let track = verse_lib::engine::projection::ProjectedTrack {
            name: "Unattested token".into(),
            source_track_id: "unattested".into(),
            muted: false,
            notes: ns.clone(),
        };
        assert!(verse_lib::engine::language::complete_words(&track)
            .iter()
            .all(|word| word.key != "blanc"));
        for (note, language) in ns.iter_mut().zip(route.languages) {
            note.pronunciation_language = language;
        }
        apply(&mut ns);
        assert_eq!(
            phones(&ns[1]),
            None,
            "Automatic must not invent a French reading without French context"
        );
    }
}

#[test]
fn orphan_begin_never_becomes_a_complete_blanc_or_receives_its_hint() {
    let mut ns = notes(&[
        ("NO", Syllabic::Begin),
        ("EL", Syllabic::End),
        ("blan", Syllabic::Begin),
    ]);
    let original = ns[2].clone();
    let track = verse_lib::engine::projection::ProjectedTrack {
        name: "Bass".into(),
        source_track_id: "orphan".into(),
        muted: false,
        notes: ns.clone(),
    };
    let words = verse_lib::engine::language::complete_words(&track);
    assert!(words.iter().all(|word| word.key != "blanc"));
    // Snapshot lookup consumes these complete-word identities. There must be
    // no candidate for this head, even for a one-attack context correction.
    assert!(words.iter().all(|word| !word.matches_head(&ns[2])));
    let diagnostics = apply(&mut ns);
    assert_eq!(ns[2], original);
    assert_eq!(phones(&ns[2]), None);
    assert!(diagnostics.iter().any(|code| code == french::UNSUPPORTED));
}

#[test]
#[ignore = "requires the hash-pinned private OPEN fixture via VERSE_BLANC_SOURCE"]
fn private_open_bass_nasal_layout_keeps_its_attacks_and_french_owner() {
    use sha2::{Digest, Sha256};
    let path = std::env::var_os("VERSE_BLANC_SOURCE").expect("VERSE_BLANC_SOURCE is required");
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        "c688a0d62fd63c0157f2bc84266328da24355ee7b49d7291e930086f743f4b48"
    );
    let ir = musescore::parse(&bytes).unwrap();
    let project = convert(&ir, Ustx, Automatic);
    let pair: Vec<_> = project
        .tracks
        .iter()
        .flat_map(|t| &t.notes)
        .filter(|n| {
            n.source_evidence
                .as_ref()
                .and_then(|e| e.origin.as_ref())
                .is_some_and(|o| {
                    o.source.staff_id.as_deref() == Some("4")
                        && o.source.occurrence == 1
                        && o.source.measure == Some(36)
                        && matches!(source(n).raw.as_str(), "blan" | "an")
                })
        })
        .collect();
    assert_eq!(pair.len(), 2);
    for (note, (raw, binding, hint, onset, pitch)) in pair.iter().zip([
        ("blan", Syllabic::Begin, NASAL, 131520, 55),
        ("an", Syllabic::End, "fr/en", 132000, 53),
    ]) {
        assert_eq!(source(note).raw, raw);
        assert_eq!(source(note).syllabic, Some(binding));
        assert_eq!(source(note).verse, 2);
        assert_eq!(
            (note.onset_ticks, note.duration_ticks, note.pitch),
            (onset, 480, pitch)
        );
        assert_eq!(phones(note), Some(hint));
        assert_eq!(
            note.pronunciation_language,
            Some(PronunciationLanguage::French)
        );
    }
    let native = ustx::serialize(&project).unwrap();
    let head = native.voice_parts[3]
        .notes
        .iter()
        .find(|n| n.position == 131520)
        .unwrap();
    assert_eq!(head.lyric, "blan[fr/b fr/l fr/en]");
    assert_eq!(
        head.phonemizer.as_deref(),
        Some(target::diffsinger::FRENCH_NAME)
    );
}

fn convert(source: &Midi, target: ExportTarget, profile: PronunciationProfile) -> ProjectedProject {
    let out = convert_midi_with_profile(source, "english", None, target, profile);
    assert!(out.ok, "{:?}", out.msg);
    out.svp.unwrap()
}

fn archive(name: &str, xml: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(xml.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

fn score(native: Option<&str>) -> String {
    let words = [
        ("rêve", "single"),
        ("blanc", "single"),
        ("neige", "single"),
        ("blan", "single"),
        ("merci", "single"),
        ("blanc", "begin"),
        ("an", "end"),
        ("blan", "begin"),
        ("an", "end"),
    ];
    let mut parts = String::new();
    let mut body = String::new();
    for id in 1..=3 {
        if native.is_some() {
            parts.push_str(&format!(
                "<Part><Staff id=\"{id}\"/><trackName>Voice {id}</trackName></Part>"
            ));
            let ns: String = words.iter().map(|(w, s)| format!("<Chord><durationType>quarter</durationType><Lyrics><syllabic>{s}</syllabic><text>{w}</text></Lyrics><Note><pitch>{}</pitch><tpc>14</tpc></Note></Chord>", 60 - id)).collect();
            body.push_str(&format!("<Staff id=\"{id}\"><Measure len=\"9/4\"><voice><TimeSig><sigN>9</sigN><sigD>4</sigD></TimeSig>{ns}</voice></Measure></Staff>"));
        } else {
            parts.push_str(&format!(
                "<score-part id=\"P{id}\"><part-name>Voice {id}</part-name></score-part>"
            ));
            let ns: String = words.iter().map(|(w, s)| format!("<note><pitch><step>C</step><octave>{}</octave></pitch><duration>1</duration><voice>1</voice><staff>1</staff><lyric><syllabic>{s}</syllabic><text>{w}</text></lyric></note>", 5 - id)).collect();
            body.push_str(&format!("<part id=\"P{id}\"><measure number=\"1\"><attributes><divisions>1</divisions><time><beats>9</beats><beat-type>4</beat-type></time></attributes>{ns}</measure></part>"));
        }
    }
    match native {
        Some(version) => format!("<museScore version=\"{version}\"><Score><Division>480</Division>{parts}{body}</Score></museScore>"),
        None => format!("<score-partwise version=\"4.0\"><part-list>{parts}</part-list>{body}</score-partwise>"),
    }
}

#[test]
fn score_adapters_preserve_both_targets_and_each_source_lyric() {
    let xml = score(None);
    let mut sources = vec![
        musicxml::parse(xml.as_bytes()).unwrap(),
        musicxml::parse(&archive("score.musicxml", &xml)).unwrap(),
    ];
    for version in ["3.02", "4.0"] {
        let xml = score(Some(version));
        sources.push(musescore::parse(xml.as_bytes()).unwrap());
        sources.push(musescore::parse(&archive("score.mscx", &xml)).unwrap());
    }
    for source_ir in sources {
        let before = format!("{source_ir:?}");
        for target in [Ustx, Svp] {
            let baseline = convert(&source_ir, target, Default);
            for profile in [FrenchMillefeuille, Automatic] {
                let project = convert(&source_ir, target, profile);
                assert_eq!(project.tracks.len(), baseline.tracks.len());
                assert_eq!(
                    project
                        .tracks
                        .iter()
                        .map(|t| &t.source_track_id)
                        .collect::<Vec<_>>(),
                    baseline
                        .tracks
                        .iter()
                        .map(|t| &t.source_track_id)
                        .collect::<Vec<_>>()
                );
                for (track, original) in project.tracks.iter().zip(&baseline.tracks) {
                    assert_eq!(track.notes.len(), original.notes.len());
                    for (index, (n, old)) in track.notes.iter().zip(&original.notes).enumerate() {
                        let mut lyric = source(n).clone();
                        // Default joins source words and changes their projected
                        // binding. It is the geometry oracle, not the raw lyric oracle.
                        lyric.state = source(old).state.clone();
                        lyric.syllabic = source(old).syllabic.clone();
                        assert_eq!(&lyric, source(old));
                        if target == Ustx {
                            let expected = match index {
                                5 | 7 => Syllabic::Begin,
                                6 | 8 => Syllabic::End,
                                _ => Syllabic::Single,
                            };
                            assert_eq!(source(n).syllabic, Some(expected));
                            assert_eq!(source(n).state, LyricState::Text(source(n).raw.clone()));
                        }
                        let mut restored = n.clone();
                        restored.lyric = old.lyric.clone();
                        restored.pronunciation_language = old.pronunciation_language;
                        assert_eq!(&restored, old);
                    }
                }
                if target == Svp {
                    let json =
                        String::from_utf8(target::serialize_to(target, &project).unwrap()).unwrap();
                    for token in ["fr/", "en/", "phonemizer"] {
                        assert!(!json.contains(token));
                    }
                    if profile == FrenchMillefeuille {
                        assert_eq!(project, baseline);
                    }
                } else {
                    let native = ustx::serialize(&project).unwrap();
                    assert_eq!(native.voice_parts.len(), 3);
                    for (part, track) in native.voice_parts.iter().zip(&project.tracks) {
                        for index in [1, 3, 5, 7] {
                            assert_eq!(phones(&track.notes[index]), Some(NASAL));
                        }
                        for index in [6, 8] {
                            assert_eq!(phones(&track.notes[index]), Some("fr/en"));
                        }
                        assert_eq!(part.notes[3].lyric, "blan[fr/b fr/l fr/en]");
                        if profile == Automatic {
                            assert!(track
                                .notes
                                .iter()
                                .all(|n| n.pronunciation_language
                                    == Some(PronunciationLanguage::French)));
                        }
                    }
                }
            }
        }
        assert_eq!(format!("{source_ir:?}"), before);
    }
}

#[test]
fn midi_and_kar_use_context_without_inventing_score_boundaries() {
    for karaoke in [false, true] {
        let mut track = Vec::new();
        if karaoke {
            track.extend_from_slice(b"\0\xff\x01\x13@KMIDI KARAOKE FILE");
        }
        for (i, word) in ["neige", "blanc", "neige", "blan", "neige", "blan-", "-an"]
            .iter()
            .enumerate()
        {
            track.extend_from_slice(&[0, 0xff, 0x05, word.len() as u8]);
            track.extend_from_slice(word.as_bytes());
            track.extend_from_slice(&[0, 0x90, 60 + i as u8, 100]);
            track.extend_from_slice(&[0x83, 0x60, 0x80, 60 + i as u8, 0]);
        }
        track.extend_from_slice(&[0, 0xff, 0x2f, 0]);
        let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend(track);
        let ir = if karaoke {
            midi::parse_with_karaoke_profile(&bytes)
        } else {
            midi::parse(&bytes)
        }
        .unwrap();
        for profile in [FrenchMillefeuille, Automatic] {
            let out = convert(&ir, Ustx, profile);
            let ns = &out.tracks[0].notes;
            assert_eq!(phones(&ns[3]), Some(NASAL));
            assert_ne!(
                phones(&ns[5]),
                Some(NASAL),
                "typed hyphens are not bilateral score evidence"
            );
            let svp = convert(&ir, Svp, profile);
            let text = String::from_utf8(target::serialize_to(Svp, &svp).unwrap()).unwrap();
            assert!(!text.contains("fr/") && !text.contains("phonemizer"));
        }
    }
}
