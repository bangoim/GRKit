//! Modal sheets for pairing, connecting, update progress, and errors.

use nsh_base::{
    SheetEvent, carousel, carousel_content, carousel_item, carousel_pagination,
    carousel_pagination_item, progress, sheet, theme,
};
use nsh_core::prelude::*;

use crate::domain::{CameraModel, UpdateError, UpdateKind, UpdatePhase};
use crate::store::{AppState, Sheet};
use crate::strings::{format_bytes, tr};
use crate::ui::{
    STEP1, STEP2, STEP3, StepState, camera_art, checklist_row, secondary_button,
    sheet_primary_button, sheet_shell, sheet_surface,
};

pub fn overlay(app: AppState) -> impl IntoElement {
    let dismissible = {
        let app = app.clone();
        move || matches!(app.sheet(), Some(Sheet::Nearby | Sheet::Scan))
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
            s.mx(px(8.0))
                .mb_safe_area_plus(px(8.0))
                .rounded(px(40.0))
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
                if let SheetEvent::OpenChange(false, _) = e
                    && matches!(app.sheet(), Some(Sheet::Nearby | Sheet::Scan))
                {
                    app.set_sheet(None);
                }
            }
        })
        .child(dynamic({
            let app = app.clone();
            move || match app.sheet() {
                Some(Sheet::Nearby) => nearby(app.clone()).into_element(),
                Some(Sheet::Scan) => scan(app.clone()).into_element(),
                Some(Sheet::Connecting { step }) => connecting(app.clone(), step).into_element(),
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

fn nearby(app: AppState) -> impl IntoElement {
    let model = app.nearby_model().unwrap_or(CameraModel::Gr3);
    sheet_shell(
        model.display_name(),
        Some(tr("nearby.powered_on", &[])),
        true,
        {
            let app = app.clone();
            move || app.set_sheet(None)
        },
        view()
            .w_full()
            .gap_5()
            .items_center()
            .child(camera_art(model, 220.0, false))
            .child(
                text(move || tr("nearby.hint", &[]))
                    .text_size(px(14.0))
                    .text_color(move || theme().muted_foreground)
                    .text_center(),
            )
            .child(
                view()
                    .w_full()
                    .gap_1()
                    .child(sheet_primary_button(tr("nearby.connect", &[]), {
                        let app = app.clone();
                        move || app.start_connect()
                    }))
                    .child(secondary_button(tr("nearby.not_now", &[]), move || {
                        app.set_sheet(None)
                    })),
            ),
    )
}

fn scan(app: AppState) -> impl IntoElement {
    let nearby = app.nearby_model();
    sheet_shell(
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
                    .rounded(px(22.0))
                    .overflow_hidden()
                    .bg(move || theme().muted)
                    .child(nearby.map(|model| {
                        let app = app.clone();
                        view()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .min_h(px(72.0))
                            .px_4()
                            .role(AccessibilityRole::Button)
                            .label(model.display_name())
                            .on_press(move || app.start_connect())
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
                                        text(move || tr("nearby.powered_on", &[]))
                                            .text_size(px(13.0))
                                            .text_color(move || theme().muted_foreground),
                                    ),
                            )
                    }))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .min_h(px(52.0))
                            .px_4()
                            .child(
                                text(move || tr("scan.looking", &[]))
                                    .text_size(px(15.0))
                                    .text_color(move || theme().muted_foreground),
                            ),
                    ),
            )
            .child(
                text(move || tr("scan.hint", &[]))
                    .text_size(px(13.0))
                    .text_color(move || theme().muted_foreground),
            ),
    )
}

fn connecting(app: AppState, step: u8) -> impl IntoElement {
    let model = app.model_name();
    sheet_shell(
        tr("connecting.title", &[("model", &model)]),
        Some(tr("connecting.keep_on", &[])),
        false,
        || {},
        view()
            .w_full()
            .gap_4()
            .items_center()
            .child(camera_art(
                app.nearby_model().unwrap_or(CameraModel::Gr3),
                200.0,
                false,
            ))
            .child(
                view()
                    .w_full()
                    .rounded(px(22.0))
                    .bg(move || theme().muted)
                    .child(checklist_row(
                        &tr("connecting.bt", &[]),
                        None,
                        if step > 0 {
                            StepState::Done
                        } else {
                            StepState::Current
                        },
                    ))
                    .child(checklist_row(
                        &tr("connecting.wifi", &[]),
                        Some(&tr("connecting.wifi_sub", &[])),
                        if step > 1 {
                            StepState::Done
                        } else if step == 1 {
                            StepState::Current
                        } else {
                            StepState::Waiting
                        },
                    ))
                    .child(checklist_row(
                        &tr("connecting.version", &[]),
                        None,
                        if step >= 2 {
                            StepState::Current
                        } else {
                            StepState::Waiting
                        },
                    )),
            ),
    )
}

