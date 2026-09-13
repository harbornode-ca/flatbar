//! System Tray widget hosting Freedesktop StatusNotifierItems (SNI).
//!
//! Renders a summary icon with attention badge on the status bar, and provides
//! a popup menu grid model for interacting with tray applications.

use std::collections::HashMap;
use std::time::Duration;

use crate::config::{Color, MenuBackend};
use crate::dbus::bridge::DbusBridgeHandle;
use crate::dbus::sni::{SniItem, SniStatus, TrayEvent};
use crate::dbus::tray_icons::TrayIconSource;
use crate::render::layout::Rect;
use crate::render::text::{TextRenderParams, TextRenderer};
use crate::shell::scaling::logical_rect_to_buffer;
use crate::widget::menu::{MenuItem, MenuModel};
use crate::widget::state::{Span, WidgetState};
use crate::widget::Widget;

/// Configuration options for the system tray widget.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayConfig {
    pub summary_icon: String,
    pub attention_icon: String,
    pub hidden: Vec<String>,
    pub only: Vec<String>,
    pub menu_backend: MenuBackend,
    pub icon_size: u32,
    /// Safety-net poll interval in seconds for items that emit no signals
    /// (0 = event-driven only).
    pub fallback_poll_secs: u64,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            summary_icon: "fa-chevron-right".to_string(),
            attention_icon: "fa-chevron-down".to_string(),
            hidden: Vec::new(),
            only: Vec::new(),
            menu_backend: MenuBackend::Popup,
            icon_size: 16,
            fallback_poll_secs: 10,
        }
    }
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

struct TrayInner {
    items: HashMap<String, SniItem>,
}

/// System Tray widget that tracks active SNI items and presents a summary on the bar.
pub struct TrayWidget {
    id: String,
    config: TrayConfig,
    inner: Mutex<TrayInner>,
    has_attention: AtomicBool,
    dbus_handle: Mutex<Option<DbusBridgeHandle>>,
}

impl TrayWidget {
    /// Create a new TrayWidget with the specified configuration.
    pub fn new(id: impl Into<String>, config: TrayConfig) -> Self {
        Self {
            id: id.into(),
            config,
            inner: Mutex::new(TrayInner {
                items: HashMap::new(),
            }),
            has_attention: AtomicBool::new(false),
            dbus_handle: Mutex::new(None),
        }
    }

    /// Safety-net poll interval for the DBus bridge (seconds, 0 = off).
    pub fn fallback_poll_secs(&self) -> u64 {
        self.config.fallback_poll_secs
    }

    /// Process a TrayEvent from the DBus bridge.
    pub fn handle_event(&self, event: TrayEvent) {
        let mut inner = self.inner.lock().unwrap();
        match event {
            TrayEvent::Added(item) => {
                let key = format!("{}{}", item.service, item.path);
                inner.items.insert(key, item);
                self.recalculate_attention_locked(&inner);
            }
            TrayEvent::Changed(item) => {
                let key = format!("{}{}", item.service, item.path);
                inner.items.insert(key, item);
                self.recalculate_attention_locked(&inner);
            }
            TrayEvent::Removed(key) => {
                inner.items.remove(&key);
                self.recalculate_attention_locked(&inner);
            }
            TrayEvent::AttentionChanged(att) => {
                self.has_attention.store(att, Ordering::Relaxed);
            }
        }
    }

    /// Recalculate attention state across visible items.
    fn recalculate_attention_locked(&self, inner: &TrayInner) {
        let att = self
            .filter_items(&inner.items)
            .iter()
            .any(|item| item.status == SniStatus::NeedsAttention);
        self.has_attention.store(att, Ordering::Relaxed);
    }

    /// Return snapshot of visible SNI items that pass whitelist and blacklist.
    pub fn visible_items(&self) -> Vec<SniItem> {
        let inner = self.inner.lock().unwrap();
        self.filter_items(&inner.items)
    }

