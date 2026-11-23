use super::*;
use sdir::{file_preview::Preview, model::FileEntry};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Inspector {
    Preview,
    Details,
}

#[derive(Clone)]
pub(super) struct PreviewRequest {
    id: u64,
    path: NativePath,
    location: PaneLocation,
    result: Option<Result<Preview, String>>,
}

pub(super) fn preview_tick(mut model: Signal<AppModel>, bridge: &Bridge) {
    let selection = {
        let state = model.read();
        (state.inspector == Some(Inspector::Preview))
            .then(|| {
                let tab = state.panes[state.active].tab();
                if tab.selection.len() != 1 {
                    return None;
                }
                let path = tab.selection.first()?;
                Some((PaneLocation::capture(&state, state.active), path.clone()))
            })
            .flatten()
    };
    let Some((location, path)) = selection else {
        let had_preview = model.read().file_preview.is_some();
        if had_preview {
            model.write().file_preview = None;
            bridge.file_preview.cancel();
        }
        return;
    };
    let unchanged = {
        let state = model.read();
        state
            .file_preview
            .as_ref()
            .is_some_and(|p| p.path == path && p.location.matches(&state))
    };
    if unchanged {
        return;
    }
    let mut state = model.write();
    let id = state.id();
    state.file_preview = Some(PreviewRequest {
        id,
        path: path.clone(),
        location,
        result: None,
    });
    bridge
        .file_preview
        .replace(Request::ReadPreview { id, path });
}

pub(super) fn finish_preview(
    mut model: Signal<AppModel>,
    id: u64,
    result: Result<Preview, String>,
) {
    if let Some(request) = model.write().file_preview.as_mut().filter(|r| r.id == id) {
        request.result = Some(result);
    }
}

#[component]
pub(super) fn FileInspector(mut model: Signal<AppModel>, mode: Inspector) -> Element {
    let state = model.read();
    let tab = state.panes[state.active].tab();
    let count = tab.selection.len();
    let first: Option<&FileEntry> = tab
        .selection
        .first()
        .and_then(|path| tab.entries.iter().find(|entry| &entry.path == path));
    let preview = state
        .file_preview
        .as_ref()
        .filter(|p| p.location.matches(&state) && first.is_some_and(|entry| entry.path == p.path))
        .and_then(|p| p.result.as_ref());
    let title = if mode == Inspector::Preview {
        "미리보기"
    } else {
        "상세 정보"
    };
    rsx! {aside{class:"file-inspector", "aria-label":title,
        div{class:"inspector-header",strong{"{title}"}button{class:"icon-button",
            "aria-label":"정보 창 닫기",onclick:move |_|model.write().inspector=None,"×"}}
        if let Some(first) = first {
            if count > 1 {
                p{"{count}개 항목 선택됨"}
            } else {
                strong{class:"inspector-file",title:"{first.path.display()}","{first.name}"}
                if mode == Inspector::Details {
                    dl{
                        dt{"위치"}dd{"{tab.path.display()}"}
                        dt{"크기"}dd{"{readable_size(first.bytes)}"}
                        dt{"수정 날짜"}dd{
                            {first.modified.and_then(|time| chrono::DateTime::from_timestamp(time as i64,0))
                                .map(|time| time.with_timezone(&chrono::Local).format("%Y.%m.%d %H:%M").to_string())
                                .unwrap_or_else(||"—".into())}
                        }
                    }
                } else if first.is_dir {
                    p{"파일을 선택하면 내용을 표시합니다."}
                } else {
                    match preview {
                        Some(Ok(Preview::Text { text, truncated })) => rsx! {
                            pre{"{text}"} if *truncated {p{"파일의 처음 64 KiB를 표시합니다."}}
                        },
                        Some(Ok(Preview::Image { url })) => rsx! {img{src:"{url}",alt:"{first.name}"}},
                        Some(Err(error)) => rsx! {p{"{error}"}},
                        None => rsx! {p{"읽는 중…"}},
                    }
                }
            }
        } else {
            p{"파일 또는 폴더를 선택하세요."}
        }
    }}
}

