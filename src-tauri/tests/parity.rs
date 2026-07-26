//! Engine parity tests on real song files, plus pure score-structure tests.
//!
//! The song fixtures are copyrighted karaoke/score files and are therefore NOT
//! committed to the public repository (see .gitignore). File-based tests are
//! explicitly ignored unless requested with `cargo test --test parity -- --ignored`;
//! once requested, every missing fixture is a hard failure. The unroll tests
//! below always run.
use std::collections::HashMap;
use verse_lib::engine::convert::{convert_auto, convert_bytes, convert_midi_with, ConvertOutcome};
use verse_lib::engine::midi;

fn read_fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!("required private fixture is unavailable at {path}: {error}")
    })
}

fn conv(name: &str) -> ConvertOutcome {
    convert_bytes(&read_fixture(name), "english")
}

fn conv_auto(name: &str) -> ConvertOutcome {
    convert_auto(&read_fixture(name), "english")
}

#[test]
fn unroll_repeat_with_voltas() {
    use verse_lib::engine::midi::{unroll, MeasureMarks};
    // M0 | M1(||: ) M2 | M3(volta 1, :||) | M4(volta 2) | M5
    let mut m = vec![MeasureMarks::default(); 6];
    m[1].start_repeat = true;
    m[3].volta = Some(vec![1]);
    m[3].end_repeat = 2;
    m[4].volta = Some(vec![2]);
    let order = unroll(&m).expect("repeat structure is representable");
    // pass 1: 0 1 2 3 -> back; pass 2: 1 2 (3 skipped) 4 5
    assert_eq!(
        order,
        vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (1, 1),
            (2, 1),
            (4, 0),
            (5, 0)
        ]
    );
}

#[test]
fn unroll_ds_al_coda() {
    use verse_lib::engine::midi::{unroll, Jump, MeasureMarks};
    // M0(segno) M1 M2(To Coda) M3(D.S. al Coda) M4(Coda) M5
    let mut m = vec![MeasureMarks::default(); 6];
    m[0].segno = true;
    m[2].to_coda = true;
    m[3].jump = Some(Jump::DsAlCoda);
    m[4].coda = true;
    let order = unroll(&m).expect("D.S. al Coda structure is representable");
    // 0 1 2 3 -> D.S. -> 0 1 2 -> To Coda -> 4 5
    assert_eq!(
        order,
        vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (1, 1),
            (2, 1),
            (4, 0),
            (5, 0)
        ]
    );
}

#[test]
fn unroll_dc_al_fine() {
    use verse_lib::engine::midi::{unroll, Jump, MeasureMarks};
    // M0 M1(Fine) M2 M3(D.C. al Fine)
    let mut m = vec![MeasureMarks::default(); 4];
    m[1].fine = true;
    m[3].jump = Some(Jump::DcAlFine);
    let order = unroll(&m).expect("D.C. al Fine structure is representable");
    // 0 1 2 3 -> D.C. -> 0 1(Fine, stop)
    assert_eq!(order, vec![(0, 0), (1, 0), (2, 0), (3, 0), (0, 1), (1, 1)]);
}

#[test]
#[ignore = "requires the private hound_dog.kar fixture"]
fn hound_dog_multitrack() {
    let data = read_fixture("hound_dog.kar");
    let parsed = midi::parse(&data).expect("valid KAR/SMF");
    let automatic = convert_midi_with(&parsed, "english", None);
    assert!(automatic.ok, "Hound Dog must convert");
    // One report entry per projection lane. A source track that sounds two
    // notes at once becomes two monophonic lanes, so lanes outnumber the source
    // voices `n_tracks` counts; every lane must still be inventoried.
    assert_eq!(
        automatic.tracks.len(),
        parsed.tracks.len(),
        "every projection lane is inventoried"
    );
    assert_eq!(automatic.n_tracks, parsed.topology.voice_count());
    assert_eq!(
        automatic.placed, 244,
        "the complete lyrics stream must bind to its unique timing-compatible melody"
    );
    let automatic_vocal: Vec<_> = automatic
        .tracks
        .iter()
        .filter(|track| track.role == "vocal")
        .collect();
    assert_eq!(automatic_vocal.len(), 1);
    assert_eq!(automatic_vocal[0].placed, 244);

    // The override targets a track by report index. Splitting a source track
    // into monophonic voices moves those indices, so the lead vocal is found by
    // name instead of by a number that no longer means the same lane.
    let lead = automatic
        .tracks
        .iter()
        .find(|track| track.track.starts_with("Lead Vox"))
        .expect("the lead vocal track is inventoried")
        .id;
    let r = convert_midi_with(&parsed, "english", Some(&HashMap::from([(lead, true)])));
    assert_eq!(
        r.placed, 244,
        "the proven external binding remains source-backed under an explicit override; report: {:?}",
        r.tracks
    );
    let vox = r
        .tracks
        .iter()
        .find(|t| t.role == "vocal")
        .expect("one singing track");
    assert_eq!(vox.placed, 244);
    let svp = r.svp.unwrap();
    assert_eq!(
        svp.tracks
            .iter()
            .flat_map(|track| &track.main_group.notes)
            .filter(|note| !note.lyrics.is_empty())
            .count(),
        244
    );
}

