//! Looks tab: the camera's Looks in menu order, and the official catalog.

use nsh_base::{nav_page, theme};
use nsh_core::prelude::*;

use crate::domain::{CameraModel, Look, LookError, LookFilter, LookGroup, LookSource, group_of};
use crate::store::{AppState, CatalogItem, LooksView, Route, Sheet, Tab};
use crate::strings::tr;
use crate::ui::{
    badge, camera_art, chip, circle_button, footnote, grouped_card, page_title, picture,
    primary_button, secondary_button, segmented, token,
};

const PLUS: &str = "M12 5v14M5 12h14";

pub fn looks_root(app: AppState) -> impl IntoElement {
    nav_page()
        .header_visible(false)
        .child(dynamic(move || looks_content(app)))
}

fn looks_content(app: AppState) -> impl IntoElement {
    let connected = app.status().is_some();
    let can_import = connected && app.looks_blocked().is_none() && app.looks_snapshot().is_some();
    let plus = circle_button(
        PLUS,
        tr("looks.import_action", &[]),
        can_import,
        move || app.set_sheet(Some(Sheet::ImportLook)),
    )
    .into_element();
    crate::ui::page_scroll()
        .flex_1()
        .bg(move || token("grouped"))
        .child(
            view()
                .px_6()
                .pt(px(72.0))
                .pb(px(crate::pages::look_pages::TAB_BAR_CLEARANCE))
                .gap_4()
                .child(page_title(
                    tr("looks.title", &[]),
                    connected.then_some(plus),
                ))
                .child(view_switch(app))
                .child(match app.looks_view() {
                    LooksView::OnCamera => on_camera(app),
                    LooksView::All => catalog(app),
                }),
        )
}

fn view_switch(app: AppState) -> impl IntoElement {
    let view = app.looks_view();
    let on_camera = match (app.status(), app.looks_snapshot()) {
        (Some(status), Some(snapshot)) => format!(
            "{} · {}",
            tr(
                "looks.on_camera",
                &[("model", status.camera.model.display_name())]
            ),
            snapshot.looks.len()
        ),
        (Some(status), None) => tr(
            "looks.on_camera",
            &[("model", status.camera.model.display_name())],
        ),
        _ => tr("looks.on_camera_short", &[]),
    };
    segmented(vec![
        (
            on_camera,
            view == LooksView::OnCamera,
            Box::new(move || app.set_looks_view(LooksView::OnCamera)),
        ),
        (
            tr("looks.all", &[]),
            view == LooksView::All,
            Box::new(move || app.set_looks_view(LooksView::All)),
        ),
    ])
}

fn on_camera(app: AppState) -> AnyElement {
    if app.status().is_none() {
        return no_camera(app).into_element();
    }
    let Some(snapshot) = app.looks_snapshot() else {
        if app.looks_error() == Some(LookError::NeedsFastStart) {
            // Trying again does not help until the camera starts again.
            return grouped_card(
                view()
                    .p_4()
                    .child(text(tr("looks.needs_fast_start", &[])).text_size(px(17.0))),
            )
            .into_element();
        }
        if app.looks_error().is_some() {
            return grouped_card(
                view()
                    .p_4()
                    .gap_3()
                    .child(text(tr("looks.read_failed", &[])).text_size(px(17.0)))
                    .child(secondary_button(tr("looks.retry", &[]), move || {
                        app.refresh_looks()
                    })),
            )
            .into_element();
        }
        return grouped_card(
            view().p_4().child(
                text(tr("looks.loading", &[]))
                    .text_size(px(15.0))
                    .text_color(move || theme().muted_foreground),
            ),
        )
        .into_element();
    };
    let blocked = app.looks_blocked().is_some();
    let count = snapshot.looks.len();
    let mut rows = view();
    for (i, look) in snapshot.looks.iter().enumerate() {
        rows = rows.child(look_row(app, look.clone(), i + 1 == count));
    }
    view()
        .gap_4()
        .child(blocked.then(pending_banner))
        .child(
            view()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(crate::ui::section_title(tr("looks.menu_order", &[])))
                        .child((!blocked && count > 1).then(|| {
                            view()
                                .min_h(px(32.0))
                                .justify_center()
                                .role(AccessibilityRole::Button)
                                .label(tr("looks.edit_order", &[]))
                                .on_press(move || app.open_look_order())
                                .child(
                                    text(tr("looks.edit_order", &[]))
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(move || token("link")),
                                )
                        })),
                )
                .child(grouped_card(rows)),
        )
        .child(footnote(tr(
            "looks.footer",
            &[
                ("count", &count.to_string()),
                ("loaded", &snapshot.loaded.to_string()),
                ("max", &snapshot.max_loaded.to_string()),
            ],
        )))
        .into_element()
}

