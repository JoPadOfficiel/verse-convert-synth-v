//! Spanish/Portuguese DiffSinger pronunciation hints from pinned MFA evidence.
//!
//! The bundled MFA dictionaries are the lexical pronunciation authority. This
//! module lowers only source phones whose meaning is represented explicitly by
//! OpenUtau's built-in DiffSinger Spanish/Portuguese symbol inventories. A word
//! is hinted only when every bundled reading is representable and all readings
//! converge on one target sequence. Otherwise the source spelling is retained
//! and OpenUtau may use its own singer-side fallback after Verse has reported a
//! stable diagnostic; Verse never guesses a lossy phone replacement.

use std::collections::BTreeSet;

use crate::engine::convert::{Diagnostic, DiagnosticSeverity};
use crate::engine::midi::Syllabic;
use crate::engine::projection::{ProjectedNote, PronunciationLanguage};

use super::{lexical, portuguese, spanish};

pub const FRENCH_NAME: &str = "DiffSinger French Millefeuille Phonemizer";
pub const ENGLISH_NAME: &str = "DiffSinger English Phonemizer";
pub const SPANISH_NAME: &str = "DiffSinger Spanish Phonemizer";
pub const PORTUGUESE_NAME: &str = "DiffSinger Portuguese Phonemizer";

pub const SPANISH_PHONEMIZER: &str = "OpenUtau.Core.DiffSinger.DiffSingerSpanishPhonemizer";
pub const PORTUGUESE_PHONEMIZER: &str = "OpenUtau.Core.DiffSinger.DiffSingerPortuguesePhonemizer";

pub const APPLIED: &str = "DIFFSINGER_PRONUNCIATION_APPLIED";
pub const UNKNOWN: &str = "DIFFSINGER_PRONUNCIATION_UNKNOWN";
pub const UNMAPPABLE: &str = "DIFFSINGER_PRONUNCIATION_UNMAPPABLE";
pub const AMBIGUOUS: &str = "DIFFSINGER_PRONUNCIATION_AMBIGUOUS";
pub const INCOMPLETE: &str = "DIFFSINGER_PRONUNCIATION_INCOMPLETE_WORD";
pub const SYLLABLE_MISMATCH: &str = "DIFFSINGER_PRONUNCIATION_SYLLABLE_MISMATCH";

fn language_name(language: PronunciationLanguage) -> &'static str {
    match language {
        PronunciationLanguage::Spanish => "Spanish",
        PronunciationLanguage::Portuguese => "Portuguese",
        PronunciationLanguage::French => "French",
        PronunciationLanguage::English => "English",
    }
}

fn readings(language: PronunciationLanguage, key: &str) -> &'static [(&'static str, &'static str)] {
    match language {
        PronunciationLanguage::Spanish => spanish::pronunciation_readings(key),
        PronunciationLanguage::Portuguese => portuguese::pronunciation_readings(key),
        PronunciationLanguage::French | PronunciationLanguage::English => &[],
    }
}

fn has_reading(language: PronunciationLanguage, key: &str) -> bool {
    match language {
        PronunciationLanguage::Spanish => spanish::has_pronunciation(key),
        PronunciationLanguage::Portuguese => portuguese::has_pronunciation(key),
        PronunciationLanguage::French | PronunciationLanguage::English => false,
    }
}

fn spanish_vowel(phone: &str) -> bool {
    matches!(
        phone,
        "a" | "e" | "i" | "o" | "u" | "ã" | "ẽ" | "ĩ" | "õ" | "ũ"
    )
}

