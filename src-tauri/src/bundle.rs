//! Transactional preservation-bundle export.
//!
//! A bundle is the preservation unit: the original source is retained
//! byte-for-byte, genuine vocal material remains editable in the SVP, and the
//! original full score is rendered into a real audio-backed instrumental
//! track.

use crate::engine::convert::{
    attached_lyric_instance_id, note_instance_id, standalone_lyric_instance_id, ProjectionEvidence,
};
use crate::engine::midi::{Kind, Midi};
use crate::engine::svp::{append_instrumental_track, SvpProject};
use crate::renderer::{
    sha256_bytes, sha256_file, validate_wav, AudioRenderer, RenderError, RenderLimits,
    RendererIdentity,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const AUDIO_RELATIVE_PATH: &str = "audio/full-score.wav";
pub const PROJECT_AUDIO_REFERENCE: &str = "../audio/full-score.wav";
pub const PRESERVATION_RELATIVE_PATH: &str = "preservation.json";
pub const MANIFEST_RELATIVE_PATH: &str = "manifest.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct BundleLayout {
    pub project_relative_path: String,
    pub audio_relative_path: String,
    pub source_relative_path: String,
    pub preservation_relative_path: String,
    pub manifest_relative_path: String,
}

impl BundleLayout {
    pub fn new(destination: &Path, original_name: &str) -> Result<Self, BundleError> {
        validate_original_name(original_name)?;
        let stem = destination
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or(BundleError::InvalidDestination)?;
        let project_name = format!("{}.svp", sanitize_filename(stem));
        Ok(Self {
            project_relative_path: format!("project/{project_name}"),
            audio_relative_path: AUDIO_RELATIVE_PATH.into(),
            source_relative_path: format!("source/{original_name}"),
            preservation_relative_path: PRESERVATION_RELATIVE_PATH.into(),
            manifest_relative_path: MANIFEST_RELATIVE_PATH.into(),
        })
    }
}

pub struct BundleInput {
    pub original_name: String,
    pub source_format: String,
    pub source_bytes: Vec<u8>,
    pub project: SvpProject,
    pub ledger: PreservationLedger,
    /// Source/projection diagnostics that must remain visible after the UI is
    /// closed. These are copied verbatim into the auditable bundle manifest.
    pub warnings: Vec<String>,
}

