# A06 · Lokale Tonart in Prepare

## Prüfverfahren

Für die Entwicklung dienen synthetische Kadenzen. Eine Modulation von C-Dur nach D-Dur soll innerhalb eines Takts lokalisiert werden. Eine weitere Aufnahme enthält mehrere Akkorde innerhalb von C-Dur. Rauschen soll als unbekannt erscheinen. Die Audioausgabe wird mit zwei laufenden Decks über zweimal 3.000 Blöcke bei der bestehenden Frist von 10 ms gemessen.

Für die anschließende Auswertung wurden vor dem ersten Lauf auf echten Aufnahmen drei Aufnahmen der Interpretationsreihe HU33 aus dem Schubert Winterreise Dataset festgelegt: D 911 Nr. 2, 11 und 22. Maßgeblich sind die lokalen Tonarten von Annotator 1 in den veröffentlichten Zeitkoordinaten der jeweiligen Aufnahme. Die Audio- und Referenzdateien werden außerhalb des Repositorys gehalten. [Datensatz und Annotationen](https://zenodo.org/records/10839767)

Für jede Aufnahme werden die korrekt benannte Dauer und die als unbekannt ausgewiesene Dauer auf allen annotierten Sekunden angegeben. Enharmonische Schreibweisen sind gleichwertig, Dur und Moll sind verschieden. Eine fehlende Vorhersage gilt als unbekannt. Als Vergleich dient der günstigste durchgehende Einzelwert pro Aufnahme, nachträglich aus ihrer Referenz bestimmt. Die Auswertung ordnet tatsächlichen und gemeldeten Tonartwechseln bei höchstens zwei Sekunden Abstand jeweils einen Partner zu. Sie weist Treffer, zusätzliche und verfehlte Wechsel sowie die absoluten Zeitfehler der Treffer aus. Übergänge in oder aus unbekannten Passagen werden gesondert gezählt.

Das Korpus enthält begleitetes Kunstlied. Die gemeinsame Prüfung mit eigenen DJ-Tracks und der vorhandenen Hardware erfolgt am Ende des Plans.

## Signal- und Korpusergebnisse

Im synthetischen Stereo-Beispiel mit gegenphasigen Kanälen erkannte der PCM-Pfad C-Dur (8B) von 0 bis 8 s und D-Dur (10B) von 8 bis 16 s. Die Abweichung zum vorgegebenen Wechsel betrug 0 ms. Akkordwechsel innerhalb einer C-Dur-Kadenz lösten keine Modulation aus. Stille und breitbandiges Rauschen ergaben unbekannte Abschnitte. Der Test prüft auch eine solche Unterbrechung zwischen zwei tonalen Passagen.

| D 911 | Annotiert | Tonart korrekt | Unbekannt | Bester Einzelwert | Wechsel getroffen / Referenz | Zusätzlich gemeldet | Absolute Fehler der Treffer |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 2 | 95,60 s | 40,34 s | 29,50 s | 53,80 s | 1 / 11 | 1 | 0,14 s |
| 11 | 251,42 s | 196,28 s | 13,00 s | 186,74 s | 2 / 5 | 5 | 0,12 und 1,12 s |
| 22 | 74,10 s | 48,64 s | 1,00 s | 39,04 s | 4 / 7 | 1 | 0,16, 1,02, 0,14 und 0,18 s |
| **Gesamt** | **421,12 s** | **285,26 s (67,74 %)** | **43,50 s (10,33 %)** | **279,58 s (66,39 %)** | **7 / 23** | **7** | **Median 0,16 s** |

Die lokale Spur erreicht gegenüber dem bestmöglichen Einzelwert pro Aufnahme insgesamt 1,35 Prozentpunkte mehr korrekt bezeichnete Zeit. Sie erkennt 7 von 23 annotierten Tonartwechseln und meldet insgesamt 14 direkte Wechsel. Übergänge mit unbekannter Tonart kamen 28-mal vor. Bei den 176 Akkordwechseln mit mehr als zwei Sekunden Abstand zu einer echten Modulation trat sechsmal ein zusätzlicher Tonartwechsel in der Nähe auf. Die Ergebnisse zeigen insbesondere bei kurzen Modulationen Grenzen. Die Anzeige dient als Hörhinweis und schreibt keine globale Tonart in den Katalog.

## Bedienung und Daten

In der Library unter **Prepare → Local keys** einen Titel öffnen. Das Auswahlfeld listet Abschnitte mit Anfang, Ende, Tonart und Zuverlässigkeit. Unter **Entry (s)** und **Exit (s)** lassen sich zwei frei gewählte Positionen vergleichen. Ein leeres Ausstiegsfeld steht für das Trackende. Ein vorhandenes Beatgrid ergänzt Takt und Schlag. Ohne Grid bleiben die Quellenzeiten verfügbar.

FFT und Dateiprüfung laufen im Hintergrund. Der Katalog ab SQLite-Schema 22 speichert die Abschnitte pro Titel mit SHA-256 der Quelle und Analyseversion. Beim ersten Öffnen eines älteren Katalogs ist keine Neuanalyse der gesamten Library nötig. Pro Titel sind höchstens 4.096 Abschnitte und 512 KiB gespeicherte Analysedaten erlaubt. Die automatisierten Abläufe prüfen unter anderem den Neustart, einen Dateitausch bei identischem Pfad, identischer Größe und Änderungszeit, die anschließende Neuanalyse und den Erhalt der manuellen Track-Tonart.

Unter Windows bestanden alle 84 Tests des Bibliotheksspeichers und alle 17 Oberflächentests. Letztere prüfen in Classic und Modern die Eingabe beider Zeiten und den Schutz vor Deck-Tastenkürzeln. Beim Schließen erhält die Library ihren Platz für die Cover zurück. Vier unabhängige Python-Tests prüfen die Zuordnung enharmonischer Namen, die Bewertung von Zeitlücken, das einmalige Zuordnen von Wechseln und den Vergleich mit reinen Akkordwechseln.

## Audiolast unter Windows

Der Test `a06_equal_duration_local_key_analysis_with_two_decks` rendert zwei laufende Decks in beiden Durchgängen 30 Sekunden lang. Beide Durchgänge nutzen 3.000 Mono-Blöcke mit je 480 Frames bei 48 kHz. Im zweiten Durchgang untersucht ein separater Worker wiederholt denselben vollständigen Testtrack.

| Messgröße | Referenzlauf ohne Analyse | Mit lokaler Tonartanalyse |
| --- | ---: | ---: |
| Vollständig analysierte Testtracks | 0 | 108 |
| Renderzeit p99 / Maximum | 3,484 / 4,813 ms | 3,606 / 4,006 ms |
| Startverzögerung p99 / Maximum | 0,727 / 1,556 ms | 0,649 / 0,933 ms |
| Verfehlte Fristen | 0 | 0 |
| Fehlerhafte Signalblöcke / Render-Allokationen | 0 / 0 | 0 / 0 |

### Auswertung wiederholen

Aus dem SWD-Archiv `Schubert_Winterreise_Dataset_v2-1.zip` die drei WAV-Dateien `Schubert_D911-{02,11,22}_HU33.wav` aus `01_RawData/audio_wav` in einen gemeinsamen lokalen Ordner übernehmen. Die gleichnamigen CSV-Dateien aus `02_Annotations/ann_audio_localkey-ann1` und `02_Annotations/ann_audio_chord` daneben als `*.keys.csv` und `*.chords.csv` ablegen. Die Aufnahmen haben folgende SHA-256-Prüfsummen:

| D 911 | SHA-256 der WAV-Datei |
| --- | --- |
| 2 | `82df858661d662f162ab3bd3066925dc0faefeb3429584c152bc7b5e91c4e0d2` |
| 11 | `c573658476e203f2513a4d7b30e060e0d27a72f61d56b751e02f7c3c317e33a6` |
| 22 | `cb2d7d959980b65f3627b9e15b6f5a92a4fb584e597afcafd3cfbf2803433380` |

`DRDJPRO_HARMONY_CORPUS` auf diesen Ordner und `DRDJPRO_HARMONY_TSV` auf eine neue lokale TSV-Datei setzen. Der folgende Lauf schreibt die Vorhersagen, der zweite wertet sie unabhängig gegen die Annotationen aus (PowerShell).

```powershell
cargo test --locked --bin drdjpro-ui-spike a06_corpus_intervals_tsv -- --ignored --nocapture --test-threads=1
python -B tools/local_key_score.py $env:DRDJPRO_HARMONY_CORPUS $env:DRDJPRO_HARMONY_TSV
```
