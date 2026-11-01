# SWORTA

Ein kleines Windows-Programm, das Fotos und Videos von der Kamerakarte in Sekunden sauber sortiert. Karte rein, einmal auf "Importieren" klicken, fertig. Gebaut für Sony-Kameras (Alpha-Serie), weil dort alles in einem Wust aus `DCIM`-Unterordnern und `M4ROOT/CLIP` liegt.

SWORTA sitzt im Tray, braucht kaum Ressourcen und meldet sich nur, wenn wirklich eine Karte mit `DCIM`-Ordner im Rechner steckt.

![Startseite](docs/home.png)

## Was es macht

Alles landet in einem Ordner pro Aufnahmetag, darin getrennt nach Typ:

```
Zielordner/
└── 260112/
    ├── JPG/    .JPG   aus DCIM
    ├── RAW/    .ARW   aus DCIM
    └── VID/    .MP4 und .XML aus PRIVATE/M4ROOT/CLIP
```

Der Ordnername ist das Aufnahmedatum als `JJMMTT`. Ein Shooting über zwei Tage landet also in zwei Ordnern.

- Karte oder USB-Stick einstecken, Benachrichtigung erscheint
- Importieren per Klick, direkt in der Benachrichtigung oder im Fenster
- Was schon im Ziel liegt (gleicher Name, gleiche Größe), wird übersprungen
- Gleicher Name, aber andere Datei, wird nicht überschrieben sondern bekommt ein `_1`
- Änderungsdatum der Dateien bleibt erhalten
- Auf der Karte wird nichts gelöscht

## Download

Die aktuelle Version gibt es unter [Releases](https://github.com/vxnsin/sworta/releases/latest).

- `SWORTA.exe` einzeln, oder
- `SWORTA.zip` mit Exe und README

Keine Installation nötig, einfach die Exe an einen festen Ort legen (zum Beispiel `Dokumente\SWORTA`) und starten. Beim ersten Mal warnt Windows SmartScreen vor einem unbekannten Herausgeber, weil die Exe nicht signiert ist. "Weitere Informationen" und dann "Trotzdem ausführen".

## Einstellungen

![Einstellungen](docs/settings.png)

- **Zielordner**: Standard ist `Bilder\SWORTA`
- **Mit Windows starten**: läuft dann unsichtbar im Tray mit
- **Benachrichtigung bei Karte**: aus, wenn du lieber selbst das Fenster aufmachst
- **Updates automatisch laden**: siehe unten

Die Einstellungen liegen in `%APPDATA%\SWORTA\settings.json`.

## Updates

SWORTA schaut beim Start und danach alle paar Stunden bei GitHub nach einer neueren Version. Ist eine da, lädt es die neue Exe, tauscht sie aus und startet sich neu, nie mitten in einem Import. Wer das nicht will, schaltet "Updates automatisch laden" aus und bekommt dann nur einen Hinweis mit Button. Manuell geht es unter Einstellungen mit "Nach Updates suchen".

## Selber bauen

Rust (stable) und die MSVC-Build-Tools reichen.

```
cargo run              # Entwicklung
cargo build --release  # target/release/sworta.exe
```

## Neue Version veröffentlichen

1. Version in `Cargo.toml` hochzählen
2. Committen und pushen
3. Tag setzen: `git tag v0.2.0 && git push origin v0.2.0`

Die GitHub Action baut daraus die Exe und legt das Release an. Der Tag muss zur Version in `Cargo.toml` passen, sonst bricht der Build ab. Alle installierten SWORTAs ziehen sich das Update danach selbst.

## Lizenz

MIT
