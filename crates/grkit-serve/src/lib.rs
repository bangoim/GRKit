//! The GRKit service on the camera's Linux: a small HTTP server that a phone
//! reaches over the camera's Wi-Fi. `SCAN.SH` starts it once at each
//! power-on (`grkit_core::runtime::package_runtime_files_with`).
//!
//! The service answers one request on each connection and then closes it. It
//! admits a bounded number of connections and waits in `accept` between them. Every path starts with `/v<PROTOCOL>/`. The port has no
//! other service, so the paths do not name GRKit.
//!
//! - `GET status`: the GRKit version, the Linux uptime, and the boot log
//!   lines of this start. Its `update` field is null when the package folder
//!   on the card cannot be read yet.
//! - `GET diag`: the addresses of the connection and the kernel's network,
//!   memory, and process files, as text.
//! - `GET package`: the installed package's version, modules, effects, and
//!   left-out effects from `PACKAGE.TXT`, and the camera model and firmware
//!   when the camera's API answers (else null).
//! - `PUT update`, with a package ZIP that a release signed: checks it and
//!   writes an update that waits for the confirmation on the camera, then
//!   installs at the next power-on (`update`). `GET update` gives its state
//!   and the version that a rollback returns to, and `DELETE update` removes
//!   it.
//! - `PUT rollback`: writes the rollback of the last update, which waits for
//!   the confirmation on the camera as an update does.
//!
//! Soon after power-on the card may not be readable yet. Then `GET update`,
//! `GET package`, `PUT update`, and `PUT rollback` answer 503 and the client
//! can retry. That is not the same as no update (`state` `"none"`).
//!
//! The service takes no `POST`: a web page can send a `POST` to another
//! address without the browser asking the service first, but not a `PUT`.

pub mod api;
pub mod changes;
pub mod control;
pub mod identity;
pub mod library;
pub mod look_api;
pub mod looks;
pub mod protocol;
pub mod runtime_backend;
pub mod transport;
pub mod trust;
pub mod update;

use grkit_core::{card::LINUX_DATA_DIRECTORY, runtime::BOOT_LOG};
use serde_json::{json, Map, Value};
use std::io::{self, Read};

/// The TCP port of the service. The camera's own HTTP API uses port 80.
pub const PORT: u16 = 8080;
/// The version of the protocol, which every path names.
pub const PROTOCOL: u32 = 1;
/// The largest request head that the service reads.
pub const MAX_HEAD: usize = 8 * 1024;
/// The most bytes of one file that a response holds.
pub const MAX_FILE: usize = 64 * 1024;
/// The most boot log lines of this start that `status` gives.
const MAX_BOOT_LINES: usize = 50;

/// The kernel files that `diag` gives: the mounts, the interfaces,
/// addresses, routes, sockets, and neighbors of the network, and the load and
/// the memory of the system and of the service.
pub const DIAG_FILES: [&str; 13] = [
    "/proc/version",
    "/proc/mounts",
    "/proc/uptime",
    "/proc/loadavg",
    "/proc/meminfo",
    "/proc/self/stat",
    "/proc/self/status",
    "/proc/net/dev",
    "/proc/net/route",
    "/proc/net/fib_trie",
    "/proc/net/tcp",
    "/proc/net/udp",
    "/proc/net/arp",
];

/// The files that the service reads. The camera reads the real files; the
/// tests give their own.
pub trait Files {
    /// The text of the file at `path`, at most `MAX_FILE` bytes, or `None`
    /// if the file cannot be read.
    fn read(&self, path: &str) -> Option<String>;
}

/// The files of the running system.
pub struct System;

impl Files for System {
    fn read(&self, path: &str) -> Option<String> {
        let file = std::fs::File::open(path).ok()?;
        let mut bytes = Vec::new();
        file.take(MAX_FILE as u64).read_to_end(&mut bytes).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// The two ends of a connection, as the service saw them.
pub struct Connection {
    pub local: String,
    pub peer: String,
}

/// A request that the service accepts: the method, the path without the
/// query, its query and headers, and the length of its body.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub length: usize,
    pub query: Option<String>,
    pub headers: std::collections::BTreeMap<String, String>,
}

/// What a request can change and read besides the files: the card and the
/// camera (`update::Device`), which the camera's own HTTP API describes.
pub struct Env<'a, F: Files> {
    pub files: &'a F,
    pub connection: &'a Connection,
    pub card: &'a update::Card,
    pub device: &'a dyn Fn() -> anyhow::Result<update::Device>,
}