pub struct BundleRequest {
    pub destination: PathBuf,
    pub input: BundleInput,
    pub renderer: Arc<dyn AudioRenderer>,
    pub render_limits: RenderLimits,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioArtifactRecord {
    #[serde(flatten)]
    pub artifact: ArtifactRecord,
    pub duration_seconds: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub frames: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentRecord {
    pub policy: String,
    pub svp_blick_offset: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub schema_version: u32,
    pub verse_version: String,
    pub source_format: String,
    pub source: ArtifactRecord,
    pub project: ArtifactRecord,
    pub audio: AudioArtifactRecord,
    pub preservation: ArtifactRecord,
    pub renderer: RendererIdentity,
    pub alignment: AlignmentRecord,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreservationLedger {
    pub schema_version: u32,
    pub expected_source_ids: Vec<String>,
    pub entries: Vec<DispositionEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DispositionEntry {
    pub source_id: String,
    pub item_kind: SourceItemKind,
    pub disposition: PrimaryDisposition,
    pub artifact_paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceItemKind {
    Track,
    Instrument,
    Event,
    Note,
    Lyric,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PrimaryDisposition {
    ProjectedExact,
    /// The source item was supplied to the full-score renderer. Its individual
    /// acoustic contribution cannot be proven from the mixed WAV alone.
    ReferenceMixCandidate,
    SourceOnly {
        reason: String,
    },
    MetadataOnly,
}

impl PreservationLedger {
    pub fn validate(&self, allowed_artifacts: &BTreeSet<String>) -> Result<(), BundleError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(BundleError::InvalidLedger(format!(
                "unsupported preservation schema version {}",
                self.schema_version
            )));
        }
        let expected: BTreeSet<_> = self.expected_source_ids.iter().cloned().collect();
        if expected.len() != self.expected_source_ids.len() {
            return Err(BundleError::InvalidLedger(
                "duplicate expected source ID".into(),
            ));
        }
        let actual: BTreeSet<_> = self
            .entries
            .iter()
            .map(|entry| entry.source_id.clone())
            .collect();
        if actual.len() != self.entries.len() {
            return Err(BundleError::InvalidLedger(
                "multiple dispositions for one source ID".into(),
            ));
        }
        if expected != actual {
            return Err(BundleError::InvalidLedger(
                "every inventoried item must have exactly one disposition".into(),
            ));
        }
        for entry in &self.entries {
            if entry.artifact_paths.is_empty() {
                return Err(BundleError::InvalidLedger(format!(
                    "{} has no preserving artifact",
                    entry.source_id
                )));
            }
            if entry
                .artifact_paths
                .iter()
                .any(|path| !allowed_artifacts.contains(path))
            {
                return Err(BundleError::InvalidLedger(format!(
                    "{} references an unknown artifact",
                    entry.source_id
                )));
            }
        }
        Ok(())
    }
}

/// Builds a complete disposition for each item retained by the current rich
/// source model. Every entry also points to the exact source snapshot.
pub fn build_preservation_ledger(
    midi: &Midi,
    projection: &ProjectionEvidence,
    layout: &BundleLayout,
) -> PreservationLedger {
    let mut entries = Vec::new();

    for track in &midi.tracks {
        push_entry(
            &mut entries,
            format!("track:{}", track.id),
            SourceItemKind::Track,
            PrimaryDisposition::ReferenceMixCandidate,
            artifact_paths(false, true, layout),
        );
        for (index, instrument) in track.instruments.iter().enumerate() {
            push_entry(
                &mut entries,
                format!(
                    "instrument:{}:{}:{}",
                    track.id,
                    index,
                    instrument.id.as_deref().unwrap_or("unnamed")
                ),
                SourceItemKind::Instrument,
                PrimaryDisposition::ReferenceMixCandidate,
                artifact_paths(false, true, layout),
            );
        }
        for event in &track.events {
            let event_id = format!("event:{}:{}", track.id, event.order);
            let projected = projection.source_ids.contains(&event_id);
            let (disposition, project, audio) = match &event.kind {
                Kind::NoteOn(_) | Kind::NoteOff(_) if projected => {
                    (PrimaryDisposition::ProjectedExact, true, true)
                }
                Kind::NoteOn(_) | Kind::NoteOff(_) => {
                    (PrimaryDisposition::ReferenceMixCandidate, false, true)
                }
                Kind::Tempo(_) | Kind::TimeSig { .. } if projected => {
                    (PrimaryDisposition::ProjectedExact, true, true)
                }
                Kind::Tempo(_) | Kind::TimeSig { .. } => (
                    PrimaryDisposition::SourceOnly {
                        reason: "the source timing event was not represented in SVP".into(),
                    },
                    false,
                    true,
                ),
                Kind::Lyrics(_) if projected => (PrimaryDisposition::ProjectedExact, true, false),
                Kind::Lyrics(_) => (
                    PrimaryDisposition::SourceOnly {
                        reason: "no exact vocal-note ownership was available".into(),
                    },
                    false,
                    false,
                ),
                Kind::TrackName(_) => (PrimaryDisposition::MetadataOnly, false, false),
                Kind::Text(_) | Kind::Meta { .. } | Kind::SysEx { .. } => (
                    PrimaryDisposition::SourceOnly {
                        reason: "retained in the byte-identical source".into(),
                    },
                    false,
                    false,
                ),
                _ => (PrimaryDisposition::ReferenceMixCandidate, false, true),
            };
            push_entry(
                &mut entries,
                event_id.clone(),
                SourceItemKind::Event,
                disposition,
                artifact_paths(project, audio, layout),
            );
            match &event.kind {
                Kind::NoteOn(note) if note.velocity != Some(0) => {
                    let note_id = note_instance_id(&track.id, &note.source, event.order);
                    let note_projected = projection.source_ids.contains(&note_id);
                    push_entry(
                        &mut entries,
                        note_id,
                        SourceItemKind::Note,
                        if note_projected {
                            PrimaryDisposition::ProjectedExact
                        } else {
                            PrimaryDisposition::ReferenceMixCandidate
                        },
                        artifact_paths(note_projected, true, layout),
                    );
                    for lyric in &note.lyrics {
                        let lyric_id = attached_lyric_instance_id(lyric, &note.source, event.order);
                        let lyric_projected = projection.source_ids.contains(&lyric_id);
                        push_entry(
                            &mut entries,
                            lyric_id,
                            SourceItemKind::Lyric,
                            if lyric_projected {
                                PrimaryDisposition::ProjectedExact
                            } else {
                                PrimaryDisposition::SourceOnly {
                                    reason:
                                        "this lyric occurrence was not projected to a vocal note"
                                            .into(),
                                }
                            },
                            artifact_paths(lyric_projected, false, layout),
                        );
                    }
                }
                Kind::Lyrics(lyric) => {
                    let lyric_id = standalone_lyric_instance_id(lyric, &track.id, event.order);
                    let projected = projection.source_ids.contains(&lyric_id);
                    push_entry(
                        &mut entries,
                        lyric_id,
                        SourceItemKind::Lyric,
                        if projected {
                            PrimaryDisposition::ProjectedExact
                        } else {
                            PrimaryDisposition::SourceOnly {
                                reason: "no exact vocal-note ownership was available".into(),
                            }
                        },
                        artifact_paths(projected, false, layout),
                    );
                }
                _ => {}
            }
        }
    }
    entries.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    let expected_source_ids = entries
        .iter()
        .map(|entry| entry.source_id.clone())
        .collect();
    PreservationLedger {
        schema_version: SCHEMA_VERSION,
        expected_source_ids,
        entries,
    }
}

fn push_entry(
    entries: &mut Vec<DispositionEntry>,
    source_id: String,
    item_kind: SourceItemKind,
    disposition: PrimaryDisposition,
    artifact_paths: Vec<String>,
) {
    entries.push(DispositionEntry {
        source_id,
        item_kind,
        disposition,
        artifact_paths,
    });
}

fn artifact_paths(project: bool, audio: bool, layout: &BundleLayout) -> Vec<String> {
    let mut paths = vec![layout.source_relative_path.clone()];
    if project {
        paths.push(layout.project_relative_path.clone());
    }
    if audio {
        paths.push(layout.audio_relative_path.clone());
    }
    paths
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleResult {
    pub bundle_path: PathBuf,
    pub project_path: PathBuf,
    pub audio_path: PathBuf,
    pub source_path: PathBuf,
    pub manifest_path: PathBuf,
    pub renderer: RendererIdentity,
    pub audio_duration_seconds: f64,
    pub audio_sample_rate: u32,
    pub audio_channels: u16,
    pub warnings: Vec<String>,
}

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("bundle destination must be a new .versebundle directory")]
    InvalidDestination,
    #[error("bundle destination already exists")]
    DestinationExists,
    #[error("source filename is unsafe or cannot be represented")]
    InvalidSourceName,
    #[error("renderer failed: {0}")]
    Render(#[from] RenderError),
    #[error("bundle I/O failed during {phase}: {source}")]
    Io {
        phase: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("cannot serialize bundle metadata: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("preservation ledger is incomplete: {0}")]
    InvalidLedger(String),
    #[error("bundle integrity validation failed: {0}")]
    Integrity(String),
    #[error("bundle commit failed: {0}")]
    Commit(String),
    #[cfg(test)]
    #[error("injected bundle failure at {0}")]
    Injected(String),
}

impl BundleError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidDestination => "INVALID_DESTINATION",
            Self::DestinationExists => "DESTINATION_EXISTS",
            Self::InvalidSourceName => "INVALID_SOURCE_NAME",
            Self::Render(RenderError::NotFound { .. } | RenderError::InvalidExecutable) => {
                "RENDERER_NOT_FOUND"
            }
            Self::Render(
                RenderError::UnsupportedVersion { .. } | RenderError::ProbeRejected { .. },
            ) => "RENDERER_UNSUPPORTED",
            Self::Render(RenderError::Timeout { .. }) => "RENDERER_TIMEOUT",
            Self::Render(_) => "RENDERER_FAILED",
            Self::Io { .. } => "BUNDLE_IO_FAILED",
            Self::Serialize(_) => "BUNDLE_SERIALIZE_FAILED",
            Self::InvalidLedger(_) => "PRESERVATION_INCOMPLETE",
            Self::Integrity(_) => "BUNDLE_INTEGRITY_FAILED",
            Self::Commit(_) => "BUNDLE_COMMIT_FAILED",
            #[cfg(test)]
            Self::Injected(_) => "INJECTED_TEST_FAILURE",
        }
    }
}

pub fn export_bundle(request: BundleRequest) -> Result<BundleResult, BundleError> {
    export_bundle_with_hook(request, &NoopHook)
}

fn export_bundle_with_hook(
    mut request: BundleRequest,
    hook: &dyn BundleHook,
) -> Result<BundleResult, BundleError> {
    validate_destination(&request.destination)?;
    let layout = BundleLayout::new(&request.destination, &request.input.original_name)?;
    let allowed_artifacts = BTreeSet::from([
        layout.source_relative_path.clone(),
        layout.project_relative_path.clone(),
        layout.audio_relative_path.clone(),
    ]);
    request.input.ledger.validate(&allowed_artifacts)?;

    let parent = request
        .destination
        .parent()
        .ok_or(BundleError::InvalidDestination)?;
    let mut staging = StagingGuard::create(parent, &request.destination)?;
    let root = staging.path().to_path_buf();
    for directory in ["source", "project", "audio", ".render-work"] {
        create_directory(&root.join(directory), "create staging directories")?;
    }

    let source_path = safe_join(&root, &layout.source_relative_path)?;
    write_new(
        &source_path,
        &request.input.source_bytes,
        "write source snapshot",
    )?;
    let source_hash = sha256_file(&source_path)?;
    if source_hash != sha256_bytes(&request.input.source_bytes) {
        return Err(BundleError::Integrity(
            "source snapshot differs from converted input bytes".into(),
        ));
    }
    hook.checkpoint(FaultPoint::AfterSource)?;

    let render_output = root.join(".render-work/full-score.wav");
    let rendered = request
        .renderer
        .render(&source_path, &render_output, &request.render_limits)?;
    if rendered.path != render_output {
        return Err(BundleError::Integrity(
            "renderer returned a path other than the owned render output".into(),
        ));
    }
    let rendered_metadata =
        fs::symlink_metadata(&rendered.path).map_err(|source| BundleError::Io {
            phase: "inspect renderer output path",
            source,
        })?;
    if !rendered_metadata.file_type().is_file()
        || fs::canonicalize(&rendered.path).map_err(|source| BundleError::Io {
            phase: "resolve renderer output path",
            source,
        })? != fs::canonicalize(&render_output).map_err(|source| BundleError::Io {
            phase: "resolve expected renderer output path",
            source,
        })?
    {
        return Err(BundleError::Integrity(
            "renderer returned an output outside the owned render path".into(),
        ));
    }
    let audio_path = safe_join(&root, &layout.audio_relative_path)?;
    fs::rename(&render_output, &audio_path).map_err(|source| BundleError::Io {
        phase: "publish rendered audio into staging",
        source,
    })?;
    let wav = validate_wav(&audio_path, request.render_limits.max_output_bytes)?;
    if wav.sha256 != rendered.wav.sha256 {
        return Err(BundleError::Integrity(
            "rendered WAV changed after validation".into(),
        ));
    }
    fs::remove_dir_all(root.join(".render-work")).map_err(|source| BundleError::Io {
        phase: "remove private renderer work directory",
        source,
    })?;
    hook.checkpoint(FaultPoint::AfterAudio)?;

    append_instrumental_track(
        &mut request.input.project,
        "Full score reference mix (MuseScore)".into(),
        PROJECT_AUDIO_REFERENCE.into(),
        wav.duration_seconds,
        0,
    );
    let project_path = safe_join(&root, &layout.project_relative_path)?;
    let project_json = serde_json::to_vec(&request.input.project)?;
    write_new(&project_path, &project_json, "write Synthesizer V project")?;
    hook.checkpoint(FaultPoint::AfterProject)?;

    let preservation_path = safe_join(&root, &layout.preservation_relative_path)?;
    let preservation_json = serde_json::to_vec_pretty(&request.input.ledger)?;
    write_new(
        &preservation_path,
        &preservation_json,
        "write preservation ledger",
    )?;
    hook.checkpoint(FaultPoint::AfterPreservation)?;

    let source_record = artifact_record(&root, &layout.source_relative_path)?;
    let project_record = artifact_record(&root, &layout.project_relative_path)?;
    let preservation_record = artifact_record(&root, &layout.preservation_relative_path)?;
    let audio_record = AudioArtifactRecord {
        artifact: artifact_record(&root, &layout.audio_relative_path)?,
        duration_seconds: wav.duration_seconds,
        sample_rate: wav.sample_rate,
        channels: wav.channels,
        bits_per_sample: wav.bits_per_sample,
        frames: wav.frames,
    };
    let mut warnings = request.input.warnings;
    warnings.push(
        "The audio asset is MuseScore's original full-score reference mix; vocal parts were not removed."
            .into(),
    );
    warnings.push(
        "Reference-mix candidates were supplied to MuseScore, but an item's individual acoustic contribution cannot be proven from the mixed WAV."
            .into(),
    );
    warnings.sort();
    warnings.dedup();
    let manifest = BundleManifest {
        schema_version: SCHEMA_VERSION,
        verse_version: env!("CARGO_PKG_VERSION").into(),
        source_format: request.input.source_format,
        source: source_record,
        project: project_record,
        audio: audio_record,
        preservation: preservation_record,
        renderer: rendered.renderer.clone(),
        alignment: AlignmentRecord {
            policy: "source-tick-zero".into(),
            svp_blick_offset: 0,
        },
        warnings,
    };
    let manifest_path = safe_join(&root, &layout.manifest_relative_path)?;
    write_new(
        &manifest_path,
        &serde_json::to_vec_pretty(&manifest)?,
        "write bundle manifest",
    )?;
    hook.checkpoint(FaultPoint::AfterManifest)?;
    verify_bundle(&root, &layout)?;
    hook.checkpoint(FaultPoint::BeforeCommit)?;

    sync_directory(&root, "sync staging directory")?;
    sync_directory(parent, "sync destination parent before commit")?;
    match rename_no_replace(&root, &request.destination) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(BundleError::DestinationExists)
        }
        Err(error) => return Err(BundleError::Commit(error.to_string())),
    }
    if let Err(error) = sync_directory(parent, "sync destination parent after commit") {
        remove_owned_destination(&request.destination);
        return Err(error);
    }
    if let Err(error) = hook.checkpoint(FaultPoint::AfterRename) {
        remove_owned_destination(&request.destination);
        return Err(error);
    }
    if let Err(error) = verify_bundle(&request.destination, &layout) {
        remove_owned_destination(&request.destination);
        return Err(error);
    }
    if let Err(source) = fs::remove_file(request.destination.join(".verse-staging")) {
        remove_owned_destination(&request.destination);
        return Err(BundleError::Io {
            phase: "finalize committed bundle",
            source,
        });
    }
    staging.commit();

    Ok(BundleResult {
        bundle_path: request.destination.clone(),
        project_path: request
            .destination
            .join(path_from_manifest(&layout.project_relative_path)),
        audio_path: request
            .destination
            .join(path_from_manifest(&layout.audio_relative_path)),
        source_path: request
            .destination
            .join(path_from_manifest(&layout.source_relative_path)),
        manifest_path: request
            .destination
            .join(path_from_manifest(&layout.manifest_relative_path)),
        renderer: rendered.renderer,
        audio_duration_seconds: wav.duration_seconds,
        audio_sample_rate: wav.sample_rate,
        audio_channels: wav.channels,
        warnings: manifest.warnings,
    })
}

fn remove_owned_destination(destination: &Path) {
    let marker = destination.join(".verse-staging");
    if fs::read(&marker).ok().as_deref() == Some(b"owned by Verse\n") {
        let _ = fs::remove_dir_all(destination);
    }
}

fn validate_destination(destination: &Path) -> Result<(), BundleError> {
    if destination.exists() {
        return Err(BundleError::DestinationExists);
    }
    if destination
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("versebundle"))
    {
        return Err(BundleError::InvalidDestination);
    }
    let parent = destination
        .parent()
        .ok_or(BundleError::InvalidDestination)?;
    if !parent.is_dir()
        || destination
            .file_name()
            .and_then(|value| value.to_str())
            .is_none()
    {
        return Err(BundleError::InvalidDestination);
    }
    Ok(())
}

