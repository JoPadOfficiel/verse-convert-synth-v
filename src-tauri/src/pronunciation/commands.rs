//! IPC adapters. Pronunciation and database work runs on blocking workers, never webview.
use super::{
    exchange::Package,
    feedback::{compare, Review},
    files::read_bounded,
    hash,
    memory::{HistoryRecord, Memory, Scope},
    source_map::Reference,
    with_snapshot, Error, Snapshot, Work,
};
use crate::{
    engine::target::{ExportTarget, PronunciationProfile},
    CommandErrorDto, FileResult,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::State;

#[derive(Clone)]
struct Plan {
    signature: String,
    snapshot: Arc<Snapshot>,
}
pub struct AppState {
    pub root: PathBuf,
    plans: Mutex<HashMap<String, Plan>>,
    active: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    next_operation: AtomicU64,
    retained_limit: usize,
    #[cfg(test)]
    test_renderer: Option<Arc<dyn crate::renderer::AudioRenderer>>,
    #[cfg(test)]
    test_liaison: Option<Arc<dyn crate::engine::lectura::LiaisonPredictor>>,
}
struct Operation {
    state: Arc<AppState>,
    id: u64,
    work: Work,
}
impl Drop for Operation {
    fn drop(&mut self) {
        if let Ok(mut active) = self.state.active.lock() {
            active.remove(&self.id);
        }
    }
}
impl AppState {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            plans: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            next_operation: AtomicU64::new(0),
            retained_limit: super::MAX_RETAINED_BYTES,
            #[cfg(test)]
            test_renderer: None,
            #[cfg(test)]
            test_liaison: None,
        }
    }
    fn operation(self: &Arc<Self>) -> Result<Operation, Error> {
        let id = self.next_operation.fetch_add(1, Ordering::Relaxed);
        let work = Work::new(Duration::from_secs(30));
        self.active
            .lock()
            .map_err(|_| Error::new("PRONUNCIATION_STATE", "Cancellation registry unavailable"))?
            .insert(id, work.cancelled.clone());
        Ok(Operation {
            state: self.clone(),
            id,
            work,
        })
    }
    pub fn cancel_running(&self) -> Result<(), Error> {
        for token in self
            .active
            .lock()
            .map_err(|_| Error::new("PRONUNCIATION_STATE", "Cancellation registry unavailable"))?
            .values()
        {
            token.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
    fn release(&self, ids: &[String]) -> Result<(), Error> {
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| Error::new("PRONUNCIATION_STATE", "Snapshot state unavailable"))?;
        for id in ids {
            plans.remove(id);
        }
        super::OBSERVED_OUTCOME.with(|s| s.replace(None));
        super::OBSERVED.with(|s| s.replace(None));
        Ok(())
    }
    fn publish_plan(
        &self,
        signature: String,
        mut snapshot: Snapshot,
        digest: String,
    ) -> Result<Arc<Snapshot>, Error> {
        snapshot.expected_projection_sha256 = Some(digest);
        let retained = super::OBSERVED_OUTCOME.with(|s| s.take());
        let outcome_identity = retained.as_ref().map(|r| &r.identity);
        snapshot.retained_bytes = retained.as_ref().map_or(0, |r| r.bytes);
        snapshot.sealed = true;
        snapshot.id = hash(&serde_json::to_vec(&(
            &snapshot.id,
            &snapshot.expected_projection_sha256,
            outcome_identity,
        ))?);
        snapshot.analysis_outcome = retained.map(|r| r.value);
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| Error::new("PRONUNCIATION_STATE", "Snapshot state unavailable"))?;
        if let Some(existing) = plans.get(&snapshot.id) {
            return Ok(existing.snapshot.clone());
        }
        if plans.len() >= 128 {
            return Err(Error::new(
                "PRONUNCIATION_SNAPSHOT_LIMIT",
                "Release unused analyses before publishing another preview",
            ));
        }
        let retained_bytes = plans
            .values()
            .try_fold(snapshot.retained_bytes, |bytes, plan| {
                bytes.checked_add(plan.snapshot.retained_bytes)
            });
        if retained_bytes.is_none_or(|bytes| bytes > self.retained_limit) {
            return Err(Error::new(
                "PRONUNCIATION_SNAPSHOT_LIMIT",
                "Retained analysis byte budget exceeded; release unused analyses",
            ));
        }
        let snapshot = Arc::new(snapshot);
        plans.insert(
            snapshot.id.clone(),
            Plan {
                signature,
                snapshot: snapshot.clone(),
            },
        );
        Ok(snapshot)
    }
    fn signature(
        path: &str,
        language: &Option<String>,
        overrides: &Option<HashMap<String, bool>>,
        target: Option<ExportTarget>,
        profile: Option<PronunciationProfile>,
    ) -> Result<String, Error> {
        let ordered: std::collections::BTreeMap<_, _> =
            overrides.clone().unwrap_or_default().into_iter().collect();
        Ok(hash(&serde_json::to_vec(&(
            path,
            language.as_deref().unwrap_or("english"),
            ordered,
            target.unwrap_or_default(),
            profile.unwrap_or_default(),
        ))?))
    }
    fn new_plan(
        &self,
        path: &str,
        signature: String,
        corrections: Arc<super::memory::CorrectionSet>,
        work: Work,
    ) -> Result<Arc<Snapshot>, Error> {
        let data = read_bounded(Path::new(path), crate::MAX_INPUT_BYTES)?;
        let mut snapshot = Snapshot::with_corrections(hash(&data), corrections)?;
        snapshot.memory_root = Some(self.root.clone());
        snapshot.source_label = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        snapshot.work = work;
        snapshot.id = hash(&serde_json::to_vec(&(&signature, &snapshot.id))?);
        Ok(Arc::new(snapshot))
    }

    fn export_plan(
        &self,
        id: Option<&str>,
        path: &str,
        signature: &str,
        work: Work,
    ) -> Result<Arc<Snapshot>, Error> {
        let plans = self
            .plans
            .lock()
            .map_err(|_| Error::new("PRONUNCIATION_STATE", "Snapshot state unavailable"))?;
        let plan = id
            .and_then(|id| plans.get(id))
            .filter(|p| {
                p.signature == signature
                    && p.snapshot.sealed
                    && p.snapshot.expected_projection_sha256.is_some()
            })
            .ok_or_else(|| {
                Error::new(
                    "PRONUNCIATION_SNAPSHOT_STALE",
                    "Missing or stale analysis snapshot; analyse again",
                )
            })?;
        if hash(&read_bounded(Path::new(path), crate::MAX_INPUT_BYTES)?)
            != plan.snapshot.source_sha256
            || Memory::open(&self.root)?.trusted_snapshot()?.identity()
                != plan.snapshot.corrections.identity()
        {
            return Err(Error::new(
                "PRONUNCIATION_SNAPSHOT_STALE",
                "Source or confirmed memory changed; analyse again",
            ));
        }
        let mut snapshot = (*plan.snapshot).clone();
        snapshot.work = work;
        snapshot.sealed = true;
        Ok(Arc::new(snapshot))
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pronunciation_convert_files(
    state: State<'_, Arc<AppState>>,
    paths: Vec<String>,
    write: bool,
    out_dir: Option<String>,
    language: Option<String>,
    overrides: Option<HashMap<String, HashMap<String, bool>>>,
    export_target: Option<ExportTarget>,
    pronunciation_profile: Option<PronunciationProfile>,
    snapshot_ids: Option<HashMap<String, String>>,
) -> Result<Vec<FileResult>, CommandErrorDto> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        convert_batch(
            state,
            paths,
            write,
            out_dir,
            language,
            overrides,
            export_target,
            pronunciation_profile,
            snapshot_ids,
        )
    })
    .await
    .map_err(|e| CommandErrorDto::new("PRONUNCIATION_WORKER", e.to_string()))?
    .map_err(CommandErrorDto::from)
}

