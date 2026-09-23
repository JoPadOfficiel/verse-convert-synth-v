//! Reads a Standard MIDI File's tracks for its stems: which tracks sound notes
//! and under which names (the mapping proof for MuseScore's whole-file import),
//! which channel/key routes several tracks share, and one single-track file per
//! source track. That file, the byte-identical `MTrk` plus the file's global
//! marks, is the fallback stem when the import cannot be mapped; MuseScore may
//! quantize a track imported alone differently than inside the whole file.

use std::collections::{BTreeMap, BTreeSet};

/// A single-track Standard MIDI File carrying one source track and the meta
/// marks that place it on the score's timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiTrackSlice {
    /// Index of the source `MTrk` chunk this slice carries.
    pub source_track: usize,
    pub bytes: Vec<u8>,
}

/// Meta events that govern playback of the whole file rather than one track.
/// A stem rendered without them would play at the default 120 BPM in 4/4 and
/// would not line up with the reference mix.
const GLOBAL_META: [u8; 4] = [
    0x51, // set tempo
    0x58, // time signature
    0x59, // key signature
    0x54, // SMPTE offset
];

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(count)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| "MIDI chunk ends inside an event".to_string())?;
        let slice = &self.data[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn peek(&self) -> Result<u8, String> {
        self.data
            .get(self.position)
            .copied()
            .ok_or_else(|| "MIDI chunk ends inside an event".to_string())
    }

    /// Variable-length quantity, bounded to the four bytes the format allows.
    fn varint(&mut self) -> Result<u32, String> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let byte = self.byte()?;
            value = (value << 7) | u32::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("MIDI variable-length quantity is too long".into())
    }

    fn done(&self) -> bool {
        self.position >= self.data.len()
    }
}

fn write_varint(out: &mut Vec<u8>, mut value: u32) {
    let mut stack = [0u8; 5];
    let mut len = 0;
    loop {
        stack[len] = (value & 0x7f) as u8;
        len += 1;
        value >>= 7;
        if value == 0 {
            break;
        }
    }
    for index in (0..len).rev() {
        let mut byte = stack[index];
        if index != 0 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

/// Header of a Standard MIDI File: format, track count, division.
struct Header {
    division: u16,
    track_count: usize,
    body_offset: usize,
}

fn read_header(data: &[u8]) -> Result<Header, String> {
    if data.len() < 14 || &data[0..4] != b"MThd" {
        return Err("not a Standard MIDI File".into());
    }
    let length = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if length < 6 {
        return Err("MIDI header chunk is too short".into());
    }
    if u16::from_be_bytes([data[8], data[9]]) > 1 {
        return Err("MIDI_STEM_INDEPENDENT_SEQUENCES: independent MIDI sequences cannot share a stem timeline".into());
    }
    let division = u16::from_be_bytes([data[12], data[13]]);
    let track_count = usize::from(u16::from_be_bytes([data[10], data[11]]));
    let body_offset = 8usize
        .checked_add(length as usize)
        .ok_or_else(|| "MIDI header length overflows".to_string())?;
    Ok(Header {
        division,
        track_count,
        body_offset,
    })
}

/// Byte ranges of every `MTrk` chunk body, in file order.
fn track_bodies(data: &[u8], header: &Header) -> Result<Vec<(usize, usize)>, String> {
    let mut bodies = Vec::new();
    let mut offset = header.body_offset;
    while bodies.len() < header.track_count {
        let end = offset
            .checked_add(8)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| "MIDI file ends before its declared tracks".to_string())?;
        let length = u32::from_be_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]) as usize;
        let body_end = end
            .checked_add(length)
            .filter(|body_end| *body_end <= data.len())
            .ok_or_else(|| "MIDI track chunk runs past the end of the file".to_string())?;
        if &data[offset..offset + 4] == b"MTrk" {
            bodies.push((end, body_end));
        }
        offset = body_end;
    }
    Ok(bodies)
}

/// A global meta event and the absolute tick it sits on.
type TimedMeta = (u32, Vec<u8>);