/// A response: the status and the JSON body.
#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Value,
}

impl Response {
    pub fn error(status: u16, message: &str) -> Self {
        Response {
            status,
            body: serde_json::to_value(protocol::Problem::new(status, error_code(status), message))
                .expect("problem serializes"),
        }
    }

    /// The bytes of the response on the connection, which the service closes
    /// after them.
    pub fn to_bytes(&self) -> Vec<u8> {
        let body = self.body.to_string();
        let reason = reason(self.status);
        let content_type = if self.status >= 400 {
            "application/problem+json"
        } else {
            "application/json"
        };
        format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {content_type}\r\n\
             Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            self.status,
            body.len()
        )
        .into_bytes()
    }
}

pub(crate) fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        417 => "Expectation Failed",
        422 => "Unprocessable Content",
        428 => "Precondition Required",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Error",
    }
}
fn error_code(status: u16) -> &'static str {
    match status {
        400 => "invalid_request",
        403 => "access_denied",
        404 => "not_found",
        405 => "method_not_allowed",
        409 => "conflict",
        413 => "body_too_large",
        417 => "unsupported_expectation",
        503 => "unavailable",
        _ => "request_failed",
    }
}

/// Reads the request head from `stream`: the bytes up to and with the empty
/// line, at most `MAX_HEAD` bytes. Preserve any body prefix read with the head.
pub fn read_head(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut buffer = [0; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if head.len() >= MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too long"));
        }
        let count = stream.read(&mut buffer[..(MAX_HEAD - head.len()).min(1024)])?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.extend_from_slice(&buffer[..count]);
    }
    Ok(head)
}

/// The end of the head in `bytes`, after its empty line.
pub fn head_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at + 4)
}

/// The request of a request head, or `None` if the request line is not
/// `<method> <path> HTTP/1.x` or its `Content-Length` is not a number.
pub fn parse(head: &[u8]) -> Option<Request> {
    let text = std::str::from_utf8(&head[..head_end(head).unwrap_or(head.len())]).ok()?;
    let mut lines = text.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return None;
    }
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || target.contains('#')
        || target.bytes().any(|c| c <= 32 || c == 127)
        || !method.bytes().all(|c| c.is_ascii_uppercase())
    {
        return None;
    }
    let mut headers = std::collections::BTreeMap::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':')?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
            || value.bytes().any(|c| (c < 32 && c != b'\t') || c == 127)
        {
            return None;
        }
        // Reject ambiguous framing and duplicated admission headers.
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return None;
        }
    }
    if headers.contains_key("transfer-encoding") {
        return None;
    }
    let length = match headers.get("content-length") {
        Some(value) if !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) => {
            value.parse().ok()?
        }
        Some(_) => return None,
        None => 0,
    };
    let (path, query) = target
        .split_once('?')
        .map_or((target, None), |(p, q)| (p, Some(q.to_owned())));
    Some(Request {
        method: method.to_owned(),
        path: path.to_owned(),
        length,
        query,
        headers,
    })
}

