//! Modal sheets for pairing, connecting, update progress, and errors.

use nsh_base::{SheetEvent, progress, sheet, theme};
use nsh_core::prelude::*;

use crate::domain::{CameraModel, LinkError, UpdateError, UpdateKind, UpdatePhase};
use crate::store::{AppState, Sheet, link_error_body, link_error_title};
use crate::strings::{format_bytes, tr};
use crate::ui::{
    STEP1, STEP2, STEP3, StepState, camera_art, checklist_row, connection_button, connection_icon,
    secondary_button, sheet_primary_button, sheet_shell, sheet_surface, token, touch_target,
};

pub fn overlay(app: AppState) -> impl IntoElement {
    let dismissible = {
        let app = app.clone();
        move || {
            matches!(
                app.sheet(),
                Some(Sheet::Nearby | Sheet::Scan | Sheet::ImportLook)
            )
        }
    };
    sheet()
        .open({
            let app = app.clone();
            move || Some(app.sheet().is_some())
        })
        .side(Placement::Bottom)
        .close_on_backdrop_press(dismissible)
        .close_on_swipe(dismissible)
        .close_on_escape(dismissible)
        .backdrop_style(|s| s.bg(rgba(0x00000047)))
        .popup_style(|s| {
            s.mx(px(if cfg!(target_os = "android") {
                0.0
            } else {
                8.0
            }))
            .mb(px(if cfg!(target_os = "android") {
                0.0
            } else {
                8.0
            }))
            .rounded_t(px(if cfg!(target_os = "android") {
                28.0
            } else {
                30.0
            }))
            .rounded_b(px(if cfg!(target_os = "android") {
                0.0
            } else {
                30.0
            }))
            .bg(sheet_surface)
            .shadow([BoxShadow {
                offset_x: Points(0.0),
                offset_y: Points(20.0),
                blur: Points(60.0),
                spread: Points(0.0),
                color: rgba(0x00000040).into(),
                inset: false,
            }])
        })
        .on_open_change({
            let app = app.clone();
            move |e: &SheetEvent| {
                if let SheetEvent::OpenChange(false, _) = e {
                    match app.sheet() {
                        Some(Sheet::Nearby) => app.dismiss_nearby(),
                        Some(Sheet::Scan | Sheet::ImportLook) => app.set_sheet(None),
                        _ => {}
                    }
                }
            }
        })
        .child(dynamic({
            let app = app.clone();
            move || match app.sheet() {
                Some(Sheet::Nearby) => nearby(app.clone()).into_element(),
                Some(Sheet::Scan) => scan(app.clone()).into_element(),
                Some(Sheet::ImportLook) => crate::pages::import_sheet(app.clone()).into_element(),
                Some(Sheet::Connecting { step }) => connecting(app.clone(), step).into_element(),
                Some(Sheet::ConnectError(error)) => {
                    connect_error_sheet(app.clone(), error).into_element()
                }
                Some(Sheet::Sending) => sending(app.clone()).into_element(),
                Some(Sheet::Confirm) => confirm(app.clone()).into_element(),
                Some(Sheet::Restart) => restart(app.clone()).into_element(),
                Some(Sheet::Installed { version, previous }) => {
                    installed(app.clone(), version, previous).into_element()
                }
                Some(Sheet::Error(error)) => error_sheet(app.clone(), error).into_element(),
                None => view().into_element(),
            }
        }))
}

