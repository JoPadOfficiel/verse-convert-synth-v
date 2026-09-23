use std::collections::HashMap;
use verse_lib::engine::convert::convert_midi_with_profile;
use verse_lib::engine::midi::{
    self, InstrumentInfo, Kind, Midi, NoteInstrumentRole, TrackRoleHint,
};
use verse_lib::engine::target::{ExportTarget, PronunciationProfile};
use verse_lib::engine::{midi_split, musescore, musicxml, target};
use verse_lib::stems::StemPlan;

#[path = "support/percussion_fixtures.rs"]
mod fixtures;

fn parse(extension: &str, data: &[u8]) -> Midi {
    match extension {
        "kar" => midi::parse_with_karaoke_profile(data),
        "mid" | "midi" => midi::parse(data),
        "mscx" | "mscz" => musescore::parse(data),
        _ => musicxml::parse(data),
    }
    .unwrap()
}

fn exported(midi: &Midi, target: ExportTarget, force: bool) -> (Vec<u8>, usize) {
    let before = midi.tracks.clone();
    let overrides: HashMap<_, _> = (0..midi.tracks.len()).map(|i| (i, true)).collect();
    let result = convert_midi_with_profile(
        midi,
        "english",
        force.then_some(&overrides),
        target,
        PronunciationProfile::Default,
    );
    assert!(result.ok, "{:?}", result.msg);
    assert_eq!(
        before, midi.tracks,
        "conversion must not rewrite source evidence"
    );
    let project = result.svp.as_ref().unwrap();
    for note in project.tracks.iter().flat_map(|track| &track.notes) {
        let origin = note
            .source_evidence
            .as_ref()
            .unwrap()
            .origin
            .as_ref()
            .unwrap();
        assert!(origin.source.instrument_role.allows_vocal());
        assert!(origin.source.unpitched.is_none());
    }
    let count = project.tracks.iter().map(|track| track.notes.len()).sum();
    let bytes = target::serialize_to(target, project).unwrap();
    (bytes, count)
}

fn assert_serialized_voice(bytes: &[u8], target: ExportTarget) {
    match target {
        ExportTarget::Svp => {
            let data: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            let notes: Vec<_> = data["tracks"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|t| t["mainGroup"]["notes"].as_array().unwrap())
                .collect();
            assert_eq!(notes.len(), 2);
            for (n, (onset, pitch, lyric)) in notes
                .iter()
                .zip([(0, 60, "Hello"), (705_600_000u64, 64, "World")])
            {
                assert_eq!(n["onset"], onset);
                assert_eq!(n["duration"], 705_600_000u64);
                assert_eq!(n["pitch"], pitch);
                assert_eq!(n["lyrics"], lyric);
            }
        }
        ExportTarget::Ustx => {
            let text = std::str::from_utf8(bytes).unwrap();
            target::ustx::audit(text).unwrap();
            let values = |prefix: &str| {
                text.lines()
                    .filter_map(|l| l.trim().strip_prefix(prefix))
                    .collect::<Vec<_>>()
            };
            assert_eq!(values("tone: "), ["60", "64"]);
            assert_eq!(values("duration: "), ["480", "480"]);
            assert_eq!(values("lyric: "), ["\"Hello\"", "\"World\""]);
            // Actual emitted note positions, distinct from part/track positions.
            assert!(text.contains("      - position: 0\n        duration: 480"));
            assert!(text.contains("      - position: 480\n        duration: 480"));
        }
    }
}

#[test]
fn all_eight_formats_keep_drums_out_of_both_serialized_targets_and_preserve_stem_owners() {
    for (extension, bytes) in fixtures::cases() {
        let midi = parse(extension, &bytes);
        assert_eq!(midi.topology.parts.len(), 4, "{extension}");
        assert_eq!(
            midi.tracks
                .iter()
                .filter(|t| t.role_hint == TrackRoleHint::Percussion)
                .count(),
            1,
            "{extension}"
        );
        assert_eq!(
            midi.tracks
                .iter()
                .flat_map(|t| &t.events)
                .filter(|e| matches!(e.kind, Kind::NoteOn(_)))
                .count(),
            8
        );
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let (output, count) = exported(&midi, target, false);
            assert_eq!(count, 2, "{extension} {target:?}");
            assert_serialized_voice(&output, target);
            let (forced, count) = exported(&midi, target, true);
            assert_eq!(
                count, 6,
                "guitar and bass overrides must survive in {extension}"
            );
            let text = std::str::from_utf8(&forced).unwrap();
            assert!(!text.contains("\"drum\"") && !text.contains("\"beat\""));
            let result = convert_midi_with_profile(
                &midi,
                "english",
                None,
                target,
                PronunciationProfile::Default,
            );
            assert!(result.tracks.iter().any(|t| t
                .warnings
                .iter()
                .any(|d| d.code == "SOURCE_PERCUSSION_NOT_VOCAL")));
            let plan = StemPlan::from_source(&midi, &result.tracks).unwrap();
            assert_eq!(
                plan.stems.len(),
                4,
                "drums must remain in accompaniment: {extension}"
            );
            for part in &midi.topology.parts {
                assert_eq!(
                    plan.stems
                        .iter()
                        .filter(|s| s.source_part_id == part.id)
                        .count(),
                    1
                );
            }
        }
    }
}

