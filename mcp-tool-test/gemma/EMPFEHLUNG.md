# Empfehlung für pushover-mcp-rs (gemessen mit Gemma 4 E4B in Otomo)

Stand 09.11.2026, Messkit in diesem Ordner (README.md). Gemessen wurde das Binary aus dem Arbeitsbaum gegen eine Fake-API. Die vorgeschlagenen Serveränderungen bildet textproxy.py nach, die Texte setzt er aus `texts/v5.json` (Otomo mit `--sender`) bzw. `texts/c5.json` (Claude ohne `--sender`) ein.

## Ergebnis

Gemma 4 E4B in Otomo, die Texte von 1.0.0 (Original) gegen die Empfehlung (nachgebildet):

| Satz | Original | Empfehlung |
|---|---|---|
| Katalog, 32 Bitten, 3 Läufe | 62/96 | 94/96 (v4) |
| Zurückgehaltene Bitten, 13, 3 Läufe | 24/39 | 37/39 (v5) |
| Neue Bitten, erst am Ende geschrieben, 6, 3 Läufe | 8/18 | 16/18 (v5) |
| Claude Sonnet 5.5, Katalog ohne Folgefragen | 26/26 | 26/26 |
| Claude Sonnet 5.5, zurückgehaltene Bitten | 11/11 | 11/11 |

Claude nennt in beiden Fällen sich und seine Sitzung als sender, die neuen Texte kosten Claude also nichts. Die Ablation (v4 = 64/64, je eine Änderung weg, je 2 Läufe): ohne Tonprüfung 62, ohne Geräteprüfung 61, mit den alten Instructions 59, ohne flaches Enum 63. Die Regel "letzter Alarm" machte aus 0/8 Folgefragen 8/8.

Bekannte Grenzen, die kein Servertext löst:
- "Schick meiner Frau eine Push-Nachricht ..." geht in etwa einem von vier Fällen an die eigenen Geräte des Nutzers und Gemma behauptet, die Frau sei informiert. Sonst fragt sie nach einem Gerät für die Frau. Der Satz "cannot reach other people" in der Werkzeugbeschreibung brachte 9/12 gegen 8/12, das ist Rauschen. Das ist ein Thema für Otomos Harness (behauptete Aktionen), der Satz bleibt trotzdem, weil er stimmt.
- Ein deutscher Tonname ohne passenden Anzeigenamen ("Registrierkasse"): Nach der Ablehnung bietet Gemma "cashregister" an, statt erneut zu senden (1 von 3).
- Gerätenamen, die die Art des Geräts nicht verraten (thinkpad): Gemma fragt dann zurück und nennt die Geräte. Das ist richtig so.
- Bitten, die nach "später" oder "wenn" klingen ("Gib mir per Push Bescheid", "Start in zehn Minuten"), nimmt Gemma als Erinnerung oder Beobachtung. Das passiert mit jedem Text und ist meistens auch gemeint.

## Serveränderungen

Jede hat in der Ablation (v4 minus genau diese Änderung, je zwei Läufe) Fälle gekostet, wo sie fehlte.