fn file_error(path: &str, error: Error) -> FileResult {
    FileResult {
        pronunciation_snapshot_id: None,
        path: path.into(),
        name: Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .into(),
        ok: false,
        error: Some(CommandErrorDto::new(&error.code, error.message.clone())),
        msg: Some(error.message),
        n_parts: 0,
        n_voices: 0,
        n_tracks: 0,
        placed: 0,
        parts: vec![],
        tracks: vec![],
        audio_status: crate::AudioStatusDto::NotRendered,
        requires_voice_assignment: false,
        bundle_ready: false,
        warnings: vec![],
        out: None,
    }
}

fn liaison_enabled(write: bool, target: ExportTarget, profile: PronunciationProfile) -> bool {
    !write
        && target == ExportTarget::Ustx
        && matches!(
            profile,
            PronunciationProfile::FrenchMillefeuille | PronunciationProfile::Automatic
        )
}
#[allow(clippy::too_many_arguments)]
fn convert_batch(
    state: Arc<AppState>,
    paths: Vec<String>,
    write: bool,
    out_dir: Option<String>,
    language: Option<String>,
    overrides: Option<HashMap<String, HashMap<String, bool>>>,
    export_target: Option<ExportTarget>,
    pronunciation_profile: Option<PronunciationProfile>,
    snapshot_ids: Option<HashMap<String, String>>,
) -> Result<Vec<FileResult>, Error> {
    if paths.len() > 128 {
        return Err(Error::new(
            "PRONUNCIATION_FILE_LIMIT",
            "Too many files in one analysis batch",
        ));
    }
    let operation = state.operation()?;
    let corrections = Memory::open(&state.root)?.trusted_snapshot()?;
    let mut results = Vec::new();
    let use_liaison = liaison_enabled(
        write,
        export_target.unwrap_or_default(),
        pronunciation_profile.unwrap_or_default(),
    );
    #[cfg(not(test))]
    let lectura = use_liaison.then(crate::engine::lectura::Client::production);
    #[cfg(not(test))]
    let predictor = lectura
        .as_ref()
        .map(|c| c as &dyn crate::engine::lectura::LiaisonPredictor);
    #[cfg(test)]
    let predictor = use_liaison
        .then_some(state.test_liaison.as_deref())
        .flatten();
    let budgeted = predictor
        .map(|p| crate::engine::lectura::BudgetedPredictor::new(p, Duration::from_secs(8)));
    let predictor = budgeted
        .as_ref()
        .map(|p| p as &dyn crate::engine::lectura::LiaisonPredictor);
    for path in paths {
        let ov = overrides.as_ref().and_then(|m| m.get(&path)).cloned();
        let signature =
            AppState::signature(&path, &language, &ov, export_target, pronunciation_profile)?;
        let attempt = (|| -> Result<FileResult, Error> {
            let snapshot = if write {
                state.export_plan(
                    snapshot_ids
                        .as_ref()
                        .and_then(|ids| ids.get(&path))
                        .map(String::as_str),
                    &path,
                    &signature,
                    operation.work.clone(),
                )?
            } else {
                state.new_plan(
                    &path,
                    signature.clone(),
                    corrections.clone(),
                    operation.work.clone(),
                )?
            };
            let result = with_snapshot(snapshot, || {
                let parsed = ov.map(|m| {
                    m.into_iter()
                        .filter_map(|(k, v)| k.parse().ok().map(|k| (k, v)))
                        .collect()
                });
                let mut result = crate::process_one_with_predictor(
                    &path,
                    write,
                    out_dir.as_deref(),
                    language.as_deref().unwrap_or("english"),
                    parsed.as_ref(),
                    export_target.unwrap_or_default(),
                    pronunciation_profile.unwrap_or_default(),
                    predictor,
                );
                if !write && result.ok {
                    let (_, _, digest) = super::observed_projection().ok_or_else(|| {
                        Error::new(
                            "PRONUNCIATION_PROJECTION_INVALID",
                            "Analysis did not produce a complete projection",
                        )
                    })?;
                    let draft = (*super::current_snapshot().unwrap()).clone();
                    let published = state.publish_plan(signature.clone(), draft, digest)?;
                    result.pronunciation_snapshot_id = Some(published.id.clone());
                } else if !result.ok {
                    result.pronunciation_snapshot_id = None;
                }
                Ok::<_, Error>(result)
            })?;
            Ok(result)
        })();
        results.push(attempt.unwrap_or_else(|error| file_error(&path, error)));
    }
    Ok::<_, Error>(results)
}
#[tauri::command]
pub async fn pronunciation_release_snapshots(
    state: State<'_, Arc<AppState>>,
    snapshot_ids: Vec<String>,
) -> Result<(), CommandErrorDto> {
    state.release(&snapshot_ids).map_err(CommandErrorDto::from)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pronunciation_export_svp(
    state: State<'_, Arc<AppState>>,
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
    export_target: Option<ExportTarget>,
    pronunciation_profile: Option<PronunciationProfile>,
    snapshot_id: Option<String>,
) -> Result<String, CommandErrorDto> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        export_vocals(
            state,
            path,
            target,
            language,
            overrides,
            export_target,
            pronunciation_profile,
            snapshot_id,
        )
    })
    .await
    .map_err(|e| CommandErrorDto::new("PRONUNCIATION_WORKER", e.to_string()))?
}

