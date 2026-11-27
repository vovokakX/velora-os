//! Animations: easing curves, values that move to a new target over a
//! short time, and transitions between two pictures (a cross-fade and a
//! slide with a fade). Time is in milliseconds from the program's clock;
//! no floating point.

use crate::canvas::Canvas;
use crate::geom::Rect;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Ease-in-out (cubic) of `t` in 0..=1000, giving 0..=1000.
pub fn ease_in_out(t: u32) -> u32 {
    let t = t.min(1000) as i64;
    let v = if t < 500 {
        4 * t * t * t / 1_000_000
    } else {
        let u = 2000 - 2 * t; // 0..1000 as t goes 500..1000
        1000 - u * u * u / 2_000_000
    };
    v.clamp(0, 1000) as u32
}

/// Ease-out (cubic) of `t` in 0..=1000.
pub fn ease_out(t: u32) -> u32 {
    let u = 1000 - t.min(1000) as i64;
    (1000 - u * u * u / 1_000_000).clamp(0, 1000) as u32
}

/// Durations of the toolkit's animations, in milliseconds.
pub const FAST: u32 = 120;
pub const NORMAL: u32 = 180;
/// Theme and accent changes, screen transitions.
pub const SLOW: u32 = 260;

#[derive(Clone, Copy, Debug)]
struct Tween {
    from: i32,
    to: i32,
    start: u64,
    dur: u32,
    seen: u64,
}

impl Tween {
    fn at(&self, now: u64) -> i32 {
        let el = now.saturating_sub(self.start);
        if el >= self.dur as u64 || self.dur == 0 {
            return self.to;
        }
        let e = ease_in_out((el * 1000 / self.dur as u64) as u32) as i64;
        (self.from as i64 + (self.to - self.from) as i64 * e / 1000) as i32
    }
}

/// Animated values by key. A value seen for the first time starts at its
/// target; a new target starts a move from the current value.
pub struct Tweens {
    map: BTreeMap<u64, Tween>,
    pub now: u64,
    /// Off: values jump to their targets.
    pub enabled: bool,
    active: bool,
}

impl Default for Tweens {
    fn default() -> Self {
        Tweens {
            map: BTreeMap::new(),
            now: 0,
            enabled: true,
            active: false,
        }
    }
}

impl Tweens {
    /// Starts a frame at time `now` (ms).
    pub fn begin(&mut self, now: u64) {
        self.now = now;
        self.active = false;
        // Forget values not used for a while (widgets of other screens).
        if self.map.len() > 256 {
            self.map.retain(|_, t| now.saturating_sub(t.seen) < 5000);
        }
    }

    /// The current value of `key` moving to `target` over `dur` ms.
    pub fn value(&mut self, key: u64, target: i32, dur: u32) -> i32 {
        let now = self.now;
        if !self.enabled {
            self.map.remove(&key);
            return target;
        }
        let t = self.map.entry(key).or_insert(Tween {
            from: target,
            to: target,
            start: now,
            dur,
            seen: now,
        });
        t.seen = now;
        if t.to != target {
            t.from = t.at(now);
            t.to = target;
            t.start = now;
            t.dur = dur;
        }
        let v = t.at(now);
        if v != t.to {
            self.active = true;
        }
        v
    }

    /// Sets `key` to `v` without animation.
    pub fn set(&mut self, key: u64, v: i32) {
        let now = self.now;
        self.map.insert(
            key,
            Tween {
                from: v,
                to: v,
                start: now,
                dur: 0,
                seen: now,
            },
        );
    }

    /// Some value is still moving: draw another frame soon.
    pub fn active(&self) -> bool {
        self.active
    }

    pub fn mark_active(&mut self) {
        self.active = true;
    }
}

/// How a transition moves the old picture away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The old picture fades into the new one in place.
    Fade,
    /// The old picture slides out and fades while the new one slides in
    /// from the other side; `dir` 1 moves to the left (forward), -1 to the
    /// right (back). `dist` is the distance in pixels.
    Slide { dir: i32, dist: i32 },
}

/// A transition from the picture that was on the screen to the frames
/// being drawn now, inside `rect`.
pub struct Transition {
    old: Vec<u32>,
    new: Vec<u32>,
    pub rect: Rect,
    start: u64,
    dur: u32,
    kind: Kind,
    /// Frames shown so far: the clock starts with the second frame, so a
    /// slow first frame (a new background drawn) does not eat the
    /// transition.
    frames: u32,
}

impl Transition {
    /// Takes `rect` of `c` (the last frame shown) as the old picture.
    pub fn capture(c: &Canvas, rect: Rect, now: u64, dur: u32, kind: Kind) -> Transition {
        let rect = rect.intersect(c.bounds());
        Transition {
            old: copy_rect(c, rect),
            new: Vec::new(),
            rect,
            start: now,
            dur,
            kind,
            frames: 0,
        }
    }

    /// Progress 0..=1000 (eased) at `now`.
    pub fn progress(&self, now: u64) -> u32 {
        let el = now.saturating_sub(self.start);
        if self.dur == 0 || el >= self.dur as u64 {
            1000
        } else {
            ease_in_out((el * 1000 / self.dur as u64) as u32)
        }
    }

