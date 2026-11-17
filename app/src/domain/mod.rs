//! Camera and update flow models used by the UI and by demo mode.
//!
//! Kept free of NSH so unit tests run without an engine.

mod advertisement;
mod camera;
mod connect_flow;
mod event_filter;
mod fake;
mod http_api;
mod link;
mod nearby_prompt;
mod remembered;
mod update;
mod update_api;
pub use update_api::{
    JournalState, RollbackResource, RollbackSnapshot, UpdateIntent, UpdateObservation,
    UpdateResource, UpdateSnapshot, new_request_id, observe_update, response_error,
};
pub mod uuids;
mod wifi_cred;

pub use advertisement::{Advertisement, is_supported_camera, model_from_code, parse_advertisement};
pub use camera::{Camera, CameraModel, ConnectionState, PackageInfo};
pub use connect_flow::{
    BleSetup, ConnectStep, ConnectTransport, WifiCredentials, run_connect, timing,
};
pub use event_filter::{ApReadyLatch, is_ble_pairing_error, matches_operation};
pub use fake::{DemoFail, FakeCamera, FakeConfig};
pub use http_api::{
    CAMERA_HTTP_BASE, PackageCamera, PackageMapError, PackageResponse, PropsResponse,
    STATUS_POLL_BUDGET_MS, StatusAttempt, classify_props_response, installed_to_models,
    package_to_models, parse_package, parse_props, should_retry_status, status_backoff_ms,
    status_poll_timeout_error, version_at_least,
};
pub use link::{
    CameraLink, CameraStatus, LinkError, NearbyCamera, SendProgress, UpdatePoll, block_on_ready,
};
pub use nearby_prompt::NearbyPromptGate;
pub use remembered::{RememberedCamera, decode as decode_remembered, encode as encode_remembered};
pub use update::{CONFIRM_DEADLINE, UpdateError, UpdateFlow, UpdateKind, UpdatePhase};
pub use wifi_cred::decode_cred;