fn one_part(inventory: &str, notes: &str) -> String {
    format!(
        r#"<score-partwise version="4.0"><part-list><score-part id="P"><part-name>Mixed</part-name>{inventory}</score-part></part-list><part id="P"><measure number="1"><attributes><divisions>480</divisions></attributes>{notes}</measure></part></score-partwise>"#
    )
}

fn inventory() -> &'static str {
    r#"<score-instrument id="voice"><instrument-name>Voice</instrument-name></score-instrument><midi-instrument id="voice"><midi-channel>1</midi-channel><midi-program>53</midi-program></midi-instrument><score-instrument id="drums"><instrument-name>Drums</instrument-name></score-instrument><midi-instrument id="drums"><midi-channel>10</midi-channel><midi-program>1</midi-program></midi-instrument>"#
}

fn xml_note(refs: &str, word: &str) -> String {
    format!(
        r#"<note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration>{refs}<lyric><text>{word}</text></lyric></note>"#
    )
}

#[test]
fn musicxml_used_note_owners_preserve_melody_with_mixed_and_unused_percussion_inventory() {
    for notes in [
        xml_note(r#"<instrument id="voice"/>"#, "Hello"),
        format!(
            "{}{}",
            xml_note(r#"<instrument id="voice"/>"#, "Hello"),
            xml_note(r#"<instrument id="drums"/>"#, "drum")
        ),
    ] {
        let midi = musicxml::parse(one_part(inventory(), &notes).as_bytes()).unwrap();
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            for force in [false, true] {
                let (bytes, count) = exported(&midi, target, force);
                assert_eq!(count, 1);
                assert!(!std::str::from_utf8(&bytes).unwrap().contains("\"drum\""));
            }
        }
    }
}

#[test]
fn unresolved_musicxml_owners_keep_source_evidence_and_emit_stable_diagnostics() {
    for refs in [
        "",
        r#"<instrument id="missing"/>"#,
        r#"<instrument id="voice"/><instrument id="drums"/>"#,
        "<instrument/>",
    ] {
        let xml = one_part(inventory(), &xml_note(refs, "Hello"));
        let midi = musicxml::parse(xml.as_bytes()).unwrap();
        let before = midi.tracks.clone();
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            assert_eq!(exported(&midi, target, true).1, 0, "{refs}");
            let out = convert_midi_with_profile(
                &midi,
                "english",
                None,
                target,
                PronunciationProfile::Default,
            );
            assert!(
                out.tracks.iter().any(|t| t
                    .warnings
                    .iter()
                    .any(|d| d.code == "SOURCE_INSTRUMENT_OWNERSHIP_UNRESOLVED")),
                "{refs}"
            );
        }
        assert_eq!(before, midi.tracks);
    }
}

#[test]
fn unpitched_or_percussion_clef_without_instrument_metadata_never_invents_vocal_pitch() {
    for note in [
        xml_note("", "Hello").replace("<pitch><step>C</step><octave>4</octave></pitch>", "<unpitched><display-step>C</display-step><display-octave>4</display-octave></unpitched>"),
        format!("<attributes><clef><sign>percussion</sign></clef></attributes>{}", xml_note("", "Hello")),
    ] {
        let midi = musicxml::parse(one_part("", &note).as_bytes()).unwrap();
        let before = midi.tracks.clone();
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            assert_eq!(exported(&midi, target, true).1, 0);
            let result = convert_midi_with_profile(
                &midi,
                "english",
                None,
                target,
                PronunciationProfile::Default,
            );
            assert!(result.tracks.iter().any(|track| track
                .warnings
                .iter()
                .any(|warning| warning.code == "SOURCE_PERCUSSION_NOT_VOCAL")));
        }
        assert_eq!(before, midi.tracks);
    }
}

#[test]
fn native_unused_drum_channel_does_not_own_the_initial_pitched_notes() {
    let xml = fixtures::mscx().replacen(
        "</Channel>",
        "</Channel><Channel><midiChannel>9</midiChannel><program value=\"0\"/></Channel>",
        1,
    );
    let midi = musescore::parse(xml.as_bytes()).unwrap();
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        assert_serialized_voice(&exported(&midi, target, false).0, target);
    }
}

