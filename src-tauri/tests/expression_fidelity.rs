//! Compact, authored, lyric-bearing MIDI/KAR fixtures for EXP-002.
use std::collections::BTreeSet;
use verse_lib::engine::convert::{convert_midi_with_target, ConvertOutcome};
use verse_lib::engine::midi;
use verse_lib::engine::target::{self, ustx, ExportTarget};

type Events = Vec<(u32, Vec<u8>)>;
fn cc(tick: u32, channel: u8, controller: u8, value: u8) -> (u32, Vec<u8>) {
    (tick, vec![0xb0 | channel, controller, value])
}
fn bend(tick: u32, channel: u8, value: u16) -> (u32, Vec<u8>) {
    (
        tick,
        vec![0xe0 | channel, (value & 127) as u8, (value >> 7) as u8],
    )
}
fn range(channel: u8, semitones: u8) -> Events {
    vec![
        cc(0, channel, 101, 0),
        cc(0, channel, 100, 0),
        cc(0, channel, 6, semitones),
    ]
}
fn voice(onset: u32, duration: u32, channel: u8, key: u8) -> Events {
    vec![
        (onset, vec![0xff, 5, 2, b'l', b'a']),
        // L80 is neutral under EXP003, isolating the original bend/CC protocol assertions.
        (onset, vec![0x90 | channel, key, 80]),
        (onset + duration, vec![0x80 | channel, key, 12]),
    ]
}
fn port(value: u8) -> (u32, Vec<u8>) {
    (0, vec![0xff, 0x21, 1, value])
}
fn vlq(mut value: u32, out: &mut Vec<u8>) {
    let mut bytes = vec![(value & 127) as u8];
    value >>= 7;
    while value > 0 {
        bytes.push((value & 127) as u8 | 128);
        value >>= 7;
    }
    out.extend(bytes.into_iter().rev());
}
fn file(ppq: u16, tracks: Vec<Events>) -> Vec<u8> {
    let mut file = b"MThd\0\0\0\x06\0\x01".to_vec();
    file.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    file.extend_from_slice(&ppq.to_be_bytes());
    for mut events in tracks {
        events.sort_by_key(|(tick, _)| *tick);
        let mut bytes = Vec::new();
        let mut previous = 0;
        for (tick, event) in events {
            vlq(tick - previous, &mut bytes);
            bytes.extend(event);
            previous = tick;
        }
        bytes.extend([0, 0xff, 0x2f, 0]);
        file.extend_from_slice(b"MTrk");
        file.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        file.extend(bytes);
    }
    file
}
fn convert(events: Events) -> (midi::Midi, ConvertOutcome, ustx::UstxProject) {
    let source = midi::parse(&file(480, vec![events])).unwrap();
    let before = source.tracks.clone();
    let result = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(result.ok, "{:?}", result.msg);
    assert_eq!(source.tracks, before, "raw source is immutable");
    let model = ustx::serialize(result.svp.as_ref().unwrap()).unwrap();
    (source, result, model)
}
fn curve<'a>(model: &'a ustx::UstxProject, abbr: &str) -> &'a ustx::UstxCurve {
    model.voice_parts[0]
        .curves
        .iter()
        .find(|c| c.abbr == abbr)
        .unwrap()
}
fn sample(curve: &ustx::UstxCurve, tick: i32) -> i32 {
    match curve.xs.binary_search(&tick) {
        Ok(index) => curve.ys[index],
        Err(index) if index > 0 && index < curve.xs.len() => {
            let t = f64::from(tick - curve.xs[index - 1])
                / f64::from(curve.xs[index] - curve.xs[index - 1]);
            (f64::from(curve.ys[index - 1]) + t * f64::from(curve.ys[index] - curve.ys[index - 1]))
                .round_ties_even() as i32
        }
        _ => 0,
    }
}
fn codes(result: &ConvertOutcome) -> BTreeSet<&str> {
    result
        .tracks
        .iter()
        .flat_map(|t| &t.warnings)
        .map(|d| d.code.as_str())
        .collect()
}

#[test]
fn explicit_pitch_is_held_at_exact_ticks_and_governs_only_its_note_base() {
    let mut events = range(0, 2);
    events.extend([
        bend(0, 0, 8192),
        bend(240, 0, 12288),
        bend(480, 0, 0),
        bend(720, 0, 8192),
    ]);
    events.extend(voice(0, 480, 0, 60));
    events.extend(voice(480, 480, 0, 64));
    let (_, result, model) = convert(events);
    let pitd = curve(&model, "pitd");
    for tick in 0..=960 {
        let expected = match tick {
            0..240 => 0,
            240..480 => 100,
            480..720 => -200,
            _ => 0,
        };
        assert_eq!(sample(pitd, tick), expected, "tick {tick}");
    }
    assert_eq!(
        model.voice_parts[0]
            .notes
            .iter()
            .map(|n| (n.position, n.duration, n.tone))
            .collect::<Vec<_>>(),
        vec![(0, 480, 60), (480, 480, 64)]
    );
    for note in &model.voice_parts[0].notes {
        assert!(!note.pitch.snap_first);
        assert_eq!(note.pitch.data.len(), 1);
        assert_eq!((note.pitch.data[0].x, note.pitch.data[0].y), (0.0, 0.0));
        assert_eq!(note.vibrato.length, 0.0);
    }
    assert!(codes(&result).contains("MIDI_PERFORMANCE_MAPPED"));
    assert!(!result.projection.performance_mapped.is_empty());
    assert!(result.projection.performance_unmapped.is_empty());
    if let Ok(dir) = std::env::var("VERSE_PERFORMANCE_PROBE_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join("pitch.ustx"),
            ustx::to_yaml(&model),
        )
        .unwrap();
    }
}

