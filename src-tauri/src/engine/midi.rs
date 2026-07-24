//! MIDI parser (pure Rust, no external dependencies).
//! Port of the reference Python engine's parser, with the same robustness
//! guards (running-status cleared on meta/sysex, unknown chunks skipped, hard
//! anti-corruption bound).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFormat {
    StandardMidi,
    KaraokeMidi,
    MusicXml,
    MuseScore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeBase {
    PulsesPerQuarter(u16),
    Smpte {
        frames_per_second: i8,
        ticks_per_frame: u8,
    },
}

#[derive(Debug)]
pub struct Midi {
    /// Kept for the SVP time conversion used by PPQ sources. `time_base` is
    /// authoritative; SMPTE sources are retained but rejected explicitly by
    /// the current SVP projector instead of being silently interpreted as PPQ.
    pub ticks_per_beat: u16,
    pub time_base: TimeBase,
    pub format: u16,
    pub source_format: SourceFormat,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrackRoleHint {
    Vocal,
    Instrumental,
    Percussion,
    Mixed,
    #[default]
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MidiTextProfile {
    #[default]
    Generic,
    KaraokeControl,
    KaraokeLyrics,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackSource {
    pub source_track: usize,
    pub part_id: Option<String>,
    pub staff_id: Option<String>,
    pub voice: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstrumentInfo {
    pub id: Option<String>,
    pub name: Option<String>,
    /// Value exactly as written by the source. MusicXML uses one-based
    /// channel/program values while MIDI and MuseScore use zero-based values.
    pub source_channel: Option<i32>,
    pub source_program: Option<i32>,
    /// Zero-based MIDI channel when the source supplies one.
    pub channel: Option<u8>,
    /// Zero-based MIDI program when the source supplies one.
    pub program: Option<u8>,
    pub bank_msb: Option<u8>,
    pub bank_lsb: Option<u8>,
    pub volume: Option<f64>,
    pub pan: Option<f64>,
    pub controllers: Vec<(u8, u8)>,
    /// Raw MusicXML playback mapping (1..=128), when present.
    pub midi_unpitched: Option<u8>,
    pub percussion: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub source: TrackSource,
    pub role_hint: TrackRoleHint,
    /// Qualification is local to this track. A marker in one SMF track never
    /// turns generic Text events in another track into lyrics.
    pub text_profile: MidiTextProfile,
    /// All instruments declared for this source track/part.
    pub instruments: Vec<InstrumentInfo>,
    /// Primary instrument retained for callers that only need one summary.
    pub instrument: Option<InstrumentInfo>,
    pub events: Vec<Event>,
}

impl Track {
    pub fn new(id: impl Into<String>, source_track: usize) -> Self {
        Self {
            id: id.into(),
            name: String::new(),
            source: TrackSource {
                source_track,
                ..TrackSource::default()
            },
            role_hint: TrackRoleHint::Ambiguous,
            text_profile: MidiTextProfile::Generic,
            instruments: Vec::new(),
            instrument: None,
            events: Vec::new(),
        }
    }
}

/// Structure markers of a score measure.
#[derive(Default, Clone)]
pub struct MeasureMarks {
    pub start_repeat: bool,
    pub end_repeat: u32, // 0 = none; otherwise total number of passes (>= 2)
    pub volta: Option<Vec<u32>>, // pass numbers (1-based) on which the measure is played
    pub segno: bool,
    pub coda: bool,    // target of the "al Coda" jump (coda symbol)
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
    let mut jumps_left: Vec<u32> = marks
        .iter()
        .map(|m| m.end_repeat.saturating_sub(1))
        .collect();
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

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub tick: u32,
    /// Stable source order for deterministic same-tick handling.
    pub order: u32,
    pub kind: Kind,
}

impl Event {
    pub fn new(tick: u32, order: u32, kind: Kind) -> Self {
        Self { tick, order, kind }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Syllabic {
    Single,
    Begin,
    Middle,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineBreak {
    Line,
    Paragraph,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LyricState {
    Text(String),
    Continuation,
    SyllableSplit,
    ExplicitEmpty,
    Unsupported(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LyricExtension {
    Start,
    Continue,
    Stop,
    Unspecified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LyricFragment {
    Text(String),
    Elision(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lyric {
    pub id: String,
    /// Original decoded source payload, before any KAR control interpretation.
    pub raw: String,
    /// Original MIDI payload when the lyric came from an SMF meta event.
    pub raw_bytes: Vec<u8>,
    /// Ordered semantic fragments. MusicXML elisions remain explicit instead
    /// of disappearing during projection.
    pub fragments: Vec<LyricFragment>,
    pub lane: String,
    pub verse: u32,
    pub state: LyricState,
    pub syllabic: Option<Syllabic>,
    pub line_break: Option<LineBreak>,
    pub time_only: Vec<u32>,
    /// MusicXML extension phase. Kept independently because a lyric can carry
    /// both real text and an `<extend>` marker.
    pub extension: Option<LyricExtension>,
    /// MuseScore lyric-extension duration in source ticks, when present.
    pub extend_ticks: Option<i64>,
    /// MuseScore exact fractional extension, when present.
    pub extend_fraction: Option<(i64, i64)>,
}

impl Lyric {
    pub fn text(id: impl Into<String>, value: String) -> Self {
        let state = if value.is_empty() {
            LyricState::ExplicitEmpty
        } else {
            LyricState::Text(value.clone())
        };
        Self {
            id: id.into(),
            raw: value,
            raw_bytes: Vec::new(),
            fragments: vec![LyricFragment::Text(match &state {
                LyricState::Text(text) => text.clone(),
                _ => String::new(),
            })],
            lane: "1".into(),
            verse: 1,
            state,
            syllabic: None,
            line_break: None,
            time_only: Vec::new(),
            extension: None,
            extend_ticks: None,
            extend_fraction: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnpitchedInfo {
    pub instrument_id: Option<String>,
    pub display_step: Option<String>,
    pub display_octave: Option<i8>,
    /// Raw MusicXML `midi-unpitched` value (1..=128), when present.
    pub midi_unpitched: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteSource {
    pub id: String,
    pub part_id: Option<String>,
    pub staff_id: Option<String>,
    pub voice: Option<String>,
    pub chord_id: Option<String>,
    pub instrument_id: Option<String>,
    pub occurrence: u32,
    pub grace: bool,
    pub unpitched: Option<UnpitchedInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteOn {
    pub channel: Option<u8>,
    /// Playback pitch. `None` preserves an unpitched source for which no
    /// source-owned MIDI mapping exists, without inventing C4.
    pub key: Option<u8>,
    pub velocity: Option<u8>,
    pub source: NoteSource,
    /// XML/MuseScore lyrics remain owned by this exact note/chord occurrence.
    /// MIDI lyric meta events remain standalone `Kind::Lyrics` events.
    pub lyrics: Vec<Lyric>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteOff {
    pub channel: Option<u8>,
    pub key: Option<u8>,
    pub velocity: Option<u8>,
    /// XML adapters use the exact source-note identity to close overlapping
    /// same-channel/same-key voices. Native MIDI has no such identifier.
    pub source_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEvent {
    pub text: String,
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    NoteOn(NoteOn),
    NoteOff(NoteOff),
    PolyPressure {
        channel: u8,
        key: u8,
        pressure: u8,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    ChannelPressure {
        channel: u8,
        pressure: u8,
    },
    PitchBend {
        channel: u8,
        value: u16,
    },
    Port(u8),
    SysEx {
        escaped: bool,
        data: Vec<u8>,
    },
    Meta {
        meta_type: u8,
        data: Vec<u8>,
    },
    Tempo(u32),
    TimeSig {
        num: u8,
        den: u16,
        clocks_per_click: Option<u8>,
        notated_32nds: Option<u8>,
    },
    Text(TextEvent),
    Lyrics(Lyric),
    TrackName(String),
}

fn be_u32(d: &[u8], p: usize) -> u32 {
    ((d[p] as u32) << 24) | ((d[p + 1] as u32) << 16) | ((d[p + 2] as u32) << 8) | d[p + 3] as u32
}
fn be_u16(d: &[u8], p: usize) -> u16 {
    ((d[p] as u16) << 8) | d[p + 1] as u16
}

fn read_vlq(d: &[u8], mut p: usize, end: usize) -> Result<(u32, usize), String> {
    let mut v: u32 = 0;
    for _ in 0..4 {
        if p >= end {
            return Err("truncated MIDI variable-length quantity".into());
        }
        let c = d[p];
        p += 1;
        v = (v << 7) | (c & 0x7f) as u32;
        if c & 0x80 == 0 {
            return Ok((v, p));
        }
    }
    Err("invalid MIDI variable-length quantity".into())
}

fn windows_1252_char(byte: u8) -> char {
    match byte {
        0x80 => '\u{20ac}',
        0x81 => '\u{0081}',
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
        0x8d => '\u{008d}',
        0x8e => '\u{017d}',
        0x8f => '\u{008f}',
        0x90 => '\u{0090}',
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
        0x9d => '\u{009d}',
        0x9e => '\u{017e}',
        0x9f => '\u{0178}',
        _ => char::from(byte),
    }
}

/// UTF-8 when valid, otherwise true Windows-1252. The source bytes are kept
/// separately on Text/Lyric events so decoding never destroys provenance.
fn decode_text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().copied().map(windows_1252_char).collect(),
    }
}

pub fn parse(data: &[u8]) -> Result<Midi, String> {
    parse_smf(data)
}

/// Retained for callers that know the extension is `.kar`. The extension is
/// not evidence by itself: Text events are qualified only by markers carried
/// by their own source track.
pub fn parse_with_karaoke_profile(data: &[u8]) -> Result<Midi, String> {
    parse_smf(data)
}

fn parse_smf(data: &[u8]) -> Result<Midi, String> {
    const MAX_TRACKS: usize = 4096;
    const MAX_EVENTS: usize = 2_000_000;
    let n = data.len();
    if n < 14 || &data[0..4] != b"MThd" {
        return Err("not a MIDI file".into());
    }
    let mut p = 4usize;
    let hlen = be_u32(data, p) as usize;
    p += 4;
    if hlen < 6 || p.checked_add(hlen).is_none_or(|end| end > n) {
        return Err("invalid MIDI header length".into());
    }
    let format = be_u16(data, p);
    p += 2;
    let ntrk = be_u16(data, p) as usize;
    p += 2;
    if ntrk > MAX_TRACKS {
        return Err("too many MIDI tracks".into());
    }
    let division = be_u16(data, p);
    p += 2;
    if hlen > 6 {
        p += hlen - 6;
    }
    let (time_base, ticks_per_beat) = if division & 0x8000 == 0 {
        if division == 0 {
            return Err("MIDI PPQ division must be non-zero".into());
        }
        let ppq = division;
        (TimeBase::PulsesPerQuarter(ppq), ppq)
    } else {
        let encoded_fps = (division >> 8) as u8 as i8;
        let fps = encoded_fps.saturating_neg();
        let ticks = division as u8;
        if !matches!(fps, 24 | 25 | 29 | 30) || ticks == 0 {
            return Err("invalid MIDI SMPTE time division".into());
        }
        (
            TimeBase::Smpte {
                frames_per_second: fps,
                ticks_per_frame: ticks,
            },
            480,
        )
    };

    let mut tracks: Vec<Track> = Vec::new();
    let mut total_events = 0usize;
    while p + 8 <= n && tracks.len() < ntrk {
        let is_mtrk = &data[p..p + 4] == b"MTrk";
        p += 4;
        let clen = be_u32(data, p) as usize;
        p += 4;
        let end = p
            .checked_add(clen)
            .filter(|&end| end <= n)
            .ok_or_else(|| "truncated MIDI track chunk".to_string())?;
        if !is_mtrk {
            p = end;
            continue;
        }
        let track_index = tracks.len();
        let mut events: Vec<Event> = Vec::new();
        let mut tick: u32 = 0;
        let mut running: u8 = 0;
        while p < end {
            if total_events >= MAX_EVENTS {
                return Err("too many MIDI events".into());
            }
            let (delta, np) = read_vlq(data, p, end)?;
            p = np;
            tick = tick
                .checked_add(delta)
                .ok_or_else(|| "MIDI tick overflow".to_string())?;
            if p >= end {
                return Err("truncated MIDI event".into());
            }
            let mut status = data[p];
            if status & 0x80 != 0 {
                p += 1;
                running = if status < 0xF0 { status } else { 0 };
            } else {
                if running == 0 {
                    return Err("MIDI running status without channel status".into());
                }
                status = running;
            }
            let order = events.len() as u32;
            if status == 0xFF {
                if p >= end {
                    return Err("truncated MIDI meta event".into());
                }
                let mtype = data[p];
                p += 1;
                let (len, np) = read_vlq(data, p, end)?;
                p = np;
                let pend = p
                    .checked_add(len as usize)
                    .filter(|&pend| pend <= end)
                    .ok_or_else(|| "truncated MIDI meta payload".to_string())?;
                let payload = &data[p..pend];
                p = pend;
                let kind = match mtype {
                    0x51 if payload.len() == 3 => {
                        let us = ((payload[0] as u32) << 16)
                            | ((payload[1] as u32) << 8)
                            | payload[2] as u32;
                        Kind::Tempo(us)
                    }
                    0x58 if payload.len() >= 4 && payload[1] <= 15 => {
                        let den = 1u16 << payload[1];
                        Kind::TimeSig {
                            num: payload[0],
                            den,
                            clocks_per_click: Some(payload[2]),
                            notated_32nds: Some(payload[3]),
                        }
                    }
                    0x01 => Kind::Text(TextEvent {
                        text: decode_text(payload),
                        raw: payload.to_vec(),
                    }),
                    0x05 => {
                        let decoded = decode_text(payload);
                        let mut lyric =
                            Lyric::text(format!("midi-t{track_index}-e{order}-lyric"), decoded);
                        lyric.raw_bytes = payload.to_vec();
                        Kind::Lyrics(lyric)
                    }
                    0x03 => Kind::TrackName(decode_text(payload)),
                    0x21 if payload.len() == 1 => Kind::Port(payload[0]),
                    _ => Kind::Meta {
                        meta_type: mtype,
                        data: payload.to_vec(),
                    },
                };
                events.push(Event::new(tick, order, kind));
                total_events += 1;
                if mtype == 0x2f {
                    break;
                }
            } else if status == 0xF0 || status == 0xF7 {
                let (len, np) = read_vlq(data, p, end)?;
                let pend = np
                    .checked_add(len as usize)
                    .filter(|&pend| pend <= end)
                    .ok_or_else(|| "truncated MIDI SysEx payload".to_string())?;
                events.push(Event::new(
                    tick,
                    order,
                    Kind::SysEx {
                        escaped: status == 0xF7,
                        data: data[np..pend].to_vec(),
                    },
                ));
                total_events += 1;
                p = pend;
            } else if status >= 0xF0 {
                return Err(format!("unsupported MIDI system status 0x{status:02x}"));
            } else {
                let hi = status & 0xF0;
                let channel = status & 0x0F;
                if hi == 0xC0 || hi == 0xD0 {
                    if p >= end {
                        return Err("truncated MIDI channel event".into());
                    }
                    let d1 = data[p];
                    p += 1;
                    let kind = if hi == 0xC0 {
                        Kind::ProgramChange {
                            channel,
                            program: d1,
                        }
                    } else {
                        Kind::ChannelPressure {
                            channel,
                            pressure: d1,
                        }
                    };
                    events.push(Event::new(tick, order, kind));
                    total_events += 1;
                } else {
                    if p + 1 >= end {
                        return Err("truncated MIDI channel event".into());
                    }
                    let d1 = data[p];
                    let d2 = data[p + 1];
                    p += 2;
                    let kind = match hi {
                        0x80 => Kind::NoteOff(NoteOff {
                            channel: Some(channel),
                            key: Some(d1),
                            velocity: Some(d2),
                            source_id: None,
                        }),
                        0x90 => Kind::NoteOn(NoteOn {
                            channel: Some(channel),
                            key: Some(d1),
                            velocity: Some(d2),
                            source: NoteSource {
                                id: format!("midi-t{track_index}-e{order}-note"),
                                voice: Some((channel + 1).to_string()),
                                ..NoteSource::default()
                            },
                            lyrics: Vec::new(),
                        }),
                        0xA0 => Kind::PolyPressure {
                            channel,
                            key: d1,
                            pressure: d2,
                        },
                        0xB0 => Kind::ControlChange {
                            channel,
                            controller: d1,
                            value: d2,
                        },
                        0xE0 => Kind::PitchBend {
                            channel,
                            value: (d1 as u16) | ((d2 as u16) << 7),
                        },
                        _ => return Err(format!("unsupported MIDI channel status 0x{status:02x}")),
                    };
                    events.push(Event::new(tick, order, kind));
                    total_events += 1;
                }
            }
        }
        p = end;
        let mut track = Track::new(format!("midi-track-{track_index}"), track_index);
        track.name = events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::TrackName(name) => Some(name.trim().to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let mut channels = std::collections::BTreeSet::new();
        let mut first_program = None;
        let mut bank_msb = None;
        let mut bank_lsb = None;
        for event in &events {
            match event.kind {
                Kind::NoteOn(ref note) => {
                    if let Some(channel) = note.channel {
                        channels.insert(channel);
                    }
                }
                Kind::ProgramChange { channel, program } => {
                    channels.insert(channel);
                    first_program.get_or_insert(program);
                }
                Kind::ControlChange {
                    channel,
                    controller,
                    value,
                } => {
                    channels.insert(channel);
                    if controller == 0 {
                        bank_msb.get_or_insert(value);
                    } else if controller == 32 {
                        bank_lsb.get_or_insert(value);
                    }
                }
                _ => {}
            }
        }
        let channel = (channels.len() == 1)
            .then(|| channels.iter().next().copied())
            .flatten();
        let percussion = channels.contains(&9);
        if percussion {
            track.role_hint = if channels.len() == 1 {
                TrackRoleHint::Percussion
            } else {
                TrackRoleHint::Mixed
            };
        }
        if channel.is_some() || first_program.is_some() || bank_msb.is_some() || bank_lsb.is_some()
        {
            let instrument = InstrumentInfo {
                source_channel: channel.map(i32::from),
                source_program: first_program.map(i32::from),
                channel,
                program: first_program,
                bank_msb,
                bank_lsb,
                percussion,
                ..InstrumentInfo::default()
            };
            track.instrument = Some(instrument.clone());
            track.instruments.push(instrument);
        }
        track.text_profile = classify_text_profile(&events);
        track.events = events;
        tracks.push(track);
    }
    if tracks.len() != ntrk {
        return Err(format!(
            "MIDI declares {ntrk} tracks but only {} were found",
            tracks.len()
        ));
    }
    let source_format = if tracks
        .iter()
        .any(|track| track.text_profile != MidiTextProfile::Generic)
    {
        SourceFormat::KaraokeMidi
    } else {
        SourceFormat::StandardMidi
    };
    Ok(Midi {
        ticks_per_beat,
        time_base,
        format,
        source_format,
        tracks,
    })
}

fn classify_text_profile(events: &[Event]) -> MidiTextProfile {
    let text_events: Vec<&TextEvent> = events
        .iter()
        .filter_map(|event| match &event.kind {
            Kind::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    let has_kmidi = text_events.iter().any(|text| {
        text.text
            .trim_start()
            .to_ascii_uppercase()
            .starts_with("@KMIDI")
    });
    let lyric_payloads: Vec<_> = text_events
        .iter()
        .filter(|text| !text.text.trim_start().starts_with('@'))
        .collect();
    let has_line_control = lyric_payloads
        .iter()
        .any(|text| matches!(text.raw.first().copied(), Some(b'\\') | Some(b'/')));
    if has_line_control && lyric_payloads.len() >= 2 {
        MidiTextProfile::KaraokeLyrics
    } else if has_kmidi {
        MidiTextProfile::KaraokeControl
    } else {
        MidiTextProfile::Generic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smf(division: u16, tracks: Vec<Vec<u8>>) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(b"MThd");
        data.extend_from_slice(&6u32.to_be_bytes());
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(
            &u16::try_from(tracks.len())
                .expect("test track count fits")
                .to_be_bytes(),
        );
        data.extend_from_slice(&division.to_be_bytes());
        for mut track in tracks {
            if !track.ends_with(&[0, 0xff, 0x2f, 0]) {
                track.extend_from_slice(&[0, 0xff, 0x2f, 0]);
            }
            data.extend_from_slice(b"MTrk");
            data.extend_from_slice(
                &u32::try_from(track.len())
                    .expect("test track length fits")
                    .to_be_bytes(),
            );
            data.extend_from_slice(&track);
        }
        data
    }

    fn text_meta(meta_type: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 128);
        let mut event = vec![0, 0xff, meta_type, payload.len() as u8];
        event.extend_from_slice(payload);
        event
    }

    #[test]
    fn zero_ppq_division_is_rejected() {
        let error = parse(&smf(0, vec![Vec::new()])).expect_err("zero PPQ is invalid");
        assert!(error.contains("non-zero"), "unexpected error: {error}");
    }

    #[test]
    fn windows_1252_lyrics_keep_decoded_text_and_source_bytes() {
        let midi = parse(&smf(480, vec![text_meta(0x05, &[0x92])])).unwrap();
        let lyric = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::Lyrics(lyric) => Some(lyric),
                _ => None,
            })
            .expect("lyric event");
        assert_eq!(lyric.raw, "\u{2019}");
        assert_eq!(lyric.raw_bytes, vec![0x92]);
        assert_eq!(lyric.state, LyricState::Text("\u{2019}".into()));
    }

    #[test]
    fn time_signature_keeps_both_midi_metronome_fields() {
        let midi = parse(&smf(480, vec![text_meta(0x58, &[7, 3, 36, 8])])).unwrap();
        assert!(matches!(
            midi.tracks[0].events[0].kind,
            Kind::TimeSig {
                num: 7,
                den: 8,
                clocks_per_click: Some(36),
                notated_32nds: Some(8)
            }
        ));
    }

    #[test]
    fn unsupported_time_signature_exponent_stays_raw_meta() {
        let midi = parse(&smf(480, vec![text_meta(0x58, &[4, 16, 24, 8])])).unwrap();
        assert!(matches!(
            &midi.tracks[0].events[0].kind,
            Kind::Meta {
                meta_type: 0x58,
                data
            } if data == &[4, 16, 24, 8]
        ));
    }

    #[test]
    fn kmidi_marker_never_qualifies_text_on_another_track() {
        let control = text_meta(0x01, b"@KMIDI KARAOKE FILE");
        let mut other = text_meta(0x01, b"ordinary metadata");
        other.extend(text_meta(0x01, b"still metadata"));
        let midi = parse_with_karaoke_profile(&smf(480, vec![control, other])).unwrap();
        assert_eq!(midi.tracks[0].text_profile, MidiTextProfile::KaraokeControl);
        assert_eq!(midi.tracks[1].text_profile, MidiTextProfile::Generic);
    }

    #[test]
    fn karaoke_extension_hint_cannot_qualify_unproven_text() {
        let mut text = text_meta(0x01, b"first");
        text.extend(text_meta(0x01, b"second"));
        let midi = parse_with_karaoke_profile(&smf(480, vec![text])).unwrap();
        assert_eq!(midi.tracks[0].text_profile, MidiTextProfile::Generic);
        assert_eq!(midi.source_format, SourceFormat::StandardMidi);
    }

    #[test]
    fn line_controls_qualify_only_their_own_text_track() {
        let mut lyrics = text_meta(0x01, b"\\let");
        lyrics.extend(text_meta(0x01, b"/it"));
        let midi = parse(&smf(480, vec![lyrics, Vec::new()])).unwrap();
        assert_eq!(midi.tracks[0].text_profile, MidiTextProfile::KaraokeLyrics);
        assert_eq!(midi.tracks[1].text_profile, MidiTextProfile::Generic);
    }
}
