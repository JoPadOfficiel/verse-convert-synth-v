//! Private, reproducible corpus gates. The copyrighted files are never
//! committed; requesting this ignored test makes every expected fixture
//! mandatory.

use std::path::{Path, PathBuf};
use verse_lib::engine::{convert::convert_midi, midi, musescore, musicxml, target};
use verse_lib::stems::StemPlan;

type ScoreParser = fn(&[u8]) -> Result<midi::Midi, String>;

fn required_fixture(root: &Path, name: &str) -> PathBuf {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("fixture name has a UTF-8 stem");
    let mut candidates = vec![
        root.join(name),
        root.join("Musique_maman_a_convertire").join(name),
        root.join(format!("{stem}.versebundle"))
            .join("source")
            .join(name),
    ];
    if let Some(parent) = root.parent() {
        candidates.push(parent.join(name));
        candidates.push(parent.join("Musique_maman_a_convertire").join(name));
        candidates.push(
            parent
                .join(format!("{stem}.versebundle"))
                .join("source")
                .join(name),
        );
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| {
            panic!(
                "required topology fixture {name:?} was not found under {}",
                root.display()
            )
        })
}

#[test]
#[ignore = "requires VERSE_CORPUS_DIR with the seven audited private KAR files"]
fn audited_kar_corpus_keeps_only_proven_lyric_bindings() {
    let root = PathBuf::from(
        std::env::var("VERSE_CORPUS_DIR")
            .expect("VERSE_CORPUS_DIR must point to the audited private corpus"),
    );
    let fixtures = [
        ("Beatles - All you need is love.kar", 207usize),
        ("Beatles - HELP.kar", 314),
        ("Dirty_Dancing_._She_s_like_the_wind.kar", 218),
        ("Elvis_Presley_._Heartbreak_hotel.kar", 276),
        ("Elvis_Presley_._Hound_dog.kar", 244),
        ("Liza_Minelli_._Cabaret.kar", 162),
        ("Queen_._Crazy_little_thing_called_love.kar", 0),
    ];

    for (name, expected_placed) in fixtures {
        let path = required_fixture(&root, name);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("required fixture {}: {error}", path.display()));
        let midi = midi::parse_with_karaoke_profile(&bytes)
            .unwrap_or_else(|error| panic!("{name} must parse: {error}"));
        let outcome = convert_midi(&midi, "english");
        assert!(outcome.ok, "{name}: {:?}", outcome.msg);
        assert_eq!(
            outcome.placed, expected_placed,
            "{name}: unexpected projection; report: {:?}",
            outcome.tracks
        );
        assert_eq!(
            outcome
                .projection
                .source_ids
                .iter()
                .filter(|source_id| source_id.starts_with("lyric:"))
                .count(),
            expected_placed,
            "{name}: every emitted lyric must have one source evidence ID"
        );

        let project =
            target::svp::serialize(&outcome.svp.expect("successful conversion has a project"))
                .expect("exactly representable");
        for lyric in project
            .tracks
            .iter()
            .flat_map(|track| &track.main_group.notes)
            .map(|note| note.lyrics.as_str())
            .filter(|lyric| !lyric.is_empty())
        {
            let display_body = lyric
                .strip_prefix('\\')
                .or_else(|| lyric.strip_prefix('/'))
                .unwrap_or(lyric)
                .trim();
            assert!(
                !lyric.starts_with('@')
                    && !matches!(lyric, "\r" | "\n" | "\r\n")
                    && !display_body.chars().all(|character| character == '.'),
                "{name}: a control record was emitted as lyric: {lyric:?}"
            );
        }

        if name.contains("Cabaret") {
            assert!(outcome.tracks.iter().any(|track| {
                track.warnings.iter().any(|warning| {
                    warning.code == "KARAOKE_CHORD_PITCH_AMBIGUOUS"
                        && warning.message.starts_with('8')
                })
            }));
        }
        if name.starts_with("Queen") {
            assert!(project.tracks.is_empty());
        }
    }
}

#[test]
#[ignore = "requires VERSE_CORPUS_DIR with the four audited private score files"]
fn audited_score_corpus_has_stable_part_staff_voice_topology() {
    let root = PathBuf::from(
        std::env::var("VERSE_CORPUS_DIR")
            .expect("VERSE_CORPUS_DIR must point to the audited private corpus"),
    );
    let fixtures: [(&str, ScoreParser, usize, usize); 4] = [
        ("This Little S_Pno Melodie.mscz", musescore::parse, 2, 3),
        ("Help SAB PB MZ4.mxl", musicxml::parse, 6, 10),
        ("Help SAB PB MZ4.mscz", musescore::parse, 6, 10),
        ("Iko Iko-Georg for Brass.mscz", musescore::parse, 8, 9),
    ];
    let mut help_signature = None;

    for (name, parse, expected_parts, expected_voices) in fixtures {
        let path = required_fixture(&root, name);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("required fixture {}: {error}", path.display()));
        let midi =
            parse(&bytes).unwrap_or_else(|error| panic!("{name} must parse safely: {error}"));
        assert_eq!(midi.topology.part_count(), expected_parts, "{name}");
        assert_eq!(midi.topology.voice_count(), expected_voices, "{name}");
        assert!(
            midi.topology.projection_lane_count() >= midi.topology.voice_count(),
            "{name}: every source voice needs at least one projection lane"
        );

        let outcome = convert_midi(&midi, "english");
        assert!(outcome.ok, "{name}: {:?}", outcome.msg);
        assert_eq!(outcome.topology, midi.topology, "{name}");
        assert_eq!(
            outcome.n_tracks, expected_voices,
            "{name}: technical chord lanes must not be counted as source tracks"
        );
        let stem_plan = StemPlan::from_source(&midi, &outcome.tracks)
            .unwrap_or_else(|error| panic!("{name}: source Part stem plan failed: {error}"));
        assert_eq!(
            stem_plan.stems.len(),
            expected_parts,
            "{name}: every note-bearing source Part needs exactly one stem"
        );
        assert!(
            target::svp::serialize(
                outcome
                    .svp
                    .as_ref()
                    .expect("successful conversion has a project")
            )
            .expect("exactly representable")
            .tracks
            .iter()
            .all(|track| !track.main_group.notes.is_empty()),
            "{name}: no empty vocal track may be serialized"
        );

        if name.starts_with("Help SAB") {
            let signature: Vec<Vec<usize>> = midi
                .topology
                .parts
                .iter()
                .map(|part| part.staves.iter().map(|staff| staff.voices.len()).collect())
                .collect();
            if let Some(expected) = &help_signature {
                assert_eq!(
                    &signature, expected,
                    "Help MXL and MSCZ must expose the same Part/staff/voice shape"
                );
            } else {
                help_signature = Some(signature);
            }
        }
    }
}
