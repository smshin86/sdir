use dioxus::html::{Code, HasFileData};
use dioxus::prelude::*;
use sdir::{
    filesystem::readable_size,
    history::Record,
    model::{FileAction, GroupBy, NativePath, Pane, SortColumn, ViewMode, sort_entries},
    platform,
    search::{IndexStatus, SearchHit, SearchResult},
    settings::{self, Settings, Theme, WindowPlacement, Workspace},
    worker::{Bridge, Event as WorkerEvent, Request},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};

fn row_height(compact: bool) -> f64 {
    if compact { 30. } else { 38. }
}
mod extras;
mod file_appearance;
use file_appearance::*;
mod parity;
use parity::*;
mod pane_arrangement;
use pane_arrangement::{LayoutSwitcher, grid_style};
use sdir::pane_arrangement::PaneArrangement;
#[cfg(windows)]
#[path = "ui/lifecycle.rs"]
mod lifecycle;
#[cfg(target_os = "macos")]
#[path = "ui/lifecycle_macos.rs"]
mod lifecycle;
#[path = "ui/session_close.rs"]
mod session_close;

const PLATFORM_NAME: &str = if cfg!(windows) { "Windows" } else { "macOS" };

fn command_modifier(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.meta()
    } else {
        modifiers.ctrl()
    }
}
use extras::*;
const STYLE: &str = include_str!("style.css");
const SIDEBAR_MIN_WIDTH: u16 = 155;
const SIDEBAR_MAX_WIDTH: u16 = 360;
#[derive(Clone)]
enum Dialog {
    NewFolder,
    Recycle(Vec<NativePath>),
    Delete(Vec<NativePath>),
}
#[derive(Clone)]
struct PaneLocation {
    pane: usize,
    tab: usize,
    generation: u64,
    path: NativePath,
}
impl PaneLocation {
    fn capture(s: &AppModel, pane: usize) -> Self {
        let p = &s.panes[pane];
        Self {
            pane,
            tab: p.active,
            generation: p.generation,
            path: p.tab().path.clone(),
        }
    }
    fn matches(&self, s: &AppModel) -> bool {
        let p = &s.panes[self.pane];
        self.pane < s.layout
            && self.tab == p.active
            && self.generation == p.generation
            && self.path == p.tab().path
    }
}
#[derive(Clone)]
struct InlineRename {
    id: u64,
    location: PaneLocation,
    source: NativePath,
    original: String,
    draft: String,
    interaction: u64,
    submitted_job: Option<u64>,
}

#[derive(Clone)]
struct PasteContext {
    target: PaneLocation,
    origin: PaneLocation,
    interaction: u64,
}
#[derive(Clone)]
struct CopyDestination {
    sources: Vec<NativePath>,
    origin: PaneLocation,
    destination: Option<(usize, usize, NativePath)>,
    error: Option<String>,
}
#[derive(Clone)]
struct ScopeEditDraft {
    guid: sdir::index_policy::VolumeGuid,
    include_text: String,
    exclude_text: String,
    error: Option<String>,
}
struct JobContext {
    action: FileAction,
    history: Option<bool>,
    origin: PaneLocation,
    interaction: u64,
    reveal: Option<PasteContext>,
    folders: Vec<NativePath>,
    cut_sources: Vec<NativePath>,
}
struct PendingReveal {
    generation: u64,
    paths: Vec<NativePath>,
    interaction: u64,
    focus: bool,
    preserve_scroll: Option<f64>,
    from_search: bool,
}
#[derive(Clone)]
struct FolderWatch {
    id: u64,
    path: NativePath,
    ready: bool,
}
#[derive(Clone)]
struct JobProgressStatus {
    id: u64,
    kind: &'static str,
    message: String,
    percent: Option<u8>,
}
fn job_progress_status(id: u64, action: &FileAction) -> Option<JobProgressStatus> {
    let (sources, destination) = match action {
        FileAction::Copy {
            sources,
            destination,
        }
        | FileAction::Move {
            sources,
            destination,
        } => (sources, destination),
        _ => return None,
    };
    let message = if sources.len() == 1 {
        format!(
            "{} 중 · {}개 · {} → {}",
            action.label(),
            sources.len(),
            sources[0].name(),
            destination.display()
        )
    } else {
        format!(
            "{} 중 · {}개 → {}",
            action.label(),
            sources.len(),
            destination.display()
        )
    };
    Some(JobProgressStatus {
        id,
        kind: action.label(),
        message,
        percent: None,
    })
}
struct AppModel {
    panes: Vec<Pane>,
    pane_names: Vec<String>,
    theme: Theme,
    compact: bool,
    action_bar_visible: bool,
    sidebar_visible: bool,
    sidebar_width: Option<u16>,
    search_preferences: sdir::preferences::SearchPreferences,
    ntfs_indexing: sdir::index_policy::NtfsIndexingSettings,
    resume_live_index: bool,
    close_to_tray: bool,
    start_on_login: bool,
    preferences_open: bool,
    workspaces_open: bool,
    workspaces: Vec<Workspace>,
    closing: bool,
    memory_saving: bool,
    close_save_failed: bool,
    window: Option<WindowPlacement>,
    tree: sdir::tree::Tree,
    tree_cursor: Option<NativePath>,
    tree_follow: bool,
    tree_seen: Option<NativePath>,
    tree_tasks: std::collections::VecDeque<(NativePath, bool)>,
    tree_pending_auto: bool,
    layout: usize,
    pane_arrangement: PaneArrangement,
    active: usize,
    favorites: Vec<NativePath>,
    recent_files: Vec<NativePath>,
    folder_visits: Vec<settings::FolderVisit>,
    remember_activity: bool,
    visited: [Option<NativePath>; 4],
    show_hidden: bool,
    show_extensions: bool,
    selection_checkboxes: bool,
    notice: String,
    utility_pid: Option<u32>,
    search_open: bool,
    search_target: Option<usize>,
    search_expanded: bool,
    search_focus: Option<u64>,
    index_id: u64,
    config_generation: u64,
    host_pid: Option<u32>,
    config_pending: Option<(
        u64,
        u64,
        Option<u32>,
        sdir::index_policy::NtfsIndexingSettings,
    )>,
    pending_rebuild: Option<(
        u64,
        u64,
        u64,
        sdir::index_policy::VolumeGuid,
        Option<u32>,
        bool,
    )>,
    index: Option<IndexStatus>,
    index_starting: bool,
    spotlight: bool,
    search_live: bool,
    index_dirty: bool,
    query: String,
    query_requested: bool,
    query_due: Option<Instant>,
    query_id: u64,

    query_loading: bool,
    result: Option<SearchResult>,
    search_error: Option<String>,
    index_error: Option<String>,
    next_id: u64,
    job: Option<u64>,
    job_status: Option<JobProgressStatus>,
    batch: Option<BatchDialog>,
    journal: Vec<sdir::journal::Entry>,
    journal_open: bool,
    appearance: Option<usize>,
    context: Option<ContextMenu>,
    pending_context: Option<(u64, ContextMenu)>,
    dialog: Option<Dialog>,
    inline_rename: Option<InlineRename>,
    address_history: Option<usize>,
    inspector: Option<Inspector>,
    file_preview: Option<PreviewRequest>,
    folder_search: Option<FolderSearch>,
    network_dialog: Option<bool>,
    thumbnails: BTreeMap<ThumbnailKey, Option<Arc<str>>>,
    thumbnail_order: std::collections::VecDeque<ThumbnailKey>,
    thumbnail_request: Option<(u64, Vec<ThumbnailKey>)>,
    file_appearance: AppearanceCache,
    copy_destination: Option<CopyDestination>,
    dialog_name: String,
    pastes: BTreeMap<u64, (PasteContext, bool)>,
    job_context: Option<JobContext>,
    undo: Vec<Record>,
    redo: Vec<Record>,
    history_direction: Option<bool>,
    interaction: u64,
    clipboard_marked: Arc<BTreeSet<NativePath>>,
    clipboard_cut: bool,
    clipboard_sequence: Option<u32>,
    clipboard_pending: Option<(u64, Vec<NativePath>, bool)>,
    reveal: BTreeMap<usize, PendingReveal>,
    file_drag: Option<ArmedDrag>,
    #[cfg(windows)]
    external_drag: Option<(u64, sdir::drag::MouseGesture)>,
    save_due: Option<Instant>,
    pending_focus: Option<(usize, u64)>,
    pending_focus_guarded: bool,
    refreshes: Vec<Option<BackgroundRefresh>>,
    watches: Vec<Option<FolderWatch>>,
    watch_due: Vec<Option<Instant>>,
    watch_renames: Vec<Vec<sdir::watch::Rename>>,
    watch_errors: Vec<Option<String>>,
}
impl AppModel {
    fn new(s: Settings) -> Self {
        let mut search_preferences = s.search_preferences.clone();
        search_preferences.normalize();
        let home = settings::home();
        let pane_names = (0..4)
            .map(|i| settings::pane_name(s.pane_names.get(i).map(String::as_str).unwrap_or(""), i))
            .collect();
        let panes = s.workspace(String::new()).restore_panes(&home);
        let layout = settings::valid_layout(s.layout);
        let workspaces = s
            .workspaces
            .iter()
            .take(16)
            .cloned()
            .filter_map(|mut w| {
                w.name = settings::workspace_name(&w.name);
                (!w.name.is_empty()).then_some(w)
            })
            .collect();
        Self {
            panes,
            pane_names,
            theme: s.theme,
            compact: s.compact,
            action_bar_visible: s.action_bar_visible,
            sidebar_visible: s.sidebar_visible,
            sidebar_width: s
                .sidebar_width
                .map(|width| width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)),
            search_preferences,
            ntfs_indexing: s.ntfs_indexing,
            resume_live_index: s.resume_live_index,
            close_to_tray: s.close_to_tray,
            start_on_login: s.start_on_login,
            preferences_open: false,
            workspaces_open: false,
            workspaces,
            closing: false,
            memory_saving: false,
            close_save_failed: false,
            window: s.window,
            tree: {
                let mut t = sdir::tree::Tree::new(platform::navigation_drives());
                t.open = s.tree_open;
                t
            },
            tree_cursor: None,
            tree_follow: s.tree_follow,
            tree_seen: None,
            tree_tasks: {
                let mut v = s
                    .tree_expanded
                    .into_iter()
                    .map(sdir::tree::tree_key)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .take(64)
                    .collect::<Vec<_>>();
                v.sort_by_key(|p| p.to_path().components().count());
                v.into_iter().map(|p| (p, true)).collect()
            },
            tree_pending_auto: false,
            layout,
            pane_arrangement: s.pane_arrangement.layout(layout).arrangement,
            active: s.active_pane.min(layout - 1),
            favorites: s.favorites,
            recent_files: s.recent_files.into_iter().take(32).collect(),
            folder_visits: s.folder_visits.into_iter().take(64).collect(),
            remember_activity: s.remember_activity,
            visited: Default::default(),
            show_hidden: s.show_hidden,
            show_extensions: s.show_extensions,
            selection_checkboxes: s.selection_checkboxes,
            notice: "준비됐습니다.".into(),
            utility_pid: None,
            search_open: false,
            search_target: None,
            search_expanded: false,
            search_focus: None,
            index_id: 0,
            config_generation: 0,
            host_pid: None,
            config_pending: None,
            pending_rebuild: None,
            index: None,
            index_starting: false,
            spotlight: false,
            search_live: false,
            index_dirty: false,
            query: String::new(),
            query_requested: false,
            query_due: None,
            query_id: 0,

            query_loading: false,
            result: None,
            search_error: None,
            index_error: None,
            next_id: 1,
            job: None,
            job_status: None,
            batch: None,
            journal: vec![],
            journal_open: false,
            appearance: None,
            context: None,
            pending_context: None,
            dialog: None,
            inline_rename: None,
            address_history: None,
            inspector: None,
            file_preview: None,
            folder_search: None,
            network_dialog: None,
            thumbnails: BTreeMap::new(),
            thumbnail_order: Default::default(),
            thumbnail_request: None,
            file_appearance: AppearanceCache::default(),
            copy_destination: None,
            dialog_name: String::new(),
            pastes: BTreeMap::new(),
            job_context: None,
            undo: vec![],
            redo: vec![],
            history_direction: None,
            interaction: 0,
            clipboard_marked: Arc::new(BTreeSet::new()),
            clipboard_cut: false,
            clipboard_sequence: None,
            clipboard_pending: None,
            reveal: BTreeMap::new(),
            file_drag: None,
            #[cfg(windows)]
            external_drag: None,
            save_due: None,
            pending_focus: Some((s.active_pane.min(layout - 1), 0)),
            pending_focus_guarded: false,
            refreshes: (0..4).map(|_| None).collect(),
            watches: vec![None; 4],
            watch_due: vec![None; 4],
            watch_renames: vec![vec![]; 4],
            watch_errors: vec![None; 4],
        }
    }
    fn modal_open(&self) -> bool {
        self.batch.is_some()
            || self.journal_open
            || self.context.is_some()
            || self.appearance.is_some()
            || self.dialog.is_some()
            || self.copy_destination.is_some()
            || self.preferences_open
            || self.workspaces_open
            || self.folder_search.is_some()
            || self.network_dialog.is_some()
    }
    fn id(&mut self) -> u64 {
        let n = self.next_id;
        self.next_id += 1;
        n
    }
    fn selected(&self) -> Vec<NativePath> {
        self.panes[self.active]
            .tab()
            .selection
            .iter()
            .cloned()
            .collect()
    }
    fn path(&self) -> NativePath {
        self.panes[self.active].tab().path.clone()
    }
    fn remember_folder(&mut self, pane: usize) {
        if !self.remember_activity {
            return;
        }
        let path = self.panes[pane].tab().path.clone();
        let key = sdir::tree::tree_key(path.clone());
        if self.visited[pane].as_ref() == Some(&key) {
            return;
        }
        self.visited[pane] = Some(key.clone());
        let existing = self
            .folder_visits
            .iter()
            .position(|visit| sdir::tree::tree_key(visit.path.clone()) == key);
        let count = existing.map_or(1, |index| {
            self.folder_visits.remove(index).count.saturating_add(1)
        });
        self.folder_visits
            .insert(0, settings::FolderVisit { path, count });
        self.folder_visits.truncate(64);
        self.dirty();
    }

    fn remember_open(&mut self, path: NativePath) {
        if !self.remember_activity {
            return;
        }
        let key = sdir::tree::tree_key(path.clone());
        self.recent_files
            .retain(|item| sdir::tree::tree_key(item.clone()) != key);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(32);
        self.dirty();
    }

    fn dirty(&mut self) {
        self.save_due = Some(Instant::now() + Duration::from_millis(800));
    }
    fn settings(&self) -> Settings {
        Settings {
            version: 1,
            layout: self.layout,
            pane_arrangement: self.pane_arrangement,
            panes: self
                .panes
                .iter()
                .map(|p| p.tabs.iter().map(|t| t.path.clone()).collect())
                .collect(),
            favorites: self.favorites.clone(),
            recent_files: self.recent_files.clone(),
            folder_visits: self.folder_visits.clone(),
            remember_activity: self.remember_activity,
            show_hidden: self.show_hidden,
            show_extensions: self.show_extensions,
            selection_checkboxes: self.selection_checkboxes,
            pane_names: self.pane_names.clone(),
            theme: self.theme,
            compact: self.compact,
            action_bar_visible: self.action_bar_visible,
            sidebar_visible: self.sidebar_visible,
            sidebar_width: self.sidebar_width,
            search_preferences: self.search_preferences.clone(),
            ntfs_indexing: self.ntfs_indexing.clone(),
            resume_live_index: self.resume_live_index,
            close_to_tray: self.close_to_tray,
            start_on_login: self.start_on_login,
            active_tabs: self.panes.iter().map(|p| p.active).collect(),
            active_pane: self.active,
            workspaces: self.workspaces.clone(),
            pane_views: self.panes.iter().map(|p| p.view).collect(),
            pane_columns: self.panes.iter().map(|p| p.columns.clone()).collect(),
            pane_icon_sizes: self.panes.iter().map(|p| p.icon_size).collect(),
            tab_states: self
                .panes
                .iter()
                .map(|p| p.tabs.iter().map(|t| t.snapshot()).collect())
                .collect(),
            window: self.window.clone(),
            tree_expanded: self
                .tree
                .nodes
                .iter()
                .filter(|(_, n)| n.expanded)
                .map(|(p, _)| p.clone())
                .chain(
                    self.tree_tasks
                        .iter()
                        .filter(|(_, expanded)| *expanded)
                        .map(|(p, _)| p.clone()),
                )
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(64)
                .collect(),
            tree_follow: self.tree_follow,
            tree_open: self.tree.open,
        }
    }
    fn edit_query(&mut self, query: String) {
        self.query = query;
        self.query_id += 1;
        self.query_requested = self.search_preferences.instant;
        self.query_loading = false;
        self.result = None;
        self.search_error = None;
        self.query_due = (self.query_requested
            && self.index.is_some()
            && self.config_pending.is_none()
            && self.pending_rebuild.is_none())
        .then(|| Instant::now() + Duration::from_millis(180));
    }

    fn submit_query(&mut self) {
        self.query_requested = true;
        self.query_id += 1;
        self.query_loading = false;
        self.result = None;
        self.search_error = None;
        self.query_due = (self.index.is_some()
            && self.config_pending.is_none()
            && self.pending_rebuild.is_none())
        .then(Instant::now);
    }

    fn set_search_preferences(&mut self, mut value: sdir::preferences::SearchPreferences) {
        value.normalize();
        let instant_changed = value.instant != self.search_preferences.instant;
        self.search_preferences = value;
        if instant_changed {
            self.edit_query(self.query.clone());
        }
        self.save_due = Some(Instant::now());
    }
}

pub fn launch() {
    let login_start = std::env::args().any(|arg| arg == "--login-start");
    if matches!(lifecycle::claim_window(login_start), Ok(false)) {
        return;
    }
    let settings = settings::load();
    dioxus::LaunchBuilder::desktop()
        .with_cfg(window_config(settings.window.as_ref(), false))
        .launch(app);
}

#[derive(Clone)]
struct IndependentWindow(NativePath);

fn window_config(
    placement: Option<&WindowPlacement>,
    independent: bool,
) -> dioxus::desktop::Config {
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
    let size = placement
        .map(|p| (p.width.clamp(980., 16384.), p.height.clamp(580., 16384.)))
        .unwrap_or((1380., 880.));
    let suffix = if independent { " · 새 창" } else { "" };
    let window = WindowBuilder::new()
        .with_title(format!(
            "SDIR {} · {PLATFORM_NAME}{suffix}",
            env!("CARGO_PKG_VERSION")
        ))
        .with_inner_size(LogicalSize::new(size.0, size.1))
        .with_min_inner_size(LogicalSize::new(980., 580.));
    #[cfg(windows)]
    let window = {
        use dioxus::desktop::tao::{
            platform::windows::{IconExtWindows, WindowBuilderExtWindows},
            window::Icon,
        };
        let icon = Icon::from_resource(101, None).ok();
        window
            .with_window_icon(icon.clone())
            .with_taskbar_icon(icon)
    };
    let config = Config::new()
        .with_window(window)
        .with_menu(None)
        .with_data_directory(settings::data_dir().join("webview"));
    // Remote inspection is enabled only for explicitly requested local development runs.
    #[cfg(windows)]
    let config = match std::env::var("SDIR_DEBUG_PORT") {
        Ok(port) if port.parse::<u16>().is_ok() => {
            config.with_windows_browser_args(format!("--remote-debugging-port={port}"))
        }
        _ => config,
    };
    config
}

fn new_window(path: NativePath) {
    let dom = VirtualDom::new(app).with_root_context(IndependentWindow(path));
    dioxus::desktop::window().new_window(dom, window_config(None, true));
}
#[cfg(target_os = "macos")]
fn close_macos_window(model: Signal<AppModel>) {
    let window = dioxus::desktop::window();
    if try_consume_context::<IndependentWindow>().is_some() {
        window.close();
        return;
    }
    let writer = consume_context::<Arc<settings::SettingsWriter>>();
    let bridge = consume_context::<Arc<Bridge>>();
    session_close::save_then_close(model, &writer, &window, &bridge);
}

fn active_pane(model: Signal<AppModel>) -> usize {
    model.read().active
}
fn activate_pane(mut model: Signal<AppModel>, pane: usize) {
    let mut s = model.write();
    if s.active != pane {
        s.active = pane;
        s.address_history = None;
        s.pending_focus = None;
        s.dirty();
    }
}
fn name_pane(mut model: Signal<AppModel>, pane: usize, value: String) {
    let mut s = model.write();
    s.pane_names[pane] = settings::pane_name(&value, pane);
    s.save_due = Some(Instant::now());
}
fn set_hidden(mut model: Signal<AppModel>, bridge: &Bridge, hidden: bool) {
    bridge.tree.cancel();
    let layout = {
        let mut s = model.write();
        s.show_hidden = hidden;
        s.tree.reset();
        for pane in &mut s.panes {
            for tab in &mut pane.tabs {
                tab.loaded = false;
            }
        }
        s.dirty();
        s.layout
    };
    for pane in 0..layout {
        load(model, bridge, pane);
    }
}

fn load(mut model: Signal<AppModel>, bridge: &Bridge, pane: usize) {
    let (id, path, hidden) = {
        let mut s = model.write();
        s.reveal.remove(&pane);
        if s.address_history == Some(pane) {
            s.address_history = None;
        }
        if s.inline_rename
            .as_ref()
            .is_some_and(|edit| edit.location.pane == pane)
        {
            s.inline_rename = None;
        }
        if let Some(r) = s.refreshes[pane].take() {
            s.panes[pane].generation = s.panes[pane].generation.max(r.id);
        }
        let hidden = s.show_hidden;
        let p = &mut s.panes[pane];
        if matches!(p.view, ViewMode::Icons | ViewMode::Tiles) {
            let tab = p.tab_mut();
            tab.group_by = GroupBy::None;
            if let Some(saved) = tab.resume.as_mut() {
                saved.group_by = GroupBy::None;
            }
        }
        let id = p.begin_load();
        (id, p.tab().path.clone(), hidden)
    };
    let watch_request = {
        let mut s = model.write();
        if pane < s.layout && s.watches[pane].as_ref().is_none_or(|w| w.path != path) {
            let watch_id = s.id();
            s.watches[pane] = Some(FolderWatch {
                id: watch_id,
                path: path.clone(),
                ready: false,
            });
            s.watch_errors[pane] = None;
            s.watch_due[pane] = None;
            s.watch_renames[pane].clear();
            Some(Request::Watch {
                pane,
                id: watch_id,
                path: path.clone(),
            })
        } else {
            None
        }
    };
    if let Some(request) = watch_request {
        bridge.watches[pane].replace(request);
    }
    bridge.panes[pane].replace(Request::List {
        pane,
        id,
        path,
        hidden,
    });
}
fn navigate(mut model: Signal<AppModel>, bridge: &Bridge, pane: usize, path: NativePath) {
    release_edit_focus(pane);
    model.write().panes[pane].navigate(path);
    model.write().dirty();
    load(model, bridge, pane);
    focus_list_after_render(model, pane);
}
fn reveal_hit(model: Signal<AppModel>, bridge: &Bridge, hit: SearchHit) {
    reveal_hit_in(model, bridge, hit, None);
}

