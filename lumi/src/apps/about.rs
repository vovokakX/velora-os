//! About: the logo, the version, the processor, memory and the screen.

use super::{App, AppId, Cx};
use crate::host::{Host, SysInfo};
use alloc::format;
use alloc::string::String;
use velora_ui::Rect;
use velora_ui::decor;
use velora_ui::text::Weight;

pub struct About {
    info: SysInfo,
    screen: (i32, i32),
    next: u64,
}

impl About {
    pub fn new(host: &mut dyn Host, screen: (i32, i32)) -> About {
        About {
            info: host.sysinfo(),
            screen,
            next: host.now_ms() + 2000,
        }
    }
}

fn mib(b: u64) -> String {
    if b >= 10 << 30 {
        format!("{} GiB", b >> 30)
    } else if b >= 1 << 30 {
        format!("{}.{} GiB", b >> 30, ((b >> 20) % 1024) * 10 / 1024)
    } else {
        format!("{} MiB", (b + (1 << 19)) >> 20)
    }
}

impl App for About {
    fn id(&self) -> AppId {
        AppId::About
    }

    fn title(&self, ru: bool) -> String {
        String::from(AppId::About.name(ru))
    }

    fn size(&self) -> ((i32, i32), (i32, i32)) {
        ((540, 320), (480, 290))
    }

    fn poll(&mut self, host: &mut dyn Host, now: u64) -> bool {
        if now < self.next {
            return false;
        }
        self.next = now + 2000;
        let info = host.sysinfo();
        let changed =
            info.mem_used != self.info.mem_used || info.uptime_s / 60 != self.info.uptime_s / 60;
        self.info = info;
        changed
    }

    fn wake_at(&self, _now: u64) -> Option<u64> {
        Some(self.next)
    }

    fn draw(&mut self, cx: &mut Cx, r: Rect) {
        let t = cx.theme;
        let sc = cx.ui.scale;
        let s = move |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
        let p13 = cx.ui.px(13);
        cx.c.fill_rect(r, t.window);
        decor::waves(cx.c, r, 0, &t);
        // The logo on the left.
        let logo = s(124);
        let lx = r.x + s(44);
        let ly = r.y + (r.h - logo) / 2 - s(10);
        let img = cx.assets.logo.fit(logo as usize, logo as usize);
        let (iw, ih) = (img.width as i32, img.height as i32);
        cx.c.draw_image(img, lx + (logo - iw) / 2, ly + (logo - ih) / 2, 255);
        let name_y = ly + logo + s(28);
        let shell = "Lumi";
        let w = cx.f.measure(Weight::Medium, p13, shell);
        cx.f.draw(
            cx.c,
            Weight::Medium,
            p13,
            lx + (logo - w) / 2,
            name_y,
            shell,
            t.text_dim,
        );
        // The facts on the right.
        let x = lx + logo + s(48);
        let mut y = r.y + s(76);
        let version = String::from(self.info.version.trim());
        cx.f.draw(
            cx.c,
            Weight::SemiBold,
            cx.ui.px(28),
            x,
            y,
            "Velora OS",
            t.text,
        );
        y += s(26);
        let ver = format!("{} {}", cx.tr("Версия", "Version"), version);
        cx.f.draw(cx.c, Weight::Regular, p13, x, y, &ver, t.text_dim);
        y += s(36);
        let label_w = s(104);
        let row = |cx: &mut Cx, y: i32, label: &str, value: &str| {
            cx.f.draw(cx.c, Weight::Regular, p13, x, y, label, t.text_dim);
            cx.f.draw(cx.c, Weight::Medium, p13, x + label_w, y, value, t.text);
        };
        let cores = if cx.ru {
            format!("{} ({} яд.)", self.info.cpu.trim(), self.info.cpus)
        } else {
            format!("{} ({} cores)", self.info.cpu.trim(), self.info.cpus)
        };
        let cpu_label = cx.tr("Процессор", "Processor");
        row(cx, y, cpu_label, &cores);
        y += s(25);
        let mem = format!("{} / {}", mib(self.info.mem_used), mib(self.info.mem_total));
        let mem_label = cx.tr("Память", "Memory");
        row(cx, y, mem_label, &mem);
        // A thin bar of memory in use.
        let bar = Rect::new(
            x + label_w,
            y + s(9),
            (r.right() - s(44) - x - label_w).max(s(60)),
            s(4),
        );
        cx.c.fill_rrect(bar, bar.h / 2, t.progress_track);
        let used = if self.info.mem_total > 0 {
            (bar.w as u64 * self.info.mem_used / self.info.mem_total) as i32
        } else {
            0
        };
        cx.c.fill_rrect(
            Rect::new(bar.x, bar.y, used.max(bar.h), bar.h),
            bar.h / 2,
            t.accent,
        );
        y += s(32);
        let screen = format!("{} x {}", self.screen.0, self.screen.1);
        let screen_label = cx.tr("Экран", "Display");
        row(cx, y, screen_label, &screen);
        y += s(25);
        let up = self.info.uptime_s;
        let uptime = if cx.ru {
            format!("{} ч {} мин", up / 3600, up / 60 % 60)
        } else {
            format!("{} h {} min", up / 3600, up / 60 % 60)
        };
        let up_label = cx.tr("Работает", "Uptime");
        row(cx, y, up_label, &uptime);
        y += s(25);
        let procs = format!("{}", self.info.processes);
        let proc_label = cx.tr("Процессы", "Processes");
        row(cx, y, proc_label, &procs);
    }
}