#[test]
fn native_note_instrument_reference_and_subchannel_select_the_same_channel_variant() {
    let xml = fixtures::mscx()
        .replacen(
            "<Channel><program value=\"52\"/><midiPort>0</midiPort><midiChannel>0</midiChannel></Channel>",
            "<Channel><program value=\"52\"/><midiPort>0</midiPort><midiChannel>0</midiChannel></Channel><Channel><program value=\"0\"/><midiPort>0</midiPort><midiChannel>9</midiChannel></Channel>",
            1,
        )
        .replacen(
            "<Note><pitch>60</pitch></Note>",
            "<Note><pitch>60</pitch><instrument id=\"Voice-I\"/><subchannel>1</subchannel></Note>",
            1,
        )
        .replacen(
            "<Note><pitch>64</pitch></Note>",
            "<Note><pitch>64</pitch><instrument id=\"Voice-I\"/><subchannel>0</subchannel></Note>",
            1,
        );
    let midi = musescore::parse(xml.as_bytes()).unwrap();
    let voice = midi
        .tracks
        .iter()
        .find(|track| track.name.starts_with("Voice"))
        .unwrap();
    let roles: Vec<_> = voice
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            Kind::NoteOn(note) if note.velocity != Some(0) => {
                Some(voice.note_instrument_role(note))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        roles,
        [NoteInstrumentRole::Percussion, NoteInstrumentRole::Pitched]
    );
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        assert_eq!(exported(&midi, target, false).1, 1);
        assert_eq!(exported(&midi, target, true).1, 5);
    }

    let invalid = xml.replace("<subchannel>1</subchannel>", "<subchannel>99</subchannel>");
    let midi = musescore::parse(invalid.as_bytes()).unwrap();
    let out = convert_midi_with_profile(
        &midi,
        "english",
        None,
        ExportTarget::Svp,
        PronunciationProfile::Default,
    );
    assert!(out.tracks.iter().any(|track| track
        .warnings
        .iter()
        .any(|warning| { warning.code == "SOURCE_INSTRUMENT_OWNERSHIP_UNRESOLVED" })));
}

#[test]
fn notation_taxonomy_alone_keeps_declared_percussion_out_of_vocals() {
    for sound in [
        "drum.group",
        "percussion.snare-drum",
        "pitched-percussion.xylophone",
    ] {
        let inventory = format!(
            "<score-instrument id=\"drums\"><instrument-name>Drums</instrument-name><instrument-sound>{sound}</instrument-sound></score-instrument>"
        );
        let xml = one_part(&inventory, &xml_note(r#"<instrument id="drums"/>"#, "drum"));
        let midi = musicxml::parse(xml.as_bytes()).unwrap();
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            assert_eq!(exported(&midi, target, true).1, 0, "{sound} {target:?}");
        }
    }

    let xml = fixtures::mscx()
        .replace("<useDrumset>1</useDrumset>", "")
        .replace(
            "<midiChannel>9</midiChannel>",
            "<midiChannel>3</midiChannel>",
        );
    let midi = musescore::parse(xml.as_bytes()).unwrap();
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        assert_eq!(exported(&midi, target, true).1, 6, "{target:?}");
    }
}

fn insert_first(xml: &str, at: &str, inserted: &str) -> String {
    xml.replacen(at, &format!("{at}{inserted}"), 1)
}

fn insert_last(xml: &str, at: &str, inserted: &str) -> String {
    let index = xml.rfind(at).unwrap() + at.len();
    format!("{}{inserted}{}", &xml[..index], &xml[index..])
}

/// Track, tick, key, channel, instrument, role and lyrics of one note.
type OwnedNote = (
    String,
    u32,
    Option<u8>,
    Option<u8>,
    Option<String>,
    String,
    Vec<String>,
);

/// Every note with its instant, key, channel, owner and lyrics; element
/// positions in source IDs are not part of ownership.
fn owned_notes(midi: &Midi) -> Vec<OwnedNote> {
    midi.tracks
        .iter()
        .flat_map(|track| {
            track
                .events
                .iter()
                .filter_map(move |event| match &event.kind {
                    Kind::NoteOn(note) => Some((
                        track.id.clone(),
                        event.tick,
                        note.key,
                        note.channel,
                        note.source.instrument_id.clone(),
                        format!("{:?}", note.source.instrument_role),
                        note.lyrics.iter().map(|lyric| lyric.raw.clone()).collect(),
                    )),
                    _ => None,
                })
        })
        .collect()
}

fn assert_refused(result: Result<Midi, String>, case: &str) {
    assert!(
        result
            .unwrap_err()
            .contains("SOURCE_INSTRUMENT_OWNERSHIP_UNRESOLVED"),
        "{case}"
    );
}