1. **Letzter Alarm ohne Quittung.** In `check_emergency_receipt` wird `receipt` optional. Ohne `receipt` (und bei `cancel_emergency_notification` auch ohne `tag`) gelten beide dem letzten Notfall-Alarm, den dieser Serverprozess gesendet hat. Hat er noch keinen gesendet, kommt ein Fehler wie "No emergency notification was sent yet; give a receipt." Grund: Otomos Verlauf behält nur Gesprochenes, die Quittung ist im nächsten Zug weg. Gemma fragt dann den Nutzer nach der "Bestätigungsnummer". Folgefragen gingen damit von 0/8 auf 8/8.
2. **`--sender NAME`.** Der Absender ist für diesen Client fest, `sender` verschwindet aus dem Schema, Tool-Beschreibung und Instructions erwähnen ihn nicht (Texte unten). Ohne die Option bleibt alles wie heute, mit `sender` als Pflicht. Für Otomo `--sender Otomo`. Ohne die Option fragte Gemma "Wer soll die Nachricht schicken?", kopierte "pushover mcp" aus dem Beispiel oder nahm "Küchengong" als Absender. Mit ausgeblendetem Feld kam keiner dieser Fälle mehr vor.
3. **Gerät prüfen.** Ist `device` gesetzt, prüft der Server die Namen gegen `users/validate.json` (Ergebnis je Prozess zwischenspeichern ist in Ordnung). Ein unbekannter Name ergibt den Fehler: `Unknown device 'Laptop'. The user's devices are pixel9, thinkpad: send again with the one the user means (a phone, a laptop: tell it by its name), or omit device to reach all of them.` Ohne die Prüfung geht "nur an den Laptop" mit device "Laptop" still an alle Geräte (0/2). Mit ihr kam 2/2, entweder das richtige Gerät oder eine Rückfrage mit den Gerätenamen.
4. **Ton prüfen.** `sounds.json` einmal je Prozess laden. Ein Anzeigename (ohne Groß- und Kleinschreibung zu beachten) wird auf seinen Namen abgebildet, "Küchengong" also auf "kuechengong". Ein unbekannter Ton ergibt den Fehler: `Unknown sound 'X'. The sounds are pushover (Pushover (default)), bike (Bike), ... Send again with the name that fits what the user asked for, or omit sound for the user's default.` Ohne den Satz "Send again ..." bot Gemma bei "Registrierkasse" den Ton "cashregister" nur an, statt erneut zu senden. Ohne die Prüfung ging "mit dem Küchengong" mit sound "Küchengong" raus und wurde still verworfen (0/2), mit ihr 2/2.
5. **Priorität flach und robust.** Das Enum steht direkt am Feld als `enum` mit den Beschreibungen, ohne `$ref` (im Server `src/schema.rs`). Zusätzlich die Pushover-Zahlen -2 bis 2 annehmen und als Fehlertext für alles andere "priority must be one of silent, low, normal, high, emergency". Mit den Texten von 1.0.0 schickte Gemma `"priority": 1` (drei Mal) und ließ die Priorität nach der Ablehnung weg. Seit die Instructions die Werte nennen, kamen keine Zahlen mehr. Das flache Enum allein brachte in der Ablation einen Fall, ist aber billig und hilft jedem Client.

## Texte

### INSTRUCTIONS

Mit `--sender` (für Gemma gemessen):

> Sends push notifications to the user's own phone and other devices through Pushover; it reaches nobody else. send_notification needs only message; leave out every other parameter the user does not ask for. Without device a notification reaches all of the user's devices, the phone included. Priority high is for something important or urgent; emergency only for an alarm the user wants repeated until acknowledged; low for no sound and no vibration; silent for no alert at all. For vibration alone use the sound vibrate. A sound the user names that is not among the built-in names of the sound parameter, such as a gong or a sound of their own, is one of the account's own: call list_sounds and take its name. check_emergency_receipt and cancel_emergency_notification act on the latest emergency notification unless you give a receipt. Pushover cannot schedule: every notification goes out at once.

Ohne `--sender` derselbe Text mit "send_notification needs only message and sender;" (mit Claude gemessen). Der Halbsatz "it reaches nobody else" kam mit den zurückgehaltenen Fällen dazu: Bei "Schick meiner Frau ..." ging die Nachricht einmal an die eigenen Geräte des Nutzers und Gemma behauptete, sie an seine Frau geschickt zu haben. Die Ablation mit den alten Instructions kostete fünf Fälle: "wichtig" wurde zum Notfall, "nur vibriert" zu priority low und den Küchengong fand Gemma nicht.

### send_notification

- Beschreibung mit `--sender`: `Send a push notification to the user's own devices now; it cannot reach other people. Only message is required. Returns the request ID, and for emergency priority a receipt.`
- Beschreibung ohne: `Send a push notification to the user's own devices now; it cannot reach other people. message and sender are required. Returns the request ID, and for emergency priority a receipt.`
- `sender` (nur ohne `--sender`): `Who sends it, shown first in the title: your own name, with the session or project you work in if there is one, e.g. "Claude (shop backend)". At most 64 characters.`
- `device`: `Only to reach one device and not the others: its name as list_devices gives it (several joined with commas). Device names are the user's own, such as iphone15 or work-mbp, never a word like phone or laptop, nor a person: call list_devices first. Omit to reach all devices; a notification "to my phone" needs none.`
- `sound`: `Sound name. Built in: pushover, bike (bicycle bell), bugle, cashregister, classical, cosmic, falling, gamelan, incoming, intermission, magic, mechanical, pianobar, siren, spacealarm, tugboat, alien, climb, persistent, echo, updown, vibrate (vibration only), none (silent). The account's own sounds, such as a gong, only list_sounds names. Omit for the user's default sound.`
- `priority`: wie heute, nur flach (siehe 5).
- Alle anderen Felder bleiben unverändert.

