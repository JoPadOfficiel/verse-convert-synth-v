//! OpenUtau project (.ustx) structures, `ustx_version` 0.6.
//!
//! This is the only module that knows what OpenUtau wants: 480 integer ticks
//! per quarter, `"+~"` for a held syllable and `"+"` for a syllable split,
//! `lyric: ""` for a note the source never texted, the mandatory non-empty
//! `pitch.data`, the 10-tick duration floor, and the `ustx_version` floor that
//! keeps a time map alive.
//! Everything above it works in source-exact IR ticks through
//! [`ProjectedProject`].
//!
//! The format facts below were read from the OpenUtau `0.1.568` sources, not
//! from documentation, and each one is cited at the line that depends on it.
use crate::engine::midi::LyricState;
use crate::engine::projection::{
    ProjectedLyric, ProjectedNote, ProjectedProject, ProjectedTempo, ProjectedTrack,
};
use std::collections::BTreeMap;

/// One quarter note. `UProject.resolution` is `[YamlIgnore] => 480`, so the
/// emitted `resolution:` is ignored on load and rescaling is impossible: this
/// is a hard property of the format, not a tuning constant.
pub const TICKS_PER_QUARTER: u32 = 480;

/// The lowest version that may ever be emitted, and the version emitted.
///
/// `Ustx.Load` replaces the whole `time_signatures` and `tempos` lists with one
/// entry each, taken from the obsolete `bpm`/`beat_per_bar`/`beat_unit` scalars,
/// for any project declaring less than `0.6`. Every tempo and meter change in
/// the score would be destroyed on load. `0.6` is accepted and upgraded in place.
pub const USTX_VERSION: &str = "0.6";

/// `UNote.Validate` does `duration = Math.Max(10, duration)`, so a shorter note
/// is silently lengthened. Verse refuses instead: a lengthened note is silent
/// loss, and a refusal is not.
pub const MIN_NOTE_TICKS: i32 = 10;

/// OpenUtau's own default phonemizer, named because a track must carry one and
/// this is the one a new OpenUtau project uses. It asserts nothing about a
/// voicebank, which Verse has never seen.
pub const DEFAULT_PHONEMIZER: &str = "OpenUtau.Core.DefaultPhonemizer";

/// A held syllable: the previous vowel sustained across this note.
///
/// OpenUtau spells this `+~`, not `+`. `MusicXML.cs:157-160` writes `+~` for a
/// slur, commented "OpenUtau uses +~ for extending the current syllable", and
/// `MidiWriter.cs:209-211` converts an imported MIDI `-` into `+~`, which
/// `:272-274` converts back to `-` on export. Synthesizer V spells the same idea
/// `-`, so the two vocabularies must never be swapped.
pub const HELD_SYLLABLE: &str = "+~";

/// The next syllable of one multi-syllable word.
///
/// `MusicXML.cs:147-149` writes `+` here, commented "For multisyllable words,
/// OpenUtau use `+` to place the following syllables", and
/// `NotePresets.SplittedLyric` is `"+"`. Synthesizer V uses `+` for this too, so
/// it is the one marker the two targets agree on.
pub const SYLLABLE_SPLIT: &str = "+";

/// `UVoicePart.Validate` sets `Extends` on any lyric starting with `"+"`, which is
/// both markers above, so both need the note to touch its predecessor.
pub const MARKER_PREFIX: &str = "+";

#[derive(Clone, Debug, PartialEq)]
pub struct UstxProject {
    /// Emitted as a string scalar: `UProject.ustxVersion` is a `Version`
    /// declared `[YamlMember(SerializeAs = typeof(string))]`.
    pub ustx_version: String,
    pub resolution: u32,
    /// Obsolete since ustx v0.6 and ignored at this version. Written anyway, set
    /// from the first tempo and meter, so that a mistaken downgrade loses the
    /// later changes rather than corrupting the opening of the score.
    pub bpm: f64,
    pub beat_per_bar: u32,
    pub beat_unit: u32,
    pub time_signatures: Vec<UstxTimeSignature>,
    pub tempos: Vec<UstxTempo>,
    pub tracks: Vec<UstxTrack>,
    pub voice_parts: Vec<UstxVoicePart>,
    /// Audio parts. A projection carries no audio, so [`serialize`] always leaves
    /// this empty; only a preservation bundle, which owns real rendered WAVs, adds
    /// one through [`append_wave_part`].
    pub wave_parts: Vec<UstxWavePart>,
}