/// Stable diagnostic code for note routes several source tracks use at once.
pub const MIDI_STEM_SHARED_NOTE_ROUTE: &str = "MIDI_STEM_SHARED_NOTE_ROUTE";

struct NoteEvent {
    tick: u32,
    port: u8,
    channel: u8,
    key: u8,
    sounding: bool,
}

struct TrackScan {
    metas: Vec<TimedMeta>,
    end_tick: u32,
    notes: Vec<NoteEvent>,
    name: Option<Vec<u8>>,
}

/// A source track that sounds at least one note, with its first raw track
/// name, as MuseScore's MIDI import turns it into a Part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteTrack {
    pub source_track: usize,
    pub name: Option<Vec<u8>>,
}

/// Keys of one port/channel whose notes overlap, or start or stop together, in
/// the same set of source tracks.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SharedNoteRoute {
    pub port: u8,
    /// Zero-based MIDI channel.
    pub channel: u8,
    pub source_tracks: Vec<usize>,
    pub keys: Vec<u8>,
}

impl SharedNoteRoute {
    /// The warning for the stem path actually used: stems cut from the
    /// whole-file import, or the per-track split.
    pub fn diagnostic(&self, whole_file_import: bool) -> String {
        let list = |values: Vec<String>| values.join(", ");
        format!(
            "[{MIDI_STEM_SHARED_NOTE_ROUTE}] Source tracks {} share MIDI port {}, channel {}, {} {} with overlapping or simultaneous notes. {}",
            list(self.source_tracks.iter().map(usize::to_string).collect()),
            self.port,
            u16::from(self.channel) + 1,
            if self.keys.len() == 1 { "key" } else { "keys" },
            list(self.keys.iter().map(u8::to_string).collect()),
            if whole_file_import {
                "The stems carry MuseScore's import of the whole file, so these notes sound as in the reference mix."
            } else {
                "Each stem keeps its own track's bytes, so these notes can sound differently in the stems than in the reference mix."
            }
        )
    }
}

/// The per-track stems of one source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiSplit {
    pub slices: Vec<MidiTrackSlice>,
    pub shared_note_routes: Vec<SharedNoteRoute>,
    /// Tracks that sound a note, in file order.
    pub note_tracks: Vec<NoteTrack>,
}

/// Absolute-tick global meta events and note events of one track, plus the
/// tick of its end. Running status is honoured so events are read exactly.
fn scan_track(body: &[u8]) -> Result<TrackScan, String> {
    let mut reader = Reader::new(body);
    let mut tick: u32 = 0;
    let mut metas = Vec::new();
    let mut running: Option<u8> = None;
    let mut port = 0;
    let mut notes = Vec::new();
    let mut name = None;
    while !reader.done() {
        let delta = reader.varint()?;
        tick = tick
            .checked_add(delta)
            .ok_or_else(|| "MIDI track timing overflows".to_string())?;
        let status = reader.peek()?;
        if status == 0xff {
            let start = reader.position;
            reader.byte()?;
            let kind = reader.byte()?;
            let length = reader.varint()? as usize;
            let payload = reader.take(length)?;
            if kind == 0x21 {
                if payload.len() != 1 || payload[0] > 127 {
                    return Err("MIDI_STEM_PORT_UNRESOLVED: invalid MIDI port declaration".into());
                }
                port = payload[0];
            }
            if kind == 0x03 && name.is_none() {
                name = Some(payload.to_vec());
            }
            if GLOBAL_META.contains(&kind) {
                metas.push((tick, body[start..reader.position].to_vec()));
            }
            running = None;
            if kind == 0x2f {
                break;
            }
        } else if status == 0xf0 || status == 0xf7 {
            reader.byte()?;
            let length = reader.varint()? as usize;
            reader.take(length)?;
            running = None;
        } else {
            let status = if status & 0x80 != 0 {
                reader.byte()?;
                running = Some(status);
                status
            } else {
                running
                    .ok_or_else(|| "MIDI running status without a preceding event".to_string())?
            };
            let data_bytes = match status & 0xf0 {
                0xc0 | 0xd0 => 1,
                0x80 | 0x90 | 0xa0 | 0xb0 | 0xe0 => 2,
                _ => return Err(format!("unsupported MIDI status byte {status:#04x}")),
            };
            let payload = reader.take(data_bytes)?;
            if payload.iter().any(|byte| *byte > 127) {
                return Err("MIDI channel event has an invalid data byte".into());
            }
            if matches!(status & 0xf0, 0x80 | 0x90) {
                notes.push(NoteEvent {
                    tick,
                    port,
                    channel: status & 0x0f,
                    key: payload[0],
                    sounding: status & 0xf0 == 0x90 && payload[1] != 0,
                });
            }
        }
    }
    Ok(TrackScan {
        metas,
        end_tick: tick,
        notes,
        name,
    })
}

