use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use verse_lib::engine::convert::convert_auto;
use verse_lib::engine::midi::{self, Kind, LyricState, SourceFormat};
use verse_lib::engine::{musescore, musicxml, target};

fn smf(track: &[u8]) -> Vec<u8> {
    let mut data = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
    data.extend_from_slice(&(track.len() as u32).to_be_bytes());
    data.extend_from_slice(track);
    data
}

fn smf_tracks(tracks: &[Vec<u8>]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"MThd");
    data.extend_from_slice(&6u32.to_be_bytes());
    data.extend_from_slice(&1u16.to_be_bytes());
    data.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    data.extend_from_slice(&480u16.to_be_bytes());
    for track in tracks {
        data.extend_from_slice(b"MTrk");
        data.extend_from_slice(&(track.len() as u32).to_be_bytes());
        data.extend_from_slice(track);
    }
    data
}

fn push_vlq(out: &mut Vec<u8>, mut value: u32) {
    let mut bytes = [0u8; 5];
    let mut len = 0usize;
    loop {
        bytes[len] = (value & 0x7f) as u8;
        len += 1;
        value >>= 7;
        if value == 0 {
            break;
        }
    }
    for index in (0..len).rev() {
        let mut byte = bytes[index];
        if index != 0 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn push_meta(out: &mut Vec<u8>, delta: u32, kind: u8, payload: &[u8]) {
    push_vlq(out, delta);
    out.extend_from_slice(&[0xff, kind, payload.len() as u8]);
    out.extend_from_slice(payload);
}

fn push_note(out: &mut Vec<u8>, delta: u32, status: u8, key: u8, velocity: u8) {
    push_vlq(out, delta);
    out.extend_from_slice(&[status, key, velocity]);
}

fn multi_tempo_midi(karaoke: bool) -> Vec<u8> {
    let mut meta = Vec::new();
    push_meta(&mut meta, 0, 0x51, &[0x0a, 0x2c, 0x2b]); // 90 BPM
    if karaoke {
        push_meta(&mut meta, 0, 0x01, b"@KMIDI KARAOKE FILE");
    }
    push_meta(&mut meta, 960, 0x51, &[0x06, 0x1a, 0x80]); // 150 BPM
    push_meta(&mut meta, 480, 0x51, &[0x0a, 0x2c, 0x2b]); // 90 BPM
    push_meta(&mut meta, 0, 0x2f, &[]);

    let mut soprano = Vec::new();
    push_meta(&mut soprano, 0, 0x03, b"Soprano");
    for (lyric, key) in [
        (b"one".as_slice(), 72),
        (b"two", 74),
        (b"three", 76),
        (b"four", 77),
    ] {
        push_meta(&mut soprano, 0, 0x05, lyric);
        push_note(&mut soprano, 0, 0x90, key, 100);
        push_note(&mut soprano, 480, 0x80, key, 0);
    }
    push_meta(&mut soprano, 0, 0x2f, &[]);

    let mut alto = Vec::new();
    push_meta(&mut alto, 0, 0x03, b"Alto");
    for (lyric, key, duration) in [
        (b"one".as_slice(), 60, 480),
        (b"two".as_slice(), 62, 960),
        (b"three".as_slice(), 64, 480),
        (b"four".as_slice(), 65, 480),
    ] {
        push_meta(&mut alto, 0, 0x05, lyric);
        push_note(&mut alto, 0, 0x90, key, 100);
        push_note(&mut alto, duration, 0x80, key, 0);
    }
    push_meta(&mut alto, 0, 0x2f, &[]);

    smf_tracks(&[meta, soprano, alto])
}

fn multi_tempo_musicxml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list>
    <score-part id="P1"><part-name>Soprano</part-name></score-part>
    <score-part id="P2"><part-name>Alto</part-name></score-part>
  </part-list>
  <part id="P1"><measure number="1">
    <attributes><divisions>480</divisions><time><beats>5</beats><beat-type>4</beat-type></time></attributes>
    <direction><sound tempo="90"/></direction>
    <note><pitch><step>C</step><octave>5</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>one</text></lyric></note>
    <note><pitch><step>D</step><octave>5</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>two</text></lyric></note>
    <direction><sound tempo="150"/></direction>
    <note><pitch><step>E</step><octave>5</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>three</text></lyric></note>
    <direction><sound tempo="90"/></direction>
    <note><pitch><step>F</step><octave>5</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>four</text></lyric></note>
    <note><rest/><duration>480</duration><voice>1</voice></note>
  </measure></part>
  <part id="P2"><measure number="1">
    <attributes><divisions>480</divisions><time><beats>5</beats><beat-type>4</beat-type></time></attributes>
    <note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>one</text></lyric></note>
    <note><pitch><step>D</step><octave>4</octave></pitch><duration>960</duration><voice>1</voice><lyric><text>two</text></lyric></note>
    <note><pitch><step>E</step><octave>4</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>three</text></lyric></note>
    <note><pitch><step>F</step><octave>4</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>four</text></lyric></note>
  </measure></part>
</score-partwise>"#
}

fn mxl(xml: &str) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default();
    writer
        .start_file("META-INF/container.xml", options)
        .expect("MXL container entry");
    writer
        .write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?><container><rootfiles><rootfile full-path="score.musicxml"/></rootfiles></container>"#,
        )
        .expect("MXL container contents");
    writer
        .start_file("score.musicxml", options)
        .expect("MXL score entry");
    writer
        .write_all(xml.as_bytes())
        .expect("MXL score contents");
    writer.finish().expect("MXL archive").into_inner()
}

