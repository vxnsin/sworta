# SWORTA

A small Windows tray app that sorts your camera card for you. Plug in the card, pick a folder, click import, done.

Every camera lays out its card differently. A Sony hides video clips in `PRIVATE/M4ROOT/CLIP`, a DJI Osmo Action writes low-res proxies next to every clip. SWORTA works out which camera a card comes from and sorts it the way you set that camera up. It stays quiet in the tray and only speaks up when a camera card shows up.

![Home](docs/home.png)

## What it does

By default everything ends up in one folder per shooting day, split by type:

```
Destination/
└── 260112/
    ├── JPG/    .JPG from DCIM
    ├── RAW/    .ARW from DCIM
    └── VID/    .MP4 and .XML from PRIVATE/M4ROOT/CLIP
```

The folder name is the capture date (`YYMMDD` or `YYYY-MM-DD`), taken from the file's modified time or, for cameras that put it there, from the file name. A shoot that spans two days lands in two folders.

- Detects SD cards and USB drives that contain a `DCIM` folder or the folders of a known camera
- Asks for the destination folder on every import, because every shoot goes somewhere else (can be switched to a fixed folder)
- Imports can be cancelled at any time; files already copied stay, the half-written one is removed
- Several cards can import at the same time, and several files per card are copied in parallel
- Optional speed limit so the import does not choke your drive or network
- Files that already exist at the destination (same name, size and time) are skipped
- Same name but a different file gets a `_1` suffix instead of being overwritten
- Original modified times are kept
- Nothing is ever deleted from the card

## Cameras

SWORTA does not need the camera, only the card. It reads make and model from the first photo or clip, and if there is none it goes by the folders the camera left behind.

Cameras come from a small library that lives in this repository under [`cameras/`](cameras), sorted into action, mirrorless, cinema, drone, 360 and phone. The CAMERAS page lists it with a search, and one click adds a camera. When an unknown card turns out to match a camera from the library, the card itself offers to add it. Nothing of that is baked into the exe: a new camera merged here shows up in every SWORTA right away, and without internet the last list is kept. Anything SWORTA cannot place at all is sorted by sensible defaults until you set it up.

Setting up a camera happens on the card itself: plug it in, click **Set up**, and SWORTA lists what it found (`MP4 12 · LRF 12 · JPG 30`). You pick a folder for each file type or leave it empty to skip it, choose how the folders are laid out (`date/type`, `camera/date/type`, …) and optionally a fixed import folder for that camera. If a camera later writes a file type it has no folder for yet, the card says so instead of quietly leaving the files behind.

Two cameras of the same brand can each get their own setup, the more specific model name wins (`ILCE-7M4` over `ILCE-`). If SWORTA cannot tell a card apart at all, pick the camera from the list on the card. That choice is remembered for the card until it gets formatted.

Each camera is a small JSON file. **Export** on the camera page writes it out, **Import file** on another machine adds it, so a colleague can hand you their setup. To add a camera for everybody, drop the exported file into the right folder of [`cameras/`](cameras) with a pull request; [`cameras/README.md`](cameras/README.md) explains the format.

Phones plugged in by cable show up like a card. Windows gives an iPhone or an Android phone no drive letter, so SWORTA reads it the way Explorer does: unlock the phone, allow access if it asks, and the card fills in while the files are listed. A full phone can take a few minutes for that, and an iPhone that keeps photos in iCloud first has to fetch them. Files come off a phone one at a time, and unplugging it in the middle stops the read or the import cleanly.

## Notifications

When a card shows up, a small popup appears in the corner of your screen with the card contents and an import button. It stays for a few seconds (or until you close it) and turns into a progress view once the import runs.

![Popup](docs/popup.png)

If you prefer, switch to normal Windows notifications or turn them off in the settings. "Preview popup" shows you what it looks like.

## Download

Grab the latest build from [Releases](https://github.com/vxnsin/sworta/releases/latest):

- **`SWORTA-Setup.exe`**: the installer (recommended). Installs for your user only, no admin rights needed, adds a Start menu entry and an uninstaller.
- **`SWORTA-Portable.zip`**: no installation, just unzip somewhere permanent (for example `Documents\SWORTA`) and run `SWORTA.exe`.
- `SWORTA.exe` on its own is the bare program the in-app updater downloads. You normally do not need it.

Windows SmartScreen will warn about an unknown publisher on first launch because the files are not code-signed: click "More info", then "Run anyway".

Both variants update themselves the same way, see below.

## Settings

![Settings](docs/settings.png)

- **Destination**: ask on every import, or use a fixed folder; folder name format
- **Import**: speed limit, files at once (1 to 4 per card), auto import into the last folder, open the folder when done
- **Notifications**: popup, Windows toast or off, and how long the popup stays
- **System**: start with Windows (runs hidden in the tray), automatic updates

Settings live in `%APPDATA%\SWORTA\settings.json`, your cameras in `%APPDATA%\SWORTA\cameras\`. The main window can be pulled larger at its edges and keeps its size. If a card or phone does not behave, `%APPDATA%\SWORTA\sworta.log` says what SWORTA saw.

## Updates

On startup and every few hours after that, SWORTA checks this repository for a newer release. If there is one, it downloads the new exe, swaps it in and restarts itself, never in the middle of an import. If you turn off automatic updates you just get a notice with an install button. You can also check by hand under Settings.

## Build from source

Stable Rust and the MSVC build tools are enough.

```
cargo run              # development
cargo build --release  # target/release/sworta.exe
```

## Publishing a release

1. Bump `version` in `Cargo.toml`
2. Commit and push
3. Tag it: `git tag v0.2.2 && git push origin v0.2.2`

The GitHub Action builds the exe and creates the release. The tag has to match the version in `Cargo.toml` or the build fails. Every installed SWORTA picks the update up on its own.

## License

MIT