#[test]
fn pitched_only_mid_score_changes_keep_the_initial_owner() {
    let baseline = musescore::parse(fixtures::mscx().as_bytes()).unwrap();
    for change in [
        "<InstrumentChange><Instrument><instrumentId>pluck.harp</instrumentId><Channel><program value=\"46\"/><midiChannel>0</midiChannel></Channel></Instrument></InstrumentChange>",
        "<InstrumentChange><Instrument id=\"oboe\"><instrumentId>wind.reed.oboe</instrumentId></Instrument></InstrumentChange>",
        "<channelSwitch voice=\"0\" name=\"open\"/>",
        "<articulationChange voice=\"0\" name=\"staccato\"/>",
        "<StaffTypeChange><StaffType group=\"pitched\"><name>stdNormal</name></StaffType></StaffTypeChange>",
    ] {
        let midi =
            musescore::parse(insert_first(&fixtures::mscx(), "<voice>", change).as_bytes())
                .unwrap_or_else(|error| panic!("{change}: {error}"));
        assert_eq!(owned_notes(&midi), owned_notes(&baseline), "{change}");
        assert_eq!(
            midi.tracks.iter().map(|track| &track.instruments).collect::<Vec<_>>(),
            baseline.tracks.iter().map(|track| &track.instruments).collect::<Vec<_>>()
        );
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            assert_eq!(
                exported(&midi, target, false).1,
                exported(&baseline, target, false).1
            );
        }
    }
    let notes = xml_note(r#"<instrument id="voice"/>"#, "Hello");
    let baseline = musicxml::parse(one_part(inventory(), &notes).as_bytes()).unwrap();
    for sound in [
        r#"<midi-instrument id="voice"><midi-channel>2</midi-channel><midi-program>41</midi-program></midi-instrument>"#,
        r#"<midi-device port="2"/>"#,
        r#"<instrument-change id="voice"><instrument-sound>wind.reed.oboe</instrument-sound></instrument-change>"#,
    ] {
        let xml = one_part(
            inventory(),
            &format!("<direction><sound>{sound}</sound></direction>{notes}"),
        );
        let midi =
            musicxml::parse(xml.as_bytes()).unwrap_or_else(|error| panic!("{sound}: {error}"));
        assert_eq!(owned_notes(&midi), owned_notes(&baseline), "{sound}");
    }
}

#[test]
fn percussion_identity_changes_are_refused_without_reassigning_notes() {
    let notes = xml_note(r#"<instrument id="voice"/>"#, "Hello");
    for sound in [
        r#"<midi-instrument id="voice"><midi-channel>10</midi-channel></midi-instrument>"#,
        r#"<midi-instrument id="voice"><midi-unpitched>36</midi-unpitched></midi-instrument>"#,
        r#"<midi-instrument id="drums"><midi-channel>1</midi-channel></midi-instrument>"#,
        r#"<instrument-change id="voice"><instrument-sound>drum.snare-drum</instrument-sound></instrument-change>"#,
        r#"<midi-instrument><midi-channel>10</midi-channel></midi-instrument>"#,
    ] {
        let xml = one_part(
            inventory(),
            &format!("<direction><sound>{sound}</sound></direction>{notes}"),
        );
        assert_refused(musicxml::parse(xml.as_bytes()), sound);
    }

    let to_drums = "<InstrumentChange><Instrument><instrumentId>drum.group</instrumentId><useDrumset>1</useDrumset></Instrument></InstrumentChange>";
    let to_channel_ten = "<InstrumentChange><Instrument><instrumentId>keyboard.piano</instrumentId><Channel><midiChannel>9</midiChannel></Channel></Instrument></InstrumentChange>";
    let to_harp = "<InstrumentChange><Instrument><instrumentId>pluck.harp</instrumentId></Instrument></InstrumentChange>";
    let to_percussion_staff =
        "<StaffTypeChange><StaffType group=\"percussion\"/></StaffTypeChange>";
    let to_pitched_staff = "<StaffTypeChange><StaffType group=\"pitched\"/></StaffTypeChange>";
    let percussion_declared = fixtures::mscx().replace(
        "<Staff id=\"4\"/>",
        "<Staff id=\"4\"><StaffType group=\"percussion\"/></Staff>",
    );
    let mixed_channels = fixtures::mscx().replace(
        "<Channel><program value=\"52\"/><midiPort>0</midiPort><midiChannel>0</midiChannel></Channel>",
        "<Channel><program value=\"52\"/><midiPort>0</midiPort><midiChannel>0</midiChannel></Channel><Channel name=\"perc\"><midiPort>0</midiPort><midiChannel>9</midiChannel></Channel>",
    );
    for (case, xml) in [
        (
            "to a drumset",
            insert_first(&fixtures::mscx(), "<voice>", to_drums),
        ),
        (
            "to channel 10",
            insert_first(&fixtures::mscx(), "<voice>", to_channel_ten),
        ),
        (
            "from a drumset",
            insert_last(&fixtures::mscx(), "<voice>", to_harp),
        ),
        (
            "unreadable target",
            insert_first(&fixtures::mscx(), "<voice>", "<InstrumentChange/>"),
        ),
        (
            "to a percussion staff",
            insert_first(&fixtures::mscx(), "<voice>", to_percussion_staff),
        ),
        (
            "from a percussion staff",
            insert_last(&percussion_declared, "<voice>", to_pitched_staff),
        ),
        (
            "channel switch in a mixed instrument",
            insert_first(
                &mixed_channels,
                "<voice>",
                "<channelSwitch voice=\"0\" name=\"perc\"/>",
            ),
        ),
    ] {
        assert_refused(musescore::parse(xml.as_bytes()), case);
    }
}

#[test]
fn native_midi_channel_and_both_program_forms_preserve_actual_instrument_ownership() {
    for program in [
        "<program>24</program>",
        "<program value=\"-1\">24</program>",
        "<program value=\"24\"/>",
    ] {
        let xml = fixtures::mscx()
            .replace("<program value=\"24\"/>", program)
            .replace("<useDrumset>1</useDrumset>", "")
            .replace("<midiPort>0</midiPort>", "<midiPort>2</midiPort>");
        let midi = musescore::parse(xml.as_bytes()).unwrap();
        let guitar = midi
            .tracks
            .iter()
            .find(|track| track.name.starts_with("Guitar"))
            .unwrap();
        let instrument = guitar.instrument.as_ref().unwrap();
        assert_eq!(instrument.program, Some(24));
        assert_eq!(instrument.source_program, Some(24));
        assert_eq!(instrument.channel, Some(1));
        assert_eq!(instrument.source_port, Some(2));
        assert_eq!(instrument.port, Some(2));
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            assert_serialized_voice(&exported(&midi, target, false).0, target);
            assert_eq!(
                exported(&midi, target, true).1,
                6,
                "native channel 9 remains percussion without useDrumset"
            );
        }
    }
    let unassigned = fixtures::mscx()
        .replacen(
            "<midiChannel>0</midiChannel>",
            "<midiChannel>-1</midiChannel>",
            1,
        )
        .replace("<midiPort>0</midiPort>", "<midiPort>-1</midiPort>");
    let midi = musescore::parse(unassigned.as_bytes()).unwrap();
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        assert_serialized_voice(&exported(&midi, target, false).0, target);
    }
}

