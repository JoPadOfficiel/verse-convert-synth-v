//! Shared lookup and source-word guards for explicit DiffSinger profiles.
//! A dictionary supplies a reading, never a missing note or a word boundary.
use std::collections::HashMap;

use unicode_normalization::UnicodeNormalization;

use crate::engine::midi::{Lyric, LyricState, Syllabic};
use crate::engine::projection::{ProjectedLyric, ProjectedNote};
use crate::engine::syllable::{hyphen_markers, touches};

pub(super) fn index(data: &'static str) -> HashMap<&'static str, &'static str> {
    data.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some((fields.next()?, fields.next()?))
        })
        .collect()
}

fn edge(c: char) -> bool {
    c.is_whitespace() || ",.;:!?…\"«»“”„".contains(c)
}

fn trim_edges(text: &str) -> &str {
    let mut text = text.trim_start_matches(|c: char| edge(c) || c == '(');
    loop {
        text = text.trim_end_matches(edge);
        if !text.ends_with(')') {
            return text;
        }
        if let Some((base, variant)) = text[..text.len() - 1].rsplit_once('(') {
            if !base.is_empty()
                && !variant.is_empty()
                && variant.bytes().all(|b| b.is_ascii_digit())
            {
                return text;
            }
        }
        text = &text[..text.len() - 1];
    }
}

/// Preserve literal dictionary variants `word(2)` and internal apostrophes.
pub(super) fn normalize(text: &str) -> String {
    let text = trim_edges(text);
    let text =
        if text.chars().count() > 2 && text.starts_with(['\'', '‘']) && text.ends_with(['\'', '’'])
        {
            &text[text.chars().next().unwrap().len_utf8()
                ..text.len() - text.chars().last().unwrap().len_utf8()]
        } else {
            text
        };
    let text = trim_edges(text);
    // A bare elision is a source fragment, not the name of a letter (`l'`/L).
    let apostrophe_key = text.to_lowercase().replace(['‘', '’'], "'");
    let text = if matches!(
        apostrophe_key.as_str(),
        "l'" | "d'" | "j'" | "t'" | "m'" | "n'" | "s'" | "c'" | "qu'"
    ) {
        text
    } else {
        trim_edges(text.trim_end_matches(|c: char| edge(c) || "'’".contains(c)))
    };
    hyphen_markers(text)
        .2
        .trim()
        .to_lowercase()
        .replace(['‘', '’'], "'")
        .nfc()
        .collect()
}

pub(super) fn source(lyric: &ProjectedLyric) -> Option<&Lyric> {
    match lyric {
        ProjectedLyric::Source(source)
        | ProjectedLyric::Pronounced { source, .. }
        | ProjectedLyric::PronouncedSplit { source } => Some(source),
        _ => None,
    }
}

pub(super) fn raw_text(lyric: &ProjectedLyric) -> Option<&str> {
    match &source(lyric)?.state {
        LyricState::Text(text) => Some(text),
        _ => None,
    }
}

pub(super) fn candidate(lyric: &ProjectedLyric) -> Option<String> {
    if !matches!(lyric, ProjectedLyric::Source(_)) {
        return None;
    }
    let text = raw_text(lyric)?;
    if matches!(
        text.trim(),
        "-" | "br" | "AP" | "EP" | "SP" | "cl" | "q" | "gs" | "R"
    ) {
        return None;
    }
    if text.contains(['[', ']']) || text.trim_start().starts_with(['+', '?']) {
        return None;
    }
    let key = normalize(text);
    (!key.is_empty()).then_some(key)
}

fn ends_phrase(text: &str) -> bool {
    text.trim_end_matches(|c: char| c.is_whitespace() || "'’\"»”)]}".contains(c))
        .ends_with([',', '.', ';', ':', '!', '?', '…'])
}

