use flatbar_core::config::{Color, Config, MenuBackend};
use flatbar_core::render::text::TextRenderer;
use flatbar_core::widget::menu::{layout_menu, render_menu, MenuItem, MenuModel};
use std::path::PathBuf;

#[test]
fn test_menu_model_hierarchy() {
    let sub = vec![
        MenuItem::item("sub1", "Sub Item 1"),
        MenuItem::item("sub2", "Sub Item 2"),
    ];

    let menu = MenuModel::new(vec![
        MenuItem::item_with_icon("play", "Play Media", "fa-bolt"),
        MenuItem::checkbox("mute", "Mute Audio", false),
        MenuItem::Separator,
        MenuItem::submenu("more", "More Options", sub),
    ])
    .with_title("Audio Menu");

    assert_eq!(menu.title.as_deref(), Some("Audio Menu"));
    assert_eq!(menu.items.len(), 4);
    assert_eq!(menu.items[0].id(), Some("play"));
    assert!(menu.items[0].is_enabled());
    assert!(!menu.items[2].is_enabled()); // Separator
    assert_eq!(menu.items[2].id(), None);
}

#[test]
fn test_menu_layout_and_hit_test() {
    let menu = MenuModel::new(vec![
        MenuItem::item("first", "First Action"),
        MenuItem::Separator,
        MenuItem::checkbox("check", "Enable Feature", true),
    ]);

    let renderer = TextRenderer::new();
    let layout = layout_menu(&menu, &renderer, "sans-serif", 13.0);

    assert_eq!(layout.items.len(), 3);
    assert!(layout.width >= 120);
    assert!(layout.height > 0);

    // Hit test item 0
    let y0 = layout.items[0].rect.y + 5;
    assert_eq!(layout.hit_test(30, y0), Some(0));

    // Hit test separator (index 1) -> None
    let y1 = layout.items[1].rect.y + 2;
    assert_eq!(layout.hit_test(30, y1), None);

    // Hit test item 2 (checkbox)
    let y2 = layout.items[2].rect.y + 5;
    assert_eq!(layout.hit_test(30, y2), Some(2));
}

#[test]
fn test_render_menu_buffer_execution() {
    let menu = MenuModel::new(vec![
        MenuItem::item("a", "Alpha"),
        MenuItem::Separator,
        MenuItem::checkbox("b", "Beta", true),
    ]);

    let renderer = TextRenderer::new();
    let layout = layout_menu(&menu, &renderer, "sans-serif", 13.0);

    let width = layout.width;
    let height = layout.height;
    let mut pixels = vec![0u32; (width * height) as usize];

    let render_params = flatbar_core::widget::menu::MenuRenderParams {
        menu: &menu,
        layout: &layout,
        hover_index: Some(0), // Hover on item 0
        font_family: "sans-serif",
        font_size: 13.0,
        scale: 1.0,
        bg_color: Color::rgb(30, 30, 46),
        fg_color: Color::rgb(205, 214, 244),
        scroll_offset: 0,
    };

    render_menu(&mut pixels, width, height, &renderer, &render_params);

    // Verify buffer was rendered (not all zeroes)
    assert!(pixels.iter().any(|&p| p != 0));
}

#[test]
fn test_config_menu_backend_options() {
    let toml_popup = r#"
        [bar]
        name = "test_popup_bar"
        menu_backend = "popup"
    "#;
    let config_popup = Config::parse_str(toml_popup, PathBuf::from("popup.toml")).unwrap();
    assert_eq!(config_popup.bar.menu_backend, MenuBackend::Popup);

    let toml_launcher = r#"
        [bar]
        name = "test_launcher_bar"
        menu_backend = "launcher"
        menu_command = "fuzzel --dmenu"
    "#;
    let config_launcher = Config::parse_str(toml_launcher, PathBuf::from("launcher.toml")).unwrap();
    assert_eq!(config_launcher.bar.menu_backend, MenuBackend::Launcher);
    assert_eq!(
        config_launcher.bar.menu_command.as_deref(),
        Some("fuzzel --dmenu")
    );
}

#[test]
fn test_calendar_popup_rendering() {
    use flatbar_core::widget::calendar::{
        layout_calendar, render_calendar, CalendarHit, CalendarModel, CalendarRenderParams,
        FirstDay,
    };

    let model = CalendarModel::new(2026, 9, FirstDay::Monday, (2026, 9, 12));
    let renderer = TextRenderer::new();
    let layout = layout_calendar(&model, &renderer, "sans-serif", 13.0, None);

    assert!(layout.width >= 150);
    assert!(layout.height >= 150);
    assert_eq!(layout.cells.len(), 30); // Sept 2026 has 30 days

    let mut pixels = vec![0u32; (layout.width * layout.height) as usize];
    let params = CalendarRenderParams {
        model: &model,
        layout: &layout,
        hovered: CalendarHit::None,
        font_family: "sans-serif",
        font_size: 13.0,
        scale: 1.0,
        bg_color: Color::rgb(0, 0, 0),
        fg_color: Color::rgb(255, 255, 255),
    };

    render_calendar(&mut pixels, layout.width, layout.height, &renderer, &params);
    assert!(pixels.iter().any(|&p| p != 0));
}
