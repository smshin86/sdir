use sdir::{
    model::NativePath,
    worker::{Event, Request},
};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Worker {
    child: Child,
    events: mpsc::Receiver<Event>,
}
impl Worker {
    fn new(search: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sdir"));
        command
            .arg(if search {
                "--search-worker"
            } else {
                "--worker"
            })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let event: Event = serde_json::from_str(&line).expect("valid worker protocol");
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        Self { child, events }
    }
    fn send(&mut self, request: Request) {
        let out = self.child.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *out, &request).unwrap();
        writeln!(out).unwrap();
        out.flush().unwrap();
    }
    fn next(&self) -> Event {
        self.events
            .recv_timeout(Duration::from_secs(15))
            .expect("worker responds within deadline")
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn listing_worker_streams_every_batch_and_finishes() {
    let d = tempfile::tempdir().unwrap();
    for i in 0..600 {
        std::fs::write(d.path().join(format!("보고서-{i}.txt")), b"test").unwrap();
    }
    let mut worker = Worker::new(false);
    worker.send(Request::List {
        pane: 2,
        id: 47,
        path: NativePath::new(d.path()),
        hidden: true,
    });
    let (mut count, mut batches) = (0, 0);
    loop {
        match worker.next() {
            Event::Batch { pane, id, entries } => {
                assert_eq!((pane, id), (2, 47));
                count += entries.len();
                batches += 1;
                assert!(entries.len() <= 1024);
                if batches == 1 {
                    assert!(entries.len() <= 256);
                }
                for entry in entries {
                    let entry = entry.expand(d.path());
                    assert_eq!(entry.bytes, Some(4));
                    assert_eq!(entry.path.parent(), Some(NativePath::new(d.path())));
                    assert!(entry.name.starts_with("보고서-"));
                }
            }
            Event::Listed { pane, id, error } => {
                assert_eq!((pane, id), (2, 47));
                assert!(error.is_none());
                break;
            }
            e => panic!("unexpected {e:?}"),
        }
    }
    assert_eq!(count, 600);
    assert!(batches >= 2);
}

#[test]
fn search_worker_indexes_only_selected_root_and_preserves_query_ids() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("nested")).unwrap();
    std::fs::write(d.path().join("nested/회의록.txt"), b"hello").unwrap();
    std::fs::write(d.path().join("other.bin"), b"x").unwrap();
    let mut worker = Worker::new(true);
    worker.send(Request::Index {
        id: 9,
        root: NativePath::new(d.path()),
    });
    loop {
        if let Event::Index { index_id, status } = worker.next() {
            assert_eq!(index_id, 9);
            if status.complete {
                break;
            }
        }
    }
    worker.send(Request::Query {
        id: 55,
        text: "회의록".into(),
    });
    loop {
        if let Event::Search { id, result } = worker.next() {
            assert_eq!(id, 55);
            let result = result.unwrap();
            assert_eq!(result.total, 1);
            assert_eq!(
                result.hits[0].path.to_path(),
                d.path().join("nested/회의록.txt")
            );
            assert!(result.complete);
            break;
        }
    }
    worker.send(Request::Query {
        id: 56,
        text: "ext:bin".into(),
    });
    loop {
        if let Event::Search { id, result } = worker.next() {
            assert_eq!(id, 56);
            assert_eq!(result.unwrap().total, 1);
            break;
        }
    }
}

#[test]
fn unreadable_folder_reports_completion_with_error() {
    let d = tempfile::tempdir().unwrap();
    let mut worker = Worker::new(false);
    worker.send(Request::List {
        pane: 0,
        id: 1,
        path: NativePath::new(d.path().join("missing")),
        hidden: true,
    });
    match worker.next() {
        Event::Listed { error, .. } => assert!(error.is_some()),
        e => panic!("unexpected {e:?}"),
    }
}

