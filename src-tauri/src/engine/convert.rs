//! MIDI -> Synthesizer V conversion logic. 1:1 port of kar2svp_core.py.
use crate::engine::midi::{
    self, Kind, LineBreak, Lyric, Midi, MidiTextProfile, NoteOn, SourceTopology, TimeBase, Track,
    TrackRoleHint,
};
use crate::engine::svp::*;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceRole {
    Vocal,
    Instrumental,
    Percussion,
    Mixed,
    LyricsOnly,
    Metadata,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LyricStatusState {
    SourceOwned,
    ExplicitEmpty,
    MetadataOnly,
    None,
    Ambiguous,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricStatus {
    pub state: LyricStatusState,
    pub source_text_count: usize,
    pub projected_text_count: usize,
    pub explicit_empty_count: usize,
    pub continuation_count: usize,
    pub unsupported_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportRepresentation {
    VocalNotes,
    ReferenceMixMember,
    VocalNotesAndReferenceMix,
    SourceOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TrackReport {
    pub id: usize, // stable track identifier (original order), used for overrides
    pub source_id: String,
    pub track: String,
    pub notes: usize,
    /// Compatibility summary for older callers. New UI code uses
    /// `source_role` and `export_representation`, which deliberately separate
    /// source evidence from a user-selected vocal projection.
    pub role: String,
    pub placed: usize,
    pub source_role: SourceRole,
    pub lyric_status: LyricStatus,
    pub export_representation: ExportRepresentation,
    pub requires_voice_assignment: bool,
    pub warnings: Vec<Diagnostic>,
}

pub struct ConvertOutcome {
    pub ok: bool,
    pub msg: Option<String>,
    pub svp: Option<SvpProject>,
    pub topology: SourceTopology,
    pub tracks: Vec<TrackReport>,
    pub n_tracks: usize,
    pub placed: usize,
    pub projection: ProjectionEvidence,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectionEvidence {
    /// IDs use the exact namespace consumed by the preservation ledger:
    /// `track:…`, `event:…`, `note:…`, and `lyric:…`.
    pub source_ids: BTreeSet<String>,
}

pub(crate) fn note_instance_id(
    track_id: &str,
    source: &midi::NoteSource,
    note_on_order: u32,
) -> String {
    format!(
        "note:{track_id}:{}:occurrence:{}:event:{note_on_order}",
        source.id, source.occurrence
    )
}

pub(crate) fn attached_lyric_instance_id(
    lyric: &Lyric,
    source: &midi::NoteSource,
    note_on_order: u32,
) -> String {
    format!(
        "lyric:{}:occurrence:{}:note-event:{note_on_order}",
        lyric.id, source.occurrence
    )
}

pub(crate) fn standalone_lyric_instance_id(lyric: &Lyric, track_id: &str, order: u32) -> String {
    format!("lyric:{}:event:{track_id}:{order}", lyric.id)
}

#[derive(Clone, Debug)]
struct SourceNote {
    onset: u32,
    duration: u32,
    pitch: Option<u8>,
    source_order: u32,
    end_order: u32,
    source: midi::NoteSource,
    lyrics: Vec<Lyric>,
}

/// Native MIDI notes are paired FIFO by `(channel, key)`. XML adapters also
/// supply an exact source ID so overlapping same-key voices close correctly.
fn extract_notes(track: &Track) -> Vec<SourceNote> {
    let mut active: HashMap<String, (u32, u32, NoteOn)> = HashMap::new();
    let mut by_key: HashMap<(Option<u8>, Option<u8>), Vec<String>> = HashMap::new();
    let mut out = Vec::new();
    for event in &track.events {
        match &event.kind {
            Kind::NoteOn(note) if note.velocity == Some(0) => {
                close_fifo_note(
                    &mut active,
                    &mut by_key,
                    note.channel,
                    note.key,
                    event.tick,
                    event.order,
                    &mut out,
                );
            }
            Kind::NoteOn(note) => {
                let id = note.source.id.clone();
                active.insert(id.clone(), (event.tick, event.order, note.clone()));
                by_key.entry((note.channel, note.key)).or_default().push(id);
            }
            Kind::NoteOff(note) => {
                if let Some(source_id) = &note.source_id {
                    if let Some((onset, source_order, start)) = active.remove(source_id) {
                        if let Some(ids) = by_key.get_mut(&(note.channel, note.key)) {
                            ids.retain(|id| id != source_id);
                        }
                        finish_note(
                            onset,
                            source_order,
                            start,
                            event.tick,
                            event.order,
                            &mut out,
                        );
                    }
                } else {
                    close_fifo_note(
                        &mut active,
                        &mut by_key,
                        note.channel,
                        note.key,
                        event.tick,
                        event.order,
                        &mut out,
                    );
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|note| (note.onset, note.source_order, note.pitch));
    out
}

fn close_fifo_note(
    active: &mut HashMap<String, (u32, u32, NoteOn)>,
    by_key: &mut HashMap<(Option<u8>, Option<u8>), Vec<String>>,
    channel: Option<u8>,
    key: Option<u8>,
    end: u32,
    end_order: u32,
    out: &mut Vec<SourceNote>,
) {
    let Some(ids) = by_key.get_mut(&(channel, key)) else {
        return;
    };
    while !ids.is_empty() {
        let id = ids.remove(0);
        if let Some((onset, source_order, start)) = active.remove(&id) {
            finish_note(onset, source_order, start, end, end_order, out);
            break;
        }
    }
}

fn finish_note(
    onset: u32,
    source_order: u32,
    start: NoteOn,
    end: u32,
    end_order: u32,
    out: &mut Vec<SourceNote>,
) {
    if end >= onset {
        out.push(SourceNote {
            onset,
            duration: end - onset,
            pitch: start.key,
            source_order,
            end_order,
            source: start.source,
            lyrics: start.lyrics,
        });
    }
}

#[derive(Clone, Debug)]
struct TimedLyric {
    track_id: String,
    tick: u32,
    order: u32,
    lyric: Lyric,
    origin: TimedLyricOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimedLyricOrigin {
    MidiLyric,
    KaraokeText,
}

pub(crate) fn karaoke_text_lyric(
    track_id: &str,
    tick: u32,
    order: u32,
    text: &midi::TextEvent,
) -> Option<Lyric> {
    let raw = text.text.as_str();
    if midi::is_soft_karaoke_text_control(raw) {
        return None;
    }
    let (line_break, value) = if let Some(rest) = raw.strip_prefix('\\') {
        (Some(LineBreak::Paragraph), rest)
    } else if let Some(rest) = raw.strip_prefix('/') {
        (Some(LineBreak::Line), rest)
    } else {
        (None, raw)
    };
    let mut lyric = Lyric::text(format!("{track_id}-text-{tick}-{order}"), value.to_string());
    lyric.raw = raw.to_string();
    lyric.raw_bytes = text.raw.clone();
    lyric.line_break = line_break;
    Some(lyric)
}

/// Generic MIDI Text is metadata. It is considered lyric material only under
/// evidence carried by this exact track.
fn track_tokens(track: &Track) -> Vec<TimedLyric> {
    let mut tokens = Vec::new();
    for event in &track.events {
        let (lyric, origin) = match &event.kind {
            Kind::Lyrics(lyric) if !midi::is_midi_lyric_line_break(&lyric.raw) => {
                (Some(lyric.clone()), TimedLyricOrigin::MidiLyric)
            }
            Kind::Text(text) if track.text_profile == MidiTextProfile::KaraokeLyrics => (
                karaoke_text_lyric(&track.id, event.tick, event.order, text),
                TimedLyricOrigin::KaraokeText,
            ),
            _ => (None, TimedLyricOrigin::MidiLyric),
        };
        if let Some(lyric) = lyric {
            tokens.push(TimedLyric {
                track_id: track.id.clone(),
                tick: event.tick,
                order: event.order,
                lyric,
                origin,
            });
        }
    }
    tokens.sort_by_key(|token| (token.tick, token.order));
    tokens
}

const BLICKS_PER_QUARTER_INTEGER: u64 = 705_600_000;

fn exact_blick_position(ticks: u32, ticks_per_beat: u16, context: &str) -> Result<i64, String> {
    if ticks_per_beat == 0 {
        return Err("MIDI PPQ division must be non-zero".into());
    }
    let numerator = u128::from(ticks) * u128::from(BLICKS_PER_QUARTER_INTEGER);
    let denominator = u128::from(ticks_per_beat);
    if numerator % denominator != 0 {
        return Err(format!(
            "{context} at MIDI tick {ticks} cannot be represented exactly in Synthesizer V blicks \
             with PPQ {ticks_per_beat}"
        ));
    }
    i64::try_from(numerator / denominator)
        .map_err(|_| format!("{context} exceeds the Synthesizer V blick range"))
}

fn read_tempo(midi: &Midi, ticks_per_beat: u16) -> Result<(Vec<Tempo>, BTreeSet<String>), String> {
    let mut seen: BTreeMap<i64, (f64, String)> = BTreeMap::new();
    for track in &midi.tracks {
        for event in &track.events {
            if let Kind::Tempo(us) = event.kind {
                if us > 0 {
                    let pos = exact_blick_position(
                        event.tick,
                        ticks_per_beat,
                        &format!("tempo event {}:{}", track.id, event.order),
                    )?;
                    let bpm = (60_000_000.0 / us as f64 * 1e6).round() / 1e6;
                    seen.insert(pos, (bpm, format!("event:{}:{}", track.id, event.order)));
                }
            }
        }
    }
    if seen.is_empty() {
        return Ok((
            vec![Tempo {
                bpm: 120.0,
                position: 0,
            }],
            BTreeSet::new(),
        ));
    }
    let evidence = seen.values().map(|(_, id)| id.clone()).collect();
    let tempo = seen
        .into_iter()
        .map(|(position, (bpm, _))| Tempo { bpm, position })
        .collect();
    Ok((tempo, evidence))
}

fn read_meter(midi: &Midi, ticks_per_beat: u16) -> Result<(Vec<Meter>, BTreeSet<String>), String> {
    if ticks_per_beat == 0 {
        return Err("MIDI PPQ division must be non-zero".into());
    }
    let mut changes = BTreeMap::new();
    for track in &midi.tracks {
        for event in &track.events {
            if let Kind::TimeSig { num, den, .. } = event.kind {
                changes.insert(
                    event.tick,
                    (
                        num,
                        den,
                        Some(format!("event:{}:{}", track.id, event.order)),
                    ),
                );
            }
        }
    }
    changes.entry(0).or_insert((4, 4, None));
    let mut out = Vec::with_capacity(changes.len());
    let mut evidence = BTreeSet::new();
    let mut previous_tick = 0u32;
    let mut previous_meter = (4u8, 4u16);
    let mut measure_index = 0u64;
    for (tick, (num, den, source_id)) in changes {
        if num == 0 || den == 0 {
            return Err(format!(
                "invalid time signature {num}/{den} at MIDI tick {tick}"
            ));
        }
        let delta = u64::from(tick.saturating_sub(previous_tick));
        let bar_numerator = u128::from(ticks_per_beat) * 4 * u128::from(previous_meter.0);
        let elapsed_numerator = u128::from(delta) * u128::from(previous_meter.1);
        if elapsed_numerator % bar_numerator != 0 {
            let source = source_id
                .as_deref()
                .map(|id| format!(" ({id})"))
                .unwrap_or_default();
            return Err(format!(
                "time signature change at MIDI tick {tick}{source} falls inside a \
                 {}/{} measure; Synthesizer V meter changes require a measure boundary",
                previous_meter.0, previous_meter.1
            ));
        }
        let elapsed_measures = u64::try_from(elapsed_numerator / bar_numerator)
            .map_err(|_| "MIDI meter position exceeds the supported range".to_string())?;
        measure_index = measure_index
            .checked_add(elapsed_measures)
            .ok_or_else(|| "MIDI meter position exceeds the supported range".to_string())?;
        let index = u32::try_from(measure_index)
            .map_err(|_| "MIDI meter position exceeds the supported range".to_string())?;
        out.push(Meter {
            denominator: u32::from(den),
            index,
            numerator: u32::from(num),
        });
        if let Some(source_id) = source_id {
            evidence.insert(source_id);
        }
        previous_tick = tick;
        previous_meter = (num, den);
    }
    Ok((out, evidence))
}

fn build_track(idx: usize, name: String, notes: Vec<Note>) -> SvpTrack {
    let uid = uuid(idx);
    SvpTrack {
        name,
        disp_color: COLORS[idx % COLORS.len()].to_string(),
        disp_order: idx as u32,
        render_enabled: true,
        mixer: Mixer {
            gain_decibel: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            display: true,
        },
        main_ref: MainRef {
            audio: Audio {
                filename: String::new(),
                duration: 0.0,
            },
            database: Database {
                name: String::new(),
                language: String::new(),
                phoneset: String::new(),
            },
            dictionary: String::new(),
            voice: serde_json::json!({}),
            group_id: uid.clone(),
            is_instrumental: false,
            blick_offset: 0,
        },
        main_group: MainGroup {
            name: "main".into(),
            uuid: uid,
            parameters: Parameters::default(),
            notes,
        },
        groups: vec![],
    }
}

fn lyric_text(lyric: &Lyric) -> String {
    match &lyric.state {
        midi::LyricState::Text(text) => text.clone(),
        midi::LyricState::Continuation => "-".into(),
        midi::LyricState::SyllableSplit => "+".into(),
        midi::LyricState::ExplicitEmpty | midi::LyricState::Unsupported(_) => String::new(),
    }
}

/// Picks the lyric a note sings. `lanes` is indexed by repeat pass: a single
/// entry pins one lane for the whole track, while several entries mean verse N
/// is sung on pass N, which is what a score with stacked verses under one
/// melody actually notates. A pass beyond the last verse reuses the last one.
/// Source note ids that the playback order reaches more than once, so a note
/// can tell whether another pass will sing it again.
fn replayed_note_ids(notes: &[SourceNote]) -> BTreeSet<&str> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for note in notes {
        *seen.entry(note.source.id.as_str()).or_default() += 1;
    }
    seen.into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(id, _)| id)
        .collect()
}

/// Every `(measure, lane)` pair on which some note carries an actual word. A
/// verse is written under the passage it belongs to, so a lane absent from a
/// whole measure has nothing to say there and the text written under that
/// measure is common to every pass — a refrain notated once. A lane that does
/// sing elsewhere in the same measure is a real alternative, and its silence on
/// one note is the verses dividing a word into different syllables.
fn lane_words_by_measure(notes: &[SourceNote]) -> BTreeSet<(u32, &str)> {
    notes
        .iter()
        .filter_map(|note| Some((note.source.measure?, note)))
        .flat_map(|(measure, note)| {
            note.lyrics
                .iter()
                .filter(|lyric| {
                    matches!(&lyric.state, midi::LyricState::Text(text) if !text.trim().is_empty())
                })
                .map(move |lyric| (measure, lyric.lane.as_str()))
        })
        .collect()
}

fn selected_attached_lyric<'a>(
    note: &'a SourceNote,
    lanes: &[String],
    replayed: &BTreeSet<&str>,
    lane_words: &BTreeSet<(u32, &str)>,
) -> Option<&'a Lyric> {
    let playback = note.source.occurrence + 1;
    let pick = |lane: &String| {
        note.lyrics
            .iter()
            .filter(|lyric| &lyric.lane == lane)
            .find(|lyric| !lyric.time_only.is_empty() && lyric.time_only.contains(&playback))
            .or_else(|| {
                note.lyrics
                    .iter()
                    .find(|lyric| &lyric.lane == lane && lyric.time_only.is_empty())
            })
    };
    let this_pass = usize::try_from(note.source.occurrence)
        .ok()
        .and_then(|pass| lanes.get(pass))
        .or_else(|| lanes.last())?;
    if let Some(lyric) = pick(this_pass) {
        return Some(lyric);
    }
    // This pass's verse says nothing here. Sing what the score does write only
    // when doing so cannot steal another pass's verse:
    //   - the playback order reaches this note once, so every stacked verse
    //     lands on that single instant and the other verses are the only text
    //     there is — verse markers and pickup syllables are commonly written on
    //     a later verse alone, exactly at such a spot;
    //   - or this pass's verse is absent from the whole measure, so the text
    //     written there is common to every pass. A refrain under a repeated
    //     passage is notated once and sung on every pass; dropping it would
    //     silence the passage instead of repeating it.
    // Otherwise the verse does sing in this measure and its silence on this
    // note is deliberate: the verses divide the word differently, and the
    // neighbouring verse belongs to its own pass, never to this one.
    let verse_is_silent_here = note
        .source
        .measure
        .is_some_and(|measure| !lane_words.contains(&(measure, this_pass.as_str())));
    if lanes.len() > 1 && (!replayed.contains(note.source.id.as_str()) || verse_is_silent_here) {
        return lanes.iter().find_map(pick);
    }
    None
}

struct TrackProjection<'a> {
    source_track_id: &'a str,
    lanes: &'a [String],
    standalone: &'a HashMap<usize, TimedLyric>,
    evidence: &'a mut ProjectionEvidence,
}

fn make_track(
    idx: usize,
    name: &str,
    notes: &[SourceNote],
    ticks_per_beat: u16,
    projection: TrackProjection<'_>,
) -> Result<SvpTrack, String> {
    let mut svp_notes = Vec::with_capacity(notes.len());
    let replayed = replayed_note_ids(notes);
    let lane_words = lane_words_by_measure(notes);
    let mut explicit_extension_end = None;
    let mut musicxml_extension_open = false;
    for (index, source_note) in notes.iter().enumerate() {
        let mut lyric_source_id = None;
        let mut lyric_event_id = None;
        let attached =
            selected_attached_lyric(source_note, projection.lanes, &replayed, &lane_words);
        let lyric = if let Some(attached) = attached {
            if let Some(ticks) = attached.extend_ticks.filter(|ticks| *ticks > 0) {
                explicit_extension_end = u32::try_from(ticks)
                    .ok()
                    .and_then(|ticks| source_note.onset.checked_add(ticks));
            } else {
                explicit_extension_end = None;
            }
            let text = lyric_text(attached);
            match attached.extension {
                Some(midi::LyricExtension::Start)
                | Some(midi::LyricExtension::Continue)
                | Some(midi::LyricExtension::Unspecified) => {
                    musicxml_extension_open = true;
                }
                Some(midi::LyricExtension::Stop) => {
                    musicxml_extension_open = false;
                }
                None if !matches!(attached.state, midi::LyricState::Continuation) => {
                    musicxml_extension_open = false;
                }
                None => {}
            }
            lyric_source_id = Some(attached_lyric_instance_id(
                attached,
                &source_note.source,
                source_note.source_order,
            ));
            text
        } else if let Some(standalone) = projection.standalone.get(&index) {
            lyric_source_id = Some(standalone_lyric_instance_id(
                &standalone.lyric,
                &standalone.track_id,
                standalone.order,
            ));
            lyric_event_id = Some(format!(
                "event:{}:{}",
                standalone.track_id, standalone.order
            ));
            lyric_text(&standalone.lyric)
        } else if musicxml_extension_open
            || explicit_extension_end.is_some_and(|end| source_note.onset < end)
        {
            // Continuation is emitted only from a source lyric extension.
            "-".into()
        } else {
            String::new()
        };
        let Some(pitch) = source_note.pitch else {
            continue;
        };
        if source_note.duration == 0 {
            continue;
        }
        if let Some(source_id) = lyric_source_id {
            projection.evidence.source_ids.insert(source_id);
        }
        if let Some(source_id) = lyric_event_id {
            projection.evidence.source_ids.insert(source_id);
        }
        projection.evidence.source_ids.insert(note_instance_id(
            projection.source_track_id,
            &source_note.source,
            source_note.source_order,
        ));
        projection.evidence.source_ids.insert(format!(
            "event:{}:{}",
            projection.source_track_id, source_note.source_order
        ));
        projection.evidence.source_ids.insert(format!(
            "event:{}:{}",
            projection.source_track_id, source_note.end_order
        ));
        let onset = exact_blick_position(
            source_note.onset,
            ticks_per_beat,
            &format!("note onset on source track {}", projection.source_track_id),
        )?;
        let duration = exact_blick_position(
            source_note.duration,
            ticks_per_beat,
            &format!(
                "note duration on source track {}",
                projection.source_track_id
            ),
        )?;
        svp_notes.push(Note {
            attributes: serde_json::json!({}),
            duration,
            lyrics: lyric,
            onset,
            phonemes: String::new(),
            pitch,
        });
    }
    Ok(build_track(idx, name.to_string(), svp_notes))
}

/// Detects the format (MIDI / MusicXML / MuseScore) and converts.
pub fn convert_auto(data: &[u8], language: &str) -> ConvertOutcome {
    convert_auto_with(data, language, None)
}

/// Like `convert_auto`, with explicit per-track vocal-export overrides.
pub fn convert_auto_with(
    data: &[u8],
    language: &str,
    overrides: Option<&HashMap<usize, bool>>,
) -> ConvertOutcome {
    use crate::engine::musescore as ms;
    use crate::engine::musicxml as mx;
    let fail = |m: String| ConvertOutcome {
        ok: false,
        msg: Some(m),
        svp: None,
        topology: SourceTopology::default(),
        tracks: vec![],
        n_tracks: 0,
        placed: 0,
        projection: ProjectionEvidence::default(),
    };
    if mx::looks_like_xml(data) {
        if ms::is_musescore_xml(data) {
            return match ms::parse(data) {
                Ok(midi) => convert_midi_with(&midi, language, overrides),
                Err(e) => fail(format!("unreadable MuseScore ({})", e)),
            };
        }
        return match mx::parse(data) {
            Ok(midi) => convert_midi_with(&midi, language, overrides),
            Err(e) => fail(format!("unreadable MusicXML ({})", e)),
        };
    }
    if mx::is_zip(data) {
        if mx::zip_has_musicxml(data) {
            return match mx::parse(data) {
                Ok(midi) => convert_midi_with(&midi, language, overrides),
                Err(e) => fail(format!("unreadable MusicXML ({})", e)),
            };
        }
        if ms::zip_has_mscx(data) {
            return match ms::parse(data) {
                Ok(midi) => convert_midi_with(&midi, language, overrides),
                Err(e) => fail(format!("unreadable MuseScore ({})", e)),
            };
        }
        return fail(
            "archive contains no recognized score (neither MusicXML nor MuseScore)".into(),
        );
    }
    let midi = match midi::parse(data) {
        Ok(m) => m,
        Err(e) => return fail(format!("unreadable file ({})", e)),
    };
    convert_midi_with(&midi, language, overrides)
}

pub fn convert_bytes(data: &[u8], language: &str) -> ConvertOutcome {
    let midi = match midi::parse(data) {
        Ok(m) => m,
        Err(e) => {
            return ConvertOutcome {
                ok: false,
                msg: Some(format!("unreadable file ({})", e)),
                svp: None,
                topology: SourceTopology::default(),
                tracks: vec![],
                n_tracks: 0,
                placed: 0,
                projection: ProjectionEvidence::default(),
            }
        }
    };
    convert_midi(&midi, language)
}

/// Conversion from an intermediate MIDI structure (shared by native MIDI and
/// MusicXML, which produces the same structure).
pub fn convert_midi(midi: &Midi, language: &str) -> ConvertOutcome {
    convert_midi_with(midi, language, None)
}

/// Like `convert_midi`, with user overrides: `overrides[track_id] = true`
/// requests an SVP vocal-note projection, while `false` leaves the track in
/// the full-score reference mix only. Source roles never change.
pub fn convert_midi_with(
    midi: &Midi,
    language: &str,
    overrides: Option<&HashMap<usize, bool>>,
) -> ConvertOutcome {
    let fail = |msg: String| ConvertOutcome {
        ok: false,
        msg: Some(msg),
        svp: None,
        // Parsing succeeded, so a projection refusal must not erase the
        // source Part/staff/voice evidence from diagnostics or manifests.
        topology: midi.topology.clone(),
        tracks: vec![],
        n_tracks: 0,
        placed: 0,
        projection: ProjectionEvidence::default(),
    };
    let tpb = match midi.time_base {
        TimeBase::PulsesPerQuarter(0) => {
            return fail("MIDI PPQ division must be non-zero".into());
        }
        TimeBase::PulsesPerQuarter(ppq) if midi.ticks_per_beat == 0 => {
            return fail(format!(
                "MIDI PPQ division must be non-zero (time base declares {ppq})"
            ));
        }
        TimeBase::PulsesPerQuarter(ppq) if midi.ticks_per_beat != ppq => {
            return fail(format!(
                "inconsistent MIDI PPQ values: time base declares {ppq}, \
                 ticks_per_beat declares {}",
                midi.ticks_per_beat
            ));
        }
        TimeBase::PulsesPerQuarter(ppq) => ppq,
        TimeBase::Smpte { .. } => {
            return fail(
                "SMPTE-timed MIDI is preserved but SVP projection is not supported yet".into(),
            );
        }
    };
    if midi.format == 2 {
        return fail(
            "MIDI format 2 contains independent sequences and cannot be flattened safely".into(),
        );
    }
    let (meter, meter_evidence) = match read_meter(midi, tpb) {
        Ok(meter) => meter,
        Err(error) => {
            return fail(format!("MIDI meter cannot be projected safely: {error}"));
        }
    };

    let mut svp_tracks: Vec<SvpTrack> = Vec::new();
    let mut report: Vec<TrackReport> = Vec::new();
    let mut total_placed = 0usize;
    let mut projection = ProjectionEvidence::default();
    let notes_by_track: Vec<_> = midi.tracks.iter().map(extract_notes).collect();
    let tokens_by_track: Vec<_> = midi.tracks.iter().map(track_tokens).collect();
    let external = resolve_external_lyrics(midi, &notes_by_track, &tokens_by_track, tpb);

    for (index, track) in midi.tracks.iter().enumerate() {
        let notes = &notes_by_track[index];
        let source_note_count = source_note_count(track);
        let own_tokens = &tokens_by_track[index];
        let source_binding = external.binding_for_source(index);
        let source_binding_active = source_binding.filter(|binding| {
            overrides.and_then(|map| map.get(&binding.target_track).copied()) != Some(false)
        });
        let source_projected = source_binding_active
            .map(|binding| binding.assignment.len())
            .unwrap_or_default();
        let name = if track.name.is_empty() {
            format!("Track {}", track.source.source_track)
        } else {
            track.name.clone()
        };
        if notes.is_empty() {
            let lyric_status = lyric_status(track, source_projected);
            let source_role = source_role(
                track,
                !own_tokens.is_empty(),
                source_note_count,
                &lyric_status,
            );
            let mut warnings = track_warnings(
                track,
                &lyric_status,
                source_role,
                source_note_count,
                0,
                !own_tokens.is_empty(),
                overrides.and_then(|map| map.get(&index).copied()),
            );
            append_external_warnings(
                &mut warnings,
                midi,
                &external,
                index,
                source_binding_active.is_some(),
            );
            report.push(TrackReport {
                id: index,
                source_id: track.id.clone(),
                track: name,
                notes: source_note_count,
                role: if own_tokens.is_empty() {
                    "metadata".into()
                } else {
                    "lyrics".into()
                },
                placed: 0,
                source_role,
                lyric_status,
                export_representation: ExportRepresentation::SourceOnly,
                requires_voice_assignment: false,
                warnings,
            });
            continue;
        }
        let lanes = ordered_lanes(&attached_lanes(notes));
        let attached = !lanes.is_empty();
        let mut unplaced_verses = 0usize;
        let target_binding = external.binding_for_target(index).filter(|binding| {
            overrides.and_then(|map| map.get(&binding.target_track).copied()) != Some(false)
        });
        let assignment = if let Some(binding) = target_binding {
            binding.assignment.clone()
        } else if source_binding_active.is_some() {
            // The source tokens have proven ownership on another track. Do
            // not also duplicate them onto this track's unrelated notes.
            HashMap::new()
        } else if track.text_profile == MidiTextProfile::KaraokeLyrics {
            karaoke_assignment(own_tokens, notes, tpb)
        } else {
            exact_assignment(own_tokens, notes)
        };

        let source_vocal = attached || !assignment.is_empty();
        let explicit_override = overrides.and_then(|map| map.get(&index).copied());
        let sing = explicit_override.unwrap_or(source_vocal);
        let mut placed = 0usize;
        if sing {
            let no_assignment = HashMap::new();
            // Stacked verses under one melody are alternatives, not simultaneous
            // voices: the score plays the music again and sings the next verse.
            // When the repeat provides a pass per verse, project one track that
            // follows that reading. Otherwise there is no place to put the extra
            // verses, so keep a track each and say so.
            let groups: Vec<Vec<String>> = if lanes.is_empty() {
                vec![Vec::new()]
            } else if lanes.len() > 1 && repeat_passes(notes) >= lanes.len() {
                vec![lanes.clone()]
            } else {
                lanes.iter().map(|lane| vec![lane.clone()]).collect()
            };
            if groups.len() > 1 {
                unplaced_verses = groups.len() - 1;
            }
            for group in &groups {
                let standalone = if group.is_empty() {
                    &assignment
                } else {
                    &no_assignment
                };
                placed += projected_lyric_count(notes, group, standalone);
                let lane_name = match group.as_slice() {
                    [lane] if lanes.len() > 1 => format!("{name} — lyric lane {lane}"),
                    _ => name.clone(),
                };
                let mut svp_track = match make_track(
                    svp_tracks.len(),
                    &lane_name,
                    notes,
                    tpb,
                    TrackProjection {
                        source_track_id: &track.id,
                        lanes: group,
                        standalone,
                        evidence: &mut projection,
                    },
                ) {
                    Ok(track) => track,
                    Err(error) => {
                        return fail(format!("source timing cannot be projected safely: {error}"));
                    }
                };
                if !svp_track.main_group.notes.is_empty() {
                    svp_track.main_ref.database.language = language.to_string();
                    svp_tracks.push(svp_track);
                }
            }
        }
        total_placed += placed;
        let projectable_notes = projectable_note_count(notes);
        let standalone_with_attached_lanes = attached && !own_tokens.is_empty();
        let status_projected = if source_binding_active.is_some() {
            source_projected
        } else if target_binding.is_some() {
            0
        } else {
            placed
        };
        let mut lyric_status = lyric_status(track, status_projected);
        if standalone_with_attached_lanes {
            lyric_status.state = LyricStatusState::Ambiguous;
        }
        let source_role = source_role(track, source_vocal, source_note_count, &lyric_status);
        let requires_voice_assignment = sing && projectable_notes > 0;
        let export_representation = if requires_voice_assignment {
            ExportRepresentation::VocalNotesAndReferenceMix
        } else {
            ExportRepresentation::ReferenceMixMember
        };
        let mut warnings = track_warnings(
            track,
            &lyric_status,
            source_role,
            source_note_count,
            projectable_notes,
            source_vocal,
            explicit_override,
        );
        append_external_warnings(
            &mut warnings,
            midi,
            &external,
            index,
            source_binding_active.is_some() || target_binding.is_some(),
        );
        if unplaced_verses > 0 {
            warnings.push(report_warning(
                "LYRIC_VERSES_EXCEED_REPEAT_PASSES",
                DiagnosticSeverity::Info,
                "The source stacks more verses under this melody than the repeat structure \
                 plays it back, so the extra verses keep a track of their own at the same \
                 instants instead of being sung one pass after the other.",
                &track.id,
            ));
        }
        if standalone_with_attached_lanes {
            warnings.push(report_warning(
                "STANDALONE_LYRICS_LEFT_SOURCE_ONLY",
                DiagnosticSeverity::Warning,
                "Standalone lyric events coexist with note-owned lyric lanes. They remain \
                 source-only because choosing a lane or duplicating vocal notes would guess.",
                &track.id,
            ));
        }
        if sing
            && placed == 0
            && matches!(
                lyric_status.state,
                LyricStatusState::SourceOwned | LyricStatusState::Unsupported
            )
        {
            lyric_status.state = LyricStatusState::Ambiguous;
            warnings.push(report_warning(
                "LYRIC_PROJECTION_AMBIGUOUS",
                DiagnosticSeverity::Warning,
                "Source lyrics were preserved but could not be assigned to vocal notes without guessing.",
                &track.id,
            ));
        }
        report.push(TrackReport {
            id: index,
            source_id: track.id.clone(),
            track: name,
            notes: source_note_count,
            role: if sing {
                "vocal".into()
            } else {
                "backing".into()
            },
            placed,
            source_role,
            lyric_status,
            export_representation,
            requires_voice_assignment,
            warnings,
        });
    }

    for (display_order, track) in svp_tracks.iter_mut().enumerate() {
        track.disp_order = display_order as u32;
        track.disp_color = COLORS[display_order % COLORS.len()].to_string();
    }

    let (tempo, tempo_evidence) = match read_tempo(midi, tpb) {
        Ok(tempo) => tempo,
        Err(error) => {
            return fail(format!("source timing cannot be projected safely: {error}"));
        }
    };
    projection.source_ids.extend(meter_evidence);
    projection.source_ids.extend(tempo_evidence);
    let svp = SvpProject {
        version: 113,
        time: Time { meter, tempo },
        render_config: RenderConfig::default(),
        tracks: svp_tracks,
    };
    let n_tracks = midi.topology.voice_count();
    ConvertOutcome {
        ok: true,
        msg: None,
        svp: Some(svp),
        topology: midi.topology.clone(),
        tracks: report,
        n_tracks,
        placed: total_placed,
        projection,
    }
}

fn exact_assignment(tokens: &[TimedLyric], notes: &[SourceNote]) -> HashMap<usize, TimedLyric> {
    let mut notes_by_tick: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut tokens_by_tick: HashMap<u32, Vec<&TimedLyric>> = HashMap::new();
    for (index, note) in notes.iter().enumerate() {
        notes_by_tick.entry(note.onset).or_default().push(index);
    }
    for token in tokens {
        tokens_by_tick.entry(token.tick).or_default().push(token);
    }
    let mut assignment = HashMap::new();
    for (tick, tick_tokens) in tokens_by_tick {
        if tick_tokens.len() != 1 {
            continue;
        }
        let Some(tick_notes) = notes_by_tick.get(&tick) else {
            continue;
        };
        if tick_notes.len() == 1 {
            assignment.insert(tick_notes[0], tick_tokens[0].clone());
        }
    }
    assignment
}

/// Soft Karaoke stores words and melody in separate SMF tracks and commonly
/// offsets lyric events slightly before note-on events. This qualified-profile
/// rule aligns monotonically within half a quarter note. It never creates a
/// token and never applies to generic MIDI Text.
fn karaoke_assignment(
    tokens: &[TimedLyric],
    notes: &[SourceNote],
    ticks_per_beat: u16,
) -> HashMap<usize, TimedLyric> {
    let tolerance = u32::from(ticks_per_beat).div_ceil(2);
    let distance = |left: u32, right: u32| (i64::from(left) - i64::from(right)).unsigned_abs();
    let mut assignment = HashMap::new();
    let mut note_index = 0usize;
    for token in tokens {
        while note_index < notes.len()
            && (notes[note_index].pitch.is_none() || notes[note_index].duration == 0)
        {
            note_index += 1;
        }
        if note_index >= notes.len() {
            break;
        }
        while note_index + 1 < notes.len() {
            let mut next = note_index + 1;
            while next < notes.len() && (notes[next].pitch.is_none() || notes[next].duration == 0) {
                next += 1;
            }
            if next >= notes.len()
                || distance(notes[next].onset, token.tick)
                    >= distance(notes[note_index].onset, token.tick)
            {
                break;
            }
            note_index = next;
        }
        if distance(notes[note_index].onset, token.tick) <= u64::from(tolerance) {
            assignment.insert(note_index, token.clone());
            note_index += 1;
        }
    }
    assignment
}

#[derive(Clone, Debug)]
struct ExternalLyricBinding {
    source_track: usize,
    target_track: usize,
    assignment: HashMap<usize, TimedLyric>,
    chord_ambiguities: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalLyricStatus {
    Bound {
        target_track: usize,
        chord_ambiguities: usize,
    },
    NoCompleteCandidate,
    AmbiguousCandidates {
        count: usize,
    },
    TargetClaimConflict,
}

#[derive(Default)]
struct ExternalLyricResolution {
    bindings: Vec<ExternalLyricBinding>,
    status_by_source: HashMap<usize, ExternalLyricStatus>,
}

impl ExternalLyricResolution {
    fn binding_for_source(&self, source_track: usize) -> Option<&ExternalLyricBinding> {
        self.bindings
            .iter()
            .find(|binding| binding.source_track == source_track)
    }

    fn binding_for_target(&self, target_track: usize) -> Option<&ExternalLyricBinding> {
        self.bindings
            .iter()
            .find(|binding| binding.target_track == target_track)
    }
}

/// Resolve a lyrics-only KAR stream against source melody notes. A binding is
/// accepted only when every real token maps monotonically and injectively,
/// exactly one non-percussion candidate satisfies that contract, and no other
/// lyric stream claims the same target. Track names and GM programs never
/// participate in the decision.
fn resolve_external_lyrics(
    midi: &Midi,
    notes_by_track: &[Vec<SourceNote>],
    tokens_by_track: &[Vec<TimedLyric>],
    ticks_per_beat: u16,
) -> ExternalLyricResolution {
    // Cross-track lyric ownership is a narrowly qualified KAR repair. Plain
    // MIDI, MusicXML, and MuseScore lyrics remain owned by their source lane;
    // rebinding them would invent score semantics that those formats did not
    // declare.
    if midi.source_format != midi::SourceFormat::KaraokeMidi {
        return ExternalLyricResolution::default();
    }

    let mut resolution = ExternalLyricResolution::default();
    let mut proposals = Vec::new();

    for (source_index, tokens) in tokens_by_track.iter().enumerate() {
        if tokens.is_empty() {
            continue;
        }
        let has_text = tokens
            .iter()
            .any(|token| token.origin == TimedLyricOrigin::KaraokeText);
        let has_midi_lyric = tokens
            .iter()
            .any(|token| token.origin == TimedLyricOrigin::MidiLyric);
        if has_text && has_midi_lyric {
            resolution.status_by_source.insert(
                source_index,
                ExternalLyricStatus::AmbiguousCandidates { count: 0 },
            );
            continue;
        }

        // Qualified KAR Text can be transferred only from a lyrics-only
        // source track. A genuine MIDI Lyric stream may coexist with notes,
        // but it stays on its own track when that track already gives a full
        // source-owned assignment.
        if has_text && projectable_note_count(&notes_by_track[source_index]) > 0 {
            continue;
        }
        if has_midi_lyric {
            let owned = karaoke_assignment(tokens, &notes_by_track[source_index], ticks_per_beat);
            if !tokens.is_empty() && owned.len() == tokens.len() {
                continue;
            }
        }

        let mut candidates = Vec::new();
        for (target_index, notes) in notes_by_track.iter().enumerate() {
            if target_index == source_index
                || projectable_note_count(notes) < tokens.len()
                || is_percussion_candidate(&midi.tracks[target_index])
                || !tokens_by_track[target_index].is_empty()
                || !attached_lanes(notes).is_empty()
            {
                continue;
            }
            let assignment = karaoke_assignment(tokens, notes, ticks_per_beat);
            if assignment.len() == tokens.len() {
                candidates.push((target_index, assignment));
            }
        }

        match candidates.len() {
            0 => {
                resolution
                    .status_by_source
                    .insert(source_index, ExternalLyricStatus::NoCompleteCandidate);
            }
            1 => {
                let (target_track, mut assignment) =
                    candidates.pop().expect("one candidate was measured");
                let chord_ambiguities =
                    remove_chord_ambiguities(&mut assignment, &notes_by_track[target_track]);
                proposals.push(ExternalLyricBinding {
                    source_track: source_index,
                    target_track,
                    assignment,
                    chord_ambiguities,
                });
            }
            count => {
                resolution.status_by_source.insert(
                    source_index,
                    ExternalLyricStatus::AmbiguousCandidates { count },
                );
            }
        }
    }

    let mut claims: HashMap<usize, usize> = HashMap::new();
    for proposal in &proposals {
        *claims.entry(proposal.target_track).or_default() += 1;
    }
    for proposal in proposals {
        if claims.get(&proposal.target_track).copied() != Some(1) {
            resolution.status_by_source.insert(
                proposal.source_track,
                ExternalLyricStatus::TargetClaimConflict,
            );
            continue;
        }
        resolution.status_by_source.insert(
            proposal.source_track,
            ExternalLyricStatus::Bound {
                target_track: proposal.target_track,
                chord_ambiguities: proposal.chord_ambiguities,
            },
        );
        resolution.bindings.push(proposal);
    }
    resolution
}

fn projectable_note_count(notes: &[SourceNote]) -> usize {
    notes
        .iter()
        .filter(|note| note.pitch.is_some() && note.duration > 0)
        .count()
}

fn is_percussion_candidate(track: &Track) -> bool {
    matches!(
        track.role_hint,
        TrackRoleHint::Percussion | TrackRoleHint::Mixed
    ) || track
        .instruments
        .iter()
        .any(|instrument| instrument.percussion || instrument.channel == Some(9))
        || track.events.iter().any(|event| {
            matches!(
                &event.kind,
                Kind::NoteOn(note)
                    if note.velocity != Some(0)
                        && (note.channel == Some(9) || note.source.unpitched.is_some())
            )
        })
}

fn remove_chord_ambiguities(
    assignment: &mut HashMap<usize, TimedLyric>,
    notes: &[SourceNote],
) -> usize {
    let mut notes_per_onset: HashMap<u32, usize> = HashMap::new();
    for note in notes
        .iter()
        .filter(|note| note.pitch.is_some() && note.duration > 0)
    {
        *notes_per_onset.entry(note.onset).or_default() += 1;
    }
    let ambiguous: Vec<usize> = assignment
        .keys()
        .copied()
        .filter(|index| {
            notes_per_onset
                .get(&notes[*index].onset)
                .copied()
                .unwrap_or_default()
                > 1
        })
        .collect();
    for index in &ambiguous {
        assignment.remove(index);
    }
    ambiguous.len()
}

fn projected_lyric_count(
    notes: &[SourceNote],
    lanes: &[String],
    assignment: &HashMap<usize, TimedLyric>,
) -> usize {
    let replayed = replayed_note_ids(notes);
    let lane_words = lane_words_by_measure(notes);
    notes
        .iter()
        .enumerate()
        .filter(|(index, note)| {
            if note.pitch.is_none() || note.duration == 0 {
                return false;
            }
            selected_attached_lyric(note, lanes, &replayed, &lane_words)
                .or_else(|| assignment.get(index).map(|timed| &timed.lyric))
                .is_some_and(|lyric| {
                    matches!(
                        lyric.state,
                        midi::LyricState::Text(_)
                            | midi::LyricState::Continuation
                            | midi::LyricState::SyllableSplit
                    )
                })
        })
        .count()
}

fn source_note_count(track: &Track) -> usize {
    track
        .events
        .iter()
        .filter(|event| !matches!(&event.kind, Kind::NoteOn(note) if note.velocity == Some(0)))
        .filter(|event| matches!(event.kind, Kind::NoteOn(_)))
        .count()
}

fn lyric_status(track: &Track, projected_text_count: usize) -> LyricStatus {
    let mut source_text_count = 0usize;
    let mut explicit_empty_count = 0usize;
    let mut continuation_count = 0usize;
    let mut unsupported_count = 0usize;
    let mut generic_text_count = 0usize;

    let mut count = |lyric: &Lyric| match lyric.state {
        midi::LyricState::Text(_) => source_text_count += 1,
        midi::LyricState::Continuation | midi::LyricState::SyllableSplit => continuation_count += 1,
        midi::LyricState::ExplicitEmpty => explicit_empty_count += 1,
        midi::LyricState::Unsupported(_) => unsupported_count += 1,
    };

    for event in &track.events {
        match &event.kind {
            Kind::NoteOn(note) if note.velocity != Some(0) => {
                for lyric in &note.lyrics {
                    count(lyric);
                }
            }
            Kind::Lyrics(lyric) if !midi::is_midi_lyric_line_break(&lyric.raw) => count(lyric),
            Kind::Text(_) if track.text_profile == MidiTextProfile::Generic => {
                generic_text_count += 1
            }
            Kind::Text(_) if track.text_profile == MidiTextProfile::KaraokeLyrics => {
                if let Some(lyric) = match &event.kind {
                    Kind::Text(text) => {
                        karaoke_text_lyric(&track.id, event.tick, event.order, text)
                    }
                    _ => None,
                } {
                    count(&lyric);
                }
            }
            _ => {}
        }
    }

    let state = if source_text_count > 0 || continuation_count > 0 {
        LyricStatusState::SourceOwned
    } else if unsupported_count > 0 {
        LyricStatusState::Unsupported
    } else if explicit_empty_count > 0 {
        LyricStatusState::ExplicitEmpty
    } else if generic_text_count > 0 {
        LyricStatusState::MetadataOnly
    } else {
        LyricStatusState::None
    };

    LyricStatus {
        state,
        source_text_count,
        projected_text_count,
        explicit_empty_count,
        continuation_count,
        unsupported_count,
    }
}

fn source_role(
    track: &Track,
    has_source_vocal_evidence: bool,
    source_notes: usize,
    lyric_status: &LyricStatus,
) -> SourceRole {
    if source_notes == 0 {
        return if matches!(
            lyric_status.state,
            LyricStatusState::SourceOwned
                | LyricStatusState::ExplicitEmpty
                | LyricStatusState::Unsupported
        ) {
            SourceRole::LyricsOnly
        } else {
            SourceRole::Metadata
        };
    }
    match track.role_hint {
        TrackRoleHint::Vocal => SourceRole::Vocal,
        TrackRoleHint::Instrumental => SourceRole::Instrumental,
        TrackRoleHint::Percussion => SourceRole::Percussion,
        TrackRoleHint::Mixed => SourceRole::Mixed,
        TrackRoleHint::Ambiguous if has_source_vocal_evidence => SourceRole::Vocal,
        TrackRoleHint::Ambiguous => SourceRole::Ambiguous,
    }
}

fn report_warning(
    code: &str,
    severity: DiagnosticSeverity,
    message: impl Into<String>,
    source_id: &str,
) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity,
        message: message.into(),
        source_id: Some(source_id.into()),
    }
}

fn track_warnings(
    track: &Track,
    status: &LyricStatus,
    source_role: SourceRole,
    source_notes: usize,
    projectable_notes: usize,
    source_vocal: bool,
    explicit_override: Option<bool>,
) -> Vec<Diagnostic> {
    let mut warnings = Vec::new();
    if status.state == LyricStatusState::MetadataOnly {
        warnings.push(report_warning(
            "GENERIC_MIDI_TEXT_NOT_LYRICS",
            DiagnosticSeverity::Info,
            "Generic MIDI Text was preserved as metadata and was not converted into lyrics.",
            &track.id,
        ));
    }
    if status.unsupported_count > 0 {
        warnings.push(report_warning(
            "UNSUPPORTED_LYRIC_CONTENT",
            DiagnosticSeverity::Warning,
            format!(
                "{} source lyric item(s) cannot be represented as Synthesizer V text.",
                status.unsupported_count
            ),
            &track.id,
        ));
    }
    if source_notes > projectable_notes {
        warnings.push(report_warning(
            "SOURCE_NOTES_NOT_IN_VOCAL_SVP",
            DiagnosticSeverity::Info,
            format!(
                "{} source note(s) have no source-owned pitched/duration representation for a vocal SVP track; they remain in the source and full-score mix.",
                source_notes - projectable_notes
            ),
            &track.id,
        ));
    }
    if explicit_override == Some(true) && !source_vocal {
        warnings.push(report_warning(
            "USER_VOCAL_OVERRIDE",
            DiagnosticSeverity::Info,
            "This track is exported as vocal notes only because of the explicit user override; no lyrics were invented.",
            &track.id,
        ));
    }
    if source_role == SourceRole::Ambiguous {
        warnings.push(report_warning(
            "AMBIGUOUS_SOURCE_ROLE",
            DiagnosticSeverity::Info,
            "The source does not identify this musical track as vocal or instrumental.",
            &track.id,
        ));
    }
    warnings
}

fn append_external_warnings(
    warnings: &mut Vec<Diagnostic>,
    midi: &Midi,
    resolution: &ExternalLyricResolution,
    track_index: usize,
    binding_active: bool,
) {
    let track = &midi.tracks[track_index];
    let control_count = track
        .events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                Kind::Lyrics(lyric) if midi::is_midi_lyric_line_break(&lyric.raw)
            ) || matches!(
                &event.kind,
                Kind::Text(text)
                    if track.text_profile != MidiTextProfile::Generic
                        && midi::is_soft_karaoke_text_control(&text.text)
            )
        })
        .count();
    if control_count > 0 {
        warnings.push(report_warning(
            "KARAOKE_CONTROLS_PRESERVED_AS_METADATA",
            DiagnosticSeverity::Info,
            format!(
                "{control_count} karaoke control record(s) were preserved as metadata and not sung."
            ),
            &track.id,
        ));
    }

    if let Some(status) = resolution.status_by_source.get(&track_index) {
        match status {
            ExternalLyricStatus::Bound {
                target_track,
                chord_ambiguities,
            } if binding_active => {
                warnings.push(report_warning(
                    "EXTERNAL_KARAOKE_LYRICS_BOUND",
                    DiagnosticSeverity::Info,
                    format!(
                        "The complete source lyric stream was bound to source track {} by timing evidence.",
                        midi.tracks[*target_track].id
                    ),
                    &track.id,
                ));
                if *chord_ambiguities > 0 {
                    warnings.push(report_warning(
                        "KARAOKE_CHORD_PITCH_AMBIGUOUS",
                        DiagnosticSeverity::Warning,
                        format!(
                            "{chord_ambiguities} lyric item(s) coincide with multi-pitch chord onsets and remain source-only."
                        ),
                        &track.id,
                    ));
                }
            }
            ExternalLyricStatus::Bound { .. } => {}
            ExternalLyricStatus::NoCompleteCandidate => warnings.push(report_warning(
                "EXTERNAL_KARAOKE_LYRICS_UNRESOLVED",
                DiagnosticSeverity::Warning,
                "No source melody track matched every lyric token; the stream remains source-only.",
                &track.id,
            )),
            ExternalLyricStatus::AmbiguousCandidates { count } => warnings.push(report_warning(
                "EXTERNAL_KARAOKE_LYRICS_AMBIGUOUS",
                DiagnosticSeverity::Warning,
                format!(
                    "{count} complete melody candidate(s) were found without unique ownership; the stream remains source-only."
                ),
                &track.id,
            )),
            ExternalLyricStatus::TargetClaimConflict => warnings.push(report_warning(
                "EXTERNAL_KARAOKE_TARGET_CONFLICT",
                DiagnosticSeverity::Warning,
                "Multiple lyric streams claim the same melody track; all remain source-only.",
                &track.id,
            )),
        }
    }
    if binding_active {
        if let Some(binding) = resolution.binding_for_target(track_index) {
            warnings.push(report_warning(
                "EXTERNAL_KARAOKE_MELODY_TARGET",
                DiagnosticSeverity::Info,
                format!(
                    "Vocal lyrics are source-owned by track {} and were bound here by complete monotonic timing.",
                    midi.tracks[binding.source_track].id
                ),
                &track.id,
            ));
        }
    }
}

