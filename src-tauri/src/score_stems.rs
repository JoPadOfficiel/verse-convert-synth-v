//! Score stems rendered from the whole source score.
//!
//! A Part rendered on its own loses the timing other Parts impose on the
//! score: a fermata or breath written on one Part holds every Part. A stem is
//! therefore the complete score in which every other Part's notes and chord
//! symbols are set not to play, so it shares the reference mix's timeline.

use crate::engine::midi::SourceTopology;
use crate::engine::musescore;
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

const SILENCE: &[u8] = b"<play>0</play>";
const MAX_MASTER_BYTES: u64 = 64 * 1024 * 1024;

/// The isolation method recorded in the manifest for these stems.
pub const ISOLATION_METHOD: &str = "musescore-silenced-score";

#[derive(Debug)]
enum Container {
    Zip {
        archive: Vec<u8>,
        master_path: String,
    },
    Mscx,
}

/// One main-score Part as MuseScore reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScorePart {
    /// The Part ID the MuseScore parser gives this Part.
    pub id: String,
    /// Resolved IDs of the staves this Part declares, including linked views.
    pub staff_ids: Vec<String>,
    /// The Part's own `<trackName>` text, exactly as written.
    pub track_name: Option<String>,
    /// Byte offsets, in the master score, of the closing tag of every note and
    /// chord symbol this Part plays.
    audible: Vec<usize>,
}

impl ScorePart {
    pub fn audible_elements(&self) -> usize {
        self.audible.len()
    }
}

/// A MuseScore container whose Parts can be silenced one by one.
#[derive(Debug)]
pub struct ScoreStems {
    container: Container,
    master: Vec<u8>,
    parts: Vec<ScorePart>,
    render_prepared: bool,
}

impl ScoreStems {
    /// Read a `.mscz` archive or a raw `.mscx` score. Every body staff that
    /// holds a note or chord symbol must belong to exactly one Part.
    pub fn read(container: &[u8]) -> Result<Self, String> {
        let (container, master) = if crate::engine::musicxml::is_zip(container) {
            let unsilenceable = |error: String| {
                if error.starts_with("SCORE_STEM_") {
                    error
                } else {
                    format!("SCORE_STEM_UNSILENCEABLE: {error}")
                }
            };
            let master_path = musescore::master_mscx_path(container).map_err(unsilenceable)?;
            let master = read_entry(container, &master_path).map_err(unsilenceable)?;
            (
                Container::Zip {
                    archive: container.to_vec(),
                    master_path,
                },
                master,
            )
        } else {
            if container.len() as u64 > MAX_MASTER_BYTES {
                return Err(too_large());
            }
            (Container::Mscx, container.to_vec())
        };
        let parts = score_parts(&master)?;
        Ok(Self {
            container,
            master,
            parts,
            render_prepared: false,
        })
    }

    pub fn parts(&self) -> &[ScorePart] {
        &self.parts
    }

    /// File extension the renderer needs to read the silenced container.
    pub fn extension(&self) -> &'static str {
        match self.container {
            Container::Zip { .. } => "mscz",
            Container::Mscx => "mscx",
        }
    }

    /// Preserve qualified legacy playback defaults and drum templates in a
    /// private MuseScore 3/4 render copy. The preserved source is never changed.
    pub fn prepare_for_renderer(&mut self, major: u32) -> Result<Vec<String>, String> {
        if !matches!(major, 3 | 4) {
            return Ok(Vec::new());
        }
        let (master, mut warnings) = if major == 4 {
            prepare_drum_templates(&self.master)?
        } else {
            (self.master.clone(), Vec::new())
        };
        let (master, timing_warnings) = preserve_legacy_fermata_defaults(&master)?;
        warnings.extend(timing_warnings);
        if warnings.is_empty() {
            return Ok(warnings);
        }
        let parts = score_parts(&master)?;
        match &mut self.container {
            Container::Mscx => {}
            Container::Zip {
                archive,
                master_path,
            } => {
                *archive = replace_entry(archive, master_path, &master)
                    .map_err(|error| format!("MUSESCORE_RENDER_PREPARATION_FAILED: {error}"))?;
            }
        }
        self.master = master;
        self.parts = parts;
        self.render_prepared = true;
        Ok(warnings)
    }

    pub fn has_render_preparation(&self) -> bool {
        self.render_prepared
    }

    /// The coherent full-score render input, before Part silencing.
    pub fn render_container(&self) -> &[u8] {
        match &self.container {
            Container::Zip { archive, .. } => archive,
            Container::Mscx => &self.master,
        }
    }

    /// Check that this container's Parts are the source's, in order.
    /// `native` containers are the source itself and keep its Part IDs; a
    /// converted score only keeps the Part and staff structure.
    pub fn validate_topology(&self, topology: &SourceTopology, native: bool) -> Result<(), String> {
        if self.parts.len() != topology.parts.len() {
            return Err(format!(
                "SCORE_STEM_TOPOLOGY_MISMATCH: the MuseScore score has {} Parts but the source topology has {}",
                self.parts.len(),
                topology.parts.len()
            ));
        }
        for (index, (part, source)) in self.parts.iter().zip(&topology.parts).enumerate() {
            let same_staves = if native {
                part.id == source.id
                    && source
                        .staves
                        .iter()
                        .all(|staff| part.staff_ids.contains(&staff.id))
            } else {
                part.staff_ids.len() == source.staves.len()
            };
            if !same_staves {
                return Err(format!(
                    "SCORE_STEM_TOPOLOGY_MISMATCH: MuseScore Part {} does not match source Part {:?}",
                    index + 1,
                    source.id
                ));
            }
        }
        Ok(())
    }

    /// The container with every Part other than `keep` silenced. Only
    /// `<play>0</play>` is inserted into the master score; every other entry
    /// keeps its original compressed bytes, and a score with nothing else
    /// audible is returned byte for byte.
    pub fn silenced(&self, keep: usize) -> Result<Vec<u8>, String> {
        if keep >= self.parts.len() {
            return Err(format!(
                "SCORE_STEM_TOPOLOGY_MISMATCH: the score has no Part {}",
                keep + 1
            ));
        }
        let mut offsets: Vec<usize> = self
            .parts
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != keep)
            .flat_map(|(_, part)| part.audible.iter().copied())
            .collect();
        offsets.sort_unstable();
        if offsets.is_empty() {
            return Ok(match &self.container {
                Container::Zip { archive, .. } => archive.clone(),
                Container::Mscx => self.master.clone(),
            });
        }
        let mut master = Vec::with_capacity(self.master.len() + offsets.len() * SILENCE.len());
        let mut copied = 0;
        for offset in offsets {
            master.extend_from_slice(&self.master[copied..offset]);
            master.extend_from_slice(SILENCE);
            copied = offset;
        }
        master.extend_from_slice(&self.master[copied..]);
        match &self.container {
            Container::Mscx => Ok(master),
            Container::Zip {
                archive,
                master_path,
            } => replace_entry(archive, master_path, &master)
                .map_err(|error| format!("SCORE_STEM_UNSILENCEABLE: {error}")),
        }
    }
}

fn read_entry(archive: &[u8], path: &str) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(|e| e.to_string())?;
    let entry = zip.by_name(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    entry
        .take(MAX_MASTER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_MASTER_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}

fn too_large() -> String {
    "SCORE_STEM_UNSILENCEABLE: abnormally large MuseScore score (rejected for safety)".into()
}

/// A narrowly qualified compatibility exception, not instrument inference.
/// Pinned MuseScore 4 resolves `piano` before consulting percussion evidence;
/// its `drumset` template selects the Standard kit without replacing the map.
/// MuseScore 3.6.2 Fermata::propertyDefault(TIME_STRETCH) is 1.0; MuseScore
/// 4.7.5 uses subtype-dependent defaults, including 2.0 for a normal fermata.
/// Pin only omitted durations to neutral playback in the private copy.
/// Numerically authored durations remain source evidence. Version4 software
/// defaults must not reintroduce timing that the vocal projection did not use.
fn preserve_legacy_fermata_defaults(master: &[u8]) -> Result<(Vec<u8>, Vec<String>), String> {
    let offset = usize::from(master.starts_with(&[0xef, 0xbb, 0xbf])) * 3;
    let xml = std::str::from_utf8(&master[offset..]).map_err(|e| e.to_string())?;
    let document = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 5_000_000,
        },
    )
    .map_err(|e| e.to_string())?;
    let native_major = document
        .root_element()
        .attribute("version")
        .and_then(|v| v.split('.').next());
    if !matches!(native_major, Some("3" | "4")) {
        return Ok((master.to_vec(), Vec::new()));
    }
    let score = document
        .root_element()
        .children()
        .find(|n| n.has_tag_name("Score"))
        .ok_or("SCORE_STEM_UNSILENCEABLE: MuseScore Score element not found")?;
    let active_explicit = score
        .descendants()
        .filter(|n| {
            n.has_tag_name("Fermata")
                && n.ancestors().find(|a| a.has_tag_name("Score")) == Some(score)
        })
        .any(|mark| {
            !mark
                .children()
                .any(|n| n.has_tag_name("play") && n.text().is_some_and(|v| v.trim() == "0"))
                && mark
                    .children()
                    .find(|n| n.has_tag_name("timeStretch"))
                    .and_then(|n| n.text())
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .is_some_and(|v| v.is_finite() && v > 0.0 && v != 1.0)
        });
    if native_major == Some("3") && active_explicit {
        let plan = musescore::written_playback_plan(xml)?;
        if plan.has_implicit_gaps {
            return preserve_legacy_playback_map(master, &document, score, offset);
        }
    }
    let mut edits = Vec::new();
    let mut omitted_marks = 0usize;
    for node in score.descendants().filter(|n| {
        n.has_tag_name("Fermata") && n.ancestors().find(|a| a.has_tag_name("Score")) == Some(score)
    }) {
        if node.children().any(|n| n.has_tag_name("timeStretch")) {
            continue;
        }
        omitted_marks += 1;
        let mut plays = node.children().filter(|n| n.has_tag_name("play"));
        let play = plays.next();
        if plays.next().is_some() {
            return Err("MUSESCORE_RENDER_PREPARATION_FAILED: duplicate fermata play".into());
        }
        if let Some(play) = play {
            if play.children().any(|n| n.is_element())
                || !matches!(play.text().map(str::trim), Some("0" | "1"))
            {
                return Err("MUSESCORE_RENDER_PREPARATION_FAILED: invalid fermata play".into());
            }
            if play.text().map(str::trim) == Some("1") {
                let mut text_nodes = play.children().filter(|n| n.is_text());
                let text = text_nodes
                    .next()
                    .ok_or("MUSESCORE_RENDER_PREPARATION_FAILED: missing fermata play text")?;
                if text_nodes.next().is_some() {
                    return Err(
                        "MUSESCORE_RENDER_PREPARATION_FAILED: ambiguous fermata play text".into(),
                    );
                }
                let range = text.range();
                edits.push((range.start + offset, range.end + offset, b"0".to_vec()));
            }
        }
        let disabled = if play.is_none() { "<play>0</play>" } else { "" };
        let range = node.range();
        if xml[range.clone()].ends_with("/>") {
            let tag = xml[range.start + 1..range.end - 2]
                .split_whitespace()
                .next()
                .ok_or("MUSESCORE_RENDER_PREPARATION_FAILED: missing fermata tag")?;
            edits.push((
                range.end - 2 + offset,
                range.end + offset,
                format!("><timeStretch>1</timeStretch>{disabled}</{tag}>").into_bytes(),
            ));
        } else {
            let at = closing_tag_offset(xml, node)? + offset;
            edits.push((
                at,
                at,
                format!("<timeStretch>1</timeStretch>{disabled}").into_bytes(),
            ));
        }
    }
    if edits.is_empty() {
        return Ok((master.to_vec(), Vec::new()));
    }
    let count = omitted_marks;
    edits.sort_by_key(|edit| edit.0);
    let mut size = master.len();
    let mut covered = 0;
    for (start, end, replacement) in &edits {
        if *start < covered || start > end || *end > master.len() {
            return Err("MUSESCORE_RENDER_PREPARATION_FAILED: overlapping timing edits".into());
        }
        size = size
            .checked_sub(end - start)
            .and_then(|n| n.checked_add(replacement.len()))
            .filter(|n| (*n as u64) <= MAX_MASTER_BYTES)
            .ok_or_else(too_large)?;
        covered = *end;
    }
    let mut prepared = Vec::with_capacity(size);
    let mut copied = 0;
    for (start, end, replacement) in edits {
        prepared.extend_from_slice(&master[copied..start]);
        prepared.extend_from_slice(&replacement);
        copied = end;
    }
    prepared.extend_from_slice(&master[copied..]);
    let warning = if native_major == Some("3") {
        format!("[MUSESCORE_LEGACY_FERMATA_DEFAULT_PRESERVED] Preserved written timing by disabling undefined playback for {count} source MuseScore 3 fermata marks in the private reference/stem render inputs; source notes and tempo are unchanged.")
    } else {
        format!("[MUSESCORE_IMPLICIT_FERMATA_TIMING_PRESERVED] {count} fermata symbols without an explicit playback duration remain in source evidence. Their undefined playback is disabled only in private reference/stem inputs, preserving written tempo and note timing; explicit stretches are unchanged.")
    };
    Ok((prepared, vec![warning]))
}