    fn filter_items(&self, items: &HashMap<String, SniItem>) -> Vec<SniItem> {
        let mut visible = Vec::new();
        for item in items.values() {
            let identifier = if !item.id.is_empty() {
                &item.id
            } else if !item.title.is_empty() {
                &item.title
            } else {
                &item.service
            };

            // Check blacklist
            if self
                .config
                .hidden
                .iter()
                .any(|h| h.eq_ignore_ascii_case(identifier))
            {
                continue;
            }

            // Check whitelist if non-empty
            if !self.config.only.is_empty()
                && !self
                    .config
                    .only
                    .iter()
                    .any(|o| o.eq_ignore_ascii_case(identifier))
            {
                continue;
            }

            visible.push(item.clone());
        }

        // Sort by id / title for deterministic display
        visible.sort_by(|a, b| {
            let key_a = if !a.title.is_empty() { &a.title } else { &a.id };
            let key_b = if !b.title.is_empty() { &b.title } else { &b.id };
            key_a.cmp(key_b)
        });

        visible
    }

    /// Build a top-level `MenuModel` representing all visible tray applications.
    pub fn build_tray_menu(&self) -> MenuModel {
        let visible = self.visible_items();
        let mut menu_items = Vec::new();

        if visible.is_empty() {
            menu_items.push(MenuItem::Item {
                id: "empty".to_string(),
                label: "No tray items".to_string(),
                icon: None,
                enabled: false,
            });
            return MenuModel::new(menu_items).with_title("System Tray");
        }

        for item in visible {
            let label = if !item.title.is_empty() {
                item.title.clone()
            } else if !item.id.is_empty() {
                item.id.clone()
            } else {
                item.service.clone()
            };

            let item_id = format!("{}:{}", item.service, item.path);
            let icon = item.icon_name.clone();

            menu_items.push(MenuItem::Item {
                id: item_id,
                label,
                icon,
                enabled: true,
            });
        }

        MenuModel::new(menu_items).with_title("System Tray")
    }

    /// Get current config reference.
    /// Store DBus bridge handle for action dispatch.
    pub fn set_dbus_handle(&self, handle: crate::dbus::bridge::DbusBridgeHandle) {
        let mut inner_handle = self.dbus_handle.lock().unwrap();
        *inner_handle = Some(handle);
    }

    /// Get current config reference.
    pub fn config(&self) -> &TrayConfig {
        &self.config
    }
}

impl Widget for TrayWidget {
    fn id(&self) -> &str {
        &self.id
    }

    fn as_tray(&self) -> Option<&TrayWidget> {
        Some(self)
    }

    fn update_interval(&self) -> Duration {
        Duration::from_secs(1)
    }

    fn state(&self) -> WidgetState {
        let visible = self.visible_items();
        let count = visible.len();

        let tooltip = if count == 0 {
            tracing::debug!("Systray widget: no StatusNotifierItem items registered");
            Some("System Tray (empty)".to_string())
        } else {
            let names: Vec<&str> = visible
                .iter()
                .map(|i| {
                    if !i.title.is_empty() {
                        i.title.as_str()
                    } else if !i.id.is_empty() {
                        i.id.as_str()
                    } else {
                        i.service.as_str()
                    }
                })
                .collect();
            Some(format!("System Tray ({count}): {}", names.join(", ")))
        };

        if self.has_attention.load(Ordering::Relaxed) {
            WidgetState::new(vec![Span::emphasized_icon(&self.config.attention_icon)])
                .with_tooltip(tooltip)
        } else {
            WidgetState::new(vec![Span::icon(&self.config.summary_icon)]).with_tooltip(tooltip)
        }
    }

    fn get_menu(&self) -> Option<MenuModel> {
        Some(self.build_tray_menu())
    }

    fn popup_request(&self) -> Option<crate::widget::PopupRequest> {
        Some(crate::widget::PopupRequest::TrayGrid)
    }