fn attached_lanes(notes: &[SourceNote]) -> BTreeSet<String> {
    notes
        .iter()
        .flat_map(|note| note.lyrics.iter().map(|lyric| lyric.lane.clone()))
        .collect()
}

/// Verse order, not label order: lanes are numbered in both source formats, and
/// sorting them as text would place verse 10 between verse 1 and verse 2.
/// Non-numeric labels keep a stable place after the numbered ones.
fn ordered_lanes(lanes: &BTreeSet<String>) -> Vec<String> {
    let mut ordered: Vec<String> = lanes.iter().cloned().collect();
    ordered.sort_by(|a, b| {
        let key = |lane: &String| (lane.parse::<u32>().ok().is_none(), lane.parse::<u32>().ok());
        key(a).cmp(&key(b)).then_with(|| a.cmp(b))
    });
    ordered
}

/// How many times the source is played back, counting repeat unrolling. A score
/// with two verses needs two passes for both to be singable in place.
fn repeat_passes(notes: &[SourceNote]) -> usize {
    notes
        .iter()
        .map(|note| note.source.occurrence as usize + 1)
        .max()
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn midi_with(tracks: Vec<Track>) -> Midi {
        Midi {
            ticks_per_beat: 480,
            time_base: TimeBase::PulsesPerQuarter(480),
            format: 1,
            source_format: midi::SourceFormat::StandardMidi,
            topology: midi::SourceTopology::from_tracks(&tracks),
            tracks,
        }
    }

    fn kar_with(tracks: Vec<Track>) -> Midi {
        let mut midi = midi_with(tracks);
        midi.source_format = midi::SourceFormat::KaraokeMidi;
        midi
    }

    fn note_events(
        track_id: &str,
        onset: u32,
        duration: u32,
        lyrics: Vec<Lyric>,
    ) -> Vec<midi::Event> {
        let source_id = format!("{track_id}-note");
        vec![
            midi::Event::new(
                onset,
                0,
                Kind::NoteOn(NoteOn {
                    channel: Some(0),
                    key: Some(60),
                    velocity: Some(90),
                    source: midi::NoteSource {
                        id: source_id.clone(),
                        ..midi::NoteSource::default()
                    },
                    lyrics,
                }),
            ),
            midi::Event::new(
                onset + duration,
                1,
                Kind::NoteOff(midi::NoteOff {
                    channel: Some(0),
                    key: Some(60),
                    velocity: Some(0),
                    source_id: Some(source_id),
                }),
            ),
        ]
    }

    fn pitched_track(id: &str, notes: &[(u32, u32, u8, u8)]) -> Track {
        let mut track = Track::new(id, 0);
        track.name = id.into();
        for (index, (onset, duration, pitch, channel)) in notes.iter().copied().enumerate() {
            let source_id = format!("{id}-note-{index}");
            let order = u32::try_from(index * 2).expect("test order fits");
            track.events.push(midi::Event::new(
                onset,
                order,
                Kind::NoteOn(NoteOn {
                    channel: Some(channel),
                    key: Some(pitch),
                    velocity: Some(90),
                    source: midi::NoteSource {
                        id: source_id.clone(),
                        ..midi::NoteSource::default()
                    },
                    lyrics: Vec::new(),
                }),
            ));
            track.events.push(midi::Event::new(
                onset + duration,
                order + 1,
                Kind::NoteOff(midi::NoteOff {
                    channel: Some(channel),
                    key: Some(pitch),
                    velocity: Some(0),
                    source_id: Some(source_id),
                }),
            ));
        }
        track
    }

    fn karaoke_text_track(id: &str, values: &[(u32, &str)]) -> Track {
        let mut track = Track::new(id, 0);
        track.name = id.into();
        track.text_profile = MidiTextProfile::KaraokeLyrics;
        for (order, (tick, value)) in values.iter().enumerate() {
            track.events.push(midi::Event::new(
                *tick,
                u32::try_from(order).expect("test order fits"),
                Kind::Text(midi::TextEvent {
                    text: (*value).into(),
                    raw: value.as_bytes().to_vec(),
                }),
            ));
        }
        track
    }

    #[test]
    fn genuine_la_is_preserved_but_absence_stays_empty() {
        let lyric = Lyric::text("source", "la".into());
        assert_eq!(lyric_text(&lyric), "la");
        assert_eq!(lyric_text(&Lyric::text("empty", String::new())), "");
    }

    #[test]
    fn public_conversion_rejects_zero_or_inconsistent_ppq() {
        let mut zero_time_base = midi_with(Vec::new());
        zero_time_base.time_base = TimeBase::PulsesPerQuarter(0);
        let outcome = convert_midi(&zero_time_base, "english");
        assert!(!outcome.ok);
        assert!(outcome
            .msg
            .as_deref()
            .is_some_and(|message| message.contains("non-zero")));

        let mut zero_compatibility_field = midi_with(Vec::new());
        zero_compatibility_field.ticks_per_beat = 0;
        let outcome = convert_midi(&zero_compatibility_field, "english");
        assert!(!outcome.ok);
        assert!(outcome
            .msg
            .as_deref()
            .is_some_and(|message| message.contains("non-zero")));

        let mut inconsistent = midi_with(Vec::new());
        inconsistent.ticks_per_beat = 960;
        let outcome = convert_midi(&inconsistent, "english");
        assert!(!outcome.ok);
        assert!(outcome
            .msg
            .as_deref()
            .is_some_and(|message| message.contains("inconsistent")));
    }

    #[test]
    fn meter_index_comes_from_elapsed_bars_not_change_ordinal() {
        let mut track = Track::new("meter", 0);
        track.events = vec![
            midi::Event::new(
                0,
                0,
                Kind::TimeSig {
                    num: 4,
                    den: 4,
                    clocks_per_click: None,
                    notated_32nds: None,
                },
            ),
            midi::Event::new(
                3_840,
                1,
                Kind::TimeSig {
                    num: 3,
                    den: 4,
                    clocks_per_click: None,
                    notated_32nds: None,
                },
            ),
            midi::Event::new(
                5_280,
                2,
                Kind::TimeSig {
                    num: 5,
                    den: 8,
                    clocks_per_click: None,
                    notated_32nds: None,
                },
            ),
        ];
        let (meter, evidence) =
            read_meter(&midi_with(vec![track]), 480).expect("meter changes are bar-aligned");
        assert_eq!(
            meter.iter().map(|meter| meter.index).collect::<Vec<_>>(),
            vec![0, 2, 3]
        );
        assert_eq!(evidence.len(), 3);
    }

    #[test]
    fn mid_measure_meter_change_fails_instead_of_claiming_an_exact_projection() {
        let mut track = Track::new("meter", 0);
        track.events = vec![
            midi::Event::new(
                0,
                0,
                Kind::TimeSig {
                    num: 4,
                    den: 4,
                    clocks_per_click: None,
                    notated_32nds: None,
                },
            ),
            midi::Event::new(
                480,
                1,
                Kind::TimeSig {
                    num: 3,
                    den: 4,
                    clocks_per_click: None,
                    notated_32nds: None,
                },
            ),
        ];
        let midi = midi_with(vec![track]);
        let error = match read_meter(&midi, 480) {
            Err(error) => error,
            Ok(_) => panic!("tick 480 is inside the 4/4 measure"),
        };
        assert!(error.contains("falls inside"), "unexpected error: {error}");

        let outcome = convert_midi(&midi, "english");
        assert!(!outcome.ok);
        assert!(outcome.svp.is_none());
        assert!(outcome
            .msg
            .as_deref()
            .is_some_and(|message| message.contains("cannot be projected safely")));
        assert_eq!(outcome.topology, midi.topology);
    }

    #[test]
    fn tempo_and_meter_evidence_names_only_the_deduplicated_winner() {
        let mut first = Track::new("first", 0);
        first.events = vec![
            midi::Event::new(0, 0, Kind::Tempo(500_000)),
            midi::Event::new(
                0,
                1,
                Kind::TimeSig {
                    num: 4,
                    den: 4,
                    clocks_per_click: Some(24),
                    notated_32nds: Some(8),
                },
            ),
        ];
        let mut second = Track::new("second", 1);
        second.events = vec![
            midi::Event::new(0, 0, Kind::Tempo(666_667)),
            midi::Event::new(
                0,
                1,
                Kind::TimeSig {
                    num: 3,
                    den: 4,
                    clocks_per_click: None,
                    notated_32nds: None,
                },
            ),
        ];
        let midi = midi_with(vec![first, second]);
        let (tempo, tempo_evidence) =
            read_tempo(&midi, 480).expect("tick zero is exactly representable");
        let (meter, meter_evidence) =
            read_meter(&midi, 480).expect("same-tick meter changes are bar-aligned");
        assert_eq!(tempo.len(), 1);
        assert_eq!(tempo[0].bpm, 89.999955);
        assert_eq!(
            tempo_evidence,
            BTreeSet::from(["event:second:0".to_string()])
        );
        assert_eq!(meter[0].numerator, 3);
        assert_eq!(
            meter_evidence,
            BTreeSet::from(["event:second:1".to_string()])
        );
    }

    #[test]
    fn inexact_svp_blick_positions_fail_instead_of_rounding() {
        let mut track = Track::new("voice", 0);
        track.events = note_events("voice", 1, 1, vec![Lyric::text("word", "word".into())]);
        let mut midi = midi_with(vec![track]);
        midi.ticks_per_beat = 1024;
        midi.time_base = TimeBase::PulsesPerQuarter(1024);

        let outcome = convert_midi(&midi, "english");
        assert!(!outcome.ok);
        assert!(outcome.svp.is_none());
        assert!(outcome.msg.as_deref().is_some_and(|message| {
            message.contains("cannot be represented exactly in Synthesizer V blicks")
        }));
        assert_eq!(outcome.topology, midi.topology);
    }

    #[test]
    fn every_attached_lyric_lane_gets_its_own_svp_track() {
        let mut first = Lyric::text("lane-1", "one".into());
        first.lane = "1".into();
        let mut second = Lyric::text("lane-2", "two".into());
        second.lane = "2".into();
        second.verse = 2;
        let mut track = Track::new("voice", 0);
        track.name = "Voice".into();
        track.events = note_events("voice", 0, 480, vec![first, second]);

        let outcome = convert_midi(&midi_with(vec![track]), "english");
        let project = outcome.svp.expect("conversion succeeds");
        assert_eq!(project.tracks.len(), 2);
        assert_eq!(project.tracks[0].main_group.notes[0].lyrics, "one");
        assert_eq!(project.tracks[1].main_group.notes[0].lyrics, "two");
        assert_eq!(outcome.placed, 2);
        assert!(outcome
            .projection
            .source_ids
            .contains("lyric:lane-1:occurrence:0:note-event:0"));
        assert!(outcome
            .projection
            .source_ids
            .contains("lyric:lane-2:occurrence:0:note-event:0"));
    }

    /// Two occurrences of the same note, as repeat unrolling produces them:
    /// one source id, two `occurrence` values. Both carry every verse, so the
    /// projector has to pick one per pass.
    fn repeated_note_events(track_id: &str, lyrics: Vec<Lyric>) -> Vec<midi::Event> {
        let mut events = Vec::new();
        let source_id = format!("{track_id}-note");
        for (pass, onset) in [0u32, 480].into_iter().enumerate() {
            let source = midi::NoteSource {
                id: source_id.clone(),
                occurrence: pass as u32,
                ..midi::NoteSource::default()
            };
            events.push(midi::Event::new(
                onset,
                events.len() as u32,
                Kind::NoteOn(NoteOn {
                    channel: Some(0),
                    key: Some(60),
                    velocity: Some(90),
                    source,
                    lyrics: lyrics.clone(),
                }),
            ));
            events.push(midi::Event::new(
                onset + 480,
                events.len() as u32,
                Kind::NoteOff(midi::NoteOff {
                    channel: Some(0),
                    key: Some(60),
                    velocity: Some(0),
                    source_id: Some(source_id.clone()),
                }),
            ));
        }
        events
    }

    fn verse(id: &str, text: &str, lane: &str) -> Lyric {
        let mut lyric = Lyric::text(id, text.into());
        lyric.lane = lane.into();
        lyric
    }

    #[test]
    fn stacked_verses_are_sung_one_repeat_pass_after_the_other() {
        // A score that stacks two verses under one melody means "play it again
        // and sing the next verse", not "sing both at once".
        let mut track = Track::new("voice", 0);
        track.name = "Voice".into();
        track.events = repeated_note_events(
            "voice",
            vec![verse("lane-1", "one", "1"), verse("lane-2", "two", "2")],
        );

        let outcome = convert_midi(&midi_with(vec![track]), "english");
        let project = outcome.svp.expect("conversion succeeds");
        assert_eq!(project.tracks.len(), 1);
        let sung: Vec<_> = project.tracks[0]
            .main_group
            .notes
            .iter()
            .map(|note| note.lyrics.as_str())
            .collect();
        assert_eq!(sung, vec!["one", "two"]);
    }

    #[test]
    fn a_word_only_a_later_verse_carries_is_still_sung_on_a_note_played_once() {
        // Verse markers and pickup syllables are commonly written on one verse
        // alone, at a spot outside the repeat. That note is played once, so
        // every verse is stacked on it and the later verse is the only text
        // there is: dropping it would lose a source word.
        let mut track = Track::new("voice", 0);
        track.name = "Voice".into();
        let mut events = repeated_note_events(
            "voice",
            vec![verse("lane-1", "one", "1"), verse("lane-2", "two", "2")],
        );
        let played_once = midi::NoteSource {
            id: "voice-tail".into(),
            ..midi::NoteSource::default()
        };
        events.push(midi::Event::new(
            960,
            events.len() as u32,
            Kind::NoteOn(NoteOn {
                channel: Some(0),
                key: Some(60),
                velocity: Some(90),
                source: played_once,
                lyrics: vec![verse("lane-2-only", "3.", "2")],
            }),
        ));
        events.push(midi::Event::new(
            1440,
            events.len() as u32,
            Kind::NoteOff(midi::NoteOff {
                channel: Some(0),
                key: Some(60),
                velocity: Some(0),
                source_id: Some("voice-tail".into()),
            }),
        ));
        track.events = events;

        let outcome = convert_midi(&midi_with(vec![track]), "english");
        let project = outcome.svp.expect("conversion succeeds");
        assert_eq!(project.tracks.len(), 1);
        let sung: Vec<_> = project.tracks[0]
            .main_group
            .notes
            .iter()
            .map(|note| note.lyrics.as_str())
            .collect();
        assert_eq!(sung, vec!["one", "two", "3."]);
    }

    #[test]
    fn a_replayed_note_never_borrows_another_verses_word() {
        // The first note is reached twice, so verse 2 owns its second pass.
        // MIDI has no measures, so nothing here can prove verse 2 is absent
        // from the whole passage rather than deliberately silent on this note;
        // borrowing verse 1's syllable would risk singing a word from the wrong
        // verse. The last note is played once, so every verse lands on it.
        let mut track = Track::new("voice", 0);
        track.name = "Voice".into();
        let mut events = repeated_note_events("voice", vec![verse("lane-1", "one", "1")]);
        let played_once = midi::NoteSource {
            id: "voice-tail".into(),
            ..midi::NoteSource::default()
        };
        events.push(midi::Event::new(
            960,
            events.len() as u32,
            Kind::NoteOn(NoteOn {
                channel: Some(0),
                key: Some(60),
                velocity: Some(90),
                source: played_once,
                lyrics: vec![verse("lane-2", "two", "2")],
            }),
        ));
        events.push(midi::Event::new(
            1440,
            events.len() as u32,
            Kind::NoteOff(midi::NoteOff {
                channel: Some(0),
                key: Some(60),
                velocity: Some(0),
                source_id: Some("voice-tail".into()),
            }),
        ));
        track.events = events;

        let outcome = convert_midi(&midi_with(vec![track]), "english");
        let project = outcome.svp.expect("conversion succeeds");
        let sung: Vec<_> = project.tracks[0]
            .main_group
            .notes
            .iter()
            .map(|note| note.lyrics.as_str())
            .collect();
        assert_eq!(sung, vec!["one", "", "two"]);
    }

    #[test]
    fn verses_with_nowhere_to_go_keep_a_track_each_and_say_so() {
        // Without a repeat there is no second pass to put verse 2 on, so the
        // old shape is kept rather than silently dropping a verse.
        let mut track = Track::new("voice", 0);
        track.name = "Voice".into();
        track.events = note_events(
            "voice",
            0,
            480,
            vec![verse("lane-1", "one", "1"), verse("lane-2", "two", "2")],
        );

        let outcome = convert_midi(&midi_with(vec![track]), "english");
        let project = outcome.svp.expect("conversion succeeds");
        assert_eq!(project.tracks.len(), 2);
        assert!(outcome.tracks.iter().any(|report| report
            .warnings
            .iter()
            .any(|warning| warning.code == "LYRIC_VERSES_EXCEED_REPEAT_PASSES")));
    }

    #[test]
    fn verse_order_follows_the_number_not_the_label() {
        // Sorting lane labels as text would sing verse 10 on the second pass.
        let lanes: BTreeSet<String> = ["1", "2", "10"].iter().map(|s| s.to_string()).collect();
        assert_eq!(ordered_lanes(&lanes), vec!["1", "2", "10"]);
    }

    fn sung_lyrics(xml: &str) -> Vec<String> {
        let outcome = convert_auto(xml.as_bytes(), "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        outcome
            .svp
            .expect("valid SVP")
            .tracks
            .first()
            .expect("one vocal track")
            .main_group
            .notes
            .iter()
            .map(|note| note.lyrics.clone())
            .collect()
    }

    #[test]
    fn a_refrain_written_once_under_a_repeat_is_sung_on_every_pass() {
        // Verses stack only under the passage whose words differ. The refrain
        // that follows carries one line of text for both passes, and the score
        // repeats it verbatim. Reading the second pass as "verse 2 is silent
        // here" would delete the refrain from half the piece.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure><startRepeat/><voice>
        <Chord><durationType>whole</durationType>
          <Lyrics><text>one</text></Lyrics>
          <Lyrics><no>1</no><text>two</text></Lyrics>
          <Note><pitch>60</pitch></Note>
        </Chord>
      </voice></Measure>
      <Measure><voice>
        <Chord><durationType>whole</durationType>
          <Lyrics><text>bam</text></Lyrics>
          <Note><pitch>62</pitch></Note>
        </Chord>
      </voice><endRepeat>2</endRepeat></Measure>
    </Staff>
  </Score>
</museScore>"#;
        assert_eq!(sung_lyrics(xml), vec!["one", "bam", "two", "bam"]);
    }

    #[test]
    fn a_verse_that_sings_elsewhere_in_the_measure_stays_silent_where_it_says_nothing() {
        // Both verses sing in this measure but divide the words differently:
        // verse 1 spends two notes on "o-pened" where verse 2 spends one on
        // "ne" and then holds. Verse 2's silence on the second note is what the
        // score writes, and borrowing verse 1's syllable would sing a word from
        // the wrong verse.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure><startRepeat/><voice>
        <Chord><durationType>half</durationType>
          <Lyrics><text>o</text></Lyrics>
          <Lyrics><no>1</no><text>ne</text></Lyrics>
          <Note><pitch>60</pitch></Note>
        </Chord>
        <Chord><durationType>half</durationType>
          <Lyrics><text>pened</text></Lyrics>
          <Note><pitch>62</pitch></Note>
        </Chord>
      </voice><endRepeat>2</endRepeat></Measure>
    </Staff>
  </Score>
</museScore>"#;
        assert_eq!(sung_lyrics(xml), vec!["o", "pened", "ne", ""]);
    }

    #[test]
    fn standalone_lyrics_remain_source_only_when_attached_lanes_exist() {
        let mut attached = Lyric::text("attached", "owned".into());
        attached.lane = "1".into();
        let standalone = Lyric::text("standalone", "event".into());
        let mut track = Track::new("mixed-lyrics", 0);
        track.events = note_events("mixed-lyrics", 0, 480, vec![attached]);
        track.events[1].order = 2;
        track
            .events
            .insert(1, midi::Event::new(0, 1, Kind::Lyrics(standalone.clone())));

        let outcome = convert_midi(&midi_with(vec![track]), "english");
        let project = outcome.svp.as_ref().expect("conversion succeeds");
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes[0].lyrics, "owned");
        assert_eq!(outcome.placed, 1);
        assert_eq!(
            outcome.tracks[0].lyric_status.state,
            LyricStatusState::Ambiguous
        );
        assert!(outcome.tracks[0]
            .warnings
            .iter()
            .any(|warning| warning.code == "STANDALONE_LYRICS_LEFT_SOURCE_ONLY"));
        assert!(!outcome
            .projection
            .source_ids
            .contains(&standalone_lyric_instance_id(
                &standalone,
                "mixed-lyrics",
                1
            )));
        assert!(!outcome
            .projection
            .source_ids
            .contains("event:mixed-lyrics:1"));
    }

    #[test]
    fn unique_complete_external_lyric_stream_binds_without_name_evidence() {
        let mut lyrics = Track::new("lyrics", 0);
        lyrics.events.push(midi::Event::new(
            0,
            0,
            Kind::Lyrics(Lyric::text("word", "let".into())),
        ));
        let mut melody = Track::new("melody", 1);
        melody.name = "Soprano Vocal Melody".into();
        melody.events = note_events("melody", 0, 480, Vec::new());

        let midi = kar_with(vec![lyrics, melody]);
        let automatic = convert_midi(&midi, "english");
        let project = automatic.svp.expect("conversion succeeds");
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes[0].lyrics, "let");
        assert_eq!(automatic.placed, 1);
        assert_eq!(automatic.tracks[1].role, "vocal");
        assert_eq!(automatic.tracks[1].source_role, SourceRole::Vocal);
        assert!(automatic
            .projection
            .source_ids
            .contains("lyric:word:event:lyrics:0"));
        assert_eq!(
            automatic.tracks[1].export_representation,
            ExportRepresentation::VocalNotesAndReferenceMix
        );
        assert!(automatic.tracks[1]
            .warnings
            .iter()
            .any(|warning| warning.code == "EXTERNAL_KARAOKE_MELODY_TARGET"));
    }

    #[test]
    fn ordinary_midi_never_rebinds_a_detached_lyric_stream() {
        let mut lyrics = Track::new("lyrics", 0);
        lyrics.events.push(midi::Event::new(
            0,
            0,
            Kind::Lyrics(Lyric::text("word", "let".into())),
        ));
        let melody = pitched_track("melody", &[(0, 480, 60, 0)]);

        let outcome = convert_midi(&midi_with(vec![lyrics, melody]), "english");
        let project = outcome.svp.expect("conversion succeeds");
        assert_eq!(outcome.placed, 0);
        assert!(project.tracks.is_empty());
        assert!(!outcome
            .projection
            .source_ids
            .contains("lyric:word:event:lyrics:0"));
    }

    #[test]
    fn kar_binding_never_replaces_a_target_owned_lyric_stream() {
        let mut detached = Track::new("detached", 0);
        detached.events.push(midi::Event::new(
            0,
            0,
            Kind::Lyrics(Lyric::text("detached-word", "external".into())),
        ));
        let mut melody = pitched_track("melody", &[(0, 480, 60, 0)]);
        melody.events.insert(
            0,
            midi::Event::new(
                0,
                0,
                Kind::Lyrics(Lyric::text("owned-word", "owned".into())),
            ),
        );
        for (order, event) in melody.events.iter_mut().enumerate() {
            event.order = order as u32;
        }

        let outcome = convert_midi(&kar_with(vec![detached, melody]), "english");
        assert_eq!(outcome.placed, 1);
        let project = outcome.svp.expect("conversion succeeds");
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes[0].lyrics, "owned");
        assert!(!outcome
            .projection
            .source_ids
            .contains("lyric:detached-word:event:detached:0"));
    }

    #[test]
    fn ambiguous_external_lyric_streams_remain_unassigned_even_with_override() {
        let mut lyrics_a = Track::new("lyrics-a", 0);
        lyrics_a.events.push(midi::Event::new(
            0,
            0,
            Kind::Lyrics(Lyric::text("word-a", "first".into())),
        ));
        let mut lyrics_b = Track::new("lyrics-b", 1);
        lyrics_b.events.push(midi::Event::new(
            0,
            0,
            Kind::Lyrics(Lyric::text("word-b", "second".into())),
        ));
        let mut melody = Track::new("melody", 2);
        melody.events = note_events("melody", 0, 480, Vec::new());

        let outcome = convert_midi_with(
            &kar_with(vec![lyrics_a, lyrics_b, melody]),
            "english",
            Some(&HashMap::from([(2usize, true)])),
        );
        let project = outcome.svp.expect("override still exports the notes");
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes[0].lyrics, "");
        assert_eq!(outcome.placed, 0);
    }

    #[test]
    fn soft_karaoke_controls_are_metadata_and_never_sung() {
        let lyrics = karaoke_text_track(
            "words",
            &[(0, "\\let"), (120, "."), (240, "/...."), (480, "/it")],
        );
        let melody = pitched_track("melody", &[(0, 240, 60, 0), (480, 240, 62, 0)]);

        let outcome = convert_midi(&kar_with(vec![lyrics, melody]), "english");
        let project = outcome.svp.expect("conversion succeeds");
        assert_eq!(outcome.placed, 2);
        assert_eq!(
            project.tracks[0]
                .main_group
                .notes
                .iter()
                .map(|note| note.lyrics.as_str())
                .collect::<Vec<_>>(),
            vec!["let", "it"]
        );
        assert_eq!(outcome.tracks[0].lyric_status.source_text_count, 2);
        assert!(outcome.tracks[0].warnings.iter().any(|warning| {
            warning.code == "KARAOKE_CONTROLS_PRESERVED_AS_METADATA"
                && warning.message.starts_with('2')
        }));
    }

    #[test]
    fn incomplete_or_non_unique_external_matches_remain_source_only() {
        let partial_words =
            karaoke_text_track("partial", &[(0, "\\one"), (480, "two"), (960, "three")]);
        let partial_melody = pitched_track("short", &[(0, 240, 60, 0), (480, 240, 62, 0)]);
        let partial = convert_midi(&kar_with(vec![partial_words, partial_melody]), "english");
        assert_eq!(partial.placed, 0);
        assert!(partial.svp.expect("conversion succeeds").tracks.is_empty());

        let ambiguous_words = karaoke_text_track("ambiguous", &[(0, "\\one"), (480, "two")]);
        let first = pitched_track("first", &[(0, 240, 60, 0), (480, 240, 62, 0)]);
        let second = pitched_track("second", &[(0, 240, 65, 1), (480, 240, 67, 1)]);
        let ambiguous = convert_midi(&kar_with(vec![ambiguous_words, first, second]), "english");
        assert_eq!(ambiguous.placed, 0);
        assert!(ambiguous
            .svp
            .expect("conversion succeeds")
            .tracks
            .is_empty());
        assert!(ambiguous.tracks[0]
            .warnings
            .iter()
            .any(|warning| warning.code == "EXTERNAL_KARAOKE_LYRICS_AMBIGUOUS"));
    }

    #[test]
    fn percussion_candidates_are_excluded_and_chord_pitches_stay_unassigned() {
        let percussion_words = karaoke_text_track("drum-words", &[(0, "\\hit")]);
        let percussion = pitched_track("drums", &[(0, 240, 36, 9)]);
        let rejected = convert_midi(&kar_with(vec![percussion_words, percussion]), "english");
        assert_eq!(rejected.placed, 0);
        assert!(rejected.svp.expect("conversion succeeds").tracks.is_empty());

        let words = karaoke_text_track("words", &[(0, "\\one"), (480, "two")]);
        let chord = pitched_track(
            "melody",
            &[(0, 240, 60, 0), (0, 240, 64, 0), (480, 240, 67, 0)],
        );
        let outcome = convert_midi(&kar_with(vec![words, chord]), "english");
        let project = outcome.svp.expect("conversion succeeds");
        assert_eq!(outcome.placed, 1);
        assert_eq!(
            project.tracks[0]
                .main_group
                .notes
                .iter()
                .filter(|note| note.lyrics == "two")
                .count(),
            1
        );
        assert!(project.tracks[0]
            .main_group
            .notes
            .iter()
            .filter(|note| note.onset == 0)
            .all(|note| note.lyrics.is_empty()));
        assert!(outcome.tracks[0].warnings.iter().any(|warning| {
            warning.code == "KARAOKE_CHORD_PITCH_AMBIGUOUS" && warning.message.starts_with('1')
        }));
    }

    #[test]
    fn midi_lyric_line_breaks_are_controls_not_tokens() {
        let mut lyrics = Track::new("lyrics", 0);
        for (order, (tick, value)) in [(0, (0, "\r")), (1, (0, "let")), (2, (480, "it"))] {
            lyrics.events.push(midi::Event::new(
                tick,
                order,
                Kind::Lyrics(Lyric::text(format!("lyric-{order}"), value.into())),
            ));
        }
        let melody = pitched_track("melody", &[(0, 240, 60, 0), (480, 240, 62, 0)]);
        let outcome = convert_midi(&kar_with(vec![lyrics, melody]), "english");
        let project = outcome.svp.expect("conversion succeeds");
        assert_eq!(outcome.placed, 2);
        assert_eq!(
            project.tracks[0]
                .main_group
                .notes
                .iter()
                .map(|note| note.lyrics.as_str())
                .collect::<Vec<_>>(),
            vec!["let", "it"]
        );
        assert_eq!(outcome.tracks[0].lyric_status.source_text_count, 2);
    }

    #[test]
    fn karaoke_tokens_skip_zero_duration_and_unpitched_notes() {
        let source_note = |id: &str, onset, duration, pitch| SourceNote {
            onset,
            duration,
            pitch,
            source_order: 0,
            end_order: 1,
            source: midi::NoteSource {
                id: id.into(),
                ..midi::NoteSource::default()
            },
            lyrics: Vec::new(),
        };
        let token = TimedLyric {
            track_id: "lyrics".into(),
            tick: 0,
            order: 0,
            lyric: Lyric::text("token", "word".into()),
            origin: TimedLyricOrigin::KaraokeText,
        };
        let notes = vec![
            source_note("zero-duration", 0, 0, Some(60)),
            source_note("unpitched", 0, 480, None),
            source_note("projectable", 10, 480, Some(62)),
        ];

        let assignment = karaoke_assignment(&[token], &notes, 480);
        assert_eq!(assignment.len(), 1);
        assert_eq!(
            assignment.get(&2).map(|timed| &timed.lyric.state),
            Some(&midi::LyricState::Text("word".into()))
        );
    }

    #[test]
    fn grace_note_is_counted_but_never_given_an_invented_svp_duration() {
        let mut lyric = Lyric::text("grace-lyric", "let".into());
        lyric.lane = "1".into();
        let mut track = Track::new("grace", 0);
        track.events = note_events("grace", 0, 0, vec![lyric]);

        let outcome = convert_midi(&midi_with(vec![track]), "english");
        assert_eq!(outcome.tracks[0].notes, 1);
        assert_eq!(outcome.placed, 0);
        assert!(outcome.svp.expect("conversion succeeds").tracks.is_empty());
        assert!(!outcome
            .projection
            .source_ids
            .contains("note:grace:grace-note:occurrence:0:event:0"));
    }
}
