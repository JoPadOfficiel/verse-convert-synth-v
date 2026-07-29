//! Bounded, source-faithful score rendering through a user-installed
//! MuseScore Studio 3.6.2/4 executable with verified score-parts support.
//!
//! The renderer is intentionally a narrow process adapter: the frontend can
//! select an executable, but it cannot supply arguments or invoke arbitrary
//! commands.

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Aggregate deadline for Part extraction, the full reference and every Part
/// stem in one bundle. Real choral/orchestral scores can require many
/// sequential MuseScore renders, so this remains bounded without imposing the
/// old five-minute ceiling on the entire score.
pub const DEFAULT_RENDER_TIMEOUT: Duration = Duration::from_secs(20 * 60);
pub const DEFAULT_MAX_WAV_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_HELP_BYTES: usize = 1024 * 1024;
const MAX_PARTS_JSON_BYTES: usize = 128 * 1024 * 1024;
const MAX_PART_COUNT: usize = 256;
const MAX_PART_MSCZ_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_PART_MSCZ_BYTES: usize = 512 * 1024 * 1024;
const MAX_NATIVE_SCORE_PROLOGUE_BYTES: usize = 1024 * 1024;
// MuseScore PR #31084 documents the macOS shutdown race fixed on upstream
// master: an async Channel destructor reaches an already-destroyed audio
// EngineController mutex and aborts with "mutex lock failed: Invalid argument".
// MuseScore 4 releases before 4.7 can still exhibit it after successful
// console conversion, so those score-loading processes are serialized and
// cooled down. The upstream fix is included in MuseScore 4.7 and later.
// https://github.com/musescore/MuseScore/pull/31084
const MACOS_MUSESCORE4_SCORE_LOAD_COOLDOWN: Duration = Duration::from_secs(10);
const MAX_MACOS_MUSESCORE4_SCORE_LOAD_ATTEMPTS: usize = 3;
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const PIPE_DRAIN_GRACE: Duration = Duration::from_millis(250);
const COMPLETE_STDOUT_EXIT_GRACE: Duration = Duration::from_secs(1);
const TERMINATION_GRACE: Duration = Duration::from_millis(500);
const COMMON_ENVIRONMENT_KEYS: &[&str] = &["SystemRoot", "WINDIR", "LANG", "LC_ALL"];
static PRIVATE_WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
static SCORE_LOAD_PROCESS_GATE: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Clone, Debug)]
pub struct MuseScoreConfig {
    pub executable: Option<PathBuf>,
    pub timeout: Duration,
    pub max_wav_bytes: u64,
}

