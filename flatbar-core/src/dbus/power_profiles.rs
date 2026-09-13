//! power-profiles-daemon D-Bus client implementation for CPU power profile management.

use std::sync::{Arc, Mutex};
use std::thread;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

/// Snapshot of power-profiles-daemon state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerProfilesState {
    pub available: bool,
    pub active_profile: String,
    pub profiles: Vec<String>,
}

impl Default for PowerProfilesState {
    fn default() -> Self {
        Self {
            available: false,
            active_profile: "balanced".to_string(),
            profiles: vec![
                "power-saver".to_string(),
                "balanced".to_string(),
                "performance".to_string(),
            ],
        }
    }
}

/// Client managing power profile queries and switching over the D-Bus system bus.
#[derive(Debug, Clone)]
pub struct PowerProfilesClient {
    state: Arc<Mutex<PowerProfilesState>>,
}

impl Default for PowerProfilesClient {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerProfilesClient {
    /// Create a new PowerProfilesClient and perform an initial background query.
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(PowerProfilesState::default()));
        let client = Self { state };
        client.refresh();
        client
    }

    /// Read current snapshot of power profiles state.
    pub fn state(&self) -> PowerProfilesState {
        self.state.lock().unwrap().clone()
    }

    /// Test seam: directly install a state snapshot.
    #[cfg(test)]
    pub(crate) fn set_state_for_test(&self, state: PowerProfilesState) {
        *self.state.lock().unwrap() = state;
    }

    /// Refresh power profiles state from system D-Bus in the background.
    pub fn refresh(&self) {
        let state_lock = self.state.clone();
        thread::Builder::new()
            .name("flatbar-power-profiles-poll".to_string())
            .spawn(move || {
                if let Ok(new_state) = query_power_profiles() {
                    let mut lock = state_lock.lock().unwrap();
                    *lock = new_state;
                }
            })
            .ok();
    }

    /// Switch active profile asynchronously.
    ///
    /// Notifies the desktop on success (in the background, after the D-Bus
    /// join confirmation). Left-click cycling and right-click menu selection
    /// both route through this method.
    pub fn set_active_profile(&self, profile: &str) -> Result<(), String> {
        let prof = profile.to_string();
        let state_lock = self.state.clone();

        // Optimistically update local active_profile
        {
            let mut lock = state_lock.lock().unwrap();
            lock.active_profile = prof.clone();
        }

        thread::Builder::new()
            .name("flatbar-power-profile-set".to_string())
            .spawn(move || {
                let label = match prof.as_str() {
                    "power-saver" => "Power Saver",
                    "balanced" => "Balanced",
                    "performance" => "Performance",
                    other => other,
                };
                if let Err(e) = set_profile_dbus(&prof) {
                    tracing::warn!("Failed to set power profile to '{prof}': {e}");
                } else {
                    let _ = crate::dbus::notify::send_notification(
                        "Flatbar",
                        &format!("Power profile: {label}"),
                        "performance",
                    );
                }
                if let Ok(new_state) = query_power_profiles() {
                    let mut lock = state_lock.lock().unwrap();
                    *lock = new_state;
                }
            })
            .map_err(|e| format!("Failed to spawn profile setter thread: {e}"))?;

        Ok(())
    }

    /// Cycle to the next or previous profile.
    pub fn cycle_profile(&self, forward: bool) {
        let state = self.state();
        if !state.available || state.profiles.is_empty() {
            return;
        }

        let cur_idx = state
            .profiles
            .iter()
            .position(|p| p == &state.active_profile)
            .unwrap_or(0);

        let next_idx = if forward {
            (cur_idx + 1) % state.profiles.len()
        } else if cur_idx == 0 {
            state.profiles.len() - 1
        } else {
            cur_idx - 1
        };

        let next_prof = &state.profiles[next_idx];
        let _ = self.set_active_profile(next_prof);
    }
}

const SERVICES: &[(&str, &str)] = &[
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
    ),
];

/// Check whether a bus name actually has an owner before creating any proxy for it.
/// This avoids zbus's internal properties-cache task hitting the bus for a service
/// that does not exist (it emits a noisy `ServiceUnknown` WARN on `GetAll`).
fn service_owned(conn: &Connection, service: &str) -> bool {
    let dbus_proxy = match zbus::blocking::fdo::DBusProxy::new(conn) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let bus_name = match zbus::names::OwnedBusName::try_from(service) {
        Ok(name) => zbus::names::BusName::from(name),
        Err(_) => return false,
    };
    dbus_proxy
        .get_name_owner(bus_name)
        .map(|owner| !owner.as_str().is_empty())
        .unwrap_or(false)
}

