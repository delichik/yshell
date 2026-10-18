//! Proxy configuration, state, and a lightweight connectivity probe.
//!
//! The live SOCKS/HTTP CONNECT transports are still scaffold-only (see
//! [`crate::real`]); the probe here is what the Proxy Manager "Test" action
//! uses to validate a saved profile without opening an SSH target. It performs
//! a bounded TCP connect and, for SOCKS5, the authentication negotiation that
//! does not need a destination address. SOCKS4/SOCKS4a and HTTP CONNECT stay at
//! the TCP level because their handshakes only start once a target is known.

use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

use crate::error::{SshError, SshErrorKind, SshResult};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProxyConfig {
    #[default]
    None,
    Socks4 {
        address: String,
        username: Option<String>,
    },
    Socks4a {
        address: String,
        username: Option<String>,
    },
    Socks5 {
        address: String,
        username: Option<String>,
        password: Option<String>,
        resolve_dns_by_proxy: bool,
    },
    HttpConnect {
        address: String,
        username: Option<String>,
        password: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyState {
    Disabled,
    Connecting { address: String },
    Connected { address: String },
    Failed { address: String, reason: String },
}

impl ProxyConfig {
    pub fn address(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Socks4 { address, .. }
            | Self::Socks4a { address, .. }
            | Self::Socks5 { address, .. }
            | Self::HttpConnect { address, .. } => Some(address),
        }
    }

    /// Stable protocol id used by status strings and the UI (`socks5`, …).
    #[must_use]
    pub const fn protocol_id(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Socks4 { .. } => "socks4",
            Self::Socks4a { .. } => "socks4a",
            Self::Socks5 { .. } => "socks5",
            Self::HttpConnect { .. } => "http-connect",
        }
    }
}

/// Successful outcome of [`probe_proxy_connectivity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyProbeReport {
    /// Protocol id (`socks4`, `socks4a`, `socks5`, `http-connect`).
    pub protocol: &'static str,
    /// Configured `host:port`.
    pub address: String,
    /// Resolved socket that accepted the TCP connection.
    pub remote: String,
    /// Wall-clock duration of the probe.
    pub elapsed_ms: u128,
    /// Human-readable description of how far the handshake got.
    pub detail: String,
}

/// Probes a proxy for reachability without connecting to a destination.
///
/// * SOCKS5: TCP connect + method negotiation (+ RFC 1929 username/password
///   authentication when the server selects it).
/// * SOCKS4/SOCKS4a/HTTP CONNECT: TCP connect. Their request needs a target
///   host, so the probe cannot go further by design.
///
/// `timeout` bounds the TCP connect and every read/write. The call is blocking
/// and must run off the UI thread.
pub fn probe_proxy_connectivity(
    config: &ProxyConfig,
    timeout: Duration,
) -> SshResult<ProxyProbeReport> {
    let started = Instant::now();
    let address = config
        .address()
        .ok_or_else(|| SshError::configuration("no proxy is configured"))?
        .to_owned();
    let (mut stream, remote) = connect_proxy(&address, timeout)?;
    let detail = match config {
        ProxyConfig::None => unreachable!("address() rejected None"),
        ProxyConfig::Socks5 {
            username, password, ..
        } => negotiate_socks5(&mut stream, username.as_deref(), password.as_deref(), timeout)?,
        ProxyConfig::Socks4 { .. }
        | ProxyConfig::Socks4a { .. }
        | ProxyConfig::HttpConnect { .. } => {
            "TCP connection established (the protocol handshake needs a target host)".to_owned()
        }
    };
    Ok(ProxyProbeReport {
        protocol: config.protocol_id(),
        address,
        remote: remote.to_string(),
        elapsed_ms: started.elapsed().as_millis(),
        detail,
    })
}

fn connect_proxy(address: &str, timeout: Duration) -> SshResult<(TcpStream, SocketAddr)> {
    let candidates = address.to_socket_addrs().map_err(|error| {
        SshError::new(
            SshErrorKind::Dns,
            format!("cannot resolve proxy address `{address}`: {error}"),
        )
        .with_connection_stage("proxy-negotiation")
    })?;
    let mut last_error: Option<io::Error> = None;
    for candidate in candidates {
        match TcpStream::connect_timeout(&candidate, timeout) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                stream.set_read_timeout(Some(timeout)).map_err(|error| {
                    probe_io_error("setting the proxy read timeout", error, timeout)
                })?;
                stream.set_write_timeout(Some(timeout)).map_err(|error| {
                    probe_io_error("setting the proxy write timeout", error, timeout)
                })?;
                return Ok((stream, candidate));
            }
            Err(error) => last_error = Some(error),
        }
    }
    let reason = match last_error {
        Some(error) => error.to_string(),
        None => "the proxy address resolved to no socket address".to_owned(),
    };
    Err(SshError::new(
        SshErrorKind::TcpConnect,
        format!("cannot connect to proxy `{address}`: {reason}"),
    )
    .with_connection_stage("proxy-negotiation"))
}