impl Default for MuseScoreConfig {
    fn default() -> Self {
        Self {
            executable: None,
            timeout: DEFAULT_RENDER_TIMEOUT,
            max_wav_bytes: DEFAULT_MAX_WAV_BYTES,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RendererIdentity {
    pub provider: String,
    pub version: String,
    pub major: u32,
    pub executable_sha256: String,
    pub full_score_mix: bool,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RendererCapabilities {
    pub identity: RendererIdentity,
    pub supported_extensions: Vec<&'static str>,
    pub output_format: &'static str,
    pub score_parts: bool,
}

#[derive(Clone, Debug)]
pub struct RenderLimits {
    pub timeout: Duration,
    pub max_output_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WavInfo {
    pub bytes: u64,
    pub sha256: String,
    pub duration_seconds: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub frames: u64,
}

#[derive(Clone, Debug)]
pub struct RenderedAudio {
    pub path: PathBuf,
    pub wav: WavInfo,
    pub renderer: RendererIdentity,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtractedScorePart {
    pub ordinal: usize,
    pub name: String,
    pub metadata: serde_json::Value,
    pub mscz: Vec<u8>,
}

pub trait AudioRenderer: Send + Sync {
    fn capabilities(&self) -> &RendererCapabilities;

    fn extract_score_parts(
        &self,
        _input: &Path,
        _limits: &RenderLimits,
    ) -> Result<Vec<ExtractedScorePart>, RenderError> {
        Err(RenderError::UnsupportedCapabilities {
            missing: vec!["score-parts".into()],
        })
    }

    fn render(
        &self,
        input: &Path,
        output: &Path,
        limits: &RenderLimits,
    ) -> Result<RenderedAudio, RenderError>;

    /// Render one isolated source Part. A structurally valid silent WAV is
    /// allowed because a source Part can contain only inaudible/unsupported
    /// events; complete reference mixes remain subject to signal validation.
    fn render_part(
        &self,
        input: &Path,
        output: &Path,
        limits: &RenderLimits,
    ) -> Result<RenderedAudio, RenderError> {
        self.render(input, output, limits)
    }
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("MuseScore Studio was not found")]
    NotFound { searched: Vec<String> },
    #[error("configured renderer is not a regular executable file")]
    InvalidExecutable,
    #[error("renderer probe timed out")]
    ProbeTimeout,
    #[error("the configured executable is not MuseScore Studio")]
    ProbeRejected { output: String },
    #[error("MuseScore Studio 3.6.2 or 4 is required (detected: {detected})")]
    UnsupportedVersion { detected: String },
    #[error("renderer is missing required capabilities: {missing:?}")]
    UnsupportedCapabilities { missing: Vec<String> },
    #[error("MuseScore {renderer_major} cannot open a native MuseScore {score_major} score")]
    IncompatibleScore {
        renderer_major: u32,
        score_major: u32,
    },
    #[error("MuseScore score-parts output is invalid: {reason}")]
    InvalidScoreParts { reason: String },
    #[error(
        "renderer executable changed since validation (expected SHA-256 {expected}, observed {observed})"
    )]
    ExecutableChanged { expected: String, observed: String },
    #[error("cannot start renderer: {0}")]
    Spawn(#[source] io::Error),
    #[error("renderer timed out after {milliseconds} ms")]
    Timeout { milliseconds: u64 },
    #[error("renderer exited unsuccessfully ({code:?}): {log}")]
    Exit { code: Option<i32>, log: String },
    #[error("renderer did not create a WAV file")]
    MissingOutput,
    #[error("renderer output exceeded {limit} bytes (observed {bytes})")]
    OutputTooLarge { bytes: u64, limit: u64 },
    #[error("renderer output is not a regular file")]
    OutputIsNotRegularFile,
    #[error("renderer output is not a valid non-empty WAV: {reason}")]
    InvalidWav { reason: String },
    #[error("renderer I/O failed: {0}")]
    Io(#[from] io::Error),
}

pub struct MuseScoreRenderer {
    executable: PathBuf,
    capabilities: RendererCapabilities,
}

impl MuseScoreRenderer {
    pub fn discover(config: &MuseScoreConfig) -> Result<Self, RenderError> {
        if let Some(path) = &config.executable {
            return Self::probe(path);
        }

        let candidates = discovery_candidates();
        let mut searched = Vec::with_capacity(candidates.len());
        let mut incompatible = None;
        for candidate in candidates {
            searched.push(candidate.to_string_lossy().into_owned());
            if !candidate.is_file() {
                continue;
            }
            match Self::probe(&candidate) {
                Ok(renderer) => return Ok(renderer),
                Err(error) => incompatible = Some(error),
            }
        }
        incompatible.map_or_else(|| Err(RenderError::NotFound { searched }), Err)
    }

    pub fn probe(path: &Path) -> Result<Self, RenderError> {
        let executable = fs::canonicalize(path).map_err(|_| RenderError::InvalidExecutable)?;
        let metadata = fs::metadata(&executable).map_err(|_| RenderError::InvalidExecutable)?;
        if !metadata.is_file() || !plausible_musescore_filename(&executable) {
            return Err(RenderError::InvalidExecutable);
        }

        let executable_sha256 = sha256_file(&executable)?;
        let private_work = PrivateWorkDir::create("probe")?;
        let result = run_bounded_process(
            &executable,
            &[OsString::from("--version")],
            private_work.path(),
            PROBE_TIMEOUT,
            None,
            DEFAULT_MAX_WAV_BYTES,
        )
        .map_err(|error| match error {
            ProcessError::Spawn(error) => RenderError::Spawn(error),
            ProcessError::Timeout => RenderError::ProbeTimeout,
            ProcessError::OutputTooLarge { bytes, limit } => {
                RenderError::OutputTooLarge { bytes, limit }
            }
            ProcessError::Io(error) => RenderError::Io(error),
        })?;
        verify_executable_hash(&executable, &executable_sha256)?;
        let output = result.log();
        if !result.status.success() {
            return Err(RenderError::ProbeRejected { output });
        }
        if !output.to_ascii_lowercase().contains("musescore") {
            return Err(RenderError::ProbeRejected { output });
        }
        let version = musescore_version(&output).ok_or_else(|| RenderError::ProbeRejected {
            output: output.clone(),
        })?;
        if !supported_musescore_version(version) {
            return Err(RenderError::UnsupportedVersion { detected: output });
        }
        let major = version.major;
        let help = run_bounded_process_with_capture(
            &executable,
            &[OsString::from("--help")],
            private_work.path(),
            PROBE_TIMEOUT,
            None,
            DEFAULT_MAX_WAV_BYTES,
            MAX_HELP_BYTES,
        )
        .map_err(process_probe_error)?;
        verify_executable_hash(&executable, &executable_sha256)?;
        let help_output = help.log();
        if !help.status.success() {
            return Err(RenderError::ProbeRejected {
                output: help_output,
            });
        }
        let score_parts = help_output.contains("--score-parts");
        if !score_parts {
            return Err(RenderError::UnsupportedCapabilities {
                missing: vec!["score-parts".into()],
            });
        }

        let identity = RendererIdentity {
            provider: "musescore".into(),
            version: output.trim().to_string(),
            major,
            executable_sha256,
            full_score_mix: true,
            capabilities: vec![
                "full-score-wav".into(),
                "score-parts".into(),
                "part-wav".into(),
            ],
        };
        Ok(Self {
            executable,
            capabilities: RendererCapabilities {
                identity,
                supported_extensions: vec![
                    "kar", "mid", "midi", "mxl", "xml", "musicxml", "mscz", "mscx",
                ],
                output_format: "wav",
                score_parts,
            },
        })
    }

    fn render_args(input: &Path, output: &Path) -> Vec<OsString> {
        vec![
            OsString::from("-F"),
            OsString::from("-o"),
            output.as_os_str().to_owned(),
            input.as_os_str().to_owned(),
        ]
    }

    fn score_parts_args(input: &Path) -> Vec<OsString> {
        vec![
            OsString::from("-F"),
            OsString::from("--score-parts"),
            input.as_os_str().to_owned(),
        ]
    }

    fn validate_input(&self, input: &Path) -> Result<(), RenderError> {
        let input_meta = fs::metadata(input)?;
        if !input_meta.is_file() {
            return Err(RenderError::InvalidExecutable);
        }
        let extension = input
            .extension()
            .and_then(OsStr::to_str)
            .map(str::to_ascii_lowercase)
            .ok_or(RenderError::InvalidExecutable)?;
        if !self
            .capabilities
            .supported_extensions
            .contains(&extension.as_str())
        {
            return Err(RenderError::InvalidExecutable);
        }
        if self.capabilities.identity.major == 3 {
            if let Some(score_major) = native_musescore_score_major(input)? {
                if score_major > 3 {
                    return Err(RenderError::IncompatibleScore {
                        renderer_major: 3,
                        score_major,
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct ScoreLoadRetryPolicy {
    enabled: bool,
    cooldown: Duration,
    max_attempts: usize,
}

impl ScoreLoadRetryPolicy {
    fn for_renderer(identity: &RendererIdentity) -> Self {
        let enabled = cfg!(target_os = "macos")
            && identity.major == 4
            && needs_macos_score_load_workaround(&identity.version);
        Self {
            enabled,
            cooldown: MACOS_MUSESCORE4_SCORE_LOAD_COOLDOWN,
            max_attempts: if enabled {
                MAX_MACOS_MUSESCORE4_SCORE_LOAD_ATTEMPTS
            } else {
                1
            },
        }
    }
}

/// Always true on a MuseScore 4 host.
///
/// This used to return false from 4.7.0 onward, on the grounds that upstream
/// PR #31084 fixed the abort there. Measured against 4.7.4 on macOS 15: a bare
/// private home aborts with `mutex lock failed: Invalid argument` in **four of
/// five** consecutive `--score-parts` runs of the same score. The upstream fix
/// narrowed the race but did not close it, and gating the workaround on 4.7 left
/// exactly the newest installations with `max_attempts = 1` and no cooldown — so
/// a bundle export failed almost every time on a current MuseScore.
///
/// Serializing and retrying costs a few seconds on a host that would not have
/// raced; not retrying costs the export. The parameter is kept so the signature
/// still documents that this is version-dependent behaviour upstream may yet fix.
fn needs_macos_score_load_workaround(_version_output: &str) -> bool {
    true
}

fn needs_macos_complete_stdout_workaround(version_output: &str) -> bool {
    musescore_version(version_output).is_some_and(|version| {
        version
            >= (MuseScoreVersion {
                major: 4,
                minor: 7,
                patch: 0,
            })
    })
}

fn can_recover_macos_shutdown_abort(identity: &RendererIdentity, status: &ExitStatus) -> bool {
    cfg!(target_os = "macos")
        && identity.major == 4
        && needs_macos_complete_stdout_workaround(&identity.version)
        && status_is_sigabrt(status)
}

fn render_timeout(timeout: Duration) -> RenderError {
    RenderError::Timeout {
        milliseconds: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
    }
}

fn remaining_render_budget(started: Instant, timeout: Duration) -> Result<Duration, RenderError> {
    timeout
        .checked_sub(started.elapsed())
        .filter(|duration| *duration > Duration::ZERO)
        .ok_or_else(|| render_timeout(timeout))
}

fn acquire_score_load_gate(
    policy: ScoreLoadRetryPolicy,
    started: Instant,
    timeout: Duration,
) -> Result<Option<MutexGuard<'static, Option<Instant>>>, RenderError> {
    if !policy.enabled {
        return Ok(None);
    }
    loop {
        match SCORE_LOAD_PROCESS_GATE.try_lock() {
            Ok(guard) => return Ok(Some(guard)),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                return Ok(Some(poisoned.into_inner()));
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                let remaining = remaining_render_budget(started, timeout)?;
                thread::sleep(POLL_INTERVAL.min(remaining));
            }
        }
    }
}

fn wait_for_score_load_cooldown(
    gate: &Option<MutexGuard<'static, Option<Instant>>>,
    policy: ScoreLoadRetryPolicy,
    started: Instant,
    timeout: Duration,
) -> Result<(), RenderError> {
    if !policy.enabled {
        return Ok(());
    }
    let Some(last_finished) = gate.as_ref().and_then(|guard| **guard) else {
        return Ok(());
    };
    let wait = policy.cooldown.saturating_sub(last_finished.elapsed());
    if wait.is_zero() {
        return Ok(());
    }
    let remaining = remaining_render_budget(started, timeout)?;
    if wait >= remaining {
        return Err(render_timeout(timeout));
    }
    thread::sleep(wait);
    Ok(())
}

fn record_score_load_finished(gate: &mut Option<MutexGuard<'static, Option<Instant>>>) {
    if let Some(guard) = gate {
        **guard = Some(Instant::now());
    }
}

fn status_is_sigabrt(status: &ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal() == Some(libc::SIGABRT)
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        false
    }
}

fn remove_failed_renderer_output(output: &Path) -> Result<(), RenderError> {
    match fs::symlink_metadata(output) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(output)?;
        }
        Ok(_) => return Err(RenderError::OutputIsNotRegularFile),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(RenderError::Io(error)),
    }
    Ok(())
}

impl MuseScoreRenderer {
    fn extract_score_parts_with_policy(
        &self,
        input: &Path,
        limits: &RenderLimits,
        policy: ScoreLoadRetryPolicy,
    ) -> Result<Vec<ExtractedScorePart>, RenderError> {
        self.validate_input(input)?;
        if !self.capabilities.score_parts {
            return Err(RenderError::UnsupportedCapabilities {
                missing: vec!["score-parts".into()],
            });
        }
        verify_executable_hash(
            &self.executable,
            &self.capabilities.identity.executable_sha256,
        )?;

        let started = Instant::now();
        let mut gate = acquire_score_load_gate(policy, started, limits.timeout)?;
        wait_for_score_load_cooldown(&gate, policy, started, limits.timeout)?;
        for attempt in 0..policy.max_attempts {
            let remaining = remaining_render_budget(started, limits.timeout)?;
            let private_work = PrivateWorkDir::create("score-parts")?;
            let process_result = if cfg!(target_os = "macos")
                && needs_macos_complete_stdout_workaround(&self.capabilities.identity.version)
            {
                run_bounded_process_with_capture_until_complete_stdout(
                    &self.executable,
                    &Self::score_parts_args(input),
                    private_work.path(),
                    remaining,
                    None,
                    limits.max_output_bytes,
                    MAX_PARTS_JSON_BYTES,
                    score_parts_json_is_complete,
                )
            } else {
                run_bounded_process_with_capture(
                    &self.executable,
                    &Self::score_parts_args(input),
                    private_work.path(),
                    remaining,
                    None,
                    limits.max_output_bytes,
                    MAX_PARTS_JSON_BYTES,
                )
            };
            record_score_load_finished(&mut gate);
            let result =
                process_result.map_err(|error| process_render_error(error, limits.timeout))?;
            verify_executable_hash(
                &self.executable,
                &self.capabilities.identity.executable_sha256,
            )?;
            let payload_is_fully_valid = parse_score_parts_json(&result.stdout).is_ok();
            if result.status.success()
                || result.completed_from_stdout
                || (payload_is_fully_valid
                    && can_recover_macos_shutdown_abort(
                        &self.capabilities.identity,
                        &result.status,
                    ))
            {
                return parse_score_parts_json(&result.stdout);
            }

            let retryable_sigabrt = policy.enabled
                && status_is_sigabrt(&result.status)
                && payload_is_fully_valid
                && attempt + 1 < policy.max_attempts;
            if !retryable_sigabrt {
                return Err(RenderError::Exit {
                    code: result.status.code(),
                    log: result.failure_log(),
                });
            }
            wait_for_score_load_cooldown(&gate, policy, started, limits.timeout)?;
        }
        unreachable!("bounded score-parts attempt loop always returns")
    }

    fn render_with_policy(
        &self,
        input: &Path,
        output: &Path,
        limits: &RenderLimits,
        policy: ScoreLoadRetryPolicy,
        allow_silence: bool,
    ) -> Result<RenderedAudio, RenderError> {
        self.validate_input(input)?;
        if output.exists() {
            return Err(RenderError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "renderer output already exists",
            )));
        }
        verify_executable_hash(
            &self.executable,
            &self.capabilities.identity.executable_sha256,
        )?;
        let renderer_identity = self.capabilities.identity.clone();
        let work_dir = output.parent().ok_or_else(|| {
            RenderError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output has no parent directory",
            ))
        })?;

        let started = Instant::now();
        let mut gate = acquire_score_load_gate(policy, started, limits.timeout)?;
        wait_for_score_load_cooldown(&gate, policy, started, limits.timeout)?;
        for attempt in 0..policy.max_attempts {
            let remaining = remaining_render_budget(started, limits.timeout)?;
            let process_result = run_bounded_process(
                &self.executable,
                &Self::render_args(input, output),
                work_dir,
                remaining,
                Some(output),
                limits.max_output_bytes,
            );
            record_score_load_finished(&mut gate);
            let result =
                process_result.map_err(|error| process_render_error(error, limits.timeout))?;
            verify_executable_hash(&self.executable, &renderer_identity.executable_sha256)?;
            if result.status.success() {
                if !output.exists() {
                    return Err(RenderError::MissingOutput);
                }
                let wav = validate_wav_with_policy(output, limits.max_output_bytes, allow_silence)?;
                return Ok(RenderedAudio {
                    path: output.to_path_buf(),
                    wav,
                    renderer: renderer_identity,
                });
            }

            let was_sigabrt = status_is_sigabrt(&result.status);
            if can_recover_macos_shutdown_abort(&renderer_identity, &result.status)
                && output.exists()
            {
                if let Ok(wav) =
                    validate_wav_with_policy(output, limits.max_output_bytes, allow_silence)
                {
                    return Ok(RenderedAudio {
                        path: output.to_path_buf(),
                        wav,
                        renderer: renderer_identity,
                    });
                }
            }
            if was_sigabrt {
                remove_failed_renderer_output(output)?;
            }
            let retryable_sigabrt =
                policy.enabled && was_sigabrt && attempt + 1 < policy.max_attempts;
            if !retryable_sigabrt {
                return Err(RenderError::Exit {
                    code: result.status.code(),
                    log: result.failure_log(),
                });
            }
            wait_for_score_load_cooldown(&gate, policy, started, limits.timeout)?;
        }
        unreachable!("bounded render attempt loop always returns")
    }
}

impl AudioRenderer for MuseScoreRenderer {
    fn capabilities(&self) -> &RendererCapabilities {
        &self.capabilities
    }

    fn extract_score_parts(
        &self,
        input: &Path,
        limits: &RenderLimits,
    ) -> Result<Vec<ExtractedScorePart>, RenderError> {
        self.extract_score_parts_with_policy(
            input,
            limits,
            ScoreLoadRetryPolicy::for_renderer(&self.capabilities.identity),
        )
    }

    fn render(
        &self,
        input: &Path,
        output: &Path,
        limits: &RenderLimits,
    ) -> Result<RenderedAudio, RenderError> {
        self.render_with_policy(
            input,
            output,
            limits,
            ScoreLoadRetryPolicy::for_renderer(&self.capabilities.identity),
            false,
        )
    }

    fn render_part(
        &self,
        input: &Path,
        output: &Path,
        limits: &RenderLimits,
    ) -> Result<RenderedAudio, RenderError> {
        self.render_with_policy(
            input,
            output,
            limits,
            ScoreLoadRetryPolicy::for_renderer(&self.capabilities.identity),
            true,
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScorePartsEnvelope {
    parts: Vec<String>,
    parts_bin: Vec<String>,
    #[serde(default)]
    parts_meta: Vec<serde_json::Value>,
}

fn parse_score_parts_json(bytes: &[u8]) -> Result<Vec<ExtractedScorePart>, RenderError> {
    if bytes.is_empty() || bytes.len() > MAX_PARTS_JSON_BYTES {
        return Err(RenderError::InvalidScoreParts {
            reason: "empty or oversized JSON response".into(),
        });
    }
    let envelope: ScorePartsEnvelope =
        serde_json::from_slice(bytes).map_err(|error| RenderError::InvalidScoreParts {
            reason: format!("invalid JSON: {error}"),
        })?;
    if envelope.parts.is_empty() || envelope.parts.len() > MAX_PART_COUNT {
        return Err(RenderError::InvalidScoreParts {
            reason: format!(
                "part count {} is outside 1..={MAX_PART_COUNT}",
                envelope.parts.len()
            ),
        });
    }
    if envelope.parts.len() != envelope.parts_bin.len()
        || (!envelope.parts_meta.is_empty() && envelope.parts.len() != envelope.parts_meta.len())
    {
        return Err(RenderError::InvalidScoreParts {
            reason: "parts, partsBin and partsMeta lengths differ".into(),
        });
    }

    let mut total = 0usize;
    let mut result = Vec::with_capacity(envelope.parts.len());
    for (ordinal, (name, encoded)) in envelope
        .parts
        .into_iter()
        .zip(envelope.parts_bin)
        .enumerate()
    {
        let name = sanitize_part_display_name(&name);
        if name.is_empty() {
            return Err(RenderError::InvalidScoreParts {
                reason: format!("part {ordinal} has an empty display name"),
            });
        }
        let decoded_upper_bound = encoded.len().saturating_add(3) / 4 * 3;
        if decoded_upper_bound > MAX_PART_MSCZ_BYTES {
            return Err(RenderError::InvalidScoreParts {
                reason: format!("part {ordinal} exceeds the decoded-size limit"),
            });
        }
        let mscz = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(|error| RenderError::InvalidScoreParts {
                reason: format!("part {ordinal} is not valid base64: {error}"),
            })?;
        if mscz.is_empty() || mscz.len() > MAX_PART_MSCZ_BYTES {
            return Err(RenderError::InvalidScoreParts {
                reason: format!("part {ordinal} has an invalid decoded size"),
            });
        }
        validate_part_mscz(&mscz, ordinal)?;
        total = total
            .checked_add(mscz.len())
            .ok_or_else(|| RenderError::InvalidScoreParts {
                reason: "decoded part-size overflow".into(),
            })?;
        if total > MAX_TOTAL_PART_MSCZ_BYTES {
            return Err(RenderError::InvalidScoreParts {
                reason: "decoded parts exceed the aggregate-size limit".into(),
            });
        }
        result.push(ExtractedScorePart {
            ordinal,
            name,
            metadata: envelope
                .parts_meta
                .get(ordinal)
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            mscz,
        });
    }
    Ok(result)
}

fn sanitize_part_display_name(value: &str) -> String {
    let mut sanitized = String::new();
    let mut separator_pending = false;
    for character in value.chars().take(1024) {
        if character.is_control() || character.is_whitespace() {
            separator_pending = true;
        } else {
            if separator_pending && !sanitized.is_empty() {
                sanitized.push(' ');
            }
            sanitized.push(character);
            separator_pending = false;
        }
    }
    sanitized.trim().to_string()
}

fn validate_part_mscz(bytes: &[u8], ordinal: usize) -> Result<(), RenderError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        RenderError::InvalidScoreParts {
            reason: format!("part {ordinal} is not an MSCZ archive: {error}"),
        }
    })?;
    if archive.is_empty() || archive.len() > 128 {
        return Err(RenderError::InvalidScoreParts {
            reason: format!("part {ordinal} has an invalid archive entry count"),
        });
    }
    let mut score_paths = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| RenderError::InvalidScoreParts {
                reason: format!("part {ordinal} archive cannot be read: {error}"),
            })?;
        if entry.enclosed_name().is_none() {
            return Err(RenderError::InvalidScoreParts {
                reason: format!("part {ordinal} contains an unsafe archive path"),
            });
        }
        if !entry.is_dir() && entry.name().to_ascii_lowercase().ends_with(".mscx") {
            score_paths.push(entry.name().to_string());
        }
    }
    if score_paths.len() != 1 {
        return Err(RenderError::InvalidScoreParts {
            reason: format!(
                "part {ordinal} must contain exactly one unambiguous master MSCX score (found {})",
                score_paths.len()
            ),
        });
    }
    Ok(())
}

fn process_probe_error(error: ProcessError) -> RenderError {
    match error {
        ProcessError::Spawn(error) => RenderError::Spawn(error),
        ProcessError::Timeout => RenderError::ProbeTimeout,
        ProcessError::OutputTooLarge { bytes, limit } => {
            RenderError::OutputTooLarge { bytes, limit }
        }
        ProcessError::Io(error) => RenderError::Io(error),
    }
}

fn process_render_error(error: ProcessError, timeout: Duration) -> RenderError {
    match error {
        ProcessError::Spawn(error) => RenderError::Spawn(error),
        ProcessError::Timeout => RenderError::Timeout {
            milliseconds: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
        },
        ProcessError::OutputTooLarge { bytes, limit } => {
            RenderError::OutputTooLarge { bytes, limit }
        }
        ProcessError::Io(error) => RenderError::Io(error),
    }
}

fn native_musescore_score_major(path: &Path) -> Result<Option<u32>, RenderError> {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("mscx") => {
            let file = fs::File::open(path)?;
            read_native_musescore_major(file, "native MSCX score")
        }
        Some("mscz") => {
            let file = fs::File::open(path)?;
            let mut archive =
                zip::ZipArchive::new(file).map_err(|error| RenderError::InvalidScoreParts {
                    reason: format!("cannot inspect native MuseScore package: {error}"),
                })?;
            if archive.len() > 4096 {
                return Err(RenderError::InvalidScoreParts {
                    reason: "native MuseScore package has too many entries".into(),
                });
            }
            let mut detected = None;
            for index in 0..archive.len() {
                let mut entry =
                    archive
                        .by_index(index)
                        .map_err(|error| RenderError::InvalidScoreParts {
                            reason: format!("cannot inspect native MuseScore entry: {error}"),
                        })?;
                if entry.enclosed_name().is_none()
                    || !entry.name().to_ascii_lowercase().ends_with(".mscx")
                {
                    continue;
                }
                if let Some(major) =
                    read_native_musescore_major(&mut entry, "native MSCZ score entry")?
                {
                    detected = Some(detected.map_or(major, |current: u32| current.max(major)));
                }
            }
            Ok(detected)
        }
        _ => Ok(None),
    }
}

