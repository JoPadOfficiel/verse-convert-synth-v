//! Explicit DIFFS EN / CMU ARPAbet policy. No runtime G2P or language guessing.
use std::collections::HashMap;
use std::sync::OnceLock;

use super::lexical;
use crate::engine::convert::{Diagnostic, DiagnosticSeverity};
use crate::engine::midi::Syllabic;
use crate::engine::projection::{ProjectedLyric, ProjectedNote};
use crate::engine::syllable::preserve_bracketed_melismas;

pub const PHONEMIZER: &str = "OpenUtau.Core.DiffSinger.DiffSingerEnglishPhonemizer";
pub const APPLIED: &str = "ENGLISH_PRONUNCIATION_APPLIED";
pub const UNSUPPORTED: &str = "ENGLISH_PRONUNCIATION_UNSUPPORTED";
pub const AMBIGUOUS: &str = "ENGLISH_PRONUNCIATION_AMBIGUOUS";
pub const VOWEL_MISMATCH: &str = "ENGLISH_PRONUNCIATION_VOWEL_MISMATCH";

const DATA: &str = include_str!("english-lexicon.tsv");
static DICTIONARY: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

fn lookup(key: &str) -> Option<&'static str> {
    DICTIONARY
        .get_or_init(|| lexical::index(DATA))
        .get(key)
        .copied()
}

/// Read-only lexical evidence for the automatic FR/EN router. This deliberately
/// ignores semantic ambiguity: ambiguity is resolved by surrounding text and the
/// existing pronunciation pass still refuses unsafe dictionary variants.
pub(crate) fn contains_lexeme(key: &str) -> bool {
    let key = lexical::normalize(key);
    !key.is_empty() && lookup(&key).is_some()
}

fn ambiguous(key: &str) -> bool {
    // Identical vowel counts do not resolve grammar or meaning. Literal CMU
    // numbered keys are explicit requests and deliberately do not match here.
    matches!(
        key,
        "read"
            | "wound"
            | "lead"
            | "live"
            | "bow"
            | "tear"
            | "wind"
            | "bass"
            | "close"
            | "desert"
            | "present"
            | "object"
            | "record"
            | "content"
            | "minute"
    )
}

fn valid_hint(hint: &str) -> bool {
    // Validate the complete reading: OpenUtau silently filtering an unknown
    // symbol would not constitute a valid pronunciation of the source word.
    lexical::vowel_count(hint) > 0
        && hint.split_whitespace().all(|phone| {
            matches!(
                phone,
                "en/aa"
                    | "en/ae"
                    | "en/ah"
                    | "en/ao"
                    | "en/aw"
                    | "en/ay"
                    | "en/eh"
                    | "en/er"
                    | "en/ey"
                    | "en/ih"
                    | "en/iy"
                    | "en/ow"
                    | "en/oy"
                    | "en/uh"
                    | "en/uw"
                    | "en/b"
                    | "en/ch"
                    | "en/d"
                    | "en/dh"
                    | "en/f"
                    | "en/g"
                    | "en/hh"
                    | "en/jh"
                    | "en/k"
                    | "en/l"
                    | "en/m"
                    | "en/n"
                    | "en/ng"
                    | "en/p"
                    | "en/r"
                    | "en/s"
                    | "en/sh"
                    | "en/t"
                    | "en/th"
                    | "en/v"
                    | "en/w"
                    | "en/y"
                    | "en/z"
                    | "en/zh"
            )
        })
}

fn diagnostic(code: &str, message: String, source_id: &str) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity: if code == APPLIED {
            DiagnosticSeverity::Info
        } else {
            DiagnosticSeverity::Warning
        },
        message,
        source_id: Some(source_id.into()),
    }
}

fn reading(key: &str) -> Result<&'static str, (&'static str, String)> {
    if ambiguous(key) {
        return Err((AMBIGUOUS, format!("English ARPAbet: {key:?} needs a grammatical or semantic reading; select an explicit dictionary variant or manual hint. Source text and attacks were retained.")));
    }
    let hint = lookup(key).ok_or_else(|| (UNSUPPORTED, format!("English ARPAbet has no pinned dictionary reading for {key:?}; source text and attacks were retained.")))?;
    if !valid_hint(hint) {
        return Err((UNSUPPORTED, format!("English ARPAbet: {key:?} has no complete CMU39 reading with a sung vowel; source text and attacks were retained.")));
    }
    Ok(hint)
}

