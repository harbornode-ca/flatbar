//! Performance widget: power-profiles-daemon mode indicator with click-to-cycle.

use std::time::Duration;

use crate::dbus::power_profiles::PowerProfilesClient;
use crate::widget::{Span, Widget, WidgetState};

/// Icons for the three standard profiles; unknown profiles fall back to a gauge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceIcons {
    pub powersaver: String,
    pub balanced: String,
    pub performance: String,
}

impl Default for PerformanceIcons {
    fn default() -> Self {
        Self {
            powersaver: "fa-leaf".to_string(),
            balanced: "fa-scale-balanced".to_string(),
            performance: "fa-rocket".to_string(),
        }
    }
}

impl PerformanceIcons {
    pub fn for_profile(&self, profile: &str) -> &str {
        match profile {
            "power-saver" => &self.powersaver,
            "balanced" => &self.balanced,
            "performance" => &self.performance,
            _ => "fa-gauge",
        }
    }
}

/// Status bar widget showing the active power profile. Left-click cycles
/// power-saver → balanced → performance; right-click opens a picker menu.
/// Every change (cycled or picked) sends a desktop notification after the
/// daemon confirms. Hidden entirely when power-profiles-daemon is unavailable.
pub struct PerformanceWidget {
    id: String,
    interval: Duration,
    icons: PerformanceIcons,
    client: PowerProfilesClient,
}

/// Human-readable label for the standard profile names; unknown names pass
/// through unchanged.
pub fn profile_display_name(profile: &str) -> &str {
    match profile {
        "power-saver" => "Power Saver",
        "balanced" => "Balanced",
        "performance" => "Performance",
        other => other,
    }
}

impl PerformanceWidget {
    pub fn new(id: impl Into<String>, interval_secs: u64) -> Self {
        Self {
            id: id.into(),
            interval: Duration::from_secs(interval_secs.max(1)),
            icons: PerformanceIcons::default(),
            client: PowerProfilesClient::new(),
        }
    }

    pub fn with_icons(mut self, icons: PerformanceIcons) -> Self {
        self.icons = icons;
        self
    }

    pub fn with_client(mut self, client: PowerProfilesClient) -> Self {
        self.client = client;
        self
    }
}

impl Widget for PerformanceWidget {
    fn id(&self) -> &str {
        &self.id
    }

    fn update_interval(&self) -> Duration {
        self.interval
    }

    fn refresh(&self) {
        self.client.refresh();
    }

    fn handle_mouse_event(&self, event: crate::shell::pointer::WidgetMouseEvent) {
        if event == crate::shell::pointer::WidgetMouseEvent::LeftClick {
            self.client.cycle_profile(true);
        }
    }

    fn get_menu(&self) -> Option<crate::widget::menu::MenuModel> {
        use crate::widget::menu::{MenuItem, MenuModel};

        let pp = self.client.state();
        if !pp.available || pp.profiles.is_empty() {
            return None;
        }

        let mut items = Vec::with_capacity(pp.profiles.len() + 1);
        for profile in &pp.profiles {
            let is_active = profile == &pp.active_profile;
            let check = if is_active { "✓ " } else { "" };
            items.push(MenuItem::item_with_icon(
                format!("profile:{profile}"),
                format!("{check}{}", profile_display_name(profile)),
                self.icons.for_profile(profile),
            ));
        }
        items.push(MenuItem::Separator);
        items.push(MenuItem::item_with_icon(
            "cycle-profile",
            "Cycle to next",
            "fa-arrows-rotate",
        ));
        Some(MenuModel::new(items).with_title("Performance"))
    }

    fn handle_menu_action(&self, action_id: &str) {
        if let Some(profile) = action_id.strip_prefix("profile:") {
            let _ = self.client.set_active_profile(profile);
            return;
        }
        if action_id == "cycle-profile" {
            self.client.cycle_profile(true);
        }
    }

