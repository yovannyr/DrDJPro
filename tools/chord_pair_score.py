"""Evaluate D01 interval recognition against performed and written chord references."""
import argparse
import csv
import json
from collections import defaultdict
from pathlib import Path

from local_key_score import RECORDINGS, checked, read_reference, score


GUITAR = {"Rock": "05_Rock1-130-A_comp", "Funk": "05_Funk1-97-C_comp",
          "Jazz": "05_Jazz3-150-C_comp"}
NOTES = ("C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B")


def triad(label):
    """Reduce extensions to major/minor only if the declared chord has that third."""
    if label in ("N", "X", "unknown"):
        return label
    base = label.split("/")[0]
    note, _, suffix = base.partition(":")
    pitch = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}[note[0]]
    for accidental in note[1:]:
        pitch += {"b": -1, "#": 1}[accidental]
    quality = suffix.split("(")[0]
    if quality.startswith("min"):
        mode = "min"
    elif quality.startswith("maj") or quality in ("", "7", "9", "11", "13"):
        mode = "maj"
    else:
        mode = "other/" + suffix
    return f"{NOTES[pitch % 12]}:{mode}"


def load_estimates(tsv):
    tracks = defaultdict(list)
    with tsv.open(encoding="utf-8", newline="") as file:
        for row in csv.DictReader(file, delimiter="\t"):
            label = row["chord"]
            if label != "unknown":
                if label != triad(label) or label.split(":")[1] not in ("maj", "min"):
                    raise ValueError(f"Unexpected predicted chord {label!r}")
            tracks[row["path"]].append((int(row["start_ms"]) / 1000,
                                        int(row["end_ms"]) / 1000, label))
    return {name: checked(intervals) for name, intervals in tracks.items()}


def jams_references(file):
    annotations = json.loads(file.read_text(encoding="utf-8"))["annotations"]
    refs = {}
    for layer in annotations:
        if layer["namespace"] != "chord":
            continue
        source = layer["annotation_metadata"]["data_source"]
        if source and "manual verification" not in source.lower():
            raise ValueError(f"Unknown annotation source: {source}")
        role = "performed" if source else "instructed"
        if role in refs:
            raise ValueError(f"Ambiguous {role} chord annotations")
        refs[role] = checked([(round(r["time"], 6), round(r["time"] + r["duration"], 6),
                               triad(r["value"])) for r in layer["data"]])
    if set(refs) != {"performed", "instructed"}:
        raise ValueError(f"Missing chord annotation layer in {file}")
    return refs


def compare(ref, pred):
    measures = score(ref, pred, [], tolerance=0.25)
    matches = measures["matched"]
    reference = measures["reference_changes"]
    reported = measures["predictions"]
    result = {k: v for k, v in measures.items() if k not in ("chord_only", "near_chord_only")}
    result.update(unsupported_s=sum(b - a for a, b, label in ref
                                    if label in ("X", "N") or ":other/" in label),
                  precision=matches / reported if reported else None,
                  recall=matches / reference if reference else None,
                  f1=2 * matches / (reported + reference) if reported + reference else None)
    return result


def evaluate(folder, tsv):
    estimates = load_estimates(tsv)
    results = {}
    for name in RECORDINGS:
        rows = read_reference(folder / f"{name}.chords.csv", "majmin")
        ref = [(a, b, triad(c)) for a, b, c in rows]
        results[name] = {"genre": "Art song", "annotated": compare(ref, estimates[name + ".wav"])}
    for genre, name in GUITAR.items():
        pred = estimates[name + "_mic.wav"]
        results[name] = {"genre": genre}
        for role, ref in jams_references(folder / f"{name}.jams").items():
            results[name][role] = compare(ref, pred)
    return results


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("predictions", type=Path)
    args = parser.parse_args()
    print(json.dumps(evaluate(args.corpus, args.predictions), indent=2))
