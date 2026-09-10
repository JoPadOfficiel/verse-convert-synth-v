//! FID-002 acceptance through the real MuseScore adapter, projector and writers.
//! Synthetic notation only: no private score or rendered audio is embedded.
//! Blank measures preserve the audited absolute ticks and repeat interval while
//! the few authored attacks isolate ownership, eligibility and non-restoration.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use verse_lib::bundle::{
    build_preservation_ledger, BundleLayout, PrimaryDisposition, SourceItemKind,
};
use verse_lib::engine::convert::{convert_midi_with_profile, ConvertOutcome};
use verse_lib::engine::midi::{Kind, Lyric, Midi, NoteSource};
use verse_lib::engine::musescore;
use verse_lib::engine::projection::{
    ProjectedLyric, ProjectedNote, ProjectedProject, ProjectedTrack,
};
use verse_lib::engine::target::{self, ExportTarget, PronunciationProfile};
use verse_lib::stems::StemPlan;

const ALTI_MAIN: &str = "mscx:staff:2:voice:1";
const ALTI_HIGH: &str = "mscx:staff:2:voice:1:polyphonic-member:2";
const SOP: &str = "mscx:staff:1:voice:1";
const TARGET_PROFILES: [(ExportTarget, PronunciationProfile); 4] = [
    (ExportTarget::Svp, PronunciationProfile::Default),
    (ExportTarget::Ustx, PronunciationProfile::Default),
    (ExportTarget::Ustx, PronunciationProfile::FrenchMillefeuille),
    (ExportTarget::Ustx, PronunciationProfile::EnglishArpabet),
];

fn lyric(row: u8, text: &str, evidence: &str) -> String {
    format!("<Lyrics><no>{row}</no>{evidence}<text>{text}</text></Lyrics>")
}

fn chord(duration: &str, pitches: &[u8], lyrics: &str, tie: &str) -> String {
    let notes: String = pitches
        .iter()
        .map(|pitch| format!("<Note><pitch>{pitch}</pitch>{tie}</Note>"))
        .collect();
    format!("<Chord><durationType>{duration}</durationType>{lyrics}{notes}</Chord>")
}

fn rest(duration: &str) -> String {
    format!("<Rest><durationType>{duration}</durationType></Rest>")
}

fn before_last_eighth() -> String {
    format!("{}{}{}", rest("half"), rest("quarter"), rest("eighth"))
}

const TIE_NEXT_BAR: &str = r#"<Spanner type="Tie"><Tie/><next><location><measures>1</measures><fractions>-7/8</fractions></location></next></Spanner>"#;
const TIE_PREVIOUS_BAR: &str = r#"<Spanner type="Tie"><prev><location><measures>-1</measures><fractions>7/8</fractions></location></prev></Spanner>"#;

