//! fontsubset: keeps only the glyphs of the characters Velora shows in a
//! TrueType font and empties the others.
//!
//! `fontsubset IN.ttf OUT.ttf [--terminal]`
//!
//! fontdue (D47) turns the outline of every glyph of a font into line
//! segments when the font is loaded: Inter's 2937 glyphs take about
//! 8.4 MiB per face. Velora's programs show Latin with its European
//! extensions, Cyrillic and punctuation (and, with `--terminal`, box
//! drawing and blocks), so the other outlines are dropped. Glyph numbers
//! stay the same, so the character map, kerning and substitution tables
//! stay valid; a dropped glyph draws as nothing. Only `glyf`, `loca` and
//! `head` change. The fonts are under the SIL OFL 1.1, which allows
//! modified versions (neither font has a Reserved Font Name).

use std::collections::BTreeSet;
use std::process::exit;

fn be16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Characters kept.
fn wanted(terminal: bool) -> Vec<(u32, u32)> {
    let mut v = vec![
        (0x20, 0x7e),
        (0xa0, 0x17f),    // Latin-1 and Latin Extended-A
        (0x218, 0x21b),   // Romanian comma below
        (0x2c6, 0x2dd),   // spacing accents
        (0x300, 0x36f),   // combining accents
        (0x400, 0x52f),   // Cyrillic
        (0x2000, 0x206f), // general punctuation
        (0x20ac, 0x20ac), // euro
        (0x20bd, 0x20bd), // rouble
        (0x20b4, 0x20b4), // hryvnia
        (0x2116, 0x2116), // numero
        (0x2122, 0x2122), // trade mark
        (0x2190, 0x21ff), // arrows
        (0x2212, 0x2212), // minus
        (0x2026, 0x2026), // ellipsis
        (0x25cf, 0x25cf), // black circle (passwords)
        (0xfffd, 0xfffd), // replacement character
    ];
    if terminal {
        v.push((0x2500, 0x25ff)); // box drawing, blocks, shapes
        v.push((0x2800, 0x28ff)); // braille patterns (progress bars)
    }
    v
}

struct Table {
    tag: [u8; 4],
    data: Vec<u8>,
}

