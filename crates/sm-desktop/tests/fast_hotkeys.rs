//! The hidden-window fast path of system-wide hotkeys. Its state is
//! process-wide, so it lives in its own test binary (no window harness in
//! parallel that would report "shown").

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::Env;

#[test]
fn hidden_window_hotkeys_go_straight_to_the_runtime() {
    use sm_desktop::global_hotkeys::fast;
    let env = Env::demo();
    let before = env.state()["playheads"].clone();
    // Window shown: the UI pass handles hotkeys, the fast path stays out.
    fast::update(&env.handle, false, None);
    assert!(!fast::fire_hidden("go"));
    // Hidden: GO is sent at once, without any UI pass.
    fast::update(&env.handle, true, None);
    assert!(fast::fire_hidden("go"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while env.state()["playheads"] == before {
        assert!(std::time::Instant::now() < deadline, "GO did not arrive");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Editing / UI actions are not fast-path actions.
    assert!(!fast::fire_hidden("save"));
    fast::update(&env.handle, false, None);
}
