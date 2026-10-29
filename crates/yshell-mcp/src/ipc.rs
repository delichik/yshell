//! Newline-delimited JSON framing over TCP.

use std::io::{self, BufRead, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::protocol::{IpcRequest, IpcResponse};

#[derive(Debug)]
pub enum IpcError {
    Io(io::Error),
    Json(serde_json::Error),
    Protocol(String),
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Protocol(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for IpcError {}

impl From<io::Error> for IpcError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for IpcError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn write_message<W: Write>(writer: &mut W, response: &IpcResponse) -> Result<(), IpcError> {
    serde_json::to_writer(&mut *writer, response)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub fn write_request<W: Write>(writer: &mut W, request: &IpcRequest) -> Result<(), IpcError> {
    serde_json::to_writer(&mut *writer, request)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub fn read_message<R: BufRead>(reader: &mut R) -> Result<IpcRequest, IpcError> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(IpcError::Protocol("connection closed".to_owned()));
    }
    Ok(serde_json::from_str(line.trim_end())?)
}

pub fn read_response<R: BufRead>(reader: &mut R) -> Result<IpcResponse, IpcError> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(IpcError::Protocol("connection closed".to_owned()));
    }
    Ok(serde_json::from_str(line.trim_end())?)
}

/// Process one TCP connection until EOF, calling `on_request` for each message.
pub fn handle_connection<F>(stream: TcpStream, mut on_request: F) -> Result<(), IpcError>
where
    F: FnMut(IpcRequest) -> IpcResponse,
{
    stream.set_nodelay(true)?;
    // Consent can wait on the UI for a long time; keep the socket open.
    stream.set_read_timeout(None)?;
    let mut reader = io::BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    loop {
        match read_message(&mut reader) {
            Ok(request) => {
                let response = on_request(request);
                write_message(&mut writer, &response)?;
            }
            Err(IpcError::Protocol(message)) if message.contains("connection closed") => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Client helper used by the stdio bridge (one request/response per TCP connect).
pub fn call(
    endpoint: &crate::EndpointInfo,
    request: &IpcRequest,
) -> Result<IpcResponse, IpcError> {
    let stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut writer = stream.try_clone()?;
    write_request(&mut writer, request)?;
    let mut reader = io::BufReader::new(stream);
    read_response(&mut reader)
}
