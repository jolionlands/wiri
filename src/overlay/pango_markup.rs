//! Pango markup for configured `hotkey-overlay-title` text.
//!
//! niri hands a bind's `hotkey-overlay-title` to Pango's `set_markup`
//! (`niri-config/src/binds.rs:873-875` for the option itself; the title is painted by
//! `src/ui/hotkey_overlay.rs`, and the wiki page Configuration:-Key-Bindings,
//! section "Custom Hotkey Overlay Titles", documents the markup).  So a title is markup,
//! not text: `<b>Browser</b>` paints a bold *Browser*, `R&amp;D` paints `R&D`, and an
//! **invalid** title is not partially styled — Pango falls back to the whole original
//! string, tags and all, as plain text.  A bare `&` (or a bare `<`) is a markup error,
//! not a literal character, which is why a literal ampersand has to be written `&amp;`.
//!
//! This module is the pure half of the implementation: it parses a title into styled
//! [`Run`]s and provides the measurement helpers the painter uses, so measuring and
//! painting cannot disagree.  GDI is the rendering half (`overlay::markup_text`).
//!
//! # What a GDI renderer can honour
//!
//! Honoured: `b`, `big`, `i`, `s`, `small`, `sub`, `sup`, `tt`, `u`, and on `<span>`
//! `font`/`font_desc`, `font_family`/`face`, `size`, `style`, `weight`, `variant`,
//! `stretch`, `foreground`/`fgcolor`/`color`, `background`/`bgcolor`, `alpha`,
//! `fgalpha`, `bgalpha`, `underline`, `underline_color`, `strikethrough`,
//! `strikethrough_color`, `rise` and `letter_spacing`.
//!
//! Accepted but not rendered (no GDI equivalent; documented rather than rejected):
//! `small_caps` (`variant`), `stretch` (glyph scaling), `font_features`, `fallback`,
//! and `lang`/`gravity` (which only choose glyphs/hinting in Pango).  They still take
//! part in style comparison, so two runs that differ only there are drawn separately —
//! the pixels are identical, the parser is not lying about what the title asked for.
//!
//! Pango version differences: attributes newer than the pinned Pango (for example
//! `text_transform`, `baseline_shift`, `font_scale`, `insert_hyphens`, `show`) are
//! *unknown attributes* here and therefore a markup error, exactly like any other
//! unknown attribute.  Colour names are a practical subset of the X11 list.

/// Pango's fixed-point unit for point sizes (`PANGO_SCALE`): sizes are stored in 1024ths
/// of a point, so `9.75pt` is `9984`.
pub const PANGO_SCALE: i32 = 1024;

/// Pango weights (`PANGO_WEIGHT_*`).
pub const WEIGHT_NORMAL: u16 = 400;
pub const WEIGHT_BOLD: u16 = 700;

/// `PANGO_SCALE_SMALL`: `<small>`, `size="smaller"` and `size="small"` scale by this.
pub const SCALE_SMALL: f64 = 5.0 / 6.0;
/// `PANGO_SCALE_LARGE`: `<big>`, `size="larger"` and `size="large"` scale by this.
pub const SCALE_LARGE: f64 = 1.2;
/// `PANGO_SCALE_X_SMALL`.
pub const SCALE_X_SMALL: f64 = 25.0 / 36.0; // 0.6944…
/// `PANGO_SCALE_XX_SMALL`.
pub const SCALE_XX_SMALL: f64 = 0.578_703_7;
/// `PANGO_SCALE_X_LARGE`.
pub const SCALE_X_LARGE: f64 = 1.44;
/// `PANGO_SCALE_XX_LARGE`.
pub const SCALE_XX_LARGE: f64 = 1.728;

/// The largest size the sheet can be asked for, in [`PANGO_SCALE`]ths of a point: 1000 pt
/// (1333 px at 96 dpi).  A markup title may name any number, and an unbounded one makes both
/// the pixel arithmetic and GDI's font creation impossible.
pub const MAX_SIZE: i32 = 1000 * PANGO_SCALE;
/// A size is never zero or negative: a font of height zero is not a font.
pub const MIN_SIZE: i32 = 1;
/// The largest `rise` or `letter_spacing`, in [`PANGO_SCALE`]ths of a point: 1000 pt of extra
/// spacing per character, far past anything a layout can use.
pub const MAX_OFFSET: i32 = 1000 * PANGO_SCALE;

/// A size, kept inside the range a title can sensibly ask for.
pub fn clamp_size(size: i32) -> i32 {
    size.clamp(MIN_SIZE, MAX_SIZE)
}

/// A `rise` or `letter_spacing`, kept inside the range the measurer and the painter can
/// multiply by a title's length without overflowing.
pub fn clamp_offset(units: i32) -> i32 {
    units.clamp(-MAX_OFFSET, MAX_OFFSET)
}

/// The concrete GDI face a Pango *generic* monospace family resolves to on Windows
/// (Pango would ask fontconfig for `Monospace`).
pub const MONOSPACE_FACE: &str = "Consolas";

/// A colour as Pango markup writes it: eight bits per channel, alpha last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 0xff }
    }
    /// The `COLORREF` GDI wants for this colour (alpha is composited separately).
    pub fn colorref(self) -> u32 {
        (self.r as u32) | ((self.g as u32) << 8) | ((self.b as u32) << 16)
    }
}

/// The `underline` attribute and the `<u>` tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Underline {
    Single,
    Double,
    Low,
    Error,
}

/// The resolved style of one run of a title.
///
/// Every field is resolved to concrete values while parsing, so painting never has to
/// walk a stack of attributes again: a run is painted exactly as measured.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Style {
    /// GDI face name; `None` is the sheet's default (Segoe UI).
    pub face: Option<String>,
    /// Absolute size in [`PANGO_SCALE`]ths of a point.
    pub size: i32,
    pub weight: u16,
    pub italic: bool,
    /// `variant="small-caps"`: accepted, not rendered by GDI (documented above).
    pub small_caps: bool,
    /// `PANGO_STRETCH_*` (0 = ultra-condensed, 4 = normal, 8 = ultra-expanded);
    /// accepted, GDI does not scale glyph widths for it.
    pub stretch: u8,
    pub fg: Option<Rgba>,
    pub bg: Option<Rgba>,
    /// `fgalpha`/`alpha`, 0..=65535, applied to the foreground colour.
    pub fg_alpha: Option<u16>,
    /// `bgalpha`/`alpha`, 0..=65535, applied to the background colour.
    pub bg_alpha: Option<u16>,
    pub underline: Option<Underline>,
    pub underline_color: Option<Rgba>,
    pub strikethrough: bool,
    pub strikethrough_color: Option<Rgba>,
    /// Baseline displacement in [`PANGO_SCALE`]ths of a point, positive is up.
    pub rise: i32,
    /// Extra spacing per character in [`PANGO_SCALE`]ths of a point.
    pub letter_spacing: i32,
    /// `font_features`: accepted, not rendered.
    pub features: Option<String>,
    /// `fallback`: accepted, not rendered.
    pub fallback: Option<String>,
    /// `lang`: accepted, not rendered.
    pub lang: Option<String>,
    /// `gravity`: accepted, not rendered.
    pub gravity: Option<String>,
}

