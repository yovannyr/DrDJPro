# D04 · 85 und 170 BPM in Prepare vergleichen

In Classic oder Modern einen analysierten Titel mit Downbeat oder einen Titel mit gespeichertem Grid in **Library → Prepare** auswählen. **Compare BPM** zeigt beide Deutungen. Jeder Klick startet denselben bis zu acht Sekunden langen Ausschnitt von vorn. Ohne eigenständigen Kopfhörerausgang bleiben die Beatmarken sichtbar. **Apply grid** speichert BPM und Downbeat für den ausgewählten Titel. Ein laufendes Deck behält sein bereits geladenes Raster.

## Automatisierte Gegenproben

- Die Signaltests prüfen den ersten Schlag bei 1,000 s, Beat- und Taktabstände mit maximal einem Sample Abweichung, den Taktakzent, die Abspielgeschwindigkeit und die getrennten Kanäle. Der Master wird sampleweise mit einer zweiten Engine ohne Vorschau verglichen.
- Die Speichertests prüfen Neustart, gesperrte und zwischenzeitlich geänderte Grids, Tempo-, Takt- und Warpmarker sowie einen erzwungenen SQLite-Schreibfehler. Cues und Loops gehören zum Vergleich des Datenbestands.
- Der Vergleich liest die Musikdatei im Hintergrund. Beim Titelwechsel wird die alte Anforderung abgebrochen. Pro Fenster darf höchstens ein Decoder gleichzeitig laufen. Im Audiothread werden vorbereitete Samples und Klickmarken gelesen.

## Windows-Lastmessung am 23. November 2026

Zwei laufende Decks, vier Ausgabekanäle, 48 kHz, 480 Frames pro Block. Beide Läufe nutzten denselben Windows-Build, dieselben Quellsignale und jeweils 100 Aufwärmblöcke. Die Frist von 10 ms umfasst die Verzögerung bis zum Start des Blocks und die Renderzeit.

| Messgröße | Vorschau aus | Vorschau aktiv |
| --- | ---: | ---: |
| Dauer / Blöcke | 30 s / 3.000 | 30 s / 3.000 |
| Wechsel zwischen den Deutungen | 0 | 116 |
| Renderzeit p99 / Maximum | 7,195 / 7,928 ms | 6,500 / 7,694 ms |
| Startverzögerung p99 / Maximum | 0,690 / 1,505 ms | 0,739 / 1,612 ms |
| Verfehlte 10-ms-Fristen | 0 | 0 |
| Blöcke mit fehlerhaftem Ausgangssignal | 0 | 0 |
| Heap-Allokationen im Renderpfad | 0 | 0 |

Der Test lässt sich mit `cargo test --locked --bin drdjpro-ui-spike d04_equal_duration_headphone_switching_load -- --ignored --nocapture --test-threads=1` wiederholen. Er prüft die Produktionsmischung mit einem programmgesteuerten Ausgang. Der konkrete Audiotreiber und die physische Kopfhörerbuchse gehören zur gemeinsamen Abnahme.

## Gemeinsame Abnahme am Ende

Die [annotierten Hörbeispiele](../../../tests/fixtures/README.md) in Prepare ausprobieren. Während Deck A spielt, beide Deutungen über den Hardware-Kopfhörerausgang abhören und den Master gegenhören. Danach mit einem musikalisch geprüften eigenen Titel wiederholen und die bevorzugte Deutung speichern. Die visuelle und akustische Abnahme am Gerät ist offen.