#[test]
fn sensitivity_changes_and_tempo_changes_do_not_move_source_events() {
    let mut events = range(0, 2);
    events.extend([
        bend(0, 0, 12288),
        cc(480, 0, 6, 12),
        cc(720, 0, 38, 50),
        (480, vec![0xff, 0x51, 3, 0x0f, 0x42, 0x40]),
    ]);
    events.extend(voice(240, 720, 0, 60));
    events.extend(voice(1440, 480, 0, 62));
    let (_, _, model) = convert(events);
    let pitd = curve(&model, "pitd");
    for (tick, value) in [
        (0, 100),
        (239, 100),
        (479, 100),
        (480, 600),
        (719, 600),
        (720, 625),
        (1439, 625),
        (1920, 625),
    ] {
        assert_eq!(sample(pitd, tick), value);
    }
    assert_eq!(
        model
            .tempos
            .iter()
            .map(|t| (t.position, t.bpm))
            .collect::<Vec<_>>(),
        vec![(0, 120.0), (480, 60.0)]
    );
    if let Ok(dir) = std::env::var("VERSE_PERFORMANCE_PROBE_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join("tempo-rest.ustx"),
            ustx::to_yaml(&model),
        )
        .unwrap();
    }
}

#[test]
fn linear_gain_policy_mutes_exactly_then_restores_once_per_part() {
    let mut events = vec![
        cc(0, 0, 7, 127),
        cc(0, 0, 11, 127),
        cc(240, 0, 11, 64),
        cc(480, 0, 11, 0),
        cc(720, 0, 11, 127),
    ];
    events.extend(voice(0, 960, 0, 60));
    let (_, result, model) = convert(events);
    let dyn_curve = curve(&model, "dyn");
    for (tick, value) in [
        (0, 0),
        (239, 0),
        (240, -60),
        (479, -60),
        (480, -240),
        (719, -240),
        (720, 0),
        (960, 0),
    ] {
        assert_eq!(sample(dyn_curve, tick), value);
    }
    assert_eq!(model.voice_parts[0].curves.len(), 1);
    assert!(
        model.voice_parts[0].notes[0].pitch.snap_first,
        "gain does not govern pitch"
    );
    assert!(result.tracks[0]
        .warnings
        .iter()
        .any(|d| d.message.contains("not a universal GM response")));
    assert!(ustx::to_yaml(&model).contains("phoneme_expressions: []"));
    if let Ok(dir) = std::env::var("VERSE_PERFORMANCE_PROBE_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join("gain.ustx"),
            ustx::to_yaml(&model),
        )
        .unwrap();
    }
}

#[test]
fn positive_gain_never_becomes_mute_and_pitch_never_silently_clips() {
    let mut quiet = vec![cc(0, 0, 7, 1), cc(0, 0, 11, 1)];
    quiet.extend(voice(0, 480, 0, 60));
    let (_, result, model) = convert(quiet);
    assert!(model.voice_parts[0].curves.is_empty());
    assert!(codes(&result).contains("MIDI_PERFORMANCE_REPRESENTATION_LIMIT"));
    let mut wide = range(0, 24);
    wide.push(bend(0, 0, 0));
    wide.extend(voice(0, 480, 0, 60));
    let (_, result, model) = convert(wide);
    assert!(model.voice_parts[0].curves.iter().all(|c| c.abbr != "pitd"));
    assert_eq!(
        sample(curve(&model, "dyn"), 0),
        0,
        "neutral explicit attack remains independently mapped"
    );
    assert!(codes(&result).contains("MIDI_PERFORMANCE_REPRESENTATION_LIMIT"));
}

#[test]
fn missing_sensitivity_and_state_hazards_are_visible_without_partial_gain() {
    for hazard in [
        cc(100, 0, 39, 1),
        cc(100, 0, 43, 1),
        cc(100, 0, 121, 0),
        (100, vec![0xf0, 5, 0x7e, 0x7f, 9, 1, 0xf7]),
    ] {
        let mut events = vec![
            cc(0, 0, 7, 64),
            hazard,
            cc(200, 0, 11, 127),
            bend(0, 0, 12288),
        ];
        events.extend(voice(0, 480, 0, 60));
        let (_, result, model) = convert(events);
        assert!(model.voice_parts[0].curves.iter().all(|c| c.abbr != "pitd"));
        let gain = curve(&model, "dyn");
        assert_eq!(sample(gain, 99), -60);
        assert_eq!(sample(gain, 100), 0, "opaque hazard stops editable gain");
        assert_eq!(sample(gain, 300), 0, "later CC cannot clear opaque hazard");
        assert!(codes(&result).contains("MIDI_PERFORMANCE_UNSUPPORTED"));
        assert!(!result.projection.performance_unmapped.is_empty());
    }
}

