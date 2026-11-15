//! Join the camera access point via AccessorySetupKit (iOS 18+) or HotspotConfiguration.

use futures_util::future::{Either, select};
use nsh_core::platform::Lifecycle;
use nsh_core::prelude::{lifecycle, timer};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use futures_channel::oneshot;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_accessory_setup_kit::{
    ASAccessory, ASAccessoryEvent, ASAccessoryEventType, ASAccessorySession, ASAccessoryState,
    ASDiscoveryDescriptor, ASPickerDisplayItem,
};
use objc2_foundation::{NSArray, NSError, NSProcessInfo, NSString};
use objc2_network_extension::{
    NEHotspotConfiguration, NEHotspotConfigurationError, NEHotspotConfigurationManager,
};
use objc2_ui_kit::UIImage;

use crate::domain::{LinkError, WifiCredentials};

struct PendingJoin {
    id: u64,
    joining: bool,
    ssid: String,
    passphrase: String,
    reply: Option<oneshot::Sender<Result<(), LinkError>>>,
}

struct AskState {
    session: Retained<ASAccessorySession>,
    activated: bool,
    next_id: u64,
    picker_id: Option<u64>,
    pending: Option<PendingJoin>,
    activate_waiters: Vec<oneshot::Sender<()>>,
}

/// One session for the app lifetime; activated once and reused for each join.
static ASK: OnceLock<Mutex<AskState>> = OnceLock::new();

/// Join the camera Wi-Fi. Prefer AccessorySetupKit on iOS 18+; otherwise HotspotConfiguration.
pub async fn join(creds: &WifiCredentials) -> Result<(), LinkError> {
    if creds.ssid.is_empty() {
        return Err(LinkError::WifiJoinRejected);
    }
    app_info!("Wi-Fi join start ssid_len={}", creds.ssid.len());
    if ios_major() >= 18 {
        // ASK consent is authoritative. Never bypass a denial with another API.
        return join_accessory_setup(creds).await;
    }
    let result = join_hotspot_configuration(creds).await;
    match &result {
        Ok(()) => app_info!("Wi-Fi join via HotspotConfiguration ok"),
        Err(e) => log::warn!("Wi-Fi join failed: {e:?}"),
    }
    result
}

async fn join_hotspot_configuration(creds: &WifiCredentials) -> Result<(), LinkError> {
    let (tx, rx) = oneshot::channel::<Result<(), LinkError>>();
    let ssid = creds.ssid.clone();
    let passphrase = creds.passphrase.clone();
    DispatchQueue::main().exec_async(move || {
        // Cancellation may happen before the main queue starts this operation.
        if tx.is_canceled() {
            return;
        }
        let ssid_ns = NSString::from_str(&ssid);
        let pass_ns = NSString::from_str(&passphrase);
        let config = unsafe {
            NEHotspotConfiguration::initWithSSID_passphrase_isWEP(
                NEHotspotConfiguration::alloc(),
                &ssid_ns,
                &pass_ns,
                false,
            )
        };
        unsafe { config.setJoinOnce(false) };
        let reply = Mutex::new(Some(tx));
        let handler = RcBlock::new(move |error: *mut NSError| {
            let Some(tx) = reply.lock().unwrap().take() else {
                return;
            };
            let _ = tx.send(map_hotspot_error(error));
        });
        unsafe {
            NEHotspotConfigurationManager::sharedManager()
                .applyConfiguration_completionHandler(&config, Some(&handler));
        }
    });
    wait_join(rx, || true).await
}

/// Dropping an attempt detaches its waiter; native callbacks retain their own id.
struct JoinLease(u64);
impl Drop for JoinLease {
    fn drop(&mut self) {
        fail_pending(self.0, LinkError::Cancelled);
    }
}

async fn wait_join(
    rx: oneshot::Receiver<Result<(), LinkError>>,
    is_joining: impl Fn() -> bool,
) -> Result<(), LinkError> {
    let deadline = async {
        let mut foreground = Duration::ZERO;
        loop {
            let active = lifecycle() == Lifecycle::Active;
            timer(Duration::from_millis(250)).await;
            // Exclude consent time and suspension behind system dialogs.
            if is_joining() && active && lifecycle() == Lifecycle::Active {
                foreground += Duration::from_millis(250);
                if foreground >= Duration::from_secs(30) {
                    return Err(LinkError::WifiJoinFailed);
                }
            }
        }
    };
    let http_proof = async {
        loop {
            // Never let an HTTP answer bypass the ASK consent sheet.
            if is_joining()
                && lifecycle() == Lifecycle::Active
                && crate::platform::http::service_ready().await
            {
                return Ok(());
            }
            timer(Duration::from_millis(600)).await;
        }
    };
    match select(
        Box::pin(rx),
        Box::pin(select(Box::pin(deadline), Box::pin(http_proof))),
    )
    .await
    {
        Either::Left((result, _)) => result.unwrap_or(Err(LinkError::Cancelled)),
        Either::Right((Either::Left((result, _)) | Either::Right((result, _)), _)) => result,
    }
}