fn read_native_musescore_major<R: Read>(
    reader: R,
    source: &str,
) -> Result<Option<u32>, RenderError> {
    let mut bytes = Vec::with_capacity(MAX_NATIVE_SCORE_PROLOGUE_BYTES.min(64 * 1024));
    reader
        .take((MAX_NATIVE_SCORE_PROLOGUE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if let Some(major) = musescore_document_major(&bytes) {
        return Ok(Some(major));
    }
    if bytes.len() > MAX_NATIVE_SCORE_PROLOGUE_BYTES {
        return Err(RenderError::InvalidScoreParts {
            reason: format!(
                "{source} has no MuseScore document root within the bounded {}-byte prologue",
                MAX_NATIVE_SCORE_PROLOGUE_BYTES
            ),
        });
    }
    Ok(None)
}

fn musescore_document_major(bytes: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(bytes).ok()?;
    let root = xml_root_start_tag(text)?;
    let (name, attributes) = split_xml_element_name(root)?;
    if name != "museScore" {
        return None;
    }
    xml_attribute(attributes, "version")?
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn xml_root_start_tag(mut text: &str) -> Option<&str> {
    text = text.strip_prefix('\u{feff}').unwrap_or(text);
    loop {
        text = text.trim_start();
        if let Some(rest) = text.strip_prefix("<?") {
            text = rest.split_once("?>")?.1;
            continue;
        }
        if let Some(rest) = text.strip_prefix("<!--") {
            text = rest.split_once("-->")?.1;
            continue;
        }
        if text.starts_with("<!") {
            // MuseScore documents do not require declarations. Refusing them
            // keeps root detection deterministic instead of interpreting an
            // attacker-controlled DTD or a fake tag inside one.
            return None;
        }
        let rest = text.strip_prefix('<')?;
        let mut quote = None;
        for (index, character) in rest.char_indices() {
            match (quote, character) {
                (Some(expected), found) if expected == found => quote = None,
                (None, '"' | '\'') => quote = Some(character),
                (None, '>') => return Some(&rest[..index]),
                _ => {}
            }
        }
        return None;
    }
}

fn split_xml_element_name(tag: &str) -> Option<(&str, &str)> {
    let end = tag
        .find(|character: char| character.is_whitespace() || character == '/')
        .unwrap_or(tag.len());
    let name = &tag[..end];
    (!name.is_empty()).then_some((name, &tag[end..]))
}

fn xml_attribute<'a>(mut attributes: &'a str, requested: &str) -> Option<&'a str> {
    loop {
        attributes = attributes.trim_start();
        if attributes.is_empty() || attributes.starts_with('/') {
            return None;
        }
        let name_end = attributes.find(|character: char| {
            character.is_whitespace() || character == '=' || character == '/'
        })?;
        let name = &attributes[..name_end];
        attributes = attributes[name_end..].trim_start();
        attributes = attributes.strip_prefix('=')?.trim_start();
        let quote = attributes.chars().next()?;
        if !matches!(quote, '"' | '\'') {
            return None;
        }
        let value_start = quote.len_utf8();
        let value_end = attributes[value_start..].find(quote)? + value_start;
        let value = &attributes[value_start..value_end];
        attributes = &attributes[value_end + quote.len_utf8()..];
        if name == requested {
            return Some(value);
        }
    }
}

pub fn validate_wav(path: &Path, max_bytes: u64) -> Result<WavInfo, RenderError> {
    validate_wav_with_policy(path, max_bytes, false)
}

pub fn validate_wav_allowing_silence(path: &Path, max_bytes: u64) -> Result<WavInfo, RenderError> {
    validate_wav_with_policy(path, max_bytes, true)
}

fn validate_wav_with_policy(
    path: &Path,
    max_bytes: u64,
    allow_silence: bool,
) -> Result<WavInfo, RenderError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            RenderError::MissingOutput
        } else {
            RenderError::Io(error)
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(RenderError::OutputIsNotRegularFile);
    }
    let bytes = metadata.len();
    if bytes == 0 {
        return Err(RenderError::InvalidWav {
            reason: "empty file".into(),
        });
    }
    if bytes > max_bytes {
        return Err(RenderError::OutputTooLarge {
            bytes,
            limit: max_bytes,
        });
    }
    let mut reader = hound::WavReader::open(path).map_err(|error| RenderError::InvalidWav {
        reason: error.to_string(),
    })?;
    let spec = reader.spec();
    let frames = u64::from(reader.duration());
    if spec.sample_rate == 0 || spec.channels == 0 || frames == 0 {
        return Err(RenderError::InvalidWav {
            reason: "zero sample rate, channel count, or frame count".into(),
        });
    }
    let duration_seconds = frames as f64 / f64::from(spec.sample_rate);
    if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return Err(RenderError::InvalidWav {
            reason: "invalid duration".into(),
        });
    }
    let mut decoded_samples = 0_u64;
    let mut sum_squares = 0.0_f64;
    let mut peak = 0.0_f64;
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for sample in reader.samples::<f32>() {
                let sample = sample.map_err(|error| RenderError::InvalidWav {
                    reason: format!("truncated or invalid sample data: {error}"),
                })?;
                accumulate_sample_energy(
                    f64::from(sample),
                    &mut decoded_samples,
                    &mut sum_squares,
                    &mut peak,
                )?;
            }
        }
        hound::SampleFormat::Int => {
            if !(1..=32).contains(&spec.bits_per_sample) {
                return Err(RenderError::InvalidWav {
                    reason: format!(
                        "unsupported integer sample width: {} bits",
                        spec.bits_per_sample
                    ),
                });
            }
            let full_scale = (1_u64 << (spec.bits_per_sample - 1)) as f64;
            for sample in reader.samples::<i32>() {
                let sample = sample.map_err(|error| RenderError::InvalidWav {
                    reason: format!("truncated or invalid sample data: {error}"),
                })?;
                accumulate_sample_energy(
                    f64::from(sample) / full_scale,
                    &mut decoded_samples,
                    &mut sum_squares,
                    &mut peak,
                )?;
            }
        }
    }
    let expected_samples = frames
        .checked_mul(u64::from(spec.channels))
        .ok_or_else(|| RenderError::InvalidWav {
            reason: "sample count overflow".into(),
        })?;
    if decoded_samples != expected_samples {
        return Err(RenderError::InvalidWav {
            reason: format!(
                "sample count mismatch (decoded {decoded_samples}, expected {expected_samples})"
            ),
        });
    }
    let rms = (sum_squares / decoded_samples as f64).sqrt();
    if !allow_silence && (peak == 0.0 || rms == 0.0) {
        return Err(RenderError::InvalidWav {
            reason: "audio samples contain no non-zero signal energy (WAV is silent)".into(),
        });
    }
    Ok(WavInfo {
        bytes,
        sha256: sha256_file(path)?,
        duration_seconds,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        bits_per_sample: spec.bits_per_sample,
        frames,
    })
}

