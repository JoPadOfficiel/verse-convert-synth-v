//! French Millefeuille pronunciation with indexed, pinned community readings.
//!
//! `french-lexicon.tsv` pins the community readings and their original symbols.
//! Layouts below choose sung variants only from source note evidence. Numbers
//! in dictionary keys are literal variants, never a general schwa convention.
use std::{collections::HashMap, sync::OnceLock};

use super::lexical as shared;
use crate::engine::convert::{Diagnostic, DiagnosticSeverity};
use crate::engine::midi::{Lyric, LyricState, Syllabic};
use crate::engine::projection::{ProjectedLyric, ProjectedNote};
use crate::engine::syllable::{preserve_bracketed_melismas, touches};

// The misspelling is the actual installed OpenUtau type name.
pub const PHONEMIZER: &str = "OpenUtau.Core.DiffSinger.DiffSingerFrenchMillfeuillePhonemizer";
pub const UNSUPPORTED: &str = "FRENCH_PRONUNCIATION_UNSUPPORTED";
pub const APPLIED: &str = "FRENCH_PRONUNCIATION_APPLIED";
pub const LIAISON: &str = "FRENCH_LIAISON_APPLIED";

const LEXICON: &str = include_str!("french-lexicon.tsv");

const COMMUNITY: &str = include_str!("french-community.tsv");
static CURATED_INDEX: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
static COMMUNITY_INDEX: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

fn normalize(text: &str) -> String {
    shared::normalize(text)
}

/// These base forms need grammatical context. Numbered source variants remain
/// explicit choices; their number is never interpreted as a general rule.
fn ambiguous(key: &str) -> bool {
    matches!(
        key,
        "bus" | "fils" | "as" | "tous" | "os" | "plus" | "couvent"
        | "content" | "portions" | "président" | "résident" | "sens" | "est"
        // These audited isolated fragments also collide with rare dictionary
        // lexemes; only a complete word/layout can disambiguate their use.
        | "rai" | "ur" | "urs" | "ure" | "ures"
    )
}

fn lexical(key: &str) -> Option<String> {
    let special = match key {
        "d'un" => Some("fr/d fr/in"),
        "tau" => Some("fr/t fr/oh"),
        "rê" => Some("fr/r fr/ae"),
        "laisses" => return lexical("laisse"),
        _ => None,
    };
    if let Some(hint) = special {
        return Some(hint.into());
    }
    // The original curated grammatical reading of est supports the verified
    // est/un liaison. Other ambiguous base forms remain unforced.
    if let Some(hint) = CURATED_INDEX
        .get_or_init(|| shared::index(LEXICON))
        .get(key)
    {
        return Some((*hint).into());
    }
    if ambiguous(key) {
        return None;
    }
    let dictionary = COMMUNITY_INDEX.get_or_init(|| shared::index(COMMUNITY));
    if let Some(hint) = dictionary.get(key) {
        return Some((*hint).into());
    }
    // An explicit elision contributes its consonant only when its remainder
    // is itself known. It does not create a liaison at another word boundary.
    for (prefix, phone) in [
        ("l'", "fr/l"),
        ("d'", "fr/d"),
        ("j'", "fr/j"),
        ("t'", "fr/t"),
        ("m'", "fr/m"),
        ("n'", "fr/n"),
        ("s'", "fr/s"),
        ("c'", "fr/s"),
        ("qu'", "fr/k"),
    ] {
        if let Some(remainder) = key.strip_prefix(prefix) {
            if !remainder.contains('\'')
                && !ambiguous(remainder)
                && (remainder.chars().count() > 1 || matches!(remainder, "a" | "y"))
            {
                let hint = CURATED_INDEX
                    .get_or_init(|| shared::index(LEXICON))
                    .get(remainder)
                    .or_else(|| dictionary.get(remainder));
                if let Some(hint) = hint {
                    return Some(format!("{phone} {hint}"));
                }
            }
        }
    }
    None
}

fn standalone_allowed(lyric: &ProjectedLyric, key: &str) -> bool {
    // FR-001 explicitly audited these isolated spellings despite orphan score
    // metadata. New community entries need a complete word or an independent
    // source token, not an unfinished Begin/Middle/End fragment.
    source(lyric).is_some_and(|source| {
        matches!(source.syllabic, None | Some(Syllabic::Single))
            || CURATED_INDEX
                .get_or_init(|| shared::index(LEXICON))
                .contains_key(key)
            || matches!(key, "d'un" | "tau" | "rê" | "laisses")
    })
}

struct Layout {
    word: &'static str,
    syllables: &'static [&'static str],
    hints: &'static [&'static str],
}

