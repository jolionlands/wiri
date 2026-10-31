//! The GDI half of Pango-markup titles: one font per distinct run style, and the
//! measurement and painting that use it.
//!
//! Everything here lives exactly as long as the Important Hotkeys sheet is on screen:
//! [`FontCache`] is created with the sheet's session, keeps one `CreateFontIndirectW`
//! handle per distinct [`Style`] it is asked for, and deletes them when the session ends
//! (there is no global font state, no timer and no thread of its own).  Measurement
//! ([`FontCache::advance`], through [`pango_markup::TextMeasurer`]) and painting
//! ([`draw`]) select the *same* cached font for the same style, which is what makes the
//! width the sheet lays out with the width it paints.
//!
//! Honoured from markup: face, size, weight, italic, foreground, background, alpha
//! (composited against the sheet — GDI text has no alpha channel), underline
//! (none/single/double/low/error, in `underline_color`), strikethrough (in
//! `strikethrough_color`), `rise` (baseline shift) and `letter_spacing`.  Not honourable
//! in GDI and therefore not rendered: small caps, font stretch, font features, fallback,
//! language and gravity.

use crate::overlay::pango_markup::{self, Run, Style, TextMeasurer, Underline, PANGO_SCALE};
use std::collections::HashMap;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, CreateSolidBrush, DeleteObject, ExtTextOutW, FillRect,
    GetTextExtentPoint32W, GetTextMetricsW, SelectObject, SetBkMode, SetTextCharacterExtra,
    SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    ETO_CLIPPED, FF_DONTCARE, FF_MODERN, FIXED_PITCH, HDC, HGDIOBJ, LOGFONTW, OUT_DEFAULT_PRECIS,
    TEXTMETRICW, TRANSPARENT,
};

/// The sheet's background (`FillRect` with `COLORREF(0x00201c1c)`), which is what a
/// semi-transparent run colour is composited against.
pub const SHEET_BACKGROUND: pango_markup::Rgba = pango_markup::Rgba::rgb(0x1c, 0x1c, 0x20);

/// The face the sheet uses when a run names no family: `None` in [`Style::face`] means
/// this one.
const DEFAULT_FACE: &str = "Segoe UI";

/// One cached GDI font and its metrics, copied out so callers do not borrow the cache.
#[derive(Debug, Clone, Copy)]
struct Face {
    font: isize,
    ascent: i32,
    descent: i32,
}

/// What makes two runs need different GDI fonts.  Everything that only changes colour,
/// decoration, rise or spacing shares one font.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FontKey {
    face: Option<String>,
    size: i32,
    weight: u16,
    italic: bool,
}

/// The fonts of one showing of the sheet.  Created on first use, deleted on drop.
#[derive(Default)]
pub struct FontCache {
    faces: HashMap<FontKey, Face>,
}

impl FontCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many distinct GDI fonts this showing created (for tests and log lines).
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// The font for `style`, created on first use.  `dc` is only used to read the
    /// metrics of a new font.
    fn face(&mut self, dc: HDC, style: &Style, scale: f64) -> Face {
        let key = FontKey {
            face: style.face.clone(),
            size: style.size,
            weight: style.weight,
            italic: style.italic,
        };
        if let Some(face) = self.faces.get(&key) {
            return *face;
        }
        let face = unsafe { create_face(dc, &key, scale) };
        self.faces.insert(key, face);
        face
    }

    /// The advance of `text` in `style`, with that style's font selected: the one
    /// measurement the sheet lays a title out with and paints it with.
    pub fn advance(&mut self, dc: HDC, text: &str, style: &Style, scale: f64) -> i32 {
        if text.is_empty() {
            return 0;
        }
        let face = self.face(dc, style, scale);
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let old = SelectObject(dc, HGDIOBJ(face.font as *mut _));
            // Measured without the character extra and added back per UTF-16 unit, so
            // the number does not depend on what the last painted run set.
            let _ = SetTextCharacterExtra(dc, 0);
            let mut size = SIZE::default();
            let ok = GetTextExtentPoint32W(dc, &wide, &mut size).as_bool();
            SelectObject(dc, old);
            if ok {
                size.cx + letter_spacing_px(style, scale) * wide.len() as i32
            } else {
                0
            }
        }
    }
    /// Select the cached font for `style` into `dc` and return what was selected before.
    pub fn select(&mut self, dc: HDC, style: &Style, scale: f64) -> HGDIOBJ {
        let face = self.face(dc, style, scale);
        unsafe { SelectObject(dc, HGDIOBJ(face.font as *mut _)) }
    }
}