fn accumulate_sample_energy(
    sample: f64,
    decoded_samples: &mut u64,
    sum_squares: &mut f64,
    peak: &mut f64,
) -> Result<(), RenderError> {
    if !sample.is_finite() {
        return Err(RenderError::InvalidWav {
            reason: "audio samples contain a non-finite value".into(),
        });
    }
    *decoded_samples = decoded_samples
        .checked_add(1)
        .ok_or_else(|| RenderError::InvalidWav {
            reason: "decoded sample count overflow".into(),
        })?;
    let magnitude = sample.abs();
    *peak = peak.max(magnitude);
    *sum_squares += sample * sample;
    if !sum_squares.is_finite() {
        return Err(RenderError::InvalidWav {
            reason: "audio signal energy overflow".into(),
        });
    }
    Ok(())
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> Result<String, RenderError> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_executable_hash(path: &Path, expected: &str) -> Result<(), RenderError> {
    let observed = sha256_file(path)?;
    if observed != expected {
        return Err(RenderError::ExecutableChanged {
            expected: expected.to_owned(),
            observed,
        });
    }
    Ok(())
}

#[derive(Debug)]
struct ProcessResult {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    completed_from_stdout: bool,
}

impl ProcessResult {
    fn log(&self) -> String {
        bounded_combined_log(&self.stdout, &self.stderr)
    }

    fn failure_log(&self) -> String {
        // stderr carries Crashpad/Qt diagnostics, while score-parts stdout can
        // be tens of megabytes of base64. Put stderr first so bounded logs
        // retain the actual failure instead of truncating it away.
        let log = bounded_combined_log(&self.stderr, &self.stdout);
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = self.status.signal() {
                return format!("terminated by Unix signal {signal}: {log}");
            }
        }
        log
    }
}

fn bounded_combined_log(first: &[u8], second: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(first.len().saturating_add(second.len()).min(MAX_LOG_BYTES));
    bytes.extend_from_slice(&first[..first.len().min(MAX_LOG_BYTES)]);
    if !bytes.is_empty() && !second.is_empty() && bytes.len() < MAX_LOG_BYTES {
        bytes.push(b'\n');
    }
    let remaining = MAX_LOG_BYTES.saturating_sub(bytes.len());
    bytes.extend_from_slice(&second[..second.len().min(remaining)]);
    String::from_utf8_lossy(&bytes).into_owned()
}

#[derive(Debug)]
enum ProcessError {
    Spawn(io::Error),
    Timeout,
    OutputTooLarge { bytes: u64, limit: u64 },
    Io(io::Error),
}

#[derive(Clone, Copy, Debug)]
enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug)]
enum DrainEvent {
    Data {
        stream: LogStream,
        bytes: Vec<u8>,
    },
    Finished {
        stream: LogStream,
        result: io::Result<()>,
    },
    LimitExceeded {
        bytes: u64,
        limit: u64,
    },
}

struct ProcessOutputCollector {
    receiver: mpsc::Receiver<DrainEvent>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    capture_limit: usize,
    stdout_finished: bool,
    stderr_finished: bool,
    error: Option<io::Error>,
    overflow: Option<(u64, u64)>,
}

impl ProcessOutputCollector {
    fn new(receiver: mpsc::Receiver<DrainEvent>, capture_limit: usize) -> Self {
        Self {
            receiver,
            stdout: Vec::new(),
            stderr: Vec::new(),
            capture_limit,
            stdout_finished: false,
            stderr_finished: false,
            error: None,
            overflow: None,
        }
    }

    fn handle(&mut self, event: DrainEvent) {
        match event {
            DrainEvent::Data { stream, bytes } => {
                let destination = match stream {
                    LogStream::Stdout => &mut self.stdout,
                    LogStream::Stderr => &mut self.stderr,
                };
                let remaining = self.capture_limit.saturating_sub(destination.len());
                destination.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
            }
            DrainEvent::Finished { stream, result } => {
                match stream {
                    LogStream::Stdout => self.stdout_finished = true,
                    LogStream::Stderr => self.stderr_finished = true,
                }
                if let Err(error) = result {
                    self.error.get_or_insert(error);
                }
            }
            DrainEvent::LimitExceeded { bytes, limit } => {
                self.overflow.get_or_insert((bytes, limit));
            }
        }
    }

    fn poll(&mut self) {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => self.handle(event),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if !self.finished() {
                        self.error.get_or_insert_with(|| {
                            io::Error::other("renderer log reader stopped unexpectedly")
                        });
                        self.stdout_finished = true;
                        self.stderr_finished = true;
                    }
                    break;
                }
            }
        }
    }

    fn wait_until(&mut self, deadline: Instant) -> bool {
        self.poll();
        while !self.finished() {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let wait = deadline.saturating_duration_since(now).min(POLL_INTERVAL);
            match self.receiver.recv_timeout(wait) {
                Ok(event) => self.handle(event),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.poll();
                    break;
                }
            }
        }
        self.finished()
    }

    fn finished(&self) -> bool {
        self.stdout_finished && self.stderr_finished
    }

    fn take_error(&mut self) -> Option<io::Error> {
        self.poll();
        self.error.take()
    }

    fn take_overflow(&mut self) -> Option<(u64, u64)> {
        self.poll();
        self.overflow.take()
    }
}

fn run_bounded_process(
    program: &Path,
    args: &[OsString],
    current_dir: &Path,
    timeout: Duration,
    monitored_output: Option<&Path>,
    max_output_bytes: u64,
) -> Result<ProcessResult, ProcessError> {
    run_bounded_process_with_capture(
        program,
        args,
        current_dir,
        timeout,
        monitored_output,
        max_output_bytes,
        MAX_LOG_BYTES,
    )
}

