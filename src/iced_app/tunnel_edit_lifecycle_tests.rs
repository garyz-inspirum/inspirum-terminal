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

/// Open the real profile editor on the saved profile, apply `edit` to the
/// form, then press Save.
fn submit_profile_edit(app: &mut App, edit: impl FnOnce(&mut ConnectionForm)) {
    let _ = app.update(Message::EditProfile(0));
    assert!(app.editing_profile.is_some(), "the profile editor is open");
    edit(&mut app.form);
    let _ = app.update(Message::Submit);
}

fn assert_profile_edit_refused(app: &mut App, key: &str, alpha: &tunnels::Status) {
    let error = app.form.error.clone().unwrap_or_default();
    assert!(error.contains("Stop"), "{error:?}");
    assert!(
        matches!(app.dialog, Some(Dialog::Connection)),
        "draft stays open"
    );
    assert_eq!(app.editing_profile.as_deref(), Some(key));
    assert_eq!(load_sessions(&app.profiles_path).unwrap()[0].name, "srv");
    assert_eq!(
        load_sessions(&app.profiles_path).unwrap()[0].host,
        "srv.invalid"
    );
    assert_eq!(session_profile_key(&app.profiles[0]), key);
    app.refresh_tunnel_status();
    assert_alpha_untouched(app, key, alpha);
    let _ = app.update(Message::CloseDialog);
}

#[test]
fn profile_editor_refuses_saves_that_would_orphan_a_running_forward() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, alpha) = app_with_alpha_running(dir.path());

    // Renaming changes the profile key the live listener is filed under.
    submit_profile_edit(&mut app, |form| form.name = "renamed".into());
    assert_profile_edit_refused(&mut app, &key, &alpha);
    // Moving folders changes the key just the same.
    submit_profile_edit(&mut app, |form| form.folder = "prod".into());
    assert_profile_edit_refused(&mut app, &key, &alpha);
    // Removing the active spec leaves its listener without a row.
    submit_profile_edit(&mut app, |form| form.local_forwards = BRAVO.into());
    assert_profile_edit_refused(&mut app, &key, &alpha);
    // Changing the active spec misattributes the old listener.
    submit_profile_edit(&mut app, |form| {
        form.local_forwards = format!("127.0.0.1:18083:web:80; {BRAVO}");
    });
    assert_profile_edit_refused(&mut app, &key, &alpha);
    // A new SSH target would report the old server's listener as its own.
    submit_profile_edit(&mut app, |form| form.host = "other.invalid".into());
    assert_profile_edit_refused(&mut app, &key, &alpha);
    submit_profile_edit(&mut app, |form| form.user = "root".into());
    assert_profile_edit_refused(&mut app, &key, &alpha);
    submit_profile_edit(&mut app, |form| form.port = "2222".into());
    assert_profile_edit_refused(&mut app, &key, &alpha);

    // The quick switch can still stop the original listener.
    let _ = app.update(Message::TunnelForwardToggle(
        key.clone(),
        LOCAL,
        ALPHA.into(),
    ));
    assert_eq!(
        app.tunnels.forward_status(&key, LOCAL, ALPHA),
        tunnels::Status::Stopped,
        "{}",
        app.tunnel_notice
    );
    app.tunnels.stop_all();
}

#[test]
fn profile_editor_saves_idle_sibling_and_display_edits_beside_a_running_forward() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, alpha) = app_with_alpha_running(dir.path());
    let edited = "127.0.0.1:18084:web:81";
    submit_profile_edit(&mut app, |form| {
        form.local_forwards = format!("{ALPHA}; {edited}");
        form.favorite = true;
        form.tags = vec!["web".into()];
    });
    assert_eq!(app.form.error, None);
    assert!(app.dialog.is_none(), "{:?}", app.form.error);
    let saved = &load_sessions(&app.profiles_path).unwrap()[0];
    assert_eq!(
        saved.ssh.local_forwards,
        vec![ALPHA.to_owned(), edited.to_owned()]
    );
    assert!(saved.favorite);
    assert_eq!(saved.tags, vec!["web".to_owned()]);
    app.refresh_tunnel_status();
    assert_eq!(app.tunnels.forward_status(&key, LOCAL, ALPHA), alpha);
    assert!(
        app.tunnel_quick
            .iter()
            .any(|quick| quick.row.spec == ALPHA && quick.running)
    );
    app.tunnels.stop_all();
}

