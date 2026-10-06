//! Persisted two-pane SSH workspace policy and synchronized-input targeting.
use crate::Session;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, io::Read, path::Path};

const MAX_WORKSPACE_BYTES: usize = 1_048_576;
pub const MAX_WORKSPACE_PANES: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLayout {
    pub version: u8,
    pub axis: SplitAxis,
    /// Reconnection is intentionally opt-in and defaults to false.
    #[serde(default)]
    pub reconnect_on_restore: bool,
    pub panes: Vec<Session>,
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            version: 1,
            axis: SplitAxis::Horizontal,
            reconnect_on_restore: false,
            panes: Vec::new(),
        }
    }
}

impl WorkspaceLayout {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported workspace layout version");
        ensure!(
            !self.panes.is_empty() && self.panes.len() <= MAX_WORKSPACE_PANES,
            "workspace must contain one or two SSH panes"
        );
        for pane in &self.panes {
            pane.ssh_args().context("invalid SSH pane in workspace")?;
        }
        Ok(())
    }

    /// Loading layout metadata never means permission to connect.
    pub fn may_reconnect(&self, explicit_user_restore: bool) -> bool {
        explicit_user_restore && self.reconnect_on_restore
    }
}

pub fn save_layout(path: &Path, layout: &WorkspaceLayout) -> Result<()> {
    layout.validate()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).context("create workspace directory")?;
    let bytes = serde_json::to_vec_pretty(layout)?;
    ensure!(
        bytes.len() <= MAX_WORKSPACE_BYTES,
        "workspace layout exceeds 1 MiB"
    );
    let mut temp = tempfile::NamedTempFile::new_in(parent).context("create workspace temp file")?;
    use std::io::Write;
    temp.write_all(&bytes)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| error.error)
        .context("replace workspace layout")?;
    Ok(())
}

pub fn load_layout(path: &Path) -> Result<WorkspaceLayout> {
    let file = fs::File::open(path).context("open workspace layout")?;
    let mut bytes = Vec::new();
    file.take((MAX_WORKSPACE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_WORKSPACE_BYTES,
        "workspace layout exceeds 1 MiB"
    );
    let layout: WorkspaceLayout =
        serde_json::from_slice(&bytes).context("parse workspace layout")?;
    layout.validate()?;
    Ok(layout)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncInputState {
    armed: bool,
    targets: BTreeSet<u64>,
}

impl SyncInputState {
    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn set_armed(&mut self, armed: bool) {
        self.armed = armed && self.targets.len() >= 2;
    }

    pub fn is_target(&self, id: u64) -> bool {
        self.targets.contains(&id)
    }

    pub fn set_target(&mut self, id: u64, selected: bool) {
        let changed = if selected {
            self.targets.insert(id)
        } else {
            self.targets.remove(&id)
        };
        if changed {
            self.armed = false;
        }
    }

    pub fn remove_pane(&mut self, id: u64) {
        if self.targets.remove(&id) {
            self.armed = false;
        }
    }

    /// Mirroring is only allowed when armed and the source was explicitly selected.
    pub fn destinations(&self, source: u64) -> Vec<u64> {
        if !self.armed || !self.targets.contains(&source) {
            return Vec::new();
        }
        self.targets
            .iter()
            .copied()
            .filter(|id| *id != source)
            .collect()
    }

    pub fn selected_count(&self) -> usize {
        self.targets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str, host: &str) -> Session {
        Session {
            name: name.into(),
            host: host.into(),
            ..Session::default()
        }
    }

    #[test]
    fn restore_requires_both_persisted_opt_in_and_explicit_action() {
        let mut layout = WorkspaceLayout {
            panes: vec![session("one", "host-one")],
            ..WorkspaceLayout::default()
        };
        assert!(!layout.may_reconnect(false));
        assert!(!layout.may_reconnect(true));
        layout.reconnect_on_restore = true;
        assert!(!layout.may_reconnect(false));
        assert!(layout.may_reconnect(true));
    }

    #[test]
    fn layout_round_trip_is_validated_and_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let layout = WorkspaceLayout {
            axis: SplitAxis::Vertical,
            reconnect_on_restore: false,
            panes: vec![session("one", "host-one"), session("two", "host-two")],
            ..WorkspaceLayout::default()
        };
        save_layout(&path, &layout).unwrap();
        let first = fs::read(&path).unwrap();
        let loaded = load_layout(&path).unwrap();
        assert_eq!(loaded, layout);
        save_layout(&path, &loaded).unwrap();
        assert_eq!(fs::read(&path).unwrap(), first);
    }

    #[test]
    fn synchronized_input_needs_two_explicit_targets_and_armed_source() {
        let mut sync = SyncInputState::default();
        sync.set_target(10, true);
        sync.set_armed(true);
        assert!(!sync.armed());
        assert!(sync.destinations(10).is_empty());

        sync.set_target(20, true);
        assert!(!sync.armed());
        sync.set_armed(true);
        assert!(sync.armed());
        assert_eq!(sync.destinations(10), vec![20]);
        assert_eq!(sync.destinations(20), vec![10]);
        assert!(sync.destinations(30).is_empty());

        sync.remove_pane(20);
        assert!(!sync.armed());
        assert!(sync.destinations(10).is_empty());

        sync.set_target(20, true);
        sync.set_armed(true);
        assert!(sync.armed());
        sync.set_target(30, true);
        assert!(!sync.armed(), "changing the target set must always disarm sync");
    }

    #[test]
    fn layout_rejects_more_than_two_panes() {
        let layout = WorkspaceLayout {
            panes: vec![
                session("one", "host-one"),
                session("two", "host-two"),
                session("three", "host-three"),
            ],
            ..WorkspaceLayout::default()
        };
        assert!(layout.validate().is_err());
    }
}
