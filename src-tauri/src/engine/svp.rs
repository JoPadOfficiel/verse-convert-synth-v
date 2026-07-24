//! Synthesizer V project (.svp) structures, version 113.
//! Serialization identical to the reference Python engine (kar2svp_core.py).
use serde::Serialize;

pub const BLICKS_PER_QUARTER: f64 = 705_600_000.0;

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

/// Adds one genuine audio-backed instrumental track. Symbolic instrument
/// notes must never be copied into this vocal-track shape.
pub fn append_instrumental_track(
    project: &mut SvpProject,
    name: String,
    relative_audio_filename: String,
    duration_seconds: f64,
    blick_offset: i64,
) {
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
            mute: false,
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
            uuid: uid,
            parameters: Parameters::default(),
            notes: Vec::new(),
        },
        groups: Vec::new(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
        );
        let value = serde_json::to_value(project).unwrap();
        let track = &value["tracks"][0];
        assert_eq!(track["mainRef"]["isInstrumental"], true);
        assert_eq!(track["mainRef"]["blickOffset"], 0);
        assert_eq!(
            track["mainRef"]["audio"]["filename"],
            "../audio/full-score.wav"
        );
        assert_eq!(track["mainRef"]["audio"]["duration"], 2.5);
        assert_eq!(track["mainGroup"]["notes"], serde_json::json!([]));
        assert_eq!(track["mixer"]["mute"], false);
    }
}
