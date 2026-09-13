use crate::config::Color;
use crate::render::icons::{resolve_icon_or, FALLBACK_UNKNOWN_GLYPH};
use crate::render::layout::Rect;
use crate::render::text::{TextRenderParams, TextRenderer};
use crate::shell::scaling::logical_rect_to_buffer;

/// An item within a menu model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Item {
        id: String,
        label: String,
        icon: Option<String>,
        enabled: bool,
    },
    Checkbox {
        id: String,
        label: String,
        checked: bool,
        enabled: bool,
    },
    Separator,
    SubMenu {
        id: String,
        label: String,
        icon: Option<String>,
        items: Vec<MenuItem>,
    },
}

impl MenuItem {
    /// Builder-style helper to disable an item (unclickable in the popup).
    pub fn disabled(mut self, disabled: bool) -> Self {
        if let MenuItem::Item { enabled, .. } = &mut self {
            *enabled = !disabled
        }
        self
    }
    pub fn item(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::Item {
            id: id.into(),
            label: label.into(),
            icon: None,
            enabled: true,
        }
    }

    pub fn item_with_icon(
        id: impl Into<String>,
        label: impl Into<String>,
        icon: impl Into<String>,
    ) -> Self {
        Self::Item {
            id: id.into(),
            label: label.into(),
            icon: Some(icon.into()),
            enabled: true,
        }
    }

    pub fn checkbox(id: impl Into<String>, label: impl Into<String>, checked: bool) -> Self {
        Self::Checkbox {
            id: id.into(),
            label: label.into(),
            checked,
            enabled: true,
        }
    }

    pub fn submenu(id: impl Into<String>, label: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self::SubMenu {
            id: id.into(),
            label: label.into(),
            icon: None,
            items,
        }
    }

    pub fn is_separator(&self) -> bool {
        matches!(self, Self::Separator)
    }

    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Item { enabled, .. } | Self::Checkbox { enabled, .. } => *enabled,
            Self::Separator => false,
            Self::SubMenu { items, .. } => !items.is_empty(),
        }
    }

    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Item { id, .. } | Self::Checkbox { id, .. } | Self::SubMenu { id, .. } => {
                Some(id.as_str())
            }
            Self::Separator => None,
        }
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Item { label, .. }
            | Self::Checkbox { label, .. }
            | Self::SubMenu { label, .. } => Some(label.as_str()),
            Self::Separator => None,
        }
    }

    pub fn icon(&self) -> Option<&str> {
        match self {
            Self::Item { icon, .. } | Self::SubMenu { icon, .. } => icon.as_deref(),
            Self::Checkbox { .. } | Self::Separator => None,
        }
    }
}

/// A structured hierarchical menu definition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuModel {
    pub title: Option<String>,
    pub items: Vec<MenuItem>,
    /// Optional per-popup width override. Widths are content-fit by default
    /// and clamped at ¼ of the output width by the popup layer.
    pub max_width: Option<u32>,
}

impl MenuModel {
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self {
            title: None,
            items,
            max_width: None,
        }
    }

    pub fn with_max_width(mut self, width: u32) -> Self {
        self.max_width = Some(width);
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Item bounding box and metadata in popup layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItemPlacement {
    pub index: usize,
    pub rect: Rect,
    pub is_separator: bool,
    pub is_enabled: bool,
}

/// Layout computation for a menu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuLayout {
    pub width: u32,
    pub height: u32,
    pub items: Vec<MenuItemPlacement>,
}

impl MenuLayout {
    pub fn hit_test(&self, x: i32, y: i32) -> Option<usize> {
        self.items
            .iter()
            .find(|item| !item.is_separator && item.is_enabled && item.rect.contains(x, y))
            .map(|item| item.index)
    }
}

/// Calculate menu geometry and bounding boxes.
pub fn layout_menu(
    menu: &MenuModel,
    text_renderer: &TextRenderer,
    font_family: &str,
    font_size: f32,
) -> MenuLayout {
    layout_menu_with_limits(menu, text_renderer, font_family, font_size, None)
}

