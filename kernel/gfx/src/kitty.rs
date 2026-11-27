//! The Kitty terminal graphics protocol
//! (<https://sw.kovidgoyal.net/kitty/graphics-protocol/>).
//!
//! A command is an APC string: `ESC _ G <keys> ; <base64 payload> ESC \`,
//! where keys are `k=v` pairs separated by commas. Large payloads are
//! sent in chunks: every chunk but the last has `m=1`, and only the first
//! carries the other keys. The terminal answers commands that have an
//! image id `i` with `ESC _ G i=<id> ; OK ESC \` or an error text.
//!
//! Supported here: actions `t` (store), `T` (store and show), `p` (show a
//! stored image), `q` (check without storing), `d` (forget images);
//! formats 24 (RGB) and 32 (RGBA); direct transmission (`t=d`); size in
//! cells `c`/`r`; cursor policy `C`; quiet modes `q`; chunking `m`.
//! NOT IMPLEMENTED: PNG payloads (`f=100`), zlib compression (`o=z`),
//! files and shared memory (`t=f/t/s`), source rectangles (`x,y,w,h`),
//! pixel offsets (`X,Y`), z-index, image numbers (`I`), animation, and
//! deleting images already on the screen (they are drawn into the text
//! screen and go away as text is written over them or scrolls).

use crate::base64::{self, Decoder};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// One parsed command (control keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    /// `a`: t, T, p, q, d.
    pub action: u8,
    /// `f`: 24, 32 or 100.
    pub format: u32,
    /// `t`: d (direct), f, t, s.
    pub medium: u8,
    /// `o`: 0 or z.
    pub compression: u8,
    /// `s`, `v`: image size in pixels.
    pub width: u32,
    pub height: u32,
    /// `c`, `r`: area on the screen in cells (0 = from the image size).
    pub cols: u32,
    pub rows: u32,
    /// `i`: image id (0 = none; no answer is sent then).
    pub id: u32,
    /// `m`: more chunks follow.
    pub more: bool,
    /// `q`: 1 = no OK answers, 2 = no answers at all.
    pub quiet: u32,
    /// `C`: 1 = do not move the cursor after showing the image.
    pub keep_cursor: bool,
    /// `d`: what to delete (a = all).
    pub delete: u8,
}

impl Default for Command {
    fn default() -> Self {
        Self {
            action: b't',
            format: 32,
            medium: b'd',
            compression: 0,
            width: 0,
            height: 0,
            cols: 0,
            rows: 0,
            id: 0,
            more: false,
            quiet: 0,
            keep_cursor: false,
            delete: b'a',
        }
    }
}

/// Parses the keys of a command (`a=T,f=32,s=10,v=10`). Unknown keys are
/// ignored, as the protocol asks.
pub fn parse(keys: &str) -> Result<Command, &'static str> {
    let mut c = Command::default();
    for pair in keys.split(',').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').ok_or("EINVAL:malformed key")?;
        let num = || v.parse::<u32>().map_err(|_| "EINVAL:malformed number");
        let ch = || match v.as_bytes() {
            [b] => Ok(*b),
            _ => Err("EINVAL:malformed value"),
        };
        match k {
            "a" => c.action = ch()?,
            "f" => c.format = num()?,
            "t" => c.medium = ch()?,
            "o" => c.compression = ch()?,
            "s" => c.width = num()?,
            "v" => c.height = num()?,
            "c" => c.cols = num()?,
            "r" => c.rows = num()?,
            "i" => c.id = num()?,
            "m" => c.more = num()? == 1,
            "q" => c.quiet = num()?,
            "C" => c.keep_cursor = num()? == 1,
            "d" => c.delete = ch()?,
            _ => {}
        }
    }
    Ok(c)
}

/// A finished command from [`Parser`].
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Command {
        cmd: Command,
        data: Vec<u8>,
    },
    /// The command could not be received; answer `msg` if allowed.
    Error {
        id: u32,
        quiet: u32,
        msg: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Before the first byte (which must be 'G').
    Start,
    Keys,
    Payload,
    /// Not a graphics command: skip to the end.
    Ignore,
}

/// Receives APC strings one byte at a time, joins chunks and decodes the
/// payload as it arrives, so a large image is never held as base64 text.
pub struct Parser {
    phase: Phase,
    keys: String,
    /// The command being received (from its first chunk).
    cmd: Option<Command>,
    data: Vec<u8>,
    dec: Decoder,
    error: Option<&'static str>,
    /// The keys were too long or not ASCII.
    bad_keys: bool,
    /// Largest payload accepted, in decoded bytes.
    limit: usize,
}

