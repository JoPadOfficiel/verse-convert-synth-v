//! R10: compare against an independently compiled, hash-pinned work-start engine.
//! The immutable baseline executable uses its frozen copy of this capture code
//! and only the pre-FID002 parser/converter. This current bridge preserves that
//! historical nominal/MIDI schema. EXP003 Score bindings are checked separately
//! against untouched raw normalization, with proven tie inheritance and exact
//! selected-attack accent recovery checked separately;
//! they never rewrite the old goldens or replace them with a within-model control.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use verse_lib::engine::convert::convert_midi_with_profile;
use verse_lib::engine::midi::{Kind, Lyric, LyricState, Midi, NoteSource, SourceNoteRef};
use verse_lib::engine::musescore;
use verse_lib::engine::performance::{self, PerformanceIndex, PerformanceNote, PerformanceOwner};
use verse_lib::engine::projection::{ProjectedLyric, ProjectedProject};
use verse_lib::engine::score_intensity::{Fraction, NoteChain, NoteIntensity, ScoreVoice};
use verse_lib::engine::target::{ExportTarget, PronunciationProfile};

pub const MASTERS: [(&str, &str, &str); 2] = [
    (
        "pb",
        "Au-bout-de-mes-reves-Goldman-SAB PB.mscz",
        "d859cc506b7510283b193b78914095dc25718b856ccdd2aa0544edeb24564ba5",
    ),
    (
        "chant",
        "Au-bout-SAB chant.mscz",
        "efb13f3809c15d099d60967f0b71a51daa3b6c7b06f0508d03baf90fecebd873",
    ),
];

pub const CONFIGURATIONS: [(&str, ExportTarget, PronunciationProfile); 4] = [
    (
        "svp-default",
        ExportTarget::Svp,
        PronunciationProfile::Default,
    ),
    (
        "ustx-default",
        ExportTarget::Ustx,
        PronunciationProfile::Default,
    ),
    (
        "ustx-french",
        ExportTarget::Ustx,
        PronunciationProfile::FrenchMillefeuille,
    ),
    (
        "ustx-english",
        ExportTarget::Ustx,
        PronunciationProfile::EnglishArpabet,
    ),
];

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn lyric_record(lyric: &Lyric) -> Value {
    // Debug is restricted to stable shared enums; never stringify the growing
    // NoteSource/NoteEvidence structs or erase new fields from the current IR.
    json!({
        "id": lyric.id, "raw": lyric.raw, "raw_bytes": lyric.raw_bytes,
        "fragments": lyric.fragments.iter().map(|part| format!("{part:?}")).collect::<Vec<_>>(),
        "row": lyric.lane, "verse": lyric.verse, "verse_from_score": lyric.verse_from_score,
        "state": format!("{:?}", lyric.state), "syllabic": format!("{:?}", lyric.syllabic),
        "line_break": format!("{:?}", lyric.line_break), "time_only": lyric.time_only,
        "extension": format!("{:?}", lyric.extension), "extend_ticks": lyric.extend_ticks,
        "extend_fraction": lyric.extend_fraction,
    })
}

fn projected_lyric_record(lyric: &ProjectedLyric) -> Value {
    match lyric {
        ProjectedLyric::Absent => json!({"kind": "absent"}),
        ProjectedLyric::Extension => json!({"kind": "extension"}),
        ProjectedLyric::Source(source) => json!({"kind": "source", "source": lyric_record(source)}),
        ProjectedLyric::Pronounced {
            source,
            text,
            phonemes,
        } => json!({
            "kind": "pronounced", "source": lyric_record(source), "text": text, "phonemes": phonemes,
        }),
        ProjectedLyric::PronouncedSplit { source } => json!({
            "kind": "pronounced_split", "source": lyric_record(source),
        }),
    }
}

pub fn source_records(midi: &Midi) -> BTreeMap<String, Value> {
    let mut notes = BTreeMap::new();
    for track in &midi.tracks {
        for (index, event) in track.events.iter().enumerate() {
            let Kind::NoteOn(note) = &event.kind else {
                continue;
            };
            // Score adapters have explicit source-owned NoteOff IDs. This is
            // the original parser event stream, not a geometry-based join.
            let (off, end) = track.events[index + 1..]
                .iter()
                .find_map(|candidate| match &candidate.kind {
                    Kind::NoteOff(end)
                        if end.source_id.as_deref() == Some(note.source.id.as_str()) =>
                    {
                        Some((candidate, end))
                    }
                    _ => None,
                })
                .expect("every original score NoteOn has its source-owned NoteOff");
            let source = &note.source;
            let id = format!(
                "note:{}:{}:occurrence:{}:event:{}",
                track.id, source.id, source.occurrence, event.order
            );
            let record = json!({
                "id": id, "track_id": track.id, "source_id": source.id,
                "on_event_id": format!("event:{}:{}", track.id, event.order),
                "off_event_id": format!("event:{}:{}", track.id, off.order),
                "on_order": event.order, "off_order": off.order,
                "onset": event.tick, "duration": off.tick.checked_sub(event.tick).expect("nonnegative source duration"),
                "pitch": note.key, "channel": note.channel, "velocity": note.velocity,
                "off_pitch": end.key, "off_channel": end.channel, "off_velocity": end.velocity,
                "part": source.part_id, "staff": source.staff_id, "voice": source.voice,
                "chord": source.chord_id, "instrument": source.instrument_id,
                "occurrence": source.occurrence, "measure": source.measure, "grace": source.grace,
                "unpitched": format!("{:?}", source.unpitched),
                "lyrics": note.lyrics.iter().map(lyric_record).collect::<Vec<_>>(),
            });
            assert!(
                notes.insert(id, record).is_none(),
                "duplicate original source identity"
            );
        }
    }
    notes
}

