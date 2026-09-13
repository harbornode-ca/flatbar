mod common;

use common::HeadlessSwayHarness;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn find_flatbar_binary() -> PathBuf {
    let mut path = env::current_exe().expect("Failed to get current_exe");
    // e.g. target/debug/deps/m1_smoke_test-xyz -> target/debug/flatbar
    path.pop(); // pop test binary name
    if path.file_name().and_then(|f| f.to_str()) == Some("deps") {
        path.pop(); // pop "deps"
    }
    path.push("flatbar");
    path
}

#[test]
fn test_m1_bar_renders_on_headless_sway() {
    let harness = match HeadlessSwayHarness::try_new() {
        Ok(h) => h,
        Err(msg) => {
            eprintln!("SKIPPED: {msg}");
            return;
        }
    };

    // Create test config
    let config_path = harness.runtime_dir.path().join("test_config.toml");
    let test_config = r##"
        [bar]
        name = "test-bar"
        height = 30
        background = "#1e1e2e"
        foreground = "#cdd6f4"
        position = "top"
        font_family = "sans-serif"
        font_size = 13.0

        [widgets.datetime]
        type = "datetime"
        format = "%H:%M:%S"
        interval = 1
    "##;
    fs::write(&config_path, test_config).expect("Failed to write test config");

    let bin_path = find_flatbar_binary();

    // Spawn flatbar against the headless sway display
    let mut flatbar_proc = Command::new(&bin_path)
        .arg("--config")
        .arg(&config_path)
        .env("XDG_RUNTIME_DIR", harness.runtime_dir.path())
        .env("WAYLAND_DISPLAY", &harness.socket_name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn flatbar at {}: {e}", bin_path.display()));

    // Give flatbar a moment to connect and map its layer surface
    thread::sleep(Duration::from_millis(600));

    // Capture screenshot
    let screenshot_path = harness.runtime_dir.path().join("bar_screenshot.png");
    let img = harness
        .screenshot(&screenshot_path)
        .expect("Failed to screenshot headless sway with flatbar running");

    // Cleanup flatbar process
    let _ = flatbar_proc.kill();
    let _ = flatbar_proc.wait();

    assert_eq!(img.width(), 1280);
    assert_eq!(img.height(), 800);

    // Assert that the top 30px bar contains the background color (#1e1e2e -> rgb(30, 30, 46))
    // and foreground pixels (#cdd6f4 -> rgb(205, 214, 244))
    let mut found_bar_bg = false;
    let mut found_bar_fg = false;

    for y in 0..30 {
        for x in 0..1280 {
            let pixel = img.get_pixel(x, y);
            // Check for background color ~ (30, 30, 46)
            if (pixel[0] as i32 - 30).abs() <= 5
                && (pixel[1] as i32 - 30).abs() <= 5
                && (pixel[2] as i32 - 46).abs() <= 5
            {
                found_bar_bg = true;
            }
            // Check for text pixels (significantly brighter than background)
            if pixel[0] > 100 && pixel[1] > 100 && pixel[2] > 100 {
                found_bar_fg = true;
            }
        }
    }

    assert!(
        found_bar_bg,
        "Bar background pixels not found in screenshot"
    );
    assert!(
        found_bar_fg,
        "Bar text/foreground pixels not found in screenshot"
    );
}
