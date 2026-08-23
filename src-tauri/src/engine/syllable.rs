//! Reassembles the syllables a source spreads over several notes back into the
//! word they spell.
//!
//! A score writes one syllable per note and states the binding either with
//! `<syllabic>` (MuseScore, MusicXML) or with a hyphen inside the text itself
//! (MIDI/KAR exporters, and scores where the hyphen was typed by hand). Both
//! targets ask for the opposite shape: the whole word on the first note, and a
//! syllable-split marker on every following one — `+` in OpenUtau and in
//! Synthesizer V. A phonemizer looks the lyric up in a pronunciation
//! dictionary, so `"mê"` and `"me"` sung as two words is not the word `"même"`
//! by any reading.
//!
//! Only what the source states is joined, and only where a marker is legal:
//! every note of a run must begin exactly where its predecessor ends, because
//! neither target can carry a marker across a gap.

use crate::engine::midi::{LyricState, Syllabic};
use crate::engine::projection::{ProjectedLyric, ProjectedNote};

/// Every dash a score uses where a syllable hyphen is meant, including the ones
/// a word processor substitutes for the ASCII one. Kept as a set rather than a
/// `char::is_dash_punctuation` test so a source cannot widen it silently.
const SYLLABLE_HYPHENS: [char; 11] = [
    '\u{002D}', // HYPHEN-MINUS
    '\u{2010}', // HYPHEN
    '\u{2011}', // NON-BREAKING HYPHEN
    '\u{2012}', // FIGURE DASH
    '\u{2013}', // EN DASH
    '\u{2014}', // EM DASH
    '\u{2015}', // HORIZONTAL BAR
    '\u{2212}', // MINUS SIGN
    '\u{FE58}', // SMALL EM DASH
    '\u{FE63}', // SMALL HYPHEN-MINUS
    '\u{FF0D}', // FULLWIDTH HYPHEN-MINUS
];

/// Reported once per lane whose syllables were reassembled into words.
pub const SYLLABLES_JOINED_INTO_WORDS: &str = "SYLLABLES_JOINED_INTO_WORDS";

/// Reported once per lane holding a word whose syllables silence separates, so
/// the binding the source states cannot be written and each syllable stays a
/// word of its own.
pub const WORD_NOT_JOINED_ACROSS_A_GAP: &str = "WORD_NOT_JOINED_ACROSS_A_GAP";

/// Splits a syllable's hyphen markers off its text: `("-nu-")` is a syllable
/// that continues the previous note and runs on into the next.
///
/// A token that is nothing but hyphens keeps them: it spells no syllable, and
/// what a bare dash means is not something the text states.
pub fn hyphen_markers(text: &str) -> (bool, bool, &str) {
    let trimmed = text.trim();
    let core = trimmed.trim_matches(|c| SYLLABLE_HYPHENS.contains(&c));
    if core.is_empty() {
        return (false, false, trimmed);
    }
    let joins_previous = trimmed.starts_with(SYLLABLE_HYPHENS);
    let joins_next = trimmed.ends_with(SYLLABLE_HYPHENS);
    (joins_previous, joins_next, core)
}

/// What one projected note contributes to a word.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    /// A syllable, with what the source says about the notes around it.
    Syllable {
        joins_previous: bool,
        joins_next: bool,
        /// Whether `<syllabic>` states this note's place in its word. A hyphen
        /// inside free text says a syllable runs on; it does not say where the
        /// word ends, so it cannot bracket a wordless note.
        stated: bool,
        core: String,
    },
    /// A note the source already states as held: a melisma inside the word. It
    /// belongs to the group and closes nothing.
    Held,
    /// A note carrying no word at all. Between two syllables of one word it is
    /// not silence — the score brackets it inside the word and draws the
    /// continuation dash over it — so the word sustains across it. Anywhere
    /// else it stays what it was.
    Untexted,
    /// Anything a word cannot cross: an explicit empty, a vocalization.
    Break,
}

