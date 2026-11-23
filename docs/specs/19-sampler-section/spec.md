# Feature Specification: Sampler Section

**Feature Branch**: `19-sampler-section`  
**Created**: December 15, 2025  
**Status**: In Development  
**Priority**: 🟡 Medium Priority

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Sample Triggering (Priority: P1)

As a DJ adding sounds to mixes, I want to trigger samples from 8 pads so I can add air horns, vocals, effects, and other sounds to my performance.

**Why this priority**: Core sampler functionality - standard DJ feature for sound effects.

**Independent Test**: Can be fully tested by loading sample, triggering pad, and verifying sample plays.

**Acceptance Scenarios**:

1. **Given** sample is loaded to pad, **When** pad clicked, **Then** sample plays from beginning
2. **Given** sample is playing, **When** pad clicked again, **Then** sample restarts from beginning
3. **Given** multiple pads triggered, **When** simultaneously pressed, **Then** samples play polyphonically
4. **Given** pad is empty, **When** clicked, **Then** nothing plays and UI indicates no sample loaded

---

### User Story 2 - Sample Library Management (Priority: P1)

As a DJ with many samples, I want to browse and load samples from a library so I can quickly find the right sound for my set.

**Why this priority**: Essential workflow - DJs need organized sample access.

**Independent Test**: Can be tested by opening library, selecting sample, and loading to pad.

**Acceptance Scenarios**:

1. **Given** pad is empty, **When** right-clicked or load button pressed, **Then** sample library modal opens
2. **Given** library is open, **When** sample clicked, **Then** sample is loaded to selected pad
3. **Given** samples are displayed, **When** browsed, **Then** sample name, duration, and category are shown
4. **Given** library has many samples, **When** searched, **Then** samples are filtered by name

---

### User Story 3 - Volume and Loop Control (Priority: P1)

As a DJ mixing samples, I want independent volume and loop controls per pad so I can balance levels and sustain sounds.

**Why this priority**: Essential mixing control - prevents samples from being too loud or needing repeated triggering.

**Independent Test**: Can be tested by adjusting pad volume and enabling loop, verifying changes are applied.

**Acceptance Scenarios**:

1. **Given** pad has sample, **When** volume slider adjusted, **Then** sample playback volume changes accordingly
2. **Given** loop is enabled, **When** pad triggered, **Then** sample loops continuously until stopped
3. **Given** loop is disabled, **When** pad triggered, **Then** sample plays once and stops
4. **Given** volume is at 0%, **When** pad triggered, **Then** sample is muted but still triggered

---

### User Story 4 - Multi-Pad Performance (Priority: P2)

As a performing DJ, I want to trigger multiple pads simultaneously and stop all pads at once so I can create layered sounds and reset quickly.

**Why this priority**: Performance enhancement - enables complex sampler performances.

**Independent Test**: Can be tested by triggering multiple pads and using "Stop All" button.

**Acceptance Scenarios**:

1. **Given** multiple pads are playing, **When** "Stop All" clicked, **Then** all samples stop immediately
2. **Given** pads 1-4 are set to loop, **When** all triggered, **Then** all samples loop in sync
3. **Given** samples are layered, **When** playing together, **Then** no audio clipping or distortion occurs
4. **Given** "Loop All" button clicked, **When** activated, **Then** all loaded samples are set to loop mode

---

### User Story 5 - Pad Color Coding and Organization (Priority: P2)

As a DJ with organized samples, I want visual color coding per pad so I can quickly identify sample types (drums, vocals, effects).

**Why this priority**: Visual organization - speeds up live performance sample selection.

**Independent Test**: Can be tested by loading samples and verifying each has distinct color.

**Acceptance Scenarios**:

1. **Given** sample is loaded, **When** displayed, **Then** pad shows sample's assigned color
2. **Given** sample is playing, **When** active, **Then** pad glows/pulses with colored shadow
3. **Given** samples are categorized, **When** loaded, **Then** colors match categories (e.g., drums=red, vocals=blue)
4. **Given** pad is empty, **When** viewed, **Then** neutral color with dashed border indicates availability

---

### User Story 6 - Sample Information Display (Priority: P3)

As a DJ preparing pads, I want to see sample name and duration on each pad so I know what's loaded without triggering.

**Why this priority**: Convenience - helps avoid wrong sample triggers during performance.

**Independent Test**: Can be tested by loading sample and verifying name and duration are displayed.

**Acceptance Scenarios**:

1. **Given** sample is loaded, **When** pad displays info, **Then** truncated sample name is shown
2. **Given** sample duration is available, **When** displayed, **Then** duration in seconds is shown (e.g., "2.5s")
3. **Given** pad is hovered, **When** mouse over, **Then** tooltip shows full sample name and additional info
4. **Given** sample is one-shot vs looped, **When** displayed, **Then** loop icon indicates mode

---

### Edge Cases

- What happens when sample file is missing or corrupted?
- How does system handle extremely long samples (30+ seconds)?
- What happens when triggering pad at maximum polyphony limit?
- How are samples handled when switching between decks?
- What happens when sample is triggered before previous instance finishes (non-loop mode)?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide 8 sample pads
- **FR-002**: System MUST support one-shot and loop playback modes per pad
- **FR-003**: System MUST allow independent volume control per pad (0-100%)
- **FR-004**: System MUST trigger samples with <10ms latency
- **FR-005**: System MUST support polyphonic sample playback (all 8 pads simultaneously)
- **FR-006**: System MUST provide sample library browser modal
- **FR-007**: System MUST display sample name and duration on each pad
- **FR-008**: System MUST support "Stop All" button to silence all playing samples
- **FR-009**: System MUST support "Loop All" button to enable loop mode on all pads
- **FR-010**: System MUST color-code pads based on sample properties
- **FR-011**: System MUST show visual feedback when pad is triggered (animation/glow)
- **FR-012**: System MUST persist loaded samples across sessions
- **FR-013**: System MUST support sample categorization (drums, vocals, FX, percussion, etc.)
- **FR-014**: System MUST allow clearing/unloading samples from pads
- **FR-015**: System MUST prevent audio clipping when multiple samples play
- **FR-016**: System MUST provide keyboard shortcuts for triggering pads (1-8 keys)
- **FR-017**: System MUST support custom sample upload

### Key Entities

- **Sample**: ID, name, file path, duration (seconds), file size, category, color, waveform data
- **Sample Pad**: Pad number (1-8), loaded sample, volume (0-100%), loop enabled, is playing, trigger count
- **Sample Library**: Sample collection, categories, search index, favorites, recently used
- **Pad State**: Sample reference, volume level, loop mode, playing state, trigger timestamp

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Sample triggering latency under 10ms
- **SC-002**: Sample library loads and displays within 500ms
- **SC-003**: Volume changes apply within 50ms
- **SC-004**: "Stop All" silences all samples within 10ms
- **SC-005**: System supports 8 simultaneous samples without audio dropouts
- **SC-006**: Sample loading to pad completes within 1 second
- **SC-007**: Visual pad feedback (animation) responds within 16ms (60 FPS)
- **SC-008**: Sample library supports 500+ samples without search lag

## Dependencies

- Web Audio API for sample playback
- Audio buffer management system
- File upload/import system
- Sample storage (IndexedDB or cloud storage)

## Assumptions

- Sample files are in supported formats (WAV, MP3, OGG)
- Sample library has pre-loaded common DJ sounds (air horn, etc.)
- Users have basic understanding of sampler functionality
- Samples are mono or stereo audio files
- Sample quality is adequate for performance use

## Technical Constraints

- Web Audio API decoding limitations for certain formats
- Memory usage scales with number and size of loaded samples
- Browser storage limits affect sample library size
- Sample triggering precision limited by JavaScript timing
- Polyphony limited by system audio capabilities

## Out of Scope

- Sample editing (pitch, trim, fade, reverse) - basic playback only
- MIDI velocity-sensitive triggering
- Sample synthesis or generation
- Advanced sample effects (beyond volume control)
- Sample slicing or chopping within sampler
- Recording custom samples within app
- Sample BPM sync or time-stretching
- Complex pad modes (slicer, roll, etc.) - covered by Feature 16 (Performance Pads)

## Geplante Erweiterungen für die Sampler-Bank

Die folgenden Anforderungen erweitern die oben beschriebene Basiswiedergabe um Vorschau, Ebenen und tonales Spielen. Die Originaldateien dienen als Quellen. Für den MC4000 gilt die dokumentierte Bank mit vier gemeinsam genutzten Slots, die von beiden Deckseiten erreichbar ist. Das Ziel von acht Padplätzen in FR-001 und die vorhandene Hardwarebelegung sind gesondert zu prüfen. [Umsetzungsplan](../MIXED_IN_KEY_UMSETZUNGSPLAN.md) · [GUI- und Controller-Inventar](../../GUI_COMPONENTS.md)

### M01

