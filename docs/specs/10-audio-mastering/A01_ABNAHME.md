# A01 · Abgeschnittene Signalspitzen prüfen

In **Library → Prepare** einen Titel wählen und **Clip check** öffnen. Die Prüfung läuft über die gesamte Datei. Die Liste zeigt Zeitbereiche mit wiederholt abgeflachten Spitzen. Ein Treffer ist ein Hinweis für die Hörprobe. Auch absichtlich verzerrte Instrumente können solche Wellenformen erzeugen.

Einen Treffer auswählen. **Original** und **Processed** spielen denselben Ausschnitt bei angeglichener Lautheit über den getrennten Kopfhörerausgang. Die Bearbeitung glättet die markierten Spitzen. Der Vergleich rekonstruiert keine vor der Aufnahme verlorenen Samples. **Stop** beendet die Hörprobe. **Close** beendet die Prüfung.

**Export WAV copy** schreibt den gesamten bearbeiteten Titel als separate Datei in einen ausdrücklich gewählten Zielordner. Die Endung des neuen Namens lautet `-clip-preview.wav`. Eine dort bereits vorhandene Datei wird geschützt. Die Lautheitsanpassung dient dem A/B-Vergleich über Kopfhörer.

## Automatisierte Gegenproben

Ein synthetischer 1-kHz-Testton enthält abgeflachte Spitzen hinter der achten Sekunde. Die Prüfung markiert den betroffenen Bereich. Ein sauberer Sinus mit derselben Spitzenhöhe, ein einzelner Peak und Stille sind die Negativkontrollen. FFmpeg hat für Original und Bearbeitung der hörbaren Testpassage unabhängig jeweils **−2,9 LUFS** gemessen. Die Differenz beträgt bei der Auflösung dieses Messers 0,0 LU. Das Abnahmekriterium liegt bei höchstens 0,5 LU.

Die Exporttests vergleichen unmarkierte Abschnitte und die Quelldatei vor und nach dem Export. Sie prüfen außerdem Abbruch, Schreibfehler, Namenskollision und die RIFF-Größengrenze. Der Controller-Test ersetzt eine Datei unter derselben Katalog-ID bei gleicher Länge und wiederhergestelltem Zeitstempel. Ein erneuter Hörklick spielt die alte Passage danach nicht ab.

Eine Referenz mit **48.137 hörbaren Frames** wurde mit FFmpeg als MP3 und als AAC/M4A kodiert. Bei beiden Dateien ergaben die unabhängige FFmpeg-Dekodierung, der Scan und der Export aus der App jeweils genau 48.137 Frames. So fließen Encoder-Vorlauf und Endpadding nicht in die angezeigten Zeiten ein. Bei AAC unterstützt die Diagnose eine zusammenhängende Editliste mit normaler Geschwindigkeit. Mehrteilige oder beschleunigte Präsentationsfolgen meldet sie als nicht unterstützt.

## Audiolast unter Windows

Gleicher Build, zwei laufende Decks bei 48 kHz, vier Kanäle und 480 Frames je Block. In jedem Lauf folgten auf 100 Aufwärmblöcke 3.000 Blöcke mit 10-ms-Frist.

| Messgröße | Diagnose aus | Diagnose und PFL aktiv |
| --- | ---: | ---: |
| Vollständige Testtrack-Scans mit A/B-Wechsel | 0 | 50 |
| Renderzeit p99 / Maximum | 6,081 / 6,590 ms | 6,372 / 7,306 ms |
| Startverzögerung p99 / Maximum | 0,754 / 1,218 ms | 0,725 / 0,955 ms |
| Verfehlte Fristen | 0 | 0 |
| Fehlerhafte Signalblöcke / Render-Allokationen | 0 / 0 | 0 / 0 |

Der separate Lasttest heißt `a01_equal_duration_clip_check_with_two_decks`. Die unabhängige Lautheitsprüfung heißt `a01_independent_ffmpeg_integrated_loudness`. Beide lassen sich mit `cargo test --locked --bin drdjpro-ui-spike TESTNAME -- --ignored --nocapture --test-threads=1` ausführen.

## Gemeinsame Abnahme am Ende

In einer bekannten Aufnahme eine markierte Stelle über den MC4000 abhören. Bei laufendem Master zwischen Original und Bearbeitung wechseln. Eine unauffällige Aufnahme zur Gegenprobe prüfen.
