//! Mouse and tablet reports as the kernel queues them for programs.

/// One mouse movement or button change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseEvent {
    pub buttons: u8,
    pub dx: i8,
    pub dy: i8,
    pub wheel: i8,
    /// A tablet's position, 0..=32767 on each axis (dx and dy are 0).
    pub abs: Option<(u16, u16)>,
}

impl MouseEvent {
    /// Adds `next` to this event when the two say no more than one event
    /// can: the same buttons, no wheel, and movements whose sum fits (or
    /// two tablet positions, of which the later one counts). A queue that
    /// merges what it can keeps every movement while a program is busy.
    pub fn merge(&mut self, next: &MouseEvent) -> bool {
        if self.buttons != next.buttons || self.wheel != 0 || next.wheel != 0 {
            return false;
        }
        match (self.abs, next.abs) {
            (Some(_), Some(p)) => {
                self.abs = Some(p);
                true
            }
            (None, None) => {
                let x = self.dx as i16 + next.dx as i16;
                let y = self.dy as i16 + next.dy as i16;
                let fits = |v: i16| (i8::MIN as i16..=i8::MAX as i16).contains(&v);
                if fits(x) && fits(y) {
                    self.dx = x as i8;
                    self.dy = y as i8;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(buttons: u8, dx: i8, dy: i8) -> MouseEvent {
        MouseEvent {
            buttons,
            dx,
            dy,
            ..MouseEvent::default()
        }
    }

    #[test]
    fn movements_add_up() {
        let mut a = rel(0, 3, -4);
        assert!(a.merge(&rel(0, 5, 2)));
        assert_eq!(a, rel(0, 8, -2));
    }

    #[test]
    fn sums_that_do_not_fit_stay_apart() {
        let mut a = rel(0, 100, 0);
        assert!(!a.merge(&rel(0, 100, 0)));
        assert_eq!(a, rel(0, 100, 0));
        let mut b = rel(0, 0, -100);
        assert!(!b.merge(&rel(0, 0, -29)));
        assert!(b.merge(&rel(0, 0, -28)));
        assert_eq!(b.dy, -128);
    }

    #[test]
    fn buttons_and_wheel_are_kept() {
        let mut a = rel(0, 1, 1);
        assert!(!a.merge(&rel(1, 1, 1)), "a press is an event of its own");
        let wheel = MouseEvent {
            wheel: 1,
            ..MouseEvent::default()
        };
        assert!(!a.merge(&wheel));
        let mut w = wheel;
        assert!(!w.merge(&rel(0, 1, 1)));
    }

    #[test]
    fn tablet_positions_keep_the_last() {
        let at = |x, y| MouseEvent {
            abs: Some((x, y)),
            ..MouseEvent::default()
        };
        let mut a = at(10, 20);
        assert!(a.merge(&at(30, 40)));
        assert_eq!(a, at(30, 40));
        assert!(!a.merge(&rel(0, 1, 1)), "relative and absolute differ");
    }
}