const MAX_KEYS: usize = 512;

impl Parser {
    pub const fn new(limit: usize) -> Self {
        Self {
            phase: Phase::Start,
            keys: String::new(),
            cmd: None,
            data: Vec::new(),
            dec: Decoder::new(),
            error: None,
            bad_keys: false,
            limit,
        }
    }

    /// Called after `ESC _`.
    pub fn begin(&mut self) {
        self.phase = Phase::Start;
        self.keys.clear();
        self.bad_keys = false;
    }

    /// One byte of the APC string (non-ASCII characters are passed as
    /// any byte above 0x7f).
    pub fn push(&mut self, b: u8) {
        match self.phase {
            Phase::Start => {
                self.phase = if b == b'G' {
                    Phase::Keys
                } else {
                    Phase::Ignore
                };
            }
            Phase::Keys => {
                if b == b';' {
                    self.keys_done();
                    self.phase = Phase::Payload;
                } else if self.keys.len() < MAX_KEYS && b.is_ascii() {
                    self.keys.push(b as char);
                } else {
                    self.bad_keys = true;
                }
            }
            Phase::Payload => {
                if self.error.is_some() {
                    return;
                }
                if !self.dec.push(b, &mut self.data) {
                    self.fail("EINVAL:bad base64 data");
                } else if self.data.len() > self.limit {
                    self.fail("EFBIG:image too large");
                }
            }
            Phase::Ignore => {}
        }
    }

    fn fail(&mut self, msg: &'static str) {
        self.error.get_or_insert(msg);
        self.data = Vec::new();
    }

    fn keys_done(&mut self) {
        let keys = core::mem::take(&mut self.keys);
        let parsed = if self.bad_keys {
            Err("EINVAL:malformed keys")
        } else {
            parse(&keys)
        };
        self.keys = keys;
        match (self.cmd.as_mut(), parsed) {
            // A later chunk: only `m` (and `q`) matter.
            (Some(first), Ok(c)) => {
                first.more = c.more;
                if c.quiet != 0 {
                    first.quiet = c.quiet;
                }
            }
            (Some(first), Err(e)) => {
                first.more = false;
                self.error.get_or_insert(e);
            }
            (None, Ok(c)) => {
                self.cmd = Some(c);
                self.data.clear();
                self.dec = Decoder::new();
                self.error = None;
            }
            (None, Err(e)) => {
                self.cmd = Some(Command::default());
                self.data.clear();
                self.error = Some(e);
            }
        }
    }

    /// Called at `ESC \`. Returns the command once its last chunk is in.
    pub fn end(&mut self) -> Option<Event> {
        match self.phase {
            Phase::Start | Phase::Ignore => return None,
            Phase::Keys => self.keys_done(),
            Phase::Payload => {}
        }
        self.phase = Phase::Start;
        let cmd = self.cmd?;
        if cmd.more {
            return None;
        }
        self.cmd = None;
        let data = core::mem::take(&mut self.data);
        self.dec = Decoder::new();
        match self.error.take() {
            Some(msg) => Some(Event::Error {
                id: cmd.id,
                quiet: cmd.quiet,
                msg,
            }),
            None => Some(Event::Command { cmd, data }),
        }
    }

    /// An APC string cut short (`ESC` not followed by `\`): drop it.
    pub fn abort(&mut self) {
        self.phase = Phase::Start;
        self.cmd = None;
        self.data = Vec::new();
        self.error = None;
    }
}

/// The answer to a command, or `None` if none should be sent (no id, or
/// silenced by `q`).
pub fn answer(id: u32, quiet: u32, result: Result<(), &str>) -> Option<String> {
    if id == 0 {
        return None;
    }
    match result {
        Ok(()) if quiet == 0 => Some(format!("\x1b_Gi={};OK\x1b\\", id)),
        Err(e) if quiet < 2 => Some(format!("\x1b_Gi={};{}\x1b\\", id, e)),
        _ => None,
    }
}

/// Pixel size of a transmitted image's data in bytes per pixel.
pub fn bytes_per_pixel(format: u32) -> Result<usize, &'static str> {
    match format {
        24 => Ok(3),
        32 => Ok(4),
        100 => Err("EINVAL:PNG data (f=100) is not supported, send f=24 or f=32"),
        _ => Err("EINVAL:unknown format"),
    }
}

