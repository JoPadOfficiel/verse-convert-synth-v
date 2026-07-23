pub mod engine;

use engine::convert::convert_auto_with;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Serialize)]
pub struct TrackInfo {
    pub id: usize,
    pub track: String,
    pub notes: usize,
    pub role: String,
    pub placed: usize,
}

#[derive(Serialize)]
pub struct FileResult {
    pub path: String,
    pub name: String,
    pub ok: bool,
    pub msg: Option<String>,
    #[serde(rename = "nTracks")]
    pub n_tracks: usize,
    pub placed: usize,
    pub tracks: Vec<TrackInfo>,
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

const MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024; // 128 MB, far beyond any real file
const SUPPORTED_EXT: [&str; 8] = ["kar", "mid", "midi", "mxl", "xml", "musicxml", "mscz", "mscx"];

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
    let err = |name: String, msg: String| FileResult {
        path: path.into(),
        name,
        ok: false,
        msg: Some(msg),
        n_tracks: 0,
        placed: 0,
        tracks: vec![],
        out: None,
    };
    // Backend-side validations (we do not trust the frontend filter)
    let ext_ok = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false);
    if !ext_ok {
        return err(name, "unsupported file type".into());
    }
    match std::fs::metadata(path) {
        Ok(md) if md.len() > MAX_INPUT_BYTES => {
            return err(name, "abnormally large file (rejected for safety)".into());
        }
        _ => {}
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            return FileResult {
                path: path.into(),
                name,
                ok: false,
                msg: Some(format!("cannot read file ({})", e)),
                n_tracks: 0,
                placed: 0,
                tracks: vec![],
                out: None,
            }
        }
    };
    let r = convert_auto_with(&data, language, overrides);
    let tracks = r
        .tracks
        .iter()
        .map(|t| TrackInfo {
            id: t.id,
            track: t.track.clone(),
            notes: t.notes,
            role: t.role.clone(),
            placed: t.placed,
        })
        .collect();
    let mut out = None;
    if write && r.ok {
        if let Some(svp) = &r.svp {
            let out_path = svp_out_path(path, out_dir);
            if let Ok(json) = serde_json::to_string(svp) {
                if std::fs::write(&out_path, json).is_ok() {
                    out = Some(out_path);
                }
            }
        }
    }
    FileResult {
        path: path.into(),
        name,
        ok: r.ok,
        msg: r.msg,
        n_tracks: r.n_tracks,
        placed: r.placed,
        tracks,
        out,
    }
}

/// Analyzes and/or converts a list of files.
/// `write=false` -> analysis only (preview); `write=true` -> writes the .svp files.
/// Convert a single file and write the .svp to an exact target path chosen by
/// the user (via a Save dialog on the frontend). Returns the written path.
#[tauri::command]
fn export_svp(
    path: String,
    target: String,
    language: Option<String>,
    overrides: Option<HashMap<String, bool>>,
) -> Result<String, String> {
    let ext_ok = Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false);
    if !ext_ok {
        return Err("unsupported file type".into());
    }
    if let Ok(md) = std::fs::metadata(&path) {
        if md.len() > MAX_INPUT_BYTES {
            return Err("abnormally large file (rejected for safety)".into());
        }
    }
    let data = std::fs::read(&path).map_err(|e| format!("cannot read file ({})", e))?;
    let ov: HashMap<usize, bool> = overrides
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| k.parse::<usize>().ok().map(|k| (k, v)))
        .collect();
    let lang = language.as_deref().unwrap_or("english");
    let r = convert_auto_with(&data, lang, Some(&ov));
    if !r.ok {
        return Err(r.msg.unwrap_or_else(|| "conversion failed".into()));
    }
    let svp = r.svp.ok_or_else(|| "no output produced".to_string())?;
    let json = serde_json::to_string(&svp).map_err(|e| e.to_string())?;
    std::fs::write(&target, json).map_err(|e| format!("cannot write file ({})", e))?;
    Ok(target)
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
    // out_dir must be an existing directory, otherwise we write next to the sources
    let out_dir = out_dir.filter(|d| Path::new(d).is_dir());
    // Sings/Muted overrides: path -> (track id -> sings?)
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
        .map(|p| process_one(p, write, out_dir.as_deref(), lang, overrides.get(p.as_str())))
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![convert_files, export_svp])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
