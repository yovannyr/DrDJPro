# KI-Anbieter einrichten

Öffne in Classic oder Modern über das Zahnrad die **Einstellungen** und wähle **KI**. Bei einer neuen Installation sind alle drei KI-Funktionen ausgeschaltet. Du kannst deine Decks und die Library sofort wie gewohnt benutzen.

## Einen Anbieter verbinden

| Du möchtest … | So gehst du vor |
| --- | --- |
| Ein Modell auf deinem Computer verwenden | Starte den Modellserver, öffne **Lokale KIs erkennen** und wähle einen Treffer. Prüfe Modell und Adresse und speichere den Anbieter. Bei einem anderen Port trägst du die Adresse selbst ein. |
| Einen eigenen API-Schlüssel verwenden | Wähle die Vorlage deines Anbieters, trage den Schlüssel und das Modell ein. Schalte **Nur lokale KIs** aus, wenn du einen Cloud-Anbieter testen möchtest. Mit **Speichern und testen** schickst du eine kleine technische Anfrage. Dabei können Kosten beim Anbieter entstehen. |
| Dein ChatGPT-Konto verwenden | Wähle die OpenAI-Vorlage und **ChatGPT-Abo**, speichere den Anbieter und starte **Continue with ChatGPT**. Gib die Nutzung deines Tarifs im Systembrowser frei und kehre zur App zurück. Lade die verfügbaren Modelle und wähle eines für die gewünschte Funktion. Ob dein Konto und Workspace den Zugriff erlauben, entscheidet OpenAI. [Nutzung und Freigaben verwalten](https://chatgpt.com/settings/usage) |

Im Anbieterformular kannst du Modelle vom Server abrufen oder eine Modellkennung eingeben. Die Fähigkeitsfelder beziehen sich auf das ausgewählte Modell. Aktiviere Werkzeugaufrufe für den Assistenten und Embeddings für Vektoren. Ein Sprachmodell und ein Embedding-Modell können unterschiedliche Anbieter oder Modellkennungen benötigen.

## Mit einem Track arbeiten

1. Markiere einen Track in der Library.
2. Öffne **Einstellungen → KI** und wähle unter den Funktionen **Assistent**, **Embeddings** oder **Erklärungen**.
3. Wähle den Anbieter und das Modell. Bei einem Cloud-Anbieter zeigt die App die Datenklasse an und fragt nach deiner Freigabe für diese Zuordnung.
4. Klicke auf **Zuordnen** und anschließend auf **Für gewählten Titel ausführen**. Du erhältst einen Vorschlag zum ausgewählten Track oder die Dimension des erzeugten Vektors. Streaming-fähige Modelle können Text schon während der Antwort anzeigen.

Die Funktion verwendet Titel und Interpret der Auswahl. Vorschläge werden als Text angezeigt. Du entscheidest selbst, welchen Track du lädst oder wann du einen Übergang spielst. Der lokale Library-Assistent, die Audioanalyse und die Wiedergabe funktionieren eigenständig.

## Datenschutz, Kosten und Schlüssel

- **Nur lokale KIs** beschränkt Anfragen auf den eigenen Computer. Ein Server im WLAN zählt als extern. Die automatische Suche prüft ausschließlich drei lokale Standardports.
- **Freigabe widerrufen** beendet die Cloud-Freigabe für die Zuordnung. Mit **Zuordnung lösen** deaktivierst du die Funktion.
- Der Sitzungszähler zeigt Anfragen und die vom Anbieter gemeldeten Tokens. Das optionale Tokenlimit dient als Hinweis. Die verbindlichen Tarifgrenzen und Kosten siehst du beim Anbieter.
- Ein eingegebener Schlüssel lässt sich ersetzen oder löschen. Beim Duplizieren eines Anbieters gibst du dessen Zugang separat ein. Entfernst du den Anbieter, löscht die App auch seine Zugangsdaten und Zuordnungen.
- Windows verwendet die Anmeldeinformationsverwaltung, macOS den Schlüsselbund und Linux den Secret Service für den Tresorschlüssel. Ist der Dienst gesperrt oder fehlt er, zeigt die KI-Seite den Sitzungsbetrieb an. Für dauerhafte Speicherung entsperrst du den Dienst, startest Dr. DJ Pro erneut und speicherst den Schlüssel.
- **Konto trennen** beendet die lokale ChatGPT-Sitzung und versucht den Widerruf bei OpenAI. Meldet die App einen ausstehenden Widerruf, kannst du die Freigabe in deinem Konto entfernen. **Alle KI-Zugangsdaten entfernen** löscht die lokalen Schlüssel und Kontodaten für sämtliche Anbieter. [ChatGPT-Nutzung verwalten](https://chatgpt.com/settings/usage)

## Wenn eine Anfrage fehlschlägt

| Anzeige | Was hilft |
| --- | --- |
| Lokaler Anbieter nicht erreichbar | Prüfe, ob der Modellserver läuft, ob der Port stimmt und ob das Modell geladen ist. |
| Authentifizierung fehlgeschlagen | Ersetze den Schlüssel oder melde das Konto erneut an. |
| Modell nicht gefunden | Rufe die Modelle des Anbieters ab und speichere die passende Kennung. |
| Keine Cloud-Freigabe | Prüfe den globalen Schalter und die Freigabe der einzelnen Funktion. |
| Anbieter meldet ein Limit | Prüfe deine Nutzung beim ausgewählten Anbieter. Ein anderer Anbieter wird nur durch deine eigene Auswahl verwendet. |