#[cfg(windows)]
#[test]
fn native_file_jobs_copy_rename_move_inside_temporary_directory() {
    use sdir::model::FileAction;
    use sdir::platform::run_action;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let dest = root.path().join("dest");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&dest).unwrap();
    let file = source.join("한글.txt");
    std::fs::write(&file, b"sdir-smoke").unwrap();
    let actions = [
        FileAction::Copy {
            sources: vec![NativePath::new(&file)],
            destination: NativePath::new(&dest),
        },
        FileAction::Rename {
            source: NativePath::new(dest.join("한글.txt")),
            name: "renamed.txt".into(),
        },
        FileAction::NewFolder {
            destination: NativePath::new(&dest),
            name: "target".into(),
        },
        FileAction::Move {
            sources: vec![NativePath::new(dest.join("renamed.txt"))],
            destination: NativePath::new(dest.join("target")),
        },
    ];
    for action in actions {
        let result = run_action(&action);
        assert!(result.ok, "{result:?}");
        assert_eq!(
            result.destinations.len(),
            1,
            "Shell must return one top-level result: {result:?}"
        );
        assert!(result.destinations[0].to_path().exists());
    }
    assert_eq!(
        std::fs::read(dest.join("target/renamed.txt")).unwrap(),
        b"sdir-smoke"
    );
    assert!(file.exists());
    assert!(!dest.join("renamed.txt").exists());
    let result = run_action(&FileAction::Copy {
        sources: vec![NativePath::new(&file)],
        destination: NativePath::new(&source),
    });
    assert!(result.ok, "same-folder copy creates a new name: {result:?}");
    let copies: Vec<_> = std::fs::read_dir(&source)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(copies.len(), 2);
    assert_eq!(result.destinations.len(), 1);
    assert_ne!(result.destinations[0], NativePath::new(&file));
    assert!(copies.contains(&result.destinations[0].to_path()));
    for copy in copies {
        assert_eq!(std::fs::read(copy).unwrap(), b"sdir-smoke");
    }
    let tree = source.join("tree");
    std::fs::create_dir_all(tree.join("nested")).unwrap();
    std::fs::write(tree.join("nested/inside.txt"), b"nested-content").unwrap();
    let result = run_action(&FileAction::Copy {
        sources: vec![NativePath::new(&tree)],
        destination: NativePath::new(&dest),
    });
    assert!(result.ok, "{result:?}");
    assert_eq!(
        result.destinations,
        vec![NativePath::new(dest.join("tree"))],
        "nested Shell callbacks must not replace the top-level selection"
    );
    assert_eq!(
        std::fs::read(dest.join("tree/nested/inside.txt")).unwrap(),
        b"nested-content"
    );
    let mixed = root.path().join("CaseDestination_Δ");
    std::fs::create_dir(&mixed).unwrap();
    let requested = std::path::PathBuf::from(mixed.to_string_lossy().to_lowercase());
    let result = run_action(&FileAction::Copy {
        sources: vec![NativePath::new(&file)],
        destination: NativePath::new(&requested),
    });
    assert!(result.ok, "{result:?}");
    assert_eq!(
        result.destinations,
        vec![NativePath::new(requested.join("한글.txt"))]
    );
    assert_eq!(
        std::fs::read(mixed.join("한글.txt")).unwrap(),
        b"sdir-smoke"
    );
}

#[cfg(windows)]
#[test]
fn native_app_history_copy_move_rename_folder_and_recycle_round_trip() {
    use sdir::{model::FileAction, platform::run_action};
    fn reverse(result: sdir::model::JobResult) -> sdir::model::JobResult {
        assert!(result.ok, "{result:?}");
        let record = result
            .undo
            .expect("completed operation has inverse receipt");
        let result = run_action(&FileAction::History { record });
        assert!(result.ok, "{result:?}");
        result
    }
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("A");
    let b = d.path().join("B");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let original = a.join("한글.txt");
    std::fs::write(&original, b"history-content").unwrap();
    let copy = run_action(&FileAction::Copy {
        sources: vec![NativePath::new(&original)],
        destination: NativePath::new(&b),
    });
    let undo = reverse(copy);
    assert!(!b.join("한글.txt").exists());
    let _ = reverse(undo);
    assert_eq!(
        std::fs::read(b.join("한글.txt")).unwrap(),
        b"history-content"
    );
    let moved = run_action(&FileAction::Move {
        sources: vec![NativePath::new(&original)],
        destination: NativePath::new(&b.join("nested")),
    });
    assert!(!moved.ok);
    assert!(original.exists());
    let moved = run_action(&FileAction::Rename {
        source: NativePath::new(&original),
        name: "renamed.txt".into(),
    });
    let undo = reverse(moved);
    assert!(original.exists());
    let _ = reverse(undo);
    assert!(a.join("renamed.txt").exists());
    let moved = run_action(&FileAction::Move {
        sources: vec![NativePath::new(a.join("renamed.txt"))],
        destination: NativePath::new(&b),
    });
    let undo = reverse(moved);
    assert!(a.join("renamed.txt").exists());
    let _ = reverse(undo);
    assert!(b.join("renamed.txt").exists());
    let folder = run_action(&FileAction::NewFolder {
        destination: NativePath::new(&a),
        name: "empty".into(),
    });
    let undo = reverse(folder);
    assert!(!a.join("empty").exists());
    let _ = reverse(undo);
    assert!(a.join("empty").is_dir());
    let recycle = run_action(&FileAction::Recycle {
        sources: vec![NativePath::new(b.join("renamed.txt"))],
    });
    let restored = reverse(recycle);
    assert_eq!(
        std::fs::read(b.join("renamed.txt")).unwrap(),
        b"history-content"
    );
    let recycled = reverse(restored);
    assert!(!b.join("renamed.txt").exists());
    let _ = reverse(recycled);
    assert!(b.join("renamed.txt").exists());
    let copy = run_action(&FileAction::Copy {
        sources: vec![NativePath::new(b.join("renamed.txt"))],
        destination: NativePath::new(&a),
    });
    let record = copy.undo.unwrap();
    std::fs::write(a.join("renamed.txt"), b"user-edited-content").unwrap();
    let refused = run_action(&FileAction::History { record });
    assert!(!refused.ok);
    assert_eq!(
        std::fs::read(a.join("renamed.txt")).unwrap(),
        b"user-edited-content"
    );
}

