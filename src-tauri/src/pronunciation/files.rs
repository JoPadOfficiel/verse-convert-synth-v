//! Bounded local reads and SHA-256 validation shared by correction workflows.
use super::Error;
use std::{fs, io::Read, path::Path};

pub fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > max {
        return Err(Error::new(
            "PRONUNCIATION_FILE_LIMIT",
            "Expected a bounded regular file",
        ));
    }
    let mut data = Vec::new();
    fs::File::open(path)?.take(max + 1).read_to_end(&mut data)?;
    if data.len() as u64 > max {
        return Err(Error::new(
            "PRONUNCIATION_FILE_LIMIT",
            "File grew beyond its limit",
        ));
    }
    Ok(data)
}
