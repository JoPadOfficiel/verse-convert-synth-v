//! Offline comparison artifacts; never renders audio or overwrites a project.
//! Usage: cargo run --example french_profile_export -- OUTPUT_DIR SCORE...
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use verse_lib::engine::convert::convert_midi_with_profile;
use verse_lib::engine::musescore;
use verse_lib::engine::target::{serialize_to, ExportTarget, PronunciationProfile};

#[derive(Default)]
struct CreatedArtifacts {
    paths: Vec<PathBuf>,
    committed: bool,
}

impl CreatedArtifacts {
    fn create(
        &mut self,
        path: &Path,
        write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        // Register immediately: even a short write or a later conversion failure
        // must remove our partial files so a retry can use create_new again.
        self.paths.push(path.to_owned());
        write(&mut file)
    }

    fn write(&mut self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.create(path, |file| file.write_all(bytes))
    }
}

impl Drop for CreatedArtifacts {
    fn drop(&mut self) {
        if !self.committed {
            for path in self.paths.iter().rev() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

fn run(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(
        args.next()
            .ok_or("expected output directory, then MuseScore paths")?,
    );
    let scores: Vec<_> = args.map(PathBuf::from).collect();
    if scores.is_empty() {
        return Err("expected at least one MuseScore path".into());
    }
    std::fs::create_dir_all(&output)?;
    let mut artifacts = CreatedArtifacts::default();
    let mut messages = Vec::new();
    for path in scores {
        let data = std::fs::read(&path)?;
        let midi = musescore::parse(&data)?;
        let stem = path
            .file_stem()
            .ok_or("missing filename")?
            .to_string_lossy();
        for (name, profile) in [
            ("default", PronunciationProfile::Default),
            (
                "french-millefeuille",
                PronunciationProfile::FrenchMillefeuille,
            ),
        ] {
            let outcome =
                convert_midi_with_profile(&midi, "english", None, ExportTarget::Ustx, profile);
            if !outcome.ok {
                return Err(outcome.msg.unwrap_or_default().into());
            }
            let project = outcome.svp.as_ref().ok_or("missing projection")?;
            let bytes = serialize_to(ExportTarget::Ustx, project).map_err(|e| e.to_string())?;
            let destination = output.join(format!("{stem}.{name}.ustx"));
            artifacts.write(&destination, &bytes)?;
            let diagnostics: Vec<_> = outcome.tracks.iter().map(|track| serde_json::json!({"track": track.track, "sourceId": track.source_id, "placed": track.placed, "warnings": track.warnings})).collect();
            artifacts.write(
                &output.join(format!("{stem}.{name}.diagnostics.json")),
                &serde_json::to_vec_pretty(&diagnostics)?,
            )?;
            messages.push(format!(
                "{}: {} notes, {} vocal lanes",
                destination.display(),
                project.tracks.iter().map(|t| t.notes.len()).sum::<usize>(),
                project.tracks.len()
            ));
        }
    }
    artifacts.committed = true;
    for message in messages {
        println!("{message}");
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(std::env::args_os().skip(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temporary() -> PathBuf {
        std::env::temp_dir().join(format!(
            "verse-french-example-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn output_directory_without_scores_is_rejected_before_writing() {
        let root = temporary();
        assert!(run([root.as_os_str().to_owned()].into_iter()).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn partial_write_is_removed_and_can_be_retried() {
        let root = temporary();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("partial.ustx");
        {
            let mut files = CreatedArtifacts::default();
            let failure = files.create(&path, |file| {
                file.write_all(b"partial")?;
                Err(std::io::Error::other("injected write failure"))
            });
            assert!(failure.is_err());
        }
        assert!(!path.exists());
        let mut retry = CreatedArtifacts::default();
        retry.write(&path, b"complete").unwrap();
        retry.committed = true;
        assert_eq!(std::fs::read(&path).unwrap(), b"complete");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn later_failure_rolls_back_created_files_and_never_replaces_existing_outputs() {
        let root = temporary();
        std::fs::create_dir(&root).unwrap();
        let source = root.join("song.mscx");
        std::fs::write(&source, br#"<museScore version="4.0"><Score><Division>480</Division><Part><Staff id="1"/><trackName>Voice</trackName></Part><Staff id="1"><Measure><voice><Chord><durationType>quarter</durationType><Lyrics><text>un</text></Lyrics><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff></Score></museScore>"#).unwrap();
        let existing = root.join("song.default.diagnostics.json");
        std::fs::write(&existing, b"existing output").unwrap();
        assert!(
            run([root.as_os_str().to_owned(), source.as_os_str().to_owned()].into_iter()).is_err()
        );
        assert_eq!(std::fs::read(&existing).unwrap(), b"existing output");
        assert!(!root.join("song.default.ustx").exists());
        std::fs::remove_file(&existing).unwrap();
        run([root.as_os_str().to_owned(), source.as_os_str().to_owned()].into_iter()).unwrap();
        assert!(root.join("song.french-millefeuille.ustx").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
