//! Codec verbs, the widget graph of the audio function group, output
//! paths and their volume (HDA specification 1.0a, section 7).

use alloc::vec::Vec;

/// Sends one verb to a codec and returns its response (`None`: no
/// response in time).
pub trait Codec {
    fn cmd(&mut self, verb: u32) -> Option<u32>;
}

// 12-bit verbs (8-bit payload).
pub const GET_PARAMETER: u32 = 0xf00;
pub const GET_CONN_LIST: u32 = 0xf02;
pub const GET_CONFIG_DEFAULT: u32 = 0xf1c;
pub const SET_CONN_SELECT: u32 = 0x701;
pub const SET_POWER_STATE: u32 = 0x705;
pub const SET_STREAM: u32 = 0x706;
pub const SET_PIN_CONTROL: u32 = 0x707;
pub const SET_EAPD: u32 = 0x70c;
// 4-bit verbs (16-bit payload).
pub const SET_FORMAT: u32 = 0x2;
pub const SET_AMP: u32 = 0x3;

// Parameters.
pub const P_VENDOR: u8 = 0x00;
pub const P_NODES: u8 = 0x04;
pub const P_FUNCTION: u8 = 0x05;
pub const P_WIDGET_CAPS: u8 = 0x09;
pub const P_PIN_CAPS: u8 = 0x0c;
pub const P_CONN_LEN: u8 = 0x0e;
pub const P_OUT_AMP: u8 = 0x12;

// Widget capabilities.
const WC_STEREO: u32 = 1 << 0;
const WC_IN_AMP: u32 = 1 << 1;
const WC_OUT_AMP: u32 = 1 << 2;
const WC_AMP_OVERRIDE: u32 = 1 << 3;
const WC_CONN_LIST: u32 = 1 << 8;
// Pin capabilities.
const PC_OUTPUT: u32 = 1 << 4;
const PC_HEADPHONE: u32 = 1 << 3;
const PC_EAPD: u32 = 1 << 16;
// Pin widget control.
const PIN_OUT: u32 = 1 << 6;
const PIN_HP: u32 = 1 << 7;
// Amplifier gain/mute payload.
const AMP_OUT: u32 = 1 << 15;
const AMP_IN: u32 = 1 << 14;
const AMP_LEFT: u32 = 1 << 13;
const AMP_RIGHT: u32 = 1 << 12;
const AMP_MUTE: u32 = 1 << 7;

/// A verb with an 8-bit payload.
pub fn verb12(cad: u8, nid: u8, verb: u32, payload: u8) -> u32 {
    (cad as u32 & 0xf) << 28 | (nid as u32) << 20 | (verb & 0xfff) << 8 | payload as u32
}

/// A verb with a 16-bit payload.
pub fn verb4(cad: u8, nid: u8, verb: u32, payload: u16) -> u32 {
    (cad as u32 & 0xf) << 28 | (nid as u32) << 20 | (verb & 0xf) << 16 | payload as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetType {
    Output,
    Input,
    Mixer,
    Selector,
    Pin,
    Other,
}

/// What an output pin is connected to (default device of its
/// configuration).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jack {
    LineOut,
    Speaker,
    Headphones,
    Digital,
    Other,
}

impl Jack {
    /// A name for the user (Russian, English).
    pub fn name(self) -> (&'static str, &'static str) {
        match self {
            Jack::Speaker => ("Динамики", "Speakers"),
            Jack::Headphones => ("Наушники", "Headphones"),
            Jack::LineOut => ("Линейный выход", "Line out"),
            Jack::Digital => ("Цифровой выход", "Digital out"),
            Jack::Other => ("Выход", "Output"),
        }
    }

    /// Order of preference for the default output.
    fn rank(self) -> u8 {
        match self {
            Jack::Speaker => 0,
            Jack::Headphones => 1,
            Jack::LineOut => 2,
            Jack::Other => 3,
            Jack::Digital => 4,
        }
    }
}

/// Output amplifier capabilities.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AmpCaps {
    /// Gain step of 0 dB.
    pub offset: u8,
    /// Highest gain step.
    pub steps: u8,
    pub mute: bool,
}

impl AmpCaps {
    fn parse(v: u32) -> AmpCaps {
        AmpCaps {
            offset: (v & 0x7f) as u8,
            steps: ((v >> 8) & 0x7f) as u8,
            mute: v & (1 << 31) != 0,
        }
    }

    fn usable(&self) -> bool {
        self.steps > 0 || self.mute
    }
}

