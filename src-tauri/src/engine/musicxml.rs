//! MusicXML parser (.xml / .musicxml) and compressed MusicXML (.mxl).
//! Produces the same intermediate `Midi` structure as the MIDI parser, so the
//! whole multi-track conversion logic can be reused.
use crate::engine::midi::{
    unroll, Event, InstrumentInfo, Jump, Kind, Lyric, LyricExtension, LyricFragment, LyricState,
    MeasureMarks, Midi, MidiTextProfile, NoteOff, NoteOn, NoteSource, SourceFormat, Syllabic,
    TimeBase, Track, TrackRoleHint, TrackSource, UnpitchedInfo,
};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;

type MusicXmlVoiceKey = (String, String, usize);
type MusicXmlTieKey = (String, String, usize, String, Option<String>);
type MusicXmlVoiceEvents = BTreeMap<MusicXmlVoiceKey, Vec<Event>>;

pub fn is_zip(data: &[u8]) -> bool {
    data.len() >= 2 && &data[0..2] == b"PK"
}

pub fn looks_like_xml(data: &[u8]) -> bool {
    xml_bytes_contain_ascii(data, b"<?xml")
        || xml_bytes_contain_ascii(data, b"<score-partwise")
        || xml_bytes_contain_ascii(data, b"<score-timewise")
        || xml_bytes_contain_ascii(data, b"<museScore")
}

pub(crate) fn xml_bytes_contain_ascii(data: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if data.windows(needle.len()).any(|window| window == needle) {
        return true;
    }
    let mut utf16_le = Vec::with_capacity(needle.len() * 2);
    let mut utf16_be = Vec::with_capacity(needle.len() * 2);
    for byte in needle {
        utf16_le.extend_from_slice(&[*byte, 0]);
        utf16_be.extend_from_slice(&[0, *byte]);
    }
    data.windows(utf16_le.len())
        .any(|window| window == utf16_le || window == utf16_be)
}

/// True if the ZIP archive contains a MusicXML (and not a native MuseScore .mscx).
pub fn zip_has_musicxml(data: &[u8]) -> bool {
    if let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(data)) {
        for i in 0..zip.len() {
            if let Ok(f) = zip.by_index(i) {
                let name = f.name().to_string();
                if !name.starts_with("META-INF")
                    && (name.ends_with(".xml") || name.ends_with(".musicxml"))
                {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_mxl(data: &[u8]) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    // rootfile via META-INF/container.xml, otherwise 1st .xml outside META-INF
    let mut root: Option<String> = None;
    if let Ok(mut cf) = zip.by_name("META-INF/container.xml") {
        let s = read_zip_entry_capped(&mut cf)?;
        check_nesting(&s)?;
        let doc = roxmltree::Document::parse_with_options(
            &s,
            roxmltree::ParsingOptions {
                allow_dtd: false,
                nodes_limit: 100_000,
            },
        )
        .map_err(|error| format!("invalid MusicXML container: {error}"))?;
        let rf = doc
            .descendants()
            .find(|node| node.has_tag_name("rootfile"))
            .ok_or_else(|| "MusicXML container has no rootfile".to_string())?;
        root = Some(
            rf.attribute("full-path")
                .ok_or_else(|| "MusicXML rootfile has no full-path".to_string())?
                .to_string(),
        );
    }
    let name = match root {
        Some(r) => r,
        None => {
            let mut found = None;
            for i in 0..zip.len() {
                let f = zip.by_index(i).map_err(|e| e.to_string())?;
                let n = f.name().to_string();
                if !n.starts_with("META-INF") && (n.ends_with(".xml") || n.ends_with(".musicxml")) {
                    found = Some(n);
                    break;
                }
            }
            found.ok_or_else(|| "no MusicXML in archive".to_string())?
        }
    };
    let mut f = zip.by_name(&name).map_err(|e| e.to_string())?;
    read_zip_entry_capped(&mut f)
}

/// Reads a ZIP entry with a decompressed-size cap (anti zip-bomb).
pub(crate) fn read_zip_entry_capped(f: &mut impl Read) -> Result<String, String> {
    const MAX: u64 = 64 * 1024 * 1024; // 64 MB, far beyond any real score
    let mut bytes = Vec::new();
    f.take(MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX {
        return Err("abnormally large archive (rejected for safety)".into());
    }
    decode_xml_bytes(&bytes)
}

fn xml_declared_encoding(data: &[u8]) -> Option<String> {
    let data = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data);
    if !data.starts_with(b"<?xml") {
        return None;
    }
    let end = data
        .windows(2)
        .position(|window| window == b"?>")
        .unwrap_or(data.len());
    let declaration = String::from_utf8_lossy(&data[..end]);
    let lower = declaration.to_ascii_lowercase();
    let start = lower.find("encoding")? + "encoding".len();
    let tail = declaration.get(start..)?;
    let tail = tail.trim_start();
    let tail = tail.strip_prefix('=')?.trim_start();
    let quote = tail.as_bytes().first().copied()?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let value = &tail[1..];
    let end = value.as_bytes().iter().position(|byte| *byte == quote)?;
    Some(value[..end].trim().to_ascii_lowercase())
}

fn decode_utf16_xml(data: &[u8], little_endian: bool) -> Result<String, String> {
    if !data.chunks_exact(2).remainder().is_empty() {
        return Err("invalid XML: odd-length UTF-16 input".into());
    }
    let words = data.chunks_exact(2).map(|chunk| {
        let bytes = [chunk[0], chunk[1]];
        if little_endian {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }
    });
    char::decode_utf16(words)
        .collect::<Result<String, _>>()
        .map_err(|_| "invalid XML: malformed UTF-16 input".into())
}

fn windows_1252_char(byte: u8) -> char {
    match byte {
        0x80 => '\u{20ac}',
        0x82 => '\u{201a}',
        0x83 => '\u{0192}',
        0x84 => '\u{201e}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02c6}',
        0x89 => '\u{2030}',
        0x8a => '\u{0160}',
        0x8b => '\u{2039}',
        0x8c => '\u{0152}',
        0x8e => '\u{017d}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201c}',
        0x94 => '\u{201d}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02dc}',
        0x99 => '\u{2122}',
        0x9a => '\u{0161}',
        0x9b => '\u{203a}',
        0x9c => '\u{0153}',
        0x9e => '\u{017e}',
        0x9f => '\u{0178}',
        _ => char::from(byte),
    }
}

/// Decodes an XML byte stream according to its BOM and XML declaration.
/// XML defaults to UTF-8; malformed or unsupported encodings are rejected
/// explicitly instead of being replaced with U+FFFD.
pub(crate) fn decode_xml_bytes(data: &[u8]) -> Result<String, String> {
    if let Some(bytes) = data.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(bytes.to_vec())
            .map_err(|_| "invalid XML: malformed UTF-8 input".into());
    }
    if let Some(bytes) = data.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16_xml(bytes, true);
    }
    if let Some(bytes) = data.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16_xml(bytes, false);
    }
    if data.starts_with(&[b'<', 0, b'?', 0, b'x', 0, b'm', 0, b'l', 0]) {
        return decode_utf16_xml(data, true);
    }
    if data.starts_with(&[0, b'<', 0, b'?', 0, b'x', 0, b'm', 0, b'l']) {
        return decode_utf16_xml(data, false);
    }

    match xml_declared_encoding(data).as_deref() {
        None | Some("utf-8" | "utf8" | "us-ascii" | "ascii") => String::from_utf8(data.to_vec())
            .map_err(|_| "invalid XML: malformed UTF-8 input".into()),
        Some("iso-8859-1" | "iso_8859-1" | "latin1" | "latin-1") => {
            Ok(data.iter().map(|byte| char::from(*byte)).collect())
        }
        Some("windows-1252" | "windows1252" | "cp1252") => {
            Ok(data.iter().map(|byte| windows_1252_char(*byte)).collect())
        }
        Some("utf-16" | "utf-16le") => decode_utf16_xml(data, true),
        Some("utf-16be") => decode_utf16_xml(data, false),
        Some(encoding) => Err(format!("unsupported XML encoding: {encoding}")),
    }
}

pub fn parse(data: &[u8]) -> Result<Midi, String> {
    let xml = if is_zip(data) {
        extract_mxl(data)?
    } else {
        decode_xml_bytes(data)?
    };
    parse_musicxml(&xml)
}

fn step_semitone(s: &str) -> Option<i32> {
    match s {
        "C" => Some(0),
        "D" => Some(2),
        "E" => Some(4),
        "F" => Some(5),
        "G" => Some(7),
        "A" => Some(9),
        "B" => Some(11),
        _ => None,
    }
}