fn sending(app: AppState) -> impl IntoElement {
    let model = app.model_name();
    let progress_val = match app.flow_phase() {
        UpdatePhase::Sending { progress, .. } => progress * 100.0,
        _ => 100.0,
    };
    let version = app
        .package()
        .and_then(|p| p.available)
        .unwrap_or_else(|| "1.7.0".into());
    sheet_shell(
        tr("sending.title", &[("model", &model)]),
        Some(format!("GRKit {version}")),
        false,
        || {},
        view()
            .w_full()
            .gap_4()
            .items_center()
            .child(camera_art(
                app.nearby_model().unwrap_or(CameraModel::Gr3),
                200.0,
                false,
            ))
            .child(
                view()
                    .w_full()
                    .gap_2()
                    .px_1()
                    .child(
                        progress()
                            .value(progress_val)
                            .accessibility_label(tr("sending.progress", &[]))
                            .track_style(|s| {
                                s.h(px(6.0))
                                    .rounded(px(3.0))
                                    .overflow_hidden()
                                    .bg(move || if dark() { rgb(0x3A3A3C) } else { rgb(0xE3E3E8) })
                            })
                            .indicator_style(|s| {
                                s.h(px(6.0)).rounded(px(3.0)).bg(move || theme().foreground)
                            }),
                    )
                    .child(
                        text(move || tr("sending.progress", &[]))
                            .text_size(px(15.0))
                            .text_color(move || theme().muted_foreground)
                            .text_center(),
                    ),
            )
            .child(steps_box(0))
            .child(
                text(move || tr("sending.keep_on", &[]))
                    .text_size(px(13.0))
                    .text_color(move || theme().muted_foreground),
            ),
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
    let index = signal(0u32);
    let time = app.confirm_deadline_label();
    sheet_shell(
        title,
        Some(subtitle),
        false,
        || {},
        view()
            .w_full()
            .gap_4()
            .child(
                carousel()
                    .index(move || Some(index.get()))
                    .on_change(move |i: &u32| index.set(*i))
                    .gap(px(12.0))
                    .accessibility_label("Steps on the camera")
                    .child(
                        carousel_content()
                            .mx(px(-20.0))
                            .px(px(26.0))
                            .gap(px(12.0))
                            .child(step_slide(STEP1, 1, tr("confirm.step1", &[])))
                            .child(step_slide(STEP2, 2, tr("confirm.step2", &[])))
                            .child(step_slide(STEP3, 3, tr("confirm.step3", &[]))),
                    )
                    .child(
                        carousel_pagination()
                            .flex_row()
                            .justify_center()
                            .gap(px(7.0))
                            .child(page_dot(0))
                            .child(page_dot(1))
                            .child(page_dot(2)),
                    ),
            )
            .child(
                text(tr("confirm.waiting", &[("time", &time)]))
                    .text_size(px(15.0))
                    .text_color(move || theme().muted_foreground)
                    .text_center(),
            ),
    )
}

fn page_dot(i: u32) -> impl IntoElement {
    carousel_pagination_item()
        .index(i)
        .size(px(7.0))
        .rounded_full()
        .bg(rgb(0xC7C7CC))
        .selected_style(|s| s.bg(move || theme().foreground))
}

fn step_slide(asset: ImageAsset, number: u8, caption: String) -> impl IntoElement {
    carousel_item().w(px(300.0)).child(
        view()
            .gap(px(10.0))
            .child(
                view().p(px(6.0)).rounded(px(22.0)).bg(rgb(0x111113)).child(
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
                    .items_center()
                    .gap(px(10.0))
                    .px_1()
                    .child(
                        view()
                            .size(px(24.0))
                            .rounded_full()
                            .items_center()
                            .justify_center()
                            .bg(move || theme().foreground)
                            .child(
                                text(format!("{number}"))
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(move || theme().background),
                            ),
                    )
                    .child(
                        text(caption)
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
        false,
        || {},
        view()
            .w_full()
            .gap_4()
            .items_center()
            .child(camera_art(
                app.nearby_model().unwrap_or(CameraModel::Gr3),
                200.0,
                false,
            ))
            .child(steps_box(2))
            .child(
                view()
                    .w_full()
                    .gap_1()
                    .child(sheet_primary_button(tr("restart.done", &[]), {
                        let app = app.clone();
                        move || app.finish_restart()
                    }))
                    .child(secondary_button(tr("restart.cancel", &[]), move || {
                        app.cancel_update()
                    })),
            ),
    )
}

fn installed(app: AppState, version: String, previous: String) -> impl IntoElement {
    view()
        .w_full()
        .items_center()
        .gap(px(22.0))
        .pt(px(30.0))
        .px(px(20.0))
        .pb(px(22.0))
        .child(
            view()
                .relative()
                .child(camera_art(
                    app.nearby_model().unwrap_or(CameraModel::Gr3),
                    220.0,
                    false,
                ))
                .child(
                    view()
                        .absolute()
                        .right(px(-6.0))
                        .bottom(px(-6.0))
                        .size(px(44.0))
                        .rounded_full()
                        .items_center()
                        .justify_center()
                        .border(px(4.0))
                        .border_color(sheet_surface)
                        .bg(rgb(0x248A3D))
                        .child(
                            text("✓")
                                .text_size(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0xffffff)),
                        ),
                ),
        )
        .child(
            view()
                .items_center()
                .gap(px(6.0))
                .max_w(px(300.0))
                .child(
                    text(tr("installed.title", &[("version", &version)]))
                        .text_size(px(22.0))
                        .font_weight(FontWeight::BOLD)
                        .text_center(),
                )
                .child(
                    text(tr("installed.subtitle", &[("previous", &previous)]))
                        .text_size(px(15.0))
                        .text_color(move || theme().muted_foreground)
                        .text_center(),
                ),
        )
        .child(sheet_primary_button(tr("installed.done", &[]), move || {
            app.dismiss_installed()
        }))
}

fn error_sheet(app: AppState, error: UpdateError) -> impl IntoElement {
    let (title, body, retry_label, retry) = match &error {
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
            .child(secondary_button(tr("error.close", &[]), move || {
                app.dismiss_error()
            })),
    )
}

fn steps_box(current: u8) -> impl IntoElement {
    view()
        .w_full()
        .rounded(px(22.0))
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