/// Three source voices, five adapter lanes, 27 original note instances and 21
/// editable notes. PB previously lost two of those 21; chant lost only Alti.
/// No performance marks: source-owned tie contexts must not create automation.
fn continuity_score(chant: bool) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><museScore version="3.02"><programVersion>3.6.2</programVersion><Score><Division>480</Division>"#,
    );
    for (staff, name) in [(1, "Sop"), (2, "Alti"), (3, "Bass")] {
        xml.push_str(&format!(
            r#"<Part><trackName>{name}</trackName><Staff id="{staff}"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part>"#,
        ));
    }
    for staff in 1..=3 {
        xml.push_str(&format!(r#"<Staff id="{staff}">"#));
        for measure in 1..=84 {
            xml.push_str("<Measure>");
            // MuseScore stores global repeat navigation on the first staff.
            if staff == 1 && measure == 1 {
                xml.push_str("<startRepeat/>");
            }
            if staff == 1 && measure == 29 {
                xml.push_str("<endRepeat>2</endRepeat>");
            }
            xml.push_str("<voice>");
            let body = match (staff, measure) {
                (2, 10) => format!("{}{}", rest("quarter"), chord("quarter", &[59], &lyric(0, "presse", ""), "")),
                (2, 11) => format!(
                    "{}{}{}{}",
                    rest("eighth"),
                    chord("eighth", &[59, 68], &format!("{}{}",
                        lyric(0, if chant { "Même" } else { "Même'" }, "<ticks>240</ticks><ticks_f>1/8</ticks_f>"),
                        lyric(1, "chan", "<syllabic>begin</syllabic>")), ""),
                    chord("eighth", &[68], &lyric(1, "ger", "<syllabic>end</syllabic><ticks_f>0/8</ticks_f>"), ""),
                    // A verse-one word in the endpoint's main lane prevents
                    // the existing refrain rule from borrowing verse two.
                    chord("quarter", &[64], &format!("{}{}", lyric(0, "fin", ""), lyric(1, "fin", "")), ""),
                ),
                (1, 20) => format!("{}{}", before_last_eighth(), chord("eighth", &[64], &format!("{}{}",
                    lyric(0, if chant { "mur" } else { "murs" }, if chant { "<ticks>240</ticks><ticks_f>1/8</ticks_f>" } else { "" }),
                    lyric(1, "cou", "<syllabic>begin</syllabic><ticks>1</ticks><ticks_f>1/1920</ticks_f>")), TIE_NEXT_BAR)),
                (1, 21) => format!("{}{}",
                    chord("half", &[64], &lyric(1, "rants", "<syllabic>end</syllabic>"), TIE_PREVIOUS_BAR),
                    chord("quarter", &[65], &format!("{}{}", lyric(0, "fin", ""), lyric(1, "fin", "")), "")),
                (3, 65) => format!("{}{}", before_last_eighth(), chord("eighth", &[61], &lyric(0, "presse,", ""), "")),
                // Bass 180720 follows a real 240-tick rest.
                (3, 66) => format!("{}{}", rest("eighth"), chord("quarter", &[61], "", "")),
                (2 | 3, 79) => {
                    let pitch = if staff == 2 { 66 } else { 57 };
                    format!("{}{}{}", rest("quarter"), chord("eighth", &[pitch], &lyric(0, "même", ""), ""), chord("eighth", &[pitch], "", ""))
                }
                (2 | 3, 83) => {
                    let pitch = if staff == 2 { 64 } else { 59 };
                    format!("{}{}{}{}", rest("half"), rest("quarter"), chord("eighth", &[pitch], &lyric(0, "mes", ""), ""), chord("eighth", &[pitch], "", if staff == 2 { TIE_NEXT_BAR } else { "" }))
                }
                (2, 84) => chord("eighth", &[64], "", TIE_PREVIOUS_BAR),
                _ => String::new(),
            };
            xml.push_str(&body);
            xml.push_str("</voice></Measure>");
        }
        xml.push_str("</Staff>");
    }
    xml.push_str("</Score></museScore>");
    xml
}

#[derive(Debug)]
struct OriginalNote {
    id: String,
    track_id: String,
    staff: String,
    on_event: String,
    off_event: String,
    onset: u32,
    duration: u32,
    pitch: Option<u8>,
    occurrence: u32,
    source: NoteSource,
    on_order: u32,
    off_order: u32,
    lyrics: Vec<Lyric>,
}

/// Build an oracle from adapter identity and explicit note-off references.
/// Never infer the original identity from the destination lane or geometry.
fn originals(midi: &Midi) -> BTreeMap<String, OriginalNote> {
    let mut result = BTreeMap::new();
    for track in &midi.tracks {
        for event in &track.events {
            let Kind::NoteOn(note) = &event.kind else {
                continue;
            };
            let off = track.events.iter().filter(|off| off.tick >= event.tick).find(|off| {
                matches!(&off.kind, Kind::NoteOff(end) if end.source_id.as_deref() == Some(note.source.id.as_str()))
            }).expect("source-owned note-off");
            let id = format!(
                "note:{}:{}:occurrence:{}:event:{}",
                track.id, note.source.id, note.source.occurrence, event.order
            );
            let original = OriginalNote {
                id: id.clone(),
                track_id: track.id.clone(),
                staff: note.source.staff_id.clone().expect("score staff"),
                on_event: format!("event:{}:{}", track.id, event.order),
                off_event: format!("event:{}:{}", track.id, off.order),
                onset: event.tick,
                duration: off.tick - event.tick,
                pitch: note.key,
                occurrence: note.source.occurrence,
                source: note.source.clone(),
                on_order: event.order,
                off_order: off.order,
                lyrics: note.lyrics.clone(),
            };
            assert!(
                result.insert(id, original).is_none(),
                "duplicate adapter identity"
            );
        }
    }
    result
}

fn original_at<'a>(
    originals: &'a BTreeMap<String, OriginalNote>,
    staff: &str,
    onset: u32,
    pitch: u8,
) -> &'a OriginalNote {
    let matches: Vec<_> = originals
        .values()
        .filter(|note| note.staff == staff && note.onset == onset && note.pitch == Some(pitch))
        .collect();
    assert_eq!(matches.len(), 1, "fixture address {staff}/{onset}/{pitch}");
    matches[0]
}