/// Lower one MFA Spanish reading to OpenUtau's documented base Spanish G2P
/// alphabet. `I`/`U` are off-glides while `y`/`w` are onset glides in the stock
/// dictionary, so their choice is contextual rather than a global substitution.
fn map_spanish_reading(source: &str) -> Result<String, String> {
    let phones: Vec<&str> = source.split_whitespace().collect();
    let mut target = Vec::with_capacity(phones.len());
    for (index, phone) in phones.iter().copied().enumerate() {
        let mapped = match phone {
            "a" => "a",
            "b" => "b",
            "β" => "B",
            "d̪" => "d",
            "ð" => "D",
            "e" => "e",
            "f" => "f",
            "ɡ" => "g",
            "ɣ" => "G",
            "i" => "i",
            "j" => {
                if phones
                    .get(index + 1)
                    .is_some_and(|next| spanish_vowel(next))
                {
                    "y"
                } else {
                    "I"
                }
            }
            "k" => "k",
            "l" => "l",
            "m" => "m",
            "n" => "n",
            "o" => "o",
            "p" => "p",
            // MFA uses /r/ for the trill and /ɾ/ for the tap. OpenUtau's stock
            // Spanish dictionary spells those `rr` and `r` respectively.
            "r" => "rr",
            "ɾ" => "r",
            "s" => "s",
            "tʃ" => "ch",
            "t̪" => "t",
            "u" => "u",
            "w" => {
                if phones
                    .get(index + 1)
                    .is_some_and(|next| spanish_vowel(next))
                {
                    "w"
                } else {
                    "U"
                }
            }
            "x" => "x",
            "ɲ" => "gn",
            "ʎ" => "ll",
            // The pinned packs agree on this Spanish distinction: MFA /ʝ/
            // appears in words such as `ayer`, `mayo` and `playa`, where
            // OpenUtau uses `y`; MFA /ɟʝ/ appears in `yo`, `ya` and `hielo`,
            // where OpenUtau uses the strengthened `Y` symbol.
            "ʝ" => "y",
            "ɟʝ" => "Y",
            "θ" => "z",
            // Nasalized vowels, palatal stops/affricates, assimilated nasals and
            // aspirated/voiced sibilants have no one-to-one symbol in the stock
            // Spanish DiffSinger inventory. Do not erase those distinctions.
            unsupported => return Err(unsupported.to_string()),
        };
        target.push(mapped);
    }
    Ok(target.join(" "))
}

/// Lower one MFA Portuguese reading to OpenUtau's documented base Portuguese
/// G2P alphabet. The stock alphabet represents the open vowels and the common
/// palatal/nasal phones directly, but not European Portuguese [ɐ, ɨ, β, ð, ɣ]
/// or a dialect-independent strong-r realization.
fn map_portuguese_reading(source: &str) -> Result<String, String> {
    let mut target = Vec::new();
    for phone in source.split_whitespace() {
        let mapped = match phone {
            "a" => "a",
            "b" => "b",
            "d" => "d",
            "dʒ" => "dZ",
            "e" => "e",
            "ẽ" => "e~",
            "f" => "f",
            "i" => "i",
            "ĩ" => "i~",
            "j" => "j",
            "j̃" => "j~",
            "k" => "k",
            "l" => "l",
            "m" => "m",
            "n" => "n",
            "o" => "o",
            "õ" => "o~",
            "p" => "p",
            "s" => "s",
            "t" => "t",
            "tʃ" => "tS",
            "u" => "u",
            "ũ" => "u~",
            "v" => "v",
            "w" => "w",
            "w̃" => "w~",
            // The pinned OpenUtau PT pack uses `X` for the same Brazilian
            // /x/ readings that MFA records in words such as `mar`, `lugar`
            // and `melhor`.
            "x" => "X",
            "z" => "z",
            "ɐ̃" => "a~",
            "ɔ" => "O",
            "ɛ" => "E",
            "ɡ" => "g",
            "ɲ" => "J",
            "ɾ" => "r",
            "ʃ" => "S",
            "ʎ" => "L",
            "ʒ" => "Z",
            unsupported => return Err(unsupported.to_string()),
        };
        target.push(mapped);
    }
    Ok(target.join(" "))
}

fn map_reading(language: PronunciationLanguage, source: &str) -> Result<String, String> {
    match language {
        PronunciationLanguage::Spanish => map_spanish_reading(source),
        PronunciationLanguage::Portuguese => map_portuguese_reading(source),
        PronunciationLanguage::French | PronunciationLanguage::English => {
            Err("unsupported language route".into())
        }
    }
}

fn target_vowel_count(language: PronunciationLanguage, hint: &str) -> usize {
    hint.split_whitespace()
        .filter(|phone| match language {
            PronunciationLanguage::Spanish => matches!(*phone, "a" | "e" | "i" | "o" | "u"),
            PronunciationLanguage::Portuguese => matches!(
                *phone,
                "E" | "O" | "a" | "a~" | "e" | "e~" | "i" | "i~" | "o" | "o~" | "u" | "u~"
            ),
            PronunciationLanguage::French | PronunciationLanguage::English => false,
        })
        .count()
}

enum Resolution {
    Exact(String),
    Unknown,
    Unmappable(Vec<String>),
    Ambiguous(Vec<String>),
}