/// Bar-indexed, exactly like Synthesizer V's meter, so no arithmetic converts it.
/// `UTimeSignature.barPosition` is a C# `int`, which is the one thing the writer
/// still has to bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UstxTimeSignature {
    pub bar_position: i32,
    pub beat_per_bar: u32,
    pub beat_unit: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UstxTempo {
    /// `UTempo.position` is a C# `int`, which is what bounds the tick range.
    pub position: i32,
    pub bpm: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UstxTrack {
    pub phonemizer: String,
    pub track_name: String,
    pub mute: bool,
    pub solo: bool,
    pub volume: f64,
}

/// One monophonic lane. `renderer_settings` and `singer` are deliberately
/// absent: `URenderSettings.Validate` resolves the renderer from whatever singer
/// the user assigns, and naming one would assert something about a voicebank
/// Verse never saw.
#[derive(Clone, Debug, PartialEq)]
pub struct UstxVoicePart {
    pub name: String,
    pub track_no: i32,
    pub position: i32,
    pub notes: Vec<UstxNote>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UstxNote {
    pub position: i32,
    pub duration: i32,
    pub tone: u8,
    pub lyric: String,
    pub pitch: UstxPitch,
    pub vibrato: UstxVibrato,
}

/// `UNote.Validate` dereferences `pitch.data[0]` with no guard, so `data` must
/// carry at least one point. The two `y: 0` points below are that structural
/// requirement, not musical invention.
#[derive(Clone, Debug, PartialEq)]
pub struct UstxPitch {
    pub data: Vec<UstxPitchPoint>,
    pub snap_first: bool,
}

impl Default for UstxPitch {
    fn default() -> Self {
        UstxPitch {
            data: vec![
                UstxPitchPoint {
                    x: -1.0,
                    y: 0.0,
                    shape: PitchShape::Io,
                },
                UstxPitchPoint {
                    x: 1.0,
                    y: 0.0,
                    shape: PitchShape::Io,
                },
            ],
            snap_first: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UstxPitchPoint {
    /// Milliseconds relative to the note onset, per `PitchPoint.X`.
    pub x: f64,
    /// Tenths of a semitone relative to the note's tone, per `PitchPoint.Y`.
    pub y: f64,
    pub shape: PitchShape,
}

/// `PitchPointShape` is a closed C# enum, not a string, which is why it is the
/// one scalar emitted as a bare token: it can never carry source text, so the
/// always-double-quote rule that protects arbitrary lyrics does not apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PitchShape {
    /// `SineInOut`, the shape OpenUtau itself writes for a default portamento.
    Io,
}

impl PitchShape {
    fn token(self) -> &'static str {
        match self {
            PitchShape::Io => "io",
        }
    }
}

/// `length: 0` disables vibrato. Every other field is the value OpenUtau's own
/// `UVibrato` initializes, so this states no expression at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UstxVibrato {
    pub length: f64,
    pub period: f64,
    pub depth: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    pub shift: f64,
    pub drift: f64,
}

impl Default for UstxVibrato {
    fn default() -> Self {
        UstxVibrato {
            length: 0.0,
            period: 175.0,
            depth: 25.0,
            fade_in: 10.0,
            fade_out: 10.0,
            shift: 0.0,
            drift: 0.0,
        }
    }
}

/// One audio part: a WAV the project plays as it stands, with no note, no lyric
/// and no singer.
///
/// `UWavePart.AfterLoad` resolves the file as
/// `Path.GetFullPath(Path.Combine(Path.GetDirectoryName(project.FilePath), relativePath))`,
/// so `relative_path` is relative to the `.ustx` file's **own directory** and
/// `../audio/…` reaches out of the directory the project sits in — exactly as the
/// Synthesizer V case does.
#[derive(Clone, Debug, PartialEq)]
pub struct UstxWavePart {
    /// Discarded on load: the `FilePath` setter assigns
    /// `name = Path.GetFileName(value)` and `AfterLoad` sets `FilePath` from
    /// `relative_path`, so OpenUtau always shows the file's own name whatever this
    /// says. [`append_wave_part`] therefore writes exactly that, and the
    /// human-readable label lives on the track, where it survives.
    pub name: String,
    pub comment: String,
    /// `UProject.AfterLoad` indexes `tracks[part.trackNo]` with no bounds check,
    /// so every wave part must name a track the project actually holds.
    pub track_no: i32,
    /// Ticks, on the same 480-per-quarter grid as a note.
    pub position: i32,
    pub relative_path: String,
    /// Milliseconds. `UWavePart.fileDurationMs` is a C# `double`; `Load` replaces
    /// it with the opened file's own length and falls back to what is written here
    /// only when the file cannot be opened.
    pub file_duration_ms: f64,
    /// Ticks cut from the head of the file, per `GetSkipMs`.
    pub skip: i32,
    /// Ticks cut from the tail of the file, per `GetTrimMs`.
    pub trim: i32,
    /// Ticks of fade applied by `TrimSamples`.
    pub fadein: i32,
    pub fadeout: i32,
}

/// The declared length of a wave part, in milliseconds, from the two integers a
/// WAV header states.
///
/// Derived from `frames` and `sample_rate` and never from a seconds-valued float:
/// `frames * 1000` is exact in an `f64` for any WAV a bundle accepts, so a single
/// division yields the correctly rounded value of the exact rational, while
/// multiplying an already-rounded seconds quotient by 1000 rounds twice and can
/// land on a different double.
pub fn file_duration_ms(frames: u64, sample_rate: u32) -> f64 {
    frames as f64 * 1000.0 / f64::from(sample_rate)
}

/// The name OpenUtau keeps for a wave part: the file's own name, because
/// `UWavePart.FilePath`'s setter assigns `name = Path.GetFileName(value)` and
/// `AfterLoad` sets `FilePath` from `relative_path`. Bundle references are always
/// `/`-joined, which is the separator this splits on.
fn wave_part_name(relative_path: &str) -> String {
    relative_path
        .rsplit('/')
        .next()
        .unwrap_or(relative_path)
        .to_string()
}

/// Adds one real audio-backed wave part, and the track it sits on, to a project
/// this module already built.
///
/// The track is not optional: `UProject.AfterLoad` dereferences
/// `tracks[part.trackNo]` unguarded, so a wave part without one makes the project
/// unopenable. It carries the mute state because OpenUtau has no per-part mute —
/// the same place Synthesizer V keeps it, on the track rather than on the
/// reference.
///
/// Every other field states "play this file from the start, whole": `position`,
/// `skip`, `trim`, `fadein` and `fadeout` are all `0`, which is the same claim the
/// Synthesizer V path makes with `blickOffset: 0`. Returns the `track_no` the part
/// was placed on.
pub fn append_wave_part(
    project: &mut UstxProject,
    track_name: String,
    relative_path: String,
    frames: u64,
    sample_rate: u32,
    muted: bool,
) -> Result<i32, String> {
    // `UPart.trackNo` is a C# `int`, so a project with more lanes than that range
    // must be refused rather than wrapped onto another track's audio.
    let track_no = i32::try_from(project.tracks.len()).map_err(|_| {
        "the project holds more tracks than OpenUtau stores a track index in".to_string()
    })?;
    let file_duration_ms = file_duration_ms(frames, sample_rate);
    // `validate_wav` refuses a zero sample rate or frame count before a bundle
    // ever reaches here, but a length YAML cannot state as a number must not be
    // written even so: YamlDotNet would read `inf` back as a string.
    if !file_duration_ms.is_finite() {
        return Err(format!(
            "{relative_path} states no length OpenUtau can hold in milliseconds \
             ({frames} frames at {sample_rate} Hz)"
        ));
    }
    project.tracks.push(UstxTrack {
        phonemizer: DEFAULT_PHONEMIZER.into(),
        track_name,
        mute: muted,
        solo: false,
        volume: 0.0,
    });
    project.wave_parts.push(UstxWavePart {
        name: wave_part_name(&relative_path),
        comment: String::new(),
        track_no,
        position: 0,
        relative_path,
        file_duration_ms,
        skip: 0,
        trim: 0,
        fadein: 0,
        fadeout: 0,
    });
    Ok(track_no)
}

/// Converts a tick **quantity** onto OpenUtau's fixed 480-per-quarter grid — a
/// position and a duration alike, the map being linear with no offset. Timing
/// that does not land exactly on that grid is refused, never rounded: OpenUtau's
/// own MusicXML importer truncates the same case with
/// `(int)note.Duration * 480 / divisions`, so refusing is strictly more faithful
/// than the software's native behaviour.
fn exact_ustx_ticks(ticks: u32, ticks_per_beat: u16, context: &str) -> Result<i32, String> {
    if ticks_per_beat == 0 {
        return Err("MIDI PPQ division must be non-zero".into());
    }
    let numerator = u128::from(ticks) * u128::from(TICKS_PER_QUARTER);
    let denominator = u128::from(ticks_per_beat);
    if numerator % denominator != 0 {
        return Err(format!(
            "{context} at MIDI tick {ticks} cannot be represented exactly in OpenUtau's 480 ticks \
             per quarter with PPQ {ticks_per_beat}"
        ));
    }
    // Every USTX position is a C# `int`, so the grid is exact but bounded, and a
    // score longer than that range is refused rather than wrapped.
    i32::try_from(numerator / denominator)
        .map_err(|_| format!("{context} exceeds the OpenUtau tick range"))
}

/// OpenUtau's marker vocabulary. The projection carries [`LyricState`] instead
/// of this text because the two markers do not agree between targets, and because
/// OpenUtau spells the two ideas with two different markers where Synthesizer V
/// uses `-` and `+`. The match is exhaustive with no `_` arm so that a new source
/// state cannot silently fall into a wrong marker.
fn lyric_text(lyric: &ProjectedLyric) -> String {
    match lyric {
        ProjectedLyric::Source(source) => match &source.state {
            LyricState::Text(text) => text.clone(),
            // The source states that the previous syllable is *held* across this
            // note. OpenUtau spells a hold `+~`, not `+`: `MusicXML.cs:157-160`
            // writes `+~` for a slur ("extending the current syllable"), and
            // `MidiWriter.cs:209-211` turns an imported MIDI `-` into `+~`, which
            // `:272-274` turns back into `-` on export. `+` is a different idea
            // (see below), so writing it here would hand the phonemizer a
            // split-syllable group instead of a sustained vowel.
            LyricState::Continuation => HELD_SYLLABLE.into(),
            // The next syllable of one multi-syllable word. This is exactly what
            // OpenUtau's `+` means — `MusicXML.cs:147-149`, "For multisyllable
            // words, OpenUtau use + to place the following syllables", and
            // `NotePresets.SplittedLyric = "+"`. Synthesizer V spells it `+` too,
            // so the two targets happen to agree on this one marker alone.
            LyricState::SyllableSplit => SYLLABLE_SPLIT.into(),
            // The source states that nothing is sung, or states a vocalization
            // no target can express. `lyric: ""` is a state no OpenUtau importer
            // can produce, and it is the only honest one: `"a"` invents a
            // syllable, `"+~"` claims a hold, `"R"` claims a rest.
            LyricState::ExplicitEmpty | LyricState::Unsupported(_) => String::new(),
        },
        // The second encoding of a hold: a MusicXML `<extend>` or a MuseScore
        // extension length stated on a neighbouring note, with no lyric object of
        // its own. It must render exactly like `LyricState::Continuation`.
        ProjectedLyric::Extension => HELD_SYLLABLE.into(),
        ProjectedLyric::Absent => String::new(),
    }
}

/// Whether this lyric is a marker Verse rendered rather than text the source
/// carries. Both markers need the note to touch its predecessor, and neither can
/// be recognised by comparing the emitted string: a source word may spell `"+"`
/// or `"+~"` itself, and that word is text, not a marker.
fn is_rendered_marker(lyric: &ProjectedLyric) -> bool {
    match lyric {
        ProjectedLyric::Extension => true,
        ProjectedLyric::Source(source) => matches!(
            source.state,
            LyricState::Continuation | LyricState::SyllableSplit
        ),
        ProjectedLyric::Absent => false,
    }
}

/// Reports text that OpenUtau will read as something other than the word it
/// spells, so the projection can diagnose it.
///
/// The file states the source text exactly, byte for byte — that is what the
/// always-double-quote emitter guarantees, and it is not negotiable. What this
/// function covers is the *reading*: two OpenUtau behaviours reinterpret a lyric
/// before it is sung, and neither has an escape in the format.
///
/// - `UVoicePart.Validate` sets `Extends` on any lyric starting with `"+"`, so a
///   source word that genuinely begins with `+` becomes a continuation of the
///   previous note instead of a word of its own.
/// - `UNote.ToPhonemizerNote` runs `phoneticHintPattern.Replace` over the lyric
///   with `\[(.*)\]`, so bracketed source text is taken as a phonetic hint and
///   stripped before the phonemizer ever sees it.
///
/// Refusing a whole score over one word would be disproportionate, and silence
/// would be silent loss, so this is diagnosed and the text is written unchanged.
///
/// Call it only on text the **source** carries. A `"+"` that Verse rendered
/// itself as a continuation marker is not a source word and must not be
/// diagnosed — [`lyric_text`] is the only thing that produces those.
pub fn lyric_reinterpretation(text: &str) -> Option<String> {
    // Both readings can apply to one lyric — `"+sing [hint]"` is extended *and*
    // stripped — so neither branch may return early and hide the other.
    let mut readings: Vec<String> = Vec::new();
    if text.starts_with(MARKER_PREFIX) {
        readings.push(
            "read it as a continuation of the previous note rather than as the word it spells, \
             because it treats any lyric starting with \"+\" as an extension"
                .to_string(),
        );
    }
    // `.` does not match a line feed in .NET without `RegexOptions.Singleline`,
    // so the hint only applies to a `[` and a `]` on one line, in that order.
    if text.split('\n').any(|line| {
        line.split_once('[')
            .is_some_and(|(_, rest)| rest.contains(']'))
    }) {
        readings.push(
            "strip its bracketed part as a phonetic hint before phonemizing, because it replaces \
             every `[...]` in a lyric"
                .to_string(),
        );
    }
    if readings.is_empty() {
        return None;
    }
    Some(format!(
        "OpenUtau will {} for the source lyric \"{text}\". The text is written exactly as the \
         source states it; only OpenUtau's reading of it differs, and the format offers no way to \
         escape it.",
        readings.join(", and will ")
    ))
}

/// The single entry point of this target: one neutral projection in, one
/// OpenUtau 0.6 project out.
pub fn serialize(project: &ProjectedProject) -> Result<UstxProject, String> {
    // Unreachable from `convert_midi_with`, which refuses a zero PPQ before it
    // ever projects, but `serialize` is public and a hand-built projection must
    // not silently produce a file whose every position went unvalidated.
    if project.ticks_per_beat == 0 {
        return Err("MIDI PPQ division must be non-zero".into());
    }
    // `UProject`'s constructor guarantees exactly one tempo and one time
    // signature, and an explicit empty list in the file *clears* that default:
    // `Validate` then builds the time axis from nothing. `read_tempo` and
    // `read_meter` both floor at one entry, so this is unreachable from
    // `convert_midi_with_target` — but `serialize` is public, and a projection
    // that states no time base must be refused rather than written.
    if project.tempos.is_empty() {
        return Err("a project must state at least one tempo".into());
    }
    if project.meters.is_empty() {
        return Err("a project must state at least one time signature".into());
    }
    // Note timing is refused before tempo timing, which is the order Verse has
    // always surfaced these two in.
    let mut tracks = Vec::with_capacity(project.tracks.len());
    let mut voice_parts = Vec::with_capacity(project.tracks.len());
    for (index, track) in project.tracks.iter().enumerate() {
        let track_no = i32::try_from(index)
            .map_err(|_| "projected lane count exceeds the OpenUtau track range".to_string())?;
        tracks.push(UstxTrack {
            phonemizer: DEFAULT_PHONEMIZER.into(),
            track_name: track.name.clone(),
            mute: false,
            solo: false,
            volume: 0.0,
        });
        voice_parts.push(serialize_voice_part(
            track_no,
            track,
            project.ticks_per_beat,
        )?);
    }
    // Same shape as the Synthesizer V target: refuse a position while walking
    // the source in discovery order, so the event a refusal names is the one the
    // source revealed first, then collect what survives into a map keyed by
    // position so the emitted list ascends whatever order the projection held.
    let mut by_tick: BTreeMap<u32, UstxTempo> = BTreeMap::new();
    for source in project.tempos_in_discovery_order() {
        by_tick.insert(
            source.tick,
            serialize_tempo(source, project.ticks_per_beat)?,
        );
    }
    let tempos: Vec<UstxTempo> = by_tick.into_values().collect();
    // Meter needs no arithmetic: OpenUtau indexes it by bar, exactly as
    // Synthesizer V does, and the projection already carries a bar index.
    // Collected through a map keyed by bar, exactly as the tempos are: OpenUtau
    // sorts `time_signatures` by bar on load and would keep both entries of a
    // duplicated bar, so the emitted list must already hold one per bar in bar
    // order rather than trusting the producer to have done it.
    let mut by_bar: BTreeMap<i32, UstxTimeSignature> = BTreeMap::new();
    for meter in &project.meters {
        // `UTimeSignature.barPosition` is a C# `int`. `ProjectedMeter.bar_index` is
        // a `u32` and `read_meter` allows its whole range, so a bar index above
        // 2^31 would be written and then fail to deserialize, leaving OpenUtau
        // unable to open the file at all.
        let bar_position = i32::try_from(meter.bar_index).map_err(|_| {
            format!(
                "the time signature at bar {} lies beyond the 32-bit bar range OpenUtau stores a \
                 bar position in",
                meter.bar_index
            )
        })?;
        by_bar.insert(
            bar_position,
            UstxTimeSignature {
                bar_position,
                beat_per_bar: meter.numerator,
                beat_unit: meter.denominator,
            },
        );
    }
    let time_signatures: Vec<UstxTimeSignature> = by_bar.into_values().collect();
    // The obsolete scalars restate the opening of the emitted lists. When a list
    // is empty there is nothing to restate, so the value OpenUtau's own
    // `UProject` fields already hold is written: it makes no claim about a score
    // that stated no tempo or no meter.
    let opening = time_signatures.first();
    Ok(UstxProject {
        ustx_version: USTX_VERSION.into(),
        resolution: TICKS_PER_QUARTER,
        bpm: tempos.first().map_or(120.0, |tempo| tempo.bpm),
        beat_per_bar: opening.map_or(4, |meter| meter.beat_per_bar),
        beat_unit: opening.map_or(4, |meter| meter.beat_unit),
        time_signatures,
        tempos,
        tracks,
        voice_parts,
        // A projection carries notes and lyrics, never audio. Real rendered WAVs
        // exist only inside a preservation bundle, which appends them itself.
        wave_parts: Vec::new(),
    })
}

fn serialize_tempo(tempo: &ProjectedTempo, ticks_per_beat: u16) -> Result<UstxTempo, String> {
    // A source carrying no tempo event at all implies 120 BPM at tick 0, which
    // is exactly representable, so the unnamed case never actually refuses.
    let context = match &tempo.source {
        Some(source) => format!("tempo event {source}"),
        None => "tempo".to_string(),
    };
    // `read_tempo` only ever derives a BPM from a non-zero microseconds-per-
    // quarter, so this is unreachable from the converter. `serialize` is public,
    // though, and a value YAML cannot state as a number must not be written.
    if !tempo.bpm.is_finite() {
        return Err(format!(
            "{context} carries a tempo that is not a finite BPM"
        ));
    }
    Ok(UstxTempo {
        bpm: tempo.bpm,
        position: exact_ustx_ticks(tempo.tick, ticks_per_beat, &context)?,
    })
}

fn serialize_voice_part(
    track_no: i32,
    track: &ProjectedTrack,
    ticks_per_beat: u16,
) -> Result<UstxVoicePart, String> {
    let mut notes = Vec::with_capacity(track.notes.len());
    for note in &track.notes {
        notes.push((
            note.onset_ticks,
            is_rendered_marker(&note.lyric),
            serialize_note(note, &track.source_track_id, ticks_per_beat)?,
        ));
    }
    // `UNote.CompareTo` falls back to `GetHashCode()` at equal positions, so the
    // order OpenUtau loads a part in is only defined while positions ascend.
    // Sorting is stable and claims nothing: it reorders no overlapping pair,
    // because an overlapping pair is refused immediately below.
    notes.sort_by_key(|(_, _, note)| note.position);
    for pair in notes.windows(2) {
        let (previous_onset, _, previous) = &pair[0];
        let (onset, marker, note) = &pair[1];
        // Checked because both operands only have to fit `i32` on their own:
        // `UNote.position` and `UNote.duration` are each a C# `int`, so two late
        // notes can each pass `exact_ustx_ticks` while their sum does not. The
        // release profile sets `overflow-checks = true`, so an unchecked add here
        // would abort the process during analysis, on merely adding a file.
        let previous_end = previous
            .position
            .checked_add(previous.duration)
            .ok_or_else(|| {
                format!(
                    "the note at MIDI tick {previous_onset} on source track {} ends beyond the \
                     32-bit tick range OpenUtau stores a position in",
                    track.source_track_id
                )
            })?;
        if previous_end > note.position {
            return Err(format!(
                "notes at MIDI ticks {previous_onset} and {onset} on source track {} overlap; one \
                 OpenUtau voice part is monophonic and marks the later note with an overlap error \
                 instead of singing it",
                track.source_track_id
            ));
        }
        // `UVoicePart.Validate` wires `Extends` only when `Prev.End == position`.
        // A marker on a note that does not touch its predecessor stays unwired and
        // reaches the phonemizer as a lyric, so the hold or the split is lost and
        // the marker itself is sung as a word. OpenUtau cannot state either across
        // a gap. Tested on provenance, not on the emitted string: a source word may
        // spell `"+"` or `"+~"` itself, and such a word is text, not a marker.
        if *marker && previous_end != note.position {
            return Err(format!(
                "the note at MIDI tick {onset} on source track {} carries the marker \"{}\", which \
                 continues the previous note, but does not begin where that note ends; OpenUtau \
                 only carries a syllable across notes that touch and would sing the marker as a \
                 word instead",
                track.source_track_id, note.lyric
            ));
        }
    }
    // The same trap on the first note of a lane: it has no predecessor at all, so a
    // marker there can never be wired to anything.
    if let Some((onset, marker, first)) = notes.first() {
        if *marker {
            return Err(format!(
                "the first note at MIDI tick {onset} on source track {} carries the marker \
                 \"{}\", which continues a previous note, but nothing precedes it in this \
                 OpenUtau voice part",
                track.source_track_id, first.lyric
            ));
        }
    }
    Ok(UstxVoicePart {
        name: track.name.clone(),
        track_no,
        // Note positions are relative to the part, so a part at 0 keeps every
        // position identical to the projection's own timeline.
        position: 0,
        notes: notes.into_iter().map(|(_, _, note)| note).collect(),
    })
}

fn serialize_note(
    note: &ProjectedNote,
    source_track_id: &str,
    ticks_per_beat: u16,
) -> Result<UstxNote, String> {
    let position = exact_ustx_ticks(
        note.onset_ticks,
        ticks_per_beat,
        &format!("note onset on source track {source_track_id}"),
    )?;
    let duration = exact_ustx_ticks(
        note.duration_ticks,
        ticks_per_beat,
        &format!("note duration on source track {source_track_id}"),
    )?;
    if duration < MIN_NOTE_TICKS {
        return Err(format!(
            "note duration on source track {source_track_id} at MIDI tick {} is {duration} \
             OpenUtau ticks, under the {MIN_NOTE_TICKS}-tick floor OpenUtau silently lengthens a \
             note to",
            note.onset_ticks
        ));
    }
    Ok(UstxNote {
        position,
        duration,
        tone: note.pitch,
        lyric: lyric_text(&note.lyric),
        pitch: UstxPitch::default(),
        vibrato: UstxVibrato::default(),
    })
}

/// Emits the project as UTF-8 YAML bytes.
///
/// Hand-written on purpose: the schema is closed and tiny, byte-exact output is
/// testable, and unconditionally double-quoting every string scalar removes
/// every YAML ambiguity for arbitrary lyric text — a `:`, a `#`, a quote, a
/// backslash, a leading space — without a dependency or a plain-scalar
/// heuristic. Keys belong to the closed schema and are emitted bare, exactly as
/// OpenUtau's own serializer writes them.
pub fn to_yaml(project: &UstxProject) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "ustx_version: {}\n",
        quoted(&project.ustx_version)
    ));
    out.push_str(&format!("resolution: {}\n", project.resolution));
    out.push_str(&format!("bpm: {}\n", number(project.bpm)));
    out.push_str(&format!("beat_per_bar: {}\n", project.beat_per_bar));
    out.push_str(&format!("beat_unit: {}\n", project.beat_unit));
    out.push_str(&flow_list(
        "time_signatures",
        &project.time_signatures,
        |meter| {
            format!(
                "{{bar_position: {}, beat_per_bar: {}, beat_unit: {}}}",
                meter.bar_position, meter.beat_per_bar, meter.beat_unit
            )
        },
    ));
    out.push_str(&flow_list("tempos", &project.tempos, |tempo| {
        format!(
            "{{position: {}, bpm: {}}}",
            tempo.position,
            number(tempo.bpm)
        )
    }));
    // `Ustx.Load` calls `AddDefaultExpressions` on every load, so an empty map
    // is complete: authoring expressions would state something the source never
    // said about a voicebank Verse never saw.
    out.push_str("expressions: {}\n");
    if project.tracks.is_empty() {
        out.push_str("tracks: []\n");
    } else {
        out.push_str("tracks:\n");
        for track in &project.tracks {
            out.push_str(&format!("  - phonemizer: {}\n", quoted(&track.phonemizer)));
            out.push_str(&format!("    track_name: {}\n", quoted(&track.track_name)));
            out.push_str(&format!("    mute: {}\n", track.mute));
            out.push_str(&format!("    solo: {}\n", track.solo));
            out.push_str(&format!("    volume: {}\n", number(track.volume)));
        }
    }
    if project.voice_parts.is_empty() {
        out.push_str("voice_parts: []\n");
    } else {
        out.push_str("voice_parts:\n");
        for part in &project.voice_parts {
            out.push_str(&format!("  - name: {}\n", quoted(&part.name)));
            out.push_str(&format!("    track_no: {}\n", part.track_no));
            out.push_str(&format!("    position: {}\n", part.position));
            if part.notes.is_empty() {
                out.push_str("    notes: []\n");
            } else {
                out.push_str("    notes:\n");
                for note in &part.notes {
                    out.push_str(&format!("      - position: {}\n", note.position));
                    out.push_str(&format!("        duration: {}\n", note.duration));
                    out.push_str(&format!("        tone: {}\n", note.tone));
                    out.push_str(&format!("        lyric: {}\n", quoted(&note.lyric)));
                    out.push_str(&format!("        pitch: {}\n", flow_pitch(&note.pitch)));
                    out.push_str(&format!(
                        "        vibrato: {}\n",
                        flow_vibrato(&note.vibrato)
                    ));
                    // A phonemizer result is not source evidence, and an
                    // override is a user edit Verse has never been told about.
                    out.push_str("        phoneme_expressions: []\n");
                    out.push_str("        phoneme_overrides: []\n");
                }
            }
            // No curve is source evidence either: every expression curve in
            // OpenUtau is an authored performance edit.
            out.push_str("    curves: []\n");
        }
    }
    // Audio reaches a project only through a preservation bundle, which owns the
    // rendered WAVs. A vocal-only export states an empty list rather than omitting
    // the key, for the same reason every other list does.
    if project.wave_parts.is_empty() {
        out.push_str("wave_parts: []\n");
    } else {
        out.push_str("wave_parts:\n");
        for part in &project.wave_parts {
            // `UWavePart`'s members are ordered by `[YamlMember(Order = 100..105)]`
            // after the four `UPart` fields, which is the order written here.
            out.push_str(&format!("  - name: {}\n", quoted(&part.name)));
            out.push_str(&format!("    comment: {}\n", quoted(&part.comment)));
            out.push_str(&format!("    track_no: {}\n", part.track_no));
            out.push_str(&format!("    position: {}\n", part.position));
            out.push_str(&format!(
                "    relative_path: {}\n",
                quoted(&part.relative_path)
            ));
            out.push_str(&format!(
                "    file_duration_ms: {}\n",
                number(part.file_duration_ms)
            ));
            out.push_str(&format!("    skip: {}\n", part.skip));
            out.push_str(&format!("    trim: {}\n", part.trim));
            out.push_str(&format!("    fadein: {}\n", part.fadein));
            out.push_str(&format!("    fadeout: {}\n", part.fadeout));
        }
    }
    out
}

