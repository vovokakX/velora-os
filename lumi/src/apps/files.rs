//! Files: folders and files with their size and date, places on the
//! left, and making, renaming and deleting. A double click or Enter opens
//! a folder or a file (text in Plume, PNG pictures in the viewer,
//! programs in a terminal). Keys: arrows, Enter, Backspace (up), F2
//! (rename), Delete, Ctrl+N (new folder), Ctrl+H (hidden files).

use super::{Action, App, AppId, Cx, tr};
use crate::host::{DirEntry, Host, join, parent};
use crate::tz;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use velora_ui::canvas::{FX, fade, lerp};
use velora_ui::icons::{self, Icon};
use velora_ui::text::Weight;
use velora_ui::ui::{ButtonKind, Ctx, Key, mods};
use velora_ui::{Event, Rect};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Dialog {
    None,
    NewFolder(String),
    Rename(String),
    Delete,
}

pub struct Files {
    pub path: String,
    home: String,
    entries: Vec<DirEntry>,
    sel: Option<usize>,
    scroll: i32,
    back: Vec<String>,
    hidden: bool,
    dialog: Dialog,
    status: String,
    error: bool,
    last_click: (u64, usize),
    region: String,
}

/// What kind of file a name is, for its icon and how it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Text,
    Picture,
    Program,
    Other,
}

pub fn kind_of(e: &DirEntry) -> Kind {
    if e.dir {
        return Kind::Folder;
    }
    let lower = e.name.to_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, x)| x);
    match ext {
        "png" => Kind::Picture,
        "txt" | "md" | "conf" | "rs" | "toml" | "sh" | "log" | "json" | "csv" | "ini" | "c"
        | "h" | "py" => Kind::Text,
        _ if e.exec => Kind::Program,
        // Files without an extension are usually text on Velora (motd,
        // passwd, notes).
        "" => Kind::Text,
        _ => Kind::Other,
    }
}

fn size_text(b: u64, ru: bool) -> String {
    let (k, m) = (tr(ru, "КБ", "KB"), tr(ru, "МБ", "MB"));
    if b < 1024 {
        format!("{} {}", b, tr(ru, "Б", "B"))
    } else if b < 1024 * 1024 {
        format!("{} {}", b.div_ceil(1024), k)
    } else {
        format!("{}.{} {}", b >> 20, ((b >> 10) % 1024) * 10 / 1024, m)
    }
}

impl Files {
    pub fn new(host: &mut dyn Host, path: &str, home: &str, region: &str) -> Files {
        let mut f = Files {
            path: String::from(path),
            home: String::from(home),
            entries: Vec::new(),
            sel: None,
            scroll: 0,
            back: Vec::new(),
            hidden: false,
            dialog: Dialog::None,
            status: String::new(),
            error: false,
            last_click: (0, usize::MAX),
            region: String::from(region),
        };
        f.reload(host);
        f
    }

