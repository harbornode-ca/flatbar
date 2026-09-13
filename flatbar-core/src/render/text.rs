//! cosmic-text wrapper for font loading, text shaping, and alpha mask rasterization.

use crate::config::Color;
use crate::render::text_cache::{CachedGlyph, ShapedRun, ShapingCache, TextCacheKey};
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache};
use std::sync::Mutex;

/// Parameters for rendering text onto a target buffer.
#[derive(Debug, Clone)]
pub struct TextRenderParams<'a> {
    pub text: &'a str,
    pub font_family: &'a str,
    pub font_size_pt: f32,
    pub scale: f64,
    pub dest_x: i32,
    pub dest_y: i32,
    pub fg_color: Color,
}

/// Text renderer wrapping cosmic-text's FontSystem and SwashCache with an LRU shaping cache.
pub struct TextRenderer {
    pub font_system: Mutex<FontSystem>,
    pub swash_cache: Mutex<SwashCache>,
    pub shaping_cache: ShapingCache,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    pub fn new() -> Self {
        Self {
            font_system: Mutex::new(FontSystem::new()),
            swash_cache: Mutex::new(SwashCache::new()),
            shaping_cache: ShapingCache::new(2048),
        }
    }

    /// Measure logical text layout dimensions without drawing.
    pub fn measure_text(&self, params: &TextRenderParams<'_>) -> (u32, u32) {
        let key = TextCacheKey::new(
            params.text,
            params.font_family,
            params.font_size_pt,
            params.scale,
        );

        if let Some(cached) = self.shaping_cache.get(&key) {
            return (cached.width, cached.height);
        }

        let mut font_system = self.font_system.lock().unwrap();
        let font_size_px = (params.font_size_pt as f64 * params.scale) as f32;
        let line_height_px = font_size_px * 1.3;

        let metrics = Metrics::new(font_size_px, line_height_px);
        let mut buffer = Buffer::new(&mut font_system, metrics);

        let family = if params.font_family.is_empty() || params.font_family == "sans-serif" {
            Family::SansSerif
        } else {
            Family::Name(params.font_family)
        };

        let attrs = Attrs::new().family(family);
        buffer.set_text(params.text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut font_system, false);

        let mut measured_width = 0.0f32;
        for run in buffer.layout_runs() {
            if run.line_w > measured_width {
                measured_width = run.line_w;
            }
        }
        let measured_w = (measured_width / params.scale as f32).ceil() as u32;
        let measured_h = (line_height_px / params.scale as f32).ceil() as u32;

        (measured_w, measured_h)
    }