async fn join_accessory_setup(creds: &WifiCredentials) -> Result<(), LinkError> {
    ensure_ask_activated().await?;
    let (tx, rx) = oneshot::channel::<Result<(), LinkError>>();
    let id = {
        let mut state = ask_state().lock().unwrap();
        // A previous picker may still be on screen after app-side cancellation.
        if state.pending.is_some() || state.picker_id.is_some() {
            return Err(LinkError::Busy);
        }
        state.next_id = state.next_id.wrapping_add(1);
        let id = state.next_id;
        state.pending = Some(PendingJoin {
            id,
            joining: false,
            ssid: creds.ssid.clone(),
            passphrase: creds.passphrase.clone(),
            reply: Some(tx),
        });
        id
    };
    let _lease = JoinLease(id);
    DispatchQueue::main().exec_async(move || start_authorize_or_join(id));
    wait_join(rx, || {
        ask_state()
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == id && pending.joining)
    })
    .await
}

fn ask_state() -> &'static Mutex<AskState> {
    ASK.get_or_init(|| {
        let session = unsafe { ASAccessorySession::new() };
        Mutex::new(AskState {
            session,
            activated: false,
            next_id: 0,
            picker_id: None,
            pending: None,
            activate_waiters: Vec::new(),
        })
    })
}

async fn ensure_ask_activated() -> Result<(), LinkError> {
    let (need_activate, waiter) = {
        let mut state = ask_state().lock().unwrap();
        if state.activated {
            return Ok(());
        }
        let (tx, rx) = oneshot::channel();
        // Timed-out or cancelled callers must not suppress a fresh activation.
        state
            .activate_waiters
            .retain(|waiter| !waiter.is_canceled());
        state.activate_waiters.push(tx);
        let need = state.activate_waiters.len() == 1;
        (need, rx)
    };
    if need_activate {
        DispatchQueue::main().exec_async(|| {
            activate_ask_session();
        });
    }
    crate::platform::with_timeout(waiter, Duration::from_secs(10))
        .await
        .ok_or(LinkError::WifiJoinRejected)?
        .map_err(|_| LinkError::Cancelled)?;
    Ok(())
}

fn activate_ask_session() {
    let session = {
        let state = ask_state().lock().unwrap();
        state.session.clone()
    };

    let handler = RcBlock::new(move |event_ptr: std::ptr::NonNull<ASAccessoryEvent>| {
        let event = unsafe { event_ptr.as_ref() };
        let event_type = unsafe { event.eventType() };
        if event_type == ASAccessoryEventType::Activated {
            let waiters = {
                let mut state = ask_state().lock().unwrap();
                state.activated = true;
                std::mem::take(&mut state.activate_waiters)
            };
            for w in waiters {
                let _ = w.send(());
            }
        } else if event_type == ASAccessoryEventType::PickerDidDismiss {
            let id = ask_state().lock().unwrap().picker_id.take();
            if let Some(id) = id {
                finish_after_picker(id);
            }
        } else if event_type == ASAccessoryEventType::Invalidated {
            // Should not happen for a lifetime session; fail any in-flight join.
            let id = ask_state().lock().unwrap().pending.as_ref().map(|p| p.id);
            if let Some(id) = id {
                fail_pending(id, LinkError::WifiJoinRejected);
            }
            let mut state = ask_state().lock().unwrap();
            state.activated = false;
            state.picker_id = None;
            state.activate_waiters.clear();
        }
    });

    unsafe {
        session.activateWithQueue_eventHandler(DispatchQueue::main(), &handler);
    }
}

