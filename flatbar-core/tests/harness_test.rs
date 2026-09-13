mod common;

use common::HeadlessSwayHarness;

#[test]
fn test_headless_sway_harness() {
    let harness = match HeadlessSwayHarness::try_new() {
        Ok(h) => h,
        Err(msg) => {
            eprintln!("SKIPPED: {msg}");
            return;
        }
    };

    println!("Booted headless sway on socket: {}", harness.socket_name);
    let screenshot_path = harness.runtime_dir.path().join("screen.png");
    let img = harness
        .screenshot(&screenshot_path)
        .expect("Failed to screenshot headless sway");

    assert_eq!(img.width(), 1280);
    assert_eq!(img.height(), 800);
}
