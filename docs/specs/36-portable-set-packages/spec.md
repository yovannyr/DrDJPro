# Spec 36 – Transportable Setpakete und Übergabe an einen zweiten DJ

**Stand 23. November 2026.** Geplante Funktion, fachliche und technische Abnahme offen. Die beiden Ausbaustufen stehen im [priorisierten Umsetzungsplan](../MIXED_IN_KEY_UMSETZUNGSPLAN.md).

## Ziel und Nutzerablauf

Ein DJ will seine vorbereitete Crate auf einem zweiten Rechner ohne Zugriff auf die ursprünglichen Musikordner spielen. Bei einem gemeinsamen Auftritt soll der zweite DJ dieselbe Reihenfolge und dieselben Cue-Positionen erhalten, aber seine eigenen lokalen Musikdateien zuordnen können.

1. Eine geordnete Setliste auswählen und als **Spielbares Set** (D08) oder **Übergabe an DJ** (M06) vorbereiten.
2. Vor dem Export die Reihenfolge, die mitzunehmenden Dateien und deren Gesamtgröße prüfen. Für fehlende Quellen wird angezeigt, welche Einträge betroffen sind.
3. Das Paket am Ziel öffnen. Die App prüft zuerst Formatversion, Manifest, Verweise und mitgelieferte Dateien. Die Importvorschau zeigt zugeordnete, fehlende, mehrdeutige oder beschädigte Einträge.
4. Nach ausdrücklicher Bestätigung wird die Setliste im Zielkatalog gespeichert. Eine bestehende Zuordnung oder ein gleichnamiger Titel im Ziel wird nur nach ausdrücklicher Konfliktentscheidung verändert.

## Bestand und Abhängigkeiten

- [Spec 13, FR-016](../13-library-management/spec.md) beschreibt M3U/PLS-Export. Eine Pfadliste allein reicht für transportable Cue-Zeiten und den Abgleich der Dateien nicht aus.
- [Spec 21, FR-007 bis FR-009 und FR-014](../21-track-preparation/spec.md) beschreibt gespeicherte Trackmarken. Das Paket übernimmt die vorhandenen Marken mit ihrer Position innerhalb der jeweiligen Audiodatei.
- [Spec 27, FR-001, FR-006 bis FR-009 und FR-013](../27-protobuf-serialization/spec.md) beschreibt Playlist-/Track-/Cue-Daten, Versionierung und Validierung. Die [Playlist-Konvertierung](../../../src/protobuf_playlist.rs) erhält Eintragsidentitäten. Für die Übergabe braucht es zusätzlich rechnerunabhängige Dateizuordnungen, ein Medienmanifest und Regeln für Konflikte.
- Das [Handbuch](../../wiki/Installation-und-Updates.md) beschreibt die Sicherung der Bibliothek und der Musikdateien. Diese Spec plant einen selektiven Export für einen Auftritt. Ein vollständiges Backup des Katalogs und die Synchronisation über einen Online-Dienst gehören zu anderen Abläufen.

## Gemeinsamer Vertrag

Das versionierte Manifest enthält die geordneten **Eintragsidentitäten**, ihre Trackreferenzen, die zugehörigen Cue- und Übergangspositionen mit angegebener Zeiteinheit sowie die Dateireferenzen. Ein Track darf im Set mehrfach vorkommen. Eine beigefügte Datei wird pro Inhalt nur einmal kopiert und mit Dateilänge, Prüfverfahren und Inhaltsprüfsumme aufgeführt. Für nicht beigefügte Musik werden Dateimerkmal und Inhaltsprüfsumme der lesbaren Quelle als Zuordnungshilfe transportiert. Lokale Datenbank-IDs allein identifizieren die Aufnahme auf einem zweiten Rechner nicht zuverlässig.

Alle beigefügten Dateien haben paketrelative Pfade. Die Vorschau zeigt Kollisionen zwischen Namen, etwa bei abweichender Groß-/Kleinschreibung auf Windows und Linux. Beim Import müssen absolute Pfade, Pfade außerhalb des Paketordners und Verweise über Links nach außen abgewiesen werden. Abbruch, zu wenig Speicherplatz und unbekannte inkompatible Versionen hinterlassen die Quelldateien, ein bereits vorhandenes Zielpaket und den Zielkatalog unversehrt. Die Vorbereitung arbeitet außerhalb der Echtzeit-Audioverarbeitung.

### D08

**REQ-D08** Beim Export eines spielbaren Sets müssen alle ausgewählten Titel und ihre Vorbereitungsdaten portabel vorliegen. Fehlende oder während des Exports geänderte Dateien meldet die App mit dem betroffenen Set-Eintrag. Ein unvollständiges Paket erhält keine Fertigmeldung. Beim Öffnen muss der DJ jede beigefügte Datei gegen das Manifest prüfen können.

**AC-D08** Eine Crate mit zwei verschiedenen gleichnamigen Dateien und einem wiederholten Track auf einen anderen Datenträger exportieren. Nach Trennen der Quelle müssen auf dem zweiten Rechner Reihenfolge, Wiederholung und Cue-Zeiten exakt erhalten bleiben und die richtige Aufnahme muss für jeden Eintrag laden. Ein geändertes Byte muss den betroffenen Medieneintrag als beschädigt markieren. Abbruch während des Kopierens, voller Zieldatenträger oder eine unzulässige Pfadangabe dürfen weder ein gültiges Paket ersetzen noch Dateien außerhalb des Pakets anlegen.

### M06

**REQ-M06** Bei einer Übergabe ohne Medienauswahl exportiert die App ausschließlich die Setdaten. Eigene Audioclips lassen sich einzeln beilegen. Die zweite Person kann bereits vorhandene lokale Dateien anhand übereinstimmender Inhalte zuordnen. Abweichende Edits oder mehrdeutige Treffer werden zur Entscheidung angezeigt. Fehlende Titel bleiben an ihrer ursprünglichen Position sichtbar. Ein anderer Ordner oder Laufwerksbuchstabe darf die Übergangsmarken nicht verschieben.

**AC-M06** Ein Drei-Titel-Set mit Übergangsmarken in eine zweite Testbibliothek mit anderer Ordnerstruktur und absichtlich kollidierenden numerischen Track-IDs übertragen. Zwei identische lokale Aufnahmen müssen die ursprünglichen Positionen und Cue-Zeiten erhalten. Der dritte Titel bleibt als fehlend markiert. Ein anderer Edit mit demselben Dateinamen darf nicht automatisch zugeordnet werden. Die Dateiliste des Standardexports enthält kein Audio. Nach ausdrücklicher Auswahl eines eigenen Clips enthält der nächste Export genau dessen Audiodatei. Erneuter Import mit Abbruch darf keine zusätzlichen Set-Einträge erzeugen.

## Geplante Prüfung

Für beide Varianten sind automatisierte Manifest-, Konflikt- und Abbruchtests sowie ein Import in einen unabhängigen Zielkatalog vorgesehen. Für den plattformübergreifenden Nachweis dasselbe Paket unter Windows, macOS und Linux öffnen, die Dateizuordnung prüfen und die vorbereiteten Cue-Positionen vergleichen. Eine Hörprobe an zwei Rechnern mit getrennten Musikordnern schließt den manuellen Teil ab.