/// Glyph ids of the characters in `ranges`, from the cmap.
fn cmap_glyphs(cmap: &[u8], ranges: &[(u32, u32)]) -> BTreeSet<u16> {
    let want = |c: u32| ranges.iter().any(|&(a, b)| c >= a && c <= b);
    let mut out = BTreeSet::new();
    let n = be16(cmap, 2) as usize;
    for i in 0..n {
        let off = be32(cmap, 4 + i * 8 + 4) as usize;
        let st = &cmap[off..];
        match be16(st, 0) {
            4 => {
                let segs = be16(st, 6) as usize / 2;
                let ends = 14;
                let starts = ends + segs * 2 + 2;
                let deltas = starts + segs * 2;
                let offsets = deltas + segs * 2;
                for s in 0..segs {
                    let end = be16(st, ends + s * 2) as u32;
                    let start = be16(st, starts + s * 2) as u32;
                    let delta = be16(st, deltas + s * 2);
                    let ro = be16(st, offsets + s * 2) as usize;
                    for c in start..=end.min(0xfffe) {
                        if !want(c) {
                            continue;
                        }
                        let g = if ro == 0 {
                            (c as u16).wrapping_add(delta)
                        } else {
                            let at = offsets + s * 2 + ro + (c - start) as usize * 2;
                            if at + 1 >= st.len() {
                                continue;
                            }
                            let g = be16(st, at);
                            if g == 0 { 0 } else { g.wrapping_add(delta) }
                        };
                        if g != 0 {
                            out.insert(g);
                        }
                    }
                }
            }
            12 => {
                let groups = be32(st, 12) as usize;
                for k in 0..groups {
                    let b = 16 + k * 12;
                    let (a, e, g0) = (be32(st, b), be32(st, b + 4), be32(st, b + 8));
                    for c in a..=e {
                        if want(c) {
                            out.insert((g0 + c - a) as u16);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: fontsubset IN.ttf OUT.ttf [--terminal]");
        exit(2);
    }
    let terminal = args.iter().any(|a| a == "--terminal");
    let font = std::fs::read(&args[0]).unwrap_or_else(|e| {
        eprintln!("fontsubset: {}: {}", args[0], e);
        exit(1)
    });
    let num = be16(&font, 4) as usize;
    let mut tables: Vec<Table> = (0..num)
        .map(|i| {
            let r = 12 + i * 16;
            let off = be32(&font, r + 8) as usize;
            let len = be32(&font, r + 12) as usize;
            Table {
                tag: font[r..r + 4].try_into().unwrap(),
                data: font[off..off + len].to_vec(),
            }
        })
        .collect();
    let find = |t: &[Table], tag: &[u8; 4]| t.iter().position(|x| &x.tag == tag);
    let (Some(hi), Some(li), Some(gi), Some(ci), Some(mi)) = (
        find(&tables, b"head"),
        find(&tables, b"loca"),
        find(&tables, b"glyf"),
        find(&tables, b"cmap"),
        find(&tables, b"maxp"),
    ) else {
        eprintln!("fontsubset: not a TrueType (glyf) font");
        exit(1)
    };
    let glyphs = be16(&tables[mi].data, 4) as usize;
    let long = be16(&tables[hi].data, 50) == 1;
    let loca = &tables[li].data;
    let at = |g: usize| -> usize {
        if long {
            be32(loca, g * 4) as usize
        } else {
            be16(loca, g * 2) as usize * 2
        }
    };
    let glyf = tables[gi].data.clone();
    let mut keep = cmap_glyphs(&tables[ci].data, &wanted(terminal));
    keep.insert(0);
    // Components of composite glyphs are needed too.
    let mut todo: Vec<u16> = keep.iter().copied().collect();
    while let Some(g) = todo.pop() {
        let (a, b) = (at(g as usize), at(g as usize + 1));
        if b <= a || (be16(&glyf, a) as i16) >= 0 {
            continue;
        }
        let mut p = a + 10;
        loop {
            let flags = be16(&glyf, p);
            let comp = be16(&glyf, p + 2);
            if keep.insert(comp) {
                todo.push(comp);
            }
            p += 4 + if flags & 1 != 0 { 4 } else { 2 };
            p += if flags & 0x8 != 0 {
                2
            } else if flags & 0x40 != 0 {
                4
            } else if flags & 0x80 != 0 {
                8
            } else {
                0
            };
            if flags & 0x20 == 0 {
                break;
            }
        }
    }
    let mut new_glyf = Vec::new();
    let mut new_loca = Vec::with_capacity((glyphs + 1) * 4);
    for g in 0..glyphs {
        new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
        if keep.contains(&(g as u16)) {
            new_glyf.extend_from_slice(&glyf[at(g)..at(g + 1)]);
            while new_glyf.len() % 4 != 0 {
                new_glyf.push(0);
            }
        }
    }
    new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
    tables[gi].data = new_glyf;
    tables[li].data = new_loca;
    // Long offsets; the checksum adjustment is left at zero.
    tables[hi].data[50..52].copy_from_slice(&1u16.to_be_bytes());
    tables[hi].data[8..12].copy_from_slice(&[0; 4]);
    // Write the font again.
    let mut out = Vec::new();
    out.extend_from_slice(&font[0..12]);
    let mut offset = 12 + num * 16;
    let mut body = Vec::new();
    for t in &tables {
        let mut sum = 0u32;
        let mut padded = t.data.clone();
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        for c in padded.chunks(4) {
            sum = sum.wrapping_add(u32::from_be_bytes(c.try_into().unwrap()));
        }
        out.extend_from_slice(&t.tag);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(t.data.len() as u32).to_be_bytes());
        offset += padded.len();
        body.extend_from_slice(&padded);
    }
    out.extend_from_slice(&body);
    std::fs::write(&args[1], &out).unwrap_or_else(|e| {
        eprintln!("fontsubset: {}: {}", args[1], e);
        exit(1)
    });
    println!(
        "fontsubset: wrote {} ({} of {} glyphs, {} KiB)",
        args[1],
        keep.len(),
        glyphs,
        out.len() / 1024
    );
}
