//! Camera profiles: which camera a card comes from and where its files go.

use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::BufReader,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Used when no camera matches a card.
pub const FALLBACK: &str = "other";

/// The only camera inside the exe; real cameras come from the library in `cameras/`.
const BUILTIN: [&str; 1] = [include_str!("fallback.json")];

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Camera {
    pub id: String,
    pub name: String,
    /// Model names as found in EXIF or the Sony clip XML; a part of the name is enough.
    pub models: Vec<String>,
    /// Paths only this camera creates on a card, `*` matches inside one folder name.
    pub markers: Vec<String>,
    /// Names Windows shows for the device plugged in by cable ("Apple iPhone"); a part is enough.
    pub devices: Vec<String>,
    /// Folders on the card that hold the media.
    pub sources: Vec<String>,
    /// Lowercase extension → subfolder at the destination, empty = skip.
    pub types: BTreeMap<String, String>,
    /// Card folder → subfolder for everything in it, ahead of `types` (Sony proxies in PRIVATE/M4ROOT/SUB are MP4 too).
    pub folders: BTreeMap<String, String>,
    /// Path below the import folder with `{date}`, `{type}` and `{camera}`.
    pub layout: String,
    /// Take the shooting day from the file name (DJI_20261102143000_0001_D.MP4) instead of the file time.
    pub date_from_name: bool,
    /// Always import this camera into this folder; None = general setting.
    pub dest: Option<PathBuf>,
}

/// How a card was matched to its camera.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum How {
    /// The user picked the camera for this card before.
    Card,
    Model,
    Folders,
    Unknown,
}

pub struct Found<'a> {
    pub camera: &'a Camera,
    pub how: How,
    /// Model name read from the files, if any.
    pub model: Option<String>,
}

pub fn builtins() -> Vec<Camera> {
    BUILTIN.iter().map(|s| parse(s).expect("built-in camera")).collect()
}

pub fn is_builtin(id: &str) -> bool {
    builtins().iter().any(|c| c.id == id)
}

fn dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("SWORTA").join("cameras")
}

fn file(id: &str) -> PathBuf {
    dir().join(format!("{id}.json"))
}

/// Built-in cameras, replaced or extended by the user's own files; the fallback stays last.
pub fn load_all() -> Vec<Camera> {
    let mut all = builtins();
    if let Ok(rd) = fs::read_dir(dir()) {
        let mut own: Vec<Camera> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))
            .filter_map(|p| read(&p).ok())
            .collect();
        own.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        for c in own {
            match all.iter_mut().find(|b| b.id == c.id) {
                Some(slot) => *slot = c,
                None => all.push(c),
            }
        }
    }
    all.sort_by_key(|c| c.id == FALLBACK);
    all
}

