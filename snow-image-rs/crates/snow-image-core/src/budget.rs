use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
const KINDS: usize = 11;

/// Byte resources participate in the aggregate memory limit except temporary disk.
/// Queue entries and codec threads are counts, not bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(usize)]
pub enum ResourceKind {
    Input,
    Metadata,
    Pixels,
    Scratch,
    Transport,
    EncodedOutput,
    PreparedPixels,
    TempDisk,
    QueuedWork,
    CodecThreads,
    CodecOperations,
}

impl ResourceKind {
    pub fn is_memory(self) -> bool {
        !matches!(
            self,
            Self::TempDisk | Self::QueuedWork | Self::CodecThreads | Self::CodecOperations
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourcePolicy {
    pub max_input_bytes: u64,
    pub max_metadata_bytes: u64,
    pub max_animation_frames: u32,
    pub working_bytes_per_job: u64,
    pub total_memory_bytes: u64,
    pub max_raster_bytes: u64,
    pub max_prepared_bytes: u64,
    pub preview_max_dimension: u32,
    pub max_artifact_bytes: u64,
    pub max_temp_bytes: u64,
    /// Finite pending-work count, validated in the range 1..=1024 before queue allocation.
    pub max_queued_jobs: u32,
    pub max_codec_operations: u32,
    pub max_codec_threads: u32,
    /// Address space and physical footprint are deliberately distinct limits.
    pub worker_address_space_bytes: u64,
    pub worker_memory_bytes: u64,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self {
            max_input_bytes: 256 * MIB,
            max_metadata_bytes: 8 * MIB,
            max_animation_frames: 10_000,
            working_bytes_per_job: 512 * MIB,
            total_memory_bytes: GIB,
            max_raster_bytes: 8 * GIB,
            max_prepared_bytes: 64 * MIB,
            preview_max_dimension: 2048,
            max_artifact_bytes: GIB,
            max_temp_bytes: 16 * GIB,
            max_queued_jobs: 16,
            max_codec_operations: 2,
            max_codec_threads: 8,
            // Modern macOS dyld reserves hundreds of GiB of virtual address space.
            // This explicit virtual limit is independent of the physical footprint cap.
            worker_address_space_bytes: 1024 * GIB,
            worker_memory_bytes: 768 * MIB,
        }
    }
}

impl ResourcePolicy {
    pub fn snow_shot() -> Self {
        Self::default()
    }
    pub fn validate(&self) -> Result<()> {
        if self.total_memory_bytes == 0
            || self.working_bytes_per_job == 0
            || self.max_raster_bytes == 0
            || self.preview_max_dimension == 0
            || self.max_animation_frames == 0
            || !(1..=1024).contains(&self.max_queued_jobs)
            || !(1..=2).contains(&self.max_codec_operations)
            || self.max_codec_threads == 0
            || self.worker_address_space_bytes == 0
            || self.worker_memory_bytes == 0
        {
            return Err(Error::InvalidInput("resource policy needs finite positive working limits, 1–1024 queued jobs, and one or two codec operations".into()));
        }
        Ok(())
    }
    pub fn limit(&self, resource: ResourceKind) -> u64 {
        match resource {
            ResourceKind::Input => self.max_input_bytes,
            ResourceKind::Metadata => self.max_metadata_bytes,
            ResourceKind::Pixels => self.max_raster_bytes,
            ResourceKind::Scratch | ResourceKind::Transport => self.total_memory_bytes,
            ResourceKind::EncodedOutput => self.max_artifact_bytes,
            ResourceKind::PreparedPixels => self.max_prepared_bytes,
            ResourceKind::TempDisk => self.max_temp_bytes,
            ResourceKind::QueuedWork => u64::from(self.max_queued_jobs),
            ResourceKind::CodecThreads => u64::from(self.max_codec_threads),
            ResourceKind::CodecOperations => u64::from(self.max_codec_operations),
        }
    }
}

#[derive(Debug, Default)]
struct Usage {
    resources: [u64; KINDS],
    memory: u64,
    peak_resources: [u64; KINDS],
    peak_memory: u64,
}
#[derive(Debug)]
struct Inner {
    policy: ResourcePolicy,
    usage: Mutex<Usage>,
}
#[derive(Clone, Debug)]
pub struct Budget(Arc<Inner>);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Accounting remains valid when a caller panics while holding an unrelated owner.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Budget {
    pub fn new(policy: ResourcePolicy) -> Self {
        Self(Arc::new(Inner {
            policy,
            usage: Mutex::new(Usage::default()),
        }))
    }
    pub fn policy(&self) -> &ResourcePolicy {
        &self.0.policy
    }
    pub fn used(&self, resource: ResourceKind) -> u64 {
        lock(&self.0.usage).resources[resource as usize]
    }
    pub fn total_used(&self) -> u64 {
        lock(&self.0.usage).memory
    }
    /// Monotonic high-water mark of admitted memory reservations, not physical footprint.
    pub fn peak_total_used(&self) -> u64 {
        lock(&self.0.usage).peak_memory
    }
    pub fn peak_used(&self, resource: ResourceKind) -> u64 {
        lock(&self.0.usage).peak_resources[resource as usize]
    }
    pub fn reserve(&self, resource: ResourceKind, amount: u64) -> Result<Reservation> {
        let mut usage = lock(&self.0.usage);
        let category = checked_limit(
            usage.resources[resource as usize],
            amount,
            self.0.policy.limit(resource),
            resource,
        )?;
        let memory = if resource.is_memory() {
            checked_limit(
                usage.memory,
                amount,
                self.0.policy.total_memory_bytes,
                resource,
            )?
        } else {
            usage.memory
        };
        usage.resources[resource as usize] = category;
        usage.memory = memory;
        usage.peak_memory = usage.peak_memory.max(memory);
        usage.peak_resources[resource as usize] =
            usage.peak_resources[resource as usize].max(category);
        Ok(Reservation {
            budget: self.clone(),
            resource,
            amount,
            job: None,
        })
    }
    pub fn job(&self) -> JobBudget {
        JobBudget {
            budget: self.clone(),
            usage: Arc::new(Mutex::new(0)),
        }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new(ResourcePolicy::default())
    }
}

fn checked_limit(current: u64, amount: u64, limit: u64, resource: ResourceKind) -> Result<u64> {
    match current.checked_add(amount) {
        Some(total) if total <= limit => Ok(total),
        total => Err(Error::ResourceLimit {
            resource,
            requested: total.unwrap_or(u64::MAX),
            limit,
        }),
    }
}

/// Reservations cannot be cloned: transfer ownership or retain them in shared storage.
#[derive(Debug)]
pub struct Reservation {
    budget: Budget,
    resource: ResourceKind,
    amount: u64,
    job: Option<Arc<Mutex<u64>>>,
}
impl Reservation {
    pub(crate) fn budget(&self) -> &Budget {
        &self.budget
    }
    pub fn amount(&self) -> u64 {
        self.amount
    }
    pub fn resource(&self) -> ResourceKind {
        self.resource
    }
    pub fn resize(&mut self, amount: u64) -> Result<()> {
        if amount == self.amount {
            return Ok(());
        }
        if amount > self.amount {
            let delta = amount - self.amount;
            let mut job_usage = self.job.as_ref().map(|job| lock(job));
            let new_job = job_usage
                .as_ref()
                .map(|usage| {
                    checked_limit(
                        **usage,
                        delta,
                        self.budget.policy().working_bytes_per_job,
                        self.resource,
                    )
                })
                .transpose()?;
            let mut additional = self.budget.reserve(self.resource, delta)?;
            // Absorb the charge without running its release path.
            additional.amount = 0;
            if let (Some(usage), Some(new)) = (&mut job_usage, new_job) {
                **usage = new;
            }
        } else {
            let delta = self.amount - amount;
            let mut usage = lock(&self.budget.0.usage);
            usage.resources[self.resource as usize] -= delta;
            if self.resource.is_memory() {
                usage.memory -= delta;
            }
            drop(usage);
            if let Some(job) = &self.job {
                *lock(job) -= delta;
            }
        }
        self.amount = amount;
        Ok(())
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut usage = lock(&self.budget.0.usage);
        usage.resources[self.resource as usize] -= self.amount;
        if self.resource.is_memory() {
            usage.memory -= self.amount;
        }
        drop(usage);
        if let Some(job) = &self.job {
            *lock(job) -= self.amount;
        }
    }
}

#[derive(Clone, Debug)]
pub struct JobBudget {
    budget: Budget,
    usage: Arc<Mutex<u64>>,
}
impl JobBudget {
    pub fn budget(&self) -> &Budget {
        &self.budget
    }
    pub fn used(&self) -> u64 {
        *lock(&self.usage)
    }
    pub fn reserve(&self, resource: ResourceKind, amount: u64) -> Result<Reservation> {
        if !resource.is_memory() {
            return self.budget.reserve(resource, amount);
        }
        let mut usage = lock(&self.usage);
        let total = checked_limit(
            *usage,
            amount,
            self.budget.policy().working_bytes_per_job,
            resource,
        )?;
        let mut reservation = self.budget.reserve(resource, amount)?;
        reservation.job = Some(self.usage.clone());
        *usage = total;
        Ok(reservation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_capacity_is_bounded_before_service_allocates_it() {
        for max_queued_jobs in [0, 1025, u32::MAX] {
            assert!(
                ResourcePolicy {
                    max_queued_jobs,
                    ..ResourcePolicy::default()
                }
                .validate()
                .is_err()
            );
        }
        for max_queued_jobs in [1, 16, 1024] {
            assert!(
                ResourcePolicy {
                    max_queued_jobs,
                    ..ResourcePolicy::default()
                }
                .validate()
                .is_ok()
            );
        }
    }
}
