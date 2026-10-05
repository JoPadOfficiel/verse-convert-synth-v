//! Source-faithful development capture. Gold is external; no title inference.
use serde_json::json;
use std::{
    io::{self, BufRead},
    path::Path,
};
use verse_lib::{
    engine::{
        convert::convert_midi_with_snapshot,
        midi::{self, Midi},
        musescore, musicxml,
        projection::ProjectedLyric,
        target::{self, ExportTarget, PronunciationProfile},
    },
    pronunciation::{hash, source_map::note_owner, Language, Snapshot},
};

fn parse(path: &Path, bytes: &[u8]) -> Result<Midi, String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    match extension.as_str() {
        "mid" | "midi" => midi::parse(bytes).map_err(|e| e.to_string()),
        "kar" => midi::parse_with_karaoke_profile(bytes).map_err(|e| e.to_string()),
        "xml" | "musicxml" | "mxl" => musicxml::parse(bytes).map_err(|e| e.to_string()),
        "mscx" | "mscz" => musescore::parse(bytes).map_err(|e| e.to_string()),
        _ => Err("unsupported source extension".into()),
    }
}
fn label_for_word<'a>(
    notes: &'a [serde_json::Value],
    word: &verse_lib::pronunciation::source_map::Word,
    field: &str,
) -> Option<&'a str> {
    let owner = serde_json::to_value(&word.owner).ok()?;
    notes
        .iter()
        .find(|n| n["id"] == word.id && n["owner"] == owner)
        .and_then(|n| n[field].as_str())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        let request: serde_json::Value = serde_json::from_str(&line?)?;
        let path = Path::new(request["path"].as_str().ok_or("missing source path")?);
        let bytes = std::fs::read(path)?;
        let source_sha256 = hash(&bytes);
        let source = match parse(path, &bytes) {
            Ok(source) => source,
            Err(message) => {
                println!(
                    "{}",
                    json!({"source_sha256":source_sha256,"ok":false,"stage":"parse","diagnostic_code":"SOURCE_PARSE_FAILED","diagnostic":message})
                );
                continue;
            }
        };
        let snapshot = Snapshot::baseline(source_sha256.clone(), Vec::new())?;
        let baseline = convert_midi_with_snapshot(
            &source,
            "english",
            None,
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            None,
        );
        let rejected = convert_midi_with_snapshot(
            &source,
            "english",
            None,
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            Some(&snapshot),
        );
        if !baseline.ok {
            println!(
                "{}",
                json!({"source_sha256":source_sha256,"ok":false,"stage":"projection","diagnostic_code":"CONVERSION_FAILED","diagnostic":baseline.msg})
            );
            continue;
        }
        let project = baseline.svp.as_ref().ok_or("missing project")?;
        let emitted =
            target::serialize_to(ExportTarget::Ustx, project).map_err(|e| e.to_string())?;
        let fallback = target::serialize_to(
            ExportTarget::Ustx,
            rejected
                .svp
                .as_ref()
                .ok_or("missing rejected model control")?,
        )
        .map_err(|e| e.to_string())?;
        if emitted != fallback {
            return Err("rejected candidate differs from baseline".into());
        }
        let mut candidate_snapshot = Snapshot::baseline(source_sha256.clone(), Vec::new())?;
        let candidate_gate = if let Some(root) = request["candidate_root"].as_str() {
            let expected = request["candidate_manifest_sha256"]
                .as_str()
                .ok_or("Frozen candidate manifest hash required")?;
            let target = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
            match verse_lib::pronunciation::laya::Laya::load_for_evaluation(
                Path::new(root),
                &target,
                expected,
            ) {
                Ok(model) => {
                    candidate_snapshot.model = Some(std::sync::Arc::new(model));
                    "calibrated_development_candidate".to_string()
                }
                Err(e) => format!("unavailable:{}", e.code),
            }
        } else {
            "unavailable:LAYA_CALIBRATION_UNQUALIFIED".into()
        };
        let candidate = convert_midi_with_snapshot(
            &source,
            "english",
            None,
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            Some(&candidate_snapshot),
        );
        let candidate_project = candidate
            .svp
            .as_ref()
            .ok_or("candidate projection failed")?;
        let mut notes = Vec::new();
        for (track_index, track) in project.tracks.iter().enumerate() {
            for note in &track.notes {
                let lyric = match &note.lyric {
                    ProjectedLyric::Source(s)
                    | ProjectedLyric::Pronounced { source: s, .. }
                    | ProjectedLyric::PronouncedSplit { source: s } => Some(s),
                    _ => None,
                };
                let evidence = note
                    .source_evidence
                    .as_ref()
                    .ok_or("missing note evidence")?;
                let origin = evidence.origin.as_ref().ok_or("missing note origin")?;
                let matches: Vec<_> = candidate_project
                    .tracks
                    .iter()
                    .flat_map(|t| &t.notes)
                    .filter(|n| {
                        n.source_evidence.as_ref() == Some(evidence)
                            && note_owner(n) == note_owner(note)
                    })
                    .collect();
                if matches.len() != 1 {
                    return Err("candidate source association differs".into());
                }
                let h = matches[0];
                if (note.onset_ticks, note.duration_ticks, note.pitch)
                    != (h.onset_ticks, h.duration_ticks, h.pitch)
                {
                    return Err("candidate changed source geometry".into());
                }
                notes.push(json!({"owner":note_owner(note),"track":track_index,"id":evidence.note_id,"source_note_id":origin.source.id,"lyric_id":lyric.map(|l|&l.id),"raw":lyric.map(|l|&l.raw),"lane":lyric.map(|l|&l.lane),"verse":lyric.map(|l|l.verse),"staff":origin.source.staff_id,"part":origin.source.part_id,"voice":origin.source.voice,"measure":origin.source.measure,"occurrence":origin.source.occurrence,"onset":note.onset_ticks,"duration":note.duration_ticks,"pitch":note.pitch,"continues":note.lyric.continues_previous_note(),"hybrid_owner":h.pronunciation_language.map(Language::from_projected),"hybrid_hint":match &h.lyric{ProjectedLyric::Pronounced{phonemes,..}=>Some(phonemes),_=>None},"baseline_owner":note.pronunciation_language.map(Language::from_projected),"hint":match &note.lyric{ProjectedLyric::Pronounced{phonemes,..}=>Some(phonemes),_=>None}}));
            }
        }
        let words:Vec<_>=baseline.pronunciation_words.iter().map(|word|{
            let owner=label_for_word(&notes,word,"baseline_owner");
            let hybrid=label_for_word(&notes,word,"hybrid_owner");
            json!({"source_word":word,"baseline_owner":owner,"hybrid_owner":hybrid,"context":word.context.iter().enumerate().map(|(i,k)|if i==word.context_target{format!("<target>{k}</target>")}else{k.clone()}).collect::<Vec<_>>().join(" ")})
        }).collect();
        println!(
            "{}",
            json!({"source_sha256":source_sha256,"ok":true,"policy":verse_lib::pronunciation::POLICY,"baseline":verse_lib::pronunciation::BASELINE,"baseline_export_sha256":hash(&emitted),"hybrid_rejected_equivalent":true,"candidate_gate":candidate_gate,"candidate_diagnostics":candidate.tracks.iter().flat_map(|t|&t.warnings).collect::<Vec<_>>(),"candidate_evidence":candidate_snapshot.evidence.lock().unwrap().clone(),"laya_routed_ablation_gate":"unavailable:not_qualified","words":words,"notes":notes,"diagnostics":baseline.tracks.iter().flat_map(|t|&t.warnings).collect::<Vec<_>>() })
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_labels_distinguish_two_verses_with_identical_note_ids() {
        let mut word:verse_lib::pronunciation::source_map::Word=serde_json::from_value(json!({"id":"shared","key":"word","original":["word"],"members":["shared"],"owner":{"track":"track","part":"P1","staff":"1","voice":"1","occurrence":1,"segment":0,"lane":"lyrics","verse":1},"context":["word"],"context_target":0,"attacks":1,"manual":false})).unwrap();
        let first = serde_json::to_value(&word.owner).unwrap();
        word.owner.verse = 2;
        let second = serde_json::to_value(&word.owner).unwrap();
        let notes = vec![
            json!({"id":"shared","owner":first,"baseline_owner":"en"}),
            json!({"id":"shared","owner":second,"baseline_owner":"fr"}),
        ];
        assert_eq!(label_for_word(&notes, &word, "baseline_owner"), Some("fr"));
        word.owner.verse = 1;
        assert_eq!(label_for_word(&notes, &word, "baseline_owner"), Some("en"));
    }
}
