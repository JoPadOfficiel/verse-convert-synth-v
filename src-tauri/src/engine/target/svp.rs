//! Synthesizer V project (.svp) structures, version 113.
//! Serialization identical to the reference Python engine (kar2svp_core.py).
//!
//! This is the only module that knows what Synthesizer V wants: blicks, the
//! `-`/`+` marker meanings, track colours, display order, the voice-database
//! language field and the v113 project shape. Everything above it works in
//! source-exact IR ticks through [`ProjectedProject`].
use crate::engine::midi::LyricState;
use crate::engine::projection::{
    ProjectedLyric, ProjectedNote, ProjectedProject, ProjectedTempo, ProjectedTrack,
};
use serde::Serialize;
use std::collections::BTreeMap;

/// One quarter note. A compatibility contract, not a tuning constant.
pub const BLICKS_PER_QUARTER: u64 = 705_600_000;

#[derive(Serialize)]
pub struct SvpProject {
    pub version: i32,
    pub time: Time,
    #[serde(rename = "renderConfig")]
    pub render_config: RenderConfig,
    pub tracks: Vec<SvpTrack>,
}

#[derive(Serialize)]
pub struct Time {
    pub meter: Vec<Meter>,
    pub tempo: Vec<Tempo>,
}

#[derive(Serialize)]
pub struct Meter {
    pub denominator: u32,
    pub index: u32,
    pub numerator: u32,
}

#[derive(Serialize)]
pub struct Tempo {
    pub bpm: f64,
    pub position: i64,
}

#[derive(Serialize)]
pub struct RenderConfig {
    #[serde(rename = "aspirationFormat")]
    pub aspiration_format: String,
    #[serde(rename = "bitDepth")]
    pub bit_depth: u32,
    pub destination: String,
    #[serde(rename = "exportMixDown")]
    pub export_mix_down: bool,
    pub filename: String,
    #[serde(rename = "numChannels")]
    pub num_channels: u32,
    #[serde(rename = "sampleRate")]
    pub sample_rate: u32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        RenderConfig {
            aspiration_format: "noAspiration".into(),
            bit_depth: 16,
            destination: "./".into(),
            export_mix_down: true,
            filename: "untitled".into(),
            num_channels: 1,
            sample_rate: 44100,
        }
    }
}

#[derive(Serialize)]
pub struct SvpTrack {
    pub name: String,
    #[serde(rename = "dispColor")]
    pub disp_color: String,
    #[serde(rename = "dispOrder")]
    pub disp_order: u32,
    #[serde(rename = "renderEnabled")]
    pub render_enabled: bool,
    pub mixer: Mixer,
    #[serde(rename = "mainRef")]
    pub main_ref: MainRef,
    #[serde(rename = "mainGroup")]
    pub main_group: MainGroup,
    pub groups: Vec<serde_json::Value>,
}

#[derive(Serialize)]
pub struct Mixer {
    #[serde(rename = "gainDecibel")]
    pub gain_decibel: f64,
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
    pub display: bool,
}

#[derive(Serialize)]
pub struct MainRef {
    pub audio: Audio,
    pub database: Database,
    pub dictionary: String,
    pub voice: serde_json::Value,
    #[serde(rename = "groupID")]
    pub group_id: String,
    #[serde(rename = "isInstrumental")]
    pub is_instrumental: bool,
    #[serde(rename = "blickOffset")]
    pub blick_offset: i64,
}

#[derive(Serialize)]
pub struct Audio {
    pub filename: String,
    /// Audio duration in seconds, as used by Synthesizer V instrumental refs.
    pub duration: f64,
}

#[derive(Serialize)]
pub struct Database {
    pub name: String,
    pub language: String,
    pub phoneset: String,
}

#[derive(Serialize)]
pub struct MainGroup {
    pub name: String,
    pub uuid: String,
    pub parameters: Parameters,
    pub notes: Vec<Note>,
}

