//! Source-owned MIDI/KAR performance, before any target scaling or sampling.
//!
//! Events are reconstructed in their original physical-track order before
//! resolving ports. Technical monophonic lanes never own controller state.
//! The gain policy is EXP-002's linear product of normalized CC7 and CC11,
//! with absent contributors neutral. It is not a universal GM amplitude law
//! or a claim of identical soundfont playback.
use super::midi::{Event, Kind, Midi, SourceFormat};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChannelKey {
    pub port: u8,
    pub channel: u8,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum Dimension {
    PitchCents,
    LinearGain,
    Other,
}

/// A value is held from this exact source tick until the next point.
/// `None` means unknown/unmapped state, never a neutral or zero value.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldPoint {
    pub tick: u32,
    pub value: Option<f64>,
    pub source_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PerformanceIssue {
    pub tick: u32,
    pub dimension: Dimension,
    pub source_ids: Vec<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelPerformance {
    pub pitch_cents: Vec<HeldPoint>,
    pub linear_gain: Vec<HeldPoint>,
    pub issues: Vec<PerformanceIssue>,
}

#[derive(Clone, Debug, Default)]
pub struct PerformanceIndex {
    pub channels: BTreeMap<ChannelKey, Arc<ChannelPerformance>>,
    /// (original projection lane ID, original note-on order), not note pitch
    /// or display name. The parser preserves event order through decomposition.
    pub notes: BTreeMap<(String, u32), ChannelKey>,
    pub events: BTreeMap<String, PerformanceEvent>,
}

#[derive(Clone, Debug)]
pub struct PerformanceEvent {
    pub track_id: String,
    pub tick: u32,
    pub dimension: Dimension,
}

/// The exact note ownership retained alongside a projected lane. The curve is
/// shared, not copied or composed once per phoneme.
#[derive(Clone, Debug, PartialEq)]
pub struct PerformanceNote {
    pub source_id: String,
    pub key: ChannelKey,
    pub timeline: Arc<ChannelPerformance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferStatus {
    Mapped,
    Unsupported,
    RepresentationLimit,
}

/// An adapter's editable-expression result, separate from raw source retention
/// and from nominal-note projection evidence. Bounds are source IR ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct PerformanceTransfer {
    pub track_id: String,
    /// Index of the actual target track/voice part, not merely a source lane.
    pub target_track: Option<usize>,
    pub dimension: Dimension,
    pub start_tick: u32,
    pub end_tick: u32,
    pub source_ids: Vec<String>,
    pub note_ids: Vec<String>,
    pub status: TransferStatus,
    pub message: String,
}

/// A bounded shared table in the ledger. Entries refer to its indices, so a
/// source event shared by many notes doesn't duplicate every note ID per row.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceReference {
    pub target: String,
    pub target_track: Option<usize>,
    pub source_track_id: String,
    pub dimension: Dimension,
    pub start_tick: u32,
    pub end_tick: u32,
    pub note_ids: Vec<String>,
    pub status: TransferStatus,
    pub detail: String,
}

#[derive(Clone)]
struct Input<'a> {
    physical_track: usize,
    event: &'a Event,
    id: String,
    port_id: Option<String>,
    port_pitch_hazard: bool,
}

impl Input<'_> {
    fn ids(&self) -> Vec<String> {
        let mut ids = vec![self.id.clone()];
        ids.extend(self.port_id.clone());
        ids
    }
}

#[derive(Default)]
struct State {
    rpn_msb: Option<(u8, String)>,
    rpn_lsb: Option<(u8, String)>,
    semitones: Option<(u8, Vec<String>)>,
    cents: Option<(u8, Vec<String>)>,
    bend: Option<(u16, Vec<String>)>,
    volume: Option<(u8, Vec<String>)>,
    expression: Option<(u8, Vec<String>)>,
    pitch_blocked: bool,
    gain_blocked: bool,
    nrpn_selected: bool,
    pitch_conflict_tick: Option<u32>,
    gain_conflicts: BTreeMap<u8, u32>,
    gain_conflict_ids: BTreeMap<u8, Vec<String>>,
    pitch_hazard_ids: Vec<String>,
    gain_hazard_ids: Vec<String>,
    result: ChannelPerformance,
}

