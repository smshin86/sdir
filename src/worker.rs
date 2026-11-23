use crate::{
    filesystem,
    model::{FileAction, JobResult, NativePath},
    platform,
    search::{FolderIndex, IndexStatus, SearchResult},
};
use crossbeam_channel::{Receiver, Sender, bounded};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Thumbnails {
        id: u64,
        paths: Vec<NativePath>,
    },
    ReadPreview {
        id: u64,
        path: NativePath,
    },
    Preview {
        id: u64,
        paths: Vec<NativePath>,
        pattern: crate::batch::Pattern,
    },
    Utility {
        action: crate::utility::Utility,
    },
    Watch {
        pane: usize,
        id: u64,
        path: NativePath,
    },
    Tree {
        id: u64,
        path: NativePath,
        hidden: bool,
    },
    List {
        pane: usize,
        id: u64,
        path: NativePath,
        hidden: bool,
    },
    Index {
        id: u64,
        root: NativePath,
    },
    IndexVolumes {
        id: u64,
        memory_mib: u16,
        #[serde(default)]
        service_only: bool,
        #[serde(default)]
        settings: Box<crate::index_policy::NtfsIndexingSettings>,
    },
    Configure {
        session: u64,
        generation: u64,
        settings: Box<crate::index_policy::NtfsIndexingSettings>,
    },
    Rebuild {
        session: u64,
        generation: u64,
        request_id: u64,
        guid: crate::index_policy::VolumeGuid,
    },
    VolumeQuery {
        session: u64,
        generation: u64,
        query_id: u64,
        text: String,
    },
    Query {
        id: u64,
        text: String,
    },
    Job {
        id: u64,
        action: FileAction,
    },
    Open {
        path: NativePath,
    },
    Properties {
        path: NativePath,
    },
    ClipboardSet {
        id: u64,
        paths: Vec<NativePath>,
        cut: bool,
    },
    ClipboardGet {
        id: u64,
    },
    #[serde(skip)]
    ProxyInputClosed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Thumbnails {
        id: u64,
        images: Vec<Option<String>>,
    },
    Opened {
        path: NativePath,
    },
    ReadPreview {
        id: u64,
        result: Result<crate::file_preview::Preview, String>,
    },
    Preview {
        id: u64,
        result: Result<Vec<crate::batch::RenamePair>, String>,
    },
    Utility {
        action: crate::utility::Utility,
        result: Result<Option<u32>, String>,
    },
    WatchReady {
        pane: usize,
        id: u64,
    },
    Changed {
        pane: usize,
        id: u64,
        renames: Vec<crate::watch::Rename>,
    },
    WatchError {
        pane: usize,
        id: u64,
        message: String,
    },
    Tree {
        id: u64,
        result: Result<crate::tree::Listing, String>,
    },
    Batch {
        pane: usize,
        id: u64,
        entries: Vec<crate::listing::ListingEntry>,
    },
    Listed {
        pane: usize,
        id: u64,
        error: Option<String>,
    },
    Index {
        index_id: u64,
        status: IndexStatus,
    },
    Search {
        id: u64,
        result: Result<SearchResult, String>,
    },
    VolumeIndex {
        session: u64,
        generation: u64,
        host_pid: u32,
        status: Box<IndexStatus>,
    },
    VolumeSearch {
        session: u64,
        generation: u64,
        query_id: u64,
        host_pid: Option<u32>,
        result: Result<Box<SearchResult>, String>,
    },
    ConfigAck {
        session: u64,
        generation: u64,
        host_pid: Option<u32>,
        accepted: bool,
        message: Option<String>,
    },
    RebuildAck {
        session: u64,
        generation: u64,
        request_id: u64,
        guid: crate::index_policy::VolumeGuid,
        host_pid: Option<u32>,
        accepted: bool,
        message: Option<String>,
    },
    JobProgress {
        id: u64,
        percent: u8,
    },
    Job {
        id: u64,
        result: JobResult,
    },
    Clipboard {
        id: u64,
        result: Result<(Vec<NativePath>, bool), String>,
        #[serde(default)]
        sequence: u32,
    },
    ClipboardWritten {
        id: u64,
        cut: bool,
        result: Result<(), String>,
        #[serde(default)]
        sequence: u32,
    },
    Notice {
        message: String,
    },
    Failure {
        lane: usize,
        id: u64,
        message: String,
    },
}
fn output(e: &Event) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if serde_json::to_writer(&mut out, e).is_ok() {
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}
fn parent_lifetime() {
    std::thread::spawn(|| {
        loop {
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
                #[cfg(windows)]
                unsafe {
                    let _ = windows::Win32::System::Threading::TerminateProcess(
                        windows::Win32::System::Threading::GetCurrentProcess(),
                        0,
                    );
                }
                std::process::exit(0);
            }
        }
    });
}
pub fn worker_main(search: bool) -> Result<(), String> {
    if search {
        return search_main();
    }
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let request: Request = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    if matches!(
        request,
        Request::Watch { .. } | Request::Utility { .. } | Request::Properties { .. }
    ) {
        parent_lifetime();
    }
    match request {
        Request::Thumbnails { id, paths } => output(&Event::Thumbnails {
            id,
            images: paths
                .iter()
                .take(8)
                .map(crate::file_preview::thumbnail)
                .collect(),
        }),
        Request::ReadPreview { id, path } => output(&Event::ReadPreview {
            id,
            result: crate::file_preview::read(&path),
        }),
        Request::Preview { id, paths, pattern } => output(&Event::Preview {
            id,
            result: crate::batch::preview(&paths, &pattern),
        }),
        Request::Utility { action } => output(&Event::Utility {
            result: crate::utility::run(&action),
            action,
        }),
        Request::Watch { pane, id, path } => {
            if let Err(message) = crate::watch::run(
                &path,
                || output(&Event::WatchReady { pane, id }),
                |renames| output(&Event::Changed { pane, id, renames }),
            ) {
                output(&Event::WatchError { pane, id, message });
            }
        }
        Request::Tree { id, path, hidden } => output(&Event::Tree {
            id,
            result: crate::tree::list(&path, hidden),
        }),
        Request::List {
            pane,
            id,
            path,
            hidden,
        } => {
            let result = filesystem::list_folder(&path, hidden, |entries| {
                let entries = entries.into_iter().map(Into::into).collect();
                output(&Event::Batch { pane, id, entries });
                true
            });
            let error = match result {
                Err(e) => Some(e),
                Ok(n) if n > 0 => Some(format!("읽지 못한 항목 {n}개")),
                _ => None,
            };
            output(&Event::Listed { pane, id, error });
        }
        Request::Job { id, action } => {
            let result = platform::run_action_with_progress(&action, move |percent| {
                output(&Event::JobProgress { id, percent });
            });
            output(&Event::Job { id, result });
        }
        Request::Open { path } => match platform::open_path(&path) {
            Ok(()) => output(&Event::Opened { path }),
            Err(message) => output(&Event::Notice { message }),
        },
        Request::Properties { path } => {
            let result = platform::properties(&path);
            output(&Event::Notice {
                message: match result {
                    Ok(()) => "속성 요청 처리가 끝났습니다.".into(),
                    Err(e) => format!("속성 창을 열지 못했습니다: {e}"),
                },
            });
        }
        Request::ClipboardSet { id, paths, cut } => output(&Event::ClipboardWritten {
            id,
            cut,
            result: platform::clipboard_set(&paths, cut),
            sequence: platform::clipboard_sequence(),
        }),
        Request::ClipboardGet { id } => output(&Event::Clipboard {
            id,
            result: platform::clipboard_get(),
            sequence: platform::clipboard_sequence(),
        }),
        _ => return Err("잘못된 작업 요청".into()),
    }
    Ok(())
}
fn search_main() -> Result<(), String> {
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(req) = serde_json::from_str::<Request>(&line) {
                if tx.send(req).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send(Request::ProxyInputClosed);
    });
    let mut index: Option<FolderIndex> = None;
    let mut index_id = 0;
    let mut last_status = Instant::now();
    let mut query: Option<(u64, String)> = None;
    loop {
        let first = if index.as_ref().is_none_or(|i| i.status().complete) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(x) => Some(x),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                Err(_) => None,
            }
        } else {
            rx.try_recv().ok()
        };
        let mut requests = first.into_iter().collect::<Vec<_>>();
        requests.extend(rx.try_iter());
        let mut requests = requests.into_iter();
        while let Some(request) = requests.next() {
            match request {
                Request::IndexVolumes {
                    id,
                    memory_mib,
                    service_only,
                    settings,
                } => {
                    let queued = requests.collect::<Vec<_>>();
                    if let Err(message) = crate::live_search::proxy(
                        id,
                        memory_mib,
                        service_only,
                        *settings,
                        queued,
                        rx,
                    ) {
                        output(&Event::Failure {
                            lane: 4,
                            id,
                            message: message.clone(),
                        });
                        return Err(message);
                    }
                    return Ok(());
                }
                Request::Index { id, root } => {
                    index_id = id;
                    query = None;
                    match FolderIndex::new(root) {
                        Ok(i) => index = Some(i),
                        Err(message) => {
                            output(&Event::Failure {
                                lane: 4,
                                id,
                                message,
                            });
                            index = None;
                        }
                    }
                }
                Request::Query { id, text } => query = Some((id, text)),
                Request::ProxyInputClosed => return Ok(()),
                request @ (Request::Configure { .. }
                | Request::Rebuild { .. }
                | Request::VolumeQuery { .. }) => {
                    let event = request_failure(
                        request,
                        "전체 드라이브 검색 세션이 시작되기 전인 제어 명령입니다.".into(),
                    );
                    let _ = output(&event);
                }
                _ => {}
            }
        }
        if let Some(i) = index.as_mut() {
            let was_complete = i.status().complete;
            if !was_complete {
                i.step(64);
            }
            if let Some((id, text)) = query.take() {
                output(&Event::Search {
                    id,
                    result: i.query(&text),
                });
            }
            if !was_complete
                && (i.status().complete || last_status.elapsed() > Duration::from_millis(300))
            {
                output(&Event::Index {
                    index_id,
                    status: i.status(),
                });
                last_status = Instant::now();
            }
        }
    }
    Ok(())
}