fn reveal_hit_in(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    hit: SearchHit,
    target: Option<usize>,
) {
    if hit.volume_guid.is_some() || hit.volume_mount.is_some() {
        let matches = match (&hit.volume_guid, &hit.volume_mount) {
            (Some(guid), Some(mount)) => sdir::live_search::result_mount_matches(guid, mount),
            _ => false,
        };
        if !matches {
            let mut state = model.write();
            state.query_id += 1;
            state.result = None;
            state.query_loading = false;
            state.search_error =
                Some("드라이브가 바뀌어 검색 결과를 다시 확인하고 있습니다.".into());
            state.query_due =
                (state.search_live && state.query_requested && !state.query.is_empty())
                    .then(Instant::now);
            return;
        }
    }
    let path = hit.path;
    let pane = {
        let state = model.read();
        target
            .or(state.search_target)
            .filter(|&target| target < state.layout)
            .unwrap_or(state.active)
    };
    if let Some(parent) = path.parent() {
        {
            let mut state = model.write();
            if !state.panes[pane].new_tab_at(parent) {
                state.search_error =
                    Some("대상 패널의 탭이 16개입니다. 탭을 닫은 뒤 다시 시도하세요.".into());
                return;
            }
            state.search_error = None;
            state.search_expanded = false;
            state.active = pane;
            state.dirty();
        }
        release_edit_focus(pane);
        load(model, bridge, pane);
        focus_list_after_render(model, pane);
        let mut state = model.write();
        let generation = state.panes[pane].generation;
        let interaction = state.interaction;
        state.reveal.insert(
            pane,
            PendingReveal {
                generation,
                paths: vec![path],
                interaction,
                focus: true,
                preserve_scroll: None,
                from_search: true,
            },
        );
    }
}
fn submit(model: Signal<AppModel>, bridge: &Bridge, action: FileAction) {
    submit_context(model, bridge, action, None);
}
fn submit_context(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    action: FileAction,
    requested: Option<PasteContext>,
) {
    if model.read().job.is_some() {
        model.write().notice = "진행 중인 파일 작업이 끝난 뒤 다시 시도하세요.".into();
        return;
    }
    let context = {
        let s = model.read();
        let destination = action.destination();
        let reveal = requested.or_else(|| {
            let path = destination.as_ref()?;
            let next = (s.active + 1) % s.layout;
            let order = if matches!(action, FileAction::Copy { .. } | FileAction::Move { .. }) {
                [next, s.active]
            } else {
                [s.active, next]
            };
            let pane = order
                .into_iter()
                .chain(0..s.layout)
                .find(|&p| s.panes[p].tab().path == *path)?;
            Some(PasteContext {
                target: PaneLocation::capture(&s, pane),
                origin: PaneLocation::capture(&s, s.active),
                interaction: s.interaction,
            })
        });
        let mut folders: Vec<_> = destination.into_iter().collect();
        match &action {
            FileAction::Move { sources, .. }
            | FileAction::Recycle { sources }
            | FileAction::Delete { sources } => {
                folders.extend(sources.iter().filter_map(NativePath::parent))
            }
            FileAction::Rename { source, .. } => folders.extend(source.parent()),
            FileAction::History { record } => folders.extend(record.folders()),
            _ => {}
        }
        let cut_sources = if let FileAction::Move { sources, .. } = &action {
            sources.clone()
        } else {
            vec![]
        };
        JobContext {
            action: action.clone(),
            history: if matches!(action, FileAction::History { .. }) {
                s.history_direction
            } else {
                None
            },
            origin: PaneLocation::capture(&s, s.active),
            interaction: s.interaction,
            reveal,
            folders,
            cut_sources,
        }
    };
    let id = model.write().id();
    let status = job_progress_status(id, &action);
    match bridge.job(Request::Job { id, action }) {
        Ok(()) => {
            let mut s = model.write();
            s.job = Some(id);
            s.job_status = status;
            s.job_context = Some(context);
            s.notice =
                "Windows에서 파일을 처리하고 있습니다. 취소는 Windows 진행 창에서 할 수 있습니다."
                    .into();
            s.dialog = None;
        }
        Err(e) => model.write().notice = e,
    }
}

fn history_action(mut model: Signal<AppModel>, bridge: &Bridge, redo: bool) {
    if model.read().job.is_some() {
        return;
    }
    let record = {
        let s = model.read();
        if redo {
            s.redo.last().cloned()
        } else {
            s.undo.last().cloned()
        }
    };
    if let Some(record) = record {
        model.write().history_direction = Some(redo);
        submit(model, bridge, FileAction::History { record });
    } else {
        model.write().notice = if redo {
            "다시 실행할 파일 작업이 없습니다."
        } else {
            "실행 취소할 파일 작업이 없습니다. 덮어쓰기·영구 삭제는 기록에서 제외합니다."
        }
        .into();
    }
}
fn copy(mut model: Signal<AppModel>, bridge: &Bridge, cut: bool) {
    let paths = model.read().selected();
    if paths.is_empty() {
        return;
    }
    let id = model.write().id();
    match bridge.job(Request::ClipboardSet {
        id,
        paths: paths.clone(),
        cut,
    }) {
        Ok(()) => {
            let mut s = model.write();
            s.clipboard_pending = Some((id, paths, cut));
            s.notice = "파일 목록을 클립보드에 기록하고 있습니다.".into();
        }
        Err(e) => model.write().notice = e,
    }
}
fn paste(model: Signal<AppModel>, bridge: &Bridge) {
    paste_with_intent(model, bridge, false);
}

fn paste_with_intent(mut model: Signal<AppModel>, bridge: &Bridge, move_files: bool) {
    if model.read().job.is_some() {
        model.write().notice = "진행 중인 파일 작업이 끝난 뒤 다시 시도하세요.".into();
        return;
    }
    let (id, context) = {
        let mut s = model.write();
        let id = s.id();
        let target = PaneLocation::capture(&s, s.active);
        (
            id,
            PasteContext {
                target: target.clone(),
                origin: target,
                interaction: s.interaction,
            },
        )
    };
    model.write().pastes.insert(id, (context, move_files));
    if let Err(e) = bridge.job(Request::ClipboardGet { id }) {
        let mut s = model.write();
        s.pastes.remove(&id);
        s.notice = e;
    }
}

fn open_copy_destination(mut model: Signal<AppModel>) {
    let s = model.read();
    let sources = s.selected();
    if sources.is_empty() || s.layout < 2 || s.job.is_some() {
        return;
    }
    let state = CopyDestination {
        sources,
        origin: PaneLocation::capture(&s, s.active),
        destination: None,
        error: None,
    };
    drop(s);
    model.write().copy_destination = Some(state);
}
fn close_copy_destination(mut model: Signal<AppModel>) {
    let origin = model.write().copy_destination.take().map(|d| d.origin.pane);
    if let Some(pane) = origin {
        model.write().active = pane;
        focus_list_after_render(model, pane);
    }
}
fn choose_copy_panel(mut model: Signal<AppModel>, pane: usize) {
    let mut s = model.write();
    let Some(dialog) = &s.copy_destination else {
        return;
    };
    if pane >= s.layout || pane == dialog.origin.pane {
        return;
    }
    let tab = s.panes[pane].active;
    let path = s.panes[pane].tab().path.clone();
    let dialog = s.copy_destination.as_mut().unwrap();
    dialog.destination = Some((pane, tab, path));
    dialog.error = None;
}
fn choose_copy_tab(mut model: Signal<AppModel>, tab: usize) {
    let mut s = model.write();
    let Some((pane, _, _)) = s
        .copy_destination
        .as_ref()
        .and_then(|d| d.destination.clone())
    else {
        return;
    };
    let Some(target) = s.panes[pane].tabs.get(tab) else {
        return;
    };
    let path = target.path.clone();
    let dialog = s.copy_destination.as_mut().unwrap();
    dialog.destination = Some((pane, tab, path));
    dialog.error = None;
}
fn confirm_copy_destination(mut model: Signal<AppModel>, bridge: &Bridge) {
    let Some(dialog) = model.read().copy_destination.clone() else {
        return;
    };
    let Some((pane, tab, destination)) = dialog.destination.clone() else {
        return;
    };
    let valid = {
        let s = model.read();
        s.job.is_none()
            && dialog.origin.matches(&s)
            && pane < s.layout
            && pane != dialog.origin.pane
            && s.panes[pane]
                .tabs
                .get(tab)
                .is_some_and(|t| t.path == destination)
    };
    if !valid {
        if let Some(d) = model.write().copy_destination.as_mut() {
            d.error = Some("원본 또는 대상 탭이 변경됐습니다. 닫은 뒤 다시 선택하세요.".into());
        }
        return;
    }
    let reload = {
        let mut s = model.write();
        s.active = dialog.origin.pane;
        let p = &mut s.panes[pane];
        let reload = p.active != tab || !p.tabs[tab].loaded;
        p.active = tab;
        p.address = destination.display();
        s.dirty();
        reload
    };
    if reload {
        load(model, bridge, pane);
    }
    let context = {
        let s = model.read();
        PasteContext {
            target: PaneLocation::capture(&s, pane),
            origin: dialog.origin,
            interaction: s.interaction,
        }
    };
    submit_context(
        model,
        bridge,
        FileAction::Copy {
            sources: dialog.sources,
            destination,
        },
        Some(context),
    );
    if model.read().job.is_some() {
        model.write().copy_destination = None;
    } else {
        let message = model.read().notice.clone();
        if let Some(d) = model.write().copy_destination.as_mut() {
            d.error = Some(message);
        }
    }
}

fn reset_volume_session(s: &mut AppModel, id: u64, live: bool) {
    s.index_id = id;
    s.config_generation = 0;
    s.host_pid = None;
    s.config_pending = None;
    s.pending_rebuild = None;
    s.index = None;
    s.index_starting = true;
    s.search_live = live;
    s.spotlight = false;
    s.index_dirty = false;
    s.result = None;
    s.search_error = None;
    s.index_error = None;
    s.search_open = true;
    s.query_loading = false;
    s.query_due = None;
    s.query_id += 1;
}

#[cfg(target_os = "macos")]
fn start_spotlight(mut model: Signal<AppModel>, bridge: &Bridge) {
    let id = {
        let mut state = model.write();
        let open = state.search_open;
        let id = state.id();
        reset_volume_session(&mut state, id, false);
        state.spotlight = true;
        state.search_open = open;
        id
    };
    bridge.search.replace(Request::IndexSpotlight { id });
}

fn start_index(mut model: Signal<AppModel>, bridge: &Bridge) {
    let (id, root) = {
        let mut s = model.write();
        let id = s.id();
        reset_volume_session(&mut s, id, false);
        s.resume_live_index = false;
        s.save_due = Some(Instant::now());
        (id, s.path())
    };
    bridge.search.replace(Request::Index { id, root });
}

fn start_volume_index(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    memory_mib: u16,
    service_only: bool,
) {
    if try_consume_context::<IndependentWindow>().is_some() {
        model.write().notice = "전체 드라이브 색인 설정은 처음 연 SDIR 창에서 변경하세요.".into();
        return;
    }
    let (id, settings) = {
        let mut s = model.write();
        let id = s.id();
        let search_open = s.search_open;
        reset_volume_session(&mut s, id, true);
        if service_only {
            s.search_open = search_open;
        }
        // Retain the persistent policy after the user has first requested live search.
        let _ = s.ntfs_indexing.set_persistent(true);
        (id, Box::new(s.ntfs_indexing.clone()))
    };
    bridge.search.replace(Request::IndexVolumes {
        id,
        memory_mib,
        service_only,
        settings,
    });
}

fn apply_index_settings(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    settings: sdir::index_policy::NtfsIndexingSettings,
) {
    let command = {
        let mut s = model.write();
        if s.config_pending.is_some() || s.pending_rebuild.is_some() {
            return;
        }
        if s.search_live && s.index_starting {
            return;
        }
        if !settings.is_valid() {
            s.index_error = Some("색인 설정이 유효하지 않습니다.".into());
            return;
        }
        if settings == s.ntfs_indexing {
            s.index_error = None;
            return;
        }
        if !s.search_live {
            s.ntfs_indexing = settings;
            s.save_due = Some(Instant::now());
            s.index_error = None;
            return;
        }
        let Some(generation) = s.config_generation.checked_add(1) else {
            s.index_error = Some("검색 설정 세대가 더 이상 증가할 수 없습니다.".into());
            return;
        };
        let Some(host_pid) = s.host_pid else {
            s.index_error = Some("검색 호스트 상태를 확인한 뒤 다시 시도하세요.".into());
            return;
        };
        s.query_id += 1;
        s.query_loading = false;
        s.query_due = None;
        s.result = None;
        s.search_error = None;
        s.index_error = None;
        s.config_pending = Some((s.index_id, generation, Some(host_pid), settings.clone()));
        Request::Configure {
            session: s.index_id,
            generation,
            settings: Box::new(settings),
        }
    };
    if let Err(error) = bridge.search.send(command) {
        let mut s = model.write();
        s.config_pending = None;
        s.query_loading = false;
        s.result = None;
        s.index_error = Some(error);
    }
}

fn configure_auto_volumes(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    fixed: Option<bool>,
    removable: Option<bool>,
) {
    let mut settings = model.read().ntfs_indexing.clone();
    if let Some(value) = fixed {
        if let Err(error) = settings.set_auto_fixed(value) {
            model.write().index_error = Some(error.to_string());
            return;
        }
    }
    if let Some(value) = removable {
        if let Err(error) = settings.set_auto_removable(value) {
            model.write().index_error = Some(error.to_string());
            return;
        }
    }
    apply_index_settings(model, bridge, settings);
}

fn configure_volume(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    guid: sdir::index_policy::VolumeGuid,
    mode: Option<sdir::index_policy::VolumeOverride>,
    monitor: Option<bool>,
    scope: Option<sdir::index_policy::ScopePolicy>,
) {
    let mut settings = model.read().ntfs_indexing.clone();
    let current = settings.volume_policy(&guid);
    let next_mode = mode
        .unwrap_or_else(|| current.map_or(sdir::index_policy::VolumeOverride::Auto, |p| p.mode()));
    let next_monitor = monitor.unwrap_or_else(|| current.map_or(true, |p| p.monitor()));
    let next_scope = scope.unwrap_or_else(|| {
        current.map_or_else(sdir::index_policy::ScopePolicy::default, |p| {
            p.scope().clone()
        })
    });
    let result = if next_mode == sdir::index_policy::VolumeOverride::Auto
        && next_monitor
        && next_scope.include_roots().is_empty()
        && next_scope.exclude_roots().is_empty()
    {
        settings.remove_volume_policy(&guid).map(|_| ())
    } else {
        settings.set_volume_policy(sdir::index_policy::VolumePolicy::new(
            guid,
            next_mode,
            next_monitor,
            next_scope,
        ))
    };
    if let Err(error) = result {
        model.write().index_error = Some(error.to_string());
        return;
    }
    apply_index_settings(model, bridge, settings);
}

fn scope_roots_text(roots: &[sdir::index_policy::RelativePathRoot]) -> String {
    roots
        .iter()
        .map(|root| root.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_scope_roots(text: &str) -> Result<Vec<sdir::index_policy::RelativePathRoot>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            sdir::index_policy::RelativePathRoot::parse(line)
                .map_err(|error| format!("{}행: {error}", index + 1))
        })
        .collect()
}

fn parse_scope_policy(
    include_text: &str,
    exclude_text: &str,
) -> Result<sdir::index_policy::ScopePolicy, String> {
    let include = parse_scope_roots(include_text)?;
    let exclude = parse_scope_roots(exclude_text)?;
    sdir::index_policy::ScopePolicy::new(include, exclude).map_err(|error| error.to_string())
}

#[component]
fn VolumeScopeEditor(
    model: Signal<AppModel>,
    guid: sdir::index_policy::VolumeGuid,
    scope: sdir::index_policy::ScopePolicy,
    disabled: bool,
) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let mut draft = use_signal(|| None::<ScopeEditDraft>);
    let active_draft = draft.read().clone().filter(|value| value.guid == guid);
    let mut open_draft = draft;
    let open_guid = guid.clone();
    let open_scope = scope.clone();
    rsx! {
        div{class:"volume-scope","data-volume-scope-guid":"{guid.as_str()}",
            div{class:"volume-scope-roots",
                span{class:"volume-scope-label","포함"}
                if scope.include_roots().is_empty(){span{class:"volume-scope-empty","모든 경로"}}
                else{for root in scope.include_roots(){code{class:"volume-scope-root","data-scope-root":"include","{root.as_str()}"}}}
                span{class:"volume-scope-label","제외"}
                if scope.exclude_roots().is_empty(){span{class:"volume-scope-empty","없음"}}
                else{for root in scope.exclude_roots(){code{class:"volume-scope-root","data-scope-root":"exclude","{root.as_str()}"}}}
            }
            if let Some(active_draft)=active_draft.as_ref(){
                div{class:"volume-scope-editor",
                    label{span{"포함할 상대 경로 · 한 줄에 하나"}textarea{class:"volume-scope-include","data-ime-commit":"true","aria-label":"포함할 상대 경로",initial_value:"{active_draft.include_text}",disabled,
                        onchange:move|event:FormEvent|{if let Some(value)=draft.write().as_mut(){value.include_text=event.value();value.error=None;}}}}
                    label{span{"제외할 상대 경로 · 한 줄에 하나"}textarea{class:"volume-scope-exclude","data-ime-commit":"true","aria-label":"제외할 상대 경로",initial_value:"{active_draft.exclude_text}",disabled,
                        onchange:move|event:FormEvent|{if let Some(value)=draft.write().as_mut(){value.exclude_text=event.value();value.error=None;}}}}
                    p{class:"preference-note","비워 두면 포함 제한이 없습니다. 제외 경로는 포함 경로보다 우선합니다."}
                    if let Some(error)=active_draft.error.as_ref(){p{class:"volume-scope-error","{error}"}}
                    div{class:"volume-scope-actions",
                        button{class:"volume-scope-save",disabled,onclick:{
                            let mut save_draft=draft;
                            let save_guid=guid.clone();
                            let save_bridge=bridge.clone();
                            move |_|{
                                let Some(value)=save_draft.read().clone() else{return;};
                                if value.guid!=save_guid{return;}
                                match parse_scope_policy(&value.include_text,&value.exclude_text){
                                    Ok(scope)=>{
                                        configure_volume(model,&save_bridge,save_guid.clone(),None,None,Some(scope));
                                        save_draft.set(None);
                                    }
                                    Err(error)=>{
                                        if let Some(value)=save_draft.write().as_mut(){value.error=Some(error);}
                                    }
                                }
                            }
                        },"저장"}
                        button{class:"volume-scope-cancel",onclick:move |_|draft.set(None),"취소"}
                    }
                }
            }else{
                button{class:"volume-scope-edit",disabled,onclick:move |_|open_draft.set(Some(ScopeEditDraft{
                    guid:open_guid.clone(),
                    include_text:scope_roots_text(open_scope.include_roots()),
                    exclude_text:scope_roots_text(open_scope.exclude_roots()),
                    error:None,
                })),"검색 결과 범위 편집"}
            }
        }
    }
}

fn rebuild_volume(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    guid: sdir::index_policy::VolumeGuid,
) {
    let command = {
        let mut s = model.write();
        if !s.search_live || s.config_pending.is_some() || s.pending_rebuild.is_some() {
            return;
        }
        let Some(host_pid) = s.host_pid else {
            s.index_error = Some("검색 호스트 상태를 확인한 뒤 다시 시도하세요.".into());
            return;
        };
        let request_id = s.id();
        let session = s.index_id;
        let generation = s.config_generation;
        s.query_id += 1;
        s.query_due = None;
        s.query_loading = false;
        s.result = None;
        s.search_error = None;
        s.index_error = None;
        s.pending_rebuild = Some((
            session,
            generation,
            request_id,
            guid.clone(),
            Some(host_pid),
            false,
        ));
        Request::Rebuild {
            session,
            generation,
            request_id,
            guid,
        }
    };
    if let Err(error) = bridge.search.send(command) {
        let mut s = model.write();
        s.pending_rebuild = None;
        s.query_loading = false;
        s.index_error = Some(error);
    }
}

fn dialog(mut model: Signal<AppModel>, kind: &str) {
    let selection = model.read().selected();
    let mut s = model.write();
    match kind {
        "new" => {
            s.dialog = Some(Dialog::NewFolder);
            s.dialog_name = "새 폴더".into();
        }
        "recycle" if !selection.is_empty() => s.dialog = Some(Dialog::Recycle(selection)),
        "delete" if !selection.is_empty() => s.dialog = Some(Dialog::Delete(selection)),
        _ => s.notice = "작업할 항목을 선택하세요.".into(),
    }
}
fn confirm(model: Signal<AppModel>, bridge: &Bridge) {
    let s = model.read();
    let action = match s.dialog.clone() {
        Some(Dialog::NewFolder) => FileAction::NewFolder {
            destination: s.path(),
            name: s.dialog_name.clone(),
        },
        Some(Dialog::Recycle(sources)) => FileAction::Recycle { sources },
        Some(Dialog::Delete(sources)) => FileAction::Delete { sources },
        None => return,
    };
    drop(s);
    submit(model, bridge, action);
}
fn start_inline_rename(mut model: Signal<AppModel>) {
    let (pane, position) = {
        let mut state = model.write();
        if state.modal_open() || state.closing {
            return;
        }
        if state.job.is_some() {
            state.notice = "진행 중인 파일 작업이 끝난 뒤 다시 시도하세요.".into();
            return;
        }
        let pane = state.active;
        let tab = state.panes[pane].tab();
        let mut selected = tab.selection.iter();
        let (Some(path), None) = (selected.next(), selected.next()) else {
            state.notice = "이름을 바꿀 항목 하나를 선택하세요.".into();
            return;
        };
        let Some(position) = tab
            .visible
            .iter()
            .position(|&index| tab.entries[index].path == *path)
        else {
            return;
        };
        let original = tab.entries[tab.visible[position]].name.clone();
        let source = path.clone();
        let location = PaneLocation::capture(&state, pane);
        let id = state.id();
        state.pending_focus = None;
        state.inline_rename = Some(InlineRename {
            id,
            location,
            source,
            draft: original.clone(),
            original,
            interaction: state.interaction,
            submitted_job: None,
        });
        (pane, position)
    };
    scroll_selection(model, pane, position);
}

fn cancel_inline_rename(mut model: Signal<AppModel>, id: u64, return_focus: bool) {
    let pane = {
        let mut state = model.write();
        let Some(edit) = state
            .inline_rename
            .as_ref()
            .filter(|edit| edit.id == id && edit.submitted_job.is_none())
        else {
            return;
        };
        let pane = edit.location.pane;
        state.inline_rename = None;
        pane
    };
    if return_focus {
        focus_list(pane);
    }
}

fn confirm_inline_rename(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    id: u64,
    return_focus: bool,
) -> bool {
    let Some(edit) = model
        .read()
        .inline_rename
        .as_ref()
        .filter(|edit| edit.id == id && edit.submitted_job.is_none())
        .cloned()
    else {
        return true;
    };
    let valid_target = {
        let state = model.read();
        edit.location.matches(&state)
            && !state.modal_open()
            && !state.closing
            && state.panes[edit.location.pane]
                .tab()
                .entries
                .iter()
                .any(|entry| entry.path == edit.source)
    };
    if !valid_target {
        cancel_inline_rename(model, id, false);
        model.write().notice = "이름을 바꿀 항목이 현재 폴더에 없습니다.".into();
        return true;
    }
    if edit.draft == edit.original {
        cancel_inline_rename(model, id, return_focus);
        return true;
    }
    if let Err(error) = sdir::filesystem::validate_name(&edit.draft) {
        model.write().notice = error;
        return false;
    }
    if model.read().job.is_some() {
        model.write().notice = "진행 중인 파일 작업이 끝난 뒤 다시 시도하세요.".into();
        return false;
    }
    submit(
        model,
        bridge,
        FileAction::Rename {
            source: edit.source,
            name: edit.draft,
        },
    );
    let mut state = model.write();
    let job = state.job;
    if let Some(edit) = state.inline_rename.as_mut().filter(|edit| edit.id == id) {
        edit.submitted_job = job;
    }
    job.is_some()
}

fn confirm_inline_rename_on_list_click(model: Signal<AppModel>, bridge: &Bridge) -> bool {
    let id = model.read().inline_rename.as_ref().map(|edit| edit.id);
    if let Some(id) = id {
        if !confirm_inline_rename(model, bridge, id, false) {
            let _ = document::eval(&format!(
                "document.getElementById('inline-rename-{id}')?.focus({{preventScroll:true}});"
            ));
            return false;
        }
    }
    true
}

