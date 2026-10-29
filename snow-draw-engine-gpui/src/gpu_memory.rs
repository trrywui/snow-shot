use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use anyhow::{Result, ensure};
use gpui::GpuService;

/// One accounting domain for the service, including resources awaiting GPU retirement.
/// The registry retains configuration while the service lives, without retaining
/// the service itself or introducing a service/deferred-payload ownership cycle.
pub(crate) struct GpuMemory {
    service: Option<Weak<dyn GpuService>>,
    epoch: u64,
    limit: usize,
    used: AtomicUsize,
    peak: AtomicUsize,
    allocation_count: AtomicUsize,
    pending: AtomicUsize,
    retiring: AtomicUsize,
    scheduled: AtomicBool,
}

pub(crate) struct Allocation {
    owner: Arc<GpuMemory>,
    bytes: usize,
    gpu_used: AtomicBool,
}

/// The GPU service alone owns this payload and drops it after its completion
/// marker (or device invalidation). Subsequent drops cannot join this older fence.
struct RetirementBatch {
    owner: Arc<GpuMemory>,
    bytes: usize,
}
impl Drop for RetirementBatch {
    fn drop(&mut self) {
        self.owner.used.fetch_sub(self.bytes, Ordering::AcqRel);
        self.owner.retiring.fetch_sub(self.bytes, Ordering::AcqRel);
        self.owner.scheduled.store(false, Ordering::Release);
        self.owner.schedule_retirement();
    }
}
impl Allocation {
    pub fn mark_gpu_use(&self) {
        self.gpu_used.store(true, Ordering::Relaxed);
    }
    /// A uniquely owned lease may become idle only after its complete native
    /// operation was observed. Shared/published texture leases cannot use this.
    pub fn mark_gpu_complete(&mut self) {
        self.gpu_used.store(false, Ordering::Relaxed);
    }
}

