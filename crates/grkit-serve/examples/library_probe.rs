//! Exercise the production signed-release bridge on an isolated emulator card.
use anyhow::{bail, ensure, Result};
use grkit_serve::{
    library::Patch,
    protocol::Problem,
    runtime_backend::{LinuxBackend, RuntimeBackend, RuntimeSnapshot},
};
use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

fn problem(error: Problem) -> anyhow::Error {
    anyhow::anyhow!("{}", serde_json::to_string(&error).unwrap())
}

fn report(snapshot: &RuntimeSnapshot) -> Result<()> {
    // Camera console output has a bounded single-response buffer. Keep full
    // receipt verification above the report, even when only a summary fits.
    if std::env::var("GRKIT_PROBE_COMPACT").as_deref() == Ok("1") {
        let focus = std::env::var("GRKIT_PROBE_LOOK_ID").ok();
        let position = focus
            .as_ref()
            .and_then(|id| snapshot.manifest.order.iter().position(|entry| entry == id));
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "cardId": snapshot.card_id,
                "generation": snapshot.generation,
                "hotReload": snapshot.context.hot_reload,
                "revision": snapshot.revision().map_err(problem)?,
                "count": snapshot.manifest.entries.len(),
                "loadedCount": snapshot.loaded_ids.len(),
                "selected": snapshot.context.selected,
                "position": position,
                "look": focus.as_ref().and_then(|id| snapshot.manifest.entries.get(id)),
                "loaded": focus.as_ref().is_some_and(|id| snapshot.loaded_ids.contains(id)),
            }))?
        );
        return Ok(());
    }
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "cardId": snapshot.card_id,
            "runtimeEpoch": snapshot.runtime_epoch,
            "generation": snapshot.generation,
            "hotReload": snapshot.context.hot_reload,
            "revision": snapshot.revision().map_err(problem)?,
            "manifest": snapshot.manifest,
            "loadedIds": snapshot.loaded_ids,
            "selected": snapshot.context.selected,
        }))?
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(args.len() >= 3,
        "Usage: library_probe CARD_ROOT PUBLIC_KEY_HEX snapshot|import ID FILE|rename ID NAME|enable ID true|false|order ID...|delete ID [off]");
    ensure!(
        args[1].len() == 64 && args[1].bytes().all(|b| b.is_ascii_hexdigit()),
        "The verification key must contain 64 hexadecimal digits"
    );
    let mut key = [0; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&args[1][index * 2..index * 2 + 2], 16)?;
    }
    let mut nonce = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    let mut backend = LinuxBackend::discover(Path::new(&args[0]), &[key], nonce)?;
    let before = backend.snapshot().map_err(problem)?;
    let action = args[2].as_str();
    let operands = &args[3..];
    if action == "snapshot" && operands.is_empty() {
        return report(&before);
    }
    let prepared = match (action, operands) {
        ("import", [id, path]) => {
            before
                .manifest
                .prepare_put(id, &std::fs::read(path)?, &before.context)
        }
        ("rename", [id, name]) => before.manifest.prepare_patch(
            id,
            &Patch {
                name: Some(name.clone()),
                enabled: None,
            },
            before.content(id).map_err(problem)?,
            &before.context,
        ),
        ("enable", [id, enabled]) => before.manifest.prepare_patch(
            id,
            &Patch {
                name: None,
                enabled: Some(enabled.parse()?),
            },
            before.content(id).map_err(problem)?,
            &before.context,
        ),
        ("order", ids) => before.manifest.prepare_order(ids, &before.context),
        ("delete", [id]) => before.manifest.prepare_delete(id, false, &before.context),
        ("delete", [id, off]) if off == "off" => {
            before.manifest.prepare_delete(id, true, &before.context)
        }
        _ => bail!("Invalid action or operands"),
    }
    .map_err(problem)?;
    let expected = prepared.clone();
    let after = match backend.apply(&before.revision().map_err(problem)?, prepared) {
        Ok(after) => after,
        Err(error) if error.code == "recovery_required" => {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Ok(Some(after)) = backend.reconcile_apply() {
                    break after;
                }
                if Instant::now() >= deadline {
                    return Err(problem(error));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Err(error) => return Err(problem(error)),
    };
    after.verify_receipt(&before, &expected).map_err(problem)?;
    report(&after)
}