pub(super) struct FolderSearch {
    pane: usize,
    root: NativePath,
    index_id: u64,
    ready: bool,
    text: String,
    due: Option<Instant>,
    query_id: u64,
    pub(super) result: Option<SearchResult>,
    error: Option<String>,
}

pub(super) fn open_folder_search(mut model: Signal<AppModel>, bridge: &Bridge, pane: usize) {
    let mut state = model.write();
    let root = state.panes[pane].tab().path.clone();
    let index_id = state.id();
    state.folder_search = Some(FolderSearch {
        pane,
        root: root.clone(),
        index_id,
        ready: false,
        text: String::new(),
        due: None,
        query_id: 0,
        result: None,
        error: None,
    });
    bridge
        .folder_search
        .replace(Request::Index { id: index_id, root });
}

fn close_folder_search(mut model: Signal<AppModel>, bridge: &Bridge) {
    let pane = model.write().folder_search.take().map(|search| search.pane);
    bridge.folder_search.cancel();
    if let Some(pane) = pane {
        focus_list(pane);
    }
}

pub(super) fn scoped_index_event(
    mut model: Signal<AppModel>,
    id: u64,
    status: &IndexStatus,
) -> bool {
    if !model
        .read()
        .folder_search
        .as_ref()
        .is_some_and(|s| s.index_id == id)
    {
        return false;
    }
    let mut state = model.write();
    let Some(search) = state.folder_search.as_mut().filter(|s| s.index_id == id) else {
        return false;
    };
    if status.complete || status.limited {
        search.ready = true;
    }
    true
}

pub(super) fn scoped_query_event(
    mut model: Signal<AppModel>,
    id: u64,
    result: &Result<SearchResult, String>,
) -> bool {
    if !model
        .read()
        .folder_search
        .as_ref()
        .is_some_and(|s| s.query_id == id && id != 0)
    {
        return false;
    }
    let mut state = model.write();
    let Some(search) = state
        .folder_search
        .as_mut()
        .filter(|s| s.query_id == id && id != 0)
    else {
        return false;
    };
    match result {
        Ok(result) => search.result = Some(result.clone()),
        Err(error) => search.error = Some(error.clone()),
    }
    true
}

pub(super) fn scoped_failure(mut model: Signal<AppModel>, id: u64, error: &str) -> bool {
    if !model
        .read()
        .folder_search
        .as_ref()
        .is_some_and(|s| s.index_id == id)
    {
        return false;
    }
    let mut state = model.write();
    let Some(search) = state.folder_search.as_mut().filter(|s| s.index_id == id) else {
        return false;
    };
    search.error = Some(error.to_owned());
    true
}

pub(super) fn scoped_search_tick(mut model: Signal<AppModel>, bridge: &Bridge) {
    let ready = model.read().folder_search.as_ref().is_some_and(|s| {
        s.ready && s.due.is_some_and(|due| due <= Instant::now()) && !s.text.trim().is_empty()
    });
    if !ready {
        return;
    }
    let mut state = model.write();
    let search = state.folder_search.as_mut().unwrap();
    search.due = None;
    let _ = bridge.folder_search.send(Request::Query {
        id: search.query_id,
        text: search.text.clone(),
    });
}

