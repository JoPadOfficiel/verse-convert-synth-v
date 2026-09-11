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

/// Field-owned mapping evidence must not replace the NoteOn's nominal-note claim.
pub fn attack_field_id(track: &str, order: u32) -> String {
    format!("expression:event:{track}:{order}:velocity")
}

/// Scale a nonnegative exact quarter coordinate without multiplying its wide
/// numerator by the tick rate. The remainder stays below the denominator even
/// when that multiplication would overflow. Check the unrounded range first.
fn exact_tick_parts(
    time: super::score_intensity::Time,
    rate: u32,
    maximum: u32,
) -> Result<(u32, u128, u128), String> {
    let (n, d) = time.ratio();
    if n < 0 || d <= 0 || rate == 0 {
        return Err("Invalid nonnegative performance time or tick rate".into());
    }
    let (n, d) = (n as u128, d as u128);
    let whole = (n / d)
        .checked_mul(u128::from(rate))
        .ok_or("Performance time exceeds tick range")?;
    let fraction = n % d;
    let (mut quotient, mut remainder) = (0u128, 0u128);
    // d <= i128::MAX, so each individual doubled/additive remainder fits u128.
    for bit in (0..u32::BITS).rev() {
        quotient *= 2;
        remainder *= 2;
        if remainder >= d {
            remainder -= d;
            quotient += 1;
        }
        if rate & (1 << bit) != 0 {
            remainder += fraction;
            if remainder >= d {
                remainder -= d;
                quotient += 1;
            }
        }
    }
    let ticks = whole
        .checked_add(quotient)
        .filter(|ticks| {
            *ticks < u128::from(maximum) || (*ticks == u128::from(maximum) && remainder == 0)
        })
        .ok_or("Performance time exceeds tick range")?;
    Ok((ticks as u32, remainder, d))
}

/// Shared by USTX and ledger validation. Nearest, ties away from zero, with
/// exact nonnegative i32 target range; Time's decimal-string encoding is intact.
pub(crate) fn exact_target_tick(time: super::score_intensity::Time) -> Result<i32, String> {
    let (ticks, remainder, denominator) = exact_tick_parts(time, 480, i32::MAX as u32)?;
    let rounded = ticks + u32::from(remainder >= denominator - remainder);
    i32::try_from(rounded).map_err(|_| "Performance time exceeds target tick range".into())
}

/// Exact outward source coverage, shared by emitted records and ledger checks.
pub(crate) fn exact_source_tick_bounds(
    start: super::score_intensity::Time,
    end: super::score_intensity::Time,
    ppq: u16,
) -> Result<(u32, u32), String> {
    if end < start {
        return Err("Reversed exact performance bounds".into());
    }
    let (lo, _, _) = exact_tick_parts(start, u32::from(ppq), u32::MAX)?;
    let (hi, remainder, _) = exact_tick_parts(end, u32::from(ppq), u32::MAX)?;
    Ok((lo, hi + u32::from(remainder != 0)))
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
    pub bindings: BTreeMap<(String, u32), PerformanceNote>,
    pub channels: BTreeMap<ChannelKey, Arc<ChannelPerformance>>,
    /// (original projection lane ID, original note-on order), not note pitch
    /// or display name. The parser preserves event order through decomposition.
    pub notes: BTreeMap<(String, u32), ChannelKey>,
    pub events: BTreeMap<String, PerformanceEvent>,
    /// Complete performed diagnostics, shared independently of selected notes.
    pub(crate) score_issues: Vec<ScoreIssueOwner>,
    /// Original source lanes may share a score voice, and a metadata staff lane
    /// may retain several declared voices. Never derive this relation from IDs.
    pub(crate) score_track_owners: BTreeMap<String, BTreeSet<super::score_intensity::ScoreVoice>>,
    /// Normalization and every selected lyric group spend the same counters.
    provenance_budget: std::cell::RefCell<super::score_intensity::ProvenanceBudget>,
}

