//! BlueZ D-Bus client implementation for adapter and device management.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// Snapshot of a single Bluetooth device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BluetoothDevice {
    pub path: String,
    pub name: String,
    pub address: String,
    pub connected: bool,
    pub paired: bool,
    pub battery: Option<u8>,
    pub icon: Option<String>,
}

/// Snapshot of overall Bluetooth system state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BluetoothState {
    pub adapter_present: bool,
    pub adapter_powered: bool,
    pub adapter_path: Option<String>,
    pub adapter_name: Option<String>,
    pub devices: Vec<BluetoothDevice>,
}

impl BluetoothState {
    pub fn connected_devices(&self) -> Vec<&BluetoothDevice> {
        self.devices.iter().filter(|d| d.connected).collect()
    }

    pub fn paired_devices(&self) -> Vec<&BluetoothDevice> {
        self.devices.iter().filter(|d| d.paired).collect()
    }
}

/// BlueZ client managing Bluetooth queries and actions over the D-Bus system bus.
#[derive(Debug, Clone)]
pub struct BluetoothClient {
    state: Arc<Mutex<BluetoothState>>,
}

impl Default for BluetoothClient {
    fn default() -> Self {
        Self::new()
    }
}

impl BluetoothClient {
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(BluetoothState::default()));
        let client = Self { state };
        client.refresh();
        client
    }

    /// Read current snapshot of Bluetooth state.
    pub fn state(&self) -> BluetoothState {
        self.state.lock().unwrap().clone()
    }

    /// Refresh Bluetooth state from system D-Bus.
    pub fn refresh(&self) {
        let state_lock = self.state.clone();
        // Run refresh in background to prevent blocking UI thread
        thread::Builder::new()
            .name("flatbar-bluetooth-poll".to_string())
            .spawn(move || {
                if let Ok(new_state) = query_bluez_state() {
                    let mut lock = state_lock.lock().unwrap();
                    *lock = new_state;
                }
            })
            .ok();
    }

    /// Connect to a Bluetooth device by D-Bus object path.
    pub fn connect_device(&self, device_path: &str) -> Result<(), String> {
        let dev_path = device_path.to_string();
        let state_lock = self.state.clone();
        thread::Builder::new()
            .name("flatbar-bluetooth-connect".to_string())
            .spawn(move || {
                if let Ok(conn) = Connection::system() {
                    let path: ObjectPath = match dev_path.as_str().try_into() {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!("Invalid device path {dev_path}: {e}");
                            return;
                        }
                    };
                    let proxy = match zbus::blocking::Proxy::new(
                        &conn,
                        "org.bluez",
                        path,
                        "org.bluez.Device1",
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!("Failed to create Device1 proxy: {e}");
                            return;
                        }
                    };
                    let _: Result<(), _> = proxy.call("Connect", &());
                    if let Ok(new_state) = query_bluez_state() {
                        let mut lock = state_lock.lock().unwrap();
                        *lock = new_state;
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Disconnect a Bluetooth device by D-Bus object path.
    pub fn disconnect_device(&self, device_path: &str) -> Result<(), String> {
        let dev_path = device_path.to_string();
        let state_lock = self.state.clone();
        thread::Builder::new()
            .name("flatbar-bluetooth-disconnect".to_string())
            .spawn(move || {
                if let Ok(conn) = Connection::system() {
                    let path: ObjectPath = match dev_path.as_str().try_into() {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!("Invalid device path {dev_path}: {e}");
                            return;
                        }
                    };
                    let proxy = match zbus::blocking::Proxy::new(
                        &conn,
                        "org.bluez",
                        path,
                        "org.bluez.Device1",
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!("Failed to create Device1 proxy: {e}");
                            return;
                        }
                    };
                    let _: Result<(), _> = proxy.call("Disconnect", &());
                    if let Ok(new_state) = query_bluez_state() {
                        let mut lock = state_lock.lock().unwrap();
                        *lock = new_state;
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Set adapter powered state.
    pub fn set_adapter_power(&self, powered: bool) -> Result<(), String> {
        let state = self.state();
        let adapter_path = match state.adapter_path {
            Some(p) => p,
            None => return Err("No Bluetooth adapter found".to_string()),
        };

        let state_lock = self.state.clone();
        thread::Builder::new()
            .name("flatbar-bluetooth-power".to_string())
            .spawn(move || {
                if let Ok(conn) = Connection::system() {
                    let path: ObjectPath = match adapter_path.as_str().try_into() {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!("Invalid adapter path {adapter_path}: {e}");
                            return;
                        }
                    };
                    let proxy = match zbus::blocking::Proxy::new(
                        &conn,
                        "org.bluez",
                        path,
                        "org.freedesktop.DBus.Properties",
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!("Failed to create Properties proxy: {e}");
                            return;
                        }
                    };
                    let val = Value::Bool(powered);
                    let _: Result<(), _> =
                        proxy.call("Set", &("org.bluez.Adapter1", "Powered", val));
                    thread::sleep(Duration::from_millis(150));
                    if let Ok(new_state) = query_bluez_state() {
                        let mut lock = state_lock.lock().unwrap();
                        *lock = new_state;
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}

/// Query BlueZ managed objects directly from system D-Bus.
pub fn query_bluez_state() -> Result<BluetoothState, String> {
    let conn = Connection::system().map_err(|e| format!("System bus unavailable: {e}"))?;
    let proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.bluez",
        "/",
        "org.freedesktop.DBus.ObjectManager",
    )
    .map_err(|e| format!("ObjectManager proxy error: {e}"))?;

    let reply: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> = proxy
        .call("GetManagedObjects", &())
        .map_err(|e| format!("GetManagedObjects failed: {e}"))?;

    parse_bluez_managed_objects(&reply)
}

/// Parse managed objects dictionary into `BluetoothState`.
pub fn parse_bluez_managed_objects(
    objects: &HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>,
) -> Result<BluetoothState, String> {
    let mut state = BluetoothState::default();
    let mut devices = Vec::new();
    let mut batteries: HashMap<String, u8> = HashMap::new();

    // First pass: find batteries
    for (path, ifaces) in objects {
        let path_str = path.to_string();
        if let Some(battery_props) = ifaces.get("org.bluez.Battery1") {
            if let Some(val) = battery_props.get("Percentage") {
                if let Ok(pct) = u8::try_from(val) {
                    batteries.insert(path_str.clone(), pct);
                }
            }
        }
    }

    // Second pass: find adapters and devices
    for (path, ifaces) in objects {
        let path_str = path.to_string();

        if let Some(adapter_props) = ifaces.get("org.bluez.Adapter1") {
            state.adapter_present = true;
            state.adapter_path = Some(path_str.clone());

            if let Some(val) = adapter_props.get("Powered") {
                if let Ok(p) = bool::try_from(val) {
                    state.adapter_powered = p;
                }
            }

            if let Some(val) = adapter_props
                .get("Alias")
                .or_else(|| adapter_props.get("Name"))
            {
                if let Ok(name) = <&str>::try_from(val) {
                    state.adapter_name = Some(name.to_string());
                }
            }
        }

        if let Some(dev_props) = ifaces.get("org.bluez.Device1") {
            let address = dev_props
                .get("Address")
                .and_then(|v| <&str>::try_from(v).ok())
                .unwrap_or_default()
                .to_string();

            let name = dev_props
                .get("Alias")
                .or_else(|| dev_props.get("Name"))
                .and_then(|v| <&str>::try_from(v).ok())
                .unwrap_or(&address)
                .to_string();

            let connected = dev_props
                .get("Connected")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false);

            let paired = dev_props
                .get("Paired")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false);

            let icon = dev_props
                .get("Icon")
                .and_then(|v| <&str>::try_from(v).ok())
                .map(|s| s.to_string());

            let battery = batteries.get(&path_str).copied();

            devices.push(BluetoothDevice {
                path: path_str,
                name,
                address,
                connected,
                paired,
                battery,
                icon,
            });
        }
    }

    // Sort devices: connected first, then paired, then by name
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| b.paired.cmp(&a.paired))
            .then_with(|| a.name.cmp(&b.name))
    });

    state.devices = devices;
    Ok(state)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_parse_bluez_managed_objects_mock() {
        let mut objects: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> =
            HashMap::new();

        // Adapter
        let mut adapter_props = HashMap::new();
        adapter_props.insert(
            "Powered".to_string(),
            OwnedValue::try_from(Value::Bool(true)).unwrap(),
        );
        adapter_props.insert(
            "Name".to_string(),
            OwnedValue::try_from(Value::Str("BlueZ 5.72".into())).unwrap(),
        );

        let mut adapter_ifaces = HashMap::new();
        adapter_ifaces.insert("org.bluez.Adapter1".to_string(), adapter_props);
        objects.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(),
            adapter_ifaces,
        );

        // Device 1: Sony WH-1000XM4 (Connected + Battery)
        let mut dev1_props = HashMap::new();
        dev1_props.insert(
            "Name".to_string(),
            OwnedValue::try_from(Value::Str("WH-1000XM4".into())).unwrap(),
        );
        dev1_props.insert(
            "Address".to_string(),
            OwnedValue::try_from(Value::Str("94:DB:56:AA:BB:CC".into())).unwrap(),
        );
        dev1_props.insert(
            "Connected".to_string(),
            OwnedValue::try_from(Value::Bool(true)).unwrap(),
        );
        dev1_props.insert(
            "Paired".to_string(),
            OwnedValue::try_from(Value::Bool(true)).unwrap(),
        );

        let mut dev1_battery = HashMap::new();
        dev1_battery.insert(
            "Percentage".to_string(),
            OwnedValue::try_from(Value::U8(85)).unwrap(),
        );

        let mut dev1_ifaces = HashMap::new();
        dev1_ifaces.insert("org.bluez.Device1".to_string(), dev1_props);
        dev1_ifaces.insert("org.bluez.Battery1".to_string(), dev1_battery);
        objects.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0/dev_94_DB_56_AA_BB_CC").unwrap(),
            dev1_ifaces,
        );

        // Device 2: Keychron K2 (Paired, Disconnected)
        let mut dev2_props = HashMap::new();
        dev2_props.insert(
            "Name".to_string(),
            OwnedValue::try_from(Value::Str("Keychron K2".into())).unwrap(),
        );
        dev2_props.insert(
            "Address".to_string(),
            OwnedValue::try_from(Value::Str("DC:2C:26:11:22:33".into())).unwrap(),
        );
        dev2_props.insert(
            "Connected".to_string(),
            OwnedValue::try_from(Value::Bool(false)).unwrap(),
        );
        dev2_props.insert(
            "Paired".to_string(),
            OwnedValue::try_from(Value::Bool(true)).unwrap(),
        );

        let mut dev2_ifaces = HashMap::new();
        dev2_ifaces.insert("org.bluez.Device1".to_string(), dev2_props);
        objects.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0/dev_DC_2C_26_11_22_33").unwrap(),
            dev2_ifaces,
        );

        let parsed = parse_bluez_managed_objects(&objects).unwrap();
        assert!(parsed.adapter_present);
        assert!(parsed.adapter_powered);
        assert_eq!(parsed.adapter_name.as_deref(), Some("BlueZ 5.72"));
        assert_eq!(parsed.devices.len(), 2);

        let connected = parsed.connected_devices();
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].name, "WH-1000XM4");
        assert_eq!(connected[0].battery, Some(85));

        let paired = parsed.paired_devices();
        assert_eq!(paired.len(), 2);
    }
}
