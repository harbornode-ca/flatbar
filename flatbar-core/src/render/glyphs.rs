//! Rasterization of layout elements, glyph blitting, and emphasis state rendering.

use crate::config::Color;
use crate::render::layout::LayoutResult;
use crate::render::text::{TextRenderParams, TextRenderer};
use crate::shell::scaling::logical_rect_to_buffer;

/// Options for rendering a layout.
#[derive(Debug, Clone)]
pub struct RenderLayoutOptions<'a> {
    pub font_family: &'a str,
    pub font_size: f32,
    pub scale: f64,
    pub bg_color: Color,
    pub fg_color: Color,
}

/// Render all widgets from a computed layout into the physical ARGB8888 buffer.
pub fn render_layout(
    pixels: &mut [u32],
    buf_width: u32,
    buf_height: u32,
    layout: &LayoutResult,
    text_renderer: &TextRenderer,
    opts: &RenderLayoutOptions<'_>,
) {
    // 1. Draw emphasis backgrounds
    for wp in layout.all_widgets() {
        if wp.emphasis {
            let (bx, by, bw, bh) = logical_rect_to_buffer(
                wp.rect.x,
                wp.rect.y,
                wp.rect.width,
                wp.rect.height,
                opts.scale,
            );
            let fg_u32 = opts.fg_color.to_argb_u32();

            for row in 0..bh {
                let target_y = by + row as i32;
                if target_y < 0 || target_y >= buf_height as i32 {
                    continue;
                }
                for col in 0..bw {
                    let target_x = bx + col as i32;
                    if target_x < 0 || target_x >= buf_width as i32 {
                        continue;
                    }
                    let idx = (target_y as usize) * (buf_width as usize) + (target_x as usize);
                    if idx < pixels.len() {
                        pixels[idx] = fg_u32;
                    }
                }
            }
        }
    }

    // 2. Draw spans (text / icons)
    for wp in layout.all_widgets() {
        for sp in &wp.spans {
            let (bx, by, _, _) = logical_rect_to_buffer(
                sp.rect.x,
                sp.rect.y,
                sp.rect.width,
                sp.rect.height,
                opts.scale,
            );

            // Invert colors if the widget or span is emphasized
            let effective_fg = if wp.emphasis || sp.span.emphasis {
                opts.bg_color
            } else {
                opts.fg_color
            };

            let params = TextRenderParams {
                text: &sp.display_text,
                font_family: opts.font_family,
                font_size_pt: opts.font_size,
                scale: opts.scale,
                dest_x: bx,
                dest_y: by,
                fg_color: effective_fg,
            };

            text_renderer.render_text(pixels, buf_width, buf_height, &params);
        }
    }
}
