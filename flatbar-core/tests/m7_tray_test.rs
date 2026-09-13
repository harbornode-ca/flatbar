use flatbar_core::config::Config;
use flatbar_core::dbus::dbusmenu::{dbusmenu_to_menu_model, sanitize_label, DBusMenuItem};
use flatbar_core::dbus::sni::{SniItem, SniStatus, TrayEvent};
use flatbar_core::dbus::tray_icons::{rasterize_svg_bytes, TrayIconPixmap};
use flatbar_core::shell::build_widgets;
use flatbar_core::widget::menu::MenuItem;
use flatbar_core::widget::tray::{TrayConfig, TrayWidget};
use flatbar_core::widget::Widget;
use std::collections::HashMap;
use std::path::PathBuf;
use zbus::zvariant::{OwnedValue, Value};

#[test]
fn test_tray_pixmap_decode_and_premultiplied_alpha() {
    // 3x1 ARGB pixmap in network byte order: (Alpha, Red, Green, Blue)
    let raw_bytes: Vec<u8> = vec![
        255, 255, 128, 0, // Opaque Orange
        128, 0, 128, 255, // 50% Alpha Blue/Purple
        0, 0, 0, 0, // Fully Transparent
    ];

    let pixmap = TrayIconPixmap::from_sni_pixmap(3, 1, &raw_bytes).expect("Failed to parse pixmap");
    assert_eq!(pixmap.width, 3);
    assert_eq!(pixmap.height, 1);
    assert_eq!(pixmap.pixels.len(), 3);
    assert_eq!(pixmap.pixels[0], 0xFFFF8000);
    assert_eq!(pixmap.pixels[1], 0x800080FF);
    assert_eq!(pixmap.pixels[2], 0x00000000);

    // Invalid dimensions return None
    assert!(TrayIconPixmap::from_sni_pixmap(0, 5, &raw_bytes).is_none());
    assert!(TrayIconPixmap::from_sni_pixmap(3, 2, &raw_bytes).is_none()); // Not enough bytes
}

#[test]
fn test_tray_pixmap_best_size_selection() {
    let p16 = (16, 16, vec![0u8; 16 * 16 * 4]);
    let p24 = (24, 24, vec![0u8; 24 * 24 * 4]);
    let p32 = (32, 32, vec![0u8; 32 * 32 * 4]);
    let p48 = (48, 48, vec![0u8; 48 * 48 * 4]);

    let list = vec![p16, p24, p32, p48];

    let chosen16 = TrayIconPixmap::select_best_pixmap(&list, 16).unwrap();
    assert_eq!(chosen16.width, 16);

    let chosen22 = TrayIconPixmap::select_best_pixmap(&list, 22).unwrap();
    assert_eq!(chosen22.width, 24);

    let chosen64 = TrayIconPixmap::select_best_pixmap(&list, 64).unwrap();
    assert_eq!(chosen64.width, 48);
}

#[test]
fn test_svg_icon_rasterization() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24">
        <circle cx="12" cy="12" r="10" fill="#4285F4"/>
    </svg>"##;

    let pixmap = rasterize_svg_bytes(svg.as_bytes(), 24, 24).expect("SVG rasterization failed");
    assert_eq!(pixmap.width, 24);
    assert_eq!(pixmap.height, 24);
    assert_eq!(pixmap.pixels.len(), 24 * 24);

    // Center pixel should be filled (non-zero)
    let center_pixel = pixmap.pixels[12 * 24 + 12];
    assert!(center_pixel & 0xFF000000 != 0); // Non-transparent alpha
}

#[test]
fn test_dbusmenu_label_sanitization() {
    assert_eq!(sanitize_label("_File"), "File");
    assert_eq!(sanitize_label("E_xit Application"), "Exit Application");
    assert_eq!(sanitize_label("Save __ All"), "Save _ All");
    assert_eq!(sanitize_label("No Mnemonics"), "No Mnemonics");
}

#[test]
fn test_dbusmenu_layout_tree_conversion() {
    let mut root_props = HashMap::new();
    root_props.insert(
        "children-display".to_string(),
        OwnedValue::try_from(Value::from("submenu")).unwrap(),
    );

    let mut item1 = HashMap::new();
    item1.insert(
        "label".to_string(),
        OwnedValue::try_from(Value::from("_Settings")).unwrap(),
    );
    item1.insert(
        "icon-name".to_string(),
        OwnedValue::try_from(Value::from("preferences-system")).unwrap(),
    );
    item1.insert(
        "enabled".to_string(),
        OwnedValue::try_from(Value::from(true)).unwrap(),
    );

    let mut item2 = HashMap::new();
    item2.insert(
        "type".to_string(),
        OwnedValue::try_from(Value::from("separator")).unwrap(),
    );

    let mut item3 = HashMap::new();
    item3.insert(
        "label".to_string(),
        OwnedValue::try_from(Value::from("Autostart")).unwrap(),
    );
    item3.insert(
        "toggle-type".to_string(),
        OwnedValue::try_from(Value::from("checkmark")).unwrap(),
    );
    item3.insert(
        "toggle-state".to_string(),
        OwnedValue::try_from(Value::from(1i32)).unwrap(),
    );

    let mut sub_item = HashMap::new();
    sub_item.insert(
        "label".to_string(),
        OwnedValue::try_from(Value::from("Advanced Sub")).unwrap(),
    );

    let root = DBusMenuItem {
        id: 0,
        properties: root_props,
        children: vec![
            DBusMenuItem {
                id: 1,
                properties: item1,
                children: Vec::new(),
            },
            DBusMenuItem {
                id: 2,
                properties: item2,
                children: Vec::new(),
            },
            DBusMenuItem {
                id: 3,
                properties: item3,
                children: Vec::new(),
            },
            DBusMenuItem {
                id: 4,
                properties: {
                    let mut p = HashMap::new();
                    p.insert(
                        "label".to_string(),
                        OwnedValue::try_from(Value::from("More")).unwrap(),
                    );
                    p.insert(
                        "children-display".to_string(),
                        OwnedValue::try_from(Value::from("submenu")).unwrap(),
                    );
                    p
                },
                children: vec![DBusMenuItem {
                    id: 5,
                    properties: sub_item,
                    children: Vec::new(),
                }],
            },
        ],
    };

    let model = dbusmenu_to_menu_model(&root);
    assert_eq!(model.items.len(), 4);

    // 1. Action Item
    assert_eq!(model.items[0].label(), Some("Settings"));
    assert_eq!(model.items[0].icon(), Some("preferences-system"));

    // 2. Separator
    assert!(model.items[1].is_separator());

    // 3. Checkbox
    match &model.items[2] {
        MenuItem::Checkbox { label, checked, .. } => {
            assert_eq!(label, "Autostart");
            assert!(*checked);
        }
        _ => panic!("Expected checkbox item"),
    }

    // 4. SubMenu
    match &model.items[3] {
        MenuItem::SubMenu { label, items, .. } => {
            assert_eq!(label, "More");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].label(), Some("Advanced Sub"));
        }
        _ => panic!("Expected submenu"),
    }
}