/// Calculate menu geometry with optional width clamp (e.g. ¼ output width).
pub fn layout_menu_with_limits(
    menu: &MenuModel,
    text_renderer: &TextRenderer,
    font_family: &str,
    font_size: f32,
    max_width: Option<u32>,
) -> MenuLayout {
    let item_height = (font_size * 2.0).round().max(26.0) as u32;
    let separator_height = 8u32;
    let padding_h = 16u32;
    let padding_v = 8u32;

    let measure = |t: &str| -> u32 {
        let params = TextRenderParams {
            text: t,
            font_family,
            font_size_pt: font_size,
            scale: 1.0,
            dest_x: 0,
            dest_y: 0,
            fg_color: Color::rgb(255, 255, 255),
        };
        text_renderer.measure_text(&params).0
    };

    let mut max_content_width = 120u32;

    for item in &menu.items {
        match item {
            MenuItem::Item { label, icon, .. } => {
                let mut label_w = measure(label);
                if let Some(ic) = icon {
                    let ic_glyph = resolve_icon_or(ic, FALLBACK_UNKNOWN_GLYPH);
                    let ic_w = measure(&ic_glyph);
                    label_w += ic_w + 10;
                }
                max_content_width = max_content_width.max(label_w);
            }
            MenuItem::Checkbox { label, .. } => {
                let label_w = measure(label) + 26;
                max_content_width = max_content_width.max(label_w);
            }
            MenuItem::SubMenu { label, icon, .. } => {
                let mut label_w = measure(label) + 20;
                if let Some(ic) = icon {
                    let ic_glyph = resolve_icon_or(ic, FALLBACK_UNKNOWN_GLYPH);
                    let ic_w = measure(&ic_glyph);
                    label_w += ic_w + 10;
                }
                max_content_width = max_content_width.max(label_w);
            }
            MenuItem::Separator => {}
        }
    }

    let mut menu_width = max_content_width + padding_h * 2;
    if let Some(width_cap) = menu.max_width.max(max_width) {
        if menu_width > width_cap {
            menu_width = width_cap;
        }
    }
    let mut current_y = padding_v as i32;
    let mut placements = Vec::new();

    for (idx, item) in menu.items.iter().enumerate() {
        let h = if item.is_separator() {
            separator_height
        } else {
            item_height
        };

        placements.push(MenuItemPlacement {
            index: idx,
            rect: Rect::new(0, current_y, menu_width, h),
            is_separator: item.is_separator(),
            is_enabled: item.is_enabled(),
        });

        current_y += h as i32;
    }

    let menu_height = (current_y as u32) + padding_v;

    MenuLayout {
        width: menu_width,
        height: menu_height,
        items: placements,
    }
}

/// Parameters for rendering a menu model into a frame buffer.
#[derive(Debug, Clone)]
pub struct MenuRenderParams<'a> {
    pub menu: &'a MenuModel,
    pub layout: &'a MenuLayout,
    pub hover_index: Option<usize>,
    pub font_family: &'a str,
    pub font_size: f32,
    pub scale: f64,
    pub bg_color: Color,
    pub fg_color: Color,
    /// Vertical scroll offset in logical pixels (only used for menus taller
    /// than the popup viewport).
    pub scroll_offset: u32,
}