**Samples vor der Pad-Zuweisung hören.** FR-006/007 beschreiben den Browser, FR-012 die gespeicherte Belegung. Die Vorschau benötigt einen eigenen Abhörweg und ein begrenztes Speicherbudget. [Multiple](https://mixedinkey.com/multiple/)

**REQ-M01** Beim Durchsuchen lokaler Samples muss der DJ eine kurze Hörprobe über den getrennten Kopfhörerausgang starten und wechseln können. Erst die bestätigte Zuweisung ersetzt den Slot. Laden und Verwerfen der Vorschauen müssen außerhalb des Audio-Callbacks erfolgen. Der Vorschaucache braucht ein in Byte prüfbares Limit. Ohne getrennten Ausgang muss die App dies melden und die Vorschau stumm lassen.

**AC-M01** Während beide Decks spielen, zwanzig unterscheidbare Hörproben zügig wechseln und die Auswahl anschließend schließen. Die belegten Pads behalten ihre Zuordnung, verspätete Ladeergebnisse starten keine alte Vorschau, der Master enthält keinen Vorschauton und die erfasste Cache-Belegung bleibt im festgelegten Limit. Beim erneuten Öffnen genau ein Sample zuweisen. Nur der bestätigte Slot ändert sich. Ein Gerät mit nur einem Ausgangspaar dient als Gegenprobe.

### M02

**Mehrere Samples einem Pad zuweisen.** FR-003–005 behandeln Pegel, Triggerlatenz und unabhängige Stimmen mehrerer Pads. FR-012/015 behandeln Speicherung und Schutz vor Übersteuerung. Hier erhält ein einzelnes Pad eine benannte, wieder aufrufbare Kombination. [Multiple](https://mixedinkey.com/multiple/)

**REQ-M02** Ein Pad muss mindestens zwei vorbereitete Ebenen gemeinsam auslösen können. Für jede Ebene gehören Quelldatei, Einsatzversatz, Pegel und Panorama zur gespeicherten Belegung. Alte Einzelbelegungen gelten als eine Ebene. Fehlende Quellen sind beim Laden zu kennzeichnen. Die Aktivierung einer unvollständigen neuen Belegung muss die vorherige funktionierende Bank erhalten. Anzahl und Speicherbedarf aktiver Stimmen sind vor der Freigabe zu begrenzen.

**AC-M02** Eine mittig positionierte Kick und eine links positionierte Clap mit 20 ms Versatz, bei 48 kHz also 960 Frames, speichern. Nach Neustart löst ein Pad beide genau einmal mit konfiguriertem Abstand, Pegel und Panorama aus. Vor erneuter Aktivierung eine Quelldatei entfernen. Der Fehler benennt die betroffene Ebene und die bisherige Bank bleibt spielbar. Ein älteres Einzelpad lädt weiterhin mit seinen bisherigen Einstellungen.

### M07

**Tonale Pads in der gewählten Decktonart.** FR-002/004 betreffen Auslösung und Wiedergabe pro Pad. [Spec 6](../6-key-detection/spec.md) FR-005/012 umfasst Konfidenz und manuelle Tonartwahl. Der zusätzliche Spielmodus transponiert ein gestimmtes One-Shot bei der Wiedergabe. Er setzt einen bekannten oder vom DJ bestätigten Grundton voraus. [Captain Play](https://mixedinkey.com/captain-plugins/how-to-guide/captain-play/)

**REQ-M07** Für ein benanntes Referenzdeck muss Dr. DJ Pro eine Belegung aus Sample-Grundton, Zieltonart und Skalenstufen berechnen, die Töne anzeigen und über Kopfhörer vorhören lassen. Erst nach Bestätigung wird die Bank aktiv. Beim Deckwechsel oder bei unsicherer Tonart bleibt die bestätigte Belegung spielbar, bis der DJ die neue Auswahl bestätigt. Ein Wechsel zurück zum normalen Padmodus stellt dessen Belegung wieder her.

**AC-M07** Bei Sample-Grundton A3 und Zieltonart a-Moll lösen die ersten vier MC4000-Pads aufsteigend A3, H3, C4 und D4 aus (MIDI 57, 59, 60 und 62). Nach Wechsel der Referenz auf C-Dur zeigt die Vorschau die neuen Töne. Vor der Bestätigung spielen die Pads weiterhin die bisherigen. Ein ungestimmtes Geräusch ohne bestätigten Grundton führt zu einem Hinweis. Die ursprüngliche Sampledatei bleibt bei allen Schritten bytegleich.
