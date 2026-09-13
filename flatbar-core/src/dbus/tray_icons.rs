//! Tray icon loading, pixmap decoding, theme lookup, and SVG rasterization.

use std::sync::Arc;

/// A decoded RGBA / ARGB image for tray icons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayIconPixmap {
    pub width: u32,
    pub height: u32,
    /// 32-bit ARGB pixels (0xAARRGGBB) in native byte order.
    pub pixels: Vec<u32>,
}

impl TrayIconPixmap {
    /// Create a new pixmap buffer.
    pub fn new(width: u32, height: u32, pixels: Vec<u32>) -> Self {
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Decode raw SNI icon pixmap format.
    ///
    /// The SNI specification defines `IconPixmap` as an array of `(i32, i32, Vec<u8>)`
    /// where the elements are: `(width, height, ARGB32_network_byte_order_data)`.
    /// Colors are premultiplied ARGB in big-endian (network byte order).
    pub fn from_sni_pixmap(width: i32, height: i32, data: &[u8]) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let w = width as usize;
        let h = height as usize;
        let expected_bytes = w.checked_mul(h)?.checked_mul(4)?;
        if data.len() < expected_bytes {
            return None;
        }

        let mut pixels = Vec::with_capacity(w * h);
        for chunk in data.chunks_exact(4).take(w * h) {
            let a = chunk[0];
            let r = chunk[1];
            let g = chunk[2];
            let b = chunk[3];
            let argb = ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
            pixels.push(argb);
        }

        Some(Self {
            width: width as u32,
            height: height as u32,
            pixels,
        })
    }

    /// Choose the best matching pixmap from a list of SNI pixmaps for a requested size.
    pub fn select_best_pixmap(
        pixmaps: &[(i32, i32, Vec<u8>)],
        target_size: u32,
    ) -> Option<TrayIconPixmap> {
        if pixmaps.is_empty() {
            return None;
        }

        // Find pixmap closest to target_size (prefer >= target_size, then largest)
        let mut best: Option<&(i32, i32, Vec<u8>)> = None;
        let mut best_diff = i32::MAX;

        for item in pixmaps {
            let (w, h, _) = item;
            if *w <= 0 || *h <= 0 {
                continue;
            }
            let dim = (*w).max(*h);
            let diff = (dim - target_size as i32).abs();
            if diff < best_diff {
                best_diff = diff;
                best = Some(item);
            }
        }

        best.and_then(|(w, h, data)| Self::from_sni_pixmap(*w, *h, data))
    }
}

/// Load a named freedesktop theme icon and render to a pixmap at requested size and scale.
pub fn load_theme_icon(
    icon_name: &str,
    target_size: u32,
    scale: f64,
    theme: Option<&str>,
) -> Option<TrayIconPixmap> {
    let physical_size = ((target_size as f64) * scale).round() as u16;
    let icon_path = if let Some(t) = theme {
        freedesktop_icons::lookup(icon_name)
            .with_theme(t)
            .with_size(physical_size)
            .find()
    } else {
        freedesktop_icons::lookup(icon_name)
            .with_size(physical_size)
            .find()
    }?;

    load_icon_from_path(&icon_path, target_size, scale)
}

/// Load an icon from a file path (PNG, SVG, etc.) and rasterize to ARGB pixmap.
pub fn load_icon_from_path(
    path: &std::path::Path,
    target_size: u32,
    scale: f64,
) -> Option<TrayIconPixmap> {
    let physical_size = ((target_size as f64) * scale).round().max(1.0) as u32;

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if ext.eq_ignore_ascii_case("svg") {
            return rasterize_svg_file(path, physical_size, physical_size);
        }
    }

    // Try raster image decode via `image` crate
    if let Ok(img) = image::open(path) {
        let rgba = img
            .resize_exact(
                physical_size,
                physical_size,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8();

        let mut pixels = Vec::with_capacity((physical_size * physical_size) as usize);
        for pixel in rgba.pixels() {
            let [r, g, b, a] = pixel.0;
            let argb = ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
            pixels.push(argb);
        }

        return Some(TrayIconPixmap {
            width: physical_size,
            height: physical_size,
            pixels,
        });
    }

    None
}

