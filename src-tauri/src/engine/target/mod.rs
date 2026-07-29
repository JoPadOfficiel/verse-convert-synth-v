//! Export targets.
//!
//! A target owns everything one output format decides for itself: its time
//! grid, its marker vocabulary, its track cosmetics, its schema version. It
//! reads [`crate::engine::projection::ProjectedProject`] and nothing else, so
//! adding a target cannot reach back into the conversion engine and cannot
//! change what another target writes.
pub mod svp;
pub mod ustx;

use crate::engine::projection::ProjectedProject;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Which format an export writes. The serde values are a protocol contract with
/// the webview, so they are lowercase and stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportTarget {
    /// Synthesizer V, the target Verse shipped first and the default, so a
    /// caller that names no target keeps 0.4.9's behaviour exactly.
    #[default]
    Svp,
    /// OpenUtau.
    Ustx,
}

impl ExportTarget {
    /// The output file extension, without a dot. Also the uppercase stem of the
    /// format's name in an error message.
    pub fn extension(self) -> &'static str {
        match self {
            ExportTarget::Svp => "svp",
            ExportTarget::Ustx => "ustx",
        }
    }

    /// The application that opens the file, for user-facing copy.
    pub fn display_name(self) -> &'static str {
        match self {
            ExportTarget::Svp => "Synthesizer V",
            ExportTarget::Ustx => "OpenUtau",
        }
    }
}

/// Why a target produced no bytes.
///
/// The two arms are kept apart because the Tauri boundary reports them under
/// different codes: a target refusing this source's timing is a conversion
/// failure the user is told about at analysis time, while an encoder failure is
/// a write fault that says nothing about the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SerializeError {
    /// The target cannot represent this projection. This is the refusal the
    /// analysis gate surfaces, so it can never appear for the first time at
    /// export.
    Unrepresentable(String),
    /// The target's own model could not be encoded into bytes.
    Encode(String),
}

impl SerializeError {
    pub fn message(&self) -> &str {
        match self {
            SerializeError::Unrepresentable(message) | SerializeError::Encode(message) => message,
        }
    }
}

impl fmt::Display for SerializeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

/// The stable code for a source lyric a target writes byte-faithfully but the
/// application reads as something other than the word it spells.
///
/// Deliberately not named after one target: the condition is "this application
/// reinterprets this text", and a later target may reinterpret text of its own.
/// The message names the application and the reason.
pub const LYRIC_REINTERPRETED_BY_TARGET: &str = "LYRIC_REINTERPRETED_BY_TARGET";

/// Asks a target whether the application it writes for will read this source
/// lyric as something other than the text it spells.
///
/// The bytes are always exact; this is about the reading, which no target can
/// escape and none may stay quiet about. Call it only on text the source
/// carries, never on a marker a target rendered itself.
pub fn lyric_reinterpretation(target: ExportTarget, text: &str) -> Option<String> {
    match target {
        // Not "Synthesizer V reinterprets nothing" — no such audit exists for it.
        // 0.4.9's diagnostics are the shipped contract for this target, and adding
        // one here would change what analysis reports for a source that has been
        // exporting cleanly for four releases. Audit it in its own change.
        ExportTarget::Svp => None,
        ExportTarget::Ustx => ustx::lyric_reinterpretation(text),
    }
}

/// Asks a target whether it can represent this projection at all, without
/// building the file.
///
/// This is the analysis gate. Both targets decide representability while
/// building their own model, so this runs exactly the arithmetic
/// [`serialize_to`] runs and cannot drift from it, and it stays as cheap as the
/// gate has always been.
pub fn validate_for(target: ExportTarget, project: &ProjectedProject) -> Result<(), String> {
    match target {
        ExportTarget::Svp => svp::serialize(project).map(|_| ()),
        ExportTarget::Ustx => ustx::serialize(project).map(|_| ()),
    }
}

