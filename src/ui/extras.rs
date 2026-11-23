use super::*;
use sdir::utility::Utility;
#[derive(Clone)]
pub(super) struct ContextMenu {
    pub pane: usize,
    pub target_tab: usize,
    pub paths: Vec<NativePath>,
    pub x: f64,
    pub y: f64,
    pub background: bool,
    pub keyboard: bool,
    pub tab: Option<usize>,
}
#[cfg(target_os = "macos")]
fn app_window_handle() -> u64 {
    0
}

#[cfg(windows)]
fn app_window_handle() -> u64 {
    use dioxus::desktop::tao::platform::windows::WindowExtWindows;
    dioxus::desktop::window().window.hwnd() as usize as u64
}
pub(super) fn selected_properties(
    bridge: &Bridge,
    folder: NativePath,
    paths: Vec<NativePath>,
) -> Result<(), String> {
    match paths.as_slice() {
        [] => bridge.properties(folder),
        [path] => bridge.properties(path.clone()),
        _ => bridge.utility(Utility::PropertiesSelection {
            folder,
            entries: paths,
        }),
    }
}

pub(super) fn open_context(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    pane: usize,
    path: Option<NativePath>,
    x: f64,
    y: f64,
    keyboard: bool,
) {
    let mut s = model.write();
    if s.modal_open() || s.job.is_some() || s.pending_context.is_some() {
        return;
    }
    s.active = pane;
    s.interaction += 1;
    s.pending_focus = None;
    let background = path.is_none();
    if let Some(p) = path {
        if !s.panes[pane].tab().selection.contains(&p) {
            s.panes[pane].select(p, false, false);
        }
    }
    let paths = if background {
        vec![s.panes[pane].tab().path.clone()]
    } else {
        s.selected()
    };
    let target_tab = s.panes[pane].active;
    let folder = s.panes[pane].tab().path.clone();
    let request = s.interaction;
    let context = ContextMenu {
        pane,
        target_tab,
        paths,
        x,
        y,
        background,
        keyboard,
        tab: None,
    };
    #[cfg(target_os = "macos")]
    {
        let _ = (bridge, folder, request);
        s.context = Some(context);
    }
    #[cfg(windows)]
    {
        let action = Utility::FolderMenu {
            request,
            folder,
            entries: if background {
                Vec::new()
            } else {
                context.paths.clone()
            },
            owner: app_window_handle(),
            point: if keyboard {
                None
            } else {
                sdir::utility::screen_cursor_position().ok()
            },
        };
        s.pending_context = Some((request, context));
        s.dirty();
        drop(s);
        if let Err(error) = bridge.utility(action.clone()) {
            finish_context(model, &action, Err(error));
        }
    }
}

pub(super) fn finish_context(
    mut model: Signal<AppModel>,
    action: &Utility,
    result: Result<Option<u32>, String>,
) {
    let Utility::FolderMenu {
        request,
        folder,
        owner,
        ..
    } = action
    else {
        return;
    };
    let mut state = model.write();
    if !state
        .pending_context
        .as_ref()
        .is_some_and(|(id, _)| id == request)
    {
        return;
    }
    let Some((_, context)) = state.pending_context.take() else {
        return;
    };
    if state.interaction != *request
        || state.active != context.pane
        || state.modal_open()
        || state.closing
    {
        return;
    }
    let Some(pane) = state.panes.get(context.pane) else {
        return;
    };
    if pane.active != context.target_tab
        || &pane.tab().path != folder
        || (!context.background && state.selected() != context.paths)
    {
        return;
    }
    #[cfg(windows)]
    {
        let active = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        if active.0 as usize as u64 != *owner {
            return;
        }
    }
    #[cfg(target_os = "macos")]
    let _ = owner;
    match result {
        Ok(Some(sdir::utility::SDIR_MENU_SELECTED)) => state.context = Some(context),
        Err(error) => {
            state.notice = error;
            state.context = Some(context);
        }
        _ => {}
    }
}

