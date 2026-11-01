use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Frame, Layout, Margin,
    Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, ViewportCommand as VC, WindowLevel,
    pos2, vec2,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

use crate::{
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
const POPUP: egui::Vec2 = vec2(390.0, 168.0);
const DEMO: &str = "DEMO";

const SPEEDS: [u32; 7] = [0, 10, 25, 50, 100, 200, 400];
const PARALLEL: [u32; 4] = [1, 2, 3, 4];
const POPUP_SECS: [u32; 4] = [5, 12, 30, 0];

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Home,
    Settings,
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
    _tray: TrayIcon,
}

struct Snap {
    cards: Vec<Card>,
    jobs: HashMap<String, Progress>,
    settings: Settings,
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
            update: g.update.clone(),
        }
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
        items: Arc::new(Vec::new()),
        jpg: 128,
        raw: 128,
        vid: 6,
        bytes: 14_200_000_000,
    }
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
                for (t, name) in [(Tab::Home, "HOME"), (Tab::Settings, "SETTINGS")] {
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
            Update::Installing => {
                ui.label(RichText::new("Installing update …").color(MUTED).size(12.0));
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
                            ui.label(RichText::new(format!("JPG {}   RAW {}   VID {}", c.jpg, c.raw, c.vid)).size(12.0));
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
                if btn(ui, "IMPORT", Btn::Primary, !c.items.is_empty()) {
                    engine::begin_import(&self.h, ctx, &root);
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
        let t = if done { "DONE".to_string() } else { format!("{}%", (p.fraction() * 100.0) as u32) };
        ui.add_space(4.0);
        dot_text(ui, &t, 7.0, if done { RED } else { FG });
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
        dots::bar(ui.painter(), bar, p.fraction(), RED, DIM);
        let line = if done {
            format!("{} copied · {} skipped · {} failed", p.copied, p.skipped, p.failed)
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
            Update::Installing => "installing …".to_string(),
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
        let head = if done { "DONE" } else if running { "IMPORTING" } else { "CARD DETECTED" };
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
                        ui.label(RichText::new(format!("JPG {}  RAW {}  VID {}", c.jpg, c.raw, c.vid)).size(12.0));
                        ui.label(RichText::new(fmt_bytes(c.bytes)).color(MUTED).size(11.5));
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
            } else {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can = card.as_ref().is_some_and(|c| !c.items.is_empty());
                    if btn(ui, "IMPORT", Btn::Primary, can) {
                        engine::begin_import(&self.h, ctx, &key);
                    }
                    if btn(ui, "OPEN", Btn::Ghost, true) {
                        self.enter_full(ctx);
                    }
                });
            }
        });
    }
}