// Longest layouts first. Each slot is an attack, including repeated vowels;
// only an actual source extension (or a bracketed empty slot) becomes a hold.
const LAYOUTS: &[Layout] = &[
    Layout {
        word: "murmures",
        syllables: &["mur", "mu", "u", "ures"],
        hints: &["fr/m fr/uh fr/r", "fr/m fr/uh", "fr/uh", "fr/uh fr/r"],
    },
    Layout {
        word: "murmure",
        syllables: &["mur", "mu", "u", "ure"],
        hints: &["fr/m fr/uh fr/r", "fr/m fr/uh", "fr/uh", "fr/uh fr/r"],
    },
    Layout {
        word: "m'arrête",
        syllables: &["m'ar", "rê", "te"],
        hints: &["fr/m fr/ah", "fr/r fr/ae", "fr/t fr/ee"],
    },
    Layout {
        word: "murs",
        syllables: &["mu", "u", "urs"],
        hints: &["fr/m fr/uh", "fr/uh", "fr/uh fr/r"],
    },
    Layout {
        word: "mur",
        syllables: &["mu", "u", "ur"],
        hints: &["fr/m fr/uh", "fr/uh", "fr/uh fr/r"],
    },
    Layout {
        word: "rêves",
        syllables: &["rê", "ê", "ves"],
        hints: &["fr/r fr/ae", "fr/ae", "fr/v fr/ee"],
    },
    // The audited Bass ending spells the full word on the first note, then
    // repeats its vowel and sings `ves` separately. Place v only on that tail.
    Layout {
        word: "rêves",
        syllables: &["rêves", "ê", "ves"],
        hints: &["fr/r fr/ae", "fr/ae", "fr/v fr/ee"],
    },
    Layout {
        word: "rêve",
        syllables: &["rê", "ê", "ve"],
        hints: &["fr/r fr/ae", "fr/ae", "fr/v fr/ee"],
    },
    Layout {
        word: "vent",
        syllables: &["vent", "en", "ent"],
        hints: &["fr/v fr/en", "fr/en", "fr/en"],
    },
    Layout {
        word: "court",
        syllables: &["court", "ou", "ourt"],
        hints: &["fr/k fr/ou", "fr/ou", "fr/ou fr/r"],
    },
    Layout {
        word: "fond",
        syllables: &["fond", "on", "on"],
        hints: &["fr/f fr/on", "fr/on", "fr/on"],
    },
    Layout {
        word: "tempêtes",
        syllables: &["tem", "pê", "tes"],
        hints: &["fr/t fr/en", "fr/p fr/ae", "fr/t fr/ee"],
    },
    Layout {
        word: "tempête",
        syllables: &["tem", "pê", "te"],
        hints: &["fr/t fr/en", "fr/p fr/ae", "fr/t fr/ee"],
    },
    Layout {
        word: "blessures",
        syllables: &["bles", "su", "res"],
        hints: &["fr/b fr/l fr/ae", "fr/s fr/uh", "fr/r fr/ee"],
    },
    Layout {
        word: "m'arrête",
        syllables: &["m'ar", "rête"],
        hints: &["fr/m fr/ah", "fr/r fr/ae fr/t"],
    },
    Layout {
        word: "jure",
        syllables: &["ju", "ure"],
        hints: &["fr/j fr/uh", "fr/uh fr/r"],
    },
    Layout {
        word: "changer",
        syllables: &["chan", "ger"],
        hints: &["fr/sh fr/en", "fr/j fr/eh"],
    },
    Layout {
        word: "même",
        syllables: &["mê", "me"],
        hints: &["fr/m fr/ae", "fr/m fr/ee"],
    },
    Layout {
        word: "presse",
        syllables: &["pres", "se"],
        hints: &["fr/p fr/r fr/ae", "fr/s fr/ee"],
    },
    Layout {
        word: "rêves",
        syllables: &["rê", "ves"],
        hints: &["fr/r fr/ae", "fr/v fr/ee"],
    },
    Layout {
        word: "rêve",
        syllables: &["rê", "ve"],
        hints: &["fr/r fr/ae", "fr/v fr/ee"],
    },
    Layout {
        word: "blessures",
        syllables: &["bles", "sures"],
        hints: &["fr/b fr/l fr/ae", "fr/s fr/uh fr/r"],
    },
    Layout {
        word: "blessure",
        syllables: &["bles", "sure"],
        hints: &["fr/b fr/l fr/ae", "fr/s fr/uh fr/r"],
    },
    Layout {
        word: "amours",
        syllables: &["a", "mours"],
        hints: &["fr/ah", "fr/m fr/ou fr/r"],
    },
    Layout {
        word: "laisses",
        syllables: &["lais", "ses"],
        hints: &["fr/l fr/ae", "fr/s fr/ee"],
    },
    Layout {
        word: "genoux",
        syllables: &["ge", "noux"],
        hints: &["fr/j fr/ee", "fr/n fr/ou"],
    },
    Layout {
        word: "ferons",
        syllables: &["fe", "rons"],
        hints: &["fr/f fr/ee", "fr/r fr/on"],
    },
    Layout {
        word: "moments",
        syllables: &["mo", "ments"],
        hints: &["fr/m fr/oo", "fr/m fr/en"],
    },
];

