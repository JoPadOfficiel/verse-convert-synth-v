//! Private structural gate; no fixtures, renderer, voicebank or audio are shipped.
//!
//! Set both environment variables and explicitly run the ignored test. The report
//! directory must be new, with an existing parent, outside the source corpus and
//! outside Git (or under `_bmad-output/`, `src-tauri/target/`, or test fixtures).
//! Every supported path is audited, even when another path has identical bytes.
//! A refusal is recorded separately from an export; unknown errors fail the gate
//! after the complete receipt is written. The parser's shared IR is the oracle
//! for structural checks, not independent proof that the parser read notation
//! correctly. Independent native MIDI comparison belongs to a separate gate;
//! no rendering, listening, or acoustic timing is qualified by this gate.
//! Without a successful sibling-target projection, a both-target exactness
//! refusal remains unexpected: this fail-closed helper can reject a valid corpus.

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use verse_lib::engine::convert::{convert_midi_with_profile, ConvertOutcome};
use verse_lib::engine::midi::{self, Event, Kind, Lyric, LyricState, Midi, TimeBase};
use verse_lib::engine::projection::{
    ProjectedLyric, ProjectedNote, ProjectedProject, PronunciationLanguage,
};
use verse_lib::engine::target::{self, ExportTarget, PronunciationProfile, SerializeError};
use verse_lib::engine::{musescore, musicxml};
use verse_lib::score_stems::ScoreStems;

const BLICKS: u64 = 705_600_000;
const USTX_TICKS: u64 = 480;
const PROFILES: [PronunciationProfile; 2] = [
    PronunciationProfile::Default,
    PronunciationProfile::Automatic,
];
const TARGETS: [ExportTarget; 2] = [ExportTarget::Svp, ExportTarget::Ustx];

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn profile_name(profile: PronunciationProfile) -> &'static str {
    match profile {
        PronunciationProfile::Default => "default",
        PronunciationProfile::Automatic => "automatic",
        _ => unreachable!("the gate requests only Default and Automatic"),
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn inside_bundle(path: &Path) -> bool {
    path.ancestors()
        .any(|part| extension(part) == "versebundle")
}

/// Follow directory aliases without deduplicating paths. Only ancestor cycles
/// are rejected; filesystem and symlink errors remain visible in the receipt.
fn discover(
    path: &Path,
    ancestors: &mut BTreeSet<PathBuf>,
    files: &mut Vec<PathBuf>,
    excluded: &mut Vec<PathBuf>,
    errors: &mut Vec<String>,
) {
    if inside_bundle(path) {
        excluded.push(path.to_path_buf());
        return;
    }
    let canonical = match fs::canonicalize(path) {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            return;
        }
    };
    if inside_bundle(&canonical) {
        excluded.push(path.to_path_buf());
        return;
    }
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            if !ancestors.insert(canonical.clone()) {
                errors.push(format!("directory symlink cycle: {}", path.display()));
                return;
            }
            match fs::read_dir(path) {
                Ok(entries) => {
                    let mut children = Vec::new();
                    for entry in entries {
                        match entry {
                            Ok(entry) => children.push(entry.path()),
                            Err(error) => errors.push(format!("{}: {error}", path.display())),
                        }
                    }
                    children.sort();
                    for child in children {
                        discover(&child, ancestors, files, excluded, errors);
                    }
                }
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
            ancestors.remove(&canonical);
        }
        Ok(metadata) if metadata.is_file() => {
            if matches!(
                extension(path).as_str(),
                "mid" | "midi" | "kar" | "musicxml" | "xml" | "mxl" | "mscx" | "mscz"
            ) {
                files.push(path.to_path_buf());
            }
        }
        Ok(_) => errors.push(format!("unsupported filesystem entry: {}", path.display())),
        Err(error) => errors.push(format!("{}: {error}", path.display())),
    }
}

/// Match the shared command boundary's sniffing and explicit KAR qualification.
/// `convert_auto_with` alone would gate only SynthV and miss extension-owned KAR.
fn parse(bytes: &[u8], path: &Path) -> Result<Midi, String> {
    if musicxml::looks_like_xml(bytes) {
        if musescore::is_musescore_xml(bytes) {
            return musescore::parse(bytes);
        }
        return musicxml::parse(bytes);
    }
    if musicxml::is_zip(bytes) {
        if musicxml::zip_has_musicxml(bytes) {
            return musicxml::parse(bytes);
        }
        if musescore::zip_has_mscx(bytes) {
            return musescore::parse(bytes);
        }
        return Err("archive contains no recognized score".into());
    }
    if extension(path) == "kar" {
        midi::parse_with_karaoke_profile(bytes)
    } else {
        midi::parse(bytes)
    }
}

fn capture<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic");
        Err(format!("panic: {message}"))
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn source_lyric(lyric: &ProjectedLyric) -> Option<&Lyric> {
    match lyric {
        ProjectedLyric::Source(source)
        | ProjectedLyric::Pronounced { source, .. }
        | ProjectedLyric::PronouncedSplit { source } => Some(source),
        ProjectedLyric::Absent | ProjectedLyric::Extension => None,
    }
}

/// Tempo positions come directly from source events, not the target serializer.
/// Same-tick precedence follows source discovery; opening 120 is the documented
/// implied tempo only when the source has not stated one at zero.
fn source_tempos(midi: &Midi) -> BTreeMap<u32, f64> {
    let mut tempos = BTreeMap::new();
    for track in &midi.tracks {
        for event in &track.events {
            if let Kind::Tempo(micros) = event.kind {
                if micros > 0 {
                    tempos.insert(event.tick, 60_000_000.0 / f64::from(micros));
                }
            }
        }
    }
    tempos.entry(0).or_insert(120.0);
    tempos
}

