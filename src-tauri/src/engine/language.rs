//! Offline automatic French/English/Spanish/Portuguese routing for sung lyrics.
//!
//! Language is pronunciation policy, never source evidence. The router works on
//! complete source-attested words, combines Lingua's local statistical model
//! with Verse's pinned French/English/Spanish/Portuguese lexicons, and decodes one deterministic
//! language sequence so short homographs do not flip a phrase on their own.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use lingua::{Language, LanguageDetector, LanguageDetectorBuilder};

use crate::engine::convert::{Diagnostic, DiagnosticSeverity};
use crate::engine::projection::{ProjectedNote, ProjectedTrack, PronunciationLanguage};
use crate::engine::syllable::touches;
use crate::engine::target::{english, french, lexical, portuguese, spanish};

pub const ROUTED: &str = "AUTOMATIC_LANGUAGE_ROUTED";
pub const LOW_CONFIDENCE: &str = "AUTOMATIC_LANGUAGE_LOW_CONFIDENCE";

impl PronunciationLanguage {
    const ALL: [Self; 4] = [Self::French, Self::English, Self::Spanish, Self::Portuguese];

    fn index(self) -> usize {
        match self {
            Self::French => 0,
            Self::English => 1,
            Self::Spanish => 2,
            Self::Portuguese => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::French => "French",
            Self::English => "English",
            Self::Spanish => "Spanish",
            Self::Portuguese => "Portuguese",
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
    spanish_score: f64,
    portuguese_score: f64,
    local_uncertain: bool,
    inherit_passage: bool,
}

#[derive(Clone, Debug)]
pub struct Route {
    pub languages: Vec<Option<PronunciationLanguage>>,
    pub diagnostics: Vec<Diagnostic>,
    inherit_passage: Vec<bool>,
    words: Vec<RoutedWord>,
}

#[derive(Clone, Debug)]
struct RoutedWord {
    members: Vec<usize>,
    contextual: bool,
    complete: bool,
}

impl RoutedWord {
    fn is_donor(&self) -> bool {
        self.complete && !self.contextual
    }
}

fn playback_segment(note: &ProjectedNote) -> Option<u32> {
    note.source_evidence
        .as_ref()?
        .origin
        .as_ref()?
        .source
        .continuity
        .as_ref()
        .map(|continuity| continuity.playback_segment)
}

fn same_continuation_domain(head: &ProjectedNote, tail: &ProjectedNote) -> bool {
    let domain = |note: &ProjectedNote| {
        note.source_evidence
            .as_ref()
            .and_then(|e| e.origin.as_ref())
            .map(|origin| {
                (
                    origin.source.part_id.clone(),
                    origin.source.staff_id.clone(),
                    origin.source.voice.clone(),
                    origin.source.occurrence,
                    playback_segment(note),
                )
            })
    };
    let row = |note: &ProjectedNote| {
        lexical::source(&note.lyric).map(|source| (source.verse, source.lane.clone()))
    };
    domain(head) == domain(tail)
        && row(tail).is_none_or(|tail_row| row(head).is_some_and(|head_row| head_row == tail_row))
}

fn same_language_context_row(head: &ProjectedNote, tail: &ProjectedNote) -> bool {
    let domain = |note: &ProjectedNote| {
        note.source_evidence
            .as_ref()
            .and_then(|e| e.origin.as_ref())
            .map(|origin| {
                (
                    origin.source.part_id.clone(),
                    origin.source.staff_id.clone(),
                    origin.source.occurrence,
                    playback_segment(note),
                )
            })
    };
    let row = |note: &ProjectedNote| {
        lexical::source(&note.lyric).map(|source| (source.verse, source.lane.clone()))
    };
    matches!(
        (domain(head), domain(tail), row(head), row(tail)),
        (Some(left_domain), Some(right_domain), Some(left_row), Some(right_row))
            if left_domain == right_domain && left_row == right_row
    )
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
            Language::Spanish | Language::Portuguese => {}
        }
    }
    (french, english)
}

fn multilingual_scores(text: &str) -> [f64; 4] {
    static DETECTOR: OnceLock<LanguageDetector> = OnceLock::new();
    let detector = DETECTOR.get_or_init(|| {
        LanguageDetectorBuilder::from_languages(&[
            Language::French,
            Language::English,
            Language::Spanish,
            Language::Portuguese,
        ])
        .with_preloaded_language_models()
        .build()
    });
    let mut scores = [0.0; 4];
    for (language, confidence) in detector.compute_language_confidence_values(text) {
        let index = match language {
            Language::French => 0,
            Language::English => 1,
            Language::Spanish => 2,
            Language::Portuguese => 3,
        };
        scores[index] = confidence;
    }
    scores
}

fn lexical_membership(key: &str) -> [bool; 4] {
    [
        french::contains_automatic_lexeme(key),
        english::contains_lexeme(key),
        spanish::contains_lexeme(key) || spanish::has_pronunciation(key),
        portuguese::contains_lexeme(key) || portuguese::has_pronunciation(key),
    ]
}

fn known_lexeme(key: &str) -> bool {
    lexical_membership(key).into_iter().any(|known| known)
}

/// These tokens are not exclusive FR/EN evidence once Spanish and Portuguese
/// participate. Their complete word and passage context decide their owner.
fn shared_romance_word(key: &str) -> bool {
    matches!(
        key,
        "a" | "à"
            | "e"
            | "o"
            | "os"
            | "as"
            | "de"
            | "do"
            | "da"
            | "dos"
            | "das"
            | "em"
            | "no"
            | "na"
            | "nos"
            | "nas"
            | "que"
            | "se"
            | "si"
            | "tu"
            | "me"
            | "te"
            | "un"
            | "la"
            | "le"
            | "les"
            | "el"
            | "lo"
            | "los"
            | "del"
            | "al"
            | "en"
            | "es"
            | "son"
            | "somos"
            | "mais"
            | "mas"
            | "por"
            | "para"
            | "con"
            | "como"
            | "y"
            | "eu"
            | "mes"
            | "ton"
            | "ta"
            | "tes"
            | "ma"
    )
}

fn automatic_anchor(key: &str) -> Option<PronunciationLanguage> {
    if shared_romance_word(key) {
        return None;
    }
    if matches!(
        key,
        "tú" | "usted" | "ustedes" | "vosotros" | "nosotros" | "estáis" | "hola" | "gracias"
    ) {
        return Some(PronunciationLanguage::Spanish);
    }
    if matches!(
        key,
        "você"
            | "vocês"
            | "não"
            | "nós"
            | "estão"
            | "também"
            | "isto"
            | "isso"
            | "olá"
            | "obrigado"
            | "obrigada"
    ) {
        return Some(PronunciationLanguage::Portuguese);
    }
    if matches!(key, "bonjour" | "bonsoir" | "merci") {
        return Some(PronunciationLanguage::French);
    }
    function_word_anchor(key)
}

/// Grammatical clues distinguish closely related ES/PT passages. These are
/// soft context votes: e.g. French `mis` and `eu` must not force another language.
fn romance_context_vote(key: &str) -> f64 {
    if matches!(
        key,
        "yo" | "con"
            | "sin"
            | "mis"
            | "tus"
            | "sus"
            | "lo"
            | "los"
            | "las"
            | "del"
            | "una"
            | "unos"
            | "unas"
            | "nuestro"
            | "nuestra"
            | "nuestros"
            | "nuestras"
            | "vuestro"
            | "vuestra"
            | "pero"
            | "muy"
            | "más"
            | "eres"
            | "soy"
            | "estoy"
    ) {
        1.0
    } else if matches!(
        key,
        "eu" | "com"
            | "sem"
            | "meu"
            | "minha"
            | "meus"
            | "minhas"
            | "seu"
            | "sua"
            | "seus"
            | "suas"
            | "nosso"
            | "nossa"
            | "nossos"
            | "nossas"
            | "um"
            | "uma"
            | "uns"
            | "umas"
            | "do"
            | "da"
            | "dos"
            | "das"
            | "ao"
            | "aos"
            | "pelo"
            | "pela"
            | "pelos"
            | "pelas"
    ) {
        -1.0
    } else {
        match automatic_anchor(key) {
            Some(PronunciationLanguage::Spanish) => return 1.0,
            Some(PronunciationLanguage::Portuguese) => return -1.0,
            _ => {}
        }
        let spanish = spanish::contains_lexeme(key);
        let portuguese = portuguese::contains_lexeme(key);
        if spanish && !portuguese {
            return 0.35;
        }
        if portuguese && !spanish {
            return -0.35;
        }
        0.0
    }
}

impl Unit {
    fn scores(&self) -> [f64; 4] {
        if self.inherit_passage {
            // A token with no independent language evidence must not pull its
            // owner toward the old two-language guess during sequence decoding.
            [0.0; 4]
        } else {
            [
                self.french_score,
                self.english_score,
                self.spanish_score,
                self.portuguese_score,
            ]
        }
    }
}

