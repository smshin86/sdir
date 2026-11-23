use super::*;
use crate::index_cache::{CacheIdentity, CacheMetadata, CachedVolume};
use crate::index_policy::{NtfsIndexingSettings, VolumeGuid};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const SAVE_INTERVAL: Duration = Duration::from_secs(60);
const CACHE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const SAVE_WORKSPACE: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
struct VolumeKey {
    guid: VolumeGuid,
    serial: Option<u32>,
    revision: u64,
    generation: u64,
}
impl VolumeKey {
    fn new(volume: &Volume, generation: u64) -> Self {
        Self {
            guid: volume.guid.clone(),
            serial: volume.serial,
            revision: volume.revision,
            generation,
        }
    }

    fn matches(&self, volume: &Volume, generation: u64) -> bool {
        self.generation == generation
            && self.guid == volume.guid
            && self.serial == volume.serial
            && self.revision == volume.revision
    }
}

enum Work {
    Load {
        identity: CacheIdentity,
        limit: u64,
    },
    Save {
        index: Arc<VolumeIndex>,
        metadata: CacheMetadata,
    },
    Purge(VolumeGuid),
}
enum Finished {
    Loaded(Result<Option<CachedVolume>, String>),
    Saved(Result<(), String>),
    Purged,
}
enum HeldMemory {
    Load,
    Save { index_address: usize, bytes: u64 },
    None,
}
struct Pending {
    volume: VolumeKey,
    held: HeldMemory,
}

pub(super) struct ResumeStore {
    requests: crossbeam_channel::Sender<Work>,
    results: crossbeam_channel::Receiver<Finished>,
    pending: Option<Pending>,
    attempted: BTreeSet<VolumeGuid>,
    offline: BTreeSet<VolumeGuid>,
}

impl ResumeStore {
    pub(super) fn start(owner_sid: String) -> Self {
        let (requests, work) = crossbeam_channel::bounded(1);
        let (done, results) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let mut store = crate::live_search::service::prepare_resume_store(&owner_sid);
            while let Ok(job) = work.recv() {
                let result = match job {
                    Work::Load { identity, limit } => Finished::Loaded(match &mut store {
                        Ok(store) => store
                            .load_verified_service(&identity, limit)
                            .map_err(|e| e.to_string()),
                        Err(_) => Ok(None),
                    }),
                    Work::Save { index, metadata } => {
                        let result = match &mut store {
                            Ok(store) => store
                                .save_verified_service(&index, &metadata)
                                .map(|_| ())
                                .map_err(|e| e.to_string()),
                            Err(_) => Err("보호된 색인 저장소를 열 수 없습니다.".into()),
                        };
                        // Completion always arrives after this worker releases its shared index.
                        drop(index);
                        Finished::Saved(result)
                    }
                    Work::Purge(guid) => {
                        if let Ok(store) = &mut store {
                            let _ = store.purge_guid(&guid);
                        }
                        Finished::Purged
                    }
                };
                if done.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            requests,
            results,
            pending: None,
            attempted: BTreeSet::new(),
            offline: BTreeSet::new(),
        }
    }

