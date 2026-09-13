//! DBus bridge background thread and command channel.
//!
//! Owns the `zbus` connection, hosts `StatusNotifierWatcher`, and forwards tray events to `calloop`.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use zbus::blocking::{connection::Builder, Connection};
use zbus::interface;
use zbus::zvariant::Value;

use crate::dbus::dbusmenu::{dbusmenu_to_menu_model, DBusMenuItem};
use crate::dbus::sni::{SniItem, TrayEvent};
use crate::dbus::tray_watch::{SigHit, TrayWatchers};
use crate::widget::menu::MenuModel;

/// Commands sent from the main Wayland loop to the DBus bridge thread.
pub enum DbusCommand {
    Activate {
        service: String,
        path: String,
        x: i32,
        y: i32,
    },
    ContextMenu {
        service: String,
        path: String,
        x: i32,
        y: i32,
    },
    SecondaryActivate {
        service: String,
        path: String,
        x: i32,
        y: i32,
    },
    Scroll {
        service: String,
        path: String,
        delta: i32,
        orientation: String,
    },
    FetchMenu {
        service: String,
        menu_path: String,
        reply: Sender<Result<MenuModel, String>>,
    },
    MenuClick {
        service: String,
        menu_path: String,
        item_id: i32,
        reply: Sender<Result<(), String>>,
    },
    Shutdown,
}

/// Handle to communicate with the DBus bridge thread.
#[derive(Clone)]
pub struct DbusBridgeHandle {
    cmd_tx: Sender<DbusCommand>,
}

impl DbusBridgeHandle {
    pub fn activate(&self, service: &str, path: &str, x: i32, y: i32) {
        let _ = self.cmd_tx.send(DbusCommand::Activate {
            service: service.to_string(),
            path: path.to_string(),
            x,
            y,
        });
    }

    pub fn context_menu(&self, service: &str, path: &str, x: i32, y: i32) {
        let _ = self.cmd_tx.send(DbusCommand::ContextMenu {
            service: service.to_string(),
            path: path.to_string(),
            x,
            y,
        });
    }

    pub fn secondary_activate(&self, service: &str, path: &str, x: i32, y: i32) {
        let _ = self.cmd_tx.send(DbusCommand::SecondaryActivate {
            service: service.to_string(),
            path: path.to_string(),
            x,
            y,
        });
    }

    pub fn scroll(&self, service: &str, path: &str, delta: i32, orientation: &str) {
        let _ = self.cmd_tx.send(DbusCommand::Scroll {
            service: service.to_string(),
            path: path.to_string(),
            delta,
            orientation: orientation.to_string(),
        });
    }

    pub fn fetch_menu(&self, service: &str, menu_path: &str) -> Result<MenuModel, String> {
        let (tx, rx) = channel();
        self.cmd_tx
            .send(DbusCommand::FetchMenu {
                service: service.to_string(),
                menu_path: menu_path.to_string(),
                reply: tx,
            })
            .map_err(|e| format!("Failed to send command to DBus bridge: {e}"))?;

        rx.recv_timeout(Duration::from_millis(500))
            .map_err(|e| format!("Timed out waiting for menu layout: {e}"))?
    }

    pub fn menu_click(&self, service: &str, menu_path: &str, item_id: i32) -> Result<(), String> {
        let (tx, rx) = channel();
        self.cmd_tx
            .send(DbusCommand::MenuClick {
                service: service.to_string(),
                menu_path: menu_path.to_string(),
                item_id,
                reply: tx,
            })
            .map_err(|e| format!("Failed to send menu click: {e}"))?;

        rx.recv_timeout(Duration::from_millis(500))
            .map_err(|e| format!("Timed out waiting for menu click: {e}"))?
    }

    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(DbusCommand::Shutdown);
    }
}

