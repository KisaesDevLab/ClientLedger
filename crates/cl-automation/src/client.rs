//! The client side: read the control file (with the same safety checks the
//! app applies), connect, and call methods.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::actions::{ErrorBody, Request, Response};
use crate::control_file::{self, ControlFileError};

/// Longest response line accepted (inspect trees can be large).
const MAX_RESPONSE: u64 = 32 * 1024 * 1024;

/// Why a call failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The control file was refused or unreadable.
    #[error(transparent)]
    File(#[from] ControlFileError),
    /// The connection failed.
    #[error("the app could not be reached: {0}")]
    Io(#[from] std::io::Error),
    /// The app answered with an error.
    #[error("{}", .0.message)]
    Remote(ErrorBody),
    /// The answer was not understood.
    #[error("the app sent an answer that could not be understood")]
    Protocol,
}

/// Anything that can call a control method (the real client, or a fake).
pub trait Transport {
    /// Call `method` and return its result.
    ///
    /// # Errors
    /// [`ClientError`].
    fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError>;
}

/// A connection to a running app.
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    token: String,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// Connect using a control file.
    ///
    /// # Errors
    /// The file is unsafe or missing, or nothing is listening.
    pub fn connect(control: &Path) -> Result<Client, ClientError> {
        let info = control_file::read_checked(control)?;
        let stream = TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], info.port)),
            Duration::from_secs(5),
        )?;
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let writer = stream.try_clone()?;
        Ok(Client {
            reader: BufReader::new(stream),
            writer,
            token: info.token,
        })
    }

    /// Wait up to `timeout` for the control file to appear and the app to
    /// answer `state`.
    ///
    /// # Errors
    /// The last connection error when the time runs out.
    pub fn connect_wait(control: &Path, timeout: Duration) -> Result<Client, ClientError> {
        let start = std::time::Instant::now();
        loop {
            match Client::connect(control) {
                Ok(mut c) => match c.call("state", Value::Null) {
                    Ok(_) => return Ok(c),
                    Err(e) if start.elapsed() >= timeout => return Err(e),
                    Err(_) => {}
                },
                // An unsafe file is final; do not wait it out.
                Err(e @ ClientError::File(ControlFileError::Unsafe(_))) => return Err(e),
                Err(e) if start.elapsed() >= timeout => return Err(e),
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

impl Transport for Client {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let request = Request {
            token: self.token.clone(),
            method: method.to_string(),
            params,
            id: None,
        };
        let mut text = serde_json::to_string(&request).map_err(|_| ClientError::Protocol)?;
        text.push('\n');
        self.writer.write_all(text.as_bytes())?;
        self.writer.flush()?;
        let mut line = String::new();
        (&mut self.reader).take(MAX_RESPONSE).read_line(&mut line)?;
        let response: Response = serde_json::from_str(&line).map_err(|_| ClientError::Protocol)?;
        if response.ok {
            Ok(response.result.unwrap_or(Value::Null))
        } else {
            Err(ClientError::Remote(response.error.ok_or(ClientError::Protocol)?))
        }
    }
}