pub(super) fn open_selected_context(model: Signal<AppModel>, bridge: &Bridge, pane: usize) {
    let path = model
        .read()
        .panes
        .get(pane)
        .and_then(|p| p.tab().selection.iter().next().cloned());
    open_context(model, bridge, pane, path, 220., 180., true);
}
pub(super) fn open_tab_context(
    mut model: Signal<AppModel>,
    pane: usize,
    tab: usize,
    x: f64,
    y: f64,
) {
    let mut s = model.write();
    if s.modal_open() {
        return;
    }
    s.active = pane;
    s.interaction += 1;
    s.context = Some(ContextMenu {
        pane,
        target_tab: tab,
        tab: Some(tab),
        paths: vec![s.panes[pane].tabs[tab].path.clone()],
        x,
        y,
        background: false,
        keyboard: false,
    });
}
pub(super) fn close_context(mut model: Signal<AppModel>) {
    let pane = model.write().context.take().map(|c| c.pane);
    if let Some(p) = pane {
        focus_list_after_render(model, p);
    }
}
fn context_command(mut model: Signal<AppModel>, bridge: &Bridge, command: &str) {
    let Some(c) = model.write().context.take() else {
        return;
    };
    model.write().active = c.pane;
    if c.tab.is_none() && model.read().panes[c.pane].active != c.target_tab {
        change_tab(
            model,
            bridge,
            c.pane,
            TabCommand::Select(c.target_tab),
            false,
        );
    }
    if let Some(tab) = c.tab {
        match command {
            "tabopen" => change_tab(model, bridge, c.pane, TabCommand::Select(tab), true),
            "tabpin" => {
                let mut s = model.write();
                s.panes[c.pane].toggle_pin(tab);
                s.dirty();
            }
            "tableft" | "tabright" => {
                let mut s = model.write();
                s.panes[c.pane].move_tab(tab, command == "tabright");
                s.dirty();
            }
            "tabclose" => change_tab(model, bridge, c.pane, TabCommand::Close(tab), true),
            "paths" => {
                if let Err(e) = bridge.utility(Utility::CopyPaths(c.paths)) {
                    model.write().notice = e;
                }
            }
            _ => {}
        }
        let _ = document::eval(&format!(
            "document.querySelector('[data-pane=\"{}\"] .tab.selected')?.focus({{preventScroll:true}})",
            c.pane
        ));
        return;
    }
    let path = c
        .paths
        .first()
        .cloned()
        .unwrap_or_else(|| model.read().panes[c.pane].tabs[c.target_tab].path.clone());
    let result = match command {
        "back" => {
            history_navigation_tab(model, bridge, c.pane, c.target_tab, false);
            Ok(())
        }
        "forward" => {
            history_navigation_tab(model, bridge, c.pane, c.target_tab, true);
            Ok(())
        }
        command if command.starts_with("view_") => {
            set_view(model, c.pane, &command[5..]);
            Ok(())
        }
        "sort_name" | "sort_size" | "sort_modified" => {
            let column = match command {
                "sort_name" => SortColumn::Name,
                "sort_size" => SortColumn::Size,
                _ => SortColumn::Modified,
            };
            let changed = model.read().panes[c.pane].tabs[c.target_tab].sort != column;
            if changed {
                sort_tab(model, c.pane, c.target_tab, column);
            }
            Ok(())
        }
        "sort_ascending" => {
            sort_direction_tab(model, c.pane, c.target_tab, false);
            Ok(())
        }
        "sort_descending" => {
            sort_direction_tab(model, c.pane, c.target_tab, true);
            Ok(())
        }
        "open" => {
            let entry = model.read().panes[c.pane]
                .tab()
                .entries
                .iter()
                .find(|e| e.path == path)
                .cloned();
            if c.background || entry.is_some_and(|e| e.is_dir) {
                navigate(model, bridge, c.pane, path);
                Ok(())
            } else {
                bridge.job(Request::Open { path })
            }
        }
        "with" => bridge.utility(Utility::OpenWith(path, app_window_handle())),
        "paths" => bridge.utility(Utility::CopyPaths(c.paths)),
        "terminal" => bridge.utility(Utility::Terminal(
            model.read().panes[c.pane].tabs[c.target_tab].path.clone(),
        )),
        "explorer" => bridge.utility(Utility::Explorer(path)),
        "properties" if cfg!(target_os = "macos") => {
            model.write().inspector = Some(Inspector::Details);
            Ok(())
        }
        "properties" => {
            let folder = model.read().panes[c.pane].tabs[c.target_tab].path.clone();
            selected_properties(bridge, folder, c.paths)
        }
        "copy" => {
            copy(model, bridge, false);
            Ok(())
        }
        "cut" => {
            copy(model, bridge, true);
            Ok(())
        }
        "paste" => {
            paste(model, bridge);
            Ok(())
        }
        "rename" => {
            start_inline_rename(model);
            Ok(())
        }
        "batch" => {
            open_batch(model);
            Ok(())
        }
        "delete" => {
            dialog(model, "recycle");
            Ok(())
        }
        "new" => {
            dialog(model, "new");
            Ok(())
        }
        "refresh" => {
            let state = model.read().panes[c.pane].tab().snapshot();
            model.write().panes[c.pane].tab_mut().resume_from(&state);
            load(model, bridge, c.pane);
            focus_list_after_render(model, c.pane);
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(e) = result {
        model.write().notice = e;
    }
    if !model.read().modal_open() {
        focus_list_after_render(model, c.pane);
    }
}
fn position_context_menu() {
    let _ = document::eval(
        r#"{
            const menu = document.querySelector('.file-context-menu');
            if (menu) {
                menu.style.setProperty('--context-height', `${menu.offsetHeight}px`);
                menu.querySelector('[data-menu-level="main"]:not(:disabled)')?.focus();
            }
        }"#,
    );
}
fn position_context_submenu() {
    let _ = document::eval(
        r#"{
            const menu = document.querySelector('.file-context-menu');
            const panel = menu?.querySelector('.context-submenu');
            const trigger = panel?.parentElement.querySelector(':scope > button');
            if (trigger) {
                const r = trigger.getBoundingClientRect();
                const {width, height} = panel.getBoundingClientRect();
                const left = r.right + width <= innerWidth - 8 ? r.right : r.left - width;
                panel.style.left = Math.max(8, left) + 'px';
                panel.style.top = Math.max(8, Math.min(r.top, innerHeight - height - 8)) + 'px';
                panel.querySelector('button')?.focus();
            }
        }"#,
    );
}
fn close_context_submenu(mut submenu: Signal<Option<&'static str>>) {
    let opened = *submenu.read();
    if let Some(command) = opened {
        submenu.set(None);
        let _ = document::eval(&format!(
            r#"document.querySelector('.file-context-menu [data-command="{command}"]')?.focus()"#,
        ));
    }
}
#[component]
pub(super) fn ContextMenuView(mut model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let mut submenu = use_signal(|| None::<&'static str>);
    let Some(c) = model.read().context.clone() else {
        return rsx! {};
    };
    let count = c.paths.len();
    let menu_state = {
        let s = model.read();
        s.panes.get(c.pane).and_then(|pane| {
            pane.tabs.get(c.target_tab).map(|tab| {
                (
                    pane_view_option(pane.view, pane.icon_size),
                    pane_view_option(ViewMode::Icons, pane.icon_size) == "icons",
                    tab.sort,
                    tab.descending,
                    !tab.back.is_empty(),
                    !tab.forward.is_empty(),
                )
            })
        })
    };
    let Some((view_option, custom_icon_size, sort_column, descending, can_back, can_forward)) =
        menu_state
    else {
        return rsx! {};
    };
    let pinned = c
        .tab
        .is_some_and(|i| model.read().panes[c.pane].tabs[i].pinned);
    let tab_count = model.read().panes[c.pane].tabs.len();
    let entries: Vec<_> = if c.tab.is_some() {
        vec![
            ("탭 열기", "tabopen"),
            ("고정/고정 해제", "tabpin"),
            ("왼쪽으로 이동", "tableft"),
            ("오른쪽으로 이동", "tabright"),
            ("탭 닫기", "tabclose"),
            ("탭 경로 복사", "paths"),
        ]
    } else if c.background {
        vec![
            ("뒤로", "back"),
            ("앞으로", "forward"),
            ("보기", "view"),
            ("정렬 기준", "sort"),
            ("새로고침", "refresh"),
            ("새 폴더", "new"),
            ("붙여넣기", "paste"),
            ("폴더 경로 복사", "paths"),
            ("여기서 터미널 열기", "terminal"),
            ("탐색기에서 표시", "explorer"),
            ("폴더 속성", "properties"),
        ]
    } else {
        vec![
            ("열기", "open"),
            ("연결 프로그램 직접 선택…", "with"),
            ("복사", "copy"),
            ("잘라내기", "cut"),
            ("이름 변경", "rename"),
            ("이름 일괄 변경", "batch"),
            ("휴지통으로 이동", "delete"),
            ("전체 경로 복사", "paths"),
            ("여기서 터미널 열기", "terminal"),
            ("탐색기에서 표시", "explorer"),
            ("속성", "properties"),
        ]
    };
    let entries = entries.into_iter().filter_map(|(label, command)| {
        if cfg!(target_os = "macos") {
            match command {
                "with" | "windows_menu" => return None,
                "explorer" => return Some(("Finder에서 표시", command)),
                "properties" => return Some(("정보 보기", command)),
                _ => {}
            }
        }
        Some((label, command))
    });
    let menu_label = if c.background {
        "폴더 배경 메뉴"
    } else if c.tab.is_some() {
        "탭 메뉴"
    } else {
        "파일 메뉴"
    };
    let opened = *submenu.read();
    rsx! {
        div {
            class: "context-dismiss",
            onclick: move |_| close_context(model),
            onkeydown: move |event: KeyboardEvent| context_keys(event, model, submenu),
            div {
                class: "file-context-menu",
                onmounted: move |_| position_context_menu(),
                role: "menu",
                "aria-label": menu_label,
                style: "--context-x:{c.x}px;--context-y:{c.y}px",
                onclick: move |event: MouseEvent| event.stop_propagation(),
                for (label, cmd) in entries {
                    if matches!(cmd, "view" | "sort") {
                        {
                            let expanded = opened == Some(cmd);
                            rsx! {
                                div {
                                    key: "{cmd}",
                                    class: "context-menu-entry",
                                    role: "none",
                                    button {
                                        role: "menuitem",
                                        tabindex: -1,
                                        "data-command": cmd,
                                        "data-menu-level": "main",
                                        "aria-haspopup": "menu",
                                        "aria-expanded": if expanded { "true" } else { "false" },
                                        onclick: move |_| {
                                            if expanded {
                                                close_context_submenu(submenu);
                                            } else {
                                                submenu.set(Some(cmd));
                                            }
                                        },
                                        "{label}"
                                        span { "aria-hidden": "true", "›" }
                                    }
                                    if expanded {
                                        {
                                            let options = if cmd == "view" {
                                                let mut options: Vec<_> = PANE_VIEW_OPTIONS.into_iter()
                                                    .map(|(value, action, label)| (label, action, view_option == value))
                                                    .collect();
                                                if custom_icon_size {
                                                    options.push(("사용자 지정 아이콘", "view_icons", view_option == "icons"));
                                                }
                                                options
                                            } else {
                                                vec![
                                                    ("이름", "sort_name", sort_column == SortColumn::Name),
                                                    ("크기", "sort_size", sort_column == SortColumn::Size),
                                                    ("수정한 날짜", "sort_modified", sort_column == SortColumn::Modified),
                                                    ("오름차순", "sort_ascending", !descending),
                                                    ("내림차순", "sort_descending", descending),
                                                ]
                                            };
                                            rsx! {
                                                div {
                                                    class: "context-submenu",
                                                    role: "menu",
                                                    "aria-label": label,
                                                    onmounted: move |_| position_context_submenu(),
                                                    for (name, action, selected) in options {
                                                        {
                                                            let b = bridge.clone();
                                                            rsx! {
                                                                button {
                                                                    key: "{action}",
                                                                    role: "menuitemradio",
                                                                    tabindex: -1,
                                                                    "data-command": action,
                                                                    "data-menu-level": "sub",
                                                                    "aria-checked": if selected { "true" } else { "false" },
                                                                    onclick: move |_| context_command(model, &b, action),
                                                                    "{name}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        {
                            let b = bridge.clone();
                            let disabled = (count != 1 && ["open", "with", "rename"].contains(&cmd))
                                || (cmd == "tabclose" && (pinned || tab_count == 1))
                                || (cmd == "tableft" && c.tab == Some(0))
                                || (cmd == "tabright" && c.tab == Some(tab_count - 1))
                                || (cmd == "back" && !can_back)
                                || (cmd == "forward" && !can_forward);
                            rsx! {
                                button {
                                    key: "{cmd}",
                                    role: "menuitem",
                                    tabindex: -1,
                                    "data-command": cmd,
                                    "data-menu-level": "main",
                                    disabled,
                                    onclick: move |event: MouseEvent| {
                                        if cmd == "windows_menu" {
                                            let point = event.client_coordinates();
                                            if let Some(context) = model.write().context.as_mut() {
                                                context.keyboard = point.x == 0. && point.y == 0.;
                                            }
                                        }
                                        context_command(model, &b, cmd);
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn tree_follow_tick(mut model: Signal<AppModel>, bridge: &Bridge) {
    {
        let s = model.read();
        let current = sdir::tree::tree_key(s.path());
        let seen = s
            .tree_seen
            .as_ref()
            .map(|path| sdir::tree::tree_key(path.clone()));
        let follow = s.tree_follow && seen.as_ref() != Some(&current);
        if !s.tree.open || (!follow && (s.tree.pending.is_some() || s.tree_tasks.is_empty())) {
            return;
        }
    }
    let next = {
        let mut s = model.write();
        if !s.tree.open {
            return;
        }
        let current = sdir::tree::tree_key(s.path());
        let seen = s
            .tree_seen
            .as_ref()
            .map(|path| sdir::tree::tree_key(path.clone()));
        if s.tree_follow && seen.as_ref() != Some(&current) {
            if s.tree_pending_auto && s.tree.pending.is_some() {
                bridge.tree.cancel();
                s.tree.pending = None;
            }
            s.tree_seen = Some(current.clone());
            s.tree_cursor = Some(current.clone());
            s.tree_tasks.push_front((current, false));
            while s.tree_tasks.len() > 65 {
                s.tree_tasks.pop_back();
            }
        }
        if s.tree.pending.is_some() {
            return;
        }
        let Some((path, expand)) = s.tree_tasks.front().cloned() else {
            return;
        };
        let path = sdir::tree::tree_key(path);
        if let Some((queued, _)) = s.tree_tasks.front_mut() {
            *queued = path.clone();
        }
        let (next, done) = s.tree.follow_step(&path, expand);
        if done {
            s.tree_tasks.pop_front();
            s.dirty();
            let rows = s.tree.rows();
            if let Some(index) = rows
                .iter()
                .position(|r| r.path == path && r.message.is_none())
            {
                let _ = document::eval(&format!(
                    "requestAnimationFrame(()=>{{const t=document.querySelector('.drive-tree');if(t){{t.scrollTop=Math.max(0,{index}*28-t.clientHeight/2);}}}})"
                ));
            }
        }
        next.map(|path| {
            let id = s.tree.begin(&path).unwrap();
            s.tree_pending_auto = true;
            s.dirty();
            Request::Tree {
                id,
                path,
                hidden: s.show_hidden,
            }
        })
    };
    if let Some(request) = next {
        bridge.tree.replace(request);
    }
}

#[component]
pub(super) fn AppearanceDialog(mut model: Signal<AppModel>, pane: usize) -> Element {
    let mut columns = use_signal(|| model.read().panes[pane].columns.clone());
    let mut size = use_signal(|| model.read().panes[pane].icon_size);
    let values = columns.read().clone();
    let current = *size.read();
    rsx! {div{class:"modal-backdrop",onkeydown:move|e:KeyboardEvent|{e.stop_propagation();if e.key()==Key::Escape{close_appearance(model,pane);}},div{class:"modal appearance-dialog",role:"dialog","aria-modal":"true","aria-label":"패널 보기 설정",
    h2{"패널 {pane+1} 보기 설정"}p{"자세히 보기의 열과 아이콘 보기의 크기를 설정합니다. 너비는 이름 120~800, 나머지 50~800px입니다."}
    div{class:"column-settings",for (index,label) in ["이름","크기","종류","수정 날짜"].iter().enumerate(){label{span{"{label}"}input{r#type:"checkbox","aria-label":"{label} 표시",disabled:index==0,checked:values.visible[index],onchange:move|e:FormEvent|{let mut c=columns.write();c.custom=true;c.visible[index]=e.checked();}}input{r#type:"number",min:if index==0{"120"}else{"50"},max:"800","aria-label":"{label} 너비",value:"{values.widths[index]}",oninput:move|e:FormEvent|{if let Ok(w)=e.value().parse::<u16>(){let mut c=columns.write();c.custom=true;c.widths[index]=w;}}}}}}
    label{class:"icon-size-setting",span{"아이콘 타일 크기 · {current}px"}input{r#type:"range",min:"80",max:"192",step:"16",value:"{current}",oninput:move|e:FormEvent|{if let Ok(v)=e.value().parse::<u16>(){size.set(v.clamp(80,192));}}}}
    div{class:"modal-actions",button{onclick:move |_|{columns.set(Default::default());size.set(112);},"기본값"}button{onclick:move |_|close_appearance(model,pane),"취소"}button{class:"primary appearance-apply",onclick:move |_|{let mut c=columns.read().clone();c.normalize();let view={let mut s=model.write();let p=&mut s.panes[pane];p.columns=c;p.icon_size=(*size.read()).clamp(80,192);let v=p.view;s.appearance=None;s.dirty();v};set_view(model,pane,view.value());},"적용"}}
    }}}
}

#[derive(Clone)]
pub(super) struct BatchDialog {
    paths: Vec<NativePath>,
    pattern: sdir::batch::Pattern,
    pub(super) id: u64,
    due: Option<Instant>,
    pub(super) result: Option<Result<Vec<sdir::batch::RenamePair>, String>>,
    pane: usize,
}
pub(super) fn open_batch(model: Signal<AppModel>) {
    open_batch_mode(model, false);
}
pub(super) fn open_numbered_batch(model: Signal<AppModel>) {
    open_batch_mode(model, true);
}
fn open_batch_mode(mut model: Signal<AppModel>, numbered: bool) {
    let mut s = model.write();
    if s.job.is_some() {
        return;
    }
    let paths = if numbered {
        let tab = s.panes[s.active].tab();
        tab.visible
            .iter()
            .map(|&index| &tab.entries[index].path)
            .filter(|path| tab.selection.contains(*path))
            .cloned()
            .collect()
    } else {
        s.selected()
    };
    if paths.is_empty() || paths.len() > 500 {
        s.notice = "1~500개 항목을 선택하세요.".into();
        return;
    }
    let id = s.id();
    s.batch = Some(BatchDialog {
        paths,
        pattern: sdir::batch::Pattern {
            whole_stem: numbered.then(String::new),
            ..Default::default()
        },
        id,
        due: Some(Instant::now()),
        result: None,
        pane: s.active,
    });
}
pub(super) fn close_batch(mut model: Signal<AppModel>, bridge: &Bridge) {
    bridge.preview.cancel();
    let pane = model.write().batch.take().map(|b| b.pane);
    if let Some(p) = pane {
        focus_list_after_render(model, p);
    }
}
fn change_pattern(mut model: Signal<AppModel>, field: usize, value: String) {
    let mut s = model.write();
    let id = s.id();
    if let Some(b) = s.batch.as_mut() {
        match field {
            0 => b.pattern.find = value,
            1 => b.pattern.replace = value,
            2 => b.pattern.prefix = value,
            3 => b.pattern.suffix = value,
            4 => b.pattern.start = value.parse().ok(),
            _ => b.pattern.whole_stem = Some(value),
        }
        b.id = id;
        b.result = None;
        b.due = Some(Instant::now() + Duration::from_millis(250));
    }
}
pub(super) fn batch_tick(mut model: Signal<AppModel>, bridge: &Bridge) {
    if !model
        .read()
        .batch
        .as_ref()
        .is_some_and(|b| b.due.is_some_and(|t| t <= Instant::now()))
    {
        return;
    }
    let request = {
        let mut s = model.write();
        let Some(b) = s.batch.as_mut() else { return };
        if !b.due.is_some_and(|t| t <= Instant::now()) {
            return;
        }
        b.due = None;
        Request::Preview {
            id: b.id,
            paths: b.paths.clone(),
            pattern: b.pattern.clone(),
        }
    };
    bridge.preview.replace(request);
}
#[component]
pub(super) fn BatchRenameDialog(mut model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let b = model.read().batch.clone().unwrap();
    let pattern = b.pattern.clone();
    let common = pattern.whole_stem;
    let values = [
        pattern.find,
        pattern.replace,
        pattern.prefix,
        pattern.suffix,
        pattern.start.map(|n| n.to_string()).unwrap_or_default(),
    ];
    let ready = b.result.as_ref().is_some_and(|r| {
        r.as_ref()
            .is_ok_and(|pairs| pairs.iter().any(|p| p.source.name() != p.name))
    });
    let cancel = bridge.clone();
    let apply = bridge.clone();
    rsx! {div{class:"modal-backdrop",onkeydown:move|e:KeyboardEvent|{e.stop_propagation();if e.key()==Key::Escape{close_batch(model,&bridge);}},div{class:"modal batch-dialog",role:"dialog","aria-modal":"true","aria-label":"이름 일괄 변경",
    h2{"이름 일괄 변경 · {b.paths.len()}개"}p{"확장자는 유지됩니다. 바뀔 이름을 미리 확인한 뒤 적용하세요. 충돌이 있으면 실행하지 않습니다."}
    if let Some(common) = common {
        label{class:"batch-fields",span{"공통 이름"},
            input{class:"batch-field batch-common-name",maxlength:200,initial_value:"{common}",
                onmounted:move |_|{let _=document::eval("document.querySelector('.batch-common-name')?.focus();");},
                onchange:move |e:FormEvent|change_pattern(model,5,e.value()),
                onkeydown:move |e:KeyboardEvent| {
                    if e.key()==Key::Enter && !e.is_composing() {
                        e.prevent_default();
                        let _=document::eval("document.querySelector('.batch-apply:not(:disabled)')?.click();");
                    }
                }
            }
        }
    } else {
    div{class:"batch-fields",for (i,label) in ["찾을 글자","바꿀 글자","앞에 추가","뒤에 추가","시작 번호 (선택)"].iter().enumerate(){label{span{"{label}"}input{class:"batch-field","data-field":i,value:"{values[i]}",maxlength:200,oninput:move|e:FormEvent|change_pattern(model,i,e.value())}}}}
    }
    div{class:"batch-preview",match b.result{Some(Ok(pairs))=>rsx!{for pair in pairs{div{class:"rename-preview",span{"{pair.source.name()}"}span{"→"}strong{"{pair.name}"}}}},Some(Err(e))=>rsx!{p{class:"batch-error","{e}"}},None=>rsx!{p{"미리보기 확인 중…"}}}}
    div{class:"modal-actions",button{class:"batch-cancel",onclick:move |_|close_batch(model,&cancel),"취소"}button{class:"primary batch-apply",disabled:!ready,onclick:move |_|{let pairs=model.read().batch.as_ref().and_then(|b|b.result.as_ref()).and_then(|r|r.as_ref().ok()).cloned();if let Some(pairs)=pairs{let pairs=pairs.into_iter().filter(|p|p.source.name()!=p.name).collect();model.write().batch=None;submit(model,&apply,FileAction::BatchRename{pairs});}},"적용"}}
    }}}
}
#[component]
pub(super) fn JournalDialog(mut model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let s = model.read();
    let entries = s.journal.clone();
    let busy = s.job.is_some();
    drop(s);
    rsx! {
        div { class:"modal-backdrop", onkeydown:move|e:KeyboardEvent|{e.stop_propagation();if e.key()==Key::Escape{close_journal(model);}},
            div { class:"modal journal-dialog", role:"dialog", "aria-modal":"true", "aria-label":"작업 기록",
                h2 { "작업 기록" }
                p { "이번 실행의 최근 32건, 항목별 표시는 최대 500개입니다. 큰 작업은 전체 경로를 보존하지 않아 재시도가 제한됩니다. 재시도는 미완료로 확인된 선택 항목만 처리합니다. 폴더는 폴더 단위로 다시 확인하며 Windows가 중복 여부를 묻습니다." }
                div { class:"journal-list",
                    if entries.is_empty() { p { "아직 파일 작업 기록이 없습니다." } }
                    for (i,entry) in entries.into_iter().enumerate().rev() {
                        {
                            let retry=entry.retry(); let b=bridge.clone();
                            let destination=entry.destination.as_ref().map(|p|p.display()).unwrap_or_default();
                            rsx! {
                                section { class:"journal-entry", "data-job-index":i,
                                    h3 { "{entry.time} · {entry.label}" }
                                    p { class:"job-message", "{entry.message}" }
                                    if !destination.is_empty() { p { class:"job-destination", title:"{destination}", "대상: {destination}" } }
                                    if entry.cancelled { p { "사용자가 취소한 작업 · 일부 처리 결과를 확인하세요." } }
                                    details {
                                        summary { "항목별 결과 · {entry.outcomes.len()}개" }
                                        for item in &entry.outcomes { p { class:if item.ok{"job-success"}else{"job-failed"}, title:"{item.source.display()}", "{item.source.display()} · {item.message}" } }
                                        for dest in &entry.destinations { p { class:"job-result-path", "결과: {dest.display()}" } }
                                    }
                                    button { class:"job-retry", disabled:busy||retry.is_none(), onclick:move |_|{if let Some(action)=retry.clone(){close_journal(model);submit(model,&b,action);if model.read().job.is_some(){let mut s=model.write();if let Some(entry)=s.journal.get_mut(i){entry.retried=true;}if let Some(c)=s.job_context.as_mut(){c.reveal=None;}}}}, "미완료 항목 재시도" }
                                }
                            }
                        }
                    }
                }
                div { class:"modal-actions", button { onclick:move |_|close_journal(model), "닫기" } }
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct DropPoint {
    pane: usize,
    generation: u64,
    folder: String,
    row: Option<usize>,
    #[serde(rename = "rowPath")]
    row_path: Option<String>,
}

struct DragFiles {
    folder: NativePath,
    paths: Vec<NativePath>,
}

pub(super) struct ArmedDrag {
    id: u64,
    source: DropPoint,
    files: Option<DragFiles>,
    #[cfg(windows)]
    capture: Option<sdir::drag::MouseGesture>,
}

#[derive(serde::Deserialize)]
struct DropTab {
    pane: usize,
    index: usize,
    folder: String,
}

impl DropTab {
    fn resolve<'a>(&self, state: &'a AppModel) -> Option<&'a NativePath> {
        let pane = (self.pane < state.layout).then(|| &state.panes[self.pane])?;
        let path = &pane.tabs.get(self.index)?.path;
        (path.display() == self.folder).then_some(path)
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum DropTarget {
    List(DropPoint),
    Tab(DropTab),
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DragMessage {
    Arm {
        id: u64,
        source: DropPoint,
    },
    Begin {
        id: u64,
    },
    Cancel {
        id: u64,
    },
    HoverTab {
        id: u64,
        tab: DropTab,
    },
    Release {
        id: u64,
        target: DropTarget,
        ctrl: bool,
        shift: bool,
        alt: bool,
    },
}

impl DropPoint {
    fn resolve<'a>(
        &self,
        state: &'a AppModel,
    ) -> Option<(&'a sdir::model::Tab, Option<&'a sdir::model::FileEntry>)> {
        let pane = (self.pane < state.layout).then(|| &state.panes[self.pane])?;
        let tab = pane.tab();
        if pane.generation != self.generation || tab.path.display() != self.folder || !tab.loaded {
            return None;
        }
        let entry = if let Some(row) = self.row {
            let entry = tab
                .visible
                .get(row)
                .and_then(|&index| tab.entries.get(index))?;
            if self.row_path.as_deref() != Some(entry.path.display().as_str()) {
                return None;
            }
            Some(entry)
        } else {
            None
        };
        Some((tab, entry))
    }
}

pub(super) fn handle_drag(mut model: Signal<AppModel>, bridge: &Arc<Bridge>, message: &str) {
    #[cfg(windows)]
    use sdir::drag::{CursorWindow, cursor_window};
    let Ok(message) = serde_json::from_str::<DragMessage>(message) else {
        return;
    };
    match message {
        #[cfg(windows)]
        DragMessage::Arm { id, source } => {
            let owner = app_window_handle();
            let events = {
                let mut state = model.write();
                if state.closing
                    || state.modal_open()
                    || state.job.is_some()
                    || !matches!(source.resolve(&state), Some((_, Some(_))))
                {
                    return;
                }
                state.file_drag = None;
                let capture = if cursor_window(owner) == CursorWindow::Own {
                    sdir::drag::MouseGesture::capture().ok().flatten()
                } else {
                    None
                };
                let events = capture.as_ref().map(sdir::drag::MouseGesture::events);
                state.file_drag = Some(ArmedDrag {
                    id,
                    source,
                    files: None,
                    capture,
                });
                events
            };
            let Some(events) = events else {
                return;
            };
            let bridge = bridge.clone();
            spawn(async move {
                while events.ready() {
                    let other = cursor_window(owner) == CursorWindow::Other;
                    let handoff = {
                        let state = model.read();
                        let Some(drag) = state.file_drag.as_ref().filter(|drag| drag.id == id)
                        else {
                            return;
                        };
                        if state.closing || state.modal_open() || state.job.is_some() {
                            break;
                        }
                        other && drag.files.is_some()
                    };
                    if handoff {
                        let Some(drag) = model.write().file_drag.take() else {
                            return;
                        };
                        let (Some(files), Some(capture)) = (drag.files, drag.capture) else {
                            return;
                        };
                        let _ = document::eval(&format!(
                            "window.dispatchEvent(new CustomEvent('sdir-native-drag',{{detail:{id}}}));"
                        ));
                        start_drag(model, &bridge, drag.source, files, id, capture);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                if let Some(drag) = model
                    .write()
                    .file_drag
                    .as_mut()
                    .filter(|drag| drag.id == id)
                {
                    drag.capture = None;
                }
            });
        }
        #[cfg(target_os = "macos")]
        DragMessage::Arm { id, source } => {
            let mut state = model.write();
            if !state.closing
                && !state.modal_open()
                && state.job.is_none()
                && matches!(source.resolve(&state), Some((_, Some(_))))
            {
                state.file_drag = Some(ArmedDrag {
                    id,
                    source,
                    files: None,
                });
            }
        }
        DragMessage::Begin { id } => {
            let mut state = model.write();
            let Some(drag) = state.file_drag.as_ref().filter(|drag| drag.id == id) else {
                return;
            };
            if drag.files.is_some() || state.closing || state.modal_open() || state.job.is_some() {
                return;
            }
            let Some((tab, Some(entry))) = drag.source.resolve(&state) else {
                state.file_drag = None;
                return;
            };
            // Keep the paths alive when hovering a tab replaces the source list.
            let paths = if tab.selection.contains(&entry.path) {
                tab.selection.iter().cloned().collect()
            } else {
                vec![entry.path.clone()]
            };
            let files = DragFiles {
                folder: tab.path.clone(),
                paths,
            };
            if let Some(drag) = &mut state.file_drag {
                drag.files = Some(files);
            }
        }
        DragMessage::HoverTab { id, tab } => {
            let activate = {
                let state = model.read();
                !state.closing
                    && !state.modal_open()
                    && state.job.is_none()
                    && state
                        .file_drag
                        .as_ref()
                        .is_some_and(|drag| drag.id == id && drag.files.is_some())
                    && tab.resolve(&state).is_some()
                    && state.panes[tab.pane].active != tab.index
            };
            if activate {
                change_tab(
                    model,
                    bridge,
                    tab.pane,
                    TabCommand::Select(tab.index),
                    false,
                );
            }
        }
        DragMessage::Cancel { id } => {
            let mut state = model.write();
            if state.file_drag.as_ref().is_some_and(|drag| drag.id == id) {
                state.file_drag = None;
            }
        }
        DragMessage::Release {
            id,
            target,
            ctrl,
            shift,
            alt,
        } => {
            let drag = {
                let mut state = model.write();
                if !state.file_drag.as_ref().is_some_and(|drag| drag.id == id) {
                    return;
                }
                state.file_drag.take()
            };
            let Some(ArmedDrag {
                source,
                files: Some(files),
                ..
            }) = drag
            else {
                return;
            };
            #[cfg(windows)]
            if cursor_window(app_window_handle()) != CursorWindow::Own {
                return;
            }
            drop_in_app(
                model,
                bridge,
                source.pane,
                files,
                target,
                (ctrl, shift, alt),
            );
        }
    }
}

fn drop_in_app(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    source_pane: usize,
    files: DragFiles,
    target: DropTarget,
    modifiers: (bool, bool, bool),
) {
    let (ctrl, shift, alt) = modifiers;
    let (pane, destination, tab_to_select) = {
        let state = model.read();
        if state.closing || state.modal_open() || state.job.is_some() {
            return;
        }
        match target {
            DropTarget::List(point) => {
                let Some((tab, row)) = point.resolve(&state) else {
                    return;
                };
                let destination = match row {
                    Some(folder) if folder.is_dir => folder.path.clone(),
                    Some(_) => return,
                    None => tab.path.clone(),
                };
                (point.pane, destination, None)
            }
            DropTarget::Tab(tab) => {
                let Some(path) = tab.resolve(&state) else {
                    return;
                };
                let pane = tab.pane;
                let select = (state.panes[pane].active != tab.index).then_some(tab.index);
                (pane, path.clone(), select)
            }
        }
    };
    if cfg!(windows) && (alt || (ctrl && shift)) {
        if let Err(error) = bridge.utility(Utility::CreateShortcuts {
            folder: destination,
            paths: files.paths,
        }) {
            model.write().notice = error;
        }
        return;
    }
    let copying = if cfg!(windows) { ctrl } else { alt };
    let moving = !copying && (shift || sdir::drag::same_volume(&files.folder, &destination));
    if moving && files.folder == destination {
        return;
    }
    activate_pane(model, source_pane);
    if let Some(index) = tab_to_select {
        change_tab(model, bridge, pane, TabCommand::Select(index), false);
    }
    let sources = files.paths;
    let action = if moving {
        FileAction::Move {
            sources,
            destination,
        }
    } else {
        FileAction::Copy {
            sources,
            destination,
        }
    };
    submit_drop(model, bridge, pane, action);
}

fn submit_drop(model: Signal<AppModel>, bridge: &Bridge, pane: usize, action: FileAction) {
    let requested = {
        let state = model.read();
        let target = PaneLocation::capture(&state, pane);
        (action.destination().as_ref() == Some(&target.path)).then(|| PasteContext {
            target,
            origin: PaneLocation::capture(&state, state.active),
            interaction: state.interaction,
        })
    };
    submit_context(model, bridge, action, requested);
}

pub(super) fn drop_external_files(
    model: Signal<AppModel>,
    bridge: &Bridge,
    pane: usize,
    destination: NativePath,
    event: DragEvent,
) {
    event.prevent_default();
    event.stop_propagation();
    let sources: Vec<_> = event
        .files()
        .iter()
        .map(|file| NativePath::new(file.path()))
        .collect();
    if !sources.is_empty() && !model.read().modal_open() {
        submit_drop(
            model,
            bridge,
            pane,
            FileAction::Copy {
                sources,
                destination,
            },
        );
    }
}

#[cfg(windows)]
fn start_drag(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    source: DropPoint,
    files: DragFiles,
    id: u64,
    capture: sdir::drag::MouseGesture,
) {
    let events = capture.events();
    if !events.ready() {
        return;
    }
    let mut state = model.write();
    if state.modal_open() || state.job.is_some() {
        return;
    }
    if let Some((_, Some(entry))) = source.resolve(&state) {
        let path = entry.path.clone();
        state.active = source.pane;
        state.interaction += 1;
        let pane = &mut state.panes[source.pane];
        if !pane.tab().selection.contains(&path) {
            pane.select(path, false, false);
        }
        focus_list(source.pane);
    }
    match bridge.drag(id, files.paths, app_window_handle(), events) {
        Err(error) => state.notice = error,
        Ok(()) => {
            state.external_drag = Some((id, capture));
            state.notice = "기본·Ctrl·오른쪽 드래그는 복사입니다. 이동은 SDIR 창 밖으로 커서를 옮기기 전에 Shift만 누르고 드롭 순간까지 유지해야 요청됩니다. Explorer 등 Shift 이동을 지원하는 OLE 파일 대상으로 드롭하세요. Esc로 취소할 수 있습니다.".into();
        }
    }
}

pub(super) fn tree_key(
    e: KeyboardEvent,
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    path: NativePath,
    expanded: bool,
) {
    let path = sdir::tree::tree_key(path);
    e.stop_propagation();
    if model.read().modal_open() {
        return;
    }
    if pane_shortcut(&e, model, active_pane(model), bridge) {
        return;
    }
    let key = e.key();
    if key == Key::Enter {
        e.prevent_default();
        tree_navigate(model, bridge, path);
        return;
    }
    if !matches!(
        key,
        Key::ArrowDown | Key::ArrowUp | Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End
    ) {
        return;
    }
    e.prevent_default();
    model.write().tree_cursor = Some(path.clone());
    let _ = document::eval("document.querySelector('.drive-tree')?.focus({preventScroll:true})");
    if key == Key::ArrowRight && !expanded {
        tree_toggle(model, bridge, path, false);
        return;
    }
    if key == Key::ArrowLeft && expanded {
        tree_toggle(model, bridge, path, false);
        return;
    }
    let target = {
        let s = model.read();
        let rows: Vec<_> = s
            .tree
            .rows()
            .into_iter()
            .filter(|r| r.message.is_none())
            .map(|r| r.path)
            .collect();
        let i = rows.iter().position(|p| p == &path).unwrap_or(0);
        match key {
            Key::ArrowUp => rows.get(i.saturating_sub(1)).cloned(),
            Key::ArrowDown => rows.get((i + 1).min(rows.len().saturating_sub(1))).cloned(),
            Key::Home => rows.first().cloned(),
            Key::End => rows.last().cloned(),
            Key::ArrowLeft => path.parent().map(sdir::tree::tree_key),
            Key::ArrowRight => s
                .tree
                .nodes
                .get(&path)
                .and_then(|n| n.children.first())
                .cloned(),
            _ => None,
        }
    };
    if let Some(target) = target {
        let row = model
            .read()
            .tree
            .rows()
            .iter()
            .enumerate()
            .find(|(_, r)| r.path == target && r.message.is_none())
            .map(|(i, _)| i);
        if let Some(index) = row {
            model.write().tree_cursor = Some(target);
            let _ = document::eval(&format!(
                "{{const tree=document.querySelector('.drive-tree');if(tree){{tree.scrollTop=Math.max(0,{index}*28-tree.clientHeight/2);tree.focus({{preventScroll:true}});}}}}"
            ));
        }
    }
}

pub(super) struct BackgroundRefresh {
    pub(super) id: u64,
    generation: u64,
    pub(super) entries: Vec<sdir::model::FileEntry>,
    renames: Vec<sdir::watch::Rename>,
}
pub(super) fn start_background(mut model: Signal<AppModel>, bridge: &Bridge, pane: usize) -> u64 {
    let (id, request) = {
        let mut s = model.write();
        s.watch_due[pane] = None;
        let generation = s.panes[pane].generation;
        let mut renames = s.refreshes[pane]
            .take()
            .filter(|refresh| refresh.generation == generation)
            .map_or_else(Vec::new, |refresh| refresh.renames);
        renames.append(&mut s.watch_renames[pane]);
        let hidden = s.show_hidden;
        let id = s.id().max(generation + 1);
        s.next_id = id + 1;
        let p = &mut s.panes[pane];
        p.loading = true;
        let path = p.tab().path.clone();
        s.refreshes[pane] = Some(BackgroundRefresh {
            id,
            generation,
            entries: vec![],
            renames,
        });
        (
            id,
            Request::List {
                pane,
                id,
                path,
                hidden,
            },
        )
    };
    bridge.panes[pane].replace(request);
    id
}
pub(super) fn finish_background(
    mut model: Signal<AppModel>,
    pane: usize,
    id: u64,
    error: Option<String>,
) -> bool {
    let mut s = model.write();
    if !s.refreshes[pane].as_ref().is_some_and(|r| r.id == id) {
        return false;
    }
    let r = s.refreshes[pane].take().unwrap();
    if s.panes[pane].generation != r.generation {
        return true;
    }
    let p = &mut s.panes[pane];
    if let Some(e) = error {
        p.finish_preserving_order(
            r.generation,
            Some(format!("갱신 실패 · 이전 목록 유지: {e}")),
        );
        return true;
    }
    let t = p.tab_mut();
    t.replace_entries_in_current_order(r.entries, &r.renames);
    for rename in r.renames {
        if t.selection.remove(&rename.from) {
            t.selection.insert(rename.to.clone());
        }
        if t.cursor.as_ref() == Some(&rename.from) {
            t.cursor = Some(rename.to.clone());
        }
        if t.anchor.as_ref() == Some(&rename.from) {
            t.anchor = Some(rename.to);
        }
    }
    p.finish_preserving_order(r.generation, None);
    let t = p.tab_mut();
    let visible: BTreeSet<_> = t
        .visible
        .iter()
        .map(|i| t.entries[*i].path.clone())
        .collect();
    t.selection.retain(|p| visible.contains(p));
    t.cursor = t.cursor.take().filter(|p| visible.contains(p));
    t.anchor = t.anchor.take().filter(|p| visible.contains(p));

    s.dirty();
    true
}

pub(super) fn dismiss_modal(mut model: Signal<AppModel>, bridge: &Bridge) {
    let network_dialog = model.read().network_dialog;
    if let Some(disconnect) = network_dialog {
        close_network_dialog(model, disconnect);
        return;
    }
    if model.read().context.is_some() {
        close_context(model);
        return;
    }
    if model.read().batch.is_some() {
        close_batch(model, bridge);
        return;
    }
    if model.read().copy_destination.is_some() {
        close_copy_destination(model);
        return;
    }
    let pane = {
        let mut s = model.write();
        s.appearance = None;
        s.journal_open = false;
        s.dialog = None;
        s.preferences_open = false;
        s.workspaces_open = false;
        s.active
    };
    focus_list_after_render(model, pane);
}
fn context_keys(
    event: KeyboardEvent,
    model: Signal<AppModel>,
    submenu: Signal<Option<&'static str>>,
) {
    event.stop_propagation();
    let index = match event.key() {
        Key::Escape | Key::ArrowLeft => {
            event.prevent_default();
            let opened = submenu.read().is_some();
            if opened {
                close_context_submenu(submenu);
            } else if event.key() == Key::Escape {
                close_context(model);
            }
            return;
        }
        Key::ArrowRight => {
            event.prevent_default();
            let _ = document::eval(
                r#"{
                    const button = document.activeElement;
                    if (button?.matches('.file-context-menu [aria-haspopup="menu"]')) {
                        if (button.getAttribute('aria-expanded') === 'true') {
                            button.nextElementSibling?.querySelector('button')?.focus();
                        } else {
                            button.click();
                        }
                    }
                }"#,
            );
            return;
        }
        Key::ArrowDown => "(current + 1) % items.length",
        Key::ArrowUp => "(current + items.length - 1) % items.length",
        Key::Tab if event.modifiers().shift() => "(current + items.length - 1) % items.length",
        Key::Tab => "(current + 1) % items.length",
        Key::Home => "0",
        Key::End => "items.length - 1",
        _ => return,
    };
    event.prevent_default();
    let _ = document::eval(&format!(
        r#"{{
            const active = document.activeElement;
            const level = active?.dataset.menuLevel === 'sub' ? 'sub' : 'main';
            const menu = document.querySelector('.file-context-menu');
            const items = [...(menu?.querySelectorAll(`[data-menu-level="${{level}}"]:not(:disabled)`) ?? [])];
            const current = items.indexOf(active);
            if (items.length) items[{index}]?.focus();
        }}"#,
    ));
}

pub(super) fn tree_navigate(mut model: Signal<AppModel>, bridge: &Bridge, path: NativePath) {
    let path = sdir::tree::tree_key(path);
    model.write().tree_cursor = Some(path.clone());
    navigate(model, bridge, active_pane(model), path);
    model.write().pending_focus = None;
    let _ = document::eval("document.querySelector('.drive-tree')?.focus({preventScroll:true})");
}

fn close_journal(mut model: Signal<AppModel>) {
    model.write().journal_open = false;
    let pane = model.read().active;
    focus_list_after_render(model, pane);
}
fn close_appearance(mut model: Signal<AppModel>, pane: usize) {
    model.write().appearance = None;
    focus_list_after_render(model, pane);
}
