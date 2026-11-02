//! Phones and cameras plugged in by cable. Windows lists them under This PC as portable
//! devices (MTP) without a drive letter, so they are read through the shell like Explorer does.
//!
//! Every call goes to the device and back, and an iPhone may first pull photos from iCloud,
//! so listing a phone can take minutes. Callers run this in the background.

use std::{
    cell::Cell,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::{FILETIME, HWND, S_OK},
        Storage::EnhancedStorage::{PKEY_DateModified, PKEY_Size},
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IStream},
            SystemServices::{SFGAO_FILESYSTEM, SFGAO_FLAGS, SFGAO_FOLDER, SFGAO_STREAM},
        },
        UI::Shell::{
            BHID_SFObject, BHID_Stream, Common::ITEMIDLIST, FOLDERID_ComputerFolder, IEnumIDList, IShellFolder,
            IShellItem, IShellItem2, ILGetSize, KF_FLAG_DEFAULT, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN,
            SHCONTF_NONFOLDERS, SHCreateItemFromIDList, SHCreateItemFromParsingName, SHCreateItemWithParent,
            SHGetIDListFromObject, SHGetKnownFolderItem, SIGDN, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
        },
    },
    core::{HSTRING, Interface},
};

use crate::camera;

/// Once per thread that talks to devices.
pub fn init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

pub struct Device {
    /// Shell parsing name, stable while the device stays plugged in.
    pub key: String,
    pub name: String,
}

pub struct File {
    pub name: String,
    pub size: u64,
    pub mtime: SystemTime,
    /// Shell id of the file; unlike its name it finds the file again from another thread.
    pub pidl: Vec<u8>,
}

fn name_of(it: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe {
        let p = it.GetDisplayName(kind).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn attrs(it: &IShellItem) -> u32 {
    let mask = SFGAO_FLAGS(SFGAO_FOLDER.0 | SFGAO_FILESYSTEM.0 | SFGAO_STREAM.0);
    unsafe { it.GetAttributes(mask).map(|a| a.0).unwrap_or(0) }
}

/// A folder that is not also a file (zip files are both).
fn is_dir(it: &IShellItem) -> bool {
    let a = attrs(it);
    a & SFGAO_FOLDER.0 != 0 && a & SFGAO_STREAM.0 == 0
}

/// The items in a folder; None when the device stopped answering.
fn children(it: &IShellItem) -> Option<Vec<IShellItem>> {
    let folder: IShellFolder = unsafe { it.BindToHandler(None, &BHID_SFObject) }.ok()?;
    let mut en: Option<IEnumIDList> = None;
    let flags = (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0 | SHCONTF_INCLUDEHIDDEN.0) as u32;
    // Without a window the device may not pop up "the device is not responding" dialogs.
    let hr = unsafe { folder.EnumObjects(HWND::default(), flags, &mut en) };
    if hr.is_err() {
        return None;
    }
    let Some(en) = en else { return Some(Vec::new()) };
    let mut out = Vec::new();
    // Every call is a round trip to the device, so fetch in big batches.
    let mut batch: Vec<*mut ITEMIDLIST> = vec![std::ptr::null_mut(); 512];
    loop {
        let mut got = 0u32;
        let hr = unsafe { en.Next(&mut batch, Some(&mut got)) };
        for &p in batch.iter().take(got as usize) {
            if let Ok(c) = unsafe { SHCreateItemWithParent::<_, IShellItem>(None, &folder, p) } {
                out.push(c);
            }
            unsafe { CoTaskMemFree(Some(p as *const _)) };
        }
        if hr.is_err() {
            return None;
        }
        if hr != S_OK || (got as usize) < batch.len() {
            break;
        }
    }
    Some(out)
}

fn pidl_of(it: &IShellItem) -> Option<Vec<u8>> {
    unsafe {
        let p = SHGetIDListFromObject(it).ok()?;
        let bytes = std::slice::from_raw_parts(p as *const u8, ILGetSize(Some(p as *const _)) as usize).to_vec();
        CoTaskMemFree(Some(p as *const _));
        Some(bytes)
    }
}

/// Portable devices under This PC; drives and network folders are skipped.
pub fn list() -> Vec<Device> {
    let Ok(pc) = (unsafe { SHGetKnownFolderItem::<IShellItem>(&FOLDERID_ComputerFolder, KF_FLAG_DEFAULT, None) }) else {
        return Vec::new();
    };
    children(&pc)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| {
            let a = attrs(c);
            a & SFGAO_FOLDER.0 != 0 && a & SFGAO_FILESYSTEM.0 == 0
        })
        .filter_map(|c| {
            let key = name_of(&c, SIGDN_DESKTOPABSOLUTEPARSING)?;
            // Devices sit at `::{This PC}\\?\usb#…`; anything else is a shell folder.
            key.contains(r"\\?\").then(|| Device { name: name_of(&c, SIGDN_NORMALDISPLAY).unwrap_or_default(), key })
        })
        .collect()
}

/// An opened device with its storages ("Internal Storage", an SD card in a phone).
pub struct Opened {
    storages: Vec<IShellItem>,
    /// Set once the device stopped answering; everything after that is skipped.
    gone: Cell<bool>,
    /// Set from outside when the device left the list of devices.
    stop: Arc<AtomicBool>,
}

impl Opened {
    pub fn new(key: &str, stop: Arc<AtomicBool>) -> Option<Self> {
        let dev = unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(key), None).ok()? };
        let storages = children(&dev)?.into_iter().filter(is_dir).collect();
        Some(Self { storages, gone: Cell::new(false), stop })
    }

    /// No storage means the phone is locked or has not allowed access yet.
    pub fn is_empty(&self) -> bool {
        self.storages.is_empty()
    }

    /// The device stopped answering or was unplugged.
    pub fn is_gone(&self) -> bool {
        self.gone.get() || self.stop.load(Ordering::SeqCst)
    }

    /// Every question to a device that is gone makes Windows show a "not responding" dialog,
    /// so the first one that comes back empty ends the whole walk.
    fn lost(&self) {
        self.gone.set(true);
    }

    fn list(&self, dir: &IShellItem) -> Vec<IShellItem> {
        if self.is_gone() {
            return Vec::new();
        }
        children(dir).unwrap_or_else(|| {
            self.lost();
            Vec::new()
        })
    }

    /// Items at `pattern` below any storage; `.` is the storage itself.
    fn find(&self, pattern: &str) -> Vec<IShellItem> {
        let segs: Vec<&str> = camera::segments(pattern).into_iter().filter(|s| *s != ".").collect();
        let mut level = self.storages.clone();
        for seg in segs {
            level = level
                .iter()
                .filter(|d| is_dir(d))
                .flat_map(|d| self.list(d))
                .filter(|c| name_of(c, SIGDN_NORMALDISPLAY).is_some_and(|n| camera::seg_match(seg, &n)))
                .collect();
        }
        level
    }

    pub fn has_path(&self, pattern: &str) -> bool {
        !self.find(pattern).is_empty()
    }

    /// Every file below the camera's folders, with its extension and the folder it came from.
    /// Stops early when the device goes away, see `is_gone`.
    pub fn walk(&self, sources: &[String], f: &mut dyn FnMut(File, &str, &str)) {
        fn go(o: &Opened, dir: &IShellItem, src: &str, f: &mut dyn FnMut(File, &str, &str)) {
            for c in o.list(dir) {
                if o.is_gone() {
                    return;
                }
                if is_dir(&c) {
                    go(o, &c, src, f);
                    continue;
                }
                let Some(name) = name_of(&c, SIGDN_NORMALDISPLAY) else { return o.lost() };
                let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
                if ext.is_empty() || name.starts_with('.') {
                    continue;
                }
                let Some(pidl) = pidl_of(&c) else { return o.lost() };
                let Some((size, mtime)) = details(&c) else { return o.lost() };
                f(File { name, size, mtime, pidl }, &ext, src);
            }
        }
        for src in sources {
            for dir in self.find(src) {
                go(self, &dir, src, f);
            }
        }
    }
}

