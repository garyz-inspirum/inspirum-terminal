use inspirum_terminal::{ControlMasterMode, Session, SshOptions, terminal::control_master_args};
use std::path::Path;

fn session(mode: ControlMasterMode) -> Session {
    Session { name: "cm".into(), host: "example.test".into(), ssh: SshOptions { control_master: mode, control_path: "/tmp/inspirum-%C.sock".into(), control_persist_seconds: Some(30), ..SshOptions::default() }, ..Session::default() }
}

#[test]
fn auto_emits_structured_control_options() {
    let args = session(ControlMasterMode::Auto).ssh_args().unwrap();
    assert!(args.windows(2).any(|w| w == ["-o", "ControlMaster=auto"]));
    assert!(args.windows(2).any(|w| w == ["-o", "ControlPath=/tmp/inspirum-%C.sock"]));
    assert!(args.windows(2).any(|w| w == ["-o", "ControlPersist=30"]));
}

#[test]
fn disabled_explicitly_prevents_multiplexing() {
    let args = session(ControlMasterMode::Disabled).ssh_args().unwrap();
    assert!(args.windows(2).any(|w| w == ["-o", "ControlMaster=no"]));
}

#[test]
fn inherit_does_not_override_openssh_config() {
    let mut s = session(ControlMasterMode::Inherit);
    s.ssh.control_path.clear();
    s.ssh.control_persist_seconds = None;
    let args = s.ssh_args().unwrap();
    assert!(!args.iter().any(|a| a.starts_with("ControlMaster=") || a.starts_with("ControlPath=")));
}

#[test]
fn auto_requires_control_path_and_lifecycle_uses_fixed_argv() {
    let mut s = session(ControlMasterMode::Auto);
    s.ssh.control_path.clear();
    assert!(s.ssh_args().is_err());
    let s = session(ControlMasterMode::Auto);
    let args = control_master_args(&s, Some(Path::new("fixture.conf")), "exit").unwrap();
    assert!(args.windows(2).any(|w| w == ["-O", "exit"]));
    assert!(args.windows(2).any(|w| w == ["-S", "/tmp/inspirum-%C.sock"]));
    assert!(control_master_args(&s, None, "shell-fragment").is_err());
}
