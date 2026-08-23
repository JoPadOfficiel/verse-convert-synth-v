//! The syllable-per-note notation every vocal score uses, end to end.
//!
//! A score writes one syllable under each note and binds the syllables of one
//! word — with `<syllabic>` in MusicXML and MuseScore, with a hyphen inside the
//! text in the files a MIDI exporter writes. Both targets want the word whole on
//! the first note of its run and a `+` on the notes that follow, which is what
//! OpenUtau's own MusicXML importer writes (`Format/MusicXML.cs`), because a
//! phonemizer looks the lyric up in a pronunciation dictionary: `"mê"` and
//! `"me"` are not the word `"même"` under any reading.

use verse_lib::engine::convert::convert_midi_with_target;
use verse_lib::engine::target::ExportTarget;
use verse_lib::engine::{musescore, musicxml, target};

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
