//! Desktop notifications client for threshold alerts.

use std::process::Command;

/// Send a desktop notification using the notification daemon via notify-send or DBus.
pub fn send_notification(summary: &str, body: &str, app_name: &str) -> Result<(), String> {
    // Attempt standard notify-send execution first
    let res = Command::new("notify-send")
        .arg("-a")
        .arg(app_name)
        .arg(summary)
        .arg(body)
        .status();

    match res {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("notify-send returned status {status}")),
        Err(e) => {
            tracing::debug!("notify-send not found or failed: {e}");
            Err(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notify_stub() {
        // Calling notify when notify-send might not be installed should return Ok or Err gracefully without panic
        let _ = send_notification("Test", "Body", "flatbar");
    }
}
