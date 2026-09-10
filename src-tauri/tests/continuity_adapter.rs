//! Iteration-1 MuseScore continuity contracts through parsing and both writers.
//! Synthetic XML/ZIP fixtures preserve original source identities; no corpus,
//! renderer or source-file mutation is required.

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use verse_lib::engine::convert::{convert_midi_with_profile, ConvertOutcome};
use verse_lib::engine::midi::{Kind, Midi, NoteOn};
use verse_lib::engine::musescore;
use verse_lib::engine::projection::{ProjectedLyric, ProjectedNote, ProjectedProject};
use verse_lib::engine::target::{self, ExportTarget, PronunciationProfile};

const LINK_INVALID: &str = "SOURCE_CONTINUITY_LINK_INVALID";

fn score(measures: &str) -> String {
    format!(
        r#"<museScore version="3.02"><programVersion>3.6.2</programVersion><Score><Division>480</Division>
        <Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part>
        <Staff id="1">{measures}</Staff></Score></museScore>"#
    )
}

fn lyric(row: u8, text: &str, fields: &str) -> String {
    format!("<Lyrics><no>{row}</no><text>{text}</text>{fields}</Lyrics>")
}

fn chord(duration: &str, lyrics: &str, notes: &str) -> String {
    format!("<Chord><durationType>{duration}</durationType>{lyrics}{notes}</Chord>")
}

fn note(pitch: u8, tie: &str) -> String {
    format!("<Note><pitch>{pitch}</pitch>{tie}</Note>")
}

fn container(xml: &str, zipped: bool) -> Vec<u8> {
    if !zipped {
        return xml.as_bytes().to_vec();
    }
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("score.mscx", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(xml.as_bytes()).unwrap();
    writer.finish().unwrap().into_inner()
}

struct Original {
    id: String,
    track_id: String,
    onset: u32,
    duration: u32,
    on_order: u32,
    off_order: u32,
    note: NoteOn,
}

fn originals(midi: &Midi) -> BTreeMap<String, Original> {
    let mut result = BTreeMap::new();
    for track in &midi.tracks {
        for (index, event) in track.events.iter().enumerate() {
            let Kind::NoteOn(note) = &event.kind else {
                continue;
            };
            let off = track.events[index + 1..].iter().find(|event| {
                matches!(&event.kind, Kind::NoteOff(off) if off.source_id.as_deref() == Some(note.source.id.as_str()))
            }).expect("the adapter retains an explicit source-owned note-off");
            let id = format!(
                "note:{}:{}:occurrence:{}:event:{}",
                track.id, note.source.id, note.source.occurrence, event.order
            );
            assert!(result
                .insert(
                    id.clone(),
                    Original {
                        id,
                        track_id: track.id.clone(),
                        onset: event.tick,
                        duration: off.tick - event.tick,
                        on_order: event.order,
                        off_order: off.order,
                        note: note.clone(),
                    }
                )
                .is_none());
        }
    }
    result
}

fn retained<'a>(project: &'a ProjectedProject, original: &Original) -> &'a ProjectedNote {
    let notes: Vec<_> = project
        .tracks
        .iter()
        .flat_map(|track| &track.notes)
        .filter(|note| {
            note.source_evidence
                .as_ref()
                .is_some_and(|e| e.note_id == original.id)
        })
        .collect();
    assert_eq!(notes.len(), 1, "one representation of {}", original.id);
    notes[0]
}

fn assert_originals(project: &ProjectedProject, source: &BTreeMap<String, Original>) {
    for note in project.tracks.iter().flat_map(|track| &track.notes) {
        let evidence = note.source_evidence.as_ref().unwrap();
        let original = &source[&evidence.note_id];
        let origin = evidence.origin.as_ref().unwrap();
        assert_eq!(
            (note.onset_ticks, note.duration_ticks, Some(note.pitch)),
            (original.onset, original.duration, original.note.key)
        );
        assert_eq!(origin.source, original.note.source);
        assert_eq!(origin.track_id, original.track_id);
        assert_eq!(origin.note_on_order, original.on_order);
        assert_eq!(origin.note_off_order, original.off_order);
        assert_eq!(
            evidence.note_on_event_id,
            format!("event:{}:{}", original.track_id, original.on_order)
        );
        assert_eq!(
            evidence.note_off_event_id,
            format!("event:{}:{}", original.track_id, original.off_order)
        );
    }
}

fn convert(midi: &Midi, target: ExportTarget) -> ConvertOutcome {
    let result =
        convert_midi_with_profile(midi, "english", None, target, PronunciationProfile::Default);
    assert!(result.ok, "{target:?}: {:?}", result.msg);
    result
}

