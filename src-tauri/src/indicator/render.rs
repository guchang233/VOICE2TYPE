use super::model::IndicatorState;
use std::f32::consts::TAU;

pub const CARD_WIDTH: f32 = 156.0;
pub const CARD_HEIGHT: f32 = 40.0;
pub const PADDING: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8);

#[derive(Clone, Copy)]
pub struct Palette {
    pub surface: Color,
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub recording: Color,
    pub processing: Color,
    pub success: Color,
    pub shadow: f32,
}

impl Palette {
    /// Keep these tokens aligned with the native overlay's source of truth: src/styles.css.
    pub fn for_theme(theme: &str, system_light: bool) -> Self {
        match theme {
            "eye-care" => Self {
                surface: Color(251, 248, 240),
                text: Color(54, 48, 31),
                muted: Color(108, 98, 75),
                border: Color(222, 214, 199),
                recording: Color(191, 79, 58),
                processing: Color(168, 122, 18),
                success: Color(75, 138, 95),
                shadow: 0.16,
            },
            "light" | "auto" if theme == "light" || system_light => Self {
                surface: Color(255, 255, 255),
                text: Color(22, 22, 28),
                muted: Color(93, 93, 106),
                border: Color(233, 233, 235),
                recording: Color(229, 72, 77),
                processing: Color(194, 131, 10),
                success: Color(30, 158, 94),
                shadow: 0.14,
            },
            _ => Self {
                surface: Color(26, 26, 31),
                text: Color(242, 242, 245),
                muted: Color(162, 162, 173),
                border: Color(44, 44, 48),
                recording: Color(255, 92, 108),
                processing: Color(245, 185, 65),
                success: Color(61, 214, 140),
                shadow: 0.35,
            },
        }
    }

    fn status(self, state: IndicatorState) -> Color {
        match state {
            IndicatorState::Recording | IndicatorState::Error => self.recording,
            IndicatorState::Processing => self.processing,
            IndicatorState::Success => self.success,
            _ => self.muted,
        }
    }
}

/// Composite straight RGB into an already premultiplied BGRA pixel.
pub fn over(pixel: &mut u32, color: Color, coverage: f32) {
    let alpha = (coverage.clamp(0.0, 1.0) * 255.0).round() as u32;
    let inverse = 255 - alpha;
    let mix =
        |source: u8, destination: u32| (source as u32 * alpha + destination * inverse + 127) / 255;
    let a = alpha + (((*pixel >> 24) * inverse + 127) / 255);
    let r = mix(color.0, (*pixel >> 16) & 255);
    let g = mix(color.1, (*pixel >> 8) & 255);
    let b = mix(color.2, *pixel & 255);
    *pixel = (a << 24) | (r << 16) | (g << 8) | b;
}

fn rounded_distance(x: f32, y: f32, half_w: f32, half_h: f32, radius: f32) -> f32 {
    let dx = x.abs() - half_w + radius;
    let dy = y.abs() - half_h + radius;
    dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius
}

