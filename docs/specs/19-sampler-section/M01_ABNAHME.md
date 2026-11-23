# M01 · Samples vor der Pad-Zuweisung vorhören

**Sampler → Browse** am gewünschten Slot öffnen. **Choose folder** listet die unterstützten Audiodateien im gewählten Ordner. Eine Datei anklicken oder mit **Next** und **Previous** wechseln. Über einen getrennten Kopfhörerausgang sind die ersten drei Sekunden des Samples zu hören. Bei Stereoausgabe über ein einziges Paar stehen Auswahl und Zuweisung zur Verfügung.

**Close** verwirft die Auswahl. **Assign to slot** lädt die ausgewählte Datei mit dem regulären Pad-Ladevorgang. Für die Zuweisung gelten die bestehenden Grenzen von zehn Sekunden und 480.000 Quellframes. Das Pad behält seine bisherige Datei, solange der Browser nur zum Vorhören verwendet wird.

## Speicherbedarf

Der Browser listet bis zu 4.096 Audiodateien aus einem Ordner. Der Cache liegt im Arbeitsspeicher, hat Platz für höchstens 128 Einträge und verwaltet bis zu 16 MiB PCM-Daten. Gezählt wird der reservierte Speicher der Samplepuffer. Die Anzeige im Browser zeigt die aktuelle Belegung. Bei vollem Cache wird zuerst das Sample entfernt, das am längsten nicht verwendet wurde. Beim Schließen oder Ordnerwechsel wird der Cache geleert.

## Automatisierte Prüfungen unter Windows

Der Controller-Test wählt 20 verschiedene lokale WAV-Dateien bei laufendem Deck A und B. Nach dem Schließen und nach einem verspäteten Ordnerdialog bleiben die vier belegten Pads erhalten. Nach der ausdrücklichen Zuweisung und dem Neustart enthält genau der gewählte Slot das neue Sample. Die UI-Prüfung bedient den Browser in Classic und Modern.

Der Audiotest prüft die zwanzig unterschiedlichen Hörproben auf den Kopfhörerkanälen und vergleicht den Master samplegenau mit einer zweiten Engine. Wiederholte Cachezugriffe prüfen den geöffneten Dateihandle auf Identität, Größe, Änderungszeit und den Änderungsmarker des Dateisystems. Tests ersetzen und überschreiben eine Datei bei gleichbleibender Länge und wiederhergestellter Änderungszeit. Fehlen verlässliche Metadaten, wird neu dekodiert. Auf Dateisystemen mit grober Zeitauflösung können Änderungen am selben Objekt innerhalb eines Zeitrasters ununterscheidbar bleiben.

Beide Windows-Lastläufe verwendeten 48 kHz, vier Ausgabekanäle, 100 Aufwärmblöcke und 3.000 Blöcke in 30 Sekunden. Der aktive Lauf wechselte über zwanzig Dateien hinweg zwischen den Hörproben.

| Messgröße | Ohne Hörproben | Mit Hörproben |
| --- | ---: | ---: |
| Samplewechsel / bestätigte Cachetreffer | 0 / 0 | 115 / 95 |
| Höchste PCM-Cachebelegung | 0 B | 7.680.000 B |
| Renderzeit p99 / Maximum | 6,018 / 6,802 ms | 6,142 / 6,717 ms |
| Startverzögerung p99 / Maximum | 0,725 / 1,867 ms | 0,729 / 0,867 ms |
| Verfehlte 10-ms-Fristen | 0 | 0 |
| Fehlerhafte Signalblöcke / Render-Allokationen | 0 / 0 | 0 / 0 |

Wiederholbar mit `cargo test --locked --bin drdjpro-ui-spike m01_equal_duration_twenty_sample_auditions_with_two_decks -- --ignored --nocapture --test-threads=1`.

## Gemeinsame Hörprobe am Ende

Bei laufendem Master eigene Kicks, Claps und weitere Samples durchschalten. Über die Kopfhörer am MC4000 prüfen, dass die Auswahl wechselt und der Master unverändert bleibt. Erst danach einen Slot zuweisen und den Wechsel am zugehörigen Pad hören.