fn validate_original_name(name: &str) -> Result<(), BundleError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || Path::new(name).file_name().and_then(|value| value.to_str()) != Some(name)
    {
        return Err(BundleError::InvalidSourceName);
    }
    Ok(())
}

fn sanitize_filename(stem: &str) -> String {
    let sanitized: String = stem
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = sanitized.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "project".into()
    } else {
        trimmed.into()
    }
}

fn create_directory(path: &Path, phase: &'static str) -> Result<(), BundleError> {
    fs::create_dir(path).map_err(|source| BundleError::Io { phase, source })
}

fn write_new(path: &Path, bytes: &[u8], phase: &'static str) -> Result<(), BundleError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| BundleError::Io { phase, source })?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|source| BundleError::Io { phase, source })
}

fn sync_directory(path: &Path, phase: &'static str) -> Result<(), BundleError> {
    #[cfg(unix)]
    {
        fs::File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| BundleError::Io { phase, source })?;
    }
    #[cfg(not(unix))]
    let _ = (path, phase);
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "android"))]
fn path_cstring(path: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains an embedded NUL byte",
        )
    })
}

#[cfg(target_os = "macos")]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let from = path_cstring(from)?;
    let to = path_cstring(to)?;
    // SAFETY: both pointers are valid NUL-terminated path strings and
    // `RENAME_EXCL` asks the kernel to fail if the destination exists.
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let from = path_cstring(from)?;
    let to = path_cstring(to)?;
    // SAFETY: the arguments are valid C strings and the syscall receives
    // fixed directory descriptors and the no-replace flag.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "windows")]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    // Windows' standard rename fails when the destination already exists.
    fs::rename(from, to)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "android",
    target_os = "windows"
)))]
fn rename_no_replace(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace directory rename is unavailable on this platform",
    ))
}

