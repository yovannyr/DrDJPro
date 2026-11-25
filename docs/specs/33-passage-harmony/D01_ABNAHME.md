# D01 · Zwei Akkordspuren im Übergang

**Stand am 24. November 2026.** Die Spuren, Korrekturen und der Kopfhörermix sind implementiert. Im ersten Signallauf lagen alle drei Wechsel der viertaktigen Referenz auf der erwarteten Grenze. Mit einem verschobenen Einstieg meldete der Detektor am Schluss zusätzlich einen 4 ms langen unbekannten Abschnitt. Die Endbehandlung stützt einen solchen Rest jetzt nur bei übereinstimmenden vorausgehenden Akkordbelegen.

## Messung auf den sechs festgelegten Aufnahmen

Die Windows-Auswertung vergleicht die vorhergesagten Akkordintervalle mit den unten benannten Referenzen. „Korrekt“ und „unbekannt“ sind Anteile der annotierten Dauer. Für GuitarSet gilt hier die nachgehörte Annotation. Die letzte Spalte nennt den absoluten Zeitfehler der zugeordneten Wechsel.

| Material | Annotierte Dauer | Korrekt | Unbekannt | Getroffene / vorhandene Wechsel | Gemeldete Wechsel | Zeitfehler Median / Maximum |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Kunstlied, drei Titel | 421,12 s | 29,15 % | 63,97 % | 50 / 261 | 145 | 30 / 220 ms |
| Funk, ein Titel | 29,69 s | 17,01 % | 73,73 % | 0 / 5 | 14 | – |
| Jazz, ein Titel | 25,60 s | 6,25 % | 78,52 % | 4 / 15 | 10 | 175 / 250 ms |
| Rock, ein Titel | 22,15 s | 30,70 % | 63,88 % | 0 / 5 | 0 | – |

Bei allen vier Gruppen blieb mehr als die Hälfte der Dauer unbekannt. Bei Funk und Rock traf die Analyse keinen Referenzwechsel innerhalb von 250 ms. Diese Ergebnisse beschreiben genau diese Aufnahmen. Für die Arbeit am Set bleibt das Abhören und Korrigieren der Vorschläge wesentlich.

## Vorab festgelegte Auswertung

Für die Entwicklung dienen eigene Synthesesignale. Bei 120 BPM in 4/4 hält A über vier Takte d-Moll, während B taktweise Dm, Gm, A und Dm spielt. Jeder Wechsel darf höchstens 125 ms von der Referenz abweichen. Rauschen und ein unbestimmter Einzelton sollen als unbekannt erscheinen. Zusätzlich prüfen Signaltests den gemeinsamen Kopfhörerausschnitt und die vom Master getrennte Ausgabe.

Der erste Lauf auf echten Aufnahmen verwendet sechs vor der Messung ausgewählte Interpretationen. Die drei Kunstlieder HU33 aus dem Schubert Winterreise Dataset sind dieselben wie bei A06. Für Akkorde ist hier die Spalte `majmin` der veröffentlichen Akkordannotation maßgeblich. Aus GuitarSet kommen drei Mikrofonaufnahmen mit Rhythmusgitarre hinzu: `05_Rock1-130-A_comp`, `05_Funk1-97-C_comp` und `05_Jazz3-150-C_comp`. Für GuitarSet gilt die semiautomatisch erstellte, manuell geprüfte Annotation der gespielten Akkorde. Die angewiesene Akkordfolge wird getrennt ausgewertet. [Schubert Winterreise Dataset](https://zenodo.org/records/10839767) · [GuitarSet](https://zenodo.org/records/3371780)

Die Bewertung reduziert Dur-/Mollakkorde und ihre Erweiterungen auf Grundton und Tongeschlecht. Umkehrungen werden gleich behandelt. Eine Sus-Passage ohne ausgewiesene Terz bleibt im Nenner und kann durch ein Dur-/Moll-Label keinen Treffer erzielen. Unbekannte oder fehlende Vorhersagen zählen ebenfalls im Nenner. Die Zeitgrenzen werden chronologisch und paarweise bei höchstens 250 ms Abweichung verglichen. Für jedes Genre werden korrekte und unbekannte Dauer, Zahl der Wechsel, Treffer und Zeitfehler ausgewiesen. Pro Genre liegt nur eine kleine Stichprobe vor.

Die drei WAV- und CSV-Paare für die Kunstlieder sind im [A06-Prüfbericht](A06_ABNAHME.md) beschrieben. Für GuitarSet die Dateien `<Name>_mic.wav` aus `audio_mono-mic.zip` und `<Name>.jams` aus `annotation.zip` in denselben lokalen Ordner legen. Die Musikdateien werden nicht mit der App ausgeliefert. `DRDJPRO_HARMONY_CORPUS` verweist auf den Ordner, `DRDJPRO_CHORD_TSV` auf eine neue Ausgabedatei.

```powershell
cargo test --locked --bin drdjpro-ui-spike d01_corpus_intervals_tsv -- --ignored --nocapture --test-threads=1
python -B tools/chord_pair_score.py $env:DRDJPRO_HARMONY_CORPUS $env:DRDJPRO_CHORD_TSV
```