/// Native 4 may subdivide implicit gaps absent from native 3 and shorten
/// an explicit fermata. Lower the original written playback map into tempo
/// annotations, with native fermata playback neutralized in this render copy.
/// Existing note/voice containers, navigation and source snapshots stay intact.
fn preserve_legacy_playback_map(
    master: &[u8],
    document: &roxmltree::Document,
    score: roxmltree::Node,
    offset: usize,
) -> Result<(Vec<u8>, Vec<String>), String> {
    let xml = document.input_text();
    let plan = musescore::written_playback_plan(xml)?;
    let staff = score
        .children()
        .find(|n| n.has_tag_name("Staff") && n.attribute("id") == Some(plan.staff_id.as_str()))
        .ok_or("MUSESCORE_PLAYBACK_MAP_UNPROVEN: source tempo staff is missing")?;
    if staff
        .descendants()
        .filter(|n| n.has_tag_name("TimeSig"))
        .any(|sig| {
            let value = |name| {
                sig.children()
                    .find(|n| n.has_tag_name(name))
                    .and_then(|n| n.text())
                    .map(|v| v.trim().parse::<i64>())
                    .transpose()
                    .ok()
                    .flatten()
                    .unwrap_or(1)
            };
            value("stretchN") != value("stretchD")
        })
    {
        return Err("MUSESCORE_PLAYBACK_MAP_UNPROVEN: local meter stretch cannot lower the source tempo annotations".into());
    }
    let measures: Vec<_> = staff
        .children()
        .filter(|n| n.has_tag_name("Measure"))
        .collect();
    if measures.len() != plan.measures.len() || measures.len() != plan.first_voice_ends.len() {
        return Err("MUSESCORE_PLAYBACK_MAP_UNPROVEN: source measure/cursor plan disagrees".into());
    }
    let mut edits: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    for body in score.children().filter(|n| n.has_tag_name("Staff")) {
        for node in body.descendants().filter(|n| {
            n.has_tag_name("Tempo")
                && n.ancestors().find(|a| a.has_tag_name("Score")) == Some(score)
        }) {
            let range = node.range();
            edits.push((range.start + offset, range.end + offset, Vec::new()));
        }
        for mark in body.descendants().filter(|n| {
            n.has_tag_name("Fermata")
                && n.ancestors().find(|a| a.has_tag_name("Score")) == Some(score)
        }) {
            if let Some(stretch) = mark.children().find(|n| n.has_tag_name("timeStretch")) {
                let range = stretch.range();
                edits.push((
                    range.start + offset,
                    range.end + offset,
                    b"<timeStretch>1</timeStretch>".to_vec(),
                ));
            } else if xml[mark.range()].ends_with("/>") {
                let range = mark.range();
                edits.push((
                    range.end - 2 + offset,
                    range.end + offset,
                    b"><timeStretch>1</timeStretch></Fermata>".to_vec(),
                ));
            } else {
                let at = closing_tag_offset(xml, mark)? + offset;
                edits.push((at, at, b"<timeStretch>1</timeStretch>".to_vec()));
            }
        }
    }
    let whole = 4i64 * i64::from(plan.ticks_per_beat);
    let location =
        |delta: i64| format!("<location><fractions>{delta}/{whole}</fractions></location>");
    for ((measure, &(start, end)), &cursor_end) in measures
        .iter()
        .zip(&plan.measures)
        .zip(&plan.first_voice_ends)
    {
        let points: Vec<_> = plan
            .tempos
            .iter()
            .copied()
            .filter(|(tick, _)| *tick >= start && *tick < end)
            .collect();
        if points.is_empty() {
            continue;
        }
        let voice = measure.children().find(|n| n.has_tag_name("voice")).ok_or(
            "MUSESCORE_PLAYBACK_MAP_UNPROVEN: native source has no existing annotation voice",
        )?;
        let mut cursor = i64::from(cursor_end);
        let mut annotations = String::new();
        for (tick, micros) in points {
            if cursor != i64::from(tick) {
                annotations.push_str(&location(i64::from(tick) - cursor));
            }
            annotations.push_str(&format!("<Tempo><tempo>{}</tempo><followText>0</followText><text>Source playback</text></Tempo>", 1_000_000.0 / f64::from(micros)));
            cursor = i64::from(tick);
        }
        if cursor != i64::from(cursor_end) {
            annotations.push_str(&location(i64::from(cursor_end) - cursor));
        }
        let at = closing_tag_offset(xml, voice)? + offset;
        edits.push((at, at, annotations.into_bytes()));
    }
    edits.sort_by_key(|edit| (edit.0, edit.1));
    for pair in edits.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(
                "MUSESCORE_PLAYBACK_MAP_UNPROVEN: overlapping source annotation edits".into(),
            );
        }
    }
    let mut prepared = master.to_vec();
    for (start, end, bytes) in edits.into_iter().rev() {
        prepared.splice(start..end, bytes);
    }
    if prepared.len() as u64 > MAX_MASTER_BYTES {
        return Err(too_large());
    }
    let original_music = musescore::parse(master)?;
    let prepared_music = musescore::parse(&prepared)?;
    let geometry = |midi: &crate::engine::midi::Midi| {
        midi.tracks
            .iter()
            .flat_map(|track| {
                track.events.iter().filter_map(move |event| {
                    use crate::engine::midi::Kind;
                    match &event.kind {
                        Kind::NoteOn(note) => {
                            Some((track.id.clone(), event.tick, true, note.channel, note.key))
                        }
                        Kind::NoteOff(note) => {
                            Some((track.id.clone(), event.tick, false, note.channel, note.key))
                        }
                        _ => None,
                    }
                })
            })
            .collect::<Vec<_>>()
    };
    let tempos = |midi: &crate::engine::midi::Midi| {
        midi.tracks
            .iter()
            .flat_map(|t| &t.events)
            .filter_map(|e| match e.kind {
                crate::engine::midi::Kind::Tempo(us) => Some((e.tick, us)),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>()
    };
    if original_music.ticks_per_beat != prepared_music.ticks_per_beat
        || geometry(&original_music) != geometry(&prepared_music)
        || tempos(&original_music) != tempos(&prepared_music)
    {
        return Err("MUSESCORE_PLAYBACK_MAP_UNPROVEN: renderer annotations change the source note or playback timeline".into());
    }
    Ok((prepared, vec!["[MUSESCORE_SOURCE_PLAYBACK_MAP_PRESERVED] The private reference/stem inputs use the exact native source tempo plan, including explicit fermata durations; source notes, voices and preserved bytes are unchanged.".into()]))
}

fn prepare_drum_templates(master: &[u8]) -> Result<(Vec<u8>, Vec<String>), String> {
    let offset = usize::from(master.starts_with(&[0xef, 0xbb, 0xbf])) * 3;
    let xml = std::str::from_utf8(&master[offset..]).map_err(|e| e.to_string())?;
    let document = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 5_000_000,
        },
    )
    .map_err(|e| e.to_string())?;
    let score = document
        .descendants()
        .find(|n| n.has_tag_name("Score"))
        .ok_or("SCORE_STEM_UNSILENCEABLE: MuseScore Score element not found")?;
    fn children<'a, 'input>(
        node: roxmltree::Node<'a, 'input>,
        name: &str,
    ) -> Vec<roxmltree::Node<'a, 'input>> {
        node.children().filter(|n| n.has_tag_name(name)).collect()
    }
    // Match the native note's instrument/subchannel selection. Channel zero
    // owns notes without a selector; unused variants cannot supply evidence.
    fn uses_channel(
        note: roxmltree::Node,
        instrument: roxmltree::Node,
        instrument_count: usize,
        channel_index: usize,
    ) -> bool {
        fn text<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<&'a str> {
            node.children()
                .find(|n| n.has_tag_name(name))
                .and_then(|n| n.text())
                .map(str::trim)
        }
        let subchannel = text(note, "subchannel")
            .or_else(|| note.parent().and_then(|chord| text(chord, "subchannel")));
        let selected = subchannel.and_then(|s| s.parse::<usize>().ok());
        if subchannel.is_some() && selected.is_none() {
            return false;
        }
        let references = children(note, "instrument");
        if references.is_empty() {
            return instrument_count == 1 && selected.unwrap_or(0) == channel_index;
        }
        if references.len() != 1 {
            return false;
        }
        let id = instrument
            .attribute("id")
            .or_else(|| text(instrument, "instrumentId"));
        let reference = references[0].attribute("id");
        id.is_some_and(|id| {
            (reference == Some(id) && selected.unwrap_or(0) == channel_index)
                || (subchannel.is_none()
                    && reference == Some(format!("{id}:channel:{channel_index}").as_str()))
        })
    }
    let parts = score_parts(master)?;
    let mut edits = Vec::new();
    let mut warnings = Vec::new();
    for (index, part) in score
        .children()
        .filter(|n| n.has_tag_name("Part"))
        .enumerate()
    {
        let staves = children(part, "Staff");
        let instruments = children(part, "Instrument");
        let body_staves: Vec<_> = score
            .children()
            .filter(|n| {
                n.has_tag_name("Staff")
                    && n.attribute("id")
                        .is_some_and(|id| parts[index].staff_ids.iter().any(|s| s == id))
            })
            .collect();
        let percussion_staff = staves.iter().any(|s| {
            s.descendants().any(|n| {
                n.has_tag_name("StaffType")
                    && matches!(n.attribute("group"), Some("percussion" | "unpitched"))
            })
        });
        for instrument in &instruments {
            if instrument.attribute("id") == Some("drumset") {
                continue;
            }
            let taxonomy = children(*instrument, "instrumentId");
            let channels: Vec<_> = instrument
                .children()
                .filter(|n| matches!(n.tag_name().name(), "Channel" | "channel"))
                .collect();
            let pitched_taxonomy = taxonomy.iter().any(|n| {
                let family = n
                    .text()
                    .unwrap_or("")
                    .trim()
                    .split('.')
                    .next()
                    .unwrap_or("");
                matches!(
                    family,
                    "keyboard" | "wind" | "strings" | "pluck" | "voice" | "vocal"
                )
            });
            let drum_evidence = percussion_staff
                || taxonomy
                    .iter()
                    .any(|n| crate::engine::midi::instrument_taxonomy_is_percussion(n.text()))
                || instrument.descendants().any(|n| {
                    n.has_tag_name("Drum")
                        || (n.has_tag_name("useDrumset")
                            && n.text().is_some_and(|t| t.trim() == "1"))
                })
                || channels.iter().enumerate().any(|(channel_index, channel)| {
                    let percussion = channel
                        .attribute("channel")
                        .into_iter()
                        .chain(
                            channel
                                .children()
                                .filter(|n| {
                                    matches!(n.tag_name().name(), "midiChannel" | "channel")
                                })
                                .map(|n| n.text().unwrap_or("")),
                        )
                        .any(|v| v.trim().parse::<i32>().ok() == Some(9));
                    percussion
                        && body_staves.iter().any(|staff| {
                            staff
                                .descendants()
                                .filter(|n| n.has_tag_name("Note") && musescore::plays(*n))
                                .any(|note| {
                                    uses_channel(
                                        note,
                                        *instrument,
                                        instruments.len(),
                                        channel_index,
                                    )
                                })
                        })
                });
            if !drum_evidence || !(instrument.attribute("id") == Some("piano") || pitched_taxonomy)
            {
                continue;
            }
            let refuse = |reason: &str| {
                format!(
                "MUSESCORE_DRUM_TEMPLATE_UNPROVEN: Part {} has conflicting percussion/template evidence ({reason}); no replacement kit is inferred", index + 1)
            };
            let version = document.root_element().attribute("version").unwrap_or("");
            if !matches!(version.split('.').next(), Some("2" | "3" | "4"))
                || !document.root_element().has_tag_name("museScore")
                || score.parent() != Some(document.root_element())
                || instruments.len() != 1
                || staves.len() != 1
                || instrument.attribute("id") != Some("piano")
                || taxonomy.len() > 1
                || taxonomy.first().is_some_and(|n| {
                    !matches!(
                        n.text().map(str::trim),
                        Some("keyboard.piano" | "drum.group.set")
                    )
                })
                || children(*instrument, "useDrumset").len() != 1
                || children(*instrument, "useDrumset")[0].text().map(str::trim) != Some("1")
                || children(staves[0], "StaffType").len() != 1
                || children(staves[0], "StaffType")[0].attribute("group") != Some("percussion")
                || instrument
                    .descendants()
                    .any(|n| n.has_tag_name("soundId") && !n.text().unwrap_or("").trim().is_empty())
                || body_staves.iter().any(|s| {
                    s.descendants().any(|n| {
                        matches!(
                            n.tag_name().name(),
                            "InstrumentChange" | "StaffTypeChange" | "channelSwitch"
                        )
                    })
                })
            {
                return Err(refuse(
                    "unsupported template, staff, sound ID or ownership layout",
                ));
            }
            if channels.len() != 1 {
                return Err(refuse("exactly one Standard-kit channel is required"));
            }
            let channel = channels[0];
            let channel_values: Vec<_> = channel
                .attribute("channel")
                .into_iter()
                .chain(
                    channel
                        .children()
                        .filter(|n| matches!(n.tag_name().name(), "midiChannel" | "channel"))
                        .map(|n| n.text().unwrap_or("")),
                )
                .collect();
            let programs = children(channel, "program");
            let program_values: Vec<_> = programs
                .iter()
                .flat_map(|n| {
                    let attribute = n.attribute("value");
                    let text = n.text().filter(|t| !t.trim().is_empty());
                    // Native -1 selects the text fallback, as in musescore.rs.
                    attribute
                        .filter(|value| !(value.trim() == "-1" && text.is_some()))
                        .into_iter()
                        .chain(text)
                })
                .collect();
            let controllers = children(channel, "controller");
            let bank = |number: &str, expected: &str| {
                let values: Vec<_> = controllers
                    .iter()
                    .filter(|n| n.attribute("ctrl") == Some(number))
                    .collect();
                values.len() == 1 && values[0].attribute("value") == Some(expected)
            };
            if channel_values.is_empty()
                || channel_values.iter().any(|v| v.trim() != "9")
                || programs.len() != 1
                || program_values.is_empty()
                || program_values.iter().any(|v| v.trim() != "0")
                || !bank("0", "1")
                || !bank("32", "0")
            {
                return Err(refuse(
                    "channel 9, bank 128 (MSB 1/LSB 0) and program 0 must be explicit",
                ));
            }
            let drums = children(*instrument, "Drum");
            let mut pitches = std::collections::BTreeSet::new();
            if drums.is_empty()
                || drums.iter().any(|n| {
                    n.attribute("pitch")
                        .and_then(|s| s.parse::<u8>().ok())
                        .is_none_or(|pitch| !(35..=81).contains(&pitch) || !pitches.insert(pitch))
                })
            {
                return Err(refuse("an explicit, unique Standard drum map is required"));
            }
            // Body IDs use the same resolution as stem topology (including 4.x
            // implicit declaration IDs); every played key must be in this map.
            for staff in &body_staves {
                if staff
                    .descendants()
                    .filter(|n| n.has_tag_name("Note") && musescore::plays(*n))
                    .any(|n| {
                        let keys = children(n, "pitch");
                        keys.len() != 1
                            || keys[0]
                                .text()
                                .and_then(|s| s.trim().parse::<u8>().ok())
                                .is_none_or(|p| !pitches.contains(&p))
                    })
                {
                    return Err(refuse("a played key is outside the source drum map"));
                }
            }
            let range = instrument.attribute_node("id").unwrap().range_value();
            edits.push((range.start + offset, range.end + offset));
            warnings.push(format!(
                "[MUSESCORE_DRUM_TEMPLATE_MAPPED] Part {} ({}) uses source-proven Standard drums: the private MuseScore 4 reference/stem inputs map template piano to drumset; source bytes, drum map and MIDI state are unchanged.",
                index + 1, parts[index].id));
        }
    }
    let mut prepared = master.to_vec();
    for (start, end) in edits.into_iter().rev() {
        prepared.splice(start..end, b"drumset".iter().copied());
    }
    if prepared.len() as u64 > MAX_MASTER_BYTES {
        return Err(too_large());
    }
    Ok((prepared, warnings))
}

