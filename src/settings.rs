use crate::model::{NativePath, Pane, Tab, TabState, ViewMode};
use crate::pane_arrangement::PaneArrangement;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub layout: usize,
    pub pane_arrangement: PaneArrangement,
    pub panes: Vec<Vec<NativePath>>,
    pub favorites: Vec<NativePath>,
    pub recent_files: Vec<NativePath>,
    pub folder_visits: Vec<FolderVisit>,
    pub remember_activity: bool,
    pub show_hidden: bool,
    pub show_extensions: bool,
    pub selection_checkboxes: bool,
    pub pane_names: Vec<String>,
    pub theme: Theme,
    pub compact: bool,
    pub action_bar_visible: bool,
    pub sidebar_visible: bool,
    pub sidebar_width: Option<u16>,
    pub search_preferences: crate::preferences::SearchPreferences,
    pub ntfs_indexing: crate::index_policy::NtfsIndexingSettings,
    pub resume_live_index: bool,
    pub close_to_tray: bool,
    pub start_on_login: bool,
    pub active_tabs: Vec<usize>,
    pub active_pane: usize,
    pub workspaces: Vec<Workspace>,
    pub pane_views: Vec<ViewMode>,
    pub pane_columns: Vec<crate::preferences::Columns>,
    pub pane_icon_sizes: Vec<u16>,
    pub tab_states: Vec<Vec<TabState>>,
    pub window: Option<WindowPlacement>,
    pub tree_expanded: Vec<NativePath>,
    pub tree_follow: bool,
    pub tree_open: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FolderVisit {
    pub path: NativePath,
    pub count: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}