#[derive(Serialize)]
pub struct Parameters {
    pub breathiness: Param,
    pub gender: Param,
    pub loudness: Param,
    #[serde(rename = "pitchDelta")]
    pub pitch_delta: Param,
    pub tension: Param,
    #[serde(rename = "vibratoEnv")]
    pub vibrato_env: Param,
    pub voicing: Param,
}

impl Default for Parameters {
    fn default() -> Self {
        let p = || Param {
            mode: "cubic".into(),
            points: vec![],
        };
        Parameters {
            breathiness: p(),
            gender: p(),
            loudness: p(),
            pitch_delta: p(),
            tension: p(),
            vibrato_env: p(),
            voicing: p(),
        }
    }
}

#[derive(Serialize)]
pub struct Param {
    pub mode: String,
    pub points: Vec<f64>,
}

#[derive(Serialize)]
pub struct Note {
    pub attributes: serde_json::Value,
    pub duration: i64,
    pub lyrics: String,
    pub onset: i64,
    pub phonemes: String,
    pub pitch: u8,
}

/// Track display colors (ARGB), muted tones -- no gradient.
pub const COLORS: [&str; 10] = [
    "ff7db235", "ff4a90d9", "ffd9534f", "ffe0a458", "ff9b59b6", "ff17a2b8", "ffe67e22", "ff2ecc71",
    "ffe84393", "ff00b894",
];

pub fn uuid(i: usize) -> String {
    format!("{:08}-0000-4000-8000-000000000000", i)
}

/// Converts a tick **quantity** onto the Synthesizer V blick grid — a position
/// and a duration alike, the map being linear with no offset. Timing that does
/// not land exactly on that grid is refused, never rounded: a rounded note is
/// silent loss, and a refusal is not.
fn exact_blicks(ticks: u32, ticks_per_beat: u16, context: &str) -> Result<i64, String> {
    if ticks_per_beat == 0 {
        return Err("MIDI PPQ division must be non-zero".into());
    }
    let numerator = u128::from(ticks) * u128::from(BLICKS_PER_QUARTER);
    let denominator = u128::from(ticks_per_beat);
    if numerator % denominator != 0 {
        return Err(format!(
            "{context} at MIDI tick {ticks} cannot be represented exactly in Synthesizer V blicks \
             with PPQ {ticks_per_beat}"
        ));
    }
    i64::try_from(numerator / denominator)
        .map_err(|_| format!("{context} exceeds the Synthesizer V blick range"))
}

/// Synthesizer V's marker vocabulary. The projection carries `LyricState`
/// instead of this text because the two markers do not agree between targets:
/// OpenUtau reads `+`, not `-`, as the continuation, so rendering here is the
/// only place it is safe to decide.
fn lyric_text(lyric: &ProjectedLyric) -> String {
    match lyric {
        ProjectedLyric::Source(source) => match &source.state {
            LyricState::Text(text) => text.clone(),
            LyricState::Continuation => "-".into(),
            LyricState::SyllableSplit => "+".into(),
            LyricState::ExplicitEmpty | LyricState::Unsupported(_) => String::new(),
        },
        ProjectedLyric::Extension => "-".into(),
        ProjectedLyric::Absent => String::new(),
    }
}

