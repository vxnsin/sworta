use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Frame, Layout, Margin,
    Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, ViewportCommand as VC, WindowLevel,
    pos2, vec2,
};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

use crate::{
    camera::{self, Camera, How},
    config::{self, Notify, Settings},
    dots,
    engine::{self, Card, Handle, Progress, Shared, State},
    update::{self, Update},
};

const BG: Color32 = Color32::from_rgb(0, 0, 0);
const LINE: Color32 = Color32::from_rgb(34, 34, 34);
const DOT: Color32 = Color32::from_rgb(58, 58, 58);
const DIM: Color32 = Color32::from_rgb(26, 26, 26);
const MUTED: Color32 = Color32::from_rgb(125, 125, 125);
const FG: Color32 = Color32::from_rgb(236, 236, 236);
const RED: Color32 = Color32::from_rgb(215, 25, 33);

const FULL: egui::Vec2 = vec2(460.0, 760.0);
const POPUP: egui::Vec2 = vec2(390.0, 176.0);
const DEMO: &str = "DEMO";

const SPEEDS: [u32; 7] = [0, 10, 25, 50, 100, 200, 400];
const PARALLEL: [u32; 4] = [1, 2, 3, 4];
const POPUP_SECS: [u32; 4] = [5, 12, 30, 0];
const LAYOUTS: [(&str, &str); 3] = [
    ("DATE/TYPE", "{date}/{type}"),
    ("CAMERA/DATE/TYPE", "{camera}/{date}/{type}"),
    ("DATE", "{date}"),
];

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Home,
    Cameras,
    Settings,
}

/// A camera being set up or edited on the CAMERAS tab.
struct Editor {
    cam: Camera,
    /// Id before editing; None for a new camera.
    orig: Option<String>,
    /// Card the editor was opened from; a new camera is remembered for it.
    root: Option<String>,
    /// Model name that card reported.
    seen_model: Option<String>,
    /// What is on that card.
    found: camera::Survey,
    models: String,
    markers: String,
    sources: String,
    new_ext: String,
    custom_layout: bool,
    error: Option<String>,
}

enum EdAction {
    Save,
    Cancel,
    Export,
    Delete,
}

fn guess_type(ext: &str) -> String {
    match ext {
        "jpg" | "jpeg" | "heic" | "hif" | "png" => "JPG".into(),
        "arw" | "cr2" | "cr3" | "nef" | "raf" | "orf" | "rw2" | "dng" => "RAW".into(),
        "mp4" | "mov" | "mts" | "avi" => "VID".into(),
        "wav" | "mp3" | "m4a" => "AUDIO".into(),
        // Proxies and thumbnails the cameras write next to the real files.
        "lrf" | "lrv" | "thm" => String::new(),
        _ => ext.to_uppercase(),
    }
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

impl Editor {
    fn new(cam: Camera, orig: Option<String>, root: Option<String>, found: camera::Survey) -> Self {
        let mut cam = cam;
        for e in found.exts.keys() {
            cam.types.entry(e.clone()).or_insert_with(|| guess_type(e));
        }
        Self {
            models: cam.models.join(", "),
            markers: cam.markers.join(", "),
            sources: cam.sources.join(", "),
            custom_layout: !LAYOUTS.iter().any(|(_, l)| *l == cam.layout),
            cam,
            orig,
            root,
            seen_model: None,
            found,
            new_ext: String::new(),
            error: None,
        }
    }

    fn blank() -> Self {
        let cam = Camera {
            name: "New camera".into(),
            sources: vec!["DCIM".into()],
            layout: LAYOUTS[0].1.into(),
            types: ["jpg", "mp4"].iter().map(|e| (e.to_string(), guess_type(e))).collect(),
            ..Default::default()
        };
        Self::new(cam, None, None, camera::Survey::default())
    }

    fn from_card(card: &Card) -> Self {
        let found = camera::survey(Path::new(&card.root), &card.camera);
        let mut ed = if card.how == How::Unknown {
            // Start from what is on the card, not from the long default list.
            let cam = Camera {
                name: card.model.clone().unwrap_or_else(|| "New camera".into()),
                models: card.model.iter().cloned().collect(),
                sources: card.camera.sources.clone(),
                layout: card.camera.layout.clone(),
                ..Default::default()
            };
            Self::new(cam, None, Some(card.root.clone()), found)
        } else {
            Self::new(card.camera.clone(), Some(card.camera.id.clone()), Some(card.root.clone()), found)
        };
        ed.seen_model = card.model.clone();
        ed
    }

    fn finished(&self) -> Camera {
        let mut c = self.cam.clone();
        c.name = c.name.trim().to_string();
        c.models = split_list(&self.models);
        c.markers = split_list(&self.markers);
        c.sources = split_list(&self.sources);
        let sources = c.sources.clone();
        c.folders.retain(|k, v| sources.contains(k) && !v.trim().is_empty());
        c
    }
}

#[derive(PartialEq, Clone)]
enum Mode {
    Hidden,
    Popup(String),
    Full,
}

pub struct App {
    h: Handle,
    tab: Tab,
    mode: Mode,
    popup_since: Instant,
    done_seen: Option<Instant>,
    synced: bool,
    editor: Option<Editor>,
    /// Result of the last camera import, shown on the CAMERAS tab.
    cam_msg: Option<String>,
    _tray: TrayIcon,
}

struct Snap {
    cards: Vec<Card>,
    jobs: HashMap<String, Progress>,
    settings: Settings,
    cameras: Vec<Camera>,
    update: Update,
}

pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let cell = size as f32 / 5.0;
    for y in 0..size {
        for x in 0..size {
            let (gx, gy) = (x as f32 / cell, y as f32 / cell);
            let (cx, cy) = (gx.floor() + 0.5, gy.floor() + 0.5);
            let d = ((gx - cx).powi(2) + (gy - cy).powi(2)).sqrt();
            if d < 0.3 {
                let center = gx.floor() == 2.0 && gy.floor() == 2.0;
                let c = if center { [215, 25, 33] } else { [237, 237, 237] };
                let i = ((y * size + x) * 4) as usize;
                px[i..i + 3].copy_from_slice(&c);
                px[i + 3] = 255;
            }
        }
    }
    px
}

