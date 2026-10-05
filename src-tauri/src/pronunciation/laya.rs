//! One fixed four-language question; phones and reconstruction are never outputs.
use super::{assets::Calibration, Error, Work};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub probabilities: [f64; 4],
    pub answer_confidence: f64,
    pub gate: String,
}

/// Python round(binary64, 4): round the exact binary rational times 10^4,
/// ties to even, without first introducing a binary multiplication-rounding
/// tie. Confidence is nonnegative and bounded by one.
pub fn round_answer_confidence(value: f64) -> f64 {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return value;
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0 {
        return 0.0;
    }
    let numerator = u128::from((bits & ((1u64 << 52) - 1)) | (1u64 << 52)) * 10_000;
    let shift = (1023 + 52 - exponent) as u32;
    if shift >= 128 {
        return 0.0;
    }
    let quotient = numerator >> shift;
    let remainder = numerator & ((1u128 << shift) - 1);
    let half = 1u128 << (shift - 1);
    let rounded =
        quotient + u128::from(remainder > half || (remainder == half && quotient & 1 == 1));
    rounded as f64 / 10_000.0
}

pub fn decode(logits: &[f32], calibration: &Calibration) -> Result<Evidence, Error> {
    if logits.len() != 4
        || logits.iter().any(|x| !x.is_finite())
        || !calibration.temperature.is_finite()
        || calibration.temperature <= 0.0
        || !calibration.fitted
        || !calibration.threshold.is_finite()
        || calibration.threshold <= 0.0
        || calibration.threshold > 1.0
    {
        return Err(Error::new(
            "LAYA_CONFIDENCE_INVALID",
            "Invalid four-language outputs or confidence policy",
        ));
    }
    // Pinned Agent/ONNXAgent decode NumPy float32 logits. Keep that scaling
    // and normalization precision before converting answer_confidence to f64.
    let temperature = calibration.temperature as f32;
    let scaled: Vec<f32> = logits.iter().map(|x| *x / temperature).collect();
    if !temperature.is_finite() || temperature <= 0.0 || scaled.iter().any(|x| !x.is_finite()) {
        return Err(Error::new(
            "LAYA_CONFIDENCE_INVALID",
            "Invalid float32 temperature-scaled logits",
        ));
    }
    let maximum = scaled.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut probabilities: Vec<f32> = scaled.iter().map(|x| (*x - maximum).exp()).collect();
    let sum: f32 = probabilities.iter().sum();
    for p in &mut probabilities {
        *p /= sum;
    }
    let probabilities: Vec<f64> = probabilities.into_iter().map(f64::from).collect();
    let answer_confidence =
        round_answer_confidence(probabilities.iter().copied().fold(0.0, f64::max));
    Ok(Evidence {
        probabilities: probabilities.try_into().unwrap(),
        answer_confidence,
        gate: if answer_confidence >= calibration.threshold {
            "passed"
        } else {
            "abstained"
        }
        .into(),
    })
}

impl Evidence {
    pub fn accepted(&self, threshold: f64) -> bool {
        self.gate == "passed"
            && threshold.is_finite()
            && threshold > 0.0
            && threshold <= 1.0
            && self.answer_confidence.is_finite()
            && self.answer_confidence >= threshold
            && self.answer_confidence <= 1.0
            && self
                .probabilities
                .iter()
                .all(|p| p.is_finite() && (0.0..=1.0).contains(p))
            && (self.probabilities.iter().sum::<f64>() - 1.0).abs() < 1e-6
            && round_answer_confidence(self.probabilities.iter().copied().fold(0.0, f64::max))
                == self.answer_confidence
    }
}

pub struct Laya {
    pub identity: String,
    pub calibration: Calibration,
    pub fusion_weight: f64,
    #[cfg(feature = "laya-native")]
    native: std::sync::Mutex<native::Runtime>,
}

