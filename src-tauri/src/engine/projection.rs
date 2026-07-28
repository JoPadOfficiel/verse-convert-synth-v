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
use crate::engine::midi::Lyric;

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
    pub source_track_id: String,
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
