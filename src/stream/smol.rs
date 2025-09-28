use super::{Error, Stream};

use imap_next::{Interrupt, Io, State};
use smol::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::trace;

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
                Io::Output(ref bytes) => match self.stream.write(bytes).await? {
                    0 => return Err(Error::Closed),
                    n => trace!("wrote {n}/{} bytes", bytes.len()),
                },
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