/// StatusNotifierWatcher DBus Interface implementation.
struct StatusNotifierWatcher {
    items: Arc<std::sync::Mutex<Vec<String>>>,
    cmd_tx: Sender<String>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl StatusNotifierWatcher {
    fn register_status_notifier_item(
        &mut self,
        service: &str,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
    ) {
        let sender = hdr.sender().map(|s| s.as_str()).unwrap_or(service);
        let target = if service.starts_with('/') {
            format!("{sender}{service}")
        } else {
            service.to_string()
        };

        {
            let mut list = self.items.lock().unwrap();
            if !list.contains(&target) {
                list.push(target.clone());
            }
        }

        let _ = self.cmd_tx.send(target);
    }

    fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

/// Start the DBus background bridge thread.
///
/// `fallback_poll_secs` controls the safety-net interval that re-fetches all
/// items and detects removals (for tray apps that emit no signals). `0`
/// disables the safety net entirely.
pub fn start_dbus_bridge(
    event_tx: Sender<TrayEvent>,
    fallback_poll_secs: u64,
) -> Result<(DbusBridgeHandle, JoinHandle<()>), String> {
    let (cmd_tx, cmd_rx) = channel::<DbusCommand>();
    let handle = DbusBridgeHandle {
        cmd_tx: cmd_tx.clone(),
    };

    let join_handle = thread::Builder::new()
        .name("flatbar-dbus-bridge".to_string())
        .spawn(move || {
            run_bridge_loop(event_tx, cmd_rx, fallback_poll_secs);
        })
        .map_err(|e| format!("Failed to spawn DBus bridge thread: {e}"))?;

    Ok((handle, join_handle))
}

fn run_bridge_loop(
    event_tx: Sender<TrayEvent>,
    cmd_rx: Receiver<DbusCommand>,
    fallback_poll_secs: u64,
) {
    let mut tracked_items: HashMap<String, SniItem> = HashMap::new();
    let registered_list = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (reg_tx, reg_rx) = channel::<String>();
    let (sig_tx, sig_rx) = channel::<SigHit>();
    let mut watchers = TrayWatchers::new(sig_tx);

    // Connect to session bus
    let conn = match Builder::session() {
        Ok(b) => match b.build() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to establish session DBus connection: {e}");
                return;
            }
        },
        Err(e) => {
            tracing::warn!("Failed to build session DBus connection: {e}");
            return;
        }
    };

    // Global signal sources: PropertiesChanged catch-all + NameOwnerChanged
    // removal detection. Per-item watchers are spawned on registration.
    watchers.spawn_global_watchers(&conn);

    // Attempt to register StatusNotifierWatcher
    let watcher = StatusNotifierWatcher {
        items: registered_list.clone(),
        cmd_tx: reg_tx,
    };

    let _ = conn.object_server().at("/StatusNotifierWatcher", watcher);
    let _ = conn.request_name("org.kde.StatusNotifierWatcher");

    // Request host name
    let pid = std::process::id();
    let host_name = format!("org.freedesktop.StatusNotifierHost-{pid}");
    let _ = conn.request_name(host_name.as_str());

    // Initial discovery of existing SNI items
    discover_existing_items(&conn, &mut tracked_items, &event_tx, &mut watchers);

    let fallback_poll = if fallback_poll_secs > 0 {
        Some(Duration::from_secs(fallback_poll_secs))
    } else {
        None
    };
    let mut last_fallback_poll = Instant::now();

