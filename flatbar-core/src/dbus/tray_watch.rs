//! Signal watchers for the tray bridge: one parked thread per event source,
//! fanning hits into a single channel consumed by the bridge loop.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::thread;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::{Header, Type as MsgType};
use zbus::{fdo::NameOwnerChanged, MatchRule};

const SNI_IFACE: &str = "org.kde.StatusNotifierItem";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
const FDO_DBUS: &str = "org.freedesktop.DBus";

/// A change detected by a watcher thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SigHit {
    /// The item with this tracked key changed (service+path).
    ItemChanged(String),
    /// The bus name owning this tracked key disappeared.
    ItemGone(String),
}

/// The SNI property-change signals we react to.
const SNI_SIGNALS: [&str; 6] = [
    "NewIcon",
    "NewAttentionIcon",
    "NewOverlayIcon",
    "NewToolTip",
    "NewTitle",
    "NewStatus",
];

/// Manages watcher threads for registered tray items.
pub struct TrayWatchers {
    sig_tx: Sender<SigHit>,
    /// key -> bus name, so PropertiesChanged/NameOwnerChanged senders can be
    /// resolved back to tracked keys.
    names: HashMap<String, String>,
}

impl TrayWatchers {
    pub fn new(sig_tx: Sender<SigHit>) -> Self {
        Self {
            sig_tx,
            names: HashMap::new(),
        }
    }

    /// Start watching an item. Must be called before the initial property
    /// fetch so early signal bursts are not missed. Idempotent per key.
    pub fn watch_item(&mut self, conn: &Connection, key: &str, service: &str, path: &str) {
        if self.names.contains_key(key) {
            return;
        }
        self.names.insert(key.to_string(), service.to_string());
        self.spawn_item_watcher(
            conn.clone(),
            key.to_string(),
            service.to_string(),
            path.to_string(),
        );
    }

    /// Resolve a sender bus name (unique or well-known) to a tracked key.
    pub fn key_for_service(&self, service: &str) -> Option<String> {
        if self.names.values().any(|n| n == service) {
            self.names
                .iter()
                .find(|(_, n)| n.as_str() == service)
                .map(|(k, _)| k.clone())
        } else {
            // Fall back to the canonical key form for unique names.
            self.names.keys().find(|k| k.starts_with(service)).cloned()
        }
    }

    /// Drop tracking for an item (its bus name is gone).
    pub fn forget_item(&mut self, key: &str) {
        self.names.remove(key);
    }

    fn spawn_item_watcher(&self, conn: Connection, key: String, service: String, path: String) {
        let tx = self.sig_tx.clone();
        let _ = thread::Builder::new()
            .name(format!("tray-watch-{key}"))
            .spawn(move || {
                let b = MatchRule::builder().msg_type(MsgType::Signal);
                let Ok(b) = b.sender(service.as_str()) else {
                    return;
                };
                let Ok(b) = b.interface(SNI_IFACE) else {
                    return;
                };
                let Ok(b) = b.path(path.as_str()) else { return };
                let rule = b.build();
                let iter = match MessageIterator::for_match_rule(rule, &conn, Some(32)) {
                    Ok(i) => i,
                    Err(_) => return,
                };
                for msg in iter {
                    let Ok(msg) = msg else { break };
                    let header = msg.header();
                    let Some(member) = header.member() else {
                        continue;
                    };
                    if SNI_SIGNALS.contains(&member.as_str())
                        && tx.send(SigHit::ItemChanged(key.clone())).is_err()
                    {
                        break;
                    }
                }
            });
    }

    /// One global thread for PropertiesChanged on the SNI interface
    /// (catch-all for apps that skip the SNI-specific signals), and one
    /// global thread for NameOwnerChanged (crash/disconnect detection).
    pub fn spawn_global_watchers(&self, conn: &Connection) {
        let props_tx = self.sig_tx.clone();
        let props_conn = conn.clone();
        let _ = thread::Builder::new()
            .name("tray-watch-props".to_string())
            .spawn(move || {
                let b = MatchRule::builder().msg_type(MsgType::Signal);
                let Ok(b) = b.interface(PROPS_IFACE) else {
                    return;
                };
                let Ok(b) = b.add_arg(SNI_IFACE) else { return };
                let rule = b.build();
                let Ok(iter) = MessageIterator::for_match_rule(rule, &props_conn, Some(64)) else {
                    return;
                };
                for msg in iter {
                    let Ok(msg) = msg else { break };
                    let Some(sender) = sender_str(&msg.header()) else {
                        continue;
                    };
                    if props_tx.send(SigHit::ItemChanged(sender)).is_err() {
                        break;
                    }
                }
            });

        let noc_tx = self.sig_tx.clone();
        let noc_conn = conn.clone();
        let _ = thread::Builder::new()
            .name("tray-watch-noc".to_string())
            .spawn(move || {
                let b = MatchRule::builder().msg_type(MsgType::Signal);
                let Ok(b) = b.sender(FDO_DBUS) else { return };
                let Ok(b) = b.interface(FDO_DBUS) else { return };
                let Ok(b) = b.member("NameOwnerChanged") else {
                    return;
                };
                let rule = b.build();
                let Ok(iter) = MessageIterator::for_match_rule(rule, &noc_conn, Some(64)) else {
                    return;
                };
                for msg in iter {
                    let Ok(msg) = msg else { break };
                    let Some(signal) = NameOwnerChanged::from_message(msg) else {
                        continue;
                    };
                    let Ok(args) = signal.args() else {
                        continue;
                    };
                    let name = args.name().to_string();
                    if !name.starts_with("org.kde.StatusNotifierItem-")
                        && !name.starts_with("org.freedesktop.StatusNotifierItem-")
                    {
                        continue;
                    }
                    let gone = args
                        .new_owner()
                        .as_ref()
                        .map(|u| u.as_str().is_empty())
                        .unwrap_or(true);
                    if gone && noc_tx.send(SigHit::ItemGone(name)).is_err() {
                        break;
                    }
                }
            });
    }
}

fn sender_str(header: &Header<'_>) -> Option<String> {
    header.sender().map(|s| s.as_str().to_string())
}