fn meta_track(metas: &[TimedMeta], end_tick: u32) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    let mut previous = 0u32;
    for (tick, event) in metas {
        let delta = tick
            .checked_sub(previous)
            .filter(|delta| *delta <= 0x0fff_ffff)
            .ok_or(
                "MIDI_STEM_TIMING_UNREPRESENTABLE: context delta exceeds the MIDI timing range",
            )?;
        write_varint(&mut body, delta);
        body.extend_from_slice(event);
        previous = *tick;
    }
    let delta = end_tick
        .checked_sub(previous)
        .filter(|delta| *delta <= 0x0fff_ffff)
        .ok_or("MIDI_STEM_TIMING_UNREPRESENTABLE: context tail exceeds the MIDI timing range")?;
    write_varint(&mut body, delta);
    body.extend_from_slice(&[0xff, 0x2f, 0x00]);
    Ok(chunk(b"MTrk", &body))
}

/// In the whole file another track's note-off on the same channel/key can end
/// a note early; a per-track stem cannot reproduce that. Sequential reuse of a
/// key by different tracks is not reported.
fn shared_note_routes(scans: &[TrackScan]) -> Vec<SharedNoteRoute> {
    let mut grouped: BTreeMap<(u8, u8, Vec<usize>), Vec<u8>> = BTreeMap::new();
    let mut routes: BTreeMap<_, Vec<(usize, &NoteEvent)>> = BTreeMap::new();
    for (owner, scan) in scans.iter().enumerate() {
        for event in &scan.notes {
            routes
                .entry((event.port, event.channel, event.key))
                .or_default()
                .push((owner, event));
        }
    }
    for ((port, channel, key), mut events) in routes {
        if events.iter().all(|(owner, _)| *owner == events[0].0) {
            continue;
        }
        events.sort_by_key(|(_, event)| event.tick);
        let mut tracks = BTreeSet::new();
        let mut previous: Option<(u32, usize)> = None;
        let mut active: Option<(usize, usize)> = None;
        for &(owner, event) in &events {
            if let Some((tick, previous_owner)) = previous {
                if tick == event.tick && owner != previous_owner {
                    tracks.extend([owner, previous_owner]);
                }
            }
            if let Some((active_owner, _)) = active {
                if active_owner != owner {
                    tracks.extend([owner, active_owner]);
                }
            }
            previous = Some((event.tick, owner));
            if event.sounding {
                match &mut active {
                    Some((active_owner, count)) if *active_owner == owner => *count += 1,
                    Some(_) => {}
                    None => active = Some((owner, 1)),
                }
            } else if let Some((active_owner, count)) = &mut active {
                if *active_owner == owner {
                    *count -= 1;
                    if *count == 0 {
                        active = None;
                    }
                }
            }
        }
        if !tracks.is_empty() {
            grouped
                .entry((port, channel, tracks.into_iter().collect()))
                .or_default()
                .push(key);
        }
    }
    grouped
        .into_iter()
        .map(|((port, channel, source_tracks), keys)| SharedNoteRoute {
            port,
            channel,
            source_tracks,
            keys,
        })
        .collect()
}