/// Render the menu into a buffer.
pub fn render_menu(
    pixels: &mut [u32],
    buffer_width: u32,
    buffer_height: u32,
    text_renderer: &TextRenderer,
    params: &MenuRenderParams<'_>,
) {
    // 1. Draw border / background
    let bg_u32 = params.bg_color.to_argb_u32();
    let fg_u32 = params.fg_color.to_argb_u32();

    for p in pixels.iter_mut() {
        *p = bg_u32;
    }

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

    // 2. Render each menu item, shifted up by the scroll offset and clipped
    // to the visible window (render_text already clips at buffer bounds).
    for item_layout in &params.layout.items {
        if item_layout.index >= params.menu.items.len() {
            continue;
        }

        let item = &params.menu.items[item_layout.index];
        let is_hovered = params.hover_index == Some(item_layout.index);

        let (px, py, pw, ph) = logical_rect_to_buffer(
            item_layout.rect.x,
            item_layout.rect.y - params.scroll_offset as i32,
            item_layout.rect.width,
            item_layout.rect.height,
            params.scale,
        );

        if py + (ph as i32) <= 0 || py >= buffer_height as i32 {
            continue;
        }

        if item_layout.is_separator {
            // Draw horizontal separator line
            let mid_y = py + (ph as i32 / 2);
            if mid_y >= 0 && (mid_y as u32) < buffer_height {
                let start_x = (12.0 * params.scale).round() as i32;
                let end_x = buffer_width as i32 - start_x;
                for x in start_x.max(0)..end_x.min(buffer_width as i32) {
                    let idx = (mid_y as u32 * buffer_width + x as u32) as usize;
                    if idx < pixels.len() {
                        pixels[idx] = border_color;
                    }
                }
            }
            continue;
        }

        // Highlight hovered item with inverted background
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
                    let idx = (row_y as u32 * buffer_width + col_x as u32) as usize;
                    if idx < pixels.len() {
                        pixels[idx] = fg_u32;
                    }
                }
            }
        }

        // Render item text and icons
        let item_fg = if is_hovered {
            params.bg_color
        } else {
            params.fg_color
        };

        let x_offset = (16.0 * params.scale).round() as i32;
        let y_offset =
            py + ((ph as i32 - (params.font_size * params.scale as f32) as i32) / 2).max(0);

        match item {
            MenuItem::Item { label, icon, .. } => {
                let mut current_x = px + x_offset;
                if let Some(ic) = icon {
                    let glyph = resolve_icon_or(ic, FALLBACK_UNKNOWN_GLYPH);
                    let t_params = TextRenderParams {
                        text: &glyph,
                        font_family: params.font_family,
                        font_size_pt: params.font_size,
                        scale: params.scale,
                        dest_x: current_x,
                        dest_y: y_offset,
                        fg_color: item_fg,
                    };
                    text_renderer.render_text(pixels, buffer_width, buffer_height, &t_params);
                    current_x += (20.0 * params.scale).round() as i32;
                }

                let t_params = TextRenderParams {
                    text: label,
                    font_family: params.font_family,
                    font_size_pt: params.font_size,
                    scale: params.scale,
                    dest_x: current_x,
                    dest_y: y_offset,
                    fg_color: item_fg,
                };
                text_renderer.render_text(pixels, buffer_width, buffer_height, &t_params);
            }
            MenuItem::Checkbox { label, checked, .. } => {
                let box_label = if *checked { "[x] " } else { "[ ] " };
                let full_text = format!("{box_label}{label}");
                let t_params = TextRenderParams {
                    text: &full_text,
                    font_family: params.font_family,
                    font_size_pt: params.font_size,
                    scale: params.scale,
                    dest_x: px + x_offset,
                    dest_y: y_offset,
                    fg_color: item_fg,
                };
                text_renderer.render_text(pixels, buffer_width, buffer_height, &t_params);
            }
            MenuItem::SubMenu { label, icon, .. } => {
                let mut current_x = px + x_offset;
                if let Some(ic) = icon {
                    let glyph = resolve_icon_or(ic, FALLBACK_UNKNOWN_GLYPH);
                    let t_params = TextRenderParams {
                        text: &glyph,
                        font_family: params.font_family,
                        font_size_pt: params.font_size,
                        scale: params.scale,
                        dest_x: current_x,
                        dest_y: y_offset,
                        fg_color: item_fg,
                    };
                    text_renderer.render_text(pixels, buffer_width, buffer_height, &t_params);
                    current_x += (20.0 * params.scale).round() as i32;
                }

                let full_text = format!("{label} ▶");
                let t_params = TextRenderParams {
                    text: &full_text,
                    font_family: params.font_family,
                    font_size_pt: params.font_size,
                    scale: params.scale,
                    dest_x: current_x,
                    dest_y: y_offset,
                    fg_color: item_fg,
                };
                text_renderer.render_text(pixels, buffer_width, buffer_height, &t_params);
            }
            MenuItem::Separator => {}
        }
    }

    // 3. Scroll indicators: thin bars at the top and bottom edges whenever
    // there is scrollable content beyond the visible window.
    let viewport_logical = ((buffer_height as f64) / params.scale).ceil();
    let min_scrollable_height = (2.0 * params.scale).ceil() as u32 * 2;

    let can_scroll_up = params.scroll_offset > 0;
    let can_scroll_down =
        (params.scroll_offset as f64) + viewport_logical < params.layout.height as f64;

    let indicator_rows = ((3.0 * params.scale).ceil()) as u32;
    let ind_x0 = (12.0 * params.scale).round() as i32;
    let ind_x1 = (buffer_width as i32 - ind_x0).max(ind_x0);

    if can_scroll_up && indicator_rows > 0 && buffer_height >= min_scrollable_height {
        for row in 0..indicator_rows {
            let y = 1 + row as usize;
            for x in ind_x0 as usize..(ind_x1 as usize).min(buffer_width as usize) {
                pixels[y * buffer_width as usize + x] = fg_u32;
            }
        }
    }
    if can_scroll_down && indicator_rows > 0 && buffer_height >= min_scrollable_height {
        for row in 0..indicator_rows {
            let y = (buffer_height - 2 - row) as usize;
            for x in ind_x0 as usize..(ind_x1 as usize).min(buffer_width as usize) {
                pixels[y * buffer_width as usize + x] = border_color;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_menu_model_layout() {
        let menu = MenuModel::new(vec![
            MenuItem::item("item1", "Item One"),
            MenuItem::checkbox("cb1", "Toggle Option", true),
            MenuItem::Separator,
            MenuItem::item("item2", "Item Two"),
        ]);

        let renderer = TextRenderer::new();
        let layout = layout_menu(&menu, &renderer, "sans-serif", 13.0);

        assert_eq!(layout.items.len(), 4);
        assert!(layout.width > 0);
        assert!(layout.height > 0);
        assert!(layout.items[2].is_separator);

        // Hit test first item
        let hit0 = layout.hit_test(20, layout.items[0].rect.y + 2);
        assert_eq!(hit0, Some(0));

        // Hit test separator should be None
        let hit_sep = layout.hit_test(20, layout.items[2].rect.y + 2);
        assert_eq!(hit_sep, None);
    }
}

#[cfg(test)]
mod scroll_limits_tests {
    use super::*;

    #[test]
    fn test_layout_width_cap_quarter_screen() {
        let menu = MenuModel::new(vec![MenuItem::item("a", "A very long menu label that obviously exceeds any quarter-screen width on a tiny test display xxxxxxxxxxxxxxx")]);
        let renderer = TextRenderer::new();
        let layout = layout_menu_with_limits(
            &menu,
            &renderer,
            "sans-serif",
            13.0,
            Some(crate::shell::scaling::quarter_output_width(480)),
        );
        assert!(layout.width <= 480 / 4);
        let item = &layout.items[0];
        assert!(item.rect.width <= 480 / 4);
    }

    #[test]
    fn test_layout_content_fit_without_cap() {
        let menu = MenuModel::new(vec![MenuItem::item("a", "short")]);
        let renderer = TextRenderer::new();
        let layout = layout_menu_with_limits(&menu, &renderer, "sans-serif", 13.0, None);
        assert!(layout.width >= 120); // minimum content-fit width
    }
}