#[test]
fn tree_worker_lists_only_immediate_folders_and_reports_unavailable_paths() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("업무")).unwrap();
    std::fs::create_dir(d.path().join("업무/nested")).unwrap();
    std::fs::write(d.path().join("file.txt"), b"not a folder").unwrap();
    let mut worker = Worker::new(false);
    worker.send(Request::Tree {
        id: 71,
        path: NativePath::new(d.path()),
        hidden: true,
    });
    match worker.next() {
        Event::Tree {
            id,
            result: Ok(listing),
        } => {
            assert_eq!(id, 71);
            assert_eq!(
                listing.folders,
                vec![NativePath::new(d.path().join("업무"))]
            );
            assert!(listing.warning.is_none());
        }
        e => panic!("unexpected {e:?}"),
    }
    let mut missing = Worker::new(false);
    missing.send(Request::Tree {
        id: 72,
        path: NativePath::new(d.path().join("missing")),
        hidden: false,
    });
    assert!(matches!(
        missing.next(),
        Event::Tree {
            id: 72,
            result: Err(_)
        }
    ));
}

#[test]
fn batch_preview_worker_is_read_only_and_rejects_collision() {
    let d = tempfile::tempdir().unwrap();
    let a = NativePath::new(d.path().join("a.txt"));
    std::fs::write(a.to_path(), b"original").unwrap();
    std::fs::write(d.path().join("x-a.txt"), b"existing").unwrap();
    let mut worker = Worker::new(false);
    worker.send(Request::Preview {
        id: 51,
        paths: vec![a.clone()],
        pattern: sdir::batch::Pattern {
            prefix: "x-".into(),
            ..Default::default()
        },
    });
    match worker.next() {
        Event::Preview { id, result } => {
            assert_eq!(id, 51);
            assert!(result.is_err())
        }
        e => panic!("{e:?}"),
    };
    assert_eq!(std::fs::read(a.to_path()).unwrap(), b"original");
    assert_eq!(
        std::fs::read(d.path().join("x-a.txt")).unwrap(),
        b"existing"
    );
}
#[cfg(windows)]
#[test]
fn native_watch_reports_external_changes_and_unavailable_root() {
    let d = tempfile::tempdir().unwrap();
    let mut worker = Worker::new(false);
    worker.send(Request::Watch {
        pane: 1,
        id: 85,
        path: NativePath::new(d.path()),
    });
    assert!(matches!(
        worker.next(),
        Event::WatchReady { pane: 1, id: 85 }
    ));
    std::fs::write(d.path().join("watch-owned.txt"), b"value").unwrap();
    assert!(matches!(
        worker.next(),
        Event::Changed {
            pane: 1,
            id: 85,
            ..
        }
    ));
    drop(worker.child.stdin.take());
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while worker.child.try_wait().unwrap().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "watch worker must exit on parent EOF"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut invalid = Worker::new(false);
    invalid.send(Request::Watch {
        pane: 0,
        id: 86,
        path: NativePath::new(d.path().join("missing")),
    });
    assert!(matches!(
        invalid.next(),
        Event::WatchError {
            pane: 0,
            id: 86,
            ..
        }
    ));
}
#[cfg(windows)]
#[test]
fn batch_native_receipts_and_undo_preserve_bytes() {
    use sdir::{model::FileAction, platform::run_action};
    let d = tempfile::tempdir().unwrap();
    let mut paths = vec![];
    for n in ["a.txt", "b.txt"] {
        let p = NativePath::new(d.path().join(n));
        std::fs::write(p.to_path(), n).unwrap();
        paths.push(p)
    }
    let pairs = sdir::batch::preview(
        &paths,
        &sdir::batch::Pattern {
            prefix: "new-".into(),
            start: Some(3),
            ..Default::default()
        },
    )
    .unwrap();
    let result = run_action(&FileAction::BatchRename { pairs });
    assert!(result.ok, "{}", result.message);
    assert_eq!(result.outcomes.len(), 2);
    assert!(result.outcomes.iter().all(|o| o.ok));
    assert_eq!(
        std::fs::read(d.path().join("new-a_3.txt")).unwrap(),
        b"a.txt"
    );
    assert_eq!(result.destinations.len(), 2);
    let undo = run_action(&FileAction::History {
        record: result.undo.unwrap(),
    });
    assert!(undo.ok, "{}", undo.message);
    assert_eq!(std::fs::read(paths[1].to_path()).unwrap(), b"b.txt");
    assert!(!d.path().join("new-a_3.txt").exists());
}
