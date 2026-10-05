use inspirum_terminal::{
    Session,
    terminal::{check_openssh, connect, launch_args},
};
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};
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
    let session = Session {
        host: "alias".into(),
        ..Session::default()
    };
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
            "alias"
        ]
    );
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
    let text: String = backend
        .sync()
        .grid
        .display_iter()
        .map(|cell| cell.c)
        .collect();
    assert!(
        text.contains("Connection refused") || text.contains("Permission denied"),
        "{text}"
    );
    let output = context.run(eframe::egui::RawInput::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            let view = egui_term::TerminalView::new(ui, &mut backend);
            ui.add(view);
        });
    });
    assert!(!output.shapes.is_empty());
}

#[cfg(target_os = "windows")]
#[test]
fn windows_pty_preserves_actual_child_argument_boundaries() {
    let dir = tempfile::Builder::new()
        .prefix("inspirum argv spaces ")
        .tempdir()
        .unwrap();
    let script = dir.path().join("argv probe with spaces.ps1");
    std::fs::write(
        &script,
        r#"$i = 0
foreach ($value in $args) {
    Write-Output ("ARG{0}=<{1}>" -f $i, $value)
    $i++
}
"#,
    )
    .unwrap();
    let expected = [
        "value with spaces",
        "quote\"inside",
        r"C:\directory with spaces\",
    ];
    let mut args = vec![
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-File".into(),
        script.to_string_lossy().into_owned(),
    ];
    args.extend(expected.iter().map(|value| (*value).to_owned()));
    let (tx, rx) = mpsc::channel();
    let mut backend = egui_term::TerminalBackend::new(
        88,
        eframe::egui::Context::default(),
        tx,
        egui_term::BackendSettings {
            shell: "powershell.exe".into(),
            args,
            working_directory: None,
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if matches!(
            rx.recv_timeout(Duration::from_millis(50)),
            Ok((88, egui_term::PtyEvent::Exit))
        ) {
            break;
        }
    }
    let text: String = backend
        .sync()
        .grid
        .display_iter()
        .map(|cell| cell.c)
        .collect();
    for (index, value) in expected.iter().enumerate() {
        assert!(
            text.contains(&format!("ARG{index}=<{value}>")),
            "missing argv[{index}] in {text:?}"
        );
    }
}
