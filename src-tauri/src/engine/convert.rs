//! MIDI -> Synthesizer V conversion logic. 1:1 port of kar2svp_core.py.
use crate::engine::midi::{self, Event, Kind, Midi};
use crate::engine::svp::*;
use std::collections::{BTreeMap, HashMap};

// Track-name heuristics. Includes common French names so scores authored in
// French (Batterie, Guitare, Ukulele...) classify correctly out of the box.
const VOCAL_KW: [&str; 15] = [
    "vox", "voice", "vocal", "voc", "melod", "lead", "chant", "sing", "choir", "voix", "choeur",
    "ch\u{153}ur", "soprano", "tenor", "baryton",
];
const INSTR_KW: [&str; 31] = [
    "guitar", "bass", "drum", "piano", "snare", "hat", "cymbal", "clap", "rim", "kick", "perc",
    "organ", "synth", "string", "brass", "sax", "trumpet", "trombone", "horn", "flute", "violin",
    "cello", "harp", "batterie", "guitare", "ukul", "violon", "trompette", "clavier", "orgue",
    "fl\u{fb}t",
];

pub struct TrackReport {
    pub id: usize, // stable track identifier (original order), used for overrides
    pub track: String,
    pub notes: usize,
    pub role: String,
    pub placed: usize,
}

pub struct ConvertOutcome {
    pub ok: bool,
    pub msg: Option<String>,
    pub svp: Option<SvpProject>,
    pub tracks: Vec<TrackReport>,
    pub n_tracks: usize,
    pub placed: usize,
}