    fn state(&self) -> WidgetState {
        let pp = self.client.state();
        if !pp.available {
            // No power-profiles-daemon: hide instead of rendering a dead control.
            return WidgetState::default();
        }

        let icon = self.icons.for_profile(&pp.active_profile);
        WidgetState {
            spans: vec![Span::icon(icon)],
            tooltip: Some(format!("Performance: {}", pp.active_profile)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_with_profile(profile: &str) -> PowerProfilesClient {
        let client = PowerProfilesClient::new();
        client.set_state_for_test(crate::dbus::power_profiles::PowerProfilesState {
            available: true,
            active_profile: profile.to_string(),
            profiles: vec![
                "power-saver".to_string(),
                "balanced".to_string(),
                "performance".to_string(),
            ],
        });
        client
    }

    #[test]
    fn test_default_icons() {
        let icons = PerformanceIcons::default();
        assert_eq!(icons.for_profile("power-saver"), "fa-leaf");
        assert_eq!(icons.for_profile("balanced"), "fa-scale-balanced");
        assert_eq!(icons.for_profile("performance"), "fa-rocket");
        assert_eq!(icons.for_profile("something-else"), "fa-gauge");
    }

    #[test]
    fn test_icon_reflects_mode() {
        for (profile, expected) in [
            ("power-saver", "fa-leaf"),
            ("balanced", "fa-scale-balanced"),
            ("performance", "fa-rocket"),
        ] {
            let widget =
                PerformanceWidget::new("perf", 5).with_client(client_with_profile(profile));
            let state = widget.state();
            assert_eq!(state.spans.len(), 1);
            assert_eq!(state.spans[0].as_icon(), Some(expected));
        }
    }

    #[test]
    fn test_custom_icons_override() {
        let icons = PerformanceIcons {
            balanced: "fa-gauge".to_string(),
            ..Default::default()
        };
        let widget = PerformanceWidget::new("perf", 5)
            .with_icons(icons)
            .with_client(client_with_profile("balanced"));
        let state = widget.state();
        assert_eq!(state.spans[0].as_icon(), Some("fa-gauge"));
    }

    #[test]
    fn test_hidden_when_daemon_unavailable() {
        let client = PowerProfilesClient::new();
        client.set_state_for_test(crate::dbus::power_profiles::PowerProfilesState {
            available: false,
            active_profile: "balanced".to_string(),
            profiles: Vec::new(),
        });
        let widget = PerformanceWidget::new("perf", 5).with_client(client);
        let state = widget.state();
        assert!(state.spans.is_empty());
    }

    #[test]
    fn test_left_click_cycles_forward() {
        let widget = PerformanceWidget::new("perf", 5).with_client(client_with_profile("balanced"));
        widget.handle_mouse_event(crate::shell::pointer::WidgetMouseEvent::LeftClick);
        assert_eq!(widget.client.state().active_profile, "performance");
    }

    #[test]
    fn test_menu_lists_profiles_with_active_marked() {
        let widget = PerformanceWidget::new("perf", 5).with_client(client_with_profile("balanced"));
        let menu = widget.get_menu().expect("menu when daemon present");
        assert_eq!(menu.title.as_deref(), Some("Performance"));

        let labels: Vec<(String, String)> = menu
            .items
            .iter()
            .take(3)
            .filter_map(|i| {
                i.id()
                    .zip(i.label())
                    .map(|(id, l)| (id.to_string(), l.to_string()))
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                ("profile:power-saver".to_string(), "Power Saver".to_string()),
                ("profile:balanced".to_string(), "✓ Balanced".to_string()),
                ("profile:performance".to_string(), "Performance".to_string()),
            ]
        );
    }

    #[test]
    fn test_menu_none_when_daemon_unavailable() {
        let client = PowerProfilesClient::new();
        client.set_state_for_test(crate::dbus::power_profiles::PowerProfilesState {
            available: false,
            active_profile: "balanced".to_string(),
            profiles: Vec::new(),
        });
        let widget = PerformanceWidget::new("perf", 5).with_client(client);
        assert!(widget.get_menu().is_none());
    }

    #[test]
    fn test_menu_action_selects_profile() {
        let widget = PerformanceWidget::new("perf", 5).with_client(client_with_profile("balanced"));
        widget.handle_menu_action("profile:performance");
        assert_eq!(widget.client.state().active_profile, "performance");

        widget.handle_menu_action("profile:power-saver");
        assert_eq!(widget.client.state().active_profile, "power-saver");

        widget.handle_menu_action("cycle-profile");
        assert_eq!(widget.client.state().active_profile, "balanced");
    }

    #[test]
    fn test_profile_display_names() {
        assert_eq!(profile_display_name("power-saver"), "Power Saver");
        assert_eq!(profile_display_name("balanced"), "Balanced");
        assert_eq!(profile_display_name("performance"), "Performance");
        assert_eq!(profile_display_name("turbo"), "turbo");
    }
}
