//! The syllable-per-note notation every vocal score uses, end to end.
//!
//! A score writes one syllable under each note and binds the syllables of one
//! word — with `<syllabic>` in MusicXML and MuseScore, with a hyphen inside the
//! text in the files a MIDI exporter writes. Both targets want the word whole on
//! the first note of its run and a `+` on the notes that follow, which is what
//! OpenUtau's own MusicXML importer writes (`Format/MusicXML.cs`), because a
//! phonemizer looks the lyric up in a pronunciation dictionary: `"mê"` and
//! `"me"` are not the word `"même"` under any reading.

use verse_lib::engine::convert::{convert_auto_with, convert_midi_with_target};
use verse_lib::engine::target::ExportTarget;
use verse_lib::engine::{musescore, musicxml, target};

fn french_export(body: &str) -> String {
    let midi = musicxml::parse(score(body).as_bytes()).unwrap();
    let outcome = verse_lib::engine::convert::convert_midi_with_profile(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        target::PronunciationProfile::FrenchMillefeuille,
    );
    assert!(outcome.ok, "{:?}", outcome.msg);
    String::from_utf8(
        target::serialize_to(ExportTarget::Ustx, outcome.svp.as_ref().unwrap()).unwrap(),
    )
    .unwrap()
}

#[test]
fn french_explicit_end_cannot_be_reinterpreted_as_a_layout_head() {
    let body = format!(
        "{}{}{}",
        note("C", Some("end"), "rêves"),
        note("D", Some("begin"), "ê"),
        note("E", Some("end"), "ves,")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&body)),
        ["rêves[fr/r fr/ae fr/v]", "ê", "ves,"]
    );
}

#[test]
fn french_unknown_connected_word_cannot_use_its_fragment_as_a_determiner() {
    let body = format!(
        "{}{}{}",
        note("C", Some("begin"), "des"),
        note("D", Some("end"), "tin"),
        note("E", None, "amis")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&body)),
        ["des", "tin", "amis[fr/ah fr/m fr/ih]"]
    );
    let midi = musicxml::parse(score(&body).as_bytes()).unwrap();
    let result = verse_lib::engine::convert::convert_midi_with_profile(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        target::PronunciationProfile::FrenchMillefeuille,
    );
    assert_eq!(
        result.tracks[0]
            .warnings
            .iter()
            .filter(|w| w.code == target::french::UNSUPPORTED)
            .count(),
        2
    );
}

#[test]
fn french_isolated_ends_never_fuse_unrelated_words() {
    let body = format!(
        "{}{}{}{}",
        note("C", None, "mes"),
        note("D", Some("end"), "rê"),
        note("E", None, "le"),
        note("F", Some("end"), "vent")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&body)),
        [
            "mes[fr/m fr/eh]",
            "rê[fr/r fr/ae]",
            "le[fr/l fr/ee]",
            "vent[fr/v fr/en]"
        ]
    );
    // Default behavior remains frozen even for this known bad source metadata.
    assert_eq!(
        ustx_lyrics(&export(&body, ExportTarget::Ustx)),
        ["mesrê", "+", "levent", "+"]
    );
}

#[test]
fn french_preserves_bracketed_melismas_without_losing_the_sung_schwa() {
    let body = format!(
        "{}{}{}",
        note("C", Some("begin"), "rê"),
        untexted("D"),
        note("E", Some("end"), "ves,")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&body)),
        ["rê[fr/r fr/ae]", "+~", "ves[fr/v fr/ee]"]
    );
}

#[test]
fn french_exact_layout_tolerates_repeated_begin_metadata_without_joining() {
    let full_word_head = format!(
        "{}{}{}",
        note("C", None, "rêves"),
        note("D", Some("begin"), "ê"),
        note("E", Some("end"), "ves,")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&full_word_head)),
        ["rêves[fr/r fr/ae]", "ê[fr/ae]", "ves[fr/v fr/ee]"]
    );
    let body = format!(
        "{}{}{}",
        note("C", Some("begin"), "rê"),
        note("D", Some("begin"), "ê"),
        note("E", Some("end"), "ves,")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&body)),
        ["rê[fr/r fr/ae]", "ê[fr/ae]", "ves[fr/v fr/ee]"]
    );
    let murmurs = format!(
        "{}{}{}{}",
        note("C", Some("begin"), "mur"),
        note("D", Some("begin"), "mu"),
        note("E", Some("middle"), "u"),
        note("F", Some("end"), "ures,")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&murmurs)),
        [
            "mur[fr/m fr/uh fr/r]",
            "mu[fr/m fr/uh]",
            "u[fr/uh]",
            "ures[fr/uh fr/r]"
        ]
    );
}

