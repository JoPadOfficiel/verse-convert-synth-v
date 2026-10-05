use super::{
    files::read_bounded,
    memory::{Correction, Memory},
    Error, POLICY,
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
pub const FORMAT: &str = "verse.pronunciation-corrections";
pub const MAX_EXCHANGE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub format: String,
    pub schema_version: u32,
    pub verse_version: String,
    pub pronunciation_policy: String,
    pub created_at_unix_seconds: u64,
    pub corrections: Vec<Correction>,
}
impl Package {
    pub fn confirmed(memory: &Memory) -> Result<Self, Error> {
        Ok(Self {
            format: FORMAT.into(),
            schema_version: 1,
            verse_version: env!("CARGO_PKG_VERSION").into(),
            pronunciation_policy: POLICY.into(),
            created_at_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            corrections: memory.snapshot()?,
        })
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.format != FORMAT
            || self.schema_version != 1
            || self.verse_version.is_empty()
            || self.corrections.len() > 10_000
        {
            return Err(Error::new(
                "PRONUNCIATION_EXCHANGE_SCHEMA",
                "Unsupported correction exchange format",
            ));
        }
        let mut ids = std::collections::BTreeMap::new();
        for c in &self.corrections {
            c.validate()?;
            if c.provenance.policy != self.pronunciation_policy {
                return Err(Error::new(
                    "PRONUNCIATION_EXCHANGE_IDENTITY",
                    "Envelope policy differs from correction policy",
                ));
            }
            if let Some(old) = ids.insert(&c.id, c) {
                if old != c {
                    return Err(Error::new(
                        "PRONUNCIATION_EXCHANGE_CONFLICT",
                        "Duplicate correction identity differs",
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn read(path: &Path) -> Result<Self, Error> {
        let package: Self = serde_json::from_slice(&read_bounded(path, MAX_EXCHANGE_BYTES)?)?;
        package.validate()?;
        Ok(package)
    }
    pub fn write(&self, path: &Path) -> Result<(), Error> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)?;
        if bytes.len() as u64 > MAX_EXCHANGE_BYTES {
            return Err(Error::new(
                "PRONUNCIATION_FILE_LIMIT",
                "Correction exchange exceeds its serialized byte budget",
            ));
        }
        crate::bundle::write_bytes_no_replace(path, &bytes)
            .map_err(|e| Error::new("PRONUNCIATION_EXCHANGE_WRITE", &e.to_string()))
    }
    pub fn import_pending(&self, memory: &mut Memory) -> Result<(), Error> {
        self.validate()?;
        memory.store(&self.corrections, false)
    }
}
