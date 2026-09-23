//! Authored, redistributable evidence fixtures. No private score data.
use std::io::Write;

pub fn smf(tracks: &[Vec<u8>]) -> Vec<u8> {
    let mut data = b"MThd\0\0\0\x06\0\x01".to_vec();
    data.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    data.extend_from_slice(&480u16.to_be_bytes());
    for track in tracks {
        data.extend_from_slice(b"MTrk");
        data.extend_from_slice(&(track.len() as u32).to_be_bytes());
        data.extend_from_slice(track);
    }
    data
}

pub fn vlq(bytes: &mut Vec<u8>, mut value: u32) {
    let mut encoded = vec![(value & 127) as u8];
    value >>= 7;
    while value > 0 {
        encoded.push((value & 127) as u8 | 128);
        value >>= 7;
    }
    bytes.extend(encoded.into_iter().rev());
}

pub fn midi() -> Vec<u8> {
    let mut tracks = Vec::new();
    for (name, channel, program, pitches, words) in [
        ("Voice", 0, 52, [60, 64], ["Hello", "World"]),
        ("Guitar", 1, 24, [48, 52], ["", ""]),
        ("Bass", 2, 33, [36, 40], ["", ""]),
        ("Drums", 9, 0, [35, 38], ["drum", "beat"]),
    ] {
        let mut track = vec![0, 0xff, 3, name.len() as u8];
        track.extend_from_slice(name.as_bytes());
        track.extend_from_slice(&[0, 0xc0 | channel, program]);
        for (pitch, word) in pitches.into_iter().zip(words) {
            if !word.is_empty() {
                track.extend_from_slice(&[0, 0xff, 5, word.len() as u8]);
                track.extend_from_slice(word.as_bytes());
            }
            track.extend_from_slice(&[0, 0x90 | channel, pitch, 96]);
            vlq(&mut track, 480);
            track.extend_from_slice(&[0x80 | channel, pitch, 0]);
        }
        track.extend_from_slice(&[0, 0xff, 0x2f, 0]);
        tracks.push(track);
    }
    smf(&tracks)
}

pub fn musicxml() -> String {
    let mut declarations = String::new();
    let mut parts = String::new();
    for (id, name, channel, program, octave, words) in [
        ("V", "Voice", 1, 53, 4, ["Hello", "World"]),
        ("G", "Guitar", 2, 25, 3, ["", ""]),
        ("B", "Bass", 3, 34, 2, ["", ""]),
        ("D", "Drums", 10, 1, 2, ["drum", "beat"]),
    ] {
        declarations.push_str(&format!(r#"<score-part id="{id}"><part-name>{name}</part-name><score-instrument id="{id}-I"><instrument-name>{name}</instrument-name></score-instrument><midi-instrument id="{id}-I"><midi-channel>{channel}</midi-channel><midi-program>{program}</midi-program></midi-instrument></score-part>"#));
        parts.push_str(&format!(r#"<part id="{id}"><measure number="1"><attributes><divisions>480</divisions><time><beats>2</beats><beat-type>4</beat-type></time></attributes>"#));
        for (step, word) in ["C", "E"].into_iter().zip(words) {
            let lyric = if word.is_empty() {
                String::new()
            } else {
                format!("<lyric><text>{word}</text></lyric>")
            };
            // Deliberately pitched-looking drum keys, identified by source MIDI channel.
            parts.push_str(&format!(r#"<note><pitch><step>{step}</step><octave>{octave}</octave></pitch><duration>480</duration><instrument id="{id}-I"/><voice>1</voice><type>quarter</type><staff>1</staff>{lyric}</note>"#));
        }
        parts.push_str("</measure></part>");
    }
    format!(
        r#"<score-partwise version="4.0"><part-list>{declarations}</part-list>{parts}</score-partwise>"#
    )
}

pub fn mscx() -> String {
    let mut parts = String::new();
    let mut staves = String::new();
    for (staff, name, sound, program, pitches, words) in [
        (1, "Voice", "voice.vocals", 52, [60, 64], ["Hello", "World"]),
        (2, "Guitar", "pluck.guitar", 24, [48, 52], ["", ""]),
        (3, "Bass", "pluck.bass", 33, [36, 40], ["", ""]),
        (4, "Drums", "drum.group", 0, [35, 38], ["drum", "beat"]),
    ] {
        let drum = if staff == 4 {
            "<useDrumset>1</useDrumset>"
        } else {
            ""
        };
        let channel = if staff == 4 { 9 } else { staff - 1 };
        parts.push_str(&format!(r#"<Part><trackName>{name}</trackName><Staff id="{staff}"/><Instrument id="{name}-I"><instrumentId>{sound}</instrumentId>{drum}<Channel><program value="{program}"/><midiPort>0</midiPort><midiChannel>{channel}</midiChannel></Channel></Instrument></Part>"#));
        staves.push_str(&format!(r#"<Staff id="{staff}"><Measure><voice>"#));
        for (pitch, word) in pitches.into_iter().zip(words) {
            let lyric = if word.is_empty() {
                String::new()
            } else {
                format!("<Lyrics><text>{word}</text></Lyrics>")
            };
            staves.push_str(&format!("<Chord><durationType>quarter</durationType>{lyric}<Note><pitch>{pitch}</pitch></Note></Chord>"));
        }
        staves.push_str("</voice></Measure></Staff>");
    }
    format!(
        r#"<museScore version="4.0"><Score><Division>480</Division>{parts}{staves}</Score></museScore>"#
    )
}

pub fn archive(name: &str, xml: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("META-INF/container.xml", options).unwrap();
    write!(
        zip,
        r#"<container><rootfiles><rootfile full-path="{name}"/></rootfiles></container>"#
    )
    .unwrap();
    zip.start_file(name, options).unwrap();
    zip.write_all(xml.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

pub fn cases() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("mid", midi()),
        ("midi", midi()),
        ("kar", midi()),
        ("musicxml", musicxml().into_bytes()),
        ("xml", musicxml().into_bytes()),
        ("mxl", archive("score.musicxml", &musicxml())),
        ("mscx", mscx().into_bytes()),
        ("mscz", archive("score.mscx", &mscx())),
    ]
}