impl GpuMemory {
    pub fn shared(
        service: &Arc<dyn GpuService>,
        requested_limit: Option<usize>,
        default_limit: usize,
    ) -> Result<Arc<Self>> {
        static SERVICES: OnceLock<Mutex<HashMap<u64, Arc<GpuMemory>>>> = OnceLock::new();
        let mut services = SERVICES
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| anyhow::anyhow!("GPU memory registry is unavailable"))?;
        services.retain(|_, memory| {
            memory
                .service
                .as_ref()
                .is_some_and(|service| service.strong_count() != 0)
        });
        if let Some(memory) = services.get(&service.service_id()) {
            ensure!(
                requested_limit.is_none_or(|limit| memory.limit == limit),
                "GPU canvases sharing a device must use the same memory budget"
            );
            if memory.epoch == service.epoch() {
                return Ok(memory.clone());
            }
            // A replacement generation has independent allocations. Retain the
            // service policy, but never charge new resources to an old epoch's
            // immediately retiring accounting domain.
            let memory = Self::create(memory.limit, Some(Arc::downgrade(service)));
            services.insert(service.service_id(), memory.clone());
            return Ok(memory);
        }
        let memory = Self::create(
            requested_limit.unwrap_or(default_limit),
            Some(Arc::downgrade(service)),
        );
        services.insert(service.service_id(), memory.clone());
        Ok(memory)
    }

    fn create(limit: usize, service: Option<Weak<dyn GpuService>>) -> Arc<Self> {
        let epoch = service
            .as_ref()
            .and_then(Weak::upgrade)
            .map_or(0, |service| service.epoch());
        Arc::new(Self {
            service,
            epoch,
            limit,
            used: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            allocation_count: AtomicUsize::new(0),
            pending: AtomicUsize::new(0),
            retiring: AtomicUsize::new(0),
            scheduled: AtomicBool::new(false),
        })
    }

    pub fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Arc<Allocation>> {
        let mut used = self.used.load(Ordering::Relaxed);
        loop {
            let next = used
                .checked_add(bytes)
                .ok_or_else(|| anyhow::anyhow!("GPU allocation size overflow"))?;
            ensure!(
                next <= self.limit,
                "Canvas GPU budget exceeded ({next} > {} bytes)",
                self.limit
            );
            match self
                .used
                .compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Relaxed)
            {
                Ok(_) => {
                    self.peak.fetch_max(next, Ordering::Relaxed);
                    self.allocation_count.fetch_add(1, Ordering::Relaxed);
                    return Ok(Arc::new(Allocation {
                        owner: self.clone(),
                        bytes,
                        gpu_used: AtomicBool::new(false),
                    }));
                }
                Err(current) => used = current,
            }
        }
    }

    fn retire(self: &Arc<Self>, bytes: usize) {
        self.retiring.fetch_add(bytes, Ordering::AcqRel);
        self.pending.fetch_add(bytes, Ordering::AcqRel);
        self.schedule_retirement();
    }

    fn schedule_retirement(self: &Arc<Self>) {
        loop {
            if self.pending.load(Ordering::Acquire) == 0
                || self
                    .scheduled
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
            {
                return;
            }
            let bytes = self.pending.swap(0, Ordering::AcqRel);
            if let Some(service) = self.service.as_ref().and_then(Weak::upgrade)
                && service.epoch() == self.epoch
            {
                // Thread-safe enqueue: the service inserts its marker on its
                // owner thread, after every use preceding the final reference.
                service.defer_release(Box::new(RetirementBatch {
                    owner: self.clone(),
                    bytes,
                }));
                return;
            }
            // A retired device cannot accept new GPU work. Drain accounting
            // without recursively scheduling synchronous invalidation callbacks.
            self.used.fetch_sub(bytes, Ordering::AcqRel);
            self.retiring.fetch_sub(bytes, Ordering::AcqRel);
            self.scheduled.store(false, Ordering::Release);
        }
    }

    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }
    pub fn allocations(&self) -> usize {
        self.allocation_count.load(Ordering::Relaxed)
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
    pub fn has_retirement(&self) -> bool {
        self.retiring.load(Ordering::Acquire) != 0
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        if self.gpu_used.load(Ordering::Relaxed) {
            self.owner.retire(self.bytes);
        } else {
            self.owner.used.fetch_sub(self.bytes, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{GpuBackend, GpuCapabilities, GpuJob, GpuReadback, GpuRect, GpuTexture};
    use std::{any::Any, sync::atomic::AtomicU64};

    struct FakeService {
        identity: u64,
        epoch: AtomicU64,
        ready: AtomicBool,
        fail_poll: AtomicBool,
        queued: Mutex<Vec<Box<dyn Any + Send>>>,
        capabilities: GpuCapabilities,
    }
    impl FakeService {
        fn new() -> Arc<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(u64::MAX / 2);
            Arc::new(Self {
                identity: NEXT.fetch_add(1, Ordering::Relaxed),
                epoch: AtomicU64::new(1),
                ready: AtomicBool::new(false),
                fail_poll: AtomicBool::new(false),
                queued: Mutex::default(),
                capabilities: GpuCapabilities {
                    backend: GpuBackend::Direct3D11,
                    adapter_name: "deterministic retirement fixture".into(),
                    vendor_id: None,
                    device_id: None,
                    max_texture_size: 16384,
                    rgba8_sample_counts: vec![1, 4].into(),
                    compute_shaders: true,
                    max_upload_bytes: 1 << 30,
                    max_pending_readback_bytes: 1 << 28,
                },
            })
        }
        fn queue_len(&self) -> usize {
            self.queued.lock().unwrap().len()
        }
    }
    impl GpuService for FakeService {
        fn service_id(&self) -> u64 {
            self.identity
        }
        fn epoch(&self) -> u64 {
            self.epoch.load(Ordering::Acquire)
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn capabilities(&self) -> &GpuCapabilities {
            &self.capabilities
        }
        fn defer_release(&self, resource: Box<dyn Any + Send>) {
            self.queued.lock().unwrap().push(resource);
        }
        fn poll_deferred(&self) -> Result<bool> {
            ensure!(
                !self.fail_poll.load(Ordering::Acquire),
                "injected completion failure"
            );
            if self.ready.load(Ordering::Acquire) {
                let completed = std::mem::take(&mut *self.queued.lock().unwrap());
                drop(completed);
            }
            Ok(self.queue_len() != 0)
        }
        fn run_job(&self, job: &mut dyn GpuJob) -> Result<()> {
            job.execute(self)
        }
        fn readback(&self, _: &Arc<dyn GpuTexture>, _: GpuRect) -> Result<Box<dyn GpuReadback>> {
            anyhow::bail!("no readback in accounting fixture")
        }
    }
    fn fixture(limit: usize) -> (Arc<FakeService>, Arc<GpuMemory>) {
        let service = FakeService::new();
        let erased: Arc<dyn GpuService> = service.clone();
        let memory = GpuMemory::shared(&erased, Some(limit), 512).unwrap();
        (service, memory)
    }

    #[test]
    fn held_versions_and_failed_reservations_stay_accounted() {
        let (_, memory) = fixture(32);
        let first = memory.reserve(20).unwrap();
        let in_flight = first.clone();
        drop(first);
        assert!(memory.reserve(16).is_err());
        assert_eq!(memory.allocations(), 1);
        assert_eq!(memory.used(), 20);
        drop(in_flight);
        assert_eq!(memory.used(), 0);
        let second = memory.reserve(32).unwrap();
        assert_eq!(memory.peak(), 32);
        drop(second);
        assert!(memory.reserve(usize::MAX).is_err());
        assert_eq!(memory.used(), 0);
        assert_eq!(memory.allocations(), 2);
    }

    #[test]
    fn retirement_coalesces_and_late_drops_require_a_later_completion() {
        let (service, memory) = fixture(128);
        let allocations: Vec<_> = (0..8).map(|_| memory.reserve(16).unwrap()).collect();
        for allocation in allocations {
            allocation.mark_gpu_use();
            drop(allocation);
        }
        assert_eq!(service.queue_len(), 1, "one coalesced batch per device");
        assert_eq!(memory.used(), 128);
        assert!(memory.has_retirement());
        assert!(memory.reserve(1).is_err());
        assert!(service.poll_deferred().unwrap());
        assert_eq!(memory.used(), 128);
        service.ready.store(true, Ordering::Release);
        assert!(service.poll_deferred().unwrap());
        assert_eq!(
            memory.used(),
            112,
            "later drops must not join the earlier fence"
        );
        assert_eq!(service.queue_len(), 1);
        assert!(!service.poll_deferred().unwrap());
        assert_eq!(memory.used(), 0);
        assert!(!memory.has_retirement());
        assert!(memory.reserve(128).is_ok());
    }

    #[test]
    fn poll_errors_preserve_bytes_and_cross_thread_final_references_retire() {
        let (service, memory) = fixture(64);
        let allocation = memory.reserve(64).unwrap();
        allocation.mark_gpu_use();
        std::thread::spawn(move || drop(allocation)).join().unwrap();
        service.fail_poll.store(true, Ordering::Release);
        assert!(service.poll_deferred().is_err());
        assert_eq!(memory.used(), 64);
        assert_eq!(service.queue_len(), 1);
        service.fail_poll.store(false, Ordering::Release);
        service.ready.store(true, Ordering::Release);
        assert!(!service.poll_deferred().unwrap());
        assert_eq!(memory.used(), 0);
    }

    #[test]
    fn device_budget_is_inherited_after_canvas_owners_drop() {
        let (service, memory) = fixture(64);
        let weak = Arc::downgrade(&memory);
        drop(memory);
        let erased: Arc<dyn GpuService> = service.clone();
        let inherited = GpuMemory::shared(&erased, None, 512).unwrap();
        assert!(Arc::ptr_eq(&weak.upgrade().unwrap(), &inherited));
        assert_eq!(inherited.limit(), 64);
        assert!(GpuMemory::shared(&erased, Some(128), 512).is_err());
        assert!(GpuMemory::shared(&erased, Some(64), 512).is_ok());
    }

    #[test]
    fn new_device_epoch_keeps_budget_policy_and_gets_independent_retirement() {
        let (service, old) = fixture(64);
        let old_allocation = old.reserve(64).unwrap();
        old_allocation.mark_gpu_use();
        service.epoch.store(2, Ordering::Release);
        let erased: Arc<dyn GpuService> = service.clone();
        let current = GpuMemory::shared(&erased, None, 512).unwrap();
        assert!(!Arc::ptr_eq(&old, &current));
        assert_eq!(current.limit(), 64);
        assert_eq!(current.used(), 0);
        let allocation = current.reserve(64).unwrap();
        allocation.mark_gpu_use();
        drop(old_allocation);
        assert_eq!(old.used(), 0);
        assert_eq!(current.used(), 64);
        drop(allocation);
        assert_eq!(
            current.used(),
            64,
            "new epoch must retain its completion lease"
        );
        service.ready.store(true, Ordering::Release);
        assert!(!service.poll_deferred().unwrap());
        assert_eq!(current.used(), 0);
    }

    #[test]
    fn device_invalidation_and_teardown_do_not_strand_late_allocation_drops() {
        let (service, memory) = fixture(64);
        let first = memory.reserve(32).unwrap();
        let late = memory.reserve(32).unwrap();
        first.mark_gpu_use();
        late.mark_gpu_use();
        drop(first);
        service.epoch.store(2, Ordering::Release);
        drop(late);
        let abandoned = std::mem::take(&mut *service.queued.lock().unwrap());
        drop(abandoned);
        assert_eq!(memory.used(), 0);
        assert!(!memory.has_retirement());
        let allocation = memory.reserve(64).unwrap();
        allocation.mark_gpu_use();
        drop(service);
        drop(allocation);
        assert_eq!(memory.used(), 0);
    }

    #[test]
    fn gpu_canvas_drop_and_external_frames_retire_on_shared_service() -> Result<()> {
        use crate::{CanvasDocument, CanvasSession, GpuCanvas};
        use gpui_windows::gpu_extensions::OffscreenGpuHost;
        let host = OffscreenGpuHost::new()?;
        let service = host.service();
        let budget = 16 * 1024 * 1024;
        let mut canvas = GpuCanvas::with_memory_budget(service.clone(), budget)?;
        let memory = GpuMemory::shared(&service, None, 512 * 1024 * 1024)?;
        let second = GpuCanvas::new(service.clone())?;
        assert_eq!(memory.limit(), budget);
        drop(second);
        assert!(GpuCanvas::with_memory_budget(service.clone(), 32 * 1024 * 1024).is_err());
        let mut document = CanvasDocument::default();
        let mut session = CanvasSession::new(&mut document)?;
        session.set_surface(&mut document, 128, 128)?;
        let frame = canvas.prepare(
            session.scene(),
            None,
            1,
            1.0,
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )?;
        let peak = memory.peak();
        assert!(peak > 0 && peak <= budget);
        drop(canvas);
        let drain = || -> Result<()> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while service.poll_deferred()? {
                ensure!(
                    std::time::Instant::now() < deadline,
                    "deferred release timed out"
                );
                std::thread::yield_now();
            }
            Ok(())
        };
        drain()?;
        assert!(
            memory.used() > 0,
            "published frame must retain its allocation"
        );
        std::thread::spawn(move || drop(frame)).join().unwrap();
        drain()?;
        assert_eq!(
            memory.used(),
            0,
            "last frame after canvas drop stranded bytes"
        );
        let retry = GpuCanvas::new(service.clone())?;
        assert_eq!(memory.limit(), budget);
        drop(retry);
        Ok(())
    }
}
