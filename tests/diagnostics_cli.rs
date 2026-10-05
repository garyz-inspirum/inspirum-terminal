use inspirum_terminal::{Session, save_sessions};
use std::{fs, process::Command};

fn command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_inspirum-terminal"));
    cmd.env_remove("DISPLAY");
    cmd.env_remove("WAYLAND_DISPLAY");
    cmd.env("INSPIRUM_DIAGNOSTIC_CANARY", "ENVIRONMENT_SECRET_CANARY");
    cmd
}

#[test]
fn diagnostics_run_headlessly_without_opening_a_profile_store_or_ssh_config() {
    let dir = tempfile::tempdir().unwrap();
    let profiles = dir.path().join("missing-profiles.json");
    let config = dir.path().join("missing-config");
    let output = command()
        .args(["--diagnostics", "--profiles"])
        .arg(&profiles)
        .arg("--ssh-config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("support report - schema 2"));
    assert!(text.contains("profile store not read"));
    assert!(text.contains("local probes only"));
    assert!(!text.contains("ENVIRONMENT_SECRET_CANARY"));
    assert!(!text.contains("missing-profiles"));
    assert!(!text.contains("missing-config"));
    assert!(!profiles.exists());
    assert!(!config.exists());
}

#[test]
fn selected_profile_report_omits_sensitive_fields_and_export_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let profiles = dir.path().join("PRIVATE_PATH_CANARY.json");
    let export = dir.path().join("support report.txt");
    let mut session = Session {
        name: "PROFILE_SECRET_CANARY".into(),
        host: "HOST_SECRET_CANARY".into(),
        user: "USER_SECRET_CANARY".into(),
        ..Session::default()
    };
    session.ssh.remote_command = "echo COMMAND_SECRET_CANARY".into();
    session.ssh.password_auth = Some(false);
    save_sessions(&profiles, &[session]).unwrap();
    let original_profiles = fs::read(&profiles).unwrap();
    let output = command()
        .args([
            "--diagnostics",
            "--diagnostic-profile",
            "PROFILE_SECRET_CANARY",
        ])
        .arg("--profiles")
        .arg(&profiles)
        .arg("--diagnostics-output")
        .arg(&export)
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = fs::read_to_string(&export).unwrap();
    for canary in [
        "PROFILE_SECRET_CANARY",
        "HOST_SECRET_CANARY",
        "USER_SECRET_CANARY",
        "COMMAND_SECRET_CANARY",
        "PRIVATE_PATH_CANARY",
        "ENVIRONMENT_SECRET_CANARY",
    ] {
        assert!(!text.contains(canary));
    }
    assert!(text.contains("password=disabled"));
    let again = command()
        .args(["--diagnostics", "--diagnostics-output"])
        .arg(&export)
        .output()
        .unwrap();
    assert!(!again.status.success());
    assert_eq!(fs::read_to_string(&export).unwrap(), text);
    assert_eq!(fs::read(&profiles).unwrap(), original_profiles);
}

#[test]
fn diagnostic_only_options_require_the_diagnostics_flag() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-created.txt");
    let output = command()
        .arg("--diagnostics-output")
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!path.exists());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("require --diagnostics"));
}


#[test]
fn support_report_is_deterministic_for_the_same_local_state() {
    let first = command().arg("--diagnostics").output().unwrap();
    let second = command().arg("--diagnostics").output().unwrap();
    assert!(first.status.success() && second.status.success());
    assert_eq!(first.stdout, second.stdout);
    let text = String::from_utf8(first.stdout).unwrap();
    assert!(!text.contains("ENVIRONMENT_SECRET_CANARY"));
    assert!(text.contains("arbitrary environment variables"));
}
