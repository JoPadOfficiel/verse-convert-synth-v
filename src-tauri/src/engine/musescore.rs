//! Native MuseScore parser (.mscz = ZIP containing a .mscx, or raw .mscx).
//! Produces the same intermediate `Midi` structure as the other parsers.
//! Covers MuseScore 3.x / 4.x: Division, Part/Instrument/longName,
//! Staff/Measure/voice, TimeSig, Tempo, Chord (dots, tuplets, graces),
//! Rest (including full measures), location, lyrics (1st verse).
use crate::engine::midi::{
    unroll, Event, InstrumentInfo, Jump, Kind, Lyric, LyricFragment, LyricState, MeasureMarks,
    Midi, MidiTextProfile, NoteOff, NoteOn, NoteSource, SourceFormat, Syllabic, TimeBase, Track,
    TrackRoleHint, TrackSource,
};
use std::collections::BTreeMap;

pub fn is_musescore_xml(data: &[u8]) -> bool {
    crate::engine::musicxml::xml_bytes_contain_ascii(data, b"<museScore")
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
        let mut zip =
            zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
        let name = (0..zip.len())
            .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|n| n.ends_with(".mscx"))
            .ok_or_else(|| "no .mscx in archive".to_string())?;
        let mut f = zip.by_name(&name).map_err(|e| e.to_string())?;
        crate::engine::musicxml::read_zip_entry_capped(&mut f)?
    } else {
        crate::engine::musicxml::decode_xml_bytes(data)?
    };
    parse_mscx(&xml)
}

fn frac(s: &str) -> Option<(i64, i64)> {
    let mut it = s.trim().split('/');
    let a = it.next()?.trim().parse::<i64>().ok()?;
    let b = it.next()?.trim().parse::<i64>().ok()?;
    if it.next().is_some() || b <= 0 || a.unsigned_abs() > 1_000_000 || b > 1_000_000 {
        None
    } else {
        Some((a, b))
    }
}

fn child<'a, 'b>(n: roxmltree::Node<'a, 'b>, tag: &str) -> Option<roxmltree::Node<'a, 'b>> {
    n.children().find(|c| c.has_tag_name(tag))
}

fn child_text<'a>(n: roxmltree::Node<'a, '_>, tag: &str) -> Option<&'a str> {
    child(n, tag).and_then(|c| c.text()).map(|t| t.trim())
}

/// Raw concatenation of every descendant text node, skipping `<sym>` elements
/// (their content is a SMuFL glyph name like "space", not lyric text) and
/// turning `<br/>` line breaks into spaces so adjacent words never fuse.
/// Rich text (`<text>`, names) may embed formatting elements (`<font size=..>`,
/// `<b>`, `<i>`, `<u>`, `<sup>`, `<sub>`, ...) around or between the words, so
/// a plain first-child `.text()` misses the content.
pub(crate) fn deep_text_raw(n: roxmltree::Node, out: &mut String) {
    for c in n.children() {
        if c.is_text() {
            out.push_str(c.text().unwrap_or(""));
        } else if c.has_tag_name("br") {
            out.push(' ');
        } else if !c.has_tag_name("sym") {
            deep_text_raw(c, out);
        }
    }
}

/// `deep_text_raw`, end-trimmed. Control characters and stray punctuation
/// inside the text are cleaned downstream (clean_syllable for lyrics,
/// collapse_ws for names).
pub(crate) fn deep_text(n: roxmltree::Node) -> String {
    let mut raw = String::new();
    deep_text_raw(n, &mut raw);
    raw.trim().to_string()
}

/// Collapses every whitespace run (spaces, tabs, newlines) into one space.
/// Used for display names, where a two-line MuseScore label must become a
/// single readable line.
pub(crate) fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Duration as a rational number of quarter notes.
fn duration_ratio(kind: &str) -> Option<(i64, i64)> {
    Some(match kind {
        "long" => (16, 1),
        "breve" => (8, 1),
        "whole" => (4, 1),
        "half" => (2, 1),
        "quarter" => (1, 1),
        "eighth" => (1, 2),
        "16th" => (1, 4),
        "32nd" => (1, 8),
        "64th" => (1, 16),
        "128th" => (1, 32),
        "256th" => (1, 64),
        _ => return None, // "measure" handled separately
    })
}

fn checked_ratio_mul(
    ratio: (i64, i64),
    numerator: i64,
    denominator: i64,
    context: &str,
) -> Result<(i64, i64), String> {
    if denominator <= 0 {
        return Err(format!("{context} has a non-positive denominator"));
    }
    let numerator = i128::from(ratio.0)
        .checked_mul(i128::from(numerator))
        .ok_or_else(|| format!("{context} numerator overflow"))?;
    let denominator = i128::from(ratio.1)
        .checked_mul(i128::from(denominator))
        .ok_or_else(|| format!("{context} denominator overflow"))?;
    let numerator =
        i64::try_from(numerator).map_err(|_| format!("{context} numerator overflow"))?;
    let denominator =
        i64::try_from(denominator).map_err(|_| format!("{context} denominator overflow"))?;
    let divisor = gcd_i64(numerator.unsigned_abs(), denominator as u64);
    Ok((
        numerator / i64::try_from(divisor).unwrap_or(1),
        denominator / i64::try_from(divisor).unwrap_or(1),
    ))
}

fn dotted_ratio(ratio: (i64, i64), dots: u32) -> Result<(i64, i64), String> {
    let denominator = 1i64
        .checked_shl(dots)
        .ok_or_else(|| "MuseScore dot denominator overflow".to_string())?;
    let numerator = denominator
        .checked_mul(2)
        .and_then(|value| value.checked_sub(1))
        .ok_or_else(|| "MuseScore dot numerator overflow".to_string())?;
    checked_ratio_mul(ratio, numerator, denominator, "MuseScore dotted duration")
}

