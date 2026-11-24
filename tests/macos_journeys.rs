#![cfg(target_os = "macos")]

use sdir::{
    model::{FileAction, NativePath},
    platform::run_action,
    settings::{Settings, load_at, save_at},
};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::Duration,
};

#[test]
fn create_rename_copy_move_and_restore_workspace() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let base = NativePath::new(root);
    let folder = NativePath::new(root.join("제안 자료"));
    let archive = NativePath::new(root.join("업무"));
    for name in ["제안 자료", "업무"] {
        let result = run_action(&FileAction::NewFolder {
            destination: base.clone(),
            name: name.into(),
        });
        assert!(result.ok, "{}", result.message);
    }
    let source = NativePath::new(root.join("회의.txt"));
    fs::write(source.to_path(), b"fixture content").unwrap();
    let renamed = NativePath::new(root.join("회의 결과.txt"));
    let rename = run_action(&FileAction::Rename {
        source: source.clone(),
        name: renamed.name(),
    });
    assert!(rename.ok, "{}", rename.message);
    assert_eq!(rename.destinations, vec![renamed.clone()]);
    assert!(!source.to_path().exists());

    let copy = run_action(&FileAction::Copy {
        sources: vec![renamed.clone()],
        destination: folder.clone(),
    });
    assert!(copy.ok, "{}", copy.message);
    let copy_path = NativePath::new(folder.to_path().join(renamed.name()));
    assert_eq!(copy.destinations, vec![copy_path.clone()]);
    assert_eq!(fs::read(copy_path.to_path()).unwrap(), b"fixture content");

    fs::write(copy_path.to_path(), b"preserve the existing copy").unwrap();
    let conflict = run_action(&FileAction::Copy {
        sources: vec![renamed.clone()],
        destination: folder.clone(),
    });
    assert!(!conflict.ok);
    assert_eq!(
        fs::read(copy_path.to_path()).unwrap(),
        b"preserve the existing copy"
    );

    let moved = run_action(&FileAction::Move {
        sources: vec![renamed.clone()],
        destination: archive.clone(),
    });
    assert!(moved.ok, "{}", moved.message);
    let moved_path = archive.to_path().join(renamed.name());
    assert!(!renamed.to_path().exists());
    assert_eq!(fs::read(&moved_path).unwrap(), b"fixture content");
    let undo = run_action(&FileAction::History {
        record: moved.undo.expect("undo for move"),
    });
    assert!(undo.ok, "{}", undo.message);
    assert_eq!(fs::read(renamed.to_path()).unwrap(), b"fixture content");
    assert!(!moved_path.exists());

    let settings = Settings {
        layout: 2,
        panes: vec![vec![folder], vec![archive]],
        ..Settings::default()
    };
    let profile = root.join("profile");
    save_at(&profile, &settings).unwrap();
    let restored = load_at(&profile);
    assert_eq!(restored.layout, settings.layout);
    assert_eq!(restored.panes, settings.panes);
}

#[test]
fn panel_worker_observes_an_external_create_rename_and_delete() {
    use sdir::worker::{Event, Request};
    let fixture = tempfile::tempdir().unwrap();
    let path = NativePath::new(fixture.path());
    let mut worker = Command::new(env!("CARGO_BIN_EXE_sdir"))
        .arg("--worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = worker.stdin.take().unwrap();
    let stdout = worker.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(event) = serde_json::from_str::<Event>(&line) {
                let _ = send.send(event);
            }
        }
    });
    let exercise = (|| -> Result<(), String> {
        serde_json::to_writer(
            &mut stdin,
            &Request::Watch {
                pane: 0,
                id: 7,
                path,
            },
        )
        .map_err(|error| error.to_string())?;
        writeln!(stdin).map_err(|error| error.to_string())?;
        let event = || {
            receive
                .recv_timeout(Duration::from_secs(10))
                .map_err(|err| err.to_string())
        };
        match event()? {
            Event::WatchReady { pane: 0, id: 7 } => {}
            event => {
                return Err(format!(
                    "감시 준비 실패: {}",
                    serde_json::to_string(&event).unwrap()
                ));
            }
        }
        let original = fixture.path().join("새 문서.txt");
        let renamed = fixture.path().join("고친 문서.txt");
        fs::write(&original, b"fixture").map_err(|error| error.to_string())?;
        if !matches!(event()?, Event::Changed { pane: 0, id: 7, .. }) {
            return Err("파일 생성 알림을 받지 못했습니다.".into());
        }
        fs::rename(&original, &renamed).map_err(|error| error.to_string())?;
        if !matches!(event()?, Event::Changed { pane: 0, id: 7, .. }) || !renamed.is_file() {
            return Err("파일 이름 변경을 확인하지 못했습니다.".into());
        }
        fs::remove_file(&renamed).map_err(|error| error.to_string())?;
        if !matches!(event()?, Event::Changed { pane: 0, id: 7, .. }) || renamed.exists() {
            return Err("파일 삭제를 확인하지 못했습니다.".into());
        }
        Ok(())
    })();
    drop(stdin);
    let timeout = std::time::Instant::now() + Duration::from_secs(5);
    let closed = loop {
        if let Some(_status) = worker.try_wait().unwrap() {
            break true;
        }
        if std::time::Instant::now() >= timeout {
            worker.kill().unwrap();
            worker.wait().unwrap();
            break false;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    reader.join().unwrap();
    assert!(
        closed,
        "watch worker did not exit after its input was closed"
    );
    exercise.unwrap();
}

#[test]
fn native_rename_preserves_contents_across_64_macos_workflows() {
    let fixture = tempfile::tempdir().unwrap();
    for index in 0..64 {
        let directory = fixture.path().join(format!("{index:02}"));
        fs::create_dir(&directory).unwrap();
        let original = directory.join("old name.txt");
        let name = match index % 4 {
            0 => "new name.txt",
            1 => "업무 새 문서.txt",
            2 => "résumé (2).txt",
            _ => "final.2026.txt",
        };
        let destination = directory.join(name);
        let contents = format!("original contents {index}");
        fs::write(&original, contents.as_bytes()).unwrap();
        let operation = FileAction::Rename {
            source: NativePath::new(&original),
            name: name.into(),
        };
        if index % 4 == 0 {
            fs::write(&destination, b"existing file contents").unwrap();
            let conflict = run_action(&operation);
            assert!(!conflict.ok, "collision was accepted for case {index}");
            assert_eq!(fs::read(&original).unwrap(), contents.as_bytes());
            assert_eq!(fs::read(&destination).unwrap(), b"existing file contents");
            fs::remove_file(&destination).unwrap();
        }
        let result = run_action(&operation);
        assert!(result.ok, "case {index}: {}", result.message);
        assert!(!original.exists());
        assert_eq!(fs::read(&destination).unwrap(), contents.as_bytes());
        if index % 2 == 0 {
            let undo = run_action(&FileAction::History {
                record: result.undo.expect("undo record for single rename"),
            });
            assert!(undo.ok, "case {index}: {}", undo.message);
            assert!(!destination.exists());
            assert_eq!(fs::read(&original).unwrap(), contents.as_bytes());
            let redo = run_action(&FileAction::History {
                record: undo.undo.expect("redo record for single rename"),
            });
            assert!(redo.ok, "case {index}: {}", redo.message);
            assert!(!original.exists());
            assert_eq!(fs::read(&destination).unwrap(), contents.as_bytes());
        }
        println!("SDIR_MAC_NATIVE_RENAME_PASS {index}");
    }
}