impl Style {
    /// The hotkey sheet's body text: Segoe UI, 9.75 pt (13 px at 96 dpi), regular.
    /// `big`/`small`/`size="large"` resolve against this, and a title with no styling is
    /// exactly this style, which is what lets the painter keep plain titles on the old
    /// `DrawTextW` path.
    pub fn overlay_body() -> Self {
        Self {
            face: None,
            size: 9_984, // 9.75 pt
            weight: WEIGHT_NORMAL,
            italic: false,
            small_caps: false,
            stretch: 4,
            fg: None,
            bg: None,
            fg_alpha: None,
            bg_alpha: None,
            underline: None,
            underline_color: None,
            strikethrough: false,
            strikethrough_color: None,
            rise: 0,
            letter_spacing: 0,
            features: None,
            fallback: None,
            lang: None,
            gravity: None,
        }
    }
}

/// One contiguous piece of a title with a single style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

/// A markup error, worded like Pango's (`Error on line 1 char 4: …`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkupError {
    pub line: usize,
    pub ch: usize,
    pub message: String,
}

impl std::fmt::Display for MarkupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Error on line {} char {}: {}",
            self.line, self.ch, self.message
        )
    }
}

/// The result of interpreting a title as markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub runs: Vec<Run>,
    /// The title contained at least one tag or entity, so the runs are not the raw text.
    pub markup: bool,
    /// The markup was invalid; `runs` then holds the whole original string as plain text.
    pub error: Option<MarkupError>,
}

impl Parsed {
    /// The text as it paints (concatenated runs), for tests and log messages.
    pub fn text(&self) -> String {
        runs_text(&self.runs)
    }
}

/// The concatenated text of runs.
pub fn runs_text(runs: &[Run]) -> String {
    let mut s = String::new();
    for run in runs {
        s.push_str(&run.text);
    }
    s
}

/// Interpret `markup` as Pango markup, falling back to the whole original string when the
/// markup is invalid (Pango's behaviour: `pango_layout_set_markup` warns and then sets the
/// text to the original markup string).  `base` is the style of the un-styled text.
pub fn title(markup: &str, base: &Style) -> Parsed {
    match parse(markup, base) {
        Ok(runs) => Parsed {
            runs,
            markup: markup.contains('<') || markup.contains('&'),
            error: None,
        },
        Err(error) => Parsed {
            runs: vec![Run {
                text: markup.to_string(),
                style: base.clone(),
            }],
            markup: false,
            error: Some(error),
        },
    }
}

/// Parse Pango markup into styled runs, or say why it is not markup.
pub fn parse(src: &str, base: &Style) -> Result<Vec<Run>, MarkupError> {
    let mut p = Parser { src, pos: 0 };
    let mut runs: Vec<Run> = Vec::new();
    let mut text = String::new();
    let mut style = base.clone();
    let mut stack: Vec<(String, Style)> = Vec::new();

    while let Some(c) = p.peek() {
        match c {
            '<' => {
                flush(&mut runs, &mut text, &style);
                let tag = p.tag()?;
                if tag.closing {
                    match stack.pop() {
                        Some((open, parent)) if open == tag.name => style = parent,
                        Some((open, _)) => {
                            return Err(p.err(format!(
                                "element <{}> was closed, but the currently open element is <{}>",
                                tag.name, open
                            )))
                        }
                        None => {
                            return Err(p.err(format!("unexpected closing tag </{}>", tag.name)))
                        }
                    }
                } else {
                    let inner = open_tag(&tag, &style).map_err(|m| p.err(m))?;
                    if !tag.empty {
                        stack.push((tag.name.clone(), style.clone()));
                        style = inner;
                    }
                }
            }
            '&' => {
                p.bump();
                let c = p.entity()?;
                text.push(c);
            }
            _ => {
                p.bump();
                text.push(c);
            }
        }
    }
    flush(&mut runs, &mut text, &style);
    if let Some((open, _)) = stack.pop() {
        return Err(p.err(format!(
            "document ended unexpectedly with <{}> still open",
            open
        )));
    }
    Ok(runs)
}

fn flush(runs: &mut Vec<Run>, text: &mut String, style: &Style) {
    if text.is_empty() {
        return;
    }
    // Adjacent text of one style is one run, so a title that only changes style in the
    // middle stays two runs and a title with an empty tag or a re-opened tag stays one.
    if let Some(last) = runs.last_mut() {
        if last.style == *style {
            last.text.push_str(text);
            text.clear();
            return;
        }
    }
    runs.push(Run {
        text: std::mem::take(text),
        style: style.clone(),
    });
}

// ---------------------------------------------------------------------------------------
// measurement: the painter and the layout use these, so they cannot disagree
// ---------------------------------------------------------------------------------------

/// One run's advance in physical pixels, with that run's own font selected.  The GDI
/// implementation lives in `overlay::markup_text`; tests use a recording fake.
pub trait TextMeasurer {
    fn advance(&mut self, text: &str, style: &Style) -> i32;
}

/// The width of `runs`, measured run by run with each run's own style.  The sum saturates:
/// a title is arbitrary text and no wrapped width may turn into a negative one.
pub fn runs_width(runs: &[Run], m: &mut dyn TextMeasurer) -> i32 {
    runs.iter()
        .map(|run| m.advance(&run.text, &run.style))
        .fold(0, i32::saturating_add)
}

/// The runs to paint in `width` pixels: all of them when they fit, otherwise the prefix
/// that fits plus Pango's ellipsis (U+2026) in the last run's style.  Never splits a
/// character and never returns a run wider than `width`.
pub fn fit_runs(runs: &[Run], width: i32, m: &mut dyn TextMeasurer) -> Vec<Run> {
    if width <= 0 {
        return Vec::new();
    }
    if runs_width(runs, m) <= width {
        return runs.to_vec();
    }
    let ellipsis = '\u{2026}';
    let mut out: Vec<Run> = Vec::new();
    let mut used = 0;
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        let whole = m.advance(&run.text, &run.style);
        if used + whole <= width {
            out.push(run.clone());
            used += whole;
            continue;
        }
        let mut piece = String::new();
        for ch in run.text.chars() {
            let mut candidate = piece.clone();
            candidate.push(ch);
            if used + m.advance(&candidate, &run.style) > width {
                break;
            }
            piece = candidate;
        }
        let dot = ellipsis.to_string();
        let dot_width = m.advance(&dot, &run.style);
        while !piece.is_empty() && used + m.advance(&piece, &run.style) + dot_width > width {
            piece.pop();
        }
        let room = used + m.advance(&piece, &run.style) + dot_width <= width;
        if room {
            if !piece.is_empty() {
                out.push(Run {
                    text: piece,
                    style: run.style.clone(),
                });
            }
            out.push(Run {
                text: dot,
                style: run.style.clone(),
            });
        }
        break;
    }
    out
}

// ---------------------------------------------------------------------------------------
// the parser
// ---------------------------------------------------------------------------------------

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

