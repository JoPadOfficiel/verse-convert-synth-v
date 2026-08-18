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

/// One source, projected. A target consumes this and nothing else.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectedProject {
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
    pub onset_ticks: u32,
    pub duration_ticks: u32,
    pub pitch: u8,
    pub lyric: ProjectedLyric,
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
    /// A source lyric extension carries the previous syllable onto this note.
    /// There is no lyric object of its own; the source stated the extension on
    /// a neighbour, as a MusicXML `<extend>` or a MuseScore extension length.
    Extension,
    /// The source says nothing here. Absence is not the same as an
    /// `ExplicitEmpty` lyric, which is the source stating that nothing is sung.
    Absent,
}

impl ProjectedLyric {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(onset_ticks: u32, duration_ticks: u32, pitch: u8) -> ProjectedNote {
        ProjectedNote {
            onset_ticks,
            duration_ticks,
            pitch,
            lyric: ProjectedLyric::Absent,
        }
    }

    fn lane(notes: Vec<ProjectedNote>) -> ProjectedProject {
        ProjectedProject {
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