    /// Render a single line of text onto a target ARGB8888 buffer.
    /// `target_buffer` is a slice of u32 pixels with dimensions `(buf_width, buf_height)`.
    pub fn render_text(
        &self,
        target_buffer: &mut [u32],
        buf_width: u32,
        buf_height: u32,
        params: &TextRenderParams<'_>,
    ) -> (u32, u32) {
        let key = TextCacheKey::new(
            params.text,
            params.font_family,
            params.font_size_pt,
            params.scale,
        );

        // Check if shaped run is already in cache
        if let Some(cached) = self.shaping_cache.get(&key) {
            self.draw_cached_run(target_buffer, buf_width, buf_height, params, &cached);
            return (cached.width, cached.height);
        }

        let mut font_system = self.font_system.lock().unwrap();
        let mut swash_cache = self.swash_cache.lock().unwrap();

        // Physical pixel size for font metrics
        let font_size_px = (params.font_size_pt as f64 * params.scale) as f32;
        let line_height_px = font_size_px * 1.3;

        let metrics = Metrics::new(font_size_px, line_height_px);
        let mut buffer = Buffer::new(&mut font_system, metrics);

        let family = if params.font_family.is_empty() || params.font_family == "sans-serif" {
            Family::SansSerif
        } else {
            Family::Name(params.font_family)
        };

        let attrs = Attrs::new().family(family);
        buffer.set_text(params.text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut font_system, false);

        // Measure text layout size
        let mut measured_width = 0.0f32;
        for run in buffer.layout_runs() {
            if run.line_w > measured_width {
                measured_width = run.line_w;
            }
        }
        let measured_w = measured_width.ceil() as u32;
        let measured_h = line_height_px.ceil() as u32;

        let cosmic_fg = cosmic_text::Color::rgba(
            params.fg_color.r,
            params.fg_color.g,
            params.fg_color.b,
            params.fg_color.a,
        );

        let mut cached_glyphs = Vec::new();

        // Draw glyphs into buffer and record in cache
        buffer.draw(
            &mut font_system,
            &mut swash_cache,
            cosmic_fg,
            |x, y, w, h, color| {
                let alpha = color.a();
                if alpha > 0 {
                    cached_glyphs.push(CachedGlyph {
                        x,
                        y,
                        width: w,
                        height: h,
                        alpha_mask: vec![alpha; (w * h) as usize],
                    });
                }

                let px_x = params.dest_x + x;
                let px_y = params.dest_y + y;

                for row in 0..h {
                    let target_y = px_y + row as i32;
                    if target_y < 0 || target_y >= buf_height as i32 {
                        continue;
                    }

                    for col in 0..w {
                        let target_x = px_x + col as i32;
                        if target_x < 0 || target_x >= buf_width as i32 {
                            continue;
                        }

                        let idx = (target_y as usize) * (buf_width as usize) + (target_x as usize);
                        if idx >= target_buffer.len() {
                            continue;
                        }

                        if alpha == 0 {
                            continue;
                        }

                        let r = color.r();
                        let g = color.g();
                        let b = color.b();

                        if alpha == 255 {
                            target_buffer[idx] = ((255u32) << 24)
                                | ((r as u32) << 16)
                                | ((g as u32) << 8)
                                | (b as u32);
                        } else {
                            // Alpha blend with existing buffer pixel
                            let existing = target_buffer[idx];
                            let ex_a = (existing >> 24) & 0xff;
                            let ex_r = (existing >> 16) & 0xff;
                            let ex_g = (existing >> 8) & 0xff;
                            let ex_b = existing & 0xff;

                            let a_u32 = alpha as u32;
                            let inv_a = 255 - a_u32;

                            let out_r = (r as u32 * a_u32 + ex_r * inv_a) / 255;
                            let out_g = (g as u32 * a_u32 + ex_g * inv_a) / 255;
                            let out_b = (b as u32 * a_u32 + ex_b * inv_a) / 255;
                            let out_a = (a_u32 * 255 + ex_a * inv_a) / 255;

                            target_buffer[idx] =
                                (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b;
                        }
                    }
                }
            },
        );

        // Store into shaping cache
        self.shaping_cache.insert(
            key,
            ShapedRun {
                width: measured_w,
                height: measured_h,
                glyphs: cached_glyphs,
            },
        );

        (measured_w, measured_h)
    }

    fn draw_cached_run(
        &self,
        target_buffer: &mut [u32],
        buf_width: u32,
        buf_height: u32,
        params: &TextRenderParams<'_>,
        run: &ShapedRun,
    ) {
        let fg = params.fg_color;
        let r = fg.r as u32;
        let g = fg.g as u32;
        let b = fg.b as u32;

        for glyph in &run.glyphs {
            let px_x = params.dest_x + glyph.x;
            let px_y = params.dest_y + glyph.y;

            for row in 0..glyph.height {
                let target_y = px_y + row as i32;
                if target_y < 0 || target_y >= buf_height as i32 {
                    continue;
                }

                for col in 0..glyph.width {
                    let target_x = px_x + col as i32;
                    if target_x < 0 || target_x >= buf_width as i32 {
                        continue;
                    }

                    let mask_idx = (row * glyph.width + col) as usize;
                    let alpha = if mask_idx < glyph.alpha_mask.len() {
                        glyph.alpha_mask[mask_idx]
                    } else {
                        255
                    };

                    if alpha == 0 {
                        continue;
                    }

                    let idx = (target_y as usize) * (buf_width as usize) + (target_x as usize);
                    if idx >= target_buffer.len() {
                        continue;
                    }

                    if alpha == 255 {
                        target_buffer[idx] = (255 << 24) | (r << 16) | (g << 8) | b;
                    } else {
                        let existing = target_buffer[idx];
                        let ex_a = (existing >> 24) & 0xff;
                        let ex_r = (existing >> 16) & 0xff;
                        let ex_g = (existing >> 8) & 0xff;
                        let ex_b = existing & 0xff;

                        let a_u32 = alpha as u32;
                        let inv_a = 255 - a_u32;

                        let out_r = (r * a_u32 + ex_r * inv_a) / 255;
                        let out_g = (g * a_u32 + ex_g * inv_a) / 255;
                        let out_b = (b * a_u32 + ex_b * inv_a) / 255;
                        let out_a = (a_u32 * 255 + ex_a * inv_a) / 255;

                        target_buffer[idx] = (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_renderer_cache_hit() {
        let renderer = TextRenderer::new();
        let mut buffer = vec![0u32; 200 * 40];
        let params = TextRenderParams {
            text: "CacheMe",
            font_family: "sans-serif",
            font_size_pt: 13.0,
            scale: 1.0,
            dest_x: 10,
            dest_y: 10,
            fg_color: Color::rgb(255, 255, 255),
        };

        // First run: cache miss
        let (w1, h1) = renderer.render_text(&mut buffer, 200, 40, &params);
        assert_eq!(renderer.shaping_cache.stats().1, 1); // 1 miss

        // Second run: cache hit
        let (w2, h2) = renderer.render_text(&mut buffer, 200, 40, &params);
        assert_eq!(w1, w2);
        assert_eq!(h1, h2);
        assert_eq!(renderer.shaping_cache.stats().0, 1); // 1 hit
    }
}
