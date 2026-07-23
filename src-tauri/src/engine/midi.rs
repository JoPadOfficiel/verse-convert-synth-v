//! MIDI parser (pure Rust, no external dependencies).
//! Port of the reference Python engine's parser, with the same robustness
//! guards (running-status cleared on meta/sysex, unknown chunks skipped, hard
//! anti-corruption bound).

pub struct Midi {
    pub ticks_per_beat: u16,
    pub tracks: Vec<Vec<Event>>,
}

/// Structure markers of a score measure.
#[derive(Default, Clone)]
pub struct MeasureMarks {
    pub start_repeat: bool,
    pub end_repeat: u32,         // 0 = none; otherwise total number of passes (>= 2)
    pub volta: Option<Vec<u32>>, // pass numbers (1-based) on which the measure is played
    pub segno: bool,
    pub coda: bool,   // target of the "al Coda" jump (coda symbol)
    pub to_coda: bool, // "To Coda" point
    pub fine: bool,
    pub jump: Option<Jump>, // taken once, after the measure
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Jump {
    DsAlFine,
    DsAlCoda,
    Ds,
    DcAlFine,
    DcAlCoda,
    Dc,
}

/// Unrolls a score into its actual playback order: repeats, voltas
/// (alternate endings), D.S./D.C. (al Fine / al Coda), To Coda, Fine.
/// Returns (measure_index, pass_number 0-based) pairs -- the pass determines
/// which verse is sung. Usual conventions: after a D.S./D.C. jump, repeats
/// are not replayed and only the last endings are played.
pub fn unroll(marks: &[MeasureMarks]) -> Vec<(usize, u32)> {
    let n = marks.len();
    if n == 0 {
        return vec![];
    }
    let mut order: Vec<(usize, u32)> = Vec::with_capacity(n * 2);
    let mut emitted = vec![0u32; n];
    let mut jumps_left: Vec<u32> =
        marks.iter().map(|m| m.end_repeat.saturating_sub(1)).collect();
    let mut i = 0usize;
    let mut repeat_start = 0usize;
    let mut region_pass: u32 = 1; // current pass (1-based) within the active repeat
    let mut after_jump = false;
    let mut jump_taken = false;
    let mut jump_mode: Option<Jump> = None;
    let cap = n * 8 + 16; // anti-loop guard for pathological data
    while i < n && order.len() < cap {
        let m = &marks[i];
        if m.start_repeat {
            if emitted[i] == 0 {
                region_pass = 1; // "fresh" entry into a new repeat
            }
            repeat_start = i;
        }
        // volta: the measure only plays on the listed passes
        let play = match &m.volta {
            Some(v) if after_jump => v.iter().any(|&x| x >= 2), // last endings
            Some(v) => v.contains(&region_pass),
            None => true,
        };
        if play {
            order.push((i, emitted[i]));
            emitted[i] += 1;

            if after_jump
                && m.fine
                && matches!(jump_mode, Some(Jump::DsAlFine) | Some(Jump::DcAlFine))
            {
                break; // "al Fine": we stop at the Fine
            }
            if after_jump
                && m.to_coda
                && matches!(jump_mode, Some(Jump::DsAlCoda) | Some(Jump::DcAlCoda))
            {
                if let Some(c) = marks.iter().position(|x| x.coda) {
                    i = c; // "To Coda": jump to the coda symbol
                    continue;
                }
            }
            if !jump_taken {
                if let Some(j) = m.jump {
                    jump_taken = true;
                    after_jump = true;
                    jump_mode = Some(j);
                    i = match j {
                        Jump::DsAlFine | Jump::DsAlCoda | Jump::Ds => {
                            marks.iter().position(|x| x.segno).unwrap_or(0)
                        }
                        _ => 0, // D.C.: back to the beginning
                    };
                    continue;
                }
            }
            if !after_jump && jumps_left[i] > 0 {
                jumps_left[i] -= 1;
                region_pass += 1;
                i = repeat_start;
                continue;
            }
        }
        i += 1;
    }
    order
}

pub struct Event {
    pub tick: u32,
    pub kind: Kind,
}

pub enum Kind {
    NoteOn(u8),
    NoteOff(u8),
    Tempo(u32),
    TimeSig { num: u8, den: u16 },
    Text(String),
    Lyrics(String),
    TrackName(String),
}

fn be_u32(d: &[u8], p: usize) -> u32 {
    ((d[p] as u32) << 24) | ((d[p + 1] as u32) << 16) | ((d[p + 2] as u32) << 8) | d[p + 3] as u32
}
fn be_u16(d: &[u8], p: usize) -> u16 {
    ((d[p] as u16) << 8) | d[p + 1] as u16
}

fn read_vlq(d: &[u8], mut p: usize, end: usize) -> (u32, usize) {
    let mut v: u32 = 0;
    while p < end {
        let c = d[p];
        p += 1;
        v = (v << 7) | (c & 0x7f) as u32;
        if c & 0x80 == 0 {
            break;
        }
    }
    (v, p)
}

/// UTF-8 if valid, otherwise Latin-1 (each byte -> code point). Close to the
/// Python behavior (utf-8 then cp1252); syllable counts are identical on the
/// tested files.
fn decode_text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

pub fn parse(data: &[u8]) -> Result<Midi, String> {
    let n = data.len();
    if n < 14 || &data[0..4] != b"MThd" {
        return Err("not a MIDI file".into());
    }
    let mut p = 4usize;
    let hlen = be_u32(data, p);
    p += 4;
    p += 2; // format (ignored)
    let ntrk = be_u16(data, p) as usize;
    p += 2;
    let tpb = be_u16(data, p);
    p += 2;
    if hlen > 6 {
        p += (hlen - 6) as usize;
    }
    let mut tracks: Vec<Vec<Event>> = Vec::new();
    while p + 8 <= n && tracks.len() < ntrk {
        let is_mtrk = &data[p..p + 4] == b"MTrk";
        p += 4;
        let clen = be_u32(data, p) as usize;
        p += 4;
        if !is_mtrk {
            p = (p + clen).min(n); // skip an unknown chunk
            continue;
        }
        let end = (p + clen).min(n); // hard anti-corruption bound
        let mut events: Vec<Event> = Vec::new();
        let mut tick: u32 = 0;
        let mut running: u8 = 0;
        while p < end {
            let (delta, np) = read_vlq(data, p, end);
            p = np;
            tick = tick.wrapping_add(delta);
            if p >= end {
                break;
            }
            let mut status = data[p];
            if status & 0x80 != 0 {
                p += 1;
                running = if status < 0xF0 { status } else { 0 };
            } else {
                status = running;
            }
            if status == 0xFF {
                if p >= end {
                    break;
                }
                let mtype = data[p];
                p += 1;
                let (len, np) = read_vlq(data, p, end);
                p = np;
                let pend = (p + len as usize).min(end);
                let payload = &data[p..pend];
                p = pend;
                match mtype {
                    0x51 if payload.len() == 3 => {
                        let us = ((payload[0] as u32) << 16)
                            | ((payload[1] as u32) << 8)
                            | payload[2] as u32;
                        events.push(Event { tick, kind: Kind::Tempo(us) });
                    }
                    0x58 if payload.len() >= 2 => {
                        let den = if payload[1] <= 10 { 1u16 << payload[1] } else { 4 };
                        events.push(Event {
                            tick,
                            kind: Kind::TimeSig { num: payload[0], den },
                        });
                    }
                    0x01 => events.push(Event { tick, kind: Kind::Text(decode_text(payload)) }),
                    0x05 => events.push(Event { tick, kind: Kind::Lyrics(decode_text(payload)) }),
                    0x03 => events.push(Event { tick, kind: Kind::TrackName(decode_text(payload)) }),
                    _ => {}
                }
            } else if status == 0xF0 || status == 0xF7 {
                let (len, np) = read_vlq(data, p, end);
                p = (np + len as usize).min(end);
            } else {
                let hi = status & 0xF0;
                if hi == 0xC0 || hi == 0xD0 {
                    p += 1;
                } else if p + 1 < end {
                    let d1 = data[p];
                    let d2 = data[p + 1];
                    p += 2;
                    if hi == 0x90 {
                        events.push(Event {
                            tick,
                            kind: if d2 > 0 { Kind::NoteOn(d1) } else { Kind::NoteOff(d1) },
                        });
                    } else if hi == 0x80 {
                        events.push(Event { tick, kind: Kind::NoteOff(d1) });
                    }
                } else {
                    p = end;
                }
            }
        }
        p = end;
        tracks.push(events);
    }
    Ok(Midi {
        ticks_per_beat: if tpb == 0 { 480 } else { tpb },
        tracks,
    })
}
