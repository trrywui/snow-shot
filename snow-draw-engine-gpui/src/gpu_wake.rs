//! Optional event-driven owner-task wakeups. All D3D calls stay on GPUI's owner;
//! the Windows wait callback only wakes the Rust future. Job completion remains
//! authoritative and is checked again after every wake, including the watchdog.
use std::{
    ffi::c_void,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
};

use anyhow::{Context as _, Result, ensure};
use gpui::GpuService;
use gpui_windows::gpu_extensions::DirectXGpuService;
use windows::{
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE},
        Graphics::Direct3D11::{
            D3D11_FENCE_FLAG_NONE, ID3D11Device, ID3D11Device5, ID3D11DeviceContext,
            ID3D11DeviceContext4, ID3D11Fence,
        },
        System::Threading::{
            CloseThreadpoolWait, CreateEventW, CreateThreadpoolWait,
            DisassociateCurrentThreadFromCallback, PTP_CALLBACK_INSTANCE, PTP_WAIT,
            SetThreadpoolWait, WaitForThreadpoolWaitCallbacks,
        },
    },
    core::Interface,
};

pub(crate) struct Notifier {
    service: Arc<dyn GpuService>,
    epoch: u64,
    fence: Option<Fence>,
}
impl Notifier {
    pub fn new(service: Arc<dyn GpuService>) -> Result<Self> {
        let native = service
            .as_any()
            .downcast_ref::<DirectXGpuService>()
            .context("D3D11 completion device missing")?;
        let fence = native.with_context(|gpu| Ok(Fence::new(gpu.device, gpu.immediate_context)))?;
        Ok(Self {
            epoch: service.epoch(),
            service,
            fence,
        })
    }
    pub fn wake(&self) -> Result<Option<GpuCompletionWake>> {
        ensure!(
            self.epoch == self.service.epoch(),
            "Completion device epoch changed"
        );
        let native = self
            .service
            .as_any()
            .downcast_ref::<DirectXGpuService>()
            .context("D3D11 completion device missing")?;
        native.with_context(|_| {
            self.fence
                .as_ref()
                .map(|fence| fence.arm(self.service.clone()))
                .transpose()
        })
    }
}

struct Fence {
    fence: ID3D11Fence,
    context: ID3D11DeviceContext4,
    next: AtomicU64,
}
impl Fence {
    pub fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Option<Self> {
        let device = device.cast::<ID3D11Device5>().ok()?;
        let context = context.cast::<ID3D11DeviceContext4>().ok()?;
        let mut fence = None;
        unsafe { device.CreateFence(0, D3D11_FENCE_FLAG_NONE, &mut fence) }.ok()?;
        Some(Self {
            fence: fence?,
            context,
            next: AtomicU64::new(1),
        })
    }
    /// Caller must hold a validated native owner-context scope.
    pub fn arm(&self, service: Arc<dyn GpuService>) -> Result<GpuCompletionWake> {
        let value = self
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| anyhow::anyhow!("GPU wake fence sequence exhausted"))?;
        // Submit the signal before registering the event. A failed registration
        // therefore cannot leave an event attached to an unreachable fence value.
        unsafe {
            self.context.Signal(&self.fence, value)?;
            self.context.Flush();
        }
        GpuCompletionWake::new(service, self.fence.clone(), value)
    }
}

struct Event {
    handle: HANDLE,
    _fence: ID3D11Fence,
}
// Kernel event handles can cross threads. Shared ownership prevents CloseHandle
// until the wait callback is disarmed and the submitted GPU signal has retired.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}
impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[derive(Default)]
struct State {
    ready: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

/// Awaitable scheduling hint for already submitted work on the runtime's device.
/// It never polls an immediate context from a worker. Dropping a pending wake
/// cancels its callback and retains its event through native GPU retirement.
/// A watchdog also wakes the task so device-loss/cancellation checks can proceed.
pub struct GpuCompletionWake {
    service: Arc<dyn GpuService>,
    event: Option<Arc<Event>>,
    state: Arc<State>,
    wait: PTP_WAIT,
    registered: bool,
}
impl GpuCompletionWake {
    fn new(service: Arc<dyn GpuService>, fence: ID3D11Fence, value: u64) -> Result<Self> {
        let event = Arc::new(Event {
            handle: unsafe { CreateEventW(None, true, false, None) }?,
            _fence: fence,
        });
        let state = Arc::new(State::default());
        let wait = unsafe {
            CreateThreadpoolWait(
                Some(completed),
                Some(Arc::as_ptr(&state).cast_mut().cast()),
                None,
            )
        }
        .context("Creating GPU completion wait")?;
        let mut wake = Self {
            service,
            event: Some(event),
            state,
            wait,
            registered: false,
        };
        let event = wake.event.as_ref().unwrap();
        unsafe { event._fence.SetEventOnCompletion(value, event.handle) }?;
        wake.registered = true;
        // Relative timeout: 500 ms in 100 ns units. Healthy GPU completion uses
        // the event; the timeout only provides progress for cancellation/TDR.
        let ticks = (-5_000_000_i64) as u64;
        let timeout = FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        unsafe { SetThreadpoolWait(wait, Some(event.handle), Some(&timeout)) };
        Ok(wake)
    }
}
impl Future for GpuCompletionWake {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // Register before checking ready to avoid losing a signal between them.
        let mut waker = self.state.waker.lock().unwrap_or_else(|e| e.into_inner());
        if !waker.as_ref().is_some_and(|old| old.will_wake(cx.waker())) {
            *waker = Some(cx.waker().clone());
        }
        if self.state.ready.load(Ordering::Acquire) {
            waker.take();
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}
impl Drop for GpuCompletionWake {
    fn drop(&mut self) {
        unsafe {
            // Disarm first: waiting for callbacks alone does not cancel a wait
            // which has not yet signalled. The callback disassociates before
            // invoking arbitrary executor code, so it cannot wait for itself.
            SetThreadpoolWait(self.wait, None, None);
            WaitForThreadpoolWaitCallbacks(self.wait, true);
            CloseThreadpoolWait(self.wait);
        }
        self.state
            .waker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(event) = self.event.take()
            && self.registered
        {
            // The fence may still hold the event after an early drop/watchdog.
            // This payload contains no service Arc, avoiding a retirement cycle.
            self.service.defer_release(Box::new(event));
        }
    }
}

unsafe extern "system" fn completed(
    instance: PTP_CALLBACK_INSTANCE,
    context: *mut c_void,
    _: PTP_WAIT,
    _: u32,
) {
    // Drop disarms/waits before releasing the raw context's parent Arc. Take an
    // independent Arc before disassociating; the callback can then outlive Drop.
    let raw = context.cast::<State>();
    let state = unsafe {
        Arc::increment_strong_count(raw);
        Arc::from_raw(raw)
    };
    state.ready.store(true, Ordering::Release);
    let waker = state.waker.lock().unwrap_or_else(|e| e.into_inner()).take();
    unsafe { DisassociateCurrentThreadFromCallback(instance) };
    if let Some(waker) = waker {
        waker.wake();
    }
}

#[cfg(test)]
#[path = "gpu_wake_tests.rs"]
mod tests;
