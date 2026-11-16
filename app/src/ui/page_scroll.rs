//! Scroll only when page content exceeds the space below system chrome.
use nsh_core::prelude::*;

#[derive(Debug, Clone, PartialEq, Props)]
pub struct PageScrollProps {
    pub children: Prop<Children>,
}

#[component]
pub fn PageScroll(props: PageScrollProps, cx: Scope<NoEvent>) -> impl IntoElement {
    let viewport = signal(0.0f32);
    let content = signal(0.0f32);
    let overflowing = move || {
        let insets = cx.insets();
        let available = (viewport.get() - insets.top.0 - insets.bottom.0).max(0.0);
        viewport.get() > 0.0 && content.get() > available + 1.0
    };
    scroll()
        .flex_1()
        .bounces(false)
        .scrollable(overflowing)
        .scrollbar(overflowing)
        .on_layout(move |bounds: &Bounds<Points>| viewport.set(bounds.size.height.0))
        .child(
            view()
                .w_full()
                .flex_shrink_0()
                .overflow_hidden()
                .on_layout(move |bounds: &Bounds<Points>| content.set(bounds.size.height.0))
                .child(props.children),
        )
}