struct Tag {
    name: String,
    closing: bool,
    empty: bool,
    attrs: Vec<(String, String)>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.src[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.bump();
        }
    }

    fn err(&self, message: impl Into<String>) -> MarkupError {
        let (mut line, mut ch) = (1, 1);
        for c in self.src[..self.pos.min(self.src.len())].chars() {
            if c == '\n' {
                line += 1;
                ch = 1;
            } else {
                ch += 1;
            }
        }
        MarkupError {
            line,
            ch,
            message: message.into(),
        }
    }

    /// `&` has been consumed.  Pango's five named entities and `&#NNN;` / `&#xHH;`.
    fn entity(&mut self) -> Result<char, MarkupError> {
        let start = self.pos;
        loop {
            match self.peek() {
                None => return Err(self.err("entity did not end with a semicolon")),
                Some(';') => break,
                Some(c) if c.is_ascii_alphanumeric() || c == '#' => {
                    self.bump();
                }
                Some(_) => {
                    return Err(self.err(
                        "Entity did not end with a semicolon; most likely you used an ampersand \
                         character without intending to start an entity, escape ampersand as &amp;",
                    ))
                }
            }
        }
        let name = &self.src[start..self.pos];
        self.bump(); // ';'
        match name {
            "amp" => Ok('&'),
            "lt" => Ok('<'),
            "gt" => Ok('>'),
            "quot" => Ok('"'),
            "apos" => Ok('\''),
            _ => match name.strip_prefix('#') {
                Some(digits) => {
                    let hex = digits.strip_prefix(['x', 'X']);
                    let code = match hex {
                        Some(digits) => u32::from_str_radix(digits, 16).ok(),
                        None => digits.parse::<u32>().ok(),
                    };
                    match code.filter(|c| *c != 0).and_then(char::from_u32) {
                        Some(c) => Ok(c),
                        None => Err(self.err(format!("invalid character reference '&{name};'"))),
                    }
                }
                None => Err(self.err(format!("unknown entity '&{name};'"))),
            },
        }
    }

    fn name(&mut self) -> Result<String, MarkupError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':' {
                self.bump();
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.err("expected a name"));
        }
        Ok(self.src[start..self.pos].to_ascii_lowercase())
    }

    fn attribute(&mut self, tag: &str) -> Result<(String, String), MarkupError> {
        let name = self.name()?;
        self.skip_space();
        if !self.eat("=") {
            return Err(self.err(format!("attribute '{}' of <{}> has no value", name, tag)));
        }
        self.skip_space();
        let quote = match self.peek() {
            Some(q @ ('"' | '\'')) => {
                self.bump();
                q
            }
            _ => {
                return Err(self.err(format!(
                    "value of attribute '{}' of <{}> must be quoted",
                    name, tag
                )))
            }
        };
        let mut value = String::new();
        loop {
            match self.bump() {
                None => {
                    return Err(self.err(format!(
                        "value of attribute '{}' of <{}> is never closed",
                        name, tag
                    )))
                }
                Some(c) if c == quote => break,
                Some('&') => value.push(self.entity()?),
                Some(c) => value.push(c),
            }
        }
        Ok((name, value))
    }

    /// The parser is on `<`.
    fn tag(&mut self) -> Result<Tag, MarkupError> {
        self.bump();
        let closing = self.eat("/");
        let name = self.name()?;
        let mut attrs = Vec::new();
        let mut empty = false;
        loop {
            self.skip_space();
            match self.peek() {
                None => return Err(self.err(format!("<{}> is never closed", name))),
                Some('>') => {
                    self.bump();
                    break;
                }
                Some('/') => {
                    self.bump();
                    if !self.eat(">") {
                        return Err(self.err(format!("expected '>' after '/' in <{}>", name)));
                    }
                    empty = true;
                    break;
                }
                Some(q @ ('"' | '\'')) => {
                    let _ = q;
                    return Err(self.err(format!("attribute of <{}> has no name", name)));
                }
                Some(_) => attrs.push(self.attribute(&name)?),
            }
        }
        if closing && (!attrs.is_empty() || empty) {
            return Err(self.err(format!("closing tag </{}> takes no attributes", name)));
        }
        Ok(Tag {
            name,
            closing,
            empty,
            attrs,
        })
    }
}

fn open_tag(tag: &Tag, base: &Style) -> Result<Style, String> {
    const TAGS: &[&str] = &[
        "span", "b", "big", "i", "s", "small", "sub", "sup", "tt", "u",
    ];
    if !TAGS.contains(&tag.name.as_str()) {
        return Err(format!("unknown tag '{}'", tag.name));
    }
    let mut style = base.clone();
    for (name, value) in &tag.attrs {
        apply_attribute(&mut style, name, value)?;
    }
    match tag.name.as_str() {
        "b" => style.weight = WEIGHT_BOLD,
        "big" => style.size = scaled(style.size, SCALE_LARGE),
        "i" => style.italic = true,
        "s" => style.strikethrough = true,
        "small" => style.size = scaled(style.size, SCALE_SMALL),
        // Pango's markup parser can only emit absolute rises here (sizes are resolved
        // later), so `<sub>`/`<sup>` use fixed point offsets rather than a fraction of
        // whatever size is in force.
        "sub" => style.rise = -2 * PANGO_SCALE,
        "sup" => style.rise = 2 * PANGO_SCALE,
        "tt" => style.face = Some(MONOSPACE_FACE.to_string()),
        "u" => style.underline = Some(Underline::Single),
        _ => {}
    }
    Ok(style)
}

fn scaled(size: i32, factor: f64) -> i32 {
    // Nested `<big>` tags and percentages can ask for any size, so the clamp belongs here,
    // where the size is computed.
    clamp_size(((size as f64) * factor).round() as i32)
}

fn apply_attribute(style: &mut Style, name: &str, value: &str) -> Result<(), String> {
    match name {
        "font" | "font_desc" => apply_font_description(style, value),
        "font_family" | "face" => {
            style.face = resolve_family(value);
            Ok(())
        }
        "size" => {
            style.size = parse_size(style.size, value)?;
            Ok(())
        }
        "style" => {
            style.italic = match value.trim().to_ascii_lowercase().as_str() {
                "normal" => false,
                "italic" | "oblique" => true,
                other => return Err(format!("invalid style '{other}'")),
            };
            Ok(())
        }
        "weight" => {
            style.weight = parse_weight(value)?;
            Ok(())
        }
        "variant" => {
            style.small_caps = match value.trim().to_ascii_lowercase().as_str() {
                "normal" => false,
                "small-caps" | "small_caps" => true,
                other => return Err(format!("invalid variant '{other}'")),
            };
            Ok(())
        }
        "stretch" => {
            style.stretch = parse_stretch(value)?;
            Ok(())
        }
        "foreground" | "fgcolor" | "color" => {
            style.fg = Some(parse_color(value)?);
            Ok(())
        }
        "background" | "bgcolor" => {
            style.bg = Some(parse_color(value)?);
            Ok(())
        }
        "alpha" => {
            let alpha = parse_alpha(value)?;
            style.fg_alpha = Some(alpha);
            style.bg_alpha = Some(alpha);
            Ok(())
        }
        "fgalpha" => {
            style.fg_alpha = Some(parse_alpha(value)?);
            Ok(())
        }
        "bgalpha" => {
            style.bg_alpha = Some(parse_alpha(value)?);
            Ok(())
        }
        "underline" => {
            style.underline = match value.trim().to_ascii_lowercase().as_str() {
                "none" => None,
                "single" => Some(Underline::Single),
                "double" => Some(Underline::Double),
                "low" => Some(Underline::Low),
                "error" => Some(Underline::Error),
                other => return Err(format!("invalid underline '{other}'")),
            };
            Ok(())
        }
        "underline_color" => {
            style.underline_color = Some(parse_color(value)?);
            Ok(())
        }
        "strikethrough" => {
            style.strikethrough = parse_bool(value)?;
            Ok(())
        }
        "strikethrough_color" => {
            style.strikethrough_color = Some(parse_color(value)?);
            Ok(())
        }
        "rise" => {
            style.rise = parse_units(value)?;
            Ok(())
        }
        "letter_spacing" => {
            style.letter_spacing = parse_units(value)?;
            Ok(())
        }
        "font_features" => {
            style.features = Some(value.to_string());
            Ok(())
        }
        "fallback" => {
            style.fallback = Some(value.to_string());
            Ok(())
        }
        "lang" => {
            style.lang = Some(value.to_string());
            Ok(())
        }
        "gravity" => {
            style.gravity = Some(value.to_string());
            Ok(())
        }
        other => Err(format!("unknown attribute '{}'", other)),
    }
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        other => Err(format!("invalid boolean '{other}'")),
    }
}

