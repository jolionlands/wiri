//! What the overlay layer draws, as niri's `layout {}` describes it: `focus-ring`, `border`,
//! `shadow`, `insert-hint` and `tab-indicator`, with niri's colours (CSS names, `#rgb`,
//! `rgb()`, `hsl()`) and linear gradients.
//!
//! These are plain data.  `config/decode.rs` fills them, `layout/engine/decor.rs` turns them
//! into what the overlay layer paints.  Colours are kept as the text wiri stores everywhere
//! else in the config (`#rrggbb` or `#rrggbbaa`), parsed again where they are drawn.

/// A colour as the four straight (not premultiplied) channels.
pub type Rgba8 = [u8; 4];

/// niri's colour syntax: CSS names, `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`,
/// `hsl()` and `hsla()`.  Returns `None` for anything else.
pub fn parse_css_color(text: &str) -> Option<Rgba8> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(hex) = t.strip_prefix('#') {
        return parse_hex(hex);
    }
    let lower = t.to_ascii_lowercase();
    if let Some(open) = lower.find('(') {
        if !lower.ends_with(')') {
            return None;
        }
        let name = lower[..open].trim();
        let inner = &lower[open + 1..lower.len() - 1];
        return match name {
            "rgb" | "rgba" => parse_rgb_function(inner),
            "hsl" | "hsla" => parse_hsl_function(inner),
            _ => None,
        };
    }
    named_color(&lower)
}

fn parse_hex(hex: &str) -> Option<Rgba8> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    match hex.len() {
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255]),
        4 => Some([
            digit(0)? * 17,
            digit(1)? * 17,
            digit(2)? * 17,
            digit(3)? * 17,
        ]),
        6 => Some([pair(0)?, pair(2)?, pair(4)?, 255]),
        8 => Some([pair(0)?, pair(2)?, pair(4)?, pair(6)?]),
        _ => None,
    }
}

