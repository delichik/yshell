//! Quick Connect parser for SSH destination strings.

use std::{error::Error, fmt};

use crate::schema::SessionProfile;

/// Parsed Quick Connect target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickConnectTarget {
    pub username: Option<String>,
    pub host: String,
    pub port: u16,
}

impl QuickConnectTarget {
    /// Converts the parsed target into an unsaved session profile.
    #[must_use]
    pub fn into_session_profile(self, id: impl Into<String>) -> SessionProfile {
        let mut profile = SessionProfile::new(id, self.host.clone(), self.host);
        profile.username = self.username;
        profile.port = self.port;
        profile
    }
}

/// Quick Connect parse/validation failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickConnectError {
    Empty,
    UnsupportedScheme,
    MissingHost,
    InvalidHost(String),
    InvalidPort(String),
    InvalidUsername,
}

impl fmt::Display for QuickConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("quick connect target is empty"),
            Self::UnsupportedScheme => f.write_str("only ssh:// quick connect URLs are supported"),
            Self::MissingHost => f.write_str("quick connect target is missing a host"),
            Self::InvalidHost(host) => write!(f, "invalid quick connect host '{host}'"),
            Self::InvalidPort(port) => write!(f, "invalid quick connect port '{port}'"),
            Self::InvalidUsername => f.write_str("quick connect username is invalid"),
        }
    }
}

impl Error for QuickConnectError {}

/// Parses host, user@host, host:port, user@host:port, and ssh://user@host:port.
pub fn parse_quick_connect(input: &str) -> Result<QuickConnectTarget, QuickConnectError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(QuickConnectError::Empty);
    }

    let without_scheme = if let Some(rest) = trimmed.strip_prefix("ssh://") {
        rest
    } else if trimmed.contains("://") {
        return Err(QuickConnectError::UnsupportedScheme);
    } else {
        trimmed
    };

    let authority = without_scheme
        .split_once('/')
        .map_or(without_scheme, |(authority, _)| authority);
    let (username, host_port) = parse_username(authority)?;
    let (host, port) = parse_host_port(host_port)?;
    validate_host(host)?;

    Ok(QuickConnectTarget {
        username,
        host: host.to_owned(),
        port,
    })
}

fn parse_username(authority: &str) -> Result<(Option<String>, &str), QuickConnectError> {
    match authority.rsplit_once('@') {
        Some((username, host_port)) => {
            if username.is_empty() || username.contains(char::is_whitespace) {
                return Err(QuickConnectError::InvalidUsername);
            }
            Ok((Some(username.to_owned()), host_port))
        }
        None => Ok((None, authority)),
    }
}

fn parse_host_port(host_port: &str) -> Result<(&str, u16), QuickConnectError> {
    if host_port.is_empty() {
        return Err(QuickConnectError::MissingHost);
    }

    if host_port.starts_with('[') {
        let (host, rest) = host_port
            .split_once(']')
            .ok_or_else(|| QuickConnectError::InvalidHost(host_port.to_owned()))?;
        let host = &host[1..];
        let port = if rest.is_empty() {
            22
        } else if let Some(port) = rest.strip_prefix(':') {
            parse_port(port)?
        } else {
            return Err(QuickConnectError::InvalidHost(host_port.to_owned()));
        };
        return Ok((host, port));
    }

    match host_port.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Ok((host, parse_port(port)?)),
        Some(_) if host_port.matches(':').count() > 1 => Ok((host_port, 22)),
        Some((host, _)) => Ok((host, 22)),
        None => Ok((host_port, 22)),
    }
}

fn parse_port(port: &str) -> Result<u16, QuickConnectError> {
    if port.is_empty() || !port.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(QuickConnectError::InvalidPort(port.to_owned()));
    }
    let parsed = port
        .parse::<u16>()
        .map_err(|_| QuickConnectError::InvalidPort(port.to_owned()))?;
    if parsed == 0 {
        return Err(QuickConnectError::InvalidPort(port.to_owned()));
    }
    Ok(parsed)
}

fn validate_host(host: &str) -> Result<(), QuickConnectError> {
    if host.is_empty() {
        return Err(QuickConnectError::MissingHost);
    }
    if host.contains(char::is_whitespace) || host.contains('@') || host.starts_with('-') {
        return Err(QuickConnectError::InvalidHost(host.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_forms() {
        let cases = [
            ("example.com", None, "example.com", 22),
            ("alice@example.com", Some("alice"), "example.com", 22),
            ("example.com:2200", None, "example.com", 2200),
            ("alice@example.com:2200", Some("alice"), "example.com", 2200),
            (
                "ssh://alice@example.com:2200",
                Some("alice"),
                "example.com",
                2200,
            ),
        ];

        for (input, user, host, port) in cases {
            let parsed = parse_quick_connect(input).expect(input);
            assert_eq!(parsed.username.as_deref(), user);
            assert_eq!(parsed.host, host);
            assert_eq!(parsed.port, port);
        }
    }

    #[test]
    fn rejects_bad_port_and_host() {
        assert_eq!(
            parse_quick_connect("example.com:0").expect_err("bad port"),
            QuickConnectError::InvalidPort("0".to_owned())
        );
        assert_eq!(
            parse_quick_connect("bad host").expect_err("bad host"),
            QuickConnectError::InvalidHost("bad host".to_owned())
        );
    }
}
