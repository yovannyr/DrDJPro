# R04 · Kopien und verschiedene Edits beim Import prüfen

Unter **Library → Files** Dateien oder einen Ordner auswählen. Findet Dr. DJ Pro mögliche Kopien oder Edits, zeigt die Importprüfung pro Datei den Quellpfad, den Vergleichspfad und den Grund. **Keep** übernimmt die angezeigte Datei in die Auswahl. **Skip** überspringt sie. Über die Dateinummer lässt sich direkt zu einem bestimmten Eintrag springen, während beide Decks bedienbar sind.

| Anzeige | Grundlage | Entscheidung |
| --- | --- | --- |
| Exact copy | Gleiche Größe, identischer SHA-256-Wert und ein anschließender byteweiser Vergleich | Die gewünschte Kopie ausdrücklich auswählen. |
| Possible edit | Titel und gegebenenfalls Interpret stimmen nach dem Entfernen von Versionszusätzen überein. Bekannte Laufzeiten stehen im Befund. | Radio- und Extended-Version lassen sich einzeln behalten. |
| Not checked | Eine Datei ließ sich nicht prüfen oder änderte sich während der Prüfung. | Ursache beheben und mit **Rescan** erneut prüfen. |

**Apply import** prüft die Auswahl nochmals gegen Dateien und Katalog. Bei geänderten Befunden öffnet sich die aktualisierte Auswahl. **Cancel** verwirft die laufende Entscheidung. Der Import schreibt keine Tags in Musikdateien.

## Automatisierte Gegenproben

- Eine umbenannte Kopie, ein Radio- und ein Extended-Edit und gleich große Dateien mit unterschiedlichen Bytes werden mit ihrem erwarteten Befund verglichen. Zwei identische beschädigte MP3-Dateien bleiben gesperrt.
- Die Tests rufen die Datei- und Ordnerauswahl über den Controller auf. Sie brechen eine laufende Bestätigung ab, ersetzen eine Datei zwischen Auswahl und Bestätigung und öffnen den gespeicherten Katalog erneut. Titel-IDs, Crate-Zugehörigkeit, Cue und Dateiprüfsummen werden einzeln verglichen.
- Die UI-Prüfung springt bei 100.000 Einträgen zum letzten Eintrag. Sie prüft die Dateientscheidung und die Bedienbarkeit beider Decks in Classic und Modern. Eine weitere Gegenprobe übernimmt sieben Audioformate und spielt jedes auf beiden Decks ab.

## Messung mit zwei laufenden Decks

Unter Windows lief dieselbe Vierkanalmischung mit 48 kHz und 480 Frames pro Block. Nach jeweils 100 Aufwärmblöcken wurden je 3.000 Blöcke über 30 Sekunden gemessen. Ein Hintergrundthread las im aktiven Lauf wiederholt die Header dreier lokaler WAV-Dateien und verglich die gleich großen Dateien. Zwei davon waren bytegleich. Beide Läufe nutzten den programmgesteuerten Ausgang.

| Messgröße | Ohne Prüfung | Mit Dateiprüfung |
| --- | ---: | ---: |
| Abgeschlossene Prüfdurchgänge | 0 | 1.285 |
| Renderzeit p99 / Maximum | 7,765 / 8,204 ms | 7,253 / 8,156 ms |
| Startverzögerung p99 / Maximum | 0,692 / 1,553 ms | 0,603 / 1,798 ms |
| Verfehlte 10-ms-Fristen | 0 | 0 |
| Blöcke mit fehlerhaftem Signal | 0 | 0 |
| Heap-Allokationen im Renderpfad | 0 | 0 |

Wiederholbar mit `cargo test --locked --bin drdjpro-ui-spike r04_equal_duration_import_checks_with_two_playing_decks -- --ignored --nocapture --test-threads=1`. Netzwerkfreigaben und der physische Audiotreiber waren nicht Teil dieser Messung.

## Gemeinsame Abnahme

Eine bekannte Kopie und zwei eigene Edits in Classic und Modern importieren. Die Pfade und Gründe vergleichen, die Aufnahme beider Edits bestätigen und während der Auswahl beide Decks bedienen. Die Hör- und Bedienprobe am Gerät steht aus.