fn mscz(xml: &str) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file("score.mscx", zip::write::SimpleFileOptions::default())
        .expect("MSCZ score entry");
    writer
        .write_all(xml.as_bytes())
        .expect("MSCZ score contents");
    writer.finish().expect("MSCZ archive").into_inner()
}

fn multi_tempo_mscx() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Soprano</trackName><Staff id="1"/></Part>
    <Part><trackName>Alto</trackName><Staff id="2"/></Part>
    <Staff id="1"><Measure><voice>
      <Tempo><tempo>1.5</tempo></Tempo>
      <Chord><durationType>quarter</durationType><Lyrics><text>one</text></Lyrics><Note><pitch>72</pitch></Note></Chord>
      <Chord><durationType>quarter</durationType><Lyrics><text>two</text></Lyrics><Note><pitch>74</pitch></Note></Chord>
      <Tempo><tempo>2.5</tempo></Tempo>
      <Chord><durationType>quarter</durationType><Lyrics><text>three</text></Lyrics><Note><pitch>76</pitch></Note></Chord>
      <Tempo><tempo>1.5</tempo></Tempo>
      <Chord><durationType>quarter</durationType><Lyrics><text>four</text></Lyrics><Note><pitch>77</pitch></Note></Chord>
    </voice></Measure></Staff>
    <Staff id="2"><Measure><voice>
      <Chord><durationType>quarter</durationType><Lyrics><text>one</text></Lyrics><Note><pitch>60</pitch></Note></Chord>
      <Chord><durationType>half</durationType><Lyrics><text>two</text></Lyrics><Note><pitch>62</pitch></Note></Chord>
      <Chord><durationType>quarter</durationType><Lyrics><text>three</text></Lyrics><Note><pitch>64</pitch></Note></Chord>
      <Chord><durationType>quarter</durationType><Lyrics><text>four</text></Lyrics><Note><pitch>65</pitch></Note></Chord>
    </voice></Measure></Staff>
  </Score>
</museScore>"#
}

