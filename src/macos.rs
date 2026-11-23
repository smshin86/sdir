use crate::{
    file_appearance::{FileAppearance, FileAppearanceKey},
    history::{Change, ChangeKind, ItemSource, ItemTarget},
    model::{FileAction, ItemOutcome, NativePath},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use objc2::{rc::autoreleasepool, runtime::ProtocolObject};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardItem, NSWorkspace,
};
use objc2_foundation::{NSArray, NSDictionary, NSFileManager, NSString, NSURL};

fn url(path: &NativePath) -> objc2::rc::Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.display()))
}

pub(crate) fn open(path: &NativePath) -> Result<(), String> {
    autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .openURL(&url(path))
            .then_some(())
            .ok_or_else(|| "파일을 열 수 없습니다.".into())
    })
}

pub(crate) fn properties(path: &NativePath) -> Result<(), String> {
    std::fs::symlink_metadata(path.to_path()).map_err(|error| error.to_string())?;
    reveal(path);
    Ok(())
}

pub(crate) fn reveal(path: &NativePath) {
    autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url(path)]));
    });
}

pub(crate) fn clip_sequence() -> u32 {
    autoreleasepool(|_| NSPasteboard::generalPasteboard().changeCount() as u32)
}

pub(crate) fn clip_write(paths: &[NativePath], cut: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Err("복사할 항목을 선택해 주세요.".into());
    }
    autoreleasepool(|_| {
        let file_type = NSString::from_str("public.file-url");
        let cut_type = NSString::from_str("com.sdir.clipboard.cut");
        let items = paths
            .iter()
            .map(|path| {
                let value = url(path)
                    .absoluteString()
                    .ok_or("파일 URL을 만들 수 없습니다.")?;
                let item = NSPasteboardItem::new();
                if !item.setString_forType(&value, &file_type)
                    || (cut && !item.setString_forType(&NSString::from_str("1"), &cut_type))
                {
                    return Err("클립보드 항목을 만들 수 없습니다.");
                }
                Ok(item)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let items: Vec<_> = items
            .iter()
            .map(|item| ProtocolObject::from_ref(&**item))
            .collect();
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard
            .writeObjects(&NSArray::from_slice(&items))
            .then_some(())
            .ok_or_else(|| "파일 목록을 클립보드에 쓰지 못했습니다.".into())
    })
}

pub(crate) fn clip_read() -> Result<(Vec<NativePath>, bool), String> {
    autoreleasepool(|_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        let sequence = pasteboard.changeCount();
        let file_type = NSString::from_str("public.file-url");
        let cut_type = NSString::from_str("com.sdir.clipboard.cut");
        let mut cut = true;
        let mut paths = Vec::new();
        if let Some(items) = pasteboard.pasteboardItems() {
            for item in items.to_vec() {
                let Some(url) = item
                    .stringForType(&file_type)
                    .and_then(|text| NSURL::URLWithString(&text))
                else {
                    continue;
                };
                if !url.isFileURL()
                    || url.host().is_some_and(|host| {
                        let host = host.to_string();
                        !host.is_empty() && host != "localhost"
                    })
                {
                    continue;
                }
                if let Some(path) = url.path().map(|path| NativePath::new(path.to_string())) {
                    if path.to_path().is_absolute() {
                        cut &= item
                            .stringForType(&cut_type)
                            .is_some_and(|value| value.to_string() == "1");
                        paths.push(path);
                    }
                }
            }
        }
        if pasteboard.changeCount() != sequence {
            return Err("클립보드가 바뀌었습니다. 붙여넣기를 다시 시도해 주세요.".into());
        }
        if paths.is_empty() {
            return Err("클립보드에 파일이나 폴더가 없습니다.".into());
        }
        Ok((paths, cut))
    })
}

pub(crate) fn copy_path_text(paths: &[NativePath]) -> Result<(), String> {
    autoreleasepool(|_| {
        let value = paths
            .iter()
            .map(NativePath::display)
            .collect::<Vec<_>>()
            .join("\n");
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard
            .setString_forType(
                &NSString::from_str(&value),
                &NSString::from_str("public.utf8-plain-text"),
            )
            .then_some(())
            .ok_or_else(|| "경로를 클립보드에 쓰지 못했습니다.".into())
    })
}

pub(crate) fn appearance(key: &FileAppearanceKey) -> FileAppearance {
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        // The native lookup falls back to a generic icon for an unregistered extension.
        #[allow(deprecated)]
        let image = workspace.iconForFileType(&NSString::from_str(&key.extension));
        let icon = (|| {
            let data = image.TIFFRepresentation()?;
            let bitmap = NSBitmapImageRep::imageRepWithData(&data)?;
            // An empty property dictionary uses the native PNG defaults.
            let png = unsafe {
                bitmap.representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &NSDictionary::new(),
                )
            }?;
            (png.length() <= 128 * 1024)
                .then(|| format!("data:image/png;base64,{}", STANDARD.encode(png.to_vec())))
        })();
        FileAppearance {
            type_name: if key.extension.is_empty() {
                "파일".into()
            } else {
                format!("{} 파일", key.extension.to_uppercase())
            },
            small: icon.clone(),
            large: icon,
        }
    })
}

#[derive(Default)]
struct Actions {
    destinations: Vec<NativePath>,
    changes: Vec<Change>,
    outcomes: Vec<ItemOutcome>,
    error: Option<String>,
}

