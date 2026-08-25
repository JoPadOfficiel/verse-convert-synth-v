//! Source-owned stem planning.
//!
//! Synthesizer V cannot represent arbitrary symbolic instruments as editable
//! instrument-note tracks.  A stem therefore corresponds to one source Part
//! and is later rendered as an audio-backed instrumental track.

use crate::engine::convert::{ExportRepresentation, TrackReport};
use crate::engine::midi::{Kind, Midi};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StemRole {
    VocalReference,
    Accompaniment,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StemDescriptor {
    /// Bundle-owned, filename-safe identifier. Source names are never paths.
    pub stem_id: String,
    pub source_part_id: String,
    /// Position of this Part in the source topology. Stems skip note-free
    /// Parts, so this is not the stem's own index, and it is the key that maps
    /// a stem onto the Part MuseScore cut for it.
    pub source_part_index: usize,
    pub display_name: String,
    pub source_track_ids: Vec<String>,
    pub source_note_count: usize,
    pub role: StemRole,
    pub active_by_default: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StemPlan {
    pub stems: Vec<StemDescriptor>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StemPlanError {
    #[error("source contains no note-bearing Parts")]
    NoNoteBearingParts,
    #[error("source topology contains duplicate Part ID {0}")]
    DuplicatePartId(String),
    #[error("source topology references unknown projection track {0}")]
    UnknownProjectionTrack(String),
    #[error("stem plan contains duplicate stem ID {0}")]
    DuplicateStemId(String),
    #[error("projection track {track_id} belongs to more than one stem")]
    TrackAssignedTwice { track_id: String },
    #[error("stem {0} has no source projection tracks")]
    EmptyStem(String),
    #[error("stem {0} has no source notes")]
    NoteFreeStem(String),
}

impl StemPlan {
    /// Builds one render stem per note-bearing source Part, preserving source
    /// Part order. Technical chord-member lanes remain grouped inside their
    /// owning Part.
    pub fn from_source(midi: &Midi, reports: &[TrackReport]) -> Result<Self, StemPlanError> {
        let tracks = midi
            .tracks
            .iter()
            .map(|track| (track.id.as_str(), track))
            .collect::<BTreeMap<_, _>>();
        let projected_vocals = reports
            .iter()
            .filter(|report| {
                matches!(
                    report.export_representation,
                    ExportRepresentation::VocalNotes
                        | ExportRepresentation::VocalNotesAndReferenceMix
                )
            })
            .map(|report| report.source_id.as_str())
            .collect::<BTreeSet<_>>();

        let mut seen_parts = BTreeSet::new();
        let mut stems = Vec::new();
        for (part_index, part) in midi.topology.parts.iter().enumerate() {
            if !seen_parts.insert(part.id.clone()) {
                return Err(StemPlanError::DuplicatePartId(part.id.clone()));
            }

            let mut source_track_ids = Vec::new();
            let mut seen_tracks = BTreeSet::new();
            for voice in part.staves.iter().flat_map(|staff| &staff.voices) {
                for track_id in &voice.projection_track_ids {
                    if !tracks.contains_key(track_id.as_str()) {
                        return Err(StemPlanError::UnknownProjectionTrack(track_id.clone()));
                    }
                    if seen_tracks.insert(track_id.clone()) {
                        source_track_ids.push(track_id.clone());
                    }
                }
            }
            if source_track_ids.is_empty() {
                continue;
            }

            let source_note_count = source_track_ids
                .iter()
                .filter_map(|track_id| tracks.get(track_id.as_str()))
                .flat_map(|track| &track.events)
                .filter(|event| {
                    matches!(
                        &event.kind,
                        Kind::NoteOn(note) if note.velocity.unwrap_or(1) != 0
                    )
                })
                .count();
            if source_note_count == 0 {
                // Metadata/KAR word tracks are preserved by the source and
                // ledger; they are not fake silent instrumental stems.
                continue;
            }

            let has_vocal_projection = source_track_ids
                .iter()
                .any(|track_id| projected_vocals.contains(track_id.as_str()));
            let role = if has_vocal_projection {
                StemRole::VocalReference
            } else {
                StemRole::Accompaniment
            };
            stems.push(StemDescriptor {
                stem_id: stable_stem_id(part_index, &part.id),
                source_part_id: part.id.clone(),
                source_part_index: part_index,
                display_name: nonempty_display_name(&part.name, part_index),
                source_track_ids,
                source_note_count,
                role,
                active_by_default: !has_vocal_projection,
            });
        }

        let plan = Self { stems };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), StemPlanError> {
        if self.stems.is_empty() {
            return Err(StemPlanError::NoNoteBearingParts);
        }
        let mut stem_ids = BTreeSet::new();
        let mut part_ids = BTreeSet::new();
        let mut track_ids = BTreeSet::new();
        for stem in &self.stems {
            if !stem_ids.insert(stem.stem_id.clone()) {
                return Err(StemPlanError::DuplicateStemId(stem.stem_id.clone()));
            }
            if !part_ids.insert(stem.source_part_id.clone()) {
                return Err(StemPlanError::DuplicatePartId(stem.source_part_id.clone()));
            }
            if stem.source_track_ids.is_empty() {
                return Err(StemPlanError::EmptyStem(stem.stem_id.clone()));
            }
            if stem.source_note_count == 0 {
                return Err(StemPlanError::NoteFreeStem(stem.stem_id.clone()));
            }
            for track_id in &stem.source_track_ids {
                if !track_ids.insert(track_id.clone()) {
                    return Err(StemPlanError::TrackAssignedTwice {
                        track_id: track_id.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn expected_stem_ids(&self) -> Vec<String> {
        self.stems.iter().map(|stem| stem.stem_id.clone()).collect()
    }
}

fn stable_stem_id(index: usize, source_part_id: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(source_part_id.as_bytes()));
    format!("part-{:03}-{}", index + 1, &digest[..12])
}

fn nonempty_display_name(name: &str, index: usize) -> String {
    let name = name.trim();
    if name.is_empty() {
        format!("Part {}", index + 1)
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::convert::{
        Diagnostic, LyricStatus, LyricStatusState, SourceRole, TrackReport,
    };
    use crate::engine::midi::{
        Event, Midi, NoteOn, NoteSource, SourceFormat, SourcePart, SourceStaff, SourceTopology,
        SourceVoice, TimeBase, Track,
    };

    fn report(source_id: &str, representation: ExportRepresentation) -> TrackReport {
        TrackReport {
            id: 0,
            source_id: source_id.into(),
            track: source_id.into(),
            notes: 1,
            role: String::new(),
            placed: 1,
            source_role: SourceRole::Vocal,
            lyric_status: LyricStatus {
                state: LyricStatusState::SourceOwned,
                source_text_count: 1,
                projected_text_count: 1,
                explicit_empty_count: 0,
                continuation_count: 0,
                unsupported_count: 0,
            },
            export_representation: representation,
            requires_voice_assignment: true,
            warnings: Vec::<Diagnostic>::new(),
        }
    }

    fn note_track(id: &str, source_track: usize) -> Track {
        let mut track = Track::new(id, source_track);
        track.events.push(Event {
            tick: 0,
            order: 0,
            kind: Kind::NoteOn(NoteOn {
                channel: Some(0),
                key: Some(60),
                velocity: Some(100),
                source: NoteSource::default(),
                lyrics: vec![],
            }),
        });
        track
    }

    #[test]
    fn groups_technical_lanes_by_part_and_mutes_vocal_reference_stems() {
        let tracks = vec![
            note_track("sop-a", 0),
            note_track("sop-b", 0),
            note_track("pno", 1),
        ];
        let midi = Midi {
            ticks_per_beat: 480,
            time_base: TimeBase::PulsesPerQuarter(480),
            format: 1,
            source_format: SourceFormat::MuseScore,
            topology: SourceTopology {
                parts: vec![
                    SourcePart {
                        id: "part:soprano".into(),
                        name: "Soprano".into(),
                        source_track_ids: vec!["sop-a".into(), "sop-b".into()],
                        staves: vec![SourceStaff {
                            id: "staff:1".into(),
                            voices: vec![SourceVoice {
                                id: "voice:1".into(),
                                number: "1".into(),
                                projection_track_ids: vec!["sop-a".into(), "sop-b".into()],
                            }],
                        }],
                    },
                    SourcePart {
                        id: "part:piano".into(),
                        name: "Piano".into(),
                        source_track_ids: vec!["pno".into()],
                        staves: vec![SourceStaff {
                            id: "staff:2".into(),
                            voices: vec![SourceVoice {
                                id: "voice:2".into(),
                                number: "1".into(),
                                projection_track_ids: vec!["pno".into()],
                            }],
                        }],
                    },
                ],
            },
            tracks,
        };
        let plan =
            StemPlan::from_source(&midi, &[report("sop-a", ExportRepresentation::VocalNotes)])
                .unwrap();
        assert_eq!(plan.stems.len(), 2);
        assert_eq!(plan.stems[0].source_track_ids, ["sop-a", "sop-b"]);
        assert_eq!(plan.stems[0].role, StemRole::VocalReference);
        assert!(!plan.stems[0].active_by_default);
        assert_eq!(plan.stems[1].role, StemRole::Accompaniment);
        assert!(plan.stems[1].active_by_default);
        assert!(plan.stems.iter().all(|stem| {
            stem.stem_id.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
        }));
    }

    #[test]
    fn note_free_metadata_parts_are_not_fake_stems() {
        let tracks = vec![Track::new("words", 0), note_track("music", 1)];
        let midi = Midi {
            ticks_per_beat: 480,
            time_base: TimeBase::PulsesPerQuarter(480),
            format: 1,
            source_format: SourceFormat::KaraokeMidi,
            topology: SourceTopology::from_tracks(&tracks),
            tracks,
        };
        let plan = StemPlan::from_source(&midi, &[]).unwrap();
        assert_eq!(plan.stems.len(), 1);
        assert_eq!(plan.stems[0].source_track_ids, ["music"]);
    }
}