/// Only paths and layout are retained; saved workspaces never keep file buffers or workers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub name: String,
    pub layout: usize,
    pub pane_arrangement: PaneArrangement,
    pub panes: Vec<Vec<NativePath>>,
    pub pane_names: Vec<String>,
    pub active_tabs: Vec<usize>,
    pub active_pane: usize,
    pub pane_views: Vec<ViewMode>,
    pub pane_columns: Vec<crate::preferences::Columns>,
    pub pane_icon_sizes: Vec<u16>,
    pub tab_states: Vec<Vec<TabState>>,
}
pub fn workspace_name(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect()
}
pub fn valid_layout(layout: usize) -> usize {
    if (1..=4).contains(&layout) { layout } else { 2 }
}
impl Workspace {
    pub fn restore_panes(&self, home: &NativePath) -> Vec<Pane> {
        (0..4)
            .map(|i| {
                let paths = self.panes.get(i).filter(|p| !p.is_empty());
                let mut pane = Pane::new(home.clone());
                pane.view = self.pane_views.get(i).copied().unwrap_or_default();
                pane.columns = self.pane_columns.get(i).cloned().unwrap_or_default();
                pane.columns.normalize();
                pane.icon_size = self
                    .pane_icon_sizes
                    .get(i)
                    .copied()
                    .unwrap_or(112)
                    .clamp(80, 192);
                if let Some(paths) = paths {
                    pane.tabs = paths.iter().take(16).cloned().map(Tab::new).collect();
                }
                if let Some(states) = self.tab_states.get(i) {
                    for (tab, state) in pane.tabs.iter_mut().zip(states) {
                        tab.resume_from(state);
                    }
                }
                pane.active = self
                    .active_tabs
                    .get(i)
                    .copied()
                    .unwrap_or(0)
                    .min(pane.tabs.len() - 1);
                pane.address = pane.tab().path.display();
                pane
            })
            .collect()
    }
}
impl Settings {
    pub fn workspace(&self, name: String) -> Workspace {
        Workspace {
            name,
            layout: self.layout,
            pane_arrangement: self.pane_arrangement,
            panes: self.panes.clone(),
            pane_names: self.pane_names.clone(),
            active_tabs: self.active_tabs.clone(),
            active_pane: self.active_pane,
            pane_views: self.pane_views.clone(),
            pane_columns: self.pane_columns.clone(),
            pane_icon_sizes: self.pane_icon_sizes.clone(),
            tab_states: self.tab_states.clone(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}
impl Theme {
    pub fn value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}
pub fn pane_name(value: &str, pane: usize) -> String {
    let value: String = value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    if value.is_empty() {
        format!("패널 {}", pane + 1)
    } else {
        value
    }
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            layout: 2,
            pane_arrangement: PaneArrangement::default(),
            panes: vec![],
            favorites: vec![],
            recent_files: vec![],
            folder_visits: vec![],
            remember_activity: true,
            show_hidden: false,
            show_extensions: true,
            selection_checkboxes: false,
            pane_names: vec![],
            theme: Theme::System,
            compact: true,
            action_bar_visible: true,
            sidebar_visible: true,
            sidebar_width: None,
            search_preferences: Default::default(),
            ntfs_indexing: Default::default(),
            resume_live_index: false,
            close_to_tray: false,
            start_on_login: false,
            active_tabs: vec![],
            active_pane: 0,
            workspaces: vec![],
            pane_views: vec![],
            pane_columns: vec![],
            pane_icon_sizes: vec![],
            tab_states: vec![],
            window: None,
            tree_expanded: vec![],
            tree_follow: true,
            tree_open: true,
        }
    }
}
#[cfg(target_os = "macos")]
fn legacy_data_dir() -> PathBuf {
    home().to_path().join("Library/Application Support/SDIR")
}

#[cfg(not(target_os = "macos"))]
fn legacy_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("SDIR")
}
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("SDIR_PROFILE_DIR") {
        return p.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            if parent.join("portable.flag").is_file() {
                return parent.join("data");
            }
        }
    }
    legacy_data_dir()
}
pub fn seed_portable(dir: &std::path::Path, legacy: &std::path::Path) -> Result<(), String> {
    if dir == legacy
        || dir.join("settings.json").exists()
        || !legacy.join("settings.json").is_file()
    {
        return Ok(());
    }
    let bytes = fs::read(legacy.join("settings.json")).map_err(|e| e.to_string())?;
    let settings: Settings = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    save_at(dir, &settings)
}
pub fn load_at(dir: &std::path::Path) -> Settings {
    fs::read(dir.join("settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .filter(|settings: &Settings| settings.version == 1)
        .unwrap_or_default()
}

pub fn load() -> Settings {
    let dir = data_dir();
    if std::env::var_os("SDIR_PROFILE_DIR").is_none() {
        let _ = seed_portable(&dir, &legacy_data_dir());
    }
    load_at(&dir)
}
pub fn save(settings: &Settings) -> Result<(), String> {
    save_at(&data_dir(), settings)
}
pub fn save_at(dir: &std::path::Path, settings: &Settings) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join("settings.json.tmp");
    let dest = dir.join("settings.json");
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // std::fs::rename replaces the destination on both Windows and Unix.
    fs::rename(tmp, dest).map_err(|e| e.to_string())
}
pub fn home() -> NativePath {
    NativePath::new(
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" })),
    )
}

/// A single writer coalesces pending preferences; disk delays cannot spawn more writers.
pub struct SettingsWriter {
    tx: crossbeam_channel::Sender<WriteRequest>,
    pending: crossbeam_channel::Receiver<WriteRequest>,
    pub errors: crossbeam_channel::Receiver<String>,
}
struct WriteRequest {
    settings: Settings,
    completions: Vec<crossbeam_channel::Sender<Result<(), String>>>,
}
impl SettingsWriter {
    pub fn new() -> Self {
        Self::at(data_dir())
    }
    fn at(dir: PathBuf) -> Self {
        let (tx, rx) = crossbeam_channel::bounded::<WriteRequest>(1);
        let (errors_tx, errors) = crossbeam_channel::bounded(1);
        let pending = rx.clone();
        std::thread::spawn(move || {
            while let Ok(mut request) = rx.recv() {
                for mut latest in rx.try_iter() {
                    request.settings = latest.settings;
                    request.completions.append(&mut latest.completions);
                }
                let result = save_at(&dir, &request.settings);
                if let Err(ref e) = result {
                    let _ = errors_tx.try_send(e.clone());
                }
                for completion in request.completions {
                    let _ = completion.send(result.clone());
                }
            }
        });
        Self {
            tx,
            pending,
            errors,
        }
    }
    pub fn submit(&self, settings: Settings) {
        self.enqueue(WriteRequest {
            settings,
            completions: vec![],
        });
    }
    pub fn flush(&self, settings: Settings) -> crossbeam_channel::Receiver<Result<(), String>> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.enqueue(WriteRequest {
            settings,
            completions: vec![tx],
        });
        rx
    }
    fn enqueue(&self, request: WriteRequest) {
        if let Err(crossbeam_channel::TrySendError::Full(mut request)) = self.tx.try_send(request) {
            if let Ok(mut old) = self.pending.try_recv() {
                request.completions.append(&mut old.completions);
            }
            if let Err(error) = self.tx.try_send(request) {
                let request = error.into_inner();
                for completion in request.completions {
                    let _ = completion.send(Err("설정 저장 대기열을 사용할 수 없습니다.".into()));
                }
            }
        }
    }
}
impl Default for SettingsWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_seed_preserves_legacy_and_never_overwrites_a_portable_profile() {
        let legacy = tempfile::tempdir().unwrap();
        let portable = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        s.layout = 4;
        save_at(legacy.path(), &s).unwrap();
        let original = fs::read(legacy.path().join("settings.json")).unwrap();
        seed_portable(portable.path(), legacy.path()).unwrap();
        assert_eq!(
            fs::read(portable.path().join("settings.json")).unwrap(),
            original
        );
        s.layout = 1;
        save_at(portable.path(), &s).unwrap();
        seed_portable(portable.path(), legacy.path()).unwrap();
        let got: Settings =
            serde_json::from_slice(&fs::read(portable.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(got.layout, 1);
        assert_eq!(
            fs::read(legacy.path().join("settings.json")).unwrap(),
            original
        );
    }
    #[test]
    fn workspace_round_trip_preserves_selected_paths_and_clamps_invalid_indices() {
        let home = NativePath::new("home");
        let mut s = Settings::default();
        s.layout = 4;
        s.panes = vec![
            vec![NativePath::new("first"), NativePath::new("active")],
            vec![],
        ];
        s.active_tabs = vec![1, 99];
        s.active_pane = 3;
        s.pane_names = vec!["업무용".into()];
        s.pane_views = vec![ViewMode::Icons, ViewMode::List];
        s.workspaces
            .push(s.workspace(workspace_name("  개발용 🛠  ")));
        let encoded = serde_json::to_vec(&s).unwrap();
        let decoded: Settings = serde_json::from_slice(&encoded).unwrap();
        let w = &decoded.workspaces[0];
        assert_eq!(w.name, "개발용 🛠");
        assert_eq!(w.active_pane, 3);
        assert_eq!(w.pane_names, vec!["업무용"]);
        let restored = w.restore_panes(&home);
        assert_eq!(restored[0].tab().path, NativePath::new("active"));
        assert_eq!(restored[0].view, ViewMode::Icons);
        assert_eq!(restored[1].view, ViewMode::List);
        assert_eq!(restored[2].view, ViewMode::Details);
        assert_eq!(restored[1].tab().path, home);
        assert_eq!(restored[3].active, 0);
        assert!(restored.iter().all(|p| !p.tab().loaded));
        let malformed = Workspace {
            layout: 99,
            panes: vec![vec![home.clone(); 30]],
            active_tabs: vec![500],
            ..Workspace::default()
        };
        assert_eq!(valid_layout(malformed.layout), 2);
        assert_eq!(malformed.restore_panes(&home)[0].tabs.len(), 16);
        assert_eq!(malformed.restore_panes(&home)[0].active, 15);
    }
    #[test]
    fn settings_replace_and_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        save_at(d.path(), &s).unwrap();
        s.layout = 4;
        s.favorites.push(NativePath::new("C:\\작업"));
        s.pane_names = vec![
            "업무용".into(),
            "제안용".into(),
            "개발용".into(),
            "자료".into(),
        ];
        s.theme = Theme::Dark;
        s.compact = false;
        save_at(d.path(), &s).unwrap();
        let got: Settings =
            serde_json::from_slice(&fs::read(d.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!(got.layout, 4);
        assert_eq!(got.favorites, s.favorites);
        assert_eq!(got.pane_names, s.pane_names);
        assert_eq!(got.theme, Theme::Dark);
        assert!(!got.compact);
    }
    #[test]
    fn legacy_settings_preserve_paths_with_default_preferences() {
        let old = r#"{"version":1,"layout":4,"panes":[[[67,58,92,65]]],"favorites":[],"show_hidden":true}"#;
        let settings: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.layout, 4);
        assert_eq!(settings.panes[0][0], NativePath::new("C:\\A"));
        assert!(settings.show_hidden && settings.compact);
        assert_eq!(settings.theme, Theme::System);
        assert_eq!(settings.search_preferences, Default::default());
        assert!(settings.ntfs_indexing.is_valid());
        assert!(settings.ntfs_indexing.auto_fixed());
        assert!(!settings.ntfs_indexing.auto_removable());
        assert_eq!(
            settings.ntfs_indexing.offline_policy(),
            crate::index_policy::OfflinePolicy::DropCache
        );
        assert!(!settings.ntfs_indexing.persistent());
        assert!(settings.workspaces.is_empty() && settings.active_tabs.is_empty());
        assert_eq!(settings.active_pane, 0);
        assert_eq!(pane_name("", 2), "패널 3");
        assert_eq!(pane_name("  개발용  ", 2), "개발용");
    }
    #[test]
    fn search_preferences_round_trip_and_partial_settings_keep_safe_defaults() {
        let mut s: Settings = serde_json::from_str(
            r#"{"layout":4,"panes":[[[67,58,92,65]]],"search_preferences":{"instant":false,"memory_mib":2048}}"#,
        )
        .unwrap();
        assert!(!s.search_preferences.instant);
        assert!(s.search_preferences.select_on_focus && s.search_preferences.focus_on_open);
        assert_eq!(s.search_preferences.memory_mib, 2048);
        s.search_preferences.select_on_focus = false;
        s.search_preferences.focus_on_open = false;
        let decoded: Settings = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(decoded.search_preferences, s.search_preferences);
        assert_eq!(decoded.panes, s.panes);
        let mut invalid = crate::preferences::SearchPreferences {
            memory_mib: 0,
            ..Default::default()
        };
        invalid.normalize();
        assert_eq!(invalid.memory_mib, 1024);
        invalid.memory_mib = 65535;
        invalid.normalize();
        assert_eq!(invalid.memory_mib, 1024);
    }
    #[test]
    fn flush_acknowledges_the_latest_persisted_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let writer = SettingsWriter::at(dir.path().into());
        for layout in [1, 2, 4, 2] {
            let mut s = Settings::default();
            s.layout = layout;
            writer.submit(s);
        }
        let mut latest = Settings::default();
        latest.layout = 1;
        latest.pane_names = vec!["마지막 설정".into()];
        writer
            .flush(latest)
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let saved: Settings =
            serde_json::from_slice(&fs::read(dir.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved.layout, 1);
        assert_eq!(saved.pane_names, vec!["마지막 설정"]);
    }
    #[test]
    fn flush_reports_failure_without_touching_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, b"preserved").unwrap();
        let writer = SettingsWriter::at(blocked.clone());
        assert!(
            writer
                .flush(Settings::default())
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .is_err()
        );
        assert_eq!(fs::read(blocked).unwrap(), b"preserved");
    }

    #[test]
    fn ntfs_indexing_settings_save_and_load_through_the_profile() {
        use crate::index_policy::{
            NtfsVolumeClass, OfflinePolicy, RelativePathRoot, ScopePolicy, VolumeDecision,
            VolumeGuid, VolumeOverride, VolumePolicy,
        };

        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        settings.ntfs_indexing.set_auto_removable(true).unwrap();
        settings
            .ntfs_indexing
            .set_offline_policy(OfflinePolicy::KeepCache)
            .unwrap();
        settings.ntfs_indexing.set_persistent(true).unwrap();
        let guid = VolumeGuid::parse("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap();
        let scope = ScopePolicy::new(
            vec![RelativePathRoot::parse("Work/Reports").unwrap()],
            vec![RelativePathRoot::parse("Work/Reports/Private").unwrap()],
        )
        .unwrap();
        settings
            .ntfs_indexing
            .set_volume_policy(VolumePolicy::new(
                guid.clone(),
                VolumeOverride::Include,
                false,
                scope,
            ))
            .unwrap();

        save_at(dir.path(), &settings).unwrap();
        let loaded = load_at(dir.path());
        assert_eq!(loaded.version, 1);
        assert!(loaded.ntfs_indexing.is_valid());
        assert!(loaded.ntfs_indexing.auto_fixed());
        assert!(loaded.ntfs_indexing.auto_removable());
        assert_eq!(
            loaded.ntfs_indexing.offline_policy(),
            OfflinePolicy::KeepCache
        );
        assert!(loaded.ntfs_indexing.persistent());
        assert_eq!(
            loaded
                .ntfs_indexing
                .decision_for(&guid, NtfsVolumeClass::Removable),
            VolumeDecision::Included { monitor: false }
        );
        assert!(
            loaded
                .ntfs_indexing
                .scope_allows_path(&guid, "Work/Reports/quarterly.pdf")
        );
        assert!(
            !loaded
                .ntfs_indexing
                .scope_allows_path(&guid, "Work/Reports/Private/notes.txt")
        );
    }

    #[test]
    fn invalid_scoped_settings_fail_closed_and_preserve_other_preferences() {
        use crate::index_policy::{NtfsVolumeClass, VolumeDecision, VolumeGuid};

        let dir = tempfile::tempdir().unwrap();
        let invalid_indexing = serde_json::json!({
            "auto_fixed": true,
            "auto_removable": false,
            "offline": "keep_cache",
            "persistent": true,
            "volumes": [{
                "guid": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                "mode": "include",
                "monitor": true,
                "include_roots": ["../outside"],
                "exclude_roots": []
            }]
        });
        let raw = serde_json::json!({
            "version": 1,
            "layout": 4,
            "search_preferences": {
                "instant": false,
                "select_on_focus": false,
                "focus_on_open": true,
                "memory_mib": 512
            },
            "ntfs_indexing": invalid_indexing
        });
        fs::write(
            dir.path().join("settings.json"),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();

        let loaded = load_at(dir.path());
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.layout, 4);
        assert!(!loaded.search_preferences.instant);
        assert!(!loaded.search_preferences.select_on_focus);
        assert!(loaded.search_preferences.focus_on_open);
        assert_eq!(loaded.search_preferences.memory_mib, 512);
        assert!(!loaded.ntfs_indexing.is_valid());
        assert_eq!(
            loaded.ntfs_indexing.offline_policy(),
            crate::index_policy::OfflinePolicy::KeepCache
        );

        let guid = VolumeGuid::parse("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap();
        assert_eq!(
            loaded
                .ntfs_indexing
                .decision_for(&guid, NtfsVolumeClass::Fixed),
            VolumeDecision::Excluded
        );
        assert!(!loaded.ntfs_indexing.scope_allows_path(&guid, "anything"));

        save_at(dir.path(), &loaded).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["ntfs_indexing"], raw["ntfs_indexing"]);
        let reloaded = load_at(dir.path());
        assert!(!reloaded.ntfs_indexing.is_valid());
        assert_eq!(reloaded.search_preferences, loaded.search_preferences);
    }
}