#[allow(clippy::too_many_arguments)]
fn export_vocals(
    state: Arc<AppState>,
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
    export_target: Option<ExportTarget>,
    pronunciation_profile: Option<PronunciationProfile>,
    snapshot_id: Option<String>,
) -> Result<String, CommandErrorDto> {
    let operation = state.operation()?;
    let signature = AppState::signature(
        &path,
        &language,
        &overrides,
        export_target,
        pronunciation_profile,
    )?;
    let snapshot = state.export_plan(
        snapshot_id.as_deref(),
        &path,
        &signature,
        operation.work.clone(),
    )?;
    with_snapshot(snapshot, || {
        crate::export_svp(
            path,
            target,
            language,
            overrides,
            export_target,
            pronunciation_profile,
        )
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pronunciation_export_bundle(
    state: State<'_, Arc<AppState>>,
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
    renderer_path: Option<String>,
    export_target: Option<ExportTarget>,
    pronunciation_profile: Option<PronunciationProfile>,
    snapshot_id: Option<String>,
    on_progress: tauri::ipc::Channel<crate::bundle::BundleProgressEvent>,
) -> Result<crate::bundle::BundleResult, CommandErrorDto> {
    let state = state.inner().clone();
    #[cfg(test)]
    let test_renderer = state.test_renderer.clone();
    tauri::async_runtime::spawn_blocking(move || {
        export_bundle_service(
            state,
            path,
            target,
            language,
            overrides,
            renderer_path,
            export_target,
            pronunciation_profile,
            snapshot_id,
            on_progress,
            #[cfg(test)]
            test_renderer,
        )
    })
    .await
    .map_err(|e| CommandErrorDto::new("PRONUNCIATION_WORKER", e.to_string()))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    pub history: Vec<HistoryRecord>,
    pub references: Vec<super::memory::ReferenceInfo>,
    pub baseline: String,
}
#[allow(clippy::too_many_arguments)]
fn export_bundle_service(
    state: Arc<AppState>,
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
    renderer_path: Option<String>,
    export_target: Option<ExportTarget>,
    pronunciation_profile: Option<PronunciationProfile>,
    snapshot_id: Option<String>,
    on_progress: tauri::ipc::Channel<crate::bundle::BundleProgressEvent>,
    #[cfg(test)] test_renderer: Option<Arc<dyn crate::renderer::AudioRenderer>>,
) -> Result<crate::bundle::BundleResult, CommandErrorDto> {
    let operation = state.operation()?;
    let signature = AppState::signature(
        &path,
        &language,
        &overrides,
        export_target,
        pronunciation_profile,
    )?;
    let snapshot = state.export_plan(
        snapshot_id.as_deref(),
        &path,
        &signature,
        operation.work.clone(),
    )?;
    with_snapshot(snapshot, || {
        crate::export_bundle_blocking(
            path,
            target,
            language,
            overrides,
            renderer_path,
            export_target,
            pronunciation_profile,
            &|event| {
                let _ = on_progress.send(event);
            },
            #[cfg(test)]
            test_renderer,
        )
    })
}

#[tauri::command]
pub async fn pronunciation_memory(
    state: State<'_, Arc<AppState>>,
) -> Result<MemoryView, CommandErrorDto> {
    let state = state.inner().clone();
    worker(move || {
        let memory = Memory::open(&state.root)?;
        Ok(MemoryView {
            history: memory.history()?,
            references: memory.references()?,
            baseline: super::BASELINE.into(),
        })
    })
    .await
}
async fn worker<T: Send + 'static>(
    action: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, CommandErrorDto> {
    tauri::async_runtime::spawn_blocking(action)
        .await
        .map_err(|e| CommandErrorDto::new("PRONUNCIATION_WORKER", e.to_string()))?
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn pronunciation_compare(
    state: State<'_, Arc<AppState>>,
    reference_id: String,
    corrected_path: String,
) -> Result<Review, CommandErrorDto> {
    let root = state.root.clone();
    worker(move || {
        let reference = Memory::open(&root)?.reference(&reference_id)?;
        let (digest, tracks) = Reference::read_corrected(Path::new(&corrected_path))?;
        compare(&reference, digest, &tracks)
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirmation {
    pub correction_id: String,
    pub scope: Scope,
    pub listened: bool,
}
#[tauri::command]
pub async fn pronunciation_confirm(
    state: State<'_, Arc<AppState>>,
    reference_id: String,
    corrected_path: String,
    selections: Vec<Confirmation>,
) -> Result<(), CommandErrorDto> {
    let root = state.root.clone();
    worker(move || confirm_review(&root, &reference_id, &corrected_path, selections)).await
}
fn confirm_review(
    root: &Path,
    reference_id: &str,
    corrected_path: &str,
    selections: Vec<Confirmation>,
) -> Result<(), Error> {
    let mut memory = Memory::open(root)?;
    let reference = memory.reference(reference_id)?;
    let (digest, tracks) = Reference::read_corrected(Path::new(&corrected_path))?;
    let review = compare(&reference, digest, &tracks)?;
    let mut records = Vec::new();
    let mut selected = std::collections::BTreeSet::new();
    for selection in selections {
        if !selection.listened || !selected.insert(selection.correction_id.clone()) {
            return Err(Error::new(
                "PRONUNCIATION_CONFIRMATION_REQUIRED",
                "Each selected correction needs explicit confirmation after listening",
            ));
        }
        let mut record = review
            .proposals
            .iter()
            .find(|c| c.id == selection.correction_id)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "PRONUNCIATION_SNAPSHOT_STALE",
                    "Correction proposal changed; compare again",
                )
            })?;
        record.scope = selection.scope;
        record.provenance.confirmed_after_listening = true;
        record.seal()?;
        record.validate()?;
        records.push(record);
    }
    memory.store(&records, true)
}

#[tauri::command]
pub async fn pronunciation_set_status(
    state: State<'_, Arc<AppState>>,
    id: String,
    status: String,
    confirmed: bool,
) -> Result<(), CommandErrorDto> {
    let root = state.root.clone();
    worker(move || {
        if status == "active" && !confirmed {
            return Err(Error::new(
                "PRONUNCIATION_CONFIRMATION_REQUIRED",
                "Imported correction needs explicit local confirmation",
            ));
        }
        Memory::open(&root)?.set_status(&id, &status)
    })
    .await
}
#[tauri::command]
pub async fn pronunciation_exchange(
    state: State<'_, Arc<AppState>>,
    path: String,
    import: bool,
) -> Result<(), CommandErrorDto> {
    let root = state.root.clone();
    worker(move || {
        let mut memory = Memory::open(&root)?;
        if import {
            Package::read(Path::new(&path))?.import_pending(&mut memory)
        } else {
            Package::confirmed(&memory)?.write(Path::new(&path))
        }
    })
    .await
}
#[tauri::command]
pub fn pronunciation_cancel(state: State<'_, Arc<AppState>>) -> Result<(), CommandErrorDto> {
    state.cancel_running().map_err(CommandErrorDto::from)
}

#[cfg(test)]
mod tests {
    use super::super::memory::Correction;
    use super::*;
    use tauri::Manager;

    fn state() -> (PathBuf, Arc<AppState>, String, String) {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "verse-pronunciation-command-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("source.xml");
        std::fs::write(&path,b"<score-partwise><part-list><score-part id=\"P1\"><part-name>Voice</part-name></score-part></part-list><part id=\"P1\"><measure number=\"1\"><attributes><divisions>480</divisions></attributes><note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><lyric><text>hello</text></lyric></note></measure></part></score-partwise>").unwrap();
        let state = Arc::new(AppState::new(root.join("memory")));
        let path = path.to_string_lossy().to_string();
        let signature = AppState::signature(
            &path,
            &None,
            &None,
            Some(ExportTarget::Ustx),
            Some(PronunciationProfile::Automatic),
        )
        .unwrap();
        (root, state, path, signature)
    }
    fn analysed(state: &Arc<AppState>, path: &str) -> Arc<Snapshot> {
        let result = convert_batch(
            state.clone(),
            vec![path.into()],
            false,
            None,
            None,
            None,
            Some(ExportTarget::Ustx),
            Some(PronunciationProfile::Automatic),
            None,
        )
        .unwrap()
        .remove(0);
        assert!(result.ok, "{:?}", result.msg);
        state.plans.lock().unwrap()[result.pronunciation_snapshot_id.as_ref().unwrap()]
            .snapshot
            .clone()
    }

    fn french_state() -> (PathBuf, Arc<AppState>, String, String) {
        let result = state();
        let xml = std::fs::read_to_string(&result.2).unwrap().replace("<text>hello</text>", "<text>tes</text>")
            .replace("</measure>", "<note><pitch><step>D</step><octave>4</octave></pitch><duration>480</duration><lyric><text>yeux</text></lyric></note></measure>");
        std::fs::write(&result.2, xml).unwrap();
        result
    }

    #[test]
    fn identical_native_bytes_keep_distinct_success_and_fallback_diagnostics_in_either_order() {
        use crate::engine::lectura::{Failure, Label, LiaisonPredictor, LOOKUP_FAILED};
        struct Switch(AtomicBool);
        impl LiaisonPredictor for Switch {
            fn predict(&self, words: &[String]) -> Result<Vec<Label>, Failure> {
                assert_eq!(words, ["tes", "yeux"]);
                if self.0.load(Ordering::Relaxed) {
                    Err(Failure::Http)
                } else {
                    Ok(vec![Label::Lz, Label::None])
                }
            }
        }
        for failure_first in [false, true] {
            let (root, mut state, path, signature) = french_state();
            let predictor = Arc::new(Switch(AtomicBool::new(failure_first)));
            Arc::get_mut(&mut state).unwrap().test_liaison = Some(predictor.clone());
            let first = analysed(&state, &path);
            predictor.0.store(!failure_first, Ordering::Relaxed);
            let second = analysed(&state, &path);
            assert_eq!(
                first.expected_projection_sha256,
                second.expected_projection_sha256
            );
            assert_ne!(first.id, second.id);
            for (approved, failed) in [(&first, failure_first), (&second, !failure_first)] {
                let exported = state
                    .export_plan(
                        Some(&approved.id),
                        &path,
                        &signature,
                        Work::new(Duration::from_secs(30)),
                    )
                    .unwrap();
                let outcome = exported.analysis_outcome.as_ref().unwrap();
                assert_eq!(
                    outcome
                        .tracks
                        .iter()
                        .flat_map(|t| &t.warnings)
                        .any(|d| d.code == LOOKUP_FAILED),
                    failed
                );
            }
            assert!(super::super::OBSERVED_OUTCOME.with(|s| s.borrow().is_none()));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cached_engine_inputs_refuse_changed_ppq_and_overrides_and_hash_order_deterministically() {
        use crate::engine::convert::convert_midi_with_snapshot;
        let (root, state, path, _) = french_state();
        let approved = analysed(&state, &path);
        let bytes = std::fs::read(&path).unwrap();
        let mut midi = crate::parse_source_snapshot(&bytes, "xml").unwrap();
        midi.ticks_per_beat = 479;
        midi.time_base = crate::engine::midi::TimeBase::PulsesPerQuarter(479);
        let changed = convert_midi_with_snapshot(
            &midi,
            "english",
            None,
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            Some(&approved),
        );
        assert!(!changed.ok);
        assert!(changed.svp.is_none());
        assert!(changed
            .source_warnings
            .iter()
            .any(|d| d.code == "PRONUNCIATION_SNAPSHOT_STALE"));
        let midi = crate::parse_source_snapshot(&bytes, "xml").unwrap();
        let changed = convert_midi_with_snapshot(
            &midi,
            "english",
            Some(&HashMap::from([(0, false)])),
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            Some(&approved),
        );
        assert!(!changed.ok);
        assert!(changed
            .source_warnings
            .iter()
            .any(|d| d.code == "PRONUNCIATION_SNAPSHOT_STALE"));
        with_snapshot(approved.clone(), || {
            assert_eq!(
                super::super::observe_projection(&changed, ExportTarget::Ustx)
                    .unwrap_err()
                    .code,
                "PRONUNCIATION_SNAPSHOT_STALE"
            )
        });
        let mut left = HashMap::new();
        left.insert(0, true);
        left.insert(1, false);
        let mut right = HashMap::new();
        right.insert(1, false);
        right.insert(0, true);
        let draft = Snapshot::baseline(super::super::hash(&bytes), vec![]).unwrap();
        let a = convert_midi_with_snapshot(
            &midi,
            "english",
            Some(&left),
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            Some(&draft),
        );
        let b = convert_midi_with_snapshot(
            &midi,
            "english",
            Some(&right),
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            Some(&draft),
        );
        assert!(a.ok && b.ok);
        assert_eq!(a.engine_inputs, b.engine_inputs);
        let mut sealed = draft;
        sealed.sealed = true;
        sealed.analysis_outcome = Some(Arc::new(a));
        assert!(
            convert_midi_with_snapshot(
                &midi,
                "english",
                Some(&right),
                ExportTarget::Ustx,
                PronunciationProfile::Automatic,
                Some(&sealed)
            )
            .ok
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retained_byte_budget_and_release_drop_the_outcome_and_observer_reference() {
        let (root, mut state, path, _) = french_state();
        let approved = analysed(&state, &path);
        assert!(approved.retained_bytes > 0);
        Arc::get_mut(&mut state).unwrap().retained_limit = approved.retained_bytes;
        let weak = Arc::downgrade(approved.analysis_outcome.as_ref().unwrap());
        let second = root.join("second.xml").to_string_lossy().into_owned();
        std::fs::copy(&path, &second).unwrap();
        let denied = convert_batch(
            state.clone(),
            vec![second.clone()],
            false,
            None,
            None,
            None,
            Some(ExportTarget::Ustx),
            Some(PronunciationProfile::Automatic),
            None,
        )
        .unwrap()
        .remove(0);
        assert!(!denied.ok);
        assert_eq!(denied.error.unwrap().code, "PRONUNCIATION_SNAPSHOT_LIMIT");
        assert!(super::super::OBSERVED_OUTCOME.with(|s| s.borrow().is_none()));
        let id = approved.id.clone();
        drop(approved);
        state.release(&[id]).unwrap();
        assert!(weak.upgrade().is_none());
        assert!(state.plans.lock().unwrap().is_empty());
        assert!(super::super::observed_projection().is_none());
        assert!(analysed(&state, &second).retained_bytes > 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_and_test_activation_share_every_profile_target_and_export_guard() {
        use crate::engine::lectura::{Failure, Label, LiaisonPredictor};
        struct Count(AtomicU64);
        impl LiaisonPredictor for Count {
            fn predict(&self, words: &[String]) -> Result<Vec<Label>, Failure> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(vec![Label::None; words.len()])
            }
        }
        let (root, mut state, path, _) = french_state();
        let predictor = Arc::new(Count(AtomicU64::new(0)));
        Arc::get_mut(&mut state).unwrap().test_liaison = Some(predictor.clone());
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            for profile in [
                PronunciationProfile::Automatic,
                PronunciationProfile::FrenchMillefeuille,
                PronunciationProfile::Default,
                PronunciationProfile::EnglishArpabet,
                PronunciationProfile::SpanishDiffSinger,
                PronunciationProfile::PortugueseDiffSinger,
            ] {
                let before = predictor.0.load(Ordering::Relaxed);
                let analysis = convert_batch(
                    state.clone(),
                    vec![path.clone()],
                    false,
                    None,
                    None,
                    None,
                    Some(target),
                    Some(profile),
                    None,
                )
                .unwrap()
                .remove(0);
                assert!(analysis.ok, "{:?}", analysis.msg);
                let expected = target == ExportTarget::Ustx
                    && matches!(
                        profile,
                        PronunciationProfile::Automatic | PronunciationProfile::FrenchMillefeuille
                    );
                assert_eq!(
                    predictor.0.load(Ordering::Relaxed) - before,
                    u64::from(expected)
                );
                assert_eq!(liaison_enabled(false, target, profile), expected);
                assert!(!liaison_enabled(true, target, profile));
                let directory = root.join(format!("{target:?}-{profile:?}"));
                std::fs::create_dir(&directory).unwrap();
                let after = predictor.0.load(Ordering::Relaxed);
                let export = convert_batch(
                    state.clone(),
                    vec![path.clone()],
                    true,
                    Some(directory.to_string_lossy().into_owned()),
                    None,
                    None,
                    Some(target),
                    Some(profile),
                    Some(HashMap::from([(
                        path.clone(),
                        analysis.pronunciation_snapshot_id.unwrap(),
                    )])),
                )
                .unwrap()
                .remove(0);
                assert!(export.ok, "{:?}", export.msg);
                assert_eq!(predictor.0.load(Ordering::Relaxed), after);
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_service_is_looked_up_once_per_batch_and_all_files_keep_local_fallback() {
        use crate::engine::lectura::{Failure, Label, LiaisonPredictor, LOOKUP_FAILED};
        struct Unavailable(AtomicU64);
        impl LiaisonPredictor for Unavailable {
            fn predict(&self, _: &[String]) -> Result<Vec<Label>, Failure> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Err(Failure::Timeout)
            }
        }
        let (root, mut state, path, _) = french_state();
        let predictor = Arc::new(Unavailable(AtomicU64::new(0)));
        Arc::get_mut(&mut state).unwrap().test_liaison = Some(predictor.clone());
        let mut paths = vec![path.clone()];
        for index in 1..3 {
            let other = root
                .join(format!("source-{index}.xml"))
                .to_string_lossy()
                .into_owned();
            std::fs::copy(&path, &other).unwrap();
            paths.push(other);
        }
        let results = convert_batch(
            state.clone(),
            paths,
            false,
            None,
            None,
            None,
            Some(ExportTarget::Ustx),
            Some(PronunciationProfile::FrenchMillefeuille),
            None,
        )
        .unwrap();
        assert_eq!(predictor.0.load(Ordering::Relaxed), 1);
        for result in results {
            assert!(result.ok, "{:?}", result.msg);
            assert!(result
                .tracks
                .iter()
                .flat_map(|t| &t.warnings)
                .any(|d| d.code == LOOKUP_FAILED));
            let cached = state.plans.lock().unwrap()
                [result.pronunciation_snapshot_id.as_ref().unwrap()]
            .snapshot
            .analysis_outcome
            .clone()
            .unwrap();
            let bytes = crate::engine::target::serialize_to(
                ExportTarget::Ustx,
                cached.svp.as_ref().unwrap(),
            )
            .unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains("yeux[fr/z fr/y fr/ee]"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lectura_analysis_is_reused_by_direct_batch_and_bundle_exports() {
        use crate::engine::lectura::{Failure, Label, LiaisonPredictor};
        struct Count(AtomicU64);
        impl LiaisonPredictor for Count {
            fn predict(&self, words: &[String]) -> Result<Vec<Label>, Failure> {
                assert_eq!(words, ["tes", "yeux"]);
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(vec![Label::None; words.len()])
            }
        }
        let (root, mut state, path, _) = state();
        let predictor = Arc::new(Count(AtomicU64::new(0)));
        Arc::get_mut(&mut state).unwrap().test_liaison = Some(predictor.clone());
        std::fs::write(&path, b"<score-partwise><part-list><score-part id=\"P1\"><part-name>Voice</part-name></score-part></part-list><part id=\"P1\"><measure number=\"1\"><attributes><divisions>480</divisions></attributes><note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><lyric><text>tes</text></lyric></note><note><pitch><step>D</step><octave>4</octave></pitch><duration>480</duration><lyric><text>yeux</text></lyric></note></measure></part></score-partwise>").unwrap();
        for profile in [
            PronunciationProfile::Automatic,
            PronunciationProfile::FrenchMillefeuille,
        ] {
            let analysis = convert_batch(
                state.clone(),
                vec![path.clone()],
                false,
                None,
                None,
                None,
                Some(ExportTarget::Ustx),
                Some(profile),
                None,
            )
            .unwrap()
            .remove(0);
            assert!(analysis.ok, "{:?}", analysis.msg);
            let id = analysis.pronunciation_snapshot_id.unwrap();
            let approved = state.plans.lock().unwrap()[&id].snapshot.clone();
            let expected = crate::engine::target::serialize_to(
                ExportTarget::Ustx,
                approved
                    .analysis_outcome
                    .as_ref()
                    .unwrap()
                    .svp
                    .as_ref()
                    .unwrap(),
            )
            .unwrap();
            assert!(!String::from_utf8_lossy(&expected).contains("fr/z"));
            let calls = predictor.0.load(Ordering::Relaxed);
            let direct = root.join(format!("{profile:?}.ustx"));
            export_vocals(
                state.clone(),
                path.clone(),
                direct.to_string_lossy().into_owned(),
                None,
                None,
                Some(ExportTarget::Ustx),
                Some(profile),
                Some(id.clone()),
            )
            .unwrap();
            assert_eq!(std::fs::read(direct).unwrap(), expected);
            let out = root.join(format!("batch-{profile:?}"));
            std::fs::create_dir(&out).unwrap();
            let exported = convert_batch(
                state.clone(),
                vec![path.clone()],
                true,
                Some(out.to_string_lossy().into_owned()),
                None,
                None,
                Some(ExportTarget::Ustx),
                Some(profile),
                Some(HashMap::from([(path.clone(), id.clone())])),
            )
            .unwrap()
            .remove(0);
            assert!(exported.ok, "{:?}", exported.msg);
            assert_eq!(std::fs::read(exported.out.unwrap()).unwrap(), expected);
            let bundle = export_bundle_service(
                state.clone(),
                path.clone(),
                root.join(format!("bundle-{profile:?}.versebundle"))
                    .to_string_lossy()
                    .into_owned(),
                None,
                None,
                None,
                Some(ExportTarget::Ustx),
                Some(profile),
                Some(id),
                tauri::ipc::Channel::new(|_| Ok(())),
                Some(crate::bundle::tests::successful_renderer()),
            )
            .unwrap();
            let native: serde_yaml::Value =
                serde_yaml::from_slice(&std::fs::read(bundle.project_path).unwrap()).unwrap();
            let direct: serde_yaml::Value = serde_yaml::from_slice(&expected).unwrap();
            assert_eq!(native["voice_parts"], direct["voice_parts"]);
            assert_eq!(predictor.0.load(Ordering::Relaxed), calls);
        }
        assert_eq!(predictor.0.load(Ordering::Relaxed), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires VERSE_COMPLETE_SOURCE, VERSE_MUSESCORE_GATE and a new VERSE_COMPLETE_OUTPUT_DIR"]
    fn supplied_complete_export_uses_frozen_app_snapshot_through_native_audio() {
        let path = std::env::var("VERSE_COMPLETE_SOURCE").expect("required real score");
        let renderer = std::env::var("VERSE_MUSESCORE_GATE").expect("required native renderer");
        let root = PathBuf::from(
            std::env::var_os("VERSE_COMPLETE_OUTPUT_DIR").expect("required new receipt directory"),
        );
        std::fs::create_dir(&root).expect("receipt directory must not exist");
        let source_bytes = std::fs::read(&path).unwrap();
        let before_hash = super::super::hash(&source_bytes);
        assert_eq!(
            before_hash, "c688a0d62fd63c0157f2bc84266328da24355ee7b49d7291e930086f743f4b48",
            "pin the reported legacy score"
        );
        let state = Arc::new(AppState::new(root.join("memory")));
        for target in [ExportTarget::Ustx, ExportTarget::Svp] {
            let analysis = convert_batch(
                state.clone(),
                vec![path.clone()],
                false,
                None,
                None,
                None,
                Some(target),
                Some(PronunciationProfile::Automatic),
                None,
            )
            .unwrap()
            .remove(0);
            assert!(analysis.ok, "{:?}", analysis.msg);
            assert_eq!(analysis.n_parts, 6);
            let expected = super::super::observed_projection().unwrap().0;
            assert_eq!(expected.tracks.len(), 4);
            let expected_bytes = crate::engine::target::serialize_to(target, &expected).unwrap();
            let snapshot_id = analysis.pronunciation_snapshot_id.unwrap();
            let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
            let sink = events.clone();
            let channel = tauri::ipc::Channel::new(move |body| {
                if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                    sink.lock()
                        .unwrap()
                        .push(serde_json::from_str(&json).unwrap());
                }
                Ok(())
            });
            let destination = root.join(format!("Real-{}.versebundle", target.extension()));
            let started = std::time::Instant::now();
            let result = export_bundle_service(
                state.clone(),
                path.clone(),
                destination.to_string_lossy().into_owned(),
                None,
                None,
                Some(renderer.clone()),
                Some(target),
                Some(PronunciationProfile::Automatic),
                Some(snapshot_id),
                channel,
                None,
            )
            .unwrap();
            let elapsed = started.elapsed().as_secs_f64();
            assert_eq!(result.stem_count, 6);
            assert_eq!(std::fs::read(&result.source_path).unwrap(), source_bytes);
            assert_eq!(
                super::super::hash(&std::fs::read(&path).unwrap()),
                before_hash
            );
            let saved = std::fs::read(&result.project_path).unwrap();
            match target {
                ExportTarget::Ustx => {
                    let saved: serde_yaml::Value = serde_yaml::from_slice(&saved).unwrap();
                    let direct: serde_yaml::Value =
                        serde_yaml::from_slice(&expected_bytes).unwrap();
                    assert_eq!(saved["voice_parts"], direct["voice_parts"]);
                    assert_eq!(saved["tempos"], direct["tempos"]);
                    assert_eq!(saved["wave_parts"].as_sequence().unwrap().len(), 7);
                }
                ExportTarget::Svp => {
                    let saved: serde_json::Value = serde_json::from_slice(&saved).unwrap();
                    let direct: serde_json::Value =
                        serde_json::from_slice(&expected_bytes).unwrap();
                    assert_eq!(
                        &saved["tracks"].as_array().unwrap()[..expected.tracks.len()],
                        direct["tracks"].as_array().unwrap()
                    );
                    assert_eq!(saved["time"], direct["time"]);
                    assert_eq!(saved["tracks"].as_array().unwrap().len(), 11);
                }
            }
            if target == ExportTarget::Ustx {
                let memory = Memory::open(&state.root).unwrap();
                let references = memory.references().unwrap();
                assert_eq!(references.len(), 1);
                let reference = memory.reference(&references[0].id).unwrap();
                assert_eq!(reference.export_sha256, super::super::hash(&saved));
                assert!(!reference.words.is_empty());
            }
            let manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&result.manifest_path).unwrap()).unwrap();
            assert_eq!(manifest["audio"]["coverage"]["complete"], true);
            let events = events.lock().unwrap();
            assert_eq!(events.last().unwrap()["phase"], "finished");
            assert_eq!(events.last().unwrap()["completed"], 10);
            let receipt = serde_json::json!({"target":target.extension(),"sourceSha256":before_hash,"elapsedSeconds":elapsed,"pronunciationDeadlineSeconds":30,"completedAfterPronunciationDeadline":elapsed>30.0,"aggregateRendererLimitSeconds":crate::renderer::DEFAULT_RENDER_TIMEOUT.as_secs(),"sourceParts":6,"vocalTracks":4,"projectedLyrics":analysis.placed,"stems":6,"completeSnapshotBoundExport":true,"vocalContentAndTempoPreserved":true,"sourceUnchanged":true,"events":*events,"bundle":destination});
            std::fs::write(
                root.join(format!("verification-{}.json", target.extension())),
                serde_json::to_vec_pretty(&receipt).unwrap(),
            )
            .unwrap();
            eprintln!("{receipt}");
        }
    }

    #[test]
    fn exports_refuse_missing_unknown_or_wrong_signature_snapshot_ids() {
        let (root, state, path, signature) = state();
        let operation = state.operation().unwrap();
        let snapshot = state
            .new_plan(
                &path,
                signature.clone(),
                Arc::new(super::super::memory::CorrectionSet::new(vec![]).unwrap()),
                operation.work.clone(),
            )
            .unwrap();
        for (id, requested_signature) in [
            (None, signature.as_str()),
            (Some("unknown-analysis"), signature.as_str()),
            (Some(snapshot.id.as_str()), "different-export-options"),
        ] {
            assert_eq!(
                state
                    .export_plan(id, &path, requested_signature, operation.work.clone())
                    .err()
                    .unwrap()
                    .code,
                "PRONUNCIATION_SNAPSHOT_STALE"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_or_expired_direct_exports_create_no_output() {
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            for cancelled in [true, false] {
                let (root, state, path, signature) = state();
                let approved = analysed(&state, &path);
                let operation = state.operation().unwrap();
                let mut snapshot = state
                    .export_plan(
                        Some(&approved.id),
                        &path,
                        &signature,
                        operation.work.clone(),
                    )
                    .unwrap();
                if cancelled {
                    state.cancel_running().unwrap();
                } else {
                    Arc::make_mut(&mut snapshot).work.deadline = std::time::Instant::now();
                }
                let output = root.join(format!("cancelled.{}", target.extension()));
                let error = with_snapshot(snapshot, || {
                    crate::export_svp(
                        path,
                        output.to_string_lossy().into_owned(),
                        None,
                        None,
                        Some(target),
                        Some(PronunciationProfile::Automatic),
                    )
                })
                .unwrap_err();
                assert_eq!(
                    error.code,
                    if cancelled {
                        "PRONUNCIATION_CANCELLED"
                    } else {
                        "PRONUNCIATION_DEADLINE"
                    }
                );
                assert!(!output.exists());
                assert!(Memory::open(&state.root)
                    .unwrap()
                    .references()
                    .unwrap()
                    .is_empty());
                drop(operation);
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[test]
    fn delayed_export_has_fresh_work_and_source_memory_changes_are_refused() {
        let (root, state, path, signature) = state();
        let operation = state.operation().unwrap();
        let snapshot = analysed(&state, &path);
        let id = snapshot.id.clone();
        let bytes = std::fs::read(&path).unwrap();
        let midi = crate::engine::musicxml::parse(&bytes).unwrap();
        let outcome = crate::engine::convert::convert_midi_with_snapshot(
            &midi,
            "english",
            None,
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            None,
        );
        let original =
            crate::engine::target::serialize_to(ExportTarget::Ustx, outcome.svp.as_ref().unwrap())
                .unwrap();
        {
            let mut plans = state.plans.lock().unwrap();
            let snapshot = Arc::make_mut(&mut plans.get_mut(&id).unwrap().snapshot);
            snapshot.work.deadline = std::time::Instant::now() - Duration::from_secs(31);
            snapshot.expected_projection_sha256 = Some(hash(&original));
            snapshot.sealed = true;
        }
        let export_operation = state.operation().unwrap();
        let export = state
            .export_plan(Some(&id), &path, &signature, export_operation.work.clone())
            .unwrap();
        assert!(export.work.check().is_ok());
        with_snapshot(export.clone(), || {
            super::super::check_source(&bytes).unwrap();
            let result = crate::engine::convert::convert_midi_with_snapshot(
                &midi,
                "english",
                None,
                ExportTarget::Ustx,
                PronunciationProfile::Automatic,
                Some(&export),
            );
            super::super::observe_projection(&result, ExportTarget::Ustx).unwrap();
            let output = root.join("delayed.ustx");
            crate::export_svp(
                path.clone(),
                output.to_string_lossy().into_owned(),
                None,
                None,
                Some(ExportTarget::Ustx),
                Some(PronunciationProfile::Automatic),
            )
            .unwrap();
            assert!(output.is_file());
        });
        std::fs::write(&path, b"changed source").unwrap();
        assert_eq!(
            state
                .export_plan(Some(&id), &path, &signature, export_operation.work.clone())
                .err()
                .unwrap()
                .code,
            "PRONUNCIATION_SNAPSHOT_STALE"
        );
        std::fs::write(&path, &bytes).unwrap();
        let word = outcome.pronunciation_words[0].clone();
        let mut correction = Correction {
            id: String::new(),
            fingerprint: String::new(),
            word,
            before: super::super::source_map::Reading {
                authority: None,
                aliases: None,
                language: Some(super::super::Language::English),
                phonemizer: super::super::Language::English.native_name().into(),
                lexical_reading: Some("hello".into()),
                phones: None,
                alphabet: None,
            },
            after: super::super::source_map::Reading {
                authority: None,
                aliases: None,
                language: Some(super::super::Language::French),
                phonemizer: super::super::Language::French.native_name().into(),
                lexical_reading: Some("hello".into()),
                phones: None,
                alphabet: None,
            },
            dialect: None,
            accepted_variant: None,
            scope: Scope::CompatibleContext,
            target: "ustx".into(),
            profile: "automatic".into(),
            voice: None,
            provenance: super::super::memory::Provenance {
                source_sha256: hash(&bytes),
                export_sha256: hash(&original),
                corrected_sha256: hash(b"corrected"),
                confirmed_after_listening: true,
                symbol_validation: "unknown".into(),
                policy: super::super::POLICY.into(),
                observed_singer: None,
            },
        };
        correction.seal().unwrap();
        Memory::open(&state.root)
            .unwrap()
            .store(&[correction], true)
            .unwrap();
        assert_eq!(
            state
                .export_plan(Some(&id), &path, &signature, export_operation.work.clone())
                .err()
                .unwrap()
                .code,
            "PRONUNCIATION_SNAPSHOT_STALE"
        );
        drop(export_operation);
        drop(operation);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cancel_command_reaches_the_registered_fresh_export_token() {
        let (root, state, path, signature) = state();
        let analysis = state.operation().unwrap();
        let snapshot = analysed(&state, &path);
        drop(analysis);
        let export_operation = state.operation().unwrap();
        let export = state
            .export_plan(
                Some(&snapshot.id),
                &path,
                &signature,
                export_operation.work.clone(),
            )
            .unwrap();
        let app = tauri::test::mock_app();
        app.manage(state.clone());
        pronunciation_cancel(app.state()).unwrap();
        assert_eq!(
            export.work.check().unwrap_err().code,
            "PRONUNCIATION_CANCELLED"
        );
        assert!(!snapshot.work.cancelled.load(Ordering::Relaxed));
        drop(export_operation);
        assert!(state.active.lock().unwrap().is_empty());
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn repeated_complete_analysis_reuses_quota_and_release_recovers() {
        let (root, state, path, signature) = state();
        let snapshot = analysed(&state, &path);
        for _ in 0..5 {
            assert_eq!(analysed(&state, &path).id, snapshot.id);
        }
        assert_eq!(state.plans.lock().unwrap().len(), 1);
        let original = state.plans.lock().unwrap()[&snapshot.id].snapshot.clone();
        let draft = state
            .new_plan(
                &path,
                signature.clone(),
                original.corrections.clone(),
                Work::new(Duration::from_secs(30)),
            )
            .unwrap();
        assert!(!draft.sealed);
        assert!(state
            .export_plan(
                Some(&original.id),
                &path,
                &signature,
                Work::new(Duration::from_secs(30))
            )
            .is_ok());
        {
            let mut plans = state.plans.lock().unwrap();
            let plan = plans[&snapshot.id].clone();
            for i in 0..127 {
                plans.insert(format!("prior-{i}"), plan.clone());
            }
        }
        assert!(Arc::ptr_eq(&original, &analysed(&state, &path)));
        assert_eq!(
            state
                .publish_plan(signature, (*draft).clone(), hash(b"different projection"))
                .err()
                .unwrap()
                .code,
            "PRONUNCIATION_SNAPSHOT_LIMIT"
        );
        let ids: Vec<_> = state.plans.lock().unwrap().keys().cloned().collect();
        state.release(&ids).unwrap();
        assert!(analysed(&state, &path).sealed);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_batch_keeps_successes_around_missing_and_stale_middle_files() {
        let (root, state, path, _) = state();
        let bytes = std::fs::read(&path).unwrap();
        let middle = root.join("middle.xml").to_string_lossy().to_string();
        let last = root.join("last.xml").to_string_lossy().to_string();
        std::fs::write(&last, &bytes).unwrap();
        let app = tauri::test::mock_app();
        app.manage(state.clone());
        let paths = vec![path.clone(), middle.clone(), last.clone()];
        let run = |write, ids, out| {
            tauri::async_runtime::block_on(pronunciation_convert_files(
                app.state(),
                paths.clone(),
                write,
                out,
                None,
                None,
                Some(ExportTarget::Ustx),
                Some(PronunciationProfile::Automatic),
                ids,
            ))
        };
        let result = run(false, None, None).unwrap();
        assert_eq!(
            result.iter().map(|r| r.ok).collect::<Vec<_>>(),
            vec![true, false, true]
        );
        std::fs::write(&middle, &bytes).unwrap();
        let analysed = run(false, None, None).unwrap();
        let ids = analysed
            .iter()
            .map(|r| (r.path.clone(), r.pronunciation_snapshot_id.clone().unwrap()))
            .collect();
        std::fs::write(&middle, b"changed source").unwrap();
        let out = root.join("outputs");
        std::fs::create_dir(&out).unwrap();
        let result = run(true, Some(ids), Some(out.to_string_lossy().into_owned())).unwrap();
        assert_eq!(
            result.iter().map(|r| r.ok).collect::<Vec<_>>(),
            vec![true, false, true]
        );
        assert_eq!(
            result[1].error.as_ref().unwrap().code,
            "PRONUNCIATION_SNAPSHOT_STALE"
        );
        assert!(Path::new(result[0].out.as_ref().unwrap()).is_file());
        assert!(Path::new(result[2].out.as_ref().unwrap()).is_file());
        assert!(!out.join("middle_LYRICS.ustx").exists());
        assert_eq!(
            Memory::open(&state.root)
                .unwrap()
                .references()
                .unwrap()
                .len(),
            2
        );
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_bound_complete_export_retains_real_words_after_the_computation_deadline() {
        use crate::renderer::{
            AudioRenderer, RenderError, RenderLimits, RenderedAudio, RendererCapabilities,
        };
        struct WaitForExpiry(Arc<dyn AudioRenderer>);
        impl AudioRenderer for WaitForExpiry {
            fn capabilities(&self) -> &RendererCapabilities {
                self.0.capabilities()
            }
            fn convert_to_mscz(
                &self,
                input: &Path,
                output: &Path,
                limits: &RenderLimits,
            ) -> Result<Vec<u8>, RenderError> {
                self.0.convert_to_mscz(input, output, limits)
            }
            fn render(
                &self,
                input: &Path,
                output: &Path,
                limits: &RenderLimits,
            ) -> Result<RenderedAudio, RenderError> {
                let work = super::super::current_snapshot().unwrap().work.clone();
                std::thread::sleep(
                    work.deadline
                        .saturating_duration_since(std::time::Instant::now())
                        + Duration::from_millis(2),
                );
                assert_eq!(work.check().unwrap_err().code, "PRONUNCIATION_DEADLINE");
                self.0.render(input, output, limits)
            }
            fn render_part(
                &self,
                input: &Path,
                output: &Path,
                limits: &RenderLimits,
            ) -> Result<RenderedAudio, RenderError> {
                self.0.render_part(input, output, limits)
            }
        }
        let (root, state, path, _) = state();
        let approved = analysed(&state, &path);
        let expected = super::super::observed_projection().unwrap().0;
        assert!(!expected.tracks[0].notes.is_empty());
        let direct = crate::engine::target::serialize_to(ExportTarget::Ustx, &expected).unwrap();
        let result = export_bundle_service(
            state.clone(),
            path.clone(),
            root.join("long-audio.versebundle")
                .to_string_lossy()
                .into_owned(),
            None,
            None,
            None,
            Some(ExportTarget::Ustx),
            Some(PronunciationProfile::Automatic),
            Some(approved.id.clone()),
            tauri::ipc::Channel::new(|_| Ok(())),
            Some(Arc::new(WaitForExpiry(
                crate::bundle::tests::successful_renderer(),
            ))),
        )
        .unwrap();
        let saved = std::fs::read(&result.project_path).unwrap();
        let native: serde_yaml::Value = serde_yaml::from_slice(&saved).unwrap();
        let direct: serde_yaml::Value = serde_yaml::from_slice(&direct).unwrap();
        assert_eq!(native["voice_parts"], direct["voice_parts"]);
        let memory = Memory::open(&state.root).unwrap();
        let refs = memory.references().unwrap();
        assert_eq!(refs.len(), 1);
        let reference = memory.reference(&refs[0].id).unwrap();
        assert!(!reference.words.is_empty());
        assert_eq!(reference.export_sha256, super::super::hash(&saved));
        assert_eq!(
            reference.source_sha256,
            super::super::hash(&std::fs::read(&path).unwrap())
        );
        assert_eq!(reference.snapshot_id, approved.id);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_exports_retain_references_refuse_divergence_and_guard_listening() {
        let (root, mut state, path, _) = state();
        Arc::get_mut(&mut state).unwrap().test_renderer =
            Some(crate::bundle::tests::successful_renderer());
        let app = tauri::test::mock_app();
        app.manage(state.clone());
        let result = tauri::async_runtime::block_on(pronunciation_convert_files(
            app.state(),
            vec![path.clone()],
            false,
            None,
            None,
            None,
            Some(ExportTarget::Ustx),
            Some(PronunciationProfile::Automatic),
            None,
        ))
        .unwrap()
        .remove(0);
        let id = result.pronunciation_snapshot_id.unwrap();
        let direct = root.join("direct.ustx");
        let vocal = |target: PathBuf| {
            tauri::async_runtime::block_on(pronunciation_export_svp(
                app.state(),
                path.clone(),
                target.to_string_lossy().into_owned(),
                None,
                None,
                Some(ExportTarget::Ustx),
                Some(PronunciationProfile::Automatic),
                Some(id.clone()),
            ))
        };
        vocal(direct.clone()).unwrap();
        let refs = Memory::open(&state.root).unwrap().references().unwrap();
        assert_eq!(refs.len(), 1);
        let bundle = tauri::async_runtime::block_on(pronunciation_export_bundle(
            app.state(),
            path.clone(),
            root.join("bundle.versebundle")
                .to_string_lossy()
                .into_owned(),
            None,
            None,
            None,
            Some(ExportTarget::Ustx),
            Some(PronunciationProfile::Automatic),
            Some(id.clone()),
            tauri::ipc::Channel::new(|_| Ok(())),
        ))
        .unwrap();
        let memory = Memory::open(&state.root).unwrap();
        let references = memory.references().unwrap();
        assert_eq!(references.len(), 2);
        for reference in references {
            memory.reference(&reference.id).unwrap();
        }
        assert!(Path::new(&bundle.project_path).is_file());
        let corrected = root.join("corrected.ustx");
        let mut doc: serde_yaml::Value =
            serde_yaml::from_slice(&std::fs::read(&direct).unwrap()).unwrap();
        doc["voice_parts"][0]["notes"][0]["phoneme_overrides"] =
            serde_yaml::from_str("- index: 0\n  phoneme: bank-alias").unwrap();
        std::fs::write(&corrected, serde_yaml::to_string(&doc).unwrap()).unwrap();
        let review = tauri::async_runtime::block_on(pronunciation_compare(
            app.state(),
            refs[0].id.clone(),
            corrected.to_string_lossy().into_owned(),
        ))
        .unwrap();
        let selection = |listened| {
            vec![Confirmation {
                correction_id: review.proposals[0].id.clone(),
                scope: Scope::OccurrenceOnly,
                listened,
            }]
        };
        let confirm = |listened| {
            tauri::async_runtime::block_on(pronunciation_confirm(
                app.state(),
                refs[0].id.clone(),
                corrected.to_string_lossy().into_owned(),
                selection(listened),
            ))
        };
        assert_eq!(
            confirm(false).unwrap_err().code,
            "PRONUNCIATION_CONFIRMATION_REQUIRED"
        );
        assert!(Memory::open(&state.root)
            .unwrap()
            .snapshot()
            .unwrap()
            .is_empty());
        confirm(true).unwrap();
        assert_eq!(
            Memory::open(&state.root).unwrap().snapshot().unwrap().len(),
            1
        );
        // Reanalyse after confirmation, then corrupt only the approved projection proof.
        let fresh = analysed(&state, &path);
        {
            let mut plans = state.plans.lock().unwrap();
            Arc::make_mut(&mut plans.get_mut(&fresh.id).unwrap().snapshot)
                .expected_projection_sha256 = Some(hash(b"divergent projection"));
        }
        let refused = root.join("refused.ustx");
        let error = tauri::async_runtime::block_on(pronunciation_export_svp(
            app.state(),
            path.clone(),
            refused.to_string_lossy().into_owned(),
            None,
            None,
            Some(ExportTarget::Ustx),
            Some(PronunciationProfile::Automatic),
            Some(fresh.id.clone()),
        ))
        .unwrap_err();
        assert_eq!(error.code, "PRONUNCIATION_SNAPSHOT_STALE");
        assert!(!refused.exists());
        let refused_bundle = root.join("refused.versebundle");
        let error = tauri::async_runtime::block_on(pronunciation_export_bundle(
            app.state(),
            path.clone(),
            refused_bundle.to_string_lossy().into_owned(),
            None,
            None,
            None,
            Some(ExportTarget::Ustx),
            Some(PronunciationProfile::Automatic),
            Some(fresh.id.clone()),
            tauri::ipc::Channel::new(|_| Ok(())),
        ))
        .unwrap_err();
        assert_eq!(error.code, "PRONUNCIATION_SNAPSHOT_STALE");
        assert!(!refused_bundle.exists());
        drop(memory);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }
}
