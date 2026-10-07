//! UI-only fixture model. Deliberately has no network, process or filesystem APIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub group: String,
}

pub fn samples() -> Vec<Profile> {
    [
        ("prod-api-01", "Production"),
        ("prod-db-01", "Production"),
        ("staging-web-01", "Staging"),
        ("dev-sandbox", "Development"),
    ]
    .into_iter()
    .map(|(name, group)| Profile {
        name: name.into(),
        host: format!("{name}.example"),
        user: "ops".into(),
        port: 22,
        group: group.into(),
    })
    .collect()
}

pub fn matches(profile: &Profile, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    [&profile.name, &profile.host, &profile.group, &profile.user]
        .iter()
        .any(|value| value.to_lowercase().contains(&query))
}

#[derive(Clone, Debug)]
pub struct ConnectionForm {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: String,
    pub advanced: bool,
    pub error: Option<String>,
}

impl Default for ConnectionForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            user: String::new(),
            port: "22".into(),
            advanced: false,
            error: None,
        }
    }
}

impl ConnectionForm {
    // Interaction validation only; production must use the existing SSH policy.
    pub fn validate(&self) -> Result<Profile, String> {
        let host = self.host.trim();
        if host.is_empty() {
            return Err("Enter a host name or IP address.".into());
        }
        if host.len() > 253
            || host.starts_with('-')
            || !host.chars().all(|c| c.is_ascii_alphanumeric() || "._-:[]%".contains(c))
        {
            return Err("Use a host name or IP address, not a command or URL.".into());
        }
        let port = self.port.trim().parse::<u16>()
            .ok().filter(|port| *port != 0)
            .ok_or("Port must be a number from 1 to 65535.")?;
        let user = self.user.trim();
        if user.len() > 128 || user.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("Username cannot contain spaces or control characters.".into());
        }
        let name = self.name.trim();
        if name.len() > 120 || name.chars().any(char::is_control) {
            return Err("Session name must be at most 120 characters with no control characters.".into());
        }
        Ok(Profile {
            name: if name.is_empty() { host.into() } else { name.into() },
            host: host.into(),
            user: user.into(),
            port,
            group: "Preview sessions".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn form() -> ConnectionForm {
        ConnectionForm { host: "server.example".into(), ..Default::default() }
    }
    #[test]
    fn defaults_to_ssh_port_and_host_label() {
        let p = form().validate().unwrap();
        assert_eq!(p.port, 22);
        assert_eq!(p.name, "server.example");
        assert!(p.user.is_empty());
    }
    #[test]
    fn requires_host() { assert!(ConnectionForm::default().validate().is_err()); }
    #[test]
    fn rejects_command_like_hosts() {
        for host in ["-F", "a;id", "a b", "$(id)", "ssh://a", "a\nb"] {
            assert!(ConnectionForm { host: host.into(), ..form() }.validate().is_err());
        }
    }
    #[test]
    fn accepts_hostname_ipv4_ipv6() {
        for host in ["host.example", "127.0.0.1", "::1", "[2001:db8::1]"] {
            assert!(ConnectionForm { host: host.into(), ..form() }.validate().is_ok());
        }
    }
    #[test]
    fn rejects_bad_ports() {
        for port in ["0", "65536", "-22", "abc", ""] {
            assert!(ConnectionForm { port: port.into(), ..form() }.validate().is_err());
        }
    }
    #[test]
    fn accepts_port_boundaries() {
        for port in ["1", "65535"] {
            assert!(ConnectionForm { port: port.into(), ..form() }.validate().is_ok());
        }
    }
    #[test]
    fn trims_profile_fields() {
        let p = ConnectionForm { name: " Demo ".into(), host: " server.example ".into(), user: " ops ".into(), ..form() }.validate().unwrap();
        assert_eq!((p.name.as_str(), p.host.as_str(), p.user.as_str()), ("Demo", "server.example", "ops"));
    }
    #[test]
    fn search_is_case_insensitive_and_includes_groups() {
        assert!(matches(&samples()[0], " PROD "));
        assert!(matches(&samples()[2], "staging"));
        assert!(!matches(&samples()[0], "not-found"));
    }
    #[test]
    fn rejects_control_characters_in_labels() {
        assert!(ConnectionForm { name: "oops\n".repeat(2), ..form() }.validate().is_err());
    }
    #[test]
    fn rejects_invalid_username() {
        assert!(ConnectionForm { user: "two words".into(), ..form() }.validate().is_err());
    }
}
