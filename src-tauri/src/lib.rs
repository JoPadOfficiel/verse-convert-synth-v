pub mod bundle;
pub mod engine;
pub mod renderer;
pub mod stems;

use bundle::{
    build_preservation_ledger, export_bundle_with_progress as write_bundle, BundleInput,
    BundleLayout, BundleProgressEvent, BundleProject, BundleRequest, BundleResult,
};
use engine::convert::{
    convert_midi_with_target, Diagnostic, ExportRepresentation, LyricStatus, LyricStatusState,
    SourceRole, TrackReport,
};
use engine::midi::{Midi, SourceFormat, SourceTopology};
use engine::target::{ExportTarget, SerializeError};
use renderer::{
    AudioRenderer, MuseScoreConfig, MuseScoreRenderer, RenderLimits, DEFAULT_MAX_WAV_BYTES,
    DEFAULT_RENDER_TIMEOUT,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::ipc::Channel;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackInfo {
    pub id: usize,
    pub source_id: String,
    pub track: String,
    pub notes: usize,
    /// Compatibility field for older webviews. `source_role` and
    /// `export_representation` are the authoritative, non-conflated fields.
    pub role: String,
    pub placed: usize,
    pub source_role: SourceRole,
    pub lyric_status: LyricStatus,
    pub export_representation: ExportRepresentation,
    pub requires_voice_assignment: bool,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartInfo {
    pub source_id: String,
    pub part: String,
    pub staves: usize,
    pub voices: usize,
    pub track_ids: Vec<usize>,
    pub vocal_candidate_track_ids: Vec<usize>,
    pub source_track_ids: Vec<String>,
    pub notes: usize,
    pub placed: usize,
    pub source_role: SourceRole,
    pub lyric_status: LyricStatus,
    pub export_representation: ExportRepresentation,
    pub requires_voice_assignment: bool,
    pub has_audio_stem: bool,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum AudioStatusDto {
    NotRendered,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub path: String,
    pub name: String,
    pub ok: bool,
    pub error: Option<CommandErrorDto>,
    /// Compatibility message retained for the current desktop webview.
    pub msg: Option<String>,
    pub n_parts: usize,
    pub n_voices: usize,
    pub n_tracks: usize,
    pub placed: usize,
    pub parts: Vec<PartInfo>,
    pub tracks: Vec<TrackInfo>,
    pub audio_status: AudioStatusDto,
    pub requires_voice_assignment: bool,
    /// Whether a complete bundle can be written for this source. A bundle always
    /// writes a Synthesizer V project, so this stays true for a source only the
    /// OpenUtau target refuses.
    pub bundle_ready: bool,
    pub warnings: Vec<Diagnostic>,
    pub out: Option<String>,
}

/// The batch-export filename. The `_LYRICS` stem is 0.4.9's and stays; only the
/// extension follows the chosen target.
fn vocal_out_path(path: &str, out_dir: Option<&str>, target: ExportTarget) -> String {
    let p = Path::new(path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let fname = format!("{}_LYRICS.{}", stem, target.extension());
    match out_dir {
        Some(d) if !d.is_empty() => Path::new(d).join(fname).to_string_lossy().to_string(),
        _ => p.with_file_name(fname).to_string_lossy().to_string(),
    }
}

fn validate_new_output_target(source: &Path, target: &Path) -> Result<(), String> {
    let source = std::fs::canonicalize(source)
        .map_err(|error| format!("cannot resolve source path ({error})"))?;
    let parent = target
        .parent()
        .ok_or_else(|| "output path has no parent directory".to_string())?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|error| format!("cannot resolve output directory ({error})"))?;
    let file_name = target
        .file_name()
        .ok_or_else(|| "output path has no filename".to_string())?;
    let resolved_target = parent.join(file_name);
    if resolved_target == source {
        return Err("output path is the source file; input files are never overwritten".into());
    }
    if resolved_target.exists() {
        return Err("output already exists; choose a new filename".into());
    }
    Ok(())
}

const MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const SUPPORTED_EXT: [&str; 8] = [
    "kar", "mid", "midi", "mxl", "xml", "musicxml", "mscz", "mscx",
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandErrorDto {
    pub code: String,
    pub message: String,
    pub remediation: Option<String>,
}

impl CommandErrorDto {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            remediation: None,
        }
    }
}

impl From<bundle::BundleError> for CommandErrorDto {
    fn from(error: bundle::BundleError) -> Self {
        let code = error.code();
        let remediation = match code {
            "RENDERER_NOT_FOUND" => Some(
                "Configure MuseScore Studio 3.6.2 or 4 with score-parts support, then retry the bundle export.",
            ),
            "RENDERER_UNSUPPORTED" => Some(
                "Install or select MuseScore Studio 3.6.2 or 4 with verified score-parts support.",
            ),
            "DESTINATION_EXISTS" => Some("Choose a new bundle name; Verse never overwrites."),
            _ => None,
        };
        let mut dto = Self::new(code, error.to_string());
        dto.remediation = remediation.map(str::to_string);
        dto
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererStatusDto {
    pub state: String,
    pub configured: bool,
    pub provider: Option<String>,
    pub version: Option<String>,
    pub full_score_mix: bool,
    pub message: Option<String>,
}

fn part_infos(topology: &SourceTopology, reports: &[TrackReport]) -> Vec<PartInfo> {
    topology
        .parts
        .iter()
        .map(|part| {
            let mut projection_track_ids = Vec::new();
            for track_id in part
                .staves
                .iter()
                .flat_map(|staff| &staff.voices)
                .flat_map(|voice| &voice.projection_track_ids)
            {
                if !projection_track_ids.contains(track_id) {
                    projection_track_ids.push(track_id.clone());
                }
            }
            let source_track_ids = if part.source_track_ids.is_empty() {
                projection_track_ids.clone()
            } else {
                part.source_track_ids.clone()
            };
            let part_reports = reports
                .iter()
                .filter(|report| source_track_ids.contains(&report.source_id))
                .collect::<Vec<_>>();
            let notes = part_reports.iter().map(|report| report.notes).sum();
            let placed = part_reports.iter().map(|report| report.placed).sum();
            let mut roles = Vec::new();
            let mut warnings = Vec::new();
            for report in &part_reports {
                if !roles.contains(&report.source_role) {
                    roles.push(report.source_role);
                }
                for warning in &report.warnings {
                    if !warnings.contains(warning) {
                        warnings.push(warning.clone());
                    }
                }
            }
            let source_role = match roles.as_slice() {
                [] => SourceRole::Metadata,
                [role] => *role,
                roles
                    if roles.iter().all(|role| {
                        matches!(role, SourceRole::Metadata | SourceRole::LyricsOnly)
                    }) =>
                {
                    if roles.contains(&SourceRole::LyricsOnly) {
                        SourceRole::LyricsOnly
                    } else {
                        SourceRole::Metadata
                    }
                }
                _ => SourceRole::Mixed,
            };
            let mut lyric_status = LyricStatus {
                state: LyricStatusState::None,
                source_text_count: 0,
                projected_text_count: 0,
                explicit_empty_count: 0,
                continuation_count: 0,
                unsupported_count: 0,
            };
            for report in &part_reports {
                lyric_status.source_text_count += report.lyric_status.source_text_count;
                lyric_status.projected_text_count += report.lyric_status.projected_text_count;
                lyric_status.explicit_empty_count += report.lyric_status.explicit_empty_count;
                lyric_status.continuation_count += report.lyric_status.continuation_count;
                lyric_status.unsupported_count += report.lyric_status.unsupported_count;
            }
            lyric_status.state = if part_reports
                .iter()
                .any(|report| report.lyric_status.state == LyricStatusState::Unsupported)
            {
                LyricStatusState::Unsupported
            } else if part_reports
                .iter()
                .any(|report| report.lyric_status.state == LyricStatusState::Ambiguous)
            {
                LyricStatusState::Ambiguous
            } else if lyric_status.source_text_count > 0 {
                LyricStatusState::SourceOwned
            } else if lyric_status.explicit_empty_count > 0 {
                LyricStatusState::ExplicitEmpty
            } else if part_reports
                .iter()
                .any(|report| report.lyric_status.state == LyricStatusState::MetadataOnly)
            {
                LyricStatusState::MetadataOnly
            } else {
                LyricStatusState::None
            };
            let vocal_projection = part_reports.iter().any(|report| {
                matches!(
                    report.export_representation,
                    ExportRepresentation::VocalNotes
                        | ExportRepresentation::VocalNotesAndReferenceMix
                )
            });
            let has_audio_stem = notes > 0;
            let export_representation = match (vocal_projection, has_audio_stem) {
                (true, true) => ExportRepresentation::VocalNotesAndReferenceMix,
                (true, false) => ExportRepresentation::VocalNotes,
                (false, true) => ExportRepresentation::ReferenceMixMember,
                (false, false) => ExportRepresentation::SourceOnly,
            };
            PartInfo {
                source_id: part.id.clone(),
                part: if part.name.trim().is_empty() {
                    part.id.clone()
                } else {
                    part.name.clone()
                },
                staves: part.staves.len(),
                voices: part.staves.iter().map(|staff| staff.voices.len()).sum(),
                track_ids: part_reports.iter().map(|report| report.id).collect(),
                vocal_candidate_track_ids: part_reports
                    .iter()
                    .filter(|report| {
                        projection_track_ids.contains(&report.source_id)
                            && report.notes > 0
                            && !matches!(
                                report.source_role,
                                SourceRole::Percussion
                                    | SourceRole::Metadata
                                    | SourceRole::LyricsOnly
                            )
                    })
                    .map(|report| report.id)
                    .collect(),
                source_track_ids,
                notes,
                placed,
                source_role,
                lyric_status,
                export_representation,
                requires_voice_assignment: part_reports
                    .iter()
                    .any(|report| report.requires_voice_assignment),
                has_audio_stem,
                warnings,
            }
        })
        .collect()
}

fn process_one(
    path: &str,
    write: bool,
    out_dir: Option<&str>,
    language: &str,
    overrides: Option<&HashMap<usize, bool>>,
    target: ExportTarget,
) -> FileResult {
    let name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string();
    let err = |name: String, code: &str, msg: String| FileResult {
        path: path.into(),
        name,
        ok: false,
        error: Some(CommandErrorDto::new(code, msg.clone())),
        msg: Some(msg),
        n_parts: 0,
        n_voices: 0,
        n_tracks: 0,
        placed: 0,
        parts: vec![],
        tracks: vec![],
        audio_status: AudioStatusDto::NotRendered,
        requires_voice_assignment: false,
        bundle_ready: false,
        warnings: vec![],
        out: None,
    };
    let extension = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|extension| SUPPORTED_EXT.contains(&extension.as_str()));
    let Some(extension) = extension else {
        return err(name, "UNSUPPORTED_FILE", "unsupported file type".into());
    };
    match std::fs::metadata(path) {
        Ok(md) if md.len() > MAX_INPUT_BYTES => {
            return err(
                name,
                "SOURCE_TOO_LARGE",
                "abnormally large file (rejected for safety)".into(),
            );
        }
        _ => {}
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            return err(
                name,
                "SOURCE_READ_FAILED",
                format!("cannot read file ({e})"),
            );
        }
    };
    let midi = match parse_source_snapshot(&data, &extension) {
        Ok(midi) => midi,
        Err(message) => return err(name, "SOURCE_PARSE_FAILED", message),
    };
    let r = convert_midi_with_target(&midi, language, overrides, target);
    let tracks: Vec<_> = r
        .tracks
        .iter()
        .map(|t| TrackInfo {
            id: t.id,
            source_id: t.source_id.clone(),
            track: t.track.clone(),
            notes: t.notes,
            role: t.role.clone(),
            placed: t.placed,
            source_role: t.source_role,
            lyric_status: t.lyric_status.clone(),
            export_representation: t.export_representation,
            requires_voice_assignment: t.requires_voice_assignment,
            warnings: t.warnings.clone(),
        })
        .collect();
    let parts = part_infos(&r.topology, &r.tracks);
    let n_parts = parts.len();
    let n_tracks = tracks.len();
    let n_voices = r
        .topology
        .parts
        .iter()
        .flat_map(|part| &part.staves)
        .map(|staff| staff.voices.len())
        .sum();
    let requires_voice_assignment = tracks.iter().any(|track| track.requires_voice_assignment);
    let warnings = tracks
        .iter()
        .flat_map(|track| track.warnings.iter().cloned())
        .collect();
    let mut out = None;
    let mut ok = r.ok;
    let mut msg = r.msg;
    let mut error = msg
        .as_ref()
        .map(|message| CommandErrorDto::new("CONVERSION_FAILED", message.clone()));
    if write && ok {
        // A projection refusal is not a write fault: the vocal-export command
        // classifies exactly that condition as CONVERSION_FAILED, so this
        // boundary matches it. Every pre-existing arm keeps the WRITE_FAILED code
        // it had in 0.4.9.
        let write_result = (|| -> Result<String, (&'static str, String)> {
            let projected = r.svp.as_ref().ok_or_else(|| {
                (
                    "WRITE_FAILED",
                    format!(
                        "no {} output was produced",
                        target.extension().to_ascii_uppercase()
                    ),
                )
            })?;
            let out_path = vocal_out_path(path, out_dir, target);
            validate_new_output_target(Path::new(path), Path::new(&out_path))
                .map_err(|message| ("WRITE_FAILED", message))?;
            // The neutral projection becomes one target's file only here, at the
            // boundary that writes it.
            let bytes =
                engine::target::serialize_to(target, projected).map_err(|error| match error {
                    SerializeError::Unrepresentable(message) => ("CONVERSION_FAILED", message),
                    SerializeError::Encode(message) => (
                        "WRITE_FAILED",
                        format!(
                            "cannot serialize {} ({message})",
                            target.extension().to_ascii_uppercase()
                        ),
                    ),
                })?;
            bundle::write_bytes_no_replace(Path::new(&out_path), &bytes).map_err(|error| {
                (
                    "WRITE_FAILED",
                    format!(
                        "cannot write {} ({error})",
                        target.extension().to_ascii_uppercase()
                    ),
                )
            })?;
            Ok(out_path)
        })();
        match write_result {
            Ok(path) => out = Some(path),
            Err((code, write_error)) => {
                ok = false;
                msg = Some(write_error.clone());
                error = Some(CommandErrorDto::new(code, write_error));
            }
        }
    }
    FileResult {
        path: path.into(),
        name,
        ok,
        error,
        msg,
        n_parts,
        n_voices,
        n_tracks,
        placed: r.placed,
        parts,
        tracks,
        audio_status: AudioStatusDto::NotRendered,
        requires_voice_assignment,
        bundle_ready: r.bundle_ready,
        warnings,
        out,
    }
}

/// Writes one vocal-only project.
///
/// `target` is the **output path**, which it has been since 0.1.0, so the export
/// format is `export_target` — an optional parameter that keeps every existing
/// caller writing `.svp` exactly as before.
#[tauri::command]
fn export_svp(
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
    export_target: Option<ExportTarget>,
) -> Result<String, CommandErrorDto> {
    let export_target = export_target.unwrap_or_default();
    let extension = Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|extension| SUPPORTED_EXT.contains(&extension.as_str()))
        .ok_or_else(|| CommandErrorDto::new("UNSUPPORTED_FILE", "unsupported file type"))?;
    if let Ok(md) = std::fs::metadata(&path) {
        if md.len() > MAX_INPUT_BYTES {
            return Err(CommandErrorDto::new(
                "SOURCE_TOO_LARGE",
                "abnormally large file (rejected for safety)",
            ));
        }
    }
    let data = std::fs::read(&path).map_err(|error| {
        CommandErrorDto::new("SOURCE_READ_FAILED", format!("cannot read file ({error})"))
    })?;
    let ov: HashMap<usize, bool> = overrides
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| k.parse::<usize>().ok().map(|k| (k, v)))
        .collect();
    let lang = language.as_deref().unwrap_or("english");
    let midi = parse_source_snapshot(&data, &extension)
        .map_err(|message| CommandErrorDto::new("SOURCE_PARSE_FAILED", message))?;
    let r = convert_midi_with_target(&midi, lang, Some(&ov), export_target);
    if !r.ok {
        return Err(CommandErrorDto::new(
            "CONVERSION_FAILED",
            r.msg.unwrap_or_else(|| "conversion failed".into()),
        ));
    }
    let projected = r
        .svp
        .ok_or_else(|| CommandErrorDto::new("CONVERSION_FAILED", "no output produced"))?;
    let source_path = Path::new(&path);
    let target_path = Path::new(&target);
    // The output path and the export format arrive as independent arguments, and
    // the save dialog's filter is only advisory. Writing OpenUtau YAML into a
    // `.svp`, or a Synthesizer V project into a `.ustx`, produces a file neither
    // application will open, so the two must agree.
    let stated_extension = target_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if stated_extension.as_deref() != Some(export_target.extension()) {
        return Err(CommandErrorDto::new(
            "INVALID_OUTPUT",
            format!(
                "the output filename must end in .{} to hold a {} project",
                export_target.extension(),
                export_target.display_name()
            ),
        ));
    }
    validate_new_output_target(source_path, target_path)
        .map_err(|message| CommandErrorDto::new("INVALID_OUTPUT", message))?;
    // The neutral projection becomes one target's file only here.
    let bytes =
        engine::target::serialize_to(export_target, &projected).map_err(|error| match error {
            SerializeError::Unrepresentable(message) => {
                CommandErrorDto::new("CONVERSION_FAILED", message)
            }
            SerializeError::Encode(message) => CommandErrorDto::new("SERIALIZE_FAILED", message),
        })?;
    bundle::write_bytes_no_replace(target_path, &bytes).map_err(|error| {
        CommandErrorDto::new("WRITE_FAILED", format!("cannot write file ({error})"))
    })?;
    Ok(target)
}