impl Drop for FontCache {
    fn drop(&mut self) {
        for (_, face) in self.faces.drain() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(face.font as *mut _));
            }
        }
    }
}

/// The GDI measurer the layout uses: one advance per run, in that run's font.
pub struct GdiMeasurer<'a> {
    dc: HDC,
    scale: f64,
    fonts: &'a mut FontCache,
}

impl<'a> GdiMeasurer<'a> {
    pub fn new(dc: HDC, scale: f64, fonts: &'a mut FontCache) -> Self {
        Self { dc, scale, fonts }
    }
}

impl TextMeasurer for GdiMeasurer<'_> {
    fn advance(&mut self, text: &str, style: &Style) -> i32 {
        self.fonts.advance(self.dc, text, style, self.scale)
    }
}

fn letter_spacing_px(style: &Style, scale: f64) -> i32 {
    points_to_px(style.letter_spacing, scale).round() as i32
}

/// [`PANGO_SCALE`]ths of a point to physical pixels at this DPI scale (96 dpi reference).
fn points_to_px(units: i32, scale: f64) -> f64 {
    (units as f64 / PANGO_SCALE as f64) * (96.0 / 72.0) * scale
}

unsafe fn create_face(dc: HDC, key: &FontKey, scale: f64) -> Face {
    let height = (-points_to_px(key.size, scale).round() as i32).min(-1);
    let family = key.face.as_deref().unwrap_or(DEFAULT_FACE);
    let mono = family.eq_ignore_ascii_case("Consolas");
    let mut logfont = LOGFONTW {
        lfHeight: height,
        lfWidth: 0,
        lfEscapement: 0,
        lfOrientation: 0,
        lfWeight: key.weight as i32,
        lfItalic: key.italic as u8,
        lfUnderline: 0,
        lfStrikeOut: 0,
        lfCharSet: DEFAULT_CHARSET,
        lfOutPrecision: OUT_DEFAULT_PRECIS,
        lfClipPrecision: CLIP_DEFAULT_PRECIS,
        lfQuality: CLEARTYPE_QUALITY,
        lfPitchAndFamily: if mono {
            FIXED_PITCH.0 | FF_MODERN.0
        } else {
            DEFAULT_PITCH.0 | FF_DONTCARE.0
        },
        lfFaceName: [0; 32],
    };
    for (slot, unit) in logfont
        .lfFaceName
        .iter_mut()
        .zip(family.encode_utf16().take(31))
    {
        *slot = unit;
    }
    let font = CreateFontIndirectW(&logfont);
    let old = SelectObject(dc, HGDIOBJ(font.0));
    let mut metrics = TEXTMETRICW::default();
    let ok = GetTextMetricsW(dc, &mut metrics).as_bool();
    SelectObject(dc, old);
    Face {
        font: font.0 as isize,
        ascent: if ok { metrics.tmAscent } else { 10 },
        descent: if ok { metrics.tmDescent } else { 3 },
    }
}

/// Paint `runs` inside `rect`, left-aligned on one baseline.  `default_color` is the
/// column's plain text colour (a `COLORREF`); runs without their own colour use it.
///
/// The baseline is the one `DT_VCENTER` gives the unstyled body font, so styling a title
/// never moves it up or down relative to an unstyled row.
pub unsafe fn draw(
    dc: HDC,
    runs: &[Run],
    rect: RECT,
    default_color: u32,
    scale: f64,
    fonts: &mut FontCache,
) {
    if runs.is_empty() || rect.right <= rect.left {
        return;
    }
    let body = fonts.face(dc, &Style::overlay_body(), scale);
    let baseline = rect.top + (rect.bottom - rect.top + body.ascent - body.descent) / 2;
    let unit = scale.round().max(1.0) as i32;
    let mut x = rect.left;
    for run in runs {
        if run.text.is_empty() || x >= rect.right {
            continue;
        }
        let face = fonts.face(dc, &run.style, scale);
        let width = fonts.advance(dc, &run.text, &run.style, scale);
        if width <= 0 {
            continue;
        }
        let colour = foreground(run.style.fg, run.style.fg_alpha, default_color);
        if let Some(background) = run.style.bg {
            let background = composite(background, run.style.bg_alpha);
            let brush = CreateSolidBrush(COLORREF(background.colorref()));
            let behind = RECT {
                left: x,
                top: rect.top,
                right: (x + width).min(rect.right),
                bottom: rect.bottom,
            };
            FillRect(dc, &behind, brush);
            let _ = DeleteObject(brush);
        }
        let old = SelectObject(dc, HGDIOBJ(face.font as *mut _));
        let _ = SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(colour));
        let _ = SetTextCharacterExtra(dc, letter_spacing_px(&run.style, scale));
        let clip = RECT {
            left: x,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        let rise = points_to_px(run.style.rise, scale).round() as i32;
        let wide: Vec<u16> = run.text.encode_utf16().collect();
        let _ = ExtTextOutW(
            dc,
            x,
            baseline - face.ascent - rise,
            ETO_CLIPPED,
            Some(&clip),
            PCWSTR(wide.as_ptr()),
            wide.len() as u32,
            None,
        );
        SelectObject(dc, old);
        let _ = SetTextCharacterExtra(dc, 0);
        decorate(dc, &run.style, x, x + width, baseline, face, colour, unit);
        x += width;
    }
}