/// Writes a single output through a same-directory temporary file and commits
/// it with the same kernel-level no-replace primitive used by bundle export.
/// The destination is therefore never truncated or silently overwritten.
pub(crate) fn write_bytes_no_replace(destination: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "output has no parent directory",
        )
    })?;
    if !parent.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "output parent directory does not exist",
        ));
    }
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid output filename"))?;
    let mut last_collision = None;
    for attempt in 0..100_u64 {
        let temporary = parent.join(format!(
            ".{file_name}.verse-partial-{}-{attempt}",
            std::process::id()
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last_collision = Some(error);
                continue;
            }
            Err(error) => return Err(error),
        };
        let written = file.write_all(bytes).and_then(|_| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        #[cfg(unix)]
        if let Err(error) = fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        let committed = rename_no_replace(&temporary, destination);
        if let Err(error) = committed {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        #[cfg(unix)]
        fs::File::open(parent).and_then(|directory| directory.sync_all())?;
        return Ok(());
    }
    Err(last_collision.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot allocate a unique output staging file",
        )
    }))
}

fn artifact_record(root: &Path, relative_path: &str) -> Result<ArtifactRecord, BundleError> {
    let path = safe_join(root, relative_path)?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| BundleError::Io {
        phase: "inspect staged artifact",
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(BundleError::Integrity(format!(
            "{relative_path} is not a regular file"
        )));
    }
    Ok(ArtifactRecord {
        path: relative_path.into(),
        bytes: metadata.len(),
        sha256: sha256_file(&path)?,
    })
}