fn segment(x: f32, y: f32, start: (f32, f32), end: (f32, f32)) -> f32 {
    let (vx, vy) = (end.0 - start.0, end.1 - start.1);
    let t = (((x - start.0) * vx + (y - start.1) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
    (x - start.0 - t * vx).hypot(y - start.1 - t * vy)
}

fn icon_distance(state: IndicatorState, x: f32, y: f32, seconds: f32) -> f32 {
    match state {
        IndicatorState::Recording => {
            let mut distance = f32::MAX;
            for i in -2..=2 {
                let amplitude = 2.0 + 3.5 * (seconds * 4.0 + i as f32 * 0.8).sin().powi(2);
                distance =
                    distance.min(rounded_distance(x - i as f32 * 3.0, y, 0.9, amplitude, 0.9));
            }
            distance
        }
        IndicatorState::Processing => {
            let phase = (y.atan2(x) - seconds * 4.5).rem_euclid(TAU);
            if phase < TAU * 0.75 {
                (x.hypot(y) - 5.5).abs() - 0.9
            } else {
                f32::MAX
            }
        }
        IndicatorState::Success => {
            segment(x, y, (-5.0, 0.0), (-1.5, 3.5)).min(segment(x, y, (-1.5, 3.5), (5.5, -3.5)))
                - 0.9
        }
        IndicatorState::Error => segment(x, y, (0.0, -5.0), (0.0, 1.0)).min(x.hypot(y - 4.5)) - 1.0,
        IndicatorState::Cancelled => {
            segment(x, y, (-4.0, -4.0), (4.0, 4.0)).min(segment(x, y, (4.0, -4.0), (-4.0, 4.0)))
                - 0.9
        }
        IndicatorState::Hidden => f32::MAX,
    }
}

/// Every row uses the DIB's actual stride, independently of the visible frame width.
pub fn paint(
    pixels: &mut [u32],
    stride: usize,
    width: usize,
    height: usize,
    scale: f32,
    palette: Palette,
    state: IndicatorState,
    seconds: f32,
) {
    assert!(stride >= width && pixels.len() >= stride * height);
    pixels.fill(0);
    let center_x = width as f32 / scale / 2.0;
    let center_y = height as f32 / scale / 2.0;
    let color = palette.status(state);
    for y in 0..height {
        for x in 0..width {
            let px = (x as f32 + 0.5) / scale;
            let py = (y as f32 + 0.5) / scale;
            let pixel = &mut pixels[y * stride + x];
            let d = rounded_distance(
                px - center_x,
                py - center_y,
                CARD_WIDTH / 2.0,
                CARD_HEIGHT / 2.0,
                12.0,
            );
            let shadow = rounded_distance(
                px - center_x,
                py - center_y - 2.0,
                CARD_WIDTH / 2.0,
                CARD_HEIGHT / 2.0,
                12.0,
            )
            .max(0.0);
            over(
                pixel,
                Color(0, 0, 0),
                palette.shadow * (-shadow * shadow / 32.0).exp(),
            );
            over(pixel, palette.border, (0.5 - d * scale).clamp(0.0, 1.0));
            over(
                pixel,
                palette.surface,
                (0.5 - (d + 1.0) * scale).clamp(0.0, 1.0),
            );

            let ix = px - (center_x - CARD_WIDTH / 2.0 + 26.0);
            let iy = py - center_y;
            let badge = rounded_distance(ix, iy, 12.0, 12.0, 7.0);
            over(pixel, color, (0.5 - badge * scale).clamp(0.0, 1.0) * 0.10);
            if ix.abs() < 9.0 && iy.abs() < 9.0 {
                let icon = icon_distance(state, ix, iy, seconds);
                over(pixel, color, (0.5 - icon * scale).clamp(0.0, 1.0));
            }
        }
    }
}

/// GDI draws white glyphs on a separate black mask, never into the alpha surface.
/// Grayscale coverage preserves text color and avoids ClearType fringes on transparency.
pub fn composite_text(pixels: &mut [u32], mask: &[u32], color: Color) {
    assert_eq!(pixels.len(), mask.len());
    for (pixel, glyph) in pixels.iter_mut().zip(mask) {
        let coverage =
            (((glyph >> 16) & 255) + ((glyph >> 8) & 255) + (glyph & 255)) as f32 / 765.0;
        if coverage > 0.0 {
            over(pixel, color, coverage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painting_uses_bitmap_stride_and_keeps_padding_clear() {
        let mut pixels = vec![u32::MAX; 200 * 64];
        paint(
            &mut pixels,
            200,
            180,
            64,
            1.0,
            Palette::for_theme("dark", false),
            IndicatorState::Recording,
            0.0,
        );
        assert!(pixels
            .chunks(200)
            .all(|row| row[180..].iter().all(|p| *p == 0)));
        assert_ne!(pixels[32 * 200 + 90], 0);
    }

    #[test]
    fn all_states_themes_and_scales_produce_premultiplied_pixels() {
        for theme in ["dark", "light", "eye-care", "auto"] {
            for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
                let width = ((CARD_WIDTH + PADDING * 2.0) * scale).ceil() as usize;
                let height = ((CARD_HEIGHT + PADDING * 2.0) * scale).ceil() as usize;
                let mut pixels = vec![0; width * height];
                for state in [
                    IndicatorState::Recording,
                    IndicatorState::Processing,
                    IndicatorState::Success,
                    IndicatorState::Error,
                    IndicatorState::Cancelled,
                ] {
                    paint(
                        &mut pixels,
                        width,
                        width,
                        height,
                        scale,
                        Palette::for_theme(theme, true),
                        state,
                        0.3,
                    );
                    assert!(pixels.iter().all(|p| {
                        let a = p >> 24;
                        ((p >> 16) & 255) <= a && ((p >> 8) & 255) <= a && (p & 255) <= a
                    }));
                }
            }
        }
    }

    #[test]
    fn grayscale_mask_composites_light_and_dark_text() {
        for color in [Color(242, 242, 245), Color(22, 22, 28)] {
            let mut pixels = [0; 3];
            composite_text(&mut pixels, &[0, 0x00808080, 0x00ffffff], color);
            assert_eq!(pixels[0], 0);
            assert_eq!(pixels[1] >> 24, 128);
            assert_eq!(
                pixels[2],
                0xff000000 | ((color.0 as u32) << 16) | ((color.1 as u32) << 8) | color.2 as u32
            );
        }
    }
}
