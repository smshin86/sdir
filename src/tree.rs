//! Lazy navigation cache. Only an explicitly expanded directory is enumerated.
use crate::{filesystem, model::NativePath};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    time::{Duration, Instant},
};

pub const CHILD_LIMIT: usize = 1000;
pub const NODE_LIMIT: usize = 5000;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Listing {
    pub folders: Vec<NativePath>,
    pub warning: Option<String>,
}
pub fn list(path: &NativePath, hidden: bool) -> Result<Listing, String> {
    let started = Instant::now();
    let mut result = Listing::default();
    let mut wire_bytes = 0usize;
    for item in fs::read_dir(path.to_path()).map_err(|e| e.to_string())? {
        if started.elapsed() > Duration::from_secs(3) {
            result.warning = Some(
                "조회 시간 한도 · 일부 폴더만 표시합니다. 폴더를 열어 전체 목록을 확인하세요."
                    .into(),
            );
            break;
        }
        let entry = item
            .map_err(|e| e.to_string())
            .and_then(|e| filesystem::directory_entry(&e));
        match entry {
            Ok(e) if e.is_dir && (hidden || !e.hidden) => {
                if result.folders.len() == CHILD_LIMIT {
                    result.warning = Some(
                        "하위 폴더 1,000개까지만 표시합니다. 폴더를 열어 전체 목록을 확인하세요."
                            .into(),
                    );
                    break;
                }
                wire_bytes += e.path.0.len() * 6 + 128;
                if wire_bytes > 500_000 {
                    result.warning =
                        Some("경로 표시 한도 · 폴더를 열어 전체 목록을 확인하세요.".into());
                    break;
                }
                result.folders.push(e.path);
            }
            Err(_) => result.warning = Some("읽지 못한 항목이 있습니다. 다시 시도하세요.".into()),
            _ => {}
        }
    }
    result
        .folders
        .sort_by_cached_key(|p| (p.name().to_lowercase(), p.clone()));
    Ok(result)
}
#[derive(Clone, Debug, Default)]
pub struct Branch {
    pub expanded: bool,
    pub loaded: bool,
    pub children: Vec<NativePath>,
    pub message: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Row {
    pub path: NativePath,
    pub depth: usize,
    pub message: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub roots: Vec<NativePath>,
    pub nodes: BTreeMap<NativePath, Branch>,
    pub open: bool,
    pub pending: Option<(u64, NativePath)>,
    serial: u64,
}
impl Tree {
    pub fn new(roots: Vec<NativePath>) -> Self {
        let nodes = roots
            .iter()
            .cloned()
            .map(|p| (p, Branch::default()))
            .collect();
        Self {
            roots,
            nodes,
            open: true,
            ..Self::default()
        }
    }
    /// Make the known current path visible without enumerating sibling subtrees.
    /// Returns the next single directory to read and whether the path is ready/blocked.
    pub fn follow_step(
        &mut self,
        target: &NativePath,
        expand_target: bool,
    ) -> (Option<NativePath>, bool) {
        let mut chain: Vec<_> = target.to_path().ancestors().map(NativePath::new).collect();
        chain.reverse();
        if chain.is_empty() || chain.len() > 129 {
            return (None, true);
        }
        let root = chain[0].clone();
        if !self.roots.contains(&root) {
            if self.roots.len() >= 32 {
                return (None, true);
            }
            self.roots.push(root);
        }
        for (i, path) in chain.iter().enumerate() {
            if !self.nodes.contains_key(path) && self.nodes.len() >= NODE_LIMIT {
                return (None, true);
            }
            self.nodes.entry(path.clone()).or_default();
            if i > 0 {
                let parent = self.nodes.get_mut(&chain[i - 1]).unwrap();
                if !parent.children.contains(path) {
                    parent.children.push(path.clone());
                    parent.children.sort();
                }
                parent.expanded = true;
            }
        }
        let count = chain.len() - usize::from(!expand_target);
        for path in &chain[..count] {
            let n = self.nodes.get_mut(path).unwrap();
            n.expanded = true;
            if !n.loaded {
                if n.message.is_some() {
                    return (None, true);
                }
                if self.pending.is_some() {
                    return (None, false);
                }
                return (Some(path.clone()), false);
            }
        }
        (None, true)
    }
    pub fn reset(&mut self) {
        self.pending = None;
        self.serial += 1;
        self.nodes = self
            .roots
            .iter()
            .cloned()
            .map(|p| (p, Branch::default()))
            .collect();
    }
    fn prune(&mut self, path: &NativePath) {
        let mut todo = self
            .nodes
            .get_mut(path)
            .map(|n| std::mem::take(&mut n.children))
            .unwrap_or_default();
        while let Some(p) = todo.pop() {
            if let Some(node) = self.nodes.remove(&p) {
                todo.extend(node.children);
            }
            if self.pending.as_ref().is_some_and(|(_, path)| *path == p) {
                self.pending = None;
            }
        }
    }
    pub fn collapse(&mut self, path: &NativePath) {
        self.prune(path);
        if self.pending.as_ref().is_some_and(|(_, p)| p == path) {
            self.pending = None;
        }
        if let Some(node) = self.nodes.get_mut(path) {
            *node = Branch::default();
        }
    }
    pub fn begin(&mut self, path: &NativePath) -> Option<u64> {
        if !self.nodes.contains_key(path) {
            return None;
        }
        if let Some((_, old)) = self.pending.take() {
            if let Some(node) = self.nodes.get_mut(&old) {
                node.message = Some("읽기를 중단했습니다. 다시 시도하세요.".into());
            }
        }
        self.prune(path);
        self.serial += 1;
        let node = self.nodes.get_mut(path).unwrap();
        node.expanded = true;
        node.loaded = false;
        node.message = None;
        self.pending = Some((self.serial, path.clone()));
        Some(self.serial)
    }
    pub fn finish(&mut self, id: u64, result: Result<Listing, String>) -> bool {
        let Some((expected, path)) = self.pending.as_ref() else {
            return false;
        };
        if *expected != id {
            return false;
        }
        let path = path.clone();
        self.pending = None;
        let listing = match result {
            Ok(mut l) => {
                let remaining = NODE_LIMIT.saturating_sub(self.nodes.len());
                if l.folders.len() > remaining {
                    l.folders.truncate(remaining);
                    l.warning =
                        Some("트리 표시 한도 · 다른 가지를 접거나 폴더를 직접 여세요.".into());
                }
                l
            }
            Err(e) => {
                self.nodes.get_mut(&path).unwrap().message = Some(format!("열 수 없습니다: {e}"));
                return true;
            }
        };
        for p in &listing.folders {
            self.nodes.entry(p.clone()).or_default();
        }
        let node = self.nodes.get_mut(&path).unwrap();
        node.loaded = true;
        node.children = listing.folders;
        node.message = listing.warning;
        true
    }
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![];
        if !self.open {
            return rows;
        }
        let mut todo: Vec<_> = self.roots.iter().rev().cloned().map(|p| (p, 0)).collect();
        while let Some((path, depth)) = todo.pop() {
            rows.push(Row {
                path: path.clone(),
                depth,
                message: None,
            });
            let Some(node) = self.nodes.get(&path) else {
                continue;
            };
            if !node.expanded {
                continue;
            }
            let pending = self.pending.as_ref().is_some_and(|(_, p)| *p == path);
            let message = if pending {
                Some("읽는 중…".into())
            } else {
                node.message.clone().or_else(|| {
                    (node.loaded && node.children.is_empty()).then(|| "하위 폴더 없음".into())
                })
            };
            if let Some(message) = message {
                rows.push(Row {
                    path: path.clone(),
                    depth: depth + 1,
                    message: Some(message),
                });
            }
            if depth < 128 {
                todo.extend(node.children.iter().rev().cloned().map(|p| (p, depth + 1)));
            }
        }
        rows
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collapse_cancels_descendants_and_rejects_stale_replies() {
        let root = NativePath::new("root");
        let child = NativePath::new("root/child");
        let mut t = Tree::new(vec![root.clone()]);
        let id = t.begin(&root).unwrap();
        assert!(t.finish(
            id,
            Ok(Listing {
                folders: vec![child.clone()],
                warning: None
            })
        ));
        let stale = t.begin(&child).unwrap();
        t.collapse(&root);
        assert!(!t.finish(stale, Ok(Listing::default())));
        assert_eq!(t.nodes.len(), 1);
        let new = t.begin(&root).unwrap();
        assert!(!t.finish(id, Ok(Listing::default())));
        assert!(t.finish(new, Err("offline".into())));
        assert!(
            t.rows()
                .iter()
                .any(|r| r.message.as_ref().is_some_and(|s| s.contains("offline")))
        );
    }
    #[test]
    fn folders_only_and_bounded_listing_never_reads_descendants() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("file.txt"), b"file").unwrap();
        fs::create_dir(d.path().join("nested")).unwrap();
        fs::create_dir(d.path().join("nested/deep")).unwrap();
        let l = list(&NativePath::new(d.path()), true).unwrap();
        assert_eq!(l.folders, vec![NativePath::new(d.path().join("nested"))]);
        for i in 0..CHILD_LIMIT {
            fs::create_dir(d.path().join(format!("dir{i}"))).unwrap();
        }
        let l = list(&NativePath::new(d.path()), true).unwrap();
        assert_eq!(l.folders.len(), CHILD_LIMIT);
        assert!(l.warning.is_some());
        let root = NativePath::new(d.path());
        let mut t = Tree::new(vec![root.clone()]);
        let id = t.begin(&root).unwrap();
        t.finish(
            id,
            Ok(Listing {
                folders: (0..NODE_LIMIT + 1)
                    .map(|i| NativePath::new(format!("item{i}")))
                    .collect(),
                warning: None,
            }),
        );
        assert_eq!(t.nodes.len(), NODE_LIMIT);
        t.reset();
        assert_eq!(t.nodes.len(), 1);
    }
}
