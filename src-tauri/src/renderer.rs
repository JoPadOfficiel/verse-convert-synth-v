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
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

pub const DEFAULT_RENDER_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_MAX_WAV_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_LOG_BYTES: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(25);
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
            executable_sha256: sha256_file(&executable)?,
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
            renderer: self.capabilities.identity.clone(),
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
    let decoded_samples = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().try_fold(0_u64, |count, sample| {
            sample.map(|_| count.saturating_add(1))
        }),
        hound::SampleFormat::Int => reader.samples::<i32>().try_fold(0_u64, |count, sample| {
            sample.map(|_| count.saturating_add(1))
        }),
    }
    .map_err(|error| RenderError::InvalidWav {
        reason: format!("truncated or invalid sample data: {error}"),
    })?;
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

fn run_bounded_process(
    program: &Path,
    args: &[OsString],
    current_dir: &Path,
    timeout: Duration,
    monitored_output: Option<&Path>,
    max_output_bytes: u64,
) -> Result<ProcessResult, ProcessError> {
    for directory in ["config", "cache", "appdata", "localappdata", "tmp"] {
        fs::create_dir_all(current_dir.join(directory)).map_err(ProcessError::Io)?;
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
    let stdout_thread = drain_bounded(stdout, MAX_LOG_BYTES);
    let stderr_thread = drain_bounded(stderr, MAX_LOG_BYTES);

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(ProcessError::Io)? {
            break status;
        }
        if let Some(output) = monitored_output {
            if let Ok(metadata) = fs::metadata(output) {
                if metadata.len() > max_output_bytes {
                    terminate_process_tree(&mut child);
                    let _ = child.wait();
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    return Err(ProcessError::OutputTooLarge {
                        bytes: metadata.len(),
                        limit: max_output_bytes,
                    });
                }
            }
        }
        if started.elapsed() >= timeout {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Err(ProcessError::Timeout);
        }
        thread::sleep(POLL_INTERVAL);
    };
    let stdout = stdout_thread
        .join()
        .map_err(|_| ProcessError::Io(io::Error::other("stdout reader panicked")))?
        .map_err(ProcessError::Io)?;
    let stderr = stderr_thread
        .join()
        .map_err(|_| ProcessError::Io(io::Error::other("stderr reader panicked")))?
        .map_err(ProcessError::Io)?;
    Ok(ProcessResult {
        status,
        stdout,
        stderr,
    })
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
    let _ = child.kill();
}

fn drain_bounded<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut kept = Vec::with_capacity(limit.min(4096));
        let mut buffer = [0_u8; 8192];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            let remaining = limit.saturating_sub(kept.len());
            kept.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        Ok(kept)
    })
}

fn copy_required_environment(command: &mut Command, private_dir: &Path) {
    for key in ["SystemRoot", "WINDIR", "LANG", "LC_ALL"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env("HOME", private_dir);
    command.env("XDG_CONFIG_HOME", private_dir.join("config"));
    command.env("XDG_CACHE_HOME", private_dir.join("cache"));
    command.env("APPDATA", private_dir.join("appdata"));
    command.env("LOCALAPPDATA", private_dir.join("localappdata"));
    command.env("TMP", private_dir.join("tmp"));
    command.env("TEMP", private_dir.join("tmp"));
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
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                    }
                    return Ok(Self(path));
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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
    fn logs_are_bounded_while_the_pipe_is_fully_drained() {
        let handle = drain_bounded(Cursor::new(vec![b'x'; 1024]), 16);
        assert_eq!(handle.join().unwrap().unwrap().len(), 16);
    }
}
