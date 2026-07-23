//! MusicXML parser (.xml / .musicxml) and compressed MusicXML (.mxl).
//! Produces the same intermediate `Midi` structure as the MIDI parser, so the
//! whole multi-track conversion logic can be reused.
use crate::engine::midi::{unroll, Event, Jump, Kind, MeasureMarks, Midi};
use std::io::Read;

pub fn is_zip(data: &[u8]) -> bool {
    data.len() >= 2 && &data[0..2] == b"PK"
}

pub fn looks_like_xml(data: &[u8]) -> bool {
    let n = data.len().min(400);
    let head = String::from_utf8_lossy(&data[..n]);
    let t = head.trim_start();
    t.starts_with("<?xml") || t.contains("score-partwise") || t.contains("score-timewise")
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
        let s = read_zip_entry_capped(&mut cf).unwrap_or_default(); // capped (anti zip-bomb)
        if let Ok(doc) = roxmltree::Document::parse(&s) {
            if let Some(rf) = doc.descendants().find(|n| n.has_tag_name("rootfile")) {
                if let Some(fp) = rf.attribute("full-path") {
                    root = Some(fp.to_string());
                }
            }
        }
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
    let mut s = String::new();
    f.take(MAX + 1).read_to_string(&mut s).map_err(|e| e.to_string())?;
    if s.len() as u64 > MAX {
        return Err("abnormally large archive (rejected for safety)".into());
    }
    Ok(s)
}

pub fn parse(data: &[u8]) -> Result<Midi, String> {
    let xml = if is_zip(data) {
        extract_mxl(data)?
    } else {
        String::from_utf8_lossy(data).into_owned()
    };
    parse_musicxml(&xml)
}

fn step_semitone(s: &str) -> i32 {
    match s {
        "C" => 0, "D" => 2, "E" => 4, "F" => 5, "G" => 7, "A" => 9, "B" => 11, _ => 0,
    }
}

fn scale(pos: i64, tpb: u16, local_div: u32) -> u32 {
    if local_div == 0 {
        return pos.max(0) as u32;
    }
    ((pos.max(0) as i128 * tpb as i128) / local_div as i128) as u32
}

fn child_i64(n: roxmltree::Node, tag: &str) -> Option<i64> {
    n.children()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.text())
        .and_then(|t| t.trim().parse::<i64>().ok())
}