fn parse_source_snapshot(data: &[u8], extension: &str) -> Result<Midi, String> {
    use engine::musescore as ms;
    use engine::musicxml as mx;

    if mx::looks_like_xml(data) {
        return if ms::is_musescore_xml(data) {
            ms::parse(data).map_err(|error| format!("unreadable MuseScore ({error})"))
        } else {
            mx::parse(data).map_err(|error| format!("unreadable MusicXML ({error})"))
        };
    }
    if mx::is_zip(data) {
        if mx::zip_has_musicxml(data) {
            return mx::parse(data).map_err(|error| format!("unreadable MusicXML ({error})"));
        }
        if ms::zip_has_mscx(data) {
            return ms::parse(data).map_err(|error| format!("unreadable MuseScore ({error})"));
        }
        return Err("archive contains no recognized score".into());
    }
    if extension.eq_ignore_ascii_case("kar") {
        engine::midi::parse_with_karaoke_profile(data)
    } else {
        engine::midi::parse(data)
    }
    .map_err(|error| format!("unreadable MIDI ({error})"))
}

fn source_format_name(format: SourceFormat) -> &'static str {
    match format {
        SourceFormat::StandardMidi => "standardMidi",
        SourceFormat::KaraokeMidi => "karaokeMidi",
        SourceFormat::MusicXml => "musicXml",
        SourceFormat::MuseScore => "museScore",
    }
}

