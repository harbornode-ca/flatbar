//! Integration tests for the DBus tray bridge, running against a real session
//! bus spawned via `dbus-run-session`. Skips gracefully when unavailable.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flatbar_core::dbus::bridge::start_dbus_bridge;
use flatbar_core::dbus::sni::TrayEvent;
use zbus::blocking::Connection;
use zbus::interface;

const ITEM_NAME: &str = "org.kde.StatusNotifierItem-flatbar-test";
const ITEM_PATH: &str = "/StatusNotifierItem";
const SNI_IFACE: &str = "org.kde.StatusNotifierItem";

/// Serializes tests that mutate `DBUS_SESSION_BUS_ADDRESS` (process-global).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A live session bus kept alive by a sleeping child.
struct BusFixture {
    child: Child,
}

impl BusFixture {
    fn spawn(_lock: &Mutex<()>) -> Option<(Self, String, zbus::Result<Connection>)> {
        if !Command::new("dbus-run-session")
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            eprintln!("dbus-run-session not available; skipping");
            return None;
        }

        let mut child = Command::new("dbus-run-session")
            .arg("sh")
            .arg("-c")
            .arg("echo $DBUS_SESSION_BUS_ADDRESS; exec sleep 120")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn dbus-run-session");

        let stdout = child.stdout.take().expect("piped stdout");
        let mut lines = BufReader::new(stdout).lines();
        let address = match lines.next() {
            Some(Ok(line)) => line.trim().to_string(),
            _ => {
                eprintln!("no bus address; skipping");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        };
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        let conn = Connection::session();
        Some((Self { child }, address, conn))
    }
}

impl Drop for BusFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Minimal in-process SNI fake: owns the name and serves Id/Status.
struct FakeSni {
    status: Arc<Mutex<String>>,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl FakeSni {
    #[zbus(property)]
    fn id(&self) -> String {
        "flatbar-test-item".to_string()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        "Test Item".to_string()
    }
}

/// Claim the item name, serve the SNI object, register with the bridge's
/// watcher. Returns (connection, status handle) — mutating the handle then
/// emitting a signal produces a Changed event.
fn spawn_fake_item() -> (Connection, Arc<Mutex<String>>) {
    let conn = Connection::session().expect("item session connection");
    let _ = conn.request_name(ITEM_NAME);
    let status = Arc::new(Mutex::new("Active".to_string()));
    let _ = conn.object_server().at(
        ITEM_PATH,
        FakeSni {
            status: status.clone(),
        },
    );

    let watcher = Connection::session().expect("watcher connection");
    let deadline = Instant::now() + Duration::from_secs(10);
    let registered = loop {
        match watcher.call_method(
            Some("org.kde.StatusNotifierWatcher"),
            "/StatusNotifierWatcher",
            Some("org.kde.StatusNotifierWatcher"),
            "RegisterStatusNotifierItem",
            &ITEM_NAME,
        ) {
            Ok(_) => break true,
            // The bridge thread registers the watcher asynchronously; retry
            // until the name shows up.
            Err(e) if Instant::now() < deadline && e.to_string().contains("ServiceUnknown") => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break false,
        }
    };
    assert!(registered, "bridge watcher never became available");
    (conn, status)
}

/// Wait for an event matching `pred`, draining unrelated events.
fn wait_for(
    rx: &std::sync::mpsc::Receiver<TrayEvent>,
    timeout: Duration,
    pred: impl Fn(&TrayEvent) -> bool,
) -> Option<TrayEvent> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        match rx.recv_timeout(remaining) {
            Ok(evt) if pred(&evt) => return Some(evt),
            Ok(_) => continue,
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn start_bridge(
    fallback_secs: u64,
) -> (
    flatbar_core::dbus::bridge::DbusBridgeHandle,
    std::sync::mpsc::Receiver<TrayEvent>,
) {
    let (tx, rx) = channel::<TrayEvent>();
    let (handle, _join) = start_dbus_bridge(tx, fallback_secs).expect("bridge starts");
    (handle, rx)
}

fn emit(member: &str, conn: &Connection) {
    let _ = conn.emit_signal(None::<&str>, ITEM_PATH, SNI_IFACE, member, &());
}

#[test]
fn test_signal_driven_added_and_changed() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let Some((_fixture, _address, Ok(_))) = BusFixture::spawn(&ENV_LOCK) else {
        return;
    };

    let (_handle, rx) = start_bridge(0);
    let (item_conn, status) = spawn_fake_item();

    let added = wait_for(&rx, Duration::from_secs(10), |evt| {
        matches!(evt, TrayEvent::Added(_))
    });
    assert!(
        added.is_some(),
        "expected TrayEvent::Added after registration"
    );

    // Change the status property, then emit NewStatus. The signal-driven
    // path should produce Changed without the safety-net poll (fallback = 0).
    *status.lock().unwrap() = "NeedsAttention".to_string();
    emit("NewStatus", &item_conn);

    let changed = wait_for(
        &rx,
        Duration::from_secs(5),
        |evt| matches!(evt, TrayEvent::Changed(i) if i.status == flatbar_core::dbus::sni::SniStatus::NeedsAttention),
    );
    assert!(
        changed.is_some(),
        "expected TrayEvent::Changed driven by NewStatus signal"
    );
}

#[test]
fn test_signal_burst_coalesces_to_single_changed() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let Some((_fixture, _address, Ok(_))) = BusFixture::spawn(&ENV_LOCK) else {
        return;
    };
    let (_handle, rx) = start_bridge(0);
    let (item_conn, status) = spawn_fake_item();