    fn handle_menu_action(&self, action_id: &str) {
        if action_id == "empty" {
            return;
        }
        if let Some((service, path)) = action_id.split_once(':') {
            if let Some(ref handle) = *self.dbus_handle.lock().unwrap() {
                handle.activate(service, path, 0, 0);
            }
        }
    }
}

/// Placement information for an item within the tray icon grid popup.
#[derive(Debug, Clone)]
pub struct TrayGridItemPlacement {
    pub item: SniItem,
    pub rect: Rect,
    pub icon_rect: Rect,
}

/// Geometry and bounding box calculation for the tray icon grid popup.
#[derive(Debug, Clone)]
pub struct TrayGridLayout {
    pub width: u32,
    pub height: u32,
    pub items: Vec<TrayGridItemPlacement>,
}

impl TrayGridLayout {
    pub fn hit_test(&self, x: i32, y: i32) -> Option<usize> {
        self.items.iter().position(|it| it.rect.contains(x, y))
    }
}

/// Compute geometry for the tray icon grid popup.
pub fn layout_tray_grid(
    items: &[SniItem],
    _text_renderer: &TextRenderer,
    _font_family: &str,
    _font_size: f32,
) -> TrayGridLayout {
    let padding: i32 = 10;
    let gap: i32 = 6;
    let cell_size: i32 = 36;
    let icon_size: i32 = 24;

    let n = items.len();
    if n == 0 {
        return TrayGridLayout {
            width: 160,
            height: 48,
            items: Vec::new(),
        };
    }

    let cols = ((n as f64).sqrt().ceil() as usize).clamp(1, 6);
    let rows = n.div_ceil(cols);

    let width =
        (padding * 2 + cols as i32 * cell_size + (cols as i32 - 1).max(0) * gap).max(0) as u32;
    let height =
        (padding * 2 + rows as i32 * cell_size + (rows as i32 - 1).max(0) * gap).max(0) as u32;

    let mut placements = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let col = (i % cols) as i32;
        let row = (i / cols) as i32;

        let cell_x = padding + col * (cell_size + gap);
        let cell_y = padding + row * (cell_size + gap);

        let icon_x = cell_x + (cell_size - icon_size) / 2;
        let icon_y = cell_y + (cell_size - icon_size) / 2;

        placements.push(TrayGridItemPlacement {
            item: item.clone(),
            rect: Rect::new(cell_x, cell_y, cell_size as u32, cell_size as u32),
            icon_rect: Rect::new(icon_x, icon_y, icon_size as u32, icon_size as u32),
        });
    }

    TrayGridLayout {
        width,
        height,
        items: placements,
    }
}

/// Parameters for rendering the tray icon grid popup surface.
pub struct TrayGridRenderParams<'a> {
    pub items: &'a [SniItem],
    pub layout: &'a TrayGridLayout,
    pub hover_index: Option<usize>,
    pub font_family: &'a str,
    pub font_size: f32,
    pub scale: f64,
    pub bg_color: Color,
    pub fg_color: Color,
}