fn work_area() -> (f32, f32, f32, f32) {
    use windows_sys::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{SPI_GETWORKAREA, SystemParametersInfoW},
    };
    let mut r = RECT { left: 0, top: 0, right: 1920, bottom: 1040 };
    unsafe {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as *mut _, 0);
    }
    (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32)
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, hidden: bool) -> Self {
        let ctx = cc.egui_ctx.clone();
        apply_style(&ctx);

        let settings = config::load();
        if !cfg!(debug_assertions) {
            config::set_autostart(settings.autostart);
        }
        let h: Handle = Arc::new(Mutex::new(Shared {
            cards: Vec::new(),
            jobs: HashMap::new(),
            settings,
            cameras: camera::load_all(),
            picked: HashMap::new(),
            update: Default::default(),
            popup_request: None,
            want_full: false,
        }));
        engine::spawn_watcher(h.clone(), ctx.clone());
        update::spawn_checker(h.clone(), ctx.clone());

        let open = MenuItem::with_id("open", "Open", true, None);
        let quit = MenuItem::with_id("quit", "Quit", true, None);
        let menu = Menu::new();
        let _ = menu.append_items(&[&open, &quit]);
        let tray = TrayIconBuilder::new()
            .with_tooltip("SWORTA")
            .with_icon(tray_icon::Icon::from_rgba(icon_rgba(64), 64, 64).unwrap())
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .expect("tray icon");

        let (c, hh) = (ctx.clone(), h.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| match e.id.0.as_str() {
            "open" => engine::show_window(&hh, &c),
            "quit" => update::quit(&c),
            _ => {}
        }));
        let (c, hh) = (ctx.clone(), h.clone());
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                engine::show_window(&hh, &c);
            }
        }));

        Self {
            h,
            tab: Tab::Home,
            mode: if hidden { Mode::Hidden } else { Mode::Full },
            popup_since: Instant::now(),
            done_seen: None,
            synced: false,
            editor: None,
            cam_msg: None,
            _tray: tray,
        }
    }

    fn enter_full(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let (l, t, r, b) = work_area();
        let (w, h) = ((r - l) / ppp, (b - t) / ppp);
        ctx.send_viewport_cmd(VC::InnerSize(FULL));
        ctx.send_viewport_cmd(VC::OuterPosition(pos2(
            l / ppp + (w - FULL.x) / 2.0,
            t / ppp + ((h - FULL.y) / 2.0).max(0.0),
        )));
        ctx.send_viewport_cmd(VC::WindowLevel(WindowLevel::Normal));
        ctx.send_viewport_cmd(VC::Visible(true));
        ctx.send_viewport_cmd(VC::Minimized(false));
        ctx.send_viewport_cmd(VC::Focus);
        self.mode = Mode::Full;
    }

    fn enter_popup(&mut self, ctx: &egui::Context, root: String) {
        let ppp = ctx.pixels_per_point();
        let (_, _, r, b) = work_area();
        ctx.send_viewport_cmd(VC::InnerSize(POPUP));
        ctx.send_viewport_cmd(VC::OuterPosition(pos2(
            r / ppp - POPUP.x - 16.0,
            b / ppp - POPUP.y - 16.0,
        )));
        ctx.send_viewport_cmd(VC::WindowLevel(WindowLevel::AlwaysOnTop));
        ctx.send_viewport_cmd(VC::Visible(true));
        self.mode = Mode::Popup(root);
        self.popup_since = Instant::now();
        self.done_seen = None;
    }

    fn hide(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(VC::Visible(false));
        self.mode = Mode::Hidden;
    }

    /// Applies requests that background threads left in the shared state.
    fn pump(&mut self, ctx: &egui::Context) {
        let (req, full) = {
            let mut g = self.h.lock().unwrap();
            (g.popup_request.take(), std::mem::take(&mut g.want_full))
        };
        if full {
            self.enter_full(ctx);
        } else if let Some(root) = req {
            if self.mode != Mode::Full {
                self.enter_popup(ctx, root);
            }
        }
    }

    fn snap(&self) -> Snap {
        let g = self.h.lock().unwrap();
        Snap {
            cards: g.cards.clone(),
            jobs: g.jobs.clone(),
            settings: g.settings.clone(),
            cameras: g.cameras.clone(),
            update: g.update.clone(),
        }
    }

    fn open_setup(&mut self, card: &Card) {
        self.editor = Some(Editor::from_card(card));
        self.tab = Tab::Cameras;
    }

    fn reload_cameras(&mut self, ctx: &egui::Context) {
        self.h.lock().unwrap().cameras = camera::load_all();
        engine::rescan_all(&self.h, ctx);
    }

    fn save_editor(&mut self, ctx: &egui::Context) {
        let Some(ed) = self.editor.as_mut() else { return };
        let mut cam = ed.finished();
        if cam.name.is_empty() {
            ed.error = Some("Give the camera a name.".into());
            return;
        }
        if ed.orig.is_none() {
            let cams = self.h.lock().unwrap().cameras.clone();
            cam.id = camera::unique_id(&cam.name, &cams);
        }
        let cam = match camera::parse(&camera::to_json(&cam)) {
            Ok(c) => c,
            Err(e) => {
                ed.error = Some(e);
                return;
            }
        };
        if let Err(e) = camera::save(&cam) {
            ed.error = Some(format!("Could not save the camera ({e})."));
            return;
        }
        // A new camera set up from a card belongs to that card, even without a model name to go by.
        let pin = if ed.orig.is_none() { ed.root.clone() } else { None };
        self.editor = None;
        self.h.lock().unwrap().cameras = camera::load_all();
        match pin {
            Some(root) => engine::assign(&self.h, ctx, &root, &cam.id),
            None => engine::rescan_all(&self.h, ctx),
        }
    }

    fn delete_editor(&mut self, ctx: &egui::Context) {
        let Some(id) = self.editor.as_ref().and_then(|e| e.orig.clone()) else { return };
        camera::remove(&id);
        if !camera::is_builtin(&id) {
            let mut g = self.h.lock().unwrap();
            g.settings.cards.retain(|_, v| *v != id);
            g.picked.retain(|_, v| *v != id);
            config::save(&g.settings);
        }
        self.editor = None;
        self.reload_cameras(ctx);
    }

    fn export_editor(&mut self) {
        let Some(ed) = self.editor.as_mut() else { return };
        let mut cam = ed.finished();
        if cam.id.is_empty() {
            cam.id = camera::slug(&cam.name);
        }
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}.json", cam.name))
            .add_filter("SWORTA camera", &["json"])
            .save_file()
        else {
            return;
        };
        ed.error = std::fs::write(&path, camera::to_json(&cam))
            .err()
            .map(|e| format!("Could not write the file ({e})."));
    }

    fn import_camera(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new().add_filter("SWORTA camera", &["json"]).pick_file() else {
            return;
        };
        self.cam_msg = Some(match camera::read(&path) {
            Ok(c) => match camera::save(&c) {
                Ok(()) => {
                    self.reload_cameras(ctx);
                    format!("Added {}.", c.name)
                }
                Err(e) => format!("Could not save the camera ({e})."),
            },
            Err(e) => e,
        });
    }
}