/// Everything a preservation bundle has to be able to prove about a `.ustx` it
/// committed, read back from the file's own bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditedProject {
    /// One entry per `tracks:` item, in file order — the order `track_no` indexes,
    /// because `UProject.AfterLoad` does `tracks[part.trackNo]`.
    pub track_mutes: Vec<bool>,
    pub wave_parts: Vec<AuditedWavePart>,
}

/// One `wave_parts:` item as the file states it.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditedWavePart {
    /// The scalar exactly as written, quotes and escapes included, so a caller
    /// compares bytes against [`quoted`] instead of reversing an escape and
    /// risking a mismatch between the two directions.
    pub relative_path_scalar: String,
    pub track_no: i32,
    pub position: i32,
    pub file_duration_ms: f64,
    pub skip: i32,
    pub trim: i32,
    pub fadein: i32,
    pub fadeout: i32,
}

const TRACK_MEMBERS: [&str; 5] = ["phonemizer", "track_name", "mute", "solo", "volume"];
const WAVE_PART_MEMBERS: [&str; 10] = [
    "name",
    "comment",
    "track_no",
    "position",
    "relative_path",
    "file_duration_ms",
    "skip",
    "trim",
    "fadein",
    "fadeout",
];

/// Reads the audio facts back out of a project [`to_yaml`] wrote.
///
/// Deliberately not a YAML parser and deliberately not tolerant: it accepts
/// exactly the layout this emitter produces — a top-level key in column zero, one
/// `  - ` item head, `    key: value` members — and refuses anything else instead
/// of guessing. A bundle has to prove what the file it is about to commit actually
/// states, and a reader that accepted a shape this emitter never writes would be
/// proving something about its own tolerance instead. Blocks other than `tracks:`
/// and `wave_parts:` are skipped whole, because no audio invariant lives in them.
pub fn audit(yaml: &str) -> Result<AuditedProject, String> {
    let mut track_mutes: Option<Vec<bool>> = None;
    let mut wave_parts: Option<Vec<AuditedWavePart>> = None;
    let mut lines = yaml.lines().peekable();
    while let Some(line) = lines.next() {
        // Every scalar this emitter writes is indented, and `quoted` escapes every
        // character a YAML reader could take for a line break, so a line in column
        // zero is always a key and never source text.
        let (key, rest) = line
            .split_once(':')
            .filter(|_| !line.starts_with(' '))
            .ok_or_else(|| format!("{line:?} is not a top-level key of a Verse-written project"))?;
        match key {
            "tracks" => {
                if track_mutes.is_some() {
                    return Err("the project states tracks twice".into());
                }
                let mut mutes = Vec::with_capacity(4);
                for item in read_block(key, rest, &mut lines)? {
                    let members = checked_members("a track", &item, &TRACK_MEMBERS)?;
                    mutes.push(match member("a track", &members, "mute")? {
                        "true" => true,
                        "false" => false,
                        other => {
                            return Err(format!("a track states a mute of {other:?}"));
                        }
                    });
                }
                track_mutes = Some(mutes);
            }
            "wave_parts" => {
                if wave_parts.is_some() {
                    return Err("the project states wave parts twice".into());
                }
                let mut parts = Vec::with_capacity(4);
                for item in read_block(key, rest, &mut lines)? {
                    let members = checked_members("a wave part", &item, &WAVE_PART_MEMBERS)?;
                    parts.push(AuditedWavePart {
                        relative_path_scalar: member("a wave part", &members, "relative_path")?
                            .to_string(),
                        track_no: integer(&members, "track_no")?,
                        file_duration_ms: member("a wave part", &members, "file_duration_ms")?
                            .parse()
                            .map_err(|_| {
                                "a wave part states a file duration that is not a number"
                                    .to_string()
                            })?,
                        position: integer(&members, "position")?,
                        skip: integer(&members, "skip")?,
                        trim: integer(&members, "trim")?,
                        fadein: integer(&members, "fadein")?,
                        fadeout: integer(&members, "fadeout")?,
                    });
                }
                wave_parts = Some(parts);
            }
            _ => skip_block(&mut lines),
        }
    }
    Ok(AuditedProject {
        track_mutes: track_mutes.ok_or_else(|| "the project states no tracks".to_string())?,
        wave_parts: wave_parts.ok_or_else(|| "the project states no wave parts".to_string())?,
    })
}

