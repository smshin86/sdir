use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

/// UTF-16 preserves Windows paths even when their names contain unpaired surrogates.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NativePath(pub Vec<u16>);

impl NativePath {
    pub fn new(path: impl AsRef<Path>) -> Self {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Self(path.as_ref().as_os_str().encode_wide().collect())
        }
        #[cfg(not(windows))]
        {
            Self(path.as_ref().to_string_lossy().encode_utf16().collect())
        }
    }
    pub fn to_path(&self) -> PathBuf {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&self.0).into()
        }
        #[cfg(not(windows))]
        {
            PathBuf::from(String::from_utf16_lossy(&self.0))
        }
    }
    pub fn display(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }
    pub fn name(&self) -> String {
        self.to_path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.display())
    }
    pub fn parent(&self) -> Option<Self> {
        self.to_path().parent().map(Self::new)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudState {
    #[default]
    None,
    Remote,
    Available,
    Pinned,
}
impl CloudState {
    pub fn from_attributes(a: u32) -> Self {
        if a & (0x1000 | 0x40000 | 0x400000) != 0 {
            Self::Remote
        } else if a & 0x80000 != 0 {
            Self::Pinned
        } else if a & 0x100000 != 0 {
            Self::Available
        } else {
            Self::None
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Remote => "온라인 전용/원격",
            Self::Available => "로컬 사용 가능",
            Self::Pinned => "로컬 보관",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: NativePath,
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub bytes: Option<u64>,
    pub modified: Option<u64>,
    pub hidden: bool,
    #[serde(default)]
    pub cloud: CloudState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortColumn {
    #[default]
    Name,
    Size,
    Modified,
}

pub fn sort_entries(entries: &mut [FileEntry], column: SortColumn, descending: bool) {
    entries.sort_by_cached_key(|e| {
        // Folder grouping is stable regardless of the requested direction.
        let name = e.name.to_lowercase();
        let value = match column {
            SortColumn::Name => 0,
            SortColumn::Size => e.bytes.unwrap_or(0),
            SortColumn::Modified => e.modified.unwrap_or(0),
        };
        (
            !e.is_dir,
            if descending && column != SortColumn::Name {
                u64::MAX - value
            } else {
                value
            },
            name,
        )
    });
    if descending && column == SortColumn::Name {
        let folders = entries.partition_point(|e| e.is_dir);
        entries[..folders].reverse();
        entries[folders..].reverse();
    }
}

#[derive(Clone, Debug)]
pub struct Tab {
    pub path: NativePath,
    pub back: Vec<NativePath>,
    pub forward: Vec<NativePath>,
    pub entries: Arc<Vec<FileEntry>>,
    pub visible: Arc<Vec<usize>>,
    pub selection: BTreeSet<NativePath>,
    pub cursor: Option<NativePath>,
    pub anchor: Option<NativePath>,
    type_ahead: String,
    type_ahead_at: Option<std::time::Instant>,
    pub filter: String,
    pub sort: SortColumn,
    pub descending: bool,
    pub scroll: f64,
    pub error: Option<String>,
    pub loaded: bool,
    pub resume: Option<TabState>,
    pub pinned: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TabState {
    pub path: NativePath,
    pub back: Vec<NativePath>,
    pub forward: Vec<NativePath>,
    pub filter: String,
    pub sort: SortColumn,
    pub descending: bool,
    pub scroll: f64,
    pub selection: BTreeSet<NativePath>,
    pub cursor: Option<NativePath>,
    pub anchor: Option<NativePath>,
    pub pinned: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Details,
    List,
    Icons,
}
impl ViewMode {
    pub fn value(self) -> &'static str {
        match self {
            Self::Details => "details",
            Self::List => "list",
            Self::Icons => "icons",
        }
    }
    pub fn geometry(self, width: f64, height: f64, compact: bool) -> ViewGeometry {
        ViewGeometry {
            columns: if self == Self::Icons {
                (width / 112.).floor().clamp(1., 16.) as usize
            } else {
                1
            },
            row_height: if self == Self::Icons {
                if height < 90. { 54. } else { 108. }
            } else if compact {
                30.
            } else {
                38.
            },
            viewport_height: height.max(1.),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ViewGeometry {
    pub columns: usize,
    pub row_height: f64,
    pub viewport_height: f64,
}
impl ViewGeometry {
    pub fn row_top(self, position: usize) -> f64 {
        (position / self.columns) as f64 * self.row_height
    }
    pub fn scroll_to(self, position: usize, current: f64) -> f64 {
        let top = self.row_top(position);
        if top < current {
            top
        } else if top + self.row_height > current + self.viewport_height {
            (top + self.row_height - self.viewport_height).max(0.)
        } else {
            current
        }
    }
    pub fn virtual_range(self, count: usize, scroll: f64) -> (usize, usize, f64, f64) {
        let overscan = if self.columns == 1 { 4 } else { 2 };
        let start_row = ((scroll / self.row_height).floor() as usize).saturating_sub(overscan);
        let start = (start_row * self.columns).min(count);
        let capacity = if self.columns == 1 {
            ((self.viewport_height / self.row_height).ceil() as usize + overscan * 2 + 1)
                .clamp(1, 80)
        } else {
            (((self.viewport_height / self.row_height).ceil() as usize + 4) * self.columns).min(512)
        };
        (
            start,
            (start + capacity).min(count),
            start_row as f64 * self.row_height,
            count.div_ceil(self.columns) as f64 * self.row_height,
        )
    }
}
impl Tab {
    pub fn new(path: NativePath) -> Self {
        Self {
            path,
            back: vec![],
            forward: vec![],
            entries: Arc::new(vec![]),
            visible: Arc::new(vec![]),
            selection: BTreeSet::new(),
            cursor: None,
            anchor: None,
            type_ahead: String::new(),
            type_ahead_at: None,
            filter: String::new(),
            sort: SortColumn::Name,
            descending: false,
            scroll: 0.,
            error: None,
            loaded: false,
            resume: None,
            pinned: false,
        }
    }
    pub fn snapshot(&self) -> TabState {
        if let Some(resume) = &self.resume {
            let mut saved = resume.clone();
            saved.pinned = self.pinned;
            return saved;
        }
        TabState {
            path: self.path.clone(),
            back: self
                .back
                .iter()
                .rev()
                .take(128)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            forward: self
                .forward
                .iter()
                .rev()
                .take(128)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            filter: self.filter.chars().take(4096).collect(),
            sort: self.sort,
            descending: self.descending,
            scroll: if self.scroll.is_finite() {
                self.scroll.clamp(0., 1e12)
            } else {
                0.
            },
            selection: self.selection.iter().take(4096).cloned().collect(),
            cursor: self.cursor.clone(),
            anchor: self.anchor.clone(),
            pinned: self.pinned,
        }
    }
    pub fn resume_from(&mut self, state: &TabState) {
        self.pinned = state.pinned;
        if state.path != self.path {
            return;
        }
        self.back = state
            .back
            .iter()
            .rev()
            .take(128)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        self.forward = state
            .forward
            .iter()
            .rev()
            .take(128)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        self.filter = state.filter.chars().take(4096).collect();
        self.sort = state.sort;
        self.descending = state.descending;
        let mut state = state.clone();
        state.selection = state.selection.iter().take(4096).cloned().collect();
        self.resume = Some(state);
    }
    pub fn rebuild_visible(&mut self) {
        let needle = self.filter.to_lowercase();
        self.visible = Arc::new(
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| needle.is_empty() || e.name.to_lowercase().contains(&needle))
                .map(|(i, _)| i)
                .collect(),
        );
    }
    pub fn set_filter(&mut self, filter: String) {
        self.resume = None;
        self.filter = filter;
        self.scroll = 0.;
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        self.type_ahead.clear();
        self.type_ahead_at = None;
        self.rebuild_visible();
    }
    pub fn cursor_position(&self) -> Option<usize> {
        self.cursor.as_ref().and_then(|p| {
            self.visible
                .iter()
                .position(|&i| self.entries[i].path == *p)
        })
    }
    /// Select only confirmed filesystem results, revealing them through a name filter when necessary.
    pub fn select_results(&mut self, paths: &[NativePath]) -> Option<usize> {
        let requested: BTreeSet<_> = paths.iter().collect();
        let results: BTreeSet<_> = self
            .entries
            .iter()
            .filter(|e| requested.contains(&e.path))
            .map(|e| e.path.clone())
            .collect();
        if results.is_empty() {
            return None;
        }
        let visible: BTreeSet<_> = self
            .visible
            .iter()
            .map(|&i| &self.entries[i].path)
            .collect();
        if results.iter().any(|p| !visible.contains(p)) {
            self.set_filter(String::new());
        }
        let position = self
            .visible
            .iter()
            .position(|&i| results.contains(&self.entries[i].path))?;
        let first = self.entries[self.visible[position]].path.clone();
        self.selection = results;
        self.cursor = Some(first.clone());
        self.anchor = Some(first);
        self.type_ahead.clear();
        self.type_ahead_at = None;
        Some(position)
    }
    pub fn invert_selection(&mut self) {
        self.selection = self
            .visible
            .iter()
            .map(|&i| self.entries[i].path.clone())
            .filter(|p| !self.selection.contains(p))
            .collect();
    }
    pub fn select_position(&mut self, position: usize, extend: bool) -> Option<usize> {
        self.resume = None;
        let &index = self.visible.get(position)?;
        let path = self.entries[index].path.clone();
        let anchor = if extend {
            self.anchor
                .as_ref()
                .and_then(|p| {
                    self.visible
                        .iter()
                        .position(|&i| self.entries[i].path == *p)
                })
                .unwrap_or(position)
        } else {
            position
        };
        self.selection.clear();
        for &i in &self.visible[anchor.min(position)..=anchor.max(position)] {
            self.selection.insert(self.entries[i].path.clone());
        }
        if !extend {
            self.anchor = Some(path.clone());
        }
        self.cursor = Some(path);
        Some(position)
    }
    /// Explorer-style incremental prefix matching; repeated single letters cycle.
    pub fn type_to_select(&mut self, text: &str, now: std::time::Instant) -> Option<usize> {
        if text.chars().count() != 1 || text.trim().is_empty() || self.visible.is_empty() {
            return None;
        }
        let text = text.to_lowercase();
        let continuing = self.type_ahead_at.is_some_and(|at| {
            now.saturating_duration_since(at) < std::time::Duration::from_millis(900)
        });
        let cycle = continuing && self.type_ahead == text;
        if !continuing || cycle {
            self.type_ahead = text;
        } else {
            self.type_ahead.push_str(&text);
        }
        self.type_ahead_at = Some(now);
        let start = if cycle {
            self.cursor_position()
                .map_or(0, |i| (i + 1) % self.visible.len())
        } else {
            0
        };
        let position = (0..self.visible.len())
            .map(|offset| (start + offset) % self.visible.len())
            .find(|&position| {
                self.entries[self.visible[position]]
                    .name
                    .to_lowercase()
                    .starts_with(&self.type_ahead)
            })?;
        self.select_position(position, false)
    }
}

#[derive(Clone, Debug)]
pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub generation: u64,
    pub loading: bool,
    pub address: String,
    closed: Vec<ClosedTab>,
    pub view: ViewMode,
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub reveal_revision: u64,
    pub columns: crate::preferences::Columns,
    pub icon_size: u16,
}
#[derive(Clone, Debug)]
struct ClosedTab {
    path: NativePath,
    back: Vec<NativePath>,
    forward: Vec<NativePath>,
    filter: String,
    sort: SortColumn,
    descending: bool,
}
impl Pane {
    pub fn new(path: NativePath) -> Self {
        Self {
            address: path.display(),
            tabs: vec![Tab::new(path)],
            active: 0,
            generation: 0,
            loading: false,
            closed: vec![],
            view: ViewMode::Details,
            viewport_width: 600.,
            viewport_height: 400.,
            reveal_revision: 0,
            columns: Default::default(),
            icon_size: 112,
        }
    }
    pub fn geometry(&self, compact: bool) -> ViewGeometry {
        let mut g = self
            .view
            .geometry(self.viewport_width, self.viewport_height, compact);
        if self.view == ViewMode::Icons {
            g.columns = (self.viewport_width / f64::from(self.icon_size.clamp(80, 192)))
                .floor()
                .clamp(1., 16.) as usize;
            if self.viewport_height >= 90. {
                g.row_height = f64::from(self.icon_size.clamp(80, 192)) - 4.;
            }
        }
        g
    }
    /// Keep an already visible cursor in view when its viewport changes; free scrolling stays free.
    pub fn update_viewport(
        &mut self,
        width: f64,
        height: f64,
        scroll: f64,
        compact: bool,
    ) -> Option<(usize, f64)> {
        if width <= 0. || height <= 0. {
            return None;
        }
        let resized = (self.viewport_width - width).abs() >= 1.
            || (self.viewport_height - height).abs() >= 1.;
        let visible_cursor = if resized {
            let old = self.geometry(compact);
            self.tab().cursor_position().filter(|&position| {
                let top = old.row_top(position);
                top < self.tab().scroll + old.viewport_height
                    && top + old.row_height > self.tab().scroll
            })
        } else {
            None
        };
        self.viewport_width = width;
        self.viewport_height = height;
        let next = visible_cursor.map_or(scroll, |position| {
            self.geometry(compact).scroll_to(position, scroll)
        });
        self.tab_mut().scroll = next;
        visible_cursor.map(|position| (position, next))
    }
    pub fn move_tab(&mut self, index: usize, right: bool) -> bool {
        let target = if right {
            index + 1
        } else {
            index.saturating_sub(1)
        };
        if index >= self.tabs.len() || target >= self.tabs.len() || target == index {
            return false;
        }
        self.tabs.swap(index, target);
        if self.active == index {
            self.active = target;
        } else if self.active == target {
            self.active = index;
        }
        true
    }
    pub fn toggle_pin(&mut self, index: usize) {
        if let Some(t) = self.tabs.get_mut(index) {
            t.pinned = !t.pinned;
            if let Some(s) = t.resume.as_mut() {
                s.pinned = t.pinned;
            }
        }
    }
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
    pub fn new_tab(&mut self) -> bool {
        if self.tabs.len() >= 16 {
            return false;
        }
        self.tabs.push(Tab::new(self.tab().path.clone()));
        self.active = self.tabs.len() - 1;
        true
    }
    pub fn close_tab(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 || index >= self.tabs.len() || self.tabs[index].pinned {
            return false;
        }
        let tab = self.tabs.remove(index);
        self.closed.push(ClosedTab {
            path: tab.path,
            back: tab.back,
            forward: tab.forward,
            filter: tab.filter,
            sort: tab.sort,
            descending: tab.descending,
        });
        if self.closed.len() > 16 {
            self.closed.remove(0);
        }
        if index < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        true
    }
    pub fn can_restore_tab(&self) -> bool {
        !self.closed.is_empty() && self.tabs.len() < 16
    }
    pub fn restore_tab(&mut self) -> bool {
        if !self.can_restore_tab() {
            return false;
        }
        let closed = self.closed.pop().unwrap();
        let mut tab = Tab::new(closed.path);
        tab.back = closed.back;
        tab.forward = closed.forward;
        tab.filter = closed.filter;
        tab.sort = closed.sort;
        tab.descending = closed.descending;
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        true
    }
    pub fn begin_load(&mut self) -> u64 {
        self.generation += 1;
        self.loading = true;
        self.address = self.tab().path.display();
        let tab = self.tab_mut();
        tab.entries = Arc::new(vec![]);
        tab.visible = Arc::new(vec![]);
        tab.selection.clear();
        tab.cursor = None;
        tab.anchor = None;
        tab.type_ahead.clear();
        tab.type_ahead_at = None;
        tab.error = None;
        tab.scroll = 0.;
        tab.loaded = false;
        self.generation
    }
    pub fn navigate(&mut self, path: NativePath) {
        let tab = self.tab_mut();
        tab.resume = None;
        if tab.path != path {
            tab.back.push(tab.path.clone());
            if tab.back.len() > 128 {
                tab.back.remove(0);
            }
            tab.forward.clear();
            tab.path = path;
        }
    }
    pub fn back(&mut self) -> bool {
        let tab = self.tab_mut();
        if let Some(path) = tab.back.pop() {
            tab.resume = None;
            tab.forward.push(tab.path.clone());
            tab.path = path;
            true
        } else {
            false
        }
    }
    pub fn forward(&mut self) -> bool {
        let tab = self.tab_mut();
        if let Some(path) = tab.forward.pop() {
            tab.resume = None;
            tab.back.push(tab.path.clone());
            tab.path = path;
            true
        } else {
            false
        }
    }
    pub fn accept_batch(&mut self, generation: u64, mut entries: Vec<FileEntry>) -> bool {
        if self.generation != generation {
            return false;
        }
        let tab = self.tab_mut();
        let start = tab.entries.len();
        let filter = tab.filter.to_lowercase();
        let indices: Vec<_> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| filter.is_empty() || e.name.to_lowercase().contains(&filter))
            .map(|(i, _)| start + i)
            .collect();
        Arc::make_mut(&mut tab.entries).append(&mut entries);
        Arc::make_mut(&mut tab.visible).extend(indices);
        true
    }
    pub fn finish(&mut self, generation: u64, error: Option<String>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.loading = false;
        let tab = self.tab_mut();
        sort_entries(
            Arc::make_mut(&mut tab.entries).as_mut_slice(),
            tab.sort,
            tab.descending,
        );
        tab.rebuild_visible();
        let resumed = tab.resume.is_some();
        if let Some(saved) = tab.resume.take() {
            let visible: BTreeSet<_> = tab
                .visible
                .iter()
                .map(|&i| tab.entries[i].path.clone())
                .collect();
            tab.selection = saved.selection.intersection(&visible).cloned().collect();
            tab.cursor = saved.cursor.filter(|p| visible.contains(p));
            tab.anchor = saved.anchor.filter(|p| visible.contains(p));
            tab.scroll = if saved.scroll.is_finite() {
                saved.scroll.clamp(0., 1e12)
            } else {
                0.
            };
        }
        tab.error = error;
        tab.loaded = true;
        if resumed {
            self.reveal_revision += 1;
        }
        true
    }
    pub fn select(&mut self, path: NativePath, toggle: bool, range: bool) {
        let tab = self.tab_mut();
        let Some(position) = tab
            .visible
            .iter()
            .position(|&i| tab.entries[i].path == path)
        else {
            return;
        };
        tab.type_ahead.clear();
        tab.type_ahead_at = None;
        if range {
            tab.select_position(position, true);
            return;
        }
        if toggle {
            if !tab.selection.remove(&path) {
                tab.selection.insert(path.clone());
            }
            tab.cursor = Some(path.clone());
            tab.anchor = Some(path);
        } else {
            tab.select_position(position, false);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileAction {
    BatchRename {
        pairs: Vec<crate::batch::RenamePair>,
    },
    History {
        record: crate::history::Record,
    },
    Copy {
        sources: Vec<NativePath>,
        destination: NativePath,
    },
    Move {
        sources: Vec<NativePath>,
        destination: NativePath,
    },
    Recycle {
        sources: Vec<NativePath>,
    },
    Rename {
        source: NativePath,
        name: String,
    },
    NewFolder {
        destination: NativePath,
        name: String,
    },
}

impl FileAction {
    pub fn sources(&self) -> Vec<NativePath> {
        match self {
            Self::Copy { sources, .. } | Self::Move { sources, .. } | Self::Recycle { sources } => {
                sources.clone()
            }
            Self::Rename { source, .. } => vec![source.clone()],
            Self::BatchRename { pairs } => pairs.iter().map(|p| p.source.clone()).collect(),
            Self::History { record } => record
                .items
                .iter()
                .map(|i| i.source.path().clone())
                .collect(),
            _ => vec![],
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Copy { .. } => "복사",
            Self::Move { .. } => "이동",
            Self::Recycle { .. } => "휴지통",
            Self::Rename { .. } => "이름 변경",
            Self::BatchRename { .. } => "이름 일괄 변경",
            Self::NewFolder { .. } => "새 폴더",
            Self::History { .. } => "실행 취소/다시 실행",
        }
    }
    pub fn destination(&self) -> Option<NativePath> {
        match self {
            Self::Copy { destination, .. }
            | Self::Move { destination, .. }
            | Self::NewFolder { destination, .. } => Some(destination.clone()),
            Self::Rename { source, .. } => source.parent(),
            Self::BatchRename { pairs } => pairs.first().and_then(|p| p.source.parent()),
            Self::History { record } => record.destination(),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemOutcome {
    pub source: NativePath,
    pub ok: bool,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobResult {
    pub ok: bool,
    pub cancelled: bool,
    pub message: String,
    #[serde(default)]
    pub destinations: Vec<NativePath>,
    #[serde(default)]
    pub undo: Option<crate::history::Record>,
    #[serde(default)]
    pub history_note: Option<String>,
    #[serde(default)]
    pub outcomes: Vec<ItemOutcome>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloud_attributes_do_not_imply_sync_health() {
        assert_eq!(CloudState::from_attributes(0), CloudState::None);
        assert_eq!(CloudState::from_attributes(0x1000), CloudState::Remote);
        assert_eq!(
            CloudState::from_attributes(0x400000 | 0x80000),
            CloudState::Remote
        );
        assert_eq!(CloudState::from_attributes(0x80000), CloudState::Pinned);
        assert_eq!(CloudState::from_attributes(0x100000), CloudState::Available);
    }
    #[test]
    fn reordered_and_pinned_tabs_keep_identity() {
        let mut p = Pane::new(NativePath::new("a"));
        p.new_tab();
        p.navigate(NativePath::new("b"));
        p.toggle_pin(0);
        assert!(p.tabs[0].pinned);
        let count = p.tabs.len();
        p.close_tab(0);
        assert_eq!(p.tabs.len(), count);
        assert!(p.move_tab(1, false));
        assert_eq!(p.tab().path, NativePath::new("b"));
        assert!(p.tabs[1].pinned);
    }
    fn entry(name: &str, dir: bool) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: NativePath::new(name),
            is_dir: dir,
            is_link: false,
            bytes: Some(1),
            modified: None,
            hidden: false,
            cloud: CloudState::None,
        }
    }
    #[test]
    fn resumed_tabs_keep_filter_sort_history_and_existing_selection() {
        let mut p = Pane::new(NativePath::new("root"));
        let state = TabState {
            path: NativePath::new("root"),
            back: vec![NativePath::new("previous")],
            forward: vec![NativePath::new("next")],
            filter: "keep".into(),
            sort: SortColumn::Size,
            descending: true,
            scroll: 1200.,
            selection: [NativePath::new("keep.txt"), NativePath::new("gone")]
                .into_iter()
                .collect(),
            cursor: Some(NativePath::new("keep.txt")),
            anchor: Some(NativePath::new("keep.txt")),
            pinned: false,
        };
        p.tab_mut().resume_from(&state);
        assert_eq!(
            p.tab().snapshot().scroll,
            1200.,
            "closing during startup must not discard the saved state"
        );
        let id = p.begin_load();
        p.accept_batch(
            id,
            vec![entry("keep.txt", false), entry("other.txt", false)],
        );
        p.finish(id, None);
        assert_eq!(p.tab().filter, "keep");
        assert_eq!(p.tab().selection.len(), 1);
        assert_eq!(p.tab().cursor, Some(NativePath::new("keep.txt")));
        assert_eq!(p.tab().scroll, 1200.);
        assert_eq!(p.tab().sort, SortColumn::Size);
        assert!(p.tab().descending);
        assert!(p.back());
        assert_eq!(p.tab().path, NativePath::new("previous"));
        assert!(p.forward());
        assert_eq!(p.tab().path, NativePath::new("root"));
    }
    #[test]
    fn confirmed_results_clear_a_hiding_filter_and_set_cursor_for_followup_actions() {
        let mut tab = Tab::new(NativePath::new("folder"));
        tab.entries = Arc::new(vec![
            entry("old", false),
            entry("result-a", false),
            entry("result-b", false),
        ]);
        tab.set_filter("old".into());
        let paths = vec![
            NativePath::new("result-b"),
            NativePath::new("missing"),
            NativePath::new("result-a"),
        ];
        assert_eq!(tab.select_results(&paths), Some(1));
        assert!(tab.filter.is_empty());
        assert_eq!(tab.selection.len(), 2);
        assert_eq!(tab.cursor, Some(NativePath::new("result-a")));
        assert_eq!(tab.anchor, tab.cursor);
        let selection = tab.selection.clone();
        assert!(
            tab.select_results(&[NativePath::new("not-created")])
                .is_none()
        );
        assert_eq!(
            tab.selection, selection,
            "skipped or cancelled copies cannot select guessed targets"
        );
        tab.invert_selection();
        assert_eq!(
            tab.selection,
            [NativePath::new("old")].into_iter().collect()
        );
    }
    #[test]
    fn grid_geometry_keeps_results_visible_and_virtualizes_complete_rows() {
        let grid = ViewMode::Icons.geometry(450., 250., true);
        assert_eq!(grid.columns, 4);
        assert_eq!(grid.row_top(10), 216.);
        assert_eq!(grid.scroll_to(10, 0.), 74.);
        assert_eq!(
            grid.scroll_to(10, 100.),
            100.,
            "visible results do not need to jump"
        );
        let (start, end, offset, height) = grid.virtual_range(100_003, 10_800.);
        assert_eq!(start % 4, 0);
        assert_eq!(offset, 98. * 108.);
        assert!(end - start <= 512);
        assert_eq!(height, 25_001. * 108.);
        assert_eq!(ViewMode::Icons.geometry(80., 300., false).columns, 1);
        let details = ViewMode::Details.geometry(450., 250., false);
        assert_eq!(details.virtual_range(1000, 1140.).0, 26);
    }
    #[test]
    fn details_render_budget_tracks_viewport_without_gaps() {
        for height in [45., 250., 800.] {
            let view = ViewMode::Details.geometry(450., height, false);
            for scroll in [0., 19., 1140., 37500.] {
                let (start, end, _, _) = view.virtual_range(1000, scroll);
                let first_visible = ((scroll / view.row_height).floor() as usize).min(999);
                let after_visible =
                    (((scroll + height) / view.row_height).ceil() as usize).min(1000);
                assert!(start <= first_visible && end >= after_visible);
                assert!(end - start <= (height / view.row_height).ceil() as usize * 2 + 10);
            }
        }
    }
    #[test]
    fn resized_viewport_reveals_visible_cursor_but_preserves_manual_scroll() {
        let mut pane = Pane::new(NativePath::new("root"));
        pane.tab_mut().entries = Arc::new(
            (0..200)
                .map(|i| entry(&format!("item-{i:04}"), false))
                .collect(),
        );
        pane.tab_mut().rebuild_visible();
        pane.tab_mut().select_position(182, false);
        pane.tab_mut().scroll = pane.geometry(true).scroll_to(182, 0.);
        let old = pane.tab().scroll;
        let (_, next) = pane.update_viewport(600., 120., old, true).unwrap();
        assert!(next > old);
        assert!(pane.geometry(true).row_top(182) + 30. <= next + 120.);
        assert!(pane.update_viewport(0., 0., 0., true).is_none());
        assert_eq!(
            pane.tab().scroll,
            next,
            "minimized geometry must not erase scroll state"
        );
        assert!(pane.update_viewport(600., 120., 0., true).is_none());
        assert!(pane.update_viewport(600., 90., 0., true).is_none());
        assert_eq!(
            pane.tab().scroll,
            0.,
            "manual scrolling away from the cursor is preserved"
        );
    }
    #[test]
    fn closing_inactive_tab_preserves_active_path_and_restore_reloads_without_buffers() {
        let mut p = Pane::new(NativePath::new("a"));
        p.tab_mut().entries = Arc::new(vec![entry("payload", false)]);
        p.tab_mut().rebuild_visible();
        let weak = Arc::downgrade(&p.tab().entries);
        p.tab_mut().filter = "payload".into();
        p.new_tab();
        p.navigate(NativePath::new("b"));
        assert!(p.close_tab(0));
        assert_eq!(p.tab().path, NativePath::new("b"));
        assert!(
            weak.upgrade().is_none(),
            "closed history must release listing buffers"
        );
        assert!(p.restore_tab());
        assert_eq!(p.tab().path, NativePath::new("a"));
        assert_eq!(p.tab().filter, "payload");
        assert!(!p.tab().loaded && p.tab().entries.is_empty() && p.tab().selection.is_empty());
    }
    #[test]
    fn tab_limits_and_closed_history_are_bounded_and_pane_local() {
        let mut p = Pane::new(NativePath::new("a"));
        assert!(!p.close_tab(0));
        assert!(!p.restore_tab());
        for _ in 1..16 {
            assert!(p.new_tab());
        }
        assert!(!p.new_tab());
        assert!(p.close_tab(0));
        assert!(p.new_tab());
        assert!(!p.restore_tab());
        for i in 0..40 {
            p.navigate(NativePath::new(format!("closed-{i}")));
            assert!(p.close_tab(p.active));
            assert!(p.new_tab());
        }
        assert_eq!(p.closed.len(), 16);
        assert!(!Pane::new(NativePath::new("other")).can_restore_tab());
    }
    #[test]
    fn stale_reply_cannot_replace_new_folder() {
        let mut p = Pane::new(NativePath::new("old"));
        let old = p.begin_load();
        p.navigate(NativePath::new("new"));
        let new = p.begin_load();
        assert!(!p.accept_batch(old, vec![entry("stale", false)]));
        assert!(!p.finish(old, None));
        assert!(p.loading);
        assert!(p.accept_batch(new, vec![entry("current", false)]));
        p.finish(new, None);
        assert_eq!(p.tab().entries[0].name, "current");
    }
    #[test]
    fn panels_and_tabs_have_independent_history() {
        let mut a = Pane::new(NativePath::new("a"));
        let b = Pane::new(NativePath::new("b"));
        a.navigate(NativePath::new("c"));
        assert!(a.back());
        assert_eq!(a.tab().path, NativePath::new("a"));
        assert_eq!(b.tab().path, NativePath::new("b"));
        a.tabs.push(Tab::new(NativePath::new("other")));
        a.active = 1;
        assert!(a.tab().back.is_empty());
    }
    #[test]
    fn folders_stay_first_in_both_directions() {
        for descending in [false, true] {
            let mut e = vec![
                entry("a.txt", false),
                entry("z-folder", true),
                entry("b.txt", false),
            ];
            sort_entries(&mut e, SortColumn::Name, descending);
            assert!(e[0].is_dir);
        }
    }
    #[test]
    fn korean_paths_round_trip_json() {
        let p = NativePath::new("C:\\작업\\보고서.txt");
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<NativePath>(&json).unwrap(), p);
    }
    #[test]
    fn prefix_navigation_cycles_matches_and_respects_filter() {
        use std::time::{Duration, Instant};
        let mut tab = Tab::new(NativePath::new("folder"));
        tab.entries = Arc::new(vec![
            entry("alpha", false),
            entry("jacket", false),
            entry("jungle", false),
            entry("주간.txt", false),
        ]);
        tab.rebuild_visible();
        let now = Instant::now();
        assert_eq!(tab.type_to_select("j", now), Some(1));
        assert_eq!(
            tab.type_to_select("j", now + Duration::from_millis(50)),
            Some(2)
        );
        assert_eq!(
            tab.type_to_select("j", now + Duration::from_millis(100)),
            Some(1)
        );
        assert_eq!(
            tab.type_to_select("u", now + Duration::from_millis(150)),
            Some(2)
        );
        assert_eq!(
            tab.type_to_select("주", now + Duration::from_secs(2)),
            Some(3)
        );
        tab.set_filter("jungle".into());
        assert!(tab.selection.is_empty());
        assert_eq!(
            tab.type_to_select("j", now + Duration::from_secs(3)),
            Some(0)
        );
        assert_eq!(
            tab.selection.iter().next(),
            Some(&NativePath::new("jungle"))
        );
    }
    #[test]
    fn shift_navigation_keeps_original_anchor() {
        let mut tab = Tab::new(NativePath::new("folder"));
        tab.entries = Arc::new(vec![
            entry("a", false),
            entry("b", false),
            entry("c", false),
        ]);
        tab.rebuild_visible();
        tab.select_position(1, false);
        tab.select_position(2, true);
        assert_eq!(tab.selection.len(), 2);
        tab.select_position(0, true);
        assert_eq!(tab.selection.len(), 2);
        assert!(tab.selection.contains(&NativePath::new("a")));
        assert!(!tab.selection.contains(&NativePath::new("c")));
    }
}
