//! 8x16 bitmap font lookup.

#[path = "font_data.rs"]
mod data;

pub const WIDTH: usize = 8;
pub const HEIGHT: usize = 16;

/// Glyph for `c`, or `None` if the font does not cover it.
pub fn glyph(c: char) -> Option<&'static [u8; 16]> {
    let cp = c as u32;
    data::GLYPHS
        .binary_search_by_key(&cp, |&(code, _)| code)
        .ok()
        .map(|i| &data::GLYPHS[i].1)
}

/// Glyph for `c`, falling back to '?' for unknown characters.
pub fn glyph_or_replacement(c: char) -> &'static [u8; 16] {
    glyph(c)
        .or_else(|| glyph('?'))
        .expect("font has no '?' glyph")
}

pub fn glyph_count() -> usize {
    data::GLYPHS.len()
}
