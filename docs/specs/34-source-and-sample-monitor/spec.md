# Spec 34 – Tonartmonitor und Sample-Stimmung

**Stand:** Planung vom 23. November 2026. Die Abnahme des Monitors und der Pad-Stimmung ist offen. Der [Umsetzungsplan](../MIXED_IN_KEY_UMSETZUNGSPLAN.md) führt beide als getrennte Arbeitspakete.

## Ziel und Bestand

Beim Vorhören will der DJ erkennen, was auf Deck A, Deck B oder dem Master gerade klingt. Beim Belegen eines Pads benötigt er für eine tonale Kick oder einen Bass-One-Shot dagegen eine konkrete Note mit Cent-Abweichung. Ein Akkord kann eine Tonart nahelegen, ein einzelner Ton kann eine Frequenz belegen. Die Anzeigen müssen diese Aussagen unterscheiden können.

[Spec 6](../6-key-detection/spec.md), FR-001 und FR-005 betreffen die Dateianalyse. [Spec 20](../20-multi-deck-control/spec.md), FR-009 bis FR-011 und FR-013 beschreiben PFL, Pegel und Kanalzuordnung. Im [Renderbus](../../../src/render_bus.rs) bestehen bereits Pegelmessungen. [Spec 19](../19-sampler-section/spec.md), FR-007, FR-012 und FR-017 betreffen Sampleanzeige, Speicherung und Import. Die aktuelle [Pad-Bank](../../../src/sampler.rs) hält Pfade und Gain. Eine laufende Analyse je interner Quelle sowie die bestätigte Stimmung eines Samples sind eigene Anforderungen.

### A03

**REQ-A03** In der kompakten Monitoransicht wählt der DJ genau eine interne Audioquelle. Deck A oder B liefert sein laufendes Signal vor Kanal- und Crossfader, der Master die hörbare Summe. Die Anzeige nennt die Quelle, geschätzte BPM, Tonart und, bei eindeutigen monophonen Passagen, die gespielte Note mit Zuverlässigkeit. Stille und uneindeutiges Audiomaterial haben einen erkennbaren Zustand. Ein Quellenwechsel verwirft veraltete Ergebnisse. Der Monitor verändert weder Routing noch Aufzeichnung. Systemweites Loopback oder Mikrofonzugriff bedürfen einer eigenen plattformspezifischen Qualifikation.

**AC-A03** Geplante Abnahme:

1. A spielt eine eindeutige C-Dur-Folge mit 120 BPM, B eine Fis-Moll-Folge mit 90 BPM. Nach Umschalten auf B und Ablauf eines dokumentierten Analysefensters erscheinen Fis-Moll und 90 BPM (±2 BPM), keine nachgelieferten A-Ergebnisse. Wenn ausschließlich B im Master hörbar ist, stimmt dessen Anzeige mit B überein. Eine separate Passage mit stabilem 440-Hz-Ton ergibt A4. Für diesen einzelnen Ton darf keine gesicherte Dur-/Moll-Aussage erscheinen.
2. Den Kanal von A am Mixer schließen und A weiterspielen lassen. Die Deck-A-Analyse bleibt gültig, im reinen Master-Stillesignal erscheint nach spätestens einem vollständigen Analysefenster „unbekannt“. Eine bereits belegte PFL-Auswahl bleibt beim Quellenwechsel erhalten. Ein Lauf mit Monitor und ein gleich langer Kontrolllauf bei zwei Decks erfüllen beide die unveränderte 10-ms-Frist ohne zusätzliche Fristverletzungen, ungültige Blöcke oder Allokationen auf dem Renderthread.

### A04

**REQ-A04** Beim Belegen eines Sampler-Pads werden aus einem tonalen Sample Grundton, zugehöriger Notenname und Abweichung in Cent zu A4 = 440 Hz vorgeschlagen. Der DJ kann einen Referenzton über PFL anhören und die Stimmung des Pads bestätigen, korrigieren oder zurücksetzen. Bestätigte Werte sind an Pad **und** Sample gebunden. Die Pad-Wiedergabe übernimmt eine bestätigte Korrektur ohne Änderung der Quelldatei. Bei unzuverlässigem Befund ist eine manuelle Einstellung weiterhin möglich. Alte Bänke erhalten die Default-Stimmung von null Cent.

**AC-A04** Geplante Abnahme:

1. Einen 440-Hz-Sinus und eine um +25 Cent verstimmte Variante auf unterschiedliche Pads laden. Der Vorschlag liegt jeweils innerhalb von ±5 Cent des Sollwerts. Eine bestätigte Gegenkorrektur von −25 Cent am zweiten Pad ergibt bei der Ausgabe A4 innerhalb derselben Toleranz. Nach dem Neustart spielt nur dieses Pad mit der Korrektur.
2. Ein Rauschimpuls erhält keinen gesicherten Grundton. Nach Austausch des Samples auf dem zuvor korrigierten Pad wird die alte Korrektur verworfen. Die Quelldateien haben vor und nach Speichern und Zurücksetzen identische Prüfsummen. Bei fehlendem getrenntem Kopfhörerausgang bleibt der Referenzton stumm.

## Abhängigkeiten und spätere Qualifikation

Die Auswertung erhält begrenzte Datenmengen abseits des Audiocallbacks und arbeitet mit der bereits konfigurierten Ausgabe-Samplerate. Bei Rückstau darf ein Monitorfenster verfallen, ein Audioausgabeblock muss weiterlaufen. Die Dateianalyse eines Samples darf das Triggern bereits geladener Pads nicht verzögern. Die Umsetzung prüft Pad-Bank-Migration, Quellenwechsel, Abbruch und Rückstau getrennt. Die [Audiolastmessung](../32-ai-providers/PERFORMANCE.md) und eine reproduzierbare Messung der ausgegebenen Stimmung ergänzen die spätere Hörprobe. Die Analyse läuft lokal.
