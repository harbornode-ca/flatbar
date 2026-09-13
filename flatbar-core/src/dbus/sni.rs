//! StatusNotifierItem (SNI) data structures, property representations, and action dispatch.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use crate::dbus::tray_icons::{load_theme_icon, TrayIconPixmap, TrayIconSource};

/// Standard theme sizes tried when the initially requested size has no match.
pub const THEME_FALLBACK_SIZES: [u32; 3] = [16, 24, 32];

/// SNI Item category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SniCategory {
    #[default]
    ApplicationStatus,
    Communications,
    SystemServices,
    Hardware,
    Other,
}

impl std::str::FromStr for SniCategory {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ApplicationStatus" => Self::ApplicationStatus,
            "Communications" => Self::Communications,
            "SystemServices" => Self::SystemServices,
            "Hardware" => Self::Hardware,
            _ => Self::Other,
        })
    }
}

impl SniCategory {
    pub fn parse(s: &str) -> Self {
        s.parse().unwrap_or(Self::Other)
    }
}

/// SNI Item status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SniStatus {
    Passive,
    #[default]
    Active,
    NeedsAttention,
}

impl std::str::FromStr for SniStatus {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "Passive" => Self::Passive,
            "NeedsAttention" => Self::NeedsAttention,
            _ => Self::Active,
        })
    }
}

impl SniStatus {
    pub fn parse(s: &str) -> Self {
        s.parse().unwrap_or(Self::Active)
    }
}

/// A discovered and tracked StatusNotifierItem.
#[derive(Debug, Clone, PartialEq)]
pub struct SniItem {
    pub service: String,
    pub path: String,
    pub id: String,
    pub category: SniCategory,
    pub status: SniStatus,
    pub title: String,
    pub icon_name: Option<String>,
    pub icon_pixmaps: Vec<(i32, i32, Vec<u8>)>,
    pub attention_icon_name: Option<String>,
    pub attention_icon_pixmaps: Vec<(i32, i32, Vec<u8>)>,
    pub tooltip_title: Option<String>,
    pub tooltip_description: Option<String>,
    pub menu_path: Option<String>,
    pub window_id: u32,
}

impl SniItem {
    /// Create a new empty SNI item identifier.
    pub fn new(service: String, path: String) -> Self {
        Self {
            service,
            path,
            id: String::new(),
            category: SniCategory::default(),
            status: SniStatus::default(),
            title: String::new(),
            icon_name: None,
            icon_pixmaps: Vec::new(),
            attention_icon_name: None,
            attention_icon_pixmaps: Vec::new(),
            tooltip_title: None,
            tooltip_description: None,
            menu_path: None,
            window_id: 0,
        }
    }

    /// Resolve the most appropriate icon source for this item.
    pub fn resolve_icon(
        &self,
        target_size: u32,
        scale: f64,
        theme: Option<&str>,
    ) -> TrayIconSource {
        // If status is NeedsAttention, prefer attention icon
        if self.status == SniStatus::NeedsAttention {
            if let Some(att_name) = &self.attention_icon_name {
                if !att_name.is_empty() {
                    if let Some(pix) = load_theme_icon(att_name, target_size, scale, theme) {
                        return TrayIconSource::Pixmap(Arc::new(pix));
                    }
                    return TrayIconSource::Name(att_name.clone());
                }
            }
            if let Some(pix) =
                TrayIconPixmap::select_best_pixmap(&self.attention_icon_pixmaps, target_size)
            {
                return TrayIconSource::Pixmap(Arc::new(pix));
            }
        }

        // Standard icon: check pixmaps first if name is empty, or try theme lookup
        if let Some(name) = &self.icon_name {
            if !name.is_empty() {
                if let Some(pix) = load_theme_icon(name, target_size, scale, theme) {
                    return TrayIconSource::Pixmap(Arc::new(pix));
                }
                // Retry at the neighboring standard theme sizes before giving up.
                for retry in THEME_FALLBACK_SIZES {
                    if retry != target_size {
                        if let Some(pix) = load_theme_icon(name, retry, scale, theme) {
                            return TrayIconSource::Pixmap(Arc::new(pix));
                        }
                    }
                }
                // Theme is unavailable for this name; the renderer falls back to a
                // neutral glyph (see TrayIconSource::Name handling).
                return TrayIconSource::Name(name.clone());
            }
        }

        if let Some(pix) = TrayIconPixmap::select_best_pixmap(&self.icon_pixmaps, target_size) {
            return TrayIconSource::Pixmap(Arc::new(pix));
        }

        // Fallback default tray icon
        TrayIconSource::Glyph(crate::dbus::tray_icons::FALLBACK_TRAY_ICON.to_string())
    }