#[component]
pub(super) fn FolderSearchDialog(mut model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let state = model.read();
    let Some(search) = state.folder_search.as_ref() else {
        return rsx! {};
    };
    let root = search.root.display();
    let pane = search.pane;
    let index_id = search.index_id;
    let ready = search.ready;
    let result = search.result.clone();
    let error = search.error.clone();
    let text = search.text.clone();
    let close = bridge.clone();
    let cancel = bridge.clone();
    rsx! {div{class:"modal-backdrop",
        onkeydown:move |event:KeyboardEvent| {
            event.stop_propagation();
            if event.key()==Key::Escape && !event.is_composing() { close_folder_search(model,&close); }
        },
        div{class:"modal folder-search-dialog",role:"dialog","aria-modal":"true","aria-label":"하위 폴더 검색",
            div{class:"inspector-header",h2{"하위 폴더 검색"}button{class:"icon-button",
                "aria-label":"하위 폴더 검색 닫기",onclick:move |_|close_folder_search(model,&cancel),"×"}}
            p{class:"folder-search-root",title:"{root}","{root}"}
            input{class:"folder-search-input",r#type:"search","aria-label":"파일 또는 폴더 이름",
                placeholder:"파일 또는 폴더 이름",spellcheck:false,initial_value:"{text}",
                onmounted:move |_| {
                    if model.read().folder_search.as_ref().is_some_and(|s| s.index_id==index_id) {
                        let _=document::eval("document.querySelector('.folder-search-input')?.focus()");
                    }
                },
                oninput:move |event:FormEvent| {
                    let mut s = model.write();
                    let id = s.id();
                    if let Some(search) = s.folder_search.as_mut().filter(|search| search.index_id==index_id) {
                        search.text = event.value();
                        search.query_id=id;
                        search.result=None;
                        if search.ready { search.error=None; }
                        search.due=Some(Instant::now()+Duration::from_millis(170));
                    }
                }
            }
            if let Some(error) = error {p{role:"alert","{error}"}}
            else if let Some(result) = result {
                p{role:"status","{result.total}개 결과"}
                div{class:"folder-search-hits",
                    for hit in result.hits {
                        {let b=bridge.clone();let path=hit.path.display();let name=hit.name.clone();
                            let is_dir=hit.is_dir;
                            let appearance=if is_dir{None}else{model.read().file_appearance.for_type(&name)};
                            rsx!{button{key:"{path}",title:"{path}",
                                onclick:move |_| {
                                    close_folder_search(model,&b);
                                    reveal_hit_in(model,&b,hit.clone(),Some(pane));
                                },{file_icon(is_dir, appearance.as_deref(), false)}div{strong{"{name}"}span{"{path}"}}
                            }}
                        }
                    }
                }
            } else if !ready {p{role:"status","현재 폴더와 하위 폴더를 읽고 있습니다."}}
            else if !text.is_empty() {p{role:"status","검색 중…"}}
        }
    }}
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ThumbnailKey {
    pub path: NativePath,
    bytes: Option<u64>,
    modified: Option<u64>,
}
impl From<&FileEntry> for ThumbnailKey {
    fn from(entry: &FileEntry) -> Self {
        Self {
            path: entry.path.clone(),
            bytes: entry.bytes,
            modified: entry.modified,
        }
    }
}

pub(super) fn thumbnail_tick(mut model: Signal<AppModel>, bridge: &Bridge) {
    let keys = {
        let state = model.read();
        let pane = &state.panes[state.active];
        if pane.view != ViewMode::Icons
            || pane.loading
            || state.thumbnail_request.is_some()
            || state.closing
        {
            return;
        }
        let geometry = pane.geometry(state.compact);
        let tab = pane.tab();
        let start = ((tab.scroll / geometry.row_height).floor() as usize * geometry.columns)
            .min(tab.visible.len());
        let count = ((geometry.viewport_height / geometry.row_height).ceil() as usize + 1)
            * geometry.columns;
        let end = (start + count).min(tab.visible.len());
        tab.visible[start..end]
            .iter()
            .map(|&index| &tab.entries[index])
            .filter(|entry| {
                !entry.is_dir && !entry.is_link && entry.cloud != sdir::model::CloudState::Remote
            })
            .filter(|entry| {
                std::path::Path::new(&entry.name)
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|ext| {
                        matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "jpg"
                                | "jpeg"
                                | "png"
                                | "webp"
                                | "mp4"
                                | "mkv"
                                | "mov"
                                | "avi"
                                | "wmv"
                                | "webm"
                        )
                    })
            })
            .map(ThumbnailKey::from)
            .filter(|key| !state.thumbnails.contains_key(key))
            .take(8)
            .collect::<Vec<_>>()
    };
    if keys.is_empty() {
        return;
    }
    let paths = keys.iter().map(|key| key.path.clone()).collect();
    let mut state = model.write();
    let id = state.id();
    state.thumbnail_request = Some((id, keys));
    bridge.thumbnails.replace(Request::Thumbnails { id, paths });
}