fn source(lyric: &ProjectedLyric) -> Option<&Lyric> {
    match lyric {
        ProjectedLyric::Source(source)
        | ProjectedLyric::Pronounced { source, .. }
        | ProjectedLyric::PronouncedSplit { source } => Some(source),
        _ => None,
    }
}

fn text(lyric: &ProjectedLyric) -> Option<&str> {
    match &source(lyric)?.state {
        LyricState::Text(text) => Some(text),
        _ => None,
    }
}

fn manual(text: &str) -> bool {
    text.contains('[') || text.contains(']') || text.trim_start().starts_with(['+', '?'])
}

fn candidate(lyric: &ProjectedLyric) -> Option<String> {
    shared::candidate(lyric)
}

fn same_lane(left: &ProjectedLyric, right: &ProjectedLyric) -> bool {
    match (source(left), source(right)) {
        (Some(left), Some(right)) => left.lane == right.lane,
        _ => false,
    }
}

fn ends_phrase(text: &str) -> bool {
    text.trim_end_matches(|c: char| c.is_whitespace() || "'’\"»”)]}".contains(c))
        .ends_with([',', '.', ';', ':', '!', '?', '…'])
}

/// Next attack inside this source row, allowing only touching genuine holds.
fn next_attack(notes: &[ProjectedNote], head: usize) -> Option<usize> {
    let mut index = head + 1;
    while index < notes.len() && touches(&notes[index - 1], &notes[index]) {
        if source(&notes[index].lyric).is_some()
            && !same_lane(&notes[head].lyric, &notes[index].lyric)
        {
            return None;
        }
        match &notes[index].lyric {
            ProjectedLyric::Extension => {}
            ProjectedLyric::Source(lyric) if lyric.state == LyricState::Continuation => {}
            _ => return Some(index),
        }
        index += 1;
    }
    None
}

fn layout_members(notes: &[ProjectedNote], head: usize, layout: &Layout) -> Option<Vec<usize>> {
    let mut members = Vec::new();
    let mut index = head;
    for (slot, expected) in layout.syllables.iter().enumerate() {
        if slot > 0 {
            index = next_attack(notes, index)?;
            if !same_lane(&notes[head].lyric, &notes[index].lyric) {
                return None;
            }
        }
        if candidate(&notes[index].lyric).as_deref() != Some(expected) {
            return None;
        }
        // `single` explicitly asks for an independent word. Repeated `begin`
        // metadata also occurs inside the audited rê/ê/ves and mur/mu/u/ures
        // layouts. Exact curated layouts can supply pronunciation despite that
        // incomplete binding: each syllable still has its own original attack,
        // text and metadata, and no words are joined.
        let syllabic = source(&notes[index].lyric)?.syllabic.clone();
        if syllabic == Some(Syllabic::Single)
            || (slot + 1 < layout.syllables.len()
                && (syllabic == Some(Syllabic::End) || ends_phrase(text(&notes[index].lyric)?)))
        {
            return None;
        }
        members.push(index);
    }
    Some(members)
}

fn diagnose(code: &str, message: String, id: &str) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity: if code == UNSUPPORTED {
            DiagnosticSeverity::Warning
        } else {
            DiagnosticSeverity::Info
        },
        message,
        source_id: Some(id.into()),
    }
}

fn pronounce(note: &mut ProjectedNote, hint: &str) {
    let ProjectedLyric::Source(source) = &note.lyric else {
        return;
    };
    let Some(text) = text(&note.lyric) else {
        return;
    };
    note.lyric = ProjectedLyric::Pronounced {
        source: source.clone(),
        text: normalize(text),
        phonemes: hint.into(),
    };
}