impl Laya {
    pub fn load(root: &std::path::Path, target: &str) -> Result<Self, Error> {
        Self::load_mode(root, target, true)
    }
    /// Development-only candidate evaluation does not presuppose benefit.
    /// Calibration, assets, rights receipts and telemetry-free native identity
    /// still have to qualify before any native library is initialized.
    pub fn load_for_evaluation(
        root: &std::path::Path,
        target: &str,
        manifest_sha256: &str,
    ) -> Result<Self, Error> {
        if super::hash(&super::assets::read_bounded(
            &root.join("manifest.json"),
            64 * 1024,
        )?) != manifest_sha256
        {
            return Err(Error::new(
                "LAYA_ASSET_IDENTITY",
                "Frozen evaluation manifest differs",
            ));
        }
        Self::load_mode(root, target, false)
    }
    fn load_mode(root: &std::path::Path, target: &str, activation: bool) -> Result<Self, Error> {
        #[cfg(feature = "laya-native")]
        {
            let manifest = super::assets::validate(root, target, activation)?;
            let identity = super::hash(&super::assets::read_bounded(
                &root.join("manifest.json"),
                64 * 1024,
            )?);
            Ok(Self {
                identity,
                calibration: manifest.calibration.clone(),
                fusion_weight: manifest.fusion_weight,
                native: std::sync::Mutex::new(native::Runtime::load(root, manifest)?),
            })
        }
        #[cfg(not(feature = "laya-native"))]
        {
            let _ = (root, target, activation);
            Err(Error::new(
                "LAYA_RUNTIME_NOT_PACKAGED",
                "Native Laya support is not packaged in this build",
            ))
        }
    }
    pub fn predict(&self, contexts: &[String], work: &Work) -> Result<Vec<Evidence>, Error> {
        work.check()?;
        #[cfg(feature = "laya-native")]
        {
            let mut native = self
                .native
                .try_lock()
                .map_err(|_| Error::new("LAYA_BUSY", "CPU session is already in use"))?;
            native
                .predict(contexts, work)?
                .iter()
                .map(|row| decode(row, &self.calibration))
                .collect()
        }
        #[cfg(not(feature = "laya-native"))]
        {
            let _ = contexts;
            Err(Error::new(
                "LAYA_RUNTIME_NOT_PACKAGED",
                "Native Laya support is unavailable",
            ))
        }
    }
}