    pub(super) fn loading(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|work| matches!(work.held, HeldMemory::Load))
    }

    pub(super) fn detached_bytes(&self, volumes: &[Volume]) -> u64 {
        let Some(Pending {
            held:
                HeldMemory::Save {
                    index_address,
                    bytes,
                },
            ..
        }) = self.pending.as_ref()
        else {
            return 0;
        };
        let still_present = volumes.iter().any(|volume| {
            volume
                .index
                .as_ref()
                .is_some_and(|index| Arc::as_ptr(index) as usize == *index_address)
        });
        SAVE_WORKSPACE + if still_present { 0 } else { *bytes }
    }

    fn submit(&mut self, pending: Pending, work: Work) -> bool {
        if self.requests.try_send(work).is_err() {
            return false;
        }
        self.pending = Some(pending);
        true
    }

    pub(super) fn skip_restore(&mut self, guid: &VolumeGuid) {
        self.attempted.insert(guid.clone());
    }

    pub(super) fn observe_offline(&mut self, guid: &VolumeGuid) {
        self.offline.insert(guid.clone());
    }

    pub(super) fn observe_online(&mut self, guid: &VolumeGuid) {
        self.offline.remove(guid);
    }

    pub(super) fn poll(&mut self, volumes: &mut [Volume], generation: u64) -> bool {
        let Ok(completed) = self.results.try_recv() else {
            return false;
        };
        let Some(pending) = self.pending.take() else {
            return false;
        };
        let Some(volume) = volumes
            .iter_mut()
            .find(|v| pending.volume.matches(v, generation))
        else {
            return false;
        };
        match completed {
            Finished::Loaded(Ok(Some(saved))) if load_eligible(volume) => {
                match install(volume, saved) {
                    Ok(()) => {}
                    Err(reason) => volume.message = Some(reason.message().into()),
                }
                true
            }
            Finished::Loaded(Ok(None)) if load_eligible(volume) => {
                volume.message = Some(CacheReason::Missing.message().into());
                true
            }
            Finished::Loaded(Err(_)) if load_eligible(volume) => {
                volume.message = Some(CacheReason::Corrupt.message().into());
                true
            }
            Finished::Saved(Ok(())) if volume.state == Phase::Live => {
                volume.saved_revision = Some(volume.revision);
                volume.message = Some(if volume.restored {
                    "저장된 색인 복원 · 변경분 반영 및 저장 완료".into()
                } else {
                    "드라이브 색인 저장 완료".into()
                });
                true
            }
            Finished::Saved(Err(_)) if volume.state == Phase::Live => {
                volume.message = Some("이번 색인을 디스크에 저장하지 못했습니다.".into());
                true
            }
            _ => false,
        }
    }

    pub(super) fn schedule(
        &mut self,
        volumes: &mut [Volume],
        settings: &NtfsIndexingSettings,
        generation: u64,
        available: u64,
    ) -> bool {
        if self.pending.is_some() || !settings.persistent() {
            return false;
        }
        if settings.offline_cache_action()
            == crate::index_policy::OfflineCacheAction::DeleteOwnedCache
        {
            if let Some(guid) = self.offline.pop_first() {
                let pending = Pending {
                    volume: VolumeKey {
                        guid: guid.clone(),
                        serial: None,
                        revision: 0,
                        generation,
                    },
                    held: HeldMemory::None,
                };
                self.submit(pending, Work::Purge(guid));
                return false;
            }
        }
        for volume in volumes.iter_mut().filter(|v| load_eligible(v)) {
            if self.attempted.contains(&volume.guid) {
                continue;
            }
            let Some(serial) = volume.serial else {
                continue;
            };
            let Ok(identity) = cache_identity(settings, &volume.guid, serial) else {
                continue;
            };
            self.skip_restore(&volume.guid);
            let limit = available;
            if limit <= SAVE_WORKSPACE {
                volume.message = Some(CacheReason::MemoryLimit.message().into());
                return true;
            }
            let pending = Pending {
                volume: VolumeKey::new(volume, generation),
                held: HeldMemory::Load,
            };
            if self.submit(pending, Work::Load { identity, limit }) {
                volume.message = Some("저장된 드라이브 색인 불러오는 중".into());
                return true;
            }
            return false;
        }
        if available < SAVE_WORKSPACE {
            return false;
        }
        let now = Instant::now();
        for volume in volumes {
            if volume.classification != super::super::InventoryClass::Fixed
                || volume.state != Phase::Live
                || volume.saved_revision == Some(volume.revision)
                || volume.cache_save_after > now
            {
                continue;
            }
            let (Some(serial), Some(journal), Some(index)) =
                (volume.serial, volume.journal, volume.index.as_ref())
            else {
                continue;
            };
            if !cursor_is_valid(journal.journal_id, volume.cursor, journal) {
                continue;
            }
            let Ok(identity) = cache_identity(settings, &volume.guid, serial) else {
                continue;
            };
            let Ok(metadata) = CacheMetadata::at_now(identity, journal.journal_id, volume.cursor)
            else {
                continue;
            };
            let pending = Pending {
                volume: VolumeKey::new(volume, generation),
                held: HeldMemory::Save {
                    index_address: Arc::as_ptr(index) as usize,
                    bytes: volume.allocated_bytes(),
                },
            };
            volume.cache_save_after = now + SAVE_INTERVAL;
            self.submit(
                pending,
                Work::Save {
                    index: Arc::clone(index),
                    metadata,
                },
            );
            break;
        }
        false
    }
}

fn load_eligible(volume: &Volume) -> bool {
    volume.classification == super::super::InventoryClass::Fixed
        && volume.state == Phase::Indexing
        && !volume.layout_started
        && !volume.snapshot_only
        && volume.handle.is_some()
        && volume.journal.is_some()
}

fn install(volume: &mut Volume, saved: CachedVolume) -> Result<(), CacheReason> {
    let cursor = saved.metadata.committed_next_usn();
    let journal_id = saved.metadata.journal_id();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CacheReason::FutureTimestamp)?
        .as_millis();
    let created = u128::from(saved.metadata.created_at_ms());
    if created > now {
        return Err(CacheReason::FutureTimestamp);
    }
    if now - created > CACHE_MAX_AGE.as_millis() {
        return Err(CacheReason::TooOld);
    }
    let handle = volume.handle.as_ref().ok_or(CacheReason::Incompatible)?;
    confirm_volume_identity(
        volume.stop.as_ref(),
        handle,
        &volume.guid,
        volume.serial,
        volume.mount.as_ref(),
    )
    .map_err(|_| CacheReason::Incompatible)?;
    let journal = match host_winfs(volume.stop.as_ref(), || {
        goz_winfs::query_usn_journal(handle)
    }) {
        Ok(JournalQuery::Active(journal)) => journal,
        _ => return Err(CacheReason::JournalChanged),
    };
    if journal_id != journal.journal_id {
        return Err(CacheReason::JournalChanged);
    }
    if cursor > journal.next_usn {
        return Err(CacheReason::FutureCursor);
    }
    if !cursor_is_valid(journal_id, cursor, journal) {
        return Err(CacheReason::CursorOutsideWindow);
    }
    volume.index = Some(Arc::new(saved.index));
    volume.cursor = cursor;
    volume.high_water = Some(journal.next_usn);
    volume.journal = Some(journal);
    volume.state = Phase::CatchingUp;
    volume.layout_started = true;
    volume.restored = true;
    volume.revision += 1;
    volume.message = Some("저장된 색인 복원 · NTFS 변경분 반영 중".into());
    Ok(())
}
