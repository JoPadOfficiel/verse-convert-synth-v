//! EXP-003's `verse-score-intensity-v1`, independent of parsers and targets.
//!
//! Inputs use written score coordinates in quarters, before repeat expansion.
//! Loaders must preserve their raw fields and source identities, pair spanners,
//! and supply actual attacks / tie chains. Outputs are evaluable segments, not
//! a pair of held controller points masquerading as a ramp. Target descriptors,
//! rounding, sampling and the bounded niente tail belong to the target adapter.
//!
//! Shared source-owned evaluator; target scaling remains in the USTX adapter.
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const POLICY: &str = "verse-score-intensity-v1";
pub(crate) const MAX_RESOLUTION_WORK: usize = 50_000_000;
const RUNGS: [i64; 12] = [1, 5, 10, 16, 33, 49, 64, 80, 96, 112, 126, 127];
type Result<T> = std::result::Result<T, String>;

/// Exact decimal levels and rational musical time. No target-grid rounding.
/// Overflow is an explicit error, never saturation or a floating-point fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
pub struct Fraction {
    #[serde(serialize_with = "serialize_wide_integer")]
    numerator: i128,
    #[serde(serialize_with = "serialize_wide_integer")]
    denominator: i128,
}

impl Fraction {
    pub const ZERO: Self = Self::integer(0);
    pub const ONE: Self = Self::integer(1);
    pub const fn integer(value: i64) -> Self {
        Self {
            numerator: value as i128,
            denominator: 1,
        }
    }
    pub fn new(numerator: i64, denominator: i64) -> Result<Self> {
        Self::wide(i128::from(numerator), i128::from(denominator))
    }
    pub(crate) fn wide(n: i128, d: i128) -> Result<Self> {
        if d <= 0 {
            return Err("Nonpositive rational denominator.".into());
        }
        let divisor = gcd(n.unsigned_abs(), d as u128) as i128;
        Ok(Self {
            numerator: n / divisor,
            denominator: d / divisor,
        })
    }
    pub fn decimal(raw: &str) -> Result<Self> {
        let text = raw.trim();
        let negative = text.starts_with('-');
        let text = text.strip_prefix(['+', '-']).unwrap_or(text);
        let (whole, tail) = text.split_once('.').unwrap_or((text, ""));
        if (whole.is_empty() && tail.is_empty())
            || !whole
                .bytes()
                .chain(tail.bytes())
                .all(|b| b.is_ascii_digit())
            || tail.len() > 18
        {
            return Err(format!("Unsupported exact decimal: {raw:?}"));
        }
        let denominator = 10i64
            .checked_pow(tail.len() as u32)
            .ok_or("Exact decimal scale overflow.")?;
        let digits = format!("{whole}{tail}");
        let n = digits
            .parse::<i128>()
            .map_err(|_| "Exact decimal overflow.")?;
        Self::wide(if negative { -n } else { n }, i128::from(denominator))
    }
    pub fn checked_add(self, other: Self) -> Result<Self> {
        let common = gcd(self.denominator as u128, other.denominator as u128) as i128;
        let left_scale = other.denominator / common;
        let right_scale = self.denominator / common;
        let n = self
            .numerator
            .checked_mul(left_scale)
            .and_then(|left| {
                other
                    .numerator
                    .checked_mul(right_scale)
                    .and_then(|right| left.checked_add(right))
            })
            .ok_or("Exact rational numerator overflow.")?;
        let d = self
            .denominator
            .checked_mul(left_scale)
            .ok_or("Exact rational denominator overflow.")?;
        Self::wide(n, d)
    }
    pub fn checked_sub(self, other: Self) -> Result<Self> {
        self.checked_add(Self {
            numerator: other
                .numerator
                .checked_neg()
                .ok_or("Exact rational negation overflow.")?,
            denominator: other.denominator,
        })
    }
    pub fn checked_mul(self, other: Self) -> Result<Self> {
        let a = gcd(self.numerator.unsigned_abs(), other.denominator as u128) as i128;
        let b = gcd(other.numerator.unsigned_abs(), self.denominator as u128) as i128;
        let n = (self.numerator / a)
            .checked_mul(other.numerator / b)
            .ok_or("Exact rational numerator overflow.")?;
        let d = (self.denominator / b)
            .checked_mul(other.denominator / a)
            .ok_or("Exact rational denominator overflow.")?;
        Self::wide(n, d)
    }
    pub fn as_f64(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }
    pub fn ratio(self) -> (i128, i128) {
        (self.numerator, self.denominator)
    }
}
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
fn serialize_wide_integer<S: serde::Serializer>(
    value: &i128,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match i64::try_from(*value) {
        Ok(value) => serializer.serialize_i64(value),
        Err(_) => serializer.serialize_str(&value.to_string()),
    }
}
impl Ord for Fraction {
    fn cmp(&self, other: &Self) -> Ordering {
        let sign = self.numerator.signum().cmp(&other.numerator.signum());
        if sign != Ordering::Equal {
            return sign;
        }
        // Continued-fraction comparison never multiplies wide numerators.
        let (mut a, mut b) = (self.numerator.unsigned_abs(), self.denominator as u128);
        let (mut c, mut d) = (other.numerator.unsigned_abs(), other.denominator as u128);
        let mut reverse = self.numerator < 0;
        loop {
            let order = (a / b).cmp(&(c / d));
            if order != Ordering::Equal {
                return if reverse { order.reverse() } else { order };
            }
            let (r, s) = (a % b, c % d);
            if r == 0 || s == 0 {
                let order = r.cmp(&s);
                return if reverse { order.reverse() } else { order };
            }
            (a, b, c, d) = (b, r, d, s);
            reverse = !reverse;
        }
    }
}
impl PartialOrd for Fraction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
/// All times in this module are quarters, not MIDI ticks or seconds.
pub type Time = Fraction;

pub mod source;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScoreVoice {
    pub part: String,
    pub instrument: Option<String>,
    pub staff: String,
    pub voice: String,
}

/// These are score identities. They must never be encoded as MIDI channels.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum Scope {
    Midi {
        port: u8,
        channel: u8,
    },
    System,
    Part(String),
    Instrument {
        part: String,
        instrument: Option<String>,
    },
    Staff {
        part: String,
        staff: String,
    },
    Voice {
        part: String,
        staff: Option<String>,
        voice: String,
    },
    Unsupported {
        part: String,
        raw: String,
    },
}
impl Scope {
    pub fn applies(&self, owner: &ScoreVoice) -> bool {
        match self {
            Self::System => true,
            Self::Part(part) => part == &owner.part,
            Self::Instrument { part, instrument } => {
                part == &owner.part
                    && instrument
                        .as_ref()
                        .is_none_or(|id| Some(id) == owner.instrument.as_ref())
            }
            Self::Staff { part, staff } => part == &owner.part && staff == &owner.staff,
            Self::Voice { part, staff, voice } => {
                part == &owner.part
                    && staff.as_ref().is_none_or(|id| id == &owner.staff)
                    && voice == &owner.voice
            }
            Self::Unsupported { .. } | Self::Midi { .. } => false,
        }
    }
    fn priority(&self) -> u8 {
        match self {
            Self::System => 0,
            Self::Part(_) | Self::Instrument { .. } => 1,
            Self::Staff { .. } => 2,
            Self::Voice { .. } | Self::Midi { .. } => 3,
            Self::Unsupported { .. } => 0,
        }
    }
    pub fn musicxml(part: &str, staff: Option<&str>, voice: Option<&str>) -> Self {
        if let Some(voice) = voice {
            Self::Voice {
                part: part.into(),
                staff: staff.map(Into::into),
                voice: voice.into(),
            }
        } else if let Some(staff) = staff {
            Self::Staff {
                part: part.into(),
                staff: staff.into(),
            }
        } else {
            Self::Part(part.into())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, PartialOrd, Ord)]
pub enum SourceContract {
    MusicXml,
    MuseScoreLegacy,
    MuseScoreModern,
    Midi,
}

/// Raw enum decoding belongs to the loader. Unknown values remain localized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceAssignment {
    CurrentVoice,
    StaffVoices,
    InstrumentVoices,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyRange {
    Staff,
    Part,
    System,
}

pub fn musescore_scope(
    owner: &ScoreVoice,
    modern: bool,
    assignment: Option<VoiceAssignment>,
    legacy: Option<LegacyRange>,
) -> (Scope, &'static str) {
    let staff = || Scope::Staff {
        part: owner.part.clone(),
        staff: owner.staff.clone(),
    };
    if modern {
        if let Some(assignment) = assignment {
            return (
                match assignment {
                    VoiceAssignment::CurrentVoice => Scope::Voice {
                        part: owner.part.clone(),
                        staff: Some(owner.staff.clone()),
                        voice: owner.voice.clone(),
                    },
                    VoiceAssignment::StaffVoices => staff(),
                    VoiceAssignment::InstrumentVoices => Scope::Instrument {
                        part: owner.part.clone(),
                        instrument: owner.instrument.clone(),
                    },
                },
                "Explicit modern voiceAssignment.",
            );
        }
        if legacy.is_none() {
            return (
                Scope::Instrument {
                    part: owner.part.clone(),
                    instrument: owner.instrument.clone(),
                },
                "Portable modern default: instrument-wide, never score-wide.",
            );
        }
    }
    (
        match legacy.unwrap_or(LegacyRange::Part) {
            LegacyRange::Staff => staff(),
            LegacyRange::Part => Scope::Part(owner.part.clone()),
            LegacyRange::System => Scope::System,
        },
        if legacy.is_some() {
            "Explicit legacy dynType compatibility mapping."
        } else {
            "Pinned legacy constructor default: part."
        },
    )
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, PartialOrd, Ord)]
pub struct Evidence {
    pub source_ids: Vec<String>,
    pub contract: SourceContract,
    pub saving_version: Option<String>,
    pub layout: String,
    /// Includes unknown, disabled, drawing-only and overridden fields verbatim.
    pub raw_fields: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, PartialOrd, Ord)]
pub enum Basis {
    ExplicitNumeric,
    StandardTable,
    DeclaredDefault,
    InferredEndpoint,
    InferredSpan,
    SourceArithmetic,
    PortableInterpretation,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, PartialOrd, Ord)]
pub struct Interpretation {
    pub field: String,
    pub source_ids: Vec<String>,
    pub basis: Basis,
    pub exact: Option<Fraction>,
    pub explanation: String,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Provenance {
    pub policy: &'static str,
    pub scope: Scope,
    pub occurrence: u32,
    pub repeat_pass: u32,
    pub evidence: Vec<Evidence>,
    pub interpretations: Vec<Interpretation>,
}
impl Provenance {
    fn from_event(event: &ScoreEvent, pass: u32) -> Self {
        Self {
            policy: POLICY,
            scope: event.scope.clone(),
            occurrence: 0,
            repeat_pass: pass,
            evidence: vec![event.evidence.clone()],
            interpretations: vec![event.scope_interpretation.clone()],
        }
    }
    fn describe(&mut self, field: &str, basis: Basis, exact: Option<Fraction>, reason: &str) {
        self.interpretations.push(Interpretation {
            field: field.into(),
            source_ids: self
                .evidence
                .first()
                .map_or_else(Vec::new, |e| e.source_ids.clone()),
            basis,
            exact,
            explanation: reason.into(),
        });
    }
    fn include(&mut self, other: &Self) {
        // Compare borrowed values in a tree, preserving the original append order.
        // In particular, do not rescan every accumulated XML slice for each item.
        let evidence = {
            let mut seen: BTreeSet<_> = self.evidence.iter().collect();
            other
                .evidence
                .iter()
                .filter(|e| seen.insert(*e))
                .collect::<Vec<_>>()
        };
        self.evidence.extend(evidence.into_iter().cloned());
        let interpretations = {
            let mut seen: BTreeSet<_> = self.interpretations.iter().collect();
            other
                .interpretations
                .iter()
                .filter(|i| seen.insert(*i))
                .collect::<Vec<_>>()
        };
        self.interpretations
            .extend(interpretations.into_iter().cloned());
    }
}

/// Counts cumulative allocations and comparison work, including temporary copies.
/// This remains separate from event-count limits: a single raw XML field can be large.
#[derive(Clone, Debug, Default)]
pub(crate) struct ProvenanceBudget {
    bytes: usize,
    work: usize,
    #[cfg(test)]
    limits: Option<(usize, usize)>,
}
const MAX_PROVENANCE_BYTES: usize = 128 * 1024 * 1024;
fn strings_size(values: &[String]) -> usize {
    values
        .iter()
        .fold(0usize, |n, s| n.saturating_add(s.len()).saturating_add(32))
}
fn evidence_size(e: &Evidence) -> usize {
    e.raw_fields.iter().fold(
        128usize
            .saturating_add(strings_size(&e.source_ids))
            .saturating_add(e.layout.len())
            .saturating_add(e.saving_version.as_ref().map_or(0, String::len)),
        |n, (key, value)| {
            n.saturating_add(key.len())
                .saturating_add(value.len())
                .saturating_add(96)
        },
    )
}
fn interpretation_size(i: &Interpretation) -> usize {
    128usize
        .saturating_add(i.field.len())
        .saturating_add(i.explanation.len())
        .saturating_add(strings_size(&i.source_ids))
}
fn scope_size(scope: &Scope) -> usize {
    match scope {
        Scope::Part(p) => p.len(),
        Scope::Instrument { part, instrument } => part
            .len()
            .saturating_add(instrument.as_ref().map_or(0, String::len)),
        Scope::Staff { part, staff } => part.len().saturating_add(staff.len()),
        Scope::Voice { part, staff, voice } => part
            .len()
            .saturating_add(staff.as_ref().map_or(0, String::len))
            .saturating_add(voice.len()),
        Scope::Unsupported { part, raw } => part.len().saturating_add(raw.len()),
        Scope::System | Scope::Midi { .. } => 0,
    }
}
fn provenance_size(p: &Provenance) -> usize {
    p.evidence
        .iter()
        .map(evidence_size)
        .chain(p.interpretations.iter().map(interpretation_size))
        .fold(
            256usize.saturating_add(scope_size(&p.scope)),
            usize::saturating_add,
        )
}
impl ProvenanceBudget {
    #[cfg(test)]
    pub(crate) fn with_limits(bytes: usize, work: usize) -> Self {
        Self {
            limits: Some((bytes, work)),
            ..Self::default()
        }
    }