const MAX_PENDING_CONTROL_REQUESTS: usize = 8;

#[derive(Default)]
struct Pending {
    replace: Option<Request>,
    controls: std::collections::VecDeque<Request>,
    query: Option<Request>,
}

impl Pending {
    fn enqueue(&mut self, request: Request) -> Result<Option<Request>, Request> {
        if is_query_request(&request) {
            return Ok(self.query.replace(request));
        }
        if self.controls.len() >= MAX_PENDING_CONTROL_REQUESTS {
            return Err(request);
        }
        self.controls.push_back(request);
        Ok(None)
    }
}

fn is_query_request(request: &Request) -> bool {
    matches!(request, Request::Query { .. } | Request::VolumeQuery { .. })
}

fn request_failure(request: Request, message: String) -> Event {
    match request {
        Request::Thumbnails { id, .. } => Event::Thumbnails {
            id,
            images: Vec::new(),
        },
        Request::ReadPreview { id, .. } => Event::ReadPreview {
            id,
            result: Err(message),
        },
        Request::Query { id, .. } => Event::Search {
            id,
            result: Err(message),
        },
        Request::VolumeQuery {
            session,
            generation,
            query_id,
            ..
        } => Event::VolumeSearch {
            session,
            generation,
            query_id,
            host_pid: None,
            result: Err(message),
        },
        Request::Configure {
            session,
            generation,
            ..
        } => Event::ConfigAck {
            session,
            generation,
            host_pid: None,
            accepted: false,
            message: Some(message),
        },
        Request::Rebuild {
            session,
            generation,
            request_id,
            guid,
        } => Event::RebuildAck {
            session,
            generation,
            request_id,
            guid,
            host_pid: None,
            accepted: false,
            message: Some(message),
        },
        Request::Index { id, .. } | Request::IndexVolumes { id, .. } => Event::Failure {
            lane: 4,
            id,
            message,
        },
        Request::List { id, .. }
        | Request::Tree { id, .. }
        | Request::Preview { id, .. }
        | Request::Watch { id, .. } => Event::Failure {
            lane: 4,
            id,
            message,
        },
        _ => Event::Notice { message },
    }
}

