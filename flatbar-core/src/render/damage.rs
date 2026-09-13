//! Logical damage tracking and physical damage rectangle conversion according to Spec A.

use crate::render::layout::{LayoutResult, Rect};
use crate::shell::scaling::logical_rect_to_buffer;
use std::collections::HashMap;

/// Damage tracker comparing successive layout frames to determine dirty regions.
#[derive(Debug, Default)]
pub struct DamageTracker {
    prev_widgets: HashMap<String, (Rect, bool, Vec<String>)>, // id -> (rect, emphasis, span_texts)
    full_redraw_needed: bool,
}

impl DamageTracker {
    pub fn new() -> Self {
        Self {
            prev_widgets: HashMap::new(),
            full_redraw_needed: true,
        }
    }

    /// Invalidate the entire surface (e.g. on resize or scale change).
    pub fn invalidate_all(&mut self) {
        self.full_redraw_needed = true;
        self.prev_widgets.clear();
    }

    /// Compute damaged logical rectangles by comparing current layout with previous frame.
    pub fn compute_damage(&mut self, current_layout: &LayoutResult) -> Vec<Rect> {
        if self.full_redraw_needed {
            self.full_redraw_needed = false;
            let mut new_map = HashMap::new();
            for wp in current_layout.all_widgets() {
                let texts = wp.spans.iter().map(|s| s.display_text.clone()).collect();
                new_map.insert(wp.id.clone(), (wp.rect, wp.emphasis, texts));
            }
            self.prev_widgets = new_map;
            return vec![Rect::new(
                0,
                0,
                current_layout.bar_width,
                current_layout.bar_height,
            )];
        }

        let mut dirty_rects = Vec::new();
        let mut new_map = HashMap::new();

        for wp in current_layout.all_widgets() {
            let texts: Vec<String> = wp.spans.iter().map(|s| s.display_text.clone()).collect();

            if let Some((old_rect, old_emp, old_texts)) = self.prev_widgets.remove(&wp.id) {
                if old_rect != wp.rect || old_emp != wp.emphasis || old_texts != texts {
                    // Damaged: add old and new rect
                    dirty_rects.push(old_rect);
                    dirty_rects.push(wp.rect);
                }
            } else {
                // Newly added widget
                dirty_rects.push(wp.rect);
            }

            new_map.insert(wp.id.clone(), (wp.rect, wp.emphasis, texts));
        }

        // Any widgets removed since last frame
        for (_, (removed_rect, _, _)) in self.prev_widgets.drain() {
            dirty_rects.push(removed_rect);
        }

        self.prev_widgets = new_map;
        dirty_rects
    }

    /// Convert logical damage rects to buffer damage coordinates.
    pub fn to_buffer_damage(logical_rects: &[Rect], scale: f64) -> Vec<(i32, i32, u32, u32)> {
        logical_rects
            .iter()
            .map(|r| logical_rect_to_buffer(r.x, r.y, r.width, r.height, scale))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::layout::WidgetPlacement;

    #[test]
    fn test_damage_tracker_initial_full_redraw() {
        let mut tracker = DamageTracker::new();
        let layout = LayoutResult {
            bar_width: 1000,
            bar_height: 30,
            ..Default::default()
        };

        let damage = tracker.compute_damage(&layout);
        assert_eq!(damage.len(), 1);
        assert_eq!(damage[0], Rect::new(0, 0, 1000, 30));
    }

    #[test]
    fn test_damage_tracker_widget_update() {
        let mut tracker = DamageTracker::new();
        let layout1 = LayoutResult {
            bar_width: 1000,
            bar_height: 30,
            left: vec![WidgetPlacement {
                id: "w1".to_string(),
                rect: Rect::new(10, 0, 50, 30),
                spans: vec![],
                emphasis: false,
            }],
            ..Default::default()
        };
        let _ = tracker.compute_damage(&layout1);

        // Frame 2 with changed state
        let layout2 = LayoutResult {
            bar_width: 1000,
            bar_height: 30,
            left: vec![WidgetPlacement {
                id: "w1".to_string(),
                rect: Rect::new(10, 0, 60, 30),
                spans: vec![],
                emphasis: false,
            }],
            ..Default::default()
        };

        let damage = tracker.compute_damage(&layout2);
        assert!(!damage.is_empty());
        assert!(damage.contains(&Rect::new(10, 0, 50, 30)));
        assert!(damage.contains(&Rect::new(10, 0, 60, 30)));
    }
}
