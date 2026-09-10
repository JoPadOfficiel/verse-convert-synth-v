//! Target-neutral projection of the source onto singable vocal material.
//!
//! This is the seam between `convert.rs`, which decides *what* the source
//! actually asks to be sung, and an export target, which decides *how* one
//! format writes it down. Nothing here belongs to a single target: there are no
//! blicks, no track colours, no display order, no rendered marker text.
//!
//! Positions are IR ticks against [`ProjectedProject::ticks_per_beat`], which
//! each parser derives from the source (`musescore.rs` from `Division`,
//! `musicxml.rs` from the LCM of every `divisions`) precisely so that every
//! source duration is exactly representable. Ticks are therefore the one unit
//! that loses nothing, and each target converts out of them exactly once.
use crate::engine::midi::{Lyric, LyricState};

/// Original ownership and final editable placement of a retained note. This
/// table is collected from the projection independently of performance spans.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntensityNoteProjection {
    pub note_id: String,
    pub source_track_id: String,
    pub target_track: usize,
    pub destination_track_id: String,
    pub start_tick: u32,
    pub end_tick: u32,
    /// Independently proven original attack of a recovered score tie chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intensity_attack_note_id: Option<String>,
}

/// One source, projected. A target consumes this and nothing else.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectedProject {
    pub pronunciation_profile: crate::engine::target::PronunciationProfile,
    /// IR ticks per quarter note; the denominator of every position below.
    pub ticks_per_beat: u16,
    /// The voice-database language the user selected. Neutral here because it
    /// names a source-independent user choice, not a target field.
    pub language: String,
    pub meters: Vec<ProjectedMeter>,
    /// The source's tempo changes. A target keys them by tick itself, so neither
    /// this order nor the absence of duplicate ticks is something a target may
    /// rely on: only [`ProjectedTempo::discovery_index`] carries meaning here.
    pub tempos: Vec<ProjectedTempo>,
    pub tracks: Vec<ProjectedTrack>,
}

impl ProjectedProject {
    /// A proven continuation must still follow its actual retained predecessor
    /// after filtering, pronunciation and technical lane splitting.
    pub fn continuity_violation(&self) -> Option<String> {
        let mut intensity_budget = crate::engine::score_intensity::ProvenanceBudget::default();
        for track in &self.tracks {
            let mut notes: Vec<_> = track.notes.iter().collect();
            notes.sort_by_key(|note| note.onset_ticks);
            let mut root_owners = std::collections::BTreeMap::<&str, &str>::new();
            for (index, note) in notes.iter().enumerate() {
                if let (Some(evidence), Some(owner)) =
                    (&note.source_evidence, note.lyric.sung_owner_identity())
                {
                    root_owners.insert(&evidence.note_id, owner);
                }
                let Some(link) = note
                    .source_evidence
                    .as_ref()
                    .and_then(|e| e.origin.as_ref())
                    .and_then(|o| o.continuation.as_ref())
                else {
                    continue;
                };
                let previous = index.checked_sub(1).map(|i| notes[i]);
                if !note.lyric.continues_previous_note()
                    || link.destination_track_id != track.source_track_id
                    || root_owners.get(link.predecessor_id.as_str()).copied()
                        != Some(link.lyric_owner_id.as_str())
                    || previous.is_none_or(|head| {
                        head.source_evidence
                            .as_ref()
                            .is_none_or(|e| e.note_id != link.predecessor_id)
                            || head.onset_ticks.checked_add(head.duration_ticks)
                                != Some(note.onset_ticks)
                    })
                {
                    return Some(format!("source continuation at tick {} on track {} no longer follows its proven predecessor {}",
                        note.onset_ticks, track.source_track_id, link.predecessor_id));
                }
                let head = previous.unwrap();
                match link.kind {
                    ContinuationKind::Extension if link.intensity_attack_note_id.is_some() => {
                        return Some(
                            "A syllable extension cannot claim an inherited tie attack".into(),
                        );
                    }
                    ContinuationKind::Tie => {
                        let head_evidence = head.source_evidence.as_ref().unwrap();
                        let expected_root = head_evidence
                            .origin
                            .as_ref()
                            .and_then(|o| o.continuation.as_ref())
                            .filter(|link| link.kind == ContinuationKind::Tie)
                            .and_then(|link| link.intensity_attack_note_id.as_deref())
                            .unwrap_or(&head_evidence.note_id);
                        let source = &note
                            .source_evidence
                            .as_ref()
                            .unwrap()
                            .origin
                            .as_ref()
                            .unwrap()
                            .source;
                        let typed_tail = source
                            .continuity
                            .as_ref()
                            .and_then(|c| c.incoming_tie.as_ref())
                            .is_some_and(|tie| {
                                tie.tail.source_id == source.id
                                    && tie.tail.occurrence == source.occurrence
                                    && source.continuity.as_ref().is_some_and(|c| {
                                        c.playback_segment == tie.tail.playback_segment
                                    })
                                    && tie.contact_tick == note.onset_ticks
                                    && tie.pitch == note.pitch
                                    && head.pitch == note.pitch
                            });
                        if !typed_tail
                            || link.intensity_attack_note_id.as_deref() != Some(expected_root)
                        {
                            return Some(
                                "Source tie attack identity no longer follows its proven chain"
                                    .into(),
                            );
                        }
                        use crate::engine::performance::PerformanceOwner;
                        let valid = match (&note.performance, &head.performance) {
                            (None, None) => true,
                            (Some(tail), Some(head)) => match (&tail.owner, &head.owner) {
                                (
                                    PerformanceOwner::Score { voice: a },
                                    PerformanceOwner::Score { voice: b },
                                ) => {
                                    match tail.intensity.as_deref().zip(head.intensity.as_deref()) {
                                        Some((tail, head)) if a == b => {
                                            match tail.is_continuation_of_bounded(
                                                head,
                                                &mut intensity_budget,
                                            ) {
                                                Ok(valid) => valid,
                                                Err(error) => return Some(error),
                                            }
                                        }
                                        _ => false,
                                    }
                                }
                                (PerformanceOwner::Midi { .. }, PerformanceOwner::Midi { .. }) => {
                                    true
                                }
                                _ => false,
                            },
                            (Some(note), None) | (None, Some(note)) => note.channel_key().is_some(),
                        };
                        if !valid {
                            return Some(format!(
                                "Source tie at tick {} lost its proven inherited intensity attack",
                                note.onset_ticks
                            ));
                        }
                    }
                    ContinuationKind::Extension => {}
                }
                if let Some(evidence) = &note.source_evidence {
                    root_owners.insert(&evidence.note_id, &link.lyric_owner_id);
                }
            }
        }
        None
    }