/// The single entry point of this target: one neutral projection in, one
/// Synthesizer V v113 project out.
pub fn serialize(project: &ProjectedProject) -> Result<SvpProject, String> {
    // Unreachable from `convert_midi_with`, which refuses a zero PPQ before it
    // ever projects, but `serialize` is public and a hand-built projection must
    // not silently produce a file whose every position went unvalidated.
    if project.ticks_per_beat == 0 {
        return Err("MIDI PPQ division must be non-zero".into());
    }
    // Note timing is refused before tempo timing, because the converter has
    // always reached its track loop before it reads the tempo map.
    let mut tracks = Vec::with_capacity(project.tracks.len());
    for (index, track) in project.tracks.iter().enumerate() {
        tracks.push(serialize_track(index, track, project)?);
    }
    // 0.4.9's own shape, transplanted: refuse a position while walking the
    // source in discovery order, then collect what survives into a map keyed by
    // position. Refusing in discovery order is what names the same event 0.4.9
    // named, and collecting into the map is what puts the emitted array in
    // position order — so neither property rests on the producer having sorted
    // or deduplicated anything. Two events at one tick share that tick and
    // therefore share exactness, so one representative per tick refuses exactly
    // the same set of sources.
    let mut by_tick: BTreeMap<u32, Tempo> = BTreeMap::new();
    for source in project.tempos_in_discovery_order() {
        by_tick.insert(
            source.tick,
            serialize_tempo(source, project.ticks_per_beat)?,
        );
    }
    let tempo: Vec<Tempo> = by_tick.into_values().collect();
    // Meter needs no arithmetic: Synthesizer V indexes it by measure and the
    // projection already carries a bar index, so nothing here can refuse.
    let meter = project
        .meters
        .iter()
        .map(|meter| Meter {
            denominator: meter.denominator,
            index: meter.bar_index,
            numerator: meter.numerator,
        })
        .collect();
    Ok(SvpProject {
        version: 113,
        time: Time { meter, tempo },
        render_config: RenderConfig::default(),
        tracks,
    })
}

fn serialize_tempo(tempo: &ProjectedTempo, ticks_per_beat: u16) -> Result<Tempo, String> {
    // A source carrying no tempo event at all implies 120 BPM at tick 0, which
    // is exactly representable, so the unnamed case never actually refuses.
    let context = match &tempo.source {
        Some(source) => format!("tempo event {source}"),
        None => "tempo".to_string(),
    };
    Ok(Tempo {
        bpm: tempo.bpm,
        position: exact_blicks(tempo.tick, ticks_per_beat, &context)?,
    })
}

/// `index` is the place this lane takes in the finished project, and it drives
/// the group UUID, the display order and the track colour together.
fn serialize_track(
    index: usize,
    track: &ProjectedTrack,
    project: &ProjectedProject,
) -> Result<SvpTrack, String> {
    let mut notes = Vec::with_capacity(track.notes.len());
    for note in &track.notes {
        notes.push(serialize_note(
            note,
            &track.source_track_id,
            project.ticks_per_beat,
        )?);
    }
    let uid = uuid(index);
    Ok(SvpTrack {
        name: track.name.clone(),
        disp_color: COLORS[index % COLORS.len()].to_string(),
        disp_order: index as u32,
        render_enabled: true,
        mixer: Mixer {
            gain_decibel: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            display: true,
        },
        main_ref: MainRef {
            audio: Audio {
                filename: String::new(),
                duration: 0.0,
            },
            database: Database {
                name: String::new(),
                // Selecting a voice-database language is a Synthesizer V
                // concern. It never translates or phoneticizes source text.
                language: project.language.clone(),
                phoneset: String::new(),
            },
            dictionary: String::new(),
            voice: serde_json::json!({}),
            group_id: uid.clone(),
            is_instrumental: false,
            blick_offset: 0,
        },
        main_group: MainGroup {
            name: "main".into(),
            uuid: uid,
            parameters: Parameters::default(),
            notes,
        },
        groups: vec![],
    })
}

fn serialize_note(
    note: &ProjectedNote,
    source_track_id: &str,
    ticks_per_beat: u16,
) -> Result<Note, String> {
    let onset = exact_blicks(
        note.onset_ticks,
        ticks_per_beat,
        &format!("note onset on source track {source_track_id}"),
    )?;
    let duration = exact_blicks(
        note.duration_ticks,
        ticks_per_beat,
        &format!("note duration on source track {source_track_id}"),
    )?;
    Ok(Note {
        attributes: serde_json::json!({}),
        duration,
        lyrics: lyric_text(&note.lyric),
        onset,
        phonemes: String::new(),
        pitch: note.pitch,
    })
}

