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
    #[cfg(test)]
    test_renderer: Option<Arc<dyn crate::renderer::AudioRenderer>>,
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
            #[cfg(test)]
            test_renderer: None,
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
        Ok(())
    }
    fn publish_plan(
        &self,
        signature: String,
        mut snapshot: Snapshot,
        digest: String,
    ) -> Result<Arc<Snapshot>, Error> {
        snapshot.expected_projection_sha256 = Some(digest);
        snapshot.sealed = true;
        snapshot.id = hash(&serde_json::to_vec(&(
            &snapshot.id,
            &snapshot.expected_projection_sha256,
        ))?);
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
                let mut result = crate::process_one(
                    &path,
                    write,
                    out_dir.as_deref(),
                    language.as_deref().unwrap_or("english"),
                    parsed.as_ref(),
                    export_target.unwrap_or_default(),
                    pronunciation_profile.unwrap_or_default(),
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