/// Blit an ARGB pixmap onto a destination ARGB buffer.
pub fn blit_pixmap(
    dest: &mut [u32],
    dest_w: u32,
    dest_h: u32,
    pixmap: &crate::dbus::tray_icons::TrayIconPixmap,
    target_rect: Rect,
) {
    if pixmap.width == 0 || pixmap.height == 0 {
        return;
    }
    let offset_x = target_rect.x + (target_rect.width as i32 - pixmap.width as i32) / 2;
    let offset_y = target_rect.y + (target_rect.height as i32 - pixmap.height as i32) / 2;

    for py in 0..pixmap.height {
        let dy = offset_y + py as i32;
        if dy < 0 || dy >= dest_h as i32 {
            continue;
        }
        for px in 0..pixmap.width {
            let dx = offset_x + px as i32;
            if dx < 0 || dx >= dest_w as i32 {
                continue;
            }
            let src = pixmap.pixels[(py * pixmap.width + px) as usize];
            let a = (src >> 24) & 0xff;
            if a == 0 {
                continue;
            }
            let dest_idx = (dy as u32 * dest_w + dx as u32) as usize;
            if dest_idx >= dest.len() {
                continue;
            }
            if a == 255 {
                dest[dest_idx] = src | 0xff000000;
            } else {
                let dst = dest[dest_idx];
                let inv_a = 255 - a;
                let r = ((src >> 16) & 0xff) + (((dst >> 16) & 0xff) * inv_a) / 255;
                let g = ((src >> 8) & 0xff) + (((dst >> 8) & 0xff) * inv_a) / 255;
                let b = (src & 0xff) + ((dst & 0xff) * inv_a) / 255;
                dest[dest_idx] = (0xff << 24) | (r.min(255) << 16) | (g.min(255) << 8) | b.min(255);
            }
        }
    }
}