#[test]
fn malformed_native_instrument_declarations_are_refused_instead_of_disappearing() {
    for (original, replacements) in [
        (
            "<midiChannel>0</midiChannel>",
            vec![
                "<midiChannel/>",
                "<midiChannel>bad</midiChannel>",
                "<midiChannel>16</midiChannel>",
                "<midiChannel>-2</midiChannel>",
                "<midiChannel>999999999999999999999</midiChannel>",
                "<midiChannel>0</midiChannel><channel>9</channel>",
            ],
        ),
        (
            "<midiPort>0</midiPort>",
            vec![
                "<midiPort/>",
                "<midiPort>-2</midiPort>",
                "<midiPort>999999999999999999999</midiPort>",
            ],
        ),
        (
            "<program value=\"52\"/>",
            vec![
                "<program/>",
                "<program value=\"bad\"/>",
                "<program>128</program>",
                "<program value=\"52\">24</program>",
                "<controller ctrl=\"0\" value=\"128\"/>",
                "<useDrumset>invalid</useDrumset><program value=\"52\"/>",
            ],
        ),
    ] {
        for replacement in replacements {
            // useDrumset is owned by Instrument, not Channel.
            let xml = if replacement.contains("<useDrumset>") {
                fixtures::mscx().replacen(
                    "<Channel>",
                    "<useDrumset>invalid</useDrumset><Channel>",
                    1,
                )
            } else {
                fixtures::mscx().replacen(original, replacement, 1)
            };
            let error = musescore::parse(xml.as_bytes()).unwrap_err();
            assert!(
                error.contains("SOURCE_INSTRUMENT_OWNERSHIP_UNRESOLVED"),
                "{replacement}: {error}"
            );
        }
    }
}