### check_emergency_receipt

- Beschreibung: `Check whether an emergency notification was acknowledged or has expired: the latest one sent, or the one of receipt.`
- `receipt` (optional): `Receipt of the emergency notification; omit for the latest one sent.`

### cancel_emergency_notification

- Beschreibung: `Stop the repeated alerts of an emergency notification: the latest one sent, the one of receipt, or all sent with tag.`
- `receipt`: `Receipt of the emergency notification to stop; omit receipt and tag for the latest one sent.`
- `tag`: `Stop every emergency notification sent with this tag.`

## Grenze für laute Prioritäten (`--max-priority`)

Der Wunsch dahinter: Kein Modell soll den Nutzer nachts wecken können. Laute Prioritäten brauchen deshalb eine Freigabe beim Start des Servers, wie `--sender` als Argument.

1. **Option `--max-priority LEVEL`** mit silent, low, normal, high oder emergency (die Zahlen -2 bis 2 zählen wie die Namen), Default normal. high gehört mit unter die Grenze, weil high die Ruhezeiten durchbricht und immer klingelt. Wer Alarme zulassen will, startet mit `--max-priority emergency`. Das gilt für jeden Client, auch für Claude-Code-Sitzungen: Mit dem Default gehen keine Alarme raus, bis `--max-priority emergency` in den args steht.
2. **Herunterstufen statt ablehnen.** Liegt die verlangte Priorität über der Grenze, sendet der Server mit der Grenze und lässt retry, expire und tags weg. Ins Ergebnis kommt ein Feld `note`, das auch im Text des Ergebnisses steht (Otomo zeigt Gemma den Text): `Sent as an ordinary notification, with {cap} priority instead of {priority}: alarms are switched off for this client (--max-priority {cap}). Tell the user that it went out as an ordinary notification.` Beim Server sieht das so aus: `{"note":"Sent as an ordinary notification, with normal priority instead of high: ...","request_id":"..."}`
3. **Schema vollständig lassen.** Das Enum behält alle fünf Werte mit ihren Beschreibungen und die Instructions behalten den Satz zu high und emergency. Auch retry und expire bleiben, denn ohne sie las Gemma "Notfall-Alarm, der alle fünf Minuten wiederholt wird" als Auftrag zum Nachsehen und sendete nichts (0/3 unter normal und unter high, mit den Feldern 3/3). Von der Grenze steht im Schema nichts, erst die note sagt es.
4. **Unter emergency fällt weg, was nur der Quittung dient:** die Werkzeuge check_emergency_receipt und cancel_emergency_notification, an send_notification das Feld tags, in den Instructions der Satz "check_emergency_receipt and cancel_emergency_notification act on the latest emergency notification unless you give a receipt." und in der Beschreibung von send_notification der Teil ", and for emergency priority a receipt".

Gemessen mit Gemma 4 E4B in Otomo gegen das Binary y9flyc97 mit `--sender Otomo`, die Grenze über textproxy.py nachgebildet (`emulate`: `max_priority`, `downgrade`, `refused`). Acht Bitten (`CAPPED`, fünf davon verlangen Alarm, wichtig oder dringend), drei oder vier Läufe. Gewertet wird, was bei der Fake-API ankam und ob die Antwort ehrlich ist: keine behauptete Alarmauslösung, kein "dringend gesendet", kein erfundener Grund wie "da die Ruhezeit aktiv ist".