/// Draw a solid rectangle (an underline or strikethrough stroke).
unsafe fn stroke(dc: HDC, left: i32, right: i32, top: i32, thickness: i32, colour: u32) {
    if right <= left {
        return;
    }
    let brush = CreateSolidBrush(COLORREF(colour));
    let rect = RECT {
        left,
        top,
        right,
        bottom: top + thickness.max(1),
    };
    FillRect(dc, &rect, brush);
    let _ = DeleteObject(brush);
}

#[allow(clippy::too_many_arguments)]
unsafe fn decorate(
    dc: HDC,
    style: &Style,
    left: i32,
    right: i32,
    baseline: i32,
    face: Face,
    colour: u32,
    unit: i32,
) {
    let decoration = |own: Option<pango_markup::Rgba>| {
        own.map(|c| composite(c, None).colorref()).unwrap_or(colour)
    };
    if let Some(underline) = style.underline {
        let underline_colour = decoration(style.underline_color);
        match underline {
            // Pango's single underline sits one pixel under the baseline; `low` sits
            // further down, `double` is two strokes and `error` is its red/wavy variant
            // (GDI draws it as a straight stroke in the same colour).
            Underline::Single => stroke(dc, left, right, baseline + unit, unit, underline_colour),
            Underline::Low => stroke(dc, left, right, baseline + 3 * unit, unit, underline_colour),
            Underline::Double => {
                stroke(dc, left, right, baseline + unit, unit, underline_colour);
                stroke(dc, left, right, baseline + 4 * unit, unit, underline_colour);
            }
            Underline::Error => {
                stroke(dc, left, right, baseline + 2 * unit, unit, underline_colour)
            }
        }
    }
    if style.strikethrough {
        let strike_colour = decoration(style.strikethrough_color);
        let middle = baseline - (face.ascent * 2 / 5).max(unit);
        stroke(dc, left, right, middle, unit, strike_colour);
    }
}

/// The colour a run paints in: its own, or the column's default, with the alpha attribute
/// composited against the sheet.
fn foreground(fg: Option<pango_markup::Rgba>, alpha: Option<u16>, default: u32) -> u32 {
    let base = fg.unwrap_or_else(|| pango_markup::Rgba {
        r: (default & 0xff) as u8,
        g: ((default >> 8) & 0xff) as u8,
        b: ((default >> 16) & 0xff) as u8,
        a: 0xff,
    });
    composite(base, alpha).colorref()
}

/// Blend a colour with the sheet background (`alpha`, 0..=65535, overrides the colour's
/// own alpha).  GDI text has no alpha channel, so this is composited here.
fn composite(colour: pango_markup::Rgba, alpha: Option<u16>) -> pango_markup::Rgba {
    let alpha = alpha
        .map(|a| (a as u32 * 255 / 65535) as u8)
        .unwrap_or(colour.a);
    if alpha == 0xff {
        return colour;
    }
    let backdrop = SHEET_BACKGROUND;
    let mix = |front: u8, back: u8| {
        ((front as u32 * alpha as u32 + back as u32 * (255 - alpha as u32)) / 255) as u8
    };
    pango_markup::Rgba {
        r: mix(colour.r, backdrop.r),
        g: mix(colour.g, backdrop.g),
        b: mix(colour.b, backdrop.b),
        a: 0xff,
    }
}