#[test]
fn french_added_layouts_never_cross_rests_or_invent_a_final_schwa() {
    let gap = format!(
        "{}{}{}",
        note("C", Some("begin"), "ju"),
        rest(),
        note("D", Some("end"), "ure,")
    );
    assert_eq!(ustx_lyrics(&french_export(&gap)), ["ju", "ure,"]);
    let held = format!(
        "{}{}{}",
        note("C", Some("begin"), "ju"),
        untexted("D"),
        note("E", Some("end"), "ure,")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&held)),
        ["ju[fr/j fr/uh]", "+~", "ure[fr/uh fr/r]"]
    );
}

#[test]
fn french_rests_untexted_notes_and_explicit_words_block_layouts() {
    let independent_word = format!(
        "{}{}{}",
        note("C", Some("single"), "rêves"),
        note("D", Some("begin"), "ê"),
        note("E", Some("end"), "ves,")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&independent_word)),
        ["rêves[fr/r fr/ae fr/v]", "ê", "ves,"]
    );
    let gap = format!(
        "{}{}{}",
        note("C", Some("begin"), "mê"),
        rest(),
        note("E", Some("end"), "me")
    );
    assert_eq!(ustx_lyrics(&french_export(&gap)), ["mê", "me"]);
    let unmarked = format!(
        "{}{}{}",
        note("C", None, "mê"),
        untexted("D"),
        note("E", None, "me")
    );
    assert_eq!(ustx_lyrics(&french_export(&unmarked)), ["mê", "me"]);
    let words = format!(
        "{}{}",
        note("C", Some("single"), "mê"),
        note("D", Some("single"), "me")
    );
    assert_eq!(ustx_lyrics(&french_export(&words)), ["mê", "me"]);
    let liaison = format!(
        "{}{}{}",
        note("C", None, "tout"),
        rest(),
        note("D", None, "au")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&liaison)),
        ["tout[fr/t fr/ou]", "au[fr/oh]"]
    );
}

#[test]
fn french_does_not_cross_source_lyric_rows_or_override_manual_hints() {
    let body = format!(
        "{}{}",
        note("C", None, "tout"),
        note("D", None, "au").replace("<lyric>", "<lyric number=\"2\">")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&body)),
        ["tout[fr/t fr/ou]", "au[fr/oh]"]
    );
    let manual = format!(
        "{}{}",
        note("C", Some("begin"), "mê[fr/m fr/ae]"),
        note("D", Some("end"), "me")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&manual)),
        ["mê[fr/m fr/ae]", "me"]
    );
    let manual_liaison = format!(
        "{}{}",
        note("C", None, "tout"),
        note("D", None, "au[fr/t fr/oh]")
    );
    assert_eq!(
        ustx_lyrics(&french_export(&manual_liaison)),
        ["tout[fr/t fr/ou]", "au[fr/t fr/oh]"]
    );
}

