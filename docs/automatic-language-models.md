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
