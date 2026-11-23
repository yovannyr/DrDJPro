# R02 · Wünsche und Ausschlüsse für einen Auftritt

Unter **Library → Prepare → Gig plan** einen Auftritt anlegen oder im Auswahlfeld **Active gig** auswählen. Einen Titel markieren, seinen Status wählen und bei Bedarf den Grund eintragen. **Save** speichert die Regel für diesen Titel und Auftritt. Ein Wechsel des Titels oder Auftritts verwirft ungespeicherte Eingaben.

| Status | Verhalten |
| --- | --- |
| Must play | Der Titel bleibt offen, bis für den aktiven Auftritt ein bestätigter Abspieleintrag entsteht. Laden und Vorhören erfüllen ihn nicht. |
| Request | Hält einen Wunsch samt Begründung für diesen Auftritt fest. |
| Avoid tonight | Bei manueller Auswahl erscheint eine Warnung. Die Vorschläge für ähnliche Titel und Übergänge übergehen den Titel. |

**Clear** entfernt die Regel im ausgewählten Auftritt. **No active gig** hebt die Auswahl des Auftritts auf. Ein Titel kann in zwei Auftritten unterschiedliche Regeln haben.

## Automatisierte Gegenproben

- Der Windows-Test legt für denselben Titel zwei Auftritte an. Nach einem Neustart lädt er ihn trotz Warnung in Gig A manuell. In Gig B probiert er die Kopfhörervorwahl und den temporären Cue aus. Erst Play erfüllt den Pflichttrack. Ein zweites Play und eine Änderung der Begründung erhalten den ersten Erfüllungszeitpunkt.
- Der Speichertest öffnet einen Katalog der Version 20 mit bestehendem Verlauf. Er prüft die Migration auf Version 21 und erzwingt einen Schreibfehler. Der neue Abspieleintrag und die Erfüllung werden zusammen zurückgerollt. Nach einem erfolgreichen zweiten Versuch wird der gesamte Katalog gespeichert. Ein Neustart prüft die Regeln beider Auftritte.
- Bei der Prüfung über die Kommandozeile liefert die Suche nach ähnlichen Titeln und Übergängen jeweils Titel 3 für Gig A und Titel 2 für Gig B. Der Ausschluss wird vor der Rangbildung berücksichtigt. Die manuelle Suche behält den Titel, die Auswertung historischer Übergänge bleibt unabhängig von der heutigen Auswahl.
- Auf Windows bestanden 620 reguläre Tests der Anwendung, alle 13 Bedienoberflächentests und die sechs Python-Tests des Library-Assistenten. Die Python-Tests prüfen auch einen älteren Katalog ohne Gig-Tabellen und die Kommunikation über MCP.

## Audiolast unter Windows

Zwei Decks spielten gleichzeitig, mit 48 kHz, vier Kanälen und 480 Frames je Block. Pro Durchlauf wurden nach 100 Aufwärmblöcken 3.000 Blöcke in 30 Sekunden geprüft. Im aktiven Lauf wechselte ein separater Thread zwischen zwei Auftritten, änderte beide Regeln und bestätigte die Wiedergabe. Ein zweites Öffnen der Datenbank prüfte alle Abspieleinträge und den zuletzt gespeicherten Regelzustand.

| Messgröße | Ohne Änderungen | Mit Gig-Änderungen |
| --- | ---: | ---: |
| Vom Speicherthread bestätigte Änderungen | 0 | 279 |
| Renderzeit p99 / Maximum | 6,239 / 7,518 ms | 6,117 / 6,996 ms |
| Startverzögerung p99 / Maximum | 0,668 / 0,844 ms | 0,693 / 0,851 ms |
| Verfehlte 10-ms-Fristen | 0 | 0 |
| Fehlerhafte Signalblöcke / Render-Allokationen | 0 / 0 | 0 / 0 |

Im ersten Vergleich wurden bei 279 Änderungen zwei Fristen verfehlt (ein Renderblock bis 14,813 ms). Der Kontrolllauf hatte keine Überschreitung. Die Gig-Daten werden auf dem Bibliotheksthread gespeichert. Im Renderpfad gibt es keinen Zugriff auf diese Daten. Für die Wiederholung lief derselbe Build mit denselben Einstellungen und derselben Änderungsfrequenz. Währenddessen ruhten weitere lokale Prüfarbeiten. Die erste Überschreitung ließ sich dabei nicht reproduzieren. Ihre genaue Ursache ist offen.

Wiederholbar mit `cargo test --locked --bin drdjpro-ui-spike r02_equal_duration_gig_changes_with_two_playing_decks -- --ignored --nocapture --test-threads=1`.

## Gemeinsame Abnahme

Zwei eigene Auftritte anlegen und einen Titel einmal als Pflichttrack, einmal als Ausschluss kennzeichnen. Den Wechsel auf dem eigenen Bildschirm und das Vorhören am Controller mit anschließendem Play ausprobieren.