fn push_point(points: &mut Vec<HeldPoint>, tick: u32, value: Option<f64>, ids: Vec<String>) {
    let ids: BTreeSet<String> = ids.into_iter().collect();
    if points.last().is_some_and(|point| point.tick == tick) {
        // Earlier same-track values at this instant have zero held duration.
        // Their raw events survive; only the final state's actual contributors
        // are attributed to the resulting point. This also bounds dense ticks.
        points.pop();
    }
    points.push(HeldPoint {
        tick,
        value,
        source_ids: ids.into_iter().collect(),
    });
}

impl State {
    fn issue(&mut self, input: &Input<'_>, dimension: Dimension, reason: &str) {
        self.result.issues.push(PerformanceIssue {
            tick: input.event.tick,
            dimension,
            source_ids: input.ids(),
            reason: reason.into(),
        });
    }

    fn pitch(&mut self, input: &Input<'_>) {
        let Some((bend, bend_ids)) = &self.bend else {
            return;
        };
        let mut ids = input.ids();
        ids.extend(self.pitch_hazard_ids.iter().cloned());
        ids.extend(bend_ids.iter().cloned());
        for (_, source) in [&self.semitones, &self.cents].into_iter().flatten() {
            ids.extend(source.iter().cloned());
        }
        let value = if self.pitch_blocked {
            None
        } else if *bend == 8192 {
            Some(0.0)
        } else {
            self.semitones.as_ref().map(|(semitones, _)| {
                let range = 100.0 * f64::from(*semitones)
                    + self
                        .cents
                        .as_ref()
                        .map_or(0.0, |(cents, _)| f64::from(*cents));
                (f64::from(*bend) - 8192.0) * range / 8192.0
            })
        };
        // Record the first unresolved transition, not one issue per held
        // recomputation. The unknown point carries its contributors forward.
        if value.is_none()
            && self
                .result
                .pitch_cents
                .last()
                .is_none_or(|p| p.value.is_some())
        {
            self.issue(
                input,
                Dimension::PitchCents,
                "Bend sensitivity or channel pitch state is unknown; cents were not guessed.",
            );
        }
        push_point(&mut self.result.pitch_cents, input.event.tick, value, ids);
    }

    fn gain(&mut self, input: &Input<'_>) {
        if self.volume.is_none()
            && self.expression.is_none()
            && !self.gain_blocked
            && self.gain_conflicts.is_empty()
        {
            return;
        }
        let mut ids = input.ids();
        ids.extend(self.gain_hazard_ids.iter().cloned());
        ids.extend(self.gain_conflict_ids.values().flatten().cloned());
        for (_, source) in [&self.volume, &self.expression].into_iter().flatten() {
            ids.extend(source.iter().cloned());
        }
        let value = (!self.gain_blocked && self.gain_conflicts.is_empty()).then(|| {
            let component = |value: &Option<(u8, Vec<String>)>| {
                value.as_ref().map_or(1.0, |(v, _)| f64::from(*v) / 127.0)
            };
            component(&self.volume) * component(&self.expression)
        });
        push_point(&mut self.result.linear_gain, input.event.tick, value, ids);
    }

    fn invalidate(&mut self, input: &Input<'_>, pitch: bool, gain: bool, reason: &str) {
        if pitch {
            self.pitch_blocked = true;
            self.pitch_hazard_ids = input.ids();
            self.issue(input, Dimension::PitchCents, reason);
            push_point(
                &mut self.result.pitch_cents,
                input.event.tick,
                None,
                input.ids(),
            );
        }
        if gain {
            self.gain_blocked = true;
            self.gain_hazard_ids = input.ids();
            self.issue(input, Dimension::LinearGain, reason);
            self.gain(input);
        }
    }

