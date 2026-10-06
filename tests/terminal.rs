use inspirum_terminal::{
    Session,
    terminal::{
        HostKeyTarget, check_openssh, check_sftp, connect, inspect_known_host, launch_args,
        parse_host_key_target, remove_known_host, resolve_host_key_target, sftp_launch_args,
    },
};
#[cfg(unix)]
use std::thread;
use std::{
    path::Path,
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

const ARGV_DUMP_HELPER: &str = r#"use std::{env, fmt::Write as _, fs};
fn main() {
    let mut args = env::args_os();
    let executable = args.next().unwrap().into_string().unwrap();
    let output = args.next().unwrap();
    let values: Vec<_> = args.collect();
    let mut encoded = String::from("ARGV0=");
    for byte in executable.into_bytes() {
        write!(&mut encoded, "{byte:02X}").unwrap();
    }
    write!(&mut encoded, "\nARGC={}\n", values.len()).unwrap();
    for (index, value) in values.into_iter().enumerate() {
        write!(&mut encoded, "ARG{index}=").unwrap();
        for byte in value.into_string().unwrap().into_bytes() {
            write!(&mut encoded, "{byte:02X}").unwrap();
        }
        encoded.push('\n');
    }
    fs::write(output, encoded).unwrap();
}
"#;

fn compile_argv_dump_helper(source: &Path, executable: &Path) {
    std::fs::write(source, ARGV_DUMP_HELPER).unwrap();
    let compiled = Command::new("rustc")
        .arg("--crate-name")
        .arg("inspirum_argv_probe")
        .arg(source)
        .arg("-o")
        .arg(executable)
        .status()
        .unwrap();
    assert!(compiled.success(), "native argv helper did not compile");
}

#[test]
fn native_argv_helper_source_with_spaced_filename_compiles() {
    let dir = tempfile::Builder::new()
        .prefix("inspirum argv compiler ")
        .tempdir()
        .unwrap();
    let source = dir.path().join("argv dump helper.rs");
    let executable = dir
        .path()
        .join(format!("argv dump helper{}", std::env::consts::EXE_SUFFIX));
    compile_argv_dump_helper(&source, &executable);
    assert!(executable.is_file());
}

#[test]
fn windows_program_serialization_quotes_one_token_and_rejects_unsafe_input() {
    assert_eq!(
        egui_term::serialize_windows_program(r"C:\Program Files\OpenSSH\ssh.exe").unwrap(),
        r#""C:\Program Files\OpenSSH\ssh.exe""#
    );
    assert_eq!(
        egui_term::serialize_windows_program("ssh").unwrap(),
        r#""ssh""#
    );
    assert!(egui_term::serialize_windows_program("bad\0path").is_err());
    assert!(egui_term::serialize_windows_program("bad\"path").is_err());
}

#[cfg(unix)]
fn wait_for_grid(backend: &mut egui_term::TerminalBackend, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let text: String = backend
            .sync()
            .grid
            .display_iter()
            .map(|cell| cell.c)
            .collect();
        if text.contains(needle) {
            return text;
        }
        assert!(Instant::now() < deadline, "missing {needle:?} in {text:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn line_capture_backend(id: u64) -> egui_term::TerminalBackend {
    let (tx, _rx) = mpsc::channel();
    egui_term::TerminalBackend::new(
        id,
        eframe::egui::Context::default(),
        tx,
        egui_term::BackendSettings {
            shell: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "IFS= read -r line; printf '\\nCAPTURE=<%s>\\n' \"$line\"; sleep 1".into(),
            ],
            working_directory: None,
        },
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
fn retained_history_snapshot_includes_scrollback_and_survives_viewport_navigation() {
    let (tx, _rx) = mpsc::channel();
    let mut backend = egui_term::TerminalBackend::new(
        91,
        eframe::egui::Context::default(),
        tx,
        egui_term::BackendSettings {
            shell: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                r#"i=1; while [ $i -le 80 ]; do printf 'HIST-%03d\n' "$i"; i=$((i+1)); done; sleep 1"#.into(),
            ],
            working_directory: None,
        },
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let history = loop {
        let lines = backend.history_lines();
        if lines.iter().any(|line| line.contains("HIST-080")) {
            break lines;
        }
        assert!(Instant::now() < deadline, "retained history did not fill");
        thread::sleep(Duration::from_millis(20));
    };

    let retained_scrollback = history
        .iter()
        .position(|line| line.contains("HIST-040"))
        .expect("offscreen generated line retained");
    assert!(
        history.iter().any(|line| line.contains("HIST-080")),
        "newest generated line retained"
    );
    assert!(backend.scroll_to_history_index(retained_scrollback));
    assert!(
        backend
            .history_lines()
            .iter()
            .any(|line| line.contains("HIST-040")),
        "scrolling the viewport must not alter retained history"
    );
}

#[test]
fn missing_ssh_has_actionable_error() {
    let error = check_openssh(Path::new("inspirum-nonexistent-ssh-7e65"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("OpenSSH"));
    assert!(error.contains("Windows"));
}
#[test]
fn config_path_is_one_argument_and_policy_precedes_it() {
    let mut session = Session {
        host: "alias".into(),
        ..Session::default()
    };
    session.ssh.remote_command = "printf ready".into();
    let args = launch_args(&session, Some(Path::new("config with spaces"))).unwrap();
    assert_eq!(
        args,
        [
            "-tt",
            "-o",
            "StrictHostKeyChecking=ask",
            "-F",
            "config with spaces",
            "--",
            "alias",
            "printf ready"
        ]
    );
}
#[test]
fn missing_sftp_has_actionable_error() {
    let error = check_sftp(Path::new("inspirum-nonexistent-sftp-7e65"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("sftp"));
    assert!(error.contains("OpenSSH"));
}

#[test]
fn sftp_args_reuse_auth_routing_and_connection_policy() {
    let mut session = Session {
        host: "work-alias".into(),
        user: "alice".into(),
        port: Some(2222),
        strict: true,
        ..Session::default()
    };
    session.ssh.identity_file = "/keys/work key".into();
    session.ssh.proxy_jump = "bastion".into();
    session.ssh.compression = Some(false);
    session.ssh.connect_timeout_seconds = Some(12);
    session.ssh.server_alive_interval_seconds = Some(30);
    session.ssh.agent_forwarding = Some(true);
    session.ssh.x11_forwarding = Some(true);
    session.ssh.remote_command = "ignored for sftp".into();
    session.ssh.local_forwards = vec!["127.0.0.1:8080:internal:80".into()];

    assert_eq!(
        sftp_launch_args(&session, Some(Path::new("config with spaces"))).unwrap(),
        [
            "-o",
            "StrictHostKeyChecking=yes",
            "-F",
            "config with spaces",
            "-i",
            "/keys/work key",
            "-J",
            "bastion",
            "-o",
            "Compression=no",
            "-o",
            "ConnectTimeout=12",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "User=alice",
            "-P",
            "2222",
            "work-alias"
        ]
    );
}

#[test]
fn host_key_target_matches_openssh_known_hosts_identity_rules() {
    assert_eq!(
        parse_host_key_target("hostname example.test\nport 22\nhostkeyalias none\n").unwrap(),
        HostKeyTarget {
            hostname: "example.test".into(),
            port: 22,
            host_key_alias: None,
            lookup: "example.test".into(),
        }
    );
    assert_eq!(
        parse_host_key_target("hostname example.test\nport 2222\n")
            .unwrap()
            .lookup,
        "[example.test]:2222"
    );
    assert_eq!(
        parse_host_key_target("hostname 2001:db8::7\nport 2200\n")
            .unwrap()
            .lookup,
        "[2001:db8::7]:2200"
    );
    let alias =
        parse_host_key_target("hostname 2001:db8::7\nport 2200\nhostkeyalias pinned-name\n")
            .unwrap();
    assert_eq!(alias.host_key_alias.as_deref(), Some("pinned-name"));
    assert_eq!(alias.lookup, "pinned-name");
    assert!(parse_host_key_target("port 22\n").is_err());
    assert!(parse_host_key_target("hostname host\nport 0\n").is_err());
}

#[test]
fn ssh_g_resolves_config_alias_for_known_hosts_target_without_connecting() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    std::fs::write(
        &config,
        "Host trust-alias\n HostName 2001:db8::9\n Port 2201\n HostKeyAlias pinned-via-config\n",
    )
    .unwrap();
    let session = Session {
        host: "trust-alias".into(),
        ..Session::default()
    };
    let target = resolve_host_key_target(&session, Some(&config)).unwrap();
    assert_eq!(target.hostname, "2001:db8::9");
    assert_eq!(target.port, 2201);
    assert_eq!(target.host_key_alias.as_deref(), Some("pinned-via-config"));
    assert_eq!(target.lookup, "pinned-via-config");
}

#[test]
fn ssh_keygen_inspection_and_removal_use_exact_known_hosts_target() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("host-key");
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(status.success(), "ssh-keygen test key generation failed");

    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let mut fields = public.split_whitespace();
    let key_type = fields.next().unwrap();
    let key_body = fields.next().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    std::fs::write(
        &known_hosts,
        format!("[example.test]:2222 {key_type} {key_body}\n"),
    )
    .unwrap();
    let hashed = Command::new("ssh-keygen")
        .args(["-q", "-H", "-f"])
        .arg(&known_hosts)
        .status()
        .unwrap();
    assert!(hashed.success(), "known_hosts hashing failed");
    let _ = std::fs::remove_file(known_hosts.with_extension("old"));
    assert!(
        !std::fs::read_to_string(&known_hosts)
            .unwrap()
            .contains("example.test")
    );

    let target = HostKeyTarget {
        hostname: "example.test".into(),
        port: 2222,
        host_key_alias: None,
        lookup: "[example.test]:2222".into(),
    };
    let found = inspect_known_host(&target, Some(&known_hosts)).unwrap();
    assert!(found.contains("[example.test]:2222"), "{found}");
    assert!(found.contains("SHA256:"), "{found}");

    let removal = remove_known_host(&target, Some(&known_hosts)).unwrap();
    assert!(
        removal.contains("known_hosts") || removal.contains("found"),
        "{removal}"
    );
    assert_eq!(inspect_known_host(&target, Some(&known_hosts)).unwrap(), "");
    assert!(
        std::fs::read_to_string(&known_hosts)
            .unwrap()
            .trim()
            .is_empty()
    );
}

#[test]
fn explicit_missing_known_hosts_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let target = HostKeyTarget {
        hostname: "example.test".into(),
        port: 22,
        host_key_alias: None,
        lookup: "example.test".into(),
    };
    let missing = dir.path().join("missing-known-hosts");
    let error = inspect_known_host(&target, Some(&missing))
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not exist"), "{error}");
}

#[test]
fn headless_widget_receives_actual_ssh_exit() {
    let (tx, rx) = mpsc::channel();
    let context = eframe::egui::Context::default();
    let session = Session {
        host: "127.0.0.1".into(),
        port: Some(1),
        strict: true,
        ..Session::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    std::fs::write(&config, "Host *\n ConnectTimeout 2\n BatchMode yes\n").unwrap();
    let mut backend = connect(1, context.clone(), tx, &session, Some(&config)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut exited = false;
    while Instant::now() < deadline {
        if let Ok((_, egui_term::PtyEvent::Exit)) = rx.recv_timeout(Duration::from_millis(50)) {
            exited = true;
            break;
        }
    }
    assert!(exited, "OpenSSH child did not exit");
    let output = context.run(eframe::egui::RawInput::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            let view = egui_term::TerminalView::new(ui, &mut backend);
            ui.add(view);
        });
    });
    assert!(!output.shapes.is_empty());
}

#[cfg(unix)]
#[test]
fn focused_terminal_dispatches_keyboard_events_after_pointer_leaves() {
    use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};

    let context = eframe::egui::Context::default();
    let mut backend = line_capture_backend(81);
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0));
    let press = RawInput {
        screen_rect: Some(screen),
        events: vec![
            Event::PointerMoved(Pos2::new(50.0, 50.0)),
            Event::PointerButton {
                pos: Pos2::new(50.0, 50.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::default(),
            },
        ],
        ..RawInput::default()
    };
    let _ = context.run(press, |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            let view = egui_term::TerminalView::new(ui, &mut backend)
                .set_size(vec2(200.0, 100.0))
                .set_focus(true);
            ui.add(view);
        });
    });

    let release = RawInput {
        screen_rect: Some(screen),
        events: vec![
            Event::PointerMoved(Pos2::new(50.0, 50.0)),
            Event::PointerButton {
                pos: Pos2::new(50.0, 50.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::default(),
            },
        ],
        ..RawInput::default()
    };
    let focused = std::cell::Cell::new(false);
    let _ = context.run(release, |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            let view = egui_term::TerminalView::new(ui, &mut backend)
                .set_size(vec2(200.0, 100.0))
                .set_focus(true);
            focused.set(ui.add(view).has_focus());
        });
    });
    assert!(
        focused.get(),
        "terminal must retain focus after pointer click dispatch"
    );

    let text_input = RawInput {
        screen_rect: Some(screen),
        events: vec![
            Event::PointerMoved(Pos2::new(500.0, 400.0)),
            Event::Text("typed".into()),
        ],
        ..RawInput::default()
    };
    let _ = context.run(text_input, |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            let view = egui_term::TerminalView::new(ui, &mut backend)
                .set_size(vec2(200.0, 100.0))
                .set_focus(true);
            ui.add(view);
        });
    });
    let paste_input = RawInput {
        screen_rect: Some(screen),
        modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
        events: vec![Event::Paste("-pasted".into())],
        ..RawInput::default()
    };
    let _ = context.run(paste_input, |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            let view = egui_term::TerminalView::new(ui, &mut backend)
                .set_size(vec2(200.0, 100.0))
                .set_focus(true);
            ui.add(view);
        });
    });
    let enter_input = RawInput {
        screen_rect: Some(screen),
        events: vec![Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        }],
        ..RawInput::default()
    };
    let _ = context.run(enter_input, |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            let view = egui_term::TerminalView::new(ui, &mut backend)
                .set_size(vec2(200.0, 100.0))
                .set_focus(true);
            ui.add(view);
        });
    });
    wait_for_grid(&mut backend, "CAPTURE=<typed-pasted>");
}

