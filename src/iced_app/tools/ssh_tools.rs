use super::*;
use crate::{ControlMasterMode, scp, support, tmux};
use iced::widget::column;

#[derive(Clone, Copy, Debug)]
pub(in crate::iced_app) enum SshField {
    Local,
    Remote,
    Dynamic,
    ControlPath,
    ControlPersist,
}
impl App {
    fn tool_target(&self) -> Result<Session, String> {
        self.tools
            .target
            .clone()
            .ok_or_else(|| "Open a session or select a saved profile in Profiles first.".into())
    }
    pub(super) fn ssh_result(&mut self, data: Data) {
        match data {
            Data::Text(value) => self.tools.output = value,
            Data::Report(value) => {
                self.tools.report = value;
                self.status = "Sanitized support report generated.".into();
            }
            Data::Host(key, target, value) => {
                if self
                    .tools
                    .target
                    .as_ref()
                    .is_some_and(|s| session_profile_key(s) == key)
                {
                    self.tools.host_target = Some(target);
                    self.tools.output = value;
                }
            }
            Data::Tmux(key, sessions) => {
                if self
                    .tools
                    .target
                    .as_ref()
                    .is_some_and(|s| session_profile_key(s) == key)
                {
                    self.tools.tmux = sessions;
                }
            }
            Data::Tunnel(key, tunnel) => {
                self.tools.tunnel_target = Some(key);
                self.tools.tunnel = Some(tunnel);
                self.status="Tunnel process started. Authentication must already be available non-interactively.".into();
            }
            _ => {}
        }
    }
    pub(super) fn ssh_confirm(&mut self, confirm: Confirm) -> Task<Message> {
        match confirm {
            Confirm::RemoveKey(target, path) => {
                return self.tool_job(move || {
                    terminal::remove_known_host(&target, path.as_deref())
                        .map(Data::Text)
                        .map_err(|e| format!("{e:#}"))
                });
            }
            Confirm::CloseMaster(session) => {
                let config = self.ssh_config.clone();
                return self.tool_job(move || {
                    terminal::control_master_operation(&session, config.as_deref(), "exit")
                        .map(Data::Text)
                        .map_err(|e| format!("{e:#}"))
                });
            }
            Confirm::Scp {
                session,
                upload,
                local,
                remote,
                overwrite,
            } => {
                if self.tools.scp_cancel.is_some() {
                    return Task::none();
                }
                let config = self.ssh_config.clone();
                let cancel = Arc::new(AtomicBool::new(false));
                self.tools.scp_cancel = Some(cancel.clone());
                self.tools.scp_id = self.tools.scp_id.wrapping_add(1);
                let id = self.tools.scp_id;
                self.tools.scp_status = format!(
                    "SCP {} · {}",
                    if upload { "upload" } else { "download" },
                    session.name
                );
                self.tools.scp_progress = (0, 0);
                let (sender, receiver) = mpsc::unbounded();
                thread::spawn(move || {
                    let result = (|| -> Result<String, String> {
                        let mut transfer = if upload {
                            scp::start_upload(
                                &session,
                                config.as_deref(),
                                &local,
                                &remote,
                                overwrite,
                            )
                        } else {
                            scp::start_download(
                                &session,
                                config.as_deref(),
                                &remote,
                                &local,
                                overwrite,
                            )
                        }
                        .map_err(|e| format!("{e:#}"))?;
                        loop {
                            if cancel.load(Ordering::Acquire) {
                                transfer.cancel().map_err(|e| format!("{e:#}"))?;
                                return Ok(
                                    "SCP cancelled; partial data was not published as success."
                                        .into(),
                                );
                            }
                            let _ = sender.unbounded_send(Action::ScpProgress(
                                id,
                                transfer.transferred_bytes(),
                                transfer.total_bytes(),
                            ));
                            match transfer.poll(){Ok(Some(()))=>return Ok("SCP complete; staged destination committed after verification.".into()),Ok(None)=>thread::sleep(Duration::from_millis(100)),Err(e)=>return Err(format!("{e:#}"))}
                        }
                    })();
                    let _ = sender.unbounded_send(Action::ScpDone(id, result));
                });
                return Task::run(receiver, msg);
            }
            _ => {}
        }
        Task::none()
    }
    pub(super) fn ssh_update(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::SshText(field, value) => {
                if let Some(session) = &mut self.tools.target {
                    match field {
                        SshField::Local => {
                            session.ssh.local_forwards = value
                                .split(';')
                                .map(str::trim)
                                .filter(|v| !v.is_empty())
                                .map(str::to_owned)
                                .collect()
                        }
                        SshField::Remote => {
                            session.ssh.remote_forwards = value
                                .split(';')
                                .map(str::trim)
                                .filter(|v| !v.is_empty())
                                .map(str::to_owned)
                                .collect()
                        }
                        SshField::Dynamic => {
                            session.ssh.dynamic_forwards = value
                                .split(';')
                                .map(str::trim)
                                .filter(|v| !v.is_empty())
                                .map(str::to_owned)
                                .collect()
                        }
                        SshField::ControlPath => session.ssh.control_path = value,
                        SshField::ControlPersist => {
                            self.tools.control_persist = value;
                        }
                    }
                }
            }
            Action::MasterMode(mode) => {
                if let Some(session) = &mut self.tools.target {
                    session.ssh.control_master = mode;
                }
            }
            Action::SshPolicy(index, value) => {
                if let Some(session) = &mut self.tools.target {
                    match index {
                        0 => session.ssh.identities_only = value,
                        _ => session.ssh.gssapi_delegate_credentials = value,
                    }
                }
            }
            Action::SaveSshPolicy => {
                if let Ok(mut session) = self.tool_target() {
                    if self.tools.control_persist.trim().is_empty() {
                        session.ssh.control_persist_seconds = None;
                    } else {
                        match self.tools.control_persist.trim().parse::<u32>() {
                            Ok(v) => session.ssh.control_persist_seconds = Some(v),
                            Err(_) => {
                                self.status =
                                    "ControlPersist must be a non-negative integer or blank."
                                        .into();
                                return Task::none();
                            }
                        }
                    }
                    let key = session_profile_key(&session);
                    match save_session_edit(&self.profiles, Some(&key), session.clone()) {
                        Ok(profiles) => {
                            let path = self.profiles_path.clone();
                            self.tools.target = Some(session);
                            return self.tool_job(move || {
                                save_sessions(&path, &profiles)
                                    .map(|_| Data::Profiles(profiles))
                                    .map_err(|e| format!("{e:#}"))
                            });
                        }
                        Err(e) => self.status = format!("{e:#}"),
                    }
                }
            }
            Action::InspectKey => match self.tool_target() {
                Ok(session) => {
                    let key = session_profile_key(&session);
                    let config = self.ssh_config.clone();
                    let path = (!self.tools.known_hosts.trim().is_empty())
                        .then(|| PathBuf::from(self.tools.known_hosts.trim()));
                    return self.tool_job(move || {
                        let target = terminal::resolve_host_key_target(&session, config.as_deref())
                            .map_err(|e| format!("{e:#}"))?;
                        let value = terminal::inspect_known_host(&target, path.as_deref())
                            .map_err(|e| format!("{e:#}"))?;
                        Ok(Data::Host(key, target, value))
                    });
                }
                Err(e) => self.status = e,
            },
            Action::AskRemoveKey => {
                if let Some(target) = &self.tools.host_target {
                    self.tools.confirm = Some(Confirm::RemoveKey(
                        target.clone(),
                        (!self.tools.known_hosts.trim().is_empty())
                            .then(|| PathBuf::from(self.tools.known_hosts.trim())),
                    ));
                } else {
                    self.status =
                        "Inspect the exact trusted-key target before requesting removal.".into();
                }
            }
            Action::MasterCheck => match self.tool_target() {
                Ok(session) => {
                    let config = self.ssh_config.clone();
                    return self.tool_job(move || {
                        terminal::control_master_operation(&session, config.as_deref(), "check")
                            .map(Data::Text)
                            .map_err(|e| format!("{e:#}"))
                    });
                }
                Err(e) => self.status = e,
            },
            Action::MasterClose => {
                if let Ok(session) = self.tool_target() {
                    self.tools.confirm = Some(Confirm::CloseMaster(session));
                }
            }
            Action::StartTunnels => {
                if self.tools.tunnel.is_some() {
                    self.status =
                        "Stop the existing tunnel manager before starting another.".into();
                    return Task::none();
                }
                match self.tool_target() {
                    Ok(session) => {
                        let key = session_profile_key(&session);
                        let config = self.ssh_config.clone();
                        match if crate::session_requires_forward_risk_ack(&session)
                            && !self.tools.tunnel_exposure
                        {
                            Err(anyhow::anyhow!(
                                "Non-loopback tunnel listeners require explicit acknowledgement."
                            ))
                        } else {
                            Ok(())
                        } {
                            Ok(()) => {
                                self.tools.tunnel_exposure = false;
                                return self.tool_job(move || {
                                    terminal::start_tunnels(&session, config.as_deref())
                                        .map(|t| Data::Tunnel(key, Arc::new(Mutex::new(t))))
                                        .map_err(|e| format!("{e:#}"))
                                });
                            }
                            Err(e) => self.status = format!("{e:#}"),
                        }
                    }
                    Err(e) => self.status = e,
                }
            }
            Action::TunnelExposure(v) => self.tools.tunnel_exposure = v,
            Action::StopTunnels => {
                if self.tools.busy {
                    return Task::none();
                }
                if let Some(tunnel) = self.tools.tunnel.take() {
                    self.tools.tunnel_target = None;
                    return self.tool_job(move || {
                        tunnel
                            .lock()
                            .map_err(|_| "Tunnel state lock failed.".to_owned())?
                            .stop()
                            .map(|_| Data::Notice("Tunnel manager stopped.".into()))
                            .map_err(|e| format!("{e:#}"))
                    });
                }
            }
            Action::TmuxRefresh => match self.tool_target() {
                Ok(session) => {
                    let key = session_profile_key(&session);
                    let config = self.ssh_config.clone();
                    return self.tool_job(move || {
                        tmux::list_sessions(&session, config.as_deref())
                            .map(|v| Data::Tmux(key, v))
                            .map_err(|e| format!("{e:#}"))
                    });
                }
                Err(e) => self.status = e,
            },
            Action::TmuxAttach(name) => {
                match self.tool_target().and_then(|session| {
                    tmux::attach_session(&session, &name).map_err(|e| format!("{e:#}"))
                }) {
                    Ok(session) => self.open_profile_value(session),
                    Err(e) => self.status = e,
                }
            }
            Action::TmuxCreate => {
                match self.tool_target().and_then(|session| {
                    tmux::create_session(&session, &self.tools.tmux_name)
                        .map_err(|e| format!("{e:#}"))
                }) {
                    Ok(session) => {
                        let attach = tmux::attach_session(
                            &self.tools.target.clone().unwrap(),
                            &self.tools.tmux_name,
                        )
                        .unwrap();
                        self.open_profile_value(session);
                        if let Some(tab) = self.tabs.last_mut() {
                            for (_, p) in tab.panes.iter_mut() {
                                p.profile = attach.clone();
                            }
                            tab.profile = attach;
                        }
                    }
                    Err(e) => self.status = e,
                }
            }
            Action::SftpTerminal => match self.tool_target() {
                Ok(profile) => {
                    self.tools.sync.set_armed(false);
                    let pane = self.new_terminal_pane_kind(profile.clone(), true);
                    self.tabs.push(Workspace::new(profile, pane));
                    self.active = self.tabs.len() - 1;
                    self.dialog = None;
                }
                Err(e) => self.status = e,
            },
            Action::ScpStart(upload) => {
                if self.tools.scp_cancel.is_none() {
                    match self.tool_target() {
                        Ok(session) => {
                            self.tools.confirm = Some(Confirm::Scp {
                                session,
                                upload,
                                local: PathBuf::from(self.tools.scp_local.trim()),
                                remote: self.tools.scp_remote.trim().to_owned(),
                                overwrite: self.tools.scp_overwrite,
                            })
                        }
                        Err(e) => self.status = e,
                    }
                }
            }
            Action::ScpOverwrite(v) => self.tools.scp_overwrite = v,
            Action::ScpCancel => {
                if let Some(cancel) = &self.tools.scp_cancel {
                    cancel.store(true, Ordering::Release);
                    self.tools.scp_status = "Cancelling SCP…".into();
                }
            }
            Action::ScpProgress(id, done, total) => {
                if id == self.tools.scp_id {
                    self.tools.scp_progress = (done, total);
                }
            }
            Action::ScpDone(id, result) => {
                if id == self.tools.scp_id {
                    self.tools.scp_cancel = None;
                    self.tools.scp_status = match result {
                        Ok(s) => s,
                        Err(e) => {
                            self.tools.errors.record(&e);
                            e
                        }
                    };
                }
            }
            Action::GenerateReport => {
                let profile = self.tools.target.clone();
                let errors = self.tools.errors.clone();
                let explicit = self.ssh_config.is_some();
                return self.tool_job(move || {
                    Ok(Data::Report(support::collect(
                        profile.as_ref(),
                        explicit,
                        &errors,
                    )))
                });
            }
            Action::ExportReport => {
                let path = PathBuf::from(self.tools.report_path.trim());
                let report = self.tools.report.clone();
                if report.is_empty() {
                    self.status = "Generate a report first.".into();
                    return Task::none();
                }
                return self.tool_job(move || {
                    support::export(&path, &report)
                        .map(|_| {
                            Data::Notice(
                                "Sanitized support report exported without overwriting.".into(),
                            )
                        })
                        .map_err(|e| format!("{e:#}"))
                });
            }
            Action::ClearErrors => self.tools.errors.clear(),
            _ => {}
        }
        Task::none()
    }
    pub(super) fn ssh_tool_view(&self) -> Element<'_, Message> {
        let target = self.tools.target.as_ref();
        let title = target
            .map(|s| format!("Target: {} · {}", s.name, s.host))
            .unwrap_or_else(|| {
                "Choose a saved profile in Profiles or focus an SSH pane first.".into()
            });
        match self.tools.panel{
            Panel::Scp=>column![text("SCP transfer").size(24),text(title),input("Local file",&self.tools.scp_local,Field::ScpLocal),input("Remote file",&self.tools.scp_remote,Field::ScpRemote),checkbox(self.tools.scp_overwrite).label("Explicitly allow destination overwrite").on_toggle(|v|msg(Action::ScpOverwrite(v))),row![act("Upload…",Action::ScpStart(true)),act("Download…",Action::ScpStart(false)),act("Cancel transfer",Action::ScpCancel)].spacing(6),text(format!("{} · {} / {} bytes",self.tools.scp_status,self.tools.scp_progress.0,self.tools.scp_progress.1)),text("Transfers use staged destinations and existing host-trust, authentication and proxy policy. Partial files are not reported as success.").size(12)].spacing(12).into(),
            Panel::Diagnostics=>column![text("Sanitized support report").size(24),text(title),row![act("Generate report",Action::GenerateReport),act("Clear recent errors",Action::ClearErrors)].spacing(6),scrollable(text(&self.tools.report).font(Font::MONOSPACE).size(12)).height(300),input("New report export path",&self.tools.report_path,Field::ReportPath),act("Export report",Action::ExportReport)].spacing(12).into(),
            _=>{
                let mut tmux_list=column![].spacing(4);for session in &self.tools.tmux{tmux_list=tmux_list.push(row![text(format!("{} · {} clients",session.name,session.attached_clients)),act("Attach",Action::TmuxAttach(session.name.clone()))].spacing(8));}
                let mut policy=column![text("Advanced profile policy")].spacing(8);if let Some(session)=target{
                    for(label,field,value)in[("Local forwards (separate specs with ;)",SshField::Local,session.ssh.local_forwards.join(";")),("Remote forwards",SshField::Remote,session.ssh.remote_forwards.join(";")),("Dynamic SOCKS forwards",SshField::Dynamic,session.ssh.dynamic_forwards.join(";")),("ControlPath",SshField::ControlPath,session.ssh.control_path.clone()),("ControlPersist seconds",SshField::ControlPersist,self.tools.control_persist.clone())]{policy=policy.push(column![text(label).size(12),text_input(label,&value).on_input(move|v|msg(Action::SshText(field,v))).padding(8)].spacing(4));}
                    policy=policy.push(row![act("Inherit multiplexing",Action::MasterMode(ControlMasterMode::Inherit)).style(if session.ssh.control_master==ControlMasterMode::Inherit{selected_button}else{quiet}),act("Disable",Action::MasterMode(ControlMasterMode::Disabled)).style(if session.ssh.control_master==ControlMasterMode::Disabled{selected_button}else{quiet}),act("Auto",Action::MasterMode(ControlMasterMode::Auto)).style(if session.ssh.control_master==ControlMasterMode::Auto{selected_button}else{quiet})].spacing(5));
                    for(index,label,value)in[(0,"IdentitiesOnly",session.ssh.identities_only),(1,"GSSAPI credential delegation",session.ssh.gssapi_delegate_credentials)]{policy=policy.push(column![text(label).size(12),row![act("Inherit",Action::SshPolicy(index,None)).style(if value.is_none(){selected_button}else{quiet}),act("Enable",Action::SshPolicy(index,Some(true))).style(if value==Some(true){selected_button}else{quiet}),act("Disable",Action::SshPolicy(index,Some(false))).style(if value==Some(false){selected_button}else{quiet})].spacing(5)]);}
                    policy=policy.push(text("Credential delegation exposes credentials to the remote host. Enable only for trusted hosts.").size(12)).push(act("Save advanced profile policy",Action::SaveSshPolicy));
                }
                column![text("SSH management").size(24),text(title),input("known_hosts override (blank uses OpenSSH default)",&self.tools.known_hosts,Field::KnownHosts),row![act("Inspect trusted key",Action::InspectKey),act("Remove trusted key…",Action::AskRemoveKey)].spacing(6),scrollable(text(&self.tools.output).font(Font::MONOSPACE).size(12)).height(120),policy,row![act("Check ControlMaster",Action::MasterCheck),act("Close ControlMaster…",Action::MasterClose)].spacing(6),text(format!("Tunnel manager: {}",self.tools.tunnel_target.as_deref().unwrap_or("stopped"))),checkbox(self.tools.tunnel_exposure).label("Acknowledge non-loopback tunnel listener exposure").on_toggle(|v|msg(Action::TunnelExposure(v))),row![act("Start tunnels",Action::StartTunnels),act("Stop tunnels",Action::StopTunnels)].spacing(6),row![act("Refresh tmux sessions",Action::TmuxRefresh),act("Open SFTP terminal",Action::SftpTerminal)].spacing(6),tmux_list,input("New tmux session name",&self.tools.tmux_name,Field::TmuxName),act("Create tmux session",Action::TmuxCreate)].spacing(10).into()
            }
        }
    }
}