#[test]
fn profile_editor_refuses_forward_edits_while_the_whole_profile_runs() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("ssh_config");
    let (mut app, key, _) = app_with_alpha_running(dir.path());
    app.tunnels.stop_forward(&key, LOCAL, ALPHA).unwrap();
    let session = app.profiles[0].clone();
    let whole = app.tunnels.start(&session, Some(&config)).unwrap();
    assert!(whole.is_running(), "{whole:?}");

    // The aggregate ssh owns every saved forward, even idle-looking Bravo.
    submit_profile_edit(&mut app, |form| {
        form.local_forwards = format!("{ALPHA}; 127.0.0.1:18084:web:81");
    });
    let error = app.form.error.clone().unwrap_or_default();
    assert!(error.contains("Stop"), "{error:?}");
    assert_eq!(
        saved_forwards(&app),
        vec![ALPHA.to_owned(), BRAVO.to_owned()]
    );
    assert_eq!(app.tunnels.status(&key), whole);
    let _ = app.update(Message::CloseDialog);

    // Display-only edits stay allowed.
    submit_profile_edit(&mut app, |form| form.favorite = true);
    assert_eq!(app.form.error, None);
    assert!(load_sessions(&app.profiles_path).unwrap()[0].favorite);
    assert_eq!(app.tunnels.status(&key), whole);
    app.tunnels.stop_all();
}

/// Add an idle sibling profile after the running `srv` profile.
fn add_idle_profile(app: &mut App) -> String {
    let idle = Session {
        name: "zz-idle".into(),
        host: "idle.invalid".into(),
        ..Session::default()
    };
    let key = session_profile_key(&idle);
    let mut profiles = app.profiles.clone();
    profiles.push(idle);
    save_sessions(&app.profiles_path, &profiles).unwrap();
    app.profiles = profiles;
    app.refresh_tunnel_status();
    key
}

/// Open the real profile editor on `srv`, apply `edit`, then press
/// "Save only" (the async tools writer, not `Message::Submit`).
fn save_draft_profile_edit(app: &mut App, edit: impl FnOnce(&mut ConnectionForm)) {
    let _ = app.update(Message::EditProfile(0));
    assert!(app.editing_profile.is_some(), "the profile editor is open");
    edit(&mut app.form);
    let _ = app.update(Message::Tool(tools::Action::SaveDraft));
}

fn assert_save_draft_refused(app: &mut App, key: &str, alpha: &tunnels::Status) {
    let error = app.form.error.clone().unwrap_or_default();
    assert!(error.contains("Stop"), "{error:?}");
    assert!(!app.tools.busy, "no async profile write was started");
    assert!(matches!(app.dialog, Some(Dialog::Connection)));
    assert_eq!(load_sessions(&app.profiles_path).unwrap()[0].name, "srv");
    assert_eq!(
        load_sessions(&app.profiles_path).unwrap()[0].host,
        "srv.invalid"
    );
    assert_eq!(session_profile_key(&app.profiles[0]), key);
    app.refresh_tunnel_status();
    assert_alpha_untouched(app, key, alpha);
    let _ = app.update(Message::CloseDialog);
}