fn resolve(language: PronunciationLanguage, key: &str) -> Resolution {
    let source_readings = readings(language, key);
    if source_readings.is_empty() {
        return Resolution::Unknown;
    }
    let mut mapped = BTreeSet::new();
    let mut failures = Vec::new();
    for (dialect, source) in source_readings {
        match map_reading(language, source) {
            Ok(hint) => {
                mapped.insert(hint);
            }
            Err(phone) => failures.push(format!("{dialect}: {source} (unsupported {phone})")),
        }
    }
    if !failures.is_empty() {
        return Resolution::Unmappable(failures);
    }
    if mapped.len() != 1 {
        return Resolution::Ambiguous(mapped.into_iter().collect());
    }
    Resolution::Exact(mapped.pop_first().expect("one exact pronunciation"))
}

fn diagnostic(code: &str, severity: DiagnosticSeverity, message: String, id: &str) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity,
        message,
        source_id: Some(id.into()),
    }
}

fn resolution_diagnostic(
    language: PronunciationLanguage,
    key: &str,
    resolution: &Resolution,
    id: &str,
) -> Diagnostic {
    let name = language_name(language);
    match resolution {
        Resolution::Unknown => diagnostic(
            UNKNOWN,
            DiagnosticSeverity::Warning,
            format!(
                "{name} DiffSinger: {key:?} has no reading in the pinned MFA pronunciation dictionaries; source spelling is retained."
            ),
            id,
        ),
        Resolution::Unmappable(readings) => diagnostic(
            UNMAPPABLE,
            DiagnosticSeverity::Warning,
            format!(
                "{name} DiffSinger: {key:?} has MFA reading(s) that cannot be represented exactly by OpenUtau's base {name} DiffSinger phone inventory ({}); source spelling is retained and no approximate hint is emitted.",
                readings.join("; ")
            ),
            id,
        ),
        Resolution::Ambiguous(hints) => diagnostic(
            AMBIGUOUS,
            DiagnosticSeverity::Warning,
            format!(
                "{name} DiffSinger: {key:?} has multiple exact MFA-derived target readings ({}) with no source-owned dialect selector; source spelling is retained rather than choosing one.",
                hints.join(" | ")
            ),
            id,
        ),
        Resolution::Exact(_) => unreachable!("exact readings are applied, not diagnosed as failures"),
    }
}

fn fallback_join_word(notes: &mut [ProjectedNote], members: &[usize]) {
    // Even when a safe external hint cannot be emitted, preserve the source's
    // proven complete-word ownership for OpenUtau. An empty hint leaves the
    // consumer free to use singer-side fallback without Verse inventing phones.
    let spelling: String = members
        .iter()
        .filter_map(|&index| lexical::raw_text(&notes[index].lyric))
        .collect();
    lexical::pronounce_word(notes, members, &spelling, "");
}

