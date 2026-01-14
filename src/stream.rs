use std::io::ErrorKind;

use imap_next::{Interrupt, Io, State};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::trace;

pub struct Stream<S> {
    stream: S,
    buf: Vec<u8>,
}

impl<S> Stream<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buf: vec![0; 1024].into(),
        }
    }

    pub fn into_inner(self) -> S {
        self.stream
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Stream<S> {
    pub async fn next<F: State>(&mut self, mut state: F) -> Result<F::Event, Error<F::Error>> {
        let event = loop {
            // Progress the client/server
            let result = state.next();

            // Return events immediately without doing IO
            let interrupt = match result {
                Err(interrupt) => interrupt,
                Ok(event) => break event,
            };

            // Return errors immediately without doing IO
            let io = match interrupt {
                Interrupt::Io(io) => io,
                Interrupt::Error(err) => return Err(Error::State(err)),
            };

            // Handle the output bytes from the client/server
            match io {
                Io::Output(ref bytes) => {
                    match self.stream.write_all(bytes).await {
                        Ok(()) => trace!("wrote {} bytes", bytes.len()),
                        Err(e) if e.kind() == ErrorKind::WriteZero => return Err(Error::Closed),
                        Err(e) => return Err(e.into()),
                    }
                    self.stream.flush().await?;
                }
                Io::NeedMoreInput => {
                    trace!("more input needed");
                }
            }

            match self.stream.read(&mut self.buf).await? {
                0 => return Err(Error::Closed),
                n => {
                    trace!("read {n}/{} bytes", self.buf.len());
                    state.enqueue_input(&self.buf[..n]);
                }
            }
        };

        Ok(event)
    }
}

/// Error during reading into or writing from a stream.
#[derive(Debug, Error)]
pub enum Error<E> {
    /// Operation failed because stream is closed.
    ///
    /// We detect this by checking if the read or written byte count is 0. Whether the stream is
    /// closed indefinitely or temporarily depends on the actual stream implementation.
    #[error("Stream was closed")]
    Closed,
    /// An I/O error occurred in the underlying stream.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// An error occurred while progressing the state.
    #[error(transparent)]
    State(E),
}