fn apply_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.override_text_color = Some(FG);
    v.selection.bg_fill = RED;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        for (_, f) in s.text_styles.iter_mut() {
            f.family = FontFamily::Monospace;
        }
        s.spacing.item_spacing = vec2(10.0, 8.0);
        s.spacing.scroll.floating = true;
        s.spacing.scroll.bar_width = 3.0;
    });
}

// ---------- widgets ----------

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Primary,
    Ghost,
}

fn btn(ui: &mut Ui, text: &str, kind: Btn, enabled: bool) -> bool {
    let hover_probe = ui.next_widget_position();
    let _ = hover_probe;
    let (fill, fg, stroke) = (BG, FG, LINE);
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::monospace(12.0), fg);
    let size = galley.size() + vec2(28.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hot = enabled && resp.hovered();
    let (fill, fg, stroke) = match (kind, enabled) {
        (_, false) => (fill, MUTED, stroke),
        (Btn::Primary, _) => (if hot { Color32::from_rgb(238, 44, 52) } else { RED }, Color32::WHITE, RED),
        (Btn::Ghost, _) => (
            if hot { Color32::from_gray(24) } else { BG },
            FG,
            if hot { FG } else { Color32::from_gray(80) },
        ),
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(2), fill);
    p.rect_stroke(rect, CornerRadius::same(2), Stroke::new(1.0, stroke), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, text, FontId::monospace(12.0), fg);
    if hot {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    enabled && resp.clicked()
}

fn rule(ui: &mut Ui, num: &str, title: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let p = ui.painter();
    let r = p.text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        format!("{num}  {title}"),
        FontId::monospace(11.0),
        MUTED,
    );
    let mut x = r.right() + 12.0;
    while x < rect.right() {
        p.circle_filled(pos2(x, rect.center().y), 0.9, DOT);
        x += 6.0;
    }
}

fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(30.0);
        ui.label(RichText::new(label).size(12.0));
        ui.with_layout(Layout::right_to_left(Align::Center), add);
    });
}

fn toggle(ui: &mut Ui, on: &mut bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 20.0), Sense::click());
    let mut changed = false;
    if resp.clicked() {
        *on = !*on;
        changed = true;
    }
    let p = ui.painter();
    p.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, if *on { RED } else { Color32::from_gray(80) }),
        StrokeKind::Inside,
    );
    let k = if *on {
        Rect::from_min_size(rect.min + vec2(22.0, 3.0), vec2(15.0, 14.0))
    } else {
        Rect::from_min_size(rect.min + vec2(3.0, 3.0), vec2(15.0, 14.0))
    };
    p.rect_filled(k, CornerRadius::same(1), if *on { RED } else { MUTED });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    changed
}

/// Row of dots; click or drag picks one of `n` steps.
fn steps(ui: &mut Ui, idx: &mut usize, n: usize) -> bool {
    let w = 18.0 * n as f32;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click_and_drag());
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        let i = (((pos.x - rect.left()) / w) * n as f32).floor().clamp(0.0, n as f32 - 1.0) as usize;
        if i != *idx {
            *idx = i;
            changed = true;
        }
    }
    let p = ui.painter();
    for i in 0..n {
        let c = pos2(rect.left() + 9.0 + i as f32 * 18.0, rect.center().y);
        if i == *idx {
            p.circle_filled(c, 5.5, RED);
        } else if i < *idx {
            p.circle_filled(c, 3.0, FG);
        } else {
            p.circle_filled(c, 2.5, DOT);
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    changed
}

fn choice(ui: &mut Ui, idx: &mut usize, labels: &[&str]) -> bool {
    let mut changed = false;
    // Inside a right-to-left row the items would come out mirrored.
    let mut order: Vec<usize> = (0..labels.len()).collect();
    if ui.layout().prefer_right_to_left() {
        order.reverse();
    }
    ui.horizontal(|ui| {
        for i in order {
            let l = labels[i];
            let on = i == *idx;
            let g = ui.painter().layout_no_wrap(
                l.to_string(),
                FontId::monospace(11.0),
                if on { FG } else { MUTED },
            );
            let (rect, resp) = ui.allocate_exact_size(g.size() + vec2(18.0, 10.0), Sense::click());
            if on {
                ui.painter().circle_filled(pos2(rect.left() + 4.0, rect.center().y), 2.5, RED);
            }
            ui.painter().galley(pos2(rect.left() + 12.0, rect.center().y - g.size().y / 2.0), g, FG);
            if resp.clicked() && !on {
                *idx = i;
                changed = true;
            }
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
        }
    });
    changed
}

fn dot_text(ui: &mut Ui, t: &str, pitch: f32, on: Color32) {
    let (rect, _) = ui.allocate_exact_size(dots::text_size(t, pitch), Sense::hover());
    dots::text(ui.painter(), rect.min, t, pitch, on, DIM);
}

fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 { format!("{:.1} GB", b / 1e9) } else { format!("{:.0} MB", b / 1e6) }
}

