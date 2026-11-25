# Dr. DJ Pro – UI-Prüfbericht Windows, 21. November 2026

## Ergebnis

Der Windows-Release-Build von `main` startete mit angeschlossenem Denon DJ MC4000. Der native Ordnerimport, die Tabellenaktion zum Laden, die Cover-Miniatur, das Setzen eines Hot Cues und der Einstieg in die Einstellungen funktionierten mit einer frischen Library. Beim Ersttest in Classic blieb Deck A leer, als eine MP3 aus **Files** auf den runden Teller gezogen wurde. **Load A** lud denselben Titel anschließend erfolgreich.

**D1 ist erledigt.** Bei der [manuellen Gegenprobe vom 22. November](#manuelle-gegenprobe-vom-22-november-2026) luden beide Decks in Classic den abgelegten Titel und blieben pausiert.

**Die [MC4000-Hörprobe in Modern](#mc4000-hörprobe-in-modern) ist erledigt.** Der Anwender bestätigte die Wirkung aller fünf Mixer-Regler und die PFL-Zuordnung auf beiden Decks.

Während des Ersttests sperrte sich die Windows-Sitzung. Bis zur Sperre wurde das echte Fenster bedient. Die weiteren Ergebnisse dieses Laufs stammen aus automatisierten UI-, Integrations- und Gerätetests. Eine vollständige native Freigabe für einen Live-Einsatz steht aus.

## Aufbau und Nachweise

- Windows 11 Enterprise, Build 26300, Windows x64. Standard-Release aus dem [CI-Lauf des geprüften Hauptzweigs](https://github.com/yovannyr/DrDJPro/actions/runs/44909306866). Die ausführbare Datei bestätigte mit `--crash-build-info` selbst Commit `17744cc99db8278c8000c814caf1ce2d947b4fb3`, Ziel und Release-Profil.
- Eigenes temporäres Verzeichnis und absolute `DRDJ_LIBRARY_PATH` für den Test. Zwei eigens erzeugte, leise 65-Sekunden-Töne als MP3 mit eingebettetem PNG und als FLAC sowie ein kurzes Ogg/Vorbis-Beispiel. Die produktive Library wurde für diesen Lauf nicht geöffnet. Der reguläre Crash-Logger nutzt auch bei separater Library den Windows-Benutzerdatenordner.
- Normales Fenster mit ungefähr 1.280 Pixeln Breite, Classic mit MC4000. Keine Modellinstallation, kein Build des gesamten Projekts, kein Schreiben in die private Musiksammlung. Die erste Startkonfiguration mit Messbericht und Library im selben Ordner wurde mit verständlicher Meldung abgelehnt. Mit getrennten Zielordnern startete die App.
- Sichtbare Fensterzustände, Windows-Geräteauflistung, nachträgliche Nur-Lese-Abfrage der Test-SQLite-Datei und getrennte Testprotokolle dienten als Nachweise. Die zusätzlichen lokalen Testprogramme stammen aus vorhandenen Builds des funktionalen Settings-Stands. Sie decken sich in den geprüften Bedienpfaden weitgehend mit dem Hauptzweig, enthalten aber nicht durchgehend dessen Branding, Verpackung und neuesten Buildstand.

## Im echten Fenster und am angeschlossenen Gerät

| Prüfung | Beobachtung | Bewertung |
| --- | --- | --- |
| Start und Erkennung | Hauptfenster öffnet sich. Beide Decks zeigen MC4000. Die Statuszeile meldet „MC4000 input + LEDs connected“ und aktivierte Transport-/Mixersteuerung. Windows führt MIDI- und Audio-Endpunkte als betriebsbereit. | Verbindung belegt. Tatsächliches Leuchten und Betätigen einzelner Pads offen. |
| Leere Library und Ladeschutz | Vor dem Import 0/0 Titel und beide PLAY-Schaltflächen deaktiviert. | Bestanden für leere Decks. |
| Files → Add folder | Der Windows-Ordnerdialog öffnet sich. Nach Auswahl des Testordners erscheint „Added 3 tracks“, 0 übersprungen, 0 fehlend. Titel, Dauer und der Test-Interpret werden angezeigt. | Bestanden. |
| Cover in der Tabelle | Nur der präparierte MP3-Titel zeigt die erwartete Miniatur in der Cover-Spalte. | Bestanden für diese PNG-Datei in Classic. |
| Zeile → Classic-Plattenteller A | Gültige MP3 aus der sichtbaren Tabelle auf die Mitte des virtuellen Plattentellers gezogen. Danach weiterhin „No track loaded“ und PLAY gesperrt. | Fehlversuch am 21. November. Gegenprobe auf A und B am 22. November bestanden. |
| Tabellenaktionen Load A / Load B | Dieselbe MP3 per **Load A** geladen. Deck A zeigt Dateinamen, 01:05 und Pause. Anschließend FLAC ausgewählt und **Load B** gedrückt. In der Test-Library enthält die gespeicherte Decksession die unterschiedlichen Track-IDs 1 und 2. | A visuell und persistent bestätigt, B persistent bestätigt. |
| Hot Cue, PLAY und Einstellungen | Ersten Cue auf A gesetzt, PLAY gedrückt und mit dem Zahnrad in die Deck-Einstellungen gewechselt. Die Datenbank enthält Cue 1 an Position 0 und einen Verlaufseintrag für Titel 1. | Aktionen und Übergang bestätigt. Ein lückenloses Audiosignal während des Wechsels wurde nicht aufgezeichnet. |
| Hardware-Endpunkte | Zwei ausdrücklich gestartete Hardwaretests öffneten den MIDI-Eingang und prüften den WASAPI-Ausgang des angeschlossenen MC4000 samt übereinstimmender Geräte-ID in dieser Sitzung und vier Kanälen. Beide bestanden. | Identität und Öffnen bestätigt. Die [Hörprobe vom 22. November](#mc4000-hörprobe-in-modern) bestätigt zusätzlich die Deckzuordnung im Kopfhörer. |
| Dateidecoder des geprüften Release | Dieselbe EXE dekodierte per `--analyze` acht Fixtures erfolgreich: MP3, AAC/M4A, ALAC/M4A, AIFF, FLAC, Vorbis sowie Opus in Mono und Stereo. Sie lieferte jeweils Frames, Dauer und Signalpegel. | Decoderprüfung. Die Einzeldateien wurden nicht alle auf beide sichtbaren Decks gezogen. |

## Weitere Bedienwege und Testtiefe

| Bereich | Automatisiert geprüft | Noch nativ offen |
| --- | --- | --- |
| Classic, Modern, zwei Decks und vier Slots | Navigation, Wechsel bei erhaltenem Deckzustand, Mindestbreite, erreichbare Transport- und Wellenformsteuerung, Schutz der Anzeigeplätze C/D. | Lesbarkeit beider Ansichten bei weiteren DPI-Stufen und schmalem Fenster. |
| Einstellungen und Tastatur | Alle fünf Reiter, Escape, aktive Reiterbeschreibung, Hot-Cue-Sperre und gesperrte Dateiablage im Setup. | Vollständiger Rundgang per Tastatur im nativen Fenster. |
| Transport und Mixer | Cue/Stutter, Hot Cues, gespeicherte und automatische Loops, Beatgrid, Quantize, Pitch/Keylock, Sync, Scratch, Nudge, EQ, Filter, Crossfader, FX und Sampler. | Reale Haptik, Scratch-Klang, Hörprüfung der übrigen Mixerfunktionen und separate Kontrolle des physischen Masterausgangs. |
| Library | Suche, Sortierung, Spalten, Crates, Verlauf, Prep Queue, Persistenz, Fehlerpfade sowie Datei- und Ordnerimport. | Langer Bedienlauf auf einer großen Sammlung, echte Explorer-Ablage und Interaktion mit dem iTunes-Dateidialog. |
| Audio und Controller | Profilwiederherstellung und Gerätewechsel mit injizierten Geräten, reale Endpoint-Prüfungen wie oben. MIDI-Kommandos, LED-Zustände, HID-Mapping und USB-Finder mit Testdaten. | Gerätewechsel bei geladenen Decks in diesem Build, Hotplug, physische Bedienelemente, optische LED-Rückmeldung. |
| Weitere Fenster und optionale Funktionen | Remote-Display/-Eingabe, iTunes, Browserregeln, Berichte und Freigabestatus, optionale Inferenz und DVS mit Testsignalen. | Sichtbare Fenster, WebView2-Darstellung, echter zweiter Monitor und Timecode-Hardware. Der Standard-Release enthält die optionalen ML- und DVS-Buildmerkmale nicht. |

### Ergebnisse der lokalen Testprogramme

| Lauf | Ergebnis |
| --- | --- |
| Vollständige App-Testsammlung | 583 bestanden, 0 fehlgeschlagen, 12 regulär ignoriert. 191,53 Sekunden im abschließenden Lauf. Davon 150 Controller-, 55 Playback-, 77 Library-Store- und 28 MC4000-Fälle. |
| Layout und moderne Wellenformen | 6 + 8 bestanden. |
| Ergänzende Fenster- und Routingtests | 28 bestanden. |
| Langtest Sync | Beide Deckrichtungen renderten offline je 119,988 Sekunden. Maximaler Phasenfehler nach der Einlaufzeit 0,000000 bzw. 0,000053 Beats bei einer Grenze von 0,10. |
| Zusätzliche Modul- und Integrationstests | 149 innerhalb dieser Auswahl bestanden, einschließlich 33 UI-Fällen, 115 Modul-/Integrationsfällen und eines optionalen App-Importfalls. Diese Auswahl überschneidet sich mit den vorherigen Läufen. |
| Reale Geräteprüfungen | Die beiden oben genannten opt-in Tests bestanden separat. |

Die Laufgruppen wurden anhand der Testfälle abgeglichen. 626 verschiedene Fälle im ersten Block und 149 in der Zusatzsuite überschneiden sich in 33 Fällen. Zusammen mit zwei separat aktivierten Hardwaretests ergibt das 744 unterschiedliche bestandene Fälle. Die acht CLI-Decodierungen und die zusätzlichen Drag-Gesten sind getrennt gezählt.

Im ersten App-Testlauf fehlte eine relative FLAC-Testdatei im isolierten Arbeitsverzeichnis. Mit der mitgelieferten Datei bestanden der Einzeltest und die vollständige Wiederholung. Beim optionalen ML-Import war im ersten Lauf zusätzlich ein Beat-Modell aktiviert. Die dokumentierte Konfiguration für diesen Test bestand beim Wiederholen. Beide Erstfehler sind als Testaufbau nachvollziehbar.

## Gezielte Gegenprobe zum Classic-Plattenteller

Nach dem nativen Fehlversuch wurde ein zusätzlicher Test mit echtem Ladecontroller, synthetischen Slint-Zeigerereignissen und einem Mixer im Speicher ausgeführt. Geprüft wurden beide Library-Reiter (Files und Library), beide Decks, Teller und LOAD-Fläche, zwei Fenstergrößen (850 × 860 und 1280 × 860) sowie Trainingsmodus und simulierte MC4000-Präsentation. Jeder Fall startete mit einem leeren Deck. Geprüft wurden der kanonische Pfad der geladenen WAV-Datei und der pausierte Transport.

| Beginn der Ziehgeste | Erfolgreich |
| --- | --- |
| Zuerst horizontal aus der Zeile, dann auf das Deck | 32/32 |
| Direkt diagonal auf das Deck | 32/32 |
| Zuerst vertikal, dann auf das Deck | 32/32 |

Die 96 Fälle reproduzierten den nativen Fehlversuch nicht. Insbesondere unterscheiden sich Eingabelieferung (Slint-Testbackend gegenüber nativer Windows-Eingabe) und Testdatei (WAV gegenüber MP3 mit Cover). Die temporäre Teständerung wurde vollständig zurückgenommen. Als nächster Diagnoseschritt war eine Wiederholung mit der identischen MP3 und echten Windows-Zeigerereignissen vorgesehen.

## Befund D1 – erledigt

### Ursprünglicher Reparaturplan vom 21. November 2026

**Auswirkung:** Ein DJ könnte beim Vorbereiten erwarten, dass der Titel bereits geladen ist. Der getestete Ausweichweg ist die Auswahl der Zeile mit anschließendem **Load A**. Ein reproduzierbarer Fehler im Produktcode oder ein Datenverlust ist durch diesen Einzelversuch nicht belegt.

1. **Native Reproduktion priorisieren.** Mit derselben Datei und angeschlossenem MC4000 je drei kontrollierte Mausgesten aus **Files** und **Library** ausführen. In Classic getrennt Teller und LOAD-Fläche auf A/B ansteuern, in Modern Cover und Teller. Mit eingesetzter Zeilenauswahl und einer Zwischenbewegung oberhalb der Drag-Schwelle wiederholen. Eingangs- und Zielkoordinaten sowie Skalierung festhalten. Beim Erfolg müssen Titel und Dauer erscheinen und das Deck pausiert bleiben.
2. **Ereigniskette eingrenzen.** Bei einem Fehlschlag Drag-Start am Track, angebotene Daten, Hit-Test und Entscheidung des Ladecontrollers getrennt beobachten. Der bestehende Test `library_pointer_drag_loads_both_decks_at_supported_sizes` prüft in Classic die LOAD-Fläche und in Modern auch Cover/Teller. Der native Versuch zielte in Classic auf den Teller.
3. **Kleinste passende Korrektur ableiten.** Falls der Drag erst gar nicht startet, Zeilenauswahl und Drag-Geste gemeinsam untersuchen. Falls der Teller den Drop abfängt, seine Trefferfläche und die Schichtung mit der Deck-DropArea prüfen. Falls ausschließlich die Testgeste zu kurz ist, den nativen Testtreiber korrigieren. Ein Release-Fix braucht eine fehlschlagende Vorher- und erfolgreiche Nachher-Probe.
4. **Regression absichern.** Classic und Modern bei schmalem und normalem Fenster, beide Decks, laufendes Zieldeck und geöffnetes Setup prüfen. Scratching, Laden per Klick und die separate Windows-Explorer-Ablage müssen weiter funktionieren.
5. **Abnahme abschließen.** Danach die verbleibenden nativen Nebenfenster, Audio-Gerätewechsel und Sitzungswiederherstellung prüfen. Für physische Pads, LEDs und Kopfhörer-/Master-Trennung ist eine kurze Hardware- und Hörprobe separat zu protokollieren.

Die Testinstanz mit der separaten Library war am Ende weiterhin geöffnet und antwortete. Die Windows-Sitzung war gesperrt. Es wurden keine Produktänderungen vorgenommen.

### D1 – Nachprüfung vom 21. November 2026

Auf dem Windows-Stand `17744cc` mit angeschlossenem MC4000 ließ sich der Fehler in Classic auf beiden Decks wiederholen. In **Files** begann die Geste auf der Titelzelle von **QA Azul** bei (380, 795) und endete auf Teller A bei (70, 166) bzw. Teller B bei (707, 166). Die Koordinaten beziehen sich auf die Aufnahme des 1282 × 892 Pixel großen Fensters einschließlich Titelleiste. Beide leeren Decks zeigten danach weiterhin „No track loaded“. Auch ein bereits pausiert geladener Titel wurde beim Ziehen aus der Library nicht ersetzt. Die Gegenprobe über **Load A** zeigte `QA-Azul.mp3`, **01:05.000** und **PAUSED**.

Im automatisierten Vergleich verfehlte die Geste mit einer einzigen Bewegung beide Teller. Alle acht Vergleichsgesten mit Zwischenbewegung luden erfolgreich. Windows Classic bestimmt das Ziel jetzt beim Loslassen anhand der tatsächlichen Zeigerposition. Der Test mit der Original-MP3 (SHA-256 `a718c45b8199d0bf2f5a59d007b61f1baab42a44a1c67d09adae919a5119081b`) bestätigte den geladenen Dateipfad und den Pausenzustand auf A und B. Weitere Prüfungen decken Zeilenauswahl, Rechtsklick, Loslassen außerhalb der Decks, geöffnetes Setup, spielendes Ziel, Escape und Fokusverlust ab.

Der Stand [ab01345](https://github.com/yovannyr/DrDJPro/commit/ab0134545489e525f7afcf31d27be76b3ba5f103) bestand 584 reguläre Tests der Desktop-Anwendung, 14 Layout- und Wellenformtests sowie zwei separat ausgeführte MC4000-Gerätetests. Zehn weitere optionale Qualifikationstests blieben ausgelassen. Formatierung und striktes Clippy bestanden. Ein unabhängiges Review mit GPT-6.1 Sol (Mittel) bestätigte die Korrektur nach der Ergänzung des Escape-Abbruchs.

Die automatische Nachprüfung verwendete den echten Ladecontroller mit Slint-Testeingaben. Die manuelle Gegenprobe am Windows-Desktop stand am 21. November wegen der gesperrten Sitzung noch aus.

### Manuelle Gegenprobe vom 22. November 2026

Der Anwender prüfte Drag-and-drop in der Classic-Ansicht unter Windows auf dem lokal gebauten `main`-Stand `6eb86e0` nach dem Merge von [PR #22](https://github.com/yovannyr/DrDJPro/pull/22). Er bestätigte für **Deck A** und **Deck B** jeweils, dass das Deck den abgelegten Titel lud und pausiert blieb. Damit ist die manuelle Abnahme für D1 bestanden.

## MC4000-Hörprobe in Modern

Am 22. November 2026 prüfte der Anwender die Modern-Ansicht unter Windows mit Kopfhörern am Denon DJ MC4000. Die App lief auf dem lokalen `main`-Stand `17c0288` nach dem Merge von [PR #23](https://github.com/yovannyr/DrDJPro/pull/23). In der App war der Master auf OUT 1/2 und das Vorhören auf OUT 3/4 geroutet.

| Prüfung | Deck A | Deck B |
| --- | --- | --- |
| Gain, High, Mid, Low und Filter | Alle fünf Regler wirkten hörbar auf Deck A. | Alle fünf Regler wirkten hörbar auf Deck B. |
| PFL | Mit PFL A war Deck A im Kopfhörer zu hören. | Mit PFL B war Deck B im Kopfhörer zu hören. |
