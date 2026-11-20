use crate::{filesystem, model::NativePath};
use goz_core::{
    index::{FrnMap, NTFS_ROOT_FRN, PathStatus, VolumeIndex},
    query::{Budget, parse_query, run_query_refs},
    types::Frn,
    usn::record::{FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_FILE_CREATE},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, ReadDir},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const MAX_ENTRIES: usize = 2_000_000;
const MAX_DEPTH: usize = 128;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: NativePath,
    pub name: String,
    pub is_dir: bool,
    #[serde(default)]
    pub volume_guid: Option<crate::index_policy::VolumeGuid>,
    #[serde(default)]
    pub volume_mount: Option<NativePath>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub total: u64,
    pub elapsed_ms: f64,
    pub complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexStatus {
    pub root: NativePath,
    pub entries: usize,
    pub skipped: usize,
    pub complete: bool,
    pub limited: bool,
    pub allocated_bytes: u64,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub host_pid: Option<u32>,
    #[serde(default)]
    pub volumes: Vec<crate::live_search::VolumeStatus>,
}

pub struct FolderIndex {
    pub root: NativePath,
    index: VolumeIndex,
    stack: Vec<(ReadDir, Frn)>,
    next: u64,
    skipped: usize,
    done: bool,
    limited: bool,
}
impl FolderIndex {
    pub fn new(root: NativePath) -> Result<Self, String> {
        let dir = fs::read_dir(root.to_path()).map_err(|e| e.to_string())?;
        Ok(Self {
            root,
            index: VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse()),
            stack: vec![(dir, NTFS_ROOT_FRN)],
            next: 1000,
            skipped: 0,
            done: false,
            limited: false,
        })
    }
    pub fn step(&mut self, count: usize) {
        if self.done {
            return;
        }
        for _ in 0..count {
            let Some((reader, parent)) = self.stack.last_mut() else {
                self.done = true;
                self.index.optimize_storage();
                break;
            };
            let parent = *parent;
            let Some(next) = reader.next() else {
                self.stack.pop();
                continue;
            };
            let entry = match next {
                Ok(e) => e,
                Err(_) => {
                    self.skipped += 1;
                    continue;
                }
            };
            let info = match filesystem::directory_entry(&entry) {
                Ok(e) => e,
                Err(_) => {
                    self.skipped += 1;
                    continue;
                }
            };
            if info.hidden {
                continue;
            }
            let path = NativePath::new(entry.path());
            let native_name = NativePath::new(std::path::Path::new(&entry.file_name()));
            let mut bytes = vec![];
            let lossy = goz_core::wtf8::from_utf16(&native_name.0, &mut bytes);
            let frn = Frn(self.next | (1u64 << 48));
            self.next += 1;
            self.index.insert_enum(&ParsedUsnRecord {
                major_version: 2,
                frn,
                parent_frn: parent,
                usn: 0,
                timestamp_ft: 0,
                reason: USN_REASON_FILE_CREATE,
                attributes: if info.is_dir {
                    FILE_ATTRIBUTE_DIRECTORY
                } else {
                    0
                },
                name: bytes,
                name_lossy: lossy,
            });
            if info.is_dir && !info.is_link {
                if self.stack.len() < MAX_DEPTH {
                    match fs::read_dir(path.to_path()) {
                        Ok(d) => self.stack.push((d, frn)),
                        Err(_) => self.skipped += 1,
                    }
                } else {
                    self.skipped += 1;
                }
            }
            if self.index.len() >= MAX_ENTRIES {
                self.stack.clear();
                self.done = true;
                self.limited = true;
                self.index.optimize_storage();
                break;
            }
        }
    }
    pub fn status(&self) -> IndexStatus {
        IndexStatus {
            root: self.root.clone(),
            entries: self.index.len().saturating_sub(2),
            skipped: self.skipped,
            complete: self.done,
            limited: self.limited,
            allocated_bytes: self.index.memory().total().allocated,
            live: false,
            host_pid: None,
            volumes: vec![],
        }
    }
    pub fn query(&self, text: &str) -> Result<SearchResult, String> {
        query_index(&self.index, &self.root, text, self.done)
    }
}
pub(crate) fn validate_query_text(text: &str) -> Result<(), String> {
    if text.len() > 4096 {
        Err("검색어가 너무 깁니다.".into())
    } else {
        Ok(())
    }
}
pub(crate) fn query_index(
    index: &VolumeIndex,
    root: &NativePath,
    text: &str,
    complete: bool,
) -> Result<SearchResult, String> {
    query_index_with_policy(index, root, text, complete, None)
}