pub fn project_record(project: &ProjectedProject) -> Value {
    let mut notes: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut tracks = Vec::new();
    let mut arcs: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (track_index, track) in project.tracks.iter().enumerate() {
        tracks.push(json!({"source_track_id": track.source_track_id, "name": track.name, "muted": track.muted}));
        for (note_index, note) in track.notes.iter().enumerate() {
            let evidence = note
                .source_evidence
                .as_ref()
                .expect("real retained note has FID001 evidence");
            // Keep every legacy MIDI field and its sharing groups comparable to
            // the actual work-start executable. Score had no old payload; its
            // full current binding is independently checked before this capture.
            let performance = note.performance.as_ref().and_then(|binding| {
                let PerformanceOwner::Midi { key, timeline } = &binding.owner else {
                    return None;
                };
                arcs.entry(std::sync::Arc::as_ptr(timeline) as usize).or_default()
                    .push(format!("{}@{track_index}:{note_index}", evidence.note_id));
                let points = |points: &[verse_lib::engine::performance::HeldPoint]| {
                    points.iter().map(|point| json!({"tick": point.tick, "value": point.value, "source_ids": point.source_ids})).collect::<Vec<_>>()
                };
                Some(json!({
                    "source_id": binding.source_id,
                    "key": {"port": key.port, "channel": key.channel},
                    "pitch_cents": points(&timeline.pitch_cents),
                    "linear_gain": points(&timeline.linear_gain),
                    "issues": timeline.issues.iter().map(|issue| json!({
                        "tick": issue.tick, "dimension": format!("{:?}", issue.dimension),
                        "source_ids": issue.source_ids, "reason": issue.reason,
                    })).collect::<Vec<_>>(),
                }))
            });
            notes.entry(evidence.note_id.clone()).or_default().push(json!({
                "track_index": track_index, "source_track_id": track.source_track_id,
                "onset": note.onset_ticks, "duration": note.duration_ticks, "pitch": note.pitch,
                "on_event_id": evidence.note_on_event_id, "off_event_id": evidence.note_off_event_id,
                "lyric_id": evidence.lyric_id, "lyric_event_id": evidence.lyric_event_id,
                "lyric": projected_lyric_record(&note.lyric), "performance": performance,
            }));
        }
    }
    let mut aliases: Vec<_> = arcs.into_values().collect();
    for group in &mut aliases {
        group.sort();
    }
    aliases.sort();
    json!({
        "ticks_per_beat": project.ticks_per_beat, "language": project.language,
        "profile": format!("{:?}", project.pronunciation_profile),
        "meters": project.meters.iter().map(|meter| json!({"bar_index": meter.bar_index, "numerator": meter.numerator, "denominator": meter.denominator})).collect::<Vec<_>>(),
        "tempos": project.tempos.iter().map(|tempo| json!({"tick": tempo.tick, "bpm": tempo.bpm, "source": tempo.source, "discovery_index": tempo.discovery_index})).collect::<Vec<_>>(),
        "tracks": tracks, "notes": notes, "performance_alias_groups": aliases,
    })
}

fn matches_source(reference: &SourceNoteRef, source: &NoteSource) -> bool {
    reference.source_id == source.id
        && reference.occurrence == source.occurrence
        && source
            .continuity
            .as_ref()
            .is_some_and(|continuity| reference.playback_segment == continuity.playback_segment)
}

fn sung_owner(lyric: &ProjectedLyric) -> Option<&Lyric> {
    let source = match lyric {
        ProjectedLyric::Source(source)
        | ProjectedLyric::Pronounced { source, .. }
        | ProjectedLyric::PronouncedSplit { source } => source,
        _ => return None,
    };
    matches!(&source.state, LyricState::Text(text) if !text.trim().is_empty()).then_some(source)
}

fn score_voice(source: &NoteSource) -> ScoreVoice {
    ScoreVoice {
        part: source.part_id.clone().unwrap_or_default(),
        staff: source.staff_id.clone().unwrap_or_default(),
        voice: source.voice.clone().unwrap_or_default(),
        instrument: source.instrument_id.clone(),
    }
}

// Determine exclusions from parser ties and the selected lyric/predecessor chain,
// never from the candidate's performance source_id or its rebinding result.
fn proven_selected_tails(
    midi: &Midi,
    track: &verse_lib::engine::projection::ProjectedTrack,
) -> BTreeSet<(String, u32)> {
    let mut excluded = BTreeSet::new();
    let mut roots: BTreeMap<
        &str,
        (
            &verse_lib::engine::projection::ProjectedNote,
            Option<String>,
        ),
    > = BTreeMap::new();
    for note in &track.notes {
        let evidence = note.source_evidence.as_ref().unwrap();
        let origin = evidence.origin.as_ref().unwrap();
        let event = midi
            .tracks
            .iter()
            .find(|t| t.id == origin.track_id)
            .unwrap()
            .events
            .iter()
            .find(|e| e.order == origin.note_on_order)
            .unwrap();
        let Kind::NoteOn(source) = &event.kind else {
            panic!("original NoteOn");
        };
        assert_eq!(
            source.source, origin.source,
            "original source metadata changed"
        );
        let root = if let Some(link) = &origin.continuation {
            let (head, root) = roots
                .get(link.predecessor_id.as_str())
                .expect("earlier selected predecessor");
            assert_eq!(root.as_deref(), Some(link.lyric_owner_id.as_str()));
            assert_eq!(link.destination_track_id, track.source_track_id);
            assert_eq!(
                head.onset_ticks.checked_add(head.duration_ticks),
                Some(note.onset_ticks)
            );
            assert!(!origin.lyric_conflict);
            assert!(matches!(
                note.lyric,
                ProjectedLyric::Extension | ProjectedLyric::PronouncedSplit { .. }
            ));
            if let Some(tie) = source
                .source
                .continuity
                .as_ref()
                .and_then(|c| c.incoming_tie.as_ref())
            {
                let head_source = &head
                    .source_evidence
                    .as_ref()
                    .unwrap()
                    .origin
                    .as_ref()
                    .unwrap()
                    .source;
                assert!(matches_source(&tie.tail, &source.source));
                assert!(matches_source(&tie.head, head_source));
                assert_eq!(tie.contact_tick, event.tick);
                assert_eq!(tie.contact_tick, note.onset_ticks);
                assert_eq!(source.key, Some(tie.pitch));
                assert_eq!(note.pitch, tie.pitch);
                assert_eq!(head.pitch, tie.pitch);
                assert_eq!(score_voice(&source.source), score_voice(head_source));
                assert_eq!(source.source.occurrence, head_source.occurrence);
                assert_eq!(
                    source.source.continuity.as_ref().unwrap().playback_segment,
                    head_source.continuity.as_ref().unwrap().playback_segment
                );
                excluded.insert((origin.track_id.clone(), origin.note_on_order));
            }
            root.clone()
        } else {
            sung_owner(&note.lyric).map(|l| l.id.clone())
        };
        // Pronunciation may split one original note; retain its final touching
        // representation here. The full validator below checks each separately.
        roots.insert(&evidence.note_id, (note, root));
    }
    excluded
}