/// The write boundary: one neutral projection in, one target's file bytes out.
///
/// The only place a caller needs to name a target is when it chooses one, so
/// nothing above this function matches on the target itself.
pub fn serialize_to(
    target: ExportTarget,
    project: &ProjectedProject,
) -> Result<Vec<u8>, SerializeError> {
    match target {
        ExportTarget::Svp => {
            let model = svp::serialize(project).map_err(SerializeError::Unrepresentable)?;
            serde_json::to_vec(&model).map_err(|error| SerializeError::Encode(error.to_string()))
        }
        ExportTarget::Ustx => {
            let model = ustx::serialize(project).map_err(SerializeError::Unrepresentable)?;
            Ok(ustx::to_yaml(&model).into_bytes())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::midi::Lyric;
    use crate::engine::projection::{
        ProjectedLyric, ProjectedMeter, ProjectedNote, ProjectedTempo, ProjectedTrack,
    };

    fn projected() -> ProjectedProject {
        ProjectedProject {
            ticks_per_beat: 480,
            language: "japanese".into(),
            meters: vec![ProjectedMeter {
                bar_index: 0,
                numerator: 4,
                denominator: 4,
            }],
            tempos: vec![ProjectedTempo {
                tick: 0,
                bpm: 120.0,
                source: None,
                discovery_index: 0,
            }],
            tracks: vec![ProjectedTrack {
                name: "Voice".into(),
                source_track_id: "voice".into(),
                muted: false,
                notes: vec![ProjectedNote {
                    onset_ticks: 0,
                    duration_ticks: 480,
                    pitch: 60,
                    lyric: ProjectedLyric::Source(Box::new(Lyric::text("word", "sing".into()))),
                }],
            }],
        }
    }

    /// The serde values are a protocol contract with the webview: renaming one
    /// silently breaks the target selector.
    #[test]
    fn the_target_names_are_stable_and_synthesizer_v_is_the_default() {
        assert_eq!(ExportTarget::default(), ExportTarget::Svp);
        assert_eq!(
            serde_json::to_string(&ExportTarget::Svp).expect("serializes"),
            "\"svp\""
        );
        assert_eq!(
            serde_json::to_string(&ExportTarget::Ustx).expect("serializes"),
            "\"ustx\""
        );
        assert_eq!(
            serde_json::from_str::<ExportTarget>("\"svp\"").expect("deserializes"),
            ExportTarget::Svp
        );
        assert_eq!(
            serde_json::from_str::<ExportTarget>("\"ustx\"").expect("deserializes"),
            ExportTarget::Ustx
        );
        assert!(serde_json::from_str::<ExportTarget>("\"midi\"").is_err());
    }

    #[test]
    fn each_target_names_its_own_extension_and_application() {
        assert_eq!(ExportTarget::Svp.extension(), "svp");
        assert_eq!(ExportTarget::Ustx.extension(), "ustx");
        assert_eq!(ExportTarget::Svp.display_name(), "Synthesizer V");
        assert_eq!(ExportTarget::Ustx.display_name(), "OpenUtau");
    }

    /// One entry point, two files. The Synthesizer V bytes must stay the exact
    /// JSON `serde_json::to_vec` has always produced.
    #[test]
    fn one_entry_point_writes_each_target_in_its_own_format() {
        let project = projected();
        let svp_bytes = serialize_to(ExportTarget::Svp, &project).expect("representable");
        assert_eq!(
            svp_bytes,
            serde_json::to_vec(&svp::serialize(&project).expect("representable"))
                .expect("serializes")
        );
        assert!(svp_bytes.starts_with(b"{\"version\":113,"));

        let ustx_bytes = serialize_to(ExportTarget::Ustx, &project).expect("representable");
        assert!(String::from_utf8(ustx_bytes)
            .expect("USTX is UTF-8")
            .starts_with("ustx_version: \"0.6\"\n"));
    }

    /// The gate and the write boundary must agree, because a refusal the gate
    /// missed would resurface at export — the thing the analysis gate exists to
    /// prevent.
    #[test]
    fn the_gate_refuses_exactly_what_the_write_boundary_refuses() {
        let mut septuplet = projected();
        septuplet.ticks_per_beat = 448;
        septuplet.tracks[0].notes[0].onset_ticks = 64;
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            assert_eq!(
                validate_for(target, &septuplet).err(),
                serialize_to(target, &septuplet)
                    .err()
                    .map(|error| error.message().to_string())
            );
        }
        // And the point of taking the target: blicks accept this source, 480
        // ticks per quarter do not.
        assert!(validate_for(ExportTarget::Svp, &septuplet).is_ok());
        assert!(validate_for(ExportTarget::Ustx, &septuplet).is_err());
    }

    #[test]
    fn a_refusal_and_an_encoder_fault_stay_distinguishable() {
        let mut septuplet = projected();
        septuplet.ticks_per_beat = 448;
        septuplet.tracks[0].notes[0].onset_ticks = 64;
        let error = serialize_to(ExportTarget::Ustx, &septuplet).expect_err("refused");
        assert!(matches!(error, SerializeError::Unrepresentable(_)));
        assert_eq!(error.to_string(), error.message());
    }
    /// The reinterpretation audit is a target's own knowledge, dispatched here so
    /// no caller matches on the target. Synthesizer V reports nothing because no
    /// such audit exists for it yet, not because it reinterprets nothing.
    #[test]
    fn only_the_target_that_reinterprets_text_reports_it() {
        for text in ["+plus", "sing [hint] it"] {
            assert_eq!(lyric_reinterpretation(ExportTarget::Svp, text), None);
            let reported = lyric_reinterpretation(ExportTarget::Ustx, text)
                .expect("OpenUtau reinterprets this text");
            assert!(reported.contains(text), "{reported}");
            assert!(reported.contains("OpenUtau"), "{reported}");
        }
        for innocent in ["sing", "-held", ""] {
            assert_eq!(lyric_reinterpretation(ExportTarget::Ustx, innocent), None);
            assert_eq!(lyric_reinterpretation(ExportTarget::Svp, innocent), None);
        }
        assert_eq!(
            LYRIC_REINTERPRETED_BY_TARGET, "LYRIC_REINTERPRETED_BY_TARGET",
            "the code is a machine-stable protocol value"
        );
    }
}
