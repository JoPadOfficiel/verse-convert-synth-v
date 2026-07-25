//! Deterministic, machine-readable corpus audit for openly licensed score
//! collections. This binary never downloads data; the wrapper script pins and
//! verifies the corpus before invoking it.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use verse_lib::engine::{convert::convert_midi, midi::Midi, musescore, musicxml};
use verse_lib::renderer::{
    AudioRenderer, MuseScoreConfig, MuseScoreRenderer, RenderLimits, DEFAULT_MAX_WAV_BYTES,
};

const REPORT_SCHEMA_VERSION: u32 = 2;
const DEFAULT_RENDER_SAMPLE_SIZE: usize = 3;
const DEFAULT_MAX_FILES: usize = 5_000;
const MAX_DIRECTORY_DEPTH: usize = 32;
const RENDER_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const OPEN_SCORE_BASELINE_NAME: &str = "OpenScore Lieder";
const OPEN_SCORE_BASELINE_REPOSITORY: &str = "https://github.com/OpenScore/Lieder";
const OPEN_SCORE_BASELINE_COMMIT: &str = "6b2dc542ce2e8aa4b78c8ee62103b210efc07015";
const OPEN_SCORE_BASELINE_LICENSE: &str = "CC0-1.0";

#[derive(Debug)]
struct Options {
    input: PathBuf,
    report: PathBuf,
    corpus_name: String,
    repository: String,
    commit: String,
    license: String,
    full_parse: bool,
    render_sample_size: usize,
    renderer: Option<PathBuf>,
    max_files: usize,
    extensions: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditReport {
    schema_version: u32,
    generated_unix_seconds: u64,
    corpus: CorpusIdentity,
    configuration: AuditConfiguration,
    baseline: Option<BaselineAudit>,
    summary: AuditSummary,
    files: Vec<FileAudit>,
    renders: Vec<RenderAudit>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CorpusIdentity {
    name: String,
    repository: String,
    commit: String,
    license: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditConfiguration {
    input_root: String,
    full_parse: bool,
    render_sample_size: usize,
    deterministic_sample_algorithm: &'static str,
    max_files: usize,
    extensions: Vec<String>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditSummary {
    discovered_files: usize,
    parsed_files: usize,
    projected_files: usize,
    typed_errors: usize,
    ineligible_evidence_files: usize,
    unexpected_errors: usize,
    source_parts: usize,
    source_voices: usize,
    source_notes: usize,
    source_lyrics: usize,
    projected_lyrics: usize,
    rendered_scores: usize,
    rendered_part_stems: usize,
    render_errors: usize,
    evidence_invariant_failures: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct BaselineCounts {
    discovered_files: usize,
    parsed_files: usize,
    projected_files: usize,
    typed_errors: usize,
    ineligible_evidence_files: usize,
    unexpected_errors: usize,
    evidence_invariant_failures: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BaselineAudit {
    name: &'static str,
    passed: bool,
    expected: BaselineCounts,
    actual: BaselineCounts,
    mismatches: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileAudit {
    relative_path: String,
    format: String,
    bytes: u64,
    status: &'static str,
    source_parts: usize,
    source_voices: usize,
    projection_lanes: usize,
    source_notes: usize,
    source_lyrics: usize,
    projected_lyrics: usize,
    vocal_tracks: usize,
    diagnostic_codes: Vec<String>,
    eligibility_code: Option<&'static str>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RenderAudit {
    relative_path: String,
    sample_rank_sha256: String,
    status: &'static str,
    expected_parts: usize,
    extracted_parts: usize,
    rendered_part_stems: usize,
    full_score_bytes: Option<u64>,
    renderer_provider: Option<String>,
    renderer_version: Option<String>,
    error: Option<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(has_failures) => {
            if has_failures {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("corpus audit failed before a report could be completed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, String> {
    let options = parse_options(env::args_os().skip(1))?;
    let input = fs::canonicalize(&options.input).map_err(|error| {
        format!(
            "cannot resolve input root {}: {error}",
            options.input.display()
        )
    })?;
    if !input.is_dir() {
        return Err(format!(
            "input root is not a directory: {}",
            input.display()
        ));
    }

    let files = discover_score_files(&input, options.max_files, &options.extensions)?;
    if files.is_empty() {
        return Err(format!(
            "no supported score files were found under {}",
            input.display()
        ));
    }
    validate_report_destination(&input, &options.report, &files)?;
    eprintln!("auditing {} score files", files.len());

    let mut summary = AuditSummary {
        discovered_files: files.len(),
        ..AuditSummary::default()
    };
    let mut audits = Vec::with_capacity(files.len());
    let mut render_candidates = Vec::new();

    for path in &files {
        let relative = relative_path(&input, path)?;
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                summary.typed_errors += 1;
                summary.unexpected_errors += 1;
                audits.push(FileAudit::unexpected_error(
                    relative,
                    extension(path),
                    0,
                    format!("read failed: {error}"),
                ));
                continue;
            }
        };
        let byte_len = bytes.len() as u64;
        match parse_score(path, &bytes) {
            Ok(midi) => {
                summary.parsed_files += 1;
                let source_parts = midi.topology.part_count();
                let source_voices = midi.topology.voice_count();
                let projection_lanes = midi.topology.projection_lane_count();
                let outcome = convert_midi(&midi, "english");
                let source_notes = outcome.tracks.iter().map(|track| track.notes).sum();
                let source_lyrics = outcome
                    .tracks
                    .iter()
                    .map(|track| track.lyric_status.source_text_count)
                    .sum();
                let vocal_tracks = outcome
                    .svp
                    .as_ref()
                    .map_or(0, |project| project.tracks.len());
                let diagnostic_codes = outcome
                    .tracks
                    .iter()
                    .flat_map(|track| track.warnings.iter())
                    .map(|warning| warning.code.clone())
                    .collect::<Vec<_>>();
                let topology_preserved = outcome.topology == midi.topology;

                summary.source_parts += source_parts;
                summary.source_voices += source_voices;
                summary.source_notes += source_notes;
                summary.source_lyrics += source_lyrics;
                summary.projected_lyrics += outcome.placed;

                if outcome.ok && topology_preserved {
                    summary.projected_files += 1;
                    render_candidates.push((path.clone(), relative.clone(), source_parts));
                    audits.push(FileAudit {
                        relative_path: relative,
                        format: extension(path),
                        bytes: byte_len,
                        status: "projected",
                        source_parts,
                        source_voices,
                        projection_lanes,
                        source_notes,
                        source_lyrics,
                        projected_lyrics: outcome.placed,
                        vocal_tracks,
                        diagnostic_codes,
                        eligibility_code: None,
                        error: None,
                    });
                } else {
                    summary.typed_errors += 1;
                    if !topology_preserved {
                        summary.evidence_invariant_failures += 1;
                        summary.unexpected_errors += 1;
                        audits.push(FileAudit {
                            relative_path: relative,
                            format: extension(path),
                            bytes: byte_len,
                            status: "unexpectedError",
                            source_parts,
                            source_voices,
                            projection_lanes,
                            source_notes,
                            source_lyrics,
                            projected_lyrics: outcome.placed,
                            vocal_tracks,
                            diagnostic_codes,
                            eligibility_code: None,
                            error: Some(outcome.msg.unwrap_or_else(|| {
                                "source topology changed during projection".into()
                            })),
                        });
                        continue;
                    }
                    let error = outcome
                        .msg
                        .unwrap_or_else(|| "projection failed without a diagnostic".into());
                    let eligibility_code = known_ineligibility(&error);
                    if eligibility_code.is_some() {
                        summary.ineligible_evidence_files += 1;
                    } else {
                        summary.unexpected_errors += 1;
                    }
                    audits.push(FileAudit {
                        relative_path: relative,
                        format: extension(path),
                        bytes: byte_len,
                        status: if eligibility_code.is_some() {
                            "ineligibleEvidenceError"
                        } else {
                            "unexpectedError"
                        },
                        source_parts,
                        source_voices,
                        projection_lanes,
                        source_notes,
                        source_lyrics,
                        projected_lyrics: outcome.placed,
                        vocal_tracks,
                        diagnostic_codes,
                        eligibility_code,
                        error: Some(error),
                    });
                }
            }
            Err(error) => {
                summary.typed_errors += 1;
                let eligibility_code = known_ineligibility(&error);
                if eligibility_code.is_some() {
                    summary.ineligible_evidence_files += 1;
                } else {
                    summary.unexpected_errors += 1;
                }
                audits.push(FileAudit::classified_error(
                    relative,
                    extension(path),
                    byte_len,
                    error,
                    eligibility_code,
                ));
            }
        }
    }

    let renders = if options.render_sample_size > 0 {
        render_deterministic_sample(
            &input,
            &render_candidates,
            options.render_sample_size,
            options.renderer.as_deref(),
            &mut summary,
        )
    } else {
        Vec::new()
    };
    let baseline = pinned_baseline(&options, &summary);

    let report = AuditReport {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        corpus: CorpusIdentity {
            name: options.corpus_name,
            repository: options.repository,
            commit: options.commit,
            license: options.license,
        },
        configuration: AuditConfiguration {
            input_root: input.to_string_lossy().into_owned(),
            full_parse: options.full_parse,
            render_sample_size: options.render_sample_size,
            deterministic_sample_algorithm: "sha256(relative-path), ascending",
            max_files: options.max_files,
            extensions: options.extensions,
        },
        baseline,
        summary,
        files: audits,
        renders,
    };
    write_report(&options.report, &report)?;

    eprintln!(
        "report written to {} ({} parsed, {} projected, {} evidence-ineligible, {} unexpected, {} render errors)",
        options.report.display(),
        report.summary.parsed_files,
        report.summary.projected_files,
        report.summary.ineligible_evidence_files,
        report.summary.unexpected_errors,
        report.summary.render_errors
    );

    Ok(report.summary.unexpected_errors > 0
        || report.summary.render_errors > 0
        || report.summary.evidence_invariant_failures > 0
        || report
            .baseline
            .as_ref()
            .is_some_and(|baseline| !baseline.passed))
}

fn pinned_baseline(options: &Options, summary: &AuditSummary) -> Option<BaselineAudit> {
    if options.corpus_name != OPEN_SCORE_BASELINE_NAME
        || options.repository != OPEN_SCORE_BASELINE_REPOSITORY
        || options.commit != OPEN_SCORE_BASELINE_COMMIT
        || options.license != OPEN_SCORE_BASELINE_LICENSE
    {
        return None;
    }

    let expected = BaselineCounts {
        discovered_files: 1_352,
        parsed_files: 1_343,
        projected_files: 1_277,
        typed_errors: 75,
        ineligible_evidence_files: 75,
        unexpected_errors: 0,
        evidence_invariant_failures: 0,
    };
    let actual = BaselineCounts::from(summary);
    let mut mismatches = Vec::new();
    record_baseline_mismatch(
        &mut mismatches,
        "discoveredFiles",
        expected.discovered_files,
        actual.discovered_files,
    );
    record_baseline_mismatch(
        &mut mismatches,
        "parsedFiles",
        expected.parsed_files,
        actual.parsed_files,
    );
    record_baseline_mismatch(
        &mut mismatches,
        "projectedFiles",
        expected.projected_files,
        actual.projected_files,
    );
    record_baseline_mismatch(
        &mut mismatches,
        "typedErrors",
        expected.typed_errors,
        actual.typed_errors,
    );
    record_baseline_mismatch(
        &mut mismatches,
        "ineligibleEvidenceFiles",
        expected.ineligible_evidence_files,
        actual.ineligible_evidence_files,
    );
    record_baseline_mismatch(
        &mut mismatches,
        "unexpectedErrors",
        expected.unexpected_errors,
        actual.unexpected_errors,
    );
    record_baseline_mismatch(
        &mut mismatches,
        "evidenceInvariantFailures",
        expected.evidence_invariant_failures,
        actual.evidence_invariant_failures,
    );
    Some(BaselineAudit {
        name: "openscore-lieder-6b2dc542ce2e",
        passed: mismatches.is_empty(),
        expected,
        actual,
        mismatches,
    })
}

impl From<&AuditSummary> for BaselineCounts {
    fn from(summary: &AuditSummary) -> Self {
        Self {
            discovered_files: summary.discovered_files,
            parsed_files: summary.parsed_files,
            projected_files: summary.projected_files,
            typed_errors: summary.typed_errors,
            ineligible_evidence_files: summary.ineligible_evidence_files,
            unexpected_errors: summary.unexpected_errors,
            evidence_invariant_failures: summary.evidence_invariant_failures,
        }
    }
}

fn record_baseline_mismatch(
    mismatches: &mut Vec<String>,
    field: &str,
    expected: usize,
    actual: usize,
) {
    if expected != actual {
        mismatches.push(format!("{field}: expected {expected}, observed {actual}"));
    }
}

impl FileAudit {
    fn unexpected_error(relative_path: String, format: String, bytes: u64, error: String) -> Self {
        Self::classified_error(relative_path, format, bytes, error, None)
    }

    fn classified_error(
        relative_path: String,
        format: String,
        bytes: u64,
        error: String,
        eligibility_code: Option<&'static str>,
    ) -> Self {
        Self {
            relative_path,
            format,
            bytes,
            status: if eligibility_code.is_some() {
                "ineligibleEvidenceError"
            } else {
                "unexpectedError"
            },
            source_parts: 0,
            source_voices: 0,
            projection_lanes: 0,
            source_notes: 0,
            source_lyrics: 0,
            projected_lyrics: 0,
            vocal_tracks: 0,
            diagnostic_codes: Vec::new(),
            eligibility_code,
            error: Some(error),
        }
    }
}

/// Errors in this allowlist are not parser crashes or silent losses. They are
/// explicit proof that a source score asks for timing/navigation semantics
/// which the exact SVP projection contract deliberately refuses to guess.
/// Keep the predicates narrow: an unknown error must always fail the corpus
/// audit and therefore cannot be hidden by a broad substring match.
fn known_ineligibility(error: &str) -> Option<&'static str> {
    if error
        .starts_with("MIDI meter cannot be projected safely: time signature change at MIDI tick ")
        && error.contains(" falls inside a ")
        && error.ends_with(" measure; Synthesizer V meter changes require a measure boundary")
    {
        return Some("svp-meter-requires-regular-measure-boundary");
    }
    if error == "repeat start has no matching repeat ending" {
        return Some("unmatched-repeat-start");
    }
    if error.starts_with("nested repeat starting at measure ")
        && error.ends_with(" is not representable safely")
    {
        return Some("nested-repeat-not-representable");
    }
    if error.starts_with("multiple navigation jumps are not representable safely (found ")
        && error.ends_with(')')
    {
        return Some("multiple-navigation-jumps");
    }
    if error.starts_with("D.S. jump at measure ")
        && error.contains(" requires exactly one segno target; found ")
    {
        return Some("ambiguous-segno-target");
    }
    if error.starts_with("al Fine jump at measure ") && error.ends_with(" has no Fine target") {
        return Some("missing-fine-target");
    }
    if error.starts_with("MuseScore time signatures at tick ")
        && error.contains(" disagree about the global measure duration (")
        && error.ends_with(')')
    {
        return Some("conflicting-global-meter-durations");
    }
    None
}

fn parse_options(arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<Options, String> {
    let mut arguments = arguments.peekable();
    let mut input = None;
    let mut report = None;
    let mut corpus_name = "OpenScore Lieder".to_string();
    let mut repository = "https://github.com/OpenScore/Lieder".to_string();
    let mut commit = "unknown".to_string();
    let mut license = "CC0-1.0".to_string();
    let mut full_parse = false;
    let mut render_sample_size = 0;
    let mut renderer = None;
    let mut max_files = DEFAULT_MAX_FILES;
    let mut extensions = Vec::new();

    while let Some(argument) = arguments.next() {
        let argument_text = argument.to_string_lossy();
        match argument_text.as_ref() {
            "--input" => input = Some(next_path(&mut arguments, "--input")?),
            "--report" => report = Some(next_path(&mut arguments, "--report")?),
            "--corpus-name" => {
                corpus_name = next_string(&mut arguments, "--corpus-name")?;
            }
            "--repository" => {
                repository = next_string(&mut arguments, "--repository")?;
            }
            "--commit" => commit = next_string(&mut arguments, "--commit")?,
            "--license" => license = next_string(&mut arguments, "--license")?,
            "--full-parse" => full_parse = true,
            "--render-sample" => {
                render_sample_size = arguments
                    .peek()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.starts_with('-'))
                    .map(str::parse::<usize>)
                    .transpose()
                    .map_err(|_| "--render-sample expects a positive integer".to_string())?
                    .unwrap_or(DEFAULT_RENDER_SAMPLE_SIZE);
                if arguments
                    .peek()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| !value.starts_with('-'))
                {
                    arguments.next();
                }
            }
            "--renderer" => renderer = Some(next_path(&mut arguments, "--renderer")?),
            "--max-files" => {
                max_files = next_string(&mut arguments, "--max-files")?
                    .parse()
                    .map_err(|_| "--max-files expects a positive integer".to_string())?;
            }
            "--extension" => {
                let extension = next_string(&mut arguments, "--extension")?.to_ascii_lowercase();
                if !matches!(
                    extension.as_str(),
                    "mscx" | "mscz" | "mxl" | "xml" | "musicxml"
                ) {
                    return Err(format!("unsupported --extension value: {extension}"));
                }
                if !extensions.contains(&extension) {
                    extensions.push(extension);
                }
            }
            "--help" | "-h" => {
                return Err(
                    "usage: corpus-audit --input PATH --report PATH --full-parse \
                     [--render-sample [N]] [--renderer PATH] [--commit SHA]"
                        .into(),
                );
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if !full_parse && render_sample_size == 0 {
        return Err("select --full-parse and/or --render-sample".into());
    }
    if max_files == 0 || render_sample_size > max_files {
        return Err("file and sample limits must be positive and internally consistent".into());
    }
    if extensions.is_empty() {
        extensions = vec![
            "mscx".into(),
            "mscz".into(),
            "mxl".into(),
            "xml".into(),
            "musicxml".into(),
        ];
    }
    extensions.sort();
    Ok(Options {
        input: input.ok_or_else(|| "--input is required".to_string())?,
        report: report.ok_or_else(|| "--report is required".to_string())?,
        corpus_name,
        repository,
        commit,
        license,
        full_parse,
        render_sample_size,
        renderer,
        max_files,
        extensions,
    })
}

fn next_path(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    flag: &str,
) -> Result<PathBuf, String> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn next_string(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    flag: &str,
) -> Result<String, String> {
    arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| format!("{flag} requires a UTF-8 value"))
}

fn discover_score_files(
    root: &Path,
    max_files: usize,
    extensions: &[String],
) -> Result<Vec<PathBuf>, String> {
    let mut directories = vec![(root.to_path_buf(), 0usize)];
    let mut files = Vec::new();
    while let Some((directory, depth)) = directories.pop() {
        if depth > MAX_DIRECTORY_DEPTH {
            return Err(format!(
                "directory nesting exceeds {MAX_DIRECTORY_DEPTH} below {}",
                directory.display()
            ));
        }
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry
                .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                directories.push((entry.path(), depth + 1));
            } else if file_type.is_file()
                && supported_score_path(&entry.path())
                && extensions.contains(&extension(&entry.path()))
            {
                files.push(entry.path());
                if files.len() > max_files {
                    return Err(format!(
                        "corpus exceeds the configured {max_files}-file limit"
                    ));
                }
            }
        }
    }
    files.sort_by(|left, right| {
        left.to_string_lossy()
            .as_bytes()
            .cmp(right.to_string_lossy().as_bytes())
    });
    Ok(files)
}

fn supported_score_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "mscx" | "mscz" | "mxl" | "xml" | "musicxml"
            )
        })
}

fn parse_score(path: &Path, bytes: &[u8]) -> Result<Midi, String> {
    match extension(path).as_str() {
        "mscx" | "mscz" => musescore::parse(bytes),
        "mxl" | "xml" | "musicxml" => musicxml::parse(bytes),
        other => Err(format!("unsupported score extension: {other}")),
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn relative_path(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| format!("{} escaped corpus root {}", path.display(), root.display()))
}

fn validate_report_destination(
    input_root: &Path,
    report: &Path,
    corpus_files: &[PathBuf],
) -> Result<(), String> {
    let resolved_input_root = fs::canonicalize(input_root).map_err(|error| {
        format!(
            "cannot resolve corpus root {} while validating report destination: {error}",
            input_root.display()
        )
    })?;
    let resolved_report = resolve_future_path(report)?;
    if resolved_report.starts_with(&resolved_input_root) {
        return Err(format!(
            "report destination must remain outside the corpus root: {}",
            report.display()
        ));
    }
    if report.exists() {
        for corpus_file in corpus_files {
            if same_file(report, corpus_file)? {
                return Err(format!(
                    "report destination aliases corpus file {}",
                    corpus_file.display()
                ));
            }
        }
    }
    Ok(())
}

fn resolve_future_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| format!("cannot resolve current directory: {error}"))?
            .join(path)
    };
    let mut existing = absolute.as_path();
    let mut missing_components = Vec::new();
    while !existing.exists() {
        let component = existing.file_name().ok_or_else(|| {
            format!(
                "report destination has no resolvable ancestor: {}",
                path.display()
            )
        })?;
        missing_components.push(component.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            format!(
                "report destination has no resolvable ancestor: {}",
                path.display()
            )
        })?;
    }
    let mut resolved = fs::canonicalize(existing)
        .map_err(|error| format!("cannot resolve report destination ancestor: {error}"))?;
    for component in missing_components.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn same_file(left: &Path, right: &Path) -> Result<bool, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left_metadata = fs::metadata(left)
            .map_err(|error| format!("cannot inspect report destination: {error}"))?;
        let right_metadata = fs::metadata(right)
            .map_err(|error| format!("cannot inspect corpus file {}: {error}", right.display()))?;
        Ok(left_metadata.dev() == right_metadata.dev()
            && left_metadata.ino() == right_metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let left = fs::canonicalize(left)
            .map_err(|error| format!("cannot resolve report destination: {error}"))?;
        let right = fs::canonicalize(right)
            .map_err(|error| format!("cannot resolve corpus file: {error}"))?;
        Ok(left == right)
    }
}