/// Preserve the qualified FR/EN conditional scorer and compare that language
/// family with ES/PT using the bundled four-language model. Subtracting the
/// same normalizer from both old scores preserves their relative decisions.
fn extend_language_scores(units: &mut [Unit], notes: &[ProjectedNote]) {
    let keys: Vec<_> = units.iter().map(|unit| unit.key.clone()).collect();
    let boundaries: Vec<_> = units.iter().map(|unit| unit.boundary_after).collect();
    let heads: Vec<_> = units.iter().map(|unit| unit.members[0]).collect();
    let mut cache = HashMap::new();
    for (index, unit) in units.iter_mut().enumerate() {
        let start = (0..index)
            .rev()
            .find(|&i| boundaries[i])
            .map_or(0, |i| i + 1);
        let end = (index..boundaries.len())
            .find(|&i| boundaries[i])
            .map_or(keys.len(), |i| i + 1);
        let context = keys[index.saturating_sub(2).max(start)..(index + 3).min(end)].join(" ");
        let phrase = keys[start..end].join(" ");
        let own = *cache
            .entry(unit.key.clone())
            .or_insert_with(|| multilingual_scores(&unit.key));
        let nearby = *cache
            .entry(context.clone())
            .or_insert_with(|| multilingual_scores(&context));
        let passage = *cache
            .entry(phrase.clone())
            .or_insert_with(|| multilingual_scores(&phrase));
        let neutral = matches!(
            unit.key.as_str(),
            "ah" | "aah" | "oh" | "ooh" | "ouh" | "hou" | "uh" | "mm" | "mmm" | "la" | "na"
        );
        let membership = lexical_membership(&unit.key);
        let lexical_match_count = membership.into_iter().filter(|known| *known).count();
        let mut ranked = own;
        ranked.sort_by(|left, right| right.total_cmp(left));
        let ambiguous = shared_romance_word(&unit.key)
            || lexical_match_count > 1
            || (ranked[0] - ranked[1] < 0.3)
            || (starts_capitalized(&notes[unit.members[0]])
                && french::contains_lexeme(&unit.key)
                && english::contains_lexeme(&unit.key));
        let weights = if neutral {
            [0.0, 1.0, 0.5]
        } else if ambiguous {
            [0.25, 1.0, 0.5]
        } else {
            [1.4, 0.35, 0.15]
        };
        let mut family_odds = -1.2;
        let mut spanish_odds = 0.0;
        for (scores, weight) in [own, nearby, passage].into_iter().zip(weights) {
            family_odds += weight * log_odds(scores[2] + scores[3], scores[0] + scores[1]);
            spanish_odds += weight * log_odds(scores[2], scores[3]);
        }
        match (membership[2], membership[3]) {
            (true, false) => {
                family_odds += 0.25;
                spanish_odds += 0.35;
            }
            (false, true) => {
                family_odds += 0.25;
                spanish_odds -= 0.35;
            }
            _ => {}
        }
        if (membership[0] || membership[1]) && !membership[2] && !membership[3] {
            family_odds -= 0.25;
        }
        // Hunspell membership is intentionally soft: its .dic files contain
        // roots plus affix flags, so an inflected form can be absent even when
        // its lemma is present. Whole-passage grammatical evidence is stronger
        // for ES/PT and keeps a monolingual song stable without preventing a
        // passage that contains clear per-word switches from switching.
        let passage_romance_vote: f64 = keys[start..end]
            .iter()
            .map(|key| romance_context_vote(key))
            .sum::<f64>()
            .clamp(-3.0, 3.0);
        if keys[start..end].len() >= 3 {
            spanish_odds += 0.8 * passage_romance_vote;
            family_odds += 0.15 * passage_romance_vote.abs();
        }
        for (neighbor, key) in keys
            .iter()
            .enumerate()
            .take((index + 4).min(end))
            .skip(index.saturating_sub(3).max(start))
        {
            let vote = romance_context_vote(key);
            let strength = 1.0 / (1.0 + index.abs_diff(neighbor) as f64);
            spanish_odds += 1.5 * vote * strength;
            family_odds += 0.65 * vote.abs() * strength;
        }
        // Shared content words usually finish the preceding phrase, while
        // a grammatical connector may begin the following one. Retain both
        // directions instead of overwriting a new phrase with a backward window.
        if ambiguous && membership[2] && membership[3] {
            let forward = if shared_romance_word(&unit.key) && index + 1 < end {
                let text = keys[index..(index + 3).min(end)].join(" ");
                let scores = *cache
                    .entry(text.clone())
                    .or_insert_with(|| multilingual_scores(&text));
                (scores[2].max(scores[3]) > 0.75).then_some(log_odds(scores[2], scores[3]))
            } else {
                None
            };
            let backward = if index > start {
                let text = keys[index.saturating_sub(2).max(start)..=index].join(" ");
                let scores = *cache
                    .entry(text.clone())
                    .or_insert_with(|| multilingual_scores(&text));
                (scores[2].max(scores[3]) > 0.75).then_some(log_odds(scores[2], scores[3]))
            } else {
                None
            };
            if let Some(odds) = forward.or(backward) {
                spanish_odds = odds;
            }
        }
        let normalizer = unit.french_score.max(unit.english_score);
        unit.french_score -= normalizer;
        unit.english_score -= normalizer;
        unit.spanish_score = family_odds + (2.0 * spanish_odds).min(0.0);
        unit.portuguese_score = family_odds + (-2.0 * spanish_odds).min(0.0);
        // A short Portuguese/Spanish word is not an unknown FR/EN vocalise.
        if !neutral
            && family_odds > 1.0
            && (own[2].max(own[3]) > 0.7 || ambiguous || membership[2] || membership[3])
        {
            unit.inherit_passage = false;
            unit.local_uncertain = ranked[0] - ranked[1] < 0.3;
        }
        // Dictionary overlap alone cannot justify an FR/EN prior. Use
        // positive, independent words from the same lyric row. A rest or
        // punctuation can isolate a repeated refrain, so consult its adjacent
        // passage when that span contains no independent word at all.
        let mut independent: Vec<_> = keys[start..end]
            .iter()
            .filter(|key| *key != &unit.key)
            .cloned()
            .collect();
        if independent.is_empty() {
            let row = lexical::source(&notes[unit.members[0]].lyric)
                .map(|source| (&source.lane, source.verse));
            let head = unit.members[0];
            let same_row = |other: usize| {
                let other_head = heads[other];
                lexical::source(&notes[other_head].lyric).map(|source| (&source.lane, source.verse))
                    == row
                    && playback_segment(&notes[head]) == playback_segment(&notes[other_head])
                    && !(head.min(other_head)..=head.max(other_head))
                        .any(|index| carries_manual_hint(&notes[index]))
            };
            let mut previous: Vec<_> = (0..index)
                .rev()
                .filter(|&other| keys[other] != unit.key && same_row(other))
                .take(2)
                .collect();
            previous.reverse();
            let next = ((index + 1)..keys.len())
                .filter(|&other| keys[other] != unit.key && same_row(other))
                .take(2);
            for other in previous.into_iter().chain(next) {
                independent.push(keys[other].clone());
            }
        }
        if (membership[0] || membership[1]) && lexical_match_count > 1 && !independent.is_empty() {
            let contrary = independent.join(" ");
            let scores = *cache
                .entry(contrary.clone())
                .or_insert_with(|| multilingual_scores(&contrary));
            let romance_anchor = independent.iter().any(|key| {
                matches!(
                    automatic_anchor(key),
                    Some(PronunciationLanguage::Spanish | PronunciationLanguage::Portuguese)
                )
            });
            if scores[0] + scores[1] > 0.65 && !romance_anchor {
                unit.spanish_score = unit.spanish_score.min(-1.0);
                unit.portuguese_score = unit.portuguese_score.min(-1.0);
            }
        }
        if let Some(anchor) = automatic_anchor(&unit.key).filter(|anchor| {
            // Inflected forms absent from Hunspell roots can still be genuine
            // Romance words in the pronunciation dictionaries. A complete
            // sentence may disambiguate them against their function-word prior.
            !(matches!(
                anchor,
                PronunciationLanguage::French | PronunciationLanguage::English
            ) && (membership[2] || membership[3])
                && own[anchor.index()] < 0.7
                && family_odds > 1.0
                && keys[start..end].iter().any(|key| key != &unit.key))
        }) {
            let mut scores = [-12.0; 4];
            scores[anchor.index()] = 12.0;
            [
                unit.french_score,
                unit.english_score,
                unit.spanish_score,
                unit.portuguese_score,
            ] = scores;
            unit.inherit_passage = false;
            unit.local_uncertain = false;
        }
    }
}

