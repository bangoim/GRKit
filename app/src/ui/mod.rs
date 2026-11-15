//! Shared phone chrome: grouped rows, camera art, primary buttons.

mod chrome;

pub use chrome::*;

mod loading;
pub use loading::{camera_skeleton, update_skeleton};