    fn apply(&mut self, input: &Input<'_>) {
        if input.port_pitch_hazard {
            self.invalidate(
                input,
                true,
                false,
                "Possible MPE zone configuration makes member-channel pitch state unsupported.",
            );
            return;
        }
        if self.pitch_conflict_tick == Some(input.event.tick)
            && pitch_state(&input.event.kind)
            && !matches!(input.event.kind, Kind::SysEx { .. })
        {
            return; // No same-tick ordering winner; later explicit state may recover.
        }
        match input.event.kind {
            Kind::PitchBend { value, .. } => {
                self.bend = Some((value, input.ids()));
                if value > 16383 {
                    self.invalidate(input, true, false, "Invalid 14-bit pitch bend.");
                } else {
                    self.pitch(input);
                }
            }
            Kind::ControlChange {
                controller, value, ..
            } => {
                if value > 127 || controller > 127 {
                    self.invalidate(input, true, true, "Invalid MIDI controller byte.");
                    return;
                }
                match controller {
                    101 => { self.nrpn_selected = false; self.rpn_msb = Some((value, input.id.clone())); }
                    100 => { self.nrpn_selected = false; self.rpn_lsb = Some((value, input.id.clone())); }
                    98 | 99 => { self.nrpn_selected = true; }
                    6 | 38 | 96 | 97 => {
                        if self.nrpn_selected {
                            self.invalidate(input, true, true, "Unknown NRPN data entry may alter pitch or gain; neither dimension has a verified mapping.");
                            return;
                        }
                        let selection = (self.rpn_msb.as_ref().map(|v| v.0), self.rpn_lsb.as_ref().map(|v| v.0));
                        match selection {
                            (Some(0), Some(0)) if controller == 6 || controller == 38 => {
                                let mut ids = input.ids();
                                ids.extend([&self.rpn_msb, &self.rpn_lsb].into_iter().flatten().map(|(_, id)| id.clone()));
                                if controller == 6 { self.semitones = Some((value, ids)); }
                                else { self.cents = Some((value, ids)); }
                                self.pitch(input);
                            }
                            (Some(127), Some(127)) => {
                                self.issue(input, Dimension::Other, "Data entry under null RPN selection has no mapped parameter.");
                            }
                            (Some(0), Some(0..=6)) => self.invalidate(input, true, false, "RPN tuning, MPE or relative parameter entry has no verified pitch mapping."),
                            _ => self.invalidate(input, true, true, "Unknown parameter data entry may alter pitch or gain; neither dimension has a verified mapping."),
                        }
                    }
                    7 | 11 => {
                        if self.gain_conflicts.get(&controller) != Some(&input.event.tick) {
                            self.gain_conflicts.remove(&controller);
                            self.gain_conflict_ids.remove(&controller);
                            if controller == 7 { self.volume = Some((value, input.ids())); }
                            else { self.expression = Some((value, input.ids())); }
                        }
                        self.gain(input);
                    }
                    39 | 43 => self.invalidate(input, false, true, "14-bit CC7/CC11 pairs are unsupported; no partial MSB-only gain is transferred."),
                    120..=127 => self.invalidate(input, true, true, "Channel-mode/reset behavior is unsupported."),
                    5 | 37 | 65 | 84 => self.invalidate(input, true, false, "Source portamento state has no verified pitch mapping."),
                    0 | 32 => {}, // Bank selection alone is not a performance envelope.
                    _ => self.issue(input, Dimension::Other, "This MIDI controller has no editable performance mapping; CC1 is not a fully specified vibrato."),
                }
            }
            Kind::SysEx { .. } => self.invalidate(
                input,
                true,
                true,
                "Unhandled SysEx may alter tuning, MPE, gain or reset state.",
            ),
            Kind::ChannelPressure { .. } | Kind::PolyPressure { .. } => self.issue(
                input,
                Dimension::Other,
                "MIDI pressure has no verified editable performance mapping.",
            ),
            _ => {}
        }
    }
}

