//! GRKit phone app: update GRKit on a RICOH GR camera over Wi-Fi.
//!
//! Layers, top down:
//! - `shell`: tab bar and navigation stacks
//! - `pages`: Camera, Settings, Software Update, sheets
//! - `ui`: shared phone chrome
//! - `store`: app state and demo mode
//! - `domain`: camera model, update flow, fake camera link
//! - `strings`: English and Simplified Chinese copy

#![allow(clippy::clone_on_copy)]

pub mod domain;
mod pages;
mod platform;
mod shell;
pub mod store;
mod strings;
mod ui;

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::store::AppState;

/// Keeps the core crate linked for a later task that talks to packages.
#[allow(dead_code)]
fn _link_core() {
    let _ = std::any::type_name::<grkit_core::camera::Camera>();
}

/// The crate's UI set (`nsh.json` `"rust"`, `"root": "App"`).
#[native_module("grkit/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>().assets(assets!());
}

/// Application root.
#[component]
pub fn App() -> impl IntoElement {
    let app = AppState::new();
    app.ensure_ticker();
    provide_context(app.clone());
    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(shell::shell(app.clone()))
        .child(pages::overlay(app))
}
