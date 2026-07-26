//! Transactional preservation-bundle export.
//!
//! A bundle is the preservation unit: the original source is retained
//! byte-for-byte, genuine vocal material remains editable in the SVP, and the
//! note-bearing source Parts are rendered into real audio-backed instrumental
//! tracks alongside a muted full-score reference.

use crate::engine::convert::{
    attached_lyric_instance_id, karaoke_text_lyric, note_instance_id, standalone_lyric_instance_id,
    ProjectionEvidence,
};
use crate::engine::midi::{self, Kind, Midi, MidiTextProfile, SourceTopology};
use crate::engine::svp::{append_instrumental_track, SvpProject};
use crate::renderer::{
    sha256_bytes, sha256_file, validate_wav, validate_wav_allowing_silence, AudioRenderer,
    ExtractedScorePart, RenderError, RenderLimits, RendererIdentity, WavInfo,
};
use crate::stems::{StemDescriptor, StemPlan, StemRole};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const AUDIO_RELATIVE_PATH: &str = "audio/full-score.wav";
pub const PROJECT_AUDIO_REFERENCE: &str = "../audio/full-score.wav";
pub const STEM_AUDIO_DIRECTORY: &str = "audio/stems";
pub const PRESERVATION_RELATIVE_PATH: &str = "preservation.json";
pub const MANIFEST_RELATIVE_PATH: &str = "manifest.json";
const SCHEMA_VERSION: u32 = 2;
const MAX_TOTAL_AUDIO_BYTES: u64 = 8 * 1024 * 1024 * 1024;

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

    pub fn stem_audio_relative_path(&self, stem: &StemDescriptor) -> String {
        format!(
            "{STEM_AUDIO_DIRECTORY}/{}-{}.wav",
            stem.stem_id,
            sanitize_stem_slug(&stem.display_name)
        )
    }

    pub fn project_stem_audio_reference(&self, stem: &StemDescriptor) -> String {
        format!("../{}", self.stem_audio_relative_path(stem))
    }
}

