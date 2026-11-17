//! Pages pushed from the Looks tab: one Look, rename, menu order, import.

use nsh_base::{alert_dialog, input, nav_page, theme};
use nsh_core::AccessibilityActionDef;
use nsh_core::prelude::*;

use crate::domain::{Look, LookSource, group_of, looks::NAME_MAX};
use crate::store::{AppState, LookPicture, Route};
use crate::strings::tr;
use crate::ui::{
    action_row, badge, control_row, footnote, grouped_card, picture, primary_button,
    secondary_button, toggle, token,
};

fn full_picture(picture_: Option<LookPicture>, faded: bool) -> AnyElement {
    let pixels = picture_.map(|p| {
        let f = p.full();
        (f.width, f.height, f.rgba.clone())
    });
    // Phone width less the page margins, at the sample's 3:2.
    picture(pixels, 345.0, 230.0, 10.0, faded)
}

fn icon_view(icon: Option<crate::domain::look_media::Picture>) -> AnyElement {
    picture(
        icon.map(|i| (i.width, i.height, i.rgba)),
        28.0,
        28.0,
        6.0,
        false,
    )
}

// --- One Look ---------------------------------------------------------------

pub fn look_page(app: AppState, id: String, on_dismiss: impl Fn() + 'static) -> impl IntoElement {
    let title_id = id.clone();
    nav_page()
        .title(move || app.look(&title_id).map(|l| l.name).unwrap_or_default())
        .on_dismiss(on_dismiss)
        .child(dynamic(move || match app.look(&id) {
            Some(look) => look_content(app, look).into_element(),
            None => view().flex_1().bg(move || token("grouped")).into_element(),
        }))
}

fn look_content(app: AppState, look: Look) -> impl IntoElement {
    let busy = app.look_busy().is_some();
    let blocked = app.looks_blocked().is_some();
    let editable = !busy && !blocked;
    let kind = crate::pages::looks::look_kind(&look);
    let status = if look.selected {
        tr("looks.in_use", &[])
    } else if !look.enabled {
        tr("looks.off", &[])
    } else if look.loaded {
        tr("looks.loaded", &[])
    } else {
        tr("looks.not_loaded", &[])
    };
    let source = match look.source {
        LookSource::Official => tr("looks.official", &[]),
        LookSource::Custom => tr("looks.imported", &[]),
        LookSource::Unknown => tr("looks.unknown", &[]),
    };
    let used_by = match look.dependencies.len() {
        0 => tr("looks.none", &[]),
        1 => tr("looks.recipes_one", &[]),
        n => tr("looks.recipes_many", &[("count", &n.to_string())]),
    };

    let mut settings = view().child(control_row(
        tr("looks.show_in_menu", &[]),
        toggle(look.enabled, tr("looks.show_in_menu", &[]), editable, {
            let (id, enabled) = (look.id.clone(), look.enabled);
            move || app.set_look_enabled(id.clone(), !enabled)
        }),
        false,
    ));
    if look.editable() {
        let id = look.id.clone();
        settings = settings
            .child(
                view()
                    .role(AccessibilityRole::Button)
                    .label(format!("{}, {}", tr("looks.name", &[]), look.name))
                    .when(editable, |v| {
                        v.on_press(move || app.push(Route::LookRename(id.clone())))
                    })
                    .child(control_row(
                        tr("looks.name", &[]),
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                text(look.name.clone())
                                    .text_size(px(17.0))
                                    .text_color(move || theme().muted_foreground),
                            )
                            .child(
                                text("›")
                                    .text_size(px(22.0))
                                    .text_color(move || theme().muted_foreground),
                            ),
                        false,
                    )),
            )
            .child(control_row(
                tr("looks.cover", &[]),
                dynamic({
                    let id = look.id.clone();
                    move || icon_view(app.look_icon(&id))
                }),
                false,
            ));
    } else {
        settings = settings.child(control_row(
            tr("looks.group", &[]),
            text(tr(group_of(&look.id, false).label_key(), &[]))
                .text_size(px(17.0))
                .text_color(move || theme().muted_foreground),
            false,
        ));
    }
    settings = settings.child(control_row(
        tr("looks.used_by", &[]),
        text(used_by)
            .text_size(px(17.0))
            .text_color(move || theme().muted_foreground),
        true,
    ));

    let actions = if look.editable() {
        let id = look.id.clone();
        view().child(action_row(
            tr("looks.replace", &[]),
            false,
            false,
            move || {
                if editable {
                    app.begin_replace(id.clone())
                }
            },
        ))
    } else {
        let id = look.id.clone();
        view().child(action_row(
            tr("looks.make_copy", &[]),
            false,
            false,
            move || {
                if editable {
                    app.copy_look(id.clone())
                }
            },
        ))
    };
    let actions = actions.child(action_row(tr("looks.delete", &[]), true, true, {
        let look = look.clone();
        move || {
            if editable {
                confirm_delete(app, look.clone())
            }
        }
    }));

    view().flex_1().bg(move || token("grouped")).child(
        crate::ui::page_scroll().flex_1().child(
            view()
                .px_6()
                .pt_4()
                .pb(px(TAB_BAR_CLEARANCE))
                .gap_4()
                .child(dynamic({
                    let hash = look.content_hash.clone();
                    move || full_picture(app.look_picture(&hash), false)
                }))
                .child(
                    view()
                        .flex_row()
                        .items_start()
                        .justify_between()
                        .gap_3()
                        .child(
                            view()
                                .flex_1()
                                .gap(px(2.0))
                                .child(
                                    text(look.name.clone())
                                        .text_size(px(24.0))
                                        .font_weight(FontWeight::BOLD),
                                )
                                .child(
                                    text(if look.editable() {
                                        format!("{source} · {status}")
                                    } else {
                                        format!("{source} · {kind} · {status}")
                                    })
                                    .text_size(px(15.0))
                                    .text_color(move || theme().muted_foreground),
                                ),
                        )
                        .child(look.selected.then(|| badge(tr("looks.in_use", &[])))),
                )
                .child(
                    view()
                        .gap_2()
                        .child(grouped_card(settings))
                        .child(footnote(tr("looks.show_note", &[]))),
                )
                .child(
                    view()
                        .gap_2()
                        .child(grouped_card(actions))
                        .child((!look.editable()).then(|| footnote(tr("looks.copy_note", &[])))),
                )
                .child(footnote(tr("looks.sample_credit", &[]))),
        ),
    )
}