fn chunk(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(tag);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// One playable file per source track, in source order: the file's tempo,
/// meter, key and SMPTE marks, then the source chunk byte for byte. A track
/// without events is still returned so slices index by source track number.
pub fn split(data: &[u8]) -> Result<MidiSplit, String> {
    let header = read_header(data)?;
    let bodies = track_bodies(data, &header)?;
    let mut metas: Vec<TimedMeta> = Vec::new();
    let mut end_tick = 0u32;
    let mut scans = Vec::with_capacity(bodies.len());
    for (start, end) in &bodies {
        let scan = scan_track(&data[*start..*end])?;
        metas.extend(scan.metas.iter().cloned());
        end_tick = end_tick.max(scan.end_tick);
        scans.push(scan);
    }
    let mut global_owners = BTreeMap::new();
    for (owner, scan) in scans.iter().enumerate() {
        for (tick, bytes) in &scan.metas {
            let key = (*tick, bytes[1]);
            let (first_owner, first_bytes, multiple_owners, different_values) = global_owners
                .entry(key)
                .or_insert((owner, bytes, false, false));
            *multiple_owners |= *first_owner != owner;
            *different_values |= *first_bytes != bytes;
            if *multiple_owners && *different_values {
                return Err("MIDI_STEM_GLOBAL_STATE_ORDER_UNRESOLVED: conflicting simultaneous global playback marks in different tracks".into());
            }
        }
    }
    // A stable sort keeps two marks written on the same tick in file order.
    metas.sort_by_key(|(tick, _)| *tick);
    let meta = meta_track(&metas, end_tick)?;

    let mut header_body = Vec::with_capacity(6);
    header_body.extend_from_slice(&1u16.to_be_bytes()); // format 1: parallel tracks
    header_body.extend_from_slice(&2u16.to_be_bytes());
    header_body.extend_from_slice(&header.division.to_be_bytes());
    let prefix = chunk(b"MThd", &header_body);

    let slices = bodies
        .iter()
        .enumerate()
        .map(|(source_track, (start, end))| {
            let mut bytes = prefix.clone();
            bytes.extend_from_slice(&meta);
            bytes.extend_from_slice(&chunk(b"MTrk", &data[*start..*end]));
            MidiTrackSlice {
                source_track,
                bytes,
            }
        })
        .collect();
    let note_tracks = scans
        .iter()
        .enumerate()
        .filter(|(_, scan)| scan.notes.iter().any(|note| note.sounding))
        .map(|(source_track, scan)| NoteTrack {
            source_track,
            name: scan.name.clone(),
        })
        .collect();
    Ok(MidiSplit {
        slices,
        shared_note_routes: shared_note_routes(&scans),
        note_tracks,
    })
}

/// The slices of [`split`] alone.
pub fn split_tracks(data: &[u8]) -> Result<Vec<MidiTrackSlice>, String> {
    split(data).map(|split| split.slices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smf(division: u16, tracks: &[&[u8]]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
        body.extend_from_slice(&division.to_be_bytes());
        let mut out = chunk(b"MThd", &body);
        for track in tracks {
            out.extend_from_slice(&chunk(b"MTrk", track));
        }
        out
    }

    /// Tempo 500000 µs/quarter at tick 0, then 4/4, then one note.
    const TEMPO_AND_METER: &[u8] = &[
        0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // tempo
        0x00, 0xff, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08, // 4/4
        0x00, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0, // a note
        0x00, 0xff, 0x2f, 0x00,
    ];
    const SECOND_VOICE: &[u8] = &[
        0x00, 0x91, 67, 90, 0x87, 0x40, 0x81, 67, 0, 0x00, 0xff, 0x2f, 0x00,
    ];

    #[test]
    fn each_source_track_becomes_one_playable_file() {
        let data = smf(480, &[TEMPO_AND_METER, SECOND_VOICE]);
        let slices = split_tracks(&data).expect("split");
        assert_eq!(slices.len(), 2);
        assert_eq!(
            slices.iter().map(|s| s.source_track).collect::<Vec<_>>(),
            vec![0, 1]
        );
        for slice in &slices {
            let header = read_header(&slice.bytes).expect("header");
            assert_eq!(header.division, 480);
            assert_eq!(
                header.track_count, 2,
                "a meta track precedes the source one"
            );
        }
    }

    #[test]
    fn the_source_chunk_is_copied_byte_for_byte() {
        // A stem must be a subset of the source, never a re-encoding of it.
        let data = smf(480, &[TEMPO_AND_METER, SECOND_VOICE]);
        let slices = split_tracks(&data).expect("split");
        let carried = &slices[1].bytes;
        let needle = chunk(b"MTrk", SECOND_VOICE);
        assert!(
            carried
                .windows(needle.len())
                .any(|window| window == needle.as_slice()),
            "the source track chunk must appear verbatim"
        );
    }

    #[test]
    fn a_track_without_the_tempo_still_carries_it() {
        // Rendered without the file's tempo, a stem plays at the default 120
        // BPM and drifts away from the reference mix within a bar.
        let data = smf(480, &[TEMPO_AND_METER, SECOND_VOICE]);
        let slices = split_tracks(&data).expect("split");
        for slice in &slices {
            assert!(
                slice
                    .bytes
                    .windows(4)
                    .any(|w| w == [0xff, 0x51, 0x03, 0x07]),
                "every slice carries the set-tempo mark"
            );
            assert!(
                slice.bytes.windows(3).any(|w| w == [0xff, 0x58, 0x04]),
                "every slice carries the time signature"
            );
        }
    }

    #[test]
    fn running_status_is_followed_instead_of_guessed() {
        // Channel events may omit the repeated status byte. Mis-reading their
        // length would shift every following delta and silently corrupt the
        // tempo map this code collects.
        let running = &[
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // tempo
            0x00, 0x90, 60, 100, // note on, explicit status
            0x60, 62, 100, // note on, running status
            0x60, 62, 0, // note off through velocity 0
            0x00, 0x80, 60, 0, 0x00, 0xff, 0x2f, 0x00,
        ];
        let data = smf(480, &[running]);
        let slices = split_tracks(&data).expect("running status is understood");
        assert_eq!(slices.len(), 1);
        assert!(slices[0]
            .bytes
            .windows(4)
            .any(|w| w == [0xff, 0x51, 0x03, 0x07]));
    }

    #[test]
    fn a_truncated_track_is_refused_instead_of_half_read() {
        let mut data = smf(480, &[TEMPO_AND_METER]);
        data.truncate(data.len() - 4);
        assert!(split_tracks(&data).is_err());
    }

    #[test]
    fn a_file_that_is_not_midi_is_refused() {
        assert!(split_tracks(b"RIFF....WAVEfmt ").is_err());
    }

    #[test]
    fn a_stem_holds_only_global_marks_and_its_own_track() {
        // Program and controller state of another track stays out of the stem.
        let state: &[u8] = &[
            0x00, 0xff, 0x21, 0x01, 0x00, 0x00, 0xc9, 24, 0x00, 0xb9, 7, 90, 0x00, 0xf0, 0x02,
            0x7e, 0xf7, 0x00, 0xff, 0x2f, 0x00,
        ];
        let drums: &[u8] = &[
            0x00, 0x99, 36, 100, 0x83, 0x60, 0x89, 36, 0, 0x00, 0xff, 0x2f, 0x00,
        ];
        let data = smf(480, &[TEMPO_AND_METER, state, drums]);
        let divided = split(&data).expect("cross-track state no longer blocks a stem");
        let stem = &divided.slices[2].bytes;
        let meta_start = 14;
        let meta_len =
            u32::from_be_bytes(stem[meta_start + 4..meta_start + 8].try_into().unwrap()) as usize;
        let meta = &stem[meta_start + 8..meta_start + 8 + meta_len];
        assert_eq!(
            meta,
            [
                0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, 0x00, 0xff, 0x58, 0x04, 0x04, 0x02, 0x18,
                0x08, 0x83, 0x60, 0xff, 0x2f, 0x00
            ]
        );
        assert_eq!(&stem[meta_start + 8 + meta_len..], chunk(b"MTrk", drums));
        assert!(divided.shared_note_routes.is_empty());
        assert_eq!(
            divided.note_tracks,
            [
                NoteTrack {
                    source_track: 0,
                    name: None
                },
                NoteTrack {
                    source_track: 2,
                    name: None
                }
            ]
        );
    }

    #[test]
    fn overlapping_notes_of_one_key_in_two_tracks_are_reported_not_refused() {
        let first: &[u8] = &[
            0x00, 0x99, 36, 100, 0x83, 0x60, 0x89, 36, 0, 0x00, 0xff, 0x2f, 0x00,
        ];
        let second: &[u8] = &[
            0x60, 0x99, 36, 90, 0x60, 0x89, 36, 0, 0x00, 0xff, 0x2f, 0x00,
        ];
        let divided = split(&smf(480, &[first, second])).expect("shared routes still split");
        assert_eq!(divided.slices.len(), 2);
        assert_eq!(
            divided.shared_note_routes,
            [SharedNoteRoute {
                port: 0,
                channel: 9,
                source_tracks: vec![0, 1],
                keys: vec![36],
            }]
        );
        let diagnostic = divided.shared_note_routes[0].diagnostic(false);
        assert!(diagnostic.starts_with("[MIDI_STEM_SHARED_NOTE_ROUTE]"));
        assert!(diagnostic.contains("channel 10, key 36"));
        assert!(diagnostic.contains("Each stem keeps its own track's bytes"));
        assert!(divided.shared_note_routes[0]
            .diagnostic(true)
            .contains("so these notes sound as in the reference mix"));

        let chord: &[u8] = &[
            0x00, 0x99, 36, 100, 0x00, 0x99, 38, 100, 0x83, 0x60, 0x89, 36, 0, 0x00, 0x89, 38, 0,
            0x00, 0xff, 0x2f, 0x00,
        ];
        let both = split(&smf(480, &[chord, chord]))
            .unwrap()
            .shared_note_routes;
        assert_eq!(both.len(), 1, "one warning names every key of one route");
        assert_eq!(both[0].keys, [36, 38]);
        assert!(both[0]
            .diagnostic(true)
            .contains("channel 10, keys 36, 38 with"));

        let later: &[u8] = &[
            0x83, 0x61, 0x99, 36, 90, 0x60, 0x89, 36, 0, 0x00, 0xff, 0x2f, 0x00,
        ];
        assert!(split(&smf(480, &[first, later]))
            .unwrap()
            .shared_note_routes
            .is_empty());
    }

    #[test]
    fn unrepresentable_midi_is_still_refused() {
        let mut format_two = smf(480, &[TEMPO_AND_METER]);
        format_two[9] = 2;
        assert!(split(&format_two)
            .unwrap_err()
            .contains("MIDI_STEM_INDEPENDENT_SEQUENCES"));
        let bad_port: &[u8] = &[0x00, 0xff, 0x21, 0x01, 0x80, 0x00, 0xff, 0x2f, 0x00];
        assert!(split(&smf(480, &[bad_port]))
            .unwrap_err()
            .contains("MIDI_STEM_PORT_UNRESOLVED"));
        let slow: &[u8] = &[
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, 0x00, 0xff, 0x2f, 0x00,
        ];
        let fast: &[u8] = &[
            0x00, 0xff, 0x51, 0x03, 0x06, 0x1a, 0x80, 0x00, 0xff, 0x2f, 0x00,
        ];
        assert!(split(&smf(480, &[slow, fast]))
            .unwrap_err()
            .contains("MIDI_STEM_GLOBAL_STATE_ORDER_UNRESOLVED"));
        let huge: &[u8] = &[
            0xff, 0xff, 0xff, 0x7f, 0x90, 60, 96, 0x01, 0x80, 60, 0, 0x00, 0xff, 0x2f, 0x00,
        ];
        assert!(split(&smf(480, &[huge]))
            .unwrap_err()
            .contains("MIDI_STEM_TIMING_UNREPRESENTABLE"));
    }
}