#[derive(Clone, Debug)]
pub(crate) struct ScoreIssueOwner {
    pub track_id: String,
    pub voice: super::score_intensity::ScoreVoice,
    pub timeline: Arc<super::score_intensity::Timeline>,
    pub terminal: Option<super::score_intensity::Time>,
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
    pub owner: PerformanceOwner,
    pub intensity: Option<Arc<super::score_intensity::NoteIntensity>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PerformanceOwner {
    Midi {
        key: ChannelKey,
        timeline: Arc<ChannelPerformance>,
    },
    Score {
        voice: super::score_intensity::ScoreVoice,
    },
}

impl PerformanceNote {
    /// Called only after source-typed tie proof and selected-verse eligibility.
    /// Tail identity/channel ownership remains its own; only the attack context
    /// is shared. This is not a tie detector and never changes nominal geometry.
    pub fn continue_tied_intensity(&mut self, head: &Self) -> Result<(), String> {
        self.continue_tied_intensity_bounded(
            head,
            &mut super::score_intensity::ProvenanceBudget::default(),
        )
    }
    fn continue_tied_intensity_bounded(
        &mut self,
        head: &Self,
        budget: &mut super::score_intensity::ProvenanceBudget,
    ) -> Result<(), String> {
        match (&self.owner, &head.owner) {
            (PerformanceOwner::Score { voice: tail }, PerformanceOwner::Score { voice: head })
                if tail == head => {}
            _ => {
                return Err(
                    "A score tie cannot transfer attack context across source owners".into(),
                )
            }
        }
        if let Some(head_intensity) = &head.intensity {
            let continued = super::score_intensity::NoteIntensity::continued_from_bounded(
                head_intensity,
                self.intensity.as_deref(),
                budget,
            )?;
            self.intensity = Some(Arc::new(continued));
        } else {
            return Err("A source-proven tie needs its head's resolved intensity context before continuation binding".into());
        }
        Ok(())
    }
    pub fn channel_key(&self) -> Option<ChannelKey> {
        match &self.owner {
            PerformanceOwner::Midi { key, .. } => Some(*key),
            _ => None,
        }
    }
    pub fn channel(&self) -> Option<&ChannelPerformance> {
        match &self.owner {
            PerformanceOwner::Midi { timeline, .. } => Some(timeline),
            _ => None,
        }
    }
}

impl PerformanceIndex {
    /// Production continuity adoption shares normalization's cumulative budget.
    pub(crate) fn continue_tied_intensity(
        &self,
        tail: &mut PerformanceNote,
        head: &PerformanceNote,
    ) -> Result<(), String> {
        tail.continue_tied_intensity_bounded(head, &mut self.provenance_budget.borrow_mut())
    }
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
    pub intensity: Option<serde_json::Value>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intensity: Option<serde_json::Value>,
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

/// Bind authored score fields and native MIDI attacks through one neutral
/// contributor. Synthetic score-loader velocities never become attack evidence.
fn bind_intensity(
    midi: &Midi,
    index: &mut PerformanceIndex,
    excluded_attacks: &BTreeSet<(String, u32)>,
    provenance_budget: &mut super::score_intensity::ProvenanceBudget,
) -> Result<(), String> {
    use super::score_intensity::{self as si, source::time};
    let ppq = midi.ticks_per_beat;
    let mut timelines = BTreeMap::<si::ScoreVoice, Arc<si::Timeline>>::new();
    provenance_budget.charge(0, midi.tracks.iter().map(|t| t.events.len()).sum())?;
    let mut empty_channels = BTreeMap::<ChannelKey, Arc<ChannelPerformance>>::new();
    let mut all = Vec::new();
    for track in &midi.tracks {
        for note in intensity_notes(track, provenance_budget)? {
            all.push((track, note));
        }
    }
    // Keep legitimate, source-normalized contexts for both ends of typed score
    // ties, even when one end has no authored contributor. Selected-row recovery
    // decides later whether to inherit; a text-bearing genuine attack stays its
    // own attack. No binding is invented by copying a predecessor's payload.
    let mut tie_participants = BTreeSet::new();
    if midi.score_intensity.is_some() {
        for (_, note) in &all {
            if let Some(tie) = note
                .source
                .continuity
                .as_ref()
                .and_then(|c| c.incoming_tie.as_ref())
            {
                for reference in [&tie.head, &tie.tail] {
                    provenance_budget.charge(reference.source_id.len().saturating_add(96), 1)?;
                    tie_participants.insert((
                        reference.source_id.clone(),
                        reference.occurrence,
                        reference.playback_segment,
                    ));
                }
            }
        }
    }
    let mut attacks = BTreeMap::<si::ScoreVoice, Vec<si::Time>>::new();
    let mut ends = BTreeMap::<si::ScoreVoice, Vec<si::Time>>::new();
    let owners = if midi.score_intensity.is_some() {
        discover_score_owners(midi, index, provenance_budget)?
    } else {
        BTreeMap::new()
    };
    for owner in owners.keys() {
        provenance_budget.charge(
            score_voice_bytes(owner)
                .saturating_mul(2)
                .saturating_add(128),
            2,
        )?;
        attacks.entry(owner.clone()).or_default();
        ends.entry(owner.clone()).or_default();
    }
    for (track, note) in &all {
        provenance_budget.charge(track.id.len().saturating_add(256), 1)?;
        if midi.score_intensity.is_some() {
            attacks
                .entry(source_voice(note.source, provenance_budget)?)
                .or_default();
            ends.entry(source_voice(note.source, provenance_budget)?)
                .or_default();
        }
        if note.pitch.is_some() {
            if !excluded_attacks.contains(&(track.id.clone(), note.source_order)) {
                attacks
                    .entry(source_voice(note.source, provenance_budget)?)
                    .or_default()
                    .push(time(i64::from(note.onset), ppq)?);
            }
            // The voice remains present even if all its retained notes are tails.
            attacks
                .entry(source_voice(note.source, provenance_budget)?)
                .or_default();
            ends.entry(source_voice(note.source, provenance_budget)?)
                .or_default()
                .push(time(i64::from(note.onset) + i64::from(note.duration), ppq)?);
        }
    }
    for values in attacks.values_mut() {
        values.sort();
        values.dedup();
    }
    for values in ends.values_mut() {
        values.sort();
        values.dedup();
    }
    let mut continuations = BTreeMap::<(&str, u32), Vec<(u32, &si::VelocityEvidence)>>::new();
    if let Some(input) = &midi.score_intensity {
        let mut unresolved_declarations = BTreeSet::new();
        for ((tail, pass, tick), head) in &input.ties {
            if let Some(velocity) = input.overrides.get(tail) {
                provenance_budget.charge(128, 1)?;
                continuations
                    .entry((head.as_str(), *pass))
                    .or_default()
                    .push((*tick, velocity));
            }
        }
        for owner in attacks.keys() {
            provenance_budget.charge(
                score_voice_bytes(owner)
                    .saturating_mul(2)
                    .saturating_add(128),
                1,
            )?;
            provenance_budget.charge(
                owner
                    .part
                    .len()
                    .saturating_add(owner.staff.len())
                    .saturating_add(64),
                1,
            )?;
            let runs = input
                .runs
                .get(&(owner.part.clone(), owner.staff.clone()))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let ambiguous = ambiguous_declarations(input, owner, provenance_budget)?;
            provenance_budget.charge(0, input.score.occurrence_work(owner, runs)?)?;
            provenance_budget.charge(
                runs.len().saturating_mul(std::mem::size_of::<si::Time>()),
                runs.len(),
            )?;
            let mut sounding = Vec::new();
            for run in runs {
                let end = run
                    .performed_start
                    .checked_add(run.written_end.checked_sub(run.written_start)?)?;
                let voice_ends = &ends[owner];
                let last = voice_ends
                    .partition_point(|t| *t <= end)
                    .checked_sub(1)
                    .map(|i| voice_ends[i])
                    .filter(|t| *t > run.performed_start)
                    .unwrap_or(run.performed_start);
                sounding.push(
                    last.checked_sub(run.performed_start)?
                        .checked_add(run.written_start)?,
                );
            }
            let mut timeline =
                input.occurrences_bounded(owner, runs, &sounding, provenance_budget)?;
            provenance_budget.charge(0, input.issues.len().saturating_mul(runs.len()))?;
            for issue in &input.issues {
                provenance_budget.charge(
                    0,
                    issue
                        .provenance
                        .evidence
                        .iter()
                        .map(|e| e.source_ids.len())
                        .sum(),
                )?;
                if issue
                    .provenance
                    .evidence
                    .iter()
                    .flat_map(|e| &e.source_ids)
                    .any(|id| ambiguous.contains(id))
                {
                    continue;
                }
                let pass_work = issue
                    .provenance
                    .evidence
                    .iter()
                    .flat_map(|e| &e.source_ids)
                    .fold(0usize, |work, id| {
                        work.saturating_add(1).saturating_add(
                            input.issue_passes.get(id).map_or(0, |passes| passes.len()),
                        )
                    });
                provenance_budget.charge(0, pass_work.saturating_mul(runs.len()))?;
                let applies = issue.provenance.scope.applies(owner)
                    || matches!(&issue.provenance.scope,
                    si::Scope::Unsupported { part, .. } if part == &owner.part);
                if !applies {
                    continue;
                }
                for (i, run) in runs.iter().enumerate() {
                    let ids = issue.provenance.evidence.iter().flat_map(|e| &e.source_ids);
                    provenance_budget.charge(
                        0,
                        ids.clone().fold(0usize, |work, id| {
                            work.saturating_add(runs.len().saturating_mul(3).saturating_add(16))
                                .saturating_add(
                                    input.declarations.get(id).map_or(0, |d| d.time_only.len()),
                                )
                        }),
                    )?;
                    // A ranged parser diagnostic still belongs to its original
                    // declaration. An overlapping range cannot prove that the
                    // measure which authored it was visited on this route.
                    if !ids
                        .filter(|id| input.declarations.contains_key(*id))
                        .all(|id| input.declaration_on_route(id, owner, run.ordinal, run.pass))
                    {
                        continue;
                    }
                    // A pass-filter diagnostic describes the excluded pass;
                    // applying that same time-only filter would erase it. Only
                    // a parser-pinned nonzero pass can bypass field membership.
                    let excluded_pass = issue.kind == si::IssueKind::PassFiltered
                        && issue.provenance.repeat_pass != 0;
                    let enabled = (issue.provenance.repeat_pass == 0
                        || issue.provenance.repeat_pass == run.pass)
                        && (excluded_pass
                            || issue
                                .provenance
                                .evidence
                                .iter()
                                .flat_map(|e| &e.source_ids)
                                .filter_map(|id| input.issue_passes.get(id))
                                .all(|passes| passes.contains(&run.pass)));
                    if enabled {
                        if !si::intersects(
                            issue.start,
                            issue.end,
                            run.written_start,
                            run.written_end,
                            i + 1 == runs.len(),
                        ) {
                            continue;
                        }
                        provenance_budget.reserve_issue(issue)?;
                        if let Some(mapped) = si::performed_issue(issue, run, i + 1 == runs.len())?
                        {
                            Arc::get_mut(&mut timeline)
                                .ok_or("Score timeline was shared before parser issue replay")?
                                .issues
                                .push(mapped);
                        }
                    }
                }
            }
            // Written-only diagnostics must never enter a sounding note's
            // timeline, even when their written coordinate overlaps that note.
            let mut written_timeline = Arc::new(si::Timeline::default());
            unresolved_declaration_issues(
                input,
                &ambiguous,
                &mut written_timeline,
                &mut unresolved_declarations,
                provenance_budget,
            )?;
            if !written_timeline.issues.is_empty() {
                let track_id = owners
                    .get(owner)
                    .ok_or("Unresolved declaration has no typed source lane")?;
                provenance_budget.charge(
                    track_id
                        .len()
                        .saturating_add(score_voice_bytes(owner))
                        .saturating_add(128),
                    1,
                )?;
                index.score_issues.push(ScoreIssueOwner {
                    track_id: track_id.clone(),
                    voice: owner.clone(),
                    timeline: written_timeline,
                    terminal: None,
                });
            }
            if ends[owner].is_empty() {
                silent_owner_issues(&mut timeline, provenance_budget)?;
            }
            timelines.insert(owner.clone(), timeline);
        }
        // Locate retained fields in their actual source owner and performed
        // occurrence. Do not attribute another part's declarations to track zero.
        let mut locations = BTreeMap::<String, (String, u32)>::new();
        for (track, note) in &all {
            if let Some(velocity) = input.overrides.get(&note.source.id) {
                for id in &velocity.evidence.source_ids {
                    provenance_budget.charge(
                        id.len().saturating_add(track.id.len()).saturating_add(128),
                        1,
                    )?;
                    locations
                        .entry(id.clone())
                        .or_insert((track.id.clone(), note.onset));
                }
            }
            // A nominally merged tie tail may no longer have a NoteOn, but its
            // original velocity field still owns its explicit source tick.
            for (tick, velocity) in continuations
                .get(&(note.source.id.as_str(), note.source.occurrence))
                .into_iter()
                .flatten()
                .filter(|(tick, _)| {
                    *tick >= note.onset
                        && u64::from(*tick) < u64::from(note.onset) + u64::from(note.duration)
                })
            {
                for id in &velocity.evidence.source_ids {
                    provenance_budget.charge(
                        id.len().saturating_add(track.id.len()).saturating_add(128),
                        1,
                    )?;
                    locations
                        .entry(id.clone())
                        .or_insert((track.id.clone(), *tick));
                }
            }
        }
        for (id, declaration) in &input.declarations {
            provenance_budget.charge(0, owners.len())?;
            for (owner, track_id) in &owners {
                let applicable = declaration.scope.applies(owner)
                    || matches!(&declaration.scope,
                    si::Scope::Unsupported { part, .. } if part == &owner.part);
                if !applicable {
                    continue;
                }
                provenance_budget.charge(
                    owner
                        .part
                        .len()
                        .saturating_add(owner.staff.len())
                        .saturating_add(64),
                    1,
                )?;
                let runs = input.runs.get(&(owner.part.clone(), owner.staff.clone()));
                provenance_budget.charge(
                    0,
                    runs.map_or(0, Vec::len)
                        .saturating_mul(1 + declaration.time_only.len()),
                )?;
                let at = runs
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .find(|(i, r)| {
                        !unresolved_declarations.contains(id)
                            && r.written_start <= declaration.at
                            && (declaration.at < r.written_end
                                || (*i + 1 == runs.map_or(0, Vec::len)
                                    && declaration.at == r.written_end))
                            && (declaration.time_only.is_empty()
                                || declaration.time_only.contains(&r.pass))
                    })
                    .map(|(_, r)| {
                        r.performed_start
                            .checked_add(declaration.at.checked_sub(r.written_start)?)
                    })
                    .transpose()?
                    .unwrap_or(declaration.at);
                let tick = exact_source_tick_bounds(at, at, ppq)?.0;
                provenance_budget.charge(
                    id.len().saturating_add(track_id.len()).saturating_add(128),
                    1,
                )?;
                locations
                    .entry(id.clone())
                    .or_insert((track_id.clone(), tick));
                break;
            }
        }
        // Unmatched wedges and disabled/unrecognized declarations can exist
        // only as issues. Their performed positions are still source-owned,
        // including points in rests with no editable note attribution.
        for (owner, timeline) in &timelines {
            let Some(track_id) = owners.get(owner) else {
                continue;
            };
            if !timeline.issues.is_empty() {
                provenance_budget.charge(
                    track_id
                        .len()
                        .saturating_add(score_voice_bytes(owner))
                        .saturating_add(128),
                    1,
                )?;
                index.score_issues.push(ScoreIssueOwner {
                    track_id: track_id.clone(),
                    voice: owner.clone(),
                    timeline: Arc::clone(timeline),
                    terminal: input
                        .runs
                        .get(&(owner.part.clone(), owner.staff.clone()))
                        .and_then(|runs| runs.last())
                        .map(|run| {
                            run.performed_start
                                .checked_add(run.written_end.checked_sub(run.written_start)?)
                        })
                        .transpose()?,
                });
            }
            for issue in &timeline.issues {
                let tick = exact_source_tick_bounds(issue.start, issue.start, ppq)?.0;
                for id in issue.provenance.evidence.iter().flat_map(|e| &e.source_ids) {
                    provenance_budget.charge(
                        id.len().saturating_add(track_id.len()).saturating_add(128),
                        1,
                    )?;
                    locations
                        .entry(id.clone())
                        .or_insert((track_id.clone(), tick));
                }
            }
        }
        for id in input.retained.keys() {
            provenance_budget.charge(
                id.len()
                    .saturating_add(locations.get(id).map_or(0, |(track, _)| track.len()))
                    .saturating_add(128),
                1,
            )?;
            let (track_id, tick) = locations.get(id).cloned().ok_or(
                "SCORE_INTENSITY_OWNER_MISSING: retained field has no typed source lane and coordinate",
            )?;
            index.events.insert(
                id.clone(),
                PerformanceEvent {
                    track_id,
                    tick,
                    dimension: Dimension::LinearGain,
                },
            );
        }
    }
    for track in &midi.tracks {
        for event in &track.events {
            if matches!(event.kind, Kind::NoteOn(_)) {
                provenance_budget.charge(track.id.len().saturating_add(128), 1)?;
            }
        }
    }
    let event_notes: BTreeMap<_, _> = midi
        .tracks
        .iter()
        .flat_map(|track| {
            track
                .events
                .iter()
                .filter_map(move |event| match &event.kind {
                    Kind::NoteOn(note) => Some(((track.id.clone(), event.order), note)),
                    _ => None,
                })
        })
        .collect();
    for (track, note) in &all {
        if note.pitch.is_none() || note.duration == 0 {
            continue;
        }
        provenance_budget.charge(
            track
                .id
                .len()
                .saturating_mul(3)
                .saturating_add(note.source.id.len().saturating_mul(3))
                .saturating_add(512),
            1,
        )?;
        let lookup = (track.id.clone(), note.source_order);
        let id = super::convert::note_instance_id(&track.id, note.source, note.source_order);
        let start = time(i64::from(note.onset), ppq)?;
        let end = time(i64::from(note.onset) + i64::from(note.duration), ppq)?;
        let binding = if let Some(key) = index.notes.get(&lookup).copied() {
            let velocity = event_notes
                .get(&lookup)
                .and_then(|n| n.velocity)
                .filter(|v| *v > 0);
            let intensity = velocity
                .map(|velocity| {
                    si::NoteIntensity::midi_attack(
                        id.clone(),
                        start,
                        end,
                        velocity,
                        key.port,
                        key.channel,
                        format!("event:{}:{}", track.id, note.source_order),
                    )
                })
                .transpose()?
                .map(Arc::new);
            if intensity.is_some() {
                index
                    .events
                    .entry(attack_field_id(&track.id, note.source_order))
                    .or_insert(PerformanceEvent {
                        track_id: track.id.clone(),
                        tick: note.onset,
                        dimension: Dimension::LinearGain,
                    });
            }
            let timeline = index
                .channels
                .get(&key)
                .cloned()
                .unwrap_or_else(|| empty_channels.entry(key).or_default().clone());
            Some(PerformanceNote {
                source_id: id,
                owner: PerformanceOwner::Midi { key, timeline },
                intensity,
            })
        } else if let Some(input) = &midi.score_intensity {
            let owner = source_voice(note.source, provenance_budget)?;
            let timeline = timelines.get(&owner).cloned().unwrap_or_default();
            let velocity = input
                .overrides
                .get(&note.source.id)
                .map(|velocity| {
                    provenance_budget.reserve_velocity(velocity)?;
                    Ok::<_, String>(velocity.clone())
                })
                .transpose()?;
            let continuation_velocities = continuations
                .get(&(note.source.id.as_str(), note.source.occurrence))
                .into_iter()
                .flatten()
                .filter(|(tick, _)| {
                    *tick >= note.onset
                        && u64::from(*tick) < u64::from(note.onset) + u64::from(note.duration)
                })
                .map(|(_, velocity)| {
                    provenance_budget.reserve_velocity(velocity)?;
                    Ok::<_, String>((*velocity).clone())
                })
                .collect::<Result<Vec<_>, _>>()?;
            provenance_budget.charge(
                owner
                    .part
                    .len()
                    .saturating_add(owner.staff.len())
                    .saturating_add(64),
                1,
            )?;
            let runs = input
                .runs
                .get(&(owner.part.clone(), owner.staff.clone()))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let run = runs
                .partition_point(|r| r.performed_start <= start)
                .checked_sub(1)
                .map(|i| &runs[i]);
            let occurrence = run.map_or(0, |r| r.ordinal);
            let pass = run.map_or(note.source.occurrence + 1, |r| r.pass);
            provenance_budget.charge(score_voice_bytes(&owner).saturating_add(128), 1)?;
            let chain = si::NoteChain {
                source_id: id.clone(),
                start,
                end,
                owner: owner.clone(),
                occurrence,
                pass,
                velocity,
                continuation_velocities,
            };
            let source_attacks = attacks.get(&owner).map(Vec::as_slice).unwrap_or(&[]);
            let accent_count = timeline.accent_range(source_attacks, start).len();
            let first = timeline.segments.partition_point(|s| s.end <= start);
            let last = timeline.segments.partition_point(|s| s.start <= end);
            let logarithmic = |n: usize| usize::BITS as usize - n.leading_zeros() as usize;
            let work = 4
                + logarithmic(source_attacks.len())
                + logarithmic(timeline.accents.len())
                + accent_count.saturating_mul(2)
                + last.saturating_sub(first).saturating_mul(5)
                + timeline.issues.len();
            provenance_budget.charge(0, work)?;
            let intensity = si::NoteIntensity::resolve_bounded(
                &chain,
                timeline,
                source_attacks,
                provenance_budget,
            )?;
            provenance_budget.charge(note.source.id.len().saturating_add(64), 1)?;
            let tie_participant = note.source.continuity.as_ref().is_some_and(|c| {
                tie_participants.contains(&(
                    note.source.id.clone(),
                    note.source.occurrence,
                    c.playback_segment,
                ))
            });
            let active = tie_participant
                || intensity.provenance.is_some()
                || !intensity.issues.is_empty()
                || intensity.timeline.segments[first..last].iter().any(|s| {
                    si::intersects(s.start, s.end, start, end, true)
                        && !matches!(s.curve, si::Curve::Absent)
                })
                || intensity
                    .timeline
                    .issues
                    .iter()
                    .any(|i| si::intersects(i.start, i.end, start, end, true));
            active.then(|| PerformanceNote {
                source_id: id,
                owner: PerformanceOwner::Score { voice: owner },
                intensity: Some(Arc::new(intensity)),
            })
        } else {
            None
        };
        if let Some(binding) = binding {
            index.bindings.insert(lookup, binding);
        }
    }
    Ok(())
}

fn score_voice_bytes(voice: &super::score_intensity::ScoreVoice) -> usize {
    voice
        .part
        .len()
        .saturating_add(voice.staff.len())
        .saturating_add(voice.voice.len())
        .saturating_add(voice.instrument.as_ref().map_or(0, String::len))
}

fn ambiguous_declarations<'a>(
    input: &'a super::score_intensity::source::ScoreInput,
    owner: &super::score_intensity::ScoreVoice,
    budget: &mut super::score_intensity::ProvenanceBudget,
) -> Result<BTreeSet<&'a String>, String> {
    use super::score_intensity as si;
    budget.charge(0, input.declarations.len())?;
    let mut ambiguous = BTreeSet::new();
    for (id, declaration) in &input.declarations {
        if !declaration.scope.applies(owner)
            && !matches!(&declaration.scope, si::Scope::Unsupported { part, .. } if part == &owner.part)
        {
            continue;
        }
        let depth = |n: usize| usize::BITS as usize - n.leading_zeros() as usize + 1;
        let comparisons = depth(input.original_declarations.len())
            .saturating_add(depth(input.runs.len()).saturating_mul(3));
        budget.charge(
            0,
            comparisons
                .saturating_mul(1 + id.len() / 64 + owner.part.len() / 64 + owner.staff.len() / 64),
        )?;
        // Parser-owned written membership survives playback offsets and skipped
        // endings. Equal playback coordinates never establish measure ownership.
        if input.declaration_is_unresolved(id, owner) {
            budget.charge(64, 1)?;
            ambiguous.insert(id);
        }
    }
    Ok(ambiguous)
}

