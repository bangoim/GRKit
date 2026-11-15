//! Inset grouped lists and shared controls.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::domain::CameraModel;
use crate::strings::tr;

const ART_GR3: ImageAsset = asset!("art-gr3.png");
const ART_GR3X: ImageAsset = asset!("art-gr3x.png");
const ART_GR4: ImageAsset = asset!("art-gr4.png");
const GR_SNAP: ImageAsset = asset!("gr-snap.png");
pub const STEP1: ImageAsset = asset!("step1.jpg");
pub const STEP2: ImageAsset = asset!("step2.jpg");
pub const STEP3: ImageAsset = asset!("step3.jpg");

pub fn gr_snap_icon() -> ImageAsset {
    GR_SNAP
}

pub fn token(name: &'static str) -> AdaptiveColor {
    theme().color(name).unwrap_or_else(|| theme().background)
}

/// Floating sheet card fill: light `rgba(250,250,252,0.96)`, matching dark.
pub fn sheet_surface() -> Rgba {
    if dark() {
        rgba(0x2C2C2EF5)
    } else {
        rgba(0xFAFAFCF5)
    }
}

/// Line art for a model, tinted with the text color.
pub fn camera_art(model: CameraModel, width: f32, faded: bool) -> impl IntoElement {
    let asset = match model {
        CameraModel::Gr3 => ART_GR3,
        CameraModel::Gr3x => ART_GR3X,
        CameraModel::Gr4 => ART_GR4,
    };
    let opacity = if faded {
        move || if dark() { 0.22_f32 } else { 0.32_f32 }
    } else {
        move || if dark() { 0.85_f32 } else { 1.0_f32 }
    };
    image(asset)
        .w(px(width))
        .object_fit(ResizeMode::Contain)
        .tint_color(move || theme().foreground)
        .opacity(opacity)
}

pub fn primary_button(title: impl Into<String>, on_press: impl Fn() + 'static) -> impl IntoElement {
    let title = title.into();
    view()
        .h(px(50.0))
        .px_8()
        .rounded(px(25.0))
        .items_center()
        .justify_center()
        .bg(move || theme().primary)
        .role(AccessibilityRole::Button)
        .label(title.clone())
        .on_press(on_press)
        .child(
            text(title)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().primary_foreground),
        )
}

/// Full-width primary CTA inside a floating sheet (52×radius 26).
pub fn sheet_primary_button(
    title: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let title = title.into();
    view()
        .w_full()
        .h(px(52.0))
        .rounded(px(26.0))
        .items_center()
        .justify_center()
        .bg(move || theme().primary)
        .role(AccessibilityRole::Button)
        .label(title.clone())
        .on_press(on_press)
        .child(
            text(title)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().primary_foreground),
        )
}

pub fn secondary_button(
    title: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let title = title.into();
    view()
        .w_full()
        .h(px(44.0))
        .items_center()
        .justify_center()
        .role(AccessibilityRole::Button)
        .label(title.clone())
        .on_press(on_press)
        .child(
            text(title)
                .text_size(px(15.0))
                .text_color(move || theme().muted_foreground),
        )
}

pub fn close_chip(on_press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .size(px(44.0))
        .rounded_full()
        .items_center()
        .justify_center()
        .bg(move || theme().muted)
        .role(AccessibilityRole::Button)
        .label(tr("scan.close", &[]))
        .on_press(on_press)
        .child(
            text("✕")
                .text_size(px(17.0))
                .text_color(move || theme().muted_foreground),
        )
}

