//! Overview background rasterization, reusing the decoration shadow renderer.
use crate::config::overview_config::OverviewConfig;
use crate::overlay::decor::paint::{draw_shadow, RRect, Rgba, Surface};
use crate::utils::Rect;

/// The host is opaque (foreign DWM thumbnails require a non-layered destination).
/// Alpha colours composite against black, then workspace shadows composite over that.
pub(super) fn background(
    bounds: Rect,
    workspaces: impl IntoIterator<Item = Rect>,
    style: OverviewConfig,
    zoom: f64,
) -> Vec<u32> {
    let [r, g, b, a] = style.backdrop_color.map(u32::from);
    let base = 0xff00_0000 | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255);
    let mut pixels = vec![base; bounds.size.w as usize * bounds.size.h as usize];
    let shadow = style.workspace_shadow;
    if shadow.off || shadow.color[3] == 0 {
        return pixels;
    }
    let mut surface = Surface::new(bounds.size.w as i32, bounds.size.h as i32, &mut pixels);
    let scale = zoom.clamp(0.0, 1.0) as f32;
    for rect in workspaces {
        // Skip workspaces whose entire shadow is outside the viewport.
        let tile = RRect::of(rect, (bounds.loc.x, bounds.loc.y), 0.0);
        let shape = tile.outset(shadow.spread as f32 * scale).moved(
            shadow.offset_x as f32 * scale,
            shadow.offset_y as f32 * scale,
        );
        draw_shadow(
            &mut surface,
            &shape,
            &tile,
            shadow.softness as f32 * scale,
            Rgba::from_array(shadow.color),
            false,
        );
    }
    pixels
}

#[cfg(target_os = "windows")]
pub(super) unsafe fn paint(dc: windows::Win32::Graphics::Gdi::HDC, bounds: Rect, pixels: &[u32]) {
    use windows::Win32::Graphics::Gdi::*;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bounds.size.w as i32,
            biHeight: -(bounds.size.h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let _ = SetDIBitsToDevice(
        dc,
        0,
        0,
        bounds.size.w,
        bounds.size.h,
        0,
        0,
        0,
        bounds.size.h,
        pixels.as_ptr().cast(),
        &info,
        DIB_RGB_COLORS,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backdrop_and_shadow_off_have_exact_pixels() {
        let mut style = OverviewConfig {
            backdrop_color: [20, 40, 60, 255],
            ..Default::default()
        };
        style.workspace_shadow.off = true;
        assert_eq!(
            background(
                Rect::new(-10, -20, 40, 40),
                [Rect::new(0, 0, 10, 10)],
                style,
                0.5
            ),
            vec![0xff14_283c; 1600]
        );
    }
    #[test]
    fn shadow_offset_softness_spread_and_alpha_affect_pixels() {
        let bounds = Rect::new(-40, 0, 100, 100);
        let tile = Rect::new(-10, 30, 40, 40);
        let mut style = OverviewConfig {
            backdrop_color: [255; 4],
            ..Default::default()
        };
        style.workspace_shadow.color = [0, 0, 0, 255];
        style.workspace_shadow.offset_y = 8.0;
        let a = background(bounds, [tile], style, 0.5);
        style.workspace_shadow.spread = -8.0;
        style.workspace_shadow.softness = 0.0;
        let b = background(bounds, [tile], style, 0.5);
        assert_ne!(a, b);
        assert!(a.iter().any(|p| *p != 0xffff_ffff));
        assert!(
            b.iter().all(|p| *p == 0xffff_ffff),
            "contracted zero-softness shadow is behind the opaque workspace"
        );
    }
}