pub fn parse(text: &str) -> Result<Camera, String> {
    let mut c: Camera = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("Not a SWORTA camera file ({e})."))?;
    c.name = c.name.trim().to_string();
    if c.name.is_empty() {
        return Err("The camera file has no name.".into());
    }
    c.id = slug(if c.id.trim().is_empty() { &c.name } else { &c.id });
    c.types = c
        .types
        .into_iter()
        .map(|(k, v)| (k.trim().trim_start_matches('.').to_lowercase(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect();
    c.folders = c
        .folders
        .into_iter()
        .map(|(k, v)| (k.trim().trim_matches(['/', '\\']).to_string(), v.trim().to_string()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect();
    clean_list(&mut c.models);
    clean_list(&mut c.markers);
    clean_list(&mut c.devices);
    clean_list(&mut c.sources);
    if c.sources.is_empty() {
        c.sources.push("DCIM".into());
    }
    if c.layout.trim().is_empty() {
        c.layout = "{date}/{type}".into();
    }
    Ok(c)
}

fn clean_list(v: &mut Vec<String>) {
    for s in v.iter_mut() {
        *s = s.trim().trim_matches(['/', '\\']).to_string();
    }
    v.retain(|s| !s.is_empty());
}

pub fn read(path: &Path) -> Result<Camera, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("Could not read the file ({e})."))?;
    parse(&text)
}

pub fn to_json(c: &Camera) -> String {
    serde_json::to_string_pretty(c).unwrap_or_default()
}

pub fn save(c: &Camera) -> std::io::Result<()> {
    fs::create_dir_all(dir())?;
    fs::write(file(&c.id), to_json(c))
}

/// Removes the user's file; a built-in camera falls back to its defaults.
pub fn remove(id: &str) {
    let _ = fs::remove_file(file(id));
}

pub fn has_own_file(id: &str) -> bool {
    file(id).is_file()
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "camera".into() } else { out }
}

pub fn unique_id(name: &str, taken: &[Camera]) -> String {
    let base = slug(name);
    let mut id = base.clone();
    let mut n = 2;
    while taken.iter().any(|c| c.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

// ---------- detection ----------

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub fn segments(p: &str) -> Vec<&str> {
    p.split(['/', '\\']).filter(|s| !s.is_empty()).collect()
}

/// Case-insensitive match with `*` for any run of characters.
pub fn seg_match(pat: &str, name: &str) -> bool {
    let (pat, name) = (pat.to_lowercase(), name.to_lowercase());
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return pat == name;
    }
    let mut rest = name.as_str();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(at) = rest.find(part) else { return false };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

/// True if `pattern` (like `DCIM/DJI_*`) exists below `root`.
pub fn has_path(root: &Path, pattern: &str) -> bool {
    fn go(dir: &Path, segs: &[&str]) -> bool {
        let Some((first, rest)) = segs.split_first() else { return true };
        if !first.contains('*') {
            let p = dir.join(first);
            return if rest.is_empty() { p.exists() } else { p.is_dir() && go(&p, rest) };
        }
        let Ok(rd) = fs::read_dir(dir) else { return false };
        rd.flatten().any(|e| {
            seg_match(first, &e.file_name().to_string_lossy())
                && (rest.is_empty() || (e.path().is_dir() && go(&e.path(), rest)))
        })
    }
    let segs = segments(pattern);
    !segs.is_empty() && go(root, &segs)
}

/// A drive counts as a camera card if it has a DCIM folder or a known camera's folders.
pub fn looks_like_card(root: &Path, cameras: &[Camera]) -> bool {
    root.join("DCIM").is_dir() || cameras.iter().any(|c| c.markers.iter().any(|m| has_path(root, m)))
}

fn first_file(dir: &Path, exts: &[&str], depth: u32) -> Option<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in &entries {
        if p.is_file() && exts.contains(&ext(p).as_str()) && !hidden(p) {
            return Some(p.clone());
        }
    }
    if depth == 0 {
        return None;
    }
    entries.iter().filter(|p| p.is_dir()).find_map(|p| first_file(p, exts, depth - 1))
}

fn xml_attr(xml: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = xml.find(&key)? + key.len();
    let end = xml[start..].find('"')?;
    let v = xml[start..start + end].trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// "FUJIFILM" + "X-T5" → "FUJIFILM X-T5"; "NIKON CORPORATION" + "NIKON Z 6" stays "NIKON Z 6".
fn with_make(make: Option<String>, model: String) -> String {
    match make {
        Some(m) if !m.is_empty() => {
            let first = m.split_whitespace().next().unwrap_or(&m).to_lowercase();
            if model.to_lowercase().starts_with(&first) { model } else { format!("{m} {model}") }
        }
        _ => model,
    }
}

fn exif_model(p: &Path) -> Option<String> {
    let mut r = BufReader::new(fs::File::open(p).ok()?);
    let ex = exif::Reader::new().read_from_container(&mut r).ok()?;
    let text = |tag| {
        let field = ex.get_field(tag, exif::In::PRIMARY)?;
        let exif::Value::Ascii(v) = &field.value else { return None };
        let s = String::from_utf8_lossy(v.first()?).trim_matches(char::from(0)).trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    Some(with_make(text(exif::Tag::Make), text(exif::Tag::Model)?))
}

/// Make and model from the Sony clip XML or the EXIF of the first photo.
pub fn read_model(root: &Path) -> Option<String> {
    if let Some(x) = first_file(&root.join("PRIVATE").join("M4ROOT").join("CLIP"), &["xml"], 0) {
        if let Ok(s) = fs::read_to_string(x) {
            if let Some(m) = xml_attr(&s, "modelName") {
                return Some(with_make(xml_attr(&s, "manufacturer"), m));
            }
        }
    }
    // Photos first: a JPG header is quick, a RAW is read whole.
    let photos: [&[&str]; 2] = [
        &["jpg", "jpeg", "hif", "heic"],
        &["arw", "dng", "nef", "cr2", "raf", "orf", "rw2"],
    ];
    photos
        .iter()
        .find_map(|exts| first_file(&root.join("DCIM"), exts, 2).and_then(|f| exif_model(&f)))
}

/// Picks the camera for a card: remembered card, then model name, then folder layout.
pub fn detect<'a>(root: &Path, cameras: &'a [Camera], remembered: Option<&str>) -> Found<'a> {
    let fallback = cameras
        .iter()
        .find(|c| c.id == FALLBACK)
        .or(cameras.last())
        .expect("at least the built-in cameras");
    if let Some(c) = remembered.and_then(|id| cameras.iter().find(|c| c.id == id)) {
        return Found { camera: c, how: How::Card, model: None };
    }

    let model = read_model(root);
    if let Some(m) = &model {
        let n = norm(m);
        // The most specific pattern wins, so "ILCE-7M4" beats "ILCE-".
        let best = cameras
            .iter()
            .flat_map(|c| c.models.iter().map(move |p| (c, norm(p))))
            .filter(|(_, p)| !p.is_empty() && n.contains(p.as_str()))
            .max_by_key(|(_, p)| p.len());
        if let Some((c, _)) = best {
            return Found { camera: c, how: How::Model, model };
        }
    }

    match by_folders(cameras, &|m| has_path(root, m)) {
        Some(c) => Found { camera: c, how: How::Folders, model },
        None => Found { camera: fallback, how: How::Unknown, model },
    }
}

/// The camera whose folders fit best: the most markers found, and on a tie the one with none
/// missing. A GoPro MAX card has `DCIM/100GOPRO` like a HERO, but only the MAX also expects `.360` files.
fn by_folders<'a>(cameras: &'a [Camera], has: &dyn Fn(&str) -> bool) -> Option<&'a Camera> {
    cameras
        .iter()
        .filter_map(|c| {
            let hits = c.markers.iter().filter(|m| has(m)).count();
            (hits > 0).then_some((c, hits, std::cmp::Reverse(c.markers.len() - hits)))
        })
        .max_by_key(|(_, hits, missing)| (*hits, *missing))
        .map(|(c, _, _)| c)
}