/// Writes one complete preservation bundle.
///
/// `target` is the **destination path**, which it has been since the bundle
/// shipped, so the project format inside the bundle is `export_target` — optional,
/// defaulting to Synthesizer V, so a caller that names no format writes 0.4.9's
/// bundle exactly.
fn export_bundle_blocking(
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
    renderer_path: Option<String>,
    export_target: Option<ExportTarget>,
    progress: &(dyn Fn(BundleProgressEvent) + Sync),
) -> Result<BundleResult, CommandErrorDto> {
    let export_target = export_target.unwrap_or_default();
    let source_path = PathBuf::from(&path);
    let extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|value| SUPPORTED_EXT.contains(&value.as_str()))
        .ok_or_else(|| CommandErrorDto::new("UNSUPPORTED_FILE", "unsupported file type"))?;
    let metadata = std::fs::metadata(&source_path)
        .map_err(|error| CommandErrorDto::new("SOURCE_READ_FAILED", error.to_string()))?;
    if !metadata.is_file() {
        return Err(CommandErrorDto::new(
            "SOURCE_READ_FAILED",
            "source is not a regular file",
        ));
    }
    if metadata.len() > MAX_INPUT_BYTES {
        return Err(CommandErrorDto::new(
            "SOURCE_TOO_LARGE",
            "abnormally large file (rejected for safety)",
        ));
    }
    let source_bytes = std::fs::read(&source_path)
        .map_err(|error| CommandErrorDto::new("SOURCE_READ_FAILED", error.to_string()))?;
    let midi = parse_source_snapshot(&source_bytes, &extension)
        .map_err(|message| CommandErrorDto::new("SOURCE_PARSE_FAILED", message))?;
    let parsed_overrides: HashMap<usize, bool> = overrides
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| key.parse::<usize>().ok().map(|key| (key, value)))
        .collect();
    // A bundle carries the chosen target's project, so it gates on that target: a
    // source OpenUtau cannot represent must be refused here, before any staging
    // exists, rather than surfacing after the renderer has run.
    let outcome = engine::convert::convert_midi_with_target(
        &midi,
        language.as_deref().unwrap_or("english"),
        Some(&parsed_overrides),
        export_target,
    );
    if !outcome.ok {
        return Err(CommandErrorDto::new(
            "CONVERSION_FAILED",
            outcome
                .msg
                .unwrap_or_else(|| "source projection failed".into()),
        ));
    }

    let destination = PathBuf::from(target);
    let original_name = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            CommandErrorDto::new(
                "INVALID_SOURCE_NAME",
                "source filename cannot be represented safely",
            )
        })?
        .to_string();
    let layout = BundleLayout::new(&destination, &original_name, export_target)
        .map_err(CommandErrorDto::from)?;
    let stem_plan = stems::StemPlan::from_source(&midi, &outcome.tracks)
        .map_err(|error| CommandErrorDto::new("STEM_PLAN_INVALID", error.to_string()))?;
    let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);
    let manifest_warnings: Vec<String> = outcome
        .tracks
        .iter()
        .flat_map(|track| track.warnings.iter())
        .map(|warning| {
            let severity = match warning.severity {
                engine::convert::DiagnosticSeverity::Info => "info",
                engine::convert::DiagnosticSeverity::Warning => "warning",
            };
            match &warning.source_id {
                Some(source_id) => format!(
                    "[{severity}:{}] {} (source: {source_id})",
                    warning.code, warning.message
                ),
                None => format!("[{severity}:{}] {}", warning.code, warning.message),
            }
        })
        .collect();
    let projected = outcome.svp.ok_or_else(|| {
        CommandErrorDto::new(
            "CONVERSION_FAILED",
            format!(
                "no {} project produced",
                export_target.extension().to_ascii_uppercase()
            ),
        )
    })?;
    // The neutral projection becomes the chosen target's project at this boundary,
    // before the transaction starts; the bundle then appends its real audio-backed
    // stem references to whichever shape it received.
    let project = BundleProject::from_projection(export_target, &projected)
        .map_err(|message| CommandErrorDto::new("CONVERSION_FAILED", message))?;

    let config = MuseScoreConfig {
        executable: renderer_path
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from),
        timeout: DEFAULT_RENDER_TIMEOUT,
        max_wav_bytes: DEFAULT_MAX_WAV_BYTES,
    };
    let renderer = MuseScoreRenderer::discover(&config)
        .map_err(|error| CommandErrorDto::from(bundle::BundleError::Render(error)))?;
    write_bundle(
        BundleRequest {
            destination,
            input: BundleInput {
                original_name,
                source_format: source_format_name(midi.source_format).into(),
                source_bytes,
                project,
                stem_plan,
                ledger,
                warnings: manifest_warnings,
            },
            renderer: Arc::new(renderer),
            render_limits: RenderLimits {
                timeout: config.timeout,
                max_output_bytes: config.max_wav_bytes,
            },
        },
        progress,
    )
    .map_err(CommandErrorDto::from)
}