/// `size`: absolute 1024ths of a point, `Npt`, `Npx`, `N%`, the HTML-style keywords and
/// `smaller`/`larger`.
///
/// The keywords are resolved against the sheet's default body size with Pango's
/// `PANGO_SCALE_*` factors (that is what makes them *absolute*: `medium` is the default
/// size no matter how deeply the span is nested).  `smaller`, `larger` and percentages are
/// relative to the size in force, as in Pango.
fn parse_size(current: i32, value: &str) -> Result<i32, String> {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    let default = Style::overlay_body().size;
    let keyword = match lower.as_str() {
        "xx-small" => Some(SCALE_XX_SMALL),
        "x-small" => Some(SCALE_X_SMALL),
        "small" => Some(SCALE_SMALL),
        "medium" => Some(1.0),
        "large" => Some(SCALE_LARGE),
        "x-large" => Some(SCALE_X_LARGE),
        "xx-large" => Some(SCALE_XX_LARGE),
        _ => None,
    };
    if let Some(factor) = keyword {
        return Ok(scaled(default, factor));
    }
    if lower == "smaller" {
        return Ok(scaled(current, SCALE_SMALL));
    }
    if lower == "larger" {
        return Ok(scaled(current, SCALE_LARGE));
    }
    let (number, suffix) = split_number(v).ok_or_else(|| format!("invalid size '{value}'"))?;
    match suffix.to_ascii_lowercase().as_str() {
        // A bare number is Pango units, i.e. 1024ths of a point: size="12288" is 12 pt.
        "" => Ok(clamp_size(number.round() as i32)),
        "pt" => Ok(clamp_size((number * PANGO_SCALE as f64).round() as i32)),
        // 96 dpi is the CSS reference: 1 px is 0.75 pt.
        "px" => Ok(clamp_size((number * 0.75 * PANGO_SCALE as f64).round() as i32)),
        "%" => Ok(scaled(current, number / 100.0)),
        other => Err(format!("invalid size unit '{other}'")),
    }
}

/// `rise` and `letter_spacing`: Pango units (1024ths of a point) or a `pt` value.
fn parse_units(value: &str) -> Result<i32, String> {
    let v = value.trim();
    let (number, suffix) = split_number(v).ok_or_else(|| format!("invalid value '{value}'"))?;
    let units = match suffix.to_ascii_lowercase().as_str() {
        "" => number.round() as i32,
        "pt" => (number * PANGO_SCALE as f64).round() as i32,
        other => return Err(format!("invalid unit '{other}'")),
    };
    // The measurer adds this once per character and the painter shifts the baseline by it:
    // a value from a title is clamped so neither can overflow.
    Ok(clamp_offset(units))
}

/// Split `12.5pt` into `(12.5, "pt")`.  A leading `+`/`-` is allowed.
fn split_number(value: &str) -> Option<(f64, &str)> {
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(end);
    let number: f64 = number.parse().ok()?;
    if !number.is_finite() {
        return None;
    }
    Some((number, suffix))
}

fn parse_weight(value: &str) -> Result<u16, String> {
    let lower = value.trim().to_ascii_lowercase();
    let weight = match lower.as_str() {
        "thin" => 100,
        "ultralight" | "ultra-light" | "extralight" | "extra-light" => 200,
        "light" => 300,
        "semilight" | "semi-light" | "demilight" | "demi-light" => 350,
        "book" => 380,
        "normal" | "regular" => 400,
        "medium" => 500,
        "semibold" | "semi-bold" | "demibold" | "demi-bold" => 600,
        "bold" => 700,
        "ultrabold" | "ultra-bold" | "extrabold" | "extra-bold" => 800,
        "heavy" | "black" => 900,
        "ultraheavy" | "ultra-heavy" => 1000,
        _ => {
            let n: u16 = lower
                .parse()
                .map_err(|_| format!("invalid weight '{value}'"))?;
            if !(1..=1000).contains(&n) {
                return Err(format!("weight '{value}' is outside 1..1000"));
            }
            n
        }
    };
    Ok(weight)
}

fn parse_stretch(value: &str) -> Result<u8, String> {
    let lower = value.trim().to_ascii_lowercase();
    let stretch = match lower.as_str() {
        "ultra-condensed" | "ultracondensed" => 0,
        "extra-condensed" | "extracondensed" => 1,
        "condensed" => 2,
        "semi-condensed" | "semicondensed" => 3,
        "normal" => 4,
        "semi-expanded" | "semiexpanded" => 5,
        "expanded" => 6,
        "extra-expanded" | "extraexpanded" => 7,
        "ultra-expanded" | "ultraexpanded" => 8,
        _ => {
            let n: u8 = lower
                .parse()
                .map_err(|_| format!("invalid stretch '{value}'"))?;
            if n > 8 {
                return Err(format!("stretch '{value}' is outside 0..8"));
            }
            n
        }
    };
    Ok(stretch)
}

/// `alpha`/`fgalpha`/`bgalpha`: a fraction (`0.5`) or a percentage (`50%`), 0..1.
fn parse_alpha(value: &str) -> Result<u16, String> {
    let v = value.trim();
    let (number, percent) = match v.strip_suffix('%') {
        Some(rest) => (rest.trim(), true),
        None => (v, false),
    };
    let number: f64 = number
        .parse()
        .map_err(|_| format!("invalid alpha '{value}'"))?;
    let fraction = if percent { number / 100.0 } else { number };
    if !(0.0..=1.0).contains(&fraction) {
        return Err(format!("alpha '{value}' is outside 0..1"));
    }
    Ok((fraction * 65535.0).round() as u16)
}

/// Resolve the first family of a comma-separated list.  Pango's generic families are
/// mapped to concrete Windows faces or to the sheet's default (`None`).
fn resolve_family(value: &str) -> Option<String> {
    let first = value.split(',').map(str::trim).find(|s| !s.is_empty())?;
    Some(match first.to_ascii_lowercase().as_str() {
        "sans" | "sans-serif" | "sans serif" | "system-ui" | "ui-sans-serif" | "default" => {
            return None
        }
        "monospace" | "mono" | "ui-monospace" => MONOSPACE_FACE.to_string(),
        "serif" | "ui-serif" => "Times New Roman".to_string(),
        _ => first.to_string(),
    })
}

