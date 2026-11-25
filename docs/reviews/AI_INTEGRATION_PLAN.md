# KI-Integration DRDJPro-Rust — Plan (Slice 0, A0, A1)

Stand: 2026-11-11 · Basis: Commit `86d871d` (öffentlicher Klon, nur gelesen).
**Nicht geprüft:** Die Analyse stammt aus dem Quelltext. Es war keine Rust-Toolchain verfügbar, daher wurde nichts kompiliert und kein Test ausgeführt. Alle Aussagen zu "funktioniert/funktioniert nicht" gelten für den Code, nicht für das laufende Programm.

Dieses Dokument folgt AGENTS.md (Regel 7): vor dem Code Scope, Dateien, Akzeptanzkriterien und Testplan; nach dem Code Zusammenfassung, Befehle, Ergebnisse, Restrisiken. Vor Umsetzung gilt Regel 2 (`docs/MIGRATION_PLAN.md` und `docs/GUI_COMPONENTS.md` lesen).

---

## 1. Ausgangslage (im Quelltext verifiziert)

| Bereich | Befund | Beleg |
|---|---|---|
| BPM/Key/Lautheit | Echt: Onset-Hüllkurve + Autokorrelation, Chroma-FFT + Krumhansl-Kessler, EBU R128, Abbruch via `Cancellation`, Tests | `src/audio_analyzer.rs` |
| Stems | Attrappe: `extract_band()` gibt `samples.to_vec()` zurück; `MLBased` → `ModelNotLoaded`; Qualität hartcodiert `0.75` | `src/stems.rs` |
| Neural Sync | Regelbasierte Bewertung, kein Modell; Harmonik-Vorschlag ist Placeholder (Score `0.75`) | `src/neural_sync_engine.rs` |
| Track-Analyse (Prep) | Simuliert: Beatgrid aus angenommenem BPM 128 | `src/track_analysis.rs` |
| Track-Prep | Mock-Ergebnis laut Kommentar | `src/track_prep_engine.rs` |
| Analyse-Scheduler | Nicht eingebunden (keine Referenz außerhalb der Datei); Fortschrittsmeldungen sind vorgetäuscht (Status "Detecting BPM…" ohne Arbeit), danach ein einziger `analyze`-Aufruf | `src/analysis_scheduler.rs` |
| Realer Analysepfad | `controller.rs` → `start_import_work` → `AudioAnalyzer::analyze_cancellable` | `src/controller.rs` |
| ML-Abhängigkeiten | Keine (`ort`, `candle`, `tract` fehlen) | `Cargo.toml` |
| Gap-Analyse | 490 Zeilen; zählt Stubs als "implementiert" | `docs/serato_feature_gap_analysis.md` |
| Fremdcode-Präzedenz | `crates/drdj-keylock` bindet vendored C++ über `cc` ein; Dependencies sind mit `=` gepinnt | `Cargo.toml` |

Schlussfolgerung: Das Fundament für Beat-/Struktur-KI (echter Analyzer, `Cancellation`, Cache-Typen) ist teilweise da. Die KI-Features selbst existieren nur als Hüllen.

---

## 2. Leitprinzipien

1. **KI nie im Audio-Callback.** Inferenz läuft ausschließlich in Worker-Threads; der Audio-Thread liest nur fertige Ergebnisse (Stems, Grid, Cues).
2. **Offline vor Echtzeit.** Analyse beim Import, Ergebnisse persistent cachen (Hash, Modellversion, Parameter als Schlüssel).
3. **Jedes KI-Feature hat einen klassischen Fallback** und lässt sich abschalten.
4. **Keine Statusbehauptung ohne Messung.** Qualität wird gegen annotierte Referenzen gemessen (siehe Slices B/C), nicht geschätzt.
5. **Fehler statt Fake.** Fehlendes Modell liefert einen Fehler, keine plausibel aussehenden Dummy-Daten.
6. **Modelle werden nicht im Repo gebündelt.** Download auf Nutzerwunsch, SHA-256-Prüfung, Offline-Betrieb bleibt möglich.
7. **Lizenz zuerst.** Vor jeder Modellaufnahme ist Lizenz der Gewichte (nicht nur des Codes) und Trainingsdaten-Hinweis zu prüfen und in der Registry zu dokumentieren. Das ist vor kommerzieller Nutzung zu klären; keine Rechtsberatung.