/// Without an unambiguous route, retain written source coordinates explicitly.
/// Original written measure membership identifies declarations whose performed
/// route cannot be proved, including velocity fields in unplayed endings.
fn unresolved_declaration_issues(
    input: &super::score_intensity::source::ScoreInput,
    ambiguous: &BTreeSet<&String>,
    timeline: &mut Arc<super::score_intensity::Timeline>,
    reported: &mut BTreeSet<String>,
    budget: &mut super::score_intensity::ProvenanceBudget,
) -> Result<(), String> {
    use super::score_intensity as si;
    budget.charge(0, ambiguous.len())?;
    for &id in ambiguous {
        if reported.contains(id) {
            continue;
        }
        let declaration = &input.declarations[id];
        let evidence = input
            .retained
            .get(id)
            .ok_or("Score declaration has no retained evidence")?;
        // Preflight borrowed JSON and structural allocation before any clones.
        budget.charge(
            1024usize
                .saturating_add(evidence.raw_fields.len().saturating_mul(128))
                .saturating_add(evidence.source_ids.len().saturating_mul(128))
                .saturating_add(id.len().saturating_mul(2)),
            1,
        )?;
        reserve_score_json(
            &(&declaration.scope, evidence, &declaration.time_only),
            budget,
        )?;
        let message = "Source intensity declaration retained at its written position; performed scope is unresolved because no unambiguous source route identifies its occurrence";
        reported.insert(id.clone());
        Arc::get_mut(timeline).ok_or("Score timeline was shared before declaration diagnostics")?.issues.push(si::Issue {
            start: declaration.at,
            end: declaration.at,
            kind: si::IssueKind::UnresolvedSpan,
            provenance: si::Provenance {
                policy: si::POLICY,
                scope: declaration.scope.clone(),
                occurrence: 0,
                repeat_pass: 0,
                evidence: vec![evidence.clone()],
                interpretations: vec![si::Interpretation {
                    field: "performed-scope".into(),
                    source_ids: vec![id.clone()],
                    basis: si::Basis::PortableInterpretation,
                    exact: Some(declaration.at),
                    explanation: format!("{message}; occurrence/pass 0 denote unresolved ownership. Source order {}, enabled {}, time-only {:?} remain declaration metadata.", declaration.order, declaration.enabled, declaration.time_only),
                }],
            },
            message: message.into(),
        });
    }
    Ok(())
}