/// A Pango font description: `FAMILY-LIST [STYLE-OPTIONS] [SIZE]`, e.g.
/// `Segoe UI Bold Italic 12`, `monospace 9`, `Arial, Helvetica 10.5px`.
///
/// Pango's own parser never fails on a font description (unknown words are simply not
/// understood), so this one does not either: an unparsable description leaves the style
/// unchanged.
fn apply_font_description(style: &mut Style, value: &str) -> Result<(), String> {
    let mut words: Vec<&str> = value.split_whitespace().collect();
    if words.is_empty() {
        return Ok(());
    }
    if let Some(last) = words.last() {
        if let Some(size) = parse_description_size(last) {
            style.size = size;
            words.pop();
        }
    }
    loop {
        let Some(last) = words.last() else { break };
        let lower = last.to_ascii_lowercase();
        if let Ok(weight) = parse_weight(&lower) {
            style.weight = weight;
            words.pop();
        } else if matches!(lower.as_str(), "italic" | "oblique") {
            style.italic = true;
            words.pop();
        } else if lower == "normal" {
            words.pop();
        } else if matches!(lower.as_str(), "small-caps" | "small_caps") {
            style.small_caps = true;
            words.pop();
        } else if let Ok(stretch) = parse_stretch(&lower) {
            style.stretch = stretch;
            words.pop();
        } else {
            break;
        }
    }
    let family = words.join(" ");
    if !family.is_empty() {
        style.face = resolve_family(&family);
    }
    Ok(())
}

/// The size word of a font description: bare numbers are points, `Npx` is pixels,
/// `Npt` is points.
fn parse_description_size(word: &str) -> Option<i32> {
    let (number, suffix) = split_number(word)?;
    match suffix.to_ascii_lowercase().as_str() {
        "" | "pt" => Some(clamp_size((number * PANGO_SCALE as f64).round() as i32)),
        "px" => Some(clamp_size((number * 0.75 * PANGO_SCALE as f64).round() as i32)),
        _ => None,
    }
}

/// A Pango colour: `#rgb`, `#rrggbb`, `#rrrrggggbbbb`, `rgb(...)`/`rgba(...)` (0..255 or
/// percentages) and a practical subset of the X11 names.
fn parse_color(value: &str) -> Result<Rgba, String> {
    let v = value.trim();
    if let Some(hex) = v.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()
            .ok_or_else(|| format!("invalid colour '{value}'"))?;
        return match digits.len() {
            3 => Ok(Rgba::rgb(digits[0] * 17, digits[1] * 17, digits[2] * 17)),
            6 => Ok(Rgba::rgb(
                digits[0] * 16 + digits[1],
                digits[2] * 16 + digits[3],
                digits[4] * 16 + digits[5],
            )),
            12 => Ok(Rgba::rgb(
                digits[0] * 16 + digits[1],
                digits[4] * 16 + digits[5],
                digits[8] * 16 + digits[9],
            )),
            _ => Err(format!("invalid colour '{value}'")),
        };
    }
    let lower = v.to_ascii_lowercase();
    for (prefix, alpha) in [("rgba(", true), ("rgb(", false)] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let Some(inner) = rest.strip_suffix(')') else {
                return Err(format!("invalid colour '{value}'"));
            };
            let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
            let wanted = if alpha { 4 } else { 3 };
            if parts.len() != wanted {
                return Err(format!("invalid colour '{value}'"));
            }
            let channel = |text: &str| -> Result<u8, String> {
                match text.strip_suffix('%') {
                    Some(p) => {
                        let n: f64 = p
                            .trim()
                            .parse()
                            .map_err(|_| format!("invalid colour '{value}'"))?;
                        if !(0.0..=100.0).contains(&n) {
                            return Err(format!("invalid colour '{value}'"));
                        }
                        Ok((n * 255.0 / 100.0).round() as u8)
                    }
                    None => {
                        let n: f64 = text
                            .parse()
                            .map_err(|_| format!("invalid colour '{value}'"))?;
                        if !(0.0..=255.0).contains(&n) {
                            return Err(format!("invalid colour '{value}'"));
                        }
                        Ok(n.round() as u8)
                    }
                }
            };
            let r = channel(parts[0])?;
            let g = channel(parts[1])?;
            let b = channel(parts[2])?;
            let a = if alpha {
                let n: f64 = parts[3]
                    .parse()
                    .map_err(|_| format!("invalid colour '{value}'"))?;
                if !(0.0..=1.0).contains(&n) {
                    return Err(format!("invalid colour '{value}'"));
                }
                (n * 255.0).round() as u8
            } else {
                0xff
            };
            return Ok(Rgba { r, g, b, a });
        }
    }
    named_color(&lower).ok_or_else(|| format!("unknown colour '{value}'"))
}

