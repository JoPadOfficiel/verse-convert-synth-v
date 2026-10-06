//! Authored native 3/4 regressions: structural fidelity, not native-host/audio proof.
//! Checks retained source XML and BundleInput bytes; does not render/publish a bundle.
use std::{collections::BTreeSet, path::Path};
use verse_lib::{
    bundle::{
        build_preservation_ledger, BundleInput, BundleLayout, BundleProject, PrimaryDisposition,
        SourceItemKind,
    },
    engine::{
        convert::convert_midi_with_profile,
        midi::{Kind, LyricState, Midi},
        musescore,
        projection::{ProjectedLyric, ProjectedProject},
        target::{self, ExportTarget, PronunciationProfile},
    },
    stems::StemPlan,
};

fn score(version: &str, bodies: &[String]) -> String {
    let (program, concert, size) = if version.starts_with('3') {
        ("3.6.2", 0, 1.0)
    } else {
        ("4.7.5", 1, 1.5)
    };
    let parts: String = bodies.iter().enumerate().map(|(i, _)| format!(
        r#"<Part><trackName>Voice {i}</trackName><Staff id="{}"/><Instrument id="voice{i}"><instrumentId>voice.soprano</instrumentId><transposeChromatic>-12</transposeChromatic><transposeDiatonic>-7</transposeDiatonic></Instrument></Part>"#, i + 1)).collect();
    let staves: String = bodies
        .iter()
        .enumerate()
        .map(|(i, body)| format!(r#"<Staff id="{}">{body}</Staff>"#, i + 1))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><museScore version="{version}"><programVersion>{program}</programVersion><Score><Division>480</Division><Style><concertPitch>{concert}</concertPitch><spatium>{size}</spatium></Style>{parts}{staves}</Score></museScore>"#
    )
}

fn chord(duration: &str, pitch: u8, lyric: &str) -> String {
    format!("<Chord><durationType>{duration}</durationType>{lyric}<Note><pitch>{pitch}</pitch></Note></Chord>")
}

const WORD: &str = "<Lyrics><text>one</text></Lyrics>";
const TIE_START: &str = r#"<Spanner type="Tie"><Tie/><next><location><fractions>1/8</fractions></location></next></Spanner>"#;
const TIE_STOP: &str = r#"<Spanner type="Tie"><prev><location><fractions>-1/8</fractions></location></prev></Spanner>"#;

fn paired(version: &str) -> String {
    // Explicit performance fields are identical; only source version/display differs.
    let head = chord("eighth", 60, WORD).replace("</Note>", &format!("{TIE_START}</Note>"));
    let tail = chord("eighth", 60, "").replace("</Note>", &format!("{TIE_STOP}</Note>"));
    let first = format!(
        r#"
      <Measure len="3/4"><startRepeat/><voice>
        <TimeSig><sigN>3</sigN><sigD>4</sigD></TimeSig><Tempo><tempo>2</tempo></Tempo>
        <Dynamic><subtype>p</subtype><velocity>49</velocity></Dynamic>
        <Chord><durationType>quarter</durationType><dots>1</dots><Lyrics><text>hold</text><ticks>720</ticks><ticks_f>3/8</ticks_f></Lyrics>
          <Note><pitch>60</pitch><color>#ff123456</color></Note><Note><pitch>64</pitch></Note></Chord>
        <Chord><durationType>eighth</durationType><Note><pitch>62</pitch></Note><Note><pitch>66</pitch></Note></Chord>
        {}</voice><voice>{}</voice></Measure>
      <Measure len="1/4"><voice><TimeSig><sigN>1</sigN><sigD>4</sigD></TimeSig><Tempo><tempo>1.5</tempo></Tempo>
        <Dynamic><subtype>f</subtype><velocity>96</velocity></Dynamic>{head}{tail}</voice><endRepeat>2</endRepeat></Measure>"#,
        chord("quarter", 67, "<Lyrics><text>end</text></Lyrics>"),
        chord("half", 55, "<Lyrics><text>low</text></Lyrics>")
            .replace("</durationType>", "</durationType><dots>1</dots>")
    );
    let second = format!(
        r#"<Measure len="3/4"><voice>{}<Rest><durationType>half</durationType></Rest></voice></Measure>
      <Measure len="1/4"><voice>{}</voice></Measure>"#,
        chord("quarter", 48, "<Lyrics><text>bass</text></Lyrics>"),
        chord("quarter", 50, "<Lyrics><text>end</text></Lyrics>")
    );
    score(version, &[first, second])
}

fn geometry(midi: &Midi) -> Vec<(u32, u32, u8)> {
    let mut rows = Vec::new();
    for track in &midi.tracks {
        for event in &track.events {
            if let Kind::NoteOn(note) = &event.kind {
                if let Some(pitch) = note.key {
                    let off = track.events.iter().filter(|e| e.tick >= event.tick).find(|e| matches!(&e.kind, Kind::NoteOff(n) if n.source_id.as_deref() == Some(note.source.id.as_str()))).unwrap();
                    rows.push((event.tick, off.tick - event.tick, pitch));
                }
            }
        }
    }
    rows.sort_unstable();
    rows
}

fn tempos(midi: &Midi) -> Vec<(u32, u32)> {
    midi.tracks
        .iter()
        .flat_map(|t| &t.events)
        .filter_map(|e| match e.kind {
            Kind::Tempo(us) => Some((e.tick, us)),
            _ => None,
        })
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_iter()
        .collect()
}

fn projection(midi: &Midi, export: ExportTarget) -> ProjectedProject {
    let result =
        convert_midi_with_profile(midi, "english", None, export, PronunciationProfile::Default);
    assert!(result.ok, "{export:?}: {:?}", result.msg);
    result.svp.unwrap()
}

fn assert_targets(
    midi: &Midi,
    expected: &[(u32, u32, u8)],
    expected_tempos: &[(u32, f64)],
    holds: usize,
) {
    for export in [ExportTarget::Svp, ExportTarget::Ustx] {
        let project = projection(midi, export);
        target::validate_for(export, &project).unwrap();
        assert_eq!(project.monophony_violation(), None);
        for note in project.tracks.iter().flat_map(|t| &t.notes) {
            let evidence = note.source_evidence.as_ref().unwrap();
            let origin = evidence.origin.as_ref().unwrap();
            let track = midi
                .tracks
                .iter()
                .find(|t| t.id == origin.track_id)
                .unwrap();
            let event = track
                .events
                .iter()
                .find(|e| e.order == origin.note_on_order)
                .unwrap();
            let Kind::NoteOn(source) = &event.kind else {
                panic!("source-owned note-on")
            };
            assert_eq!(origin.source, source.source);
            assert_eq!(note.onset_ticks, event.tick);
            assert_eq!(Some(note.pitch), source.key);
            let off = track
                .events
                .iter()
                .find(|e| e.order == origin.note_off_order)
                .unwrap();
            assert_eq!(note.duration_ticks, off.tick - event.tick);
            assert_eq!(origin.source.part_id, track.source.part_id);
            assert_eq!(origin.source.staff_id, track.source.staff_id);
            assert_eq!(origin.source.voice, track.source.voice);
            if let Some(id) = &evidence.lyric_id {
                // NoteEvidence uses the ledger's occurrence/event-qualified ID.
                let original = source
                    .lyrics
                    .iter()
                    .find(|lyric| {
                        *id == format!(
                            "lyric:{}:occurrence:{}:note-event:{}",
                            lyric.id, source.source.occurrence, origin.note_on_order
                        )
                    })
                    .expect("lyric instance belongs to this exact source occurrence");
                if let ProjectedLyric::Source(lyric) = &note.lyric {
                    assert_eq!(
                        lyric.as_ref(),
                        original,
                        "original lyric fields stay source-owned"
                    );
                }
            }
        }
        let mut notes = Vec::new();
        let mut lane_notes = Vec::new();
        let map: Vec<(i64, f64)> = match export {
            ExportTarget::Svp => {
                let model = target::svp::serialize(&project).unwrap();
                assert_eq!(model.time.meter.len(), project.meters.len());
                for (actual, source) in model.time.meter.iter().zip(&project.meters) {
                    assert_eq!(
                        (actual.index, actual.numerator, actual.denominator),
                        (source.bar_index, source.numerator, source.denominator)
                    );
                }
                for (lane, track) in model.tracks.into_iter().enumerate() {
                    assert_eq!(track.name, project.tracks[lane].name);
                    for n in track.main_group.notes {
                        lane_notes.push((lane, n.onset, n.duration, n.pitch, n.lyrics.clone()));
                        notes.push((n.onset, n.duration, n.pitch, n.lyrics));
                    }
                }
                model
                    .time
                    .tempo
                    .into_iter()
                    .map(|t| (t.position, t.bpm))
                    .collect()
            }
            ExportTarget::Ustx => {
                let model = target::ustx::serialize(&project).unwrap();
                assert_eq!(model.time_signatures.len(), project.meters.len());
                for (actual, source) in model.time_signatures.iter().zip(&project.meters) {
                    assert_eq!(
                        (
                            actual.bar_position as u32,
                            actual.beat_per_bar,
                            actual.beat_unit
                        ),
                        (source.bar_index, source.numerator, source.denominator)
                    );
                }
                for part in model.voice_parts {
                    let lane = usize::try_from(part.track_no).unwrap();
                    assert_eq!(model.tracks[lane].track_name, project.tracks[lane].name);
                    for n in part.notes {
                        lane_notes.push((
                            lane,
                            i64::from(part.position + n.position),
                            i64::from(n.duration),
                            n.tone,
                            n.lyric.clone(),
                        ));
                        notes.push((
                            i64::from(part.position + n.position),
                            i64::from(n.duration),
                            n.tone,
                            n.lyric,
                        ));
                    }
                }
                model
                    .tempos
                    .into_iter()
                    .map(|t| (i64::from(t.position), t.bpm))
                    .collect()
            }
        };
        let grid = if export == ExportTarget::Svp {
            705_600_000
        } else {
            480
        };
        let scale = |tick: u32| i64::from(tick) * grid / i64::from(midi.ticks_per_beat);
        notes.sort();
        assert_eq!(
            notes.iter().map(|n| (n.0, n.1, n.2)).collect::<Vec<_>>(),
            expected
                .iter()
                .map(|&(on, dur, pitch)| (scale(on), scale(dur), pitch))
                .collect::<Vec<_>>()
        );
        let marker = if export == ExportTarget::Svp {
            "-"
        } else {
            "+~"
        };
        let mut expected_lanes: Vec<_> = project
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(lane, track)| {
                track.notes.iter().map(move |note| {
                    // These authored Default-profile fixtures use complete source words
                    // and proven holds. Unexpected lyric states must fail this oracle.
                    let text = match &note.lyric {
                        ProjectedLyric::Source(lyric) => match &lyric.state {
                            LyricState::Text(text) => text.clone(),
                            LyricState::Continuation => marker.into(),
                            other => panic!("unexpected authored lyric state: {other:?}"),
                        },
                        ProjectedLyric::Extension => marker.into(),
                        other => panic!("unexpected authored projection lyric: {other:?}"),
                    };
                    (
                        lane,
                        scale(note.onset_ticks),
                        scale(note.duration_ticks),
                        note.pitch,
                        text,
                    )
                })
            })
            .collect();
        expected_lanes.sort();
        lane_notes.sort();
        assert_eq!(
            lane_notes, expected_lanes,
            "{export:?}: lyric/geometry at each original lane and onset"
        );
        assert_eq!(notes.iter().filter(|n| n.3 == marker).count(), holds);
        assert_eq!(map.len(), expected_tempos.len());
        for ((position, bpm), &(tick, expected_bpm)) in map.iter().zip(expected_tempos) {
            assert_eq!(*position, scale(tick));
            assert!((bpm - expected_bpm).abs() < 0.001);
        }
    }
}

#[test]
fn paired_native_three_four_preserve_parts_voices_harmony_and_source_bytes() {
    let mut expected: Vec<_> = [0, 1920]
        .into_iter()
        .flat_map(|offset| {
            [
                (0, 720, 60),
                (0, 720, 64),
                (720, 240, 62),
                (720, 240, 66),
                (960, 480, 67),
                (0, 1440, 55),
                (1440, 480, 60),
                (0, 480, 48),
                (1440, 480, 50),
            ]
            .map(move |(on, dur, pitch)| (on + offset, dur, pitch))
        })
        .collect();
    expected.sort_unstable();
    // These untexted polyphonic attacks have no qualified vocal continuation.
    // Keep their source geometry/ledger inventory; do not require invented holds.
    let vocal: Vec<_> = expected
        .iter()
        .copied()
        .filter(|(_, _, pitch)| !matches!(pitch, 62 | 66))
        .collect();
    for version in ["3.02", "4.70"] {
        let bytes = paired(version).into_bytes();
        let midi = musescore::parse(&bytes).unwrap();
        assert_eq!(geometry(&midi), expected);
        assert_eq!(midi.topology.parts.len(), 2);
        assert_eq!(
            midi.topology
                .parts
                .iter()
                .flat_map(|p| &p.staves)
                .flat_map(|s| &s.voices)
                .count(),
            3
        );
        let original_notes: Vec<_> = midi
            .tracks
            .iter()
            .flat_map(|t| &t.events)
            .filter_map(|e| match &e.kind {
                Kind::NoteOn(n) => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(
            original_notes.len(),
            20,
            "merged tie tails remain source evidence"
        );
        assert_eq!(
            original_notes
                .iter()
                .filter(|n| n.source.continuity.as_ref().unwrap().incoming_tie.is_some())
                .count(),
            2
        );
        for note in &original_notes {
            let proof = &note.source.continuity.as_ref().unwrap().evidence;
            assert_eq!(proof.source_version.as_deref(), Some(version));
            assert!(std::str::from_utf8(&bytes)
                .unwrap()
                .contains(proof.raw_xml.as_ref()));
        }
        assert_targets(
            &midi,
            &vocal,
            &[(0, 120.0), (1440, 90.0), (1920, 120.0), (3360, 90.0)],
            0,
        );
        for export in [ExportTarget::Svp, ExportTarget::Ustx] {
            let result = convert_midi_with_profile(
                &midi,
                "english",
                None,
                export,
                PronunciationProfile::Default,
            );
            let project = result.svp.as_ref().unwrap();
            assert_eq!(
                project
                    .meters
                    .iter()
                    .map(|m| (m.bar_index, m.numerator, m.denominator))
                    .collect::<Vec<_>>(),
                [(0, 3, 4), (1, 1, 4), (2, 3, 4), (3, 1, 4)]
            );
            let layout =
                BundleLayout::new(Path::new("authored.versebundle"), "authored.mscx", export)
                    .unwrap();
            let stems = StemPlan::from_source(&midi, &result.tracks).unwrap();
            let ledger = build_preservation_ledger(&midi, &result.projection, &layout, &stems);
            let allowed: BTreeSet<_> = [
                layout.source_relative_path.clone(),
                layout.project_relative_path.clone(),
            ]
            .into_iter()
            .chain(
                stems
                    .stems
                    .iter()
                    .map(|s| layout.stem_audio_relative_path(s)),
            )
            .collect();
            ledger.validate(&allowed).unwrap();
            assert!(ledger
                .entries
                .iter()
                .all(|e| e.artifact_paths.contains(&layout.source_relative_path)));
            assert_eq!(
                ledger
                    .entries
                    .iter()
                    .filter(|e| e.item_kind == SourceItemKind::Note)
                    .count(),
                20
            );
            for track in &midi.tracks {
                for event in &track.events {
                    if let Kind::NoteOn(note) = &event.kind {
                        if matches!(note.key, Some(62 | 66)) {
                            let id = format!(
                                "note:{}:{}:occurrence:{}:event:{}",
                                track.id, note.source.id, note.source.occurrence, event.order
                            );
                            let entry = ledger.entries.iter().find(|e| e.source_id == id).unwrap();
                            assert!(!matches!(
                                entry.disposition,
                                PrimaryDisposition::ProjectedExact
                                    | PrimaryDisposition::ProjectedMapped { .. }
                            ));
                        }
                    }
                }
            }
            let intensity = midi.score_intensity.as_ref().unwrap();
            let dynamics: Vec<_> = intensity
                .original_declarations
                .iter()
                .filter(|(_, declaration)| {
                    declaration.kinds.contains(
                        &verse_lib::engine::score_intensity::source::DeclarationKind::Dynamic,
                    )
                })
                .collect();
            assert_eq!(
                dynamics.len(),
                2,
                "both authored dynamics remain inventoried"
            );
            let mut raw_dynamics = BTreeSet::new();
            for (id, _) in dynamics {
                let raw = &intensity.retained[id].raw_fields["xml"];
                assert!(std::str::from_utf8(&bytes).unwrap().contains(raw));
                raw_dynamics.insert(raw.as_str());
                let entry = ledger
                    .entries
                    .iter()
                    .find(|entry| &entry.source_id == id)
                    .unwrap();
                assert_eq!(entry.item_kind, SourceItemKind::Event);
                match &entry.disposition {
                    PrimaryDisposition::ProjectedMapped { policy, .. } => {
                        assert!(!policy.is_empty());
                        assert_eq!(result.projection.performance_mapped.get(id), Some(policy));
                        assert!(entry.artifact_paths.contains(&layout.project_relative_path));
                    }
                    PrimaryDisposition::SourceOnly { reason } => {
                        let expected_reason = result.projection.performance_unmapped.get(id)
                            .map(String::as_str)
                            .unwrap_or("Retained authored score expression without an eligible editable target span");
                        assert_eq!(reason, expected_reason);
                        assert!(!result.projection.performance_mapped.contains_key(id));
                        assert!(!entry.artifact_paths.contains(&layout.project_relative_path));
                    }
                    other => panic!("authored intensity requires a mapped or source-only disposition: {other:?}"),
                }
            }
            assert_eq!(
                raw_dynamics,
                BTreeSet::from([
                    "<Dynamic><subtype>p</subtype><velocity>49</velocity></Dynamic>",
                    "<Dynamic><subtype>f</subtype><velocity>96</velocity></Dynamic>",
                ])
            );
            let encoded = target::serialize_to(export, project).unwrap();
            let text = std::str::from_utf8(&encoded).unwrap();
            for visual in ["spatium", "concertPitch", "#ff123456", "transposeChromatic"] {
                assert!(!text.contains(visual));
            }
            let input = BundleInput {
                original_name: "authored.mscx".into(),
                source_format: "mscx".into(),
                source_bytes: bytes.clone(),
                project: BundleProject::from_projection(export, project).unwrap(),
                stem_plan: stems,
                ledger,
                warnings: Vec::new(),
            };
            assert_eq!(
                input.source_bytes, bytes,
                "snapshot payload preserves even unsupported visuals"
            );
        }
    }
}

fn held(version: &str, properties: &str, other: Option<String>) -> String {
    let mut bodies = vec![format!(
        r#"<Measure len="1/4"><voice><Tempo><tempo>2</tempo></Tempo><Fermata>{properties}</Fermata>{}</voice></Measure>"#,
        chord("quarter", 60, WORD)
    )];
    if let Some(body) = other {
        bodies.push(body);
    }
    score(version, &bodies)
}

#[test]
fn omitted_native_three_four_fermata_duration_keeps_the_same_written_tempo() {
    let legacy = musescore::parse(held("3.02", "", None).as_bytes()).unwrap();
    let modern =
        musescore::parse(held("4.70", "<subtype>fermataAbove</subtype>", None).as_bytes()).unwrap();
    assert_eq!(geometry(&legacy), geometry(&modern));
    assert_eq!(geometry(&legacy), [(0, 480, 60)]);
    assert_eq!(tempos(&legacy), [(0, 500_000)]);
    assert_eq!(tempos(&modern), [(0, 500_000)]);
    assert_targets(&legacy, &[(0, 480, 60)], &[(0, 120.0)], 0);
    assert_targets(&modern, &[(0, 480, 60)], &[(0, 120.0)], 0);
}

#[test]
fn native_three_four_monophonic_source_extension_keeps_both_target_hold_markers() {
    let body = format!(
        "<Measure len=\"1/2\"><voice>{}{}</voice></Measure>",
        chord(
            "quarter",
            60,
            "<Lyrics><text>one</text><ticks>480</ticks><ticks_f>1/4</ticks_f></Lyrics>"
        ),
        chord("quarter", 62, "")
    );
    for version in ["3.02", "4.70"] {
        let midi =
            musescore::parse(score(version, std::slice::from_ref(&body)).as_bytes()).unwrap();
        assert_eq!(geometry(&midi), [(0, 480, 60), (480, 480, 62)]);
        assert_targets(&midi, &[(0, 480, 60), (480, 480, 62)], &[(0, 120.0)], 1);
    }
}

#[test]
fn explicit_native_three_four_stretch_converges_on_dense_qualified_input() {
    for version in ["3.02", "4.70"] {
        let other = format!(
            r#"<Measure len="1/4"><voice>{}{}</voice></Measure>"#,
            chord("eighth", 48, WORD),
            chord("eighth", 50, WORD)
        );
        let midi = musescore::parse(
            held(version, "<timeStretch>1.5</timeStretch>", Some(other)).as_bytes(),
        )
        .unwrap();
        assert_eq!(tempos(&midi), [(0, 750_000), (239, 500_000)]);
        assert_targets(
            &midi,
            &[(0, 240, 48), (0, 480, 60), (240, 240, 50)],
            &[(0, 80.0), (239, 120.0)],
            0,
        );
    }
}

#[test]
fn native_four_active_fermata_over_implicit_gap_fails_closed() {
    let sparse = "<Measure><voice><Rest><durationType>whole</durationType></Rest></voice><voice><location><fractions>3/8</fractions></location><Chord><durationType>half</durationType><Note><pitch>48</pitch></Note></Chord></voice></Measure>";
    let xml = held(
        "4.70",
        "<timeStretch>1.5</timeStretch>",
        Some(sparse.into()),
    )
    .replace("len=\"1/4\"", "len=\"4/4\"")
    .replace(
        "<durationType>quarter</durationType>",
        "<durationType>whole</durationType>",
    );
    let error = musescore::parse(xml.as_bytes()).unwrap_err();
    assert!(error.starts_with("SOURCE_PLAYBACK_UNSUPPORTED:"), "{error}");
    assert!(error.contains("implicit gap decomposition"), "{error}");
    let disabled = xml.replace(
        "<timeStretch>1.5</timeStretch>",
        "<play>0</play><timeStretch>1.5</timeStretch>",
    );
    let nominal = musescore::parse(disabled.as_bytes()).unwrap();
    assert_eq!(geometry(&nominal), [(0, 1920, 60), (720, 960, 48)]);
    assert_eq!(tempos(&nominal), [(0, 500_000)]);
}

#[test]
fn four_native_four_tuplet_fixtures_pin_both_target_exactness_verdicts() {
    // Nine notes with 6:9 eighths reduce to exact thirds of a quarter.
    for (normal, actual, svp_ok, ustx_ok) in [
        (2, 3, true, true),
        (6, 9, true, true),
        (1, 7, true, false),
        (8, 9, true, false),
    ] {
        let notes = (0..actual)
            .map(|_| chord("eighth", 60, WORD))
            .collect::<String>();
        let xml = score("4.70", &[format!("<Measure><voice><Tuplet><normalNotes>{normal}</normalNotes><actualNotes>{actual}</actualNotes></Tuplet>{notes}<endTuplet/></voice></Measure>")]);
        let midi = musescore::parse(xml.as_bytes()).unwrap();
        let rows = geometry(&midi);
        assert_eq!(rows.len(), actual as usize);
        for (i, &(onset, duration, pitch)) in rows.iter().enumerate() {
            assert_eq!(
                u64::from(duration) * 2 * actual,
                u64::from(midi.ticks_per_beat) * normal
            );
            assert_eq!(onset, i as u32 * duration);
            assert_eq!(pitch, 60);
        }
        // Obtain the shared projection from a grid which accepts every authored fraction.
        let seed = projection(&midi, ExportTarget::Svp);
        for (export, accepted) in [(ExportTarget::Svp, svp_ok), (ExportTarget::Ustx, ustx_ok)] {
            let verdict = target::validate_for(export, &seed);
            assert_eq!(verdict.is_ok(), accepted, "{normal}:{actual}/{export:?}");
            let outcome = convert_midi_with_profile(
                &midi,
                "english",
                None,
                export,
                PronunciationProfile::Default,
            );
            assert_eq!(
                outcome.ok, accepted,
                "analysis gate: {normal}:{actual}/{export:?}"
            );
            if accepted {
                if ustx_ok {
                    assert_targets(&midi, &rows, &[(0, 120.0)], 0);
                }
                let model = target::svp::serialize(&seed).unwrap();
                let notes = &model.tracks[0].main_group.notes;
                let duration = (705_600_000 * normal / (2 * actual)) as i64;
                assert_eq!(notes.len(), actual as usize);
                for (i, note) in notes.iter().enumerate() {
                    assert_eq!(
                        (note.onset, note.duration, note.pitch),
                        (i as i64 * duration, duration, 60)
                    );
                }
            } else {
                let error = verdict.unwrap_err();
                assert!(error.contains("cannot be represented exactly"), "{error}");
                assert!(
                    matches!(target::serialize_to(export, &seed), Err(target::SerializeError::Unrepresentable(message)) if message == error)
                );
                assert!(outcome
                    .msg
                    .unwrap()
                    .contains("cannot be represented exactly"));
            }
        }
    }
}

fn repeat_bar(count: usize, size: Option<usize>) -> String {
    let item = size.map_or_else(
        || "<Rest><durationType>measure</durationType></Rest>".into(),
        |n| format!("<MeasureRepeat><subtype>{n}</subtype><durationType>measure</durationType><duration>1/4</duration></MeasureRepeat>"),
    );
    format!("<Measure len=\"1/4\"><measureRepeatCount>{count}</measureRepeatCount><voice>{item}</voice></Measure>")
}

fn repeat_score(version: &str, backing: &str, bars: usize, hold_bar: usize) -> String {
    let vocal: String = (0..bars)
        .map(|bar| {
            let tempo = if bar == 0 {
                "<Tempo><tempo>2</tempo></Tempo>"
            } else {
                ""
            };
            let hold = if bar == hold_bar {
                "<Fermata><subtype>fermataAbove</subtype><timeStretch>2</timeStretch></Fermata>"
            } else {
                ""
            };
            format!(
                "<Measure len=\"1/4\"><voice>{tempo}{hold}{}</voice></Measure>",
                chord("quarter", 60, WORD)
            )
        })
        .collect();
    score(version, &[vocal, backing.into()]).replace("<Instrument id=\"voice1\"><instrumentId>voice.soprano</instrumentId>",
        "<Instrument id=\"voice1\"><instrumentId>drum.group.set</instrumentId><useDrumset>1</useDrumset>")
}

#[test]
fn native_four_repeat_groups_and_chains_qualify_without_copied_ir_or_clock_attacks() {
    let dense = format!(
        "<Measure len=\"1/4\"><voice>{}{}</voice></Measure>",
        chord("eighth", 36, ""),
        chord("eighth", 38, "")
    );
    for size in [1, 2, 4] {
        let group: String = (1..=size)
            .map(|count| {
                repeat_bar(
                    count,
                    (count == if size == 1 { 1 } else { 2 }).then_some(size),
                )
            })
            .collect();
        // A second group resolves through the first to genuine written bars.
        let backing = format!("{}{group}{group}", dense.repeat(size));
        let xml = repeat_score("4.70", &backing, 3 * size, 2 * size);
        let midi = musescore::parse(xml.as_bytes()).unwrap();
        let disabled = musescore::parse(
            xml.replace(
                "<subtype>fermataAbove</subtype>",
                "<subtype>fermataAbove</subtype><play>0</play>",
            )
            .as_bytes(),
        )
        .unwrap();
        let musical_events = |m: &Midi| {
            m.tracks
                .iter()
                .flat_map(|t| &t.events)
                .filter(|e| matches!(e.kind, Kind::NoteOn(_) | Kind::NoteOff(_)))
                .map(|e| (e.tick, e.kind.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            musical_events(&midi),
            musical_events(&disabled),
            "all original note/lyric/provenance fields"
        );
        assert_eq!(
            midi.tracks
                .iter()
                .flat_map(|t| &t.events)
                .filter(|e| matches!(e.kind, Kind::NoteOn(_)))
                .count(),
            5 * size
        );
        let start = 960 * size as u32;
        // Native score.cpp next1() sees the notated glyph, not copied drum attacks
        // at start+240. The restore remains at the next written boundary minus eps.
        assert_eq!(
            tempos(&midi),
            [(0, 500_000), (start, 1_000_000), (start + 479, 500_000)]
        );
        let expected: Vec<_> = (0..3 * size)
            .map(|bar| (bar as u32 * 480, 480, 60))
            .collect();
        assert_targets(
            &midi,
            &expected,
            &[(0, 120.0), (start, 60.0), (start + 479, 120.0)],
            0,
        );
        let legacy = musescore::parse(
            repeat_score("3.02", &backing, 3 * size, 2 * size)
                .replace("<timeStretch>2</timeStretch>", "")
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(geometry(&legacy), geometry(&disabled));
        assert_eq!(
            tempos(&legacy),
            [(0, 500_000)],
            "native 3 omitted stretch stays one"
        );
    }
}

#[test]
fn native_four_mixed_repeat_members_and_chains_use_exact_n_bar_references() {
    let dense = format!(
        "<Measure len=\"1/4\"><voice>{}{}</voice></Measure>",
        chord("eighth", 36, ""),
        chord("eighth", 38, "")
    );
    let sparse = format!(
        "<Measure len=\"1/4\"><voice>{}</voice></Measure>",
        chord("eighth", 36, "")
    );
    for size in [2, 4] {
        for dense_first in [true, false] {
            let source: String = (0..size)
                .map(|member| {
                    if (member % 2 == 0) == dense_first {
                        dense.as_str()
                    } else {
                        sparse.as_str()
                    }
                })
                .collect();
            let group: String = (1..=size)
                .map(|member| repeat_bar(member, (member == 2).then_some(size)))
                .collect();
            let backing = format!("{source}{group}{group}");
            for chain in [1, 2] {
                for member in 0..size {
                    let hold_bar = chain * size + member;
                    let parsed = musescore::parse(
                        repeat_score("4.70", &backing, 3 * size, hold_bar).as_bytes(),
                    );
                    if (member % 2 == 0) == dense_first {
                        let midi =
                            parsed.expect("this member resolves to its earlier dense source bar");
                        let start = hold_bar as u32 * 480;
                        assert_eq!(
                            tempos(&midi),
                            [(0, 500_000), (start, 1_000_000), (start + 479, 500_000)]
                        );
                        let expected: Vec<_> = (0..3 * size)
                            .map(|bar| (bar as u32 * 480, 480, 60))
                            .collect();
                        assert_targets(
                            &midi,
                            &expected,
                            &[(0, 120.0), (start, 60.0), (start + 479, 120.0)],
                            0,
                        );
                    } else {
                        let error = parsed
                            .expect_err("this member resolves to an unqualified gapped source bar");
                        assert!(error.starts_with("SOURCE_PLAYBACK_UNSUPPORTED:"), "{error}");
                        assert!(error.contains("MeasureRepeat"), "{error}");
                    }
                }
            }
        }
    }
}

#[test]
fn native_four_unqualified_repeat_scope_or_reference_stays_fail_closed() {
    let dense = format!(
        "<Measure len=\"1/4\"><voice>{}{}</voice></Measure>",
        chord("eighth", 36, ""),
        chord("eighth", 38, "")
    );
    let sparse = format!(
        "<Measure len=\"1/4\"><voice>{}</voice></Measure>",
        chord("eighth", 36, "")
    );
    let repeated = repeat_bar(1, Some(1));
    for (backing, bars, hold_bar) in [
        (repeated.clone(), 1, 0), // Earlier source missing; no forward/self fallback.
        (format!("{sparse}{repeated}"), 2, 1), // Source gap decomposition unqualified.
        (format!("{dense}{}", repeat_bar(1, Some(3))), 2, 1),
        (format!("{dense}{}", repeat_bar(1, Some(2))), 2, 1), // Incomplete group.
        (
            format!(
                "{dense}{}",
                repeated.replace(
                    "</voice>",
                    &format!("{}</voice>", chord("quarter", 48, WORD))
                )
            ),
            2,
            1,
        ),
        (
            format!(
                "{dense}{}",
                repeated.replace(
                    "<subtype>1</subtype>",
                    "<subtype>1</subtype><subtype>2</subtype>"
                )
            ),
            2,
            1,
        ),
        (
            format!("{dense}{}", repeated.replace("len=\"1/4\"", "len=\"1/2\"")),
            2,
            1,
        ),
    ] {
        let error = musescore::parse(repeat_score("4.70", &backing, bars, hold_bar).as_bytes())
            .unwrap_err();
        assert!(error.starts_with("SOURCE_PLAYBACK_UNSUPPORTED:"), "{error}");
        assert!(error.contains("MeasureRepeat"), "{error}");
    }
}