    /// Read properties from an active DBus connection for this item.
    pub fn fetch_properties(&mut self, conn: &Connection) -> Result<(), zbus::Error> {
        let proxy = zbus::blocking::Proxy::new(
            conn,
            self.service.as_str(),
            self.path.as_str(),
            "org.kde.StatusNotifierItem",
        )?;

        if let Ok(id) = proxy.get_property::<String>("Id") {
            self.id = id;
        }
        if let Ok(cat_str) = proxy.get_property::<String>("Category") {
            self.category = SniCategory::parse(&cat_str);
        }
        if let Ok(stat_str) = proxy.get_property::<String>("Status") {
            self.status = SniStatus::parse(&stat_str);
        }
        if let Ok(title) = proxy.get_property::<String>("Title") {
            self.title = title;
        }
        if let Ok(icon_name) = proxy.get_property::<String>("IconName") {
            if !icon_name.is_empty() {
                self.icon_name = Some(icon_name);
            }
        }
        if let Ok(att_name) = proxy.get_property::<String>("AttentionIconName") {
            if !att_name.is_empty() {
                self.attention_icon_name = Some(att_name);
            }
        }
        if let Ok(menu) = proxy.get_property::<zbus::zvariant::OwnedObjectPath>("Menu") {
            let p = menu.as_str().to_string();
            if !p.is_empty() && p != "/" {
                self.menu_path = Some(p);
            }
        }
        if let Ok(win_id) = proxy.get_property::<u32>("WindowId") {
            self.window_id = win_id;
        }

        // Parse IconPixmap: Array<(i32, i32, Vec<u8>)>
        if let Ok(pixmap_val) = proxy.get_property::<Value<'_>>("IconPixmap") {
            self.icon_pixmaps = parse_pixmap_array(&pixmap_val);
        }
        if let Ok(att_pixmap_val) = proxy.get_property::<Value<'_>>("AttentionIconPixmap") {
            self.attention_icon_pixmaps = parse_pixmap_array(&att_pixmap_val);
        }

        // Tooltip: (icon_name: String, icon_data: Array, title: String, description: String)
        if let Ok(Value::Structure(s)) = proxy.get_property::<Value<'_>>("ToolTip") {
            let fields = s.fields();
            if fields.len() >= 4 {
                if let Value::Str(t) = &fields[2] {
                    if !t.is_empty() {
                        self.tooltip_title = Some(t.to_string());
                    }
                }
                if let Value::Str(d) = &fields[3] {
                    if !d.is_empty() {
                        self.tooltip_description = Some(d.to_string());
                    }
                }
            }
        }

        Ok(())
    }

    /// Invoke `Activate(x, y)` on the item.
    pub fn activate(&self, conn: &Connection, x: i32, y: i32) -> Result<(), zbus::Error> {
        let proxy = zbus::blocking::Proxy::new(
            conn,
            self.service.as_str(),
            self.path.as_str(),
            "org.kde.StatusNotifierItem",
        )?;
        proxy.call("Activate", &(x, y))
    }

    /// Invoke `ContextMenu(x, y)` on the item.
    pub fn context_menu(&self, conn: &Connection, x: i32, y: i32) -> Result<(), zbus::Error> {
        let proxy = zbus::blocking::Proxy::new(
            conn,
            self.service.as_str(),
            self.path.as_str(),
            "org.kde.StatusNotifierItem",
        )?;
        proxy.call("ContextMenu", &(x, y))
    }

    /// Invoke `SecondaryActivate(x, y)` on the item.
    pub fn secondary_activate(&self, conn: &Connection, x: i32, y: i32) -> Result<(), zbus::Error> {
        let proxy = zbus::blocking::Proxy::new(
            conn,
            self.service.as_str(),
            self.path.as_str(),
            "org.kde.StatusNotifierItem",
        )?;
        proxy.call("SecondaryActivate", &(x, y))
    }

    /// Invoke `Scroll(delta, orientation)` on the item.
    pub fn scroll(
        &self,
        conn: &Connection,
        delta: i32,
        orientation: &str,
    ) -> Result<(), zbus::Error> {
        let proxy = zbus::blocking::Proxy::new(
            conn,
            self.service.as_str(),
            self.path.as_str(),
            "org.kde.StatusNotifierItem",
        )?;
        proxy.call("Scroll", &(delta, orientation))
    }
}

/// Helper to parse array of pixmap structures: `a(iiay)`.
fn parse_pixmap_array(val: &Value<'_>) -> Vec<(i32, i32, Vec<u8>)> {
    let mut result = Vec::new();
    let arr = match val {
        Value::Array(a) => a,
        _ => return result,
    };

    for item in arr.iter() {
        if let Value::Structure(s) = item {
            let fields = s.fields();
            if fields.len() >= 3 {
                let w = match &fields[0] {
                    Value::I32(w) => *w,
                    Value::U32(w) => *w as i32,
                    _ => continue,
                };
                let h = match &fields[1] {
                    Value::I32(h) => *h,
                    Value::U32(h) => *h as i32,
                    _ => continue,
                };
                let bytes = match &fields[2] {
                    Value::Array(b) => {
                        let mut byte_vec = Vec::with_capacity(b.len());
                        for bv in b.iter() {
                            if let Value::U8(u) = bv {
                                byte_vec.push(*u);
                            }
                        }
                        byte_vec
                    }
                    _ => Vec::new(),
                };
                if w > 0 && h > 0 && !bytes.is_empty() {
                    result.push((w, h, bytes));
                }
            }
        }
    }
    result
}

/// Event messages emitted from the DBus bridge thread to the main Wayland event loop.
#[derive(Debug, Clone)]
pub enum TrayEvent {
    /// A new StatusNotifierItem has registered.
    Added(SniItem),
    /// An existing item updated properties or icon.
    Changed(SniItem),
    /// An item unmounted or disconnected.
    Removed(String),
    /// Overall attention state changed.
    AttentionChanged(bool),
}
