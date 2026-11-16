//! Bounded HTTP transport admission. This layer never writes camera state.
use crate::{protocol::valid_request_id, Request, Response};
use std::{
    io::{self, Read},
    net::{IpAddr, SocketAddr, TcpStream},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub const MAX_CONNECTIONS: usize = 4;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
// A separate total bound prevents a peer from keeping a worker forever by
// sending one byte before each idle timeout. Measure this on physical Wi-Fi.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);

pub fn admit(request: &Request, local: SocketAddr) -> Result<(), Response> {
    let host = request
        .headers
        .get("host")
        .ok_or_else(|| Response::error(400, "Host is required"))?;
    let address = host.parse::<SocketAddr>().ok().or_else(|| {
        host.parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, 80))
    });
    if address != Some(local) {
        return Err(Response::error(
            403,
            "Host must match the local IP address and port",
        ));
    }
    if request.headers.contains_key("origin") {
        return Err(Response::error(403, "Browser requests are not allowed"));
    }
    if request.method != "GET" {
        let id = request.headers.get("x-grkit-request-id");
        if !id.is_some_and(|id| valid_request_id(id)) {
            return Err(Response::error(400, "X-GRKit-Request-Id must be a UUID"));
        }
    }
    let body_type = match (request.method.as_str(), request.path.as_str()) {
        ("PUT", "/v1/update") => Some((crate::update::MAX_PACKAGE, "application/zip")),
        ("POST", "/v1/update/cancel" | "/v1/rollback" | "/v1/rollback/cancel") => {
            Some((1024, "application/json"))
        }
        ("PUT", "/v1/params/looks/order") => Some((4096, "application/json")),
        ("PUT", path)
            if path
                .strip_prefix("/v1/looks/")
                .and_then(|path| path.strip_suffix("/content"))
                .is_some_and(grkit_core::effects::valid_id) =>
        {
            if !request.headers.contains_key("content-length") {
                return Err(Response::error(400, "Content-Length is required"));
            }
            if !matches!(request.length, 8192 | 8704) {
                return Err(Response::error(422, "Unsupported GRF length"));
            }
            Some((8704, "application/octet-stream"))
        }
        ("PATCH", path)
            if path
                .strip_prefix("/v1/looks/")
                .is_some_and(grkit_core::effects::valid_id) =>
        {
            Some((1024, "application/json"))
        }
        _ => None,
    };
    if let Some((limit, content_type)) = body_type {
        if request.length > limit {
            return Err(Response::error(
                413,
                "The body exceeds this resource's limit",
            ));
        }
        let supplied = request
            .headers
            .get("content-type")
            .map(|value| value.split(';').next().unwrap_or("").trim());
        if supplied != Some(content_type) {
            return Err(Response::error(415, "Unsupported request media type"));
        }
    } else if request.length > 0 {
        return Err(Response::error(
            400,
            "This endpoint does not accept a request body",
        ));
    }
    if request.headers.contains_key("expect") {
        return Err(Response::error(417, "Expect is not supported"));
    }
    Ok(())
}

/// A slot is released even if a connection exits early or a worker panics.
pub struct ConnectionSlot(Arc<AtomicUsize>);
impl ConnectionSlot {
    pub fn acquire(count: &Arc<AtomicUsize>) -> Option<Self> {
        let mut current = count.load(Ordering::Acquire);
        loop {
            if current >= MAX_CONNECTIONS {
                return None;
            }
            match count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self(Arc::clone(count))),
                Err(actual) => current = actual,
            }
        }
    }
}
impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Apply the total deadline to each read, including the request head.
pub struct DeadlineReader<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}
impl<'a> DeadlineReader<'a> {
    pub fn new(stream: &'a mut TcpStream) -> Self {
        Self {
            stream,
            deadline: Instant::now() + TRANSFER_TIMEOUT,
        }
    }
}
impl Read for DeadlineReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|time| !time.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "transfer deadline exceeded"))?;
        self.stream
            .set_read_timeout(Some(remaining.min(IDLE_TIMEOUT)))?;
        self.stream.read(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    fn accepted(headers: &str, method: &str) -> bool {
        let request =
            parse(format!("{method} /v1/ping HTTP/1.1\r\n{headers}\r\n").as_bytes()).unwrap();
        admit(&request, "192.168.0.1:8080".parse().unwrap()).is_ok()
    }
    #[test]
    fn rejects_browser_and_rebinding_requests() {
        assert!(accepted("Host: 192.168.0.1:8080\r\n", "GET"));
        assert!(!accepted("Host: camera.example:8080\r\n", "GET"));
        assert!(!accepted("Host: 192.168.0.2:8080\r\n", "GET"));
        assert!(!accepted(
            "Host: 192.168.0.1:8080\r\nOrigin: null\r\n",
            "GET"
        ));
        assert!(!accepted("", "GET"));
    }
    #[test]
    fn all_mutations_require_a_request_id() {
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            assert!(!accepted("Host: 192.168.0.1:8080\r\n", method));
            assert!(accepted("Host: 192.168.0.1:8080\r\nX-GRKit-Request-Id: 550e8400-e29b-41d4-a716-446655440000\r\n", method));
        }
    }
    #[test]
    fn connection_slots_are_bounded_and_released() {
        let count = Arc::new(AtomicUsize::new(0));
        let slots: Vec<_> = (0..MAX_CONNECTIONS)
            .map(|_| ConnectionSlot::acquire(&count).unwrap())
            .collect();
        assert!(ConnectionSlot::acquire(&count).is_none());
        drop(slots);
        assert!(ConnectionSlot::acquire(&count).is_some());
    }
    #[test]
    fn total_deadline_expires_even_when_idle_timeout_has_not() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let mut reader = DeadlineReader {
            stream: &mut stream,
            deadline: Instant::now(),
        };
        assert_eq!(
            reader.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
}