/// The response to `request`, whose body is `body`.
pub fn respond(request: &Request, body: &[u8], env: &Env<impl Files>) -> Response {
    let prefix = format!("/v{PROTOCOL}/");
    let Some(route) = request.path.strip_prefix(&prefix) else {
        return Response::error(404, "not found");
    };
    let ok = |body| Response { status: 200, body };
    // A card that is not ready answers 503, so the client tries again;
    // another error answers `status`.
    let failed = |error: anyhow::Error, status: u16| {
        let status = if update::card_not_ready(&error) {
            503
        } else {
            status
        };
        Response::error(status, &format!("{error:#}"))
    };
    match (route, request.method.as_str()) {
        ("ping", "GET") => ok(json!({ "protocol": PROTOCOL })),
        ("status", "GET") => ok(status(env)),
        ("diag" | "debug/diagnostics", "GET") => {
            let mut body = diag(env.files, env.connection);
            let trusted =
                trust::installed_release(env.card.root(), &grkit_core::release::OFFICIAL_KEYS)
                    .map(|release| release.official_looks.into_values().collect())
                    .unwrap_or_default();
            body["lookLibrary"] = match looks::inventory(env.card.root(), &trusted) {
                Ok(inventory) => inventory,
                Err(error) => json!({"available": false, "detail": format!("{error:#}")}),
            };
            ok(body)
        }
        ("package", "GET") => match env.card.package() {
            Ok(mut body) => {
                let camera = match (env.device)() {
                    Ok(device) => json!({
                        "model": device.model,
                        "firmware": device.firmware,
                    }),
                    Err(_) => Value::Null,
                };
                body.as_object_mut()
                    .expect("package body")
                    .insert("camera".into(), camera);
                ok(body)
            }
            Err(error) => failed(error, 500),
        },
        ("update", "GET") => match update_state(env.card) {
            Ok(body) => ok(body),
            Err(error) => failed(error, 500),
        },
        ("update", "PUT") => {
            if body.len() > update::MAX_PACKAGE {
                return Response::error(413, "the package is too large");
            }
            if let Err(error) = env.card.ensure_readable() {
                return failed(error, 500);
            }
            let device = match (env.device)() {
                Ok(device) => device,
                Err(error) => return Response::error(503, &format!("{error:#}")),
            };
            match update::stage(env.card, body, &device, &grkit_core::release::OFFICIAL_KEYS) {
                Ok(summary) => ok(summary),
                Err(error) => failed(error, 400),
            }
        }
        ("update", "DELETE") => match env.card.cancel() {
            Ok(cancelled) => ok(json!({ "state": "none", "cancelled": cancelled })),
            Err(error) => failed(error, 500),
        },
        ("rollback", "PUT") => match update::stage_rollback(env.card) {
            Ok(summary) => ok(summary),
            Err(error) => failed(error, 409),
        },
        (
            "ping" | "status" | "diag" | "debug/diagnostics" | "package" | "update" | "rollback",
            _,
        ) => Response::error(405, "method not allowed"),
        _ => Response::error(404, "not found"),
    }
}

fn status(env: &Env<impl Files>) -> Value {
    let files = env.files;
    let uptime = files
        .read("/proc/uptime")
        .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok());
    let boot = files
        .read(&format!("{LINUX_DATA_DIRECTORY}/{BOOT_LOG}"))
        .map(|text| json!(this_start(&text)));
    // Null when the package folder cannot be read yet; not an update state.
    let update = update_state(env.card).unwrap_or(Value::Null);
    json!({
        "protocol": PROTOCOL,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime": uptime,
        "boot": boot,
        "update": update,
    })
}

/// The state of the update, and the version that a rollback of the last
/// update returns to, or null without one. An error means the package folder
/// cannot be read.
fn update_state(card: &update::Card) -> anyhow::Result<Value> {
    Ok(json!({
        "state": card.state()?,
        "rollback": card.rollback()?.map(|rollback| json!({ "version": rollback.version })),
    }))
}

/// The boot log lines of this start: those after the last `load` line, which
/// the first `SCAN.SH` of a power-on writes. At most `MAX_BOOT_LINES`, the
/// last ones.
fn this_start(log: &str) -> Vec<&str> {
    let lines: Vec<&str> = log.lines().collect();
    let first = lines
        .iter()
        .rposition(|line| line.split_whitespace().nth(1) == Some("load"))
        .unwrap_or(0);
    let start = &lines[first..];
    start[start.len().saturating_sub(MAX_BOOT_LINES)..].to_vec()
}

