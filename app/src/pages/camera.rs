//! Camera tab: empty home, connected update-available, and up-to-date.

use nsh_base::{nav_page, theme};
use nsh_core::prelude::*;

use crate::domain::{CameraModel, ConnectionState};
use crate::store::{AppState, is_up_to_date};
use crate::strings::tr;
use crate::ui::{
    camera_art, chevron_row, gr_mark, grouped_card, info_row, primary_button, status_dot_row, token,
};

pub fn camera_root(app: AppState) -> impl IntoElement {
    // No navigation title: the design uses the camera art as the header.
    // The stack still pushes Software Update with a system back button.
    nav_page().header_visible(false).child(dynamic(move || {
        let status = app.status();
        match status {
            Some(s) if s.camera.connection == ConnectionState::Connected => {
                connected(app.clone(), s.camera.model).into_element()
            }
            _ => empty_home(app.clone()).into_element(),
        }
    }))
}

fn empty_home(app: AppState) -> impl IntoElement {
    view()
        .flex_1()
        .bg(move || token("grouped"))
        .items_center()
        .justify_center()
        .px_8()
        .gap_4()
        .child(camera_art(CameraModel::Gr3, 236.0, true))
        .child(
            text(move || tr("home.title", &[]))
                .text_size(px(22.0))
                .font_weight(FontWeight::BOLD)
                .text_center(),
        )
        .child(
            text(move || tr("home.subtitle", &[]))
                .text_size(px(15.0))
                .text_color(move || theme().muted_foreground)
                .text_center()
                .max_w(px(300.0)),
        )
        .child(primary_button(tr("home.add", &[]), move || app.open_scan()))
        .child(
            view().h(px(48.0)).child(
                text(move || tr("home.footnote", &[]))
                    .text_size(px(13.0))
                    .text_color(move || theme().muted_foreground)
                    .text_center(),
            ),
        )
}

fn connected(app: AppState, model: CameraModel) -> impl IntoElement {
    let package = app.package();
    let up_to_date = package.as_ref().is_some_and(is_up_to_date);
    // Art starts 84 pt from the screen top (below the status bar / safe area).
    scroll().flex_1().bg(move || token("grouped")).child(
        view()
            .gap_5()
            .pt(px(84.0))
            .px_4()
            .pb_10()
            .child(
                view()
                    .items_center()
                    .gap_3()
                    .child(camera_art(model, 236.0, false))
                    .child(status_dot_row(
                        model.display_name(),
                        tr("connected.status", &[]),
                    )),
            )
            .child(if up_to_date {
                up_to_date_card(app.clone()).into_element()
            } else {
                update_available_card(app.clone()).into_element()
            })
            .child(version_card(app)),
    )
}

fn update_available_card(app: AppState) -> impl IntoElement {
    let version = app
        .package()
        .and_then(|p| p.available)
        .unwrap_or_else(|| "1.7.0".into());
    grouped_card(
        view().p_4().gap_3().child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .role(AccessibilityRole::Button)
                .label(format!("GRKit {version}"))
                .on_press({
                    let app = app.clone();
                    move || app.open_software_update()
                })
                .child(gr_mark(48.0))
                .child(
                    view()
                        .flex_1()
                        .gap(px(2.0))
                        .child(
                            text(format!("GRKit {version}"))
                                .text_size(px(17.0))
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(
                            text(move || tr("connected.whats_new", &[]))
                                .text_size(px(13.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(
                    view()
                        .h(px(34.0))
                        .px_4()
                        .rounded(px(17.0))
                        .items_center()
                        .justify_center()
                        .bg(move || theme().primary)
                        .role(AccessibilityRole::Button)
                        .label(tr("connected.update", &[]))
                        .on_press({
                            let app = app.clone();
                            move || {
                                app.begin_update(crate::domain::UpdateKind::Install);
                            }
                        })
                        .child(
                            text(move || tr("connected.update", &[]))
                                .text_size(px(15.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || theme().primary_foreground),
                        ),
                ),
        ),
    )
}

fn up_to_date_card(app: AppState) -> impl IntoElement {
    grouped_card(chevron_row(
        tr("connected.up_to_date", &[]),
        Some(tr("connected.software_update", &[])),
        move || app.open_software_update(),
    ))
}

fn version_card(app: AppState) -> impl IntoElement {
    let package = app.package();
    let grkit = package
        .as_ref()
        .map(|p| p.installed.clone())
        .unwrap_or_default();
    let firmware = package
        .as_ref()
        .map(|p| p.firmware.clone())
        .unwrap_or_default();
    grouped_card(
        view()
            .child(info_row(tr("connected.grkit", &[]), grkit, false))
            .child(info_row(tr("connected.firmware", &[]), firmware, true)),
    )
}