#[test]
fn malformed_musicxml_ownership_never_becomes_an_absent_pitched_default() {
    for tag in [
        "midi-channel",
        "midi-program",
        "midi-unpitched",
        "midi-bank",
    ] {
        for value in ["", "bad", "0", "-1", "999999999999999999999999"] {
            let declaration = format!(
                r#"<score-instrument id="owner"><instrument-name>Source</instrument-name></score-instrument><midi-instrument id="owner"><{tag}>{value}</{tag}></midi-instrument>"#
            );
            let xml = one_part(
                &declaration,
                &xml_note(r#"<instrument id="owner"/>"#, "Hello"),
            );
            let error = musicxml::parse(xml.as_bytes()).unwrap_err();
            assert!(
                error.contains("SOURCE_INSTRUMENT_OWNERSHIP_UNRESOLVED"),
                "{tag}={value}: {error}"
            );
        }
    }
    for declaration in ["<score-instrument/>", "<midi-instrument/>", "<midi-instrument id=\"\"/>", "<midi-instrument id=\"owner\"><midi-channel>1</midi-channel><midi-channel>10</midi-channel></midi-instrument>"] {
        let xml = one_part(declaration, &xml_note("", "Hello"));
        assert!(musicxml::parse(xml.as_bytes()).unwrap_err().contains("SOURCE_INSTRUMENT_OWNERSHIP_UNRESOLVED"));
    }
}

#[test]
fn contradictory_generic_ir_instrument_ids_are_not_resolved_by_first_match() {
    let mut track = midi::Track::new("test", 0);
    track.instruments = vec![
        InstrumentInfo {
            id: Some("owner".into()),
            channel: Some(0),
            ..Default::default()
        },
        InstrumentInfo {
            id: Some("owner".into()),
            channel: Some(9),
            percussion: true,
            ..Default::default()
        },
    ];
    let mut note = midi::NoteOn {
        channel: Some(0),
        key: Some(60),
        velocity: Some(96),
        source: midi::NoteSource {
            instrument_id: Some("owner".into()),
            ..Default::default()
        },
        lyrics: vec![],
    };
    for role in [NoteInstrumentRole::Unspecified, NoteInstrumentRole::Pitched] {
        note.source.instrument_role = role;
        assert_eq!(
            track.note_instrument_role(&note),
            NoteInstrumentRole::Unresolved
        );
    }
    track.instruments.pop();
    note.source.instrument_ids = vec!["another-owner".into()];
    assert_eq!(
        track.note_instrument_role(&note),
        NoteInstrumentRole::Unresolved
    );
}

#[test]
fn both_analysis_and_writers_reject_tampered_percussion_provenance() {
    let midi = parse("mid", &fixtures::midi());
    let result = convert_midi_with_profile(
        &midi,
        "english",
        None,
        ExportTarget::Svp,
        PronunciationProfile::Default,
    );
    for role in [
        NoteInstrumentRole::Percussion,
        NoteInstrumentRole::Unresolved,
    ] {
        let mut project = result.svp.clone().unwrap();
        project.tracks[0].notes[0]
            .source_evidence
            .as_mut()
            .unwrap()
            .origin
            .as_mut()
            .unwrap()
            .source
            .instrument_role = role;
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let error = target::validate_for(target, &project).unwrap_err();
            assert!(error.contains("SOURCE_INSTRUMENT_NOT_VOCAL"));
            assert!(target::serialize_to(target, &project).is_err());
        }
    }
}

/// Channel events a stem carries: its own track's, with nothing added.
fn channel_events(bytes: &[u8]) -> Vec<(u32, Kind)> {
    midi::parse(bytes)
        .unwrap()
        .tracks
        .iter()
        .flat_map(|track| &track.events)
        .filter(|event| {
            matches!(
                event.kind,
                Kind::NoteOn(_)
                    | Kind::NoteOff(_)
                    | Kind::ControlChange { .. }
                    | Kind::ProgramChange { .. }
                    | Kind::PitchBend { .. }
                    | Kind::ChannelPressure { .. }
                    | Kind::PolyPressure { .. }
                    | Kind::SysEx { .. }
            )
        })
        .map(|event| {
            let mut kind = event.kind.clone();
            if let Kind::NoteOn(note) = &mut kind {
                // Parser IDs name the track position, which a stem changes.
                note.source = Default::default();
            }
            (event.tick, kind)
        })
        .collect()
}