/// Apply once to one source voice / repeat occurrence before default joining.
/// Original source objects remain immutable. Running again adds nothing.
pub fn apply(notes: &mut [ProjectedNote], note_ids: &[String]) -> Vec<Diagnostic> {
    assert_eq!(notes.len(), note_ids.len());
    preserve_bracketed_melismas(notes);
    let mut diagnostics = Vec::new();
    let mut changed = vec![false; notes.len()];
    let fragments = shared::fragments(notes);
    // Word boundaries are independent of the surface syllable under a note.
    // In particular, the `tes` tail of tempêtes is never a determiner.
    let mut words: Vec<(usize, usize, String)> = Vec::new();
    for head in 0..notes.len() {
        for layout in LAYOUTS {
            let Some(members) = layout_members(notes, head, layout) else {
                continue;
            };
            words.push((head, *members.last().unwrap(), layout.word.into()));
            for (member, hint) in members.into_iter().zip(layout.hints) {
                pronounce(&mut notes[member], hint);
                changed[member] = true;
            }
            break;
        }
    }
    // Curated sung layouts take priority. Other complete source words use the
    // native phonemizer's syllable allocation only when every attack has a vowel.
    for members in shared::words(notes) {
        let key = shared::joined_key(notes, &members);
        if let Some(hint) = lexical(&key) {
            if shared::vowel_count(&hint) == members.len() {
                shared::pronounce_word(notes, &members, &key, &hint);
                words.push((members[0], *members.last().unwrap(), key));
                for member in members {
                    changed[member] = true;
                }
            }
        }
    }
    for index in 0..notes.len() {
        if let Some(key) = candidate(&notes[index].lyric) {
            // A variant is looked up literally, never selected from a guessed
            // grammatical or sung-schwa rule.
            if (!fragments[index] || key == "rê") && standalone_allowed(&notes[index].lyric, &key)
            {
                if let Some(hint) = lexical(&key) {
                    pronounce(&mut notes[index], &hint);
                    changed[index] = true;
                    if key != "rê" {
                        words.push((index, index, key));
                    }
                    continue;
                }
            }
            diagnostics.push(diagnose(UNSUPPORTED, format!(
                "French Millefeuille has no unambiguous reading compatible with the source syllable layout for lyric {:?} in this layout; text and attacks were retained.", text(&notes[index].lyric).unwrap_or_default()
            ), &note_ids[index]));
        }
    }
    // Liaison belongs to the next attack, never a trailing phoneme attached to
    // the preceding vowel. Do not cross a hold, rest, row or manual hint.
    words.sort_by_key(|word| word.0);
    for pair in words.windows(2) {
        let (_, tail, left) = &pair[0];
        let (index, _, right) = &pair[1];
        let (tail, index) = (*tail, *index);
        if tail + 1 != index
            || !touches(&notes[tail], &notes[index])
            || !same_lane(&notes[tail].lyric, &notes[index].lyric)
        {
            continue;
        }
        let Some(left_text) = text(&notes[tail].lyric) else {
            continue;
        };
        if manual(left_text) || ends_phrase(left_text) {
            continue;
        }
        let consonant = match (left.as_str(), right.as_str()) {
            ("tout", "au" | "à") | ("est", "un") => "fr/t",
            (
                "mes" | "tes" | "ses" | "les" | "des" | "nos" | "vos",
                "ami" | "amis" | "amours" | "enfant" | "enfants" | "homme" | "hommes",
            ) => "fr/z",
            ("un" | "mon" | "ton" | "son", "ami" | "enfant") => "fr/n",
            _ => continue,
        };
        let ProjectedLyric::Pronounced { phonemes, .. } = &mut notes[index].lyric else {
            continue;
        };
        if phonemes.split_whitespace().next() == Some(consonant) {
            continue;
        }
        *phonemes = format!("{consonant} {phonemes}");
        diagnostics.push(diagnose(LIAISON, format!(
            "French Millefeuille: {left} / {right} adds {consonant} on the next touching note in the same source lane."
        ), &note_ids[index]));
    }
    for index in 0..notes.len() {
        if changed[index] {
            let ProjectedLyric::Pronounced {
                source,
                text,
                phonemes,
            } = &notes[index].lyric
            else {
                if let ProjectedLyric::PronouncedSplit { source } = &notes[index].lyric {
                    diagnostics.push(diagnose(APPLIED, format!(
                        "French Millefeuille: {:?} remains a separate source-owned syllable of the preceding dictionary word; rendered as +.", source.raw
                    ), &note_ids[index]));
                }
                continue;
            };
            diagnostics.push(diagnose(APPLIED, format!(
                "French Millefeuille: {:?} is rendered as {text}[{phonemes}]; original lyric evidence is preserved.", source.raw
            ), &note_ids[index]));
        }
    }
    diagnostics
}