/// Checks the transfer keys and turns the payload into an image.
pub fn image(cmd: &Command, data: &[u8]) -> Result<crate::image::Image, &'static str> {
    if cmd.medium != b'd' {
        return Err("EINVAL:only direct transmission (t=d) is supported");
    }
    if cmd.compression != 0 {
        return Err("EINVAL:compression is not supported");
    }
    let bpp = bytes_per_pixel(cmd.format)?;
    let (w, h) = (cmd.width as usize, cmd.height as usize);
    if w == 0 || h == 0 {
        return Err("EINVAL:missing image size (s, v)");
    }
    if w.checked_mul(h).and_then(|n| n.checked_mul(bpp)) != Some(data.len()) {
        return Err("ENODATA:payload size does not match s, v and f");
    }
    let img = if bpp == 3 {
        crate::image::Image::from_rgb(w, h, data)
    } else {
        crate::image::Image::from_rgba(w, h, data)
    };
    img.ok_or("EINVAL:bad image data")
}

/// Where an image goes: (cols, rows) of cells and (width, height) in
/// pixels. With both `c` and `r` the image is stretched to fill them;
/// with one of them the other follows the aspect ratio; with neither the
/// image keeps its pixel size.
pub fn placement(
    cmd: &Command,
    img_w: usize,
    img_h: usize,
    cell_w: usize,
    cell_h: usize,
) -> (usize, usize, usize, usize) {
    let (c, r) = (cmd.cols as usize, cmd.rows as usize);
    let (w, h) = match (c, r) {
        (0, 0) => (img_w, img_h),
        (c, 0) => {
            let w = c * cell_w;
            (w, (img_h * w).div_ceil(img_w.max(1)).max(1))
        }
        (0, r) => {
            let h = r * cell_h;
            ((img_w * h).div_ceil(img_h.max(1)).max(1), h)
        }
        (c, r) => (c * cell_w, r * cell_h),
    };
    (w.div_ceil(cell_w), h.div_ceil(cell_h), w, h)
}

/// The query programs send to find out whether the terminal shows
/// images: a 1x1 RGB image checked with `a=q` (not stored), then a
/// Primary Device Attributes request. Every terminal answers the latter,
/// so a program reads until that answer and then knows if the graphics
/// answer came before it.
pub const QUERY: &str = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";
/// The image id used by [`QUERY`].
pub const QUERY_ID: u32 = 31;

/// What the answers to [`QUERY`] say so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// The device attributes answer has not arrived yet.
    Waiting,
    Yes,
    No,
}

/// Reads the terminal's answers to [`QUERY`].
pub fn support(answers: &[u8]) -> Support {
    let ok = format!("\x1b_Gi={};OK\x1b\\", QUERY_ID);
    let has_ok = answers.windows(ok.len()).any(|w| w == ok.as_bytes());
    // Device attributes: ESC [ ? ... c
    let mut da = false;
    for (i, w) in answers.windows(3).enumerate() {
        if w == b"\x1b[?" {
            let rest = &answers[i + 3..];
            if let Some(end) = rest
                .iter()
                .position(|b| !(b.is_ascii_digit() || *b == b';'))
            {
                da |= rest[end] == b'c';
            }
        }
    }
    match (da, has_ok) {
        (false, _) => Support::Waiting,
        (true, true) => Support::Yes,
        (true, false) => Support::No,
    }
}

/// The escape sequences that send `data` with the given keys (without
/// `m`), split into chunks of at most 4096 base64 characters.
pub fn transmit(keys: &str, data: &[u8]) -> String {
    const CHUNK: usize = 4096;
    let b64 = base64::encode(data);
    let mut out = String::with_capacity(b64.len() + b64.len() / CHUNK * 16 + keys.len() + 16);
    let parts: Vec<&str> = b64
        .as_bytes()
        .chunks(CHUNK)
        // Base64 is ASCII, so every split is a character boundary.
        .map(|c| core::str::from_utf8(c).unwrap_or(""))
        .collect();
    if parts.len() <= 1 {
        out.push_str("\x1b_G");
        out.push_str(keys);
        out.push(';');
        out.push_str(&b64);
        out.push_str("\x1b\\");
        return out;
    }
    for (i, p) in parts.iter().enumerate() {
        out.push_str("\x1b_G");
        if i == 0 {
            out.push_str(keys);
            out.push(',');
        }
        out.push_str(if i + 1 == parts.len() { "m=0;" } else { "m=1;" });
        out.push_str(p);
        out.push_str("\x1b\\");
    }
    out
}