/// Render the tray grid popup into a pixel buffer.
pub fn render_tray_grid(
    pixels: &mut [u32],
    buffer_width: u32,
    buffer_height: u32,
    text_renderer: &TextRenderer,
    params: &TrayGridRenderParams<'_>,
) {
    let bg_u32 = params.bg_color.to_argb_u32();
    let fg_u32 = params.fg_color.to_argb_u32();

    pixels.fill(bg_u32);

    // Draw 1px border around popup
    let border_color = fg_u32;
    for x in 0..buffer_width {
        pixels[x as usize] = border_color;
        let bottom_idx = ((buffer_height - 1) * buffer_width + x) as usize;
        if bottom_idx < pixels.len() {
            pixels[bottom_idx] = border_color;
        }
    }
    for y in 0..buffer_height {
        pixels[(y * buffer_width) as usize] = border_color;
        let right_idx = (y * buffer_width + (buffer_width - 1)) as usize;
        if right_idx < pixels.len() {
            pixels[right_idx] = border_color;
        }
    }

    if params.layout.items.is_empty() {
        let msg = "No tray items";
        let (tw, th) = text_renderer.measure_text(&TextRenderParams {
            text: msg,
            font_family: params.font_family,
            font_size_pt: params.font_size,
            scale: params.scale,
            dest_x: 0,
            dest_y: 0,
            fg_color: params.fg_color,
        });

        let dest_x = (buffer_width as i32 - tw as i32) / 2;
        let dest_y = (buffer_height as i32 - th as i32) / 2;

        text_renderer.render_text(
            pixels,
            buffer_width,
            buffer_height,
            &TextRenderParams {
                text: msg,
                font_family: params.font_family,
                font_size_pt: params.font_size,
                scale: params.scale,
                dest_x,
                dest_y,
                fg_color: params.fg_color,
            },
        );
        return;
    }

    for (idx, placement) in params.layout.items.iter().enumerate() {
        let is_hovered = params.hover_index == Some(idx);
        let (px, py, pw, ph) = logical_rect_to_buffer(
            placement.rect.x,
            placement.rect.y,
            placement.rect.width,
            placement.rect.height,
            params.scale,
        );

        if is_hovered {
            for dy in 0..ph {
                let row_y = py + dy as i32;
                if row_y < 0 || row_y >= buffer_height as i32 {
                    continue;
                }
                for dx in 0..pw {
                    let col_x = px + dx as i32;
                    if col_x < 0 || col_x >= buffer_width as i32 {
                        continue;
                    }
                    let pidx = (row_y as u32 * buffer_width + col_x as u32) as usize;
                    if pidx < pixels.len() {
                        pixels[pidx] = fg_u32;
                    }
                }
            }
        }

        let (ipx, ipy, ipw, iph) = logical_rect_to_buffer(
            placement.icon_rect.x,
            placement.icon_rect.y,
            placement.icon_rect.width,
            placement.icon_rect.height,
            params.scale,
        );

        let icon_source = placement.item.resolve_icon(24, params.scale, None);
        match icon_source {
            TrayIconSource::Pixmap(pixmap) => {
                blit_pixmap(
                    pixels,
                    buffer_width,
                    buffer_height,
                    &pixmap,
                    Rect::new(ipx, ipy, ipw, iph),
                );
            }
            TrayIconSource::Name(name) => {
                // This item's theme icon name could not be resolved by resolve_icon
                // (which already retried the standard sizes). Render a neutral
                // glyph instead of routing the raw theme name through the alias
                // table (that would show the broken-widget "⚠" marker).
                tracing::debug!(
                    "tray item icon '{}' unavailable in theme; using fallback glyph",
                    name
                );
                let glyph =
                    crate::render::icons::resolve_icon(crate::dbus::tray_icons::FALLBACK_TRAY_ICON);
                let (gw, gh) = text_renderer.measure_text(&TextRenderParams {
                    text: &glyph,
                    font_family: params.font_family,
                    font_size_pt: params.font_size * 1.2,
                    scale: params.scale,
                    dest_x: 0,
                    dest_y: 0,
                    fg_color: if is_hovered {
                        params.bg_color
                    } else {
                        params.fg_color
                    },
                });
                let dest_x = ipx + (ipw as i32 - gw as i32) / 2;
                let dest_y = ipy + (iph as i32 - gh as i32) / 2;
                text_renderer.render_text(
                    pixels,
                    buffer_width,
                    buffer_height,
                    &TextRenderParams {
                        text: &glyph,
                        font_family: params.font_family,
                        font_size_pt: params.font_size * 1.2,
                        scale: params.scale,
                        dest_x,
                        dest_y,
                        fg_color: if is_hovered {
                            params.bg_color
                        } else {
                            params.fg_color
                        },
                    },
                );
            }
            TrayIconSource::Glyph(name) => {
                let glyph = crate::render::icons::resolve_icon(&name);
                let (gw, gh) = text_renderer.measure_text(&TextRenderParams {
                    text: &glyph,
                    font_family: params.font_family,
                    font_size_pt: params.font_size * 1.2,
                    scale: params.scale,
                    dest_x: 0,
                    dest_y: 0,
                    fg_color: if is_hovered {
                        params.bg_color
                    } else {
                        params.fg_color
                    },
                });
                let dest_x = ipx + (ipw as i32 - gw as i32) / 2;
                let dest_y = ipy + (iph as i32 - gh as i32) / 2;
                text_renderer.render_text(
                    pixels,
                    buffer_width,
                    buffer_height,
                    &TextRenderParams {
                        text: &glyph,
                        font_family: params.font_family,
                        font_size_pt: params.font_size * 1.2,
                        scale: params.scale,
                        dest_x,
                        dest_y,
                        fg_color: if is_hovered {
                            params.bg_color
                        } else {
                            params.fg_color
                        },
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tray_filtering() {
        let mut config = TrayConfig::default();
        config.hidden.push("discord".to_string());

        let widget = TrayWidget::new("systray", config);

        let mut item1 = SniItem::new(":1.1".to_string(), "/StatusNotifierItem".to_string());
        item1.id = "nm-applet".to_string();
        item1.title = "Network".to_string();

        let mut item2 = SniItem::new(":1.2".to_string(), "/StatusNotifierItem".to_string());
        item2.id = "discord".to_string();
        item2.title = "Discord".to_string();

        widget.handle_event(TrayEvent::Added(item1));
        widget.handle_event(TrayEvent::Added(item2));

        let visible = widget.visible_items();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "nm-applet");
    }

    #[test]
    fn test_tray_attention_state() {
        let widget_config = TrayConfig::default();
        let widget = TrayWidget::new("systray", widget_config);

        let mut item = SniItem::new(":1.1".to_string(), "/StatusNotifierItem".to_string());
        item.id = "chat".to_string();
        item.status = SniStatus::Active;

        widget.handle_event(TrayEvent::Added(item.clone()));
        assert!(!widget.has_attention.load(Ordering::Relaxed));

        // Update item with NeedsAttention
        item.status = SniStatus::NeedsAttention;
        widget.handle_event(TrayEvent::Changed(item));
        assert!(widget.has_attention.load(Ordering::Relaxed));

        let state = widget.state();
        assert_eq!(state.spans.len(), 1);
        assert!(state.spans[0].is_emphasized());
    }

    #[test]
    fn test_tray_grid_layout_and_render() {
        let renderer = TextRenderer::new();
        let items = vec![
            SniItem::new(":1.1".to_string(), "/StatusNotifierItem".to_string()),
            SniItem::new(":1.2".to_string(), "/StatusNotifierItem".to_string()),
        ];

        let layout = layout_tray_grid(&items, &renderer, "sans-serif", 13.0);
        assert_eq!(layout.items.len(), 2);
        assert!(layout.width > 0);
        assert!(layout.height > 0);

        // Hit test first item
        let first_rect = layout.items[0].rect;
        assert_eq!(layout.hit_test(first_rect.x + 2, first_rect.y + 2), Some(0));
        assert_eq!(layout.hit_test(9999, 9999), None);

        // Test render
        let mut pixels = vec![0u32; (layout.width * layout.height) as usize];
        let params = TrayGridRenderParams {
            items: &items,
            layout: &layout,
            hover_index: Some(0),
            font_family: "sans-serif",
            font_size: 13.0,
            scale: 1.0,
            bg_color: Color::rgb(30, 30, 46),
            fg_color: Color::rgb(205, 214, 244),
        };

        render_tray_grid(&mut pixels, layout.width, layout.height, &renderer, &params);
        assert!(pixels.iter().any(|&p| p != 0));
    }

    #[test]
    fn test_sni_iconless_item_falls_back_to_neutral_glyph() {
        let item = SniItem::new(":1.9".to_string(), "/StatusNotifierItem".to_string());
        let source = item.resolve_icon(24, 1.0, None);
        assert_eq!(source, TrayIconSource::Glyph("fa-circle-dot".to_string()));
    }

    #[test]
    fn test_missing_theme_icon_name_does_not_hit_alias_fallback() {
        let mut item = SniItem::new(":1.10".to_string(), "/StatusNotifierItem".to_string());
        item.icon_name = Some("definitely-not-a-real-icon-xyz".to_string());
        let source = item.resolve_icon(24, 1.0, None);
        match source {
            // If themes are unavailable in the test environment the name is
            // handed back for the renderer to substitute the neutral glyph.
            TrayIconSource::Name(n) => assert_eq!(n, "definitely-not-a-real-icon-xyz"),
            // A pixmap means a theme lookup succeeded (local icon theme present).
            TrayIconSource::Pixmap(_) => {}
            other => panic!("unexpected source: {other:?}"),
        }
    }

    #[test]
    fn test_fallback_glyph_renders_without_warning_marker() {
        // The renderer resolves the fallback constant through the alias table;
        // it must be a known alias so "⚠" can never appear for missing icons.
        assert_eq!(
            crate::render::icons::resolve_icon(crate::dbus::tray_icons::FALLBACK_TRAY_ICON),
            "\u{f192}"
        );
    }

    #[test]
    fn test_summary_and_attention_icons_resolve() {
        let config = TrayConfig::default();
        assert_ne!(
            crate::render::icons::resolve_icon(&config.summary_icon),
            "⚠",
            "default summary icon must be a known alias"
        );
        assert_ne!(
            crate::render::icons::resolve_icon(&config.attention_icon),
            "⚠",
            "default attention icon must be a known alias"
        );
    }
}