fn binding(lyric: &ProjectedLyric) -> Option<(bool, bool)> {
    let lyric = source(lyric)?;
    let LyricState::Text(text) = &lyric.state else {
        return None;
    };
    Some(match lyric.syllabic {
        Some(Syllabic::Begin) => (false, true),
        Some(Syllabic::Middle) => (true, true),
        Some(Syllabic::End) => (true, false),
        Some(Syllabic::Single) => (false, false),
        None => {
            let (previous, next, _) = hyphen_markers(text);
            (previous, next)
        }
    })
}

fn next_attack(notes: &[ProjectedNote], head: usize) -> Option<usize> {
    let lane = &source(&notes[head].lyric)?.lane;
    let mut next = head + 1;
    while next < notes.len() && touches(&notes[next - 1], &notes[next]) {
        if source(&notes[next].lyric).is_some_and(|lyric| &lyric.lane != lane) {
            return None;
        }
        match &notes[next].lyric {
            ProjectedLyric::Extension => {}
            ProjectedLyric::Source(lyric) if lyric.state == LyricState::Continuation => {}
            _ => return Some(next),
        }
        next += 1;
    }
    None
}

/// Even incomplete coherent bindings protect a fragment from whole-word lookup.
pub(super) fn fragments(notes: &[ProjectedNote]) -> Vec<bool> {
    let mut result: Vec<bool> = notes
        .iter()
        .map(|note| {
            source(&note.lyric).is_some_and(|lyric| {
                lyric.syllabic.is_none()
                    && raw_text(&note.lyric).is_some_and(|text| {
                        let (left, right, _) = hyphen_markers(text);
                        left || right
                    })
            })
        })
        .collect();
    // Qualified MIDI/KAR text retains its encoded payload and line controls.
    // When the source demonstrates a whitespace-delimited word convention,
    // records without a boundary are fragments even across performed gaps.
    // This is only a lookup guard: native + cannot bridge those gaps without
    // changing the source duration. XML lyrics have no encoded MIDI payload.
    fn encoded(lyric: &ProjectedLyric) -> Option<&Lyric> {
        source(lyric).filter(|s| !s.raw_bytes.is_empty() && s.syllabic.is_none())
    }
    let has_lines = notes
        .iter()
        .any(|n| encoded(&n.lyric).is_some_and(|s| s.line_break.is_some()));
    let has_word_spaces = notes.iter().any(|n| {
        encoded(&n.lyric).is_some_and(|s| {
            s.raw.starts_with(char::is_whitespace) || s.raw.ends_with(char::is_whitespace)
        })
    });
    if has_lines && has_word_spaces {
        for (head, pair) in notes.windows(2).enumerate() {
            if let (Some(left), Some(right)) = (encoded(&pair[0].lyric), encoded(&pair[1].lyric)) {
                if left.lane == right.lane
                    && right.line_break.is_none()
                    && !right.raw.starts_with(char::is_whitespace)
                    && !left.raw.ends_with(char::is_whitespace)
                    && raw_text(&pair[0].lyric)
                        .is_some_and(|text| !text.is_empty() && !ends_phrase(text))
                    && raw_text(&pair[1].lyric).is_some_and(|text| !text.is_empty())
                {
                    result[head] = true;
                    result[head + 1] = true;
                }
            }
        }
    }
    for head in 0..notes.len() {
        if raw_text(&notes[head].lyric).is_none_or(ends_phrase) {
            continue;
        }
        if let Some(next) = next_attack(notes, head) {
            let Some((_, right)) = binding(&notes[head].lyric) else {
                continue;
            };
            let Some((left, _)) = binding(&notes[next].lyric) else {
                continue;
            };
            let first = source(&notes[head].lyric).unwrap();
            let second = source(&notes[next].lyric).unwrap();
            let dashed_pair = first.syllabic != Some(Syllabic::Single)
                && second.syllabic != Some(Syllabic::Single)
                && ((first.syllabic.is_none() && right) || (second.syllabic.is_none() && left));
            if (right && left) || dashed_pair {
                result[head] = true;
                result[next] = true;
            }
        }
    }
    result
}