fn classify(note: &ProjectedNote) -> Part {
    match &note.lyric {
        ProjectedLyric::Extension => Part::Held,
        ProjectedLyric::Absent => Part::Untexted,
        ProjectedLyric::Source(source) => match &source.state {
            LyricState::Text(text) => {
                let (hyphen_previous, hyphen_next, core) = hyphen_markers(text);
                // A token spelling no letter is not a syllable: a bare dash, a
                // lone comma a score parks on its own note. Absorbing one into
                // a word would hand the phonemizer a word that is not written.
                if !core.chars().any(char::is_alphanumeric) {
                    return Part::Break;
                }
                // `<syllabic>` is structure the source states about this note;
                // a hyphen is a mark inside free text. Where both are present
                // the structure decides, and the hyphen is still stripped so a
                // score that states both does not sing the dash.
                let (joins_previous, joins_next) = match source.syllabic {
                    Some(Syllabic::Begin) => (false, true),
                    Some(Syllabic::Middle) => (true, true),
                    Some(Syllabic::End) => (true, false),
                    Some(Syllabic::Single) => (false, false),
                    None => (hyphen_previous, hyphen_next),
                };
                Part::Syllable {
                    joins_previous,
                    joins_next,
                    stated: source.syllabic.is_some(),
                    core: core.to_string(),
                }
            }
            LyricState::Continuation | LyricState::SyllableSplit => Part::Held,
            LyricState::ExplicitEmpty | LyricState::Unsupported(_) => Part::Break,
        },
    }
}

fn touches(previous: &ProjectedNote, next: &ProjectedNote) -> bool {
    u64::from(previous.onset_ticks) + u64::from(previous.duration_ticks)
        == u64::from(next.onset_ticks)
}

/// The next syllable on a lane, and what lies between it and the one before.
struct Reach {
    syllable: usize,
    /// Untexted notes on the way, which the word sustains if it reaches past
    /// them and which are nothing of the sort if it does not.
    untexted: Vec<usize>,
    /// Whether every note from the previous syllable to this one touches. A
    /// marker cannot be stated across silence in either target.
    contiguous: bool,
}

/// One word: the syllables it occupies starting at `head`, the untexted notes
/// it sustains between them, and the syllable a stated binding could not reach
/// because a rest separates it.
///
/// One side is enough to bind two syllables: `<syllabic>` states both, while
/// the common hyphen convention marks only the syllable that runs on (`mi-`,
/// `nu-`, `te`) or only the one that continues (`mi`, `-nu`, `-te`).
fn word_run(
    notes: &[ProjectedNote],
    parts: &[Part],
    head: usize,
) -> (Vec<usize>, Vec<usize>, Option<usize>) {
    let mut members = vec![head];
    let mut sustained = Vec::new();
    let mut last = head;
    loop {
        let Some(reach) = next_syllable(notes, parts, last) else {
            return (members, sustained, None);
        };
        let (
            Part::Syllable {
                joins_next,
                stated: opens_stated,
                ..
            },
            Part::Syllable {
                joins_previous,
                stated: closes_stated,
                ..
            },
        ) = (&parts[last], &parts[reach.syllable])
        else {
            return (members, sustained, None);
        };
        if !joins_next && !joins_previous {
            return (members, sustained, None);
        }
        // A wordless note is inside the word only where `<syllabic>` brackets
        // it. A hyphen in free text — all a MIDI exporter can write — states
        // that a syllable runs on, never where the word ends, and reading a
        // hold out of it would invent the melisma the source never claimed.
        let brackets_the_word = *opens_stated && *closes_stated;
        if !reach.untexted.is_empty() && !brackets_the_word {
            return (members, sustained, None);
        }
        // Neither target can carry a syllable-split marker across silence:
        // OpenUtau wires an extension only when the previous note ends exactly
        // where this one begins, and would otherwise sing the marker as a word.
        if !reach.contiguous {
            return (members, sustained, Some(reach.syllable));
        }
        sustained.extend(reach.untexted);
        members.push(reach.syllable);
        last = reach.syllable;
    }
}

