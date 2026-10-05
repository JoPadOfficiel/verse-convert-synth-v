//! Closed local asset contract. No network, cache discovery or repair path.
use super::{hash, Error, POLICY};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub model_sha256: String,
    pub external_data_sha256: String,
    pub tokenizer_sha256: String,
    pub policy: String,
    pub bucket: String,
    pub temperature: f64,
    pub threshold: f64,
    pub fitted: bool,
    pub calibration_families_sha256: String,
}

impl Calibration {
    pub fn validate(&self, model: &str, external_data: &str, tokenizer: &str) -> Result<(), Error> {
        if !self.fitted
            || self.model_sha256 != model
            || self.external_data_sha256 != external_data
            || self.tokenizer_sha256 != tokenizer
            || self.policy != POLICY
            || self.bucket != "choice:3-5"
            || !self.temperature.is_finite()
            || self.temperature <= 0.0
            || !self.threshold.is_finite()
            || !(0.0..=1.0).contains(&self.threshold)
            || self.threshold == 0.0
            || !valid_hash(&self.calibration_families_sha256)
        {
            return Err(Error::new(
                "LAYA_CALIBRATION_UNQUALIFIED",
                "Matching fitted four-language calibration is required",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub policy: String,
    pub target: String,
    pub runtime_path: String,
    pub files: Vec<Asset>,
    pub calibration: Calibration,
    pub redistribution_qualified: bool,
    pub telemetry_free_build: bool,
    pub native_parity_qualified: bool,
    pub independent_benefit_qualified: bool,
    pub qualification_receipt_sha256: Option<String>,
    pub fusion_weight: f64,
    pub max_tokens: usize,
    pub max_rows: usize,
    pub threads: usize,
    pub deadline_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Qualification {
    pub schema_version: u32,
    pub policy: String,
    pub target: String,
    pub graph_sha256: String,
    pub external_data_sha256: String,
    pub tokenizer_sha256: String,
    pub calibration_sha256: String,
    pub runtime_sha256: String,
    pub evidence: Vec<Asset>,
    pub rights_evidence: String,
    pub offline_parity_evidence: String,
    pub independent_evaluation_evidence: Option<String>,
    pub resource_evidence: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RoutingConfig {
    fusion_weight: f64,
    max_tokens: usize,
    max_rows: usize,
    threads: usize,
    deadline_ms: u64,
    context_radius: usize,
    routing_policy: String,
    prompt_sha256: String,
    options_sha256: String,
}
impl RoutingConfig {
    fn actual(manifest: &Manifest) -> Self {
        Self {
            fusion_weight: manifest.fusion_weight,
            max_tokens: manifest.max_tokens,
            max_rows: manifest.max_rows,
            threads: manifest.threads,
            deadline_ms: manifest.deadline_ms,
            context_radius: 2,
            routing_policy: "verse-weak-complete-context-v1".into(),
            prompt_sha256: hash(super::PROMPT.as_bytes()),
            options_sha256: hash(&serde_json::to_vec(&super::OPTIONS).unwrap()),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FamilySet {
    schema_version: u32,
    families: Vec<String>,
}
fn family_set(
    root: &Path,
    receipt: &Qualification,
    path: &str,
    expected: &str,
) -> Result<BTreeSet<String>, Error> {
    let asset = receipt
        .evidence
        .iter()
        .find(|a| a.path == path && a.sha256 == expected)
        .ok_or_else(|| {
            Error::new(
                "LAYA_QUALIFICATION_INVALID",
                "Retained family set identity missing",
            )
        })?;
    verify_identity(root, asset)?;
    let value: FamilySet =
        serde_json::from_slice(&read_bounded(&confined(root, path)?, 16 * 1024 * 1024)?)?;
    let families: BTreeSet<_> = value.families.iter().cloned().collect();
    if value.schema_version != 1
        || families.is_empty()
        || families.len() != value.families.len()
        || families.len() > 10000
        || families
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 256)
    {
        return Err(Error::new(
            "LAYA_QUALIFICATION_INVALID",
            "Invalid or duplicate family identifiers",
        ));
    }
    Ok(families)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GateEvidence {
    schema_version: u32,
    policy: String,
    target: String,
    graph_sha256: String,
    external_data_sha256: String,
    tokenizer_sha256: String,
    calibration_sha256: String,
    runtime_sha256: String,
    observation: Observation,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Observation {
    Rights {
        checkpoint_license_sha256: String,
        tokenizer_permission_sha256: String,
        review_sha256: String,
    },
    OfflineParity {
        token_cases: u64,
        shape_cases: u64,
        max_logit_error: f64,
        max_action_logit_error: f64,
        network_attempts: u64,
        missing_corrupt_cases: u64,
        immutable_assets: bool,
    },
    Resources {
        peak_rss_bytes: u64,
        package_bytes: u64,
        cold_seconds: f64,
        warm_p95_seconds: f64,
        hardware: String,
        threads: usize,
    },
    IndependentBenefit {
        routing: RoutingConfig,
        test_families_file: String,
        calibration_families_file: String,
        test_families: u64,
        test_manifest_sha256: String,
        calibration_manifest_sha256: String,
        paired_lower_95: f64,
        baseline_macro_accuracy: f64,
        candidate_macro_accuracy: f64,
        subgroup_regressions: u64,
        source_violations: u64,
        illegal_phone_violations: u64,
    },
}

fn qualify_evidence(
    root: &Path,
    receipt: &Qualification,
    filename: &str,
    kind: &str,
    manifest: &Manifest,
) -> Result<(), Error> {
    let evidence: GateEvidence =
        serde_json::from_slice(&read_bounded(&confined(root, filename)?, 64 * 1024)?)?;
    if evidence.schema_version != 1
        || evidence.policy != receipt.policy
        || evidence.target != receipt.target
        || evidence.graph_sha256 != receipt.graph_sha256
        || evidence.external_data_sha256 != receipt.external_data_sha256
        || evidence.tokenizer_sha256 != receipt.tokenizer_sha256
        || evidence.calibration_sha256 != receipt.calibration_sha256
        || evidence.runtime_sha256 != receipt.runtime_sha256
    {
        return Err(Error::new(
            "LAYA_QUALIFICATION_INVALID",
            "Gate observations belong to another asset/policy/runtime set",
        ));
    }
    let qualified = match (kind, evidence.observation) {
        (
            "rights",
            Observation::Rights {
                checkpoint_license_sha256,
                tokenizer_permission_sha256,
                review_sha256,
            },
        ) => [
            checkpoint_license_sha256,
            tokenizer_permission_sha256,
            review_sha256,
        ]
        .iter()
        .all(|h| valid_hash(h)),
        (
            "parity",
            Observation::OfflineParity {
                token_cases,
                shape_cases,
                max_logit_error,
                max_action_logit_error,
                network_attempts,
                missing_corrupt_cases,
                immutable_assets,
            },
        ) => {
            token_cases > 0
                && shape_cases >= 4
                && max_logit_error.is_finite()
                && (0.0..=0.001).contains(&max_logit_error)
                && max_action_logit_error.is_finite()
                && (0.0..=0.01).contains(&max_action_logit_error)
                && network_attempts == 0
                && missing_corrupt_cases >= 7
                && immutable_assets
        }
        (
            "resources",
            Observation::Resources {
                peak_rss_bytes,
                package_bytes,
                cold_seconds,
                warm_p95_seconds,
                hardware,
                threads,
            },
        ) => {
            peak_rss_bytes > 0
                && package_bytes > 0
                && cold_seconds.is_finite()
                && cold_seconds > 0.0
                && warm_p95_seconds.is_finite()
                && warm_p95_seconds > 0.0
                && !hardware.is_empty()
                && (1..=2).contains(&threads)
        }
        (
            "benefit",
            Observation::IndependentBenefit {
                routing,
                test_families_file,
                calibration_families_file,
                test_families,
                test_manifest_sha256,
                calibration_manifest_sha256,
                paired_lower_95,
                baseline_macro_accuracy,
                candidate_macro_accuracy,
                subgroup_regressions,
                source_violations,
                illegal_phone_violations,
            },
        ) => {
            let test = family_set(root, receipt, &test_families_file, &test_manifest_sha256)?;
            let calibration = family_set(
                root,
                receipt,
                &calibration_families_file,
                &calibration_manifest_sha256,
            )?;
            routing == RoutingConfig::actual(manifest)
                && calibration_manifest_sha256 == manifest.calibration.calibration_families_sha256
                && test.is_disjoint(&calibration)
                && test.len() as u64 == test_families
                && test_families > 1
                && valid_hash(&test_manifest_sha256)
                && valid_hash(&calibration_manifest_sha256)
                && test_manifest_sha256 != calibration_manifest_sha256
                && paired_lower_95.is_finite()
                && paired_lower_95 > 0.0
                && baseline_macro_accuracy.is_finite()
                && (0.0..=1.0).contains(&baseline_macro_accuracy)
                && candidate_macro_accuracy.is_finite()
                && (0.0..=1.0).contains(&candidate_macro_accuracy)
                && candidate_macro_accuracy > baseline_macro_accuracy
                && subgroup_regressions == 0
                && source_violations == 0
                && illegal_phone_violations == 0
        }
        _ => false,
    };
    if !qualified {
        return Err(Error::new(
            "LAYA_QUALIFICATION_UNPASSED",
            "Gate observations do not qualify this asset set",
        ));
    }
    Ok(())
}

fn verify_identity(root: &Path, asset: &Asset) -> Result<(), Error> {
    if !valid_hash(&asset.sha256) || asset.bytes == 0 || asset.bytes > 16 * 1024 * 1024 {
        return Err(Error::new(
            "LAYA_QUALIFICATION_INVALID",
            "Invalid qualification evidence identity",
        ));
    }
    let bytes = read_bounded(&confined(root, &asset.path)?, asset.bytes)?;
    if bytes.len() as u64 != asset.bytes || hash(&bytes) != asset.sha256 {
        return Err(Error::new(
            "LAYA_QUALIFICATION_INVALID",
            "Qualification evidence bytes differ",
        ));
    }
    Ok(())
}

// Minimal bounded ONNX protobuf inspection. All tensor-bearing graph/attribute,
// function, sparse and training branches are visited, not just initializers.
// Field numbers follow ONNX's pinned onnx.proto. No ORT code runs here.
pub fn external_paths(bytes: &[u8]) -> Result<BTreeSet<String>, Error> {
    fn invalid() -> Error {
        Error::new(
            "LAYA_GRAPH_EXTERNAL_DATA",
            "Malformed ONNX or unexpected external tensor data",
        )
    }
    fn varint(data: &[u8], at: &mut usize) -> Result<u64, Error> {
        let mut value = 0u64;
        for shift in (0..70).step_by(7) {
            let byte = *data.get(*at).ok_or_else(invalid)?;
            *at += 1;
            if shift == 63 && byte > 1 {
                return Err(invalid());
            }
            value |= u64::from(byte & 127) << shift;
            if byte & 128 == 0 {
                return Ok(value);
            }
        }
        Err(invalid())
    }
    fn walk(
        data: &[u8],
        kind: u8,
        depth: usize,
        work: &mut usize,
        paths: &mut BTreeSet<String>,
    ) -> Result<(), Error> {
        if depth > 32 {
            return Err(invalid());
        }
        let mut at = 0;
        let mut location = None;
        let mut external = false;
        while at < data.len() {
            *work += 1;
            if *work > 2_000_000 {
                return Err(invalid());
            }
            let tag = varint(data, &mut at)?;
            let number = tag >> 3;
            let wire = tag & 7;
            if number == 0 {
                return Err(invalid());
            }
            let mut raw = None;
            match wire {
                0 => {
                    let value = varint(data, &mut at)?;
                    if kind == 4 && number == 14 {
                        if value > 1 {
                            return Err(invalid());
                        }
                        external = value == 1;
                    }
                }
                1 => {
                    at = at
                        .checked_add(8)
                        .filter(|&v| v <= data.len())
                        .ok_or_else(invalid)?
                }
                2 => {
                    let length = usize::try_from(varint(data, &mut at)?).map_err(|_| invalid())?;
                    let end = at
                        .checked_add(length)
                        .filter(|&v| v <= data.len())
                        .ok_or_else(invalid)?;
                    raw = Some(&data[at..end]);
                    at = end;
                }
                5 => {
                    at = at
                        .checked_add(4)
                        .filter(|&v| v <= data.len())
                        .ok_or_else(invalid)?
                }
                _ => return Err(invalid()),
            }
            if let Some(raw) = raw {
                let child = match (kind, number) {
                    (0, 7) => Some(1),
                    (0, 20) => Some(6),
                    (0, 25) => Some(7),
                    (1, 1) | (7, 7) => Some(2),
                    (1, 5) => Some(4),
                    (1, 15) => Some(5),
                    (2, 5) | (7, 11) => Some(3),
                    (3, 5) | (3, 10) | (5, 1) | (5, 2) => Some(4),
                    (3, 6) | (3, 11) | (6, 1) | (6, 2) => Some(1),
                    (3, 22) | (3, 23) => Some(5),
                    (4, 13) => Some(8),
                    _ => None,
                };
                if let Some(child) = child {
                    if child == 8 {
                        // Parse StringStringEntryProto independently: only a
                        // location entry becomes a path; offsets remain numeric.
                        let mut pos = 0;
                        let mut key = None;
                        let mut value = None;
                        while pos < raw.len() {
                            let t = varint(raw, &mut pos)?;
                            if t & 7 != 2 {
                                return Err(invalid());
                            }
                            let n =
                                usize::try_from(varint(raw, &mut pos)?).map_err(|_| invalid())?;
                            let end = pos
                                .checked_add(n)
                                .filter(|&v| v <= raw.len())
                                .ok_or_else(invalid)?;
                            let text = std::str::from_utf8(&raw[pos..end])
                                .map_err(|_| invalid())?
                                .to_string();
                            pos = end;
                            match t >> 3 {
                                1 => {
                                    if key.replace(text).is_some() {
                                        return Err(invalid());
                                    }
                                }
                                2 => {
                                    if value.replace(text).is_some() {
                                        return Err(invalid());
                                    }
                                }
                                _ => return Err(invalid()),
                            }
                        }
                        let key = key.ok_or_else(invalid)?;
                        let value = value.ok_or_else(invalid)?;
                        if key == "location" {
                            if location.replace(value).is_some() {
                                return Err(invalid());
                            }
                        } else if !["offset", "length", "checksum"].contains(&key.as_str()) {
                            return Err(invalid());
                        }
                    } else {
                        walk(raw, child, depth + 1, work, paths)?;
                    }
                }
            }
        }
        if kind == 4 {
            match (external, location) {
                (true, Some(path)) if path == "model.onnx.data" => {
                    paths.insert(path);
                }
                (false, None) => {}
                _ => return Err(invalid()),
            }
        }
        Ok(())
    }
    let mut paths = BTreeSet::new();
    walk(bytes, 0, 0, &mut 0, &mut paths)?;
    Ok(paths)
}

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

pub fn confined(root: &Path, relative: &str) -> Result<PathBuf, Error> {
    if relative.is_empty()
        || relative.contains('\\')
        || Path::new(relative)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::new(
            "LAYA_ASSET_PATH",
            "Asset path must stay inside the local asset directory",
        ));
    }
    let mut path = root.to_path_buf();
    for part in Path::new(relative).components() {
        path.push(part);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(Error::new("LAYA_ASSET_PATH", "Asset symlinks are refused"));
        }
    }
    Ok(path)
}

pub fn validate(root: &Path, target: &str, activation: bool) -> Result<Manifest, Error> {
    let data = read_bounded(&root.join("manifest.json"), 64 * 1024)?;
    let manifest: Manifest = serde_json::from_slice(&data)?;
    if manifest.schema_version != 1
        || manifest.policy != POLICY
        || manifest.target != target
        || ![0.5, 1.0, 2.0].contains(&manifest.fusion_weight)
        || manifest.max_tokens != 1024
        || !(1..=8).contains(&manifest.max_rows)
        || !(1..=2).contains(&manifest.threads)
        || !(1..=30_000).contains(&manifest.deadline_ms)
        || manifest.files.len() > 24
    {
        return Err(Error::new(
            "LAYA_ASSET_CONTRACT",
            "Unsupported model, architecture or resource policy",
        ));
    }
    let mut paths = BTreeSet::new();
    for asset in &manifest.files {
        if !paths.insert(asset.path.as_str())
            || !valid_hash(&asset.sha256)
            || asset.bytes == 0
            || asset.bytes > 2 * 1024 * 1024 * 1024
        {
            return Err(Error::new(
                "LAYA_ASSET_CONTRACT",
                "Invalid or duplicate asset identity",
            ));
        }
        let path = confined(root, &asset.path)?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() || metadata.len() != asset.bytes {
            return Err(Error::new(
                "LAYA_ASSET_INTEGRITY",
                "Asset byte count differs",
            ));
        }
        use sha2::{Digest, Sha256};
        let mut reader = fs::File::open(path)?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            digest.update(&buffer[..n]);
        }
        if format!("{:x}", digest.finalize()) != asset.sha256 {
            return Err(Error::new("LAYA_ASSET_INTEGRITY", "Asset SHA-256 differs"));
        }
    }
    for required in [
        "model.onnx",
        "model.onnx.data",
        "tokenizer/tokenizer.json",
        "tokenizer/tokenizer_config.json",
        "rl_agent_config.json",
        "calibration.json",
        manifest.runtime_path.as_str(),
    ] {
        if !paths.contains(required) {
            return Err(Error::new(
                "LAYA_ASSET_MISSING",
                "A mandatory local asset is missing",
            ));
        }
    }
    let sha = |name: &str| {
        manifest
            .files
            .iter()
            .find(|a| a.path == name)
            .unwrap()
            .sha256
            .as_str()
    };
    let calibration: Calibration = serde_json::from_slice(&read_bounded(
        &confined(root, "calibration.json")?,
        64 * 1024,
    )?)?;
    if hash(&serde_json::to_vec(&calibration)?) != hash(&serde_json::to_vec(&manifest.calibration)?)
    {
        return Err(Error::new(
            "LAYA_CALIBRATION_UNQUALIFIED",
            "Calibration manifest differs from its file",
        ));
    }
    calibration.validate(
        sha("model.onnx"),
        sha("model.onnx.data"),
        sha("tokenizer/tokenizer.json"),
    )?;
    if !manifest.telemetry_free_build {
        return Err(Error::new(
            "LAYA_RUNTIME_UNQUALIFIED",
            "A telemetry-free CPU build is required before native initialization",
        ));
    }
    let graph = read_bounded(&confined(root, "model.onnx")?, 16 * 1024 * 1024)?;
    if external_paths(&graph)? != BTreeSet::from(["model.onnx.data".to_string()]) {
        return Err(Error::new(
            "LAYA_GRAPH_EXTERNAL_DATA",
            "Expected the hash-bound adjacent tensor sidecar only",
        ));
    }
    let receipt_hash = manifest
        .qualification_receipt_sha256
        .as_deref()
        .ok_or_else(|| {
            Error::new(
                "LAYA_QUALIFICATION_MISSING",
                "Identity-bound qualification receipt is required",
            )
        })?;
    let receipt_asset = manifest
        .files
        .iter()
        .find(|a| a.path == "qualification.json")
        .ok_or_else(|| {
            Error::new(
                "LAYA_QUALIFICATION_MISSING",
                "Qualification receipt file is missing from the manifest",
            )
        })?;
    if receipt_asset.sha256 != receipt_hash {
        return Err(Error::new(
            "LAYA_QUALIFICATION_INVALID",
            "Qualification receipt hash differs",
        ));
    }
    let receipt: Qualification = serde_json::from_slice(&read_bounded(
        &confined(root, "qualification.json")?,
        64 * 1024,
    )?)?;
    if receipt.schema_version != 1
        || receipt.policy != POLICY
        || receipt.target != target
        || receipt.graph_sha256 != sha("model.onnx")
        || receipt.external_data_sha256 != sha("model.onnx.data")
        || receipt.tokenizer_sha256 != sha("tokenizer/tokenizer.json")
        || receipt.calibration_sha256 != sha("calibration.json")
        || receipt.runtime_sha256 != sha(&manifest.runtime_path)
        || receipt.evidence.len() > 12
    {
        return Err(Error::new(
            "LAYA_QUALIFICATION_INVALID",
            "Qualification receipt is not bound to this exact asset set",
        ));
    }
    for evidence in &receipt.evidence {
        verify_identity(root, evidence)?;
    }
    for required in [
        &receipt.rights_evidence,
        &receipt.offline_parity_evidence,
        &receipt.resource_evidence,
    ] {
        if !receipt.evidence.iter().any(|e| &e.path == required) {
            return Err(Error::new(
                "LAYA_QUALIFICATION_MISSING",
                "A required qualification evidence file is absent",
            ));
        }
    }
    if activation
        && !receipt
            .independent_evaluation_evidence
            .as_ref()
            .is_some_and(|required| receipt.evidence.iter().any(|e| &e.path == required))
    {
        return Err(Error::new(
            "LAYA_QUALIFICATION_MISSING",
            "Independent benefit evidence is required for activation",
        ));
    }
    qualify_evidence(
        root,
        &receipt,
        &receipt.rights_evidence,
        "rights",
        &manifest,
    )?;
    qualify_evidence(
        root,
        &receipt,
        &receipt.offline_parity_evidence,
        "parity",
        &manifest,
    )?;
    qualify_evidence(
        root,
        &receipt,
        &receipt.resource_evidence,
        "resources",
        &manifest,
    )?;
    if activation {
        qualify_evidence(
            root,
            &receipt,
            receipt.independent_evaluation_evidence.as_deref().unwrap(),
            "benefit",
            &manifest,
        )?;
    }
    if activation
        && (!manifest.redistribution_qualified
            || !manifest.native_parity_qualified
            || !manifest.independent_benefit_qualified
            || !manifest
                .qualification_receipt_sha256
                .as_deref()
                .is_some_and(valid_hash))
    {
        return Err(Error::new(
            "LAYA_ACTIVATION_UNQUALIFIED",
            "Redistribution, native parity and independent paired benefit are unqualified",
        ));
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn benefit_binds_exact_fusion_and_retained_disjoint_families() {
        let root =
            std::env::temp_dir().join(format!("verse-benefit-binding-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let asset = |name: &str, value: serde_json::Value| {
            let bytes = serde_json::to_vec(&value).unwrap();
            fs::write(root.join(name), &bytes).unwrap();
            Asset {
                path: name.into(),
                bytes: bytes.len() as u64,
                sha256: hash(&bytes),
            }
        };
        let test = asset(
            "test-families.json",
            serde_json::json!({"schema_version":1,"families":["test-a","test-b"]}),
        );
        let calibration = asset(
            "calibration-families.json",
            serde_json::json!({"schema_version":1,"families":["calibration-a"]}),
        );
        let mut manifest:Manifest=serde_json::from_value(serde_json::json!({"schema_version":1,"policy":POLICY,"target":"test-cpu","runtime_path":"native","files":[],
            "calibration":{"model_sha256":"a".repeat(64),"external_data_sha256":"b".repeat(64),"tokenizer_sha256":"c".repeat(64),"policy":POLICY,"bucket":"choice:3-5","temperature":1.0,"threshold":0.9,"fitted":true,"calibration_families_sha256":calibration.sha256},
            "redistribution_qualified":false,"telemetry_free_build":false,"native_parity_qualified":false,"independent_benefit_qualified":false,"qualification_receipt_sha256":null,
            "fusion_weight":1.0,"max_tokens":1024,"max_rows":2,"threads":2,"deadline_ms":30000})).unwrap();
        let mut receipt = Qualification {
            schema_version: 1,
            policy: POLICY.into(),
            target: "test-cpu".into(),
            graph_sha256: "a".repeat(64),
            external_data_sha256: "b".repeat(64),
            tokenizer_sha256: "c".repeat(64),
            calibration_sha256: "d".repeat(64),
            runtime_sha256: "e".repeat(64),
            evidence: vec![test.clone(), calibration.clone()],
            rights_evidence: "".into(),
            offline_parity_evidence: "".into(),
            independent_evaluation_evidence: None,
            resource_evidence: "".into(),
        };
        // Contract-only synthetic observations: no runtime or actual qualification.
        let mut evidence = serde_json::json!({"schema_version":1,"policy":POLICY,"target":"test-cpu","graph_sha256":receipt.graph_sha256,"external_data_sha256":receipt.external_data_sha256,"tokenizer_sha256":receipt.tokenizer_sha256,"calibration_sha256":receipt.calibration_sha256,"runtime_sha256":receipt.runtime_sha256,
            "observation":{"kind":"independent_benefit","routing":RoutingConfig::actual(&manifest),"test_families_file":test.path,"calibration_families_file":calibration.path,"test_families":2,"test_manifest_sha256":test.sha256,"calibration_manifest_sha256":calibration.sha256,"paired_lower_95":0.01,"baseline_macro_accuracy":0.95,"candidate_macro_accuracy":0.99,"subgroup_regressions":0,"source_violations":0,"illegal_phone_violations":0}});
        fs::write(
            root.join("benefit.json"),
            serde_json::to_vec(&evidence).unwrap(),
        )
        .unwrap();
        qualify_evidence(&root, &receipt, "benefit.json", "benefit", &manifest).unwrap();
        manifest.fusion_weight = 2.0;
        assert!(qualify_evidence(&root, &receipt, "benefit.json", "benefit", &manifest).is_err());
        manifest.fusion_weight = 1.0;
        let overlap = asset(
            "calibration-families.json",
            serde_json::json!({"schema_version":1,"families":["test-a"]}),
        );
        receipt.evidence[1] = overlap.clone();
        manifest.calibration.calibration_families_sha256 = overlap.sha256.clone();
        evidence["observation"]["calibration_manifest_sha256"] = overlap.sha256.into();
        fs::write(
            root.join("benefit.json"),
            serde_json::to_vec(&evidence).unwrap(),
        )
        .unwrap();
        assert!(qualify_evidence(&root, &receipt, "benefit.json", "benefit", &manifest).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
