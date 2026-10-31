//! Framing shared by the external sidecar transports (`tsrs --api` now, the
//! VS Code extension channel later). See
//! `docs/research/dependency/tsrs-type-aware-checking.md` ("Wire protocol").

pub mod message_pack;

use std::fmt;
use std::io::{self, Read};

/// Largest frame we will read; matches tsrs's `DEFAULT_MAX_FRAME_BYTES`.
pub const DEFAULT_MAX_FRAME_BYTES: u64 = 1 << 30;

#[derive(Debug)]
pub enum TransportError {
    /// Clean EOF before the first byte of a frame.
    Eof,
    /// EOF inside a frame.
    UnexpectedEof(&'static str),
    Io(io::Error),
    /// Malformed frame or message.
    Protocol(String),
    /// A declared frame length above the configured limit.
    FrameTooLarge {
        declared: u64,
        limit: u64,
    },
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Eof => write!(f, "connection closed"),
            TransportError::UnexpectedEof(what) => {
                write!(f, "connection closed while reading {what}")
            }
            TransportError::Io(e) => write!(f, "io error: {e}"),
            TransportError::Protocol(msg) => write!(f, "protocol error: {msg}"),
            TransportError::FrameTooLarge { declared, limit } => {
                write!(f, "frame of {declared} bytes exceeds limit of {limit}")
            }
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TransportError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(e: io::Error) -> Self {
        TransportError::Io(e)
    }
}

/// Reads exactly `len` bytes without trusting `len` for the up-front
/// allocation: the buffer grows only as bytes actually arrive, so a hostile
/// length cannot force a huge allocation.
pub(crate) fn read_exact_bounded<R: io::Read>(
    r: &mut R,
    len: u64,
    limit: u64,
    what: &'static str,
) -> Result<Vec<u8>, TransportError> {
    if len > limit {
        return Err(TransportError::FrameTooLarge { declared: len, limit });
    }
    let mut out = Vec::with_capacity(len.min(64 * 1024) as usize);
    let n = r.take(len).read_to_end(&mut out)?;
    if n as u64 != len {
        return Err(TransportError::UnexpectedEof(what));
    }
    Ok(out)
}
