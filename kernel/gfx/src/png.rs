//! PNG decoding: every colour type, bit depths 1 to 16, palette
//! transparency (`tRNS`). NOT IMPLEMENTED: Adam7 interlacing (such
//! files are refused), gamma and colour profiles (ignored), chunk CRCs
//! (not checked; the zlib checksum still guards the image data).

use crate::image::Image;
use crate::inflate;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NotPng,
    Truncated,
    Corrupt,
    /// Interlaced, or a colour type/bit depth combination PNG does not allow.
    Unsupported,
    /// More pixels than the caller allows.
    TooLarge,
}

impl From<inflate::Error> for Error {
    fn from(e: inflate::Error) -> Self {
        match e {
            inflate::Error::Truncated => Error::Truncated,
            inflate::Error::TooLarge => Error::TooLarge,
            _ => Error::Corrupt,
        }
    }
}

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Width and height from the header, without decoding.
pub fn size(data: &[u8]) -> Result<(usize, usize), Error> {
    if data.len() < 24 || data[..8] != SIGNATURE || &data[12..16] != b"IHDR" {
        return Err(Error::NotPng);
    }
    Ok((be32(&data[16..]) as usize, be32(&data[20..]) as usize))
}

/// Decodes a PNG file into an image of at most `max_pixels` pixels.
pub fn decode(data: &[u8], max_pixels: usize) -> Result<Image, Error> {
    let (width, height) = size(data)?;
    let mut pos = 8;
    let mut header = None;
    let mut palette: &[u8] = &[];
    let mut trns: &[u8] = &[];
    let mut idat = Vec::new();
    loop {
        let len = be32(data.get(pos..pos + 4).ok_or(Error::Truncated)?) as usize;
        let kind = data.get(pos + 4..pos + 8).ok_or(Error::Truncated)?;
        let body = data.get(pos + 8..pos + 8 + len).ok_or(Error::Truncated)?;
        match kind {
            b"IHDR" => header = Some(body),
            b"PLTE" => palette = body,
            b"tRNS" => trns = body,
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    let h = header.ok_or(Error::Corrupt)?;
    if h.len() < 13 {
        return Err(Error::Corrupt);
    }
    let (depth, ctype, interlace) = (h[8] as usize, h[9], h[12]);
    if width == 0 || height == 0 {
        return Err(Error::Corrupt);
    }
    if width.checked_mul(height).is_none_or(|n| n > max_pixels) {
        return Err(Error::TooLarge);
    }
    let channels = match (ctype, depth) {
        (0, 1 | 2 | 4 | 8 | 16) => 1,
        (3, 1 | 2 | 4 | 8) => 1,
        (2, 8 | 16) => 3,
        (4, 8 | 16) => 2,
        (6, 8 | 16) => 4,
        _ => return Err(Error::Unsupported),
    };
    if interlace != 0 || h[10] != 0 || h[11] != 0 {
        return Err(Error::Unsupported);
    }
    let bits = channels * depth;
    let stride = (width * bits).div_ceil(8);
    let bpp = bits.div_ceil(8);
    let raw = inflate::zlib(&idat, (stride + 1) * height)?;
    if raw.len() != (stride + 1) * height {
        return Err(Error::Truncated);
    }
    let rows = unfilter(&raw, stride, height, bpp)?;

    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = &rows[y * stride..(y + 1) * stride];
        for x in 0..width {
            let px = match ctype {
                0 | 3 => {
                    let v = sample(row, x, depth);
                    if ctype == 3 {
                        let i = v as usize;
                        let c = palette.get(i * 3..i * 3 + 3).ok_or(Error::Corrupt)?;
                        [c[0], c[1], c[2], trns.get(i).copied().unwrap_or(255)]
                    } else {
                        let g = scale(v, depth);
                        let a = if trns.len() >= 2 && v as usize == be16(trns) as usize {
                            0
                        } else {
                            255
                        };
                        [g, g, g, a]
                    }
                }
                _ => {
                    let mut c = [0u8; 4];
                    for (k, slot) in c.iter_mut().enumerate().take(channels) {
                        // 16-bit samples keep their high byte.
                        *slot = row[(x * channels + k) * (depth / 8)];
                    }
                    match ctype {
                        2 => {
                            let key = trns.len() >= 6
                                && (0..3).all(|k| {
                                    let at = (x * 3 + k) * (depth / 8);
                                    let v = if depth == 16 {
                                        be16(&row[at..])
                                    } else {
                                        row[at] as u16
                                    };
                                    v == be16(&trns[k * 2..])
                                });
                            [c[0], c[1], c[2], if key { 0 } else { 255 }]
                        }
                        4 => [c[0], c[0], c[0], c[1]],
                        _ => c,
                    }
                }
            };
            rgba.extend_from_slice(&px);
        }
    }
    Image::from_rgba(width, height, &rgba).ok_or(Error::Corrupt)
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// Sample `x` of a grey or palette row with `depth` bits per pixel.
fn sample(row: &[u8], x: usize, depth: usize) -> u16 {
    match depth {
        16 => be16(&row[x * 2..]),
        8 => row[x] as u16,
        _ => {
            let per = 8 / depth;
            let byte = row[x / per];
            let shift = 8 - depth * (x % per + 1);
            ((byte >> shift) & ((1u8 << depth) - 1)) as u16
        }
    }
}

/// A grey sample of `depth` bits as 8 bits.
fn scale(v: u16, depth: usize) -> u8 {
    match depth {
        16 => (v >> 8) as u8,
        8 => v as u8,
        _ => (v as u32 * 255 / ((1u32 << depth) - 1)) as u8,
    }
}

/// Undoes the per-row filters; returns the rows without filter bytes.
fn unfilter(raw: &[u8], stride: usize, height: usize, bpp: usize) -> Result<Vec<u8>, Error> {
    let mut out = alloc::vec![0u8; stride * height];
    for y in 0..height {
        let filter = raw[y * (stride + 1)];
        let src = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        let (done, rest) = out.split_at_mut(y * stride);
        let prev = if y > 0 {
            &done[(y - 1) * stride..]
        } else {
            &[][..]
        };
        let cur = &mut rest[..stride];
        for i in 0..stride {
            let a = if i >= bpp { cur[i - bpp] } else { 0 };
            let b = prev.get(i).copied().unwrap_or(0);
            let c = if i >= bpp {
                prev.get(i - bpp).copied().unwrap_or(0)
            } else {
                0
            };
            let pred = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err(Error::Corrupt),
            };
            cur[i] = src[i].wrapping_add(pred);
        }
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = (
        (p - a as i16).abs(),
        (p - b as i16).abs(),
        (p - c as i16).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Builds a PNG around raw (filtered) scan lines, stored without
    /// compression.
    fn png(
        w: u32,
        h: u32,
        depth: u8,
        ctype: u8,
        extra: &[(&[u8; 4], &[u8])],
        rows: &[u8],
    ) -> Vec<u8> {
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            out.extend_from_slice(&[0; 4]); // CRC is not checked
        }
        let mut out = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[depth, ctype, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        for (k, b) in extra {
            chunk(&mut out, k, b);
        }
        let mut z = vec![0x78, 0x01, 0x01];
        z.extend_from_slice(&(rows.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(rows.len() as u16)).to_le_bytes());
        z.extend_from_slice(rows);
        z.extend_from_slice(&inflate::adler32(rows).to_be_bytes());
        // Split IDAT in two to check that the parts are joined.
        let half = z.len() / 2;
        chunk(&mut out, b"IDAT", &z[..half]);
        chunk(&mut out, b"IDAT", &z[half..]);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn rgb_with_all_filters() {
        // 2x5 RGB; row y uses filter y. Raw pixels: (10,20,30) and (40,50,60)
        // on every row, encoded against each predictor.
        let rows = [
            0, 10, 20, 30, 40, 50, 60, // none
            1, 10, 20, 30, 30, 30, 30, // sub
            2, 0, 0, 0, 0, 0, 0, // up
            3, 5, 10, 15, 15, 15, 15, // average: (a+b)/2
            4, 0, 0, 0, 0, 0, 0, // paeth picks b
        ];
        let img = decode(&png(2, 5, 8, 2, &[], &rows), 100).unwrap();
        assert_eq!((img.width, img.height), (2, 5));
        for y in 0..5 {
            assert_eq!(img.pixel(0, y), [10, 20, 30, 255], "row {}", y);
            assert_eq!(img.pixel(1, y), [40, 50, 60, 255], "row {}", y);
        }
    }

    #[test]
    fn palette_with_transparency_and_low_depth() {
        // 4 pixels, 2 bits each: indices 0,1,2,1.
        let rows = [0, 0b00_01_10_01];
        let plte: &[u8] = &[255, 0, 0, 0, 255, 0, 0, 0, 255];
        let trns: &[u8] = &[0];
        let img = decode(
            &png(4, 1, 2, 3, &[(b"PLTE", plte), (b"tRNS", trns)], &rows),
            100,
        )
        .unwrap();
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(img.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(img.pixel(2, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn grey_alpha_and_16_bit() {
        let rows = [0, 200, 255, 100, 0];
        let img = decode(&png(2, 1, 8, 4, &[], &rows), 100).unwrap();
        assert_eq!(img.pixel(0, 0), [200, 200, 200, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 0]);
        let rows16 = [0, 0x12, 0x34, 0xab, 0xcd, 0xff, 0xff];
        let img = decode(&png(1, 1, 16, 2, &[], &rows16), 100).unwrap();
        assert_eq!(img.pixel(0, 0), [0x12, 0xab, 0xff, 255]);
        let grey1 = [0, 0b1010_0000];
        let img = decode(&png(3, 1, 1, 0, &[], &grey1), 100).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn refuses_bad_files() {
        let rows = [0, 1, 2, 3];
        let ok = png(1, 1, 8, 2, &[], &rows);
        assert_eq!(decode(&ok[..20], 100), Err(Error::NotPng));
        assert_eq!(decode(&ok[..60], 100), Err(Error::Truncated));
        assert_eq!(decode(&ok, 0), Err(Error::TooLarge));
        assert_eq!(
            decode(b"GIF89a.........................", 100),
            Err(Error::NotPng)
        );
        let mut interlaced = ok.clone();
        interlaced[28] = 1;
        assert_eq!(decode(&interlaced, 100), Err(Error::Unsupported));
        let bad_depth = png(1, 1, 4, 2, &[], &rows);
        assert_eq!(decode(&bad_depth, 100), Err(Error::Unsupported));
        let mut bad_filter = rows;
        bad_filter[0] = 9;
        assert_eq!(
            decode(&png(1, 1, 8, 2, &[], &bad_filter), 100),
            Err(Error::Corrupt)
        );
    }

    #[test]
    fn decodes_the_velora_logo() {
        let data = include_bytes!("../../../assets/velora-logo.png");
        assert_eq!(size(data), Ok((1024, 1024)));
        let img = decode(data, 1 << 21).unwrap();
        // Dark background in the corner, white in the middle of the bar.
        let corner = img.pixel(5, 5);
        assert!(corner[0] < 40 && corner[3] == 255);
        assert_eq!(img.pixel(510, 380), [255, 255, 255, 255]);
    }
}