fn assert_global_multi_tempo_across_targets(data: &[u8], label: &str) {
    let outcome = convert_auto(data, "english");
    assert!(outcome.ok, "{label}: {:?}", outcome.msg);
    let projected = outcome
        .svp
        .unwrap_or_else(|| panic!("{label}: missing projected project"));
    assert_eq!(
        projected
            .tempos
            .iter()
            .map(|tempo| (tempo.tick, tempo.bpm))
            .collect::<Vec<_>>(),
        vec![(0, 89.999955), (960, 150.0), (1_440, 89.999955)],
        "{label}: shared tempo map changed before OpenUtau serialization"
    );

    let projected_onsets = |part_name: &str| {
        projected
            .tracks
            .iter()
            .find(|track| track.name.contains(part_name))
            .unwrap_or_else(|| panic!("{label}: missing projected {part_name} lane"))
            .notes
            .iter()
            .map(|note| note.onset_ticks)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        projected_onsets("Soprano"),
        vec![0, 480, 960, 1_440],
        "{label}: Soprano timeline moved around a tempo change"
    );
    assert_eq!(
        projected_onsets("Alto"),
        vec![0, 480, 1_440, 1_920],
        "{label}: Alto timeline must use the same global tempo map"
    );
    let alto = projected
        .tracks
        .iter()
        .find(|track| track.name.contains("Alto"))
        .unwrap_or_else(|| panic!("{label}: missing projected Alto lane"));
    assert_eq!(
        (alto.notes[1].onset_ticks, alto.notes[1].duration_ticks),
        (480, 960),
        "{label}: Alto sustained note must cross the middle tempo change intact"
    );

    let svp = target::svp::serialize(&projected)
        .unwrap_or_else(|error| panic!("{label}: serialize Synthesizer V project: {error}"));
    assert_eq!(
        svp.time
            .tempo
            .iter()
            .map(|tempo| (tempo.position, tempo.bpm))
            .collect::<Vec<_>>(),
        vec![
            (0, 89.999955),
            (1_411_200_000, 150.0),
            (2_116_800_000, 89.999955),
        ],
        "{label}: Synthesizer V must receive every global tempo change"
    );

    let ustx = target::ustx::serialize(&projected)
        .unwrap_or_else(|error| panic!("{label}: serialize OpenUtau project: {error}"));
    assert_eq!(
        ustx.tempos
            .iter()
            .map(|tempo| (tempo.position, tempo.bpm))
            .collect::<Vec<_>>(),
        vec![(0, 89.999955), (960, 150.0), (1_440, 89.999955)],
        "{label}: OpenUtau must receive every global tempo change"
    );
    for (part_name, expected) in [
        ("Soprano", vec![0, 480, 960, 1_440]),
        ("Alto", vec![0, 480, 1_440, 1_920]),
    ] {
        let track_no = ustx
            .tracks
            .iter()
            .position(|track| track.track_name.contains(part_name))
            .unwrap_or_else(|| panic!("{label}: missing OpenUtau {part_name} track"))
            as i32;
        let part = ustx
            .voice_parts
            .iter()
            .find(|part| part.track_no == track_no)
            .unwrap_or_else(|| panic!("{label}: missing OpenUtau {part_name} voice part"));
        assert_eq!(
            part.notes
                .iter()
                .map(|note| part.position + note.position)
                .collect::<Vec<_>>(),
            expected,
            "{label}: OpenUtau {part_name} note positions changed around tempo changes"
        );
    }

    let yaml = target::ustx::to_yaml(&ustx);
    let tempo_line = yaml
        .lines()
        .find(|line| line.starts_with("tempos: "))
        .unwrap_or_else(|| panic!("{label}: emitted USTX has no tempos list"));
    assert_eq!(
        tempo_line.matches("bpm:").count(),
        3,
        "{label}: emitted USTX must contain all three tempo entries"
    );
    assert!(tempo_line.contains("position: 960, bpm: 150"));
    assert!(tempo_line.contains("position: 1440, bpm: 89.999955"));
}