/// Adds one genuine audio-backed instrumental track. Symbolic instrument
/// notes must never be copied into this vocal-track shape.
pub fn append_instrumental_track(
    project: &mut SvpProject,
    name: String,
    relative_audio_filename: String,
    duration_seconds: f64,
    blick_offset: i64,
    muted: bool,
) -> String {
    let index = project.tracks.len();
    let uid = uuid(index);
    project.tracks.push(SvpTrack {
        name,
        disp_color: "ff4794cb".into(),
        disp_order: index as u32,
        render_enabled: true,
        mixer: Mixer {
            gain_decibel: 0.0,
            pan: 0.0,
            mute: muted,
            solo: false,
            display: true,
        },
        main_ref: MainRef {
            audio: Audio {
                filename: relative_audio_filename,
                duration: duration_seconds,
            },
            database: Database {
                name: String::new(),
                language: String::new(),
                phoneset: String::new(),
            },
            dictionary: String::new(),
            voice: serde_json::json!({}),
            group_id: uid.clone(),
            is_instrumental: true,
            blick_offset,
        },
        main_group: MainGroup {
            name: "main".into(),
            uuid: uid.clone(),
            parameters: Parameters::default(),
            notes: Vec::new(),
        },
        groups: Vec::new(),
    });
    uid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::midi::Lyric;
    use crate::engine::projection::ProjectedMeter;

    /// A projection covering every decision this target has to make: onset,
    /// duration, both marker states, a derived extension, an explicitly empty
    /// lyric, an absent lyric, a tempo change and a meter.
    fn projected() -> ProjectedProject {
        let mut held = Lyric::text("held", "held".into());
        held.state = LyricState::Continuation;
        let mut split = Lyric::text("split", "syl".into());
        split.state = LyricState::SyllableSplit;
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
                    ProjectedNote {
                        onset_ticks: 0,
                        duration_ticks: 480,
                        pitch: 60,
                        lyric: ProjectedLyric::Source(Box::new(Lyric::text("word", "sing".into()))),
                    },
                    ProjectedNote {
                        onset_ticks: 480,
                        duration_ticks: 240,
                        pitch: 62,
                        lyric: ProjectedLyric::Source(Box::new(held)),
                    },
                    ProjectedNote {
                        onset_ticks: 720,
                        duration_ticks: 240,
                        pitch: 64,
                        lyric: ProjectedLyric::Source(Box::new(split)),
                    },
                    ProjectedNote {
                        onset_ticks: 960,
                        duration_ticks: 480,
                        pitch: 65,
                        lyric: ProjectedLyric::Extension,
                    },
                    ProjectedNote {
                        onset_ticks: 1440,
                        duration_ticks: 480,
                        pitch: 67,
                        lyric: ProjectedLyric::Source(Box::new(Lyric::text(
                            "silent",
                            String::new(),
                        ))),
                    },
                    ProjectedNote {
                        onset_ticks: 1920,
                        duration_ticks: 480,
                        pitch: 69,
                        lyric: ProjectedLyric::Absent,
                    },
                ],
            }],
        }
    }

    /// 0.4.9 validated every tempo event while reading the source, so the event
    /// a refusal named was the first the source revealed, not the earliest in the
    /// bar. The tempo map is emitted in tick order, so the two orders differ and
    /// only `discovery_index` keeps the message identical.
    #[test]
    fn an_unrepresentable_tempo_names_the_event_the_source_revealed_first() {
        // At PPQ 1024 an odd tick is not exactly representable in blicks.
        let project = ProjectedProject {
            ticks_per_beat: 1024,
            tempos: vec![
                ProjectedTempo {
                    tick: 1,
                    bpm: 90.0,
                    source: Some("later-in-the-bar:2".into()),
                    discovery_index: 1,
                },
                ProjectedTempo {
                    tick: 3,
                    bpm: 100.0,
                    source: Some("revealed-first:5".into()),
                    discovery_index: 0,
                },
            ],
            ..Default::default()
        };
        let Err(error) = serialize(&project) else {
            panic!("an odd tick at PPQ 1024 cannot be exact");
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

    /// The emitted `time.tempo` array must be in position order and hold one
    /// entry per tick whatever order the projection arrived in, because that
    /// ordering is Synthesizer V's contract and not the producer's promise.
    #[test]
    fn the_emitted_tempo_map_is_position_ordered_whatever_the_projection_holds() {
        let project = ProjectedProject {
            ticks_per_beat: 480,
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
        let svp = serialize(&project).expect("480 divides every tick here");
        let emitted: Vec<(i64, f64)> = svp
            .time
            .tempo
            .iter()
            .map(|tempo| (tempo.position, tempo.bpm))
            .collect();
        assert_eq!(
            emitted,
            vec![(0, 120.0), (960 * 1_470_000, 144.0)],
            "positions must ascend and the later event at a tick must win"
        );
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

    #[test]
    fn genuine_la_is_preserved_but_absence_stays_empty() {
        let lyric = Lyric::text("source", "la".into());
        assert_eq!(lyric_text(&ProjectedLyric::Source(Box::new(lyric))), "la");
        assert_eq!(
            lyric_text(&ProjectedLyric::Source(Box::new(Lyric::text(
                "empty",
                String::new()
            )))),
            ""
        );
    }

    /// `-` continues a syllable in Synthesizer V and `+` splits one. OpenUtau
    /// reads `+` as the continuation, so a projection that carried rendered
    /// marker text instead of `LyricState` would corrupt one of the two.
    #[test]
    fn the_two_markers_are_rendered_here_and_are_not_swapped() {
        let mut held = Lyric::text("held", "held".into());
        held.state = LyricState::Continuation;
        let mut split = Lyric::text("split", "syl".into());
        split.state = LyricState::SyllableSplit;
        assert_eq!(lyric_text(&ProjectedLyric::Source(Box::new(held))), "-");
        assert_eq!(lyric_text(&ProjectedLyric::Source(Box::new(split))), "+");
        assert_eq!(lyric_text(&ProjectedLyric::Extension), "-");
        assert_eq!(lyric_text(&ProjectedLyric::Absent), "");
    }

    /// Pins the whole seam: one fixed projection in, one exact Synthesizer V
    /// project out. Any future target that shifts a blick, a colour, a display
    /// order, a UUID or a marker fails here.
    #[test]
    fn the_seam_produces_an_exact_synthesizer_v_project() {
        let project = serialize(&projected()).expect("480 PPQ is exactly representable");
        assert_eq!(
            serde_json::to_value(&project).expect("serializes"),
            serde_json::json!({
                "version": 113,
                "time": {
                    "meter": [{"denominator": 4, "index": 0, "numerator": 3}],
                    "tempo": [
                        {"bpm": 120.0, "position": 0},
                        {"bpm": 90.0, "position": 2_116_800_000i64}
                    ]
                },
                "renderConfig": {
                    "aspirationFormat": "noAspiration",
                    "bitDepth": 16,
                    "destination": "./",
                    "exportMixDown": true,
                    "filename": "untitled",
                    "numChannels": 1,
                    "sampleRate": 44100
                },
                "tracks": [{
                    "name": "Voice",
                    "dispColor": "ff7db235",
                    "dispOrder": 0,
                    "renderEnabled": true,
                    "mixer": {
                        "gainDecibel": 0.0,
                        "pan": 0.0,
                        "mute": false,
                        "solo": false,
                        "display": true
                    },
                    "mainRef": {
                        "audio": {"filename": "", "duration": 0.0},
                        "database": {"name": "", "language": "japanese", "phoneset": ""},
                        "dictionary": "",
                        "voice": {},
                        "groupID": "00000000-0000-4000-8000-000000000000",
                        "isInstrumental": false,
                        "blickOffset": 0
                    },
                    "mainGroup": {
                        "name": "main",
                        "uuid": "00000000-0000-4000-8000-000000000000",
                        "parameters": {
                            "breathiness": {"mode": "cubic", "points": []},
                            "gender": {"mode": "cubic", "points": []},
                            "loudness": {"mode": "cubic", "points": []},
                            "pitchDelta": {"mode": "cubic", "points": []},
                            "tension": {"mode": "cubic", "points": []},
                            "vibratoEnv": {"mode": "cubic", "points": []},
                            "voicing": {"mode": "cubic", "points": []}
                        },
                        "notes": [
                            {"attributes": {}, "duration": 705_600_000i64, "lyrics": "sing", "onset": 0, "phonemes": "", "pitch": 60},
                            {"attributes": {}, "duration": 352_800_000i64, "lyrics": "-", "onset": 705_600_000i64, "phonemes": "", "pitch": 62},
                            {"attributes": {}, "duration": 352_800_000i64, "lyrics": "+", "onset": 1_058_400_000i64, "phonemes": "", "pitch": 64},
                            {"attributes": {}, "duration": 705_600_000i64, "lyrics": "-", "onset": 1_411_200_000i64, "phonemes": "", "pitch": 65},
                            {"attributes": {}, "duration": 705_600_000i64, "lyrics": "", "onset": 2_116_800_000i64, "phonemes": "", "pitch": 67},
                            {"attributes": {}, "duration": 705_600_000i64, "lyrics": "", "onset": 2_822_400_000i64, "phonemes": "", "pitch": 69}
                        ]
                    },
                    "groups": []
                }]
            })
        );
    }

    /// Timing that misses the blick grid is refused, never rounded, and the
    /// refusal names the source track exactly as it always has.
    #[test]
    fn inexact_tick_quantities_are_refused_instead_of_rounded() {
        let mut inexact = projected();
        inexact.ticks_per_beat = 1024;
        inexact.tracks[0].notes[0].onset_ticks = 1;
        let error = match serialize(&inexact) {
            Err(error) => error,
            Ok(_) => panic!("tick 1 at PPQ 1024 is not exactly representable"),
        };
        assert_eq!(
            error,
            "note onset on source track voice at MIDI tick 1 cannot be represented exactly in \
             Synthesizer V blicks with PPQ 1024"
        );
    }

    /// A refusal is reported for the notes before the tempo map, which is the
    /// order the converter has always surfaced these two.
    #[test]
    fn a_note_refusal_is_reported_before_a_tempo_refusal() {
        let mut inexact = projected();
        inexact.ticks_per_beat = 1024;
        inexact.tempos[1].tick = 1;
        inexact.tracks[0].notes[0].onset_ticks = 3;
        let both = match serialize(&inexact) {
            Err(error) => error,
            Ok(_) => panic!("both the note and the tempo are inexact"),
        };
        assert!(
            both.starts_with("note onset on source track voice"),
            "unexpected error: {both}"
        );

        let mut tempo_only = projected();
        tempo_only.ticks_per_beat = 1024;
        tempo_only.tempos[1].tick = 1;
        tempo_only.tracks[0].notes.clear();
        let tempo = match serialize(&tempo_only) {
            Err(error) => error,
            Ok(_) => panic!("the tempo position is inexact"),
        };
        assert!(
            tempo.starts_with("tempo event voice:7 at MIDI tick 1"),
            "unexpected error: {tempo}"
        );
    }

    #[test]
    fn instrumental_track_is_audio_backed_and_contains_no_vocal_notes() {
        let mut project = SvpProject {
            version: 113,
            time: Time {
                meter: vec![],
                tempo: vec![],
            },
            render_config: RenderConfig::default(),
            tracks: vec![],
        };
        append_instrumental_track(
            &mut project,
            "Full score reference mix (MuseScore)".into(),
            "../audio/full-score.wav".into(),
            2.5,
            0,
            true,
        );
        let value = serde_json::to_value(project).unwrap();
        let track = &value["tracks"][0];
        assert_eq!(track["mainRef"]["isInstrumental"], true);
        assert_eq!(track["mainRef"]["blickOffset"], 0);
        assert_eq!(
            track["mainRef"]["audio"]["filename"],
            "../audio/full-score.wav"
        );
        assert_eq!(track["mixer"]["mute"], true);
        assert_eq!(track["mainRef"]["audio"]["duration"], 2.5);
        assert_eq!(track["mainGroup"]["notes"], serde_json::json!([]));
    }
}
