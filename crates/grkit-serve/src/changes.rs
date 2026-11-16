//! Server-only change subscriptions. HTTP remains authoritative after reconnect.
use crate::protocol::{ChangeHub, NextChange, Problem, Watermark};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::{
    io::{self, Cursor, Read, Write},
    net::TcpStream,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tungstenite::{
    protocol::{frame::coding::CloseCode, CloseFrame, WebSocketConfig},
    Error, Message,
};

pub type SharedChanges = Arc<Mutex<ChangeHub<Value>>>;
pub const MAX_SUBSCRIBERS: usize = 2;
const READ_POLL: Duration = Duration::from_millis(100);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const PING_INTERVAL: Duration = Duration::from_secs(15);
const PONG_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_FRAME: usize = 1024;
const MAX_OUTPUT: usize = 16 * 1024;

#[derive(Clone)]
pub struct Changes {
    hub: SharedChanges,
    active: Arc<AtomicUsize>,
}

impl Changes {
    pub fn new(hub: SharedChanges) -> Self {
        Self {
            hub,
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Reserve before starting a dedicated WebSocket worker. Release the HTTP
    /// worker slot once that worker owns the connection. A failed spawn drops
    /// this value and releases both the subscriber and its capacity reservation.
    pub fn reserve(&self) -> Result<Subscription, Problem> {
        let mut count = self.active.load(Ordering::Acquire);
        loop {
            if count >= MAX_SUBSCRIBERS {
                return Err(Problem::new(
                    503,
                    "subscriber_capacity",
                    "The change subscription limit has been reached.",
                ));
            }
            match self.active.compare_exchange_weak(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => count = current,
            }
        }
        let registered = self
            .hub
            .lock()
            .map_err(|_| {
                Problem::new(
                    503,
                    "coordinator_unavailable",
                    "The runtime coordinator is unavailable.",
                )
            })
            .and_then(|mut hub| hub.subscribe());
        match registered {
            Ok((id, watermark)) => Ok(Subscription {
                hub: Arc::clone(&self.hub),
                active: Arc::clone(&self.active),
                id,
                watermark,
            }),
            Err(error) => {
                self.active.fetch_sub(1, Ordering::AcqRel);
                Err(error)
            }
        }
    }
}

pub struct Subscription {
    hub: SharedChanges,
    active: Arc<AtomicUsize>,
    id: u64,
    watermark: Watermark,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Ok(mut hub) = self.hub.lock() {
            hub.unsubscribe(self.id);
        }
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Subscription {
    /// `prefix` contains the complete, admitted HTTP head and any bytes read
    /// with it. Replaying these bytes preserves an early first WebSocket frame.
    pub fn serve(self, stream: TcpStream, prefix: Vec<u8>) -> Result<()> {
        if prefix.len() > crate::MAX_HEAD || crate::head_end(&prefix).is_none() {
            bail!("The WebSocket request head is incomplete or oversized");
        }
        let request = crate::parse(&prefix).context("Invalid WebSocket request")?;
        if request.method != "GET" || request.path != "/v1/changes" || request.length != 0 {
            bail!("Invalid change subscription endpoint");
        }
        crate::transport::admit(&request, stream.local_addr()?)
            .map_err(|response| anyhow!("WebSocket admission failed: {}", response.body))?;
        let config = WebSocketConfig::default()
            .read_buffer_size(2048)
            .write_buffer_size(0)
            .max_write_buffer_size(MAX_OUTPUT)
            .max_message_size(Some(MAX_FRAME))
            .max_frame_size(Some(MAX_FRAME));
        let io = SubscriptionIo::new(stream, prefix);
        let mut socket = tungstenite::accept_with_config(io, Some(config))
            .map_err(|error| anyhow!("WebSocket handshake failed: {error}"))?;
        socket.get_mut().begin_write();
        socket.send(Message::text(
            json!({
                "type": "hello", "bootId": self.watermark.boot_id,
                "cardId": self.watermark.card_id, "seq": self.watermark.snapshot_seq,
            })
            .to_string(),
        ))?;
        let mut next_ping = Instant::now() + PING_INTERVAL;
        let mut awaiting_pong = None;
        loop {
            // Never hold the coordinator lock across network I/O.
            let next = self
                .hub
                .lock()
                .map_err(|_| anyhow!("Runtime coordinator unavailable"))?
                .next(self.id);
            match next {
                NextChange::Event(event) => {
                    let text = serde_json::to_string(&event)?;
                    if text.len() + 16 > MAX_OUTPUT {
                        bail!("Change event exceeds socket output limit");
                    }
                    socket.get_mut().begin_write();
                    socket.send(Message::text(text))?;
                }
                NextChange::ResyncRequired | NextChange::UnknownSubscriber => {
                    socket.get_mut().begin_write();
                    socket.close(Some(CloseFrame {
                        code: CloseCode::Again,
                        reason: "Read fresh snapshots and subscribe again".into(),
                    }))?;
                    return Ok(());
                }
                NextChange::Empty => {}
            }
            let now = Instant::now();
            if awaiting_pong.is_some_and(|sent| now.duration_since(sent) >= PONG_TIMEOUT) {
                bail!("WebSocket heartbeat timed out");
            }
            if awaiting_pong.is_none() && now >= next_ping {
                socket.get_mut().begin_write();
                socket.send(Message::Ping(b"grkit".to_vec().into()))?;
                awaiting_pong = Some(now);
            }
            socket.get_mut().begin_read();
            match socket.read() {
                Ok(Message::Ping(_)) => {
                    socket.get_mut().begin_write();
                    socket.flush()?; // tungstenite queued the matching Pong.
                }
                Ok(Message::Pong(payload)) => {
                    if payload.as_ref() == b"grkit" && awaiting_pong.is_some() {
                        awaiting_pong = None;
                        next_ping = Instant::now() + PING_INTERVAL;
                    }
                }
                Ok(Message::Close(_)) => {
                    socket.get_mut().begin_write();
                    let _ = socket.flush();
                    return Ok(());
                }
                Ok(_) => {
                    socket.get_mut().begin_write();
                    socket.close(Some(CloseFrame {
                        code: CloseCode::Policy,
                        reason: "Use HTTP for application requests".into(),
                    }))?;
                    return Ok(());
                }
                Err(Error::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(Error::ConnectionClosed | Error::AlreadyClosed) => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
    }
}

/// Per-call deadlines also bound streams of tiny fragments or partial writes.
/// Tungstenite retains partial frames across a read timeout.
struct SubscriptionIo {
    stream: TcpStream,
    prefix: Cursor<Vec<u8>>,
    read_deadline: Instant,
    write_deadline: Instant,
}
impl SubscriptionIo {
    fn new(stream: TcpStream, prefix: Vec<u8>) -> Self {
        let now = Instant::now();
        Self {
            stream,
            prefix: Cursor::new(prefix),
            read_deadline: now + WRITE_TIMEOUT,
            write_deadline: now + WRITE_TIMEOUT,
        }
    }
    fn begin_read(&mut self) {
        self.read_deadline = Instant::now() + READ_POLL;
    }
    fn begin_write(&mut self) {
        self.write_deadline = Instant::now() + WRITE_TIMEOUT;
    }
    fn remaining(deadline: Instant) -> io::Result<Duration> {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "WebSocket I/O deadline exceeded")
            })
    }
}
impl Read for SubscriptionIo {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining = Self::remaining(self.read_deadline)?;
        if self.prefix.position() < self.prefix.get_ref().len() as u64 {
            return self.prefix.read(bytes);
        }
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(bytes)
    }
}
impl Write for SubscriptionIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream
            .set_write_timeout(Some(Self::remaining(self.write_deadline)?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};
    use tungstenite::WebSocket;

    fn changes() -> Changes {
        Changes::new(Arc::new(Mutex::new(ChangeHub::new(
            json!({}),
            "boot-test".into(),
            "card-test".into(),
            2,
            4,
            4096,
        ))))
    }

    fn connect(
        subscription: Subscription,
    ) -> (WebSocket<TcpStream>, thread::JoinHandle<Result<()>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let prefix = crate::read_head(&mut stream).unwrap();
            subscription.serve(stream, prefix)
        });
        let stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (client, _) =
            tungstenite::client(format!("ws://{address}/v1/changes"), stream).unwrap();
        (client, worker)
    }

    fn hello(client: &mut WebSocket<TcpStream>) -> Value {
        serde_json::from_str(client.read().unwrap().to_text().unwrap()).unwrap()
    }

    #[test]
    fn sends_registered_hello_changes_and_control_pong() {
        let changes = changes();
        let (mut client, worker) = connect(changes.reserve().unwrap());
        let greeting = hello(&mut client);
        assert_eq!(
            greeting,
            json!({"type":"hello","bootId":"boot-test","cardId":"card-test","seq":0})
        );
        changes
            .hub
            .lock()
            .unwrap()
            .publish(
                json!({"revision":1}),
                json!({"type":"resourceChanged","changed":"looks"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap();
        let event: Value = serde_json::from_str(client.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(event["seq"], 1);
        assert_eq!(event["changed"], "looks");
        client.send(Message::Ping(b"test".to_vec().into())).unwrap();
        assert_eq!(
            client.read().unwrap(),
            Message::Pong(b"test".to_vec().into())
        );
        client.close(None).unwrap();
        let _ = client.read();
        worker.join().unwrap().unwrap();
        assert_eq!(changes.active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn rejects_application_commands_and_releases_subscription() {
        let changes = changes();
        let (mut client, worker) = connect(changes.reserve().unwrap());
        hello(&mut client);
        client
            .send(Message::text("{\"action\":\"delete\"}"))
            .unwrap();
        let Message::Close(Some(close)) = client.read().unwrap() else {
            panic!("expected policy close");
        };
        assert_eq!(close.code, CloseCode::Policy);
        worker.join().unwrap().unwrap();
        assert_eq!(changes.active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn overflow_closes_instead_of_silently_losing_reliable_changes() {
        let changes = changes();
        let subscription = changes.reserve().unwrap();
        for seq in 0..5 {
            changes
                .hub
                .lock()
                .unwrap()
                .publish(
                    json!({"revision":seq}),
                    json!({"type":"resourceChanged"})
                        .as_object()
                        .unwrap()
                        .clone(),
                )
                .unwrap();
        }
        let (mut client, worker) = connect(subscription);
        assert_eq!(hello(&mut client)["seq"], 0);
        let Message::Close(Some(close)) = client.read().unwrap() else {
            panic!("expected resync close");
        };
        assert_eq!(close.code, CloseCode::Again);
        worker.join().unwrap().unwrap();
        assert!(changes.reserve().is_ok());
    }

    #[test]
    fn oversized_client_frames_release_the_subscription() {
        let changes = changes();
        let (mut client, worker) = connect(changes.reserve().unwrap());
        hello(&mut client);
        client
            .send(Message::Binary(vec![0; MAX_FRAME + 1].into()))
            .unwrap();
        assert!(client.read().is_err());
        assert!(worker.join().unwrap().is_err());
        assert_eq!(changes.active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn failed_handshake_releases_the_reservation() {
        let changes = changes();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        assert!(changes
            .reserve()
            .unwrap()
            .serve(stream, Vec::new())
            .is_err());
        assert_eq!(changes.active.load(Ordering::Acquire), 0);
        drop(peer);
    }

    #[test]
    fn limits_subscribers_and_releases_unused_reservations() {
        let changes = changes();
        let one = changes.reserve().unwrap();
        let two = changes.reserve().unwrap();
        assert_eq!(changes.reserve().err().unwrap().status, 503);
        drop(one);
        assert!(changes.reserve().is_ok());
        drop(two);
        assert_eq!(changes.active.load(Ordering::Acquire), 0);
    }
}
