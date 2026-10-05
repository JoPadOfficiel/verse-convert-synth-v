//! Local pronunciation evidence and explicitly confirmed correction memory.
pub mod commands;
pub mod exchange;
pub mod feedback;
pub mod files;
pub mod memory;
pub mod reading;
pub mod source_map;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
pub const POLICY: &str = "verse-pronunciation-v1";
pub const BASELINE: &str = "verse-lingua-lexical-context-v1";
// Legacy use cases stay callable without application state. Desktop commands
// bind a frozen snapshot only for their synchronous blocking worker; the guard
// restores it even on panic. No snapshot crosses an async suspension point.
thread_local! { static CURRENT: std::cell::RefCell<Option<Arc<Snapshot>>> = const { std::cell::RefCell::new(None) }; }
thread_local! { static OBSERVED: std::cell::RefCell<Option<(crate::engine::projection::ProjectedProject, Vec<source_map::Word>, String)>> = const { std::cell::RefCell::new(None) }; }
pub fn current_snapshot() -> Option<Arc<Snapshot>> {
    CURRENT.with(|s| s.borrow().clone())
}
pub fn with_snapshot<T>(snapshot: Arc<Snapshot>, action: impl FnOnce() -> T) -> T {
    struct Guard(Option<Arc<Snapshot>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            CURRENT.with(|s| *s.borrow_mut() = self.0.take());
        }
    }
    let _guard = Guard(CURRENT.with(|s| s.replace(Some(snapshot))));
    OBSERVED.with(|s| s.replace(None));
    action()
}
pub fn check_source(bytes: &[u8]) -> Result<(), Error> {
    check_work()?;
    if current_snapshot().is_some_and(|s| s.source_sha256 != hash(bytes)) {
        Err(Error::new(
            "PRONUNCIATION_SNAPSHOT_STALE",
            "Source changed since analysis; analyse again",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn check_work() -> Result<(), Error> {
    if let Some(snapshot) = current_snapshot() {
        snapshot.work.check()?;
    }
    Ok(())
}
/// Publication still honors cancellation after pronunciation has completed.
/// Its computation deadline does not bound separately timed audio rendering.
pub(crate) fn check_cancellation() -> Result<(), Error> {
    if let Some(snapshot) = current_snapshot() {
        snapshot.work.check_cancellation()?;
    }
    Ok(())
}
pub fn observe_projection(
    outcome: &crate::engine::convert::ConvertOutcome,
    target: crate::engine::target::ExportTarget,
) -> Result<(), Error> {
    let Some(snapshot) = current_snapshot() else {
        return Ok(());
    };
    snapshot.work.check()?;
    if let Some(project) = &outcome.svp {
        let bytes = crate::engine::target::serialize_to(target, project)
            .map_err(|e| Error::new("PRONUNCIATION_PROJECTION_INVALID", &e.to_string()))?;
        let digest = hash(&bytes);
        if snapshot.sealed && snapshot.expected_projection_sha256.is_none() {
            return Err(Error::new(
                "PRONUNCIATION_SNAPSHOT_STALE",
                "Export requires a complete approved projection",
            ));
        }
        if snapshot
            .expected_projection_sha256
            .as_ref()
            .is_some_and(|expected| expected != &digest)
        {
            return Err(Error::new(
                "PRONUNCIATION_SNAPSHOT_STALE",
                "Pronunciation plan differs from analysis; export refused",
            ));
        }
        if target == crate::engine::target::ExportTarget::Ustx
            && snapshot.memory_root.is_some()
            && project.pronunciation_profile
                == crate::engine::target::PronunciationProfile::Automatic
        {
            let reference = source_map::Reference::build(
                snapshot.source_sha256.clone(),
                snapshot.id.clone(),
                project,
                &outcome.pronunciation_words,
                &bytes,
            )?;
            reference.check_budget(&bytes)?;
        }
        OBSERVED.with(|s| {
            s.replace(Some((
                project.clone(),
                outcome.pronunciation_words.clone(),
                digest,
            )))
        });
    }
    snapshot.work.check()
}
pub fn observed_projection() -> Option<(
    crate::engine::projection::ProjectedProject,
    Vec<source_map::Word>,
    String,
)> {
    OBSERVED.with(|s| s.borrow().clone())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Error {
    pub code: String,
    pub message: String,
}
impl Error {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new("PRONUNCIATION_IO", &e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new("PRONUNCIATION_JSON_INVALID", &e.to_string())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::new("PRONUNCIATION_DATABASE", &e.to_string())
    }
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Language {
    #[serde(rename = "fr")]
    French,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "es")]
    Spanish,
    #[serde(rename = "pt")]
    Portuguese,
}
impl Language {
    pub fn native_name(self) -> &'static str {
        use crate::engine::target::diffsinger;
        match self {
            Self::French => diffsinger::FRENCH_NAME,
            Self::English => diffsinger::ENGLISH_NAME,
            Self::Spanish => diffsinger::SPANISH_NAME,
            Self::Portuguese => diffsinger::PORTUGUESE_NAME,
        }
    }
    pub fn projected(self) -> crate::engine::projection::PronunciationLanguage {
        use crate::engine::projection::PronunciationLanguage as P;
        match self {
            Self::French => P::French,
            Self::English => P::English,
            Self::Spanish => P::Spanish,
            Self::Portuguese => P::Portuguese,
        }
    }
    pub fn from_projected(value: crate::engine::projection::PronunciationLanguage) -> Self {
        use crate::engine::projection::PronunciationLanguage as P;
        match value {
            P::French => Self::French,
            P::English => Self::English,
            P::Spanish => Self::Spanish,
            P::Portuguese => Self::Portuguese,
        }
    }
    pub fn phonemizer(self) -> &'static str {
        use crate::engine::target::{diffsinger, english, french};
        match self {
            Self::French => french::PHONEMIZER,
            Self::English => english::PHONEMIZER,
            Self::Spanish => diffsinger::SPANISH_PHONEMIZER,
            Self::Portuguese => diffsinger::PORTUGUESE_PHONEMIZER,
        }
    }
}

#[derive(Clone)]
pub struct Work {
    pub cancelled: Arc<AtomicBool>,
    pub deadline: Instant,
}
impl Work {
    pub fn new(duration: Duration) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + duration,
        }
    }
    pub fn check(&self) -> Result<(), Error> {
        self.check_cancellation()?;
        if Instant::now() >= self.deadline {
            Err(Error::new(
                "PRONUNCIATION_DEADLINE",
                "Pronunciation resource deadline exceeded",
            ))
        } else {
            Ok(())
        }
    }
    pub(crate) fn check_cancellation(&self) -> Result<(), Error> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(Error::new(
                "PRONUNCIATION_CANCELLED",
                "Pronunciation operation was cancelled",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
pub struct Snapshot {
    pub id: String,
    pub source_sha256: String,
    pub corrections: Arc<memory::CorrectionSet>,
    pub work: Work,
    pub expected_projection_sha256: Option<String>,
    pub memory_root: Option<std::path::PathBuf>,
    pub source_label: Option<String>,
    pub sealed: bool,
}
impl Snapshot {
    pub fn baseline(
        source_sha256: String,
        corrections: Vec<memory::Correction>,
    ) -> Result<Self, Error> {
        Self::with_corrections(
            source_sha256,
            Arc::new(memory::CorrectionSet::new(corrections)?),
        )
    }
    pub fn with_corrections(
        source_sha256: String,
        corrections: Arc<memory::CorrectionSet>,
    ) -> Result<Self, Error> {
        let id = hash(&serde_json::to_vec(&(
            POLICY,
            BASELINE,
            &source_sha256,
            corrections.identity(),
        ))?);
        Ok(Self {
            id,
            source_sha256,
            corrections,
            work: Work::new(Duration::from_secs(30)),
            expected_projection_sha256: None,
            memory_root: None,
            source_label: None,
            sealed: false,
        })
    }
}

pub fn retain_reference(bytes: &[u8], export_label: Option<String>) -> Result<(), Error> {
    let Some(snapshot) = current_snapshot() else {
        return Ok(());
    };
    let Some(root) = &snapshot.memory_root else {
        return Ok(());
    };
    let Some((project, words, _)) = observed_projection() else {
        return Ok(());
    };
    if project.pronunciation_profile != crate::engine::target::PronunciationProfile::Automatic {
        return Ok(());
    }
    let mut reference = source_map::Reference::build(
        snapshot.source_sha256.clone(),
        snapshot.id.clone(),
        &project,
        &words,
        bytes,
    )?;
    reference.source_label = snapshot.source_label.clone();
    reference.export_label = export_label;
    reference.created_at_unix_seconds = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    );
    memory::Memory::open(root)?.save_reference(&reference, bytes)
}
