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
        "ouh" => Some("fr/ou"),
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
    // An empty identity denotes the independent words in `syllables`.
    word: &'static str,
    syllables: &'static [&'static str],
    hints: &'static [&'static str],
    // Only explicitly audited sung layouts can override a premature End.
    orphan_end_slots: &'static [usize],
}

// Longest layouts first. Each slot is an attack, including repeated vowels;
// only an actual source extension (or a bracketed empty slot) becomes a hold.
const LAYOUTS: &[Layout] = &[
    Layout {
        word: "murmures",
        syllables: &["mur", "mu", "u", "ures"],
        hints: &["fr/m fr/uh fr/r", "fr/m fr/uh", "fr/uh", "fr/uh fr/r"],
        orphan_end_slots: &[1],
    },
    Layout {
        word: "murmure",
        syllables: &["mur", "mu", "u", "ure"],
        hints: &["fr/m fr/uh fr/r", "fr/m fr/uh", "fr/uh", "fr/uh fr/r"],
        orphan_end_slots: &[1],
    },
    Layout {
        word: "j'effacerai",
        syllables: &["j'ef", "fa", "ce", "rai"],
        hints: &["fr/j fr/ae", "fr/f fr/ah", "fr/s fr/ee", "fr/r fr/ae"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "courants",
        syllables: &["cou", "rant", "an", "an"],
        hints: &["fr/k fr/ou", "fr/r fr/en", "fr/en", "fr/en"],
        orphan_end_slots: &[1],
    },
    Layout {
        word: "courants",
        syllables: &["cou", "rants", "an", "an"],
        hints: &["fr/k fr/ou", "fr/r fr/en", "fr/en", "fr/en"],
        orphan_end_slots: &[1],
    },
    Layout {
        word: "partir",
        syllables: &["par", "ti", "i", "ir"],
        hints: &["fr/p fr/ah fr/r", "fr/t fr/ih", "fr/ih", "fr/ih fr/r"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "espace",
        syllables: &["es", "pa", "a", "ace"],
        hints: &["fr/ae fr/s", "fr/p fr/ah", "fr/ah", "fr/ah fr/s"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "amours",
        syllables: &["a", "mou", "ou", "ours"],
        hints: &["fr/ah", "fr/m fr/ou", "fr/ou", "fr/ou fr/r"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "amours",
        syllables: &["a", "mou", "ou", "our"],
        hints: &["fr/ah", "fr/m fr/ou", "fr/ou", "fr/ou fr/r"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "m'arrête",
        syllables: &["m'ar", "rê", "te"],
        hints: &["fr/m fr/ah", "fr/r fr/ae", "fr/t fr/ee"],
        orphan_end_slots: &[1],
    },
    Layout {
        word: "murs",
        syllables: &["mu", "u", "urs"],
        hints: &["fr/m fr/uh", "fr/uh", "fr/uh fr/r"],
        orphan_end_slots: &[0],
    },
    Layout {
        word: "mur",
        syllables: &["mu", "u", "ur"],
        hints: &["fr/m fr/uh", "fr/uh", "fr/uh fr/r"],
        orphan_end_slots: &[0],
    },
    Layout {
        word: "rêves",
        syllables: &["rê", "ê", "ves"],
        hints: &["fr/r fr/ae", "fr/ae", "fr/v fr/ee"],
        orphan_end_slots: &[0],
    },
    Layout {
        word: "rêves",
        syllables: &["rêves", "ê", "ves"],
        hints: &["fr/r fr/ae", "fr/ae", "fr/v fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "rêve",
        syllables: &["rê", "ê", "ve"],
        hints: &["fr/r fr/ae", "fr/ae", "fr/v fr/ee"],
        orphan_end_slots: &[0],
    },
    Layout {
        word: "vent",
        syllables: &["vent", "en", "ent"],
        hints: &["fr/v fr/en", "fr/en", "fr/en"],
        orphan_end_slots: &[0],
    },
    Layout {
        word: "court",
        syllables: &["court", "ou", "ourt"],
        hints: &["fr/k fr/ou", "fr/ou", "fr/ou fr/r"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "fond",
        syllables: &["fond", "on", "on"],
        hints: &["fr/f fr/on", "fr/on", "fr/on"],
        orphan_end_slots: &[0, 1],
    },
    Layout {
        word: "tempêtes",
        syllables: &["tem", "pê", "tes"],
        hints: &["fr/t fr/en", "fr/p fr/ae", "fr/t fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "tempête",
        syllables: &["tem", "pê", "te"],
        hints: &["fr/t fr/en", "fr/p fr/ae", "fr/t fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "blessures",
        syllables: &["bles", "su", "res"],
        hints: &["fr/b fr/l fr/ae", "fr/s fr/uh", "fr/r fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "raison s'achève",
        syllables: &["rai", "sons'a", "chève"],
        hints: &["fr/r fr/ae", "fr/z fr/on fr/s fr/ah", "fr/sh fr/ae fr/v"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "garderai",
        syllables: &["gar", "de", "rai"],
        hints: &["fr/g fr/ah fr/r", "fr/d fr/ee", "fr/r fr/ae"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "trompettes",
        syllables: &["trom", "pet", "te"],
        hints: &["fr/t fr/r fr/on", "fr/p fr/ae", "fr/t fr/ee"],
        orphan_end_slots: &[1],
    },
    Layout {
        word: "trompettes",
        syllables: &["trom", "pet", "tes"],
        hints: &["fr/t fr/r fr/on", "fr/p fr/ae", "fr/t fr/ee"],
        orphan_end_slots: &[1],
    },
    Layout {
        word: "presse",
        syllables: &["pre", "sse", "e"],
        hints: &["fr/p fr/r fr/ae", "fr/s fr/ee", "fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "trace",
        syllables: &["tra", "a", "ce"],
        hints: &["fr/t fr/r fr/ah", "fr/ah", "fr/s fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "l'exil",
        syllables: &["l'e", "xi", "il"],
        hints: &["fr/l fr/ae", "fr/g fr/z fr/ih", "fr/ih fr/l"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "l'empreinte",
        syllables: &["l'em", "prein", "te"],
        hints: &["fr/l fr/en", "fr/p fr/r fr/in", "fr/t fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "blessure",
        syllables: &["bles", "su", "re"],
        hints: &["fr/b fr/l fr/ae", "fr/s fr/uh", "fr/r fr/ee"],
        orphan_end_slots: &[1],
    },
    Layout {
        word: "jours",
        syllables: &["jours", "ou", "our"],
        hints: &["fr/j fr/ou", "fr/ou", "fr/ou fr/r"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "jours",
        syllables: &["jours", "ou", "ours"],
        hints: &["fr/j fr/ou", "fr/ou", "fr/ou fr/r"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "espace",
        syllables: &["es", "pa", "ace"],
        hints: &["fr/ae fr/s", "fr/p fr/ah", "fr/ah fr/s"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "",
        syllables: &["au", "bout", "de"],
        hints: &["fr/oh", "fr/b fr/ou", "fr/d fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "m'arrête",
        syllables: &["m'ar", "rête"],
        hints: &["fr/m fr/ah", "fr/r fr/ae fr/t"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "jure",
        syllables: &["ju", "ure"],
        hints: &["fr/j fr/uh", "fr/uh fr/r"],
        orphan_end_slots: &[0],
    },
    Layout {
        word: "changer",
        syllables: &["chan", "ger"],
        hints: &["fr/sh fr/en", "fr/j fr/eh"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "même",
        syllables: &["mê", "me"],
        hints: &["fr/m fr/ae", "fr/m fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "presse",
        syllables: &["pres", "se"],
        hints: &["fr/p fr/r fr/ae", "fr/s fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "rêves",
        syllables: &["rê", "ves"],
        hints: &["fr/r fr/ae", "fr/v fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "rêve",
        syllables: &["rê", "ve"],
        hints: &["fr/r fr/ae", "fr/v fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "blessures",
        syllables: &["bles", "sures"],
        hints: &["fr/b fr/l fr/ae", "fr/s fr/uh fr/r"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "blessure",
        syllables: &["bles", "sure"],
        hints: &["fr/b fr/l fr/ae", "fr/s fr/uh fr/r"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "amours",
        syllables: &["a", "mours"],
        hints: &["fr/ah", "fr/m fr/ou fr/r"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "laisses",
        syllables: &["lais", "ses"],
        hints: &["fr/l fr/ae", "fr/s fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "genoux",
        syllables: &["ge", "noux"],
        hints: &["fr/j fr/ee", "fr/n fr/ou"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "ferons",
        syllables: &["fe", "rons"],
        hints: &["fr/f fr/ee", "fr/r fr/on"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "moments",
        syllables: &["mo", "ments"],
        hints: &["fr/m fr/oo", "fr/m fr/en"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "j'irai",
        syllables: &["j'i", "rai"],
        hints: &["fr/j fr/ih", "fr/r fr/ae"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "raison",
        syllables: &["rai", "son"],
        hints: &["fr/r fr/ae", "fr/z fr/on"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "s'achève",
        syllables: &["s'a", "chève"],
        hints: &["fr/s fr/ah", "fr/sh fr/ae fr/v"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "garde",
        syllables: &["gar", "de"],
        hints: &["fr/g fr/ah fr/r", "fr/d fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "force",
        syllables: &["for", "ce"],
        hints: &["fr/f fr/oo fr/r", "fr/s fr/ee"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "tempête",
        syllables: &["tem", "pête"],
        hints: &["fr/t fr/en", "fr/p fr/ae fr/t"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "courant",
        syllables: &["cou", "rant"],
        hints: &["fr/k fr/ou", "fr/r fr/en"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "plier",
        syllables: &["pli", "er"],
        hints: &["fr/p fr/l fr/ih", "fr/y fr/eh"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "années",
        syllables: &["an", "nées"],
        hints: &["fr/ah", "fr/n fr/eh"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "laisse",
        syllables: &["lais", "se"],
        hints: &["fr/l fr/ae", "fr/s fr/ee"],
        orphan_end_slots: &[0],
    },
    Layout {
        word: "minutes",
        syllables: &["mi", "nutes"],
        hints: &["fr/m fr/ih", "fr/n fr/uh fr/t"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "soufflant",
        syllables: &["souf", "flant"],
        hints: &["fr/s fr/ou", "fr/f fr/l fr/en"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "courber",
        syllables: &["cour", "ber"],
        hints: &["fr/k fr/ou fr/r", "fr/b fr/eh"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "tête",
        syllables: &["tê", "te"],
        hints: &["fr/t fr/ae", "fr/t fr/ee"],
        orphan_end_slots: &[0],
    },
    Layout {
        word: "teste",
        syllables: &["tes", "te"],
        hints: &["fr/t fr/ae fr/s", "fr/t fr/ee"],
        orphan_end_slots: &[0],
    },
    Layout {
        word: "briser",
        syllables: &["bri", "ser"],
        hints: &["fr/b fr/r fr/ih", "fr/z fr/eh"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "chercher",
        syllables: &["cher", "cher"],
        hints: &["fr/sh fr/ae fr/r", "fr/sh fr/eh"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "genou",
        syllables: &["ge", "nou"],
        hints: &["fr/j fr/ee", "fr/n fr/ou"],
        orphan_end_slots: &[],
    },
    Layout {
        word: "dessus",
        syllables: &["des", "sus"],
        hints: &["fr/d fr/ee", "fr/s fr/uh"],
        orphan_end_slots: &[],
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
    let key = shared::candidate(lyric)?;
    let source = source(lyric)?;
    // A prefix is notation only when the explicit verse proves its number.
    // Literal dictionary variants such as word(2) never enter this branch.
    if let Some((number, remainder)) = key.split_once('.') {
        if source.verse_from_score && number == source.verse.to_string() && !remainder.is_empty() {
            return Some(normalize(remainder));
        }
    }
    Some(key)
}

fn same_lane(left: &ProjectedLyric, right: &ProjectedLyric) -> bool {
    match (source(left), source(right)) {
        (Some(left), Some(right)) => left.lane == right.lane && left.verse == right.verse,
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

fn same_lane_or_hold(left: &ProjectedLyric, right: &ProjectedLyric) -> bool {
    matches!(right, ProjectedLyric::Extension) || same_lane(left, right)
}

fn ends_word(lyric: &ProjectedLyric) -> bool {
    source(lyric).is_some_and(|s| !matches!(s.syllabic, Some(Syllabic::Begin | Syllabic::Middle)))
        && !text(lyric).is_some_and(|s| s.trim_end().ends_with('-'))
}

fn layout_members(notes: &[ProjectedNote], head: usize, layout: &Layout) -> Option<Vec<usize>> {
    let mut members = Vec::new();
    let mut index = head;
    let mut crossed_gap = false;
    for (slot, expected) in layout.syllables.iter().enumerate() {
        if slot > 0 {
            loop {
                let next = index + 1;
                let note = notes.get(next)?;
                let end =
                    u64::from(notes[index].onset_ticks) + u64::from(notes[index].duration_ticks);
                let onset = u64::from(note.onset_ticks);
                if end > onset || !same_lane_or_hold(&notes[head].lyric, &note.lyric) {
                    return None;
                }
                let held = matches!(&note.lyric, ProjectedLyric::Extension)
                    || matches!(&note.lyric, ProjectedLyric::Source(s) if s.state == LyricState::Continuation);
                // A true hold must touch its predecessor; only the next text
                // attack may follow a rest inside a completely bound word.
                if held && end != onset {
                    return None;
                }
                crossed_gap |= end < onset;
                index = next;
                if !held {
                    break;
                }
            }
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
            || (slot + 1 == layout.syllables.len() && !ends_word(&notes[index].lyric))
            || (slot + 1 < layout.syllables.len()
                && ((syllabic == Some(Syllabic::End) && !layout.orphan_end_slots.contains(&slot))
                    || ends_phrase(text(&notes[index].lyric)?)))
        {
            return None;
        }
        members.push(index);
    }
    // A rest needs a complete written word, not only an exact surface echo.
    // Emit independent hints below; native + must never bridge the silence.
    if crossed_gap
        && (layout.word.is_empty()
            || layout.word.contains(' ')
            || members.iter().enumerate().any(|(slot, &member)| {
                source(&notes[member].lyric).map(|s| &s.syllabic)
                    != Some(&Some(if slot == 0 {
                        Syllabic::Begin
                    } else if slot + 1 == members.len() {
                        Syllabic::End
                    } else {
                        Syllabic::Middle
                    }))
            }))
    {
        return None;
    }
    Some(members)
}

/// Exact independent readings for a caller that has proved source provenance.
/// Returned groups let it require that a layout actually spans polyphonic members.
pub(crate) fn contextual_readings(notes: &[ProjectedNote]) -> Vec<Vec<(usize, String)>> {
    // Evaluate the proven source chronology before extracting hints, retaining
    // complete word identity for liaison on split polyphonic words.
    let mut pronounced = notes.to_vec();
    apply(&mut pronounced, &vec![String::new(); notes.len()]);
    let mut claimed = vec![false; notes.len()];
    let mut result = Vec::new();
    for head in 0..notes.len() {
        for layout in LAYOUTS {
            let Some(members) = layout_members(notes, head, layout) else {
                continue;
            };
            if members.iter().any(|&member| claimed[member]) {
                continue;
            }
            for &member in &members {
                claimed[member] = true;
            }
            result.push(
                members
                    .into_iter()
                    .map(|index| {
                        let ProjectedLyric::Pronounced { phonemes, .. } = &pronounced[index].lyric
                        else {
                            unreachable!("matched independent layout")
                        };
                        (index, phonemes.clone())
                    })
                    .collect(),
            );
            break;
        }
    }
    result
}

pub(crate) fn apply_contextual_reading(
    note: &mut ProjectedNote,
    hint: &str,
    id: &str,
) -> Diagnostic {
    pronounce(note, hint);
    diagnose(APPLIED, format!(
        "French Millefeuille: matching written lyric provenance across polyphonic members supplies [{hint}]; original lyric ownership and note geometry are preserved."
    ), id)
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
        text: candidate(&note.lyric).unwrap_or_else(|| normalize(text)),
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
        if candidate(&notes[head].lyric).as_deref() != Some("laisse")
            || !text(&notes[head].lyric).is_some_and(ends_phrase)
        {
            continue;
        }
        let Some(tail) = next_attack(notes, head) else {
            continue;
        };
        if candidate(&notes[tail].lyric).as_deref() == Some("se")
            && same_lane(&notes[head].lyric, &notes[tail].lyric)
            && ends_word(&notes[tail].lyric)
        {
            // This is an independent written schwa attack after punctuation.
            // Keep the preceding word's full consonant; never redistribute it.
            pronounce(&mut notes[tail], "fr/s fr/ee");
            changed[tail] = true;
        }
    }
    for head in 0..notes.len() {
        for layout in LAYOUTS {
            let Some(members) = layout_members(notes, head, layout) else {
                continue;
            };
            if layout.word.is_empty() {
                words.extend(
                    members
                        .iter()
                        .zip(layout.syllables)
                        .map(|(&member, word)| (member, member, (*word).into())),
                );
            } else {
                words.push((head, *members.last().unwrap(), layout.word.into()));
            }
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
        if !members
            .iter()
            .all(|&member| same_lane(&notes[members[0]].lyric, &notes[member].lyric))
        {
            continue;
        }
        let Some(parts) = members
            .iter()
            .map(|&i| candidate(&notes[i].lyric))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let key = parts.concat();
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