pub(crate) fn query_index_with_policy(
    index: &VolumeIndex,
    root: &NativePath,
    text: &str,
    complete: bool,
    scope: Option<&crate::index_policy::ScopePolicy>,
) -> Result<SearchResult, String> {
    let started = Instant::now();
    let cancel = AtomicBool::new(false);
    let budget = Budget::until(started + Duration::from_millis(250), &cancel);
    query_index_with_scope(index, root, text, complete, scope, &budget)
}

pub(crate) fn query_index_with_scope(
    index: &VolumeIndex,
    root: &NativePath,
    text: &str,
    complete: bool,
    scope: Option<&crate::index_policy::ScopePolicy>,
    budget: &Budget<'_>,
) -> Result<SearchResult, String> {
    let started = Instant::now();
    validate_query_text(text)?;
    if text.trim().is_empty() {
        return Ok(SearchResult {
            hits: vec![],
            total: 0,
            elapsed_ms: 0.,
            complete,
        });
    }

    let scoped_policy = scope
        .filter(|policy| !policy.include_roots().is_empty() || !policy.exclude_roots().is_empty());
    if scoped_policy.is_some() && budget.check() {
        return Err(search_timeout_error());
    }

    let mut parsed = parse_query(text).map_err(|error| error.to_string())?;
    let mount_prefix = if let Some(policy) = scoped_policy {
        if budget.check() {
            return Err(search_timeout_error());
        }
        let mut graph_dependent = false;
        if let Some(expression) = parsed.expr.as_ref() {
            expression.for_each_leaf(&mut |leaf| {
                if matches!(
                    leaf,
                    goz_core::query::Leaf::Related { .. }
                        | goz_core::query::Leaf::Count { .. }
                        | goz_core::query::Leaf::TotalChildSize(_)
                ) {
                    graph_dependent = true;
                }
            });
        }
        if graph_dependent {
            return Err(
                "범위 검색에서는 Related, Count, TotalChildSize 조건을 지원하지 않습니다.".into(),
            );
        }
        let mount_prefix = scoped_mount_prefix(root);
        let scope_expression = scoped_expression(policy, &mount_prefix);
        parsed.expr = Some(match parsed.expr.take() {
            Some(existing) => goz_core::query::Expr::And(vec![existing, scope_expression]),
            None => scope_expression,
        });
        mount_prefix
    } else {
        root.display()
    };

    if scoped_policy.is_some() && budget.check() {
        return Err(search_timeout_error());
    }
    let out = run_query_refs(
        index,
        &mount_prefix,
        &parsed,
        None,
        Default::default(),
        0,
        Some(200),
        false,
        budget,
    )
    .map_err(|_| search_timeout_error())?;
    if scoped_policy.is_some() && budget.check() {
        return Err(search_timeout_error());
    }

    let mut hits = vec![];
    for result in out.refs {
        if scoped_policy.is_some() && budget.check() {
            return Err(search_timeout_error());
        }
        let mut relative = vec![];
        if index.path_of(result.idx, &mut relative) != PathStatus::Ok {
            continue;
        }
        let path = join_relative(root, &relative);
        hits.push(SearchHit {
            name: path.name(),
            path,
            is_dir: index.entry(result.idx).is_dir(),
            volume_guid: None,
            volume_mount: None,
        });
    }
    if scoped_policy.is_some() && budget.check() {
        return Err(search_timeout_error());
    }
    Ok(SearchResult {
        hits,
        total: out.total,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.,
        complete,
    })
}

