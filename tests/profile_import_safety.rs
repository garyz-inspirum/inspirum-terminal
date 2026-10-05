use inspirum_terminal::{
    Session, SessionImportMode, import_sessions, load_sessions, save_sessions,
};
use std::fs;

fn profile(name: &str) -> Session {
    Session {
        name: name.into(),
        host: "example.test".into(),
        ..Session::default()
    }
}

#[test]
fn missing_import_never_clears_an_existing_library() {
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("active.json");
    let missing = dir.path().join("missing.json");
    let existing = vec![profile("Keep")];
    save_sessions(&active, &existing).unwrap();
    let before = fs::read(&active).unwrap();
    for mode in [SessionImportMode::Merge, SessionImportMode::Replace] {
        let result = import_sessions(&missing, &existing, mode);
        assert!(result.is_err(), "missing import must fail in {mode:?} mode");
        assert_eq!(fs::read(&active).unwrap(), before);
        assert_eq!(load_sessions(&active).unwrap(), existing);
    }
    assert!(!missing.exists());
    assert!(load_sessions(&missing).unwrap().is_empty());
}

#[test]
fn explicit_empty_json_is_distinct_from_a_missing_or_zero_byte_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("import.json");
    let existing = vec![profile("Keep")];
    fs::write(&path, b"[]").unwrap();
    assert!(
        import_sessions(&path, &existing, SessionImportMode::Replace)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        import_sessions(&path, &existing, SessionImportMode::Merge).unwrap(),
        existing
    );
    fs::write(&path, b"").unwrap();
    assert!(import_sessions(&path, &existing, SessionImportMode::Replace).is_err());
}

#[test]
fn malformed_and_oversized_imports_leave_active_store_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("active.json");
    let source = dir.path().join("source.json");
    let existing = vec![profile("Keep")];
    save_sessions(&active, &existing).unwrap();
    let before = fs::read(&active).unwrap();
    for bytes in [b"{bad".to_vec(), vec![b' '; 1_048_577]] {
        fs::write(&source, bytes).unwrap();
        for mode in [SessionImportMode::Merge, SessionImportMode::Replace] {
            assert!(import_sessions(&source, &existing, mode).is_err());
            assert_eq!(fs::read(&active).unwrap(), before);
        }
    }
}

#[test]
fn directory_is_not_an_import_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(import_sessions(dir.path(), &[], SessionImportMode::Replace).is_err());
}

#[test]
fn individually_valid_libraries_cannot_merge_past_the_serialized_limit() {
    let dir = tempfile::tempdir().unwrap();
    let make_library = |prefix: &str| -> Vec<Session> {
        (0..120)
            .map(|index| {
                let mut session = profile(&format!("{prefix}-{index}"));
                session.ssh.remote_command = "x".repeat(5000);
                session
            })
            .collect()
    };
    let existing = make_library("existing");
    let incoming = make_library("incoming");
    let active = dir.path().join("active.json");
    let source = dir.path().join("source.json");
    save_sessions(&active, &existing).unwrap();
    save_sessions(&source, &incoming).unwrap();
    let before = fs::read(&active).unwrap();
    let error = import_sessions(&source, &existing, SessionImportMode::Merge).unwrap_err();
    assert!(format!("{error:#}").contains("1 MiB"));
    assert_eq!(fs::read(active).unwrap(), before);
    assert_eq!(
        import_sessions(&source, &existing, SessionImportMode::Replace).unwrap(),
        incoming
    );
}

#[test]
fn merge_validates_existing_profiles_as_well_as_imported_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.json");
    save_sessions(&source, &[profile("Incoming")]).unwrap();
    let mut invalid = profile("Existing");
    invalid.port = Some(0);
    assert!(import_sessions(&source, &[invalid], SessionImportMode::Merge).is_err());
}
