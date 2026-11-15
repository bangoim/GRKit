//! Camera tab: empty home, connected update-available, and up-to-date.

use nsh_base::{nav_page, theme};
use nsh_core::prelude::*;

use crate::domain::{CameraModel, ConnectionState};
use crate::store::{AppState, is_up_to_date};
use crate::strings::tr;
use crate::ui::{camera_art, chevron_row, gr_mark, grouped_card, primary_button, token};

pub fn camera_root(app: AppState) -> impl IntoElement {
    // Each camera state supplies its own header.
    // The stack still pushes Software Update with a system back button.
    nav_page().header_visible(false).child(dynamic(move || {
        let status = app.status();
        match status {
            Some(s) => connected(
                app.clone(),
                s.camera.model,
                s.camera.connection == ConnectionState::Connected,
            )
            .into_element(),
            _ => empty_home(app.clone()).into_element(),
        }
    }))
}

fn empty_home(app: AppState) -> impl IntoElement {
    scroll()
        .flex_1()
        .bg(move || token("connection-background"))
        .child(
            view()
                .px_6()
                .pt(px(72.0))
                .gap_6()
                .child(
                    view()
                        .gap_2()
                        .child(
                            view()
                                .w(px(28.0))
                                .h(px(3.0))
                                .bg(move || token("connection-brand")),
                        )
                        .child(
                            text("GRKit")
                                .text_size(px(28.0))
                                .font_weight(FontWeight::BOLD),
                        )
                        .child(
                            text(tr("home.subtitle", &[]))
                                .text_size(px(15.0))
                                .line_height(px(22.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(
                    view()
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(move || token("connection-line"))
                        .bg(move || token("connection-background"))
                        .p_6()
                        .gap_5()
                        .child(view().items_center().py_4().child(camera_art(
                            CameraModel::Gr3,
                            180.0,
                            false,
                        )))
                        .child(
                            view()
                                .gap_2()
                                .child(
                                    text(tr("home.title", &[]))
                                        .text_size(px(22.0))
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(
                                    text(tr("connecting.bt", &[]))
                                        .text_size(px(14.0))
                                        .text_color(move || theme().muted_foreground),
                                ),
                        )
                        .child(crate::ui::connection_button(
                            tr("home.add", &[]),
                            move || app.open_scan(),
                        )),
                )
                .child(
                    text(tr("home.footnote", &[]))
                        .text_size(px(13.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

fn connected(app: AppState, model: CameraModel, online: bool) -> impl IntoElement {
    let package = app.package();
    let update_unknown = !app.demo() && package.as_ref().is_none_or(|p| p.available.is_none());
    let up_to_date = package.as_ref().is_some_and(is_up_to_date);
    scroll().flex_1().bg(move || token("grouped")).child(
        view()
            .gap_4()
            .pt(px(72.0))
            .px_6()
            .pb_10()
            .child(
                view()
                    .gap_2()
                    .child(view().w(px(28.0)).h(px(3.0)).bg(move || token("tint")))
                    .child(
                        text("GRKit")
                            .text_size(px(28.0))
                            .font_weight(FontWeight::BOLD),
                    ),
            )
            .child(grouped_card(
                view()
                    .p_4()
                    .gap_4()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_4()
                            .child(camera_art(model, 80.0, false))
                            .child(
                                view()
                                    .flex_1()
                                    .gap(px(4.0))
                                    .child(
                                        text(format!("RICOH {}", model.display_name()))
                                            .text_size(px(20.0))
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(
                                        text(if online {
                                            app.remembered()
                                                .map(|r| r.last_ssid)
                                                .unwrap_or_else(|| tr("connected.status", &[]))
                                        } else {
                                            tr("connected.offline", &[])
                                        })
                                        .text_size(px(15.0))
                                        .text_color(move || theme().muted_foreground),
                                    ),
                            ),
                    )
                    .child(version_card(app.clone())),
            ))
            .child(if !online {
                primary_button(tr("nearby.connect", &[]), move || app.start_connect())
                    .into_element()
            } else if update_unknown {
                grouped_card(chevron_row(
                    tr("connected.software_update", &[]),
                    Some(tr(
                        if app.release_checking() {
                            "software.checking"
                        } else {
                            "software.unavailable"
                        },
                        &[],
                    )),
                    move || app.open_software_update(),
                ))
                .into_element()
            } else if up_to_date {
                up_to_date_card(app).into_element()
            } else {
                update_available_card(app).into_element()
            }),
    )
}

fn update_available_card(app: AppState) -> impl IntoElement {
    let version = app.package().and_then(|p| p.available).unwrap_or_default();
    grouped_card(
        view()
            .p_4()
            .gap_4()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .role(AccessibilityRole::Button)
                    .label(format!("GRKit {version}"))
                    .active(|s| s.opacity(0.72))
                    .on_press({
                        let app = app.clone();
                        move || app.open_software_update()
                    })
                    .child(gr_mark(48.0))
                    .child(
                        view()
                            .flex_1()
                            .gap(px(4.0))
                            .child(
                                text(format!("GRKit {version}"))
                                    .text_size(px(17.0))
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .child(
                                text(tr("connected.whats_new", &[]))
                                    .text_size(px(15.0))
                                    .line_height(px(22.0))
                                    .text_color(move || theme().muted_foreground),
                            ),
                    )
                    .child(
                        text("›")
                            .text_size(px(24.0))
                            .text_color(move || theme().muted_foreground),
                    ),
            )
            .child(primary_button(tr("connected.update", &[]), move || {
                app.begin_update(crate::domain::UpdateKind::Install)
            })),
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
    view()
        .flex_row()
        .gap_2()
        .child(version_tile(grkit, tr("connected.grkit", &[])))
        .child(version_tile(firmware, tr("connected.firmware", &[])))
}

fn version_tile(value: String, label: String) -> impl IntoElement {
    view()
        .flex_1()
        .gap(px(4.0))
        .p_3()
        .rounded(px(8.0))
        .bg(move || theme().muted)
        .child(
            text(value)
                .text_size(px(20.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            text(label)
                .text_size(px(13.0))
                .text_color(move || theme().muted_foreground),
        )
}