    /// The tempo entries in the order the source revealed them, which is the
    /// order a target must validate positions in: it decides which event a
    /// refusal names. Emission order is [`ProjectedProject::tempos`]; only
    /// refusal order is this.
    pub fn tempos_in_discovery_order(&self) -> Vec<&ProjectedTempo> {
        let mut ordered: Vec<&ProjectedTempo> = self.tempos.iter().collect();
        ordered.sort_by_key(|tempo| tempo.discovery_index);
        ordered
    }

    /// The first lane holding two notes at once, described by source track and
    /// both ticks, or `None` when every lane is monophonic.
    ///
    /// [`ProjectedTrack`] claims monophony and every producer establishes it, so
    /// this proves the claim rather than repairing it: a lane that still overlaps
    /// here means an adapter stopped decomposing simultaneity, and no target can
    /// make that right. One Synthesizer V group and one OpenUtau voice part are
    /// each monophonic, and only one of the two says so — OpenUtau refuses the
    /// export while Synthesizer V writes the stack and sings one note of it.
    pub fn monophony_violation(&self) -> Option<String> {
        for track in &self.tracks {
            // Sorted because this is asked of any projection, including one a
            // caller built by hand; a producer's own order is not the contract.
            let mut spans: Vec<(u32, u32)> = track
                .notes
                .iter()
                .map(|note| (note.onset_ticks, note.duration_ticks))
                .collect();
            spans.sort_by_key(|(onset, _)| *onset);
            for pair in spans.windows(2) {
                // Widened because a lane's last note may legitimately end past
                // the tick range, and an overflow here would abort on analysis.
                let end = u64::from(pair[0].0) + u64::from(pair[0].1);
                if end > u64::from(pair[1].0) {
                    return Some(format!(
                        "the note at MIDI tick {} on source track {} still sounds at tick {}, \
                         where the next note of the same lane begins",
                        pair[0].0, track.source_track_id, pair[1].0
                    ));
                }
            }
        }
        None
    }
}

/// A meter change, carried as a bar index because that is what the source
/// states and what every target's time-signature list wants. No target needs
/// arithmetic here, so a meter change inside a bar is rejected upstream rather
/// than rounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectedMeter {
    pub bar_index: u32,
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedTempo {
    pub tick: u32,
    /// The BPM in force from this tick: the *last* source event at it, because
    /// a later event at the same instant is the one that takes effect.
    pub bpm: f64,
    /// `track_id:event_order` of the *first* source event at this tick, so a
    /// target that has to refuse the position names the same event the source
    /// revealed first. `None` for the default tempo a source carrying no tempo
    /// event at all implies.
    pub source: Option<String>,
    /// Where this tick first appeared while reading the source, across all
    /// tracks. Not emitted anywhere; it exists only so a target can refuse
    /// positions in the order the source revealed them. See
    /// [`ProjectedProject::tempos_in_discovery_order`].
    pub discovery_index: usize,
}