    fn reload(&mut self, host: &mut dyn Host) {
        match host.list(&self.path) {
            Ok(mut v) => {
                if !self.hidden {
                    v.retain(|e| !e.name.starts_with('.'));
                }
                v.retain(|e| e.name != "." && e.name != "..");
                v.sort_by(|a, b| {
                    b.dir
                        .cmp(&a.dir)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                self.entries = v;
                self.error = false;
                self.status.clear();
            }
            Err(e) => {
                self.entries.clear();
                self.status = e;
                self.error = true;
            }
        }
        if self.sel.is_some_and(|s| s >= self.entries.len()) {
            self.sel = None;
        }
    }

    fn go(&mut self, host: &mut dyn Host, path: String) {
        if path != self.path {
            self.back.push(core::mem::replace(&mut self.path, path));
            self.sel = None;
            self.scroll = 0;
            self.reload(host);
        }
    }

    fn open(&mut self, host: &mut dyn Host, i: usize, out: &mut Vec<Action>) {
        let Some(e) = self.entries.get(i).cloned() else {
            return;
        };
        let p = join(&self.path, &e.name);
        if e.dir {
            self.go(host, p);
        } else {
            out.push(Action::Open(p));
        }
    }

    fn selected_path(&self) -> Option<String> {
        self.sel
            .and_then(|i| self.entries.get(i))
            .map(|e| join(&self.path, &e.name))
    }

    fn finish_dialog(&mut self, host: &mut dyn Host, ru: bool) {
        let d = core::mem::replace(&mut self.dialog, Dialog::None);
        let result = match d {
            Dialog::NewFolder(name) => {
                let name = String::from(name.trim());
                if name.is_empty() || name.contains('/') {
                    Err(String::from(tr(ru, "Недопустимое имя", "Invalid name")))
                } else {
                    host.mkdir(&join(&self.path, &name)).map(|_| name)
                }
            }
            Dialog::Rename(name) => {
                let name = String::from(name.trim());
                match self.selected_path() {
                    Some(old) if !name.is_empty() && !name.contains('/') => {
                        host.rename(&old, &join(&self.path, &name)).map(|_| name)
                    }
                    _ => Err(String::from(tr(ru, "Недопустимое имя", "Invalid name"))),
                }
            }
            Dialog::Delete => match self.selected_path() {
                Some(p) => host.remove(&p).map(|_| String::new()),
                None => Ok(String::new()),
            },
            Dialog::None => return,
        };
        match result {
            Ok(name) => {
                self.reload(host);
                self.sel = self.entries.iter().position(|e| e.name == name);
                self.status.clear();
            }
            Err(e) => {
                self.reload(host);
                self.status = e;
                self.error = true;
            }
        }
    }

    fn places(&self, ru: bool) -> [(Icon, &'static str, String); 4] {
        [
            (
                Icon::Home,
                tr(ru, "Домашняя папка", "Home"),
                self.home.clone(),
            ),
            (Icon::Disk, tr(ru, "Система", "Computer"), String::from("/")),
            (
                Icon::Folder,
                tr(ru, "Временные", "Temporary"),
                String::from("/tmp"),
            ),
            (Icon::Disk, tr(ru, "Диски", "Drives"), String::from("/mnt")),
        ]
    }

    fn date(&self, t: u64, ru: bool) -> String {
        if t == 0 {
            return String::from("—");
        }
        let l = tz::local(&self.region, t as i64);
        let _ = ru;
        format!("{:02}.{:02}.{} {}", l.day, l.month, l.year, l.hhmm())
    }
}

impl App for Files {
    fn id(&self) -> AppId {
        AppId::Files
    }

    fn title(&self, ru: bool) -> String {
        if self.path == self.home {
            String::from(tr(ru, "Домашняя папка", "Home"))
        } else if self.path == "/" {
            String::from("/")
        } else {
            String::from(self.path.rsplit('/').next().unwrap_or(&self.path))
        }
    }

    fn size(&self) -> ((i32, i32), (i32, i32)) {
        ((780, 500), (480, 300))
    }

    fn draw(&mut self, cx: &mut Cx, r: Rect) {
        let t = cx.theme;
        let ru = cx.ru;
        let sc = cx.ui.scale;
        let s = move |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
        let (side_w, bar_h, status_h) = (s(180), s(48), s(30));
        let row_h = s(30);
        // Keys first: the list takes them when no dialog is open.
        let dialog_open = self.dialog != Dialog::None;
        for ev in cx.events {
            if let Event::Chord {
                key: Key::Char(ch),
                mods: m,
            } = *ev
                && m & mods::CTRL != 0
                && !dialog_open
            {
                match ch {
                    'n' => self.dialog = Dialog::NewFolder(String::new()),
                    'h' => {
                        self.hidden = !self.hidden;
                        self.reload(cx.host);
                    }
                    _ => {}
                }
            }
        }
        if !dialog_open && cx.focused && cx.ui.focused().is_none() {
            let n = self.entries.len();
            if cx.ui.take_key(Key::Down) && n > 0 {
                self.sel = Some(self.sel.map_or(0, |s| (s + 1).min(n - 1)));
            }
            if cx.ui.take_key(Key::Up) && n > 0 {
                self.sel = Some(self.sel.map_or(0, |s| s.saturating_sub(1)));
            }
            if cx.ui.take_key(Key::Home) && n > 0 {
                self.sel = Some(0);
            }
            if cx.ui.take_key(Key::End) && n > 0 {
                self.sel = Some(n - 1);
            }
            if cx.ui.take_key(Key::Enter)
                && let Some(i) = self.sel
            {
                self.open(cx.host, i, cx.out);
            }
            if cx.ui.take_key(Key::Backspace) {
                let p = parent(&self.path);
                self.go(cx.host, p);
            }
            if cx.ui.take_key(Key::F(2))
                && let Some(e) = self.sel.and_then(|i| self.entries.get(i))
            {
                self.dialog = Dialog::Rename(e.name.clone());
            }
            if cx.ui.take_key(Key::Delete) && self.sel.is_some() {
                self.dialog = Dialog::Delete;
            }
            if cx.ui.take_key(Key::F(5)) {
                self.reload(cx.host);
            }
        }
        cx.c.fill_rect(r, t.window);
        // Places.
        let side = Rect::new(r.x, r.y, side_w, r.h);
        cx.c.fill_rect(side, t.sidebar);
        let mut y = r.y + s(14);
        let places = self.places(ru);
        for (i, (icon, name, path)) in places.iter().enumerate() {
            let row = Rect::new(side.x + s(8), y, side.w - s(16), s(34));
            let id = 100 + i as u32;
            let resp = cx.ui.interact(id, row);
            let here = self.path == *path;
            let hov = cx.ui.anim(id, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            if here {
                cx.c.fill_rrect(row, s(8), lerp(t.selected, t.accent, 46));
            } else if hov > 0 {
                cx.c.fill_rrect(row, s(8), fade(t.selected, hov));
            }
            let col = if here { t.text } else { t.text_dim };
            icons::draw(
                cx.c,
                *icon,
                row.x + s(10),
                row.center_y() - s(9),
                s(18),
                col,
                0,
            );
            cx.f.draw_centered_y(
                cx.c,
                Weight::Medium,
                cx.ui.px(13),
                row.x + s(38),
                row.center_y(),
                name,
                col,
            );
            if resp.activated && !dialog_open {
                let p = path.clone();
                self.go(cx.host, p);
            }
            y += s(38);
        }
        // Toolbar.
        let main = Rect::new(side.right(), r.y, r.w - side_w, r.h);
        let bar = Rect::new(main.x, main.y, main.w, bar_h);
        let mut bx = bar.x + s(12);
        let by = bar.y + (bar_h - s(32)) / 2;
        let icon_btn = |cx: &mut Cx, id: u32, x: i32, icon: Icon, enabled: bool| -> bool {
            let r = Rect::new(x, by, s(32), s(32));
            cx.ui.set_enabled(enabled);
            let resp = cx.ui.interact(id, r);
            cx.ui.set_enabled(true);
            let hov = cx.ui.anim(id, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            if hov > 0 {
                cx.c.fill_rrect(r, s(8), fade(t.field_hover, hov));
            }
            let col = if enabled { t.text } else { t.text_faint };
            icons::draw(cx.c, icon, r.x + s(7), r.y + s(7), s(18), col, 0);
            resp.activated
        };
        let can_back = !self.back.is_empty();
        if icon_btn(cx, 200, bx, Icon::ArrowLeft, can_back && !dialog_open)
            && let Some(p) = self.back.pop()
        {
            self.path = p;
            self.sel = None;
            self.reload(cx.host);
        }
        bx += s(36);
        if icon_btn(cx, 201, bx, Icon::ArrowUp, self.path != "/" && !dialog_open) {
            let p = parent(&self.path);
            self.go(cx.host, p);
        }
        bx += s(44);
        // The path as a pill.
        let tools_w = s(36) * 3 + s(12);
        let path_r = Rect::new(
            bx,
            by,
            (bar.right() - tools_w - bx - s(8)).max(s(60)),
            s(32),
        );
        cx.c.fill_rrect(path_r, s(8), t.field);
        let old = cx.c.set_clip(path_r.inset(s(2)));
        cx.f.draw_centered_y(
            cx.c,
            Weight::Medium,
            cx.ui.px(13),
            path_r.x + s(12),
            path_r.center_y(),
            &self.path,
            t.text,
        );
        cx.c.set_clip(old);
        let mut tx = bar.right() - tools_w;
        let has_sel = self.sel.is_some();
        if icon_btn(cx, 202, tx, Icon::FolderPlus, !dialog_open) {
            self.dialog = Dialog::NewFolder(String::new());
        }
        tx += s(36);
        if icon_btn(cx, 203, tx, Icon::Pencil, has_sel && !dialog_open)
            && let Some(e) = self.sel.and_then(|i| self.entries.get(i))
        {
            self.dialog = Dialog::Rename(e.name.clone());
        }
        tx += s(36);
        if icon_btn(cx, 204, tx, Icon::Trash, has_sel && !dialog_open) {
            self.dialog = Dialog::Delete;
        }
        cx.c.fill_rect(
            Rect::new(main.x, bar.bottom() - 1, main.w, 1),
            t.window_border,
        );
        // Column titles.
        let list = Rect::new(main.x, bar.bottom(), main.w, main.h - bar_h - status_h);
        let head = Rect::new(list.x, list.y, list.w, s(28));
        let (size_x, date_x) = (list.right() - s(260), list.right() - s(170));
        let hpx = cx.ui.px(11);
        cx.f.draw_centered_y(
            cx.c,
            Weight::Medium,
            hpx,
            list.x + s(52),
            head.center_y(),
            tr(ru, "Имя", "Name"),
            t.text_faint,
        );
        cx.f.draw_centered_y(
            cx.c,
            Weight::Medium,
            hpx,
            size_x,
            head.center_y(),
            tr(ru, "Размер", "Size"),
            t.text_faint,
        );
        cx.f.draw_centered_y(
            cx.c,
            Weight::Medium,
            hpx,
            date_x,
            head.center_y(),
            tr(ru, "Изменён", "Modified"),
            t.text_faint,
        );
        let rows = Rect::new(list.x, head.bottom(), list.w, list.h - head.h);
        // Scrolling.
        let visible = (rows.h / row_h).max(1);
        let max_scroll = (self.entries.len() as i32 - visible).max(0);
        if rows.contains(cx.ui.mouse.0, cx.ui.mouse.1) && !dialog_open {
            for ev in cx.events {
                if let Event::Wheel(d) = *ev {
                    self.scroll = (self.scroll + d * 2).clamp(0, max_scroll);
                }
            }
        }
        if let Some(sel) = self.sel {
            let sel = sel as i32;
            if sel < self.scroll {
                self.scroll = sel;
            } else if sel >= self.scroll + visible {
                self.scroll = sel - visible + 1;
            }
        }
        self.scroll = self.scroll.clamp(0, max_scroll);
        let old = cx.c.set_clip(rows);
        let px = cx.ui.px(13);
        let mut clicked = None;
        for (vi, i) in (self.scroll as usize..self.entries.len()).enumerate() {
            let ry = rows.y + vi as i32 * row_h;
            if ry >= rows.bottom() {
                break;
            }
            let row = Rect::new(rows.x + s(8), ry, rows.w - s(16), row_h);
            let hovered = !dialog_open
                && row.contains(cx.ui.mouse.0, cx.ui.mouse.1)
                && rows.contains(cx.ui.mouse.0, cx.ui.mouse.1);
            cx.ui.area(1000 + i as u32, row);
            if self.sel == Some(i) {
                let fill = if cx.focused {
                    lerp(t.selected, t.accent, 70)
                } else {
                    t.selected
                };
                cx.c.fill_rrect(row, s(7), fill);
            } else if hovered {
                cx.c.fill_rrect(row, s(7), fade(t.selected, 150));
            }
            if hovered && cx.ui.take_click(row) {
                clicked = Some(i);
            }
            let e = &self.entries[i];
            let (icon, col) = match kind_of(e) {
                Kind::Folder => (Icon::Folder, t.accent),
                Kind::Text => (Icon::FileText, t.text_dim),
                Kind::Picture => (Icon::Picture, t.text_dim),
                Kind::Program => (Icon::Terminal, t.text_dim),
                Kind::Other => (Icon::File, t.text_dim),
            };
            icons::draw(
                cx.c,
                icon,
                row.x + s(12),
                row.center_y() - s(9),
                s(18),
                col,
                0,
            );
            let name_clip = Rect::new(row.x, row.y, size_x - row.x - s(12), row.h).intersect(rows);
            let o2 = cx.c.set_clip(name_clip);
            cx.f.draw_centered_y(
                cx.c,
                Weight::Regular,
                px,
                row.x + s(44),
                row.center_y(),
                &e.name,
                t.text,
            );
            cx.c.set_clip(o2.intersect(rows));
            let size = if e.dir {
                String::from("—")
            } else {
                size_text(e.size, ru)
            };
            cx.f.draw_centered_y(
                cx.c,
                Weight::Regular,
                px,
                size_x,
                row.center_y(),
                &size,
                t.text_dim,
            );
            let date = self.date(e.mtime, ru);
            cx.f.draw_centered_y(
                cx.c,
                Weight::Regular,
                px,
                date_x,
                row.center_y(),
                &date,
                t.text_dim,
            );
        }
        if self.entries.is_empty() {
            let msg = if self.error {
                tr(ru, "Папку не удалось открыть", "Cannot open the folder")
            } else {
                tr(ru, "Папка пуста", "The folder is empty")
            };
            let w = cx.f.measure(Weight::Medium, px, msg);
            cx.f.draw(
                cx.c,
                Weight::Medium,
                px,
                rows.x + (rows.w - w) / 2,
                rows.y + s(60),
                msg,
                t.text_faint,
            );
        }
        cx.c.set_clip(old);
        if let Some(i) = clicked {
            let double = self.last_click.1 == i && cx.now.saturating_sub(self.last_click.0) < 450;
            self.sel = Some(i);
            cx.ui.set_focus(None);
            self.last_click = (cx.now, i);
            if double {
                self.last_click = (0, usize::MAX);
                self.open(cx.host, i, cx.out);
            }
        }
        // Scroll bar.
        if max_scroll > 0 {
            let total = self.entries.len() as i32;
            let h = (rows.h * visible / total).max(s(24));
            let y = rows.y + (rows.h - h) * self.scroll / max_scroll;
            cx.c.fill_rrect(
                Rect::new(rows.right() - s(6), y, s(4), h),
                s(2),
                t.scrollbar,
            );
        }
        // Status line.
        let st = Rect::new(main.x, r.bottom() - status_h, main.w, status_h);
        cx.c.fill_rect(Rect::new(st.x, st.y, st.w, 1), t.window_border);
        let text = if !self.status.is_empty() {
            self.status.clone()
        } else if ru {
            format!("Объектов: {}", self.entries.len())
        } else {
            format!("{} items", self.entries.len())
        };
        let col = if self.error { t.error } else { t.text_faint };
        cx.f.draw_centered_y(
            cx.c,
            Weight::Regular,
            cx.ui.px(11),
            st.x + s(16),
            st.center_y(),
            &text,
            col,
        );
        if dialog_open {
            self.draw_dialog(cx, r);
        }
    }
}

impl Files {
    fn draw_dialog(&mut self, cx: &mut Cx, r: Rect) {
        let t = cx.theme;
        let ru = cx.ru;
        let sc = cx.ui.scale;
        let s = move |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
        cx.c.fill_rect(r, fade(t.shadow, 90));
        let card = Rect::new(r.center_x() - s(190), r.center_y() - s(90), s(380), s(170));
        cx.c.shadow(card.offset(0, s(8)), s(16), s(24), t.shadow);
        cx.c.fill_rrect(card, s(16), t.window);
        cx.c.stroke_rrect_fx(card, s(16), FX, t.window_border);
        let (title, ok) = match &self.dialog {
            Dialog::NewFolder(_) => (
                tr(ru, "Новая папка", "New folder"),
                tr(ru, "Создать", "Create"),
            ),
            Dialog::Rename(_) => (
                tr(ru, "Переименовать", "Rename"),
                tr(ru, "Переименовать", "Rename"),
            ),
            Dialog::Delete => (tr(ru, "Удалить?", "Delete?"), tr(ru, "Удалить", "Delete")),
            Dialog::None => return,
        };
        cx.f.draw(
            cx.c,
            Weight::SemiBold,
            cx.ui.px(17),
            card.x + s(24),
            card.y + s(40),
            title,
            t.text,
        );
        let field = Rect::new(card.x + s(24), card.y + s(60), card.w - s(48), s(40));
        let mut submit = false;
        let mut ctx = Ctx {
            ui: cx.ui,
            c: cx.c,
            f: cx.f,
        };
        match &mut self.dialog {
            Dialog::NewFolder(name) | Dialog::Rename(name) => {
                if ctx.ui.focused().is_none() {
                    ctx.ui.set_focus(Some(300));
                }
                let ph = tr(ru, "Имя", "Name");
                ctx.text_field(300, field, name, ph, false, 120);
                if ctx.ui.focused() == Some(300) && ctx.ui.take_key(Key::Enter) {
                    submit = true;
                }
            }
            Dialog::Delete => {
                let name = self
                    .sel
                    .and_then(|i| self.entries.get(i))
                    .map_or(String::new(), |e| e.name.clone());
                let msg = if ru {
                    format!("«{}» будет удалён без возврата.", name)
                } else {
                    format!("\"{}\" will be deleted for good.", name)
                };
                let px = ctx.ui.px(13);
                ctx.f.draw_centered_y(
                    ctx.c,
                    Weight::Regular,
                    px,
                    field.x,
                    field.center_y(),
                    &msg,
                    t.text_dim,
                );
                if ctx.ui.take_key(Key::Enter) {
                    submit = true;
                }
            }
            Dialog::None => {}
        }
        let bw = s(120);
        let by = card.bottom() - s(56);
        let cancel = Rect::new(card.right() - s(24) - bw * 2 - s(10), by, bw, s(36));
        let okr = Rect::new(card.right() - s(24) - bw, by, bw, s(36));
        let cancel_label = tr(ru, "Отмена", "Cancel");
        let cancelled = ctx.button(301, cancel, cancel_label, ButtonKind::Secondary, None, None)
            || ctx.ui.take_key(Key::Escape);
        if ctx.button(302, okr, ok, ButtonKind::Primary, None, None) {
            submit = true;
        }
        if cancelled {
            self.dialog = Dialog::None;
            cx.ui.set_focus(None);
        } else if submit {
            self.finish_dialog(cx.host, ru);
            cx.ui.set_focus(None);
        }
    }
}