fn inline_rename_editor(
    mut model: Signal<AppModel>,
    bridge: Arc<Bridge>,
    edit: &InlineRename,
    is_dir: bool,
) -> Element {
    let id = edit.id;
    let name = edit.original.clone();
    let submitting = edit.submitted_job.is_some();
    let stem = if is_dir {
        name.as_str()
    } else {
        std::path::Path::new(&name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(&name)
    };
    let select_end = stem.encode_utf16().count();
    rsx! {
        input {
            key: "{id}", id: "inline-rename-{id}", class: "inline-rename-input",
            "data-ime-commit": "true",
            "aria-label": "파일 또는 폴더 이름", spellcheck: false, initial_value: "{name}",
            readonly: submitting,
            onmounted: move |_| {
                let state = model.read();
                if let Some(current) = state.inline_rename.as_ref().filter(|edit| edit.id == id) {
                    let focus = current.location.matches(&state)
                        && state.active == current.location.pane
                        && current.interaction == state.interaction;
                    let draft = serde_json::to_string(&current.draft).unwrap();
                    let _ = document::eval(&format!(
                        "{{const e=document.getElementById('inline-rename-{id}');if(e){{const draft={draft};if(e.value!==draft)e.value=draft;if({focus}){{e.focus({{preventScroll:true}});e.setSelectionRange(0,{select_end});}}}}}}"
                    ));
                }
            },
            onchange: move |event| {
                event.stop_propagation();
                if let Some(edit) = model.write().inline_rename.as_mut().filter(|edit| edit.id == id) {
                    edit.draft = event.value();
                }
            },
            onclick: move |event| event.stop_propagation(),
            ondoubleclick: move |event| event.stop_propagation(),
            oncontextmenu: move |event| event.stop_propagation(),
            onkeydown: move |event: KeyboardEvent| {
                event.stop_propagation();
                if event.is_composing() { return; }
                match event.key() {
                    Key::Enter => { event.prevent_default(); confirm_inline_rename(model, &bridge, id, true); }
                    Key::Escape => { event.prevent_default(); cancel_inline_rename(model, id, true); }
                    _ => {}
                }
            },
        }
    }
}

fn sort(model: Signal<AppModel>, pane: usize, column: SortColumn) {
    let tab = model.read().panes[pane].active;
    sort_tab(model, pane, tab, column);
}
fn sort_tab(mut model: Signal<AppModel>, pane: usize, tab: usize, column: SortColumn) {
    let mut s = model.write();
    let Some(t) = s.panes.get_mut(pane).and_then(|p| p.tabs.get_mut(tab)) else {
        return;
    };
    if t.sort == column {
        t.descending = !t.descending;
    } else {
        t.sort = column;
        t.descending = false;
    }
    sort_entries(
        Arc::make_mut(&mut t.entries).as_mut_slice(),
        t.sort,
        t.descending,
    );
    t.rebuild_visible();
    s.active = pane;
    s.dirty();
}
fn sort_direction_tab(mut model: Signal<AppModel>, pane: usize, tab: usize, descending: bool) {
    let mut s = model.write();
    let Some(t) = s.panes.get_mut(pane).and_then(|p| p.tabs.get_mut(tab)) else {
        return;
    };
    if t.descending != descending {
        t.descending = descending;
        sort_entries(
            Arc::make_mut(&mut t.entries).as_mut_slice(),
            t.sort,
            t.descending,
        );
        t.rebuild_visible();
    }
    s.active = pane;
    s.dirty();
}
fn focus_list(pane: usize) {
    let _ = document::eval(&format!(
        "document.querySelector('[data-pane=\"{pane}\"] .list-scroll')?.focus({{preventScroll:true}});"
    ));
}
enum TabCommand {
    New,
    Close(usize),
    Restore,
    Select(usize),
}
fn change_tab(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    pane: usize,
    command: TabCommand,
    focus_listing: bool,
) {
    let changed = {
        let mut s = model.write();
        let p = &mut s.panes[pane];
        let changed = match command {
            TabCommand::New => p.new_tab(),
            TabCommand::Close(i) => p.close_tab(i),
            TabCommand::Restore => p.restore_tab(),
            TabCommand::Select(i) if i < p.tabs.len() => {
                p.active = i;
                true
            }
            _ => false,
        };
        if changed {
            s.active = pane;
            s.dirty();
        }
        changed
    };
    if changed {
        if focus_listing {
            // Keep rapid follow-up shortcuts inside a stable event target while the list is replaced.
            let _ = document::eval("document.querySelector('.app')?.focus({preventScroll:true});");
        }
        load(model, bridge, pane);
        if focus_listing {
            focus_list_after_render(model, pane);
        }
    }
}

fn open_workspace(mut model: Signal<AppModel>, bridge: &Bridge, index: usize) {
    let layout = {
        let mut s = model.write();
        if s.job.is_some() {
            return;
        }
        let Some(workspace) = s.workspaces.get(index).cloned() else {
            return;
        };
        let mut panes = workspace.restore_panes(&settings::home());
        for (pane, old) in panes.iter_mut().zip(&s.panes) {
            // A reply queued before switching must never match a fresh pane.
            pane.generation = old.generation + 1;
        }
        s.panes = panes;
        s.layout = settings::valid_layout(workspace.layout);
        s.pane_arrangement = workspace.pane_arrangement.layout(s.layout).arrangement;
        s.active = workspace.active_pane.min(s.layout - 1);
        s.search_target = s.search_target.filter(|&pane| pane < s.layout);
        s.pane_names = (0..4)
            .map(|i| {
                settings::pane_name(
                    workspace
                        .pane_names
                        .get(i)
                        .map(String::as_str)
                        .unwrap_or(""),
                    i,
                )
            })
            .collect();
        s.reveal.clear();
        s.pending_focus = Some((s.active, s.interaction));
        s.workspaces_open = false;
        s.notice = format!("‘{}’ 작업공간을 열었습니다.", workspace.name);
        s.dirty();
        s.layout
    };
    let _ = document::eval("document.querySelector('.app')?.focus({preventScroll:true});");
    for pane in 0..4 {
        bridge.panes[pane].cancel();
    }
    for pane in 0..layout {
        load(model, bridge, pane);
    }
}
fn release_edit_focus(pane: usize) {
    let _ = document::eval(&format!(
        "{{const a=document.activeElement;if(a?.id==='address-{pane}'||a?.id==='pane-query-{pane}')document.querySelector('.app')?.focus({{preventScroll:true}});}}"
    ));
}
fn focus_list_after_render(mut model: Signal<AppModel>, pane: usize) {
    let mut s = model.write();
    s.panes[pane].reveal_revision += 1;
    s.pending_focus = Some((pane, s.interaction));
    s.pending_focus_guarded = false;
}

fn history_navigation(model: Signal<AppModel>, bridge: &Bridge, pane: usize, forward: bool) {
    let tab = model.read().panes[pane].active;
    history_navigation_tab(model, bridge, pane, tab, forward);
}
fn history_navigation_tab(
    mut model: Signal<AppModel>,
    bridge: &Bridge,
    pane: usize,
    tab: usize,
    forward: bool,
) {
    let previous_pane = active_pane(model);
    let changed = {
        let mut s = model.write();
        let changed = match s.panes.get_mut(pane) {
            Some(pane) if pane.active == tab => {
                if forward {
                    pane.forward()
                } else {
                    pane.back()
                }
            }
            _ => false,
        };
        if changed {
            s.active = pane;
            s.dirty();
        }
        changed
    };
    if changed {
        release_edit_focus(previous_pane);
        release_edit_focus(pane);
        load(model, bridge, pane);
        focus_list_after_render(model, pane);
    }
}
fn mouse_history(event: MouseEvent, model: Signal<AppModel>, bridge: &Bridge, pane: usize) {
    use dioxus::html::input_data::MouseButton;
    let forward = match event.trigger_button() {
        Some(MouseButton::Fourth) => false,
        Some(MouseButton::Fifth) => true,
        _ => return,
    };
    event.prevent_default();
    event.stop_propagation();
    let s = model.read();
    if s.modal_open() {
        return;
    }
    drop(s);
    history_navigation(model, bridge, pane, forward);
}

fn focus_address(pane: usize) {
    let _ = document::eval(&format!(
        "{{const e=document.getElementById('address-{pane}');e?.focus();e?.select();}}"
    ));
}

fn pane_shortcut(
    event: &KeyboardEvent,
    mut model: Signal<AppModel>,
    pane: usize,
    bridge: &Bridge,
) -> bool {
    if event.is_composing() {
        return false;
    }
    let control = command_modifier(event.modifiers());
    let key = event.key();
    let alt = event.modifiers().alt();
    #[cfg(target_os = "macos")]
    if control && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("q")) {
        event.prevent_default();
        event.stop_propagation();
        close_macos_window(model);
        return true;
    }
    if key == Key::F11 {
        event.prevent_default();
        event.stop_propagation();
        let window = dioxus::desktop::window();
        window.set_fullscreen(window.fullscreen().is_none());
        return true;
    }
    if key == Key::F6 && !control {
        event.prevent_default();
        event.stop_propagation();
        let reverse = event.modifiers().shift();
        let _ = document::eval(&format!("window.__sdirCycleFocus?.({reverse});"));
        return true;
    }
    if key == Key::F6 {
        event.prevent_default();
        event.stop_propagation();
        let next = {
            let s = model.read();
            if event.modifiers().shift() {
                (s.active + s.layout - 1) % s.layout
            } else {
                (s.active + 1) % s.layout
            }
        };
        model.write().active = next;
        model.write().dirty();
        focus_list(next);
        return true;
    }

    let char_key = |name: &str| matches!(&key,Key::Character(c) if c.eq_ignore_ascii_case(name));
    if (alt || (cfg!(target_os = "macos") && control && key == Key::ArrowUp))
        && matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp)
    {
        event.prevent_default();
        event.stop_propagation();
        if key == Key::ArrowUp {
            let parent = model.read().panes[pane].tab().path.parent();
            if let Some(path) = parent {
                navigate(model, bridge, pane, path);
            }
        } else {
            history_navigation(model, bridge, pane, key == Key::ArrowRight);
        }
        return true;
    }
    if alt && char_key("p") {
        event.prevent_default();
        event.stop_propagation();
        let mode = if event.modifiers().shift() {
            Inspector::Details
        } else {
            Inspector::Preview
        };
        let mut s = model.write();
        s.inspector = (s.inspector != Some(mode)).then_some(mode);
        return true;
    }
    if cfg!(target_os = "macos") && control && !alt && char_key("i") {
        event.prevent_default();
        event.stop_propagation();
        let mut state = model.write();
        state.inspector =
            (state.inspector != Some(Inspector::Details)).then_some(Inspector::Details);
        return true;
    }
    if cfg!(windows) && alt && key == Key::Enter {
        event.prevent_default();
        event.stop_propagation();
        let (folder, paths) = {
            let state = model.read();
            let tab = state.panes[pane].tab();
            (
                tab.path.clone(),
                tab.selection.iter().cloned().collect::<Vec<_>>(),
            )
        };
        if let Err(error) = selected_properties(bridge, folder, paths) {
            model.write().notice = error;
        }
        return true;
    }
    if control && event.modifiers().shift() && char_key("e") {
        event.prevent_default();
        event.stop_propagation();
        let mut state = model.write();
        let path = sdir::tree::tree_key(state.panes[pane].tab().path.clone());
        state.tree.open = true;
        state.tree_cursor = Some(path.clone());
        state.tree_tasks.push_front((path, false));
        state.tree_tasks.truncate(65);
        state.dirty();
        return true;
    }
    if control && event.code() == Code::NumpadAdd {
        event.prevent_default();
        event.stop_propagation();
        fit_columns(model, pane);
        return true;
    }
    if control && !event.modifiers().shift() && char_key("n") {
        event.prevent_default();
        event.stop_propagation();
        new_window(model.read().panes[pane].tab().path.clone());
        return true;
    }
    if control && event.modifiers().shift() && char_key("n") {
        event.prevent_default();
        event.stop_propagation();
        model.write().active = pane;
        dialog(model, "new");
        return true;
    }
    if control && event.modifiers().shift() {
        let value = match event.code() {
            Code::Digit1 => Some("icons-192"),
            Code::Digit2 => Some("icons-144"),
            Code::Digit3 => Some("icons-112"),
            Code::Digit4 => Some("icons-80"),
            Code::Digit5 => Some("list"),
            Code::Digit6 => Some("details"),
            Code::Digit7 => Some("tiles"),
            Code::Digit8 => Some("content"),
            _ => None,
        };
        if let Some(value) = value {
            event.prevent_default();
            event.stop_propagation();
            set_view(model, pane, value);
            return true;
        }
    }

    if key == Key::F5 || (control && char_key("r")) {
        let snapshot = model.read().panes[pane].tab().snapshot();
        model.write().panes[pane].tab_mut().resume_from(&snapshot);
        event.prevent_default();
        event.stop_propagation();
        load(model, bridge, pane);
        focus_list_after_render(model, pane);
        return true;
    }
    if control && !event.modifiers().shift() {
        if let Key::Character(number) = &key {
            if let Ok(number) = number.parse::<usize>() {
                if (1..=9).contains(&number) && number <= model.read().panes[pane].tabs.len() {
                    event.prevent_default();
                    event.stop_propagation();
                    change_tab(model, bridge, pane, TabCommand::Select(number - 1), true);
                    return true;
                }
            }
        }
    }
    let tab_command = if control && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("t"))
    {
        Some(if event.modifiers().shift() {
            TabCommand::Restore
        } else {
            TabCommand::New
        })
    } else if control && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("w")) {
        let (close_window, tab) = {
            let state = model.read();
            let p = &state.panes[pane];
            (
                state.layout == 1 && p.tabs.len() == 1 && !p.tab().pinned,
                p.active,
            )
        };
        if close_window {
            event.prevent_default();
            event.stop_propagation();
            #[cfg(windows)]
            lifecycle::request_close(&dioxus::desktop::window());
            #[cfg(target_os = "macos")]
            close_macos_window(model);
            return true;
        }
        Some(TabCommand::Close(tab))
    } else if control && key == Key::Tab {
        let s = model.read();
        let p = &s.panes[pane];
        let next = if event.modifiers().shift() {
            (p.active + p.tabs.len() - 1) % p.tabs.len()
        } else {
            (p.active + 1) % p.tabs.len()
        };
        Some(TabCommand::Select(next))
    } else {
        None
    };
    if let Some(command) = tab_command {
        event.prevent_default();
        event.stop_propagation();
        change_tab(model, bridge, pane, command, true);
        return true;
    }
    if key == Key::F4 || (control && char_key("l")) || (alt && char_key("d")) {
        let mut state = model.write();
        state.pending_focus = None;
        state.address_history =
            (key == Key::F4 && state.address_history != Some(pane)).then_some(pane);
        drop(state);
        event.prevent_default();
        event.stop_propagation();
        focus_address(pane);
        return true;
    }
    if model.read().address_history == Some(pane) {
        match key {
            Key::Escape => {
                event.prevent_default();
                event.stop_propagation();
                model.write().address_history = None;
                focus_address(pane);
                return true;
            }
            Key::ArrowDown | Key::ArrowUp if !control && !alt => {
                event.prevent_default();
                event.stop_propagation();
                let step = if key == Key::ArrowDown { 1 } else { -1 };
                let _ = document::eval(&format!(
                    "{{const b=[...document.querySelectorAll('[data-pane=\"{pane}\"] .address-history button')];const i=b.indexOf(document.activeElement);b[i<0?({step}>0?0:b.length-1):(i+b.length+{step})%b.length]?.focus();}}"
                ));
                return true;
            }
            _ => {}
        }
    }
    if (control && !event.modifiers().shift() && char_key("f")) || key == Key::F3 {
        event.prevent_default();
        event.stop_propagation();
        if !model.read().modal_open() {
            open_folder_search(model, bridge, pane);
        }
        return true;
    }
    if control && (char_key("f") || (char_key("e") && !event.modifiers().shift())) {
        event.prevent_default();
        event.stop_propagation();
        model.write().pending_focus = None;
        if control && event.modifiers().shift() {
            let mounted = model.read().search_open;
            {
                let mut s = model.write();
                s.search_open = true;
                s.search_focus = if mounted { None } else { Some(s.interaction) };
            }
            if mounted {
                let _ = document::eval("window.sdirFocusSearch?.()");
            }
        } else {
            let _ = document::eval(&format!(
                "document.getElementById('pane-query-{pane}')?.focus();document.getElementById('pane-query-{pane}')?.select();"
            ));
        }
        return true;
    }
    false
}
const PANE_VIEW_OPTIONS: [(&str, &str, &str); 8] = [
    ("icons-192", "view_icons-192", "아주 큰 아이콘"),
    ("icons-144", "view_icons-144", "큰 아이콘"),
    ("icons-112", "view_icons-112", "보통 아이콘"),
    ("icons-80", "view_icons-80", "작은 아이콘"),
    ("list", "view_list", "목록"),
    ("details", "view_details", "자세히"),
    ("tiles", "view_tiles", "타일"),
    ("content", "view_content", "내용"),
];

fn pane_view_option(view: ViewMode, size: u16) -> &'static str {
    if view == ViewMode::Icons {
        match size {
            192 => "icons-192",
            144 => "icons-144",
            112 => "icons-112",
            80 => "icons-80",
            _ => "icons",
        }
    } else {
        view.value()
    }
}

fn set_view(mut model: Signal<AppModel>, pane: usize, value: &str) {
    let mut s = model.write();
    s.interaction += 1;
    let compact = s.compact;
    let p = &mut s.panes[pane];
    p.view = match value {
        "list" => ViewMode::List,
        "tiles" => ViewMode::Tiles,
        "content" => ViewMode::Content,
        "icons" => ViewMode::Icons,
        "icons-192" | "icons-144" | "icons-112" | "icons-80" => {
            p.icon_size = value[6..].parse().expect("icon preset is numeric");
            ViewMode::Icons
        }
        _ => ViewMode::Details,
    };
    if matches!(p.view, ViewMode::Icons | ViewMode::Tiles) && p.tab().group_by != GroupBy::None {
        let tab = p.tab_mut();
        tab.group_by = GroupBy::None;
        tab.rebuild_visible();
    }
    let geometry = p.geometry(compact);
    let position = p.tab().cursor_position();
    p.tab_mut().scroll = position.map_or(0., |position| {
        geometry.row_top(p.tab().render_position(position))
    });
    s.active = pane;
    s.pending_focus = Some((pane, s.interaction));
    s.dirty();
}
fn fit_columns(mut model: Signal<AppModel>, pane: usize) {
    fn width(text: &str, padding: u16) -> u16 {
        text.chars().take(100).fold(padding, |width, ch| {
            width
                + if ch.is_ascii() {
                    if matches!(ch, 'M' | 'W' | '@') { 12 } else { 8 }
                } else {
                    14
                }
        })
    }
    let mut state = model.write();
    let pane = &mut state.panes[pane];
    if pane.view != ViewMode::Details {
        return;
    }
    let mut widths = [120, 65, 58, 145];
    let tab = pane.tab();
    for &index in tab.visible.iter() {
        let entry = &tab.entries[index];
        widths[0] = widths[0].max(width(&entry.name, 43));
        widths[1] = widths[1].max(width(&readable_size(entry.bytes), 23));
        if let Some(extension) = std::path::Path::new(&entry.name).extension() {
            widths[2] = widths[2].max(width(&extension.to_string_lossy(), 49));
        }
    }
    for (index, width) in widths.into_iter().enumerate() {
        pane.columns.resize(index, width);
    }
    state.dirty();
}

fn zoom_icons(model: Signal<AppModel>, pane: usize, larger: bool) {
    let presets = ["details", "icons-80", "icons-112", "icons-144", "icons-192"];
    let next = {
        let state = model.read();
        let pane = &state.panes[pane];
        let current = pane_view_option(pane.view, pane.icon_size);
        let index = presets
            .iter()
            .position(|value| *value == current)
            .unwrap_or(0);
        if larger {
            (index + 1).min(presets.len() - 1)
        } else {
            index.saturating_sub(1)
        }
    };
    set_view(model, pane, presets[next]);
}

#[derive(serde::Deserialize)]
struct SelectionRect {
    generation: u64,
    start: [f64; 2],
    end: [f64; 2],
    width: f64,
    ctrl: bool,
    shift: bool,
}

fn select_rectangle(mut model: Signal<AppModel>, pane: usize, value: &str) {
    let Ok(rect) = serde_json::from_str::<SelectionRect>(value) else {
        return;
    };
    let mut state = model.write();
    if state.modal_open()
        || !rect.start.iter().chain(&rect.end).all(|v| v.is_finite())
        || !rect.width.is_finite()
        || rect.width <= 0.
    {
        return;
    }
    let p = &state.panes[pane];
    if p.generation != rect.generation {
        return;
    }
    let geometry = p.geometry(state.compact);
    let tab = p.tab();
    let [x, y] = rect.start;
    let [end_x, end_y] = rect.end;
    if x.max(end_x) < 0. || x.min(end_x) >= rect.width {
        return;
    }
    let cell_width = rect.width / geometry.columns as f64;
    let col = |x: f64| ((x / cell_width).floor() as usize).min(geometry.columns - 1);
    let row_count = tab.render_len().div_ceil(geometry.columns);
    let row = |y: f64| ((y / geometry.row_height).floor() as usize).min(row_count);
    let first_col = col(x.min(end_x));
    let last_col = col(x.max(end_x));
    let tab = state.panes[pane].tab_mut();
    if !(rect.ctrl || rect.shift) {
        tab.clear_selection();
    }
    let mut last = None;
    for display_row in row(y.min(end_y))..=row(y.max(end_y)) {
        for column in first_col..=last_col {
            let Ok(position) = tab.item_at_render_row(display_row * geometry.columns + column)
            else {
                continue;
            };
            let Some(&entry) = tab.visible.get(position) else {
                continue;
            };
            let path = &tab.entries[entry].path;
            if !rect.ctrl || !tab.selection.remove(path) {
                tab.selection.insert(path.clone());
            }
            last = Some(position);
        }
    }
    if let Some(position) = last {
        tab.move_cursor(position);
        tab.anchor = tab.cursor.clone();
    }
    state.active = pane;
    state.interaction += 1;
    state.pending_focus = None;
    state.dirty();
    drop(state);
    focus_list(pane);
}

fn invert_selection(mut model: Signal<AppModel>, pane: usize) {
    model.write().panes[pane].tab_mut().invert_selection();
    model.write().active = pane;
    focus_list(pane);
}
fn scroll_selection(mut model: Signal<AppModel>, pane: usize, position: usize) {
    let scroll = {
        let mut s = model.write();
        let compact = s.compact;
        let p = &mut s.panes[pane];
        let scroll = p
            .geometry(compact)
            .scroll_to(p.tab().render_position(position), p.tab().scroll);
        p.tab_mut().scroll = scroll;
        scroll
    };
    let _ = document::eval(&format!(
        "window.__sdirScrollSelection?.({pane},{position},{scroll});"
    ));
}