---

## 3. Slice 0 — Ehrlichkeit herstellen (klein, vor allem anderen)

**Ziel:** Kein Modul meldet mehr eine Fähigkeit, die es nicht hat.

**Scope**
- `stems.rs`: `BandSplit` darf keine unveränderten Kopien liefern. Entweder echte FFT-Bandfilterung (rustfft ist bereits Dependency) oder `StemError::NotImplemented`. Hartcodierte Qualität `0.75` entfernen.
- `track_analysis.rs`: Simulation (BPM 128) durch Aufruf des realen `AudioAnalyzer` ersetzen oder Fehler zurückgeben.
- `neural_sync_engine.rs`: Placeholder-Harmonikvorschlag an `harmonie_wheel.rs`/Key-Analyse anbinden oder entfernen; `phrases_detected` darf nur aus echter Erkennung stammen.
- `analysis_scheduler.rs`: entweder an den realen Analysepfad anbinden (mit echtem Fortschritt) oder löschen (AGENTS: "Eliminate dead code").
- `docs/serato_feature_gap_analysis.md`: Status pro Feature auf drei Stufen umstellen: *real mit Test*, *Hülle/Stub*, *fehlt*.

**Dateien:** `src/stems.rs`, `src/track_analysis.rs`, `src/neural_sync_engine.rs`, `src/analysis_scheduler.rs` (oder Löschung + `lib.rs`), `docs/serato_feature_gap_analysis.md`.

**Akzeptanzkriterien**
- Kein produktiver Pfad erzeugt Stems, Grids oder Phrasen aus Konstanten oder Kopien.
- Test: `separate(.., BandSplit)` liefert Stems, die sich untereinander und vom Mix unterscheiden (z. B. Energie der Bass-Stem bei 1-kHz-Sinus nahe 0), oder den definierten Fehler.
- Test: `track_analysis` auf `tests/fixtures/tone-9s.mp3` liefert BPM/Grid aus dem Analyzer, nicht 128 als Konstante.
- Gap-Analyse enthält pro Zeile Testverweis oder Stub-Kennzeichnung.

**Testplan:** Unit-Tests je Modul; `cargo test --workspace --locked` plus die bestehenden Analyzer-Tests unverändert grün.

**Risiko:** UI-Teile, die Stems/Phrasen anzeigen, müssen den neuen Fehlerzustand behandeln (UI-Tests in `tests/ui_layout.rs` prüfen).

---

## 4. Slice A0 — Backend-Entscheidung (Spike, nicht produktiv)

**Frage:** Welche Inferenz-Runtime passt zu DrDJ Pro (Windows zuerst, macOS später)?

| Kandidat | Pro | Contra / Risiko |
|---|---|---|
| `ort` (ONNX Runtime) | Breite Operator-Abdeckung, optionale GPU-Provider (Windows: DirectML), verbreitet | Version ist laut Quellen weiterhin ein Release Candidate (`2.0.0-rc.x`, MSRV 1.88); lädt Binaries beim Build (CI-Cache, Offline-Builds, Supply-Chain); native DLL im Installer |
| `tract` | Reines Rust, keine native Bibliothek | Nicht alle ONNX-Operatoren; Transformer-/STFT-Modelle sind ein Risiko; meist langsamer |
| `candle` | Reines Rust, flexibel | Modelle müssen in Rust nachgebaut werden: hoher Aufwand je Modell |

**Scope:** Neuer Workspace-Crate `crates/drdj-ml` mit Trait `InferenceBackend` und zwei Backends hinter Cargo-Features (`ort`, `tract`). Kein Anschluss an UI oder Audio-Engine.

