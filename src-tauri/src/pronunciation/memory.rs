use super::{
    assets::valid_hash,
    hash,
    source_map::{Reading, Reference, Word},
    Error, Language, POLICY,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct VoiceConstraint {
    pub singer: String,
    pub inventory_sha256: String,
    pub configuration_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Scope {
    OccurrenceOnly,
    CompatibleContext,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub source_sha256: String,
    pub export_sha256: String,
    pub corrected_sha256: String,
    pub confirmed_after_listening: bool,
    pub symbol_validation: String,
    pub policy: String,
    /// Native identifier observed in the corrected project, never inventory qualification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_singer: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Correction {
    pub id: String,
    pub fingerprint: String,
    pub word: Word,
    pub before: Reading,
    pub after: Reading,
    pub dialect: Option<String>,
    pub accepted_variant: Option<String>,
    pub scope: Scope,
    pub target: String,
    pub profile: String,
    pub voice: Option<VoiceConstraint>,
    pub provenance: Provenance,
}

impl Correction {
    pub fn fingerprint(&self) -> Result<String, Error> {
        Ok(hash(&serde_json::to_vec(&(
            &self.word,
            &self.before,
            &self.after,
            &self.dialect,
            &self.accepted_variant,
            &self.scope,
            &self.target,
            &self.profile,
            &self.voice,
            &self.provenance,
        ))?))
    }
    pub fn seal(&mut self) -> Result<(), Error> {
        self.fingerprint = self.fingerprint()?;
        self.id = format!("correction:{}", self.fingerprint);
        Ok(())
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.fingerprint != self.fingerprint()?
            || self.id != format!("correction:{}", self.fingerprint)
            || !self.provenance.confirmed_after_listening
            || !["unknown", "base_inventory_valid", "native_inventory_valid"]
                .contains(&self.provenance.symbol_validation.as_str())
            || self.target != "ustx"
            || self.profile != "automatic"
            || self.word.manual
            || self.word.key.is_empty()
            || self.word.key.len() > 512
            || self.word.original.is_empty()
            || self.word.original.len() > 32
            || self.word.members.is_empty()
            || self.word.members.len() > 64
            || self
                .word
                .members
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.word.members.len()
            || self.word.context.is_empty()
            || self.word.context.len() > 5
            || self.word.context_target >= self.word.context.len()
            || self.word.context[self.word.context_target] != self.word.key
            || self.word.context.iter().any(|s| s.len() > 512)
            || self.word.attacks == 0
            || self.word.attacks > self.word.members.len()
            || self.after.language.is_none()
            || self.after.language.is_some_and(|l| {
                l.phonemizer() != self.after.phonemizer && l.native_name() != self.after.phonemizer
            })
            || [
                &self.provenance.source_sha256,
                &self.provenance.export_sha256,
                &self.provenance.corrected_sha256,
            ]
            .iter()
            .any(|h| !valid_hash(h))
        {
            return Err(Error::new(
                "PRONUNCIATION_CORRECTION_INVALID",
                "Invalid correction identity, ownership, context or confirmation",
            ));
        }
        if self
            .provenance
            .observed_singer
            .as_ref()
            .is_some_and(|singer| {
                singer.trim().is_empty() || singer.len() > 4096 || singer.contains('\0')
            })
        {
            return Err(Error::new(
                "PRONUNCIATION_SINGER_OBSERVATION_INVALID",
                "Observed singer must be a bounded nonblank native identifier",
            ));
        }
        if let Some(voice) = &self.voice {
            if voice.singer.is_empty()
                || voice.singer.len() > 256
                || !valid_hash(&voice.inventory_sha256)
                || !valid_hash(&voice.configuration_sha256)
            {
                return Err(Error::new(
                    "PRONUNCIATION_VOICE_SCOPE_INVALID",
                    "Complete singer/inventory/configuration identity required",
                ));
            }
        }
        if let Some(phones) = &self.after.phones {
            if phones.is_empty()
                || phones.len() > 128
                || phones.iter().any(|p| {
                    p.is_empty()
                        || p.len() > 64
                        || p.chars().any(char::is_whitespace)
                        || p.contains(['[', ']', '\0'])
                })
                || self.after.lexical_reading.is_none()
            {
                return Err(Error::new(
                    "PRONUNCIATION_PHONES_INVALID",
                    "Malformed ordered phone reading",
                ));
            }
            if self.scope == Scope::CompatibleContext
                && (self.voice.is_none()
                    || self.after.alphabet.is_none()
                    || self.provenance.symbol_validation != "native_inventory_valid")
                && !self.declared_reading_authority()
            {
                return Err(Error::new(
                    "PRONUNCIATION_INVENTORY_REQUIRED",
                    "Reusable phones require qualified native voice inventories",
                ));
            }
            if self.after.alphabet != super::source_map::phone_alphabet(Some(phones)) {
                return Err(Error::new(
                    "PRONUNCIATION_ALPHABET_INVALID",
                    "Declared alphabet differs from retained explicit symbol prefixes",
                ));
            }
        } else if self.after.alphabet.is_some() {
            return Err(Error::new(
                "PRONUNCIATION_PHONES_INVALID",
                "Alphabet cannot manufacture missing phones",
            ));
        }
        if self.after.authority.is_some() && !self.declared_reading_authority() {
            return Err(Error::new(
                "PRONUNCIATION_READING_AUTHORITY_INVALID",
                "Malformed reading authority or missing explicit phone evidence",
            ));
        }
        if let Some(aliases) = &self.after.aliases {
            let mut seen = std::collections::BTreeSet::new();
            if aliases.is_empty()
                || aliases.len() > 128
                || aliases.iter().any(|a| {
                    a.member >= self.word.members.len()
                        || a.index >= 128
                        || a.phone.is_empty()
                        || a.phone.len() > 256
                        || a.phone.contains(['\0', '\n', '\r'])
                        || !seen.insert((a.member, a.index))
                })
            {
                return Err(Error::new(
                    "PRONUNCIATION_ALIAS_INVALID",
                    "Malformed or ambiguous native alias correction",
                ));
            }
            if self.scope == Scope::CompatibleContext
                && (self.voice.is_none()
                    || self.provenance.symbol_validation != "native_inventory_valid")
            {
                return Err(Error::new(
                    "PRONUNCIATION_INVENTORY_REQUIRED",
                    "Native aliases require qualified voice inventories for cross-song reuse",
                ));
            }
        }
        if self
            .accepted_variant
            .as_ref()
            .is_some_and(|key| Some(key) != self.after.lexical_reading.as_ref())
        {
            return Err(Error::new(
                "PRONUNCIATION_READING_AUTHORITY_INVALID",
                "Variant and after-reading identities differ",
            ));
        }
        if self.accepted_variant.is_some()
            && self.scope == Scope::CompatibleContext
            && !self.declared_reading_authority()
        {
            return Err(Error::new(
                "PRONUNCIATION_READING_AUTHORITY_UNQUALIFIED",
                "Unqualified native variants cannot be generalized across songs",
            ));
        }
        Ok(())
    }
    fn declared_reading_authority(&self) -> bool {
        self.after.aliases.is_none()
            && self.after.phones.is_some()
            && self
                .after
                .authority
                .as_deref()
                .and_then(|a| a.strip_prefix(&format!("{}:dictionary:", self.provenance.policy)))
                .is_some_and(valid_hash)
    }
    pub fn compatible(&self) -> bool {
        self.provenance.policy == POLICY
            && (self.after.authority.is_none()
                || super::reading::attested(&self.word.key, &self.after))
    }
    pub fn matches(
        &self,
        word: &Word,
        source_sha256: &str,
        voice: Option<&VoiceConstraint>,
    ) -> bool {
        self.provenance.policy == POLICY
            && self.provenance.confirmed_after_listening
            && self.after.language.is_some()
            && !word.manual
            && self.word.key == word.key
            && self.word.context == word.context
            && self.word.context_target == word.context_target
            && self.word.attacks == word.attacks
            && self.voice.as_ref().is_none_or(|v| Some(v) == voice)
            && match self.scope {
                Scope::OccurrenceOnly => {
                    self.provenance.source_sha256 == source_sha256 && self.word == *word
                }
                Scope::CompatibleContext => true,
            }
    }
}

pub const MAX_CORRECTIONS: usize = 10_000;
pub const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LookupKey {
    key: String,
    context: Vec<String>,
    target: usize,
    attacks: usize,
}
impl From<&Word> for LookupKey {
    fn from(w: &Word) -> Self {
        Self {
            key: w.key.clone(),
            context: w.context.clone(),
            target: w.context_target,
            attacks: w.attacks,
        }
    }
}
#[derive(Clone, Copy)]
struct Resolution {
    first: usize,
    conflict: bool,
}
/// Validated immutable records and precomputed conflicts. Conversion never
/// serializes/hashes each correction again or scans unrelated records.
pub struct CorrectionSet {
    records: Vec<Correction>,
    identity: String,
    reusable: std::collections::BTreeMap<(LookupKey, Option<VoiceConstraint>), Resolution>,
    occurrences: std::collections::BTreeMap<(String, Word, Option<VoiceConstraint>), Resolution>,
}
impl serde::Serialize for CorrectionSet {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.records.serialize(serializer)
    }
}
impl std::ops::Deref for CorrectionSet {
    type Target = [Correction];
    fn deref(&self) -> &Self::Target {
        &self.records
    }
}
impl CorrectionSet {
    pub fn new(records: Vec<Correction>) -> Result<Self, Error> {
        if records.len() > MAX_CORRECTIONS {
            return Err(Error::new(
                "PRONUNCIATION_MEMORY_LIMIT",
                "Correction snapshot record limit exceeded",
            ));
        }
        let mut bytes = 0;
        for c in &records {
            c.validate()?;
            bytes += serde_json::to_vec(c)?.len();
            if bytes > MAX_SNAPSHOT_BYTES {
                return Err(Error::new(
                    "PRONUNCIATION_MEMORY_LIMIT",
                    "Correction snapshot byte limit exceeded",
                ));
            }
        }
        Ok(Self::validated(records))
    }
    fn validated(records: Vec<Correction>) -> Self {
        let mut result = Self {
            identity: hash(&serde_json::to_vec(&records).expect("correction structs serialize")),
            records,
            reusable: Default::default(),
            occurrences: Default::default(),
        };
        for (index, c) in result.records.iter().enumerate() {
            if !c.compatible() {
                continue;
            }
            let resolution = match c.scope {
                Scope::CompatibleContext => result
                    .reusable
                    .entry((LookupKey::from(&c.word), c.voice.clone()))
                    .or_insert(Resolution {
                        first: index,
                        conflict: false,
                    }),
                Scope::OccurrenceOnly => result
                    .occurrences
                    .entry((
                        c.provenance.source_sha256.clone(),
                        c.word.clone(),
                        c.voice.clone(),
                    ))
                    .or_insert(Resolution {
                        first: index,
                        conflict: false,
                    }),
            };
            if result.records[resolution.first].after != c.after {
                resolution.conflict = true;
            }
        }
        result
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn find(
        &self,
        word: &Word,
        source_sha256: &str,
        voice: Option<&VoiceConstraint>,
    ) -> Result<Option<&Correction>, Error> {
        if word.manual {
            return Ok(None);
        }
        let key = LookupKey::from(word);
        let mut result: Option<&Correction> = None;
        for constraint in
            [None, voice.cloned()]
                .into_iter()
                .take(if voice.is_some() { 2 } else { 1 })
        {
            for resolution in [
                self.reusable.get(&(key.clone(), constraint.clone())),
                self.occurrences
                    .get(&(source_sha256.to_string(), word.clone(), constraint)),
            ]
            .into_iter()
            .flatten()
            {
                let record = &self.records[resolution.first];
                if resolution.conflict || result.is_some_and(|first| first.after != record.after) {
                    return Err(Error::new(
                        "PRONUNCIATION_MEMORY_CONFLICT",
                        "Conflicting confirmed corrections require review",
                    ));
                }
                result = Some(record);
            }
        }
        Ok(result)
    }
}

pub fn unique_match<'a>(
    records: &'a [Correction],
    word: &Word,
    source_sha256: &str,
    voice: Option<&VoiceConstraint>,
) -> Result<Option<&'a Correction>, Error> {
    let matching: Vec<_> = records
        .iter()
        .filter(|c| c.matches(word, source_sha256, voice))
        .collect();
    if matching.windows(2).any(|w| w[0].after != w[1].after) {
        return Err(Error::new(
            "PRONUNCIATION_MEMORY_CONFLICT",
            "Conflicting confirmed corrections require review",
        ));
    }
    Ok(matching.first().copied())
}