fn retained<'a>(
    project: &'a ProjectedProject,
    id: &str,
) -> (&'a ProjectedTrack, &'a ProjectedNote) {
    let matches: Vec<_> = project
        .tracks
        .iter()
        .flat_map(|lane| lane.notes.iter().map(move |note| (lane, note)))
        .filter(|(_, note)| {
            note.source_evidence
                .as_ref()
                .is_some_and(|evidence| evidence.note_id == id)
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "exactly one editable representation for {id}"
    );
    matches[0]
}

fn raw_lyric(note: &ProjectedNote) -> Option<&str> {
    match &note.lyric {
        ProjectedLyric::Source(source)
        | ProjectedLyric::Pronounced { source, .. }
        | ProjectedLyric::PronouncedSplit { source } => Some(&source.raw),
        _ => None,
    }
}

/// Disabling only the new typed proof exercises the unchanged legacy projection
/// path. Compare every previous identity and payload, not merely total counts.
fn assert_only_proven_delta(
    bytes: &[u8],
    after: &ProjectedProject,
    target: ExportTarget,
    profile: PronunciationProfile,
    added: BTreeSet<String>,
) {
    let mut legacy = musescore::parse(bytes).unwrap();
    for track in &mut legacy.tracks {
        for event in &mut track.events {
            if let Kind::NoteOn(note) = &mut event.kind {
                note.source.continuity = None;
            }
        }
    }
    let baseline = convert_midi_with_profile(&legacy, "french", None, target, profile);
    assert!(
        baseline.ok,
        "legacy projection baseline: {:?}",
        baseline.msg
    );
    let notes_by_identity = |project: &ProjectedProject| {
        let mut notes = BTreeMap::new();
        for track in &project.tracks {
            for original in &track.notes {
                let mut note = original.clone();
                let evidence = note.source_evidence.as_mut().unwrap();
                let id = evidence.note_id.clone();
                let origin = evidence.origin.as_mut().unwrap();
                origin.source.continuity = None;
                origin.continuation = None;
                // This supplemental no-routing control removes typed tie proof.
                // A dormant, source-owned Score context may consequently be absent;
                // it is not an authored contributor and must produce no automation.
                if note.performance.as_ref().is_some_and(|p| {
                    matches!(
                        p.owner,
                        verse_lib::engine::performance::PerformanceOwner::Score { .. }
                    ) && p.intensity.as_ref().is_some_and(|i| {
                        use verse_lib::engine::score_intensity::{intersects, Curve, Fraction};
                        let start = Fraction::new(
                            i64::from(note.onset_ticks),
                            i64::from(project.ticks_per_beat),
                        )
                        .unwrap();
                        let end = Fraction::new(
                            i64::from(note.onset_ticks) + i64::from(note.duration_ticks),
                            i64::from(project.ticks_per_beat),
                        )
                        .unwrap();
                        i.provenance.is_none()
                            && i.issues.is_empty()
                            && i.timeline.segments.iter().all(|s| {
                                !intersects(s.start, s.end, start, end, true)
                                    || matches!(s.curve, Curve::Absent)
                            })
                            && i.timeline
                                .issues
                                .iter()
                                .all(|issue| !intersects(issue.start, issue.end, start, end, true))
                    })
                }) {
                    note.performance = None;
                }
                assert!(notes
                    .insert(id, (track.source_track_id.clone(), note))
                    .is_none());
            }
        }
        notes
    };
    let before = notes_by_identity(baseline.svp.as_ref().unwrap());
    let after = notes_by_identity(after);
    for (id, note) in &before {
        assert_eq!(
            after.get(id),
            Some(note),
            "previous retained identity changed: {id}"
        );
    }
    assert_eq!(
        after
            .keys()
            .filter(|id| !before.contains_key(*id))
            .cloned()
            .collect::<BTreeSet<_>>(),
        added
    );
}

fn assert_hold(
    project: &ProjectedProject,
    originals: &BTreeMap<String, OriginalNote>,
    staff: &str,
    head_tick: u32,
    tail_tick: u32,
    pitch: u8,
    destination: &str,
) {
    let head = original_at(originals, staff, head_tick, pitch);
    let tail = original_at(originals, staff, tail_tick, pitch);
    let (head_lane, head_note) = retained(project, &head.id);
    let (tail_lane, tail_note) = retained(project, &tail.id);
    assert_eq!(head_lane.source_track_id, destination);
    assert_eq!(tail_lane.source_track_id, destination);
    assert_eq!(
        head_note.onset_ticks + head_note.duration_ticks,
        tail_note.onset_ticks
    );
    assert!(matches!(tail_note.lyric, ProjectedLyric::Extension));
    assert!(head_note.lyric.is_sung());
    assert!(!matches!(head_note.lyric, ProjectedLyric::Extension));
    assert_eq!(
        head.duration, 240,
        "do not merge the text-bearing tail into its head"
    );
    assert_eq!(tail_note.duration_ticks, tail.duration);
    let link = tail_note
        .source_evidence
        .as_ref()
        .unwrap()
        .origin
        .as_ref()
        .unwrap()
        .continuation
        .as_ref()
        .expect("validated source continuation owner");
    assert_eq!(link.predecessor_id, head.id);
    let selected_owner = head
        .lyrics
        .iter()
        .find(|lyric| lyric.verse == 1 && !lyric.raw.trim().is_empty())
        .expect("source verse-one sung owner");
    assert_eq!(link.lyric_owner_id, selected_owner.id);
    assert_eq!(raw_lyric(head_note), Some(selected_owner.raw.as_str()));
    assert_eq!(link.destination_track_id, destination);
}

fn assert_recoveries(project: &ProjectedProject, originals: &BTreeMap<String, OriginalNote>) {
    assert_hold(project, originals, "2", 19_440, 19_680, 68, ALTI_HIGH);
    assert_hold(project, originals, "1", 38_160, 38_400, 64, SOP);
    let endpoint = original_at(originals, "2", 19_680, 68);
    assert_eq!(
        endpoint.track_id, ALTI_MAIN,
        "origin differs from destination"
    );
    assert_eq!(endpoint.occurrence, 0);
    let low = original_at(originals, "2", 19_440, 59);
    let high = original_at(originals, "2", 19_440, 68);
    assert_eq!(
        low.lyrics[0].id, high.lyrics[0].id,
        "one shared source lyric owner"
    );
    assert_eq!(high.lyrics[0].extend_ticks, Some(240));
    assert_eq!(endpoint.lyrics.len(), 1);
    assert_eq!(
        (endpoint.lyrics[0].verse, endpoint.lyrics[0].raw.as_str()),
        (2, "ger")
    );
    retained(project, &low.id);
    for (staff, onset, pitch, text, lane) in [
        ("2", 75_360, 68, "ger", ALTI_MAIN),
        ("1", 93_840, 64, "cou", SOP),
        ("1", 94_080, 64, "rants", SOP),
    ] {
        let original = original_at(originals, staff, onset, pitch);
        let (actual_lane, note) = retained(project, &original.id);
        assert_eq!(original.occurrence, 1);
        assert_eq!(
            actual_lane.source_track_id, lane,
            "eligible pass-two syllable keeps its lane"
        );
        assert_eq!(raw_lyric(note), Some(text));
        assert!(!matches!(note.lyric, ProjectedLyric::Extension));
    }
    for pitch in [59, 68] {
        let head = original_at(originals, "2", 75_120, pitch);
        assert_eq!(raw_lyric(retained(project, &head.id).1), Some("chan"));
    }
    let before = original_at(originals, "2", 17_760, 59);
    assert_eq!(before.onset + before.duration, 18_240);
    assert_eq!(
        retained(project, &before.id).1.duration_ticks,
        480,
        "do not heal the 1440-tick main-lane gap"
    );
}

fn assert_serialized(target: ExportTarget, project: &ProjectedProject) -> Vec<u8> {
    target::validate_for(target, project).expect("analysis accepts the exact projection");
    let bytes = target::serialize_to(target, project).expect("write agrees with analysis");
    match target {
        ExportTarget::Ustx => {
            let saved = target::ustx::serialize(project).unwrap();
            assert_eq!(bytes, target::ustx::to_yaml(&saved).as_bytes());
            assert_eq!(saved.voice_parts.len(), project.tracks.len());
            for (lane, part) in project.tracks.iter().zip(&saved.voice_parts) {
                assert_eq!(part.notes.len(), lane.notes.len());
                for (note, emitted) in lane.notes.iter().zip(&part.notes) {
                    assert_eq!(
                        (
                            i64::from(part.position) + i64::from(emitted.position),
                            emitted.duration,
                            emitted.tone
                        ),
                        (
                            i64::from(note.onset_ticks),
                            i32::try_from(note.duration_ticks).unwrap(),
                            note.pitch
                        )
                    );
                    assert!(!emitted.lyric.is_empty());
                    if matches!(note.lyric, ProjectedLyric::Extension) {
                        assert_eq!(emitted.lyric, "+~");
                    }
                    if matches!(note.lyric, ProjectedLyric::PronouncedSplit { .. }) {
                        assert_eq!(emitted.lyric, "+");
                    }
                }
            }
        }
        ExportTarget::Svp => {
            let saved: serde_json::Value = serde_json::from_slice(&bytes).expect("saved JSON");
            let tracks = saved["tracks"].as_array().unwrap();
            assert_eq!(tracks.len(), project.tracks.len());
            let scale = target::svp::BLICKS_PER_QUARTER / 480;
            for (lane, track) in project.tracks.iter().zip(tracks) {
                let notes = track["mainGroup"]["notes"].as_array().unwrap();
                assert_eq!(notes.len(), lane.notes.len());
                for (note, emitted) in lane.notes.iter().zip(notes) {
                    assert_eq!(
                        emitted["onset"].as_u64(),
                        Some(u64::from(note.onset_ticks) * scale)
                    );
                    assert_eq!(
                        emitted["duration"].as_u64(),
                        Some(u64::from(note.duration_ticks) * scale)
                    );
                    assert_eq!(emitted["pitch"].as_u64(), Some(u64::from(note.pitch)));
                    if matches!(note.lyric, ProjectedLyric::Extension) {
                        assert_eq!(emitted["lyrics"], "-");
                    }
                }
            }
        }
    }
    bytes
}

fn assert_evidence_and_ledger(midi: &Midi, outcome: &ConvertOutcome, target: ExportTarget) {
    let project = outcome.svp.as_ref().unwrap();
    let originals = originals(midi);
    let mut retained_ids = BTreeSet::new();
    for note in project.tracks.iter().flat_map(|lane| &lane.notes) {
        let evidence = note
            .source_evidence
            .as_ref()
            .expect("original evidence travels with every note");
        let original = &originals[&evidence.note_id];
        assert_eq!(
            (note.onset_ticks, note.duration_ticks, Some(note.pitch)),
            (original.onset, original.duration, original.pitch),
            "{}",
            original.id
        );
        assert_eq!(evidence.note_on_event_id, original.on_event);
        assert_eq!(evidence.note_off_event_id, original.off_event);
        let origin = evidence
            .origin
            .as_ref()
            .expect("universal original source metadata");
        assert_eq!(origin.track_id, original.track_id);
        assert_eq!(origin.note_on_order, original.on_order);
        assert_eq!(origin.note_off_order, original.off_order);
        assert_eq!(
            origin.source, original.source,
            "routing does not replace original staff/voice/occurrence/link evidence"
        );

        assert!(
            retained_ids.insert(evidence.note_id.clone()),
            "no duplicate endpoint"
        );
        for id in evidence.source_ids() {
            assert!(
                outcome.projection.source_ids.contains(id),
                "missing retained {id}"
            );
        }
    }
    let claimed_notes: BTreeSet<_> = outcome
        .projection
        .source_ids
        .iter()
        .filter(|id| id.starts_with("note:"))
        .cloned()
        .collect();
    assert_eq!(
        claimed_notes, retained_ids,
        "FID001 describes final editable identities"
    );
    let stems = StemPlan::from_source(midi, &outcome.tracks).expect("unchanged source stem plan");
    let note_bearing_parts: BTreeSet<_> = originals
        .values()
        .map(|note| note.source.part_id.as_ref().expect("original score Part"))
        .collect();
    assert_eq!(
        stems.stems.len(),
        note_bearing_parts.len(),
        "every original note-bearing Part keeps its stem"
    );
    let layout = BundleLayout::new(
        Path::new("/tmp/fid002.versebundle"),
        "synthetic.mscx",
        target,
    )
    .unwrap();
    let ledger = build_preservation_ledger(midi, &outcome.projection, &layout, &stems);
    let allowed: BTreeSet<_> = std::iter::once(layout.source_relative_path.clone())
        .chain(std::iter::once(layout.project_relative_path.clone()))
        .chain(
            stems
                .stems
                .iter()
                .map(|stem| layout.stem_audio_relative_path(stem)),
        )
        .collect();
    ledger
        .validate(&allowed)
        .expect("actual builder produces a complete valid ledger");
    assert_eq!(
        ledger
            .entries
            .iter()
            .filter(|entry| entry.item_kind == SourceItemKind::Note)
            .count(),
        originals.len()
    );
    let entries: BTreeMap<_, _> = ledger
        .entries
        .iter()
        .map(|entry| (entry.source_id.as_str(), entry))
        .collect();
    for original in originals.values() {
        let projected = retained_ids.contains(&original.id);
        let stem = stems
            .stems
            .iter()
            .find(|stem| stem.source_track_ids.contains(&original.track_id))
            .expect("original Part owns a stem");
        let mut expected_paths = vec![
            layout.source_relative_path.clone(),
            layout.stem_audio_relative_path(stem),
        ];
        if projected {
            expected_paths.push(layout.project_relative_path.clone());
        }
        expected_paths.sort();
        for id in [&original.id, &original.on_event, &original.off_event] {
            let entry = entries[id.as_str()];
            assert_eq!(
                entry.disposition,
                if projected {
                    PrimaryDisposition::ProjectedExact
                } else {
                    PrimaryDisposition::RenderedStem {
                        stem_id: stem.stem_id.clone(),
                    }
                },
                "{id}"
            );
            let mut paths = entry.artifact_paths.clone();
            paths.sort();
            assert_eq!(
                paths, expected_paths,
                "original Part, source and actual editable evidence for {id}"
            );
        }
    }
}

#[test]
fn musescore3_owned_continuity_survives_both_targets_and_supported_profiles() {
    for chant in [false, true] {
        let xml = continuity_score(chant);
        let midi = musescore::parse(xml.as_bytes()).expect("tiny MuseScore3 source");
        let before = format!("{midi:?}");
        let original = originals(&midi);
        assert_eq!(original.len(), 27);
        assert_eq!(
            (
                midi.topology.part_count(),
                midi.topology.staff_count(),
                midi.topology.voice_count()
            ),
            (3, 3, 3)
        );
        for (target, profile) in TARGET_PROFILES {
            let outcome = convert_midi_with_profile(&midi, "french", None, target, profile);
            assert!(
                outcome.ok,
                "chant={chant}, {target:?}/{profile:?}: {:?}",
                outcome.msg
            );
            let project = outcome.svp.as_ref().unwrap();
            assert_eq!(
                project.tracks.len(),
                5,
                "technical routing must not add a voice"
            );
            assert_eq!(
                project
                    .tracks
                    .iter()
                    .map(|lane| lane.notes.len())
                    .sum::<usize>(),
                21,
                "two PB recoveries; only Alti for chant, with no duplicate tie proof"
            );
            assert_eq!(project.monophony_violation(), None);
            let mut added = BTreeSet::from([original_at(&original, "2", 19_680, 68).id.clone()]);
            if !chant {
                added.insert(original_at(&original, "1", 38_400, 64).id.clone());
            }
            assert_only_proven_delta(xml.as_bytes(), project, target, profile, added);
            assert_eq!(project.ticks_per_beat, 480);
            assert_eq!(outcome.topology, midi.topology);
            assert!(
                outcome.projection.performance_spans.is_empty(),
                "no authored performance"
            );
            if target == ExportTarget::Ustx {
                assert!(target::ustx::serialize(project)
                    .unwrap()
                    .voice_parts
                    .iter()
                    .all(|part| part.curves.is_empty()));
            }
            assert_recoveries(project, &original);
            assert_evidence_and_ledger(&midi, &outcome, target);
            assert_serialized(target, project);
            assert_eq!(
                format!("{midi:?}"),
                before,
                "conversion never rewrites source IR"
            );
        }
    }
}

#[test]
fn five_untexted_variants_remain_source_and_stem_only() {
    let midi = musescore::parse(continuity_score(false).as_bytes()).unwrap();
    let original = originals(&midi);
    for (target, profile) in TARGET_PROFILES {
        let outcome = convert_midi_with_profile(&midi, "french", None, target, profile);
        assert!(outcome.ok, "{target:?}/{profile:?}: {:?}", outcome.msg);
        let project = outcome.svp.as_ref().unwrap();
        for (staff, onset, duration, pitch) in [
            ("2", 206_160, 240, 66),
            ("2", 214_800, 480, 64), // ordinary untexted tie remains merged
            ("3", 180_720, 480, 61), // real rest, no extension
            ("3", 206_160, 240, 57),
            ("3", 214_800, 240, 59),
        ] {
            let excluded = original_at(&original, staff, onset, pitch);
            assert_eq!(excluded.duration, duration);
            assert!(excluded.lyrics.is_empty());
            for id in [&excluded.id, &excluded.on_event, &excluded.off_event] {
                assert!(
                    !outcome.projection.source_ids.contains(id),
                    "untexted {id} is not editable"
                );
            }
            assert!(project
                .tracks
                .iter()
                .flat_map(|lane| &lane.notes)
                .all(|note| note.source_evidence.as_ref().unwrap().note_id != excluded.id));
        }
        let tail = original
            .values()
            .find(|note| note.staff == "2" && note.onset == 215_040)
            .unwrap();
        assert_eq!(
            tail.pitch, None,
            "do not globally unmerge correct bare ties"
        );
        assert!(!outcome.projection.source_ids.contains(&tail.id));
        assert_evidence_and_ledger(&midi, &outcome, target);
    }
}

#[test]
fn eligible_second_pass_keeps_existing_pronunciation_splits() {
    let xml = continuity_score(false);
    let midi = musescore::parse(xml.as_bytes()).unwrap();
    // Without ties the same eligible syllables already have their own attacks.
    // This reference preserves the exact verse selection and linguistic context.
    let untied = xml.replace(TIE_NEXT_BAR, "").replace(TIE_PREVIOUS_BAR, "");
    let reference = musescore::parse(untied.as_bytes()).unwrap();
    let source = originals(&midi);
    for (target, profile) in TARGET_PROFILES {
        let actual = convert_midi_with_profile(&midi, "french", None, target, profile);
        let baseline = convert_midi_with_profile(&reference, "french", None, target, profile);
        assert!(actual.ok, "{:?}", actual.msg);
        assert!(baseline.ok, "{:?}", baseline.msg);
        for tick in [93_840, 94_080] {
            let id = &original_at(&source, "1", tick, 64).id;
            let (_, actual_note) = retained(actual.svp.as_ref().unwrap(), id);
            let (_, baseline_note) = retained(baseline.svp.as_ref().unwrap(), id);
            assert_eq!(
                actual_note.lyric, baseline_note.lyric,
                "{target:?}/{profile:?}: an eligible word or split must not become a hold"
            );
            assert_eq!(
                (
                    actual_note.onset_ticks,
                    actual_note.duration_ticks,
                    actual_note.pitch
                ),
                (
                    baseline_note.onset_ticks,
                    baseline_note.duration_ticks,
                    baseline_note.pitch
                )
            );
        }
        assert_serialized(target, actual.svp.as_ref().unwrap());
    }
}

#[test]
fn routing_is_independent_of_adapter_lane_iteration_order() {
    for (target, profile) in TARGET_PROFILES {
        let mut midi = musescore::parse(continuity_score(false).as_bytes()).unwrap();
        let first = convert_midi_with_profile(&midi, "french", None, target, profile);
        assert!(first.ok, "{:?}", first.msg);
        midi.tracks.reverse();
        let reversed = convert_midi_with_profile(&midi, "french", None, target, profile);
        assert!(reversed.ok, "{:?}", reversed.msg);
        let by_lane = |project: &ProjectedProject| -> BTreeMap<String, Vec<ProjectedNote>> {
            project
                .tracks
                .iter()
                .map(|lane| (lane.source_track_id.clone(), lane.notes.clone()))
                .collect()
        };
        assert_eq!(
            by_lane(first.svp.as_ref().unwrap()),
            by_lane(reversed.svp.as_ref().unwrap())
        );
        assert_eq!(first.projection.source_ids, reversed.projection.source_ids);
        assert_serialized(target, reversed.svp.as_ref().unwrap());
    }
}

#[test]
fn a_recovered_endpoint_in_the_old_main_lane_still_fails_the_native_gap_gate() {
    let midi = musescore::parse(continuity_score(false).as_bytes()).unwrap();
    let outcome = convert_midi_with_profile(
        &midi,
        "french",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Default,
    );
    assert!(outcome.ok, "{:?}", outcome.msg);
    let mut project = outcome.svp.unwrap();
    let original = originals(&midi);
    let id = &original_at(&original, "2", 19_680, 68).id;
    let high = project
        .tracks
        .iter_mut()
        .find(|lane| lane.source_track_id == ALTI_HIGH)
        .unwrap();
    let index = high
        .notes
        .iter()
        .position(|note| note.source_evidence.as_ref().unwrap().note_id == *id)
        .expect("recovered endpoint");
    let tail = high.notes.remove(index);
    let main = project
        .tracks
        .iter_mut()
        .find(|lane| lane.source_track_id == ALTI_MAIN)
        .unwrap();
    main.notes.push(tail);
    main.notes.sort_by_key(|note| note.onset_ticks);
    // The identity gate rejects the misroute first, on both targets.
    for target in [ExportTarget::Ustx, ExportTarget::Svp] {
        let error = target::validate_for(target, &project).expect_err("wrong proven predecessor");
        assert!(
            error.contains("source continuation at tick 19680"),
            "{error}"
        );
        assert_eq!(
            target::serialize_to(target, &project)
                .unwrap_err()
                .message(),
            error
        );
    }
    // Also exercise the existing marker/gap refusal without a router record,
    // as an ordinary in-memory projection supplied directly to the writer.
    let endpoint = project
        .tracks
        .iter_mut()
        .flat_map(|lane| &mut lane.notes)
        .find(|note| note.source_evidence.as_ref().unwrap().note_id == *id)
        .unwrap();
    endpoint
        .source_evidence
        .as_mut()
        .unwrap()
        .origin
        .as_mut()
        .unwrap()
        .continuation = None;

    let error = target::validate_for(ExportTarget::Ustx, &project)
        .expect_err("1440-tick gap cannot bind Même");
    assert_eq!(error, "the note at MIDI tick 19680 on source track mscx:staff:2:voice:1 carries the marker \"+~\", which continues the previous note, but does not begin where that note ends; OpenUtau only carries a syllable across notes that touch and would sing the marker as a word instead");
    assert_eq!(
        target::serialize_to(ExportTarget::Ustx, &project)
            .unwrap_err()
            .message(),
        error
    );
}

#[test]
fn extension_endpoint_is_inclusive_and_sentinels_do_not_recover_later_chords() {
    for (extension, expected) in [
        ("<ticks>960</ticks>", 3),
        ("<ticks_f>1/2</ticks_f>", 3),
        ("", 1),
        ("<ticks>0</ticks>", 1),
        ("<ticks>1</ticks><ticks_f>1/1920</ticks_f>", 1),
    ] {
        let xml = format!(
            r#"<museScore version="3.02"><Score><Division>480</Division><Part><Staff id="1"/></Part><Staff id="1"><Measure><voice>{}{}{}{}</voice></Measure></Staff></Score></museScore>"#,
            chord("quarter", &[60], &lyric(0, "sing", extension), ""),
            chord("quarter", &[62], "", ""),
            chord("quarter", &[64], "", ""),
            chord("quarter", &[65], "", "")
        );
        let midi = musescore::parse(xml.as_bytes()).unwrap();
        for (target, profile) in TARGET_PROFILES {
            let outcome = convert_midi_with_profile(&midi, "english", None, target, profile);
            assert!(outcome.ok, "{extension}: {:?}", outcome.msg);
            let project = outcome.svp.as_ref().unwrap();
            let notes = &project.tracks[0].notes;
            assert_eq!(notes.len(), expected, "{extension}/{target:?}/{profile:?}");
            assert_eq!(
                notes
                    .iter()
                    .map(|note| note.onset_ticks)
                    .collect::<Vec<_>>(),
                (0..expected as u32)
                    .map(|index| index * 480)
                    .collect::<Vec<_>>()
            );
            assert!(notes.iter().all(|note| note.duration_ticks == 480));
            assert!(notes
                .iter()
                .skip(1)
                .all(|note| matches!(note.lyric, ProjectedLyric::Extension)));
            assert_serialized(target, project);
        }
    }
}

/// Explicit opt-in: emits only synthetic fixture exports for a separate native
/// consumer. This test never loads a singer, compiles a probe or renders audio.
#[test]
#[ignore = "requires VERSE_CONTINUITY_PROBE_DIR pointing to a new output directory"]
fn export_synthetic_continuity_for_native_consumer() {
    let directory = std::env::var("VERSE_CONTINUITY_PROBE_DIR")
        .expect("VERSE_CONTINUITY_PROBE_DIR is required");
    std::fs::create_dir(&directory).expect("use a new directory; never overwrite outputs");
    for chant in [false, true] {
        let midi = musescore::parse(continuity_score(chant).as_bytes()).unwrap();
        for (target, profile) in TARGET_PROFILES {
            let outcome = convert_midi_with_profile(&midi, "french", None, target, profile);
            assert!(outcome.ok, "{:?}", outcome.msg);
            let project = outcome.svp.as_ref().unwrap();
            assert_recoveries(project, &originals(&midi));
            let bytes = assert_serialized(target, project);
            let master = if chant { "chant" } else { "pb" };
            let profile_name = match profile {
                PronunciationProfile::Default => "default",
                PronunciationProfile::FrenchMillefeuille => "french",
                PronunciationProfile::EnglishArpabet => "english",
            };
            let path = Path::new(&directory)
                .join(format!("{master}-{profile_name}.{}", target.extension()));
            std::fs::write(path, bytes).unwrap();
        }
    }
}

/// These are private, hash-pinned inputs. Explicitly requesting this test makes
/// both masters mandatory; absence is never treated as a passing corpus gate.
#[test]
#[ignore = "requires VERSE_CONTINUITY_CORPUS_DIR with both original PB and chant MSCZ files"]
fn private_pb_and_chant_preserve_the_exact_audited_continuity_delta() {
    let directory = std::env::var("VERSE_CONTINUITY_CORPUS_DIR").expect(
        "VERSE_CONTINUITY_CORPUS_DIR must name the directory containing both original masters",
    );
    for (chant, filename, hash) in [
        (
            false,
            "Au-bout-de-mes-reves-Goldman-SAB PB.mscz",
            "d859cc506b7510283b193b78914095dc25718b856ccdd2aa0544edeb24564ba5",
        ),
        (
            true,
            "Au-bout-SAB chant.mscz",
            "efb13f3809c15d099d60967f0b71a51daa3b6c7b06f0508d03baf90fecebd873",
        ),
    ] {
        let path = Path::new(&directory).join(filename);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("required private master {}: {error}", path.display()));
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            hash,
            "wrong private master: {filename}"
        );
        let midi = musescore::parse(&bytes).expect("original master parses");
        let before = format!("{midi:?}");
        let source = originals(&midi);
        // These three configurations have audited counts. English is covered
        // by the synthetic matrix without inventing a private baseline for it.
        for (target, profile) in [
            (ExportTarget::Svp, PronunciationProfile::Default),
            (ExportTarget::Ustx, PronunciationProfile::Default),
            (ExportTarget::Ustx, PronunciationProfile::FrenchMillefeuille),
        ] {
            let outcome = convert_midi_with_profile(&midi, "french", None, target, profile);
            assert!(
                outcome.ok,
                "{filename}/{target:?}/{profile:?}: {:?}",
                outcome.msg
            );
            let project = outcome.svp.as_ref().unwrap();
            let expected = if chant {
                1566
            } else if profile == PronunciationProfile::FrenchMillefeuille {
                1561
            } else {
                1552
            };
            assert_eq!(
                project.tracks.len(),
                5,
                "{filename}: unchanged five vocal lanes"
            );
            assert_eq!(project.tracks.iter().map(|lane| lane.notes.len()).sum::<usize>(), expected,
                "{filename}/{target:?}/{profile:?}: rebaseline only with an explained identity-level delta");
            assert_eq!(outcome.topology, midi.topology);
            assert_eq!(project.monophony_violation(), None);
            let mut added = BTreeSet::from([original_at(&source, "2", 19_680, 68).id.clone()]);
            if !chant {
                added.insert(original_at(&source, "1", 38_400, 64).id.clone());
            }
            assert_only_proven_delta(&bytes, project, target, profile, added);

            assert_hold(project, &source, "2", 19_440, 19_680, 68, ALTI_HIGH);
            assert_hold(project, &source, "1", 38_160, 38_400, 64, SOP);
            let endpoint = original_at(&source, "2", 19_680, 68);
            assert_eq!(endpoint.id, "note:mscx:staff:2:voice:1:mscx:staff:2:measure:10:voice:0:chord:2:note:0:occurrence:0:event:18");
            assert_eq!(endpoint.duration, 240);
            assert_eq!(original_at(&source, "1", 38_400, 64).duration, 960);
            for (staff, onset, pitch, text, destination) in [
                (
                    "2",
                    75_120,
                    59,
                    "chan",
                    "mscx:staff:2:voice:1:polyphonic-member:1",
                ),
                ("2", 75_120, 68, "chan", ALTI_HIGH),
                ("2", 75_360, 68, "ger", ALTI_MAIN),
                ("1", 93_840, 64, "cou", SOP),
                ("1", 94_080, 64, if chant { "rant" } else { "rants" }, SOP),
            ] {
                let original = original_at(&source, staff, onset, pitch);
                let (lane, note) = retained(project, &original.id);
                assert_eq!(lane.source_track_id, destination);
                assert_eq!(raw_lyric(note), Some(text));
                assert!(!matches!(note.lyric, ProjectedLyric::Extension));
            }
            if !chant {
                for (staff, onset, duration, pitch) in [
                    ("2", 206_160, 240, 66),
                    ("2", 214_800, 480, 64),
                    ("3", 180_720, 480, 61),
                    ("3", 206_160, 240, 57),
                    ("3", 214_800, 240, 59),
                ] {
                    let excluded = original_at(&source, staff, onset, pitch);
                    assert_eq!(excluded.duration, duration);
                    assert!(excluded.lyrics.is_empty());
                    for id in [&excluded.id, &excluded.on_event, &excluded.off_event] {
                        assert!(
                            !outcome.projection.source_ids.contains(id),
                            "unproven note {id} stays source/stem-only"
                        );
                    }
                }
            }
            // Identity-keyed geometry checks cover every retained note, not
            // merely the count or the two recovered windows; the live builder
            // also verifies actual original-part artifact ownership.
            assert_evidence_and_ledger(&midi, &outcome, target);
            assert_serialized(target, project);
            assert_eq!(format!("{midi:?}"), before);
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "private source remains byte-identical"
        );
    }
}
