use crate::{Result, err};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::Path,
    time::Duration,
};
pub(crate) fn connect_timeout(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid Unix socket path",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = *byte as libc::c_char;
    }
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let socket = unsafe { UnixStream::from_raw_fd(raw) };
    // Linux AF_UNIX connect uses SO_SNDTIMEO while waiting for listener backlog space.
    socket.set_write_timeout(Some(timeout))?;
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    if unsafe {
        libc::connect(
            socket.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length as libc::socklen_t,
        )
    } < 0
    {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Unix socket connection timed out",
            ));
        }
        return Err(error);
    }
    Ok(socket)
}
pub fn request(request: Value) -> Result<Value> {
    let path = std::env::var_os("NIRI_SOCKET").ok_or_else(|| err("NIRI_SOCKET is unset"))?;
    let mut socket = connect_timeout(Path::new(&path), Duration::from_secs(2))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    serde_json::to_writer(&mut socket, &request)?;
    socket.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(socket)
        .take(4 * 1024 * 1024)
        .read_line(&mut line)?;
    decode(&line)
}
pub fn decode(line: &str) -> Result<Value> {
    let reply: Value = serde_json::from_str(line)?;
    if let Some(value) = reply.get("Ok") {
        Ok(value.clone())
    } else {
        Err(err(format!(
            "Niri IPC: {}",
            reply.get("Err").unwrap_or(&reply)
        )))
    }
}
pub fn action(action: Value) -> Result<()> {
    request(json!({"Action":action}))?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replies_reject_errors() {
        assert_eq!(
            decode(r#"{"Ok":{"Outputs":{}}}"#).unwrap(),
            json!({"Outputs":{}})
        );
        assert!(decode(r#"{"Err":"bad action"}"#).is_err());
        assert!(decode("garbage").is_err());
    }
    #[test]
    fn full_unix_backlog_times_out_before_server_accepts() {
        use std::{os::fd::AsRawFd, os::unix::net::UnixListener, sync::mpsc, time::Instant};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("niri.sock");
        let listener = UnixListener::bind(&path).unwrap();
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let _queued = UnixStream::connect(&path).unwrap();
        let (done, release) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            // A regressed blocking connect is released rather than hanging the test.
            if release.recv_timeout(Duration::from_millis(500)).is_err() {
                let _ = listener.accept();
            }
        });
        let began = Instant::now();
        let result = connect_timeout(&path, Duration::from_millis(60));
        let elapsed = began.elapsed();
        let _ = done.send(());
        watchdog.join().unwrap();
        let error = result.expect_err("A full listener backlog must not wait indefinitely");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            elapsed < Duration::from_millis(400),
            "connect took {elapsed:?}"
        );
    }

    #[test]
    fn bounded_connect_preserves_socket_io_non_utf8_paths_and_cloexec() {
        use std::{
            ffi::OsString,
            os::unix::{ffi::OsStringExt, net::UnixListener},
        };
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .join(OsString::from_vec(b"niri-\xff.sock".to_vec()));
        let listener = UnixListener::bind(&path).unwrap();
        let mut socket = connect_timeout(&path, Duration::from_millis(60)).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        socket.write_all(b"request\n").unwrap();
        let mut data = [0; 8];
        peer.read_exact(&mut data).unwrap();
        assert_eq!(&data, b"request\n");
        assert_ne!(
            unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert_eq!(
            connect_timeout(Path::new("bad\0path"), Duration::from_millis(60))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}
