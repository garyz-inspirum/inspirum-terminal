//! Editing or deleting a saved forward must never orphan its live listener.

use super::*;

const ALPHA: &str = "127.0.0.1:18081:web:80";
const BRAVO: &str = "127.0.0.1:18082:web:81";
const LOCAL: tunnels::ForwardKind = tunnels::ForwardKind::Local;

/// Boot the app with one saved profile holding Alpha and Bravo, then start
/// Alpha as a real forwarding-only `ssh -N`. The ProxyCommand never answers,
/// so OpenSSH stays alive waiting for a banner without binding anything.
fn app_with_alpha_running(dir: &std::path::Path) -> (App, String, tunnels::Status) {
    let config = dir.join("ssh_config");
    std::fs::write(&config, "Host *\n  ProxyCommand sleep 30\n").unwrap();
    let path = dir.join("profiles.json");
    let mut session = Session {
        name: "srv".into(),
        host: "srv.invalid".into(),
        ..Session::default()
    };
    session.ssh.local_forwards = vec![ALPHA.into(), BRAVO.into()];
    save_sessions(&path, &[session]).unwrap();
    let mut app = App::boot(path, Some(config.clone()));
    let session = app.profiles[0].clone();
    let key = session_profile_key(&session);
    let alpha = app
        .tunnels
        .start_forward(&session, LOCAL, ALPHA, Some(&config))
        .unwrap();
    assert!(alpha.is_running(), "Alpha must run: {alpha:?}");
    app.refresh_tunnel_status();
    (app, key, alpha)
}

fn saved_forwards(app: &App) -> Vec<String> {
    load_sessions(&app.profiles_path).unwrap()[0]
        .ssh
        .local_forwards
        .clone()
}

fn assert_alpha_untouched(app: &mut App, key: &str, alpha: &tunnels::Status) {
    assert_eq!(
        saved_forwards(app),
        vec![ALPHA.to_owned(), BRAVO.to_owned()]
    );
    assert_eq!(
        app.profiles[0].ssh.local_forwards,
        vec![ALPHA.to_owned(), BRAVO.to_owned()]
    );
    assert_eq!(&app.tunnels.forward_status(key, LOCAL, ALPHA), alpha);
    assert_eq!(
        app.tunnels.forward_status(key, LOCAL, BRAVO),
        tunnels::Status::Stopped
    );
    // The live listener keeps its row, so Stop/Stop all can still reach it.
    assert!(
        app.tunnel_quick
            .iter()
            .any(|quick| quick.row.spec == ALPHA && quick.running)
    );
}

#[test]
fn editing_a_running_forward_is_refused_and_keeps_its_listener() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, alpha) = app_with_alpha_running(dir.path());
    let _ = app.update(Message::TunnelEdit(key.clone(), LOCAL, ALPHA.into()));
    app.tunnel_edit.as_mut().unwrap().draft.listen_port = "18083".into();
    let _ = app.update(Message::TunnelSaveEdit);
    assert!(
        app.tunnel_notice.contains("Stop this forward"),
        "{}",
        app.tunnel_notice
    );
    assert!(app.tunnel_edit.is_some(), "the rejected draft stays open");
    assert_alpha_untouched(&mut app, &key, &alpha);

    // A label-only rename keeps the running key and its listener.
    app.tunnel_edit.as_mut().unwrap().draft.listen_port = "18081".into();
    app.tunnel_edit.as_mut().unwrap().draft.label = "Alpha".into();
    let _ = app.update(Message::TunnelSaveEdit);
    assert_eq!(app.tunnel_notice, "Tunnel saved.");
    assert_eq!(app.tunnels.forward_status(&key, LOCAL, ALPHA), alpha);
    assert_eq!(tunnels::label_for(&app.profiles[0], LOCAL, ALPHA), "Alpha");

    // Unrelated idle Bravo may still be edited; Alpha keeps running.
    let edited = "127.0.0.1:18084:web:81";
    let _ = app.update(Message::TunnelEdit(key.clone(), LOCAL, BRAVO.into()));
    app.tunnel_edit.as_mut().unwrap().draft.listen_port = "18084".into();
    let _ = app.update(Message::TunnelSaveEdit);
    assert_eq!(app.tunnel_notice, "Tunnel saved.");
    let forwards = saved_forwards(&app);
    assert!(forwards.contains(&ALPHA.to_owned()) && forwards.contains(&edited.to_owned()));
    assert!(!forwards.contains(&BRAVO.to_owned()));
    assert_eq!(app.tunnels.forward_status(&key, LOCAL, ALPHA), alpha);
    app.tunnels.stop_all();
}

#[test]
fn deleting_a_running_forward_is_refused_and_keeps_its_listener() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, alpha) = app_with_alpha_running(dir.path());
    let _ = app.update(Message::TunnelEdit(key.clone(), LOCAL, ALPHA.into()));
    let _ = app.update(Message::TunnelDeleteEdit);
    assert!(
        app.tunnel_notice.contains("Stop this forward"),
        "{}",
        app.tunnel_notice
    );
    assert_alpha_untouched(&mut app, &key, &alpha);

    // Unrelated idle Bravo may still be deleted; Alpha keeps running.
    let _ = app.update(Message::TunnelEdit(key.clone(), LOCAL, BRAVO.into()));
    let _ = app.update(Message::TunnelDeleteEdit);
    assert_eq!(app.tunnel_notice, "Tunnel deleted.");
    assert_eq!(saved_forwards(&app), vec![ALPHA.to_owned()]);
    assert_eq!(app.tunnels.forward_status(&key, LOCAL, ALPHA), alpha);

    // Once stopped, Alpha itself may be deleted.
    app.tunnels.stop_forward(&key, LOCAL, ALPHA).unwrap();
    let _ = app.update(Message::TunnelEdit(key.clone(), LOCAL, ALPHA.into()));
    let _ = app.update(Message::TunnelDeleteEdit);
    assert_eq!(app.tunnel_notice, "Tunnel deleted.");
    assert!(saved_forwards(&app).is_empty());
}
