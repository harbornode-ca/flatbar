//! Layout engine for Left / Center / Right sections and pointer hit-testing.

use crate::config::LayoutConfig;
use crate::render::icons::resolve_icon;
use crate::render::text::{TextRenderParams, TextRenderer};
use crate::widget::{Span, SpanKind, Widget, WidgetState};
use std::collections::HashMap;

/// A 2D rectangle in logical pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }
}

/// Placement information for a single span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanPlacement {
    pub span: Span,
    pub display_text: String,
    pub rect: Rect,
}

/// Placement information for an entire widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetPlacement {
    pub id: String,
    pub rect: Rect,
    pub spans: Vec<SpanPlacement>,
    pub emphasis: bool,
}

/// Complete computed layout for the bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutResult {
    pub left: Vec<WidgetPlacement>,
    pub center: Vec<WidgetPlacement>,
    pub right: Vec<WidgetPlacement>,
    pub bar_width: u32,
    pub bar_height: u32,
}

impl LayoutResult {
    /// Hit-test a logical coordinate `(x, y)` against all widgets in the layout.
    pub fn hit_test(&self, x: i32, y: i32) -> Option<&WidgetPlacement> {
        self.left
            .iter()
            .chain(self.center.iter())
            .chain(self.right.iter())
            .find(|wp| wp.rect.contains(x, y))
    }

    /// Iterate over all widget placements.
    pub fn all_widgets(&self) -> impl Iterator<Item = &WidgetPlacement> {
        self.left
            .iter()
            .chain(self.center.iter())
            .chain(self.right.iter())
    }
}