#[tauri::command]
async fn export_bundle(
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
    renderer_path: Option<String>,
    export_target: Option<ExportTarget>,
    on_progress: Channel<BundleProgressEvent>,
) -> Result<BundleResult, CommandErrorDto> {
    tauri::async_runtime::spawn_blocking(move || {
        export_bundle_blocking(
            path,
            target,
            language,
            overrides,
            renderer_path,
            export_target,
            &|event| {
                let _ = on_progress.send(event);
            },
        )
    })
    .await
    .map_err(|error| {
        CommandErrorDto::new(
            "BUNDLE_TASK_FAILED",
            format!("bundle worker did not complete: {error}"),
        )
    })?
}

#[tauri::command]
async fn renderer_status(renderer_path: Option<String>) -> RendererStatusDto {
    let configured = renderer_path
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let task = tauri::async_runtime::spawn_blocking(move || {
        let config = MuseScoreConfig {
            executable: renderer_path
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            ..MuseScoreConfig::default()
        };
        MuseScoreRenderer::discover(&config)
    })
    .await;
    match task {
        Ok(Ok(renderer)) => {
            let identity = &renderer.capabilities().identity;
            RendererStatusDto {
                state: "available".into(),
                configured,
                provider: Some(identity.provider.clone()),
                version: Some(identity.version.clone()),
                full_score_mix: identity.full_score_mix,
                message: None,
            }
        }
        Ok(Err(error @ renderer::RenderError::UnsupportedVersion { .. }))
        | Ok(Err(error @ renderer::RenderError::UnsupportedCapabilities { .. }))
        | Ok(Err(error @ renderer::RenderError::IncompatibleScore { .. }))
        | Ok(Err(error @ renderer::RenderError::ProbeRejected { .. })) => RendererStatusDto {
            state: "unsupported".into(),
            configured,
            provider: None,
            version: None,
            full_score_mix: false,
            message: Some(error.to_string()),
        },
        Ok(Err(error)) => RendererStatusDto {
            state: "missing".into(),
            configured,
            provider: None,
            version: None,
            full_score_mix: false,
            message: Some(error.to_string()),
        },
        Err(error) => RendererStatusDto {
            state: "missing".into(),
            configured,
            provider: None,
            version: None,
            full_score_mix: false,
            message: Some(format!("renderer probe did not complete: {error}")),
        },
    }
}