/// Collects one block's items. An empty block is the literal `[]` this emitter
/// writes; anything else on the key's own line is a shape it never writes.
fn read_block<'a>(
    key: &str,
    rest: &str,
    lines: &mut std::iter::Peekable<std::str::Lines<'a>>,
) -> Result<Vec<Vec<(&'a str, &'a str)>>, String> {
    if rest == " []" {
        return Ok(Vec::new());
    }
    if !rest.is_empty() {
        return Err(format!("{key} states {rest:?} where a block was written"));
    }
    let mut items: Vec<Vec<(&'a str, &'a str)>> = Vec::new();
    // Copied out of the peek so the block below can consume the line it just read.
    while let Some(&line) = lines.peek() {
        let member = if let Some(head) = line.strip_prefix("  - ") {
            items.push(Vec::with_capacity(WAVE_PART_MEMBERS.len()));
            head
        } else if let Some(member) = line.strip_prefix("    ") {
            member
        } else if line.starts_with(' ') {
            return Err(format!(
                "{key} holds {line:?}, which this emitter never writes"
            ));
        } else {
            break;
        };
        if member.starts_with(' ') {
            return Err(format!(
                "{key} holds {line:?}, which this emitter never writes"
            ));
        }
        let entry = member
            .split_once(": ")
            .ok_or_else(|| format!("{key} holds a member {member:?} that states no value"))?;
        items
            .last_mut()
            .ok_or_else(|| format!("{key} states a member before any item"))?
            .push(entry);
        lines.next();
    }
    Ok(items)
}

fn skip_block(lines: &mut std::iter::Peekable<std::str::Lines<'_>>) {
    while lines.peek().is_some_and(|line| line.starts_with(' ')) {
        lines.next();
    }
}

/// Every member the emitter writes for this item and nothing besides. An unknown
/// or repeated key is refused rather than ignored: a near-miss spelling is exactly
/// how a verified field would stop being the field that was verified.
fn checked_members<'a>(
    kind: &str,
    item: &[(&'a str, &'a str)],
    allowed: &[&str],
) -> Result<BTreeMap<&'a str, &'a str>, String> {
    let mut members = BTreeMap::new();
    for (key, value) in item {
        if !allowed.contains(key) {
            return Err(format!(
                "{kind} states {key:?}, which this emitter never writes"
            ));
        }
        if members.insert(*key, *value).is_some() {
            return Err(format!("{kind} states {key:?} twice"));
        }
    }
    if members.len() != allowed.len() {
        return Err(format!(
            "{kind} states {} members where this emitter writes {}",
            members.len(),
            allowed.len()
        ));
    }
    Ok(members)
}

fn member<'a>(kind: &str, members: &BTreeMap<&str, &'a str>, key: &str) -> Result<&'a str, String> {
    members
        .get(key)
        .copied()
        .ok_or_else(|| format!("{kind} states no {key}"))
}

fn integer(members: &BTreeMap<&str, &str>, key: &str) -> Result<i32, String> {
    member("a wave part", members, key)?
        .parse()
        .map_err(|_| format!("a wave part states a {key} that is not a 32-bit integer"))
}

/// A one-line flow sequence, the shape OpenUtau's own `time_signatures` and
/// `tempos` take. An empty list is `[]`, never an omitted key: `Ustx.Load`
/// keeps whatever the field holds, and a missing key would silently leave
/// OpenUtau's constructor default in place.
fn flow_list<T>(key: &str, entries: &[T], entry: impl Fn(&T) -> String) -> String {
    let mut out = format!("{key}: [");
    for (index, value) in entries.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&entry(value));
    }
    out.push_str("]\n");
    out
}

fn flow_pitch(pitch: &UstxPitch) -> String {
    let mut out = String::from("{data: [");
    for (index, point) in pitch.data.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!(
            "{{x: {}, y: {}, shape: {}}}",
            number(point.x),
            number(point.y),
            point.shape.token()
        ));
    }
    out.push_str(&format!("], snap_first: {}}}", pitch.snap_first));
    out
}

fn flow_vibrato(vibrato: &UstxVibrato) -> String {
    format!(
        "{{length: {}, period: {}, depth: {}, in: {}, out: {}, shift: {}, drift: {}}}",
        number(vibrato.length),
        number(vibrato.period),
        number(vibrato.depth),
        number(vibrato.fade_in),
        number(vibrato.fade_out),
        number(vibrato.shift),
        number(vibrato.drift)
    )
}

/// Formats a float as a plain YAML number. Rust's `Display` writes the shortest
/// form that round-trips and never uses exponent notation, so the output is
/// deterministic and always a scalar YamlDotNet reads back as a `double`.
/// Non-finite values are refused while the project is built, never here.
fn number(value: f64) -> String {
    format!("{value}")
}