fn run_bounded_process_with_capture(
    program: &Path,
    args: &[OsString],
    current_dir: &Path,
    timeout: Duration,
    monitored_output: Option<&Path>,
    max_output_bytes: u64,
    capture_limit: usize,
) -> Result<ProcessResult, ProcessError> {
    run_bounded_process_with_capture_policy(
        program,
        args,
        current_dir,
        timeout,
        monitored_output,
        max_output_bytes,
        capture_limit,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_bounded_process_with_capture_until_complete_stdout(
    program: &Path,
    args: &[OsString],
    current_dir: &Path,
    timeout: Duration,
    monitored_output: Option<&Path>,
    max_output_bytes: u64,
    capture_limit: usize,
    complete_stdout: fn(&[u8]) -> bool,
) -> Result<ProcessResult, ProcessError> {
    run_bounded_process_with_capture_policy(
        program,
        args,
        current_dir,
        timeout,
        monitored_output,
        max_output_bytes,
        capture_limit,
        Some(complete_stdout),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_bounded_process_with_capture_policy(
    program: &Path,
    args: &[OsString],
    current_dir: &Path,
    timeout: Duration,
    monitored_output: Option<&Path>,
    max_output_bytes: u64,
    capture_limit: usize,
    complete_stdout: Option<fn(&[u8]) -> bool>,
) -> Result<ProcessResult, ProcessError> {
    if capture_limit == 0 {
        return Err(ProcessError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture limit must be positive",
        )));
    }
    for directory in [
        "config",
        "cache",
        "data",
        "state",
        "runtime",
        "appdata",
        "localappdata",
        "tmp",
    ] {
        fs::create_dir_all(current_dir.join(directory)).map_err(ProcessError::Io)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            current_dir.join("runtime"),
            fs::Permissions::from_mode(0o700),
        )
        .map_err(ProcessError::Io)?;
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(current_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    copy_required_environment(&mut command, current_dir);

    let mut child = command.spawn().map_err(ProcessError::Spawn)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProcessError::Io(io::Error::other("renderer stdout unavailable")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProcessError::Io(io::Error::other("renderer stderr unavailable")))?;
    let (drain_sender, drain_receiver) = mpsc::channel();
    spawn_bounded_drain(
        stdout,
        capture_limit,
        LogStream::Stdout,
        drain_sender.clone(),
    );
    spawn_bounded_drain(stderr, capture_limit, LogStream::Stderr, drain_sender);
    let mut output_collector = ProcessOutputCollector::new(drain_receiver, capture_limit);

    let started = Instant::now();
    let mut complete_stdout_seen_at = None;
    let mut completed_from_stdout = false;
    let status = loop {
        output_collector.poll();
        if let Some(error) = output_collector.take_error() {
            terminate_and_reap(&mut child, &mut output_collector);
            return Err(ProcessError::Io(error));
        }
        if let Some((bytes, limit)) = output_collector.take_overflow() {
            terminate_and_reap(&mut child, &mut output_collector);
            return Err(ProcessError::OutputTooLarge { bytes, limit });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                terminate_and_reap(&mut child, &mut output_collector);
                return Err(ProcessError::Io(error));
            }
        }
        if let Some(is_complete) = complete_stdout {
            if complete_stdout_seen_at.is_none() && is_complete(&output_collector.stdout) {
                complete_stdout_seen_at = Some(Instant::now());
            }
            if complete_stdout_seen_at
                .is_some_and(|seen_at| seen_at.elapsed() >= COMPLETE_STDOUT_EXIT_GRACE)
            {
                terminate_process_tree(&mut child);
                let deadline = Instant::now() + TERMINATION_GRACE;
                if !wait_for_child_until(&mut child, deadline).map_err(ProcessError::Io)? {
                    return Err(ProcessError::Io(io::Error::other(
                        "renderer remained alive after complete stdout termination",
                    )));
                }
                completed_from_stdout = true;
                break child.try_wait().map_err(ProcessError::Io)?.ok_or_else(|| {
                    ProcessError::Io(io::Error::other("renderer termination status unavailable"))
                })?;
            }
        }
        if let Some(output) = monitored_output {
            if let Ok(metadata) = fs::metadata(output) {
                if metadata.len() > max_output_bytes {
                    terminate_and_reap(&mut child, &mut output_collector);
                    return Err(ProcessError::OutputTooLarge {
                        bytes: metadata.len(),
                        limit: max_output_bytes,
                    });
                }
            }
        }
        if started.elapsed() >= timeout {
            terminate_and_reap(&mut child, &mut output_collector);
            return Err(ProcessError::Timeout);
        }
        thread::sleep(POLL_INTERVAL);
    };

    if !output_collector.wait_until(Instant::now() + PIPE_DRAIN_GRACE) {
        // A renderer parent can exit while a detached descendant still owns
        // the inherited pipes. Do not join reader threads without a deadline.
        // Kill the process tree where the platform permits it, then retain
        // whatever bounded output was received.
        terminate_descendants_after_parent_exit(child.id());
        let deadline = Instant::now() + TERMINATION_GRACE;
        let _ = output_collector.wait_until(deadline);
    }
    if let Some(error) = output_collector.take_error() {
        return Err(ProcessError::Io(error));
    }
    if let Some((bytes, limit)) = output_collector.take_overflow() {
        return Err(ProcessError::OutputTooLarge { bytes, limit });
    }
    Ok(ProcessResult {
        status,
        stdout: output_collector.stdout,
        stderr: output_collector.stderr,
        completed_from_stdout,
    })
}

fn score_parts_json_is_complete(bytes: &[u8]) -> bool {
    let Some(last_non_whitespace) = bytes.iter().rfind(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    *last_non_whitespace == b'}' && serde_json::from_slice::<ScorePartsEnvelope>(bytes).is_ok()
}

fn terminate_descendants_after_parent_exit(parent_id: u32) {
    #[cfg(unix)]
    {
        // The process group remains addressable while an ordinary descendant
        // still belongs to it, even though the original leader has exited.
        let process_group = -(parent_id as i32);
        // SAFETY: the group id was created for this renderer invocation and a
        // constant signal is used. There is no equivalent safe std operation.
        unsafe {
            libc::kill(process_group, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = parent_id;
}

fn terminate_and_reap(
    child: &mut std::process::Child,
    output_collector: &mut ProcessOutputCollector,
) {
    terminate_process_tree(child);
    let deadline = Instant::now() + TERMINATION_GRACE;
    let _ = wait_for_child_until(child, deadline);
    let _ = output_collector.wait_until(deadline);
}

fn wait_for_child_until(child: &mut std::process::Child, deadline: Instant) -> io::Result<bool> {
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        thread::sleep(deadline.saturating_duration_since(now).min(POLL_INTERVAL));
    }
}

fn terminate_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // The renderer is started as a new process group. Killing the group
        // also closes pipes inherited by ordinary descendants, so timeout
        // handling cannot block forever while joining the drain threads.
        let process_group = -(child.id() as i32);
        // SAFETY: `kill` is called with the process-group id created above
        // and a constant signal. Failure is harmless; `Child::kill` below is
        // retained as a direct-child fallback.
        unsafe {
            libc::kill(process_group, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let system_root = std::env::var_os("SystemRoot")
            .or_else(|| std::env::var_os("WINDIR"))
            .filter(|value| Path::new(value).is_absolute());
        if let Some(system_root) = system_root {
            let taskkill = PathBuf::from(system_root).join("System32/taskkill.exe");
            if let Ok(mut killer) = Command::new(taskkill)
                .args([
                    OsString::from("/PID"),
                    OsString::from(child.id().to_string()),
                    OsString::from("/T"),
                    OsString::from("/F"),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                let deadline = Instant::now() + TERMINATION_GRACE;
                let _ = wait_for_child_until(&mut killer, deadline);
                let _ = killer.kill();
                let _ = killer.try_wait();
            }
        }
    }
    let _ = child.kill();
}

fn spawn_bounded_drain<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
    stream: LogStream,
    sender: mpsc::Sender<DrainEvent>,
) {
    thread::spawn(move || {
        let mut kept = 0_usize;
        let mut total = 0_u64;
        let mut limit_reported = false;
        let mut buffer = [0_u8; 8192];
        let result = loop {
            match reader.read(&mut buffer) {
                Ok(0) => break Ok(()),
                Ok(count) => {
                    total = total.saturating_add(count as u64);
                    if total > limit as u64 && !limit_reported {
                        limit_reported = true;
                        if sender
                            .send(DrainEvent::LimitExceeded {
                                bytes: total,
                                limit: limit as u64,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    let remaining = limit.saturating_sub(kept);
                    let retained = count.min(remaining);
                    if retained > 0 {
                        if sender
                            .send(DrainEvent::Data {
                                stream,
                                bytes: buffer[..retained].to_vec(),
                            })
                            .is_err()
                        {
                            return;
                        }
                        kept += retained;
                    }
                }
                Err(error) => break Err(error),
            }
        };
        let _ = sender.send(DrainEvent::Finished { stream, result });
    });
}

fn copy_required_environment(command: &mut Command, private_dir: &Path) {
    copy_environment_keys(command, COMMON_ENVIRONMENT_KEYS, |key| {
        std::env::var_os(key)
    });
    #[cfg(target_os = "linux")]
    copy_linux_session_environment(command, |key| std::env::var_os(key), private_dir);

    command.env("HOME", private_dir);
    // CoreFoundation does not consistently derive NSSearchPathDirectory
    // locations from HOME on macOS. Without this override, MuseScore can still
    // scan the user's real Documents/MuseScore4/Plugins directory. A stalled
    // File Provider mount there would freeze every console conversion before
    // the input score is opened.
    #[cfg(target_os = "macos")]
    command.env("CFFIXED_USER_HOME", private_dir);
    command.env("XDG_CONFIG_HOME", private_dir.join("config"));
    command.env("XDG_CACHE_HOME", private_dir.join("cache"));
    command.env("XDG_DATA_HOME", private_dir.join("data"));
    command.env("XDG_STATE_HOME", private_dir.join("state"));
    command.env("APPDATA", private_dir.join("appdata"));
    command.env("LOCALAPPDATA", private_dir.join("localappdata"));
    command.env("TMP", private_dir.join("tmp"));
    command.env("TEMP", private_dir.join("tmp"));
    command.env("TMPDIR", private_dir.join("tmp"));
    #[cfg(unix)]
    command.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    #[cfg(windows)]
    if let Some(system_root) =
        std::env::var_os("SystemRoot").filter(|value| Path::new(value).is_absolute())
    {
        let mut search_path = vec![PathBuf::from(&system_root).join("System32")];
        search_path.push(PathBuf::from(system_root));
        if let Ok(search_path) = std::env::join_paths(search_path) {
            command.env("PATH", search_path);
        }
    }
}

fn copy_environment_keys<F>(command: &mut Command, keys: &[&str], mut get_environment: F)
where
    F: FnMut(&str) -> Option<OsString>,
{
    for key in keys {
        if let Some(value) = get_environment(key) {
            command.env(key, value);
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn copy_linux_session_environment<F>(
    command: &mut Command,
    mut get_environment: F,
    private_dir: &Path,
) where
    F: FnMut(&str) -> Option<OsString>,
{
    let display = get_environment("DISPLAY").filter(|value| !value.is_empty());
    let wayland_display =
        get_environment("WAYLAND_DISPLAY").filter(|value| safe_wayland_display(value.as_os_str()));
    let runtime_dir = get_environment("XDG_RUNTIME_DIR")
        .filter(|value| Path::new(value).is_absolute())
        .unwrap_or_else(|| private_dir.join("runtime").into_os_string());
    let xauthority = get_environment("XAUTHORITY").filter(|value| Path::new(value).is_absolute());
    let session_type = get_environment("XDG_SESSION_TYPE").filter(|value| {
        value
            .to_str()
            .is_some_and(|value| matches!(value, "x11" | "wayland"))
    });
    let dbus_address = get_environment("DBUS_SESSION_BUS_ADDRESS")
        .filter(|value| safe_dbus_session_address(value.as_os_str()));
    let qt_platform =
        get_environment("QT_QPA_PLATFORM").filter(|value| safe_qt_qpa_platform(value.as_os_str()));

    for (key, value) in [
        ("DISPLAY", display.as_ref()),
        ("WAYLAND_DISPLAY", wayland_display.as_ref()),
        ("XDG_RUNTIME_DIR", Some(&runtime_dir)),
        ("XDG_SESSION_TYPE", session_type.as_ref()),
        ("XAUTHORITY", xauthority.as_ref()),
        ("DBUS_SESSION_BUS_ADDRESS", dbus_address.as_ref()),
    ] {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    if let Some(qt_platform) = qt_platform {
        command.env("QT_QPA_PLATFORM", qt_platform);
    } else if display.is_none() && wayland_display.is_none() {
        command.env("QT_QPA_PLATFORM", "offscreen");
    }
}

#[cfg(any(target_os = "linux", test))]
fn safe_wayland_display(value: &OsStr) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        || (path.components().count() == 1
            && path
                .file_name()
                .is_some_and(|name| !name.is_empty() && name != OsStr::new("."))
            && value != OsStr::new(".."))
}

#[cfg(any(target_os = "linux", test))]
fn safe_dbus_session_address(value: &OsStr) -> bool {
    value.to_str().is_some_and(|value| {
        !value.is_empty()
            && value.split(';').all(|address| {
                address.starts_with("unix:path=") || address.starts_with("unix:abstract=")
            })
    })
}

#[cfg(any(target_os = "linux", test))]
fn safe_qt_qpa_platform(value: &OsStr) -> bool {
    value.to_str().is_some_and(|value| {
        !value.is_empty()
            && value.split(';').all(|platform| {
                matches!(
                    platform,
                    "xcb" | "wayland" | "wayland-egl" | "offscreen" | "minimal"
                )
            })
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct MuseScoreVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

fn musescore_version(output: &str) -> Option<MuseScoreVersion> {
    let lowercase = output.to_ascii_lowercase();
    let marker = lowercase.find("musescore")?;
    let candidates = output[marker + "musescore".len()..]
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .filter(|piece| piece.chars().any(|character| character.is_ascii_digit()))
        .collect::<Vec<_>>();
    let version = candidates
        .iter()
        .copied()
        .find(|piece| piece.contains('.'))
        .or_else(|| candidates.first().copied())?;
    let mut components = version.split('.');
    Some(MuseScoreVersion {
        major: components.next()?.parse().ok()?,
        minor: components.next().unwrap_or("0").parse().ok()?,
        patch: components.next().unwrap_or("0").parse().ok()?,
    })
}

fn supported_musescore_version(version: MuseScoreVersion) -> bool {
    version.major == 4
        || (version.major == 3
            && version
                >= (MuseScoreVersion {
                    major: 3,
                    minor: 6,
                    patch: 2,
                }))
}

fn plausible_musescore_filename(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.contains("musescore") || name == "mscore" || name.starts_with("mscore.")
}

struct PrivateWorkDir(PathBuf);

impl PrivateWorkDir {
    fn create(label: &str) -> Result<Self, RenderError> {
        for _ in 0..100 {
            let counter = PRIVATE_WORK_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("verse-{label}-{}-{counter}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        return Self::secure_created(path, |path| {
                            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                        });
                    }
                    #[cfg(not(unix))]
                    return Self::secure_created(path, |_| Ok(()));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(RenderError::Io(error)),
            }
        }
        Err(RenderError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot allocate private renderer work directory",
        )))
    }

    fn secure_created<F>(path: PathBuf, secure: F) -> Result<Self, RenderError>
    where
        F: FnOnce(&Path) -> io::Result<()>,
    {
        let private_work = Self(path);
        secure(private_work.path())?;
        Ok(private_work)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateWorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn discovery_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        for app in [
            "MuseScore Studio 4.app",
            "MuseScore 4.app",
            "MuseScore 3.app",
            "MuseScore 3.6.app",
        ] {
            candidates.push(
                Path::new("/Applications")
                    .join(app)
                    .join("Contents/MacOS/mscore"),
            );
            if let Some(home) = std::env::var_os("HOME") {
                candidates.push(
                    PathBuf::from(home)
                        .join("Applications")
                        .join(app)
                        .join("Contents/MacOS/mscore"),
                );
            }
        }
    }
    if cfg!(target_os = "windows") {
        for root in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(program_files) = std::env::var_os(root) {
                for (folder, executable) in [
                    ("MuseScore Studio 4", "MuseScore4.exe"),
                    ("MuseScore 4", "MuseScore4.exe"),
                    ("MuseScore 3", "MuseScore3.exe"),
                ] {
                    let installation = PathBuf::from(&program_files).join(folder);
                    candidates.push(installation.join(executable));
                    candidates.push(installation.join("bin").join(executable));
                }
            }
        }
    }
    for name in [
        "mscore4",
        "musescore4",
        "mscore3",
        "musescore3",
        "mscore",
        "musescore",
    ] {
        candidates.extend(find_on_path(name));
    }
    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn find_on_path(name: &str) -> Vec<PathBuf> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let suffixes: &[&str] = if cfg!(target_os = "windows") {
        &[".exe", ""]
    } else {
        &[""]
    };
    std::env::split_paths(&path)
        .flat_map(|directory| {
            suffixes
                .iter()
                .map(move |suffix| directory.join(format!("{name}{suffix}")))
        })
        .filter(|candidate| candidate.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "verse-renderer-{label}-{}-{}",
            std::process::id(),
            counter
        ));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for sample in 0..441 {
            writer
                .write_sample::<i16>(if sample == 220 { 1_000 } else { 0 })
                .unwrap();
        }
        writer.finalize().unwrap();
    }

    fn write_silent_wav(path: &Path, sample_format: hound::SampleFormat) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: match sample_format {
                hound::SampleFormat::Float => 32,
                hound::SampleFormat::Int => 16,
            },
            sample_format,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        match sample_format {
            hound::SampleFormat::Float => {
                for _ in 0..882 {
                    writer.write_sample::<f32>(0.0).unwrap();
                }
            }
            hound::SampleFormat::Int => {
                for _ in 0..882 {
                    writer.write_sample::<i16>(0).unwrap();
                }
            }
        }
        writer.finalize().unwrap();
    }

    fn part_mscz(version: &str) -> Vec<u8> {
        part_mscz_with_scores(&[("score.mscx", version)])
    }

    fn part_mscz_with_scores(scores: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write as _;
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut archive = zip::ZipWriter::new(&mut bytes);
            for (path, version) in scores {
                archive
                    .start_file(*path, zip::write::SimpleFileOptions::default())
                    .unwrap();
                archive
                    .write_all(
                        format!(r#"<museScore version="{version}"><Score/></museScore>"#)
                            .as_bytes(),
                    )
                    .unwrap();
            }
            archive.finish().unwrap();
        }
        bytes.into_inner()
    }

    #[test]
    fn validates_non_empty_wav_and_hashes_it() {
        let dir = temp_dir("valid");
        let path = dir.join("mix.wav");
        write_wav(&path);
        let info = validate_wav(&path, 1024 * 1024).unwrap();
        assert_eq!(info.sample_rate, 44_100);
        assert_eq!(info.channels, 1);
        assert_eq!(info.frames, 441);
        assert!((info.duration_seconds - 0.01).abs() < 0.000_001);
        assert_eq!(info.sha256, sha256_file(&path).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_empty_and_corrupt_wav() {
        let dir = temp_dir("invalid");
        let empty = dir.join("empty.wav");
        fs::File::create(&empty).unwrap();
        assert!(matches!(
            validate_wav(&empty, 1024),
            Err(RenderError::InvalidWav { .. })
        ));
        let corrupt = dir.join("corrupt.wav");
        fs::write(&corrupt, b"not a wave").unwrap();
        assert!(matches!(
            validate_wav(&corrupt, 1024),
            Err(RenderError::InvalidWav { .. })
        ));
        let truncated = dir.join("truncated.wav");
        write_wav(&truncated);
        let mut bytes = fs::read(&truncated).unwrap();
        bytes.truncate(bytes.len() - 3);
        fs::write(&truncated, bytes).unwrap();
        assert!(matches!(
            validate_wav(&truncated, 1024 * 1024),
            Err(RenderError::InvalidWav { .. })
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_silent_pcm_and_float_wavs() {
        let dir = temp_dir("silent");
        for (name, sample_format) in [
            ("silent-pcm.wav", hound::SampleFormat::Int),
            ("silent-float.wav", hound::SampleFormat::Float),
        ] {
            let path = dir.join(name);
            write_silent_wav(&path, sample_format);
            let error = validate_wav(&path, 1024 * 1024).unwrap_err();
            assert!(
                matches!(
                    &error,
                    RenderError::InvalidWav { reason } if reason.contains("silent")
                ),
                "{error}"
            );
            let allowed = validate_wav_allowing_silence(&path, 1024 * 1024)
                .expect("isolated source Parts may render a structurally valid silent WAV");
            assert!(allowed.frames > 0);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn validates_float_wav_energy_and_rejects_non_finite_samples() {
        let dir = temp_dir("float-energy");
        let valid = dir.join("valid.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&valid, spec).unwrap();
        for sample in [0.0_f32, -0.25, 0.5, 0.0] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        assert_eq!(validate_wav(&valid, 1024 * 1024).unwrap().frames, 4);

        let invalid = dir.join("non-finite.wav");
        let mut writer = hound::WavWriter::create(&invalid, spec).unwrap();
        writer.write_sample(f32::NAN).unwrap();
        writer.finalize().unwrap();
        let error = validate_wav(&invalid, 1024 * 1024).unwrap_err();
        assert!(matches!(
            error,
            RenderError::InvalidWav { reason } if reason.contains("non-finite")
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn renderer_arguments_are_fixed_and_never_shell_text() {
        let args = MuseScoreRenderer::render_args(Path::new("a score.mscz"), Path::new("mix.wav"));
        assert_eq!(
            args,
            vec![
                OsString::from("-F"),
                OsString::from("-o"),
                OsString::from("mix.wav"),
                OsString::from("a score.mscz")
            ]
        );
    }

    #[test]
    fn score_parts_json_is_bounded_decoded_and_keeps_ordinal_metadata() {
        let first = base64::engine::general_purpose::STANDARD.encode(part_mscz("4.50"));
        let second = base64::engine::general_purpose::STANDARD.encode(part_mscz("4.50"));
        let json = serde_json::to_vec(&serde_json::json!({
            "parts": ["Soprano", "Piano"],
            "partsBin": [first, second],
            "partsMeta": [{"id": "P1"}, {"id": "P2"}]
        }))
        .unwrap();
        let parts = parse_score_parts_json(&json).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].ordinal, 0);
        assert_eq!(parts[1].name, "Piano");
        assert_eq!(parts[1].metadata["id"], "P2");
        assert!(!parts[0].mscz.is_empty());
    }

    #[test]
    fn score_parts_json_rejects_mismatched_or_untrusted_payloads() {
        for payload in [
            serde_json::json!({"parts": ["Piano"], "partsBin": []}),
            serde_json::json!({"parts": ["Piano"], "partsBin": ["%%%"]}),
            serde_json::json!({
                "parts": ["Piano"],
                "partsBin": [base64::engine::general_purpose::STANDARD.encode(b"not a zip")]
            }),
            serde_json::json!({"parts": [""], "partsBin": ["AA=="]}),
        ] {
            assert!(matches!(
                parse_score_parts_json(&serde_json::to_vec(&payload).unwrap()),
                Err(RenderError::InvalidScoreParts { .. })
            ));
        }
    }

    #[test]
    fn score_parts_json_rejects_an_ambiguous_multi_mscx_archive() {
        let ambiguous = base64::engine::general_purpose::STANDARD.encode(part_mscz_with_scores(&[
            ("score.mscx", "4.50"),
            ("alternate.mscx", "4.50"),
        ]));
        let payload = serde_json::json!({
            "parts": ["Piano"],
            "partsBin": [ambiguous]
        });
        let error =
            parse_score_parts_json(&serde_json::to_vec(&payload).unwrap()).expect_err("ambiguous");
        assert!(matches!(
            error,
            RenderError::InvalidScoreParts { reason }
                if reason.contains("exactly one unambiguous master MSCX")
        ));
    }

    #[test]
    fn native_score_major_is_detected_for_mscx_and_mscz() {
        let dir = temp_dir("score-major");
        let mscx = dir.join("score.mscx");
        fs::write(
            &mscx,
            br#"<?xml version="1.0"?><museScore version="4.50"><Score/></museScore>"#,
        )
        .unwrap();
        assert_eq!(native_musescore_score_major(&mscx).unwrap(), Some(4));
        let mscz = dir.join("score.mscz");
        fs::write(&mscz, part_mscz("3.60")).unwrap();
        assert_eq!(native_musescore_score_major(&mscz).unwrap(), Some(3));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_score_major_is_detected_after_a_long_bounded_xml_prologue() {
        let dir = temp_dir("score-major-long-prologue");
        let mscx = dir.join("score.mscx");
        let prologue = format!(
            "<?xml version=\"1.0\"?>\n<!--{}-->\n",
            "prologue".repeat(2_048)
        );
        fs::write(
            &mscx,
            format!("{prologue}<museScore version='4.50'><Score/></museScore>"),
        )
        .unwrap();
        assert!(prologue.len() > 8 * 1024);
        assert_eq!(native_musescore_score_major(&mscx).unwrap(), Some(4));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn configured_missing_renderer_is_blocking() {
        let config = MuseScoreConfig {
            executable: Some(PathBuf::from("/definitely/not/a/musescore/executable")),
            ..MuseScoreConfig::default()
        };
        assert!(matches!(
            MuseScoreRenderer::discover(&config),
            Err(RenderError::InvalidExecutable)
        ));
    }

    #[test]
    fn configured_non_musescore_binary_is_rejected_before_execution() {
        let executable = std::env::current_exe().unwrap();
        assert!(matches!(
            MuseScoreRenderer::probe(&executable),
            Err(RenderError::InvalidExecutable)
        ));
    }

    #[test]
    fn version_probe_enforces_musescore_three_point_six_point_two_floor() {
        assert_eq!(
            musescore_version("MuseScore 4.5.2"),
            Some(MuseScoreVersion {
                major: 4,
                minor: 5,
                patch: 2
            })
        );
        assert_eq!(
            musescore_version("MuseScore 3.6.2"),
            Some(MuseScoreVersion {
                major: 3,
                minor: 6,
                patch: 2
            })
        );
        assert_eq!(
            musescore_version("Qt 6.6.3 / MuseScore 3.6.2"),
            Some(MuseScoreVersion {
                major: 3,
                minor: 6,
                patch: 2
            })
        );
        assert_eq!(
            musescore_version("MuseScore4 4.7.4.252060402"),
            Some(MuseScoreVersion {
                major: 4,
                minor: 7,
                patch: 4
            })
        );
        assert_eq!(musescore_version("not a version"), None);
        for version in [
            MuseScoreVersion {
                major: 3,
                minor: 6,
                patch: 2,
            },
            MuseScoreVersion {
                major: 3,
                minor: 7,
                patch: 0,
            },
            MuseScoreVersion {
                major: 4,
                minor: 0,
                patch: 0,
            },
        ] {
            assert!(supported_musescore_version(version));
        }
        for version in [
            MuseScoreVersion {
                major: 3,
                minor: 6,
                patch: 1,
            },
            MuseScoreVersion {
                major: 3,
                minor: 5,
                patch: 99,
            },
            MuseScoreVersion {
                major: 2,
                minor: 3,
                patch: 2,
            },
            MuseScoreVersion {
                major: 5,
                minor: 0,
                patch: 0,
            },
        ] {
            assert!(!supported_musescore_version(version));
        }
    }

    /// This test previously asserted the opposite for 4.7 and later, on the
    /// grounds that upstream PR #31084 fixed the abort. Measured against a real
    /// MuseScore4 4.7.4 on macOS: a bare private home aborts with `mutex lock
    /// failed: Invalid argument` in four of five consecutive `--score-parts` runs
    /// of the same score. The old gate therefore left the newest installations
    /// with one attempt and no cooldown, which is why a complete bundle export
    /// failed on a current MuseScore almost every time. The expectations below
    /// follow the measurement, not the upstream release note.
    #[test]
    fn every_macos_musescore_four_gets_the_score_load_cooldown() {
        assert!(needs_macos_score_load_workaround("MuseScore 4.6.4"));
        assert!(needs_macos_score_load_workaround("MuseScore 4.7.0"));
        assert!(needs_macos_score_load_workaround(
            "MuseScore4 4.7.4.252060402"
        ));
        assert!(needs_macos_score_load_workaround("unparseable MuseScore"));
    }

    #[test]
    fn complete_stdout_workaround_targets_musescore_four_seven_and_later() {
        assert!(!needs_macos_complete_stdout_workaround("MuseScore 4.6.4"));
        assert!(needs_macos_complete_stdout_workaround("MuseScore 4.7.0"));
        assert!(needs_macos_complete_stdout_workaround(
            "MuseScore4 4.7.4.252060402"
        ));
        assert!(!needs_macos_complete_stdout_workaround(
            "unparseable MuseScore"
        ));
    }

    #[cfg(unix)]
    fn fake_probe_executable(label: &str, version: &str, help: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let directory = temp_dir(label);
        let executable = directory.join("mscore");
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) printf '%s\\n' '{version}' ;;\n  \
             --help) printf '%s\\n' '{help}' ;;\n  *) exit 2 ;;\nesac\n"
        );
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        (directory, executable)
    }

    #[cfg(unix)]
    fn fake_score_parts_failure_executable(label: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let directory = temp_dir(label);
        let executable = directory.join("mscore");
        let encoded = base64::engine::general_purpose::STANDARD.encode(part_mscz("4.50"));
        let payload = serde_json::json!({
            "parts": ["Piano"],
            "partsBin": [encoded]
        })
        .to_string();
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) printf '%s\\n' 'MuseScore 4.5.2' ;;\n  \
             --help) printf '%s\\n' '--score-parts' ;;\n  -F) printf '%s\\n' '{payload}'; exit 7 ;;\n  \
             *) exit 2 ;;\nesac\n"
        );
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        (directory, executable)
    }

    #[cfg(unix)]
    fn fake_score_parts_signals_then_succeeds(
        label: &str,
        version: &str,
        signal_attempts: usize,
        signal: &str,
        valid_payload: bool,
    ) -> (PathBuf, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let directory = temp_dir(label);
        let executable = directory.join("mscore");
        let attempts = directory.join("attempts");
        let encoded = base64::engine::general_purpose::STANDARD.encode(part_mscz("4.50"));
        let valid_json = serde_json::json!({
            "parts": ["Piano"],
            "partsBin": [encoded]
        })
        .to_string();
        let payload = if valid_payload {
            valid_json
        } else {
            "incomplete-json".to_string()
        };
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) printf '%s\\n' 'MuseScore {version}' ;;\n  \
             --help) printf '%s\\n' '--score-parts' ;;\n  -F)\n    count=0\n    if [ -f \
             '{attempts}' ]; then read -r count < '{attempts}'; fi\n    count=$((count + 1))\n    \
             printf '%s\\n' \"$count\" > '{attempts}'\n    printf '%s\\n' '{payload}'\n    if [ \
             \"$count\" -le {signal_attempts} ]; then kill -{signal} $$; fi\n    ;;\n  *) exit 2 \
             ;;\nesac\n",
            attempts = attempts.display()
        );
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        (directory, executable, attempts)
    }

    #[cfg(unix)]
    fn fake_render_signals_then_succeeds(
        label: &str,
        version: &str,
        signal_attempts: usize,
    ) -> (PathBuf, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let directory = temp_dir(label);
        let executable = directory.join("mscore");
        let attempts = directory.join("render-attempts");
        let wav_template = directory.join("template.wav");
        write_wav(&wav_template);
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) printf '%s\\n' 'MuseScore {version}' ;;\n  \
             --help) printf '%s\\n' '--score-parts' ;;\n  -F)\n    count=0\n    if [ -f \
             '{attempts}' ]; then read -r count < '{attempts}'; fi\n    count=$((count + 1))\n    \
             printf '%s\\n' \"$count\" > '{attempts}'\n    if [ -e \"$3\" ]; then exit 9; fi\n    \
             cp '{wav_template}' \"$3\"\n    if [ \"$count\" -le {signal_attempts} ]; then kill \
             -ABRT $$; fi\n    ;;\n  *) exit 2 ;;\nesac\n",
            attempts = attempts.display(),
            wav_template = wav_template.display()
        );
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        (directory, executable, attempts)
    }

    fn no_score_load_retry_policy() -> ScoreLoadRetryPolicy {
        ScoreLoadRetryPolicy {
            enabled: false,
            cooldown: Duration::ZERO,
            max_attempts: 1,
        }
    }

    fn fast_score_load_retry_policy() -> ScoreLoadRetryPolicy {
        ScoreLoadRetryPolicy {
            enabled: true,
            cooldown: Duration::from_millis(10),
            max_attempts: 3,
        }
    }

    #[cfg(unix)]
    fn fake_score_input(directory: &Path) -> PathBuf {
        let input = directory.join("score.mscx");
        fs::write(
            &input,
            br#"<?xml version="1.0"?><museScore version="4.50"><Score/></museScore>"#,
        )
        .unwrap();
        input
    }

    #[cfg(unix)]
    #[test]
    fn capability_probe_accepts_only_qualified_musescore_three_or_four() {
        let (qualified_dir, qualified) =
            fake_probe_executable("probe-ms3", "MuseScore 3.6.2", "--score-parts");
        let renderer = MuseScoreRenderer::probe(&qualified).expect("qualified MuseScore 3");
        assert_eq!(renderer.capabilities().identity.major, 3);
        assert!(renderer.capabilities().score_parts);
        fs::remove_dir_all(qualified_dir).unwrap();

        let (missing_dir, missing) =
            fake_probe_executable("probe-missing", "MuseScore 3.6.2", "--export-to");
        assert!(matches!(
            MuseScoreRenderer::probe(&missing),
            Err(RenderError::UnsupportedCapabilities { missing })
                if missing == vec!["score-parts"]
        ));
        fs::remove_dir_all(missing_dir).unwrap();

        let (too_old_dir, too_old) =
            fake_probe_executable("probe-ms3-too-old", "MuseScore 3.6.1", "--score-parts");
        assert!(matches!(
            MuseScoreRenderer::probe(&too_old),
            Err(RenderError::UnsupportedVersion { .. })
        ));
        fs::remove_dir_all(too_old_dir).unwrap();

        let (future_dir, future) =
            fake_probe_executable("probe-future", "MuseScore 5.0.0", "--score-parts");
        assert!(matches!(
            MuseScoreRenderer::probe(&future),
            Err(RenderError::UnsupportedVersion { .. })
        ));
        fs::remove_dir_all(future_dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn score_parts_rejects_nonzero_exit_even_when_stdout_is_valid_json() {
        let (directory, executable) = fake_score_parts_failure_executable("score-parts-nonzero");
        let input = fake_score_input(&directory);
        let renderer = MuseScoreRenderer::probe(&executable).unwrap();
        let error = renderer
            .extract_score_parts_with_policy(
                &input,
                &RenderLimits {
                    timeout: Duration::from_secs(2),
                    max_output_bytes: 1024 * 1024,
                },
                no_score_load_retry_policy(),
            )
            .expect_err("nonzero renderer status must be authoritative");
        assert!(matches!(error, RenderError::Exit { code: Some(7), .. }));
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn complete_score_parts_stdout_bounds_a_renderer_that_stays_alive() {
        use std::os::unix::fs::PermissionsExt;

        let directory = temp_dir("score-parts-complete-stdout");
        let executable = directory.join("mscore");
        let encoded = base64::engine::general_purpose::STANDARD.encode(part_mscz("4.50"));
        let payload = serde_json::json!({
            "parts": ["Piano"],
            "partsBin": [encoded]
        })
        .to_string();
        fs::write(
            &executable,
            format!("#!/bin/sh\nprintf '%s\\n' '{payload}'\nsleep 10\n"),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let started = Instant::now();
        let result = run_bounded_process_with_capture_until_complete_stdout(
            &executable,
            &[],
            &directory,
            Duration::from_secs(5),
            None,
            1024 * 1024,
            MAX_PARTS_JSON_BYTES,
            score_parts_json_is_complete,
        )
        .expect("complete score-parts JSON should end the lingering process");

        assert!(result.completed_from_stdout);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(parse_score_parts_json(&result.stdout).unwrap().len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn score_parts_rejects_signaled_process_even_when_stdout_is_valid_json() {
        let (directory, executable, attempts) = fake_score_parts_signals_then_succeeds(
            "score-parts-signal-rejected",
            "4.5.2",
            1,
            "TERM",
            true,
        );
        let input = fake_score_input(&directory);
        let renderer = MuseScoreRenderer::probe(&executable).unwrap();
        let error = renderer
            .extract_score_parts_with_policy(
                &input,
                &RenderLimits {
                    timeout: Duration::from_secs(2),
                    max_output_bytes: 1024 * 1024,
                },
                fast_score_load_retry_policy(),
            )
            .expect_err("a complete payload cannot override signal termination");
        assert!(matches!(
            error,
            RenderError::Exit { code: None, log }
                if log.contains("Unix signal 15") && log.contains("\"parts\"")
        ));
        assert_eq!(fs::read_to_string(attempts).unwrap().trim(), "1");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn score_parts_sigabrt_with_incomplete_payload_is_not_retried() {
        let (directory, executable, attempts) = fake_score_parts_signals_then_succeeds(
            "score-parts-sigabrt-incomplete",
            "4.5.2",
            1,
            "ABRT",
            false,
        );
        let input = fake_score_input(&directory);
        let renderer = MuseScoreRenderer::probe(&executable).unwrap();
        let error = renderer
            .extract_score_parts_with_policy(
                &input,
                &RenderLimits {
                    timeout: Duration::from_secs(2),
                    max_output_bytes: 1024 * 1024,
                },
                fast_score_load_retry_policy(),
            )
            .expect_err("an incomplete payload must make SIGABRT non-retryable");
        assert!(matches!(
            error,
            RenderError::Exit { code: None, log }
                if log.contains("Unix signal 6") && log.contains("incomplete-json")
        ));
        assert_eq!(fs::read_to_string(attempts).unwrap().trim(), "1");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn score_parts_retries_only_sigabrt_with_valid_payload_and_requires_exit_zero() {
        let (directory, executable, attempts) = fake_score_parts_signals_then_succeeds(
            "score-parts-sigabrt-retry",
            "4.5.2",
            1,
            "ABRT",
            true,
        );
        let input = fake_score_input(&directory);
        let renderer = MuseScoreRenderer::probe(&executable).unwrap();
        let parts = renderer
            .extract_score_parts_with_policy(
                &input,
                &RenderLimits {
                    timeout: Duration::from_secs(2),
                    max_output_bytes: 1024 * 1024,
                },
                fast_score_load_retry_policy(),
            )
            .expect("second attempt exits zero");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].name, "Piano");
        assert_eq!(fs::read_to_string(attempts).unwrap().trim(), "2");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn score_parts_recovers_valid_macos_four_seven_output_after_shutdown_abort() {
        let (directory, executable, attempts) = fake_score_parts_signals_then_succeeds(
            "score-parts-four-seven-shutdown-abort",
            "4.7.4",
            1,
            "ABRT",
            true,
        );
        let input = fake_score_input(&directory);
        let renderer = MuseScoreRenderer::probe(&executable).unwrap();
        let parts = renderer
            .extract_score_parts_with_policy(
                &input,
                &RenderLimits {
                    timeout: Duration::from_secs(2),
                    max_output_bytes: 1024 * 1024,
                },
                no_score_load_retry_policy(),
            )
            .expect("fully validated output survives MuseScore shutdown abort");

        assert_eq!(parts.len(), 1);
        assert_eq!(fs::read_to_string(attempts).unwrap().trim(), "1");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn score_parts_sigabrt_retries_stop_at_the_hard_limit() {
        let (directory, executable, attempts) = fake_score_parts_signals_then_succeeds(
            "score-parts-sigabrt-limit",
            "4.5.2",
            3,
            "ABRT",
            true,
        );
        let input = fake_score_input(&directory);
        let renderer = MuseScoreRenderer::probe(&executable).unwrap();
        let error = renderer
            .extract_score_parts_with_policy(
                &input,
                &RenderLimits {
                    timeout: Duration::from_secs(2),
                    max_output_bytes: 1024 * 1024,
                },
                fast_score_load_retry_policy(),
            )
            .expect_err("all bounded attempts abort");
        assert!(matches!(
            error,
            RenderError::Exit { code: None, log }
                if log.contains("Unix signal 6")
        ));
        assert_eq!(fs::read_to_string(attempts).unwrap().trim(), "3");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn wav_render_sigabrt_removes_failed_output_and_requires_exit_zero() {
        let (directory, executable, attempts) =
            fake_render_signals_then_succeeds("render-sigabrt-retry", "4.5.2", 1);
        let input = fake_score_input(&directory);
        let output = directory.join("mix.wav");
        let renderer = MuseScoreRenderer::probe(&executable).unwrap();
        let rendered = renderer
            .render_with_policy(
                &input,
                &output,
                &RenderLimits {
                    timeout: Duration::from_secs(2),
                    max_output_bytes: 1024 * 1024,
                },
                fast_score_load_retry_policy(),
                false,
            )
            .expect("second render exits zero");
        assert_eq!(rendered.path, output);
        assert_eq!(fs::read_to_string(attempts).unwrap().trim(), "2");
        assert!(rendered.wav.duration_seconds > 0.0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn wav_render_recovers_valid_macos_four_seven_output_after_shutdown_abort() {
        let (directory, executable, attempts) =
            fake_render_signals_then_succeeds("render-four-seven-shutdown-abort", "4.7.4", 1);
        let input = fake_score_input(&directory);
        let output = directory.join("mix.wav");
        let renderer = MuseScoreRenderer::probe(&executable).unwrap();
        let rendered = renderer
            .render_with_policy(
                &input,
                &output,
                &RenderLimits {
                    timeout: Duration::from_secs(2),
                    max_output_bytes: 1024 * 1024,
                },
                no_score_load_retry_policy(),
                false,
            )
            .expect("fully validated WAV survives MuseScore shutdown abort");

        assert_eq!(rendered.path, output);
        assert_eq!(fs::read_to_string(attempts).unwrap().trim(), "1");
        assert!(rendered.wav.duration_seconds > 0.0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_process_log_prioritizes_stderr_over_large_stdout() {
        let status = Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .status()
            .unwrap();
        let result = ProcessResult {
            status,
            stdout: vec![b'j'; MAX_LOG_BYTES * 2],
            stderr: b"mutex lock failed: Invalid argument".to_vec(),
            completed_from_stdout: false,
        };

        assert!(
            result
                .failure_log()
                .starts_with("mutex lock failed: Invalid argument\n"),
            "failure stderr must not be hidden by score-parts JSON"
        );
    }

    #[test]
    fn executable_hash_change_invalidates_renderer_identity() {
        let dir = temp_dir("identity-change");
        let executable = dir.join("musescore4");
        fs::write(&executable, b"first executable").unwrap();
        let expected = sha256_file(&executable).unwrap();
        verify_executable_hash(&executable, &expected).unwrap();

        fs::write(&executable, b"replacement executable").unwrap();
        assert!(matches!(
            verify_executable_hash(&executable, &expected),
            Err(RenderError::ExecutableChanged {
                expected: error_expected,
                observed,
            }) if error_expected == expected && observed == sha256_file(&executable).unwrap()
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_private_work_hardening_removes_created_directory() {
        let parent = temp_dir("private-cleanup");
        let work = parent.join("work");
        fs::create_dir(&work).unwrap();
        let result = PrivateWorkDir::secure_created(work.clone(), |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "simulated permission failure",
            ))
        });
        assert!(matches!(result, Err(RenderError::Io(_))));
        assert!(!work.exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn linux_session_environment_preserves_only_safe_values() {
        let private = Path::new("/private/renderer");
        let source = BTreeMap::from([
            ("DISPLAY", OsString::from(":1")),
            ("WAYLAND_DISPLAY", OsString::from("wayland-0")),
            ("XDG_RUNTIME_DIR", OsString::from("/run/user/1000")),
            ("XDG_SESSION_TYPE", OsString::from("wayland")),
            ("XAUTHORITY", OsString::from("/run/user/1000/Xauthority")),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                OsString::from("unix:path=/run/user/1000/bus"),
            ),
            ("QT_QPA_PLATFORM", OsString::from("wayland;xcb")),
            ("QT_PLUGIN_PATH", OsString::from("/tmp/untrusted-plugins")),
            ("LD_PRELOAD", OsString::from("/tmp/untrusted.so")),
        ]);
        let mut command = Command::new("renderer");
        command.env_clear();
        copy_linux_session_environment(&mut command, |key| source.get(key).cloned(), private);
        let environment = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
            .collect::<BTreeMap<_, _>>();

        for key in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "XDG_SESSION_TYPE",
            "XAUTHORITY",
            "DBUS_SESSION_BUS_ADDRESS",
            "QT_QPA_PLATFORM",
        ] {
            assert_eq!(environment.get(OsStr::new(key)), source.get(key));
        }
        assert!(!environment.contains_key(OsStr::new("QT_PLUGIN_PATH")));
        assert!(!environment.contains_key(OsStr::new("LD_PRELOAD")));
        assert!(!safe_qt_qpa_platform(OsStr::new("/tmp/plugin")));
        assert!(!safe_dbus_session_address(OsStr::new(
            "unixexec:path=/bin/sh"
        )));
    }

    #[test]
    fn bounded_process_timeout_kills_and_waits_for_the_child() {
        let executable = std::env::current_exe().unwrap();
        let work = temp_dir("timeout");
        let started = Instant::now();
        let result = run_bounded_process(
            &executable,
            &[
                OsString::from("--ignored"),
                OsString::from("--exact"),
                OsString::from("renderer::tests::bounded_process_sleep_helper"),
            ],
            &work,
            Duration::from_millis(75),
            None,
            1024,
        );
        assert!(matches!(result, Err(ProcessError::Timeout)));
        assert!(started.elapsed() < Duration::from_secs(2));
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    #[ignore = "helper launched by bounded_process_timeout_kills_and_waits_for_the_child"]
    fn bounded_process_sleep_helper() {
        thread::sleep(Duration::from_secs(5));
    }

    #[test]
    fn timeout_kills_descendants_that_inherit_the_log_pipes() {
        let executable = std::env::current_exe().unwrap();
        let work = temp_dir("descendant-timeout");
        let started = Instant::now();
        let result = run_bounded_process(
            &executable,
            &[
                OsString::from("--ignored"),
                OsString::from("--exact"),
                OsString::from("renderer::tests::bounded_process_parent_helper"),
            ],
            &work,
            Duration::from_millis(150),
            None,
            1024,
        );
        assert!(matches!(result, Err(ProcessError::Timeout)));
        assert!(started.elapsed() < Duration::from_secs(2));
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    #[ignore = "helper launched by timeout_kills_descendants_that_inherit_the_log_pipes"]
    fn bounded_process_parent_helper() {
        let mut descendant = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "renderer::tests::bounded_process_sleep_helper",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let _ = descendant.wait();
    }

    #[test]
    fn completed_parent_cannot_make_pipe_collection_unbounded() {
        let executable = std::env::current_exe().unwrap();
        let work = temp_dir("orphaned-pipe");
        let started = Instant::now();
        let result = run_bounded_process(
            &executable,
            &[
                OsString::from("--ignored"),
                OsString::from("--exact"),
                OsString::from("renderer::tests::bounded_process_orphaning_parent_helper"),
            ],
            &work,
            Duration::from_secs(3),
            None,
            1024,
        );
        assert!(result.unwrap().status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    #[ignore = "helper launched by completed_parent_cannot_make_pipe_collection_unbounded"]
    #[allow(clippy::zombie_processes)]
    fn bounded_process_orphaning_parent_helper() {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "renderer::tests::bounded_process_sleep_helper",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
    }

    #[test]
    fn logs_are_bounded_while_the_pipe_is_fully_drained() {
        let (sender, receiver) = mpsc::channel();
        spawn_bounded_drain(Cursor::new(vec![b'x'; 1024]), 16, LogStream::Stdout, sender);
        let mut retained = Vec::new();
        loop {
            match receiver.recv_timeout(Duration::from_secs(1)).unwrap() {
                DrainEvent::Data { bytes, .. } => retained.extend(bytes),
                DrainEvent::LimitExceeded { .. } => {}
                DrainEvent::Finished { result, .. } => {
                    result.unwrap();
                    break;
                }
            }
        }
        assert_eq!(retained.len(), 16);
    }

    #[test]
    fn configured_real_musescore_extracts_bounded_parts_and_renders_audio() {
        let (Ok(executable), Ok(score)) = (
            std::env::var("VERSE_MUSESCORE_GATE"),
            std::env::var("VERSE_SCORE_PARTS_GATE"),
        ) else {
            return;
        };
        let renderer = MuseScoreRenderer::probe(Path::new(&executable)).unwrap();
        let limits = RenderLimits {
            timeout: Duration::from_secs(120),
            max_output_bytes: 512 * 1024 * 1024,
        };
        let parts = renderer
            .extract_score_parts(Path::new(&score), &limits)
            .unwrap();
        assert!(!parts.is_empty());
        assert!(parts.iter().all(|part| !part.mscz.is_empty()));
        let root = temp_dir("real-gate");
        let output = root.join("full.wav");
        let rendered = renderer
            .render(Path::new(&score), &output, &limits)
            .unwrap();
        assert!(rendered.wav.duration_seconds > 0.0);
        fs::remove_dir_all(root).unwrap();
    }
}