    let added = wait_for(&rx, Duration::from_secs(10), |evt| {
        matches!(evt, TrayEvent::Added(_))
    });
    assert!(
        added.is_some(),
        "expected TrayEvent::Added after registration"
    );

    // Burst: several signals in quick succession → exactly one Changed.
    *status.lock().unwrap() = "NeedsAttention".to_string();
    emit("NewStatus", &item_conn);
    emit("NewIcon", &item_conn);
    emit("NewToolTip", &item_conn);
    emit("NewTitle", &item_conn);

    let changed = wait_for(
        &rx,
        Duration::from_secs(5),
        |evt| matches!(evt, TrayEvent::Changed(i) if i.status == flatbar_core::dbus::sni::SniStatus::NeedsAttention),
    );
    assert!(changed.is_some(), "expected a Changed event from the burst");

    // Drain for a moment: no second Changed may arrive without further
    // property mutation (all burst hits dedupe to one fetch).
    let extra = wait_for(
        &rx,
        Duration::from_millis(1500),
        |evt| matches!(evt, TrayEvent::Changed(names) if names.status == flatbar_core::dbus::sni::SniStatus::NeedsAttention),
    );
    assert!(extra.is_none(), "burst must coalesce to one Changed");
}

#[test]
fn test_item_exit_sends_removed() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let Some((_fixture, _address, Ok(_))) = BusFixture::spawn(&ENV_LOCK) else {
        return;
    };
    let (_handle, rx) = start_bridge(0);
    let (item_conn, _status) = spawn_fake_item();

    let added = wait_for(&rx, Duration::from_secs(10), |evt| {
        matches!(evt, TrayEvent::Added(_))
    });
    assert!(
        added.is_some(),
        "expected TrayEvent::Added after registration"
    );

    // Dropping the item's connection releases the name → NameOwnerChanged
    // with empty new_owner → immediate Removed.
    drop(item_conn);

    // Collect everything that arrives, reporting non-matching events.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut removed = None;
    let mut other_events = Vec::new();
    while removed.is_none() && Instant::now() < deadline {
        match rx.recv_timeout(deadline - Instant::now()) {
            Ok(TrayEvent::Removed(key)) if key.starts_with(ITEM_NAME) => {
                removed = Some(key);
            }
            Ok(other) => other_events.push(other),
            Err(_) => break,
        }
    }
    assert!(
        removed.is_some(),
        "expected TrayEvent::Removed via NameOwnerChanged; unrelated events: {other_events:?}"
    );
}
