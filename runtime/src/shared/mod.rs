use crate::sim::types::{SimSharedSlot, SimSharedSlotRaw};
use memmap2::MmapMut;
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

const WRITER_NAME_LEN: usize = 64;
const MAX_SNAPSHOT_SPINS: usize = 32;

#[repr(C)]
#[derive(Clone, Copy)]
struct SharedHeader {
    generation: u64,
    slot_count: u32,
    initialized: u32,
    writer_session: [u8; WRITER_NAME_LEN],
}

pub struct SharedRegion {
    mmap: MmapMut,
    slot_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedSnapshotError {
    Busy { attempts: usize },
    Uninitialized,
    Invalid(String),
}

impl std::fmt::Display for SharedSnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { attempts } => {
                write!(
                    f,
                    "shared snapshot remained unstable after {attempts} read attempts"
                )
            }
            Self::Uninitialized => write!(f, "shared snapshot has not been published yet"),
            Self::Invalid(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SharedSnapshotError {}

impl SharedRegion {
    pub fn open(
        path: &Path,
        slot_count: usize,
        writer_session: &str,
        initialize: bool,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed to create shared region parent '{}': {e}",
                    parent.display()
                )
            })?;
        }

        let expected_len = Self::byte_len(slot_count);
        if !initialize && !path.exists() {
            return Err(format!(
                "shared region '{}' has not been initialized by its writer yet",
                path.display()
            ));
        }
        let file = OpenOptions::new()
            .create(initialize)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("failed to open shared region '{}': {e}", path.display()))?;
        let current_len = file
            .metadata()
            .map_err(|e| format!("failed to inspect shared region '{}': {e}", path.display()))?
            .len() as usize;
        let mut should_initialize = initialize;
        if current_len == 0 {
            if !initialize {
                return Err(format!(
                    "shared region '{}' has not been initialized by its writer yet",
                    path.display()
                ));
            }
            file.set_len(expected_len as u64).map_err(|e| {
                format!(
                    "failed to size shared region '{}' to {} bytes: {e}",
                    path.display(),
                    expected_len
                )
            })?;
            should_initialize = true;
        } else if current_len != expected_len {
            return Err(format!(
                "shared region '{}' has size {} but expected {}",
                path.display(),
                current_len,
                expected_len
            ));
        }

        let mut mmap = unsafe {
            MmapMut::map_mut(&file)
                .map_err(|e| format!("failed to mmap shared region '{}': {e}", path.display()))?
        };
        if should_initialize {
            let header = SharedHeader {
                generation: 0,
                slot_count: slot_count as u32,
                initialized: 0,
                writer_session: encode_writer(writer_session),
            };
            Self::write_header(&mut mmap, &header);
            let offset = std::mem::size_of::<SharedHeader>();
            mmap[offset..].fill(0);
        } else {
            let header = Self::read_header(&mmap);
            if header.slot_count as usize != slot_count {
                return Err(format!(
                    "shared region '{}' slot count mismatch: region={} expected={}",
                    path.display(),
                    header.slot_count,
                    slot_count
                ));
            }
        }
        Ok(Self { mmap, slot_count })
    }

    pub fn publish(&mut self, slots: &[SimSharedSlot]) -> Result<(), String> {
        if slots.len() != self.slot_count {
            return Err(format!(
                "attempted to publish {} slots into region with capacity {}",
                slots.len(),
                self.slot_count
            ));
        }
        for (expected_slot_id, slot) in slots.iter().enumerate() {
            if slot.slot_id as usize != expected_slot_id {
                return Err(format!(
                    "shared slot id {} appeared at dense index {}; expected slot id {}",
                    slot.slot_id, expected_slot_id, expected_slot_id
                ));
            }
        }
        let generation = self.generation();
        self.set_generation(generation.wrapping_add(1)); // odd = write in progress
        {
            let slot_storage = self.slot_storage_mut();
            for (idx, slot) in slots.iter().enumerate() {
                slot_storage[idx] = slot.to_raw();
            }
        }
        self.set_initialized(true);
        self.set_generation(generation.wrapping_add(2)); // even = stable snapshot
        self.mmap
            .flush_async()
            .map_err(|e| format!("failed flushing shared snapshot: {e}"))?;
        Ok(())
    }

    pub fn read_snapshot(&self) -> Result<Vec<SimSharedSlot>, SharedSnapshotError> {
        for _ in 0..MAX_SNAPSHOT_SPINS {
            let before = self.generation();
            if !before.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            if !self.is_initialized() {
                return Err(SharedSnapshotError::Uninitialized);
            }
            let snapshot = self
                .slot_storage()
                .iter()
                .enumerate()
                .map(|(expected_slot_id, slot)| {
                    let decoded = SimSharedSlot::try_from_raw(*slot).map_err(|err| {
                        SharedSnapshotError::Invalid(format!(
                            "shared snapshot slot {expected_slot_id} is invalid: {err}"
                        ))
                    })?;
                    if decoded.slot_id as usize != expected_slot_id {
                        return Err(SharedSnapshotError::Invalid(format!(
                            "shared snapshot slot {expected_slot_id} reported slot id {}",
                            decoded.slot_id
                        )));
                    }
                    Ok(decoded)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let after = self.generation();
            if before == after && after.is_multiple_of(2) {
                return Ok(snapshot);
            }
            std::hint::spin_loop();
        }
        Err(SharedSnapshotError::Busy {
            attempts: MAX_SNAPSHOT_SPINS,
        })
    }

    fn byte_len(slot_count: usize) -> usize {
        std::mem::size_of::<SharedHeader>() + (slot_count * std::mem::size_of::<SimSharedSlotRaw>())
    }

    fn read_header(mmap: &MmapMut) -> SharedHeader {
        let header_ptr = mmap.as_ptr().cast::<SharedHeader>();
        unsafe { *header_ptr }
    }

    fn write_header(mmap: &mut MmapMut, header: &SharedHeader) {
        let header_ptr = mmap.as_mut_ptr().cast::<SharedHeader>();
        unsafe {
            *header_ptr = *header;
        }
    }

    fn generation(&self) -> u64 {
        let header = self.mmap.as_ptr().cast::<SharedHeader>();
        let generation_ptr = unsafe { std::ptr::addr_of!((*header).generation) as *mut u64 };
        // SAFETY:
        // - `generation_ptr` points to the `generation` field inside the mmap-backed
        //   `SharedHeader`, which is a valid, initialized `u64` for the lifetime of `self`.
        // - `SharedHeader` is `#[repr(C)]`, so the field address is stable.
        // - access to this field is performed atomically via `generation()`/`set_generation()`
        //   once the region is initialized, satisfying `AtomicU64::from_ptr` requirements.
        let generation = unsafe { AtomicU64::from_ptr(generation_ptr) };
        generation.load(Ordering::Acquire)
    }

    fn set_generation(&mut self, value: u64) {
        let header = self.mmap.as_mut_ptr().cast::<SharedHeader>();
        let generation_ptr = unsafe { std::ptr::addr_of_mut!((*header).generation) };
        let generation = unsafe { AtomicU64::from_ptr(generation_ptr) };
        generation.store(value, Ordering::Release);
    }

    fn is_initialized(&self) -> bool {
        let header = Self::read_header(&self.mmap);
        header.initialized != 0
    }

    fn set_initialized(&mut self, initialized: bool) {
        let header = self.mmap.as_mut_ptr().cast::<SharedHeader>();
        unsafe {
            (*header).initialized = u32::from(initialized);
        }
    }

    fn slot_storage(&self) -> &[SimSharedSlotRaw] {
        let offset = std::mem::size_of::<SharedHeader>();
        let ptr = unsafe { self.mmap.as_ptr().add(offset).cast::<SimSharedSlotRaw>() };
        unsafe { std::slice::from_raw_parts(ptr, self.slot_count) }
    }

    fn slot_storage_mut(&mut self) -> &mut [SimSharedSlotRaw] {
        let offset = std::mem::size_of::<SharedHeader>();
        let ptr = unsafe {
            self.mmap
                .as_mut_ptr()
                .add(offset)
                .cast::<SimSharedSlotRaw>()
        };
        unsafe { std::slice::from_raw_parts_mut(ptr, self.slot_count) }
    }
}

fn encode_writer(writer_session: &str) -> [u8; WRITER_NAME_LEN] {
    let mut out = [0_u8; WRITER_NAME_LEN];
    let bytes = writer_session.as_bytes();
    let len = bytes.len().min(WRITER_NAME_LEN.saturating_sub(1));
    out[..len].copy_from_slice(&bytes[..len]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::types::SignalValue;

    #[test]
    fn shared_region_roundtrip_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir should be creatable");
        let path = dir.path().join("region.bin");
        let mut region = SharedRegion::open(&path, 2, "writer", true)
            .expect("shared region should open for writer");
        region
            .publish(&[
                SimSharedSlot {
                    slot_id: 0,
                    value: SignalValue::F32(12.5),
                },
                SimSharedSlot {
                    slot_id: 1,
                    value: SignalValue::Bool(true),
                },
            ])
            .expect("publish should succeed");

        let reader =
            SharedRegion::open(&path, 2, "writer", false).expect("reader should open region");
        let snapshot = reader
            .read_snapshot()
            .expect("snapshot should be consistent");
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.iter().any(|slot| slot.slot_id == 0));
        assert!(snapshot.iter().any(|slot| slot.slot_id == 1));
    }

    #[test]
    fn publish_invalid_slot_id_does_not_poison_generation() {
        let dir = tempfile::tempdir().expect("tempdir should be creatable");
        let path = dir.path().join("region.bin");
        let mut region = SharedRegion::open(&path, 2, "writer", true)
            .expect("shared region should open for writer");
        region
            .publish(&[
                SimSharedSlot {
                    slot_id: 0,
                    value: SignalValue::F32(7.0),
                },
                SimSharedSlot {
                    slot_id: 1,
                    value: SignalValue::Bool(false),
                },
            ])
            .expect("initial publish should succeed");
        let before = region.generation();

        let err = region.publish(&[
            SimSharedSlot {
                slot_id: 0,
                value: SignalValue::F32(7.0),
            },
            SimSharedSlot {
                slot_id: 9,
                value: SignalValue::Bool(true),
            },
        ]);
        assert!(err.is_err(), "publish should fail for invalid slot id");
        assert_eq!(
            region.generation(),
            before,
            "failed publish must not leave generation in a poisoned state"
        );
        assert!(
            region.generation().is_multiple_of(2),
            "generation must remain even after failed publish"
        );
        let snapshot = region
            .read_snapshot()
            .expect("snapshot should remain readable after failed publish");
        assert!(
            snapshot
                .iter()
                .any(|slot| slot.slot_id == 0 && slot.value == SignalValue::F32(7.0)),
            "previous snapshot payload should remain readable after failed publish"
        );
    }

    #[test]
    fn publish_wraps_generation_without_leaving_odd_state() {
        let dir = tempfile::tempdir().expect("tempdir should be creatable");
        let path = dir.path().join("region.bin");
        let mut region = SharedRegion::open(&path, 2, "writer", true)
            .expect("shared region should open for writer");
        region.set_generation(u64::MAX - 1);
        region.set_initialized(true);

        region
            .publish(&[
                SimSharedSlot {
                    slot_id: 0,
                    value: SignalValue::F32(0.0),
                },
                SimSharedSlot {
                    slot_id: 1,
                    value: SignalValue::Bool(true),
                },
            ])
            .expect("publish should succeed near generation rollover");

        let generation = region.generation();
        assert_eq!(generation, 0, "generation should wrap to 0 after publish");
        assert!(
            generation.is_multiple_of(2),
            "generation must remain even after wrapped publish"
        );
        let snapshot = region
            .read_snapshot()
            .expect("snapshot should remain readable after wrapped publish");
        assert!(
            snapshot
                .iter()
                .any(|slot| slot.slot_id == 1 && slot.value == SignalValue::Bool(true)),
            "snapshot payload should remain readable after wrapped publish"
        );
    }

    #[test]
    fn read_snapshot_fails_when_writer_never_finishes() {
        let dir = tempfile::tempdir().expect("tempdir should be creatable");
        let path = dir.path().join("region.bin");
        let mut region = SharedRegion::open(&path, 2, "writer", true)
            .expect("shared region should open for writer");
        region.set_generation(1);

        let err = region
            .read_snapshot()
            .expect_err("reader should refuse unstable snapshot");
        assert_eq!(
            err,
            SharedSnapshotError::Busy {
                attempts: MAX_SNAPSHOT_SPINS
            }
        );
    }

    #[test]
    fn writer_reinitialization_clears_previous_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir should be creatable");
        let path = dir.path().join("region.bin");
        let mut writer =
            SharedRegion::open(&path, 2, "writer", true).expect("writer should open shared region");
        writer
            .publish(&[
                SimSharedSlot {
                    slot_id: 0,
                    value: SignalValue::F32(9.5),
                },
                SimSharedSlot {
                    slot_id: 1,
                    value: SignalValue::Bool(false),
                },
            ])
            .expect("publish should succeed");

        let reopened = SharedRegion::open(&path, 2, "writer", true)
            .expect("reinitialized writer should reopen shared region");
        let err = reopened
            .read_snapshot()
            .expect_err("reinitialized writer should clear any previously published snapshot");
        assert_eq!(
            err,
            SharedSnapshotError::Uninitialized,
            "reinitializing a writer should leave the region unpublished until the writer publishes again"
        );
    }
}