#[test]
fn short_pulses_are_reported_and_five_tick_pulses_survive_every_origin() {
    for width in [2, 5] {
        let mut events = range(0, 2);
        events.extend([
            bend(0, 0, 8192),
            bend(241, 0, 12288),
            bend(241 + width, 0, 8192),
        ]);
        events.extend(voice(0, 480, 0, 60));
        let (_, result, model) = convert(events);
        if width == 2 {
            assert!(model.voice_parts[0].curves.iter().all(|c| c.abbr != "pitd"));
            assert_eq!(sample(curve(&model, "dyn"), 241), 0);
            assert!(codes(&result).contains("MIDI_PERFORMANCE_REPRESENTATION_LIMIT"));
        } else {
            let pitd = curve(&model, "pitd");
            for origin in -4..=0 {
                assert!(
                    (0..100).any(|i| sample(pitd, origin + 5 * i) == 100),
                    "origin {origin}"
                );
            }
            if let Ok(dir) = std::env::var("VERSE_PERFORMANCE_PROBE_DIR") {
                std::fs::write(
                    std::path::Path::new(&dir).join("pulse.ustx"),
                    ustx::to_yaml(&model),
                )
                .unwrap();
            }
        }
    }
}

#[test]
fn terminal_pulses_need_five_ticks_before_the_exclusive_note_endpoint() {
    for width in [0, 1, 4, 5] {
        for gain in [false, true] {
            let mut events = if gain {
                vec![cc(0, 0, 11, 127)]
            } else {
                let mut events = range(0, 2);
                events.push(bend(0, 0, 8192));
                events
            };
            events.push(if gain {
                cc(480 - width, 0, 11, 0)
            } else {
                bend(480 - width, 0, 12288)
            });
            events.extend(voice(0, 480, 0, 60));
            let (_, result, model) = convert(events);
            if width == 0 {
                let c = curve(&model, if gain { "dyn" } else { "pitd" });
                assert_eq!(sample(c, 479), 0);
                assert_eq!(sample(c, 480), 0);
                assert!(!codes(&result).contains("MIDI_PERFORMANCE_REPRESENTATION_LIMIT"));
                assert!(result.projection.performance_spans.iter().any(|span| span
                    .target_track
                    .is_none()
                    && span.note_ids.is_empty()
                    && span.start_tick == 480));
            } else if width < 5 {
                let abbr = if gain { "dyn" } else { "pitd" };
                if let Some(c) = model.voice_parts[0].curves.iter().find(|c| c.abbr == abbr) {
                    assert!(
                        (0..=480).all(|t| sample(c, t) == 0),
                        "short pulse must not claim an active mapped pulse"
                    );
                }
                assert!(codes(&result).contains("MIDI_PERFORMANCE_REPRESENTATION_LIMIT"));
                assert!(result.projection.performance_spans.iter().any(|s| s.status
                    == verse_lib::engine::performance::TransferStatus::RepresentationLimit));
                assert!(
                    (0..5).any(|origin| !(0..96).any(|i| {
                        let tick = origin + 5 * i;
                        tick >= 480 - width && tick < 480
                    })),
                    "a terminal pulse can disappear entirely before note end"
                );
            } else {
                let c = curve(&model, if gain { "dyn" } else { "pitd" });
                for origin in 0..5 {
                    assert!(
                        (0..96).any(|i| sample(c, origin + 5 * i) == if gain { -240 } else { 100 })
                    );
                }
            }
        }
    }
}

#[test]
fn touching_different_owners_report_pitch_boundary_limit_but_keep_gain_scoped() {
    let mut events = range(0, 2);
    events.extend([bend(0, 0, 12288), cc(0, 0, 11, 64)]);
    events.extend(voice(0, 480, 0, 60));
    events.extend(voice(480, 480, 1, 64));
    let (_, result, model) = convert(events);
    assert_eq!(model.voice_parts.len(), 1);
    assert!(model.voice_parts[0].curves.iter().all(|c| c.abbr != "pitd"));
    assert_eq!(sample(curve(&model, "dyn"), 479), -60);
    assert_eq!(sample(curve(&model, "dyn"), 480), 0);
    assert_eq!(sample(curve(&model, "dyn"), 960), 0);
    assert!(codes(&result).contains("MIDI_PERFORMANCE_REPRESENTATION_LIMIT"));
    assert!(model.voice_parts[0]
        .notes
        .iter()
        .all(|n| n.pitch.snap_first));
}

