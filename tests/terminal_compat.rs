#![cfg(unix)]

use egui_term::{BackendCommand, BackendSettings, TerminalBackend, TerminalMode};
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn backend_for(script: &str) -> TerminalBackend {
    let (tx, _rx) = mpsc::channel();
    TerminalBackend::new(
        2300,
        eframe::egui::Context::default(),
        tx,
        BackendSettings {
            shell: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            working_directory: None,
        },
    )
    .expect("create compatibility PTY")
}

fn grid_text(backend: &mut TerminalBackend) -> String {
    backend
        .sync()
        .grid
        .display_iter()
        .map(|cell| cell.c)
        .collect()
}

fn wait_for_text(backend: &mut TerminalBackend, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let text = grid_text(backend);
        if text.contains(needle) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needle:?}; visible grid was {text:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn unicode_wide_combining_and_emoji_survive_terminal_path() {
    let mut backend = backend_for(
        "printf '%s\\n' 'ASCII 中文 é 😀' 'UNICODE_DONE'; sleep 1",
    );
    let text = wait_for_text(&mut backend, "UNICODE_DONE", Duration::from_secs(5));

    assert!(text.contains("ASCII"));
    assert!(text.contains("中文"));
    assert!(text.contains('😀'));
    assert!(text.contains('e'));
    assert!(
        !text.contains('�'),
        "terminal path introduced a Unicode replacement character: {text:?}"
    );
}

#[test]
fn alternate_screen_switches_and_restores_primary_screen() {
    let mut backend = backend_for(
        "printf '\\033[?1049hALT_SCREEN\\n'; sleep 1; \
         printf '\\033[?1049lMAIN_SCREEN\\n'; sleep 1",
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_alt = false;

    while Instant::now() < deadline {
        let content = backend.sync();
        let text: String = content.grid.display_iter().map(|cell| cell.c).collect();
        if content.terminal_mode.contains(TerminalMode::ALT_SCREEN) && text.contains("ALT_SCREEN") {
            saw_alt = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    assert!(saw_alt, "alternate screen mode was never observed");
    let text = wait_for_text(&mut backend, "MAIN_SCREEN", Duration::from_secs(5));
    assert!(
        !backend.sync().terminal_mode.contains(TerminalMode::ALT_SCREEN),
        "alternate screen mode remained enabled after ?1049l"
    );
    assert!(text.contains("MAIN_SCREEN"));
}

#[test]
fn sgr_mouse_mode_is_observable_and_can_be_disabled() {
    let mut backend = backend_for(
        "printf '\\033[?1000h\\033[?1006hMOUSE_ON\\n'; sleep 1; \
         printf '\\033[?1006l\\033[?1000lMOUSE_OFF\\n'; sleep 1",
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_sgr = false;

    while Instant::now() < deadline {
        let content = backend.sync();
        let text: String = content.grid.display_iter().map(|cell| cell.c).collect();
        if content.terminal_mode.contains(TerminalMode::SGR_MOUSE) && text.contains("MOUSE_ON") {
            saw_sgr = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    assert!(saw_sgr, "SGR mouse mode was never observed");
    let _ = wait_for_text(&mut backend, "MOUSE_OFF", Duration::from_secs(5));
    assert!(
        !backend.sync().terminal_mode.contains(TerminalMode::SGR_MOUSE),
        "SGR mouse mode remained enabled after ?1006l"
    );
}

#[test]
fn representative_vt_cursor_and_erase_sequences_render_expected_state() {
    let mut backend = backend_for(
        "printf 'abcdef'; printf '\\033[3DXYZ'; printf '\\n'; \
         printf 'ERASE_ME'; printf '\\r\\033[2KVT_DONE\\n'; sleep 1",
    );
    let text = wait_for_text(&mut backend, "VT_DONE", Duration::from_secs(5));

    assert!(text.contains("abcXYZ"), "cursor-back overwrite failed: {text:?}");
    assert!(
        !text.contains("ERASE_ME"),
        "CSI 2K did not erase the target line: {text:?}"
    );
}

#[test]
fn high_volume_output_keeps_terminal_responsive_and_scrollback_navigable() {
    let mut backend = backend_for(
        "i=0; while [ \"$i\" -lt 5000 ]; do \
         printf 'line-%05d payload-abcdefghijklmnopqrstuvwxyz0123456789\\n' \"$i\"; \
         i=$((i + 1)); done; printf 'HIGH_VOLUME_DONE\\n'; sleep 1",
    );

    let _ = wait_for_text(
        &mut backend,
        "HIGH_VOLUME_DONE",
        Duration::from_secs(15),
    );
    backend.process_command(BackendCommand::Scroll(80));
    let scrolled = grid_text(&mut backend);

    assert!(scrolled.contains("line-"), "scrollback did not retain earlier output");
    assert!(
        !scrolled.contains("HIGH_VOLUME_DONE"),
        "scrolling did not move away from the live bottom"
    );
}

#[test]
#[ignore = "opt-in throughput benchmark; run with --ignored --nocapture"]
fn terminal_high_volume_benchmark() {
    let mut backend = backend_for(
        "i=0; while [ \"$i\" -lt 20000 ]; do \
         printf 'bench-%05d payload-abcdefghijklmnopqrstuvwxyz0123456789\\n' \"$i\"; \
         i=$((i + 1)); done; printf 'BENCH_DONE\\n'; sleep 1",
    );
    let started = Instant::now();
    let _ = wait_for_text(&mut backend, "BENCH_DONE", Duration::from_secs(30));
    let elapsed = started.elapsed();

    backend.process_command(BackendCommand::Scroll(200));
    let scrolled = grid_text(&mut backend);
    assert!(scrolled.contains("bench-"));

    eprintln!(
        "terminal_compat benchmark: 20000 lines reached BENCH_DONE in {:.3}s; scrollback remained navigable",
        elapsed.as_secs_f64()
    );
}
