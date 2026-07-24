use std::collections::BTreeSet;
use verse_lib::engine::convert::convert_auto;
use verse_lib::engine::midi::{self, Kind, LyricState, SourceFormat};
use verse_lib::engine::{musescore, musicxml};

fn smf(track: &[u8]) -> Vec<u8> {
    let mut data = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
    data.extend_from_slice(&(track.len() as u32).to_be_bytes());
    data.extend_from_slice(track);
    data
}

#[test]
fn lyric_free_midi_succeeds_without_a_synthetic_vocal_track() {
    let data = smf(&[
        0x00, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0, 0x00, 0xff, 0x2f, 0x00,
    ]);
    let parsed = midi::parse(&data).expect("valid MIDI");
    assert_eq!(parsed.source_format, SourceFormat::StandardMidi);
    let outcome = convert_auto(&data, "english");
    assert!(outcome.ok, "{:?}", outcome.msg);
    assert_eq!(outcome.tracks.len(), 1);
    assert_eq!(outcome.tracks[0].notes, 1);
    assert_eq!(outcome.tracks[0].role, "backing");
    assert!(outcome.svp.expect("valid empty project").tracks.is_empty());
}

#[test]
fn generic_midi_text_is_not_a_lyric_and_performance_events_survive() {
    let data = smf(&[
        0x00, 0xff, 0x01, 0x03, b'l', b'e', b't', // generic Text
        0x00, 0xc2, 12, // program
        0x00, 0xb2, 7, 99, // controller
        0x00, 0xe2, 0, 64, // centred pitch bend
        0x00, 0x92, 64, 73, // note on
        0x81, 0x70, 0x82, 64, 12, // note off
        0x00, 0xff, 0x2f, 0x00,
    ]);
    let parsed = midi::parse(&data).expect("valid MIDI");
    assert_eq!(parsed.source_format, SourceFormat::StandardMidi);
    let events = &parsed.tracks[0].events;
    assert!(events.iter().any(|event| matches!(
        event.kind,
        Kind::ProgramChange {
            channel: 2,
            program: 12
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind,
        Kind::ControlChange {
            channel: 2,
            controller: 7,
            value: 99
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind,
        Kind::PitchBend {
            channel: 2,
            value: 8192
        }
    )));
    let outcome = convert_auto(&data, "english");
    assert!(outcome.ok);
    assert_eq!(outcome.placed, 0);
    assert!(outcome.svp.expect("valid project").tracks.is_empty());
}

#[test]
fn note_on_velocity_zero_remains_distinguishable_and_closes_the_note() {
    let data = smf(&[
        0x00, 0x90, 67, 81, 0x81, 0x70, 0x90, 67, 0, 0x00, 0xff, 0x2f, 0x00,
    ]);
    let parsed = midi::parse(&data).unwrap();
    assert!(parsed.tracks[0]
        .events
        .iter()
        .any(|event| matches!(&event.kind, Kind::NoteOn(note) if note.velocity == Some(0))));
    let outcome = convert_auto(&data, "english");
    assert!(outcome.ok);
    assert_eq!(outcome.tracks[0].notes, 1);
    assert_eq!(outcome.placed, 0);
}

#[test]
fn a_karaoke_extension_hint_cannot_qualify_unproven_text() {
    let data = smf(&[
        0x00, 0xff, 0x01, 0x03, b'l', b'e', b't', 0x00, 0xff, 0x2f, 0x00,
    ]);
    let standard = midi::parse(&data).unwrap();
    let karaoke = midi::parse_with_karaoke_profile(&data).unwrap();
    assert_eq!(standard.source_format, SourceFormat::StandardMidi);
    assert_eq!(karaoke.source_format, SourceFormat::StandardMidi);
    assert_eq!(
        karaoke.tracks[0].text_profile,
        midi::MidiTextProfile::Generic
    );
}

#[test]
fn supplied_musescore_gate_when_configured() {
    let Ok(path) = std::env::var("VERSE_MSCZ_GATE") else {
        return;
    };
    let data = std::fs::read(path).expect("read supplied MSCZ");
    let parsed = musescore::parse(&data).expect("parse supplied MSCZ");
    let mut note_ids = BTreeSet::new();
    let mut lyric_ids = BTreeSet::new();
    for track in &parsed.tracks {
        for event in &track.events {
            if let Kind::NoteOn(note) = &event.kind {
                note_ids.insert(note.source.id.clone());
                for lyric in &note.lyrics {
                    lyric_ids.insert(lyric.id.clone());
                }
            }
        }
    }
    assert_eq!(note_ids.len(), 924, "all source notes must survive");
    assert_eq!(lyric_ids.len(), 171, "all source lyrics must survive once");

    let outcome = convert_auto(&data, "english");
    assert!(outcome.ok, "{:?}", outcome.msg);
    let svp = outcome.svp.expect("valid SVP");
    let vocal = svp
        .tracks
        .iter()
        .find(|track| track.name.contains("Soprano"))
        .expect("source-owned soprano track");
    assert_eq!(vocal.main_group.notes[0].pitch, 65);
    assert_eq!(vocal.main_group.notes[0].lyrics, "");
    assert_eq!(
        vocal
            .main_group
            .notes
            .iter()
            .find(|note| !note.lyrics.is_empty())
            .map(|note| note.lyrics.as_str()),
        Some("let")
    );
    let source_la = parsed
        .tracks
        .iter()
        .flat_map(|track| track.events.iter())
        .filter_map(|event| match &event.kind {
            Kind::NoteOn(note) => Some(note.lyrics.iter()),
            _ => None,
        })
        .flatten()
        .filter(|lyric| matches!(&lyric.state, LyricState::Text(text) if text == "la"))
        .count();
    let projected_la = vocal
        .main_group
        .notes
        .iter()
        .filter(|note| note.lyrics == "la")
        .count();
    assert_eq!(
        projected_la, source_la,
        "every projected `la` must have source provenance"
    );
}

#[test]
fn supplied_musicxml_percussion_gate_when_configured() {
    let Ok(path) = std::env::var("VERSE_MXL_GATE") else {
        return;
    };
    let data = std::fs::read(path).expect("read supplied MXL");
    let parsed = musicxml::parse(&data).expect("parse supplied MXL");
    let percussion_ids: BTreeSet<_> = parsed
        .tracks
        .iter()
        .filter(|track| track.source.part_id.as_deref() == Some("P6"))
        .flat_map(|track| track.events.iter())
        .filter_map(|event| match &event.kind {
            Kind::NoteOn(note) if note.source.unpitched.is_some() => Some(note.source.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        percussion_ids.len(),
        695,
        "all source percussion notes must remain inventoried"
    );
    assert!(parsed
        .tracks
        .iter()
        .filter(|track| track.source.part_id.as_deref() == Some("P6"))
        .flat_map(|track| track.instruments.iter())
        .any(|instrument| {
            instrument.source_channel == Some(10)
                && instrument.channel == Some(9)
                && instrument.midi_unpitched.is_some()
        }));
}
