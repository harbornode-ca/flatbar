use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[allow(dead_code)]
pub struct HeadlessSwayHarness {
    pub runtime_dir: TempDir,
    pub socket_name: String,
    pub socket_path: PathBuf,
    sway_process: Child,
}

impl HeadlessSwayHarness {
    /// Check if both `sway` and `grim` are available on PATH.
    pub fn is_available() -> bool {
        let sway_ok = Command::new("sway")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        let grim_ok = Command::new("grim")
            .arg("-h")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        sway_ok && grim_ok
    }

    /// Try to spawn headless sway. Returns None if sway/grim are missing.
    pub fn try_new() -> Result<Self, String> {
        if !Self::is_available() {
            return Err(
                "sway or grim not found on PATH. Headless compositor tests skipped.".to_string(),
            );
        }

        let runtime_dir = tempfile::Builder::new()
            .prefix("flatbar-sway-runtime-")
            .tempdir()
            .map_err(|e| format!("Failed to create temp runtime dir: {e}"))?;

        let config_path = runtime_dir.path().join("sway.conf");
        let sway_config = "output * mode 1280x800\n";
        fs::write(&config_path, sway_config)
            .map_err(|e| format!("Failed to write sway test config: {e}"))?;

        let mut child = Command::new("sway")
            .arg("-c")
            .arg(&config_path)
            .env("XDG_RUNTIME_DIR", runtime_dir.path())
            .env("WLR_BACKENDS", "headless")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("WLR_RENDERER", "pixman")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn sway: {e}"))?;

        // Discover Wayland socket
        let start = Instant::now();
        let timeout = Duration::from_secs(5);
        let mut discovered_socket = None;

        while start.elapsed() < timeout {
            // Check if sway process died prematurely
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("Sway exited prematurely with status {status:?}"));
            }

            if let Ok(entries) = fs::read_dir(runtime_dir.path()) {
                for entry in entries.flatten() {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    if file_name.starts_with("wayland-") && !file_name.ends_with(".lock") {
                        let path = entry.path();
                        discovered_socket = Some((file_name, path));
                        break;
                    }
                }
            }

            if discovered_socket.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        let (socket_name, socket_path) = match discovered_socket {
            Some(s) => s,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Timed out waiting for sway Wayland socket in XDG_RUNTIME_DIR".to_string(),
                );
            }
        };

        // Give sway a short moment to finish initializing
        thread::sleep(Duration::from_millis(150));

        Ok(Self {
            runtime_dir,
            socket_name,
            socket_path,
            sway_process: child,
        })
    }

    /// Take a screenshot of the headless compositor using `grim`.
    pub fn screenshot(&self, output_path: &Path) -> Result<image::RgbaImage, String> {
        let status = Command::new("grim")
            .env("XDG_RUNTIME_DIR", self.runtime_dir.path())
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .arg(output_path)
            .status()
            .map_err(|e| format!("Failed to execute grim: {e}"))?;

        if !status.success() {
            return Err(format!("grim failed with exit status: {status:?}"));
        }

        let img = image::open(output_path)
            .map_err(|e| {
                format!(
                    "Failed to load screenshot at {}: {e}",
                    output_path.display()
                )
            })?
            .to_rgba8();

        Ok(img)
    }
}

impl Drop for HeadlessSwayHarness {
    fn drop(&mut self) {
        let _ = self.sway_process.kill();
        let _ = self.sway_process.wait();

        // Ensure sockets in the temp runtime directory are cleaned up
        if let Ok(entries) = fs::read_dir(self.runtime_dir.path()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("wayland-") {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
    }
}