#[cfg(unix)]
#[test]
fn focused_form_dispatch_does_not_leak_into_hovered_terminal() {
    use eframe::egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};

    let context = eframe::egui::Context::default();
    let mut backend = line_capture_backend(82);
    let mut form = String::new();
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0));
    let _ = context.run(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                ui.text_edit_singleline(&mut form).request_focus();
                let view = egui_term::TerminalView::new(ui, &mut backend)
                    .set_size(vec2(200.0, 100.0))
                    .set_focus(false);
                ui.add(view);
            });
        },
    );
    let _ = context.run(
        RawInput {
            screen_rect: Some(screen),
            events: vec![
                Event::PointerMoved(Pos2::new(50.0, 60.0)),
                Event::Text("form-typed".into()),
                Event::Paste("-pasted".into()),
                Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::default(),
                },
            ],
            ..RawInput::default()
        },
        |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                ui.text_edit_singleline(&mut form);
                let view = egui_term::TerminalView::new(ui, &mut backend)
                    .set_size(vec2(200.0, 100.0))
                    .set_focus(false);
                ui.add(view);
            });
        },
    );
    assert_eq!(form, "form-typed-pasted");
    backend.process_command(egui_term::BackendCommand::Write(
        b"expected-only\n".to_vec(),
    ));
    let text = wait_for_grid(&mut backend, "CAPTURE=<expected-only>");
    assert!(!text.contains("form-typed"), "form input leaked: {text:?}");
    assert!(!text.contains("pasted"), "form paste leaked: {text:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn subscription_spawn_failure_rolls_back_started_pty() {
    use std::{fs, io};

    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("spawn-failure.pid");
    let (tx, _rx) = mpsc::channel();
    let settings = egui_term::BackendSettings {
        shell: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "echo $$ > \"$1\"; exec sleep 30".into(),
            "inspirum-spawn-failure".into(),
            pid_file.to_string_lossy().into_owned(),
        ],
        working_directory: None,
    };
    let result = egui_term::TerminalBackend::new_with_subscription_spawner(
        83,
        eframe::egui::Context::default(),
        tx,
        settings,
        |_, _| {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !pid_file.is_file() {
                assert!(Instant::now() < deadline, "PTY child never started");
                thread::sleep(Duration::from_millis(10));
            }
            Err(io::Error::other("injected subscription spawn failure"))
        },
    );
    assert!(result.is_err());
    let pid: u32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let process = std::path::PathBuf::from(format!("/proc/{pid}"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.exists() {
        assert!(
            Instant::now() < deadline,
            "PTY child {pid} survived subscription spawn rollback"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn disconnected_subscriber_stops_forwarding_thread() {
    use std::fs;

    fn subscription_threads() -> usize {
        fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|task| fs::read_to_string(task.path().join("comm")).ok())
            .filter(|name| name.trim() == "pty_event_subsc")
            .count()
    }

    let baseline = subscription_threads();
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let backend = egui_term::TerminalBackend::new(
        84,
        eframe::egui::Context::default(),
        tx,
        egui_term::BackendSettings {
            shell: "/bin/sh".into(),
            args: vec!["-c".into(), "exit 0".into()],
            working_directory: None,
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if subscription_threads() == baseline {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "subscriber survived disconnected forwarding channel"
        );
        thread::sleep(Duration::from_millis(20));
    }
    drop(backend);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_pty_preserves_actual_child_argument_boundaries() {
    use std::fmt::Write as _;

    let dir = tempfile::Builder::new()
        .prefix("inspirum argv spaces ")
        .tempdir()
        .unwrap();
    let source = dir.path().join("argv dump helper.rs");
    let executable = dir.path().join("argv dump helper.exe");
    let output = dir.path().join("captured argv.txt");
    compile_argv_dump_helper(&source, &executable);

    let expected = [
        "value with spaces",
        "quote\"inside",
        r"C:\directory with spaces\",
    ];
    let mut args = vec![output.to_string_lossy().into_owned()];
    args.extend(expected.iter().map(|value| (*value).to_owned()));
    let (tx, rx) = mpsc::channel();
    let mut backend = egui_term::TerminalBackend::new(
        88,
        eframe::egui::Context::default(),
        tx,
        egui_term::BackendSettings {
            shell: executable.to_string_lossy().into_owned(),
            args,
            working_directory: None,
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut exited = false;
    while Instant::now() < deadline {
        if matches!(
            rx.recv_timeout(Duration::from_millis(50)),
            Ok((88, egui_term::PtyEvent::Exit))
        ) {
            exited = true;
            break;
        }
    }
    assert!(exited, "native argv helper did not emit PTY exit");

    let mut encoded = String::from("ARGV0=");
    for byte in executable.to_string_lossy().as_bytes() {
        write!(&mut encoded, "{byte:02X}").unwrap();
    }
    write!(&mut encoded, "\nARGC={}\n", expected.len()).unwrap();
    for (index, value) in expected.iter().enumerate() {
        write!(&mut encoded, "ARG{index}=").unwrap();
        for byte in value.as_bytes() {
            write!(&mut encoded, "{byte:02X}").unwrap();
        }
        encoded.push('\n');
    }
    assert_eq!(std::fs::read_to_string(output).unwrap(), encoded);
    let _ = backend.sync();
}
