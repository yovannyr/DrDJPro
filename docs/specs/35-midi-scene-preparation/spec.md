# Spec 35 · MIDI-Szenen für die Trackvorbereitung

**Stand 24. November 2026.** Der MIDI-Szeneneditor ist im Feature-Branch über **Prepare → Set planning → MIDI scene editor** erreichbar. Testergebnisse, Messbedingungen und offene Gegenproben stehen im [Prüfbericht zu den 35 Funktionen](../MUSIKVORBEREITUNG_ABSCHLUSSPRUEFUNG.md).

Der Editor speichert das musikalische Raster, die Noten und die Spurenrollen. Unter **Sampler groove** wirkt die Zeitverschiebung gemeinsam auf die gewählten Pads. Unter **Audio phrase** kann der DJ eine isolierte Melodie verknüpfen, die erkannten Noten bearbeiten und die freigegebenen Ereignisse als MIDI exportieren. **Accompaniment** bietet für vier Takte zwei Vorschauen an. Die Kopfhörerwiedergabe verwendet eine gemeinsame Abspielposition für Original, Bass und Akkorde. Unter **Scale** lassen sich beide Tonleitern vergleichen und einzelne chromatische Töne ausdrücklich zuordnen. Die Drumspur behält ihre Tonhöhen.

## Ziel und Arbeitsablauf

Ein DJ markiert in Prepare eine kurze einstimmige Phrase, kontrolliert ihre Noten und probiert eine Begleitstimme aus. Eine Szene enthält musikalische Zeitpositionen, Notendauern und die Trennung von tonalen Spuren und Drums. Der DJ kann die Szene vorhören, bearbeiten, erneut öffnen und die freigegebene Fassung als Standard-MIDI-Datei an seine DAW übergeben. Der [Umsetzungsplan](../MIXED_IN_KEY_UMSETZUNGSPLAN.md) priorisiert die vier eigenständig prüfbaren Schritte.

[Spec 18](../18-live-remix-engine/spec.md) FR-007–010 und FR-016 behandelt Muster, zeitgestempelte Slice-Trigger und JSON. [Spec 4](../4-midi-controller/spec.md) FR-003/004 behandelt MIDI-Nachrichten von und zu Geräten. [Spec 12](../12-stem-control/spec.md) FR-001/013 liefert dafür nutzbare isolierte Audioquellen. Eine bearbeitbare Notenfolge mit musikalischer Zeitbasis und MIDI-Dateiexport braucht eigene Anforderungen. Eine lokal verfügbare isolierte Audiodatei ist eine gleichwertige Eingabe, wenn keine vorbereiteten Stems installiert sind.

## Schnittstellen und Bedingungen

- Die Szene muss Taktart, Tempo, definierte Ticks pro Viertelnote und eine Szenenlänge besitzen. Start, Dauer, MIDI-Tonhöhe, Anschlagstärke und Spurenrolle jedes freigegebenen Ereignisses müssen nach Speichern und erneutem Öffnen erhalten bleiben. Eine Pause bleibt als zeitlicher Abstand erhalten, auch am Anfang oder Ende der Szene.
- Die Vorschau verwendet die separate Kopfhörerroute. Fehlt sie, weist die Oberfläche vor dem Abspielen darauf hin. Weder die Quelldatei noch das Beatgrid oder der Transport eines laufenden Decks werden durch das Bearbeiten geändert.
- Analyse und Export laufen außerhalb des Audio-Callbacks. Abbruch muss eine bereits gespeicherte Szene und eine bestehende Exportdatei erhalten. Für Abnahmen werden musikalische Ereignisse nach erneutem Einlesen verglichen. Das Ergebnis trägt keine Aussage zur erreichbaren Genauigkeit auf beliebiger Musik vorweg.

### M03