#[test]
#[ignore = "requires the private help.kar fixture"]
fn help_kar_binds_words_only_to_the_unique_lead_track() {
    let data = read_fixture("help.kar");
    let parsed = midi::parse(&data).expect("valid KAR/SMF");
    let automatic = convert_midi_with(&parsed, "english", None);
    assert_eq!(
        automatic
            .tracks
            .iter()
            .filter(|track| track.role == "vocal")
            .count(),
        1,
        "only the complete timing-compatible melody may receive the Words stream"
    );
    assert_eq!(automatic.placed, 314);
    // The user may explicitly export Lead + Harm 1 + Harm 2 as vocal notes,
    // but that never transfers the separate Words track into them.
    // Overrides address report indices, which move once a source track is
    // split into monophonic voices. Select the three sung parts by name so the
    // test states its intent rather than a numbering.
    let chosen: HashMap<usize, bool> = automatic
        .tracks
        .iter()
        .filter(|track| {
            ["Lead", "Harm 1", "Harm 2"]
                .iter()
                .any(|name| track.track.contains(name))
        })
        .map(|track| (track.id, true))
        .collect();
    let r = convert_midi_with(&parsed, "english", Some(&chosen));
    assert!(r.ok);
    // Every source voice owns at least one projection lane; a voice that
    // sounds two notes at once owns several.
    assert!(
        r.tracks.len() >= r.n_tracks,
        "every source voice keeps at least one inventoried lane"
    );
    let vocal: Vec<_> = r.tracks.iter().filter(|t| t.role == "vocal").collect();
    assert_eq!(
        vocal.len(),
        chosen.len(),
        "every selected part sings, one lane at a time; report: {:?}",
        r.tracks
    );
    let lead = vocal
        .iter()
        .find(|t| t.track.contains("Lead"))
        .expect("Lead track");
    assert_eq!(lead.placed, 314);
    assert!(vocal
        .iter()
        .any(|t| t.track.contains("Harm 1") && t.placed == 0));
    assert!(vocal
        .iter()
        .any(|t| t.track.contains("Harm 2") && t.placed == 0));
    // the 3 explicitly selected singing tracks are the only SVP tracks
    let svp = r.svp.unwrap();
    assert_eq!(svp.tracks.len(), 3);
    assert!(svp.tracks[0].name.contains("Lead") || svp.tracks[0].name.contains("Harm"));
    assert_eq!(
        svp.tracks
            .iter()
            .flat_map(|track| &track.main_group.notes)
            .filter(|note| !note.lyrics.is_empty())
            .count(),
        314
    );
    assert!(svp
        .tracks
        .iter()
        .filter(|track| track.name.contains("Harm"))
        .flat_map(|track| &track.main_group.notes)
        .all(|note| note.lyrics.is_empty()));
}

#[test]
#[ignore = "requires the private help.mxl fixture"]
fn musicxml_help_lyrics() {
    let r = conv_auto("help.mxl");
    assert!(r.ok, "the MusicXML must convert: {:?}", r.msg);
    assert!(
        r.tracks.iter().any(|t| t.role == "vocal"),
        "at least one singing track"
    );
    assert!(r.placed > 100, "many syllables placed, got {}", r.placed);
    let svp = r.svp.unwrap();
    let has = |w: &str| {
        svp.tracks.iter().any(|tr| {
            tr.main_group
                .notes
                .iter()
                .any(|n| n.lyrics.to_lowercase().contains(w))
        })
    };
    assert!(has("help"), "the real lyrics (Help) must appear");
    // "changed" only exists in verse 2 -> proves the MusicXML unrolling
    assert!(
        has("changed"),
        "verse 2 must be sung (.mxl repeats unrolled)"
    );
}