pub(super) fn finish_thumbnails(mut model: Signal<AppModel>, id: u64, images: Vec<Option<String>>) {
    if !model
        .read()
        .thumbnail_request
        .as_ref()
        .is_some_and(|(request, _)| *request == id)
    {
        return;
    }
    let mut state = model.write();
    let (_, keys) = state
        .thumbnail_request
        .take()
        .expect("thumbnail request was checked");
    for (index, key) in keys.into_iter().enumerate() {
        while state.thumbnails.len() >= 256 {
            let Some(old) = state.thumbnail_order.pop_front() else {
                break;
            };
            state.thumbnails.remove(&old);
        }
        state.thumbnail_order.push_back(key.clone());
        state.thumbnails.insert(
            key,
            images
                .get(index)
                .and_then(Option::as_ref)
                .map(|data| Arc::<str>::from(data.as_str())),
        );
    }
}

pub(super) fn close_network_dialog(mut model: Signal<AppModel>, disconnect: bool) {
    let pane = {
        let mut state = model.write();
        state.network_dialog = None;
        state.active
    };
    let index = usize::from(disconnect);
    let _ = document::eval(&format!(
        "{{const b=document.querySelectorAll('.network-drive-controls button')[{index}];
         const l=document.querySelector('[data-pane=\"{pane}\"] .list-scroll');
         (b?.getClientRects().length ? b : l)?.focus({{preventScroll:true}});}}"
    ));
}

#[component]
pub(super) fn NetworkDriveDialog(mut model: Signal<AppModel>, disconnect: bool) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let mut drive = use_signal(String::new);
    let mut path = use_signal(String::new);
    let mut remember = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let title = if disconnect {
        "네트워크 드라이브 연결 끊기"
    } else {
        "네트워크 드라이브 연결"
    };
    rsx! {div{class:"modal-backdrop",
        onkeydown:move |e:KeyboardEvent| {
            e.stop_propagation();
            if e.key()==Key::Escape && !e.is_composing() {
                e.prevent_default();
                close_network_dialog(model,disconnect);
            }
        },
        div{class:"modal network-drive-dialog",role:"dialog","aria-modal":"true","aria-label":title,
            h2{"{title}"}
            form{onsubmit:move |e:FormEvent| {
                e.prevent_default();
                let drive = drive.read().trim().to_ascii_uppercase();
                let remote = (!disconnect).then(|| path.read().trim().to_owned());
                if let Err(message) = sdir::utility::validate_network_drive(&drive,remote.as_deref()) {
                    error.set(Some(message));
                    return;
                }
                let action = sdir::utility::Utility::NetworkDrive {drive,remote,remember:*remember.read()};
                if let Err(message) = bridge.utility(action) {
                    error.set(Some(message));
                } else {
                    model.write().notice="Windows에서 네트워크 드라이브 요청을 처리하고 있습니다.".into();
                    close_network_dialog(model,disconnect);
                }
            },
                label{span{"드라이브 문자"}input{class:"network-drive-letter",placeholder:"Z:",maxlength:2,
                    onmounted:move |_| {
                        if model.read().network_dialog == Some(disconnect) {
                            let _ = document::eval("document.querySelector('.network-drive-dialog .network-drive-letter')?.focus({preventScroll:true});");
                        }
                    },
                    oninput:move|e:FormEvent|drive.set(e.value())}}
                if !disconnect {
                    label{span{"공유 폴더 경로"}input{class:"network-drive-path",placeholder:r"\\서버\공유",maxlength:2048,
                        oninput:move|e:FormEvent|path.set(e.value())}}
                    label{class:"network-remember",input{r#type:"checkbox",checked:*remember.read(),
                        onchange:move|e:FormEvent|remember.set(e.checked())}span{"로그인할 때 다시 연결"}}
                }
                if let Some(ref message)=*error.read(){p{class:"pane-error",role:"alert","{message}"}}
                div{class:"modal-actions",
                    button{r#type:"button",class:"network-cancel",onclick:move |_|close_network_dialog(model,disconnect),"취소"}
                    button{r#type:"submit",class:"primary network-submit",if disconnect {"연결 끊기"}else{"연결"}}
                }
            }
        }
    }}
}