fn reserve_score_json<T: serde::Serialize>(
    value: &T,
    budget: &mut super::score_intensity::ProvenanceBudget,
) -> Result<(), String> {
    struct Counter<'a>(&'a mut super::score_intensity::ProvenanceBudget);
    impl std::io::Write for Counter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .charge(bytes.len().saturating_mul(2), bytes.len() / 8 + 1)
                .map_err(std::io::Error::other)?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(budget), value).map_err(|error| error.to_string())
}

/// A declared silent voice has a performed source scope, but cannot own an
/// editable-note claim. Keep its resolved provenance and passes explicitly.
fn silent_owner_issues(
    timeline: &mut Arc<super::score_intensity::Timeline>,
    budget: &mut super::score_intensity::ProvenanceBudget,
) -> Result<(), String> {
    use super::score_intensity as si;
    let timeline = Arc::get_mut(timeline)
        .ok_or("Score timeline was shared before silent-owner diagnostics")?;
    budget.charge(
        0,
        timeline
            .segments
            .len()
            .saturating_add(timeline.accents.len()),
    )?;
    let declarations = timeline
        .segments
        .iter()
        .filter(|s| !matches!(s.curve, si::Curve::Absent | si::Curve::Held(None)))
        .filter_map(|s| s.provenance.as_ref().map(|p| (s.start, s.end, p)))
        .chain(timeline.accents.iter().map(|a| (a.at, a.at, &a.provenance)));
    for (start, end, provenance) in declarations {
        budget.charge(
            1024usize
                .saturating_add(provenance.evidence.len().saturating_mul(128))
                .saturating_add(provenance.interpretations.len().saturating_mul(128)),
            provenance
                .evidence
                .len()
                .saturating_add(provenance.interpretations.len()),
        )?;
        for evidence in &provenance.evidence {
            budget.charge(
                evidence
                    .raw_fields
                    .len()
                    .saturating_mul(128)
                    .saturating_add(evidence.source_ids.len().saturating_mul(128)),
                1,
            )?;
        }
        for interpretation in &provenance.interpretations {
            budget.charge(interpretation.source_ids.len().saturating_mul(128), 1)?;
        }
        reserve_score_json(provenance, budget)?;
        timeline.issues.push(si::Issue {
            start, end, kind: si::IssueKind::UnresolvedSpan,
            provenance: provenance.clone(),
            message: "Source intensity retained for a declared voice with no sounding notes; no editable target span is claimed".into(),
        });
    }
    Ok(())
}

