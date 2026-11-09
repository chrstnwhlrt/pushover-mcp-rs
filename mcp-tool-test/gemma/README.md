# Gemma-Test der Pushover-Werkzeuge

Misst, ob Gemma 4 E4B über den Sprachassistenten Otomo die Werkzeuge von pushover-mcp-rs richtig aufruft: das richtige Werkzeug, die richtigen Argumente, eine passende Antwort. Gemma bekommt dabei alles, was sie in Otomo hat: dessen Systemprompt, den Torwächter, die eingebauten Werkzeuge (Erinnerungen, Agenten, Merkliste) und die Pushover-Werkzeuge über Otomos MCP-Client. Es antwortet der echte Server, aber gegen eine Fake-API. Es geht also nie ein Push auf ein echtes Gerät.

## Bestandteile

- `run.py` fragt den Katalog über `otomo-probe chat --json` ab und wertet aus. Die Konfiguration von Otomo wird für den Lauf kopiert: nur eine Identität, nur Pushover als Server, ohne Rückfrage vor Aufrufen und mit dem Proxy vor Ollama. Die Kopie liegt nur während des Laufs vor, lesbar nur für den Besitzer und wird nie ausgegeben.
- `fakeapi.py` steht für die Pushover-API. Die Welt: zwei Geräte (pixel9 und thinkpad), alle eingebauten Töne plus ein eigener (kuechengong, "Küchengong"), ein Kontingent von 10000 mit 9500 frei, eine eigene Quittung je Notfall-Alarm. Ein unbekanntes Gerät oder ein unbekannter Ton gehen ohne Fehler durch, wie bei der echten API (am 08.11.2026 geprüft).
- `textproxy.py` sitzt als MCP-Server zwischen Otomo und pushover-mcp-rs. Er setzt die Texte einer Variante ein (`--texts`) und hält jeden Aufruf mit Ergebnis fest. Unter `emulate` bildet er Serveränderungen nach, bevor der Server sie hat: Quittung des letzten Alarms, fester Absender, Prüfung von Gerät und Ton, die Grenze `--max-priority` (`max_priority` mit `downgrade` für Herunterstufen samt note oder `refused` für Ablehnen).
- `proxy.py` vor Ollama hält jede Modellanfrage mit Gemmas Denken fest. `why.py TAG REGEX` zeigt, warum sie ein Werkzeug nahm.
- `cases.py`: 32 gesprochene Bitten (`CASES`), 13 zurückgehaltene (`HELD`), sechs erst am Ende geschriebene (`FRESH`) und die Bitten für einen Server mit Grenze: neun unter `--max-priority normal` (`CAPPED`), acht unter `high` (`CAPPED_HIGH`), vier unter `low` (`CAPPED_LOW`) und drei unter `silent` (`CAPPED_SILENT`), jede mit ihrer Prüfung. Dazu kommen drei Bitten um einen Wecker (`WECKER`), die nichts senden dürfen, als Gegenprobe für "alarm" in den Werkzeugtexten. Folgefragen laufen im selben Gespräch wie die Bitte davor. Gewertet wird, was bei der Fake-API ankam und ob die Antwort sagt, was wirklich rausging.
- `claude_check.py` stellt dieselben Bitten Claude über `claude -p`, mit `--strict-mcp-config`. So sieht man, ob Texte für Gemma Claude etwas kosten. Ein Werkzeug eines anderen Servers bricht den Lauf ab.
- `captest.py BINARY [--sender NAME]` prüft ohne Modell, was der Server unter jeder Grenze tut: welche Werkzeuge und Felder er zeigt und mit welcher Priorität und note eine Sendung bei der Fake-API ankommt, für jede verlangte Priorität und ohne eine.
- `texts/` enthält die gemessenen Varianten, die Tabellen in EMPFEHLUNG.md nennen sie. `v1.json` und `v2.json` nutzen noch `device_hint`, eine frühe Nachbildung der Geräteprüfung, die textproxy.py nicht mehr kennt.

## Aufruf

Gebraucht werden ein Checkout von Otomo mit Release-Build (`otomo-probe`) und seiner `config.toml`, Ollama mit Gemma 4 E4B und ein gebautes pushover-mcp-rs. Den Checkout nennt `OTOMO_DIR`, ohne die Variable liegt er als `otomo` neben diesem Repository. Gefragt wird als die Identität `OTOMO_IDENTITY` der Konfiguration, ohne die Variable `Christian`.

```
python3 -I run.py TAG [--runs N] [--texts texts/X.json] [--set NAME] [--only REGEX] [--binary PFAD] [--server-arg ARG ...]
python3 -I run.py TAG --judge [--claude]
python3 -I claude_check.py TAG [--texts texts/X.json] [--set NAME] [--binary PFAD] [--server-arg ARG ...]
python3 -I captest.py PFAD --sender Otomo
python3 -I why.py TAG "REGEX"
```

`--set` nimmt `cases`, `held`, `fresh`, `capped`, `capped-high`, `capped-low`, `capped-silent` oder `wecker`. Die Sets mit Grenze brauchen den Server mit derselben Grenze, etwa `--server-arg=--max-priority --server-arg=high`. Für Otomo kommt `--server-arg=--sender --server-arg=Otomo` dazu, Claude misst man ohne `--sender`.

Ein Durchgang des Katalogs dauert mit Gemma rund fünf Minuten. Gemessen wird nie parallel auf Ollama, das stört Gemmas Drafter.

Nach dem Umbau im Server misst man den neuen Stand ohne Nachbildung: `run.py TAG` ohne `--texts`, gegen das neu gebaute Binary.