/// A MIDI stem is the global marks plus the byte-identical source `MTrk`,
/// never another track's channel state.
fn assert_per_track_stems(source: &[u8]) {
    let slices = midi_split::split_tracks(source).unwrap();
    let mut offset = 14;
    for slice in slices {
        let len = u32::from_be_bytes(source[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let chunk = &source[offset..offset + 8 + len];
        assert!(
            slice.bytes.ends_with(chunk),
            "the chosen MTrk must remain byte-identical"
        );
        let meta_len = u32::from_be_bytes(slice.bytes[18..22].try_into().unwrap()) as usize;
        assert_eq!(
            slice.bytes.len(),
            22 + meta_len + chunk.len(),
            "only the global context track precedes the source chunk"
        );
        let alone = fixtures::smf(&[source[offset + 8..offset + 8 + len].to_vec()]);
        assert_eq!(channel_events(&slice.bytes), channel_events(&alone));
        offset += 8 + len;
    }
}

#[test]
fn midi_stems_are_the_byte_identical_track_and_its_own_channel_state() {
    assert_per_track_stems(&fixtures::midi());
    for slice in midi_split::split_tracks(&fixtures::midi()).unwrap() {
        assert_eq!(
            channel_events(&slice.bytes)
                .iter()
                .filter(|(_, kind)| matches!(kind, Kind::NoteOn(_)))
                .count(),
            2
        );
    }
    // Another track's bank/program on the same channel is not copied in:
    // MuseScore applies channel state per imported track.
    let state = vec![0, 0xb0, 0, 3, 0, 0xc0, 24, 0, 0xff, 0x2f, 0];
    let notes = vec![
        0x83, 0x60, 0x90, 60, 96, 0x83, 0x60, 0x80, 60, 0, 0, 0xff, 0x2f, 0,
    ];
    let source = fixtures::smf(&[state, notes.clone()]);
    assert_per_track_stems(&source);
    assert!(
        !channel_events(&midi_split::split_tracks(&source).unwrap()[1].bytes)
            .iter()
            .any(|(_, kind)| matches!(
                kind,
                Kind::ProgramChange { .. } | Kind::ControlChange { .. }
            ))
    );
}

#[test]
fn midi_stem_simultaneous_cross_track_state_no_longer_blocks_the_split() {
    let notes = vec![0, 0x90, 60, 96, 0x83, 0x60, 0x80, 60, 0, 0, 0xff, 0x2f, 0];
    let state = vec![0, 0xc0, 24, 0, 0xff, 0x2f, 0];
    let source = fixtures::smf(&[state, notes.clone()]);
    assert_per_track_stems(&source);
    let other_port = vec![0, 0xff, 0x21, 1, 1, 0, 0xc0, 24, 0, 0xff, 0x2f, 0];
    assert_per_track_stems(&fixtures::smf(&[other_port, notes]));
}

fn events(messages: &[(u32, &[u8])]) -> Vec<u8> {
    let mut track = Vec::new();
    for (delta, message) in messages {
        fixtures::vlq(&mut track, *delta);
        track.extend_from_slice(message);
    }
    track.extend_from_slice(&[0, 0xff, 0x2f, 0]);
    track
}

#[test]
fn mixed_channel_midi_and_kar_keep_the_pitched_voice_and_the_original_stem() {
    let track = events(&[
        (0, &[0xc0, 52]),
        (0, &[0xc9, 0]),
        (0, &[0xff, 5, 5, b'H', b'e', b'l', b'l', b'o']),
        (0, &[0x90, 60, 96]),
        (0, &[0x99, 35, 96]),
        (480, &[0x80, 60, 0]),
        (0, &[0x89, 35, 0]),
        (0, &[0xff, 5, 5, b'W', b'o', b'r', b'l', b'd']),
        (0, &[0x90, 64, 96]),
        (0, &[0x99, 38, 96]),
        (480, &[0x80, 64, 0]),
        (0, &[0x89, 38, 0]),
    ]);
    let bytes = fixtures::smf(&[track]);
    for extension in ["mid", "midi", "kar"] {
        let midi = parse(extension, &bytes);
        assert_eq!(midi.topology.parts.len(), 1);
        assert!(midi
            .tracks
            .iter()
            .all(|track| track.instruments.len() == 2 && track.instrument.is_none()));
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            for force in [false, true] {
                let (output, count) = exported(&midi, target, force);
                assert_eq!(count, 2, "{extension} {target:?} force={force}");
                assert_serialized_voice(&output, target);
            }
            let result = convert_midi_with_profile(
                &midi,
                "english",
                None,
                target,
                PronunciationProfile::Default,
            );
            assert_eq!(
                StemPlan::from_source(&midi, &result.tracks)
                    .unwrap()
                    .stems
                    .len(),
                1
            );
        }
    }
    let slices = midi_split::split_tracks(&bytes).unwrap();
    assert_eq!(slices.len(), 1);
    assert!(slices[0].bytes.ends_with(&bytes[14..]));
}

#[test]
fn distinct_midi_ports_keep_distinct_programs_and_unsafe_note_collisions_are_refused() {
    let source = fixtures::smf(&[events(&[
        (0, &[0xc0, 24]),
        (0, &[0x90, 48, 96]),
        (480, &[0x80, 48, 0]),
        (0, &[0xff, 0x21, 1, 1]),
        (0, &[0xc0, 33]),
        (0, &[0x90, 36, 96]),
        (480, &[0x80, 36, 0]),
    ])]);
    let midi = midi::parse(&source).unwrap();
    assert!(midi.tracks[0].instrument.is_none());
    assert_eq!(
        midi.tracks[0]
            .instruments
            .iter()
            .map(|instrument| (instrument.port, instrument.channel, instrument.program))
            .collect::<Vec<_>>(),
        [(Some(0), Some(0), Some(24)), (Some(1), Some(0), Some(33))]
    );
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        assert_eq!(exported(&midi, target, true).1, 2);
    }
    assert!(midi_split::split_tracks(&source).unwrap()[0]
        .bytes
        .ends_with(&source[14..]));
    for conflicting in [&[0x90, 60, 96][..], &[0x80, 60, 0][..]] {
        let bytes = fixtures::smf(&[events(&[
            (0, &[0x90, 60, 96]),
            (0, &[0xff, 0x21, 1, 1]),
            (480, conflicting),
        ])]);
        assert!(midi::parse(&bytes)
            .unwrap_err()
            .contains("SOURCE_INSTRUMENT_OWNERSHIP_UNRESOLVED"));
    }
    for invalid in [
        &[0xff, 0x21, 0][..],
        &[0xff, 0x21, 1, 128][..],
        &[0xc0, 128][..],
        &[0x90, 128, 96][..],
    ] {
        let bytes = fixtures::smf(&[events(&[(0, invalid)])]);
        assert!(midi::parse(&bytes).is_err());
        assert!(midi_split::split_tracks(&bytes).is_err());
    }
}