#[test]
#[ignore = "requires the private help.mscz fixture"]
fn musescore_mscz_native() {
    // The user's primary format: the .mscz must convert natively.
    let r = conv_auto("help.mscz");
    assert!(r.ok, "the .mscz must convert: {:?}", r.msg);
    let vocal = r.tracks.iter().filter(|t| t.role == "vocal").count();
    assert!(vocal >= 3, "the 3 voices must sing, got {}", vocal);
    assert!(r.placed > 300, "many syllables placed, got {}", r.placed);
    let svp = r.svp.unwrap();
    let has = |w: &str| {
        svp.tracks.iter().any(|tr| {
            tr.main_group
                .notes
                .iter()
                .any(|n| n.lyrics.to_lowercase().contains(w))
        })
    };
    assert!(has("help"), "the real lyrics (Help) must appear");
    // "changed" only exists in verse 2 -> proves the repeat unrolling
    assert!(
        has("changed"),
        "verse 2 must be sung on the 2nd pass of the repeat"
    );
    // part names must come from longName (not "Track N")
    assert!(
        svp.tracks.iter().any(|tr| {
            tr.name.starts_with("Mi-que") || tr.name.starts_with("Do") || tr.name.starts_with('T')
        }),
        "real part names expected"
    );
}

#[test]
#[ignore = "requires the private help.mscz and help.mxl fixtures"]
fn the_same_score_projects_identically_from_mscz_and_mxl() {
    // The two fixtures are the same piece exported twice, so the two paths must
    // agree note for note. Before ties were merged and repeats were computed
    // score-wide, the .mscz gave 250/162/173 notes against the .mxl's
    // 214/210/231 — and both dropped the syllable sitting on a tied note.
    let from_mscz = conv_auto("help.mscz");
    let from_mxl = conv_auto("help.mxl");
    let voices = |outcome: &ConvertOutcome| {
        outcome
            .svp
            .as_ref()
            .expect("conversion succeeds")
            .tracks
            .iter()
            .map(|track| {
                track
                    .main_group
                    .notes
                    .iter()
                    .map(|note| (note.onset, note.duration, note.pitch, note.lyrics.clone()))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let mscz = voices(&from_mscz);
    let mxl = voices(&from_mxl);
    assert_eq!(
        mscz.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![214, 210, 235],
        "each staff must sustain its ties and unroll the repeat"
    );
    assert_eq!(
        mscz, mxl,
        "the MuseScore projection must match the MusicXML one note for note"
    );

    // No word may be lost on either path. "ate" sits on a tied note and used to
    // vanish with the merge.
    let words = |projection: &Vec<Vec<(i64, i64, u8, String)>>| {
        projection
            .iter()
            .flatten()
            .map(|note| note.3.trim().to_lowercase())
            .filter(|text| !text.is_empty())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert!(
        words(&mscz).contains("ate"),
        "a syllable on a tied note is sung"
    );
    assert_eq!(words(&mscz), words(&mxl));

    // The verses are stacked two deep only where their words differ; the
    // refrain that follows them is written on a single lyric line for both
    // passes. Reading that single line as "verse 2 is silent here" deleted the
    // whole refrain from the second pass — ninety words across the three
    // voices, twenty seconds of the piece with notes but nothing to sing.
    let sung = |projection: &Vec<Vec<(i64, i64, u8, String)>>| {
        projection
            .iter()
            .map(|track| {
                track
                    .iter()
                    .filter(|note| !note.3.trim().is_empty())
                    .count()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        sung(&mscz),
        vec![197, 194, 212],
        "each pass sings its own verse and the refrain both passes share"
    );
    assert_eq!(sung(&mscz), sung(&mxl));
    assert_eq!(
        mscz[0].iter().filter(|note| note.3 == "bam").count(),
        18,
        "the refrain is sung on the repeat, not only on the first pass"
    );
}

#[test]
#[ignore = "requires the private queen.kar fixture"]
fn queen_lyrics_without_source_melody_do_not_create_c4() {
    let r = conv("queen.kar");
    assert!(r.ok, "a lyric-only karaoke file must still be accepted");
    // Every source voice owns at least one projection lane; a voice that
    // sounds two notes at once owns several.
    assert!(
        r.tracks.len() >= r.n_tracks,
        "every source voice keeps at least one inventoried lane"
    );
    assert!(
        r.tracks.iter().all(|t| t.role != "vocal_synth"),
        "the converter must not invent a synthetic melody track"
    );
    let svp = r.svp.expect("a valid, possibly empty SVP project");
    assert!(
        svp.tracks
            .iter()
            .flat_map(|track| track.main_group.notes.iter())
            .all(|note| note.pitch != 60 || !note.lyrics.is_empty()),
        "no lyric-only token may become an invented C4"
    );
}

#[test]
#[ignore = "requires the private help.mscz fixture"]
fn help_mscz_styled_names_are_not_fused() {
    let r = conv_auto("help.mscz");
    assert!(r.ok, "help.mscz must convert: {:?}", r.msg);
    // <longName>Batterie ou<br/>persussions<br/>corporelles</longName>:
    // <br/> must become a space, never fuse the words.
    assert!(
        r.tracks
            .iter()
            .any(|t| t.track.contains("ou persussions corporelles")),
        "multi-line longName must be collapsed with spaces, got: {:?}",
        r.tracks.iter().map(|t| t.track.clone()).collect::<Vec<_>>()
    );
    assert!(
        r.tracks.iter().all(|t| !t.track.contains("oupersussions")),
        "words fused across <br/>"
    );
}

#[test]
#[ignore = "requires the private help.mxl fixture"]
fn help_mxl_converts() {
    let r = conv_auto("help.mxl");
    assert!(r.ok, "help.mxl must convert: {:?}", r.msg);
    assert!(r.placed > 0, "lyrics must be placed");
}

/// Syllables a Soft Karaoke file actually asks to be sung, one list per text
/// track. Control records (`@…`), line/paragraph markers and punctuation-only
/// tokens are not words.
fn kar_syllable_streams(data: &[u8]) -> Vec<Vec<String>> {
    let parsed = midi::parse_with_karaoke_profile(data).expect("valid KAR");
    parsed
        .tracks
        .iter()
        .map(|track| {
            track
                .events
                .iter()
                .filter_map(|event| match &event.kind {
                    // Generic Text is only karaoke lyrics when this very track
                    // proves the Soft Karaoke profile, exactly as production
                    // qualifies it. Counting unqualified metadata as required
                    // syllables would fail the test for words nobody sings.
                    midi::Kind::Text(text)
                        if track.text_profile == midi::MidiTextProfile::KaraokeLyrics =>
                    {
                        Some(text.text.clone())
                    }
                    midi::Kind::Lyrics(lyric) => match &lyric.state {
                        midi::LyricState::Text(text) => Some(text.clone()),
                        _ => None,
                    },
                    _ => None,
                })
                .filter(|text| !text.starts_with('@'))
                .map(|text| {
                    text.replace(['\r', '\n'], "")
                        .trim_start_matches(['\\', '/'])
                        .trim()
                        .to_string()
                })
                .filter(|text| !text.is_empty() && text.chars().any(|c| c != '.'))
                .collect()
        })
        .collect()
}

/// Every syllable the source writes must reach the project. A KAR may transcribe
/// the same passage in two competing text tracks, so the fullest stream is the
/// reference: adding them together counts a duplicate as a loss.
fn assert_no_syllable_is_lost(fixture: &str) {
    let data = read_fixture(fixture);
    let reference = kar_syllable_streams(&data)
        .into_iter()
        .max_by_key(Vec::len)
        .unwrap_or_default();
    assert!(
        !reference.is_empty(),
        "{fixture} must carry singable syllables"
    );

    let outcome = convert_bytes(&data, "english");
    assert!(outcome.ok, "{fixture}: {:?}", outcome.msg);
    let mut projected: HashMap<&str, usize> = HashMap::new();
    let project = outcome.svp.as_ref().expect("valid SVP");
    for note in project
        .tracks
        .iter()
        .flat_map(|track| track.main_group.notes.iter())
    {
        let lyric = note.lyrics.trim();
        if !lyric.is_empty() {
            *projected.entry(lyric).or_default() += 1;
        }
    }
    let mut missing: Vec<&str> = Vec::new();
    let mut needed: HashMap<&str, usize> = HashMap::new();
    for syllable in &reference {
        *needed.entry(syllable.as_str()).or_default() += 1;
    }
    for (syllable, count) in needed {
        if projected.get(syllable).copied().unwrap_or(0) < count {
            missing.push(syllable);
        }
    }
    missing.sort_unstable();
    assert!(
        missing.is_empty(),
        "{fixture} drops syllables the source writes: {missing:?}"
    );
}

#[test]
#[ignore = "requires the private KAR fixtures"]
fn no_karaoke_syllable_is_lost_on_the_way_to_the_project() {
    // The counters said these files were complete while a whole refrain line
    // was missing from one of them, so the oracle is the words themselves.
    for fixture in ["help.kar", "hound_dog.kar"] {
        assert_no_syllable_is_lost(fixture);
    }
}
