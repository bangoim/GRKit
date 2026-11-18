//! Exercise only the production control transport on an isolated camera or
//! emulator card. Installer digests must be supplied from the verified build.
use anyhow::{ensure, Context, Result};
use grkit_serve::control::{self, State};
use std::{
    collections::BTreeMap,
    io::Read,
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let mode = if args.first().is_some_and(|arg| {
        matches!(
            arg.as_str(),
            "--submit-only"
                | "--resume"
                | "--identity"
                | "--snapshot"
                | "--snapshot-hold"
                | "--snapshot-ticket"
        )
    }) {
        args.remove(0)
    } else {
        String::new()
    };
    ensure!(
        args.len() >= 3 && args.len() <= 33 && args.len() % 2 == 1,
        "Usage: control_probe [--submit-only|--resume|--identity|--snapshot|--snapshot-hold|--snapshot-ticket] CARD_ROOT INSTALLER.BIN SHA256 [INSTALLER.BIN SHA256 ...]"
    );
    let mut trusted = BTreeMap::new();
    for pair in args[1..].as_chunks::<2>().0 {
        ensure!(
            trusted.insert(pair[0].clone(), pair[1].clone()).is_none(),
            "Duplicate installer name"
        );
    }
    let mut nonce = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    let (mut client, cid) = control::discover(Path::new(&args[0]), &trusted)?;
    println!("cardCid {cid:?}");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = client.reconcile()?;
        println!("reconcile {state:?}");
        match state {
            State::Idle { .. } => break,
            State::Completed { sequence, .. } => {
                client.release(sequence)?;
            }
            State::Pending { .. } => {
                ensure!(
                    Instant::now() < deadline,
                    "Previous control ticket is still pending; it was not reset"
                );
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    if mode == "--resume" {
        return Ok(());
    }
    for _ in 0..2 {
        let ticket = client.attach_or_ping(nonce)?;
        println!(
            "submitted sequence={} command={}",
            ticket.sequence, ticket.command
        );
        if mode == "--submit-only" {
            println!(
                "left sequence={} for the next process to reconcile",
                ticket.sequence
            );
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let outcome = loop {
            if let Some(outcome) = client.poll(&ticket)? {
                break outcome;
            }
            ensure!(
                Instant::now() < deadline,
                "Control ticket timed out; request remains pending for reconciliation"
            );
            thread::sleep(Duration::from_millis(20));
        };
        println!("completed {outcome:?}");
        let State::Completed { result, .. } = outcome else {
            anyhow::bail!("Unexpected control state");
        };
        client
            .release(ticket.sequence)
            .context("Release completed control ticket")?;
        ensure!(
            result == 0,
            "The runtime rejected the control command with result {result}"
        );
    }
    if mode == "--identity" {
        let State::Idle { generation, .. } = client.reconcile()? else {
            anyhow::bail!("The runtime has an outstanding command");
        };
        let ticket =
            client.submit_library(grkit_core::abi::CONTROL_CARD_IDENTITY, generation, &[])?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let outcome = loop {
            if let Some(outcome) = client.poll(&ticket)? {
                break outcome;
            }
            ensure!(
                Instant::now() < deadline,
                "Owner identity probe timed out; pending ticket was retained"
            );
            thread::sleep(Duration::from_millis(20));
        };
        println!("identity completed {outcome:?}");
        let bytes = client.response(&ticket)?;
        client.release_library(&ticket)?;
        ensure!(
            matches!(outcome, State::Completed { result: 0, .. }),
            "Owner identity probe was rejected"
        );
        println!(
            "cardId {}",
            grkit_serve::identity::owner_evidence(&bytes)?.0
        );
        println!(
            "ownerCid {}",
            bytes[4..20]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
    }
    if mode == "--snapshot-ticket" {
        let State::Idle { generation, .. } = client.reconcile()? else {
            anyhow::bail!("Pending command");
        };
        let ticket =
            client.submit_library(grkit_core::abi::CONTROL_SNAPSHOT_BEGIN, generation, &[])?;
        println!("holding unconsumed snapshot ticket {}", ticket.sequence);
        // The test supervisor terminates this process without releasing the ticket.
        thread::sleep(Duration::from_secs(120));
        anyhow::bail!("The interruption test did not terminate the probe");
    }
    if mode == "--snapshot" || mode == "--snapshot-hold" {
        use grkit_core::abi::{
            CONTROL_SNAPSHOT_BEGIN, CONTROL_SNAPSHOT_END, CONTROL_SNAPSHOT_READ,
        };
        use grkit_serve::runtime_backend::{decode_snapshot, SnapshotStream, StreamFrame};
        let State::Idle { generation, .. } = client.reconcile()? else {
            anyhow::bail!("Pending command");
        };
        let started = Instant::now();
        let frame = StreamFrame::decode(&exchange(
            &mut client,
            CONTROL_SNAPSHOT_BEGIN,
            generation,
            &[],
        )?)
        .map_err(problem)?;
        if mode == "--snapshot-hold" {
            println!("holding snapshot pin generation={}", frame.generation);
            // Keep the pin open until the test supervisor kills this process.
            thread::sleep(Duration::from_secs(120));
            anyhow::bail!("The interruption test did not terminate the probe");
        }
        let pin = frame.clone();
        let mut stream = SnapshotStream::begin(frame).map_err(problem)?;
        let mut chunks = 1;
        while !stream.complete() {
            let request = stream.next_request();
            let frame = StreamFrame::decode(&exchange(
                &mut client,
                CONTROL_SNAPSHOT_READ,
                pin.generation,
                &request.encode().map_err(problem)?,
            )?)
            .map_err(problem)?;
            stream.append(frame).map_err(problem)?;
            chunks += 1;
        }
        exchange(
            &mut client,
            CONTROL_SNAPSHOT_END,
            pin.generation,
            &stream.next_request().encode().map_err(problem)?,
        )?;
        let snapshot =
            decode_snapshot(&stream.finish().map_err(problem)?, &pin).map_err(problem)?;
        println!(
            "snapshot card={} generation={} looks={} loaded={} chunks={} elapsed_ms={}",
            snapshot.card_id,
            snapshot.generation,
            snapshot.manifest.entries.len(),
            snapshot.loaded_ids.len(),
            chunks,
            started.elapsed().as_millis()
        );
    }
    Ok(())
}

fn exchange<M: control::MailboxIo>(
    client: &mut control::Client<M>,
    command: u32,
    generation: u32,
    payload: &[u8],
) -> Result<Vec<u8>> {
    let ticket = client.submit_library(command, generation, payload)?;
    let started = Instant::now();
    loop {
        if let Some(State::Completed { result, .. }) = client.poll(&ticket)? {
            let bytes = client.response(&ticket)?;
            client.release_library(&ticket)?;
            println!(
                "owner command={command} result={result} elapsed_ms={} bytes={}",
                started.elapsed().as_millis(),
                bytes.len()
            );
            ensure!(result == 0, "Owner rejected command {command}: {result}");
            return Ok(bytes);
        }
        ensure!(
            started.elapsed() < Duration::from_secs(2),
            "Owner command {command} exceeded production deadline; ticket remains pending"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn problem(error: grkit_serve::protocol::Problem) -> anyhow::Error {
    anyhow::anyhow!("{error:?}")
}
