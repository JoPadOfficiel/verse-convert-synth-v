//! Offline automatic French/English routing for sung lyrics.
//!
//! Language is pronunciation policy, never source evidence. The router works on
//! complete source-attested words, combines Lingua's local statistical model
//! with Verse's pinned French/English lexicons, and decodes one deterministic
//! language sequence so short homographs do not flip a phrase on their own.

use std::collections::HashMap;
use std::sync::OnceLock;

use lingua::{Language, LanguageDetector, LanguageDetectorBuilder};

use crate::engine::convert::{Diagnostic, DiagnosticSeverity};
use crate::engine::projection::{ProjectedNote, ProjectedTrack, PronunciationLanguage};
use crate::engine::syllable::touches;
use crate::engine::target::{english, french, lexical};

pub const ROUTED: &str = "AUTOMATIC_LANGUAGE_ROUTED";
pub const LOW_CONFIDENCE: &str = "AUTOMATIC_LANGUAGE_LOW_CONFIDENCE";

impl PronunciationLanguage {
    fn label(self) -> &'static str {
        match self {
            Self::French => "French",
            Self::English => "English",
        }
    }
}

#[derive(Clone, Debug)]
struct Unit {
    members: Vec<usize>,
    key: String,
    source_id: String,
    boundary_after: bool,
    french_score: f64,
    english_score: f64,
    local_uncertain: bool,
    inherit_passage: bool,
}

#[derive(Clone, Debug)]
pub struct Route {
    pub languages: Vec<Option<PronunciationLanguage>>,
    pub diagnostics: Vec<Diagnostic>,
    inherit_passage: Vec<bool>,
}

fn detector() -> &'static LanguageDetector {
    static DETECTOR: OnceLock<LanguageDetector> = OnceLock::new();
    DETECTOR.get_or_init(|| {
        LanguageDetectorBuilder::from_languages(&[Language::English, Language::French])
            .with_preloaded_language_models()
            .build()
    })
}

fn lingua_scores(text: &str) -> (f64, f64) {
    let values = detector().compute_language_confidence_values(text.to_owned());
    let mut french = 0.5;
    let mut english = 0.5;
    for (language, value) in values {
        match language {
            Language::French => french = value,
            Language::English => english = value,
        }
    }
    (french, english)
}

fn log_odds(left: f64, right: f64) -> f64 {
    const EPS: f64 = 1.0e-9;
    ((left + EPS) / (right + EPS)).ln().clamp(-4.0, 4.0)
}

fn function_word_anchor(key: &str) -> Option<PronunciationLanguage> {
    if matches!(
        key,
        "je" | "tu"
            | "il"
            | "elle"
            | "nous"
            | "vous"
            | "ils"
            | "elles"
            | "le"
            | "les"
            | "des"
            | "du"
            | "de"
            | "et"
            | "est"
            | "une"
            | "un"
            | "au"
            | "aux"
            | "que"
            | "qui"
            | "dans"
            | "mais"
            | "pour"
            | "mon"
            | "ma"
            | "mes"
            | "ton"
            | "ta"
            | "tes"
            | "ce"
            | "cette"
            | "ces"
            | "ça"
            | "pas"
    ) {
        return Some(PronunciationLanguage::French);
    }
    if matches!(
        key,
        "the"
            | "and"
            | "of"
            | "to"
            | "my"
            | "your"
            | "his"
            | "her"
            | "our"
            | "their"
            | "with"
            | "for"
            | "from"
            | "this"
            | "that"
            | "these"
            | "those"
            | "you"
            | "i"
            | "i'm"
            | "i've"
            | "it's"
            | "is"
            | "are"
            | "was"
            | "were"
            | "been"
            | "be"
            | "can't"
            | "no"
            | "sir"
            | "yeah"
    ) {
        return Some(PronunciationLanguage::English);
    }
    None
}

fn ends_phrase(note: &ProjectedNote) -> bool {
    let Some(source) = lexical::source(&note.lyric) else {
        return false;
    };
    if source.line_break.is_some() {
        return true;
    }
    lexical::raw_text(&note.lyric).is_some_and(|text| {
        text.trim_end_matches(|c: char| c.is_whitespace() || "'’\"»”)]}".contains(c))
            .ends_with([',', '.', ';', ':', '!', '?', '…'])
    })
}

fn starts_capitalized(note: &ProjectedNote) -> bool {
    lexical::raw_text(&note.lyric)
        .and_then(|text| text.chars().find(|character| character.is_alphabetic()))
        .is_some_and(|character| character.is_uppercase())
}

fn carries_manual_hint(note: &ProjectedNote) -> bool {
    lexical::raw_text(&note.lyric)
        .is_some_and(|text| text.contains(['[', ']']) || text.trim_start().starts_with(['+', '?']))
}