/// Inventory original owners independently of sounding-note extraction. A
/// parser-owned staff metadata lane can retain explicit topology voices without
/// creating projection voices or manufacturing note identities.
fn discover_score_owners(
    midi: &Midi,
    index: &mut PerformanceIndex,
    budget: &mut super::score_intensity::ProvenanceBudget,
) -> Result<BTreeMap<super::score_intensity::ScoreVoice, String>, String> {
    use super::score_intensity::ScoreVoice;
    fn insert(
        index: &mut PerformanceIndex,
        track: &super::midi::Track,
        owner: ScoreVoice,
        budget: &mut super::score_intensity::ProvenanceBudget,
    ) -> Result<(), String> {
        budget.charge(
            track
                .id
                .len()
                .saturating_add(score_voice_bytes(&owner))
                .saturating_add(128),
            1,
        )?;
        index
            .score_track_owners
            .entry(track.id.clone())
            .or_default()
            .insert(owner);
        Ok(())
    }
    budget.charge(0, midi.tracks.len())?;
    let mut metadata = BTreeMap::<(&str, &str), Vec<&super::midi::Track>>::new();
    let mut by_id = BTreeMap::new();
    for track in &midi.tracks {
        budget.charge(128, 1)?;
        by_id.insert(track.id.as_str(), track);
        budget.charge(0, track.events.len())?;
        for event in &track.events {
            if let Kind::NoteOn(note) = &event.kind {
                let owner = source_voice(&note.source, budget)?;
                insert(index, track, owner, budget)?;
            }
        }
        let (Some(part), Some(staff)) = (&track.source.part_id, &track.source.staff_id) else {
            continue;
        };
        if let Some(voice) = &track.source.voice {
            if !index.score_track_owners.contains_key(&track.id) {
                budget.charge(
                    part.len()
                        .saturating_add(staff.len())
                        .saturating_add(voice.len())
                        .saturating_add(
                            track
                                .instrument
                                .as_ref()
                                .and_then(|i| i.id.as_ref())
                                .map_or(0, String::len),
                        ),
                    1,
                )?;
                insert(
                    index,
                    track,
                    ScoreVoice {
                        part: part.clone(),
                        staff: staff.clone(),
                        voice: voice.clone(),
                        instrument: track.instrument.as_ref().and_then(|i| i.id.clone()),
                    },
                    budget,
                )?;
            }
        } else {
            budget.charge(64, 1)?;
            metadata
                .entry((part.as_str(), staff.as_str()))
                .or_default()
                .push(track);
        }
    }
    for part in &midi.topology.parts {
        budget.charge(0, 1 + part.staves.len())?;
        for staff in &part.staves {
            budget.charge(0, staff.voices.len())?;
            // Empty means unspecified source voice, not a new editable voice.
            let voices: Vec<_> = if staff.voices.is_empty() {
                budget.charge(32, 1)?;
                vec![("", &[][..])]
            } else {
                budget.charge(staff.voices.len().saturating_mul(32), staff.voices.len())?;
                staff
                    .voices
                    .iter()
                    .map(|voice| (voice.number.as_str(), voice.projection_track_ids.as_slice()))
                    .collect()
            };
            for (voice, lanes) in voices {
                budget.charge(0, lanes.len())?;
                let declared = lanes
                    .iter()
                    .filter_map(|id| by_id.get(id.as_str()).copied());
                let staff_lanes = metadata
                    .get(&(part.id.as_str(), staff.id.as_str()))
                    .into_iter()
                    .flatten()
                    .copied();
                for track in declared.chain(staff_lanes) {
                    budget.charge(0, 1)?;
                    let existing = index.score_track_owners.get(&track.id);
                    budget.charge(0, existing.map_or(0, BTreeSet::len))?;
                    if existing.is_some_and(|owners| {
                        owners
                            .iter()
                            .any(|o| o.part == part.id && o.staff == staff.id && o.voice == voice)
                    }) {
                        continue;
                    }
                    budget.charge(
                        part.id
                            .len()
                            .saturating_add(staff.id.len())
                            .saturating_add(voice.len())
                            .saturating_add(
                                track
                                    .instrument
                                    .as_ref()
                                    .and_then(|i| i.id.as_ref())
                                    .map_or(0, String::len),
                            ),
                        1,
                    )?;
                    insert(
                        index,
                        track,
                        ScoreVoice {
                            part: part.id.clone(),
                            staff: staff.id.clone(),
                            voice: voice.to_string(),
                            instrument: track.instrument.as_ref().and_then(|i| i.id.clone()),
                        },
                        budget,
                    )?;
                }
            }
        }
    }
    let mut owners = BTreeMap::new();
    for track in &midi.tracks {
        for owner in index
            .score_track_owners
            .get(&track.id)
            .into_iter()
            .flatten()
        {
            budget.charge(
                score_voice_bytes(owner)
                    .saturating_add(track.id.len())
                    .saturating_add(128),
                1,
            )?;
            owners
                .entry(owner.clone())
                .or_insert_with(|| track.id.clone());
        }
    }
    Ok(owners)
}

