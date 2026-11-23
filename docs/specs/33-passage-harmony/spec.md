# Spec 33 – Harmonieverlauf im Übergang

**Stand:** Planung vom 23. November 2026. Die Umsetzung und die musikalische Abnahme stehen aus. Reihenfolge und Abhängigkeiten stehen im [Umsetzungsplan](../MIXED_IN_KEY_UMSETZUNGSPLAN.md).

## Ziel und Bestand

Ein DJ markiert in Prepare die Ausstiegsphrase von A und den Einstieg von B. Beide Titel können als d-Moll katalogisiert sein und innerhalb dieser acht Takte trotzdem ganz unterschiedliche Akkorde enthalten. Der DJ soll die entscheidende Passage hören, den zeitlichen Verlauf prüfen und eine bestätigte Akkordfolge zur Weiterarbeit mitnehmen können.

[Spec 6](../6-key-detection/spec.md), FR-001, FR-005 und FR-016 betreffen die globale Tonart, die Zuverlässigkeit und die Speicherung. [Spec 25](../25-enhanced-harmonic-mixing/spec.md), FR-025-003, FR-025-006 und FR-025-008 verwenden Tonarten für Kompatibilität, Suche und Deckanzeige. Die [bestehende Dateianalyse](../../../src/audio_analyzer.rs) aggregiert Tonhöhenklassen über die Datei. Eine Akkordspur und eine lokale Tonartspur haben verschiedene Bedeutungen. Sie sind ein neuer, vorab berechneter Gegenstand dieser Spec. Die Live-Quellenanalyse ist in [Spec 34](../34-source-and-sample-monitor/spec.md) beschrieben.

## Gemeinsamer Ablauf und Abhängigkeiten

- Eingabe sind ein lokaler Track und sein Analysebereich in der Zeit der Quelldatei. Gespeicherte Beatgrids aus [Spec 17](../17-beat-grid-sync/spec.md) ordnen diese Zeit Takt und Schlag zu. Das Hörfenster bestimmt der DJ in Prepare. Bei fehlender Zeitbasis bleiben Zeitangaben in Sekunden nutzbar, die Oberfläche weist fehlende Taktpositionen aus.
- Vergleiche verwenden die vom DJ gewählten Eintrittspositionen, Tempi und hörbaren Tonhöhen beider Titel. Die Vorschau ist unabhängig vom Transport der live spielenden Decks. Der Nutzer entscheidet über Annahme oder Korrektur von Befunden.
- Die Hintergrundanalyse muss abbrechbar sein. Kompakte Ereignisse tragen die Identität der Audiodateiversion und der Analyseversion. Nach einem Dateitausch dürfen alte Ergebnisse nicht erscheinen. Eine geänderte Grid-Zuordnung muss vor erneuter Verwendung der Taktpositionen berücksichtigt werden. Bestehende Katalogeinträge ohne Verlauf bleiben lesbar.

### D01

**REQ-D01** Prepare legt für das gewählte Übergangsfenster die Akkordspuren beider Titel nebeneinander. Ein Ereignis umfasst Anfang, Ende, Akkordbezeichnung und Zuverlässigkeit. Die Ansicht zeigt, wann sich Akkorde wechseln. Schwach belegte Intervalle erscheinen als unbekannt. Für eine Passage kann der DJ die Zuordnung prüfen, berichtigen und bestätigen. Über PFL lässt sich der zeitlich ausgerichtete Ausschnitt als Paar hören.

**AC-D01** Geplante Abnahme:

1. Zwei kontrollierte d-Moll-Beispiele über vier Takte erzeugen. In A bleibt der Dreiklang gleich, B spielt Dm–Gm–A–Dm, jeweils einen Takt lang. A zeigt ein durchgehendes Ereignis, B die vier erwarteten Akkorde. Die Grenzabweichung beträgt im Beispiel höchstens einen Viertelschlag. Ein geänderter Einsatzpunkt von B verschiebt dessen Spur und hörbaren Einsatz entsprechend.
2. Eine zusätzliche reine Rauschpassage muss als unbekannt erscheinen. Das Vorhören verändert weder die Masterausgabe noch die Position des live spielenden Decks. Eine bestätigte Korrektur lässt sich nach einem Neustart erneut aufrufen.
3. Für echte Aufnahmen wird auf einem vorher getrennten, annotierten Hörkorpus die korrekt bezeichnete Dauer, die Dauer unbekannter Abschnitte und der Fehler der Wechselpositionen angegeben. Ergebnisse getrennt nach Genre ausweisen. Synthetische Testfälle allein reichen für die musikalische Abnahme nicht aus.

### A06

**REQ-A06** Eine zweite Spur zeigt für jeden stabil erkannten Tonartabschnitt Beginn, Ende, Tonart und Zuverlässigkeit. Prepare verwendet am Ein- bzw. Ausstieg die jeweils gültige lokale Tonart. Die globale Bibliotheksangabe und manuelle Track-Korrekturen bleiben eigenständige Daten. Ein Akkordwechsel genügt nicht als Nachweis einer Modulation.

**AC-A06** Geplante Abnahme:

1. Eine Referenz mit klar etablierter C-Dur- und anschließender D-Dur-Kadenz samt markierter Wechselposition prüfen. Die beiden Abschnitte sind korrekt benannt, der Wechsel liegt höchstens einen Takt neben der Referenz. Ein Wechsel des gewählten Einstiegspunkts über die Grenze zeigt die jeweils zugehörige Tonart.
2. Eine Kadenz mit mehreren Akkorden, die in C-Dur bleibt, erzeugt keine zweite Tonart. In einer nichttonalen Testpassage ist die lokale Tonart unbekannt. Vorher gespeicherte manuelle Katalogwerte bleiben in beiden Fällen erhalten.

### D02

**REQ-D02** Der DJ kann ein ausgewähltes Intervall der bestätigten Akkordspur aus D01 als Standard-MIDI-Datei exportieren. Das Intervall beginnt im Export bei Tick null. Enthalten sind Notenhöhen, Einsätze, Dauern, Tempo und Taktart. Noten an den Auswahlgrenzen werden beschnitten. Unbestätigte oder unbekannte Intervalle bleiben als Pausen auf der Zeitachse. Ein fehlendes oder im gewählten Abschnitt nicht abbildbares Raster wird vor dem Export angezeigt. Ein Abbruch oder Schreibfehler darf keine fertige Datei vortäuschen.

**AC-D02** Geplante Abnahme:

1. Vier Takte in 4/4 bei 120 BPM mit bestätigten Akkorden und einer Pause exportieren und mit einem unabhängigen MIDI-Leser öffnen. Tonhöhen, Taktart und Tempo entsprechen der Referenz, Einsätze und Dauern stimmen bis auf einen Dateitick. Endzeit ist Takt fünf, Schlag eins. Note-off-Ereignisse enden spätestens an dieser Grenze.
2. Eine Auswahl mitten in einem gehaltenen Akkord und mit einer unbestätigten Passage exportieren. Der erste bestätigte, angeschnittene Akkord beginnt bei null, die unbestätigte Passage bleibt still. Bei einem Schreibfehler bleibt eine bereits vorhandene Zieldatei unverändert. Es gibt keine Erfolgsmeldung.

## Spätere Qualifikation

In der Umsetzung sind Datenvertrag, Tests an annotiertem Material, unabhängiger MIDI-Reimport und die native PFL-Hörprobe separat nachzuweisen. Bei Analyse unter Wiedergabelast sind zwei gleich lange Durchläufe mit zwei Decks, gleichem Material und gleicher 10-ms-Frist zu vergleichen. Als Referenz für Fristverletzungen, Renderzeit, ungültige Blöcke und Allokationen auf dem Audiothread dient das [Messverfahren der Anwendung](../32-ai-providers/PERFORMANCE.md). Die Gesamttonart eines Titels oder die Übereinstimmung zweier Tonartcodes allein ist kein Abnahmekriterium für diesen Abschnitt.