/// Analyses (`write = false`) or batch-exports (`write = true`) every path.
///
/// `export_target` is optional and defaults to Synthesizer V, so a caller that
/// names no target gets 0.4.9's analysis verdict and 0.4.9's bytes. It reaches
/// analysis and not only the writer because the timing a target accepts is part
/// of the convertibility verdict this returns.
#[tauri::command]
fn convert_files(
    paths: Vec<String>,
    write: bool,
    out_dir: Option<String>,
    language: Option<String>,
    overrides: Option<HashMap<String, HashMap<String, bool>>>,
    export_target: Option<ExportTarget>,
) -> Vec<FileResult> {
    let lang = language.as_deref().unwrap_or("english");
    let export_target = export_target.unwrap_or_default();
    let out_dir = out_dir.filter(|d| Path::new(d).is_dir());
    let overrides: HashMap<String, HashMap<usize, bool>> = overrides
        .unwrap_or_default()
        .into_iter()
        .map(|(p, m)| {
            let parsed = m
                .into_iter()
                .filter_map(|(k, v)| k.parse::<usize>().ok().map(|k| (k, v)))
                .collect();
            (p, parsed)
        })
        .collect();
    paths
        .iter()
        .map(|p| {
            process_one(
                p,
                write,
                out_dir.as_deref(),
                lang,
                overrides.get(p.as_str()),
                export_target,
            )
        })
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            convert_files,
            export_svp,
            export_bundle,
            renderer_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "verse-output-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn source_and_existing_outputs_are_never_accepted() {
        let root = temp_dir();
        let source = root.join("source.mid");
        std::fs::write(&source, b"source").unwrap();
        assert!(validate_new_output_target(&source, &source).is_err());

        let existing = root.join("existing.svp");
        std::fs::write(&existing, b"mine").unwrap();
        assert!(validate_new_output_target(&source, &existing).is_err());
        assert_eq!(std::fs::read(&existing).unwrap(), b"mine");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_output_writer_commits_once_without_overwrite() {
        let root = temp_dir();
        let target = root.join("new.svp");
        bundle::write_bytes_no_replace(&target, b"first").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"first");
        assert!(bundle::write_bytes_no_replace(&target, b"second").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"first");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The analysis pass, not the export, is what tells the user whether a
    /// source is convertible. A target that cannot represent the timing must
    /// therefore refuse here, with `write=false` and nothing written: moving
    /// that refusal to export time would have Verse call the file fine and then
    /// fail on the way out.
    #[test]
    fn analysis_still_refuses_timing_no_target_can_represent_exactly() {
        let root = temp_dir();
        let source = root.join("inexact.mid");
        // PPQ 1024: a one-tick duration is not representable on the
        // Synthesizer V blick grid, and is refused rather than rounded.
        let track: Vec<u8> = vec![
            0x00, 0xff, 0x05, 0x03, b'l', b'e', b't', //
            0x00, 0x90, 60, 100, //
            0x01, 0x80, 60, 0, //
            0x00, 0xff, 0x2f, 0x00,
        ];
        let mut data = b"MThd\0\0\0\x06\0\0\0\x01\x04\x00".to_vec();
        data.extend_from_slice(b"MTrk");
        data.extend_from_slice(&(track.len() as u32).to_be_bytes());
        data.extend_from_slice(&track);
        std::fs::write(&source, &data).unwrap();

        let result = process_one(
            source.to_str().unwrap(),
            false,
            None,
            "english",
            None,
            ExportTarget::Svp,
        );
        assert!(!result.ok, "an unprojectable source is not convertible");
        assert_eq!(
            result.msg.as_deref(),
            Some(
                "source timing cannot be projected safely: note duration on source track \
                 midi-track-0 at MIDI tick 1 cannot be represented exactly in Synthesizer V \
                 blicks with PPQ 1024"
            )
        );
        assert_eq!(
            result.error.as_ref().map(|error| error.code.as_str()),
            Some("CONVERSION_FAILED")
        );
        assert!(result.out.is_none(), "analysis writes nothing");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn track_dto_uses_explicit_camel_case_source_semantics() {
        let track = TrackInfo {
            id: 2,
            source_id: "part:P2:voice:1".into(),
            track: "Piano".into(),
            notes: 12,
            role: "backing".into(),
            placed: 0,
            source_role: SourceRole::Instrumental,
            lyric_status: LyricStatus {
                state: engine::convert::LyricStatusState::None,
                source_text_count: 0,
                projected_text_count: 0,
                explicit_empty_count: 0,
                continuation_count: 0,
                unsupported_count: 0,
            },
            export_representation: ExportRepresentation::ReferenceMixMember,
            requires_voice_assignment: false,
            warnings: vec![Diagnostic {
                code: "SOURCE_ONLY_TEST".into(),
                severity: engine::convert::DiagnosticSeverity::Info,
                message: "preserved".into(),
                source_id: Some("part:P2:voice:1".into()),
            }],
        };
        let value = serde_json::to_value(track).unwrap();
        assert_eq!(value["sourceId"], "part:P2:voice:1");
        assert_eq!(value["sourceRole"], "instrumental");
        assert_eq!(value["lyricStatus"]["state"], "none");
        assert_eq!(value["exportRepresentation"], "referenceMixMember");
        assert_eq!(value["requiresVoiceAssignment"], false);
        assert_eq!(value["warnings"][0]["severity"], "info");
        assert!(value.get("source_id").is_none());
    }

    #[test]
    fn audio_status_is_a_discriminated_camel_case_object() {
        let value = serde_json::to_value(AudioStatusDto::NotRendered).unwrap();
        assert_eq!(value["state"], "notRendered");
    }

    #[test]
    fn part_dto_groups_technical_chord_lanes_under_source_part() {
        use engine::midi::{SourcePart, SourceStaff, SourceVoice};

        let topology = SourceTopology {
            parts: vec![
                SourcePart {
                    id: "part:P1".into(),
                    name: "Voice".into(),
                    source_track_ids: vec![
                        "part:P1:voice:1:lane:1".into(),
                        "part:P1:voice:1:lane:2".into(),
                        "part:P1:unassigned-lyrics".into(),
                    ],
                    staves: vec![SourceStaff {
                        id: "staff:1".into(),
                        voices: vec![SourceVoice {
                            id: "part:P1:staff:1:voice:1".into(),
                            number: "1".into(),
                            projection_track_ids: vec![
                                "part:P1:voice:1:lane:1".into(),
                                "part:P1:voice:1:lane:2".into(),
                            ],
                        }],
                    }],
                },
                SourcePart {
                    id: "part:P2".into(),
                    name: "Piano".into(),
                    source_track_ids: vec!["part:P2:voice:1".into()],
                    staves: vec![SourceStaff {
                        id: "staff:2".into(),
                        voices: vec![SourceVoice {
                            id: "part:P2:staff:2:voice:1".into(),
                            number: "1".into(),
                            projection_track_ids: vec!["part:P2:voice:1".into()],
                        }],
                    }],
                },
            ],
        };
        let lyric_status = LyricStatus {
            state: engine::convert::LyricStatusState::SourceOwned,
            source_text_count: 1,
            projected_text_count: 1,
            explicit_empty_count: 0,
            continuation_count: 0,
            unsupported_count: 0,
        };
        let reports = vec![
            TrackReport {
                id: 0,
                source_id: "part:P1:voice:1:lane:1".into(),
                track: "Voice".into(),
                notes: 5,
                role: "vocal".into(),
                placed: 5,
                source_role: SourceRole::Vocal,
                lyric_status: lyric_status.clone(),
                export_representation: ExportRepresentation::VocalNotesAndReferenceMix,
                requires_voice_assignment: true,
                warnings: Vec::new(),
            },
            TrackReport {
                id: 1,
                source_id: "part:P1:voice:1:lane:2".into(),
                track: "Voice — polyphonic member 2".into(),
                notes: 3,
                role: "backing".into(),
                placed: 0,
                source_role: SourceRole::Vocal,
                lyric_status: LyricStatus {
                    state: engine::convert::LyricStatusState::None,
                    source_text_count: 0,
                    projected_text_count: 0,
                    explicit_empty_count: 0,
                    continuation_count: 0,
                    unsupported_count: 0,
                },
                export_representation: ExportRepresentation::ReferenceMixMember,
                requires_voice_assignment: false,
                warnings: Vec::new(),
            },
            TrackReport {
                id: 2,
                source_id: "part:P2:voice:1".into(),
                track: "Piano".into(),
                notes: 12,
                role: "backing".into(),
                placed: 0,
                source_role: SourceRole::Instrumental,
                lyric_status: LyricStatus {
                    state: engine::convert::LyricStatusState::None,
                    source_text_count: 0,
                    projected_text_count: 0,
                    explicit_empty_count: 0,
                    continuation_count: 0,
                    unsupported_count: 0,
                },
                export_representation: ExportRepresentation::ReferenceMixMember,
                requires_voice_assignment: false,
                warnings: Vec::new(),
            },
            TrackReport {
                id: 3,
                source_id: "part:P1:unassigned-lyrics".into(),
                track: "Voice — unassigned chord lyrics".into(),
                notes: 0,
                role: "metadata".into(),
                placed: 0,
                source_role: SourceRole::LyricsOnly,
                lyric_status: LyricStatus {
                    state: engine::convert::LyricStatusState::Ambiguous,
                    source_text_count: 1,
                    projected_text_count: 0,
                    explicit_empty_count: 0,
                    continuation_count: 0,
                    unsupported_count: 1,
                },
                export_representation: ExportRepresentation::SourceOnly,
                requires_voice_assignment: false,
                warnings: vec![Diagnostic {
                    code: "UNASSIGNED_CHORD_LYRIC".into(),
                    severity: engine::convert::DiagnosticSeverity::Warning,
                    message: "Source lyric remains source-only.".into(),
                    source_id: Some("part:P1:unassigned-lyrics".into()),
                }],
            },
        ];

        let parts = part_infos(&topology, &reports);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].voices, 1);
        assert_eq!(parts[0].track_ids, vec![0, 1, 3]);
        assert_eq!(parts[0].vocal_candidate_track_ids, vec![0, 1]);
        assert_eq!(parts[0].notes, 8);
        assert_eq!(parts[0].placed, 5);
        assert!(parts[0]
            .warnings
            .iter()
            .any(|warning| warning.code == "UNASSIGNED_CHORD_LYRIC"));
        assert_eq!(
            parts[0].export_representation,
            ExportRepresentation::VocalNotesAndReferenceMix
        );
        assert_eq!(parts[1].part, "Piano");
        assert_eq!(parts[1].track_ids, vec![2]);
        assert_eq!(parts[1].source_role, SourceRole::Instrumental);
    }

    fn detached_lyric_kar() -> Vec<u8> {
        let mut bytes = b"MThd\x00\x00\x00\x06\x00\x01\x00\x02\x01\xe0".to_vec();
        let lyric_track = b"\x00\xff\x05\x05Hello\x00\xff\x2f\x00";
        bytes.extend_from_slice(b"MTrk\x00\x00\x00\x0d");
        bytes.extend_from_slice(lyric_track);
        let melody_track = b"\x00\x90\x3c\x64\x83\x60\x80\x3c\x00\x00\xff\x2f\x00";
        bytes.extend_from_slice(b"MTrk\x00\x00\x00\x0d");
        bytes.extend_from_slice(melody_track);
        bytes
    }

    #[test]
    fn every_application_path_uses_the_kar_container_profile() {
        let root = temp_dir();
        let source = root.join("detached-lyrics.kar");
        std::fs::write(&source, detached_lyric_kar()).unwrap();

        let analysis = process_one(
            source.to_str().unwrap(),
            false,
            None,
            "english",
            None,
            ExportTarget::Svp,
        );
        assert!(analysis.ok, "{:?}", analysis.msg);
        assert_eq!(analysis.placed, 1);
        assert_eq!(
            analysis
                .tracks
                .iter()
                .map(|track| track.placed)
                .sum::<usize>(),
            1
        );

        let direct_target = root.join("direct.svp");
        export_svp(
            source.to_string_lossy().into_owned(),
            direct_target.to_string_lossy().into_owned(),
            Some("english".into()),
            None,
            None,
        )
        .unwrap();
        let direct: serde_json::Value =
            serde_json::from_slice(&std::fs::read(direct_target).unwrap()).unwrap();
        let direct_lyrics = direct["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|track| track["mainGroup"]["notes"].as_array().unwrap())
            .map(|note| note["lyrics"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(direct_lyrics, vec!["Hello"]);

        // The output path and the export format arrive as independent arguments
        // and the save dialog's filter is only advisory, so writing OpenUtau YAML
        // into a `.svp` — a file neither application opens — has to be refused
        // here rather than produced.
        let mismatched = export_svp(
            source.to_string_lossy().into_owned(),
            root.join("wrong-extension.svp")
                .to_string_lossy()
                .into_owned(),
            Some("english".into()),
            None,
            Some(ExportTarget::Ustx),
        );
        let error = mismatched.expect_err("a .svp path cannot hold an OpenUtau project");
        assert_eq!(error.code, "INVALID_OUTPUT");
        assert!(
            error.message.contains(".ustx"),
            "the message must name the extension the format needs: {}",
            error.message
        );
        assert!(
            !root.join("wrong-extension.svp").exists(),
            "nothing may be written when the format and the filename disagree"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_result_track_count_reports_projection_lanes_not_parts() {
        let root = temp_dir();
        let source = root.join("two-voices.musicxml");
        std::fs::write(
            &source,
            br#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name>Choir</part-name></score-part></part-list>
  <part id="P1"><measure number="1">
    <attributes><divisions>1</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>
    <note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration><voice>1</voice></note>
    <backup><duration>1</duration></backup>
    <note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration><voice>2</voice></note>
  </measure></part>
</score-partwise>"#,
        )
        .unwrap();

        let result = process_one(
            source.to_str().unwrap(),
            false,
            None,
            "english",
            None,
            ExportTarget::Svp,
        );
        assert!(result.ok, "{:?}", result.msg);
        assert_eq!(result.n_parts, 1);
        assert_eq!(result.n_voices, 2);
        assert_eq!(result.n_tracks, result.tracks.len());
        assert_eq!(result.n_tracks, 2);

        std::fs::remove_dir_all(root).unwrap();
    }
}