fn fmt_speed(bps: f64) -> String {
    format!("{:.0} MB/S", bps / 1e6)
}

fn fmt_eta(p: &Progress) -> String {
    let sp = p.speed();
    if sp <= 0.0 {
        return "--:--".into();
    }
    let s = ((p.bytes_total.saturating_sub(p.bytes_done)) as f64 / sp) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn demo_card() -> Card {
    Card {
        root: "E:\\".into(),
        serial: None,
        camera: camera::builtins().remove(0),
        how: How::Model,
        model: Some("ILCE-7M4".into()),
        items: Arc::new(Vec::new()),
        counts: BTreeMap::from([("JPG".into(), 128), ("RAW".into(), 128), ("VID".into(), 6)]),
        unsorted: BTreeMap::new(),
        bytes: 14_200_000_000,
    }
}

fn how_text(c: &Card) -> String {
    match c.how {
        How::Card => "picked for this card".into(),
        How::Model => format!("model {}", c.model.as_deref().unwrap_or("?")),
        How::Folders => "recognised by its folders".into(),
        How::Unknown => "unknown camera".into(),
    }
}

fn field(ui: &mut Ui, label: &str, v: &mut String) {
    ui.label(RichText::new(label).color(MUTED).size(11.0));
    ui.add(egui::TextEdit::singleline(v).desired_width(f32::INFINITY).font(FontId::monospace(12.0)));
}

fn note(ui: &mut Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).color(MUTED).size(11.0));
}

fn editor_ui(ui: &mut Ui, ed: &mut Editor, s: &Settings) -> Option<EdAction> {
    let mut act = None;
    rule(ui, "01", if ed.orig.is_some() { "EDIT CAMERA" } else { "NEW CAMERA" });
    field(ui, "Name", &mut ed.cam.name);
    field(ui, "Model names, a part is enough", &mut ed.models);
    if let Some(m) = &ed.seen_model {
        note(ui, format!("This card says: {m}"));
    }
    field(ui, "Folders only this camera makes (DCIM/DJI_*)", &mut ed.markers);
    field(ui, "Folders with photos and videos", &mut ed.sources);
    ui.add_space(10.0);

    rule(ui, "02", "FILES");
    note(ui, "Card folders: a folder name here takes everything in that folder, like proxies.");
    for src in split_list(&ed.sources) {
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            ui.label(RichText::new(&src).size(11.5));
            let n = ed.found.folders.get(&src).map(|n| format!("{n}")).unwrap_or_default();
            ui.label(RichText::new(n).color(MUTED).size(11.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let v = ed.cam.folders.entry(src.clone()).or_default();
                ui.add(
                    egui::TextEdit::singleline(v)
                        .hint_text("by file type")
                        .desired_width(110.0)
                        .font(FontId::monospace(12.0)),
                );
                ui.label(RichText::new("→").color(MUTED));
            });
        });
    }
    ui.add_space(6.0);
    note(ui, "File types everywhere else:");
    let keys: Vec<String> = ed.cam.types.keys().cloned().collect();
    let mut remove = None;
    for k in &keys {
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            ui.label(RichText::new(format!("{:<5}", k.to_uppercase())).size(12.0));
            let n = ed.found.exts.get(k).map(|n| format!("{n} on card")).unwrap_or_default();
            ui.label(RichText::new(n).color(MUTED).size(11.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if btn(ui, "×", Btn::Ghost, true) {
                    remove = Some(k.clone());
                }
                if let Some(v) = ed.cam.types.get_mut(k) {
                    ui.add(
                        egui::TextEdit::singleline(v)
                            .hint_text("skip")
                            .desired_width(110.0)
                            .font(FontId::monospace(12.0)),
                    );
                }
                ui.label(RichText::new("→").color(MUTED));
            });
        });
    }
    if let Some(k) = remove {
        ed.cam.types.remove(&k);
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut ed.new_ext)
                .hint_text("extension, e.g. wav")
                .desired_width(170.0)
                .font(FontId::monospace(12.0)),
        );
        let e = ed.new_ext.trim().trim_start_matches('.').to_lowercase();
        if btn(ui, "ADD", Btn::Ghost, !e.is_empty()) {
            ed.cam.types.entry(e.clone()).or_insert_with(|| guess_type(&e));
            ed.new_ext.clear();
        }
    });
    note(ui, "Folder per file type. Empty means the type is not imported.");
    ui.add_space(10.0);

    rule(ui, "03", "DESTINATION");
    let labels: Vec<&str> = LAYOUTS.iter().map(|(n, _)| *n).chain(["CUSTOM"]).collect();
    let mut li = if ed.custom_layout {
        LAYOUTS.len()
    } else {
        LAYOUTS.iter().position(|(_, l)| *l == ed.cam.layout).unwrap_or(LAYOUTS.len())
    };
    if choice(ui, &mut li, &labels) {
        ed.custom_layout = li == LAYOUTS.len();
        if !ed.custom_layout {
            ed.cam.layout = LAYOUTS[li].1.into();
        }
    }
    if ed.custom_layout {
        field(ui, "{date}, {type} and {camera}, folders split by /", &mut ed.cam.layout);
    }
    let mut di = ed.cam.date_from_name as usize;
    row(ui, "Day taken from", |ui| {
        choice(ui, &mut di, &["FILE TIME", "FILE NAME"]);
    });
    ed.cam.date_from_name = di == 1;

    let mut fi = ed.cam.dest.is_some() as usize;
    let mut fixed_changed = false;
    row(ui, "Import folder", |ui| {
        fixed_changed = choice(ui, &mut fi, &["SETTINGS", "FIXED"]);
    });
    if fixed_changed {
        ed.cam.dest = if fi == 1 {
            rfd::FileDialog::new().set_directory(&s.dest).pick_folder()
        } else {
            None
        };
    }
    match &ed.cam.dest {
        Some(d) => {
            note(ui, format!("always {}", d.display()));
            if btn(ui, "CHANGE FOLDER", Btn::Ghost, true) {
                if let Some(p) = rfd::FileDialog::new().set_directory(d).pick_folder() {
                    ed.cam.dest = Some(p);
                }
            }
        }
        None if s.ask_dest => note(ui, "asks on every import, like in Settings"),
        None => note(ui, format!("{}, like in Settings", s.dest.display())),
    }
    let base = ed.cam.dest.clone().unwrap_or_else(|| s.dest.clone());
    let date = chrono::Local::now().format(if s.long_date { "%Y-%m-%d" } else { "%y%m%d" }).to_string();
    let ty = ed.cam.types.values().find(|v| !v.is_empty()).cloned().unwrap_or_default();
    let example = camera::target_dir(&base, &ed.cam.layout, &date, &ty, ed.cam.name.trim());
    ui.label(RichText::new(format!("→ {}\\…", example.display())).size(11.0));
    ui.add_space(10.0);

    if let Some(e) = &ed.error {
        ui.label(RichText::new(e).color(RED).size(11.5));
    }
    ui.horizontal(|ui| {
        if btn(ui, "SAVE", Btn::Primary, true) {
            act = Some(EdAction::Save);
        }
        if btn(ui, "CANCEL", Btn::Ghost, true) {
            act = Some(EdAction::Cancel);
        }
        if btn(ui, "EXPORT", Btn::Ghost, true) {
            act = Some(EdAction::Export);
        }
        if let Some(id) = &ed.orig {
            if camera::has_own_file(id) {
                let label = if camera::is_builtin(id) { "RESET" } else { "DELETE" };
                if btn(ui, label, Btn::Ghost, true) {
                    act = Some(EdAction::Delete);
                }
            }
        }
    });
    act
}