#[test]
fn fractional_target_ticks_refuse_at_analysis_and_write_boundaries() {
    let mut events = range(0, 2);
    events.push(bend(1, 0, 12288));
    events.extend(voice(0, 960, 0, 60));
    let source = midi::parse(&file(960, vec![events])).unwrap();
    let result = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(!result.ok);
    let message = result.msg.unwrap();
    assert!(
        message.contains("MIDI tick 1")
            && message.contains("PPQ 960")
            && message.contains("event:"),
        "{message}"
    );
    let svp = convert_midi_with_target(&source, "english", None, ExportTarget::Svp);
    assert!(svp.ok);
    assert!(codes(&svp).contains("MIDI_PERFORMANCE_UNSUPPORTED"));
}

#[test]
fn port_channel_and_polyphonic_ownership_survive_karaoke_projection() {
    let mut controls = vec![port(3)];
    controls.extend(range(0, 2));
    controls.push(bend(0, 0, 12288));
    let melody = vec![
        port(3),
        (240, vec![0x90, 60, 73]),
        (240, vec![0x90, 64, 73]),
        (720, vec![0x80, 60, 12]),
        (720, vec![0x80, 64, 12]),
    ];
    let lyrics = vec![(240, vec![0xff, 5, 2, b'l', b'a'])];
    let mut other_port = vec![port(4)];
    other_port.extend(voice(240, 480, 0, 67));
    let mut other_channel = vec![port(3)];
    other_channel.extend(voice(240, 480, 1, 69));
    let source = midi::parse_with_karaoke_profile(&file(
        480,
        vec![controls, melody, lyrics, other_port, other_channel],
    ))
    .unwrap();
    let result = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(result.ok, "{:?}", result.msg);
    let model = ustx::serialize(result.svp.as_ref().unwrap()).unwrap();
    assert_eq!(model.voice_parts.len(), 4);
    assert_eq!(
        model
            .voice_parts
            .iter()
            .filter(|part| part.curves.iter().any(|c| c.abbr == "pitd"))
            .count(),
        2
    );
    for part in &model.voice_parts {
        if part.notes[0].tone == 60 || part.notes[0].tone == 64 {
            assert_eq!(sample(&part.curves[0], 240), 100);
            assert!(!part.notes[0].pitch.snap_first);
        } else {
            assert!(part.curves.iter().all(|c| c.abbr != "pitd"));
            assert!(
                part.curves.iter().any(|c| c.abbr == "dyn"),
                "explicit native attack remains owned by this port/channel"
            );
            assert!(part.notes[0].pitch.snap_first);
        }
    }
}

#[test]
fn conflicting_physical_tracks_do_not_choose_a_pitch_or_gain_winner() {
    let mut first = range(0, 2);
    first.push(cc(0, 0, 7, 64));
    let mut second = vec![bend(0, 0, 12288), cc(0, 0, 7, 127)];
    second.extend(voice(0, 480, 0, 60));
    let source = midi::parse(&file(480, vec![first, second])).unwrap();
    let result = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(result.ok);
    assert!(ustx::serialize(result.svp.as_ref().unwrap())
        .unwrap()
        .voice_parts[0]
        .curves
        .is_empty());
    assert!(codes(&result).contains("MIDI_PERFORMANCE_UNSUPPORTED"));
}

#[test]
fn saved_ledger_distinguishes_mapped_curves_from_raw_retention_and_svp() {
    use verse_lib::bundle::{build_preservation_ledger, BundleLayout, PrimaryDisposition};
    use verse_lib::stems::StemPlan;
    let mut events = range(0, 2);
    events.push(bend(0, 0, 12288));
    events.extend(voice(0, 480, 0, 60));
    let source = midi::parse(&file(480, vec![events])).unwrap();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let result = convert_midi_with_target(&source, "english", None, target);
        assert!(result.ok);
        let layout = BundleLayout::new(
            std::path::Path::new("/tmp/Expression.versebundle"),
            "source.mid",
            target,
        )
        .unwrap();
        let stems = StemPlan::from_source(&source, &result.tracks).unwrap();
        let ledger = build_preservation_ledger(&source, &result.projection, &layout, &stems);
        let bend = ledger
            .entries
            .iter()
            .find(|entry| entry.source_id == "event:midi-track-0:3")
            .unwrap();
        assert!(bend
            .artifact_paths
            .iter()
            .any(|p| p.ends_with("source.mid")));
        if target == ExportTarget::Ustx {
            assert!(matches!(
                bend.disposition,
                PrimaryDisposition::ProjectedMapped { .. }
            ));
            assert!(bend.artifact_paths.iter().any(|p| p.ends_with(".ustx")));
            assert!(serde_json::to_string(&ledger)
                .unwrap()
                .contains("projectedMapped"));
        } else {
            assert!(matches!(
                bend.disposition,
                PrimaryDisposition::SourceOnly { .. }
            ));
            assert!(!bend.artifact_paths.iter().any(|p| p.ends_with(".svp")));
            assert!(codes(&result).contains("MIDI_PERFORMANCE_UNSUPPORTED"));
            let svp = target::svp::serialize(result.svp.as_ref().unwrap()).unwrap();
            assert!(svp.tracks[0]
                .main_group
                .parameters
                .pitch_delta
                .points
                .is_empty());
        }
    }
}