fn safe_join(root: &Path, relative_path: &str) -> Result<PathBuf, BundleError> {
    let relative = path_from_manifest(relative_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(BundleError::Integrity(format!(
            "unsafe artifact path: {relative_path}"
        )));
    }
    Ok(root.join(relative))
}

fn path_from_manifest(path: &str) -> PathBuf {
    path.split('/').collect()
}

fn verify_bundle(root: &Path, layout: &BundleLayout) -> Result<(), BundleError> {
    let manifest_path = safe_join(root, &layout.manifest_relative_path)?;
    let manifest: BundleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(|source| BundleError::Io {
            phase: "reopen bundle manifest",
            source,
        })?)?;
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(BundleError::Integrity(format!(
            "unsupported bundle manifest schema version {}",
            manifest.schema_version
        )));
    }
    for record in [&manifest.source, &manifest.project, &manifest.preservation] {
        verify_artifact(root, record)?;
    }
    verify_artifact(root, &manifest.audio.artifact)?;

    let audio_path = safe_join(root, &manifest.audio.artifact.path)?;
    let wav = validate_wav(&audio_path, manifest.audio.artifact.bytes)?;
    if wav.sha256 != manifest.audio.artifact.sha256
        || wav.sample_rate != manifest.audio.sample_rate
        || wav.channels != manifest.audio.channels
        || wav.bits_per_sample != manifest.audio.bits_per_sample
        || wav.frames != manifest.audio.frames
        || (wav.duration_seconds - manifest.audio.duration_seconds).abs() > 0.000_001
    {
        return Err(BundleError::Integrity(
            "WAV metadata differs from manifest".into(),
        ));
    }

    let project_path = safe_join(root, &manifest.project.path)?;
    let project: serde_json::Value =
        serde_json::from_slice(&fs::read(&project_path).map_err(|source| BundleError::Io {
            phase: "reopen Synthesizer V project",
            source,
        })?)?;
    let tracks = project["tracks"]
        .as_array()
        .ok_or_else(|| BundleError::Integrity("SVP has no tracks array".into()))?;
    let matching_instrumentals: Vec<_> = tracks
        .iter()
        .filter(|track| {
            track["mainRef"]["isInstrumental"] == serde_json::Value::Bool(true)
                && track["mainRef"]["audio"]["filename"] == PROJECT_AUDIO_REFERENCE
        })
        .collect();
    if matching_instrumentals.len() != 1 {
        return Err(BundleError::Integrity(format!(
            "SVP must contain exactly one bundle-owned instrumental audio track, found {}",
            matching_instrumentals.len()
        )));
    }
    let instrumental = matching_instrumentals[0];
    if instrumental["mainRef"]["audio"]["filename"] != PROJECT_AUDIO_REFERENCE
        || instrumental["mainRef"]["blickOffset"] != 0
        || instrumental["mainGroup"]["notes"] != serde_json::json!([])
    {
        return Err(BundleError::Integrity(
            "SVP instrumental track has an invalid schema".into(),
        ));
    }
    let referenced = project_path
        .parent()
        .ok_or_else(|| BundleError::Integrity("SVP project has no parent".into()))?
        .join(PROJECT_AUDIO_REFERENCE);
    let referenced = fs::canonicalize(referenced).map_err(|source| BundleError::Io {
        phase: "resolve SVP audio reference",
        source,
    })?;
    let canonical_root = fs::canonicalize(root).map_err(|source| BundleError::Io {
        phase: "resolve bundle root",
        source,
    })?;
    let canonical_audio = fs::canonicalize(audio_path).map_err(|source| BundleError::Io {
        phase: "resolve validated audio asset",
        source,
    })?;
    if !referenced.starts_with(&canonical_root) || referenced != canonical_audio {
        return Err(BundleError::Integrity(
            "SVP audio reference escapes or mismatches the bundle".into(),
        ));
    }

    let ledger_path = safe_join(root, &manifest.preservation.path)?;
    let ledger: PreservationLedger =
        serde_json::from_slice(&fs::read(ledger_path).map_err(|source| BundleError::Io {
            phase: "reopen preservation ledger",
            source,
        })?)?;
    let allowed = BTreeSet::from([
        manifest.source.path,
        manifest.project.path,
        manifest.audio.artifact.path,
    ]);
    ledger.validate(&allowed)
}

fn verify_artifact(root: &Path, record: &ArtifactRecord) -> Result<(), BundleError> {
    let path = safe_join(root, &record.path)?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| BundleError::Io {
        phase: "reopen bundle artifact",
        source,
    })?;
    if !metadata.file_type().is_file()
        || metadata.len() != record.bytes
        || sha256_file(&path)? != record.sha256
    {
        return Err(BundleError::Integrity(format!(
            "{} failed its size/hash check",
            record.path
        )));
    }
    Ok(())
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