fn render_deterministic_sample(
    input_root: &Path,
    candidates: &[(PathBuf, String, usize)],
    sample_size: usize,
    configured_renderer: Option<&Path>,
    summary: &mut AuditSummary,
) -> Vec<RenderAudit> {
    let mut ranked = candidates
        .iter()
        .map(|(path, relative, expected_parts)| {
            let digest = hex_digest(relative.as_bytes());
            (digest, path, relative, *expected_parts)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then(left.2.cmp(right.2)));
    ranked.truncate(sample_size.min(ranked.len()));

    let renderer_config = MuseScoreConfig {
        executable: configured_renderer.map(Path::to_path_buf),
        timeout: RENDER_TIMEOUT,
        max_wav_bytes: DEFAULT_MAX_WAV_BYTES,
    };
    let renderer = match MuseScoreRenderer::discover(&renderer_config) {
        Ok(renderer) => renderer,
        Err(error) => {
            summary.render_errors += ranked.len();
            return ranked
                .into_iter()
                .map(|(rank, _, relative, expected_parts)| RenderAudit {
                    relative_path: relative.clone(),
                    sample_rank_sha256: rank,
                    status: "renderError",
                    expected_parts,
                    extracted_parts: 0,
                    rendered_part_stems: 0,
                    full_score_bytes: None,
                    renderer_provider: None,
                    renderer_version: None,
                    error: Some(format!("renderer probe failed: {error}")),
                })
                .collect();
        }
    };
    let identity = renderer.capabilities().identity.clone();
    let limits = RenderLimits {
        timeout: RENDER_TIMEOUT,
        max_output_bytes: DEFAULT_MAX_WAV_BYTES,
    };

    ranked
        .into_iter()
        .map(|(rank, path, relative, expected_parts)| {
            eprintln!("rendering deterministic sample {relative}");
            let work = input_root
                .parent()
                .unwrap_or(input_root)
                .join(format!(".verse-corpus-render-{}", std::process::id()))
                .join(&rank[..16]);
            let result = render_one_sample(&renderer, path, &work, expected_parts, &limits);
            let _ = fs::remove_dir_all(&work);
            match result {
                Ok((extracted_parts, rendered_part_stems, full_score_bytes)) => {
                    summary.rendered_scores += 1;
                    summary.rendered_part_stems += rendered_part_stems;
                    RenderAudit {
                        relative_path: relative.clone(),
                        sample_rank_sha256: rank,
                        status: "rendered",
                        expected_parts,
                        extracted_parts,
                        rendered_part_stems,
                        full_score_bytes: Some(full_score_bytes),
                        renderer_provider: Some(identity.provider.clone()),
                        renderer_version: Some(identity.version.clone()),
                        error: None,
                    }
                }
                Err(error) => {
                    summary.render_errors += 1;
                    RenderAudit {
                        relative_path: relative.clone(),
                        sample_rank_sha256: rank,
                        status: "renderError",
                        expected_parts,
                        extracted_parts: 0,
                        rendered_part_stems: 0,
                        full_score_bytes: None,
                        renderer_provider: Some(identity.provider.clone()),
                        renderer_version: Some(identity.version.clone()),
                        error: Some(error),
                    }
                }
            }
        })
        .collect()
}

fn render_one_sample(
    renderer: &MuseScoreRenderer,
    source: &Path,
    work: &Path,
    expected_parts: usize,
    limits: &RenderLimits,
) -> Result<(usize, usize, u64), String> {
    let started = Instant::now();
    fs::create_dir_all(work)
        .map_err(|error| format!("cannot create render work directory: {error}"))?;
    let full_output = work.join("full-score.wav");
    let full = renderer
        .render(
            source,
            &full_output,
            &remaining_render_limits(started, limits)?,
        )
        .map_err(|error| format!("full-score render failed: {error}"))?;
    let parts = renderer
        .extract_score_parts(source, &remaining_render_limits(started, limits)?)
        .map_err(|error| format!("Part extraction failed: {error}"))?;
    if parts.len() != expected_parts {
        return Err(format!(
            "Part coverage mismatch: parser expected {expected_parts}, renderer extracted {}",
            parts.len()
        ));
    }

    for part in &parts {
        let part_source = work.join(format!("part-{:04}.mscz", part.ordinal));
        let part_output = work.join(format!("part-{:04}.wav", part.ordinal));
        fs::write(&part_source, &part.mscz)
            .map_err(|error| format!("cannot stage Part {}: {error}", part.ordinal))?;
        renderer
            .render(
                &part_source,
                &part_output,
                &remaining_render_limits(started, limits)?,
            )
            .map_err(|error| format!("Part {} render failed: {error}", part.ordinal))?;
    }
    Ok((parts.len(), parts.len(), full.wav.bytes))
}

fn remaining_render_limits(
    started: Instant,
    limits: &RenderLimits,
) -> Result<RenderLimits, String> {
    remaining_render_limits_after(started.elapsed(), limits)
}

fn remaining_render_limits_after(
    elapsed: Duration,
    limits: &RenderLimits,
) -> Result<RenderLimits, String> {
    let remaining = limits
        .timeout
        .checked_sub(elapsed)
        .filter(|duration| *duration > Duration::ZERO)
        .ok_or_else(|| {
            format!(
                "score render exceeded its aggregate {} ms deadline",
                limits.timeout.as_millis()
            )
        })?;
    Ok(RenderLimits {
        timeout: remaining,
        max_output_bytes: limits.max_output_bytes,
    })
}

fn write_report(path: &Path, report: &AuditReport) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("report path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create report directory {}: {error}",
            parent.display()
        )
    })?;
    let encoded = serde_json::to_vec_pretty(report)
        .map_err(|error| format!("cannot serialize corpus report: {error}"))?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&temporary, encoded)
        .map_err(|error| format!("cannot write report staging file: {error}"))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("cannot publish report atomically: {error}"))
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn test_options() -> Options {
        Options {
            input: PathBuf::from("scores"),
            report: PathBuf::from("report.json"),
            corpus_name: OPEN_SCORE_BASELINE_NAME.into(),
            repository: OPEN_SCORE_BASELINE_REPOSITORY.into(),
            commit: OPEN_SCORE_BASELINE_COMMIT.into(),
            license: OPEN_SCORE_BASELINE_LICENSE.into(),
            full_parse: true,
            render_sample_size: 0,
            renderer: None,
            max_files: DEFAULT_MAX_FILES,
            extensions: vec!["mscx".into()],
        }
    }

    fn matching_summary() -> AuditSummary {
        AuditSummary {
            discovered_files: 1_352,
            parsed_files: 1_343,
            projected_files: 1_277,
            typed_errors: 75,
            ineligible_evidence_files: 75,
            unexpected_errors: 0,
            evidence_invariant_failures: 0,
            ..AuditSummary::default()
        }
    }

    fn temp_directory(label: &str) -> PathBuf {
        let directory = env::temp_dir().join(format!(
            "verse-corpus-audit-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        directory
    }

    #[test]
    fn sample_flag_uses_bounded_default_without_consuming_next_flag() {
        let options = parse_options(
            [
                "--input",
                "scores",
                "--report",
                "report.json",
                "--full-parse",
                "--render-sample",
                "--commit",
                "abc",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .expect("arguments should parse");
        assert_eq!(options.render_sample_size, DEFAULT_RENDER_SAMPLE_SIZE);
        assert_eq!(options.commit, "abc");
    }

    #[test]
    fn sample_ranking_is_stable() {
        assert_eq!(
            hex_digest(b"a/b/lc1.mscx"),
            "cf0b49824ddc2ff0e75ad627c7b3bd0510e48f8394fe22f5f6946a6d568f465d"
        );
    }

    #[test]
    fn known_evidence_ineligibility_is_narrowly_classified() {
        assert_eq!(
            known_ineligibility(
                "MIDI meter cannot be projected safely: time signature change at MIDI tick 480 \
                 (event:track:1) falls inside a 4/4 measure; Synthesizer V meter changes require \
                 a measure boundary"
            ),
            Some("svp-meter-requires-regular-measure-boundary")
        );
        assert_eq!(
            known_ineligibility(
                "MuseScore tuplet duration cannot be represented exactly at division 480"
            ),
            None
        );
        assert_eq!(
            known_ineligibility(
                "MuseScore time signatures at tick 480 disagree about the global measure \
                 duration (2/4 versus 4/4)"
            ),
            Some("conflicting-global-meter-durations")
        );
        assert_eq!(known_ineligibility("invalid XML: unexpected EOF"), None);
        assert_eq!(
            known_ineligibility("MIDI meter cannot be projected safely: arbitrary failure"),
            None
        );
    }

    #[test]
    fn pinned_openscore_baseline_is_exact_and_reports_drift() {
        let options = test_options();
        let matching = pinned_baseline(&options, &matching_summary()).unwrap();
        assert!(matching.passed);
        assert!(matching.mismatches.is_empty());

        let mut drifted_summary = matching_summary();
        drifted_summary.projected_files -= 1;
        drifted_summary.ineligible_evidence_files += 1;
        let drifted = pinned_baseline(&options, &drifted_summary).unwrap();
        assert!(!drifted.passed);
        assert_eq!(drifted.mismatches.len(), 2);
        assert!(drifted
            .mismatches
            .iter()
            .any(|mismatch| mismatch.starts_with("projectedFiles:")));
    }

    #[test]
    fn unrelated_corpus_identity_has_no_pinned_baseline() {
        let mut options = test_options();
        options.commit = "different".into();
        assert!(pinned_baseline(&options, &matching_summary()).is_none());
    }

    #[test]
    fn report_destination_cannot_enter_or_alias_the_corpus() {
        let root = temp_directory("report-alias");
        let corpus = root.join("scores");
        let reports = root.join("reports");
        fs::create_dir(&corpus).unwrap();
        fs::create_dir(&reports).unwrap();
        let score = corpus.join("song.mscx");
        fs::write(&score, b"score").unwrap();
        let files = vec![score.clone()];

        assert!(validate_report_destination(&corpus, &corpus.join("audit.json"), &files).is_err());
        assert!(validate_report_destination(&corpus, &score, &files).is_err());

        #[cfg(unix)]
        {
            let alias = reports.join("alias.json");
            fs::hard_link(&score, &alias).unwrap();
            assert!(validate_report_destination(&corpus, &alias, &files).is_err());
        }

        assert!(validate_report_destination(&corpus, &reports.join("audit.json"), &files).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn aggregate_render_deadline_only_returns_remaining_budget() {
        let limits = RenderLimits {
            timeout: Duration::from_secs(10),
            max_output_bytes: 123,
        };
        let remaining = remaining_render_limits_after(Duration::from_secs(3), &limits).unwrap();
        assert_eq!(remaining.timeout, Duration::from_secs(7));
        assert_eq!(remaining.max_output_bytes, 123);
        assert!(remaining_render_limits_after(Duration::from_secs(10), &limits).is_err());
        assert!(remaining_render_limits_after(Duration::from_secs(11), &limits).is_err());
    }
}