#[test]
fn unsupported_pressure_keeps_pitch_vibrato_and_attack_yaml_bytes() {
    let (_, _, plain) = convert(voice(0, 480, 0, 60));
    let mut with_pressure = voice(0, 480, 0, 60);
    with_pressure.push((10, vec![0xd0, 64]));
    let (_, reported, pressure) = convert(with_pressure);
    assert_eq!(ustx::to_yaml(&plain), ustx::to_yaml(&pressure));
    assert_eq!(
        plain.voice_parts[0].notes[0].pitch,
        ustx::UstxPitch::default()
    );
    assert_eq!(
        plain.voice_parts[0].notes[0].vibrato,
        ustx::UstxVibrato::default()
    );
    assert!(codes(&reported).contains("MIDI_PERFORMANCE_UNSUPPORTED"));
    if let Ok(dir) = std::env::var("VERSE_PERFORMANCE_PROBE_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join("default.ustx"),
            ustx::to_yaml(&plain),
        )
        .unwrap();
    }
}

// The emitter's curve block is a strict YAML sequence with JSON-compatible
// quoted scalars and integer arrays. Parse those actual bytes, independently
// of the Rust model, so replacing emitted curves with [] fails ordinary tests.
fn serialized_curves(
    project: &verse_lib::engine::projection::ProjectedProject,
) -> Vec<ustx::UstxCurve> {
    let bytes = target::serialize_to(ExportTarget::Ustx, project).unwrap();
    let yaml = String::from_utf8(bytes).unwrap();
    let lines: Vec<_> = yaml.lines().collect();
    let mut curves = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(abbr) = line.strip_prefix("      - abbr: ") {
            curves.push(ustx::UstxCurve {
                abbr: serde_json::from_str(abbr).unwrap(),
                xs: serde_json::from_str(lines[index + 1].strip_prefix("        xs: ").unwrap())
                    .unwrap(),
                ys: serde_json::from_str(lines[index + 2].strip_prefix("        ys: ").unwrap())
                    .unwrap(),
            });
        }
    }
    curves
}

#[test]
fn ordinary_write_boundary_emits_active_yaml_pitch_and_mute_restore() {
    let mut events = range(0, 2);
    events.extend([
        bend(0, 0, 8192),
        bend(240, 0, 12288),
        bend(480, 0, 0),
        bend(720, 0, 8192),
        cc(0, 0, 11, 127),
        cc(240, 0, 11, 64),
        cc(480, 0, 11, 0),
        cc(720, 0, 11, 127),
    ]);
    events.extend(voice(0, 960, 0, 60));
    let (_, result, _) = convert(events);
    let curves = serialized_curves(result.svp.as_ref().unwrap());
    assert_eq!(curves.len(), 2, "serialized curves cannot be empty");
    for (abbr, expected) in [("pitd", [0, 100, -200, 0]), ("dyn", [0, -60, -240, 0])] {
        let c = curves.iter().find(|c| c.abbr == abbr).unwrap();
        for (i, tick) in [0, 240, 480, 720].into_iter().enumerate() {
            assert_eq!(sample(c, tick), expected[i]);
            assert_eq!(sample(c, tick + 239), expected[i]);
        }
    }
}

#[test]
fn parameter_selection_does_not_write_gain_and_nrpn_entry_blocks_both_dimensions() {
    for entry in [None, Some(6), Some(38), Some(96), Some(97)] {
        let mut events = range(0, 2);
        events.extend([
            bend(0, 0, 12288),
            cc(0, 0, 7, 64),
            cc(100, 0, 99, 1),
            cc(100, 0, 98, 2),
        ]);
        if let Some(controller) = entry {
            events.push(cc(200, 0, controller, 64));
        }
        events.extend([cc(300, 0, 7, 127), cc(300, 0, 11, 127), bend(300, 0, 8192)]);
        events.extend(voice(0, 480, 0, 60));
        let (_, result, model) = convert(events);
        assert_eq!(sample(curve(&model, "dyn"), 199), -60);
        assert_eq!(sample(curve(&model, "pitd"), 199), 100);
        if entry.is_some() {
            assert_eq!(sample(curve(&model, "dyn"), 350), 0);
            assert!(result
                .projection
                .performance_spans
                .iter()
                .any(|r| r.start_tick == 200
                    && r.end_tick == 300
                    && r.status == verse_lib::engine::performance::TransferStatus::Unsupported));
            assert!(!result
                .projection
                .performance_spans
                .iter()
                .any(|r| r.start_tick >= 200
                    && r.status == verse_lib::engine::performance::TransferStatus::Mapped));
        } else {
            assert_eq!(sample(curve(&model, "dyn"), 300), 0);
            assert!(!result
                .projection
                .performance_spans
                .iter()
                .any(|r| r.target_track.is_some()
                    && r.status == verse_lib::engine::performance::TransferStatus::Unsupported));
        }
    }
}

