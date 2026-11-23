use super::*;
use sdir::file_appearance::{BATCH_LIMIT, EmbeddedIcon, FileAppearance, FileAppearanceKey};
use sdir::model::{CloudState, FileEntry};
use std::{collections::VecDeque, path::Path};

const CACHE_LIMIT: usize = 1024;

#[derive(Default)]
pub(super) struct AppearanceCache {
    values: BTreeMap<FileAppearanceKey, Option<Arc<FileAppearance>>>,
    order: VecDeque<FileAppearanceKey>,
    pending: Option<(u64, Vec<FileAppearanceKey>)>,
}

fn type_key(name: &str) -> FileAppearanceKey {
    let extension = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    FileAppearanceKey {
        extension,
        embedded: None,
    }
}

fn entry_key(entry: &FileEntry) -> FileAppearanceKey {
    let mut key = type_key(&entry.name);
    if !entry.is_link
        && entry.cloud == CloudState::None
        && matches!(key.extension.as_str(), "exe" | "ico")
    {
        key.embedded = Some(EmbeddedIcon {
            path: entry.path.clone(),
            bytes: entry.bytes,
            modified: entry.modified,
        });
    }
    key
}

impl AppearanceCache {
    pub(super) fn for_entry(&self, entry: &FileEntry) -> Option<Arc<FileAppearance>> {
        self.values.get(&entry_key(entry)).and_then(Clone::clone)
    }

    pub(super) fn for_type(&self, name: &str) -> Option<Arc<FileAppearance>> {
        self.values.get(&type_key(name)).and_then(Clone::clone)
    }
}

pub(super) fn appearance_tick(mut model: Signal<AppModel>, bridge: &Bridge) {
    let mut keys = Vec::new();
    {
        let state = model.read();
        if state.closing || state.file_appearance.pending.is_some() {
            return;
        }
        let mut add = |key| {
            if keys.len() < BATCH_LIMIT
                && !state.file_appearance.values.contains_key(&key)
                && !keys.contains(&key)
            {
                keys.push(key);
            }
        };
        for pane in &state.panes[..state.layout] {
            let tab = pane.tab();
            let (start, end, _, _) = pane
                .geometry(state.compact)
                .virtual_range(tab.render_len(), tab.scroll);
            for position in (start..end).filter_map(|row| tab.item_at_render_row(row).ok()) {
                let entry = &tab.entries[tab.visible[position]];
                if !entry.is_dir {
                    add(entry_key(entry));
                }
            }
        }
        for result in state
            .result
            .as_ref()
            .filter(|_| state.search_open)
            .into_iter()
            .chain(
                state
                    .folder_search
                    .as_ref()
                    .and_then(|search| search.result.as_ref()),
            )
        {
            for hit in result.hits.iter().filter(|hit| !hit.is_dir) {
                add(type_key(&hit.name));
            }
        }
    }
    if keys.is_empty() {
        return;
    }
    let mut state = model.write();
    let id = state.id();
    state.file_appearance.pending = Some((id, keys.clone()));
    bridge
        .file_appearance
        .replace(Request::FileAppearances { id, keys });
}

pub(super) fn finish_appearances(
    mut model: Signal<AppModel>,
    id: u64,
    items: Vec<Option<FileAppearance>>,
) {
    if !model
        .read()
        .file_appearance
        .pending
        .as_ref()
        .is_some_and(|(request, _)| *request == id)
    {
        return;
    }
    let mut state = model.write();
    let cache = &mut state.file_appearance;
    let (_, keys) = cache
        .pending
        .take()
        .expect("matching file appearance request");
    let mut items = items.into_iter();
    for key in keys {
        if cache.values.len() >= CACHE_LIMIT
            && let Some(old) = cache.order.pop_front()
        {
            cache.values.remove(&old);
        }
        cache.order.push_back(key.clone());
        cache
            .values
            .insert(key, items.next().flatten().map(Arc::new));
    }
}

pub(super) fn file_icon(is_dir: bool, appearance: Option<&FileAppearance>, large: bool) -> Element {
    if is_dir {
        return rsx! { Icon{kind:"folder"} };
    }
    if let Some(image) = appearance.and_then(|item| {
        if large {
            item.large.as_ref().or(item.small.as_ref())
        } else {
            item.small.as_ref()
        }
    }) {
        rsx! { img{class:"icon icon-shell-file",src:"{image}",alt:"","aria-hidden":"true",draggable:false} }
    } else {
        rsx! { Icon{kind:"file"} }
    }
}