/// One monophonic projection lane.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedTrack {
    pub name: String,
    /// The source track this lane was projected from. Provenance, and the
    /// identifier a target names when it must refuse this lane's timing.
    ///
    /// Not a key: several lanes legitimately share one source track. Stacked
    /// verses do it, and so does a lane and its untexted companion.
    /// A proven continuation may come from a sibling technical adapter lane;
    /// its original ownership stays in the note's `source_evidence.origin`.
    pub source_track_id: String,
    /// Whether this lane opens silent in the target application.
    ///
    /// Playback state, not a target cosmetic: the source decides it by leaving
    /// notes untexted, and both targets must agree or the same project would
    /// sing different notes depending on which file the user opened. Each target
    /// still owns *where* it writes the flag — OpenUtau puts it on the track,
    /// Synthesizer V inside the track's mixer.
    pub muted: bool,
    pub notes: Vec<ProjectedNote>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedNote {
    /// Original channel ownership and a shared held performance timeline.
    /// Travels with the note through filtering and lane splitting; never joined
    /// back by pitch, time or display name. No target units live here.
    pub performance: Option<crate::engine::performance::PerformanceNote>,
    pub onset_ticks: u32,
    pub duration_ticks: u32,
    pub pitch: u8,
    pub lyric: ProjectedLyric,
    /// Original ownership, captured before lyric transforms and moved with the
    /// note through filtering and lane splitting. Never serialized by a target.
    /// `None` is reserved for synthetic analysis notes and hand-built fixtures.
    pub source_evidence: Option<NoteEvidence>,
}