fn scale_duration(value: i64, ticks_per_beat: u16, local_divisions: u32) -> Result<i64, String> {
    if local_divisions == 0 {
        return Err("MusicXML divisions must be positive".into());
    }
    let numerator = i128::from(value) * i128::from(ticks_per_beat);
    let denominator = i128::from(local_divisions);
    if numerator % denominator != 0 {
        return Err("MusicXML timing cannot be represented exactly".into());
    }
    i64::try_from(numerator / denominator).map_err(|_| "MusicXML timing overflow".into())
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn exact_tick_base(doc: &roxmltree::Document) -> Result<u16, String> {
    let mut value = 1u64;
    let mut found = false;
    for node in doc
        .descendants()
        .filter(|node| node.has_tag_name("divisions"))
    {
        let text = node
            .text()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| "MusicXML divisions value is empty".to_string())?;
        let divisions = text
            .parse::<u64>()
            .map_err(|_| format!("MusicXML divisions value is invalid: {text:?}"))?;
        if divisions == 0 {
            return Err("MusicXML divisions must be positive".into());
        }
        found = true;
        value = value
            .checked_div(gcd(value, divisions))
            .and_then(|base| base.checked_mul(divisions))
            .ok_or_else(|| "MusicXML divisions overflow".to_string())?;
        if value > u64::from(u16::MAX) {
            return Err("MusicXML divisions exceed the exact timing range".into());
        }
    }
    Ok(if found { value as u16 } else { 480 })
}

fn child_i64(n: roxmltree::Node, tag: &str) -> Option<i64> {
    n.children()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.text())
        .and_then(|t| t.trim().parse::<i64>().ok())
}

fn strict_child_i64(
    node: roxmltree::Node,
    tag: &str,
    context: &str,
) -> Result<Option<i64>, String> {
    let Some(child) = node.children().find(|child| child.has_tag_name(tag)) else {
        return Ok(None);
    };
    let text = child
        .text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("{context} {tag} is empty"))?;
    text.parse::<i64>()
        .map(Some)
        .map_err(|_| format!("{context} {tag} is invalid: {text:?}"))
}

fn required_child_text<'a>(
    node: roxmltree::Node<'a, '_>,
    tag: &str,
    context: &str,
) -> Result<&'a str, String> {
    node.children()
        .find(|child| child.has_tag_name(tag))
        .and_then(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("{context} is missing {tag}"))
}

fn note_source_duration(note: roxmltree::Node, grace: bool) -> Result<i64, String> {
    let duration = strict_child_i64(note, "duration", "MusicXML note")?;
    if grace {
        return match duration {
            None | Some(0) => Ok(0),
            Some(value) => Err(format!(
                "MusicXML grace note duration must be omitted or zero, got {value}"
            )),
        };
    }
    duration
        .filter(|value| *value > 0)
        .ok_or_else(|| "MusicXML non-grace note duration must be positive".to_string())
}

fn movement_duration(node: roxmltree::Node, kind: &str) -> Result<i64, String> {
    strict_child_i64(node, "duration", &format!("MusicXML {kind}"))?
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("MusicXML {kind} duration must be positive"))
}

fn pitched_key(pitch: roxmltree::Node) -> Result<u8, String> {
    let step_text = required_child_text(pitch, "step", "MusicXML pitch")?;
    let step = step_semitone(step_text)
        .map(i64::from)
        .ok_or_else(|| format!("MusicXML pitch step is invalid: {step_text:?}"))?;
    let alter = strict_child_i64(pitch, "alter", "MusicXML pitch")?.unwrap_or(0);
    let octave = strict_child_i64(pitch, "octave", "MusicXML pitch")?
        .ok_or_else(|| "MusicXML pitch is missing octave".to_string())?;
    let midi = octave
        .checked_add(1)
        .and_then(|value| value.checked_mul(12))
        .and_then(|value| value.checked_add(step))
        .and_then(|value| value.checked_add(alter))
        .ok_or_else(|| "MusicXML pitch calculation overflow".to_string())?;
    u8::try_from(midi)
        .ok()
        .filter(|value| *value <= 127)
        .ok_or_else(|| format!("MusicXML pitch is outside the MIDI range: {midi}"))
}

fn parse_additive_beats(value: &str) -> Result<u64, String> {
    let mut total = 0u64;
    let mut found = false;
    for term in value.split('+') {
        let term = term.trim();
        if term.is_empty() {
            return Err(format!(
                "MusicXML additive time-signature beats are invalid: {value:?}"
            ));
        }
        let value = term
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                format!("MusicXML additive time-signature beats are invalid: {value:?}")
            })?;
        total = total
            .checked_add(value)
            .ok_or_else(|| "MusicXML time-signature numerator overflow".to_string())?;
        found = true;
    }
    if !found {
        return Err("MusicXML time-signature beats are empty".into());
    }
    Ok(total)
}

fn parse_time_signature(time: roxmltree::Node) -> Result<(u8, u16), String> {
    let elements: Vec<_> = time.children().filter(|node| node.is_element()).collect();
    let mut index = 0usize;
    let mut numerator = 0u64;
    let mut denominator = 1u64;
    let mut found = false;
    while index < elements.len() {
        if !elements[index].has_tag_name("beats") {
            index += 1;
            continue;
        }
        let beats_text = elements[index]
            .text()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| "MusicXML time-signature beats are empty".to_string())?;
        let beats = parse_additive_beats(beats_text)?;
        let beat_type = elements
            .get(index + 1)
            .filter(|node| node.has_tag_name("beat-type"))
            .and_then(|node| node.text())
            .map(str::trim)
            .and_then(|text| text.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                format!("MusicXML time-signature beat-type after {beats_text:?} is invalid")
            })?;
        let common = denominator
            .checked_div(gcd(denominator, beat_type))
            .and_then(|value| value.checked_mul(beat_type))
            .ok_or_else(|| "MusicXML time-signature denominator overflow".to_string())?;
        numerator = numerator
            .checked_mul(common / denominator)
            .and_then(|value| {
                beats
                    .checked_mul(common / beat_type)
                    .and_then(|term| value.checked_add(term))
            })
            .ok_or_else(|| "MusicXML time-signature numerator overflow".to_string())?;
        denominator = common;
        found = true;
        index += 2;
    }
    if !found {
        return Err("MusicXML time signature has no beats/beat-type pair".into());
    }
    Ok((
        u8::try_from(numerator)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| "MusicXML time-signature numerator exceeds 255".to_string())?,
        u16::try_from(denominator)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| "MusicXML time-signature denominator exceeds 65535".to_string())?,
    ))
}

fn beat_unit_quarters(value: &str) -> Option<f64> {
    Some(match value.trim() {
        "maxima" => 32.0,
        "long" => 16.0,
        "breve" => 8.0,
        "whole" => 4.0,
        "half" => 2.0,
        "quarter" => 1.0,
        "eighth" => 0.5,
        "16th" => 0.25,
        "32nd" => 0.125,
        "64th" => 0.0625,
        "128th" => 0.03125,
        "256th" => 0.015625,
        "512th" => 0.0078125,
        "1024th" => 0.00390625,
        _ => return None,
    })
}

fn tempo_micros(quarter_bpm: f64) -> Result<u32, String> {
    if !quarter_bpm.is_finite() || quarter_bpm <= 0.0 {
        return Err("MusicXML tempo must be a positive finite number".into());
    }
    let micros = (60_000_000.0 / quarter_bpm).round();
    if !(1.0..=f64::from(u32::MAX)).contains(&micros) {
        return Err("MusicXML tempo exceeds the supported range".into());
    }
    Ok(micros as u32)
}