/// Ask before deleting. A Look that recipes use cannot be deleted; the Look
/// in use is deleted with the camera switching to Off.
pub fn confirm_delete(app: AppState, look: Look) {
    spawn(async move {
        if !look.dependencies.is_empty() {
            let _ = alert_dialog()
                .title(tr("looks.cannot_delete_title", &[("name", &look.name)]))
                .description(tr("looks.cannot_delete_body", &[]))
                .cancel("ok", tr("looks.ok", &[]))
                .show()
                .await;
            return;
        }
        let body = if look.selected {
            tr("looks.delete_active_body", &[])
        } else {
            tr("looks.delete_body", &[])
        };
        let answer = alert_dialog()
            .title(tr("looks.delete_title", &[("name", &look.name)]))
            .description(body)
            .cancel("cancel", tr("looks.cancel", &[]))
            .destructive("delete", tr("looks.delete_action", &[]))
            .show()
            .await;
        if matches!(answer, Ok(r) if r.action.as_deref() == Some("delete")) {
            app.delete_look(look.id.clone(), look.selected);
        }
    });
}

// --- Rename -----------------------------------------------------------------

pub fn rename_page(app: AppState, id: String, on_dismiss: impl Fn() + 'static) -> impl IntoElement {
    let name = signal(app.look(&id).map(|l| l.name).unwrap_or_default());
    let save = {
        let id = id.clone();
        move || app.rename_look(id.clone(), name.get())
    };
    nav_page()
        .title(tr("looks.rename_title", &[]))
        .on_dismiss(on_dismiss)
        .header_right(header_text_button(tr("looks.save", &[]), save.clone()))
        .child(
            view()
                .flex_1()
                .bg(move || token("grouped"))
                .px_6()
                .pt_4()
                .gap_2()
                .child(name_field(name, move |value| name.set(value), save))
                .child(footnote(tr("looks.name_note", &[]))),
        )
}