    loop {
        // 1. Process registrations from watcher
        while let Ok(target) = reg_rx.try_recv() {
            let (service, path) = parse_service_and_path(&target);
            let key = format!("{service}{path}");
            // Watchers must be active before the first fetch so early signal
            // bursts are not missed (duplicate downstream updates are
            // suppressed by the diff check).
            watchers.watch_item(&conn, &key, &service, &path);
            let mut item = SniItem::new(service.clone(), path.clone());
            if item.fetch_properties(&conn).is_ok() {
                event_tx.send(TrayEvent::Added(item.clone())).ok();
                tracked_items.insert(target, item);
                update_attention(&tracked_items, &event_tx);
            }
        }

        // 2. Drain signal hits, coalescing to one fetch per item per pass
        let mut hits = std::collections::HashSet::new();
        while let Ok(hit) = sig_rx.try_recv() {
            hits.insert(hit);
        }
        for hit in hits {
            match hit {
                SigHit::ItemChanged(service) => {
                    let key = watchers
                        .key_for_service(&service)
                        .unwrap_or_else(|| service.clone());
                    refresh_single_item(&conn, &key, &mut tracked_items, &event_tx);
                }
                SigHit::ItemGone(name) => {
                    let key = watchers
                        .key_for_service(&name)
                        .or_else(|| find_key_by_name(&tracked_items, &name));
                    if let Some(key) = key {
                        watchers.forget_item(&key);
                        tracked_items.remove(&key);
                        event_tx.send(TrayEvent::Removed(key)).ok();
                        update_attention(&tracked_items, &event_tx);
                    }
                }
            }
        }

        // 3. Process incoming UI commands
        match cmd_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(DbusCommand::Activate {
                service,
                path,
                x,
                y,
            }) => {
                if let Some(item) = tracked_items.get(&format!("{service}{path}")).or_else(|| {
                    tracked_items
                        .values()
                        .find(|i| i.service == service && i.path == path)
                }) {
                    let _ = item.activate(&conn, x, y);
                }
            }
            Ok(DbusCommand::ContextMenu {
                service,
                path,
                x,
                y,
            }) => {
                if let Some(item) = tracked_items.get(&format!("{service}{path}")).or_else(|| {
                    tracked_items
                        .values()
                        .find(|i| i.service == service && i.path == path)
                }) {
                    let _ = item.context_menu(&conn, x, y);
                }
            }
            Ok(DbusCommand::SecondaryActivate {
                service,
                path,
                x,
                y,
            }) => {
                if let Some(item) = tracked_items.get(&format!("{service}{path}")).or_else(|| {
                    tracked_items
                        .values()
                        .find(|i| i.service == service && i.path == path)
                }) {
                    let _ = item.secondary_activate(&conn, x, y);
                }
            }
            Ok(DbusCommand::Scroll {
                service,
                path,
                delta,
                orientation,
            }) => {
                if let Some(item) = tracked_items.get(&format!("{service}{path}")).or_else(|| {
                    tracked_items
                        .values()
                        .find(|i| i.service == service && i.path == path)
                }) {
                    let _ = item.scroll(&conn, delta, &orientation);
                }
            }
            Ok(DbusCommand::FetchMenu {
                service,
                menu_path,
                reply,
            }) => {
                let res = fetch_dbusmenu_model(&conn, &service, &menu_path);
                let _ = reply.send(res);
            }
            Ok(DbusCommand::MenuClick {
                service,
                menu_path,
                item_id,
                reply,
            }) => {
                let res = send_menu_event(&conn, &service, &menu_path, item_id);
                let _ = reply.send(res);
            }
            Ok(DbusCommand::Shutdown) => {
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Safety-net refresh: slow poll catching items that emit no
                // signals (also recovers any missed watcher hits).
                if fallback_poll
                    .map(|p| last_fallback_poll.elapsed() >= p)
                    .unwrap_or(false)
                {
                    last_fallback_poll = Instant::now();
                    refresh_items(&conn, &mut tracked_items, &event_tx);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }
}

fn parse_service_and_path(target: &str) -> (String, String) {
    if let Some(pos) = target.find('/') {
        (target[..pos].to_string(), target[pos..].to_string())
    } else {
        (target.to_string(), "/StatusNotifierItem".to_string())
    }
}

fn discover_existing_items(
    conn: &Connection,
    tracked: &mut HashMap<String, SniItem>,
    event_tx: &Sender<TrayEvent>,
    watchers: &mut TrayWatchers,
) {
    let dbus_proxy = match zbus::blocking::fdo::DBusProxy::new(conn) {
        Ok(p) => p,
        Err(_) => return,
    };

    if let Ok(names) = dbus_proxy.list_names() {
        for name in names {
            let name_str = name.as_str();
            if name_str.starts_with("org.freedesktop.StatusNotifierItem-")
                || name_str.starts_with("org.kde.StatusNotifierItem-")
            {
                let mut item =
                    SniItem::new(name_str.to_string(), "/StatusNotifierItem".to_string());
                let key = format!("{name_str}/StatusNotifierItem");
                watchers.watch_item(conn, &key, name_str, "/StatusNotifierItem");
                if item.fetch_properties(conn).is_ok() {
                    event_tx.send(TrayEvent::Added(item.clone())).ok();
                    tracked.insert(key, item);
                }
            }
        }
    }
    update_attention(tracked, event_tx);
}

/// Re-fetch properties for a single tracked item after a signal hit, emitting
/// `Changed` only when something actually differs.
fn refresh_single_item(
    conn: &Connection,
    key: &str,
    tracked: &mut HashMap<String, SniItem>,
    event_tx: &Sender<TrayEvent>,
) {
    if let Some(item) = tracked.get_mut(key) {
        let mut updated = item.clone();
        if updated.fetch_properties(conn).is_ok() && updated != *item {
            *item = updated.clone();
            event_tx.send(TrayEvent::Changed(updated)).ok();
            update_attention(tracked, event_tx);
        }
    }
}

/// Locate the tracked key whose service name matches a bus name that vanished.
fn find_key_by_name(tracked: &HashMap<String, SniItem>, name: &str) -> Option<String> {
    tracked
        .values()
        .find(|i| i.service == name)
        .map(|i| format!("{}{}", i.service, i.path))
}

fn refresh_items(
    conn: &Connection,
    tracked: &mut HashMap<String, SniItem>,
    event_tx: &Sender<TrayEvent>,
) {
    let mut to_remove = Vec::new();

    for (key, item) in tracked.iter_mut() {
        let mut updated = item.clone();
        if updated.fetch_properties(conn).is_ok() {
            if updated != *item {
                *item = updated.clone();
                event_tx.send(TrayEvent::Changed(updated)).ok();
            }
        } else {
            to_remove.push(key.clone());
        }
    }

    for key in to_remove {
        tracked.remove(&key);
        event_tx.send(TrayEvent::Removed(key)).ok();
    }

    update_attention(tracked, event_tx);
}

fn update_attention(tracked: &HashMap<String, SniItem>, event_tx: &Sender<TrayEvent>) {
    let has_attention = tracked
        .values()
        .any(|i| i.status == crate::dbus::sni::SniStatus::NeedsAttention);
    event_tx
        .send(TrayEvent::AttentionChanged(has_attention))
        .ok();
}

fn fetch_dbusmenu_model(
    conn: &Connection,
    service: &str,
    menu_path: &str,
) -> Result<MenuModel, String> {
    let proxy = zbus::blocking::Proxy::new(conn, service, menu_path, "com.canonical.dbusmenu")
        .map_err(|e| format!("Failed to create dbusmenu proxy: {e}"))?;

    // Call AboutToShow(0)
    let _: Result<bool, _> = proxy.call("AboutToShow", &(0i32,));

    // Call GetLayout(0, 10, ["label", "enabled", "visible", "type", "icon-name", "toggle-type", "toggle-state", "children-display"])
    let prop_names: Vec<&str> = vec![
        "label",
        "enabled",
        "visible",
        "type",
        "icon-name",
        "toggle-type",
        "toggle-state",
        "children-display",
    ];

    let reply: (u32, zbus::zvariant::OwnedValue) = proxy
        .call("GetLayout", &(0i32, 10i32, prop_names))
        .map_err(|e| format!("GetLayout failed: {e}"))?;

    let root_item = DBusMenuItem::parse(&reply.1)
        .ok_or_else(|| "Failed to parse DBusMenu layout structure".to_string())?;

    Ok(dbusmenu_to_menu_model(&root_item))
}

fn send_menu_event(
    conn: &Connection,
    service: &str,
    menu_path: &str,
    item_id: i32,
) -> Result<(), String> {
    let proxy = zbus::blocking::Proxy::new(conn, service, menu_path, "com.canonical.dbusmenu")
        .map_err(|e| format!("Failed to create dbusmenu proxy: {e}"))?;

    let dummy_val = Value::from(0i32);
    let _: () = proxy
        .call("Event", &(item_id, "clicked", &dummy_val, 0u32))
        .map_err(|e| format!("Menu Event call failed: {e}"))?;

    Ok(())
}