struct SlotInner {
    pending: Mutex<Pending>,
    stop: AtomicBool,
    wake: Condvar,
    events: Sender<Event>,
}

pub struct Slot {
    inner: Arc<SlotInner>,
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        self.inner.wake.notify_one();
    }
}
impl Slot {
    fn new(lane: usize, events: Sender<Event>, search: bool) -> Self {
        let events = if search {
            event_forwarder(events)
        } else {
            events
        };
        let inner = Arc::new(SlotInner {
            pending: Mutex::new(Pending::default()),
            stop: AtomicBool::new(false),
            wake: Condvar::new(),
            events: events.clone(),
        });
        let worker = inner.clone();
        std::thread::spawn(move || manage(worker, lane, events, search));
        Self { inner }
    }
    pub fn replace(&self, request: Request) {
        let discarded = {
            let mut pending = self.inner.pending.lock().unwrap();
            let mut discarded = pending.controls.drain(..).collect::<Vec<_>>();
            discarded.extend(pending.query.take());
            discarded.extend(pending.replace.replace(request));
            self.inner.wake.notify_one();
            discarded
        };
        for request in discarded {
            let _ = self.inner.events.send(request_failure(
                request,
                "요청이 새 검색 세션으로 교체되었습니다.".into(),
            ));
        }
    }
    pub fn send(&self, request: Request) -> Result<(), String> {
        let result = self.inner.pending.lock().unwrap().enqueue(request);
        self.inner.wake.notify_one();
        match result {
            Ok(replaced) => {
                if let Some(request) = replaced {
                    let _ = self.inner.events.send(request_failure(
                        request,
                        "새 검색어로 대체되었습니다.".into(),
                    ));
                }
                Ok(())
            }
            Err(request) => {
                let message =
                    "검색 제어 대기열이 가득 찼습니다. 잠시 후 다시 시도하세요.".to_string();
                let _ = self
                    .inner
                    .events
                    .send(request_failure(request, message.clone()));
                Err(message)
            }
        }
    }
    pub fn cancel(&self) {
        self.replace(Request::Query {
            id: 0,
            text: String::new(),
        });
    }
}
pub struct Bridge {
    pub panes: Vec<Slot>,
    pub search: Slot,
    pub tree: Slot,
    pub watches: Vec<Slot>,
    pub preview: Slot,
    pub file_preview: Slot,
    pub thumbnails: Slot,
    pub folder_search: Slot,
    pub events: Receiver<Event>,
    jobs: Sender<Request>,
    property_events: Sender<Event>,
    property_busy: Arc<AtomicBool>,
}
impl Bridge {
    pub fn new() -> Self {
        let (tx, rx) = bounded(128);
        let panes = (0..4).map(|i| Slot::new(i, tx.clone(), false)).collect();
        let search = Slot::new(4, tx.clone(), true);
        let tree = Slot::new(5, tx.clone(), false);
        let preview = Slot::new(10, tx.clone(), false);
        let file_preview = Slot::new(11, tx.clone(), false);
        let thumbnails = Slot::new(13, tx.clone(), false);
        let folder_search = Slot::new(12, tx.clone(), true);
        let watches = (0..4)
            .map(|i| Slot::new(6 + i, tx.clone(), false))
            .collect();
        let (jobs, requests) = bounded::<Request>(8);
        let property_events = tx.clone();
        std::thread::spawn(move || {
            while let Ok(req) = requests.recv() {
                let original = req.clone();
                if let Err(message) = run_one(req, &tx) {
                    let e = match original {
                        Request::Job { id, .. } => Event::Job {
                            id,
                            result: JobResult {
                                ok: false,
                                cancelled: false,
                                message,
                                destinations: vec![],
                                undo: None,
                                history_note: None,
                                outcomes: vec![],
                            },
                        },
                        Request::ClipboardGet { id } => Event::Clipboard {
                            id,
                            result: Err(message),
                            sequence: 0,
                        },
                        Request::ClipboardSet { id, cut, .. } => Event::ClipboardWritten {
                            id,
                            cut,
                            result: Err(message),
                            sequence: 0,
                        },
                        _ => Event::Notice { message },
                    };
                    let _ = tx.send(e);
                }
            }
        });
        Self {
            panes,
            search,
            tree,
            watches,
            preview,
            file_preview,
            thumbnails,
            folder_search,
            events: rx,
            jobs,
            property_events,
            property_busy: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn job(&self, req: Request) -> Result<(), String> {
        self.jobs
            .try_send(req)
            .map_err(|_| "작업 대기열이 가득 찼습니다. 잠시 후 다시 시도하세요.".into())
    }
    pub fn properties(&self, path: NativePath) -> Result<(), String> {
        self.shell(Request::Properties { path })
    }
    pub fn utility(&self, action: crate::utility::Utility) -> Result<(), String> {
        self.shell(Request::Utility { action })
    }
    #[cfg(windows)]
    pub fn drag(
        &self,
        id: u64,
        paths: Vec<NativePath>,
        owner: u64,
        capture: Arc<crate::drag::GestureEvents>,
    ) -> Result<(), String> {
        let action = crate::utility::Utility::Drag {
            id,
            paths,
            owner,
            ticket: None,
        };
        self.shell_prepared(Request::Utility { action }, move |child, request| {
            let Request::Utility {
                action: crate::utility::Utility::Drag { ticket, .. },
            } = request
            else {
                return Err("드래그 작업 요청이 올바르지 않습니다.".into());
            };
            *ticket = Some(capture.duplicate_for(child)?);
            Ok(())
        })
    }

    fn shell(&self, request: Request) -> Result<(), String> {
        self.shell_prepared(request, |_, _| Ok(()))
    }

    fn shell_prepared(
        &self,
        request: Request,
        prepare: impl FnOnce(&Child, &mut Request) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        if self.property_busy.swap(true, Ordering::AcqRel) {
            return Err("열려 있는 외부 작업 창을 먼저 닫아 주세요.".into());
        }
        let events = self.property_events.clone();
        let busy = self.property_busy.clone();
        std::thread::spawn(move || {
            let tracked = match &request {
                Request::Utility {
                    action:
                        action @ (crate::utility::Utility::FolderMenu { .. }
                        | crate::utility::Utility::Drag { .. }),
                } => Some(action.clone()),
                _ => None,
            };
            if let Err(message) = run_one_prepared(request, &events, prepare) {
                let event = if let Some(action) = tracked {
                    Event::Utility {
                        action,
                        result: Err(message),
                    }
                } else {
                    Event::Notice { message }
                };
                let _ = events.send(event);
            }
            busy.store(false, Ordering::Release);
        });
        Ok(())
    }
}
impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}
fn child(search: bool) -> Result<Child, String> {
    let mut c = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    c.arg(if search {
        "--search-worker"
    } else {
        "--worker"
    })
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c.spawn().map_err(|e| e.to_string())
}
fn send_request(child: &mut Child, request: &Request) -> Result<(), String> {
    let out = child.stdin.as_mut().ok_or("작업 입력을 열지 못했습니다.")?;
    serde_json::to_writer(&mut *out, request).map_err(|e| e.to_string())?;
    out.write_all(b"\n")
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())
}
fn run_one(request: Request, events: &Sender<Event>) -> Result<(), String> {
    run_one_prepared(request, events, |_, _| Ok(()))
}
fn run_one_prepared(
    mut request: Request,
    events: &Sender<Event>,
    prepare: impl FnOnce(&Child, &mut Request) -> Result<(), String>,
) -> Result<(), String> {
    let mut c = child(false)?;
    if let Err(error) = prepare(&c, &mut request) {
        drop(c.stdin.take());
        let _ = c.wait();
        return Err(error);
    }
    #[cfg(windows)]
    if let Request::Utility {
        action: crate::utility::Utility::FolderMenu { owner, .. },
    } = &request
    {
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{AllowSetForegroundWindow, GetForegroundWindow},
        };
        let owner = HWND(*owner as usize as *mut std::ffi::c_void);
        if unsafe { GetForegroundWindow() } == owner {
            let _ = unsafe { AllowSetForegroundWindow(c.id()) };
        }
    }
    if let Err(error) = send_request(&mut c, &request) {
        drop(c.stdin.take());
        let _ = c.wait();
        return Err(error);
    }
    let tracked = matches!(
        &request,
        Request::Utility {
            action: crate::utility::Utility::FolderMenu { .. }
                | crate::utility::Utility::Drag { .. }
        }
    );
    let mut completed = false;
    let shell = matches!(
        request,
        Request::Utility { .. } | Request::Properties { .. }
    );
    if !shell {
        drop(c.stdin.take());
    }
    if let Some(out) = c.stdout.take() {
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if let Ok(e) = serde_json::from_str::<Event>(&line) {
                let terminal = shell && matches!(e, Event::Utility { .. } | Event::Notice { .. });
                let menu_result = matches!(e, Event::Utility { .. });
                if events.send(e).is_err() {
                    break;
                }
                if terminal {
                    completed = !tracked || menu_result;
                    drop(c.stdin.take());
                    break;
                }
            }
        }
    }
    drop(c.stdin.take());
    let status = c.wait().map_err(|e| e.to_string())?;
    if tracked && !completed {
        return Err(format!(
            "Windows 작업 프로세스가 결과 없이 종료됐습니다 ({status})."
        ));
    }
    if !status.success() {
        let message = format!("작업 프로세스가 종료됐습니다 ({status}). 결과를 확인하세요.");
        match request {
            Request::Job { id, .. } => {
                let _ = events.send(Event::Job {
                    id,
                    result: JobResult {
                        ok: false,
                        cancelled: false,
                        message,
                        destinations: vec![],
                        undo: None,
                        history_note: None,
                        outcomes: vec![],
                    },
                });
            }
            Request::ClipboardSet { id, cut, .. } => {
                let _ = events.send(Event::ClipboardWritten {
                    id,
                    cut,
                    result: Err(message),
                    sequence: 0,
                });
            }
            Request::ClipboardGet { id } => {
                let _ = events.send(Event::Clipboard {
                    id,
                    result: Err(message),
                    sequence: 0,
                });
            }
            _ => {
                let _ = events.send(Event::Notice { message });
            }
        }
    }
    Ok(())
}
fn stop_managed_child(child: &mut Child, search: bool) {
    if !search {
        let _ = child.kill();
        let _ = child.wait();
        return;
    }

    drop(child.stdin.take());
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn event_forwarder(events: Sender<Event>) -> Sender<Event> {
    let (sender, receiver) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            if events.send(event).is_err() {
                break;
            }
        }
    });
    sender
}