fn pending_banner() -> impl IntoElement {
    view()
        .rounded(px(10.0))
        .p_3()
        .gap(px(2.0))
        .bg(move || token("warning-soft"))
        .role(AccessibilityRole::Text)
        .child(
            text(tr("looks.pending_title", &[]))
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || token("warning")),
        )
        .child(
            text(tr("looks.pending_body", &[]))
                .text_size(px(15.0))
                .line_height(px(20.0))
                .text_color(move || token("warning")),
        )
}

/// The kind of a Look on the camera: its film group, or where it came from.
pub fn look_kind(look: &Look) -> String {
    match look.source {
        LookSource::Official => tr(group_of(&look.id, false).label_key(), &[]),
        LookSource::Custom => tr("looks.imported", &[]),
        LookSource::Unknown => tr("looks.unknown", &[]),
    }
}

fn look_meta(look: &Look) -> String {
    let kind = look_kind(look);
    if !look.enabled {
        format!("{} · {kind}", tr("looks.off", &[]))
    } else if !look.loaded {
        format!("{} · {kind}", tr("looks.not_loaded", &[]))
    } else if look.source == LookSource::Official {
        format!("{kind} · {}", tr("looks.official", &[]))
    } else {
        kind
    }
}

fn thumb(app: AppState, hash: &str, faded: bool) -> AnyElement {
    let pixels = app.look_picture(hash).map(|p| {
        let t = p.thumb();
        (t.width, t.height, t.rgba.clone())
    });
    picture(pixels, 60.0, 40.0, 6.0, faded)
}