/// The next syllable on the lane. A note the source already holds is
/// transparent, and an untexted one is reported so the caller can decide
/// whether the word reaches past it.
fn next_syllable(notes: &[ProjectedNote], parts: &[Part], from: usize) -> Option<Reach> {
    let mut previous = from;
    let mut untexted = Vec::new();
    let mut contiguous = true;
    for index in (from + 1)..notes.len() {
        contiguous &= touches(&notes[previous], &notes[index]);
        match parts[index] {
            Part::Break => return None,
            Part::Held => previous = index,
            Part::Untexted => {
                untexted.push(index);
                previous = index;
            }
            Part::Syllable { .. } => {
                return Some(Reach {
                    syllable: index,
                    untexted,
                    contiguous,
                })
            }
        }
    }
    None
}

/// What one lane's syllables turned into.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoinedWords {
    /// The words written whole on the first note of their run.
    pub joined: Vec<String>,
    /// Words the source binds whose syllables sit either side of a silence, so
    /// no target can state the binding and each syllable stays a word of its
    /// own. Each entry is the word the source spells.
    pub separated: Vec<String>,
    /// Notes the score writes with no word of their own between two syllables
    /// of one word, now held rather than left out of the lane.
    pub sustained: usize,
}

/// Rewrites every run of syllables the source binds into one word: the word on
/// its first note, a syllable split on the rest.
///
/// Nothing else about a lyric changes — the source object, its id and its raw
/// bytes are carried on untouched, so the bundle still inventories every
/// syllable the score wrote.
pub fn join_words(notes: &mut [ProjectedNote]) -> JoinedWords {
    let parts: Vec<Part> = notes.iter().map(classify).collect();
    let core = |index: &usize| match &parts[*index] {
        Part::Syllable { core, .. } => core.as_str(),
        _ => "",
    };
    let mut result = JoinedWords::default();
    let mut index = 0;
    while index < notes.len() {
        if !matches!(parts[index], Part::Syllable { .. }) {
            index += 1;
            continue;
        }
        let (members, sustained, across_gap) = word_run(notes, &parts, index);
        if let Some(partner) = across_gap {
            let mut word: String = members.iter().map(core).collect();
            word.push_str(core(&partner));
            result.separated.push(word);
        }
        if members.len() < 2 {
            index += 1;
            continue;
        }
        // The score brackets these notes inside the word and draws its
        // continuation dash over them, so the syllable in front of each is
        // sustained across it rather than the note being wordless.
        for held in &sustained {
            notes[*held].lyric = ProjectedLyric::Extension;
        }
        result.sustained += sustained.len();
        let word: String = members.iter().map(core).collect();
        for (position, member) in members.iter().enumerate() {
            let ProjectedLyric::Source(source) = &mut notes[*member].lyric else {
                continue;
            };
            if position == 0 {
                source.state = LyricState::Text(word.clone());
                // The note now carries a whole word, so the pass run again over
                // the same lane finds nothing left to bind.
                source.syllabic = Some(Syllabic::Single);
            } else {
                source.state = LyricState::SyllableSplit;
            }
        }
        index = members.last().expect("non-empty") + 1;
        result.joined.push(word);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::midi::Lyric;

    fn note(onset: u32, duration: u32, lyric: ProjectedLyric) -> ProjectedNote {
        ProjectedNote {
            onset_ticks: onset,
            duration_ticks: duration,
            pitch: 60,
            lyric,
        }
    }

    fn word(text: &str, syllabic: Option<Syllabic>) -> ProjectedLyric {
        let mut lyric = Lyric::text("l", text.to_string());
        lyric.syllabic = syllabic;
        ProjectedLyric::Source(Box::new(lyric))
    }

    fn sung(notes: &[ProjectedNote]) -> Vec<String> {
        notes
            .iter()
            .map(|note| match &note.lyric {
                ProjectedLyric::Source(source) => match &source.state {
                    LyricState::Text(text) => text.clone(),
                    LyricState::SyllableSplit => "<split>".into(),
                    LyricState::Continuation => "<hold>".into(),
                    LyricState::ExplicitEmpty => "<empty>".into(),
                    LyricState::Unsupported(what) => format!("<{what}>"),
                },
                ProjectedLyric::Extension => "<ext>".into(),
                ProjectedLyric::Absent => "<none>".into(),
            })
            .collect()
    }

    fn chain(syllables: &[(&str, Option<Syllabic>)]) -> Vec<ProjectedNote> {
        syllables
            .iter()
            .enumerate()
            .map(|(index, (text, syllabic))| {
                note(index as u32 * 240, 240, word(text, syllabic.clone()))
            })
            .collect()
    }

    #[test]
    fn hyphen_markers_read_both_sides_and_keep_the_syllable() {
        assert_eq!(hyphen_markers("mi-"), (false, true, "mi"));
        assert_eq!(hyphen_markers("-nu-"), (true, true, "nu"));
        assert_eq!(hyphen_markers("-te"), (true, false, "te"));
        assert_eq!(hyphen_markers("te"), (false, false, "te"));
    }

    /// Every dash a word processor substitutes for the ASCII hyphen binds a
    /// syllable, or a score autocorrected once would stop converting.
    #[test]
    fn every_dash_variant_binds_a_syllable() {
        for dash in SYLLABLE_HYPHENS {
            let text = format!("mi{dash}");
            assert_eq!(hyphen_markers(&text), (false, true, "mi"), "{dash:?}");
        }
    }

    /// A dash alone spells no syllable, and what it means is not stated.
    #[test]
    fn a_bare_dash_keeps_its_text_and_binds_nothing() {
        assert_eq!(hyphen_markers("-"), (false, false, "-"));
        assert_eq!(hyphen_markers("--"), (false, false, "--"));
    }

    #[test]
    fn syllabic_state_rebuilds_the_word_on_the_first_note() {
        let mut notes = chain(&[
            ("mê", Some(Syllabic::Begin)),
            ("me", Some(Syllabic::End)),
            ("si", None),
        ]);
        assert_eq!(join_words(&mut notes).joined, vec!["même"]);
        assert_eq!(sung(&notes), vec!["même", "<split>", "si"]);
    }

    #[test]
    fn a_three_syllable_word_splits_onto_every_following_note() {
        let mut notes = chain(&[
            ("m'ar", Some(Syllabic::Begin)),
            ("rê", Some(Syllabic::Middle)),
            ("te", Some(Syllabic::End)),
        ]);
        assert_eq!(join_words(&mut notes).joined, vec!["m'arrête"]);
        assert_eq!(sung(&notes), vec!["m'arrête", "<split>", "<split>"]);
    }

    /// The convention a MIDI exporter writes: only the syllable that runs on is
    /// marked, and the last one carries no hyphen at all.
    #[test]
    fn trailing_hyphens_alone_bind_a_word() {
        let mut notes = chain(&[("mi-", None), ("nu-", None), ("te", None)]);
        assert_eq!(join_words(&mut notes).joined, vec!["minute"]);
        assert_eq!(sung(&notes), vec!["minute", "<split>", "<split>"]);
    }

    /// The mirror convention: only the continuing syllable is marked.
    #[test]
    fn leading_hyphens_alone_bind_a_word() {
        let mut notes = chain(&[("mi", None), ("-nu", None), ("-te", None)]);
        assert_eq!(join_words(&mut notes).joined, vec!["minute"]);
        assert_eq!(sung(&notes), vec!["minute", "<split>", "<split>"]);
    }

    #[test]
    fn whole_words_under_one_note_each_are_left_alone() {
        let mut notes = chain(&[("tout", None), ("au", None), ("bout", None)]);
        assert!(join_words(&mut notes).joined.is_empty());
        assert_eq!(sung(&notes), vec!["tout", "au", "bout"]);
    }

    /// Both notations in one phrase, which is what a real score writes.
    #[test]
    fn a_mixed_phrase_joins_only_what_the_source_binds() {
        let mut notes = chain(&[
            ("j'i", Some(Syllabic::Begin)),
            ("rai", Some(Syllabic::End)),
            ("au", None),
            ("bout", None),
            ("de", None),
            ("mes", None),
            ("rê", Some(Syllabic::Begin)),
            ("ves", Some(Syllabic::End)),
        ]);
        assert_eq!(join_words(&mut notes).joined, vec!["j'irai", "rêves"]);
        assert_eq!(
            sung(&notes),
            vec!["j'irai", "<split>", "au", "bout", "de", "mes", "rêves", "<split>"]
        );
    }

    /// A melisma inside a word: the held note stays a hold and the word still
    /// closes on the syllable that ends it.
    #[test]
    fn a_held_note_inside_a_word_stays_a_hold() {
        let mut notes = vec![
            note(0, 240, word("rê", Some(Syllabic::Begin))),
            note(240, 240, ProjectedLyric::Extension),
            note(480, 240, word("ves", Some(Syllabic::End))),
        ];
        assert_eq!(join_words(&mut notes).joined, vec!["rêves"]);
        assert_eq!(sung(&notes), vec!["rêves", "<ext>", "<split>"]);
    }

    /// A marker neither target can carry across a gap, so a word the source
    /// spreads over notes that do not touch is left exactly as it was written.
    #[test]
    fn syllables_separated_by_a_gap_are_not_joined() {
        let mut notes = vec![
            note(0, 240, word("mê", Some(Syllabic::Begin))),
            note(480, 240, word("me", Some(Syllabic::End))),
        ];
        assert!(join_words(&mut notes).joined.is_empty());
        assert_eq!(sung(&notes), vec!["mê", "me"]);
    }

    /// Silence inside a word is not something a target can spell, and it is not
    /// something to stay quiet about either: the score binds the two syllables
    /// and the file will not.
    #[test]
    fn a_word_a_rest_separates_is_reported_by_the_word_it_spells() {
        let mut notes = vec![
            note(0, 240, word("rê", Some(Syllabic::Begin))),
            note(480, 240, word("ves,", Some(Syllabic::End))),
        ];
        let words = join_words(&mut notes);
        assert!(words.joined.is_empty());
        assert_eq!(words.separated, vec!["rêves,"]);
    }

    /// Two words the score never binds are not a separated word.
    #[test]
    fn unbound_syllables_either_side_of_a_rest_are_not_reported() {
        let mut notes = vec![
            note(0, 240, word("tout", None)),
            note(480, 240, word("au", None)),
        ];
        assert_eq!(join_words(&mut notes), JoinedWords::default());
    }

    /// A note with no word of its own between two syllables of one word is the
    /// melisma the score draws its continuation dash over — MuseScore writes no
    /// extension length for it, the bracketing syllables state it. Left out of
    /// the lane it would shorten a word the score sustains.
    #[test]
    fn an_untexted_note_inside_a_word_is_sustained_rather_than_dropped() {
        let mut notes = vec![
            note(0, 240, word("rê", Some(Syllabic::Begin))),
            note(240, 240, ProjectedLyric::Absent),
            note(480, 240, ProjectedLyric::Absent),
            note(720, 240, word("ves,", Some(Syllabic::End))),
        ];
        let words = join_words(&mut notes);
        assert_eq!(words.joined, vec!["rêves,"]);
        assert_eq!(words.sustained, 2);
        assert_eq!(sung(&notes), vec!["rêves,", "<ext>", "<ext>", "<split>"]);
    }

    /// A hyphen states that a syllable runs on, never where its word ends, so
    /// it cannot bracket a wordless note. Reading a hold out of it would invent
    /// the melisma a Standard MIDI file never claimed.
    #[test]
    fn a_hyphen_alone_never_brackets_a_wordless_note() {
        let mut notes = vec![
            note(0, 240, word("mi-", None)),
            note(240, 240, ProjectedLyric::Absent),
            note(480, 240, word("te", None)),
        ];
        let words = join_words(&mut notes);
        assert!(words.joined.is_empty());
        assert_eq!(words.sustained, 0);
        assert_eq!(sung(&notes), vec!["mi-", "<none>", "te"]);
    }

    /// Only a word that closes reaches past an untexted note. A syllable whose
    /// binding leads nowhere leaves the notes after it exactly as they were, so
    /// an instrumental tail is never absorbed into a word.
    #[test]
    fn an_untexted_note_a_word_never_reaches_past_stays_untexted() {
        let mut notes = vec![
            note(0, 240, word("mi-", None)),
            note(240, 240, ProjectedLyric::Absent),
            note(480, 240, ProjectedLyric::Absent),
        ];
        let words = join_words(&mut notes);
        assert!(words.joined.is_empty());
        assert_eq!(words.sustained, 0);
        assert_eq!(sung(&notes), vec!["mi-", "<none>", "<none>"]);
    }

    /// Silence inside the melisma is still silence: a marker cannot be stated
    /// across it, so nothing is absorbed and the word is reported instead.
    #[test]
    fn an_untexted_note_that_does_not_touch_is_never_absorbed() {
        let mut notes = vec![
            note(0, 240, word("rê", Some(Syllabic::Begin))),
            note(720, 240, ProjectedLyric::Absent),
            note(960, 240, word("ves", Some(Syllabic::End))),
        ];
        let words = join_words(&mut notes);
        assert!(words.joined.is_empty());
        assert_eq!(words.sustained, 0);
        assert_eq!(words.separated, vec!["rêves"]);
        assert_eq!(sung(&notes), vec!["rê", "<none>", "ves"]);
    }

    /// `begin` with nothing to close it still binds the note in front of it —
    /// that is what the source states — but never reaches past a break.
    #[test]
    fn an_unclosed_begin_binds_only_the_note_it_touches() {
        let mut notes = vec![
            note(0, 240, word("bri", Some(Syllabic::Begin))),
            note(240, 240, word("ser", None)),
            note(480, 240, ProjectedLyric::Absent),
            note(720, 240, word("des", None)),
        ];
        assert_eq!(join_words(&mut notes).joined, vec!["briser"]);
        assert_eq!(sung(&notes), vec!["briser", "<split>", "<none>", "des"]);
    }

    /// `single` is the source saying this note carries a whole word, so a stray
    /// dash inside the text does not steal the note after it.
    #[test]
    fn a_single_syllable_never_reaches_the_next_note() {
        let mut notes = chain(&[("rock-", Some(Syllabic::Single)), ("roll", None)]);
        assert!(join_words(&mut notes).joined.is_empty());
        assert_eq!(sung(&notes), vec!["rock-", "roll"]);
    }

    /// The joined word is the syllables as written, so accents and elisions
    /// survive exactly; only the hyphens the source used as markers are gone.
    #[test]
    fn joining_preserves_the_source_characters() {
        let mut notes = chain(&[("s'a-", None), ("chè-", None), ("ve", None)]);
        assert_eq!(join_words(&mut notes).joined, vec!["s'achève"]);
    }

    /// A word already joined must not be joined again, which is what running
    /// the pass over a lane twice would do if a split were read as a syllable.
    #[test]
    fn the_pass_is_idempotent() {
        let mut notes = chain(&[
            ("mi", Some(Syllabic::Begin)),
            ("nu", Some(Syllabic::Middle)),
            ("te", Some(Syllabic::End)),
            ("si", None),
        ]);
        assert_eq!(join_words(&mut notes).joined, vec!["minute"]);
        assert!(join_words(&mut notes).joined.is_empty());
        assert_eq!(sung(&notes), vec!["minute", "<split>", "<split>", "si"]);
    }

    /// An explicit empty is the source stating that nothing is sung here, which
    /// no word may absorb.
    #[test]
    fn an_explicit_empty_breaks_a_word() {
        let mut notes = vec![
            note(0, 240, word("mê", Some(Syllabic::Begin))),
            note(240, 240, word("", None)),
            note(480, 240, word("me", Some(Syllabic::End))),
        ];
        assert!(join_words(&mut notes).joined.is_empty());
    }
}
