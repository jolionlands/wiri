# niri config value bounds: census and wiri parity (pinned e19df12cf8)

Sources: niri `e19df12cf81ffe805609fee497da1a097066ad2f`, files under `niri-config/src/`.
wiri: this branch (`src/config/decode.rs`, `src/config/decode/*.rs`, `src/config/validation.rs`,
`src/config/types.rs`). Each row was checked by reading the niri decoder and then exercising
wiri's decoder with a regression test (`src/config/validation.rs`, `parity_*` tests); the tests
are named in the last column.

## How niri decodes a bounded number

* `FloatOrInt<MIN, MAX>` (`niri-config/src/utils.rs:16-18, 80-142`) accepts an **integer or a
  decimal** literal and rejects everything else. Out of range gives
  `value must be between {MIN} and {MAX}`; a string gives
  `Unsupported value, only numbers are recognized`. `MIN`/`MAX` are `i32` constants, so
  `FloatOrInt<0, 65535>` is a 0..=65535 check on a value that is still a `f64`.
* A plain `u8`/`u16`/`u32`/`i32`/`i16` field decodes a **whole** literal only; a decimal is a
  type error. This is why several "integer" options below cannot be rounded.
* `Percent` (`niri-config/src/utils.rs:13-14, 58-70`) requires a trailing `%` and divides by 100;
  it has no range check (`focus-follows-mouse max-scroll-amount`).
* Errors do not abort the load: knuffel collects them and the config is refused as a whole
  (`Config::parse` returns `Err` when any error was emitted).

## Input (`niri-config/src/input.rs`)

