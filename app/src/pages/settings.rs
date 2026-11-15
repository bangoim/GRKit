//! Settings tab: About and More from the Maker (GR Snap).

use nsh_base::{nav_page, theme};
use nsh_core::prelude::*;

use crate::store::{APP_BUILD, APP_VERSION, GR_SNAP_URL, PRIVACY_URL, rate_url};
use crate::strings::tr;
use crate::ui::{gr_snap_icon, grouped_card, section_title, token};

pub fn settings_root() -> impl IntoElement {
    nav_page()
        .title(move || tr("settings.title", &[]))
        .header_large_title(true)
        .child(
            scroll().flex_1().bg(move || token("grouped")).child(
                view()
                    .gap_6()
                    .pt_4()
                    .px_4()
                    .pb_12()
                    .child(
                        view()
                            .gap_2()
                            .child(section_title(tr("settings.about", &[])))
                            .child(grouped_card(
                                view()
                                    .child(value_row(
                                        tr("settings.version", &[]),
                                        format!("{APP_VERSION} ({APP_BUILD})"),
                                        false,
                                    ))
                                    .child(link_row(tr("settings.rate", &[]), false, || {
                                        spawn(async move {
                                            let _ = open_url(rate_url()).await;
                                        });
                                    }))
                                    .child(link_row(tr("settings.privacy", &[]), true, || {
                                        spawn(async move {
                                            let _ = open_url(PRIVACY_URL).await;
                                        });
                                    })),
                            )),
                    )
                    .child(
                        view()
                            .gap_2()
                            .child(section_title(tr("settings.more_maker", &[])))
                            .child(grouped_card(gr_snap_row())),
                    ),
            ),
        )
}

fn value_row(title: String, value: String, last: bool) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .min_h(px(52.0))
        .px_4()
        .style_when(!last, |st| {
            st.border_b_1().border_b_color(move || theme().border)
        })
        .child(text(title).text_size(px(17.0)))
        .child(
            text(value)
                .text_size(px(17.0))
                .text_color(move || theme().muted_foreground),
        )
}

fn link_row(title: String, last: bool, on_press: impl Fn() + 'static) -> impl IntoElement {
    let label = title.clone();
    view()
        .flex_row()
        .items_center()
        .min_h(px(52.0))
        .px_4()
        .style_when(!last, |st| {
            st.border_b_1().border_b_color(move || theme().border)
        })
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(on_press)
        .active(|st| st.bg(move || theme().accent))
        .child(text(title).text_size(px(17.0)))
}

fn gr_snap_row() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .p_4()
        .min_h(px(72.0))
        .child(
            image(gr_snap_icon())
                .size(px(56.0))
                .rounded(px(13.0))
                .object_fit(ResizeMode::Cover),
        )
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(
                    text(move || tr("settings.gr_snap_name", &[]))
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    text(move || tr("settings.gr_snap_desc", &[]))
                        .text_size(px(13.0))
                        .text_color(move || theme().muted_foreground)
                        .number_of_lines(2),
                ),
        )
        .child(
            view()
                .h(px(32.0))
                .px_3()
                .rounded(px(16.0))
                .items_center()
                .justify_center()
                .bg(move || theme().muted)
                .role(AccessibilityRole::Button)
                .label(tr("settings.view", &[]))
                .on_press(|| {
                    spawn(async move {
                        let _ = open_url(GR_SNAP_URL).await;
                    });
                })
                .child(
                    text(move || tr("settings.view", &[]))
                        .text_size(px(15.0))
                        .font_weight(FontWeight::MEDIUM),
                ),
        )
}