fn direction_tempo(direction: roxmltree::Node) -> Result<Option<u32>, String> {
    let sound = if direction.has_tag_name("sound") {
        Some(direction)
    } else {
        direction
            .descendants()
            .find(|node| node.has_tag_name("sound"))
    };
    if let Some(value) = sound.and_then(|node| node.attribute("tempo")) {
        let bpm = value
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("MusicXML sound tempo is invalid: {value:?}"))?;
        return tempo_micros(bpm).map(Some);
    }
    if direction.has_tag_name("sound") {
        return Ok(None);
    }
    let Some(metronome) = direction.descendants().find(|node| {
        node.has_tag_name("metronome")
            && node
                .children()
                .any(|child| child.has_tag_name("per-minute"))
    }) else {
        return Ok(None);
    };
    let beat_unit_text = metronome
        .children()
        .find(|node| node.has_tag_name("beat-unit"))
        .and_then(|node| node.text())
        .map(str::trim)
        .ok_or_else(|| "MusicXML metronome is missing beat-unit".to_string())?;
    let beat_unit = beat_unit_quarters(beat_unit_text).ok_or_else(|| {
        format!("MusicXML metronome beat-unit is unsupported: {beat_unit_text:?}")
    })?;
    let dots = metronome
        .children()
        .filter(|node| node.has_tag_name("beat-unit-dot"))
        .count();
    let mut dotted_unit = beat_unit;
    let mut addition = beat_unit / 2.0;
    for _ in 0..dots {
        dotted_unit += addition;
        addition /= 2.0;
    }
    let per_minute_text = metronome
        .children()
        .find(|node| node.has_tag_name("per-minute"))
        .and_then(|node| node.text())
        .map(str::trim)
        .ok_or_else(|| "MusicXML metronome is missing per-minute".to_string())?;
    let per_minute = per_minute_text
        .parse::<f64>()
        .map_err(|_| format!("MusicXML metronome per-minute is invalid: {per_minute_text:?}"))?;
    tempo_micros(per_minute * dotted_unit).map(Some)
}

fn direction_tick(
    direction: roxmltree::Node,
    position: i64,
    ticks_per_beat: u16,
    divisions: u32,
) -> Result<u32, String> {
    let offset = direction
        .children()
        .find(|node| node.has_tag_name("offset"));
    let delta = match offset {
        Some(offset) => {
            let text = offset
                .text()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .ok_or_else(|| "MusicXML direction offset is empty".to_string())?;
            let value = text
                .parse::<i64>()
                .map_err(|_| format!("MusicXML direction offset is invalid: {text:?}"))?;
            scale_duration(value, ticks_per_beat, divisions)?
        }
        None => 0,
    };
    checked_tick(
        position
            .checked_add(delta)
            .ok_or_else(|| "MusicXML direction offset overflows the timeline".to_string())?,
    )
}

/// Refuses pathologically nested XML before it reaches roxmltree, whose
/// recursive-descent parser (one native stack frame per level) overflows the
/// stack and aborts the whole process on a forged file. Real scores nest well
/// under 30 levels. The scan honors quoted attribute values, comments, CDATA
/// and processing instructions, so it never miscounts valid documents.
pub(crate) fn check_nesting(xml: &str) -> Result<(), String> {
    const MAX_DEPTH: i64 = 200;
    let b = xml.as_bytes();
    let mut depth: i64 = 0;
    let mut i = 0usize;
    while i + 1 < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        match b[i + 1] {
            b'/' => {
                depth -= 1;
                i += 2;
            }
            b'?' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'?' && b[i + 1] == b'>') {
                    i += 1;
                }
                i += 2;
            }
            b'!' => {
                if b[i..].starts_with(b"<!--") {
                    i += 4;
                    while i + 2 < b.len() && &b[i..i + 3] != b"-->" {
                        i += 1;
                    }
                    i += 3;
                } else if b[i..].starts_with(b"<![CDATA[") {
                    i += 9;
                    while i + 2 < b.len() && &b[i..i + 3] != b"]]>" {
                        i += 1;
                    }
                    i += 3;
                } else {
                    while i < b.len() && b[i] != b'>' {
                        i += 1;
                    }
                }
            }
            _ => {
                let mut j = i + 1;
                let mut quote: u8 = 0;
                while j < b.len() {
                    let c = b[j];
                    if quote != 0 {
                        if c == quote {
                            quote = 0;
                        }
                    } else if c == b'"' || c == b'\'' {
                        quote = c;
                    } else if c == b'>' {
                        break;
                    }
                    j += 1;
                }
                if !(j < b.len() && j > 0 && b[j - 1] == b'/') {
                    depth += 1;
                    if depth > MAX_DEPTH {
                        return Err("invalid XML: nesting too deep".into());
                    }
                }
                i = j + 1;
            }
        }
    }
    Ok(())
}