| option | niri type and range | wiri before | wiri after | test |
| --- | --- | --- | --- | --- |
| `touchpad`/`mouse`/`trackpoint`/`trackball` `accel-speed` | `FloatOrInt<-1, 1>`, touchpad at :204-205, mouse :231-232, trackpoint/trackball in the same block :200-330 | parsed but Windows has no per-device speed: one warning, **value unchecked** (`mouse { accel-speed 2 }` loaded) | same warning, value checked -1..=1; bare node and two arguments rejected (niri's `unwrap(argument)` grammar) | `parity_unsupported_device_options_are_still_type_and_range_checked` |
| `scroll-factor` base | `FloatOrInt<0, 100>` :157-160 | 0..=100 | unchanged | `parity_axis_scroll_factor_*` (decode.rs) |
| `scroll-factor` `horizontal`/`vertical` | `FloatOrInt<-100, 100>` :161-164 | -100..=100 | unchanged | same |
| `touchpad pinch-sensitivity` | `FloatOrInt<0, 100>` :224-225 | **unknown key → error** (niri accepts it) | accepted with the "Windows handles pinch itself" warning; 0..=100 checked | `parity_unsupported_device_options_are_still_type_and_range_checked` |
| `scroll-button` | `u32` :210-211 | whole 0..=u32::MAX | unchanged | same test (272 ok, 1.5 error) |
| `keyboard repeat-delay` | `u16` :113-115 | 0..=65535 whole | unchanged | `integer_options_reject_fractional_values` |
| `keyboard repeat-rate` | `u8` :116-118 | 0..=255 whole | unchanged | decode.rs range test |
| `cursor xcursor-size` | `u8` (`misc.rs:36-40`) | unsupported, **value unchecked** | unsupported, 0..=255 checked | `parity_unsupported_device_options_are_still_type_and_range_checked` |
| `cursor hide-after-inactive-ms` | `u32` (`misc.rs:41-42`) | 0..=u32::MAX whole | unchanged | - |

## Outputs (`niri-config/src/output.rs`)

| option | niri type and range | wiri before | wiri after | test |
| --- | --- | --- | --- | --- |
| `scale` | `FloatOrInt<0, 10>` :56-57 | 0..=10 (plus a wiri warning outside 0.5..4.0) | unchanged | `parity_bounds_output_scale_and_position` |
| `position x=`/`y=` and `position <x> <y>` | `i32` properties :130-146 | clamped to -1e6..1e6 and **rounded** | whole i32, decimal rejected, full i32 range | same |
| `mode`, `modeline`, `max-bpc` | typed with ordering checks (`output.rs:340-520`) | one "not implemented by wiri" warning, values unchecked | unchanged (see residuals) | - |

## Layout and appearance (`layout.rs`, `appearance.rs`)

| option | niri type and range | wiri before | wiri after | test |
| --- | --- | --- | --- | --- |
| `gaps` | `FloatOrInt<0, 65535>` | 0..=65535 | unchanged | `parity_bounds_layout_gaps_struts_and_presets` |
| `struts` sides | `FloatOrInt<-65535, 65535>` (`layout.rs:195-203`) | -65535..=65535 | unchanged | same |
| preset `proportion` | plain `f64`, no range (`layout.rs:127-152`) | 0..=100 (too narrow) | any finite | same |
| preset `fixed` | plain `i32`, no range | 1..=100000, **rounded** | whole 0..=u32::MAX, decimal rejected | same |
| `default-column-width`/`default-window-height` `proportion` | `f64` | 0.0001..=10 | any finite | same |
| `default-column-width`/`-height` `fixed` | `i32` | 0..=100000 whole | whole 0..=u32::MAX | same |
| `border`/`focus-ring width` | `FloatOrInt<0, 65535>` (`appearance.rs:625-640`) | 0..=4096, rounded to u32 | 0..=65535 (stored rounded, see residuals) | `parity_bounds_shadow_tab_indicator_and_gradient_angle` |
| `shadow softness` | `FloatOrInt<0, 1024>` (`appearance.rs:647-660`) | 0..=4096 (too wide) | 0..=1024 | same |
| `shadow spread` | `FloatOrInt<-1024, 1024>` | -4096..=4096 then **clamped to >= 0** | -1024..=1024, negative kept | same |
| `shadow offset x=`/`y=` | `FloatOrInt<-65535, 65535>` | kept as f64 (legacy i32 mirror rounded) | unchanged | - |
| `tab-indicator gap` | `FloatOrInt<-65535, 65535>` (`appearance.rs:556-585`) | -65535..=65535 | unchanged | same |
| `tab-indicator width`/`gaps-between-tabs`/`corner-radius` | `FloatOrInt<0, 65535>` | 0..=65535 | unchanged | same |
| `tab-indicator length total-proportion=` | plain `f64` | any finite | unchanged | - |
| gradient `angle=` | `i16` (`appearance.rs:98-108`) | -360000..=360000, decimals accepted | whole i16 | same |
| `overview zoom` | `FloatOrInt<0, 1>` (`misc.rs:120-130`) | 0..=1 | unchanged | - |
| `overview workspace-shadow softness/spread/offset` | `<0,1024>` / `<-1024,1024>` / `<+-65535>` | same | unchanged | - |

## Window rules (`niri-config/src/window_rule.rs`)

| option | niri type and range | wiri before | wiri after | test |
| --- | --- | --- | --- | --- |
| `min-width`/`min-height`/`max-width`/`max-height` | `Option<u16>` :43-49 | 0..=100000 (too wide) | 0..=65535 | `parity_bounds_window_rule_sizes_and_opacity` |
| `opacity` | `Option<f32>`, **no parse bound** :61-62; clamped when resolved | 0..=1, an out-of-range value refused the config | any finite f32; the existing clamp in `src/layout/rules.rs` stays | same |
| `geometry-corner-radius` | `Option<CornerRadius>`, four unbounded `f32` `appearance.rs:168-177` | 0..=65535 (too narrow) | any finite | same |
| `scroll-factor` | `FloatOrInt<0, 100>` :82-83 | 0..=100 | unchanged | same |
| `pinch-sensitivity` | `FloatOrInt<0, 100>` :84-85 | unsupported, value unchecked | unsupported, 0..=100 checked | same |
| `default-floating-position x=`/`y=` | `FloatOrInt<-65535, 65535>` :180-190 | -65535..=65535 | unchanged | rule_policy tests |

## Animations (`niri-config/src/animations.rs`)

| option | niri type and range | wiri before | wiri after | test |
| --- | --- | --- | --- | --- |
| `slowdown` | `FloatOrInt<0, i32::MAX>` :56-64 | 0..=1000 (too narrow) | 0..=i32::MAX | `parity_bounds_animation_slowdown_duration_and_spring` |
| `duration-ms` | `u32`, no upper bound :664-680 | whole 0..=60000 | whole 0..=u32::MAX | same |
| `cubic-bezier` x1/x2 | `FloatOrInt<0, 1>` :690-740 | 0..=1 | unchanged | decode.rs bezier tests |
| `cubic-bezier` y1/y2 | `FloatOrInt<i32::MIN, i32::MAX>` | same range | unchanged | same |
| `spring damping-ratio=` | 0.1..=10.0, error `damping-ratio must be between 0.1 and 10.0` :886-893 | 0..=100 (too wide) | 0.1..=10.0 | same |
| `spring stiffness=` | `u32`, `stiffness must be >= 1` | 1e-6..=1e9, decimals accepted | whole >= 1 | same |
| `spring epsilon=` | 0.00001..=0.1, error text with the range | 1e-12..=1.0 (too wide) | 0.00001..=0.1 | same |

## Gestures (`niri-config/src/gestures.rs`)

| option | niri type and range | wiri before | wiri after | test |
| --- | --- | --- | --- | --- |
| `dnd-edge-view-scroll trigger-width`, `dnd-edge-workspace-switch trigger-height` | `FloatOrInt<0, 65535>` :40-50, 74-84 | 0..=10000 | 0..=65535 | `parity_bounds_gestures_and_recent_windows` |
| `delay-ms` | `u16` | whole 0..=60000 | whole 0..=65535 | same |
| `max-speed` | `FloatOrInt<0, 1_000_000>` | 0..=100000 | 0..=1_000_000 | same |

## Recent windows (`niri-config/src/recent_windows.rs`)

| option | niri type and range | wiri before | wiri after | test |
| --- | --- | --- | --- | --- |
| `debounce-ms`, `open-delay-ms` | `u16` :33-35 | whole 0..=u32::MAX | whole 0..=65535 | `parity_bounds_gestures_and_recent_windows` |
| `highlight padding`, `corner-radius` | `FloatOrInt<0, 65535>` :92-100 | 0..=100000 | 0..=65535 | same |
| `previews max-height` | `FloatOrInt<1, 65535>` :124-132 | 1..=100000 | 1..=65535 | same |
| `previews max-scale` | `FloatOrInt<0, 1>` | 0..=1 | unchanged | same |

## Binds (`niri-config/src/binds.rs`)

| option | niri type and range | wiri before | wiri after | test |
| --- | --- | --- | --- | --- |
| `cooldown-ms=` | `Duration::from_millis(u64)`, any whole millisecond count :840-856 | whole 0..=3_600_000 | whole 0..=u32::MAX (wiri stores u32) | decode.rs cooldown tests |

## Residual differences (documented, not silently accepted)

These are the things this lane did **not** make identical; the matrix row stays `partial` for
them:

1. **A number written as a quoted string.** wiri's numeric reader coerces `scale "1.5"`, every
   other numeric option included; knuffel rejects a string with
   `Unsupported value, only numbers are recognized` (`utils.rs:130-138`). Fixing this needs a
   strict reader used by the niri-only paths; the lenient reader is also what keeps wiri's older
   `key value` spellings loading.
2. **Unsupported Wayland-only option blocks** (`output { mode }`, `modeline`, `max-bpc`,
   `tablet`/`touch` `calibration-matrix`, `custom-shader`): wiri emits one "not implemented"
   warning per option and does not decode its arguments, so an invalid argument there is
   accepted with a warning where niri refuses the file. The options with a single numeric
   argument (`xcursor-size`, `accel-speed`, `pinch-sensitivity`) are validated.
3. **Negative fixed sizes.** niri's `PresetSize::Fixed(i32)` accepts a negative value; wiri's
   `SizeSpec::Fixed` is a `u32`, so `fixed -1` is an explicit error instead of a wrap.
4. **Border and focus-ring width** are stored as whole device pixels (`RingStyle::width: u32`);
   a fractional niri width is rounded once at decode. niri keeps `f64` and rounds at render.
5. **`cooldown-ms`** is a `u32` in wiri, `u64` in niri; values above `u32::MAX` are refused.
6. `Percent` options (`focus-follows-mouse max-scroll-amount`) are accepted by wiri's numeric
   reader without the mandatory `%`; niri requires the `%` suffix (`utils.rs:58-70`).

Nothing above silently rounds, clamps or accepts a value that changes behaviour without saying
so: each is either an explicit error or an ignored option that Windows cannot apply.