#[test]
fn mpe_requires_applicable_rpn_and_an_actual_data_entry() {
    for (msb, entry, blocked) in [
        (None, true, false),
        (Some(1), true, false),
        (Some(0), false, false),
        (Some(0), true, true),
    ] {
        let mut master = Vec::new();
        if let Some(msb) = msb {
            master.push(cc(0, 0, 101, msb));
        }
        master.push(cc(0, 0, 100, 6));
        if entry {
            master.push(cc(100, 0, 6, 4));
        }
        let mut member = range(1, 2);
        member.push(bend(0, 1, 12288));
        member.extend(voice(200, 480, 1, 60));
        let source = midi::parse(&file(480, vec![master, member])).unwrap();
        let result = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
        assert!(result.ok, "{:?}", result.msg);
        let model = ustx::serialize(result.svp.as_ref().unwrap()).unwrap();
        assert_eq!(
            model.voice_parts[0].curves.iter().all(|c| c.abbr != "pitd"),
            blocked,
            "msb={msb:?} entry={entry}"
        );
    }
}

#[test]
fn explicit_state_recovers_gain_and_pitch_conflicts_only_after_resolution() {
    let mut first = range(0, 2);
    first.push(cc(0, 0, 7, 64));
    let mut second = vec![
        bend(0, 0, 12288),
        cc(0, 0, 7, 100),
        cc(100, 0, 7, 127),
        cc(120, 0, 11, 64),
    ];
    second.extend([
        cc(140, 0, 101, 0),
        cc(140, 0, 100, 0),
        cc(140, 0, 6, 2),
        bend(160, 0, 12288),
    ]);
    second.extend(voice(0, 480, 0, 60));
    let source = midi::parse(&file(480, vec![first, second])).unwrap();
    let result = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(result.ok, "{:?}", result.msg);
    let model = ustx::serialize(result.svp.as_ref().unwrap()).unwrap();
    assert_eq!(sample(curve(&model, "dyn"), 119), 0);
    assert_eq!(sample(curve(&model, "dyn"), 120), -60);
    assert_eq!(sample(curve(&model, "pitd"), 159), 0);
    assert_eq!(sample(curve(&model, "pitd"), 160), 100);
    use verse_lib::engine::performance::{Dimension, TransferStatus};
    assert!(result
        .projection
        .performance_spans
        .iter()
        .any(|r| r.dimension == Dimension::LinearGain
            && r.start_tick == 100
            && r.end_tick == 120
            && r.status == TransferStatus::Mapped));
    assert!(result
        .projection
        .performance_spans
        .iter()
        .any(|r| r.dimension == Dimension::PitchCents
            && r.start_tick == 160
            && r.end_tick == 480
            && r.status == TransferStatus::Mapped));
    assert!(result
        .projection
        .performance_spans
        .iter()
        .filter(|r| r.status != TransferStatus::Mapped && r.target_track.is_some())
        .all(|r| r.end_tick <= 160));
}

#[test]
fn short_rest_boundary_uses_milliseconds_and_the_tempo_change() {
    for (tempo, limited) in [(1_000_000u32, false), (500_000, true), (250_000, true)] {
        let mut events = range(0, 2);
        events.push(bend(0, 0, 12288));
        events.push((
            480,
            vec![
                0xff,
                0x51,
                3,
                (tempo >> 16) as u8,
                (tempo >> 8) as u8,
                tempo as u8,
            ],
        ));
        events.extend(voice(0, 480, 0, 60));
        events.extend(voice(500, 480, 1, 64));
        let (_, result, model) = convert(events);
        assert_eq!(
            codes(&result).contains("MIDI_PERFORMANCE_REPRESENTATION_LIMIT"),
            limited
        );
        assert_eq!(
            model.voice_parts[0].curves.iter().all(|c| c.abbr != "pitd"),
            limited
        );
        assert_eq!(model.voice_parts[0].notes[1].position, 500);
        assert!(model.voice_parts[0].notes[1].pitch.snap_first);
    }
}

#[test]
fn past_issues_and_future_svp_events_do_not_name_unaffected_notes() {
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        let mut events = vec![(0, vec![0xd0, 64]), cc(1200, 0, 11, 0)];
        events.extend(voice(240, 480, 0, 60));
        let source = midi::parse(&file(480, vec![events])).unwrap();
        let result = convert_midi_with_target(&source, "english", None, target);
        assert!(result.ok);
        for track in &source.tracks {
            for event in &track.events {
                if matches!(
                    event.kind,
                    midi::Kind::ChannelPressure { .. } | midi::Kind::ControlChange { .. }
                ) {
                    let id = format!("event:{}:{}", track.id, event.order);
                    assert!(!result.projection.performance_mapped.contains_key(&id));
                    for index in &result.projection.performance_refs[&id] {
                        let span = &result.projection.performance_spans[*index];
                        assert!(span.note_ids.is_empty() && span.target_track.is_none());
                    }
                }
            }
        }
    }
}