/// Raw normalization cannot decide which repeat/verse notes are actual attacks.
/// Rebind only accent-bearing score contexts after the continuity planner has
/// proven their selected tails. Different selected lyric groups stay separate.
pub(crate) fn bind_selected_score_attacks(
    midi: &Midi,
    raw: &PerformanceIndex,
    pending: &mut [(usize, Vec<String>, super::projection::ProjectedTrack)],
) -> Result<(), String> {
    if midi.score_intensity.is_none()
        || !raw.bindings.values().any(|p| {
            p.intensity
                .as_ref()
                .is_some_and(|i| !i.timeline.accents.is_empty())
        })
    {
        return Ok(());
    }
    let mut provenance_budget = raw.provenance_budget.borrow_mut();
    let mut excluded = BTreeMap::<Vec<String>, BTreeSet<(String, u32)>>::new();
    for (_, group, track) in pending.iter() {
        for note in &track.notes {
            let Some(binding) = &note.performance else {
                continue;
            };
            if binding.channel_key().is_some()
                || binding
                    .intensity
                    .as_ref()
                    .is_none_or(|i| i.source_id == binding.source_id)
            {
                continue;
            }
            let origin = note
                .source_evidence
                .as_ref()
                .and_then(|e| e.origin.as_ref())
                .ok_or("Selected tie has no original source ownership")?;
            if origin.continuation.is_none() {
                return Err("Selected tie attack context lacks its proven predecessor".into());
            }
            for id in group {
                provenance_budget.charge(id.len().saturating_add(32), 1)?;
            }
            provenance_budget.charge(origin.track_id.len().saturating_add(128), 1)?;
            excluded
                .entry(group.clone())
                .or_default()
                .insert((origin.track_id.clone(), origin.note_on_order));
        }
    }
    let source_work = midi.tracks.iter().map(|t| t.events.len()).sum::<usize>();
    if source_work.saturating_mul(excluded.len()) > super::score_intensity::MAX_RESOLUTION_WORK {
        return Err("SCORE_INTENSITY_LIMIT: selected attack contexts exceed bounded work".into());
    }
    for (group, excluded) in excluded {
        // This branch is score-only. Rebuild selected bindings without cloning
        // the raw event/binding maps or their already shared immutable timelines.
        let mut selected = PerformanceIndex::default();
        bind_intensity(midi, &mut selected, &excluded, &mut provenance_budget)?;
        for (_, selected_group, track) in pending.iter_mut().filter(|(_, g, _)| *g == group) {
            debug_assert_eq!(*selected_group, group);
            let mut completed = BTreeMap::<String, PerformanceNote>::new();
            for note in &mut track.notes {
                let Some(origin) = note
                    .source_evidence
                    .as_ref()
                    .and_then(|e| e.origin.as_ref())
                else {
                    continue;
                };
                if note
                    .performance
                    .as_ref()
                    .is_some_and(|old| old.channel_key().is_some())
                {
                    continue;
                }
                let inherited = note.performance.as_ref().is_some_and(|old| {
                    old.intensity
                        .as_ref()
                        .is_some_and(|i| i.source_id != old.source_id)
                });
                provenance_budget.charge(origin.track_id.len().saturating_add(32), 1)?;
                let key = (origin.track_id.clone(), origin.note_on_order);
                let Some(binding) = selected.bindings.get(&key) else {
                    continue;
                };
                reserve_binding(binding, &mut provenance_budget)?;
                let mut updated = binding.clone();
                if inherited {
                    let link = origin
                        .continuation
                        .as_ref()
                        .ok_or("Selected tie is missing its predecessor")?;
                    let head = completed
                        .get(&link.predecessor_id)
                        .ok_or("Selected tie no longer follows its resolved attack context")?;
                    updated.continue_tied_intensity_bounded(head, &mut provenance_budget)?;
                }
                reserve_binding(&updated, &mut provenance_budget)?;
                provenance_budget.charge(updated.source_id.len().saturating_add(64), 1)?;
                completed.insert(updated.source_id.clone(), updated.clone());
                note.performance = Some(updated);
            }
        }
    }
    Ok(())
}

pub fn normalize(midi: &Midi) -> Result<PerformanceIndex, String> {
    // Match the parser's event ceiling after port-wide hazard expansion too.
    let mut index = normalize_bounded(midi, 2_000_000)?;
    let mut budget = super::score_intensity::ProvenanceBudget::default();
    bind_intensity(midi, &mut index, &BTreeSet::new(), &mut budget)?;
    index.provenance_budget = std::cell::RefCell::new(budget);
    Ok(index)
}

fn reserve_binding(
    binding: &PerformanceNote,
    budget: &mut super::score_intensity::ProvenanceBudget,
) -> Result<(), String> {
    let owner = match &binding.owner {
        PerformanceOwner::Score { voice } => score_voice_bytes(voice),
        PerformanceOwner::Midi { .. } => 0,
    };
    budget.charge(
        binding
            .source_id
            .len()
            .saturating_add(owner)
            .saturating_add(128),
        1,
    )
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
            score_intensity: None,
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

/// Performance needs geometry and borrowed source ownership, not copied lyrics
/// or continuity metadata from the nominal projection's note extractor.
struct IntensityNote<'a> {
    onset: u32,
    duration: u32,
    pitch: Option<u8>,
    source_order: u32,
    source: &'a super::midi::NoteSource,
}
fn intensity_notes<'a>(
    track: &'a super::midi::Track,
    budget: &mut super::score_intensity::ProvenanceBudget,
) -> Result<Vec<IntensityNote<'a>>, String> {
    budget.charge(0, track.events.len())?;
    let count = track
        .events
        .iter()
        .filter(|e| matches!(e.kind, Kind::NoteOn(_)))
        .count();
    // Active maps, FIFO entries, sorted output and the caller's reference list.
    budget.charge(count.saturating_mul(256), track.events.len())?;
    let mut active = BTreeMap::new();
    let mut queues = BTreeMap::<_, std::collections::VecDeque<(&str, usize)>>::new();
    let mut notes = Vec::new();
    for (generation, event) in track.events.iter().enumerate() {
        // BTree lookups compare borrowed IDs; no lyric/source payload is copied.
        // Exact offs leave tombstones, each visited at most once by anonymous
        // FIFO closure. A generation prevents an old tombstone closing a reused ID.
        let depth = usize::BITS as usize - active.len().leading_zeros() as usize + 1;
        let id_bytes = match &event.kind {
            Kind::NoteOn(note) => note.source.id.len(),
            Kind::NoteOff(note) => note.source_id.as_ref().map_or(0, String::len),
            _ => continue,
        };
        budget.charge(
            0,
            depth.saturating_mul(2 + id_bytes / 64).saturating_add(32),
        )?;
        let (key, exact_id) = match &event.kind {
            Kind::NoteOn(note) if note.velocity != Some(0) => {
                active.insert(
                    note.source.id.as_str(),
                    (generation, event.tick, event.order, note),
                );
                queues
                    .entry((note.channel, note.key))
                    .or_default()
                    .push_back((&note.source.id, generation));
                continue;
            }
            Kind::NoteOn(note) => ((note.channel, note.key), None),
            Kind::NoteOff(note) => ((note.channel, note.key), note.source_id.as_deref()),
            _ => continue,
        };
        let start = if let Some(id) = exact_id {
            active.remove(id)
        } else {
            let mut start = None;
            if let Some(queue) = queues.get_mut(&key) {
                while let Some((id, generation)) = queue.front().copied() {
                    budget.charge(0, depth.saturating_mul(2 + id.len() / 64))?;
                    queue.pop_front();
                    if active.get(id).is_some_and(|note| note.0 == generation) {
                        start = active.remove(id);
                        break;
                    }
                }
            }
            start
        };
        if let Some((_, onset, source_order, note)) = start {
            if let Some(duration) = event.tick.checked_sub(onset) {
                notes.push(IntensityNote {
                    onset,
                    duration,
                    source_order,
                    pitch: note.key,
                    source: &note.source,
                });
            }
        }
    }
    let depth = usize::BITS as usize - notes.len().leading_zeros() as usize;
    budget.charge(0, notes.len().saturating_mul(depth))?;
    notes.sort_by_key(|note| (note.onset, note.source_order, note.pitch));
    Ok(notes)
}

fn source_voice(
    source: &super::midi::NoteSource,
    budget: &mut super::score_intensity::ProvenanceBudget,
) -> Result<super::score_intensity::ScoreVoice, String> {
    let bytes = [
        &source.part_id,
        &source.staff_id,
        &source.voice,
        &source.instrument_id,
    ]
    .into_iter()
    .flatten()
    .fold(128usize, |bytes, text| bytes.saturating_add(text.len()));
    budget.charge(bytes, 1)?;
    Ok(super::score_intensity::ScoreVoice {
        part: source.part_id.clone().unwrap_or_default(),
        staff: source.staff_id.clone().unwrap_or_default(),
        voice: source.voice.clone().unwrap_or_default(),
        instrument: source.instrument_id.clone(),
    })
}

