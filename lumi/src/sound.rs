//! Sounds Lumi plays (48 kHz, 16-bit, stereo, as `/dev/audio` takes
//! them): a quiet click that confirms a new volume, and the test chime of
//! the Sound settings. Sine waves come from the recurrence
//! y[n] = 2 cos(w) y[n-1] - y[n-2], so no sine function is needed.

use alloc::vec;
use alloc::vec::Vec;

const RATE: usize = 48_000;

/// A sine note: 2 cos(w) and sin(w) for its frequency at 48 kHz.
struct Note {
    k: f32,
    s1: f32,
}

/// 2000 Hz.
const CLICK: Note = Note {
    k: 1.931_852,
    s1: 0.258_819,
};
/// A5, 880 Hz.
const A5: Note = Note {
    k: 1.986_745,
    s1: 0.114_937,
};
/// E6, 1318.5 Hz.
const E6: Note = Note {
    k: 1.970_288,
    s1: 0.171_738,
};

/// Adds `note` to `out` (mono, f32) from frame `at` for `frames`, at peak
/// `amp`, rising over `attack` frames and then decaying by `decay` each
/// frame.
fn add(
    out: &mut [f32],
    note: &Note,
    at: usize,
    frames: usize,
    amp: f32,
    attack: usize,
    decay: f32,
) {
    let (mut y1, mut y2) = (note.s1, 0.0f32);
    let mut env = 1.0f32;
    for i in 0..frames.min(out.len().saturating_sub(at)) {
        let rise = if i < attack {
            i as f32 / attack as f32
        } else {
            env *= decay;
            1.0
        };
        out[at + i] += y2 * amp * rise * env;
        let y = note.k * y1 - y2;
        y2 = y1;
        y1 = y;
    }
}

/// Mono samples to interleaved stereo 16-bit little-endian bytes.
fn to_pcm(mono: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(mono.len() * 4);
    for &v in mono {
        let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&s.to_le_bytes());
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// A short, quiet click (18 ms at 2 kHz, a tenth of full scale).
pub fn click() -> Vec<u8> {
    let n = RATE * 18 / 1000;
    let mut m = vec![0.0f32; n];
    add(&mut m, &CLICK, 0, n, 0.1, 48, 0.994);
    to_pcm(&m)
}

/// The test sound: two bell-like notes, A5 then E6, one second.
pub fn chime() -> Vec<u8> {
    let n = RATE;
    let mut m = vec![0.0f32; n];
    add(&mut m, &A5, 0, n, 0.3, 240, 0.999_93);
    add(&mut m, &E6, RATE / 5, n, 0.25, 240, 0.999_9);
    // A short fade at the end so it does not stop with a click.
    let fade = RATE / 50;
    for i in 0..fade {
        m[n - 1 - i] *= i as f32 / fade as f32;
    }
    to_pcm(&m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn left(pcm: &[u8]) -> Vec<i16> {
        pcm.chunks(4)
            .map(|f| i16::from_le_bytes([f[0], f[1]]))
            .collect()
    }

    /// Upward zero crossings per second.
    fn pitch(s: &[i16]) -> f32 {
        let n = s.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
        n as f32 * RATE as f32 / s.len() as f32
    }

    #[test]
    fn the_click_is_short_and_quiet_and_the_chime_is_in_tune() {
        let c = left(&click());
        assert_eq!(c.len(), 864);
        let peak = c.iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert!((1500..=3300).contains(&peak), "peak {}", peak);
        assert!((1900.0..2100.0).contains(&pitch(&c)), "{}", pitch(&c));
        let ch = left(&chime());
        assert_eq!(ch.len(), RATE);
        // The first fifth is A5 alone.
        let p = pitch(&ch[..RATE / 5]);
        assert!((860.0..900.0).contains(&p), "{}", p);
        assert!(ch.iter().all(|v| v.unsigned_abs() < 32767));
        assert!(ch[RATE - 1].unsigned_abs() < 50, "no fade at the end");
    }
}