/// MusicXML files commonly carry the official external PUBLIC doctype.
/// roxmltree cannot keep DTD parsing disabled while accepting that declaration,
/// so we mask only a simple external declaration, never resolve it, reject
/// internal subsets/entities, then parse with DTD support disabled.
fn mask_external_musicxml_doctype(xml: &str) -> Result<String, String> {
    let bytes = xml.as_bytes();
    let mut doctype: Option<(usize, usize)> = None;
    let mut index = 0usize;
    while index < bytes.len() {
        let Some(relative) = bytes[index..].iter().position(|byte| *byte == b'<') else {
            break;
        };
        let start = index + relative;
        if bytes[start..].starts_with(b"<!--") {
            let Some(end) = xml[start + 4..].find("-->") else {
                return Err("invalid XML comment".into());
            };
            index = start + 4 + end + 3;
            continue;
        }
        if bytes[start..].starts_with(b"<![CDATA[") {
            let Some(end) = xml[start + 9..].find("]]>") else {
                return Err("invalid XML CDATA".into());
            };
            index = start + 9 + end + 3;
            continue;
        }
        if bytes[start..].starts_with(b"<!DOCTYPE") {
            if doctype.is_some() {
                return Err("multiple XML doctypes are not allowed".into());
            }
            let mut cursor = start + "<!DOCTYPE".len();
            let mut quote = None;
            let mut end = None;
            while cursor < bytes.len() {
                let byte = bytes[cursor];
                if let Some(delimiter) = quote {
                    if byte == delimiter {
                        quote = None;
                    }
                } else if byte == b'"' || byte == b'\'' {
                    quote = Some(byte);
                } else if byte == b'[' {
                    return Err("MusicXML internal DTD subsets are not allowed".into());
                } else if byte == b'>' {
                    end = Some(cursor + 1);
                    break;
                }
                cursor += 1;
            }
            let end = end.ok_or_else(|| "unterminated MusicXML doctype".to_string())?;
            let declaration = xml[start..end].to_ascii_uppercase();
            if !declaration.contains(" PUBLIC ") && !declaration.contains(" SYSTEM ") {
                return Err("only an external MusicXML doctype is allowed".into());
            }
            doctype = Some((start, end));
            index = end;
            continue;
        }
        index = start + 1;
    }
    if xml.to_ascii_uppercase().contains("<!ENTITY") {
        return Err("XML entity declarations are not allowed".into());
    }
    let Some((start, end)) = doctype else {
        return Ok(xml.to_string());
    };
    let mut masked = xml.as_bytes().to_vec();
    for byte in &mut masked[start..end] {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
    String::from_utf8(masked).map_err(|_| "invalid UTF-8 after doctype masking".into())
}

fn parse_time_only(value: &str) -> Vec<u32> {
    value
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter_map(|piece| piece.trim().parse::<u32>().ok())
        .collect()
}

/// Returns every lyric lane owned by this exact MusicXML note. Lane number and
/// playback occurrence are independent: only `time-only` constrains a repeat.
fn note_lyrics(note: roxmltree::Node, source_id: &str) -> Vec<Lyric> {
    note.children()
        .filter(|child| child.has_tag_name("lyric"))
        .enumerate()
        .map(|(index, lyric_node)| {
            let lane = lyric_node.attribute("number").unwrap_or("1").to_string();
            let verse = lane.parse::<u32>().unwrap_or(1);
            let mut raw = String::new();
            let mut fragments = Vec::new();
            for child in lyric_node.children() {
                if child.has_tag_name("text") {
                    let mut value = String::new();
                    crate::engine::musescore::deep_text_raw(child, &mut value);
                    raw.push_str(&value);
                    fragments.push(LyricFragment::Text(value));
                } else if child.has_tag_name("elision") {
                    let mut value = String::new();
                    crate::engine::musescore::deep_text_raw(child, &mut value);
                    raw.push_str(&value);
                    fragments.push(LyricFragment::Elision(value));
                }
            }
            let extension = lyric_node
                .children()
                .find(|child| child.has_tag_name("extend"))
                .map(|extend| match extend.attribute("type") {
                    Some("start") => LyricExtension::Start,
                    Some("continue") => LyricExtension::Continue,
                    Some("stop") => LyricExtension::Stop,
                    _ => LyricExtension::Unspecified,
                });
            let state = if !raw.is_empty() {
                LyricState::Text(raw.clone())
            } else if extension.is_some() {
                LyricState::Continuation
            } else if lyric_node
                .children()
                .any(|child| child.has_tag_name("humming"))
            {
                LyricState::Unsupported("humming".into())
            } else if lyric_node
                .children()
                .any(|child| child.has_tag_name("laughing"))
            {
                LyricState::Unsupported("laughing".into())
            } else {
                LyricState::ExplicitEmpty
            };
            let syllabic = lyric_node
                .children()
                .find(|child| child.has_tag_name("syllabic"))
                .and_then(|child| child.text())
                .and_then(|value| match value.trim() {
                    "single" => Some(Syllabic::Single),
                    "begin" => Some(Syllabic::Begin),
                    "middle" => Some(Syllabic::Middle),
                    "end" => Some(Syllabic::End),
                    _ => None,
                });
            Lyric {
                id: format!("{source_id}:lyric:{index}"),
                raw: raw.clone(),
                raw_bytes: Vec::new(),
                fragments,
                lane,
                verse,
                state,
                syllabic,
                line_break: None,
                time_only: lyric_node
                    .attribute("time-only")
                    .map(parse_time_only)
                    .unwrap_or_default(),
                extension,
                extend_ticks: None,
                extend_fraction: None,
            }
        })
        .collect()
}

fn note_ties(note: roxmltree::Node) -> (bool, bool) {
    let mut starts = false;
    let mut stops = false;
    for tie in note
        .descendants()
        .filter(|node| node.has_tag_name("tie") || node.has_tag_name("tied"))
    {
        match tie.attribute("type") {
            Some("start") => starts = true,
            Some("stop") => stops = true,
            Some("continue") => {
                starts = true;
                stops = true;
            }
            _ => {}
        }
    }
    (starts, stops)
}

fn mark_unresolved_ties_source_only(
    voice_events: &mut MusicXmlVoiceEvents,
    active_ties: &HashMap<MusicXmlTieKey, String>,
) {
    for source_id in active_ties.values() {
        for events in voice_events.values_mut() {
            for event in events {
                match &mut event.kind {
                    Kind::NoteOn(note) if note.source.id == *source_id => note.key = None,
                    Kind::NoteOff(note)
                        if note.source_id.as_deref() == Some(source_id.as_str()) =>
                    {
                        note.key = None;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Playback order of MusicXML measures: repeats (<repeat>), voltas
/// (<ending>), jumps (<sound> attributes: segno/dalsegno/dacapo/coda/
/// tocoda/fine).
fn playback_order_mxl(measures: &[roxmltree::Node]) -> Result<Vec<(usize, u32)>, String> {
    let mut marks = vec![MeasureMarks::default(); measures.len()];
    let mut open_ending: Option<Vec<u32>> = None;

    for (i, m) in measures.iter().enumerate() {
        for b in m.children().filter(|c| c.has_tag_name("barline")) {
            for r in b.children().filter(|c| c.has_tag_name("repeat")) {
                match r.attribute("direction") {
                    Some("forward") => marks[i].start_repeat = true,
                    Some("backward") => {
                        marks[i].end_repeat = r
                            .attribute("times")
                            .and_then(|t| t.parse::<u32>().ok())
                            .unwrap_or(2)
                            .max(2);
                    }
                    _ => {}
                }
            }
            for e in b.children().filter(|c| c.has_tag_name("ending")) {
                let nums: Vec<u32> = e
                    .attribute("number")
                    .unwrap_or("1")
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                let nums = if nums.is_empty() { vec![1] } else { nums };
                match e.attribute("type") {
                    Some("start") => {
                        marks[i].volta = Some(nums.clone());
                        open_ending = Some(nums);
                    }
                    Some("stop") | Some("discontinue") => {
                        marks[i].volta = Some(open_ending.clone().unwrap_or(nums));
                        open_ending = None;
                    }
                    _ => {}
                }
            }
        }
        if marks[i].volta.is_none() {
            if let Some(v) = &open_ending {
                marks[i].volta = Some(v.clone());
            }
        }
        for s in m.descendants().filter(|c| c.has_tag_name("sound")) {
            if s.attribute("segno").is_some() {
                marks[i].segno = true;
            }
            if s.attribute("coda").is_some() {
                marks[i].coda = true;
            }
            if s.attribute("tocoda").is_some() {
                marks[i].to_coda = true;
            }
            if s.attribute("fine").is_some() {
                marks[i].fine = true;
            }
            if s.attribute("dalsegno").is_some() {
                marks[i].jump = Some(Jump::Ds);
            }
            if s.attribute("dacapo").is_some() {
                marks[i].jump = Some(Jump::Dc);
            }
        }
    }
    // MusicXML does not write "al Fine / al Coda" on the jump: we infer it
    // from the presence of To Coda / Fine marks in the piece.
    let has_tocoda = marks.iter().any(|m| m.to_coda);
    let has_fine = marks.iter().any(|m| m.fine);
    for mk in marks.iter_mut() {
        mk.jump = mk.jump.map(|j| {
            let ds = matches!(j, Jump::Ds | Jump::DsAlCoda | Jump::DsAlFine);
            if has_tocoda {
                if ds {
                    Jump::DsAlCoda
                } else {
                    Jump::DcAlCoda
                }
            } else if has_fine {
                if ds {
                    Jump::DsAlFine
                } else {
                    Jump::DcAlFine
                }
            } else if ds {
                Jump::Ds
            } else {
                Jump::Dc
            }
        });
    }
    unroll(&marks)
}

fn parse_musicxml(xml: &str) -> Result<Midi, String> {
    check_nesting(xml)?;
    let safe_xml = mask_external_musicxml_doctype(xml)?;
    let opts = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 5_000_000, // bounds the memory cost of a forged XML
    };
    let doc = roxmltree::Document::parse_with_options(&safe_xml, opts)
        .map_err(|e| format!("invalid XML: {}", e))?;
    let root = doc.root_element();
    if !root.has_tag_name("score-partwise") {
        return Err("unsupported MusicXML (expected: score-partwise)".into());
    }
    let tpb = exact_tick_base(&doc)?;

    #[derive(Clone, Debug, Default)]
    struct PartInfo {
        name: String,
        instruments: BTreeMap<String, InstrumentInfo>,
    }

    let mut part_info: HashMap<String, PartInfo> = HashMap::new();
    if let Some(list) = root.children().find(|n| n.has_tag_name("part-list")) {
        for sp in list.children().filter(|n| n.has_tag_name("score-part")) {
            let Some(part_id) = sp.attribute("id") else {
                continue;
            };
            let name = sp
                .children()
                .find(|node| node.has_tag_name("part-name"))
                .map(crate::engine::musescore::deep_text)
                .map(|value| crate::engine::musescore::collapse_ws(&value))
                .unwrap_or_default();
            let mut instruments = BTreeMap::new();
            for score_instrument in sp
                .children()
                .filter(|node| node.has_tag_name("score-instrument"))
            {
                let Some(id) = score_instrument.attribute("id") else {
                    continue;
                };
                let instrument_name = score_instrument
                    .children()
                    .find(|node| node.has_tag_name("instrument-name"))
                    .map(crate::engine::musescore::deep_text)
                    .filter(|value| !value.is_empty());
                instruments.insert(
                    id.to_string(),
                    InstrumentInfo {
                        id: Some(id.to_string()),
                        name: instrument_name,
                        ..InstrumentInfo::default()
                    },
                );
            }
            for midi_instrument in sp
                .children()
                .filter(|node| node.has_tag_name("midi-instrument"))
            {
                let Some(id) = midi_instrument.attribute("id") else {
                    continue;
                };
                let instrument =
                    instruments
                        .entry(id.to_string())
                        .or_insert_with(|| InstrumentInfo {
                            id: Some(id.to_string()),
                            ..InstrumentInfo::default()
                        });
                instrument.source_channel = child_i64(midi_instrument, "midi-channel")
                    .and_then(|value| i32::try_from(value).ok());
                instrument.channel = instrument
                    .source_channel
                    .filter(|value| (1..=16).contains(value))
                    .and_then(|value| u8::try_from(value - 1).ok());
                instrument.source_program = child_i64(midi_instrument, "midi-program")
                    .and_then(|value| i32::try_from(value).ok());
                instrument.program = instrument
                    .source_program
                    .filter(|value| (1..=128).contains(value))
                    .and_then(|value| u8::try_from(value - 1).ok());
                if let Some(bank) = child_i64(midi_instrument, "midi-bank")
                    .filter(|value| (1..=16_384).contains(value))
                    .and_then(|value| u16::try_from(value - 1).ok())
                {
                    instrument.bank_msb = Some((bank >> 7) as u8);
                    instrument.bank_lsb = Some((bank & 0x7f) as u8);
                }
                instrument.volume = midi_instrument
                    .children()
                    .find(|node| node.has_tag_name("volume"))
                    .and_then(|node| node.text())
                    .and_then(|value| value.trim().parse::<f64>().ok());
                instrument.pan = midi_instrument
                    .children()
                    .find(|node| node.has_tag_name("pan"))
                    .and_then(|node| node.text())
                    .and_then(|value| value.trim().parse::<f64>().ok());
                instrument.midi_unpitched = child_i64(midi_instrument, "midi-unpitched")
                    .filter(|value| (1..=128).contains(value))
                    .and_then(|value| u8::try_from(value).ok());
                instrument.percussion =
                    instrument.channel == Some(9) || instrument.midi_unpitched.is_some();
            }
            part_info.insert(part_id.to_string(), PartInfo { name, instruments });
        }
    }

    let mut tracks = Vec::new();
    let mut global_events = Vec::new();
    for (part_index, part) in root
        .children()
        .filter(|n| n.has_tag_name("part"))
        .enumerate()
    {
        let part_id = part
            .attribute("id")
            .map(str::to_string)
            .unwrap_or_else(|| format!("part-{}", part_index + 1));
        let info = part_info.get(&part_id).cloned().unwrap_or_default();
        let measures: Vec<_> = part
            .children()
            .filter(|n| n.has_tag_name("measure"))
            .collect();
        let mut divisions_at_measure = Vec::with_capacity(measures.len());
        let mut carried_divisions = 1u32;
        for measure in &measures {
            divisions_at_measure.push(carried_divisions);
            let mut declared_divisions = None;
            for attributes in measure
                .children()
                .filter(|node| node.has_tag_name("attributes"))
            {
                if let Some(value) =
                    strict_child_i64(attributes, "divisions", "MusicXML attributes")?
                {
                    if value <= 0 {
                        return Err("MusicXML divisions must be positive".into());
                    }
                    declared_divisions.get_or_insert(value);
                }
            }
            if let Some(value) = declared_divisions {
                carried_divisions = u32::try_from(value)
                    .map_err(|_| "MusicXML divisions exceed the supported range")?;
                if let Some(entry) = divisions_at_measure.last_mut() {
                    *entry = carried_divisions;
                }
            }
        }
        let mut voice_events = MusicXmlVoiceEvents::new();
        let mut active_ties: HashMap<MusicXmlTieKey, String> = HashMap::new();
        let mut previous_measure = None;
        let mut mstart: i64 = 0;

        for &(mi, pass) in playback_order_mxl(&measures)?.iter() {
            if previous_measure.is_some_and(|previous| mi != previous + 1) {
                mark_unresolved_ties_source_only(&mut voice_events, &active_ties);
                active_ties.clear();
            }
            previous_measure = Some(mi);
            let measure = measures[mi];
            let mut local_div = divisions_at_measure[mi];
            let mut pos: i64 = mstart;
            let mut maxpos: i64 = mstart;
            let mut last_onset: i64 = mstart;
            let mut last_chord_id = String::new();
            let mut chord_member = 0usize;
            let mut note_index = 0usize;
            for (element_index, node) in measure.children().filter(|n| n.is_element()).enumerate() {
                match node.tag_name().name() {
                    "attributes" => {
                        if let Some(value) =
                            strict_child_i64(node, "divisions", "MusicXML attributes")?
                        {
                            if value <= 0 {
                                return Err("MusicXML divisions must be positive".into());
                            }
                            local_div = u32::try_from(value)
                                .map_err(|_| "MusicXML divisions exceed the supported range")?;
                        }
                        if let Some(time) = node.children().find(|n| n.has_tag_name("time")) {
                            let (num, den) = parse_time_signature(time)?;
                            push_global_event(
                                &mut global_events,
                                checked_tick(pos)?,
                                Kind::TimeSig {
                                    num,
                                    den,
                                    clocks_per_click: None,
                                    notated_32nds: None,
                                },
                            );
                        }
                    }
                    "direction" | "sound" => {
                        if let Some(micros) = direction_tempo(node)? {
                            let tick = if node.has_tag_name("direction") {
                                direction_tick(node, pos, tpb, local_div)?
                            } else {
                                checked_tick(pos)?
                            };
                            push_global_event(&mut global_events, tick, Kind::Tempo(micros));
                        }
                    }
                    "note" => {
                        let is_chord = node.children().any(|n| n.has_tag_name("chord"));
                        if is_chord {
                            chord_member = chord_member.checked_add(1).ok_or_else(|| {
                                "MusicXML chord member index exceeds the supported range"
                                    .to_string()
                            })?;
                        } else {
                            chord_member = 0;
                        }
                        let is_rest = node.children().any(|n| n.has_tag_name("rest"));
                        let is_grace = node.children().any(|child| child.has_tag_name("grace"));
                        let source_duration = note_source_duration(node, is_grace)?;
                        let duration = scale_duration(source_duration, tpb, local_div)?;
                        let onset = if is_chord { last_onset } else { pos };
                        if !is_rest {
                            let voice = node
                                .children()
                                .find(|child| child.has_tag_name("voice"))
                                .and_then(|child| child.text())
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .unwrap_or("1")
                                .to_string();
                            let staff = node
                                .children()
                                .find(|child| child.has_tag_name("staff"))
                                .and_then(|child| child.text())
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .unwrap_or("1")
                                .to_string();
                            let source_id =
                                format!("musicxml:{part_id}:measure:{mi}:note:{note_index}");
                            if !is_chord {
                                last_chord_id = format!(
                                    "musicxml:{part_id}:measure:{mi}:chord:{element_index}"
                                );
                            }
                            let instrument_id = node
                                .children()
                                .find(|child| child.has_tag_name("instrument"))
                                .and_then(|child| child.attribute("id"))
                                .map(str::to_string);
                            let instrument = instrument_id
                                .as_ref()
                                .and_then(|id| info.instruments.get(id))
                                .or_else(|| {
                                    (info.instruments.len() == 1)
                                        .then(|| info.instruments.values().next())
                                        .flatten()
                                });
                            let unpitched_node = node
                                .children()
                                .find(|child| child.has_tag_name("unpitched"));
                            let unpitched = unpitched_node.map(|unpitched| UnpitchedInfo {
                                instrument_id: instrument_id.clone(),
                                display_step: unpitched
                                    .children()
                                    .find(|child| child.has_tag_name("display-step"))
                                    .and_then(|child| child.text())
                                    .map(|value| value.trim().to_string()),
                                display_octave: child_i64(unpitched, "display-octave")
                                    .and_then(|value| i8::try_from(value).ok()),
                                midi_unpitched: instrument
                                    .and_then(|instrument| instrument.midi_unpitched),
                            });
                            let pitch = if let Some(pitch_node) =
                                node.children().find(|n| n.has_tag_name("pitch"))
                            {
                                Some(pitched_key(pitch_node)?)
                            } else if unpitched_node.is_some() {
                                instrument
                                    .and_then(|instrument| instrument.midi_unpitched)
                                    .and_then(|value| value.checked_sub(1))
                            } else {
                                return Err(format!(
                                    "MusicXML note {source_id:?} has neither pitch nor unpitched"
                                ));
                            };
                            let events = voice_events
                                .entry((staff.clone(), voice.clone(), chord_member))
                                .or_default();
                            let on = checked_tick(onset)?;
                            let off = checked_tick(
                                onset
                                    .checked_add(duration)
                                    .ok_or_else(|| "MusicXML note timing overflow".to_string())?,
                            )?;
                            let channel = instrument.and_then(|instrument| instrument.channel);
                            let (tie_starts, tie_stops) = note_ties(node);
                            let tie_pitch = pitch
                                .map(|value| format!("midi:{value}"))
                                .or_else(|| {
                                    unpitched.as_ref().map(|value| {
                                        format!(
                                            "unpitched:{}:{}:{}",
                                            value.display_step.as_deref().unwrap_or("?"),
                                            value
                                                .display_octave
                                                .map(|octave| octave.to_string())
                                                .as_deref()
                                                .unwrap_or("?"),
                                            value
                                                .midi_unpitched
                                                .map(|pitch| pitch.to_string())
                                                .as_deref()
                                                .unwrap_or("?")
                                        )
                                    })
                                })
                                .unwrap_or_else(|| format!("unknown:{mi}:{note_index}"));
                            let tie_key = (
                                staff.clone(),
                                voice.clone(),
                                chord_member,
                                tie_pitch,
                                instrument_id.clone(),
                            );
                            let continued_source = if tie_stops {
                                active_ties.get(&tie_key).cloned()
                            } else {
                                None
                            };
                            if let Some(source_id) = &continued_source {
                                let previous_off = events.iter_mut().rev().find(|event| {
                                    matches!(
                                        &event.kind,
                                        Kind::NoteOff(note_off)
                                            if note_off.source_id.as_ref() == Some(source_id)
                                    )
                                });
                                let previous_off = previous_off.ok_or_else(|| {
                                    format!(
                                        "MusicXML tie target {source_id:?} has no matching note-off"
                                    )
                                })?;
                                previous_off.tick = off;
                            }
                            // A tie continuation is retained as a source-only
                            // note while the first note's playback duration is
                            // extended across the chain. This preserves every
                            // source occurrence without projecting a repeated
                            // Synthesizer V attack.
                            let playback_pitch = if tie_stops { None } else { pitch };
                            push_event(
                                events,
                                on,
                                Kind::NoteOn(NoteOn {
                                    channel,
                                    key: playback_pitch,
                                    velocity: None,
                                    source: NoteSource {
                                        id: source_id.clone(),
                                        part_id: Some(part_id.clone()),
                                        staff_id: Some(staff.clone()),
                                        voice: Some(voice),
                                        chord_id: Some(last_chord_id.clone()),
                                        instrument_id,
                                        occurrence: pass,
                                        grace: is_grace,
                                        unpitched,
                                    },
                                    lyrics: note_lyrics(node, &source_id),
                                }),
                            );
                            push_event(
                                events,
                                off,
                                Kind::NoteOff(NoteOff {
                                    channel,
                                    key: playback_pitch,
                                    velocity: None,
                                    source_id: Some(source_id.clone()),
                                }),
                            );
                            if tie_stops {
                                active_ties.remove(&tie_key);
                            }
                            if tie_starts {
                                active_ties.insert(tie_key, continued_source.unwrap_or(source_id));
                            }
                        }
                        if !is_chord {
                            last_onset = onset;
                            pos = pos
                                .checked_add(duration)
                                .ok_or_else(|| "MusicXML cursor overflow".to_string())?;
                        }
                        note_index += 1;
                    }
                    "backup" => {
                        let duration =
                            scale_duration(movement_duration(node, "backup")?, tpb, local_div)?;
                        let next = pos
                            .checked_sub(duration)
                            .ok_or_else(|| "MusicXML backup underflows the timeline".to_string())?;
                        if next < mstart {
                            return Err("MusicXML backup moves before the measure start".into());
                        }
                        pos = next;
                    }
                    "forward" => {
                        let duration =
                            scale_duration(movement_duration(node, "forward")?, tpb, local_div)?;
                        pos = pos
                            .checked_add(duration)
                            .ok_or_else(|| "MusicXML cursor overflow".to_string())?;
                    }
                    _ => {}
                }
                if pos > maxpos {
                    maxpos = pos;
                }
            }
            mstart = maxpos;
        }
        mark_unresolved_ties_source_only(&mut voice_events, &active_ties);

        let track_count = voice_events.len();
        for ((staff, voice, chord_member), mut events) in voice_events {
            events.sort_by_key(|event| (event.tick, event.order));
            if !events
                .iter()
                .any(|event| matches!(event.kind, Kind::NoteOn(_)))
            {
                continue;
            }
            let has_lyrics = events
                .iter()
                .any(|event| matches!(&event.kind, Kind::NoteOn(note) if !note.lyrics.is_empty()));
            let has_unpitched = events.iter().any(|event| {
                matches!(&event.kind, Kind::NoteOn(note) if note.source.unpitched.is_some())
            });
            let has_pitched = events
                .iter()
                .any(|event| matches!(&event.kind, Kind::NoteOn(note) if note.key.is_some() && note.source.unpitched.is_none()));
            let role_hint = if has_lyrics {
                TrackRoleHint::Vocal
            } else if has_unpitched && has_pitched {
                TrackRoleHint::Mixed
            } else if has_unpitched {
                TrackRoleHint::Percussion
            } else {
                TrackRoleHint::Instrumental
            };
            let suffix = if track_count > 1 {
                if chord_member == 0 {
                    format!(" — staff {staff}, voice {voice}")
                } else {
                    format!(
                        " — staff {staff}, voice {voice}, chord member {}",
                        chord_member + 1
                    )
                }
            } else {
                String::new()
            };
            let instruments: Vec<_> = info.instruments.values().cloned().collect();
            tracks.push(Track {
                id: if chord_member == 0 {
                    format!("musicxml:{part_id}:staff:{staff}:voice:{voice}")
                } else {
                    format!(
                        "musicxml:{part_id}:staff:{staff}:voice:{voice}:chord-member:{}",
                        chord_member + 1
                    )
                },
                name: format!(
                    "{}{suffix}",
                    if info.name.is_empty() {
                        part_id.as_str()
                    } else {
                        info.name.as_str()
                    }
                ),
                source: TrackSource {
                    source_track: tracks.len(),
                    part_id: Some(part_id.clone()),
                    staff_id: Some(staff),
                    voice: Some(voice),
                },
                role_hint,
                text_profile: MidiTextProfile::Generic,
                instrument: instruments.first().cloned(),
                instruments,
                events,
            });
        }
    }

    if !global_events.is_empty() {
        if tracks.is_empty() {
            tracks.push(Track {
                id: "musicxml:metadata".into(),
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
        return Err("no usable part in the MusicXML".into());
    }
    Ok(Midi {
        ticks_per_beat: tpb,
        time_base: TimeBase::PulsesPerQuarter(tpb),
        format: 1,
        source_format: SourceFormat::MusicXml,
        tracks,
    })
}

fn checked_tick(value: i64) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| "MusicXML tick exceeds the supported range".into())
}

fn push_event(events: &mut Vec<Event>, tick: u32, kind: Kind) {
    let order = events.len() as u32;
    events.push(Event::new(tick, order, kind));
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

    fn mxl(lyric_inner: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="3.1">
  <part-list>
    <score-part id="P1"><part-name>Voice</part-name></score-part>
  </part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>480</divisions></attributes>
      <note>
        <pitch><step>C</step><octave>4</octave></pitch>
        <duration>480</duration>
        <lyric number="1">{}</lyric>
      </note>
    </measure>
  </part>
</score-partwise>"#,
            lyric_inner
        )
    }

    fn score_with_measure_body(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list>
    <score-part id="P1"><part-name>Voice</part-name></score-part>
  </part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>480</divisions></attributes>
      {body}
    </measure>
  </part>
</score-partwise>"#
        )
    }

    fn zipped_score(name: &str, bytes: &[u8]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn windows_1252(value: &str) -> Vec<u8> {
        value
            .chars()
            .map(|character| match character {
                'é' => 0xe9,
                '’' => 0x92,
                character if character.is_ascii() => character as u8,
                character => panic!("test encoder does not cover {character:?}"),
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
        let midi = parse(mxl("<syllabic>single</syllabic><text>la</text>").as_bytes()).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["la"]);
    }

    #[test]
    fn every_present_divisions_value_must_be_valid_and_positive() {
        for value in ["", "0", "-1", "not-a-number", "480.5"] {
            let xml = mxl("<text>la</text>").replace(
                "<divisions>480</divisions>",
                &format!("<divisions>{value}</divisions>"),
            );
            let error = parse(xml.as_bytes()).expect_err("invalid divisions must never be ignored");
            assert!(
                error.to_ascii_lowercase().contains("divisions"),
                "unexpected error for {value:?}: {error}"
            );
        }
    }

    #[test]
    fn non_grace_note_duration_is_required_positive_and_never_clamped() {
        for replacement in [
            String::new(),
            "<duration>0</duration>".into(),
            "<duration>-1</duration>".into(),
            "<duration>invalid</duration>".into(),
        ] {
            let xml = mxl("<text>la</text>").replace("<duration>480</duration>", &replacement);
            let error =
                parse(xml.as_bytes()).expect_err("normal-note duration must fail explicitly");
            assert!(
                error.to_ascii_lowercase().contains("duration"),
                "unexpected error for {replacement:?}: {error}"
            );
        }

        let xml = mxl("<text>long</text>")
            .replace("<duration>480</duration>", "<duration>100000001</duration>");
        let midi = parse(xml.as_bytes()).expect("large exact duration must not be clamped");
        let off = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| matches!(event.kind, Kind::NoteOff(_)).then_some(event.tick))
            .unwrap();
        assert_eq!(off, 100_000_001);
    }

    #[test]
    fn only_real_grace_notes_may_omit_or_zero_the_duration() {
        for duration in ["", "<duration>0</duration>"] {
            let xml = mxl("<text>grace</text>")
                .replace(
                    "<pitch><step>C</step><octave>4</octave></pitch>",
                    "<grace/><pitch><step>C</step><octave>4</octave></pitch>",
                )
                .replace("<duration>480</duration>", duration);
            let midi = parse(xml.as_bytes()).expect("true grace duration exception");
            let note = midi.tracks[0]
                .events
                .iter()
                .find_map(|event| match &event.kind {
                    Kind::NoteOn(note) => Some((event.tick, note)),
                    _ => None,
                })
                .unwrap();
            assert_eq!(note.0, 0);
            assert!(note.1.source.grace);
            assert!(midi.tracks[0]
                .events
                .iter()
                .any(|event| matches!(event.kind, Kind::NoteOff(_)) && event.tick == 0));
        }

        let xml = mxl("<text>grace</text>")
            .replace(
                "<pitch><step>C</step><octave>4</octave></pitch>",
                "<grace/><pitch><step>C</step><octave>4</octave></pitch>",
            )
            .replace("<duration>480</duration>", "<duration>1</duration>");
        let error = parse(xml.as_bytes()).expect_err("grace duration must not advance the cursor");
        assert!(error.to_ascii_lowercase().contains("grace"));
    }

    #[test]
    fn backup_and_forward_require_positive_durations_without_cursor_clamping() {
        for kind in ["backup", "forward"] {
            for duration in ["", "<duration>0</duration>", "<duration>-1</duration>"] {
                let body = format!(
                    "<note><pitch><step>C</step><octave>4</octave></pitch>\
                     <duration>480</duration></note><{kind}>{duration}</{kind}>"
                );
                let xml = score_with_measure_body(&body);
                let error =
                    parse(xml.as_bytes()).expect_err("movement duration must fail explicitly");
                assert!(
                    error.to_ascii_lowercase().contains("duration"),
                    "unexpected error for {kind}/{duration:?}: {error}"
                );
            }
        }

        let xml = score_with_measure_body(
            "<note><pitch><step>C</step><octave>4</octave></pitch>\
             <duration>480</duration></note><backup><duration>960</duration></backup>",
        );
        let error = parse(xml.as_bytes()).expect_err("backup before measure start must fail");
        assert!(
            error.to_ascii_lowercase().contains("measure start"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_or_out_of_range_pitched_notes_are_rejected() {
        let original = "<pitch><step>C</step><octave>4</octave></pitch>";
        for invalid in [
            "<pitch><octave>4</octave></pitch>",
            "<pitch><step>H</step><octave>4</octave></pitch>",
            "<pitch><step>C</step></pitch>",
            "<pitch><step>C</step><octave>invalid</octave></pitch>",
            "<pitch><step>C</step><alter>invalid</alter><octave>4</octave></pitch>",
            "<pitch><step>C</step><alter>0.5</alter><octave>4</octave></pitch>",
            "<pitch><step>C</step><octave>20</octave></pitch>",
            "<pitch><step>C</step><alter>200</alter><octave>4</octave></pitch>",
        ] {
            let xml = mxl("<text>la</text>").replace(original, invalid);
            let error = parse(xml.as_bytes()).expect_err("invalid pitch must not become key=None");
            assert!(
                error.to_ascii_lowercase().contains("pitch")
                    || error.to_ascii_lowercase().contains("step")
                    || error.to_ascii_lowercase().contains("octave")
                    || error.to_ascii_lowercase().contains("alter"),
                "unexpected error for {invalid}: {error}"
            );
        }

        let xml = mxl("<text>la</text>").replace(original, "");
        let error = parse(xml.as_bytes()).expect_err("pitched note needs pitch or unpitched");
        assert!(
            error.contains("neither pitch nor unpitched"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn absent_alter_defaults_to_zero_but_unmapped_unpitched_stays_source_only() {
        let pitched = parse(mxl("<text>la</text>").as_bytes()).unwrap();
        assert!(pitched.tracks[0]
            .events
            .iter()
            .any(|event| matches!(&event.kind, Kind::NoteOn(note) if note.key == Some(60))));

        let xml = score_with_measure_body(
            "<note>\
               <unpitched><display-step>C</display-step><display-octave>4</display-octave></unpitched>\
               <duration>480</duration>\
             </note>",
        );
        let unpitched = parse(xml.as_bytes()).expect("unmapped percussion is source-only");
        let note = unpitched.tracks[0]
            .events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some(note),
                _ => None,
            })
            .unwrap();
        assert_eq!(note.key, None);
        assert!(note.source.unpitched.is_some());
    }

    #[test]
    fn raw_and_zipped_windows_1252_follow_the_xml_declaration() {
        let xml = mxl("<text>l’été</text>").replace("UTF-8", "windows-1252");
        let bytes = windows_1252(&xml);
        let raw = parse(&bytes).expect("raw MusicXML uses its declared encoding");
        assert_eq!(lyrics_of(&raw), vec!["l’été"]);

        let archive = zipped_score("score.musicxml", &bytes);
        let zipped = parse(&archive).expect("zipped MusicXML uses the entry declaration");
        assert_eq!(lyrics_of(&zipped), vec!["l’été"]);
    }

    #[test]
    fn utf16_musicxml_is_detected_and_decoded() {
        let xml = mxl("<text>été</text>").replace("UTF-8", "UTF-16");
        let mut bytes = vec![0xff, 0xfe];
        for word in xml.encode_utf16() {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        assert!(looks_like_xml(&bytes));
        let midi = parse(&bytes).expect("UTF-16 MusicXML should decode from its BOM");
        assert_eq!(lyrics_of(&midi), vec!["été"]);
    }

    #[test]
    fn root_detection_is_not_limited_to_an_initial_byte_window() {
        let xml = format!(
            "{}<score-partwise version=\"3.1\"><part-list/></score-partwise>",
            " ".repeat(2_000)
        );
        assert!(looks_like_xml(xml.as_bytes()));
    }

    #[test]
    fn direction_offset_and_metronome_tempo_are_applied_exactly() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>480</divisions></attributes>
      <direction>
        <direction-type>
          <metronome>
            <beat-unit>eighth</beat-unit>
            <per-minute>120</per-minute>
          </metronome>
        </direction-type>
        <offset>240</offset>
      </direction>
      <note>
        <pitch><step>C</step><octave>4</octave></pitch>
        <duration>480</duration>
      </note>
    </measure>
  </part>
</score-partwise>"#;
        let midi = parse(xml.as_bytes()).unwrap();
        let tempos: Vec<_> = midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .filter_map(|event| match &event.kind {
                Kind::Tempo(micros) => Some((event.tick, *micros)),
                _ => None,
            })
            .collect();
        assert_eq!(tempos, vec![(240, 1_000_000)]);
    }

    #[test]
    fn sound_tempo_takes_priority_over_display_metronome() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>480</divisions></attributes>
      <direction>
        <direction-type>
          <metronome><beat-unit>quarter</beat-unit><per-minute>90</per-minute></metronome>
        </direction-type>
        <sound tempo="120"/>
      </direction>
      <note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration></note>
    </measure>
  </part>
</score-partwise>"#;
        let midi = parse(xml.as_bytes()).unwrap();
        assert!(midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .any(|event| matches!(event.kind, Kind::Tempo(500_000))));
    }

    #[test]
    fn additive_meter_is_summed_and_malformed_grouping_is_rejected() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes>
        <divisions>480</divisions>
        <time><beats>3+2</beats><beat-type>8</beat-type></time>
      </attributes>
      <note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration></note>
    </measure>
  </part>
</score-partwise>"#;
        let midi = parse(xml.as_bytes()).unwrap();
        assert!(midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .any(|event| matches!(event.kind, Kind::TimeSig { num: 5, den: 8, .. })));

        let malformed = xml.replace("<beats>3+2</beats>", "<beats>3+x</beats>");
        let error = parse(malformed.as_bytes()).expect_err("malformed grouping must not vanish");
        assert!(
            error.contains("additive") || error.contains("beats"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn ties_merge_playback_but_keep_continuation_source_only() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>480</divisions></attributes>
      <note>
        <pitch><step>C</step><octave>4</octave></pitch>
        <duration>480</duration>
        <tie type="start"/>
        <notations><tied type="start"/></notations>
        <lyric><text>hold</text></lyric>
      </note>
      <note>
        <pitch><step>C</step><octave>4</octave></pitch>
        <duration>480</duration>
        <tie type="stop"/>
        <notations><tied type="stop"/></notations>
      </note>
    </measure>
  </part>
</score-partwise>"#;
        let midi = parse(xml.as_bytes()).unwrap();
        let note_events: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some(("on", event.tick, note.key)),
                Kind::NoteOff(note) => Some(("off", event.tick, note.key)),
                _ => None,
            })
            .collect();
        assert_eq!(
            note_events,
            vec![
                ("on", 0, Some(60)),
                ("on", 480, None),
                ("off", 960, Some(60)),
                ("off", 960, None),
            ]
        );
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        let project = outcome.svp.unwrap();
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes[0].lyrics, "hold");
        assert_eq!(
            project.tracks[0].main_group.notes[0].duration,
            crate::engine::svp::BLICKS_PER_QUARTER as i64 * 2
        );
    }

    #[test]
    fn dangling_tie_is_source_only_instead_of_claimed_exact() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>480</divisions></attributes>
      <note>
        <pitch><step>C</step><octave>4</octave></pitch>
        <duration>480</duration>
        <tie type="start"/>
        <lyric><text>hold</text></lyric>
      </note>
    </measure>
  </part>
</score-partwise>"#;
        let midi = parse(xml.as_bytes()).unwrap();
        assert!(midi.tracks[0]
            .events
            .iter()
            .any(|event| matches!(&event.kind, Kind::NoteOn(note) if note.key.is_none())));
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        assert_eq!(outcome.placed, 0);
        assert!(outcome.svp.unwrap().tracks.is_empty());
    }

    #[test]
    fn explicit_musicxml_chord_members_become_separate_monophonic_tracks() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>480</divisions></attributes>
      <note>
        <pitch><step>C</step><octave>4</octave></pitch>
        <duration>480</duration>
        <lyric><text>lead</text></lyric>
      </note>
      <note>
        <chord/>
        <pitch><step>E</step><octave>4</octave></pitch>
        <duration>480</duration>
      </note>
    </measure>
  </part>
</score-partwise>"#;
        let midi = parse(xml.as_bytes()).unwrap();
        assert_eq!(midi.tracks.len(), 2);
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        let project = outcome.svp.unwrap();
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes[0].pitch, 60);
        assert_eq!(project.tracks[0].main_group.notes[0].lyrics, "lead");
    }

    #[test]
    fn elision_keeps_every_text_fragment() {
        // Two syllables sung on one note: <text>to</text><elision/><text>a</text>
        let midi = parse(
            mxl("<syllabic>end</syllabic><text>to</text><elision>\u{203f}</elision><syllabic>begin</syllabic><text>a</text>")
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(lyrics_of(&midi), vec!["to\u{203f}a"]);
        let lyric = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::NoteOn(note) => note.lyrics.first(),
                _ => None,
            })
            .expect("lyric remains attached");
        assert_eq!(
            lyric.fragments,
            vec![
                LyricFragment::Text("to".into()),
                LyricFragment::Elision("\u{203f}".into()),
                LyricFragment::Text("a".into())
            ]
        );
    }

    #[test]
    fn non_standard_nested_markup_is_tolerated() {
        let midi = parse(mxl("<text><i>hey</i></text>").as_bytes()).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["hey"]);
    }

    #[test]
    fn whitespace_elision_joins_two_words_with_a_space() {
        // Sibelius/Finale write <elision> </elision> when two whole words
        // share one note: "my love", not the nonword "mylove".
        let midi = parse(
            mxl("<syllabic>single</syllabic><text>my</text><elision> </elision><syllabic>single</syllabic><text>love</text>")
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(lyrics_of(&midi), vec!["my love"]);
    }

    #[test]
    fn trailing_space_inside_a_fragment_survives_elision() {
        // Some exporters put the joining space inside the first <text>.
        let midi = parse(
            mxl("<syllabic>single</syllabic><text>my </text><elision/><syllabic>single</syllabic><text>love</text>")
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(lyrics_of(&midi), vec!["my love"]);
    }

    #[test]
    fn official_external_doctype_is_accepted_without_enabling_dtds() {
        let xml = mxl("<text>let</text>").replace(
            "<score-partwise",
            "<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n<score-partwise",
        );
        let midi = parse(xml.as_bytes()).expect("external MusicXML doctype is safely masked");
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn internal_dtd_subset_and_entities_are_rejected() {
        let xml = mxl("<text>&word;</text>").replace(
            "<score-partwise",
            "<!DOCTYPE score-partwise [<!ENTITY word \"invented\">]>\n<score-partwise",
        );
        let error = match parse(xml.as_bytes()) {
            Err(error) => error,
            Ok(_) => panic!("internal entities must stay disabled"),
        };
        assert!(
            error.contains("internal DTD") || error.contains("entity"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn lyric_lane_number_is_not_a_repeat_pass() {
        let midi =
            parse(mxl("<text>one</text></lyric><lyric number=\"2\"><text>two</text>").as_bytes())
                .unwrap();
        let lyrics: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some(note.lyrics.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(lyrics.len(), 2);
        assert_eq!(lyrics[0].lane, "1");
        assert_eq!(lyrics[1].lane, "2");

        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        let project = outcome.svp.unwrap();
        assert_eq!(project.tracks.len(), 2);
        assert_eq!(project.tracks[0].main_group.notes[0].lyrics, "one");
        assert_eq!(project.tracks[1].main_group.notes[0].lyrics, "two");
    }

    #[test]
    fn repeat_occurrences_reemit_tempo_and_meter() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="3.1">
  <part-list>
    <score-part id="P1"><part-name>Voice</part-name></score-part>
  </part-list>
  <part id="P1">
    <measure number="1">
      <attributes>
        <divisions>480</divisions>
        <time><beats>3</beats><beat-type>4</beat-type></time>
      </attributes>
      <direction><sound tempo="90"/></direction>
      <barline location="left"><repeat direction="forward"/></barline>
      <note>
        <pitch><step>C</step><octave>4</octave></pitch>
        <duration>480</duration>
        <lyric><text>let</text></lyric>
      </note>
      <forward><duration>960</duration></forward>
      <barline location="right"><repeat direction="backward" times="2"/></barline>
    </measure>
  </part>
</score-partwise>"#;
        let midi = parse(xml.as_bytes()).unwrap();
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
        let orders: std::collections::BTreeSet<_> = midi.tracks[0]
            .events
            .iter()
            .map(|event| event.order)
            .collect();
        assert_eq!(orders.len(), midi.tracks[0].events.len());
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert_eq!(
            outcome
                .projection
                .source_ids
                .iter()
                .filter(|id| id.starts_with("note:musicxml:"))
                .count(),
            2
        );
        assert_eq!(
            outcome
                .projection
                .source_ids
                .iter()
                .filter(|id| id.starts_with("lyric:musicxml:"))
                .count(),
            2
        );
    }

    #[test]
    fn globals_survive_when_the_first_part_contains_only_rests() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="3.1">
  <part-list>
    <score-part id="P1"><part-name>Rest staff</part-name></score-part>
    <score-part id="P2"><part-name>Voice</part-name></score-part>
  </part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>480</divisions></attributes>
      <note><rest/><duration>480</duration></note>
    </measure>
  </part>
  <part id="P2">
    <measure number="1">
      <attributes>
        <divisions>480</divisions>
        <time><beats>6</beats><beat-type>8</beat-type></time>
      </attributes>
      <direction><sound tempo="72"/></direction>
      <note><pitch><step>D</step><octave>4</octave></pitch><duration>480</duration></note>
    </measure>
  </part>
</score-partwise>"#;
        let midi = parse(xml.as_bytes()).unwrap();
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
    fn deeply_nested_forged_xml_is_rejected_cleanly() {
        let mut xml = String::from("<score-partwise version=\"3.1\">");
        for _ in 0..250 {
            xml.push_str("<x>");
        }
        xml.push('y');
        for _ in 0..250 {
            xml.push_str("</x>");
        }
        xml.push_str("</score-partwise>");
        let err = match parse(xml.as_bytes()) {
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
    fn check_nesting_ignores_comments_cdata_and_quoted_attrs() {
        // None of these constructs may be miscounted as element depth.
        let xml = r#"<?xml es="<a><a><a>"?><!-- <a><a><a> --><root attr="<a><a>"><![CDATA[<a><a><a>]]><child/></root>"#;
        assert!(check_nesting(xml).is_ok());
    }
}