pub struct BundleInput {
    pub original_name: String,
    pub source_format: String,
    pub source_bytes: Vec<u8>,
    pub project: SvpProject,
    pub stem_plan: StemPlan,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceMixRecord {
    pub asset: AudioArtifactRecord,
    pub svp_group_id: String,
    pub muted_by_default: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StemAudioRecord {
    pub stem_id: String,
    pub display_name: String,
    pub source_part_id: String,
    pub source_track_ids: Vec<String>,
    pub role: StemRole,
    pub isolation_method: String,
    pub active_by_default: bool,
    pub asset: AudioArtifactRecord,
    pub svp_group_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioCoverageRecord {
    pub complete: bool,
    pub expected_stem_ids: Vec<String>,
    pub rendered_stem_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BundleAudioRecord {
    pub reference_mix: ReferenceMixRecord,
    pub stems: Vec<StemAudioRecord>,
    pub coverage: AudioCoverageRecord,
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
    pub audio: BundleAudioRecord,
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
    /// The source item belongs to a source-owned Part rendered as an isolated
    /// audio stem.
    RenderedStem {
        stem_id: String,
    },
    /// Retained for schema readability when reopening older diagnostic data.
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
    stem_plan: &StemPlan,
) -> PreservationLedger {
    let mut entries = Vec::new();
    let stem_by_track = stem_plan
        .stems
        .iter()
        .flat_map(|stem| {
            let path = layout.stem_audio_relative_path(stem);
            stem.source_track_ids
                .iter()
                .map(move |track_id| (track_id.as_str(), (stem.stem_id.as_str(), path.clone())))
        })
        .collect::<BTreeMap<_, _>>();

    for track in &midi.tracks {
        let stem = stem_by_track.get(track.id.as_str());
        let stem_disposition = || {
            stem.map_or(
                PrimaryDisposition::SourceOnly {
                    reason: "this source lane has no note-bearing Part stem".into(),
                },
                |(stem_id, _)| PrimaryDisposition::RenderedStem {
                    stem_id: (*stem_id).to_string(),
                },
            )
        };
        push_entry(
            &mut entries,
            format!("track:{}", track.id),
            SourceItemKind::Track,
            stem_disposition(),
            artifact_paths(false, stem.map(|(_, path)| path.as_str()), layout),
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
                stem_disposition(),
                artifact_paths(false, stem.map(|(_, path)| path.as_str()), layout),
            );
        }
        for event in &track.events {
            let event_id = format!("event:{}:{}", track.id, event.order);
            let projected = projection.source_ids.contains(&event_id);
            let (disposition, project, include_stem) = match &event.kind {
                Kind::NoteOn(_) | Kind::NoteOff(_) if projected => {
                    (PrimaryDisposition::ProjectedExact, true, true)
                }
                Kind::NoteOn(_) | Kind::NoteOff(_) => (stem_disposition(), false, true),
                Kind::Tempo(_) | Kind::TimeSig { .. } if projected => {
                    (PrimaryDisposition::ProjectedExact, true, true)
                }
                Kind::Tempo(_) | Kind::TimeSig { .. } => (
                    PrimaryDisposition::SourceOnly {
                        reason: "the source timing event was not represented in SVP".into(),
                    },
                    false,
                    stem.is_some(),
                ),
                Kind::Lyrics(lyric) if !midi::is_midi_lyric_line_break(&lyric.raw) && projected => {
                    (PrimaryDisposition::ProjectedExact, true, false)
                }
                Kind::Lyrics(lyric) if !midi::is_midi_lyric_line_break(&lyric.raw) => (
                    PrimaryDisposition::SourceOnly {
                        reason: "no exact vocal-note ownership was available".into(),
                    },
                    false,
                    false,
                ),
                Kind::Lyrics(_) => (PrimaryDisposition::MetadataOnly, false, false),
                Kind::Text(text)
                    if track.text_profile == MidiTextProfile::KaraokeLyrics
                        && karaoke_text_lyric(&track.id, event.tick, event.order, text)
                            .is_some_and(|lyric| {
                                projection
                                    .source_ids
                                    .contains(&standalone_lyric_instance_id(
                                        &lyric,
                                        &track.id,
                                        event.order,
                                    ))
                            }) =>
                {
                    (PrimaryDisposition::ProjectedExact, true, false)
                }
                Kind::TrackName(_) => (PrimaryDisposition::MetadataOnly, false, false),
                Kind::Text(_) | Kind::Meta { .. } | Kind::SysEx { .. } => (
                    PrimaryDisposition::SourceOnly {
                        reason: "retained in the byte-identical source".into(),
                    },
                    false,
                    false,
                ),
                _ if stem.is_some() => (stem_disposition(), false, true),
                _ => (
                    PrimaryDisposition::SourceOnly {
                        reason: "retained in the byte-identical source".into(),
                    },
                    false,
                    false,
                ),
            };
            push_entry(
                &mut entries,
                event_id.clone(),
                SourceItemKind::Event,
                disposition,
                artifact_paths(
                    project,
                    include_stem
                        .then(|| stem.map(|(_, path)| path.as_str()))
                        .flatten(),
                    layout,
                ),
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
                            stem_disposition()
                        },
                        artifact_paths(note_projected, stem.map(|(_, path)| path.as_str()), layout),
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
                            artifact_paths(lyric_projected, None, layout),
                        );
                    }
                }
                Kind::Lyrics(lyric) if !midi::is_midi_lyric_line_break(&lyric.raw) => {
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
                        artifact_paths(projected, None, layout),
                    );
                }
                Kind::Text(text) if track.text_profile == MidiTextProfile::KaraokeLyrics => {
                    if let Some(lyric) =
                        karaoke_text_lyric(&track.id, event.tick, event.order, text)
                    {
                        let lyric_id = standalone_lyric_instance_id(&lyric, &track.id, event.order);
                        let projected = projection.source_ids.contains(&lyric_id);
                        push_entry(
                            &mut entries,
                            lyric_id,
                            SourceItemKind::Lyric,
                            if projected {
                                PrimaryDisposition::ProjectedExact
                            } else {
                                PrimaryDisposition::SourceOnly {
                                    reason:
                                        "the karaoke text token was not projected to a vocal note"
                                            .into(),
                                }
                            },
                            artifact_paths(projected, None, layout),
                        );
                    }
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

fn artifact_paths(project: bool, audio_path: Option<&str>, layout: &BundleLayout) -> Vec<String> {
    let mut paths = vec![layout.source_relative_path.clone()];
    if project {
        paths.push(layout.project_relative_path.clone());
    }
    if let Some(audio_path) = audio_path {
        paths.push(audio_path.to_string());
    }
    paths
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleResult {
    pub bundle_path: PathBuf,
    pub project_path: PathBuf,
    /// Full-score reference mix retained for compatibility and auditing.
    pub audio_path: PathBuf,
    pub audio_paths: Vec<PathBuf>,
    pub stem_count: usize,
    pub source_path: PathBuf,
    pub manifest_path: PathBuf,
    pub renderer: RendererIdentity,
    pub audio_duration_seconds: f64,
    pub audio_sample_rate: u32,
    pub audio_channels: u16,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BundleProgressEvent {
    pub phase: BundleProgressPhase,
    pub completed: usize,
    pub total: usize,
    pub message: String,
    pub stem_id: Option<String>,
    pub stem_name: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BundleProgressPhase {
    Preparing,
    ExtractingParts,
    RenderingReference,
    RenderingStem,
    Finalizing,
    Finished,
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
    #[error("source stem plan is invalid: {0}")]
    InvalidStemPlan(String),
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
                RenderError::UnsupportedVersion { .. }
                | RenderError::ProbeRejected { .. }
                | RenderError::UnsupportedCapabilities { .. }
                | RenderError::IncompatibleScore { .. },
            ) => "RENDERER_UNSUPPORTED",
            Self::Render(RenderError::Timeout { .. }) => "RENDERER_TIMEOUT",
            Self::Render(_) => "RENDERER_FAILED",
            Self::Io { .. } => "BUNDLE_IO_FAILED",
            Self::Serialize(_) => "BUNDLE_SERIALIZE_FAILED",
            Self::InvalidLedger(_) => "PRESERVATION_INCOMPLETE",
            Self::InvalidStemPlan(_) => "STEM_PLAN_INVALID",
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

pub fn export_bundle_with_progress(
    request: BundleRequest,
    progress: &(dyn Fn(BundleProgressEvent) + Sync),
) -> Result<BundleResult, BundleError> {
    export_bundle_with_hook_and_progress(request, &NoopHook, progress)
}

fn export_bundle_with_hook(
    request: BundleRequest,
    hook: &dyn BundleHook,
) -> Result<BundleResult, BundleError> {
    export_bundle_with_hook_and_progress(request, hook, &|_| {})
}

fn export_bundle_with_hook_and_progress(
    mut request: BundleRequest,
    hook: &dyn BundleHook,
    progress: &(dyn Fn(BundleProgressEvent) + Sync),
) -> Result<BundleResult, BundleError> {
    validate_destination(&request.destination)?;
    let layout = BundleLayout::new(&request.destination, &request.input.original_name)?;
    request
        .input
        .stem_plan
        .validate()
        .map_err(|error| BundleError::InvalidStemPlan(error.to_string()))?;
    if !request.renderer.capabilities().score_parts {
        return Err(RenderError::UnsupportedCapabilities {
            missing: vec!["score-parts".into()],
        }
        .into());
    }
    let progress_total = request
        .input
        .stem_plan
        .stems
        .len()
        .checked_add(4)
        .ok_or_else(|| BundleError::Integrity("bundle progress size overflow".into()))?;
    progress(BundleProgressEvent {
        phase: BundleProgressPhase::Preparing,
        completed: 0,
        total: progress_total,
        message: "Preparing source and conversion plan".into(),
        stem_id: None,
        stem_name: None,
    });

    let stem_relative_paths = request
        .input
        .stem_plan
        .stems
        .iter()
        .map(|stem| layout.stem_audio_relative_path(stem))
        .collect::<Vec<_>>();
    let mut allowed_artifacts = BTreeSet::from([
        layout.source_relative_path.clone(),
        layout.project_relative_path.clone(),
        layout.audio_relative_path.clone(),
    ]);
    allowed_artifacts.extend(stem_relative_paths.iter().cloned());
    request.input.ledger.validate(&allowed_artifacts)?;

    let parent = request
        .destination
        .parent()
        .ok_or(BundleError::InvalidDestination)?;
    let mut staging = StagingGuard::create(parent, &request.destination)?;
    let root = staging.path().to_path_buf();
    for directory in ["source", "project", "audio", STEM_AUDIO_DIRECTORY] {
        create_directory(&root.join(directory), "create staging directories")?;
    }
    // Renderer intermediates must stay on the platform-local temporary
    // filesystem. A bundle destination may be a network or Parallels shared
    // folder where recursively deleting a work directory can fail even after
    // every renderer process has exited.
    let mut render_work = RenderWorkGuard::create()?;
    create_directory(
        &render_work.path().join("parts"),
        "create private renderer parts directory",
    )?;

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

    let render_started = Instant::now();
    progress(BundleProgressEvent {
        phase: BundleProgressPhase::ExtractingParts,
        completed: 1,
        total: progress_total,
        message: "Extracting source Parts with MuseScore".into(),
        stem_id: None,
        stem_name: None,
    });
    let extracted_parts = if is_midi_source(&request.input.source_format) {
        midi_source_parts(&request.input.source_bytes, &request.input.stem_plan.stems)?
    } else {
        let extracted = request.renderer.extract_score_parts(
            &source_path,
            &remaining_render_limits(render_started, &request.render_limits)?,
        )?;
        align_extracted_parts(
            &request.input.source_format,
            &request.input.stem_plan.stems,
            extracted,
        )?
    };

    progress(BundleProgressEvent {
        phase: BundleProgressPhase::RenderingReference,
        completed: 2,
        total: progress_total,
        message: "Rendering the full-score reference mix".into(),
        stem_id: None,
        stem_name: None,
    });
    let render_output = render_work.path().join("full-score.wav");
    let rendered = render_owned(
        request.renderer.as_ref(),
        &source_path,
        &render_output,
        &remaining_render_limits(render_started, &request.render_limits)?,
    )?;
    let audio_path = safe_join(&root, &layout.audio_relative_path)?;
    copy_new_file(
        &render_output,
        &audio_path,
        "publish rendered audio into staging",
    )?;
    let reference_wav = validate_wav(&audio_path, request.render_limits.max_output_bytes)?;
    if reference_wav.sha256 != rendered.wav.sha256 {
        return Err(BundleError::Integrity(
            "rendered WAV changed after validation".into(),
        ));
    }
    let mut total_audio_bytes = reference_wav.bytes;
    let mut rendered_stems = Vec::with_capacity(extracted_parts.len());
    for (stem_index, ((stem, relative_path), part)) in request
        .input
        .stem_plan
        .stems
        .iter()
        .zip(&stem_relative_paths)
        .zip(extracted_parts)
        .enumerate()
    {
        progress(BundleProgressEvent {
            phase: BundleProgressPhase::RenderingStem,
            completed: stem_index + 3,
            total: progress_total,
            message: format!(
                "Rendering Part {} of {}: {}",
                stem_index + 1,
                request.input.stem_plan.stems.len(),
                stem.display_name
            ),
            stem_id: Some(stem.stem_id.clone()),
            stem_name: Some(stem.display_name.clone()),
        });
        let part_input = render_work.path().join("parts").join(format!(
            "{}.{}",
            stem.stem_id,
            part_container_extension(&request.input.source_format)
        ));
        write_new(&part_input, &part.mscz, "write extracted source Part")?;
        let part_output = render_work
            .path()
            .join("parts")
            .join(format!("{}.wav", stem.stem_id));
        let rendered_part = render_part_owned(
            request.renderer.as_ref(),
            &part_input,
            &part_output,
            &remaining_render_limits(render_started, &request.render_limits)?,
        )?;
        if rendered_part.renderer != rendered.renderer {
            return Err(BundleError::Integrity(
                "renderer identity changed during stem export".into(),
            ));
        }
        total_audio_bytes = total_audio_bytes
            .checked_add(rendered_part.wav.bytes)
            .ok_or_else(|| BundleError::Integrity("aggregate audio size overflow".into()))?;
        if total_audio_bytes > MAX_TOTAL_AUDIO_BYTES {
            return Err(BundleError::Integrity(format!(
                "aggregate rendered audio exceeds {MAX_TOTAL_AUDIO_BYTES} bytes"
            )));
        }
        let published_path = safe_join(&root, relative_path)?;
        copy_new_file(
            &part_output,
            &published_path,
            "publish rendered stem into staging",
        )?;
        let wav =
            validate_wav_allowing_silence(&published_path, request.render_limits.max_output_bytes)?;
        if wav.sha256 != rendered_part.wav.sha256 {
            return Err(BundleError::Integrity(format!(
                "stem {} changed after validation",
                stem.stem_id
            )));
        }
        ensure_same_timeline(&reference_wav, &wav, &stem.stem_id)?;
        rendered_stems.push(RenderedStem {
            descriptor: stem.clone(),
            relative_path: relative_path.clone(),
            extracted_name: part.name,
            wav,
        });
    }
    render_work.cleanup()?;
    hook.checkpoint(FaultPoint::AfterAudio)?;

    progress(BundleProgressEvent {
        phase: BundleProgressPhase::Finalizing,
        completed: progress_total - 1,
        total: progress_total,
        message: "Writing and verifying the preservation bundle".into(),
        stem_id: None,
        stem_name: None,
    });
    let mut stem_audio_records = Vec::with_capacity(rendered_stems.len());
    // A score stem is the Part MuseScore extracted; a MIDI stem is the source
    // track Verse divided out itself. Naming the second one after MuseScore
    // would credit a decomposition it never made.
    let stem_origin = if is_midi_source(&request.input.source_format) {
        "MIDI track"
    } else {
        "MuseScore Part"
    };
    for stem in &rendered_stems {
        let group_id = append_instrumental_track(
            &mut request.input.project,
            format!("{} ({stem_origin})", stem.descriptor.display_name),
            format!("../{}", stem.relative_path),
            stem.wav.duration_seconds,
            0,
            !stem.descriptor.active_by_default,
        );
        stem_audio_records.push(StemAudioRecord {
            stem_id: stem.descriptor.stem_id.clone(),
            display_name: stem.descriptor.display_name.clone(),
            source_part_id: stem.descriptor.source_part_id.clone(),
            source_track_ids: stem.descriptor.source_track_ids.clone(),
            role: stem.descriptor.role,
            isolation_method: "musescore-score-parts".into(),
            active_by_default: stem.descriptor.active_by_default,
            asset: AudioArtifactRecord {
                artifact: artifact_record(&root, &stem.relative_path)?,
                duration_seconds: stem.wav.duration_seconds,
                sample_rate: stem.wav.sample_rate,
                channels: stem.wav.channels,
                bits_per_sample: stem.wav.bits_per_sample,
                frames: stem.wav.frames,
            },
            svp_group_id: group_id,
        });
    }
    let reference_group_id = append_instrumental_track(
        &mut request.input.project,
        "Full score reference mix (MuseScore)".into(),
        PROJECT_AUDIO_REFERENCE.into(),
        reference_wav.duration_seconds,
        0,
        true,
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
    let reference_audio_record = AudioArtifactRecord {
        artifact: artifact_record(&root, &layout.audio_relative_path)?,
        duration_seconds: reference_wav.duration_seconds,
        sample_rate: reference_wav.sample_rate,
        channels: reference_wav.channels,
        bits_per_sample: reference_wav.bits_per_sample,
        frames: reference_wav.frames,
    };
    let expected_stem_ids = request.input.stem_plan.expected_stem_ids();
    let rendered_stem_ids = stem_audio_records
        .iter()
        .map(|stem| stem.stem_id.clone())
        .collect::<Vec<_>>();
    let mut warnings = request.input.warnings;
    warnings.push(
        "The full-score reference mix is retained muted; source Parts are rendered as separate audio-backed SVP tracks.".into(),
    );
    for stem in &rendered_stems {
        if normalize_part_name(&stem.extracted_name)
            != normalize_part_name(&stem.descriptor.display_name)
        {
            warnings.push(format!(
                "[PART_NAME_DIFFERENCE] Source Part '{}' was returned by MuseScore as '{}'; verified source ordinal was preserved.",
                stem.descriptor.display_name, stem.extracted_name
            ));
        }
    }
    warnings.sort();
    warnings.dedup();
    let manifest = BundleManifest {
        schema_version: SCHEMA_VERSION,
        verse_version: env!("CARGO_PKG_VERSION").into(),
        source_format: request.input.source_format,
        source: source_record,
        project: project_record,
        audio: BundleAudioRecord {
            reference_mix: ReferenceMixRecord {
                asset: reference_audio_record,
                svp_group_id: reference_group_id,
                muted_by_default: true,
            },
            stems: stem_audio_records,
            coverage: AudioCoverageRecord {
                complete: expected_stem_ids == rendered_stem_ids,
                expected_stem_ids,
                rendered_stem_ids,
            },
        },
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

    let result = BundleResult {
        bundle_path: request.destination.clone(),
        project_path: request
            .destination
            .join(path_from_manifest(&layout.project_relative_path)),
        audio_path: request
            .destination
            .join(path_from_manifest(&layout.audio_relative_path)),
        audio_paths: stem_relative_paths
            .iter()
            .map(|path| request.destination.join(path_from_manifest(path)))
            .collect(),
        stem_count: stem_relative_paths.len(),
        source_path: request
            .destination
            .join(path_from_manifest(&layout.source_relative_path)),
        manifest_path: request
            .destination
            .join(path_from_manifest(&layout.manifest_relative_path)),
        renderer: rendered.renderer,
        audio_duration_seconds: reference_wav.duration_seconds,
        audio_sample_rate: reference_wav.sample_rate,
        audio_channels: reference_wav.channels,
        warnings: manifest.warnings,
    };
    progress(BundleProgressEvent {
        phase: BundleProgressPhase::Finished,
        completed: progress_total,
        total: progress_total,
        message: "Complete project ready".into(),
        stem_id: None,
        stem_name: None,
    });
    Ok(result)
}

/// Both files start at zero and share a sample rate, so a stem stays in step
/// with the reference for every frame it has. A stem that stops earlier is a
/// Part that falls silent before the end — a MIDI track that finishes its last
/// phrase early renders exactly that way — and padding it would add audio the
/// source never carried. A stem that runs *longer* than the whole score is not
/// explainable and is still refused.
fn ensure_same_timeline(
    reference: &WavInfo,
    stem: &WavInfo,
    stem_id: &str,
) -> Result<(), BundleError> {
    if stem.sample_rate != reference.sample_rate || stem.frames > reference.frames {
        return Err(BundleError::Integrity(format!(
            "stem {stem_id} is not aligned with the full-score reference \
             (reference: {} Hz / {} frames; stem: {} Hz / {} frames)",
            reference.sample_rate, reference.frames, stem.sample_rate, stem.frames
        )));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct RenderedStem {
    descriptor: StemDescriptor,
    relative_path: String,
    extracted_name: String,
    wav: WavInfo,
}

/// True when the source is a MIDI file, whose stems Verse divides itself.
fn is_midi_source(source_format: &str) -> bool {
    matches!(source_format, "standardMidi" | "karaokeMidi")
}

/// Filename extension a Part container needs so the renderer imports it as the
/// format it actually is.
fn part_container_extension(source_format: &str) -> &'static str {
    if is_midi_source(source_format) {
        "mid"
    } else {
        "mscz"
    }
}

/// One renderable Part per stem, taken from the source itself.
///
/// MuseScore decides on its own how an imported MIDI becomes Parts — merging
/// tracks that share an instrument, dropping empty ones — so its Part list
/// answers a different question than "which source track is this". The counts
/// disagreed and every MIDI bundle failed. A MIDI, unlike a score, divides
/// exactly along its own `MTrk` chunks, so Verse cuts it here and knows which
/// track each stem carries because it chose it.
fn midi_source_parts(
    source_bytes: &[u8],
    stems: &[StemDescriptor],
) -> Result<Vec<ExtractedScorePart>, BundleError> {
    let slices = crate::engine::midi_split::split_tracks(source_bytes).map_err(|error| {
        BundleError::Integrity(format!("source MIDI cannot be divided: {error}"))
    })?;
    stems
        .iter()
        .enumerate()
        .map(|(ordinal, stem)| {
            let source_track =
                SourceTopology::midi_part_track(&stem.source_part_id).ok_or_else(|| {
                    BundleError::Integrity(format!(
                        "stem {} does not name a source MIDI track",
                        stem.stem_id
                    ))
                })?;
            let slice = slices
                .iter()
                .find(|slice| slice.source_track == source_track)
                .ok_or_else(|| {
                    BundleError::Integrity(format!(
                        "source MIDI has no track {source_track} for stem {}",
                        stem.stem_id
                    ))
                })?;
            Ok(ExtractedScorePart {
                ordinal,
                name: stem.display_name.clone(),
                metadata: serde_json::Value::Null,
                mscz: slice.bytes.clone(),
            })
        })
        .collect()
}

fn align_extracted_parts(
    _source_format: &str,
    stems: &[StemDescriptor],
    parts: Vec<ExtractedScorePart>,
) -> Result<Vec<ExtractedScorePart>, BundleError> {
    if parts.len() != stems.len() {
        return Err(BundleError::Integrity(format!(
            "MuseScore extracted {} Parts but the source topology requires {}",
            parts.len(),
            stems.len()
        )));
    }
    let mut available = vec![None; parts.len()];
    for part in parts {
        let ordinal = part.ordinal;
        if ordinal >= available.len() || available[ordinal].replace(part).is_some() {
            return Err(BundleError::Integrity(
                "MuseScore returned duplicated or out-of-range Part ordinals".into(),
            ));
        }
    }

    available
        .into_iter()
        .enumerate()
        .map(|(ordinal, part)| {
            part.ok_or_else(|| {
                BundleError::Integrity(format!(
                    "MuseScore returned no Part at source ordinal {ordinal}"
                ))
            })
        })
        .collect()
}

fn remaining_render_limits(
    started: Instant,
    limits: &RenderLimits,
) -> Result<RenderLimits, BundleError> {
    let remaining = limits
        .timeout
        .checked_sub(started.elapsed())
        .filter(|duration| *duration > Duration::ZERO)
        .ok_or_else(|| RenderError::Timeout {
            milliseconds: limits.timeout.as_millis().min(u64::MAX as u128) as u64,
        })?;
    Ok(RenderLimits {
        timeout: remaining,
        max_output_bytes: limits.max_output_bytes,
    })
}

fn render_owned(
    renderer: &dyn AudioRenderer,
    input: &Path,
    expected_output: &Path,
    limits: &RenderLimits,
) -> Result<crate::renderer::RenderedAudio, BundleError> {
    let rendered = renderer.render(input, expected_output, limits)?;
    validate_owned_render(rendered, expected_output)
}

fn render_part_owned(
    renderer: &dyn AudioRenderer,
    input: &Path,
    expected_output: &Path,
    limits: &RenderLimits,
) -> Result<crate::renderer::RenderedAudio, BundleError> {
    let rendered = renderer.render_part(input, expected_output, limits)?;
    validate_owned_render(rendered, expected_output)
}

fn validate_owned_render(
    rendered: crate::renderer::RenderedAudio,
    expected_output: &Path,
) -> Result<crate::renderer::RenderedAudio, BundleError> {
    if rendered.path != expected_output {
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
        })? != fs::canonicalize(expected_output).map_err(|source| BundleError::Io {
            phase: "resolve expected renderer output path",
            source,
        })?
    {
        return Err(BundleError::Integrity(
            "renderer returned an output outside the owned render path".into(),
        ));
    }
    Ok(rendered)
}

fn normalize_part_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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

fn sanitize_stem_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut separator_pending = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if separator_pending && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character.to_ascii_lowercase());
            separator_pending = false;
        } else {
            separator_pending = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "part".into()
    } else {
        slug.into()
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

fn copy_new_file(
    source_path: &Path,
    destination_path: &Path,
    phase: &'static str,
) -> Result<(), BundleError> {
    let metadata =
        fs::symlink_metadata(source_path).map_err(|source| BundleError::Io { phase, source })?;
    if !metadata.file_type().is_file() {
        return Err(BundleError::Integrity(format!(
            "renderer output is not a regular file: {}",
            source_path.display()
        )));
    }
    let mut source_file =
        fs::File::open(source_path).map_err(|source| BundleError::Io { phase, source })?;
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination_path)
        .map_err(|source| BundleError::Io { phase, source })?;
    let copied = io::copy(&mut source_file, &mut destination_file)
        .map_err(|source| BundleError::Io { phase, source })?;
    destination_file
        .sync_all()
        .map_err(|source| BundleError::Io { phase, source })?;
    if copied != metadata.len() {
        return Err(BundleError::Integrity(format!(
            "renderer output copy length changed: expected {}, copied {copied}",
            metadata.len()
        )));
    }
    Ok(())
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
    verify_audio_artifact(root, &manifest.audio.reference_mix.asset, false)?;
    if !manifest.audio.reference_mix.muted_by_default {
        return Err(BundleError::Integrity(
            "the full-score reference mix must be muted by default".into(),
        ));
    }

    let expected_ids = manifest
        .audio
        .coverage
        .expected_stem_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let rendered_ids = manifest
        .audio
        .coverage
        .rendered_stem_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let record_ids = manifest
        .audio
        .stems
        .iter()
        .map(|stem| stem.stem_id.clone())
        .collect::<BTreeSet<_>>();
    if !manifest.audio.coverage.complete
        || expected_ids.len() != manifest.audio.coverage.expected_stem_ids.len()
        || rendered_ids.len() != manifest.audio.coverage.rendered_stem_ids.len()
        || record_ids.len() != manifest.audio.stems.len()
        || expected_ids != rendered_ids
        || expected_ids != record_ids
        || expected_ids.is_empty()
    {
        return Err(BundleError::Integrity(
            "stem coverage is incomplete, duplicated, or inconsistent".into(),
        ));
    }
    let mut audio_paths = BTreeSet::new();
    audio_paths.insert(manifest.audio.reference_mix.asset.artifact.path.clone());
    for stem in &manifest.audio.stems {
        if stem.source_track_ids.is_empty()
            || stem.isolation_method != "musescore-score-parts"
            || !stem
                .asset
                .artifact
                .path
                .starts_with(&format!("{STEM_AUDIO_DIRECTORY}/"))
            || !audio_paths.insert(stem.asset.artifact.path.clone())
        {
            return Err(BundleError::Integrity(format!(
                "stem {} has invalid ownership or artifact metadata",
                stem.stem_id
            )));
        }
        verify_audio_artifact(root, &stem.asset, true)?;
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
    let canonical_root = fs::canonicalize(root).map_err(|source| BundleError::Io {
        phase: "resolve bundle root",
        source,
    })?;
    verify_svp_audio_track(
        root,
        &canonical_root,
        &project_path,
        tracks,
        PROJECT_AUDIO_REFERENCE,
        &manifest.audio.reference_mix.asset.artifact.path,
        &manifest.audio.reference_mix.svp_group_id,
        true,
    )?;
    for stem in &manifest.audio.stems {
        verify_svp_audio_track(
            root,
            &canonical_root,
            &project_path,
            tracks,
            &format!("../{}", stem.asset.artifact.path),
            &stem.asset.artifact.path,
            &stem.svp_group_id,
            !stem.active_by_default,
        )?;
    }

    let ledger_path = safe_join(root, &manifest.preservation.path)?;
    let ledger: PreservationLedger =
        serde_json::from_slice(&fs::read(ledger_path).map_err(|source| BundleError::Io {
            phase: "reopen preservation ledger",
            source,
        })?)?;
    let mut allowed = BTreeSet::from([manifest.source.path, manifest.project.path]);
    allowed.extend(audio_paths);
    ledger.validate(&allowed)
}

fn verify_audio_artifact(
    root: &Path,
    record: &AudioArtifactRecord,
    allow_silence: bool,
) -> Result<(), BundleError> {
    verify_artifact(root, &record.artifact)?;
    let audio_path = safe_join(root, &record.artifact.path)?;
    let wav = if allow_silence {
        validate_wav_allowing_silence(&audio_path, record.artifact.bytes)?
    } else {
        validate_wav(&audio_path, record.artifact.bytes)?
    };
    if wav.sha256 != record.artifact.sha256
        || wav.sample_rate != record.sample_rate
        || wav.channels != record.channels
        || wav.bits_per_sample != record.bits_per_sample
        || wav.frames != record.frames
        || (wav.duration_seconds - record.duration_seconds).abs() > 0.000_001
    {
        return Err(BundleError::Integrity(format!(
            "{} WAV metadata differs from manifest",
            record.artifact.path
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_svp_audio_track(
    root: &Path,
    canonical_root: &Path,
    project_path: &Path,
    tracks: &[serde_json::Value],
    project_reference: &str,
    artifact_path: &str,
    group_id: &str,
    expected_muted: bool,
) -> Result<(), BundleError> {
    let matches = tracks
        .iter()
        .filter(|track| {
            track["mainRef"]["isInstrumental"] == serde_json::Value::Bool(true)
                && track["mainRef"]["audio"]["filename"] == project_reference
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(BundleError::Integrity(format!(
            "SVP must contain exactly one audio track for {artifact_path}, found {}",
            matches.len()
        )));
    }
    let track = matches[0];
    if track["mainRef"]["blickOffset"] != 0
        || track["mainRef"]["groupID"] != group_id
        || track["mainGroup"]["uuid"] != group_id
        || track["mainGroup"]["notes"] != serde_json::json!([])
        || track["mixer"]["mute"] != serde_json::Value::Bool(expected_muted)
    {
        return Err(BundleError::Integrity(format!(
            "SVP audio track for {artifact_path} has an invalid schema or mute state"
        )));
    }
    let referenced = project_path
        .parent()
        .ok_or_else(|| BundleError::Integrity("SVP project has no parent".into()))?
        .join(project_reference);
    let referenced = fs::canonicalize(referenced).map_err(|source| BundleError::Io {
        phase: "resolve SVP audio reference",
        source,
    })?;
    let canonical_audio =
        fs::canonicalize(safe_join(root, artifact_path)?).map_err(|source| BundleError::Io {
            phase: "resolve validated audio asset",
            source,
        })?;
    if !referenced.starts_with(canonical_root) || referenced != canonical_audio {
        return Err(BundleError::Integrity(format!(
            "SVP audio reference for {artifact_path} escapes or mismatches the bundle"
        )));
    }
    Ok(())
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
static RENDER_WORK_COUNTER: AtomicU64 = AtomicU64::new(0);

struct RenderWorkGuard {
    path: PathBuf,
    cleaned: bool,
}

impl RenderWorkGuard {
    fn create() -> Result<Self, BundleError> {
        for _ in 0..100 {
            let counter = RENDER_WORK_COUNTER.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "verse-bundle-render-{}-{timestamp}-{counter}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    let guard = Self {
                        path,
                        cleaned: false,
                    };
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Err(source) =
                            fs::set_permissions(&guard.path, fs::Permissions::from_mode(0o700))
                        {
                            let _ = fs::remove_dir_all(&guard.path);
                            return Err(BundleError::Io {
                                phase: "secure private renderer work directory",
                                source,
                            });
                        }
                    }
                    return Ok(guard);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(BundleError::Io {
                        phase: "create private renderer work directory",
                        source,
                    })
                }
            }
        }
        Err(BundleError::Commit(
            "cannot allocate a private renderer work directory".into(),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(&mut self) -> Result<(), BundleError> {
        const ATTEMPTS: usize = 40;
        for attempt in 0..ATTEMPTS {
            match fs::remove_dir_all(&self.path) {
                Ok(()) => {
                    self.cleaned = true;
                    return Ok(());
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.cleaned = true;
                    return Ok(());
                }
                Err(source) if attempt + 1 == ATTEMPTS => {
                    return Err(BundleError::Io {
                        phase: "remove private renderer work directory",
                        source,
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        unreachable!("bounded cleanup loop always returns")
    }
}

impl Drop for RenderWorkGuard {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

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
    use crate::renderer::{MuseScoreRenderer, RendererCapabilities, WavInfo};
    use crate::stems::{StemDescriptor, StemPlan, StemRole};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Mutex;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Copy)]
    enum FakeMode {
        Success,
        SilentStem,
        MisalignedStem,
        Missing,
        Corrupt,
        Timeout,
    }

    struct FakeRenderer {
        mode: FakeMode,
        parts: Vec<ExtractedScorePart>,
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

        fn extract_score_parts(
            &self,
            _input: &Path,
            _limits: &RenderLimits,
        ) -> Result<Vec<ExtractedScorePart>, RenderError> {
            Ok(vec![fake_part(0)])
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
            Self::with_parts(mode, 1)
        }

        fn with_parts(mode: FakeMode, part_count: usize) -> Self {
            Self::with_extracted_parts(mode, (0..part_count).map(fake_part).collect())
        }

        fn with_stems(mode: FakeMode, stems: &[StemDescriptor]) -> Self {
            Self::with_extracted_parts(
                mode,
                stems
                    .iter()
                    .enumerate()
                    .map(|(ordinal, stem)| ExtractedScorePart {
                        ordinal,
                        name: stem.display_name.clone(),
                        metadata: serde_json::json!({
                            "id": stem.source_part_id
                                .strip_prefix("musescore-part-")
                                .unwrap_or(&stem.source_part_id)
                        }),
                        mscz: format!("fake MSCZ for {}", stem.source_part_id).into_bytes(),
                    })
                    .collect(),
            )
        }

        fn with_extracted_parts(mode: FakeMode, parts: Vec<ExtractedScorePart>) -> Self {
            Self {
                mode,
                parts,
                capabilities: RendererCapabilities {
                    identity: RendererIdentity {
                        provider: "fake-musescore".into(),
                        version: "MuseScore 4.99-test".into(),
                        major: 4,
                        executable_sha256: "00".repeat(32),
                        full_score_mix: true,
                        capabilities: vec![
                            "full-score-wav".into(),
                            "score-parts".into(),
                            "part-wav".into(),
                        ],
                    },
                    supported_extensions: vec!["mid", "mscz", "mxl"],
                    output_format: "wav",
                    score_parts: true,
                },
            }
        }
    }

    impl AudioRenderer for FakeRenderer {
        fn capabilities(&self) -> &RendererCapabilities {
            &self.capabilities
        }

        fn extract_score_parts(
            &self,
            _input: &Path,
            limits: &RenderLimits,
        ) -> Result<Vec<ExtractedScorePart>, RenderError> {
            match self.mode {
                FakeMode::Missing => return Err(RenderError::MissingOutput),
                FakeMode::Timeout => {
                    return Err(RenderError::Timeout {
                        milliseconds: limits.timeout.as_millis() as u64,
                    })
                }
                FakeMode::Corrupt => {
                    return Err(RenderError::InvalidScoreParts {
                        reason: "injected corrupt response".into(),
                    })
                }
                FakeMode::Success | FakeMode::SilentStem | FakeMode::MisalignedStem => {}
            }
            Ok(self.parts.clone())
        }

        fn render(
            &self,
            input: &Path,
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
                FakeMode::Success | FakeMode::SilentStem => write_test_wav(output),
                FakeMode::MisalignedStem => {
                    // Stems are staged under `parts/`; the reference mix is not.
                    // A stem running past the end of the whole score cannot be
                    // explained by a Part falling silent early.
                    if input.parent().and_then(Path::file_name) == Some("parts".as_ref()) {
                        write_test_wav_with_frames(output, 884);
                    } else {
                        write_test_wav(output);
                    }
                }
            }
            let wav = validate_wav(output, limits.max_output_bytes)?;
            Ok(crate::renderer::RenderedAudio {
                path: output.into(),
                wav,
                renderer: self.capabilities.identity.clone(),
            })
        }

        fn render_part(
            &self,
            input: &Path,
            output: &Path,
            limits: &RenderLimits,
        ) -> Result<crate::renderer::RenderedAudio, RenderError> {
            if matches!(self.mode, FakeMode::SilentStem) {
                write_silent_test_wav(output);
                let wav = validate_wav_allowing_silence(output, limits.max_output_bytes)?;
                return Ok(crate::renderer::RenderedAudio {
                    path: output.into(),
                    wav,
                    renderer: self.capabilities.identity.clone(),
                });
            }
            self.render(input, output, limits)
        }
    }

    fn fake_part(ordinal: usize) -> ExtractedScorePart {
        let (name, id) = match ordinal {
            0 => ("Music".to_string(), "part:midi-track-0".to_string()),
            1 => ("Piano".to_string(), "part:piano".to_string()),
            _ => (
                format!("Part {}", ordinal + 1),
                format!("part:fake-{ordinal}"),
            ),
        };
        ExtractedScorePart {
            ordinal,
            name,
            metadata: serde_json::json!({"test": true, "id": id}),
            mscz: b"fake MSCZ for fake renderer".to_vec(),
        }
    }

    fn write_test_wav(path: &Path) {
        write_test_wav_with_frames(path, 882);
    }

    fn write_silent_test_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..882 {
            writer.write_sample::<i16>(0).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn write_test_wav_with_frames(path: &Path, interleaved_samples: usize) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for sample in 0..interleaved_samples {
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
        let stem_plan = StemPlan {
            stems: vec![StemDescriptor {
                stem_id: "part-001-test".into(),
                source_part_id: "midi:track:0".into(),
                display_name: "Music".into(),
                source_track_ids: vec!["midi-track-0".into()],
                source_note_count: 1,
                role: StemRole::Accompaniment,
                active_by_default: true,
            }],
        };
        let entry = DispositionEntry {
            source_id: "track:midi-track-0".into(),
            item_kind: SourceItemKind::Track,
            disposition: PrimaryDisposition::RenderedStem {
                stem_id: stem_plan.stems[0].stem_id.clone(),
            },
            artifact_paths: vec![
                layout.source_relative_path.clone(),
                layout.stem_audio_relative_path(&stem_plan.stems[0]),
            ],
        };
        BundleRequest {
            destination,
            input: BundleInput {
                original_name: "source.mid".into(),
                source_format: "standardMidi".into(),
                source_bytes: one_track_midi(),
                project: empty_project(),
                stem_plan,
                ledger: PreservationLedger {
                    schema_version: SCHEMA_VERSION,
                    expected_source_ids: vec![entry.source_id.clone()],
                    entries: vec![entry],
                },
                warnings: vec!["[TEST_WARNING] retained diagnostic".into()],
            },
            renderer: Arc::new(FakeRenderer::new(mode)),
            render_limits: RenderLimits {
                timeout: std::time::Duration::from_secs(1),
                max_output_bytes: 1024 * 1024,
            },
        }
    }

    fn test_wav_info() -> WavInfo {
        WavInfo {
            bytes: 0,
            sha256: String::new(),
            duration_seconds: 0.0,
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            frames: 0,
        }
    }

    /// A real one-track Standard MIDI File. The MIDI stem path divides the
    /// source itself, so a bundle test must hand it something divisible.
    fn one_track_midi() -> Vec<u8> {
        smf(&[
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // tempo
            0x00, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0, 0x00, 0xff, 0x2f, 0x00,
        ])
    }

    /// A real two-track Standard MIDI File, for the two-stem cases.
    fn two_track_midi() -> Vec<u8> {
        let mut data = b"MThd\0\0\0\x06\0\x01\0\x02\x01\xe0".to_vec();
        for track in [
            &[
                0x00u8, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, 0x00, 0x90, 60, 100, 0x83, 0x60, 0x80,
                60, 0, 0x00, 0xff, 0x2f, 0x00,
            ][..],
            &[
                0x00, 0x91, 67, 90, 0x83, 0x60, 0x81, 67, 0, 0x00, 0xff, 0x2f, 0x00,
            ][..],
        ] {
            data.extend_from_slice(b"MTrk");
            data.extend_from_slice(&(track.len() as u32).to_be_bytes());
            data.extend_from_slice(track);
        }
        data
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
        assert_eq!(fs::read(&result.source_path).unwrap(), one_track_midi());
        let project: serde_json::Value =
            serde_json::from_slice(&fs::read(&result.project_path).unwrap()).unwrap();
        assert_eq!(project["tracks"].as_array().unwrap().len(), 2);
        assert_eq!(project["tracks"][0]["mainRef"]["isInstrumental"], true);
        assert_eq!(project["tracks"][0]["mixer"]["mute"], false);
        assert_eq!(
            project["tracks"][1]["mainRef"]["audio"]["filename"],
            PROJECT_AUDIO_REFERENCE
        );
        assert_eq!(project["tracks"][1]["mixer"]["mute"], true);
        let manifest: BundleManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.source.sha256, sha256_bytes(&one_track_midi()));
        assert!(manifest.audio.reference_mix.asset.duration_seconds > 0.0);
        assert_eq!(manifest.audio.stems.len(), 1);
        assert!(manifest.audio.coverage.complete);
        assert_eq!(result.stem_count, 1);
        assert_eq!(result.audio_paths.len(), 1);
        assert!(manifest
            .warnings
            .iter()
            .any(|warning| warning.contains("[TEST_WARNING]")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_bundle_reports_bounded_monotonic_progress() {
        let root = temp_dir("progress");
        let events = Mutex::new(Vec::new());
        export_bundle_with_progress(request(&root, FakeMode::Success), &|event| {
            events.lock().unwrap().push(event);
        })
        .unwrap();
        let events = events.into_inner().unwrap();

        assert_eq!(
            events.iter().map(|event| event.phase).collect::<Vec<_>>(),
            [
                BundleProgressPhase::Preparing,
                BundleProgressPhase::ExtractingParts,
                BundleProgressPhase::RenderingReference,
                BundleProgressPhase::RenderingStem,
                BundleProgressPhase::Finalizing,
                BundleProgressPhase::Finished,
            ]
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event.completed)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        assert!(events.iter().all(|event| event.total == 5));
        assert_eq!(events[3].stem_name.as_deref(), Some("Music"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multiple_source_parts_become_distinct_audio_tracks_with_safe_mute_defaults() {
        let root = temp_dir("multiple-stems");
        let mut request = request(&root, FakeMode::Success);
        request.input.source_bytes = two_track_midi();
        request.input.stem_plan.stems[0].role = StemRole::VocalReference;
        request.input.stem_plan.stems[0].active_by_default = false;
        let second = StemDescriptor {
            stem_id: "part-002-test".into(),
            source_part_id: "midi:track:1".into(),
            display_name: "Piano".into(),
            source_track_ids: vec!["piano".into()],
            source_note_count: 4,
            role: StemRole::Accompaniment,
            active_by_default: true,
        };
        let second_path = BundleLayout::new(&request.destination, "source.mid")
            .unwrap()
            .stem_audio_relative_path(&second);
        request.input.stem_plan.stems.push(second.clone());
        let entry = DispositionEntry {
            source_id: "track:piano".into(),
            item_kind: SourceItemKind::Track,
            disposition: PrimaryDisposition::RenderedStem {
                stem_id: second.stem_id.clone(),
            },
            artifact_paths: vec!["source/source.mid".into(), second_path],
        };
        request
            .input
            .ledger
            .expected_source_ids
            .push(entry.source_id.clone());
        request.input.ledger.entries.push(entry);
        request.renderer = Arc::new(FakeRenderer::with_parts(FakeMode::Success, 2));

        let result = export_bundle(request).unwrap();
        let project: serde_json::Value =
            serde_json::from_slice(&fs::read(result.project_path).unwrap()).unwrap();
        let tracks = project["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0]["mixer"]["mute"], true);
        assert_eq!(tracks[1]["mixer"]["mute"], false);
        assert_eq!(tracks[2]["mixer"]["mute"], true);
        assert!(tracks.iter().all(|track| {
            track["mainRef"]["isInstrumental"] == true
                && track["mainGroup"]["notes"] == serde_json::json!([])
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_part_extraction_is_blocking_and_transactional() {
        let root = temp_dir("missing-part");
        let mut request = request(&root, FakeMode::Success);
        // Only a score has Parts extracted by MuseScore; a MIDI is divided by
        // Verse itself and cannot come back short.
        request.input.source_format = "museScore".into();
        request.renderer = Arc::new(FakeRenderer::with_parts(FakeMode::Success, 2));
        let destination = request.destination.clone();
        assert!(matches!(
            export_bundle(request),
            Err(BundleError::Integrity(message)) if message.contains("source topology")
        ));
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_scores_align_parts_by_verified_source_order() {
        let descriptors = vec![
            StemDescriptor {
                stem_id: "first".into(),
                source_part_id: "musescore-part-native-b".into(),
                display_name: "Same".into(),
                source_track_ids: vec!["b".into()],
                source_note_count: 1,
                role: StemRole::Accompaniment,
                active_by_default: true,
            },
            StemDescriptor {
                stem_id: "second".into(),
                source_part_id: "musescore-part-native-a".into(),
                display_name: "Same".into(),
                source_track_ids: vec!["a".into()],
                source_note_count: 1,
                role: StemRole::Accompaniment,
                active_by_default: true,
            },
        ];
        let parts = vec![
            ExtractedScorePart {
                ordinal: 0,
                name: "Same".into(),
                metadata: serde_json::json!({"id": "native-a"}),
                mscz: vec![1],
            },
            ExtractedScorePart {
                ordinal: 1,
                name: "Same".into(),
                metadata: serde_json::json!({"id": "native-b"}),
                mscz: vec![2],
            },
        ];
        let aligned = align_extracted_parts("museScore", &descriptors, parts).unwrap();
        assert_eq!(aligned[0].mscz, [1]);
        assert_eq!(aligned[1].mscz, [2]);
    }

    #[test]
    fn imported_scores_align_parts_by_source_order_when_musescore_rewrites_names_and_ids() {
        let descriptors = vec![
            StemDescriptor {
                stem_id: "banjo".into(),
                source_part_id: "midi:track:3".into(),
                display_name: "BANJO MELODY".into(),
                source_track_ids: vec!["midi:track:3".into()],
                source_note_count: 1,
                role: StemRole::Accompaniment,
                active_by_default: true,
            },
            StemDescriptor {
                stem_id: "strings".into(),
                source_part_id: "midi:track:5".into(),
                display_name: "STRINGS".into(),
                source_track_ids: vec!["midi:track:5".into()],
                source_note_count: 1,
                role: StemRole::Accompaniment,
                active_by_default: true,
            },
        ];
        let parts = vec![
            ExtractedScorePart {
                ordinal: 0,
                name: "Banjo, BANJO MELODY".into(),
                metadata: serde_json::json!({"id": "generated-banjo-id"}),
                mscz: vec![1],
            },
            ExtractedScorePart {
                ordinal: 1,
                name: "Violins, STRINGS".into(),
                metadata: serde_json::json!({"id": "generated-strings-id"}),
                mscz: vec![2],
            },
        ];

        let aligned = align_extracted_parts("karaokeMidi", &descriptors, parts).unwrap();
        assert_eq!(aligned[0].mscz, [1]);
        assert_eq!(aligned[1].mscz, [2]);
    }

    #[test]
    fn native_scores_tolerate_musescore_rewritten_part_identity() {
        let descriptors = vec![StemDescriptor {
            stem_id: "voice".into(),
            source_part_id: "musescore-part-voice".into(),
            display_name: "Voice".into(),
            source_track_ids: vec!["voice".into()],
            source_note_count: 1,
            role: StemRole::VocalReference,
            active_by_default: false,
        }];
        let parts = vec![ExtractedScorePart {
            ordinal: 0,
            name: "Piano".into(),
            metadata: serde_json::json!({"id": "piano"}),
            mscz: vec![1],
        }];

        let aligned = align_extracted_parts("museScore", &descriptors, parts).unwrap();
        assert_eq!(aligned[0].name, "Piano");
        assert_eq!(aligned[0].mscz, [1]);
    }

    #[test]
    fn a_stem_that_falls_silent_before_the_end_is_still_accepted() {
        // A MIDI track that finishes its last phrase early renders shorter than
        // the whole score. Both files start at zero, so it stays in step for
        // every frame it has; padding it would add audio the source never
        // carried, and refusing it blocked every such bundle.
        let reference = WavInfo {
            sample_rate: 44_100,
            frames: 882,
            ..test_wav_info()
        };
        let short = WavInfo {
            sample_rate: 44_100,
            frames: 441,
            ..test_wav_info()
        };
        assert!(ensure_same_timeline(&reference, &short, "part-001").is_ok());

        let long = WavInfo {
            sample_rate: 44_100,
            frames: 883,
            ..test_wav_info()
        };
        assert!(ensure_same_timeline(&reference, &long, "part-001").is_err());
        let resampled = WavInfo {
            sample_rate: 48_000,
            frames: 882,
            ..test_wav_info()
        };
        assert!(ensure_same_timeline(&reference, &resampled, "part-001").is_err());
    }

    #[test]
    fn misaligned_stem_timeline_is_blocking_and_transactional() {
        let root = temp_dir("misaligned-stem");
        let request = request(&root, FakeMode::MisalignedStem);
        let destination = request.destination.clone();

        assert!(matches!(
            export_bundle(request),
            Err(BundleError::Integrity(message)) if message.contains("not aligned")
        ));
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn silent_isolated_source_part_is_preserved_when_reference_mix_is_audible() {
        let root = temp_dir("silent-stem");
        let request = request(&root, FakeMode::SilentStem);

        let result = export_bundle(request).expect("a legitimate silent Part must be preserved");
        assert_eq!(result.stem_count, 1);
        assert!(validate_wav(&result.audio_path, 1024 * 1024).is_ok());
        assert!(validate_wav_allowing_silence(&result.audio_paths[0], 1024 * 1024).is_ok());
        assert!(validate_wav(&result.audio_paths[0], 1024 * 1024).is_err());
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
            false,
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
                    major: 4,
                    executable_sha256: "00".repeat(32),
                    full_score_mix: true,
                    capabilities: vec![
                        "full-score-wav".into(),
                        "score-parts".into(),
                        "part-wav".into(),
                    ],
                },
                supported_extensions: vec!["mid"],
                output_format: "wav",
                score_parts: true,
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
        let stem_plan = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
        let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);
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
    fn generic_text_and_midi_line_controls_are_not_inventoried_as_lyrics() {
        use crate::engine::midi::{
            Event, Lyric, Midi, SourceFormat, SourceTopology, TextEvent, TimeBase, Track,
        };

        let mut metadata = Track::new("metadata", 0);
        metadata.events = vec![
            Event::new(
                0,
                0,
                Kind::Text(TextEvent {
                    text: "hello".into(),
                    raw: b"hello".to_vec(),
                }),
            ),
            Event::new(0, 1, Kind::Lyrics(Lyric::text("line-break", "\r".into()))),
        ];
        let tracks = vec![metadata];
        let midi = Midi {
            ticks_per_beat: 480,
            time_base: TimeBase::PulsesPerQuarter(480),
            format: 1,
            source_format: SourceFormat::StandardMidi,
            topology: SourceTopology::from_tracks(&tracks),
            tracks,
        };
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        let root = temp_dir("metadata-ledger");
        let layout = BundleLayout::new(&root.join("Song.versebundle"), "source.mid").unwrap();
        let stem_plan = StemPlan { stems: Vec::new() };
        let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);

        assert_eq!(
            ledger
                .entries
                .iter()
                .filter(|entry| entry.item_kind == SourceItemKind::Lyric)
                .count(),
            0
        );
        assert!(ledger
            .entries
            .iter()
            .any(|entry| entry.source_id == "event:metadata:0"));
        assert!(ledger
            .entries
            .iter()
            .any(|entry| entry.source_id == "event:metadata:1"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn projected_soft_karaoke_text_is_ledgered_as_text_not_synthetic_la() {
        use crate::engine::midi::{
            Event, MidiTextProfile, NoteOff, NoteOn, NoteSource, SourceFormat, SourceTopology,
            TimeBase, Track,
        };

        let mut words = Track::new("words", 0);
        words.name = "Words".into();
        words.text_profile = MidiTextProfile::KaraokeLyrics;
        for (order, (tick, value)) in [(0, "@KMIDI KARAOKE FILE"), (0, "\\let"), (480, "/it")]
            .into_iter()
            .enumerate()
        {
            words.events.push(Event::new(
                tick,
                order as u32,
                Kind::Text(crate::engine::midi::TextEvent {
                    text: value.into(),
                    raw: value.as_bytes().to_vec(),
                }),
            ));
        }
        let mut melody = Track::new("melody", 1);
        melody.name = "Melody".into();
        for (index, (onset, pitch)) in [(0, 60), (480, 62)].into_iter().enumerate() {
            let source = NoteSource {
                id: format!("note-{index}"),
                ..NoteSource::default()
            };
            melody.events.push(Event::new(
                onset,
                (index * 2) as u32,
                Kind::NoteOn(NoteOn {
                    channel: Some(0),
                    key: Some(pitch),
                    velocity: Some(100),
                    source: source.clone(),
                    lyrics: vec![],
                }),
            ));
            melody.events.push(Event::new(
                onset + 240,
                (index * 2 + 1) as u32,
                Kind::NoteOff(NoteOff {
                    channel: Some(0),
                    key: Some(pitch),
                    velocity: Some(0),
                    source_id: Some(source.id),
                }),
            ));
        }
        let tracks = vec![words, melody];
        let midi = Midi {
            ticks_per_beat: 480,
            time_base: TimeBase::PulsesPerQuarter(480),
            format: 1,
            source_format: SourceFormat::KaraokeMidi,
            topology: SourceTopology::from_tracks(&tracks),
            tracks,
        };
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert_eq!(outcome.placed, 2);
        let root = temp_dir("kar-text-ledger");
        let layout = BundleLayout::new(&root.join("Song.versebundle"), "source.kar").unwrap();
        let stem_plan = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
        let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);
        let projected_lyrics = ledger
            .entries
            .iter()
            .filter(|entry| {
                entry.item_kind == SourceItemKind::Lyric
                    && matches!(entry.disposition, PrimaryDisposition::ProjectedExact)
            })
            .count();
        assert_eq!(projected_lyrics, 2);
        assert!(ledger
            .entries
            .iter()
            .all(|entry| !entry.source_id.contains("la")));
        let project = outcome.svp.unwrap();
        assert_eq!(
            project.tracks[0]
                .main_group
                .notes
                .iter()
                .map(|note| note.lyrics.as_str())
                .collect::<Vec<_>>(),
            ["let", "it"]
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
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|track| {
            track["mainGroup"]["notes"] == serde_json::json!([])
                && track["mainRef"]["isInstrumental"].as_bool().unwrap()
        }));
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
            let stem_plan = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
            let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);
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
                    stem_plan: stem_plan.clone(),
                    ledger,
                    warnings: vec![],
                },
                renderer: Arc::new(FakeRenderer::with_stems(
                    FakeMode::Success,
                    &stem_plan.stems,
                )),
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

    #[test]
    fn configured_real_renderer_exports_one_verified_stem_per_source_part() {
        let (Ok(executable), Ok(score)) = (
            std::env::var("VERSE_MUSESCORE_GATE"),
            std::env::var("VERSE_BUNDLE_GATE"),
        ) else {
            return;
        };
        let source_path = PathBuf::from(score);
        let source_bytes = fs::read(&source_path).unwrap();
        let midi = crate::engine::musescore::parse(&source_bytes).unwrap();
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert!(outcome.ok);
        let stem_plan = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
        let expected_stems = stem_plan.stems.len();
        let vocal_tracks = outcome.svp.as_ref().unwrap().tracks.len();
        let root = temp_dir("real-bundle-gate");
        let destination = root.join("Real.versebundle");
        let original_name = source_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let layout = BundleLayout::new(&destination, &original_name).unwrap();
        let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);
        let renderer = MuseScoreRenderer::probe(Path::new(&executable)).unwrap();
        let result = export_bundle(BundleRequest {
            destination,
            input: BundleInput {
                original_name,
                source_format: "museScore".into(),
                source_bytes,
                project: outcome.svp.unwrap(),
                stem_plan,
                ledger,
                warnings: vec![],
            },
            renderer: Arc::new(renderer),
            render_limits: RenderLimits {
                timeout: std::time::Duration::from_secs(5 * 60),
                max_output_bytes: 2 * 1024 * 1024 * 1024,
            },
        })
        .unwrap();
        assert_eq!(result.stem_count, expected_stems);
        let manifest: BundleManifest =
            serde_json::from_slice(&fs::read(result.manifest_path).unwrap()).unwrap();
        assert!(manifest.audio.coverage.complete);
        assert_eq!(manifest.audio.stems.len(), expected_stems);
        let project: serde_json::Value =
            serde_json::from_slice(&fs::read(result.project_path).unwrap()).unwrap();
        assert_eq!(
            project["tracks"].as_array().unwrap().len(),
            vocal_tracks + expected_stems + 1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_real_renderer_part_count_matches_source_topology() {
        let (Ok(executable), Ok(gate_path)) = (
            std::env::var("VERSE_MUSESCORE_GATE"),
            std::env::var("VERSE_PART_MAPPING_GATE"),
        ) else {
            return;
        };
        let gate_path = PathBuf::from(gate_path);
        let mut source_paths = if gate_path.is_dir() {
            fs::read_dir(&gate_path)
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|extension| {
                            matches!(
                                extension.to_ascii_lowercase().as_str(),
                                "kar"
                                    | "mid"
                                    | "midi"
                                    | "mxl"
                                    | "xml"
                                    | "musicxml"
                                    | "mscz"
                                    | "mscx"
                            )
                        })
                })
                .collect::<Vec<_>>()
        } else {
            vec![gate_path]
        };
        source_paths.sort();
        assert!(!source_paths.is_empty());
        let renderer = MuseScoreRenderer::probe(Path::new(&executable)).unwrap();
        for source_path in source_paths {
            eprintln!("checking {}", source_path.display());
            let source_bytes = fs::read(&source_path).unwrap();
            let extension = source_path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap()
                .to_ascii_lowercase();
            let midi = match extension.as_str() {
                "mscz" | "mscx" => crate::engine::musescore::parse(&source_bytes).unwrap(),
                "mxl" | "xml" | "musicxml" => {
                    crate::engine::musicxml::parse(&source_bytes).unwrap()
                }
                "kar" | "mid" | "midi" => crate::engine::midi::parse(&source_bytes).unwrap(),
                _ => panic!("unsupported gate extension"),
            };
            let outcome = crate::engine::convert::convert_midi(&midi, "english");
            assert!(outcome.ok);
            let stem_plan = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
            let parts = renderer
                .extract_score_parts(
                    &source_path,
                    &RenderLimits {
                        timeout: std::time::Duration::from_secs(2 * 60),
                        max_output_bytes: 512 * 1024 * 1024,
                    },
                )
                .unwrap();
            assert_eq!(
                parts.len(),
                stem_plan.stems.len(),
                "{}: MuseScore Parts {:?} differ from source stems {:?}",
                source_path.display(),
                parts.iter().map(|part| &part.name).collect::<Vec<_>>(),
                stem_plan
                    .stems
                    .iter()
                    .map(|stem| &stem.display_name)
                    .collect::<Vec<_>>()
            );
        }
    }
}