/// Rasterize an SVG file to an ARGB32 pixmap.
pub fn rasterize_svg_file(
    path: &std::path::Path,
    target_w: u32,
    target_h: u32,
) -> Option<TrayIconPixmap> {
    let svg_data = std::fs::read(path).ok()?;
    rasterize_svg_bytes(&svg_data, target_w, target_h)
}

/// Rasterize raw SVG XML bytes to an ARGB32 pixmap.
pub fn rasterize_svg_bytes(
    svg_data: &[u8],
    target_w: u32,
    target_h: u32,
) -> Option<TrayIconPixmap> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(svg_data, &opt).ok()?;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(target_w, target_h)?;

    let size = tree.size();
    let sx = target_w as f32 / size.width();
    let sy = target_h as f32 / size.height();
    let transform = resvg::tiny_skia::Transform::from_scale(sx, sy);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // Convert RGBA premultiplied pixels to ARGB32
    let data = pixmap.data();
    let mut pixels = Vec::with_capacity((target_w * target_h) as usize);
    for chunk in data.chunks_exact(4) {
        let r = chunk[0];
        let g = chunk[1];
        let b = chunk[2];
        let a = chunk[3];
        let argb = ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        pixels.push(argb);
    }

    Some(TrayIconPixmap {
        width: target_w,
        height: target_h,
        pixels,
    })
}

/// Source of an SNI icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayIconSource {
    /// Named icon in the Freedesktop icon theme (e.g. "nm-signal-100").
    Name(String),
    /// Raw ARGB32 pixmap provided directly over DBus.
    Pixmap(Arc<TrayIconPixmap>),
    /// Fallback glyph or icon alias (e.g. "fa-bell").
    Glyph(String),
}

/// Icon-font glyph shown when neither a pixmap nor a theme icon can be resolved.
/// Deliberately neutral: never the broken-widget "⚠" marker.
pub const FALLBACK_TRAY_ICON: &str = "fa-circle-dot";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sni_pixmap_decode() {
        // 2x2 pixmap, 4 bytes per pixel: A, R, G, B
        let data: Vec<u8> = vec![
            255, 255, 0, 0, // Red
            255, 0, 255, 0, // Green
            255, 0, 0, 255, // Blue
            128, 64, 64, 64, // Semi-transparent
        ];

        let pix = TrayIconPixmap::from_sni_pixmap(2, 2, &data).unwrap();
        assert_eq!(pix.width, 2);
        assert_eq!(pix.height, 2);
        assert_eq!(pix.pixels.len(), 4);
        assert_eq!(pix.pixels[0], 0xFFFF0000);
        assert_eq!(pix.pixels[1], 0xFF00FF00);
        assert_eq!(pix.pixels[2], 0xFF0000FF);
        assert_eq!(pix.pixels[3], 0x80404040);
    }

    #[test]
    fn test_select_best_pixmap() {
        let small = (16, 16, vec![0u8; 16 * 16 * 4]);
        let medium = (24, 24, vec![0u8; 24 * 24 * 4]);
        let large = (48, 48, vec![0u8; 48 * 48 * 4]);

        let pixmaps = vec![small, medium, large];

        let chosen = TrayIconPixmap::select_best_pixmap(&pixmaps, 24).unwrap();
        assert_eq!(chosen.width, 24);
        assert_eq!(chosen.height, 24);

        let chosen_large = TrayIconPixmap::select_best_pixmap(&pixmaps, 64).unwrap();
        assert_eq!(chosen_large.width, 48);
    }

    #[test]
    fn test_svg_rasterize_simple() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <rect width="100" height="100" fill="red"/>
        </svg>"#;

        let pixmap = rasterize_svg_bytes(svg.as_bytes(), 32, 32).unwrap();
        assert_eq!(pixmap.width, 32);
        assert_eq!(pixmap.height, 32);
        assert_eq!(pixmap.pixels.len(), 32 * 32);
        // Alpha and Red channel should be non-zero
        assert!(pixmap.pixels[0] & 0xFF000000 != 0);
    }
}
