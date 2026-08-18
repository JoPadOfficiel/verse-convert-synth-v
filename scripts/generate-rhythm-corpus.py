#!/usr/bin/env python3
"""Generate the committed MusicXML rhythm corpus with music21.

Run out of CI, commit what it writes:

    uv run --python 3.12 --with music21==10.5.0 python scripts/generate-rhythm-corpus.py

The generated `.musicxml` is the fixture; music21 is never a build or test
dependency, which keeps CI offline and Python-free. Re-run only to change the
corpus, and commit the diff so a reviewer sees what moved.

music21 is used as an independent producer rather than an oracle. Its exporter
writes shapes Verse must survive and a hand-written fixture would not think to
include: `<divisions>` stated once per part and never repeated, `<voice>`
omitted entirely for single-voice music, XML comments between every measure,
content-hash part IDs, and a DOCTYPE. `defaults.divisionsPerQuarter` is 10080 =
2^5 * 3^2 * 5 * 7, so 3-, 5-, 7- and 9-tuplets are exact and an 11-tuplet is
not: music21 rounds it and overfills the bar. Both belong in the corpus.
"""

from __future__ import annotations

import sys
from pathlib import Path

try:
    from music21 import duration, meter, note, stream, tempo, tie
except ImportError:  # pragma: no cover - the message is the whole point
    sys.exit(
        "music21 is missing. Run:\n"
        "  uv run --python 3.12 --with music21==10.5.0 "
        "python scripts/generate-rhythm-corpus.py"
    )

OUTPUT = Path(__file__).resolve().parent.parent / "src-tauri" / "tests" / "corpora" / "rhythm"

# A word per note keeps the material vocal, which is the only material Verse
# projects. Wordless notes would leave the corpus testing nothing but parsing.
SYLLABLES = [
    ("Glo", "begin"),
    ("ri", "middle"),
    ("a", "end"),
    ("in", "single"),
    ("ex", "begin"),
    ("cel", "middle"),
    ("sis", "end"),
    ("De", "begin"),
    ("o", "end"),
    ("hal", "begin"),
    ("le", "middle"),
    ("lu", "middle"),
    ("jah", "end"),
]

PITCHES = ["C4", "D4", "E4", "F4", "G4", "A4", "B4", "C5", "B4", "A4", "G4", "F4", "E4"]


def sung(index: int, pitch: str, note_type: str) -> note.Note:
    """One note carrying the syllable at `index`, cycling the word list."""
    written = note.Note(pitch, type=note_type)
    text, syllabic = SYLLABLES[index % len(SYLLABLES)]
    written.lyrics.append(note.Lyric(text=text, syllabic=syllabic, number=1))
    return written


def tuplet_group(index: int, actual: int, normal: int, base: str) -> list[note.Note]:
    """`actual` notes in the time of `normal`.

    Each note needs its own `Tuplet`: one attached to a Duration is frozen, and
    reusing the object raises rather than sharing the ratio.
    """
    notes = []
    for member in range(actual):
        written = sung(index + member, PITCHES[(index + member) % len(PITCHES)], base)
        written.duration.appendTuplet(duration.Tuplet(actual, normal, base))
        notes.append(written)
    return notes


def part_with(measures: list[stream.Measure]) -> stream.Score:
    score = stream.Score()
    part = stream.Part()
    for measure in measures:
        part.append(measure)
    score.append(part)
    return score


def measure(number: int, *contents) -> stream.Measure:
    bar = stream.Measure(number=number)
    for item in contents:
        if isinstance(item, list):
            bar.append(item)
        else:
            bar.append(item)
    return bar


