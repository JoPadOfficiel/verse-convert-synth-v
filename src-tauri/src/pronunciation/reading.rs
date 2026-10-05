//! Confirmed lexical keys still use the existing pronunciation authorities.
use super::{Error, Language};
use crate::engine::{
    projection::{ProjectedLyric, ProjectedTrack},
    target::{diffsinger, english, french, lexical},
};

pub fn authority(language: Language) -> String {
    use std::sync::OnceLock;
    static IDENTITIES: OnceLock<[String; 4]> = OnceLock::new();
    let values = IDENTITIES.get_or_init(|| {
        [
            super::hash(
                &[
                    include_bytes!("../engine/target/french-lexicon.tsv").as_slice(),
                    include_bytes!("../engine/target/french-community.tsv").as_slice(),
                ]
                .concat(),
            ),
            super::hash(include_bytes!("../engine/target/english-lexicon.tsv")),
            super::hash(include_bytes!("../engine/target/spanish-pronunciation.tsv")),
            super::hash(include_bytes!(
                "../engine/target/portuguese-pronunciation.tsv"
            )),
        ]
    });
    format!(
        "{}:dictionary:{}",
        super::POLICY,
        values[super::memory::language_index(language)]
    )
}

pub fn attested(word: &str, reading: &super::source_map::Reading) -> bool {
    if reading.aliases.is_some() {
        return false;
    }
    let (Some(language), Some(key), Some(phones), Some(identity)) = (
        reading.language,
        reading.lexical_reading.as_deref(),
        reading.phones.as_ref(),
        reading.authority.as_deref(),
    ) else {
        return false;
    };
    identity == authority(language)
        && known_reading(word, key, language).is_ok_and(|hint| {
            phones
                .iter()
                .map(String::as_str)
                .eq(hint.split_whitespace())
        })
}

pub fn same_word_variant(word: &str, key: &str) -> bool {
    let normalized = lexical::normalize(key);
    let variant = normalized
        .strip_prefix(word)
        .and_then(|suffix| suffix.strip_prefix('('))
        .and_then(|s| s.strip_suffix(')'))
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    normalized == word || variant
}

pub fn known_reading(word: &str, key: &str, language: Language) -> Result<String, Error> {
    let normalized = lexical::normalize(key);
    if !same_word_variant(word, key) {
        return Err(Error::new(
            "PRONUNCIATION_WORD_REPLACED",
            "A reading key must name the same source word or an attested literal variant",
        ));
    }
    let hint = match language {
        Language::French => french::lexical(&normalized),
        Language::English => english::reading(&normalized).ok().map(str::to_owned),
        Language::Spanish | Language::Portuguese => {
            diffsinger::confirmed_reading(language.projected(), &normalized)
        }
    };
    hint.ok_or_else(||Error::new("PRONUNCIATION_READING_UNSUPPORTED","Selected reading is absent, ambiguous or unrepresentable in the pinned pronunciation authority"))
}

pub fn apply(
    track: &mut ProjectedTrack,
    snapshot: &super::Snapshot,
    words: &[super::source_map::Word],
) -> Vec<crate::engine::convert::Diagnostic> {
    let mut diagnostics = Vec::new();
    if snapshot.corrections.is_empty() {
        return diagnostics;
    }
    for word in words {
        let correction = match snapshot
            .corrections
            .find(word, &snapshot.source_sha256, None)
        {
            Ok(Some(c)) => c,
            Err(_) | Ok(None) => continue,
        };
        if !track.notes.iter().any(|n| word.matches_head(n)) {
            continue;
        }
        let selected = correction
            .accepted_variant
            .as_deref()
            .or(correction.after.lexical_reading.as_deref());
        let Some(key) = selected.filter(|key| {
            Some(*key) != correction.before.lexical_reading.as_deref()
                || correction.accepted_variant.is_some()
                || attested(&word.key, &correction.after)
        }) else {
            if correction.after.phones.is_some() || correction.after.aliases.is_some() {
                diagnostics.push(diagnostic("PRONUNCIATION_PHONE_REUSE_UNQUALIFIED","Custom phone reading retained without automatic reuse; qualified voice inventories are required",&word.id));
            }
            continue;
        };
        let language = correction.after.language.unwrap();
        let result=if !attested(&word.key,&correction.after){Err(Error::new("PRONUNCIATION_READING_AUTHORITY_UNQUALIFIED","A native variant label is not a Verse reading; exact explicit phone evidence and a matching pinned authority are required"))}else{known_reading(&word.key,key,language)}.and_then(|hint|{
            let vowels=match language {Language::French|Language::English=>lexical::vowel_count(&hint),_=>diffsinger::target_vowel_count(language.projected(),&hint)};
            if vowels!=word.attacks{return Err(Error::new("PRONUNCIATION_READING_ALLOCATION","Known reading vowel count differs from source-owned attacks; holds do not consume vowels"));}
            if correction.after.phones.as_ref().is_some_and(|phones|phones.iter().map(String::as_str).ne(hint.split_whitespace())) {
                return Err(Error::new("PRONUNCIATION_PHONE_REUSE_UNQUALIFIED","Custom phone sequence differs from the known authority and cannot be promoted to an automatic hint"));
            }
            Ok(hint)
        });
        match result {
            Ok(hint) => {
                let mut applied = 0;
                for note in &mut track.notes {
                    if !note
                        .source_evidence
                        .as_ref()
                        .is_some_and(|e| word.members.contains(&e.note_id))
                    {
                        continue;
                    }
                    let hold = matches!(&note.lyric, ProjectedLyric::Extension)
                        || matches!(&note.lyric,ProjectedLyric::Source(s) if matches!(s.state,crate::engine::midi::LyricState::Continuation));
                    if hold {
                        continue;
                    }
                    let Some(source) = lexical::source(&note.lyric).cloned() else {
                        continue;
                    };
                    if word.matches_head(note) {
                        note.lyric = ProjectedLyric::Pronounced {
                            source: Box::new(source),
                            text: word.key.clone(),
                            phonemes: hint.clone(),
                        };
                    } else {
                        note.lyric = ProjectedLyric::PronouncedSplit {
                            source: Box::new(source),
                        };
                    }
                    applied += 1;
                }
                if applied == word.attacks {
                    diagnostics.push(diagnostic("PRONUNCIATION_READING_MEMORY_APPLIED","Confirmed dictionary-attested reading applied without changing source words or musical geometry",&word.id));
                }
            }
            Err(e) => diagnostics.push(diagnostic(&e.code, &e.message, &word.id)),
        }
    }
    diagnostics
}
fn diagnostic(code: &str, message: &str, id: &str) -> crate::engine::convert::Diagnostic {
    crate::engine::convert::Diagnostic {
        code: code.into(),
        severity: crate::engine::convert::DiagnosticSeverity::Info,
        message: message.into(),
        source_id: Some(id.into()),
    }
}