#[test]
fn save_only_refuses_drafts_that_would_orphan_a_running_forward() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, alpha) = app_with_alpha_running(dir.path());

    save_draft_profile_edit(&mut app, |form| form.name = "renamed".into());
    assert_save_draft_refused(&mut app, &key, &alpha);
    save_draft_profile_edit(&mut app, |form| form.folder = "prod".into());
    assert_save_draft_refused(&mut app, &key, &alpha);
    save_draft_profile_edit(&mut app, |form| form.local_forwards = BRAVO.into());
    assert_save_draft_refused(&mut app, &key, &alpha);
    save_draft_profile_edit(&mut app, |form| form.host = "other.invalid".into());
    assert_save_draft_refused(&mut app, &key, &alpha);

    // An idle sibling edit beside the live forward still reaches the store
    // writer (the job is queued, not refused).
    save_draft_profile_edit(&mut app, |form| {
        form.local_forwards = format!("{ALPHA}; 127.0.0.1:18084:web:81");
    });
    assert_eq!(app.form.error, None);
    assert!(app.tools.busy, "the idle sibling save was queued");
    assert_eq!(app.tunnels.forward_status(&key, LOCAL, ALPHA), alpha);
    app.tunnels.stop_all();
}

#[test]
fn deleting_a_profile_with_a_running_forward_is_refused_but_idle_profiles_delete() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, alpha) = app_with_alpha_running(dir.path());
    let idle = add_idle_profile(&mut app);

    let _ = app.update(Message::Tool(tools::Action::Delete(key.clone())));
    let _ = app.update(Message::Tool(tools::Action::Confirm));
    assert!(app.status.contains("Stop"), "{}", app.status);
    assert!(!app.tools.busy, "no async profile write was started");
    assert_eq!(load_sessions(&app.profiles_path).unwrap().len(), 2);
    assert_eq!(app.profiles.len(), 2);
    assert_alpha_untouched(&mut app, &key, &alpha);

    // The idle sibling may still be deleted while Alpha runs.
    let _ = app.update(Message::Tool(tools::Action::Delete(idle)));
    let _ = app.update(Message::Tool(tools::Action::Confirm));
    assert!(!app.status.contains("Stop"), "{}", app.status);
    assert!(app.tools.busy, "the idle delete was queued");
    assert_eq!(app.tunnels.forward_status(&key, LOCAL, ALPHA), alpha);
    app.tunnels.stop_all();
}

#[test]
fn deleting_a_profile_with_nothing_running_is_still_queued() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, _) = app_with_alpha_running(dir.path());
    app.tunnels.stop_forward(&key, LOCAL, ALPHA).unwrap();
    let _ = app.update(Message::Tool(tools::Action::Delete(key)));
    let _ = app.update(Message::Tool(tools::Action::Confirm));
    assert!(!app.status.contains("Stop"), "{}", app.status);
    assert!(app.tools.busy, "the delete was queued");
}

#[test]
fn deleting_a_profile_whose_aggregate_tunnel_runs_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("ssh_config");
    let (mut app, key, _) = app_with_alpha_running(dir.path());
    app.tunnels.stop_forward(&key, LOCAL, ALPHA).unwrap();
    let session = app.profiles[0].clone();
    let whole = app.tunnels.start(&session, Some(&config)).unwrap();
    assert!(whole.is_running(), "{whole:?}");
    let _ = app.update(Message::Tool(tools::Action::Delete(key.clone())));
    let _ = app.update(Message::Tool(tools::Action::Confirm));
    assert!(app.status.contains("Stop"), "{}", app.status);
    assert!(!app.tools.busy);
    assert_eq!(load_sessions(&app.profiles_path).unwrap().len(), 1);
    assert_eq!(app.tunnels.status(&key), whole);
    app.tunnels.stop_all();
}

#[test]
fn replacement_snapshot_refuses_libraries_that_drop_or_change_a_live_owner() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, alpha) = app_with_alpha_running(dir.path());
    let idle_key = add_idle_profile(&mut app);
    let owners = app.live_tunnel_owners();
    let srv = app.profiles[0].clone();
    let idle = app.profiles[1].clone();

    // Unchanged live owner with idle siblings dropped or added: allowed.
    assert_eq!(owners.conflict(&app.profiles), None);
    assert_eq!(owners.conflict(std::slice::from_ref(&srv)), None);
    // Library without the live owner, or with it pointed elsewhere: refused.
    assert!(owners.conflict(std::slice::from_ref(&idle)).is_some());
    let mut moved = srv.clone();
    moved.host = "other.invalid".into();
    assert!(owners.conflict(&[moved, idle]).is_some());
    let mut dropped = srv;
    dropped.ssh.local_forwards = vec![BRAVO.into()];
    assert!(owners.conflict(&[dropped]).is_some());
    assert_ne!(idle_key, key);
    assert_eq!(app.tunnels.forward_status(&key, LOCAL, ALPHA), alpha);
    app.tunnels.stop_all();
}

