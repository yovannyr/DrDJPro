# Test fixture

`tone-9s.mp3` is an original synthetic 440 Hz sine tone (9 seconds, stereo, 44.1 kHz), generated locally for codec/load regression tests. It contains no third-party music. Tests read it from the repository and do not need ffmpeg or network access.

Generation command (ffmpeg with libmp3lame):

```sh
ffmpeg -hide_banner -loglevel error -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=9" -ac 2 -codec:a libmp3lame -b:a 32k tests/fixtures/tone-9s.mp3
```

WAV files are generated deterministically inside tests and removed through RAII.

## Prepare tempo comparison (D04)

Generate both listening excerpts into an empty folder of your choice. The script uses Python 3 and its standard library. It writes two 8-second, 48 kHz, mono, 16-bit PCM WAV files (about 1.47 MiB together).

```sh
python tests/fixtures/prepare_tempo.py ./d04-audio
```

Both scores and drum sounds are original project fixtures covered by the [repository license](../../LICENSE). Each file starts with one second of silence, followed by two scored bars. The table gives sixteenth-note positions in each bar, counting from zero.

| File | Kick | Snare | Closed hi-hat |
| --- | --- | --- | --- |
| `straight-85.wav` | 0, 4, 8, 12 | 4, 12 | 0, 2, 4, 6, 8, 10, 12, 14 |
| `breakbeat-85.wav` | 0, 3, 7, 10, 14 | 4, 9, 12 | 0, 2, 6, 8, 10, 14 |

In Prepare, use the first kick at **1.000 s** (frame **48,000**) as the common downbeat. Listen for the steady kick in the straight beat and the early kicks around the snare in the breakbeat. The musical events should occupy identical frames when switching between interpretations. Frame numbers below are zero-based and rounded to the nearest sample.

| Interpretation | First four beat frames | Start of the next 4/4 bar |
| --- | --- | --- |
| 85 BPM | 48,000 · 81,882 · 115,765 · 149,647 | 183,529 (3.82352 s) |
| 170 BPM | 48,000 · 64,941 · 81,882 · 98,824 | 115,765 (2.41177 s) |

These scored examples support reproducible checks of timing and signal routing. Musical acceptance on annotated recordings and the physical headphone check remain separate.

## Additional codec fixtures


The `format-aac.m4a`, `format-alac.m4a`, `format-flac.flac`, `format-aiff.aiff` and `format-vorbis.ogg` fixtures are original synthetic one-second 440 Hz tones at 48 kHz stereo. They carry the synthetic title `Fixture Tone` and artist `Fixture Artist` where the container supports these tags. They were generated locally with FFmpeg and contain no third-party music:

```sh
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -ac 2 -metadata "title=Fixture Tone" -metadata "artist=Fixture Artist" -c:a aac tests/fixtures/format-aac.m4a
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -ac 2 -metadata "title=Fixture Tone" -metadata "artist=Fixture Artist" -c:a alac tests/fixtures/format-alac.m4a
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -ac 2 -metadata "title=Fixture Tone" -metadata "artist=Fixture Artist" -c:a flac tests/fixtures/format-flac.flac
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -ac 2 -metadata "title=Fixture Tone" -metadata "artist=Fixture Artist" -c:a pcm_s16be tests/fixtures/format-aiff.aiff
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -ac 2 -metadata "title=Fixture Tone" -metadata "artist=Fixture Artist" -c:a libvorbis tests/fixtures/format-vorbis.ogg
```

The `format-opus.opus` and `format-opus-mono.opus` fixtures are original synthetic one-second 440 Hz tones at 48 kHz, tagged with the same title and artist. Generate them with FFmpeg and libopus:

```sh
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -ac 2 -metadata "title=Fixture Tone" -metadata "artist=Fixture Artist" -c:a libopus tests/fixtures/format-opus.opus
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -ac 1 -metadata "title=Fixture Tone" -metadata "artist=Fixture Artist" -c:a libopus tests/fixtures/format-opus-mono.opus
```
