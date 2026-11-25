# Spec 37 – Set-Arrangement und Offline-Export

**Stand 24. November 2026.** Arrangement, WAV-Export und Videoauszug sind im Feature-Branch unter **Prepare → Set planning → Timeline** angeschlossen. Testergebnisse, Messbedingungen und offene Gegenproben stehen im [Prüfbericht zu den 35 Funktionen](../MUSIKVORBEREITUNG_ABSCHLUSSPRUEFUNG.md). [Umsetzungsplan](../MIXED_IN_KEY_UMSETZUNGSPLAN.md)

Ein Projekt speichert die Bearbeitung als `.drdjset` und verweist auf die Musikdateien. Die Renderzeitbasis beträgt 48 kHz. Der WAV-Export schreibt Stereo-PCM mit 16 Bit. Die Kopfhörervorschau umfasst bis zu 16 Sekunden. Der MP4-Export verwendet denselben Audiomix und ein lokales FFmpeg mit H.264- und AAC-Encoder. Er erstellt 30 Bilder pro Sekunde. Für zehn Minuten Video belegt das temporäre Stereo-Audio rund 110 MiB zuzüglich der Zieldatei. Für das Bild ist jeweils der zuletzt einsetzende aktive Clip maßgeblich.

Ein DJ bereitet einen Radiomix aus drei lokalen Titeln vor. Er setzt zwei Übergänge, hört das Ergebnis ab, speichert das Projekt und exportiert später denselben Mix. Für eine Ankündigung wählt er 30 Sekunden daraus und erstellt ein Video mit dem jeweiligen Cover.

## Vorhandene Anforderungen und Abgrenzung

[Spec 9, FR-006/008](../9-ai-automix/spec.md) plant Reihenfolgen und automatisierte Übergänge. [Spec 18, FR-005/015/016](../18-live-remix-engine/spec.md) behandelt Slice-Arrangements und Remix-Presets. [Spec 14, FR-001/003/008](../14-recording-system/spec.md) erfasst einen laufenden Audiobus. Diese Konzepte liefern Übergangspunkte, Trackbezüge und Audioformate. Eine wiederaufrufbare Produktion über mehrere Titel benötigt zusätzlich eine eigene Zeitachse, bearbeitbare Übergänge und einen Offline-Renderer. [Spec 8, FR-016–020](../8-video-mixing/spec.md) behandelt die Live-Vorschau und schließt Videokodierung aus. Der lokale Dateiexport ist Gegenstand dieser Spec. Ausgangspunkte im Code sind [Automix](../../../src/automix.rs) und [Videoaufnahme](../../../src/video_recording.rs).

### D07

**REQ-D07** Das Projekt speichert die Reihenfolge, die verwendeten Ausschnitte und ihre Quellpositionen, das Projekttempo, Überlappungen sowie zeitlich verankerte Pegel-, EQ- und Crossfader-Verläufe. Übergänge lassen sich in der Vorschau wiederholt abhören und einzeln ändern. Speichern und Öffnen müssen die Bearbeitung rekonstruieren. Der lokale Audioexport rendert aus demselben Projektstand. Projekt und Export lassen die Quelldateien bytegleich. Fehlende Quellen werden mit ihrer Position im Projekt angezeigt. Wiederfinden, Abbrechen und erneutes Exportieren sind ohne Verlust der vorhandenen Bearbeitung möglich.

**AC-D07** Ein Drei-Titel-Projekt mit zwei unterschiedlichen Übergängen und einer Pegel- sowie einer EQ-Kurve speichern, schließen und öffnen. Reihenfolge, Schnittpunkte und Automationswerte müssen übereinstimmen. Den Verlauf mit Testtönen und Zeitmarken gegen die erwarteten Pegel und Einsätze im unkomprimierten Export prüfen. Gleiches Projekt bei gleicher Abtastrate ergibt die gleichen Ereignispositionen bis auf ein Sample. Zeitachse, Monitoring und Export sind vom Transport der Live-Decks entkoppelt. Bei fehlender Quelle, Abbruch oder vollem Ziellaufwerk bleiben Projekt, Quelldateien und ein zuvor vollständiger Export erhalten. Ein erneuter erfolgreicher Export hinterlässt keine unvollständigen Dateien.

### R07

**REQ-R07** Aus einem markierten Intervall des Arrangements erzeugt die App lokal einen Videoclip mit dem zugehörigen Audiomix, Titel-/Coverwechseln und einer wählbaren, beat-synchronen Animation. Die Projektzeit bestimmt alle Ereignisse. Die Vorschau zeigt das Intervall und seine Länge. Bei fehlendem Cover erscheint eine neutrale Ersatzdarstellung. Ohne verlässliche Beatmarken lässt sich der Clip mit statischem Bild exportieren. Ein Abbruch beendet auch den Hintergrundexport.

**AC-R07** Aus einem längeren Mix 30 Sekunden mit bekanntem Beatgrid und einem Titelwechsel genau zwölf Sekunden nach Clipbeginn als 30-fps-Clip exportieren. Den Clip wieder öffnen und dekodieren. Die Dauer, der Titelwechsel, ein markierter Beat am Anfang und ein weiterer nahe dem Ende müssen jeweils innerhalb eines Videoframes mit den zugehörigen Audiozeitmarken übereinstimmen. Einen erneuten Export ohne Cover und einen mit Abbruch simulieren. Die Ersatzdarstellung bleibt lesbar, der Abbruch meldet keinen Erfolg und ein vorhandener fertiger Clip bleibt erhalten.

## Abhängigkeiten und spätere Nachweise

Zuerst das reproduzierbare Arrangement mit lokalem Audioexport (D07) abnehmen, danach den Videoclip (R07). Die Übergabe vorbereiteter Setlisten, Medienzuordnungen und Cue-Positionen an einen zweiten DJ gehört zu [Spec 36](../36-portable-set-packages/spec.md). Der Transfer einer vollständigen Arrangement-Zeitachse samt Automation liegt außerhalb beider Verträge. Temporärdaten pro Projekt begrenzen. Während eines parallelen Exports gilt der [gemeinsame Audio-Lasttest](../MIXED_IN_KEY_UMSETZUNGSPLAN.md#audio-und-daten).