fn look_row(app: AppState, look: Look, last: bool) -> impl IntoElement {
    let id = look.id.clone();
    let off = !look.enabled;
    let label = format!("{}, {}", look.name, look_meta(&look));
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .min_h(px(66.0))
        .py(px(10.0))
        .pl(px(12.0))
        .pr(px(14.0))
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(move || app.push(Route::LookDetail(id.clone())))
        .active(|s| s.opacity(0.72))
        .style_when(!last, |st| {
            st.border_b_1().border_b_color(move || theme().border)
        })
        .child(dynamic({
            let hash = look.content_hash.clone();
            move || thumb(app, &hash, off)
        }))
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(
                    text(look.name.clone())
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
                .child(
                    text(look_meta(&look))
                        .text_size(px(15.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(look.selected.then(|| badge(tr("looks.in_use", &[]))))
        .child(
            text("›")
                .text_size(px(22.0))
                .text_color(move || theme().muted_foreground),
        )
}

fn no_camera(app: AppState) -> impl IntoElement {
    view()
        .gap_4()
        .child(grouped_card(
            view()
                .p_6()
                .gap_4()
                .items_center()
                .child(
                    view()
                        .opacity(0.35)
                        .child(camera_art(CameraModel::Gr3, 180.0, false)),
                )
                .child(
                    text(tr("looks.no_camera_title", &[]))
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    text(tr("looks.no_camera_body", &[]))
                        .text_size(px(15.0))
                        .line_height(px(22.0))
                        .text_center()
                        .text_color(move || theme().muted_foreground),
                )
                .child(view().w_full().child(primary_button(
                    tr("looks.connect", &[]),
                    move || {
                        app.set_tab(Tab::Camera);
                        app.open_scan();
                    },
                ))),
        ))
        .child(footnote(tr("looks.no_camera_note", &[])))
}

fn catalog(app: AppState) -> AnyElement {
    let filter = app.looks_filter();
    let chips = view()
        .flex_row()
        .gap_2()
        .child(chip(
            tr("looks.filter.all", &[]),
            filter == LookFilter::All,
            move || app.set_looks_filter(LookFilter::All),
        ))
        .child(chip(
            tr("looks.filter.color", &[]),
            filter == LookFilter::Color,
            move || app.set_looks_filter(LookFilter::Color),
        ))
        .child(chip(
            tr("looks.filter.mono", &[]),
            filter == LookFilter::Mono,
            move || app.set_looks_filter(LookFilter::Mono),
        ));
    let Some(items) = app.catalog_items() else {
        let message = app
            .catalog_error()
            .unwrap_or_else(|| tr("looks.catalog_loading", &[]));
        return view()
            .gap_4()
            .child(chips)
            .child(grouped_card(
                view().p_4().child(
                    text(message)
                        .text_size(px(15.0))
                        .line_height(px(22.0))
                        .text_color(move || theme().muted_foreground),
                ),
            ))
            .into_element();
    };
    let mut column = view().gap_4().child(chips);
    for group in LookGroup::ORDER {
        if !filter.accepts(group) {
            continue;
        }
        let members: Vec<CatalogItem> = items
            .iter()
            .filter(|item| item.group == group)
            .cloned()
            .collect();
        if members.is_empty() {
            continue;
        }
        let count = members.len();
        let mut rows = view();
        for (i, item) in members.into_iter().enumerate() {
            rows = rows.child(catalog_row(app, item, i + 1 == count));
        }
        column = column.child(
            view()
                .gap_2()
                .child(crate::ui::section_title(tr(group.label_key(), &[])))
                .child(grouped_card(rows)),
        );
    }
    column.into_element()
}

fn catalog_row(app: AppState, item: CatalogItem, last: bool) -> impl IntoElement {
    let id = item.look.id.clone();
    let on_camera = app
        .looks_snapshot()
        .is_some_and(|snapshot| snapshot.get(&id).is_some());
    let adding = app.catalog_adding(&id);
    let can_add =
        app.status().is_some() && app.looks_blocked().is_none() && app.look_busy().is_none();
    let meta = item
        .look
        .cover
        .as_ref()
        .and_then(|c| c.brand.clone())
        .unwrap_or_else(|| tr(item.group.label_key(), &[]));
    let trailing: Option<AnyElement> = if on_camera {
        Some(
            text(format!("✓ {}", tr("looks.on_camera_mark", &[])))
                .text_size(px(15.0))
                .text_color(move || theme().muted_foreground)
                .into_element(),
        )
    } else if adding {
        Some(
            text(tr("looks.adding", &[]))
                .text_size(px(15.0))
                .text_color(move || theme().muted_foreground)
                .into_element(),
        )
    } else if can_add {
        let name = item.look.name.clone();
        Some(
            view()
                .h(px(32.0))
                .px(px(14.0))
                .rounded_full()
                .items_center()
                .justify_center()
                .bg(move || theme().muted)
                .role(AccessibilityRole::Button)
                .label(tr("looks.add_named", &[("name", &name)]))
                .on_press({
                    let id = id.clone();
                    move || app.add_catalog_look(id.clone())
                })
                .active(|s| s.opacity(0.72))
                .child(
                    text(tr("looks.add", &[]))
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || token("link")),
                )
                .into_element(),
        )
    } else {
        None
    };
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .min_h(px(66.0))
        .py(px(10.0))
        .pl(px(12.0))
        .pr(px(14.0))
        .style_when(!last, |st| {
            st.border_b_1().border_b_color(move || theme().border)
        })
        .child(dynamic({
            let hash = item.look.sha256.clone();
            move || thumb(app, &hash, false)
        }))
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(
                    text(item.look.name.clone())
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    text(meta)
                        .text_size(px(15.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(trailing)
}

/// The sheet behind the Add button: what kind of file to import.
pub fn import_sheet(app: AppState) -> impl IntoElement {
    let option = move |title: String, ext: String| {
        crate::ui::chevron_row(title, Some(ext), move || app.begin_import())
    };
    crate::ui::sheet_shell(
        tr("looks.import_title", &[]),
        Some(tr("looks.import_body", &[])),
        None,
        true,
        move || app.set_sheet(None),
        view()
            .gap_3()
            .child(grouped_card(
                view()
                    .child(
                        view()
                            .border_b_1()
                            .border_b_color(move || theme().border)
                            .child(option(
                                tr("looks.import_lut", &[]),
                                tr("looks.import_lut_ext", &[]),
                            )),
                    )
                    .child(option(
                        tr("looks.import_preset", &[]),
                        tr("looks.import_preset_ext", &[]),
                    )),
            ))
            .child(footnote(tr("looks.import_note", &[]))),
    )
}