#[test]
fn musicxml_device_ports_are_scoped_to_their_source_instruments() {
    let declarations = format!(
        r#"{}<midi-device id="voice" port="2"/><midi-device id="drums" port="3"/>"#,
        inventory()
    );
    let midi = musicxml::parse(
        one_part(
            &declarations,
            &xml_note(r#"<instrument id="voice"/>"#, "Hello"),
        )
        .as_bytes(),
    )
    .unwrap();
    let track = midi
        .tracks
        .iter()
        .find(|track| !track.instruments.is_empty())
        .unwrap();
    for (id, raw, normalized) in [("voice", 2, 1), ("drums", 3, 2)] {
        let instrument = track
            .instruments
            .iter()
            .find(|instrument| instrument.id.as_deref() == Some(id))
            .unwrap();
        assert_eq!(instrument.source_port, Some(raw));
        assert_eq!(instrument.port, Some(normalized));
    }
    for device in [
        r#"<midi-device port="0"/>"#,
        r#"<midi-device port="17"/>"#,
        r#"<midi-device port="bad"/>"#,
        r#"<midi-device id="missing" port="2"/>"#,
        r#"<midi-device port="2"/><midi-device id="voice" port="3"/>"#,
    ] {
        let xml = one_part(&format!("{}{device}", inventory()), &xml_note("", "Hello"));
        assert!(musicxml::parse(xml.as_bytes())
            .unwrap_err()
            .contains("SOURCE_INSTRUMENT_OWNERSHIP_UNRESOLVED"));
    }
}

#[test]
fn midi_stems_never_inject_bank_program_bend_pressure_or_sustain_from_other_tracks() {
    let state = events(&[
        (0, &[0xff, 0x21, 1, 2]),
        (0, &[0xb0, 0, 2]),
        (0, &[0xb0, 32, 5]),
        (0, &[0xc0, 24]),
        (0, &[0xb0, 7, 90]),
        (0, &[0xb0, 10, 40]),
        (120, &[0xb0, 64, 127]),
        (0, &[0xe0, 0, 72]),
        (0, &[0xd0, 65]),
        (0, &[0xa0, 60, 30]),
        (120, &[0xc0, 33]),
        (480, &[0xb0, 64, 0]),
        (0, &[0xc1, 100]),
    ]);
    let notes = events(&[
        (0, &[0xff, 0x21, 1, 2]),
        (0, &[0xc0, 52]),
        (480, &[0x90, 60, 96]),
        (480, &[0x80, 60, 0]),
    ]);
    let source = fixtures::smf(&[state, notes.clone()]);
    assert_per_track_stems(&source);
    let slices = midi_split::split_tracks(&source).unwrap();
    assert_eq!(
        channel_events(&slices[1].bytes)
            .into_iter()
            .filter(|(_, kind)| !matches!(kind, Kind::NoteOn(_) | Kind::NoteOff(_)))
            .collect::<Vec<_>>(),
        [(
            0,
            Kind::ProgramChange {
                channel: 0,
                program: 52
            }
        )],
        "only the track's own program change survives"
    );
    let midi = midi::parse(&slices[1].bytes).unwrap();
    assert!(midi
        .tracks
        .iter()
        .flat_map(|track| &track.instruments)
        .all(|instrument| instrument.port == Some(2) && instrument.program == Some(52)));
}

#[test]
fn midi_stems_report_shared_notes_keep_system_state_local_and_refuse_bad_timing() {
    let on = events(&[(0, &[0x90, 60, 96])]);
    let off = events(&[(480, &[0x80, 60, 0])]);
    let divided = midi_split::split(&fixtures::smf(&[on, off])).unwrap();
    assert_eq!(divided.slices.len(), 2);
    assert_eq!(
        divided
            .shared_note_routes
            .iter()
            .map(|route| (
                route.port,
                route.channel,
                route.source_tracks.clone(),
                route.keys.clone()
            ))
            .collect::<Vec<_>>(),
        [(0, 0, vec![0, 1], vec![60])]
    );
    let notes = events(&[(480, &[0x90, 60, 96]), (480, &[0x80, 60, 0])]);
    let system = events(&[(0, &[0xf0, 2, 0x7e, 0xf7])]);
    let source = fixtures::smf(&[system, notes]);
    assert_per_track_stems(&source);
    assert!(
        !channel_events(&midi_split::split_tracks(&source).unwrap()[1].bytes)
            .iter()
            .any(|(_, kind)| matches!(kind, Kind::SysEx { .. }))
    );
    let huge = events(&[(0x0fff_ffff, &[0x90, 60, 96]), (1, &[0x80, 60, 0])]);
    assert!(midi_split::split_tracks(&fixtures::smf(&[huge]))
        .unwrap_err()
        .contains("MIDI_STEM_TIMING_UNREPRESENTABLE"));
    let first = events(&[(0, &[0xff, 0x51, 3, 7, 0xa1, 0x20])]);
    let second = events(&[(0, &[0xff, 0x51, 3, 6, 0x1a, 0x80])]);
    assert!(midi_split::split_tracks(&fixtures::smf(&[first, second]))
        .unwrap_err()
        .contains("MIDI_STEM_GLOBAL_STATE_ORDER_UNRESOLVED"));
}