/// Picks the camera for a device plugged in by cable: remembered device, then its name, then its folders.
pub fn detect_device<'a>(
    name: &str,
    has_path: &dyn Fn(&str) -> bool,
    cameras: &'a [Camera],
    remembered: Option<&str>,
) -> Found<'a> {
    let fallback = cameras
        .iter()
        .find(|c| c.id == FALLBACK)
        .or(cameras.last())
        .expect("at least the built-in cameras");
    if let Some(c) = remembered.and_then(|id| cameras.iter().find(|c| c.id == id)) {
        return Found { camera: c, how: How::Card, model: None };
    }
    let model = Some(name.to_string());
    let n = norm(name);
    let by_name = cameras
        .iter()
        .flat_map(|c| c.devices.iter().map(move |p| (c, norm(p))))
        .filter(|(_, p)| !p.is_empty() && n.contains(p.as_str()))
        .max_by_key(|(_, p)| p.len());
    if let Some((c, _)) = by_name {
        return Found { camera: c, how: How::Model, model };
    }
    match by_folders(cameras, has_path) {
        Some(c) => Found { camera: c, how: How::Folders, model },
        None => Found { camera: fallback, how: How::Unknown, model },
    }
}

/// Windows volume serial of a drive root; changes when the card is formatted.
pub fn volume_serial(root: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
    let mut serial = 0u32;
    let ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    (ok != 0 && serial != 0).then(|| format!("{serial:08X}"))
}

// ---------- sorting ----------

pub fn ext(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// macOS leaves `._name` and `.DS_Store` on cards.
fn hidden(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path, &fs::Metadata)) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            walk(&p, f);
        } else if !hidden(&p) {
            f(&p, &md);
        }
    }
}

/// Every media file in the camera's folders, with its extension and the folder it came from.
pub fn walk_media(root: &Path, sources: &[String], f: &mut dyn FnMut(&Path, &fs::Metadata, &str, &str)) {
    for src in sources {
        let mut dir = root.to_path_buf();
        dir.extend(segments(src));
        walk(&dir, &mut |p, md| {
            let e = ext(p);
            if !e.is_empty() {
                f(p, md, &e, src);
            }
        });
    }
}

