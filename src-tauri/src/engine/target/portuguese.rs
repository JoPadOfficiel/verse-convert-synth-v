//! Portuguese routing membership plus pinned Brazil/Portugal MFA readings.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::lexical;

const COMMUNITY: &str = include_str!("portuguese-community.tsv");
const PRONUNCIATION: &str = include_str!("portuguese-pronunciation.tsv");
static COMMUNITY_INDEX: OnceLock<HashSet<&'static str>> = OnceLock::new();
static PRONUNCIATION_INDEX: OnceLock<HashMap<&'static str, Vec<(&'static str, &'static str)>>> =
    OnceLock::new();

fn index() -> &'static HashSet<&'static str> {
    COMMUNITY_INDEX.get_or_init(|| {
        COMMUNITY
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .filter_map(|line| line.split_once('\t').map(|(word, _)| word))
            .collect()
    })
}

pub(crate) fn contains_lexeme(key: &str) -> bool {
    let key = lexical::normalize(key);
    !key.is_empty() && index().contains(key.as_str())
}

fn pronunciation_index() -> &'static HashMap<&'static str, Vec<(&'static str, &'static str)>> {
    PRONUNCIATION_INDEX.get_or_init(|| {
        let mut index: HashMap<&'static str, Vec<(&'static str, &'static str)>> = HashMap::new();
        for line in PRONUNCIATION
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
        {
            let mut fields = line.split('\t');
            let (Some(word), Some(dialect), Some(phones), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            index.entry(word).or_default().push((dialect, phones));
        }
        index
    })
}

pub(crate) fn pronunciation_readings(key: &str) -> &'static [(&'static str, &'static str)] {
    let key = lexical::normalize(key);
    pronunciation_index()
        .get(key.as_str())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub(crate) fn has_pronunciation(key: &str) -> bool {
    !pronunciation_readings(key).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_community_membership_covers_brazil_and_portugal() {
        for word in ["você", "coração", "obrigado", "portugal", "brasil"] {
            assert!(contains_lexeme(word), "missing {word}");
        }
        assert!(!contains_lexeme("hola"));
    }

    #[test]
    fn pinned_mfa_readings_keep_brazil_and_portugal_distinct() {
        let obrigado = pronunciation_readings("obrigado");
        assert!(obrigado
            .iter()
            .any(|(dialect, phones)| { *dialect == "brazil" && *phones == "o b ɾ i ɡ a d o" }));
        assert!(obrigado.iter().any(|(dialect, phones)| {
            *dialect == "portugal" && *phones == "ɔ β ɾ i ɣ a ð u"
        }));
    }
}
