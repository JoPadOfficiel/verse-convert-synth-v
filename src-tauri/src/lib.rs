pub mod bundle;
pub mod engine;
pub mod renderer;

use bundle::{
    build_preservation_ledger, export_bundle as write_bundle, BundleInput, BundleLayout,
    BundleRequest, BundleResult,
};
use engine::convert::{
    convert_auto_with, Diagnostic, ExportRepresentation, LyricStatus, SourceRole,
};
use engine::midi::{Midi, SourceFormat};
use renderer::{
    AudioRenderer, MuseScoreConfig, MuseScoreRenderer, RenderLimits, DEFAULT_MAX_WAV_BYTES,
    DEFAULT_RENDER_TIMEOUT,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    pub n_tracks: usize,
    pub placed: usize,
    pub tracks: Vec<TrackInfo>,
    pub audio_status: AudioStatusDto,
    pub requires_voice_assignment: bool,
    pub warnings: Vec<Diagnostic>,
    pub out: Option<String>,
}

fn svp_out_path(path: &str, out_dir: Option<&str>) -> String {
    let p = Path::new(path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let fname = format!("{}_LYRICS.svp", stem);
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
            "RENDERER_NOT_FOUND" => {
                Some("Configure MuseScore Studio 4, then retry the bundle export.")
            }
            "RENDERER_UNSUPPORTED" => Some("Install or select MuseScore Studio 4 or later."),
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

fn process_one(
    path: &str,
    write: bool,
    out_dir: Option<&str>,
    language: &str,
    overrides: Option<&HashMap<usize, bool>>,
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
        n_tracks: 0,
        placed: 0,
        tracks: vec![],
        audio_status: AudioStatusDto::NotRendered,
        requires_voice_assignment: false,
        warnings: vec![],
        out: None,
    };
    let ext_ok = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false);
    if !ext_ok {
        return err(name, "UNSUPPORTED_FILE", "unsupported file type".into());
    }
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
    let r = convert_auto_with(&data, language, overrides);
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
        let write_result = (|| -> Result<String, String> {
            let svp = r
                .svp
                .as_ref()
                .ok_or_else(|| "no SVP output was produced".to_string())?;
            let out_path = svp_out_path(path, out_dir);
            validate_new_output_target(Path::new(path), Path::new(&out_path))?;
            let json = serde_json::to_vec(svp)
                .map_err(|error| format!("cannot serialize SVP ({error})"))?;
            bundle::write_bytes_no_replace(Path::new(&out_path), &json)
                .map_err(|error| format!("cannot write SVP ({error})"))?;
            Ok(out_path)
        })();
        match write_result {
            Ok(path) => out = Some(path),
            Err(write_error) => {
                ok = false;
                msg = Some(write_error.clone());
                error = Some(CommandErrorDto::new("WRITE_FAILED", write_error));
            }
        }
    }
    FileResult {
        path: path.into(),
        name,
        ok,
        error,
        msg,
        n_tracks: r.n_tracks,
        placed: r.placed,
        tracks,
        audio_status: AudioStatusDto::NotRendered,
        requires_voice_assignment,
        warnings,
        out,
    }
}

#[tauri::command]
fn export_svp(
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
) -> Result<String, CommandErrorDto> {
    let ext_ok = Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false);
    if !ext_ok {
        return Err(CommandErrorDto::new(
            "UNSUPPORTED_FILE",
            "unsupported file type",
        ));
    }
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
    let r = convert_auto_with(&data, lang, Some(&ov));
    if !r.ok {
        return Err(CommandErrorDto::new(
            "CONVERSION_FAILED",
            r.msg.unwrap_or_else(|| "conversion failed".into()),
        ));
    }
    let svp = r
        .svp
        .ok_or_else(|| CommandErrorDto::new("CONVERSION_FAILED", "no output produced"))?;
    let source_path = Path::new(&path);
    let target_path = Path::new(&target);
    validate_new_output_target(source_path, target_path)
        .map_err(|message| CommandErrorDto::new("INVALID_OUTPUT", message))?;
    let json = serde_json::to_vec(&svp)
        .map_err(|error| CommandErrorDto::new("SERIALIZE_FAILED", error.to_string()))?;
    bundle::write_bytes_no_replace(target_path, &json).map_err(|error| {
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

fn export_bundle_blocking(
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
    renderer_path: Option<String>,
) -> Result<BundleResult, CommandErrorDto> {
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
    let outcome = engine::convert::convert_midi_with(
        &midi,
        language.as_deref().unwrap_or("english"),
        Some(&parsed_overrides),
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
    let layout = BundleLayout::new(&destination, &original_name).map_err(CommandErrorDto::from)?;
    let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout);
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
    let project = outcome
        .svp
        .ok_or_else(|| CommandErrorDto::new("CONVERSION_FAILED", "no SVP project produced"))?;

    let config = MuseScoreConfig {
        executable: renderer_path
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from),
        timeout: DEFAULT_RENDER_TIMEOUT,
        max_wav_bytes: DEFAULT_MAX_WAV_BYTES,
    };
    let renderer = MuseScoreRenderer::discover(&config)
        .map_err(|error| CommandErrorDto::from(bundle::BundleError::Render(error)))?;
    write_bundle(BundleRequest {
        destination,
        input: BundleInput {
            original_name,
            source_format: source_format_name(midi.source_format).into(),
            source_bytes,
            project,
            ledger,
            warnings: manifest_warnings,
        },
        renderer: Arc::new(renderer),
        render_limits: RenderLimits {
            timeout: config.timeout,
            max_output_bytes: config.max_wav_bytes,
        },
    })
    .map_err(CommandErrorDto::from)
}

#[tauri::command]
async fn export_bundle(
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
    renderer_path: Option<String>,
) -> Result<BundleResult, CommandErrorDto> {
    tauri::async_runtime::spawn_blocking(move || {
        export_bundle_blocking(path, target, language, overrides, renderer_path)
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

#[tauri::command]
fn convert_files(
    paths: Vec<String>,
    write: bool,
    out_dir: Option<String>,
    language: Option<String>,
    overrides: Option<HashMap<String, HashMap<String, bool>>>,
) -> Vec<FileResult> {
    let lang = language.as_deref().unwrap_or("english");
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
}