fn assert_serialized(project: &ProjectedProject, export: ExportTarget) {
    target::validate_for(export, project).expect("analysis accepts exact continuity");
    let bytes = target::serialize_to(export, project).expect("the actual writer accepts it too");
    match export {
        ExportTarget::Svp => {
            let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let tracks = saved["tracks"].as_array().unwrap();
            assert_eq!(tracks.len(), project.tracks.len());
            for (track, saved) in project.tracks.iter().zip(tracks) {
                let notes = saved["mainGroup"]["notes"].as_array().unwrap();
                assert_eq!(notes.len(), track.notes.len());
                for (note, saved) in track.notes.iter().zip(notes) {
                    let exact = |ticks| {
                        u64::from(ticks) * target::svp::BLICKS_PER_QUARTER
                            / u64::from(project.ticks_per_beat)
                    };
                    assert_eq!(saved["onset"].as_u64(), Some(exact(note.onset_ticks)));
                    assert_eq!(saved["duration"].as_u64(), Some(exact(note.duration_ticks)));
                    assert_eq!(saved["pitch"].as_u64(), Some(u64::from(note.pitch)));
                    if matches!(note.lyric, ProjectedLyric::Extension) {
                        assert_eq!(saved["lyrics"], "-");
                    }
                }
            }
        }
        ExportTarget::Ustx => {
            let saved = target::ustx::serialize(project).unwrap();
            assert_eq!(bytes, target::ustx::to_yaml(&saved).as_bytes());
            assert_eq!(saved.voice_parts.len(), project.tracks.len());
            for (track, part) in project.tracks.iter().zip(&saved.voice_parts) {
                assert_eq!(part.notes.len(), track.notes.len());
                for (note, emitted) in track.notes.iter().zip(&part.notes) {
                    let exact = |ticks| i64::from(ticks) * 480 / i64::from(project.ticks_per_beat);
                    assert_eq!(
                        i64::from(part.position) + i64::from(emitted.position),
                        exact(note.onset_ticks)
                    );
                    assert_eq!(i64::from(emitted.duration), exact(note.duration_ticks));
                    assert_eq!(emitted.tone, note.pitch);
                    if matches!(note.lyric, ProjectedLyric::Extension) {
                        assert_eq!(emitted.lyric, "+~");
                    }
                }
            }
        }
    }
}

fn extension_phrase(fields: &str) -> String {
    format!(
        r#"<Measure len="3/4"><voice>{}{}{}</voice></Measure>"#,
        chord("quarter", &lyric(0, "hold", fields), &note(65, "")),
        chord("quarter", "", &note(65, "")),
        chord("quarter", &lyric(0, "end", ""), &note(67, ""))
    )
}

#[test]
fn repeated_extension_fields_require_numeric_agreement_in_mscx_and_mscz() {
    for (fields, valid) in [
        ("<ticks>480</ticks><ticks>0480</ticks>", true),
        ("<ticks_f>1/4</ticks_f><ticks_f>2/8</ticks_f>", true),
        (
            "<ticks>480</ticks><ticks>480</ticks><ticks_f>1/4</ticks_f><ticks_f>2/8</ticks_f>",
            true,
        ),
        ("<ticks>480</ticks><ticks>960</ticks>", false),
        ("<ticks>960</ticks><ticks>480</ticks>", false),
        ("<ticks_f>1/4</ticks_f><ticks_f>1/2</ticks_f>", false),
        ("<ticks_f>1/2</ticks_f><ticks_f>1/4</ticks_f>", false),
        ("<ticks>480</ticks><ticks>broken</ticks>", false),
        ("<ticks_f>1/4</ticks_f><ticks_f/>", false),
    ] {
        let xml = score(&format!(
            "{}{}",
            extension_phrase(fields),
            extension_phrase("<ticks>480</ticks>")
        ));
        for zipped in [false, true] {
            let bytes = container(&xml, zipped);
            let before = bytes.clone();
            let midi = musescore::parse(&bytes).unwrap();
            assert_eq!(bytes, before, "source container is immutable");
            let source = originals(&midi);
            let head = source.values().find(|note| note.onset == 0).unwrap();
            let proof = head.note.source.continuity.as_ref().unwrap();
            assert_eq!(
                proof.extensions[0].end_tick,
                valid.then_some(480),
                "{fields}"
            );
            assert_eq!(proof.issues.is_empty(), valid, "{fields}");
            assert!(proof.extensions[0].evidence.raw_xml.contains(fields));
            assert_eq!(
                proof.extensions[0].evidence.source_id,
                head.note.lyrics[0].id
            );
            assert_eq!(head.note.lyrics[0].raw, "hold");
            if !valid {
                assert_eq!(proof.issues[0].code, LINK_INVALID);
                assert!(proof.issues[0].message.contains(&head.note.lyrics[0].id));
            }
            for export in [ExportTarget::Svp, ExportTarget::Ustx] {
                let result = convert(&midi, export);
                let project = result.svp.as_ref().unwrap();
                assert_originals(project, &source);
                let held: Vec<_> = project
                    .tracks
                    .iter()
                    .flat_map(|track| &track.notes)
                    .filter(|note| matches!(note.lyric, ProjectedLyric::Extension))
                    .map(|note| note.onset_ticks)
                    .collect();
                assert_eq!(
                    held,
                    if valid { vec![480, 1920] } else { vec![1920] },
                    "{fields}"
                );
                if !valid {
                    assert!(result
                        .tracks
                        .iter()
                        .flat_map(|track| &track.warnings)
                        .any(|issue| issue.code == LINK_INVALID
                            && issue.source_id.as_deref() == Some(head.id.as_str())));
                }
                assert_serialized(project, export);
            }
        }
    }
}

