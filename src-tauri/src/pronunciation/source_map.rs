//! Durable source ownership associated with an immutable original USTX export.
use super::{assets::read_bounded, hash, Error, Language, POLICY};
use crate::engine::{
    projection::{ProjectedNote, ProjectedProject},
    target::{lexical, ustx},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub track: String,
    pub part: Option<String>,
    pub staff: Option<String>,
    pub voice: Option<String>,
    pub occurrence: u32,
    pub segment: Option<u32>,
    pub lane: String,
    pub verse: u32,
}

pub fn note_owner(note: &ProjectedNote) -> Option<Owner> {
    let origin = note.source_evidence.as_ref()?.origin.as_ref()?;
    let lyric = lexical::source(&note.lyric)?;
    Some(Owner {
        track: origin.track_id.clone(),
        part: origin.source.part_id.clone(),
        staff: origin.source.staff_id.clone(),
        voice: origin.source.voice.clone(),
        occurrence: origin.source.occurrence,
        segment: origin
            .source
            .continuity
            .as_ref()
            .map(|c| c.playback_segment),
        lane: lyric.lane.clone(),
        verse: lyric.verse,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Word {
    pub id: String,
    pub key: String,
    pub original: Vec<String>,
    pub members: Vec<String>,
    pub owner: Owner,
    pub context: Vec<String>,
    pub context_target: usize,
    pub attacks: usize,
    pub manual: bool,
}

impl Word {
    /// Note IDs alone do not distinguish alternative lyric rows projected on
    /// the same original note. The complete source owner participates too.
    pub fn matches_head(&self, note: &ProjectedNote) -> bool {
        let Some(evidence) = &note.source_evidence else {
            return false;
        };
        let Some(origin) = &evidence.origin else {
            return false;
        };
        let Some(lyric) = lexical::source(&note.lyric) else {
            return false;
        };
        evidence.note_id == self.id
            && origin.track_id == self.owner.track
            && origin.source.part_id == self.owner.part
            && origin.source.staff_id == self.owner.staff
            && origin.source.voice == self.owner.voice
            && origin.source.occurrence == self.owner.occurrence
            && origin
                .source
                .continuity
                .as_ref()
                .map(|c| c.playback_segment)
                == self.owner.segment
            && lyric.lane == self.owner.lane
            && lyric.verse == self.owner.verse
    }
}

pub fn word(
    notes: &[ProjectedNote],
    members: &[usize],
    key: String,
    context: Vec<String>,
    context_target: usize,
) -> Option<Word> {
    let head = notes.get(*members.first()?)?;
    let evidence = head.source_evidence.as_ref()?;
    let source = evidence.origin.as_ref()?;
    let lyric = lexical::source(&head.lyric)?;
    let mut ids = Vec::new();
    let mut original = Vec::new();
    let mut attacks = 0;
    let mut manual = false;
    for &index in members {
        let note = notes.get(index)?;
        ids.push(note.source_evidence.as_ref()?.note_id.clone());
        if let Some(lyric) = lexical::source(&note.lyric) {
            original.push(lyric.raw.clone());
            manual |= lyric.raw.contains(['[', ']']) || lyric.raw.trim_start().starts_with('?');
        }
        let hold = matches!(
            &note.lyric,
            crate::engine::projection::ProjectedLyric::Extension
        ) || matches!(&note.lyric,crate::engine::projection::ProjectedLyric::Source(s) if matches!(s.state,crate::engine::midi::LyricState::Continuation));
        if !hold {
            attacks += 1;
        }
    }
    Some(Word {
        id: evidence.note_id.clone(),
        key,
        original,
        members: ids,
        context,
        context_target,
        attacks,
        manual,
        owner: Owner {
            track: source.track_id.clone(),
            part: source.source.part_id.clone(),
            staff: source.source.staff_id.clone(),
            voice: source.source.voice.clone(),
            occurrence: source.source.occurrence,
            segment: source
                .source
                .continuity
                .as_ref()
                .map(|c| c.playback_segment),
            lane: lyric.lane.clone(),
            verse: lyric.verse,
        },
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reading {
    #[serde(default)]
    pub authority: Option<String>,
    #[serde(default)]
    pub aliases: Option<Vec<AliasOverride>>,
    pub language: Option<Language>,
    pub phonemizer: String,
    pub lexical_reading: Option<String>,
    pub phones: Option<Vec<String>>,
    pub alphabet: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AliasOverride {
    pub member: usize,
    pub index: u32,
    pub phone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeNote {
    #[serde(default)]
    pub alias_overrides: Vec<AliasOverride>,
    pub position: i64,
    pub duration: i64,
    pub tone: i64,
    pub lyric: String,
    pub phonemizer: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeTrack {
    pub notes: Vec<NativeNote>,
    pub singer: Option<String>,
    pub phonemizer: String,
}

pub fn parse_ustx(bytes: &[u8]) -> Result<Vec<NativeTrack>, Error> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(Error::new("PRONUNCIATION_FILE_LIMIT", "USTX is too large"));
    }
    let document: serde_yaml::Value = serde_yaml::from_slice(bytes)
        .map_err(|_| Error::new("PRONUNCIATION_USTX_INVALID", "Cannot parse corrected USTX"))?;
    let tracks = document["tracks"]
        .as_sequence()
        .ok_or_else(|| Error::new("PRONUNCIATION_USTX_INVALID", "Missing tracks"))?;
    if tracks.len() > 4096 {
        return Err(Error::new(
            "PRONUNCIATION_FILE_LIMIT",
            "Too many USTX tracks",
        ));
    }
    let mut result: Vec<_> = tracks
        .iter()
        .map(|t| NativeTrack {
            notes: Vec::new(),
            singer: t["singer"].as_str().map(str::to_owned),
            phonemizer: t["phonemizer"]
                .as_str()
                .unwrap_or(ustx::DEFAULT_PHONEMIZER)
                .into(),
        })
        .collect();
    let parts = document["voice_parts"]
        .as_sequence()
        .ok_or_else(|| Error::new("PRONUNCIATION_USTX_INVALID", "Missing voice parts"))?;
    let mut count = 0;
    for part in parts {
        let track = part["track_no"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|&i| i < result.len())
            .ok_or_else(|| Error::new("PRONUNCIATION_USTX_INVALID", "Invalid track index"))?;
        let offset = part["position"]
            .as_i64()
            .ok_or_else(|| Error::new("PRONUNCIATION_USTX_INVALID", "Invalid part position"))?;
        let notes = part["notes"]
            .as_sequence()
            .ok_or_else(|| Error::new("PRONUNCIATION_USTX_INVALID", "Missing notes"))?;
        count += notes.len();
        if count > 100_000 {
            return Err(Error::new(
                "PRONUNCIATION_FILE_LIMIT",
                "Too many USTX notes",
            ));
        }
        for note in notes {
            let integer = |key: &str| {
                note[key].as_i64().ok_or_else(|| {
                    Error::new("PRONUNCIATION_USTX_INVALID", "Invalid note geometry")
                })
            };
            let mut alias_overrides = Vec::new();
            // Pinned UPhoneme.cs:296-303 exposes index/phoneme separately from
            // offset and envelope timing; only explicit nonblank aliases are
            // pronunciation evidence. Timing-only overrides stay excluded.
            if let Some(overrides) = note["phoneme_overrides"].as_sequence() {
                if overrides.len() > 128 {
                    return Err(Error::new(
                        "PRONUNCIATION_FILE_LIMIT",
                        "Too many native alias overrides",
                    ));
                }
                let mut seen = std::collections::BTreeSet::new();
                for item in overrides {
                    let Some(phone) = item["phoneme"].as_str().filter(|s| !s.trim().is_empty())
                    else {
                        continue;
                    };
                    let index = item["index"].as_u64().filter(|&i| i < 128).ok_or_else(|| {
                        Error::new("PRONUNCIATION_READING_INVALID", "Invalid alias index")
                    })? as u32;
                    if !seen.insert(index)
                        || phone.len() > 256
                        || phone.contains(['\0', '\n', '\r'])
                    {
                        return Err(Error::new(
                            "PRONUNCIATION_READING_INVALID",
                            "Ambiguous or malformed explicit native alias",
                        ));
                    }
                    alias_overrides.push(AliasOverride {
                        member: 0,
                        index,
                        phone: phone.into(),
                    });
                }
                alias_overrides.sort_by_key(|a| a.index);
            }
            result[track].notes.push(NativeNote {
                alias_overrides,
                position: offset
                    .checked_add(integer("position")?)
                    .ok_or_else(|| Error::new("PRONUNCIATION_USTX_INVALID", "Position overflow"))?,
                duration: integer("duration")?,
                tone: integer("tone")?,
                lyric: note["lyric"]
                    .as_str()
                    .ok_or_else(|| Error::new("PRONUNCIATION_USTX_INVALID", "Missing lyric"))?
                    .into(),
                phonemizer: note["phonemizer"].as_str().map(str::to_owned),
            });
        }
    }
    for track in &mut result {
        track.notes.sort_by_key(|n| n.position);
        if track
            .notes
            .windows(2)
            .any(|w| w[0].position == w[1].position)
        {
            return Err(Error::new(
                "PRONUNCIATION_MATCH_AMBIGUOUS",
                "Duplicate note positions",
            ));
        }
    }
    Ok(result)
}

pub fn reading(note: &NativeNote, fallback: &str) -> Result<Reading, Error> {
    let phonemizer = note.phonemizer.as_deref().unwrap_or(fallback).to_string();
    let language = [
        Language::French,
        Language::English,
        Language::Spanish,
        Language::Portuguese,
    ]
    .into_iter()
    .find(|l| {
        l.native_name() == phonemizer || (note.phonemizer.is_none() && l.phonemizer() == phonemizer)
    });
    let (lexical_reading, phones) = if let Some((text, hint)) = note.lyric.split_once('[') {
        let hint = hint
            .strip_suffix(']')
            .filter(|h| !h.contains(['[', ']']))
            .ok_or_else(|| {
                Error::new("PRONUNCIATION_READING_INVALID", "Malformed explicit hint")
            })?;
        (
            Some(text.into()),
            Some(hint.split_whitespace().map(str::to_owned).collect()),
        )
    } else {
        (Some(note.lyric.clone()), None)
    };
    let alphabet = phone_alphabet(phones.as_deref());
    Ok(Reading {
        authority: None,
        aliases: (!note.alias_overrides.is_empty()).then(|| note.alias_overrides.clone()),
        language,
        phonemizer,
        lexical_reading,
        phones,
        alphabet,
    })
}

/// Symbol prefixes are retained evidence, independently of the selected
/// phonemizer. Bare/mixed inventories remain unknown, never inferred from route.
pub fn phone_alphabet(phones: Option<&[String]>) -> Option<String> {
    let phones = phones?;
    if phones.is_empty() {
        return None;
    }
    [
        ("fr/", "millefeuille"),
        ("en/", "cmu39"),
        ("es/", "diffsinger-es"),
        ("pt/", "diffsinger-pt"),
    ]
    .into_iter()
    .find(|(prefix, _)| {
        phones
            .iter()
            .all(|p| p.starts_with(prefix) && p.len() > prefix.len())
    })
    .map(|(_, alphabet)| alphabet.into())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoundWord {
    pub word: Word,
    pub track: usize,
    pub notes: Vec<usize>,
    pub before: Reading,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reference {
    #[serde(default)]
    pub source_label: Option<String>,
    #[serde(default)]
    pub export_label: Option<String>,
    #[serde(default)]
    pub created_at_unix_seconds: Option<u64>,
    pub id: String,
    pub policy: String,
    pub snapshot_id: String,
    pub source_sha256: String,
    pub export_sha256: String,
    pub words: Vec<BoundWord>,
    pub tracks: Vec<NativeTrack>,
}

impl Reference {
    pub fn check_budget(&self, bytes: &[u8]) -> Result<(), Error> {
        // Reserve space for the local labels populated during publication.
        if bytes.len() > 32 * 1024 * 1024
            || serde_json::to_vec(self)?.len() > 32 * 1024 * 1024 - 8192
        {
            return Err(Error::new(
                "PRONUNCIATION_REFERENCE_LIMIT",
                "Original reference exceeds the retention byte budget",
            ));
        }
        Ok(())
    }
    pub fn build(
        source_sha256: String,
        snapshot_id: String,
        project: &ProjectedProject,
        words: &[Word],
        bytes: &[u8],
    ) -> Result<Self, Error> {
        let tracks = parse_ustx(bytes)?;
        let mut bound = Vec::new();
        // Geometry is checked against the emitted project, while membership is
        // taken exclusively from the production source-word builder.
        for word in words {
            let mut matches = Vec::new();
            for (ti, track) in project.tracks.iter().enumerate() {
                if !track.notes.iter().any(|n| word.matches_head(n)) {
                    continue;
                }
                let indices: Vec<_> = track
                    .notes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| {
                        n.source_evidence
                            .as_ref()
                            .is_some_and(|e| word.members.contains(&e.note_id))
                    })
                    .map(|(i, _)| i)
                    .collect();
                if indices.len() == word.members.len() {
                    matches.push((ti, indices));
                }
            }
            if matches.len() != 1 {
                continue;
            }
            let (track, projection_indices) = matches.pop().unwrap();
            let Some(emitted) = tracks.get(track) else {
                return Err(Error::new(
                    "PRONUNCIATION_REFERENCE_INVALID",
                    "Projection/export track association differs",
                ));
            };
            let mut indices = Vec::new();
            for projection_index in projection_indices {
                let note = &project.tracks[track].notes[projection_index];
                let exact_tick = |ticks: u32| -> Result<i64, Error> {
                    let scaled = u64::from(ticks) * 480;
                    let ppq = u64::from(project.ticks_per_beat);
                    if ppq == 0 || scaled % ppq != 0 {
                        return Err(Error::new(
                            "PRONUNCIATION_REFERENCE_INVALID",
                            "Member timing is not exactly representable",
                        ));
                    }
                    i64::try_from(scaled / ppq).map_err(|_| {
                        Error::new("PRONUNCIATION_REFERENCE_INVALID", "Member timing overflow")
                    })
                };
                let geometry = (
                    exact_tick(note.onset_ticks)?,
                    exact_tick(note.duration_ticks)?,
                    i64::from(note.pitch),
                );
                let candidates: Vec<_> = emitted
                    .notes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| (n.position, n.duration, n.tone) == geometry)
                    .map(|(i, _)| i)
                    .collect();
                if candidates.len() != 1 || indices.contains(&candidates[0]) {
                    return Err(Error::new(
                        "PRONUNCIATION_REFERENCE_INVALID",
                        "Emitted member geometry differs or is ambiguous",
                    ));
                }
                indices.push(candidates[0]);
            }
            // Source member order follows performed onset, independently of
            // projection Vec ordering or the serializer's sorting.
            indices.sort_by_key(|&i| emitted.notes[i].position);
            let mut before = reading(&tracks[track].notes[indices[0]], &tracks[track].phonemizer)?;
            before.aliases = word_aliases(&tracks[track], &indices);
            before.language = project.tracks[track]
                .notes
                .iter()
                .find(|note| word.matches_head(note))
                .ok_or_else(|| {
                    Error::new(
                        "PRONUNCIATION_REFERENCE_INVALID",
                        "Source head association missing",
                    )
                })?
                .pronunciation_language
                .map(Language::from_projected)
                .or(before.language);
            bound.push(BoundWord {
                word: word.clone(),
                track,
                notes: indices,
                before,
            });
        }
        let export_sha256 = hash(bytes);
        let id = hash(&serde_json::to_vec(&(
            POLICY,
            &source_sha256,
            &snapshot_id,
            &export_sha256,
        ))?);
        Ok(Self {
            source_label: None,
            export_label: None,
            created_at_unix_seconds: None,
            id,
            policy: POLICY.into(),
            snapshot_id,
            source_sha256,
            export_sha256,
            words: bound,
            tracks,
        })
    }
    pub fn read_corrected(path: &Path) -> Result<(String, Vec<NativeTrack>), Error> {
        let bytes = read_bounded(path, 32 * 1024 * 1024)?;
        Ok((hash(&bytes), parse_ustx(&bytes)?))
    }
}

pub fn word_aliases(track: &NativeTrack, members: &[usize]) -> Option<Vec<AliasOverride>> {
    let aliases: Vec<_> = members
        .iter()
        .enumerate()
        .flat_map(|(member, &index)| {
            track.notes[index]
                .alias_overrides
                .iter()
                .cloned()
                .map(move |mut a| {
                    a.member = member;
                    a
                })
        })
        .collect();
    (!aliases.is_empty()).then_some(aliases)
}