pub fn section_title(title: impl Into<String>) -> impl IntoElement {
    text(title.into())
        .text_size(px(18.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(move || theme().muted_foreground)
}

pub fn grouped_card(child: impl IntoElement) -> impl IntoElement {
    view()
        .rounded(px(20.0))
        .overflow_hidden()
        .bg(move || theme().card)
        .child(child)
}

pub fn info_row(
    title: impl Into<String>,
    value: impl Into<String>,
    last: bool,
) -> impl IntoElement {
    let title = title.into();
    let value = value.into();
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .min_h(px(50.0))
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

pub fn chevron_row(
    title: impl Into<String>,
    subtitle: Option<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let title = title.into();
    let label = match &subtitle {
        Some(s) => format!("{title}, {s}"),
        None => title.clone(),
    };
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .min_h(px(52.0))
        .px_4()
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(on_press)
        .active(|st| st.bg(move || theme().accent))
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(
                    text(title.clone())
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(subtitle.map(|s| {
                    text(s)
                        .text_size(px(15.0))
                        .text_color(move || theme().muted_foreground)
                })),
        )
        .child(
            text("›")
                .text_size(px(22.0))
                .text_color(move || theme().muted_foreground),
        )
}

pub fn status_dot_row(title: impl Into<String>, status: impl Into<String>) -> impl IntoElement {
    view()
        .items_center()
        .gap(px(6.0))
        .child(
            text(title.into())
                .text_size(px(28.0))
                .font_weight(FontWeight::BOLD),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(
                    view()
                        .size(px(7.0))
                        .rounded_full()
                        .bg(move || token("success")),
                )
                .child(
                    text(status.into())
                        .text_size(px(15.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

pub fn checklist_row(title: &str, subtitle: Option<&str>, state: StepState) -> impl IntoElement {
    let title = title.to_owned();
    let subtitle = subtitle.map(str::to_owned);
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .min_h(px(50.0))
        .px_4()
        .child(step_icon(state))
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(
                    text(title)
                        .text_size(px(17.0))
                        .style_when(matches!(state, StepState::Current | StepState::Done), |s| {
                            s.font_weight(FontWeight::SEMIBOLD)
                        })
                        .text_color(move || match state {
                            StepState::Waiting => theme().muted_foreground,
                            _ => theme().foreground,
                        }),
                )
                .child(subtitle.map(|s| {
                    text(s)
                        .text_size(px(13.0))
                        .text_color(move || theme().muted_foreground)
                })),
        )
}

#[derive(Clone, Copy)]
pub enum StepState {
    Done,
    Current,
    Waiting,
}

fn step_icon(state: StepState) -> impl IntoElement {
    match state {
        StepState::Done => view()
            .size(px(22.0))
            .rounded_full()
            .items_center()
            .justify_center()
            .bg(move || token("success"))
            .child(
                text("✓")
                    .text_size(px(12.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xffffff)),
            )
            .into_element(),
        // Filled ring: outer stroke + solid center (spinner alternative).
        StepState::Current => view()
            .size(px(22.0))
            .rounded_full()
            .items_center()
            .justify_center()
            .border_2()
            .border_color(move || theme().foreground)
            .child(
                view()
                    .size(px(8.0))
                    .rounded_full()
                    .bg(move || theme().foreground),
            )
            .into_element(),
        StepState::Waiting => view()
            .size(px(22.0))
            .rounded_full()
            .border_2()
            .border_color(move || theme().muted_foreground)
            .opacity(0.35)
            .into_element(),
    }
}

pub fn sheet_shell(
    title: impl Into<String>,
    subtitle: Option<String>,
    show_close: bool,
    on_close: impl Fn() + 'static,
    body: impl IntoElement,
) -> impl IntoElement {
    let title = title.into();
    view()
        .flex_col()
        .w_full()
        .gap_5()
        .pt(px(14.0))
        .px(px(20.0))
        .pb(px(22.0))
        .child(
            view()
                .flex_row()
                .items_start()
                .child(view().w(px(44.0)))
                .child(
                    view()
                        .flex_1()
                        .items_center()
                        .gap(px(4.0))
                        .pt_2()
                        .child(
                            text(title)
                                .text_size(px(22.0))
                                .font_weight(FontWeight::BOLD)
                                .text_center(),
                        )
                        .child(subtitle.map(|s| {
                            text(s)
                                .text_size(px(15.0))
                                .text_color(move || theme().muted_foreground)
                                .text_center()
                        })),
                )
                .child(if show_close {
                    close_chip(on_close).into_element()
                } else {
                    view().w(px(44.0)).into_element()
                }),
        )
        .child(body)
}

pub fn gr_mark(size: f32) -> impl IntoElement {
    view()
        .size(px(size))
        .rounded(px(size * 0.25))
        .items_center()
        .justify_center()
        .bg(move || theme().primary)
        .child(
            text("GR")
                .text_size(px(size * 0.33))
                .font_weight(FontWeight::BOLD)
                .text_color(move || theme().primary_foreground),
        )
}