fn routing_message(counts: [usize; 4]) -> String {
    let [french, english, spanish, portuguese] = counts;
    format!("Automatic FR+EN+ES+PT classified {french} French, {english} English, {spanish} Spanish and {portuguese} Portuguese word(s).")
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

fn context_barrier(note: &ProjectedNote) -> bool {
    carries_manual_hint(note)
        || note
            .source_evidence
            .as_ref()
            .and_then(|evidence| evidence.origin.as_ref())
            .is_some_and(|origin| origin.lyric_conflict)
}

fn has_independent_language_evidence(
    track: &ProjectedTrack,
    word: &RoutedWord,
    language: PronunciationLanguage,
) -> bool {
    let head = word.members[0];
    let key = lexical::preferred_joined_key(&track.notes, &word.members, known_lexeme);
    if automatic_anchor(&key) == Some(language) {
        return true;
    }
    let membership = lexical_membership(&key);
    let index = language.index();
    let scores = multilingual_scores(&key);
    let strongest_other = scores
        .iter()
        .enumerate()
        .filter(|(candidate, _)| *candidate != index)
        .map(|(_, score)| *score)
        .fold(0.0_f64, f64::max);
    let margin = scores[index] - strongest_other;
    if !membership[index] {
        return scores[index] >= 0.75 && margin >= 0.5;
    }
    let membership_count = membership.iter().filter(|known| **known).count();
    let unique_lexicon = membership
        .iter()
        .enumerate()
        .all(|(candidate, known)| candidate == index || !known);
    let capitalized_shared_model = starts_capitalized(&track.notes[head])
        && membership_count > 1
        && !shared_romance_word(&key)
        && function_word_anchor(&key).is_none()
        && match language {
            PronunciationLanguage::French | PronunciationLanguage::English => {
                let (french, english) = lingua_scores(&key);
                match language {
                    PronunciationLanguage::French => french - english >= 0.1,
                    PronunciationLanguage::English => english - french >= 0.1,
                    _ => unreachable!(),
                }
            }
            PronunciationLanguage::Spanish | PronunciationLanguage::Portuguese => margin >= 0.15,
        };
    let capitalized_phrase_model = starts_capitalized(&track.notes[head])
        && [
            head.checked_sub(1),
            word.members.last().copied().map(|tail| tail + 1),
        ]
        .into_iter()
        .flatten()
        .filter(|&neighbor| neighbor < track.notes.len())
        .filter(|&neighbor| starts_capitalized(&track.notes[neighbor]))
        .filter(|&neighbor| {
            same_language_context_row(&track.notes[head], &track.notes[neighbor])
                && !context_barrier(&track.notes[neighbor])
        })
        .filter_map(|neighbor| {
            lexical::candidate(&track.notes[neighbor].lyric).map(|neighbor_key| {
                if neighbor < head {
                    format!("{neighbor_key} {key}")
                } else {
                    format!("{key} {neighbor_key}")
                }
            })
        })
        .any(|phrase| match language {
            PronunciationLanguage::French | PronunciationLanguage::English => {
                let (french, english) = lingua_scores(&phrase);
                match language {
                    PronunciationLanguage::French => french - english >= 0.1,
                    PronunciationLanguage::English => english - french >= 0.1,
                    _ => unreachable!(),
                }
            }
            PronunciationLanguage::Spanish | PronunciationLanguage::Portuguese => {
                let scores = multilingual_scores(&phrase);
                let own = scores[index];
                let other = scores
                    .iter()
                    .enumerate()
                    .filter(|(candidate, _)| *candidate != index)
                    .map(|(_, score)| *score)
                    .fold(0.0_f64, f64::max);
                own - other >= 0.2
            }
        });
    (unique_lexicon && margin >= 0.15) || capitalized_shared_model || capitalized_phrase_model
}

/// Technical chord-member tracks can contain no lexical evidence of their own.
/// Share only a language decision from the same original source domain and
/// lyric row; no text, note or word membership moves between tracks.
pub fn route_tracks(tracks: &mut [&mut ProjectedTrack]) -> Vec<Vec<Diagnostic>> {
    let routed: Vec<_> = tracks
        .iter_mut()
        .map(|track| route_track_with_inheritance(track))
        .collect();
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct Domain {
        part: String,
        staff: String,
        voice: String,
        occurrence: u32,
        segment: Option<u32>,
        verse: u32,
        lane: String,
    }
    let domain = |note: &ProjectedNote| -> Option<Domain> {
        let source = &note.source_evidence.as_ref()?.origin.as_ref()?.source;
        let lyric = lexical::source(&note.lyric)?;
        Some(Domain {
            part: source.part_id.clone()?,
            staff: source.staff_id.clone()?,
            voice: source.voice.clone()?,
            occurrence: source.occurrence,
            segment: playback_segment(note),
            verse: lyric.verse,
            lane: lyric.lane.clone(),
        })
    };
    let mut evidence: HashMap<Domain, Vec<(u32, PronunciationLanguage)>> = HashMap::new();
    let mut barriers: HashMap<Domain, Vec<u32>> = HashMap::new();
    for (track, (_, words)) in tracks.iter().zip(&routed) {
        for note in &track.notes {
            if context_barrier(note) {
                if let Some(key) = domain(note) {
                    barriers.entry(key).or_default().push(note.onset_ticks);
                }
            }
        }
        for word in words {
            if !word.is_donor()
                || word
                    .members
                    .iter()
                    .any(|&member| context_barrier(&track.notes[member]))
            {
                continue;
            }
            // A later syllable or held attack does not start a new word and
            // therefore cannot supersede a genuinely newer word head.
            let head = &track.notes[word.members[0]];
            if let (Some(key), Some(language)) = (domain(head), head.pronunciation_language) {
                evidence
                    .entry(key)
                    .or_default()
                    .push((head.onset_ticks, language));
            }
        }
    }
    let mut results = Vec::with_capacity(tracks.len());
    for (track, (mut diagnostics, words)) in tracks.iter_mut().zip(routed) {
        for word in words.iter().filter(|word| word.contextual) {
            let head = &track.notes[word.members[0]];
            if word
                .members
                .iter()
                .any(|&member| context_barrier(&track.notes[member]))
            {
                continue;
            }
            let Some(key) = domain(head) else {
                continue;
            };
            let Some(candidates) = evidence.get(&key) else {
                continue;
            };
            let admissible = |onset: u32| {
                !barriers.get(&key).is_some_and(|times| {
                    times.iter().any(|time| {
                        *time >= onset.min(head.onset_ticks) && *time <= onset.max(head.onset_ticks)
                    })
                })
            };
            // Filter blocked donors first: an inadmissible predecessor must
            // not hide a valid successor on the other side of the recipient.
            let previous = candidates
                .iter()
                .filter(|(onset, _)| *onset <= head.onset_ticks && admissible(*onset))
                .map(|(onset, _)| *onset)
                .max();
            let next = candidates
                .iter()
                .filter(|(onset, _)| *onset > head.onset_ticks && admissible(*onset))
                .map(|(onset, _)| *onset)
                .min();
            let Some(onset) = previous.or(next) else {
                continue;
            };
            let mut owners = candidates
                .iter()
                .filter(|(time, _)| *time == onset)
                .map(|(_, language)| *language);
            let Some(language) = owners.next() else {
                continue;
            };
            if owners.any(|other| other != language) {
                continue;
            }
            // Source reconstruction already proved every member belongs to
            // this head, including text syllables and source-proven holds.
            for &member in &word.members {
                let note = &mut track.notes[member];
                note.pronunciation_language = Some(language);
                if let Some(source_id) = note.source_evidence.as_ref().map(|e| &e.note_id) {
                    for diagnostic in &mut diagnostics {
                        if diagnostic.code == LOW_CONFIDENCE
                            && diagnostic.source_id.as_ref() == Some(source_id)
                        {
                            diagnostic.message = format!("Automatic FR+EN+ES+PT: contextual sung text resolves to {} from the same source voice, playback segment and lyric row.", language.label());
                        }
                    }
                }
            }
        }
        diagnostics.retain(|diagnostic| diagnostic.code != ROUTED);
        let mut counts = [0usize; 4];
        for word in &words {
            if let Some(language) = track.notes[word.members[0]].pronunciation_language {
                counts[language.index()] += 1;
            }
        }
        if counts.iter().any(|count| *count > 0) {
            diagnostics.push(Diagnostic {
                code: ROUTED.into(),
                severity: DiagnosticSeverity::Info,
                message: routing_message(counts),
                source_id: track
                    .notes
                    .first()
                    .and_then(|note| note.source_evidence.as_ref())
                    .map(|e| e.note_id.clone()),
            });
        }
        results.push(diagnostics);
    }
    results
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
        let key = lexical::preferred_joined_key(notes, &members, known_lexeme);
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
            let key = lexical::preferred_joined_key(notes, &members, known_lexeme);
            if known_lexeme(&key) {
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
            _ => {}
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
                        PronunciationLanguage::Spanish | PronunciationLanguage::Portuguese => {}
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
                        PronunciationLanguage::Spanish | PronunciationLanguage::Portuguese => {}
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

        // Capitalization is weaker evidence than an attested function-word
        // phrase for a shared, locally uncertain word. Repeated attacks of the
        // same complete word retain that phrase context; repetition adds no
        // independent evidence for the isolated spelling's language.
        if capitalized && shared && unit_anchor.is_none() && local_log_odds.abs() < 0.8 {
            let mut run_start = position;
            while run_start > phrase_start && keys[run_start - 1] == key {
                run_start -= 1;
            }
            if run_start > phrase_start {
                if let Some(anchor) = function_word_anchor(&keys[run_start - 1]) {
                    let pair = format!("{} {key}", keys[run_start - 1]);
                    let (pair_fr, pair_en) = *score_cache
                        .entry(pair.clone())
                        .or_insert_with(|| lingua_scores(&pair));
                    let odds = log_odds(pair_fr, pair_en);
                    match anchor {
                        PronunciationLanguage::French if odds >= 0.8 => {
                            french_score = 3.5;
                            english_score = -3.5;
                        }
                        PronunciationLanguage::English if odds <= -0.8 => {
                            french_score = -3.5;
                            english_score = 3.5;
                        }
                        _ => {}
                    }
                }
            }
        }

        units.push(Unit {
            members,
            key,
            source_id: note_ids[head].clone(),
            boundary_after: boundaries[position],
            french_score,
            english_score,
            spanish_score: f64::NEG_INFINITY,
            portuguese_score: f64::NEG_INFINITY,
            local_uncertain,
            inherit_passage: (neutral_vocalise || short_unknown)
                && function_word_anchor(&self_text).is_none(),
        });
    }
    extend_language_scores(&mut units, notes);
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
    let mut dp = vec![[f64::NEG_INFINITY; 4]; units.len()];
    let mut back = vec![[0usize; 4]; units.len()];
    dp[0] = units[0].scores();
    for index in 1..units.len() {
        let penalty = transition_penalty(&units[index - 1]);
        for state in 0..4 {
            let mut best = dp[index - 1][state];
            let mut predecessor = state;
            for (previous, previous_score) in dp[index - 1].iter().copied().enumerate() {
                let candidate = previous_score - if previous == state { 0.0 } else { penalty };
                if candidate > best {
                    best = candidate;
                    predecessor = previous;
                }
            }
            dp[index][state] = best + units[index].scores()[state];
            back[index][state] = predecessor;
        }
    }
    let mut state = 0;
    for candidate in 1..4 {
        if dp.last().unwrap()[candidate] > dp.last().unwrap()[state] {
            state = candidate;
        }
    }
    let mut states = vec![PronunciationLanguage::French; units.len()];
    for index in (0..units.len()).rev() {
        states[index] = PronunciationLanguage::ALL[state];
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
    if !notes.iter().any(context_barrier) {
        return route_span(notes, note_ids);
    }
    // Isolate barriers before scoring and decoding, not only while choosing a
    // final donor. Otherwise a neutral word retains a decision already borrowed
    // from the other side of a manual hint or conflicting source record.
    let mut result = Route {
        languages: vec![None; notes.len()],
        inherit_passage: vec![false; notes.len()],
        diagnostics: Vec::new(),
        words: Vec::new(),
    };
    let mut start = 0;
    while start < notes.len() {
        let end = if context_barrier(&notes[start]) {
            start + 1
        } else {
            ((start + 1)..notes.len())
                .find(|&index| context_barrier(&notes[index]))
                .unwrap_or(notes.len())
        };
        let routed = route_span(&notes[start..end], &note_ids[start..end]);
        result.languages[start..end].copy_from_slice(&routed.languages);
        result.inherit_passage[start..end].copy_from_slice(&routed.inherit_passage);
        result
            .words
            .extend(routed.words.into_iter().map(|mut word| {
                for member in &mut word.members {
                    *member += start;
                }
                word
            }));
        result.diagnostics.extend(
            routed
                .diagnostics
                .into_iter()
                .filter(|diagnostic| diagnostic.code != ROUTED),
        );
        start = end;
    }
    let mut counts = [0usize; 4];
    for word in result.words.iter().filter(|word| word.complete) {
        if let Some(language) = result.languages[word.members[0]] {
            counts[language.index()] += 1;
        }
    }
    if counts.iter().any(|count| *count > 0) {
        result.diagnostics.push(Diagnostic {
            code: ROUTED.into(),
            severity: DiagnosticSeverity::Info,
            message: routing_message(counts),
            source_id: note_ids.first().cloned(),
        });
    }
    result
}

fn route_span(notes: &[ProjectedNote], note_ids: &[String]) -> Route {
    let units = build_units(notes, note_ids);
    let decisions = decode(&units);
    let mut languages = vec![None; notes.len()];
    let mut inherit_passage = vec![false; notes.len()];
    let mut diagnostics = Vec::new();
    let mut counts = [0usize; 4];
    let mut words: Vec<RoutedWord> = units
        .iter()
        .map(|unit| RoutedWord {
            members: unit.members.clone(),
            contextual: unit.inherit_passage,
            complete: true,
        })
        .collect();
    let mut ownership = vec![None; notes.len()];
    for (owner, word) in words.iter().enumerate() {
        for &member in &word.members {
            ownership[member] = Some(owner);
        }
    }

    for (unit, language) in units.iter().zip(&decisions) {
        for &member in &unit.members {
            languages[member] = Some(*language);
            inherit_passage[member] = unit.inherit_passage;
        }
        counts[language.index()] += 1;
        let mut scores = unit.scores();
        scores.sort_by(|left, right| right.total_cmp(left));
        let margin = scores[0] - scores[1];
        if unit.local_uncertain || margin < 1.0 {
            diagnostics.push(Diagnostic {
                code: LOW_CONFIDENCE.into(),
                severity: DiagnosticSeverity::Info,
                message: format!(
                    "Automatic FR+EN+ES+PT: {:?} resolves to {} from surrounding phrase context with a low score margin ({margin:.2}); export continues deterministically.",
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
            && same_continuation_domain(
                &notes[ownership[index - 1].map_or(index - 1, |owner| words[owner].members[0])],
                &notes[index],
            )
        {
            languages[index] = languages[index - 1];
            inherit_passage[index] = inherit_passage[index - 1];
            if let Some(owner) = ownership[index - 1] {
                ownership[index] = Some(owner);
                words[owner].members.push(index);
            }
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
        if context_barrier(&notes[index]) {
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
            .and_then(automatic_anchor)
        {
            languages[index] = Some(anchor);
            inherit_passage[index] = false;
            ownership[index] = Some(words.len());
            words.push(RoutedWord {
                members: vec![index],
                contextual: false,
                complete: false,
            });
            continue;
        }
        let mut owner = None;
        for candidate in (0..index).rev() {
            if context_barrier(&notes[candidate]) {
                break;
            }
            if let Some(language) = languages[candidate]
                .filter(|_| ownership[candidate].is_some_and(|owner| words[owner].is_donor()))
            {
                owner = Some(language);
                break;
            }
        }
        if owner.is_none() {
            for candidate in (index + 1)..notes.len() {
                if context_barrier(&notes[candidate]) {
                    break;
                }
                if let Some(language) = languages[candidate]
                    .filter(|_| ownership[candidate].is_some_and(|owner| words[owner].is_donor()))
                {
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
        ownership[index] = Some(words.len());
        words.push(RoutedWord {
            members: vec![index],
            contextual: true,
            complete: false,
        });
    }

    // Fragment recovery above can establish an owner after the initial hold
    // pass. Retain those source-proven tails in the same ownership metadata.
    for index in 1..notes.len() {
        if ownership[index].is_some()
            || !notes[index].lyric.continues_previous_note()
            || !touches(&notes[index - 1], &notes[index])
        {
            continue;
        }
        let Some(owner) = ownership[index - 1] else {
            continue;
        };
        let head = words[owner].members[0];
        if !same_continuation_domain(&notes[head], &notes[index]) {
            continue;
        }
        ownership[index] = Some(owner);
        words[owner].members.push(index);
        languages[index] = languages[head];
        inherit_passage[index] = inherit_passage[head];
    }

    words.sort_by_key(|word| word.members[0]);
    if !units.is_empty() {
        diagnostics.push(Diagnostic {
            code: ROUTED.into(),
            severity: DiagnosticSeverity::Info,
            message: routing_message(counts),
            source_id: note_ids.first().cloned(),
        });
    }

    Route {
        languages,
        diagnostics,
        inherit_passage,
        words,
    }
}

/// Route every source voice/repeat domain in a projected lane and store the
/// decision directly on each note so alternative lyric lanes cannot collide.
pub fn route_track(track: &mut ProjectedTrack) -> Vec<Diagnostic> {
    route_track_with_inheritance(track).0
}

const TRACK_STABILIZATION_WORD_RADIUS: usize = 8;

fn stabilize_low_confidence_track_words(
    track: &mut ProjectedTrack,
    words: &[RoutedWord],
    low_confidence_heads: &HashSet<usize>,
    changed: &mut HashMap<String, PronunciationLanguage>,
) {
    if low_confidence_heads.is_empty() {
        return;
    }
    let row_keys: Vec<_> = track
        .notes
        .iter()
        .map(|note| {
            let lyric = lexical::source(&note.lyric)?;
            let origin = note.source_evidence.as_ref()?.origin.as_ref()?;
            Some((
                origin.source.part_id.clone(),
                origin.source.staff_id.clone(),
                lyric.verse,
                lyric.lane.clone(),
                origin.source.occurrence,
                playback_segment(note),
            ))
        })
        .collect();
    let barriers: Vec<_> = track.notes.iter().map(context_barrier).collect();
    let initial_languages: Vec<_> = track
        .notes
        .iter()
        .map(|note| note.pronunciation_language)
        .collect();
    // A projected sung lane may restart its source voice bookkeeping without
    // changing the authored lyric row. Part/staff, however, remain hard source
    // ownership boundaries for this wider contextual repair.
    let same_row = |left: usize, right: usize| match (&row_keys[left], &row_keys[right]) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    };
    let context_is_open = |left: usize, right: usize| {
        let Some(row) = row_keys[left].as_ref() else {
            return false;
        };
        let from = left.min(right);
        let to = left.max(right);
        !(from..=to).any(|index| row_keys[index].as_ref() == Some(row) && barriers[index])
    };

    for (position, word) in words.iter().enumerate() {
        if !word.is_donor() || !low_confidence_heads.contains(&word.members[0]) {
            continue;
        }
        let head = word.members[0];
        if word.members.iter().any(|&member| barriers[member]) {
            continue;
        }
        let previous = words[..position]
            .iter()
            .rev()
            .filter(|candidate| same_row(candidate.members[0], head))
            .take(TRACK_STABILIZATION_WORD_RADIUS)
            .find_map(|candidate| {
                (candidate.is_donor()
                    && !low_confidence_heads.contains(&candidate.members[0])
                    && candidate.members.iter().all(|&member| !barriers[member])
                    && context_is_open(candidate.members[0], head))
                .then(|| initial_languages[candidate.members[0]])
                .flatten()
            });
        let next = words[position + 1..]
            .iter()
            .filter(|candidate| same_row(candidate.members[0], head))
            .take(TRACK_STABILIZATION_WORD_RADIUS)
            .find_map(|candidate| {
                (candidate.is_donor()
                    && !low_confidence_heads.contains(&candidate.members[0])
                    && candidate.members.iter().all(|&member| !barriers[member])
                    && context_is_open(candidate.members[0], head))
                .then(|| initial_languages[candidate.members[0]])
                .flatten()
            });
        let Some(language) = previous.filter(|language| Some(*language) == next) else {
            continue;
        };
        if initial_languages[head].is_some_and(|current| {
            current != language && has_independent_language_evidence(track, word, current)
        }) {
            continue;
        }
        for &member in &word.members {
            if track.notes[member].pronunciation_language == Some(language) {
                continue;
            }
            if let Some(source_id) = track.notes[member]
                .source_evidence
                .as_ref()
                .map(|evidence| evidence.note_id.clone())
            {
                changed.insert(source_id, language);
            }
            track.notes[member].pronunciation_language = Some(language);
        }
    }
}

fn record_low_confidence_heads(
    routed: &Route,
    note_ids: &[String],
    start: usize,
    low_confidence_heads: &mut HashSet<usize>,
) {
    for diagnostic in routed
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == LOW_CONFIDENCE)
    {
        let Some(source_id) = diagnostic.source_id.as_ref() else {
            continue;
        };
        if let Some(word) = routed.words.iter().find(|word| {
            note_ids
                .get(word.members[0])
                .is_some_and(|candidate| candidate == source_id)
        }) {
            low_confidence_heads.insert(start + word.members[0]);
        }
    }
}

fn route_track_with_inheritance(track: &mut ProjectedTrack) -> (Vec<Diagnostic>, Vec<RoutedWord>) {
    let mut diagnostics = Vec::new();
    let mut inherit_passage = vec![false; track.notes.len()];
    let mut changed = HashMap::new();
    let mut words = Vec::new();
    let mut low_confidence_heads = HashSet::new();
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
                    playback_segment(&track.notes[start]),
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
                        playback_segment(&track.notes[end]),
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
        record_low_confidence_heads(&routed, &ids, start, &mut low_confidence_heads);
        words.extend(routed.words.iter().map(|word| RoutedWord {
            members: word.members.iter().map(|member| member + start).collect(),
            contextual: word.contextual,
            complete: word.complete,
        }));
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
            && playback_segment(&track.notes[start - 1]) == playback_segment(&track.notes[start])
            && touches(&track.notes[start - 1], &track.notes[start])
            && !ends_phrase(&track.notes[start - 1])
            && words
                .iter()
                .any(|word| word.is_donor() && word.members[0] == start - 1)
            && !context_barrier(&track.notes[start - 1])
        {
            let previous_index = start - 1;
            if let (Some(previous_key), Some(previous_language)) = (
                lexical::candidate(&track.notes[previous_index].lyric),
                track.notes[previous_index].pronunciation_language,
            ) {
                if automatic_anchor(&previous_key) == Some(previous_language) {
                    let local_notes = &track.notes[start..end];
                    if let Some(first_word) = lexical::automatic_words(local_notes)
                        .into_iter()
                        .min_by_key(|members| members.first().copied().unwrap_or(usize::MAX))
                    {
                        if let Some(&head) = first_word.first() {
                            let key = lexical::preferred_joined_key(
                                local_notes,
                                &first_word,
                                known_lexeme,
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
                            // The local decoder already combines the complete
                            // word with its passage. Low isolated ES/PT
                            // confidence must not discard that evidence.
                            let explicit_new_language = local_notes[head]
                                .pronunciation_language
                                .is_some_and(|language| {
                                    matches!(
                                        language,
                                        PronunciationLanguage::Spanish
                                            | PronunciationLanguage::Portuguese
                                    ) && language != previous_language
                                        && !routed.inherit_passage[head]
                                });
                            if automatic_anchor(&key).is_none()
                                && context_dependent
                                && !proper_name_anchor
                                && !explicit_new_language
                                && !(previous_index..=start + head)
                                    .any(|index| context_barrier(&track.notes[index]))
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
                                    && !context_barrier(&track.notes[continuation])
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

    // Local statistical windows are intentionally allowed to propose a
    // language switch, but a low-confidence island must not split an otherwise
    // established source row merely because a rest or phrase boundary made its
    // local window tiny. Confident words on both sides are whole-track context:
    // if they agree, use that owner for the uncertain complete word. Confident
    // switches remain untouched, so genuinely multilingual passages still
    // select their own phonemizers.
    stabilize_low_confidence_track_words(track, &words, &low_confidence_heads, &mut changed);

    // Some source domains contain only a neutral vocalise or a very short
    // unknown spelling. Such a domain has no trustworthy local language to
    // decode. Resolve it from the nearest strong word in the same lyric row
    // after all source domains have been routed; this keeps translated rows
    // independent while allowing a passage to survive voice/repeat bookkeeping
    // boundaries.
    for word in words.iter().filter(|word| word.contextual) {
        let index = word.members[0];
        let row = lexical::source(&track.notes[index].lyric)
            .map(|source| (source.verse, source.lane.clone()));
        let same_passage = |candidate: usize| {
            let same_row = lexical::source(&track.notes[candidate].lyric)
                .map(|source| (source.verse, source.lane.clone()))
                == row;
            if !same_row
                || playback_segment(&track.notes[candidate])
                    != playback_segment(&track.notes[index])
                || candidate.abs_diff(index) > 16
            {
                return false;
            }
            let (from, to) = if candidate < index {
                (candidate, index)
            } else {
                (index, candidate)
            };
            !(from..to).any(|between| ends_phrase(&track.notes[between]))
                && !(from..=to).any(|between| context_barrier(&track.notes[between]))
        };
        let previous = words
            .iter()
            .filter(|word| word.is_donor())
            .map(|word| word.members[0])
            .filter(|&candidate| candidate < index)
            .rev()
            .find(|&candidate| {
                !inherit_passage[candidate]
                    && same_passage(candidate)
                    && track.notes[candidate].pronunciation_language.is_some()
                    && !track.notes[candidate].lyric.continues_previous_note()
            });
        let next = words
            .iter()
            .filter(|word| word.is_donor())
            .map(|word| word.members[0])
            .filter(|&candidate| candidate > index)
            .find(|&candidate| {
                !inherit_passage[candidate]
                    && same_passage(candidate)
                    && track.notes[candidate].pronunciation_language.is_some()
                    && !track.notes[candidate].lyric.continues_previous_note()
            });
        let local_language = previous
            .or(next)
            .and_then(|owner| track.notes[owner].pronunciation_language);
        let row_language = if local_language.is_none() {
            let mut counts = [0usize; 4];
            for donor in words.iter().filter(|word| word.is_donor()) {
                let note = &track.notes[donor.members[0]];
                let from = index.min(donor.members[0]);
                let to = index.max(donor.members[0]);
                if playback_segment(note) != playback_segment(&track.notes[index])
                    || (from..=to).any(|between| context_barrier(&track.notes[between]))
                    || note.lyric.continues_previous_note()
                    || lexical::source(&note.lyric)
                        .map(|source| (source.verse, source.lane.clone()))
                        != row
                {
                    continue;
                }
                if let Some(language) = note.pronunciation_language {
                    counts[language.index()] += 1;
                }
            }
            let highest = *counts.iter().max().unwrap();
            if highest > 0 && counts.iter().filter(|&&count| count == highest).count() == 1 {
                Some(
                    PronunciationLanguage::ALL
                        [counts.iter().position(|&count| count == highest).unwrap()],
                )
            } else {
                None
            }
        } else {
            None
        };
        let Some(language) = local_language.or(row_language) else {
            continue;
        };
        for &member in &word.members {
            if track.notes[member].pronunciation_language != Some(language) {
                if let Some(source_id) = track.notes[member]
                    .source_evidence
                    .as_ref()
                    .map(|e| e.note_id.clone())
                {
                    changed.insert(source_id, language);
                }
            }
            track.notes[member].pronunciation_language = Some(language);
        }
    }

    // A standalone function word is explicit language evidence and must win
    // over passage inheritance. This is especially important after a quoted
    // English section: a new French sentence beginning with `Et` must not keep
    // the previous English pronunciation merely because a source voice or
    // repeat boundary restarted there. Split-word heads are excluded so a
    // syllable such as `et-` inside a longer word cannot be forced in isolation.
    for (index, &inherits_passage) in inherit_passage.iter().enumerate() {
        if context_barrier(&track.notes[index]) {
            continue;
        }
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
        let Some(language) = automatic_anchor(&key) else {
            continue;
        };
        if !inherits_passage && track.notes[index].pronunciation_language.is_some() {
            continue;
        }
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
            && !context_barrier(&track.notes[continuation])
            && touches(&track.notes[continuation - 1], &track.notes[continuation])
            && same_continuation_domain(&track.notes[index], &track.notes[continuation])
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
            "Automatic FR+EN+ES+PT: low-confidence sung text resolves to {} from the surrounding passage; export continues deterministically.",
            language.label()
        );
    }

    let mut counts = [0usize; 4];
    for note in &track.notes {
        if note.lyric.continues_previous_note() || lexical::candidate(&note.lyric).is_none() {
            continue;
        }
        if let Some(language) = note.pronunciation_language {
            counts[language.index()] += 1;
        }
    }
    if counts.iter().any(|&count| count > 0) {
        diagnostics.push(Diagnostic {
            code: ROUTED.into(),
            severity: DiagnosticSeverity::Info,
            message: routing_message(counts),
            source_id: track
                .notes
                .first()
                .and_then(|note| note.source_evidence.as_ref())
                .map(|evidence| evidence.note_id.clone()),
        });
    }
    (diagnostics, words)
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

    #[test]
    fn shared_inflected_verb_follows_spanish_and_portuguese_sentences() {
        for (text, language) in [
            (
                "ella une nuestras voces al amanecer",
                PronunciationLanguage::Spanish,
            ),
            (
                "o vento une nossas vozes ao amanhecer",
                PronunciationLanguage::Portuguese,
            ),
        ] {
            let words: Vec<_> = text.split_whitespace().collect();
            assert_eq!(
                languages(&words).languages,
                vec![Some(language); words.len()],
                "{text}"
            );
        }
    }

    #[test]
    fn isolated_shared_romance_words_are_not_capped_without_contrary_context() {
        for key in ["vida", "cada", "casa", "mesa"] {
            for words in [vec![key], vec![key, key]] {
                let result = languages(&words);
                assert!(
                    result.languages.iter().all(|owner| matches!(
                        owner,
                        Some(PronunciationLanguage::Spanish | PronunciationLanguage::Portuguese)
                    )),
                    "{words:?}: {:?}",
                    result.languages
                );
            }
        }
    }

    #[test]
    fn strong_english_function_word_survives_foreign_dictionary_membership() {
        assert_eq!(
            languages(&["hola", "the", "gracias"]).languages,
            [
                Some(PronunciationLanguage::Spanish),
                Some(PronunciationLanguage::English),
                Some(PronunciationLanguage::Spanish)
            ]
        );
    }

    #[test]
    fn shared_phrase_start_uses_forward_portuguese_evidence() {
        assert_eq!(
            languages(&["quiero", "cantar", "no", "meu", "coração"]).languages,
            [
                Some(PronunciationLanguage::Spanish),
                Some(PronunciationLanguage::Spanish),
                Some(PronunciationLanguage::Portuguese),
                Some(PronunciationLanguage::Portuguese),
                Some(PronunciationLanguage::Portuguese)
            ]
        );
    }

    #[test]
    fn unpunctuated_four_language_passage_keeps_shared_word_with_preceding_phrase() {
        let words: Vec<_> =
            "je chante doucement we follow the river quiero bailar contigo quero dançar contigo"
                .split_whitespace()
                .collect();
        let expected: Vec<_> = [
            (PronunciationLanguage::French, 3),
            (PronunciationLanguage::English, 4),
            (PronunciationLanguage::Spanish, 3),
            (PronunciationLanguage::Portuguese, 3),
        ]
        .into_iter()
        .flat_map(|(language, count)| std::iter::repeat_n(Some(language), count))
        .collect();
        assert_eq!(languages(&words).languages, expected);
    }

    #[test]
    fn contextual_word_owners_reach_output_phonemizers_and_exact_hints() {
        use crate::engine::{
            convert::convert_midi_with_profile,
            musicxml,
            target::{self, ExportTarget, PronunciationProfile},
        };
        for (text, expected) in [
            ("we hear the Radio Radio", vec![PronunciationLanguage::English; 5]),
            ("ella une nuestras voces al amanecer", vec![PronunciationLanguage::Spanish; 6]),
            ("o vento une nossas vozes ao amanhecer", vec![PronunciationLanguage::Portuguese; 7]),
            ("je chante doucement we follow the river quiero bailar contigo quero dançar contigo", [vec![PronunciationLanguage::French; 3], vec![PronunciationLanguage::English; 4], vec![PronunciationLanguage::Spanish; 3], vec![PronunciationLanguage::Portuguese; 3]].concat()),
        ] {
            let words: Vec<_> = text.split_whitespace().collect();
            let notes = words.iter().map(|word| format!("<note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration><lyric><syllabic>single</syllabic><text>{word}</text></lyric></note>")).collect::<String>();
            let xml = format!("<score-partwise version=\"4.0\"><part-list><score-part id=\"P1\"><part-name>Voice</part-name></score-part></part-list><part id=\"P1\"><measure number=\"1\"><attributes><divisions>1</divisions></attributes>{notes}</measure></part></score-partwise>");
            let source = musicxml::parse(xml.as_bytes()).unwrap();
            let baseline = convert_midi_with_profile(&source, "english", None, ExportTarget::Ustx, PronunciationProfile::Default);
            let outcome = convert_midi_with_profile(&source, "english", None, ExportTarget::Ustx, PronunciationProfile::Automatic);
            assert!(outcome.ok, "{:?}", outcome.msg);
            let project = outcome.svp.as_ref().unwrap();
            let projected = &project.tracks[0].notes;
            assert_eq!(projected.len(), words.len());
            for (((note, original), word), language) in projected.iter().zip(&baseline.svp.as_ref().unwrap().tracks[0].notes).zip(&words).zip(&expected) {
                assert_eq!(note.pronunciation_language, Some(*language), "{text}: {word}");
                assert_eq!(lexical::source(&note.lyric).unwrap().raw, *word);
                assert_eq!((note.onset_ticks, note.duration_ticks, note.pitch, &note.source_evidence, &note.performance), (original.onset_ticks, original.duration_ticks, original.pitch, &original.source_evidence, &original.performance));
            }
            let native = target::ustx::serialize(project).unwrap();
            for (note, language) in native.voice_parts[0].notes.iter().zip(expected) {
                let phonemizer = match language {
                    PronunciationLanguage::French => target::diffsinger::FRENCH_NAME,
                    PronunciationLanguage::English => target::diffsinger::ENGLISH_NAME,
                    PronunciationLanguage::Spanish => target::diffsinger::SPANISH_NAME,
                    PronunciationLanguage::Portuguese => target::diffsinger::PORTUGUESE_NAME,
                };
                assert_eq!(note.phonemizer.as_deref(), Some(phonemizer));
            }
            if text.starts_with("ella") {
                assert_eq!(native.voice_parts[0].notes[1].lyric, "une[u n e]");
            }
        }
    }

    #[test]
    fn automatic_recognizes_monolingual_passages_in_all_four_languages() {
        for (words, language) in [
            (
                vec!["je", "chante", "avec", "mes", "amis"],
                PronunciationLanguage::French,
            ),
            (
                vec!["I", "sing", "with", "my", "friends"],
                PronunciationLanguage::English,
            ),
            (
                vec!["yo", "canto", "con", "mis", "amigos"],
                PronunciationLanguage::Spanish,
            ),
            (
                vec!["eu", "canto", "com", "meus", "amigos"],
                PronunciationLanguage::Portuguese,
            ),
            (
                vec!["você", "não", "está", "sozinho"],
                PronunciationLanguage::Portuguese,
            ),
            (
                vec!["nós", "estamos", "aqui", "contigo"],
                PronunciationLanguage::Portuguese,
            ),
        ] {
            assert_eq!(
                languages(&words).languages,
                vec![Some(language); words.len()],
                "{words:?}"
            );
        }
    }

    #[test]
    fn shared_romance_function_words_follow_their_passage() {
        for (words, language) in [
            (
                vec!["je", "ne", "sais", "pas", "mais", "je", "chante"],
                PronunciationLanguage::French,
            ),
            (
                vec!["no", "sé", "lo", "que", "quieres"],
                PronunciationLanguage::Spanish,
            ),
            (
                vec!["eu", "sei", "que", "você", "quer", "mais"],
                PronunciationLanguage::Portuguese,
            ),
        ] {
            assert_eq!(
                languages(&words).languages,
                vec![Some(language); words.len()],
                "{words:?}"
            );
        }
    }

    #[test]
    fn clear_four_language_switches_keep_individual_word_owners() {
        assert_eq!(
            languages(&["bonjour", "beautiful", "hola", "obrigado", "merci"]).languages,
            [
                PronunciationLanguage::French,
                PronunciationLanguage::English,
                PronunciationLanguage::Spanish,
                PronunciationLanguage::Portuguese,
                PronunciationLanguage::French
            ]
            .map(Some)
        );
    }

    #[test]
    fn neutral_vocalises_inherit_spanish_and_portuguese_passages() {
        for (words, expected) in [
            (["hola", "amigos", "oh"], PronunciationLanguage::Spanish),
            (["você", "não", "oh"], PronunciationLanguage::Portuguese),
        ] {
            assert_eq!(languages(&words).languages[2], Some(expected));
        }
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

    fn test_track(notes: Vec<ProjectedNote>) -> ProjectedTrack {
        ProjectedTrack {
            name: "Voice".into(),
            source_track_id: "track".into(),
            muted: false,
            notes,
        }
    }

    fn routed_word(index: usize) -> RoutedWord {
        RoutedWord {
            members: vec![index],
            contextual: false,
            complete: true,
        }
    }

    #[test]
    fn track_stabilization_leaves_a_low_confidence_word_when_sides_disagree() {
        let mut track = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "de", "1"),
            domain_note(960, "beautiful", "1"),
        ]);
        track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        track.notes[1].pronunciation_language = Some(PronunciationLanguage::Spanish);
        track.notes[2].pronunciation_language = Some(PronunciationLanguage::English);
        let words = (0..3).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([1]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::Spanish)
        );
        assert!(changed.is_empty());
    }

    #[test]
    fn track_stabilization_keeps_a_real_single_word_switch_with_independent_evidence() {
        let mut track = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "the", "1"),
            domain_note(960, "merci", "1"),
        ]);
        track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        track.notes[1].pronunciation_language = Some(PronunciationLanguage::English);
        track.notes[2].pronunciation_language = Some(PronunciationLanguage::French);
        let words = (0..3).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([1]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
        assert!(changed.is_empty());
    }

    #[test]
    fn track_stabilization_keeps_strong_model_only_language_evidence() {
        assert!(!known_lexeme("nottinghamshire"));
        let scores = multilingual_scores("nottinghamshire");
        assert!(
            scores[PronunciationLanguage::English.index()] >= 0.75,
            "unexpected detector score: {scores:?}"
        );
        let mut track = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "Nottinghamshire", "1"),
            domain_note(960, "merci", "1"),
        ]);
        track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        track.notes[1].pronunciation_language = Some(PronunciationLanguage::English);
        track.notes[2].pronunciation_language = Some(PronunciationLanguage::French);
        let words = (0..3).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([1]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
        assert!(changed.is_empty());
    }

    #[test]
    fn track_stabilization_keeps_capitalized_proper_name_evidence() {
        let mut track = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "Beatles", "1"),
            domain_note(960, "merci", "1"),
        ]);
        track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        track.notes[1].pronunciation_language = Some(PronunciationLanguage::English);
        track.notes[2].pronunciation_language = Some(PronunciationLanguage::French);
        let words = (0..3).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([1]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
        assert!(changed.is_empty());
    }

    #[test]
    fn track_stabilization_keeps_a_capitalized_multiword_name_from_phrase_evidence() {
        let mut track = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "Big", "1"),
            domain_note(960, "Ben", "1"),
            domain_note(1440, "merci", "1"),
        ]);
        track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        track.notes[1].pronunciation_language = Some(PronunciationLanguage::English);
        track.notes[2].pronunciation_language = Some(PronunciationLanguage::English);
        track.notes[3].pronunciation_language = Some(PronunciationLanguage::French);
        let words = (0..4).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([1, 2]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
        assert_eq!(
            track.notes[2].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
        assert!(changed.is_empty());
    }

    #[test]
    fn track_stabilization_keeps_capitalized_multiword_name_across_voice_restart() {
        let mut track = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "Big", "1"),
            domain_note(960, "Ben", "2"),
            domain_note(1440, "merci", "1"),
        ]);
        track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        track.notes[1].pronunciation_language = Some(PronunciationLanguage::English);
        track.notes[2].pronunciation_language = Some(PronunciationLanguage::English);
        track.notes[3].pronunciation_language = Some(PronunciationLanguage::French);
        let words = (0..4).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([1, 2]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
        assert_eq!(
            track.notes[2].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
        assert!(changed.is_empty());
    }

    #[test]
    fn track_stabilization_can_correct_capitalized_unaccented_french_reves() {
        let mut track = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "Reves", "1"),
            domain_note(960, "merci", "1"),
        ]);
        track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        track.notes[1].pronunciation_language = Some(PronunciationLanguage::English);
        track.notes[2].pronunciation_language = Some(PronunciationLanguage::French);
        let words = (0..3).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([1]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::French)
        );
        assert_eq!(
            changed.get("note-1-480"),
            Some(&PronunciationLanguage::French)
        );
    }

    #[test]
    fn track_stabilization_requires_model_support_for_capitalized_shared_word() {
        let mut track = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "La", "1"),
            domain_note(960, "merci", "1"),
        ]);
        track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        track.notes[1].pronunciation_language = Some(PronunciationLanguage::English);
        track.notes[2].pronunciation_language = Some(PronunciationLanguage::French);
        let words = (0..3).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([1]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::French)
        );
    }

    #[test]
    fn track_stabilization_updates_every_member_of_a_low_confidence_split_word() {
        let mut track = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "suis", "1"),
            domain_note(960, "moi", "1"),
            domain_note(1440, "merci", "1"),
        ]);
        let ProjectedLyric::Source(left) = &mut track.notes[1].lyric else {
            unreachable!()
        };
        left.syllabic = Some(crate::engine::midi::Syllabic::Begin);
        let ProjectedLyric::Source(right) = &mut track.notes[2].lyric else {
            unreachable!()
        };
        right.syllabic = Some(crate::engine::midi::Syllabic::End);
        track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        track.notes[1].pronunciation_language = Some(PronunciationLanguage::Spanish);
        track.notes[2].pronunciation_language = Some(PronunciationLanguage::Spanish);
        track.notes[3].pronunciation_language = Some(PronunciationLanguage::French);
        let words = vec![
            routed_word(0),
            RoutedWord {
                members: vec![1, 2],
                contextual: false,
                complete: true,
            },
            routed_word(3),
        ];
        let low_confidence_heads = HashSet::from([1]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::French)
        );
        assert_eq!(
            track.notes[2].pronunciation_language,
            Some(PronunciationLanguage::French)
        );
        assert_eq!(changed.len(), 2);
    }

    #[test]
    fn unrelated_rows_do_not_consume_the_stabilization_radius_or_add_barriers() {
        let mut track = test_track(
            (0..19)
                .map(|index| {
                    domain_note(
                        index as u32 * 480,
                        if index == 0 {
                            "bonjour"
                        } else if index == 9 {
                            "de"
                        } else if index == 18 {
                            "merci"
                        } else {
                            "other"
                        },
                        "1",
                    )
                })
                .collect(),
        );
        for index in 1..18 {
            if index == 9 {
                continue;
            }
            let ProjectedLyric::Source(source) = &mut track.notes[index].lyric else {
                unreachable!()
            };
            source.lane = "other".into();
        }
        let ProjectedLyric::Source(manual) = &mut track.notes[5].lyric else {
            unreachable!()
        };
        manual.raw = "?manual".into();
        manual.state = LyricState::Text("?manual".into());
        for (index, note) in track.notes.iter_mut().enumerate() {
            note.pronunciation_language = Some(if matches!(index, 0 | 18) {
                PronunciationLanguage::French
            } else {
                PronunciationLanguage::Spanish
            });
        }
        let words = (0..track.notes.len()).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([9]);
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[9].pronunciation_language,
            Some(PronunciationLanguage::French)
        );
    }

    #[test]
    fn low_confidence_heads_are_scoped_to_the_routed_occurrence_not_note_id() {
        let warning = Route {
            languages: vec![Some(PronunciationLanguage::French)],
            diagnostics: vec![Diagnostic {
                code: LOW_CONFIDENCE.into(),
                severity: DiagnosticSeverity::Info,
                message: "test".into(),
                source_id: Some("same-source-note".into()),
            }],
            inherit_passage: vec![false],
            words: vec![routed_word(0)],
        };
        let confident = Route {
            languages: vec![Some(PronunciationLanguage::French)],
            diagnostics: Vec::new(),
            inherit_passage: vec![false],
            words: vec![routed_word(0)],
        };
        let ids = vec!["same-source-note".into()];
        let mut heads = HashSet::new();

        record_low_confidence_heads(&warning, &ids, 0, &mut heads);
        record_low_confidence_heads(&confident, &ids, 3, &mut heads);

        assert_eq!(heads, HashSet::from([0]));
    }

    #[test]
    fn track_stabilization_respects_part_and_staff_but_allows_voice_restart() {
        for boundary in ["part", "staff"] {
            let mut track = test_track(vec![
                domain_note(0, "bonjour", "1"),
                domain_note(480, "de", "1"),
                domain_note(960, "merci", "1"),
            ]);
            track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
            track.notes[1].pronunciation_language = Some(PronunciationLanguage::Spanish);
            track.notes[2].pronunciation_language = Some(PronunciationLanguage::French);
            let source = &mut track.notes[1]
                .source_evidence
                .as_mut()
                .unwrap()
                .origin
                .as_mut()
                .unwrap()
                .source;
            if boundary == "part" {
                source.part_id = Some("P2".into());
            } else {
                source.staff_id = Some("2".into());
            }
            let words = (0..3).map(routed_word).collect::<Vec<_>>();
            let low_confidence_heads = HashSet::from([1]);
            let mut changed = HashMap::new();
            stabilize_low_confidence_track_words(
                &mut track,
                &words,
                &low_confidence_heads,
                &mut changed,
            );
            assert_eq!(
                track.notes[1].pronunciation_language,
                Some(PronunciationLanguage::Spanish),
                "{boundary} must remain a hard source boundary"
            );
        }

        let mut restarted = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(480, "de", "2"),
            domain_note(960, "merci", "1"),
        ]);
        restarted.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
        restarted.notes[1].pronunciation_language = Some(PronunciationLanguage::Spanish);
        restarted.notes[2].pronunciation_language = Some(PronunciationLanguage::French);
        let words = (0..3).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = HashSet::from([1]);
        let mut changed = HashMap::new();
        stabilize_low_confidence_track_words(
            &mut restarted,
            &words,
            &low_confidence_heads,
            &mut changed,
        );
        assert_eq!(
            restarted.notes[1].pronunciation_language,
            Some(PronunciationLanguage::French)
        );
    }

    #[test]
    fn track_stabilization_respects_row_occurrence_segment_and_manual_boundaries() {
        for boundary in [
            "verse",
            "lane",
            "occurrence",
            "segment",
            "manual",
            "conflict",
        ] {
            let mut track = test_track(vec![
                domain_note(0, "bonjour", "1"),
                domain_note(480, "de", "1"),
                domain_note(960, "merci", "1"),
            ]);
            track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
            track.notes[1].pronunciation_language = Some(PronunciationLanguage::Spanish);
            track.notes[2].pronunciation_language = Some(PronunciationLanguage::French);
            match boundary {
                "verse" | "lane" => {
                    let ProjectedLyric::Source(source) = &mut track.notes[1].lyric else {
                        unreachable!()
                    };
                    if boundary == "verse" {
                        source.verse += 1;
                    } else {
                        source.lane = "other".into();
                    }
                }
                "occurrence" => {
                    track.notes[1]
                        .source_evidence
                        .as_mut()
                        .unwrap()
                        .origin
                        .as_mut()
                        .unwrap()
                        .source
                        .occurrence += 1;
                }
                "segment" => set_playback_segment(&mut track.notes[1], 2),
                "manual" => {
                    let ProjectedLyric::Source(source) = &mut track.notes[1].lyric else {
                        unreachable!()
                    };
                    source.raw = "?manual".into();
                    source.state = LyricState::Text("?manual".into());
                }
                "conflict" => {
                    track.notes[1]
                        .source_evidence
                        .as_mut()
                        .unwrap()
                        .origin
                        .as_mut()
                        .unwrap()
                        .lyric_conflict = true;
                }
                _ => unreachable!(),
            }
            let words = (0..3).map(routed_word).collect::<Vec<_>>();
            let low_confidence_heads = HashSet::from([1]);
            let mut changed = HashMap::new();

            stabilize_low_confidence_track_words(
                &mut track,
                &words,
                &low_confidence_heads,
                &mut changed,
            );

            assert_eq!(
                track.notes[1].pronunciation_language,
                Some(PronunciationLanguage::Spanish),
                "{boundary}"
            );
            assert!(changed.is_empty(), "{boundary}");
        }
    }

    #[test]
    fn track_stabilization_cannot_cross_same_row_manual_or_conflict_barrier() {
        for barrier in ["manual", "conflict"] {
            let mut track = test_track(vec![
                domain_note(0, "bonjour", "1"),
                domain_note(480, "barrier", "1"),
                domain_note(960, "de", "1"),
                domain_note(1440, "merci", "1"),
            ]);
            track.notes[0].pronunciation_language = Some(PronunciationLanguage::French);
            track.notes[1].pronunciation_language = Some(PronunciationLanguage::French);
            track.notes[2].pronunciation_language = Some(PronunciationLanguage::Spanish);
            track.notes[3].pronunciation_language = Some(PronunciationLanguage::French);
            if barrier == "manual" {
                let ProjectedLyric::Source(source) = &mut track.notes[1].lyric else {
                    unreachable!()
                };
                source.raw = "?manual".into();
                source.state = LyricState::Text("?manual".into());
            } else {
                track.notes[1]
                    .source_evidence
                    .as_mut()
                    .unwrap()
                    .origin
                    .as_mut()
                    .unwrap()
                    .lyric_conflict = true;
            }
            let words = (0..4).map(routed_word).collect::<Vec<_>>();
            let low_confidence_heads = HashSet::from([2]);
            let mut changed = HashMap::new();

            stabilize_low_confidence_track_words(
                &mut track,
                &words,
                &low_confidence_heads,
                &mut changed,
            );

            assert_eq!(
                track.notes[2].pronunciation_language,
                Some(PronunciationLanguage::Spanish),
                "{barrier}"
            );
            assert!(changed.is_empty(), "{barrier}");
        }
    }

    #[test]
    fn track_stabilization_does_not_borrow_beyond_its_word_radius() {
        let mut track = test_track(
            (0..19)
                .map(|index| {
                    domain_note(
                        index as u32 * 480,
                        if index == 0 {
                            "bonjour"
                        } else if index == 18 {
                            "merci"
                        } else {
                            "de"
                        },
                        "1",
                    )
                })
                .collect(),
        );
        for (index, note) in track.notes.iter_mut().enumerate() {
            note.pronunciation_language = Some(if index == 0 || index == 18 {
                PronunciationLanguage::French
            } else {
                PronunciationLanguage::Spanish
            });
        }
        let words = (0..track.notes.len()).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = (1..18).collect::<HashSet<_>>();
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert!(track.notes[1..18]
            .iter()
            .all(|note| { note.pronunciation_language == Some(PronunciationLanguage::Spanish) }));
        assert!(changed.is_empty());
    }

    #[test]
    fn track_stabilization_borrows_at_exact_word_radius() {
        let mut track = test_track(
            (0..17)
                .map(|index| {
                    domain_note(
                        index as u32 * 480,
                        if index == 0 {
                            "bonjour"
                        } else if index == 16 {
                            "merci"
                        } else {
                            "de"
                        },
                        "1",
                    )
                })
                .collect(),
        );
        for (index, note) in track.notes.iter_mut().enumerate() {
            note.pronunciation_language = Some(if index == 0 || index == 16 {
                PronunciationLanguage::French
            } else {
                PronunciationLanguage::Spanish
            });
        }
        let words = (0..track.notes.len()).map(routed_word).collect::<Vec<_>>();
        let low_confidence_heads = (1..16).collect::<HashSet<_>>();
        let mut changed = HashMap::new();

        stabilize_low_confidence_track_words(
            &mut track,
            &words,
            &low_confidence_heads,
            &mut changed,
        );

        assert_eq!(
            track.notes[8].pronunciation_language,
            Some(PronunciationLanguage::French)
        );
    }

    #[test]
    fn neutral_technical_lane_uses_matching_source_sibling_in_every_language() {
        for (word, expected) in [
            ("bonjour", PronunciationLanguage::French),
            ("the", PronunciationLanguage::English),
            ("hola", PronunciationLanguage::Spanish),
            ("você", PronunciationLanguage::Portuguese),
        ] {
            let mut established = test_track(vec![domain_note(0, word, "1")]);
            let mut neutral = test_track(vec![
                domain_note(480, "hou", "1"),
                domain_note(960, "oh", "1"),
            ]);
            let original = neutral.clone();
            let diagnostics = route_tracks(&mut [&mut established, &mut neutral]);
            for note in &neutral.notes {
                assert_eq!(note.pronunciation_language, Some(expected), "{word}");
            }
            assert!(diagnostics[1]
                .iter()
                .any(|d| d.code == LOW_CONFIDENCE && d.message.contains(expected.label())));
            for note in &mut neutral.notes {
                note.pronunciation_language = None;
            }
            assert_eq!(neutral, original);
        }
    }

    #[test]
    fn sibling_context_requires_identical_source_domain_and_lyric_row() {
        for difference in 0..6 {
            let mut established = test_track(vec![domain_note(0, "the", "1")]);
            let mut neutral = test_track(vec![domain_note(480, "hou", "1")]);
            let evidence = neutral.notes[0]
                .source_evidence
                .as_mut()
                .unwrap()
                .origin
                .as_mut()
                .unwrap();
            match difference {
                0 => evidence.source.part_id = Some("other".into()),
                1 => evidence.source.staff_id = Some("2".into()),
                2 => evidence.source.voice = Some("2".into()),
                3 => evidence.source.occurrence += 1,
                _ => {
                    let ProjectedLyric::Source(source) = &mut neutral.notes[0].lyric else {
                        unreachable!()
                    };
                    if difference == 4 {
                        source.verse += 1;
                    } else {
                        source.lane = "other".into();
                    }
                }
            }
            let mut isolated = neutral.clone();
            route_track(&mut isolated);
            route_tracks(&mut [&mut established, &mut neutral]);
            assert_eq!(neutral, isolated, "difference {difference}");
        }
    }

    #[test]
    fn sibling_context_keeps_neutral_begin_end_word_under_its_head_owner() {
        let mut observed = Vec::new();
        for (first, last, complete) in [("a", "h", "ah"), ("h", "ou", "hou")] {
            for reverse in [false, true] {
                let mut donor = test_track(vec![
                    domain_note(0, "the", "1"),
                    domain_note(720, "bonjour", "1"),
                ]);
                let mut neutral = test_track(vec![
                    domain_note(480, first, "1"),
                    domain_note(960, last, "1"),
                ]);
                for (note, syllabic) in neutral
                    .notes
                    .iter_mut()
                    .zip([Syllabic::Begin, Syllabic::End])
                {
                    let ProjectedLyric::Source(source) = &mut note.lyric else {
                        unreachable!()
                    };
                    source.syllabic = Some(syllabic);
                }
                let units = build_units(&neutral.notes, &["head".into(), "tail".into()]);
                assert_eq!(units.len(), 1);
                assert_eq!(units[0].key, complete);
                assert_eq!(units[0].members, [0, 1]);
                assert!(units[0].inherit_passage);
                let original = neutral.clone();
                if reverse {
                    route_tracks(&mut [&mut neutral, &mut donor]);
                } else {
                    route_tracks(&mut [&mut donor, &mut neutral]);
                }
                observed.push(
                    neutral
                        .notes
                        .iter()
                        .map(|note| note.pronunciation_language)
                        .collect::<Vec<_>>(),
                );
                for note in &mut neutral.notes {
                    note.pronunciation_language = None;
                }
                assert_eq!(neutral, original, "routing changes ownership only");
            }
        }
        assert_eq!(observed, vec![vec![Some(PronunciationLanguage::English); 2]; 4],
            "a donor switch between source syllables cannot split the complete word; normal/reversed track order");
    }

    #[test]
    fn sibling_context_uses_donor_time_before_first_and_after_last_in_either_track_order() {
        for reverse in [false, true] {
            let mut english = test_track(vec![domain_note(480, "the", "1")]);
            let mut french = test_track(vec![domain_note(1440, "bonjour", "1")]);
            let mut neutral = test_track(vec![
                domain_note(0, "hou", "1"),
                domain_note(960, "hou", "1"),
                domain_note(2400, "hou", "1"),
            ]);
            let original = neutral.clone();
            if reverse {
                route_tracks(&mut [&mut neutral, &mut french, &mut english]);
            } else {
                route_tracks(&mut [&mut english, &mut french, &mut neutral]);
            }
            assert_eq!(
                neutral
                    .notes
                    .iter()
                    .map(|note| note.pronunciation_language)
                    .collect::<Vec<_>>(),
                [
                    Some(PronunciationLanguage::English),
                    Some(PronunciationLanguage::English),
                    Some(PronunciationLanguage::French)
                ],
                "reverse={reverse}; before first, between donors, after last"
            );
            for note in &mut neutral.notes {
                note.pronunciation_language = None;
            }
            assert_eq!(neutral, original);
        }
    }

    #[test]
    fn sibling_context_only_complete_word_heads_supply_donor_time() {
        let mut old = test_track(vec![
            domain_note(0, "bon", "1"),
            domain_note(480, "jour", "1"),
        ]);
        for (note, syllabic) in old.notes.iter_mut().zip([Syllabic::Begin, Syllabic::End]) {
            let ProjectedLyric::Source(source) = &mut note.lyric else {
                unreachable!()
            };
            source.syllabic = Some(syllabic);
        }
        let mut newer = test_track(vec![domain_note(240, "the", "1")]);
        let mut neutral = test_track(vec![domain_note(960, "hou", "1")]);
        route_tracks(&mut [&mut old, &mut newer, &mut neutral]);
        assert_eq!(
            neutral.notes[0].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
    }

    #[test]
    fn recovered_function_fragments_keep_own_label_without_donating_context() {
        for syllabic in [Syllabic::Begin, Syllabic::Middle] {
            let mut fragment = domain_note(480, "the", "1");
            let ProjectedLyric::Source(source) = &mut fragment.lyric else {
                unreachable!()
            };
            source.syllabic = Some(syllabic.clone());
            let mut complete = test_track(vec![domain_note(0, "bonjour", "1")]);
            let mut recovered = test_track(vec![fragment.clone()]);
            let mut neutral = test_track(vec![domain_note(960, "hou", "1")]);
            route_tracks(&mut [&mut complete, &mut recovered, &mut neutral]);
            assert_eq!(
                recovered.notes[0].pronunciation_language,
                Some(PronunciationLanguage::English)
            );
            assert_eq!(
                neutral.notes[0].pronunciation_language,
                Some(PronunciationLanguage::French),
                "sibling {syllabic:?}"
            );
            for voice in ["1", "2"] {
                let mut local = test_track(vec![
                    domain_note(0, "bonjour", "1"),
                    fragment.clone(),
                    domain_note(960, "hou", voice),
                ]);
                route_track(&mut local);
                assert_eq!(
                    local.notes[1].pronunciation_language,
                    Some(PronunciationLanguage::English)
                );
                assert_eq!(
                    local.notes[2].pronunciation_language,
                    Some(PronunciationLanguage::French),
                    "local {syllabic:?}, voice={voice}"
                );
            }
        }
    }

    #[test]
    fn local_context_cannot_borrow_through_manual_or_conflict_barriers() {
        for conflict in [false, true] {
            for successor in [false, true] {
                let mut barrier = domain_note(480, if conflict { "the" } else { "?manual" }, "1");
                if conflict {
                    barrier
                        .source_evidence
                        .as_mut()
                        .unwrap()
                        .origin
                        .as_mut()
                        .unwrap()
                        .lyric_conflict = true;
                }
                let mut extension = domain_note(1440, "", "1");
                extension.lyric = ProjectedLyric::Extension;
                let mut local = test_track(vec![
                    domain_note(0, "the", "1"),
                    barrier,
                    domain_note(960, "hou", "1"),
                    extension,
                ]);
                let isolated = route(&local.notes[2..], &["neutral".into(), "hold".into()]);
                let expected = if successor {
                    Some(PronunciationLanguage::Spanish)
                } else {
                    isolated.languages[0]
                };
                if successor {
                    local.notes.push(domain_note(1920, "hola", "1"));
                }
                let original = local.clone();
                let ids: Vec<_> = (0..local.notes.len())
                    .map(|index| format!("n{index}"))
                    .collect();
                let direct = route(&local.notes, &ids);
                assert_eq!(
                    direct.languages[2..4],
                    [expected; 2],
                    "route conflict={conflict}, successor={successor}"
                );
                route_track(&mut local);
                assert_eq!(
                    local.notes[2..4]
                        .iter()
                        .map(|note| note.pronunciation_language)
                        .collect::<Vec<_>>(),
                    [expected; 2],
                    "route_track conflict={conflict}, successor={successor}"
                );
                let mut combined = original.clone();
                route_tracks(&mut [&mut combined]);
                assert_eq!(
                    combined.notes[2..4]
                        .iter()
                        .map(|note| note.pronunciation_language)
                        .collect::<Vec<_>>(),
                    [expected; 2],
                    "route_tracks conflict={conflict}, successor={successor}"
                );
                for track in [&mut local, &mut combined] {
                    for note in &mut track.notes {
                        note.pronunciation_language = None;
                    }
                    assert_eq!(*track, original);
                }
            }
        }
    }

    #[test]
    fn sibling_context_newer_sibling_evidence_supersedes_old_local_evidence() {
        let mut local = test_track(vec![
            domain_note(0, "bonjour", "1"),
            domain_note(960, "hou", "1"),
        ]);
        let mut newer = test_track(vec![domain_note(480, "the", "1")]);
        route_tracks(&mut [&mut local, &mut newer]);
        assert_eq!(
            local
                .notes
                .iter()
                .map(|note| note.pronunciation_language)
                .collect::<Vec<_>>(),
            [
                Some(PronunciationLanguage::French),
                Some(PronunciationLanguage::English)
            ]
        );
    }

    #[test]
    fn sibling_context_filters_barriers_before_selecting_a_donor() {
        for conflict in [false, true] {
            let mut previous = test_track(vec![domain_note(0, "the", "1")]);
            let mut barrier = test_track(vec![domain_note(
                480,
                if conflict { "bonjour" } else { "?manual" },
                "1",
            )]);
            if conflict {
                barrier.notes[0]
                    .source_evidence
                    .as_mut()
                    .unwrap()
                    .origin
                    .as_mut()
                    .unwrap()
                    .lyric_conflict = true;
            }
            let mut next = test_track(vec![domain_note(1440, "hola", "1")]);
            let mut neutral = test_track(vec![domain_note(960, "hou", "1")]);
            route_tracks(&mut [&mut previous, &mut barrier, &mut next, &mut neutral]);
            assert_eq!(
                neutral.notes[0].pronunciation_language,
                Some(PronunciationLanguage::Spanish),
                "conflict={conflict}"
            );
        }
    }

    fn set_playback_segment(note: &mut ProjectedNote, segment: u32) {
        use crate::engine::midi::{SourceContinuity, SourceEvidenceRef, SourceFormat};
        let source = &mut note
            .source_evidence
            .as_mut()
            .unwrap()
            .origin
            .as_mut()
            .unwrap()
            .source;
        source.continuity = Some(std::sync::Arc::new(SourceContinuity {
            evidence: SourceEvidenceRef {
                source_format: SourceFormat::MusicXml,
                source_version: None,
                program_version: None,
                source_id: source.id.clone(),
                raw_xml: "<note/>".into(),
            },
            chord_id: source.id.clone(),
            playback_segment: segment,
            extensions: vec![],
            incoming_tie: None,
            issues: vec![],
        }));
    }

    #[test]
    fn sibling_context_and_continuations_respect_playback_segments() {
        let mut donor = test_track(vec![domain_note(0, "the", "1")]);
        set_playback_segment(&mut donor.notes[0], 1);
        let mut neutral = test_track(vec![
            domain_note(480, "hou", "1"),
            domain_note(960, "", "1"),
        ]);
        set_playback_segment(&mut neutral.notes[0], 1);
        set_playback_segment(&mut neutral.notes[1], 2);
        neutral.notes[1].lyric = ProjectedLyric::Extension;
        let mut other_segment = test_track(vec![domain_note(480, "hou", "1")]);
        set_playback_segment(&mut other_segment.notes[0], 2);
        route_tracks(&mut [&mut donor, &mut neutral, &mut other_segment]);
        assert_eq!(
            neutral.notes[0].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
        assert_eq!(neutral.notes[1].pronunciation_language, None);
        assert_ne!(
            other_segment.notes[0].pronunciation_language,
            Some(PronunciationLanguage::English)
        );

        let mut same_track = test_track(vec![
            domain_note(0, "the", "1"),
            domain_note(480, "hou", "1"),
        ]);
        set_playback_segment(&mut same_track.notes[0], 1);
        set_playback_segment(&mut same_track.notes[1], 2);
        route_track(&mut same_track);
        assert_ne!(
            same_track.notes[1].pronunciation_language,
            Some(PronunciationLanguage::English)
        );
    }

    #[test]
    fn sibling_context_cannot_cross_manual_hint_or_competing_owners() {
        for barrier in [true, false] {
            let mut established = test_track(vec![domain_note(0, "the", "1")]);
            let mut other = test_track(vec![domain_note(
                if barrier { 240 } else { 0 },
                if barrier { "?manual" } else { "bonjour" },
                "1",
            )]);
            let mut neutral = test_track(vec![domain_note(480, "hou", "1")]);
            let mut isolated = neutral.clone();
            route_track(&mut isolated);
            route_tracks(&mut [&mut established, &mut other, &mut neutral]);
            assert_eq!(neutral, isolated);
        }
    }

    #[test]
    fn sibling_owner_reaches_extensions_and_splits_but_stops_at_source_boundaries() {
        for stop in 0..4 {
            let mut established = test_track(vec![domain_note(0, "the", "1")]);
            let mut neutral = test_track(vec![
                domain_note(480, "hou", "1"),
                domain_note(960, "", "1"),
                domain_note(1440, "", "1"),
                domain_note(1920, "", "1"),
            ]);
            neutral.notes[1].lyric = ProjectedLyric::Extension;
            let ProjectedLyric::Source(source) = &mut neutral.notes[2].lyric else {
                unreachable!()
            };
            source.state = LyricState::SyllableSplit;
            match stop {
                0 => neutral.notes[3] = domain_note(1920, "bonjour", "2"),
                1 => {
                    neutral.notes[3] = domain_note(1920, "", "2");
                    neutral.notes[3].lyric = ProjectedLyric::Extension;
                }
                2 => {
                    neutral.notes[3].onset_ticks += 480;
                    neutral.notes[3].lyric = ProjectedLyric::Extension;
                }
                _ => {
                    let ProjectedLyric::Source(source) = &mut neutral.notes[3].lyric else {
                        unreachable!()
                    };
                    source.state = LyricState::Continuation;
                    source.verse += 1;
                }
            }
            let original = neutral.clone();
            let diagnostics = route_tracks(&mut [&mut established, &mut neutral]);
            for note in &neutral.notes[..3] {
                assert_eq!(
                    note.pronunciation_language,
                    Some(PronunciationLanguage::English),
                    "stop {stop}"
                );
            }
            assert_ne!(
                neutral.notes[3].pronunciation_language,
                Some(PronunciationLanguage::English),
                "stop {stop}"
            );
            assert_eq!(
                diagnostics[1].iter().filter(|d| d.code == ROUTED).count(),
                1
            );
            for note in &mut neutral.notes {
                note.pronunciation_language = None;
            }
            assert_eq!(neutral, original);
        }
    }

    #[test]
    fn repeated_shared_word_fragments_agree_with_complete_word_routing() {
        let mut notes = vec![note(0, "we"), note(480, "hear"), note(960, "the")];
        for (index, word) in ["Ra", "di", "o", "Ra", "di", "o"].into_iter().enumerate() {
            let mut fragment = note((index as u32 + 3) * 480, word);
            let ProjectedLyric::Source(source) = &mut fragment.lyric else {
                unreachable!()
            };
            source.syllabic = Some(match index % 3 {
                0 => Syllabic::Begin,
                1 => Syllabic::Middle,
                _ => Syllabic::End,
            });
            notes.push(fragment);
        }
        let ids: Vec<_> = (0..notes.len()).map(|i| format!("n{i}")).collect();
        // Both the declared English phrase and complete-word ownership must
        // survive capitalized repetitions and source-attested syllable splits.
        let whole = languages(&["we", "hear", "the", "Radio", "Radio"]).languages;
        assert_eq!(whole, vec![Some(PronunciationLanguage::English); 5]);
        for spelling in ["radio", "Radio", "RADIO"] {
            assert_eq!(
                languages(&["we", "hear", "the", spelling, spelling]).languages,
                vec![Some(PronunciationLanguage::English); 5]
            );
            assert_eq!(
                languages(&["nous", "écoutons", "une", spelling, spelling]).languages,
                vec![Some(PronunciationLanguage::French); 5]
            );
        }
        let expected: Vec<_> = whole[..3]
            .iter()
            .copied()
            .chain(
                whole[3..]
                    .iter()
                    .flat_map(|owner| std::iter::repeat_n(*owner, 3)),
            )
            .collect();
        assert_eq!(route(&notes, &ids).languages, expected);
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
    fn track_context_keeps_a_low_confidence_french_island_french_across_a_rest() {
        let mut track = test_track(vec![
            domain_note(0, "dans", "1"),
            domain_note(480, "des", "1"),
            domain_note(960, "trompettes", "1"),
            domain_note(3840, "ou", "1"),
            domain_note(4320, "àforce", "1"),
            domain_note(4800, "de", "1"),
            domain_note(5280, "murmures", "1"),
        ]);
        let diagnostics = route_track(&mut track);
        assert_eq!(
            track
                .notes
                .iter()
                .map(|note| note.pronunciation_language)
                .collect::<Vec<_>>(),
            vec![Some(PronunciationLanguage::French); 7]
        );
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == LOW_CONFIDENCE
                && diagnostic.source_id.as_deref() == Some("note-1-3840")
                && diagnostic.message.contains("French")
                && diagnostic.message.contains("surrounding passage")
        }));
    }

    #[test]
    fn track_context_keeps_unaccented_french_reves_with_its_french_row() {
        let mut track = test_track(vec![
            domain_note(0, "au", "1"),
            domain_note(480, "bout", "1"),
            domain_note(960, "de", "1"),
            domain_note(1440, "mes", "1"),
            domain_note(1920, "reves", "1"),
            domain_note(2400, "j'irai", "1"),
        ]);
        route_track(&mut track);
        assert_eq!(
            track
                .notes
                .iter()
                .map(|note| note.pronunciation_language)
                .collect::<Vec<_>>(),
            vec![Some(PronunciationLanguage::French); 6]
        );
    }

    #[test]
    fn track_context_keeps_monolingual_rows_stable_across_rests_in_all_four_languages() {
        for (words, expected) in [
            (
                ["bonjour", "ou", "de", "murmures", "merci"],
                PronunciationLanguage::French,
            ),
            (
                ["the", "on", "radio", "with", "friends"],
                PronunciationLanguage::English,
            ),
            (
                ["hola", "de", "la", "vida", "gracias"],
                PronunciationLanguage::Spanish,
            ),
            (
                ["olá", "de", "uma", "vida", "obrigado"],
                PronunciationLanguage::Portuguese,
            ),
        ] {
            let mut track = test_track(
                words
                    .iter()
                    .enumerate()
                    .map(|(index, word)| domain_note(index as u32 * 960, word, "1"))
                    .collect(),
            );
            route_track(&mut track);
            assert_eq!(
                track
                    .notes
                    .iter()
                    .map(|note| note.pronunciation_language)
                    .collect::<Vec<_>>(),
                vec![Some(expected); words.len()],
                "{words:?}"
            );
        }
    }

    #[test]
    fn track_context_preserves_real_language_switches_with_ambiguous_words() {
        let passages = [
            (["bonjour", "de", "merci"], PronunciationLanguage::French),
            (["the", "on", "baby"], PronunciationLanguage::English),
            (["hola", "de", "gracias"], PronunciationLanguage::Spanish),
            (["olá", "de", "obrigado"], PronunciationLanguage::Portuguese),
        ];
        let mut notes = Vec::new();
        let mut expected = Vec::new();
        for (passage_index, (words, language)) in passages.into_iter().enumerate() {
            let base = passage_index as u32 * 2400;
            for (word_index, word) in words.into_iter().enumerate() {
                notes.push(domain_note(base + word_index as u32 * 480, word, "1"));
                expected.push(Some(language));
            }
        }
        let mut track = test_track(notes);
        route_track(&mut track);
        assert_eq!(
            track
                .notes
                .iter()
                .map(|note| note.pronunciation_language)
                .collect::<Vec<_>>(),
            expected
        );
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
    fn cross_domain_split_portuguese_word_keeps_its_local_passage_owner() {
        for (previous, passage, expected) in [
            (
                "et",
                ["você", "não", "está", "sozinho"],
                PronunciationLanguage::Portuguese,
            ),
            (
                "the",
                ["você", "não", "está", "sozinho"],
                PronunciationLanguage::Portuguese,
            ),
            (
                "hola",
                ["você", "não", "está", "sozinho"],
                PronunciationLanguage::Portuguese,
            ),
        ] {
            let confidence = multilingual_scores("contigo")[expected.index()];
            assert!(confidence < 0.7, "{expected:?}: {confidence}");
            let mut notes = vec![
                domain_note(0, previous, "1"),
                domain_note(480, "con", "2"),
                domain_note(960, "tigo", "2"),
                domain_note(1440, "-", "2"),
            ];
            for (index, syllabic) in [(1, Syllabic::Begin), (2, Syllabic::End)] {
                let ProjectedLyric::Source(source) = &mut notes[index].lyric else {
                    unreachable!()
                };
                source.syllabic = Some(syllabic);
            }
            let ProjectedLyric::Source(source) = &mut notes[3].lyric else {
                unreachable!()
            };
            source.state = LyricState::Continuation;
            notes.extend(
                passage
                    .iter()
                    .enumerate()
                    .map(|(index, word)| domain_note((index as u32 + 4) * 480, word, "2")),
            );
            let ids: Vec<_> = (1..notes.len()).map(|index| format!("n{index}")).collect();
            let local = route(&notes[1..], &ids);
            assert_eq!(local.languages, vec![Some(expected); notes.len() - 1]);
            assert!(!local.inherit_passage[0]);
            let original = notes.clone();
            let mut track = ProjectedTrack {
                source_track_id: "track".into(),
                name: "Voice".into(),
                muted: false,
                notes,
            };
            route_track(&mut track);
            assert_eq!(
                track.notes[0].pronunciation_language,
                automatic_anchor(previous)
            );
            for (index, (actual, source)) in track.notes.iter().zip(original).enumerate() {
                if index > 0 {
                    assert_eq!(
                        actual.pronunciation_language,
                        Some(expected),
                        "{previous}/{index}"
                    );
                }
                let mut restored = actual.clone();
                restored.pronunciation_language = None;
                assert_eq!(restored, source);
            }
        }
    }

    #[test]
    fn hard_automatic_anchors_clear_local_uncertainty() {
        for key in ["bonjour", "the", "hola", "você"] {
            let notes = [note(0, key)];
            let ids = ["anchor".into()];
            let mut units = build_units(&notes, &ids);
            assert_eq!(units.len(), 1);
            // Exercise stale uncertainty from the earlier scoring pass.
            units[0].local_uncertain = true;
            units[0].inherit_passage = true;
            extend_language_scores(&mut units, &notes);
            assert!(!units[0].local_uncertain, "{key}");
            assert!(!units[0].inherit_passage, "{key}");
            assert_eq!(decode(&units), [automatic_anchor(key).unwrap()]);
            let routed = route(&notes, &ids);
            assert_eq!(routed.languages, [automatic_anchor(key)]);
            assert!(
                !routed
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == LOW_CONFIDENCE),
                "{key}"
            );
        }
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
