# Abschlussprüfung der 35 Funktionen zur Musikvorbereitung

Windows 11 Enterprise x64 (Build 26300), Intel Core i7-11850H, Rust 1.99.1 und FFmpeg 9.0.2. Stand 24. November 2026 auf `codex/musikvorbereitung-35`.

## Ergebnis und offene Abnahmen

Alle 35 Funktionen haben einen automatisierten Windows-Nachweis. Die unveränderte Frist von 10 ms schließt das Warten ab geplantem Blockbeginn ein. Nach der Mixerkorrektur bestanden in Debug und Release jeweils zwölf Zwei-Deck-Vergleiche. Der langsamste der jeweils 72.000 Blöcke war nach 7,660 ms in Debug bzw. 3,180 ms in Release fertig. Alle Signalprüfungen bestanden und der Renderthread verzeichnete keine Allokationen.

Die musikalische Zuverlässigkeit bleibt insbesondere bei Akkordwechseln begrenzt. Die gemeinsame Hörprobe mit MC4000, eigenen Tracks und den tatsächlichen Master- und Kopfhörerausgängen sowie die Setübergabe an einem zweiten Rechner stehen aus. Unter Windows wurde der Pakettransfer zwischen C: und D: sowie zwischen zwei unabhängigen Katalogen geprüft. Die plattformübergreifende Paketprüfung gehört zur [Rust-CI des Branches](https://github.com/yovannyr/DrDJPro/actions/workflows/rust-ui.yml?query=branch%3Acodex%2Fmusikvorbereitung-35).

## Fristprüfung mit identischer Samplerlast

Bei 48 kHz und 480 Frames je Block lief in jedem frischen Testprozess derselbe Ausschnitt zunächst 30 Sekunden mit zwei Decks, dann 30 Sekunden mit hörbarem Samplerpattern, Quellenmonitor, Wellenformanalyse und Abschnittssprüngen. Alle 3.000 Lastblöcke jedes Paares enthielten den Samplerton. Das unoptimierte Anwendungsprofil (`opt-level = 0`), die Referenzsignale, die Threadpriorität und die Fristmessung waren in beiden Fassungen gleich. Die Tabelle enthält alle drei vorab vorgesehenen Wiederholungen je Stand. Die Werte jeder Zelle stehen in der Reihenfolge Referenz / Last.

| Debug-Lauf | Renderzeit p99 (ms) | Spätester Abschluss ab Sollbeginn (ms) | Fristfehler | Wellenformanalysen / Monitoraktualisierungen / Sprünge unter Last |
| --- | ---: | ---: | ---: | ---: |
| Vorher 1 | 6,277 / 8,707 | 7,759 / 9,984 | 0 / 0 | 44 / 44 / 14 |
| Vorher 2 | 6,279 / 8,566 | 7,613 / 9,669 | 0 / 0 | 42 / 42 / 14 |
| Vorher 3 | 6,656 / 8,763 | 7,671 / 10,261 | 0 / 1 | 43 / 43 / 14 |
| Nachher 1 | 3,450 / 5,543 | 4,276 / 6,580 | 0 / 0 | 44 / 41 / 14 |
| Nachher 2 | 3,531 / 5,278 | 5,966 / 6,862 | 0 / 0 | 44 / 43 / 14 |
| Nachher 3 | 3,289 / 5,824 | 4,050 / 8,278 | 0 / 0 | 43 / 43 / 14 |

Der Median der drei p99-Werte unter Last sank von 8,707 auf 5,543 ms. Der Vorher-Lauf erfasste eine Fristverletzung bei 10,261 ms. Die drei Wiederholungen auf der endgültigen Fassung ergaben über 18.000 Blöcke null Frist-, Signal- oder Allokationsfehler.

Der Master summiert die drei permanenten Quellen in gleicher Reihenfolge und im bereits gemeinsamen Audioformat. Damit entfallen im Vierkanalbetrieb je 480-Frame-Block 5.760 Durchläufe durch Rodios Formatadapter. In diesem ersten Schritt nahmen die dynamischen Deckmixer weiterhin neue Titel auf. Jede Delay-Stufe berechnet ihre Ringposition einmal pro Frame und pflegt auch im ausgeschalteten Zustand die Echohistorie. Unveränderte EQ-Einstellungen benötigen einen Versionsabruf. Beim Eintritt ins Kopfhörervorhören werden die betroffenen Kanalhistorien geleert. Die neuen Regressionen vergleichen die Summe bitgenau mit Rodio, prüfen die Aufrufreihenfolge auf 1, 2, 4 und 32 Kanälen und führen Laden, Auswerfen, Ringumlauf und Filterwechsel über den Produktionspfad aus.

Ein Zwischenstand mit diesen Mixer- und Effektänderungen bestand drei gezielte Wiederholungen, verfehlte in der breiteren Debug-Reihe aber 33 von 72.000 Fristen, darunter eine im Referenzlauf der Samplerprüfung. Der späteste Blockabschluss lag bei 14,095 ms im Video-Referenzlauf. Die Signal- und Allokationsprüfungen blieben grün. Der Stand wurde nicht veröffentlicht.

Im zweiten Schritt ersetzte ein direkter Deckausgang den bisherigen Transportadapter. Ein atomares Play/Pause-Gate greift an der Framegrenze und friert bei Pause die inneren Audiodaten, den Deck-EQ und den Quellpegel ein. Die äußeren Effekte führen ausklingende Echos weiter. Der allgemeine Rodio-Sink mit seinen periodischen Sperren für hier ungenutzte Transportfunktionen entfällt. Die PCM-Abtastung berechnet die Interpolationsposition gemeinsam für alle Kanäle eines Frames. Im normalen Loopabschnitt entfällt die 128-Bit-Restwertberechnung. Bei einem Überschreiten der Loopgrenze gilt weiterhin die vollständige Umlaufrechnung. Regressionen prüfen Pause und Wiederaufnahme, Befehle mitten im Frame, Quellenfreigabe sowie Interpolation gegen die frühere breite Rechnung bei unterschiedlichen Raten, Loopgrenzen, Dateiende und großen Phasenwerten.

Ein bestehender Positionstest deckte beim vollständigen Durchlauf eine Regression auf. Nach einem expliziten Sprung ans Titelende schloss das Gate, bevor der Renderer die neue Position bestätigen konnte. Der Fix lässt genau einen vollständigen Frame zur Bestätigung durch die innere Kette laufen. Master und Kopfhörer geben dabei Stille aus, auch bei aktivem Filter. Ein weiterer Sprung mitten im Frame macht die ältere Bestätigung ungültig. Regressionen prüfen die letzte bestätigte Position, erneuten Seek, Neustart, einen überholenden Spielbefehl und alle vier Ausgangskanäle.

Nach dieser Korrektur meldeten drei unoptimierte Sampler-Vergleiche fünf verfehlte Fristen in 18.000 Blöcken (eine in der Referenz, vier unter Last). Der späteste Abschluss lag bei 13,184 ms. Die Signale und die Speicherprüfung des Renderthreads blieben fehlerfrei. Windows protokollierte für diesen Zeitraum eine Modern-Standby-Sitzung von 19:19 bis 21:55 Uhr. Im regulären Betrieb nach dem Aufwecken wurde dieselbe Programmdatei vor der nächsten Änderung erneut dreimal geprüft. Der Zustand allein erklärt die einzelnen Ausreißer nicht.

Im dritten Schritt bekam jedes Deck einen Eingang im festen Geräteformat. Es übernimmt den neuesten Titel an einer Framegrenze. Bei gleichzeitigem Zugriff spielt es den aktuellen Frame weiter und prüft den Wechsel am nächsten Frame erneut. Die Bedienseite gibt abgelöste Quellen frei. Die beiden Eingänge sparen im Vierkanalbetrieb je 480 Frames zusammen 3.840 Durchläufe durch Rodios dynamischen Mixer einschließlich Formatadapter. Alle sechs Delay-Stufen behalten ihre eigene Echohistorie. Der Audiothread besitzt die Puffer jetzt exklusiv und führt pro Block 23.040 atomare PCM-Lese- und Schreibzugriffe weniger aus. Die drei Effektstufen jedes Decks nutzen pro Frame denselben Kopfhörerstatus. Regressionen prüfen schnelle Titelwechsel, Auswerfen, Geräteabbau, verzögerte Speicherfreigabe, Kanalgrenzen und PFL bei laufenden Filter- und Echoeffekten.

**Gegenvergleich am durchgehend wachen Rechner.** Für diese Messreihe blieben Rechner und Anzeige über eine temporäre Windows-Anforderung aktiv. Energieprofil und Threadpriorität waren gleich. Jeder Stand durchlief dreimal 30 Sekunden Referenz und 30 Sekunden Last. Das Samplerpattern war in allen 9.000 Lastblöcken pro Stand nachweisbar. Die 9.000 Referenzblöcke enthielten nur die beiden Decks. Der Median der aktiven p99-Renderzeit ging von 6,351 auf 5,543 ms zurück. Die Werte jeder Zelle stehen in der Reihenfolge Referenz / Last.

| Lauf | p99 vor Schritt 3 (ms) | p99 nach Schritt 3 (ms) | Spätester Abschluss vorher (ms) | Spätester Abschluss nachher (ms) |
| --- | ---: | ---: | ---: | ---: |
| 1 | 4,077 / 6,351 | 3,450 / 5,543 | 4,905 / 7,333 | 4,276 / 6,580 |
| 2 | 4,454 / 5,937 | 3,531 / 5,278 | 6,448 / 7,718 | 5,966 / 6,862 |
| 3 | 3,986 / 6,444 | 3,289 / 5,824 | 7,291 / 8,466 | 4,050 / 8,278 |

Unabhängiges Code-Review der Korrektur durch **GPT-6.1 Sol, Denkaufwand Mittel**. Der Reviewer verglich die tatsächliche Rodio-0.21.1-Implementierung und prüfte Rundung, Umschaltgrenzen, Signalführung und Ressourcen im Renderpfad. Es blieben keine blockierenden Befunde.

**Abschlussprüfungen unter Windows.** 2.676 Rust-Tests und 33 Python-Tests bestanden. 32 gesondert gekennzeichnete Tests waren im regulären Workspace-Lauf ausgenommen. Der separate Testlauf mit aktivierter xwax-DVS-Funktion bestand 10 DVS-Tests. Die zwölf Lastvergleiche je Profil liefen anschließend explizit. Formatierung, Clippy für alle Workspace-Ziele mit Warnungen als Fehlern und der optimierte Anwendungsbuild bestanden. Die ausführbare Datei umfasst 53.150.720 Bytes. Der vollständige Rust-Lauf einschließlich Kompilierung dauerte 612,11 s. Die beobachteten Spitzen des Prozessbaums und der isolierten temporären Dateien im 100-ms-Raster betrugen 3.089.993.728 bzw. 44.337.472 Bytes. Alle lokalen Builds nutzten ein gemeinsames Buildverzeichnis.

| Endgültiger Stand · zwölf Paarvergleiche | Debug Renderzeit p99 (ms) | Debug spätester Abschluss (ms) | Release Renderzeit p99 (ms) | Release spätester Abschluss (ms) |
| --- | ---: | ---: | ---: | ---: |
| A06 · Lokale Tonart | 1,971 / 2,055 | 3,598 / 3,951 | 0,729 / 0,704 | 1,851 / 1,912 |
| D01 · Akkordvergleich | 3,036 / 3,503 | 4,251 / 4,563 | 1,472 / 1,526 | 2,587 / 2,682 |
| A01 · Clipping | 2,983 / 3,328 | 4,381 / 5,126 | 1,399 / 1,565 | 2,540 / 2,756 |
| R02 · Auftrittswünsche | 3,033 / 3,431 | 4,307 / 4,666 | 1,334 / 1,349 | 2,745 / 2,698 |
| R04 · Import | 3,185 / 3,400 | 6,001 / 5,024 | 1,571 / 0,522 | 3,180 / 1,785 |
| A07 · Referenzpiano | 3,170 / 3,227 | 4,520 / 4,815 | 1,471 / 1,111 | 2,692 / 2,553 |
| R05 · Metadaten | 3,184 / 2,993 | 4,909 / 4,665 | 0,552 / 1,133 | 1,199 / 2,638 |
| D06/A03/A05/M07/M08 · Sampler, Sprung und Analyse | 3,203 / 5,673 | 5,336 / 7,660 | 0,744 / 0,876 | 2,625 / 2,593 |
| M01 · Sample-Vorhören | 3,065 / 3,207 | 4,788 / 4,912 | 0,869 / 0,519 | 2,026 / 1,207 |
| D04 · 85/170 BPM | 3,163 / 3,117 | 4,596 / 5,397 | 0,534 / 0,576 | 1,283 / 1,159 |
| D07 · WAV-Export | 2,939 / 3,319 | 3,674 / 4,661 | 0,656 / 0,932 | 1,826 / 2,986 |
| R07/M03/M04 · Video und MIDI | 3,052 / 3,688 | 4,270 / 5,101 | 0,648 / 0,656 | 1,602 / 2,095 |

## Prüfumfang

Die Prüfungen verwenden die Produktionslogik mit temporären Bibliotheken, generierten Audiosignalen und einer simulierten Audioausgabe. UI-Tests bedienen die Slint-Komponenten in Classic und Modern. Die Hörprobe am physischen Controller, mit eigenen Aufnahmen sowie an den tatsächlichen Master- und Kopfhörerausgängen erfolgt anschließend gemeinsam.

| Rang | Funktion | Automatisierte Gegenprobe |
| ---: | --- | --- |
| 1 · D04 | 85/170 BPM | Gleicher Quellausschnitt, unterschiedliche Klick-/Taktakzente, gesperrtes Grid und Bestätigung |
| 2 · D03 | Coverbeschriftung | Tonart/Energie im Katalog, unbekannte Daten, Quelle und Neustart |
| 3 · R04 | Importvergleich | Identische und verschiedene Dateien, bestätigte Auswahl und veränderte Quelle |
| 4 · R05 | Metadatendiagnose | Herkunft der BPM-/Tonartwerte, Halb-/Doppeltempo und Schreibschutz |
| 5 · R02 | Auftrittswünsche | Getrennte Auftritte, Ausschlüsse, bestätigter Play und Neustart |
| 6 · A07 | Referenzpiano | Grundton und Dreiklang, Kopfhörerroute, Übernahme und Rücknahme |
| 7 · M01 | Sample-Browser | 20 Vorhörwechsel, belegte Pads und Cachegrenze |
| 8 · A01 | Clipping | Plateaus gegenüber unverzerrten Spitzen, Lautheitsabgleich, getrennte Kopie und Quellschutz |
| 9 · A06 | Lokale Tonart | Kadenz, Wechselposition, Stille, Quellenwechsel und drei annotierte Aufnahmen |
| 10 · D01 | Akkordvergleich | Viertaktfolge, verschobener Einstieg, Quellenkorrektur und sechs annotierte Aufnahmen |
| 11 · D05 | Cue-Feinschnitt | Referenzcue, freier Frame einschließlich Trackende, Speicherung und Konflikt |
| 12 · D06 | Abschnittssprung | Taktgrenze und Verlauf der 12-ms-Blende, Widerruf vor dem Sprung |
| 13 · A08 | Transposition bei Vorschlägen | Gleicher Seed, Halbtonbegrenzung und unbekannte Tonart |
| 14 · R03 | Brückentitel | Tempobedingungen an beiden Übergängen und zugehörige Cue-Zeiten |
| 15 · R06 | Crate ordnen | Feste Positionen 1/7/12, Wiederholung, konkurrierende Änderung und Neustart |
| 16 · R01 | Wortspielmarken | Zwei Trackpositionen, Änderungen, Löschen und Wiederherstellen aus der Sicherung |
| 17 · A05 | Wellenformschichten | Kick und Synth einzeln und gemischt, Stille und verspätetes Analyseergebnis nach Auswahlwechsel |
| 18 · A02 | Lautheitsvergleich | Gleiches Signal, Differenz von 6 dB und fehlender Drum-Stem |
| 19 · A03 | Quellenmonitor | Bekannte Tonart und Tempi, Stille, Einzelton, Rückstau und Wiederaufnahme |
| 20 · A04 | Sample stimmen | Frequenz und Phase eines laufenden Pads, Feinstimmung und Aliasunterdrückung |
| 21 · M02 | Sample-Layer | Stereokanal, relativer Pegel und 20 ms Startversatz |
| 22 · M07 | Tonale Pads | Vorschau, explizite Aktivierung und Rückkehr zur vorherigen Belegung |
| 23 · M09 | Pad-Phrase | Ereigniszeiten, verspätete Zustellung, simulierter MC4000-Verbindungsabbruch, Bestätigung und Neustart |
| 24 · M08 | Fill | Takt vier, Rückkehr ins Grundmuster, Widerruf und nächste Runde |
| 25 · R08 | Gesangsüberlappung | Zwei Stem-Signale, Versatz, Stille und schwache Anteile |
| 26 · D08 | Spielbares Setpaket | Medienidentität, gleichnamige Titel, wiederholter Eintrag und beschädigter Inhalt |
| 27 · M06 | Übergabe an einen zweiten DJ | Zwei SQLite-Kataloge mit kollidierenden IDs, selektive Beilage, manuelle Zuordnung, Verwerfen, Abbruch und Bestätigung |
| 28 · D07 | Set-Timeline | Drei Clips, überlappende Signalpegel, Automation, Exportfehler und ältere Zieldatei |
| 29 · D09 | Hook-Übung | Zwei geladene Titel, wiederverwendetes Pad und tatsächlich gemessene Wechselzeit |
| 30 · D02 | Akkord-MIDI | Bestätigter Ausschnitt, Pausen und musikalisches Ende bei erneutem Einlesen |
| 31 · M05 | Sampler-Groove | Gekoppelte Ereignisse, Intensität und unveränderte übrige Spuren |
| 32 · M03 | Audio zu MIDI | Acht Noten, gültige Stille, unvollständiges Quellintervall und MIDI-Reimport |
| 33 · M04 | Begleitung | Durchgehende Loop-Zeit beim Umschalten/Stummschalten und Export der gewählten Stimme |
| 34 · R09 | Skalenwechsel | Chromatische Entscheidungen, geschützte Drumspur und Rücknahme |
| 35 · R07 | Videoexport | 30 Sekunden mit 900 Bildern, Titelwechsel und Beat-Anzeige, unabhängiges Einlesen mit FFmpeg |

## Korrekturen aus der Prüfung

- Beim Wechsel des ausgewählten Library-Tracks leert Prepare die alten Wellenformschichten sofort. Ein bereits fertiges Ergebnis des vorherigen Titels kann sie nicht erneut anzeigen. Der Regressionstest prüft auch laufende Wiedergabe, Cues und Zoom.
- Stumme Sampler-Pads benötigen pro Frame nur eine gemeinsame Prüfung auf neue Befehle. Beim Transponieren verwendet der Filter im Inneren des Clips ein zusammenhängendes Fenster. An beiden Clipenden gilt dieselbe Randbehandlung. Der Test vergleicht die Ausgabe bitgenau mit der skalaren Referenz.
- Das unabhängige Review fand eine Lücke beim gleichzeitigen Ersetzen und Lesen von Sampledaten. Bankwechsel, PCM-Zugriffe und abschließende Gültigkeitsprüfung haben jetzt eine gemeinsame atomare Reihenfolge. Das gilt für Live-Wiedergabe und Kopfhörervorschau. Der Renderpfad wartet dabei auf keinen Ladeauftrag.
- Die sechs Delay-Stufen beider Decks lesen pro Frame zuerst die Versionsnummer ihrer Einstellungen. Nur eine neue, vollständig veröffentlichte Version erfordert die vier einzelnen Werte und die anschließende Versionsprüfung. Bei unveränderten Werten entfallen so 14.400 atomare Lesezugriffe pro 480-Frame-Block. Der Ringpuffer führt die Echo-Historie auch bei ausgeschaltetem Effekt weiter.
- Die Prepare-Tabelle erhält bei eingeblendeten Wellenformwerkzeugen weiterhin Platz für ihre Spaltenköpfe und mindestens eine Titelzeile. Bei den UI-Gegenproben werden Feldänderungen vor dem folgenden Klick über den regulären Slint-Ereignisablauf zugestellt.

Das unabhängige Abschlussreview mit **GPT-6.1 Sol, Denkaufwand Mittel**, deckte den Funktionsumfang und die anschließenden Korrekturen ab. Nach Prüfung der Sampler- und Delay-Korrekturen, der UI-Regressionen, des zusätzlichen Video-/MIDI-Lastfalls und der Windows-Zyklusmessung meldete der Reviewer keine offenen blockierenden Codebefunde. Die nachfolgenden Zahlen stammen aus ausgeführten Tests.

## Musikalische Treffer

Auf den drei Kunstliedern ordnete A06 285,26 von 421,12 annotierten Sekunden der korrekten lokalen Tonart zu (67,74 %). 43,50 Sekunden blieben unbekannt. Sieben von 23 Tonartwechseln wurden im festgelegten Zeitfenster getroffen. Bei D01 reichte der korrekt bezeichnete Zeitanteil je Genre von 6,25 % bis 30,70 %. Die Messung verwendet die vor der Auswertung festgelegten Dateien. [A06-Verfahren](33-passage-harmony/A06_ABNAHME.md) · [D01-Ergebnisse nach Genre](33-passage-harmony/D01_ABNAHME.md)

## Signale, Dateien und Speicher

- **Clipping und Drum-Pegel.** Die getrennte EBU-R128-Messung mit FFmpeg ergab für die ursprüngliche und die bearbeitete Hörprobe jeweils −2,9 LUFS. Beim A02-Prüfmix betrug die Differenz im Gesamtmix 0,00 LU und beim Drum-Stem +6,02 LU.
- **Sampler.** Ein 18-kHz-Ton bei 48 kHz Abtastrate prüfte die Oktavtransposition nach oben. Die Spitze des unerwünschten Signals betrug nach der Korrektur 0,000258 im Live-Pfad und 0,000129 im Kopfhörerpfad. Der 2-kHz-Kontrollton blieb mit Spitzen von 0,488336 bzw. 0,244168 erhalten. Alle Angaben sind normierte digitale Amplituden.
- **Quellenmonitor und Wellenform.** Mit gefüllter Monitorwarteschlange (135.808 Bytes Blockdaten) kamen alle 17.280 geprüften Stereo-Frames unverändert am Ausgang an. Fünf Analyseblöcke wurden verworfen. Nach dem Leeren setzte die Analyse bei Sequenz 133 wieder ein. Die Renderprüfung erfasste 0 zusätzliche Heap-Bytes. Bei der separaten Wellenformanalyse mit 256 Ausgabepunkten betrug die gemessene Heap-Spitze des Workers 111.772 Bytes.
- **Gesangsüberlappung.** Zwei vorbereitete Stereo-Stems ergaben das erwartete Fenster von 6 bis 8 Sekunden. Die zeitlich getrennten Testimpulse erreichten auf beiden Kopfhörerkanälen jeweils 0,076904 Spitze. Alle 9.600 gleichzeitig gemessenen Master-Frames stimmten bitgenau mit der unabhängigen Referenz überein.
- **WAV.** Zwei unabhängige Renderläufe desselben Projekts ergaben byteidentische Dateien mit je 36.000 Stereo-Frames und 144.044 Bytes.
- **Setpaket.** Das Paket mit drei Einträgen und zwei gleichnamigen Aufnahmen umfasste 3.183 Bytes. Nach dem Transfer von C: nach D: unter Windows blieb der doppelte Eintrag erhalten. Ein geändertes Medienbyte wurde erkannt. Bei einem weiteren Kopiervorgang lagen auf dem Ziel 80.583 Bytes im fertigen Paket und 65.536 Bytes in der angefangenen Kopie. Nach dem Abbruch waren dort nur noch die 80.583 Bytes des überprüften Altpakets vorhanden. Eine separate Gegenprobe injizierte beim Schreiben einen Platzmangel. Die Byteangaben beschreiben Dateiinhalte, die Dateisystem-Blockbelegung kann abweichen.
- **Portabilität der Prüfdaten.** Der reguläre Test prüft auf jedem Betriebssystem denselben SHA-256-Wert des Manifests (`4b7398450b21d5e1aa1373b8dd2277cddcff7c65c672f3c59c58a5e4b5096468`). Darin sind beide Medien mit Größe und SHA-256 festgelegt. Das Öffnen prüft die tatsächlichen Dateien, Reihenfolge, Wiederholung und Cue-Zeiten.
- **Video.** Der 30-s-Export ergab eine 562.228 Byte große H.264-/AAC-Datei mit 900 Bildern. Der unabhängig dekodierte Ton wechselte bei 12.000 ms die Polarität, der Titel bei Bild 360. Der gemessene Versatz betrug 0,000 Bilder. Die Prüfung las die Beat-Markierungen bei Bild 0, 360 und 885. Ein zweiter Export wurde bei laufendem Encoder abgebrochen und der fertige Clip anschließend geprüft.
- **Speicher beim Video-Test.** Mit zusätzlicher AAC-Dekodierung und Abbruchprüfung erreichte der Windows-Testprozess einen maximalen Working Set von 76.136.448 Bytes. Die größte im 100-ms-Raster beobachtete Summe der Working Sets des Tests und seiner Kindprozesse betrug 163.262.464 Bytes. Diese Summe kann gemeinsam genutzte Speicherseiten mehrfach enthalten. Im isolierten temporären Ordner wurden maximal 6.802.408 Bytes Dateiinhalte beobachtet. Kurzlebige Spitzen zwischen den Stichproben sind möglich.

## Audiofristen

Je Bedingung laufen zwei Decks bei 48 kHz für 30 Sekunden. Die 3.000 Blöcke enthalten jeweils 480 Frames. Die 10-ms-Frist beginnt beim vorgesehenen Blockstart und schließt Startverzögerung und Renderzeit ein. Eine Fristverletzung, ein fehlerhafter Signalblock oder eine Heap-Allokation im Renderpfad lässt die Prüfung scheitern.

**Erster Durchlauf zur Fehlersuche im regulären Testprofil.** Beim inaktiven A06-Vergleich kam der Renderthread einmal bereits 11,862 ms zu spät zum Start. Bei D01 verfehlte der Referenzlauf einmal die Frist, obwohl jeder einzelne Renderdurchlauf unter 10 ms blieb. Beim kombinierten Test mit Quellenmonitor, Abschnittssprüngen, Wellenformanalyse und tonalem Pattern ergab sich darüber hinaus ein Mehrbedarf im Renderpfad:

| Erste Messung, vier Ausgänge | Renderzeit p99 / Maximum | Startverzug p99 / Maximum | Renderzeit über 10 ms | Verfehlte Fristen | Fehlerhafte Signalblöcke / Render-Allokationen |
| --- | ---: | ---: | ---: | ---: | ---: |
| Referenz | 7,474 / 8,751 ms | 0,695 / 1,639 ms | 0 | 0 | 0 / 0 |
| Mit Zusatzlast | 10,511 / 11,012 ms | 3,818 / 7,951 ms | 142 | 385 | 0 / 0 |

Der erste Test ließ das Pattern laufen und setzte seinen Ausgangspegel auf null, damit die Prüfsumme des Masters konstant blieb. Er erfasste 41 Wellenformanalysen, 41 Monitor-Aktualisierungen und 14 ausgeführte Abschnittssprünge. In einzelnen anderen Szenarien beendete die damalige Testfassung den Vergleich vor dem Lastdurchgang, sobald die Referenz fehlgeschlagen war. Die gemeinsame Prüfung wertet deshalb beide Phasen erst nach dem zweiten Durchgang aus.

### Gegenprobe nach der ersten Samplerkorrektur

Das reguläre Testprofil führte das hörbare Pattern durch alle 3.000 aktiven Blöcke. Im kombinierten Fall sank die Renderzeit p99 von 10,511 auf 9,179 ms. Die Referenz hatte 0 Fristfehler, der Lastlauf noch 8 (Rendermaximum 10,108 ms, spätester Blockabschluss 10,613 ms). Alle Signal- und Allokationsprüfungen bestanden. Beim Sample-Browser überschritt genau ein Referenzblock die Frist mit 10,231 ms, während der Lastlauf vollständig innerhalb der Frist blieb.

Ein weiterer Debug-Vergleich am Referenzpiano wurde durch Windows Modern Standby unterbrochen. Der langsamste Referenzblock dauerte rund 1.242 Sekunden. Windows protokollierte das Verlassen des modernen Standbys um 13:48 Uhr (Kernel-Power, Ereignis 507). In diesem unterbrochenen Vergleich meldete der Test 395 Fristfehler in der Referenz und 12 unter Last. Bei der Wiederholung mit temporär wachgehaltenem Rechner waren beide Phasen fehlerfrei. Der späteste Abschluss lag bei 8,028 bzw. 7,483 ms. Im separaten Debug-Vergleich mit Video und MIDI gab es 13 Fristfehler in der Referenz und 9 unter Last. Schon in der Referenz wurde ein Block erst 12,970 ms nach dem geplanten Beginn gestartet.

Eine weitere Änderung speichert links und rechts zusammen in einem atomaren 64-Bit-Wert. Damit liest der Sampler pro 32-Tap-Filterausgabe 32 atomare Werte. Im gezielten Debug-Vergleich mit hörbarem Pattern lag die Renderzeit p99 bei 7,023 ms in der Referenz und 9,016 ms unter Last. Die Frist wurde 0 bzw. 4 Mal verfehlt. Der langsamste Lastblock benötigte ab geplantem Start 10,812 ms, davon 10,184 ms Renderwallzeit. Die neue Windows-Messsonde erfasste für diesen Block 25.373.088 Threadzyklen. Das Abfragepaar dauerte im Median 300 ns. Die Zyklen des Renderthreads haben bei variabler Taktfrequenz keine feste Dauer.

Im optimierten Zwischenstand nach der ersten Samplerkorrektur lagen alle 72.000 Blöcke innerhalb der Frist. Der späteste Abschluss lag bei 4,000 ms. Dieses Ergebnis dient dem Vergleich mit der folgenden Abschlussmessung.

### Frühere Messung im regulären Testprofil vor der Mixerkorrektur

Die folgenden zwölf Vergleichspaare liefen nacheinander auf demselben wachgehaltenen Rechner. Die Anwendung ist in diesem Cargo-Testprofil unoptimiert (`opt-level = 0`). Bei 72.000 Blöcken wurde die Frist einmal unter kombinierter Last verfehlt. Über die gesamte Reihe meldeten die Signalprüfungen 0 Fehler und der Renderthread 0 Allokationen.

| Funktion | Render p99 / Maximum, Referenz (ms) | Render p99 / Maximum, Last (ms) | Spätester Blockabschluss Referenz / Last (ms) | Fristfehler Referenz / Last |
| --- | ---: | ---: | ---: | ---: |
| A06 · lokale Tonarten | 2,980 / 4,854 | 2,975 / 3,931 | 5,422 / 4,287 | 0 / 0 |
| D01 · Akkordvergleich | 5,487 / 6,244 | 7,143 / 8,998 | 6,443 / 9,361 | 0 / 0 |
| A01 · Clipping | 5,754 / 6,163 | 6,260 / 7,049 | 6,635 / 7,357 | 0 / 0 |
| R02 · Auftrittswünsche | 5,354 / 6,185 | 5,748 / 8,066 | 6,429 / 8,556 | 0 / 0 |
| R04 · Import | 5,355 / 6,102 | 6,073 / 6,495 | 6,538 / 6,823 | 0 / 0 |
| A07 · Piano | 5,568 / 6,021 | 5,442 / 6,286 | 6,511 / 6,536 | 0 / 0 |
| R05 · Metadaten | 5,451 / 6,944 | 5,439 / 6,005 | 7,052 / 6,633 | 0 / 0 |
| D06/A03/A05/M07/M08 · Sprung, Analyse und Sampler | 5,868 / 6,951 | 7,857 / 10,873 | 7,188 / 11,100 | 0 / 1 |
| M01 · Sample-Vorhören | 5,534 / 8,496 | 5,457 / 8,439 | 8,815 / 8,952 | 0 / 0 |
| D04 · 85/170 BPM | 5,675 / 6,450 | 5,857 / 6,299 | 6,738 / 6,714 | 0 / 0 |
| D07 · WAV-Export | 5,607 / 6,278 | 5,731 / 6,242 | 6,379 / 7,309 | 0 / 0 |
| R07/M03/M04 · Video und MIDI | 5,831 / 6,448 | 6,643 / 8,668 | 6,688 / 8,757 | 0 / 0 |

Der verspätete Block begann 0,226 ms nach seinem Sollbeginn und benötigte 10,873 ms zum Rendern. Einschließlich Startverzug meldete der Test 11,100 ms (Zeiten einzeln auf Mikrosekunden gekürzt). Seine 17.410.104 Threadzyklen lagen unter dem 99. Perzentil des Lastlaufs von 19.491.264 Zyklen. Die Zählung allein klärt die Ursache der längeren verstrichenen Zeit nicht. Das unabhängige Nachreview fand im geprüften Sampler- und Filterpfad keine weitere belegte Fehlerursache.

Das Pattern war in allen 3.000 Lastblöcken messbar. Gleichzeitig liefen 44 Wellenformanalysen, 43 Monitor-Aktualisierungen und 14 Abschnittssprünge. Im Video-/MIDI-Vergleich entstanden während der aktiven Messphase 9 vollständige Videos und 29 MIDI-Szenen. Der höchste beobachtete Speicherverbrauch eines Testprozessbaums lag bei 153.542.656 Bytes, das größte beobachtete Volumen temporärer Dateien bei 7.596.849 Bytes.

### Separate Ursachenmessung unter Windows

Für genau einen zusätzlichen kombinierten Paarvergleich hielt ein isolierter Testbuild die Windows-Threadprofilierung während jeder Messphase aktiv und las die Zähler vor und nach jedem Renderblock. Die Sonde erfasste ausschließlich den eigenen Audiotestthread. Beide Durchläufe meldeten die Freigabe des Profiling-Handles. Die zusätzlichen Abfragen sind in den Zeiten enthalten. Dieser Vergleich wird separat gezählt.

| 3.000 Blöcke je Phase | Renderzeit p99 / Maximum | Spätester Abschluss | Rendern länger als 10 ms | Verfehlte Fristen | Signalfehler / Render-Allokationen |
| --- | ---: | ---: | ---: | ---: | ---: |
| Referenz | 6,434 / 7,100 ms | 7,562 ms | 0 | 0 | 0 / 0 |
| Kombinierte Last | 9,467 / 10,499 ms | 12,708 ms | 13 | 23 | 0 / 0 |

In 20 der 23 verspäteten Blöcke erfasste Windows während des Renderns keinen Threadwechsel, in den drei übrigen je einen. 17 verspätete Blöcke benötigten mehr Threadzyklen als das 99. Perzentil der Lastphase. Der Block mit dem spätesten Abschluss begann mit 2,606 ms Rückstand und renderte 10,101 ms. Das spricht für knappe Rechenreserve im unoptimierten Pfad. Der Zeitpunkt der Messung und die zusätzlichen Profiling-Abfragen unterscheiden sich vom regulären Lauf. Die Daten weisen weder einen einzelnen Funktionsaufruf als Ursache aus noch erklären sie rückwirkend den einen ursprünglichen Ausreißer.

**Einzelmessung des Samplerfilters.** Ein eigenständiger Rust-Vergleich berechnete 480 Frames mit drei dauerhaft hochtransponierten Stimmen und identischen Eingangsdaten. Die vorhandene 32-Tap-Faltung brauchte im Median 0,639 ms unoptimiert und 0,0468 ms optimiert. Bei identischer Summenfolge und denselben atomaren Stereo-Lesezugriffen sparte die schnellste Variante mit vollständig ausgeschriebener Schleife im gepaarten Median 0,1323 ms bzw. 0,00278 ms. Unoptimiert lag die Ersparnis in allen 16 Runden zwischen 0,110 und 0,155 ms. Alle drei untersuchten Varianten ergaben je Profil bei 21.504 Vergleichen und in sämtlichen gemessenen Blöcken bitgleiche Ergebnisse. Dieser Aufwand erklärt die fehlende Zeitreserve im Gesamtsystem nur zum Teil. Die zusätzliche Schleifenentrollung wurde deshalb nicht übernommen.

**Stichproben im vollständigen Renderpfad.** LLDB erfasste in einem weiteren Diagnosepaar 488 zeitlich versetzte Aufrufstapel des eigenen Testprozesses. Nach Ausschluss von Schlaf-, Einrichtungs- und Kontrollphasen blieben 86 Renderstichproben in der Referenz und 123 unter Last. Dem Delay waren unmittelbar 14 bzw. 22 Treffer zuzuordnen, dem Equalizer 12 bzw. 13 und der Sampler-Faltung 0 bzw. 8. Acht Lasttreffer entfielen auf das Lesen des Delay-Ringpuffers einschließlich seiner Bibliotheksaufrufe. Drei Effektstufen pro Deck pflegen die Echohistorie auch bei ausgeschaltetem Effekt, damit beim Zuschalten die vorangegangenen Samples verfügbar sind. Die Stichproben weisen keinen einzelnen entbehrlichen Verarbeitungsschritt mit gesicherter Einsparung von einer Millisekunde aus. Der Debugger hielt den Prozess zur Aufnahme im Median 15 ms an. Fristwerte aus diesem Vergleich eignen sich deshalb nicht zur Abnahme. Beide Phasen meldeten 0 Signalfehler und 0 Render-Allokationen.

### Frühere Messung im optimierten Windows-Build vor der Mixerkorrektur

Alle zwölf Paare bestanden mit demselben Quellstand und denselben Eingangssignalen wie im regulären Testprofil. Alle 72.000 Blöcke waren rechtzeitig fertig. Der langsamste Block war der 85/170-BPM-Wechsel mit 4,123 ms ab Sollbeginn. Es gab 0 ungültige Signalblöcke und 0 Allokationen im Renderthread.

| Funktion | Render p99 / Maximum, Referenz (ms) | Render p99 / Maximum, Last (ms) | Spätester Blockabschluss Referenz / Last (ms) | Fristfehler Referenz / Last |
| --- | ---: | ---: | ---: | ---: |
| A06 · lokale Tonarten | 0,881 / 1,211 | 0,860 / 1,703 | 2,055 / 2,105 | 0 / 0 |
| D01 · Akkordvergleich | 2,072 / 2,800 | 2,052 / 2,722 | 3,238 / 3,382 | 0 / 0 |
| A01 · Clipping | 1,790 / 2,843 | 2,193 / 3,196 | 3,280 / 3,796 | 0 / 0 |
| R02 · Auftrittswünsche | 2,053 / 2,637 | 2,013 / 2,731 | 3,406 / 3,273 | 0 / 0 |
| R04 · Import | 2,066 / 2,755 | 1,001 / 1,173 | 3,198 / 2,024 | 0 / 0 |
| A07 · Piano | 2,263 / 3,355 | 2,038 / 3,482 | 3,861 / 3,889 | 0 / 0 |
| R05 · Metadaten | 1,915 / 2,791 | 2,116 / 3,141 | 3,266 / 3,861 | 0 / 0 |
| D06/A03/A05/M07/M08 · Sprung, Analyse und Sampler | 1,844 / 2,460 | 2,252 / 2,804 | 2,938 / 3,389 | 0 / 0 |
| M01 · Sample-Vorhören | 2,070 / 2,780 | 2,211 / 2,750 | 3,481 / 3,400 | 0 / 0 |
| D04 · 85/170 BPM | 2,218 / 3,321 | 2,127 / 3,270 | 3,590 / 4,123 | 0 / 0 |
| D07 · WAV-Export | 2,216 / 2,705 | 2,123 / 3,631 | 3,317 / 4,097 | 0 / 0 |
| R07/M03/M04 · Video und MIDI | 2,000 / 3,109 | 1,157 / 2,198 | 3,657 / 2,648 | 0 / 0 |

Im kombinierten Lastfall enthielten alle 3.000 aktiven Blöcke den Samplerton. Parallel wurden 57 Wellenformanalysen, 44 Monitor-Aktualisierungen und 14 ausgeführte Abschnittssprünge gezählt. Während des separaten Exportvergleichs wurden 14 vollständige 30-s-Videos und 29 MIDI-Szenen fertig. Im 100-ms-Raster wurden für den größten Testprozessbaum 164.134.912 Bytes und für die temporären Testdateien bis zu 7.596.849 Bytes beobachtet.

### Messung wiederholen

Die folgenden Befehle führen alle zwölf Paare nacheinander aus. Für die hier protokollierte Messreihe startete dieselbe Testdatei je Paar in einem frischen Prozess mit eigenem temporärem Ordner. Beide Phasen eines Paares liefen im selben Prozess. Für den kombinierten Video-/MIDI-Fall müssen FFmpeg mit libx264 und AAC sowie FFprobe im Suchpfad liegen. Während der Zeitmessung andere Builds beenden.

```powershell
cargo test --release --locked --bin drdjpro-ui-spike equal_duration -- --ignored --nocapture --test-threads=1
cargo test --locked --bin drdjpro-ui-spike equal_duration -- --ignored --nocapture --test-threads=1
```

Für die drei gezielten Wiederholungen im unoptimierten Profil führt dieser Befehl das unveränderte Sampler-Szenario in drei getrennten Prozessen aus. Bei der Abnahme liefen während der Zeitmessung keine Builds.

```powershell
1..3 | ForEach-Object {
    cargo test --locked --bin drdjpro-ui-spike playback::tests::preparation_performance::equal_duration_live_monitor_waveform_and_sampler_tools -- --exact --ignored --nocapture --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "Sampler-Fristprüfung fehlgeschlagen" }
}
```

Für den vollständigen Windows-Funktionstest `SLINT_EMIT_DEBUG_INFO=1` setzen. Für den Datenträgerwechsel bei D08 `DRDJ_TEST_PACKAGE_DESTINATION` auf den vorhandenen zweiten Datenträger setzen. Die Tests legen dort einen eigenen temporären Unterordner an.

```powershell
cargo test --workspace --locked --no-fail-fast -- --test-threads=1
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo fmt --all -- --check
python -B -m unittest discover -s tools -p 'test_*.py'
```