fn list_key(event: KeyboardEvent, mut model: Signal<AppModel>, bridge: &Bridge, pane: usize) {
    event.stop_propagation();
    if model.read().modal_open() {
        if event.key() == Key::Escape {
            dismiss_modal(model, bridge);
        }
        return;
    }
    model.write().interaction += 1;
    if event.is_composing() {
        return;
    }
    model.write().active = pane;
    if event.key() == Key::ContextMenu || (event.key() == Key::F10 && event.modifiers().shift()) {
        event.prevent_default();
        event.stop_propagation();
        open_selected_context(model, bridge, pane);
        return;
    }
    if pane_shortcut(&event, model, pane, bridge) {
        return;
    }
    let ctrl = command_modifier(event.modifiers());
    let shift = event.modifiers().shift();
    let key = event.key();
    let cursor_key = matches!(
        key,
        Key::ArrowDown
            | Key::ArrowUp
            | Key::ArrowLeft
            | Key::ArrowRight
            | Key::Home
            | Key::End
            | Key::PageDown
            | Key::PageUp
    );
    let space = matches!(&key, Key::Character(text) if text == " ");
    if cfg!(target_os = "macos") && ctrl && event.modifiers().alt() && event.code() == Code::KeyV {
        event.prevent_default();
        paste_with_intent(model, bridge, true);
        return;
    }
    if ctrl && !cursor_key && !space {
        if let Key::Character(ref k) = key {
            match k.to_lowercase().as_str() {
                "a" => {
                    event.prevent_default();
                    let mut s = model.write();
                    let t = s.panes[pane].tab_mut();
                    t.selection = t
                        .visible
                        .iter()
                        .map(|&i| t.entries[i].path.clone())
                        .collect();
                }
                "d" if cfg!(windows) => {
                    event.prevent_default();
                    dialog(model, "recycle");
                }
                "z" => {
                    event.prevent_default();
                    history_action(model, bridge, event.modifiers().shift());
                }
                "y" => {
                    event.prevent_default();
                    history_action(model, bridge, true);
                }
                "i" => {
                    event.prevent_default();
                    invert_selection(model, pane);
                }
                "c" => {
                    event.prevent_default();
                    if shift {
                        let paths = model.read().selected();
                        if !paths.is_empty() {
                            if let Err(error) =
                                bridge.utility(sdir::utility::Utility::CopyPaths(paths))
                            {
                                model.write().notice = error;
                            }
                        }
                    } else {
                        copy(model, bridge, false);
                    }
                }
                "x" => {
                    event.prevent_default();
                    copy(model, bridge, true);
                }
                "v" => {
                    event.prevent_default();
                    paste(model, bridge);
                }
                _ => {}
            }
        }
        return;
    }
    let mac_open = cfg!(target_os = "macos") && ctrl && key == Key::ArrowDown;
    let mac_trash = cfg!(target_os = "macos") && ctrl && key == Key::Backspace;
    if event.modifiers().alt()
        || (event.modifiers().meta()
            && !mac_open
            && !mac_trash
            && !(cfg!(target_os = "macos") && (cursor_key || space)))
    {
        return;
    }
    let position = match &key {
        Key::Character(_) if space => {
            event.prevent_default();
            let mut state = model.write();
            let tab = state.panes[pane].tab_mut();
            tab.toggle_position(tab.cursor_position().unwrap_or(0))
        }
        Key::Character(text) => {
            event.prevent_default();
            model.write().panes[pane]
                .tab_mut()
                .type_to_select(text, Instant::now())
        }
        Key::ArrowDown
        | Key::ArrowUp
        | Key::ArrowLeft
        | Key::ArrowRight
        | Key::Home
        | Key::End
        | Key::PageDown
        | Key::PageUp => {
            event.prevent_default();
            let mut s = model.write();
            let compact = s.compact;
            let p = &s.panes[pane];
            let geometry = p.geometry(compact);
            let page = ((geometry.viewport_height / geometry.row_height).floor() as usize).max(1)
                * geometry.columns;
            if mac_open {
                // Finder opens the selected item with Command–Down.
                None
            } else {
                let tab = s.panes[pane].tab_mut();
                let next = match key {
                    Key::Home => 0,
                    Key::End => tab.visible.len().saturating_sub(1),
                    Key::ArrowDown => tab.cursor_position().map_or(0, |i| {
                        (i + geometry.columns).min(tab.visible.len().saturating_sub(1))
                    }),
                    Key::ArrowUp => tab
                        .cursor_position()
                        .map_or(0, |i| i.saturating_sub(geometry.columns)),
                    Key::ArrowRight => tab
                        .cursor_position()
                        .map_or(0, |i| (i + 1).min(tab.visible.len().saturating_sub(1))),
                    Key::ArrowLeft => tab.cursor_position().map_or(0, |i| i.saturating_sub(1)),
                    Key::PageDown => tab
                        .cursor_position()
                        .unwrap_or(0)
                        .saturating_add(page)
                        .min(tab.visible.len().saturating_sub(1)),
                    _ => tab.cursor_position().unwrap_or(0).saturating_sub(page),
                };
                if ctrl && !shift {
                    tab.move_cursor(next)
                } else {
                    tab.select_position(next, shift)
                }
            }
        }
        _ => None,
    };
    if let Some(position) = position {
        scroll_selection(model, pane, position);
        focus_list(pane);
        model.write().dirty();
        return;
    }
    match key {
        Key::Enter if cfg!(target_os = "macos") && !ctrl => {
            event.prevent_default();
            start_inline_rename(model);
        }
        Key::Enter | Key::ArrowDown if key == Key::Enter || mac_open => {
            event.prevent_default();
            let selected = model.read().selected();
            if let Some(path) = selected.first().cloned() {
                let is_dir = model.read().panes[pane]
                    .tab()
                    .entries
                    .iter()
                    .any(|e| e.path == path && e.is_dir);
                if is_dir {
                    navigate(model, bridge, pane, path);
                } else {
                    let _ = bridge.job(Request::Open { path });
                }
            }
        }
        Key::Delete => {
            event.prevent_default();
            if cfg!(windows) && !shift {
                let sources = model.read().selected();
                if sources.is_empty() {
                    model.write().notice = "작업할 항목을 선택하세요.".into();
                } else {
                    submit(model, bridge, FileAction::Recycle { sources });
                }
            } else {
                dialog(model, if shift { "delete" } else { "recycle" });
            }
        }
        Key::F2 => {
            event.prevent_default();
            if model.read().panes[pane].tab().selection.len() > 1 {
                open_numbered_batch(model);
            } else {
                start_inline_rename(model);
            }
        }
        Key::Backspace if mac_trash => {
            event.prevent_default();
            dialog(model, "recycle");
        }
        Key::Backspace if cfg!(windows) => history_navigation(model, bridge, pane, false),
        _ => {}
    }
}

