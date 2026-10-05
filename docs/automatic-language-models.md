# Automatic language models

Verse uses bundled Lingua 1.8 data for French, English, Spanish and Portuguese.
Lingua is a statistical language detector, not a generative LLM. Verse first
reconstructs source-attested words, scores their local and passage context, then
resolves a language sequence. Pronunciation dictionaries are separate: a language
owner selects the appropriate native phonemizer and the exact supported hints.
Neither detector scores nor successful YAML parsing prove acoustic quality.

Low-confidence local islands also use a bounded wider source lyric row as context. If
the nearest confident complete words on both sides, within eight source words,
belong to the same language and the same part/staff, verse/lane, playback segment
and occurrence, Verse keeps that language across rests and source-voice bookkeeping
restarts. Confident switches remain untouched, and a low-confidence word that has
independent lexical/model evidence for its current language is not flattened by the
surrounding row. This keeps proper names and real single-word switches while a
monolingual track does not acquire a stray phonemizer.

## Historical candidate and measured decision

The initial investigation considered fastText `lid.176.ftz`, a compressed
176-language classifier. It was a candidate, not a previously integrated mini
LLM. The subsequent implementation specification selected Lingua.
[fastText's official model documentation](https://fasttext.cc/docs/en/language-identification.html)
describes its model sizes and training data.
[Lingua's implementation documentation](https://github.com/pemistahl/lingua-rs)
describes statistical language identification and language-specific build features.

A development-only comparison on 2026-09-15 tested the official fastText model
and native Rust Lingua 1.8 on self-authored monolingual/mixed passages and selected
private score contexts. Private lyrics remained local. The reproducible script,
model hash/license and raw outputs are retained locally under
`_bmad-output/implementation-artifacts/fasttext-benchmark-20260915/`.

Both models correctly classified the 26 sampled monolingual sentences when given
the whole sentence. On the same 195 tokens with centered five-word windows,
Lingua classified 192/195 correctly and fastText restricted to FR/EN/ES/PT
classified 184/195. On 86 mixed-language tokens the results were 78/86 and 79/86.
These raw window labels are not Verse's complete routed output. The samples and
overlapping windows are small and correlated, not population accuracy estimates
or a held-out model-selection benchmark.
fastText helps some names/context windows and worsens others; no general
replacement benefit is established. Scores from the models are not calibrated
probabilities and must not be compared as confidence guarantees.

The `une` failures were particularly informative: Lingua already classified
`ella une nuestras voces al amanecer` as Spanish and
`o vento une nossas vozes ao amanhecer` as Portuguese at sentence level. Verse's
function-word override was discarding that evidence. Correcting the override
and source-context ownership directly addresses the demonstrated defect.

The corrective release retains Lingua and fixes context handling rather than
adding a second runtime or claiming that a model replacement guarantees zero
errors. There are no runtime downloads, remote classification calls or generated
lyrics. Quality gates require the unchanged private development oracle and
self-authored boundary regressions; listening with compatible banks remains a
separate acoustic validation.
# Laya integration gate (2026-10-05)

Verse now has an optional Rust tokenizer/ONNX CPU adapter and bounded hybrid
evidence injection before the existing deterministic decoder. Only complete
source-attested weak words qualify; independent language evidence, manual hints,
source conflicts and trusted human corrections remain protected. The decoder,
source ownership and native pronunciation dictionaries retain their authority.

Laya is **not activated** in the shipped resource set. Canonical assets were
locally SHA-256 verified and an FP32 fused ONNX graph was prepared. A fixed
ordered question uses `fr: French pronunciation`, `en: English pronunciation`,
`es: Spanish pronunciation`, `pt: Portuguese pronunciation` exactly as pinned
`render_options`/`build_sequence` renders the Python mapping. The token parity
test compares both token IDs and marker positions, independently of logits.

The runtime requires confined regular files, exact sizes/hashes, adjacent ONNX
external data, fitted model/tokenizer/policy-bound calibration, a telemetry-free
CPU build, and hash-checked qualification/evidence files. Production additionally
requires redistribution, native parity and independent paired benefit. The
development evaluation entry point can measure a calibrated candidate before
that benefit gate has passed. No runtime download, Python service or remote
classifier is introduced.

Human correction memory is local SQLite under stable application user data.
Analysis freezes model/policy/memory identities and caches evidence; export uses
fresh registered cancellation/deadline state and verifies the reproduced plan.
Changed source, settings or active memory requires reanalysis. JSON imports are
pending until the user reviews before/after reading, source/context/voice scope
and explicitly confirms listening. Predictions and file differences never
become trusted memory automatically. Unqualified phone records remain inactive
for phone reuse; dictionary/native inventory legality and listening are distinct.
Correction provenance optionally retains the native `observed_singer` identifier
from the corrected track, including for unqualified aliases. It survives
confirmation, storage and contribution JSON export. This observation is separate
from `VoiceConstraint`: it supplies no inventory/configuration hashes and cannot
qualify bank-specific reuse. Older records omit it and retain their fingerprints.

The current local raw Laya comparison is development evidence, not a four-language
accuracy study. Both exposed song-family oracles, independent ES/PT gold,
reconstruction/phone gold, fitted calibration, telemetry-free native-package
parity and blinded acoustic listening must retain separate qualification status.
99% is an objective; no population, acoustic or universal hardware guarantee is
supported by these implementation gates.
