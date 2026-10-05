//! Headless CLI wrapper around the shared privacy-safe support diagnostics.
use inspirum_terminal::{
    load_sessions,
    support::{self, SanitizedErrorHistory},
};
use std::path::Path;

pub fn collect(profiles: Option<&Path>, selected: Option<&str>, explicit_config: bool) -> String {
    let history = SanitizedErrorHistory::default();
    match (profiles, selected) {
        (Some(path), Some(name)) => match load_sessions(path) {
            Ok(sessions) => {
                let mut matches = sessions.iter().filter(|session| session.name == name);
                let first = matches.next();
                let unique = first.is_some() && matches.next().is_none();
                if unique {
                    support::collect(first, explicit_config, &history)
                } else {
                    let report = support::collect(None, explicit_config, &history);
                    report.replace(
                        "Effective app launch policy: not requested\n",
                        "Saved profile: not found or ambiguous (identifiers omitted)\nEffective app launch policy: unavailable\n",
                    )
                }
            }
            Err(_) => {
                let report = support::collect(None, explicit_config, &history);
                report.replace(
                    "Effective app launch policy: not requested\n",
                    "Saved profiles: unavailable or invalid (details omitted)\nEffective app launch policy: unavailable\n",
                )
            }
        },
        _ => {
            let report = support::collect(None, explicit_config, &history);
            report.replace(
                "Effective app launch policy: not requested\n",
                "Effective app launch policy: not requested; profile store not read\n",
            )
        }
    }
}

pub fn export(path: &Path, report: &str) -> anyhow::Result<()> {
    support::export(path, report)
}
