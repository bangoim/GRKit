//! The GRKit service (`grkit_serve`): `SERVE.BIN` on the card.
//!
//! `SCAN.SH` starts the service from the card, because the camera's `/tmp`
//! does not run programs on every camera (GR IV mounts it without exec). The
//! service then runs a copy of itself from RAM (`run_from_ram`), so that no
//! file on the card stays open while it runs.

use grkit_serve::{
    head_end, parse, read_head, respond,
    update::{Card, Device, MAX_PACKAGE},
    Connection, Env, Response, System, PORT,
};
#[cfg(target_os = "linux")]
use std::{
    fs::File,
    os::{
        fd::{FromRawFd, IntoRawFd},
        unix::process::CommandExt,
    },
    process::Command,
};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    process::ExitCode,
    thread,
    time::Duration,
};

/// How long the service waits for a slow peer. It serves one connection at a
/// time, so a peer that stops sending must not hold it.
const PEER_TIMEOUT: Duration = Duration::from_secs(5);
/// The pause after a failed `accept`, so that a lasting failure (for example,
/// no free file descriptor) does not keep the CPU busy.
const ACCEPT_PAUSE: Duration = Duration::from_millis(500);
/// The argument of the copy that runs from RAM.
const FROM_RAM: &str = "--from-ram";

fn main() -> ExitCode {
    // Elsewhere, such as on a development computer, the service runs in place.
    if cfg!(target_os = "linux") && std::env::args().nth(1).as_deref() != Some(FROM_RAM) {
        // `run_from_ram` returns only if it fails. The service then stops,
        // because a service that runs from the card keeps its file open.
        let _ = run_from_ram();
        return ExitCode::FAILURE;
    }
    let Ok(listener) = TcpListener::bind(("0.0.0.0", PORT)) else {
        return ExitCode::FAILURE;
    };
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                // An error ends only this connection.
                let _ = serve(&mut stream);
            }
            Err(_) => thread::sleep(ACCEPT_PAUSE),
        }
    }
}

/// Copies this program into a memory file and runs the copy in place of this
/// process, with the same process ID. Returns only if a step fails.
#[cfg(target_os = "linux")]
fn run_from_ram() -> io::Result<()> {
    let program = std::fs::read("/proc/self/exe")?;
    // SAFETY: the name is a NUL-terminated string, and the flags are valid.
    // The exec opens the file by its path before it closes the descriptor.
    let fd = unsafe { libc::memfd_create(c"grkit-serve".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(&program)?;
    // The copy runs from the descriptor, which stays open until the exec.
    let fd = file.into_raw_fd();
    Err(Command::new(format!("/proc/self/fd/{fd}"))
        .arg0("grkit-serve")
        .arg(FROM_RAM)
        .exec())
}

#[cfg(not(target_os = "linux"))]
fn run_from_ram() -> io::Result<()> {
    unreachable!("the service runs in place")
}

fn serve(stream: &mut TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(PEER_TIMEOUT))?;
    stream.set_write_timeout(Some(PEER_TIMEOUT))?;
    let connection = Connection {
        local: stream.local_addr()?.to_string(),
        peer: stream.peer_addr()?.to_string(),
    };
    let mut bytes = read_head(stream)?;
    let response = match parse(&bytes) {
        None => Response::error(400, "bad request"),
        Some(request) if request.length > MAX_PACKAGE => {
            Response::error(413, "the package is too large")
        }
        Some(request) => {
            // The head may have brought the first bytes of the body.
            let mut body = bytes.split_off(head_end(&bytes).unwrap_or(bytes.len()));
            body.truncate(request.length);
            let rest = request.length - body.len();
            Read::by_ref(stream)
                .take(rest as u64)
                .read_to_end(&mut body)?;
            if body.len() < request.length {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            let card = Card::new(grkit_core::card::LINUX_CARD);
            let env = Env {
                files: &System,
                connection: &connection,
                card: &card,
                device: &device,
            };
            respond(&request, &body, &env)
        }
    };
    stream.write_all(&response.to_bytes())?;
    stream.shutdown(Shutdown::Both)
}

/// The camera, as its own HTTP API on port 80 describes it, and the free
/// space of the card.
fn device() -> anyhow::Result<Device> {
    use anyhow::Context;
    let mut stream = TcpStream::connect_timeout(&([127, 0, 0, 1], 80).into(), PEER_TIMEOUT)
        .context("The camera's Wi-Fi API is not ready")?;
    stream.set_read_timeout(Some(PEER_TIMEOUT))?;
    stream.write_all(b"GET /v1/props HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
    let mut answer = Vec::new();
    // The API may keep the connection open after its answer: stop at the end
    // of the JSON body.
    let mut buffer = [0; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => answer.extend_from_slice(&buffer[..count]),
            Err(error) if answer.is_empty() => return Err(error.into()),
            Err(_) => break,
        }
        if let Some(at) = head_end(&answer) {
            if serde_json::from_slice::<serde_json::Value>(&answer[at..]).is_ok() {
                break;
            }
        }
    }
    let at = head_end(&answer).context("The camera's Wi-Fi API gave no answer")?;
    let props: serde_json::Value =
        serde_json::from_slice(&answer[at..]).context("Read the camera's properties")?;
    let text = |key: &str| {
        props[key]
            .as_str()
            .map(str::to_owned)
            .with_context(|| format!("The camera's properties have no {key}"))
    };
    Ok(Device {
        model: text("model")?,
        firmware: text("firmwareVersion")?,
        free_bytes: free_bytes(grkit_core::card::LINUX_CARD),
    })
}

/// The free space of the file system at `path`.
#[cfg(target_os = "linux")]
fn free_bytes(path: &str) -> Option<u64> {
    let path = std::ffi::CString::new(path).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a NUL-terminated string and `stats` has room for the
    // result, which is read only on success.
    let stats = unsafe {
        if libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) != 0 {
            return None;
        }
        stats.assume_init()
    };
    Some(u64::from(stats.f_bavail) * u64::from(stats.f_frsize))
}

#[cfg(not(target_os = "linux"))]
fn free_bytes(_path: &str) -> Option<u64> {
    None
}