/// Apply a pinned Spanish/Portuguese pronunciation pass to one source domain.
pub(crate) fn apply(
    notes: &mut [ProjectedNote],
    note_ids: &[String],
    language: PronunciationLanguage,
    automatic_recovery: bool,
) -> Vec<Diagnostic> {
    assert_eq!(notes.len(), note_ids.len());
    assert!(matches!(
        language,
        PronunciationLanguage::Spanish | PronunciationLanguage::Portuguese
    ));
    let mut diagnostics = Vec::new();
    let mut handled = vec![false; notes.len()];
    let fragments = lexical::fragments(notes);
    let words = if automatic_recovery {
        lexical::automatic_words(notes)
    } else {
        lexical::words(notes)
    };

    for members in words {
        let key = if automatic_recovery {
            lexical::preferred_joined_key(notes, &members, |candidate| {
                has_reading(language, candidate)
            })
        } else {
            lexical::joined_key(notes, &members)
        };
        let resolution = resolve(language, &key);
        match &resolution {
            Resolution::Exact(hint) => {
                let vowels = target_vowel_count(language, hint);
                if members.len() > 1 && vowels != members.len() {
                    fallback_join_word(notes, &members);
                    diagnostics.push(diagnostic(
                        SYLLABLE_MISMATCH,
                        DiagnosticSeverity::Warning,
                        format!(
                            "{} DiffSinger: {key:?} maps exactly to [{hint}] but has {vowels} target vowel nuclei for {} source syllable attacks; source word ownership is retained without an explicit hint.",
                            language_name(language),
                            members.len()
                        ),
                        &note_ids[members[0]],
                    ));
                } else {
                    lexical::pronounce_word(notes, &members, &key, hint);
                    diagnostics.push(diagnostic(
                        APPLIED,
                        DiagnosticSeverity::Info,
                        format!(
                            "{} DiffSinger: pinned MFA reading for {key:?} maps exactly to [{hint}]; original lyric evidence and note geometry are preserved.",
                            language_name(language)
                        ),
                        &note_ids[members[0]],
                    ));
                }
            }
            _ => {
                fallback_join_word(notes, &members);
                diagnostics.push(resolution_diagnostic(
                    language,
                    &key,
                    &resolution,
                    &note_ids[members[0]],
                ));
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
            diagnostics.push(diagnostic(
                INCOMPLETE,
                DiagnosticSeverity::Warning,
                format!(
                    "{} DiffSinger: source fragment {:?} is not a proven complete word; no independent pronunciation reading is applied.",
                    language_name(language),
                    lexical::raw_text(&notes[index].lyric).unwrap_or_default()
                ),
                &note_ids[index],
            ));
            continue;
        }
        let resolution = resolve(language, &key);
        match &resolution {
            Resolution::Exact(hint) => {
                lexical::pronounce(&mut notes[index], &key, hint);
                diagnostics.push(diagnostic(
                    APPLIED,
                    DiagnosticSeverity::Info,
                    format!(
                        "{} DiffSinger: pinned MFA reading for {key:?} maps exactly to [{hint}]; original lyric evidence and note geometry are preserved.",
                        language_name(language)
                    ),
                    &note_ids[index],
                ));
            }
            _ => diagnostics.push(resolution_diagnostic(
                language,
                &key,
                &resolution,
                &note_ids[index],
            )),
        }
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_exact_map_preserves_trill_tap_and_community_allophones() {
        assert_eq!(map_spanish_reading("p e r o").unwrap(), "p e rr o");
        assert_eq!(map_spanish_reading("p e ɾ o").unwrap(), "p e r o");
        assert_eq!(map_spanish_reading("k a ð a").unwrap(), "k a D a");
        assert_eq!(map_spanish_reading("l a ɣ o").unwrap(), "l a G o");
        assert_eq!(map_spanish_reading("s a β e").unwrap(), "s a B e");
        assert_eq!(map_spanish_reading("m a ʝ o").unwrap(), "m a y o");
        assert_eq!(map_spanish_reading("ɟʝ o").unwrap(), "Y o");
    }

    #[test]
    fn spanish_glides_use_the_stock_onset_and_offglide_symbols() {
        assert_eq!(map_spanish_reading("b w e n o").unwrap(), "b w e n o");
        assert_eq!(map_spanish_reading("k a w s a").unwrap(), "k a U s a");
        assert_eq!(map_spanish_reading("s j e l o").unwrap(), "s y e l o");
        assert_eq!(map_spanish_reading("a j ɾ e").unwrap(), "a I r e");
    }

    #[test]
    fn rich_unrepresented_phones_fail_closed() {
        assert_eq!(map_spanish_reading("k ã n θ j õ n"), Err("ã".into()));
        assert_eq!(map_portuguese_reading("ɔ β ɾ i ɣ a ð u"), Err("β".into()));
        assert_eq!(map_portuguese_reading("n o j t ɨ"), Err("ɨ".into()));
    }

    #[test]
    fn portuguese_exact_map_covers_stock_nasal_open_and_palatal_symbols() {
        assert_eq!(map_portuguese_reading("a ʃ e j").unwrap(), "a S e j");
        assert_eq!(map_portuguese_reading("m a x").unwrap(), "m a X");
        assert_eq!(
            map_portuguese_reading("a l ɲ o ɐ̃ w̃").unwrap(),
            "a l J o a~ w~"
        );
        assert_eq!(map_portuguese_reading("k ɐ z ɐ"), Err("ɐ".into()));
    }

    #[test]
    fn dictionary_resolution_requires_all_variants_to_converge() {
        assert!(matches!(
            resolve(PronunciationLanguage::Spanish, "hola"),
            Resolution::Exact(ref hint) if hint == "o l a"
        ));
        assert!(matches!(
            resolve(PronunciationLanguage::Spanish, "canción"),
            Resolution::Unmappable(_)
        ));
        assert!(matches!(
            resolve(PronunciationLanguage::Portuguese, "você"),
            Resolution::Ambiguous(_)
        ));
        assert!(matches!(
            resolve(PronunciationLanguage::Portuguese, "zzq-not-a-word"),
            Resolution::Unknown
        ));
    }
}