**Referenzmodell:** Ein Beat-/Downbeat-Modell (z. B. Beat This!), sofern ein ONNX-Export machbar ist. Fallback: kleines synthetisches ONNX (Conv + GRU) aus einem Exportskript in `tools/`. Ob ein vorhandenes ONNX-Modell tatsächlich existiert und lizenzkonform nutzbar ist, ist im Spike zuerst zu klären.

**Messungen (je Backend):**
- Clean Build unter Windows (Dauer, Fehler, Downloads), Binärgröße, zusätzliche DLLs
- Modell-Ladezeit kalt/warm, Spitzen-RAM
- Inferenzzeit je Minute Audio (Echtzeitfaktor) bei 1, 2 und 4 Threads
- Numerische Parität gegen Referenzausgabe (Python/PyTorch): Vorschlag max. Abweichung ≤ 1e-3, im Spike festzulegen

**Akzeptanzkriterien**
- ADR `docs/adr/ML-BACKEND.md` mit Messtabelle und Entscheidung.
- Ablehnungskriterien: Parität nicht erreichbar; Build nicht reproduzierbar offline/CI; Echtzeitfaktor so schlecht, dass Import-Analyse unzumutbar wird.
- Der zweite Kandidat bleibt als austauschbar hinter dem Trait erhalten oder wird dokumentiert verworfen.

**Testplan:** Parität als automatisierter Test mit eingecheckter Referenz-Ausgabe (kleines Fixture, kein Modell im Repo). Benchmarks als `#[ignore]`-Tests oder Tool unter `tools/`, nicht in `cargo test`.

---

## 5. Slice A1 — Inference-Service

**Voraussetzung:** Slice 0 und A0 abgeschlossen.

**Scope**
- `ModelRegistry`: Modell-ID, Version, Pfad, SHA-256, Lizenzfeld, erlaubte Verwendung, Mindest-RAM. Fehlendes oder hash-abweichendes Modell → `ModelNotLoaded`/`ModelCorrupt`.
- `InferenceService`: begrenzte Job-Queue mit Priorität (Muster aus `analysis_scheduler`: Low/Normal/High), Abbruch über das vorhandene `Cancellation`, Fortschritt nur aus echter Arbeit (Chunk-Zähler), Backpressure.
- Wiedergabe-Schutz: Threadanzahl der Runtime begrenzen (Vorschlag: höchstens die Hälfte der Kerne, bei aktiver Wiedergabe 1–2); unter Windows Worker mit niedriger Thread-Priorität (`windows`-Crate ist bereits Dependency).
- Ergebnis-Cache: Schlüssel = Audio-Hash + Modell-ID + Modellversion + Parameter; Ablage im bestehenden Library-Speicher (SQLite) oder dateibasiert, je nach `docs/LIBRARY_STORAGE.md`.
- Noch **keine** Feature-Anbindung (kein Stem-, Beat- oder Struktur-Modell produktiv). Nur ein Test-Modell.

**Dateien (voraussichtlich)**
- `crates/drdj-ml/{Cargo.toml, src/lib.rs, backend.rs, registry.rs, service.rs, job.rs, cache.rs, error.rs}`
- Root-`Cargo.toml`: Workspace-Member und gepinnte Dependency (`=` wie im Repo üblich)
- `tests/ml_service.rs`, Test-Fixture (winziges ONNX, generiert per Skript)
- `docs/adr/ML-BACKEND.md` (aus A0), `docs/MIGRATION_PLAN.md` (Statuszeile)

**Akzeptanzkriterien**
1. Abbruch mitten in einer Inferenz beendet den Job innerhalb eines Chunks und hinterlässt keinen Teil-Cache.
2. Fortschritt ist monoton und endet bei 100 % nur bei Erfolg.
3. Volle Queue blockiert nicht den Aufrufer-Thread der UI (nicht blockierendes Enqueue mit `QueueFull`).
4. Hash-Abweichung des Modells wird erkannt und abgelehnt.
5. Cache-Treffer lädt kein Modell.
6. **Echtzeit-Nachweis:** In einem Soak-Test (Vorschlag: 10 Minuten) läuft die Audio-Engine mit aktiver Inferenzlast ohne Underruns; Allokationen im Callback bleiben über das vorhandene `test_alloc.rs` bei null (soweit dort bereits geprüft). Dies ist ein Headless-Test und ersetzt **keine** native Hörabnahme.
7. Offline-Build und Test ohne Netz sind nach dem ersten Binary-Download möglich (dokumentiert).

