//! MIDI -> Synthesizer V conversion logic. 1:1 port of kar2svp_core.py.
use crate::engine::midi::{
    self, Kind, LineBreak, Lyric, Midi, MidiTextProfile, NoteOn, TimeBase, Track, TrackRoleHint,
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
}

fn karaoke_text_lyric(
    track_id: &str,
    tick: u32,
    order: u32,
    text: &midi::TextEvent,
) -> Option<Lyric> {
    let raw = text.text.as_str();
    if raw.starts_with('@') {
        return None;
    }
    let (line_break, value) = if let Some(rest) = raw.strip_prefix('\\') {
        (Some(LineBreak::Paragraph), rest)
    } else if let Some(rest) = raw.strip_prefix('/') {
        (Some(LineBreak::Line), rest)
    } else {
        (None, raw)
    };
    if value.is_empty() {
        return None;
    }
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
        let lyric = match &event.kind {
            Kind::Lyrics(lyric) => Some(lyric.clone()),
            Kind::Text(text) if track.text_profile == MidiTextProfile::KaraokeLyrics => {
                karaoke_text_lyric(&track.id, event.tick, event.order, text)
            }
            _ => None,
        };
        if let Some(lyric) = lyric {
            tokens.push(TimedLyric {
                track_id: track.id.clone(),
                tick: event.tick,
                order: event.order,
                lyric,
            });
        }
    }
    tokens.sort_by_key(|token| (token.tick, token.order));
    tokens
}

fn read_tempo(midi: &Midi, bpt: f64) -> (Vec<Tempo>, BTreeSet<String>) {
    let mut seen: BTreeMap<i64, (f64, String)> = BTreeMap::new();
    for track in &midi.tracks {
        for event in &track.events {
            if let Kind::Tempo(us) = event.kind {
                if us > 0 {
                    let pos = (event.tick as f64 * bpt).round() as i64;
                    let bpm = (60_000_000.0 / us as f64 * 1e6).round() / 1e6;
                    seen.insert(pos, (bpm, format!("event:{}:{}", track.id, event.order)));
                }
            }
        }
    }
    if seen.is_empty() {
        return (
            vec![Tempo {
                bpm: 120.0,
                position: 0,
            }],
            BTreeSet::new(),
        );
    }
    let evidence = seen.values().map(|(_, id)| id.clone()).collect();
    let tempo = seen
        .into_iter()
        .map(|(position, (bpm, _))| Tempo { bpm, position })
        .collect();
    (tempo, evidence)
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

fn selected_attached_lyric<'a>(note: &'a SourceNote, lane: &str) -> Option<&'a Lyric> {
    let playback = note.source.occurrence + 1;
    note.lyrics
        .iter()
        .filter(|lyric| lyric.lane == lane)
        .find(|lyric| !lyric.time_only.is_empty() && lyric.time_only.contains(&playback))
        .or_else(|| {
            note.lyrics
                .iter()
                .find(|lyric| lyric.lane == lane && lyric.time_only.is_empty())
        })
}

struct TrackProjection<'a> {
    source_track_id: &'a str,
    lane: Option<&'a str>,
    standalone: &'a HashMap<usize, TimedLyric>,
    evidence: &'a mut ProjectionEvidence,
}