/// Text of verse `pass+1` of a note (number attribute of <lyric>),
/// falling back to verse 1 on later passes.
fn verse_lyric_mxl(note: roxmltree::Node, pass: u32) -> Option<String> {
    let pick = |v: u32| {
        note.children()
            .filter(|c| c.has_tag_name("lyric"))
            .find(|ly| {
                ly.attribute("number").and_then(|t| t.parse::<u32>().ok()).unwrap_or(1) == v
            })
            .and_then(|ly| ly.children().find(|c| c.has_tag_name("text")))
            .and_then(|t| t.text())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    pick(pass + 1).or_else(|| if pass > 0 { pick(1) } else { None })
}

/// Playback order of MusicXML measures: repeats (<repeat>), voltas
/// (<ending>), jumps (<sound> attributes: segno/dalsegno/dacapo/coda/
/// tocoda/fine).
fn playback_order_mxl(measures: &[roxmltree::Node]) -> Vec<(usize, u32)> {
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
                if ds { Jump::DsAlCoda } else { Jump::DcAlCoda }
            } else if has_fine {
                if ds { Jump::DsAlFine } else { Jump::DcAlFine }
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
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 5_000_000, // bounds the memory cost of a forged XML
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(xml, opts)
        .map_err(|e| format!("invalid XML: {}", e))?;
    let root = doc.root_element();
    if !root.has_tag_name("score-partwise") {
        return Err("unsupported MusicXML (expected: score-partwise)".into());
    }
    // global divisions (for the tick base)
    let divisions = doc
        .descendants()
        .find(|n| n.has_tag_name("divisions"))
        .and_then(|d| d.text())
        .and_then(|t| t.trim().parse::<u32>().ok())
        .unwrap_or(480);
    let tpb = divisions.clamp(1, 65535) as u16;

    // Part names (part-list > score-part > part-name), indexed by id.
    let mut part_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Some(list) = root.children().find(|n| n.has_tag_name("part-list")) {
        for sp in list.children().filter(|n| n.has_tag_name("score-part")) {
            if let (Some(id), Some(name)) = (
                sp.attribute("id"),
                sp.children()
                    .find(|n| n.has_tag_name("part-name"))
                    .and_then(|n| n.text()),
            ) {
                let name = name.trim();
                if !name.is_empty() {
                    part_names.insert(id.to_string(), name.to_string());
                }
            }
        }
    }

    let mut tracks: Vec<Vec<Event>> = Vec::new();

    for part in root.children().filter(|n| n.has_tag_name("part")) {
        let mut events: Vec<Event> = Vec::new();
        if let Some(name) = part.attribute("id").and_then(|id| part_names.get(id)) {
            events.push(Event { tick: 0, kind: Kind::TrackName(name.clone()) });
        }
        let mut local_div: u32 = divisions;
        let measures: Vec<_> = part.children().filter(|n| n.has_tag_name("measure")).collect();
        let mut mstart: i64 = 0;

        // Unrolled repeats: pass k -> verse k (fallback to verse 1).
        for &(mi, pass) in playback_order_mxl(&measures).iter() {
            let measure = measures[mi];
            let mut pos: i64 = mstart;
            let mut maxpos: i64 = mstart;
            let mut last_onset: i64 = mstart;
            for node in measure.children().filter(|n| n.is_element()) {
                match node.tag_name().name() {
                    "attributes" => {
                        if let Some(v) = child_i64(node, "divisions") {
                            if v > 0 {
                                local_div = v as u32;
                            }
                        }
                        if let Some(time) = node.children().find(|n| n.has_tag_name("time")) {
                            if pass == 0 {
                                let num = child_i64(time, "beats").unwrap_or(4).clamp(1, 255) as u8;
                                let den = child_i64(time, "beat-type").unwrap_or(4).clamp(1, 1024) as u16;
                                events.push(Event {
                                    tick: scale(pos, tpb, local_div),
                                    kind: Kind::TimeSig { num, den },
                                });
                            }
                        }
                    }
                    "direction" | "sound" => {
                        let snd = if node.has_tag_name("sound") {
                            Some(node)
                        } else {
                            node.descendants().find(|n| n.has_tag_name("sound"))
                        };
                        if let Some(s) = snd {
                            if let Some(t) = s.attribute("tempo").and_then(|v| v.parse::<f64>().ok()) {
                                if t > 0.0 && pass == 0 {
                                    events.push(Event {
                                        tick: scale(pos, tpb, local_div),
                                        kind: Kind::Tempo((60_000_000.0 / t) as u32),
                                    });
                                }
                            }
                        }
                    }
                    "note" => {
                        let is_chord = node.children().any(|n| n.has_tag_name("chord"));
                        let is_rest = node.children().any(|n| n.has_tag_name("rest"));
                        let dur = child_i64(node, "duration").unwrap_or(0).clamp(0, 100_000_000);
                        let onset = if is_chord { last_onset } else { pos };
                        if !is_rest {
                            if let Some(p) = node.children().find(|n| n.has_tag_name("pitch")) {
                                let step = p
                                    .children()
                                    .find(|n| n.has_tag_name("step"))
                                    .and_then(|n| n.text())
                                    .unwrap_or("C");
                                let alter = child_i64(p, "alter").unwrap_or(0).clamp(-4, 4) as i32;
                                let octave = child_i64(p, "octave").unwrap_or(4).clamp(-2, 12) as i32;
                                let pitch =
                                    ((octave + 1) * 12 + step_semitone(step) + alter).clamp(0, 127) as u8;
                                let on = scale(onset, tpb, local_div);
                                let off = scale(onset + dur.max(1), tpb, local_div);
                                events.push(Event { tick: on, kind: Kind::NoteOn(pitch) });
                                events.push(Event { tick: off.max(on + 1), kind: Kind::NoteOff(pitch) });
                                if let Some(txt) = verse_lyric_mxl(node, pass) {
                                    events.push(Event { tick: on, kind: Kind::Lyrics(txt) });
                                }
                            }
                        }
                        if !is_chord {
                            last_onset = onset;
                            pos += dur;
                        }
                    }
                    "backup" => {
                        pos = (pos - child_i64(node, "duration").unwrap_or(0).clamp(0, 100_000_000)).max(0);
                    }
                    "forward" => {
                        pos += child_i64(node, "duration").unwrap_or(0).clamp(0, 100_000_000);
                    }
                    _ => {}
                }
                if pos > maxpos {
                    maxpos = pos;
                }
            }
            mstart = maxpos;
        }
        // Stable sort by tick (backup/forward can shuffle order), note_on before note_off guaranteed.
        events.sort_by_key(|e| e.tick);
        if events.iter().any(|e| matches!(e.kind, Kind::NoteOn(_) | Kind::Lyrics(_))) {
            tracks.push(events);
        }
    }

    if tracks.is_empty() {
        return Err("no usable part in the MusicXML".into());
    }
    Ok(Midi { ticks_per_beat: tpb, tracks })
}
