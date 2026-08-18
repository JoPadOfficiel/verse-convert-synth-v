use std::collections::BTreeSet;
use verse_lib::engine::convert::convert_auto;
use verse_lib::engine::midi::{self, Kind, LyricState, SourceFormat};
use verse_lib::engine::{musescore, musicxml, target};

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
    assert!(
        target::svp::serialize(&outcome.svp.expect("valid empty project"))
            .expect("exactly representable")
            .tracks
            .is_empty()
    );
}

/// A note the source never texts is left out of the vocal project, never filled
/// in and never deleted from the bundle.
///
/// `"a"`, `"la"`, `"+~"` and `"R"` are each a different way of putting a word in
/// the singer's mouth, and an empty lyric is not an option either: OpenUtau's
/// phonemizer marks one `error`, so writing them produced a project that reads
/// as a failed conversion. A wordless note is not vocal material — it stays in
/// the preserved source and in the stem rendered from it.
#[test]
fn an_untexted_note_is_left_out_and_never_given_a_word() {
    let data = smf(&[
        0x00, 0xff, 0x05, 0x03, b'l', b'e', b't', // lyric "let"
        0x00, 0x90, 60, 100, // C4 on at 0
        0x83, 0x60, 0x80, 60, 0, // off at 480
        0x00, 0x90, 62, 100, // D4 on at 480, never texted
        0x83, 0x60, 0x80, 62, 0, // off at 960
        0x00, 0xff, 0x2f, 0x00,
    ]);
    let outcome = convert_auto(&data, "english");
    assert!(outcome.ok, "{:?}", outcome.msg);
    // The source note is still inventoried: the report counts two.
    assert_eq!(outcome.tracks[0].notes, 2);
    let projected = outcome.svp.expect("a projection");
    assert_eq!(
        projected
            .tracks
            .iter()
            .map(|lane| (lane.name.as_str(), lane.muted, lane.notes.len()))
            .collect::<Vec<_>>(),
        vec![("Track 0", false, 1)],
        "one lane, nothing muted beside it"
    );
    assert_eq!(
        projected.tracks[0]
            .notes
            .iter()
            .map(|note| (note.onset_ticks, note.duration_ticks, note.pitch))
            .collect::<Vec<_>>(),
        vec![(0, 480, 60)],
    );

    let svp = target::svp::serialize(&projected).expect("exactly representable");
    assert!(svp.tracks[0].render_enabled);
    assert!(!svp.tracks[0].main_ref.is_instrumental);
    assert!(svp.tracks[0]
        .main_group
        .notes
        .iter()
        .all(|note| !note.lyrics.is_empty()));

    let ustx = target::ustx::serialize(&projected).expect("exactly representable");
    assert_eq!(
        ustx.tracks
            .iter()
            .map(|track| track.mute)
            .collect::<Vec<_>>(),
        vec![false],
    );
    assert!(ustx.wave_parts.is_empty(), "a projection carries no audio");
    let yaml = target::ustx::to_yaml(&ustx);
    for invented in [
        "lyric: \"a\"",
        "lyric: \"la\"",
        "lyric: \"+~\"",
        "lyric: \"R\"",
        "lyric: \"\"",
    ] {
        assert!(!yaml.contains(invented), "{invented} must not be written");
    }
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
    assert!(target::svp::serialize(&outcome.svp.expect("valid project"))
        .expect("exactly representable")
        .tracks
        .is_empty());
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
fn a_karaoke_container_cannot_qualify_unproven_text() {
    let data = smf(&[
        0x00, 0xff, 0x01, 0x03, b'l', b'e', b't', 0x00, 0xff, 0x2f, 0x00,
    ]);
    let standard = midi::parse(&data).unwrap();
    let karaoke = midi::parse_with_karaoke_profile(&data).unwrap();
    assert_eq!(standard.source_format, SourceFormat::StandardMidi);
    assert_eq!(karaoke.source_format, SourceFormat::KaraokeMidi);
    assert_eq!(
        karaoke.tracks[0].text_profile,
        midi::MidiTextProfile::Generic
    );
}

#[test]
fn merging_a_tie_sustains_the_note_without_losing_any_source_identity() {
    // Merging is a projection decision, not a parsing loss. The tail of a tie
    // must keep its source id and its lyric in the IR so the preservation
    // ledger still accounts for it, while Synthesizer V receives one held note
    // instead of a second attack that would cut the sound in half.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure><voice>
        <Chord><durationType>quarter</durationType>
          <Lyrics><text>shine</text></Lyrics>
          <Note><pitch>65</pitch><Spanner type="Tie"><Tie/><next><location><fractions>1/4</fractions></location></next></Spanner></Note>
        </Chord>
        <Chord><durationType>quarter</durationType>
          <Note><pitch>65</pitch><Spanner type="Tie"><prev><location><fractions>-1/4</fractions></location></prev></Spanner></Note>
        </Chord>
      </voice></Measure>
    </Staff>
  </Score>
</museScore>"#;
    let parsed = musescore::parse(xml.as_bytes()).expect("parse");
    let note_ids: BTreeSet<_> = parsed
        .tracks
        .iter()
        .flat_map(|track| track.events.iter())
        .filter_map(|event| match &event.kind {
            Kind::NoteOn(note) => Some(note.source.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        note_ids.len(),
        2,
        "both source notes must survive the merge"
    );

    let outcome = convert_auto(xml.as_bytes(), "english");
    assert!(outcome.ok, "{:?}", outcome.msg);
    let svp = target::svp::serialize(&outcome.svp.expect("valid SVP")).expect("valid SVP");
    let notes = &svp.tracks[0].main_group.notes;
    assert_eq!(notes.len(), 1, "the tie is sung as one sustained note");
    assert_eq!(notes[0].lyrics, "shine");
    // 2 quarters at 705_600_000 blicks each.
    assert_eq!(notes[0].duration, 1_411_200_000);
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
    let projected = outcome.svp.expect("valid SVP");
    let svp = target::svp::serialize(&projected).expect("valid SVP");
    let soprano = |companion: bool| {
        projected
            .tracks
            .iter()
            .find(|lane| {
                lane.name.contains("Soprano")
                    && lane.name.ends_with(" — untexted notes") == companion
            })
            .expect("the soprano lane")
    };
    // The score opens on an untexted F4. It is not sung, so it no longer opens
    // the sung lane — and it is not invented away either: it opens the muted
    // companion instead, still an F4 and still untexted.
    let sung = soprano(false);
    assert!(
        sung.notes.iter().all(|note| note.lyric.is_sung()),
        "the sung lane holds only notes the source asks to be sung"
    );
    assert!(!sung.muted);
    let untexted = soprano(true);
    assert!(untexted.muted, "the companion opens silent");
    assert!(untexted.notes.iter().all(|note| !note.lyric.is_sung()));
    assert_eq!(untexted.notes[0].pitch, 65);
    // The companion directly follows the lane it belongs to, which is what lets
    // a reader pair them by position rather than by parsing names.
    let position = |lane: &verse_lib::engine::projection::ProjectedTrack| {
        projected
            .tracks
            .iter()
            .position(|candidate| std::ptr::eq(candidate, lane))
            .expect("the lane is in the projection")
    };
    assert_eq!(position(untexted), position(sung) + 1);
    let vocal = svp
        .tracks
        .iter()
        .find(|track| track.name.contains("Soprano"))
        .expect("source-owned soprano track");
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

/// A Soft Karaoke exporter may write the same words twice, once as the Text
/// stream the format is built around and once as MIDI lyric events. Counting
/// both made the file report twice the words it has, so a project singing every
/// one of them still read as having dropped half. Where the two disagree the
/// loser used to vanish with nothing said.
#[test]
fn a_track_writing_its_words_in_both_encodings_states_them_once() {
    fn meta(kind: u8, text: &str) -> Vec<u8> {
        let mut out = vec![0x00, 0xff, kind, text.len() as u8];
        out.extend_from_slice(text.as_bytes());
        out
    }

    for (karaoke, duplicate) in [("Hel", "Hel"), ("Hel", "WRONG")] {
        let mut track = Vec::new();
        track.extend(meta(0x01, "@KMIDI"));
        track.extend(meta(0x01, &format!("\\{karaoke}")));
        track.extend(meta(0x05, duplicate));
        track.extend([0x00, 0x90, 60, 64]);
        track.extend([0x83, 0x60, 0x80, 60, 0]);
        track.extend(meta(0x01, "lo"));
        track.extend(meta(0x05, "lo"));
        track.extend([0x00, 0x90, 62, 64]);
        track.extend([0x83, 0x60, 0x80, 62, 0]);
        track.extend([0x00, 0xff, 0x2f, 0x00]);

        let outcome = convert_auto(&smf(&track), "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        let stated: usize = outcome
            .tracks
            .iter()
            .map(|report| report.lyric_status.source_text_count)
            .sum();
        assert_eq!(
            (stated, outcome.placed),
            (2, 2),
            "two words written twice are two words, and both are sung"
        );

        let project = outcome.svp.as_ref().expect("a projection");
        let sung: Vec<String> = project
            .tracks
            .iter()
            .flat_map(|lane| &lane.notes)
            .map(|note| match &note.lyric {
                verse_lib::engine::projection::ProjectedLyric::Source(lyric) => {
                    match &lyric.state {
                        LyricState::Text(text) => text.clone(),
                        other => format!("{other:?}"),
                    }
                }
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            sung,
            vec![karaoke.to_string(), "lo".to_string()],
            "the karaoke stream is the one this file is built around"
        );

        let codes: Vec<&str> = outcome
            .tracks
            .iter()
            .flat_map(|report| report.warnings.iter())
            .map(|warning| warning.code.as_str())
            .collect();
        assert!(
            codes.contains(&"TWO_LYRIC_ENCODINGS"),
            "choosing between two encodings is never silent: {codes:?}"
        );
    }
}