struct StagingGuard {
    path: PathBuf,
    committed: bool,
}

impl StagingGuard {
    fn create(parent: &Path, destination: &Path) -> Result<Self, BundleError> {
        let destination_name = destination
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(BundleError::InvalidDestination)?;
        for _ in 0..100 {
            let counter = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = parent.join(format!(
                ".{destination_name}.partial-{}-{timestamp}-{counter}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    let guard = Self {
                        path,
                        committed: false,
                    };
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&guard.path, fs::Permissions::from_mode(0o700))
                            .map_err(|source| BundleError::Io {
                                phase: "secure sibling staging directory",
                                source,
                            })?;
                    }
                    write_new(
                        &guard.path.join(".verse-staging"),
                        b"owned by Verse\n",
                        "write staging ownership marker",
                    )?;
                    return Ok(guard);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(BundleError::Io {
                        phase: "create sibling staging directory",
                        source,
                    })
                }
            }
        }
        Err(BundleError::Commit(
            "cannot allocate a unique staging directory".into(),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if !self.committed
            && (self.path.join(".verse-staging").is_file()
                || self
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".partial-")))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FaultPoint {
    AfterSource,
    AfterAudio,
    AfterProject,
    AfterPreservation,
    AfterManifest,
    BeforeCommit,
    AfterRename,
}

trait BundleHook {
    fn checkpoint(&self, point: FaultPoint) -> Result<(), BundleError>;
}

struct NoopHook;