#[test]
fn ssh_policy_save_from_a_stale_target_cannot_drop_a_running_forward() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, alpha) = app_with_alpha_running(dir.path());
    // The tools target is a copy taken earlier; here it predates Alpha.
    let mut stale = app.profiles[0].clone();
    stale.ssh.local_forwards = vec![BRAVO.into()];
    app.tools.target = Some(stale);
    app.tools.control_persist = "60".into();
    let _ = app.update(Message::Tool(tools::Action::SaveSshPolicy));
    assert!(app.status.contains("Stop"), "{}", app.status);
    assert!(!app.tools.busy);
    assert_alpha_untouched(&mut app, &key, &alpha);
    app.tunnels.stop_all();
}

/// Queue "Save only" renaming the idle `srv` profile, returning the job's
/// generation and the library it will commit (written to disk here, as the
/// worker would, so `JobDone` can be delivered by hand).
fn queue_rename_while_idle(app: &mut App, key: &str) -> (u64, Vec<Session>, Session) {
    save_draft_profile_edit(app, |form| form.name = "renamed".into());
    assert_eq!(app.form.error, None);
    assert!(app.tools.busy, "the rename was queued");
    let generation = app.tools.generation;
    assert_eq!(app.tools.profile_write, Some(generation));
    let draft = app.form.session().unwrap();
    let profiles = save_session_edit(&app.profiles, Some(key), draft.clone()).unwrap();
    (generation, profiles, draft)
}

fn assert_nothing_started(app: &mut App, key: &str) {
    assert_eq!(app.tunnel_notice, PROFILE_WRITE_PENDING);
    assert_eq!(
        app.tunnels.forward_status(key, LOCAL, ALPHA),
        tunnels::Status::Stopped
    );
    assert!(!app.tunnels.status(key).is_running());
    assert_eq!(app.tunnels.running_count(), 0);
}