    pub fn done(&self, now: u64) -> bool {
        self.progress(now) >= 1000
    }

    /// Mixes the old picture into the frame just drawn in `c`. Returns
    /// false once the transition is over (nothing is changed then).
    pub fn apply(&mut self, c: &mut Canvas, now: u64) -> bool {
        if self.frames < 2 {
            self.frames += 1;
            self.start = now;
        }
        let e = self.progress(now);
        if e >= 1000 || self.rect.is_empty() {
            return false;
        }
        let r = self.rect;
        let w = r.w as usize;
        let a = e * 255 / 1000; // weight of the new picture
        match self.kind {
            Kind::Fade => {
                for y in 0..r.h as usize {
                    let row = (r.y as usize + y) * c.width + r.x as usize;
                    let px = &mut c.pixels[row..row + w];
                    let old = &self.old[y * w..y * w + w];
                    for (p, o) in px.iter_mut().zip(old) {
                        *p = mix(*o, *p, a);
                    }
                }
            }
            Kind::Slide { dir, dist } => {
                self.new = copy_rect(c, r);
                let old_off = -dir * dist * e as i32 / 1000;
                let new_off = dir * dist * (1000 - e as i32) / 1000;
                let last = r.w - 1;
                for y in 0..r.h as usize {
                    let row = (r.y as usize + y) * c.width + r.x as usize;
                    let base = y * w;
                    for x in 0..r.w {
                        let ox = (x - old_off).clamp(0, last) as usize;
                        let nx = (x - new_off).clamp(0, last) as usize;
                        c.pixels[row + x as usize] =
                            mix(self.old[base + ox], self.new[base + nx], a);
                    }
                }
            }
        }
        true
    }
}

fn copy_rect(c: &Canvas, r: Rect) -> Vec<u32> {
    let mut out = Vec::with_capacity((r.w.max(0) * r.h.max(0)) as usize);
    for y in r.y..r.bottom() {
        let row = y as usize * c.width;
        out.extend_from_slice(&c.pixels[row + r.x as usize..row + r.right() as usize]);
    }
    out
}

/// `a` mixed towards `b` by `t` / 255 (opaque pixels).
#[inline]
fn mix(a: u32, b: u32, t: u32) -> u32 {
    let inv = 255 - t;
    let ch = |s: u32| ((((a >> s) & 0xff) * inv + ((b >> s) & 0xff) * t + 127) / 255) << s;
    ch(16) | ch(8) | ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_curves() {
        assert_eq!(ease_in_out(0), 0);
        assert_eq!(ease_in_out(1000), 1000);
        assert_eq!(ease_in_out(500), 500);
        assert!(ease_in_out(100) < 100 && ease_in_out(900) > 900);
        let mut last = 0;
        for t in 0..=1000 {
            let v = ease_in_out(t);
            assert!(v >= last);
            last = v;
        }
        assert_eq!(ease_out(1000), 1000);
        assert!(ease_out(200) > 200);
    }

    #[test]
    fn tweens_move_to_new_targets() {
        let mut tw = Tweens::default();
        tw.begin(0);
        assert_eq!(tw.value(1, 0, 200), 0);
        assert!(!tw.active());
        tw.begin(10);
        assert_eq!(tw.value(1, 1000, 200), 0);
        assert!(tw.active());
        tw.begin(110);
        let mid = tw.value(1, 1000, 200);
        assert!(mid > 300 && mid < 700, "{}", mid);
        tw.begin(210);
        assert_eq!(tw.value(1, 1000, 200), 1000);
        assert!(!tw.active());
        tw.enabled = false;
        tw.begin(220);
        assert_eq!(tw.value(1, 0, 200), 0);
        assert!(!tw.active());
    }

    #[test]
    fn transitions_fade_and_slide() {
        let mut c = Canvas::new(8, 2);
        c.pixels.fill(0x000000);
        let mut t = Transition::capture(&c, c.bounds(), 0, 200, Kind::Fade);
        c.pixels.fill(0xffffff);
        // The first two frames show the old picture; the clock starts then.
        assert!(t.apply(&mut c, 0));
        assert_eq!(c.pixels[0], 0);
        c.pixels.fill(0xffffff);
        assert!(t.apply(&mut c, 0));
        c.pixels.fill(0xffffff);
        assert!(t.apply(&mut c, 100));
        let v = c.pixels[0] & 0xff;
        assert!(v > 100 && v < 160, "{}", v);
        c.pixels.fill(0xffffff);
        assert!(!t.apply(&mut c, 200));
        assert_eq!(c.pixels[0], 0xffffff);
        // A slide: at the start the old picture is in place.
        let mut s = Canvas::new(8, 1);
        for (i, p) in s.pixels.iter_mut().enumerate() {
            *p = i as u32;
        }
        let mut t = Transition::capture(&s, s.bounds(), 0, 200, Kind::Slide { dir: 1, dist: 4 });
        s.pixels.fill(0xff);
        t.apply(&mut s, 0);
        assert_eq!(s.pixels[3], 3);
    }
}
