//! Bounded, source-faithful score rendering through a user-installed
//! MuseScore Studio 4 executable.
//!
//! The renderer is intentionally a narrow process adapter: the frontend can
//! select an executable, but it cannot supply arguments or invoke arbitrary
//! commands.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

pub const DEFAULT_RENDER_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_MAX_WAV_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_LOG_BYTES: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const PIPE_DRAIN_GRACE: Duration = Duration::from_millis(250);
const TERMINATION_GRACE: Duration = Duration::from_millis(500);
const COMMON_ENVIRONMENT_KEYS: &[&str] = &["SystemRoot", "WINDIR", "LANG", "LC_ALL"];
static PRIVATE_WORK_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    pub executable_sha256: String,
    pub full_score_mix: bool,
}

#[derive(Clone, Debug)]
pub struct RendererCapabilities {
    pub identity: RendererIdentity,
    pub supported_extensions: Vec<&'static str>,
    pub output_format: &'static str,
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

pub trait AudioRenderer: Send + Sync {
    fn capabilities(&self) -> &RendererCapabilities;

    fn render(
        &self,
        input: &Path,
        output: &Path,
        limits: &RenderLimits,
    ) -> Result<RenderedAudio, RenderError>;
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("MuseScore Studio 4 was not found")]
    NotFound { searched: Vec<String> },
    #[error("configured renderer is not a regular executable file")]
    InvalidExecutable,
    #[error("renderer probe timed out")]
    ProbeTimeout,
    #[error("the configured executable is not MuseScore Studio")]
    ProbeRejected { output: String },
    #[error("MuseScore Studio 4 or later is required (detected: {detected})")]
    UnsupportedVersion { detected: String },
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
        let major = musescore_version_major(&output).ok_or_else(|| RenderError::ProbeRejected {
            output: output.clone(),
        })?;
        if major < 4 {
            return Err(RenderError::UnsupportedVersion { detected: output });
        }

        let identity = RendererIdentity {
            provider: "musescore".into(),
            version: output.trim().to_string(),
            executable_sha256,
            full_score_mix: true,
        };
        Ok(Self {
            executable,
            capabilities: RendererCapabilities {
                identity,
                supported_extensions: vec![
                    "kar", "mid", "midi", "mxl", "xml", "musicxml", "mscz", "mscx",
                ],
                output_format: "wav",
            },
        })
    }

    fn render_args(input: &Path, output: &Path) -> Vec<OsString> {
        vec![
            OsString::from("-o"),
            output.as_os_str().to_owned(),
            input.as_os_str().to_owned(),
        ]
    }
}

impl AudioRenderer for MuseScoreRenderer {
    fn capabilities(&self) -> &RendererCapabilities {
        &self.capabilities
    }

    fn render(
        &self,
        input: &Path,
        output: &Path,
        limits: &RenderLimits,
    ) -> Result<RenderedAudio, RenderError> {
        let input_meta = fs::metadata(input)?;
        if !input_meta.is_file() {
            return Err(RenderError::InvalidExecutable);
        }
        if output.exists() {
            return Err(RenderError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "renderer output already exists",
            )));
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
        let result = run_bounded_process(
            &self.executable,
            &Self::render_args(input, output),
            work_dir,
            limits.timeout,
            Some(output),
            limits.max_output_bytes,
        )
        .map_err(|error| match error {
            ProcessError::Spawn(error) => RenderError::Spawn(error),
            ProcessError::Timeout => RenderError::Timeout {
                milliseconds: limits.timeout.as_millis().min(u128::from(u64::MAX)) as u64,
            },
            ProcessError::OutputTooLarge { bytes, limit } => {
                RenderError::OutputTooLarge { bytes, limit }
            }
            ProcessError::Io(error) => RenderError::Io(error),
        })?;
        verify_executable_hash(&self.executable, &renderer_identity.executable_sha256)?;
        if !result.status.success() {
            return Err(RenderError::Exit {
                code: result.status.code(),
                log: result.log(),
            });
        }
        if !output.exists() {
            return Err(RenderError::MissingOutput);
        }
        let wav = validate_wav(output, limits.max_output_bytes)?;
        Ok(RenderedAudio {
            path: output.to_path_buf(),
            wav,
            renderer: renderer_identity,
        })
    }
}