fn bpm_matches(actual: f64, expected: f64) -> bool {
    actual.is_finite() && (actual - expected).abs() <= 0.000_000_51
}

fn assert_source(project: &ProjectedProject, midi: &Midi, outcome: &ConvertOutcome) {
    assert_eq!(project.ticks_per_beat, midi.ticks_per_beat);
    assert!(project.ticks_per_beat > 0);
    // Compare the complete final ownership ledger, not only surviving notes.
    // Source-only exclusions are outside this set and remain source evidence.
    let retained: BTreeSet<_> = project
        .tracks
        .iter()
        .flat_map(|lane| &lane.notes)
        .flat_map(|note| {
            note.source_evidence
                .as_ref()
                .expect("source evidence")
                .source_ids()
        })
        .filter(|id| id.starts_with("note:") || id.starts_with("lyric:"))
        .map(str::to_owned)
        .collect();
    let associated: BTreeSet<_> = outcome
        .projection
        .source_ids
        .iter()
        .filter(|id| id.starts_with("note:") || id.starts_with("lyric:"))
        .cloned()
        .collect();
    assert_eq!(
        retained, associated,
        "final source note/lyric association coverage changed"
    );
    if !outcome.projection.intensity_note_owners.is_empty() {
        let mut expected: Vec<_> = outcome
            .projection
            .intensity_note_owners
            .iter()
            .map(|owner| {
                (
                    owner.note_id.as_str(),
                    owner.source_track_id.as_str(),
                    owner.target_track,
                    owner.destination_track_id.as_str(),
                    owner.start_tick,
                    u64::from(owner.end_tick),
                )
            })
            .collect();
        let mut actual: Vec<_> = project
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(index, lane)| {
                lane.notes.iter().map(move |note| {
                    let evidence = note.source_evidence.as_ref().unwrap();
                    (
                        evidence.note_id.as_str(),
                        evidence.origin.as_ref().unwrap().track_id.as_str(),
                        index,
                        lane.source_track_id.as_str(),
                        note.onset_ticks,
                        u64::from(note.onset_ticks) + u64::from(note.duration_ticks),
                    )
                })
            })
            .collect();
        expected.sort();
        actual.sort();
        assert_eq!(
            actual, expected,
            "final note placement associations changed"
        );
    }
    let source_tracks: BTreeMap<_, _> = midi
        .tracks
        .iter()
        .map(|track| (track.id.as_str(), track))
        .collect();
    // A narrow independent eligibility witness: one attached lyric row, one
    // unconditional text record, a pitched non-grace note and a positive,
    // identity-owned note-off. Repeated verses, conflicts, pitchless/grace
    // notes and standalone MIDI bindings need the ledger rather than a guess.
    let mut eligible = BTreeSet::new();
    for track in &midi.tracks {
        let rows: BTreeSet<_> = track
            .events
            .iter()
            .filter_map(|event| {
                if let Kind::NoteOn(note) = &event.kind {
                    Some(&note.lyrics)
                } else {
                    None
                }
            })
            .flatten()
            .map(|lyric| &lyric.lane)
            .collect();
        if rows.len() != 1 {
            continue;
        }
        for event in &track.events {
            let Kind::NoteOn(note) = &event.kind else {
                continue;
            };
            if note.velocity == Some(0)
                || note.key.is_none()
                || note.source.grace
                || note.source.unpitched.is_some()
                || !track.note_instrument_role(note).allows_vocal()
                || note.lyrics.len() != 1
            {
                continue;
            }
            let lyric = &note.lyrics[0];
            let end = track.events.iter().find(|end| {
                end.tick >= event.tick
                    && matches!(&end.kind, Kind::NoteOff(off) if off.source_id.as_deref() == Some(note.source.id.as_str()))
            });
            if !lyric.time_only.is_empty()
                || !matches!(&lyric.state, LyricState::Text(text) if !text.trim().is_empty())
                || end.is_none_or(|end| end.tick <= event.tick)
            {
                continue;
            }
            eligible.insert(format!(
                "note:{}:{}:occurrence:{}:event:{}",
                track.id, note.source.id, note.source.occurrence, event.order
            ));
            eligible.insert(format!(
                "lyric:{}:occurrence:{}:note-event:{}",
                lyric.id, note.source.occurrence, event.order
            ));
        }
    }
    assert_eq!(
        eligible.intersection(&retained).count(),
        eligible.len(),
        "provably eligible source note/lyric lost"
    );
    let events: BTreeMap<String, &Event> = midi
        .tracks
        .iter()
        .flat_map(|track| {
            track
                .events
                .iter()
                .map(move |event| (format!("event:{}:{}", track.id, event.order), event))
        })
        .collect();
    let expected_tempos = source_tempos(midi);
    assert_eq!(project.tempos.len(), expected_tempos.len(), "tempo count");
    let mut seen_tempos = BTreeSet::new();
    for tempo in &project.tempos {
        assert!(seen_tempos.insert(tempo.tick), "duplicate projected tempo");
        assert!(
            bpm_matches(tempo.bpm, expected_tempos[&tempo.tick]),
            "source tempo changed at tick {}",
            tempo.tick
        );
    }
    for lane in &project.tracks {
        let destination = source_tracks[lane.source_track_id.as_str()];
        // A pronunciation convention may divide a source note. Its pieces must
        // tile the exact original interval, within this destination lyric lane.
        let mut intervals: BTreeMap<&str, Vec<(u32, u64)>> = BTreeMap::new();
        let mut bounds = BTreeMap::new();
        for note in &lane.notes {
            let evidence = note.source_evidence.as_ref().expect("source note evidence");
            let origin = evidence.origin.as_ref().expect("source note origin");
            if lane.source_track_id != origin.track_id {
                let link = origin
                    .continuation
                    .as_ref()
                    .expect("cross-track note needs explicit continuity proof");
                assert!(
                    note.lyric.continues_previous_note(),
                    "cross-track note is not a continuation"
                );
                assert_eq!(link.destination_track_id, lane.source_track_id);
                let predecessor = lane
                    .notes
                    .iter()
                    .find(|head| {
                        head.source_evidence
                            .as_ref()
                            .is_some_and(|e| e.note_id == link.predecessor_id)
                            && u64::from(head.onset_ticks) + u64::from(head.duration_ticks)
                                == u64::from(note.onset_ticks)
                    })
                    .expect("cross-track continuation needs its touching predecessor in this lane");
                let head = &predecessor
                    .source_evidence
                    .as_ref()
                    .unwrap()
                    .origin
                    .as_ref()
                    .unwrap()
                    .source;
                let tail = &origin.source;
                assert!(
                    tail.part_id.is_some() && tail.staff_id.is_some() && tail.voice.is_some(),
                    "sibling continuity needs native ownership"
                );
                assert_eq!(
                    (&tail.part_id, &tail.staff_id, &tail.voice, tail.occurrence),
                    (&head.part_id, &head.staff_id, &head.voice, head.occurrence),
                    "continuation crossed source ownership"
                );
                assert_eq!(
                    tail.continuity.as_ref().map(|c| c.playback_segment),
                    head.continuity.as_ref().map(|c| c.playback_segment)
                );
            }
            for (declared, owned) in [
                (&destination.source.part_id, &origin.source.part_id),
                (&destination.source.staff_id, &origin.source.staff_id),
                (&destination.source.voice, &origin.source.voice),
            ] {
                if declared.is_some() {
                    assert_eq!(declared, owned, "destination lane changed native ownership");
                }
            }
            assert_eq!(
                evidence.note_on_event_id,
                format!("event:{}:{}", origin.track_id, origin.note_on_order)
            );
            assert_eq!(
                evidence.note_off_event_id,
                format!("event:{}:{}", origin.track_id, origin.note_off_order)
            );
            let on = events[&evidence.note_on_event_id];
            let off = events[&evidence.note_off_event_id];
            let Kind::NoteOn(original) = &on.kind else {
                panic!("retained attack is not a source note-on");
            };
            assert_eq!(
                (
                    &origin.source.id,
                    &origin.source.part_id,
                    &origin.source.staff_id,
                    &origin.source.voice,
                    origin.source.occurrence
                ),
                (
                    &original.source.id,
                    &original.source.part_id,
                    &original.source.staff_id,
                    &original.source.voice,
                    original.source.occurrence
                ),
                "source origin ownership changed"
            );
            let (channel, pitch, source_id) = match &off.kind {
                Kind::NoteOff(end) => (end.channel, end.key, end.source_id.as_deref()),
                Kind::NoteOn(end) if end.velocity == Some(0) => (end.channel, end.key, None),
                _ => panic!("retained end is not a source note-off"),
            };
            assert_eq!((channel, pitch), (original.channel, original.key));
            if let Some(id) = source_id {
                assert_eq!(id, original.source.id);
            }
            assert_eq!(Some(note.pitch), original.key, "invented or changed pitch");
            assert_eq!(
                evidence.note_id,
                format!(
                    "note:{}:{}:occurrence:{}:event:{}",
                    origin.track_id, original.source.id, original.source.occurrence, on.order
                )
            );
            assert!(off.tick > on.tick, "nonpositive original note");
            assert!(note.duration_ticks > 0, "nonpositive projected note");
            let id = evidence.note_id.as_str();
            bounds.insert(id, (on.tick, u64::from(off.tick)));
            intervals.entry(id).or_default().push((
                note.onset_ticks,
                u64::from(note.onset_ticks) + u64::from(note.duration_ticks),
            ));
            if let Some(lyric) = source_lyric(&note.lyric) {
                assert!(evidence.lyric_id.is_some(), "lyric has no source record");
                if let Some(event_id) = &evidence.lyric_event_id {
                    match &events[event_id].kind {
                        Kind::Lyrics(original) => {
                            assert_eq!(lyric.id, original.id);
                            assert_eq!(lyric.raw, original.raw);
                            assert_eq!(lyric.raw_bytes, original.raw_bytes);
                        }
                        Kind::Text(original) => {
                            assert_eq!(lyric.raw, original.text);
                            assert_eq!(lyric.raw_bytes, original.raw);
                        }
                        _ => panic!("lyric event is not source text"),
                    }
                } else {
                    let original = original
                        .lyrics
                        .iter()
                        .find(|candidate| candidate.id == lyric.id)
                        .expect("lyric must belong to its exact source note");
                    assert_eq!(lyric.raw, original.raw);
                    assert_eq!(lyric.raw_bytes, original.raw_bytes);
                    assert_eq!(lyric.fragments, original.fragments);
                    assert_eq!((&lyric.lane, lyric.verse), (&original.lane, original.verse));
                }
            }
        }
        for (id, mut pieces) in intervals {
            pieces.sort();
            let (start, end) = bounds[id];
            assert_eq!(pieces[0].0, start, "source onset changed: {id}");
            assert_eq!(
                pieces.last().unwrap().1,
                end,
                "source duration changed: {id}"
            );
            assert!(
                pieces
                    .windows(2)
                    .all(|pair| pair[0].1 == u64::from(pair[1].0)),
                "source note split has a gap or overlap: {id}"
            );
        }
    }
}