fn to_system_time(ft: FILETIME) -> SystemTime {
    // FILETIME counts 100 ns steps since 1601.
    let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    let unix = ticks.saturating_sub(116_444_736_000_000_000);
    UNIX_EPOCH + Duration::from_nanos(unix.saturating_mul(100))
}

/// Size and time of a file; None when the device does not answer.
fn details(it: &IShellItem) -> Option<(u64, SystemTime)> {
    let it2 = it.cast::<IShellItem2>().ok()?;
    unsafe {
        let size = it2.GetUInt64(&PKEY_Size).ok()?;
        let mtime = it2.GetFileTime(&PKEY_DateModified).map(to_system_time).unwrap_or_else(|_| SystemTime::now());
        Some((size, mtime))
    }
}

/// Reads a file from the device.
pub struct Reader(IStream);

pub fn read(pidl: &[u8]) -> io::Result<Reader> {
    let it = unsafe { SHCreateItemFromIDList::<IShellItem>(pidl.as_ptr() as *const ITEMIDLIST) }
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "the file is gone from the device"))?;
    let s = unsafe { it.BindToHandler::<_, IStream>(None, &BHID_Stream) }.map_err(io::Error::other)?;
    Ok(Reader(s))
}

impl io::Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0u32;
        let hr = unsafe { self.0.Read(buf.as_mut_ptr() as *mut _, buf.len().min(u32::MAX as usize) as u32, Some(&mut n)) };
        hr.ok().map_err(io::Error::other)?;
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a phone plugged in and unlocked: `SWORTA_PROBE=202610_a cargo test plugged_in -- --ignored --nocapture`.
    /// Only reads from the phone.
    #[test]
    #[ignore]
    fn plugged_in_devices() {
        use std::io::Read;
        let t = std::time::Instant::now();
        let folder = std::env::var("SWORTA_PROBE").unwrap_or_else(|_| ".".into());
        init();
        for d in list() {
            eprintln!("{:?} device {:?}", t.elapsed(), d.name);
            let Some(o) = Opened::new(&d.key, Default::default()) else { continue };
            eprintln!("{:?}   storages: {}", t.elapsed(), o.storages.len());
            let mut files = Vec::new();
            o.walk(&[folder.clone()], &mut |f, _, _| files.push(f));
            let bytes: u64 = files.iter().map(|f| f.size).sum();
            eprintln!("{:?}   {} files, {bytes} bytes", t.elapsed(), files.len());
            // Read from another thread, the way the import does.
            if let Some(f) = files.into_iter().find(|f| f.name.to_lowercase().ends_with(".jpg")) {
                let (name, size) = (f.name.clone(), f.size);
                let got = std::thread::spawn(move || {
                    init();
                    let mut all = Vec::new();
                    read(&f.pidl).unwrap().read_to_end(&mut all).unwrap();
                    all
                })
                .join()
                .unwrap();
                eprintln!("{:?}   read {name}: {} of {size} bytes, starts with {:02X?}", t.elapsed(), got.len(), &got[..4]);
                assert_eq!(got.len() as u64, size);
            }
        }
    }

    #[test]
    fn filetime_to_unix() {
        // 2026-11-02 00:00:00 UTC
        let t = 1_793_577_600u64;
        let ticks = t * 10_000_000 + 116_444_736_000_000_000;
        let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        assert_eq!(to_system_time(ft), UNIX_EPOCH + Duration::from_secs(t));
    }
}