fn selected_attack_binding(
    midi: &Midi,
    raw: &PerformanceIndex,
    lookup: &(String, u32),
    excluded: &BTreeSet<(String, u32)>,
) -> Option<PerformanceNote> {
    let original = raw.bindings.get(lookup);
    if excluded.is_empty() || original.is_some_and(|b| b.channel_key().is_some()) {
        return original.cloned();
    }
    let source_track = midi.tracks.iter().find(|t| t.id == lookup.0).unwrap();
    let event = source_track
        .events
        .iter()
        .find(|e| e.order == lookup.1)
        .unwrap();
    let Kind::NoteOn(note) = &event.kind else {
        panic!("source attack");
    };
    let owner = score_voice(&note.source);
    let Some(timeline) = raw
        .bindings
        .values()
        .find_map(|b| match &b.owner {
            PerformanceOwner::Score { voice } if voice == &owner => {
                b.intensity.as_ref().map(|i| i.timeline.clone())
            }
            _ => None,
        })
        .filter(|t| !t.accents.is_empty())
    else {
        return original.cloned();
    };
    let input = midi.score_intensity.as_ref().unwrap();
    let time = |t: u32| Fraction::new(i64::from(t), i64::from(midi.ticks_per_beat)).unwrap();
    // Pair source-owned NoteOff identity, never projected duration or lane.
    let end = source_track
        .events
        .iter()
        .find(|e| {
            e.order > event.order
                && matches!(&e.kind,
        Kind::NoteOff(off) if off.source_id.as_deref() == Some(note.source.id.as_str()))
        })
        .expect("original source NoteOff")
        .tick;
    let (start, end) = (time(event.tick), time(end));
    let run = input
        .runs
        .get(&(owner.part.clone(), owner.staff.clone()))
        .into_iter()
        .flatten()
        .rfind(|r| r.performed_start <= start);
    let source_id = format!(
        "note:{}:{}:occurrence:{}:event:{}",
        lookup.0, note.source.id, note.source.occurrence, lookup.1
    );
    let continuation_velocities = input
        .ties
        .iter()
        .filter(|((_, occurrence, tick), head)| {
            **head == note.source.id
                && *occurrence == note.source.occurrence
                && time(*tick) >= start
                && time(*tick) < end
        })
        .filter_map(|((tail, _, _), _)| input.overrides.get(tail).cloned())
        .collect();
    let chain = NoteChain {
        source_id: source_id.clone(),
        start,
        end,
        owner: owner.clone(),
        occurrence: run.map_or(0, |r| r.ordinal),
        pass: run.map_or(note.source.occurrence + 1, |r| r.pass),
        velocity: input.overrides.get(&note.source.id).cloned(),
        continuation_velocities,
    };
    let mut attacks = BTreeSet::new();
    let mut selected = BTreeSet::new();
    for track in &midi.tracks {
        for event in &track.events {
            if let Kind::NoteOn(n) = &event.kind {
                if n.key.is_some() && score_voice(&n.source) == owner {
                    attacks.insert(time(event.tick));
                    if !excluded.contains(&(track.id.clone(), event.order)) {
                        selected.insert(time(event.tick));
                    }
                }
            }
        }
    }
    if attacks == selected {
        return original.cloned();
    }
    let attacks: Vec<_> = attacks.into_iter().collect();
    let selected: Vec<_> = selected.into_iter().collect();
    // The neutral constructor consumes independently proven source attacks. It
    // does not invoke bind_selected_score_attacks or repeat normalization with
    // the candidate's exclusion set as an oracle.
    let before = NoteIntensity::resolve(&chain, timeline.clone(), &attacks).unwrap();
    if let Some(binding) = original {
        assert_eq!(binding.source_id, source_id);
        assert_eq!(
            binding.owner,
            PerformanceOwner::Score {
                voice: owner.clone()
            }
        );
        assert_eq!(
            binding.intensity.as_deref(),
            Some(&before),
            "raw attack reconstruction differs"
        );
    }
    let after = NoteIntensity::resolve(&chain, timeline, &selected).unwrap();
    if before == after {
        return original.cloned();
    }
    // Every admitted change must actually recover a source accent at this attack.
    assert!(!after.timeline.accents.is_empty());
    Some(PerformanceNote {
        source_id,
        owner: PerformanceOwner::Score { voice: owner },
        intensity: Some(std::sync::Arc::new(after)),
    })
}

/// EXP003 has no historical Score payload to compare. Check every current
/// binding by original source lookup, independently of the converter's output.
/// Only intensity may differ, by exact tie inheritance or source accent recovery
/// after raw incoming-tie and final owner-chain proof. MIDI never uses that rule.
type CheckedBinding = (usize, Option<PerformanceNote>, Option<String>);