#[test]
fn untexted_and_controller_only_sources_have_no_invented_editable_owner() {
    for notes in [false, true] {
        let mut events = vec![cc(0, 0, 7, 64), cc(40, 0, 99, 1), cc(50, 0, 6, 1)];
        if notes {
            events.extend([(0, vec![0x90, 60, 73]), (480, vec![0x80, 60, 0])]);
        }
        let source = midi::parse(&file(480, vec![events])).unwrap();
        let result = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
        assert!(result.ok);
        assert!(result.svp.as_ref().unwrap().tracks.is_empty());
        assert!(result.projection.performance_mapped.is_empty());
        assert!(codes(&result).contains("MIDI_PERFORMANCE_UNSUPPORTED"));
        assert!(result
            .projection
            .performance_spans
            .iter()
            .all(|r| r.note_ids.is_empty() && r.target_track.is_none()));
    }
}

#[test]
fn mixed_success_ledger_bytes_keep_versioned_lane_note_and_span_references() {
    use verse_lib::bundle::{build_preservation_ledger, BundleLayout, PreservationLedger};
    use verse_lib::stems::StemPlan;
    let controls = vec![cc(0, 0, 11, 127), cc(236, 0, 11, 64)];
    let melody = vec![
        (0, vec![0x90, 60, 73]),
        (0, vec![0x90, 64, 73]),
        (240, vec![0x80, 64, 0]),
        (480, vec![0x80, 60, 0]),
    ];
    let lyrics = vec![(0, vec![0xff, 5, 2, b'l', b'a'])];
    let source =
        midi::parse_with_karaoke_profile(&file(480, vec![controls, melody, lyrics])).unwrap();
    let result = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
    assert!(result.ok, "{:?}", result.msg);
    assert_eq!(result.svp.as_ref().unwrap().tracks.len(), 2);
    let layout = BundleLayout::new(
        std::path::Path::new("/tmp/Mixed.versebundle"),
        "source.kar",
        ExportTarget::Ustx,
    )
    .unwrap();
    let stems = StemPlan::from_source(&source, &result.tracks).unwrap();
    let ledger = build_preservation_ledger(&source, &result.projection, &layout, &stems);
    let bytes = serde_json::to_vec(&ledger).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["schemaVersion"], 3);
    let entry = json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["sourceId"] == "event:midi-track-0:1")
        .unwrap();
    assert_eq!(entry["disposition"]["kind"], "projectedMapped");
    assert!(entry["disposition"]["policy"]
        .as_str()
        .unwrap()
        .contains("linear"));
    assert!(entry["disposition"]["limitations"]
        .as_str()
        .unwrap()
        .contains("5-tick"));
    let refs = entry["performanceRefs"].as_array().unwrap();
    let spans: Vec<_> = refs
        .iter()
        .map(|i| &json["performanceSpans"][i.as_u64().unwrap() as usize])
        .collect();
    assert!(spans
        .iter()
        .any(|s| s["status"] == "mapped" && s["endTick"] == 480));
    assert!(spans
        .iter()
        .any(|s| s["status"] == "representationLimit" && s["endTick"] == 240));
    assert_eq!(
        spans
            .iter()
            .map(|s| s["targetTrack"].as_u64().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
    assert!(spans
        .iter()
        .all(|s| s["noteIds"].as_array().unwrap().len() == 1));
    let decoded: PreservationLedger = serde_json::from_slice(&bytes).unwrap();
    let allowed = decoded
        .entries
        .iter()
        .flat_map(|e| e.artifact_paths.iter().cloned())
        .collect();
    decoded.validate(&allowed).unwrap();
    let mut wrong_version = decoded;
    wrong_version.schema_version = 2;
    assert!(wrong_version.validate(&allowed).is_err());
    let old = r#"{"schemaVersion":2,"expectedSourceIds":["event:old"],"entries":[{"sourceId":"event:old","itemKind":"event","disposition":{"kind":"sourceOnly","reason":"raw"},"artifactPaths":["source.mid"]}]}"#;
    let old: PreservationLedger = serde_json::from_str(old).unwrap();
    old.validate(&BTreeSet::from(["source.mid".into()]))
        .unwrap();
    let old_json = serde_json::to_string(&old).unwrap();
    assert!(!old_json.contains("performanceSpans") && !old_json.contains("performanceRefs"));
}

#[test]
fn adapter_refuses_overflow_and_bounds_alternating_ownership_work() {
    use std::sync::Arc;
    use verse_lib::engine::performance::{
        ChannelKey, ChannelPerformance, PerformanceNote, PerformanceOwner,
    };
    let (_, result, _) = convert(voice(0, 480, 0, 60));
    let mut project = result.svp.unwrap();
    let mut note = project.tracks[0].notes[0].clone();
    note.onset_ticks = u32::MAX - 1;
    note.duration_ticks = 4;
    note.performance = Some(PerformanceNote {
        source_id: "overflow".into(),
        intensity: None,
        owner: PerformanceOwner::Midi {
            key: ChannelKey {
                port: 0,
                channel: 0,
            },
            timeline: Arc::new(ChannelPerformance::default()),
        },
    });
    project.tracks[0].notes = vec![note.clone()];
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        assert!(target::performance_report(target, &project)
            .unwrap_err()
            .contains("overflows"));
    }
    project.tracks[0].notes = (0..16_000)
        .map(|i| {
            let mut n = note.clone();
            n.onset_ticks = i * 10;
            n.duration_ticks = 5;
            if let PerformanceOwner::Midi { key, .. } = &mut n.performance.as_mut().unwrap().owner {
                key.channel = (i % 2) as u8;
            }
            n
        })
        .collect();
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let error = target::performance_report(target, &project).unwrap_err();
        assert!(error.contains("MIDI_PERFORMANCE_LIMIT"), "{error}");
    }
}