impl Actions {
    fn record(&mut self, source: Option<&NativePath>, result: Result<Option<Change>, String>) {
        self.record_result(source, result, true);
    }

    fn record_result(
        &mut self,
        source: Option<&NativePath>,
        result: Result<Option<Change>, String>,
        reveal: bool,
    ) {
        let message = match result {
            Ok(change) => {
                if let Some(change) = change {
                    if let Some(path) = change.destination.clone().filter(|_| reveal) {
                        self.destinations.push(path);
                    }
                    self.changes.push(change);
                }
                None
            }
            Err(error) => {
                self.error.get_or_insert_with(|| error.clone());
                Some(error)
            }
        };
        if let Some(source) = source {
            self.outcomes.push(ItemOutcome {
                source: source.clone(),
                ok: message.is_none(),
                message: message.unwrap_or_else(|| "완료".into()),
            });
        }
    }
}

fn change(kind: ChangeKind, source: Option<&NativePath>, destination: NativePath) -> Change {
    Change {
        kind,
        source: source.cloned().map(ItemSource::File),
        destination: Some(destination),
        recycled: None,
        overwritten: false,
    }
}

fn transfer(
    file_manager: &NSFileManager,
    source: &NativePath,
    target: NativePath,
    moving: bool,
) -> Result<Option<Change>, String> {
    if source == &target {
        return Err("원본과 대상이 같습니다.".into());
    }
    let from = url(source);
    let to = url(&target);
    let kind = if moving {
        file_manager
            .moveItemAtURL_toURL_error(&from, &to)
            .map_err(|error| error.to_string())?;
        ChangeKind::Move
    } else {
        file_manager
            .copyItemAtURL_toURL_error(&from, &to)
            .map_err(|error| error.to_string())?;
        ChangeKind::Copy
    };
    Ok(Some(change(kind, Some(source), target)))
}

fn trash(file_manager: &NSFileManager, source: &NativePath) -> Result<Option<Change>, String> {
    let mut result = None;
    file_manager
        .trashItemAtURL_resultingItemURL_error(&url(source), Some(&mut result))
        .map_err(|error| error.to_string())?;
    Ok(result.and_then(|url| url.path()).map(|path| {
        change(
            ChangeKind::Move,
            Some(source),
            NativePath::new(path.to_string()),
        )
    }))
}

fn rename(
    file_manager: &NSFileManager,
    source: &NativePath,
    name: &str,
) -> Result<Option<Change>, String> {
    let parent = source.parent().ok_or("볼륨의 이름은 변경할 수 없습니다.")?;
    let dest = NativePath::new(parent.to_path().join(name));
    if source == &dest {
        return Ok(None);
    }
    transfer(file_manager, source, dest, true)
}

pub(crate) fn perform(
    action: &FileAction,
    progress: impl Fn(u8),
) -> (
    Result<bool, String>,
    Vec<NativePath>,
    Vec<Change>,
    Vec<ItemOutcome>,
) {
    autoreleasepool(|_| {
        let manager = NSFileManager::defaultManager();
        let mut actions = Actions::default();
        match action {
            FileAction::NewFolder { destination, name } => {
                let path = NativePath::new(destination.to_path().join(name));
                actions.record(
                    None,
                    std::fs::create_dir(path.to_path())
                        .map(|()| Some(change(ChangeKind::New, None, path)))
                        .map_err(|error| error.to_string()),
                );
            }
            FileAction::Copy {
                sources,
                destination,
            }
            | FileAction::Move {
                sources,
                destination,
            } => {
                let moving = matches!(action, FileAction::Move { .. });
                for (index, source) in sources.iter().enumerate() {
                    let result = source
                        .to_path()
                        .file_name()
                        .map(|name| NativePath::new(destination.to_path().join(name)))
                        .ok_or("볼륨은 파일 작업의 원본으로 사용할 수 없습니다.".into())
                        .and_then(|target| transfer(&manager, source, target, moving));
                    actions.record(Some(source), result);
                    progress(((index + 1) * 100 / sources.len()) as u8);
                }
            }
            FileAction::Rename { source, name } => {
                actions.record(Some(source), rename(&manager, source, name))
            }
            FileAction::BatchRename { pairs } => {
                for pair in pairs {
                    actions.record(
                        Some(&pair.source),
                        rename(&manager, &pair.source, &pair.name),
                    );
                }
            }
            FileAction::Recycle { sources } => {
                for source in sources {
                    actions.record_result(Some(source), trash(&manager, source), false);
                }
            }
            FileAction::Delete { sources } => {
                for source in sources {
                    actions.record(
                        Some(source),
                        manager
                            .removeItemAtURL_error(&url(source))
                            .map(|()| None)
                            .map_err(|error| error.to_string()),
                    );
                }
            }
            FileAction::History { record } => {
                for item in &record.items {
                    let result = match (&item.source, &item.target) {
                        (ItemSource::File(source), ItemTarget::Path(target)) => {
                            transfer(&manager, source, target.clone(), true)
                        }
                        (ItemSource::File(source), ItemTarget::Recycle) => trash(&manager, source),
                        _ => Err("이 휴지통 기록은 macOS에서 복원할 수 없습니다.".into()),
                    };
                    actions.record_result(
                        Some(item.source.path()),
                        result,
                        !matches!(item.target, ItemTarget::Recycle),
                    );
                }
            }
        }
        (
            actions.error.map_or(Ok(false), Err),
            actions.destinations,
            actions.changes,
            actions.outcomes,
        )
    })
}