/// Destination subfolder for one file: None = no rule yet, Some("") = skip.
pub fn type_for<'a>(c: &'a Camera, src: &str, ext: &str) -> Option<&'a str> {
    c.folders.get(src).or_else(|| c.types.get(ext)).map(String::as_str)
}

#[derive(Default)]
pub struct Survey {
    /// Files per extension, outside folders that have their own target.
    pub exts: BTreeMap<String, usize>,
    /// Files per card folder.
    pub folders: BTreeMap<String, usize>,
}

/// What is on a card, for the camera editor.
pub fn survey(root: &Path, c: &Camera) -> Survey {
    let mut out = Survey::default();
    walk_media(root, &c.sources, &mut |_, _, e, src| {
        *out.folders.entry(src.to_string()).or_insert(0) += 1;
        if !c.folders.contains_key(src) {
            *out.exts.entry(e.to_string()).or_insert(0) += 1;
        }
    });
    out
}

/// First plausible YYYYMMDD in a file name.
pub fn date_from_name(name: &str) -> Option<NaiveDate> {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let s = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i - s >= 8 {
            let num = |a: usize, z: usize| name[s + a..s + z].parse::<u32>().ok();
            if let (Some(y), Some(m), Some(d)) = (num(0, 4), num(4, 6), num(6, 8)) {
                if (2000..2100).contains(&y) {
                    if let Some(date) = NaiveDate::from_ymd_opt(y as i32, m, d) {
                        return Some(date);
                    }
                }
            }
        }
    }
    None
}

pub fn shoot_date(c: &Camera, path: &Path, mtime: SystemTime) -> NaiveDate {
    if c.date_from_name {
        if let Some(d) = path.file_name().and_then(|n| date_from_name(&n.to_string_lossy())) {
            return d;
        }
    }
    DateTime::<Local>::from(mtime).date_naive()
}

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if "<>:\"|?*/\\".contains(c) || c.is_control() { '_' } else { c })
        .collect()
}