fn replace_entry(archive: &[u8], path: &str, master: &[u8]) -> Result<Vec<u8>, String> {
    let mut source = zip::ZipArchive::new(Cursor::new(archive)).map_err(|e| e.to_string())?;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.set_raw_comment(source.comment().into());
    for index in 0..source.len() {
        let entry = source.by_index_raw(index).map_err(|e| e.to_string())?;
        if entry.name() == path {
            let mut options = zip::write::SimpleFileOptions::default()
                .compression_method(entry.compression())
                .last_modified_time(entry.last_modified().unwrap_or_default());
            if let Some(mode) = entry.unix_mode() {
                options = options.unix_permissions(mode);
            }
            let name = entry.name().to_string();
            drop(entry);
            writer
                .start_file(name, options)
                .map_err(|e| e.to_string())?;
            writer.write_all(master).map_err(|e| e.to_string())?;
        } else {
            writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
        }
    }
    Ok(writer.finish().map_err(|e| e.to_string())?.into_inner())
}

fn score_parts(master: &[u8]) -> Result<Vec<ScorePart>, String> {
    let (offset, text) = match master.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        Some(rest) => (3, rest),
        None => (0, master),
    };
    let xml = std::str::from_utf8(text).map_err(|_| {
        "SCORE_STEM_UNSILENCEABLE: the MuseScore score is not UTF-8 and cannot be silenced in place"
    })?;
    crate::engine::musicxml::check_nesting(xml)
        .map_err(|error| format!("SCORE_STEM_UNSILENCEABLE: {error}"))?;
    let document = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 5_000_000,
        },
    )
    .map_err(|error| format!("SCORE_STEM_UNSILENCEABLE: invalid XML: {error}"))?;
    let score = document
        .descendants()
        .find(|node| node.has_tag_name("Score"))
        .ok_or("SCORE_STEM_UNSILENCEABLE: MuseScore Score element not found")?;
    let body_ids: Vec<&str> = score
        .children()
        .filter(|node| node.has_tag_name("Staff"))
        .filter_map(|node| node.attribute("id"))
        .collect();
    let mut parts = Vec::new();
    let mut owners = BTreeMap::new();
    let mut staff_cursor = 0;
    for (part_index, part) in score
        .children()
        .filter(|node| node.has_tag_name("Part"))
        .enumerate()
    {
        let mut staff_ids = Vec::new();
        for (staff_index, staff) in part
            .children()
            .filter(|node| node.has_tag_name("Staff"))
            .enumerate()
        {
            let id = musescore::declaration_staff_id(
                part,
                staff,
                part_index,
                staff_index,
                staff_cursor,
                &body_ids,
            );
            staff_cursor += 1;
            if owners.insert(id.clone(), part_index).is_some() {
                return Err(format!(
                    "SCORE_STEM_STAFF_UNOWNED: MuseScore duplicate resolved staff declaration ID: {id:?}"
                ));
            }
            staff_ids.push(id);
        }
        parts.push(ScorePart {
            id: part
                .attribute("id")
                .map(|value| format!("musescore-part-{value}"))
                .unwrap_or_else(|| format!("musescore-part-{part_index}")),
            staff_ids,
            track_name: part
                .children()
                .find(|node| node.has_tag_name("trackName"))
                .map(|node| node.text().unwrap_or("").to_string()),
            audible: Vec::new(),
        });
    }
    for staff in score.children().filter(|node| node.has_tag_name("Staff")) {
        let audible = staff
            .descendants()
            .filter(|node| {
                (node.has_tag_name("Note") || node.has_tag_name("Harmony"))
                    && musescore::plays(*node)
            })
            .map(|node| closing_tag_offset(xml, node).map(|at| at + offset))
            .collect::<Result<Vec<_>, _>>()?;
        if audible.is_empty() {
            continue;
        }
        let owner = staff
            .attribute("id")
            .and_then(|id| owners.get(id))
            .ok_or_else(|| {
                format!(
                    "SCORE_STEM_STAFF_UNOWNED: MuseScore staff {:?} cannot be mapped to exactly one source Part",
                    staff.attribute("id").unwrap_or("")
                )
            })?;
        parts[*owner].audible.extend(audible);
    }
    Ok(parts)
}

