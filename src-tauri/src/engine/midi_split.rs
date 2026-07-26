//! Splits a Standard MIDI File into one single-track file per source track.
//!
//! MuseScore decides on its own how an imported MIDI becomes Parts: it merges
//! tracks that share an instrument, drops empty ones, and splits others. That
//! decomposition is its own, and asking it to reproduce the source track layout
//! is asking for something it never promised — the counts simply disagree, and
//! the audio Part of a MIDI track could not be identified.
//!
//! A MIDI file, unlike a score, can be divided exactly: its tracks are already
//! separate `MTrk` chunks. Each stem is therefore the source chunk copied byte
//! for byte, preceded by a rebuilt meta track carrying only the marks that
//! govern the whole file — tempo, meter, key, SMPTE offset. Nothing is
//! transposed, quantised, or invented; a stem is a subset of the source, and
//! which source track it holds is known because this code chose it.

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

/// Absolute-tick global meta events of one track, plus the tick its last event
/// sits on. Running status is honoured so channel events are skipped exactly.
fn scan_track(body: &[u8]) -> Result<(Vec<TimedMeta>, u32), String> {
    let mut reader = Reader::new(body);
    let mut tick: u32 = 0;
    let mut metas = Vec::new();
    let mut running: Option<u8> = None;
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
            reader.take(length)?;
            if GLOBAL_META.contains(&kind) {
                metas.push((tick, body[start..reader.position].to_vec()));
            }
            running = None;
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
            reader.take(data_bytes)?;
        }
    }
    Ok((metas, tick))
}

fn meta_track(metas: &[TimedMeta], end_tick: u32) -> Vec<u8> {
    let mut body = Vec::new();
    let mut previous = 0u32;
    for (tick, event) in metas {
        write_varint(&mut body, tick.saturating_sub(previous));
        body.extend_from_slice(event);
        previous = *tick;
    }
    write_varint(&mut body, end_tick.saturating_sub(previous));
    body.extend_from_slice(&[0xff, 0x2f, 0x00]);
    chunk(b"MTrk", &body)
}

fn chunk(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(tag);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// One playable single-track file per source track, in source order.
///
/// Every slice keeps its source chunk byte for byte and gains a meta track
/// holding the file's tempo, meter, key and SMPTE marks so it renders on the
/// same timeline as the whole file. A track carrying no events of its own is
/// still returned, so callers can index slices by source track number.
pub fn split_tracks(data: &[u8]) -> Result<Vec<MidiTrackSlice>, String> {
    let header = read_header(data)?;
    let bodies = track_bodies(data, &header)?;
    let mut metas: Vec<TimedMeta> = Vec::new();
    let mut end_tick = 0u32;
    for (start, end) in &bodies {
        let (track_metas, last_tick) = scan_track(&data[*start..*end])?;
        metas.extend(track_metas);
        end_tick = end_tick.max(last_tick);
    }
    // A stable sort keeps two marks written on the same tick in file order, so
    // the meta track reads exactly as the source does.
    metas.sort_by_key(|(tick, _)| *tick);
    let meta = meta_track(&metas, end_tick);

    let mut header_body = Vec::with_capacity(6);
    header_body.extend_from_slice(&1u16.to_be_bytes()); // format 1: parallel tracks
    header_body.extend_from_slice(&2u16.to_be_bytes());
    header_body.extend_from_slice(&header.division.to_be_bytes());
    let prefix = chunk(b"MThd", &header_body);

    Ok(bodies
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
        .collect())
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
}