fn search_timeout_error() -> String {
    "검색 시간이 초과됐습니다. 검색 범위를 줄여 주세요.".to_string()
}

fn scoped_mount_prefix(root: &NativePath) -> String {
    let separator = char::from(92).to_string();
    let mut mount = root.display().replace("/", &separator);
    while mount.ends_with(char::from(92)) {
        mount.pop();
    }
    mount.push(char::from(92));
    mount
}

fn scoped_expression(
    policy: &crate::index_policy::ScopePolicy,
    mount: &str,
) -> goz_core::query::Expr {
    let include = if policy.include_roots().is_empty() {
        scoped_full_path_leaf(mount, false)
    } else {
        scoped_roots_expression(mount, policy.include_roots())
    };
    let mut terms = vec![include];
    if !policy.exclude_roots().is_empty() {
        terms.push(goz_core::query::Expr::Not(Box::new(
            scoped_roots_expression(mount, policy.exclude_roots()),
        )));
    }
    goz_core::query::Expr::And(terms)
}

fn scoped_roots_expression(
    mount: &str,
    roots: &[crate::index_policy::RelativePathRoot],
) -> goz_core::query::Expr {
    let separator = char::from(92).to_string();
    let mut alternatives = Vec::with_capacity(roots.len());
    for root in roots {
        let mut exact_path = mount.to_owned();
        exact_path.push_str(&root.as_str().replace("/", &separator));
        let exact = scoped_full_path_leaf(&exact_path, true);
        exact_path.push(char::from(92));
        let descendants = scoped_full_path_leaf(&exact_path, false);
        alternatives.push(goz_core::query::Expr::Or(vec![exact, descendants]));
    }
    goz_core::query::Expr::Or(alternatives)
}