**Testplan**
- Unit: Registry (fehlend, korrupt, falsche Version), Cache-Schlüssel, Queue-Reihenfolge nach Priorität, Backpressure, Abbruch.
- Integration: Mini-Modell durch `InferenceService` mit Fortschritt und Abbruch.
- Real-time: Engine-Callback-Messung mit und ohne Inferenzlast (p99-Jitter und Underruns protokollieren).
- Befehle: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --locked`.

**Restrisiken:** Native Runtime im Installer (DLL-Paketierung), CPU-Konkurrenz mit der Wiedergabe auf schwachen Geräten, RC-Status von `ort`, Speicherbedarf großer Modelle.

---

## 6. Folge-Slices (Skizze, jeweils eigene Spezifikation)

| Slice | Inhalt | Messung vor "fertig" |
|---|---|---|
| B | Neuronales Beat-/Downbeat-Tracking, ersetzt Heuristik wo besser; klassischer Analyzer bleibt Fallback | F-Measure auf annotierten Datensätzen (z. B. Ballroom/GTZAN-Beats, Lizenz der Daten prüfen) und Vergleich mit Serato-Grids eigener Tracks |
| C | Strukturerkennung (Intro/Build/Drop/Break/Outro) → `PhraseType`, Auto-Cues | Segmentgrenzen-Trefferquote gegen manuell annotierte eigene Tracks |
| D | Stems offline beim Import (z. B. Demucs-Familie), Anbindung an `StemCache`/`StemMixState` | Bewertung per SDR auf Testset plus Hörprüfung durch den Nutzer; Importzeit je Track |
| E | Embeddings + lokale Ähnlichkeitssuche, später Text-zu-Audio-Suche | A/B-Test gegen tatsächlich gespielte Folgetracks aus der Mix-Historie |
| F | Transition-Copilot (baut auf B, C, E) | Nutzertest; erklärbare Vorschläge |
| G | MCP-Server für die Library (Agenten-Zugriff) | Schnittstellentests |

Bekanntes Risiko für D: Hybrid-Transformer-Stemmodelle enthalten oft STFT-/Komplexzahl-Operationen, die sich schwer nach ONNX exportieren lassen. Im Spike A0 mitprüfen.

---

## 7. Offene Entscheidungen (brauchen deine Antwort)

1. **Modell-Distribution:** Download-on-Demand (empfohlen) oder mitgeliefert, und von welcher Quelle?
2. **GPU-Beschleunigung:** CPU-only in V1 oder optionaler Provider (z. B. DirectML)?
3. **Zielgruppe:** Club-/Event-/Hobby-DJ. Sie bestimmt, ob Slice C (Struktur) oder Lyrics-/Clean-Mode Vorrang bekommt.
4. **Reviewer und Modellwahl:** AGENTS.md nennt widersprüchliche Review-Zeitpunkte (Review erst nach allen 25 Specs vs. pro Slice). Für diesen Plan bitte festlegen. Nach der Routing-Policy fällt Slice A1 unter "DSP/Performance/mehrere Module" und damit auf das Primärmodell.
5. **Lizenzprüfung:** Wer klärt Gewichts- und Trainingsdaten-Lizenzen vor Slice B/D?

---

## 8. Berichtsformat nach jedem Slice (AGENTS Regel 7)

Zusammenfassung der Änderungen · ausgeführte Befehle · echte Ergebnisse (keine Annahmen) · Restrisiken · ausdrücklich offene Punkte (native Hör-/Sichtabnahme, Hardware).
