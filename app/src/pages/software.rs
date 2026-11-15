//! Software Update page (push) for install and roll-back entry.

use nsh_base::{alert_dialog, nav_page, theme};
use nsh_core::prelude::*;

use crate::domain::UpdateKind;
use crate::store::{AppState, is_up_to_date};
use crate::strings::tr;
use crate::ui::{gr_mark, grouped_card, info_row, primary_button, token};

pub fn software_page(app: AppState, on_dismiss: impl Fn() + 'static) -> impl IntoElement {
    let package = app.package();
    let up_to_date = package.as_ref().is_some_and(is_up_to_date);
    let available = package
        .as_ref()
        .and_then(|p| p.available.clone())
        .unwrap_or_else(|| "1.7.0".into());
    let installed = package
        .as_ref()
        .map(|p| p.installed.clone())
        .unwrap_or_default();
    let rollback = package.as_ref().and_then(|p| p.rollback.clone());
    let notes = package
        .as_ref()
        .map(|p| p.release_notes.clone())
        .unwrap_or_default();
    let model = app.model_name();
    let mb = package
        .as_ref()
        .map(|p| format!("{:.1} MB", p.package_bytes as f64 / 1_000_000.0))
        .unwrap_or_else(|| "4.7 MB".into());

    nav_page()
        .title(move || tr("software.title", &[]))
        .on_dismiss(on_dismiss)
        .child(
            view()
                .flex_1()
                .bg(move || token("grouped"))
                .child(
                    scroll().flex_1().child(
                        view()
                            .gap_5()
                            .p_4()
                            .pb_10()
                            .child(header_card(
                                if up_to_date {
                                    installed.clone()
                                } else {
                                    available.clone()
                                },
                                if up_to_date {
                                    tr("software.up_to_date", &[])
                                } else {
                                    format!("{model} · {mb}")
                                },
                                notes,
                                up_to_date,
                            ))
                            .child(if up_to_date {
                                view().into_element()
                            } else {
                                grouped_card(
                                    view()
                                        .child(info_row(
                                            tr("software.installed", &[]),
                                            installed.clone(),
                                            false,
                                        ))
                                        .child(
                                            view().px_4().py_3().child(
                                                text(move || tr("software.looks_stay", &[]))
                                                    .text_size(px(13.0))
                                                    .text_color(move || theme().muted_foreground),
                                            ),
                                        ),
                                )
                                .into_element()
                            })
                            .child(rollback.map(|version| {
                                let app_roll = app.clone();
                                let ver = version.clone();
                                grouped_card(
                                    view()
                                        .child(
                                            view()
                                                .flex_row()
                                                .items_center()
                                                .min_h(px(52.0))
                                                .px_4()
                                                .role(AccessibilityRole::Button)
                                                .label(tr(
                                                    "software.roll_back_to",
                                                    &[("version", &version)],
                                                ))
                                                .on_press(move || {
                                                    confirm_rollback(app_roll.clone(), ver.clone())
                                                })
                                                .child(
                                                    text(tr(
                                                        "software.roll_back_to",
                                                        &[("version", &version)],
                                                    ))
                                                    .text_size(px(17.0))
                                                    .text_color(move || theme().destructive),
                                                ),
                                        )
                                        .child(
                                            view().px_4().pb_3().child(
                                                text(move || tr("software.roll_footnote", &[]))
                                                    .text_size(px(13.0))
                                                    .text_color(move || theme().muted_foreground),
                                            ),
                                        ),
                                )
                            })),
                    ),
                )
                .child((!up_to_date).then(|| {
                    view()
                        .px_4()
                        .pb_8()
                        .gap_2()
                        .child(primary_button(tr("software.update_now", &[]), {
                            let app = app.clone();
                            move || app.begin_update(UpdateKind::Install)
                        }))
                        .child(
                            text(move || tr("software.confirm_note", &[]))
                                .text_size(px(13.0))
                                .text_color(move || theme().muted_foreground)
                                .text_center(),
                        )
                })),
        )
}

fn header_card(
    version: String,
    subtitle: String,
    notes: String,
    up_to_date: bool,
) -> impl IntoElement {
    grouped_card(
        view()
            .p_4()
            .gap_3()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(gr_mark(60.0))
                    .child(
                        view()
                            .flex_1()
                            .gap(px(2.0))
                            .child(
                                text(format!("GRKit {version}"))
                                    .text_size(px(20.0))
                                    .font_weight(FontWeight::BOLD),
                            )
                            .child(
                                text(subtitle)
                                    .text_size(px(14.0))
                                    .text_color(move || theme().muted_foreground),
                            ),
                    ),
            )
            .child((!up_to_date && !notes.is_empty()).then(|| {
                view()
                    .gap_2()
                    .child(
                        text(notes)
                            .text_size(px(15.0))
                            .text_color(move || theme().foreground),
                    )
                    .child(
                        text(move || tr("software.learn_more", &[]))
                            .text_size(px(15.0))
                            .text_color(move || theme().muted_foreground),
                    )
            })),
    )
}

fn confirm_rollback(app: AppState, version: String) {
    spawn(async move {
        let answer = alert_dialog()
            .title(tr("roll.alert_title", &[("version", &version)]))
            .description(tr("roll.alert_body", &[]))
            .cancel("cancel", tr("roll.cancel", &[]))
            .destructive("rollback", tr("roll.confirm", &[]))
            .show()
            .await;
        if matches!(answer, Ok(r) if r.action.as_deref() == Some("rollback")) {
            app.begin_update(UpdateKind::Rollback);
        }
    });
}