/// The numbers of a CSS colour function: separated by commas, spaces or a slash.
fn function_args(inner: &str) -> Vec<&str> {
    inner
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `a` as a channel: a number 0..=255, or a percentage.
fn channel(arg: &str) -> Option<u8> {
    if let Some(p) = arg.strip_suffix('%') {
        let v: f64 = p.trim().parse().ok()?;
        return Some((v.clamp(0.0, 100.0) * 255.0 / 100.0).round() as u8);
    }
    let v: f64 = arg.parse().ok()?;
    Some(v.clamp(0.0, 255.0).round() as u8)
}

/// An alpha: 0..=1, or a percentage.
fn alpha(arg: &str) -> Option<u8> {
    if let Some(p) = arg.strip_suffix('%') {
        let v: f64 = p.trim().parse().ok()?;
        return Some((v.clamp(0.0, 100.0) * 255.0 / 100.0).round() as u8);
    }
    let v: f64 = arg.parse().ok()?;
    Some((v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn parse_rgb_function(inner: &str) -> Option<Rgba8> {
    let args = function_args(inner);
    if args.len() != 3 && args.len() != 4 {
        return None;
    }
    let a = if args.len() == 4 {
        alpha(args[3])?
    } else {
        255
    };
    Some([channel(args[0])?, channel(args[1])?, channel(args[2])?, a])
}

fn parse_hsl_function(inner: &str) -> Option<Rgba8> {
    let args = function_args(inner);
    if args.len() != 3 && args.len() != 4 {
        return None;
    }
    let hue_text = args[0].trim_end_matches("deg");
    let hue: f64 = hue_text.parse().ok()?;
    let sat: f64 = args[1].strip_suffix('%')?.trim().parse::<f64>().ok()? / 100.0;
    let light: f64 = args[2].strip_suffix('%')?.trim().parse::<f64>().ok()? / 100.0;
    let a = if args.len() == 4 {
        alpha(args[3])?
    } else {
        255
    };
    let (r, g, b) = hsl_to_rgb(hue, sat.clamp(0.0, 1.0), light.clamp(0.0, 1.0));
    Some([r, g, b, a])
}

fn hsl_to_rgb(hue_deg: f64, sat: f64, light: f64) -> (u8, u8, u8) {
    let h = hue_deg.rem_euclid(360.0) / 360.0;
    if sat == 0.0 {
        let v = (light * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if light < 0.5 {
        light * (1.0 + sat)
    } else {
        light + sat - light * sat
    };
    let p = 2.0 * light - q;
    let channel = |mut t: f64| -> u8 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    (channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

/// The CSS named colours (lowercase), sorted for the binary search of `named_color`.
const CSS_NAMES: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

/// The CSS named colour `name` (lowercase).
fn named_color(name: &str) -> Option<Rgba8> {
    if name == "transparent" {
        return Some([0, 0, 0, 0]);
    }
    CSS_NAMES
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| {
            let rgb = CSS_NAMES[i].1;
            [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]
        })
}

/// The text wiri stores for a colour: `#rrggbb`, or `#rrggbbaa` when it is not opaque.
pub fn color_text(c: Rgba8) -> String {
    if c[3] == 255 {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
    }
}

/// The colour space a gradient is interpolated in.  Only `Srgb` is drawn exactly; the others
/// are read (so a niri config loads) and drawn as `Srgb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientSpace {
    Srgb,
    SrgbLinear,
    Oklab,
    Oklch,
}

/// niri's `active-gradient from="#80c8ff" to="#bbddff" angle=45 relative-to="workspace-view"`:
/// a CSS `linear-gradient(angle, from, to)`.
#[derive(Debug, Clone, PartialEq)]
pub struct GradientStyle {
    pub from: String,
    pub to: String,
    /// CSS angle in degrees: 0 points up, 90 right, 180 (the default) down.
    pub angle: f64,
    /// Measured against the whole view of the workspace instead of the window.
    pub relative_to_view: bool,
    pub space: GradientSpace,
}

/// A colour or a gradient.
#[derive(Debug, Clone, PartialEq)]
pub enum PaintStyle {
    Color(String),
    Gradient(GradientStyle),
}

impl PaintStyle {
    pub fn color(text: &str) -> Self {
        PaintStyle::Color(text.to_string())
    }
}

/// `focus-ring` and `border`: they have the same options.
#[derive(Debug, Clone, PartialEq)]
pub struct RingStyle {
    pub off: bool,
    /// Thickness in pixels.
    pub width: u32,
    pub active: PaintStyle,
    pub inactive: PaintStyle,
    pub urgent: PaintStyle,
}

impl RingStyle {
    /// The focus ring around the active window: on, 3 px, wiri's blue (niri: 4 px, #7fc8ff).
    pub fn focus_ring_default() -> Self {
        Self {
            off: false,
            width: 3,
            active: PaintStyle::color("#3381d9"),
            inactive: PaintStyle::color("#505050"),
            urgent: PaintStyle::color("#cc3344"),
        }
    }

    /// The border around every window: 4 px; wiri has always kept this space around a window.
    pub fn border_default() -> Self {
        Self {
            off: false,
            width: 4,
            active: PaintStyle::color("#3381d9"),
            inactive: PaintStyle::color("#4d4d4d"),
            urgent: PaintStyle::color("#cc3344"),
        }
    }
}

/// niri's `shadow {}`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowStyle {
    pub on: bool,
    /// Blur radius in pixels (CSS box-shadow): the gaussian's sigma is half of it.
    pub softness: f64,
    pub spread: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    /// Fill the shadow behind the window too (a translucent window shows it).
    pub draw_behind_window: bool,
    pub color: String,
    /// The colour for windows that are not active; `None`: `color` at three quarters.
    pub inactive_color: Option<String>,
}

impl Default for ShadowStyle {
    fn default() -> Self {
        Self {
            on: false,
            softness: 30.0,
            spread: 5.0,
            offset_x: 0.0,
            offset_y: 5.0,
            draw_behind_window: false,
            color: "#00000077".to_string(),
            inactive_color: None,
        }
    }
}

/// niri's `insert-hint {}`: the translucent rectangle that shows where a window dropped now
/// would go.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertHintStyle {
    pub off: bool,
    pub paint: PaintStyle,
}

impl Default for InsertHintStyle {
    fn default() -> Self {
        Self {
            off: false,
            paint: PaintStyle::color("#7fc8ff80"),
        }
    }
}

/// Where the tab indicator sits against its column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabPosition {
    Left,
    Right,
    Top,
    Bottom,
}

/// niri's `tab-indicator {}`.
#[derive(Debug, Clone, PartialEq)]
pub struct TabIndicatorStyle {
    pub off: bool,
    pub hide_when_single_tab: bool,
    pub place_within_column: bool,
    /// Between the indicator and the window; may be negative (on top of the window).
    pub gap: f64,
    /// Thickness of the indicator.
    pub width: f64,
    /// All tabs together, as a proportion of the side of the column they run along.
    pub length: f64,
    pub position: TabPosition,
    pub gaps_between_tabs: f64,
    pub corner_radius: f64,
    pub active: Option<PaintStyle>,
    pub inactive: Option<PaintStyle>,
    pub urgent: Option<PaintStyle>,
}

impl Default for TabIndicatorStyle {
    fn default() -> Self {
        Self {
            off: false,
            hide_when_single_tab: false,
            place_within_column: false,
            gap: 5.0,
            width: 4.0,
            length: 0.5,
            position: TabPosition::Left,
            gaps_between_tabs: 0.0,
            corner_radius: 0.0,
            active: None,
            inactive: None,
            urgent: None,
        }
    }
}

/// Everything the overlay layer can draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Effects {
    pub focus_ring: RingStyle,
    pub border: RingStyle,
    pub shadow: ShadowStyle,
    pub insert_hint: InsertHintStyle,
    pub tab_indicator: TabIndicatorStyle,
    /// The corner radius the user asks windows to have (`border { radius N }`).  On
    /// Windows 11 DWM knows three: square (0), small (4 px) and round (8 px); a radius is
    /// mapped to the nearest.  `None`: windows keep what they have.
    pub corner_radius: Option<u32>,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            focus_ring: RingStyle::focus_ring_default(),
            border: RingStyle::border_default(),
            shadow: ShadowStyle::default(),
            insert_hint: InsertHintStyle::default(),
            tab_indicator: TabIndicatorStyle::default(),
            corner_radius: None,
        }
    }
}

