//! Helper for blocking communication over the realand socket.

use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::{Event, Reply, Request};

/// Maximum size of one request, reply, or event line, including its newline.
///
/// The compositor applies this same bound to every IPC line.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Name of the environment variable containing the realand IPC socket path.
pub const SOCKET_PATH_ENV: &str = "REALAND_SOCKET";

/// Helper for blocking communication over the realand socket.
///
/// This struct is used to communicate with the realand IPC server. It handles the socket connection
/// and serialization/deserialization of messages.
pub struct Socket {
    stream: BufReader<UnixStream>,
}

impl Socket {
    /// Connects to the default realand IPC socket.
    ///
    /// This is equivalent to calling [`Self::connect_to`] with the path taken from the
    /// [`SOCKET_PATH_ENV`] environment variable.
    pub fn connect() -> io::Result<Self> {
        let socket_path = env::var_os(SOCKET_PATH_ENV).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{SOCKET_PATH_ENV} is not set, are you running this within realand?"),
            )
        })?;
        Self::connect_to(socket_path)
    }

    /// Connects to the realand IPC socket at the given path.
    pub fn connect_to(path: impl AsRef<Path>) -> io::Result<Self> {
        let stream = UnixStream::connect(path.as_ref())?;
        let stream = BufReader::new(stream);
        Ok(Self { stream })
    }

    /// Sends a request to realand and returns the response.
    ///
    /// Return values:
    ///
    /// * `Ok(Ok(response))`: successful [`Response`](crate::Response) from realand
    /// * `Ok(Err(message))`: error message from realand
    /// * `Err(error)`: error communicating with realand
    ///
    /// Requests that cannot be represented as JSON, or exceed the 16 MiB line
    /// limit, return [`io::ErrorKind::InvalidInput`]. A response line larger
    /// than 16 MiB is rejected with [`io::ErrorKind::InvalidData`].
    pub fn send(&mut self, request: Request) -> io::Result<Reply> {
        let mut request = serde_json::to_vec(&request)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        if request.len() + 1 > MAX_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "IPC request too large",
            ));
        }
        request.push(b'\n');
        self.stream.get_mut().write_all(&request)?;

        let response = read_line_bounded(&mut self.stream)?;

        let reply = serde_json::from_slice(&response)?;
        Ok(reply)
    }

    /// Starts reading event stream [`Event`]s from the socket.
    ///
    /// The returned function will block until the next [`Event`] arrives, then return it.
    ///
    /// Use this only after requesting an [`EventStream`][Request::EventStream].
    /// Each event line is bounded to 16 MiB; larger lines return
    /// [`io::ErrorKind::InvalidData`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use realand_ipc::{Request, Response};
    /// use realand_ipc::socket::Socket;
    ///
    /// fn main() -> std::io::Result<()> {
    ///     let mut socket = Socket::connect()?;
    ///
    ///     let reply = socket.send(Request::EventStream)?;
    ///     if matches!(reply, Ok(Response::Handled)) {
    ///         let mut read_event = socket.read_events();
    ///         while let Ok(event) = read_event() {
    ///             println!("Received event: {event:?}");
    ///         }
    ///     }
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn read_events(self) -> impl FnMut() -> io::Result<Event> {
        let Self { mut stream } = self;
        let _ = stream.get_mut().shutdown(Shutdown::Write);
        let mut line = Vec::new();

        move || {
            read_line_bounded_into(&mut stream, &mut line)?;
            let event = serde_json::from_slice(&line)?;
            Ok(event)
        }
    }
}

fn read_line_bounded(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    read_line_bounded_into(reader, &mut line)?;
    Ok(line)
}

fn read_line_bounded_into(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<()> {
    line.clear();
    (&mut *reader)
        .take((MAX_LINE_BYTES + 1) as u64)
        .read_until(b'\n', line)?;
    if line.len() > MAX_LINE_BYTES {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC line too large",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn socket_from(stream: UnixStream) -> Socket {
        Socket {
            stream: BufReader::new(stream),
        }
    }

    fn write_allowing_disconnect(peer: &mut UnixStream, bytes: &[u8]) -> io::Result<()> {
        match peer.write_all(bytes) {
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) =>
            {
                // Rejecting a line can close the reader before the producer
                // finishes writing its payload or trailing newline.
                Ok(())
            }
            result => result,
        }
    }

    #[test]
    fn rejected_line_producer_allows_the_reader_to_disconnect_early() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        drop(client);
        write_allowing_disconnect(&mut peer, b"\n").unwrap();
    }

    #[test]
    fn serializes_non_finite_request_values_without_panicking() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let mut socket = socket_from(client);
        let request = Request::Action(crate::Action::MoveFloatingWindow {
            id: None,
            x: crate::PositionChange::SetFixed(f64::NAN),
            y: crate::PositionChange::SetFixed(0.),
        });

        // Depending on serde_json's float policy, the non-finite value is
        // either rejected or encoded as `null`; in both cases the client must
        // return an error or a normal reply rather than panic. Keep a peer
        // reader so a successful serialization cannot block waiting for the
        // response line.
        let task = std::thread::spawn(move || {
            peer.set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            let mut request = Vec::new();
            if BufReader::new(&mut peer)
                .read_until(b'\n', &mut request)
                .is_ok()
            {
                peer.write_all(b"{\"Ok\":\"Handled\"}\n").unwrap();
            }
        });
        let result = socket.send(request);
        task.join().unwrap();
        assert!(matches!(result, Ok(Ok(crate::Response::Handled)) | Err(_)));
    }

    #[test]
    fn rejects_oversized_replies() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let mut socket = socket_from(client);
        let task = std::thread::spawn(move || {
            let mut request = Vec::new();
            BufReader::new(&mut peer)
                .read_until(b'\n', &mut request)
                .unwrap();
            write_allowing_disconnect(&mut peer, &vec![b'x'; MAX_LINE_BYTES + 1]).unwrap();
            write_allowing_disconnect(&mut peer, b"\n").unwrap();
        });

        let error = socket.send(Request::Version).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        task.join().unwrap();
    }

    #[test]
    fn rejects_oversized_requests_before_writing() {
        let (client, _peer) = UnixStream::pair().unwrap();
        let mut socket = socket_from(client);
        let request = Request::PreviewOutputConfig {
            root_path: "/tmp/config.kdl".into(),
            edited_path: "/tmp/config.kdl".into(),
            content: "x".repeat(MAX_LINE_BYTES),
        };

        let error = socket.send(request).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn event_stream_lines_are_bounded() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let mut events = socket_from(client).read_events();
        let task = std::thread::spawn(move || {
            write_allowing_disconnect(&mut peer, &vec![b'x'; MAX_LINE_BYTES + 1]).unwrap();
            write_allowing_disconnect(&mut peer, b"\n").unwrap();
        });

        let error = events().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        drop(events);
        task.join().unwrap();
    }
}