/// SOCKS5 greeting + optional RFC 1929 authentication (no CONNECT request).
fn negotiate_socks5(
    stream: &mut TcpStream,
    username: Option<&str>,
    password: Option<&str>,
    timeout: Duration,
) -> SshResult<String> {
    let username = username.filter(|value| !value.is_empty());
    let mut methods: Vec<u8> = vec![0x00];
    if username.is_some() {
        methods.push(0x02);
    }
    let mut greeting = Vec::with_capacity(2 + methods.len());
    greeting.push(0x05);
    greeting.push(u8::try_from(methods.len()).unwrap_or(u8::MAX));
    greeting.extend_from_slice(&methods);
    write_proxy(stream, &greeting, "sending the SOCKS5 greeting", timeout)?;

    let mut method_reply = [0u8; 2];
    read_proxy(
        stream,
        &mut method_reply,
        "reading the SOCKS5 method reply",
        timeout,
    )?;
    if method_reply[0] != 0x05 {
        return Err(proxy_protocol_error(format!(
            "unexpected SOCKS5 version byte {:#04x}",
            method_reply[0]
        )));
    }
    match method_reply[1] {
        0x00 => Ok("SOCKS5 negotiation succeeded (no authentication)".to_owned()),
        0x02 => {
            let username = username.ok_or_else(|| {
                proxy_protocol_error(
                    "the proxy requires username/password authentication but the profile has no username",
                )
            })?;
            let password = password.unwrap_or_default();
            let username_bytes = username.as_bytes();
            if username_bytes.len() > usize::from(u8::MAX) {
                return Err(proxy_protocol_error(
                    "the proxy username is longer than 255 bytes",
                ));
            }
            if password.len() > usize::from(u8::MAX) {
                return Err(proxy_protocol_error(
                    "the proxy password is longer than 255 bytes",
                ));
            }
            let mut auth = Vec::with_capacity(3 + username_bytes.len() + password.len());
            auth.push(0x01);
            auth.push(u8::try_from(username_bytes.len()).unwrap_or(u8::MAX));
            auth.extend_from_slice(username_bytes);
            auth.push(u8::try_from(password.len()).unwrap_or(u8::MAX));
            auth.extend_from_slice(password.as_bytes());
            write_proxy(
                stream,
                &auth,
                "sending the SOCKS5 username/password request",
                timeout,
            )?;
            let mut auth_reply = [0u8; 2];
            read_proxy(
                stream,
                &mut auth_reply,
                "reading the SOCKS5 authentication reply",
                timeout,
            )?;
            if auth_reply[1] != 0x00 {
                return Err(proxy_protocol_error(format!(
                    "the proxy rejected username/password authentication (status {:#04x})",
                    auth_reply[1]
                )));
            }
            Ok("SOCKS5 negotiation and username/password authentication succeeded".to_owned())
        }
        0xff => Err(proxy_protocol_error(
            "the proxy accepted none of the offered SOCKS5 authentication methods",
        )),
        method => Err(proxy_protocol_error(format!(
            "the proxy selected unsupported SOCKS5 authentication method {method:#04x}"
        ))),
    }
}

fn write_proxy(
    stream: &mut TcpStream,
    bytes: &[u8],
    action: &str,
    timeout: Duration,
) -> SshResult<()> {
    stream
        .write_all(bytes)
        .map_err(|error| probe_io_error(action, error, timeout))
}

fn read_proxy(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    action: &str,
    timeout: Duration,
) -> SshResult<()> {
    stream
        .read_exact(buffer)
        .map_err(|error| probe_io_error(action, error, timeout))
}

fn probe_io_error(action: &str, error: io::Error, timeout: Duration) -> SshError {
    let kind = match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => SshErrorKind::Timeout,
        _ => SshErrorKind::Proxy,
    };
    let message = if kind == SshErrorKind::Timeout {
        format!("{action} timed out after {} ms", timeout.as_millis())
    } else {
        format!("{action} failed: {error}")
    };
    SshError::new(kind, message).with_connection_stage("proxy-negotiation")
}