#[cfg(test)]
mod second_review_tests {
    use super::*;
    use crate::engine::score_intensity::{ProvenanceBudget, MAX_RESOLUTION_WORK};

    fn review5_ranged_issue_source(ending: u32) -> Midi {
        let sung = "<Chord><durationType>quarter</durationType><Lyrics><text>la</text></Lyrics><Note><pitch>60</pitch></Note></Chord>";
        let xml = format!(
            r#"<museScore version="4.70"><programVersion>4.6.5</programVersion><Score><Division>480</Division>
            <Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part><Staff id="1">
            <Measure len="1/4"><startRepeat/><voice>{sung}</voice></Measure>
            <Measure len="1/4"><Spanner type="Volta"><Volta><endings>{ending}</endings></Volta><next><location><measures>1</measures></location></next></Spanner><voice>
              <Spanner type="HairPin"><HairPin><subtype>0</subtype><ticks>1440</ticks><veloChange>20</veloChange></HairPin><next><location><measures>99</measures></location></next></Spanner>{sung}
            </voice><endRepeat>2</endRepeat></Measure>
            <Measure len="1/4"><Spanner type="Volta"><Volta><endings>2</endings></Volta><next><location><measures>1</measures></location></next></Spanner><voice>{sung}</voice></Measure>
            <Measure len="1/4"><voice>{sung}</voice></Measure>
            </Staff></Score></museScore>"#
        );
        crate::engine::musescore::parse(xml.as_bytes()).unwrap()
    }

    #[test]
    fn source_review5_ranged_parser_issue_does_not_replay_from_a_skipped_ending() {
        use crate::engine::score_intensity::{IssueKind, Time};
        let source = review5_ranged_issue_source(1);
        let before = source.tracks.clone();
        let input = source.score_intensity.as_ref().unwrap();
        let parser_issue = input
            .issues
            .iter()
            .find(|i| i.message.contains("outside the written score"))
            .unwrap();
        assert_eq!(
            (parser_issue.start, parser_issue.end),
            (Time::ONE, Time::integer(4))
        );
        let index = normalize(&source).unwrap();
        let replayed: BTreeSet<_> = index
            .bindings
            .values()
            .filter_map(|b| b.intensity.as_ref())
            .flat_map(|i| &i.timeline.issues)
            .filter(|i| i.kind == IssueKind::UnresolvedSpan && i.message == parser_issue.message)
            .map(|i| {
                (
                    i.start,
                    i.end,
                    i.provenance.occurrence,
                    i.provenance.repeat_pass,
                )
            })
            .collect();
        assert_eq!(
            replayed,
            BTreeSet::from([(Time::ONE, Time::integer(2), 1, 1)])
        );
        assert_eq!(
            source.tracks, before,
            "diagnostic filtering must preserve nominal notes and timing"
        );
    }