fn make_track(
    idx: usize,
    name: &str,
    notes: &[SourceNote],
    bpt: f64,
    projection: TrackProjection<'_>,
) -> SvpTrack {
    let mut svp_notes = Vec::with_capacity(notes.len());
    let mut explicit_extension_end = None;
    let mut musicxml_extension_open = false;
    for (index, source_note) in notes.iter().enumerate() {
        let mut lyric_source_id = None;
        let mut lyric_event_id = None;
        let attached = projection
            .lane
            .and_then(|lane| selected_attached_lyric(source_note, lane));
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
        svp_notes.push(Note {
            attributes: serde_json::json!({}),
            duration: (source_note.duration as f64 * bpt).round() as i64,
            lyrics: lyric,
            onset: (source_note.onset as f64 * bpt).round() as i64,
            phonemes: String::new(),
            pitch,
        });
    }
    build_track(idx, name.to_string(), svp_notes)
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
    let bpt = BLICKS_PER_QUARTER / f64::from(tpb);
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

    for (index, track) in midi.tracks.iter().enumerate() {
        let notes = extract_notes(track);
        let source_note_count = source_note_count(track);
        let own_tokens = track_tokens(track);
        let name = if track.name.is_empty() {
            format!("Track {}", track.source.source_track)
        } else {
            track.name.clone()
        };
        if notes.is_empty() {
            let lyric_status = lyric_status(track, 0);
            let source_role = source_role(
                track,
                !own_tokens.is_empty(),
                source_note_count,
                &lyric_status,
            );
            let warnings = track_warnings(
                track,
                &lyric_status,
                source_role,
                source_note_count,
                0,
                !own_tokens.is_empty(),
                overrides.and_then(|map| map.get(&index).copied()),
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
        let lanes = attached_lanes(&notes);
        let attached = !lanes.is_empty();
        let assignment = if track.text_profile == MidiTextProfile::KaraokeLyrics {
            karaoke_assignment(&own_tokens, &notes, tpb)
        } else {
            exact_assignment(&own_tokens, &notes)
        };

        let source_vocal = attached || !assignment.is_empty();
        let explicit_override = overrides.and_then(|map| map.get(&index).copied());
        let sing = explicit_override.unwrap_or(source_vocal);
        let mut placed = 0usize;
        if sing {
            if lanes.is_empty() {
                placed = projected_lyric_count(&notes, None, &assignment);
                let mut svp_track = make_track(
                    svp_tracks.len(),
                    &name,
                    &notes,
                    bpt,
                    TrackProjection {
                        source_track_id: &track.id,
                        lane: None,
                        standalone: &assignment,
                        evidence: &mut projection,
                    },
                );
                if !svp_track.main_group.notes.is_empty() {
                    svp_track.main_ref.database.language = language.to_string();
                    svp_tracks.push(svp_track);
                }
            } else {
                let no_assignment = HashMap::new();
                for lane in &lanes {
                    placed += projected_lyric_count(&notes, Some(lane), &no_assignment);
                    let lane_name = if lanes.len() == 1 {
                        name.clone()
                    } else {
                        format!("{name} — lyric lane {lane}")
                    };
                    let mut svp_track = make_track(
                        svp_tracks.len(),
                        &lane_name,
                        &notes,
                        bpt,
                        TrackProjection {
                            source_track_id: &track.id,
                            lane: Some(lane),
                            standalone: &no_assignment,
                            evidence: &mut projection,
                        },
                    );
                    if !svp_track.main_group.notes.is_empty() {
                        svp_track.main_ref.database.language = language.to_string();
                        svp_tracks.push(svp_track);
                    }
                }
            }
        }
        total_placed += placed;
        let projectable_notes = notes
            .iter()
            .filter(|note| note.pitch.is_some() && note.duration > 0)
            .count();
        let standalone_with_attached_lanes = attached && !own_tokens.is_empty();
        let mut lyric_status = lyric_status(track, placed);
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

    let (tempo, tempo_evidence) = read_tempo(midi, bpt);
    projection.source_ids.extend(meter_evidence);
    projection.source_ids.extend(tempo_evidence);
    let svp = SvpProject {
        version: 113,
        time: Time { meter, tempo },
        render_config: RenderConfig::default(),
        tracks: svp_tracks,
    };
    let n_tracks = report.len();
    ConvertOutcome {
        ok: true,
        msg: None,
        svp: Some(svp),
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

fn projected_lyric_count(
    notes: &[SourceNote],
    lane: Option<&str>,
    assignment: &HashMap<usize, TimedLyric>,
) -> usize {
    notes
        .iter()
        .enumerate()
        .filter(|(index, note)| {
            if note.pitch.is_none() || note.duration == 0 {
                return false;
            }
            lane.and_then(|lane| selected_attached_lyric(note, lane))
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
            Kind::Lyrics(lyric) => count(lyric),
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

fn attached_lanes(notes: &[SourceNote]) -> BTreeSet<String> {
    notes
        .iter()
        .flat_map(|note| note.lyrics.iter().map(|lyric| lyric.lane.clone()))
        .collect()
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
            tracks,
        }
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
        let (tempo, tempo_evidence) = read_tempo(&midi, BLICKS_PER_QUARTER / 480.0);
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
    fn vocal_override_never_copies_lyrics_from_another_track() {
        let mut lyrics = Track::new("lyrics", 0);
        lyrics.events.push(midi::Event::new(
            0,
            0,
            Kind::Lyrics(Lyric::text("word", "let".into())),
        ));
        let mut melody = Track::new("melody", 1);
        melody.name = "Soprano Vocal Melody".into();
        melody.events = note_events("melody", 0, 480, Vec::new());

        let midi = midi_with(vec![lyrics, melody]);
        let automatic = convert_midi(&midi, "english");
        assert!(automatic
            .svp
            .expect("conversion succeeds")
            .tracks
            .is_empty());
        assert_eq!(automatic.tracks[1].role, "backing");
        assert_eq!(automatic.tracks[1].source_role, SourceRole::Ambiguous);

        let forced = convert_midi_with(&midi, "english", Some(&HashMap::from([(1usize, true)])));
        let forced_project = forced.svp.expect("explicit override succeeds");
        assert_eq!(forced_project.tracks.len(), 1);
        assert_eq!(forced_project.tracks[0].main_group.notes[0].lyrics, "");
        assert_eq!(forced.placed, 0);
        assert!(!forced
            .projection
            .source_ids
            .contains("lyric:word:event:lyrics:0"));
        assert_eq!(
            forced.tracks[1].source_role,
            SourceRole::Ambiguous,
            "an export override must never rewrite the source role"
        );
        assert_eq!(
            forced.tracks[1].export_representation,
            ExportRepresentation::VocalNotesAndReferenceMix
        );
        assert!(forced.tracks[1]
            .warnings
            .iter()
            .any(|warning| warning.code == "USER_VOCAL_OVERRIDE"));
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
            &midi_with(vec![lyrics_a, lyrics_b, melody]),
            "english",
            Some(&HashMap::from([(2usize, true)])),
        );
        let project = outcome.svp.expect("override still exports the notes");
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].main_group.notes[0].lyrics, "");
        assert_eq!(outcome.placed, 0);
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
