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

fn replace_entry(archive: &[u8], path: &str, master: &[u8]) -> Result<Vec<u8>, String> {
    let mut source = zip::ZipArchive::new(Cursor::new(archive)).map_err(|e| e.to_string())?;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let entry = source.by_index_raw(index).map_err(|e| e.to_string())?;
        if entry.name() == path {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(entry.compression())
                .last_modified_time(entry.last_modified().unwrap_or_default());
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
