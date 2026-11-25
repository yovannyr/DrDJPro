# R05 · BPM und Tonart nach Quelle prüfen

In Classic oder Modern einen Titel unter **Library → Prepare** auswählen und **BPM / key info** öffnen. Der Dialog liest die Dateitags für den ausgewählten Titel und stellt ihnen die letzte Audioanalyse sowie die im Katalog wirksamen Werte gegenüber.

| Abschnitt | Was er beantwortet |
| --- | --- |
| File tags | Welche BPM- und Tonartangaben enthält die Musikdatei? |
| Last audio analysis | Welche Werte hat die zuletzt gespeicherte Analyse ermittelt? |
| Effective catalog | Welches Grid und welche bestätigte Tonart gelten für diesen Titel? |

Stehen beispielsweise 85 BPM im Tag und 170 BPM im gespeicherten Grid, können beide Zahlen denselben Puls unterschiedlich zählen. **Compare BPM** öffnet bei vorhandenem Bezugsbeat den [Hörvergleich in Prepare](../3-bpm-detection/D04_ABNAHME.md). Eine manuell bestätigte Tonart bleibt beim Öffnen und Schließen der Diagnose erhalten.

## Automatisierte Gegenproben

Der Windows-Test öffnet die Diagnose in beiden Ansichten für zwei schreibgeschützte MP3-Dateien. Für die erste Datei prüft er 85 BPM im Tag, eine fehlende Audioanalyse und 170 BPM im gespeicherten Grid. Für die zweite prüft er den Tag `Am` und die abweichende bestätigte Tonart `11B`. Nach dem Schließen vergleicht er beide Dateiprüfsummen, das Schreibschutzattribut und die aus SQLite neu geladenen Korrekturen.

Weitere Tests wechseln die Auswahl während einer laufenden Anfrage, prüfen die getrennte Benennung nicht lesbarer und fehlender Werte und lesen Opus-Kommentare. Die Oberflächentests prüfen die Navigation mit Tastatur, die schreibgeschützte Anzeige über die Bedienungshilfen und den Aufruf des Hörvergleichs.

Auch bei einem 2,25 MiB großen eingebetteten Cover findet der MP3-Test die dahinter gespeicherten Werte in ID3v2.3 und ID3v2.4. Der Leser überspringt das Bild und liest pro Anfrage höchstens 2 MiB aus der Datei. AAC, ALAC, FLAC, AIFF, Vorbis, Opus und MP3 wurden mit den Audiodateien aus dem Repository geprüft.

## Audiolast unter Windows

Zwei Decks spielten dieselben Signale über vier Kanäle bei 48 kHz. Nach 100 Aufwärmblöcken lief jede Messung 30 Sekunden mit 3.000 Blöcken zu je 480 Frames. Im zweiten Lauf öffnete ein Hintergrundthread wiederholt die getaggte MP3-Datei und erstellte die Diagnose vollständig.

| Messgröße | Ohne Diagnose | Mit Diagnose |
| --- | ---: | ---: |
| Abgeschlossene Diagnosen | 0 | 280 |
| Renderzeit p99 / Maximum | 5,796 / 7,259 ms | 7,489 / 9,283 ms |
| Startverzögerung p99 / Maximum | 0,720 / 1,295 ms | 0,672 / 1,104 ms |
| Verfehlte 10-ms-Fristen | 0 | 0 |
| Blöcke mit fehlerhaftem Signal | 0 | 0 |
| Heap-Allokationen im Renderpfad | 0 | 0 |

Der Test ist mit `cargo test --locked --bin drdjpro-ui-spike r05_equal_duration_metadata_diagnosis_with_two_decks -- --ignored --nocapture --test-threads=1` wiederholbar. Beide Läufe verwendeten den programmgesteuerten Ausgang.

## Gemeinsame Abnahme

Einen eigenen Titel mit widersprüchlichen Tags auswählen. Die Werte mit einem unabhängig ausgelesenen Dateitag vergleichen und über Kopfhörer zwischen beiden Tempo-Deutungen wechseln. Lesbarkeit in beiden Ansichten und Hörprobe am Gerät sind offen.