fn start_authorize_or_join(id: u64) {
    let (session, ssid, passphrase) = {
        let state = ask_state().lock().unwrap();
        if !state.activated {
            return;
        }
        let Some(pending) = state.pending.as_ref().filter(|pending| pending.id == id) else {
            return;
        };
        (
            state.session.clone(),
            pending.ssid.clone(),
            pending.passphrase.clone(),
        )
    };

    if let Some(accessory) = find_authorized(&session, &ssid) {
        finish_join(id, &accessory, &passphrase);
        return;
    }

    let descriptor = unsafe { ASDiscoveryDescriptor::new() };
    let ssid_ns = NSString::from_str(&ssid);
    unsafe { descriptor.setSSID(Some(&ssid_ns)) };
    let Some(image) = UIImage::systemImageNamed(&NSString::from_str("wifi")) else {
        fail_pending(id, LinkError::WifiJoinRejected);
        return;
    };
    let item = unsafe {
        ASPickerDisplayItem::initWithName_productImage_descriptor(
            ASPickerDisplayItem::alloc(),
            &ssid_ns,
            &image,
            &descriptor,
        )
    };
    let items = NSArray::from_retained_slice(&[item]);
    ask_state().lock().unwrap().picker_id = Some(id);
    let picker_done = RcBlock::new(move |error: *mut NSError| {
        if !error.is_null() {
            let mut state = ask_state().lock().unwrap();
            if state.picker_id == Some(id) {
                state.picker_id = None;
            }
            drop(state);
            fail_pending(id, LinkError::WifiJoinRejected);
        }
    });
    unsafe {
        session.showPickerForDisplayItems_completionHandler(&items, &picker_done);
    }
}

fn finish_after_picker(id: u64) {
    let (session, ssid, passphrase) = {
        let state = ask_state().lock().unwrap();
        let Some(pending) = state.pending.as_ref().filter(|pending| pending.id == id) else {
            return;
        };
        (
            state.session.clone(),
            pending.ssid.clone(),
            pending.passphrase.clone(),
        )
    };
    if let Some(accessory) = find_authorized(&session, &ssid) {
        finish_join(id, &accessory, &passphrase);
        return;
    }
    fail_pending(id, LinkError::Cancelled);
}

fn find_authorized(session: &ASAccessorySession, ssid: &str) -> Option<Retained<ASAccessory>> {
    let accessories = unsafe { session.accessories() };
    for accessory in accessories {
        let accessory_ssid = unsafe { accessory.SSID() }
            .map(|s| s.to_string())
            .unwrap_or_default();
        let state = unsafe { accessory.state() };
        if accessory_ssid == ssid && state == ASAccessoryState::Authorized {
            return Some(accessory);
        }
    }
    None
}

fn finish_join(id: u64, accessory: &ASAccessory, passphrase: &str) {
    let pass_ns = NSString::from_str(passphrase);
    {
        let mut state = ask_state().lock().unwrap();
        let Some(pending) = state.pending.as_mut().filter(|p| p.id == id && !p.joining) else {
            return;
        };
        pending.joining = true;
    }
    let done = RcBlock::new(move |error: *mut NSError| {
        finish_pending(id, map_hotspot_error(error));
    });
    unsafe {
        NEHotspotConfigurationManager::sharedManager()
            .joinAccessoryHotspot_passphrase_completionHandler(accessory, &pass_ns, Some(&done));
    }
}

fn finish_pending(id: u64, result: Result<(), LinkError>) {
    let mut state = ask_state().lock().unwrap();
    if state.pending.as_ref().is_some_and(|p| p.id == id)
        && let Some(mut pending) = state.pending.take()
        && let Some(tx) = pending.reply.take()
    {
        let _ = tx.send(result);
    }
}

fn fail_pending(id: u64, err: LinkError) {
    finish_pending(id, Err(err));
}

fn map_hotspot_error(error: *mut NSError) -> Result<(), LinkError> {
    if error.is_null() {
        return Ok(());
    }
    let err = unsafe { &*error };
    if err.code() == NEHotspotConfigurationError::AlreadyAssociated.0 {
        Ok(())
    } else if err.code() == NEHotspotConfigurationError::UserDenied.0 {
        Err(LinkError::Cancelled)
    } else if [
        NEHotspotConfigurationError::Invalid.0,
        NEHotspotConfigurationError::InvalidSSID.0,
        NEHotspotConfigurationError::InvalidWPAPassphrase.0,
        NEHotspotConfigurationError::InvalidWEPPassphrase.0,
        NEHotspotConfigurationError::InvalidSSIDPrefix.0,
        NEHotspotConfigurationError::ApplicationIsNotInForeground.0,
        NEHotspotConfigurationError::UserUnauthorized.0,
        NEHotspotConfigurationError::SystemDenied.0,
    ]
    .contains(&err.code())
    {
        Err(LinkError::WifiJoinRejected)
    } else {
        Err(LinkError::WifiJoinFailed)
    }
}

fn ios_major() -> u32 {
    NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion as u32
}