fn build_units(notes: &[ProjectedNote], note_ids: &[String]) -> Vec<Unit> {
    let mut ownership: Vec<Option<usize>> = vec![None; notes.len()];
    let fragments = lexical::fragments(notes);
    let declared_fragments: Vec<bool> = notes
        .iter()
        .enumerate()
        .map(|(index, note)| {
            fragments[index]
                || lexical::source(&note.lyric).is_some_and(|source| {
                    matches!(
                        source.syllabic,
                        Some(
                            crate::engine::midi::Syllabic::Begin
                                | crate::engine::midi::Syllabic::Middle
                                | crate::engine::midi::Syllabic::End
                        )
                    )
                })
        })
        .collect();
    let mut groups = lexical::automatic_words(notes);
    groups.sort_by_key(|members| members.first().copied().unwrap_or(usize::MAX));

    let mut raw_units: Vec<(Vec<usize>, String)> = Vec::new();
    for members in groups {
        if members.is_empty() || members.iter().any(|&index| ownership[index].is_some()) {
            continue;
        }
        let key = lexical::preferred_joined_key(notes, &members, |candidate| {
            french::contains_lexeme(candidate) || english::contains_lexeme(candidate)
        });
        if key.is_empty() {
            continue;
        }
        let unit_index = raw_units.len();
        for &member in &members {
            ownership[member] = Some(unit_index);
        }
        raw_units.push((members, key));
    }

    // Real scores sometimes mark every syllable of a word as Middle (the
    // private corpus contains this for repeated sung titles). The strict word
    // parser correctly refuses to invent a Begin/End pair, but automatic
    // routing may still recover a complete word when a contiguous fragment run
    // in one source lyric lane has a dictionary-attested concatenation. This is
    // lookup-only recovery: source lyrics, timings and syllabic markers remain
    // untouched.
    for head in 0..notes.len() {
        if ownership[head].is_some()
            || !declared_fragments[head]
            || lexical::candidate(&notes[head].lyric).is_none()
        {
            continue;
        }
        let Some(head_source) = lexical::source(&notes[head].lyric) else {
            continue;
        };
        let belongs_to_previous_fragment = head > 0
            && ownership[head - 1].is_none()
            && declared_fragments[head - 1]
            && touches(&notes[head - 1], &notes[head])
            && lexical::source(&notes[head - 1].lyric)
                .is_some_and(|source| source.lane == head_source.lane);
        if belongs_to_previous_fragment {
            continue;
        }

        let mut members = vec![head];
        let mut best: Option<(Vec<usize>, String)> = None;
        let mut next = head + 1;
        while next < notes.len() && members.len() < 8 {
            if ownership[next].is_some()
                || !declared_fragments[next]
                || lexical::candidate(&notes[next].lyric).is_none()
                || !touches(&notes[next - 1], &notes[next])
                || lexical::source(&notes[next].lyric)
                    .is_none_or(|source| source.lane != head_source.lane)
            {
                break;
            }
            members.push(next);
            let key = lexical::preferred_joined_key(notes, &members, |candidate| {
                french::contains_lexeme(candidate) || english::contains_lexeme(candidate)
            });
            if french::contains_lexeme(&key) || english::contains_lexeme(&key) {
                best = Some((members.clone(), key));
            }
            next += 1;
        }
        let run_continues = next < notes.len()
            && ownership[next].is_none()
            && declared_fragments[next]
            && lexical::candidate(&notes[next].lyric).is_some()
            && touches(&notes[next - 1], &notes[next])
            && lexical::source(&notes[next].lyric)
                .is_some_and(|source| source.lane == head_source.lane);
        if run_continues {
            continue;
        }
        let Some((best_members, key)) = best else {
            continue;
        };
        // A dictionary word found only in the prefix of a longer malformed
        // fragment run is not enough evidence to split that run into two
        // invented words. Recovery is allowed only when the dictionary-backed
        // candidate consumes the complete contiguous fragment run.
        if best_members.len() != members.len() {
            continue;
        }
        let unit_index = raw_units.len();
        for &member in &best_members {
            ownership[member] = Some(unit_index);
        }
        raw_units.push((best_members, key));
    }

    for index in 0..notes.len() {
        if ownership[index].is_some() {
            continue;
        }
        // An incomplete Begin/Middle/End or dash-bound syllable is never scored
        // as an independent word. It may inherit the owning passage below.
        if fragments[index]
            || lexical::source(&notes[index].lyric).is_some_and(|source| {
                !matches!(
                    source.syllabic,
                    None | Some(crate::engine::midi::Syllabic::Single)
                )
            })
        {
            continue;
        }
        let Some(key) = lexical::candidate(&notes[index].lyric) else {
            continue;
        };
        let unit_index = raw_units.len();
        ownership[index] = Some(unit_index);
        raw_units.push((vec![index], key));
    }
    raw_units.sort_by_key(|(members, _)| members[0]);

    let keys: Vec<_> = raw_units.iter().map(|(_, key)| key.clone()).collect();
    let capitalized_units: Vec<_> = raw_units
        .iter()
        .map(|(members, _)| starts_capitalized(&notes[members[0]]))
        .collect();
    let boundaries: Vec<bool> = raw_units
        .iter()
        .enumerate()
        .map(|(position, (members, _))| {
            let tail = *members.last().unwrap();
            ends_phrase(&notes[tail])
                || raw_units
                    .get(position + 1)
                    .is_some_and(|(next, _)| !touches(&notes[tail], &notes[next[0]]))
        })
        .collect();
    let mut score_cache: HashMap<String, (f64, f64)> = HashMap::new();
    let mut units = Vec::with_capacity(raw_units.len());
    for (position, (members, key)) in raw_units.into_iter().enumerate() {
        let head = members[0];
        let self_text = key.clone();
        let phrase_start = (0..position)
            .rev()
            .find(|&index| boundaries[index])
            .map_or(0, |index| index + 1);
        let phrase_end = (position..boundaries.len())
            .find(|&index| boundaries[index])
            .map_or(keys.len(), |index| index + 1);
        let context_start = position.saturating_sub(2).max(phrase_start);
        let context_end = (position + 3).min(phrase_end);
        let context = keys[context_start..context_end].join(" ");
        let phrase = keys[phrase_start..phrase_end].join(" ");
        let pair_candidates = [
            (position > phrase_start)
                .then(|| (keys[position - 1..=position].join(" "), &keys[position - 1])),
            (position + 1 < phrase_end)
                .then(|| (keys[position..=position + 1].join(" "), &keys[position + 1])),
        ];
        let has_function_anchor_neighbor = (position > phrase_start
            && function_word_anchor(&keys[position - 1]).is_some())
            || (position + 1 < phrase_end && function_word_anchor(&keys[position + 1]).is_some());
        let has_capitalized_neighbor = (position > phrase_start && capitalized_units[position - 1])
            || (position + 1 < phrase_end && capitalized_units[position + 1]);
        let neutral_vocalise = matches!(
            self_text.as_str(),
            "ah" | "aah" | "oh" | "ooh" | "ouh" | "hou" | "uh" | "mm" | "mmm" | "la" | "na"
        );
        let (self_fr, self_en) = *score_cache
            .entry(self_text.clone())
            .or_insert_with(|| lingua_scores(&self_text));
        let (context_fr, context_en) = *score_cache
            .entry(context.clone())
            .or_insert_with(|| lingua_scores(&context));
        let (phrase_fr, phrase_en) = *score_cache
            .entry(phrase.clone())
            .or_insert_with(|| lingua_scores(&phrase));
        let fr_lex = french::contains_lexeme(&key);
        let en_lex = english::contains_lexeme(&key);
        let shared = fr_lex && en_lex;
        let shared_or_unknown = fr_lex == en_lex;
        let unknown = !fr_lex && !en_lex;
        let unit_anchor = function_word_anchor(&key).or_else(|| {
            (unknown && members.len() > 1)
                .then(|| lexical::candidate(&notes[head].lyric))
                .flatten()
                .as_deref()
                .and_then(function_word_anchor)
        });
        let capitalized = capitalized_units[position];
        let local_log_odds = log_odds(self_fr, self_en);
        let chars = self_text.chars().count();
        let short_unknown = unknown && chars <= 4;
        let local_uncertain = neutral_vocalise
            || unknown
            || local_log_odds.abs() < 0.8
            || (shared_or_unknown && local_log_odds.abs() < 1.5);
        let self_weight = if neutral_vocalise || short_unknown {
            0.0
        } else if shared && chars >= 4 {
            1.5
        } else if local_uncertain {
            0.35
        } else if chars >= 5 {
            0.9
        } else if chars >= 3 {
            0.65
        } else {
            0.25
        };
        let context_weight = if short_unknown {
            0.0
        } else if shared && chars >= 4 {
            0.2
        } else if local_uncertain {
            1.0
        } else {
            0.45
        };
        let phrase_weight = if short_unknown {
            0.0
        } else if shared && chars >= 4 {
            0.1
        } else if local_uncertain {
            1.0
        } else {
            0.35
        };
        let self_is_french = self_fr >= self_en;
        let mut pair_scores: Vec<(bool, f64)> = pair_candidates
            .into_iter()
            .flatten()
            .map(|(pair, neighbor)| {
                let (pair_fr, pair_en) = *score_cache
                    .entry(pair.clone())
                    .or_insert_with(|| lingua_scores(&pair));
                let anchored = function_word_anchor(neighbor).is_some();
                let (neighbor_fr, neighbor_en) = *score_cache
                    .entry(neighbor.clone())
                    .or_insert_with(|| lingua_scores(neighbor));
                let same_isolated_side = (neighbor_fr >= neighbor_en) == self_is_french;
                (anchored || same_isolated_side, log_odds(pair_fr, pair_en))
            })
            .collect();
        let has_preferred_pair = pair_scores.iter().any(|(preferred, _)| *preferred);
        let pair_log_odds = pair_scores
            .drain(..)
            .filter(|(preferred, _)| {
                if shared && !has_preferred_pair {
                    false
                } else {
                    !has_preferred_pair || *preferred
                }
            })
            .map(|(_, score)| score)
            .max_by(|left, right| {
                left.abs()
                    .partial_cmp(&right.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0.0);
        let pair_weight = if short_unknown {
            0.0
        } else if unknown {
            0.25
        } else if shared && capitalized && has_capitalized_neighbor {
            2.5
        } else if shared && has_function_anchor_neighbor && (self_fr - self_en).abs() < 0.5 {
            2.0
        } else if shared {
            0.8
        } else if local_uncertain {
            0.7
        } else {
            0.55
        };
        let mut french_score = self_weight * local_log_odds
            + context_weight * log_odds(context_fr, context_en)
            + phrase_weight * log_odds(phrase_fr, phrase_en)
            + pair_weight * pair_log_odds;
        let mut english_score = -french_score;

        match (fr_lex, en_lex, neutral_vocalise) {
            (_, _, true) => {}
            (true, false, false) => french_score += 1.5,
            (false, true, false) => english_score += 1.5,
            _ => {}
        }
        match unit_anchor {
            Some(PronunciationLanguage::French) => {
                french_score = 4.5;
                english_score = -4.5;
            }
            Some(PronunciationLanguage::English) => {
                french_score = -4.5;
                english_score = 4.5;
            }
            None => {}
        }
        if shared && !capitalized && unit_anchor.is_none() {
            let previous_anchor = (position > phrase_start && !boundaries[position - 1])
                .then(|| function_word_anchor(&keys[position - 1]))
                .flatten();
            let next_anchor = (position + 1 < phrase_end && !boundaries[position])
                .then(|| function_word_anchor(&keys[position + 1]))
                .flatten();
            let next_supports_local_compound = position + 1 < phrase_end
                && function_word_anchor(&keys[position + 1]).is_none()
                && french::contains_lexeme(&keys[position + 1])
                && english::contains_lexeme(&keys[position + 1])
                && {
                    let (next_fr, next_en) = *score_cache
                        .entry(keys[position + 1].clone())
                        .or_insert_with(|| lingua_scores(&keys[position + 1]));
                    (next_fr >= next_en) == self_is_french
                        && local_log_odds.abs() >= 0.5
                        && log_odds(next_fr, next_en).abs() >= 0.5
                };
            if !next_supports_local_compound {
                if let Some(anchor) = previous_anchor.or(next_anchor) {
                    match anchor {
                        PronunciationLanguage::French => {
                            french_score = 3.5;
                            english_score = -3.5;
                        }
                        PronunciationLanguage::English => {
                            french_score = -3.5;
                            english_score = 3.5;
                        }
                    }
                }
            }
        }
        // A capitalized shared title immediately followed by an unambiguous
        // English discourse/function marker (e.g. a quoted title followed by
        // "yeah") belongs to that English phrase. This is evaluated after the
        // isolated-capitalization prior below so phrase evidence can win.
        let capitalized_next_english = capitalized
            && shared
            && position + 1 < phrase_end
            && !boundaries[position]
            && function_word_anchor(&keys[position + 1]) == Some(PronunciationLanguage::English);
        let context_dependent = unknown || (shared && (self_fr - self_en).abs() < 0.5);
        if unit_anchor.is_none() && context_dependent && position > 0 {
            if let Some(anchor) = function_word_anchor(&keys[position - 1]) {
                // A preceding function word is unusually strong evidence for a
                // weak/unknown content spelling inside one sung passage. Unknown
                // vocalises may inherit it even across a performed gap; shared
                // dictionary words require an unbroken phrase.
                if unknown || !boundaries[position - 1] {
                    match anchor {
                        PronunciationLanguage::French => {
                            french_score = 3.5;
                            english_score = -3.5;
                        }
                        PronunciationLanguage::English => {
                            french_score = -3.5;
                            english_score = 3.5;
                        }
                    }
                }
            }
        }
        // Mid-phrase capitalization is useful source evidence for names and
        // titles. When the lexicons do not uniquely identify the spelling,
        // trust the isolated two-language model only if it still shows a real
        // margin. This keeps French function words around an English proper
        // name from swallowing the name while avoiding a song-specific list.
        if capitalized
            && position > phrase_start
            && shared
            && unit_anchor.is_none()
            && (self_fr - self_en).abs() >= 0.1
        {
            let anchor = 4.0 + log_odds(self_fr, self_en).abs().min(2.0);
            if self_fr > self_en {
                french_score = anchor;
                english_score = -anchor;
            } else {
                french_score = -anchor;
                english_score = anchor;
            }
        }
        if capitalized
            && shared
            && unit_anchor.is_none()
            && position > phrase_start
            && capitalized_units[position - 1]
            && local_log_odds.abs() < 0.8
        {
            let (previous_fr, previous_en) = *score_cache
                .entry(keys[position - 1].clone())
                .or_insert_with(|| lingua_scores(&keys[position - 1]));
            let previous_odds = log_odds(previous_fr, previous_en);
            if previous_odds.abs() >= 0.8 {
                if previous_odds > 0.0 {
                    french_score = 3.5;
                    english_score = -3.5;
                } else {
                    french_score = -3.5;
                    english_score = 3.5;
                }
            }
        }
        if capitalized_next_english {
            french_score = -3.5;
            english_score = 3.5;
        }

        units.push(Unit {
            members,
            key,
            source_id: note_ids[head].clone(),
            boundary_after: boundaries[position],
            french_score,
            english_score,
            local_uncertain,
            inherit_passage: (neutral_vocalise || short_unknown)
                && function_word_anchor(&self_text).is_none(),
        });
    }
    units
}

fn transition_penalty(previous: &Unit) -> f64 {
    if previous.boundary_after {
        0.25
    } else {
        0.5
    }
}

fn decode(units: &[Unit]) -> Vec<PronunciationLanguage> {
    if units.is_empty() {
        return Vec::new();
    }
    let mut dp = vec![[f64::NEG_INFINITY; 2]; units.len()];
    let mut back = vec![[0usize; 2]; units.len()];
    dp[0] = [units[0].french_score, units[0].english_score];
    for index in 1..units.len() {
        let penalty = transition_penalty(&units[index - 1]);
        for state in 0..2 {
            let emission = if state == 0 {
                units[index].french_score
            } else {
                units[index].english_score
            };
            let same = dp[index - 1][state];
            let other = dp[index - 1][1 - state] - penalty;
            if same >= other {
                dp[index][state] = same + emission;
                back[index][state] = state;
            } else {
                dp[index][state] = other + emission;
                back[index][state] = 1 - state;
            }
        }
    }
    let mut state = usize::from(dp.last().unwrap()[1] > dp.last().unwrap()[0]);
    let mut states = vec![PronunciationLanguage::French; units.len()];
    for index in (0..units.len()).rev() {
        states[index] = if state == 0 {
            PronunciationLanguage::French
        } else {
            PronunciationLanguage::English
        };
        if index > 0 {
            state = back[index][state];
        }
    }
    // Neutral vocalises and very short unknown spellings are not reliable
    // language evidence. They inherit the nearest established passage, with a
    // previous sung passage preferred across a rest or punctuation boundary.
    // This keeps e.g. an onomatopoeic tail attached to "I can't get no ..."
    // while letting the same spelling follow a preceding French passage.
    for index in 0..states.len() {
        if !units[index].inherit_passage {
            continue;
        }
        if let Some(previous) = (0..index)
            .rev()
            .find(|candidate| !units[*candidate].inherit_passage)
        {
            states[index] = states[previous];
        } else if let Some(next) =
            ((index + 1)..states.len()).find(|candidate| !units[*candidate].inherit_passage)
        {
            states[index] = states[next];
        }
    }
    states
}

/// Route one already-isolated source voice/repeat domain. No source lyric or
/// geometry is changed here. Continuations inherit their proven predecessor's
/// language and untexted notes remain neutral.
pub fn route(notes: &[ProjectedNote], note_ids: &[String]) -> Route {
    assert_eq!(notes.len(), note_ids.len());
    let units = build_units(notes, note_ids);
    let decisions = decode(&units);
    let mut languages = vec![None; notes.len()];
    let mut inherit_passage = vec![false; notes.len()];
    let mut diagnostics = Vec::new();
    let mut french_count = 0usize;
    let mut english_count = 0usize;

    for (unit, language) in units.iter().zip(&decisions) {
        for &member in &unit.members {
            languages[member] = Some(*language);
            inherit_passage[member] = unit.inherit_passage;
        }
        match language {
            PronunciationLanguage::French => french_count += 1,
            PronunciationLanguage::English => english_count += 1,
        }
        let margin = (unit.french_score - unit.english_score).abs();
        if unit.local_uncertain || margin < 1.0 {
            diagnostics.push(Diagnostic {
                code: LOW_CONFIDENCE.into(),
                severity: DiagnosticSeverity::Info,
                message: format!(
                    "Automatic FR+EN: {:?} resolves to {} from surrounding phrase context with a low score margin ({margin:.2}); export continues deterministically.",
                    unit.key,
                    language.label(),
                ),
                source_id: Some(unit.source_id.clone()),
            });
        }
    }

    for index in 1..notes.len() {
        if languages[index].is_none()
            && notes[index].lyric.continues_previous_note()
            && touches(&notes[index - 1], &notes[index])
        {
            languages[index] = languages[index - 1];
            inherit_passage[index] = inherit_passage[index - 1];
        }
    }

    // Incomplete source fragments are not independent classification units, but
    // a sung fragment still belongs to a passage. Inherit the nearest routed
    // word in this already-isolated voice/repeat domain without crossing a
    // manual phonetic hint.
    for index in 0..notes.len() {
        if languages[index].is_some() || !notes[index].lyric.is_sung() {
            continue;
        }
        let Some(source) = lexical::source(&notes[index].lyric) else {
            continue;
        };
        if carries_manual_hint(&notes[index]) {
            continue;
        }
        let is_fragment = !matches!(
            source.syllabic,
            None | Some(crate::engine::midi::Syllabic::Single)
        );
        if !is_fragment {
            continue;
        }
        if let Some(anchor) = lexical::candidate(&notes[index].lyric)
            .as_deref()
            .and_then(function_word_anchor)
        {
            languages[index] = Some(anchor);
            inherit_passage[index] = false;
            continue;
        }
        let mut owner = None;
        for candidate in (0..index).rev() {
            if carries_manual_hint(&notes[candidate]) {
                break;
            }
            if let Some(language) = languages[candidate] {
                owner = Some(language);
                break;
            }
        }
        if owner.is_none() {
            for candidate in (index + 1)..notes.len() {
                if carries_manual_hint(&notes[candidate]) {
                    break;
                }
                if let Some(language) = languages[candidate] {
                    owner = Some(language);
                    break;
                }
            }
        }
        languages[index] = owner;
        // A source-declared fragment has no independent lexical evidence. If
        // its local source domain cannot prove the owner, let route_track's
        // wider same-verse passage prior settle it after every domain has been
        // decoded.
        inherit_passage[index] = true;
    }

    if !units.is_empty() {
        diagnostics.push(Diagnostic {
            code: ROUTED.into(),
            severity: DiagnosticSeverity::Info,
            message: format!(
                "Automatic FR+EN routed {french_count} French word(s) through Millefeuille and {english_count} English word(s) through ARPAbet; no Default phonemizer is used for classified sung text."
            ),
            source_id: note_ids.first().cloned(),
        });
    }

    Route {
        languages,
        diagnostics,
        inherit_passage,
    }
}

/// Route every source voice/repeat domain in a projected lane and store the
/// decision directly on each note so alternative lyric lanes cannot collide.
pub fn route_track(track: &mut ProjectedTrack) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut inherit_passage = vec![false; track.notes.len()];
    let mut changed = HashMap::new();
    let mut start = 0usize;
    while start < track.notes.len() {
        let domain = track.notes[start]
            .source_evidence
            .as_ref()
            .and_then(|evidence| evidence.origin.as_ref())
            .map(|origin| {
                (
                    origin.source.part_id.clone(),
                    origin.source.staff_id.clone(),
                    origin.source.voice.clone(),
                    origin.source.occurrence,
                )
            });
        let mut end = start + 1;
        while end < track.notes.len()
            && track.notes[end]
                .source_evidence
                .as_ref()
                .and_then(|evidence| evidence.origin.as_ref())
                .map(|origin| {
                    (
                        origin.source.part_id.clone(),
                        origin.source.staff_id.clone(),
                        origin.source.voice.clone(),
                        origin.source.occurrence,
                    )
                })
                == domain
        {
            end += 1;
        }
        let ids: Vec<String> = track.notes[start..end]
            .iter()
            .map(|note| {
                note.source_evidence
                    .as_ref()
                    .map(|evidence| evidence.note_id.clone())
                    .unwrap_or_else(|| format!("{}:{}", track.source_track_id, note.onset_ticks))
            })
            .collect();
        // Materialize source-proven bracketed melismas before language routing.
        // Otherwise an untexted hold can sit between two syllables of one word,
        // receive no language, split the later pronunciation slice, and then be
        // removed by `drop_untexted`. Keeping this at the source-domain boundary
        // preserves the exact note chain without letting evidence cross voices
        // or repeat occurrences.
        crate::engine::syllable::preserve_bracketed_melismas(&mut track.notes[start..end]);
        let routed = route(&track.notes[start..end], &ids);
        inherit_passage[start..end].copy_from_slice(&routed.inherit_passage);
        for (note, language) in track.notes[start..end].iter_mut().zip(routed.languages) {
            note.pronunciation_language = language;
        }

        // A projected lane can continue a lyric phrase across a source-domain
        // boundary (voice bookkeeping or a restarted score voice). When the
        // new domain starts with a spelling that carries no reliable language
        // of its own, a directly preceding language-specific function word is
        // stronger phrase evidence than the isolated detector guess.
        if start > 0
            && touches(&track.notes[start - 1], &track.notes[start])
            && !ends_phrase(&track.notes[start - 1])
        {
            let previous_index = start - 1;
            if let (Some(previous_key), Some(previous_language)) = (
                lexical::candidate(&track.notes[previous_index].lyric),
                track.notes[previous_index].pronunciation_language,
            ) {
                if function_word_anchor(&previous_key) == Some(previous_language) {
                    let local_notes = &track.notes[start..end];
                    if let Some(first_word) = lexical::automatic_words(local_notes)
                        .into_iter()
                        .min_by_key(|members| members.first().copied().unwrap_or(usize::MAX))
                    {
                        if let Some(&head) = first_word.first() {
                            let key = lexical::preferred_joined_key(
                                local_notes,
                                &first_word,
                                |candidate| {
                                    french::contains_lexeme(candidate)
                                        || english::contains_lexeme(candidate)
                                },
                            );
                            let fr_lex = french::contains_lexeme(&key);
                            let en_lex = english::contains_lexeme(&key);
                            let shared = fr_lex && en_lex;
                            let unknown = !fr_lex && !en_lex;
                            let (self_fr, self_en) = lingua_scores(&key);
                            let proper_name_anchor = shared
                                && starts_capitalized(&local_notes[head])
                                && (self_fr - self_en).abs() >= 0.1;
                            let context_dependent =
                                unknown || (shared && (self_fr - self_en).abs() < 0.5);
                            if function_word_anchor(&key).is_none()
                                && context_dependent
                                && !proper_name_anchor
                            {
                                for &member in &first_word {
                                    let index = start + member;
                                    if track.notes[index].pronunciation_language
                                        != Some(previous_language)
                                    {
                                        if let Some(source_id) = track.notes[index]
                                            .source_evidence
                                            .as_ref()
                                            .map(|evidence| evidence.note_id.clone())
                                        {
                                            changed.insert(source_id, previous_language);
                                        }
                                    }
                                    track.notes[index].pronunciation_language =
                                        Some(previous_language);
                                }
                                let mut continuation =
                                    start + first_word.last().copied().unwrap_or(head) + 1;
                                while continuation < end
                                    && track.notes[continuation].lyric.continues_previous_note()
                                    && touches(
                                        &track.notes[continuation - 1],
                                        &track.notes[continuation],
                                    )
                                {
                                    if track.notes[continuation].pronunciation_language
                                        != Some(previous_language)
                                    {
                                        if let Some(source_id) = track.notes[continuation]
                                            .source_evidence
                                            .as_ref()
                                            .map(|evidence| evidence.note_id.clone())
                                        {
                                            changed.insert(source_id, previous_language);
                                        }
                                    }
                                    track.notes[continuation].pronunciation_language =
                                        Some(previous_language);
                                    continuation += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        diagnostics.extend(
            routed
                .diagnostics
                .into_iter()
                .filter(|diagnostic| diagnostic.code != ROUTED),
        );
        start = end;
    }

    // Some source domains contain only a neutral vocalise or a very short
    // unknown spelling. Such a domain has no trustworthy local language to
    // decode. Resolve it from the nearest strong word in the same lyric row
    // after all source domains have been routed; this keeps translated rows
    // independent while allowing a passage to survive voice/repeat bookkeeping
    // boundaries.
    for index in 0..track.notes.len() {
        if !inherit_passage[index] || track.notes[index].lyric.continues_previous_note() {
            continue;
        }
        let row = lexical::source(&track.notes[index].lyric)
            .map(|source| (source.verse, source.lane.clone()));
        let same_passage = |candidate: usize| {
            let same_row = lexical::source(&track.notes[candidate].lyric)
                .map(|source| (source.verse, source.lane.clone()))
                == row;
            if !same_row || candidate.abs_diff(index) > 16 {
                return false;
            }
            let (from, to) = if candidate < index {
                (candidate, index)
            } else {
                (index, candidate)
            };
            !(from..to).any(|between| {
                ends_phrase(&track.notes[between]) || carries_manual_hint(&track.notes[between])
            })
        };
        let previous = (0..index).rev().find(|&candidate| {
            !inherit_passage[candidate]
                && same_passage(candidate)
                && track.notes[candidate].pronunciation_language.is_some()
                && !track.notes[candidate].lyric.continues_previous_note()
        });
        let next = ((index + 1)..track.notes.len()).find(|&candidate| {
            !inherit_passage[candidate]
                && same_passage(candidate)
                && track.notes[candidate].pronunciation_language.is_some()
                && !track.notes[candidate].lyric.continues_previous_note()
        });
        let local_language = previous
            .or(next)
            .and_then(|owner| track.notes[owner].pronunciation_language);
        let row_language = if local_language.is_none() {
            let mut french = 0usize;
            let mut english = 0usize;
            for (&inherits_passage, note) in inherit_passage.iter().zip(&track.notes) {
                if inherits_passage
                    || note.lyric.continues_previous_note()
                    || lexical::source(&note.lyric)
                        .map(|source| (source.verse, source.lane.clone()))
                        != row
                {
                    continue;
                }
                match note.pronunciation_language {
                    Some(PronunciationLanguage::French) => french += 1,
                    Some(PronunciationLanguage::English) => english += 1,
                    None => {}
                }
            }
            match french.cmp(&english) {
                std::cmp::Ordering::Greater => Some(PronunciationLanguage::French),
                std::cmp::Ordering::Less => Some(PronunciationLanguage::English),
                std::cmp::Ordering::Equal => None,
            }
        } else {
            None
        };
        let Some(language) = local_language.or(row_language) else {
            continue;
        };
        if track.notes[index].pronunciation_language != Some(language) {
            if let Some(source_id) = track.notes[index]
                .source_evidence
                .as_ref()
                .map(|evidence| evidence.note_id.clone())
            {
                changed.insert(source_id, language);
            }
        }
        track.notes[index].pronunciation_language = Some(language);

        let mut continuation = index + 1;
        while continuation < track.notes.len()
            && inherit_passage[continuation]
            && track.notes[continuation].lyric.continues_previous_note()
            && touches(&track.notes[continuation - 1], &track.notes[continuation])
        {
            track.notes[continuation].pronunciation_language = Some(language);
            continuation += 1;
        }
    }

    // A standalone function word is explicit language evidence and must win
    // over passage inheritance. This is especially important after a quoted
    // English section: a new French sentence beginning with `Et` must not keep
    // the previous English pronunciation merely because a source voice or
    // repeat boundary restarted there. Split-word heads are excluded so a
    // syllable such as `et-` inside a longer word cannot be forced in isolation.
    for index in 0..track.notes.len() {
        let Some(source) = lexical::source(&track.notes[index].lyric) else {
            continue;
        };
        if !matches!(
            source.syllabic,
            None | Some(crate::engine::midi::Syllabic::Single)
        ) {
            continue;
        }
        let Some(key) = lexical::candidate(&track.notes[index].lyric) else {
            continue;
        };
        let Some(language) = function_word_anchor(&key) else {
            continue;
        };
        if track.notes[index].pronunciation_language != Some(language) {
            if let Some(source_id) = track.notes[index]
                .source_evidence
                .as_ref()
                .map(|evidence| evidence.note_id.clone())
            {
                changed.insert(source_id, language);
            }
        }
        track.notes[index].pronunciation_language = Some(language);
        let mut continuation = index + 1;
        while continuation < track.notes.len()
            && track.notes[continuation].lyric.continues_previous_note()
            && touches(&track.notes[continuation - 1], &track.notes[continuation])
        {
            track.notes[continuation].pronunciation_language = Some(language);
            continuation += 1;
        }
    }

    for diagnostic in &mut diagnostics {
        if diagnostic.code != LOW_CONFIDENCE {
            continue;
        }
        let Some(language) = diagnostic
            .source_id
            .as_ref()
            .and_then(|source_id| changed.get(source_id))
        else {
            continue;
        };
        diagnostic.message = format!(
            "Automatic FR+EN: low-confidence sung text resolves to {} from the surrounding passage; export continues deterministically.",
            language.label()
        );
    }

    let mut french_count = 0usize;
    let mut english_count = 0usize;
    for note in &track.notes {
        if note.lyric.continues_previous_note() || lexical::candidate(&note.lyric).is_none() {
            continue;
        }
        match note.pronunciation_language {
            Some(PronunciationLanguage::French) => french_count += 1,
            Some(PronunciationLanguage::English) => english_count += 1,
            None => {}
        }
    }
    if french_count + english_count > 0 {
        diagnostics.push(Diagnostic {
            code: ROUTED.into(),
            severity: DiagnosticSeverity::Info,
            message: format!(
                "Automatic FR+EN routed {french_count} French word(s) through Millefeuille and {english_count} English word(s) through ARPAbet; no Default phonemizer is used for classified sung text."
            ),
            source_id: track
                .notes
                .first()
                .and_then(|note| note.source_evidence.as_ref())
                .map(|evidence| evidence.note_id.clone()),
        });
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::midi::{Lyric, LyricState, NoteSource, Syllabic};
    use crate::engine::projection::{NoteEvidence, NoteOrigin, ProjectedLyric};

    fn note(onset: u32, text: &str) -> ProjectedNote {
        let mut lyric = Lyric::text(format!("lyric-{onset}"), text.into());
        lyric.syllabic = Some(Syllabic::Single);
        lyric.state = LyricState::Text(text.into());
        ProjectedNote {
            performance: None,
            pronunciation_language: None,
            onset_ticks: onset,
            duration_ticks: 480,
            pitch: 60,
            lyric: ProjectedLyric::Source(Box::new(lyric)),
            source_evidence: None,
        }
    }

    fn languages(words: &[&str]) -> Route {
        let notes: Vec<_> = words
            .iter()
            .enumerate()
            .map(|(index, word)| note(index as u32 * 480, word))
            .collect();
        let ids: Vec<_> = (0..notes.len()).map(|i| format!("n{i}")).collect();
        route(&notes, &ids)
    }

    fn domain_note(onset: u32, text: &str, voice: &str) -> ProjectedNote {
        let mut result = note(onset, text);
        result.source_evidence = Some(NoteEvidence {
            note_id: format!("note-{voice}-{onset}"),
            note_on_event_id: format!("on-{voice}-{onset}"),
            note_off_event_id: format!("off-{voice}-{onset}"),
            lyric_id: Some(format!("lyric-{onset}")),
            lyric_event_id: Some(format!("lyric-event-{voice}-{onset}")),
            origin: Some(NoteOrigin {
                track_id: "track".into(),
                note_on_order: onset,
                note_off_order: onset + 1,
                source: NoteSource {
                    id: format!("source-{voice}-{onset}"),
                    part_id: Some("P1".into()),
                    staff_id: Some("1".into()),
                    voice: Some(voice.into()),
                    ..NoteSource::default()
                },
                lyric_conflict: false,
                continuation: None,
            }),
        });
        result
    }

    #[test]
    fn context_keeps_short_homographs_inside_their_phrase() {
        let notes = [
            note(0, "je"),
            note(480, "suis"),
            note(960, "on"),
            note(1440, "heureux."),
            note(1920, "come"),
            note(2400, "on"),
            note(2880, "baby"),
        ];
        let ids: Vec<_> = (0..notes.len()).map(|i| format!("n{i}")).collect();
        let route = route(&notes, &ids);
        assert_eq!(route.languages[2], Some(PronunciationLanguage::French));
        assert_eq!(route.languages[5], Some(PronunciationLanguage::English));
        assert!(route
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == LOW_CONFIDENCE));
    }

    #[test]
    fn clear_passages_switch_french_english_french_without_splitting_the_lane() {
        let route = languages(&["bonjour", "beautiful.", "merci"]);
        assert_eq!(
            route.languages,
            [
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::English),
                Some(PronunciationLanguage::French),
            ]
        );
    }

    #[test]
    fn clear_unpunctuated_switches_are_not_smoothed_away() {
        let notes = [
            note(0, "bonjour"),
            note(480, "beautiful"),
            note(960, "merci"),
        ];
        let ids = ["a".into(), "b".into(), "c".into()];
        assert_eq!(
            route(&notes, &ids).languages,
            [
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::English),
                Some(PronunciationLanguage::French),
            ]
        );
    }

    #[test]
    fn route_track_keeps_passage_context_across_touching_source_domains() {
        let mut track = ProjectedTrack {
            name: "Voice".into(),
            source_track_id: "track".into(),
            muted: false,
            notes: vec![domain_note(0, "et", "1"), domain_note(480, "hou", "2")],
        };
        route_track(&mut track);
        assert_eq!(
            track
                .notes
                .iter()
                .map(|note| note.pronunciation_language)
                .collect::<Vec<_>>(),
            [
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::French),
            ]
        );
    }

    #[test]
    fn neutral_vocalise_inherits_its_passage_and_is_audited() {
        let route = languages(&["bonjour", "hou", "merci"]);
        assert_eq!(route.languages[1], Some(PronunciationLanguage::French));
        assert!(route.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == LOW_CONFIDENCE && diagnostic.source_id.as_deref() == Some("n1")
        }));
    }

    #[test]
    fn shared_word_uses_english_phrase_context() {
        let route = languages(&["come", "on", "baby", "do", "the", "locomotion"]);
        assert_eq!(
            route.languages,
            vec![Some(PronunciationLanguage::English); 6]
        );
    }

    #[test]
    fn embedded_english_name_does_not_flip_french_function_words() {
        let route = languages(&["et", "les", "beach", "boys", "chantaient"]);
        assert_eq!(
            route.languages,
            [
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::English),
                Some(PronunciationLanguage::English),
                Some(PronunciationLanguage::French),
            ]
        );

        let proper_name = languages(&["et", "les", "Beatles", "chantaient"]);
        assert_eq!(
            proper_name.languages,
            [
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::English),
                Some(PronunciationLanguage::French),
            ]
        );
    }

    #[test]
    fn english_phrase_and_title_can_switch_inside_french_context() {
        let phrase = languages(&["excuse", "me", "Sir", "mais"]);
        assert_eq!(
            phrase.languages,
            [
                Some(PronunciationLanguage::English),
                Some(PronunciationLanguage::English),
                Some(PronunciationLanguage::English),
                Some(PronunciationLanguage::French),
            ]
        );

        let title = languages(&["Satisfaction", "yeah", "yeah"]);
        assert!(title
            .languages
            .iter()
            .all(|language| *language == Some(PronunciationLanguage::English)));
    }

    #[test]
    fn french_context_keeps_shared_loanwords_french() {
        let ticket = languages(&["on", "a", "tous", "dans", "le", "ticket", "pour"]);
        assert!(ticket
            .languages
            .iter()
            .all(|language| *language == Some(PronunciationLanguage::French)));

        let camping = languages(&["au", "camping", "des", "flots", "bleus"]);
        assert!(camping
            .languages
            .iter()
            .all(|language| *language == Some(PronunciationLanguage::French)));
    }

    #[test]
    fn explicit_function_word_anchor_beats_malformed_fragment_marker() {
        let mut et = note(0, "Et");
        let ProjectedLyric::Source(source) = &mut et.lyric else {
            unreachable!()
        };
        source.syllabic = Some(Syllabic::End);
        assert_eq!(
            route(&[et], &["et".into()]).languages,
            [Some(PronunciationLanguage::French)]
        );
    }

    #[test]
    fn misspelling_and_vocalise_follow_their_passage() {
        let french = languages(&["et", "saintl", "malo", "dormait"]);
        assert!(french
            .languages
            .iter()
            .all(|language| *language == Some(PronunciationLanguage::French)));

        let english = languages(&["i", "can't", "get", "no", "tou", "toum"]);
        assert!(english
            .languages
            .iter()
            .all(|language| *language == Some(PronunciationLanguage::English)));
    }

    #[test]
    fn incomplete_syllable_is_not_classified_in_isolation() {
        let mut orphan = note(0, "dream");
        let ProjectedLyric::Source(source) = &mut orphan.lyric else {
            unreachable!()
        };
        source.syllabic = Some(Syllabic::Begin);
        let routed = route(&[orphan], &["orphan".into()]);
        assert_eq!(routed.languages, [None]);
    }

    #[test]
    fn incomplete_fragment_does_not_inherit_across_manual_hint() {
        let mut fragment = note(960, "dream");
        let ProjectedLyric::Source(source) = &mut fragment.lyric else {
            unreachable!()
        };
        source.syllabic = Some(Syllabic::Begin);
        let routed = route(
            &[note(0, "bonjour"), note(480, "?manual"), fragment],
            &["before".into(), "hint".into(), "fragment".into()],
        );
        assert_eq!(routed.languages[2], None);
    }

    #[test]
    fn malformed_fragment_recovery_cannot_accept_only_a_dictionary_prefix() {
        let mut notes = [note(0, "bon"), note(480, "jour"), note(960, "xyz")];
        for note in &mut notes {
            let ProjectedLyric::Source(source) = &mut note.lyric else {
                unreachable!()
            };
            source.syllabic = Some(Syllabic::Middle);
        }
        let ids = ["a".into(), "b".into(), "c".into()];
        assert!(build_units(&notes, &ids).is_empty());
    }

    #[test]
    fn untexted_note_without_proven_owner_stays_language_neutral() {
        let mut untexted = note(0, "");
        untexted.lyric = ProjectedLyric::Absent;
        let routed = route(&[untexted], &["untexted".into()]);
        assert_eq!(routed.languages, [None]);
        assert!(routed.diagnostics.is_empty());
    }

    #[test]
    fn complete_split_word_and_touching_continuation_move_together() {
        let mut first = note(0, "bon");
        let mut second = note(480, "jour");
        let mut held = note(960, "-");
        let ProjectedLyric::Source(first_source) = &mut first.lyric else {
            unreachable!()
        };
        first_source.syllabic = Some(Syllabic::Begin);
        let ProjectedLyric::Source(second_source) = &mut second.lyric else {
            unreachable!()
        };
        second_source.syllabic = Some(Syllabic::End);
        let ProjectedLyric::Source(held_source) = &mut held.lyric else {
            unreachable!()
        };
        held_source.state = LyricState::Continuation;
        let routed = route(
            &[first, second, held],
            &["n0".into(), "n1".into(), "n2".into()],
        );
        assert_eq!(
            routed.languages,
            [
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::French),
            ]
        );
    }
}
