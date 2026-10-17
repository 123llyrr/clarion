//! Bounded local IPC operations, including connect and fragmented replies.
use std::io::{self, Read};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

pub const TIMEOUT: Duration = Duration::from_millis(300);

pub fn connect(path: &Path) -> io::Result<UnixStream> {
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(path)?, TIMEOUT)?;
    let fd: std::os::fd::OwnedFd = socket.into();
    let stream = UnixStream::from(fd);
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    Ok(stream)
}

/// A per-read timeout alone can be extended indefinitely by trickling bytes.
/// Apply one deadline and one byte limit to the whole response instead.
pub fn read_line(stream: &mut UnixStream, limit: usize, timeout: Duration) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut line = Vec::new();
    let mut bytes = [0u8; 8192];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(remaining))?;
        let capacity = bytes.len().min(limit.saturating_sub(line.len()));
        if capacity == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IPC line too large",
            ));
        }
        let count = match stream.read(&mut bytes[..capacity]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if let Some(end) = bytes[..count].iter().position(|b| *b == b'\n') {
            line.extend_from_slice(&bytes[..=end]);
            return Ok(line);
        }
        line.extend_from_slice(&bytes[..count]);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    #[test]
    fn fragmented_line_and_size_limit() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let task = std::thread::spawn(move || {
            writer.write_all(b"FO").unwrap();
            std::thread::sleep(Duration::from_millis(10));
            writer.write_all(b"CUS\n").unwrap();
        });
        assert_eq!(read_line(&mut reader, 16, TIMEOUT).unwrap(), b"FOCUS\n");
        task.join().unwrap();
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"123456789").unwrap();
        assert_eq!(
            read_line(&mut reader, 8, TIMEOUT).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn byte_trickle_cannot_extend_deadline_forever() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let task = std::thread::spawn(move || {
            for _ in 0..20 {
                if writer.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let start = Instant::now();
        assert!(read_line(&mut reader, 1024, Duration::from_millis(60)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        drop(reader);
        task.join().unwrap();
    }
}