#[test]
fn every_supported_source_family_keeps_global_tempo_changes_for_openutau() {
    let midi_data = multi_tempo_midi(false);
    assert_eq!(
        midi::parse(&midi_data)
            .expect("parse multi-tempo MIDI")
            .source_format,
        SourceFormat::StandardMidi
    );

    let karaoke_data = multi_tempo_midi(true);
    assert_eq!(
        midi::parse(&karaoke_data)
            .expect("parse multi-tempo KAR")
            .source_format,
        SourceFormat::KaraokeMidi
    );

    let musicxml_data = multi_tempo_musicxml().as_bytes().to_vec();
    assert_eq!(
        musicxml::parse(&musicxml_data)
            .expect("parse multi-tempo MusicXML")
            .source_format,
        SourceFormat::MusicXml
    );
    let mxl_data = mxl(multi_tempo_musicxml());
    assert_eq!(
        musicxml::parse(&mxl_data)
            .expect("parse multi-tempo MXL")
            .source_format,
        SourceFormat::MusicXml
    );

    let mscx_data = multi_tempo_mscx().as_bytes().to_vec();
    assert_eq!(
        musescore::parse(&mscx_data)
            .expect("parse multi-tempo MSCX")
            .source_format,
        SourceFormat::MuseScore
    );
    let mscz_data = mscz(multi_tempo_mscx());
    assert_eq!(
        musescore::parse(&mscz_data)
            .expect("parse multi-tempo MSCZ")
            .source_format,
        SourceFormat::MuseScore
    );

    for (label, data) in [
        ("MIDI (.mid/.midi)", midi_data),
        ("Karaoke MIDI (.kar)", karaoke_data),
        ("MusicXML (.xml/.musicxml)", musicxml_data),
        ("compressed MusicXML (.mxl)", mxl_data),
        ("MuseScore XML (.mscx)", mscx_data),
        ("compressed MuseScore (.mscz)", mscz_data),
    ] {
        assert_global_multi_tempo_across_targets(&data, label);
    }
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

    // Retention alone above proves no editable transfer: include an actual
    // sung lyric and assert an active USTX curve plus mapped-event evidence.
    let singing = smf(&[
        0, 0xff, 5, 3, b'l', b'e', b't', 0, 0xb2, 7, 99, 0, 0xe2, 0, 64, 0, 0x92, 64, 73, 0x81,
        0x70, 0x82, 64, 12, 0, 0xff, 0x2f, 0,
    ]);
    let parsed = midi::parse(&singing).unwrap();
    let before = parsed.tracks.clone();
    let outcome = verse_lib::engine::convert::convert_midi_with_target(
        &parsed,
        "english",
        None,
        target::ExportTarget::Ustx,
    );
    assert!(outcome.ok);
    assert_eq!(outcome.placed, 1);
    let project = target::ustx::serialize(outcome.svp.as_ref().unwrap()).unwrap();
    assert_eq!(project.voice_parts[0].notes[0].tone, 64);
    assert_eq!(project.voice_parts[0].notes[0].lyric, "let");
    assert_eq!(project.voice_parts[0].curves.len(), 2);
    assert!(project.voice_parts[0]
        .curves
        .iter()
        .any(|curve| curve.abbr == "dyn" && curve.ys.iter().any(|value| *value < 0)));
    assert!(outcome
        .projection
        .performance_mapped
        .contains_key("event:midi-track-0:1"));
    assert_eq!(parsed.tracks, before);
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
fn multi_tempo_mscz_preserves_shared_vocal_timeline_in_both_targets() {
    let data = mscz(multi_tempo_mscx());
    let parsed = musescore::parse(&data).expect("parse synthetic multi-tempo MSCZ");
    assert_eq!(parsed.source_format, SourceFormat::MuseScore);

    let outcome = convert_auto(&data, "english");
    assert!(outcome.ok, "{:?}", outcome.msg);
    let projected = outcome.svp.expect("valid multi-tempo projection");
    assert_eq!(
        projected
            .tempos
            .iter()
            .map(|tempo| (tempo.tick, tempo.bpm))
            .collect::<Vec<_>>(),
        vec![(0, 89.999955), (960, 150.0), (1_440, 89.999955)]
    );

    let lane_onsets = |name: &str| {
        projected
            .tracks
            .iter()
            .find(|lane| lane.name == name)
            .unwrap_or_else(|| panic!("missing projected {name} lane"))
            .notes
            .iter()
            .map(|note| note.onset_ticks)
            .collect::<Vec<_>>()
    };
    assert_eq!(lane_onsets("Soprano"), vec![0, 480, 960, 1_440]);
    assert_eq!(lane_onsets("Alto"), vec![0, 480, 1_440, 1_920]);
    let alto = projected
        .tracks
        .iter()
        .find(|lane| lane.name == "Alto")
        .expect("projected Alto lane");
    assert_eq!(
        (alto.notes[1].onset_ticks, alto.notes[1].duration_ticks),
        (480, 960),
        "Alto must retain the note that sustains across the middle tempo change"
    );

    let svp = target::svp::serialize(&projected).expect("serialize multi-tempo SVP");
    assert_eq!(
        svp.time
            .tempo
            .iter()
            .map(|tempo| (tempo.position, tempo.bpm))
            .collect::<Vec<_>>(),
        vec![
            (0, 89.999955),
            (1_411_200_000, 150.0),
            (2_116_800_000, 89.999955),
        ]
    );
    for name in ["Soprano", "Alto"] {
        let track = svp
            .tracks
            .iter()
            .find(|track| track.name == name)
            .unwrap_or_else(|| panic!("missing serialized SVP {name} track"));
        assert_eq!(
            track
                .main_group
                .notes
                .iter()
                .map(|note| note.onset)
                .collect::<Vec<_>>(),
            if name == "Soprano" {
                vec![0, 705_600_000, 1_411_200_000, 2_116_800_000]
            } else {
                vec![0, 705_600_000, 2_116_800_000, 2_822_400_000]
            }
        );
    }

    let ustx = target::ustx::serialize(&projected).expect("serialize multi-tempo USTX");
    assert_eq!(
        ustx.tempos
            .iter()
            .map(|tempo| (tempo.position, tempo.bpm))
            .collect::<Vec<_>>(),
        vec![(0, 89.999955), (960, 150.0), (1_440, 89.999955)]
    );
    for name in ["Soprano", "Alto"] {
        let track_no = ustx
            .tracks
            .iter()
            .position(|track| track.track_name == name)
            .unwrap_or_else(|| panic!("missing serialized USTX {name} track"))
            as i32;
        let part = ustx
            .voice_parts
            .iter()
            .find(|part| part.track_no == track_no)
            .unwrap_or_else(|| panic!("missing serialized USTX {name} voice part"));
        assert_eq!(
            part.notes
                .iter()
                .map(|note| part.position + note.position)
                .collect::<Vec<_>>(),
            if name == "Soprano" {
                vec![0, 480, 960, 1_440]
            } else {
                vec![0, 480, 1_440, 1_920]
            }
        );
    }
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
    let projected = outcome.svp.clone().expect("valid SVP");
    let svp = target::svp::serialize(&projected).expect("valid SVP");
    let soprano = projected
        .tracks
        .iter()
        .find(|lane| lane.name.contains("Soprano"))
        .expect("the soprano lane");
    // The score opens on an untexted F4. A note with no word is not something a
    // singer can be asked to sing, so it is left out of the vocal project and
    // counted — it is not invented away, and it is not moved to a muted
    // companion lane either: that doubled the track count and filled the project
    // with notes OpenUtau marks `error`.
    assert!(
        soprano.notes.iter().all(|note| note.lyric.is_sung()),
        "the sung lane holds only notes the source asks to be sung"
    );
    assert!(!soprano.muted, "a sung lane never opens silent");
    assert!(
        projected
            .tracks
            .iter()
            .all(|lane| !lane.name.ends_with(" — untexted notes")),
        "the muted companion lane is superseded and must not come back"
    );
    let left_out: usize = outcome
        .tracks
        .iter()
        .flat_map(|track| track.warnings.iter())
        .filter(|warning| warning.code == "UNTEXTED_NOTES_LEFT_OUT")
        .filter_map(|warning| {
            warning
                .message
                .split_whitespace()
                .next()
                .and_then(|count| count.parse::<usize>().ok())
        })
        .sum();
    assert_eq!(
        left_out, 3,
        "the notes the source never texted are reported rather than dropped in silence"
    );
    assert_eq!(
        soprano.notes.len() + left_out,
        174,
        "every source note is either sung or accounted for"
    );
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
fn supplied_multi_tempo_musescore_gate() {
    let Ok(path) = std::env::var("VERSE_MULTI_TEMPO_MSCZ_GATE") else {
        return;
    };
    let data = std::fs::read(path).expect("read supplied multi-tempo MSCZ");
    let parsed = musescore::parse(&data).expect("parse supplied multi-tempo MSCZ");

    let mut source_tempos = BTreeMap::new();
    for track in &parsed.tracks {
        for event in &track.events {
            if let Kind::Tempo(micros) = event.kind {
                assert!(micros > 0, "source tempo must be positive");
                assert!(
                    source_tempos.insert(event.tick, micros).is_none(),
                    "the supplied regression score must not contain competing tempo events at one tick"
                );
            }
        }
    }
    assert_eq!(
        source_tempos.len(),
        3,
        "the supplied regression score must expose the three authored tempo changes"
    );

    let outcome = convert_auto(&data, "english");
    assert!(outcome.ok, "{:?}", outcome.msg);
    let projected = outcome.svp.expect("valid multi-tempo projection");
    assert_eq!(
        projected.tempos.len(),
        source_tempos.len(),
        "projection must preserve every source tempo change exactly once"
    );
    for (tempo, (&tick, &micros)) in projected.tempos.iter().zip(source_tempos.iter()) {
        assert_eq!(tempo.tick, tick, "tempo tick must remain source-exact");
        let source_bpm = 60_000_000.0 / f64::from(micros);
        assert!(
            (tempo.bpm - source_bpm).abs() < 0.000_001,
            "tempo at tick {tick} changed from {source_bpm} to {} BPM",
            tempo.bpm
        );
    }
    for (tempo, expected) in projected.tempos.iter().zip([90.0, 150.0, 90.0]) {
        assert!(
            (tempo.bpm - expected).abs() < 0.001,
            "expected the supplied 90 -> 150 -> 90 map, got {} BPM at tick {}",
            tempo.bpm,
            tempo.tick
        );
    }

    let change_ticks: Vec<_> = projected
        .tempos
        .iter()
        .skip(1)
        .map(|tempo| tempo.tick)
        .collect();
    let mut verified_source_tracks: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for expected_name in ["Soprano", "Alto"] {
        let matching_lanes: Vec<_> = projected
            .tracks
            .iter()
            .filter(|lane| lane.name.contains(expected_name))
            .collect();
        assert!(
            !matching_lanes.is_empty(),
            "missing projected {expected_name} lane"
        );
        for lane in matching_lanes {
            let source_track = parsed
                .tracks
                .iter()
                .find(|track| track.id == lane.source_track_id)
                .expect("projected lane must retain source-track identity");
            assert!(
                source_track.name.contains(expected_name),
                "projected {expected_name} lane must retain its matching source track"
            );
            verified_source_tracks
                .entry(expected_name)
                .or_default()
                .insert(source_track.id.clone());

            for note in &lane.notes {
                let evidence = note
                    .source_evidence
                    .as_ref()
                    .expect("projected source note must retain source evidence");
                let origin = evidence
                    .origin
                    .as_ref()
                    .expect("projected source note must retain source origin");
                let origin_track = parsed
                    .tracks
                    .iter()
                    .find(|track| track.id == origin.track_id)
                    .expect("projected source origin must name a parsed source track");
                let source_note_on = origin_track
                    .events
                    .iter()
                    .find(|event| event.order == origin.note_on_order)
                    .expect("projected source origin must name a note-on event");
                assert!(matches!(source_note_on.kind, Kind::NoteOn(_)));
                assert_eq!(
                    note.onset_ticks, source_note_on.tick,
                    "projected {expected_name} note {} moved from source tick {}",
                    evidence.note_id, source_note_on.tick
                );
            }

            let source_note_ticks: Vec<_> = source_track
                .events
                .iter()
                .filter_map(|event| matches!(event.kind, Kind::NoteOn(_)).then_some(event.tick))
                .collect();
            for change_tick in &change_ticks {
                let source_spans_change = source_note_ticks.iter().any(|tick| tick < change_tick)
                    && source_note_ticks.iter().any(|tick| tick >= change_tick);
                if !source_spans_change {
                    continue;
                }
                let projected_before = lane
                    .notes
                    .iter()
                    .any(|note| note.onset_ticks < *change_tick);
                let projected_after = lane
                    .notes
                    .iter()
                    .any(|note| note.onset_ticks >= *change_tick);
                assert!(
                    projected_before && projected_after,
                    "projected {expected_name} lane {} lost note onsets across the shared tempo change at tick {change_tick}",
                    lane.name
                );
            }
        }
    }
    let soprano_sources = verified_source_tracks
        .get("Soprano")
        .expect("verified Soprano source tracks");
    let alto_sources = verified_source_tracks
        .get("Alto")
        .expect("verified Alto source tracks");
    assert!(
        soprano_sources.is_disjoint(alto_sources),
        "Soprano and Alto must be distinct source tracks"
    );

    let svp = target::svp::serialize(&projected).expect("serialize supplied multi-tempo SVP");
    let ustx = target::ustx::serialize(&projected).expect("serialize supplied multi-tempo USTX");
    assert_eq!(svp.time.tempo.len(), 3, "SVP must keep all three tempos");
    assert_eq!(ustx.tempos.len(), 3, "USTX must keep all three tempos");
    for (tempo, source) in svp.time.tempo.iter().zip(&projected.tempos) {
        let numerator = u128::from(source.tick) * u128::from(target::svp::BLICKS_PER_QUARTER);
        let denominator = u128::from(projected.ticks_per_beat);
        assert_eq!(
            numerator % denominator,
            0,
            "SVP tempo must be exactly representable"
        );
        assert_eq!(tempo.position, (numerator / denominator) as i64);
        assert!((tempo.bpm - source.bpm).abs() < 0.001);
    }
    for (tempo, source) in ustx.tempos.iter().zip(&projected.tempos) {
        let numerator = u128::from(source.tick) * u128::from(target::ustx::TICKS_PER_QUARTER);
        let denominator = u128::from(projected.ticks_per_beat);
        assert_eq!(
            numerator % denominator,
            0,
            "USTX tempo must be exactly representable"
        );
        assert_eq!(tempo.position, (numerator / denominator) as i32);
        assert!((tempo.bpm - source.bpm).abs() < 0.001);
    }
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

/// A track qualifies as karaoke on a line control plus two payloads, which two
/// section markers satisfy on their own. Reading the Text stream there sang the
/// marker `Chorus` and left seven of the eight words the source states out of
/// the project, because those words were in the Lyric meta events the marker
/// displaced. The words are what the file is for.
#[test]
fn section_markers_never_displace_the_words_a_track_states() {
    fn meta(kind: u8, text: &str) -> Vec<u8> {
        let mut out = vec![0x00, 0xff, kind, text.len() as u8];
        out.extend_from_slice(text.as_bytes());
        out
    }
    let words = ["Right", "this", "way", "your", "ta", "bles", "wai", "ting"];
    let mut track = Vec::new();
    // A line control and one more payload: enough to look like karaoke.
    track.extend(meta(0x01, "/Chorus"));
    track.extend(meta(0x01, "Intro"));
    for (index, word) in words.iter().enumerate() {
        track.extend(meta(0x05, word));
        track.extend([0x00, 0x90, 60 + index as u8, 100]);
        track.extend([0x83, 0x60, 0x80, 60 + index as u8, 0]);
    }
    track.extend([0x00, 0xff, 0x2f, 0x00]);

    let outcome = convert_auto(&smf(&track), "english");
    assert!(outcome.ok, "{:?}", outcome.msg);
    assert_eq!(
        outcome.placed,
        words.len(),
        "every word the source states is sung"
    );
    let sung: Vec<String> = outcome
        .svp
        .as_ref()
        .expect("a projection")
        .tracks
        .iter()
        .flat_map(|lane| &lane.notes)
        .filter_map(|note| match &note.lyric {
            verse_lib::engine::projection::ProjectedLyric::Source(lyric) => match &lyric.state {
                LyricState::Text(text) => Some(text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(sung, words.map(str::to_string).to_vec());
}

/// A Soft Karaoke exporter may write the same words twice, once as MIDI lyric
/// events and once as the Text stream the format is built around. Counting both
/// made the file report twice the words it has, so a project singing every one
/// of them still read as having dropped half. Where the two disagree the loser
/// used to vanish with nothing said.
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
            vec![duplicate.to_string(), "lo".to_string()],
            "the Lyric meta event is the event MIDI defines for words"
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
