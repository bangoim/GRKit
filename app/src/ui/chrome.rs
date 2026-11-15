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

/// Floating sheet surface, with an opaque fill in light mode.
pub fn sheet_surface() -> Rgba {
    if dark() {
        rgba(0x242424FF)
    } else {
        rgba(0xFFFFFFFF)
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
        .min_h(px(50.0))
        .py_3()
        .px_8()
        .rounded(px(8.0))
        .items_center()
        .justify_center()
        .bg(move || token("button"))
        .role(AccessibilityRole::Button)
        .label(title.clone())
        .on_press(on_press)
        .active(|s| s.opacity(0.72))
        .child(
            text(title)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || token("button-foreground")),
        )
}

/// Full-width primary action inside a sheet.
pub fn sheet_primary_button(
    title: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let title = title.into();
    view()
        .w_full()
        .min_h(px(50.0))
        .py_3()
        .rounded(px(8.0))
        .items_center()
        .justify_center()
        .bg(move || token("button"))
        .role(AccessibilityRole::Button)
        .label(title.clone())
        .on_press(on_press)
        .active(|s| s.opacity(0.72))
        .child(
            text(title)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || token("button-foreground")),
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
        .active(|s| s.opacity(0.72))
        .child(
            text(title)
                .text_size(px(15.0))
                .line_height(px(22.0))
                .text_color(move || token("link")),
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
        .active(|s| s.opacity(0.72))
        .child(
            text("✕")
                .text_size(px(17.0))
                .text_color(move || theme().muted_foreground),
        )
}

pub fn section_title(title: impl Into<String>) -> impl IntoElement {
    text(title.into())
        .text_size(px(15.0))
        .line_height(px(22.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(move || theme().muted_foreground)
}

pub fn grouped_card(child: impl IntoElement) -> impl IntoElement {
    view()
        .rounded(px(10.0))
        .border_1()
        .border_color(move || theme().border)
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
        .min_h(px(68.0))
        .py_3()
        .px_4()
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(on_press)
        .active(|s| s.opacity(0.72).bg(move || theme().accent))
        .child(
            view()
                .flex_1()
                .gap(px(4.0))
                .child(
                    text(title.clone())
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(subtitle.map(|s| {
                    text(s)
                        .text_size(px(15.0))
                        .line_height(px(22.0))
                        .text_color(move || theme().muted_foreground)
                })),
        )
        .child(
            text("›")
                .text_size(px(22.0))
                .text_color(move || theme().muted_foreground),
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
                    .text_color(move || token("button-foreground")),
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
    hero: Option<CameraModel>,
    show_close: bool,
    on_close: impl Fn() + 'static,
    body: impl IntoElement,
) -> impl IntoElement {
    let title = title.into();
    view()
        .relative()
        .flex_col()
        .w_full()
        .gap_5()
        .pt(px(8.0))
        .px(px(24.0))
        .pb_safe_area_plus(px(8.0))
        .child(
            view()
                .self_center()
                .w(px(36.0))
                .h(px(5.0))
                .rounded_full()
                .bg(rgb(0xD1D1D6)),
        )
        .child(hero.map(|model| {
            view()
                .items_center()
                .pt_4()
                .child(camera_art(model, 140.0, false))
        }))
        .child(
            view()
                .gap(px(6.0))
                .pt_2()
                .child(
                    text(title)
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
            view()
                .absolute()
                .right(px(14.0))
                .top(px(22.0))
                .child(close_chip(on_close))
        }))
        .child(body)
}

pub fn gr_mark(size: f32) -> impl IntoElement {
    view()
        .size(px(size))
        .rounded(px(8.0))
        .items_center()
        .justify_center()
        .bg(move || token("button"))
        .child(
            text("GR")
                .text_size(px(size * 0.33))
                .font_weight(FontWeight::BOLD)
                .text_color(move || token("button-foreground")),
        )
}

/// Controls scoped to the connection flow.
pub fn connection_button(title: String, on_press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .w_full()
        .min_h(px(50.0))
        .py_3()
        .rounded(px(8.0))
        .items_center()
        .justify_center()
        .bg(move || token("connection-accent"))
        .role(AccessibilityRole::Button)
        .label(title.clone())
        .on_press(on_press)
        .active(|s| s.opacity(0.72))
        .child(
            text(title)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || token("connection-accent-foreground")),
        )
}

pub fn connection_icon(path: &'static str) -> impl IntoElement {
    svg(nsh_core::element::ImageSrc::svg_markup(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="{path}"/></svg>"#
    ))).size(px(20.0))
}