/// Where the description column starts inside one sheet column: the widest chord on the
/// page plus a gap, bounded to 25%..45% of the column width so a long chord cannot
/// swallow the description and a short one does not leave a hole.
pub fn chord_split(column_x: i32, column_width: i32, widest_chord: i32, gap: i32) -> i32 {
    let min = column_width * 25 / 100;
    let max = column_width * 45 / 100;
    column_x + (widest_chord + gap).clamp(min, max)
}

/// The sheet's title font: 14.25 pt (19 px at 96 dpi) semibold.
pub fn heading_style() -> Style {
    Style {
        size: 14_592,
        weight: 600,
        ..Style::overlay_body()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::pango_markup::{fit_runs, runs_width};

    #[test]
    fn the_chord_column_cannot_swallow_the_description() {
        let x = 20;
        let width = 800;
        // A short chord keeps the 25% floor.
        assert_eq!(chord_split(x, width, 40, 12), x + 200);
        // A chord that needs more room gets it.
        assert_eq!(chord_split(x, width, 300, 12), x + 312);
        // A very long chord stops at the 45% ceiling.
        assert_eq!(chord_split(x, width, 700, 12), x + 360);
        // Negative monitor origins and degenerate widths do not panic.
        assert_eq!(chord_split(-1366, 0, 0, 12), -1366);
        assert_eq!(chord_split(-1366, 0, 999, 12), -1366);
    }

    #[test]
    fn the_measurer_and_the_fit_agree_on_styled_runs() {
        // A fake measurer standing in for the DC: bold is wider than regular, which is
        // exactly what the painted pixels would show.
        struct Fake;
        impl TextMeasurer for Fake {
            fn advance(&mut self, text: &str, style: &Style) -> i32 {
                let per_char = if style.weight >= pango_markup::WEIGHT_BOLD {
                    10
                } else {
                    6
                };
                text.chars().count() as i32 * per_char
            }
        }
        let style = Style::overlay_body();
        let bold = Style {
            weight: pango_markup::WEIGHT_BOLD,
            ..style.clone()
        };
        let runs = vec![
            Run {
                text: "Bold".into(),
                style: bold,
            },
            Run {
                text: "Plain".into(),
                style,
            },
        ];
        let mut fake = Fake;
        assert_eq!(runs_width(&runs, &mut fake), 40 + 30);
        // 46 px fits the bold run (40) and the ellipsis (6) but not a plain character.
        let fitted = fit_runs(&runs, 46, &mut fake);
        assert_eq!(pango_markup::runs_text(&fitted), "Bold\u{2026}");
        let mut fake = Fake;
        assert!(runs_width(&fitted, &mut fake) <= 46);
    }

    #[test]
    fn alpha_and_default_colours_composite_to_the_sheet() {
        // No colour and no alpha: the column's own colour, unchanged.
        assert_eq!(foreground(None, None, 0x00ee_eeee), 0x00ee_eeee);
        // A half-transparent red over the dark sheet is a dark red, not black.
        let half = foreground(
            Some(pango_markup::Rgba::rgb(255, 0, 0)),
            Some(32_768),
            0x00ee_eeee,
        );
        let r = half & 0xff;
        let g = (half >> 8) & 0xff;
        assert!(r > 100 && r < 160, "red was not halved: {half:#x}");
        assert!(g < 20);
        // Fully transparent paints the sheet background.
        let sheet = SHEET_BACKGROUND.colorref();
        assert_eq!(
            foreground(Some(pango_markup::Rgba::rgb(255, 0, 0)), Some(0), 0),
            sheet
        );
        // Opaque colours are untouched.
        assert_eq!(
            composite(pango_markup::Rgba::rgb(1, 2, 3), None).colorref(),
            0x0003_0201
        );
    }

    #[test]
    fn a_heading_is_larger_and_heavier_than_the_body() {
        let body = Style::overlay_body();
        let heading = heading_style();
        assert!(heading.size > body.size);
        assert!(heading.weight > body.weight);
        // 19 px at 96 dpi, the size the sheet always used for its title.
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(near(points_to_px(heading.size, 1.0), 19.0));
        assert!(near(points_to_px(body.size, 1.0), 13.0));
        assert!(near(points_to_px(body.size, 1.5), 19.5));
    }

    #[test]
    fn an_unused_cache_creates_and_deletes_no_fonts() {
        // No window is created here: an unused showing holds no GDI handle, so dropping
        // it cannot leak one.  (Font creation needs a real DC and is covered by the live
        // overlay gate.)
        let cache = FontCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        drop(cache);
    }
}