/// A YAML double-quoted scalar. Inside double quotes only `"` and `\` are
/// special, plus anything a YAML reader may treat as a line break, so those are
/// the only characters escaped and every other code point — including all
/// non-ASCII — is written literally as UTF-8.
///
/// Public because [`audit`] hands a caller the scalar exactly as the file states
/// it: comparing bytes through this function is the one comparison that cannot
/// disagree with what was written, whereas an inverse unescaper could.
pub fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            // The remaining C0 controls, DEL and the whole C1 block, which
            // includes NEL: YAML 1.1 reads several of these as line breaks, so
            // none of them may ever appear literally.
            character if character < ' ' || ('\u{7f}'..='\u{9f}').contains(&character) => {
                out.push_str(&format!("\\x{:02x}", character as u32));
            }
            // Line and paragraph separators are YAML 1.1 line breaks too, and a
            // byte-order mark is one wherever it appears.
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '\u{feff}' => out.push_str("\\ufeff"),
            // The two non-characters below `c-printable`'s `[#xE000-#xFFFD]`
            // ceiling. A `.kar` text event carrying `EF BF BF` decodes to valid
            // UTF-8, so a lyric really can hold one, and emitting it literally
            // would make the whole document non-conforming rather than spoil one
            // lyric. Everything at or above U+10000 is `c-printable` and needs no
            // escape.
            '\u{fffe}' | '\u{ffff}' => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::midi::Lyric;
    use crate::engine::projection::ProjectedMeter;

    fn note(
        onset_ticks: u32,
        duration_ticks: u32,
        pitch: u8,
        lyric: ProjectedLyric,
    ) -> ProjectedNote {
        ProjectedNote {
            onset_ticks,
            duration_ticks,
            pitch,
            lyric,
        }
    }

    fn source(id: &str, text: &str) -> ProjectedLyric {
        ProjectedLyric::Source(Box::new(Lyric::text(id, text.into())))
    }

    fn stated(id: &str, text: &str, state: LyricState) -> ProjectedLyric {
        let mut lyric = Lyric::text(id, text.into());
        lyric.state = state;
        ProjectedLyric::Source(Box::new(lyric))
    }

    /// A projection covering every decision this target has to make and can
    /// honour: onset, duration, both continuation encodings, two genuine
    /// syllables, an explicitly empty lyric, an absent lyric, a tempo change and
    /// a meter. Every refusal has its own projection instead.
    fn projected() -> ProjectedProject {
        ProjectedProject {
            ticks_per_beat: 480,
            language: "japanese".into(),
            meters: vec![ProjectedMeter {
                bar_index: 0,
                numerator: 3,
                denominator: 4,
            }],
            tempos: vec![
                ProjectedTempo {
                    tick: 0,
                    bpm: 120.0,
                    source: None,
                    discovery_index: 0,
                },
                ProjectedTempo {
                    tick: 1440,
                    bpm: 90.0,
                    source: Some("voice:7".into()),
                    discovery_index: 1,
                },
            ],
            tracks: vec![ProjectedTrack {
                name: "Voice".into(),
                source_track_id: "voice".into(),
                notes: vec![
                    note(0, 480, 60, source("word", "sing")),
                    note(
                        480,
                        240,
                        62,
                        stated("held", "held", LyricState::Continuation),
                    ),
                    // A second genuine syllable. A Synthesizer V syllable split
                    // deliberately does NOT live in this fixture: OpenUtau has no
                    // marker for it and this target refuses it, which
                    // `a_syllable_split_is_refused_rather_than_silently_dropped`
                    // exercises on its own projection.
                    note(720, 240, 64, source("second", "syl")),
                    note(960, 480, 65, ProjectedLyric::Extension),
                    note(1440, 480, 67, source("silent", "")),
                    note(1920, 480, 69, ProjectedLyric::Absent),
                ],
            }],
        }
    }

    fn yaml(project: &ProjectedProject) -> String {
        to_yaml(&serialize(project).expect("the projection is exactly representable"))
    }

    /// Pins the whole seam: one fixed projection in, one exact OpenUtau project
    /// out. Any future change that shifts a tick, a marker, a structural default
    /// or a single emitted byte fails here.
    #[test]
    fn the_seam_produces_an_exact_openutau_project() {
        assert_eq!(
            yaml(&projected()),
            concat!(
                "ustx_version: \"0.6\"\n",
                "resolution: 480\n",
                "bpm: 120\n",
                "beat_per_bar: 3\n",
                "beat_unit: 4\n",
                "time_signatures: [{bar_position: 0, beat_per_bar: 3, beat_unit: 4}]\n",
                "tempos: [{position: 0, bpm: 120}, {position: 1440, bpm: 90}]\n",
                "expressions: {}\n",
                "tracks:\n",
                "  - phonemizer: \"OpenUtau.Core.DefaultPhonemizer\"\n",
                "    track_name: \"Voice\"\n",
                "    mute: false\n",
                "    solo: false\n",
                "    volume: 0\n",
                "voice_parts:\n",
                "  - name: \"Voice\"\n",
                "    track_no: 0\n",
                "    position: 0\n",
                "    notes:\n",
                "      - position: 0\n",
                "        duration: 480\n",
                "        tone: 60\n",
                "        lyric: \"sing\"\n",
                "        pitch: {data: [{x: -1, y: 0, shape: io}, {x: 1, y: 0, shape: io}], snap_first: true}\n",
                "        vibrato: {length: 0, period: 175, depth: 25, in: 10, out: 10, shift: 0, drift: 0}\n",
                "        phoneme_expressions: []\n",
                "        phoneme_overrides: []\n",
                "      - position: 480\n",
                "        duration: 240\n",
                "        tone: 62\n",
                "        lyric: \"+~\"\n",
                "        pitch: {data: [{x: -1, y: 0, shape: io}, {x: 1, y: 0, shape: io}], snap_first: true}\n",
                "        vibrato: {length: 0, period: 175, depth: 25, in: 10, out: 10, shift: 0, drift: 0}\n",
                "        phoneme_expressions: []\n",
                "        phoneme_overrides: []\n",
                "      - position: 720\n",
                "        duration: 240\n",
                "        tone: 64\n",
                "        lyric: \"syl\"\n",
                "        pitch: {data: [{x: -1, y: 0, shape: io}, {x: 1, y: 0, shape: io}], snap_first: true}\n",
                "        vibrato: {length: 0, period: 175, depth: 25, in: 10, out: 10, shift: 0, drift: 0}\n",
                "        phoneme_expressions: []\n",
                "        phoneme_overrides: []\n",
                "      - position: 960\n",
                "        duration: 480\n",
                "        tone: 65\n",
                "        lyric: \"+~\"\n",
                "        pitch: {data: [{x: -1, y: 0, shape: io}, {x: 1, y: 0, shape: io}], snap_first: true}\n",
                "        vibrato: {length: 0, period: 175, depth: 25, in: 10, out: 10, shift: 0, drift: 0}\n",
                "        phoneme_expressions: []\n",
                "        phoneme_overrides: []\n",
                "      - position: 1440\n",
                "        duration: 480\n",
                "        tone: 67\n",
                "        lyric: \"\"\n",
                "        pitch: {data: [{x: -1, y: 0, shape: io}, {x: 1, y: 0, shape: io}], snap_first: true}\n",
                "        vibrato: {length: 0, period: 175, depth: 25, in: 10, out: 10, shift: 0, drift: 0}\n",
                "        phoneme_expressions: []\n",
                "        phoneme_overrides: []\n",
                "      - position: 1920\n",
                "        duration: 480\n",
                "        tone: 69\n",
                "        lyric: \"\"\n",
                "        pitch: {data: [{x: -1, y: 0, shape: io}, {x: 1, y: 0, shape: io}], snap_first: true}\n",
                "        vibrato: {length: 0, period: 175, depth: 25, in: 10, out: 10, shift: 0, drift: 0}\n",
                "        phoneme_expressions: []\n",
                "        phoneme_overrides: []\n",
                "    curves: []\n",
                "wave_parts: []\n",
            )
        );
    }

    /// The `ustx_version` floor is the single most destructive value in this
    /// file. `Ustx.Load` replaces the whole `time_signatures` and `tempos` lists
    /// with one entry each, taken from the obsolete scalars, for any project
    /// declaring below `0.6`: every tempo and meter change in the score is
    /// destroyed on load. This test fails if the emitted version ever drops.
    #[test]
    fn the_emitted_ustx_version_never_drops_below_the_time_map_floor() {
        let emitted = serialize(&projected()).expect("480 PPQ is exactly representable");
        let parsed = |version: &str| -> (u32, u32) {
            let mut parts = version.split('.');
            let major = parts.next().and_then(|part| part.parse().ok());
            let minor = parts.next().and_then(|part| part.parse().ok());
            (major.expect("a major"), minor.expect("a minor"))
        };
        assert!(
            parsed(&emitted.ustx_version) >= (0, 6),
            "below 0.6 OpenUtau destroys every tempo and meter change on load, got {:?}",
            emitted.ustx_version
        );
        assert_eq!(
            emitted.ustx_version, "0.6",
            "0.6 is the lowest version that keeps the time map and is upgraded in place"
        );
        assert!(
            yaml(&projected()).starts_with("ustx_version: \"0.6\"\n"),
            "the emitted file must declare the floor on its first line"
        );
    }

    /// The obsolete scalars exist only so that a mistaken downgrade loses the
    /// later changes instead of corrupting the opening of the score, so they must
    /// restate the first tempo and the first meter and nothing else.
    #[test]
    fn the_obsolete_scalars_restate_the_first_tempo_and_meter() {
        let emitted = serialize(&projected()).expect("480 PPQ is exactly representable");
        assert_eq!(emitted.bpm, 120.0);
        assert_eq!(emitted.beat_per_bar, 3);
        assert_eq!(emitted.beat_unit, 4);
        // The lists themselves keep every change, which is what version 0.6 buys.
        assert_eq!(emitted.tempos.len(), 2);
        assert_eq!(emitted.time_signatures.len(), 1);
    }

    /// A tempo and a meter change must both survive into the emitted lists,
    /// because that is the acceptance criterion the version floor protects.
    #[test]
    fn a_tempo_change_and_a_meter_change_both_survive() {
        let mut project = projected();
        project.meters.push(ProjectedMeter {
            bar_index: 1,
            numerator: 4,
            denominator: 4,
        });
        let emitted = serialize(&project).expect("480 PPQ is exactly representable");
        assert_eq!(
            emitted.time_signatures,
            vec![
                UstxTimeSignature {
                    bar_position: 0,
                    beat_per_bar: 3,
                    beat_unit: 4
                },
                UstxTimeSignature {
                    bar_position: 1,
                    beat_per_bar: 4,
                    beat_unit: 4
                }
            ]
        );
        assert_eq!(
            emitted
                .tempos
                .iter()
                .map(|tempo| (tempo.position, tempo.bpm))
                .collect::<Vec<_>>(),
            vec![(0, 120.0), (1440, 90.0)]
        );
    }

    /// PPQ 480 is the identity map and PPQ 768 is the exact eighth-note case from
    /// the I/O matrix. Neither may round.
    #[test]
    fn representable_ppqs_convert_exactly() {
        let quarter = exact_ustx_ticks(480, 480, "note onset").expect("the identity map");
        assert_eq!(quarter, 480);
        // An eighth note is 384 ticks at PPQ 768 and 240 at 480 per quarter:
        // 384 * 480 / 768 = 240. Half a quarter either way, and exact.
        let eighth = exact_ustx_ticks(384, 768, "note onset").expect("384 * 480 / 768 is exact");
        assert_eq!(eighth, 240);
        // A whole bar of 4/4 at PPQ 768 is 3072 ticks and 1920 at 480.
        let bar = exact_ustx_ticks(3072, 768, "note onset").expect("3072 * 480 / 768 is exact");
        assert_eq!(bar, 1920);
    }

    /// A septuplet: `480 = 2^5 * 3 * 5` has no factor 7, so the position is not
    /// representable and must be refused rather than rounded. The message names
    /// the tick and the PPQ so the user can find the note.
    #[test]
    fn a_tick_off_the_480_grid_is_refused_instead_of_rounded() {
        let mut inexact = projected();
        inexact.ticks_per_beat = 448;
        inexact.tracks[0].notes[0].onset_ticks = 64;
        let error = match serialize(&inexact) {
            Err(error) => error,
            Ok(_) => panic!("a septuplet position cannot be exact at 480 ticks per quarter"),
        };
        assert_eq!(
            error,
            "note onset on source track voice at MIDI tick 64 cannot be represented exactly in \
             OpenUtau's 480 ticks per quarter with PPQ 448"
        );
    }

    /// The same source that `.ustx` refuses is still exactly representable in
    /// Synthesizer V blicks, which is the whole reason the analysis gate has to
    /// take the caller's target instead of assuming one.
    #[test]
    fn a_source_ustx_refuses_is_still_representable_in_blicks() {
        let mut inexact = projected();
        inexact.ticks_per_beat = 448;
        inexact.tracks[0].notes[0].onset_ticks = 64;
        assert!(serialize(&inexact).is_err());
        assert!(crate::engine::target::svp::serialize(&inexact).is_ok());
    }

    /// `UNote.Validate` does `duration = Math.Max(10, duration)`, so a shorter
    /// note is silently lengthened. Refuse it, and refuse it with its own
    /// message so the reason is never confused with the grid refusal.
    #[test]
    fn a_note_under_the_ten_tick_floor_is_refused_with_its_own_message() {
        let mut short = projected();
        // 4 IR ticks at PPQ 480 is 4 USTX ticks, under the floor but exactly on
        // the grid: only the floor can refuse this one.
        short.tracks[0].notes[0].duration_ticks = 4;
        short.tracks[0].notes[0].onset_ticks = 0;
        let error = match serialize(&short) {
            Err(error) => error,
            Ok(_) => panic!("a 4-tick note is under the floor OpenUtau lengthens to"),
        };
        assert_eq!(
            error,
            "note duration on source track voice at MIDI tick 0 is 4 OpenUtau ticks, under the \
             10-tick floor OpenUtau silently lengthens a note to"
        );
        // Exactly ten ticks is representable and must not be refused. Shortened on
        // the last note of the lane, because shortening one that a held syllable
        // follows would open a gap before it and be refused for that instead.
        let mut floor = projected();
        let last = floor.tracks[0].notes.len() - 1;
        floor.tracks[0].notes[last].duration_ticks = 10;
        assert!(serialize(&floor).is_ok());
    }

    /// An untexted note is the empty string: the one state no OpenUtau importer
    /// can express, and the reason this whole target exists. `"a"`, `"R"` and
    /// `"+~"` each assert something the source never said.
    #[test]
    fn an_untexted_note_is_empty_and_never_a_syllable() {
        for lyric in [
            source("empty", ""),
            ProjectedLyric::Absent,
            ProjectedLyric::Source(Box::new({
                let mut unsupported = Lyric::text("humming", "x".into());
                unsupported.state = LyricState::Unsupported("humming".into());
                unsupported
            })),
        ] {
            let rendered = lyric_text(&lyric);
            assert_eq!(rendered, "", "an untexted note must carry no text");
            assert_ne!(rendered, "a");
            assert_ne!(rendered, "R");
            assert_ne!(rendered, "+~");
        }
        assert!(
            yaml(&projected()).contains("        lyric: \"\"\n"),
            "the emitted file must state the empty lyric explicitly"
        );
        assert!(
            !yaml(&projected()).contains("lyric: \"a\""),
            "no note may ever be written with OpenUtau's default syllable"
        );
    }

    /// Both continuation encodings must render `"+"`: a `LyricState::Continuation`
    /// carried on the note's own lyric, and a `ProjectedLyric::Extension` stated
    /// on a neighbour as a MusicXML `<extend>` or a MuseScore extension length.
    #[test]
    fn both_continuation_encodings_render_the_openutau_marker() {
        // `+~`, not `+`: OpenUtau writes `+~` for a slur and turns an imported
        // MIDI `-` into `+~`. `+` is the syllable split, a different idea.
        assert_eq!(
            lyric_text(&stated("held", "held", LyricState::Continuation)),
            "+~"
        );
        assert_eq!(lyric_text(&ProjectedLyric::Extension), "+~");
        // And the split is the marker the two targets happen to agree on.
        assert_eq!(
            lyric_text(&stated("split", "syl", LyricState::SyllableSplit)),
            "+"
        );
    }

    /// `+` continues a syllable in OpenUtau and `-` continues one in
    /// Synthesizer V, while `+` *splits* one in Synthesizer V. A syllable split
    /// must therefore never be written `+` here: OpenUtau would read it as a
    /// continuation the source never stated.
    /// `UVoicePart.Validate` wires `Extends` only when `Prev.End == position`. A
    /// held syllable that does not touch its predecessor would keep the literal
    /// `"+"` and reach the phonemizer as a word: the hold is lost and something
    /// the source never wrote is sung. OpenUtau cannot state a hold across a gap.
    #[test]
    fn a_held_syllable_across_a_gap_is_refused_rather_than_sung_as_a_word() {
        let mut gap = projected();
        gap.tracks[0].notes = vec![
            note(0, 240, 60, source("word", "sing")),
            // Begins at 480, but the note before it ends at 240.
            note(480, 240, 62, ProjectedLyric::Extension),
        ];
        let Err(error) = serialize(&gap) else {
            panic!("OpenUtau cannot hold a syllable across a rest");
        };
        assert!(
            error.contains("does not begin where that note ends"),
            "the refusal must name the adjacency it needs, got {error:?}"
        );

        // Touching notes are representable, so the same hold is fine once the
        // predecessor reaches it.
        let mut touching = projected();
        touching.tracks[0].notes = vec![
            note(0, 480, 60, source("word", "sing")),
            note(480, 240, 62, ProjectedLyric::Extension),
        ];
        assert!(serialize(&touching).is_ok());
    }

    /// The same trap with nothing at all in front of it.
    #[test]
    fn a_lane_leading_continuation_is_refused_because_nothing_precedes_it() {
        let mut leading = projected();
        leading.tracks[0].notes = vec![note(0, 480, 60, ProjectedLyric::Extension)];
        let Err(error) = serialize(&leading) else {
            panic!("nothing precedes the first note of a part");
        };
        assert!(
            error.contains("nothing precedes it"),
            "unexpected error: {error:?}"
        );
    }

    /// `UNote.position` and `UNote.duration` are each a C# `int`, so two late notes
    /// can both pass the grid check while their sum does not. The release profile
    /// sets `overflow-checks = true`, so an unchecked add would abort the process
    /// during analysis — on merely adding a file.
    #[test]
    fn a_note_ending_past_the_32_bit_range_is_refused_and_never_overflows() {
        let mut late = projected();
        late.tracks[0].notes = vec![
            note(2_000_000_000, 200_000_000, 60, source("a", "one")),
            note(2_100_000_000, 480, 62, source("b", "two")),
        ];
        let Err(error) = serialize(&late) else {
            panic!("2_000_000_000 + 200_000_000 does not fit an i32");
        };
        assert!(
            error.contains("32-bit tick range"),
            "unexpected error: {error:?}"
        );
    }

    /// `UProject`'s constructor guarantees one tempo and one time signature, and an
    /// explicit empty list in the file clears that default, leaving `Validate` to
    /// build a time axis from nothing.
    #[test]
    fn a_project_stating_no_time_base_is_refused() {
        let mut no_tempo = projected();
        no_tempo.tempos.clear();
        let Err(error) = serialize(&no_tempo) else {
            panic!("an empty tempo list clears OpenUtau's own default");
        };
        assert_eq!(error, "a project must state at least one tempo");

        let mut no_meter = projected();
        no_meter.meters.clear();
        let Err(error) = serialize(&no_meter) else {
            panic!("an empty time signature list clears OpenUtau's own default");
        };
        assert_eq!(error, "a project must state at least one time signature");
    }

    /// A syllable split is the one marker the two targets spell the same way.
    /// `MusicXML.cs:147-149` writes `+` for "the following syllables" of a
    /// multi-syllable word and `NotePresets.SplittedLyric` is `"+"`, which is also
    /// Synthesizer V's spelling. It must never be confused with a hold, which
    /// OpenUtau spells `+~` and Synthesizer V spells `-`.
    #[test]
    fn a_syllable_split_is_written_with_the_marker_both_targets_share() {
        let mut split = projected();
        split.tracks[0].notes = vec![
            note(0, 480, 60, source("word", "syl")),
            note(
                480,
                480,
                64,
                stated("split", "la", LyricState::SyllableSplit),
            ),
        ];

        let emitted = serialize(&split).expect("OpenUtau spells a split `+`");
        assert_eq!(emitted.voice_parts[0].notes[1].lyric, "+");
        assert_ne!(
            emitted.voice_parts[0].notes[1].lyric, "+~",
            "a split is not a hold: `+~` would sustain the vowel instead"
        );

        // Synthesizer V spells it the same, so the two agree on this marker alone.
        assert_eq!(
            crate::engine::target::svp::serialize(&split)
                .expect("the same projection is representable in blicks")
                .tracks[0]
                .main_group
                .notes[1]
                .lyrics,
            "+"
        );
    }

    /// A genuine source syllable survives untouched, including one that happens
    /// to be `a`: absence is empty, but a real `a` stays `a`.
    #[test]
    fn a_genuine_source_syllable_is_preserved_exactly() {
        assert_eq!(lyric_text(&source("real", "a")), "a");
        assert_eq!(lyric_text(&source("real", "Hel")), "Hel");
    }

    /// Arbitrary lyric text is the reason every string scalar is double-quoted.
    /// A `:`, a `#`, a quote, a backslash, a leading or trailing space and
    /// non-ASCII must all survive as one scalar that reads back exactly.
    #[test]
    fn every_string_scalar_survives_yaml_quoting_exactly() {
        assert_eq!(quoted("plain"), "\"plain\"");
        assert_eq!(quoted("Hé: \"no\""), "\"Hé: \\\"no\\\"\"");
        assert_eq!(quoted("# not a comment"), "\"# not a comment\"");
        assert_eq!(quoted("path\\to"), "\"path\\\\to\"");
        assert_eq!(quoted("  padded  "), "\"  padded  \"");
        assert_eq!(quoted("- item"), "\"- item\"");
        assert_eq!(quoted("{flow}"), "\"{flow}\"");
        assert_eq!(
            quoted("*anchor &ref !tag |fold >fold %directive"),
            "\"*anchor &ref !tag |fold >fold %directive\""
        );
        assert_eq!(quoted("yes"), "\"yes\"");
        assert_eq!(quoted("null"), "\"null\"");
        assert_eq!(quoted("0.6"), "\"0.6\"");
        assert_eq!(quoted("日本語のうた"), "\"日本語のうた\"");
        assert_eq!(quoted("emoji \u{1f3b5}"), "\"emoji \u{1f3b5}\"");
        // The two non-characters below `c-printable`'s `[#xE000-#xFFFD]` ceiling.
        // A `.kar` text event carrying `EF BF BF` decodes to valid UTF-8, so a
        // lyric really can hold one, and emitting it literally would make the
        // whole document non-conforming instead of spoiling one lyric.
        assert_eq!(quoted("a\u{fffe}b"), "\"a\\ufffeb\"");
        assert_eq!(quoted("a\u{ffff}b"), "\"a\\uffffb\"");
        // U+FFFD and everything at or above U+10000 are `c-printable` and stay
        // literal, so the escape must not creep upward.
        assert_eq!(quoted("a\u{fffd}b"), "\"a\u{fffd}b\"");
        assert_eq!(quoted("a\u{1fffe}b"), "\"a\u{1fffe}b\"");
        // Every character a YAML reader could take for a line break is escaped.
        assert_eq!(quoted("a\nb\rc\td"), "\"a\\nb\\rc\\td\"");
        assert_eq!(
            quoted("\u{0}\u{1}\u{7}\u{8}\u{b}\u{c}\u{1b}\u{1f}"),
            "\"\\x00\\x01\\x07\\x08\\x0b\\x0c\\x1b\\x1f\""
        );
        assert_eq!(quoted("\u{7f}\u{85}\u{9f}"), "\"\\x7f\\x85\\x9f\"");
        assert_eq!(
            quoted("\u{2028}\u{2029}\u{feff}"),
            "\"\\u2028\\u2029\\ufeff\""
        );
        // A non-breaking space is printable and stays literal inside quotes.
        assert_eq!(quoted("a\u{a0}b"), "\"a\u{a0}b\"");
    }

    /// The awkward lyric reaches the file as one quoted scalar on the note that
    /// carries it, not as text the emitter reinterpreted.
    #[test]
    fn an_awkward_lyric_reaches_the_file_as_one_quoted_scalar() {
        let mut project = projected();
        project.tracks[0].notes[0].lyric = source("awkward", "Hé: \"no\" #1\\2");
        assert!(yaml(&project).contains("        lyric: \"Hé: \\\"no\\\" #1\\\\2\"\n"));
    }

    /// A lyric-free MIDI is valid: zero words, zero synthetic tracks, and a file
    /// that still declares its time map.
    #[test]
    fn a_lyric_free_source_yields_a_valid_project_with_no_tracks() {
        let mut project = projected();
        project.tracks.clear();
        let emitted = yaml(&project);
        assert!(emitted.contains("tracks: []\n"));
        assert!(emitted.contains("voice_parts: []\n"));
        assert!(!emitted.contains("lyric:"), "no word may be invented");
        assert!(emitted.contains("tempos: [{position: 0, bpm: 120}, {position: 1440, bpm: 90}]\n"));
    }

    /// Two source voices become one `track` and one `voice_part` each, paired by
    /// `track_no`, because overlap inside a single voice part sets `OverlapError`.
    #[test]
    fn two_source_voices_become_one_track_and_one_voice_part_each() {
        let mut project = projected();
        project.tracks.push(ProjectedTrack {
            name: "Voice 2".into(),
            source_track_id: "second".into(),
            notes: vec![note(0, 960, 67, source("second", "sing"))],
        });
        let emitted = serialize(&project).expect("480 PPQ is exactly representable");
        assert_eq!(emitted.tracks.len(), 2);
        assert_eq!(emitted.voice_parts.len(), 2);
        assert_eq!(
            emitted
                .voice_parts
                .iter()
                .map(|part| (part.track_no, part.name.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, "Voice"), (1, "Voice 2")]
        );
        // Both lanes start at tick 0 and neither borrows the other's notes.
        assert_eq!(emitted.voice_parts[1].notes.len(), 1);
        assert_eq!(emitted.voice_parts[1].notes[0].position, 0);
    }

    /// Positions inside one voice part must ascend strictly and never overlap:
    /// `UNote.Validate` marks the later note `OverlapError` and
    /// `UVoicePart.Validate` then skips it entirely, so it is never sung.
    #[test]
    fn overlapping_notes_in_one_lane_are_refused() {
        let mut project = projected();
        project.tracks[0].notes = vec![
            note(0, 480, 60, source("first", "sing")),
            note(240, 480, 64, source("second", "too")),
        ];
        let error = match serialize(&project) {
            Err(error) => error,
            Ok(_) => panic!("a chord inside one monophonic lane cannot be represented"),
        };
        assert_eq!(
            error,
            "notes at MIDI ticks 0 and 240 on source track voice overlap; one OpenUtau voice part \
             is monophonic and marks the later note with an overlap error instead of singing it"
        );
    }

    /// Notes that merely touch are legal, which is exactly what a continuation
    /// needs: `UVoicePart.Validate` wires `Extends` only when
    /// `Prev.End == note.position`.
    #[test]
    fn notes_that_touch_are_kept_and_keep_the_continuation_wiring_possible() {
        let mut project = projected();
        project.tracks[0].notes = vec![
            note(0, 480, 60, source("first", "sing")),
            note(480, 480, 62, ProjectedLyric::Extension),
        ];
        let emitted = serialize(&project).expect("touching notes do not overlap");
        assert_eq!(emitted.voice_parts[0].notes[0].position, 0);
        assert_eq!(emitted.voice_parts[0].notes[0].duration, 480);
        assert_eq!(emitted.voice_parts[0].notes[1].position, 480);
        assert_eq!(emitted.voice_parts[0].notes[1].lyric, "+~");
    }

    /// Emitted positions ascend whatever order the projection held, because
    /// `UNote.CompareTo` falls back to `GetHashCode()` at equal positions and the
    /// load order stops being defined.
    #[test]
    fn emitted_positions_ascend_whatever_order_the_projection_held() {
        let mut project = projected();
        project.tracks[0].notes = vec![
            note(960, 480, 64, source("third", "three")),
            note(0, 480, 60, source("first", "one")),
            note(480, 480, 62, source("second", "two")),
        ];
        let emitted = serialize(&project).expect("480 PPQ is exactly representable");
        assert_eq!(
            emitted.voice_parts[0]
                .notes
                .iter()
                .map(|note| (note.position, note.lyric.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, "one"), (480, "two"), (960, "three")]
        );
    }

    /// `pitch.data` must never be empty: `UNote.Validate` dereferences
    /// `pitch.data[0]` with no guard, so an empty list crashes the load.
    #[test]
    fn every_note_carries_a_non_empty_pitch_and_a_disabled_vibrato() {
        let emitted = serialize(&projected()).expect("480 PPQ is exactly representable");
        for note in &emitted.voice_parts[0].notes {
            assert!(
                !note.pitch.data.is_empty(),
                "UNote.Validate dereferences pitch.data[0] unguarded"
            );
            assert!(note.pitch.snap_first);
            assert!(note.pitch.data.iter().all(|point| point.y == 0.0));
            assert_eq!(
                note.vibrato.length, 0.0,
                "length 0 disables vibrato; anything else would invent an expression"
            );
        }
    }

    /// `convert_midi_with` refuses a zero PPQ before projecting, but `serialize`
    /// is public: a hand-built projection must not yield a file whose positions
    /// were never validated against a divisor.
    #[test]
    fn a_zero_ppq_is_refused_even_with_nothing_to_convert() {
        let empty = ProjectedProject {
            ticks_per_beat: 0,
            ..Default::default()
        };
        let Err(error) = serialize(&empty) else {
            panic!("a zero divisor cannot be honoured");
        };
        assert_eq!(error, "MIDI PPQ division must be non-zero");
    }

    /// A note refusal is reported before a tempo refusal, which is the order
    /// Verse has always surfaced these two in.
    #[test]
    fn a_note_refusal_is_reported_before_a_tempo_refusal() {
        let mut both_inexact = projected();
        both_inexact.ticks_per_beat = 448;
        both_inexact.tempos[1].tick = 64;
        both_inexact.tracks[0].notes[0].onset_ticks = 64;
        let both = match serialize(&both_inexact) {
            Err(error) => error,
            Ok(_) => panic!("both the note and the tempo are inexact"),
        };
        assert!(
            both.starts_with("note onset on source track voice"),
            "unexpected error: {both}"
        );

        let mut tempo_only = projected();
        tempo_only.ticks_per_beat = 448;
        tempo_only.tempos[1].tick = 64;
        tempo_only.tracks[0].notes.clear();
        let tempo = match serialize(&tempo_only) {
            Err(error) => error,
            Ok(_) => panic!("the tempo position is inexact"),
        };
        assert!(
            tempo.starts_with("tempo event voice:7 at MIDI tick 64"),
            "unexpected error: {tempo}"
        );
    }

    /// The refusal names the event the source revealed first, not the earliest
    /// in the bar, exactly as the Synthesizer V target does.
    #[test]
    fn an_unrepresentable_tempo_names_the_event_the_source_revealed_first() {
        let project = ProjectedProject {
            ticks_per_beat: 448,
            meters: vec![ProjectedMeter {
                bar_index: 0,
                numerator: 4,
                denominator: 4,
            }],
            tempos: vec![
                ProjectedTempo {
                    tick: 64,
                    bpm: 90.0,
                    source: Some("later-in-the-bar:2".into()),
                    discovery_index: 1,
                },
                ProjectedTempo {
                    tick: 192,
                    bpm: 100.0,
                    source: Some("revealed-first:5".into()),
                    discovery_index: 0,
                },
            ],
            ..Default::default()
        };
        let Err(error) = serialize(&project) else {
            panic!("neither tick is exact at PPQ 448");
        };
        assert!(
            error.contains("tempo event revealed-first:5"),
            "the refusal must name the event discovered first, got {error:?}"
        );
        assert!(
            !error.contains("later-in-the-bar"),
            "tick order must not decide which event is named, got {error:?}"
        );
    }

    /// The emitted tempo list is position-ordered and holds one entry per tick
    /// whatever order the projection arrived in, because `Validate` sorts it and
    /// a duplicate position would make the effective tempo undefined.
    #[test]
    fn the_emitted_tempo_map_is_position_ordered_whatever_the_projection_holds() {
        let project = ProjectedProject {
            ticks_per_beat: 480,
            meters: vec![ProjectedMeter {
                bar_index: 0,
                numerator: 4,
                denominator: 4,
            }],
            tempos: vec![
                ProjectedTempo {
                    tick: 960,
                    bpm: 90.0,
                    source: Some("voice:2".into()),
                    discovery_index: 0,
                },
                ProjectedTempo {
                    tick: 0,
                    bpm: 120.0,
                    source: Some("voice:1".into()),
                    discovery_index: 1,
                },
                ProjectedTempo {
                    tick: 960,
                    bpm: 144.0,
                    source: Some("voice:3".into()),
                    discovery_index: 2,
                },
            ],
            ..Default::default()
        };
        let emitted = serialize(&project).expect("480 divides every tick here");
        assert_eq!(
            emitted
                .tempos
                .iter()
                .map(|tempo| (tempo.position, tempo.bpm))
                .collect::<Vec<_>>(),
            vec![(0, 120.0), (960, 144.0)],
            "positions must ascend and the later event at a tick must win"
        );
    }

    /// A position past the C# `int` range is refused rather than wrapped: every
    /// USTX position is an `int`, so the grid is exact but bounded.
    #[test]
    fn a_position_past_the_openutau_tick_range_is_refused() {
        let mut project = projected();
        project.tracks[0].notes = vec![note(u32::MAX, 480, 60, source("far", "late"))];
        let error = match serialize(&project) {
            Err(error) => error,
            Ok(_) => panic!("u32::MAX ticks at PPQ 480 exceeds a C# int"),
        };
        assert_eq!(
            error,
            "note onset on source track voice exceeds the OpenUtau tick range"
        );
    }

    /// A BPM YAML cannot state as a number must never be written. Unreachable
    /// from the converter, which only derives a BPM from a non-zero
    /// microseconds-per-quarter, but `serialize` is public.
    #[test]
    fn a_non_finite_bpm_is_refused_rather_than_written() {
        let mut project = projected();
        project.tempos[1].bpm = f64::NAN;
        let Err(error) = serialize(&project) else {
            panic!("NaN is not a BPM");
        };
        assert_eq!(
            error,
            "tempo event voice:7 carries a tempo that is not a finite BPM"
        );
    }

    /// `Formats.DetectProjectFormat` reads only the **first ten lines** and looks
    /// for `ustx_version:` — after testing for `[#SETTING]` first. The header
    /// must therefore stay inside that window and must never carry source text
    /// that could impersonate another format.
    #[test]
    fn the_format_marker_stays_inside_the_ten_lines_openutau_sniffs() {
        let mut project = projected();
        project.tracks[0].name = "[#SETTING] MThd score-partwise".into();
        project.tracks[0].notes[0].lyric = source("hostile", "[#SETTING]");
        let emitted = yaml(&project);
        let header: Vec<&str> = emitted.lines().take(10).collect();
        assert!(
            header.iter().any(|line| line.starts_with("ustx_version:")),
            "OpenUtau would not recognise the file at all: {header:?}"
        );
        for marker in ["[#SETTING]", "MThd", "score-partwise", "\"formatVersion\":"] {
            assert!(
                !header.iter().any(|line| line.contains(marker)),
                "the header must not impersonate another format with {marker}: {header:?}"
            );
        }
    }

    /// Byte output is deterministic: the same projection twice is the same file.
    #[test]
    fn the_same_projection_emits_the_same_bytes() {
        assert_eq!(yaml(&projected()), yaml(&projected()));
    }
    /// Text OpenUtau reads as something other than the word it spells. The bytes
    /// stay exact either way; this is only about the reading, and the projection
    /// turns each of these into a per-note diagnostic.
    #[test]
    fn text_openutau_reinterprets_is_reported_and_nothing_else_is() {
        // A leading `+` becomes an extension of the previous note.
        let plus = lyric_reinterpretation("+plus").expect("a leading + is reinterpreted");
        assert!(plus.contains("\"+plus\""), "{plus}");
        assert!(plus.contains("continuation"), "{plus}");
        assert!(lyric_reinterpretation("+").is_some());
        assert!(lyric_reinterpretation("++").is_some());

        // `[...]` is stripped as a phonetic hint.
        let hint = lyric_reinterpretation("sing [hint] it").expect("brackets are reinterpreted");
        assert!(hint.contains("\"sing [hint] it\""), "{hint}");
        assert!(hint.contains("phonetic hint"), "{hint}");
        assert!(lyric_reinterpretation("[all]").is_some());

        // Ordinary words, and the markers of the *other* target, are untouched.
        for innocent in [
            "",
            "sing",
            "Hé: \"no\"",
            "-held",
            "a+b",
            "50% [",
            "] backwards [",
            "日本語",
        ] {
            assert_eq!(
                lyric_reinterpretation(innocent),
                None,
                "nothing reinterprets {innocent:?}"
            );
        }
        // `.` does not match a line feed without `RegexOptions.Singleline`, so a
        // bracket pair split across lines is not a hint.
        assert_eq!(lyric_reinterpretation("[open\nclose]"), None);
        assert!(lyric_reinterpretation("[open]\nplain").is_some());
    }

    /// Every reinterpreted word is still written exactly as the source states it.
    /// The diagnostic exists precisely because the file cannot say it any other
    /// way, so the emitter must not sanitise, escape or drop anything.
    #[test]
    fn a_reinterpreted_word_is_still_written_exactly_as_the_source_states_it() {
        for text in ["+plus", "sing [hint] it"] {
            let mut project = projected();
            project.tracks[0].notes = vec![note(0, 480, 60, source("word", text))];
            assert!(
                lyric_reinterpretation(text).is_some(),
                "the fixture must be a reinterpreted word"
            );
            let emitted = yaml(&project);
            assert!(
                emitted.contains(&format!("        lyric: {}\n", quoted(text))),
                "{emitted}"
            );
        }
    }

    /// A vocal-only export references no audio at all. Real WAVs exist only inside
    /// a preservation bundle, so a projection may never produce a wave part.
    #[test]
    fn a_projection_alone_states_no_wave_part() {
        let emitted = serialize(&projected()).expect("480 PPQ is exactly representable");
        assert!(emitted.wave_parts.is_empty());
        assert!(yaml(&projected()).ends_with("wave_parts: []\n"));
    }

    /// One wave part, byte for byte: the file it references, the track it sits on,
    /// and the claim that the whole file plays from the start of the score.
    #[test]
    fn a_wave_part_states_the_whole_file_and_the_track_it_sits_on() {
        let mut project = serialize(&projected()).expect("480 PPQ is exactly representable");
        let track_no = append_wave_part(
            &mut project,
            "Piano (MuseScore Part)".into(),
            "../audio/stems/part-001-piano.wav".into(),
            882,
            44_100,
            false,
        )
        .expect("a validated WAV has a length");
        // The projection already holds one voice track, so the audio lands on the
        // next index — the index `UProject.AfterLoad` will dereference.
        assert_eq!(track_no, 1);
        assert_eq!(project.tracks.len(), 2);
        assert_eq!(project.tracks[1].track_name, "Piano (MuseScore Part)");
        assert!(!project.tracks[1].mute);

        let emitted = to_yaml(&project);
        assert!(
            emitted.ends_with(concat!(
                "wave_parts:\n",
                // OpenUtau overwrites this name with the file's own on load, so the
                // file states what the application will show.
                "  - name: \"part-001-piano.wav\"\n",
                "    comment: \"\"\n",
                "    track_no: 1\n",
                "    position: 0\n",
                "    relative_path: \"../audio/stems/part-001-piano.wav\"\n",
                "    file_duration_ms: 20\n",
                "    skip: 0\n",
                "    trim: 0\n",
                "    fadein: 0\n",
                "    fadeout: 0\n",
            )),
            "{emitted}"
        );
        // The muted case is the full-score reference, and OpenUtau keeps the mute on
        // the track because a part has none.
        let muted = append_wave_part(
            &mut project,
            "Full score reference mix (MuseScore)".into(),
            "../audio/full-score.wav".into(),
            882,
            44_100,
            true,
        )
        .expect("a validated WAV has a length");
        assert_eq!(muted, 2);
        assert!(project.tracks[2].mute);
        assert_eq!(project.wave_parts[1].track_no, 2);
    }

    /// The declared length comes from the two integers a WAV header states, never
    /// from the seconds-valued quotient beside them: rounding a quotient and then
    /// scaling it by 1000 rounds twice and lands on a different double.
    #[test]
    fn the_wave_part_duration_comes_from_the_frame_count_and_the_sample_rate() {
        assert_eq!(file_duration_ms(882, 44_100), 20.0);
        assert_eq!(file_duration_ms(441, 44_100), 10.0);
        assert_eq!(file_duration_ms(48_000, 48_000), 1000.0);

        // The case that proves which of the two derivations is used.
        let frames = 1_234_567_u64;
        let seconds_first = (frames as f64 / 44_100.0) * 1000.0;
        assert_eq!(file_duration_ms(frames, 44_100), 27_994.716_553_287_98);
        assert_ne!(
            file_duration_ms(frames, 44_100),
            seconds_first,
            "the seconds quotient rounds twice and must not be the source of this value"
        );

        // A sample rate `validate_wav` would have refused must not reach the file as
        // a value YAML cannot state as a number.
        let mut project = serialize(&projected()).expect("480 PPQ is exactly representable");
        let error = append_wave_part(
            &mut project,
            "Broken".into(),
            "../audio/stems/broken.wav".into(),
            882,
            0,
            false,
        )
        .expect_err("no length exists at zero samples per second");
        assert!(error.contains("no length OpenUtau can hold"), "{error}");
        assert!(
            project.wave_parts.is_empty() && project.tracks.len() == 1,
            "a refused wave part must leave no track behind"
        );
    }

    /// The audit reads the audio facts back out of the file's own bytes, which is
    /// what lets a bundle prove what it is about to commit.
    #[test]
    fn the_audit_reads_back_the_audio_a_bundle_must_verify() {
        let mut project = serialize(&projected()).expect("480 PPQ is exactly representable");
        append_wave_part(
            &mut project,
            "Piano (MuseScore Part)".into(),
            "../audio/stems/part-001-piano.wav".into(),
            882,
            44_100,
            false,
        )
        .expect("a validated WAV has a length");
        append_wave_part(
            &mut project,
            "Full score reference mix (MuseScore)".into(),
            "../audio/full-score.wav".into(),
            1_234_567,
            44_100,
            true,
        )
        .expect("a validated WAV has a length");

        let audited = audit(&to_yaml(&project)).expect("the emitter's own output");
        // One entry per track, in the order `track_no` indexes.
        assert_eq!(audited.track_mutes, vec![false, false, true]);
        assert_eq!(audited.wave_parts.len(), 2);
        assert_eq!(
            audited.wave_parts[0].relative_path_scalar,
            quoted("../audio/stems/part-001-piano.wav")
        );
        assert_eq!(audited.wave_parts[0].track_no, 1);
        assert_eq!(audited.wave_parts[0].file_duration_ms, 20.0);
        assert_eq!(
            audited.wave_parts[1].file_duration_ms,
            file_duration_ms(1_234_567, 44_100),
            "the length must survive the round trip exactly, not approximately"
        );
        for part in &audited.wave_parts {
            assert_eq!((part.position, part.skip, part.trim), (0, 0, 0));
            assert_eq!((part.fadein, part.fadeout), (0, 0));
        }

        // A project with no audio still states both blocks, so the audit reports an
        // empty list rather than refusing.
        let empty = audit(&yaml(&projected())).expect("a vocal-only project is auditable");
        assert!(empty.wave_parts.is_empty());
        assert_eq!(empty.track_mutes, vec![false]);
    }

    /// The audit refuses a shape this emitter never writes instead of guessing at
    /// it. Tolerating one would mean a verified field could quietly stop being the
    /// field that was verified.
    #[test]
    fn the_audit_refuses_a_document_this_emitter_never_wrote() {
        let mut project = serialize(&projected()).expect("480 PPQ is exactly representable");
        append_wave_part(
            &mut project,
            "Piano (MuseScore Part)".into(),
            "../audio/stems/part-001-piano.wav".into(),
            882,
            44_100,
            false,
        )
        .expect("a validated WAV has a length");
        let emitted = to_yaml(&project);
        assert!(audit(&emitted).is_ok(), "the fixture must be auditable");

        for (mutation, expected) in [
            // A member this emitter never writes, including a near-miss spelling of
            // one that is verified.
            ("    track_no: 1\n", "    track_number: 1\n"),
            ("    skip: 0\n", "    skipped: 0\n"),
            // A value of the wrong kind.
            ("    track_no: 1\n", "    track_no: first\n"),
            ("    file_duration_ms: 20\n", "    file_duration_ms: long\n"),
            ("    mute: false\n", "    mute: maybe\n"),
            // A flow list where a block was written, and a duplicated block.
            ("wave_parts:\n", "wave_parts: [{track_no: 1}]\n"),
            ("wave_parts:\n", "wave_parts: []\nwave_parts:\n"),
            // An indentation the emitter never produces inside a verified block.
            // Keyed on `fadein`, which only a wave part carries: `position` is also a
            // voice part's, and a mutation landing in a skipped block proves nothing.
            ("    fadein: 0\n", "      fadein: 0\n"),
        ] {
            let broken = emitted.replacen(mutation, expected, 1);
            assert_ne!(broken, emitted, "the mutation {mutation:?} must apply");
            assert!(
                audit(&broken).is_err(),
                "the audit accepted {expected:?}, which this emitter never writes"
            );
        }

        // A member dropped altogether, and a document that states no block at all.
        assert!(audit(&emitted.replacen("    trim: 0\n", "", 1)).is_err());
        assert!(audit("ustx_version: \"0.6\"\n").is_err());
        assert!(audit("  indented: 1\n").is_err());
    }
}