#[derive(Clone, Debug)]
pub struct Widget {
    pub nid: u8,
    pub kind: WidgetType,
    pub caps: u32,
    pub pin_caps: u32,
    pub config: u32,
    pub conns: Vec<u8>,
    pub out_amp: AmpCaps,
}

impl Widget {
    fn has_out_amp(&self) -> bool {
        self.caps & WC_OUT_AMP != 0 && self.out_amp.usable()
    }
}

/// The audio function group of one codec.
#[derive(Clone, Debug)]
pub struct Graph {
    pub cad: u8,
    pub afg: u8,
    pub vendor: u32,
    pub widgets: Vec<Widget>,
}

/// One way from an output pin to a converter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    pub jack: Jack,
    /// Nodes from the pin to the converter, each with the index of the
    /// connection it takes to the next one (the converter's is 0).
    pub nodes: Vec<(u8, u8)>,
}

impl Path {
    pub fn pin(&self) -> u8 {
        self.nodes[0].0
    }

    pub fn dac(&self) -> u8 {
        self.nodes[self.nodes.len() - 1].0
    }
}

fn param(c: &mut dyn Codec, cad: u8, nid: u8, p: u8) -> Option<u32> {
    c.cmd(verb12(cad, nid, GET_PARAMETER, p))
}

/// Reads the connection list of `nid` (short and long forms, ranges).
fn conn_list(c: &mut dyn Codec, cad: u8, nid: u8) -> Option<Vec<u8>> {
    let len_v = param(c, cad, nid, P_CONN_LEN)?;
    let long = len_v & 0x80 != 0;
    let len = (len_v & 0x7f) as usize;
    let per = if long { 2 } else { 4 };
    let bits = if long { 16 } else { 8 };
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < len {
        let v = c.cmd(verb12(cad, nid, GET_CONN_LIST, i as u8))?;
        for k in 0..per {
            if i + k >= len {
                break;
            }
            let e = (v >> (k * bits)) & ((1 << bits) - 1);
            let range = e & (1 << (bits - 1)) != 0;
            let id = (e & ((1 << (bits - 1)) - 1)) as u8;
            match (range, out.last().copied()) {
                (true, Some(prev)) if id > prev => out.extend(prev + 1..=id),
                _ => out.push(id),
            }
        }
        i += per;
    }
    Some(out)
}

impl Graph {
    /// Finds the audio function group of codec `cad` and reads its
    /// widgets. `None` if the codec has none or does not answer.
    pub fn read(c: &mut dyn Codec, cad: u8) -> Option<Graph> {
        let vendor = param(c, cad, 0, P_VENDOR)?;
        let nodes = param(c, cad, 0, P_NODES)?;
        let (start, count) = (((nodes >> 16) & 0xff) as u8, (nodes & 0xff) as u8);
        let afg = (start..start.saturating_add(count))
            .find(|&n| param(c, cad, n, P_FUNCTION).is_some_and(|f| f & 0xff == 1))?;
        let afg_amp = param(c, cad, afg, P_OUT_AMP).unwrap_or(0);
        let wn = param(c, cad, afg, P_NODES)?;
        let (ws, wc) = (((wn >> 16) & 0xff) as u8, (wn & 0xff) as u8);
        let mut widgets = Vec::new();
        for nid in ws..ws.saturating_add(wc) {
            let Some(caps) = param(c, cad, nid, P_WIDGET_CAPS) else {
                continue;
            };
            let kind = match (caps >> 20) & 0xf {
                0 => WidgetType::Output,
                1 => WidgetType::Input,
                2 => WidgetType::Mixer,
                3 => WidgetType::Selector,
                4 => WidgetType::Pin,
                _ => WidgetType::Other,
            };
            let (pin_caps, config) = if kind == WidgetType::Pin {
                (
                    param(c, cad, nid, P_PIN_CAPS).unwrap_or(0),
                    c.cmd(verb12(cad, nid, GET_CONFIG_DEFAULT, 0)).unwrap_or(0),
                )
            } else {
                (0, 0)
            };
            let conns = if caps & WC_CONN_LIST != 0 {
                conn_list(c, cad, nid).unwrap_or_default()
            } else {
                Vec::new()
            };
            let amp = if caps & WC_AMP_OVERRIDE != 0 {
                param(c, cad, nid, P_OUT_AMP).unwrap_or(0)
            } else {
                afg_amp
            };
            widgets.push(Widget {
                nid,
                kind,
                caps,
                pin_caps,
                config,
                conns,
                out_amp: AmpCaps::parse(amp),
            });
        }
        Some(Graph {
            cad,
            afg,
            vendor,
            widgets,
        })
    }