fn drive_label(root: &str) -> String {
    root.chars().take(2).collect::<String>().to_uppercase()
}

// ---------- screens ----------

enum Bar {
    Min,
    Close,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.synced {
            self.synced = true;
            if self.mode == Mode::Hidden {
                ctx.send_viewport_cmd(VC::Visible(false));
            }
        }
        self.pump(&ctx);
        if ctx.input(|i| i.viewport().close_requested())
            && !update::QUITTING.load(std::sync::atomic::Ordering::SeqCst)
        {
            ctx.send_viewport_cmd(VC::CancelClose);
            self.hide(&ctx);
        }

        let snap = self.snap();
        match self.mode.clone() {
            Mode::Hidden => {}
            Mode::Full => self.full_ui(ui, &ctx, &snap),
            Mode::Popup(root) => self.popup_ui(ui, &ctx, &snap, &root),
        }
        ui.painter().rect_stroke(
            ui.max_rect(),
            CornerRadius::ZERO,
            Stroke::new(1.0, Color32::from_gray(48)),
            StrokeKind::Inside,
        );
    }
}

impl App {
    fn titlebar(&mut self, ui: &mut Ui, ctx: &egui::Context) -> Option<Bar> {
        let (bar, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click_and_drag());
        if resp.drag_started() {
            ctx.send_viewport_cmd(VC::StartDrag);
        }
        let p = ui.painter().clone();
        let cy = bar.center().y;
        for i in 0..3 {
            for j in 0..3 {
                let c = pos2(bar.left() + 16.0 + i as f32 * 5.0, cy - 5.0 + j as f32 * 5.0);
                p.circle_filled(c, 1.5, if i == 1 && j == 1 { RED } else { MUTED });
            }
        }
        p.text(pos2(bar.left() + 40.0, cy), Align2::LEFT_CENTER, "SWORTA", FontId::monospace(11.0), MUTED);

        let mut out = None;
        for (k, off) in [(Bar::Min, 62.0), (Bar::Close, 24.0)] {
            let r = Rect::from_center_size(pos2(bar.right() - off, cy), vec2(30.0, 28.0));
            let id = egui::Id::new(("titlebar", off as i32));
            let rs = ui.interact(r, id, Sense::click());
            let hot = rs.hovered();
            let col = if hot { if matches!(k, Bar::Close) { RED } else { FG } } else { MUTED };
            let s = Stroke::new(1.3, col);
            let c = r.center();
            match k {
                Bar::Min => {
                    p.line_segment([c - vec2(5.0, 0.0), c + vec2(5.0, 0.0)], s);
                }
                Bar::Close => {
                    p.line_segment([c - vec2(5.0, 5.0), c + vec2(5.0, 5.0)], s);
                    p.line_segment([c - vec2(5.0, -5.0), c + vec2(5.0, -5.0)], s);
                }
            }
            if rs.clicked() {
                out = Some(k);
            }
        }
        p.line_segment(
            [pos2(bar.left(), bar.bottom()), pos2(bar.right(), bar.bottom())],
            Stroke::new(1.0, LINE),
        );
        out
    }