#[test]
fn legacy_text_bearing_ties_preserve_original_links_and_both_target_markers() {
    // The two-chain case is deliberately unison: distinct explicit IDs, not
    // pitch/source-order selection, establish each predecessor.
    for chains in [1, 2] {
        let heads: String = (0..chains)
            .map(|i| note(65, &format!(r#"<Tie id="chain-{i}"/>"#)))
            .collect();
        let tails: String = (0..chains)
            .map(|i| note(65, &format!(r#"<endSpanner id="chain-{i}"/>"#)))
            .collect();
        let xml = score(&format!(
            r#"<Measure len="3/4"><startRepeat/>{}{}{}<endRepeat>2</endRepeat></Measure>"#,
            chord(
                "quarter",
                &format!("{}{}", lyric(0, "hold", ""), lyric(1, "again", "")),
                &heads
            ),
            chord("quarter", &lyric(1, "tail", ""), &tails),
            chord(
                "quarter",
                &format!("{}{}", lyric(0, "end", ""), lyric(1, "end", "")),
                &note(67, "")
            )
        ))
        .replace("version=\"3.02\"", "version=\"2.06\"")
        .replace(
            "<programVersion>3.6.2</programVersion>",
            "<programVersion>2.3.2</programVersion>",
        );
        for zipped in [false, true] {
            let midi = musescore::parse(&container(&xml, zipped)).unwrap();
            let source = originals(&midi);
            assert!(
                source.values().all(|n| n
                    .note
                    .source
                    .continuity
                    .as_ref()
                    .unwrap()
                    .issues
                    .is_empty()),
                "resolved text-bearing tails must not leave abandoned-start diagnostics"
            );
            for export in [ExportTarget::Svp, ExportTarget::Ustx] {
                let result = convert(&midi, export);
                let project = result.svp.as_ref().unwrap();
                assert_originals(project, &source);
                for tail in source.values().filter(|n| matches!(n.onset, 480 | 1920)) {
                    assert_eq!(tail.duration, 480);
                    assert_eq!(tail.note.key, Some(65));
                    let continuity = tail.note.source.continuity.as_ref().unwrap();
                    let tie = continuity
                        .incoming_tie
                        .as_ref()
                        .expect("legacy incoming relation");
                    assert_eq!(tie.tail.source_id, tail.note.source.id);
                    assert_eq!(tie.tail.occurrence, tail.note.source.occurrence);
                    assert_eq!(tie.contact_tick, tail.onset);
                    assert_eq!(tie.pitch, 65);
                    assert!(tie.evidence[0].raw_xml.contains("<Tie id="));
                    assert!(tie.evidence[1].raw_xml.contains("<endSpanner id="));
                    assert_eq!(tie.evidence[0].source_version.as_deref(), Some("2.06"));
                    let head = source
                        .values()
                        .find(|n| {
                            n.note.source.id == tie.head.source_id
                                && n.note.source.occurrence == tie.head.occurrence
                        })
                        .unwrap();
                    assert_eq!(
                        head.duration, 480,
                        "a text-bearing tail is never merged into its head"
                    );
                    let projected = retained(project, tail);
                    if tail.note.source.occurrence == 0 {
                        assert!(matches!(projected.lyric, ProjectedLyric::Extension));
                        let link = projected
                            .source_evidence
                            .as_ref()
                            .unwrap()
                            .origin
                            .as_ref()
                            .unwrap()
                            .continuation
                            .as_ref()
                            .unwrap();
                        assert_eq!(link.predecessor_id, head.id);
                        assert_eq!(link.lyric_owner_id, head.note.lyrics[0].id);
                        assert_eq!(link.destination_track_id, head.track_id);
                        assert_eq!(
                            retained(project, head).onset_ticks + head.duration,
                            projected.onset_ticks
                        );
                    } else {
                        assert!(
                            matches!(&projected.lyric, ProjectedLyric::Source(l) if l.raw == "tail")
                        );
                        assert!(projected
                            .source_evidence
                            .as_ref()
                            .unwrap()
                            .origin
                            .as_ref()
                            .unwrap()
                            .continuation
                            .is_none());
                    }
                }
                assert_serialized(project, export);
            }
        }
    }
}

#[test]
fn scaled_extension_bounds_keep_exact_endpoints_and_target_grid_checks() {
    for fields in [
        "<ticks>960</ticks>",
        "<ticks_f>1/2</ticks_f>",
        "<ticks>960</ticks><ticks_f>1/2</ticks_f>",
    ] {
        for sung_tuplet in [false, true] {
            let pickup = if sung_tuplet {
                lyric(0, "pick", "")
            } else {
                String::new()
            };
            let xml = score(&format!(
                r#"<Measure><voice><Tuplet id="1"><normalNotes>1</normalNotes><actualNotes>7</actualNotes></Tuplet>{}<endTuplet/></voice></Measure>
                <Measure><voice>{}{}{}{}</voice></Measure>"#,
                chord("quarter", &pickup, &note(60, "")),
                chord("quarter", &lyric(0, "hold", fields), &note(65, "")),
                chord("quarter", "", &note(65, "")),
                chord("quarter", "", &note(65, "")),
                chord("quarter", "", &note(65, ""))
            ));
            for zipped in [false, true] {
                let midi = musescore::parse(&container(&xml, zipped)).unwrap();
                assert_eq!(
                    midi.ticks_per_beat, 3360,
                    "the source duration forces sevenfold PPQ"
                );
                let source = originals(&midi);
                let head = source.values().find(|n| n.onset == 13440).unwrap();
                let extension = &head.note.source.continuity.as_ref().unwrap().extensions[0];
                assert_eq!(extension.start_tick, 13440);
                assert_eq!(extension.extend_ticks, Some(6720));
                assert_eq!(extension.end_tick, Some(20160));
                assert_eq!(
                    extension.raw_ticks,
                    fields.contains("<ticks>").then_some(960)
                );
                assert_eq!(
                    extension.extend_fraction,
                    fields.contains("<ticks_f>").then_some((1, 2))
                );
                let result = convert(&midi, ExportTarget::Svp);
                let project = result.svp.as_ref().unwrap();
                assert_originals(project, &source);
                let held: Vec<_> = project
                    .tracks
                    .iter()
                    .flat_map(|track| &track.notes)
                    .filter(|note| matches!(note.lyric, ProjectedLyric::Extension))
                    .map(|note| (note.onset_ticks, note.duration_ticks))
                    .collect();
                assert_eq!(held, vec![(16800, 3360), (20160, 3360)]);
                assert!(
                    project
                        .tracks
                        .iter()
                        .flat_map(|track| &track.notes)
                        .all(|note| note.onset_ticks != 23520),
                    "the chord after the inclusive endpoint is not covered"
                );
                assert_serialized(project, ExportTarget::Svp);
                let ustx = convert_midi_with_profile(
                    &midi,
                    "english",
                    None,
                    ExportTarget::Ustx,
                    PronunciationProfile::Default,
                );
                if sung_tuplet {
                    assert!(
                        !ustx.ok,
                        "the retained 1/7-quarter pickup is off the native USTX grid"
                    );
                    let analysis = target::validate_for(ExportTarget::Ustx, project).unwrap_err();
                    let write = target::serialize_to(ExportTarget::Ustx, project).unwrap_err();
                    assert_eq!(
                        analysis,
                        write.message(),
                        "analysis and serialization refuse the same unsafe grid"
                    );
                } else {
                    assert!(
                        ustx.ok,
                        "expanded PPQ alone is not a refusal: {:?}",
                        ustx.msg
                    );
                    assert_serialized(ustx.svp.as_ref().unwrap(), ExportTarget::Ustx);
                }
            }
        }
    }
}

#[test]
fn unresolved_outgoing_starts_diagnose_overwrite_discontinuity_and_eof_by_origin() {
    let start = r#"<Tie id="open"/>"#;
    for (measures, expected) in [
        (
            format!(
                "<Measure><voice>{}{}{}</voice></Measure>",
                chord("quarter", &lyric(0, "one", ""), &note(65, start)),
                chord("quarter", &lyric(0, "two", ""), &note(65, start)),
                chord("quarter", &lyric(0, "end", ""), &note(67, ""))
            ),
            ["overwrote", "end of score"],
        ),
        (
            format!(
                r#"<Measure len="1/4"><startRepeat/><voice>{}</voice><endRepeat>2</endRepeat></Measure>"#,
                chord("quarter", &lyric(0, "word", ""), &note(65, start))
            ),
            ["playback discontinuity", "end of score"],
        ),
    ] {
        for zipped in [false, true] {
            let midi = musescore::parse(&container(&score(&measures), zipped)).unwrap();
            let source = originals(&midi);
            let mut starts: Vec<_> = source.values().filter(|n| n.note.key == Some(65)).collect();
            starts.sort_by_key(|n| n.onset);
            assert_eq!(starts.len(), 2);
            let result = convert(&midi, ExportTarget::Svp);
            let project = result.svp.as_ref().unwrap();
            assert_originals(project, &source);
            assert!(project
                .tracks
                .iter()
                .flat_map(|t| &t.notes)
                .all(|n| !matches!(n.lyric, ProjectedLyric::Extension)));
            for (original, reason) in starts.into_iter().zip(expected) {
                assert_eq!(original.duration, 480);
                let proof = original.note.source.continuity.as_ref().unwrap();
                assert!(proof.incoming_tie.is_none());
                assert_eq!(
                    proof.issues.len(),
                    1,
                    "one bounded outgoing diagnostic per source start"
                );
                let issue = &proof.issues[0];
                assert_eq!(issue.code, LINK_INVALID);
                assert_eq!(issue.evidence.source_id, original.note.source.id);
                assert!(issue.evidence.raw_xml.contains(start));
                assert!(issue.message.contains(reason));
                assert!(issue
                    .message
                    .contains(&format!("occurrence {}", original.note.source.occurrence)));
                assert!(result
                    .tracks
                    .iter()
                    .flat_map(|t| &t.warnings)
                    .any(|warning| warning.code == LINK_INVALID
                        && warning.source_id.as_deref() == Some(original.id.as_str())
                        && warning.message.contains(reason)));
            }
        }
    }
}

#[test]
fn resolved_bare_legacy_tails_keep_the_merge_without_outgoing_diagnostics() {
    let xml = score(&format!(
        "<Measure><voice>{}{}</voice></Measure>",
        chord(
            "quarter",
            &lyric(0, "hold", ""),
            &note(65, r#"<Tie id="8"/>"#)
        ),
        chord("quarter", "", &note(65, r#"<endSpanner id="8"/>"#))
    ));
    let midi = musescore::parse(xml.as_bytes()).unwrap();
    let source = originals(&midi);
    let head = source.values().find(|n| n.onset == 0).unwrap();
    let tail = source.values().find(|n| n.onset == 480).unwrap();
    assert_eq!(head.duration, 960);
    assert_eq!(tail.note.key, None);
    assert_eq!(tail.duration, 480);
    assert!(source
        .values()
        .all(|n| n.note.source.continuity.as_ref().unwrap().issues.is_empty()));
    let tie = tail
        .note
        .source
        .continuity
        .as_ref()
        .unwrap()
        .incoming_tie
        .as_ref()
        .unwrap();
    assert_eq!(tie.head.source_id, head.note.source.id);
    for export in [ExportTarget::Svp, ExportTarget::Ustx] {
        let result = convert(&midi, export);
        let project = result.svp.as_ref().unwrap();
        assert_eq!(
            project.tracks.iter().map(|t| t.notes.len()).sum::<usize>(),
            1
        );
        assert_originals(project, &source);
        assert_serialized(project, export);
    }
}

#[test]
fn unresolved_outgoing_tracking_and_diagnostics_have_explicit_budgets() {
    for unique in [false, true] {
        let mut chords = String::new();
        for index in 0..4097 {
            let id = if unique { index } else { 0 };
            chords.push_str(&chord(
                "quarter",
                "",
                &note(65, &format!(r#"<Tie id="{id}"/>"#)),
            ));
        }
        let xml = score(&format!("<Measure><voice>{chords}</voice></Measure>"));
        let error = musescore::parse(xml.as_bytes())
            .expect_err("unbounded unresolved starts cannot return partial success");
        assert!(error.contains("SOURCE_CONTINUITY_LIMIT"), "{error}");
        assert!(
            error.contains(if unique {
                "pending outgoing ties"
            } else {
                "unresolved tie diagnostics"
            }),
            "{error}"
        );
    }
}
