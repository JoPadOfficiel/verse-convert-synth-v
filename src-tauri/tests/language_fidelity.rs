use verse_lib::engine::convert::convert_midi_with_target;
use verse_lib::engine::musicxml;
use verse_lib::engine::target::{self, ExportTarget};

fn score(words: &[&str]) -> String {
    let mut notes = String::new();
    for (i, w) in words.iter().enumerate() {
        notes.push_str(&format!(
            "<note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration>\
             <type>quarter</type><lyric><syllabic>single</syllabic><text>{}</text></lyric></note>",
            w.replace('&', "&amp;").replace('<', "&lt;")
        ));
        let _ = i;
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><score-partwise version=\"3.1\">\
         <part-list><score-part id=\"P1\"><part-name>Voix</part-name></score-part></part-list>\
         <part id=\"P1\"><measure number=\"1\"><attributes><divisions>1</divisions>\
         <time><beats>4</beats><beat-type>4</beat-type></time></attributes>{notes}</measure></part>\
         </score-partwise>"
    )
}

/// Lyrics are source text, not something a target interprets, so every language
/// works with no configuration. The `language` argument names a Synthesizer V
/// voice database and reaches exactly one field, `database.language` in the
/// `.svp`; the OpenUtau target never writes it at all. This passes a deliberately
/// WRONG language for the content to prove the text does not depend on it.
#[test]
fn latin_languages_survive_byte_exactly() {
    let sets: Vec<(&str, Vec<&str>)> = vec![
        ("FR", vec!["J'é", "tais", "là", "où"]),
        ("ES", vec!["¿Cor", "a", "zón", "ñ"]),
        ("EN", vec!["Help", "me", "if", "you"]),
        ("PT/DE", vec!["não", "für", "löschen", "ção"]),
        ("PL/TR", vec!["zażółć", "gęślą", "İstanbul", "ğüşiöç"]),
    ];
    for (tag, words) in &sets {
        let m = musicxml::parse(score(words).as_bytes()).expect("parses");
        // The language argument is deliberately WRONG for the content, to prove
        // that it is never consulted for the OpenUtau target.
        let o = convert_midi_with_target(&m, "japanese", None, ExportTarget::Ustx);
        assert!(o.ok, "{tag}: {:?}", o.msg);
        let bytes = target::serialize_to(ExportTarget::Ustx, o.svp.as_ref().unwrap()).unwrap();
        let text = String::from_utf8(bytes).expect("valid UTF-8");
        let mut missing = vec![];
        for w in words {
            if !text.contains(&format!("lyric: \"{w}\"")) {
                missing.push(*w);
            }
        }
        println!(
            "{tag:6} placed={} byte_exact={} missing={:?} lang_leaked={}",
            o.placed,
            missing.is_empty(),
            missing,
            text.contains("japanese")
        );
    }
}