    fn full_ui(&mut self, ui: &mut Ui, ctx: &egui::Context, snap: &Snap) {
        match self.titlebar(ui, ctx) {
            Some(Bar::Min) => ctx.send_viewport_cmd(VC::Minimized(true)),
            Some(Bar::Close) => self.hide(ctx),
            None => {}
        }
        Frame::new().inner_margin(Margin::symmetric(26, 14)).show(ui, |ui| {
            ui.horizontal(|ui| {
                dot_text(ui, "SWORTA", 5.0, FG);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    let running = snap.jobs.values().any(|p| p.state == State::Running);
                    let c = if running {
                        RED
                    } else if !snap.cards.is_empty() {
                        FG
                    } else {
                        Color32::from_gray(55)
                    };
                    ui.painter().circle_filled(r.center(), 4.5, c);
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (t, name) in [(Tab::Home, "HOME"), (Tab::Cameras, "CAMERAS"), (Tab::Settings, "SETTINGS")] {
                    let on = self.tab == t;
                    let g = ui.painter().layout_no_wrap(
                        name.to_string(),
                        FontId::monospace(12.0),
                        if on { FG } else { MUTED },
                    );
                    let (rect, resp) = ui.allocate_exact_size(g.size() + vec2(4.0, 12.0), Sense::click());
                    ui.painter().galley(rect.min + vec2(2.0, 2.0), g, FG);
                    if on {
                        ui.painter().circle_filled(pos2(rect.center().x, rect.bottom() - 1.0), 2.2, RED);
                    }
                    if resp.clicked() {
                        self.tab = t;
                    }
                    if resp.hovered() {
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                    }
                    ui.add_space(8.0);
                }
            });
            ui.add_space(6.0);
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // Keep controls clear of the floating scroll bar.
                ui.set_max_width(ui.available_width() - 14.0);
                match self.tab {
                    Tab::Home => self.home(ui, ctx, snap),
                    Tab::Cameras => self.cameras(ui, ctx, snap),
                    Tab::Settings => self.settings(ui, ctx, snap),
                }
            });
        });
    }

    fn home(&mut self, ui: &mut Ui, ctx: &egui::Context, snap: &Snap) {
        match &snap.update {
            Update::Available(r) => {
                rule(ui, "!!", "UPDATE");
                ui.label(RichText::new(format!("{} is available on GitHub.", r.tag)).size(12.0));
                if btn(ui, "INSTALL", Btn::Primary, true) {
                    update::install(&self.h, ctx, r.clone(), false);
                }
                ui.add_space(8.0);
            }
            Update::Installing(pct) => {
                rule(ui, "!!", "UPDATE");
                ui.label(
                    RichText::new(if *pct < 100 {
                        format!("Downloading update … {pct}%")
                    } else {
                        "Installing, SWORTA restarts in a moment …".to_string()
                    })
                    .size(12.0),
                );
                let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
                dots::bar(ui.painter(), bar, *pct as f32 / 100.0, RED, DIM);
                ui.add_space(8.0);
            }
            Update::Failed(e) => {
                rule(ui, "!!", "UPDATE FAILED");
                ui.label(RichText::new(e).color(MUTED).size(11.5));
                if btn(ui, "TRY AGAIN", Btn::Ghost, true) {
                    let (h, c) = (self.h.clone(), ctx.clone());
                    std::thread::spawn(move || update::check(&h, &c, true));
                }
                ui.add_space(8.0);
            }
            _ => {}
        }

        rule(ui, "01", "CARDS");
        let mut roots: Vec<String> = snap.cards.iter().map(|c| c.root.clone()).collect();
        for k in snap.jobs.keys() {
            if !roots.contains(k) {
                roots.push(k.clone());
            }
        }
        if roots.is_empty() {
            ui.add_space(6.0);
            dot_text(ui, "NO CARD", 4.0, MUTED);
            ui.label(RichText::new("Plug in a card or USB drive with a DCIM folder.").color(MUTED).size(12.0));
        }
        for root in roots {
            let card = snap.cards.iter().find(|c| c.root == root);
            let job = snap.jobs.get(&root);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                dot_text(ui, &drive_label(&root), 6.0, FG);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    match card {
                        Some(c) => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&c.camera.name).size(12.0));
                                ui.label(RichText::new(how_text(c)).color(MUTED).size(11.0));
                            });
                            ui.label(RichText::new(c.summary()).size(12.0));
                            ui.label(RichText::new(fmt_bytes(c.bytes)).color(MUTED).size(12.0));
                        }
                        None => {
                            ui.label(RichText::new("Card removed").color(MUTED).size(12.0));
                        }
                    }
                });
            });
            let running = job.is_some_and(|p| p.state == State::Running);
            if let (Some(c), false) = (card, job.is_some()) {
                if c.how == How::Unknown {
                    ui.label(RichText::new("Not set up yet, sorted by the default rules. Set it up once and SWORTA knows it next time.").color(RED).size(11.5));
                } else if !c.unsorted.is_empty() {
                    ui.label(RichText::new(format!("Not imported, no folder set: {}", c.unsorted_line())).color(RED).size(11.5));
                }
                ui.horizontal(|ui| {
                    if btn(ui, "IMPORT", Btn::Primary, !c.items.is_empty()) {
                        engine::begin_import(&self.h, ctx, &root);
                    }
                    let label = if c.needs_setup() { "SET UP" } else { "EDIT CAMERA" };
                    if btn(ui, label, Btn::Ghost, true) {
                        self.open_setup(c);
                    }
                });
                let mut pick = None;
                row(ui, "Camera", |ui| {
                    egui::ComboBox::from_id_salt(("camera", root.as_str()))
                        .selected_text(RichText::new(&c.camera.name).size(12.0))
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            for cam in &snap.cameras {
                                if ui.selectable_label(cam.id == c.camera.id, &cam.name).clicked() {
                                    pick = Some(cam.id.clone());
                                }
                            }
                        });
                });
                if let Some(id) = pick {
                    engine::assign(&self.h, ctx, &root, &id);
                }
            }
            if let Some(p) = job {
                self.job_block(ui, &root, p, running);
            }
            ui.add_space(4.0);
        }
    }

    fn job_block(&mut self, ui: &mut Ui, root: &str, p: &Progress, running: bool) {
        let done = p.state == State::Done;
        let t = if done {
            (if p.cancelled { "STOPPED" } else { "DONE" }).to_string()
        } else {
            format!("{}%", (p.fraction() * 100.0) as u32)
        };
        ui.add_space(4.0);
        dot_text(ui, &t, 7.0, if done { RED } else { FG });
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
        dots::bar(ui.painter(), bar, p.fraction(), RED, DIM);
        let line = if done {
            format!("{}{} copied · {} skipped · {} failed", if p.cancelled { "cancelled · " } else { "" }, p.copied, p.skipped, p.failed)
        } else {
            format!(
                "{}/{} files · {} · ETA {}",
                p.files_done,
                p.files_total,
                fmt_speed(p.speed()),
                fmt_eta(p)
            )
        };
        ui.label(RichText::new(line).color(MUTED).size(11.5));
        if running {
            ui.label(RichText::new(&p.current).color(MUTED).size(11.0));
            if btn(ui, "CANCEL", Btn::Ghost, true) {
                engine::cancel_import(&self.h, root);
            }
        }
        if done {
            ui.horizontal(|ui| {
                if btn(ui, "OPEN FOLDER", Btn::Primary, true) {
                    let dest = p.dest.clone();
                    let _ = std::process::Command::new("explorer").arg(dest).spawn();
                }
                if btn(ui, "OK", Btn::Ghost, true) {
                    self.h.lock().unwrap().jobs.remove(root);
                }
            });
        }
    }

    fn cameras(&mut self, ui: &mut Ui, ctx: &egui::Context, snap: &Snap) {
        if let Some(ed) = self.editor.as_mut() {
            match editor_ui(ui, ed, &snap.settings) {
                Some(EdAction::Save) => self.save_editor(ctx),
                Some(EdAction::Cancel) => self.editor = None,
                Some(EdAction::Export) => self.export_editor(),
                Some(EdAction::Delete) => self.delete_editor(ctx),
                None => {}
            }
            return;
        }

        rule(ui, "01", "CAMERAS");
        let mut edit = None;
        for cam in &snap.cameras {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(&cam.name).size(12.0));
                    let how = if cam.id == camera::FALLBACK {
                        "used when no other camera matches".to_string()
                    } else {
                        let mut parts = Vec::new();
                        if !cam.models.is_empty() {
                            parts.push(format!("model {}", cam.models.join(", ")));
                        }
                        if !cam.markers.is_empty() {
                            parts.push(format!("folders {}", cam.markers.join(", ")));
                        }
                        if parts.is_empty() { "only cards picked by hand".into() } else { parts.join(" · ") }
                    };
                    note(ui, how);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if btn(ui, "EDIT", Btn::Ghost, true) {
                        edit = Some(cam.clone());
                    }
                });
            });
        }
        if let Some(cam) = edit {
            let card = snap.cards.iter().find(|c| c.camera.id == cam.id);
            let found = card.map(|c| camera::survey(Path::new(&c.root), &cam)).unwrap_or_default();
            self.editor = Some(Editor::new(cam.clone(), Some(cam.id.clone()), None, found));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if btn(ui, "NEW CAMERA", Btn::Ghost, true) {
                self.editor = Some(Editor::blank());
            }
            if btn(ui, "IMPORT FILE", Btn::Ghost, true) {
                self.import_camera(ctx);
            }
        });
        if let Some(m) = &self.cam_msg {
            note(ui, m.clone());
        }
        ui.add_space(10.0);

        rule(ui, "02", "HOW CARDS ARE MATCHED");
        note(
            ui,
            "SWORTA reads the camera model from the photos and clips on the card, then looks at its folders. \
             A camera you pick for a card by hand is remembered until the card is formatted. \
             The camera never needs to be connected, the card is enough.",
        );
    }

    fn settings(&mut self, ui: &mut Ui, ctx: &egui::Context, snap: &Snap) {
        let mut s = snap.settings.clone();
        let mut changed = false;

        rule(ui, "01", "DESTINATION");
        row(ui, "Ask for folder on every import", |ui| changed |= toggle(ui, &mut s.ask_dest));
        ui.label(
            RichText::new(format!(
                "{}: {}",
                if s.ask_dest { "last used" } else { "fixed folder" },
                s.dest.display()
            ))
            .color(MUTED)
            .size(11.0),
        );
        if !s.ask_dest {
            ui.horizontal(|ui| {
                if btn(ui, "CHANGE FOLDER", Btn::Ghost, true) {
                    if let Some(p) = rfd::FileDialog::new().set_directory(&s.dest).pick_folder() {
                        s.dest = p;
                        changed = true;
                    }
                }
            });
        }
        let mut li = s.long_date as usize;
        row(ui, "Folder name", |ui| {
            if choice(ui, &mut li, &["YYMMDD", "YYYY-MM-DD"]) {
                changed = true;
            }
        });
        s.long_date = li == 1;
        ui.add_space(10.0);

        rule(ui, "02", "IMPORT");
        let mut si = SPEEDS.iter().position(|v| *v == s.speed_limit).unwrap_or(0);
        row(ui, "Speed limit", |ui| {
            if steps(ui, &mut si, SPEEDS.len()) {
                changed = true;
            }
        });
        s.speed_limit = SPEEDS[si];
        ui.label(
            RichText::new(if s.speed_limit == 0 {
                "unlimited".to_string()
            } else {
                format!("max. {} MB/s in total", s.speed_limit)
            })
            .color(MUTED)
            .size(11.0),
        );
        let mut pi = PARALLEL.iter().position(|v| *v == s.parallel).unwrap_or(1);
        row(ui, "Files at once", |ui| {
            if steps(ui, &mut pi, PARALLEL.len()) {
                changed = true;
            }
        });
        s.parallel = PARALLEL[pi];
        ui.label(
            RichText::new(format!("{} per card, several cards run in parallel", s.parallel))
                .color(MUTED)
                .size(11.0),
        );
        row(ui, "Auto import (last folder)", |ui| changed |= toggle(ui, &mut s.auto_import));
        row(ui, "Open folder when done", |ui| changed |= toggle(ui, &mut s.open_when_done));
        ui.add_space(10.0);

        rule(ui, "03", "NOTIFICATIONS");
        let mut ni = match s.notify {
            Notify::Popup => 0,
            Notify::Toast => 1,
            Notify::Off => 2,
        };
        if choice(ui, &mut ni, &["POPUP", "WINDOWS", "OFF"]) {
            changed = true;
        }
        s.notify = [Notify::Popup, Notify::Toast, Notify::Off][ni];
        let mut ti = POPUP_SECS.iter().position(|v| *v == s.popup_secs).unwrap_or(1);
        row(ui, "Popup stays", |ui| {
            if steps(ui, &mut ti, POPUP_SECS.len()) {
                changed = true;
            }
        });
        s.popup_secs = POPUP_SECS[ti];
        ui.label(
            RichText::new(if s.popup_secs == 0 {
                "until dismissed".to_string()
            } else {
                format!("{} seconds", s.popup_secs)
            })
            .color(MUTED)
            .size(11.0),
        );
        if btn(ui, "PREVIEW POPUP", Btn::Ghost, true) {
            self.h.lock().unwrap().popup_request = Some(DEMO.into());
            self.enter_popup(ctx, DEMO.into());
        }
        ui.add_space(10.0);

        rule(ui, "04", "SYSTEM");
        row(ui, "Start with Windows", |ui| changed |= toggle(ui, &mut s.autostart));
        row(ui, "Update automatically", |ui| changed |= toggle(ui, &mut s.auto_update));
        ui.add_space(10.0);

        rule(ui, "05", "VERSION");
        let status = match &snap.update {
            Update::Checking => "checking …".to_string(),
            Update::UpToDate => "up to date".to_string(),
            Update::Available(r) => format!("{} available", r.tag),
            Update::Installing(p) => format!("installing {p}%"),
            Update::Failed(e) => format!("Error: {e}"),
            Update::Idle => String::new(),
        };
        ui.label(RichText::new(format!("v{}  {}", env!("CARGO_PKG_VERSION"), status)).size(12.0));
        if btn(ui, "CHECK FOR UPDATES", Btn::Ghost, true) {
            let (h, c) = (self.h.clone(), ctx.clone());
            std::thread::spawn(move || update::check(&h, &c, true));
        }

        if changed {
            if s.autostart != snap.settings.autostart {
                config::set_autostart(s.autostart);
            }
            config::save(&s);
            self.h.lock().unwrap().settings = s;
        }
    }

    fn popup_ui(&mut self, ui: &mut Ui, ctx: &egui::Context, snap: &Snap, root: &str) {
        let demo = root == DEMO;
        let card = if demo { Some(demo_card()) } else { snap.cards.iter().find(|c| c.root == root).cloned() };
        let key = card.as_ref().map(|c| c.root.clone()).unwrap_or_else(|| root.to_string());
        let job = snap.jobs.get(&key);
        let hovered = ui.rect_contains_pointer(ui.max_rect());

        // Lifetime of the popup.
        let secs = snap.settings.popup_secs;
        let running = job.is_some_and(|p| p.state == State::Running);
        let done = job.is_some_and(|p| p.state == State::Done);
        if done && self.done_seen.is_none() {
            self.done_seen = Some(Instant::now());
        }
        let expired = if done {
            self.done_seen.is_some_and(|t| t.elapsed() > Duration::from_secs(6))
        } else if running {
            false
        } else {
            secs > 0 && self.popup_since.elapsed() > Duration::from_secs(secs as u64)
        };
        if !hovered && (expired || (card.is_none() && job.is_none())) {
            self.hide(ctx);
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(300));

        let (bar, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click_and_drag());
        if resp.drag_started() {
            ctx.send_viewport_cmd(VC::StartDrag);
        }
        let p = ui.painter().clone();
        p.circle_filled(pos2(bar.left() + 20.0, bar.center().y), 3.5, RED);
        let cancelled = job.is_some_and(|j| j.cancelled);
        let head = if done { if cancelled { "CANCELLED" } else { "DONE" } } else if running { "IMPORTING" } else { "CARD DETECTED" };
        p.text(pos2(bar.left() + 34.0, bar.center().y), Align2::LEFT_CENTER, head, FontId::monospace(11.0), MUTED);
        let x = Rect::from_center_size(pos2(bar.right() - 22.0, bar.center().y), vec2(26.0, 26.0));
        let xr = ui.interact(x, egui::Id::new("popup_close"), Sense::click());
        let col = if xr.hovered() { RED } else { MUTED };
        let s = Stroke::new(1.3, col);
        p.line_segment([x.center() - vec2(4.5, 4.5), x.center() + vec2(4.5, 4.5)], s);
        p.line_segment([x.center() - vec2(4.5, -4.5), x.center() + vec2(4.5, -4.5)], s);
        if xr.clicked() {
            if let Some(j) = job {
                if j.state == State::Done {
                    self.h.lock().unwrap().jobs.remove(&key);
                }
            }
            self.hide(ctx);
            return;
        }

        Frame::new().inner_margin(Margin::symmetric(20, 4)).show(ui, |ui| {
            ui.horizontal(|ui| {
                dot_text(ui, &drive_label(&key), 5.0, FG);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    if let Some(c) = &card {
                        let name = if c.how == How::Unknown { "Unknown camera" } else { &c.camera.name };
                        ui.label(RichText::new(name).size(12.0));
                        ui.label(RichText::new(format!("{}  ·  {}", c.summary(), fmt_bytes(c.bytes))).color(MUTED).size(11.5));
                    }
                });
            });
            ui.add_space(2.0);
            if let Some(j) = job {
                let (b, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
                dots::bar(ui.painter(), b, j.fraction(), RED, DIM);
                let line = if done {
                    format!("{} copied · {} skipped", j.copied, j.skipped)
                } else {
                    format!("{}% · {} · ETA {}", (j.fraction() * 100.0) as u32, fmt_speed(j.speed()), fmt_eta(j))
                };
                ui.label(RichText::new(line).color(MUTED).size(11.0));
                if running && btn(ui, "CANCEL", Btn::Ghost, true) {
                    engine::cancel_import(&self.h, &key);
                }
            } else {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can = card.as_ref().is_some_and(|c| !c.items.is_empty());
                    if btn(ui, "IMPORT", Btn::Primary, can) {
                        engine::begin_import(&self.h, ctx, &key);
                    }
                    let setup = !demo && card.as_ref().is_some_and(|c| c.needs_setup());
                    if btn(ui, if setup { "SET UP" } else { "OPEN" }, Btn::Ghost, true) {
                        if let (true, Some(c)) = (setup, &card) {
                            self.open_setup(c);
                        }
                        self.enter_full(ctx);
                    }
                });
            }
        });
    }
}