fn worker_request_id(request: &Request) -> u64 {
    match request {
        Request::List { id, .. }
        | Request::Index { id, .. }
        | Request::IndexVolumes { id, .. }
        | Request::Tree { id, .. }
        | Request::Preview { id, .. }
        | Request::Watch { id, .. }
        | Request::ReadPreview { id, .. }
        | Request::Thumbnails { id, .. }
        | Request::Query { id, .. } => *id,
        Request::Configure { session, .. }
        | Request::Rebuild { session, .. }
        | Request::VolumeQuery { session, .. } => *session,
        _ => 0,
    }
}

fn stop_search_child(child: &mut Child) -> Result<Option<std::process::ExitStatus>, String> {
    drop(child.stdin.take());
    child
        .try_wait()
        .map_err(|error| format!("검색 worker Stop 상태를 확인하지 못했습니다: {error}"))
}

struct ManagedChild {
    child: Child,
    events: Receiver<Event>,
    done: Arc<AtomicBool>,
    id: u64,
    exit_reported: bool,
}

fn reap_search_child(
    mut child: Child,
    events: Receiver<Event>,
    done: Arc<AtomicBool>,
    id: u64,
    lane: usize,
    output: Sender<Event>,
) {
    std::thread::spawn(move || {
        let mut wait_error_reported = false;
        loop {
            for event in events.try_iter() {
                let _ = output.send(event);
            }
            match child.try_wait() {
                Ok(Some(status)) if done.load(Ordering::Acquire) && events.is_empty() => {
                    if !status.success() {
                        let _ = output.send(Event::Failure {
                            lane,
                            id,
                            message: format!("교체된 검색 worker가 종료됐습니다 ({status})."),
                        });
                    }
                    return;
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(error) if !wait_error_reported => {
                    wait_error_reported = true;
                    let _ = output.send(Event::Failure {
                        lane,
                        id,
                        message: format!("교체된 검색 worker 종료 대기가 실패했습니다: {error}"),
                    });
                }
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}

fn manage(inner: Arc<SlotInner>, lane: usize, events: Sender<Event>, search: bool) {
    let mut running: Option<ManagedChild> = None;
    let mut active_id = 0;
    while !inner.stop.load(Ordering::Relaxed) {
        let replacement = inner.pending.lock().unwrap().replace.take();
        if let Some(request) = replacement {
            active_id = worker_request_id(&request);
            if let Some(mut old) = running.take() {
                if search {
                    match stop_search_child(&mut old.child) {
                        Ok(None) => {
                            let _ = events.send(Event::Failure {
                                lane,
                                id: old.id,
                                message: "검색 worker Stop이 아직 끝나지 않아 소유 프로세스를 계속 감시합니다.".into(),
                            });
                            reap_search_child(
                                old.child,
                                old.events,
                                old.done,
                                old.id,
                                lane,
                                events.clone(),
                            );
                        }
                        Ok(Some(_)) => reap_search_child(
                            old.child,
                            old.events,
                            old.done,
                            old.id,
                            lane,
                            events.clone(),
                        ),
                        Err(message) => {
                            let _ = events.send(Event::Failure {
                                lane,
                                id: old.id,
                                message,
                            });
                            reap_search_child(
                                old.child,
                                old.events,
                                old.done,
                                old.id,
                                lane,
                                events.clone(),
                            );
                        }
                    }
                } else {
                    stop_managed_child(&mut old.child, false);
                    let _ = events.send(Event::Failure {
                        lane,
                        id: old.id,
                        message: "이전 작업이 새 요청으로 교체되었습니다.".into(),
                    });
                }
            }
            if !matches!(&request, Request::Query { id: 0, .. }) {
                match child(search) {
                    Ok(mut next) => {
                        let output = next.stdout.take();
                        let (tx, rx): (Sender<Event>, Receiver<Event>) = if search {
                            crossbeam_channel::unbounded()
                        } else {
                            bounded(8)
                        };
                        let done = Arc::new(AtomicBool::new(false));
                        if let Some(output) = output {
                            let finished = done.clone();
                            std::thread::spawn(move || {
                                for line in BufReader::new(output).lines() {
                                    let Ok(line) = line else { break };
                                    if line.len() > 2_000_000 {
                                        break;
                                    }
                                    if let Ok(event) = serde_json::from_str::<Event>(&line) {
                                        if tx.send(event).is_err() {
                                            break;
                                        }
                                    }
                                }
                                finished.store(true, Ordering::Release);
                            });
                        } else {
                            done.store(true, Ordering::Release);
                            let _ = events.send(Event::Failure {
                                lane,
                                id: active_id,
                                message: "작업 출력을 열지 못했습니다.".into(),
                            });
                            if search {
                                drop(next.stdin.take());
                                reap_search_child(next, rx, done, active_id, lane, events.clone());
                            } else {
                                let _ = next.kill();
                                let _ = next.wait();
                            }
                            continue;
                        }
                        match send_request(&mut next, &request) {
                            Ok(()) => {
                                if !search && !matches!(&request, Request::Watch { .. }) {
                                    drop(next.stdin.take());
                                }
                                running = Some(ManagedChild {
                                    child: next,
                                    events: rx,
                                    done,
                                    id: active_id,
                                    exit_reported: false,
                                });
                            }
                            Err(message) if search => {
                                drop(next.stdin.take());
                                let _ = events.send(Event::Failure {
                                    lane,
                                    id: active_id,
                                    message,
                                });
                                reap_search_child(next, rx, done, active_id, lane, events.clone());
                            }
                            Err(message) => {
                                stop_managed_child(&mut next, false);
                                let _ = events.send(Event::Failure {
                                    lane,
                                    id: active_id,
                                    message,
                                });
                            }
                        }
                    }
                    Err(message) => {
                        let _ = events.send(Event::Failure {
                            lane,
                            id: active_id,
                            message,
                        });
                    }
                }
            }
        }
        if let Some(current) = running.as_mut() {
            let (controls, query) = {
                let mut pending = inner.pending.lock().unwrap();
                if pending.replace.is_some() {
                    (Vec::new(), None)
                } else {
                    (
                        pending.controls.drain(..).collect::<Vec<_>>(),
                        pending.query.take(),
                    )
                }
            };
            for request in controls {
                if let Err(message) = send_request(&mut current.child, &request) {
                    let _ = events.send(request_failure(request, message));
                }
            }
            if let Some(request) = query {
                if let Err(message) = send_request(&mut current.child, &request) {
                    let _ = events.send(request_failure(request, message));
                }
            }
            for _ in 0..16 {
                let Ok(event) = current.events.try_recv() else {
                    break;
                };
                let _ = events.send(event);
            }
            let mut clear_running = false;
            match current.child.try_wait() {
                Ok(Some(status)) => {
                    if !current.exit_reported && (!status.success() || (6..10).contains(&lane)) {
                        let _ = events.send(Event::Failure {
                            lane,
                            id: current.id,
                            message: format!("읽기 프로세스가 종료됐습니다 ({status})."),
                        });
                        current.exit_reported = true;
                    }
                    for event in current.events.try_iter() {
                        let _ = events.send(event);
                    }
                    clear_running =
                        current.done.load(Ordering::Acquire) && current.events.is_empty();
                }
                Err(error) if !current.exit_reported => {
                    let _ = events.send(Event::Failure {
                        lane,
                        id: current.id,
                        message: format!("읽기 프로세스 종료 상태를 확인하지 못했습니다: {error}"),
                    });
                    current.exit_reported = true;
                }
                Ok(None) | Err(_) => {}
            }
            if clear_running {
                running = None;
            }
        }
        let pending = inner.pending.lock().unwrap();
        if pending.replace.is_none() && pending.controls.is_empty() && pending.query.is_none() {
            let _unused = inner.wake.wait_timeout(
                pending,
                Duration::from_millis(if running.is_some() { 20 } else { 1000 }),
            );
        }
    }
    if let Some(mut current) = running {
        if search {
            match stop_search_child(&mut current.child) {
                Ok(None) => {
                    let _ = events.send(Event::Failure {
                        lane,
                        id: current.id,
                        message: "검색 worker Stop을 계속 감시합니다.".into(),
                    });
                    reap_search_child(
                        current.child,
                        current.events,
                        current.done,
                        current.id,
                        lane,
                        events,
                    );
                }
                Ok(Some(_)) => reap_search_child(
                    current.child,
                    current.events,
                    current.done,
                    current.id,
                    lane,
                    events,
                ),
                Err(message) => {
                    let _ = events.send(Event::Failure {
                        lane,
                        id: current.id,
                        message,
                    });
                    reap_search_child(
                        current.child,
                        current.events,
                        current.done,
                        current.id,
                        lane,
                        events,
                    );
                }
            }
        } else {
            stop_managed_child(&mut current.child, false);
        }
    }
}

#[cfg(test)]
mod pending_tests {
    use super::*;

    fn query(id: u64, text: &str) -> Request {
        Request::VolumeQuery {
            session: 7,
            generation: 0,
            query_id: id,
            text: text.into(),
        }
    }

    fn rebuild(id: u64) -> Request {
        Request::Rebuild {
            session: 7,
            generation: 0,
            request_id: id,
            guid: crate::index_policy::VolumeGuid::parse("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
                .unwrap(),
        }
    }

    #[test]
    fn pending_coalesces_only_queries_and_keeps_control_fifo() {
        let mut pending = Pending::default();
        pending
            .enqueue(Request::Configure {
                session: 7,
                generation: 1,
                settings: Box::new(crate::index_policy::NtfsIndexingSettings::default()),
            })
            .unwrap();
        assert!(pending.enqueue(query(1, "old")).unwrap().is_none());
        assert!(pending.enqueue(rebuild(1)).unwrap().is_none());
        assert!(matches!(
            pending.enqueue(query(2, "new")).unwrap(),
            Some(Request::VolumeQuery { query_id: 1, .. })
        ));
        assert!(matches!(
            pending.controls.pop_front(),
            Some(Request::Configure { generation: 1, .. })
        ));
        assert!(matches!(
            pending.controls.pop_front(),
            Some(Request::Rebuild { request_id: 1, .. })
        ));
        assert!(matches!(
            pending.query,
            Some(Request::VolumeQuery { query_id: 2, .. })
        ));
    }

    #[test]
    fn pending_control_overflow_rejects_without_losing_fifo() {
        let mut pending = Pending::default();
        for id in 1..=MAX_PENDING_CONTROL_REQUESTS as u64 {
            assert!(pending.enqueue(rebuild(id)).is_ok());
        }
        assert!(pending.enqueue(rebuild(99)).is_err());
        assert_eq!(pending.controls.len(), MAX_PENDING_CONTROL_REQUESTS);
        assert!(
            pending
                .controls
                .iter()
                .enumerate()
                .all(|(index, request)| matches!(
                    request,
                    Request::Rebuild { request_id, .. } if *request_id == index as u64 + 1
                ))
        );
    }

    #[test]
    fn search_event_forwarder_does_not_wait_for_a_full_ui_channel() {
        let (ui_tx, ui_rx) = bounded(1);
        let event_tx = event_forwarder(ui_tx);
        let (sent_tx, sent_rx) = bounded(1);
        std::thread::spawn(move || {
            for id in 0..4096 {
                event_tx
                    .send(Event::Notice {
                        message: id.to_string(),
                    })
                    .unwrap();
            }
            let _ = sent_tx.send(());
        });
        sent_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("supervisory event queue remains writable while UI is blocked");
        assert!(matches!(
            ui_rx.recv_timeout(Duration::from_secs(1)),
            Ok(Event::Notice { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn replacement_reaper_reports_the_old_search_session_native_exit() {
        use std::process::Stdio;
        use std::time::Duration;

        let system_root = std::env::var_os("SystemRoot").expect("Windows SystemRoot");
        let powershell = std::path::PathBuf::from(system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        assert!(powershell.is_file());
        let mut child = Command::new(powershell)
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 2; exit 23"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(stop_search_child(&mut child).unwrap().is_none());
        let (child_events, event_rx) = crossbeam_channel::unbounded();
        drop(child_events);
        let done = Arc::new(AtomicBool::new(true));
        let (output_tx, output_rx) = crossbeam_channel::unbounded();
        reap_search_child(child, event_rx, done, 41, 4, output_tx);
        match output_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Failure { lane, id, message } => {
                assert_eq!((lane, id), (4, 41));
                assert!(message.contains("23"));
            }
            event => panic!("unexpected old-session reaper event: {event:?}"),
        }
    }
}