fn channel(kind: &Kind) -> Option<u8> {
    match kind {
        Kind::NoteOn(n) => n.channel,
        Kind::NoteOff(n) => n.channel,
        Kind::ControlChange { channel, .. }
        | Kind::PitchBend { channel, .. }
        | Kind::ProgramChange { channel, .. }
        | Kind::ChannelPressure { channel, .. }
        | Kind::PolyPressure { channel, .. } => Some(*channel),
        _ => None,
    }
}

fn pitch_state(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::PitchBend { .. }
            | Kind::ControlChange {
                controller: 6 | 38 | 100 | 101 | 96..=99,
                ..
            }
            | Kind::SysEx { .. }
    )
}

/// Resolve native MIDI only. Score adapters' synthetic velocities and MIDI-like
/// events are not authorization to invent a score-expression interpretation.
pub fn normalize(midi: &Midi) -> Result<PerformanceIndex, String> {
    // Match the parser's event ceiling after port-wide hazard expansion too.
    normalize_bounded(midi, 2_000_000)
}

fn normalize_bounded(midi: &Midi, max_events: usize) -> Result<PerformanceIndex, String> {
    if !matches!(
        midi.source_format,
        SourceFormat::StandardMidi | SourceFormat::KaraokeMidi
    ) {
        return Ok(PerformanceIndex::default());
    }
    let mut physical: BTreeMap<usize, Vec<(&str, &Event)>> = BTreeMap::new();
    for track in &midi.tracks {
        physical
            .entry(track.source.source_track)
            .or_default()
            .extend(track.events.iter().map(|event| (track.id.as_str(), event)));
    }
    let mut index = PerformanceIndex::default();
    let mut streams: BTreeMap<ChannelKey, Vec<Input<'_>>> = BTreeMap::new();
    let mut sysex = Vec::new();
    for (physical_track, mut events) in physical {
        events.sort_by_key(|(_, event)| (event.tick, event.order));
        let mut port = 0;
        let mut port_id = None;
        for (lane, event) in events {
            let id = format!("event:{lane}:{}", event.order);
            if let Kind::Port(value) = event.kind {
                port = value;
                port_id = Some(id);
                continue;
            }
            let input = Input {
                physical_track,
                event,
                id,
                port_id: port_id.clone(),
                port_pitch_hazard: false,
            };
            let dimension = match event.kind {
                Kind::PitchBend { .. } => Some(Dimension::PitchCents),
                Kind::ControlChange {
                    controller: 7 | 11 | 39 | 43,
                    ..
                } => Some(Dimension::LinearGain),
                Kind::ControlChange {
                    controller: 0 | 32, ..
                } => None,
                Kind::ControlChange { .. }
                | Kind::SysEx { .. }
                | Kind::ChannelPressure { .. }
                | Kind::PolyPressure { .. } => Some(Dimension::Other),
                _ => None,
            };
            if let Some(dimension) = dimension {
                index.events.insert(
                    input.id.clone(),
                    PerformanceEvent {
                        track_id: lane.into(),
                        tick: event.tick,
                        dimension,
                    },
                );
            }
            if let Some(channel) = channel(&event.kind) {
                let key = ChannelKey { port, channel };
                if matches!(&event.kind, Kind::NoteOn(note) if note.velocity != Some(0)) {
                    index.notes.insert((lane.to_string(), event.order), key);
                }
                streams.entry(key).or_default().push(input);
            } else if matches!(event.kind, Kind::SysEx { .. }) {
                sysex.push((port, input));
            }
        }
    }
    // A complete RPN 0/6 data-entry operation may configure an MPE zone.
    // Parameter selection alone does not change any member's performance.
    let mut mpe = Vec::new();
    for (key, stream) in &mut streams {
        stream.sort_by_key(|input| (input.event.tick, input.physical_track, input.event.order));
        let (mut msb, mut lsb, mut nrpn) = (None, None, false);
        for input in stream.iter() {
            if let Kind::ControlChange {
                controller, value, ..
            } = input.event.kind
            {
                match controller {
                    101 => {
                        msb = Some(value);
                        nrpn = false;
                    }
                    100 => {
                        lsb = Some(value);
                        nrpn = false;
                    }
                    98 | 99 => nrpn = true,
                    6 | 38 | 96 | 97 if !nrpn && msb == Some(0) && lsb == Some(6) => {
                        let mut hazard = input.clone();
                        hazard.port_pitch_hazard = true;
                        mpe.push((key.port, hazard));
                    }
                    _ => {}
                }
            }
        }
    }
    let mut hazards_per_port = BTreeMap::<u8, usize>::new();
    for (port, _) in mpe.iter().chain(&sysex) {
        *hazards_per_port.entry(*port).or_default() += 1;
    }
    let expanded = streams.iter().try_fold(0usize, |total, (key, stream)| {
        total
            .checked_add(stream.len())?
            .checked_add(*hazards_per_port.get(&key.port).unwrap_or(&0))
    });
    if expanded.is_none_or(|count| count > max_events) {
        return Err(format!("MIDI_PERFORMANCE_LIMIT: channel performance exceeds {max_events} events after port-wide hazard expansion; no partial timeline was produced."));
    }
    let mut hazards_by_port = BTreeMap::<u8, Vec<Input<'_>>>::new();
    for (port, input) in mpe.into_iter().chain(sysex) {
        hazards_by_port.entry(port).or_default().push(input);
    }
    for (key, mut stream) in streams {
        stream.extend(
            hazards_by_port
                .get(&key.port)
                .into_iter()
                .flatten()
                .cloned(),
        );
        stream.sort_by_key(|input| (input.event.tick, input.physical_track, input.event.order));
        let mut state = State::default();
        let mut start = 0;
        while start < stream.len() {
            let end = start
                + stream[start..]
                    .partition_point(|input| input.event.tick == stream[start].event.tick);
            let group = &stream[start..end];
            // SMF ordering across physical tracks is undefined. Conservatively
            // decline same-tick pitch-state interactions instead of choosing an
            // arbitrary RPN/bend interleaving. Independent CC7 and CC11 commute.
            if group
                .iter()
                .filter(|input| pitch_state(&input.event.kind))
                .map(|input| input.physical_track)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
            {
                state.pitch_conflict_tick = Some(group[0].event.tick);
                state.rpn_msb = None;
                state.rpn_lsb = None;
                if group.iter().any(|i| {
                    matches!(
                        i.event.kind,
                        Kind::PitchBend { .. }
                            | Kind::ControlChange {
                                controller: 6 | 38 | 96 | 97,
                                ..
                            }
                    )
                }) {
                    state.semitones = None;
                    state.cents = None;
                    state.bend = None;
                    let ids = group
                        .iter()
                        .filter(|i| pitch_state(&i.event.kind))
                        .flat_map(Input::ids)
                        .collect();
                    push_point(
                        &mut state.result.pitch_cents,
                        group[0].event.tick,
                        None,
                        ids,
                    );
                    state.issue(&group[0], Dimension::PitchCents, "Simultaneous pitch state on different physical tracks has ambiguous ordering; later complete explicit state can recover it.");
                }
            }
            for controller in [7, 11] {
                let controls: Vec<_> = group
                    .iter()
                    .filter_map(|input| match input.event.kind {
                        Kind::ControlChange {
                            controller: c,
                            value,
                            ..
                        } if c == controller => Some((input, value)),
                        _ => None,
                    })
                    .collect();
                let owners = controls
                    .iter()
                    .map(|(input, _)| input.physical_track)
                    .collect::<BTreeSet<_>>();
                let values = controls
                    .iter()
                    .map(|(_, value)| *value)
                    .collect::<BTreeSet<_>>();
                if owners.len() > 1 && values.len() > 1 {
                    state.gain_conflicts.insert(controller, group[0].event.tick);
                    if controller == 7 {
                        state.volume = None;
                    } else {
                        state.expression = None;
                    }
                    let ids: Vec<_> = controls.iter().flat_map(|(input, _)| input.ids()).collect();
                    state.gain_conflict_ids.insert(controller, ids.clone());
                    push_point(
                        &mut state.result.linear_gain,
                        group[0].event.tick,
                        None,
                        ids,
                    );
                }
            }
            for input in group {
                state.apply(input);
            }
            start = end;
        }
        if state.result != ChannelPerformance::default() {
            index.channels.insert(key, Arc::new(state.result));
        }
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::super::midi::{self, SourceTopology, TimeBase, Track};
    use super::*;

    fn cc(controller: u8, value: u8) -> Kind {
        Kind::ControlChange {
            channel: 0,
            controller,
            value,
        }
    }
    fn bend(value: u16) -> Kind {
        Kind::PitchBend { channel: 0, value }
    }
    fn run(events: Vec<(u32, Kind)>) -> Arc<ChannelPerformance> {
        let mut track = Track::new("test", 0);
        track.events = events
            .into_iter()
            .enumerate()
            .map(|(order, (tick, kind))| Event::new(tick, order as u32, kind))
            .collect();
        let midi = Midi {
            staff_links: Vec::new(),
            ticks_per_beat: 480,
            time_base: TimeBase::PulsesPerQuarter(480),
            format: 1,
            source_format: SourceFormat::StandardMidi,
            topology: SourceTopology::default(),
            tracks: vec![track],
        };
        normalize(&midi).unwrap().channels[&ChannelKey {
            port: 0,
            channel: 0,
        }]
            .clone()
    }
    #[test]
    fn explicit_range_and_held_bend_are_source_exact() {
        let curve = run(vec![
            (0, cc(101, 0)),
            (0, cc(100, 0)),
            (0, cc(6, 2)),
            (0, bend(8192)),
            (240, bend(12288)),
            (480, bend(0)),
            (720, bend(8192)),
        ]);
        assert_eq!(
            curve
                .pitch_cents
                .iter()
                .map(|p| (p.tick, p.value))
                .collect::<Vec<_>>(),
            vec![
                (0, Some(0.0)),
                (240, Some(100.0)),
                (480, Some(-200.0)),
                (720, Some(0.0))
            ]
        );
        assert!(curve.pitch_cents[1]
            .source_ids
            .contains(&"event:test:2".into()));
    }
    #[test]
    fn sensitivity_changes_recompute_without_a_new_bend() {
        let curve = run(vec![
            (0, cc(101, 0)),
            (0, cc(100, 0)),
            (0, cc(6, 2)),
            (0, bend(12288)),
            (120, cc(6, 12)),
            (240, cc(38, 50)),
        ]);
        assert_eq!(
            curve
                .pitch_cents
                .iter()
                .map(|p| p.value)
                .collect::<Vec<_>>(),
            vec![Some(100.0), Some(600.0), Some(625.0)]
        );
    }
    #[test]
    fn center_needs_no_range_and_noncenter_never_guesses() {
        let curve = run(vec![(0, bend(8192)), (100, bend(16383))]);
        assert_eq!(curve.pitch_cents[0].value, Some(0.0));
        assert_eq!(curve.pitch_cents[1].value, None);
        assert!(!curve.issues.is_empty());
    }
    #[test]
    fn gain_composes_once_and_preserves_mute_restoration() {
        let curve = run(vec![
            (0, cc(7, 64)),
            (100, cc(11, 64)),
            (200, cc(11, 0)),
            (300, cc(11, 127)),
        ]);
        let gain = 64.0 / 127.0;
        assert_eq!(
            curve
                .linear_gain
                .iter()
                .map(|p| p.value)
                .collect::<Vec<_>>(),
            vec![Some(gain), Some(gain * gain), Some(0.0), Some(gain)]
        );
        assert!(curve.linear_gain[1]
            .source_ids
            .contains(&"event:test:0".into()));
    }
    #[test]
    fn null_selection_does_not_change_range_and_fractional_cents_survive() {
        let curve = run(vec![
            (0, cc(101, 0)),
            (0, cc(100, 0)),
            (0, cc(6, 2)),
            (0, bend(8193)),
            (1, cc(101, 127)),
            (1, cc(100, 127)),
            (2, cc(6, 12)),
            (3, bend(8193)),
        ]);
        assert_eq!(curve.pitch_cents[0].value, Some(200.0 / 8192.0));
        assert_eq!(
            curve.pitch_cents.last().unwrap().value,
            Some(200.0 / 8192.0)
        );
    }
    #[test]
    fn unsupported_low_bits_and_resets_never_leave_partial_gain() {
        for kind in [
            cc(39, 1),
            cc(43, 1),
            cc(121, 0),
            Kind::SysEx {
                escaped: false,
                data: vec![0x7e, 0x7f, 9, 1, 0xf7],
            },
        ] {
            let curve = run(vec![(0, cc(7, 64)), (100, kind), (200, cc(11, 127))]);
            assert_eq!(curve.linear_gain.last().unwrap().value, None);
            assert!(!curve.issues.is_empty());
        }
    }
    #[test]
    fn singing_fixture_retains_raw_events_and_original_note_ownership() {
        let bytes = [
            0, 0xb0, 7, 64, 0, 0xff, 5, 2, b'l', b'a', 0, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0,
            0, 0xff, 0x2f, 0,
        ];
        let mut file = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        file.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        file.extend(bytes);
        let midi = midi::parse(&file).unwrap();
        let before = midi.tracks.clone();
        let index = normalize(&midi).unwrap();
        assert_eq!(midi.tracks, before);
        assert_eq!(index.notes.len(), 1);
        assert_eq!(index.channels.len(), 1);
    }

    fn smf(tracks: &[&[u8]]) -> Midi {
        let mut file = b"MThd\0\0\0\x06\0\x01".to_vec();
        file.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
        file.extend_from_slice(&480u16.to_be_bytes());
        for track in tracks {
            file.extend_from_slice(b"MTrk");
            file.extend_from_slice(&((track.len() + 4) as u32).to_be_bytes());
            file.extend_from_slice(track);
            file.extend_from_slice(&[0, 0xff, 0x2f, 0]);
        }
        midi::parse(&file).unwrap()
    }

    #[test]
    fn port_hazard_expansion_is_bounded_before_copying_channel_streams() {
        let midi = smf(&[&[
            0, 0xb0, 7, 64, 0, 0xb1, 7, 64, 0, 0xf0, 5, 0x7e, 0x7f, 9, 1, 0xf7,
        ]]);
        let before = midi.tracks.clone();
        assert!(normalize_bounded(&midi, 3)
            .unwrap_err()
            .contains("MIDI_PERFORMANCE_LIMIT"));
        assert_eq!(normalize_bounded(&midi, 4).unwrap().channels.len(), 2);
        assert_eq!(midi.tracks, before);
    }

    #[test]
    fn original_port_and_channel_reach_physical_and_polyphonic_siblings() {
        let midi = smf(&[
            &[0, 0xff, 0x21, 1, 3, 0, 0xb0, 7, 64],
            &[
                0, 0xff, 0x21, 1, 3, 0, 0xff, 5, 2, b'l', b'a', 0, 0x90, 60, 100, 0, 0x90, 64, 100,
                0x83, 0x60, 0x80, 60, 0, 0, 0x80, 64, 0,
            ],
            &[
                0, 0xff, 0x21, 1, 4, 0, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0,
            ],
            &[
                0, 0xff, 0x21, 1, 3, 0, 0x91, 60, 100, 0x83, 0x60, 0x81, 60, 0,
            ],
        ]);
        assert_eq!(midi.tracks.len(), 5);
        let result = normalize(&midi).unwrap();
        let port3 = ChannelKey {
            port: 3,
            channel: 0,
        };
        assert_eq!(
            result.notes.values().filter(|key| **key == port3).count(),
            2
        );
        assert_eq!(result.channels.len(), 1);
        let point = &result.channels[&port3].linear_gain[0];
        assert_eq!(point.value, Some(64.0 / 127.0));
        assert_eq!(
            point.source_ids,
            vec!["event:midi-track-0:0", "event:midi-track-0:1"]
        );
    }

    #[test]
    fn cross_track_conflicts_are_not_resolved_by_track_number() {
        let gain = smf(&[&[0, 0xb0, 7, 64], &[0, 0xb0, 7, 127]]);
        let key = ChannelKey {
            port: 0,
            channel: 0,
        };
        let result = normalize(&gain).unwrap();
        assert_eq!(result.channels[&key].linear_gain[0].value, None);
        let bend = smf(&[
            &[0, 0xb0, 101, 0, 0, 0xb0, 100, 0, 0, 0xb0, 6, 2],
            &[0, 0xe0, 0, 96],
        ]);
        let result = normalize(&bend).unwrap();
        assert_eq!(result.channels[&key].pitch_cents[0].value, None);
        assert!(result.channels[&key]
            .issues
            .iter()
            .any(|issue| issue.reason.contains("ambiguous ordering")));
    }

    #[test]
    fn independent_cross_track_gain_contributors_commute() {
        let midi = smf(&[&[0, 0xb0, 7, 64], &[0, 0xb0, 11, 64]]);
        let result = normalize(&midi).unwrap();
        let timeline = &result.channels[&ChannelKey {
            port: 0,
            channel: 0,
        }];
        assert_eq!(
            timeline.linear_gain[0].value,
            Some((64.0 / 127.0) * (64.0 / 127.0))
        );
        assert!(timeline.issues.is_empty());
    }

    #[test]
    fn mpe_selection_invalidates_members_but_not_other_ports() {
        let midi = smf(&[
            &[0, 0xb0, 101, 0, 0, 0xb0, 100, 6, 0, 0xb0, 6, 8],
            &[
                0, 0xb1, 101, 0, 0, 0xb1, 100, 0, 0, 0xb1, 6, 2, 0, 0xe1, 0, 96,
            ],
            &[
                0, 0xff, 0x21, 1, 1, 0, 0xb1, 101, 0, 0, 0xb1, 100, 0, 0, 0xb1, 6, 2, 0, 0xe1, 0,
                96,
            ],
        ]);
        let result = normalize(&midi).unwrap();
        assert_eq!(
            result.channels[&ChannelKey {
                port: 0,
                channel: 1
            }]
                .pitch_cents[0]
                .value,
            None
        );
        assert_eq!(
            result.channels[&ChannelKey {
                port: 1,
                channel: 1
            }]
                .pitch_cents[0]
                .value,
            Some(100.0)
        );
    }

    #[test]
    fn later_null_selection_does_not_erase_range_selection_provenance() {
        let curve = run(vec![
            (0, cc(101, 0)),
            (0, cc(100, 0)),
            (0, cc(6, 2)),
            (1, cc(101, 127)),
            (1, cc(100, 127)),
            (10, bend(12288)),
        ]);
        assert!(curve.pitch_cents[0]
            .source_ids
            .contains(&"event:test:0".into()));
        assert!(curve.pitch_cents[0]
            .source_ids
            .contains(&"event:test:1".into()));
    }
}
