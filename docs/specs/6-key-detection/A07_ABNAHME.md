# A07 · Tonart mit dem Referenzpiano prüfen

In **Library → Prepare → Cover labels / Piano** einen Titel auswählen. Eine Camelot-Tonart und den Beginn der Passage in Sekunden eingeben. **Root** spielt den gewählten Ausschnitt mit dem Grundton über Kopfhörer, **Chord** mit dem zugehörigen Dreiklang. Die Passage umfasst bis zu acht Sekunden und wiederholt sich bis zum Stoppen. Bei einem Gerät mit nur einem Ausgangspaar ist die akustische Vorschau gesperrt.

**Save** bestätigt die Tonart im Katalog. Der Editor zeigt die manuelle Bestätigung und den ursprünglichen Analysewert mit ihrer Herkunft an. **Reset key** entfernt die Bestätigung für den ausgewählten Titel. Die gespeicherte Energie bleibt erhalten.

## Nachweise

- Der Signaltest mischt eine synthetische A-Moll-Passage gegenphasig in die beiden Kopfhörerkanäle. Ihre Differenz isoliert die Originalpassage, ihre Summe die Referenz. So lassen sich A4, C5 und E5 in der Referenz unabhängig vom Audiomaterial prüfen. Eine zweite Engine spielt dieselben beiden Decks als samplegenaue Master-Gegenprobe.
- Die Prüfung der Stimmung deckt alle 24 Camelot-Tonarten ab. Für 8A wird die kleine Terz, für 8B die große Terz erwartet. Weitere Prüfungen decken Grundton allein, die Schleifengrenze, Abbruch und den Verlust der getrennten Ausgaberoute ab.
- Die Controller-Tests speichern eine Bestätigung für Titel A, öffnen den Katalog erneut und setzen die Tonart zurück. Der ursprüngliche Analysewert erscheint wieder. Energie, Tonart von Titel B und die Prüfsumme der Quelldatei gehören zu den Gegenproben für die Katalogbearbeitung. Die regulären 14 UI-Tests liefen unter Windows in der gemeinsamen Slint-Oberfläche erfolgreich.

## Windows-Lastvergleich

Zwei spielende Decks bei 48 kHz, vier Ausgabekanäle, je 100 Aufwärmblöcke und 3.000 gemessene Blöcke in 30 Sekunden. Der aktive Lauf spielte gleichzeitig einen wiederholten Ausschnitt mit A-Moll-Dreiklang über die Kopfhörerroute. Die Vergleichsläufe verwendeten denselben Build und dieselben Decksignale.

| Messgröße | Vorschau aus | Vorschau aktiv |
| --- | ---: | ---: |
| Renderzeit p99 / Maximum | 7,073 / 8,002 ms | 6,984 / 8,933 ms |
| Startverzögerung p99 / Maximum | 0,697 / 1,197 ms | 0,756 / 1,880 ms |
| Verfehlte 10-ms-Fristen | 0 | 0 |
| Fehlerhafte Signalblöcke / Render-Allokationen | 0 / 0 | 0 / 0 |

Messung wiederholen mit `cargo test --locked --bin drdjpro-ui-spike a07_equal_duration_reference_piano_with_two_decks -- --ignored --nocapture --test-threads=1`.

## Gemeinsame Hörprobe am Ende

Einen eigenen Titel mit gut hörbarer Basslinie oder einem Akkord in Prepare auswählen. Bei laufendem Master dieselbe Passage mit Grundton und Dreiklang auf dem MC4000 vergleichen. Danach die Tonart bestätigen, erneut öffnen und zurücksetzen.