fn query_power_profiles() -> Result<PowerProfilesState, zbus::Error> {
    let conn = Connection::system().or_else(|_| Connection::session())?;

    for &(service, path) in SERVICES {
        if !service_owned(&conn, service) {
            continue;
        }
        if let Ok(proxy) = zbus::blocking::Proxy::new(&conn, service, path, service) {
            if let Ok(active) = proxy.get_property::<String>("ActiveProfile") {
                let mut profiles = Vec::new();
                if let Ok(profiles_val) = proxy.get_property::<Value<'_>>("Profiles") {
                    profiles = parse_profiles_array(&profiles_val);
                }
                if profiles.is_empty() {
                    profiles = vec![
                        "power-saver".to_string(),
                        "balanced".to_string(),
                        "performance".to_string(),
                    ];
                }

                return Ok(PowerProfilesState {
                    available: true,
                    active_profile: active,
                    profiles,
                });
            }
        }
    }

    tracing::debug!("Power-profiles-daemon not available on D-Bus; power modes UI disabled");
    Ok(PowerProfilesState {
        available: false,
        active_profile: "balanced".to_string(),
        profiles: Vec::new(),
    })
}

fn set_profile_dbus(profile: &str) -> Result<(), zbus::Error> {
    let conn = Connection::system().or_else(|_| Connection::session())?;

    for &(service, path) in SERVICES {
        if !service_owned(&conn, service) {
            continue;
        }
        if let Ok(proxy) = zbus::blocking::Proxy::new(&conn, service, path, service) {
            if proxy.set_property("ActiveProfile", profile).is_ok() {
                return Ok(());
            }
        }
    }

    Err(zbus::Error::Failure(
        "No power-profiles-daemon service responded".to_string(),
    ))
}

fn parse_profiles_array(val: &Value<'_>) -> Vec<String> {
    let mut result = Vec::new();
    let arr = match val {
        Value::Array(a) => a,
        _ => return result,
    };

    for item in arr.iter() {
        if let Value::Structure(s) = item {
            let fields = s.fields();
            if !fields.is_empty() {
                if let Value::Str(name) = &fields[0] {
                    result.push(name.to_string());
                    continue;
                }
            }
        }
        if let Value::Dict(d) = item {
            for (k, v) in d.iter() {
                if let Value::Str(key) = k {
                    if key.as_str() == "Profile" {
                        if let Value::Str(p) = v {
                            result.push(p.to_string());
                        }
                    }
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_absent_daemon_yields_unavailable_without_proxy_side_effects() {
        // Requires a system or session bus; skips gracefully in restricted environments.
        let conn =
            zbus::blocking::Connection::system().or_else(|_| zbus::blocking::Connection::session());
        let Ok(conn) = conn else {
            return;
        };

        // Neither service should be owned when power-profiles-daemon is absent.
        for (service, _) in SERVICES {
            let owned = service_owned(&conn, service);
            // If the daemon *is* running under this name the check must not be false.
            let daemon_reachable =
                zbus::blocking::Proxy::new(&conn, *service, path_for(service), *service)
                    .and_then(|p| p.get_property::<String>("ActiveProfile").map(|_| ()))
                    .is_ok();
            assert_eq!(
                owned, daemon_reachable,
                "ownership probe wrong for {service}"
            );
        }

        let state = query_power_profiles().map_err(|e| e.to_string()).unwrap();
        if !state.available {
            assert!(state.profiles.is_empty());
            assert_eq!(state.active_profile, "balanced");
        }
    }

    fn path_for(service: &str) -> &'static str {
        SERVICES
            .iter()
            .find(|(name, _)| *name == service)
            .map(|(_, path)| *path)
            .expect("unknown service")
    }

    #[test]
    fn test_power_profiles_state_default() {
        let state = PowerProfilesState::default();
        assert_eq!(state.active_profile, "balanced");
        assert_eq!(state.profiles.len(), 3);
        assert!(!state.available);
    }

    #[test]
    fn test_power_profiles_cycle() {
        let client = PowerProfilesClient::new();
        {
            let mut lock = client.state.lock().unwrap();
            lock.available = true;
            lock.active_profile = "balanced".to_string();
            lock.profiles = vec![
                "power-saver".to_string(),
                "balanced".to_string(),
                "performance".to_string(),
            ];
        }

        client.cycle_profile(true);
        assert_eq!(client.state().active_profile, "performance");

        client.cycle_profile(true);
        assert_eq!(client.state().active_profile, "power-saver");

        client.cycle_profile(false);
        assert_eq!(client.state().active_profile, "performance");
    }
}