fn gcd_i64(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn include_tick_ratio(
    scale: &mut i64,
    source_division: i64,
    ratio: (i64, i64),
    context: &str,
) -> Result<(), String> {
    if ratio.1 <= 0 {
        return Err(format!("{context} has a non-positive denominator"));
    }
    let base_numerator = i128::from(source_division)
        .checked_mul(i128::from(ratio.0))
        .ok_or_else(|| format!("{context} timing overflow"))?;
    let base_abs =
        u64::try_from(base_numerator.abs()).map_err(|_| format!("{context} timing overflow"))?;
    let denominator =
        u64::try_from(ratio.1).map_err(|_| format!("{context} denominator overflow"))?;
    let required = denominator / gcd_i64(base_abs, denominator);
    let current = u64::try_from(*scale).map_err(|_| "MuseScore tick scale overflow")?;
    let next = current
        .checked_div(gcd_i64(current, required))
        .and_then(|value| value.checked_mul(required))
        .ok_or_else(|| "MuseScore exact tick scale overflow".to_string())?;
    let next = i64::try_from(next).map_err(|_| "MuseScore exact tick scale overflow")?;
    let ticks_per_beat = source_division
        .checked_mul(next)
        .filter(|value| *value <= i64::from(u16::MAX))
        .ok_or_else(|| {
            format!("{context} requires a tick division beyond the supported exact range")
        })?;
    *scale = next;
    debug_assert!(ticks_per_beat > 0);
    Ok(())
}

fn exact_ticks(division: i64, ratio: (i64, i64), context: &str) -> Result<i64, String> {
    if ratio.1 <= 0 {
        return Err(format!("{context} has a non-positive denominator"));
    }
    let numerator = i128::from(division)
        .checked_mul(i128::from(ratio.0))
        .ok_or_else(|| format!("{context} timing overflow"))?;
    let denominator = i128::from(ratio.1);
    if numerator % denominator != 0 {
        return Err(format!(
            "{context} cannot be represented exactly at division {division}"
        ));
    }
    i64::try_from(numerator / denominator).map_err(|_| format!("{context} timing overflow"))
}

fn musescore_tick_scale(score: roxmltree::Node, source_division: i64) -> Result<i64, String> {
    let mut scale = 1i64;

    for measure in score
        .descendants()
        .filter(|node| node.has_tag_name("Measure"))
    {
        if let Some(value) = measure.attribute("len") {
            let (numerator, denominator) = frac(value)
                .ok_or_else(|| format!("MuseScore measure len fraction is invalid: {value:?}"))?;
            include_tick_ratio(
                &mut scale,
                source_division,
                (
                    4i64.checked_mul(numerator)
                        .ok_or_else(|| "MuseScore measure len numerator overflow".to_string())?,
                    denominator,
                ),
                "MuseScore measure len",
            )?;
        }
    }

    for voice in score
        .descendants()
        .filter(|node| node.has_tag_name("voice"))
    {
        let mut tuplet: Option<(i64, i64)> = None;
        for element in voice.children().filter(|node| node.is_element()) {
            match element.tag_name().name() {
                "TimeSig" => {
                    let numerator = child_text(element, "sigN")
                        .and_then(|value| value.parse::<i64>().ok())
                        .filter(|value| *value > 0)
                        .ok_or_else(|| "MuseScore TimeSig has an invalid sigN".to_string())?;
                    let denominator = child_text(element, "sigD")
                        .and_then(|value| value.parse::<i64>().ok())
                        .filter(|value| *value > 0)
                        .ok_or_else(|| "MuseScore TimeSig has an invalid sigD".to_string())?;
                    include_tick_ratio(
                        &mut scale,
                        source_division,
                        (
                            4i64.checked_mul(numerator).ok_or_else(|| {
                                "MuseScore time-signature numerator overflow".to_string()
                            })?,
                            denominator,
                        ),
                        "MuseScore time signature",
                    )?;
                }
                "Tuplet" => {
                    let normal = child_text(element, "normalNotes")
                        .and_then(|value| value.parse::<i64>().ok())
                        .filter(|value| (1..=64).contains(value))
                        .ok_or_else(|| "MuseScore Tuplet has invalid normalNotes".to_string())?;
                    let actual = child_text(element, "actualNotes")
                        .and_then(|value| value.parse::<i64>().ok())
                        .filter(|value| (1..=64).contains(value))
                        .ok_or_else(|| "MuseScore Tuplet has invalid actualNotes".to_string())?;
                    tuplet = Some((normal, actual));
                }
                "endTuplet" => tuplet = None,
                "location" => {
                    let text = child_text(element, "fractions")
                        .ok_or_else(|| "MuseScore location is missing fractions".to_string())?;
                    let (numerator, denominator) = frac(text).ok_or_else(|| {
                        format!("MuseScore location fraction is invalid: {text:?}")
                    })?;
                    include_tick_ratio(
                        &mut scale,
                        source_division,
                        (
                            4i64.checked_mul(numerator).ok_or_else(|| {
                                "MuseScore location numerator overflow".to_string()
                            })?,
                            denominator,
                        ),
                        "MuseScore location",
                    )?;
                }
                "Chord" | "Rest" if !is_grace(element) => {
                    let duration_type = child_text(element, "durationType").ok_or_else(|| {
                        format!(
                            "MuseScore {} is missing durationType",
                            element.tag_name().name()
                        )
                    })?;
                    let ratio = if duration_type == "measure" {
                        match child_text(element, "duration") {
                            Some(value) => {
                                let (numerator, denominator) = frac(value).ok_or_else(|| {
                                    format!("MuseScore measure duration is invalid: {value:?}")
                                })?;
                                Some((
                                    4i64.checked_mul(numerator).ok_or_else(|| {
                                        "MuseScore measure duration numerator overflow".to_string()
                                    })?,
                                    denominator,
                                ))
                            }
                            None => None,
                        }
                    } else {
                        let dots = child_text(element, "dots")
                            .map(|value| value.parse::<u32>())
                            .transpose()
                            .map_err(|_| "MuseScore dots value is invalid".to_string())?
                            .unwrap_or(0);
                        if dots > 4 {
                            return Err("MuseScore dots value is invalid".into());
                        }
                        Some(dotted_ratio(
                            duration_ratio(duration_type).ok_or_else(|| {
                                format!("MuseScore durationType is unsupported: {duration_type:?}")
                            })?,
                            dots,
                        )?)
                    };
                    if let Some(mut ratio) = ratio {
                        include_tick_ratio(
                            &mut scale,
                            source_division,
                            ratio,
                            "MuseScore base duration",
                        )?;
                        if let Some((normal, actual)) = tuplet {
                            ratio = checked_ratio_mul(
                                ratio,
                                normal,
                                actual,
                                "MuseScore tuplet duration",
                            )?;
                        }
                        include_tick_ratio(
                            &mut scale,
                            source_division,
                            ratio,
                            "MuseScore note duration",
                        )?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(scale)
}

fn is_grace(chord: roxmltree::Node) -> bool {
    chord.children().any(|c| {
        matches!(
            c.tag_name().name(),
            "acciaccatura"
                | "appoggiatura"
                | "grace4"
                | "grace8"
                | "grace16"
                | "grace32"
                | "grace8after"
                | "grace16after"
                | "grace32after"
        )
    })
}

/// Every lyric lane owned by a MuseScore chord. Selection for a repeat pass is
/// deferred to the SVP projector so no source verse is discarded here.
fn chord_lyrics(
    chord: roxmltree::Node,
    source_id: &str,
    tick_scale: i64,
) -> Result<Vec<Lyric>, String> {
    chord
        .children()
        .filter(|child| child.has_tag_name("Lyrics"))
        .enumerate()
        .map(|(index, lyric_node)| {
            let zero_based = match child_text(lyric_node, "no") {
                Some(text) => text
                    .parse::<u32>()
                    .map_err(|_| format!("MuseScore lyric lane number is invalid: {text:?}"))?,
                None => u32::try_from(index)
                    .map_err(|_| "MuseScore lyric lane index exceeds the supported range")?,
            };
            let verse = zero_based
                .checked_add(1)
                .ok_or_else(|| "MuseScore lyric lane number overflow".to_string())?;
            let mut raw = String::new();
            if let Some(text_node) = child(lyric_node, "text") {
                deep_text_raw(text_node, &mut raw);
            }
            // MuseScore indents formatted lyric XML. The projection trims only
            // outer formatting whitespace; `raw` and `fragments` retain the
            // decoded source text verbatim.
            let projected = raw.trim().to_string();
            let state = if projected.is_empty() {
                LyricState::ExplicitEmpty
            } else {
                LyricState::Text(projected)
            };
            let syllabic = match child_text(lyric_node, "syllabic") {
                Some("single") => Some(Syllabic::Single),
                Some("begin") => Some(Syllabic::Begin),
                Some("middle") => Some(Syllabic::Middle),
                Some("end") => Some(Syllabic::End),
                _ => None,
            };
            let extend_ticks = match child_text(lyric_node, "ticks") {
                Some(text) => Some(
                    text.parse::<i64>()
                        .map_err(|_| {
                            format!("MuseScore lyric extension ticks are invalid: {text:?}")
                        })?
                        .checked_mul(tick_scale)
                        .ok_or_else(|| {
                            "MuseScore lyric extension ticks overflow after exact scaling"
                                .to_string()
                        })?,
                ),
                None => None,
            };
            let extend_fraction = match child_text(lyric_node, "ticks_f") {
                Some(text) => Some(frac(text).ok_or_else(|| {
                    format!("MuseScore lyric extension fraction is invalid: {text:?}")
                })?),
                None => None,
            };
            Ok(Lyric {
                id: format!("{source_id}-lyric-{index}"),
                raw: raw.clone(),
                raw_bytes: Vec::new(),
                fragments: vec![LyricFragment::Text(raw)],
                lane: verse.to_string(),
                verse,
                state,
                syllabic,
                line_break: None,
                time_only: Vec::new(),
                extension: None,
                extend_ticks,
                extend_fraction,
            })
        })
        .collect()
}

/// Playback order of the measures: repeats, voltas, D.S./D.C., Coda, Fine.
fn playback_order(measures: &[roxmltree::Node]) -> Result<Vec<(usize, u32)>, String> {
    let mut marks = vec![MeasureMarks::default(); measures.len()];
    let mut volta_spans: Vec<(usize, usize, Vec<u32>)> = Vec::new();

    for (i, m) in measures.iter().enumerate() {
        marks[i].start_repeat = m.children().any(|c| c.has_tag_name("startRepeat"));
        if let Some(er) = m.children().find(|c| c.has_tag_name("endRepeat")) {
            marks[i].end_repeat = er
                .text()
                .and_then(|t| t.trim().parse::<u32>().ok())
                .unwrap_or(2)
                .max(2);
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
                        if ds {
                            Jump::DsAlFine
                        } else {
                            Jump::DcAlFine
                        }
                    } else if until.contains("coda") {
                        if ds {
                            Jump::DsAlCoda
                        } else {
                            Jump::DcAlCoda
                        }
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
    crate::engine::musicxml::check_nesting(xml)?;
    let opts = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 5_000_000, // bounds the memory cost of a forged XML
    };
    let doc = roxmltree::Document::parse_with_options(xml, opts)
        .map_err(|e| format!("invalid XML: {}", e))?;
    let score = doc
        .descendants()
        .find(|n| n.has_tag_name("Score"))
        .ok_or_else(|| "MuseScore: Score element not found".to_string())?;
    let source_division = match child_text(score, "Division") {
        Some(value) => value
            .parse::<i64>()
            .ok()
            .filter(|division| (1..=i64::from(u16::MAX)).contains(division))
            .ok_or_else(|| format!("MuseScore Division is invalid: {value:?}"))?,
        // MuseScore's documented default tick division when the element is
        // absent. A present malformed value is never replaced.
        None => 480,
    };
    let tick_scale = musescore_tick_scale(score, source_division)?;
    let div = source_division
        .checked_mul(tick_scale)
        .ok_or_else(|| "MuseScore exact tick division overflow".to_string())?;
    let tpb = u16::try_from(div).map_err(|_| "MuseScore Division exceeds the SVP time base")?;

    #[derive(Clone, Debug, Default)]
    struct StaffInfo {
        part_id: String,
        name: String,
        role: TrackRoleHint,
        instruments: Vec<InstrumentInfo>,
    }

    let top_level_staff_ids: Vec<String> = score
        .children()
        .filter(|node| node.has_tag_name("Staff"))
        .filter_map(|staff| staff.attribute("id").map(str::to_string))
        .collect();
    let mut staff_cursor = 0usize;
    let mut staff_info: BTreeMap<String, StaffInfo> = BTreeMap::new();
    for (part_index, part) in score
        .children()
        .filter(|n| n.has_tag_name("Part"))
        .enumerate()
    {
        let part_id = part
            .attribute("id")
            .map(|value| format!("musescore-part-{value}"))
            .unwrap_or_else(|| format!("musescore-part-{part_index}"));
        let name = part
            .children()
            .find(|c| c.has_tag_name("Instrument"))
            .and_then(|i| child(i, "longName").map(|n| collapse_ws(&deep_text(n))))
            .filter(|s| !s.is_empty())
            .or_else(|| {
                child(part, "trackName")
                    .map(|n| collapse_ws(&deep_text(n)))
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default();
        let instrument_node = part.children().find(|c| c.has_tag_name("Instrument"));
        let mut instruments = Vec::new();
        if let Some(instrument_node) = instrument_node {
            let id = instrument_node
                .attribute("id")
                .map(str::to_string)
                .or_else(|| child_text(instrument_node, "instrumentId").map(str::to_string));
            let instrument_name = child(instrument_node, "longName")
                .map(|node| collapse_ws(&deep_text(node)))
                .filter(|value| !value.is_empty())
                .or_else(|| child_text(instrument_node, "trackName").map(str::to_string));
            let percussion = child_text(instrument_node, "useDrumset") == Some("1")
                || instrument_node
                    .descendants()
                    .any(|node| node.has_tag_name("Drum"));
            let channels: Vec<_> = instrument_node
                .children()
                .filter(|node| node.has_tag_name("Channel"))
                .collect();
            if channels.is_empty() {
                instruments.push(InstrumentInfo {
                    id,
                    name: instrument_name,
                    percussion,
                    ..InstrumentInfo::default()
                });
            } else {
                for (channel_index, channel_node) in channels.into_iter().enumerate() {
                    let source_channel = channel_node
                        .attribute("channel")
                        .and_then(|value| value.parse::<i32>().ok())
                        .or_else(|| {
                            child_text(channel_node, "channel")
                                .and_then(|value| value.parse::<i32>().ok())
                        });
                    let source_program = child(channel_node, "program")
                        .and_then(|program| program.attribute("value"))
                        .and_then(|value| value.parse::<i32>().ok());
                    let controllers: Vec<(u8, u8)> = channel_node
                        .children()
                        .filter(|node| node.has_tag_name("controller"))
                        .filter_map(|node| {
                            let controller = node.attribute("ctrl")?.parse::<u8>().ok()?;
                            let value = node.attribute("value")?.parse::<u8>().ok()?;
                            Some((controller, value))
                        })
                        .collect();
                    let controller = |number| {
                        controllers
                            .iter()
                            .find_map(|&(key, value)| (key == number).then_some(value))
                    };
                    instruments.push(InstrumentInfo {
                        id: id
                            .clone()
                            .map(|value| format!("{value}:channel:{channel_index}")),
                        name: instrument_name.clone(),
                        source_channel,
                        source_program,
                        channel: source_channel.and_then(|value| u8::try_from(value).ok()),
                        program: source_program.and_then(|value| u8::try_from(value).ok()),
                        bank_msb: controller(0),
                        bank_lsb: controller(32),
                        volume: controller(7).map(f64::from),
                        pan: controller(10).map(f64::from),
                        controllers,
                        percussion,
                        ..InstrumentInfo::default()
                    });
                }
            }
        }
        let part_staves: Vec<_> = part
            .children()
            .filter(|child| child.has_tag_name("Staff"))
            .collect();
        for (staff_index, staff) in part_staves.iter().copied().enumerate() {
            let staff_id = staff
                .attribute("id")
                .map(str::to_string)
                .or_else(|| top_level_staff_ids.get(staff_cursor).cloned())
                .or_else(|| {
                    (part_staves.len() == 1)
                        .then(|| part.attribute("id"))
                        .flatten()
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("{}-{}", part_index + 1, staff_index + 1));
            staff_cursor += 1;
            let group = staff
                .children()
                .find(|node| node.has_tag_name("StaffType"))
                .and_then(|node| node.attribute("group"));
            let percussion = matches!(group, Some("percussion" | "unpitched"))
                || instruments.iter().any(|instrument| instrument.percussion);
            staff_info.insert(
                staff_id,
                StaffInfo {
                    part_id: part_id.clone(),
                    name: name.clone(),
                    role: if percussion {
                        TrackRoleHint::Percussion
                    } else {
                        TrackRoleHint::Ambiguous
                    },
                    instruments: instruments.clone(),
                },
            );
        }
    }

    let mut tracks = Vec::new();
    let mut global_events = Vec::new();

    for staff in score.children().filter(|n| n.has_tag_name("Staff")) {
        let staff_id = staff
            .attribute("id")
            .map(str::to_string)
            .unwrap_or_else(|| format!("anonymous-{}", tracks.len() + 1));
        let info = staff_info.get(&staff_id).cloned().unwrap_or_default();
        let mut voice_events: BTreeMap<(usize, Option<usize>), Vec<Event>> = BTreeMap::new();
        let mut unassigned_chord_lyrics = Vec::new();

        let mut measure_start: i64 = 0;
        let mut measure_len: i64 = 4 * div; // 4/4 by default

        let measures: Vec<_> = staff
            .children()
            .filter(|n| n.has_tag_name("Measure"))
            .collect();
        for &(mi, pass) in playback_order(&measures)?.iter() {
            let measure = measures[mi];
            let mut this_len = measure_len;
            for (voice_index, voice) in measure
                .children()
                .filter(|n| n.has_tag_name("voice"))
                .enumerate()
            {
                let mut pos = measure_start;
                let mut tuplet: Option<(i64, i64)> = None; // (normal, actual)
                for (element_index, el) in voice.children().filter(|n| n.is_element()).enumerate() {
                    match el.tag_name().name() {
                        "TimeSig" => {
                            let numerator_text = child_text(el, "sigN")
                                .ok_or_else(|| "MuseScore TimeSig is missing sigN".to_string())?;
                            let denominator_text = child_text(el, "sigD")
                                .ok_or_else(|| "MuseScore TimeSig is missing sigD".to_string())?;
                            let numerator = numerator_text
                                .parse::<i64>()
                                .ok()
                                .and_then(|value| u8::try_from(value).ok())
                                .filter(|value| *value > 0)
                                .ok_or_else(|| {
                                    format!(
                                        "MuseScore time-signature numerator is invalid: {numerator_text:?}"
                                    )
                                })?;
                            let denominator = denominator_text
                                .parse::<i64>()
                                .ok()
                                .and_then(|value| u16::try_from(value).ok())
                                .filter(|value| *value > 0)
                                .ok_or_else(|| {
                                    format!(
                                        "MuseScore time-signature denominator is invalid: {denominator_text:?}"
                                    )
                                })?;
                            measure_len = exact_ticks(
                                div,
                                (
                                    4i64.checked_mul(i64::from(numerator)).ok_or_else(|| {
                                        "MuseScore time-signature numerator overflow".to_string()
                                    })?,
                                    i64::from(denominator),
                                ),
                                "MuseScore time-signature duration",
                            )?;
                            if measure_len <= 0 {
                                return Err(
                                    "MuseScore time-signature duration is non-positive".into()
                                );
                            }
                            this_len = measure_len;
                            push_global_event(
                                &mut global_events,
                                checked_score_tick(pos)?,
                                Kind::TimeSig {
                                    num: numerator,
                                    den: denominator,
                                    clocks_per_click: None,
                                    notated_32nds: None,
                                },
                            );
                        }
                        "Tempo" => {
                            // <tempo> = quarter notes per second
                            let tempo_text = child_text(el, "tempo")
                                .ok_or_else(|| "MuseScore Tempo is missing tempo".to_string())?;
                            let quarters_per_second = tempo_text
                                .parse::<f64>()
                                .ok()
                                .filter(|value| value.is_finite() && *value > 0.0);
                            let micros = quarters_per_second
                                .map(|value| (1_000_000.0 / value).round())
                                .filter(|value| (1.0..=f64::from(u32::MAX)).contains(value))
                                .map(|value| value as u32)
                                .ok_or_else(|| {
                                    format!("MuseScore tempo is invalid: {tempo_text:?}")
                                })?;
                            push_global_event(
                                &mut global_events,
                                checked_score_tick(pos)?,
                                Kind::Tempo(micros),
                            );
                        }
                        "Tuplet" => {
                            let normal_text = child_text(el, "normalNotes").ok_or_else(|| {
                                "MuseScore Tuplet is missing normalNotes".to_string()
                            })?;
                            let actual_text = child_text(el, "actualNotes").ok_or_else(|| {
                                "MuseScore Tuplet is missing actualNotes".to_string()
                            })?;
                            let normal = normal_text
                                .parse::<i64>()
                                .ok()
                                .filter(|value| (1..=64).contains(value))
                                .ok_or_else(|| {
                                    format!(
                                        "MuseScore Tuplet normalNotes is invalid: {normal_text:?}"
                                    )
                                })?;
                            let actual = actual_text
                                .parse::<i64>()
                                .ok()
                                .filter(|value| (1..=64).contains(value))
                                .ok_or_else(|| {
                                    format!(
                                        "MuseScore Tuplet actualNotes is invalid: {actual_text:?}"
                                    )
                                })?;
                            tuplet = Some((normal, actual));
                        }
                        "endTuplet" => tuplet = None,
                        "location" => {
                            let fraction_text = child_text(el, "fractions").ok_or_else(|| {
                                "MuseScore location is missing fractions".to_string()
                            })?;
                            let (numerator, denominator) =
                                frac(fraction_text).ok_or_else(|| {
                                    format!(
                                        "MuseScore location fraction is invalid: {fraction_text:?}"
                                    )
                                })?;
                            let delta = exact_ticks(
                                div,
                                (
                                    4i64.checked_mul(numerator).ok_or_else(|| {
                                        "MuseScore location numerator overflow".to_string()
                                    })?,
                                    denominator,
                                ),
                                "MuseScore location",
                            )?;
                            pos = pos
                                .checked_add(delta)
                                .ok_or_else(|| "MuseScore cursor overflow".to_string())?;
                        }
                        "Chord" | "Rest" => {
                            let is_rest = el.has_tag_name("Rest");
                            let grace = !is_rest && is_grace(el);
                            let mut dur = if grace {
                                0
                            } else {
                                let duration_type =
                                    child_text(el, "durationType").ok_or_else(|| {
                                        format!(
                                            "MuseScore {} is missing durationType",
                                            if is_rest { "Rest" } else { "Chord" }
                                        )
                                    })?;
                                let dots = match child_text(el, "dots") {
                                    Some(value) => value
                                        .parse::<u32>()
                                        .ok()
                                        .filter(|dots| *dots <= 4)
                                        .ok_or_else(|| {
                                            format!("MuseScore dots value is invalid: {value:?}")
                                        })?,
                                    None => 0,
                                };
                                if duration_type == "measure" {
                                    match child_text(el, "duration") {
                                        Some(value) => {
                                            let (numerator, denominator) =
                                                frac(value).ok_or_else(|| {
                                                    format!(
                                                        "MuseScore measure duration is invalid: {value:?}"
                                                    )
                                                })?;
                                            exact_ticks(
                                                div,
                                                (
                                                    4i64.checked_mul(numerator).ok_or_else(
                                                        || {
                                                            "MuseScore measure duration numerator overflow"
                                                                .to_string()
                                                        },
                                                    )?,
                                                    denominator,
                                                ),
                                                "MuseScore measure duration",
                                            )?
                                        }
                                        None => this_len,
                                    }
                                } else {
                                    let ratio = dotted_ratio(
                                        duration_ratio(duration_type).ok_or_else(|| {
                                            format!(
                                                "MuseScore durationType is unsupported: {duration_type:?}"
                                            )
                                        })?,
                                        dots,
                                    )?;
                                    exact_ticks(div, ratio, "MuseScore note duration")?
                                }
                            };
                            if let Some((n, a)) = tuplet {
                                dur = exact_ticks(dur, (n, a), "MuseScore tuplet duration")?;
                            }
                            if !grace && dur <= 0 {
                                return Err(format!(
                                    "MuseScore {} has a non-positive duration",
                                    if is_rest { "Rest" } else { "Chord" }
                                ));
                            }
                            if !is_rest {
                                let on = checked_score_tick(pos)?;
                                let off = if grace {
                                    on
                                } else {
                                    checked_score_tick(pos.checked_add(dur).ok_or_else(|| {
                                        "MuseScore note timing overflow".to_string()
                                    })?)?
                                };
                                let chord_id = format!(
                                    "mscx:staff:{staff_id}:measure:{mi}:voice:{voice_index}:chord:{element_index}"
                                );
                                let lyrics = chord_lyrics(el, &chord_id, tick_scale)?;
                                let notes: Vec<_> = el
                                    .children()
                                    .filter(|child| child.has_tag_name("Note"))
                                    .collect();
                                let polyphonic = notes.len() > 1;
                                let ambiguous_lyric_ownership = notes.len() != 1;
                                if ambiguous_lyric_ownership {
                                    for lyric in &lyrics {
                                        push_event(
                                            &mut unassigned_chord_lyrics,
                                            on,
                                            Kind::Lyrics(lyric.clone()),
                                        );
                                    }
                                }
                                for (note_index, note) in notes.into_iter().enumerate() {
                                    let pitch_text = child_text(note, "pitch").ok_or_else(|| {
                                        format!(
                                            "MuseScore Note {note_index} in {chord_id} is missing pitch"
                                        )
                                    })?;
                                    let pitch = pitch_text
                                        .parse::<i64>()
                                        .ok()
                                        .and_then(|value| u8::try_from(value).ok())
                                        .filter(|value| *value <= 127)
                                        .ok_or_else(|| {
                                            format!(
                                                "MuseScore Note pitch is invalid: {pitch_text:?}"
                                            )
                                        })?;
                                    let source_id = format!("{chord_id}:note:{note_index}");
                                    let channel = info
                                        .instruments
                                        .first()
                                        .and_then(|instrument| instrument.channel);
                                    let chord_member = polyphonic.then_some(note_index);
                                    let events = voice_events
                                        .entry((voice_index, chord_member))
                                        .or_default();
                                    push_event(
                                        events,
                                        on,
                                        Kind::NoteOn(NoteOn {
                                            channel,
                                            key: Some(pitch),
                                            velocity: None,
                                            source: NoteSource {
                                                id: source_id.clone(),
                                                part_id: Some(info.part_id.clone()),
                                                staff_id: Some(staff_id.clone()),
                                                voice: Some((voice_index + 1).to_string()),
                                                chord_id: Some(chord_id.clone()),
                                                instrument_id: info
                                                    .instruments
                                                    .first()
                                                    .and_then(|instrument| instrument.id.clone()),
                                                occurrence: pass,
                                                grace,
                                                unpitched: None,
                                            },
                                            // MuseScore owns lyrics at Chord level. A
                                            // single-note chord has one unambiguous target;
                                            // a polyphonic chord keeps its lyric standalone
                                            // and source-only instead of assigning a pitch.
                                            lyrics: if !ambiguous_lyric_ownership {
                                                lyrics.clone()
                                            } else {
                                                Vec::new()
                                            },
                                        }),
                                    );
                                    push_event(
                                        events,
                                        off,
                                        Kind::NoteOff(NoteOff {
                                            channel,
                                            key: Some(pitch),
                                            velocity: None,
                                            source_id: Some(source_id),
                                        }),
                                    );
                                }
                            }
                            if !grace {
                                pos = pos
                                    .checked_add(dur)
                                    .ok_or_else(|| "MuseScore cursor overflow".to_string())?;
                            }
                        }
                        _ => {}
                    }
                }
            }
            // irregular measure (anacrusis): len="a/b" attribute
            if let Some(value) = measure.attribute("len") {
                let (numerator, denominator) = frac(value).ok_or_else(|| {
                    format!("MuseScore measure len fraction is invalid: {value:?}")
                })?;
                this_len = exact_ticks(
                    div,
                    (
                        4i64.checked_mul(numerator).ok_or_else(|| {
                            "MuseScore measure len numerator overflow".to_string()
                        })?,
                        denominator,
                    ),
                    "MuseScore measure len",
                )?;
                if this_len <= 0 {
                    return Err("MuseScore measure len is non-positive".into());
                }
            }
            measure_start = measure_start
                .checked_add(this_len)
                .ok_or_else(|| "MuseScore measure timeline overflow".to_string())?;
        }

        for ((voice_index, chord_member), mut events) in voice_events {
            sort_and_reindex(&mut events);
            if !events
                .iter()
                .any(|event| matches!(event.kind, Kind::NoteOn(_)))
            {
                continue;
            }
            let mut track = Track {
                id: match chord_member {
                    Some(member) => format!(
                        "mscx:staff:{staff_id}:voice:{}:polyphonic-member:{}",
                        voice_index + 1,
                        member + 1
                    ),
                    None => format!("mscx:staff:{staff_id}:voice:{}", voice_index + 1),
                },
                name: if info.name.is_empty() {
                    match chord_member {
                        Some(member) => {
                            format!("Staff {staff_id} — polyphonic member {}", member + 1)
                        }
                        None => format!("Staff {staff_id}"),
                    }
                } else if let Some(member) = chord_member {
                    format!("{} — polyphonic member {}", info.name, member + 1)
                } else if voice_index == 0 {
                    info.name.clone()
                } else {
                    format!("{} — voice {}", info.name, voice_index + 1)
                },
                source: TrackSource {
                    source_track: tracks.len(),
                    part_id: Some(info.part_id.clone()),
                    staff_id: Some(staff_id.clone()),
                    voice: Some((voice_index + 1).to_string()),
                },
                role_hint: info.role,
                text_profile: MidiTextProfile::Generic,
                instruments: info.instruments.clone(),
                instrument: info.instruments.first().cloned(),
                events,
            };
            if track
                .events
                .iter()
                .any(|event| matches!(&event.kind, Kind::NoteOn(note) if !note.lyrics.is_empty()))
            {
                track.role_hint = TrackRoleHint::Vocal;
            }
            tracks.push(track);
        }
        if !unassigned_chord_lyrics.is_empty() {
            sort_and_reindex(&mut unassigned_chord_lyrics);
            tracks.push(Track {
                id: format!("mscx:staff:{staff_id}:chord-lyrics"),
                name: if info.name.is_empty() {
                    format!("Staff {staff_id} — unassigned chord lyrics")
                } else {
                    format!("{} — unassigned chord lyrics", info.name)
                },
                source: TrackSource {
                    source_track: tracks.len(),
                    part_id: Some(info.part_id.clone()),
                    staff_id: Some(staff_id.clone()),
                    voice: None,
                },
                role_hint: TrackRoleHint::Ambiguous,
                text_profile: MidiTextProfile::Generic,
                instruments: Vec::new(),
                instrument: None,
                events: unassigned_chord_lyrics,
            });
        }
    }

    if !global_events.is_empty() {
        if tracks.is_empty() {
            tracks.push(Track {
                id: "mscx:metadata".into(),
                name: "Score metadata".into(),
                source: TrackSource::default(),
                role_hint: TrackRoleHint::Ambiguous,
                text_profile: MidiTextProfile::Generic,
                instruments: Vec::new(),
                instrument: None,
                events: Vec::new(),
            });
        }
        tracks[0].events.extend(global_events);
        sort_and_reindex(&mut tracks[0].events);
    }
    if tracks.is_empty() {
        return Err("no usable staff in the MuseScore file".into());
    }
    Ok(Midi {
        ticks_per_beat: tpb,
        time_base: TimeBase::PulsesPerQuarter(tpb),
        format: 1,
        source_format: SourceFormat::MuseScore,
        tracks,
    })
}

fn push_event(events: &mut Vec<Event>, tick: u32, kind: Kind) {
    let order = events.len() as u32;
    events.push(Event::new(tick, order, kind));
}

fn checked_score_tick(value: i64) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| "MuseScore tick exceeds the supported range".into())
}

fn push_global_event(events: &mut Vec<Event>, tick: u32, kind: Kind) {
    let duplicate = events.iter().any(|event| {
        if event.tick != tick {
            return false;
        }
        match (&event.kind, &kind) {
            (Kind::Tempo(left), Kind::Tempo(right)) => left == right,
            (
                Kind::TimeSig {
                    num: left_num,
                    den: left_den,
                    ..
                },
                Kind::TimeSig {
                    num: right_num,
                    den: right_den,
                    ..
                },
            ) => left_num == right_num && left_den == right_den,
            _ => false,
        }
    });
    if !duplicate {
        push_event(events, tick, kind);
    }
}

fn sort_and_reindex(events: &mut [Event]) {
    events.sort_by_key(|event| (event.tick, event.order));
    for (order, event) in events.iter_mut().enumerate() {
        event.order = u32::try_from(order).unwrap_or(u32::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

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

    fn zipped_score(bytes: &[u8]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .start_file("score.mscx", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn latin1(value: &str) -> Vec<u8> {
        value
            .chars()
            .map(|character| {
                u8::try_from(u32::from(character))
                    .unwrap_or_else(|_| panic!("test encoder does not cover {character:?}"))
            })
            .collect()
    }

    fn lyrics_of(midi: &Midi) -> Vec<String> {
        midi.tracks
            .iter()
            .flat_map(|track| track.events.iter())
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some(note.lyrics.iter()),
                _ => None,
            })
            .flatten()
            .filter_map(|lyric| match &lyric.state {
                LyricState::Text(text) => Some(text.clone()),
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
    fn raw_and_zipped_latin1_follow_the_xml_declaration() {
        let xml = mscx("<text>café</text>").replace("UTF-8", "ISO-8859-1");
        let bytes = latin1(&xml);
        let raw = parse(&bytes).expect("raw MSCX uses its declared encoding");
        assert_eq!(lyrics_of(&raw), vec!["café"]);

        let archive = zipped_score(&bytes);
        let zipped = parse(&archive).expect("zipped MSCX uses the entry declaration");
        assert_eq!(lyrics_of(&zipped), vec!["café"]);
    }

    #[test]
    fn musescore_detection_scans_past_long_prologs() {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!--{}-->\n{}",
            "padding".repeat(200),
            mscx("<text>let</text>")
                .split_once("<museScore")
                .map(|(_, tail)| format!("<museScore{tail}"))
                .unwrap()
        );
        assert!(is_musescore_xml(xml.as_bytes()));
        assert!(crate::engine::musicxml::looks_like_xml(xml.as_bytes()));
        let midi = parse(xml.as_bytes()).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn fractional_tick_durations_raise_the_exact_timebase_instead_of_truncating() {
        let xml = mscx("<text>tiny</text>").replace(
            "<durationType>quarter</durationType>",
            "<durationType>256th</durationType>",
        );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(midi.ticks_per_beat, 960);
        let ticks: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| {
                matches!(event.kind, Kind::NoteOn(_) | Kind::NoteOff(_)).then_some(event.tick)
            })
            .collect();
        assert_eq!(ticks, vec![0, 15]);
    }

    #[test]
    fn exact_tuplet_scale_is_computed_before_events_are_emitted() {
        let xml = mscx("<text>seven</text>").replace(
            "<Chord>\n            <durationType>quarter</durationType>",
            "<Tuplet><normalNotes>1</normalNotes><actualNotes>7</actualNotes></Tuplet>\
             <Chord>\n            <durationType>quarter</durationType>",
        );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(midi.ticks_per_beat, 3_360);
        let off = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| matches!(event.kind, Kind::NoteOff(_)).then_some(event.tick))
            .unwrap();
        assert_eq!(off, 480);
    }

    #[test]
    fn tuplet_scaling_keeps_the_base_duration_exact_too() {
        let xml = mscx("<text>exact</text>")
            .replace("<Division>480</Division>", "<Division>3</Division>")
            .replace(
                "<Chord>\n            <durationType>quarter</durationType>",
                "<Tuplet><normalNotes>2</normalNotes><actualNotes>3</actualNotes></Tuplet>\
                 <Chord>\n            <durationType>eighth</durationType>",
            );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(midi.ticks_per_beat, 6);
        let off = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| matches!(event.kind, Kind::NoteOff(_)).then_some(event.tick))
            .unwrap();
        assert_eq!(off, 2);
    }

    #[test]
    fn unrepresentable_fraction_is_rejected_instead_of_truncated() {
        let xml = mscx("<text>let</text>").replace(
            "<Chord>\n            <durationType>quarter</durationType>",
            "<location><fractions>1/999983</fractions></location>\
             <Chord>\n            <durationType>quarter</durationType>",
        );
        let error = parse_mscx(&xml).expect_err("oversized exact PPQ must be explicit");
        assert!(
            error.contains("exact range") || error.contains("tick division"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn polyphonic_chord_lyrics_remain_standalone_and_source_only() {
        let xml = mscx("<text>ambiguous</text>").replace(
            "<Note><pitch>60</pitch></Note>",
            "<Note><pitch>60</pitch></Note><Note><pitch>64</pitch></Note>",
        );
        let midi = parse_mscx(&xml).unwrap();
        let attached_count = midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some(note.lyrics.len()),
                _ => None,
            })
            .sum::<usize>();
        let standalone_count = midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .filter(|event| matches!(event.kind, Kind::Lyrics(_)))
            .count();
        assert_eq!(attached_count, 0);
        assert_eq!(standalone_count, 1);
        assert_eq!(
            midi.tracks
                .iter()
                .filter(|track| {
                    track
                        .events
                        .iter()
                        .any(|event| matches!(event.kind, Kind::NoteOn(_)))
                })
                .count(),
            2
        );

        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        assert_eq!(outcome.placed, 0);
        assert!(outcome.svp.unwrap().tracks.is_empty());
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
        let midi = parse_mscx(&mscx(r#"<text>shi<font face="Arial"></font>ne,</text>"#)).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["shine,"]);
    }

    #[test]
    fn empty_formatted_lyric_is_preserved_as_explicit_empty() {
        let midi = parse_mscx(&mscx(r#"<text><font size="9.2"></font></text>"#)).unwrap();
        assert!(lyrics_of(&midi).is_empty());
        let lyric = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::NoteOn(note) => note.lyrics.first(),
                _ => None,
            })
            .expect("the empty source lyric remains attached");
        assert_eq!(lyric.state, LyricState::ExplicitEmpty);
    }

    #[test]
    fn sym_glyph_name_is_not_injected() {
        // <sym> holds a SMuFL glyph identifier, not renderable lyric text.
        let midi = parse_mscx(&mscx(r#"<text>a<sym>space</sym>b</text>"#)).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["ab"]);
    }

    #[test]
    fn pretty_printed_text_is_trimmed() {
        let midi = parse_mscx(&mscx(
            "<text>\n  <font size=\"9.2\"></font>\n  let\n</text>",
        ))
        .unwrap();
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn xml_entities_are_decoded() {
        let midi = parse_mscx(&mscx("<text>rock &amp; roll</text>")).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["rock & roll"]);
    }

    #[test]
    fn every_styling_wrapper_yields_its_text() {
        // Any combination of style tags, nesting, sizes and faces must never
        // hide the syllable.
        for (xml, want) in [
            (r#"<text><b>bold</b></text>"#, "bold"),
            (r#"<text><i>ital</i></text>"#, "ital"),
            (r#"<text><u>under</u></text>"#, "under"),
            (r#"<text><s>strike</s></text>"#, "strike"),
            (r#"<text><b><i><u>all</u></i></b></text>"#, "all"),
            (
                r#"<text><font face="Comic Sans MS"></font><b>mix</b>ed</text>"#,
                "mixed",
            ),
            (
                r#"<text><font size="24"></font><font size="6"></font>tiny</text>"#,
                "tiny",
            ),
            (r#"<text>x<sup>2</sup></text>"#, "x2"),
            (r#"<text>H<sub>2</sub>O</text>"#, "H2O"),
            (
                r#"<text><font face="Arial"><b>deep</b></font></text>"#,
                "deep",
            ),
            (r#"<text><b>a<sym>space</sym>b</b></text>"#, "ab"),
            (
                r#"<text><font size="9.2"/><font face="Edwin"/>self-closed</text>"#,
                "self-closed",
            ),
        ] {
            let midi = parse_mscx(&mscx(xml)).unwrap();
            assert_eq!(lyrics_of(&midi), vec![want], "input: {}", xml);
        }
    }

    #[test]
    fn styled_track_name_is_read() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <Instrument><longName><b>Sopra</b>no</longName></Instrument>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let names: Vec<String> = midi.tracks.iter().map(|track| track.name.clone()).collect();
        assert_eq!(names, vec!["Soprano"]);
    }

    #[test]
    fn br_separates_words_in_names_and_lyrics() {
        // Lyric: <br/> must never fuse adjacent words.
        let midi = parse_mscx(&mscx("<text>a<br/>b</text>")).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["a b"]);
        // Name: real-world case from tests/fixtures/help.mscz.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <Instrument><longName>Batterie ou<br/>persussions<br/>corporelles</longName></Instrument>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let names: Vec<String> = midi.tracks.iter().map(|track| track.name.clone()).collect();
        assert_eq!(names, vec!["Batterie ou persussions corporelles"]);
    }

    #[test]
    fn multiline_track_name_is_collapsed_to_one_line() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <trackName>Soprano
Melodie</trackName>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let names: Vec<String> = midi.tracks.iter().map(|track| track.name.clone()).collect();
        assert_eq!(names, vec!["Soprano Melodie"]);
    }

    #[test]
    fn deeply_nested_forged_xml_is_rejected_cleanly() {
        let mut xml = String::from(r#"<museScore version="3.02"><Score><Division>480</Division>"#);
        for _ in 0..250 {
            xml.push_str("<b>");
        }
        xml.push('x');
        for _ in 0..250 {
            xml.push_str("</b>");
        }
        xml.push_str("</Score></museScore>");
        let err = match parse_mscx(&xml) {
            Err(e) => e,
            Ok(_) => panic!("expected a nesting error"),
        };
        assert!(
            err.contains("nesting"),
            "expected a clean nesting error, got: {}",
            err
        );
    }

    #[test]
    fn empty_long_name_falls_back_to_track_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <trackName>Voix</trackName>
      <Instrument><longName> </longName></Instrument>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let names: Vec<String> = midi.tracks.iter().map(|track| track.name.clone()).collect();
        assert_eq!(names, vec!["Voix"]);
    }

    #[test]
    fn present_invalid_division_is_rejected_instead_of_replaced() {
        for invalid in ["0", "480.5"] {
            let xml = mscx("<text>let</text>").replace(
                "<Division>480</Division>",
                &format!("<Division>{invalid}</Division>"),
            );
            let error = parse_mscx(&xml).expect_err("invalid Division must fail");
            assert!(error.contains("Division"), "unexpected error: {error}");
        }
    }

    #[test]
    fn missing_or_unknown_duration_is_never_replaced_by_a_quarter() {
        for replacement in ["", "<durationType>mystery</durationType>"] {
            let xml = mscx("<text>let</text>")
                .replace("<durationType>quarter</durationType>", replacement);
            let error = parse_mscx(&xml).expect_err("duration must fail explicitly");
            assert!(
                error.contains("durationType"),
                "unexpected error for {replacement:?}: {error}"
            );
        }
    }

    #[test]
    fn out_of_range_pitch_is_rejected_instead_of_clamped() {
        let xml = mscx("<text>let</text>").replace("<pitch>60</pitch>", "<pitch>200</pitch>");
        let error = parse_mscx(&xml).expect_err("invalid pitch must fail");
        assert!(error.contains("pitch"), "unexpected error: {error}");
    }

    #[test]
    fn grace_note_keeps_zero_playback_duration_and_is_counted_as_source() {
        let xml = mscx("<text>let</text>").replace(
            "<durationType>quarter</durationType>",
            "<acciaccatura/><durationType>eighth</durationType>",
        );
        let midi = parse_mscx(&xml).unwrap();
        let note_ticks: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(_) | Kind::NoteOff(_) => Some(event.tick),
                _ => None,
            })
            .collect();
        assert_eq!(note_ticks, vec![0, 0]);
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert_eq!(outcome.tracks[0].notes, 1);
        assert_eq!(outcome.placed, 0);
        assert!(outcome.svp.unwrap().tracks.is_empty());
    }

    #[test]
    fn repeat_occurrences_reemit_tempo_and_meter() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure>
        <startRepeat/>
        <voice>
          <TimeSig><sigN>3</sigN><sigD>4</sigD></TimeSig>
          <Tempo><tempo>1.5</tempo></Tempo>
          <Chord><durationType>quarter</durationType><Note><pitch>60</pitch></Note></Chord>
        </voice>
        <endRepeat>2</endRepeat>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let tempo_ticks: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| matches!(event.kind, Kind::Tempo(_)).then_some(event.tick))
            .collect();
        let meter_ticks: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| matches!(event.kind, Kind::TimeSig { .. }).then_some(event.tick))
            .collect();
        assert_eq!(tempo_ticks, vec![0, 1_440]);
        assert_eq!(meter_ticks, vec![0, 1_440]);
    }

    #[test]
    fn globals_survive_when_the_first_staff_contains_only_rests() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Rest</trackName><Staff id="1"/></Part>
    <Part><trackName>Voice</trackName><Staff id="2"/></Part>
    <Staff id="1">
      <Measure><voice><Rest><durationType>quarter</durationType></Rest></voice></Measure>
    </Staff>
    <Staff id="2">
      <Measure>
        <voice>
          <TimeSig><sigN>6</sigN><sigD>8</sigD></TimeSig>
          <Tempo><tempo>1.2</tempo></Tempo>
          <Chord><durationType>quarter</durationType><Note><pitch>62</pitch></Note></Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        assert!(midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .any(|event| matches!(event.kind, Kind::TimeSig { num: 6, den: 8, .. })));
        assert!(midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .any(|event| matches!(event.kind, Kind::Tempo(_))));
    }

    #[test]
    fn musescore_dtd_is_rejected() {
        let xml = mscx("<text>let</text>").replace(
            "<museScore",
            "<!DOCTYPE museScore SYSTEM \"file:///tmp/forbidden.dtd\">\n<museScore",
        );
        let error = match parse_mscx(&xml) {
            Err(error) => error,
            Ok(_) => panic!("MuseScore DTDs must stay disabled"),
        };
        assert!(error.contains("DTD") || error.contains("XML"));
    }

    #[test]
    fn negative_lyric_extension_is_preserved_without_becoming_a_continuation() {
        let midi = parse_mscx(&mscx(
            "<text>let</text><ticks>-1680</ticks><ticks_f>-7/8</ticks_f>",
        ))
        .unwrap();
        let lyric = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::NoteOn(note) => note.lyrics.first(),
                _ => None,
            })
            .unwrap();
        assert_eq!(lyric.extend_ticks, Some(-1680));
        assert_eq!(lyric.extend_fraction, Some((-7, 8)));
    }
}