def exact_tuplets() -> stream.Score:
    """Every tuplet the 10080 grid states exactly, one per bar."""
    bars = []
    bars.append(
        measure(
            1,
            meter.TimeSignature("4/4"),
            tempo.MetronomeMark(number=92, referent=duration.Duration(1.0)),
            tuplet_group(0, 3, 2, "quarter"),
            sung(3, "G4", "quarter"),
        )
    )
    bars.append(measure(2, tuplet_group(4, 5, 4, "quarter")[:4] + [sung(8, "C5", "whole")][:0]))
    bars.append(measure(3, tuplet_group(0, 7, 4, "eighth")))
    bars.append(measure(4, tuplet_group(7, 9, 8, "eighth")[:8]))
    return part_with(bars)


def mixed_tuplets_and_meters() -> stream.Score:
    """A different tuplet and a different meter in every bar."""
    bars = []
    bars.append(
        measure(
            1,
            meter.TimeSignature("4/4"),
            tempo.MetronomeMark(number=138, referent=duration.Duration(1.0)),
            tuplet_group(0, 3, 2, "quarter"),
            sung(3, "A4", "quarter"),
        )
    )
    bars.append(measure(2, meter.TimeSignature("7/8"), tuplet_group(4, 7, 4, "eighth")))
    bars.append(
        measure(
            3,
            meter.TimeSignature("5/4"),
            tuplet_group(0, 5, 4, "quarter"),
        )
    )
    bars.append(
        measure(
            4,
            meter.TimeSignature("3/2"),
            tuplet_group(5, 3, 2, "half"),
        )
    )
    bars.append(
        measure(
            5,
            meter.TimeSignature("12/8"),
            [sung(index, PITCHES[index % len(PITCHES)], "eighth") for index in range(12)],
        )
    )
    return part_with(bars)


def syncopation_across_barlines() -> stream.Score:
    """Every bar ends on a note tied into the next, so no onset lands on one."""
    bars = []
    first = measure(
        1,
        meter.TimeSignature("4/4"),
        tempo.MetronomeMark(number=76, referent=duration.Duration(1.0)),
        sung(0, "C4", "eighth"),
        sung(1, "D4", "quarter"),
        sung(2, "E4", "quarter"),
        sung(3, "F4", "quarter"),
    )
    held = sung(4, "G4", "eighth")
    held.tie = tie.Tie("start")
    first.append(held)
    bars.append(first)

    second = stream.Measure(number=2)
    landing = note.Note("G4", type="eighth")
    landing.tie = tie.Tie("stop")
    second.append(landing)
    second.append(sung(5, "A4", "quarter"))
    second.append(sung(6, "B4", "quarter"))
    second.append(sung(7, "C5", "quarter"))
    tail = sung(8, "B4", "eighth")
    tail.tie = tie.Tie("start")
    second.append(tail)
    bars.append(second)

    third = stream.Measure(number=3)
    resolve = note.Note("B4", type="eighth")
    resolve.tie = tie.Tie("stop")
    third.append(resolve)
    third.append(sung(9, "A4", "eighth"))
    third.append(sung(10, "G4", "half"))
    third.append(sung(11, "F4", "quarter"))
    bars.append(third)
    return part_with(bars)


def rounded_tuplet() -> stream.Score:
    """An 11-tuplet, which 10080 divisions cannot state.

    music21 rounds each member to 1833 and writes 20163 where the bar holds
    20160. Verse must read the notes it is given without inventing the missing
    three divisions or silently absorbing them.
    """
    bar = measure(
        1,
        meter.TimeSignature("2/4"),
        tempo.MetronomeMark(number=60, referent=duration.Duration(1.0)),
        tuplet_group(0, 11, 8, "16th"),
    )
    return part_with([bar])


CORPUS = {
    "exact-tuplets": exact_tuplets,
    "mixed-tuplets-and-meters": mixed_tuplets_and_meters,
    "syncopation-across-barlines": syncopation_across_barlines,
    "rounded-eleven-tuplet": rounded_tuplet,
}


def main() -> int:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for name, build in CORPUS.items():
        path = OUTPUT / f"{name}.musicxml"
        build().write("musicxml", fp=str(path))
        print(f"wrote {path.relative_to(OUTPUT.parent.parent.parent.parent)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