fn named_color(name: &str) -> Option<Rgba> {
    Some(match name {
        "aqua" | "cyan" => Rgba::rgb(0, 255, 255),
        "black" => Rgba::rgb(0, 0, 0),
        "blue" => Rgba::rgb(0, 0, 255),
        "brown" => Rgba::rgb(165, 42, 42),
        "fuchsia" | "magenta" => Rgba::rgb(255, 0, 255),
        "gold" => Rgba::rgb(255, 215, 0),
        "gray" | "grey" => Rgba::rgb(128, 128, 128),
        "darkgray" | "darkgrey" => Rgba::rgb(169, 169, 169),
        "lightgray" | "lightgrey" => Rgba::rgb(211, 211, 211),
        "green" => Rgba::rgb(0, 128, 0),
        "lime" => Rgba::rgb(0, 255, 0),
        "maroon" => Rgba::rgb(128, 0, 0),
        "navy" => Rgba::rgb(0, 0, 128),
        "olive" => Rgba::rgb(128, 128, 0),
        "orange" => Rgba::rgb(255, 165, 0),
        "pink" => Rgba::rgb(255, 192, 203),
        "purple" => Rgba::rgb(128, 0, 128),
        "red" => Rgba::rgb(255, 0, 0),
        "silver" => Rgba::rgb(192, 192, 192),
        "teal" => Rgba::rgb(0, 128, 128),
        "white" => Rgba::rgb(255, 255, 255),
        "yellow" => Rgba::rgb(255, 255, 0),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Style {
        Style::overlay_body()
    }

    fn styles(runs: &[Run]) -> Vec<Style> {
        runs.iter().map(|r| r.style.clone()).collect()
    }

    fn texts(runs: &[Run]) -> Vec<&str> {
        runs.iter().map(|r| r.text.as_str()).collect()
    }

    fn parsed(src: &str) -> Parsed {
        title(src, &body())
    }

    fn error(src: &str) -> MarkupError {
        parse(src, &body()).expect_err("this title must not be valid markup")
    }

    // Pinned niri e19df12 applies a title with Pango set_markup
    // (docs/wiki/Configuration:-Key-Bindings.md, "Custom Hotkey Overlay Titles";
    // src/ui/hotkey_overlay.rs; the option is niri-config/src/binds.rs:873-875).
    #[test]
    fn plain_text_is_one_unstyled_run() {
        let p = parsed("Spawn notepad.exe");
        assert_eq!(texts(&p.runs), ["Spawn notepad.exe"]);
        assert_eq!(p.runs[0].style, body());
        assert!(!p.markup);
        assert!(p.error.is_none());
        assert!(parsed("").runs.is_empty());
    }

    #[test]
    fn literal_ampersand_is_a_markup_error_and_the_whole_title_stays_literal() {
        // Pango's parser wants an entity: "R&D" is "Entity did not end with a semicolon".
        let p = parsed("<b>R&D</b>");
        assert_eq!(p.text(), "<b>R&D</b>");
        assert_eq!(p.runs.len(), 1);
        assert_eq!(p.runs[0].style, body());
        assert!(!p.markup);
        assert!(p.error.unwrap().message.contains("semicolon"));
        // ... and the escaped ampersand is markup that paints one '&'.
        let p = parsed("<b>R&amp;D</b>");
        assert_eq!(p.text(), "R&D");
        assert_eq!(p.runs[0].style.weight, WEIGHT_BOLD);
        assert!(p.markup);
    }

    #[test]
    fn every_entity_form_decodes() {
        let p = parsed("&amp;&lt;&gt;&quot;&apos;&#65;&#x42;&#x1F600;");
        assert_eq!(p.text(), "&<>\"'AB\u{1F600}");
        assert!(p.markup);
        // Numeric references also work inside attribute values.
        let p = parsed("<span font_family=\"A&#66;C\">x</span>");
        assert_eq!(p.runs[0].style.face.as_deref(), Some("ABC"));
    }

    #[test]
    fn malformed_entities_are_markup_errors() {
        assert!(error("&unknown;").message.contains("unknown entity"));
        assert!(error("a & b").message.contains("semicolon"));
        assert!(error("&#xZZ;").message.contains("character reference"));
        assert!(error("&#0;").message.contains("character reference"));
        assert!(error("&#x110000;").message.contains("character reference"));
        assert!(error("&amp").message.contains("semicolon"));
    }

    #[test]
    fn every_convenience_tag_sets_its_style() {
        let bold = parse("<b>x</b>", &body()).unwrap();
        assert_eq!(bold[0].style.weight, WEIGHT_BOLD);
        let italic = parse("<i>x</i>", &body()).unwrap();
        assert!(italic[0].style.italic);
        let strike = parse("<s>x</s>", &body()).unwrap();
        assert!(strike[0].style.strikethrough);
        let mono = parse("<tt>x</tt>", &body()).unwrap();
        assert_eq!(mono[0].style.face.as_deref(), Some(MONOSPACE_FACE));
        let underline = parse("<u>x</u>", &body()).unwrap();
        assert_eq!(underline[0].style.underline, Some(Underline::Single));
        let big = parse("<big>x</big>", &body()).unwrap();
        assert_eq!(big[0].style.size, scaled(body().size, SCALE_LARGE));
        let small = parse("<small>x</small>", &body()).unwrap();
        assert_eq!(small[0].style.size, scaled(body().size, SCALE_SMALL));
        let sub = parse("<sub>x</sub>", &body()).unwrap();
        assert_eq!(sub[0].style.rise, -2 * PANGO_SCALE);
        let sup = parse("<sup>x</sup>", &body()).unwrap();
        assert_eq!(sup[0].style.rise, 2 * PANGO_SCALE);
    }

    /// A title can name any number for these three; the measurer and the painter multiply
    /// them by the length of the text and shift the baseline by them, so they are clamped
    /// where they are parsed.
    #[test]
    fn an_unbounded_letter_spacing_rise_or_size_is_clamped() {
        let huge = "99999999999999999999";
        let style = |src: &str| parse(src, &body()).unwrap()[0].style.clone();
        assert_eq!(
            style(&format!("<span letter_spacing=\"{huge}\">x</span>")).letter_spacing,
            MAX_OFFSET
        );
        assert_eq!(
            style(&format!("<span letter_spacing=\"-{huge}\">x</span>")).letter_spacing,
            -MAX_OFFSET
        );
        assert_eq!(style(&format!("<span rise=\"{huge}\">x</span>")).rise, MAX_OFFSET);
        assert_eq!(style(&format!("<span size=\"{huge}\">x</span>")).size, MAX_SIZE);
        assert_eq!(style(&format!("<span size=\"-{huge}\">x</span>")).size, MIN_SIZE);
        // Growing a size (<big>, a percentage) stops at the maximum instead of overflowing.
        assert_eq!(scaled(i32::MAX, SCALE_LARGE), MAX_SIZE);
        assert_eq!(scaled(MAX_SIZE, SCALE_XX_LARGE), MAX_SIZE);
        assert_eq!(scaled(MIN_SIZE, SCALE_SMALL), MIN_SIZE);
        // What a real title asks for is untouched.
        let ordinary =
            style("<span letter_spacing=\"512\" rise=\"2048\" size=\"10pt\">x</span>");
        assert_eq!((ordinary.letter_spacing, ordinary.rise), (512, 2048));
        assert_eq!(ordinary.size, 10 * PANGO_SCALE);
    }

    #[test]
    fn nested_tags_compose_and_unwind() {
        let runs = parse("<b>a<i>b</i>c</b>d", &body()).unwrap();
        assert_eq!(texts(&runs), ["a", "b", "c", "d"]);
        assert!(runs[0].style.weight == WEIGHT_BOLD && !runs[0].style.italic);
        assert!(runs[1].style.weight == WEIGHT_BOLD && runs[1].style.italic);
        assert!(runs[2].style.weight == WEIGHT_BOLD && !runs[2].style.italic);
        assert_eq!(runs[3].style, body());
        // Adjacent text of one style is one run, so plain titles keep a single run.
        let runs = parse("a<b>b</b><i>c</i>", &body()).unwrap();
        assert_eq!(texts(&runs), ["a", "b", "c"]);
    }

    #[test]
    fn unknown_tags_attributes_and_unbalanced_markup_are_errors() {
        assert!(error("<blinkish>x</blinkish>")
            .message
            .contains("unknown tag 'blinkish'"));
        assert!(error("<span colour=\"#fff\">x</span>")
            .message
            .contains("unknown attribute 'colour'"));
        assert!(error("<b>x").message.contains("still open"));
        assert!(error("</b>").message.contains("unexpected closing tag"));
        assert!(error("<b><i>x</b>")
            .message
            .contains("currently open element"));
        assert!(error("<b class=x>y</b>").message.contains("quoted"));
        assert!(error("<b").message.contains("never closed"));
        assert!(error("<b/").message.contains("expected '>'"));
        assert!(error("< b>x</ b>").message.contains("expected a name"));
        assert!(error("<span font_family=\"x\">y</span")
            .message
            .contains("never closed"));
        assert!(error("<span font_family=\"x>y</span>")
            .message
            .contains("never closed"));
        assert!(error("</b class=\"x\">")
            .message
            .contains("takes no attributes"));
    }

    #[test]
    fn tag_and_attribute_names_are_case_insensitive() {
        let upper = parse("<SPAN FOREGROUND=\"#ff0000\">x</SPAN>", &body()).unwrap();
        let lower = parse("<span foreground=\"#ff0000\">x</span>", &body()).unwrap();
        assert_eq!(styles(&upper), styles(&lower));
        assert_eq!(upper[0].style.fg, Some(Rgba::rgb(255, 0, 0)));
        let italic = parse("<I>x</I>", &body()).unwrap();
        assert!(italic[0].style.italic);
        let mixed = parse("<SpAn CoLoR=\"#00ff00\">x</SpAn>", &body()).unwrap();
        assert_eq!(mixed[0].style.fg, Some(Rgba::rgb(0, 255, 0)));
    }

    #[test]
    fn size_accepts_units_percentages_and_keywords() {
        let size = |src: &str| parse(src, &body()).unwrap()[0].style.size;
        assert_eq!(size("<span size=\"12288\">x</span>"), 12 * PANGO_SCALE);
        assert_eq!(size("<span size=\"12pt\">x</span>"), 12 * PANGO_SCALE);
        // 12 px at the 96 dpi reference is 9 pt.
        assert_eq!(size("<span size=\"12px\">x</span>"), 9 * PANGO_SCALE);
        assert_eq!(
            size("<span size=\"50%\">x</span>"),
            scaled(body().size, 0.5)
        );
        assert_eq!(size("<span size=\"medium\">x</span>"), body().size);
        assert_eq!(
            size("<span size=\"xx-large\">x</span>"),
            scaled(body().size, SCALE_XX_LARGE)
        );
        // Keywords are absolute: nesting them in a bigger span does not compound.
        assert_eq!(
            size("<span size=\"xx-large\"><span size=\"small\">x</span></span>"),
            scaled(body().size, SCALE_SMALL)
        );
        // smaller/larger and percentages are relative and do compound.
        assert_eq!(
            size("<span size=\"larger\"><span size=\"larger\">x</span></span>"),
            scaled(scaled(body().size, SCALE_LARGE), SCALE_LARGE)
        );
        assert!(error("<span size=\"12em\">x</span>")
            .message
            .contains("invalid size unit"));
        assert!(error("<span size=\"big\">x</span>")
            .message
            .contains("invalid size"));
    }

    #[test]
    fn weight_style_variant_and_stretch_forms() {
        let weight = |src: &str| parse(src, &body()).unwrap()[0].style.weight;
        assert_eq!(weight("<span weight=\"bold\">x</span>"), 700);
        assert_eq!(weight("<span weight=\"heavy\">x</span>"), 900);
        assert_eq!(weight("<span weight=\"semi-light\">x</span>"), 350);
        assert_eq!(weight("<span weight=\"600\">x</span>"), 600);
        assert!(error("<span weight=\"0\">x</span>")
            .message
            .contains("1..1000"));
        assert!(error("<span weight=\"fat\">x</span>")
            .message
            .contains("weight"));
        let italic = |src: &str| parse(src, &body()).unwrap()[0].style.italic;
        assert!(italic("<span style=\"italic\">x</span>"));
        assert!(italic("<span style=\"oblique\">x</span>"));
        assert!(!italic("<span style=\"normal\">x</span>"));
        assert!(error("<span style=\"slanted\">x</span>")
            .message
            .contains("style"));
        let variant = |src: &str| parse(src, &body()).unwrap()[0].style.small_caps;
        assert!(variant("<span variant=\"small-caps\">x</span>"));
        assert!(!variant("<span variant=\"normal\">x</span>"));
        let stretch = |src: &str| parse(src, &body()).unwrap()[0].style.stretch;
        assert_eq!(stretch("<span stretch=\"condensed\">x</span>"), 2);
        assert_eq!(stretch("<span stretch=\"ultra-expanded\">x</span>"), 8);
        assert_eq!(stretch("<span stretch=\"4\">x</span>"), 4);
        assert!(error("<span stretch=\"9\">x</span>")
            .message
            .contains("0..8"));
    }

    #[test]
    fn colour_forms_and_alpha() {
        let fg = |src: &str| parse(src, &body()).unwrap()[0].style.fg.unwrap();
        assert_eq!(
            fg("<span foreground=\"#f00\">x</span>"),
            Rgba::rgb(255, 0, 0)
        );
        assert_eq!(
            fg("<span fgcolor=\"#00ff00\">x</span>"),
            Rgba::rgb(0, 255, 0)
        );
        assert_eq!(fg("<span color=\"#0000ff\">x</span>"), Rgba::rgb(0, 0, 255));
        assert_eq!(
            fg("<span foreground=\"#ffff00000000\">x</span>"),
            Rgba::rgb(255, 0, 0)
        );
        assert_eq!(
            fg("<span foreground=\"rgb(1,2,3)\">x</span>"),
            Rgba::rgb(1, 2, 3)
        );
        assert_eq!(
            fg("<span foreground=\"rgb(100%,0%,50%)\">x</span>"),
            Rgba::rgb(255, 0, 128)
        );
        assert_eq!(
            fg("<span foreground=\"rgba(10,20,30,0.5)\">x</span>"),
            Rgba {
                r: 10,
                g: 20,
                b: 30,
                a: 128
            }
        );
        assert_eq!(
            fg("<span foreground=\"orange\">x</span>"),
            Rgba::rgb(255, 165, 0)
        );
        let bg = parse("<span background=\"#010203\">x</span>", &body()).unwrap()[0]
            .style
            .bg
            .unwrap();
        assert_eq!(bg, Rgba::rgb(1, 2, 3));
        let alpha = parse("<span fgalpha=\"50%\">x</span>", &body()).unwrap()[0]
            .style
            .fg_alpha
            .unwrap();
        assert_eq!(alpha, 32_768);
        let both = parse("<span alpha=\"0\">x</span>", &body()).unwrap()[0]
            .style
            .clone();
        assert_eq!(both.fg_alpha, Some(0));
        assert_eq!(both.bg_alpha, Some(0));
        assert_eq!(
            parse("<span bgalpha=\"1\">x</span>", &body()).unwrap()[0]
                .style
                .bg_alpha,
            Some(65_535)
        );
        assert!(error("<span foreground=\"nonsense\">x</span>")
            .message
            .contains("unknown colour"));
        assert!(error("<span foreground=\"#12345\">x</span>")
            .message
            .contains("invalid colour"));
        assert!(error("<span alpha=\"2\">x</span>")
            .message
            .contains("outside 0..1"));
    }

    #[test]
    fn underline_strikethrough_rise_and_spacing() {
        let underline = |src: &str| parse(src, &body()).unwrap()[0].style.underline;
        assert_eq!(
            underline("<span underline=\"single\">x</span>"),
            Some(Underline::Single)
        );
        assert_eq!(
            underline("<span underline=\"double\">x</span>"),
            Some(Underline::Double)
        );
        assert_eq!(
            underline("<span underline=\"low\">x</span>"),
            Some(Underline::Low)
        );
        assert_eq!(
            underline("<span underline=\"error\">x</span>"),
            Some(Underline::Error)
        );
        assert_eq!(underline("<span underline=\"none\">x</span>"), None);
        assert!(error("<span underline=\"wavy\">x</span>")
            .message
            .contains("underline"));
        let style = parse(
            "<span underline_color=\"#ff8800\" strikethrough=\"true\" \
             strikethrough_color=\"#112233\" rise=\"2048\" letter_spacing=\"512\">x</span>",
            &body(),
        )
        .unwrap()[0]
            .style
            .clone();
        assert_eq!(style.underline_color, Some(Rgba::rgb(255, 136, 0)));
        assert!(style.strikethrough);
        assert_eq!(style.strikethrough_color, Some(Rgba::rgb(0x11, 0x22, 0x33)));
        assert_eq!(style.rise, 2048);
        assert_eq!(style.letter_spacing, 512);
        assert_eq!(
            parse("<span rise=\"2pt\">x</span>", &body()).unwrap()[0]
                .style
                .rise,
            2 * PANGO_SCALE
        );
        assert!(error("<span strikethrough=\"maybe\">x</span>")
            .message
            .contains("boolean"));
    }

    #[test]
    fn font_description_and_family_forms() {
        let style = |src: &str| parse(src, &body()).unwrap()[0].style.clone();
        let s = style("<span font_desc=\"Segoe UI Bold Italic 12\">x</span>");
        assert_eq!(s.face.as_deref(), Some("Segoe UI"));
        assert_eq!(s.weight, 700);
        assert!(s.italic);
        assert_eq!(s.size, 12 * PANGO_SCALE);
        let s = style("<span font=\"monospace 9\">x</span>");
        assert_eq!(s.face.as_deref(), Some(MONOSPACE_FACE));
        assert_eq!(s.size, 9 * PANGO_SCALE);
        let s = style("<span font_desc=\"Consolas 10px\">x</span>");
        assert_eq!(s.face.as_deref(), Some("Consolas"));
        assert_eq!(s.size, scaled(10 * PANGO_SCALE, 0.75));
        // A comma-separated list takes its first family; generic families are mapped.
        let s = style("<span font_family=\"Arial, Helvetica\">x</span>");
        assert_eq!(s.face.as_deref(), Some("Arial"));
        let s = style("<span font_family=\"sans-serif\">x</span>");
        assert_eq!(s.face, None);
        let s = style("<span face=\"serif\">x</span>");
        assert_eq!(s.face.as_deref(), Some("Times New Roman"));
        // Pango never fails on a font description: unknown words join the family list.
        let s = style("<span font_desc=\"Some Unknown Font\">x</span>");
        assert_eq!(s.face.as_deref(), Some("Some Unknown Font"));
        // Attributes accepted only so the title is not rejected.
        let s = style(
            "<span font_features=\"kern\" fallback=\"false\" lang=\"ja\" gravity=\"east\">x</span>",
        );
        assert_eq!(s.features.as_deref(), Some("kern"));
        assert_eq!(s.fallback.as_deref(), Some("false"));
        assert_eq!(s.lang.as_deref(), Some("ja"));
        assert_eq!(s.gravity.as_deref(), Some("east"));
    }

    #[test]
    fn attributes_are_accepted_on_convenience_tags_and_empty_tags_do_nothing() {
        let s = parse("<b weight=\"500\">x</b>", &body()).unwrap()[0]
            .style
            .clone();
        // The tag's own effect wins over an attribute of the same kind.
        assert_eq!(s.weight, WEIGHT_BOLD);
        let runs = parse("a<b/>b", &body()).unwrap();
        assert_eq!(texts(&runs), ["ab"]);
        assert_eq!(runs[0].style, body());
    }

    #[test]
    fn fallback_keeps_every_styled_part_literal() {
        for src in [
            "<b>a</b> & <i>b</i>",
            "<span foreground=\"#ff0000\">orange</span> &amp; <oops>",
            "<b>unclosed",
        ] {
            let p = parsed(src);
            assert_eq!(p.text(), src, "{src} must paint literally");
            assert_eq!(p.runs.len(), 1);
            assert_eq!(p.runs[0].style, body());
            assert!(!p.markup, "a fallback is plain text for painting");
            assert!(p.error.is_some());
        }
    }

    #[test]
    fn errors_report_pango_style_positions() {
        let e = error("ok\n<b>R&D</b>");
        assert_eq!(e.line, 2);
        assert!(e.to_string().starts_with("Error on line 2 char "), "{e}");
        assert!(e.to_string().contains("semicolon"));
    }

    // ---- measurement: the layout and the painter share these ----

    /// A fake DC: every style has its own advance per character, so a measurement that
    /// used the plain text (or the wrong style) gives a different width.
    struct FakeMeasurer {
        calls: Vec<(String, Style)>,
    }

    impl FakeMeasurer {
        fn new() -> Self {
            Self { calls: Vec::new() }
        }
    }

    impl TextMeasurer for FakeMeasurer {
        fn advance(&mut self, text: &str, style: &Style) -> i32 {
            self.calls.push((text.to_string(), style.clone()));
            let per_char = if style.weight >= WEIGHT_BOLD {
                10
            } else if style.italic {
                8
            } else {
                6
            };
            let size_scale = if style.size > Style::overlay_body().size {
                2
            } else {
                1
            };
            text.chars().count() as i32 * per_char * size_scale
        }
    }

    #[test]
    fn measurement_uses_each_runs_own_style() {
        let runs = parse("<b>aa</b>bb", &body()).unwrap();
        let mut m = FakeMeasurer::new();
        // bold "aa" = 20, plain "bb" = 12: styled runs, not 4 * 6 = 24.
        assert_eq!(runs_width(&runs, &mut m), 32);
        assert_eq!(m.calls.len(), 2);
        assert_eq!(m.calls[0].0, "aa");
        assert_eq!(m.calls[0].1.weight, WEIGHT_BOLD);
        assert_eq!(m.calls[1].1, body());
        // A plain title is measured in one call, like it is painted.
        let plain = parsed("abcdef");
        let mut m = FakeMeasurer::new();
        assert_eq!(runs_width(&plain.runs, &mut m), 36);
        assert_eq!(m.calls.len(), 1);
    }

    #[test]
    fn fitting_keeps_the_styled_runs_and_never_exceeds_the_width() {
        let runs = parse("<b>aaaa</b>bbbb", &body()).unwrap();
        // Everything fits: identical runs, no ellipsis.
        let mut m = FakeMeasurer::new();
        assert_eq!(fit_runs(&runs, 64, &mut m), runs);
        // 46 px fits the bold run (40) and the ellipsis, but no plain character: the
        // ellipsis takes the style of the run it truncates.
        let mut m = FakeMeasurer::new();
        let fitted = fit_runs(&runs, 46, &mut m);
        assert_eq!(texts(&fitted), ["aaaa", "\u{2026}"]);
        assert_eq!(fitted[1].style, body());
        // 25 px does not fit the bold run whole: two bold characters + ellipsis would be
        // 20 + 6 = 26 > 25, so one character and the ellipsis.
        let mut m = FakeMeasurer::new();
        let fitted = fit_runs(&runs, 25, &mut m);
        assert_eq!(texts(&fitted), ["a", "\u{2026}"]);
        assert_eq!(fitted[0].style.weight, WEIGHT_BOLD);
        assert_eq!(fitted[1].style.weight, WEIGHT_BOLD);
        let mut m = FakeMeasurer::new();
        assert!(runs_width(&fitted, &mut m) <= 25);
        // Nothing fits at all: no runs, so nothing paints past the column.
        let mut m = FakeMeasurer::new();
        assert!(fit_runs(&runs, 0, &mut m).is_empty());
        // A multi-byte character is never split.
        let unicode = parse("ééé", &body()).unwrap();
        let mut m = FakeMeasurer::new();
        let fitted = fit_runs(&unicode, 13, &mut m);
        assert_eq!(texts(&fitted), ["é", "\u{2026}"]);
    }

    #[test]
    fn an_outside_call_that_paints_the_same_runs_measures_the_same_width() {
        // The painter fits a label to the column and then draws exactly the returned
        // runs; measuring those again must give the width it planned for.
        let runs = parse(
            "<span foreground=\"#ff8800\">orange</span> and more",
            &body(),
        )
        .unwrap();
        let mut m = FakeMeasurer::new();
        let fitted = fit_runs(&runs, 60, &mut m);
        let mut m = FakeMeasurer::new();
        let again = runs_width(&fitted, &mut m);
        assert!(again <= 60, "{again} must fit in 60");
    }
}