    pub fn widget(&self, nid: u8) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.nid == nid)
    }

    /// The output pins with a way to a converter, the preferred one first
    /// (speakers, headphones, line out, others, digital). Pins the
    /// configuration marks as not connected are left out.
    pub fn output_paths(&self) -> Vec<Path> {
        let mut paths = Vec::new();
        for w in &self.widgets {
            if w.kind != WidgetType::Pin || w.pin_caps & PC_OUTPUT == 0 {
                continue;
            }
            // Port connectivity 1: nothing is attached.
            if (w.config >> 30) == 1 {
                continue;
            }
            let jack = match (w.config >> 20) & 0xf {
                0x0 => Jack::LineOut,
                0x1 => Jack::Speaker,
                0x2 => Jack::Headphones,
                0x4 | 0x5 => Jack::Digital,
                0x3 | 0x6..=0xe => continue, // inputs (CD, mic, line in, ...)
                _ => Jack::Other,
            };
            let mut nodes = Vec::new();
            if self.walk(w.nid, &mut nodes, 0) {
                paths.push(Path { jack, nodes });
            }
        }
        paths.sort_by_key(|p| p.jack.rank());
        paths
    }

    /// Depth-first search from `nid` to an output converter.
    fn walk(&self, nid: u8, nodes: &mut Vec<(u8, u8)>, depth: usize) -> bool {
        let Some(w) = self.widget(nid) else {
            return false;
        };
        if nodes.iter().any(|n| n.0 == nid) || depth > 6 {
            return false;
        }
        if w.kind == WidgetType::Output {
            nodes.push((nid, 0));
            return true;
        }
        if depth > 0 && !matches!(w.kind, WidgetType::Mixer | WidgetType::Selector) {
            return false;
        }
        for (i, &next) in w.conns.iter().enumerate() {
            nodes.push((nid, i as u8));
            if self.walk(next, nodes, depth + 1) {
                return true;
            }
            nodes.pop();
        }
        false
    }

    /// The node of `path` whose output amplifier sets the volume: the
    /// converter's, else the nearest one to it.
    pub fn volume_node(&self, path: &Path) -> Option<u8> {
        path.nodes
            .iter()
            .rev()
            .map(|n| n.0)
            .find(|&n| self.widget(n).is_some_and(|w| w.has_out_amp()))
    }

    /// Powers the nodes of `path` on, selects its connections, opens its
    /// amplifiers at 0 dB, enables the pin's output and connects the
    /// converter to `stream` with `format`. The volume is set apart.
    pub fn set_up(&self, c: &mut dyn Codec, path: &Path, stream: u8, format: u16) {
        let cad = self.cad;
        let mut send = |v: u32| {
            let _ = c.cmd(v);
        };
        send(verb12(cad, self.afg, SET_POWER_STATE, 0));
        for (k, &(nid, idx)) in path.nodes.iter().enumerate() {
            let Some(w) = self.widget(nid) else { continue };
            send(verb12(cad, nid, SET_POWER_STATE, 0));
            if w.conns.len() > 1 && matches!(w.kind, WidgetType::Pin | WidgetType::Selector) {
                send(verb12(cad, nid, SET_CONN_SELECT, idx));
            }
            // A mixer: open the input that comes from the next node.
            if w.kind == WidgetType::Mixer && w.caps & WC_IN_AMP != 0 {
                let p = AMP_IN | AMP_LEFT | AMP_RIGHT | (idx as u32) << 8;
                send(verb4(cad, nid, SET_AMP, p as u16));
            }
            // A pin's input amplifier on the way out does nothing; others
            // with one get it opened.
            if w.kind != WidgetType::Mixer && w.kind != WidgetType::Pin && w.caps & WC_IN_AMP != 0 {
                send(verb4(
                    cad,
                    nid,
                    SET_AMP,
                    (AMP_IN | AMP_LEFT | AMP_RIGHT) as u16,
                ));
            }
            if w.has_out_amp() {
                let p = AMP_OUT | AMP_LEFT | AMP_RIGHT | w.out_amp.offset as u32;
                send(verb4(cad, nid, SET_AMP, p as u16));
            }
            if k == 0 {
                let mut ctl = PIN_OUT;
                if path.jack == Jack::Headphones && w.pin_caps & PC_HEADPHONE != 0 {
                    ctl |= PIN_HP;
                }
                send(verb12(cad, nid, SET_PIN_CONTROL, ctl as u8));
                if w.pin_caps & PC_EAPD != 0 {
                    send(verb12(cad, nid, SET_EAPD, 0x02));
                }
            }
        }
        let dac = path.dac();
        send(verb4(cad, dac, SET_FORMAT, format));
        send(verb12(cad, dac, SET_STREAM, stream << 4));
    }

    /// Sets the volume of `path`, 0..=100 per cent (0 or `mute`: muted).
    /// Returns false if the path has no amplifier (the caller then scales
    /// the samples, [`soft_gain`]).
    pub fn set_volume(&self, c: &mut dyn Codec, path: &Path, percent: u32, mute: bool) -> bool {
        let Some(nid) = self.volume_node(path) else {
            return false;
        };
        let w = self.widget(nid).unwrap();
        let p = AMP_OUT | AMP_LEFT | AMP_RIGHT | amp_payload(&w.out_amp, percent, mute);
        let _ = c.cmd(verb4(self.cad, nid, SET_AMP, p as u16));
        true
    }

    /// Whether the widget at `nid` sends stereo.
    pub fn stereo(&self, nid: u8) -> bool {
        self.widget(nid).is_some_and(|w| w.caps & WC_STEREO != 0)
    }
}

