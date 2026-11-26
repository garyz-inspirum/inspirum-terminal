//! Tunnel Settings `Session / profile` picker: every saved session as a
//! searchable folder path, owner changes for drafts, and moves on Save.

use super::*;

const SPEC: &str = "127.0.0.1:18091:web:80";
const LOCAL: tunnels::ForwardKind = tunnels::ForwardKind::Local;

fn session(folder: &str, name: &str) -> Session {
    Session {
        name: name.into(),
        folder: folder.into(),
        host: format!("{name}.invalid"),
        ..Session::default()
    }
}

/// Boot with `profiles` saved, then open Tunnel Settings.
fn boot_with(dir: &std::path::Path, profiles: &[Session]) -> App {
    let path = dir.join("profiles.json");
    save_sessions(&path, profiles).unwrap();
    let mut app = App::boot(path, None);
    let _ = app.update(Message::Tunnels);
    app
}

fn option_for(app: &App, key: &str) -> TunnelSessionOption {
    app.tunnel_session_picker
        .options()
        .iter()
        .find(|option| option.key == key)
        .cloned()
        .expect("every saved session is a picker option")
}

fn key_of(app: &App, folder: &str, name: &str) -> String {
    let profile = app
        .profiles
        .iter()
        .find(|profile| profile.folder == folder && profile.name == name)
        .unwrap();
    session_profile_key(profile)
}

fn saved_bytes(app: &App) -> Vec<u8> {
    std::fs::read(&app.profiles_path).unwrap()
}

#[test]
fn display_path_joins_folder_segments_and_name() {
    assert_eq!(
        tunnel_session_display_path(&session("Prod/DB", "pg-primary")),
        "Prod / DB / pg-primary"
    );
    assert_eq!(tunnel_session_display_path(&session("", "solo")), "solo");
    // Stray slashes and spaces do not produce empty segments.
    assert_eq!(
        tunnel_session_display_path(&session(" Prod / /DB/ ", "x")),
        "Prod / DB / x"
    );
}

#[test]
fn options_sort_case_insensitively_and_keep_duplicate_names_distinct() {
    let profiles = [
        session("prod/db", "web"),
        session("", "alpha"),
        session("Dev", "web"),
        session("Prod/DB", "Web"),
        session("", "Beta"),
    ];
    let labels: Vec<String> = tunnel_session_options(&profiles)
        .into_iter()
        .map(|option| option.label)
        .collect();
    assert_eq!(
        labels,
        [
            "alpha",
            "Beta",
            "Dev / web",
            "Prod / DB / Web",
            "prod / db / web (2)",
        ]
    );
    // Every option carries its own profile key.
    let keys: HashSet<String> = tunnel_session_options(&profiles)
        .into_iter()
        .map(|option| option.key)
        .collect();
    assert_eq!(keys.len(), profiles.len());
}

#[test]
fn picker_options_are_rebuilt_when_profiles_change() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot_with(dir.path(), &[session("Ops", "one")]);
    assert_eq!(app.tunnel_session_picker.options().len(), 1);
    app.profiles.push(session("Ops/Edge", "two"));
    app.refresh_profile_matches();
    let labels: Vec<&str> = app
        .tunnel_session_picker
        .options()
        .iter()
        .map(|option| option.label.as_str())
        .collect();
    assert_eq!(labels, ["Ops / Edge / two", "Ops / one"]);
    app.profiles.pop();
    let _ = app.update(Message::Tunnels);
    assert_eq!(app.tunnel_session_picker.options().len(), 1);
}

#[test]
fn new_draft_is_saved_under_the_chosen_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot_with(
        dir.path(),
        &[session("Prod", "first"), session("Prod/DB", "second")],
    );
    let first = key_of(&app, "Prod", "first");
    let second = key_of(&app, "Prod/DB", "second");
    let _ = app.update(Message::TunnelAdd(first.clone()));
    let _ = app.update(Message::TunnelDraftSession(option_for(&app, &second)));
    let edit = app.tunnel_edit.as_ref().unwrap();
    assert_eq!(edit.profile_key, second, "a draft changes owner at once");
    assert_eq!(edit.target_key, second);
    let _ = app.update(Message::TunnelDraftLabel("db".into()));
    let _ = app.update(Message::TunnelDraftListenPort("18091".into()));
    let _ = app.update(Message::TunnelDraftDestinationHost("web".into()));
    let _ = app.update(Message::TunnelDraftDestinationPort("80".into()));
    let _ = app.update(Message::TunnelSaveEdit);
    assert_eq!(app.tunnel_notice, "Tunnel saved.");
    let saved = load_sessions(&app.profiles_path).unwrap();
    let by_name = |name: &str| saved.iter().find(|s| s.name == name).unwrap().clone();
    assert!(tunnels::rows_for(&by_name("first")).is_empty());
    let rows = tunnels::rows_for(&by_name("second"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "db");
}