fn proxy_protocol_error(message: impl Into<String>) -> SshError {
    SshError::new(SshErrorKind::Proxy, message).with_connection_stage("proxy-negotiation")
}

#[cfg(test)]
mod tests {
    use std::{
        net::TcpListener,
        thread::{self, JoinHandle},
    };

    use super::*;

    fn socks5_server(method: u8, auth_status: u8) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind socks5 test server");
        let address = listener.local_addr().expect("local addr").to_string();
        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept probe");
            let mut header = [0u8; 2];
            socket.read_exact(&mut header).expect("greeting header");
            let mut methods = vec![0u8; usize::from(header[1])];
            socket.read_exact(&mut methods).expect("greeting methods");
            socket
                .write_all(&[0x05, method])
                .expect("write method reply");
            if method == 0x02 {
                let mut auth_header = [0u8; 2];
                socket.read_exact(&mut auth_header).expect("auth header");
                let mut username = vec![0u8; usize::from(auth_header[1])];
                socket.read_exact(&mut username).expect("auth username");
                let mut password_len = [0u8; 1];
                socket
                    .read_exact(&mut password_len)
                    .expect("auth password len");
                let mut password = vec![0u8; usize::from(password_len[0])];
                socket.read_exact(&mut password).expect("auth password");
                socket
                    .write_all(&[0x01, auth_status])
                    .expect("write auth reply");
            }
        });
        (address, handle)
    }

    fn http_proxy_stub() -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind http test server");
        let address = listener.local_addr().expect("local addr").to_string();
        let handle = thread::spawn(move || {
            let _ = listener.accept();
        });
        (address, handle)
    }

    #[test]
    fn socks5_probe_succeeds_without_authentication() {
        let (address, server) = socks5_server(0x00, 0x00);
        let config = ProxyConfig::Socks5 {
            address,
            username: None,
            password: None,
            resolve_dns_by_proxy: true,
        };
        let report = probe_proxy_connectivity(&config, Duration::from_secs(2)).expect("probe");
        assert_eq!(report.protocol, "socks5");
        assert!(report.detail.contains("no authentication"));
        assert!(report.remote.starts_with("127.0.0.1:"));
        server.join().expect("server");
    }

    #[test]
    fn socks5_probe_authenticates_with_username_and_password() {
        let (address, server) = socks5_server(0x02, 0x00);
        let config = ProxyConfig::Socks5 {
            address,
            username: Some("proxy-user".to_owned()),
            password: Some("proxy-secret".to_owned()),
            resolve_dns_by_proxy: true,
        };
        let report = probe_proxy_connectivity(&config, Duration::from_secs(2)).expect("probe");
        assert!(report.detail.contains("authentication succeeded"));
        server.join().expect("server");
    }

    #[test]
    fn socks5_probe_rejects_when_the_server_offers_no_method() {
        let (address, server) = socks5_server(0xff, 0x00);
        let config = ProxyConfig::Socks5 {
            address,
            username: None,
            password: None,
            resolve_dns_by_proxy: true,
        };
        let error = probe_proxy_connectivity(&config, Duration::from_secs(2)).expect_err("probe");
        assert_eq!(error.kind, SshErrorKind::Proxy);
        assert!(error.message.contains("authentication methods"));
        server.join().expect("server");
    }

    #[test]
    fn http_connect_probe_reports_tcp_reachability() {
        let (address, server) = http_proxy_stub();
        let config = ProxyConfig::HttpConnect {
            address,
            username: None,
            password: None,
        };
        let report = probe_proxy_connectivity(&config, Duration::from_secs(2)).expect("probe");
        assert_eq!(report.protocol, "http-connect");
        assert!(report.detail.contains("TCP connection established"));
        server.join().expect("server");
    }

    #[test]
    fn probe_reports_unreachable_ports() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
        let address = listener.local_addr().expect("local addr").to_string();
        drop(listener);
        let config = ProxyConfig::Socks4 {
            address,
            username: None,
        };
        let error =
            probe_proxy_connectivity(&config, Duration::from_millis(500)).expect_err("probe");
        assert_eq!(error.kind, SshErrorKind::TcpConnect);
        assert!(error.message.contains("cannot connect to proxy"));
    }

    #[test]
    fn probing_none_is_a_configuration_error() {
        let error =
            probe_proxy_connectivity(&ProxyConfig::None, Duration::from_millis(50)).expect_err("probe");
        assert_eq!(error.kind, SshErrorKind::Configuration);
    }
}
