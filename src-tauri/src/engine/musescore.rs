//! Native MuseScore parser (.mscz = ZIP containing a .mscx, or raw .mscx).
//! Produces the same intermediate `Midi` structure as the other parsers.
//! Covers MuseScore 3.x / 4.x: Division, Part/Instrument/longName,
//! Staff/Measure/voice, TimeSig, Tempo, Chord (dots, tuplets, graces),
//! Rest (including full measures), location, lyrics (1st verse).
use crate::engine::midi::{unroll, Event, Jump, Kind, MeasureMarks, Midi};

pub fn is_musescore_xml(data: &[u8]) -> bool {
    let n = data.len().min(800);
    String::from_utf8_lossy(&data[..n]).contains("<museScore")
}

pub fn zip_has_mscx(data: &[u8]) -> bool {
    if let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(data)) {
        for i in 0..zip.len() {
            if let Ok(f) = zip.by_index(i) {
                if f.name().ends_with(".mscx") {
                    return true;
                }
            }
        }
    }
    false
}

pub fn parse(data: &[u8]) -> Result<Midi, String> {
    let xml = if data.len() >= 2 && &data[0..2] == b"PK" {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
        let name = (0..zip.len())
            .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|n| n.ends_with(".mscx"))
            .ok_or_else(|| "no .mscx in archive".to_string())?;
        let mut f = zip.by_name(&name).map_err(|e| e.to_string())?;
        crate::engine::musicxml::read_zip_entry_capped(&mut f)?
    } else {
        String::from_utf8_lossy(data).into_owned()
    };
    parse_mscx(&xml)
}

fn frac(s: &str) -> Option<(i64, i64)> {
    let mut it = s.trim().split('/');
    let a = it.next()?.trim().parse::<i64>().ok()?;
    let b = it.next()?.trim().parse::<i64>().ok()?;
    if b <= 0 {
        None
    } else {
        // anti-overflow bounds on forged data
        Some((a.clamp(-1_000_000, 1_000_000), b.min(1_000_000)))
    }
}

fn child<'a, 'b>(n: roxmltree::Node<'a, 'b>, tag: &str) -> Option<roxmltree::Node<'a, 'b>> {
    n.children().find(|c| c.has_tag_name(tag))
}

fn child_text<'a>(n: roxmltree::Node<'a, '_>, tag: &str) -> Option<&'a str> {
    child(n, tag).and_then(|c| c.text()).map(|t| t.trim())
}

/// Concatenated text of every descendant text node, skipping `<sym>` elements
/// (their content is a SMuFL glyph name like "space", not lyric text).
/// Lyrics `<text>` may embed formatting elements (`<font size=..>`, `<b>`,
/// `<i>`) around or between the words, so a plain first-child `.text()`
/// misses the syllable. End-trimmed only: control characters and stray
/// punctuation inside the syllable are cleaned downstream (clean_syllable).
fn deep_text(n: roxmltree::Node) -> String {
    fn walk(n: roxmltree::Node, out: &mut String) {
        for c in n.children() {
            if c.is_text() {
                out.push_str(c.text().unwrap_or(""));
            } else if !c.has_tag_name("sym") {
                walk(c, out);
            }
        }
    }
    let mut raw = String::new();
    walk(n, &mut raw);
    raw.trim().to_string()
}

/// Duration in ticks of a durationType (whole = 4 * div).
fn duration_ticks(kind: &str, div: i64) -> Option<i64> {
    let whole = 4 * div;
    Some(match kind {
        "long" => whole * 4,
        "breve" => whole * 2,
        "whole" => whole,
        "half" => whole / 2,
        "quarter" => whole / 4,
        "eighth" => whole / 8,
        "16th" => whole / 16,
        "32nd" => whole / 32,
        "64th" => whole / 64,
        "128th" => whole / 128,
        "256th" => (whole / 256).max(1),
        _ => return None, // "measure" handled separately
    })
}

fn apply_dots(dur: i64, dots: u32) -> i64 {
    // 1 dot: x1.5; 2 dots: x1.75; etc.
    let mut extra = 0i64;
    let mut half = dur;
    for _ in 0..dots.min(4) {
        half /= 2;
        extra += half;
    }
    dur + extra
}