// Independent, minimal consumers of the durable file. They do not reuse either
// writer's model or conversion arithmetic, and require musical fields to exist.
#[derive(Deserialize)]
struct SvpRead {
    time: SvpTime,
    tracks: Vec<SvpTrack>,
}
#[derive(Deserialize)]
struct SvpTime {
    tempo: Vec<TempoRead>,
}
#[derive(Deserialize)]
struct TempoRead {
    position: u64,
    bpm: f64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SvpTrack {
    name: String,
    mixer: SvpMixer,
    main_ref: SvpReference,
    main_group: SvpGroup,
}
#[derive(Deserialize)]
struct SvpMixer {
    mute: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SvpReference {
    blick_offset: i64,
    #[serde(default)]
    pitch_offset: i64,
}
#[derive(Deserialize)]
struct SvpGroup {
    notes: Vec<SvpNote>,
}
#[derive(Deserialize)]
struct SvpNote {
    onset: u64,
    duration: u64,
    pitch: u8,
    lyrics: String,
}
#[derive(Deserialize)]
struct UstxRead {
    resolution: u64,
    tempos: Vec<TempoRead>,
    tracks: Vec<UstxTrack>,
    voice_parts: Vec<UstxPart>,
}
#[derive(Deserialize)]
struct UstxTrack {
    track_name: String,
    mute: bool,
}
#[derive(Deserialize)]
struct UstxPart {
    track_no: usize,
    position: u64,
    notes: Vec<UstxNote>,
}
#[derive(Deserialize)]
struct UstxNote {
    position: u64,
    duration: u64,
    tone: u8,
    lyric: String,
}

fn assert_rational(actual: u64, units: u64, ticks: u32, ppq: u16) {
    assert_eq!(
        u128::from(actual) * u128::from(ppq),
        u128::from(ticks) * u128::from(units),
        "rounded timing: {actual}/{units} != {ticks}/{ppq} quarter notes"
    );
}

/// Contract-level spelling oracle, independent of either writer. Joined words
/// live in the source lyric's semantic state; immutable raw fragments were
/// checked against their source above. Manual hints retain every character.
fn expected_lyric(
    target: ExportTarget,
    project: &ProjectedProject,
    note: &ProjectedNote,
) -> String {
    if target == ExportTarget::Ustx {
        match &note.lyric {
            ProjectedLyric::Pronounced { text, phonemes, .. } => {
                return if phonemes.is_empty() {
                    text.clone()
                } else {
                    format!("{text}[{phonemes}]")
                };
            }
            ProjectedLyric::PronouncedSplit { .. } => return "+".into(),
            _ => {}
        }
    }
    match &note.lyric {
        ProjectedLyric::Absent => String::new(),
        ProjectedLyric::Extension => if target == ExportTarget::Svp {
            "-"
        } else {
            "+~"
        }
        .into(),
        ProjectedLyric::Source(source)
        | ProjectedLyric::Pronounced { source, .. }
        | ProjectedLyric::PronouncedSplit { source } => match &source.state {
            LyricState::Text(text) => {
                if target == ExportTarget::Ustx
                    && project.pronunciation_profile == PronunciationProfile::Automatic
                    && note.pronunciation_language == Some(PronunciationLanguage::French)
                {
                    french_render_text(text)
                } else {
                    text.clone()
                }
            }
            LyricState::Continuation => if target == ExportTarget::Svp {
                "-"
            } else {
                "+~"
            }
            .into(),
            LyricState::SyllableSplit => "+".into(),
            LyricState::ExplicitEmpty | LyricState::Unsupported(_) => String::new(),
        },
    }
}

fn french_render_text(text: &str) -> String {
    const HYPHENS: &str = "-‐‑‒–—―−﹘﹣－";
    let edge = |c: char| {
        c.is_whitespace() || ",.;:!?…\"«»‹›“”„‚'‘’(){}".contains(c) || HYPHENS.contains(c)
    };
    if text.contains(['[', ']'])
        || text.trim_start().starts_with(['+', '?'])
        || text
            .trim_start_matches(|c| edge(c) && c != '?' && c != '+')
            .starts_with(['+', '?'])
    {
        return text.into();
    }
    let core = text.trim_matches(edge);
    if !core.chars().any(char::is_alphabetic) {
        return text.into();
    }
    let start = text.len() - text.trim_start_matches(edge).len();
    let end = start + core.len();
    text.char_indices()
        .filter_map(|(index, c)| {
            ((start..end).contains(&index) || !HYPHENS.contains(c)).then_some(c)
        })
        .collect()
}

fn assert_readback(target: ExportTarget, project: &ProjectedProject, bytes: &[u8]) {
    let (units, tempos, lanes) = match target {
        ExportTarget::Svp => {
            let read: SvpRead = serde_json::from_slice(bytes).expect("durable SVP JSON");
            let lanes = read
                .tracks
                .into_iter()
                .map(|track| {
                    assert_eq!(
                        track.main_ref.blick_offset, 0,
                        "unexpected track time offset"
                    );
                    assert_eq!(
                        track.main_ref.pitch_offset, 0,
                        "unexpected track transposition"
                    );
                    (
                        track.name,
                        track.mixer.mute,
                        track
                            .main_group
                            .notes
                            .into_iter()
                            .map(|note| (note.onset, note.duration, note.pitch, note.lyrics))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            (BLICKS, read.time.tempo, lanes)
        }
        ExportTarget::Ustx => {
            let read: UstxRead = serde_yaml::from_slice(bytes).expect("durable USTX YAML");
            assert_eq!(read.resolution, USTX_TICKS);
            assert_eq!(read.voice_parts.len(), read.tracks.len());
            let mut parts = BTreeMap::new();
            for part in read.voice_parts {
                assert!(part.track_no < read.tracks.len(), "orphan voice part");
                assert_eq!(part.position, 0, "unexpected voice part offset");
                assert!(
                    parts.insert(part.track_no, part).is_none(),
                    "duplicate voice part"
                );
            }
            let lanes = read
                .tracks
                .into_iter()
                .enumerate()
                .map(|(index, track)| {
                    let part = parts.remove(&index).expect("voice part for each track");
                    (
                        track.track_name,
                        track.mute,
                        part.notes
                            .into_iter()
                            .map(|note| (note.position, note.duration, note.tone, note.lyric))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            (USTX_TICKS, read.tempos, lanes)
        }
    };
    assert_eq!(tempos.len(), project.tempos.len(), "written tempo count");
    let mut expected_tempos: Vec<_> = project.tempos.iter().collect();
    expected_tempos.sort_by_key(|tempo| tempo.tick);
    for (written, expected) in tempos.iter().zip(expected_tempos) {
        assert_rational(
            written.position,
            units,
            expected.tick,
            project.ticks_per_beat,
        );
        assert!(
            bpm_matches(written.bpm, expected.bpm),
            "written BPM changed"
        );
    }
    assert_eq!(lanes.len(), project.tracks.len(), "written lane count");
    let mut lyric_slots = 0;
    let mut written_lyric_slots = 0;
    for ((name, muted, written), expected) in lanes.iter().zip(&project.tracks) {
        assert_eq!(name, &expected.name);
        assert_eq!(*muted, expected.muted);
        assert_eq!(written.len(), expected.notes.len(), "written note count");
        let mut notes: Vec<_> = expected.notes.iter().collect();
        if target == ExportTarget::Ustx {
            notes.sort_by_key(|note| note.onset_ticks);
        }
        for ((onset, duration, pitch, text), note) in written.iter().zip(notes) {
            assert_rational(*onset, units, note.onset_ticks, project.ticks_per_beat);
            assert_rational(
                *duration,
                units,
                note.duration_ticks,
                project.ticks_per_beat,
            );
            assert_eq!(*pitch, note.pitch, "written pitch changed");
            assert_eq!(
                text,
                &expected_lyric(target, project, note),
                "written lyric or pronunciation hint changed"
            );
            if let Some(source) = source_lyric(&note.lyric) {
                if !source.raw.trim().is_empty()
                    && matches!(
                        source.state,
                        LyricState::Text(_) | LyricState::SyllableSplit
                    )
                {
                    lyric_slots += 1;
                    assert!(!text.is_empty(), "source-backed lyric slot erased");
                    written_lyric_slots += 1;
                }
            }
            if matches!(note.lyric, ProjectedLyric::Absent) {
                assert!(text.is_empty(), "invented lyric on an untexted note");
            }
        }
    }
    assert_eq!(
        written_lyric_slots, lyric_slots,
        "source-backed lyric slot count"
    );
}

/// An error string alone does not authorize an expected target refusal. Require
/// an independently demonstrable grid, duration-floor, or integer-range failure.
fn exactness_reason(
    target: ExportTarget,
    project: &ProjectedProject,
    error: &str,
) -> Option<&'static str> {
    let units = match target {
        ExportTarget::Svp => BLICKS,
        ExportTarget::Ustx => USTX_TICKS,
    };
    let ppq = u128::from(project.ticks_per_beat);
    if ppq == 0 {
        return None;
    }
    let quantities = project
        .tracks
        .iter()
        .flat_map(|track| &track.notes)
        .flat_map(|note| [note.onset_ticks, note.duration_ticks])
        .chain(project.tempos.iter().map(|tempo| tempo.tick));
    if error.contains("cannot be represented exactly")
        && quantities
            .clone()
            .any(|ticks| u128::from(ticks) * u128::from(units) % ppq != 0)
    {
        return Some("target_grid_is_inexact");
    }
    if target == ExportTarget::Ustx
        && error.contains("under the 10-tick floor")
        && project
            .tracks
            .iter()
            .flat_map(|track| &track.notes)
            .any(|note| {
                let numerator = u128::from(note.duration_ticks) * u128::from(USTX_TICKS);
                numerator % ppq == 0 && numerator / ppq < 10
            })
    {
        return Some("ustx_native_duration_floor");
    }
    let limit = match target {
        ExportTarget::Svp => i64::MAX as u128,
        ExportTarget::Ustx => i32::MAX as u128,
    };
    if (error.contains("exceeds the") || error.contains("beyond the 32-bit tick range"))
        && (quantities
            .clone()
            .any(|ticks| u128::from(ticks) * u128::from(units) / ppq > limit)
            || (target == ExportTarget::Ustx
                && project
                    .tracks
                    .iter()
                    .flat_map(|track| &track.notes)
                    .any(|note| {
                        (u128::from(note.onset_ticks) + u128::from(note.duration_ticks))
                            * u128::from(units)
                            / ppq
                            > limit
                    })))
    {
        return Some("target_integer_range");
    }
    None
}

fn source_refusal(midi: &Midi, error: &str) -> Option<&'static str> {
    if midi.format == 2
        && error == "MIDI format 2 contains independent sequences and cannot be flattened safely"
    {
        return Some("independent_midi_sequences");
    }
    if matches!(midi.time_base, TimeBase::Smpte { .. })
        && error == "SMPTE-timed MIDI is preserved but SVP projection is not supported yet"
    {
        return Some("smpte_timebase_not_projectable");
    }
    if error
        .starts_with("MIDI meter cannot be projected safely: time signature change at MIDI tick ")
        && error.contains(" falls inside a ")
        && error.ends_with(" measure; Synthesizer V meter changes require a measure boundary")
    {
        return Some("meter_change_inside_measure");
    }
    None
}

fn project_receipt(project: &ProjectedProject) -> Value {
    let mut lyric_records = BTreeSet::new();
    let mut lyric_notes = 0;
    let lanes: Vec<_> = project.tracks.iter().map(|lane| {
        let notes: Vec<_> = lane.notes.iter().map(|note| {
            if let Some(source) = source_lyric(&note.lyric) {
                if !source.raw.trim().is_empty() {
                    lyric_notes += 1;
                    lyric_records.insert(note.source_evidence.as_ref().unwrap().lyric_id.clone());
                }
            }
            json!({"onset_ticks": note.onset_ticks, "duration_ticks": note.duration_ticks,
                "pitch": note.pitch, "note_id": note.source_evidence.as_ref().map(|e| &e.note_id),
                "lyric_record_id": note.source_evidence.as_ref().and_then(|e| e.lyric_id.as_ref())})
        }).collect();
        json!({"source_track_id": lane.source_track_id, "name": lane.name, "muted": lane.muted, "notes": notes})
    }).collect();
    json!({"ticks_per_quarter": project.ticks_per_beat,
        "note_count": project.tracks.iter().map(|lane| lane.notes.len()).sum::<usize>(),
        "source_backed_lyric_notes": lyric_notes, "distinct_lyric_records": lyric_records.len(),
        "tempo_map": project.tempos.iter().map(|t| json!({"tick": t.tick, "bpm": t.bpm})).collect::<Vec<_>>(),
        "tracks": lanes})
}

fn check_writers(project: &ProjectedProject) -> Result<Value, String> {
    let mut checks = Vec::new();
    for target in TARGETS {
        let verdict = target::validate_for(target, project);
        let written = target::serialize_to(target, project);
        match (verdict, written) {
            (Ok(()), Ok(bytes)) => {
                assert_readback(target, project, &bytes);
                checks.push(json!({"target": target, "status": "accepted"}));
            }
            (Err(reason), Err(SerializeError::Unrepresentable(writer_reason))) => {
                assert_eq!(
                    reason, writer_reason,
                    "validator/writer refusal disagreement"
                );
                let classification = exactness_reason(target, project, &reason)
                    .ok_or_else(|| format!("unclassified {target:?} writer refusal: {reason}"))?;
                checks.push(
                    json!({"target": target, "status": "expected_exactness_refusal",
                    "classification": classification, "refusal": reason}),
                );
            }
            (verdict, written) => {
                return Err(format!(
                    "{target:?} validator/writer disagreement: {verdict:?}; {written:?}"
                ))
            }
        }
    }
    Ok(json!(checks))
}

fn configurations(midi: &Midi, report_dir: &Path, file_index: usize) -> Vec<Value> {
    let mut rows = Vec::new();
    for profile in PROFILES {
        let outcomes: Vec<_> = TARGETS
            .iter()
            .map(|&target| {
                capture(|| {
                    Ok(convert_midi_with_profile(
                        midi, "english", None, target, profile,
                    ))
                })
            })
            .collect();
        for (index, &target) in TARGETS.iter().enumerate() {
            let filename = format!(
                "{file_index:06}-{}.{}",
                profile_name(profile),
                target.extension()
            );
            let destination = report_dir.join(&filename);
            let mut row = json!({"target": target, "profile": profile,
                "status": "unexpected_failure", "export_filename": null});
            let checked = capture(|| {
                let outcome = outcomes[index].as_ref().map_err(Clone::clone)?;
                if !outcome.ok {
                    assert!(
                        outcome.svp.is_none(),
                        "failed analysis carries a successful projection"
                    );
                    let reason = outcome
                        .msg
                        .as_deref()
                        .unwrap_or("analysis failed without a diagnostic");
                    if let Some(classification) = source_refusal(midi, reason) {
                        return Ok(
                            json!({"status": "expected_source_refusal", "classification": classification, "refusal": reason}),
                        );
                    }
                    if let Some(reference) = outcomes
                        .iter()
                        .filter_map(|o| o.as_ref().ok())
                        .filter(|o| o.ok)
                        .find_map(|o| o.svp.as_ref())
                    {
                        let verdict = target::validate_for(target, reference)
                            .expect_err("analysis refusal must agree with validator");
                        let writer = target::serialize_to(target, reference)
                            .expect_err("refused analysis cannot produce bytes");
                        assert_eq!(writer, SerializeError::Unrepresentable(verdict.clone()));
                        let wrapped = match target {
                            ExportTarget::Svp => {
                                format!("source timing cannot be projected safely: {verdict}")
                            }
                            ExportTarget::Ustx => format!(
                                "the source cannot be projected safely to OpenUtau: {verdict}"
                            ),
                        };
                        assert_eq!(reason, wrapped, "analysis/validator refusal disagreement");
                        if let Some(classification) = exactness_reason(target, reference, &verdict)
                        {
                            return Ok(
                                json!({"status": "expected_exactness_refusal", "classification": classification, "refusal": reason,
                                "reference_projection": project_receipt(reference)}),
                            );
                        }
                    }
                    return Err(format!("unclassified analysis failure: {reason}"));
                }
                let project = outcome
                    .svp
                    .as_ref()
                    .expect("successful analysis requires projection");
                assert_eq!(outcome.topology, midi.topology, "source topology changed");
                assert_eq!(project.pronunciation_profile, profile.for_target(target));
                assert_source(project, midi, outcome);
                let writer_checks = check_writers(project)?;
                target::validate_for(target, project)?;
                let bytes = target::serialize_to(target, project).map_err(|e| e.to_string())?;
                write_new(&destination, &bytes)?;
                let durable = fs::read(&destination).map_err(|e| e.to_string())?;
                assert_eq!(durable, bytes, "durable export bytes changed");
                assert_readback(target, project, &durable);
                let notes = project
                    .tracks
                    .iter()
                    .map(|lane| lane.notes.len())
                    .sum::<usize>();
                Ok(
                    json!({"status": if notes == 0 { "exported_empty" } else { "exported" },
                    "export_filename": filename, "export_sha256": hash(&durable),
                    "projection": project_receipt(project), "writer_checks": writer_checks,
                    "reported_placed_lyrics": outcome.placed,
                    "source_lyric_count": outcome.tracks.iter().map(|t| t.lyric_status.source_text_count).sum::<usize>(),
                    "diagnostics": outcome.source_warnings.iter().chain(outcome.tracks.iter().flat_map(|t| &t.warnings)).collect::<Vec<_>>()}),
                )
            });
            match checked {
                Ok(details) => row
                    .as_object_mut()
                    .unwrap()
                    .extend(details.as_object().unwrap().clone()),
                Err(error) => {
                    row["error"] = json!(error);
                    if outcomes
                        .iter()
                        .all(|outcome| outcome.as_ref().is_ok_and(|outcome| !outcome.ok))
                    {
                        row["helper_limitation"] = json!("Both targets refused analysis; no successful neutral projection supplies an independent exactness witness. Kept unexpected and failing; this may reject a valid corpus.");
                    }
                    if destination.exists() {
                        row["partial_export_filename"] = json!(filename);
                    }
                }
            }
            rows.push(row);
        }
    }
    rows
}

fn prepare_report_directory(path: &Path, corpus: &Path) -> PathBuf {
    let parent = path
        .parent()
        .expect("report directory needs a parent")
        .canonicalize()
        .expect("report parent must exist");
    let destination = parent.join(path.file_name().expect("report directory needs a name"));
    assert!(
        !destination.starts_with(corpus),
        "report must be outside the input corpus"
    );
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    if let Ok(relative) = destination.strip_prefix(&repository) {
        assert!(
            relative.starts_with("_bmad-output")
                || relative.starts_with("src-tauri/target")
                || relative.starts_with("src-tauri/tests/fixtures"),
            "private reports must be outside Git or inside an ignored directory"
        );
    }
    fs::create_dir(&destination)
        .expect("report directory must be new; existing results are never replaced");
    destination
}

#[test]
#[ignore = "requires VERSE_RHYTHM_CORPUS_DIR and a new private VERSE_RHYTHM_REPORT_DIR; no audio qualification"]
fn private_rhythm_corpus_preserves_both_targets() {
    let corpus = PathBuf::from(
        std::env::var_os("VERSE_RHYTHM_CORPUS_DIR").expect("VERSE_RHYTHM_CORPUS_DIR is required"),
    )
    .canonicalize()
    .expect("private corpus directory must exist");
    assert!(
        corpus.is_dir() && !inside_bundle(&corpus),
        "corpus must be a directory outside .versebundle"
    );
    let report_dir = prepare_report_directory(
        &PathBuf::from(
            std::env::var_os("VERSE_RHYTHM_REPORT_DIR")
                .expect("VERSE_RHYTHM_REPORT_DIR is required"),
        ),
        &corpus,
    );
    let (mut paths, mut excluded, mut discovery_errors) = (Vec::new(), Vec::new(), Vec::new());
    discover(
        &corpus,
        &mut BTreeSet::new(),
        &mut paths,
        &mut excluded,
        &mut discovery_errors,
    );
    paths.sort();
    if paths.is_empty() {
        discovery_errors.push("no supported inputs discovered".into());
    }
    let mut rows = Vec::new();
    let mut duplicates: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (index, path) in paths.iter().enumerate() {
        let mut row = json!({"filename": path.file_name().map(|s| s.to_string_lossy()),
            "relative_path": path.strip_prefix(&corpus).unwrap(), "path": path,
            "sha256_before": null, "sha256_after": null, "input_status": "unexpected_failure"});
        match fs::read(path) {
            Ok(bytes) => {
                let before = hash(&bytes);
                duplicates
                    .entry(before.clone())
                    .or_default()
                    .push(path.display().to_string());
                row["sha256_before"] = json!(before);
                row["input_bytes"] = json!(bytes.len());
                match capture(|| parse(&bytes, path)) {
                    Ok(midi) => {
                        row["input_status"] = json!("parsed");
                        row["source_format"] = json!(format!("{:?}", midi.source_format));
                        row["source_note_attacks"] = json!(midi
                            .tracks
                            .iter()
                            .flat_map(|t| &t.events)
                            .filter(|e| matches!(&e.kind, Kind::NoteOn(n) if n.velocity != Some(0)))
                            .count());
                        row["configurations"] =
                            json!(configurations(&midi, &report_dir, index + 1));
                        if midi.source_format == midi::SourceFormat::MuseScore {
                            match capture(|| {
                                let mut source = ScoreStems::read(&bytes)?;
                                let warnings = source.prepare_for_renderer(4)?;
                                let name =
                                    format!("{:06}-renderer.{}", index + 1, source.extension());
                                write_new(&report_dir.join(&name), source.render_container())?;
                                Ok((name, warnings))
                            }) {
                                Ok((name, warnings)) => {
                                    row["prepared_filename"] = json!(name);
                                    row["renderer_preparation_warnings"] = json!(warnings);
                                }
                                Err(error) => {
                                    row["input_status"] = json!("unexpected_failure");
                                    row["error"] = json!(format!("renderer preparation: {error}"));
                                }
                            }
                        }
                    }
                    Err(error) => {
                        row["error"] = json!(&error);
                        // A diagnostic string is not an independent witness of
                        // lost representability. Every parser error fails closed.
                        row["input_status"] = json!("unexpected_failure");
                        row["classification"] = json!("parser_error_without_independent_witness");
                    }
                }
            }
            Err(error) => row["error"] = json!(error.to_string()),
        }
        // Even unreadable/rejected inputs get all four explicit configuration rows.
        if row.get("configurations").is_none() {
            row["configurations"] = json!(PROFILES.iter().flat_map(|&profile| TARGETS.iter().map(move |&target|
                json!({"target": target, "profile": profile, "status": "blocked_by_input", "export_filename": null}))).collect::<Vec<_>>());
        }
        rows.push(row);
    }
    // Rehash after the entire run: catches mutations caused while later files
    // were processed as well as while this file itself was being projected.
    let mut hash_failures = Vec::new();
    for (path, row) in paths.iter().zip(&mut rows) {
        match fs::read(path) {
            Ok(bytes) => {
                row["sha256_after"] = json!(hash(&bytes));
                if row["sha256_after"] != row["sha256_before"] {
                    hash_failures.push(format!(
                        "input changed or was unreadable before: {}",
                        path.display()
                    ));
                }
            }
            Err(error) => hash_failures.push(format!("{}: {error}", path.display())),
        }
    }
    let mut counts = BTreeMap::<String, usize>::new();
    let mut unexpected = Vec::new();
    let mut configuration_count = 0;
    for row in &rows {
        if row["input_status"] == "unexpected_failure" {
            unexpected.push(json!({"path": row["path"], "error": row["error"]}));
        }
        for configuration in row["configurations"].as_array().unwrap() {
            configuration_count += 1;
            *counts
                .entry(configuration["status"].as_str().unwrap().into())
                .or_default() += 1;
            if configuration["status"] == "unexpected_failure" {
                unexpected.push(json!({"path": row["path"], "configuration": configuration}));
            }
        }
    }
    let summary = json!({"discovered_input_paths": paths.len(), "configuration_count": configuration_count,
    "unique_input_hashes": duplicates.len(), "configuration_status_counts": counts,
    "parsed_inputs": rows.iter().filter(|r| r["input_status"] == "parsed").count(),
    "failed_inputs": rows.iter().filter(|r| r["input_status"] == "unexpected_failure").count(),
    "duplicate_content_paths": duplicates.iter().filter(|(_, paths)| paths.len() > 1).collect::<BTreeMap<_, _>>(),
    "excluded_bundle_paths": excluded, "discovery_errors": discovery_errors,
    "input_hash_failures": hash_failures, "unexpected_failures": unexpected,
    "audio_qualified": false,
    "evidence_limits": {
        "parser": "Source IR is shared adapter evidence, not independent parser correctness proof.",
        "both_target_refusals": "Without a successful projection there is no independent exactness witness; the helper fails closed and may reject a valid corpus.",
        "parser_refusals": "All parser errors remain unexpected failures without an independent witness."
    }});
    let report = json!({"schema_version": 1, "corpus_directory": corpus, "report_directory": report_dir,
        "language_hint": "english", "profiles": PROFILES, "targets": TARGETS, "summary": summary, "files": rows});
    write_new(
        &report_dir.join("report.json"),
        &serde_json::to_vec_pretty(&report).unwrap(),
    )
    .expect("durable corpus report");
    write_new(
        &report_dir.join("summary.json"),
        &serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .expect("durable corpus summary");
    #[cfg(unix)]
    fs::File::open(&report_dir)
        .and_then(|directory| directory.sync_all())
        .expect("durable report directory");
    eprintln!(
        "private rhythm receipt: {}",
        report_dir.join("report.json").display()
    );
    assert_eq!(
        configuration_count,
        paths.len() * 4,
        "every input needs both targets and both profiles"
    );
    assert!(
        discovery_errors.is_empty(),
        "incomplete discovery: {discovery_errors:?}"
    );
    assert!(
        hash_failures.is_empty(),
        "source hashes changed: {hash_failures:?}"
    );
    assert!(
        unexpected.is_empty(),
        "unexplained failures; inspect {}",
        report_dir.join("report.json").display()
    );
    assert!(counts.get("exported").copied().unwrap_or(0) > 0, "no nonempty export was verified; refusals and empty projects are not proof of vocal conservation");
}
