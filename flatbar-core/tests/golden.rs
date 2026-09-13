use flatbar_core::config::{Color, LayoutConfig};
use flatbar_core::render::snapshot::{render_snapshot, SnapshotOptions};
use flatbar_core::widget::{Span, Widget, WidgetState};
use std::time::Duration;

struct MockWidget {
    id: String,
    spans: Vec<Span>,
}

impl MockWidget {
    fn new(id: impl Into<String>, spans: Vec<Span>) -> Self {
        Self {
            id: id.into(),
            spans,
        }
    }
}

impl Widget for MockWidget {
    fn id(&self) -> &str {
        &self.id
    }
    fn update_interval(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn state(&self) -> WidgetState {
        WidgetState::new(self.spans.clone())
    }
}

#[test]
fn test_golden_three_section_bar() {
    let layout = LayoutConfig {
        left: vec!["ws".to_string(), "vol".to_string()],
        center: vec!["time".to_string()],
        right: vec!["wifi".to_string(), "bat".to_string()],
    };

    let widgets: Vec<Box<dyn Widget>> = vec![
        Box::new(MockWidget::new(
            "ws",
            vec![Span::emphasized_text("1"), Span::text(" 2 3")],
        )),
        Box::new(MockWidget::new(
            "vol",
            vec![Span::icon("fa-volume-high"), Span::text(" 80%")],
        )),
        Box::new(MockWidget::new(
            "time",
            vec![Span::icon("fa-clock"), Span::text(" 14:30")],
        )),
        Box::new(MockWidget::new(
            "wifi",
            vec![Span::icon("fa-wifi"), Span::text(" HomeNet")],
        )),
        Box::new(MockWidget::new(
            "bat",
            vec![Span::icon("fa-battery-full"), Span::text(" 95%")],
        )),
    ];

    let bg = Color::rgb(30, 30, 46);
    let fg = Color::rgb(205, 214, 244);

    let opts = SnapshotOptions {
        font_family: "sans-serif",
        font_size: 13.0,
        scale: 1.0,
        bar_width: 1000,
        bar_height: 30,
        inner_padding: 6,
        bg_color: bg,
        fg_color: fg,
    };

    let img = render_snapshot(&layout, &widgets, &opts);

    assert_eq!(img.width(), 1000);
    assert_eq!(img.height(), 30);

    // Verify background and foreground pixels exist in rendered snapshot
    let mut fg_pixels = 0;
    let mut bg_pixels = 0;
    for pixel in img.pixels() {
        if pixel[0] == 30 && pixel[1] == 30 && pixel[2] == 46 {
            bg_pixels += 1;
        } else if pixel[0] > 100 && pixel[1] > 100 {
            fg_pixels += 1;
        }
    }

    assert!(bg_pixels > 0, "Background pixels must be present");
    assert!(fg_pixels > 0, "Foreground pixels must be present");
}

#[test]
fn test_golden_emphasis_inverted_rendering() {
    let layout = LayoutConfig {
        left: vec!["normal".to_string()],
        center: vec!["emp".to_string()],
        right: vec![],
    };

    let widgets: Vec<Box<dyn Widget>> = vec![
        Box::new(MockWidget::new("normal", vec![Span::text("NORMAL")])),
        Box::new(MockWidget::new(
            "emp",
            vec![Span::emphasized_text("EMPHASIS")],
        )),
    ];

    let bg = Color::rgb(0, 0, 0);
    let fg = Color::rgb(255, 255, 255);

    let opts = SnapshotOptions {
        font_family: "sans-serif",
        font_size: 13.0,
        scale: 1.0,
        bar_width: 800,
        bar_height: 30,
        inner_padding: 6,
        bg_color: bg,
        fg_color: fg,
    };

    let img = render_snapshot(&layout, &widgets, &opts);

    // Both pure white (emphasis background fill) and black pixels must exist
    let has_white = img
        .pixels()
        .any(|p| p[0] == 255 && p[1] == 255 && p[2] == 255);
    let has_black = img.pixels().any(|p| p[0] == 0 && p[1] == 0 && p[2] == 0);

    assert!(has_white);
    assert!(has_black);
}

#[test]
fn test_golden_fractional_scale() {
    let layout = LayoutConfig {
        left: vec!["w".to_string()],
        center: vec![],
        right: vec![],
    };

    let widgets: Vec<Box<dyn Widget>> = vec![Box::new(MockWidget::new(
        "w",
        vec![Span::icon("fa-wifi"), Span::text(" HiDPI")],
    ))];

    let bg = Color::rgb(20, 20, 20);
    let fg = Color::rgb(220, 220, 220);

    let opts = SnapshotOptions {
        font_family: "sans-serif",
        font_size: 13.0,
        scale: 1.5,
        bar_width: 400,
        bar_height: 30,
        inner_padding: 6,
        bg_color: bg,
        fg_color: fg,
    };

    let img = render_snapshot(&layout, &widgets, &opts);

    // Physical dimensions must be ceil(400 * 1.5) = 600, ceil(30 * 1.5) = 45
    assert_eq!(img.width(), 600);
    assert_eq!(img.height(), 45);
}