/// The source items represented by one editable note. Geometry is not identity:
/// distinct source notes can share pitch, onset and duration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteEvidence {
    pub note_id: String,
    pub note_on_event_id: String,
    pub note_off_event_id: String,
    pub lyric_id: Option<String>,
    pub lyric_event_id: Option<String>,
    /// Universal adapter ownership, independent of optional performance and of
    /// the technical lane where this representation is eventually sung.
    pub origin: Option<NoteOrigin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteOrigin {
    pub track_id: String,
    pub note_on_order: u32,
    pub note_off_order: u32,
    pub source: crate::engine::midi::NoteSource,
    /// A selected contradictory row cannot authorize sung continuity.
    pub lyric_conflict: bool,
    pub continuation: Option<ContinuationOwner>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContinuationOwner {
    pub predecessor_id: String,
    pub lyric_owner_id: String,
    pub destination_track_id: String,
    pub kind: ContinuationKind,
    pub intensity_attack_note_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinuationKind {
    Tie,
    Extension,
}

impl NoteEvidence {
    pub fn source_ids(&self) -> impl Iterator<Item = &str> {
        [
            Some(self.note_id.as_str()),
            Some(self.note_on_event_id.as_str()),
            Some(self.note_off_event_id.as_str()),
            self.lyric_id.as_deref(),
            self.lyric_event_id.as_deref(),
        ]
        .into_iter()
        .flatten()
    }
}

/// What the source says this note sings.
///
/// Deliberately not a string: `-` and `+` are target vocabulary, and they do
/// not agree between targets — Synthesizer V reads `-` as a continuation while
/// OpenUtau reads `+` as one. Rendering a marker here would corrupt the other
/// target, so each target renders its own from [`crate::engine::midi::LyricState`].
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectedLyric {
    /// A source lyric, carried whole so no evidence is lost on the way out.
    /// Boxed because it dwarfs the other two variants, which carry no data.
    Source(Box<Lyric>),
    /// An explicit, verified reading of a source syllable. The source object is
    /// never rewritten; target spelling and phonemes are separate evidence.
    Pronounced {
        source: Box<Lyric>,
        text: String,
        phonemes: String,
    },
    /// The next source-owned syllable of an explicitly recognized word.
    /// The target uses its syllable-split marker; the original lyric survives.
    PronouncedSplit { source: Box<Lyric> },
    /// A source lyric extension carries the previous syllable onto this note.
    /// There is no lyric object of its own; the source stated the extension on
    /// a neighbour, as a MusicXML `<extend>` or a MuseScore extension length.
    Extension,
    /// The source says nothing here. Absence is not the same as an
    /// `ExplicitEmpty` lyric, which is the source stating that nothing is sung.
    Absent,
}

impl ProjectedLyric {
    /// Original lyric identity survives spelling and pronunciation transforms.
    pub fn source_identity(&self) -> Option<&str> {
        match self {
            Self::Source(source)
            | Self::Pronounced { source, .. }
            | Self::PronouncedSplit { source } => Some(&source.id),
            Self::Extension | Self::Absent => None,
        }
    }

    fn sung_owner_identity(&self) -> Option<&str> {
        match self {
            // The word joiner keeps the original lyric identity/raw text but
            // rewrites later syllables to split markers. Such a syllable still
            // owns its source-proven melisma after pronunciation preparation.
            Self::Source(source)
                if matches!(source.state, LyricState::SyllableSplit)
                    && !source.raw.trim().is_empty() =>
            {
                Some(&source.id)
            }
            Self::Source(source)
            | Self::Pronounced { source, .. }
            | Self::PronouncedSplit { source }
                if matches!(&source.state, LyricState::Text(text) if !text.trim().is_empty()) =>
            {
                Some(&source.id)
            }
            _ => None,
        }
    }

    /// Whether this lyric carries the previous note's syllable onto this note.
    ///
    /// Every target spells the marker differently — `-` and `+~` and `+` — but
    /// none can state one across a gap, so all of them need the previous note to
    /// end exactly where this one begins. That dependency is a property of the
    /// projection, not of one format, and it lives here so the converter and the
    /// targets cannot drift apart on which notes carry it.
    pub fn continues_previous_note(&self) -> bool {
        match self {
            ProjectedLyric::Extension => true,
            ProjectedLyric::Source(source) => matches!(
                source.state,
                LyricState::Continuation | LyricState::SyllableSplit
            ),
            ProjectedLyric::Absent => false,
            ProjectedLyric::Pronounced { .. } => false,
            ProjectedLyric::PronouncedSplit { .. } => true,
        }
    }

    /// Whether the source asks for this note to be sung at all.
    ///
    /// `Unsupported` is sung: a humming or laughing vocalization is a sound the
    /// score asks for, only one no target can spell. `ExplicitEmpty` is the
    /// opposite — the source stating that nothing is sung here — and `Absent` is
    /// the source stating nothing at all.
    pub fn is_sung(&self) -> bool {
        match self {
            ProjectedLyric::Extension => true,
            ProjectedLyric::Source(source) => !matches!(source.state, LyricState::ExplicitEmpty),
            ProjectedLyric::Absent => false,
            ProjectedLyric::Pronounced { .. } => true,
            ProjectedLyric::PronouncedSplit { .. } => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(onset_ticks: u32, duration_ticks: u32, pitch: u8) -> ProjectedNote {
        ProjectedNote {
            performance: None,
            source_evidence: None,
            onset_ticks,
            duration_ticks,
            pitch,
            lyric: ProjectedLyric::Absent,
        }
    }

    fn lane(notes: Vec<ProjectedNote>) -> ProjectedProject {
        ProjectedProject {
            pronunciation_profile: Default::default(),
            ticks_per_beat: 480,
            tracks: vec![ProjectedTrack {
                name: "Voice".into(),
                source_track_id: "voice".into(),
                muted: false,
                notes,
            }],
            ..ProjectedProject::default()
        }
    }

    /// Two notes sounding at once in one lane is what a score adapter used to
    /// hand over before it decomposed simultaneity. Synthesizer V accepts the
    /// stack and sings one note of it, so nothing downstream reports the loss.
    #[test]
    fn a_lane_sounding_two_notes_at_once_is_reported_with_both_ticks() {
        let project = lane(vec![note(0, 480, 60), note(240, 480, 64)]);
        assert_eq!(
            project.monophony_violation().as_deref(),
            Some(
                "the note at MIDI tick 0 on source track voice still sounds at tick 240, where \
                 the next note of the same lane begins"
            )
        );
    }

    /// A note ending exactly where the next begins is the ordinary shape of a
    /// sung line, and of every continuation marker: both targets require the
    /// two to touch, so touching must never read as an overlap.
    #[test]
    fn notes_that_touch_are_not_sounding_at_once() {
        let project = lane(vec![note(0, 480, 60), note(480, 480, 64)]);
        assert_eq!(project.monophony_violation(), None);
    }

    /// Asked of any projection, including one built by hand through the public
    /// API, so the producer's own ordering cannot hide a stack.
    #[test]
    fn an_overlap_is_found_whatever_order_the_projection_held() {
        let project = lane(vec![note(240, 480, 64), note(0, 480, 60)]);
        assert!(project.monophony_violation().is_some());
    }

    /// A lane's last note may legitimately end past the tick range; computing
    /// its end in the same width would abort the process during analysis.
    #[test]
    fn a_note_ending_past_the_tick_range_is_measured_without_overflowing() {
        let project = lane(vec![note(u32::MAX - 1, 480, 60)]);
        assert_eq!(project.monophony_violation(), None);
    }
}