/// Where `</Note>` or `</Harmony>` starts, so the inserted property is read
/// after any the element already states.
fn closing_tag_offset(xml: &str, node: roxmltree::Node) -> Result<usize, String> {
    let range = node.range();
    let element = &xml[range.clone()];
    let name = node.tag_name().name();
    let close = element
        .rfind("</")
        .filter(|at| {
            element[at + 2..]
                .split_once(':')
                .map_or(&element[at + 2..], |(_, local)| local)
                .strip_prefix(name)
                .is_some_and(|rest| rest.trim_start() == ">")
        })
        .ok_or_else(|| {
            format!(
                "SCORE_STEM_UNSILENCEABLE: MuseScore {name} element cannot be silenced in place"
            )
        })?;
    Ok(range.start + close)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRUM_CONFLICT: &str = include_str!("../tests/support/score-audio-conflict.mscx");

    #[test]
    fn legacy_fermata_defaults_are_explicit_in_every_render_copy_only() {
        for bom in [false, true] {
            for zip in [false, true] {
                let xml = score("3.02", PARTS, &staves().replace("<Fermata/>", "")).replacen(
                    "<Chord>",
                    "<Fermata><subtype>fermataAbove</subtype></Fermata><Chord>",
                    1,
                );
                let master = [
                    if bom { &[0xef, 0xbb, 0xbf][..] } else { &[] },
                    xml.as_bytes(),
                ]
                .concat();
                let original = if zip {
                    zipped(&[
                        ("score.mscx", &master),
                        ("retained.txt", b"original companion"),
                    ])
                } else {
                    master.clone()
                };
                let mut stems = ScoreStems::read(&original).unwrap();
                assert_eq!(stems.render_container(), original);
                let warnings = stems.prepare_for_renderer(4).unwrap();
                assert_eq!(warnings.len(), 1);
                assert!(warnings[0].contains("MUSESCORE_LEGACY_FERMATA_DEFAULT_PRESERVED"));
                assert!(stems.has_render_preparation());
                let prepared = if zip {
                    read_entry(stems.render_container(), "score.mscx").unwrap()
                } else {
                    stems.render_container().to_vec()
                };
                assert_eq!(
                    String::from_utf8(prepared[offset_for_bom(bom)..].to_vec())
                        .unwrap()
                        .replace("<timeStretch>1</timeStretch><play>0</play>", ""),
                    xml
                );
                for part in 0..stems.parts().len() {
                    let copy = stems.silenced(part).unwrap();
                    let copy = if zip {
                        read_entry(&copy, "score.mscx").unwrap()
                    } else {
                        copy
                    };
                    assert!(String::from_utf8_lossy(&copy).contains("<timeStretch>1</timeStretch>"));
                }
                if zip {
                    assert_eq!(
                        read_entry(stems.render_container(), "retained.txt").unwrap(),
                        b"original companion"
                    );
                }
                let once = stems.render_container().to_vec();
                assert!(stems.prepare_for_renderer(4).unwrap().is_empty());
                assert_eq!(stems.render_container(), once);
                assert_eq!(
                    original,
                    if zip {
                        zipped(&[
                            ("score.mscx", &master),
                            ("retained.txt", b"original companion"),
                        ])
                    } else {
                        master
                    }
                );
            }
        }
    }

    fn offset_for_bom(bom: bool) -> usize {
        usize::from(bom) * 3
    }

    #[test]
    fn authored_fermata_stretch_is_kept_and_implicit_modern_playback_is_neutral() {
        for version in ["4.0"] {
            for property in [
                "<timeStretch>1.5</timeStretch>",
                "<timeStretch>1</timeStretch>",
                "<timeStretch>0</timeStretch>",
            ] {
                let xml = score(version, PARTS, &staves().replace("<Fermata/>", "")).replacen(
                    "<Chord>",
                    &format!("<Fermata><subtype>fermataAbove</subtype>{property}</Fermata><Chord>"),
                    1,
                );
                let mut stems = ScoreStems::read(xml.as_bytes()).unwrap();
                assert!(stems.prepare_for_renderer(4).unwrap().is_empty());
                assert_eq!(stems.render_container(), xml.as_bytes());
            }
        }
        let xml = score("4.0", PARTS, &staves().replace("<Fermata/>", "")).replacen(
            "<Chord>",
            "<Fermata><subtype>fermataAbove</subtype></Fermata><Chord>",
            1,
        );
        let mut stems = ScoreStems::read(xml.as_bytes()).unwrap();
        let warnings = stems.prepare_for_renderer(4).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("MUSESCORE_IMPLICIT_FERMATA_TIMING_PRESERVED"));
        assert!(std::str::from_utf8(stems.render_container())
            .unwrap()
            .contains("<timeStretch>1</timeStretch>"));
        assert_eq!(
            stems.render_container(),
            xml.replace(
                "</Fermata>",
                "<timeStretch>1</timeStretch><play>0</play></Fermata>"
            )
            .as_bytes()
        );
    }

    #[test]
    fn undefined_playback_is_disabled_without_suppressing_an_explicit_speedup() {
        for version in ["3.02", "4.70"] {
            for implicit in [
                "<Fermata><subtype>fermataAbove</subtype><play>1</play></Fermata>",
                "<Fermata><subtype>fermataAbove</subtype><play comment=\">\"> 1 </play></Fermata>",
                "<m:Fermata xmlns:m=\"urn:test\"/>",
            ] {
                // The prefixed empty legacy symbol has no qualified native4 glyph.
                let implicit = if version == "4.70" && implicit.contains("m:Fermata") {
                    "<m:Fermata xmlns:m=\"urn:test\"><subtype>fermataAbove</subtype></m:Fermata>"
                } else {
                    implicit
                };
                let xml = format!("<museScore version=\"{version}\"><Score><Division>480</Division><Part><Staff id=\"1\"/><Instrument><instrumentId>voice.soprano</instrumentId></Instrument></Part><Staff id=\"1\"><Measure len=\"1/4\"><voice><Tempo><tempo>2</tempo></Tempo><Fermata><subtype>fermataAbove</subtype><timeStretch>0.5</timeStretch></Fermata>{implicit}<Chord><durationType>quarter</durationType><Lyrics><text>one</text></Lyrics><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff></Score></museScore>");
                let original = musescore::parse(xml.as_bytes()).unwrap();
                let mut stems = ScoreStems::read(xml.as_bytes()).unwrap();
                let warnings = stems.prepare_for_renderer(4).unwrap();
                assert_eq!(warnings.len(), 1);
                assert!(
                    warnings[0].contains("1 "),
                    "one implicit symbol, not edit count"
                );
                let prepared = std::str::from_utf8(stems.render_container()).unwrap();
                assert!(prepared.contains("<timeStretch>0.5</timeStretch>"));
                let doc = roxmltree::Document::parse(prepared).unwrap();
                let marks: Vec<_> = doc
                    .descendants()
                    .filter(|n| n.tag_name().name() == "Fermata")
                    .collect();
                assert_eq!(marks.len(), 2);
                assert_eq!(
                    marks[1]
                        .children()
                        .find(|n| n.tag_name().name() == "play")
                        .unwrap()
                        .text(),
                    Some("0")
                );
                let lowered = musescore::parse(prepared.as_bytes()).unwrap();
                assert_eq!(original.tracks, lowered.tracks);
                assert_eq!(
                    ScoreStems::read(xml.as_bytes()).unwrap().render_container(),
                    xml.as_bytes()
                );
            }
        }
    }

    #[test]
    fn master_only_timing_preparation_keeps_embedded_excerpts_byte_exact() {
        let excerpt="<Score><Staff id=\"99\"><Measure><voice><Fermata><subtype>fermataAbove</subtype><play>invalid</play></Fermata></voice></Measure></Staff></Score>";
        let xml = score(
            "4.70",
            PARTS,
            &staves().replace(
                "<Fermata/>",
                "<Fermata><subtype>fermataAbove</subtype></Fermata>",
            ),
        )
        .replace("</Score>", &format!("{excerpt}</Score>"));
        let mut stems = ScoreStems::read(xml.as_bytes()).unwrap();
        stems.prepare_for_renderer(4).unwrap();
        let prepared = std::str::from_utf8(stems.render_container()).unwrap();
        assert!(prepared.contains(excerpt));
        assert!(prepared.contains("<timeStretch>1</timeStretch><play>0</play>"));
    }

    #[test]
    fn both_renderer_versions_preserve_another_parts_explicit_speedup() {
        let xml="<museScore version=\"3.02\"><Score><Division>480</Division><Part><Staff id=\"1\"/><Instrument><instrumentId>voice.soprano</instrumentId></Instrument></Part><Part><Staff id=\"2\"/><Instrument><instrumentId>keyboard.piano</instrumentId></Instrument></Part><Staff id=\"1\"><Measure len=\"1/4\"><voice><Tempo><tempo>2</tempo></Tempo><Fermata><timeStretch>0.5</timeStretch></Fermata><Chord><durationType>quarter</durationType><Lyrics><text>one</text></Lyrics><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff><Staff id=\"2\"><Measure len=\"1/4\"><voice><Fermata/><Chord><durationType>quarter</durationType><Note><pitch>48</pitch></Note></Chord></voice></Measure></Staff></Score></museScore>";
        for major in [3, 4] {
            let mut stems = ScoreStems::read(xml.as_bytes()).unwrap();
            stems.prepare_for_renderer(major).unwrap();
            let prepared = std::str::from_utf8(stems.render_container()).unwrap();
            assert!(prepared.contains("<timeStretch>0.5</timeStretch>"));
            assert!(prepared.contains("<timeStretch>1</timeStretch><play>0</play>"));
            let midi = musescore::parse(prepared.as_bytes()).unwrap();
            let tempos: BTreeMap<_, _> = midi
                .tracks
                .iter()
                .flat_map(|t| &t.events)
                .filter_map(|e| {
                    if let crate::engine::midi::Kind::Tempo(v) = e.kind {
                        Some((e.tick, v))
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(
                tempos.into_iter().collect::<Vec<_>>(),
                [(0, 250_000), (479, 500_000)]
            );
        }
    }

    #[test]
    fn explicit_legacy_hold_lowers_exact_source_clock_without_new_notes_or_voices() {
        let xml = "<museScore version=\"3.02\"><Score><Division>480</Division><Part><Staff id=\"1\"/><Instrument><instrumentId>voice.soprano</instrumentId></Instrument></Part><Part><Staff id=\"2\"/><Instrument><instrumentId>keyboard.piano</instrumentId></Instrument></Part><Staff id=\"1\"><Measure><voice><Tempo><tempo>2</tempo></Tempo><Fermata><subtype>fermataAbove</subtype><timeStretch>1.5</timeStretch></Fermata><Chord><durationType>whole</durationType><Lyrics><text>one</text></Lyrics><Note><pitch>72</pitch></Note></Chord></voice></Measure></Staff><Staff id=\"2\"><Measure><voice><Chord><durationType>quarter</durationType><dots>1</dots><Note><pitch>60</pitch></Note></Chord><Rest><durationType>eighth</durationType></Rest><Rest><durationType>half</durationType></Rest></voice><voice><location><fractions>3/8</fractions></location><Chord><durationType>half</durationType><Note><pitch>48</pitch></Note></Chord></voice></Measure></Staff></Score></museScore>";
        let mut stems = ScoreStems::read(xml.as_bytes()).unwrap();
        let warnings = stems.prepare_for_renderer(4).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("MUSESCORE_SOURCE_PLAYBACK_MAP_PRESERVED"));
        let prepared = std::str::from_utf8(stems.render_container()).unwrap();
        let before = roxmltree::Document::parse(xml).unwrap();
        let after = roxmltree::Document::parse(prepared).unwrap();
        for name in ["Chord", "Rest", "Lyrics", "Note"] {
            let content = |doc: &roxmltree::Document| {
                doc.descendants()
                    .filter(|n| n.has_tag_name(name))
                    .map(|n| doc.input_text()[n.range()].to_owned())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                content(&before),
                content(&after),
                "source {name} XML changed"
            );
        }
        assert_eq!(
            before
                .descendants()
                .filter(|n| n.has_tag_name("voice"))
                .count(),
            after
                .descendants()
                .filter(|n| n.has_tag_name("voice"))
                .count()
        );
        let tempos = |bytes: &[u8]| {
            let parsed = musescore::parse(bytes).unwrap();
            parsed
                .tracks
                .iter()
                .flat_map(|t| &t.events)
                .filter_map(|e| match e.kind {
                    crate::engine::midi::Kind::Tempo(us) => Some((e.tick, us)),
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(
            tempos(xml.as_bytes()),
            BTreeMap::from([(0, 750_000), (719, 500_000)])
        );
        assert_eq!(tempos(stems.render_container()), tempos(xml.as_bytes()));
        assert!(prepared.contains("<timeStretch>1</timeStretch>"));
        let once = stems.render_container().to_vec();
        assert!(stems.prepare_for_renderer(4).unwrap().is_empty());
        assert_eq!(stems.render_container(), once);
    }

    #[test]
    fn legacy_default_preparation_handles_self_closing_marks_and_utf8() {
        let xml = score("3.02", PARTS, &staves().replace("<Fermata/>", "")).replacen(
            "<Chord>",
            "<StaffText><text>Été</text></StaffText><Fermata/><Chord>",
            1,
        );
        let (prepared, warnings) = preserve_legacy_fermata_defaults(xml.as_bytes()).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            String::from_utf8(prepared).unwrap(),
            xml.replace(
                "<Fermata/>",
                "<Fermata><timeStretch>1</timeStretch><play>0</play></Fermata>"
            )
        );
    }

    #[test]
    fn qualified_annotation_edits_accept_namespace_prefixes_and_identity_ratios() {
        let xml = "<museScore version=\"3.02\"><Score><Division>480</Division><Part><Staff id=\"1\"/><Instrument><instrumentId>voice.soprano</instrumentId></Instrument></Part><Staff id=\"1\"><Measure><m:voice xmlns:m=\"urn:test\"><TimeSig><sigN>4</sigN><sigD>4</sigD><stretchN>2</stretchN><stretchD>2</stretchD></TimeSig><Tempo><tempo>2</tempo></Tempo><Fermata><subtype>fermataAbove</subtype><timeStretch>1.5</timeStretch></Fermata><Chord><durationType>whole</durationType><Lyrics><text>one</text></Lyrics><Note><pitch>72</pitch></Note></Chord></m:voice><voice><location><fractions>3/8</fractions></location><Chord><durationType>half</durationType><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff></Score></museScore>";
        let mut source = ScoreStems::read(xml.as_bytes()).unwrap();
        assert_eq!(source.prepare_for_renderer(4).unwrap().len(), 1);
        assert!(source.has_render_preparation());
        let prepared = std::str::from_utf8(source.render_container()).unwrap();
        assert!(prepared.contains("</m:voice>"));
        assert!(prepared.contains("<stretchN>2</stretchN><stretchD>2</stretchD>"));
        assert!(prepared.contains("<timeStretch>1</timeStretch>"));
    }

    #[test]
    fn dense_legacy_holds_keep_native_relative_playback_across_tempo_repeats() {
        let xml = "<museScore version=\"3.02\"><Score><Division>480</Division><Part><Staff id=\"1\"/><Instrument><instrumentId>voice.soprano</instrumentId></Instrument></Part><Staff id=\"1\"><Measure len=\"1/4\"><voice><Tempo><tempo>2</tempo></Tempo><Chord><durationType>quarter</durationType><Lyrics><text>one</text></Lyrics><Note><pitch>60</pitch></Note></Chord></voice></Measure><Measure len=\"1/4\"><startRepeat/><voice><Fermata><subtype>fermataAbove</subtype><timeStretch>2</timeStretch></Fermata><Chord><durationType>quarter</durationType><Lyrics><text>two</text></Lyrics><Note><pitch>62</pitch></Note></Chord></voice></Measure><Measure len=\"1/4\"><voice><Tempo><tempo>4</tempo></Tempo><Chord><durationType>quarter</durationType><Lyrics><text>three</text></Lyrics><Note><pitch>64</pitch></Note></Chord></voice><endRepeat>2</endRepeat></Measure></Staff></Score></museScore>";
        let mut source = ScoreStems::read(xml.as_bytes()).unwrap();
        assert!(source.prepare_for_renderer(4).unwrap().is_empty());
        assert_eq!(source.render_container(), xml.as_bytes());
        assert!(!source.has_render_preparation());
    }

    #[test]
    #[ignore = "requires a private legacy score and a real MuseScore 4 renderer"]
    fn supplied_legacy_fermata_render_preserves_the_vocal_tempo_map() {
        use crate::engine::{
            convert::convert_midi,
            midi::{self, Kind},
        };
        use crate::renderer::{AudioRenderer, MuseScoreRenderer, RenderLimits};
        use std::{fs, path::PathBuf, process::Command, time::Duration};
        let executable = std::env::var("VERSE_MUSESCORE_GATE").expect("real renderer required");
        let source =
            PathBuf::from(std::env::var("VERSE_FERMATA_SOURCE").expect("private source required"));
        let root = PathBuf::from(
            std::env::var("VERSE_FERMATA_OUTPUT_DIR").expect("new output directory required"),
        );
        fs::create_dir(&root).expect("output directory must not exist");
        let original = fs::read(&source).unwrap();
        let native = musescore::parse(&original).unwrap();
        let projection = convert_midi(&native, "english").svp.unwrap();
        let renderer = MuseScoreRenderer::probe(std::path::Path::new(&executable)).unwrap();
        assert_eq!(renderer.capabilities().identity.major, 4);
        let mut stems = ScoreStems::read(&original).unwrap();
        let warnings = stems.prepare_for_renderer(4).unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("MUSESCORE_LEGACY_FERMATA_DEFAULT_PRESERVED")
                || w.contains("MUSESCORE_SOURCE_PLAYBACK_MAP_PRESERVED")));
        let prepared = root.join(format!("prepared.{}", stems.extension()));
        fs::write(&prepared, stems.render_container()).unwrap();
        let expected: Vec<_> = projection
            .tempos
            .iter()
            .map(|t| (t.tick, (60_000_000.0 / t.bpm).round() as u32))
            .collect();
        let mut midi_inputs = vec![prepared.clone()];
        for part in 0..stems.parts().len() {
            let input = root.join(format!("stem-{part}.{}", stems.extension()));
            fs::write(&input, stems.silenced(part).unwrap()).unwrap();
            midi_inputs.push(input);
        }
        // Native converter test mode and one Basic-profile job avoid the
        // macOS MIDI-only shutdown crash while using the same score reader and
        // playback tempo exporter. This is oracle tooling, not WAV rendering.
        let mut successful = None;
        for attempt in 0..3 {
            if cfg!(target_os = "macos") {
                std::thread::sleep(Duration::from_secs(10));
            }
            let references: Vec<_> = midi_inputs
                .iter()
                .enumerate()
                .map(|(index, _)| root.join(format!("independent-renderer-{index}-{attempt}.mid")))
                .collect();
            let jobs: Vec<_> = midi_inputs
                .iter()
                .zip(&references)
                .map(|(input, output)| serde_json::json!({"in": input, "out": output}))
                .collect();
            let job = root.join(format!("midi-job-{attempt}.json"));
            fs::write(&job, serde_json::to_vec(&jobs).unwrap()).unwrap();
            let home = root.join(format!("midi-home-{attempt}"));
            fs::create_dir(&home).unwrap();
            let output = Command::new(&executable)
                .args(["-F", "-t", "--sound-profile", "MuseScore Basic", "-j"])
                .arg(&job)
                .env_clear()
                .env("HOME", &home)
                .env("TMPDIR", std::env::temp_dir())
                .output()
                .unwrap();
            if output.status.success() {
                successful = Some(references);
                break;
            }
            fs::write(
                root.join(format!("midi-error-{attempt}.log")),
                &output.stderr,
            )
            .unwrap();
        }
        let references = successful.expect("independent MIDI job failed after three isolated attempts; retained logs identify the renderer failure");
        for (index, reference) in references.iter().enumerate() {
            let independent = midi::parse(&fs::read(reference).unwrap()).unwrap();
            let mut independent_tempos = BTreeMap::new();
            for track in &independent.tracks {
                for event in &track.events {
                    if let Kind::Tempo(us) = event.kind {
                        independent_tempos.insert(event.tick, us);
                    }
                }
            }
            assert_eq!(independent.ticks_per_beat, projection.ticks_per_beat);
            assert_eq!(
                independent_tempos.into_iter().collect::<Vec<_>>(),
                expected,
                "reference/stem {index} must not add a pause missing from the vocal timeline"
            );
        }
        let rendered = renderer
            .render(
                &prepared,
                &root.join("reference.wav"),
                &RenderLimits {
                    timeout: Duration::from_secs(300),
                    max_output_bytes: 2 * 1024 * 1024 * 1024,
                },
            )
            .unwrap();
        assert_eq!(fs::read(&source).unwrap(), original);
        let receipt = serde_json::json!({"source": source, "sourceBytesPreserved": true, "temposMatch": true, "independentlyCheckedMidiInputs": midi_inputs.len(), "vocalNoteCount": projection.tracks.iter().map(|t| t.notes.len()).sum::<usize>(), "renderer": renderer.capabilities().identity, "warnings": warnings, "wav": rendered.wav});
        fs::write(
            root.join("verification.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn drum_template_mapping_changes_only_one_template_in_bounded_render_copies() {
        for bom in [false, true] {
            let original = [
                if bom { &[0xef, 0xbb, 0xbf][..] } else { &[] },
                DRUM_CONFLICT.as_bytes(),
            ]
            .concat();
            let mut score = ScoreStems::read(&original).unwrap();
            let topology = musescore::parse(&original).unwrap().topology;
            let before = score.parts().to_vec();
            assert!(score.prepare_for_renderer(3).unwrap().is_empty());
            assert_eq!(score.render_container(), original);
            let warnings = score.prepare_for_renderer(4).unwrap();
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].starts_with("[MUSESCORE_DRUM_TEMPLATE_MAPPED]"));
            assert_eq!(
                String::from_utf8_lossy(score.render_container())
                    .replace("id=\"drumset\"", "id=\"piano\""),
                String::from_utf8_lossy(&original)
            );
            score.validate_topology(&topology, true).unwrap();
            assert_eq!(
                score
                    .parts()
                    .iter()
                    .map(|p| (&p.id, &p.staff_ids, p.audible_elements()))
                    .collect::<Vec<_>>(),
                before
                    .iter()
                    .map(|p| (&p.id, &p.staff_ids, p.audible_elements()))
                    .collect::<Vec<_>>()
            );
            assert!(score.prepare_for_renderer(4).unwrap().is_empty());
            for keep in 0..3 {
                let stem = score.silenced(keep).unwrap();
                assert!(String::from_utf8_lossy(&stem).contains("id=\"drumset\""));
                let isolated = ScoreStems::read(&stem).unwrap();
                for (index, part) in isolated.parts().iter().enumerate() {
                    assert_eq!(part.audible_elements() > 0, index == keep);
                }
            }
        }
    }

    #[test]
    fn drum_template_ownership_changes_are_scoped_to_resolved_part_staves() {
        for version in ["3.02", "4.20"] {
            let fixture =
                DRUM_CONFLICT.replace("version=\"3.02\"", &format!("version=\"{version}\""));
            let fixture = if version.starts_with('4') {
                fixture
                    .replace("<Staff id=\"1\"><StaffType", "<Staff><StaffType")
                    .replace("<Staff id=\"2\"><StaffType", "<Staff><StaffType")
                    .replace("<Staff id=\"3\"><StaffType", "<Staff><StaffType")
            } else {
                fixture
            };
            for change in [
                "<InstrumentChange><Instrument id=\"flute\"><instrumentId>wind.flutes.flute</instrumentId></Instrument></InstrumentChange>",
                "<StaffTypeChange><StaffType group=\"pitched\"/></StaffTypeChange>",
                "<channelSwitch voice=\"0\" name=\"open\"/>",
            ] {
                let unrelated = fixture.replace("<Staff id=\"1\"><Measure><voice>", &format!("<Staff id=\"1\"><Measure><voice>{change}"));
                let topology = musescore::parse(unrelated.as_bytes()).unwrap().topology;
                let mut score = ScoreStems::read(unrelated.as_bytes()).unwrap();
                assert_eq!(score.prepare_for_renderer(4).unwrap().len(), 1, "{version}: {change}");
                assert_eq!(String::from_utf8_lossy(score.render_container()).replace("id=\"drumset\"", "id=\"piano\""), unrelated);
                score.validate_topology(&topology, true).unwrap();

                let owned = fixture.replace("<Staff id=\"3\"><Measure><voice>", &format!("<Staff id=\"3\"><Measure><voice>{change}"));
                let mut score = ScoreStems::read(owned.as_bytes()).unwrap();
                assert!(score.prepare_for_renderer(4).unwrap_err().starts_with("MUSESCORE_DRUM_TEMPLATE_UNPROVEN:"), "{version}: {change}");
                assert_eq!(score.render_container(), owned.as_bytes());
            }
        }
    }

    #[test]
    fn drum_template_program_sentinel_uses_native_text_fallback() {
        let source = DRUM_CONFLICT.replace(
            "<program value=\"0\"/>",
            "<program value=\"-1\">0</program>",
        );
        musescore::parse(source.as_bytes()).unwrap();
        let mut score = ScoreStems::read(source.as_bytes()).unwrap();
        assert_eq!(score.prepare_for_renderer(4).unwrap().len(), 1);
        assert_eq!(
            String::from_utf8_lossy(score.render_container())
                .replace("id=\"drumset\"", "id=\"piano\""),
            source
        );
        for program in [
            "<program value=\"0\">8</program>",
            "<program value=\"8\">0</program>",
            "<program value=\"-1\">8</program>",
            "<program value=\"-1\"/>",
        ] {
            let source = DRUM_CONFLICT.replace("<program value=\"0\"/>", program);
            let mut score = ScoreStems::read(source.as_bytes()).unwrap();
            assert!(
                score
                    .prepare_for_renderer(4)
                    .unwrap_err()
                    .starts_with("MUSESCORE_DRUM_TEMPLATE_UNPROVEN:"),
                "{program}"
            );
            assert_eq!(score.render_container(), source.as_bytes());
        }
    }

    #[test]
    fn drum_template_recognizes_alternate_percussion_evidence_and_channel_spellings() {
        for channel in [
            "<Channel channel=\"9\"><program value=\"0\"/></Channel>",
            "<Channel><channel>9</channel><program value=\"0\"/></Channel>",
            "<channel><midiChannel>9</midiChannel><program value=\"0\"/></channel>",
        ] {
            let source = score("3.02", &format!("<Part><Staff id=\"1\"/><Instrument id=\"piano\"><instrumentId>keyboard.piano</instrumentId>{channel}</Instrument></Part>"), "<Staff id=\"1\"><Measure><voice><Chord><durationType>quarter</durationType><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff>");
            let mut score = ScoreStems::read(source.as_bytes()).unwrap();
            assert!(
                score
                    .prepare_for_renderer(4)
                    .unwrap_err()
                    .starts_with("MUSESCORE_DRUM_TEMPLATE_UNPROVEN:"),
                "{channel}"
            );
            assert_eq!(score.render_container(), source.as_bytes());
        }
        for taxonomy in [
            "drum.group",
            "percussion",
            "pitched-percussion",
            "unpitched-percussion",
        ] {
            let source = score("3.02", &format!("<Part><Staff id=\"1\"/><Instrument id=\"piano\"><instrumentId>{taxonomy}</instrumentId></Instrument></Part>"), "<Staff id=\"1\"><Measure><voice><Chord><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff>");
            let mut score = ScoreStems::read(source.as_bytes()).unwrap();
            assert!(
                score
                    .prepare_for_renderer(4)
                    .unwrap_err()
                    .starts_with("MUSESCORE_DRUM_TEMPLATE_UNPROVEN:"),
                "{taxonomy}"
            );
            assert_eq!(score.render_container(), source.as_bytes());
        }
        for extra in [
            "<channel/>",
            "<channel channel=\"9\"><program value=\"8\"/></channel>",
        ] {
            let source = DRUM_CONFLICT.replace(
                "<useDrumset>1</useDrumset>",
                &format!("<useDrumset>1</useDrumset>{extra}"),
            );
            let mut score = ScoreStems::read(source.as_bytes()).unwrap();
            assert!(
                score
                    .prepare_for_renderer(4)
                    .unwrap_err()
                    .starts_with("MUSESCORE_DRUM_TEMPLATE_UNPROVEN:"),
                "{extra}"
            );
            assert_eq!(score.render_container(), source.as_bytes());
        }
    }

    #[test]
    fn drum_template_unused_secondary_percussion_channels_do_not_own_piano_notes() {
        for secondary in [
            "<Channel><midiChannel>9</midiChannel><program value=\"0\"/></Channel>",
            "<Channel channel=\"9\"><program value=\"0\"/></Channel>",
            "<channel><midiChannel>9</midiChannel><program value=\"0\"/></channel>",
        ] {
            let source = score("3.02", &format!("<Part><Staff id=\"1\"/><Instrument id=\"piano\"><instrumentId>keyboard.piano</instrumentId><Channel><midiChannel>0</midiChannel><program value=\"0\"/></Channel>{secondary}</Instrument></Part>"), "<Staff id=\"1\"><Measure><voice><Chord><durationType>quarter</durationType><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff>");
            let mut unchanged = ScoreStems::read(source.as_bytes()).unwrap();
            assert!(
                unchanged.prepare_for_renderer(4).unwrap().is_empty(),
                "{secondary}"
            );
            assert!(!unchanged.has_render_preparation());
            assert_eq!(unchanged.render_container(), source.as_bytes());
            let midi = musescore::parse(source.as_bytes()).unwrap();
            assert!(midi
                .tracks
                .iter()
                .flat_map(|t| &t.events)
                .filter_map(|e| match &e.kind {
                    crate::engine::midi::Kind::NoteOn(note) => Some(note),
                    _ => None,
                })
                .all(|n| n.source.instrument_role
                    == crate::engine::midi::NoteInstrumentRole::Pitched));
            for selection in [
                "<subchannel>1</subchannel>",
                "<instrument id=\"piano\"/><subchannel>1</subchannel>",
                "<instrument id=\"piano:channel:1\"/>",
            ] {
                let source = source.replace(
                    "<Note><pitch>60</pitch>",
                    &format!("<Note><pitch>60</pitch>{selection}"),
                );
                let mut selected = ScoreStems::read(source.as_bytes()).unwrap();
                assert!(
                    selected
                        .prepare_for_renderer(4)
                        .unwrap_err()
                        .starts_with("MUSESCORE_DRUM_TEMPLATE_UNPROVEN:"),
                    "{secondary}: {selection}"
                );
                assert_eq!(selected.render_container(), source.as_bytes());
            }
            let chord_selected = source.replace(
                "<Chord><durationType>",
                "<Chord><subchannel>1</subchannel><durationType>",
            );
            let mut selected = ScoreStems::read(chord_selected.as_bytes()).unwrap();
            assert!(selected
                .prepare_for_renderer(4)
                .unwrap_err()
                .starts_with("MUSESCORE_DRUM_TEMPLATE_UNPROVEN:"));
            assert_eq!(selected.render_container(), chord_selected.as_bytes());
        }
    }

    #[test]
    fn drum_template_edits_the_qualified_native_attribute_not_a_namespaced_id() {
        let source = DRUM_CONFLICT.replace(
            "<Instrument id=\"piano\">",
            "<Instrument xmlns:x=\"urn:source\" x:id=\"evidence\" id=\"piano\">",
        );
        let mut score = ScoreStems::read(source.as_bytes()).unwrap();
        assert_eq!(score.prepare_for_renderer(4).unwrap().len(), 1);
        let prepared = std::str::from_utf8(score.render_container()).unwrap();
        let doc = roxmltree::Document::parse(prepared).unwrap();
        let instruments: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name("Instrument"))
            .collect();
        assert_eq!(instruments[0].attribute("id"), Some("piano"));
        assert_eq!(instruments[2].attribute("id"), Some("drumset"));
        assert_eq!(
            instruments[2].attribute(("urn:source", "id")),
            Some("evidence")
        );
        assert_eq!(prepared.replace("id=\"drumset\"", "id=\"piano\""), source);
    }

    #[test]
    fn drum_template_multiple_conflicting_parts_preserve_xml_topology_and_stems() {
        let document = roxmltree::Document::parse(DRUM_CONFLICT).unwrap();
        let score = document
            .descendants()
            .find(|n| n.has_tag_name("Score"))
            .unwrap();
        let drum_part = score
            .children()
            .filter(|n| n.has_tag_name("Part"))
            .nth(2)
            .unwrap();
        let drum_staff = score
            .children()
            .find(|n| n.has_tag_name("Staff") && n.attribute("id") == Some("3"))
            .unwrap();
        let extra_part = DRUM_CONFLICT[drum_part.range()].replace("id=\"3\"", "id=\"4\"");
        let extra_staff = DRUM_CONFLICT[drum_staff.range()].replace("id=\"3\"", "id=\"4\"");
        let source = DRUM_CONFLICT
            .replace(
                "<Staff id=\"1\"><Measure>",
                &format!("{extra_part}<Staff id=\"1\"><Measure>"),
            )
            .replace("</Score>", &format!("{extra_staff}</Score>"));
        let topology = musescore::parse(source.as_bytes()).unwrap().topology;
        let mut scores = ScoreStems::read(source.as_bytes()).unwrap();
        let before = scores.parts().to_vec();
        assert_eq!(scores.prepare_for_renderer(4).unwrap().len(), 2);
        let prepared = std::str::from_utf8(scores.render_container()).unwrap();
        let doc = roxmltree::Document::parse(prepared).unwrap();
        assert_eq!(
            doc.descendants()
                .filter(|n| n.has_tag_name("Instrument"))
                .map(|n| n.attribute("id").unwrap())
                .collect::<Vec<_>>(),
            ["piano", "electric-bass", "drumset", "drumset"]
        );
        assert_eq!(prepared.replace("id=\"drumset\"", "id=\"piano\""), source);
        scores.validate_topology(&topology, true).unwrap();
        assert_eq!(
            scores
                .parts()
                .iter()
                .map(|p| (&p.id, &p.staff_ids, p.audible_elements()))
                .collect::<Vec<_>>(),
            before
                .iter()
                .map(|p| (&p.id, &p.staff_ids, p.audible_elements()))
                .collect::<Vec<_>>()
        );
        for keep in 0..4 {
            let stem = scores.silenced(keep).unwrap();
            roxmltree::Document::parse(std::str::from_utf8(&stem).unwrap()).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&stem)
                    .matches("id=\"drumset\"")
                    .count(),
                2
            );
            let isolated = ScoreStems::read(&stem).unwrap();
            isolated.validate_topology(&topology, true).unwrap();
            for (index, part) in isolated.parts().iter().enumerate() {
                assert_eq!(part.audible_elements() > 0, index == keep);
            }
        }
    }

    #[test]
    fn drum_template_archive_keeps_unrelated_compressed_entries_and_metadata() {
        let container =
            b"<container><rootfiles><rootfile full-path=\"score.mscx\"/></rootfiles></container>";
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer.set_comment("preserve archive metadata");
        for (name, bytes) in [
            ("META-INF/container.xml", &container[..]),
            ("score.mscx", DRUM_CONFLICT.as_bytes()),
            ("Excerpts/score.mscx", DRUM_CONFLICT.as_bytes()),
            ("audio/settings.json", b"{\"source\":true}"),
        ] {
            writer
                .start_file(
                    name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated)
                        .unix_permissions(0o640),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        let original = writer.finish().unwrap().into_inner();
        let mut scores = ScoreStems::read(&original).unwrap();
        scores.prepare_for_renderer(4).unwrap();
        let mut before = zip::ZipArchive::new(Cursor::new(&original)).unwrap();
        let mut after = zip::ZipArchive::new(Cursor::new(scores.render_container())).unwrap();
        assert_eq!(before.comment(), after.comment());
        assert_eq!(
            before.file_names().collect::<Vec<_>>(),
            after.file_names().collect::<Vec<_>>()
        );
        for i in 0..before.len() {
            let mut a = before.by_index_raw(i).unwrap();
            let mut b = after.by_index_raw(i).unwrap();
            assert_eq!(a.unix_mode(), b.unix_mode());
            assert_eq!(a.last_modified(), b.last_modified());
            if a.name() == "score.mscx" {
                continue;
            }
            assert_eq!(a.crc32(), b.crc32());
            let (mut x, mut y) = (Vec::new(), Vec::new());
            a.read_to_end(&mut x).unwrap();
            b.read_to_end(&mut y).unwrap();
            assert_eq!(x, y, "unchanged compressed entry");
        }
    }

    #[test]
    fn drum_template_refuses_unproven_conflicts_without_mutating_input() {
        for (from, to) in [
            ("ctrl=\"0\" value=\"1\"", "ctrl=\"0\" value=\"0\""),
            ("ctrl=\"32\" value=\"0\"", "ctrl=\"32\" value=\"1\""),
            (
                "<midiChannel>9</midiChannel>",
                "<midiChannel>8</midiChannel>",
            ),
            ("<useDrumset>1</useDrumset>", "<useDrumset>0</useDrumset>"),
            ("group=\"percussion\"", "group=\"pitched\""),
            (
                "<useDrumset>1</useDrumset>",
                "<useDrumset>1</useDrumset><soundId>piano.keyboards</soundId>",
            ),
            (
                "<midiChannel>9</midiChannel>",
                "<midiChannel>9</midiChannel><program value=\"8\"/>",
            ),
            (
                "<useDrumset>1</useDrumset>",
                "<useDrumset>1</useDrumset><Channel/>",
            ),
            ("<Drum pitch=\"42\">", "<Drum pitch=\"82\">"),
            ("<Drum pitch=\"42\">", "<Drum pitch=\"43\">"),
            ("<program value=\"0\"/>", "<program value=\"8\"/>"),
            ("<controller ctrl=\"0\" value=\"1\"/>", ""),
            ("<Instrument id=\"piano\">", "<Instrument id=\"flute\">"),
            (
                "<useDrumset>1</useDrumset>",
                "<useDrumset>1</useDrumset><Drum pitch=\"42\"/>",
            ),
            (
                "<trackName>Drums</trackName>",
                "<trackName>Drums</trackName><Staff id=\"9\"/>",
            ),
            ("version=\"3.02\"", "version=\"5.0\""),
        ] {
            let source = DRUM_CONFLICT.replace(from, to).into_bytes();
            let mut score = ScoreStems::read(&source).unwrap();
            assert!(
                score
                    .prepare_for_renderer(4)
                    .unwrap_err()
                    .starts_with("MUSESCORE_DRUM_TEMPLATE_UNPROVEN:"),
                "{to}"
            );
            assert_eq!(score.render_container(), source);
        }
    }

    #[test]
    fn drum_template_leaves_ordinary_piano_and_canonical_drums_byte_exact() {
        let mut prepared = ScoreStems::read(DRUM_CONFLICT.as_bytes()).unwrap();
        prepared.prepare_for_renderer(4).unwrap();
        let canonical = prepared.render_container().to_vec();
        let mut canonical_score = ScoreStems::read(&canonical).unwrap();
        assert!(canonical_score.prepare_for_renderer(4).unwrap().is_empty());
        assert_eq!(canonical_score.render_container(), canonical);
        // Display names cannot authorize a mapping.
        let piano = score("3.02", "<Part><Staff id=\"1\"/><trackName>Drum Kit</trackName><Instrument id=\"piano\"><instrumentId>keyboard.piano</instrumentId></Instrument></Part>", "<Staff id=\"1\"><Measure><voice><Chord><Note><pitch>48</pitch></Note></Chord></voice></Measure></Staff>");
        let mut ordinary = ScoreStems::read(piano.as_bytes()).unwrap();
        assert!(ordinary.prepare_for_renderer(4).unwrap().is_empty());
        assert_eq!(ordinary.render_container(), piano.as_bytes());
    }

    #[test]
    fn configured_real_renderer_drum_template_matches_canonical_control() {
        use crate::renderer::{AudioRenderer, MuseScoreRenderer, RenderLimits};
        use std::{fs, path::Path, time::Duration};
        let Ok(executable) = std::env::var("VERSE_MUSESCORE_GATE") else {
            return;
        };
        let renderer = MuseScoreRenderer::probe(Path::new(&executable)).unwrap();
        if renderer.capabilities().identity.major == 3 {
            return;
        }
        assert_eq!(
            renderer.capabilities().identity.major,
            4,
            "this acoustic qualification is for MuseScore 4"
        );
        let retained = std::env::var_os("VERSE_DRUM_OUTPUT_DIR").map(std::path::PathBuf::from);
        let root = retained.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("verse-drum-control-{}", std::process::id()))
        });
        fs::create_dir(&root).expect("control output directory must be new");
        let mut compatible = ScoreStems::read(DRUM_CONFLICT.as_bytes()).unwrap();
        compatible.prepare_for_renderer(4).unwrap();
        let corrected = compatible.silenced(2).unwrap();
        // Canonical control independently states both the native template and
        // percussion taxonomy; the compatibility edit changes only the former.
        let canonical = String::from_utf8(corrected.clone())
            .unwrap()
            .replacen(
                "<Instrument id=\"drumset\"><instrumentId>keyboard.piano</instrumentId>",
                "<Instrument id=\"drumset\"><instrumentId>drum.group.set</instrumentId>",
                1,
            )
            .into_bytes();
        assert_ne!(canonical, corrected);
        let piano = String::from_utf8(corrected.clone())
            .unwrap()
            .replace("id=\"drumset\"", "id=\"piano\"")
            .replace("group=\"percussion\"", "group=\"pitched\"")
            .replace("<useDrumset>1</useDrumset>", "<useDrumset>0</useDrumset>")
            .replace(
                "<midiChannel>9</midiChannel>",
                "<midiChannel>0</midiChannel>",
            )
            .replace("ctrl=\"0\" value=\"1\"", "ctrl=\"0\" value=\"0\"");
        let doc = roxmltree::Document::parse(&piano).unwrap();
        let mut piano_bytes = piano.as_bytes().to_vec();
        for range in doc
            .descendants()
            .filter(|n| n.has_tag_name("Drum"))
            .map(|n| n.range())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            piano_bytes.drain(range);
        }
        let limits = RenderLimits {
            timeout: Duration::from_secs(5 * 60),
            max_output_bytes: 64 * 1024 * 1024,
        };
        let mut outputs = Vec::new();
        for (name, bytes) in [
            ("corrected", corrected),
            ("canonical", canonical),
            ("piano", piano_bytes),
        ] {
            let input = root.join(format!("{name}.mscx"));
            fs::write(&input, bytes).unwrap();
            let rendered = renderer
                .render(&input, &root.join(format!("{name}.wav")), &limits)
                .unwrap();
            outputs.push(rendered);
        }
        fn payload(bytes: &[u8]) -> &[u8] {
            let mut offset = 12;
            while offset + 8 <= bytes.len() {
                let len =
                    u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
                if &bytes[offset..offset + 4] == b"data" {
                    return &bytes[offset + 8..offset + 8 + len];
                }
                offset += 8 + len + len % 2;
            }
            panic!("validated WAV has no data chunk");
        }
        let samples: Vec<_> = outputs.iter().map(|o| fs::read(&o.path).unwrap()).collect();
        assert_eq!(outputs[0].wav.sample_rate, outputs[1].wav.sample_rate);
        assert_eq!(outputs[0].wav.channels, outputs[1].wav.channels);
        assert!(
            payload(&samples[0]) == payload(&samples[1]),
            "corrected and canonical PCM must be byte-identical"
        );
        let frame_bytes =
            usize::from(outputs[0].wav.channels) * usize::from(outputs[0].wav.bits_per_sample / 8);
        let quarter = outputs[0].wav.sample_rate as usize / 2 * frame_bytes;
        for (attack, name) in ["kick", "snare", "hi-hat"].into_iter().enumerate() {
            let start = attack * quarter;
            let end = start + quarter;
            assert!(
                payload(&samples[0])[start..end] != payload(&samples[2])[start..end],
                "{name} must differ from piano"
            );
        }
        let receipt = serde_json::json!({
            "renderer": renderer.capabilities().identity,
            "comparison": "byte-identical complete WAV data chunks; kick/snare/hi-hat each differ from ordinary piano over their 0.5-second source attack window",
            "canonicalMatches": true, "pianoDiffers": true,
            "wav": outputs.iter().map(|o| &o.wav).collect::<Vec<_>>(),
        });
        fs::write(
            root.join("comparison.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        eprintln!("{}", root.display());
        if retained.is_none() {
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn score(version: &str, parts: &str, staves: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<museScore version=\"{version}\">\n\
             <Score>\n<Division>480</Division>\n{parts}{staves}</Score>\n</museScore>\n"
        )
    }

    fn zipped(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    const PARTS: &str = "<Part><Staff id=\"1\"/><trackName>Voice</trackName></Part>\n\
        <Part><Staff id=\"2\"/><Staff id=\"3\"/><trackName>Piano</trackName></Part>\n\
        <Part><Staff id=\"4\"/><trackName>Chords</trackName></Part>\n";

    fn staves() -> String {
        "<Staff id=\"1\"><Measure><voice><Fermata/><Chord><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff>\n\
         <Staff id=\"2\"><Measure><voice><Chord><Note><pitch>48</pitch></Note><Note><play>0</play><pitch>52</pitch></Note></Chord></voice></Measure></Staff>\n\
         <Staff id=\"3\"><Measure><voice><Breath/><Chord><Note>\n<pitch>36</pitch>\n</Note ></Chord></voice></Measure></Staff>\n\
         <Staff id=\"4\"><Measure><voice><Harmony><root>14</root></Harmony><Rest/></voice></Measure></Staff>\n"
            .into()
    }

    fn remove_silence(bytes: &[u8]) -> Vec<u8> {
        std::str::from_utf8(bytes)
            .unwrap()
            .replace(SILENCE_TEXT, "")
            .into_bytes()
    }

    const SILENCE_TEXT: &str = "<play>0</play>";

    #[test]
    fn a_stem_is_the_whole_score_with_only_other_parts_silenced() {
        let source = score("3.02", PARTS, &staves());
        let stems = ScoreStems::read(source.as_bytes()).unwrap();
        assert_eq!(stems.extension(), "mscx");
        assert_eq!(
            stems
                .parts()
                .iter()
                .map(|part| (
                    part.id.as_str(),
                    part.staff_ids.clone(),
                    part.audible_elements()
                ))
                .collect::<Vec<_>>(),
            [
                ("musescore-part-0", vec!["1".to_string()], 1),
                ("musescore-part-1", vec!["2".into(), "3".into()], 2),
                ("musescore-part-2", vec!["4".into()], 1),
            ]
        );
        let voice = String::from_utf8(stems.silenced(0).unwrap()).unwrap();
        // The pre-existing silenced note counts once; three elements are added.
        assert_eq!(voice.matches(SILENCE_TEXT).count(), 4);
        assert!(voice.contains("<Note><pitch>60</pitch></Note>"));
        assert!(voice.contains("<Note><pitch>48</pitch><play>0</play></Note>"));
        assert!(voice.contains("<pitch>36</pitch>\n<play>0</play></Note >"));
        assert!(voice.contains("<Harmony><root>14</root><play>0</play></Harmony>"));
        // Fermata, breath and everything else stay byte-identical.
        assert_eq!(
            remove_silence(&stems.silenced(0).unwrap()),
            remove_silence(source.as_bytes())
        );
        let chords = String::from_utf8(stems.silenced(2).unwrap()).unwrap();
        assert!(chords.contains("<Harmony><root>14</root></Harmony>"));
        assert!(chords.contains("<Note><pitch>60</pitch><play>0</play></Note>"));
        assert!(stems.silenced(3).is_err());
    }

    #[test]
    fn a_zipped_score_changes_only_its_master_and_keeps_excerpts() {
        let master = score("4.20", PARTS, &staves());
        let excerpt = score("4.20", PARTS, &staves());
        let container =
            b"<container><rootfiles><rootfile full-path=\"score.mscx\"/></rootfiles></container>";
        let archive = zipped(&[
            ("META-INF/container.xml", container),
            ("score.mscx", master.as_bytes()),
            ("Excerpts/Piano/Piano.mscx", excerpt.as_bytes()),
        ]);
        let stems = ScoreStems::read(&archive).unwrap();
        assert_eq!(stems.extension(), "mscz");
        let silenced = stems.silenced(1).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(&silenced)).unwrap();
        let names: Vec<_> = zip.file_names().map(str::to_string).collect();
        assert_eq!(
            names,
            [
                "META-INF/container.xml",
                "score.mscx",
                "Excerpts/Piano/Piano.mscx"
            ]
        );
        let mut read = |name: &str| {
            let mut bytes = Vec::new();
            zip.by_name(name).unwrap().read_to_end(&mut bytes).unwrap();
            bytes
        };
        assert_eq!(read("Excerpts/Piano/Piano.mscx"), excerpt.as_bytes());
        assert_eq!(read("META-INF/container.xml"), container);
        let new_master = read("score.mscx");
        assert_eq!(
            remove_silence(&new_master),
            remove_silence(master.as_bytes())
        );
        let new_master = String::from_utf8(new_master).unwrap();
        assert!(new_master.contains("<Note><pitch>48</pitch></Note>"));
        assert!(new_master.contains("<Note><pitch>60</pitch><play>0</play></Note>"));
    }

    #[test]
    fn musescore_four_part_declarations_take_body_staff_ids_in_order() {
        let parts = "<Part id=\"1\"><Staff/><trackName>Voice</trackName></Part>\
                     <Part id=\"2\"><Staff/><Staff/><trackName>Piano</trackName></Part>";
        let staves = "<Staff id=\"1\"><Measure><voice><Chord><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff>\
                      <Staff id=\"2\"><Measure><voice><Chord><Note><pitch>48</pitch></Note></Chord></voice></Measure></Staff>\
                      <Staff id=\"3\"><Measure><voice><Chord><Note><pitch>36</pitch></Note></Chord></voice></Measure></Staff>";
        let stems = ScoreStems::read(score("4.20", parts, staves).as_bytes()).unwrap();
        assert_eq!(stems.parts()[0].id, "musescore-part-1");
        assert_eq!(stems.parts()[1].staff_ids, ["2", "3"]);
        assert_eq!(stems.parts()[1].audible_elements(), 2);
    }

    #[test]
    fn an_unowned_or_unsilenceable_staff_fails_closed() {
        let orphan = format!(
            "{}<Staff id=\"9\"><Measure><voice><Chord><Note><pitch>60</pitch></Note></Chord></voice></Measure></Staff>",
            staves()
        );
        assert!(ScoreStems::read(score("3.02", PARTS, &orphan).as_bytes())
            .unwrap_err()
            .contains("cannot be mapped to exactly one source Part"));
        let empty_note =
            "<Staff id=\"1\"><Measure><voice><Chord><Note/></Chord></voice></Measure></Staff>";
        assert!(
            ScoreStems::read(score("3.02", PARTS, empty_note).as_bytes())
                .unwrap_err()
                .contains("cannot be silenced in place")
        );
        let latin1 = [score("3.02", PARTS, &staves()).as_bytes(), &[0xe9]].concat();
        assert!(ScoreStems::read(&latin1).is_err());
    }

    #[test]
    fn a_byte_order_mark_stays_ahead_of_the_silenced_master() {
        let source = [
            &[0xef, 0xbb, 0xbf][..],
            score("3.02", PARTS, &staves()).as_bytes(),
        ]
        .concat();
        let silenced = ScoreStems::read(&source).unwrap().silenced(0).unwrap();
        assert!(silenced.starts_with(&[0xef, 0xbb, 0xbf]));
        let text = String::from_utf8(silenced[3..].to_vec()).unwrap();
        assert!(text.contains("<pitch>36</pitch>\n<play>0</play></Note >"));
        assert!(text.contains("<Harmony><root>14</root><play>0</play></Harmony>"));
        // Removing every `<play>0</play>` from both leaves identical bytes.
        assert_eq!(remove_silence(&silenced[3..]), remove_silence(&source[3..]));
        assert_eq!(
            text.matches(SILENCE_TEXT).count(),
            std::str::from_utf8(&source[3..])
                .unwrap()
                .matches(SILENCE_TEXT)
                .count()
                + 3
        );
    }

    #[test]
    fn a_master_score_over_64_mib_is_refused_zipped_or_raw() {
        let oversized = vec![b' '; MAX_MASTER_BYTES as usize + 1];
        assert!(ScoreStems::read(&oversized)
            .unwrap_err()
            .contains("abnormally large"));
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "score.mscx",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(&oversized).unwrap();
        let archive = writer.finish().unwrap().into_inner();
        assert!(archive.len() < 1024 * 1024, "deflated padding stays small");
        assert!(ScoreStems::read(&archive)
            .unwrap_err()
            .contains("abnormally large"));
    }

    #[test]
    fn topology_must_match_part_by_part() {
        let source = score("3.02", PARTS, &staves());
        let stems = ScoreStems::read(source.as_bytes()).unwrap();
        let topology = |ids: &[(&str, &[&str])]| SourceTopology {
            parts: ids
                .iter()
                .map(|(id, staves)| crate::engine::midi::SourcePart {
                    id: (*id).into(),
                    name: String::new(),
                    source_track_ids: Vec::new(),
                    staves: staves
                        .iter()
                        .map(|staff| crate::engine::midi::SourceStaff {
                            id: (*staff).into(),
                            voices: Vec::new(),
                        })
                        .collect(),
                    playable_chord_symbols: 0,
                })
                .collect(),
        };
        let native = topology(&[
            ("musescore-part-0", &["1"]),
            ("musescore-part-1", &["2", "3"]),
            ("musescore-part-2", &["4"]),
        ]);
        stems.validate_topology(&native, true).unwrap();
        let converted = topology(&[("P1", &["1"]), ("P2", &["1", "2"]), ("P3", &["1"])]);
        stems.validate_topology(&converted, false).unwrap();
        assert!(stems.validate_topology(&converted, true).is_err());
        let swapped = topology(&[("P1", &["1", "2"]), ("P2", &["1"]), ("P3", &["1"])]);
        assert!(stems.validate_topology(&swapped, false).is_err());
        let missing = topology(&[("P1", &["1"]), ("P2", &["1", "2"])]);
        assert!(stems
            .validate_topology(&missing, false)
            .unwrap_err()
            .contains("3 Parts but the source topology has 2"));
    }
}