/// What the replacement of the blocks a per-output `layout {}` names (niri: 25.11).  A block
/// that is `Some` replaces the global one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectsPartial {
    pub focus_ring: Option<RingStyle>,
    pub border: Option<RingStyle>,
    pub shadow: Option<ShadowStyle>,
    pub insert_hint: Option<InsertHintStyle>,
    pub tab_indicator: Option<TabIndicatorStyle>,
    pub corner_radius: Option<u32>,
}

impl EffectsPartial {
    /// The effects of `base` with the blocks this partial names replaced.
    pub fn apply_to(&self, base: &Effects) -> Effects {
        let mut out = base.clone();
        if let Some(v) = &self.focus_ring {
            out.focus_ring = v.clone();
        }
        if let Some(v) = &self.border {
            out.border = v.clone();
        }
        if let Some(v) = &self.shadow {
            out.shadow = v.clone();
        }
        if let Some(v) = &self.insert_hint {
            out.insert_hint = v.clone();
        }
        if let Some(v) = &self.tab_indicator {
            out.tab_indicator = v.clone();
        }
        if let Some(v) = self.corner_radius {
            out.corner_radius = Some(v);
        }
        out
    }

    /// Start every block from what is in force now, so that a second `layout {}` block
    /// changes the keys it names and leaves the others.
    pub fn seeded_from(base: &Effects) -> Self {
        Self {
            focus_ring: Some(base.focus_ring.clone()),
            border: Some(base.border.clone()),
            shadow: Some(base.shadow.clone()),
            insert_hint: Some(base.insert_hint.clone()),
            tab_indicator: Some(base.tab_indicator.clone()),
            corner_radius: base.corner_radius,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colours_in_all_four_lengths() {
        assert_eq!(parse_css_color("#f80"), Some([255, 136, 0, 255]));
        assert_eq!(parse_css_color("#f808"), Some([255, 136, 0, 136]));
        assert_eq!(parse_css_color("#7fc8ff"), Some([127, 200, 255, 255]));
        assert_eq!(parse_css_color("#7fc8ff80"), Some([127, 200, 255, 128]));
        assert_eq!(parse_css_color("#7fc8f"), None);
        assert_eq!(parse_css_color("#gggggg"), None);
    }

    #[test]
    fn css_names_are_found_in_any_case() {
        assert_eq!(parse_css_color("red"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("Gray"), Some([128, 128, 128, 255]));
        assert_eq!(parse_css_color("rebeccapurple"), Some([102, 51, 153, 255]));
        assert_eq!(parse_css_color("transparent"), Some([0, 0, 0, 0]));
        assert_eq!(parse_css_color("not-a-colour"), None);
    }

    #[test]
    fn the_name_table_is_sorted_so_the_binary_search_finds_every_name() {
        // `named_color` binary-searches the table: an entry out of order would hide others.
        for pair in CSS_NAMES.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "{} is out of order before {}",
                pair[0].0,
                pair[1].0
            );
        }
        for (name, _) in CSS_NAMES {
            assert!(
                parse_css_color(name).is_some(),
                "{} should be a colour",
                name
            );
        }
    }

    #[test]
    fn rgb_and_hsl_functions() {
        assert_eq!(
            parse_css_color("rgb(255, 127, 0)"),
            Some([255, 127, 0, 255])
        );
        assert_eq!(
            parse_css_color("rgba(255, 127, 0, 0.5)"),
            Some([255, 127, 0, 128])
        );
        assert_eq!(
            parse_css_color("rgb(255 127 0 / 50%)"),
            Some([255, 127, 0, 128])
        );
        assert_eq!(parse_css_color("rgb(100%, 0%, 0%)"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("hsl(0, 100%, 50%)"), Some([255, 0, 0, 255]));
        assert_eq!(
            parse_css_color("hsl(120deg 100% 25%)"),
            Some([0, 128, 0, 255])
        );
        assert_eq!(
            parse_css_color("hsla(240, 100%, 50%, 1)"),
            Some([0, 0, 255, 255])
        );
        assert_eq!(parse_css_color("rgb(1, 2)"), None);
        assert_eq!(parse_css_color("hsl(0, 100, 50)"), None);
    }

    #[test]
    fn colour_text_is_what_the_config_stores() {
        assert_eq!(color_text([127, 200, 255, 255]), "#7fc8ff");
        assert_eq!(color_text([127, 200, 255, 128]), "#7fc8ff80");
    }

    #[test]
    fn a_partial_replaces_only_the_blocks_it_names() {
        let base = Effects::default();
        let mut ring = RingStyle::focus_ring_default();
        ring.width = 9;
        let partial = EffectsPartial {
            focus_ring: Some(ring),
            ..Default::default()
        };
        let out = partial.apply_to(&base);
        assert_eq!(out.focus_ring.width, 9);
        assert_eq!(out.border, base.border);
        assert_eq!(out.shadow, base.shadow);
    }

    #[test]
    fn the_defaults_are_the_ones_the_layout_always_had() {
        let e = Effects::default();
        assert!(!e.border.off && e.border.width == 4);
        assert!(!e.focus_ring.off && e.focus_ring.width == 3);
        assert!(!e.shadow.on);
        assert_eq!(e.tab_indicator.position, TabPosition::Left);
        assert_eq!(e.tab_indicator.length, 0.5);
        assert_eq!(e.insert_hint.paint, PaintStyle::color("#7fc8ff80"));
    }
}
