"""Generate the two original 85/170 BPM listening excerpts documented in README.md."""

import argparse
import math
import random
import sys
import wave
from array import array
from fractions import Fraction
from pathlib import Path

RATE = 48_000
SCORES = {
    "straight-85.wav": {
        "kick": (0, 4, 8, 12),
        "snare": (4, 12),
        "hat": (0, 2, 4, 6, 8, 10, 12, 14),
    },
    "breakbeat-85.wav": {
        "kick": (0, 3, 7, 10, 14),
        "snare": (4, 9, 12),
        "hat": (0, 2, 6, 8, 10, 14),
    },
}


def drum_hit(kind):
    if kind == "kick":
        samples = 4800
        sound = [
            0.30 * math.cos(math.tau * (160 * t - 600 * t * t)) * math.exp(-38 * t)
            for t in (frame / RATE for frame in range(samples))
        ]
    else:
        snare = kind == "snare"
        samples = 3600 if snare else 1680
        noise = random.Random(42 if snare else 99)
        gain, decay = (0.25, 37) if snare else (0.12, 95)
        sound = [gain * noise.uniform(-1, 1) * math.exp(-decay * i / RATE) for i in range(samples)]
        sound[0] = gain
    return [value * (1 - i / (samples - 1)) for i, value in enumerate(sound)]


def score_pcm(score):
    pcm = array("h", [0]) * (8 * RATE)
    frames_per_step = Fraction(RATE * 60, 85 * 4)
    for kind, steps in score.items():
        hit = [round(32_767 * sample) for sample in drum_hit(kind)]
        for bar in range(2):
            for step in steps:
                start = RATE + round((16 * bar + step) * frames_per_step)
                for i, sample in enumerate(hit):
                    pcm[start + i] += sample
    return pcm


def write_wav(path, pcm):
    if sys.byteorder == "big":
        pcm.byteswap()
    with path.open("xb") as output, wave.open(output, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(RATE)
        wav.writeframes(pcm)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="output folder for both WAV files")
    directory = parser.parse_args().directory
    directory.mkdir(parents=True, exist_ok=True)
    for name in SCORES:
        if (directory / name).exists():
            parser.error(f"file already exists: {directory / name}")
    for name, score in SCORES.items():
        write_wav(directory / name, score_pcm(score))


if __name__ == "__main__":
    main()