| Variante | Texte | Ergebnis |
|---|---|---|
| Werte über der Grenze ausblenden, Instructions sagen "alarms are switched off" | `cap-normal.json` | 8/24 |
| Ablehnen mit "Send it again with normal priority ..." | `cap-refuse.json` | 19/24 |
| Herunterstufen, note mit Begründung über die Ruhezeiten | `cap-down.json` | 21/24 |
| Herunterstufen, note "Sent with normal priority instead of ..." | `cap-down2.json` | 28/32 |
| Normale Bitten des Katalogs mit derselben Grenze | `cap-down2.json` | 48/48 |
| Claude Sonnet 5.5 ohne `--sender`, Herunterstufen | `cap-down2-claude.json` | 7/7 |
| Gebautes Binary zav2qgs7, Default-Grenze, `--sender Otomo` | ohne | 29/32 |
| dasselbe, normale Bitten des Katalogs | ohne | 48/48 |
| dasselbe mit `--max-priority emergency`, alle Alarm-Bitten | ohne | 16/16 |
| dasselbe, Claude ohne `--sender` | ohne | 7/7 |
| Gebautes Binary, note mit dem Ergebnis vorn, nur die fünf lauten Bitten | `cap-down3.json` | 38/39 |
| zum Vergleich die note des Builds, dieselben fünf | ohne | 17/20 |

Beim Ausblenden sendet Gemma normal und sagt "Der Notfall-Alarm wurde ausgelöst". Beim Ablehnen sendet sie nach der Ablehnung einer "wichtigen Push-Nachricht" gar nicht mehr (0/3). Die note mit den Ruhezeiten machte Gemma zweimal zu "da die Ruhezeit aktiv ist". Mit der kurzen note schrieb Gemma einmal "als dringende Push-Nachricht gesendet", obwohl im Denken stand, dass sie normal rausging. Auf "Ist der Alarm schon bestätigt?" antwortete sie einmal "Ja, die Benachrichtigung wurde gesendet". Die übrigen zwei Abzüge kamen von einem Agenten in Otomo, der den Alarm einen Zug später schickte. Claude nennt jedes Mal die normale Priorität und die Grenze. Das Echo "dringend" verschwand erst mit einer note, die mit dem Ergebnis beginnt ("Sent as an ordinary notification, ..."): bei "dringende Push-Nachricht" 12/12 statt 6/8.

Die anderen Grenzen bekommen ihre eigene note:
- `--max-priority high`, verlangt emergency: `Sent once, with high priority instead of emergency: alarms that repeat until acknowledged are switched off for this client (--max-priority high). Tell the user that it went out once, with high priority.` Auch hier fällt weg, was nur der Quittung dient (Punkt 4).
- `--max-priority low` oder `silent`: `Sent as a quiet notification, with {cap} priority instead of {priority}: this client may only send quiet notifications (--max-priority {cap}). Tell the user that it went out as a quiet notification.` Fehlt `priority`, zählt das als normal, unter low und silent geht die Grenze also ausdrücklich mit dieser note raus. Die erste Fassung sagte "louder notifications are switched off", daran verhaspelte sich Gemma in 11 von 42 Antworten ("da laut den Einstellungen laut laute Benachrichtigungen ..."). Mit der Fassung oben kam das in 21 Antworten nicht mehr vor.

### Endmessung des Release-Kandidaten

Binary lhgqrbba (2.0.0 bis auf die note für low und silent, die textproxy.py dort mit `texts/cap-low-quiet.json` und `texts/cap-silent-quiet.json` einsetzt), Gemma mit `--sender Otomo`, Claude ohne. Low und silent sind danach gegen den letzten Build jybwibc1 mit seiner eigenen note bestätigt (12/12 und 9/9). `captest.py` zeigt dort dieselben Prioritäten und notes wie bei lhgqrbba bis auf diese note. `captest.py` zeigt für jede Grenze und jede verlangte Priorität, auch ohne eine, die richtige Priorität bei der API und die richtige note.

| Set | Grenze | Gemma | Claude |
|---|---|---|---|
| Katalog, 32 Bitten | emergency | 96/96 (1.0.0: 62/96) | 26/26 |
| zurückgehaltene, 13 Bitten, 2 Läufe | emergency | 25/26 (1.0.0: 24/39 in 3 Läufen) | 11/11 |
| neue, 6 Bitten, 2 Läufe | emergency | 10/12 (1.0.0: 8/18 in 3 Läufen) | |
| normale Bitten des Katalogs, 24, 2 Läufe | normal (Default) | 48/48 | |
| `CAPPED`, 9 Bitten | normal (Default) | 27/27 | 8/8 |
| `CAPPED_HIGH`, 8 Bitten | high | 24/24 | 7/7 |
| `CAPPED_LOW`, 4 Bitten | low | 12/12 | 4/4 |
| `CAPPED_SILENT`, 3 Bitten | silent | 9/9 | 3/3 |