pub fn validate_wav(path: &Path, max_bytes: u64) -> Result<WavInfo, RenderError> {
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
    if peak == 0.0 || rms == 0.0 {
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
}

impl ProcessResult {
    fn log(&self) -> String {
        let mut bytes = self.stdout.clone();
        if !bytes.is_empty() && !self.stderr.is_empty() {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(&self.stderr);
        if bytes.len() > MAX_LOG_BYTES {
            bytes.truncate(MAX_LOG_BYTES);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
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
}

struct ProcessOutputCollector {
    receiver: mpsc::Receiver<DrainEvent>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_finished: bool,
    stderr_finished: bool,
    error: Option<io::Error>,
}

impl ProcessOutputCollector {
    fn new(receiver: mpsc::Receiver<DrainEvent>) -> Self {
        Self {
            receiver,
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_finished: false,
            stderr_finished: false,
            error: None,
        }
    }

    fn handle(&mut self, event: DrainEvent) {
        match event {
            DrainEvent::Data { stream, bytes } => {
                let destination = match stream {
                    LogStream::Stdout => &mut self.stdout,
                    LogStream::Stderr => &mut self.stderr,
                };
                let remaining = MAX_LOG_BYTES.saturating_sub(destination.len());
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
}

fn run_bounded_process(
    program: &Path,
    args: &[OsString],
    current_dir: &Path,
    timeout: Duration,
    monitored_output: Option<&Path>,
    max_output_bytes: u64,
) -> Result<ProcessResult, ProcessError> {
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
        MAX_LOG_BYTES,
        LogStream::Stdout,
        drain_sender.clone(),
    );
    spawn_bounded_drain(stderr, MAX_LOG_BYTES, LogStream::Stderr, drain_sender);
    let mut output_collector = ProcessOutputCollector::new(drain_receiver);

    let started = Instant::now();
    let status = loop {
        output_collector.poll();
        if let Some(error) = output_collector.take_error() {
            terminate_and_reap(&mut child, &mut output_collector);
            return Err(ProcessError::Io(error));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                terminate_and_reap(&mut child, &mut output_collector);
                return Err(ProcessError::Io(error));
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
    Ok(ProcessResult {
        status,
        stdout: output_collector.stdout,
        stderr: output_collector.stderr,
    })
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
        let mut buffer = [0_u8; 8192];
        let result = loop {
            match reader.read(&mut buffer) {
                Ok(0) => break Ok(()),
                Ok(count) => {
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

fn musescore_version_major(output: &str) -> Option<u32> {
    let lowercase = output.to_ascii_lowercase();
    let marker = lowercase.find("musescore")?;
    output[marker + "musescore".len()..]
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .filter(|piece| piece.contains('.'))
        .find_map(|piece| piece.split('.').next()?.parse().ok())
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
        for app in ["MuseScore Studio 4.app", "MuseScore 4.app"] {
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
                for folder in ["MuseScore Studio 4", "MuseScore 4"] {
                    candidates.push(
                        PathBuf::from(&program_files)
                            .join(folder)
                            .join("bin/MuseScore4.exe"),
                    );
                }
            }
        }
    }
    for name in ["mscore4", "musescore4", "mscore", "musescore"] {
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
                OsString::from("-o"),
                OsString::from("mix.wav"),
                OsString::from("a score.mscz")
            ]
        );
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
    fn version_probe_requires_major_four_or_later() {
        assert_eq!(musescore_version_major("MuseScore 4.5.2"), Some(4));
        assert_eq!(musescore_version_major("MuseScore 3.6.2"), Some(3));
        assert_eq!(
            musescore_version_major("Qt 6.6.3 / MuseScore 3.6.2"),
            Some(3)
        );
        assert_eq!(musescore_version_major("not a version"), None);
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
                DrainEvent::Finished { result, .. } => {
                    result.unwrap();
                    break;
                }
            }
        }
        assert_eq!(retained.len(), 16);
    }
}