**Eine melodische Phrase als MIDI-Noten exportieren.** Vorbereitete Stems aus Spec 12 trennen Audiomaterial. Die Transkription von Tonhöhe, Einsatz und Dauer sowie deren Korrektur kommen hinzu. [Wingman, Audio zu MIDI](https://mixedinkey.com/wingman/workflows/convert-audio-to-midi/)

**REQ-M03** Einen markierten einstimmigen Bass- oder Gesangsabschnitt muss der DJ als Notenentwurf öffnen, zum Original hören, Tonhöhe, Einsatz und Dauer korrigieren sowie überzählige oder fehlende Noten bearbeiten können. Unsichere Passagen sind erkennbar. Der MIDI-Export schreibt ausschließlich die freigegebenen Ereignisse dieser Spur entsprechend ihrer musikalischen Position.

**AC-M03** Eine synthetische einstimmige Acht-Noten-Phrase mit getrennten Einsätzen und einer bekannten Pause verwenden. Die Acht-Noten-Folge und ihre Pause nachweisen, eine Tonhöhe ändern, einen Einsatz versetzen und eine Dauer kürzen. Beim Einlesen der Datei stimmen Tonhöhen und Zeitpositionen mit der freigegebenen Liste bis auf höchstens einen Export-Tick überein. Ein stummer Ausschnitt erzeugt keine klingenden Ereignisse. Ein abgebrochener Export lässt die vorherige Datei lesbar und bytegleich.

### M04

**Eine eigene Bass- oder Akkordbrücke vorbereiten.** Spec 18 FR-007/008 bewahrt bereits vom DJ erstellte Muster auf. Hier wird aus einer Melodie eine neue Begleitspur zur Auswahl erzeugt. Die Eingabe kann aus M03 oder einer manuell erfassten Notenfolge stammen. [Wingman, Melodie mit Begleitung](https://mixedinkey.com/wingman/workflows/turn-melody-into-chords-and-bass/)

**REQ-M04** Für einen viertaktigen Vocal-Loop muss der DJ mindestens zwei unterscheidbare Begleitvarianten erhalten, mit dem Original vorhören, die Begleitung stummschalten und die gewählte Fassung als eigene MIDI-Spur speichern können. Fehlt eine brauchbare Tonart- oder Tempobasis, muss die App diese Angaben zur manuellen Bestätigung anbieten. Die exportierte Begleitung enthält keine Kopie des Vocals.

**AC-M04** Einen 4/4-Loop bei festem Tempo mit zwei Varianten prüfen. Beim Umschalten bleibt der Loop auf derselben musikalischen Zeitposition, Stummschalten betrifft allein die Begleitung. Nach Auswahl und MIDI-Reimport enthält die gewählte Spur genau ihre bestätigten Noten. Das Spurende liegt bei 16 Viertelnoten, auch wenn der letzte Ton früher endet. Ohne Bestätigung einer unklaren Zeitbasis entsteht keine fertige Variante.

### M05

**Groove für mehrere Sampler-Spuren dosieren.** Spec 18 FR-006 und FR-009/010 behandeln Triggergenauigkeit, Aufnahme und Wiederholung. Ergänzt wird eine bewusst gewählte zeitliche Variation auf bestimmten Spuren. [Human Plugins](https://mixedinkey.com/human/)

**REQ-M05** Für ausgewählte Sampler-Spuren einer Szene muss der DJ ein gemeinsames Groove-Profil mit Intensität und maximalem Zeitversatz einstellen und vorhören können. Gleich positionierte, gekoppelte Ereignisse bewegen sich gemeinsam. Intensität null reproduziert die ursprünglichen Zeiten. Die gespeicherte Variante ist nach erneutem Öffnen identisch. Ereignisse bleiben innerhalb des Patterns, Noten behalten eine positive Dauer.

**AC-M05** Hi-Hat und Percussion an denselben und verschiedenen Rasterpunkten platzieren. Bei 0 % alle ursprünglichen Zeitpunkte vergleichen. Bei 50 % und einem vorgegebenen Fenster von ±12 ms erhält mindestens ein inneres Ereignis einen Versatz, jedes bleibt im Fenster und gekoppelte Anschläge bleiben zusammen. Nach erneutem Öffnen sind alle Zeiten identisch. Die nicht ausgewählte Spur, die Deck-Beatgrids und der Beginn des nächsten Patterns bleiben unverändert.

### R09

**Eine gespeicherte Szene in eine andere Tonleiter übertragen.** [Spec 6](../6-key-detection/spec.md) FR-002–005 beschreibt Tonart und Konfidenz, [Spec 19](../19-sampler-section/spec.md#m07) die hiermit verwandte tonale Live-Belegung. Für eine ganze gespeicherte Szene müssen Stufenabbildung und Rücknahme festgelegt werden. [Odesi, Wechsel der Skala](https://odesi.mixedinkey.com/detailedguide)

**REQ-R09** Für die markierte Szene sind Quelltonleiter, Zieltonleiter und die vorgeschlagene Tonhöhenabbildung vor Übernahme sichtbar und hörbar. Diatonische Noten werden anhand ihrer Stufe übertragen. Für chromatische oder außerhalb des MIDI-Bereichs liegende Töne muss die Vorschau eine explizite Entscheidung verlangen. Drumnoten, Anschlagstärken, Rhythmus, Dauern und Szenenlänge bleiben erhalten. Rückgängig stellt die bestätigte Ausgangsfassung wieder her.

**AC-R09** In einer viertaktigen C-Dur-Szene mit Schlagzeug die tonale Folge C4–E4–G4 zunächst nach c-Moll (C4–Es4–G4) und unabhängig davon nach D-Dur (D4–Fis4–A4) übertragen. Zeitpunkte und Anschlagstärken aller Ereignisse sowie sämtliche Drumtonhöhen müssen in beiden Fällen exakt gleich bleiben. Ein zusätzliches chromatisches Fis verlangt eine Entscheidung. Nach Verwerfen beziehungsweise Rückgängig stimmen alle Ereignisse wieder mit der Originalszene überein.

## Abnahmefolge

Erst Szenenrepräsentation und MIDI-Rundlauf an bekannten Ereignislisten prüfen, danach Transkription, Begleitvarianten und Variantenbearbeitung gegen die jeweiligen Kriterien testen. Vor einer Freigabe unter Wiedergabelast gleich lange Zwei-Deck-Läufe mit und ohne aktive Vorbereitung vergleichen. Die bestehende 10-ms-Frist pro Audioblock gilt in beiden Läufen. Hörproben zu Transkriptionsfehlern und musikalischer Qualität auf realen Phrasen stehen gesondert aus.