impl BundleHook for NoopHook {
    fn checkpoint(&self, _point: FaultPoint) -> Result<(), BundleError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::svp::{RenderConfig, Time};
    use crate::renderer::{RendererCapabilities, WavInfo};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Copy)]
    enum FakeMode {
        Success,
        Missing,
        Corrupt,
        Timeout,
    }

    struct FakeRenderer {
        mode: FakeMode,
        capabilities: RendererCapabilities,
    }

    struct ExternalPathRenderer {
        external: PathBuf,
        capabilities: RendererCapabilities,
    }

    impl AudioRenderer for ExternalPathRenderer {
        fn capabilities(&self) -> &RendererCapabilities {
            &self.capabilities
        }

        fn render(
            &self,
            _input: &Path,
            _output: &Path,
            limits: &RenderLimits,
        ) -> Result<crate::renderer::RenderedAudio, RenderError> {
            write_test_wav(&self.external);
            let wav = validate_wav(&self.external, limits.max_output_bytes)?;
            Ok(crate::renderer::RenderedAudio {
                path: self.external.clone(),
                wav,
                renderer: self.capabilities.identity.clone(),
            })
        }
    }

    impl FakeRenderer {
        fn new(mode: FakeMode) -> Self {
            Self {
                mode,
                capabilities: RendererCapabilities {
                    identity: RendererIdentity {
                        provider: "fake-musescore".into(),
                        version: "MuseScore 4.99-test".into(),
                        executable_sha256: "00".repeat(32),
                        full_score_mix: true,
                    },
                    supported_extensions: vec!["mid", "mscz", "mxl"],
                    output_format: "wav",
                },
            }
        }
    }

    impl AudioRenderer for FakeRenderer {
        fn capabilities(&self) -> &RendererCapabilities {
            &self.capabilities
        }

        fn render(
            &self,
            _input: &Path,
            output: &Path,
            limits: &RenderLimits,
        ) -> Result<crate::renderer::RenderedAudio, RenderError> {
            match self.mode {
                FakeMode::Missing => return Err(RenderError::MissingOutput),
                FakeMode::Timeout => {
                    return Err(RenderError::Timeout {
                        milliseconds: limits.timeout.as_millis() as u64,
                    })
                }
                FakeMode::Corrupt => {
                    fs::write(output, b"not a wave").unwrap();
                }
                FakeMode::Success => write_test_wav(output),
            }
            let wav = validate_wav(output, limits.max_output_bytes)?;
            Ok(crate::renderer::RenderedAudio {
                path: output.into(),
                wav,
                renderer: self.capabilities.identity.clone(),
            })
        }
    }

    fn write_test_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for sample in 0..882 {
            writer
                .write_sample::<i16>(if sample == 441 { 2_000 } else { 0 })
                .unwrap();
        }
        writer.finalize().unwrap();
    }

    fn temp_dir(label: &str) -> PathBuf {
        let count = TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "verse-bundle-{label}-{}-{count}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        fs::create_dir(&path).unwrap();
        path
    }

    fn empty_project() -> SvpProject {
        SvpProject {
            version: 113,
            time: Time {
                meter: vec![],
                tempo: vec![],
            },
            render_config: RenderConfig::default(),
            tracks: vec![],
        }
    }

    fn request(root: &Path, mode: FakeMode) -> BundleRequest {
        let destination = root.join("Song.versebundle");
        let layout = BundleLayout::new(&destination, "source.mid").unwrap();
        let entry = DispositionEntry {
            source_id: "track:midi-track-0".into(),
            item_kind: SourceItemKind::Track,
            disposition: PrimaryDisposition::ReferenceMixCandidate,
            artifact_paths: vec![
                layout.source_relative_path.clone(),
                layout.audio_relative_path.clone(),
            ],
        };
        BundleRequest {
            destination,
            input: BundleInput {
                original_name: "source.mid".into(),
                source_format: "standardMidi".into(),
                source_bytes: b"MThd source snapshot".to_vec(),
                project: empty_project(),
                ledger: PreservationLedger {
                    schema_version: SCHEMA_VERSION,
                    expected_source_ids: vec![entry.source_id.clone()],
                    entries: vec![entry],
                },
                warnings: vec!["[TEST_WARNING] retained diagnostic".into()],
            },
            renderer: Arc::new(FakeRenderer::new(mode)),
            render_limits: RenderLimits {
                timeout: std::time::Duration::from_millis(10),
                max_output_bytes: 1024 * 1024,
            },
        }
    }

    fn smf(track: &[u8]) -> Vec<u8> {
        let mut data = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        data.extend_from_slice(&(track.len() as u32).to_be_bytes());
        data.extend_from_slice(track);
        data
    }

    #[test]
    fn successful_bundle_is_source_exact_and_audio_backed() {
        let root = temp_dir("success");
        let result = export_bundle(request(&root, FakeMode::Success)).unwrap();
        assert_eq!(
            fs::read(&result.source_path).unwrap(),
            b"MThd source snapshot"
        );
        let project: serde_json::Value =
            serde_json::from_slice(&fs::read(&result.project_path).unwrap()).unwrap();
        assert_eq!(project["tracks"].as_array().unwrap().len(), 1);
        assert_eq!(project["tracks"][0]["mainRef"]["isInstrumental"], true);
        assert_eq!(
            project["tracks"][0]["mainRef"]["audio"]["filename"],
            PROJECT_AUDIO_REFERENCE
        );
        let manifest: BundleManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
        assert_eq!(
            manifest.source.sha256,
            sha256_bytes(b"MThd source snapshot")
        );
        assert!(manifest.audio.duration_seconds > 0.0);
        assert!(manifest
            .warnings
            .iter()
            .any(|warning| warning.contains("[TEST_WARNING]")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verification_selects_the_bundle_owned_audio_reference() {
        let root = temp_dir("preexisting-instrumental");
        let mut request = request(&root, FakeMode::Success);
        append_instrumental_track(
            &mut request.input.project,
            "Existing instrumental".into(),
            "legacy-audio.wav".into(),
            1.0,
            0,
        );
        let result = export_bundle(request).expect("bundle-owned track is unambiguous");
        let project: serde_json::Value =
            serde_json::from_slice(&fs::read(result.project_path).unwrap()).unwrap();
        assert_eq!(
            project["tracks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|track| {
                    track["mainRef"]["audio"]["filename"] == PROJECT_AUDIO_REFERENCE
                })
                .count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renderer_failures_roll_back_everything() {
        for (index, mode) in [FakeMode::Missing, FakeMode::Corrupt, FakeMode::Timeout]
            .into_iter()
            .enumerate()
        {
            let root = temp_dir(&format!("renderer-failure-{index}"));
            let destination = root.join("Song.versebundle");
            assert!(export_bundle(request(&root, mode)).is_err());
            assert!(!destination.exists());
            assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn renderer_cannot_substitute_an_external_output_path() {
        let root = temp_dir("external-render-path");
        let external = root.join("outside.wav");
        let mut request = request(&root, FakeMode::Success);
        request.renderer = Arc::new(ExternalPathRenderer {
            external: external.clone(),
            capabilities: RendererCapabilities {
                identity: RendererIdentity {
                    provider: "fake-musescore".into(),
                    version: "MuseScore 4.99-test".into(),
                    executable_sha256: "00".repeat(32),
                    full_score_mix: true,
                },
                supported_extensions: vec!["mid"],
                output_format: "wav",
            },
        });
        let destination = request.destination.clone();
        assert!(matches!(
            export_bundle(request),
            Err(BundleError::Integrity(_))
        ));
        assert!(!destination.exists());
        assert!(external.exists(), "external file is never moved or deleted");
        fs::remove_dir_all(root).unwrap();
    }

    struct FailAt(FaultPoint);

    impl BundleHook for FailAt {
        fn checkpoint(&self, point: FaultPoint) -> Result<(), BundleError> {
            if point == self.0 {
                Err(BundleError::Injected(format!("{point:?}")))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn every_transaction_phase_rolls_back() {
        for (index, point) in [
            FaultPoint::AfterSource,
            FaultPoint::AfterAudio,
            FaultPoint::AfterProject,
            FaultPoint::AfterPreservation,
            FaultPoint::AfterManifest,
            FaultPoint::BeforeCommit,
            FaultPoint::AfterRename,
        ]
        .into_iter()
        .enumerate()
        {
            let root = temp_dir(&format!("phase-{index}"));
            let destination = root.join("Song.versebundle");
            assert!(
                export_bundle_with_hook(request(&root, FakeMode::Success), &FailAt(point)).is_err()
            );
            assert!(!destination.exists(), "failed at {point:?}");
            assert_eq!(
                fs::read_dir(&root).unwrap().count(),
                0,
                "failed at {point:?}"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn an_existing_target_is_never_modified() {
        let root = temp_dir("existing");
        let destination = root.join("Song.versebundle");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("mine.txt"), b"keep me").unwrap();
        let error = export_bundle(request(&root, FakeMode::Success)).unwrap_err();
        assert!(matches!(error, BundleError::DestinationExists));
        assert_eq!(fs::read(destination.join("mine.txt")).unwrap(), b"keep me");
        fs::remove_dir_all(root).unwrap();
    }

    struct CreateDestinationAtCommit {
        destination: PathBuf,
    }

    impl BundleHook for CreateDestinationAtCommit {
        fn checkpoint(&self, point: FaultPoint) -> Result<(), BundleError> {
            if point == FaultPoint::BeforeCommit {
                fs::create_dir(&self.destination).unwrap();
                fs::write(self.destination.join("sentinel.txt"), b"external").unwrap();
            }
            Ok(())
        }
    }

    #[test]
    fn a_target_created_during_commit_is_never_replaced_or_deleted() {
        let root = temp_dir("commit-race");
        let destination = root.join("Song.versebundle");
        let hook = CreateDestinationAtCommit {
            destination: destination.clone(),
        };
        let error = export_bundle_with_hook(request(&root, FakeMode::Success), &hook).unwrap_err();
        assert!(matches!(error, BundleError::DestinationExists));
        assert_eq!(
            fs::read(destination.join("sentinel.txt")).unwrap(),
            b"external"
        );
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            1,
            "only the external destination should remain"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_ledger_is_blocking() {
        let root = temp_dir("ledger");
        let mut request = request(&root, FakeMode::Success);
        request
            .input
            .ledger
            .expected_source_ids
            .push("missing".into());
        assert!(matches!(
            export_bundle(request),
            Err(BundleError::InvalidLedger(_))
        ));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_ledger_schema_is_blocking() {
        let root = temp_dir("ledger-schema");
        let mut request = request(&root, FakeMode::Success);
        request.input.ledger.schema_version = SCHEMA_VERSION + 1;
        assert!(matches!(
            export_bundle(request),
            Err(BundleError::InvalidLedger(_))
        ));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_source_ids_are_rejected_instead_of_deduplicated() {
        let allowed = BTreeSet::from(["source/source.mid".to_string()]);
        let entry = DispositionEntry {
            source_id: "duplicate".into(),
            item_kind: SourceItemKind::Event,
            disposition: PrimaryDisposition::SourceOnly {
                reason: "test".into(),
            },
            artifact_paths: vec!["source/source.mid".into()],
        };
        let ledger = PreservationLedger {
            schema_version: SCHEMA_VERSION,
            expected_source_ids: vec!["duplicate".into(), "duplicate".into()],
            entries: vec![entry.clone(), entry],
        };
        assert!(matches!(
            ledger.validate(&allowed),
            Err(BundleError::InvalidLedger(_))
        ));
    }

    #[test]
    fn ledger_uses_per_item_projection_evidence_not_a_global_lyric_count() {
        let data = smf(&[
            0x00, 0xff, 0x05, 0x03, b'l', b'e', b't', // aligned lyric
            0x00, 0x90, 60, 100, // note
            0x81, 0x70, 0x80, 60, 0, // note off at 240
            0x00, 0xff, 0x05, 0x06, b'o', b'r', b'p', b'h', b'a', b'n', // no note
            0x00, 0xff, 0x2f, 0x00,
        ]);
        let midi = crate::engine::midi::parse(&data).unwrap();
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert_eq!(outcome.placed, 1);
        let root = temp_dir("evidence-ledger");
        let layout = BundleLayout::new(&root.join("Song.versebundle"), "source.mid").unwrap();
        let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout);
        let lyric_entries: Vec<_> = ledger
            .entries
            .iter()
            .filter(|entry| entry.item_kind == SourceItemKind::Lyric)
            .collect();
        assert_eq!(lyric_entries.len(), 2);
        assert_eq!(
            lyric_entries
                .iter()
                .filter(|entry| {
                    matches!(&entry.disposition, PrimaryDisposition::ProjectedExact)
                })
                .count(),
            1
        );
        assert_eq!(
            lyric_entries
                .iter()
                .filter(|entry| matches!(&entry.disposition, PrimaryDisposition::SourceOnly { .. }))
                .count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lyric_free_project_remains_lyric_free() {
        let root = temp_dir("lyric-free");
        let result = export_bundle(request(&root, FakeMode::Success)).unwrap();
        let project: serde_json::Value =
            serde_json::from_slice(&fs::read(result.project_path).unwrap()).unwrap();
        let tracks = project["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["mainGroup"]["notes"], serde_json::json!([]));
        assert!(tracks[0]["mainRef"]["isInstrumental"].as_bool().unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wav_info_type_stays_manifest_compatible() {
        let info = WavInfo {
            bytes: 1,
            sha256: "a".into(),
            duration_seconds: 1.0,
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            frames: 44_100,
        };
        assert_eq!(serde_json::to_value(info).unwrap()["durationSeconds"], 1.0);
    }

    #[test]
    fn supplied_scores_build_complete_fake_render_bundles_when_configured() {
        for (variable, source_format) in [
            ("VERSE_MSCZ_GATE", "museScore"),
            ("VERSE_MXL_GATE", "musicXml"),
        ] {
            let Ok(source_path) = std::env::var(variable) else {
                continue;
            };
            let source_path = PathBuf::from(source_path);
            let source_bytes = fs::read(&source_path).expect("read configured supplied score");
            let midi = if variable == "VERSE_MSCZ_GATE" {
                crate::engine::musescore::parse(&source_bytes).expect("parse supplied MuseScore")
            } else {
                crate::engine::musicxml::parse(&source_bytes).expect("parse supplied MusicXML")
            };
            let outcome = crate::engine::convert::convert_midi(&midi, "english");
            assert!(outcome.ok, "{:?}", outcome.msg);

            let root = temp_dir(&format!("supplied-{}", source_format.to_ascii_lowercase()));
            let destination = root.join("Supplied.versebundle");
            let original_name = source_path
                .file_name()
                .and_then(|value| value.to_str())
                .expect("Unicode fixture name")
                .to_string();
            let layout = BundleLayout::new(&destination, &original_name).unwrap();
            let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout);
            assert!(
                ledger.entries.len() > 900,
                "the real source inventory must be represented"
            );
            let result = export_bundle(BundleRequest {
                destination,
                input: BundleInput {
                    original_name,
                    source_format: source_format.into(),
                    source_bytes: source_bytes.clone(),
                    project: outcome.svp.expect("SVP projection"),
                    ledger,
                    warnings: vec![],
                },
                renderer: Arc::new(FakeRenderer::new(FakeMode::Success)),
                render_limits: RenderLimits {
                    timeout: std::time::Duration::from_secs(1),
                    max_output_bytes: 1024 * 1024,
                },
            })
            .expect("complete fake-render bundle");
            assert_eq!(fs::read(result.source_path).unwrap(), source_bytes);
            assert!(fs::metadata(result.audio_path).unwrap().len() > 44);
            fs::remove_dir_all(root).unwrap();
        }
    }
}