/// Called separately for each source voice and repeat occurrence. A complete
/// coherent word may use native `+` allocation only when the specified reading
/// has exactly as many vowels as the source states attacks. Holds consume none.
fn apply_inner(
    notes: &mut [ProjectedNote],
    note_ids: &[String],
    automatic_recovery: bool,
) -> Vec<Diagnostic> {
    assert_eq!(notes.len(), note_ids.len());
    preserve_bracketed_melismas(notes);
    let fragments = lexical::fragments(notes);
    let mut handled = vec![false; notes.len()];
    let mut changed = vec![false; notes.len()];
    let mut diagnostics = Vec::new();

    let words = if automatic_recovery {
        lexical::automatic_words(notes)
    } else {
        lexical::words(notes)
    };
    for members in words {
        let key = if automatic_recovery {
            lexical::preferred_joined_key(notes, &members, contains_lexeme)
        } else {
            lexical::joined_key(notes, &members)
        };
        let result = reading(&key).and_then(|hint| {
            let vowels = lexical::vowel_count(hint);
            if vowels == members.len() {
                Ok(hint)
            } else {
                Err((VOWEL_MISMATCH, format!("English ARPAbet: {key:?} has {vowels} dictionary vowel(s) for {} source syllable attacks; no alternative variant was inferred, and source text and attacks were retained.", members.len())))
            }
        });
        match result {
            Ok(hint) => {
                lexical::pronounce_word(notes, &members, &key, hint);
                for &index in &members {
                    changed[index] = true;
                }
            }
            Err((code, message)) => {
                for &index in &members {
                    diagnostics.push(diagnostic(code, message.clone(), &note_ids[index]));
                }
            }
        }
        for index in members {
            handled[index] = true;
        }
    }

    for index in 0..notes.len() {
        if handled[index] {
            continue;
        }
        let Some(key) = lexical::candidate(&notes[index].lyric) else {
            continue;
        };
        if fragments[index]
            || lexical::source(&notes[index].lyric)
                .is_some_and(|source| !matches!(source.syllabic, None | Some(Syllabic::Single)))
        {
            diagnostics.push(diagnostic(UNSUPPORTED, format!("English ARPAbet: source fragment {:?} belongs to an unrecognized or incomplete connected word; no independent whole-word reading was applied.", lexical::raw_text(&notes[index].lyric).unwrap_or_default()), &note_ids[index]));
            continue;
        }
        match reading(&key) {
            Ok(hint) => {
                lexical::pronounce(&mut notes[index], &key, hint);
                changed[index] = true;
            }
            Err((code, message)) => diagnostics.push(diagnostic(code, message, &note_ids[index])),
        }
    }

    for (index, was_changed) in changed.into_iter().enumerate() {
        if !was_changed {
            continue;
        }
        let rendered = match &notes[index].lyric {
            ProjectedLyric::Pronounced { text, phonemes, .. } => format!("{text}[{phonemes}]"),
            ProjectedLyric::PronouncedSplit { .. } => "+".into(),
            _ => continue,
        };
        diagnostics.push(diagnostic(APPLIED, format!("English ARPAbet: source lyric {:?} is rendered as {rendered}; original lyric evidence and musical identity are preserved.", lexical::source(&notes[index].lyric).map(|source| source.raw.as_str()).unwrap_or_default()), &note_ids[index]));
    }
    diagnostics
}

pub fn apply(notes: &mut [ProjectedNote], note_ids: &[String]) -> Vec<Diagnostic> {
    apply_inner(notes, note_ids, false)
}

pub(crate) fn apply_automatic(notes: &mut [ProjectedNote], note_ids: &[String]) -> Vec<Diagnostic> {
    apply_inner(notes, note_ids, true)
}

#[cfg(test)]
mod tests {
    use super::{apply_automatic, valid_hint};
    use crate::engine::midi::{Lyric, Syllabic};
    use crate::engine::projection::{ProjectedLyric, ProjectedNote};

    fn source_note(onset: u32, text: &str, syllabic: Syllabic) -> ProjectedNote {
        let mut lyric = Lyric::text(onset.to_string(), text.into());
        lyric.syllabic = Some(syllabic);
        ProjectedNote {
            performance: None,
            pronunciation_language: None,
            source_evidence: None,
            onset_ticks: onset,
            duration_ticks: 480,
            pitch: 60,
            lyric: ProjectedLyric::Source(Box::new(lyric)),
        }
    }

    #[test]
    fn english_reading_validation_rejects_the_entire_invalid_or_vowelless_hint() {
        assert!(valid_hint("en/d en/eh en/t"));
        for hint in [
            "",
            "en/d en/t",
            "en/d en/eh en/not-a-phone",
            "en/d en/eh fr/t",
            "en/d en/eh1 en/t",
        ] {
            assert!(!valid_hint(hint), "unsafe complete reading: {hint}");
        }
    }

    #[test]
    fn overlap_recovery_changes_only_the_dictionary_lookup_key() {
        let mut notes = [
            source_note(0, "Beat", Syllabic::Begin),
            source_note(480, "tles", Syllabic::End),
        ];
        let diagnostics = apply_automatic(&mut notes, &["0".into(), "1".into()]);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code == super::APPLIED),
            "unexpected recovery diagnostics: {diagnostics:?}"
        );
        let ProjectedLyric::Pronounced { text, .. } = &notes[0].lyric else {
            panic!("recovered word head should be pronounced");
        };
        assert_eq!(
            text, "beatles",
            "recovered spelling renders the complete word"
        );
        let ProjectedLyric::Pronounced { source, .. } = &notes[0].lyric else {
            unreachable!();
        };
        assert_eq!(source.raw, "Beat", "raw source evidence stays unchanged");
        assert!(matches!(
            &notes[1].lyric,
            ProjectedLyric::PronouncedSplit { source } if source.raw == "tles"
        ));
    }
}
