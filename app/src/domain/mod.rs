//! Camera and update flow models used by the UI and by demo mode.
//!
//! Kept free of NSH so unit tests run without an engine.

mod camera;
mod fake;
mod link;
mod update;

pub use camera::{Camera, CameraModel, ConnectionState, PackageInfo};
pub use fake::{DemoFail, FakeCamera, FakeConfig};
pub use link::{CameraLink, CameraStatus, LinkError, SendProgress, UpdatePoll};
pub use update::{CONFIRM_DEADLINE, UpdateError, UpdateFlow, UpdateKind, UpdatePhase};