/// The gain and mute bits for `percent` on an amplifier with `caps`.
/// The steps are even in decibels, so the per cent go along them
/// linearly, up to 0 dB (the gain above it would clip); 0 per cent is
/// muted, or the lowest step if the amplifier cannot mute.
pub fn amp_payload(caps: &AmpCaps, percent: u32, mute: bool) -> u32 {
    let top = if caps.offset > 0 {
        caps.offset
    } else {
        caps.steps
    } as u32;
    let percent = percent.min(100);
    if mute || percent == 0 {
        return if caps.mute { AMP_MUTE } else { 0 };
    }
    // The lowest audible part of the range: 1 % is a quarter of the way.
    let gain = top / 4 + (top - top / 4) * percent / 100;
    gain.min(top)
}

/// Gain for samples, of 65536, when the path has no amplifier: a power
/// curve so the low half of the slider is not mostly silence.
pub fn soft_gain(percent: u32, mute: bool) -> u32 {
    if mute || percent == 0 {
        return 0;
    }
    let p = percent.min(100);
    p * p * 65536 / 10_000
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A codec that answers from a table and records the verbs it got.
    #[derive(Default)]
    pub struct Fake {
        pub answers: HashMap<u32, u32>,
        pub sent: Vec<u32>,
    }

    impl Codec for Fake {
        fn cmd(&mut self, verb: u32) -> Option<u32> {
            self.sent.push(verb);
            Some(self.answers.get(&verb).copied().unwrap_or(0))
        }
    }

    impl Fake {
        fn param(&mut self, nid: u8, p: u8, v: u32) {
            self.answers.insert(verb12(0, nid, GET_PARAMETER, p), v);
        }

        /// A widget of `kind` (the type field) with capabilities `caps`.
        fn widget(&mut self, nid: u8, kind: u32, caps: u32) {
            self.param(nid, P_WIDGET_CAPS, kind << 20 | caps);
        }

        fn conns(&mut self, nid: u8, list: &[u8]) {
            self.param(nid, P_CONN_LEN, list.len() as u32);
            for (i, chunk) in list.chunks(4).enumerate() {
                let mut v = 0;
                for (k, &n) in chunk.iter().enumerate() {
                    v |= (n as u32) << (k * 8);
                }
                self.answers
                    .insert(verb12(0, nid, GET_CONN_LIST, (i * 4) as u8), v);
            }
        }

        fn pin(&mut self, nid: u8, caps: u32, device: u32, conn: u32) {
            self.param(nid, P_PIN_CAPS, caps);
            self.answers.insert(
                verb12(0, nid, GET_CONFIG_DEFAULT, 0),
                conn << 30 | device << 20,
            );
        }
    }

    /// A laptop-like codec: AFG 1, widgets 2..=12. DACs 2 and 3 (out amp,
    /// 0 dB at step 87 of 87, mute), mixer 4 <- (2, 7), selector 5 <- (3),
    /// speaker pin 10 <- (4), headphone pin 11 <- (4, 5), mic pin 12,
    /// line out pin 9 not connected, ADC 7 (input).
    pub fn laptop() -> Fake {
        let mut f = Fake::default();
        f.param(0, P_VENDOR, 0x10ec_0269);
        f.param(0, P_NODES, 1 << 16 | 1);
        f.param(1, P_FUNCTION, 1);
        f.param(1, P_NODES, 2 << 16 | 11);
        f.param(1, P_OUT_AMP, 1 << 31 | 87 << 8 | 87);
        for dac in [2, 3] {
            f.widget(dac, 0, WC_STEREO | WC_OUT_AMP);
        }
        f.widget(4, 2, WC_STEREO | WC_IN_AMP | WC_CONN_LIST);
        f.conns(4, &[2, 7]);
        f.widget(5, 3, WC_STEREO | WC_CONN_LIST);
        f.conns(5, &[3]);
        f.widget(7, 1, WC_STEREO);
        f.widget(9, 4, WC_STEREO | WC_CONN_LIST);
        f.conns(9, &[4]);
        f.pin(9, PC_OUTPUT, 0x0, 1);
        f.widget(10, 4, WC_STEREO | WC_CONN_LIST);
        f.conns(10, &[4]);
        f.pin(10, PC_OUTPUT | PC_EAPD, 0x1, 2);
        f.widget(11, 4, WC_STEREO | WC_CONN_LIST);
        f.conns(11, &[4, 5]);
        f.pin(11, PC_OUTPUT | PC_HEADPHONE, 0x2, 0);
        f.widget(12, 4, WC_STEREO);
        f.pin(12, 0, 0xa, 0);
        f
    }

    #[test]
    fn verbs_are_encoded() {
        assert_eq!(verb12(2, 0x14, SET_PIN_CONTROL, 0x40), 0x2147_0740);
        assert_eq!(verb4(0, 2, SET_FORMAT, 0x11), 0x0022_0011);
        assert_eq!(crate::FORMAT_48K_16_STEREO, 0x11);
    }

    #[test]
    fn paths_go_from_output_pins_to_converters() {
        let mut f = laptop();
        let g = Graph::read(&mut f, 0).unwrap();
        assert_eq!(g.afg, 1);
        assert_eq!(g.widgets.len(), 11);
        let paths = g.output_paths();
        // Speaker first; the line out is not connected; the mic is no output.
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].jack, Jack::Speaker);
        assert_eq!(paths[0].nodes, vec![(10, 0), (4, 0), (2, 0)]);
        assert_eq!(paths[1].jack, Jack::Headphones);
        assert_eq!(paths[1].dac(), 2);
        assert_eq!(g.volume_node(&paths[0]), Some(2));
        // Setting the speaker up: pin output on, EAPD, mixer input 0
        // open, DAC format and stream 1.
        f.sent.clear();
        g.set_up(&mut f, &paths[0], 1, 0x11);
        assert!(f.sent.contains(&verb12(0, 10, SET_PIN_CONTROL, 0x40)));
        assert!(f.sent.contains(&verb12(0, 10, SET_EAPD, 2)));
        assert!(f.sent.contains(&verb4(0, 4, SET_AMP, 0x7000)));
        assert!(f.sent.contains(&verb4(0, 2, SET_AMP, 0xb000 | 87)));
        assert!(f.sent.contains(&verb4(0, 2, SET_FORMAT, 0x11)));
        assert!(f.sent.contains(&verb12(0, 2, SET_STREAM, 0x10)));
        // Volume: 100 % is 0 dB, mute sets the mute bit.
        f.sent.clear();
        assert!(g.set_volume(&mut f, &paths[0], 100, false));
        assert_eq!(f.sent, vec![verb4(0, 2, SET_AMP, 0xb000 | 87)]);
        f.sent.clear();
        g.set_volume(&mut f, &paths[0], 40, true);
        assert_eq!(f.sent, vec![verb4(0, 2, SET_AMP, 0xb080)]);
    }

    #[test]
    fn connection_list_ranges_and_volume_curve() {
        let mut f = Fake::default();
        f.param(5, P_CONN_LEN, 3);
        // 2, then a range up to 5, then 9.
        f.answers.insert(
            verb12(0, 5, GET_CONN_LIST, 0),
            2 | (0x80 | 5) << 8 | 9 << 16,
        );
        assert_eq!(conn_list(&mut f, 0, 5).unwrap(), vec![2, 3, 4, 5, 9]);
        let caps = AmpCaps {
            offset: 60,
            steps: 80,
            mute: true,
        };
        let g: Vec<u32> = [1, 50, 100]
            .iter()
            .map(|&p| amp_payload(&caps, p, false))
            .collect();
        assert!(g[0] < g[1] && g[1] < g[2] && g[2] == 60, "{:?}", g);
        assert_eq!(amp_payload(&caps, 0, false), AMP_MUTE);
        assert_eq!(soft_gain(100, false), 65536);
        assert_eq!(soft_gain(50, true), 0);
    }
}