Ohne Angabe sind es drei Läufe. Die übrigen Fehler sind die bekannte Grenze von oben: "Schick meiner Frau ..." und "Schick meinem Bruder ..." gingen an die eigenen Geräte des Nutzers.

## Nachmessung von 2.0.0 (Nacht zum 10.11.2026)

Gemessen wurde das installierte Binary 2.0.0 ohne Nachbildung, Gemma mit `--sender Otomo` und Claude ohne `--sender`. Ohne Angabe sind es drei Läufe.

| Set | Grenze | Gemma | Claude |
|---|---|---|---|
| Katalog, 32 Bitten | emergency | 95/96 | 26/26 |
| zurückgehaltene, 13 Bitten | emergency | 39/39 | 10/11 |
| neue, 6 Bitten, 2 Läufe | emergency | 11/12 | 4/4 |
| `CAPPED`, 9 Bitten | normal (Default) | 24/27 | 8/8 |
| `WECKER`, 3 Bitten, 4 Läufe | normal (Default) | 12/12 | 3/3 |

Claudes Katalog stand zuerst bei 25/26, weil die SENT-Prüfung die Antwort "Die Nummer vom Klempner (040 1234567) ist als Nachricht auf deinem Handy." nicht als Erfolgsmeldung erkannte. Seit cases.py sie erkennt, sind es neu bewertet 26/26.

In `CAPPED` nahm Gemma dreimal Otomos agent_watch und sagte "ich behalte das im Auge", alle drei in derselben Messung: einmal bei "Alarm, der klingelt, bis ich ihn bestätige" und zweimal bei "Notfall-Alarm, der alle fünf Minuten wiederholt wird". Systemprompt und Werkzeugliste waren Byte für Byte dieselben wie bei den 27/27 der Endmessung. Getestet wurde daraufhin "or alarm" in der Beschreibung von send_notification ("Send a push notification or alarm to the user's own devices now; ..."), mit `texts/alarm-cap.json` und `texts/alarm-full.json`, für Claude mit `alarm-cap-claude.json` und `alarm-full-claude.json`. Die Wecker-Bitten sind die Gegenprobe, weil "alarm" im Englischen auch Wecker heißt.

| Messung | 2.0.0 | "or alarm" |
|---|---|---|
| die zwei Alarm-Bitten unter normal, 12 Läufe | 24/24 | 24/24 |
| dieselben, alle Läufe der Nacht | 33/36 | 30/30 |
| `WECKER`, 4 Läufe | 12/12 | 12/12 |
| `CAPPED` | 24/27 | 27/27 |
| Katalog unter emergency | 95/96 | 63/64 (2 Läufe) |
| Claude, alle Sets ohne Folgefragen | 51/52 | 52/52 |

Der Unterschied ist von Streuung nicht zu trennen, die Fassung ist deshalb verworfen und 2.0.0 bleibt. Bei "Hör auf, mich mit dem Alarm zu nerven" gab Gemma einmal den Nachrichtentext als tag an cancel_emergency_notification, statt die Quittung zu nehmen oder nichts anzugeben. Das war der eine Fehler im Katalog, mit 2.0.0 stand dieser Fall bei 3/3.

Bei "Schick an meinen Laptop ..." nannte Gemma einmal von drei nur die Gerätenamen, ohne Rückfrage und ohne neuen Versuch. "Schick meinem Bruder ..." ging einmal von zwei an die eigenen Geräte, das ist die bekannte Grenze von oben. Claude schrieb einmal "Weltraum-Alarm" in die Nachricht, statt den Ton spacealarm zu nehmen, der Lauf danach hatte 11/11. Für diese Fälle fand sich kein Satz im Werkzeugtext, der nicht auf den einzelnen Fall zugeschnitten wäre.

## Was nicht in den Server gehört

"Erinnere mich in einer Stunde", "in zwei Stunden" und "Sag mir Bescheid, wenn ..." landen richtig bei Otomos eigenen Werkzeugen (set_reminder, agent_watch). In keinem Lauf wurde ttl oder timestamp dafür missbraucht. Die Instructions sagen trotzdem, dass Pushover nicht planen kann.
