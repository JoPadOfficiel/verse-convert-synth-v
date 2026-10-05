//! Diff is a proposal, never acoustic approval or automatic promotion.
use super::{
    memory::{Correction, Provenance, Scope},
    source_map::{reading, NativeTrack, Reference},
    Error, POLICY,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Review {
    pub reference_id: String,
    pub corrected_sha256: String,
    pub proposals: Vec<Correction>,
    pub diagnostics: Vec<Error>,
}
fn lexical_key(text: &str) -> String {
    crate::engine::target::lexical::normalize(text.split('[').next().unwrap_or_default())
}

pub fn compare(
    reference: &Reference,
    corrected_sha256: String,
    corrected: &[NativeTrack],
) -> Result<Review, Error> {
    if reference.policy != POLICY {
        return Err(Error::new(
            "PRONUNCIATION_POLICY_INCOMPATIBLE",
            "Export reference policy differs",
        ));
    }
    let mut review = Review {
        reference_id: reference.id.clone(),
        corrected_sha256: corrected_sha256.clone(),
        proposals: Vec::new(),
        diagnostics: Vec::new(),
    };
    for bound in &reference.words {
        let original = &reference.tracks[bound.track];
        let spelling = |text: &str| text.split('[').next().unwrap_or_default().to_string();
        let matching: Vec<_> = corrected
            .iter()
            .filter(|t| {
                t.notes.len() == original.notes.len()
                    && t.notes.iter().zip(&original.notes).all(|(a, b)| {
                        a.position == b.position
                            && (spelling(&a.lyric) == spelling(&b.lyric)
                                || super::reading::same_word_variant(
                                    &lexical_key(&b.lyric),
                                    &spelling(&a.lyric),
                                ))
                    })
            })
            .collect();
        if matching.len() != 1 {
            review.diagnostics.push(Error::new(
                "PRONUNCIATION_MATCH_AMBIGUOUS",
                "Changed or ambiguous track geometry; correction association refused",
            ));
            continue;
        }
        let edited = matching[0];
        if bound.word.manual {
            continue;
        }
        let head = bound.notes[0];
        if bound.notes.iter().any(|&i| {
            let (a, b) = (&original.notes[i], &edited.notes[i]);
            (a.position, a.duration, a.tone) != (b.position, b.duration, b.tone)
        }) {
            review.diagnostics.push(Error::new(
                "PRONUNCIATION_MEMBER_CHANGED",
                "Changed source-word member geometry; association refused",
            ));
            continue;
        }
        // Every original split/hold and every source-owned member must remain.
        if bound.notes.iter().skip(1).any(|&i| {
            edited.notes[i].lyric != original.notes[i].lyric
                || edited.notes[i].phonemizer != original.notes[i].phonemizer
        }) {
            review.diagnostics.push(Error::new(
                "PRONUNCIATION_WORD_CHAIN_CHANGED",
                "Word tail, split or hold changed; complete association refused",
            ));
            continue;
        }
        let mut after = reading(&edited.notes[head], &edited.phonemizer)?;
        after.aliases = super::source_map::word_aliases(edited, &bound.notes);
        if after == bound.before || after.language.is_none() {
            continue;
        }
        // A language-only edit does not validate stale hint tokens under the
        // new alphabet. Keep the changed language, and let its qualified
        // dictionary authority build a fresh reading on later conversion.
        if after.language != bound.before.language && after.phones == bound.before.phones {
            after.phones = None;
            after.alphabet = None;
            after.authority = None;
        }
        // A phonetic hint may replace a reading, never silently replace spelling.
        let before_text = bound.before.lexical_reading.as_deref().unwrap_or_default();
        if after.lexical_reading.as_deref() != Some(before_text)
            && after.lexical_reading.as_deref() != Some(&bound.word.key)
        {
            let Some(key) = after.lexical_reading.as_deref() else {
                continue;
            };
            if super::reading::same_word_variant(&bound.word.key, key) {
                // Retain native variant labels as review evidence; their
                // numeric suffix alone never selects a Verse pronunciation.
            } else {
                review.diagnostics.push(Error::new(
                    "PRONUNCIATION_WORD_REPLACED",
                    "Changed spelling is not an associated pronunciation correction",
                ));
                continue;
            }
        }
        if after.aliases.is_none() {
            if let (Some(language), Some(key), Some(phones)) = (
                after.language,
                after.lexical_reading.as_deref(),
                after.phones.as_ref(),
            ) {
                if super::reading::known_reading(&bound.word.key, key, language).is_ok_and(|hint| {
                    phones
                        .iter()
                        .map(String::as_str)
                        .eq(hint.split_whitespace())
                }) {
                    after.authority = Some(super::reading::authority(language));
                }
            }
        }
        let accepted_variant = after
            .lexical_reading
            .clone()
            .filter(|key| key != before_text && key != &bound.word.key);
        let symbol_validation = if after.authority.is_some() {
            "base_inventory_valid"
        } else {
            "unknown"
        };
        let mut correction = Correction {
            id: String::new(),
            fingerprint: String::new(),
            word: bound.word.clone(),
            before: bound.before.clone(),
            after,
            dialect: None,
            accepted_variant,
            scope: Scope::OccurrenceOnly,
            target: "ustx".into(),
            profile: "automatic".into(),
            voice: None,
            provenance: Provenance {
                source_sha256: reference.source_sha256.clone(),
                export_sha256: reference.export_sha256.clone(),
                corrected_sha256: corrected_sha256.clone(),
                confirmed_after_listening: false,
                symbol_validation: symbol_validation.into(),
                policy: POLICY.into(),
                observed_singer: edited
                    .singer
                    .clone()
                    .filter(|singer| !singer.trim().is_empty()),
            },
        };
        correction.seal()?;
        review.proposals.push(correction);
    }
    Ok(review)
}
