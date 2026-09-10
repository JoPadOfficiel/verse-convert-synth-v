//! Transactional preservation-bundle export.
//!
//! A bundle is the preservation unit: the original source is retained
//! byte-for-byte, genuine vocal material remains editable in the vocal project,
//! and the note-bearing source Parts are rendered into real audio-backed
//! references alongside a muted full-score reference.
//!
//! Which vocal project a bundle carries is the export target's decision: a
//! Synthesizer V project referencing the stems through instrumental tracks, or an
//! OpenUtau project referencing the very same WAVs through `wave_parts`. Only the
//! project file differs; the source, the stems, the ledger and every relative
//! path are the same bytes either way.

use crate::engine::convert::{
    attached_lyric_instance_id, karaoke_text_lyric, note_instance_id, standalone_lyric_instance_id,
    ProjectionEvidence,
};
use crate::engine::midi::{self, Kind, Midi, MidiTextProfile, SourceTopology};
use crate::engine::projection::ProjectedProject;
use crate::engine::target::svp::{append_instrumental_track, SvpProject};
use crate::engine::target::ustx::{self, UstxProject};
use crate::engine::target::ExportTarget;
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
pub const STEM_AUDIO_DIRECTORY: &str = "audio/stems";
pub const PRESERVATION_RELATIVE_PATH: &str = "preservation.json";
pub const MANIFEST_RELATIVE_PATH: &str = "manifest.json";
const SCHEMA_VERSION: u32 = 2;
const PERFORMANCE_LEDGER_SCHEMA_VERSION: u32 = 3;
const MAX_TOTAL_AUDIO_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// How a project file inside `project/` names one of the bundle's own audio
/// artifacts.
///
/// The single place the `../` is stated. It belongs to the bundle's directory
/// layout and to neither format: Synthesizer V resolves an instrumental
/// `audio.filename` against the project file's directory, and OpenUtau resolves a
/// `wave_parts` `relative_path` with
/// `Path.Combine(Path.GetDirectoryName(project.FilePath), relativePath)`, so one
/// derivation serves both and no target can drift from the other.
pub fn project_audio_reference(audio_relative_path: &str) -> String {
    format!("../{audio_relative_path}")
}

#[derive(Clone, Debug)]
pub struct BundleLayout {
    /// Which project this bundle carries. It fixes the project file's extension
    /// and, at verification time, which audio invariant applies to it.
    pub target: ExportTarget,
    pub project_relative_path: String,
    pub audio_relative_path: String,
    pub source_relative_path: String,
    pub preservation_relative_path: String,
    pub manifest_relative_path: String,
}