pub struct Memory {
    connection: Connection,
    root: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub correction: Correction,
    pub status: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceInfo {
    pub id: String,
    pub export_sha256: String,
    pub source_label: Option<String>,
    pub export_label: Option<String>,
    pub created_at_unix_seconds: Option<i64>,
}
impl Memory {
    pub fn open(root: &Path) -> Result<Self, Error> {
        std::fs::create_dir_all(root)?;
        let path = root.join("pronunciation.sqlite3");
        let existed = path.exists();
        let mut connection = Connection::open(&path)?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: i64 = transaction.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 2 {
            return Err(Error::new(
                "PRONUNCIATION_SCHEMA_UNSUPPORTED",
                "Newer correction database is preserved; downgrade refused",
            ));
        }
        if existed && version < 2 {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let backup = root.join(format!("pronunciation-before-v2-{stamp}.sqlite3"));
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&backup)?;
            // A separate reader can back up committed pages while this
            // transaction holds the migration writer reservation.
            let reader =
                Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            reader.backup(rusqlite::MAIN_DB, &backup, None)?;
        }
        if version == 0 {
            transaction.execute_batch("CREATE TABLE corrections(id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL UNIQUE, payload TEXT NOT NULL, status TEXT NOT NULL); CREATE TABLE export_references(id TEXT PRIMARY KEY, export_hash TEXT NOT NULL, source_hash TEXT NOT NULL, payload TEXT NOT NULL); PRAGMA user_version=1;")?;
        }
        if version < 2 {
            transaction.execute_batch("CREATE TABLE correction_history(sequence INTEGER PRIMARY KEY AUTOINCREMENT, correction_id TEXT NOT NULL, action TEXT NOT NULL, at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); PRAGMA user_version=2;")?;
        }
        transaction.commit()?;
        std::fs::create_dir_all(root.join("references"))?;
        Ok(Self {
            connection,
            root: root.into(),
        })
    }
    pub fn snapshot(&self) -> Result<Vec<Correction>, Error> {
        Ok(self
            .history()?
            .into_iter()
            .filter(|r| r.status == "active" && r.correction.compatible())
            .map(|r| r.correction)
            .collect())
    }
    pub fn trusted_snapshot(&self) -> Result<std::sync::Arc<CorrectionSet>, Error> {
        // history validates each durable record once; validated() only indexes.
        Ok(std::sync::Arc::new(CorrectionSet::validated(
            self.snapshot()?,
        )))
    }
    pub fn history(&self) -> Result<Vec<HistoryRecord>, Error> {
        let (count, bytes): (i64, i64) = self.connection.query_row(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM corrections",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count > MAX_CORRECTIONS as i64 || bytes > MAX_SNAPSHOT_BYTES as i64 {
            return Err(Error::new(
                "PRONUNCIATION_MEMORY_LIMIT",
                "Durable correction snapshot exceeds the record/byte budget",
            ));
        }
        let mut statement = self
            .connection
            .prepare("SELECT payload,status FROM corrections ORDER BY id")?;
        let rows =
            statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut result = Vec::new();
        for row in rows {
            let (payload, status) = row?;
            let correction: Correction = serde_json::from_str(&payload)?;
            correction.validate()?;
            let status = if status != "revoked" && !correction.compatible() {
                "incompatible".into()
            } else {
                status
            };
            result.push(HistoryRecord { correction, status });
        }
        Ok(result)
    }
    pub fn store(&mut self, records: &[Correction], activate: bool) -> Result<(), Error> {
        if records.len() > MAX_CORRECTIONS {
            return Err(Error::new(
                "PRONUNCIATION_FILE_LIMIT",
                "Too many correction records",
            ));
        }
        for c in records {
            c.validate()?;
        }
        let transaction = self.connection.transaction()?;
        let (mut count, mut bytes): (i64, i64) = transaction.query_row(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM corrections",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        for c in records {
            let previous: Option<String> = transaction
                .query_row("SELECT status FROM corrections WHERE id=?", [&c.id], |r| {
                    r.get(0)
                })
                .ok();
            // Re-import never revives a removed or conflicted correction.
            if previous.is_some() {
                continue;
            }
            let payload = serde_json::to_string(c)?;
            count += 1;
            bytes += payload.len() as i64;
            if count > MAX_CORRECTIONS as i64 || bytes > MAX_SNAPSHOT_BYTES as i64 {
                return Err(Error::new(
                    "PRONUNCIATION_MEMORY_LIMIT",
                    "Correction import exceeds the aggregate record/byte budget",
                ));
            }
            let status = if !c.compatible() {
                "incompatible"
            } else if activate {
                "active"
            } else {
                "pending"
            };
            transaction.execute(
                "INSERT INTO corrections VALUES (?1,?2,?3,?4)",
                params![c.id, c.fingerprint, payload, status],
            )?;
            transaction.execute(
                "INSERT INTO correction_history(correction_id,action) VALUES (?1,?2)",
                params![c.id, status],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn set_status(&mut self, id: &str, status: &str) -> Result<(), Error> {
        if !["active", "revoked"].contains(&status) {
            return Err(Error::new(
                "PRONUNCIATION_STATUS_INVALID",
                "Unsupported correction status",
            ));
        }
        let existing = self
            .history()?
            .into_iter()
            .find(|r| r.correction.id == id)
            .ok_or_else(|| {
                Error::new("PRONUNCIATION_CORRECTION_MISSING", "Correction not found")
            })?;
        if status == "active" && !existing.correction.compatible() {
            return Err(Error::new(
                "PRONUNCIATION_POLICY_INCOMPATIBLE",
                "Correction policy is incompatible",
            ));
        }
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE corrections SET status=?1 WHERE id=?2",
            params![status, id],
        )?;
        transaction.execute(
            "INSERT INTO correction_history(correction_id,action) VALUES (?1,?2)",
            params![id, status],
        )?;
        transaction.commit()?;
        Ok(())
    }
    pub fn save_reference(&mut self, reference: &Reference, bytes: &[u8]) -> Result<(), Error> {
        reference.check_budget(bytes)?;
        if hash(bytes) != reference.export_sha256 {
            return Err(Error::new(
                "PRONUNCIATION_REFERENCE_INVALID",
                "Export hash differs",
            ));
        }
        let path = self
            .root
            .join("references")
            .join(format!("{}.ustx", reference.export_sha256));
        if path.exists() {
            if hash(&super::assets::read_bounded(&path, 32 * 1024 * 1024)?)
                != reference.export_sha256
            {
                return Err(Error::new(
                    "PRONUNCIATION_REFERENCE_INVALID",
                    "Stored original export changed",
                ));
            }
        } else {
            crate::bundle::write_bytes_no_replace(&path, bytes)
                .map_err(|e| Error::new("PRONUNCIATION_REFERENCE_WRITE", &e.to_string()))?;
        }
        self.connection.execute(
            "INSERT OR IGNORE INTO export_references VALUES (?1,?2,?3,?4)",
            params![
                reference.id,
                reference.export_sha256,
                reference.source_sha256,
                serde_json::to_string(reference)?
            ],
        )?;
        Ok(())
    }
    pub fn reference(&self, id: &str) -> Result<Reference, Error> {
        let payload: String = self.connection.query_row(
            "SELECT payload FROM export_references WHERE id=?",
            [id],
            |r| r.get(0),
        )?;
        let reference: Reference = serde_json::from_str(&payload)?;
        let path = self
            .root
            .join("references")
            .join(format!("{}.ustx", reference.export_sha256));
        if hash(&super::assets::read_bounded(&path, 32 * 1024 * 1024)?) != reference.export_sha256 {
            return Err(Error::new(
                "PRONUNCIATION_REFERENCE_INVALID",
                "Original export is missing or changed",
            ));
        }
        Ok(reference)
    }
    pub fn references(&self) -> Result<Vec<ReferenceInfo>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT id,export_hash,json_extract(payload,'$.source_label'),json_extract(payload,'$.export_label'),json_extract(payload,'$.created_at_unix_seconds') FROM export_references ORDER BY rowid DESC LIMIT 4096")?;
        let result = statement
            .query_map([], |r| {
                Ok(ReferenceInfo {
                    id: r.get(0)?,
                    export_sha256: r.get(1)?,
                    source_label: r.get(2)?,
                    export_label: r.get(3)?,
                    created_at_unix_seconds: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(result)
    }
}

pub fn apply_languages(
    tracks: &mut [&mut crate::engine::projection::ProjectedTrack],
    snapshot: &super::Snapshot,
    words: &[Word],
) -> Vec<crate::engine::convert::Diagnostic> {
    let mut diagnostics = Vec::new();
    if snapshot.corrections.is_empty() {
        return diagnostics;
    }
    for word in words {
        match snapshot
            .corrections
            .find(word, &snapshot.source_sha256, None)
        {
            Ok(Some(c)) => {
                let language = c.after.language.unwrap().projected();
                for track in tracks.iter_mut() {
                    if !track.notes.iter().any(|n| word.matches_head(n)) {
                        continue;
                    }
                    for note in &mut track.notes {
                        if note
                            .source_evidence
                            .as_ref()
                            .is_some_and(|e| word.members.contains(&e.note_id))
                        {
                            note.pronunciation_language = Some(language);
                        }
                    }
                }
                diagnostics.push(crate::engine::convert::Diagnostic {
                    code: if c.before.language!=c.after.language {"PRONUNCIATION_MEMORY_LANGUAGE_APPLIED"}else{"PRONUNCIATION_MEMORY_LANGUAGE_CONFIRMED"}.into(),
                    severity: crate::engine::convert::DiagnosticSeverity::Info,
                    message: format!("Confirmed language authority {} retained; reading qualification is separate", c.id),
                    source_id: Some(word.id.clone()),
                });
            }
            Err(e) => diagnostics.push(crate::engine::convert::Diagnostic {
                code: e.code,
                severity: crate::engine::convert::DiagnosticSeverity::Warning,
                message: e.message,
                source_id: Some(word.id.clone()),
            }),
            _ => {}
        }
    }
    diagnostics
}

pub fn language_index(language: Language) -> usize {
    match language {
        Language::French => 0,
        Language::English => 1,
        Language::Spanish => 2,
        Language::Portuguese => 3,
    }
}