    #[test]
    fn source_review5_issue_replay_preserves_pass_filtered_and_unresolved_zero_ownership() {
        use crate::engine::score_intensity::{IssueKind, Time};
        let source = crate::engine::musicxml::parse(br#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes><barline location="left"><repeat direction="forward"/></barline><direction><direction-type><wedge type="crescendo"/></direction-type><sound time-only="2"/></direction><note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><lyric><text>la</text></lyric></note><barline location="right"><repeat direction="backward" times="2"/></barline></measure></part></score-partwise>"#).unwrap();
        let index = normalize(&source).unwrap();
        assert!(index
            .bindings
            .values()
            .filter_map(|b| b.intensity.as_ref())
            .flat_map(|i| &i.timeline.issues)
            .any(|i| i.kind == IssueKind::PassFiltered
                && i.start == Time::ZERO
                && i.provenance.occurrence == 1
                && i.provenance.repeat_pass == 1));

        let skipped = review5_ranged_issue_source(3);
        let input = skipped.score_intensity.as_ref().unwrap();
        let parser_issue = input
            .issues
            .iter()
            .find(|i| i.message.contains("outside the written score"))
            .unwrap();
        let id = &parser_issue.provenance.evidence[0].source_ids[0];
        let index = normalize(&skipped).unwrap();
        assert!(index
            .score_issues
            .iter()
            .flat_map(|owner| &owner.timeline.issues)
            .any(|i| {
                i.provenance.occurrence == 0
                    && i.provenance.repeat_pass == 0
                    && i.provenance
                        .evidence
                        .iter()
                        .any(|e| e.source_ids.contains(id))
            }));
        assert!(index
            .bindings
            .values()
            .filter_map(|b| b.intensity.as_ref())
            .flat_map(|i| &i.timeline.issues)
            .all(|i| !i
                .provenance
                .evidence
                .iter()
                .any(|e| e.source_ids.contains(id))));
    }

    #[test]
    fn parser_only_issue_replay_refuses_before_copy_under_small_budget() {
        let mut source = crate::engine::musicxml::parse(br#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes><direction><direction-type><wedge type="stop"/></direction-type></direction><note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><lyric><text>la</text></lyric></note></measure></part></score-partwise>"#).unwrap();
        source.score_intensity.as_mut().unwrap().issues[0]
            .provenance
            .evidence[0]
            .raw_fields
            .insert("large-evidence".into(), "x".repeat(128 * 1024));
        let before = source.tracks.clone();
        let mut index = PerformanceIndex::default();
        let error = bind_intensity(
            &source,
            &mut index,
            &BTreeSet::new(),
            &mut ProvenanceBudget::with_limits(64 * 1024, MAX_RESOLUTION_WORK),
        )
        .unwrap_err();
        assert!(error.contains("SCORE_INTENSITY_LIMIT"), "{error}");
        assert!(index.bindings.is_empty() && index.score_issues.is_empty());
        assert_eq!(source.tracks, before);
        assert!(!normalize(&source).unwrap().score_issues.is_empty());
    }

    fn selected_source() -> Midi {
        crate::engine::musescore::parse(br#"<museScore version="2.06"><programVersion>2.3.2</programVersion><Score><Division>480</Division><Part><trackName>Voice</trackName><Staff id="1"/><Instrument id="voice"><instrumentId>voice.vocals</instrumentId></Instrument></Part><Staff id="1"><Measure len="4/4"><startRepeat/>
        <Dynamic><subtype>p</subtype></Dynamic>
        <Chord><durationType>quarter</durationType><Lyrics><no>0</no><text>hold</text></Lyrics><Lyrics><no>1</no><text>again</text></Lyrics><Note><pitch>65</pitch><Tie id="first"/></Note></Chord>
        <Dynamic><subtype>sfz</subtype></Dynamic>
        <Chord><durationType>quarter</durationType><Lyrics><no>1</no><text>ta</text></Lyrics><Note><pitch>65</pitch><endSpanner id="first"/><Tie id="second"/></Note></Chord>
        <Chord><durationType>quarter</durationType><Lyrics><no>1</no><text>il</text></Lyrics><Note><pitch>65</pitch><endSpanner id="second"/></Note></Chord>
        <Chord><durationType>quarter</durationType><Lyrics><no>0</no><text>end</text></Lyrics><Lyrics><no>1</no><text>end</text></Lyrics><Note><pitch>65</pitch></Note></Chord>
        <endRepeat>2</endRepeat></Measure></Staff></Score></museScore>"#).unwrap()
    }

    #[test]
    fn normalization_and_selected_groups_keep_one_cumulative_copy_budget() {
        let source = selected_source();
        let outcome = crate::engine::convert::convert_midi_with_target(
            &source,
            "english",
            None,
            crate::engine::target::ExportTarget::Ustx,
        );
        assert!(outcome.ok, "{:?}", outcome.msg);
        let project = outcome.svp.unwrap();
        assert!(project.tracks.iter().flat_map(|t| &t.notes).any(|n| n
            .performance
            .as_ref()
            .is_some_and(|p| p
                .intensity
                .as_ref()
                .is_some_and(|i| i.source_id != p.source_id))));
        let mut raw = PerformanceIndex::default();
        let mut budget = ProvenanceBudget::with_limits(512 * 1024, MAX_RESOLUTION_WORK);
        bind_intensity(&source, &mut raw, &BTreeSet::new(), &mut budget).unwrap();
        raw.provenance_budget = std::cell::RefCell::new(budget);
        let mut pending: Vec<_> = project
            .tracks
            .into_iter()
            .enumerate()
            .map(|(i, track)| (i, vec!["selected".into()], track))
            .collect();
        bind_selected_score_attacks(&source, &raw, &mut pending).unwrap();
        let mut refused = false;
        for _ in 0..64 {
            match bind_selected_score_attacks(&source, &raw, &mut pending) {
                Ok(()) => {}
                Err(error) => {
                    assert!(error.contains("cumulative provenance copies"), "{error}");
                    refused = true;
                    break;
                }
            }
        }
        assert!(
            refused,
            "selected rebinding must not reset the raw normalization budget"
        );
    }

    #[test]
    fn initial_continuity_adoption_spends_the_same_retained_budget() {
        let source = selected_source();
        let raw = normalize(&source).unwrap();
        let mut bindings = raw.bindings.values();
        let head = bindings.next().unwrap();
        let original_tail = bindings.next().unwrap();
        *raw.provenance_budget.borrow_mut() =
            ProvenanceBudget::with_limits(4096, MAX_RESOLUTION_WORK);
        let mut refused = false;
        for _ in 0..64 {
            let mut tail = original_tail.clone();
            match raw.continue_tied_intensity(&mut tail, head) {
                Ok(()) => {}
                Err(error) => {
                    assert!(error.contains("cumulative provenance copies"), "{error}");
                    assert_eq!(
                        tail, *original_tail,
                        "failed preflight must leave the tail intact"
                    );
                    refused = true;
                    break;
                }
            }
        }
        assert!(refused);
    }
}

#[cfg(test)]
mod third_review_tests {
    use super::*;
    use crate::engine::midi::{NoteOff, NoteOn, NoteSource, Track};
    use crate::engine::score_intensity::{Fraction, ProvenanceBudget, Time};

    #[test]
    fn exact_tick_rounding_and_outward_bounds_cover_wide_fractions_and_ranges() {
        for (raw, rounded) in [
            ("0.499999999999999999", 0),
            ("0.5", 1),
            ("0.500000000000000001", 1),
        ] {
            let at = Fraction::decimal(raw)
                .unwrap()
                .checked_mul(Fraction::new(1, 480).unwrap())
                .unwrap();
            assert_eq!(exact_target_tick(at).unwrap(), rounded);
            assert_eq!(exact_source_tick_bounds(at, at, 480).unwrap(), (0, 1));
        }
        let huge = Time::wide(i128::MAX - 1, i128::MAX).unwrap();
        assert_eq!(exact_target_tick(huge).unwrap(), 480);
        assert_eq!(
            exact_source_tick_bounds(huge, huge, 480).unwrap(),
            (479, 480)
        );
        let maximum = Time::new(i64::from(i32::MAX), 480).unwrap();
        assert_eq!(exact_target_tick(maximum).unwrap(), i32::MAX);
        let above = maximum
            .checked_add(Time::new(1, 480_000_000_000).unwrap())
            .unwrap();
        assert!(
            exact_target_tick(above).is_err(),
            "unrounded target range must be exact"
        );
        let source_maximum = Time::new(i64::from(u32::MAX), 480).unwrap();
        assert_eq!(
            exact_source_tick_bounds(source_maximum, source_maximum, 480).unwrap(),
            (u32::MAX, u32::MAX)
        );
        let above = source_maximum
            .checked_add(Time::new(1, 480_000_000_000).unwrap())
            .unwrap();
        assert!(exact_source_tick_bounds(above, above, 480).is_err());
        for invalid in [
            Time::new(-1, 480).unwrap(),
            Time::wide(i128::MAX, 1).unwrap(),
        ] {
            assert!(exact_target_tick(invalid).is_err());
            assert!(exact_source_tick_bounds(invalid, invalid, 480).is_err());
        }
        assert!(exact_source_tick_bounds(Time::ONE, Time::ZERO, 480).is_err());
        assert!(exact_source_tick_bounds(Time::ZERO, Time::ONE, 0).is_err());
        assert!(Time::wide(1, 0).is_err());
        assert!(Time::wide(1, -1).is_err());
    }

    fn on(track: &mut Track, id: &str, tick: u32) {
        track.events.push(Event::new(
            tick,
            track.events.len() as u32,
            Kind::NoteOn(NoteOn {
                channel: Some(0),
                key: Some(60),
                velocity: Some(80),
                source: NoteSource {
                    id: id.into(),
                    ..NoteSource::default()
                },
                lyrics: Vec::new(),
            }),
        ));
    }
    fn off(track: &mut Track, id: Option<&str>, tick: u32) {
        track.events.push(Event::new(
            tick,
            track.events.len() as u32,
            Kind::NoteOff(NoteOff {
                channel: Some(0),
                key: Some(60),
                velocity: None,
                source_id: id.map(str::to_owned),
            }),
        ));
    }

    #[test]
    fn exact_note_offs_and_anonymous_fifo_share_bounded_linear_deletion() {
        let mut track = Track::new("unisons", 0);
        let count = 2048;
        for i in 0..count {
            on(&mut track, &format!("n{i}"), 0);
        }
        let mut expected = BTreeMap::new();
        let (mut first, mut last, mut tick) = (0, count - 1, 1);
        while first <= last {
            // Unrelated exact IDs must neither close nor reorder the FIFO.
            off(&mut track, Some("unrelated"), tick);
            off(&mut track, Some(&format!("n{last}")), tick);
            expected.insert(format!("n{last}"), tick);
            last -= 1;
            tick += 1;
            off(&mut track, None, tick);
            expected.insert(format!("n{first}"), tick);
            first += 1;
            tick += 1;
        }
        // Drain every remaining tombstone, including an already-closed exact ID.
        off(&mut track, Some("n0"), tick);
        off(&mut track, None, tick);
        let before = track.clone();
        let notes = intensity_notes(
            &track,
            &mut ProvenanceBudget::with_limits(2 * 1024 * 1024, 1_000_000),
        )
        .unwrap();
        assert_eq!(notes.len(), count);
        for note in notes {
            assert_eq!(note.duration, expected[&note.source.id]);
        }
        assert_eq!(track, before);
        // Counting input events alone used to pass this budget despite repeated
        // uncharged whole-FIFO scans. Real lookup/deletion work must refuse.
        let error = intensity_notes(
            &track,
            &mut ProvenanceBudget::with_limits(2 * 1024 * 1024, track.events.len() * 2),
        )
        .err()
        .unwrap();
        assert!(error.contains("SCORE_INTENSITY_LIMIT"), "{error}");
        assert_eq!(track, before);
    }

    #[test]
    fn tombstones_cannot_close_a_later_reuse_of_an_exact_source_id() {
        let mut track = Track::new("generation", 0);
        on(&mut track, "reused", 0);
        off(&mut track, Some("reused"), 1);
        on(&mut track, "older-live", 2);
        on(&mut track, "reused", 3);
        off(&mut track, None, 4);
        off(&mut track, None, 5);
        let notes = intensity_notes(&track, &mut ProvenanceBudget::default()).unwrap();
        assert_eq!(
            notes
                .iter()
                .map(|n| (n.source.id.as_str(), n.onset, n.duration))
                .collect::<Vec<_>>(),
            [("reused", 0, 1), ("older-live", 2, 2), ("reused", 3, 2)]
        );
    }
}
