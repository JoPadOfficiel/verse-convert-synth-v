//! Engine parity tests on real song files, plus pure score-structure tests.
//!
//! The song fixtures are copyrighted karaoke/score files and are therefore NOT
//! committed to the public repository (see .gitignore). File-based tests skip
//! gracefully when a fixture is absent (e.g. in CI); the unroll tests below
//! always run.
use verse_lib::engine::convert::{convert_auto, convert_bytes, ConvertOutcome};

fn read_fixture(name: &str) -> Option<Vec<u8>> {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    match std::fs::read(&path) {
        Ok(d) => Some(d),
        Err(_) => {
            eprintln!("fixture not present, test skipped: {}", name);
            None
        }
    }
}

fn conv(name: &str) -> Option<ConvertOutcome> {
    read_fixture(name).map(|d| convert_bytes(&d, "english"))
}

fn conv_auto(name: &str) -> Option<ConvertOutcome> {
    read_fixture(name).map(|d| convert_auto(&d, "english"))
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c)
    })
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
    let order = unroll(&m);
    // pass 1: 0 1 2 3 -> back; pass 2: 1 2 (3 skipped) 4 5
    assert_eq!(
        order,
        vec![(0, 0), (1, 0), (2, 0), (3, 0), (1, 1), (2, 1), (4, 0), (5, 0)]
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
    let order = unroll(&m);
    // 0 1 2 3 -> D.S. -> 0 1 2 -> To Coda -> 4 5
    assert_eq!(
        order,
        vec![(0, 0), (1, 0), (2, 0), (3, 0), (0, 1), (1, 1), (2, 1), (4, 0), (5, 0)]
    );
}

#[test]
fn unroll_dc_al_fine() {
    use verse_lib::engine::midi::{unroll, Jump, MeasureMarks};
    // M0 M1(Fine) M2 M3(D.C. al Fine)
    let mut m = vec![MeasureMarks::default(); 4];
    m[1].fine = true;
    m[3].jump = Some(Jump::DcAlFine);
    let order = unroll(&m);
    // 0 1 2 3 -> D.C. -> 0 1(Fine, stop)
    assert_eq!(order, vec![(0, 0), (1, 0), (2, 0), (3, 0), (0, 1), (1, 1)]);
}

#[test]
fn hound_dog_multitrack() {
    let Some(r) = conv("hound_dog.kar") else { return };
    assert!(r.ok, "Hound Dog must convert");
    assert_eq!(r.n_tracks, 10, "10 tracks kept");
    assert_eq!(r.placed, 244, "244 syllables placed");
    let vox = r.tracks.iter().find(|t| t.role == "vocal").expect("one singing track");
    assert_eq!(vox.placed, 244);
    let svp = r.svp.unwrap();
    for tr in &svp.tracks {
        for n in &tr.main_group.notes {
            assert!(!has_cjk(&n.lyrics), "no Japanese/Chinese characters");
        }
    }
}

#[test]
fn help_kar_harmonies_sing_too() {
    // Critical bug fixed: the harmonies (Harm 1/2) must sing,
    // not only the lead voice.
    let Some(r) = conv("help.kar") else { return };
    assert!(r.ok);
    assert_eq!(r.n_tracks, 9);
    let vocal: Vec<_> = r.tracks.iter().filter(|t| t.role == "vocal").collect();
    assert_eq!(vocal.len(), 3, "Lead + Harm 1 + Harm 2 must sing");
    let lead = vocal.iter().find(|t| t.track.contains("Lead")).expect("Lead track");
    assert_eq!(lead.placed, 314);
    assert!(vocal.iter().any(|t| t.track.contains("Harm 1") && t.placed >= 50));
    assert!(vocal.iter().any(|t| t.track.contains("Harm 2") && t.placed >= 40));
    // the 3 singing tracks must be at the top of the .svp
    let svp = r.svp.unwrap();
    assert!(svp.tracks[0].name.contains("Lead") || svp.tracks[0].name.contains("Harm"));
}

#[test]
fn musicxml_help_lyrics() {
    let Some(r) = conv_auto("help.mxl") else { return };
    assert!(r.ok, "the MusicXML must convert: {:?}", r.msg);
    assert!(
        r.tracks.iter().any(|t| t.role == "vocal"),
        "at least one singing track"
    );
    assert!(r.placed > 100, "many syllables placed, got {}", r.placed);
    let svp = r.svp.unwrap();
    let has = |w: &str| {
        svp.tracks.iter().any(|tr| {
            tr.main_group.notes.iter().any(|n| n.lyrics.to_lowercase().contains(w))
        })
    };
    assert!(has("help"), "the real lyrics (Help) must appear");
    // "changed" only exists in verse 2 -> proves the MusicXML unrolling
    assert!(has("changed"), "verse 2 must be sung (.mxl repeats unrolled)");
}

#[test]
fn musescore_mscz_native() {
    // The user's primary format: the .mscz must convert natively.
    let Some(r) = conv_auto("help.mscz") else { return };
    assert!(r.ok, "the .mscz must convert: {:?}", r.msg);
    let vocal = r.tracks.iter().filter(|t| t.role == "vocal").count();
    assert!(vocal >= 3, "the 3 voices must sing, got {}", vocal);
    assert!(r.placed > 300, "many syllables placed, got {}", r.placed);
    let svp = r.svp.unwrap();
    let has = |w: &str| {
        svp.tracks.iter().any(|tr| {
            tr.main_group.notes.iter().any(|n| n.lyrics.to_lowercase().contains(w))
        })
    };
    assert!(has("help"), "the real lyrics (Help) must appear");
    // "changed" only exists in verse 2 -> proves the repeat unrolling
    assert!(has("changed"), "verse 2 must be sung on the 2nd pass of the repeat");
    // part names must come from longName (not "Track N")
    assert!(
        svp.tracks.iter().any(|tr| tr.name == "Mi-que" || tr.name == "Do" || tr.name == "T"),
        "real part names expected"
    );
}

#[test]
fn queen_synthetic_lyrics_track() {
    let Some(r) = conv("queen.kar") else { return };
    assert!(r.ok, "Queen must produce a result (Lyrics track)");
    assert_eq!(r.n_tracks, 12, "11 backing + 1 Lyrics");
    assert!(
        r.tracks.iter().any(|t| t.role == "vocal_synth"),
        "one synthetic 'Lyrics' track"
    );
    assert_eq!(r.placed, 253, "253 syllables");
}

#[test]
fn help_mscz_styled_names_are_not_fused() {
    let Some(r) = conv_auto("help.mscz") else { return };
    assert!(r.ok, "help.mscz must convert: {:?}", r.msg);
    // <longName>Batterie ou<br/>persussions<br/>corporelles</longName>:
    // <br/> must become a space, never fuse the words.
    assert!(
        r.tracks.iter().any(|t| t.track.contains("ou persussions corporelles")),
        "multi-line longName must be collapsed with spaces, got: {:?}",
        r.tracks.iter().map(|t| t.track.clone()).collect::<Vec<_>>()
    );
    assert!(
        r.tracks.iter().all(|t| !t.track.contains("oupersussions")),
        "words fused across <br/>"
    );
}

#[test]
fn help_mxl_converts() {
    let Some(r) = conv_auto("help.mxl") else { return };
    assert!(r.ok, "help.mxl must convert: {:?}", r.msg);
    assert!(r.placed > 0, "lyrics must be placed");
}