fn scoped_full_path_leaf(path: &str, exact: bool) -> goz_core::query::Expr {
    goz_core::query::Expr::Leaf(goz_core::query::Leaf::Text {
        target: goz_core::query::Target::FullPath,
        needle: goz_core::fold::fold_exact(path.as_bytes()),
        case: false,
        diacritics: true,
        anchors: goz_core::query::Anchors {
            start: true,
            end: exact,
            word_start: false,
            word_end: false,
        },
        skip: goz_core::query::Skip::default(),
    })
}
fn join_relative(root: &NativePath, relative: &[u8]) -> NativePath {
    #[cfg(windows)]
    {
        let mut units = root.0.clone();
        if !units.ends_with(&[92]) {
            units.push(92);
        }
        units.extend(goz_core::wtf8::to_utf16(relative));
        NativePath(units)
    }
    #[cfg(not(windows))]
    {
        NativePath::new(
            root.to_path()
                .join(goz_core::wtf8::to_string_lossy(relative).replace('\\', "/")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scoped_index_finds_unicode_and_returns_real_paths() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("자료")).unwrap();
        let file = dir.path().join("자료/보고서.txt");
        fs::write(&file, b"hello").unwrap();
        let mut index = FolderIndex::new(NativePath::new(dir.path())).unwrap();
        while !index.status().complete {
            index.step(2);
        }
        let result = index.query("보고서").unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].path.to_path(), file);
        assert!(index.query("ext:txt").unwrap().total == 1);
    }
    #[cfg(unix)]
    #[test]
    fn index_does_not_follow_symlink_loops() {
        use std::os::unix::fs::symlink;
        let d = tempfile::tempdir().unwrap();
        symlink(d.path(), d.path().join("loop")).unwrap();
        let mut i = FolderIndex::new(NativePath::new(d.path())).unwrap();
        for _ in 0..10 {
            i.step(4);
        }
        assert!(i.status().complete);
        assert!(i.status().entries < 5);
    }
    #[test]
    fn results_mark_incomplete_snapshot() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.txt"), b"x").unwrap();
        let i = FolderIndex::new(NativePath::new(d.path())).unwrap();
        assert!(!i.query("a").unwrap().complete);
    }
    fn make_scope(includes: &[&str], excludes: &[&str]) -> crate::index_policy::ScopePolicy {
        crate::index_policy::ScopePolicy::new(
            includes
                .iter()
                .map(|root| crate::index_policy::RelativePathRoot::parse(root).unwrap())
                .collect(),
            excludes
                .iter()
                .map(|root| crate::index_policy::RelativePathRoot::parse(root).unwrap())
                .collect(),
        )
        .unwrap()
    }

    fn completed_index(root: &std::path::Path) -> FolderIndex {
        let mut index = FolderIndex::new(NativePath::new(root)).unwrap();
        while !index.done {
            index.step(1024);
        }
        index
    }

    fn scoped_query(
        index: &FolderIndex,
        text: &str,
        policy: &crate::index_policy::ScopePolicy,
    ) -> Result<SearchResult, String> {
        query_index_with_scope(
            &index.index,
            &index.root,
            text,
            index.done,
            Some(policy),
            &Budget::unlimited(),
        )
    }

    #[test]
    fn scope_is_applied_before_the_200_hit_page_and_total() {
        let dir = tempfile::tempdir().unwrap();
        let excluded = dir.path().join("a-excluded");
        let included = dir.path().join("z-included");
        fs::create_dir(&excluded).unwrap();
        fs::create_dir(&included).unwrap();
        for number in 0..205 {
            fs::write(excluded.join(format!("a{number:03}-hit.txt")), b"x").unwrap();
            fs::write(included.join(format!("z{number:03}-hit.txt")), b"x").unwrap();
        }
        let index = completed_index(dir.path());
        let unscoped = index.query("hit").unwrap();
        assert_eq!(unscoped.total, 410);
        assert_eq!(unscoped.hits.len(), 200);
        assert!(
            unscoped
                .hits
                .iter()
                .all(|hit| hit.path.to_path().starts_with(&excluded))
        );

        let policy = make_scope(&["z-included"], &[]);
        let scoped = scoped_query(&index, "hit", &policy).unwrap();
        assert_eq!(scoped.total, 205);
        assert_eq!(scoped.hits.len(), 200);
        assert!(
            scoped
                .hits
                .iter()
                .all(|hit| hit.path.to_path().starts_with(&included))
        );
    }

    #[test]
    fn scope_boundaries_or_not_wildcards_filters_and_empty_policy() {
        let dir = tempfile::tempdir().unwrap();
        let allowed = dir.path().join("allowed");
        fs::create_dir_all(allowed.join("private")).unwrap();
        let prefix_sibling = dir.path().join("allowedness");
        fs::create_dir(&prefix_sibling).unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(allowed.join("match-root.txt"), b"x").unwrap();
        fs::write(allowed.join("keep.dat"), b"x").unwrap();
        fs::write(allowed.join("private/match-secret.txt"), b"x").unwrap();
        fs::write(prefix_sibling.join("match-prefix.txt"), b"x").unwrap();
        fs::write(outside.join("match-outside.txt"), b"x").unwrap();
        let index = completed_index(dir.path());
        let policy = make_scope(&["allowed"], &["allowed/private"]);

        let root_itself = scoped_query(&index, "name:allowed", &policy).unwrap();
        assert_eq!(root_itself.total, 1);
        assert_eq!(root_itself.hits[0].path.to_path(), allowed);
        assert_eq!(
            scoped_query(&index, "match-prefix", &policy).unwrap().total,
            0
        );
        assert_eq!(
            scoped_query(&index, "path:allowedness", &policy)
                .unwrap()
                .total,
            0
        );
        assert_eq!(scoped_query(&index, "ext:dat", &policy).unwrap().total, 1);

        for query in ["match-root|match-outside", "!never-present", "*.txt"] {
            let result = scoped_query(&index, query, &policy).unwrap();
            assert!(
                result
                    .hits
                    .iter()
                    .all(|hit| hit.path.to_path().starts_with(&allowed)),
                "{query}"
            );
        }
        assert_eq!(scoped_query(&index, "*.txt", &policy).unwrap().total, 1);

        let exclude_only = make_scope(&[], &["allowed/private"]);
        let excluded = scoped_query(&index, "match", &exclude_only).unwrap();
        assert_eq!(excluded.total, 3);
        assert!(
            excluded
                .hits
                .iter()
                .all(|hit| { !hit.path.to_path().starts_with(allowed.join("private")) })
        );
        assert!(
            excluded
                .hits
                .iter()
                .any(|hit| hit.path.to_path().starts_with(&outside))
        );

        let empty = make_scope(&[], &[]);
        let legacy = index.query("match").unwrap();
        let empty_scope = scoped_query(&index, "match", &empty).unwrap();
        assert_eq!(empty_scope.total, legacy.total);
        assert_eq!(
            empty_scope
                .hits
                .iter()
                .map(|hit| hit.path.to_path())
                .collect::<Vec<_>>(),
            legacy
                .hits
                .iter()
                .map(|hit| hit.path.to_path())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn scope_folds_case_and_unicode_but_keeps_accents() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["자료", "Case", "café", "cafe"] {
            let folder = dir.path().join(name);
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("target.txt"), b"x").unwrap();
        }
        let index = completed_index(dir.path());
        let nfd = "cafe\u{301}";
        let policy = make_scope(&["자료", "case", nfd], &[]);
        let result = scoped_query(&index, "target", &policy).unwrap();
        assert_eq!(result.total, 3);
        assert_eq!(result.hits.len(), 3);
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.path.to_path() == dir.path().join("자료/target.txt"))
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.path.to_path() == dir.path().join("Case/target.txt"))
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.path.to_path() == dir.path().join("café/target.txt"))
        );
    }

    fn test_frn(value: u64) -> Frn {
        Frn(value | (1u64 << 48))
    }

    fn test_layout_name(parent_frn: Frn, name: &str) -> goz_core::layout::LayoutName {
        goz_core::layout::LayoutName {
            parent_frn,
            name: name.as_bytes().to_vec(),
            name_lossy: false,
            dos_only: false,
        }
    }

    fn hardlink_index() -> VolumeIndex {
        use goz_core::layout::LayoutFile;
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
        let scoped_dir = test_frn(100);
        let outside_dir = test_frn(101);
        index.insert_layout(&LayoutFile {
            frn: scoped_dir,
            attributes: FILE_ATTRIBUTE_DIRECTORY,
            names: vec![test_layout_name(NTFS_ROOT_FRN, "scope")],
            ..LayoutFile::default()
        });
        index.insert_layout(&LayoutFile {
            frn: outside_dir,
            attributes: FILE_ATTRIBUTE_DIRECTORY,
            names: vec![test_layout_name(NTFS_ROOT_FRN, "outside")],
            ..LayoutFile::default()
        });
        index.insert_layout(&LayoutFile {
            frn: test_frn(102),
            attributes: 0x20,
            names: vec![
                test_layout_name(scoped_dir, "linked-hit.txt"),
                test_layout_name(outside_dir, "linked-hit.txt"),
            ],
            ..LayoutFile::default()
        });
        index.optimize_storage();
        index
    }

    #[test]
    fn scope_uses_each_hardlink_path_after_an_ancestor_rename() {
        use goz_core::usn::record::{USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME};
        let dir = tempfile::tempdir().unwrap();
        let root = NativePath::new(dir.path());
        let mut index = hardlink_index();
        let before = query_index(&index, &root, "linked-hit", true).unwrap();
        assert_eq!(before.total, 2);
        let before_scope = make_scope(&["scope"], &[]);
        let before_scoped = query_index_with_scope(
            &index,
            &root,
            "linked-hit",
            true,
            Some(&before_scope),
            &Budget::unlimited(),
        )
        .unwrap();
        assert_eq!(before_scoped.total, 1);
        assert_eq!(
            before_scoped.hits[0].path.to_path(),
            dir.path().join("scope/linked-hit.txt"),
        );

        index.apply_batch(&[ParsedUsnRecord {
            major_version: 2,
            frn: test_frn(100),
            parent_frn: NTFS_ROOT_FRN,
            usn: 0,
            timestamp_ft: 0,
            reason: USN_REASON_RENAME_OLD_NAME | USN_REASON_RENAME_NEW_NAME,
            attributes: FILE_ATTRIBUTE_DIRECTORY,
            name: b"renamed".to_vec(),
            name_lossy: false,
        }]);
        let renamed_scope = make_scope(&["renamed"], &[]);
        let renamed = query_index_with_scope(
            &index,
            &root,
            "linked-hit",
            true,
            Some(&renamed_scope),
            &Budget::unlimited(),
        )
        .unwrap();
        assert_eq!(renamed.total, 1);
        assert_eq!(
            renamed.hits[0].path.to_path(),
            dir.path().join("renamed/linked-hit.txt"),
        );
        let stale_scope = make_scope(&["scope"], &[]);
        assert_eq!(
            query_index_with_scope(
                &index,
                &root,
                "linked-hit",
                true,
                Some(&stale_scope),
                &Budget::unlimited(),
            )
            .unwrap()
            .total,
            0,
        );
    }

    #[test]
    fn scoped_graph_queries_are_rejected_but_remain_available_unscoped() {
        let dir = tempfile::tempdir().unwrap();
        let root = NativePath::new(dir.path());
        let index = hardlink_index();
        let policy = make_scope(&["scope"], &[]);
        for query in [
            "child:linked-hit",
            "child:linked-hit|linked-hit",
            "!child:linked-hit",
            "parent-count:1",
            "parents:1",
            "total-child-size:0",
        ] {
            assert!(query_index(&index, &root, query, true).is_ok(), "{query}");
            let error = query_index_with_scope(
                &index,
                &root,
                query,
                true,
                Some(&policy),
                &Budget::unlimited(),
            )
            .unwrap_err();
            assert!(
                error.contains("Related, Count, TotalChildSize"),
                "{query}: {error}"
            );
        }
    }

    #[test]
    fn scoped_query_preserves_empty_invalid_and_budget_behavior() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("scope")).unwrap();
        fs::write(dir.path().join("scope/item.txt"), b"x").unwrap();
        let index = completed_index(dir.path());
        let policy = make_scope(&["scope"], &[]);
        let empty = scoped_query(&index, "  ", &policy).unwrap();
        assert_eq!(empty.total, 0);
        assert!(empty.hits.is_empty());
        assert!(scoped_query(&index, "<", &policy).is_err());
        assert!(
            scoped_query(&index, &"x".repeat(4097), &policy)
                .unwrap_err()
                .contains("검색어가 너무 깁니다")
        );

        let cancelled = AtomicBool::new(true);
        let cancelled_budget = Budget::until(Instant::now() + Duration::from_secs(10), &cancelled);
        let cancelled_error = query_index_with_scope(
            &index.index,
            &index.root,
            "<",
            true,
            Some(&policy),
            &cancelled_budget,
        )
        .unwrap_err();
        assert!(cancelled_error.contains("초과"));

        let not_cancelled = AtomicBool::new(false);
        let expired_budget = Budget::until(Instant::now() - Duration::from_secs(1), &not_cancelled);
        let expired_error = query_index_with_scope(
            &index.index,
            &index.root,
            "item",
            true,
            Some(&policy),
            &expired_budget,
        )
        .unwrap_err();
        assert!(expired_error.contains("초과"));
    }
}