    pub(crate) fn charge(&mut self, bytes: usize, work: usize) -> Result<()> {
        #[cfg(test)]
        let (byte_limit, work_limit) = self
            .limits
            .unwrap_or((MAX_PROVENANCE_BYTES, MAX_RESOLUTION_WORK));
        #[cfg(not(test))]
        let (byte_limit, work_limit) = (MAX_PROVENANCE_BYTES, MAX_RESOLUTION_WORK);
        let bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= byte_limit)
            .ok_or("SCORE_INTENSITY_LIMIT: cumulative provenance copies exceed 128 MiB")?;
        let work = self
            .work
            .checked_add(work)
            .filter(|n| *n <= work_limit)
            .ok_or("SCORE_INTENSITY_LIMIT: cumulative provenance comparison work exceeded")?;
        self.bytes = bytes;
        self.work = work;
        Ok(())
    }
    pub(crate) fn reserve_issue(&mut self, issue: &Issue) -> Result<()> {
        let bytes = provenance_size(&issue.provenance).saturating_add(issue.message.len());
        self.charge(bytes, bytes / 8 + 1)
    }
    pub(crate) fn reserve_velocity(&mut self, velocity: &VelocityEvidence) -> Result<()> {
        let bytes = evidence_size(&velocity.evidence)
            .saturating_add(std::mem::size_of::<VelocityEvidence>())
            .saturating_add(match &velocity.value {
                AttackVelocity::Invalid(reason) => reason.len(),
                _ => 0,
            });
        self.charge(bytes, bytes / 8 + 1)
    }
    fn copy(&mut self, p: &Provenance) -> Result<Provenance> {
        let bytes = provenance_size(p);
        self.charge(bytes, bytes / 8 + 1)?;
        Ok(p.clone())
    }
    fn include(&mut self, p: &mut Provenance, other: &Provenance) -> Result<()> {
        let bytes = provenance_size(other);
        let count = p
            .evidence
            .len()
            .saturating_add(other.evidence.len())
            .saturating_add(p.interpretations.len())
            .saturating_add(other.interpretations.len());
        let depth = usize::BITS as usize - count.leading_zeros() as usize + 1;
        // Both tree construction and lookups may compare equal, long raw fields.
        let work = provenance_size(p)
            .saturating_add(bytes)
            .saturating_mul(depth)
            .saturating_mul(2);
        self.charge(bytes.saturating_add(count.saturating_mul(64)), work / 8 + 1)?;
        p.include(other);
        Ok(())
    }
    fn event(&mut self, e: &ScoreEvent) -> Result<()> {
        // Reserve the bounded number of descriptions created by prepare/level resolution,
        // including source IDs copied into each one, before any of those allocations.
        let bytes = evidence_size(&e.evidence)
            .saturating_add(scope_size(&e.scope))
            .saturating_add(interpretation_size(&e.scope_interpretation))
            .saturating_add(
                strings_size(&e.evidence.source_ids)
                    .saturating_add(512)
                    .saturating_mul(12),
            );
        self.charge(bytes, bytes / 8 + 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Level {
    Silence,
    Positive(Fraction),
}
impl Level {
    pub fn positive(level: Fraction) -> Result<Self> {
        if level > Fraction::ZERO && level <= Fraction::integer(127) {
            Ok(Self::Positive(level))
        } else {
            Err(format!(
                "Level {:?} is outside the bounded positive domain (0,127].",
                level.ratio()
            ))
        }
    }
    fn number(self) -> Fraction {
        match self {
            Self::Silence => Fraction::ZERO,
            Self::Positive(v) => v,
        }
    }
    pub fn value(self) -> Intensity {
        match self {
            Self::Silence => Intensity::Silence,
            Self::Positive(v) => Intensity::Decibels((v.as_f64() - 80.0) / 4.0),
        }
    }
}

pub fn standard_level(mark: &str) -> Option<Level> {
    let value = match mark {
        "n" | "niente" => return Some(Level::Silence),
        "pppppp" => 1,
        "ppppp" => 5,
        "pppp" => 10,
        "ppp" => 16,
        "pp" => 33,
        "p" => 49,
        "mp" => 64,
        "mf" => 80,
        "f" => 96,
        "ff" => 112,
        "fff" => 126,
        "ffff" | "fffff" | "ffffff" => 127,
        _ => return None,
    };
    Some(Level::Positive(Fraction::integer(value)))
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub enum Intensity {
    Silence,
    Decibels(f64),
}
impl Intensity {
    pub fn gain(self) -> f64 {
        match self {
            Self::Silence => 0.0,
            Self::Decibels(db) => 10.0f64.powf(db / 20.0),
        }
    }
}
/// Absence retains existing output; unknown cannot be replaced by neutral gain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Evaluation {
    Absent,
    Unknown,
    Known(Intensity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Crescendo,
    Diminuendo,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextIntent {
    Transition(Direction),
    FadeIn,
    FadeOut,
}
pub fn recognized_text(text: &str) -> Option<TextIntent> {
    let normalized = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    Some(match normalized.as_str() {
        "cresc." | "cresc" | "crescendo" => TextIntent::Transition(Direction::Crescendo),
        "dim." | "dim" | "diminuendo" | "decresc." | "decrescendo" => {
            TextIntent::Transition(Direction::Diminuendo)
        }
        "morendo" | "smorzando" | "fade out" | "fade-out" | "fondu" | "fondu au silence" => {
            TextIntent::FadeOut
        }
        "fade in" | "fade-in" => TextIntent::FadeIn,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum Easing {
    #[default]
    Normal,
    EaseIn,
    EaseOut,
    EaseInOut,
    Exponential,
}
impl Easing {
    pub fn normalized(self, x: f64, delta_level: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        if x == 0.0 || x == 1.0 {
            return x;
        }
        match self {
            Self::Normal => x,
            Self::EaseIn => 1.0 - (std::f64::consts::FRAC_PI_2 * x).cos(),
            Self::EaseOut => (std::f64::consts::FRAC_PI_2 * x).sin(),
            Self::EaseInOut => (1.0 - (std::f64::consts::PI * x).cos()) / 2.0,
            Self::Exponential if delta_level != 0.0 => {
                ((delta_level.abs() + 1.0).powf(x) - 1.0) / delta_level.abs()
            }
            Self::Exponential => x,
        }
    }
    pub fn named(raw: &str) -> Option<Self> {
        Some(match raw {
            "normal" => Self::Normal,
            "ease-in" => Self::EaseIn,
            "ease-out" => Self::EaseOut,
            "ease-in-out" => Self::EaseInOut,
            "exponential" => Self::Exponential,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Speed {
    Slow,
    #[default]
    Normal,
    Fast,
}
pub fn transition_duration(tempo_at_start: Fraction, speed: Speed) -> Result<Time> {
    if tempo_at_start <= Fraction::ZERO {
        return Err("Transition tempo must be positive.".into());
    }
    let factor = match speed {
        Speed::Slow => Fraction::new(13, 10)?,
        Speed::Normal => Fraction::new(4, 5)?,
        Speed::Fast => Fraction::new(1, 2)?,
    };
    tempo_at_start
        .checked_mul(Fraction::new(1, 120)?)?
        .checked_mul(factor)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NumericLevel {
    /// MusicXML sound@dynamics or note@dynamics, relative to forte=90.
    MusicXml(Fraction),
    /// MuseScore Dynamic/velocity; <=0 means use the symbol table.
    MuseScoreDynamic(Fraction),
    Level(Fraction),
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dynamic {
    pub symbol: Option<String>,
    pub numeric: Option<NumericLevel>,
    pub velo_change: Option<Fraction>,
    pub speed: Speed,
    pub tempo_at_start: Option<Fraction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    pub end: Time,
    pub direction: Direction,
    pub start: Option<Level>,
    pub end_level: Option<Level>,
    pub velo_change: Option<Fraction>,
    pub niente_start: bool,
    pub niente_end: bool,
    pub method: Option<String>,
    pub single_note_dynamics: Option<bool>,
}
impl Transition {
    pub fn new(end: Time, direction: Direction) -> Self {
        Self {
            end,
            direction,
            start: None,
            end_level: None,
            velo_change: None,
            niente_start: false,
            niente_end: false,
            method: None,
            single_note_dynamics: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instruction {
    Dynamic(Dynamic),
    Transition(Transition),
    Text {
        text: String,
        end: Option<Time>,
        owning_spanner: Option<String>,
    },
    Unsupported(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreEvent {
    pub at: Time,
    pub order: u32,
    pub scope: Scope,
    /// Loader's explicit/default/compatibility scope decision. Required so a
    /// modern default cannot later be mistaken for an authored assignment.
    pub scope_interpretation: Interpretation,
    pub enabled: bool,
    pub time_only: Vec<u32>,
    pub evidence: Evidence,
    pub instruction: Instruction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    UnsupportedMark,
    UnsupportedScope,
    Disabled,
    PassFiltered,
    InvalidLevel,
    UnresolvedSpan,
    Conflict,
    DirectionMismatch,
    RangeExhausted,
    Interrupted,
    EasingFallback,
    InferredSpan,
    ContinuationVelocity,
    UnknownAnchor,
}
/// The shared performance adapter translates these semantic issues to its
/// existing TransferStatus; this module does not define a second target status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub start: Time,
    pub end: Time,
    pub kind: IssueKind,
    pub provenance: Provenance,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Domain {
    RelativeDecibels,
    LinearGain,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum Curve {
    Absent,
    /// Exact resolved source level, including decimals, held through rests.
    Level(Level),
    /// None is an unknown/unmapped interval, never neutral or mute.
    Held(Option<Intensity>),
    Transition {
        start: Time,
        end: Time,
        from: Level,
        to: Level,
        easing: Easing,
        domain: Domain,
    },
}
impl Curve {
    pub fn evaluate(&self, at: Time) -> Evaluation {
        match self {
            Self::Absent => Evaluation::Absent,
            Self::Level(level) => Evaluation::Known(level.value()),
            Self::Held(None) => Evaluation::Unknown,
            Self::Held(Some(value)) => Evaluation::Known(*value),
            Self::Transition {
                start,
                end,
                from,
                to,
                easing,
                domain,
            } => {
                if at <= *start {
                    return Evaluation::Known(from.value());
                }
                if at >= *end {
                    return Evaluation::Known(to.value());
                }
                let x = (at.as_f64() - start.as_f64()) / (end.as_f64() - start.as_f64());
                let x = easing.normalized(x, to.number().as_f64() - from.number().as_f64());
                let value = match domain {
                    Domain::RelativeDecibels => {
                        let (Intensity::Decibels(a), Intensity::Decibels(b)) =
                            (from.value(), to.value())
                        else {
                            return Evaluation::Unknown;
                        };
                        Intensity::Decibels(a + x * (b - a))
                    }
                    Domain::LinearGain => {
                        let (a, b) = (from.value().gain(), to.value().gain());
                        let gain = a * (1.0 - x) + b * x;
                        if gain == 0.0 {
                            Intensity::Silence
                        } else {
                            Intensity::Decibels(20.0 * gain.log10())
                        }
                    }
                };
                Evaluation::Known(value)
            }
        }
    }
}

/// A segment can clip an original ramp while retaining its original phase.
/// `[start,end)` is active; the last endpoint is also evaluable exactly.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Segment {
    pub start: Time,
    pub end: Time,
    pub curve: Curve,
    pub attack_only: bool,
    pub provenance: Option<Provenance>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Accent {
    pub at: Time,
    pub level: Level,
    pub provenance: Provenance,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timeline {
    pub segments: Vec<Segment>,
    pub accents: Vec<Accent>,
    pub issues: Vec<Issue>,
}
impl Timeline {
    pub fn segment_at(&self, at: Time) -> Option<&Segment> {
        // Resolver/overlay/occurrence construction maintains ascending starts
        // with disjoint positive spans and explicit terminal point segments.
        let i = self
            .segments
            .partition_point(|s| s.start <= at)
            .checked_sub(1)?;
        let segment = &self.segments[i];
        (at <= segment.end).then_some(segment)
    }
    pub(crate) fn accent_range(&self, attacks: &[Time], at: Time) -> std::ops::Range<usize> {
        let previous = attacks
            .partition_point(|t| *t < at)
            .checked_sub(1)
            .map(|i| attacks[i]);
        let first = self
            .accents
            .partition_point(|a| previous.is_some_and(|t| a.at <= t));
        let end = self.accents.partition_point(|a| a.at <= at);
        first..end
    }
    pub fn evaluate(&self, at: Time) -> Evaluation {
        self.segment_at(at)
            .map_or(Evaluation::Absent, |s| s.curve.evaluate(at))
    }
    fn issue(
        &mut self,
        budget: &mut ProvenanceBudget,
        start: Time,
        end: Time,
        kind: IssueKind,
        p: &Provenance,
        message: impl Into<String>,
    ) -> Result<()> {
        self.issues.push(Issue {
            start,
            end,
            kind,
            provenance: budget.copy(p)?,
            message: message.into(),
        });
        Ok(())
    }
    /// Overlay only this interval. Any sliced transition keeps its original
    /// start/end/easing, so an interruption never stretches its remaining phase.
    fn overlay(&mut self, incoming: Segment, budget: &mut ProvenanceBudget) -> Result<()> {
        if incoming.end < incoming.start {
            return Ok(());
        }
        if incoming.end == incoming.start {
            self.segments.push(incoming);
            self.segments.sort_by_key(|s| s.start);
            return Ok(());
        }
        let mut result = Vec::new();
        for old in self.segments.drain(..) {
            if old.end <= incoming.start || old.start >= incoming.end {
                result.push(old);
                continue;
            }
            if old.start < incoming.start {
                result.push(Segment {
                    start: old.start,
                    end: incoming.start,
                    curve: old.curve.clone(),
                    attack_only: old.attack_only,
                    provenance: old
                        .provenance
                        .as_ref()
                        .map(|p| budget.copy(p))
                        .transpose()?,
                });
            }
            if old.end > incoming.end {
                result.push(Segment {
                    start: incoming.end,
                    ..old
                });
            }
        }
        result.push(incoming);
        result.sort_by_key(|s| s.start);
        self.segments = result;
        Ok(())
    }
}

/// MusicXML's playback offset policy. The caller supplies offsets already
/// divided by the locally active divisions; neither drawing x nor spread enters.
pub fn musicxml_position(
    cursor: Time,
    sound_offset: Option<Time>,
    direction_offset: Option<(Time, bool)>,
) -> Result<Time> {
    cursor.checked_add(
        sound_offset
            .or_else(|| direction_offset.and_then(|(offset, playback)| playback.then_some(offset)))
            .unwrap_or(Time::ZERO),
    )
}

fn resolve_level(dynamic: &Dynamic, p: &mut Provenance) -> Result<Option<Level>> {
    if let Some(numeric) = &dynamic.numeric {
        let value = match numeric {
            NumericLevel::MusicXml(value) => Some(value.checked_mul(Fraction::new(9, 10)?)?),
            NumericLevel::MuseScoreDynamic(value) if *value <= Fraction::ZERO => {
                p.describe(
                    "velocity",
                    Basis::SourceArithmetic,
                    Some(*value),
                    "Nonpositive Dynamic velocity is a table sentinel, not silence.",
                );
                None
            }
            NumericLevel::MuseScoreDynamic(value) | NumericLevel::Level(value) => Some(*value),
        };
        if let Some(value) = value {
            p.describe("level", Basis::ExplicitNumeric, Some(value), "Numeric level replaces the symbol in this declaration; it is not multiplied by it.");
            return if value == Fraction::ZERO {
                Ok(Some(Level::Silence))
            } else {
                Level::positive(value).map(Some)
            };
        }
    }
    let Some(symbol) = &dynamic.symbol else {
        return Ok(None);
    };
    let level = standard_level(symbol);
    if let Some(level) = level {
        p.describe(
            "level",
            Basis::StandardTable,
            Some(level.number()),
            "Portable legacy-equivalent table, not modern MPE loudness.",
        );
    }
    Ok(level)
}

fn fallback_rung(start: Level, direction: Direction) -> Option<Level> {
    match direction {
        Direction::Crescendo => RUNGS
            .iter()
            .find(|&&v| Fraction::integer(v) > start.number()),
        Direction::Diminuendo => RUNGS
            .iter()
            .rev()
            .find(|&&v| Fraction::integer(v) < start.number()),
    }
    .copied()
    .map(|v| Level::Positive(Fraction::integer(v)))
}
fn direction_matches(start: Level, end: Level, direction: Direction) -> bool {
    match direction {
        Direction::Crescendo => end.number() > start.number(),
        Direction::Diminuendo => end.number() < start.number(),
    }
}

#[derive(Clone, Debug)]
enum Action {
    Set(Option<Level>),
    Ramp(Transition),
    Accent(Level),
    Unknown,
    Conflict(Time),
}
struct ResolvedRamp {
    segment: Segment,
    written_end: Time,
    boundary: Option<Level>,
}
fn copy_segment(segment: &Segment, budget: &mut ProvenanceBudget) -> Result<Segment> {
    Ok(Segment {
        start: segment.start,
        end: segment.end,
        curve: segment.curve.clone(),
        attack_only: segment.attack_only,
        provenance: segment
            .provenance
            .as_ref()
            .map(|p| budget.copy(p))
            .transpose()?,
    })
}

#[derive(Clone, Debug)]
struct Prepared {
    at: Time,
    order: u32,
    scope: Scope,
    action: Action,
    lookahead_end: Time,
    provenance: Provenance,
}

/// A maximal forward run, ending before a repeat/jump. The loader supplies
/// these runs from its existing playback order, not from output-lane heuristics.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Occurrence {
    pub written_start: Time,
    pub written_end: Time,
    pub performed_start: Time,
    pub pass: u32,
    pub ordinal: u32,
}

#[derive(Clone, Copy)]
struct PlaybackSpan {
    start: Time,
    end: Time,
    pass: u32,
}

#[derive(Clone, Copy, Default)]
struct ResolutionRoute<'a> {
    path: Option<(&'a [PlaybackSpan], bool)>,
    input: Option<&'a source::ScoreInput>,
}

/// Ordinary spans are half-open. Point records at the final endpoint require
/// an explicit terminal owner; shared note boundaries belong to the later note.
pub fn intersects(start: Time, end: Time, lo: Time, hi: Time, terminal: bool) -> bool {
    if start == end {
        start >= lo && (start < hi || (terminal && start == hi))
    } else {
        start < hi && end > lo
    }
}
pub(crate) fn performed_issue(
    issue: &Issue,
    run: &Occurrence,
    terminal: bool,
) -> Result<Option<Issue>> {
    if !intersects(
        issue.start,
        issue.end,
        run.written_start,
        run.written_end,
        terminal,
    ) {
        return Ok(None);
    }
    let offset = run.performed_start.checked_sub(run.written_start)?;
    let mut mapped = issue.clone();
    mapped.start = issue.start.max(run.written_start).checked_add(offset)?;
    mapped.end = issue.end.min(run.written_end).checked_add(offset)?;
    mapped.provenance.occurrence = run.ordinal;
    mapped.provenance.repeat_pass = run.pass;
    Ok(Some(mapped))
}

/// All raw declarations stay here, including disabled and unresolved ones.
#[derive(Clone, Debug, Default)]
pub struct ScoreIntensity {
    pub events: Vec<ScoreEvent>,
}

impl ScoreIntensity {
    fn relevant(event: &ScoreEvent, owner: &ScoreVoice, end: Time) -> bool {
        event.at <= end
            && (event.scope.applies(owner)
                || matches!(&event.scope, Scope::Unsupported { part, .. } if part == &owner.part))
    }
    fn resolution_work(&self, owner: &ScoreVoice, end: Time) -> Result<usize> {
        let n = self
            .events
            .iter()
            .filter(|e| Self::relevant(e, owner, end))
            .count();
        // Bound traversal before preparation. Provenance is charged immediately
        // before each actual copy against one counter shared by all owners/runs;
        // ordinary declarations must not pay for hypothetical inherited copies.
        n.checked_add(1)
            .and_then(|next| n.checked_mul(next))
            .and_then(|pairs| pairs.checked_mul(8))
            .and_then(|work| work.checked_add(self.events.len()))
            .filter(|work| *work <= MAX_RESOLUTION_WORK)
            .ok_or_else(|| "SCORE_INTENSITY_LIMIT: source resolution exceeds bounded work".into())
    }
    pub(crate) fn occurrence_work(&self, owner: &ScoreVoice, runs: &[Occurrence]) -> Result<usize> {
        let mut total = 0usize;
        for run in runs {
            total = total
                .checked_add(self.resolution_work(owner, run.written_end)?)
                .filter(|work| *work <= MAX_RESOLUTION_WORK)
                .ok_or("SCORE_INTENSITY_LIMIT: repeated source resolution exceeds bounded work")?;
        }
        Ok(total)
    }

    fn prepare(
        &self,
        owner: &ScoreVoice,
        pass: u32,
        bounds: (Time, Time),
        route: ResolutionRoute<'_>,
        result: &mut Timeline,
        budget: &mut ProvenanceBudget,
    ) -> Result<Vec<Prepared>> {
        let (end, sounding_end) = bounds;
        let path = route.path;
        if route.input.is_some() {
            budget.charge(
                0,
                self.events.iter().fold(0usize, |work, event| {
                    work.saturating_add(event.evidence.source_ids.len().saturating_mul(16))
                }),
            )?;
        }
        let visited = |at: Time| {
            at <= end
                && path.is_none_or(|(spans, include_end)| {
                    let index = spans.partition_point(|span| span.start <= at);
                    index > 0
                        && (at < spans[index - 1].end
                            || (include_end && at == end && at == spans[index - 1].end))
                })
        };
        // Build one borrowed index per route resolution, never one score copy
        // per event/note. Skipped ranges and backward-reset suffixes are absent.
        let mut tempos = Vec::new();
        if let Some(input) = route.input {
            let count = input.tempo_candidates.values().map(Vec::len).sum::<usize>();
            budget.charge(count.saturating_mul(64), count.saturating_mul(16))?;
            for (at, candidates) in &input.tempo_candidates {
                if !visited(*at) {
                    continue;
                }
                for candidate in candidates {
                    if candidate
                        .evidence
                        .source_ids
                        .iter()
                        .all(|id| input.declaration_on_path(id, owner, path))
                    {
                        tempos.push((*at, candidate));
                    }
                }
            }
        }
        let mut horizons = Vec::new();
        if let Some((spans, _)) = path {
            budget.charge(
                spans.len().saturating_mul(std::mem::size_of::<Time>()),
                spans.len(),
            )?;
            horizons.resize(spans.len(), end);
            for index in (0..spans.len()).rev() {
                horizons[index] =
                    if index + 1 < spans.len() && spans[index].end == spans[index + 1].start {
                        horizons[index + 1]
                    } else {
                        spans[index].end
                    };
            }
        }
        let horizon_at = |at: Time| {
            path.and_then(|(spans, _)| {
                spans
                    .partition_point(|span| span.start <= at)
                    .checked_sub(1)
            })
            .map_or(end, |index| horizons[index])
        };
        let pass_at = |event: &ScoreEvent| {
            path.and_then(|(spans, _)| {
                spans
                    .partition_point(|span| span.start <= event.at)
                    .checked_sub(1)
                    .map(|index| spans[index].pass)
            })
            .unwrap_or(pass)
        };
        let mut prepared = Vec::new();
        let mut events: Vec<_> = self
            .events
            .iter()
            .filter(|e| {
                Self::relevant(e, owner, end)
                    && visited(e.at)
                    && route.input.is_none_or(|input| {
                        e.evidence
                            .source_ids
                            .iter()
                            .all(|id| input.declaration_on_path(id, owner, path))
                    })
            })
            .collect();
        events.sort_by_key(|e| (e.at, e.order));
        for event in events.iter().copied() {
            budget.event(event)?;
            let mut p = Provenance::from_event(event, pass);
            if let Scope::Unsupported { part, .. } = &event.scope {
                if part == &owner.part {
                    result.issue(
                        budget,
                        event.at,
                        event.at,
                        IssueKind::UnsupportedScope,
                        &p,
                        "Unknown scope retained locally; no global broadcast.",
                    )?;
                }
                continue;
            }
            if !event.scope.applies(owner) {
                continue;
            }
            if !event.enabled {
                result.issue(
                    budget,
                    event.at,
                    event.at,
                    IssueKind::Disabled,
                    &p,
                    "Playback-disabled declaration retained inactive.",
                )?;
                continue;
            }
            if !event.time_only.is_empty() && !event.time_only.contains(&pass_at(event)) {
                result.issue(
                    budget,
                    event.at,
                    event.at,
                    IssueKind::PassFiltered,
                    &p,
                    "time-only excludes this repeat pass.",
                )?;
                continue;
            }
            let action = match &event.instruction {
                Instruction::Dynamic(dynamic) => {
                    let mut compound = None;
                    let mut dynamic = dynamic.clone();
                    match dynamic.symbol.as_deref() {
                        Some("fp" | "pf") => {
                            let fp = dynamic.symbol.as_deref() == Some("fp");
                            dynamic.symbol = Some(if fp { "f" } else { "p" }.into());
                            compound = standard_level(if fp { "p" } else { "f" });
                        }
                        Some("sf" | "sfz" | "rf" | "rfz" | "fz" | "sff" | "sffz") => {
                            let loud = matches!(dynamic.symbol.as_deref(), Some("sff" | "sffz"));
                            dynamic.symbol = Some(if loud { "fff" } else { "ff" }.into());
                            if dynamic.velo_change.is_none_or(|v| v == Fraction::ZERO) {
                                let level = match resolve_level(&dynamic, &mut p) {
                                    Ok(Some(level)) => level,
                                    Err(message) => {
                                        result.issue(
                                            budget,
                                            event.at,
                                            event.at,
                                            IssueKind::InvalidLevel,
                                            &p,
                                            message,
                                        )?;
                                        continue;
                                    }
                                    _ => unreachable!(),
                                };
                                prepared.push(Prepared {
                                    at: event.at,
                                    order: event.order,
                                    scope: event.scope.clone(),
                                    action: Action::Accent(level),
                                    lookahead_end: end,
                                    provenance: p,
                                });
                                continue;
                            }
                        }
                        _ => {}
                    }
                    match resolve_level(&dynamic, &mut p) {
                        Ok(Some(start)) => {
                            if let Some(change) =
                                dynamic.velo_change.filter(|v| *v != Fraction::ZERO)
                            {
                                match start.number().checked_add(change).and_then(Level::positive) {
                                    Ok(level) => compound = Some(level),
                                    Err(message) => {
                                        result.issue(
                                            budget,
                                            event.at,
                                            end,
                                            IssueKind::InvalidLevel,
                                            &p,
                                            message,
                                        )?;
                                        compound = None;
                                        // Keep the authored attack; the invalid transition is retained in raw evidence.
                                    }
                                }
                            }
                            if let Some(target) = compound {
                                let duration = if event.evidence.contract
                                    == SourceContract::MusicXml
                                {
                                    p.describe(
                                        "duration",
                                        Basis::DeclaredDefault,
                                        Some(Time::ONE),
                                        "MusicXML fp/pf without timing: one quarter.",
                                    );
                                    Time::ONE
                                } else {
                                    let stop = tempos.partition_point(|(at, _)| *at <= event.at);
                                    let exact = if route.input.is_some() {
                                        if let Some((at, _)) =
                                            stop.checked_sub(1).map(|i| tempos[i])
                                        {
                                            let first = tempos[..stop]
                                                .partition_point(|(time, _)| *time < at);
                                            let candidates = &tempos[first..stop];
                                            budget.charge(0, candidates.len().saturating_mul(3))?;
                                            let value = candidates[0].1.exact;
                                            let unsupported =
                                                candidates.iter().any(|(_, c)| c.exact.is_none());
                                            let conflict =
                                                candidates.iter().any(|(_, c)| c.exact != value);
                                            for (_, candidate) in candidates {
                                                let evidence = &candidate.evidence;
                                                let bytes = evidence_size(evidence)
                                                    .saturating_add(strings_size(
                                                        &evidence.source_ids,
                                                    ))
                                                    .saturating_add(512);
                                                budget.charge(bytes, bytes / 8 + 1)?;
                                                p.evidence.push(evidence.clone());
                                                p.interpretations.push(Interpretation {
                                                    field: "tempo".into(), source_ids: evidence.source_ids.clone(),
                                                    basis: Basis::ExplicitNumeric, exact: candidate.exact,
                                                    explanation: "Coincident exact tempo candidate on the visited source route.".into(),
                                                });
                                            }
                                            if unsupported || conflict {
                                                result.issue(budget, event.at, event.at,
                                                    if unsupported { IssueKind::UnsupportedMark } else { IssueKind::Conflict }, &p,
                                                    if unsupported {
                                                        "Exact expression tempo precision is unsupported; nominal tempo remains unchanged and compound timing was not rounded"
                                                    } else {
                                                        "Conflicting exact tempo candidates; nominal tempo remains unchanged and no compound timing candidate was selected"
                                                    })?;
                                                continue;
                                            }
                                            value
                                        } else {
                                            None
                                        }
                                    } else {
                                        dynamic.tempo_at_start
                                    };
                                    let tempo = exact.unwrap_or(Fraction::integer(120));
                                    if exact.is_none() {
                                        p.describe(
                                            "tempo",
                                            Basis::DeclaredDefault,
                                            Some(tempo),
                                            "Absent source tempo: 120 BPM.",
                                        );
                                    }
                                    let duration = transition_duration(tempo, dynamic.speed)?;
                                    p.describe(
                                        "duration",
                                        Basis::PortableInterpretation,
                                        Some(duration),
                                        "Pinned tempo/speed duration in exact quarters.",
                                    );
                                    duration
                                };
                                let mut transition = Transition::new(
                                    event.at.checked_add(duration)?,
                                    if target.number() > start.number() {
                                        Direction::Crescendo
                                    } else {
                                        Direction::Diminuendo
                                    },
                                );
                                transition.start = Some(start);
                                transition.end_level = Some(target);
                                Action::Ramp(transition)
                            } else {
                                Action::Set(Some(start))
                            }
                        }
                        Ok(None) => {
                            result.issue(budget, event.at, event.at, IssueKind::UnsupportedMark, &p, "Unknown dynamic/compound retained without speculative interpretation.")?;
                            Action::Unknown
                        }
                        Err(message) => {
                            result.issue(
                                budget,
                                event.at,
                                end,
                                IssueKind::InvalidLevel,
                                &p,
                                message,
                            )?;
                            Action::Set(None)
                        }
                    }
                }
                Instruction::Transition(transition) => {
                    if event
                        .evidence
                        .raw_fields
                        .contains_key("intensity_direction_conflict")
                    {
                        Action::Conflict(transition.end)
                    } else {
                        Action::Ramp(transition.clone())
                    }
                }
                Instruction::Text {
                    text,
                    end: stated_end,
                    owning_spanner,
                } => {
                    if owning_spanner.is_some() {
                        continue;
                    }
                    let Some(intent) = recognized_text(text) else {
                        result.issue(
                            budget,
                            event.at,
                            event.at,
                            IssueKind::UnsupportedMark,
                            &p,
                            "Unknown free text retained; no speculative NLP.",
                        )?;
                        continue;
                    };
                    let local_end = horizon_at(event.at);
                    let next_dynamic = events.iter().copied().find(|next| next.at > event.at && next.at <= local_end && next.scope == event.scope
                        && next.enabled && (next.time_only.is_empty() || next.time_only.contains(&pass_at(next)))
                        && matches!(&next.instruction, Instruction::Dynamic(d) if is_ordinary(d)));
                    let span_end = stated_end.unwrap_or_else(|| {
                        next_dynamic
                            .map_or(sounding_end.min(local_end), |next| next.at.min(local_end))
                    });
                    if stated_end.is_none() {
                        p.describe("end", Basis::InferredSpan, Some(span_end), "Recognized standalone text ends at next same-scope dynamic or last sounding end before jump.");
                        result.issue(
                            budget,
                            event.at,
                            span_end,
                            IssueKind::InferredSpan,
                            &p,
                            "Standalone transition span inferred by the bounded text policy.",
                        )?;
                    }
                    let direction = match intent {
                        TextIntent::Transition(d) => d,
                        TextIntent::FadeIn => Direction::Crescendo,
                        TextIntent::FadeOut => Direction::Diminuendo,
                    };
                    let mut transition = Transition::new(span_end, direction);
                    transition.niente_start = intent == TextIntent::FadeIn;
                    transition.niente_end = intent == TextIntent::FadeOut;
                    Action::Ramp(transition)
                }
                Instruction::Unsupported(message) => {
                    result.issue(
                        budget,
                        event.at,
                        event.at,
                        IssueKind::UnsupportedMark,
                        &p,
                        message.clone(),
                    )?;
                    Action::Unknown
                }
            };
            prepared.push(Prepared {
                at: event.at,
                order: event.order,
                scope: event.scope.clone(),
                lookahead_end: match &action {
                    Action::Ramp(transition) => horizon_at(transition.end),
                    _ => end,
                },
                action,
                provenance: p,
            });
        }
        prepared.sort_by_key(|e| (e.at, e.order));
        Ok(prepared)
    }

    pub fn resolve(
        &self,
        owner: &ScoreVoice,
        pass: u32,
        end: Time,
        sounding_end: Time,
    ) -> Result<Timeline> {
        if end <= Time::ZERO {
            return Err("Score intensity timeline must have positive duration.".into());
        }
        self.resolve_bounded(
            owner,
            pass,
            end,
            sounding_end,
            ResolutionRoute::default(),
            &mut ProvenanceBudget::default(),
        )
    }

    fn resolve_bounded(
        &self,
        owner: &ScoreVoice,
        pass: u32,
        end: Time,
        sounding_end: Time,
        route: ResolutionRoute<'_>,
        budget: &mut ProvenanceBudget,
    ) -> Result<Timeline> {
        self.resolution_work(owner, end)?;
        let mut result = Timeline {
            segments: vec![Segment {
                start: Time::ZERO,
                end,
                curve: Curve::Absent,
                attack_only: false,
                provenance: None,
            }],
            ..Timeline::default()
        };
        let prepared =
            self.prepare(owner, pass, (end, sounding_end), route, &mut result, budget)?;
        let mut index = 0;
        while index < prepared.len() {
            let tick = prepared[index].at;
            let stop = index + prepared[index..].partition_point(|e| e.at == tick);
            let group = &prepared[index..stop];
            for entry in group {
                if let Action::Accent(level) = entry.action {
                    result.accents.push(Accent {
                        at: tick,
                        level,
                        provenance: budget.copy(&entry.provenance)?,
                    });
                }
            }
            let active: Vec<_> = group
                .iter()
                .filter(|e| !matches!(e.action, Action::Accent(_) | Action::Unknown))
                .collect();
            let priority = active.iter().map(|e| e.scope.priority()).max();
            let chosen: Vec<_> = active
                .into_iter()
                .filter(|e| Some(e.scope.priority()) == priority)
                .collect();
            let sets: Vec<_> = chosen
                .iter()
                .copied()
                .filter(|e| matches!(e.action, Action::Set(_)))
                .collect();
            let ramps: Vec<_> = chosen
                .iter()
                .copied()
                .filter(|e| matches!(e.action, Action::Ramp(_)))
                .collect();
            let next_instruction = prepared[stop..]
                .iter()
                .find(|e| !matches!(e.action, Action::Accent(_) | Action::Unknown))
                .map_or(end, |e| e.at);
            if sets.len() > 1 && sets.iter().any(|e| !same_set(&e.action, &sets[0].action)) {
                let mut p = budget.copy(&sets[0].provenance)?;
                for set in &sets[1..] {
                    budget.include(&mut p, &set.provenance)?;
                }
                result.issue(budget, tick, next_instruction, IssueKind::Conflict, &p, "Unrelated same-priority level declarations conflict; source order is retained, not used to choose a winner.")?;
                result.overlay(
                    Segment {
                        start: tick,
                        end,
                        curve: Curve::Held(None),
                        attack_only: false,
                        provenance: Some(p),
                    },
                    budget,
                )?;
            } else if let Some(set) = sets.first() {
                if result.segment_at(tick).is_some_and(|s| matches!(&s.curve, Curve::Transition { start, end, .. } if *start < tick && tick < *end)) {
                    result.issue(budget, tick, tick, IssueKind::Interrupted, &set.provenance, "An interior explicit dynamic stops the earlier transition at its authored level.")?;
                }
                let Action::Set(level) = set.action else {
                    unreachable!()
                };
                let mut p = budget.copy(&set.provenance)?;
                for duplicate in &sets[1..] {
                    budget.include(&mut p, &duplicate.provenance)?;
                }
                result.overlay(
                    Segment {
                        start: tick,
                        end,
                        curve: level.map_or(Curve::Held(None), Curve::Level),
                        attack_only: false,
                        provenance: Some(p),
                    },
                    budget,
                )?;
            }
            if sets.len() <= 1 || sets.iter().all(|e| same_set(&e.action, &sets[0].action)) {
                let mut unique: Vec<Prepared> = Vec::new();
                for ramp in ramps {
                    if let Some(same) = unique.iter_mut().find(|other| other.scope == ramp.scope && other.lookahead_end == ramp.lookahead_end && matches!((&other.action, &ramp.action), (Action::Ramp(a), Action::Ramp(b)) if a == b)) {
                        budget.include(&mut same.provenance, &ramp.provenance)?;
                    } else { unique.push(Prepared { provenance: budget.copy(&ramp.provenance)?, at: ramp.at, order: ramp.order, scope: ramp.scope.clone(), action: ramp.action.clone(), lookahead_end: ramp.lookahead_end }); }
                }
                if unique.len() == 1 {
                    self.apply_ramp(&mut result, &unique[0], &prepared[stop..], end, budget)?;
                } else if !unique.is_empty() {
                    self.apply_peer_ramps(&mut result, &unique, &prepared[stop..], end, budget)?;
                }
            }
            for entry in chosen
                .iter()
                .filter(|e| matches!(e.action, Action::Conflict(_)))
            {
                let Action::Conflict(span_end) = entry.action else {
                    unreachable!()
                };
                result.issue(budget, tick, span_end.min(end), IssueKind::Conflict, &entry.provenance,
                    "Recognized owning-spanner text conflicts with its hairpin direction; the authored span is ambiguous.")?;
                result.overlay(
                    Segment {
                        start: tick,
                        end: span_end.min(end),
                        curve: Curve::Held(None),
                        attack_only: false,
                        provenance: Some(budget.copy(&entry.provenance)?),
                    },
                    budget,
                )?;
            }

            index = stop;
        }
        for issue in result
            .issues
            .iter_mut()
            .filter(|i| i.kind == IssueKind::InvalidLevel && i.end > i.start)
        {
            if let Some(recovery) = result.segments.iter().find(|segment| {
                segment.start > issue.start
                    && segment.start < issue.end
                    && matches!(segment.curve, Curve::Level(_) | Curve::Transition { .. })
            }) {
                issue.end = recovery.start;
            }
        }
        result
            .accents
            .sort_by_key(|a| (a.at, a.provenance.scope.priority()));
        Ok(result)
    }

    fn resolve_ramp(
        &self,
        result: &mut Timeline,
        inherited: Option<&Segment>,
        event: &Prepared,
        following: &[Prepared],
        limit: Time,
        budget: &mut ProvenanceBudget,
    ) -> Result<Option<ResolvedRamp>> {
        let Action::Ramp(transition) = &event.action else {
            unreachable!()
        };
        let at = event.at;
        let mut p = budget.copy(&event.provenance)?;
        if transition.end <= at {
            result.issue(
                budget,
                at,
                at,
                IssueKind::UnresolvedSpan,
                &p,
                "Authored transition has no positive matched span.",
            )?;
            return Ok(None);
        }
        let held = inherited.map_or(Evaluation::Absent, |s| s.curve.evaluate(at));
        if let Some(previous) = inherited.as_ref().and_then(|s| s.provenance.as_ref()) {
            budget.include(&mut p, previous)?;
        }
        let start = if transition.niente_start {
            if transition.start.is_some_and(|v| v != Level::Silence)
                || inherited.as_ref().is_some_and(|s| {
                    s.start == at && matches!(s.curve, Curve::Level(Level::Positive(_)))
                })
            {
                result.issue(
                    budget,
                    at,
                    at,
                    IssueKind::Conflict,
                    &p,
                    "Explicit nonzero start conflicts with crescendo from niente.",
                )?;
            }
            Level::Silence
        } else if let Some(start) = transition.start {
            start
        } else if let Some(Segment {
            curve: Curve::Level(level),
            ..
        }) = inherited
        {
            *level
        } else {
            match held {
                Evaluation::Known(Intensity::Silence) => Level::Silence,
                Evaluation::Known(Intensity::Decibels(db)) => {
                    // A ramp may start within another eased transition. Preserve
                    // that evaluated value rather than snapping it to a table rung.
                    match evaluated_level(db) {
                        Ok(level) => level,
                        Err(message) => {
                            result.issue(
                                budget,
                                at,
                                transition.end.min(limit),
                                IssueKind::InvalidLevel,
                                &p,
                                message,
                            )?;
                            return Ok(None);
                        }
                    }
                }
                Evaluation::Absent => {
                    let value = Fraction::integer(80);
                    p.describe(
                        "start",
                        Basis::DeclaredDefault,
                        Some(value),
                        "Authored transition without a prior level starts at mf/L80.",
                    );
                    Level::Positive(value)
                }
                Evaluation::Unknown => {
                    result.issue(
                        budget,
                        at,
                        transition.end.min(limit),
                        IssueKind::UnknownAnchor,
                        &p,
                        "Conflicting prior state is unknown; it cannot be defaulted to mf.",
                    )?;
                    return Ok(Some(ResolvedRamp {
                        segment: Segment {
                            start: at,
                            end: transition.end.min(limit),
                            curve: Curve::Held(None),
                            attack_only: false,
                            provenance: Some(p),
                        },
                        written_end: transition.end,
                        boundary: None,
                    }));
                }
            }
        };
        let exact_end = following.iter().find(|next| {
            next.at == transition.end
                && next.at <= event.lookahead_end
                && next.scope == event.scope
                && matches!(next.action, Action::Set(Some(_)))
        });
        let next = following.iter().find(|next| {
            next.at >= transition.end
                && next.at <= limit.min(event.lookahead_end)
                && next.scope == event.scope
                && !matches!(next.action, Action::Accent(_))
        });
        // Any state-level instruction inside/after the ramp bounds look-ahead.
        let interior = following.iter().any(|next| {
            next.at < transition.end
                && next.scope == event.scope
                && !matches!(next.action, Action::Accent(_))
        });
        let mut explicit = exact_end.or(next.filter(|_| !interior)).and_then(|next| {
            if let Action::Set(Some(level)) = next.action {
                Some((level, next))
            } else {
                None
            }
        });
        if let Some((_, endpoint)) = explicit {
            budget.include(&mut p, &endpoint.provenance)?;
        }
        if let Some((selected, endpoint)) = explicit {
            let alternatives: Vec<_> = following
                .iter()
                .filter(|candidate| {
                    candidate.at == endpoint.at
                        && candidate.scope == event.scope
                        && matches!(candidate.action, Action::Set(_))
                })
                .collect();
            if alternatives.iter().any(|candidate| !matches!(candidate.action, Action::Set(Some(level)) if level == selected)) {
                for candidate in alternatives { budget.include(&mut p, &candidate.provenance)?; }
                result.issue(budget, at, transition.end, IssueKind::Conflict, &p,
                    "Conflicting endpoint declarations cannot choose a ramp target by source order; adjacent-rung fallback applies.")?;
                explicit = None;
            }
        }
        let written_endpoint = transition.end_level.or(explicit.map(|(level, _)| level));
        let instance_delta = transition.velo_change.filter(|v| *v != Fraction::ZERO);
        let target = if transition.niente_end {
            if written_endpoint.is_some_and(|v| v != Level::Silence) {
                result.issue(budget, at, transition.end, IssueKind::Conflict, &p, "Niente taper conflicts with a nonzero written endpoint; both instructions remain retained.")?;
            }
            Some(Level::Silence)
        } else if let Some(delta) = instance_delta {
            let signed = match transition.direction {
                Direction::Crescendo => {
                    if delta < Fraction::ZERO {
                        Fraction::ZERO.checked_sub(delta)?
                    } else {
                        delta
                    }
                }
                Direction::Diminuendo => {
                    if delta > Fraction::ZERO {
                        Fraction::ZERO.checked_sub(delta)?
                    } else {
                        delta
                    }
                }
            };
            let value = start.number().checked_add(signed)?;
            p.describe(
                "end",
                Basis::ExplicitNumeric,
                Some(value),
                "Instance HairPin veloChange uses the signed magnitude under the portable policy.",
            );
            match Level::positive(value) {
                Ok(level) => {
                    if written_endpoint.is_some_and(|v| v != level) {
                        result.issue(budget, at, transition.end, IssueKind::Conflict, &p, "Instance delta endpoint differs from written end dynamic; ramp then boundary step.")?;
                    }
                    Some(level)
                }
                Err(message) => {
                    result.issue(
                        budget,
                        at,
                        transition.end,
                        IssueKind::InvalidLevel,
                        &p,
                        message,
                    )?;
                    None
                }
            }
        } else if let Some(endpoint) = written_endpoint {
            if direction_matches(start, endpoint, transition.direction) {
                Some(endpoint)
            } else {
                result.issue(budget, at, transition.end, IssueKind::DirectionMismatch, &p, "Wrong-direction endpoint: use adjacent rung during span and retain the written boundary mark.")?;
                fallback_rung(start, transition.direction)
            }
        } else {
            let inferred = fallback_rung(start, transition.direction);
            if let Some(level) = inferred {
                p.describe("end", Basis::InferredEndpoint, Some(level.number()), "No usable endpoint before another state instruction or repeat jump: adjacent standard-table rung.");
            }
            inferred
        };
        let Some(target) = target else {
            if instance_delta.is_none() {
                result.issue(
                    budget,
                    at,
                    transition.end,
                    IssueKind::RangeExhausted,
                    &p,
                    "No adjacent rung remains in the bounded standard range.",
                )?;
            }
            return Ok(None);
        };
        let easing = transition
            .method
            .as_deref()
            .and_then(Easing::named)
            .unwrap_or(Easing::Normal);
        if transition
            .method
            .as_deref()
            .is_some_and(|m| Easing::named(m).is_none())
        {
            p.describe(
                "method",
                Basis::PortableInterpretation,
                None,
                "Unknown method retained; active normal ramp fallback.",
            );
            result.issue(
                budget,
                at,
                transition.end,
                IssueKind::EasingFallback,
                &p,
                "Unknown veloChangeMethod uses the active normal ramp.",
            )?;
        }
        let domain = if start == Level::Silence || target == Level::Silence {
            Domain::LinearGain
        } else {
            Domain::RelativeDecibels
        };
        let curve = Curve::Transition {
            start: at,
            end: transition.end,
            from: start,
            to: target,
            easing,
            domain,
        };
        let attack_only = transition.single_note_dynamics == Some(false);
        let actual_end = transition.end.min(limit);
        let boundary = transition.end_level.unwrap_or(target);
        // Keep the exact original phase if this occurrence ends at a jump.
        let segment = Segment {
            start: at,
            end: actual_end,
            curve,
            attack_only,
            provenance: Some(p),
        };
        Ok(Some(ResolvedRamp {
            segment,
            written_end: transition.end,
            boundary: Some(boundary),
        }))
    }

    fn apply_peer_ramps(
        &self,
        result: &mut Timeline,
        peers: &[Prepared],
        following: &[Prepared],
        limit: Time,
        budget: &mut ProvenanceBudget,
    ) -> Result<()> {
        let at = peers[0].at;
        let inherited = result
            .segment_at(at)
            .map(|s| copy_segment(s, budget))
            .transpose()?;
        budget.charge(
            (peers.len() + 1).saturating_mul(
                std::mem::size_of::<ResolvedRamp>() + 2 * std::mem::size_of::<Time>(),
            ),
            peers.len().saturating_mul(peers.len() + 1),
        )?;
        let mut resolved = Vec::new();
        // Resolve every peer before any overlay. In particular, an earlier
        // peer's conflict is not an unknown starting anchor for later peers.
        for peer in peers {
            if let Some(ramp) =
                self.resolve_ramp(result, inherited.as_ref(), peer, following, limit, budget)?
            {
                resolved.push(ramp);
            }
        }
        // An already-running transition participates only over its remaining
        // span; its untouched tail and held endpoint still belong to it.
        if let Some(previous) = inherited
            .filter(|s| s.end > at && matches!(s.curve, Curve::Transition { end, .. } if end > at))
        {
            let boundary = result.segment_at(previous.end).and_then(|s| match s.curve {
                Curve::Level(level) => Some(level),
                _ => None,
            });
            let Curve::Transition {
                end: written_end, ..
            } = previous.curve
            else {
                unreachable!()
            };
            resolved.push(ResolvedRamp {
                segment: previous,
                written_end,
                boundary,
            });
        }
        let mut cuts = vec![at];
        cuts.extend(resolved.iter().map(|r| r.segment.end));
        cuts.sort();
        cuts.dedup();
        for interval in cuts.windows(2) {
            let (start, end) = (interval[0], interval[1]);
            let active: Vec<_> = resolved.iter().filter(|r| r.segment.end > start).collect();
            let Some(first) = active.first() else {
                continue;
            };
            let mut segment = copy_segment(&first.segment, budget)?;
            segment.start = start;
            segment.end = end;
            for other in &active[1..] {
                if let (Some(p), Some(other)) = (&mut segment.provenance, &other.segment.provenance)
                {
                    budget.include(p, other)?;
                }
            }
            let compatible = active.iter().all(|r| {
                r.segment.curve == first.segment.curve
                    && r.segment.attack_only == first.segment.attack_only
            });
            if !compatible {
                segment.curve = Curve::Held(None);
                segment.attack_only = false;
                result.issue(budget, start, end, IssueKind::Conflict,
                    segment.provenance.as_ref().expect("resolved ramp provenance"),
                    "Simultaneous transitions conflict only in their shared active span; surviving contours keep their original phase.")?;
            }
            result.overlay(segment, budget)?;
        }
        let Some(end) = cuts.last().copied().filter(|end| *end > at) else {
            return Ok(());
        };
        let last: Vec<_> = resolved.iter().filter(|r| r.segment.end == end).collect();
        if let Some(first) = last.first() {
            if let Some(boundary) = first.boundary.filter(|boundary| {
                last.iter()
                    .all(|r| r.written_end <= limit && r.boundary == Some(*boundary))
            }) {
                let mut held = copy_segment(&first.segment, budget)?;
                for other in &last[1..] {
                    if let (Some(p), Some(other)) =
                        (&mut held.provenance, &other.segment.provenance)
                    {
                        budget.include(p, other)?;
                    }
                }
                held.start = end;
                held.end = limit;
                held.curve = Curve::Level(boundary);
                // Incompatible attack-only semantics cannot pick a held winner.
                if last
                    .iter()
                    .all(|r| r.segment.attack_only == held.attack_only)
                {
                    result.overlay(held, budget)?;
                }
            }
        }
        // Equal-end contradictory peers restore the prior held context, as in
        // the existing same-end policy; no authored endpoint wins by order.
        Ok(())
    }

    fn apply_ramp(
        &self,
        result: &mut Timeline,
        event: &Prepared,
        following: &[Prepared],
        limit: Time,
        budget: &mut ProvenanceBudget,
    ) -> Result<()> {
        let inherited = result
            .segment_at(event.at)
            .map(|s| copy_segment(s, budget))
            .transpose()?;
        let Some(resolved) =
            self.resolve_ramp(result, inherited.as_ref(), event, following, limit, budget)?
        else {
            return Ok(());
        };
        let ResolvedRamp {
            segment,
            written_end,
            boundary,
        } = resolved;
        let Some(boundary) = boundary else {
            return result.overlay(segment, budget);
        };
        let at = event.at;
        let actual_end = segment.end;
        let attack_only = segment.attack_only;
        let p = budget.copy(
            segment
                .provenance
                .as_ref()
                .expect("resolved ramp provenance"),
        )?;
        let previous_ramp =
            inherited.filter(|s| matches!(&s.curve, Curve::Transition { end, .. } if *end > at));
        if let Some(previous) = previous_ramp {
            let overlap_end = previous.end.min(actual_end);
            let mut conflict = budget.copy(&p)?;
            if let Some(old) = &previous.provenance {
                budget.include(&mut conflict, old)?;
            }
            result.issue(budget, at, overlap_end, IssueKind::Conflict, &conflict, "Overlapping transitions conflict locally; unaffected portions retain their original phase.")?;
            result.overlay(segment, budget)?;
            result.overlay(
                Segment {
                    start: at,
                    end: overlap_end,
                    curve: Curve::Held(None),
                    attack_only: false,
                    provenance: Some(conflict),
                },
                budget,
            )?;
            // If the earlier ramp lasts longer, its unaffected tail survives.
            if previous.end > actual_end {
                result.overlay(
                    Segment {
                        start: actual_end,
                        ..previous
                    },
                    budget,
                )?;
            } else if actual_end < limit {
                result.overlay(
                    Segment {
                        start: actual_end,
                        end: limit,
                        curve: Curve::Level(boundary),
                        attack_only,
                        provenance: Some(p),
                    },
                    budget,
                )?;
            }
        } else {
            result.overlay(segment, budget)?;
            if written_end <= limit {
                result.overlay(
                    Segment {
                        start: actual_end,
                        end: limit,
                        curve: Curve::Level(boundary),
                        attack_only,
                        provenance: Some(p),
                    },
                    budget,
                )?;
            }
        }
        Ok(())
    }

    /// Reevaluate destination state for each applicable pass. No final-bar level
    /// is carried backwards. Callers share the returned Arc across sibling lanes.
    pub fn occurrences(
        &self,
        owner: &ScoreVoice,
        runs: &[Occurrence],
        sounding_ends: &[Time],
    ) -> Result<Arc<Timeline>> {
        self.occurrences_bounded(owner, runs, sounding_ends, &mut ProvenanceBudget::default())
    }

    pub(crate) fn occurrences_bounded(
        &self,
        owner: &ScoreVoice,
        runs: &[Occurrence],
        sounding_ends: &[Time],
        budget: &mut ProvenanceBudget,
    ) -> Result<Arc<Timeline>> {
        self.occurrences_with_input_bounded(owner, runs, sounding_ends, None, budget)
    }

    fn occurrences_with_input_bounded(
        &self,
        owner: &ScoreVoice,
        runs: &[Occurrence],
        sounding_ends: &[Time],
        input: Option<&source::ScoreInput>,
        budget: &mut ProvenanceBudget,
    ) -> Result<Arc<Timeline>> {
        if runs.len() != sounding_ends.len() {
            return Err("Each occurrence needs its final source sounding end.".into());
        }
        self.occurrence_work(owner, runs)?;
        let mut result = Timeline::default();
        let mut previous_end = None;
        // The prefix is the route actually played, with its suffix discarded at
        // a backward jump. A forward skip leaves a hole; written declarations in
        // that hole cannot supply held state or look-ahead endpoints. Retaining
        // the visited prefix lets a destination inside a ramp recover its phase.
        let mut path: Vec<PlaybackSpan> = Vec::new();
        for (index, (run, sounding_end)) in runs.iter().zip(sounding_ends).enumerate() {
            if run.written_start < Time::ZERO || run.written_end <= run.written_start {
                return Err("Invalid written occurrence span.".into());
            }
            if run.performed_start < Time::ZERO
                || previous_end.is_some_and(|end| run.performed_start < end)
            {
                return Err("Performed occurrence spans overlap or start before zero.".into());
            }
            previous_end = Some(
                run.performed_start
                    .checked_add(run.written_end.checked_sub(run.written_start)?)?,
            );
            let backward = path.last().is_some_and(|last| run.written_start < last.end);
            let keep = path.partition_point(|span| span.start < run.written_start);
            path.truncate(keep);
            if let Some(last) = path.last_mut() {
                last.end = last.end.min(run.written_start);
            }
            if backward {
                budget.charge(0, path.len())?;
                for span in &mut path {
                    span.pass = run.pass;
                }
            }
            if let Some(last) = path
                .last_mut()
                .filter(|last| last.end == run.written_start && last.pass == run.pass)
            {
                last.end = run.written_end;
            } else {
                budget.charge(std::mem::size_of::<PlaybackSpan>(), 1)?;
                path.push(PlaybackSpan {
                    start: run.written_start,
                    end: run.written_end,
                    pass: run.pass,
                });
            }
            let offset = run.performed_start.checked_sub(run.written_start)?;
            let timeline = self.resolve_bounded(
                owner,
                run.pass,
                run.written_end,
                *sounding_end,
                ResolutionRoute {
                    path: Some((
                        &path,
                        index + 1 == runs.len() || runs[index + 1].written_start == run.written_end,
                    )),
                    input,
                },
                budget,
            )?;
            for mut segment in timeline.segments {
                let final_boundary = index + 1 == runs.len()
                    && segment.start == run.written_end
                    && segment.end == segment.start;
                if !final_boundary
                    && (segment.end <= run.written_start || segment.start >= run.written_end)
                {
                    continue;
                }
                segment.start = segment.start.max(run.written_start).checked_add(offset)?;
                segment.end = segment.end.min(run.written_end).checked_add(offset)?;
                if let Curve::Transition { start, end, .. } = &mut segment.curve {
                    *start = start.checked_add(offset)?;
                    *end = end.checked_add(offset)?;
                }
                if let Some(p) = &mut segment.provenance {
                    p.occurrence = run.ordinal;
                }
                result.segments.push(segment);
            }
            for mut accent in timeline.accents {
                if accent.at < run.written_start || accent.at >= run.written_end {
                    continue;
                }
                accent.at = accent.at.checked_add(offset)?;
                accent.provenance.occurrence = run.ordinal;
                result.accents.push(accent);
            }
            for issue in timeline.issues {
                let bytes = provenance_size(&issue.provenance).saturating_add(issue.message.len());
                budget.charge(bytes, bytes / 8 + 1)?;
                if let Some(mapped) = performed_issue(&issue, run, index + 1 == runs.len())? {
                    result.issues.push(mapped);
                }
            }
        }
        result.segments.sort_by_key(|s| s.start);
        result
            .accents
            .sort_by_key(|a| (a.at, a.provenance.scope.priority()));
        Ok(Arc::new(result))
    }
}

fn is_ordinary(d: &Dynamic) -> bool {
    d.velo_change.is_none_or(|v| v == Fraction::ZERO)
        && (d.numeric.is_some()
            || d.symbol
                .as_deref()
                .is_some_and(|s| standard_level(s).is_some()))
}
fn same_set(a: &Action, b: &Action) -> bool {
    matches!((a, b), (Action::Set(left), Action::Set(right)) if left == right)
}
fn evaluated_level(db: f64) -> Result<Level> {
    // Only an evaluated continuous interior needs a decimal reconstruction.
    // Original exact endpoints and raw values stay in the parent provenance.
    Level::positive(Fraction::decimal(&format!("{:.12}", db * 4.0 + 80.0))?)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttackVelocity {
    Invalid(String),
    /// MIDI NoteOn velocity is admitted here only when nonzero. NoteOff and
    /// release velocity never call this contributor.
    Midi(u8),
    MusicXml(Fraction),
    MuseScoreUser {
        value: Fraction,
        legacy: bool,
    },
    MuseScoreOffset {
        percent: Fraction,
        legacy: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VelocityEvidence {
    pub value: AttackVelocity,
    pub evidence: Evidence,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteChain {
    /// Head identity plus occurrence; shared across phonemes and sibling lanes.
    pub source_id: String,
    pub start: Time,
    pub end: Time,
    pub owner: ScoreVoice,
    pub occurrence: u32,
    pub pass: u32,
    pub velocity: Option<VelocityEvidence>,
    pub continuation_velocities: Vec<VelocityEvidence>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoteIntensity {
    pub source_id: String,
    pub start: Time,
    pub end: Time,
    pub timeline: Arc<Timeline>,
    pub provenance: Option<Provenance>,
    pub issues: Vec<Issue>,
    /// Absolute intensity at the actual source attack, composed only once.
    anchor: Option<Intensity>,
    score_at_attack: Evaluation,
    anchor_reference: Evaluation,
    attack_only_reference: Evaluation,
    invalid_anchor: bool,
}

fn velocity_level(
    velocity: &AttackVelocity,
    score: Evaluation,
    p: &mut Provenance,
) -> Result<Option<Level>> {
    let level = match velocity {
        AttackVelocity::Invalid(reason) => return Err(reason.clone()),
        AttackVelocity::Midi(0) => return Ok(None),
        AttackVelocity::Midi(v) => Fraction::integer(i64::from(*v)),
        AttackVelocity::MusicXml(v) => {
            let value = v.checked_mul(Fraction::new(9, 10)?)?;
            p.describe(
                "note@dynamics",
                Basis::ExplicitNumeric,
                Some(value),
                "MusicXML note-owned absolute attack override.",
            );
            if value == Fraction::ZERO {
                return Ok(Some(Level::Silence));
            }
            value
        }
        AttackVelocity::MuseScoreUser { value, legacy } => {
            if *legacy && *value == Fraction::ZERO {
                p.describe(
                    "Note/velocity",
                    Basis::SourceArithmetic,
                    Some(Fraction::ONE),
                    "Pinned legacy user velocity zero clamps to one, not silence.",
                );
                return Ok(Some(Level::Positive(Fraction::ONE)));
            }
            if !legacy && *value == Fraction::ZERO {
                p.describe(
                    "Note/velocity",
                    Basis::SourceArithmetic,
                    Some(*value),
                    "Modern zero means no attack override in the traced playback context.",
                );
                return Ok(None);
            }
            *value
        }
        AttackVelocity::MuseScoreOffset { percent, legacy } => {
            let base = match score {
                Evaluation::Known(Intensity::Decibels(db)) => evaluated_level(db)?.number(),
                Evaluation::Known(Intensity::Silence) => return Ok(Some(Level::Silence)),
                Evaluation::Unknown => {
                    return Err(
                        "Unknown score level cannot anchor an explicit velocity offset.".into(),
                    )
                }
                Evaluation::Absent => {
                    let level = Fraction::integer(80);
                    p.describe("offset reference", Basis::DeclaredDefault, Some(level), "Explicit note offset without score context uses mf/L80; absence alone creates no automation.");
                    level
                }
            };
            if *legacy {
                if percent.denominator != 1 {
                    return Err("Legacy velocity offset must be an integer percentage.".into());
                }
                // MuseScore 3.6.2 note.cpp:2451-2458: velo +
                // (velo * veloOffset()) / 100, integer division toward zero.
                let base = base.numerator / base.denominator;
                let changed = base
                    .checked_mul(percent.numerator)
                    .and_then(|delta| base.checked_add(delta / 100))
                    .ok_or("Velocity offset overflow.")?;
                let changed = i64::try_from(changed).map_err(|_| "Velocity offset overflow.")?;
                // Only the source's explicitly documented zero floor is applied;
                // levels above the frozen 127 domain remain representation issues.
                let changed = if changed <= 0 { 1 } else { changed };
                let value = Fraction::integer(changed);
                p.describe(
                    "offset",
                    Basis::SourceArithmetic,
                    Some(value),
                    "Legacy integer percentage arithmetic, truncating the delta toward zero.",
                );
                value
            } else {
                base.checked_add(base.checked_mul(percent.checked_mul(Fraction::new(1, 100)?)?)?)?
            }
        }
    };
    p.describe(
        "attack level",
        Basis::ExplicitNumeric,
        Some(level),
        "One absolute attack contributor; no second copy of the score level.",
    );
    Level::positive(level).map(Some)
}

impl NoteIntensity {
    /// Reserve every owned copy before extending a selected tie context. The
    /// immutable timeline remains shared; raw and selected bindings use the
    /// same caller-owned budget, including issues which survive recovery.
    pub(crate) fn continued_from_bounded(
        head: &Self,
        tail: Option<&Self>,
        budget: &mut ProvenanceBudget,
    ) -> Result<Self> {
        let bytes = std::mem::size_of::<Self>().saturating_add(head.source_id.len());
        budget.charge(bytes, bytes / 8 + 1)?;
        if let Some(provenance) = &head.provenance {
            let bytes = provenance_size(provenance);
            budget.charge(bytes, bytes / 8 + 1)?;
        }
        for issue in &head.issues {
            budget.reserve_issue(issue)?;
        }
        if let Some(tail) = tail {
            for issue in tail
                .issues
                .iter()
                .filter(|issue| issue.kind != IssueKind::UnknownAnchor)
            {
                budget.reserve_issue(issue)?;
            }
            if tail.anchor.is_some() {
                if let Some(provenance) = &tail.provenance {
                    let bytes = provenance_size(provenance).saturating_add(256);
                    budget.charge(bytes, bytes / 8 + 1)?;
                }
            }
        }
        Ok(Self::continued_from(head, tail))
    }

    /// A separately retained, source-proven tie tail shares the original attack.
    /// Its owning PerformanceNote still identifies that tail, not this chain head.
    pub fn continued_from(head: &Self, tail: Option<&Self>) -> Self {
        let mut continued = head.clone();
        if let Some(tail) = tail {
            continued.end = continued.end.max(tail.end);
            continued.refresh_permitted_attack_references();
            continued.issues.extend(
                tail.issues
                    .iter()
                    .filter(|issue| issue.kind != IssueKind::UnknownAnchor)
                    .cloned(),
            );
            if matches!(
                continued.anchor_reference,
                Evaluation::Known(Intensity::Decibels(_))
            ) {
                continued
                    .issues
                    .retain(|issue| issue.kind != IssueKind::UnknownAnchor);
            }
            if tail.anchor.is_some() {
                if let Some(provenance) = &tail.provenance {
                    continued.issues.push(Issue { start:tail.start,end:tail.end,
                        kind:if tail.anchor != head.anchor { IssueKind::Conflict } else { IssueKind::ContinuationVelocity },provenance:provenance.clone(),
                        message:"Continuation velocity is retained but does not re-anchor a source-proven tie chain".into() });
                }
            }
        }
        continued
    }
    fn refresh_permitted_attack_references(&mut self) {
        (self.anchor_reference, self.attack_only_reference) =
            self.permitted_attack_references(self.end);
    }

    /// Validate payload inheritance only; the caller must independently prove
    /// the source tie and predecessor. Issues and chain end may legitimately
    /// differ. No timeline/provenance copy or mutable validity flag is involved.
    pub(crate) fn is_continuation_of_bounded(
        &self,
        head: &Self,
        budget: &mut ProvenanceBudget,
    ) -> Result<bool> {
        budget.charge(
            0,
            self.source_id
                .len()
                .saturating_add(head.source_id.len())
                .saturating_add(8),
        )?;
        if self.source_id != head.source_id
            || self.start != head.start
            || self.end < head.end
            || !Arc::ptr_eq(&self.timeline, &head.timeline)
            || self.anchor != head.anchor
            || self.score_at_attack != head.score_at_attack
            || self.invalid_anchor != head.invalid_anchor
        {
            return Ok(false);
        }
        if head.score_at_attack == Evaluation::Absent {
            let depth =
                usize::BITS as usize - head.timeline.segments.len().leading_zeros() as usize + 1;
            budget.charge(0, depth.saturating_mul(3))?;
            let first = head
                .timeline
                .segments
                .partition_point(|s| s.end <= head.start);
            let end = head
                .timeline
                .segments
                .partition_point(|s| s.start < self.end)
                .max(first);
            // Two reference scans and one metadata scan, only on the path that
            // actually reads segments. Known attacks remain constant work.
            budget.charge(0, (end - first).saturating_mul(3))?;
            if let Some(p) = head.timeline.segments[first..end]
                .iter()
                .find(|s| !matches!(s.curve, Curve::Absent))
                .filter(|s| matches!(s.curve, Curve::Transition { .. }))
                .and_then(|s| s.provenance.as_ref())
            {
                budget.charge(0, p.interpretations.len().saturating_mul(8))?;
            }
        }
        let (anchor, attack_only) = head.permitted_attack_references(self.end);
        Ok(self.anchor_reference == anchor && self.attack_only_reference == attack_only)
    }

    #[cfg(test)]
    fn is_continuation_of(&self, head: &Self) -> bool {
        self.is_continuation_of_bounded(head, &mut ProvenanceBudget::default())
            .unwrap()
    }

    fn permitted_attack_references(&self, end: Time) -> (Evaluation, Evaluation) {
        let mut anchor = self.score_at_attack;
        let mut attack_only = self.score_at_attack;
        if self.score_at_attack == Evaluation::Absent {
            let first = self
                .timeline
                .segments
                .partition_point(|s| s.end <= self.start);
            let future = &self.timeline.segments[first..];
            if let Some(segment) = future
                .iter()
                .take_while(|s| s.start < end)
                .find(|s| s.attack_only && !matches!(s.curve, Curve::Absent))
            {
                attack_only = segment.curve.evaluate(segment.start.max(self.start));
            }
            // Only reuse an existing permitted transition-start default. A mute,
            // unknown value or an ordinary future mark supplies no earlier anchor.
            if let Some(segment) = future
                .iter()
                .take_while(|s| s.start < end)
                .find(|s| !matches!(s.curve, Curve::Absent))
            {
                let permitted = matches!(segment.curve, Curve::Transition { .. })
                    && segment.provenance.as_ref().is_some_and(|p| {
                        p.interpretations.iter().any(|i| {
                            i.field == "start"
                                && i.basis == Basis::DeclaredDefault
                                && i.exact == Some(Fraction::integer(80))
                        })
                    });
                if permitted {
                    anchor = segment.curve.evaluate(segment.start);
                }
            }
        }
        (anchor, attack_only)
    }

    /// Native MIDI attacks use the same level policy without fabricating score scope.
    pub fn midi_attack(
        source_id: String,
        start: Time,
        end: Time,
        velocity: u8,
        port: u8,
        channel: u8,
        event_id: String,
    ) -> Result<Self> {
        let evidence = Evidence {
            source_ids: vec![format!("expression:{event_id}:velocity")],
            contract: SourceContract::Midi,
            saving_version: None,
            layout: "nonzero NoteOn".into(),
            raw_fields: BTreeMap::from([
                ("velocity".into(), velocity.to_string()),
                ("source_event_id".into(), event_id),
            ]),
        };
        let mut provenance = Provenance {
            policy: POLICY,
            scope: Scope::Midi { port, channel },
            occurrence: 0,
            repeat_pass: 0,
            evidence: vec![evidence],
            interpretations: vec![],
        };
        let anchor = velocity_level(
            &AttackVelocity::Midi(velocity),
            Evaluation::Absent,
            &mut provenance,
        )?
        .map(Level::value);
        Ok(Self {
            source_id,
            start,
            end,
            timeline: Arc::new(Timeline::default()),
            provenance: Some(provenance),
            issues: vec![],
            anchor,
            score_at_attack: Evaluation::Absent,
            anchor_reference: Evaluation::Absent,
            attack_only_reference: Evaluation::Absent,
            invalid_anchor: false,
        })
    }
    /// `attacks` is sorted once per applicable source scope, before lane/phoneme
    /// splitting. Tied continuation ticks are excluded; equal chord attacks may
    /// be present. Timeline accents are ordered by time and scope priority.
    pub fn resolve(chain: &NoteChain, timeline: Arc<Timeline>, attacks: &[Time]) -> Result<Self> {
        Self::resolve_bounded(chain, timeline, attacks, &mut ProvenanceBudget::default())
    }
    pub(crate) fn resolve_bounded(
        chain: &NoteChain,
        timeline: Arc<Timeline>,
        attacks: &[Time],
        budget: &mut ProvenanceBudget,
    ) -> Result<Self> {
        if chain.end <= chain.start {
            return Err("Attack/tie chain needs positive duration.".into());
        }
        let score_at_attack = timeline.evaluate(chain.start);
        let mut note = Self {
            source_id: chain.source_id.clone(),
            start: chain.start,
            end: chain.end,
            timeline,
            provenance: None,
            issues: Vec::new(),
            anchor: None,
            score_at_attack,
            anchor_reference: score_at_attack,
            attack_only_reference: score_at_attack,
            invalid_anchor: false,
        };
        note.refresh_permitted_attack_references();
        let first = note
            .timeline
            .segments
            .partition_point(|s| s.end <= chain.start);
        let future = &note.timeline.segments[first..];
        // These sorted source attack and accent arrays are built once per owner.
        // Only accents since the previous distinct attack can belong to this one.
        let range = note.timeline.accent_range(attacks, chain.start);
        // Transient accents affect the next attack (all chord members share that
        // attack), then disappear. They never replace persistent score context.
        let accents: Vec<_> = note.timeline.accents[range]
            .iter()
            .filter(|accent| {
                accent.at <= chain.start
                    && accent.provenance.repeat_pass == chain.pass
                    && accent.provenance.scope.applies(&chain.owner)
                    && (accent.provenance.occurrence == 0
                        || accent.provenance.occurrence == chain.occurrence)
            })
            .collect();
        if let Some(accent) = accents.last() {
            if accents.iter().any(|other| {
                other.level != accent.level
                    && other.at == accent.at
                    && other.provenance.scope.priority() == accent.provenance.scope.priority()
            }) {
                note.invalid_anchor = true;
                note.issues.push(Issue {
                    start: chain.start,
                    end: chain.end,
                    kind: IssueKind::Conflict,
                    provenance: budget.copy(&accent.provenance)?,
                    message: "Conflicting next-attack accents are not arbitrarily ordered.".into(),
                });
            } else {
                note.anchor = Some(accent.level.value());
                note.provenance = Some(budget.copy(&accent.provenance)?);
            }
        }
        if let Some(velocity) = &chain.velocity {
            let bytes = evidence_size(&velocity.evidence)
                .saturating_add(chain.owner.part.len())
                .saturating_add(chain.owner.staff.len())
                .saturating_add(chain.owner.voice.len())
                .saturating_add(
                    strings_size(&velocity.evidence.source_ids)
                        .saturating_add(512)
                        .saturating_mul(12),
                );
            budget.charge(bytes, bytes / 8 + 1)?;
            let mut p = Provenance {
                policy: POLICY,
                scope: Scope::Voice {
                    part: chain.owner.part.clone(),
                    staff: Some(chain.owner.staff.clone()),
                    voice: chain.owner.voice.clone(),
                },
                occurrence: chain.occurrence,
                repeat_pass: chain.pass,
                evidence: vec![velocity.evidence.clone()],
                interpretations: Vec::new(),
            };
            if let Some(score_p) = note
                .timeline
                .segment_at(chain.start)
                .and_then(|s| s.provenance.as_ref())
            {
                budget.include(&mut p, score_p)?;
            }
            match velocity_level(&velocity.value, score_at_attack, &mut p) {
                Ok(Some(level)) => {
                    note.anchor = Some(level.value());
                    note.invalid_anchor = false;
                }
                Ok(None) => {
                    if let Some(active) = &note.provenance {
                        budget.include(&mut p, active)?;
                    }
                }
                Err(message) => {
                    note.invalid_anchor = true;
                    note.issues.push(Issue {
                        start: chain.start,
                        end: chain.end,
                        kind: IssueKind::InvalidLevel,
                        provenance: budget.copy(&p)?,
                        message,
                    });
                }
            }
            if note.anchor_reference != score_at_attack {
                if let Some(reference) = future
                    .iter()
                    .find(|s| !matches!(s.curve, Curve::Absent))
                    .and_then(|s| s.provenance.as_ref())
                {
                    budget.include(&mut p, reference)?;
                }
            }
            note.provenance = Some(p);
        }
        for continuation in &chain.continuation_velocities {
            if chain
                .velocity
                .as_ref()
                .is_none_or(|head| head.value != continuation.value)
            {
                let mut p = note
                    .provenance
                    .as_ref()
                    .map(|p| budget.copy(p))
                    .transpose()?
                    .unwrap_or_else(|| Provenance {
                        policy: POLICY,
                        scope: Scope::Voice {
                            part: chain.owner.part.clone(),
                            staff: Some(chain.owner.staff.clone()),
                            voice: chain.owner.voice.clone(),
                        },
                        occurrence: chain.occurrence,
                        repeat_pass: chain.pass,
                        evidence: Vec::new(),
                        interpretations: Vec::new(),
                    });
                let bytes = evidence_size(&continuation.evidence);
                budget.charge(bytes, bytes / 8 + 1)?;
                p.evidence.push(continuation.evidence.clone());
                note.issues.push(Issue { start: chain.start, end: chain.end, kind: IssueKind::ContinuationVelocity,
                    provenance: p, message: "Tied continuation velocity retained and reported; the tie chain does not re-anchor.".into() });
            }
        }
        if note.anchor.is_some()
            && !note.invalid_anchor
            && !matches!(
                note.anchor_reference,
                Evaluation::Known(Intensity::Decibels(_))
            )
        {
            for segment in future
                .iter()
                .take_while(|s| s.start < chain.end)
                .filter(|s| {
                    matches!(
                        s.curve.evaluate(s.start.max(chain.start)),
                        Evaluation::Known(Intensity::Decibels(_))
                    )
                })
            {
                if let Some(p) = &note.provenance {
                    note.issues.push(Issue { start: segment.start.max(chain.start), end: segment.end.min(chain.end),
                        kind: IssueKind::UnknownAnchor, provenance: budget.copy(p)?,
                        message: "Positive score context has no finite permitted attack reference; this adjustment is unresolved".into() });
                }
            }
        }
        Ok(note)
    }

    pub fn evaluate(&self, at: Time) -> Evaluation {
        self.evaluate_segment(at, self.timeline.segment_at(at))
    }
    pub(crate) fn evaluate_segment(&self, at: Time, segment: Option<&Segment>) -> Evaluation {
        if at < self.start || at > self.end {
            return Evaluation::Absent;
        }
        let mut score = segment.map_or(Evaluation::Absent, |s| s.curve.evaluate(at));
        // Even a positive absolute anchor cannot undo an explicit score mute.
        if score == Evaluation::Known(Intensity::Silence) {
            return score;
        }
        if self.invalid_anchor {
            return Evaluation::Unknown;
        }
        if segment.is_some_and(|s| s.attack_only) {
            score = self.attack_only_reference;
        }
        let Some(anchor) = self.anchor else {
            return score;
        };
        match (score, self.anchor_reference, anchor) {
            (Evaluation::Known(Intensity::Silence), _, _) => Evaluation::Known(Intensity::Silence),
            (Evaluation::Unknown, _, _) | (_, Evaluation::Unknown, _) => Evaluation::Unknown,
            (_, _, Intensity::Silence) => Evaluation::Known(Intensity::Silence),
            (
                Evaluation::Known(Intensity::Decibels(current)),
                Evaluation::Known(Intensity::Decibels(attack)),
                Intensity::Decibels(anchor),
            ) => Evaluation::Known(Intensity::Decibels(current + (anchor - attack))),
            (Evaluation::Absent, _, _) => Evaluation::Known(anchor),
            // There is no finite anchor adjustment for a source-muted attack.
            (Evaluation::Known(_), Evaluation::Known(Intensity::Silence), _) => Evaluation::Unknown,
            // A score instruction appearing inside a velocity-only note has no
            // source score level at that earlier attack; do not invent one.
            (Evaluation::Known(_), Evaluation::Absent, _) => Evaluation::Unknown,
        }
    }
}

/// Independent gain contributors are already resolved (e.g. the unchanged
/// EXP-002 CC7/CC11 resolver). The manual target master is deliberately absent.
#[derive(Clone, Debug, PartialEq)]
pub struct GainContributor {
    pub source_ids: BTreeSet<String>,
    pub gain: Option<f64>,
}
/// None means unknown, not neutral. No contributor means no gain automation.
pub fn compose_gain(
    intensity: Evaluation,
    contributors: &[GainContributor],
) -> Result<Option<f64>> {
    if intensity == Evaluation::Unknown {
        return Ok(None);
    }
    if intensity == Evaluation::Absent && contributors.is_empty() {
        return Ok(None);
    }
    let mut gain = match intensity {
        Evaluation::Known(value) => value.gain(),
        _ => 1.0,
    };
    if let Evaluation::Known(Intensity::Decibels(db)) = intensity {
        if !db.is_finite() || !gain.is_finite() || gain == 0.0 {
            return Err("Positive intensity is outside finite gain arithmetic; it was not converted to mute.".into());
        }
    }
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (index, contributor) in contributors.iter().enumerate() {
        if contributor.source_ids.is_empty() {
            return Err("Independent gain requires source ownership.".into());
        }
        let prior: BTreeSet<_> = contributor
            .source_ids
            .iter()
            .filter_map(|id| seen.get(id).copied())
            .collect();
        if !prior.is_empty() {
            if prior.len() == 1 && contributors[*prior.first().unwrap()] == *contributor {
                continue;
            }
            return Err("Gain contributors overlap in source ownership; composition would count a source twice.".into());
        }
        let Some(component) = contributor.gain else {
            return Ok(None);
        };
        if !component.is_finite() || component < 0.0 {
            return Err("Gain contributor must be finite and nonnegative.".into());
        }
        for id in &contributor.source_ids {
            seen.insert(id.clone(), index);
        }
        let positive = gain > 0.0 && component > 0.0;
        gain *= component;
        if positive && gain == 0.0 {
            return Err("Positive composed gain underflow; it was not converted to mute.".into());
        }
        if !gain.is_finite() {
            return Err("Composed source gain overflow; no clipping applied.".into());
        }
    }
    Ok(Some(gain))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WedgeKind {
    Start(Direction),
    Continue,
    Stop,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wedge {
    pub event: ScoreEvent,
    pub number: Option<String>,
    pub kind: WedgeKind,
    pub niente: bool,
}
/// Pair MusicXML wedges by part/effective scope/number. All input records remain
/// with the caller. Each candidate contains only its own start and shared
/// continue/stop evidence; eligible duplicate/conflicting starts are combined by
/// the resolver after pass filtering, never attributed to an excluded start.
pub fn pair_wedges(wedges: &[Wedge]) -> Result<(Vec<ScoreEvent>, Vec<Issue>)> {
    pair_wedges_bounded(wedges, &[], None, &mut ProvenanceBudget::default())
}

fn pair_wedges_bounded(
    wedges: &[Wedge],
    performed_passes: &[u32],
    route_passes: Option<&[BTreeSet<u32>]>,
    budget: &mut ProvenanceBudget,
) -> Result<(Vec<ScoreEvent>, Vec<Issue>)> {
    let count = wedges.len();
    let depth = usize::BITS as usize - count.leading_zeros() as usize + 1;
    budget.charge(count.saturating_mul(128), count.saturating_mul(depth))?;
    let mut buckets: BTreeMap<(&Scope, &str), Vec<(usize, &Wedge)>> = BTreeMap::new();
    for (index, wedge) in wedges.iter().enumerate() {
        buckets
            .entry((&wedge.event.scope, wedge.number.as_deref().unwrap_or("1")))
            .or_default()
            .push((index, wedge));
    }
    let mut events: Vec<ScoreEvent> = Vec::new();
    let mut issues = Vec::new();
    let mut emitted = BTreeMap::<Vec<usize>, usize>::new();
    let issue = |w: &Wedge,
                 pass: u32,
                 kind: IssueKind,
                 message: &str,
                 budget: &mut ProvenanceBudget|
     -> Result<Issue> {
        budget.event(&w.event)?;
        Ok(Issue {
            start: w.event.at,
            end: w.event.at,
            kind,
            provenance: Provenance::from_event(&w.event, pass),
            message: message.into(),
        })
    };
    for mut bucket in buckets.into_values() {
        bucket.sort_by_key(|(_, w)| (w.event.at, w.event.order));
        let restricted =
            route_passes.is_some() || bucket.iter().any(|(_, w)| !w.event.time_only.is_empty());
        let pass_count = bucket
            .iter()
            .fold(performed_passes.len().saturating_add(1), |n, (_, w)| {
                n.saturating_add(w.event.time_only.len())
            });
        budget.charge(pass_count.saturating_mul(64), pass_count.saturating_mul(16))?;
        let mut passes = BTreeSet::from([1]);
        passes.extend(performed_passes.iter().copied());
        passes.extend(
            bucket
                .iter()
                .flat_map(|(_, w)| w.event.time_only.iter().copied()),
        );
        // An entirely unrestricted bucket needs only one pairing: its result
        // applies to all passes. Restricted buckets enumerate actual/declared
        // passes, never an unbounded complement of a time-only list.
        if !restricted {
            passes = BTreeSet::from([1]);
        }
        budget.charge(
            bucket.len().saturating_mul(32).saturating_mul(passes.len()),
            bucket.len().saturating_mul(passes.len()),
        )?;
        for pass in passes {
            // Synthetic/default pairing passes are not performed occurrences.
            // Exclusion diagnostics require an actual positive source run.
            budget.charge(0, performed_passes.len())?;
            let performed = performed_passes.contains(&pass);
            let mut open: Vec<(usize, &Wedge)> = Vec::new();
            let mut cursor = 0;
            while cursor < bucket.len() {
                let at = bucket[cursor].1.event.at;
                budget.charge(0, depth)?;
                let stop = cursor + bucket[cursor..].partition_point(|(_, w)| w.event.at == at);
                let group = &bucket[cursor..stop];
                budget.charge(0, group.len())?;
                let eligibility_work = group.iter().fold(0usize, |n, (_, w)| {
                    n.saturating_add(w.event.time_only.len()).saturating_add(1)
                });
                budget.charge(0, eligibility_work.saturating_mul(3))?;
                let eligible = |index: usize, w: &Wedge| {
                    w.event.enabled
                        && (w.event.time_only.is_empty() || w.event.time_only.contains(&pass))
                        && route_passes.is_none_or(|passes| passes[index].contains(&pass))
                };
                for &(index, wedge) in group {
                    if !eligible(index, wedge) {
                        if !wedge.event.enabled {
                            issues.push(issue(
                                wedge,
                                if restricted { pass } else { 0 },
                                IssueKind::Disabled,
                                "Playback-disabled wedge endpoint retained inactive.",
                                budget,
                            )?);
                        } else if performed
                            && !wedge.event.time_only.is_empty()
                            && !wedge.event.time_only.contains(&pass)
                            && route_passes.is_none_or(|passes| passes[index].contains(&pass))
                        {
                            issues.push(issue(
                                wedge,
                                pass,
                                IssueKind::PassFiltered,
                                "time-only excludes this repeat pass.",
                                budget,
                            )?);
                        }
                        continue;
                    }
                    match wedge.kind {
                        WedgeKind::Start(_) => open.push((index, wedge)),
                        WedgeKind::Continue => {
                            budget.charge(0, open.len())?;
                            if open
                                .iter()
                                .any(|(_, w)| matches!(w.kind, WedgeKind::Start(_)))
                            {
                                open.push((index, wedge));
                            } else {
                                issues.push(issue(
                                    wedge,
                                    if restricted { pass } else { 0 },
                                    IssueKind::UnresolvedSpan,
                                    "Wedge continue has no eligible same-scope numbered start.",
                                    budget,
                                )?);
                            }
                        }
                        WedgeKind::Stop => {}
                    }
                }
                let stops = group
                    .iter()
                    .filter(|(index, w)| eligible(*index, w) && matches!(w.kind, WedgeKind::Stop));
                if stops.clone().next().is_some() {
                    let mut matched = false;
                    budget.charge(0, open.len())?;
                    for &(start_index, start) in open
                        .iter()
                        .filter(|(_, w)| matches!(w.kind, WedgeKind::Start(_)) && w.event.at < at)
                    {
                        matched = true;
                        let WedgeKind::Start(direction) = start.kind else {
                            unreachable!()
                        };
                        // Only this start, its subsequent continues, and every
                        // eligible coincident stop contribute to this candidate.
                        let continues = open.iter().filter(|(_, w)| {
                            matches!(w.kind, WedgeKind::Continue) && w.event.at >= start.event.at
                        });
                        budget.charge(
                            0,
                            open.len()
                                .saturating_add(eligibility_work)
                                .saturating_mul(2),
                        )?;
                        let pieces_count = 1usize
                            .saturating_add(continues.clone().count())
                            .saturating_add(stops.clone().count());
                        budget.charge(
                            pieces_count.saturating_mul(32),
                            open.len()
                                .saturating_add(group.len())
                                .saturating_add(pieces_count),
                        )?;
                        let pieces: Vec<_> = std::iter::once((start_index, start))
                            .chain(continues.copied())
                            .chain(stops.clone().copied())
                            .collect();
                        let key: Vec<_> = pieces.iter().map(|(index, _)| *index).collect();
                        let key_depth =
                            usize::BITS as usize - emitted.len().leading_zeros() as usize + 1;
                        budget
                            .charge(0, pieces_count.saturating_mul(key_depth).saturating_mul(2))?;
                        if let Some(&index) = emitted.get(&key) {
                            if restricted {
                                budget
                                    .charge(4, events[index].time_only.len().saturating_add(1))?;
                                if !events[index].time_only.contains(&pass) {
                                    events[index].time_only.push(pass);
                                }
                            }
                            continue;
                        }
                        budget.event(&start.event)?;
                        let bytes = pieces.iter().skip(1).fold(0usize, |n, (_, w)| {
                            n.saturating_add(evidence_size(&w.event.evidence))
                                .saturating_add(128)
                        });
                        budget.charge(
                            bytes.saturating_add(pieces_count.saturating_mul(32)),
                            bytes / 8 + pieces_count,
                        )?;
                        let mut event = start.event.clone();
                        event.time_only = if restricted { vec![pass] } else { vec![] };
                        let mut transition = Transition::new(at, direction);
                        for (index, (_, piece)) in pieces.iter().enumerate() {
                            let opposite = match direction {
                                Direction::Crescendo => "intensity_label_diminuendo",
                                Direction::Diminuendo => "intensity_label_crescendo",
                            };
                            if piece.event.evidence.raw_fields.contains_key(opposite) {
                                event.evidence.raw_fields.insert("intensity_direction_conflict".into(),
                                    "Recognized owning-wedge label disagrees with the authored direction".into());
                            }
                            transition.niente_start |=
                                piece.niente && direction == Direction::Crescendo;
                            transition.niente_end |=
                                piece.niente && direction == Direction::Diminuendo;
                            if index > 0 {
                                event
                                    .evidence
                                    .source_ids
                                    .extend(piece.event.evidence.source_ids.iter().cloned());
                                for (field, raw) in &piece.event.evidence.raw_fields {
                                    event
                                        .evidence
                                        .raw_fields
                                        .insert(format!("wedge:{index}/{field}"), raw.clone());
                                }
                            }
                        }
                        event.evidence.source_ids.sort();
                        event.evidence.source_ids.dedup();
                        event.instruction = Instruction::Transition(transition);
                        emitted.insert(key, events.len());
                        events.push(event);
                    }
                    if !matched {
                        for (_, wedge) in stops {
                            issues.push(issue(
                                wedge,
                                if restricted { pass } else { 0 },
                                IssueKind::UnresolvedSpan,
                                "Wedge stop has no eligible positive same-scope numbered span.",
                                budget,
                            )?);
                        }
                    }
                    // An equal-time start is not consumed by a zero-width stop.
                    budget.charge(0, open.len())?;
                    open.retain(|(_, w)| w.event.at == at);
                }
                cursor = stop;
            }
            for (_, wedge) in open {
                issues.push(issue(
                    wedge,
                    if restricted { pass } else { 0 },
                    IssueKind::UnresolvedSpan,
                    "Wedge start/continue has no eligible matched stop.",
                    budget,
                )?);
            }
        }
    }
    Ok((events, issues))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(n: i64) -> Time {
        Time::integer(n)
    }
    fn q(n: i64, d: i64) -> Time {
        Time::new(n, d).unwrap()
    }
    fn owner() -> ScoreVoice {
        ScoreVoice {
            part: "P1".into(),
            instrument: Some("voice".into()),
            staff: "1".into(),
            voice: "1".into(),
        }
    }
    fn evidence(id: &str) -> Evidence {
        Evidence {
            source_ids: vec![id.into()],
            contract: SourceContract::MusicXml,
            saving_version: Some("4.0".into()),
            layout: "score-partwise".into(),
            raw_fields: BTreeMap::from([("source".into(), id.into())]),
        }
    }
    fn event(at: Time, id: &str, instruction: Instruction) -> ScoreEvent {
        ScoreEvent {
            at,
            order: 0,
            scope: Scope::Part("P1".into()),
            enabled: true,
            scope_interpretation: Interpretation {
                field: "scope".into(),
                source_ids: vec![id.into()],
                basis: Basis::DeclaredDefault,
                exact: None,
                explanation: "Portable MusicXML part-wide default.".into(),
            },
            time_only: Vec::new(),
            evidence: evidence(id),
            instruction,
        }
    }
    fn mark(at: i64, symbol: &str) -> ScoreEvent {
        event(
            t(at),
            symbol,
            Instruction::Dynamic(Dynamic {
                symbol: Some(symbol.into()),
                ..Dynamic::default()
            }),
        )
    }
    fn ramp(start: i64, end: i64, direction: Direction) -> ScoreEvent {
        event(
            t(start),
            "ramp",
            Instruction::Transition(Transition::new(t(end), direction)),
        )
    }
    fn resolve(events: Vec<ScoreEvent>) -> Timeline {
        ScoreIntensity { events }
            .resolve(&owner(), 1, t(8), t(8))
            .unwrap()
    }
    fn db(timeline: &Timeline, at: Time) -> f64 {
        match timeline.evaluate(at) {
            Evaluation::Known(Intensity::Decibels(v)) => v,
            other => panic!("at {at:?}: {other:?}"),
        }
    }
    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }
    fn level(n: i64) -> Level {
        Level::positive(t(n)).unwrap()
    }
    fn mutate_transition(event: &mut ScoreEvent, change: impl FnOnce(&mut Transition)) {
        if let Instruction::Transition(transition) = &mut event.instruction {
            change(transition);
        } else {
            panic!("transition");
        }
    }
    fn note(velocity: Option<AttackVelocity>) -> NoteChain {
        NoteChain {
            source_id: "note:head:pass1".into(),
            start: t(0),
            end: t(4),
            owner: owner(),
            occurrence: 1,
            pass: 1,
            velocity: velocity.map(|value| VelocityEvidence {
                value,
                evidence: evidence("velocity"),
            }),
            continuation_velocities: Vec::new(),
        }
    }
    fn note_db(note: &NoteIntensity, at: Time) -> f64 {
        match note.evaluate(at) {
            Evaluation::Known(Intensity::Decibels(v)) => v,
            other => panic!("note at {at:?}: {other:?}"),
        }
    }

    #[test]
    fn occurrence_provenance_counter_is_shared_and_refuses_before_excess_copy() {
        let mut declared = mark(0, "p");
        declared
            .evidence
            .raw_fields
            .insert("large".into(), "x".repeat(1024 * 1024));
        let score = ScoreIntensity {
            events: vec![declared],
        };
        let runs: Vec<_> = (0..64)
            .map(|i| Occurrence {
                written_start: t(0),
                written_end: t(4),
                performed_start: t(i64::from(i) * 4),
                pass: 1,
                ordinal: i + 1,
            })
            .collect();
        // A single large declaration is supported. Replaying its actual copies
        // must share one budget rather than reset the ceiling for each run.
        let one = score.occurrences(&owner(), &runs[..1], &[t(4)]).unwrap();
        close(db(&one, t(2)), -7.75);
        assert!(score
            .occurrences(&owner(), &runs, &[t(4); 64])
            .is_err_and(|error| error.contains("SCORE_INTENSITY_LIMIT")));
        let mut budget = ProvenanceBudget::default();
        score
            .occurrences_bounded(&owner(), &runs[..1], &[t(4)], &mut budget)
            .unwrap();
        let used = budget.bytes;
        assert!(used > 1024 * 1024);
        budget.bytes = MAX_PROVENANCE_BYTES;
        assert!(score
            .occurrences_bounded(&owner(), &runs[..1], &[t(4)], &mut budget)
            .is_err_and(|error| error.contains("SCORE_INTENSITY_LIMIT")));
        close(
            db(&score.resolve(&owner(), 1, t(4), t(4)).unwrap(), t(2)),
            -7.75,
        );
    }

    #[test]
    fn provenance_budget_refuses_before_copy_or_merge() {
        let original = Provenance::from_event(&mark(0, "p"), 1);
        let mut destination = original.clone();
        let mut incoming = Provenance::from_event(&mark(1, "f"), 1);
        incoming.evidence[0]
            .raw_fields
            .insert("large".into(), "x".repeat(4096));
        let mut budget = ProvenanceBudget {
            bytes: MAX_PROVENANCE_BYTES,
            work: 0,
            ..ProvenanceBudget::default()
        };
        assert!(budget
            .copy(&incoming)
            .unwrap_err()
            .contains("SCORE_INTENSITY_LIMIT"));
        assert!(budget
            .include(&mut destination, &incoming)
            .unwrap_err()
            .contains("SCORE_INTENSITY_LIMIT"));
        assert_eq!(destination, original);
        assert_eq!(budget.bytes, MAX_PROVENANCE_BYTES);
        assert_eq!(budget.work, 0);
    }

    #[test]
    fn source_review_continuation_budget_is_cumulative_and_preserves_inputs() {
        let timeline = Arc::new(resolve(vec![
            mark(0, "p"),
            ramp(2, 4, Direction::Crescendo),
        ]));
        let mut head_chain = note(Some(AttackVelocity::Midi(100)));
        head_chain.end = t(1);
        let head = NoteIntensity::resolve(&head_chain, timeline.clone(), &[t(0)]).unwrap();
        let mut tail_chain = note(Some(AttackVelocity::Midi(20)));
        tail_chain.start = t(1);
        let tail = NoteIntensity::resolve(&tail_chain, timeline, &[t(0), t(1)]).unwrap();
        let original = (head.clone(), tail.clone());
        let expected = NoteIntensity::continued_from(&head, Some(&tail));
        let mut measured = ProvenanceBudget::default();
        assert_eq!(
            NoteIntensity::continued_from_bounded(&head, Some(&tail), &mut measured).unwrap(),
            expected
        );
        let mut budget = ProvenanceBudget::with_limits(measured.bytes, MAX_RESOLUTION_WORK);
        assert_eq!(
            NoteIntensity::continued_from_bounded(&head, Some(&tail), &mut budget).unwrap(),
            expected
        );
        assert!(
            NoteIntensity::continued_from_bounded(&head, Some(&tail), &mut budget)
                .is_err_and(|error| error.contains("SCORE_INTENSITY_LIMIT"))
        );
        assert_eq!((head, tail), original);
    }

    #[test]
    fn source_review5_continuation_gate_checks_payload_and_refresh_without_copying() {
        let timeline = Arc::new(resolve(vec![ramp(2, 4, Direction::Crescendo)]));
        let mut chain = note(Some(AttackVelocity::Midi(100)));
        chain.end = t(1);
        let head = NoteIntensity::resolve(&chain, timeline.clone(), &[t(0)]).unwrap();
        chain.start = t(1);
        chain.end = t(4);
        chain.velocity.as_mut().unwrap().value = AttackVelocity::Midi(20);
        let raw = NoteIntensity::resolve(&chain, timeline, &[t(0), t(1)]).unwrap();
        let inherited = NoteIntensity::continued_from(&head, Some(&raw));
        assert!(inherited.is_continuation_of(&head));
        assert!(!raw.is_continuation_of(&head));
        assert_ne!(inherited.anchor_reference, head.anchor_reference);
        for field in 0..8 {
            let mut changed = inherited.clone();
            match field {
                0 => changed.source_id.push('x'),
                1 => changed.start = t(1),
                2 => changed.end = t(0),
                3 => changed.timeline = Arc::new((*head.timeline).clone()),
                4 => changed.anchor = None,
                5 => changed.score_at_attack = Evaluation::Unknown,
                6 => changed.invalid_anchor = !head.invalid_anchor,
                7 => changed.anchor_reference = head.anchor_reference,
                _ => unreachable!(),
            }
            assert!(!changed.is_continuation_of(&head), "field {field}");
        }
        let mut changed = inherited.clone();
        changed.attack_only_reference = Evaluation::Unknown;
        assert!(!changed.is_continuation_of(&head));
        let mut measured = ProvenanceBudget::default();
        assert!(inherited
            .is_continuation_of_bounded(&head, &mut measured)
            .unwrap());
        assert_eq!(measured.bytes, 0);
        let mut budget = ProvenanceBudget::with_limits(0, measured.work);
        assert!(inherited
            .is_continuation_of_bounded(&head, &mut budget)
            .unwrap());
        assert!(inherited
            .is_continuation_of_bounded(&head, &mut budget)
            .is_err_and(|e| e.contains("SCORE_INTENSITY_LIMIT")));
    }

    #[test]
    fn source_review5_known_attack_validation_work_is_independent_of_chain_length() {
        let timeline = Arc::new(resolve(vec![mark(0, "p")]));
        let head =
            NoteIntensity::resolve(&note(Some(AttackVelocity::Midi(100))), timeline, &[t(0)])
                .unwrap();
        let mut small = ProvenanceBudget::default();
        assert!(head.is_continuation_of_bounded(&head, &mut small).unwrap());
        let mut large = head.clone();
        let template = large.timeline.segments[0].clone();
        large.timeline = Arc::new(Timeline {
            segments: (0..10_000)
                .map(|index| Segment {
                    start: t(index),
                    end: t(index + 1),
                    ..template.clone()
                })
                .collect(),
            ..Timeline::default()
        });
        large.end = t(10_000);
        let mut budget = ProvenanceBudget::with_limits(0, small.work * 1000);
        for _ in 0..1000 {
            assert!(large
                .is_continuation_of_bounded(&large, &mut budget)
                .unwrap());
        }
        assert_eq!((budget.bytes, budget.work), (0, small.work * 1000));
    }

    #[test]
    fn source_review_issue_and_invalid_velocity_messages_are_preflighted() {
        let issue = Issue {
            start: t(0),
            end: t(1),
            kind: IssueKind::UnsupportedMark,
            provenance: Provenance::from_event(&mark(0, "p"), 1),
            message: "unsupported".repeat(4096),
        };
        let velocity = VelocityEvidence {
            value: AttackVelocity::Invalid("invalid velocity".repeat(4096)),
            evidence: evidence("velocity"),
        };
        for work_limit in [0, MAX_RESOLUTION_WORK] {
            let mut budget = ProvenanceBudget::with_limits(4096, work_limit);
            assert!(budget.reserve_issue(&issue).is_err());
            assert!(budget.reserve_velocity(&velocity).is_err());
            assert_eq!((budget.bytes, budget.work), (0, 0));
        }
        let mut work_limited = ProvenanceBudget::with_limits(MAX_PROVENANCE_BYTES, 0);
        assert!(work_limited.reserve_issue(&issue).is_err());
        assert!(work_limited.reserve_velocity(&velocity).is_err());
        assert_eq!((work_limited.bytes, work_limited.work), (0, 0));
    }

    #[test]
    fn source_review_forward_pass_change_preserves_the_preceding_held_declaration() {
        let mut held = mark(0, "p");
        held.time_only = vec![2];
        let score = ScoreIntensity { events: vec![held] };
        let runs = [
            Occurrence {
                written_start: t(0),
                written_end: t(1),
                performed_start: t(0),
                pass: 2,
                ordinal: 1,
            },
            Occurrence {
                written_start: t(1),
                written_end: t(3),
                performed_start: t(1),
                pass: 1,
                ordinal: 2,
            },
        ];
        let timeline = score.occurrences(&owner(), &runs, &[t(1), t(3)]).unwrap();
        close(db(&timeline, t(2)), -7.75);
    }

    #[test]
    fn dense_source_resolution_is_bounded_before_quadratic_preparation() {
        let score = ScoreIntensity {
            events: (0..3000).map(|i| mark(i, "p")).collect(),
        };
        assert!(score
            .resolve(&owner(), 1, t(4000), t(4000))
            .unwrap_err()
            .contains("SCORE_INTENSITY_LIMIT"));
        let score = ScoreIntensity {
            events: (0..1000).map(|i| mark(i, "p")).collect(),
        };
        let runs: Vec<_> = (0..7)
            .map(|i| Occurrence {
                written_start: t(0),
                written_end: t(4000),
                performed_start: t(i * 4000),
                pass: 1,
                ordinal: i as u32 + 1,
            })
            .collect();
        assert!(score
            .occurrences(&owner(), &runs, &[t(4000); 7])
            .unwrap_err()
            .contains("SCORE_INTENSITY_LIMIT"));
    }

    #[test]
    fn indexed_segments_and_late_accents_preserve_boundaries_chords_and_next_attack() {
        // Resolver output is sorted and positive segments are disjoint; a
        // terminal point may share the preceding segment's end.
        let mut timeline = Timeline {
            segments: (0..10_000)
                .map(|i| Segment {
                    start: t(i),
                    end: t(i + 1),
                    curve: Curve::Level(Level::Positive(t(80 + i % 2))),
                    attack_only: false,
                    provenance: None,
                })
                .collect(),
            ..Timeline::default()
        };
        for i in 0..10_000 {
            assert_eq!(timeline.segment_at(t(i)).unwrap().start, t(i));
            assert_eq!(timeline.segment_at(q(i * 2 + 1, 2)).unwrap().start, t(i));
        }
        assert_eq!(timeline.segment_at(t(10_000)).unwrap().start, t(9999));
        timeline.segments.push(Segment {
            start: t(10_000),
            end: t(10_000),
            curve: Curve::Level(Level::Silence),
            attack_only: false,
            provenance: None,
        });
        assert_eq!(
            timeline.evaluate(t(10_000)),
            Evaluation::Known(Intensity::Silence)
        );
        let attacks: Vec<_> = (0..100_000).flat_map(|i| [t(i * 2), t(i * 2)]).collect();
        let accent_event = mark(199_997, "sfz");
        timeline.accents.push(Accent {
            at: t(199_997),
            level: Level::Positive(t(112)),
            provenance: Provenance::from_event(&accent_event, 1),
        });
        assert_eq!(timeline.accent_range(&attacks, t(199_996)), 0..0);
        // Both chord members share the same attack; no traversal over the
        // preceding 199,998 entries and no repeated late accent afterward.
        assert_eq!(timeline.accent_range(&attacks, t(199_998)), 0..1);
        assert_eq!(timeline.accent_range(&attacks, t(199_998)), 0..1);
        assert_eq!(timeline.accent_range(&attacks, t(200_000)), 1..1);
    }

    #[test]
    fn performed_issue_clips_at_jump_boundaries_and_marks_only_final_points() {
        let event = mark(1, "p");
        let mut issue = Issue {
            start: t(1),
            end: t(5),
            kind: IssueKind::UnresolvedSpan,
            provenance: Provenance::from_event(&event, 0),
            message: "retained span".into(),
        };
        let run = Occurrence {
            written_start: t(2),
            written_end: t(4),
            performed_start: t(10),
            pass: 2,
            ordinal: 3,
        };
        let mapped = performed_issue(&issue, &run, false).unwrap().unwrap();
        assert_eq!((mapped.start, mapped.end), (t(10), t(12)));
        assert_eq!(
            (mapped.provenance.repeat_pass, mapped.provenance.occurrence),
            (2, 3)
        );
        assert_eq!(mapped.provenance.evidence, issue.provenance.evidence);
        issue.start = t(4);
        issue.end = t(4);
        assert!(performed_issue(&issue, &run, false).unwrap().is_none());
        let endpoint = performed_issue(&issue, &run, true).unwrap().unwrap();
        assert_eq!((endpoint.start, endpoint.end), (t(12), t(12)));
        issue.start = t(1);
        issue.end = t(2);
        assert!(performed_issue(&issue, &run, false).unwrap().is_none());
    }

    #[test]
    fn long_decimal_source_tempo_keeps_exact_late_compound_endpoint() {
        let tempo = Fraction::decimal("0.83333333333333337")
            .unwrap()
            .checked_mul(t(60))
            .unwrap();
        let end = t(10_000)
            .checked_add(transition_duration(tempo, Speed::Normal).unwrap())
            .unwrap();
        assert_eq!(end, Fraction::decimal("10000.333333333333333348").unwrap());
        assert!(serde_json::to_value(end).unwrap()["numerator"].is_string());
        let near_one = Fraction::wide(i128::MAX - 1, i128::MAX).unwrap();
        assert!(near_one < Fraction::ONE);
        assert!(near_one > Fraction::new(1, 2).unwrap());
        assert!(Fraction::wide(-(i128::MAX - 1), i128::MAX).unwrap() > t(-1));
        assert!(Fraction::wide(i128::MAX, 1)
            .unwrap()
            .checked_add(Fraction::ONE)
            .is_err());
        assert!(Level::positive(Fraction::decimal("49.123456789012345678").unwrap()).is_ok());
    }

    #[test]
    fn exact_decimals_and_policy_time_are_not_integer_ticks() {
        assert_eq!(Fraction::decimal(" +100.1250 ").unwrap(), q(801, 8));
        assert_eq!(
            Fraction::decimal(".000000000000000001").unwrap(),
            q(1, 1_000_000_000_000_000_000)
        );
        assert_eq!(Fraction::decimal("-0.05").unwrap(), q(-1, 20));
        assert!(Fraction::decimal("NaN").is_err());
        assert!(Fraction::decimal("1e2").is_err());
        assert!(Fraction::new(1, 0).is_err());
        assert_eq!(transition_duration(t(120), Speed::Normal).unwrap(), q(4, 5));
        assert_eq!(transition_duration(t(90), Speed::Slow).unwrap(), q(39, 40));
        assert_eq!(transition_duration(t(60), Speed::Fast).unwrap(), q(1, 4));
        assert!(transition_duration(t(0), Speed::Normal).is_err());
    }

    #[test]
    fn full_standard_table_is_positive_monotonic_and_inside_uncomposed_domain() {
        for (symbol, expected) in [
            ("pppppp", 1),
            ("ppppp", 5),
            ("pppp", 10),
            ("ppp", 16),
            ("pp", 33),
            ("p", 49),
            ("mp", 64),
            ("mf", 80),
            ("f", 96),
            ("ff", 112),
            ("fff", 126),
            ("ffff", 127),
            ("fffff", 127),
            ("ffffff", 127),
        ] {
            assert_eq!(standard_level(symbol), Some(level(expected)));
        }
        let mut previous = -100.0;
        for n in 1..=127 {
            let Intensity::Decibels(value) = level(n).value() else {
                panic!("positive");
            };
            assert!(value > previous && (-19.75..=11.75).contains(&value));
            previous = value;
        }
        assert_eq!(standard_level("n"), Some(Level::Silence));
        assert_eq!(standard_level("sfp"), None);
    }

    #[test]
    fn p_crescendo_f_has_expected_midpoint_and_persists_across_rests() {
        let timeline = resolve(vec![
            mark(0, "p"),
            ramp(1, 2, Direction::Crescendo),
            mark(2, "f"),
        ]);
        close(db(&timeline, t(0)), -7.75);
        close(db(&timeline, t(1)), -7.75);
        close(db(&timeline, q(3, 2)), -1.875);
        close(db(&timeline, t(2)), 4.0);
        close(db(&timeline, t(7)), 4.0);
        let samples: Vec<_> = [t(1), q(3, 2), t(2)]
            .map(|at| (db(&timeline, at) * 10.0).round() as i32)
            .into();
        assert_eq!(samples, [-78, -19, 40]);
        assert!(timeline.issues.is_empty());
        let provenance = timeline
            .segment_at(q(3, 2))
            .unwrap()
            .provenance
            .as_ref()
            .unwrap();
        assert_eq!(provenance.policy, POLICY);
        assert!(provenance
            .evidence
            .iter()
            .any(|e| e.source_ids.contains(&"f".into())));
    }

    #[test]
    fn numeric_sound_replaces_symbol_and_keeps_fractional_source_value() {
        let mut event = mark(0, "p");
        if let Instruction::Dynamic(dynamic) = &mut event.instruction {
            dynamic.numeric = Some(NumericLevel::MusicXml(t(100)));
        }
        let timeline = resolve(vec![event.clone()]);
        close(db(&timeline, t(0)), 2.5);
        if let Instruction::Dynamic(dynamic) = &mut event.instruction {
            dynamic.numeric = Some(NumericLevel::MusicXml(Fraction::decimal("0.1").unwrap()));
        }
        let timeline = resolve(vec![event]);
        close(db(&timeline, t(0)), -19.9775);
        let p = timeline.segments[0].provenance.as_ref().unwrap();
        assert!(p
            .interpretations
            .iter()
            .any(|i| i.exact == Some(q(9, 100)) && i.basis == Basis::ExplicitNumeric));
    }

    #[test]
    fn musescore_numeric_sentinels_and_custom_positive_dynamics_differ() {
        for sentinel in [0, -1, -20] {
            let mut event = mark(0, "p");
            if let Instruction::Dynamic(d) = &mut event.instruction {
                d.numeric = Some(NumericLevel::MuseScoreDynamic(t(sentinel)));
            }
            close(db(&resolve(vec![event]), t(0)), -7.75);
        }
        let mut event = mark(0, "other-dynamics");
        if let Instruction::Dynamic(d) = &mut event.instruction {
            d.numeric = Some(NumericLevel::MuseScoreDynamic(t(100)));
        }
        close(db(&resolve(vec![event]), t(0)), 5.0);
        let mut event = mark(0, "p");
        if let Instruction::Dynamic(d) = &mut event.instruction {
            d.numeric = Some(NumericLevel::MusicXml(t(0)));
        }
        assert_eq!(
            resolve(vec![event]).evaluate(t(0)),
            Evaluation::Known(Intensity::Silence)
        );
    }

    #[test]
    fn automatic_endpoint_uses_adjacent_rung_and_never_mutes_ordinary_dim() {
        close(
            db(
                &resolve(vec![mark(0, "p"), ramp(1, 2, Direction::Crescendo)]),
                t(2),
            ),
            -4.0,
        );
        close(
            db(
                &resolve(vec![mark(0, "p"), ramp(1, 2, Direction::Diminuendo)]),
                t(2),
            ),
            -11.75,
        );
        let exhausted = resolve(vec![mark(0, "pppppp"), ramp(1, 2, Direction::Diminuendo)]);
        close(db(&exhausted, t(2)), -19.75);
        assert!(exhausted
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::RangeExhausted));
        let mut custom = mark(0, "p");
        if let Instruction::Dynamic(d) = &mut custom.instruction {
            d.numeric = Some(NumericLevel::Level(t(60)));
        }
        close(
            db(
                &resolve(vec![custom, ramp(1, 2, Direction::Crescendo)]),
                t(2),
            ),
            -4.0,
        );
    }

    #[test]
    fn lookahead_follows_same_scope_dynamic_but_stops_at_another_state_instruction() {
        let timeline = resolve(vec![
            mark(0, "p"),
            ramp(1, 2, Direction::Crescendo),
            mark(4, "f"),
        ]);
        close(db(&timeline, t(2)), 4.0);
        close(db(&timeline, t(3)), 4.0);
        let timeline = resolve(vec![
            mark(0, "p"),
            ramp(1, 2, Direction::Crescendo),
            ramp(3, 4, Direction::Diminuendo),
            mark(5, "f"),
        ]);
        close(db(&timeline, t(2)), -4.0);
        let timeline = resolve(vec![
            mark(0, "p"),
            ramp(1, 2, Direction::Crescendo),
            mark(3, "sfz"),
            mark(4, "f"),
        ]);
        close(db(&timeline, t(2)), 4.0);
    }

    #[test]
    fn wrong_direction_ramps_forward_then_applies_conflicting_boundary_mark() {
        let timeline = resolve(vec![
            mark(0, "p"),
            ramp(1, 3, Direction::Crescendo),
            mark(3, "pp"),
        ]);
        close(db(&timeline, t(2)), -5.875);
        close(db(&timeline, t(3)), -11.75);
        assert!(timeline
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::DirectionMismatch));
    }

    #[test]
    fn explicit_instance_delta_wins_before_boundary_dynamic() {
        let mut transition = ramp(1, 3, Direction::Crescendo);
        mutate_transition(&mut transition, |r| r.velo_change = Some(t(-20)));
        let timeline = resolve(vec![mark(0, "p"), transition, mark(3, "f")]);
        close(db(&timeline, t(2)), -5.25);
        close(db(&timeline, q(2999, 1000)), -2.7525);
        close(db(&timeline, t(3)), 4.0);
        assert!(timeline
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Conflict));
    }

    #[test]
    fn embedded_end_text_also_survives_an_instance_delta_conflict() {
        let mut transition = ramp(1, 3, Direction::Crescendo);
        mutate_transition(&mut transition, |r| {
            r.velo_change = Some(t(20));
            r.end_level = Some(level(96));
        });
        let timeline = resolve(vec![mark(0, "p"), transition]);
        close(db(&timeline, t(2)), -5.25);
        close(db(&timeline, t(3)), 4.0);
    }

    #[test]
    fn explicit_interior_dynamic_cuts_ramp_without_reset_or_averaging() {
        let timeline = resolve(vec![
            mark(0, "p"),
            ramp(1, 5, Direction::Crescendo),
            mark(3, "mp"),
            mark(5, "f"),
        ]);
        close(db(&timeline, t(2)), -4.8125);
        close(db(&timeline, t(3)), -4.0);
        close(db(&timeline, t(4)), -4.0);
        close(db(&timeline, t(5)), 4.0);
        assert!(timeline
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Interrupted));
    }

    #[test]
    fn overlapping_ramps_conflict_only_in_their_intersection() {
        let timeline = resolve(vec![
            mark(0, "p"),
            ramp(1, 5, Direction::Crescendo),
            ramp(3, 4, Direction::Diminuendo),
        ]);
        assert!(matches!(timeline.evaluate(t(2)), Evaluation::Known(_)));
        assert_eq!(timeline.evaluate(q(7, 2)), Evaluation::Unknown);
        assert!(matches!(timeline.evaluate(q(9, 2)), Evaluation::Known(_)));
        assert!(timeline
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Conflict && i.start == t(3) && i.end == t(4)));
    }

    #[test]
    fn easing_families_have_exact_endpoints_and_distinct_midpoints() {
        let expected = [
            (Easing::Normal, 0.5),
            (Easing::EaseIn, 1.0 - 2.0f64.sqrt() / 2.0),
            (Easing::EaseOut, 2.0f64.sqrt() / 2.0),
            (Easing::EaseInOut, 0.5),
            (Easing::Exponential, (21.0f64.sqrt() - 1.0) / 20.0),
        ];
        for (method, midpoint) in expected {
            close(method.normalized(0.5, 20.0), midpoint);
            assert_eq!(method.normalized(0.0, 20.0), 0.0);
            assert_eq!(method.normalized(1.0, 20.0), 1.0);
        }
        close(Easing::Exponential.normalized(0.5, 0.0), 0.5);
        let mut transition = ramp(1, 3, Direction::Crescendo);
        mutate_transition(&mut transition, |r| {
            r.method = Some("unknown-method".into())
        });
        let timeline = resolve(vec![mark(0, "p"), transition, mark(3, "f")]);
        close(db(&timeline, t(2)), -1.875);
        assert!(timeline
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::EasingFallback));
    }

    #[test]
    fn niente_interpolates_gain_and_keeps_positive_tail_without_log_zero() {
        let mut transition = ramp(1, 3, Direction::Diminuendo);
        mutate_transition(&mut transition, |r| r.niente_end = true);
        let timeline = resolve(vec![mark(0, "mf"), transition]);
        close(db(&timeline, t(2)), 20.0 * 0.5f64.log10());
        assert!(
            db(&timeline, q(2999999, 1000000)) < -23.9,
            "neutral evaluator preserves the intended limited tail for the target adapter"
        );
        assert_eq!(
            timeline.evaluate(t(3)),
            Evaluation::Known(Intensity::Silence)
        );
        assert_eq!(
            timeline.evaluate(t(7)),
            Evaluation::Known(Intensity::Silence)
        );
        let mut transition = ramp(1, 3, Direction::Crescendo);
        mutate_transition(&mut transition, |r| {
            r.niente_start = true;
            r.end_level = Some(level(80));
        });
        let timeline = resolve(vec![transition]);
        assert_eq!(
            timeline.evaluate(t(1)),
            Evaluation::Known(Intensity::Silence)
        );
        close(db(&timeline, t(2)), 20.0 * 0.5f64.log10());
        close(db(&timeline, t(3)), 0.0);
    }

    #[test]
    fn text_allowlist_is_exact_and_standalone_fades_have_bounded_spans() {
        for text in [
            "cresc.",
            "cresc",
            "crescendo",
            "dim.",
            "dim",
            "diminuendo",
            "decresc.",
            "decrescendo",
            "morendo",
            "smorzando",
            "fade out",
            "fade-out",
            "fondu",
            "fondu au silence",
            "fade in",
            "fade-in",
        ] {
            assert!(recognized_text(text).is_some(), "{text}");
        }
        assert_eq!(recognized_text("  FADE\nOUT  "), Some(TextIntent::FadeOut));
        assert_eq!(recognized_text("please fade out soon"), None);
        let fade = event(
            t(1),
            "text",
            Instruction::Text {
                text: "morendo".into(),
                end: None,
                owning_spanner: None,
            },
        );
        let timeline = ScoreIntensity {
            events: vec![mark(0, "p"), fade.clone()],
        }
        .resolve(&owner(), 1, t(8), t(4))
        .unwrap();
        assert_eq!(
            timeline.evaluate(t(4)),
            Evaluation::Known(Intensity::Silence)
        );
        assert!(timeline
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::InferredSpan && i.end == t(4)));
        let unresolved = ScoreIntensity { events: vec![fade] }
            .resolve(&owner(), 1, t(8), t(1))
            .unwrap();
        assert!(unresolved
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::UnresolvedSpan));
    }

    #[test]
    fn owning_spanner_label_is_not_interpreted_twice() {
        let label = event(
            t(1),
            "label",
            Instruction::Text {
                text: "cresc.".into(),
                end: None,
                owning_spanner: Some("ramp".into()),
            },
        );
        let timeline = resolve(vec![mark(0, "p"), ramp(1, 2, Direction::Crescendo), label]);
        close(db(&timeline, t(2)), -4.0);
        assert!(!timeline
            .issues
            .iter()
            .any(|i| matches!(i.kind, IssueKind::Conflict | IssueKind::InferredSpan)));
    }

    #[test]
    fn compound_dynamics_use_source_speed_and_musicxml_default() {
        let xml = resolve(vec![mark(0, "fp")]);
        close(db(&xml, t(0)), 4.0);
        close(db(&xml, q(1, 2)), -1.875);
        close(db(&xml, t(1)), -7.75);
        let mut ms = mark(0, "pf");
        ms.evidence.contract = SourceContract::MuseScoreLegacy;
        if let Instruction::Dynamic(d) = &mut ms.instruction {
            d.tempo_at_start = Some(t(120));
        }
        let ms = resolve(vec![ms]);
        close(db(&ms, q(2, 5)), -1.875);
        close(db(&ms, q(4, 5)), 4.0);
        assert!(ms.segments.iter().any(|s| s.end == q(4, 5)));
    }

    #[test]
    fn missing_transition_start_has_default_provenance_but_absence_has_no_curve() {
        let empty = resolve(vec![]);
        assert_eq!(empty.evaluate(t(0)), Evaluation::Absent);
        assert!(empty.segments.iter().all(|s| s.provenance.is_none()));
        let timeline = resolve(vec![ramp(1, 2, Direction::Crescendo)]);
        assert_eq!(timeline.evaluate(t(0)), Evaluation::Absent);
        close(db(&timeline, t(1)), 0.0);
        close(db(&timeline, t(2)), 4.0);
        assert!(timeline
            .segment_at(t(1))
            .unwrap()
            .provenance
            .as_ref()
            .unwrap()
            .interpretations
            .iter()
            .any(|i| i.basis == Basis::DeclaredDefault && i.exact == Some(t(80))));
    }

    #[test]
    fn staff_voice_part_scope_precedence_never_uses_channel_numbers() {
        let mut voice = mark(0, "f");
        voice.scope = Scope::musicxml("P1", Some("1"), Some("1"));
        let score = ScoreIntensity {
            events: vec![mark(0, "p"), voice],
        };
        close(
            db(&score.resolve(&owner(), 1, t(8), t(8)).unwrap(), t(0)),
            4.0,
        );
        let mut sibling = owner();
        sibling.voice = "2".into();
        close(
            db(&score.resolve(&sibling, 1, t(8), t(8)).unwrap(), t(0)),
            -7.75,
        );
        sibling.staff = "2".into();
        close(
            db(&score.resolve(&sibling, 1, t(8), t(8)).unwrap(), t(0)),
            -7.75,
        );
        sibling.part = "P2".into();
        assert_eq!(
            score
                .resolve(&sibling, 1, t(8), t(8))
                .unwrap()
                .evaluate(t(0)),
            Evaluation::Absent
        );
        let narrowed = Scope::musicxml("P1", None, Some("2"));
        sibling.part = "P1".into();
        assert!(narrowed.applies(&sibling));
    }

    #[test]
    fn legacy_and_modern_scope_defaults_and_compatibility_are_explicit() {
        let (legacy, description) = musescore_scope(&owner(), false, None, None);
        assert_eq!(legacy, Scope::Part("P1".into()));
        assert!(description.contains("default"));
        let (modern, description) = musescore_scope(&owner(), true, None, None);
        assert!(matches!(modern, Scope::Instrument { .. }));
        assert!(description.contains("default"));
        let (compat, description) =
            musescore_scope(&owner(), true, None, Some(LegacyRange::System));
        assert_eq!(compat, Scope::System);
        assert!(description.contains("compatibility"));
        let (voice, _) = musescore_scope(
            &owner(),
            true,
            Some(VoiceAssignment::CurrentVoice),
            Some(LegacyRange::System),
        );
        assert!(matches!(voice, Scope::Voice { .. }));
    }

    #[test]
    fn equal_priority_unrelated_levels_are_local_conflicts_until_replaced() {
        let timeline = resolve(vec![mark(0, "p"), mark(0, "f"), mark(2, "mp")]);
        assert_eq!(timeline.evaluate(t(1)), Evaluation::Unknown);
        close(db(&timeline, t(2)), -4.0);
        assert!(timeline
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Conflict));
        let duplicates = resolve(vec![mark(0, "p"), mark(0, "p")]);
        close(db(&duplicates, t(1)), -7.75);
        assert!(duplicates.issues.is_empty());
    }

    #[test]
    fn disabled_unknown_and_invalid_fields_stay_retained_and_local() {
        let mut disabled = mark(0, "f");
        disabled.enabled = false;
        let mut unsupported = mark(0, "ff");
        unsupported.scope = Scope::Unsupported {
            part: "P1".into(),
            raw: "99".into(),
        };
        let score = ScoreIntensity {
            events: vec![disabled, unsupported, mark(1, "mysterious"), mark(2, "p")],
        };
        let before = score.events.clone();
        let timeline = score.resolve(&owner(), 1, t(8), t(8)).unwrap();
        assert_eq!(score.events, before);
        assert_eq!(timeline.evaluate(t(1)), Evaluation::Absent);
        close(db(&timeline, t(2)), -7.75);
        for kind in [
            IssueKind::Disabled,
            IssueKind::UnsupportedScope,
            IssueKind::UnsupportedMark,
        ] {
            assert!(timeline.issues.iter().any(|i| i.kind == kind));
        }
        let mut invalid = mark(0, "f");
        if let Instruction::Dynamic(d) = &mut invalid.instruction {
            d.numeric = Some(NumericLevel::Level(t(128)));
        }
        let timeline = resolve(vec![invalid, mark(2, "p")]);
        assert_eq!(timeline.evaluate(t(0)), Evaluation::Unknown);
        close(db(&timeline, t(2)), -7.75);
    }

    #[test]
    fn musicxml_playback_offsets_ignore_visual_offsets_and_use_sound_own_offset() {
        assert_eq!(
            musicxml_position(t(2), None, Some((t(1), false))).unwrap(),
            t(2)
        );
        assert_eq!(
            musicxml_position(t(2), None, Some((t(1), true))).unwrap(),
            t(3)
        );
        assert_eq!(
            musicxml_position(t(2), Some(q(-1, 2)), Some((t(1), true))).unwrap(),
            q(3, 2)
        );
    }

    #[test]
    fn repeated_destination_restarts_from_source_state_and_filters_time_only() {
        let mut p = mark(0, "p");
        p.time_only = vec![1];
        let score = ScoreIntensity {
            events: vec![p, mark(3, "f")],
        };
        let runs = [
            Occurrence {
                written_start: t(0),
                written_end: t(4),
                performed_start: t(0),
                pass: 1,
                ordinal: 1,
            },
            Occurrence {
                written_start: t(0),
                written_end: t(4),
                performed_start: t(4),
                pass: 2,
                ordinal: 2,
            },
        ];
        let timeline = score.occurrences(&owner(), &runs, &[t(4), t(4)]).unwrap();
        close(db(&timeline, t(0)), -7.75);
        close(db(&timeline, t(3)), 4.0);
        assert_eq!(timeline.evaluate(t(4)), Evaluation::Absent);
        close(db(&timeline, t(7)), 4.0);
        assert_eq!(
            timeline
                .segment_at(t(7))
                .unwrap()
                .provenance
                .as_ref()
                .unwrap()
                .occurrence,
            2
        );
    }

    #[test]
    fn jump_reenters_original_ramp_phase_without_carrying_previous_endpoint() {
        let score = ScoreIntensity {
            events: vec![mark(0, "p"), ramp(1, 5, Direction::Crescendo), mark(5, "f")],
        };
        let runs = [
            Occurrence {
                written_start: t(0),
                written_end: t(6),
                performed_start: t(0),
                pass: 1,
                ordinal: 1,
            },
            Occurrence {
                written_start: t(2),
                written_end: t(4),
                performed_start: t(6),
                pass: 2,
                ordinal: 2,
            },
        ];
        let timeline = score.occurrences(&owner(), &runs, &[t(6), t(4)]).unwrap();
        // Second run cannot look ahead to f beyond its jump; adjacent mp endpoint.
        close(db(&timeline, t(6)), -6.8125);
        assert!(matches!(
            timeline.segment_at(t(6)).unwrap().curve,
            Curve::Transition { .. }
        ));
    }

    #[test]
    fn note_velocity_anchors_once_and_preserves_relative_score_motion() {
        let timeline = Arc::new(resolve(vec![
            mark(0, "p"),
            ramp(1, 3, Direction::Crescendo),
            mark(3, "f"),
        ]));
        let note =
            NoteIntensity::resolve(&note(Some(AttackVelocity::Midi(100))), timeline, &[t(0)])
                .unwrap();
        close(note_db(&note, t(0)), 5.0);
        close(note_db(&note, t(2)), 10.875);
        close(note_db(&note, t(3)), 16.75); // exact intended value retained, no compressor
    }

    #[test]
    fn ties_do_not_reanchor_and_explicit_attack_only_ramps_hold_inside_chain() {
        let mut transition = ramp(1, 3, Direction::Crescendo);
        mutate_transition(&mut transition, |r| r.single_note_dynamics = Some(false));
        let timeline = Arc::new(resolve(vec![mark(0, "p"), transition]));
        let mut chain = note(Some(AttackVelocity::Midi(100)));
        chain.continuation_velocities.push(VelocityEvidence {
            value: AttackVelocity::Midi(80),
            evidence: evidence("tail"),
        });
        let note = NoteIntensity::resolve(&chain, timeline, &[t(0), t(4)]).unwrap();
        close(note_db(&note, t(2)), 5.0);
        close(note_db(&note, t(4)), 5.0);
        assert!(note
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::ContinuationVelocity));
    }

    #[test]
    fn velocity_only_inputs_and_zero_contracts_are_distinct() {
        let timeline = Arc::new(resolve(vec![]));
        for (velocity, want) in [
            (
                AttackVelocity::Midi(80),
                Evaluation::Known(Intensity::Decibels(0.0)),
            ),
            (AttackVelocity::Midi(0), Evaluation::Absent),
            (
                AttackVelocity::MusicXml(t(0)),
                Evaluation::Known(Intensity::Silence),
            ),
            (
                AttackVelocity::MuseScoreUser {
                    value: t(0),
                    legacy: true,
                },
                Evaluation::Known(Intensity::Decibels(-19.75)),
            ),
            (
                AttackVelocity::MuseScoreUser {
                    value: t(0),
                    legacy: false,
                },
                Evaluation::Absent,
            ),
        ] {
            let resolved =
                NoteIntensity::resolve(&note(Some(velocity)), timeline.clone(), &[t(0)]).unwrap();
            assert_eq!(resolved.evaluate(t(1)), want);
        }
        let absent = NoteIntensity::resolve(&note(None), timeline, &[t(0)]).unwrap();
        assert_eq!(absent.evaluate(t(0)), Evaluation::Absent);
    }

    #[test]
    fn explicit_offset_uses_source_integer_arithmetic_or_declared_reference() {
        for (legacy, expected) in [(true, -6.75), (false, -6.525)] {
            let timeline = Arc::new(resolve(vec![mark(0, "p")]));
            let chain = note(Some(AttackVelocity::MuseScoreOffset {
                percent: t(10),
                legacy,
            }));
            let resolved = NoteIntensity::resolve(&chain, timeline, &[t(0)]).unwrap();
            close(note_db(&resolved, t(0)), expected);
        }
        let timeline = Arc::new(resolve(vec![]));
        let chain = note(Some(AttackVelocity::MuseScoreOffset {
            percent: t(10),
            legacy: true,
        }));
        let resolved = NoteIntensity::resolve(&chain, timeline, &[t(0)]).unwrap();
        close(note_db(&resolved, t(0)), 2.0);
        assert!(resolved
            .provenance
            .as_ref()
            .unwrap()
            .interpretations
            .iter()
            .any(|i| i.field == "offset reference"));
    }

    #[test]
    fn accent_affects_next_attack_tie_chain_then_restores_held_context() {
        let timeline = Arc::new(resolve(vec![mark(0, "p"), mark(1, "sfz")]));
        let mut first = note(None);
        first.start = t(2);
        first.end = t(3);
        let accent = NoteIntensity::resolve(&first, timeline.clone(), &[t(0), t(2), t(3)]).unwrap();
        close(note_db(&accent, t(2)), 8.0);
        let mut next = first;
        next.start = t(3);
        next.end = t(4);
        let next = NoteIntensity::resolve(&next, timeline, &[t(0), t(2), t(3)]).unwrap();
        close(note_db(&next, t(3)), -7.75);
    }

    #[test]
    fn score_mute_dominates_positive_attack_anchor() {
        let timeline = Arc::new(resolve(vec![mark(0, "p"), mark(2, "n")]));
        let note =
            NoteIntensity::resolve(&note(Some(AttackVelocity::Midi(100))), timeline, &[t(0)])
                .unwrap();
        assert_eq!(note.evaluate(t(2)), Evaluation::Known(Intensity::Silence));
    }

    #[test]
    fn gain_composition_deduplicates_owned_sources_without_changing_cc_policy() {
        let cc = GainContributor {
            source_ids: BTreeSet::from(["cc7".into(), "cc11".into()]),
            gain: Some(64.0 / 127.0),
        };
        let gain = compose_gain(
            Evaluation::Known(Intensity::Decibels(0.0)),
            &[cc.clone(), cc.clone()],
        )
        .unwrap()
        .unwrap();
        close(gain, 64.0 / 127.0);
        assert_eq!((200.0 * gain.log10()).round() as i32, -60);
        let master = GainContributor {
            source_ids: BTreeSet::from(["source-master".into()]),
            gain: Some(0.5),
        };
        close(
            compose_gain(
                Evaluation::Known(Intensity::Decibels(0.0)),
                &[cc.clone(), master],
            )
            .unwrap()
            .unwrap(),
            32.0 / 127.0,
        );
        let overlap = GainContributor {
            source_ids: BTreeSet::from(["cc7".into()]),
            gain: Some(0.5),
        };
        assert!(compose_gain(Evaluation::Absent, &[cc, overlap]).is_err());
        assert_eq!(compose_gain(Evaluation::Unknown, &[]).unwrap(), None);
        assert_eq!(compose_gain(Evaluation::Absent, &[]).unwrap(), None);
    }

    #[test]
    fn wedges_match_scope_number_and_keep_continue_evidence_and_niente() {
        let wedge = |at, id, kind| Wedge {
            event: mark(at, id),
            number: None,
            kind,
            niente: false,
        };
        let mut start = wedge(1, "start", WedgeKind::Start(Direction::Diminuendo));
        start.niente = true;
        let continued = wedge(2, "continue", WedgeKind::Continue);
        let stop = wedge(3, "stop", WedgeKind::Stop);
        let mut wrong = wedge(2, "wrong-part", WedgeKind::Stop);
        wrong.event.scope = Scope::Part("P2".into());
        let (events, issues) = pair_wedges(&[start, continued, wrong, stop]).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(issues.len(), 1);
        assert_eq!(events[0].evidence.source_ids, ["continue", "start", "stop"]);
        let Instruction::Transition(transition) = &events[0].instruction else {
            panic!("paired");
        };
        assert!(transition.niente_end);
        assert_eq!(transition.end, t(3));
        let mut source = vec![mark(0, "p")];
        source.extend(events);
        assert_eq!(
            resolve(source).evaluate(t(3)),
            Evaluation::Known(Intensity::Silence)
        );
    }

    #[test]
    fn exact_final_boundary_dynamic_is_not_lost_to_half_open_segments() {
        let score = ScoreIntensity {
            events: vec![
                mark(0, "p"),
                ramp(1, 3, Direction::Crescendo),
                mark(3, "pp"),
            ],
        };
        let timeline = score.resolve(&owner(), 1, t(3), t(3)).unwrap();
        close(db(&timeline, t(3)), -11.75);
    }

    #[test]
    fn exact_decimal_level_remains_exact_when_it_becomes_a_ramp_start() {
        let exact = Fraction::decimal("49.123456789012345").unwrap();
        let mut start = mark(0, "p");
        if let Instruction::Dynamic(d) = &mut start.instruction {
            d.numeric = Some(NumericLevel::Level(exact));
        }
        let timeline = resolve(vec![start, ramp(1, 2, Direction::Crescendo)]);
        let Curve::Transition { from, .. } = timeline.segment_at(t(1)).unwrap().curve else {
            panic!("ramp");
        };
        assert_eq!(from, Level::Positive(exact));
    }

    #[test]
    fn duplicate_equivalent_ramps_are_one_instruction_with_shared_evidence() {
        let mut second = ramp(1, 3, Direction::Crescendo);
        second.evidence = evidence("ramp-copy");
        let timeline = resolve(vec![mark(0, "p"), ramp(1, 3, Direction::Crescendo), second]);
        close(db(&timeline, t(2)), -5.875);
        assert!(timeline.issues.is_empty());
        assert!(timeline
            .segment_at(t(2))
            .unwrap()
            .provenance
            .as_ref()
            .unwrap()
            .evidence
            .iter()
            .any(|e| e.source_ids.contains(&"ramp-copy".into())));
    }

    #[test]
    fn simultaneous_incompatible_ramps_preserve_unaffected_longer_tail() {
        let timeline = resolve(vec![
            mark(0, "p"),
            ramp(1, 3, Direction::Crescendo),
            ramp(1, 5, Direction::Diminuendo),
        ]);
        assert_eq!(timeline.evaluate(t(2)), Evaluation::Unknown);
        assert!(matches!(timeline.evaluate(t(4)), Evaluation::Known(_)));
        assert!(timeline
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Conflict && i.start == t(1) && i.end == t(3)));
    }

    #[test]
    fn same_priority_level_conflict_report_stops_when_state_is_replaced() {
        let timeline = resolve(vec![mark(0, "p"), mark(0, "f"), mark(2, "mp")]);
        assert_eq!(
            timeline
                .issues
                .iter()
                .find(|i| i.kind == IssueKind::Conflict)
                .unwrap()
                .end,
            t(2)
        );
    }

    #[test]
    fn sfz_with_explicit_dynamic_delta_is_active_and_uses_pinned_duration() {
        let mut sfz = mark(0, "sfz");
        sfz.evidence.contract = SourceContract::MuseScoreLegacy;
        if let Instruction::Dynamic(d) = &mut sfz.instruction {
            d.velo_change = Some(t(-20));
            d.tempo_at_start = Some(t(120));
        }
        let timeline = resolve(vec![sfz]);
        close(db(&timeline, t(0)), 8.0);
        close(db(&timeline, q(2, 5)), 5.5);
        close(db(&timeline, q(4, 5)), 3.0);
    }

    #[test]
    fn narrower_next_attack_accent_wins_regardless_of_source_order() {
        let broad = mark(1, "sfz");
        let mut narrow = mark(1, "sffz");
        narrow.scope = Scope::musicxml("P1", Some("1"), Some("1"));
        let timeline = Arc::new(resolve(vec![mark(0, "p"), narrow, broad]));
        let mut chain = note(None);
        chain.start = t(2);
        let note = NoteIntensity::resolve(&chain, timeline, &[t(0), t(2)]).unwrap();
        close(note_db(&note, t(2)), 11.5);
    }

    #[test]
    fn positive_composed_underflow_is_reported_instead_of_becoming_mute() {
        let a = GainContributor {
            source_ids: BTreeSet::from(["a".into()]),
            gain: Some(1e-300),
        };
        let b = GainContributor {
            source_ids: BTreeSet::from(["b".into()]),
            gain: Some(1e-300),
        };
        assert!(compose_gain(Evaluation::Absent, &[a, b])
            .unwrap_err()
            .contains("underflow"));
        assert!(compose_gain(Evaluation::Known(Intensity::Decibels(-10000.0)), &[]).is_err());
    }

    #[test]
    fn repeat_expansion_keeps_final_boundary_step_and_rejects_overlapping_runs() {
        let score = ScoreIntensity {
            events: vec![
                mark(0, "p"),
                ramp(1, 3, Direction::Crescendo),
                mark(3, "pp"),
            ],
        };
        let run = Occurrence {
            written_start: t(0),
            written_end: t(3),
            performed_start: t(0),
            pass: 1,
            ordinal: 1,
        };
        let timeline = score
            .occurrences(&owner(), std::slice::from_ref(&run), &[t(3)])
            .unwrap();
        close(db(&timeline, t(3)), -11.75);
        assert!(score
            .occurrences(&owner(), &[run.clone(), run], &[t(3), t(3)])
            .is_err());
    }

    #[test]
    fn disabled_wedge_and_disjoint_time_only_passes_never_activate() {
        let mut start = Wedge {
            event: mark(0, "start"),
            number: Some("2".into()),
            kind: WedgeKind::Start(Direction::Crescendo),
            niente: false,
        };
        let mut stop = Wedge {
            event: mark(2, "stop"),
            number: Some("2".into()),
            kind: WedgeKind::Stop,
            niente: false,
        };
        start.event.time_only = vec![1];
        stop.event.time_only = vec![2];
        let (events, issues) = pair_wedges(&[start.clone(), stop.clone()]).unwrap();
        assert!(!issues.is_empty());
        assert!(events.is_empty());
        assert_eq!(resolve(events).evaluate(t(1)), Evaluation::Absent);
        start.event.time_only.clear();
        stop.event.time_only.clear();
        stop.event.enabled = false;
        let (events, _) = pair_wedges(&[start, stop]).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn source_review5_wedge_pairing_keeps_disabled_equal_duplicate_and_continue_evidence() {
        let wedge = |at, id, kind| Wedge {
            event: event(t(at), id, Instruction::Unsupported(String::new())),
            number: Some("1".into()),
            kind,
            niente: false,
        };
        let start = wedge(0, "start", WedgeKind::Start(Direction::Crescendo));
        let equal = wedge(0, "equal", WedgeKind::Stop);
        let mut disabled = wedge(1, "disabled", WedgeKind::Stop);
        disabled.event.enabled = false;
        let continued = wedge(2, "continued", WedgeKind::Continue);
        let end = wedge(4, "end", WedgeKind::Stop);
        let duplicate = wedge(4, "duplicate", WedgeKind::Stop);
        let mut unrelated = wedge(1, "unrelated", WedgeKind::Stop);
        unrelated.number = Some("2".into());
        let wedges = [start, equal, disabled, continued, end, duplicate, unrelated];
        let (events, issues) = pair_wedges(&wedges).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0].instruction, Instruction::Transition(tn) if tn.end == t(4)));
        assert_eq!(
            events[0].evidence.source_ids,
            ["continued", "duplicate", "end", "start"]
        );
        assert!(issues.iter().any(|i| i.kind == IssueKind::Disabled));
        for id in ["equal", "unrelated"] {
            assert!(issues.iter().any(|i| i
                .provenance
                .evidence
                .iter()
                .any(|e| e.source_ids.iter().any(|raw| raw == id))));
        }
        let mut denied = ProvenanceBudget::with_limits(0, 0);
        assert!(pair_wedges_bounded(&wedges, &[1, 2], None, &mut denied)
            .is_err_and(|e| e.contains("SCORE_INTENSITY_LIMIT")));
    }

    #[test]
    fn extending_a_tie_never_defaults_through_unknown_or_muted_attack() {
        for start in [
            mark(0, "n"),
            event(
                t(0),
                "invalid",
                Instruction::Dynamic(Dynamic {
                    numeric: Some(NumericLevel::Level(t(200))),
                    ..Dynamic::default()
                }),
            ),
        ] {
            let timeline = Arc::new(resolve(vec![start, ramp(2, 4, Direction::Crescendo)]));
            let mut head_chain = note(Some(AttackVelocity::Midi(100)));
            head_chain.end = t(1);
            let head = NoteIntensity::resolve(&head_chain, timeline.clone(), &[t(0)]).unwrap();
            let mut tail_chain = note(Some(AttackVelocity::Midi(20)));
            tail_chain.start = t(1);
            tail_chain.end = t(4);
            let tail = NoteIntensity::resolve(&tail_chain, timeline, &[t(0), t(1)]).unwrap();
            let inherited = NoteIntensity::continued_from(&head, Some(&tail));
            assert_eq!(inherited.evaluate(t(3)), Evaluation::Unknown);
            assert_eq!(inherited.anchor, head.anchor);
            assert_eq!(inherited.anchor_reference, head.anchor_reference);
        }
    }

    #[test]
    fn no_positive_attack_reference_has_an_explicit_local_issue() {
        let timeline = Arc::new(resolve(vec![mark(2, "p")]));
        let note =
            NoteIntensity::resolve(&note(Some(AttackVelocity::Midi(100))), timeline, &[t(0)])
                .unwrap();
        close(note_db(&note, t(1)), 5.0);
        assert_eq!(note.evaluate(t(2)), Evaluation::Unknown);
        assert!(note
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::UnknownAnchor));
    }

    #[test]
    fn contradictory_endpoints_do_not_choose_the_ramp_by_source_order() {
        for endpoints in [[mark(3, "f"), mark(3, "ff")], [mark(3, "ff"), mark(3, "f")]] {
            let mut events = vec![mark(0, "p"), ramp(1, 3, Direction::Crescendo)];
            events.extend(endpoints);
            let timeline = resolve(events);
            close(db(&timeline, t(2)), -5.875);
            assert_eq!(timeline.evaluate(t(3)), Evaluation::Unknown);
            assert!(timeline
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::Conflict && i.start == t(1)));
        }
    }

    #[test]
    fn merged_provenance_still_attributes_each_numeric_or_table_field() {
        let timeline = resolve(vec![
            mark(0, "p"),
            ramp(1, 3, Direction::Crescendo),
            mark(3, "f"),
        ]);
        let provenance = timeline
            .segment_at(t(2))
            .unwrap()
            .provenance
            .as_ref()
            .unwrap();
        assert!(provenance
            .interpretations
            .iter()
            .any(|i| i.exact == Some(t(49)) && i.source_ids == ["p"]));
        assert!(provenance
            .interpretations
            .iter()
            .any(|i| i.exact == Some(t(96)) && i.source_ids == ["f"]));
    }
}