#[test]
fn tunnel_starts_wait_for_an_in_flight_profile_save_then_retry_works() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, _) = app_with_alpha_running(dir.path());
    app.tunnels.stop_forward(&key, LOCAL, ALPHA).unwrap();
    app.refresh_tunnel_status();
    let (generation, profiles, draft) = queue_rename_while_idle(&mut app, &key);

    // While the rename is in flight, neither start entry may file a live
    // process under the key the job is about to drop.
    let _ = app.update(Message::TunnelForwardToggle(
        key.clone(),
        LOCAL,
        ALPHA.into(),
    ));
    assert_nothing_started(&mut app, &key);
    let _ = app.update(Message::TunnelStart(key.clone()));
    assert_nothing_started(&mut app, &key);

    // A late result from an older job does not release the gate.
    let _ = app.update(Message::Tool(tools::Action::JobDone(
        generation.wrapping_sub(1),
        Ok(tools::Data::Profiles(Vec::new())),
    )));
    assert_eq!(app.tools.profile_write, Some(generation));
    assert_eq!(app.profiles[0].name, "srv");
    let _ = app.update(Message::TunnelForwardToggle(
        key.clone(),
        LOCAL,
        ALPHA.into(),
    ));
    assert_nothing_started(&mut app, &key);

    // Synchronous profile writers wait too, so the job's library cannot
    // overwrite (or be overwritten by) a concurrent save.
    let _ = app.update(Message::CloseDialog);
    submit_profile_edit(&mut app, |form| form.favorite = true);
    let error = app.form.error.clone().unwrap_or_default();
    assert_eq!(error, PROFILE_WRITE_PENDING);
    let _ = app.update(Message::CloseDialog);
    let _ = app.update(Message::TunnelEdit(key.clone(), LOCAL, BRAVO.into()));
    app.tunnel_edit.as_mut().unwrap().draft.label = "Bravo".into();
    let _ = app.update(Message::TunnelSaveEdit);
    assert_eq!(app.tunnel_notice, PROFILE_WRITE_PENDING);
    let _ = app.update(Message::TunnelDeleteEdit);
    assert_eq!(app.tunnel_notice, PROFILE_WRITE_PENDING);
    app.tunnel_edit = None;
    let stored = load_sessions(&app.profiles_path).unwrap();
    assert_eq!(stored[0].name, "srv");
    assert!(!stored[0].favorite);
    assert_ne!(tunnels::label_for(&stored[0], LOCAL, BRAVO), "Bravo");
    assert_ne!(tunnels::label_for(&app.profiles[0], LOCAL, BRAVO), "Bravo");

    // The job commits; the gate opens and the old key is gone.
    save_sessions(&app.profiles_path, &profiles).unwrap();
    let _ = app.update(Message::Tool(tools::Action::JobDone(
        generation,
        Ok(tools::Data::DraftSaved(profiles, Box::new(draft))),
    )));
    assert_eq!(app.tools.profile_write, None);
    assert!(!app.tools.busy);
    assert_eq!(app.profiles[0].name, "renamed");
    let renamed = session_profile_key(&app.profiles[0]);
    assert_ne!(renamed, key);
    let _ = app.update(Message::TunnelForwardToggle(
        key.clone(),
        LOCAL,
        ALPHA.into(),
    ));
    assert_eq!(app.tunnels.running_count(), 0, "{}", app.tunnel_notice);

    // Retrying on the committed profile starts Alpha under its new key.
    let _ = app.update(Message::TunnelForwardToggle(
        renamed.clone(),
        LOCAL,
        ALPHA.into(),
    ));
    assert!(
        app.tunnels
            .forward_status(&renamed, LOCAL, ALPHA)
            .is_running(),
        "{}",
        app.tunnel_notice
    );
    assert_eq!(
        load_sessions(&app.profiles_path).unwrap()[0].name,
        "renamed"
    );
    app.tunnels.stop_all();
}

#[test]
fn a_failed_profile_save_releases_tunnel_starts() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, _) = app_with_alpha_running(dir.path());
    app.tunnels.stop_forward(&key, LOCAL, ALPHA).unwrap();
    app.refresh_tunnel_status();
    let (generation, _, _) = queue_rename_while_idle(&mut app, &key);
    let _ = app.update(Message::TunnelStart(key.clone()));
    assert_nothing_started(&mut app, &key);

    let _ = app.update(Message::Tool(tools::Action::JobDone(
        generation,
        Err("disk full".into()),
    )));
    assert_eq!(app.tools.profile_write, None);
    assert_eq!(app.status, "disk full");
    assert_eq!(session_profile_key(&app.profiles[0]), key);
    let _ = app.update(Message::TunnelStart(key.clone()));
    assert!(
        app.tunnels.status(&key).is_running(),
        "{}",
        app.tunnel_notice
    );
    app.tunnels.stop_all();
}

#[test]
fn unrelated_tool_jobs_do_not_gate_tunnel_starts() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, key, _) = app_with_alpha_running(dir.path());
    app.tunnels.stop_forward(&key, LOCAL, ALPHA).unwrap();
    let _ = app.update(Message::Tool(tools::Action::ExportProfiles));
    assert!(app.tools.busy, "the export was queued");
    assert_eq!(app.tools.profile_write, None);
    let _ = app.update(Message::TunnelForwardToggle(
        key.clone(),
        LOCAL,
        ALPHA.into(),
    ));
    assert!(
        app.tunnels.forward_status(&key, LOCAL, ALPHA).is_running(),
        "{}",
        app.tunnel_notice
    );
    app.tunnels.stop_all();
}