fn clean_syllable(t: &str) -> Option<String> {
    if t.is_empty() || t.starts_with('@') {
        return None;
    }
    let s = t.trim_start_matches(['\\', '/']);
    let s: String = s.chars().filter(|&c| c != '\r' && c != '\n').collect();
    let strip: &[char] = &[
        '.', ',', '!', '?', ';', ':', '"', '«', '»', '\u{201c}', '\u{201d}', '(', ')', '[', ']',
        '{', '}', '\u{2026}', ' ',
    ];
    let s = s.trim().trim_matches(|c| strip.contains(&c));
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn track_name(ev: &[Event]) -> String {
    // End-trim only: .kar/.mid names must stay byte-identical to the Python
    // oracle. XML parsers normalize their display names at the source
    // (musescore.rs / musicxml.rs collapse whitespace runs).
    for e in ev {
        if let Kind::TrackName(t) = &e.kind {
            return t.trim().to_string();
        }
    }
    String::new()
}

/// Sorted (onset, dur, pitch).
fn extract_notes(ev: &[Event]) -> Vec<(u32, u32, u8)> {
    let mut active: HashMap<u8, Vec<u32>> = HashMap::new();
    let mut out: Vec<(u32, u32, u8)> = Vec::new();
    for e in ev {
        match &e.kind {
            Kind::NoteOn(n) => active.entry(*n).or_default().push(e.tick),
            Kind::NoteOff(n) => {
                if let Some(v) = active.get_mut(n) {
                    if !v.is_empty() {
                        let on = v.remove(0);
                        if e.tick > on {
                            out.push((on, e.tick - on, *n));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out.sort();
    out
}

fn track_tokens(ev: &[Event]) -> Vec<(u32, String)> {
    let mut t = Vec::new();
    for e in ev {
        let txt = match &e.kind {
            Kind::Lyrics(s) | Kind::Text(s) => Some(s),
            _ => None,
        };
        if let Some(s) = txt {
            if let Some(c) = clean_syllable(s) {
                t.push((e.tick, c));
            }
        }
    }
    t
}

fn assign_monotonic(lyric_ticks: &[u32], onsets: &[u32], tol: u32) -> HashMap<usize, usize> {
    let mut res = HashMap::new();
    let mut j = 0usize;
    let m = onsets.len();
    let d = |a: u32, b: u32| (a as i64 - b as i64).abs();
    for (li, &tk) in lyric_ticks.iter().enumerate() {
        while j + 1 < m && d(onsets[j + 1], tk) < d(onsets[j], tk) {
            j += 1;
        }
        if j < m && d(onsets[j], tk) <= tol as i64 {
            res.insert(j, li);
            j += 1;
        }
    }
    res
}

fn read_tempo(midi: &Midi, bpt: f64) -> Vec<Tempo> {
    let mut seen: BTreeMap<i64, f64> = BTreeMap::new();
    for ev in &midi.tracks {
        for e in ev {
            if let Kind::Tempo(us) = e.kind {
                if us > 0 {
                    let pos = (e.tick as f64 * bpt).round() as i64;
                    let bpm = (60_000_000.0 / us as f64 * 1e6).round() / 1e6;
                    seen.insert(pos, bpm);
                }
            }
        }
    }
    if seen.is_empty() {
        return vec![Tempo { bpm: 120.0, position: 0 }];
    }
    seen.into_iter().map(|(position, bpm)| Tempo { bpm, position }).collect()
}

fn read_meter(midi: &Midi) -> Vec<Meter> {
    let mut out = Vec::new();
    for ev in &midi.tracks {
        for e in ev {
            if let Kind::TimeSig { num, den } = e.kind {
                out.push(Meter {
                    denominator: den as u32,
                    index: out.len() as u32,
                    numerator: num as u32,
                });
            }
        }
    }
    if out.is_empty() {
        vec![Meter { denominator: 4, index: 0, numerator: 4 }]
    } else {
        out
    }
}

fn build_track(idx: usize, name: String, notes: Vec<Note>, render: bool, mute: bool) -> SvpTrack {
    let uid = uuid(idx);
    SvpTrack {
        name,
        disp_color: COLORS[idx % COLORS.len()].to_string(),
        disp_order: idx as u32,
        render_enabled: render,
        mixer: Mixer { gain_decibel: 0.0, pan: 0.0, mute, solo: false, display: true },
        main_ref: MainRef {
            audio: Audio { filename: String::new(), duration: 0 },
            database: Database { name: String::new(), language: String::new(), phoneset: String::new() },
            dictionary: String::new(),
            voice: serde_json::json!({}),
            group_id: uid.clone(),
            is_instrumental: false,
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

/// Lyric tokens of the vocal stream + note-index -> token-index assignment.
type VocalAssign<'a> = (&'a [(u32, String)], &'a HashMap<usize, usize>);

fn make_track(
    idx: usize,
    name: &str,
    notes: &[(u32, u32, u8)],
    bpt: f64,
    sing: bool,
    vocal: Option<VocalAssign>,
) -> SvpTrack {
    let mut svp_notes = Vec::with_capacity(notes.len());
    let mut prev = false;
    for (i, &(onset, dur, pitch)) in notes.iter().enumerate() {
        let lyric = match (sing, vocal) {
            (true, Some((tok, asg))) => {
                if let Some(&li) = asg.get(&i) {
                    prev = true;
                    tok[li].1.clone()
                } else if prev {
                    "-".to_string()
                } else {
                    "la".to_string()
                }
            }
            _ => "la".to_string(),
        };
        svp_notes.push(Note {
            attributes: serde_json::json!({}),
            duration: (dur as f64 * bpt).round() as i64,
            lyrics: lyric,
            onset: (onset as f64 * bpt).round() as i64,
            phonemes: String::new(),
            pitch,
        });
    }
    build_track(idx, name.to_string(), svp_notes, sing, !sing)
}

fn make_synth(idx: usize, stream: &[(u32, String)], tpb: u16, bpt: f64) -> SvpTrack {
    let ticks: Vec<u32> = stream.iter().map(|s| s.0).collect();
    let dmin = (tpb as u32) / 8;
    let dmax = (tpb as u32) * 2;
    let mut svp_notes = Vec::with_capacity(stream.len());
    for (i, (tk, syl)) in stream.iter().enumerate() {
        let nxt = if i + 1 < ticks.len() { ticks[i + 1] } else { tk + (tpb as u32) / 2 };
        let raw = nxt.saturating_sub(*tk);
        let dur = raw.min(dmax).max(dmin);
        svp_notes.push(Note {
            attributes: serde_json::json!({}),
            duration: (dur as f64 * bpt).round() as i64,
            lyrics: syl.clone(),
            onset: (*tk as f64 * bpt).round() as i64,
            phonemes: String::new(),
            pitch: 60,
        });
    }
    build_track(idx, "Lyrics (melody to adjust)".into(), svp_notes, true, false)
}

/// Detects the format (MIDI / MusicXML / MuseScore) and converts.
pub fn convert_auto(data: &[u8], language: &str) -> ConvertOutcome {
    convert_auto_with(data, language, None)
}

/// Like `convert_auto`, with per-track Sings/Muted overrides.
pub fn convert_auto_with(
    data: &[u8],
    language: &str,
    overrides: Option<&HashMap<usize, bool>>,
) -> ConvertOutcome {
    use crate::engine::musescore as ms;
    use crate::engine::musicxml as mx;
    let fail = |m: String| ConvertOutcome {
        ok: false, msg: Some(m), svp: None, tracks: vec![], n_tracks: 0, placed: 0,
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
        return fail("archive contains no recognized score (neither MusicXML nor MuseScore)".into());
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
/// forces the track to sing, `false` forces it to backing.
pub fn convert_midi_with(
    midi: &Midi,
    language: &str,
    overrides: Option<&HashMap<usize, bool>>,
) -> ConvertOutcome {
    let tpb = midi.ticks_per_beat;
    let bpt = BLICKS_PER_QUARTER / tpb as f64;
    let tol = (tpb / 2).max(1) as u32;

    let streams: Vec<Vec<(u32, String)>> = midi
        .tracks
        .iter()
        .map(|ev| track_tokens(ev))
        .filter(|t| !t.is_empty())
        .collect();
    if streams.is_empty() {
        return ConvertOutcome {
            ok: false,
            msg: Some("no lyrics found in this file".into()),
            svp: None,
            tracks: vec![],
            n_tracks: 0,
            placed: 0,
        };
    }

    let mut svp_tracks: Vec<SvpTrack> = Vec::new();
    let mut report: Vec<TrackReport> = Vec::new();
    let mut idx = 0usize;
    let mut total_placed = 0usize;

    for ev in &midi.tracks {
        let notes = extract_notes(ev);
        if notes.is_empty() {
            continue;
        }
        let raw_name = track_name(ev);
        let name = if raw_name.is_empty() { format!("Track {}", idx) } else { raw_name.clone() };
        let lname = raw_name.to_lowercase();
        let onsets: Vec<u32> = notes.iter().map(|n| n.0).collect();

        let mut best_idx: Option<usize> = None;
        let mut best_assign: HashMap<usize, usize> = HashMap::new();
        let mut best_m = 0usize;
        for (si, st) in streams.iter().enumerate() {
            let ticks: Vec<u32> = st.iter().map(|t| t.0).collect();
            let a = assign_monotonic(&ticks, &onsets, tol);
            if a.len() > best_m {
                best_m = a.len();
                best_idx = Some(si);
                best_assign = a;
            }
        }
        let ratio = best_m as f64 / notes.len() as f64;
        let coverage = match best_idx {
            Some(si) => best_m as f64 / streams[si].len() as f64,
            None => 0.0,
        };
        // Two profiles of singing track:
        //  - lead voice: covers most of the text (coverage);
        //  - harmonies/choirs: low coverage but most of THEIR notes land on
        //    syllables (high ratio).
        let mut vocal = best_m > 0
            && ((coverage >= 0.8 && ratio >= 0.35) || (best_m >= 10 && ratio >= 0.75));
        // A track carrying ITS OWN lyrics aligned with ITS notes sings,
        // whatever its name (MusicXML case: choir voice named "Bass").
        let own = track_tokens(ev);
        let sings_itself = if own.is_empty() {
            false
        } else {
            let ticks: Vec<u32> = own.iter().map(|t| t.0).collect();
            let a = assign_monotonic(&ticks, &onsets, tol);
            a.len() >= 10 && a.len() * 2 >= own.len()
        };
        vocal = vocal || sings_itself;
        if INSTR_KW.iter().any(|k| lname.contains(k))
            && !VOCAL_KW.iter().any(|k| lname.contains(k))
            && !sings_itself
        {
            vocal = false;
        }

        // User override: it always has the last word.
        let sing = overrides
            .and_then(|o| o.get(&idx).copied())
            .unwrap_or(vocal);
        let vocal_data = best_idx.map(|si| (&streams[si][..], &best_assign));
        let placed = if sing { best_m } else { 0 };
        svp_tracks.push(make_track(idx, &name, &notes, bpt, sing, vocal_data));
        total_placed += placed;
        report.push(TrackReport {
            id: idx,
            track: name,
            notes: notes.len(),
            role: if sing { "vocal".into() } else { "backing".into() },
            placed,
        });
        idx += 1;
    }

    if total_placed == 0 {
        let st = streams.iter().max_by_key(|s| s.len()).unwrap();
        svp_tracks.push(make_synth(idx, st, tpb, bpt));
        report.push(TrackReport {
            id: idx,
            track: "Lyrics (melody to adjust)".into(),
            notes: st.len(),
            role: "vocal_synth".into(),
            placed: st.len(),
        });
        total_placed = st.len();
    }

    // Singing tracks first (visible at the top in SV) + singing language.
    let mut idx_order: Vec<usize> = (0..report.len()).collect();
    idx_order.sort_by_key(|&i| {
        if report[i].role == "vocal" || report[i].role == "vocal_synth" { 0usize } else { 1 }
    });
    let mut opt_t: Vec<Option<SvpTrack>> = svp_tracks.into_iter().map(Some).collect();
    let mut opt_r: Vec<Option<TrackReport>> = report.into_iter().map(Some).collect();
    let mut svp_tracks: Vec<SvpTrack> = Vec::with_capacity(opt_t.len());
    let mut report: Vec<TrackReport> = Vec::with_capacity(opt_r.len());
    for &i in &idx_order {
        svp_tracks.push(opt_t[i].take().unwrap());
        report.push(opt_r[i].take().unwrap());
    }
    for (k, tr) in svp_tracks.iter_mut().enumerate() {
        tr.disp_order = k as u32;
        tr.disp_color = COLORS[k % COLORS.len()].to_string();
        if report[k].role == "vocal" || report[k].role == "vocal_synth" {
            tr.main_ref.database.language = language.to_string();
        }
    }

    let svp = SvpProject {
        version: 113,
        time: Time { meter: read_meter(midi), tempo: read_tempo(midi, bpt) },
        render_config: RenderConfig::default(),
        tracks: svp_tracks,
    };
    let n_tracks = svp.tracks.len();
    ConvertOutcome {
        ok: true,
        msg: None,
        svp: Some(svp),
        tracks: report,
        n_tracks,
        placed: total_placed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_name_end_trims_only_for_oracle_parity() {
        // Inner whitespace of .kar/.mid names must survive byte-identical;
        // XML parsers normalize their names before emitting the event.
        let ev = vec![Event { tick: 0, kind: Kind::TrackName("  Lead  Vocal  ".into()) }];
        assert_eq!(track_name(&ev), "Lead  Vocal");
    }
}