#[cfg(feature = "laya-native")]
pub mod native {
    use super::super::{
        assets::{confined, Manifest},
        OPTIONS, PROMPT,
    };
    use super::*;
    use ort::{
        session::{RunOptions, Session},
        value::Tensor,
    };
    use std::{
        path::Path,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    use tokenizers::Tokenizer;

    pub(super) struct Runtime {
        session: Session,
        tokenizer: Tokenizer,
        manifest: Manifest,
    }
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Encoded {
        pub input_ids: Vec<i64>,
        pub marker_pos: Vec<i64>,
    }
    fn runtime_error(e: impl std::fmt::Display) -> Error {
        Error::new("LAYA_RUNTIME_FAILED", &e.to_string())
    }

    pub fn encode(tokenizer: &Tokenizer, state: &str) -> Result<Encoded, Error> {
        if state.len() > 32 * 1024 {
            return Err(Error::new(
                "LAYA_TRUNCATED",
                "Context exceeds the text budget",
            ));
        }
        for (token, id) in [
            ("<pad>", 0),
            ("<bos>", 2),
            ("<eos>", 1),
            ("<mask>", 4),
            ("<unk>", 3),
        ] {
            if tokenizer.token_to_id(token) != Some(id) {
                return Err(Error::new(
                    "LAYA_TOKENIZER_CONTRACT",
                    "Special token identities differ",
                ));
            }
        }
        let tokens = |text: &str| -> Result<Vec<i64>, Error> {
            Ok(tokenizer
                .encode(text.replace("<mask>", " "), false)
                .map_err(runtime_error)?
                .get_ids()
                .iter()
                .map(|&x| i64::from(x))
                .collect())
        };
        let instruction = tokens(&format!("choice question: {PROMPT}"))?;
        let options: Vec<_> = OPTIONS
            .iter()
            .map(|o| tokens(&format!(" {o}")))
            .collect::<Result<_, _>>()?;
        if options.iter().any(|o| o.is_empty() || o.len() > 48)
            || options
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != 4
            || instruction.len() + options.iter().map(|o| o.len() + 1).sum::<usize>() > 256
        {
            return Err(Error::new(
                "LAYA_TRUNCATED",
                "Question/options truncated or indistinguishable",
            ));
        }
        let mut input_ids = vec![2];
        input_ids.extend(instruction);
        input_ids.push(1);
        let mut marker_pos = Vec::new();
        for option in options {
            marker_pos.push(input_ids.len() as i64);
            input_ids.push(4);
            input_ids.extend(option);
        }
        input_ids.push(1);
        input_ids.extend(tokens(state)?);
        input_ids.push(1);
        if input_ids.len() > 1024 {
            return Err(Error::new(
                "LAYA_TRUNCATED",
                "Context token truncation is refused",
            ));
        }
        Ok(Encoded {
            input_ids,
            marker_pos,
        })
    }

    impl Runtime {
        pub(super) fn load(root: &Path, manifest: Manifest) -> Result<Self, Error> {
            // Manifest qualification is required before calling ORT at all.
            if !manifest.telemetry_free_build {
                return Err(Error::new(
                    "LAYA_RUNTIME_UNQUALIFIED",
                    "Telemetry-free source build required",
                ));
            }
            let runtime_path = confined(root, &manifest.runtime_path)?;
            // ORT's environment is process-global: refuse a different library identity.
            static RUNTIME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let digest = manifest
                .files
                .iter()
                .find(|a| a.path == manifest.runtime_path)
                .ok_or_else(|| Error::new("LAYA_ASSET_MISSING", "Runtime identity missing"))?
                .sha256
                .clone();
            let active = RUNTIME.get_or_init(|| digest.clone());
            if active != &digest {
                return Err(Error::new(
                    "LAYA_RUNTIME_IDENTITY",
                    "Another native runtime is already initialized",
                ));
            }
            ort::init_from(runtime_path)
                .map_err(runtime_error)?
                .with_telemetry(false)
                .commit();
            let session = Session::builder()
                .map_err(runtime_error)?
                .with_intra_threads(manifest.threads)
                .map_err(runtime_error)?
                .with_inter_threads(1)
                .map_err(runtime_error)?
                .with_execution_providers([ort::ep::CPU::default().build()])
                .map_err(runtime_error)?
                .commit_from_file(confined(root, "model.onnx")?)
                .map_err(runtime_error)?;
            use ort::value::{TensorElementType, ValueType};
            let inputs = [
                ("input_ids", TensorElementType::Int64, 2),
                ("attention_mask", TensorElementType::Int64, 2),
                ("marker_pos", TensorElementType::Int64, 2),
                ("marker_mask", TensorElementType::Bool, 2),
                ("qtype", TensorElementType::Int64, 1),
            ];
            if session.inputs().len()!=inputs.len() || inputs.iter().any(|(name,element,rank)| {
                session.inputs().iter().find(|i|i.name()==*name).is_none_or(|i|!matches!(i.dtype(),ValueType::Tensor{ty,shape,..} if ty==element && shape.len()==*rank))
            }) {return Err(Error::new("LAYA_TENSOR_ABI","Native graph input names, ranks or tensor types differ"));}
            for name in ["logits", "act_logits"] {
                if session.outputs().iter().find(|o|o.name()==name).is_none_or(|o|!matches!(o.dtype(),ValueType::Tensor{ty:TensorElementType::Float32,shape,..} if shape.len()==2)){return Err(Error::new("LAYA_TENSOR_ABI","Native graph output ABI differs"));}
            }
            let mut tokenizer = Tokenizer::from_file(confined(root, "tokenizer/tokenizer.json")?)
                .map_err(runtime_error)?;
            tokenizer.with_truncation(None).map_err(runtime_error)?;
            tokenizer.with_padding(None);
            Ok(Self {
                session,
                tokenizer,
                manifest,
            })
        }
        pub(super) fn predict(
            &mut self,
            contexts: &[String],
            work: &Work,
        ) -> Result<Vec<Vec<f32>>, Error> {
            work.check()?;
            if contexts.is_empty() || contexts.len() > self.manifest.max_rows {
                return Err(Error::new("LAYA_ROW_LIMIT", "Invalid bounded batch size"));
            }
            let rows: Vec<_> = contexts
                .iter()
                .map(|s| encode(&self.tokenizer, s))
                .collect::<Result<_, _>>()?;
            let b = rows.len();
            let length = rows.iter().map(|r| r.input_ids.len()).max().unwrap();
            let mut ids = vec![0i64; b * length];
            let mut attention = vec![0i64; b * length];
            let mut markers = vec![0i64; b * 4];
            for (i, row) in rows.iter().enumerate() {
                ids[i * length..i * length + row.input_ids.len()].copy_from_slice(&row.input_ids);
                attention[i * length..i * length + row.input_ids.len()].fill(1);
                markers[i * 4..i * 4 + 4].copy_from_slice(&row.marker_pos);
            }
            let options = Arc::new(RunOptions::new().map_err(runtime_error)?);
            let done = Arc::new(AtomicBool::new(false));
            let timer_options = options.clone();
            let timer_done = done.clone();
            let cancellation = work.cancelled.clone();
            let deadline = work
                .deadline
                .min(Instant::now() + Duration::from_millis(self.manifest.deadline_ms));
            let timer = std::thread::spawn(move || {
                while !timer_done.load(Ordering::Acquire) {
                    if cancellation.load(Ordering::Relaxed) || Instant::now() >= deadline {
                        let _ = timer_options.terminate();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
            let result = (|| {
                let outputs=self.session.run_with_options(ort::inputs![
                    "input_ids"=>Tensor::from_array(([b,length],ids)).map_err(runtime_error)?,
                    "attention_mask"=>Tensor::from_array(([b,length],attention)).map_err(runtime_error)?,
                    "marker_pos"=>Tensor::from_array(([b,4],markers)).map_err(runtime_error)?,
                    "marker_mask"=>Tensor::from_array(([b,4],vec![true;b*4])).map_err(runtime_error)?,
                    "qtype"=>Tensor::from_array(([b],vec![0i64;b])).map_err(runtime_error)?
                ], &options).map_err(runtime_error)?;
                let output = outputs
                    .get("logits")
                    .ok_or_else(|| Error::new("LAYA_OUTPUT_CONTRACT", "Missing logits"))?;
                let (shape, values) = output.try_extract_tensor::<f32>().map_err(runtime_error)?;
                if shape.as_ref() != [b as i64, 4] || values.iter().any(|x| !x.is_finite()) {
                    return Err(Error::new(
                        "LAYA_OUTPUT_CONTRACT",
                        "Invalid logits shape or values",
                    ));
                }
                let (action_shape, action_values) = outputs
                    .get("act_logits")
                    .ok_or_else(|| Error::new("LAYA_OUTPUT_CONTRACT", "Missing action logits"))?
                    .try_extract_tensor::<f32>()
                    .map_err(runtime_error)?;
                if action_shape.as_ref() != [b as i64, 2]
                    || action_values.iter().any(|v| !v.is_finite())
                {
                    return Err(Error::new(
                        "LAYA_OUTPUT_CONTRACT",
                        "Invalid action output shape or values",
                    ));
                }
                Ok(values.chunks_exact(4).map(|r| r.to_vec()).collect())
            })();
            done.store(true, Ordering::Release);
            let _ = timer.join();
            work.check()?;
            if Instant::now() >= deadline {
                return Err(Error::new(
                    "LAYA_DEADLINE",
                    "CPU inference deadline exceeded",
                ));
            }
            result
        }
    }
}