fn app() -> Element {
    let independent = try_consume_context::<IndependentWindow>();
    let primary = independent.is_none();
    use_drop(move || {
        if primary {
            lifecycle::release_activation();
        }
    });
    let mut model = use_signal(|| {
        let mut settings = settings::load();
        if let Some(IndependentWindow(path)) = independent {
            settings.layout = 1;
            settings.pane_arrangement = PaneArrangement::Standard;
            settings.active_pane = 0;
            settings.active_tabs.clear();
            settings.tab_states.clear();
            settings.panes = vec![vec![path]];
            settings.close_to_tray = false;
            settings.start_on_login = false;
            settings.resume_live_index = false;
            settings.window = None;
        }
        AppModel::new(settings)
    });
    use_hook(|| {
        let _ = document::eval(include_str!("editing.js"));
        let _ = document::eval(include_str!("search_focus.js"));
        let _ = document::eval(include_str!("sidebar_resize.js"));
        let _ = document::eval(include_str!("mouse.js"));
        let _ = document::eval(include_str!("scroll.js"));
    });
    let locations = use_hook(platform::navigation_locations);
    let bridge = use_hook(|| Arc::new(Bridge::new()));
    let settings_writer = use_hook(|| Arc::new(settings::SettingsWriter::new()));
    use_context_provider(|| settings_writer.clone());
    let close_window = dioxus::desktop::use_window();
    let close_writer = settings_writer.clone();
    let close_bridge = bridge.clone();
    let window_for_activation = close_window.clone();
    #[cfg(windows)]
    let tray_for_close = {
        let tray = use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(None)));
        let tray_for_close = tray.clone();
        use_effect(move || {
            if primary && model.read().close_to_tray {
                if tray.borrow().is_none() {
                    match lifecycle::make_tray() {
                        Ok(icon) => *tray.borrow_mut() = Some(icon),
                        Err(error) => {
                            let mut state = model.write();
                            state.close_to_tray = false;
                            state.notice = format!("트레이 아이콘을 만들지 못했습니다. {error}");
                            state.save_due = Some(Instant::now());
                        }
                    }
                }
            } else {
                tray.borrow_mut().take();
            }
        });
        let tray_window = close_window.clone();
        let tray_writer = settings_writer.clone();
        let tray_bridge = bridge.clone();
        // Native menu items share the menubar event receiver in the pinned desktop runtime.
        dioxus::desktop::use_muda_event_handler(move |event| match event.id.as_ref() {
            lifecycle::OPEN_ID if primary => lifecycle::show_window(&tray_window),
            lifecycle::EXIT_ID if primary => {
                session_close::save_then_close(model, &tray_writer, &tray_window, &tray_bridge)
            }
            _ => {}
        });
        tray_for_close
    };
    let placement_window = close_window.clone();
    use_hook(move || {
        let saved = model.read().window.clone();
        let placement = if let Some(saved) = saved.as_ref() {
            Some(lifecycle::restore_placement(&placement_window, saved))
        } else {
            lifecycle::observe_placement(&placement_window, None)
        };
        if let Some(placement) = placement {
            model.write().window = Some(placement);
        }
    });
    let placement_dirty = use_hook(|| std::rc::Rc::new(std::cell::Cell::new(false)));
    let placement_changed = placement_dirty.clone();

    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::{WindowEvent, tao::event::Event};
        #[cfg(windows)]
        if let Event::WindowEvent {
            window_id,
            event: WindowEvent::Focused(_) | WindowEvent::Resized(_),
            ..
        } = event
        {
            if *window_id == close_window.id() {
                use dioxus::desktop::wry::{MemoryUsageLevel, WebViewExtWindows};
                let minimized = close_window.is_minimized();
                if minimized != model.read().memory_saving {
                    let level = if minimized {
                        MemoryUsageLevel::Low
                    } else {
                        MemoryUsageLevel::Normal
                    };
                    if close_window.webview.set_memory_usage_level(level).is_ok() {
                        model.write().memory_saving = minimized;
                    }
                }
            }
        }
        if let Event::WindowEvent {
            window_id,
            event: WindowEvent::Moved(_) | WindowEvent::Resized(_),
            ..
        } = event
        {
            if *window_id == close_window.id() && !model.read().closing {
                placement_changed.set(true);
            }
        }
        if let Event::WindowEvent {
            window_id,
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            if *window_id != close_window.id() || !primary {
                return;
            }
            #[cfg(windows)]
            if model.read().close_to_tray
                && tray_for_close.borrow().is_some()
                && !model.read().closing
            {
                close_window.set_close_behavior(dioxus::desktop::WindowCloseBehaviour::WindowHides);
                return;
            }
            session_close::save_then_close(model, &close_writer, &close_window, &close_bridge);
        }
    });
    use_context_provider(|| bridge.clone());
    let poll = bridge.clone();
    use_future(move || {
        let bridge = poll.clone();
        let settings_writer = settings_writer.clone();
        let window = window_for_activation.clone();
        let placement_dirty = placement_dirty.clone();
        async move {
            let layout = model.read().layout;
            for p in 0..layout {
                load(model, &bridge, p);
            }
            #[cfg(target_os = "macos")]
            start_spotlight(model, &bridge);
            let resume = {
                let state = model.read();
                state
                    .resume_live_index
                    .then_some(state.search_preferences.memory_mib)
            };
            if cfg!(windows)
                && let Some(memory) = resume
            {
                start_volume_index(model, &bridge, memory, true);
            }
            let mut next_thumbnail_check = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(40)).await;
                if placement_dirty.replace(false) && !model.read().closing {
                    let placement =
                        lifecycle::observe_placement(&window, model.read().window.as_ref());
                    if let Some(placement) = placement {
                        let mut state = model.write();
                        state.window = Some(placement);
                        state.dirty();
                    }
                }
                if primary && lifecycle::activation_pending() && !model.read().closing {
                    lifecycle::show_window(&window);
                }
                let events: Vec<_> = bridge.events.try_iter().take(128).collect();
                for event in events {
                    match event {
                        WorkerEvent::Utility { action, result } => {
                            if matches!(&action, sdir::utility::Utility::FolderMenu { .. }) {
                                finish_context(model, &action, result);
                                continue;
                            }
                            let mut s = model.write();
                            #[cfg(windows)]
                            if let sdir::utility::Utility::Drag { id, .. } = &action {
                                if s.external_drag
                                    .as_ref()
                                    .is_some_and(|(current, _)| current == id)
                                {
                                    s.external_drag = None;
                                }
                            }
                            s.notice = match result {
                                Ok(pid) => match action {
                                    sdir::utility::Utility::Drag { .. } => {
                                        if pid == Some(sdir::utility::SDIR_DRAG_CANCELLED) {
                                            "외부 드래그를 취소했습니다.".into()
                                        } else if pid
                                            == Some(sdir::utility::SDIR_DRAG_VERIFIED_MOVE)
                                        {
                                            "외부 대상의 이동 효과와 원본 경로 제거를 확인했습니다."
                                                .into()
                                        } else if pid
                                            == Some(sdir::utility::SDIR_DRAG_EXTERNAL_HANDOFF)
                                        {
                                            "외부 파일 대상에 전달했습니다. 이동 완료는 확인되지 않았으니 결과를 대상에서 확인하세요.".into()
                                        } else {
                                            "외부 드래그 결과를 확인할 수 없습니다. 대상에서 파일 상태를 확인하세요.".into()
                                        }
                                    }
                                    sdir::utility::Utility::OpenWith(_, _) if pid.is_none() => {
                                        "연결 프로그램 선택을 취소했습니다.".into()
                                    }
                                    sdir::utility::Utility::CreateShortcuts { .. } => {
                                        "선택한 폴더에 바로 가기를 만들었습니다.".into()
                                    }
                                    sdir::utility::Utility::NetworkDrive { remote, .. } => {
                                        s.tree.update_roots(platform::navigation_drives());
                                        s.tree_seen = None;
                                        if remote.is_some() {
                                            "네트워크 드라이브를 연결했습니다.".into()
                                        } else {
                                            "네트워크 드라이브 연결을 끊었습니다.".into()
                                        }
                                    }
                                    sdir::utility::Utility::CopyPaths(_) => {
                                        "전체 경로를 클립보드에 복사했습니다.".into()
                                    }
                                    _ => {
                                        s.utility_pid = pid;
                                        "요청한 외부 작업을 실행했습니다.".into()
                                    }
                                },
                                Err(e) => format!("실행하지 못했습니다: {e}"),
                            };
                        }
                        WorkerEvent::WatchReady { pane, id } => {
                            let mut s = model.write();
                            if let Some(w) = s.watches[pane].as_mut() {
                                if w.id == id {
                                    w.ready = true;
                                }
                            }
                        }
                        WorkerEvent::Changed { pane, id, renames } => {
                            let mut s = model.write();
                            if pane < s.layout
                                && s.watches[pane].as_ref().is_some_and(|w| {
                                    w.id == id && w.path == s.panes[pane].tab().path
                                })
                            {
                                s.watch_due[pane]
                                    .get_or_insert(Instant::now() + Duration::from_millis(200));
                                let space = 256usize.saturating_sub(s.watch_renames[pane].len());
                                s.watch_renames[pane].extend(renames.into_iter().take(space));
                            }
                        }
                        WorkerEvent::WatchError { pane, id, message } => {
                            let mut s = model.write();
                            if s.watches[pane].as_ref().is_some_and(|w| w.id == id) {
                                s.watches[pane] = None;
                                s.watch_errors[pane] =
                                    Some(format!("자동 갱신 중단: {message} · F5로 다시 연결"));
                                s.watch_due[pane] = None;
                            }
                        }
                        WorkerEvent::Tree { id, result } => {
                            model.write().tree.finish(id, result);
                        }
                        WorkerEvent::ClipboardWritten {
                            id,
                            cut,
                            result,
                            sequence,
                        } => {
                            if model
                                .read()
                                .clipboard_pending
                                .as_ref()
                                .is_some_and(|pending| pending.0 == id)
                            {
                                let mut s = model.write();
                                let (_, paths, _) = s.clipboard_pending.take().unwrap();
                                if result.is_ok() {
                                    s.clipboard_marked = Arc::new(paths.into_iter().collect());
                                    s.clipboard_cut = cut;
                                    s.clipboard_sequence = Some(sequence);
                                } else {
                                    Arc::make_mut(&mut s.clipboard_marked).clear();
                                    s.clipboard_cut = false;
                                }
                                s.notice = match result {
                                    Ok(()) if cut => "잘라내기: 붙여넣을 폴더를 선택하세요.".into(),
                                    Ok(()) => {
                                        "복사했습니다. 다른 패널이나 탐색기에 붙여넣을 수 있습니다."
                                            .into()
                                    }
                                    Err(e) => format!("클립보드에 기록하지 못했습니다: {e}"),
                                };
                            }
                        }
                        WorkerEvent::Batch { pane, id, entries } => {
                            let mut s = model.write();
                            if s.panes[pane].generation != id
                                && !s.refreshes[pane].as_ref().is_some_and(|r| r.id == id)
                            {
                                continue;
                            }
                            let parent = s.panes[pane].tab().path.to_path();
                            let entries = entries
                                .into_iter()
                                .map(|e| e.expand(&parent))
                                .collect::<Vec<_>>();
                            if let Some(r) = s.refreshes[pane].as_mut().filter(|r| r.id == id) {
                                r.entries.extend(entries);
                            } else {
                                s.panes[pane].accept_batch(id, entries);
                            }
                        }
                        WorkerEvent::Listed { pane, id, error } => {
                            let background = finish_background(model, pane, id, error.clone());
                            let listed = error.is_none();
                            let mut reveal_in_place = None;
                            let mut s = model.write();
                            if background || s.panes[pane].finish(id, error) {
                                if !background && listed {
                                    s.remember_folder(pane);
                                }
                                if s.reveal
                                    .get(&pane)
                                    .is_some_and(|request| request.generation == id)
                                {
                                    let request = s.reveal.remove(&pane).unwrap();
                                    if background && !listed {
                                        continue;
                                    }
                                    let allowed = request.interaction == s.interaction;
                                    if request.generation == id && allowed {
                                        let compact = s.compact;
                                        let p = &mut s.panes[pane];
                                        let geometry = p.geometry(compact);
                                        let old_scroll = p.tab().scroll;
                                        if let Some(position) =
                                            p.tab_mut().select_results(&request.paths)
                                        {
                                            p.tab_mut().scroll =
                                                request.preserve_scroll.unwrap_or_else(|| {
                                                    geometry.scroll_to(
                                                        p.tab().render_position(position),
                                                        old_scroll,
                                                    )
                                                });
                                            if background && request.focus {
                                                let selected = p.tab().cursor.as_ref().unwrap();
                                                let path =
                                                    serde_json::to_string(&selected.display())
                                                        .expect(
                                                            "a file display path is serializable",
                                                        );
                                                reveal_in_place = Some((
                                                    position,
                                                    p.tab().scroll,
                                                    p.generation,
                                                    path,
                                                ));
                                            } else {
                                                p.reveal_revision += 1;
                                            }
                                            if request.focus {
                                                s.active = pane;
                                                s.pending_focus = Some((pane, s.interaction));
                                                s.dirty();
                                            }
                                        } else if request.from_search && listed {
                                            s.search_error = Some(
                                                "검색 결과를 폴더 목록에서 찾을 수 없습니다. 검색을 다시 실행하세요."
                                                    .into(),
                                            );
                                        }
                                    }
                                }
                            }
                            drop(s);
                            if let Some((position, scroll, generation, path)) = reveal_in_place {
                                let _ = document::eval(&format!(
                                    "window.__sdirScrollSelection?.({pane},{position},{scroll},{{generation:'{generation}',path:{path}}});"
                                ));
                            }
                        }
                        WorkerEvent::FileAppearances { id, items } => {
                            finish_appearances(model, id, items)
                        }
                        WorkerEvent::Thumbnails { id, images } => {
                            finish_thumbnails(model, id, images)
                        }
                        WorkerEvent::ReadPreview { id, result } => {
                            finish_preview(model, id, result)
                        }
                        WorkerEvent::Index { index_id, status } => {
                            if scoped_index_event(model, index_id, &status) {
                                continue;
                            }
                            if index_id == model.read().index_id {
                                let mut s = model.write();
                                let completed = status.complete;
                                let live = status.live;
                                let query_needed = !status.live || s.index.is_none();
                                s.index = Some(status);
                                s.index_starting = false;
                                s.index_error = None;
                                if (completed || live)
                                    && query_needed
                                    && s.query_requested
                                    && !s.query.is_empty()
                                {
                                    s.query_due = Some(Instant::now());
                                }
                            }
                        }
                        WorkerEvent::Search { id, result } => {
                            if scoped_query_event(model, id, &result) {
                                continue;
                            }
                            if id == model.read().query_id {
                                let mut s = model.write();
                                s.query_loading = false;
                                match result {
                                    Ok(r) => {
                                        s.result = Some(r);
                                        s.search_error = None;
                                    }
                                    Err(e) => s.search_error = Some(e),
                                }
                            }
                        }
                        WorkerEvent::VolumeIndex {
                            session,
                            generation,
                            host_pid,
                            status,
                        } => {
                            let current = model.read();
                            let matches = session == current.index_id
                                && generation == current.config_generation
                                && current.config_pending.is_none()
                                && status.host_pid == Some(host_pid)
                                && current.host_pid.is_none_or(|pid| pid == host_pid);
                            drop(current);
                            if matches {
                                let mut s = model.write();
                                let completed = status.complete;
                                let live = status.live;
                                let query_needed = !status.live || s.index.is_none();
                                let rebuild_outcome = s.pending_rebuild.as_ref().and_then(
                                    |(
                                        pending_session,
                                        pending_generation,
                                        _,
                                        guid,
                                        expected_pid,
                                        accepted,
                                    )| {
                                        if !accepted
                                            || *pending_session != session
                                            || *pending_generation != generation
                                            || *expected_pid != Some(host_pid)
                                        {
                                            return None;
                                        }
                                        status
                                            .volumes
                                            .iter()
                                            .find(|volume| volume.guid.as_ref() == Some(guid))
                                            .and_then(|volume| match &volume.phase {
                                                sdir::live_search::Phase::Live
                                                | sdir::live_search::Phase::Snapshot => {
                                                    Some((true, None))
                                                }
                                                sdir::live_search::Phase::Offline
                                                | sdir::live_search::Phase::Excluded
                                                | sdir::live_search::Phase::Error
                                                | sdir::live_search::Phase::Unsupported
                                                | sdir::live_search::Phase::Unavailable
                                                | sdir::live_search::Phase::Limited => {
                                                    Some((false, volume.message.clone()))
                                                }
                                                sdir::live_search::Phase::Indexing
                                                | sdir::live_search::Phase::CatchingUp => None,
                                            })
                                    },
                                );
                                s.host_pid = Some(host_pid);
                                s.index = Some(*status);
                                s.index_starting = false;
                                s.index_error = None;
                                if let Some((success, message)) = rebuild_outcome {
                                    s.pending_rebuild = None;
                                    s.query_loading = false;
                                    if success {
                                        s.search_error = None;
                                        s.index_error = None;
                                        if s.query_requested && !s.query.is_empty() {
                                            s.query_due = Some(Instant::now());
                                        }
                                    } else {
                                        s.result = None;
                                        s.index_error = Some(message.unwrap_or_else(|| {
                                            "드라이브 색인을 다시 만들지 못했습니다.".into()
                                        }));
                                    }
                                } else if s.pending_rebuild.is_none()
                                    && s.config_pending.is_none()
                                    && (completed || live)
                                    && query_needed
                                    && s.query_requested
                                    && !s.query.is_empty()
                                {
                                    s.query_due = Some(Instant::now());
                                }
                            }
                        }
                        WorkerEvent::VolumeSearch {
                            session,
                            generation,
                            query_id,
                            host_pid,
                            result,
                        } => {
                            let current = model.read();
                            let pid_matches = match host_pid {
                                Some(pid) => current.host_pid == Some(pid),
                                None => result.is_err(),
                            };
                            let matches = session == current.index_id
                                && generation == current.config_generation
                                && query_id == current.query_id
                                && current.config_pending.is_none()
                                && current.pending_rebuild.is_none()
                                && pid_matches;
                            drop(current);
                            if matches {
                                let mut s = model.write();
                                s.query_loading = false;
                                match result {
                                    Ok(result) => {
                                        s.result = Some(*result);
                                        s.search_error = None;
                                    }
                                    Err(error) => {
                                        s.result = None;
                                        s.search_error = Some(error);
                                    }
                                }
                            }
                        }
                        WorkerEvent::ConfigAck {
                            session,
                            generation,
                            host_pid,
                            accepted,
                            message,
                        } => {
                            let mut s = model.write();
                            let pending_matches = s.config_pending.as_ref().is_some_and(
                                |(pending_session, pending_generation, expected_pid, _)| {
                                    *pending_session == session
                                        && *pending_generation == generation
                                        && if let Some(pid) = host_pid {
                                            expected_pid.is_none_or(|expected| expected == pid)
                                        } else {
                                            !accepted
                                        }
                                },
                            );
                            let startup = generation == 0
                                && session == s.index_id
                                && s.search_live
                                && s.index_starting
                                && s.config_pending.is_none()
                                && s.host_pid.is_none();
                            if startup && accepted && host_pid.is_some() {
                                s.resume_live_index = true;
                                s.save_due = Some(Instant::now());
                            }
                            let startup_rejected = !accepted
                                && generation == 0
                                && session == s.index_id
                                && s.search_live
                                && s.index_starting
                                && s.config_pending.is_none()
                                && s.config_generation == 0
                                && s.host_pid.is_none();
                            if startup_rejected
                                || (pending_matches && (host_pid.is_some() || !accepted))
                            {
                                if accepted {
                                    if let (Some(pid), Some((_, _, _, settings))) =
                                        (host_pid, s.config_pending.take())
                                    {
                                        s.ntfs_indexing = settings;
                                        s.config_generation = generation;
                                        s.host_pid = Some(pid);
                                        s.index = None;
                                        s.index_starting = true;
                                        s.search_error = None;
                                        s.index_error = None;
                                        s.save_due = Some(Instant::now());
                                        // The first status in this generation resumes the requested query.
                                        s.query_due = None;
                                    }
                                } else {
                                    s.config_pending = None;
                                    s.query_due = None;
                                    s.query_loading = false;
                                    s.result = None;
                                    s.index_starting = false;
                                    s.index_error = Some(message.unwrap_or_else(|| {
                                        "검색 설정을 적용하지 못했습니다.".into()
                                    }));
                                }
                            }
                        }
                        WorkerEvent::RebuildAck {
                            session,
                            generation,
                            request_id,
                            guid,
                            host_pid,
                            accepted,
                            message,
                        } => {
                            let mut s = model.write();
                            let pending_matches = s.pending_rebuild.as_ref().is_some_and(
                                |(
                                    pending_session,
                                    pending_generation,
                                    pending_id,
                                    pending_guid,
                                    expected_pid,
                                    _,
                                )| {
                                    *pending_session == session
                                        && *pending_generation == generation
                                        && *pending_id == request_id
                                        && *pending_guid == guid
                                        && if let Some(pid) = host_pid {
                                            expected_pid.is_none_or(|expected| expected == pid)
                                        } else {
                                            !accepted
                                        }
                                },
                            );
                            if pending_matches && (host_pid.is_some() || !accepted) {
                                if accepted {
                                    if let Some(pending) = s.pending_rebuild.as_mut() {
                                        pending.5 = true;
                                    }
                                    s.index = None;
                                    s.index_starting = true;
                                    s.search_error = None;
                                    s.index_error = None;
                                } else {
                                    s.pending_rebuild = None;
                                    s.query_loading = false;
                                    s.query_due = None;
                                    s.result = None;
                                    s.index_error = Some(message.unwrap_or_else(|| {
                                        "드라이브 색인을 다시 만들지 못했습니다.".into()
                                    }));
                                }
                            }
                        }

                        WorkerEvent::Preview { id, result } => {
                            let mut s = model.write();
                            if let Some(b) = s.batch.as_mut() {
                                if b.id == id {
                                    b.result = Some(result);
                                }
                            }
                        }
                        WorkerEvent::JobProgress { id, percent } => {
                            let mut s = model.write();
                            if s.job == Some(id) && (1..=99).contains(&percent) {
                                if let Some(status) = s.job_status.as_mut() {
                                    if status.id == id {
                                        status.percent = Some(percent);
                                    }
                                }
                            }
                        }
                        WorkerEvent::Job { id, result } => {
                            if model.read().job == Some(id) {
                                let (context, keep_failed_rename) = {
                                    let mut s = model.write();
                                    let mut keep_failed_rename = false;
                                    s.job = None;
                                    s.job_status = None;
                                    s.notice = result.message.clone();
                                    if let Some(edit) = s
                                        .inline_rename
                                        .as_mut()
                                        .filter(|edit| edit.submitted_job == Some(id))
                                    {
                                        if result.ok {
                                            let editor = edit.id;
                                            let _ = document::eval(&format!(
                                                "{{const e=document.getElementById('inline-rename-{editor}');if(e&&document.activeElement===e)e.closest('.list-scroll')?.focus({{preventScroll:true}});}}"
                                            ));
                                            s.inline_rename = None;
                                        } else {
                                            edit.submitted_job = None;
                                            keep_failed_rename = result.destinations.is_empty();
                                        }
                                    }
                                    if let Some(c) = &s.job_context {
                                        let entry =
                                            sdir::journal::Entry::new(c.action.clone(), &result);
                                        s.journal.push(entry);
                                        if s.journal.len() > 32 {
                                            s.journal.remove(0);
                                        }
                                    }
                                    s.index_dirty = s.index.is_some() && !s.spotlight;
                                    (s.job_context.take(), keep_failed_rename)
                                };
                                if let Some(context) = context {
                                    {
                                        let mut s = model.write();
                                        match context.history {
                                            Some(redo) if result.ok => {
                                                if redo {
                                                    s.redo.pop();
                                                } else {
                                                    s.undo.pop();
                                                }
                                                if let Some(record) = result.undo.clone() {
                                                    let stack = if redo {
                                                        &mut s.undo
                                                    } else {
                                                        &mut s.redo
                                                    };
                                                    stack.push(record);
                                                    if stack.len() > 16 {
                                                        stack.remove(0);
                                                    }
                                                } else {
                                                    s.undo.clear();
                                                    s.redo.clear();
                                                }
                                            }
                                            Some(_) => {
                                                if result.undo.is_some()
                                                    || !result.destinations.is_empty()
                                                {
                                                    s.undo.clear();
                                                    s.redo.clear();
                                                }
                                            }
                                            None => {
                                                if let Some(record) = result.undo.clone() {
                                                    s.undo.push(record);
                                                    if s.undo.len() > 16 {
                                                        s.undo.remove(0);
                                                    }
                                                    s.redo.clear();
                                                } else if result.ok || result.history_note.is_some()
                                                {
                                                    s.undo.clear();
                                                    s.redo.clear();
                                                }
                                            }
                                        }
                                        if let Some(note) = &result.history_note {
                                            s.notice
                                                .push_str(&format!(" 실행 취소 기록 제외: {note}"));
                                        }
                                        if context.history.is_some()
                                            && result.ok
                                            && context.interaction == s.interaction
                                            && context.origin.matches(&s)
                                        {
                                            s.pending_focus = Some((s.active, s.interaction));
                                        }
                                    }
                                    let target = {
                                        let mut s = model.write();
                                        if result.ok && s.clipboard_cut {
                                            for path in &context.cut_sources {
                                                Arc::make_mut(&mut s.clipboard_marked).remove(path);
                                            }
                                        }
                                        context.reveal.filter(|c| {
                                            c.interaction == s.interaction
                                                && c.origin.matches(&s)
                                                && c.target.matches(&s)
                                                && !result.destinations.is_empty()
                                        })
                                    };
                                    let refresh: Vec<_> = {
                                        let s = model.read();
                                        (0..s.layout)
                                            .filter(|&p| {
                                                !keep_failed_rename
                                                    && context
                                                        .folders
                                                        .contains(&s.panes[p].tab().path)
                                            })
                                            .collect()
                                    };
                                    for pane in refresh {
                                        let (background, selection, scroll) = {
                                            let s = model.read();
                                            let p = &s.panes[pane];
                                            let tab = p.tab();
                                            let background = tab.loaded
                                                && tab.resume.is_none()
                                                && !s.reveal.contains_key(&pane)
                                                && (!p.loading || s.refreshes[pane].is_some());
                                            let selection = if background {
                                                vec![]
                                            } else {
                                                tab.selection.iter().cloned().collect()
                                            };
                                            (background, selection, tab.scroll)
                                        };
                                        let generation = if background {
                                            start_background(model, &bridge, pane)
                                        } else {
                                            load(model, &bridge, pane);
                                            model.read().panes[pane].generation
                                        };
                                        let mut s = model.write();
                                        let reveal =
                                            target.as_ref().filter(|c| c.target.pane == pane);
                                        let paths = if reveal.is_some() {
                                            result.destinations.clone()
                                        } else {
                                            selection
                                        };
                                        if !paths.is_empty() {
                                            let interaction = s.interaction;
                                            s.reveal.insert(
                                                pane,
                                                PendingReveal {
                                                    generation,
                                                    paths,
                                                    interaction: reveal
                                                        .map_or(interaction, |c| c.interaction),
                                                    focus: reveal.is_some(),
                                                    from_search: false,
                                                    preserve_scroll: if reveal.is_some() {
                                                        None
                                                    } else {
                                                        Some(scroll)
                                                    },
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        WorkerEvent::Clipboard {
                            id,
                            result,
                            sequence,
                        } => {
                            let context = model.write().pastes.remove(&id);
                            if let Some((context, move_files)) = context {
                                match result {
                                    Ok((sources, cut)) if !sources.is_empty() => {
                                        let cut = {
                                            let mut s = model.write();
                                            let cut = move_files
                                                || (cut
                                                    && (cfg!(windows)
                                                        || (s.clipboard_cut
                                                            && s.clipboard_sequence
                                                                == Some(sequence))));
                                            s.clipboard_marked =
                                                Arc::new(sources.iter().cloned().collect());
                                            s.clipboard_cut = cut;
                                            s.clipboard_sequence = Some(sequence);
                                            cut
                                        };
                                        let destination = context.target.path.clone();
                                        let action = if cut {
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
                                        submit_context(model, &bridge, action, Some(context));
                                    }
                                    Ok(_) => {
                                        model.write().notice = "클립보드에 파일이 없습니다.".into()
                                    }
                                    Err(e) => model.write().notice = e,
                                }
                            }
                        }
                        WorkerEvent::Notice { message } => model.write().notice = message,
                        WorkerEvent::Opened { path } => model.write().remember_open(path),
                        WorkerEvent::Failure { lane, id, message } => {
                            if scoped_failure(model, id, &message) {
                                continue;
                            }
                            if lane == 14 {
                                finish_appearances(model, id, Vec::new());
                                continue;
                            }
                            if lane == 13 {
                                finish_thumbnails(model, id, Vec::new());
                                continue;
                            }
                            if lane == 11 {
                                finish_preview(model, id, Err(message));
                                continue;
                            }
                            if lane < 4 {
                                let background =
                                    finish_background(model, lane, id, Some(message.clone()));
                                let mut state = model.write();
                                if !background {
                                    state.panes[lane].finish(id, Some(message));
                                }
                                if state
                                    .reveal
                                    .get(&lane)
                                    .is_some_and(|request| request.generation == id)
                                {
                                    state.reveal.remove(&lane);
                                }
                            } else if (6..10).contains(&lane) {
                                let pane = lane - 6;
                                let mut s = model.write();
                                if s.watches[pane].as_ref().is_some_and(|w| w.id == id) {
                                    s.watches[pane] = None;
                                    s.watch_errors[pane] =
                                        Some(format!("자동 갱신 중단: {message} · F5로 다시 연결"));
                                    s.watch_due[pane] = None;
                                }
                            } else if lane == 10 {
                                let mut s = model.write();
                                if let Some(b) = s.batch.as_mut() {
                                    if b.id == id {
                                        b.result = Some(Err(message));
                                    }
                                }
                            } else if lane == 5 {
                                model.write().tree.finish(id, Err(message));
                            } else if id == model.read().index_id {
                                let mut s = model.write();
                                if s.search_live {
                                    s.host_pid = None;
                                    s.config_pending = None;
                                    s.pending_rebuild = None;
                                    s.query_id += 1;
                                    s.index = None;
                                    s.result = None;
                                    s.query_due = None;
                                }
                                s.index_error = Some(message);
                                s.index_starting = false;
                                s.query_loading = false;
                            }
                        }
                    }
                }
                batch_tick(model, &bridge);
                tree_follow_tick(model, &bridge);
                scoped_search_tick(model, &bridge);
                preview_tick(model, &bridge);
                if Instant::now() >= next_thumbnail_check {
                    next_thumbnail_check = Instant::now() + Duration::from_millis(250);
                    thumbnail_tick(model, &bridge);
                    appearance_tick(model, &bridge);
                }
                for pane in 0..4 {
                    if pane >= model.read().layout {
                        if model.read().watches[pane].is_some() {
                            bridge.watches[pane].cancel();
                            let mut s = model.write();
                            s.watches[pane] = None;
                            s.watch_due[pane] = None;
                            s.watch_renames[pane].clear();
                        }
                        continue;
                    }
                    let refresh = {
                        let s = model.read();
                        s.watch_due[pane].is_some_and(|t| Instant::now() >= t)
                            && !s.panes[pane].loading
                            && s.panes[pane].tab().loaded
                            && s.job.is_none()
                            && !s.modal_open()
                            && !s.reveal.contains_key(&pane)
                    };
                    if refresh {
                        start_background(model, &bridge, pane);
                    }
                }
                let due = {
                    let s = model.read();
                    s.query_due.is_some_and(|t| Instant::now() >= t)
                        && s.config_pending.is_none()
                        && s.pending_rebuild.is_none()
                };
                if due {
                    let request = {
                        let mut s = model.write();
                        s.query_due = None;
                        s.query_loading = true;
                        if s.search_live {
                            Request::VolumeQuery {
                                session: s.index_id,
                                generation: s.config_generation,
                                query_id: s.query_id,
                                text: s.query.clone(),
                            }
                        } else {
                            Request::Query {
                                id: s.query_id,
                                text: s.query.clone(),
                            }
                        }
                    };
                    if let Err(message) = bridge.search.send(request) {
                        let mut s = model.write();
                        s.query_loading = false;
                        s.search_error = Some(message);
                    }
                }

                let changed_clipboard = model
                    .read()
                    .clipboard_sequence
                    .is_some_and(|sequence| sequence != platform::clipboard_sequence());
                if changed_clipboard {
                    let mut s = model.write();
                    s.clipboard_marked = Arc::new(BTreeSet::new());
                    s.clipboard_cut = false;
                    s.clipboard_sequence = None;
                }
                let save = model.read().save_due.is_some_and(|t| Instant::now() >= t);
                if save {
                    let mut state = model.write();
                    state.save_due = None;
                    if primary {
                        settings_writer.submit(state.settings());
                    }
                }
                if let Ok(error) = settings_writer.errors.try_recv() {
                    if !model.read().close_save_failed {
                        model.write().notice = format!("설정을 저장하지 못했습니다: {error}");
                    }
                }
            }
        }
    });
    let s = model.read();
    let layout = s.layout;
    let pane_layout = s.pane_arrangement.layout(layout);
    let active = s.active;
    let active_context = format!("{} · {}", s.pane_names[active], s.path().display());
    let selected = s.selected().len();
    let job = s.job.is_some();
    let action_bar_visible = s.action_bar_visible;
    let sidebar_visible = s.sidebar_visible;
    let mut sidebar_style = format!("--sidebar-max-width:{SIDEBAR_MAX_WIDTH}px");
    if let Some(width) = s.sidebar_width {
        sidebar_style.push_str(&format!(";--sidebar-width:{width}px"));
    }
    let sidebar_label = if sidebar_visible {
        "사이드바 접기"
    } else {
        "사이드바 펼치기"
    };
    let job_status = s.job_status.clone();
    let can_undo = !s.undo.is_empty();
    let can_redo = !s.redo.is_empty();
    let search_open = s.search_open;
    let utility_pid = s.utility_pid.unwrap_or(0);
    let memory_mode = if s.memory_saving { "low" } else { "normal" };
    let notice = s.notice.clone();
    let favorites = s.favorites.clone();
    let show_hidden = s.show_hidden;
    let modal = s.dialog.clone();
    let batch_open = s.batch.is_some();
    let journal_open = s.journal_open;
    let appearance_open = s.appearance;
    let context_open = s.context.is_some();
    let copy_destination_open = s.copy_destination.is_some();
    let modal_name = s.dialog_name.clone();
    let preferences_open = s.preferences_open;
    let workspaces_open = s.workspaces_open;
    let inspector = s.inspector;
    let folder_search_open = s.folder_search.is_some();
    let network_dialog = s.network_dialog;
    let theme = s.theme.value();
    let search_select_all = s.search_preferences.select_on_focus.to_string();
    let row = row_height(s.compact);
    drop(s);
    let keyboard_bridge = bridge.clone();
    let mouse_bridge = bridge.clone();
    rsx! {
        style { "{STYLE}" }
        div { class:"app","data-memory-mode":memory_mode,"data-utility-pid":utility_pid,"data-search-select-all":search_select_all,tabindex:-1,"data-theme":theme,"data-os":PLATFORM_NAME,onmouseup:move|e:MouseEvent|mouse_history(e,model,&mouse_bridge,active_pane(model)),onmousedown:move |_|model.write().interaction+=1,oninput:move |_|model.write().interaction+=1,style:"--row-height:{row}px",onkeydown:move|e:KeyboardEvent|{
            model.write().interaction+=1;if e.is_composing(){return;}let control=command_modifier(e.modifiers());
            let modal_open=model.read().modal_open();
            if modal_open{if e.key()==Key::Escape{dismiss_modal(model,&keyboard_bridge);}return;}
            if control&&matches!(e.key(),Key::Character(ref c) if ["a","c","x","v","i","z","y","d"].iter().any(|k|c.eq_ignore_ascii_case(k))){list_key(e,model,&keyboard_bridge,active_pane(model));return;}
            if e.key()==Key::F5{pane_shortcut(&e,model,active_pane(model),&keyboard_bridge);}
            if control||e.modifiers().alt()||matches!(e.key(),Key::F3|Key::F4|Key::F6|Key::F11){pane_shortcut(&e,model,active_pane(model),&keyboard_bridge);}
            if e.key()==Key::Escape{let mut s=model.write();s.dialog=None;s.preferences_open=false;s.workspaces_open=false;}
        },
            div { class:"file-drag-preview", hidden:true, "aria-hidden":"true" }
            input { class:"internal-drop-input", r#type:"hidden",
                oninput:{let b=bridge.clone(); move |event:FormEvent| {
                    event.stop_propagation();
                    handle_drag(model,&b,&event.value());
                }}
            }
            header { class:"app-header",
                button{class:"action-bar-toggle",title:"파일 작업 도구 모음 표시 전환","aria-expanded":action_bar_visible,
                    "aria-controls":"file-action-bar",onclick:move |_|{
                        let mut state = model.write();
                        state.action_bar_visible = !state.action_bar_visible;
                        state.dirty();
                    },"작업 도구"}
                if action_bar_visible {
                div {class:"action-bar",id:"file-action-bar",
                    button{class:"history-undo",title:"파일 작업 실행 취소 · Ctrl+Z","aria-label":"파일 작업 실행 취소",disabled:job||!can_undo,onclick:{let b=bridge.clone();move |_|history_action(model,&b,false)},"↶"}
                    button{class:"history-redo",title:"파일 작업 다시 실행 · Ctrl+Y","aria-label":"파일 작업 다시 실행",disabled:job||!can_redo,onclick:{let b=bridge.clone();move |_|history_action(model,&b,true)},"↷"}
                    button{title:"새 폴더","aria-label":"새 폴더",disabled:job,onclick:move |_|dialog(model,"new"),Icon{kind:"plus"}span{"새 폴더"}}
                    span{class:"action-divider"}
                    button{title:"복사","aria-label":"복사",disabled:selected==0||job,onclick:{let b=bridge.clone();move |_|copy(model,&b,false)},Icon{kind:"copy"}span{"복사"}}
                    button{title:"잘라내기","aria-label":"잘라내기",disabled:selected==0||job,onclick:{let b=bridge.clone();move |_|copy(model,&b,true)},Icon{kind:"cut"}span{"잘라내기"}}
                    button{title:"붙여넣기","aria-label":"붙여넣기",disabled:job,onclick:{let b=bridge.clone();move |_|paste(model,&b)},Icon{kind:"paste"}span{"붙여넣기"}}
                    button{title:"이름 변경","aria-label":"이름 변경",disabled:selected!=1||job,onclick:move |_|start_inline_rename(model),Icon{kind:"rename"}span{"이름 변경"}}
                    button{class:"batch-toggle",disabled:selected==0||selected>500||job,onclick:move |_|open_batch(model),"일괄 이름"}
                    button{title:"휴지통","aria-label":"휴지통",class:"delete-action",disabled:selected==0||job,onclick:move |_|dialog(model,"recycle"),Icon{kind:"trash"}span{"휴지통"}}
                    if layout>1{button{class:"transfer-action",disabled:selected==0||job,title:"복사할 패널과 탭의 폴더 선택","aria-label":"복사 대상 선택",onclick:move |_|open_copy_destination(model),Icon{kind:"arrow"}span{"복사 대상 선택"}}}
                }
                }
                div {class:"header-spacer"}
                button{class:"journal-toggle",onclick:move |_|model.write().journal_open=true,"작업 기록"}
                button{class:"workspace-toggle",title:"작업공간 저장·전환",onclick:move |_|model.write().workspaces_open=true,"작업공간"}
                button{class:"settings-toggle",title:"설정",onclick:move |_|model.write().preferences_open=true,Icon{kind:"settings"}span{"설정"}}
                button {class:if search_open{"search-toggle active"}else{"search-toggle"},title:"통합 검색 · Ctrl+Shift+F",onclick:move |_|{let mut s=model.write();s.search_open=!s.search_open;if !s.search_open{s.search_expanded=false;}s.search_focus=if s.search_open&&s.search_preferences.focus_on_open{Some(s.interaction)}else{None};},Icon{kind:"search"}span{"통합 검색"}kbd{if cfg!(windows) {"Ctrl Shift F"} else {"⌘ ⇧ F"}}}
                LayoutSwitcher { model }
            }
            div {class:"body",
                aside {id:"navigation-sidebar",class:if sidebar_visible {"sidebar"} else {"sidebar collapsed"},style:sidebar_style,"aria-label":"폴더 탐색",
                    button{class:"sidebar-toggle",r#type:"button",title:sidebar_label,"aria-label":sidebar_label,
                        "aria-expanded":sidebar_visible,"aria-controls":"navigation-sidebar-content",
                        onclick:move |_|{
                            let mut state = model.write();
                            state.sidebar_visible = !state.sidebar_visible;
                            state.dirty();
                        },Icon{kind:if sidebar_visible {"sidebar-collapse"} else {"sidebar-expand"}}}
                    div{id:"navigation-sidebar-content",class:"sidebar-content",hidden:!sidebar_visible,
                    div {class:"section-label","빠른 이동"}
                    button {class:"side-link",onclick:{let b=bridge.clone();move |_|navigate(model,&b,active_pane(model),settings::home())},Icon{kind:"home"}span{"홈"}}
                    for (name,path) in locations.iter(){
                        button {class:"side-link",onclick:{let b=bridge.clone();let path=path.clone();move |_|navigate(model,&b,active_pane(model),path.clone())},Icon{kind:"folder"}span{"{name}"}}
                    }
                    DirectoryTree { model }
                    RecentLocations{model}
                    if cfg!(windows) { details{class:"sidebar-section network-drive-controls",
                        summary{title:"네트워크 드라이브","네트워크 드라이브"}
                        for (disconnect,label) in [(false,"연결"),(true,"연결 끊기")] {
                            button{class:"side-link",onclick:move |_|model.write().network_dialog=Some(disconnect),"{label}"}
                        }
                    }
                    }
                    div {class:"sidebar-section",div {class:"section-label",span{"즐겨찾기"}button{class:"small-add",title:"현재 폴더 추가",onclick:move |_|{let path=model.read().path();let mut s=model.write();if !s.favorites.contains(&path){s.favorites.push(path);s.dirty();}},"+"}}
                        if favorites.is_empty(){p{class:"side-hint","자주 쓰는 폴더를\n추가해 보세요."}}
                        for (i,path) in favorites.iter().enumerate(){ { let p=path.clone();let name=path.name();let full=path.display(); rsx! { div{class:"favorite-row",button{class:"side-link",title:"{full}",onclick:{let b=bridge.clone();move |_|navigate(model,&b,active_pane(model),p.clone())},Icon{kind:"star"}span{"{name}"}}button{class:"remove-favorite",title:"즐겨찾기에서 제거",onclick:move |_|{model.write().favorites.remove(i);model.write().dirty();},"×"}} } } }
                    }
                    div {class:"sidebar-bottom",label{class:"hidden-option",input{r#type:"checkbox",checked:show_hidden,onchange:{let b=bridge.clone();move|e:FormEvent|set_hidden(model,&b,e.checked())}}span{"숨김 파일 표시"}}p{"SDIR "{env!("CARGO_PKG_VERSION")}" · {PLATFORM_NAME}"}}
                    }
                }
                if sidebar_visible {
                    div {class:"sidebar-resizer",role:"separator",tabindex:0,"aria-orientation":"vertical",
                        "aria-label":"사이드바 너비","aria-controls":"navigation-sidebar",
                        title:"드래그 또는 좌우 방향키로 사이드바 너비 조절",
                        onmounted:move |_|{let _ = document::eval("window.sdirSyncSidebarWidth?.()");},
                        input {class:"sidebar-width-commit",r#type:"hidden",oninput:move |event:FormEvent| {
                            event.stop_propagation();
                            if let Ok(width) = event.value().parse::<u16>() {
                                let mut state = model.write();
                                state.sidebar_width = Some(width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH));
                                state.dirty();
                            }
                        }}
                    }
                }
                main {class:"workspace","data-action-bar":if action_bar_visible{"open"}else{"closed"},
                    div {class:format!("panes layout-{layout}"),style:grid_style(pane_layout),"data-grid-rows":pane_layout.rows.len(),for pane_id in 0..layout{PaneView{key:"pane-{pane_id}",pane_id,model}}}
                }
                if let Some(mode) = inspector { FileInspector {model,mode} }
                if search_open{SearchPanel{model}}
            }
            footer {
                class: "status-bar",
                span { class: if job { "status-dot busy" } else { "status-dot" } }
                if let Some(status) = job_status {
                    span {
                        class: "status-message status-job-message",
                        title: "{status.message}",
                        "{status.message}"
                    }
                    span {
                        class: "status-job-progress",
                        if let Some(percent) = status.percent {
                            progress {
                                value: "{percent}",
                                max: "100",
                                "aria-label": "{status.kind} 진행률"
                            }
                            span { class: "status-job-percent", "약 {percent}%" }
                        } else {
                            progress {
                                max: "100",
                                "aria-label": "{status.kind} 진행률, 측정값 없음"
                            }
                            span { class: "status-job-percent", "진행 중" }
                        }
                    }
                } else {
                    span { class: "status-message", title: "{notice}", "{notice}" }
                }
                span {
                    class: "status-meta",
                    title: "{active_context}",
                    "현재 패널 {active+1} · {selected}개 선택"
                }
            }
            if folder_search_open { FolderSearchDialog {model} }
            if let Some(disconnect) = network_dialog { NetworkDriveDialog {model,disconnect} }
            if batch_open{BatchRenameDialog{model}}
            if journal_open{JournalDialog{model}}
            if let Some(pane)=appearance_open{AppearanceDialog{model,pane}}
            if context_open{ContextMenuView{model}}
            if copy_destination_open{CopyDestinationDialog{model}}
            if preferences_open{Preferences{model}}
            if workspaces_open{Workspaces{model}}
            if let Some(kind)=modal{
                div{class:"modal-backdrop",div{class:"modal",role:"dialog","aria-modal":"true",
                    onkeydown:move |event: KeyboardEvent| {
                        event.stop_propagation();
                        if event.key()==Key::Escape && !event.is_composing() {
                            event.prevent_default();
                            model.write().dialog=None;
                            focus_list(active_pane(model));
                        }
                    },
                    h2{match &kind{Dialog::NewFolder=>"새 폴더 만들기",Dialog::Recycle(_)=>"휴지통으로 이동",Dialog::Delete(_)=>"영구 삭제"}}
                    if let Dialog::Delete(ref paths)=kind{p{"선택한 {paths.len()}개 항목을 영구 삭제합니다. 휴지통으로 이동하지 않으며 이 작업은 실행 취소할 수 없습니다."}}
                    else if let Dialog::Recycle(ref paths)=kind{p{"선택한 {paths.len()}개 항목을 휴지통으로 이동합니다." if cfg!(windows) {" 위치에 따라 Windows가 영구 삭제 여부를 별도로 확인할 수 있습니다."}}}
                    else{input{class:"dialog-input new-folder-input","data-ime-commit":"true","aria-label":"새 폴더 이름",initial_value:"{modal_name}",
                        onmounted:move |_|{
                            if matches!(model.read().dialog,Some(Dialog::NewFolder)){
                                let _=document::eval("{const e=document.querySelector('.new-folder-input'),a=document.activeElement;if(e&&(a===e||a===document.body||a?.matches('.app,.list-scroll,button[aria-label=\"새 폴더\"]'))){e.focus({preventScroll:true});e.select();}}");
                            }
                        },
                        onchange:move|e|model.write().dialog_name=e.value(),onkeydown:{let b=bridge.clone();move|e:KeyboardEvent|{e.stop_propagation();if e.key()==Key::Enter&&!e.is_composing(){confirm(model,&b);}if e.key()==Key::Escape{model.write().dialog=None;}}}}}
                    div{class:"modal-actions",
                        button{onclick:move |_|model.write().dialog=None,"취소"}
                        button{class:"primary dialog-confirm",disabled:job,
                            onmounted:move |_| {
                                if matches!(model.read().dialog,Some(Dialog::Recycle(_) | Dialog::Delete(_))) {
                                    let _=document::eval("{const e=document.querySelector('.dialog-confirm');if(e&&!e.disabled&&!e.closest('.modal')?.contains(document.activeElement))e.focus({preventScroll:true});}");
                                }
                            },
                            onkeydown:{let b=bridge.clone();move |event: KeyboardEvent|{
                                if event.key() == Key::Enter && !event.is_composing() {
                                    event.prevent_default();
                                    event.stop_propagation();
                                    confirm(model, &b);
                                }
                            }},
                            onclick:{let b=bridge.clone();move |_|confirm(model,&b)},"확인"
                        }
                    }
                }}
            }
        }
    }
}

#[component]
fn PaneView(pane_id: usize, mut model: Signal<AppModel>) -> Element {
    let mut editing = use_signal(|| None::<String>);
    let mut rename_seed = use_signal(String::new);
    let mut rename_focus = use_signal(|| None::<u64>);
    let mut path_menu = use_signal(|| false);
    use_drop(move || {
        let _ = document::eval(&format!(
            "window.__sdirViewObservers?.[{pane_id}]?.disconnect();window.__sdirScrollRequests?.[{pane_id}]?.disconnect();"
        ));
    });
    let bridge = use_context::<Arc<Bridge>>();
    let state = model.read();
    let p = state.panes[pane_id].clone();
    let active = state.active == pane_id;
    let name = state.pane_names[pane_id].clone();
    let watch_ready = state.watches[pane_id].as_ref().is_some_and(|w| w.ready);
    let watch_error = state.watch_errors[pane_id].clone();
    let compact = state.compact;
    let show_extensions = state.show_extensions;
    let selection_checkboxes = state.selection_checkboxes;
    let marked = state.clipboard_marked.clone();
    let cut = state.clipboard_cut;
    let history_open = state.address_history == Some(pane_id);
    let inline_rename = state
        .inline_rename
        .as_ref()
        .filter(|edit| edit.location.pane == pane_id && edit.location.matches(&state))
        .cloned();
    drop(state);
    let view = p.view.value();
    let geometry = p.geometry(compact);
    let columns = geometry.columns;
    let tile_height = geometry.row_height;
    let small_icons = tile_height < 90.;
    let reveal_revision = p.reveal_revision;
    let detail_style = p.columns.details_style();
    let icon_size = p.icon_size;
    let selected_view = pane_view_option(p.view, icon_size);
    let custom_columns = p.columns.custom;
    let col_visible = p.columns.visible;
    let icon_pixels = if small_icons {
        24.
    } else {
        (f64::from(p.icon_size) * 0.32).clamp(24., 64.)
    };
    let tab = p.tab();
    let grouping = tab.group_by.value();
    let (start, end, offset, height) = geometry.virtual_range(tab.render_len(), tab.scroll);
    let indices = tab.visible.clone();
    let entries = tab.entries.clone();
    let selected = tab.selection.clone();
    let cursor = tab.cursor.clone();
    let focused_row = cursor.as_ref().and_then(|path| {
        (start..end)
            .filter_map(|row| tab.item_at_render_row(row).ok())
            .find(|&position| &tab.entries[tab.visible[position]].path == path)
            .map(|position| format!("pane-{pane_id}-file-{position}"))
    });
    let generation = p.generation;
    let filter = tab.filter.clone();
    let filter_revision = tab.filter_revision();
    let error = tab.error.clone();
    let count = tab.visible.len();
    let total = tab.entries.len();
    let pane_bridge = bridge.clone();
    let keys = bridge.clone();
    let pane_keys = bridge.clone();
    let click_bridge = bridge.clone();
    let editing_name = editing.read().is_some();
    let initial_name = rename_seed.read().clone();
    let ancestors: Vec<_> = tab
        .path
        .to_path()
        .ancestors()
        .map(NativePath::new)
        .collect();
    let address_revision = p.generation;
    let menu_open = *path_menu.read();
    let mut seen = BTreeSet::new();
    let history: Vec<_> = std::iter::once(&tab.path)
        .chain(tab.back.iter().rev())
        .chain(tab.forward.iter().rev())
        .take(if history_open { usize::MAX } else { 0 })
        .filter(|path| seen.insert((*path).clone()))
        .take(12)
        .cloned()
        .collect();
    rsx! {
        article{onmouseup:{let b=bridge.clone();move|e:MouseEvent|mouse_history(e,model,&b,pane_id)},class:if active{"pane focused"}else{"pane"},"data-pane":pane_id,"data-auto-watch":if watch_ready{"ready"}else{"waiting"},role:"region","aria-current":if active{"true"}else{"false"},"aria-label":format!("패널 {} · {}",pane_id+1,name),"data-view":view,"data-small-icons":small_icons,"data-custom-columns":custom_columns,style:"--tile-height:{tile_height}px;{detail_style}--file-icon-size:{icon_pixels}px",onclick:move |_|activate_pane(model,pane_id),onfocusin:move |_|activate_pane(model,pane_id),onkeydown:move|e:KeyboardEvent|{if command_modifier(e.modifiers())||e.modifiers().alt()||matches!(e.key(),Key::F3|Key::F4|Key::F6|Key::F11)||matches!(e.key(),Key::Character(ref c) if !c.trim().is_empty()){list_key(e,model,&pane_keys,pane_id);}},
            div{class:"group-header",
                span{class:"group-number","{pane_id+1}"}
                if editing_name{
                    input{class:"group-name-input","data-ime-commit":"true","aria-label":"그룹 이름",initial_value:"{initial_name}",maxlength:64,
                        onmounted:move |_|{
                            let request=rename_focus.write().take();
                            let state=model.read();
                            if request==Some(state.interaction)&&!state.modal_open(){
                                let _=document::eval(&format!("{{const e=document.querySelector('[data-pane=\"{pane_id}\"] .group-name-input');if(e&&document.activeElement===document.body){{e.focus({{preventScroll:true}});e.select();}}}}"));
                            }
                        },
                        onchange:move|e|{
                            let value=e.value();
                            if editing.peek().as_ref()!=Some(&value){editing.set(Some(value));}
                        },
                        onkeydown:move|e:KeyboardEvent|{e.stop_propagation();if e.is_composing(){return;}match e.key(){Key::Enter=>{let value=editing.read().clone().unwrap_or_default();name_pane(model,pane_id,value);editing.set(None);},Key::Escape=>editing.set(None),_=>{}}}}
                    button{class:"group-save",title:"그룹 이름 저장",onclick:move |_|{let value=editing.read().clone().unwrap_or_default();name_pane(model,pane_id,value);editing.set(None);},"저장"}
                    button{class:"group-cancel",title:"그룹 이름 취소",onclick:move |_|editing.set(None),"취소"}
                }else{
                    button{class:"group-name",title:"그룹 이름 변경",onclick:{let name=name.clone();move |_|{
                        model.write().pending_focus=None;
                        rename_focus.set(Some(model.read().interaction));
                        rename_seed.set(name.clone());
                        editing.set(Some(name.clone()));
                    }},span{class:"group-title","{name}"}Icon{kind:"rename"}}
                }
                button{class:"pane-appearance",title:"보기 설정 · 열/아이콘 크기",onclick:move |_|model.write().appearance=Some(pane_id),"⚙"}
                if active {span{class:"pane-active-badge",title:"파일 작업이 적용되는 현재 패널","현재 패널"}}
                select{class:"view-select","aria-label":"보기 방식",title:"이 패널 보기 방식",value:selected_view,onkeydown:move|e:KeyboardEvent|e.stop_propagation(),onchange:move|e:FormEvent|set_view(model,pane_id,&e.value()),
                    for (value, _, label) in PANE_VIEW_OPTIONS {
                        option{key:"{value}",value,"{label}"}
                    }
                    if pane_view_option(ViewMode::Icons, icon_size) == "icons" { option{value:"icons","사용자 지정 아이콘"} }}
            }
            div{class:"tabs",for (i,t) in p.tabs.iter().enumerate(){ { let name=t.path.name();let pinned=t.pinned; rsx! { button{oncontextmenu:move|e:MouseEvent|{e.prevent_default();e.stop_propagation();open_tab_context(model,pane_id,i,e.client_coordinates().x,e.client_coordinates().y);},class:if p.active==i{"tab selected"}else{"tab"},"data-drag-tab":i,title:"{t.path.display()}",onclick:{let b=bridge.clone();move |_|change_tab(model,&b,pane_id,TabCommand::Select(i),false)},Icon{kind:"folder"}span{"{name}"}if pinned{span{class:"tab-pinned",title:"고정된 탭 · 우클릭으로 해제","●"}}if p.tabs.len()>1&&!pinned{span{class:"tab-close",title:"탭 닫기 · Ctrl+W",onclick:{let b=bridge.clone();move|e:MouseEvent|{e.stop_propagation();change_tab(model,&b,pane_id,TabCommand::Close(i),true);}},"×"}}} } } }
                button{class:"new-tab",title:"새 탭 · Ctrl+T",disabled:p.tabs.len()>=16,onclick:move |_|change_tab(model,&pane_bridge,pane_id,TabCommand::New,true),"+"}
                button{class:"restore-tab",title:"닫은 탭 복원 · Ctrl+Shift+T",disabled:!p.can_restore_tab(),onclick:{let b=bridge.clone();move |_|change_tab(model,&b,pane_id,TabCommand::Restore,true)},"↶"}
            }
            div{class:"address-bar",
                button{class:"icon-button",title:"뒤로",disabled:tab.back.is_empty(),onclick:{let b=bridge.clone();move |_|{history_navigation(model,&b,pane_id,false)}},"‹"}
                button{class:"icon-button",title:"앞으로",disabled:tab.forward.is_empty(),onclick:{let b=bridge.clone();move |_|{history_navigation(model,&b,pane_id,true)}},"›"}
                button{class:"icon-button",title:"상위 폴더",onclick:{let b=bridge.clone();move |_|{let parent=model.read().panes[pane_id].tab().path.parent();if let Some(path)=parent{navigate(model,&b,pane_id,path);}}},"↑"}
                div{class:"address-input-wrap",button{class:"path-menu-toggle",title:"상위 경로 바로 이동","aria-expanded":menu_open,onclick:move |_|path_menu.set(!menu_open),Icon{kind:"folder"}}
                    div{class:"address-value",
                        nav{class:"address-breadcrumbs","aria-label":"폴더 경로",
                            div{class:"address-trail",
                                for (index, ancestor) in ancestors.iter().rev().enumerate() { {
                                    let location = ancestor.clone();
                                    let full = location.display();
                                    let name = location.name();
                                    let current = location == tab.path;
                                    rsx! {
                                        if index != 0 { span{class:"address-separator","aria-hidden":"true","›"} }
                                        button{class:"address-crumb",title:"{full}","data-path":"{full}",
                                            "aria-current":if current {"location"} else {"false"},
                                            onclick:{let b=bridge.clone();move|event:MouseEvent|{
                                                event.stop_propagation();
                                                if current {
                                                    focus_address(pane_id);
                                                } else {
                                                    path_menu.set(false);
                                                    activate_pane(model,pane_id);
                                                    navigate(model,&b,pane_id,location.clone());
                                                }
                                            }},
                                            "{name}"
                                        }
                                    }
                                } }
                            }
                        }
                        {rsx!{input{key:"address-{address_revision}",id:"address-{pane_id}",class:"address-input","aria-label":"주소 입력","data-ime-commit":"true",initial_value:"{p.address}",spellcheck:false,onchange:move|e|model.write().panes[pane_id].address=e.value(),onkeydown:{let b=bridge.clone();move|e:KeyboardEvent|{e.stop_propagation();model.write().interaction+=1;if pane_shortcut(&e,model,pane_id,&b){return;}if e.key()==Key::Enter&&!e.is_composing(){let value=model.read().panes[pane_id].address.clone();navigate(model,&b,pane_id,NativePath::new(value));}}}}}}
                    }
                    button{class:"address-edit",title:"주소 편집","aria-label":"주소 편집",onclick:move |_|focus_address(pane_id),Icon{kind:"rename"}}
                    if history_open {
                        div {class:"path-menu-dismiss", onclick:move |e:MouseEvent|{e.stop_propagation();model.write().address_history=None;}}
                        div {class:"path-menu address-history", role:"menu", "aria-label":"주소 기록",
                            onkeydown:{let b=bridge.clone();move|e:KeyboardEvent|{e.stop_propagation();pane_shortcut(&e,model,pane_id,&b);}},
                            for location in history {
                                {let label=location.display();let keyboard_target=location.clone();rsx!{button {
                                    class:"path-crumb", role:"menuitem", title:"{label}",
                                    onkeydown:{let b=bridge.clone();move|e:KeyboardEvent|{
                                        if e.key()==Key::Enter&&!e.is_composing(){
                                            e.prevent_default();e.stop_propagation();navigate(model,&b,pane_id,keyboard_target.clone());
                                        }
                                    }},
                                    onclick:{let b=bridge.clone();move|e:MouseEvent|{e.stop_propagation();navigate(model,&b,pane_id,location.clone());}},
                                    "{label}"
                                }}}
                            }
                        }
                    } else if menu_open{
                        div{class:"path-menu-dismiss",onclick:move|e:MouseEvent|{e.stop_propagation();path_menu.set(false);}}
                        div{class:"path-menu",role:"menu",for ancestor in ancestors.iter().rev(){ {let target=ancestor.clone();let full=target.display();let name=target.name();rsx!{button{class:"path-crumb",role:"menuitem",title:"{full}",onclick:{let b=bridge.clone();move|e:MouseEvent|{e.stop_propagation();path_menu.set(false);model.write().active=pane_id;navigate(model,&b,pane_id,target.clone());}},"{name}"}}}}}
                    }
                }
                button{class:"icon-button",title:if p.loading{"읽기 취소"}else{"새로고침 · F5"},onclick:{let b=bridge.clone();move |_|{if model.read().panes[pane_id].loading{b.panes[pane_id].cancel();let mut s=model.write();let id=s.refreshes[pane_id].take().map(|r|r.id);let p=&mut s.panes[pane_id];p.loading=false;p.generation=p.generation.max(id.unwrap_or(0))+1;s.notice="폴더 읽기를 취소했습니다.".into();}else{load(model,&b,pane_id);}}},if p.loading{"×"}else{"↻"}}
            }
            div{class:"pane-filter",Icon{kind:"search"}
                {rsx!{input{key:"filter-{filter_revision}",id:"pane-query-{pane_id}",placeholder:"현재 목록 필터 · Ctrl+E","data-ime-commit":"true",
                    title:"이 패널의 현재 폴더에서 이름 검색",initial_value:"{filter}",spellcheck:false,
                    onchange:move|event:FormEvent|{
                        event.stop_propagation();
                        let value=event.value();
                        let mut state=model.write();
                        let tab=state.panes[pane_id].tab_mut();
                        if tab.filter_revision()==filter_revision && tab.filter!=value {
                            tab.set_filter(value);
                            state.dirty();
                        }
                    },
                    onkeydown:{let bridge=bridge.clone();move|event:KeyboardEvent|{
                        event.stop_propagation();
                        if event.is_composing(){return;}
                        model.write().interaction+=1;
                        if pane_shortcut(&event,model,pane_id,&bridge){return;}
                        match event.key(){
                            Key::Escape=>{
                                release_edit_focus(pane_id);
                                let mut state=model.write();
                                state.panes[pane_id].tab_mut().clear_filter();
                                state.dirty();
                                drop(state);
                                focus_list_after_render(model,pane_id);
                            }
                            Key::Enter=>{
                                release_edit_focus(pane_id);
                                focus_list_after_render(model,pane_id);
                            }
                            _=>{}
                        }
                    }}
                }}}
                if !filter.is_empty(){button{class:"clear-pane-search",title:"패널 검색 지우기",onclick:move |_|{
                    let mut state=model.write();
                    state.panes[pane_id].tab_mut().clear_filter();
                    state.dirty();
                    drop(state);
                    focus_list_after_render(model,pane_id);
                },"×"}}
                span{class:"pane-label","패널 {pane_id+1}"}
            }
            input { class:"pane-marquee-input", r#type:"hidden",
                oninput:move |e:FormEvent| {
                    e.stop_propagation();
                    select_rectangle(model, pane_id, &e.value());
                }
            }
            input { class:"pane-zoom-input", r#type:"hidden",
                oninput:move |e:FormEvent| {
                    e.stop_propagation();
                    if !model.read().modal_open() {
                        zoom_icons(model, pane_id, e.value() == "in");
                    }
                }
            }
            if p.view == ViewMode::Details {
                div{class:"list-header detail-header",
                    div{class:"detail-header-track",
                        for (column, (label, sort_column)) in [
                            ("이름", Some(SortColumn::Name)),
                            ("크기", Some(SortColumn::Size)),
                            ("종류", None),
                            ("수정 날짜", Some(SortColumn::Modified)),
                        ].into_iter().enumerate() {
                            if col_visible[column] {
                                div{key:"column-{column}",class:"detail-heading","data-detail-column":column,
                                    if let Some(sort_column) = sort_column {
                                        button{onclick:move |_| sort(model,pane_id,sort_column),"{label}"}
                                    } else {
                                        span{"{label}"}
                                    }
                                    span{class:"detail-resize-handle",title:"{label} 너비 조정","aria-hidden":"true",
                                        "data-min-width":sdir::preferences::Columns::minimum_width(column),
                                        "data-max-width":sdir::preferences::Columns::MAX_WIDTH,
                                    }
                                }
                            }
                        }
                    }
                    input{class:"detail-width-input",r#type:"hidden",oninput:move|e:FormEvent| {
                        let value = e.value();
                        let Some((column, width)) = value.split_once(':') else { return; };
                        let (Ok(column), Ok(width)) = (column.parse::<usize>(), width.parse::<u16>()) else { return; };
                        let mut state = model.write();
                        let pane = &mut state.panes[pane_id];
                        if pane.view == ViewMode::Details && pane.columns.resize(column, width) {
                            state.dirty();
                        }
                    }}
                }
            } else {
                div{class:"list-header",button{onclick:move |_|sort(model,pane_id,SortColumn::Name),"이름"}}
            }
            {rsx! {div{key:"list-{pane_id}-{generation}-{filter}-{compact}-{view}-{icon_size}-{reveal_revision}",class:"list-scroll",tabindex:0,role:"listbox","aria-label":"{name} 파일 목록","aria-multiselectable":"true","aria-activedescendant":focused_row,"data-grouping":grouping,"data-drag-generation":generation,"data-drag-selection-count":selected.len(),"data-drag-folder":"{p.tab().path.display()}",oncontextmenu:{let b=bridge.clone();move|e:MouseEvent|{e.prevent_default();e.stop_propagation();open_context(model,&b,pane_id,None,e.client_coordinates().x,e.client_coordinates().y,false);}},"data-columns":columns,onmounted:move |_|{
                    let (scroll,focus,startup)={let s=model.read();(s.panes[pane_id].tab().scroll,s.pending_focus==Some((pane_id,s.interaction))&&s.active==pane_id&&!s.modal_open(),s.interaction==0||s.pending_focus_guarded)};
                    let _=document::eval(&format!("{{const e=document.querySelector('[data-pane=\"{pane_id}\"] .list-scroll');if(e){{e.scrollTop={scroll};window.__sdirViewObservers??={{}};window.__sdirViewObservers[{pane_id}]?.disconnect();window.__sdirScrollRequests?.[{pane_id}]?.disconnect();const observer=new ResizeObserver(()=>e.dispatchEvent(new Event('scroll')));window.__sdirViewObservers[{pane_id}]=observer;observer.observe(e);}}}}"));
                    if focus{if startup{let _=document::eval(&format!("if(!document.activeElement?.matches('input,textarea,select,button,.sidebar-resizer'))document.querySelector('[data-pane=\"{pane_id}\"] .list-scroll')?.focus({{preventScroll:true}});"));}else{let _=document::eval(&format!("if(!document.activeElement?.matches('input,textarea,.sidebar-resizer')&&!document.querySelector('.modal-backdrop,.context-dismiss'))document.querySelector('[data-pane=\"{pane_id}\"] .list-scroll')?.focus({{preventScroll:true}});"));}}
                },onclick:{let b=bridge.clone();move|_:MouseEvent|{
                    if !confirm_inline_rename_on_list_click(model, &b) { return; }
                    let mut state = model.write();
                    state.active = pane_id;
                    state.interaction += 1;
                    state.pending_focus = None;
                    state.panes[pane_id].tab_mut().clear_selection();
                    state.dirty();
                    drop(state);
                    focus_list(pane_id);
                }},onkeydown:move|e|list_key(e,model,&keys,pane_id),onscroll:move|e:ScrollEvent|{let value=e.scroll_top();let left=e.scroll_left();let _=document::eval(&format!("{{let h=document.querySelector('[data-pane=\"{pane_id}\"] .list-header');if(h)h.style.setProperty('--detail-scroll-left','{left}px');}}"));let width=e.client_width() as f64;let height=e.client_height() as f64;let change={let s=model.read();let p=&s.panes[pane_id];(value-p.tab().scroll).abs()>=1.||(width-p.viewport_width).abs()>=1.||(height-p.viewport_height).abs()>=1.};if change&&width>0.&&height>0.{let requested={let mut s=model.write();let compact=s.compact;let requested=s.panes[pane_id].update_viewport(width,height,value,compact);s.dirty();requested};if let Some((position,scroll))=requested{let _=document::eval(&format!("window.__sdirScrollSelection?.({pane_id},{position},{scroll});"));}}},
                ondragover:move|e:DragEvent|e.prevent_default(),ondrop:move|e:DragEvent|{let destination=model.read().panes[pane_id].tab().path.clone();drop_external_files(model,&click_bridge,pane_id,destination,e);},
                if total==0&&!p.loading{div{class:"empty-folder",Icon{kind:"folder"}strong{if error.is_some(){"폴더를 열지 못했습니다"}else{"폴더가 비어 있습니다"}}p{if error.is_some(){"클라우드·네트워크 드라이브의 연결과 접근 권한을 확인하세요. F5로 다시 시도할 수 있습니다."}else{"파일을 여기에 끌어다 놓아 복사할 수 있습니다."}}}}
                if p.loading&&total==0{div{class:"loading-folder",span{class:"spinner"}span{"폴더를 읽고 있습니다"}}}
                if total>0&&count==0{div{class:"empty-folder",strong{"일치하는 항목이 없습니다"}p{"필터를 바꿔 보세요."}}}
                div{class:"virtual-space",style:"height:{height}px",div{class:"virtual-rows",style:"transform:translateY({offset}px);--view-columns:{columns}",
                    {(start..end).map(|visual_row| {
                        if let Ok(position) = tab.item_at_render_row(visual_row) {
                        let entry=entries[indices[position]].clone();
                        let thumbnail=if p.view == ViewMode::Icons {
                            model.read().thumbnails.get(&ThumbnailKey::from(&entry)).and_then(Clone::clone)
                        } else {None};
                        let appearance = if entry.is_dir {None} else {model.read().file_appearance.for_entry(&entry)};
                        let display_name=if show_extensions||entry.is_dir {entry.name.as_str()} else {
                            std::path::Path::new(&entry.name).file_stem().and_then(|name|name.to_str()).unwrap_or(&entry.name)
                        };let path=entry.path.clone();let open=entry.path.clone();let chosen=selected.contains(&entry.path);let clipboard=if marked.contains(&entry.path){if cut{"clipboard-cut"}else{"clipboard-copy"}}else{""};
                        let state_class = if chosen {
                            "selected"
                        } else if cursor.as_ref() == Some(&entry.path) {
                            "keyboard-cursor"
                        } else {
                            ""
                        };
                        let class = format!("file-row {state_class} {clipboard}");
                        let kind=if entry.is_dir{"폴더".to_string()}else if let Some(item)=appearance.as_ref().filter(|item| !item.type_name.is_empty()){item.type_name.clone()}else{entry.path.to_path().extension().map(|s|format!("{} 파일",s.to_string_lossy().to_uppercase())).unwrap_or_else(||"파일".into())};let bytes=readable_size(entry.bytes);let modified=entry.modified.and_then(|s|chrono::DateTime::from_timestamp(s as i64,0)).map(|t|t.with_timezone(&chrono::Local).format("%Y.%m.%d %H:%M").to_string()).unwrap_or_else(||"—".into());let is_dir=entry.is_dir;let cloud=entry.cloud.label();
                        let row_key = String::from_utf16(&path.0).map_or_else(
                            |_| format!("raw:{:?}", path.0),
                            |path| format!("path:{path}"),
                        );
                         rsx! { div{key:"{row_key}",id:"pane-{pane_id}-file-{position}","data-row-index":position,draggable:false,"data-drag-key":"{entry.path.display()}","data-drop-directory":if is_dir{"true"}else{"false"},
                        ondrop:{let b=bridge.clone();let destination=path.clone();move|event:DragEvent|{if is_dir{drop_external_files(model,&b,pane_id,destination.clone(),event);}}},
                        oncontextmenu:{let p=path.clone();let b=bridge.clone();move|e:MouseEvent|{e.prevent_default();e.stop_propagation();open_context(model,&b,pane_id,Some(p.clone()),e.client_coordinates().x,e.client_coordinates().y,false);}},class,role:"option","aria-selected":chosen,onclick:{let b=bridge.clone();move |e:MouseEvent| {
                            e.stop_propagation();
                            if !confirm_inline_rename_on_list_click(model, &b) { return; }
                            let mut state = model.write();
                            state.active = pane_id;
                            state.interaction += 1;
                            state.pending_focus = None;
                            state.panes[pane_id].select(path.clone(), command_modifier(e.modifiers()), e.modifiers().shift());
                            state.dirty();
                            drop(state);
                            focus_list(pane_id);
                        }},ondoubleclick:{let b=bridge.clone();move |_|{if is_dir{navigate(model,&b,pane_id,open.clone());}else{let _=b.job(Request::Open{path:open.clone()});}}},
                            div{class:"file-name",
                            if selection_checkboxes {
                                input{class:"file-select-checkbox",r#type:"checkbox","aria-label":"{entry.name} 선택",tabindex:-1,checked:chosen,
                                    onclick:{let b=bridge.clone();move |e:MouseEvent| {
                                        e.stop_propagation();
                                        if !confirm_inline_rename_on_list_click(model, &b) { return; }
                                        let mut s = model.write();
                                        s.active = pane_id;
                                        s.interaction += 1;
                                        s.pending_focus = None;
                                        s.panes[pane_id].tab_mut().toggle_position(position);
                                        s.dirty();
                                        drop(s);
                                        focus_list(pane_id);
                                    }},
                                    ondoubleclick:move|e:MouseEvent|e.stop_propagation()
                                }
                            }
                            if let Some(url) = thumbnail.as_ref() {
                                img{class:"file-thumbnail",src:"{url}",alt:"","aria-hidden":"true",draggable:false}
                            } else {
                                {file_icon(is_dir, appearance.as_deref(), p.view == ViewMode::Icons)}
                            }
                            if let Some(edit)=inline_rename.as_ref().filter(|edit|edit.source==entry.path) {
                                {inline_rename_editor(model,bridge.clone(),edit,is_dir)}
                            } else { span{title:"{entry.name}","{display_name}"} }
                            if !cloud.is_empty(){span{class:"cloud-state",title:"파일 시스템이 보고한 보관 상태; 동기화 성공 여부는 공급자에서 확인하세요","{cloud}"}}if entry.is_link{span{class:"link-badge","↗"}}}span{class:if !col_visible[1]{"file-size column-hidden"}else{"file-size"},"{bytes}"}span{class:if !col_visible[2]{"file-kind column-hidden"}else{"file-kind"},"{kind}"}span{class:if !col_visible[3]{"file-date column-hidden"}else{"file-date"},"{modified}"}
                        }
                     } } else {
                        let heading = tab.item_at_render_row(visual_row).unwrap_err();
                        rsx!{div{key:"file-group-{visual_row}",class:"file-group-heading",role:"presentation","{heading}"}}
                        }
                    })}
                }}
            }
            }}
            div{class:"pane-status",
                span{class:"watch-state",title:watch_error.clone().unwrap_or_else(||if watch_ready{"현재 폴더의 외부 변경을 자동 반영합니다".into()}else{"폴더 변경 감시에 연결 중입니다".into()}),
                    if watch_error.is_some(){"갱신 중단"}else if watch_ready{"자동 갱신"}else{"연결 중"}}
                if p.loading{span{class:"mini-spinner"}span{class:"pane-item-count",title:"읽는 중 · {total}개","읽는 중 · {total}개"}}
                else{span{class:"pane-item-count",title:"{count} / {total}개 항목","{count} / {total}개 항목"}}
                if !matches!(p.view, ViewMode::Icons | ViewMode::Tiles) {
                label{class:"pane-group-items",title:"항목 분류 방법",span{"분류 방법"}select{class:"group-items-select","aria-label":"항목 분류 방법",value:grouping,
                    onchange:move |e:FormEvent| {
                        let group = match e.value().as_str() {
                            "modified" => GroupBy::Modified,
                            "kind" => GroupBy::Kind,
                            _ => GroupBy::None,
                        };
                        let mut state = model.write();
                        let tab = state.panes[pane_id].tab_mut();
                        tab.group_by = group;
                        tab.rebuild_visible();
                        tab.scroll = 0.;
                        state.dirty();
                        drop(state);
                        focus_list_after_render(model, pane_id);
                    },
                    option{value:"none","없음"} option{value:"modified","수정 날짜"} option{value:"kind","종류"}
                }}
            }
                div{class:"header-spacer"}button{class:"selection-invert",title:"선택 반전 · Ctrl+I",disabled:p.loading||count==0,onclick:move |_|invert_selection(model,pane_id),"반전"}if let Some(error)=error{span{class:"pane-error",title:"{error}","{error}"}}}
        }
    }
}

#[component]
fn RecentLocations(model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let state = model.read();
    if !state.remember_activity || (state.recent_files.is_empty() && state.folder_visits.is_empty())
    {
        return rsx! {};
    }
    let recent = state
        .recent_files
        .iter()
        .take(6)
        .cloned()
        .collect::<Vec<_>>();
    let mut frequent = state.folder_visits.clone();
    frequent.sort_by_key(|entry| std::cmp::Reverse(entry.count));
    rsx! { details {class:"sidebar-section recent-locations",
        summary {"최근 항목"}
        if !frequent.is_empty() {
            details {class:"recent-group recent-folder-group",open:true,
                summary {title:"자주 연 폴더","폴더"}
                div {class:"recent-group-items",
                    for entry in frequent.into_iter().take(6) { {
                        let full = entry.path.display();
                        let name = entry.path.name();
                        let bridge = bridge.clone();
                        rsx! { button {key:"directory-{full}",class:"side-link recent-folder",title:"{full}",
                            onclick:move |_|navigate(model,&bridge,active_pane(model),entry.path.clone()),
                            Icon {kind:"folder"} span {"{name}"}
                        }}
                    } }
                }
            }
        }
        if !recent.is_empty() {
            details {class:"recent-group recent-file-group",open:true,
                summary {"파일"}
                div {class:"recent-group-items",
                    for path in recent { {
                        let full = path.display();
                        let name = path.name();
                        let bridge = bridge.clone();
                        rsx! { button {key:"file-{full}",class:"side-link recent-file",title:"{full}",
                            onclick:move |_|{let _=bridge.job(Request::Open{path:path.clone()});},
                            Icon {kind:"file"} span {"{name}"}
                        }}
                    } }
                }
            }
        }
    }}
}

#[component]
fn Workspaces(mut model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let mut name = use_signal(String::new);
    let s = model.read();
    let workspaces = s.workspaces.clone();
    let busy = s.job.is_some();
    let clean_name = settings::workspace_name(&name.read());
    let existing = workspaces.iter().any(|w| w.name == clean_name);
    let disabled = clean_name.is_empty() || (workspaces.len() >= 16 && !existing);
    drop(s);
    rsx! {
        div{class:"modal-backdrop",onkeydown:move|e:KeyboardEvent|{e.stop_propagation();if e.key()==Key::Escape{model.write().workspaces_open=false;}},
            div{class:"modal workspaces-dialog",role:"dialog","aria-modal":"true","aria-label":"작업공간 저장·전환",
                div{class:"preferences-heading",h2{"작업공간"}button{class:"icon-button",title:"작업공간 닫기",onclick:move |_|model.write().workspaces_open=false,"×"}}
                p{"분할·그룹 이름·탭 경로와 선택 위치를 함께 저장합니다. 현재 배치가 필요하면 전환 전에 저장해 주세요."}
                div{class:"workspace-save-row",
                    input{class:"dialog-input workspace-name","data-ime-commit":"true","aria-label":"작업공간 이름",placeholder:"예: 업무용 · 개발용",maxlength:64,initial_value:"{name}",onchange:move|e|name.set(e.value())}
                    button{class:"primary workspace-save",disabled,onclick:move |_|{
                        let title = settings::workspace_name(&name.read());
                        if title.is_empty(){return;}
                        let mut s=model.write();
                        let snapshot=s.settings().workspace(title.clone());
                        if let Some(index)=s.workspaces.iter().position(|w|w.name==title){s.workspaces[index]=snapshot;}
                        else if s.workspaces.len()<16{s.workspaces.push(snapshot);}else{return;}
                        s.notice=format!("‘{title}’ 작업공간을 저장했습니다.");s.dirty();
                    },if existing{"현재 배치로 갱신"}else{"현재 배치 저장"}}
                }
                p{class:"workspace-note","최대 16개 · 같은 이름으로 저장하면 기존 배치를 갱신합니다."}
                div{class:"saved-workspaces",
                    if workspaces.is_empty(){p{"저장한 작업공간이 없습니다."}}
                    for (i,workspace) in workspaces.iter().enumerate(){
                        div{class:"saved-workspace","data-workspace":i,
                            button{class:"open-workspace",disabled:busy,title:"저장한 배치 불러오기",onclick:{let b=bridge.clone();move |_|open_workspace(model,&b,i)},strong{"{workspace.name}"}span{"{workspace.layout}분할 · {workspace.pane_arrangement.layout(workspace.layout).name}"}}
                            button{class:"delete-workspace",title:"작업공간 기록 삭제",onclick:move |_|{let mut s=model.write();s.workspaces.remove(i);s.dirty();},"삭제"}
                        }
                    }
                }
                p{class:"workspace-note","탭: Ctrl+T 새 탭 · Ctrl+W 닫기 · Ctrl+Shift+T 복원 · Ctrl+Tab 전환"}
            }
        }
    }
}

#[component]
fn Preferences(mut model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let inventory_result =
        use_signal(|| None::<Result<sdir::live_search::MountedInventory, String>>);
    use_future(move || {
        let mut inventory_result = inventory_result;
        async move {
            if !cfg!(windows) {
                return;
            }
            let result = tokio::task::spawn_blocking(sdir::live_search::mounted_inventory)
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            inventory_result.set(Some(result));
        }
    });
    let inventory_state = inventory_result.read().clone();
    let s = model.read();
    let theme = s.theme;
    let compact = s.compact;
    let hidden = s.show_hidden;
    let independent = try_consume_context::<IndependentWindow>().is_some();
    let close_to_tray = s.close_to_tray;
    let start_on_login = s.start_on_login;
    let remember_activity = s.remember_activity;
    let search = s.search_preferences.clone();
    let memory_mib = search.memory_mib;
    let indexing = s.ntfs_indexing.clone();
    let index_status = s.index.clone();
    let index_root = index_status
        .as_ref()
        .map(|status| status.root.display())
        .unwrap_or_else(|| s.path().display());
    let index_dirty = s.index_dirty;
    let spotlight = s.spotlight;
    let index_error = s.index_error.clone();
    let live_status = s
        .index
        .as_ref()
        .filter(|status| s.search_live && status.host_pid == s.host_pid && s.host_pid.is_some())
        .cloned();
    let search_live = s.search_live;
    let index_starting = s.index_starting;
    let host_pid = s.host_pid;
    let config_pending = s.config_pending.is_some();
    let rebuild_pending = s.pending_rebuild.is_some();
    drop(s);
    let drive_controls_disabled = config_pending
        || rebuild_pending
        || (search_live && (index_starting || host_pid.is_none()));
    rsx! {
        div{class:"modal-backdrop",div{class:"modal preferences",role:"dialog","aria-modal":"true","aria-label":"사용자 설정",
            div{class:"preferences-heading",h2{"사용자 설정"}button{class:"icon-button",title:"설정 닫기",onclick:move |_|model.write().preferences_open=false,"×"}}
            if !independent {
            p{class:"preference-note","변경한 설정은 바로 적용하고 자동 저장합니다."}
            if cfg!(windows) {
            div{class:"preference-section",h3{"창과 시작"}
                label{class:"preference-option",span{"X로 닫을 때 트레이에 유지"}
                    input{class:"preferences-close-to-tray",r#type:"checkbox",checked:close_to_tray,
                        onchange:move|e:FormEvent|{let mut state=model.write();state.close_to_tray=e.checked();state.save_due=Some(Instant::now());}}
                }
                label{class:"preference-option",span{"Windows 로그인 시 실행"}
                    input{class:"preferences-login-start",r#type:"checkbox",checked:start_on_login,
                        onchange:move|e:FormEvent|{
                            let enabled=e.checked();
                            let result=lifecycle::set_login_start(enabled);
                            let mut state=model.write();
                            match result {
                                Ok(())=>{state.start_on_login=enabled;state.save_due=Some(Instant::now());}
                                Err(error)=>state.notice=error,
                            }
                        }}
                }
            }
            }
            }
            div{class:"preference-section",h3{"화면 테마"}
                div{class:"theme-choices",for (value,label) in [(Theme::System,"시스템"),(Theme::Light,"밝게"),(Theme::Dark,"어둡게")]{
                    button{class:if theme==value{"theme-choice chosen"}else{"theme-choice"},"data-theme-choice":value.value(),"aria-pressed":theme==value,onclick:move |_|{let mut s=model.write();s.theme=value;s.save_due=Some(Instant::now());},"{label}"}
                }}
            }
            div{class:"preference-section",h3{"파일 목록"}
                label{class:"preference-option",span{"최근 파일과 자주 쓰는 폴더 기록"}input{class:"preferences-recent",r#type:"checkbox",checked:remember_activity,onchange:move|e:FormEvent|{
                    let mut s=model.write();
                    s.remember_activity=e.checked();
                    s.visited=Default::default();
                    s.dirty();
                }}}
                button{class:"preferences-clear-recent",onclick:move |_|{
                    let mut s=model.write();
                    s.recent_files.clear();s.folder_visits.clear();s.dirty();
                },"최근 기록 지우기"}
                label{class:"preference-option",span{"목록 간격"}select{class:"density-select","aria-label":"목록 간격",value:if compact{"compact"}else{"comfortable"},onchange:move|e:FormEvent|{let mut s=model.write();s.compact=e.value()=="compact";for pane in &mut s.panes {for tab in &mut pane.tabs {tab.scroll=0.;}}s.save_due=Some(Instant::now());},option{value:"compact","촘촘하게"}option{value:"comfortable","여유 있게"}}}
                label{class:"preference-option",span{"파일 확장명 표시"}input{class:"preferences-extensions",r#type:"checkbox",checked:model.read().show_extensions,onchange:move|e:FormEvent|{let mut s=model.write();s.show_extensions=e.checked();s.dirty();}}}
                label{class:"preference-option",span{"항목 선택 체크박스"}input{class:"preferences-selection-checkboxes",r#type:"checkbox",checked:model.read().selection_checkboxes,onchange:move|e:FormEvent|{let mut s=model.write();s.selection_checkboxes=e.checked();s.dirty();}}}
                label{class:"preference-option",span{"숨김 파일 표시"}input{class:"preferences-hidden",r#type:"checkbox",checked:hidden,onchange:{let b=bridge.clone();move|e:FormEvent|set_hidden(model,&b,e.checked())}}}
            }
            div{class:"preference-section",h3{"통합 검색"}
                div{class:"search-index-strip",
                    div{class:"search-index-status",div{class:"index-root",title:"{index_root}","색인 범위: {index_root}"}
                        if spotlight {p{class:"index-count","macOS Spotlight 색인 사용"}}
                        else if let Some(ref index)=index_status{p{class:"index-count",if index.complete{if search_live{"실시간 추적"}else if index_dirty{"다시 색인 필요"}else{"색인 완료"}}else{"색인 중"} " · {index.entries}개"}}
                        else if index_starting{p{class:"index-count","색인을 시작하고 있습니다…"}}
                        else{p{class:"index-count","색인 없음"}}
                    }
                    div{class:"index-actions",button{class:"primary",onclick:{let b=bridge.clone();move |_|start_index(model,&b)},if index_status.is_some(){"현재 폴더로 다시 색인"}else{"현재 폴더 색인"}}
                        if cfg!(windows) {button{class:"volume-index-start",disabled:independent,onclick:{let b=bridge.clone();move |_|start_volume_index(model,&b,memory_mib,false)},"전체 드라이브 실시간 검색"}}
                        {macos_search_button(model, bridge.clone(), spotlight)}
                        if search_live||index_starting||index_status.as_ref().is_some_and(|status|!status.complete){button{class:"index-stop",onclick:{let b=bridge.clone();move |_|{b.search.cancel();let mut s=model.write();s.index_id+=1;s.query_id+=1;s.index=None;s.index_starting=false;s.search_live=false;s.spotlight=false;s.resume_live_index=false;s.save_due=Some(Instant::now());s.config_generation=0;s.host_pid=None;s.config_pending=None;s.pending_rebuild=None;s.query_loading=false;s.query_due=None;s.result=None;s.index_error=Some("색인을 취소했습니다.".into());}},"중지"}}
                    }
                    if let Some(error)=index_error.as_ref(){p{class:"search-error index-error","{error}"}}
                    if index_status.as_ref().is_some_and(|index|index.limited){p{class:"index-warning","색인 상한에 도달했습니다."}}
                }
                label{class:"preference-option",span{"입력 즉시 검색"}input{class:"preferences-search-instant",r#type:"checkbox",checked:search.instant,onchange:move|e:FormEvent|{let mut s=model.write();let mut value=s.search_preferences.clone();value.instant=e.checked();s.set_search_preferences(value);}}}
                label{class:"preference-option",span{"검색창에 초점이 들어오면 전체 선택"}input{class:"preferences-search-select",r#type:"checkbox",checked:search.select_on_focus,onchange:move|e:FormEvent|{let mut s=model.write();let mut value=s.search_preferences.clone();value.select_on_focus=e.checked();s.set_search_preferences(value);}}}
                label{class:"preference-option",span{"검색 패널을 열 때 검색창으로 이동"}input{class:"preferences-search-focus",r#type:"checkbox",checked:search.focus_on_open,onchange:move|e:FormEvent|{let mut s=model.write();let mut value=s.search_preferences.clone();value.focus_on_open=e.checked();s.set_search_preferences(value);}}}
                if cfg!(windows) {
                label{class:"preference-option",span{"전체 검색 색인 메모리 상한"}select{class:"preferences-search-memory density-select","aria-label":"전체 검색 색인 메모리 상한",value:search.memory_mib.to_string(),onchange:move|e:FormEvent|{if let Ok(n)=e.value().parse::<u16>(){let mut s=model.write();let mut value=s.search_preferences.clone();value.memory_mib=n;s.set_search_preferences(value);}},option{value:"256","256 MiB"}option{value:"512","512 MiB"}option{value:"1024","1 GiB"}option{value:"2048","2 GiB"}}}
                p{class:"preference-note","입력 즉시 검색을 끄면 Enter 또는 검색 버튼으로 실행합니다. Ctrl+Shift+F는 항상 검색창으로 이동합니다. 메모리 상한은 검색을 다시 시작할 때 적용합니다."}
                }
                button{class:"search-preferences-reset",onclick:move |_|model.write().set_search_preferences(Default::default()),"검색 설정 초기화"}
                if cfg!(windows) { details{class:"drive-index-settings",
                    summary{"드라이브 색인"}
                    p{class:"preference-note","드라이브 정책은 저장 버튼을 눌러 적용합니다. 실시간 색인 중에는 검색 프로세스의 확인을 받은 뒤 적용합니다. 경로 범위는 색인된 드라이브 안에서 검색 결과를 필터링합니다."}
                    div{class:"drive-index-auto",
                        label{class:"preference-option",span{"고정 드라이브 자동 포함"}input{class:"drive-index-auto-fixed",r#type:"checkbox",checked:indexing.auto_fixed(),disabled:drive_controls_disabled,onchange:{let b=bridge.clone();move|e:FormEvent|configure_auto_volumes(model,&b,Some(e.checked()),None)}}}
                        label{class:"preference-option",span{"이동식 드라이브 자동 포함"}input{class:"drive-index-auto-removable",r#type:"checkbox",checked:indexing.auto_removable(),disabled:drive_controls_disabled,onchange:{let b=bridge.clone();move|e:FormEvent|configure_auto_volumes(model,&b,None,Some(e.checked()))}}}
                    }
                    label{class:"preference-option",span{"Windows 구성 요소(WinSxS) 표시"}
                        input{class:"drive-index-show-windows-components",r#type:"checkbox",checked:indexing.show_windows_components(),disabled:drive_controls_disabled,
                            onchange:{let b=bridge.clone();move |e:FormEvent|{
                                let mut settings=model.read().ntfs_indexing.clone();
                                match settings.set_show_windows_components(e.checked()){
                                    Ok(())=>apply_index_settings(model,&b,settings),
                                    Err(error)=>model.write().index_error=Some(error.to_string()),
                                }
                            }}
                        }
                    }
                    p{class:"preference-note","전체 드라이브 실시간 색인은 SDIR 색인 서비스 또는 관리자 승인을 사용합니다. 비NTFS 드라이브와 네트워크 위치는 폴더 색인을 이용하세요."}
                    if let Some(ref index)=live_status{
                        p{class:"preference-note","색인 항목 {index.entries}개 · 메모리 {readable_size(Some(index.allocated_bytes))} · 접근 불가·제외 {index.skipped}개"}
                    }
                    if config_pending{p{class:"preference-note","드라이브 설정을 적용하고 있습니다."}}
                    if rebuild_pending{p{class:"preference-note","선택한 드라이브를 다시 색인하고 있습니다."}}
                    if let Some(Ok(inventory))=inventory_state.as_ref(){
                        if inventory.volumes.is_empty() && inventory.network_drives.is_empty(){p{class:"preference-note","연결된 드라이브가 없습니다."}}
                        div{class:"drive-index-list",
                            {inventory.volumes.iter().map(|volume| {
                                let guid=volume.guid.clone();
                                let mount_label=volume.mounts.iter().map(|mount|mount.display()).collect::<Vec<_>>().join(", ");
                                let support_label=match volume.classification{
                                    sdir::live_search::InventoryClass::Fixed=>"고정 · 지원",
                                    sdir::live_search::InventoryClass::Removable=>"이동식 · 지원",
                                    sdir::live_search::InventoryClass::Unsupported=>"미지원",
                                    sdir::live_search::InventoryClass::Unavailable=>"사용 불가",
                                };
                                let policy=indexing.volume_policy(&guid);
                                let scope=policy.map_or_else(sdir::index_policy::ScopePolicy::default,|policy|policy.scope().clone());
                                let mode=policy.map_or(sdir::index_policy::VolumeOverride::Auto,|policy|policy.mode());
                                let monitor=policy.map_or(true,|policy|policy.monitor());
                                let runtime=live_status.as_ref().and_then(|status|status.volumes.iter().find(|item|item.guid.as_ref()==Some(&guid)));
                                let runtime_label=runtime.map_or_else(||if search_live{"현재 세션에 없음".to_string()}else{"실시간 색인 미실행".to_string()},|item|format!("{} · {}개",item.phase.label(),item.entries));
                                let supported=matches!(volume.classification,sdir::live_search::InventoryClass::Fixed|sdir::live_search::InventoryClass::Removable);
                                let editable=supported&&!volume.mounts.is_empty();
                                let can_rebuild=editable&&runtime.is_some_and(|item|matches!(&item.phase,sdir::live_search::Phase::Live|sdir::live_search::Phase::Snapshot|sdir::live_search::Phase::Limited|sdir::live_search::Phase::Error));
                                let volume_disabled=drive_controls_disabled||!editable;
                                rsx!{div{key:"{guid.as_str()}",class:"drive-index-entry","data-volume-guid":"{guid.as_str()}",
                                    "data-phase":runtime.map(|item|format!("{:?}",item.phase).to_lowercase()).unwrap_or_default(),
                                    div{class:"drive-index-entry-heading",strong{if mount_label.is_empty(){"마운트 경로 없음"}else{"{mount_label}"}},span{"{support_label} · {runtime_label}"}}
                                    if let Some(message)=runtime.and_then(|item|item.message.as_ref()).or(volume.message.as_ref()){p{class:"preference-note","{message}"}}
                                    div{class:"drive-index-policy",
                                        label{class:"preference-option",span{"포함 설정"}select{"aria-label":"{mount_label} 드라이브 색인 정책",value:match mode{sdir::index_policy::VolumeOverride::Auto=>"auto",sdir::index_policy::VolumeOverride::Include=>"include",sdir::index_policy::VolumeOverride::Exclude=>"exclude"},disabled:volume_disabled,onchange:{let b=bridge.clone();let guid=guid.clone();move|e:FormEvent|{let mode=match e.value().as_str(){"auto"=>Some(sdir::index_policy::VolumeOverride::Auto),"include"=>Some(sdir::index_policy::VolumeOverride::Include),"exclude"=>Some(sdir::index_policy::VolumeOverride::Exclude),_=>None};if let Some(mode)=mode{configure_volume(model,&b,guid.clone(),Some(mode),None,None);}}},option{value:"auto","자동"}option{value:"include","포함"}option{value:"exclude","제외"}}}
                                        label{class:"preference-option",span{"실시간 모니터링"}input{r#type:"checkbox",checked:monitor,disabled:volume_disabled,onchange:{let b=bridge.clone();let guid=guid.clone();move|e:FormEvent|configure_volume(model,&b,guid.clone(),None,Some(e.checked()),None)}}}
                                        if can_rebuild{button{class:"volume-rebuild",disabled:drive_controls_disabled||host_pid.is_none(),onclick:{let b=bridge.clone();let guid=guid.clone();move |_|rebuild_volume(model,&b,guid.clone())},"다시 색인"}}
                                    }
                                    VolumeScopeEditor{model,guid:guid.clone(),scope:scope.clone(),disabled:volume_disabled}
                                }}
                            })}
                            {indexing.volume_policies().iter().filter(|policy|{
                                !inventory.volumes.iter().any(|volume|volume.guid.as_str()==policy.guid().as_str())
                            }).map(|policy|{
                                let guid=policy.guid().clone();
                                let scope=policy.scope().clone();
                                let mode_label=match policy.mode(){
                                    sdir::index_policy::VolumeOverride::Auto=>"자동",
                                    sdir::index_policy::VolumeOverride::Include=>"포함",
                                    sdir::index_policy::VolumeOverride::Exclude=>"제외",
                                };
                                let monitor_label=if policy.monitor(){"켜짐"}else{"꺼짐"};
                                rsx!{div{key:"offline-{guid.as_str()}",class:"drive-index-entry drive-index-offline-entry","data-volume-guid":"{guid.as_str()}",
                                    div{class:"drive-index-entry-heading",strong{"저장된 드라이브 정책"}}
                                    div{class:"volume-offline-guid",code{"{guid.as_str()}"},span{"연결되지 않음"}}
                                    p{class:"preference-note","정책: {mode_label} · 실시간 모니터링 {monitor_label}"}
                                    VolumeScopeEditor{model,guid:guid.clone(),scope,disabled:drive_controls_disabled}
                                }}
                            })}
                            for path in inventory.network_drives.iter(){div{class:"drive-index-entry",strong{"{path.display()}"}span{"네트워크 · 미지원"}}}
                        }
                    }else if let Some(Err(error))=inventory_state.as_ref(){p{class:"search-error index-error","드라이브 목록을 불러오지 못했습니다: {error}"}}
                    else{p{class:"preference-note","드라이브 목록을 불러오는 중…"}}
                }
            }
            }
            div{class:"preference-section",h3{"패널 그룹"}p{class:"preference-note","각 패널 맨 위의 이름을 눌러 업무용·제안용·개발용처럼 지정하세요. 그룹 안에 여러 폴더 탭을 둘 수 있습니다."}}
            div{class:"modal-actions",button{class:"primary",onclick:move |_|model.write().preferences_open=false,"닫기"}}
        }}
    }
}

#[cfg(target_os = "macos")]
fn macos_search_button(model: Signal<AppModel>, bridge: Arc<Bridge>, spotlight: bool) -> Element {
    rsx! { button { class: "spotlight-start", disabled: spotlight, onclick: move |_| start_spotlight(model, &bridge), "Spotlight 검색" } }
}

#[cfg(windows)]
fn macos_search_button(_: Signal<AppModel>, _: Arc<Bridge>, _: bool) -> Element {
    VNode::empty()
}

#[component]
fn SearchPanel(mut model: Signal<AppModel>) -> Element {
    let initial_query = use_hook(move || model.peek().query.clone());
    let spotlight = model.read().spotlight;
    let bridge = use_context::<Arc<Bridge>>();
    let s = model.read();
    let index_host = s
        .index
        .as_ref()
        .and_then(|status| status.host_pid)
        .map(|pid| pid.to_string())
        .unwrap_or_default();
    let instant = s.search_preferences.instant;
    let query_loading = s.query_loading;
    let result = s.result.clone();
    let error = s.search_error.clone();
    let expanded = s.search_expanded;
    let target = s
        .search_target
        .map_or_else(|| "active".to_string(), |pane| pane.to_string());
    let pane_names = s.pane_names[..s.layout].to_vec();
    let expand_label = if expanded {
        "검색 화면 축소"
    } else {
        "검색 화면 확대"
    };
    drop(s);
    rsx! {
        aside{class:if expanded{"search-panel search-expanded"}else{"search-panel"},"data-index-host":"{index_host}",
            if !expanded{
                div{class:"search-resizer",role:"separator","aria-label":"검색창 너비 조절","aria-orientation":"vertical",tabindex:"0"}
            }
            div{class:"search-heading",Icon{kind:"search"}h2{if spotlight {"Spotlight 파일 검색"} else {"내장 파일 검색"}}
                button{class:"search-expand",title:"{expand_label}","aria-label":"{expand_label}","aria-pressed":expanded,
                    onclick:move |_|{let mut s=model.write();s.search_expanded=!s.search_expanded;},if expanded{"축소"}else{"확대"}}
                button{class:"icon-button",title:"검색 닫기","aria-label":"검색 닫기",onclick:move |_|{let mut s=model.write();s.search_open=false;s.search_expanded=false;},"×"}
            }
            div{class:"search-input-wrap",Icon{kind:"search"}input{id:"global-query","data-ime-commit":"true",onmounted:move |_|{let focus={let s=model.read();s.search_focus==Some(s.interaction)&&s.search_open&&!s.modal_open()};if focus{model.write().search_focus=None;let _=document::eval("window.sdirFocusSearch?.()");}},onkeydown:{let b=bridge.clone();move|e:KeyboardEvent|{e.stop_propagation();if e.is_composing(){return;}if pane_shortcut(&e,model,active_pane(model),&b){return;}if e.key()==Key::Enter{model.write().submit_query();}}},placeholder:if instant{"파일명, ext:pdf, *.txt"}else{"입력 후 Enter로 검색"},initial_value:"{initial_query}",spellcheck:false,onchange:move|e|{let value=e.value();if model.peek().query!=value{model.write().edit_query(value);}}}button{class:"search-submit",title:"검색 실행 (Enter)",onclick:move |_|model.write().submit_query(),"검색"}}
            if pane_names.len()>1{
                label{class:"search-target",span{"결과를 열 패널"}
                    select{class:"search-target-select",value:"{target}",onchange:move|e:FormEvent|{
                        let mut s=model.write();
                        s.search_target=e.value().parse::<usize>().ok().filter(|&pane|pane<s.layout);
                    },
                        option{value:"active","현재 패널"}
                        for (pane, name) in pane_names.iter().enumerate(){
                            {
                                let number = format!("패널 {}", pane + 1);
                                let label = if name == &number { number } else { format!("{number} · {name}") };
                                rsx!{option{value:"{pane}","{label}"}}
                            }
                        }
                    }
                }
            }
            if !instant{p{class:"manual-search-hint","Enter 또는 검색 버튼을 눌러 검색합니다."}}
            if let Some(error)=error{p{class:"search-error","{error}"}}
            if query_loading{div{class:"query-wait",span{class:"mini-spinner"}span{"검색 중"}}}
            if let Some(ref results)=result{div{class:"results-summary",strong{"{results.total}개 결과"}span{"{results.elapsed_ms:.1}ms"}}
                if !results.complete{p{class:"partial-results","일부 범위의 검색 결과입니다."}}
                div{class:"search-results",for hit in results.hits.iter(){ { let hit=hit.clone();let path=hit.path.clone();let name=hit.name.clone();let full=path.display();let is_dir=hit.is_dir;let appearance=if is_dir{None}else{model.read().file_appearance.for_type(&name)}; rsx! { button{class:"search-result",title:"{full}",onclick:{let b=bridge.clone();move |_|reveal_hit(model,&b,hit.clone())},{file_icon(is_dir, appearance.as_deref(), false)}div{strong{"{name}"}span{"{full}"}}} } } }
                }
                if results.total>200{p{class:"snapshot-note","처음 200개 결과를 표시합니다. 검색어를 더 입력해 범위를 줄이세요."}}
            }
        }
    }
}

#[component]
fn Icon(kind: String) -> Element {
    let path = match kind.as_str() {
        "settings" => {
            "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8M9 3h6l1 4 4 1v8l-4 1-1 4H9l-1-4-4-1V8l4-1z"
        }
        "sidebar-collapse" => "M3 5h18v14H3zM9 5v14M16 9l-3 3 3 3",
        "sidebar-expand" => "M3 5h18v14H3zM9 5v14M13 9l3 3-3 3",
        "folder" => "M3 5h6l2 2h10v12H3z",
        "home" => "M3 11l9-8 9 8M5 10v11h5v-7h4v7h5V10",
        "search" => "M20 20l-5-5M17 10a7 7 0 1 1-14 0 7 7 0 0 1 14 0",
        "drive" => "M4 4h16l2 14H2L4 4zM2 14h20M17 17h2",
        "star" => "M12 3l3 6 7 1-5 5 1 7-6-3-6 3 1-7-5-5 7-1z",
        "plus" => "M12 5v14M5 12h14",
        "copy" => "M8 8h12v13H8zM16 8V3H3v13h5",
        "cut" => {
            "M4 4l16 16M4 20L20 4M8 17a3 3 0 1 1-6 0 3 3 0 0 1 6 0M8 7a3 3 0 1 1-6 0 3 3 0 0 1 6 0"
        }
        "paste" => "M9 4H5v18h14V4h-4M9 2h6v5H9z",
        "rename" => "M4 17l-1 4 4-1L20 7l-4-4L4 17zM14 5l4 4",
        "trash" => "M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7",
        "arrow" => "M3 12h18M15 6l6 6-6 6",
        "filter" => "M3 5h18l-7 8v7l-4-2v-5L3 5z",
        _ => "M5 2h9l5 5v15H5zM14 2v6h5M8 12h8M8 16h6",
    };
    rsx! {svg{class:format!("icon icon-{kind}"),view_box:"0 0 24 24",fill:"none",stroke:"currentColor",stroke_width:"1.6",stroke_linecap:"round",stroke_linejoin:"round","aria-hidden":"true",path{d:path}}}
}

fn tree_toggle(mut model: Signal<AppModel>, bridge: &Bridge, path: NativePath, retry: bool) {
    let path = sdir::tree::tree_key(path);
    {
        let mut s = model.write();
        s.tree_tasks.clear();
        s.tree_pending_auto = false;
        s.dirty();
    }
    let (request, cancel) = {
        let mut s = model.write();
        let expanded = s.tree.nodes.get(&path).is_some_and(|n| n.expanded);
        if expanded && !retry {
            s.tree.collapse(&path);
            (None, s.tree.pending.is_none())
        } else {
            let hidden = s.show_hidden;
            let request = s
                .tree
                .begin(&path)
                .map(|id| Request::Tree { id, path, hidden });
            (request, false)
        }
    };
    if cancel {
        bridge.tree.cancel();
    }
    if let Some(request) = request {
        bridge.tree.replace(request);
    }
}
#[component]
fn DirectoryTree(mut model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let mut scroll = use_signal(|| 0f64);
    let s = model.read();
    let rows = s.tree.rows();
    let open = s.tree.open;
    let follow = s.tree_follow;
    let active_path = sdir::tree::tree_key(s.path());
    let cursor = sdir::tree::tree_key(s.tree_cursor.clone().unwrap_or_else(|| active_path.clone()));
    let focused_path = cursor.display();
    let active_descendant = rows
        .iter()
        .position(|r| r.path == cursor && r.message.is_none())
        .map(|i| format!("tree-node-{i}"))
        .unwrap_or_default();
    let tree_busy = s.tree.pending.is_some() || !s.tree_tasks.is_empty();
    let nodes = &s.tree.nodes;
    let start = ((*scroll.read() / 28.) as usize)
        .saturating_sub(4)
        .min(rows.len());
    let end = (start + 60).min(rows.len());
    let height = rows.len() * 28;
    rsx! {
        div{class:"sidebar-section tree-section",
            div{class:"section-label",span{"폴더 트리"}button{class:"tree-refresh",title:"폴더 트리 새로고침",onclick:{let b=bridge.clone();move |_|{b.tree.cancel();model.write().tree.reset();model.write().tree_seen=None;model.write().dirty();scroll.set(0.);let _=document::eval("document.querySelector('.drive-tree')?.scrollTo(0,0)");}},"↻"}}
            label{class:"tree-follow",input{r#type:"checkbox",checked:follow,onchange:move|e:FormEvent|{let mut s=model.write();s.tree_follow=e.checked();s.tree_seen=None;s.tree_tasks.clear();s.dirty();}}span{title:"현재 위치 따라가기","현재 위치 따라가기"}}
            button{class:"tree-computer", "aria-expanded":open,onclick:{let b=bridge.clone();move |_|{let mut s=model.write();s.tree.open=!s.tree.open;s.dirty();if !s.tree.open{b.tree.cancel();s.tree.reset();s.tree_tasks.clear();}else{s.tree_seen=None;}}},span{if open{"⌄"}else{"›"}}Icon{kind:"drive"}span{"내 PC"}}
            if open {
                div{class:"drive-tree",style:"height:min({height}px,clamp(112px,30vh,340px))",role:"tree","aria-label":"드라이브와 폴더",tabindex:0,"data-focused-path":"{focused_path}","aria-activedescendant":"{active_descendant}","aria-busy":tree_busy,onkeydown:{let b=bridge.clone();move|e:KeyboardEvent|{let (path,expanded)={let s=model.read();let p=sdir::tree::tree_key(s.tree_cursor.clone().unwrap_or_else(||s.path()));let expanded=s.tree.nodes.get(&p).is_some_and(|n|n.expanded);(p,expanded)};tree_key(e,model,&b,path,expanded);}},onscroll:move|e:ScrollEvent|scroll.set(e.scroll_top()),
                    div{class:"tree-space",style:"height:{height}px",
                        div{class:"tree-rows",style:"transform:translateY({start*28}px)",
                            for (offset,row) in rows[start..end].iter().enumerate() { {
                                let path=sdir::tree::tree_key(row.path.clone());let full=path.display();let depth=row.depth;let indent=depth.min(16)*13;
                                let node=nodes.get(&path).cloned().unwrap_or_default();let expanded=node.expanded;let empty=node.loaded&&node.children.is_empty()&&node.message.is_none();let loading=s.tree.pending.as_ref().is_some_and(|(_,p)|p==&path);
                                let selected=path==active_path;let keyboard=path==cursor;let row_id=format!("tree-node-{}",start+offset);let row_class=format!("tree-row{}{}",if selected{" selected"}else{""},if keyboard{" keyboard-focused"}else{""});let name=if depth==0{format!("드라이브 ({full})")}else{path.name()};let label=if expanded{"접기"}else{"펼치기"};
                                if let Some(message)=&row.message {
                                        rsx! { div{key:"message-{full}",class:"tree-message",style:"padding-left:min({indent+23}px, 45%)",title:"{message}",span{"{message}"}
                                            if !loading&&node.message.is_some(){button{class:"tree-retry",title:"{full} 다시 시도",onclick:{let b=bridge.clone();let p=path.clone();move |_|tree_toggle(model,&b,p.clone(),true)},"다시 시도"}}
                                        }
                                        }
                                    }else{
                                        rsx! { div{key:"node-{full}",class:row_class,id:row_id,role:"treeitem","aria-level":depth+1,"aria-expanded":expanded,"aria-selected":selected,"aria-busy":loading,"data-tree-path":"{full}",style:"padding-left:min({indent}px, 35%)",tabindex:-1,onkeydown:{let b=bridge.clone();let p=path.clone();move|e:KeyboardEvent|tree_key(e,model,&b,p.clone(),expanded)},
                                            button{class:"tree-expander",tabindex:-1,title:"{full} {label}","aria-label":"{full} {label}",disabled:empty,onclick:{let b=bridge.clone();let p=path.clone();move |_|tree_toggle(model,&b,p.clone(),false)},if loading{"…"}else if empty{"·"}else if expanded{"⌄"}else{"›"}}
                                            button{class:"tree-label",tabindex:-1,title:"{full}",onclick:{let b=bridge.clone();let p=path.clone();move |_|tree_navigate(model,&b,p.clone())},Icon{kind:if depth==0{"drive"}else{"folder"}}span{"{name}"}}
                                        }
                                    }
                                }
                            } }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CopyDestinationDialog(mut model: Signal<AppModel>) -> Element {
    let bridge = use_context::<Arc<Bridge>>();
    let s = model.read();
    let Some(dialog) = s.copy_destination.clone() else {
        return rsx! {};
    };
    let selected_panel = dialog
        .destination
        .as_ref()
        .map(|(p, _, _)| p.to_string())
        .unwrap_or_default();
    let selected_tab = dialog
        .destination
        .as_ref()
        .map(|(_, t, _)| t.to_string())
        .unwrap_or_default();
    let choices: Vec<_> = (0..s.layout)
        .filter(|&p| p != dialog.origin.pane)
        .map(|p| (p, format!("패널 {} · {}", p + 1, s.pane_names[p])))
        .collect();
    let tabs: Vec<_> = dialog
        .destination
        .as_ref()
        .map(|(p, _, _)| {
            s.panes[*p]
                .tabs
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let current = if i == s.panes[*p].active {
                        " · 현재 열린 탭"
                    } else {
                        ""
                    };
                    (i, format!("탭 {} · {}{}", i + 1, t.path.name(), current))
                })
                .collect()
        })
        .unwrap_or_default();
    let source = format!(
        "패널 {} · {}",
        dialog.origin.pane + 1,
        s.pane_names[dialog.origin.pane]
    );
    let source_path = dialog.origin.path.display();
    let count = dialog.sources.len();
    let busy = s.job.is_some();
    let has_tabs = !tabs.is_empty();
    drop(s);
    rsx! {
        div{class:"modal-backdrop",onkeydown:move|e:KeyboardEvent|{e.stop_propagation();if e.key()==Key::Escape{close_copy_destination(model);}},
            div{class:"modal copy-destination-dialog",role:"dialog","aria-modal":"true","aria-label":"복사 대상 선택",
                h2{"복사 대상 선택"}
                div{class:"copy-source",strong{"{source}에서 {count}개 항목 복사"}p{title:"{source_path}","{source_path}"}}
                p{class:"copy-help","대상 패널을 고른 뒤 탭과 전체 경로를 확인하세요. 복사 버튼을 누르기 전에는 파일이 복사되지 않습니다."}
                label{class:"copy-field",span{"대상 패널"}select{class:"copy-target-panel","aria-label":"대상 패널",autofocus:true,value:"{selected_panel}",onchange:move|e:FormEvent|{if let Ok(p)=e.value().parse(){choose_copy_panel(model,p);}},option{value:"",disabled:true,"패널을 선택하세요"}for (index,name) in choices{option{value:"{index}","{name}"}}}}
                label{class:"copy-field",span{"대상 탭"}select{class:"copy-target-tab","aria-label":"대상 탭",disabled:!has_tabs,value:"{selected_tab}",onchange:move|e:FormEvent|{if let Ok(t)=e.value().parse(){choose_copy_tab(model,t);}},if !has_tabs{option{value:"","먼저 패널을 선택하세요"}}for (index,name) in tabs{option{value:"{index}","{name}"}}}}
                if let Some((pane,tab,path))=&dialog.destination {
                    div{class:"copy-destination-preview","data-destination-pane":*pane,"data-destination-tab":*tab,
                        strong{"패널 {pane+1} · 탭 {tab+1}에 복사"}
                        p{class:"copy-destination-path","{path.display()}"}
                    }
                }
                if let Some(error)=&dialog.error {p{class:"copy-destination-error",role:"alert","{error}"}}
                div{class:"modal-actions",button{class:"copy-destination-cancel",onclick:move |_|close_copy_destination(model),"취소"}button{class:"primary copy-destination-confirm",disabled:busy||dialog.destination.is_none(),onclick:{let b=bridge.clone();move |_|confirm_copy_destination(model,&b)},"선택한 폴더로 복사"}}
            }
        }
    }
}