/// Connection sheets follow the camera discovery layout without changing update sheets.
fn connection_shell(
    title: impl Into<String>,
    subtitle: Option<String>,
    show_close: bool,
    on_close: impl Fn() + 'static,
    body: impl IntoElement,
) -> impl IntoElement {
    view()
        .relative()
        .w_full()
        .gap_5()
        .px(px(24.0))
        .pt(px(6.0))
        .pb_safe_area_plus(px(8.0))
        .child(
            view()
                .self_center()
                .w(px(36.0))
                .h(px(4.0))
                .rounded_full()
                .bg(rgb(0xC7C7CC)),
        )
        .child(
            view()
                .gap(px(6.0))
                .pr(if show_close {
                    px(touch_target())
                } else {
                    px(0.0)
                })
                .child(
                    text(title.into())
                        .text_size(px(22.0))
                        .font_weight(FontWeight::BOLD),
                )
                .child(subtitle.map(|s| {
                    text(s)
                        .text_size(px(15.0))
                        .line_height(px(22.0))
                        .text_color(move || theme().muted_foreground)
                })),
        )
        .child(show_close.then(|| {
            view().absolute().right(px(18.0)).top(px(22.0)).child(
                view()
                    .size(px(touch_target()))
                    .items_center()
                    .justify_center()
                    .role(AccessibilityRole::Button)
                    .label(tr("scan.close", &[]))
                    .on_press(on_close)
                    .child(connection_icon("M6 6l12 12M18 6L6 18")),
            )
        }))
        .child(body)
}