/// Folder for one file: the layout filled in below the import folder.
/// A type may hold subfolders (`VID/PROXY`); nothing can climb out of `dest`.
pub fn target_dir(dest: &Path, layout: &str, date: &str, ty: &str, camera: &str) -> PathBuf {
    let filled = layout
        .replace('\\', "/")
        .replace("{date}", &clean(date))
        .replace("{camera}", &clean(camera))
        .replace("{type}", &ty.replace('\\', "/"));
    let mut p = dest.to_path_buf();
    for seg in segments(&filled) {
        let s = clean(seg);
        let s = s.trim().trim_end_matches('.').trim();
        if !s.is_empty() {
            p.push(s);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempCard(PathBuf);

    impl TempCard {
        fn new(files: &[&str]) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!("sworta-test-{}-{n}", std::process::id()));
            for f in files {
                let p = root.join(f);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, b"x").unwrap();
            }
            Self(root)
        }
    }

    impl Drop for TempCard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The library as it is in the repo. Tests read it from disk, the exe never contains it.
    fn library() -> Vec<(String, String, Camera)> {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("cameras");
        let mut out = Vec::new();
        for cat in fs::read_dir(&base).unwrap().flatten().filter(|e| e.path().is_dir()) {
            for f in fs::read_dir(cat.path()).unwrap().flatten() {
                let p = f.path();
                if ext(&p) != "json" {
                    continue;
                }
                let cam = read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
                out.push((cat.file_name().to_string_lossy().into_owned(), stem, cam));
            }
        }
        out
    }

    fn cams() -> Vec<Camera> {
        let mut v: Vec<Camera> = library().into_iter().map(|(_, _, c)| c).collect();
        v.extend(builtins());
        v
    }

    fn lib(id: &str) -> Camera {
        cams().into_iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn only_the_fallback_is_built_in() {
        let all = builtins();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, FALLBACK);
    }

    #[test]
    fn library_files_are_valid() {
        let lib = library();
        assert!(lib.len() >= 10);
        let mut ids = std::collections::HashSet::new();
        for (cat, stem, c) in &lib {
            assert!(crate::library::CATEGORIES.contains(&cat.as_str()), "unknown category {cat}");
            assert_eq!(stem, &c.id, "file name and id differ for {}", c.name);
            assert_ne!(c.id, FALLBACK);
            assert!(ids.insert(c.id.clone()), "id {} used twice", c.id);
            assert!(
                !c.markers.is_empty() || !c.models.is_empty() || !c.devices.is_empty(),
                "{} can never be recognised",
                c.id
            );
        }
    }

    #[test]
    fn every_library_camera_is_recognised_by_its_folders() {
        let all = cams();
        for (_, _, c) in library().iter().filter(|(_, _, c)| !c.markers.is_empty()) {
            let paths: Vec<String> = c.markers.iter().map(|m| m.replace('*', "100")).collect();
            // `DCIM/DJI_100` is a folder when `DCIM/DJI_100/100.OSV` is a marker too.
            let leaves: Vec<&str> = paths
                .iter()
                .filter(|p| !paths.iter().any(|q| q.starts_with(&format!("{p}/"))))
                .map(String::as_str)
                .collect();
            let card = TempCard::new(&leaves);
            let f = detect(&card.0, &all, None);
            assert_eq!((f.camera.id.as_str(), f.how), (c.id.as_str(), How::Folders), "card for {}", c.id);
        }
    }

    #[test]
    fn unknown_card_uses_fallback() {
        let card = TempCard::new(&["DCIM/100KODAK/DCP_0001.KDC"]);
        let cams = cams();
        let f = detect(&card.0, &cams, None);
        assert_eq!(f.camera.id, FALLBACK);
        assert_eq!(f.how, How::Unknown);
    }

    #[test]
    fn specific_model_beats_brand() {
        let card = TempCard::new(&["PRIVATE/M4ROOT/CLIP/C0001.MP4"]);
        fs::write(
            card.0.join("PRIVATE/M4ROOT/CLIP/C0001M01.XML"),
            r#"<Device manufacturer="Sony" modelName="ILCE-7M4" serialNo="1"/>"#,
        )
        .unwrap();
        let mut cams = cams();
        cams.push(Camera { id: "a7iv".into(), name: "A7 IV".into(), models: vec!["ILCE-7M4".into()], ..Default::default() });
        let f = detect(&card.0, &cams, None);
        assert_eq!(f.camera.id, "a7iv");
        assert_eq!(f.how, How::Model);
        assert_eq!(f.model.as_deref(), Some("Sony ILCE-7M4"));
    }

    #[test]
    fn phones_by_device_name_and_folders() {
        let cams = cams();
        let iphone = detect_device("Apple iPhone", &|_| false, &cams, None);
        assert_eq!((iphone.camera.id.as_str(), iphone.how), ("iphone", How::Model));
        let pixel = detect_device("Pixel 8", &|p| p == "DCIM/Camera", &cams, None);
        assert_eq!((pixel.camera.id.as_str(), pixel.how), ("android", How::Folders));
        let kindle = detect_device("Kindle", &|_| false, &cams, None);
        assert_eq!(kindle.how, How::Unknown);
    }

    #[test]
    fn close_relatives_are_told_apart() {
        let cams = cams();
        let pick = |files: &[&str]| {
            let card = TempCard::new(files);
            detect(&card.0, &cams, None).camera.id.clone()
        };
        assert_eq!(pick(&["DCIM/100GOPRO/GX010001.MP4"]), "gopro-hero");
        assert_eq!(pick(&["DCIM/100GOPRO/GS010001.360"]), "gopro-max");
        assert_eq!(pick(&["DCIM/DJI_001/DJI_20261102143000_0001_D.MP4"]), "dji-action");
        assert_eq!(pick(&["DCIM/DJI_001/DJI_20261102143000_0001_D.OSV"]), "dji-osmo-360");
        assert_eq!(pick(&["DCIM/Camera01/VID_20261102_143000_00_001.insv"]), "insta360");
        assert_eq!(pick(&["DCIM/Camera/PXL_20261102_143000.jpg"]), "android");
    }

    #[test]
    fn make_is_added_once() {
        assert_eq!(with_make(Some("FUJIFILM".into()), "X-T5".into()), "FUJIFILM X-T5");
        assert_eq!(with_make(Some("NIKON CORPORATION".into()), "NIKON Z 6_2".into()), "NIKON Z 6_2");
        assert_eq!(with_make(Some("Apple".into()), "iPhone 15 Pro".into()), "Apple iPhone 15 Pro");
        assert_eq!(with_make(None, "HERO12 Black".into()), "HERO12 Black");
    }

    #[test]
    fn insta360_ace_by_its_model() {
        let cams = cams();
        let best = |model: &str| {
            let n = norm(model);
            cams.iter()
                .flat_map(|c| c.models.iter().map(move |p| (c, norm(p))))
                .filter(|(_, p)| !p.is_empty() && n.contains(p.as_str()))
                .max_by_key(|(_, p)| p.len())
                .map(|(c, _)| c.id.clone())
        };
        assert_eq!(best("Insta360 Ace Pro 2").as_deref(), Some("insta360-ace"));
        assert_eq!(best("Insta360 GO 3S").as_deref(), Some("insta360-ace"));
        assert_eq!(best("Insta360 X5").as_deref(), Some("insta360"));
    }

    #[test]
    fn remembered_card_wins() {
        let card = TempCard::new(&["DCIM/DJI_001/x.MP4"]);
        let cams = cams();
        let f = detect(&card.0, &cams, Some("sony-alpha"));
        assert_eq!((f.camera.id.as_str(), f.how), ("sony-alpha", How::Card));
    }

    #[test]
    fn survey_counts_and_hides_dotfiles() {
        let card = TempCard::new(&["DCIM/a/1.MP4", "DCIM/a/1.LRF", "DCIM/a/2.MP4", "DCIM/a/._2.MP4"]);
        let cam = Camera { sources: vec!["DCIM".into()], ..Default::default() };
        let s = survey(&card.0, &cam);
        assert_eq!(s.exts.get("mp4"), Some(&2));
        assert_eq!(s.exts.get("lrf"), Some(&1));
        assert_eq!(s.folders.get("DCIM"), Some(&3));
    }

    #[test]
    fn sony_proxies_go_to_their_own_folder() {
        let card = TempCard::new(&["PRIVATE/M4ROOT/CLIP/C0001.MP4", "PRIVATE/M4ROOT/SUB/C0001S03.MP4"]);
        let sony = lib("sony-alpha");
        let mut got = Vec::new();
        walk_media(&card.0, &sony.sources, &mut |p, _, e, src| {
            got.push((p.file_name().unwrap().to_string_lossy().into_owned(), type_for(&sony, src, e).map(str::to_string)));
        });
        got.sort();
        assert_eq!(
            got,
            vec![
                ("C0001.MP4".to_string(), Some("VID".to_string())),
                ("C0001S03.MP4".to_string(), Some("PROXY".to_string())),
            ]
        );
    }

    #[test]
    fn wildcard_markers() {
        assert!(seg_match("DJI_*", "dji_001"));
        assert!(seg_match("*_D.mp4", "x_D.MP4"));
        assert!(seg_match("a*b*c", "aXXbYYc"));
        assert!(!seg_match("DJI_*", "100MEDIA"));
    }

    #[test]
    fn dates_from_names() {
        let d = |s| date_from_name(s).map(|d| d.to_string());
        assert_eq!(d("DJI_20261102143000_0001_D.MP4").as_deref(), Some("2026-11-02"));
        assert_eq!(d("IMG_20250101_1200.jpg").as_deref(), Some("2025-01-01"));
        assert_eq!(d("C0001.MP4"), None);
        assert_eq!(d("DSC12345678.ARW"), None);
    }

    #[test]
    fn layout_stays_inside_destination() {
        let dest = Path::new("D:\\Fotos");
        assert_eq!(target_dir(dest, "{date}/{type}", "261102", "VID", "X"), dest.join("261102").join("VID"));
        assert_eq!(target_dir(dest, "{camera}/{date}", "261102", "VID", "A7 IV"), dest.join("A7 IV").join("261102"));
        assert_eq!(target_dir(dest, "../{type}", "d", "../x", "c"), dest.join("x"));
        assert_eq!(
            target_dir(dest, "{date}/{type}", "261102", "VID/PROXY", "c"),
            dest.join("261102").join("VID").join("PROXY")
        );
        assert_eq!(target_dir(dest, "{camera}", "d", "t", "A/B"), dest.join("A_B"));
    }

    #[test]
    fn parse_cleans_up() {
        let c = parse(r#"{"name":" My Cam ","types":{".MP4":" VID "},"sources":["/DCIM/"]}"#).unwrap();
        assert_eq!(c.id, "my-cam");
        assert_eq!(c.types.get("mp4").map(String::as_str), Some("VID"));
        assert_eq!(c.sources, vec!["DCIM".to_string()]);
        assert!(parse(r#"{"name":""}"#).is_err());
    }
}