impl BundleLayout {
    pub fn new(
        destination: &Path,
        original_name: &str,
        target: ExportTarget,
    ) -> Result<Self, BundleError> {
        validate_original_name(original_name)?;
        let stem = destination
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or(BundleError::InvalidDestination)?;
        let project_name = format!("{}.{}", sanitize_filename(stem), target.extension());
        Ok(Self {
            target,
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
}

/// The one vocal project a bundle carries, already in its target's own shape.
///
/// A bundle never holds two: the source, the stems and the ledger are identical
/// across targets, and a second project file would give an auditor two answers to
/// the same question. The neutral projection becomes one of these at the boundary
/// before the transaction starts, and the transaction then appends its real
/// audio-backed references to whichever shape it received.
pub enum BundleProject {
    Svp(SvpProject),
    Ustx(UstxProject),
}

impl BundleProject {
    /// Builds the project a bundle carries from the target-neutral projection.
    ///
    /// The refusal is the target's own: OpenUtau's fixed 480 ticks per quarter
    /// accept a strict subset of what Synthesizer V blicks do, so a source one
    /// target bundles is legitimately refused by the other.
    pub fn from_projection(
        target: ExportTarget,
        projected: &ProjectedProject,
    ) -> Result<Self, String> {
        if let Some(violation) = projected.continuity_violation() {
            return Err(violation);
        }
        match target {
            ExportTarget::Svp => crate::engine::target::svp::serialize(projected).map(Self::Svp),
            ExportTarget::Ustx => ustx::serialize(projected).map(Self::Ustx),
        }
    }

    pub fn target(&self) -> ExportTarget {
        match self {
            Self::Svp(_) => ExportTarget::Svp,
            Self::Ustx(_) => ExportTarget::Ustx,
        }
    }

    /// Adds one real audio-backed reference to a rendered WAV, and returns the
    /// Synthesizer V group UUID that identifies it — empty for any target that has
    /// no such thing, because a bundle states what a format holds and never
    /// invents an identity to fill a field.
    ///
    /// Both targets make the same claim about the audio: play the whole file from
    /// the start of the score. Synthesizer V spells it `blickOffset: 0`, OpenUtau
    /// spells it `position`, `skip`, `trim`, `fadein` and `fadeout` all `0`.
    fn append_audio_reference(
        &mut self,
        name: String,
        relative_path: String,
        wav: &WavInfo,
        muted: bool,
    ) -> Result<String, BundleError> {
        match self {
            Self::Svp(project) => Ok(append_instrumental_track(
                project,
                name,
                relative_path,
                wav.duration_seconds,
                0,
                muted,
            )),
            Self::Ustx(project) => {
                // The duration comes from the two integers the WAV header states,
                // never from the seconds-valued quotient beside them.
                ustx::append_wave_part(
                    project,
                    name,
                    relative_path,
                    wav.frames,
                    wav.sample_rate,
                    muted,
                )
                .map_err(BundleError::Integrity)?;
                Ok(String::new())
            }
        }
    }

    fn to_bytes(&self) -> Result<Vec<u8>, BundleError> {
        match self {
            Self::Svp(project) => Ok(serde_json::to_vec(project)?),
            Self::Ustx(project) => Ok(ustx::to_yaml(project).into_bytes()),
        }
    }
}

pub struct BundleInput {
    pub original_name: String,
    pub source_format: String,
    pub source_bytes: Vec<u8>,
    pub project: BundleProject,
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
    /// The Synthesizer V group UUID carrying this audio, and the empty string in a
    /// bundle whose project is not a Synthesizer V one.
    ///
    /// Blank rather than repurposed: an OpenUtau wave part has no group UUID, and
    /// writing a track index under a key named after another format would be a
    /// claim the file cannot support. What ties an OpenUtau reference to this
    /// record is `asset.artifact.path`, which verification already proves is
    /// referenced exactly once.
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
    /// See [`ReferenceMixRecord::svp_group_id`]: the Synthesizer V group UUID, and
    /// the empty string when the bundle's project is not a Synthesizer V one.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intensity_context: Option<IntensityContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub performance_spans: Vec<crate::engine::performance::PerformanceReference>,
    pub expected_source_ids: Vec<String>,
    pub entries: Vec<DispositionEntry>,
}

/// Independent source inventory and final eligible projection ownership. This
/// context is built from Midi/ProjectionEvidence, never from span assertions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntensityContext {
    pub source_ppq: u16,
    pub source_tracks: Vec<String>,
    pub source_notes: Vec<IntensitySourceNote>,
    pub controllers: Vec<IntensityControllerSource>,
    pub projected_notes: Vec<crate::engine::projection::IntensityNoteProjection>,
    #[serde(default)]
    pub score_owners: Vec<IntensityScoreTrack>,
    #[serde(default)]
    pub declarations: Vec<IntensityDeclarationSource>,
    /// Source-resolved dependencies in performed coordinates. These are built
    /// from bounded source normalization, never from a target transfer report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<IntensityDependencySource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntensityDependencySource {
    pub owner: IntensityScoreOwner,
    pub scope: serde_json::Value,
    pub occurrence: u32,
    pub repeat_pass: u32,
    pub start: serde_json::Value,
    pub end: serde_json::Value,
    /// True when this row is an authored source timeline segment. Other rows
    /// authenticate attack-local or diagnostic provenance without claiming a
    /// segment that the target report must reproduce.
    #[serde(default, skip_serializing_if = "is_false")]
    pub segment: bool,
    /// An original attack may use a future transition as its reference. Such
    /// evidence is bound to that attack identity, not to any later note.
    pub attack_note_id: Option<String>,
    pub source_ids: Vec<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct IntensityScoreOwner {
    pub part: String,
    pub staff: String,
    pub voice: String,
    pub instrument: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntensityScoreTrack {
    pub source_track_id: String,
    pub owner: IntensityScoreOwner,
}

/// Original parser declaration plus source-route applicability, shared by all
/// transfers. These rows never take their scope or occurrence from a report.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntensityDeclarationSource {
    pub source_id: String,
    pub kinds: serde_json::Value,
    pub scope: serde_json::Value,
    pub at: serde_json::Value,
    pub note_source_id: Option<String>,
    #[serde(default)]
    pub paired_source_ids: Vec<String>,
    pub applications: Vec<IntensityDeclarationApplication>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntensityDeclarationApplication {
    pub owner: IntensityScoreOwner,
    pub occurrence: u32,
    pub repeat_pass: u32,
    pub start: serde_json::Value,
    pub end: serde_json::Value,
    /// Original owner-run origin: `start`/`end` above are performed, while a
    /// declaration's `at` is written. Their difference is the exact route shift.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub written_start: serde_json::Value,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntensityMidiChannel {
    pub port: u8,
    pub channel: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntensitySourceNote {
    pub note_id: String,
    pub source_track_id: String,
    pub midi_channel: Option<IntensityMidiChannel>,
    pub explicit_attack: bool,
    #[serde(default)]
    pub velocity: Option<u8>,
    #[serde(default)]
    pub attack_source_id: Option<String>,
    #[serde(default)]
    pub original_source_id: String,
    #[serde(default)]
    pub score_owner: Option<IntensityScoreOwner>,
    #[serde(default)]
    pub start_tick: u32,
    #[serde(default)]
    pub source_occurrence: u32,
    /// Original raw note IDs absorbed into this nominal note by proven ties.
    #[serde(default)]
    pub merged_velocity_sources: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntensityControllerSource {
    pub source_id: String,
    pub source_track_id: String,
    pub tick: u32,
    pub owner: IntensityMidiChannel,
    pub controller: u8,
    pub value: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DispositionEntry {
    pub source_id: String,
    pub item_kind: SourceItemKind,
    pub disposition: PrimaryDisposition,
    pub artifact_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub performance_refs: Vec<usize>,
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
    /// Editable performance exists under a documented target conversion policy.
    /// This is never byte/semantic exactness of the raw controller event.
    ProjectedMapped {
        policy: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limitations: Option<String>,
    },
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
        self.validate_with_limits(allowed_artifacts, LedgerValidationLimits::default())
    }

    fn validate_with_limits(
        &self,
        allowed_artifacts: &BTreeSet<String>,
        limits: LedgerValidationLimits,
    ) -> Result<(), BundleError> {
        if self.schema_version != SCHEMA_VERSION
            && self.schema_version != PERFORMANCE_LEDGER_SCHEMA_VERSION
        {
            return Err(BundleError::InvalidLedger(format!(
                "unsupported preservation schema version {}",
                self.schema_version
            )));
        }
        let mut budget = LedgerValidationBudget::new(limits);
        // Ordinary inventory traversal is bounded by the prepaid index storage,
        // independently of the expression-specific work allowance. In particular,
        // a large schema-2 inventory must not exhaust a performance budget.
        budget.bytes(
            self.expected_source_ids
                .len()
                .saturating_mul(std::mem::size_of::<&str>()),
        )?;
        budget.bytes(self.entries.len().saturating_mul(64))?;
        if self.schema_version == 2
            && (self.intensity_context.is_some()
                || !self.performance_spans.is_empty()
                || self.entries.iter().any(|entry| {
                    !entry.performance_refs.is_empty()
                        || matches!(
                            entry.disposition,
                            PrimaryDisposition::ProjectedMapped { .. }
                        )
                }))
        {
            return Err(BundleError::InvalidLedger(
                "mapped performance requires preservation schema version 3".into(),
            ));
        }
        budget.references(self.performance_spans.len())?;
        budget.work(self.performance_spans.len())?;
        if let Some(context) = &self.intensity_context {
            budget.work(
                context
                    .source_notes
                    .len()
                    .saturating_add(context.declarations.len()),
            )?;
            budget.references(context.reference_count())?;
            budget.json(context)?;
        } else if self
            .performance_spans
            .iter()
            .any(|span| span.intensity.is_some())
        {
            return Err(BundleError::InvalidLedger(
                "intensity source context is missing or exceeded bounded storage".into(),
            ));
        }
        for span in &self.performance_spans {
            budget.references(span.note_ids.len())?;
            budget.work(span.note_ids.len())?;
            if let Some(intensity) = &span.intensity {
                budget.json(intensity)?;
            }
            for id in &span.note_ids {
                budget.bytes(id.len())?;
            }
            budget.bytes(
                span.detail
                    .len()
                    .saturating_add(span.target.len())
                    .saturating_add(span.source_track_id.len()),
            )?;
            if span.start_tick > span.end_tick
                || (span.target_track.is_none() && !span.note_ids.is_empty())
                || (span.status == crate::engine::performance::TransferStatus::Mapped
                    && (span.target_track.is_none() || span.start_tick == span.end_tick))
            {
                return Err(BundleError::InvalidLedger(
                    "invalid performance ownership span".into(),
                ));
            }
        }
        for entry in &self.entries {
            // Refuse before walking a potentially adversarial reverse list.
            budget.references(entry.performance_refs.len())?;
            budget.work(entry.performance_refs.len().saturating_mul(2))?;
            if entry
                .performance_refs
                .iter()
                .any(|index| *index >= self.performance_spans.len())
            {
                return Err(BundleError::InvalidLedger(
                    "performance reference points outside the span table".into(),
                ));
            }
            if matches!(
                entry.disposition,
                PrimaryDisposition::ProjectedMapped { .. }
            ) && !entry.performance_refs.iter().any(|index| {
                self.performance_spans[*index].status
                    == crate::engine::performance::TransferStatus::Mapped
            }) {
                return Err(BundleError::InvalidLedger(
                    "mapped disposition has no mapped target span".into(),
                ));
            }
        }
        // Borrow identity strings; storage for both ordinary indexes was reserved
        // before traversal. Their O(n log n) work is bounded by that storage cap.
        let mut expected: Vec<_> = self
            .expected_source_ids
            .iter()
            .map(String::as_str)
            .collect();
        expected.sort_unstable();
        if expected.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(BundleError::InvalidLedger(
                "duplicate expected source ID".into(),
            ));
        }
        let entries: BTreeMap<_, _> = self
            .entries
            .iter()
            .map(|entry| (entry.source_id.as_str(), entry))
            .collect();
        if entries.len() != self.entries.len() {
            return Err(BundleError::InvalidLedger(
                "multiple dispositions for one source ID".into(),
            ));
        }
        if !expected.iter().copied().eq(entries.keys().copied()) {
            return Err(BundleError::InvalidLedger(
                "every inventoried item must have exactly one disposition".into(),
            ));
        }
        let ownership = self
            .intensity_context
            .as_ref()
            .map(|context| IntensityOwnership::new(context, &entries, &mut budget))
            .transpose()
            .map_err(BundleError::InvalidLedger)?;
        budget.index(
            self.performance_spans.len(),
            std::mem::size_of::<BTreeSet<&str>>(),
        )?;
        let mut contributors = vec![BTreeSet::new(); self.performance_spans.len()];
        for entry in &self.entries {
            budget.index(entry.performance_refs.len(), 64)?;
            for index in &entry.performance_refs {
                contributors[*index].insert(entry.source_id.as_str());
            }
        }
        for (index, span) in self.performance_spans.iter().enumerate() {
            if let Some(intensity) = &span.intensity {
                validate_intensity_extension(
                    intensity,
                    span,
                    &entries,
                    ownership.as_ref().expect("required intensity context"),
                    &contributors[index],
                    &mut budget,
                )
                .map_err(BundleError::InvalidLedger)?;
            }
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

/// The persisted reference/text limits and actual validation work share one
/// pass. Tests lower these limits on the same path used before publication.
#[derive(Clone, Copy)]
struct LedgerValidationLimits {
    references: usize,
    bytes: usize,
    work: usize,
}
impl Default for LedgerValidationLimits {
    fn default() -> Self {
        Self {
            references: 250_000,
            bytes: 32 * 1024 * 1024,
            work: 2_000_000,
        }
    }
}
struct LedgerValidationBudget {
    limits: LedgerValidationLimits,
    references: usize,
    bytes: usize,
    work: usize,
}
impl LedgerValidationBudget {
    fn new(limits: LedgerValidationLimits) -> Self {
        Self {
            limits,
            references: 0,
            bytes: 0,
            work: 0,
        }
    }
    fn add(value: &mut usize, extra: usize, limit: usize) -> Result<(), String> {
        *value = value.saturating_add(extra);
        if *value > limit {
            return Err("performance evidence exceeds bounded storage".into());
        }
        Ok(())
    }
    fn references(&mut self, n: usize) -> Result<(), BundleError> {
        Self::add(&mut self.references, n, self.limits.references)
            .map_err(BundleError::InvalidLedger)
    }
    fn bytes(&mut self, n: usize) -> Result<(), BundleError> {
        Self::add(&mut self.bytes, n, self.limits.bytes).map_err(BundleError::InvalidLedger)
    }
    fn work(&mut self, n: usize) -> Result<(), BundleError> {
        Self::add(&mut self.work, n, self.limits.work).map_err(BundleError::InvalidLedger)
    }
    fn index(&mut self, n: usize, size: usize) -> Result<(), BundleError> {
        self.work(n.saturating_mul(1 + usize::BITS as usize - n.max(1).leading_zeros() as usize))?;
        self.bytes(n.saturating_mul(size))
    }
    fn visit(&mut self, n: usize) -> Result<(), String> {
        Self::add(&mut self.work, n, self.limits.work)
    }
    fn reference(&mut self, n: usize) -> Result<(), String> {
        Self::add(&mut self.references, n, self.limits.references)?;
        self.visit(n)
    }
    fn reserve(&mut self, n: usize, size: usize) -> Result<(), String> {
        self.visit(n.saturating_mul(1 + usize::BITS as usize - n.max(1).leading_zeros() as usize))?;
        Self::add(&mut self.bytes, n.saturating_mul(size), self.limits.bytes)
    }
    fn json<T: Serialize>(&mut self, value: &T) -> Result<(), BundleError> {
        // Charge bounded byte traversal, not serde's arbitrary write fragment
        // count: punctuation and escaped strings can use many tiny callbacks.
        // Every 64-byte block is prepaid, including the final partial block.
        // Reference and index traversal retain their separate work charges.
        struct Counter<'a> {
            budget: &'a mut LedgerValidationBudget,
            prepaid: usize,
        }
        impl Write for Counter<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes.len() > self.prepaid {
                    let blocks = bytes.len().saturating_sub(self.prepaid).div_ceil(64);
                    self.budget.visit(blocks).map_err(io::Error::other)?;
                    self.prepaid = self.prepaid.saturating_add(blocks.saturating_mul(64));
                }
                LedgerValidationBudget::add(
                    &mut self.budget.bytes,
                    bytes.len(),
                    self.budget.limits.bytes,
                )
                .map_err(io::Error::other)?;
                self.prepaid -= bytes.len();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        self.work(1)?;
        serde_json::to_writer(
            Counter {
                budget: self,
                prepaid: 64,
            },
            value,
        )
        .map_err(|_| {
            BundleError::InvalidLedger("performance evidence exceeds bounded storage".into())
        })
    }
}

type JsonFingerprintIndex<'a> = BTreeMap<u64, Vec<(&'a serde_json::Value, usize)>>;

fn json_fingerprint(
    value: &serde_json::Value,
    count: &mut LedgerValidationBudget,
) -> Result<(u64, usize), String> {
    use std::hash::{Hash, Hasher};
    struct WorkHasher {
        inner: std::collections::hash_map::DefaultHasher,
        bytes: usize,
    }
    impl Hasher for WorkHasher {
        fn finish(&self) -> u64 {
            self.inner.finish()
        }
        fn write(&mut self, bytes: &[u8]) {
            self.bytes = self.bytes.saturating_add(bytes.len());
            self.inner.write(bytes);
        }
    }
    let mut hasher = WorkHasher {
        inner: std::collections::hash_map::DefaultHasher::new(),
        bytes: 0,
    };
    value.hash(&mut hasher);
    let work = hasher.bytes.div_ceil(64).max(1);
    count.visit(work)?;
    Ok((hasher.finish(), work))
}

fn json_index_contains_prehashed(
    index: &JsonFingerprintIndex<'_>,
    value: &serde_json::Value,
    fingerprint: u64,
    value_work: usize,
    count: &mut LedgerValidationBudget,
) -> Result<bool, String> {
    count.visit(1 + usize::BITS as usize - index.len().max(1).leading_zeros() as usize)?;
    let Some(candidates) = index.get(&fingerprint) else {
        return Ok(false);
    };
    for (candidate, candidate_work) in candidates {
        // Equality may traverse the full JSON value. Charge that comparison as
        // work; persisted bytes were already charged once before validation.
        count.visit(value_work.saturating_add(*candidate_work))?;
        if *candidate == value {
            return Ok(true);
        }
    }
    Ok(false)
}

fn json_index_contains(
    index: &JsonFingerprintIndex<'_>,
    value: &serde_json::Value,
    count: &mut LedgerValidationBudget,
) -> Result<bool, String> {
    let (fingerprint, work) = json_fingerprint(value, count)?;
    json_index_contains_prehashed(index, value, fingerprint, work, count)
}

// The optional v1 extension has three emitted forms: a USTX gain span,
// an SVP neutral-segment report, and a scoped issue/terminal point report.
// Read borrowed JSON throughout so large raw source evidence is never cloned.
struct IntensityJsonCounter {
    bytes: usize,
}
impl Write for IntensityJsonCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > 32 * 1024 * 1024 {
            return Err(io::Error::other("intensity evidence budget exceeded"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl IntensityContext {
    fn reference_count(&self) -> usize {
        self.source_tracks
            .len()
            .saturating_add(self.source_notes.len().saturating_mul(4))
            .saturating_add(self.controllers.len().saturating_mul(2))
            .saturating_add(self.projected_notes.len().saturating_mul(4))
            .saturating_add(self.score_owners.len().saturating_mul(2))
            .saturating_add(self.declarations.iter().fold(0usize, |n, d| {
                n.saturating_add(2)
                    .saturating_add(d.applications.len())
                    .saturating_add(d.paired_source_ids.len())
            }))
            .saturating_add(self.source_notes.iter().fold(0usize, |n, note| {
                n.saturating_add(note.merged_velocity_sources.len())
            }))
            .saturating_add(self.dependencies.iter().fold(0usize, |n, dependency| {
                n.saturating_add(4)
                    .saturating_add(dependency.source_ids.len())
            }))
    }
}

fn intensity_charge(count: &mut usize, extra: usize) -> Result<(), String> {
    *count = count.saturating_add(extra);
    if *count > 250_000 {
        return Err("performance evidence exceeds bounded storage".into());
    }
    Ok(())
}

type IntensityProjectionIndex<'a> =
    BTreeMap<(usize, &'a str), Vec<&'a crate::engine::projection::IntensityNoteProjection>>;
type IntensityApplicationIndex<'a> =
    BTreeMap<(&'a str, &'a IntensityScoreOwner, u32, u32), &'a IntensityDeclarationApplication>;

struct IntensityOwnership<'a> {
    context: &'a IntensityContext,
    notes: BTreeMap<&'a str, &'a IntensitySourceNote>,
    projections: IntensityProjectionIndex<'a>,
    endpoints: BTreeSet<(usize, &'a str, u32)>,
    controllers: BTreeMap<&'a str, &'a IntensityControllerSource>,
    tracks: BTreeSet<&'a str>,
    declarations: BTreeMap<&'a str, &'a IntensityDeclarationSource>,
    applications: IntensityApplicationIndex<'a>,
    score_tracks: BTreeMap<&'a str, Vec<&'a IntensityScoreOwner>>,
    attacks: BTreeMap<&'a str, &'a IntensitySourceNote>,
    dependencies: BTreeMap<&'a str, Vec<&'a IntensityDependencySource>>,
}
impl<'a> IntensityOwnership<'a> {
    fn new(
        context: &'a IntensityContext,
        entries: &BTreeMap<&str, &DispositionEntry>,
        budget: &mut LedgerValidationBudget,
    ) -> Result<Self, String> {
        budget.visit(
            context
                .source_notes
                .len()
                .saturating_add(context.declarations.len()),
        )?;
        let attack_count = context
            .source_notes
            .iter()
            .filter(|note| note.attack_source_id.is_some())
            .count();
        let application_count = context.declarations.iter().fold(0usize, |count, row| {
            count.saturating_add(row.applications.len())
        });
        // Each borrowed index is bounded by its own row count. Persistent
        // identity-field references are not rows in one giant sorted tree.
        // Include the projection/owner vectors in their map storage allowance.
        for (rows, bytes) in [
            (context.source_tracks.len(), 64),
            (context.source_notes.len(), 64),
            (attack_count, 64),
            (context.projected_notes.len(), 128),
            (context.projected_notes.len(), 64),
            (context.projected_notes.len(), 64),
            (context.projected_notes.len(), 64),
            (context.controllers.len(), 64),
            (context.score_owners.len(), 128),
            (context.score_owners.len(), 64),
            (context.score_owners.len(), 64),
            (context.declarations.len(), 64),
            (application_count, 128),
            (application_count, 96),
        ] {
            budget.reserve(rows, bytes)?;
        }
        if context.source_ppq == 0 {
            return Err("invalid intensity source PPQ".into());
        }
        let mut tracks = BTreeSet::new();
        for id in &context.source_tracks {
            budget.reserve(1, id.len().saturating_add(6))?;
            if id.is_empty()
                || !tracks.insert(id.as_str())
                || !entries
                    .get(format!("track:{id}").as_str())
                    .is_some_and(|entry| entry.item_kind == SourceItemKind::Track)
            {
                return Err("invalid intensity source track table".into());
            }
        }
        let mut notes = BTreeMap::new();
        let mut attacks = BTreeMap::new();
        for note in &context.source_notes {
            if !tracks.contains(note.source_track_id.as_str())
                || !entries
                    .get(note.note_id.as_str())
                    .is_some_and(|e| e.item_kind == SourceItemKind::Note)
                || note.midi_channel.is_some_and(|owner| owner.channel >= 16)
                || notes.insert(note.note_id.as_str(), note).is_some()
            {
                return Err("invalid intensity source note table".into());
            }
            if note.explicit_attack != note.velocity.is_some()
                || note.velocity.is_some_and(|v| v == 0 || v > 127)
                || note.explicit_attack != note.attack_source_id.is_some()
                || (note.explicit_attack && note.midi_channel.is_none())
            {
                return Err("invalid intensity source velocity table".into());
            }
            if let Some(id) = &note.attack_source_id {
                if !entries
                    .get(id.as_str())
                    .is_some_and(|e| e.item_kind == SourceItemKind::Event)
                    || attacks.insert(id.as_str(), note).is_some()
                {
                    return Err("invalid intensity source velocity table".into());
                }
            }
        }
        let mut projections: BTreeMap<_, Vec<_>> = BTreeMap::new();
        let mut endpoints = BTreeSet::new();
        let mut destinations = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for note in &context.projected_notes {
            if note.start_tick >= note.end_tick
                || note.destination_track_id.is_empty()
                || notes
                    .get(note.note_id.as_str())
                    .is_none_or(|source| source.source_track_id != note.source_track_id)
                || !matches!(
                    entries[note.note_id.as_str()].disposition,
                    PrimaryDisposition::ProjectedExact
                )
                || !seen.insert((
                    note.target_track,
                    note.note_id.as_str(),
                    note.start_tick,
                    note.end_tick,
                ))
            {
                return Err("invalid intensity eligible projection table".into());
            }
            if destinations
                .insert(note.target_track, note.destination_track_id.as_str())
                .is_some_and(|previous| previous != note.destination_track_id)
            {
                return Err("intensity target track has conflicting destination owners".into());
            }
            projections
                .entry((note.target_track, note.note_id.as_str()))
                .or_default()
                .push(note);
            endpoints.insert((
                note.target_track,
                note.source_track_id.as_str(),
                note.end_tick,
            ));
        }
        for note in &context.projected_notes {
            if let Some(root) = &note.intensity_attack_note_id {
                let tail = notes[note.note_id.as_str()];
                let head = notes
                    .get(root.as_str())
                    .ok_or("invalid intensity attack predecessor")?;
                if root == &note.note_id
                    || head.start_tick >= tail.start_tick
                    || head.score_owner != tail.score_owner
                    || head.midi_channel != tail.midi_channel
                    || !projections.contains_key(&(note.target_track, root.as_str()))
                {
                    return Err("invalid intensity attack predecessor".into());
                }
            }
        }
        let mut controllers = BTreeMap::new();
        for controller in &context.controllers {
            if !tracks.contains(controller.source_track_id.as_str())
                || !entries
                    .get(controller.source_id.as_str())
                    .is_some_and(|entry| entry.item_kind == SourceItemKind::Event)
                || controller.owner.channel >= 16
                || !matches!(controller.controller, 7 | 11)
                || controller.value > 127
                || controllers
                    .insert(controller.source_id.as_str(), controller)
                    .is_some()
            {
                return Err("invalid intensity controller source table".into());
            }
        }
        let mut score_tracks = BTreeMap::<_, Vec<_>>::new();
        let mut seen_owners = BTreeSet::new();
        for track in &context.score_owners {
            if !tracks.contains(track.source_track_id.as_str())
                || track.owner.part.is_empty()
                || !seen_owners.insert((track.source_track_id.as_str(), &track.owner))
            {
                return Err("invalid intensity score owner table".into());
            }
            score_tracks
                .entry(track.source_track_id.as_str())
                .or_default()
                .push(&track.owner);
        }
        for note in &context.source_notes {
            if note
                .score_owner
                .as_ref()
                .is_some_and(|owner| !seen_owners.contains(&(note.source_track_id.as_str(), owner)))
            {
                return Err("intensity note has no original score owner".into());
            }
        }
        let mut declarations = BTreeMap::new();
        let mut applications = BTreeMap::new();
        let mut source_routes = BTreeMap::new();
        // Earlier schema-3 intensity contexts contain neither route origins nor
        // source-resolved dependencies. Preserve their original scope/route
        // validation without inventing a written-to-performed timing offset.
        // A partially populated new context must not silently use that path.
        budget.visit(application_count)?;
        let timed_declarations = !context.dependencies.is_empty()
            || context.declarations.iter().any(|row| {
                row.applications
                    .iter()
                    .any(|application| !application.written_start.is_null())
            });
        let declared_owners: BTreeSet<_> = context
            .score_owners
            .iter()
            .map(|track| &track.owner)
            .collect();
        for row in &context.declarations {
            if !entries
                .get(row.source_id.as_str())
                .is_some_and(|e| e.item_kind == SourceItemKind::Event)
                || !intensity_scope(&row.scope)
                || row.kinds.as_array().is_none_or(|kinds| {
                    kinds.is_empty()
                        || kinds.iter().any(|k| {
                            !matches!(
                                k.as_str(),
                                Some(
                                    "Dynamic"
                                        | "Transition"
                                        | "Text"
                                        | "Velocity"
                                        | "Tempo"
                                        | "WedgeStart"
                                        | "WedgeContinue"
                                        | "WedgeStop"
                                        | "SpannerEndpoint"
                                        | "Unsupported"
                                )
                            )
                        })
                })
                || declarations.insert(row.source_id.as_str(), row).is_some()
            {
                return Err("invalid original intensity declaration table".into());
            }
            let at = intensity_fraction(&row.at)?;
            if at < crate::engine::score_intensity::Time::ZERO
                || (row
                    .kinds
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|k| k == "Velocity")
                    && row.note_source_id.as_ref().is_none_or(String::is_empty))
            {
                return Err("invalid original intensity declaration table".into());
            }
            for application in &row.applications {
                budget.visit(1)?;
                let start = intensity_fraction(&application.start)?;
                let end = intensity_fraction(&application.end)?;
                let written_start = if timed_declarations {
                    Some(intensity_fraction(&application.written_start)?)
                } else {
                    None
                };
                if !declared_owners.contains(&application.owner)
                    || !score_scope_applies(&row.scope, &application.owner)
                    || start < crate::engine::score_intensity::Time::ZERO
                    || written_start
                        .is_some_and(|at| at < crate::engine::score_intensity::Time::ZERO)
                    || end < start
                    || ((application.occurrence == 0) != (application.repeat_pass == 0))
                    || (application.occurrence == 0
                        && (start != at || end != at || application.active))
                    || (application.occurrence != 0 && start == end)
                    || applications
                        .insert(
                            (
                                row.source_id.as_str(),
                                &application.owner,
                                application.occurrence,
                                application.repeat_pass,
                            ),
                            application,
                        )
                        .is_some()
                {
                    return Err("invalid intensity declaration occurrence table".into());
                }
                if application.occurrence != 0
                    && source_routes
                        .insert(
                            (
                                &application.owner,
                                application.occurrence,
                                application.repeat_pass,
                            ),
                            (start, end, written_start),
                        )
                        .is_some_and(|previous| previous != (start, end, written_start))
                {
                    return Err("conflicting intensity source route bounds".into());
                }
            }
        }
        for row in &context.declarations {
            budget.visit(row.paired_source_ids.len())?;
            if row
                .paired_source_ids
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            {
                return Err("invalid original intensity endpoint relation".into());
            }
            for id in &row.paired_source_ids {
                budget.visit(
                    1 + usize::BITS as usize - declarations.len().max(1).leading_zeros() as usize,
                )?;
                let other = declarations
                    .get(id.as_str())
                    .ok_or("invalid original intensity endpoint relation")?;
                budget.visit(
                    1 + usize::BITS as usize
                        - other.paired_source_ids.len().max(1).leading_zeros() as usize,
                )?;
                if id == &row.source_id
                    || other
                        .paired_source_ids
                        .binary_search(&row.source_id)
                        .is_err()
                {
                    return Err("invalid original intensity endpoint relation".into());
                }
            }
        }
        let mut dependencies = BTreeMap::<&str, Vec<&IntensityDependencySource>>::new();
        for dependency in &context.dependencies {
            budget.visit(1)?;
            let start = intensity_fraction(&dependency.start)?;
            let end = intensity_fraction(&dependency.end)?;
            if !declared_owners.contains(&dependency.owner)
                || !intensity_scope(&dependency.scope)
                || !score_scope_applies(&dependency.scope, &dependency.owner)
                || start < crate::engine::score_intensity::Time::ZERO
                || end < start
                || ((dependency.occurrence == 0) != (dependency.repeat_pass == 0))
                || dependency.source_ids.is_empty()
                || dependency.source_ids.windows(2).any(|ids| ids[0] >= ids[1])
            {
                return Err("invalid original intensity dependency table".into());
            }
            if let Some(id) = &dependency.attack_note_id {
                let note = notes
                    .get(id.as_str())
                    .ok_or("invalid intensity dependency attack")?;
                let at = crate::engine::score_intensity::source::time(
                    i64::from(note.start_tick),
                    context.source_ppq,
                )?;
                if note.score_owner.as_ref() != Some(&dependency.owner) || start != at || end != at
                {
                    return Err("invalid intensity dependency attack".into());
                }
            }
            budget.reserve(dependency.source_ids.len(), 96)?;
            for id in &dependency.source_ids {
                let row = declarations
                    .get(id.as_str())
                    .ok_or("intensity dependency has no original declaration")?;
                if !score_scope_applies(&row.scope, &dependency.owner) {
                    return Err("intensity dependency has unrelated declaration scope".into());
                }
                let application = applications
                    .get(&(
                        id.as_str(),
                        &dependency.owner,
                        dependency.occurrence,
                        dependency.repeat_pass,
                    ))
                    .ok_or("intensity dependency has no original route application")?;
                let route_start = intensity_fraction(&application.start)?;
                let route_end = intensity_fraction(&application.end)?;
                if start < route_start || end > route_end {
                    return Err(
                        "intensity dependency exceeds its original route application".into(),
                    );
                }
                dependencies.entry(id).or_default().push(dependency);
            }
        }
        Ok(Self {
            context,
            notes,
            projections,
            endpoints,
            controllers,
            tracks,
            declarations,
            applications,
            score_tracks,
            attacks,
            dependencies,
        })
    }

    fn validate_span(
        &self,
        span: &crate::engine::performance::PerformanceReference,
        start: crate::engine::score_intensity::Time,
        end: crate::engine::score_intensity::Time,
        terminal: bool,
        count: &mut LedgerValidationBudget,
    ) -> Result<(), String> {
        use crate::engine::score_intensity::Fraction;
        if !self.tracks.contains(span.source_track_id.as_str()) {
            return Err("intensity span source track is outside the source table".into());
        }
        if crate::engine::performance::exact_source_tick_bounds(
            start,
            end,
            self.context.source_ppq,
        )? != (span.start_tick, span.end_tick)
        {
            return Err("exact intensity bounds disagree with source tick coverage".into());
        }
        let Some(target) = span.target_track else {
            return Ok(());
        };
        if span.note_ids.is_empty() {
            if !terminal
                || !self.endpoints.contains(&(
                    target,
                    span.source_track_id.as_str(),
                    span.start_tick,
                ))
            {
                return Err("intensity terminal has no eligible destination endpoint".into());
            }
            return Ok(());
        }
        count.reserve(span.note_ids.len(), 64)?;
        let mut coverage = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for id in &span.note_ids {
            if !ids.insert(id)
                || self
                    .notes
                    .get(id.as_str())
                    .is_none_or(|note| note.source_track_id != span.source_track_id)
            {
                return Err("intensity note does not belong to its source track".into());
            }
            let owners = self
                .projections
                .get(&(target, id.as_str()))
                .ok_or("intensity note has no eligible target ownership")?;
            let mut owned = false;
            for note in owners {
                count.reference(1)?;
                let a = Fraction::new(
                    i64::from(note.start_tick),
                    i64::from(self.context.source_ppq),
                )?;
                let b =
                    Fraction::new(i64::from(note.end_tick), i64::from(self.context.source_ppq))?;
                let intersects = if start == end {
                    a <= start && start < b
                } else {
                    a < end && start < b
                };
                if intersects {
                    owned = true;
                    count.reserve(1, 96)?;
                    coverage.insert((a, b));
                }
            }
            if !owned {
                return Err("intensity note has no eligible interval".into());
            }
        }
        let mut covered = start;
        for (a, b) in coverage {
            if a > covered {
                break;
            }
            covered = covered.max(b);
        }
        if covered < end {
            return Err("intensity interval exceeds eligible projected notes".into());
        }
        Ok(())
    }

    fn authenticate_provenance(
        &self,
        provenance: &serde_json::Value,
        span: &crate::engine::performance::PerformanceReference,
        start: crate::engine::score_intensity::Time,
        end: crate::engine::score_intensity::Time,
        count: &mut LedgerValidationBudget,
    ) -> Result<(), String> {
        use crate::engine::performance::TransferStatus;
        use crate::engine::score_intensity::source::time;
        let occurrence = provenance["occurrence"].as_u64().unwrap() as u32;
        let pass = provenance["repeat_pass"].as_u64().unwrap() as u32;
        let scope = &provenance["scope"];
        let mut scope_witness = false;
        for evidence in provenance["evidence"].as_array().unwrap() {
            for id in evidence["source_ids"].as_array().unwrap() {
                let id = id.as_str().unwrap();
                count.visit(
                    2 * (1 + usize::BITS as usize
                        - self
                            .declarations
                            .len()
                            .max(self.attacks.len())
                            .max(1)
                            .leading_zeros() as usize),
                )?;
                if let Some(attack) = self.attacks.get(id) {
                    let owner = attack.midi_channel.unwrap();
                    if evidence["contract"] != "Midi"
                        || occurrence != 0
                        || pass != 0
                        || scope["Midi"]["port"].as_u64() != Some(u64::from(owner.port))
                        || scope["Midi"]["channel"].as_u64() != Some(u64::from(owner.channel))
                        || span.note_ids.is_empty()
                    {
                        return Err("intensity velocity has unrelated scope or occurrence".into());
                    }
                    for note_id in &span.note_ids {
                        count.visit(1)?;
                        let root = self.attack_note(span, note_id, count)?;
                        if root.note_id != attack.note_id || attack.start_tick > span.start_tick {
                            return Err(
                                "intensity velocity does not belong to its source attack".into()
                            );
                        }
                    }
                    scope_witness = true;
                    continue;
                }
                let row = self
                    .declarations
                    .get(id)
                    .ok_or("intensity contributor is not an original expression declaration")?;
                let ids = evidence["source_ids"].as_array().unwrap();
                if ids.len() > 1
                    && row.kinds.as_array().unwrap().iter().any(|kind| {
                        matches!(
                            kind.as_str(),
                            Some("WedgeContinue" | "WedgeStop" | "SpannerEndpoint")
                        )
                    })
                {
                    count.visit(ids.len().saturating_mul(
                        1 + usize::BITS as usize
                            - row.paired_source_ids.len().max(1).leading_zeros() as usize,
                    ))?;
                    if !ids.iter().any(|candidate| {
                        row.paired_source_ids
                            .binary_search_by(|paired| {
                                paired.as_str().cmp(candidate.as_str().unwrap())
                            })
                            .is_ok()
                    }) {
                        return Err("intensity endpoint has no original paired declaration".into());
                    }
                }
                scope_witness |= row.scope == *scope;
                let velocity = row
                    .kinds
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|kind| kind == "Velocity");
                if span.note_ids.is_empty() {
                    let owners = self
                        .score_tracks
                        .get(span.source_track_id.as_str())
                        .ok_or("intensity provenance has no original score owner")?;
                    let mut applicable = false;
                    for owner in owners {
                        count.visit(1)?;
                        if !score_scope_applies(scope, owner) {
                            continue;
                        }
                        if self.declaration_application(
                            row,
                            owner,
                            occurrence,
                            pass,
                            start,
                            end,
                            span.status == TransferStatus::Mapped,
                            count,
                        )? {
                            self.declaration_timing(
                                row, owner, provenance, span, start, end, count,
                            )?;
                            applicable = true;
                            break;
                        }
                    }
                    if !applicable {
                        return Err(
                            "intensity declaration has unrelated scope or occurrence".into()
                        );
                    }
                } else {
                    for note_id in &span.note_ids {
                        count.visit(1)?;
                        let note = self.notes[note_id.as_str()];
                        let owner = note
                            .score_owner
                            .as_ref()
                            .ok_or("score intensity contributor has no original score owner")?;
                        if !score_scope_applies(scope, owner) {
                            return Err(
                                "intensity provenance scope does not apply to its source owner"
                                    .into(),
                            );
                        }
                        let (lo, hi) = if velocity {
                            let root = self.attack_note(span, note_id, count)?;
                            let raw = row.note_source_id.as_deref().unwrap();
                            count.visit(root.merged_velocity_sources.len())?;
                            if root.original_source_id != raw
                                && !(span.status == TransferStatus::Unsupported
                                    && root.merged_velocity_sources.iter().any(|id| id == raw))
                                && !(span.status == TransferStatus::Unsupported
                                    && note.original_source_id == raw)
                            {
                                return Err(
                                    "intensity velocity does not belong to its source attack"
                                        .into(),
                                );
                            }
                            let at = time(i64::from(root.start_tick), self.context.source_ppq)?;
                            (at, at)
                        } else {
                            (start, end)
                        };
                        if occurrence == 0
                            || pass == 0
                            || !self.declaration_application(
                                row,
                                owner,
                                occurrence,
                                pass,
                                lo,
                                hi,
                                span.status == TransferStatus::Mapped,
                                count,
                            )?
                        {
                            return Err(
                                "intensity declaration has unrelated scope or occurrence".into()
                            );
                        }
                        if !velocity {
                            self.declaration_timing(row, owner, provenance, span, lo, hi, count)?;
                        }
                    }
                }
            }
        }
        // Combined start/held/endpoint/tempo contributors can have different
        // applicable scopes. At least one original declaration must witness the
        // reported scope; a merely nonempty invented scope is insufficient.
        if !scope_witness {
            return Err("intensity provenance scope has no original declaration witness".into());
        }
        Ok(())
    }

    fn attack_note(
        &self,
        span: &crate::engine::performance::PerformanceReference,
        id: &str,
        count: &mut LedgerValidationBudget,
    ) -> Result<&'a IntensitySourceNote, String> {
        let note = self
            .notes
            .get(id)
            .ok_or("intensity note is not inventoried")?;
        let mut root = None;
        if let Some(projections) = span
            .target_track
            .and_then(|target| self.projections.get(&(target, id)))
        {
            count.visit(projections.len())?;
            for projection in projections {
                if let Some(candidate) = &projection.intensity_attack_note_id {
                    if root.is_some_and(|previous| previous != candidate.as_str()) {
                        return Err("conflicting intensity attack predecessors".into());
                    }
                    root = Some(candidate.as_str());
                }
            }
        }
        root.map_or(Ok(*note), |root| {
            self.notes
                .get(root)
                .copied()
                .ok_or_else(|| "invalid intensity attack predecessor".into())
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn declaration_application(
        &self,
        row: &IntensityDeclarationSource,
        owner: &IntensityScoreOwner,
        occurrence: u32,
        pass: u32,
        start: crate::engine::score_intensity::Time,
        end: crate::engine::score_intensity::Time,
        active: bool,
        count: &mut LedgerValidationBudget,
    ) -> Result<bool, String> {
        count.visit(
            1 + usize::BITS as usize - self.applications.len().max(1).leading_zeros() as usize,
        )?;
        if !score_scope_applies(&row.scope, owner) {
            return Ok(false);
        }
        let Some(application) =
            self.applications
                .get(&(row.source_id.as_str(), owner, occurrence, pass))
        else {
            return Ok(false);
        };
        let lo = intensity_fraction(&application.start)?;
        let hi = intensity_fraction(&application.end)?;
        Ok((!active || application.active) && lo <= start && end <= hi)
    }

    #[allow(clippy::too_many_arguments)]
    fn declaration_timing(
        &self,
        row: &IntensityDeclarationSource,
        owner: &IntensityScoreOwner,
        provenance: &serde_json::Value,
        span: &crate::engine::performance::PerformanceReference,
        start: crate::engine::score_intensity::Time,
        end: crate::engine::score_intensity::Time,
        count: &mut LedgerValidationBudget,
    ) -> Result<(), String> {
        let occurrence = provenance["occurrence"].as_u64().unwrap() as u32;
        let pass = provenance["repeat_pass"].as_u64().unwrap() as u32;
        count.visit(
            1 + usize::BITS as usize - self.applications.len().max(1).leading_zeros() as usize,
        )?;
        let application = self
            .applications
            .get(&(row.source_id.as_str(), owner, occurrence, pass))
            .ok_or("intensity declaration has unrelated scope or occurrence")?;
        if application.written_start.is_null() {
            // IntensityOwnership::new permits this only for a wholly legacy
            // context. Its source scope, occurrence, interval, endpoint and
            // contributor checks still apply; the newer timing proof is absent.
            return Ok(());
        }
        // Current contexts authenticate the declaration through the independent
        // bounded source resolver for every interval, not merely when the
        // declaration happens in the future. That prevents a superseded earlier
        // dynamic from being substituted into a later note while preserving real
        // held dynamics, paired endpoints and tempo dependencies.
        count.visit(
            1 + usize::BITS as usize - self.dependencies.len().max(1).leading_zeros() as usize,
        )?;
        for dependency in self
            .dependencies
            .get(row.source_id.as_str())
            .into_iter()
            .flatten()
        {
            count.visit(1)?;
            if &dependency.owner != owner
                || dependency.occurrence != occurrence
                || dependency.repeat_pass != pass
                || dependency.scope != provenance["scope"]
            {
                continue;
            }
            let mut interval_owned = intensity_fraction(&dependency.start)? <= start
                && end <= intensity_fraction(&dependency.end)?;
            if let Some(attack) = &dependency.attack_note_id {
                interval_owned = !span.note_ids.is_empty();
                for id in &span.note_ids {
                    count.visit(1)?;
                    if self.attack_note(span, id, count)?.note_id != *attack {
                        interval_owned = false;
                        break;
                    }
                }
            }
            if !interval_owned {
                continue;
            }
            return Ok(());
        }
        Err("intensity declaration has unrelated time or dependency".into())
    }

    fn validate_segment_dependency_coverage(
        &self,
        span: &crate::engine::performance::PerformanceReference,
        start: crate::engine::score_intensity::Time,
        end: crate::engine::score_intensity::Time,
        summary: &[serde_json::Value],
        segments: &[serde_json::Value],
        count: &mut LedgerValidationBudget,
    ) -> Result<(), String> {
        let source_on_segment = |value: &serde_json::Value, source: &str| {
            value["evidence"].as_array().is_some_and(|evidence| {
                evidence.iter().any(|item| {
                    item["source_ids"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(source)))
                })
            })
        };
        for provenance in summary {
            let occurrence = provenance["occurrence"].as_u64().unwrap() as u32;
            let pass = provenance["repeat_pass"].as_u64().unwrap() as u32;
            let scope = &provenance["scope"];
            for source in provenance["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|evidence| evidence["source_ids"].as_array().unwrap())
                .filter_map(serde_json::Value::as_str)
            {
                if !self.declarations.contains_key(source) {
                    continue;
                }
                count.visit(
                    1 + usize::BITS as usize
                        - self.dependencies.len().max(1).leading_zeros() as usize,
                )?;
                let Some(dependencies) = self.dependencies.get(source) else {
                    continue;
                };
                for dependency in dependencies {
                    count.visit(1)?;
                    if !dependency.segment
                        || dependency.attack_note_id.is_some()
                        || dependency.occurrence != occurrence
                        || dependency.repeat_pass != pass
                        || dependency.scope != *scope
                    {
                        continue;
                    }
                    let dependency_start = intensity_fraction(&dependency.start)?;
                    let dependency_end = intensity_fraction(&dependency.end)?;
                    if dependency_start >= end || dependency_end <= start {
                        continue;
                    }
                    let owner_applies = if span.note_ids.is_empty() {
                        self.score_tracks
                            .get(span.source_track_id.as_str())
                            .is_some_and(|owners| {
                                owners.iter().any(|owner| **owner == dependency.owner)
                            })
                    } else {
                        span.note_ids.iter().any(|note_id| {
                            self.notes
                                .get(note_id.as_str())
                                .and_then(|note| note.score_owner.as_ref())
                                == Some(&dependency.owner)
                        })
                    };
                    if !owner_applies {
                        continue;
                    }
                    let mut covered = false;
                    for segment in segments {
                        count.visit(1)?;
                        if segment["provenance"].is_null() {
                            continue;
                        }
                        let (segment_start, segment_end) = intensity_bounds(segment)?;
                        let candidate = &segment["provenance"];
                        if segment_start == dependency_start
                            && segment_end == dependency_end
                            && candidate["occurrence"].as_u64() == Some(u64::from(occurrence))
                            && candidate["repeat_pass"].as_u64() == Some(u64::from(pass))
                            && candidate["scope"] == *scope
                            && source_on_segment(candidate, source)
                        {
                            covered = true;
                            break;
                        }
                    }
                    if !covered {
                        return Err(
                            "source-resolved intensity segment is missing from the target report"
                                .into(),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_controllers(
        &self,
        span: &crate::engine::performance::PerformanceReference,
        contributors: &BTreeSet<&str>,
        evidence_sources: &BTreeSet<&str>,
        count: &mut LedgerValidationBudget,
    ) -> Result<bool, String> {
        let mut supported = false;
        for source in contributors {
            count.visit(
                1 + usize::BITS as usize - self.controllers.len().max(1).leading_zeros() as usize,
            )?;
            if let Some(controller) = self.controllers.get(source) {
                if span.note_ids.is_empty() || controller.tick > span.start_tick {
                    return Err(
                        "intensity controller contributor has unrelated ownership or time".into(),
                    );
                }
                for id in &span.note_ids {
                    count.visit(
                        1 + usize::BITS as usize - self.notes.len().max(1).leading_zeros() as usize,
                    )?;
                    if self.notes.get(id.as_str()).and_then(|n| n.midi_channel)
                        != Some(controller.owner)
                    {
                        return Err(
                            "intensity controller contributor has unrelated ownership or time"
                                .into(),
                        );
                    }
                }
                supported = true;
            } else if !evidence_sources.contains(source) {
                return Err("intensity backlink has no authenticated contributor role".into());
            }
        }
        Ok(supported)
    }
}

/// Build bounded source identities before copying the eligible projection table.
/// Physical MIDI order determines ports, exactly as in the source event model;
/// neither source tracks nor note membership are recovered by parsing IDs.
fn build_intensity_context(
    midi: &Midi,
    projection: &ProjectionEvidence,
) -> Result<IntensityContext, String> {
    use crate::engine::target::performance::Budget;
    if midi.ticks_per_beat == 0
        || midi.time_base != midi::TimeBase::PulsesPerQuarter(midi.ticks_per_beat)
    {
        return Err("invalid intensity source PPQ".into());
    }
    let mut budget = Budget::default();
    let mut context = IntensityContext {
        source_ppq: midi.ticks_per_beat,
        source_tracks: Vec::new(),
        source_notes: Vec::new(),
        controllers: Vec::new(),
        projected_notes: Vec::new(),
        score_owners: Vec::new(),
        declarations: Vec::new(),
        dependencies: Vec::new(),
    };
    let native_midi = matches!(
        midi.source_format,
        midi::SourceFormat::StandardMidi | midi::SourceFormat::KaraokeMidi
    );
    let event_count = midi
        .tracks
        .iter()
        .fold(0usize, |n, track| n.saturating_add(track.events.len()));
    budget
        .work(event_count.saturating_mul(
            1 + usize::BITS as usize - event_count.max(1).leading_zeros() as usize,
        ))?;
    budget.text(event_count.saturating_mul(std::mem::size_of::<(&midi::Track, &midi::Event)>()))?;
    let mut events = Vec::with_capacity(event_count);
    for track in &midi.tracks {
        budget.references(1)?;
        budget.text(track.id.len().saturating_add(32))?;
        context.source_tracks.push(track.id.clone());
        events.extend(track.events.iter().map(|event| (track, event)));
    }
    events.sort_by_key(|(track, event)| (track.source.source_track, event.tick, event.order));
    let mut physical = None;
    let mut port = 0;
    for (track, event) in events {
        if physical != Some(track.source.source_track) {
            physical = Some(track.source.source_track);
            port = 0;
        }
        match &event.kind {
            Kind::Port(value) if native_midi => port = *value,
            Kind::NoteOn(note) if note.velocity != Some(0) => {
                budget.references(4)?;
                budget.text(
                    track
                        .id
                        .len()
                        .saturating_mul(4)
                        .saturating_add(note.source.id.len().saturating_mul(2))
                        .saturating_add(320),
                )?;
                context.source_notes.push(IntensitySourceNote {
                    note_id: note_instance_id(&track.id, &note.source, event.order),
                    source_track_id: track.id.clone(),
                    midi_channel: native_midi
                        .then_some(note.channel)
                        .flatten()
                        .map(|channel| IntensityMidiChannel { port, channel }),
                    explicit_attack: native_midi && note.velocity.is_some(),
                    velocity: native_midi.then_some(note.velocity).flatten(),
                    attack_source_id: (native_midi && note.velocity.is_some()).then(|| {
                        crate::engine::performance::attack_field_id(&track.id, event.order)
                    }),
                    original_source_id: note.source.id.clone(),
                    score_owner: if native_midi {
                        None
                    } else {
                        Some(IntensityScoreOwner::from_source(&note.source, &mut budget)?)
                    },
                    start_tick: event.tick,
                    source_occurrence: note.source.occurrence,
                    merged_velocity_sources: Vec::new(),
                });
            }
            Kind::ControlChange {
                channel,
                controller,
                value,
            } if native_midi && matches!(controller, 7 | 11) => {
                budget.references(2)?;
                budget.text(track.id.len().saturating_mul(2).saturating_add(160))?;
                context.controllers.push(IntensityControllerSource {
                    source_id: format!("event:{}:{}", track.id, event.order),
                    source_track_id: track.id.clone(),
                    tick: event.tick,
                    owner: IntensityMidiChannel {
                        port,
                        channel: *channel,
                    },
                    controller: *controller,
                    value: *value,
                });
            }
            _ => {}
        }
    }
    build_score_declaration_context(midi, &mut context, &mut budget)?;
    build_intensity_dependencies(midi, &mut context, &mut budget)?;
    for owner in &projection.intensity_note_owners {
        budget.references(4)?;
        budget.text(
            owner
                .note_id
                .len()
                .saturating_add(owner.source_track_id.len())
                .saturating_add(owner.destination_track_id.len())
                .saturating_add(
                    owner
                        .intensity_attack_note_id
                        .as_ref()
                        .map_or(0, String::len),
                )
                .saturating_add(192),
        )?;
    }
    // Include the existing performance table in the persistent evidence limit
    // before cloning the projection ownership records.
    let mut counter = IntensityJsonCounter { bytes: 0 };
    serde_json::to_writer(&mut counter, &context).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut counter, &projection.intensity_note_owners)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut counter, &projection.performance_spans)
        .map_err(|e| e.to_string())?;
    let mut count = context
        .reference_count()
        .saturating_add(projection.intensity_note_owners.len().saturating_mul(4));
    intensity_charge(&mut count, projection.performance_spans.len())?;
    for span in &projection.performance_spans {
        intensity_charge(&mut count, span.note_ids.len())?;
    }
    for ids in projection.performance_refs.values() {
        intensity_charge(&mut count, ids.len())?;
    }
    context.projected_notes = projection.intensity_note_owners.clone();
    Ok(context)
}

impl IntensityScoreOwner {
    fn from_source(
        source: &midi::NoteSource,
        budget: &mut crate::engine::target::performance::Budget,
    ) -> Result<Self, String> {
        Self::copy_fields(
            source.part_id.as_deref().unwrap_or(""),
            source.staff_id.as_deref().unwrap_or(""),
            source.voice.as_deref().unwrap_or(""),
            source.instrument_id.as_deref(),
            budget,
        )
    }
    fn copy_fields(
        part: &str,
        staff: &str,
        voice: &str,
        instrument: Option<&str>,
        budget: &mut crate::engine::target::performance::Budget,
    ) -> Result<Self, String> {
        budget.text(
            part.len()
                .saturating_add(staff.len())
                .saturating_add(voice.len())
                .saturating_add(instrument.map_or(0, str::len))
                .saturating_add(128),
        )?;
        Ok(Self {
            part: part.into(),
            staff: staff.into(),
            voice: voice.into(),
            instrument: instrument.map(str::to_owned),
        })
    }
    fn source_voice(
        &self,
        budget: &mut crate::engine::target::performance::Budget,
    ) -> Result<crate::engine::score_intensity::ScoreVoice, String> {
        let copy = Self::copy_fields(
            &self.part,
            &self.staff,
            &self.voice,
            self.instrument.as_deref(),
            budget,
        )?;
        Ok(crate::engine::score_intensity::ScoreVoice {
            part: copy.part,
            staff: copy.staff,
            voice: copy.voice,
            instrument: copy.instrument,
        })
    }
}

fn score_scope_applies(scope: &serde_json::Value, owner: &IntensityScoreOwner) -> bool {
    if scope == "System" {
        return true;
    }
    if let Some(part) = scope.get("Part") {
        return part.as_str() == Some(owner.part.as_str());
    }
    if let Some(fields) = scope.get("Instrument") {
        return fields["part"].as_str() == Some(owner.part.as_str())
            && (fields["instrument"].is_null()
                || fields["instrument"].as_str() == owner.instrument.as_deref());
    }
    if let Some(fields) = scope.get("Staff") {
        return fields["part"].as_str() == Some(owner.part.as_str())
            && fields["staff"].as_str() == Some(owner.staff.as_str());
    }
    if let Some(fields) = scope.get("Voice") {
        return fields["part"].as_str() == Some(owner.part.as_str())
            && fields["voice"].as_str() == Some(owner.voice.as_str())
            && (fields["staff"].is_null()
                || fields["staff"].as_str() == Some(owner.staff.as_str()));
    }
    scope
        .get("Unsupported")
        .is_some_and(|fields| fields["part"].as_str() == Some(owner.part.as_str()))
}

#[allow(clippy::too_many_arguments)]
fn push_intensity_dependency(
    input: &crate::engine::score_intensity::source::ScoreInput,
    owner: &crate::engine::score_intensity::ScoreVoice,
    provenance: &crate::engine::score_intensity::Provenance,
    start: crate::engine::score_intensity::Time,
    end: crate::engine::score_intensity::Time,
    segment: bool,
    attack_note_id: Option<&str>,
    context: &mut IntensityContext,
    budget: &mut crate::engine::target::performance::Budget,
) -> Result<(), String> {
    // Persist every source-resolved score declaration that actually contributes
    // to this interval/attack. This authenticates held declarations after a
    // superseding dynamic as well as legitimate future endpoint dependencies.
    // Non-declaration witnesses (for example raw MIDI velocity evidence) retain
    // their existing independent ownership tables and are intentionally omitted.
    let source_ids: BTreeSet<_> = provenance
        .evidence
        .iter()
        .flat_map(|e| &e.source_ids)
        .filter(|id| input.declarations.contains_key(id.as_str()))
        .cloned()
        .collect();
    if source_ids.is_empty() {
        return Ok(());
    }
    let source_count = source_ids.len();
    budget.references(source_count.saturating_add(4))?;
    budget.work(
        source_count.saturating_mul(
            1 + usize::BITS as usize - source_count.max(1).leading_zeros() as usize,
        ),
    )?;
    for id in &source_ids {
        budget.text(id.len().saturating_add(96))?;
    }
    budget.text(attack_note_id.map_or(0, str::len).saturating_add(320))?;
    context.dependencies.push(IntensityDependencySource {
        owner: IntensityScoreOwner::copy_fields(
            &owner.part,
            &owner.staff,
            &owner.voice,
            owner.instrument.as_deref(),
            budget,
        )?,
        scope: budget.json(&provenance.scope)?,
        occurrence: provenance.occurrence,
        repeat_pass: provenance.repeat_pass,
        start: budget.json(&start)?,
        end: budget.json(&end)?,
        segment,
        attack_note_id: attack_note_id.map(str::to_owned),
        source_ids: source_ids.into_iter().collect(),
    });
    Ok(())
}

fn push_intensity_timeline_dependencies(
    input: &crate::engine::score_intensity::source::ScoreInput,
    owner: &crate::engine::score_intensity::ScoreVoice,
    timeline: &crate::engine::score_intensity::Timeline,
    context: &mut IntensityContext,
    budget: &mut crate::engine::target::performance::Budget,
) -> Result<(), String> {
    budget.work(
        timeline
            .segments
            .len()
            .saturating_add(timeline.issues.len()),
    )?;
    for segment in &timeline.segments {
        if let Some(provenance) = &segment.provenance {
            push_intensity_dependency(
                input,
                owner,
                provenance,
                segment.start,
                segment.end,
                true,
                None,
                context,
                budget,
            )?;
        }
    }
    for issue in &timeline.issues {
        push_intensity_dependency(
            input,
            owner,
            &issue.provenance,
            issue.start,
            issue.end,
            false,
            None,
            context,
            budget,
        )?;
    }
    Ok(())
}

fn build_intensity_dependencies(
    midi: &Midi,
    context: &mut IntensityContext,
    budget: &mut crate::engine::target::performance::Budget,
) -> Result<(), String> {
    use crate::engine::performance::{normalize, PerformanceOwner};
    let Some(input) = &midi.score_intensity else {
        return Ok(());
    };
    // This one independent normalization uses the existing event and cumulative
    // provenance limits. It does not replay the resolver per report/contributor,
    // and never reads assertions supplied by the target. Shared source timelines
    // are traversed once per owner, including declarations on source-only lanes.
    let source = normalize(midi)?;
    budget.work(
        source
            .bindings
            .len()
            .saturating_add(source.score_issues.len()),
    )?;
    let mut seen = BTreeSet::new();
    for original in &source.score_issues {
        let owner = &original.voice;
        budget.text(96)?;
        if seen.insert((
            std::sync::Arc::as_ptr(&original.timeline) as usize,
            owner.part.as_str(),
            owner.staff.as_str(),
            owner.voice.as_str(),
            owner.instrument.as_deref(),
        )) {
            push_intensity_timeline_dependencies(
                input,
                owner,
                &original.timeline,
                context,
                budget,
            )?;
        }
    }
    for binding in source.bindings.values() {
        let PerformanceOwner::Score { voice: owner } = &binding.owner else {
            continue;
        };
        let Some(intensity) = &binding.intensity else {
            continue;
        };
        budget.text(96)?;
        if seen.insert((
            std::sync::Arc::as_ptr(&intensity.timeline) as usize,
            owner.part.as_str(),
            owner.staff.as_str(),
            owner.voice.as_str(),
            owner.instrument.as_deref(),
        )) {
            push_intensity_timeline_dependencies(
                input,
                owner,
                &intensity.timeline,
                context,
                budget,
            )?;
        }
        if let Some(provenance) = &intensity.provenance {
            push_intensity_dependency(
                input,
                owner,
                provenance,
                intensity.start,
                intensity.start,
                false,
                Some(&binding.source_id),
                context,
                budget,
            )?;
        }
        budget.work(intensity.issues.len())?;
        for issue in &intensity.issues {
            push_intensity_dependency(
                input,
                owner,
                &issue.provenance,
                issue.start,
                issue.end,
                false,
                None,
                context,
                budget,
            )?;
        }
    }
    Ok(())
}

fn build_score_declaration_context(
    midi: &Midi,
    context: &mut IntensityContext,
    budget: &mut crate::engine::target::performance::Budget,
) -> Result<(), String> {
    use crate::engine::score_intensity::Scope;
    let Some(input) = &midi.score_intensity else {
        return Ok(());
    };
    let mut owners = BTreeMap::<&str, BTreeSet<IntensityScoreOwner>>::new();
    for note in &context.source_notes {
        budget.work(1)?;
        if let Some(owner) = &note.score_owner {
            let copy = IntensityScoreOwner::copy_fields(
                &owner.part,
                &owner.staff,
                &owner.voice,
                owner.instrument.as_deref(),
                budget,
            )?;
            budget.references(1)?;
            owners
                .entry(&note.source_track_id)
                .or_default()
                .insert(copy);
        }
    }
    // Typed metadata lanes cover source-only declarations even without notes.
    // Track identities remain the original adapter identities.
    budget.work(midi.tracks.len())?;
    for track in &midi.tracks {
        if owners.contains_key(track.id.as_str()) {
            continue;
        }
        let (Some(part), Some(staff)) = (&track.source.part_id, &track.source.staff_id) else {
            continue;
        };
        if let Some(voice) = &track.source.voice {
            let owner = IntensityScoreOwner::copy_fields(
                part,
                staff,
                voice,
                track.instrument.as_ref().and_then(|i| i.id.as_deref()),
                budget,
            )?;
            budget.references(1)?;
            owners.entry(&track.id).or_default().insert(owner);
        } else {
            for source_part in &midi.topology.parts {
                budget.work(1)?;
                if source_part.id != *part {
                    continue;
                }
                for source_staff in &source_part.staves {
                    budget.work(1)?;
                    if source_staff.id != *staff {
                        continue;
                    }
                    if source_staff.voices.is_empty() {
                        let owner = IntensityScoreOwner::copy_fields(
                            part,
                            staff,
                            "",
                            track.instrument.as_ref().and_then(|i| i.id.as_deref()),
                            budget,
                        )?;
                        budget.references(1)?;
                        owners.entry(&track.id).or_default().insert(owner);
                    }
                    for voice in &source_staff.voices {
                        budget.work(1)?;
                        let owner = IntensityScoreOwner::copy_fields(
                            part,
                            staff,
                            &voice.number,
                            track.instrument.as_ref().and_then(|i| i.id.as_deref()),
                            budget,
                        )?;
                        budget.references(1)?;
                        owners.entry(&track.id).or_default().insert(owner);
                    }
                }
            }
        }
    }
    let mut unique = BTreeSet::new();
    for (track, voices) in owners {
        for owner in voices {
            budget.references(2)?;
            budget.text(track.len())?;
            unique.insert(IntensityScoreOwner::copy_fields(
                &owner.part,
                &owner.staff,
                &owner.voice,
                owner.instrument.as_deref(),
                budget,
            )?);
            context.score_owners.push(IntensityScoreTrack {
                source_track_id: track.into(),
                owner,
            });
        }
    }
    // Preserve nominally absorbed tie-tail velocity identities from parser ties.
    let mut merged = BTreeMap::<(&str, u32), Vec<&str>>::new();
    budget.work(input.ties.len())?;
    for ((tail, occurrence, _), head) in &input.ties {
        budget.references(1)?;
        budget.text(96)?;
        merged.entry((head, *occurrence)).or_default().push(tail);
    }
    for note in &mut context.source_notes {
        budget.work(1)?;
        if let Some(tails) = merged.get(&(note.original_source_id.as_str(), note.source_occurrence))
        {
            for id in tails {
                budget.references(1)?;
                budget.text(id.len().saturating_add(24))?;
                note.merged_velocity_sources.push((*id).into());
            }
        }
    }
    budget.work(input.retained.len())?;
    for id in input.retained.keys() {
        let declaration = input
            .declarations
            .get(id)
            .ok_or("missing original intensity declaration")?;
        let original = input
            .original_declarations
            .get(id)
            .ok_or("missing original intensity declaration kind")?;
        if original.kinds.is_empty() {
            return Err("missing original intensity declaration kind".into());
        }
        budget.references(2)?;
        budget.text(
            id.len()
                .saturating_add(original.note_source_id.as_ref().map_or(0, String::len))
                .saturating_add(192),
        )?;
        let mut row = IntensityDeclarationSource {
            source_id: id.clone(),
            kinds: budget.json(&original.kinds)?,
            scope: budget.json(&declaration.scope)?,
            at: budget.json(&declaration.at)?,
            note_source_id: original.note_source_id.clone(),
            paired_source_ids: Vec::new(),
            applications: Vec::new(),
        };
        if let Some(pairs) = input.paired_declarations.get(id) {
            budget.work(pairs.len())?;
            for paired in pairs {
                budget.references(1)?;
                budget.text(paired.len().saturating_add(24))?;
                row.paired_source_ids.push(paired.clone());
            }
        }
        for owner in &unique {
            budget.work(1)?;
            let voice = owner.source_voice(budget)?;
            if !declaration.scope.applies(&voice)
                && !matches!(&declaration.scope, Scope::Unsupported { part, .. } if part == &voice.part)
            {
                continue;
            }
            let runs = input.owner_runs(&voice).unwrap_or(&[]);
            budget.work(
                runs.len()
                    .saturating_add(declaration.time_only.len())
                    .saturating_add(1),
            )?;
            if input.declaration_is_unresolved(id, &voice) {
                budget.references(1)?;
                row.applications.push(IntensityDeclarationApplication {
                    owner: IntensityScoreOwner::copy_fields(
                        &owner.part,
                        &owner.staff,
                        &owner.voice,
                        owner.instrument.as_deref(),
                        budget,
                    )?,
                    occurrence: 0,
                    repeat_pass: 0,
                    start: budget.json(&declaration.at)?,
                    end: budget.json(&declaration.at)?,
                    written_start: budget.json(&declaration.at)?,
                    active: false,
                });
            } else {
                for run in runs {
                    budget.work(
                        runs.len()
                            .saturating_add(declaration.time_only.len())
                            .saturating_add(1),
                    )?;
                    let Some(pass) =
                        input.declaration_route_pass(id, &voice, run.ordinal, run.pass)
                    else {
                        continue;
                    };
                    let end = run
                        .performed_start
                        .checked_add(run.written_end.checked_sub(run.written_start)?)?;
                    budget.references(1)?;
                    row.applications.push(IntensityDeclarationApplication {
                        owner: IntensityScoreOwner::copy_fields(
                            &owner.part,
                            &owner.staff,
                            &owner.voice,
                            owner.instrument.as_deref(),
                            budget,
                        )?,
                        occurrence: run.ordinal,
                        repeat_pass: run.pass,
                        start: budget.json(&run.performed_start)?,
                        end: budget.json(&end)?,
                        written_start: budget.json(&run.written_start)?,
                        active: declaration.enabled
                            && pass > 0
                            && (declaration.time_only.is_empty()
                                || declaration.time_only.contains(&pass)),
                    });
                }
            }
        }
        // Native/score source notes themselves retain their exact tick position;
        // declaration coordinates remain exact quarter fractions in this table.
        context.declarations.push(row);
    }
    Ok(())
}

fn intensity_fraction(
    value: &serde_json::Value,
) -> Result<crate::engine::score_intensity::Fraction, String> {
    let integer = |key: &str| -> Option<i128> {
        let value = value.get(key)?;
        value.as_i64().map(i128::from).or_else(|| {
            let s = value.as_str()?;
            // Decimal integer strings are the serializer's exact i128 escape.
            let digits = s.strip_prefix('-').unwrap_or(s);
            (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse().ok())
                .flatten()
        })
    };
    let (Some(n), Some(d)) = (integer("numerator"), integer("denominator")) else {
        return Err("invalid exact intensity fraction".into());
    };
    crate::engine::score_intensity::Fraction::wide(n, d)
}

fn intensity_bounds(
    value: &serde_json::Value,
) -> Result<
    (
        crate::engine::score_intensity::Time,
        crate::engine::score_intensity::Time,
    ),
    String,
> {
    let start = intensity_fraction(&value["start"])?;
    let end = intensity_fraction(&value["end"])?;
    if start < crate::engine::score_intensity::Fraction::ZERO || end < start {
        return Err("invalid exact intensity bounds".into());
    }
    Ok((start, end))
}

fn intensity_scope(value: &serde_json::Value) -> bool {
    use serde_json::Value;
    let string = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty());
    let optional = |v: &Value| v.is_null() || string(v);
    if value == "System" {
        return true;
    }
    let Some(object) = value.as_object().filter(|o| o.len() == 1) else {
        return false;
    };
    let (kind, fields) = object.iter().next().unwrap();
    match kind.as_str() {
        "Part" => string(fields),
        "Midi" => {
            fields["port"].as_u64().is_some_and(|n| n <= 255)
                && fields["channel"].as_u64().is_some_and(|n| n < 16)
        }
        "Instrument" => string(&fields["part"]) && optional(&fields["instrument"]),
        "Staff" => string(&fields["part"]) && string(&fields["staff"]),
        "Voice" => {
            string(&fields["part"]) && optional(&fields["staff"]) && string(&fields["voice"])
        }
        "Unsupported" => string(&fields["part"]) && fields["raw"].is_string(),
        _ => false,
    }
}

fn intensity_curve(value: &serde_json::Value) -> Result<(), String> {
    use crate::engine::score_intensity::Fraction;
    let level = |v: &serde_json::Value| -> Result<(), String> {
        if v == "Silence" {
            return Ok(());
        }
        if v.as_object()
            .is_none_or(|o| o.len() != 1 || !o.contains_key("Positive"))
        {
            return Err("invalid intensity level".into());
        }
        let n = intensity_fraction(&v["Positive"])?;
        if n <= Fraction::ZERO || n > Fraction::integer(127) {
            return Err("invalid intensity level".into());
        }
        Ok(())
    };
    if value == "Absent" {
        return Ok(());
    }
    let object = value
        .as_object()
        .filter(|o| o.len() == 1)
        .ok_or("invalid intensity curve")?;
    let (kind, fields) = object.iter().next().unwrap();
    match kind.as_str() {
        "Level" => level(fields),
        "Held" if fields.is_null() || fields == "Silence" => Ok(()),
        "Held"
            if fields.as_object().is_some_and(|o| o.len() == 1)
                && fields["Decibels"].as_f64().is_some_and(f64::is_finite) =>
        {
            Ok(())
        }
        "Transition" => {
            let (start, end) = intensity_bounds(fields)?;
            level(&fields["from"])?;
            level(&fields["to"])?;
            if start == end
                || !matches!(
                    fields["easing"].as_str(),
                    Some("Normal" | "EaseIn" | "EaseOut" | "EaseInOut" | "Exponential")
                )
                || !matches!(
                    fields["domain"].as_str(),
                    Some("RelativeDecibels" | "LinearGain")
                )
                || (fields["domain"] == "RelativeDecibels"
                    && (fields["from"] == "Silence" || fields["to"] == "Silence"))
            {
                return Err("invalid intensity transition".into());
            }
            Ok(())
        }
        _ => Err("invalid intensity curve".into()),
    }
}

fn intensity_provenance<'a>(
    value: &'a serde_json::Value,
    entries: &BTreeMap<&str, &DispositionEntry>,
    contributors: &BTreeSet<&str>,
    evidence_sources: &mut BTreeSet<&'a str>,
    count: &mut LedgerValidationBudget,
) -> Result<(), String> {
    use crate::engine::score_intensity::POLICY;
    if value["policy"] != POLICY
        || !intensity_scope(&value["scope"])
        || !["occurrence", "repeat_pass"]
            .iter()
            .all(|k| value[*k].as_u64().is_some_and(|n| n <= u64::from(u32::MAX)))
    {
        return Err("invalid intensity provenance policy/scope/occurrence".into());
    }
    let evidence = value["evidence"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or("missing intensity source evidence")?;
    let mut sources = BTreeSet::new();
    for e in evidence {
        if !matches!(
            e["contract"].as_str(),
            Some("MusicXml" | "MuseScoreLegacy" | "MuseScoreModern" | "Midi")
        ) || !(e["saving_version"].is_null() || e["saving_version"].is_string())
            || !e["layout"].is_string()
            || e["raw_fields"]
                .as_object()
                .is_none_or(|o| !o.values().all(serde_json::Value::is_string))
        {
            return Err("invalid intensity source evidence structure".into());
        }
        let ids = e["source_ids"]
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or("missing intensity source IDs")?;
        for id in ids {
            count.reference(1)?;
            let id = id
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("invalid intensity source ID")?;
            count.visit(
                2 * (1 + usize::BITS as usize - entries.len().max(1).leading_zeros() as usize),
            )?;
            if !entries.contains_key(id) {
                return Err("intensity source ID is not inventoried".into());
            }
            if !contributors.contains(id) {
                return Err("intensity source ID does not reference its containing span".into());
            }
            count.reserve(1, 64)?;
            sources.insert(id);
            if !evidence_sources.contains(id) {
                count.reserve(1, 64)?;
                evidence_sources.insert(id);
            }
        }
    }
    for item in value["interpretations"]
        .as_array()
        .ok_or("missing intensity interpretations")?
    {
        if !item["field"].is_string()
            || !item["explanation"].is_string()
            || !matches!(
                item["basis"].as_str(),
                Some(
                    "ExplicitNumeric"
                        | "StandardTable"
                        | "DeclaredDefault"
                        | "InferredEndpoint"
                        | "InferredSpan"
                        | "SourceArithmetic"
                        | "PortableInterpretation"
                )
            )
        {
            return Err("invalid intensity interpretation".into());
        }
        if !item["exact"].is_null() {
            intensity_fraction(&item["exact"])?;
        }
        for id in item["source_ids"]
            .as_array()
            .ok_or("missing interpretation source IDs")?
        {
            count.reference(1)?;
            if id.as_str().is_none_or(|s| !sources.contains(s)) {
                return Err("intensity interpretation refers outside its source evidence".into());
            }
        }
    }
    Ok(())
}

fn validate_intensity_extension(
    value: &serde_json::Value,
    span: &crate::engine::performance::PerformanceReference,
    entries: &BTreeMap<&str, &DispositionEntry>,
    ownership: &IntensityOwnership<'_>,
    contributors: &BTreeSet<&str>,
    count: &mut LedgerValidationBudget,
) -> Result<(), String> {
    use crate::engine::performance::{Dimension, TransferStatus};
    use crate::engine::score_intensity::POLICY;
    if value["policy"] != POLICY
        || span.dimension != Dimension::LinearGain
        || !matches!(span.target.as_str(), "ustx" | "svp")
        || !ownership.tracks.contains(span.source_track_id.as_str())
    {
        return Err("invalid intensity policy/dimension/source ownership".into());
    }
    let (start, end) = intensity_bounds(value)?;
    for id in &span.note_ids {
        if entries
            .get(id.as_str())
            .is_none_or(|e| e.item_kind != SourceItemKind::Note)
        {
            return Err("intensity note ID is not an inventoried note".into());
        }
    }
    let provenance = value
        .get("provenance")
        .ok_or("missing intensity provenance")?;
    let mut evidence_sources = BTreeSet::new();
    if let Some(items) = provenance.as_array() {
        for item in items {
            intensity_provenance(item, entries, contributors, &mut evidence_sources, count)?;
        }
    } else {
        intensity_provenance(
            provenance,
            entries,
            contributors,
            &mut evidence_sources,
            count,
        )?;
    }
    let terminal = match value.get("terminalEndpoint") {
        Some(v) => v.as_bool().ok_or("invalid intensity terminal metadata")?,
        None => false,
    };
    if terminal
        && (start != end
            || span.start_tick != span.end_tick
            || !span.note_ids.is_empty()
            || span.status == TransferStatus::Mapped)
        || (span.status == TransferStatus::Mapped && start == end)
    {
        return Err("intensity terminal metadata conflicts with ownership".into());
    }
    ownership.validate_span(span, start, end, terminal, count)?;
    if let Some(curve) = value.get("curve").filter(|c| !c.is_null()) {
        intensity_curve(curve)?;
    }
    let mut segment_provenance_index: Option<JsonFingerprintIndex<'_>> = None;
    if let Some(segments) = value.get("segments") {
        if span.target != "svp"
            || span.status != TransferStatus::Unsupported
            || !provenance.is_array()
        {
            return Err("invalid SVP intensity report".into());
        }
        let summary = provenance.as_array().unwrap();
        let segments = segments.as_array().ok_or("invalid intensity segments")?;
        count.reserve(
            summary.len(),
            std::mem::size_of::<(u64, &serde_json::Value, usize)>(),
        )?;
        let mut summary_index = JsonFingerprintIndex::new();
        for item in summary {
            let (fingerprint, work) = json_fingerprint(item, count)?;
            summary_index
                .entry(fingerprint)
                .or_default()
                .push((item, work));
        }
        count.reserve(
            segments.len(),
            std::mem::size_of::<(u64, &serde_json::Value, usize)>(),
        )?;
        let mut reported_index = JsonFingerprintIndex::new();
        let mut previous_end = None;
        for segment in segments {
            count.reference(1)?;
            let (a, b) = intensity_bounds(segment)?;
            if a == b {
                return Err("intensity segment must have positive ordinary duration".into());
            }
            if previous_end.is_some_and(|previous| a < previous) {
                return Err("intensity segments overlap or are out of order".into());
            }
            previous_end = Some(b);
            // Reports retain whole source segments which intersect the note;
            // clipping here would erase the authored transition endpoints.
            if a >= end || b <= start || !segment["attack_only"].is_boolean() {
                return Err("intensity segment has no owned interval".into());
            }
            intensity_curve(&segment["curve"])?;
            if !segment["provenance"].is_null() {
                intensity_provenance(
                    &segment["provenance"],
                    entries,
                    contributors,
                    &mut evidence_sources,
                    count,
                )?;
                let (fingerprint, work) = json_fingerprint(&segment["provenance"], count)?;
                if !json_index_contains_prehashed(
                    &summary_index,
                    &segment["provenance"],
                    fingerprint,
                    work,
                    count,
                )? {
                    return Err("intensity segment is absent from its summary provenance".into());
                }
                reported_index
                    .entry(fingerprint)
                    .or_default()
                    .push((&segment["provenance"], work));
                ownership.authenticate_provenance(
                    &segment["provenance"],
                    span,
                    a.max(start),
                    b.min(end),
                    count,
                )?;
            } else if segment["curve"] != "Absent"
                && !(segment["curve"].as_object().is_some_and(|o| {
                    o.len() == 1 && o.get("Held").is_some_and(serde_json::Value::is_null)
                }))
            {
                return Err("authored intensity segment has no source provenance".into());
            }
            if let Some(transition) = segment["curve"].get("Transition") {
                let (curve_start, curve_end) = intensity_bounds(transition)?;
                if a < curve_start || b > curve_end {
                    return Err("intensity segment exceeds its authored transition".into());
                }
            }
        }
        ownership.validate_segment_dependency_coverage(
            span,
            start,
            end,
            provenance.as_array().unwrap(),
            segments,
            count,
        )?;
        segment_provenance_index = Some(reported_index);
    } else if value.get("targetStart").is_some() || value.get("targetEnd").is_some() {
        let a = value["targetStart"]
            .as_i64()
            .ok_or("invalid intensity target bounds")?;
        let b = value["targetEnd"]
            .as_i64()
            .ok_or("invalid intensity target bounds")?;
        if span.target != "ustx"
            || span.target_track.is_none()
            || span.note_ids.is_empty()
            || !provenance.is_array()
            || a < 0
            || b < a
            || b > i64::from(i32::MAX)
            || start == end
            || value["rounding"] != "nearest, ties away from zero"
            || !value["limitedNienteTail"].is_boolean()
            || value.get("curve").is_none()
        {
            return Err("invalid USTX intensity gain report".into());
        }
        if span.status == TransferStatus::Mapped && a == b {
            return Err("mapped intensity requires a positive target interval".into());
        }
        if i64::from(crate::engine::performance::exact_target_tick(start)?) != a
            || i64::from(crate::engine::performance::exact_target_tick(end)?) != b
        {
            return Err("exact intensity bounds disagree with target endpoints".into());
        }
        if value["limitedNienteTail"] == false {
            if let Some(transition) = value["curve"].get("Transition") {
                let (curve_start, curve_end) = intensity_bounds(transition)?;
                if start < curve_start || end > curve_end {
                    return Err("intensity gain interval exceeds its authored transition".into());
                }
            }
        }
        if value["limitedNienteTail"] == true {
            let curve = &value["curve"]["Transition"];
            let (a, b) = intensity_bounds(curve)?;
            if span.status != TransferStatus::RepresentationLimit
                || curve["domain"] != "LinearGain"
                || (curve["from"] != "Silence" && curve["to"] != "Silence")
                || start < a
                || end > b
            {
                return Err("limited intensity floor has no authored niente transition".into());
            }
        }
    } else if !provenance.is_object()
        || value.get("terminalEndpoint").is_none()
        || span.status == TransferStatus::Mapped
        || (value.get("curve").is_some() && !terminal)
    {
        return Err("unsupported intensity extension structure".into());
    }
    if let Some(items) = provenance.as_array() {
        for item in items {
            if let Some(segments) = segment_provenance_index.as_ref() {
                // The SVP summary is a union, not a claim that every contributor
                // owns the whole note. Nested occurrences are authenticated on
                // their intersecting segments below. An additional summary item
                // must belong to the original attack (including inherited ties).
                let segment_owned = json_index_contains(segments, item, count)?;
                if !segment_owned {
                    let mut attack = None;
                    for id in &span.note_ids {
                        count.visit(1)?;
                        let root = ownership.attack_note(span, id, count)?;
                        let at = crate::engine::score_intensity::source::time(
                            i64::from(root.start_tick),
                            ownership.context.source_ppq,
                        )?;
                        if attack.is_some_and(|previous| previous != at) {
                            return Err("intensity summary has unrelated source attacks".into());
                        }
                        attack = Some(at);
                    }
                    let at = attack.ok_or("intensity summary has no original attack")?;
                    ownership.authenticate_provenance(item, span, at, at, count)?;
                }
            } else {
                ownership.authenticate_provenance(item, span, start, end, count)?;
            }
        }
    } else {
        ownership.authenticate_provenance(provenance, span, start, end, count)?;
    }
    // Controller ownership is independent of whether an attack/score field
    // also contributed. Same-port cross-track controllers are legitimate.
    let has_provenance = provenance.as_array().is_none_or(|items| !items.is_empty());
    if !has_provenance {
        // An empty score provenance list cannot authenticate an authored curve.
        // MIDI controller-only intervals may use this shape when another note
        // enabled the common intensity adapter. Authenticate their actual CC
        // contributors against independently inventoried physical port/channel.
        let curve = &value["curve"];
        let neutral_curve = curve.is_null()
            || curve == "Absent"
            || curve.as_object().is_some_and(|o| {
                o.len() == 1 && o.get("Held").is_some_and(serde_json::Value::is_null)
            });
        if !neutral_curve
            || value.get("segments").is_some()
            || span
                .note_ids
                .iter()
                .any(|id| ownership.notes[id.as_str()].explicit_attack)
        {
            return Err(
                "intensity has no score provenance or inventoried controller support".into(),
            );
        }
    }
    let controller_support =
        ownership.validate_controllers(span, contributors, &evidence_sources, count)?;
    if !has_provenance && !controller_support {
        return Err("intensity has no score provenance or inventoried controller support".into());
    }
    Ok(())
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
                _ if projection.performance_mapped.contains_key(&event_id) => (
                    PrimaryDisposition::ProjectedMapped {
                        policy: projection.performance_mapped[&event_id].clone(),
                        limitations: projection.performance_unmapped.get(&event_id).cloned(),
                    },
                    true,
                    stem.is_some(),
                ),
                _ if projection.performance_unmapped.contains_key(&event_id) => (
                    PrimaryDisposition::SourceOnly {
                        reason: projection.performance_unmapped[&event_id].clone(),
                    },
                    false,
                    stem.is_some(),
                ),
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
                    if matches!(
                        midi.source_format,
                        midi::SourceFormat::StandardMidi | midi::SourceFormat::KaraokeMidi
                    ) && note.velocity.is_some()
                    {
                        let id =
                            crate::engine::performance::attack_field_id(&track.id, event.order);
                        let (disposition, mapped) = if let Some(policy) =
                            projection.performance_mapped.get(&id)
                        {
                            (
                                PrimaryDisposition::ProjectedMapped {
                                    policy: policy.clone(),
                                    limitations: projection.performance_unmapped.get(&id).cloned(),
                                },
                                true,
                            )
                        } else {
                            (PrimaryDisposition::SourceOnly {reason:projection.performance_unmapped.get(&id).cloned().unwrap_or_else(||"Explicit attack field is retained without an editable target mapping".into())},false)
                        };
                        push_entry(
                            &mut entries,
                            id,
                            SourceItemKind::Event,
                            disposition,
                            artifact_paths(mapped, None, layout),
                        );
                    }
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
    for link in &midi.staff_links {
        push_entry(
            &mut entries,
            link.id.clone(),
            SourceItemKind::Event,
            PrimaryDisposition::MetadataOnly,
            artifact_paths(false, None, layout),
        );
    }
    if let Some(input) = &midi.score_intensity {
        for id in input.retained.keys() {
            let (disposition, mapped) = if let Some(policy) = projection.performance_mapped.get(id)
            {
                (
                    PrimaryDisposition::ProjectedMapped {
                        policy: policy.clone(),
                        limitations: projection.performance_unmapped.get(id).cloned(),
                    },
                    true,
                )
            } else {
                (PrimaryDisposition::SourceOnly { reason:projection.performance_unmapped.get(id).cloned().unwrap_or_else(||"Retained authored score expression without an eligible editable target span".into()) },false)
            };
            push_entry(
                &mut entries,
                id.clone(),
                SourceItemKind::Event,
                disposition,
                artifact_paths(mapped, None, layout),
            );
        }
    }
    for entry in &mut entries {
        entry.performance_refs = projection
            .performance_refs
            .get(&entry.source_id)
            .cloned()
            .unwrap_or_default();
    }
    entries.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    // One source lyric can be reached by more than one projection lane: a chord
    // carrying a lyric is split into several monophonic lanes, and each lane
    // inventories the same `lyric:…:note-event:…` instance. That is genuinely the
    // same source item seen twice, not two items, so identical entries collapse.
    // Two entries that share an ID but disagree on disposition are still a real
    // conflict and are left for `validate` to reject.
    entries.dedup_by(|later, earlier| {
        later.source_id == earlier.source_id
            && later.item_kind == earlier.item_kind
            && later.disposition == earlier.disposition
            && later.artifact_paths == earlier.artifact_paths
    });
    let expected_source_ids = entries
        .iter()
        .map(|entry| entry.source_id.clone())
        .collect();
    PreservationLedger {
        intensity_context: if projection
            .performance_spans
            .iter()
            .any(|span| span.intensity.is_some())
        {
            // The public builder is infallible; a failed bounded preflight leaves
            // required context absent, so validation refuses publication.
            build_intensity_context(midi, projection).ok()
        } else {
            None
        },
        schema_version: if projection.performance_spans.is_empty() {
            SCHEMA_VERSION
        } else {
            PERFORMANCE_LEDGER_SCHEMA_VERSION
        },
        performance_spans: projection.performance_spans.clone(),
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
        performance_refs: Vec::new(),
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
    let layout = BundleLayout::new(
        &request.destination,
        &request.input.original_name,
        request.input.project.target(),
    )?;
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
        let group_id = request.input.project.append_audio_reference(
            format!("{} ({stem_origin})", stem.descriptor.display_name),
            project_audio_reference(&stem.relative_path),
            &stem.wav,
            !stem.descriptor.active_by_default,
        )?;
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
    // A Part that owns an editable vocal projection has its own stem muted, so it
    // cannot double the singer. When *every* Part owns one — which is the normal
    // shape of a MIDI whose every track carries lyrics — that leaves no audible
    // audio at all, and the user opens a bundle that promises an audible reference
    // mix and plays silence until they unmute something by hand. So the reference
    // becomes the fallback: muted whenever some accompaniment stem is already
    // audible, active when none is. This is initial mixer state, not source
    // evidence; the audio and the ledger are identical either way.
    let has_audible_stem = rendered_stems
        .iter()
        .any(|stem| stem.descriptor.active_by_default);
    let reference_group_id = request.input.project.append_audio_reference(
        "Full score reference mix (MuseScore)".into(),
        project_audio_reference(AUDIO_RELATIVE_PATH),
        &reference_wav,
        has_audible_stem,
    )?;
    let project_path = safe_join(&root, &layout.project_relative_path)?;
    let project_bytes = request.input.project.to_bytes()?;
    write_new(&project_path, &project_bytes, write_project_phase(&layout))?;
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
    // Named after the shape the project actually holds. The Synthesizer V sentence
    // is 0.4.9's, verbatim, because it is part of a manifest that must not change.
    warnings.push(match layout.target {
        ExportTarget::Svp =>
            "The full-score reference mix is retained muted; source Parts are rendered as separate audio-backed SVP tracks.".into(),
        ExportTarget::Ustx =>
            "The full-score reference mix is retained muted; source Parts are rendered as separate audio-backed OpenUtau wave parts.".to_string(),
    });
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
                muted_by_default: has_audible_stem,
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

/// The I/O phase names, per target, so a write or reopen failure still names the
/// project the bundle actually holds. The Synthesizer V strings are 0.4.9's.
fn write_project_phase(layout: &BundleLayout) -> &'static str {
    match layout.target {
        ExportTarget::Svp => "write Synthesizer V project",
        ExportTarget::Ustx => "write OpenUtau project",
    }
}

fn reopen_project_phase(layout: &BundleLayout) -> &'static str {
    match layout.target {
        ExportTarget::Svp => "reopen Synthesizer V project",
        ExportTarget::Ustx => "reopen OpenUtau project",
    }
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

/// Where each source Part sits in `parts`, in source Part order, or `None`
/// when the containers do not say which Part they hold.
///
/// `--score-parts` returns the excerpts already saved in the score alongside
/// the one-per-instrument parts it can cut, so a score whose author saved a
/// two-instrument part comes back with a container that is not any single
/// source Part and must never be rendered as one. MuseScore stamps each Part it
/// writes with the id that Part has in the score, which both says which Part a
/// container holds and orders the containers the way the score does.
fn single_source_part_container_order(parts: &[ExtractedScorePart]) -> Option<Vec<usize>> {
    let mut kept: Vec<(u64, usize)> = Vec::with_capacity(parts.len());
    let mut seen = BTreeSet::new();
    for (index, part) in parts.iter().enumerate() {
        let ids = crate::engine::musescore::container_part_ids(&part.mscz)?;
        match ids.as_slice() {
            [Some(id)] => {
                let order = id.parse::<u64>().ok()?;
                if seen.insert(order) {
                    kept.push((order, index));
                }
            }
            // A Part written without an id names nothing, so no container can
            // be placed and the whole answer is unusable.
            [None] => return None,
            // Zero Parts, or several: not one source Part.
            _ => {}
        }
    }
    kept.sort_by_key(|(order, _)| *order);
    Some(kept.into_iter().map(|(_, index)| index).collect())
}

fn align_extracted_parts(
    _source_format: &str,
    stems: &[StemDescriptor],
    parts: Vec<ExtractedScorePart>,
) -> Result<Vec<ExtractedScorePart>, BundleError> {
    let mut available = vec![None; parts.len()];
    for part in parts {
        let ordinal = part.ordinal;
        if ordinal >= available.len() || available[ordinal].replace(part).is_some() {
            return Err(BundleError::Integrity(
                "MuseScore returned duplicated or out-of-range Part ordinals".into(),
            ));
        }
    }
    let ordered = available
        .into_iter()
        .enumerate()
        .map(|(ordinal, part)| {
            part.ok_or_else(|| {
                BundleError::Integrity(format!(
                    "MuseScore returned no Part at source ordinal {ordinal}"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let selection = single_source_part_container_order(&ordered);
    let mut slots: Vec<Option<ExtractedScorePart>> = ordered.into_iter().map(Some).collect();
    let Some(selection) = selection else {
        // Containers that do not name their Part: the count is the only evidence
        // left that each one is the Part at its own source ordinal.
        if slots.len() != stems.len() {
            return Err(topology_count_mismatch(slots.len(), stems.len()));
        }
        return Ok(slots.into_iter().flatten().collect());
    };
    if selection.len() < stems.len() {
        return Err(topology_count_mismatch(selection.len(), stems.len()));
    }
    stems
        .iter()
        .map(|stem| {
            selection
                .get(stem.source_part_index)
                .and_then(|index| slots[*index].take())
                .ok_or_else(|| {
                    BundleError::Integrity(format!(
                        "MuseScore extracted no Part for source Part {} of stem {}",
                        stem.source_part_index + 1,
                        stem.stem_id
                    ))
                })
        })
        .collect()
}

fn topology_count_mismatch(extracted: usize, required: usize) -> BundleError {
    BundleError::Integrity(format!(
        "MuseScore extracted {extracted} Parts but the source topology requires {required}"
    ))
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
    // The reference is muted whenever an accompaniment stem is already audible, and
    // active when none is, so a bundle can never open silent. What must never
    // happen is *nothing* audible: that would deliver a promise of an audible
    // reference mix as silence.
    let audible_stems = manifest
        .audio
        .stems
        .iter()
        .filter(|stem| stem.active_by_default)
        .count();
    if manifest.audio.reference_mix.muted_by_default && audible_stems == 0 {
        return Err(BundleError::Integrity(
            "no audio starts audible: the full-score reference mix must be active when every \
             Part stem is muted"
                .into(),
        ));
    }
    if !manifest.audio.reference_mix.muted_by_default && audible_stems > 0 {
        return Err(BundleError::Integrity(
            "the full-score reference mix must be muted when a Part stem is already audible".into(),
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

    // The manifest names the project file it recorded; the extension it names must
    // be the one this transaction wrote, or the invariant applied below would be
    // the wrong format's.
    let project_path = safe_join(root, &manifest.project.path)?;
    if Path::new(&manifest.project.path)
        .extension()
        .and_then(|value| value.to_str())
        != Some(layout.target.extension())
    {
        return Err(BundleError::Integrity(format!(
            "the manifest records {} where this bundle holds a {} project",
            manifest.project.path,
            layout.target.display_name()
        )));
    }
    let project_bytes = fs::read(&project_path).map_err(|source| BundleError::Io {
        phase: reopen_project_phase(layout),
        source,
    })?;
    let canonical_root = fs::canonicalize(root).map_err(|source| BundleError::Io {
        phase: "resolve bundle root",
        source,
    })?;
    // One audio invariant per target, each checking the same four things about the
    // same WAVs: exactly one reference per artifact, the mute state the manifest
    // recorded, a reference that resolves inside the bundle, and equality with the
    // manifest's own path.
    match layout.target {
        ExportTarget::Svp => verify_svp_audio(
            root,
            &canonical_root,
            &project_path,
            &project_bytes,
            &manifest,
        )?,
        ExportTarget::Ustx => verify_ustx_audio(
            root,
            &canonical_root,
            &project_path,
            &project_bytes,
            &manifest,
        )?,
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

/// Every audio reference a Synthesizer V bundle must hold: the muted full-score
/// reference and one instrumental track per stem.
fn verify_svp_audio(
    root: &Path,
    canonical_root: &Path,
    project_path: &Path,
    project_bytes: &[u8],
    manifest: &BundleManifest,
) -> Result<(), BundleError> {
    let project: serde_json::Value = serde_json::from_slice(project_bytes)?;
    let tracks = project["tracks"]
        .as_array()
        .ok_or_else(|| BundleError::Integrity("SVP has no tracks array".into()))?;
    verify_svp_audio_track(
        root,
        canonical_root,
        project_path,
        tracks,
        &project_audio_reference(&manifest.audio.reference_mix.asset.artifact.path),
        &manifest.audio.reference_mix.asset.artifact.path,
        &manifest.audio.reference_mix.svp_group_id,
        manifest.audio.reference_mix.muted_by_default,
    )?;
    for stem in &manifest.audio.stems {
        verify_svp_audio_track(
            root,
            canonical_root,
            project_path,
            tracks,
            &project_audio_reference(&stem.asset.artifact.path),
            &stem.asset.artifact.path,
            &stem.svp_group_id,
            !stem.active_by_default,
        )?;
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
    verify_project_audio_reference(
        root,
        canonical_root,
        project_path,
        project_reference,
        artifact_path,
        "SVP",
    )
}

/// Every audio reference an OpenUtau bundle must hold: the muted full-score
/// reference and one wave part per stem, on the same WAVs and the same relative
/// paths the Synthesizer V bundle uses.
fn verify_ustx_audio(
    root: &Path,
    canonical_root: &Path,
    project_path: &Path,
    project_bytes: &[u8],
    manifest: &BundleManifest,
) -> Result<(), BundleError> {
    let project = ustx::audit(
        std::str::from_utf8(project_bytes)
            .map_err(|error| BundleError::Integrity(format!("USTX is not UTF-8: {error}")))?,
    )
    .map_err(BundleError::Integrity)?;
    verify_ustx_wave_part(
        root,
        canonical_root,
        project_path,
        &project,
        &manifest.audio.reference_mix.asset,
        &manifest.audio.reference_mix.svp_group_id,
        manifest.audio.reference_mix.muted_by_default,
    )?;
    for stem in &manifest.audio.stems {
        verify_ustx_wave_part(
            root,
            canonical_root,
            project_path,
            &project,
            &stem.asset,
            &stem.svp_group_id,
            !stem.active_by_default,
        )?;
    }
    Ok(())
}

fn verify_ustx_wave_part(
    root: &Path,
    canonical_root: &Path,
    project_path: &Path,
    project: &ustx::AuditedProject,
    asset: &AudioArtifactRecord,
    svp_group_id: &str,
    expected_muted: bool,
) -> Result<(), BundleError> {
    let artifact_path = asset.artifact.path.as_str();
    // A Synthesizer V group UUID in a project that has none would be an invented
    // identity, so an OpenUtau bundle must state none.
    if !svp_group_id.is_empty() {
        return Err(BundleError::Integrity(format!(
            "the manifest states a Synthesizer V group for {artifact_path} in an OpenUtau bundle"
        )));
    }
    let project_reference = project_audio_reference(artifact_path);
    // Compared as the scalar the file states, byte for byte, so the equality proves
    // what was written rather than what an unescaper made of it.
    let expected_scalar = ustx::quoted(&project_reference);
    let matches = project
        .wave_parts
        .iter()
        .filter(|part| part.relative_path_scalar == expected_scalar)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(BundleError::Integrity(format!(
            "USTX must contain exactly one wave part for {artifact_path}, found {}",
            matches.len()
        )));
    }
    let part = matches[0];
    // The whole file, from the start of the score: the same claim the Synthesizer V
    // path makes with `blickOffset: 0`. Anything else would state an edit the source
    // never asked for.
    if part.position != 0
        || part.skip != 0
        || part.trim != 0
        || part.fadein != 0
        || part.fadeout != 0
    {
        return Err(BundleError::Integrity(format!(
            "USTX wave part for {artifact_path} is offset, trimmed or faded"
        )));
    }
    // Recomputed from the two integers the validated WAV states, so a duration that
    // drifted from the audio cannot be committed.
    if part.file_duration_ms != ustx::file_duration_ms(asset.frames, asset.sample_rate) {
        return Err(BundleError::Integrity(format!(
            "USTX wave part for {artifact_path} states a duration the WAV does not"
        )));
    }
    // `UProject.AfterLoad` dereferences `tracks[part.trackNo]` unguarded, so a wave
    // part naming no track is a project OpenUtau cannot open at all. The mute state
    // lives on that track, which is where the manifest's claim has to be checked.
    let track_mute = usize::try_from(part.track_no)
        .ok()
        .and_then(|track_no| project.track_mutes.get(track_no))
        .ok_or_else(|| {
            BundleError::Integrity(format!(
                "USTX wave part for {artifact_path} sits on track {}, which the project does not hold",
                part.track_no
            ))
        })?;
    if *track_mute != expected_muted {
        return Err(BundleError::Integrity(format!(
            "USTX track for {artifact_path} has an invalid mute state"
        )));
    }
    verify_project_audio_reference(
        root,
        canonical_root,
        project_path,
        &project_reference,
        artifact_path,
        "USTX",
    )
}

/// Resolves one project audio reference the way the application that opens the
/// project resolves it — against the project file's own directory — and proves it
/// lands on the validated artifact the manifest recorded, inside the bundle.
///
/// Format-neutral on purpose: Synthesizer V combines an instrumental
/// `audio.filename` with the project's directory and OpenUtau combines a wave
/// part's `relative_path` with the same directory, so the resolution, the
/// containment and the equality are one rule and neither target can hold a weaker
/// version of it.
fn verify_project_audio_reference(
    root: &Path,
    canonical_root: &Path,
    project_path: &Path,
    project_reference: &str,
    artifact_path: &str,
    format: &str,
) -> Result<(), BundleError> {
    let referenced = project_path
        .parent()
        .ok_or_else(|| BundleError::Integrity(format!("{format} project has no parent")))?
        .join(project_reference);
    let referenced = fs::canonicalize(referenced).map_err(|source| BundleError::Io {
        phase: "resolve project audio reference",
        source,
    })?;
    let canonical_audio =
        fs::canonicalize(safe_join(root, artifact_path)?).map_err(|source| BundleError::Io {
            phase: "resolve validated audio asset",
            source,
        })?;
    if !referenced.starts_with(canonical_root) || referenced != canonical_audio {
        return Err(BundleError::Integrity(format!(
            "{format} audio reference for {artifact_path} escapes or mismatches the bundle"
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
pub(crate) mod tests {
    use super::*;
    use crate::engine::target::svp::{RenderConfig, Time};
    use crate::renderer::{MuseScoreRenderer, RendererCapabilities, WavInfo};
    use crate::stems::{StemDescriptor, StemPlan, StemRole};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Mutex;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    pub(crate) fn successful_renderer(stems: &[StemDescriptor]) -> Arc<dyn AudioRenderer> {
        Arc::new(FakeRenderer::with_stems(FakeMode::Success, stems))
    }

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

    /// A project holding no vocal track at all, in the shape the target wants. The
    /// bundle appends only audio references, so this is the smallest input that
    /// still exercises every one of them.
    fn empty_project(target: ExportTarget) -> BundleProject {
        match target {
            ExportTarget::Svp => BundleProject::Svp(SvpProject {
                version: 113,
                time: Time {
                    meter: vec![],
                    tempo: vec![],
                },
                render_config: RenderConfig::default(),
                tracks: vec![],
            }),
            ExportTarget::Ustx => BundleProject::Ustx(
                ustx::serialize(&ProjectedProject {
                    pronunciation_profile: Default::default(),
                    ticks_per_beat: 480,
                    meters: vec![crate::engine::projection::ProjectedMeter {
                        bar_index: 0,
                        numerator: 4,
                        denominator: 4,
                    }],
                    tempos: vec![crate::engine::projection::ProjectedTempo {
                        tick: 0,
                        bpm: 120.0,
                        source: None,
                        discovery_index: 0,
                    }],
                    ..ProjectedProject::default()
                })
                .expect("a bare time base is exactly representable"),
            ),
        }
    }

    /// A Synthesizer V bundle request: the shape every test that predates a second
    /// target uses, so those tests keep exercising 0.4.9's path unchanged.
    fn request(root: &Path, mode: FakeMode) -> BundleRequest {
        request_for(root, mode, ExportTarget::Svp)
    }

    fn request_for(root: &Path, mode: FakeMode, target: ExportTarget) -> BundleRequest {
        let destination = root.join("Song.versebundle");
        let layout = BundleLayout::new(&destination, "source.mid", target).unwrap();
        let stem_plan = StemPlan {
            stems: vec![StemDescriptor {
                stem_id: "part-001-test".into(),
                source_part_index: 0,
                source_part_id: "midi:track:0".into(),
                display_name: "Music".into(),
                source_track_ids: vec!["midi-track-0".into()],
                source_note_count: 1,
                role: StemRole::Accompaniment,
                active_by_default: true,
            }],
        };
        let entry = DispositionEntry {
            performance_refs: Vec::new(),
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
                project: empty_project(target),
                stem_plan,
                ledger: PreservationLedger {
                    intensity_context: None,
                    performance_spans: Vec::new(),
                    schema_version: SCHEMA_VERSION,
                    expected_source_ids: vec![entry.source_id.clone()],
                    entries: vec![entry],
                },
                warnings: vec!["[TEST_WARNING] retained diagnostic".into()],
            },
            renderer: Arc::new(FakeRenderer::new(mode)),
            render_limits: RenderLimits {
                timeout: std::time::Duration::from_secs(60),
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
            project_audio_reference(AUDIO_RELATIVE_PATH)
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

    /// Two stems whose mute defaults differ: a muted vocal reference Part and an
    /// audible accompaniment Part, plus the muted full-score reference every bundle
    /// carries. The one fixture that exercises every mute state a bundle can state.
    fn two_stem_request(root: &Path, mode: FakeMode, target: ExportTarget) -> BundleRequest {
        let mut request = request_for(root, mode, target);
        request.input.source_bytes = two_track_midi();
        request.input.stem_plan.stems[0].role = StemRole::VocalReference;
        request.input.stem_plan.stems[0].active_by_default = false;
        let second = StemDescriptor {
            stem_id: "part-002-test".into(),
            source_part_index: 1,
            source_part_id: "midi:track:1".into(),
            display_name: "Piano".into(),
            source_track_ids: vec!["piano".into()],
            source_note_count: 4,
            role: StemRole::Accompaniment,
            active_by_default: true,
        };
        let second_path = BundleLayout::new(&request.destination, "source.mid", target)
            .unwrap()
            .stem_audio_relative_path(&second);
        request.input.stem_plan.stems.push(second.clone());
        let entry = DispositionEntry {
            performance_refs: Vec::new(),
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
        request.renderer = Arc::new(FakeRenderer::with_parts(mode, 2));
        request
    }

    #[test]
    fn multiple_source_parts_become_distinct_audio_tracks_with_safe_mute_defaults() {
        let root = temp_dir("multiple-stems");
        let request = two_stem_request(&root, FakeMode::Success, ExportTarget::Svp);

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

    /// A `.mscz` holding exactly the Parts named, as MuseScore writes a part it
    /// cut from a score.
    fn part_container(part_ids: &[&str]) -> Vec<u8> {
        let parts = part_ids
            .iter()
            .map(|id| format!("<Part id=\"{id}\"><trackName>Part {id}</trackName></Part>"))
            .collect::<String>();
        let mscx = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <museScore version=\"4.20\"><Score><Division>480</Division>{parts}</Score></museScore>"
        );
        let mut writer = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        writer
            .start_file("score.mscx", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(mscx.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn part_descriptor(stem_id: &str, source_part_index: usize) -> StemDescriptor {
        StemDescriptor {
            stem_id: stem_id.into(),
            source_part_id: format!("musescore-part-{source_part_index}"),
            source_part_index,
            display_name: stem_id.into(),
            source_track_ids: vec![stem_id.into()],
            source_note_count: 1,
            role: StemRole::Accompaniment,
            active_by_default: true,
        }
    }

    fn extracted(ordinal: usize, part_ids: &[&str]) -> ExtractedScorePart {
        ExtractedScorePart {
            ordinal,
            name: part_ids.join("+"),
            metadata: serde_json::Value::Null,
            mscz: part_container(part_ids),
        }
    }

    #[test]
    fn a_part_the_author_saved_over_several_instruments_is_not_a_source_part() {
        // `--score-parts` answers with the excerpts already in the file as well
        // as the parts it can cut, so a score whose author saved a combined
        // part comes back with one container more than the source has Parts.
        let descriptors = vec![
            part_descriptor("melodie", 0),
            part_descriptor("basse", 1),
            part_descriptor("batterie", 2),
        ];
        let parts = vec![
            extracted(0, &["1"]),
            extracted(1, &["2"]),
            extracted(2, &["3"]),
            extracted(3, &["2", "3"]),
        ];

        let aligned = align_extracted_parts("museScore", &descriptors, parts).unwrap();

        assert_eq!(aligned.len(), 3);
        assert_eq!(aligned[0].mscz, part_container(&["1"]));
        assert_eq!(aligned[1].mscz, part_container(&["2"]));
        assert_eq!(aligned[2].mscz, part_container(&["3"]));
    }

    #[test]
    fn saved_parts_are_placed_on_the_source_whatever_order_musescore_lists_them() {
        let descriptors = vec![part_descriptor("first", 0), part_descriptor("second", 1)];
        let parts = vec![extracted(0, &["2"]), extracted(1, &["1"])];

        let aligned = align_extracted_parts("museScore", &descriptors, parts).unwrap();

        assert_eq!(aligned[0].mscz, part_container(&["1"]));
        assert_eq!(aligned[1].mscz, part_container(&["2"]));
    }

    #[test]
    fn a_source_part_that_carries_no_note_keeps_the_others_on_their_own_parts() {
        // A note-free Part has no stem, so the stems are a subsequence of the
        // Parts MuseScore cuts and position alone no longer places them.
        let descriptors = vec![part_descriptor("first", 0), part_descriptor("third", 2)];
        let parts = vec![
            extracted(0, &["1"]),
            extracted(1, &["2"]),
            extracted(2, &["3"]),
        ];

        let aligned = align_extracted_parts("museScore", &descriptors, parts).unwrap();

        assert_eq!(aligned[0].mscz, part_container(&["1"]));
        assert_eq!(aligned[1].mscz, part_container(&["3"]));
    }

    #[test]
    fn a_source_part_musescore_never_cut_is_still_refused() {
        let descriptors = vec![
            part_descriptor("first", 0),
            part_descriptor("second", 1),
            part_descriptor("third", 2),
        ];
        let parts = vec![extracted(0, &["1"]), extracted(1, &["2", "3"])];

        let error = align_extracted_parts("museScore", &descriptors, parts).unwrap_err();

        assert!(matches!(
            error,
            BundleError::Integrity(message) if message.contains("source topology")
        ));
    }

    #[test]
    fn native_scores_align_parts_by_verified_source_order() {
        let descriptors = vec![
            StemDescriptor {
                stem_id: "first".into(),
                source_part_index: 0,
                source_part_id: "musescore-part-native-b".into(),
                display_name: "Same".into(),
                source_track_ids: vec!["b".into()],
                source_note_count: 1,
                role: StemRole::Accompaniment,
                active_by_default: true,
            },
            StemDescriptor {
                stem_id: "second".into(),
                source_part_index: 1,
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
                source_part_index: 0,
                source_part_id: "midi:track:3".into(),
                display_name: "BANJO MELODY".into(),
                source_track_ids: vec!["midi:track:3".into()],
                source_note_count: 1,
                role: StemRole::Accompaniment,
                active_by_default: true,
            },
            StemDescriptor {
                stem_id: "strings".into(),
                source_part_index: 1,
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
            source_part_index: 0,
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
        let BundleProject::Svp(project) = &mut request.input.project else {
            panic!("this fixture is a Synthesizer V bundle");
        };
        append_instrumental_track(
            project,
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
                    track["mainRef"]["audio"]["filename"]
                        == project_audio_reference(AUDIO_RELATIVE_PATH)
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

    /// The staging directory of an in-flight export, found by the ownership marker
    /// the transaction writes: its name carries a timestamp and a counter no test
    /// can predict.
    fn staging_root(parent: &Path) -> PathBuf {
        let mut candidates = fs::read_dir(parent)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.join(".verse-staging").is_file())
            .collect::<Vec<_>>();
        assert_eq!(
            candidates.len(),
            1,
            "exactly one staging directory is in flight"
        );
        candidates.pop().unwrap()
    }

    /// Rewrites one staged file mid-transaction, so a bundle whose project or
    /// manifest states something the other does not can be proven blocking.
    ///
    /// A project mutation lands at `AfterProject`, before the manifest records the
    /// project's hash: mutating it later would fail the hash check first and never
    /// reach the audio invariants under test.
    struct MutateStagedFile {
        parent: PathBuf,
        at: FaultPoint,
        relative_path: String,
        from: &'static str,
        to: &'static str,
    }

    impl BundleHook for MutateStagedFile {
        fn checkpoint(&self, point: FaultPoint) -> Result<(), BundleError> {
            if point != self.at {
                return Ok(());
            }
            let path = staging_root(&self.parent).join(path_from_manifest(&self.relative_path));
            let original = fs::read_to_string(&path).unwrap();
            let mutated = original.replacen(self.from, self.to, 1);
            assert_ne!(mutated, original, "the mutation {:?} must apply", self.from);
            fs::write(&path, mutated).unwrap();
            Ok(())
        }
    }

    fn mutate_staged_project(
        root: &Path,
        from: &'static str,
        to: &'static str,
    ) -> MutateStagedFile {
        MutateStagedFile {
            parent: root.to_path_buf(),
            at: FaultPoint::AfterProject,
            relative_path: "project/Song.ustx".into(),
            from,
            to,
        }
    }

    /// A bundle whose project is an OpenUtau one references the same stems the
    /// Synthesizer V bundle does, through `wave_parts`, with the mute state each
    /// stem's role asks for and the full-score reference muted.
    #[test]
    fn an_openutau_bundle_references_every_stem_and_the_muted_full_score_reference() {
        let root = temp_dir("ustx-bundle");
        let request = two_stem_request(&root, FakeMode::Success, ExportTarget::Ustx);
        let stem_paths = request
            .input
            .stem_plan
            .stems
            .iter()
            .map(|stem| {
                BundleLayout::new(&request.destination, "source.mid", ExportTarget::Ustx)
                    .unwrap()
                    .stem_audio_relative_path(stem)
            })
            .collect::<Vec<_>>();

        let result = export_bundle(request).expect("an OpenUtau bundle is complete");
        assert!(
            result.project_path.ends_with("project/Song.ustx"),
            "{:?}",
            result.project_path
        );
        let project = fs::read_to_string(&result.project_path).unwrap();
        let audited = ustx::audit(&project).expect("the committed project is auditable");

        // One wave part per stem plus the full-score reference, each naming the very
        // WAV the bundle rendered.
        assert_eq!(audited.wave_parts.len(), 3);
        assert_eq!(
            audited
                .wave_parts
                .iter()
                .map(|part| part.relative_path_scalar.as_str())
                .collect::<Vec<_>>(),
            [
                ustx::quoted(&project_audio_reference(&stem_paths[0])),
                ustx::quoted(&project_audio_reference(&stem_paths[1])),
                ustx::quoted(&project_audio_reference(AUDIO_RELATIVE_PATH)),
            ]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
        );
        // The muted vocal reference Part, the audible accompaniment Part and the
        // muted full-score mix, each on the track its own wave part names.
        assert_eq!(audited.track_mutes, vec![true, false, true]);
        assert_eq!(
            audited
                .wave_parts
                .iter()
                .map(|part| part.track_no)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        // The whole file from the start of the score, and a length that is the
        // rendered WAV's own frame count over its sample rate.
        let manifest: BundleManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
        for (part, asset) in audited.wave_parts.iter().zip(
            manifest
                .audio
                .stems
                .iter()
                .map(|stem| &stem.asset)
                .chain([&manifest.audio.reference_mix.asset]),
        ) {
            assert_eq!((part.position, part.skip, part.trim), (0, 0, 0));
            assert_eq!((part.fadein, part.fadeout), (0, 0));
            assert_eq!(
                part.file_duration_ms,
                ustx::file_duration_ms(asset.frames, asset.sample_rate)
            );
        }
        // An OpenUtau project has no Synthesizer V group, so the manifest states
        // none rather than inventing one to fill the field.
        assert!(manifest.audio.reference_mix.svp_group_id.is_empty());
        assert!(manifest
            .audio
            .stems
            .iter()
            .all(|stem| stem.svp_group_id.is_empty()));
        assert!(manifest
            .warnings
            .iter()
            .any(|warning| warning.contains("audio-backed OpenUtau wave parts")));
        assert!(
            !manifest
                .warnings
                .iter()
                .any(|warning| warning.contains("SVP tracks")),
            "an OpenUtau bundle must not describe itself as a Synthesizer V one"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Everything a project states between `voice_parts:` and `wave_parts:` — the
    /// sung material, which audio must never touch.
    fn voice_parts_block(project: &str) -> &str {
        let start = project
            .find("\nvoice_parts:")
            .expect("a project states voice parts");
        let end = project
            .find("\nwave_parts:")
            .expect("a project states wave parts");
        &project[start..end]
    }

    /// Adding audio adds audio and nothing else: the voice part a bundle commits is
    /// byte-identical to the one the vocal-only export writes, and the wave parts
    /// take the track indices after it.
    #[test]
    fn an_openutau_bundle_keeps_the_vocal_part_the_vocal_only_export_writes() {
        let data = smf(&[
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // tempo
            0x00, 0xff, 0x05, 0x03, b'l', b'e', b't', // a real source lyric
            0x00, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0, // one note carrying it
            0x00, 0xff, 0x2f, 0x00,
        ]);
        let midi = crate::engine::midi::parse(&data).unwrap();
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        assert_eq!(outcome.placed, 1);
        let projected = outcome.svp.as_ref().expect("a projection");
        let vocal_only = ustx::to_yaml(&ustx::serialize(projected).expect("representable"));
        assert!(
            vocal_only.contains("        lyric: \"let\"\n"),
            "the fixture must project one real word"
        );

        let root = temp_dir("ustx-vocal-part");
        let destination = root.join("Song.versebundle");
        let layout = BundleLayout::new(&destination, "source.mid", ExportTarget::Ustx).unwrap();
        let stem_plan = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
        let stem_references = stem_plan
            .stems
            .iter()
            .map(|stem| project_audio_reference(&layout.stem_audio_relative_path(stem)))
            .collect::<Vec<_>>();
        let renderer = Arc::new(FakeRenderer::with_parts(
            FakeMode::Success,
            stem_plan.stems.len(),
        ));
        let result = export_bundle(BundleRequest {
            destination,
            input: BundleInput {
                original_name: "source.mid".into(),
                source_format: "standardMidi".into(),
                source_bytes: data.clone(),
                project: BundleProject::Ustx(ustx::serialize(projected).expect("representable")),
                stem_plan: stem_plan.clone(),
                ledger: build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan),
                warnings: vec![],
            },
            renderer,
            render_limits: RenderLimits {
                timeout: std::time::Duration::from_secs(60),
                max_output_bytes: 1024 * 1024,
            },
        })
        .expect("a complete OpenUtau bundle");

        let committed = fs::read_to_string(&result.project_path).unwrap();
        assert_eq!(
            voice_parts_block(&committed),
            voice_parts_block(&vocal_only),
            "the sung material must be exactly what the vocal-only export writes"
        );
        let audited = ustx::audit(&committed).expect("the committed project is auditable");
        // The voice lane keeps track 0 and stays audible; the audio takes the
        // indices after it, and the full-score reference is last.
        assert_eq!(audited.track_mutes.len(), stem_plan.stems.len() + 2);
        assert!(!audited.track_mutes[0]);
        // This fixture's only Part owns the vocal projection, so its stem is muted
        // to avoid doubling the singer and no accompaniment is left audible. The
        // reference therefore starts ACTIVE: it used to be muted unconditionally,
        // which opened a bundle that plays nothing at all and made the promise of
        // an audible reference mix false for every source whose every Part sings.
        assert!(!*audited.track_mutes.last().unwrap());
        assert!(
            audited.track_mutes.iter().any(|muted| !muted),
            "a bundle must never open with every track muted"
        );
        assert_eq!(
            audited
                .wave_parts
                .iter()
                .map(|part| part.track_no)
                .collect::<Vec<_>>(),
            (1..=stem_plan.stems.len() as i32 + 1).collect::<Vec<_>>()
        );
        assert_eq!(
            audited
                .wave_parts
                .iter()
                .map(|part| part.relative_path_scalar.clone())
                .collect::<Vec<_>>(),
            stem_references
                .iter()
                .map(|reference| ustx::quoted(reference))
                .chain([ustx::quoted(&project_audio_reference(AUDIO_RELATIVE_PATH))])
                .collect::<Vec<_>>()
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A note left out of the vocal project is still a source note.
    ///
    /// It is exactly the case the ledger exists for: not written into the project,
    /// preserved byte-exact in the source and audible in the stem rendered from
    /// it. The ledger is keyed by source item id and the stem plan comes from the
    /// source Part topology, so neither may shrink because the projection wrote
    /// one note fewer.
    #[test]
    fn a_lane_that_leaves_untexted_notes_out_still_inventories_them() {
        let data = smf(&[
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // tempo
            0x00, 0xff, 0x05, 0x03, b'l', b'e', b't', // a real source lyric
            0x00, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0, // the note carrying it
            0x00, 0x90, 62, 100, 0x83, 0x60, 0x80, 62, 0, // a note it never texts
            0x00, 0xff, 0x2f, 0x00,
        ]);
        let midi = crate::engine::midi::parse(&data).unwrap();
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        let projected = outcome.svp.as_ref().expect("a projection");
        assert_eq!(
            projected
                .tracks
                .iter()
                .map(|lane| (lane.muted, lane.notes.len()))
                .collect::<Vec<_>>(),
            vec![(false, 1)],
            "the untexted note is not written into the project"
        );

        let stem_plan = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
        assert_eq!(
            stem_plan.stems.len(),
            1,
            "one source Part means one stem, whatever the projection does with it"
        );

        let root = temp_dir("ustx-untexted-companion");
        let destination = root.join("Song.versebundle");
        let layout = BundleLayout::new(&destination, "source.mid", ExportTarget::Ustx).unwrap();
        let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);
        assert_eq!(
            ledger
                .entries
                .iter()
                .filter(|entry| entry.item_kind == SourceItemKind::Note)
                .count(),
            2,
            "two source notes, two note entries: leaving one out of the project \
             must not leave it out of the ledger"
        );
        let ids = ledger
            .entries
            .iter()
            .map(|entry| entry.source_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids.len(),
            ids.iter().collect::<BTreeSet<_>>().len(),
            "one primary disposition per source id"
        );

        let stem_count = stem_plan.stems.len();
        let renderer = Arc::new(FakeRenderer::with_parts(FakeMode::Success, stem_count));
        let result = export_bundle(BundleRequest {
            destination,
            input: BundleInput {
                original_name: "source.mid".into(),
                source_format: "standardMidi".into(),
                source_bytes: data.clone(),
                project: BundleProject::Ustx(ustx::serialize(projected).expect("representable")),
                stem_plan: stem_plan.clone(),
                ledger,
                warnings: vec![],
            },
            renderer,
            render_limits: RenderLimits {
                timeout: std::time::Duration::from_secs(60),
                max_output_bytes: 1024 * 1024,
            },
        })
        .expect("a bundle with a companion lane passes every verification");

        let committed = fs::read_to_string(&result.project_path).unwrap();
        let audited = ustx::audit(&committed).expect("the committed project is auditable");
        // The sung lane, one stem, and the full-score reference.
        assert_eq!(audited.track_mutes.len(), stem_count + 2);
        assert!(!audited.track_mutes[0], "the sung lane stays audible");
        assert!(
            audited.track_mutes.iter().any(|muted| !muted),
            "a bundle must never open with every track muted"
        );
        assert_eq!(
            audited
                .wave_parts
                .iter()
                .map(|part| part.track_no)
                .collect::<Vec<_>>(),
            (1..=stem_count as i32 + 1).collect::<Vec<_>>()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retained_note_ledger_matches_saved_vocals_in_both_bundle_targets() {
        let data = smf(&[
            0x00, 0xff, 0x05, 0x03, b'l', b'e', b't', // sung lyric
            0x00, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0, 0x00, 0xff, 0x05,
            0x00, // explicit empty lyric, removed with its note
            0x00, 0x90, 62, 100, 0x83, 0x60, 0x80, 62, 0, 0x00, 0x90, 64, 100, 0x83, 0x60, 0x80,
            64, 0, // genuinely untexted
            0x00, 0xff, 0x2f, 0x00,
        ]);
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let midi = midi::parse(&data).unwrap();
            let outcome =
                crate::engine::convert::convert_midi_with_target(&midi, "english", None, target);
            assert!(outcome.ok, "{:?}", outcome.msg);
            let projected = outcome.svp.as_ref().unwrap();
            assert_eq!(projected.tracks.len(), 1);
            assert_eq!(projected.tracks[0].notes.len(), 1);
            assert_eq!(projected.tracks[0].notes[0].pitch, 60);
            let direct = crate::engine::target::serialize_to(target, projected).unwrap();
            let root = temp_dir(&format!("retained-ledger-{}", target.extension()));
            let destination = root.join("Song.versebundle");
            let layout = BundleLayout::new(&destination, "source.mid", target).unwrap();
            let stem_plan = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
            let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);
            let result = export_bundle(BundleRequest {
                destination,
                input: BundleInput {
                    original_name: "source.mid".into(),
                    source_format: "standardMidi".into(),
                    source_bytes: data.clone(),
                    project: BundleProject::from_projection(target, projected).unwrap(),
                    stem_plan: stem_plan.clone(),
                    ledger,
                    warnings: vec![],
                },
                renderer: successful_renderer(&stem_plan.stems),
                render_limits: RenderLimits {
                    timeout: Duration::from_secs(60),
                    max_output_bytes: 1024 * 1024,
                },
            })
            .unwrap();
            let saved = fs::read(&result.project_path).unwrap();
            match target {
                ExportTarget::Svp => {
                    let saved: serde_json::Value = serde_json::from_slice(&saved).unwrap();
                    let direct: serde_json::Value = serde_json::from_slice(&direct).unwrap();
                    assert_eq!(saved["tracks"][0], direct["tracks"][0]);
                    assert_eq!(
                        saved["tracks"][0]["mainGroup"]["notes"]
                            .as_array()
                            .unwrap()
                            .len(),
                        1
                    );
                    assert_eq!(saved["tracks"][0]["mainGroup"]["notes"][0]["pitch"], 60);
                }
                ExportTarget::Ustx => {
                    let saved = std::str::from_utf8(&saved).unwrap();
                    let direct = std::str::from_utf8(&direct).unwrap();
                    assert_eq!(voice_parts_block(saved), voice_parts_block(direct));
                    assert_eq!(voice_parts_block(saved).matches("        tone:").count(), 1);
                    assert!(voice_parts_block(saved).contains("        tone: 60\n"));
                }
            }
            assert_eq!(fs::read(&result.source_path).unwrap(), data);
            let ledger: PreservationLedger = serde_json::from_slice(
                &fs::read(result.bundle_path.join(PRESERVATION_RELATIVE_PATH)).unwrap(),
            )
            .unwrap();
            let entry = |id: &str| {
                ledger
                    .entries
                    .iter()
                    .find(|entry| entry.source_id == id)
                    .unwrap()
            };
            for track in &midi.tracks {
                for event in &track.events {
                    if !matches!(
                        event.kind,
                        Kind::NoteOn(_) | Kind::NoteOff(_) | Kind::Lyrics(_)
                    ) {
                        continue;
                    }
                    let event_id = format!("event:{}:{}", track.id, event.order);
                    // Identify the fixture's sung word and its pitched note,
                    // independently of parser event numbering or metadata.
                    let retained = match &event.kind {
                        Kind::NoteOn(note) => note.key == Some(60),
                        Kind::NoteOff(note) => note.key == Some(60),
                        Kind::Lyrics(lyric) => lyric.raw == "let",
                        _ => unreachable!(),
                    };
                    let mut ids = vec![event_id];
                    if let Kind::NoteOn(note) = &event.kind {
                        ids.push(note_instance_id(&track.id, &note.source, event.order));
                    }
                    if let Kind::Lyrics(lyric) = &event.kind {
                        ids.push(standalone_lyric_instance_id(lyric, &track.id, event.order));
                    }
                    for id in ids {
                        let entry = entry(&id);
                        assert_eq!(
                            entry.disposition == PrimaryDisposition::ProjectedExact,
                            retained,
                            "{target:?} {id}"
                        );
                        assert_eq!(
                            entry.artifact_paths.contains(&layout.project_relative_path),
                            retained,
                            "{id}"
                        );
                        assert!(entry.artifact_paths.contains(&layout.source_relative_path));
                        if matches!(event.kind, Kind::NoteOn(_) | Kind::NoteOff(_)) {
                            let stem = &stem_plan.stems[0];
                            assert!(entry
                                .artifact_paths
                                .contains(&layout.stem_audio_relative_path(stem)));
                            if !retained {
                                assert_eq!(
                                    entry.disposition,
                                    PrimaryDisposition::RenderedStem {
                                        stem_id: stem.stem_id.clone()
                                    }
                                );
                            }
                        } else if !retained {
                            assert!(matches!(
                                entry.disposition,
                                PrimaryDisposition::SourceOnly { .. }
                            ));
                            assert_eq!(
                                entry.artifact_paths,
                                vec![layout.source_relative_path.clone()],
                                "dropped lyric/event has no project or stem artifact"
                            );
                        }
                    }
                }
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn retained_split_verses_match_every_saved_vocal_lane_and_ledger_artifact() {
        let data = crate::engine::convert::RETAINED_SPLIT_FIXTURE.as_bytes();
        let midi = crate::engine::musicxml::parse(data).unwrap();
        // The same source voice overlaps at tick 480. Each verse must split
        // into two actual voices, with the last note present only in verse 1.
        let expected = [
            vec![(0, 960, 60), (1920, 480, 62)],
            vec![(480, 960, 64)],
            vec![(0, 960, 60)],
            vec![(480, 960, 64)],
        ];
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let outcome =
                crate::engine::convert::convert_midi_with_target(&midi, "english", None, target);
            assert!(outcome.ok, "{:?}", outcome.msg);
            assert!(outcome
                .tracks
                .iter()
                .flat_map(|track| &track.warnings)
                .any(|warning| warning.code == crate::engine::convert::SIMULTANEOUS_VOICES_SPLIT));
            let project = outcome.svp.as_ref().unwrap();
            assert_eq!(project.tracks.len(), expected.len());
            for (lane, expected) in project.tracks.iter().zip(&expected) {
                assert_eq!(
                    lane.notes
                        .iter()
                        .map(|note| (note.onset_ticks, note.duration_ticks, note.pitch))
                        .collect::<Vec<_>>(),
                    *expected
                );
            }
            let direct = crate::engine::target::serialize_to(target, project).unwrap();
            let root = temp_dir(&format!("retained-split-{}", target.extension()));
            let destination = root.join("Song.versebundle");
            let layout = BundleLayout::new(&destination, "source.musicxml", target).unwrap();
            let stem_plan = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
            assert_eq!(stem_plan.stems.len(), 1);
            let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);
            let result = export_bundle(BundleRequest {
                destination,
                input: BundleInput {
                    original_name: "source.musicxml".into(),
                    source_format: "musicXml".into(),
                    source_bytes: data.to_vec(),
                    project: BundleProject::from_projection(target, project).unwrap(),
                    stem_plan: stem_plan.clone(),
                    ledger,
                    warnings: vec![],
                },
                renderer: successful_renderer(&stem_plan.stems),
                render_limits: RenderLimits {
                    timeout: Duration::from_secs(60),
                    max_output_bytes: 1024 * 1024,
                },
            })
            .unwrap();
            assert_eq!(fs::read(&result.source_path).unwrap(), data);
            let saved = fs::read(&result.project_path).unwrap();
            match target {
                ExportTarget::Svp => {
                    let saved: serde_json::Value = serde_json::from_slice(&saved).unwrap();
                    let direct: serde_json::Value = serde_json::from_slice(&direct).unwrap();
                    let vocals: Vec<_> = saved["tracks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|track| track["mainRef"]["isInstrumental"] != true)
                        .collect();
                    assert_eq!(vocals.len(), expected.len());
                    for (index, (lane, expected)) in vocals.iter().zip(&expected).enumerate() {
                        assert_eq!(
                            **lane, direct["tracks"][index],
                            "all saved vocal tracks match direct output"
                        );
                        let notes = lane["mainGroup"]["notes"].as_array().unwrap();
                        assert_eq!(notes.len(), expected.len());
                        for (note, &(onset, duration, pitch)) in notes.iter().zip(expected) {
                            assert_eq!(note["onset"], u64::from(onset) * 1_470_000);
                            assert_eq!(note["duration"], u64::from(duration) * 1_470_000);
                            assert_eq!(note["pitch"], pitch);
                        }
                    }
                }
                ExportTarget::Ustx => {
                    let saved = std::str::from_utf8(&saved).unwrap();
                    assert_eq!(
                        voice_parts_block(saved),
                        voice_parts_block(std::str::from_utf8(&direct).unwrap())
                    );
                    let parts: Vec<_> = voice_parts_block(saved)
                        .split("\n  - name: ")
                        .skip(1)
                        .collect();
                    assert_eq!(parts.len(), expected.len());
                    for (index, (part, expected)) in parts.iter().zip(&expected).enumerate() {
                        assert!(part.contains(&format!("    track_no: {index}\n")));
                        let notes: Vec<_> = part.split("      - position: ").skip(1).collect();
                        assert_eq!(notes.len(), expected.len());
                        for (note, (onset, duration, pitch)) in notes.iter().zip(expected) {
                            assert!(note.starts_with(&format!(
                                "{onset}\n        duration: {duration}\n        tone: {pitch}\n"
                            )));
                        }
                    }
                }
            }
            let ledger: PreservationLedger = serde_json::from_slice(
                &fs::read(result.bundle_path.join(PRESERVATION_RELATIVE_PATH)).unwrap(),
            )
            .unwrap();
            let entry = |id: &str| {
                ledger
                    .entries
                    .iter()
                    .find(|entry| entry.source_id == id)
                    .unwrap()
            };
            let source = &layout.source_relative_path;
            let vocal = &layout.project_relative_path;
            let stem = layout.stem_audio_relative_path(&stem_plan.stems[0]);
            let mut expected_ids = BTreeSet::new();
            for track in &midi.tracks {
                for event in &track.events {
                    let (key, note) = match &event.kind {
                        Kind::NoteOn(note) => (note.key, Some(note)),
                        Kind::NoteOff(note) => (note.key, None),
                        _ => continue,
                    };
                    let retained = key != Some(67); // source G4 has no words in either verse
                    let mut note_ids = vec![format!("event:{}:{}", track.id, event.order)];
                    if let Some(note) = note {
                        note_ids.push(note_instance_id(&track.id, &note.source, event.order));
                        for lyric in &note.lyrics {
                            let id = attached_lyric_instance_id(lyric, &note.source, event.order);
                            let entry = entry(&id);
                            let lyric_retained = !lyric.raw.is_empty();
                            if lyric_retained {
                                expected_ids.insert(id.clone());
                            }
                            assert_eq!(
                                entry.disposition == PrimaryDisposition::ProjectedExact,
                                lyric_retained,
                                "{id}"
                            );
                            assert_eq!(
                                entry.artifact_paths,
                                if lyric_retained {
                                    vec![source.clone(), vocal.clone()]
                                } else {
                                    vec![source.clone()]
                                },
                                "{id}: exact lyric artifact set"
                            );
                            if !lyric_retained {
                                assert!(matches!(
                                    entry.disposition,
                                    PrimaryDisposition::SourceOnly { .. }
                                ));
                            }
                        }
                    }
                    for id in note_ids {
                        if retained {
                            expected_ids.insert(id.clone());
                        }
                        let entry = entry(&id);
                        assert_eq!(
                            entry.disposition,
                            if retained {
                                PrimaryDisposition::ProjectedExact
                            } else {
                                PrimaryDisposition::RenderedStem {
                                    stem_id: stem_plan.stems[0].stem_id.clone(),
                                }
                            },
                            "{id}"
                        );
                        assert_eq!(
                            entry.artifact_paths,
                            if retained {
                                vec![source.clone(), vocal.clone(), stem.clone()]
                            } else {
                                vec![source.clone(), stem.clone()]
                            },
                            "{id}: exact note/event artifact set"
                        );
                    }
                }
            }
            let represented: BTreeSet<_> = project
                .tracks
                .iter()
                .flat_map(|lane| &lane.notes)
                .flat_map(|note| note.source_evidence.as_ref().unwrap().source_ids())
                .map(str::to_owned)
                .collect();
            assert_eq!(
                represented, expected_ids,
                "all original retained note/on/off/lyric identities across every saved lane"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// The stems are the bundle's, not the project's: both targets reference the
    /// same WAVs by the same relative paths, and only the file that references them
    /// differs.
    #[test]
    fn both_targets_reference_the_same_stems_by_the_same_relative_paths() {
        let mut references: Vec<Vec<String>> = Vec::new();
        let mut hashes: Vec<Vec<String>> = Vec::new();
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let root = temp_dir(&format!("same-stems-{}", target.extension()));
            let result = export_bundle(two_stem_request(&root, FakeMode::Success, target))
                .expect("both targets bundle this source");
            let manifest: BundleManifest =
                serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
            let assets = manifest
                .audio
                .stems
                .iter()
                .map(|stem| &stem.asset)
                .chain([&manifest.audio.reference_mix.asset])
                .collect::<Vec<_>>();
            references.push(
                assets
                    .iter()
                    .map(|asset| project_audio_reference(&asset.artifact.path))
                    .collect(),
            );
            hashes.push(
                assets
                    .iter()
                    .map(|asset| asset.artifact.sha256.clone())
                    .collect(),
            );

            // Whatever the project is, every reference it holds must be the one the
            // manifest recorded — which is what its own verification proved.
            let project = fs::read_to_string(&result.project_path).unwrap();
            for reference in references.last().unwrap() {
                let stated = match target {
                    ExportTarget::Svp => project.contains(&format!("\"filename\":\"{reference}\"")),
                    ExportTarget::Ustx => project
                        .contains(&format!("    relative_path: {}\n", ustx::quoted(reference))),
                };
                assert!(stated, "{target:?} does not reference {reference}");
            }
            fs::remove_dir_all(root).unwrap();
        }
        assert_eq!(references[0], references[1]);
        assert_eq!(
            hashes[0], hashes[1],
            "the same source must render byte-identical stems under either target"
        );
    }

    /// The Synthesizer V bundle is release 0.4.9's, unchanged by the second target:
    /// the same project filename, the same audio-track shape, the same group UUIDs
    /// and the same manifest sentence.
    #[test]
    fn the_synthesizer_v_bundle_is_unchanged_by_the_openutau_target() {
        let root = temp_dir("svp-unchanged");
        let result = export_bundle(request(&root, FakeMode::Success)).unwrap();
        assert!(
            result.project_path.ends_with("project/Song.svp"),
            "{:?}",
            result.project_path
        );
        let project: serde_json::Value =
            serde_json::from_slice(&fs::read(&result.project_path).unwrap()).unwrap();
        assert_eq!(project["version"], 113);
        let tracks = project["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(
            tracks[0]["mainRef"]["audio"]["filename"],
            "../audio/stems/part-001-test-music.wav"
        );
        // 441 frames at 44 100 Hz: the fake renderer's WAV, unchanged since 0.4.9.
        assert_eq!(tracks[0]["mainRef"]["audio"]["duration"], 0.01);
        assert_eq!(tracks[0]["mainRef"]["blickOffset"], 0);
        assert_eq!(
            tracks[0]["mainRef"]["groupID"],
            "00000000-0000-4000-8000-000000000000"
        );
        assert_eq!(
            tracks[1]["mainRef"]["audio"]["filename"],
            "../audio/full-score.wav"
        );
        assert_eq!(
            tracks[1]["mainRef"]["groupID"],
            "00000001-0000-4000-8000-000000000000"
        );

        let manifest: BundleManifest =
            serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.schema_version, 2);
        assert_eq!(
            manifest.audio.stems[0].svp_group_id,
            "00000000-0000-4000-8000-000000000000"
        );
        assert_eq!(
            manifest.audio.reference_mix.svp_group_id,
            "00000001-0000-4000-8000-000000000000"
        );
        assert_eq!(manifest.alignment.svp_blick_offset, 0);
        assert!(manifest.warnings.iter().any(|warning| warning
            == "The full-score reference mix is retained muted; source Parts are rendered as separate audio-backed SVP tracks."));
        fs::remove_dir_all(root).unwrap();
    }

    /// A stem the project no longer references cannot be committed, whichever way
    /// the reference stopped being there.
    #[test]
    fn a_wave_part_that_does_not_reference_its_stem_is_blocking_and_transactional() {
        let root = temp_dir("ustx-missing-wave-part");
        let destination = root.join("Song.versebundle");
        let hook = mutate_staged_project(
            &root,
            "    relative_path: \"../audio/stems/part-001-test-music.wav\"\n",
            "    relative_path: \"../audio/full-score.wav\"\n",
        );
        let error = export_bundle_with_hook(
            request_for(&root, FakeMode::Success, ExportTarget::Ustx),
            &hook,
        )
        .expect_err("a stem with no wave part is not a complete bundle");
        assert!(
            matches!(&error, BundleError::Integrity(message) if message.contains("wave part")),
            "{error}"
        );
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    /// The full-score reference is a muted reference mix, not an accompaniment. In
    /// OpenUtau the mute lives on the track, so that is where it is verified.
    #[test]
    fn an_unmuted_full_score_reference_is_blocking_in_an_openutau_bundle() {
        let root = temp_dir("ustx-unmuted-reference");
        let destination = root.join("Song.versebundle");
        let hook = mutate_staged_project(&root, "    mute: true\n", "    mute: false\n");
        let error = export_bundle_with_hook(
            request_for(&root, FakeMode::Success, ExportTarget::Ustx),
            &hook,
        )
        .expect_err("the reference mix must stay muted");
        assert!(
            matches!(&error, BundleError::Integrity(message) if message.contains("mute state")),
            "{error}"
        );
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    /// A wave part must play the whole file from the start of the score, and must
    /// sit on a track the project actually holds: `UProject.AfterLoad` dereferences
    /// `tracks[part.trackNo]` unguarded, so a missing track makes the project
    /// unopenable rather than merely wrong.
    #[test]
    fn an_offset_or_homeless_wave_part_is_blocking_and_transactional() {
        for (index, (from, to, expected)) in [
            (
                "    skip: 0\n",
                "    skip: 480\n",
                "offset, trimmed or faded",
            ),
            (
                "    trim: 0\n",
                "    trim: 480\n",
                "offset, trimmed or faded",
            ),
            (
                "    fadein: 0\n",
                "    fadein: 10\n",
                "offset, trimmed or faded",
            ),
            (
                "    track_no: 0\n",
                "    track_no: 7\n",
                "which the project does not hold",
            ),
            (
                "    file_duration_ms: 10\n",
                "    file_duration_ms: 11\n",
                "a duration the WAV does not",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let root = temp_dir(&format!("ustx-wave-part-{index}"));
            let destination = root.join("Song.versebundle");
            let error = export_bundle_with_hook(
                request_for(&root, FakeMode::Success, ExportTarget::Ustx),
                &mutate_staged_project(&root, from, to),
            )
            .expect_err("a wave part that states an edit the source never asked for");
            assert!(
                matches!(&error, BundleError::Integrity(message) if message.contains(expected)),
                "{from:?}: {error}"
            );
            assert!(!destination.exists());
            assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// An OpenUtau bundle whose manifest claims a Synthesizer V group is claiming an
    /// identity no file in it holds.
    #[test]
    fn a_manifest_claiming_a_synthesizer_v_group_in_an_openutau_bundle_is_blocking() {
        let root = temp_dir("ustx-invented-group");
        let destination = root.join("Song.versebundle");
        let hook = MutateStagedFile {
            parent: root.clone(),
            // The manifest's own bytes are not hashed by anything, so mutating it
            // after it is written reaches verification directly.
            at: FaultPoint::AfterManifest,
            relative_path: MANIFEST_RELATIVE_PATH.into(),
            from: "\"svpGroupId\": \"\"",
            to: "\"svpGroupId\": \"00000000-0000-4000-8000-000000000000\"",
        };
        let error = export_bundle_with_hook(
            request_for(&root, FakeMode::Success, ExportTarget::Ustx),
            &hook,
        )
        .expect_err("an OpenUtau bundle holds no Synthesizer V group");
        assert!(
            matches!(&error, BundleError::Integrity(message) if message.contains("Synthesizer V group")),
            "{error}"
        );
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    /// The shared canonicalisation both targets use: a reference is resolved against
    /// the project file's own directory, and it must land on the validated artifact
    /// inside the bundle. Neither a file outside the bundle nor another file inside
    /// it is that artifact.
    #[test]
    fn a_project_audio_reference_must_resolve_onto_the_validated_artifact() {
        let root = temp_dir("audio-reference");
        let bundle = root.join("Song.versebundle");
        fs::create_dir(&bundle).unwrap();
        for directory in ["project", "audio"] {
            fs::create_dir(bundle.join(directory)).unwrap();
        }
        let project_path = bundle.join("project").join("Song.svp");
        fs::write(&project_path, b"{}").unwrap();
        fs::write(bundle.join(AUDIO_RELATIVE_PATH), b"RIFF").unwrap();
        fs::write(bundle.join("manifest.json"), b"{}").unwrap();
        fs::write(root.join("outside.wav"), b"RIFF").unwrap();
        let canonical_root = fs::canonicalize(&bundle).unwrap();
        let resolve = |reference: &str| {
            verify_project_audio_reference(
                &bundle,
                &canonical_root,
                &project_path,
                reference,
                AUDIO_RELATIVE_PATH,
                "SVP",
            )
        };

        assert!(resolve(&project_audio_reference(AUDIO_RELATIVE_PATH)).is_ok());
        // A real file outside the bundle, reached by climbing out of it.
        let escape = resolve("../../outside.wav").expect_err("the reference escapes the bundle");
        assert!(
            matches!(&escape, BundleError::Integrity(message) if message.contains("escapes or mismatches")),
            "{escape}"
        );
        // A real file inside the bundle that is not the validated artifact.
        let other = resolve("../manifest.json").expect_err("the reference is not the artifact");
        assert!(
            matches!(&other, BundleError::Integrity(message) if message.contains("escapes or mismatches")),
            "{other}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_transaction_phase_rolls_back() {
        for (index, (target, point)) in [ExportTarget::Svp, ExportTarget::Ustx]
            .into_iter()
            .flat_map(|target| {
                [
                    FaultPoint::AfterSource,
                    FaultPoint::AfterAudio,
                    FaultPoint::AfterProject,
                    FaultPoint::AfterPreservation,
                    FaultPoint::AfterManifest,
                    FaultPoint::BeforeCommit,
                    FaultPoint::AfterRename,
                ]
                .into_iter()
                .map(move |point| (target, point))
            })
            .enumerate()
        {
            let root = temp_dir(&format!("phase-{index}"));
            let destination = root.join("Song.versebundle");
            assert!(export_bundle_with_hook(
                request_for(&root, FakeMode::Success, target),
                &FailAt(point)
            )
            .is_err());
            assert!(!destination.exists(), "{target:?} failed at {point:?}");
            assert_eq!(
                fs::read_dir(&root).unwrap().count(),
                0,
                "{target:?} failed at {point:?}"
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
        request.input.ledger.schema_version = 4;
        assert!(matches!(
            export_bundle(request),
            Err(BundleError::InvalidLedger(_))
        ));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_intensity_bounds_are_validated_without_requiring_the_optional_extension() {
        use crate::engine::performance::{Dimension, PerformanceReference, TransferStatus};
        let span = PerformanceReference {
            intensity: None,
            target: "ustx".into(),
            target_track: None,
            source_track_id: "score-part".into(),
            dimension: Dimension::LinearGain,
            start_tick: 0,
            end_tick: 480,
            note_ids: vec![],
            status: TransferStatus::Unsupported,
            detail: "retained source intent".into(),
        };
        let mut ledger = PreservationLedger {
            intensity_context: None,
            schema_version: 3,
            expected_source_ids: vec![],
            entries: vec![],
            performance_spans: vec![span],
        };
        let allowed = BTreeSet::new();
        assert!(ledger.validate(&allowed).is_ok());
        let legacy_bytes = serde_json::to_vec(&ledger).unwrap();
        assert!(!String::from_utf8_lossy(&legacy_bytes).contains("intensity"));
        let legacy: PreservationLedger = serde_json::from_slice(&legacy_bytes).unwrap();
        assert!(legacy.validate(&allowed).is_ok());
        ledger.expected_source_ids = vec!["track:score-part".into(), "expression:dynamic".into()];
        ledger.entries = ledger
            .expected_source_ids
            .iter()
            .enumerate()
            .map(|(i, id)| DispositionEntry {
                source_id: id.clone(),
                item_kind: if i == 0 {
                    SourceItemKind::Track
                } else {
                    SourceItemKind::Event
                },
                disposition: PrimaryDisposition::SourceOnly {
                    reason: "retained".into(),
                },
                artifact_paths: vec!["source/source.xml".into()],
                performance_refs: if i == 0 { vec![] } else { vec![0] },
            })
            .collect();
        let allowed = BTreeSet::from(["source/source.xml".into()]);
        let owner = IntensityScoreOwner {
            part: "score-part".into(),
            staff: "".into(),
            voice: "".into(),
            instrument: None,
        };
        ledger.intensity_context = Some(IntensityContext {
            source_ppq: 480,
            source_tracks: vec!["score-part".into()],
            source_notes: vec![],
            controllers: vec![],
            projected_notes: vec![],
            dependencies: vec![IntensityDependencySource {
                owner: owner.clone(),
                scope: serde_json::json!({"Part":"score-part"}),
                occurrence: 1,
                repeat_pass: 1,
                start: serde_json::json!({"numerator":0,"denominator":1}),
                end: serde_json::json!({"numerator":8_000_000,"denominator":1}),
                segment: false,
                attack_note_id: None,
                source_ids: vec!["expression:dynamic".into()],
            }],
            score_owners: vec![IntensityScoreTrack {
                source_track_id: "score-part".into(),
                owner: owner.clone(),
            }],
            declarations: vec![IntensityDeclarationSource {
                source_id: "expression:dynamic".into(),
                kinds: serde_json::json!(["Dynamic"]),
                scope: serde_json::json!({"Part":"score-part"}),
                at: serde_json::json!({"numerator":0,"denominator":1}),
                note_source_id: None,
                paired_source_ids: Vec::new(),
                applications: vec![IntensityDeclarationApplication {
                    owner,
                    occurrence: 1,
                    repeat_pass: 1,
                    active: true,
                    start: serde_json::json!({"numerator":0,"denominator":1}),
                    end: serde_json::json!({"numerator":8_000_000,"denominator":1}),
                    written_start: serde_json::json!({"numerator":0,"denominator":1}),
                }],
            }],
        });
        let extension = |bounds: serde_json::Value| {
            serde_json::json!({
                "policy":"verse-score-intensity-v1", "start":bounds["start"], "end":bounds["end"],
                "terminalEndpoint":false, "provenance":{
                    "policy":"verse-score-intensity-v1", "scope":{"Part":"score-part"},
                    "occurrence":1,"repeat_pass":1,"interpretations":[],
                    "evidence":[{"source_ids":["expression:dynamic"],"contract":"MusicXml",
                        "saving_version":null,"layout":"4.0","raw_fields":{"xml":"<dynamics><p/></dynamics>"}}]
                }
            })
        };
        for bounds in [
            serde_json::json!({"start":{"numerator":0,"denominator":1},"end":{"numerator":1,"denominator":3}}),
            serde_json::json!({"start":{"numerator":0,"denominator":1},"end":{"numerator":"10000333333333333333348","denominator":1000000000000000000i64}}),
        ] {
            let value = extension(bounds);
            let (start, end) = intensity_bounds(&value).unwrap();
            let (a, b) =
                crate::engine::performance::exact_source_tick_bounds(start, end, 480).unwrap();
            ledger.performance_spans[0].start_tick = a;
            ledger.performance_spans[0].end_tick = b;
            ledger.performance_spans[0].intensity = Some(value);
            let bytes = serde_json::to_vec(&ledger).unwrap();
            let reopened: PreservationLedger = serde_json::from_slice(&bytes).unwrap();
            reopened.validate(&allowed).unwrap();
        }
        for bad in [
            serde_json::json!({"start":{"numerator":0,"denominator":1},"end":{"numerator":"999999999999999999999999999999999999999999999","denominator":1}}),
            serde_json::json!({"start":{"numerator":0,"denominator":0},"end":{"numerator":1,"denominator":1}}),
            serde_json::json!({"start":{"numerator":2,"denominator":3},"end":{"numerator":1,"denominator":3}}),
            serde_json::json!({"start":{"numerator":-1,"denominator":1},"end":{"numerator":1,"denominator":1}}),
            serde_json::json!({"start":0,"end":1}),
        ] {
            ledger.performance_spans[0].intensity = Some(extension(bad));
            assert!(matches!(
                ledger.validate(&allowed),
                Err(BundleError::InvalidLedger(_))
            ));
        }
    }

    #[test]
    fn validation_budget_covers_actual_saved_common_contributor_path() {
        let body = format!("<direction><direction-type><dynamics><p/></dynamics></direction-type></direction>{}", "<note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><lyric><text>la</text></lyric></note>".repeat(1500));
        let source = crate::engine::musicxml::parse(format!(r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes>{body}</measure></part></score-partwise>"#).as_bytes()).unwrap();
        let outcome = crate::engine::convert::convert_midi_with_target(
            &source,
            "english",
            None,
            ExportTarget::Ustx,
        );
        assert!(outcome.ok, "{:?}", outcome.msg);
        let root = temp_dir("validation-common-contributor");
        let layout = BundleLayout::new(
            &root.join("Common.versebundle"),
            "source.xml",
            ExportTarget::Ustx,
        )
        .unwrap();
        let stems = StemPlan::from_source(&source, &outcome.tracks).unwrap();
        let allowed = [
            layout.source_relative_path.clone(),
            layout.project_relative_path.clone(),
        ]
        .into_iter()
        .chain(
            stems
                .stems
                .iter()
                .map(|s| layout.stem_audio_relative_path(s)),
        )
        .collect();
        let ledger = build_preservation_ledger(&source, &outcome.projection, &layout, &stems);
        assert!(
            ledger
                .entries
                .iter()
                .any(|e| e.performance_refs.len() >= 1500),
            "one legitimate held declaration shared by every note"
        );
        let path = root.join("preservation.json");
        fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
        let saved: PreservationLedger = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        saved.validate(&allowed).unwrap();
        for limits in [
            LedgerValidationLimits {
                references: 8,
                ..LedgerValidationLimits::default()
            },
            LedgerValidationLimits {
                bytes: 512,
                ..LedgerValidationLimits::default()
            },
            LedgerValidationLimits {
                work: 8,
                ..LedgerValidationLimits::default()
            },
        ] {
            assert!(
                matches!(saved.validate_with_limits(&allowed, limits), Err(BundleError::InvalidLedger(message)) if message == "performance evidence exceeds bounded storage")
            );
        }
        // A reverse vector exceeding the real reference ceiling is refused
        // before iterating it or allocating the reciprocal index.
        let mut bad = saved;
        bad.entries[0].performance_refs = vec![0; 250_001];
        fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        let bad: PreservationLedger = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            matches!(bad.validate(&allowed), Err(BundleError::InvalidLedger(message)) if message == "performance evidence exceeds bounded storage")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validation_comparisons_do_not_double_charge_saved_json_bytes() {
        let note = "<note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><lyric><text>la</text></lyric></note>";
        let source = crate::engine::musicxml::parse(
            format!(r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes><direction><direction-type><dynamics><p/></dynamics></direction-type></direction>{note}</measure></part></score-partwise>"#).as_bytes(),
        )
        .unwrap();
        let outcome = crate::engine::convert::convert_midi_with_target(
            &source,
            "english",
            None,
            ExportTarget::Svp,
        );
        assert!(outcome.ok, "{:?}", outcome.msg);
        let root = temp_dir("validation-json-comparison-budget");
        let layout = BundleLayout::new(
            &root.join("Comparison.versebundle"),
            "source.xml",
            ExportTarget::Svp,
        )
        .unwrap();
        let stems = StemPlan::from_source(&source, &outcome.tracks).unwrap();
        let allowed = [
            layout.source_relative_path.clone(),
            layout.project_relative_path.clone(),
        ]
        .into_iter()
        .chain(
            stems
                .stems
                .iter()
                .map(|s| layout.stem_audio_relative_path(s)),
        )
        .collect();
        let ledger = build_preservation_ledger(&source, &outcome.projection, &layout, &stems);
        let mut value = serde_json::to_value(ledger).unwrap();
        let intensity = value["performanceSpans"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find_map(|span| {
                let intensity = span.get_mut("intensity")?;
                intensity
                    .get("segments")?
                    .as_array()
                    .is_some_and(|segments| !segments.is_empty())
                    .then_some(intensity)
            })
            .expect("SVP dynamic must emit a segment report");
        let original = intensity["segments"][0]["provenance"].clone();
        let payload = serde_json::Value::String("x".repeat(8 * 1024 * 1024));
        intensity["segments"][0]["provenance"]["evidence"][0]["raw_fields"]
            ["comparison_budget_probe"] = payload.clone();
        let summary = intensity["provenance"].as_array_mut().unwrap();
        let matching = summary
            .iter_mut()
            .find(|item| **item == original)
            .expect("segment provenance must be represented in its summary");
        matching["evidence"][0]["raw_fields"]["comparison_budget_probe"] = payload;
        let saved: PreservationLedger = serde_json::from_value(value).unwrap();
        saved.validate(&allowed).unwrap();
        assert!(matches!(
            saved.validate_with_limits(
                &allowed,
                LedgerValidationLimits {
                    bytes: 8 * 1024 * 1024,
                    ..LedgerValidationLimits::default()
                },
            ),
            Err(BundleError::InvalidLedger(message))
                if message == "performance evidence exceeds bounded storage"
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_source_ids_are_rejected_instead_of_deduplicated() {
        let allowed = BTreeSet::from(["source/source.mid".to_string()]);
        let entry = DispositionEntry {
            performance_refs: Vec::new(),
            source_id: "duplicate".into(),
            item_kind: SourceItemKind::Event,
            disposition: PrimaryDisposition::SourceOnly {
                reason: "test".into(),
            },
            artifact_paths: vec!["source/source.mid".into()],
        };
        let ledger = PreservationLedger {
            intensity_context: None,
            performance_spans: Vec::new(),
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
    fn linked_staff_metadata_is_source_only_and_diagnostics_keep_original_identity() {
        use crate::engine::{convert, musescore};
        let music = "<Measure><voice><Chord><durationType>quarter</durationType><Lyrics><text>word</text></Lyrics><Note><pitch>60</pitch></Note></Chord></voice></Measure>";
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let xml = format!(
                r#"<museScore><Score><Part><Staff id="1"/><Staff id="2"><linkedTo>1</linkedTo></Staff><Staff id="3"><linkedTo>99</linkedTo></Staff></Part><Part><Staff id="9"><linkedTo>99</linkedTo></Staff></Part><Staff id="1">{music}</Staff><Staff id="2">{music}</Staff><Staff id="3">{music}</Staff></Score></museScore>"#
            );
            let midi = musescore::parse_mscx(&xml).unwrap();
            let outcome = convert::convert_midi_with_target(&midi, "english", None, target);
            assert!(outcome.ok);
            let collapsed = outcome
                .source_warnings
                .iter()
                .find(|warning| warning.code == "MUSESCORE_LINKED_VIEW_COLLAPSED")
                .unwrap();
            assert_eq!(collapsed.severity, convert::DiagnosticSeverity::Info);
            assert_eq!(collapsed.source_id.as_deref(), Some("mscx:staff:2"));
            assert_eq!(outcome.source_warnings.len(), 3);
            assert_eq!(
                midi.tracks.len(),
                2,
                "unmatched declarations never make pseudo-tracks"
            );
            let root = temp_dir("linked-source-metadata");
            let layout =
                BundleLayout::new(&root.join("Song.versebundle"), "source.mscx", target).unwrap();
            let stems = StemPlan::from_source(&midi, &outcome.tracks).unwrap();
            assert!(
                !stems.stems.is_empty(),
                "the test must exercise a real stem mapping"
            );
            let mut evidence = outcome.projection.clone();
            evidence
                .source_ids
                .extend(midi.staff_links.iter().map(|link| link.id.clone()));
            let ledger = build_preservation_ledger(&midi, &evidence, &layout, &stems);
            for link in &midi.staff_links {
                let entry = ledger
                    .entries
                    .iter()
                    .find(|entry| entry.source_id == link.id)
                    .unwrap();
                assert_eq!(entry.disposition, PrimaryDisposition::MetadataOnly);
                assert_eq!(
                    entry.artifact_paths,
                    vec![layout.source_relative_path.clone()]
                );
            }
            // Canonical note/event IDs and chronological order are untouched by
            // metadata, as are their dispositions and source evidence IDs.
            let control_xml = xml
                .replace(r#"<Staff id="2"><linkedTo>1</linkedTo></Staff>"#, "")
                .replace(&format!(r#"<Staff id="2">{music}</Staff>"#), "")
                .replace("<linkedTo>99</linkedTo>", "");
            let control = musescore::parse_mscx(&control_xml).unwrap();
            assert_eq!(midi.tracks, control.tracks);
            assert_eq!(midi.topology, control.topology);
            let control_outcome =
                convert::convert_midi_with_target(&control, "english", None, target);
            let control_stems = StemPlan::from_source(&control, &control_outcome.tracks).unwrap();
            let control_ledger = build_preservation_ledger(
                &control,
                &control_outcome.projection,
                &layout,
                &control_stems,
            );
            assert_eq!(outcome.projection, control_outcome.projection);
            for entry in control_ledger.entries {
                assert_eq!(
                    ledger
                        .entries
                        .iter()
                        .find(|candidate| candidate.source_id == entry.source_id),
                    Some(&entry)
                );
            }
            assert_eq!(
                crate::engine::target::serialize_to(target, outcome.svp.as_ref().unwrap()).unwrap(),
                crate::engine::target::serialize_to(target, control_outcome.svp.as_ref().unwrap())
                    .unwrap()
            );
        }
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
        let layout = BundleLayout::new(
            &root.join("Song.versebundle"),
            "source.mid",
            ExportTarget::Svp,
        )
        .unwrap();
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
            score_intensity: None,
            staff_links: Vec::new(),
            ticks_per_beat: 480,
            time_base: TimeBase::PulsesPerQuarter(480),
            format: 1,
            source_format: SourceFormat::StandardMidi,
            topology: SourceTopology::from_tracks(&tracks),
            tracks,
        };
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        let root = temp_dir("metadata-ledger");
        let layout = BundleLayout::new(
            &root.join("Song.versebundle"),
            "source.mid",
            ExportTarget::Svp,
        )
        .unwrap();
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
            score_intensity: None,
            staff_links: Vec::new(),
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
        let layout = BundleLayout::new(
            &root.join("Song.versebundle"),
            "source.kar",
            ExportTarget::Svp,
        )
        .unwrap();
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
        let project = crate::engine::target::svp::serialize(&outcome.svp.unwrap())
            .expect("exactly representable");
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
            let layout =
                BundleLayout::new(&destination, &original_name, ExportTarget::Svp).unwrap();
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
                    project: BundleProject::Svp(
                        crate::engine::target::svp::serialize(
                            &outcome.svp.expect("SVP projection"),
                        )
                        .expect("exactly representable"),
                    ),
                    stem_plan: stem_plan.clone(),
                    ledger,
                    warnings: vec![],
                },
                renderer: Arc::new(FakeRenderer::with_stems(
                    FakeMode::Success,
                    &stem_plan.stems,
                )),
                render_limits: RenderLimits {
                    timeout: std::time::Duration::from_secs(60),
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
        let vocal_tracks = crate::engine::target::svp::serialize(outcome.svp.as_ref().unwrap())
            .expect("exactly representable")
            .tracks
            .len();
        let root = temp_dir("real-bundle-gate");
        let destination = root.join("Real.versebundle");
        let original_name = source_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let layout = BundleLayout::new(&destination, &original_name, ExportTarget::Svp).unwrap();
        let ledger = build_preservation_ledger(&midi, &outcome.projection, &layout, &stem_plan);
        let renderer = MuseScoreRenderer::probe(Path::new(&executable)).unwrap();
        let result = export_bundle(BundleRequest {
            destination,
            input: BundleInput {
                original_name,
                source_format: "museScore".into(),
                source_bytes,
                project: BundleProject::Svp(
                    crate::engine::target::svp::serialize(&outcome.svp.unwrap())
                        .expect("exactly representable"),
                ),
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