/// Two idle sessions; `from` owns SPEC labelled `api`.
fn app_with_saved_forward(dir: &std::path::Path) -> (App, String, String) {
    let mut from = session("Prod", "from");
    tunnels::add_forward(&mut from, LOCAL, SPEC.into(), "api").unwrap();
    let app = boot_with(dir, &[from, session("Prod/DB", "to")]);
    let from = key_of(&app, "Prod", "from");
    let to = key_of(&app, "Prod/DB", "to");
    (app, from, to)
}

#[test]
fn saved_forward_moves_to_the_chosen_session_with_its_label() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, from, to) = app_with_saved_forward(dir.path());
    let _ = app.update(Message::TunnelEdit(from.clone(), LOCAL, SPEC.into()));
    let _ = app.update(Message::TunnelDraftSession(option_for(&app, &to)));
    let edit = app.tunnel_edit.as_ref().unwrap();
    assert_eq!(edit.profile_key, from, "the owner stays until Save");
    assert_eq!(edit.target_key, to);
    let _ = app.update(Message::TunnelSaveEdit);
    assert_eq!(app.tunnel_notice, "Tunnel moved to to.");
    assert!(app.tunnel_edit.is_none());
    for profiles in [
        load_sessions(&app.profiles_path).unwrap(),
        app.profiles.clone(),
    ] {
        let find = |name: &str| profiles.iter().find(|s| s.name == name).unwrap();
        assert!(tunnels::rows_for(find("from")).is_empty());
        assert!(find("from").ssh.tunnel_labels.is_empty());
        let rows = tunnels::rows_for(find("to"));
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].kind, rows[0].spec.as_str()), (LOCAL, SPEC));
        assert_eq!(rows[0].label, "api");
    }
}

#[test]
fn moving_into_a_session_with_the_same_forward_is_refused_byte_identically() {
    let dir = tempfile::tempdir().unwrap();
    let mut from = session("Prod", "from");
    tunnels::add_forward(&mut from, LOCAL, SPEC.into(), "api").unwrap();
    let mut to = session("Prod/DB", "to");
    tunnels::add_forward(&mut to, LOCAL, SPEC.into(), "other").unwrap();
    let mut app = boot_with(dir.path(), &[from, to]);
    let from = key_of(&app, "Prod", "from");
    let to = key_of(&app, "Prod/DB", "to");
    let before_file = saved_bytes(&app);
    let before = app.profiles.clone();
    let _ = app.update(Message::TunnelEdit(from, LOCAL, SPEC.into()));
    let _ = app.update(Message::TunnelDraftSession(option_for(&app, &to)));
    let _ = app.update(Message::TunnelSaveEdit);
    assert_eq!(app.tunnel_notice, TUNNEL_MOVE_DUPLICATE);
    assert_eq!(saved_bytes(&app), before_file);
    assert_eq!(app.profiles, before);
    assert!(app.tunnel_edit.is_some(), "the editor stays open");
}

#[test]
fn choosing_a_deleted_session_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, from, to) = app_with_saved_forward(dir.path());
    let stale = option_for(&app, &to);
    let _ = app.update(Message::TunnelEdit(from.clone(), LOCAL, SPEC.into()));
    app.profiles
        .retain(|profile| session_profile_key(profile) != to);
    let before_file = saved_bytes(&app);
    let _ = app.update(Message::TunnelDraftSession(stale));
    assert_eq!(app.tunnel_notice, "Profile no longer exists.");
    assert_eq!(app.tunnel_edit.as_ref().unwrap().target_key, from);
    assert_eq!(saved_bytes(&app), before_file);
}

#[test]
fn the_per_session_button_list_is_gone() {
    let source = include_str!("../iced_app.rs");
    assert!(!source.contains(concat!("Add to ", "session")));
    assert!(TUNNEL_CREATE_HINT.contains("press +"));
    assert!(!TUNNEL_CREATE_HINT.contains("below the list"));
}
