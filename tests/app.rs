use std::process::Command;

#[test]
fn launcher_supports_iced_and_rejects_the_removed_legacy_frontend() {
    let binary = env!("CARGO_BIN_EXE_inspirum-terminal");
    let iced = Command::new(binary)
        .args(["--ui", "iced", "--help"])
        .output()
        .unwrap();
    assert!(iced.status.success());
    assert!(String::from_utf8_lossy(&iced.stdout).contains("Iced is the only frontend"));
    let legacy = Command::new(binary)
        .args(["--ui", "legacy"])
        .output()
        .unwrap();
    assert!(!legacy.status.success());
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("legacy frontend has been removed"));
}
