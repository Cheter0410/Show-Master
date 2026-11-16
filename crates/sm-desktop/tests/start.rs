//! The start screen and the setup wizard of a new show.
//! `SM_SHOTS=<folder> cargo test -p sm-desktop --test start` also writes
//! pictures of every step.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{settle, shot, Env};
use egui_kittest::kittest::Queryable;
use sm_core::model::Show;
use sm_desktop::{Page, Step};

#[test]
fn a_new_show_runs_through_the_wizard() {
    let env = Env::start(Show::default());
    let mut h = env.harness();
    h.state_mut().page = Page::Start;
    settle(&mut h);
    shot(&mut h, "sm-1-start");

    h.get_by_label("New show").click();
    settle(&mut h);
    let place = env.dir.path().join("shows");
    h.state_mut().start.location = place.clone();
    h.get_by_label("Show name").click();
    settle(&mut h);
    h.get_by_label("Show name").type_text("Spring Concert");
    settle(&mut h);
    shot(&mut h, "sm-2-new");
    h.get_by_label("Create and set up").click();
    settle(&mut h);
    // A new show opens the audio device of the new show: can take a while.
    let file = place.join("Spring Concert").join("Spring Concert.smshow");
    let start = std::time::Instant::now();
    while h.state().page != Page::Wizard(Step::Audio) {
        assert!(
            start.elapsed().as_secs() < 60,
            "no wizard: {:?}",
            h.state().start.error
        );
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    settle(&mut h);
    assert_eq!(h.state().page, Page::Wizard(Step::Audio));
    assert!(file.is_file());
    assert_eq!(env.state()["show"]["name"], "Spring Concert");

    for (step, name) in [
        (Step::Outputs, "sm-3-audio"),
        (Step::Timecode, "sm-4-outputs"),
        (Step::Remote, "sm-5-timecode"),
        (Step::Ready, "sm-6-remote"),
    ] {
        shot(&mut h, name);
        h.get_by_label("Next").click();
        settle(&mut h);
        assert_eq!(h.state().page, Page::Wizard(step));
    }
    shot(&mut h, "sm-7-ready");
    h.get_by_label("Back").click();
    settle(&mut h);
    assert_eq!(h.state().page, Page::Wizard(Step::Remote));
    h.get_by_label("Next").click();
    settle(&mut h);
    h.get_by_label("Open show").click();
    settle(&mut h);
    assert_eq!(h.state().page, Page::Main);
    shot(&mut h, "sm-8-main");
}

#[test]
fn skip_setup_opens_the_show() {
    let env = Env::start(Show::default());
    let mut h = env.harness();
    h.state_mut().page = Page::Wizard(Step::Timecode);
    settle(&mut h);
    h.get_by_label("Skip setup").click();
    settle(&mut h);
    assert_eq!(h.state().page, Page::Main);
}

#[test]
fn a_recent_show_opens_from_the_start_screen() {
    let env = Env::demo();
    let mut h = env.harness();
    assert_eq!(h.state().page, Page::Main);
    env.wait(&mut h, "the demo show", |s| s["show_path"].is_string());
    let path = std::path::PathBuf::from(env.state()["show_path"].as_str().unwrap());
    h.state_mut().recent.add(&path);
    h.state_mut().back_to_start();
    settle(&mut h);
    assert_eq!(h.state().page, Page::Start);
    shot(&mut h, "sm-9-start-recent");
    let name = path.file_stem().unwrap().to_string_lossy().to_string();
    h.get_by_label(&name).click();
    settle(&mut h);
    assert_eq!(h.state().page, Page::Main);
}

#[test]
fn small_window_start_screen_fits() {
    let env = Env::start(Show::default());
    let mut h = env.harness_sized(egui::vec2(760.0, 540.0));
    h.state_mut().page = Page::Start;
    settle(&mut h);
    shot(&mut h, "sm-10-start-small");
    h.state_mut().page = Page::Wizard(Step::Outputs);
    settle(&mut h);
    shot(&mut h, "sm-11-wizard-small");
    assert!(h.query_by_label("Next").is_some());
}
