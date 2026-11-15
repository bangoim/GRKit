//! Two tabs (Camera, Settings) with navigation stacks kept mounted.

use nsh_base::{nav_stack, theme};
use nsh_core::{
    mobile::{TabBarItem, tab_bar},
    platform::Os,
    prelude::*,
};

use crate::pages::{camera_root, settings_root, software_page};
use crate::store::{AppState, Route, Tab};
use crate::strings::tr;

const TABS: &[(Tab, &str, &str)] = &[
    (Tab::Camera, "tab.camera", "camera"),
    (Tab::Settings, "tab.settings", "gearshape"),
];

pub fn shell(app: AppState) -> impl IntoElement {
    let tab = computed({
        let app = app.clone();
        move || app.tab()
    });
    let selected = move || {
        TABS.iter()
            .position(|(t, _, _)| *t == tab.get())
            .unwrap_or(0) as u32
    };
    let shown = signal(false);
    spawn(async move {
        next_frame().await;
        next_frame().await;
        shown.set(true);
    });
    let mut stacks = view().flex_1();
    for &(t, _, _) in TABS {
        let hidden = move || !shown.get() || tab.get() != t;
        stacks = stacks.child(
            view()
                .flex_1()
                .style_when(hidden, |st| st.hidden())
                .child(tab_stack(app.clone(), t)),
        );
    }
    view()
        .flex_1()
        .bg(move || theme().background)
        .child(stacks)
        .child(tabs(selected, {
            let app = app.clone();
            move |i| app.set_tab(TABS[i as usize].0)
        }))
}

fn tab_stack(app: AppState, tab: Tab) -> AnyElement {
    let mut stack = nav_stack().flex_1().child(root(app.clone(), tab));
    for depth in 0..4 {
        let at = computed({
            let app = app.clone();
            move || app.route_at(tab, depth)
        });
        let app = app.clone();
        stack = stack.child(dynamic(move || {
            at.get()
                .map(|route| untracked(|| pushed(app.clone(), tab, depth, route)))
        }));
    }
    stack.into_element()
}

fn root(app: AppState, tab: Tab) -> AnyElement {
    match tab {
        Tab::Camera => camera_root(app).into_element(),
        Tab::Settings => settings_root().into_element(),
    }
}

fn pushed(app: AppState, tab: Tab, depth: usize, route: Route) -> AnyElement {
    let back = {
        let app = app.clone();
        move || app.pop_to(tab, depth)
    };
    match route {
        Route::SoftwareUpdate => software_page(app, back).into_element(),
    }
}

fn tabs(
    selected: impl Fn() -> u32 + Copy + 'static,
    choose: impl Fn(u32) + 'static + Clone,
) -> AnyElement {
    if matches!(os(), Os::Ios | Os::Android) {
        let choose = choose.clone();
        return tab_bar()
            .items(
                TABS.iter()
                    .map(|(_, key, symbol)| TabBarItem::new(tr(key, &[])).icon(*symbol))
                    .collect::<Vec<_>>(),
            )
            .selected_index(selected)
            .on_change(move |i: &u32| choose(*i))
            .into_element();
    }
    let mut row = view()
        .flex_row()
        .border_t_1()
        .border_t_color(move || theme().border)
        .bg(move || theme().card)
        .pb_safe_area();
    for (i, (_, key, _)) in TABS.iter().enumerate() {
        let i = i as u32;
        let title = tr(key, &[]);
        let choose = choose.clone();
        row = row.child(
            view()
                .flex_1()
                .min_h(px(49.0))
                .items_center()
                .justify_center()
                .role(AccessibilityRole::Button)
                .label(title.clone())
                .on_press(move || choose(i))
                .child(
                    text(title)
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || {
                            if selected() == i {
                                theme().primary
                            } else {
                                theme().muted_foreground
                            }
                        }),
                ),
        );
    }
    row.into_element()
}