#[test]
fn french_blank_duplicates_keep_the_text_at_its_original_note() {
    for native in [false, true] {
        for blank_first in [false, true] {
            let blank = if native {
                "<Lyrics><syllabic>begin</syllabic><text></text></Lyrics>"
            } else {
                "<lyric><syllabic>begin</syllabic><text></text></lyric>"
            };
            let text = if native {
                "<Lyrics><text>rêves.</text></Lyrics>"
            } else {
                "<lyric><text>rêves.</text></lyric>"
            };
            let both = if blank_first {
                format!("{blank}{text}")
            } else {
                format!("{text}{blank}")
            };
            let midi = if native {
                musescore::parse(format!("<museScore version=\"4.0\"><Score><Division>480</Division><Part><Staff id=\"1\"/><trackName>Voice</trackName></Part><Staff id=\"1\"><Measure><voice><Chord><durationType>quarter</durationType><Lyrics><text>mes</text></Lyrics><Note><pitch>60</pitch></Note></Chord><Chord><durationType>quarter</durationType>{both}<Note><pitch>62</pitch></Note></Chord></voice></Measure></Staff></Score></museScore>").as_bytes()).unwrap()
            } else {
                musicxml::parse(
                    score(&format!(
                        "{}{}",
                        note("C", None, "mes"),
                        untexted("D").replace("</note>", &format!("{both}</note>"))
                    ))
                    .as_bytes(),
                )
                .unwrap()
            };
            let result = verse_lib::engine::convert::convert_midi_with_profile(
                &midi,
                "english",
                None,
                ExportTarget::Ustx,
                target::PronunciationProfile::FrenchMillefeuille,
            );
            assert!(result.ok, "{:?}", result.msg);
            assert_eq!(result.placed, 2);
            let output = target::ustx::serialize(result.svp.as_ref().unwrap()).unwrap();
            assert_eq!(output.voice_parts.len(), 1);
            let notes = &output.voice_parts[0].notes;
            assert_eq!(notes.len(), 2);
            assert_eq!(
                (notes[1].position, notes[1].duration, notes[1].tone),
                (480, 480, 62)
            );
            assert_eq!(notes[1].lyric, "rêves[fr/r fr/ae fr/v]");
            assert!(result.tracks[0]
                .warnings
                .iter()
                .any(|w| w.code == "FRENCH_DUPLICATE_BLANK_RESOLVED"));
            let originals: Vec<_> = midi
                .tracks
                .iter()
                .flat_map(|t| &t.events)
                .filter_map(|e| match &e.kind {
                    verse_lib::engine::midi::Kind::NoteOn(n) if n.lyrics.len() == 2 => {
                        Some(&n.lyrics)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                originals.len(),
                1,
                "both lyric records survive in source evidence"
            );
        }
    }
}

/// One `<note>` with one lyric. `syllabic` is written only when stated.
fn note(step: &str, syllabic: Option<&str>, text: &str) -> String {
    let lyric = match syllabic {
        Some(state) => {
            format!("<lyric><syllabic>{state}</syllabic><text>{text}</text></lyric>")
        }
        None => format!("<lyric><text>{text}</text></lyric>"),
    };
    format!(
        "<note><pitch><step>{step}</step><octave>4</octave></pitch><duration>1</duration>\
         <type>quarter</type>{lyric}</note>"
    )
}

fn untexted(step: &str) -> String {
    format!(
        "<note><pitch><step>{step}</step><octave>4</octave></pitch><duration>1</duration>\
         <type>quarter</type></note>"
    )
}

fn rest() -> String {
    "<note><rest/><duration>1</duration><type>quarter</type></note>".to_string()
}

fn score(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><score-partwise version=\"3.1\">\
         <part-list><score-part id=\"P1\"><part-name>Voix</part-name></score-part></part-list>\
         <part id=\"P1\"><measure number=\"1\"><attributes><divisions>1</divisions>\
         <time><beats>4</beats><beat-type>4</beat-type></time></attributes>{body}</measure></part>\
         </score-partwise>"
    )
}

fn export(source: &str, target: ExportTarget) -> String {
    let parsed = musicxml::parse(score(source).as_bytes()).expect("the score parses");
    let outcome = convert_midi_with_target(&parsed, "french", None, target);
    assert!(outcome.ok, "conversion refused: {:?}", outcome.msg);
    let projection = outcome.svp.as_ref().expect("a projection");
    let bytes = target::serialize_to(target, projection).expect("the target writes it");
    String::from_utf8(bytes).expect("valid UTF-8")
}

/// One MIDI Lyric meta event.
fn lyric(text: &[u8]) -> Vec<u8> {
    let mut event = vec![0x00, 0xff, 0x05, text.len() as u8];
    event.extend_from_slice(text);
    event
}

/// One quarter note at 480 ticks per quarter, ending where the next begins.
fn quarter(pitch: u8) -> Vec<u8> {
    vec![0x00, 0x90, pitch, 100, 0x83, 0x60, 0x80, pitch, 0x00]
}

/// One single-track SMF at 480 ticks per quarter, closed by end-of-track.
fn smf(events: &[Vec<u8>]) -> Vec<u8> {
    let mut track: Vec<u8> = events.concat();
    track.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let mut data = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
    data.extend_from_slice(&(track.len() as u32).to_be_bytes());
    data.extend_from_slice(&track);
    data
}

/// Any supported container, detected the way the application detects it.
fn ustx_of(data: &[u8]) -> String {
    let outcome = convert_auto_with(data, "french", None);
    assert!(outcome.ok, "conversion refused: {:?}", outcome.msg);
    let bytes = target::serialize_to(ExportTarget::Ustx, outcome.svp.as_ref().unwrap())
        .expect("the target writes it");
    String::from_utf8(bytes).expect("valid UTF-8")
}

fn ustx_lyrics(file: &str) -> Vec<String> {
    file.lines()
        .filter_map(|line| line.trim().strip_prefix("lyric: "))
        .map(|value| value.trim().trim_matches('"').to_string())
        .collect()
}

/// TC-01 — a two-syllable word bound by `<syllabic>`: the word on the first
/// note, OpenUtau's syllable-split marker on the second.
#[test]
fn a_word_split_over_two_notes_is_written_whole_on_the_first() {
    let file = export(
        &format!(
            "{}{}",
            note("C", Some("begin"), "J'i"),
            note("D", Some("end"), "rai")
        ),
        ExportTarget::Ustx,
    );
    assert_eq!(ustx_lyrics(&file), vec!["J'irai", "+"]);
}

/// TC-02 — the accents a French score writes survive the join byte for byte.
/// Mojibake would show up here as anything other than the source characters.
#[test]
fn accents_survive_the_join_exactly() {
    let file = export(
        &format!(
            "{}{}",
            note("C", Some("begin"), "mê"),
            note("D", Some("end"), "me")
        ),
        ExportTarget::Ustx,
    );
    assert_eq!(ustx_lyrics(&file), vec!["même", "+"]);
}

/// TC-03 — an elided word: the apostrophe belongs to the word and the accents
/// to their syllables, and three notes carry one lookup.
#[test]
fn an_elided_three_syllable_word_is_rebuilt_whole() {
    let file = export(
        &format!(
            "{}{}{}",
            note("C", Some("begin"), "s'a"),
            note("D", Some("middle"), "chè"),
            note("E", Some("end"), "ve")
        ),
        ExportTarget::Ustx,
    );
    assert_eq!(ustx_lyrics(&file), vec!["s'achève", "+", "+"]);
}

/// TC-04 — no regression on the words a score writes under a single note:
/// nothing is bound, nothing is rewritten.
#[test]
fn whole_words_under_one_note_each_are_untouched() {
    let file = export(
        &format!(
            "{}{}{}",
            note("C", Some("single"), "tout"),
            note("D", None, "au"),
            note("E", Some("single"), "bout")
        ),
        ExportTarget::Ustx,
    );
    assert_eq!(ustx_lyrics(&file), vec!["tout", "au", "bout"]);
}

/// The mode a MIDI exporter writes: the hyphen lives inside the lyric text and
/// only the syllable that runs on carries it. The dash is a marker, not a
/// letter, so it never reaches the file.
#[test]
fn hyphens_inside_the_text_bind_a_word_just_as_syllabic_does() {
    let file = export(
        &format!(
            "{}{}{}",
            note("C", None, "mi-"),
            note("D", None, "nu-"),
            note("E", None, "te")
        ),
        ExportTarget::Ustx,
    );
    assert_eq!(ustx_lyrics(&file), vec!["minute", "+", "+"]);
}

/// Both notations inside one phrase, which is what a real score mixes.
#[test]
fn a_mixed_phrase_binds_only_what_the_score_binds() {
    let file = export(
        &format!(
            "{}{}{}{}{}",
            note("C", Some("begin"), "rê"),
            note("D", Some("end"), "ves"),
            note("E", None, "tout"),
            note("F", None, "au-"),
            note("G", None, "tour")
        ),
        ExportTarget::Ustx,
    );
    assert_eq!(
        ustx_lyrics(&file),
        vec!["rêves", "+", "tout", "autour", "+"]
    );
}

/// A note the score writes inside a word with no word of its own: MuseScore
/// draws its continuation dash over it and states nothing else. Left out of the
/// lane it would shorten the melisma, so it is held instead.
#[test]
fn a_wordless_note_inside_a_word_is_held_rather_than_left_out() {
    let file = export(
        &format!(
            "{}{}{}",
            note("C", Some("begin"), "rê"),
            untexted("D"),
            note("E", Some("end"), "ves")
        ),
        ExportTarget::Ustx,
    );
    assert_eq!(ustx_lyrics(&file), vec!["rêves", "+~", "+"]);
}

/// A rest inside the word: neither target can state a marker across silence —
/// OpenUtau wires an extension only when the previous note ends exactly where
/// this one begins — so both syllables stay words, and the file still opens.
#[test]
fn a_rest_inside_a_word_leaves_both_syllables_as_written() {
    let file = export(
        &format!(
            "{}{}{}",
            note("C", Some("begin"), "rê"),
            rest(),
            note("E", Some("end"), "ves")
        ),
        ExportTarget::Ustx,
    );
    assert_eq!(ustx_lyrics(&file), vec!["rê", "ves"]);
}

/// Synthesizer V spells the syllable split `+` too, and it is the one marker the
/// two targets agree on. The hold they spell differently, and neither may be
/// swapped for the other.
#[test]
fn synthesizer_v_receives_the_same_word_and_the_marker_it_spells() {
    let file = export(
        &format!(
            "{}{}{}",
            note("C", Some("begin"), "mi"),
            note("D", Some("middle"), "nu"),
            note("E", Some("end"), "te")
        ),
        ExportTarget::Svp,
    );
    assert!(file.contains(r#""lyrics":"minute""#), "{file}");
    assert_eq!(file.matches(r#""lyrics":"+""#).count(), 2, "{file}");
}

/// TC-05 — the file is the same file on every platform. Verse writes UTF-8 with
/// no BOM and no carriage return, and the same source produces the same bytes,
/// so a Windows build and a macOS build cannot disagree about a lyric.
#[test]
fn the_written_file_is_byte_identical_and_free_of_platform_line_endings() {
    let source = format!(
        "{}{}{}",
        note("C", Some("begin"), "mê"),
        note("D", Some("end"), "me"),
        note("E", None, "si")
    );
    let first = export(&source, ExportTarget::Ustx);
    let second = export(&source, ExportTarget::Ustx);
    assert_eq!(first, second);
    assert!(!first.contains('\r'), "a carriage return reached the file");
    assert!(!first.starts_with('\u{feff}'), "a BOM reached the file");
    assert!(first.contains("lyric: \"même\""), "{first}");
}

/// The MuseScore adapter states the same binding in its own vocabulary, and a
/// `.mscz` must convert exactly like the `.musicxml` MuseScore exports from it.
#[test]
fn a_musescore_score_binds_its_syllables_the_same_way() {
    let mscx = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02"><Score><Division>480</Division>
<Part><Staff id="1"><StaffType group="pitched"/></Staff><trackName>Voix</trackName></Part>
<Staff id="1">
<Measure>
<voice>
<Chord><durationType>quarter</durationType>
<Lyrics><syllabic>begin</syllabic><text>mi</text></Lyrics>
<Note><pitch>60</pitch><tpc>14</tpc></Note></Chord>
<Chord><durationType>quarter</durationType>
<Lyrics><syllabic>middle</syllabic><text>nu</text></Lyrics>
<Note><pitch>62</pitch><tpc>16</tpc></Note></Chord>
<Chord><durationType>quarter</durationType>
<Lyrics><syllabic>end</syllabic><text>te</text></Lyrics>
<Note><pitch>64</pitch><tpc>18</tpc></Note></Chord>
</voice>
</Measure>
</Staff>
</Score></museScore>"#;
    let parsed = musescore::parse(mscx.as_bytes()).expect("the score parses");
    let outcome = convert_midi_with_target(&parsed, "french", None, ExportTarget::Ustx);
    assert!(outcome.ok, "conversion refused: {:?}", outcome.msg);
    let bytes = target::serialize_to(ExportTarget::Ustx, outcome.svp.as_ref().unwrap()).unwrap();
    let file = String::from_utf8(bytes).unwrap();
    assert_eq!(ustx_lyrics(&file), vec!["minute", "+", "+"]);
}

/// A Standard MIDI file states no `<syllabic>` — a hyphen inside the lyric text
/// is the only binding a MIDI exporter can write, and it is read the same way.
#[test]
fn a_standard_midi_file_binds_its_syllables_by_the_hyphen_it_writes() {
    let file = ustx_of(&smf(&[
        lyric(b"mi-"),
        quarter(60),
        lyric(b"nu-"),
        quarter(62),
        lyric(b"te"),
        quarter(64),
    ]));
    assert_eq!(ustx_lyrics(&file), vec!["minute", "+", "+"]);
}

/// The same file without hyphens states nothing to bind, and nothing is guessed
/// from the words themselves.
#[test]
fn a_standard_midi_file_without_hyphens_is_left_word_per_note() {
    let file = ustx_of(&smf(&[
        lyric(b"mi"),
        quarter(60),
        lyric(b"nu"),
        quarter(62),
        lyric(b"te"),
        quarter(64),
    ]));
    assert_eq!(ustx_lyrics(&file), vec!["mi", "nu", "te"]);
}

/// A MIDI melisma is not stated, so the wordless note stays untexted and the
/// word does not reach past it. `.mid` gains nothing it did not write.
#[test]
fn a_midi_wordless_note_never_binds_two_syllables() {
    let file = ustx_of(&smf(&[
        lyric(b"mi-"),
        quarter(60),
        quarter(62),
        lyric(b"te"),
        quarter(64),
    ]));
    assert_eq!(ustx_lyrics(&file), vec!["mi-", "te"]);
}

/// One `.mxl`: the score inside a container, named by `META-INF/container.xml`.
fn mxl(body: &str) -> Vec<u8> {
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file("META-INF/container.xml", options)
        .expect("container entry");
    writer
        .write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?><container><rootfiles>
               <rootfile full-path="score.musicxml"/></rootfiles></container>"#,
        )
        .expect("container written");
    writer
        .start_file("score.musicxml", options)
        .expect("score entry");
    writer
        .write_all(score(body).as_bytes())
        .expect("score written");
    writer.finish().expect("archive closed").into_inner()
}

/// A compressed MusicXML container carries the same `<syllabic>` as the raw
/// file, and the words it binds must survive the archive.
#[test]
fn a_compressed_mxl_container_binds_its_syllables() {
    let file = ustx_of(&mxl(&format!(
        "{}{}{}",
        note("C", Some("begin"), "mi"),
        note("D", Some("middle"), "nu"),
        note("E", Some("end"), "te")
    )));
    assert_eq!(ustx_lyrics(&file), vec!["minute", "+", "+"]);
}

/// Synthesizer V has to receive every shape OpenUtau does, in its own
/// vocabulary: the word whole, `+` for each following syllable, and `-` — never
/// `+~` — for the note the score sustains inside the word.
#[test]
fn synthesizer_v_receives_every_shape_in_its_own_vocabulary() {
    let file = export(
        &format!(
            "{}{}{}{}{}",
            note("C", Some("begin"), "rê"),
            untexted("D"),
            note("E", Some("end"), "ves"),
            note("F", None, "au-"),
            note("G", None, "tour")
        ),
        ExportTarget::Svp,
    );
    assert!(file.contains(r#""lyrics":"rêves""#), "{file}");
    assert!(file.contains(r#""lyrics":"autour""#), "{file}");
    assert_eq!(file.matches(r#""lyrics":"-""#).count(), 1, "{file}");
    assert_eq!(file.matches(r#""lyrics":"+""#).count(), 2, "{file}");
    assert!(!file.contains(r#""lyrics":"+~""#), "OpenUtau's hold leaked");
}

/// A rest inside the word is refused by both targets alike: Synthesizer V
/// checks nothing and would rebind the marker to whatever note precedes it.
#[test]
fn synthesizer_v_leaves_a_word_a_rest_separates_as_written() {
    let file = export(
        &format!(
            "{}{}{}",
            note("C", Some("begin"), "rê"),
            rest(),
            note("E", Some("end"), "ves")
        ),
        ExportTarget::Svp,
    );
    assert!(file.contains(r#""lyrics":"rê""#), "{file}");
    assert!(file.contains(r#""lyrics":"ves""#), "{file}");
    assert!(
        !file.contains(r#""lyrics":"+""#),
        "a marker crossed the rest"
    );
}
