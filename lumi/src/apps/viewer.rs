//! A picture opened from Files, fitted into the window.

use super::{App, AppId, Cx};
use crate::host::Host;
use alloc::string::String;
use velora_ui::Rect;
use velora_ui::image::Picture;
use velora_ui::text::Weight;

pub struct Viewer {
    name: String,
    pic: Option<Picture>,
}

impl Viewer {
    pub fn open(host: &mut dyn Host, path: &str) -> Viewer {
        let name = String::from(path.rsplit('/').next().unwrap_or(path));
        let pic = host.read(path).and_then(|d| Picture::from_png(&d));
        Viewer { name, pic }
    }
}

impl App for Viewer {
    fn id(&self) -> AppId {
        AppId::Viewer
    }

    fn title(&self, _ru: bool) -> String {
        self.name.clone()
    }

    fn size(&self) -> ((i32, i32), (i32, i32)) {
        ((720, 480), (300, 200))
    }

    fn draw(&mut self, cx: &mut Cx, r: Rect) {
        let t = cx.theme;
        cx.c.fill_rect(r, if t.dark { 0xff101012 } else { 0xffe9e9ec });
        let pad = cx.ui.s(12);
        match self.pic.as_mut() {
            Some(p) => {
                let img = p.fit(
                    (r.w - 2 * pad).max(1) as usize,
                    (r.h - 2 * pad).max(1) as usize,
                );
                let (w, h) = (img.width as i32, img.height as i32);
                cx.c.draw_image(img, r.x + (r.w - w) / 2, r.y + (r.h - h) / 2, 255);
            }
            None => {
                let msg = cx.tr(
                    "Не удалось открыть картинку (нужен PNG).",
                    "Cannot open the picture (PNG is needed).",
                );
                let px = cx.ui.px(13);
                let w = cx.f.measure(Weight::Regular, px, msg);
                cx.f.draw(
                    cx.c,
                    Weight::Regular,
                    px,
                    r.x + (r.w - w) / 2,
                    r.center_y(),
                    msg,
                    t.text_dim,
                );
            }
        }
    }
}