/// Removes APC strings (`ESC _ ... ESC \`) from a stream of text, for
/// outputs that cannot show images (the serial port, the kernel log).
/// Keeps its state between calls, since a program may split a command
/// over many writes.
pub struct ApcFilter {
    state: FilterState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterState {
    Text,
    /// `ESC` seen at the end of the previous piece (not yet passed on).
    Esc,
    Apc,
    ApcEsc,
}

impl Default for ApcFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl ApcFilter {
    pub const fn new() -> Self {
        Self {
            state: FilterState::Text,
        }
    }

    /// Passes the parts of `s` outside APC strings to `out`.
    pub fn filter(&mut self, s: &str, mut out: impl FnMut(&str)) {
        let b = s.as_bytes();
        let mut start = 0;
        let mut i = 0;
        while i < b.len() {
            match self.state {
                FilterState::Text => {
                    if b[i] == 0x1b {
                        if start < i {
                            out(&s[start..i]);
                        }
                        self.state = FilterState::Esc;
                        start = i + 1;
                    }
                    i += 1;
                }
                FilterState::Esc => {
                    if b[i] == b'_' {
                        self.state = FilterState::Apc;
                        i += 1;
                    } else {
                        out("\x1b");
                        self.state = FilterState::Text;
                    }
                    start = i;
                }
                FilterState::Apc => {
                    if b[i] == 0x1b {
                        self.state = FilterState::ApcEsc;
                    }
                    i += 1;
                    start = i;
                }
                FilterState::ApcEsc => {
                    if b[i] == b'\\' {
                        self.state = FilterState::Text;
                        i += 1;
                    } else if b[i] != 0x1b {
                        // Cut short, as the terminal treats it: the ESC
                        // starts a new sequence.
                        self.state = FilterState::Esc;
                        continue;
                    } else {
                        i += 1;
                    }
                    start = i;
                }
            }
        }
        if self.state == FilterState::Text && start < b.len() {
            out(&s[start..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn feed(p: &mut Parser, s: &str) -> Option<Event> {
        let body = s
            .strip_prefix("\x1b_")
            .and_then(|r| r.strip_suffix("\x1b\\"))
            .unwrap();
        p.begin();
        for b in body.bytes() {
            p.push(b);
        }
        p.end()
    }

    #[test]
    fn parses_keys() {
        let c = parse("a=T,f=24,s=10,v=20,c=5,r=3,i=7,q=2,C=1,X=4").unwrap();
        assert_eq!(c.action, b'T');
        assert_eq!(
            (c.format, c.width, c.height, c.cols, c.rows),
            (24, 10, 20, 5, 3)
        );
        assert_eq!((c.id, c.quiet, c.keep_cursor, c.more), (7, 2, true, false));
        assert_eq!(parse("").unwrap(), Command::default());
        assert!(parse("s=x").is_err());
        assert!(parse("a").is_err());
    }

    #[test]
    fn single_and_chunked_transfers() {
        let data: Vec<u8> = (0..30_000u32).map(|i| (i * 7) as u8).collect();
        for keys in ["a=T,f=24,s=100,v=100,i=3", "a=t,f=32,s=75,v=100"] {
            let text = transmit(keys, &data);
            assert!(text.len() > 4096 * 9, "should be chunked");
            let mut p = Parser::new(1 << 20);
            let mut events = Vec::new();
            for cmd in text.split_inclusive("\x1b\\") {
                if let Some(e) = feed(&mut p, cmd) {
                    events.push(e);
                }
            }
            assert_eq!(events.len(), 1);
            let Event::Command { cmd, data: got } = &events[0] else {
                panic!("error event");
            };
            assert_eq!(
                *cmd,
                Command {
                    more: false,
                    ..parse(keys).unwrap()
                }
            );
            assert_eq!(got, &data);
        }
        let small = transmit("a=q,i=1", &[1, 2, 3]);
        assert_eq!(small, "\x1b_Ga=q,i=1;AQID\x1b\\");
    }

    #[test]
    fn errors_and_limits() {
        let mut p = Parser::new(8);
        let e = feed(&mut p, &transmit("i=4,f=24,s=2,v=2", &[0; 12])).unwrap();
        assert_eq!(
            e,
            Event::Error {
                id: 4,
                quiet: 0,
                msg: "EFBIG:image too large"
            }
        );
        let e = feed(&mut p, "\x1b_Gi=5;AA!A\x1b\\").unwrap();
        assert!(matches!(
            e,
            Event::Error {
                id: 5,
                msg: "EINVAL:bad base64 data",
                ..
            }
        ));
        // The parser is usable again afterwards.
        let e = feed(&mut p, "\x1b_Ga=q,i=6;AAAA\x1b\\").unwrap();
        assert!(matches!(e, Event::Command { .. }));
        // Not a graphics command.
        assert_eq!(feed(&mut p, "\x1b_Xsomething\x1b\\"), None);
        // An aborted chunked transfer leaves nothing behind.
        assert_eq!(feed(&mut p, "\x1b_Gi=7,m=1;AAAA\x1b\\"), None);
        p.abort();
        let e = feed(&mut p, "\x1b_Ga=q,i=8;AAAA\x1b\\").unwrap();
        assert!(matches!(
            e,
            Event::Command {
                cmd: Command { id: 8, .. },
                ..
            }
        ));
    }

    #[test]
    fn answers() {
        assert_eq!(answer(3, 0, Ok(())).unwrap(), "\x1b_Gi=3;OK\x1b\\");
        assert_eq!(answer(3, 1, Ok(())), None);
        assert_eq!(
            answer(3, 1, Err("ENOENT:x")).unwrap(),
            "\x1b_Gi=3;ENOENT:x\x1b\\"
        );
        assert_eq!(answer(3, 2, Err("ENOENT:x")), None);
        assert_eq!(answer(0, 0, Ok(())), None);
    }

    #[test]
    fn image_checks() {
        let mut c = parse("f=24,s=2,v=1").unwrap();
        let img = image(&c, &[1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(img.pixel(1, 0), [4, 5, 6, 255]);
        assert!(image(&c, &[1, 2, 3]).unwrap_err().starts_with("ENODATA"));
        c.format = 100;
        assert!(image(&c, &[]).is_err());
        let c = parse("f=32,s=1,v=1,t=f").unwrap();
        assert!(image(&c, &[0; 4]).is_err());
        let c = parse("f=32,s=1,v=1,o=z").unwrap();
        assert!(image(&c, &[0; 4]).is_err());
    }

    #[test]
    fn placements() {
        let mut c = Command::default();
        // Native size: 20x20 pixels on 8x16 cells.
        assert_eq!(placement(&c, 20, 20, 8, 16), (3, 2, 20, 20));
        c.cols = 4;
        c.rows = 2;
        assert_eq!(placement(&c, 1, 1, 8, 16), (4, 2, 32, 32));
        c.rows = 0;
        assert_eq!(placement(&c, 100, 50, 8, 16), (4, 1, 32, 16));
        c.cols = 0;
        c.rows = 10;
        assert_eq!(placement(&c, 100, 100, 8, 16), (20, 10, 160, 160));
    }

    #[test]
    fn query_answers() {
        assert_eq!(support(b""), Support::Waiting);
        assert_eq!(support(b"\x1b_Gi=31;OK\x1b\\"), Support::Waiting);
        assert_eq!(support(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22c"), Support::Yes);
        assert_eq!(support(b"\x1b[?62;22c"), Support::No);
        assert_eq!(support(b"\x1b_Gi=31;EINVAL:x\x1b\\\x1b[?1;2c"), Support::No);
    }

    #[test]
    fn apc_filter() {
        fn run(pieces: &[&str]) -> String {
            let mut f = ApcFilter::new();
            let mut out = String::new();
            for p in pieces {
                f.filter(p, |s| out.push_str(s));
            }
            out
        }
        assert_eq!(run(&["a\x1b_Gxyz\x1b\\b"]), "ab");
        assert_eq!(run(&["a\x1b", "_Gx", "yz\x1b", "\\b"]), "ab");
        assert_eq!(run(&["\x1b[1mbold\x1b", "[0m"]), "\x1b[1mbold\x1b[0m");
        assert_eq!(run(&["пр\x1b_Gп\x1b\\ивет"]), "привет");
        // An APC cut short by another escape sequence.
        assert_eq!(run(&["x\x1b_Gab\x1b[31mred"]), "x\x1b[31mred");
        let big = transmit("a=T,f=24,s=100,v=100", &vec![9u8; 30_000]);
        assert_eq!(run(&["start ", &big, " end"]), "start  end");
    }
}