fn header_text_button(title: String, on_press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .min_h(px(44.0))
        .px_2()
        .justify_center()
        .role(AccessibilityRole::Button)
        .label(title.clone())
        .on_press(on_press)
        .child(
            text(title)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || token("link")),
        )
}

fn name_field(
    name: Signal<String>,
    on_change: impl Fn(String) + 'static,
    on_submit: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .gap_2()
        .child(crate::ui::section_title(tr("looks.name", &[])))
        .child(
            view()
                .flex_row()
                .items_center()
                .h(px(50.0))
                .px_4()
                .gap_2()
                .rounded(px(10.0))
                .border_1()
                .border_color(move || theme().border)
                .bg(move || theme().card)
                .child(
                    input()
                        .flex_1()
                        .value(move || name.get())
                        .max_length(NAME_MAX as u32)
                        .auto_correct(false)
                        .label(tr("looks.name", &[]))
                        .text_size(px(17.0))
                        .on_change(move |value: &str| on_change(value.to_owned()))
                        .on_submit(move |_: &str| on_submit()),
                )
                .child(
                    text(move || format!("{}/{NAME_MAX}", name.get().len()))
                        .text_size(px(15.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

// --- Menu order ---------------------------------------------------------------

const ORDER_ROW: f32 = 58.0;

/// Room under scrolled content for the floating tab bar.
pub const TAB_BAR_CLEARANCE: f32 = 96.0;

pub fn order_page(app: AppState, on_dismiss: impl Fn() + 'static) -> impl IntoElement {
    nav_page()
        .title(tr("looks.order_title", &[]))
        .on_dismiss(on_dismiss)
        .header_right(header_text_button(tr("looks.done", &[]), move || {
            app.save_look_order()
        }))
        .child(
            view().flex_1().bg(move || token("grouped")).child(
                crate::ui::page_scroll().flex_1().child(
                    view()
                        .px_6()
                        .pt_4()
                        .pb_8()
                        .gap_3()
                        .child(footnote(tr("looks.order_note", &[])))
                        .child(dynamic(move || order_list(app))),
                ),
            ),
        )
}

fn order_list(app: AppState) -> impl IntoElement {
    let ids = app.editing_order();
    let snapshot = app.looks_snapshot();
    let dragging = signal(None::<usize>);
    let offset = signal(0.0f32);
    let count = ids.len();
    let mut rows = view()
        .rounded(px(10.0))
        .border_1()
        .border_color(move || theme().border)
        .bg(move || theme().card);
    for (index, id) in ids.iter().enumerate() {
        let Some(look) = snapshot.as_ref().and_then(|s| s.get(id)).cloned() else {
            continue;
        };
        let lifted = move || dragging.get() == Some(index);
        let off = !look.enabled;
        let handle = view()
            .size(px(44.0))
            .items_center()
            .justify_center()
            .role(AccessibilityRole::Adjustable)
            .label(tr("looks.move", &[("name", &look.name)]))
            .accessibility_actions(vec![
                AccessibilityActionDef {
                    name: "increment".into(),
                    label: Some(tr("looks.move_down", &[]).into()),
                },
                AccessibilityActionDef {
                    name: "decrement".into(),
                    label: Some(tr("looks.move_up", &[]).into()),
                },
            ])
            .on_accessibility_action(move |action: &str| match action {
                "increment" => app.move_look_in_order(index, (index + 1).min(count - 1)),
                "decrement" => app.move_look_in_order(index, index.saturating_sub(1)),
                _ => {}
            })
            .claim_touches(TouchClaim::on_start())
            .on_responder_grant(move |_: &ResponderEvent| {
                dragging.set(Some(index));
                offset.set(0.0);
            })
            .on_responder_move(move |event: &ResponderEvent| {
                offset.set(event.gesture.delta.y.0);
            })
            .on_responder_release(move |_: &ResponderEvent| {
                let steps = (offset.get() / ORDER_ROW).round() as isize;
                let target = (index as isize + steps).clamp(0, count as isize - 1) as usize;
                dragging.set(None);
                offset.set(0.0);
                app.move_look_in_order(index, target);
            })
            .on_responder_terminate(move |_: &ResponderEvent| {
                dragging.set(None);
                offset.set(0.0);
            })
            .child(
                text("≡")
                    .text_size(px(24.0))
                    .text_color(move || theme().muted_foreground),
            );
        rows = rows.child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .h(px(ORDER_ROW))
                .pl(px(12.0))
                .pr(px(4.0))
                .bg(move || theme().card)
                .style_when(index + 1 != count, |st| {
                    st.border_b_1().border_b_color(move || theme().border)
                })
                .translate_y(move || px(if lifted() { offset.get() } else { 0.0 }))
                .z_index(move || if lifted() { 10 } else { 0 })
                .opacity(move || if lifted() { 0.92_f32 } else { 1.0_f32 })
                .child(dynamic({
                    let hash = look.content_hash.clone();
                    move || {
                        let pixels = app.look_picture(&hash).map(|p| {
                            let t = p.thumb();
                            (t.width, t.height, t.rgba.clone())
                        });
                        picture(pixels, 54.0, 36.0, 6.0, off)
                    }
                }))
                .child(
                    text(look.name.clone())
                        .flex_1()
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || {
                            if off {
                                theme().muted_foreground
                            } else {
                                theme().foreground
                            }
                        }),
                )
                .child(handle),
        );
    }
    rows
}

// --- Import -------------------------------------------------------------------

pub fn import_page(app: AppState, on_dismiss: impl Fn() + 'static) -> impl IntoElement {
    nav_page()
        .title(move || {
            let replacing = app.import_draft().is_some_and(|d| d.replacing.is_some());
            tr(
                if replacing {
                    "looks.replace_title"
                } else {
                    "looks.new_title"
                },
                &[],
            )
        })
        .on_dismiss(move || {
            app.discard_import();
            on_dismiss();
        })
        .child(dynamic(move || import_content(app)))
}

fn import_content(app: AppState) -> AnyElement {
    let Some(draft) = app.import_draft() else {
        return view().flex_1().bg(move || token("grouped")).into_element();
    };
    let base = view().flex_1().bg(move || token("grouped"));
    if draft.working {
        return base
            .px_6()
            .pt_6()
            .gap_4()
            .child(full_picture(None, false))
            .child(
                text(tr("looks.converting", &[("file", &draft.file_name)]))
                    .text_size(px(15.0))
                    .text_color(move || theme().muted_foreground),
            )
            .into_element();
    }
    if let Some(error) = draft.error.clone() {
        let (title, body) = error.keys();
        return base
            .px_6()
            .pt_6()
            .child(grouped_card(
                view()
                    .p_4()
                    .gap_2()
                    .child(
                        text(tr(title, &[]))
                            .text_size(px(17.0))
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        text(tr(body, &[]))
                            .text_size(px(15.0))
                            .line_height(px(22.0))
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(secondary_button(
                        tr("looks.choose_again", &[]),
                        move || match app.import_draft().and_then(|d| d.replacing) {
                            Some((id, _)) => app.begin_replace(id),
                            None => app.begin_import(),
                        },
                    )),
            ))
            .into_element();
    }
    let name = signal(draft.name.clone());
    let sending = draft.sending;
    base.child(
        crate::ui::page_scroll().flex_1().child(
            view()
                .px_6()
                .pt_4()
                .pb_8()
                .gap_4()
                .child(full_picture(app.import_picture(), false))
                .child(
                    view()
                        .gap_2()
                        .child(name_field(
                            name,
                            move |value| {
                                name.set(value.clone());
                                app.set_import_name(value);
                            },
                            move || app.send_import(),
                        ))
                        .child(footnote(tr("looks.name_note", &[]))),
                )
                .child(grouped_card(control_row(
                    tr("looks.cover", &[]),
                    dynamic(move || icon_view(app.import_cover())),
                    true,
                ))),
        ),
    )
    .child(
        view()
            .px_6()
            .pb_4()
            .gap_2()
            .child(primary_button(
                tr(
                    if sending {
                        "looks.sending"
                    } else {
                        "looks.send"
                    },
                    &[],
                ),
                move || {
                    if !sending {
                        app.send_import()
                    }
                },
            ))
            .child(
                text(tr("looks.send_note", &[]))
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_center()
                    .text_color(move || theme().muted_foreground),
            ),
    )
    .into_element()
}
