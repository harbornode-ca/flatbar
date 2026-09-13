//! Snapshot rendering into in-memory RGBA buffers for Layer 2 golden-image testing.

use crate::config::{Color, LayoutConfig};
use crate::render::buffer::BufferManager;
use crate::render::glyphs::{render_layout, RenderLayoutOptions};
use crate::render::layout::compute_layout;
use crate::render::text::TextRenderer;
use crate::shell::scaling::logical_to_buffer;
use crate::widget::Widget;
use image::{Rgba, RgbaImage};

/// Options for configuring a snapshot rendering.
#[derive(Debug, Clone)]
pub struct SnapshotOptions<'a> {
    pub font_family: &'a str,
    pub font_size: f32,
    pub scale: f64,
    pub bar_width: u32,
    pub bar_height: u32,
    pub inner_padding: u32,
    pub bg_color: Color,
    pub fg_color: Color,
}

/// Render a status bar snapshot directly into an in-memory RGBA image.
pub fn render_snapshot(
    layout_config: &LayoutConfig,
    widgets: &[Box<dyn Widget>],
    opts: &SnapshotOptions<'_>,
) -> RgbaImage {
    let text_renderer = TextRenderer::new();
    let layout = compute_layout(
        layout_config,
        widgets,
        &text_renderer,
        opts.font_family,
        opts.font_size,
        opts.bar_width,
        opts.bar_height,
        opts.inner_padding,
    );

    let physical_width = logical_to_buffer(opts.bar_width, opts.scale);
    let physical_height = logical_to_buffer(opts.bar_height, opts.scale);

    let mut raw_bytes = vec![0u8; (physical_width * physical_height * 4) as usize];
    BufferManager::clear_canvas(&mut raw_bytes, opts.bg_color);

    let pixels: &mut [u32] = bytemuck::cast_slice_mut(&mut raw_bytes);
    let render_opts = RenderLayoutOptions {
        font_family: opts.font_family,
        font_size: opts.font_size,
        scale: opts.scale,
        bg_color: opts.bg_color,
        fg_color: opts.fg_color,
    };

    render_layout(
        pixels,
        physical_width,
        physical_height,
        &layout,
        &text_renderer,
        &render_opts,
    );

    // Convert ARGB8888 u32 buffer to image::RgbaImage
    let mut img = RgbaImage::new(physical_width, physical_height);
    for y in 0..physical_height {
        for x in 0..physical_width {
            let idx = (y * physical_width + x) as usize;
            let p = pixels[idx];
            let a = ((p >> 24) & 0xff) as u8;
            let r = ((p >> 16) & 0xff) as u8;
            let g = ((p >> 8) & 0xff) as u8;
            let b = (p & 0xff) as u8;
            img.put_pixel(x, y, Rgba([r, g, b, a]));
        }
    }

    img
}