/// Compute the full bar layout given widget states, font metrics, and dimensions.
#[allow(clippy::too_many_arguments)]
pub fn compute_layout(
    layout_config: &LayoutConfig,
    widgets: &[Box<dyn Widget>],
    text_renderer: &TextRenderer,
    font_family: &str,
    font_size: f32,
    bar_width: u32,
    bar_height: u32,
    inner_padding: u32,
) -> LayoutResult {
    let mut widget_map: HashMap<&str, WidgetState> = HashMap::new();
    for w in widgets {
        widget_map.insert(w.id(), w.state());
    }

    let h_padding = 12i32;
    let widget_gap = 14i32;
    let span_gap = inner_padding as i32;

    // Helper to measure and create widget placement
    let measure_widget = |id: &str, state: &WidgetState| -> WidgetPlacement {
        let mut total_w = 0u32;
        let mut max_h = 0u32;
        let mut span_placements = Vec::new();
        let mut has_emphasis = false;

        for span in &state.spans {
            if span.emphasis {
                has_emphasis = true;
            }

            let display_text = match &span.kind {
                SpanKind::Text(t) => t.clone(),
                SpanKind::Icon(icon) => resolve_icon(icon),
            };

            // Measure span width in logical coordinates
            let params = TextRenderParams {
                text: &display_text,
                font_family,
                font_size_pt: font_size,
                scale: 1.0,
                dest_x: 0,
                dest_y: 0,
                fg_color: crate::config::Color::rgb(255, 255, 255),
            };
            let (sw, sh) = text_renderer.measure_text(&params);

            let span_rect = Rect::new(0, 0, sw, sh);
            span_placements.push(SpanPlacement {
                span: span.clone(),
                display_text,
                rect: span_rect,
            });

            total_w += sw + span_gap as u32;
            if sh > max_h {
                max_h = sh;
            }
        }

        if !span_placements.is_empty() {
            total_w = total_w.saturating_sub(span_gap as u32);
        }

        WidgetPlacement {
            id: id.to_string(),
            rect: Rect::new(0, 0, total_w, max_h.max(bar_height)),
            spans: span_placements,
            emphasis: has_emphasis,
        }
    };

    // 1. Compute Left section
    let mut left_placements = Vec::new();
    let mut cur_x = h_padding;
    for id in &layout_config.left {
        if let Some(state) = widget_map.get(id.as_str()) {
            if state.is_empty() {
                continue;
            }
            let mut wp = measure_widget(id, state);
            wp.rect.x = cur_x;
            wp.rect.y = 0;
            wp.rect.height = bar_height;

            // Position spans inside widget
            let mut span_x = cur_x;
            for sp in &mut wp.spans {
                sp.rect.x = span_x;
                sp.rect.y = ((bar_height as i32 - sp.rect.height as i32) / 2).max(0);
                span_x += (sp.rect.width as i32) + span_gap;
            }

            cur_x += (wp.rect.width as i32) + widget_gap;
            left_placements.push(wp);
        }
    }
    let left_end_x = cur_x;

    // 2. Compute Right section (positioned from right edge backwards)
    let mut right_placements = Vec::new();
    let mut cur_right_x = bar_width as i32 - h_padding;
    for id in layout_config.right.iter().rev() {
        if let Some(state) = widget_map.get(id.as_str()) {
            if state.is_empty() {
                continue;
            }
            let mut wp = measure_widget(id, state);
            cur_right_x -= wp.rect.width as i32;
            wp.rect.x = cur_right_x;
            wp.rect.y = 0;
            wp.rect.height = bar_height;

            // Position spans inside widget
            let mut span_x = cur_right_x;
            for sp in &mut wp.spans {
                sp.rect.x = span_x;
                sp.rect.y = ((bar_height as i32 - sp.rect.height as i32) / 2).max(0);
                span_x += (sp.rect.width as i32) + span_gap;
            }

            cur_right_x -= widget_gap;
            right_placements.push(wp);
        }
    }
    right_placements.reverse();
    let right_start_x = cur_right_x + widget_gap;

    // 3. Compute Center section
    let mut center_measured = Vec::new();
    let mut total_center_w = 0u32;
    for id in &layout_config.center {
        if let Some(state) = widget_map.get(id.as_str()) {
            if state.is_empty() {
                continue;
            }
            let wp = measure_widget(id, state);
            total_center_w += wp.rect.width + widget_gap as u32;
            center_measured.push(wp);
        }
    }
    if !center_measured.is_empty() {
        total_center_w = total_center_w.saturating_sub(widget_gap as u32);
    }

    let mut center_placements = Vec::new();
    let mut center_start_x = ((bar_width as i32 - total_center_w as i32) / 2).max(left_end_x);

    // If center overflows into right section, clamp
    if center_start_x + total_center_w as i32 > right_start_x {
        center_start_x = (right_start_x - total_center_w as i32).max(left_end_x);
    }

    let mut cur_center_x = center_start_x;
    for mut wp in center_measured {
        wp.rect.x = cur_center_x;
        wp.rect.y = 0;
        wp.rect.height = bar_height;

        let mut span_x = cur_center_x;
        for sp in &mut wp.spans {
            sp.rect.x = span_x;
            sp.rect.y = ((bar_height as i32 - sp.rect.height as i32) / 2).max(0);
            span_x += (sp.rect.width as i32) + span_gap;
        }

        cur_center_x += (wp.rect.width as i32) + widget_gap;
        center_placements.push(wp);
    }

    LayoutResult {
        left: left_placements,
        center: center_placements,
        right: right_placements,
        bar_width,
        bar_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::{Span, Widget, WidgetState};
    use std::time::Duration;

    struct StaticWidget {
        id: &'static str,
        text: &'static str,
    }

    impl Widget for StaticWidget {
        fn id(&self) -> &str {
            self.id
        }
        fn update_interval(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn state(&self) -> WidgetState {
            WidgetState::new(vec![Span::text(self.text)])
        }
    }

    #[test]
    fn test_compute_layout_three_sections() {
        let renderer = TextRenderer::new();
        let layout_cfg = LayoutConfig {
            left: vec!["left_w".to_string()],
            center: vec!["center_w".to_string()],
            right: vec!["right_w".to_string()],
        };

        let widgets: Vec<Box<dyn Widget>> = vec![
            Box::new(StaticWidget {
                id: "left_w",
                text: "LEFT",
            }),
            Box::new(StaticWidget {
                id: "center_w",
                text: "CENTER",
            }),
            Box::new(StaticWidget {
                id: "right_w",
                text: "RIGHT",
            }),
        ];

        let result = compute_layout(
            &layout_cfg,
            &widgets,
            &renderer,
            "sans-serif",
            13.0,
            1000,
            30,
            6,
        );

        assert_eq!(result.left.len(), 1);
        assert_eq!(result.center.len(), 1);
        assert_eq!(result.right.len(), 1);

        assert!(result.left[0].rect.x < result.center[0].rect.x);
        assert!(result.center[0].rect.x < result.right[0].rect.x);

        // Hit-test verification
        let hit_left = result.hit_test(result.left[0].rect.x + 2, 15);
        assert_eq!(hit_left.map(|w| w.id.as_str()), Some("left_w"));

        let hit_center = result.hit_test(result.center[0].rect.x + 2, 15);
        assert_eq!(hit_center.map(|w| w.id.as_str()), Some("center_w"));

        let hit_right = result.hit_test(result.right[0].rect.x + 2, 15);
        assert_eq!(hit_right.map(|w| w.id.as_str()), Some("right_w"));
    }
}