fn is_grace(chord: roxmltree::Node) -> bool {
    chord.children().any(|c| {
        matches!(
            c.tag_name().name(),
            "acciaccatura" | "appoggiatura" | "grace4" | "grace8" | "grace16" | "grace32"
                | "grace8after" | "grace16after" | "grace32after"
        )
    })
}

/// Non-empty text of verse `verse` (0 = verse 1) of a Chord.
/// On the 2nd pass of a repeat (verse=1), falls back to verse 1 if verse 2
/// is absent (identical choruses).
fn verse_lyric(chord: roxmltree::Node, verse: u32) -> Option<String> {
    let find = |v: u32| -> Option<String> {
        chord
            .children()
            .filter(|c| c.has_tag_name("Lyrics"))
            .find(|ly| {
                child_text(*ly, "no").and_then(|t| t.parse::<u32>().ok()).unwrap_or(0) == v
            })
            .and_then(|ly| child(ly, "text"))
            .map(deep_text)
            .filter(|t| !t.is_empty())
    };
    find(verse).or_else(|| if verse > 0 { find(0) } else { None })
}

/// Playback order of the measures: repeats, voltas, D.S./D.C., Coda, Fine.
fn playback_order(measures: &[roxmltree::Node]) -> Vec<(usize, u32)> {
    let mut marks = vec![MeasureMarks::default(); measures.len()];
    let mut volta_spans: Vec<(usize, usize, Vec<u32>)> = Vec::new();

    for (i, m) in measures.iter().enumerate() {
        marks[i].start_repeat = m.children().any(|c| c.has_tag_name("startRepeat"));
        if let Some(er) = m.children().find(|c| c.has_tag_name("endRepeat")) {
            marks[i].end_repeat =
                er.text().and_then(|t| t.trim().parse::<u32>().ok()).unwrap_or(2).max(2);
        }
        for el in m.descendants().filter(|d| d.is_element()) {
            match el.tag_name().name() {
                "Marker" => {
                    let ty = child_text(el, "type").unwrap_or("");
                    let label = child_text(el, "label").unwrap_or("");
                    match ty {
                        "segno" | "varsegno" => marks[i].segno = true,
                        "codab" | "coda" | "varcoda" | "codetta" => marks[i].coda = true,
                        "toCoda" | "toCodaSym" => marks[i].to_coda = true,
                        "fine" => marks[i].fine = true,
                        _ => match label {
                            // MuseScore legacy: label "coda" = To Coda point,
                            // label "codab" = coda symbol (target)
                            "segno" => marks[i].segno = true,
                            "codab" => marks[i].coda = true,
                            "coda" => marks[i].to_coda = true,
                            "fine" => marks[i].fine = true,
                            _ => {}
                        },
                    }
                }
                "Jump" => {
                    let to = child_text(el, "jumpTo").unwrap_or("");
                    let until = child_text(el, "playUntil").unwrap_or("");
                    let ds = to.contains("segno");
                    marks[i].jump = Some(if until == "fine" {
                        if ds { Jump::DsAlFine } else { Jump::DcAlFine }
                    } else if until.contains("coda") {
                        if ds { Jump::DsAlCoda } else { Jump::DcAlCoda }
                    } else if ds {
                        Jump::Ds
                    } else {
                        Jump::Dc
                    });
                }
                "Spanner" if el.attribute("type") == Some("Volta") => {
                    if let Some(v) = el.children().find(|c| c.has_tag_name("Volta")) {
                        let endings: Vec<u32> = child_text(v, "endings")
                            .unwrap_or("1")
                            .split(|c: char| c == ',' || c.is_whitespace())
                            .filter_map(|s| s.trim().parse().ok())
                            .collect();
                        let span = el
                            .children()
                            .find(|c| c.has_tag_name("next"))
                            .and_then(|nx| nx.children().find(|c| c.has_tag_name("location")))
                            .and_then(|loc| child_text(loc, "measures"))
                            .and_then(|t| t.trim().parse::<usize>().ok())
                            .unwrap_or(1)
                            .max(1);
                        let endings = if endings.is_empty() { vec![1] } else { endings };
                        volta_spans.push((i, span, endings));
                    }
                }
                _ => {}
            }
        }
    }
    for (start, span, endings) in volta_spans {
        for k in start..(start + span).min(marks.len()) {
            marks[k].volta = Some(endings.clone());
        }
    }
    unroll(&marks)
}