/// Complete bilateral Begin/Middle/End (or bilateral dashes), same source row.
/// Callers isolate score voice and repeat domains before entering this function.
pub(super) fn words(notes: &[ProjectedNote]) -> Vec<Vec<usize>> {
    let mut result = Vec::new();
    for head in 0..notes.len() {
        if candidate(&notes[head].lyric).is_none()
            || binding(&notes[head].lyric) != Some((false, true))
        {
            continue;
        }
        let mut members = vec![head];
        let mut current = head;
        loop {
            if raw_text(&notes[current].lyric).is_none_or(ends_phrase) {
                break;
            }
            let Some(next) = next_attack(notes, current) else {
                break;
            };
            if candidate(&notes[next].lyric).is_none() {
                break;
            }
            match binding(&notes[next].lyric) {
                Some((true, continues)) => {
                    members.push(next);
                    if !continues {
                        result.push(members);
                        break;
                    }
                    current = next;
                }
                _ => break,
            }
        }
    }
    result
}

pub(super) fn joined_key(notes: &[ProjectedNote], members: &[usize]) -> String {
    members
        .iter()
        .map(|&index| normalize(raw_text(&notes[index].lyric).unwrap_or_default()))
        .collect()
}

pub(super) fn vowel_count(hint: &str) -> usize {
    hint.split_whitespace()
        .filter(|phone| {
            matches!(
                *phone,
                "fr/ah"
                    | "fr/eh"
                    | "fr/ae"
                    | "fr/ee"
                    | "fr/oe"
                    | "fr/ih"
                    | "fr/oh"
                    | "fr/oo"
                    | "fr/ou"
                    | "fr/uh"
                    | "fr/en"
                    | "fr/in"
                    | "fr/on"
                    | "en/aa"
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
            )
        })
        .count()
}

pub(super) fn pronounce(note: &mut ProjectedNote, text: &str, hint: &str) {
    if let ProjectedLyric::Source(source) = &note.lyric {
        note.lyric = ProjectedLyric::Pronounced {
            source: source.clone(),
            text: text.into(),
            phonemes: hint.into(),
        };
    }
}

pub(super) fn pronounce_word(
    notes: &mut [ProjectedNote],
    members: &[usize],
    key: &str,
    hint: &str,
) {
    let Some((&head, tail)) = members.split_first() else {
        return;
    };
    pronounce(&mut notes[head], key, hint);
    for &index in tail {
        if let ProjectedLyric::Source(source) = &notes[index].lyric {
            notes[index].lyric = ProjectedLyric::PronouncedSplit {
                source: source.clone(),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::midi::LineBreak;

    #[test]
    fn encoded_karaoke_fragments_are_not_dictionary_words_even_across_a_gap() {
        let mut notes: Vec<_> = [("Li", "/Li"), ("ving", "ving"), ("with", " with")]
            .into_iter()
            .enumerate()
            .map(|(i, (text, raw))| {
                let mut lyric = Lyric::text(i.to_string(), text.into());
                lyric.raw = raw.into();
                lyric.raw_bytes = raw.as_bytes().to_vec();
                lyric.line_break = (i == 0).then_some(LineBreak::Line);
                ProjectedNote {
                    onset_ticks: i as u32 * 480,
                    duration_ticks: 240,
                    pitch: 60,
                    lyric: ProjectedLyric::Source(Box::new(lyric)),
                }
            })
            .collect();
        assert_eq!(fragments(&notes), [true, true, false]);
        let original = notes.clone();
        let diagnostics =
            super::super::english::apply(&mut notes, &["0".into(), "1".into(), "2".into()]);
        assert_eq!(notes[..2], original[..2]);
        assert_eq!(
            diagnostics
                .iter()
                .filter(|d| d.code == super::super::english::UNSUPPORTED)
                .count(),
            2
        );
        // A non-KAR score spelling the actual proper name Li has no such claim.
        for note in &mut notes {
            if let ProjectedLyric::Source(source) = &mut note.lyric {
                source.raw_bytes.clear();
            }
        }
        assert_eq!(fragments(&notes), [false, false, false]);
    }
}