fn connection_step(
    title: &str,
    subtitle: Option<&str>,
    state: StepState,
    number: u8,
) -> impl IntoElement {
    let current = matches!(state, StepState::Current);
    let done = matches!(state, StepState::Done);
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .p_4()
        .rounded(px(8.0))
        .style_when(current, |s| s.bg(move || token("connection-selected")))
        .child(
            view()
                .size(px(28.0))
                .rounded_full()
                .items_center()
                .justify_center()
                .bg(move || {
                    if done {
                        token("success-soft")
                    } else if current {
                        token("connection-brand")
                    } else {
                        theme().muted
                    }
                })
                .text_color(move || {
                    if done {
                        token("success")
                    } else if current {
                        rgb(0xFFFFFF).into()
                    } else {
                        theme().muted_foreground
                    }
                })
                .child(if done {
                    connection_icon("M5 12l4 4L19 6").into_element()
                } else {
                    text(number.to_string())
                        .text_size(px(13.0))
                        .line_height(px(18.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .into_element()
                }),
        )
        .child(
            view()
                .flex_1()
                .gap_1()
                .child(
                    text(title.to_owned())
                        .text_size(px(16.0))
                        .font_weight(if current {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(move || {
                            if current {
                                theme().foreground
                            } else {
                                theme().muted_foreground
                            }
                        }),
                )
                .child(subtitle.filter(|_| current).map(|s| {
                    text(s.to_owned())
                        .text_size(px(13.0))
                        .line_height(px(18.0))
                        .text_color(move || theme().muted_foreground)
                })),
        )
}

fn nearby(app: AppState) -> impl IntoElement {
    let model = app.selected_model().unwrap_or(CameraModel::Gr3);
    let hint_key = model.pairing_hint_key();
    connection_shell(
        format!("RICOH {}", model.display_name()),
        Some(tr("nearby.powered_on", &[])),
        true,
        {
            let app = app.clone();
            move || app.dismiss_nearby()
        },
        view()
            .w_full()
            .gap_5()
            .child(
                view()
                    .w_full()
                    .rounded(px(8.0))
                    .bg(move || token("connection-muted"))
                    .py_5()
                    .items_center()
                    .child(camera_art(model, 150.0, false)),
            )
            .child(
                text(move || tr(hint_key, &[]))
                    .text_size(px(15.0))
                    .line_height(px(22.0))
                    .text_color(move || theme().muted_foreground),
            )
            .child(
                view()
                    .w_full()
                    .gap_1()
                    .child(connection_button(tr("nearby.connect", &[]), {
                        let app = app.clone();
                        move || app.start_connect()
                    }))
                    .child(secondary_button(tr("nearby.not_now", &[]), move || {
                        app.dismiss_nearby()
                    })),
            ),
    )
}

fn scan(app: AppState) -> impl IntoElement {
    let cameras = app.nearby_cameras();
    let hint_key = app.pairing_hint_key();
    connection_shell(
        tr("scan.title", &[]),
        Some(tr("scan.subtitle", &[])),
        true,
        {
            let app = app.clone();
            move || app.set_sheet(None)
        },
        view()
            .w_full()
            .gap_4()
            .child(
                view()
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .bg(move || token("connection-muted"))
                    .children(cameras.into_iter().map(|cam| {
                        let app = app.clone();
                        let id = cam.peripheral_id.clone();
                        let model = cam.model;
                        let powered = cam.powered_on;
                        let status_label = if powered {
                            tr("nearby.powered_on", &[])
                        } else {
                            tr("scan.nearby_off", &[])
                        };
                        view()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .min_h(px(72.0))
                            .px_4()
                            .role(AccessibilityRole::Button)
                            .label(model.display_name())
                            .on_press(move || app.select_nearby(id.clone()))
                            .active(|s| s.bg(move || theme().accent))
                            .child(camera_art(model, 64.0, false))
                            .child(
                                view()
                                    .flex_1()
                                    .gap(px(2.0))
                                    .child(
                                        text(model.display_name())
                                            .text_size(px(17.0))
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(
                                        text(status_label)
                                            .text_size(px(13.0))
                                            .line_height(px(18.0))
                                            .text_color(move || theme().muted_foreground),
                                    ),
                            )
                    }))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .min_h(px(44.0))
                            .justify_center()
                            .px_4()
                            .child(
                                text(move || tr("scan.looking", &[]))
                                    .text_size(px(15.0))
                                    .line_height(px(22.0))
                                    .text_color(move || theme().muted_foreground),
                            ),
                    ),
            )
            .child(
                text(move || tr(hint_key, &[]))
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_color(move || theme().muted_foreground),
            ),
    )
}

fn connecting(app: AppState, step: u8) -> impl IntoElement {
    let model = format!("RICOH {}", app.model_name());
    connection_shell(
        tr("connecting.title", &[("model", &model)]),
        Some(tr("connecting.keep_on", &[])),
        true,
        move || app.cancel_connect(),
        view()
            .w_full()
            .gap_4()
            .items_center()
            .child(
                view()
                    .w_full()
                    .gap_1()
                    .child(connection_step(
                        &tr("connecting.bt", &[]),
                        None,
                        if step > 0 {
                            StepState::Done
                        } else {
                            StepState::Current
                        },
                        1,
                    ))
                    .child(connection_step(
                        &tr("connecting.wifi", &[]),
                        Some(&tr("connecting.wifi_sub", &[])),
                        if step > 1 {
                            StepState::Done
                        } else if step == 1 {
                            StepState::Current
                        } else {
                            StepState::Waiting
                        },
                        2,
                    ))
                    .child(connection_step(
                        &tr("connecting.version", &[]),
                        None,
                        if step >= 2 {
                            StepState::Current
                        } else {
                            StepState::Waiting
                        },
                        3,
                    )),
            )
            .child(secondary_button(tr("connecting.cancel", &[]), move || {
                app.cancel_connect()
            })),
    )
}

fn connect_error_sheet(app: AppState, error: LinkError) -> impl IntoElement {
    use crate::domain::ErrorAction;
    let title = link_error_title(&error);
    let body = link_error_body(&error);
    let action = error.copy().2;
    let retry = matches!(action, ErrorAction::Retry | ErrorAction::RetryOrSettings);
    let settings = matches!(action, ErrorAction::Settings | ErrorAction::RetryOrSettings);
    let open_settings = || {
        spawn(async {
            crate::platform::open_app_settings().await;
        });
    };
    sheet_shell(
        title,
        Some(body),
        None,
        false,
        || {},
        view()
            .w_full()
            .gap_1()
            .child(if retry {
                sheet_primary_button(tr("error.try_again", &[]), {
                    let app = app.clone();
                    move || app.retry_connect()
                })
                .into_element()
            } else if settings {
                sheet_primary_button(tr("conn.open_settings", &[]), open_settings).into_element()
            } else {
                view().into_element()
            })
            .child(
                (retry && settings)
                    .then(|| secondary_button(tr("conn.open_settings", &[]), open_settings)),
            )
            .child(secondary_button(tr("error.close", &[]), move || {
                app.dismiss_connect_error()
            })),
    )
}

fn sending(app: AppState) -> impl IntoElement {
    let model = app.model_name();
    let progress_val = match app.flow_phase() {
        UpdatePhase::Sending { progress, .. } => progress * 100.0,
        _ => 100.0,
    };
    let indeterminate = progress_val < 0.0;
    let rollback = matches!(
        app.flow_phase(),
        UpdatePhase::Sending {
            kind: UpdateKind::Rollback,
            ..
        }
    );
    let version = app
        .package()
        .and_then(|p| if rollback { p.rollback } else { p.available });
    sheet_shell(
        tr(
            if rollback {
                "roll.sending_title"
            } else {
                "sending.title"
            },
            &[("model", &model)],
        ),
        version.map(|version| format!("GRKit {version}")),
        Some(app.nearby_model().unwrap_or(CameraModel::Gr3)),
        false,
        || {},
        view()
            .w_full()
            .gap_4()
            .items_center()
            .child(
                view()
                    .w_full()
                    .gap_3()
                    .p_4()
                    .rounded(px(8.0))
                    .bg(move || theme().muted)
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                text(tr(
                                    if rollback {
                                        "roll.preparing"
                                    } else if indeterminate {
                                        "sending.preparing"
                                    } else {
                                        "sending.progress"
                                    },
                                    &[],
                                ))
                                .text_size(px(17.0))
                                .font_weight(FontWeight::SEMIBOLD),
                            )
                            .child((!indeterminate).then(|| {
                                text(format!("{progress_val:.0}%"))
                                    .text_size(px(15.0))
                                    .line_height(px(22.0))
                                    .text_color(move || theme().muted_foreground)
                            })),
                    )
                    .child((!indeterminate).then(|| {
                        progress()
                            .value(progress_val)
                            .accessibility_label(tr("sending.progress", &[]))
                            .track_style(|s| {
                                s.h(px(4.0))
                                    .rounded(px(3.0))
                                    .overflow_hidden()
                                    .bg(move || if dark() { rgb(0x3A3A3C) } else { rgb(0xE3E3E8) })
                            })
                            .indicator_style(|s| {
                                s.h(px(4.0)).rounded(px(3.0)).bg(move || token("tint"))
                            })
                    })),
            )
            .child(
                view()
                    .w_full()
                    .gap_1()
                    .child(checklist_row(
                        &tr("sending.step_confirm", &[]),
                        None,
                        StepState::Waiting,
                    ))
                    .child(checklist_row(
                        &tr("sending.step_restart", &[]),
                        None,
                        StepState::Waiting,
                    )),
            )
            .child(
                text(move || tr("sending.keep_on", &[]))
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_color(move || theme().muted_foreground),
            )
            .child(secondary_button(tr("sending.cancel", &[]), move || {
                app.cancel_update()
            })),
    )
}

fn confirm(app: AppState) -> impl IntoElement {
    let rollback = matches!(
        app.flow_phase(),
        UpdatePhase::WaitingConfirm {
            kind: UpdateKind::Rollback,
            ..
        }
    );
    let title = if rollback {
        tr("roll.confirm_title", &[])
    } else {
        tr("confirm.title", &[])
    };
    let subtitle = if rollback {
        tr("roll.confirm_subtitle", &[])
    } else {
        tr("confirm.subtitle", &[])
    };
    sheet_shell(
        title,
        Some(subtitle),
        None,
        false,
        || {},
        view()
            .w_full()
            .gap_4()
            .child(installation_steps())
            .child(
                view()
                    .w_full()
                    .p_4()
                    .rounded(px(8.0))
                    .bg(move || theme().muted)
                    .child(
                        text(move || {
                            tr(
                                "confirm.waiting",
                                &[("time", &app.confirm_deadline_label())],
                            )
                        })
                        .text_size(px(15.0))
                        .line_height(px(22.0))
                        .text_color(move || token("link"))
                        .text_center(),
                    ),
            )
            .child(secondary_button(tr("sending.cancel", &[]), move || {
                app.cancel_update()
            })),
    )
}

// The guide has a fixed cross-axis extent. Native iOS scroll views can otherwise
// move vertically when their content exceeds their height, even in horizontal mode.
const GUIDE_WIDTH: f32 = 300.0;
const GUIDE_HEIGHT: f32 = 258.0;
const GUIDE_STRIDE: f32 = GUIDE_WIDTH + 12.0;

fn installation_steps() -> impl IntoElement {
    let index = signal(0u32);
    let track = element_ref();
    view()
        .gap_1()
        .child(
            scroll()
                .element_ref(&track)
                .horizontal()
                .scrollbar(false)
                .bounces(false)
                .snap_interval(Points(GUIDE_STRIDE))
                .disable_interval_momentum(true)
                .deceleration(0.99)
                .h(px(GUIDE_HEIGHT))
                .mx(px(-20.0))
                .px(px(26.0))
                .gap(px(12.0))
                .label("Steps on the camera")
                .on_scroll_end(move |offset: &Point<Points>| {
                    index.set((offset.x.0 / GUIDE_STRIDE).round().clamp(0.0, 2.0) as u32);
                })
                .child(step_slide(STEP1, 1, tr("confirm.step1", &[])))
                .child(step_slide(STEP2, 2, tr("confirm.step2", &[])))
                .child(step_slide(STEP3, 3, tr("confirm.step3", &[]))),
        )
        .child(
            view()
                .flex_row()
                .justify_center()
                .child(page_dot(0, index, track))
                .child(page_dot(1, index, track))
                .child(page_dot(2, index, track)),
        )
}

fn page_dot(i: u32, index: Signal<u32>, track: ElementRef) -> impl IntoElement {
    let caption_key = ["confirm.step1", "confirm.step2", "confirm.step3"][i as usize];
    view()
        .size(px(touch_target()))
        .items_center()
        .justify_center()
        .role(AccessibilityRole::Button)
        .label(format!("{}: {}", i + 1, tr(caption_key, &[])))
        .on_press(move || {
            index.set(i);
            track.scroll_to_offset(Points(i as f32 * GUIDE_STRIDE), !reduce_motion());
        })
        .child(view().size(px(7.0)).rounded_full().bg(move || {
            if index.get() == i {
                token("tint")
            } else {
                rgb(0xC7C7CC).into()
            }
        }))
}

fn step_slide(asset: ImageAsset, number: u8, caption: String) -> impl IntoElement {
    view()
        .w(px(GUIDE_WIDTH))
        .h(px(GUIDE_HEIGHT))
        .overflow_hidden()
        .flex_shrink_0()
        .child(
            view()
                .gap(px(10.0))
                .child(
                    view().p(px(6.0)).rounded(px(10.0)).bg(rgb(0x111113)).child(
                        image(asset)
                            .w_full()
                            .aspect_ratio(3.0 / 2.0)
                            .rounded(px(16.0))
                            .object_fit(ResizeMode::Cover),
                    ),
                )
                .child(
                    view()
                        .flex_row()
                        .h(px(44.0))
                        .items_center()
                        .gap(px(10.0))
                        .px_1()
                        .child(
                            view()
                                .size(px(24.0))
                                .rounded_full()
                                .items_center()
                                .justify_center()
                                .bg(move || token("tint"))
                                .child(
                                    text(format!("{number}"))
                                        .text_size(px(13.0))
                                        .line_height(px(18.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(move || theme().background),
                                ),
                        )
                        .child(
                            text(caption)
                                .flex_1()
                                .line_height(px(22.0))
                                .number_of_lines(2u32)
                                .text_size(px(17.0))
                                .font_weight(FontWeight::MEDIUM),
                        ),
                ),
        )
}

fn restart(app: AppState) -> impl IntoElement {
    sheet_shell(
        tr("restart.title", &[]),
        Some(tr("restart.subtitle", &[])),
        Some(app.nearby_model().unwrap_or(CameraModel::Gr3)),
        false,
        || {},
        view()
            .w_full()
            .gap_4()
            .items_center()
            .child(steps_box(2))
            .child(
                view()
                    .w_full()
                    .gap_1()
                    .child(sheet_primary_button(tr("restart.done", &[]), {
                        let app = app.clone();
                        move || app.finish_restart()
                    }))
                    .child(secondary_button(
                        tr(
                            if matches!(
                                app.flow_phase(),
                                UpdatePhase::Restart {
                                    kind: UpdateKind::Rollback
                                }
                            ) {
                                "roll.cancel_pending"
                            } else {
                                "restart.cancel"
                            },
                            &[],
                        ),
                        move || app.cancel_update(),
                    )),
            ),
    )
}

fn installed(app: AppState, version: String, _previous: String) -> impl IntoElement {
    let rollback = matches!(
        app.flow_phase(),
        UpdatePhase::Installed {
            kind: UpdateKind::Rollback,
            ..
        }
    );
    let subtitle = if rollback {
        tr("roll.installed_subtitle", &[])
    } else if let Some(previous) = app.package().and_then(|package| package.rollback) {
        tr("installed.subtitle", &[("previous", &previous)])
    } else {
        tr("installed.verified", &[])
    };
    sheet_shell(
        tr(
            if rollback {
                "roll.installed_title"
            } else {
                "installed.title"
            },
            &[("version", &version)],
        ),
        Some(subtitle),
        None,
        false,
        || {},
        view()
            .w_full()
            .gap_5()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .py_3()
                    .child(camera_art(
                        app.nearby_model().unwrap_or(CameraModel::Gr3),
                        120.0,
                        false,
                    ))
                    .child(
                        view()
                            .size(px(32.0))
                            .rounded_full()
                            .items_center()
                            .justify_center()
                            .bg(move || token("success-soft"))
                            .text_color(move || token("success"))
                            .child(connection_icon("M5 12l4 4L19 6")),
                    ),
            )
            .child(sheet_primary_button(tr("installed.done", &[]), move || {
                app.dismiss_installed()
            })),
    )
}

fn error_sheet(app: AppState, error: UpdateError) -> impl IntoElement {
    let (title, body, retry_label, retry) = match &error {
        UpdateError::Service(message) => (
            tr("error.service_title", &[]),
            message.clone(),
            tr("error.try_again", &[]),
            true,
        ),
        UpdateError::Download(message) => (
            tr("error.download_title", &[]),
            message.clone(),
            tr("error.try_again", &[]),
            true,
        ),
        UpdateError::CardFull { bytes_needed } => (
            tr("error.card_full_title", &[]),
            tr(
                "error.card_full_body",
                &[("size", &format_bytes(*bytes_needed))],
            ),
            tr("error.try_again", &[]),
            true,
        ),
        UpdateError::NotConfirmed => (
            tr("error.not_confirmed_title", &[]),
            tr("error.not_confirmed_body", &[]),
            tr("error.send_again", &[]),
            true,
        ),
        UpdateError::Refused => (
            tr("error.refused_title", &[]),
            tr("error.refused_body", &[]),
            tr("error.try_again", &[]),
            true,
        ),
        UpdateError::ConnectionLost => (
            tr("error.lost_title", &[]),
            tr("error.lost_body", &[]),
            tr("error.try_again", &[]),
            false,
        ),
    };
    sheet_shell(
        title,
        Some(body),
        None,
        false,
        || {},
        view()
            .w_full()
            .gap_1()
            .child(retry.then(|| {
                sheet_primary_button(retry_label, {
                    let app = app.clone();
                    move || app.retry_update()
                })
            }))
            .child((!app.demo() && app.has_pending_update()).then(|| {
                secondary_button(tr("error.cancel_pending", &[]), move || app.cancel_update())
            }))
            .child(secondary_button(tr("error.close", &[]), move || {
                app.dismiss_error()
            })),
    )
}

fn steps_box(current: u8) -> impl IntoElement {
    view()
        .w_full()
        .rounded(px(10.0))
        .bg(move || theme().muted)
        .child(checklist_row(
            &tr("sending.step_send", &[]),
            None,
            step_state(current, 0),
        ))
        .child(checklist_row(
            &tr("sending.step_confirm", &[]),
            None,
            step_state(current, 1),
        ))
        .child(checklist_row(
            &tr("sending.step_restart", &[]),
            None,
            step_state(current, 2),
        ))
}

fn step_state(current: u8, index: u8) -> StepState {
    if current > index {
        StepState::Done
    } else if current == index {
        StepState::Current
    } else {
        StepState::Waiting
    }
}
