use super::*;
use sdir::pane_arrangement::{PANE_LAYOUTS, PaneArrangement, PaneLayout};

const AREAS: [&str; 4] = ["a", "b", "c", "d"];

pub(super) fn grid_style(layout: &PaneLayout) -> String {
    let rows = layout
        .rows
        .iter()
        .map(|row| {
            let cells = row
                .as_bytes()
                .iter()
                .map(|&cell| AREAS[usize::from(cell - b'a')])
                .collect::<Vec<_>>();
            format!("\"{}\"", cells.join(" "))
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "grid-template-areas:{rows};grid-template-columns:repeat({},minmax(0,1fr));grid-template-rows:repeat({},minmax(0,1fr))",
        layout.rows[0].len(),
        layout.rows.len()
    )
}

fn apply_layout(mut model: Signal<AppModel>, bridge: &Bridge, layout: &PaneLayout) {
    let count = layout.count;
    let previous = {
        let mut state = model.write();
        let previous = state.layout;
        state.layout = count;
        state.pane_arrangement = layout.arrangement;
        state.active = state.active.min(count - 1);
        state.search_target = state.search_target.filter(|&pane| pane < count);
        state.dirty();
        previous
    };
    for pane in previous..count {
        let mut state = model.write();
        let tab = state.panes[pane].tab_mut();
        tab.resume_from(&tab.snapshot());
        drop(state);
        load(model, bridge, pane);
    }
}

#[component]
pub(super) fn LayoutSwitcher(model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let mut open = use_signal(|| false);
    let state = model.read();
    let current = state.pane_arrangement.layout(state.layout);
    drop(state);
    let menu_open = *open.read();
    rsx! {
        div { class:"layout-switch",
            for count in 1..=4 { {
                let bridge = bridge.clone();
                rsx! { button { class:if count == current.count {"chosen"} else {""}, title:"{count}분할",
                    "aria-pressed":count == current.count, onclick:move |_| {
                        open.set(false);
                        apply_layout(model, &bridge, PaneArrangement::Standard.layout(count));
                    },"{count}" } }
            } }
            button {class:"layout-options-toggle",title:"분할 배치 선택", "aria-label":"분할 배치 선택",
                "aria-expanded":menu_open, "aria-controls":"layout-options", onclick:move |_|open.set(!menu_open),
                span {class:"layout-glyph",style:grid_style(current),"aria-hidden":"true",
                    for area in AREAS.iter().take(current.count) {i{style:"grid-area:{area}"}}
                }
            }
            if menu_open {
                div {class:"layout-options-dismiss", "aria-hidden":"true", onclick:move |_|open.set(false)}
                div {id:"layout-options",class:"layout-options",role:"group","aria-label":"분할 배치",
                    onkeydown:move|event:KeyboardEvent| {
                        if event.key()==Key::Escape {
                            event.stop_propagation();open.set(false);
                            let _=document::eval("document.querySelector('.layout-options-toggle')?.focus()");
                        }
                    },
                    for count in 1..=4 {
                       div {class:"layout-options-group",
                            span {class:"layout-options-label","{count}분할"}
                            div {class:"layout-options-items",
                                for option in PANE_LAYOUTS.iter().filter(|option| option.count == count) { {
                                    let bridge = bridge.clone();
                                    let chosen = current.count == option.count && current.arrangement == option.arrangement;
                                    rsx! {button {class:if chosen {"layout-option chosen"} else {"layout-option"},
                                        "aria-pressed":chosen,"aria-label":"{count}분할 · {option.name}",
                                        onclick:move |_| {
                                            apply_layout(model, &bridge, option);
                                            open.set(false);
                                            let _=document::eval("document.querySelector('.layout-options-toggle')?.focus()");
                                        },
                                        span {class:"layout-glyph",style:grid_style(option),"aria-hidden":"true",
                                            for area in AREAS.iter().take(option.count) {i{style:"grid-area:{area}"}}
                                        }
                                        span {"{option.name}"}
                                    }}
                                } }
                            }
                       }
                    }
                }
            }
        }
    }
}