pub fn validate_current_performance(
    midi: &Midi,
    raw: &PerformanceIndex,
    project: &ProjectedProject,
) -> Value {
    let source_events: BTreeMap<_, _> = midi
        .tracks
        .iter()
        .flat_map(|track| {
            track.events.iter().filter_map(move |event| {
                if let Kind::NoteOn(note) = &event.kind {
                    Some(((track.id.clone(), event.order), (event.tick, note)))
                } else {
                    None
                }
            })
        })
        .collect();
    let mut unchanged = 0;
    let mut inherited = 0;
    let mut score_bindings = 0;
    let mut selected_recoveries = 0;
    for track in &project.tracks {
        let excluded = proven_selected_tails(midi, track);
        // Store only already validated contexts; repeated projected IDs (e.g.
        // pronunciation splits) remain separate representations, never overwritten.
        let mut checked: BTreeMap<&str, Vec<CheckedBinding>> = BTreeMap::new();
        for (note_index, note) in track.notes.iter().enumerate() {
            let evidence = note
                .source_evidence
                .as_ref()
                .expect("retained source evidence");
            let origin = evidence
                .origin
                .as_ref()
                .expect("retained original ownership");
            let lookup = (origin.track_id.clone(), origin.note_on_order);
            let (source_onset, source_note) = source_events
                .get(&lookup)
                .expect("performance lookup must identify an original NoteOn");
            assert_eq!(
                origin.source, source_note.source,
                "projected source ownership changed"
            );
            let original = raw.bindings.get(&lookup);
            let mut expected = selected_attack_binding(midi, raw, &lookup, &excluded);
            if origin.continuation.is_none() && expected.as_ref() != original {
                selected_recoveries += 1;
            }
            let root = if let Some(link) = &origin.continuation {
                assert_eq!(link.destination_track_id, track.source_track_id);
                assert!(!origin.lyric_conflict);
                assert!(matches!(
                    note.lyric,
                    ProjectedLyric::Extension | ProjectedLyric::PronouncedSplit { .. }
                ));
                let predecessors = checked
                    .get(link.predecessor_id.as_str())
                    .expect("continuation predecessor must already be validated in this lane");
                let mut touching = predecessors.iter().filter(|(index, _, _)| {
                    let head = &track.notes[*index];
                    head.onset_ticks.checked_add(head.duration_ticks) == Some(note.onset_ticks)
                });
                let (head_index, head_binding, head_root) =
                    touching.next().expect("touching predecessor");
                assert!(
                    touching.next().is_none(),
                    "ambiguous predecessor representation"
                );
                assert_eq!(
                    head_root.as_deref(),
                    Some(link.lyric_owner_id.as_str()),
                    "continuation root lyric changed"
                );
                let head = &track.notes[*head_index];
                let head_origin = head
                    .source_evidence
                    .as_ref()
                    .unwrap()
                    .origin
                    .as_ref()
                    .unwrap();
                // Derive the expectation even when the candidate equals the raw
                // tail. Candidate equality must never suppress the tie oracle.
                // An extension without this original incoming tie keeps its own
                // binding: lyric continuity alone is not intensity inheritance.
                if let Some((continuity, tie)) = source_note
                    .source
                    .continuity
                    .as_ref()
                    .and_then(|c| c.incoming_tie.as_ref().map(|tie| (c, tie)))
                {
                    assert!(matches_source(&tie.tail, &source_note.source));
                    assert!(matches_source(&tie.head, &head_origin.source));
                    assert_eq!(tie.contact_tick, *source_onset);
                    assert_eq!(tie.contact_tick, note.onset_ticks);
                    assert_eq!(source_note.key, Some(tie.pitch));
                    assert_eq!(note.pitch, tie.pitch);
                    assert_eq!(head.pitch, tie.pitch);
                    assert_eq!(source_note.source.part_id, head_origin.source.part_id);
                    assert_eq!(source_note.source.staff_id, head_origin.source.staff_id);
                    assert_eq!(source_note.source.voice, head_origin.source.voice);
                    assert_eq!(source_note.source.occurrence, head_origin.source.occurrence);
                    assert_eq!(
                        continuity.playback_segment,
                        head_origin
                            .source
                            .continuity
                            .as_ref()
                            .unwrap()
                            .playback_segment
                    );
                    match (expected.as_mut(), head_binding.as_ref()) {
                        (Some(tail), Some(head))
                            if matches!(
                                (&tail.owner, &head.owner),
                                (
                                    PerformanceOwner::Score { .. },
                                    PerformanceOwner::Score { .. }
                                )
                            ) =>
                        {
                            assert_eq!(tail.owner, head.owner, "tie source owners differ");
                            let head_intensity = head
                                .intensity
                                .as_deref()
                                .expect("tie requires a validated head intensity context");
                            // Independent of continued_from: that helper must
                            // never be both implementation and provenance oracle.
                            assert_eq!(
                                note.performance
                                    .as_ref()
                                    .and_then(|p| p.intensity.as_ref())
                                    .and_then(|i| i.provenance.as_ref()),
                                head_intensity.provenance.as_ref(),
                                "original tie attack provenance must be inherited unchanged"
                            );
                            tail.intensity =
                                Some(std::sync::Arc::new(NoteIntensity::continued_from(
                                    head_intensity,
                                    tail.intensity.as_deref(),
                                )));
                            inherited += 1;
                        }
                        (None, None) => {}
                        (tail, head)
                            if tail.as_ref().is_none_or(|b| b.channel_key().is_some())
                                && head.is_none_or(|b| b.channel_key().is_some()) => {}
                        _ => panic!("typed score tie is missing its original intensity context"),
                    }
                }
                Some(link.lyric_owner_id.clone())
            } else {
                sung_owner(&note.lyric).map(|lyric| lyric.id.clone())
            };
            // Full equality includes original performance source_id, typed owner,
            // optional intensity, provenance, issues and complete shared timelines.
            assert_eq!(
                note.performance, expected,
                "current performance changed without exact source proof: {}",
                evidence.note_id
            );
            if note.performance.as_ref() == original {
                unchanged += 1;
            }
            if matches!(
                note.performance.as_ref().map(|binding| &binding.owner),
                Some(PerformanceOwner::Score { .. })
            ) {
                score_bindings += 1;
            }
            checked
                .entry(&evidence.note_id)
                .or_default()
                .push((note_index, expected, root));
        }
    }
    json!({"raw_equal_representations": unchanged, "proven_tie_inheritances": inherited, "score_bindings_checked": score_bindings, "selected_attack_recoveries": selected_recoveries})
}