fn diag(files: &impl Files, connection: &Connection) -> Value {
    let texts: Map<String, Value> = DIAG_FILES
        .iter()
        .map(|path| ((*path).to_owned(), json!(files.read(path))))
        .collect();
    json!({
        "connection": { "local": connection.local, "peer": connection.peer },
        "files": texts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Fake(BTreeMap<String, String>);

    impl Files for Fake {
        fn read(&self, path: &str) -> Option<String> {
            self.0.get(path).cloned()
        }
    }

    fn fake(entries: &[(&str, &str)]) -> Fake {
        Fake(
            entries
                .iter()
                .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
                .collect(),
        )
    }

    fn connection() -> Connection {
        Connection {
            local: "192.168.0.1:8080".into(),
            peer: "192.168.0.2:50000".into(),
        }
    }

    /// The response to `head` and `body` with `files`, on a card with no
    /// package folder and a camera that does not answer.
    fn request(head: &str, body: &[u8], files: &Fake) -> Response {
        let dir = tempfile::tempdir().unwrap();
        let card = update::Card::new(dir.path());
        request_on(&card, head, body, files, || anyhow::bail!("no camera"))
    }

    fn request_on(
        card: &update::Card,
        head: &str,
        body: &[u8],
        files: &Fake,
        device: impl Fn() -> anyhow::Result<update::Device>,
    ) -> Response {
        let env = Env {
            files,
            connection: &connection(),
            card,
            device: &device,
        };
        match parse(head.as_bytes()) {
            Some(request) => respond(&request, body, &env),
            None => Response::error(400, "bad request"),
        }
    }

    fn get(path: &str, files: &Fake) -> Response {
        request(
            &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"),
            b"",
            files,
        )
    }

    /// A card with a signed test package, as `update` tests build one.
    fn card_with_package() -> (tempfile::TempDir, update::Card) {
        use grkit_core::{camera, packaging, testing};
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["test-effect"]);
        let dir = tempfile::tempdir().unwrap();
        for (path, bytes) in packaging::card_files(&zip).unwrap() {
            let target = dir.path().join(&path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, bytes).unwrap();
        }
        let card = update::Card::new(dir.path());
        (dir, card)
    }

    #[test]
    fn a_head_ends_at_the_empty_line() {
        let mut input: &[u8] = b"GET / HTTP/1.1\r\nHost: x\r\n\r\nbody";
        let head = read_head(&mut input).unwrap();
        assert!(head.starts_with(b"GET / HTTP/1.1\r\n"));
        assert!(head.windows(4).any(|w| w == b"\r\n\r\n"));
    }

    #[test]
    fn a_head_without_its_end_is_an_error() {
        let mut short: &[u8] = b"GET / HTTP/1.1\r\n";
        assert_eq!(
            read_head(&mut short).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let long = vec![b'a'; MAX_HEAD + 10];
        assert_eq!(
            read_head(&mut long.as_slice()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn the_request_line_gives_the_method_and_the_path() {
        assert_eq!(
            parse(b"GET /v1/status?x=1 HTTP/1.1\r\n\r\n"),
            Some(Request {
                method: "GET".into(),
                path: "/v1/status".into(),
                length: 0,
                query: Some("x=1".into()),
                headers: Default::default(),
            })
        );
        assert_eq!(
            parse(b"PUT /v1/update HTTP/1.1\r\ncontent-length: 12\r\n\r\nabc")
                .unwrap()
                .length,
            12
        );
        assert_eq!(parse(b"PUT /a HTTP/1.1\r\nContent-Length: x\r\n\r\n"), None);
        assert_eq!(parse(b"GET /a HTTP/2\r\n\r\n"), None);
        assert_eq!(parse(b"GET a HTTP/1.1\r\n\r\n"), None);
        assert_eq!(parse(b"GET /a HTTP/1.1 x\r\n\r\n"), None);
        assert_eq!(parse(b"\xff\xfe\r\n\r\n"), None);
    }

    #[test]
    fn status_gives_the_version_the_uptime_and_this_start() {
        let files = fake(&[
            ("/proc/uptime", "12.50 20.00\n"),
            (
                "/media/storage1/GRKIT/BOOT.TXT",
                "1.0 load\n1.1 scan 0\n2.0 load\n2.1 scan-start\n2.2 scan 0\n",
            ),
        ]);
        let response = get("/v1/status", &files);
        assert_eq!(response.status, 200);
        assert_eq!(response.body["protocol"], 1);
        assert_eq!(response.body["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(response.body["uptime"], 12.5);
        assert_eq!(
            response.body["boot"],
            json!(["2.0 load", "2.1 scan-start", "2.2 scan 0"])
        );
    }

    #[test]
    fn status_without_the_files_gives_null() {
        let response = get("/v1/status", &fake(&[]));
        assert_eq!(response.status, 200);
        assert!(response.body["uptime"].is_null());
        assert!(response.body["boot"].is_null());
    }

    #[test]
    fn status_keeps_the_last_boot_lines() {
        let log: String = std::iter::once("0.0 load\n".to_owned())
            .chain((1..=80).map(|n| format!("{n}.0 step\n")))
            .collect();
        let files = fake(&[("/media/storage1/GRKIT/BOOT.TXT", &log)]);
        let boot = get("/v1/status", &files).body["boot"].clone();
        let boot = boot.as_array().unwrap();
        assert_eq!(boot.len(), MAX_BOOT_LINES);
        assert_eq!(boot.last().unwrap(), "80.0 step");
    }

    #[test]
    fn diag_gives_the_connection_and_each_file() {
        let files = fake(&[("/proc/net/dev", "lo: 0\n")]);
        let response = get("/v1/diag", &files);
        assert_eq!(response.status, 200);
        assert_eq!(response.body["connection"]["local"], "192.168.0.1:8080");
        assert_eq!(response.body["connection"]["peer"], "192.168.0.2:50000");
        let texts = response.body["files"].as_object().unwrap();
        assert_eq!(texts.len(), DIAG_FILES.len());
        assert_eq!(texts["/proc/net/dev"], "lo: 0\n");
        assert!(texts["/proc/net/arp"].is_null());
    }

    #[test]
    fn ambiguous_or_unsupported_framing_is_rejected() {
        for headers in [
            "Content-Length: 1\r\nContent-Length: 2\r\n",
            "Transfer-Encoding: chunked\r\n",
            "Content-Length: +1\r\n",
            "Host: one\r\nhost: two\r\n",
            "Host : invalid\r\n",
            " folded: invalid\r\n",
            "Bad header\r\n",
        ] {
            assert!(
                parse(format!("PUT /v1/update HTTP/1.1\r\n{headers}\r\n").as_bytes()).is_none(),
                "{headers}"
            );
        }
        assert!(parse(b"GET /v1/ping#fragment HTTP/1.1\r\n\r\n").is_none());
        assert!(parse(b"GET /v1/ping HTTP/1.9\r\n\r\n").is_none());
    }

    #[test]
    fn other_requests_get_an_error() {
        let files = fake(&[]);
        assert_eq!(get("/", &files).status, 404);
        assert_eq!(get("/v2/status", &files).status, 404);
        assert_eq!(get("/v1/other", &files).status, 404);
        let post = request("POST /v1/status HTTP/1.1\r\n\r\n", b"", &files);
        assert_eq!(post.status, 405);
        let bad = request("nonsense\r\n\r\n", b"", &files);
        assert_eq!(bad.status, 400);
    }

    #[test]
    fn an_update_needs_the_camera_and_a_package() {
        let files = fake(&[]);
        let (_dir, card) = card_with_package();
        assert_eq!(
            request_on(
                &card,
                "GET /v1/update HTTP/1.1\r\n\r\n",
                b"",
                &files,
                || anyhow::bail!("no camera")
            )
            .body,
            json!({ "state": "none", "rollback": null })
        );
        // Without an update before, no rollback.
        let rollback = request_on(
            &card,
            "PUT /v1/rollback HTTP/1.1\r\n\r\n",
            b"",
            &files,
            || anyhow::bail!("no camera"),
        );
        assert_eq!(rollback.status, 409);
        assert_eq!(
            request("GET /v1/rollback HTTP/1.1\r\n\r\n", b"", &files).status,
            405
        );
        // The paths name the protocol, not GRKit.
        assert_eq!(get("/grkit/v1/update", &files).status, 404);
        // Without an answer of the camera's API, no update starts.
        let put = request_on(
            &card,
            "PUT /v1/update HTTP/1.1\r\n\r\n",
            b"zip",
            &files,
            || anyhow::bail!("no camera"),
        );
        assert_eq!(put.status, 503);
        assert_eq!(put.body["detail"], "no camera");
        let large = vec![0; update::MAX_PACKAGE + 1];
        let put = request_on(
            &card,
            "PUT /v1/update HTTP/1.1\r\n\r\n",
            &large,
            &files,
            || anyhow::bail!("no camera"),
        );
        assert_eq!(put.status, 413);
        let post = request("POST /v1/update HTTP/1.1\r\n\r\n", b"zip", &files);
        assert_eq!(post.status, 405);
        let delete = request_on(
            &card,
            "DELETE /v1/update HTTP/1.1\r\n\r\n",
            b"",
            &files,
            || anyhow::bail!("no camera"),
        );
        assert_eq!(delete.body, json!({ "state": "none", "cancelled": false }));
        assert_eq!(
            request_on(
                &card,
                "GET /v1/status HTTP/1.1\r\n\r\n",
                b"",
                &files,
                || anyhow::bail!("no camera")
            )
            .body["update"]["state"],
            "none"
        );
    }

    #[test]
    fn package_gives_the_installed_modules_and_effects() {
        use grkit_core::{effects, modules};
        let files = fake(&[]);
        let (_dir, card) = card_with_package();
        let response = request_on(
            &card,
            "GET /v1/package HTTP/1.1\r\n\r\n",
            b"",
            &files,
            || {
                Ok(update::Device {
                    model: "RICOH GR III".into(),
                    firmware: "2.21".into(),
                    free_bytes: None,
                    serial_number: None,
                })
            },
        );
        assert_eq!(response.status, 200);
        assert_eq!(response.body["version"], "1.7.0");
        let expected_modules: Vec<_> = modules::OFFICIAL.iter().map(|m| m.id).collect();
        let mut got_modules: Vec<_> = response.body["modules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        got_modules.sort();
        let mut expected_modules = expected_modules;
        expected_modules.sort();
        assert_eq!(got_modules, expected_modules);
        let effect = effects::filename("test-effect").unwrap();
        assert_eq!(response.body["effects"], json!([effect]));
        assert_eq!(response.body["left_out"], json!([]));
        assert_eq!(response.body["camera"]["model"], "RICOH GR III");
        assert_eq!(response.body["camera"]["firmware"], "2.21");
        // A left-out line of PACKAGE.TXT shows in left_out.
        let list = _dir.path().join("SCRIPT/GRKIT/PACKAGE.TXT");
        let mut text = String::from_utf8(std::fs::read(&list).unwrap()).unwrap();
        text.push_str("-LEFT_OUT.GRF\n");
        std::fs::write(&list, text).unwrap();
        let again = request_on(
            &card,
            "GET /v1/package HTTP/1.1\r\n\r\n",
            b"",
            &files,
            || anyhow::bail!("no camera"),
        );
        assert_eq!(again.body["left_out"], json!(["LEFT_OUT.GRF"]));
        assert!(again.body["camera"].is_null());
        assert_eq!(
            request("POST /v1/package HTTP/1.1\r\n\r\n", b"", &files).status,
            405
        );
    }

    #[test]
    fn a_missing_package_folder_is_unavailable() {
        let files = fake(&[]);
        for (method, path) in [
            ("GET", "/v1/update"),
            ("GET", "/v1/package"),
            ("PUT", "/v1/rollback"),
            ("PUT", "/v1/update"),
        ] {
            let response = request(&format!("{method} {path} HTTP/1.1\r\n\r\n"), b"zip", &files);
            assert_eq!(response.status, 503, "{method} {path}");
            let error = response.body["detail"].as_str().unwrap();
            assert!(error.contains("The card is not ready"), "{error}");
            assert!(error.contains("Try again"), "{error}");
        }
        let status = get("/v1/status", &files);
        assert_eq!(status.status, 200);
        assert!(status.body["update"].is_null());
    }

    #[test]
    fn a_response_has_its_length_and_closes() {
        let bytes = Response::error(404, "not found").to_bytes();
        let text = String::from_utf8(bytes).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(head.contains(&format!("Content-Length: {}", body.len())));
        assert!(head.contains("Connection: close"));
        assert_eq!(
            serde_json::from_str::<Value>(body).unwrap()["detail"],
            "not found"
        );
    }
}