#[test]
fn repeated_issues_share_note_references_instead_of_a_note_times_event_product() {
    let mut events = Vec::new();
    for i in 0..2000 {
        events.extend(voice(i * 10, 10, 0, 60));
        events.push((i * 10 + 1, vec![0xd0, 64]));
    }
    let (_, result, _) = convert(events);
    let pressure: Vec<_> = result
        .projection
        .performance_spans
        .iter()
        .filter(|s| s.dimension == verse_lib::engine::performance::Dimension::Other)
        .collect();
    assert_eq!(pressure.len(), 1);
    assert_eq!(pressure[0].note_ids.len(), 2000);
    assert_eq!(result.projection.performance_refs.values().map(Vec::len).sum::<usize>(),4000,
        "one reference per pressure event plus one per newly interpreted attack; no note-times-event growth");
}

#[test]
fn adjacent_known_gain_owners_keep_the_old_guard_before_the_boundary() {
    let mut events = vec![cc(0, 0, 11, 64), cc(0, 1, 11, 32)];
    events.extend(voice(0, 480, 0, 60));
    events.extend(voice(480, 480, 1, 64));
    let (_, result, model) = convert(events);
    let gain = curve(&model, "dyn");
    assert_eq!(sample(gain, 479), -60);
    assert_eq!(sample(gain, 480), -120);
    assert!(result
        .projection
        .performance_spans
        .iter()
        .filter(|r| r.target_track.is_some())
        .all(|r| r.note_ids.len() == 1));
}

#[test]
fn later_controller_in_the_same_owner_run_never_names_an_earlier_note() {
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let mut events = vec![cc(600, 0, 11, 64)];
        events.extend(voice(0, 480, 0, 60));
        events.extend(voice(600, 480, 0, 64));
        let source = midi::parse(&file(480, vec![events])).unwrap();
        let result = convert_midi_with_target(&source, "english", None, target);
        assert!(result.ok);
        let controller = source
            .tracks
            .iter()
            .find_map(|track| {
                track.events.iter().find_map(|event| {
                    matches!(event.kind, midi::Kind::ControlChange { controller: 11, .. })
                        .then(|| format!("event:{}:{}", track.id, event.order))
                })
            })
            .unwrap();
        let spans: Vec<_> = result.projection.performance_refs[&controller]
            .iter()
            .map(|i| &result.projection.performance_spans[*i])
            .filter(|s| s.target_track.is_some())
            .collect();
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].start_tick, spans[0].end_tick), (600, 1080));
        assert_eq!(spans[0].note_ids.len(), 1);
    }
}

#[test]
fn explicit_native_velocity_has_one_owned_gain_contributor() {
    for (velocity, controller, expected) in [
        (73, None, -18),
        (100, None, 50),
        (80, Some(64), -60),
        (100, Some(64), -10),
    ] {
        let mut events = voice(0, 480, 0, 60);
        for (_, bytes) in &mut events {
            if bytes[0] & 0xf0 == 0x90 {
                bytes[2] = velocity;
            }
        }
        if let Some(value) = controller {
            events.push(cc(0, 0, 7, value));
        }
        let (source, result, model) = convert(events);
        assert_eq!(sample(curve(&model, "dyn"), 0), expected);
        let project = result.svp.as_ref().unwrap();
        let note = &project.tracks[0].notes[0];
        let binding = note.performance.as_ref().unwrap();
        assert_eq!(
            binding.source_id,
            note.source_evidence.as_ref().unwrap().note_id
        );
        let attack = source
            .tracks
            .iter()
            .find_map(|t| {
                t.events.iter().find_map(|e| {
                    matches!(&e.kind,midi::Kind::NoteOn(n) if n.velocity==Some(velocity))
                        .then(|| verse_lib::engine::performance::attack_field_id(&t.id, e.order))
                })
            })
            .unwrap();
        assert!(result.projection.performance_mapped.contains_key(&attack));
        let provenance = binding
            .intensity
            .as_ref()
            .unwrap()
            .provenance
            .as_ref()
            .unwrap();
        assert!(provenance
            .evidence
            .iter()
            .any(|e| e.source_ids.contains(&attack)));
        assert!(matches!(
            provenance.scope,
            verse_lib::engine::score_intensity::Scope::Midi {
                port: 0,
                channel: 0
            }
        ));
    }
}
