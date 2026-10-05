use super::*;
use std::{fs, path::PathBuf, sync::OnceLock};

fn helper() -> &'static Path {
    static HELPER: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &HELPER
        .get_or_init(|| {
            let dir = tempfile::Builder::new()
                .prefix("diagnostic helper ")
                .tempdir()
                .unwrap();
            let source = dir.path().join("probe.rs");
            let program = dir
                .path()
                .join(format!("probe{}", std::env::consts::EXE_SUFFIX));
            fs::write(
                &source,
                r#"use std::{env, io::{self, Write}, thread, time::Duration};
fn main() {
    match env::args().nth(1).as_deref() {
        Some("sleep") => thread::sleep(Duration::from_secs(30)),
        Some("large") => io::stdout().write_all(&vec![b'x'; 131072]).unwrap(),
        Some("fail") => { eprintln!("PRIVATE_ERROR_CANARY"); std::process::exit(1); }
        _ => { print!("OUT"); eprint!("ERR"); }
    }
}
"#,
            )
            .unwrap();
            let status = Command::new("rustc")
                .args(["--crate-name", "inspirum_diagnostic_probe"])
                .arg(&source)
                .arg("-o")
                .arg(&program)
                .status()
                .unwrap();
            assert!(status.success());
            (dir, program)
        })
        .1
}

#[test]
fn parses_only_version_tokens_including_windows() {
    assert_eq!(
        openssh_version("OpenSSH_9.9p2, LibreSSL 1.0"),
        Some("9.9p2".into())
    );
    assert_eq!(
        openssh_version("OpenSSH_for_Windows_9.5p1, LibreSSL 3.8.2"),
        Some("9.5p1".into())
    );
    assert!(openssh_version("OpenSSH_SECRET_CANARY").is_none());
    assert!(openssh_version("OpenSSH_9.9p1-SECRET_CANARY").is_none());
    assert!(openssh_version("OpenSSH_9..9").is_none());
}

#[test]
fn version_response_and_error_output_are_never_dumped() {
    let good = Capture {
        success: true,
        stdout: "OpenSSH_9.9p1, SECRET_CANARY".into(),
        stderr: "PRIVATE_ERROR_CANARY".into(),
    };
    assert_eq!(describe_version(Ok(good)), "OpenSSH 9.9p1");
    let failure = probe(helper(), &["fail"], PROBE_TIMEOUT).unwrap();
    assert!(!failure.success);
    assert!(!describe_version(Ok(failure)).contains("PRIVATE_ERROR_CANARY"));
}

#[test]
fn missing_tool_returns_a_static_category_without_the_path() {
    let result = probe(Path::new("inspirum-no-such-tool-82b1"), &[], PROBE_TIMEOUT);
    assert!(matches!(result, Err(ProbeFailure::Missing)));
    assert_eq!(ProbeFailure::Missing.label(), "not found on PATH");
}

#[test]
fn captures_both_streams_without_a_pipe_deadlock() {
    let result = probe(helper(), &[], PROBE_TIMEOUT).unwrap();
    assert!(result.success);
    assert_eq!(result.stdout, "OUT");
    assert_eq!(result.stderr, "ERR");
}

#[test]
fn timeout_terminates_and_reaps_the_probe() {
    let program = helper();
    let start = Instant::now();
    let result = probe(program, &["sleep"], Duration::from_millis(100));
    assert!(matches!(result, Err(ProbeFailure::Timeout)));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn excessive_output_is_rejected_instead_of_returned() {
    let result = probe(helper(), &["large"], PROBE_TIMEOUT);
    assert!(matches!(result, Err(ProbeFailure::OutputLimit)));
}

#[test]
fn profile_summary_is_deterministic_and_never_contains_profile_strings() {
    let mut session = Session {
        name: "NAME_CANARY".into(),
        host: "HOST_CANARY".into(),
        user: "USER_CANARY".into(),
        strict: true,
        ..Session::default()
    };
    session.ssh.identity_file = "/private/KEY_PATH_CANARY".into();
    session.ssh.proxy_jump = "JUMP_CANARY".into();
    session.ssh.remote_command = "echo COMMAND_SECRET_CANARY".into();
    session.ssh.local_forwards = vec!["127.0.0.1:2222:FORWARD_CANARY:22".into()];
    session.ssh.password_auth = Some(false);
    session.ssh.public_key_auth = Some(true);
    let text = profile_summary(&session);
    assert_eq!(text, profile_summary(&session));
    for canary in [
        "NAME_CANARY",
        "HOST_CANARY",
        "USER_CANARY",
        "KEY_PATH_CANARY",
        "JUMP_CANARY",
        "COMMAND_SECRET_CANARY",
        "FORWARD_CANARY",
    ] {
        assert!(!text.contains(canary));
    }
    assert!(text.contains("already trusted only"));
    assert!(text.contains("public-key=enabled, password=disabled"));
    assert!(text.contains("local=1, remote=0, dynamic=0"));
}

#[test]
fn report_export_is_no_clobber_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("support.txt");
    export(&path, "original report").unwrap();
    assert!(export(&path, "replacement report").is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "original report");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    let large = "x".repeat(OUTPUT_LIMIT as usize + 1);
    assert!(export(&dir.path().join("too-large.txt"), &large).is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
