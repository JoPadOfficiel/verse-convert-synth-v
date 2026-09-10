use verse_lib::engine::{
    convert::{convert_midi_with_profile, convert_midi_with_target},
    musescore,
    projection::ProjectedLyric,
    target::{self, ExportTarget, PronunciationProfile},
};

fn merged_endpoint_score(middle: &str, endpoint: &str) -> String {
    format!(
        r#"<museScore version="2.06"><programVersion>2.3.2</programVersion><Score><Division>480</Division>
      <Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part>
      <Staff id="1"><Measure len="4/4">
      <Chord><durationType>quarter</durationType><Lyrics><text>vent</text><ticks>960</ticks></Lyrics><Note><pitch>76</pitch></Note></Chord>
      {middle}
      <Chord><durationType>eighth</durationType><Note><pitch>76</pitch><Tie id="held"/></Note></Chord>
      {endpoint}
      <Chord><durationType>quarter</durationType><Lyrics><text>fin</text></Lyrics><Note><pitch>74</pitch></Note></Chord>
      </Measure></Staff></Score></museScore>"#
    )
}

#[test]
fn melisma_endpoint_merged_into_later_head_keeps_every_proven_note() {
    let xml = merged_endpoint_score(
        "<Chord><durationType>eighth</durationType><Note><pitch>80</pitch></Note></Chord>",
        "<Chord><durationType>quarter</durationType><Note><pitch>76</pitch><endSpanner id=\"held\"/></Note></Chord>",
    );
    let midi = musescore::parse(xml.as_bytes()).unwrap();
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        for profile in [
            PronunciationProfile::Default,
            PronunciationProfile::FrenchMillefeuille,
            PronunciationProfile::EnglishArpabet,
        ] {
            let out = convert_midi_with_profile(&midi, "french", None, target, profile);
            assert!(out.ok, "{target:?}/{profile:?}: {:?}", out.msg);
            let project = out.svp.unwrap();
            let notes = &project.tracks[0].notes;
            assert_eq!(
                notes
                    .iter()
                    .map(|n| (n.onset_ticks, n.duration_ticks, n.pitch))
                    .collect::<Vec<_>>(),
                [
                    (0, 480, 76),
                    (480, 240, 80),
                    (720, 720, 76),
                    (1440, 480, 74)
                ]
            );
            for i in [1, 2] {
                assert!(matches!(notes[i].lyric, ProjectedLyric::Extension));
                let origin = notes[i]
                    .source_evidence
                    .as_ref()
                    .unwrap()
                    .origin
                    .as_ref()
                    .unwrap();
                assert_eq!(
                    origin.continuation.as_ref().unwrap().predecessor_id,
                    notes[i - 1].source_evidence.as_ref().unwrap().note_id
                );
            }
            target::serialize_to(target, &project).unwrap();
        }
    }
}

#[test]
fn merged_endpoint_does_not_bridge_a_rest_or_a_new_selected_word() {
    for middle in [
        "<Rest><durationType>eighth</durationType></Rest>",
        "<Chord><durationType>eighth</durationType><Lyrics><text>autre</text></Lyrics><Note><pitch>80</pitch></Note></Chord>",
    ] {
        let xml = merged_endpoint_score(middle,
            "<Chord><durationType>quarter</durationType><Note><pitch>76</pitch><endSpanner id=\"held\"/></Note></Chord>");
        let midi = musescore::parse(xml.as_bytes()).unwrap();
        for target in [ExportTarget::Svp, ExportTarget::Ustx] {
            let out = convert_midi_with_target(&midi, "french", None, target);
            assert!(out.ok, "{:?}", out.msg);
            let project = out.svp.unwrap();
            assert!(!project.tracks.iter().flat_map(|t| &t.notes).any(|n| n.onset_ticks == 720));
            target::serialize_to(target, &project).unwrap();
        }
    }
}

#[test]
fn joined_final_syllable_remains_owner_of_its_written_melisma() {
    let xml = r#"<museScore version="3.02"><Score><Division>480</Division>
      <Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part>
      <Staff id="1"><Measure><voice>
      <Chord><durationType>quarter</durationType><Lyrics><text>chan</text><syllabic>begin</syllabic></Lyrics><Note><pitch>65</pitch></Note></Chord>
      <Chord><durationType>quarter</durationType><Lyrics><text>ter</text><syllabic>end</syllabic><ticks>480</ticks></Lyrics><Note><pitch>65</pitch></Note></Chord>
      <Chord><durationType>quarter</durationType><Note><pitch>67</pitch></Note></Chord>
      <Chord><durationType>quarter</durationType><Lyrics><text>encore</text></Lyrics><Note><pitch>65</pitch></Note></Chord>
      </voice></Measure></Staff></Score></museScore>"#;
    let midi = musescore::parse(xml.as_bytes()).unwrap();
    for target in [ExportTarget::Svp, ExportTarget::Ustx] {
        let outcome = convert_midi_with_target(&midi, "french", None, target);
        assert!(outcome.ok, "{:?}", outcome.msg);
        let project = outcome.svp.unwrap();
        let notes = &project.tracks[0].notes;
        assert_eq!(notes.len(), 4);
        assert!(notes[1].lyric.continues_previous_note());
        assert!(matches!(notes[2].lyric, ProjectedLyric::Extension));
        let link = notes[2]
            .source_evidence
            .as_ref()
            .unwrap()
            .origin
            .as_ref()
            .unwrap()
            .continuation
            .as_ref()
            .unwrap();
        assert_eq!(
            link.lyric_owner_id,
            notes[1].lyric.source_identity().unwrap()
        );
        assert_eq!([notes[1].onset_ticks, notes[2].onset_ticks], [480, 960]);
        target::serialize_to(target, &project).unwrap();
        let mut changed = project.clone();
        changed.tracks[0].notes[2]
            .source_evidence
            .as_mut()
            .unwrap()
            .origin
            .as_mut()
            .unwrap()
            .continuation
            .as_mut()
            .unwrap()
            .lyric_owner_id = "foreign".into();
        assert!(target::serialize_to(target, &changed).is_err());
    }
}
