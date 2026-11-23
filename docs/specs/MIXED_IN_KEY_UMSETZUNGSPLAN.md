# Umsetzungsplan für 35 Ideen aus der Mixed-In-Key-Recherche

**Stand 23. November 2026.** Die Tabelle legt die Reihenfolge bis zum letzten Arbeitspaket fest. Zu jeder Idee gehören eine federführende Spec, eine Anforderung und ein überprüfbarer Abnahmepunkt.

| Rang | Umsetzungsstand | Gemeinsame Abnahme |
| ---: | --- | --- |
| 1 · D04 | 85/170-BPM-Vergleich in Prepare umgesetzt. Signal-, Speicher-, UI- und Lasttests unter Windows bestanden. [Prüfbericht und Messwerte](3-bpm-detection/D04_ABNAHME.md) | Kopfhörer, Master und Bedienung am Gerät |
| 2–35 | Offen | Nach der Umsetzung |

## Einstieg mit 85 oder 170 BPM

Ein DJ markiert einen gebrochenen Beat in Prepare. Derselbe Ausschnitt läuft mit unveränderter Geschwindigkeit über den Kopfhörer. Er wechselt zwischen Klick und Taktbetonung bei 85 und 170 BPM, hört beide Interpretationen und entscheidet, ob er das Raster übernehmen möchte. Der erste Schnitt ist in drei überprüfbare Schritte aufgeteilt:

1. **Raster und Testmaterial.** Aus demselben Anker die beiden gültigen Deutungen ableiten. Eine synthetische 4/4-Datei mit Referenzzeiten sowie annotierte Beispiele mit geradem und gebrochenem Beat vorbereiten. Bei Tempo-, Taktwechsel- und Warpmarkern bleibt der Entwurf auf die Vorschau begrenzt.
2. **Vorhören in Classic und Modern.** Die Varianten im selben Audioabschnitt vergleichen. Ausgangsposition, Klick, Taktakzent, getrennte PFL-Route und weiterhin spielenden Master anhand separater Signale prüfen.
3. **Übernahme und Gegenprobe.** Erst die Bestätigung schreibt das entsperrte, konstante Raster. Neustart, gesperrtes Raster, Titelwechsel, Abbruch und Speicherfehler prüfen. Die numerischen Sollzeiten und Fehlerfälle stehen in [REQ-D04 und AC-D04](3-bpm-detection/spec.md#d04). Anschließend den gleich langen Lastvergleich und die native Kopfhörerprobe protokollieren.

## Abgleich mit den vorhandenen Specs

Der Abgleich umfasst die im Repository vorhandenen Specs 1–32. Die Katalog-IDs D, A, M und R bleiben als Herkunftsschlüssel erhalten. **REQ-ID** bezeichnet die ergänzte fachliche Anforderung, **AC-ID** die zugehörigen Abnahmeproben. Die Spalte „Spec“ führt direkt zu beiden. Die bestehenden FR-/SC-Kennungen behalten ihre ursprüngliche Bedeutung. Die damalige [Anforderungsmatrix zu Specs 1–25](SPEC_REQUIREMENTS_MATRIX.md) und deren Abnahmen bilden einen eigenen Planungsstand.

| Bestand | Entscheidung für den neuen Plan |
| --- | --- |
| 3 BPM, 6 Tonart, 7 Wellenform, 10 Mastering, 12 Stems, 21 Prepare | Vorhandene Analyse-, Anzeige- und Bearbeitungsanforderungen ausbauen. Ein globaler Tonartwert, eine Frequenzfarbe oder ein True-Peak-Wert belegen noch keine zeitlich markierte musikalische Aussage. |
| 9 Automix, 13 Library, 24 Smart Playlists | Bestehende Empfehlungen, Crates und Dublettenerkennung wiederverwenden. Neu zu spezifizieren sind unter anderem ein festes Übergangsziel, geschützte Anker, verschiedene Edits und die Herkunft widersprüchlicher Werte. |
| 16 Pads, 18 Remix, 19 Sampler | Vorhandene Trigger, Muster und Samplebank ausbauen. Gerätemappings und physisch verfügbare Pads werden bei der späteren Abnahme separat geprüft. |
| 4 MIDI, 8 Video, 14 Aufnahme, 17 Beatgrid, 20 Decks, 22 Phrasen/Sync, 25 harmonisches Mixen, 27 Serialisierung | Benachbarte Verträge für Ereignisse, Routing, Zeitbasis, Koordinaten oder Austausch. Der Offline-Videoexport bekommt eine eigene Zuständigkeit außerhalb des Live-Videomixers. |
| 1 Updates, 2 Cloud-Sync, 5 Streaming, 11 DVS, 15 Live-Streaming, 23 Verlauf, 26 Crash-Handler, 28 USB, 29 Remote-Display, 30 Web, 31 iTunes, 32 KI-Anbieter | Keiner dieser Bereiche ist für eine der 35 Ideen federführend. Vorhandene Daten und messbare Leistungsgrenzen dürfen verwendet werden. Ein Cloud-Konto oder Hardwarekauf ist keine Voraussetzung für lokale Vorbereitung. |

### Neue Specs und ihre Zuständigkeit

| Spec | Eigenständiger Nutzerablauf | Schnittstelle zum Bestand |
| --- | --- | --- |
| [33 – Harmonie in Zeitabschnitten](33-passage-harmony/spec.md) | Akkordwechsel im Überlappungsfenster vergleichen, Modulationen lokalisieren, geprüfte Akkorde als MIDI mitnehmen. | Ergänzt die globale Tonart und die Track-Kompatibilität der Specs 6 und 25 um Zeitabschnitte. |
| [34 – Tonmonitor und Sample-Stimmung](34-source-and-sample-monitor/spec.md) | Eine Audioquelle bewusst wählen, ihren Tonverlauf beobachten oder ein einzelnes Pad stimmen. | Nutzt Routing aus 20 und Sampler aus 19. Die Analyse darf den laufenden Signalweg nicht verändern. |
| [35 – MIDI-Szenen](35-midi-scene-preparation/spec.md) | Eigene oder transkribierte Noten als musikalisch verankerte Szene prüfen, variieren und exportieren. | Ergänzt die Trigger aus 16/18/19 und den Dateiaustausch um Noten, Pausen, Spurenrollen und Taktbezug. |
| [36 – Transportable Setpakete](36-portable-set-packages/spec.md) | Ein offline spielbares Set transportieren oder die Vorbereitung mit gezielter Medienauswahl an einen zweiten DJ übergeben. | Ergänzt Playlist-Export und versionierte Metadaten aus 13/27 um die Prüfung am anderen Rechner. |
| [37 – Set-Arrangement und Export](37-set-arrangement/spec.md) | Übergänge auf einer Projektzeitachse wiederholt bearbeiten, lokal als Audio rendern oder einen Videoausschnitt erzeugen. | Automix aus 9 und Aufnahme aus 14 dienen als Bezug. Der Echtzeitumfang von 8 bleibt getrennt. |

## Reihenfolge

P1 deckt zuerst die tägliche Arbeit am Deck und in der Library sowie die zu qualifizierende Harmonieanalyse ab. P2 verbindet diese Grundlagen mit Setplanung, Performance und Übergabe. P3 bündelt spezialisierte Produktions- und Exportabläufe. Innerhalb jeder Stufe stehen frühe, abgrenzbare Nutzerschritte vor aufwendigen Erweiterungen. Die Aufwandsklassen aus der Recherche sind relative Einschätzungen, keine Zeitversprechen.

„Vorstufe“ benennt einen Arbeitsschritt dieses Plans, dessen Ergebnis wiederverwendet wird. Ein Strich bedeutet, dass die Idee auf dem vorhandenen Projekt und den in ihrer Spec genannten Schnittstellen aufsetzen kann. Sind Nachweise für eine solche Schnittstelle offen, werden sie vor dem betreffenden Arbeitsschritt geprüft. Die Links in der letzten Spalte bezeichnen die Inspiration beim Hersteller. Anforderungen und Abnahmeschwellen für Dr. DJ Pro stehen im jeweiligen Spec-Abschnitt.

### P1 – Bibliothek, Analyse und Entscheidungen am Deck

| Rang | Idee und Arbeitsergebnis | Spec · REQ/AC | Vorstufe | Aufwand | Herkunft |
| ---: | --- | --- | --- | --- | --- |
| 1 | **Halb- und Doppeltempo vergleichen**. Auf demselben Ausschnitt 85/170 BPM mit Downbeat hören, verwerfen oder übernehmen. | [3 · D04](3-bpm-detection/spec.md#d04) | – | mittel | [Mixed In Key Pro 11 für Windows](https://mixedinkey.com/release-notes/mixed-in-key-pro/) |
| 2 | **Tonart und Energie auf dem Cover**. Optionales Overlay für kleine Cover in Classic und Modern. Unbekannte Werte bleiben erkennbar. | [13 · D03](13-library-management/spec.md#d03) | – | klein | [Mixed In Key 12 Standard und Pro](https://mixedinkey.com/pro/) |
| 3 | **Dubletten und verschiedene Edits vor dem Import unterscheiden**. Dateigleiche Kopien sicher erkennen und alternative Edits getrennt zur Entscheidung anbieten. | [13 · R04](13-library-management/spec.md#r04) | – | mittel | [Mixed In Key 11 · Gig-Vorbereitung](https://mixedinkey.com/workflows/prepare-new-music-before-a-gig/) |
| 4 | **Diagnose bei widersprüchlichen BPM- und Tonart-Tags**. Unterschiedliche BPM-/Tonartwerte auf ihre Quelle zurückführen. Schreibschutz und Oktavbeziehung erklären. | [13 · R05](13-library-management/spec.md#r05) | [D04](3-bpm-detection/spec.md#d04) | mittel | [Mixed In Key 11 · Metadaten-Fehlersuche](https://mixedinkey.com/workflows/fix-missing-key-or-bpm-tags/) |
| 5 | **Musikwünsche und Ausschlussliste pro Auftritt**. Wünsche, Pflicht- und Ausschlusstitel an den einzelnen Auftritt binden. | [24 · R02](24-smart-playlist-manager/spec.md#r02) | – | klein | [Beyond Beatmatching · Clubpraxis](https://mixedinkey.com/book/learn-how-to-dj-in-nightclubs/) |
| 6 | **Tonart mit Referenzpiano prüfen und korrigieren**. Eine erkannte Tonart mit Referenztönen vorhören, bestätigen und zurücknehmen. | [6 · A07](6-key-detection/spec.md#a07) | – | mittel | [Mixed In Key 12 · Produktbeschreibung](https://mixedinkey.com/learn-more/) |
| 7 | **Samples vorhören, bevor ein Pad sie lädt**. Samples im Browser über Kopfhörer prüfen, mit begrenztem Cache und ohne das belegte Pad neu zu laden. | [19 · M01](19-sampler-section/spec.md#m01) | – | mittel | [Multiple](https://mixedinkey.com/multiple/) |
| 8 | **Clipstellen finden und bei gleicher Lautheit vorhören**. Verdächtige Clipstellen lokalisieren, Pegel abgleichen und positive gegen unauffällige Stellen hören. | [10 · A01](10-audio-mastering/spec.md#a01) | – | mittel | [Platinum Notes 10](https://mixedinkey.com/platinum-notes/faq/) |
| 9 | **Tonartwechsel als Zeitabschnitte markieren**. Ein Trackprofil mit örtlicher Tonart und Unsicherheit qualifizieren. Akkordwechsel als Gegenprobe verwenden. | [33 · A06](33-passage-harmony/spec.md#a06) | – | hoch | [Mixed In Key Studio Edition](https://mixedinkey.com/studio-edition/) |
| 10 | **Akkordwechsel im Mixfenster**. Akkorde zweier Überlappungsfenster ausrichten, hören, berichtigen und gegen Annotationen auswerten. | [33 · D01](33-passage-harmony/spec.md#d01) | – | hoch | [Mixed In Key 12 Pro](https://mixedinkey.com/pro/) |

### P2 – Vorbereitung, Performance und Übergabe

| Rang | Idee und Arbeitsergebnis | Spec · REQ/AC | Vorstufe | Aufwand | Herkunft |
| ---: | --- | --- | --- | --- | --- |
| 11 | **Cue-Feinschnitt mit Referenzpunkt**. Viertelbeat-Raster an einem anderen Cue ausrichten und den geänderten Einstieg vorhören. | [21 · D05](21-track-preparation/spec.md#d05) | – | mittel | [Mixed In Key Pro 11 für Windows](https://mixedinkey.com/release-notes/mixed-in-key-pro/) |
| 12 | **Im laufenden Track zum nächsten Abschnitt blenden**. Eine Zielstelle im selben Deck vormerken und auf der Taktgrenze hörbar sauber überblenden. | [18 · D06](18-live-remix-engine/spec.md#d06) | – | hoch | [Flow 8 Deck](https://mixedinkey.com/flow-dj-software/) |
| 13 | **Kandidaten nach erlaubter Transposition streuen**. Folgetitel nach zulässiger Transposition streuen, Grenzen und musikalische Begründung anzeigen. | [9 · A08](9-ai-automix/spec.md#a08) | – | mittel | [Mixed In Key Pro 11](https://mixedinkey.com/release-notes/mixed-in-key-pro/) |
| 14 | **Brückentitel zwischen zwei Musikstilen**. Ausgang und Ziel festlegen. Einen Brückentitel anhand beider Übergänge vorschlagen. | [9 · R03](9-ai-automix/spec.md#r03) | – | mittel | [Mixed In Key 11 · Open-Format-Guide](https://mixedinkey.com/workflows/prepare-library-for-open-format-djing/) |
| 15 | **Eine ganze Crate mit festen Ankertracks ordnen**. Alternativen für eine vollständige Crate mit fixierten Ankern vor der Übernahme vergleichen. | [24 · R06](24-smart-playlist-manager/spec.md#r06) | – | mittel | [DJ.Studio · separates Arrangement-Produkt](https://mixedinkey.com/dj-studio/) |
| 16 | **Wortspiel-Übergänge mit eigenen Stichwörtern**. Eigene Hook-Stichwörter über Titel hinweg mit der zugehörigen Cue-Position auffinden. | [13 · R01](13-library-management/spec.md#r01) | – | klein | [DJs Checklist · Übung](https://mixedinkey.com/djs-checklist/) |
| 17 | **Perkussive und tonale Anteile getrennt zeichnen**. Percussion und tonale Anteile synchron zur regulären Wellenform betrachten. | [7 · A05](7-advanced-waveform/spec.md#a05) | – | mittel | [Mashup 2](https://mashup.mixedinkey.com/how-to-make-mashups/create-epic-mashups) |
| 18 | **Schlagzeugpegel und Gesamtlautheit vergleichen**. Drum-Pegel und Gesamtlautheit mit dem entsprechenden Abschnitt und einem Hörvergleich verbinden. | [10 · A02](10-audio-mastering/spec.md#a02) | [A01](10-audio-mastering/spec.md#a01) | mittel | [Platinum Notes 10](https://mixedinkey.com/platinum-notes/faq/) |
| 19 | **Tonartmonitor für eine ausgewählte Audioquelle**. Tonart, BPM und Noten nur für die bewusst gewählte Audioquelle beobachten. | [34 · A03](34-source-and-sample-monitor/spec.md#a03) | – | hoch | [Mixed In Key Live](https://mixedinkey.com/live/) |
| 20 | **Grundton und Feinstimmung für Sampler-Pads**. Grundton eines tonalen Samples bestimmen und die Pad-Stimmung im Kopfhörer prüfen. | [34 · A04](34-source-and-sample-monitor/spec.md#a04) | [A03](34-source-and-sample-monitor/spec.md#a03) | mittel | [Mixed In Key Studio Edition](https://mixedinkey.com/studio-edition/) |
| 21 | **Mehrere Samples als ein Pad speichern**. Mehrere Samples mit eigenen Parametern und gemeinsamem Trigger als ein Pad speichern. | [19 · M02](19-sampler-section/spec.md#m02) | [M01](19-sampler-section/spec.md#m01) | mittel | [Multiple](https://mixedinkey.com/multiple/) |
| 22 | **Tonale Performance-Pads in der Decktonart**. Pads auf Töne der gewählten Deckskala abbilden und bei unsicherer Tonart eine manuelle Wahl anbieten. | [19 · M07](19-sampler-section/spec.md#m07) | – | mittel | [Captain Play](https://mixedinkey.com/captain-plugins/how-to-guide/captain-play/) |
| 23 | **Pad-Phrase aufnehmen und vor dem Speichern verwerfen**. Gespielte Pad-Ereignisse aufnehmen, vorhören und ausdrücklich übernehmen oder verwerfen. | [18 · M09](18-live-remix-engine/spec.md#m09) | – | mittel | [Captain Beat · Handbuch der damaligen Produktgeneration](https://mixedinkey.com/captain-plugins/how-to-guide/captain-beat/) |
| 24 | **Eintaktiges Drum-Fill mit Rückkehr ins Grundmuster**. Ein Fill nur im gewählten Takt spielen und danach ins unveränderte Grundmuster wechseln. | [18 · M08](18-live-remix-engine/spec.md#m08) | – | mittel | [Captain Beat · Fill-Anleitung für Captain Plugins 5](https://mixedinkey.com/captain-plugins/how-to-guide/captain-beat/) |
| 25 | **Gleichzeitigen Gesang im Übergang sichtbar machen**. Vokale Überlappung im gewählten Zeitfenster bei verschiedenen Versätzen hörbar prüfen. | [12 · R08](12-stem-control/spec.md#r08) | – | hoch | [Mixed In Key 11 · Hörmethode](https://mixedinkey.com/workflows/build-a-harmonic-dj-set/) |
| 26 | **Transportables Setpaket mit Manifest**. Ein offline spielbares Setpaket erstellen, übertragen und vor dem Import vollständig prüfen. | [36 · D08](36-portable-set-packages/spec.md#d08) | – | mittel | [Beyond Beatmatching, Buchkapitel](https://mixedinkey.com/book/build-the-perfect-dj-laptop/) |
| 27 | **Ein vorbereitetes Set an den zweiten DJ übergeben**. Eine Setliste mit oder ohne ausgewählte Medien an einen zweiten DJ übergeben, Konflikte auflösen. | [36 · M06](36-portable-set-packages/spec.md#m06) | [D08](36-portable-set-packages/spec.md#d08) | mittel | [Satellite Sessions](https://mixedinkey.com/satellite/) |
| 28 | **Editierbare Set-Timeline mit Automation**. Ein Drei-Track-Projekt mit Automation sichern, erneut abhören und offline rendern. | [37 · D07](37-set-arrangement/spec.md#d07) | – | hoch | [DJ.Studio](https://mixedinkey.com/dj-studio/) |

### P3 – Musikproduktion und Export

| Rang | Idee und Arbeitsergebnis | Spec · REQ/AC | Vorstufe | Aufwand | Herkunft |
| ---: | --- | --- | --- | --- | --- |
| 29 | **Kurze Hook-Folgen auf den Pads üben**. Kurze Hook-Folgen auf den verfügbaren Pads üben und gezielt wiederholen. | [16 · D09](16-performance-pads/spec.md#d09) | – | mittel | [Mixed In Key, Power-Block-Tutorial](https://mixedinkey.com/harmonic-mixing-guide/power-block-dj-mixing-tutorial/) |
| 30 | **Akkordfolge als MIDI-Datei**. Die bestätigte Akkordfolge eines Ausschnitts als zeitlich korrektes MIDI exportieren. | [33 · D02](33-passage-harmony/spec.md#d02) | [D01](33-passage-harmony/spec.md#d01) | mittel | [Mixed In Key 12 Pro](https://mixedinkey.com/pro/) |
| 31 | **Gemeinsames Groove-Timing für Sampler-Spuren**. Die Timing-Variation in mehreren Sampler-Spuren reproduzierbar dosieren. | [35 · M05](35-midi-scene-preparation/spec.md#m05) | [M09](18-live-remix-engine/spec.md#m09) | mittel | [Human Plugins](https://mixedinkey.com/human/) |
| 32 | **Melodische Phrase als bearbeitbare MIDI-Noten exportieren**. Eine isolierte melodische Phrase transkribieren, korrigieren und als MIDI prüfen. | [35 · M03](35-midi-scene-preparation/spec.md#m03) | – | hoch | [Wingman](https://mixedinkey.com/wingman/workflows/convert-audio-to-midi/) |
| 33 | **Eigene Bass- oder Akkordbrücke für einen Übergang**. Eine neue Bass-/Akkordbrücke passend zur Referenz als eigene Phrase erzeugen und editieren. | [35 · M04](35-midi-scene-preparation/spec.md#m04) | [M03](35-midi-scene-preparation/spec.md#m03) | hoch | [Wingman](https://mixedinkey.com/wingman/workflows/turn-melody-into-chords-and-bass/) |
| 34 | **Eine vorbereitete MIDI-Szene in eine neue Tonleiter versetzen**. Gespeicherte tonale MIDI-Szenen in eine andere Skala setzen. Drums und Ereigniszeiten beibehalten. | [35 · R09](35-midi-scene-preparation/spec.md#r09) | [M03](35-midi-scene-preparation/spec.md#m03) | mittel | [Odesi · Kompositionswerkzeug](https://odesi.mixedinkey.com/detailedguide) |
| 35 | **Beat-synchroner Videoauszug für einen Mix**. Einen markierten Mixabschnitt lokal als Video mit synchronen Titel-, Cover- und Beatereignissen exportieren. | [37 · R07](37-set-arrangement/spec.md#r07) | [D07](37-set-arrangement/spec.md#d07) | hoch | [DJ.Studio · separates Arrangement-Produkt](https://mixedinkey.com/dj-studio/) |

## Audio und Daten

- **Audiofrist.** Bei Änderungen mit möglicher Auswirkung auf laufende Decks zwei gleich lange Läufe unter identischen Bedingungen vergleichen: gleicher Build, Rechner, Ausgangskonfiguration, zwei Decks bei 48 kHz und 480 Frames pro 10-ms-Block. Mindestens 30 Sekunden bzw. 3.000 Blöcke je Lauf nach gleicher Aufwärmphase. Einmal mit deaktivierter, einmal mit aktiver Funktion, etwa PFL-Umschalten, Analyse oder Rendern. Für beide Läufe Anzahl verspäteter Blöcke, p99, Maximum, Dropouts und Allokationen im Renderpfad berichten. Ziel: null Fristverletzungen, Dropouts und Render-Allokationen. Eine Fristverletzung erfordert Ursachenprüfung und erneute Messung. [Bestehender Messaufbau](32-ai-providers/PERFORMANCE.md)
- **Signal und Gehör.** Bei Vorhöroptionen die Master- und Kopfhörersignale unabhängig auswerten. Der Vergleich muss auf denselben Audioframes und mit definierten Pegeln erfolgen. Die Prüfung am realen Ausgang und am MC4000 wird als eigene Hör-/Bedienprobe dokumentiert.
- **Musikalische Nachweise.** Für Erkennung und Ranking synthetische Sollwerte und einen getrennten, manuell annotierten Prüfsatz einsetzen. Je nach Funktion Tempo-Deutung, Zeitgrenzen, unbestimmte Passagen, Fehlalarme und Genre ausweisen. Die Referenzen und das Erfolgskriterium für die zu bewertende reale Sammlung vor der Messung festlegen. Neue Ergebnisse zu Herstellerprodukten oder Trefferquoten werden aus den Webseiten nicht abgeleitet.
- **Platz und Datenbestand.** Bestehende Musikdateien, Cues, Grids und Benutzerkorrekturen schützen. Vorschauen und Zwischendaten bei Bedarf und mit einem dokumentierten Limit erzeugen. Für Paket- und Rendering-Slices Dateigröße, Spitzenbedarf, Verhalten bei vollem Laufwerk und Aufräumen nach Abbruch prüfen. Bei neuen persistenten Daten sind Altbestand, Neustart und erneuter Versuch Teil der Abnahme.
- **Freigabe pro Idee.** Für jede Idee die im verlinkten AC genannten positiven und negativen Fälle, das unabhängige Review und die für den jeweiligen Pfad erforderlichen Messungen dokumentieren. Offene Hör-, Geräte- und musikalische Qualitätsprüfungen stehen in der Spalte für die gemeinsame Abnahme.