pub fn parse_mscx(xml: &str) -> Result<Midi, String> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 5_000_000, // bounds the memory cost of a forged XML
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(xml, opts)
        .map_err(|e| format!("invalid XML: {}", e))?;
    let score = doc
        .descendants()
        .find(|n| n.has_tag_name("Score"))
        .ok_or_else(|| "MuseScore: Score element not found".to_string())?;
    let div = child_text(score, "Division")
        .and_then(|t| t.parse::<i64>().ok())
        .filter(|&d| d > 0 && d <= 1_000_000) // anti-overflow bound
        .unwrap_or(480);
    let tpb = div.clamp(1, 65535) as u16;

    // Track names: each Part (in order) owns N <Staff>;
    // top-level Staff ids are sequential -> table by order.
    let mut staff_names: Vec<String> = Vec::new();
    for part in score.children().filter(|n| n.has_tag_name("Part")) {
        let name = part
            .children()
            .find(|c| c.has_tag_name("Instrument"))
            .and_then(|i| child_text(i, "longName").map(str::to_string))
            .or_else(|| child_text(part, "trackName").map(str::to_string))
            .unwrap_or_default();
        let n_staves = part.children().filter(|c| c.has_tag_name("Staff")).count().max(1);
        for _ in 0..n_staves {
            staff_names.push(name.clone());
        }
    }

    let mut tracks: Vec<Vec<Event>> = Vec::new();
    let mut first_staff = true;

    for staff in score.children().filter(|n| n.has_tag_name("Staff")) {
        let idx = staff
            .attribute("id")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(tracks.len() + 1);
        let mut events: Vec<Event> = Vec::new();
        if let Some(name) = staff_names.get(idx.saturating_sub(1)) {
            if !name.is_empty() {
                events.push(Event { tick: 0, kind: Kind::TrackName(name.clone()) });
            }
        }

        let mut measure_start: i64 = 0;
        let mut measure_len: i64 = 4 * div; // 4/4 by default

        // Repeat unrolling: each measure is replayed at its pass, with the
        // matching verse (pass 1 -> verse 1, pass 2 -> verse 2).
        let measures: Vec<_> = staff.children().filter(|n| n.has_tag_name("Measure")).collect();
        for &(mi, pass) in playback_order(&measures).iter() {
            let measure = measures[mi];
            let mut this_len = measure_len;
            for voice in measure.children().filter(|n| n.has_tag_name("voice")) {
                let mut pos = measure_start;
                let mut tuplet: Option<(i64, i64)> = None; // (normal, actual)
                for el in voice.children().filter(|n| n.is_element()) {
                    match el.tag_name().name() {
                        "TimeSig" => {
                            let n = child_text(el, "sigN").and_then(|t| t.parse::<i64>().ok()).unwrap_or(4).clamp(1, 512);
                            let d = child_text(el, "sigD").and_then(|t| t.parse::<i64>().ok()).unwrap_or(4).clamp(1, 1024);
                            if d > 0 {
                                measure_len = 4 * div * n / d;
                                this_len = measure_len;
                            }
                            if first_staff && pass == 0 {
                                events.push(Event {
                                    tick: pos.max(0) as u32,
                                    kind: Kind::TimeSig {
                                        num: n.clamp(1, 255) as u8,
                                        den: d.clamp(1, 1024) as u16,
                                    },
                                });
                            }
                        }
                        "Tempo" => {
                            // <tempo> = quarter notes per second
                            if let Some(q) = child_text(el, "tempo").and_then(|t| t.parse::<f64>().ok()) {
                                if q > 0.0 && first_staff && pass == 0 {
                                    events.push(Event {
                                        tick: pos.max(0) as u32,
                                        kind: Kind::Tempo((1_000_000.0 / q) as u32),
                                    });
                                }
                            }
                        }
                        "Tuplet" => {
                            let n = child_text(el, "normalNotes").and_then(|t| t.parse::<i64>().ok());
                            let a = child_text(el, "actualNotes").and_then(|t| t.parse::<i64>().ok());
                            if let (Some(n), Some(a)) = (n, a) {
                                if (1..=64).contains(&n) && (1..=64).contains(&a) {
                                    tuplet = Some((n, a));
                                }
                            }
                        }
                        "endTuplet" => tuplet = None,
                        "location" => {
                            if let Some((a, b)) = child_text(el, "fractions").and_then(frac) {
                                pos += 4 * div * a / b;
                            }
                        }
                        "Chord" | "Rest" => {
                            let is_rest = el.has_tag_name("Rest");
                            let dtype = child_text(el, "durationType").unwrap_or("quarter");
                            let dots = child_text(el, "dots").and_then(|t| t.parse::<u32>().ok()).unwrap_or(0);
                            let mut dur = if dtype == "measure" {
                                child_text(el, "duration")
                                    .and_then(frac)
                                    .map(|(a, b)| 4 * div * a / b)
                                    .unwrap_or(this_len)
                            } else {
                                apply_dots(duration_ticks(dtype, div).unwrap_or(div), dots)
                            };
                            if let Some((n, a)) = tuplet {
                                dur = dur * n / a;
                            }
                            if dur <= 0 {
                                dur = 1;
                            }
                            if !is_rest {
                                if is_grace(el) {
                                    continue; // grace note: no duration on the grid
                                }
                                let on = pos.max(0) as u32;
                                let off = (pos + dur).max(pos + 1).max(0) as u32;
                                if let Some(txt) = verse_lyric(el, pass) {
                                    events.push(Event { tick: on, kind: Kind::Lyrics(txt) });
                                }
                                for note in el.children().filter(|c| c.has_tag_name("Note")) {
                                    if let Some(p) = child_text(note, "pitch").and_then(|t| t.parse::<i64>().ok()) {
                                        let pitch = p.clamp(0, 127) as u8;
                                        events.push(Event { tick: on, kind: Kind::NoteOn(pitch) });
                                        events.push(Event { tick: off, kind: Kind::NoteOff(pitch) });
                                    }
                                }
                            }
                            pos += dur;
                        }
                        _ => {}
                    }
                }
            }
            // irregular measure (anacrusis): len="a/b" attribute
            if let Some((a, b)) = measure.attribute("len").and_then(frac) {
                this_len = 4 * div * a / b;
            }
            measure_start += this_len;
        }

        events.sort_by_key(|e| e.tick);
        if events.iter().any(|e| matches!(e.kind, Kind::NoteOn(_) | Kind::Lyrics(_))) {
            tracks.push(events);
        }
        first_staff = false;
    }

    if tracks.is_empty() {
        return Err("no usable staff in the MuseScore file".into());
    }
    Ok(Midi { ticks_per_beat: tpb, tracks })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mscx(lyric_text_xml: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <trackName>Soprano</trackName>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Lyrics>
              {}
            </Lyrics>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#,
            lyric_text_xml
        )
    }

    fn lyrics_of(midi: &Midi) -> Vec<String> {
        midi.tracks
            .iter()
            .flat_map(|t| t.iter())
            .filter_map(|e| match &e.kind {
                Kind::Lyrics(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn plain_lyric_text() {
        let midi = parse_mscx(&mscx("<text>let</text>")).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn lyric_text_with_leading_font_elements() {
        // MuseScore stores styled lyrics as <font .../> elements inside <text>;
        // the syllable is a text node placed after them.
        let midi = parse_mscx(&mscx(
            r#"<text><font size="9.2"></font><font face="Arial"></font>let</text>"#,
        ))
        .unwrap();
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn lyric_text_interleaved_with_formatting() {
        let midi =
            parse_mscx(&mscx(r#"<text>shi<font face="Arial"></font>ne,</text>"#)).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["shine,"]);
    }

    #[test]
    fn empty_formatted_lyric_is_skipped() {
        let midi = parse_mscx(&mscx(r#"<text><font size="9.2"></font></text>"#)).unwrap();
        assert!(lyrics_of(&midi).is_empty());
    }

    #[test]
    fn sym_glyph_name_is_not_injected() {
        // <sym> holds a SMuFL glyph identifier, not renderable lyric text.
        let midi = parse_mscx(&mscx(r#"<text>a<sym>space</sym>b</text>"#)).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["ab"]);
    }

    #[test]
    fn pretty_printed_text_is_trimmed() {
        let midi = parse_mscx(&mscx("<text>\n  <font size=\"9.2\"></font>\n  let\n</text>"))
            .unwrap();
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn xml_entities_are_decoded() {
        let midi = parse_mscx(&mscx("<text>rock &amp; roll</text>")).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["rock & roll"]);
    }
}
