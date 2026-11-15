//! Load and save the last connected camera in the app data directory.

use std::fs;
use std::path::PathBuf;

use nsh_core::prelude::data_dir;

use crate::domain::{RememberedCamera, decode_remembered, encode_remembered};

fn path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("remembered_camera.json"))
}

pub fn load() -> Option<RememberedCamera> {
    let path = path()?;
    let text = fs::read_to_string(path).ok()?;
    decode_remembered(&text).ok()
}

pub fn save(camera: &RememberedCamera) -> Result<(), String> {
    let dir = data_dir().ok_or_else(|| "no app data directory".to_owned())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = encode_remembered(camera)?;
    fs::write(dir.join("remembered_camera.json"), text).map_err(|e| e.to_string())
}