#[test]
fn test_tray_widget_lifecycle_and_filtering() {
    let config = TrayConfig {
        hidden: vec!["spotify".to_string(), "hidden_app".to_string()],
        summary_icon: "fa-chevron-up".to_string(),
        ..Default::default()
    };

    let widget = TrayWidget::new("systray", config);

    // Initial state: empty tray
    let state0 = widget.state();
    assert_eq!(state0.spans.len(), 1);
    assert_eq!(state0.spans[0].as_icon(), Some("fa-chevron-up"));
    assert!(!state0.spans[0].is_emphasized());
    assert_eq!(state0.tooltip.as_deref(), Some("System Tray (empty)"));

    // Add item 1 (allowed)
    let mut item1 = SniItem::new(":1.10".to_string(), "/StatusNotifierItem".to_string());
    item1.id = "nm-applet".to_string();
    item1.title = "Network Manager".to_string();
    item1.status = SniStatus::Active;
    item1.icon_name = Some("network-wireless".to_string());
    widget.handle_event(TrayEvent::Added(item1));

    // Add item 2 (blacklisted)
    let mut item2 = SniItem::new(":1.20".to_string(), "/StatusNotifierItem".to_string());
    item2.id = "spotify".to_string();
    item2.title = "Spotify".to_string();
    widget.handle_event(TrayEvent::Added(item2));

    let visible = widget.visible_items();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, "nm-applet");

    let state1 = widget.state();
    assert!(state1.tooltip.unwrap().contains("Network Manager"));

    // Item 1 triggers NeedsAttention
    let mut item1_updated = visible[0].clone();
    item1_updated.status = SniStatus::NeedsAttention;
    widget.handle_event(TrayEvent::Changed(item1_updated));

    let state_att = widget.state();
    assert!(state_att.spans[0].is_emphasized());
    assert_eq!(state_att.spans[0].as_icon(), Some("fa-chevron-down"));

    // Remove item 1
    widget.handle_event(TrayEvent::Removed(":1.10/StatusNotifierItem".to_string()));
    assert_eq!(widget.visible_items().len(), 0);

    let state_after = widget.state();
    assert!(!state_after.spans[0].is_emphasized());
}

#[test]
fn test_tray_menu_model_generation() {
    let widget = TrayWidget::new("systray", TrayConfig::default());

    // Empty model
    let empty_menu = widget.build_tray_menu();
    assert_eq!(empty_menu.items.len(), 1);
    assert_eq!(empty_menu.items[0].label(), Some("No tray items"));

    // Add items
    let mut item = SniItem::new(":1.50".to_string(), "/StatusNotifierItem".to_string());
    item.id = "volume_control".to_string();
    item.title = "Volume".to_string();
    item.icon_name = Some("audio-volume-high".to_string());
    widget.handle_event(TrayEvent::Added(item));

    let menu = widget.build_tray_menu();
    assert_eq!(menu.items.len(), 1);
    assert_eq!(menu.items[0].label(), Some("Volume"));
    assert_eq!(menu.items[0].icon(), Some("audio-volume-high"));
}

#[test]
fn test_config_builder_with_tray_widget() {
    let toml_str = r#"
        [bar]
        name = "test_tray_bar"
        layout.right = ["systray"]

        [widgets.systray]
        type = "systray"
        summary_icon = "fa-tray"
        attention_icon = "fa-bell"
        hidden = ["unwanted_app"]
        icon_size = 20
        tray_poll_fallback_secs = 0
        menu_backend = "popup"
    "#;

    let config = Config::parse_str(toml_str, PathBuf::from("test.toml")).unwrap();
    let widgets = build_widgets(&config);

    assert_eq!(widgets.len(), 1);
    assert_eq!(widgets[0].id(), "systray");
    let state = widgets[0].state();
    assert_eq!(state.spans[0].as_icon(), Some("fa-tray"));

    let tray = widgets[0].as_tray().expect("tray widget");
    assert_eq!(tray.fallback_poll_secs(), 0);
}
