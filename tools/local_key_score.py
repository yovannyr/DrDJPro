"""Score Prepare's local key timeline on the three held-out SWD recordings."""
import argparse
import csv
import json
import math
from collections import defaultdict
from pathlib import Path


RECORDINGS = tuple(f"Schubert_D911-{number}_HU33" for number in ("02", "11", "22"))
MAJOR = (8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1)
MINOR = (5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10)


def camelot(label):
    note, mode = label.split(":")
    pitch = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}[note[0]]
    for accidental in note[1:]:
        pitch += {"#": 1, "b": -1}[accidental]
    numbers, suffix = {"maj": (MAJOR, "B"), "min": (MINOR, "A")}[mode]
    return f"{numbers[pitch % 12]}{suffix}"


def read_reference(path, column):
    with path.open(encoding="utf-8-sig", newline="") as file:
        rows = csv.DictReader(file, delimiter=";")
        return checked([(float(r["start"]), float(r["end"]), r[column]) for r in rows])


def checked(rows):
    previous = 0
    for start, end, _ in rows:
        if not math.isfinite(start) or not math.isfinite(end) or start < previous or end <= start:
            raise ValueError(f"Invalid interval {start}–{end} after {previous}")
        previous = end
    if not rows:
        raise ValueError("Empty timeline")
    return rows


def read_estimates(path):
    tracks = defaultdict(list)
    allowed = {f"{n}{mode}" for n in range(1, 13) for mode in ("A", "B")}
    with path.open(encoding="utf-8", newline="") as file:
        for row in csv.DictReader(file, delimiter="\t"):
            key = row["key"]
            if key != "unknown" and key not in allowed:
                raise ValueError(f"Unexpected key {key!r}")
            tracks[row["path"]].append((int(row["start_ms"]) / 1000,
                                        int(row["end_ms"]) / 1000, key))
    return {track: checked(rows) for track, rows in tracks.items()}


def boundary_errors(reference, estimate, tolerance):
    """Ordered one-to-one matching. Maximize count, then minimize combined error."""
    previous = [(0, 0.0)] * (len(estimate) + 1)
    back = []
    for ref in reference:
        row = [(0, 0.0)]
        choices = bytearray(len(estimate) + 1)
        for j, prediction in enumerate(estimate, 1):
            candidates = [(previous[j], 0), (row[j - 1], 1)]
            delta = abs(ref - prediction)
            if delta <= tolerance:
                count, neg_error = previous[j - 1]
                candidates.append(((count + 1, neg_error - delta), 2))
            score, choice = max(candidates)
            row.append(score)
            choices[j] = choice
        back.append(choices)
        previous = row
    errors = []
    i, j = len(reference), len(estimate)
    while i and j:
        choice = back[i - 1][j]
        if choice == 2:
            errors.append(abs(reference[i - 1] - estimate[j - 1]))
        if choice != 1:
            i -= 1
        if choice != 0:
            j -= 1
    return errors[::-1]


def changes(rows):
    return [(start, "unknown" in (old, new))
            for (_, last_end, old), (start, _, new) in zip(rows, rows[1:])
            if last_end == start and old != new]


def score(ref, prediction, chords, tolerance=2.0):
    durations = defaultdict(float)
    correct = known = 0.0
    for start, end, label in ref:
        durations[label] += end - start
        for left, right, key in prediction:
            overlap = max(0, min(right, end) - max(left, start))
            if key != "unknown":
                known += overlap
            if label == key:
                correct += overlap
    duration = sum(durations.values())
    reference = [t for t, _ in changes(ref)]
    windows = [(a, b) for a, b, _ in ref]
    inside = lambda t: ref[0][0] < t < ref[-1][1] and any(a <= t < b for a, b in windows)
    transitions = [(t, unknown) for t, unknown in changes(prediction) if inside(t)]
    estimated = [t for t, unknown in transitions if not unknown]
    errors = boundary_errors(reference, estimated, tolerance)
    far = lambda t: all(abs(t - r) > tolerance for r in reference)
    chord_only = [t for t, _ in changes(chords) if inside(t) and far(t)]
    return dict(seconds=duration, correct_s=correct, unknown_s=max(0, duration - known),
                best_constant_s=max(durations.values()), reference_changes=len(reference),
                predictions=len(estimated), matched=len(errors), missed=len(reference) - len(errors),
                extra=len(estimated) - len(errors), errors_s=errors,
                unknown_boundaries=sum(u for _, u in transitions),
                chord_only=len(chord_only),
                near_chord_only=len(boundary_errors(chord_only, [t for t in estimated if far(t)], tolerance)))


def evaluate(folder, tsv):
    estimates = read_estimates(tsv)
    output = {}
    for name in RECORDINGS:
        ref = [(a, b, camelot(c)) for a, b, c in read_reference(folder / f"{name}.keys.csv", "key")]
        chords = read_reference(folder / f"{name}.chords.csv", "majmin")
        output[name] = score(ref, estimates[name + ".wav"], chords)
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("predictions", type=Path)
    args = parser.parse_args()
    print(json.dumps(evaluate(args.corpus, args.predictions), indent=2))