pub fn capture(bytes: &[u8]) -> Value {
    let midi =
        musescore::parse(bytes).expect("the actual parser must parse the unchanged private master");
    let source = source_records(&midi);
    let raw_performance =
        performance::normalize(&midi).expect("untouched raw performance normalization");
    let mut score_validation = BTreeMap::new();
    let mut configurations = BTreeMap::new();
    for (name, target, profile) in CONFIGURATIONS {
        let outcome = convert_midi_with_profile(&midi, "french", None, target, profile);
        assert!(outcome.ok, "{name}: {:?}", outcome.msg);
        let project = outcome.svp.as_ref().expect("successful actual conversion");
        score_validation.insert(
            name,
            validate_current_performance(&midi, &raw_performance, project),
        );
        configurations.insert(name, project_record(project));
    }
    json!({"schema": 1, "source_sha256": sha256(bytes), "source_notes": source, "configurations": configurations, "current_score_validation": score_validation})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    const ALTI: &str = "note:mscx:staff:2:voice:1:mscx:staff:2:measure:10:voice:0:chord:2:note:0:occurrence:0:event:18";
    const SOP: &str = "note:mscx:staff:1:voice:1:mscx:staff:1:measure:20:voice:0:chord:0:note:0:occurrence:0:event:52";

    fn compare_master(master: &str, before: &Value, after: &Value) {
        assert_eq!(before["schema"], json!(1));
        assert_eq!(before["source_sha256"], after["source_sha256"]);
        let before_sources = before["source_notes"].as_object().unwrap();
        let after_sources = after["source_notes"].as_object().unwrap();
        assert_eq!(
            before_sources.keys().collect::<Vec<_>>(),
            after_sources.keys().collect::<Vec<_>>(),
            "{master}: parser identity set changed"
        );
        for (id, original) in before_sources {
            assert_eq!(
                Some(original),
                after_sources.get(id),
                "{master}: original parser note changed: {id}"
            );
        }
        let expected: BTreeSet<&str> = if master == "pb" {
            [ALTI, SOP].into()
        } else {
            [ALTI].into()
        };
        for (name, _, _) in CONFIGURATIONS {
            let old = &before["configurations"][name];
            let new = &after["configurations"][name];
            for field in [
                "ticks_per_beat",
                "language",
                "profile",
                "meters",
                "tempos",
                "tracks",
                "performance_alias_groups",
            ] {
                assert_eq!(old[field], new[field], "{master}/{name}: changed {field}");
            }
            let old_notes = old["notes"].as_object().unwrap();
            let new_notes = new["notes"].as_object().unwrap();
            for (id, representations) in old_notes {
                assert_eq!(
                    Some(representations),
                    new_notes.get(id),
                    "{master}/{name}: changed retained original {id}"
                );
            }
            let added: BTreeSet<_> = new_notes
                .keys()
                .filter(|id| !old_notes.contains_key(*id))
                .map(String::as_str)
                .collect();
            assert_eq!(
                added, expected,
                "{master}/{name}: only exact approved original identities may be added"
            );
            for id in &added {
                let representations = new_notes[*id].as_array().unwrap();
                assert_eq!(
                    representations.len(),
                    1,
                    "{master}/{name}: added endpoint duplicated: {id}"
                );
                let endpoint = &representations[0];
                let original = &before_sources[*id];
                for field in ["onset", "duration", "pitch", "on_event_id", "off_event_id"] {
                    assert_eq!(endpoint[field], original[field], "{master}/{name}: added note no longer represents its original {id}/{field}");
                }
                assert_eq!(endpoint["lyric"], json!({"kind": "extension"}));
                assert_eq!(
                    endpoint["performance"],
                    Value::Null,
                    "historical score payload remains absent; current Score is independently validated against raw normalization"
                );
                let destination = if *id == ALTI {
                    "mscx:staff:2:voice:1:polyphonic-member:2"
                } else {
                    "mscx:staff:1:voice:1"
                };
                assert_eq!(endpoint["source_track_id"], json!(destination));
            }
        }
    }

    #[test]
    fn baseline_validator_accepts_later_accent_after_proven_tails_and_rejects_corruption() {
        // The existing recovered_tails_do_not_consume_next_attack_accents
        // scenario: sfz lies on verse-two text that is a held tail on pass one.
        // It belongs to the later genuine attack only on that first pass.
        let lyric = |row, text| format!("<Lyrics><no>{row}</no><text>{text}</text></Lyrics>");
        let chord = |lyrics, links| {
            format!("<Chord><durationType>quarter</durationType>{lyrics}<Note><pitch>65</pitch>{links}</Note></Chord>")
        };
        for ordinary_context in [true, false] {
            let p = if ordinary_context {
                "<Dynamic><subtype>p</subtype></Dynamic>"
            } else {
                ""
            };
            let head = chord(
                format!("{}{}", lyric(0, "hold"), lyric(1, "again")),
                r#"<Tie id="first"/>"#,
            );
            let middle = chord(
                lyric(1, "ta"),
                r#"<endSpanner id="first"/><Tie id="second"/>"#,
            );
            let tail = chord(lyric(1, "il"), r#"<endSpanner id="second"/>"#);
            let end = chord(format!("{}{}", lyric(0, "end"), lyric(1, "end")), "");
            let xml = format!(
                r#"<museScore version="2.06"><programVersion>2.3.2</programVersion><Score><Division>480</Division><Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part><Staff id="1"><Measure len="4/4"><startRepeat/>{p}{head}<Dynamic><subtype>sfz</subtype></Dynamic>{middle}{tail}{end}<endRepeat>2</endRepeat></Measure></Staff></Score></museScore>"#
            );
            let midi = musescore::parse(xml.as_bytes()).unwrap();
            let raw = performance::normalize(&midi).unwrap();
            for target in [ExportTarget::Svp, ExportTarget::Ustx] {
                let outcome = convert_midi_with_profile(
                    &midi,
                    "english",
                    None,
                    target,
                    PronunciationProfile::Default,
                );
                assert!(outcome.ok, "{:?}", outcome.msg);
                let project = outcome.svp.unwrap();
                assert_eq!(project.tracks[0].notes.len(), 8);
                let report = validate_current_performance(&midi, &raw, &project);
                assert_eq!(report["selected_attack_recoveries"], 1);
                let gain = project.tracks[0].notes[3]
                    .performance
                    .as_ref()
                    .unwrap()
                    .intensity
                    .as_ref()
                    .unwrap();
                assert_eq!(
                    gain.evaluate(Fraction::new(1560, 480).unwrap()),
                    verse_lib::engine::score_intensity::Evaluation::Known(
                        verse_lib::engine::score_intensity::Intensity::Decibels(8.0)
                    )
                );
                for index in [0, 3, 5] {
                    let mut corrupt = project.clone();
                    corrupt.tracks[0].notes[index]
                        .performance
                        .as_mut()
                        .unwrap()
                        .source_id
                        .push_str("-unrelated");
                    assert!(
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            validate_current_performance(&midi, &raw, &corrupt)
                        }))
                        .is_err(),
                        "binding corruption at {index}"
                    );
                }
                let mut corrupt = project.clone();
                let intensity = std::sync::Arc::make_mut(
                    corrupt.tracks[0].notes[3]
                        .performance
                        .as_mut()
                        .unwrap()
                        .intensity
                        .as_mut()
                        .unwrap(),
                );
                intensity.provenance.as_mut().unwrap().evidence[0]
                    .source_ids
                    .push("unrelated-accent".into());
                assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    validate_current_performance(&midi, &raw, &corrupt)
                }))
                .is_err());
            }
        }
    }

    #[test]
    fn baseline_validator_rejects_restored_raw_final_tail_in_three_note_chain() {
        use verse_lib::engine::score_intensity::{Evaluation, Intensity};
        let lyric = |row, text| format!("<Lyrics><no>{row}</no><text>{text}</text></Lyrics>");
        let chord = |lyrics, links, velocity| {
            format!(
            "<Chord><durationType>quarter</durationType>{lyrics}<Note><pitch>65</pitch>{links}<veloType>user</veloType><velocity>{velocity}</velocity></Note></Chord>"
        )
        };
        let head = chord(
            format!("{}{}", lyric(0, "hold"), lyric(1, "again")),
            r#"<Tie id="first"/>"#,
            64,
        );
        let middle = chord(
            lyric(1, "ta"),
            r#"<endSpanner id="first"/><Tie id="second"/>"#,
            20,
        );
        let tail = chord(lyric(1, "il"), r#"<endSpanner id="second"/>"#, 20);
        let xml = format!(
            r#"<museScore version="2.06"><programVersion>2.3.2</programVersion><Score><Division>480</Division><Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part><Staff id="1"><Measure len="3/4"><startRepeat/><Dynamic><subtype>p</subtype></Dynamic>{head}<Dynamic><subtype>f</subtype></Dynamic>{middle}{tail}<endRepeat>2</endRepeat></Measure></Staff></Score></museScore>"#
        );
        let midi = musescore::parse(xml.as_bytes()).unwrap();
        let raw = performance::normalize(&midi).unwrap();
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let out = convert_midi_with_profile(
                &midi,
                "english",
                None,
                target,
                PronunciationProfile::Default,
            );
            assert!(out.ok, "{:?}", out.msg);
            let project = out.svp.unwrap();
            assert_eq!(project.tracks[0].notes.len(), 6);
            assert_eq!(project.continuity_violation(), None);
            verse_lib::engine::target::validate_for(target, &project).unwrap();
            verse_lib::engine::target::serialize_to(target, &project).unwrap();
            verse_lib::bundle::BundleProject::from_projection(target, &project).unwrap();
            let report = validate_current_performance(&midi, &raw, &project);
            assert_eq!(report["proven_tie_inheritances"], 2);
            let head_origin = project.tracks[0].notes[0]
                .source_evidence
                .as_ref()
                .unwrap()
                .origin
                .as_ref()
                .unwrap();
            let original_attack = raw.bindings
                [&(head_origin.track_id.clone(), head_origin.note_on_order)]
                .intensity
                .as_ref()
                .unwrap()
                .provenance
                .as_ref()
                .unwrap();
            let original_ids: Vec<_> = original_attack
                .evidence
                .iter()
                .flat_map(|e| &e.source_ids)
                .collect();
            assert!(!original_ids.is_empty());
            let transfers =
                verse_lib::engine::target::performance_report(target, &project).unwrap();
            for note in &project.tracks[0].notes[1..3] {
                assert_eq!(
                    note.performance
                        .as_ref()
                        .unwrap()
                        .intensity
                        .as_ref()
                        .unwrap()
                        .provenance
                        .as_ref(),
                    Some(original_attack),
                    "raw source head, independent of continuation helper"
                );
                let id = &note.source_evidence.as_ref().unwrap().note_id;
                if target == ExportTarget::Ustx {
                    assert!(
                        transfers.iter().any(|span| span.status
                            == verse_lib::engine::performance::TransferStatus::Mapped
                            && span.note_ids.contains(id)
                            && original_ids
                                .iter()
                                .all(|source_id| span.source_ids.contains(source_id))),
                        "emitted continuation must retain the original attack contributors: {id}"
                    );
                }
            }
            let mut missing_provenance = project.clone();
            std::sync::Arc::make_mut(
                missing_provenance.tracks[0].notes[2]
                    .performance
                    .as_mut()
                    .unwrap()
                    .intensity
                    .as_mut()
                    .unwrap(),
            )
            .provenance = None;
            assert!(verse_lib::engine::target::serialize_to(target, &missing_provenance).is_err());
            assert!(
                verse_lib::bundle::BundleProject::from_projection(target, &missing_provenance)
                    .is_err()
            );
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    validate_current_performance(&midi, &raw, &missing_provenance)
                }))
                .is_err(),
                "independent baseline oracle must reject removed attack evidence"
            );
            let tail = &project.tracks[0].notes[2];
            assert!(matches!(tail.lyric, ProjectedLyric::Extension));
            let origin = tail
                .source_evidence
                .as_ref()
                .unwrap()
                .origin
                .as_ref()
                .unwrap();
            let original = raw.bindings[&(origin.track_id.clone(), origin.note_on_order)].clone();
            let at = Fraction::new(1080, 480).unwrap();
            let expected = tail
                .performance
                .as_ref()
                .unwrap()
                .intensity
                .as_ref()
                .unwrap()
                .evaluate(at);
            let wrong = original.intensity.as_ref().unwrap().evaluate(at);
            assert_eq!(expected, Evaluation::Known(Intensity::Decibels(7.75)));
            assert_eq!(wrong, Evaluation::Known(Intensity::Decibels(-15.0)));
            // Numeric difference is 22.75 dB; this is not a provenance-only mutation.
            assert_ne!(expected, wrong);
            let mut corrupt = project.clone();
            corrupt.tracks[0].notes[2].performance = Some(original);
            assert!(corrupt.continuity_violation().is_some());
            assert!(verse_lib::engine::target::validate_for(target, &corrupt).is_err());
            assert!(verse_lib::engine::target::serialize_to(target, &corrupt).is_err());
            match target {
                ExportTarget::Svp => {
                    assert!(verse_lib::engine::target::svp::serialize(&corrupt).is_err())
                }
                ExportTarget::Ustx => {
                    assert!(verse_lib::engine::target::ustx::serialize(&corrupt).is_err())
                }
            }
            assert!(verse_lib::bundle::BundleProject::from_projection(target, &corrupt).is_err());
            let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                validate_current_performance(&midi, &raw, &corrupt)
            }))
            .expect_err("restoring the raw final-tail binding must fail");
            let message = failure
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| failure.downcast_ref::<&str>().copied())
                .unwrap();
            assert!(
                message.contains("current performance changed without exact source proof")
                    || message
                        .contains("original tie attack provenance must be inherited unchanged"),
                "{message}"
            );
            // The second pass owns separate sung syllables, so each keeps its raw attack.
            for note in &project.tracks[0].notes[3..] {
                let o = note
                    .source_evidence
                    .as_ref()
                    .unwrap()
                    .origin
                    .as_ref()
                    .unwrap();
                assert!(o.continuation.is_none());
                assert_eq!(
                    note.performance.as_ref(),
                    raw.bindings.get(&(o.track_id.clone(), o.note_on_order))
                );
            }
        }
    }

    #[test]
    fn baseline_validator_accepts_extension_without_tie_without_inheritance() {
        use verse_lib::engine::projection::ProjectedNote;
        use verse_lib::engine::score_intensity::{Evaluation, Intensity};
        // A real selected lyric on the tail would block extension recovery.
        // Leave it lyric-free so the head's written ticks are the only authority.
        let xml = r#"<museScore version="2.06"><programVersion>2.3.2</programVersion><Score><Division>480</Division><Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part><Staff id="1"><Measure len="2/4"><Dynamic><subtype>p</subtype></Dynamic><Chord><durationType>quarter</durationType><Lyrics><no>0</no><text>hold</text><ticks>480</ticks></Lyrics><Note><pitch>65</pitch><veloType>user</veloType><velocity>64</velocity></Note></Chord><Chord><durationType>quarter</durationType><Note><pitch>65</pitch><veloType>user</veloType><velocity>20</velocity></Note></Chord></Measure></Staff></Score></museScore>"#;
        let midi = musescore::parse(xml.as_bytes()).unwrap();
        let raw = performance::normalize(&midi).unwrap();
        let source_notes: Vec<_> = midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some((event.tick, note)),
                _ => None,
            })
            .collect();
        assert_eq!(source_notes.len(), 2);
        let (head_tick, head_source) = source_notes[0];
        let (tail_tick, tail_source) = source_notes[1];
        assert_eq!((head_tick, tail_tick), (0, 480));
        assert!(tail_source.lyrics.is_empty());
        assert!(tail_source
            .source
            .continuity
            .as_ref()
            .unwrap()
            .incoming_tie
            .is_none());
        let extension = &head_source.source.continuity.as_ref().unwrap().extensions[0];
        assert_eq!(extension.lyric_id, head_source.lyrics[0].id);
        assert_eq!(
            (extension.start_tick, extension.end_tick),
            (head_tick, Some(tail_tick))
        );
        assert!(extension.evidence.raw_xml.contains("<ticks>480</ticks>"));
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let out = convert_midi_with_profile(
                &midi,
                "english",
                None,
                target,
                PronunciationProfile::Default,
            );
            assert!(out.ok, "{:?}", out.msg);
            let project = out.svp.unwrap();
            assert_eq!(project.continuity_violation(), None);
            verse_lib::engine::target::validate_for(target, &project).unwrap();
            verse_lib::engine::target::serialize_to(target, &project).unwrap();
            assert_eq!(project.tracks.len(), 1);
            assert_eq!(project.tracks[0].notes.len(), 2);
            let head = &project.tracks[0].notes[0];
            let tail = &project.tracks[0].notes[1];
            assert_eq!(tail.onset_ticks, tail_tick);
            assert!(matches!(tail.lyric, ProjectedLyric::Extension));
            let o = tail
                .source_evidence
                .as_ref()
                .unwrap()
                .origin
                .as_ref()
                .unwrap();
            let link = o.continuation.as_ref().unwrap();
            assert_eq!(
                link.predecessor_id,
                head.source_evidence.as_ref().unwrap().note_id
            );
            assert_eq!(link.lyric_owner_id, extension.lyric_id);
            assert_eq!(o.source, tail_source.source);
            assert!(o.source.continuity.as_ref().unwrap().incoming_tie.is_none());
            assert_eq!(
                tail.performance.as_ref(),
                raw.bindings.get(&(o.track_id.clone(), o.note_on_order))
            );
            let attack_at = |note: &ProjectedNote, tick| {
                note.performance
                    .as_ref()
                    .unwrap()
                    .intensity
                    .as_ref()
                    .unwrap()
                    .evaluate(Fraction::new(tick, 480).unwrap())
            };
            assert_eq!(
                attack_at(head, 120),
                Evaluation::Known(Intensity::Decibels(-4.0))
            );
            assert_eq!(
                attack_at(tail, 600),
                Evaluation::Known(Intensity::Decibels(-15.0))
            );
            assert_eq!(
                validate_current_performance(&midi, &raw, &project)["proven_tie_inheritances"],
                0
            );
        }
    }

    // Trusted R10 capture/build receipts from the completed historical run.
    // These pins live outside the mutable directory they authenticate.
    const SNAPSHOT_SHA256: &str =
        "d0736abcee8a21d38ce52127c9e0b1f8d3217c99db0a055ade9cc464637b3668";
    const GOLDEN_MANIFEST_SHA256: &str =
        "c88b8fee5dadf97dc953061c4f5b3b45e2da03a549d20cc25b194c86a6d613b7";
    const GENERATOR_SHA256: &str =
        "ba18737c0e3b7bf0754e746d36ff6f0b5e329096e5f1a7f9ba1de7ed86d8ecab";

    fn pinned_json(bytes: &[u8], pin: &str, label: &str) -> Result<Value, String> {
        if sha256(bytes) != pin {
            return Err(format!("{label} hash differs from independent receipt"));
        }
        serde_json::from_slice(bytes).map_err(|error| error.to_string())
    }

    fn verify_capture_receipts(
        snapshot: &[u8],
        manifest: &[u8],
        generator: &[u8],
        pins: (&str, &str, &str),
    ) -> Result<Value, String> {
        let snapshot = pinned_json(snapshot, pins.0, "snapshot")?;
        let manifest = pinned_json(manifest, pins.1, "completed golden manifest")?;
        if sha256(generator) != pins.2 {
            return Err("generator binary hash differs from independent receipt".into());
        }
        if snapshot["baseline_commit"] != "28abaea44e20951a32d26a61de68c10139d34712"
            || snapshot["method"] != "git-base-plus-hash-verified-work-start"
            || manifest["schema"] != 1
            || manifest["snapshot_manifest_sha256"] != pins.0
            || manifest["generator_binary_sha256"] != pins.2
            || manifest["files"].as_object().is_none_or(|files| {
                files.len() != MASTERS.len()
                    || !files.contains_key("pb.json")
                    || !files.contains_key("chant.json")
            })
        {
            return Err("inconsistent historical capture receipts".into());
        }
        Ok(manifest)
    }

    #[test]
    fn independently_pinned_receipts_reject_colocated_manifest_and_generator_tampering() {
        let snapshot = serde_json::to_vec(&json!({
            "baseline_commit":"28abaea44e20951a32d26a61de68c10139d34712",
            "method":"git-base-plus-hash-verified-work-start"
        }))
        .unwrap();
        let generator = b"historical test generator";
        let golden = b"{\"original\":true}";
        let (snapshot_pin, generator_pin) = (sha256(&snapshot), sha256(generator));
        let manifest = json!({"schema":1, "snapshot_manifest_sha256":snapshot_pin,
            "generator_binary_sha256":generator_pin,
            "files":{"pb.json":sha256(golden),"chant.json":sha256(golden)}});
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let manifest_pin = sha256(&bytes);
        let pins = (
            snapshot_pin.as_str(),
            manifest_pin.as_str(),
            generator_pin.as_str(),
        );
        let verified = verify_capture_receipts(&snapshot, &bytes, generator, pins).unwrap();
        pinned_json(
            golden,
            verified["files"]["pb.json"].as_str().unwrap(),
            "golden",
        )
        .unwrap();
        assert!(verify_capture_receipts(b"{}", &bytes, generator, pins)
            .unwrap_err()
            .contains("snapshot hash"));
        assert!(
            verify_capture_receipts(&snapshot, &bytes, b"candidate generator", pins)
                .unwrap_err()
                .contains("generator binary hash")
        );
        let changed_golden = b"{\"original\":false}";
        assert!(pinned_json(
            changed_golden,
            verified["files"]["pb.json"].as_str().unwrap(),
            "golden"
        )
        .is_err());
        for (pointer, replacement) in [
            ("/files/pb.json", sha256(changed_golden)),
            ("/generator_binary_sha256", sha256(b"candidate generator")),
        ] {
            let mut forged = manifest.clone();
            *forged.pointer_mut(pointer).unwrap() = json!(replacement);
            let forged = serde_json::to_vec(&forged).unwrap();
            assert!(verify_capture_receipts(&snapshot, &forged, generator, pins)
                .unwrap_err()
                .contains("completed golden manifest hash"));
        }
    }

    /// Never silently skips. Explicit execution requires both original masters,
    /// the actual baseline goldens, and the independently pinned snapshot, manifest and generator.
    #[test]
    #[ignore = "requires VERSE_CONTINUITY_CORPUS_DIR, VERSE_CONTINUITY_BASELINE_DIR, and VERSE_CONTINUITY_BASELINE_SNAPSHOT_SHA256, and VERSE_CONTINUITY_BASELINE_GENERATOR"]
    fn private_masters_match_independent_prechange_nominal_goldens() {
        let corpus = std::env::var("VERSE_CONTINUITY_CORPUS_DIR")
            .expect("both original private masters are mandatory");
        let golden = std::env::var("VERSE_CONTINUITY_BASELINE_DIR")
            .expect("independently generated baseline directory is mandatory");
        let pinned = std::env::var("VERSE_CONTINUITY_BASELINE_SNAPSHOT_SHA256")
            .expect("pin the prepared snapshot-manifest.json SHA256 from the R10 handoff");
        assert_eq!(
            pinned, SNAPSHOT_SHA256,
            "snapshot pin must match the trusted R10 receipt"
        );
        let generator = std::env::var("VERSE_CONTINUITY_BASELINE_GENERATOR")
            .expect("path to the original R10 baseline-capture binary is mandatory");
        let dir = Path::new(&golden);
        let snapshot_bytes = std::fs::read(dir.join("snapshot-manifest.json"))
            .expect("baseline snapshot provenance is mandatory");
        let manifest_bytes = std::fs::read(dir.join("golden-manifest.json"))
            .expect("completed golden capture manifest is mandatory");
        let generator_bytes = std::fs::read(generator).expect("historical generator is mandatory");
        let manifest = verify_capture_receipts(
            &snapshot_bytes,
            &manifest_bytes,
            &generator_bytes,
            (SNAPSHOT_SHA256, GOLDEN_MANIFEST_SHA256, GENERATOR_SHA256),
        )
        .expect("authenticate historical artifacts before consuming either golden");
        for (master, filename, hash) in MASTERS {
            let input = Path::new(&corpus).join(filename);
            let bytes = std::fs::read(&input)
                .unwrap_or_else(|error| panic!("missing required {}: {error}", input.display()));
            assert_eq!(sha256(&bytes), hash, "wrong original {filename}");
            let filename = format!("{master}.json");
            let golden_bytes = std::fs::read(dir.join(&filename))
                .expect("both actual baseline goldens are mandatory");
            let before = pinned_json(
                &golden_bytes,
                manifest["files"][&filename].as_str().unwrap(),
                "golden",
            )
            .expect("golden bytes must match the authenticated completion manifest");
            assert_eq!(before["source_sha256"], json!(hash));
            assert_eq!(before["snapshot_manifest_sha256"], json!(pinned));
            let after = capture(&bytes);
            compare_master(master, &before, &after);
            assert_eq!(
                std::fs::read(&input).unwrap(),
                bytes,
                "private master must remain byte-identical"
            );
        }
    }
}
