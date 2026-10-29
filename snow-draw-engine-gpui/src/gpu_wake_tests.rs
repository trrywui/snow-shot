use super::*;
use crate::{GpuSmartErase, GpuSmartEraseInput, SourceImage};
use gpui_windows::gpu_extensions::OffscreenGpuHost;
use std::{
    sync::{Condvar, atomic::AtomicUsize},
    task::Wake,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Notify {
    count: AtomicUsize,
    signal: Mutex<bool>,
    changed: Condvar,
}
impl Wake for Notify {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.count.fetch_add(1, Ordering::SeqCst);
        *self.signal.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
fn await_wake(mut wake: GpuCompletionWake) {
    let notify = Arc::new(Notify::default());
    let waker = Waker::from(notify.clone());
    let mut cx = Context::from_waker(&waker);
    let deadline = Instant::now() + Duration::from_secs(5);
    if Pin::new(&mut wake).poll(&mut cx).is_pending() {
        let signal = notify.signal.lock().unwrap();
        let (signal, _) = notify
            .changed
            .wait_timeout_while(
                signal,
                deadline.saturating_duration_since(Instant::now()),
                |s| !*s,
            )
            .unwrap();
        assert!(*signal, "pending GPU event failed to wake its task");
        assert!(notify.count.load(Ordering::SeqCst) > 0);
        assert!(Pin::new(&mut wake).poll(&mut cx).is_ready());
    }
}

#[test]
fn smart_erase_completion_events_resume_real_compute_without_worker_context_calls() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let runtime = GpuSmartErase::new(service.clone())?;
    let Some(wake) = runtime.completion_wake()? else {
        return Ok(());
    };
    drop(wake);
    let source = SourceImage::new(image::RgbaImage::from_fn(192, 160, |x, y| {
        let n = ((x * 31 + y * 17) % 49) as u8;
        image::Rgba([70 + n, 110 + n, 150 + n, 255])
    }))?;
    let mask = (0..192 * 160)
        .map(|i| u8::from((70..118).contains(&(i % 192)) && (64..96).contains(&(i / 192))))
        .collect::<Vec<_>>();
    let mut job = runtime.start(
        GpuSmartEraseInput::new(&source, &mask, &Default::default())?,
        Default::default(),
    )?;
    let mut waits = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Poll::Ready(output) = job.poll() {
            assert!(output?.reconstructed.gpu_texture().is_some());
            break;
        }
        if !job.has_pending_submissions() {
            await_wake(runtime.completion_wake()?.unwrap());
            waits += 1;
        }
        assert!(Instant::now() < deadline);
    }
    assert!(
        waits >= 2,
        "metadata and result retirement did not both use an event"
    );
    Ok(())
}

#[test]
fn smart_erase_completion_wake_retains_events_on_early_drop_and_handles_pre_signalling()
-> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let notifier = Notifier::new(service.clone())?;
    if notifier.fence.is_none() {
        return Ok(());
    }
    for _ in 0..64 {
        let wake = notifier.wake()?.unwrap();
        let event = Arc::downgrade(wake.event.as_ref().unwrap());
        drop(wake);
        assert!(
            event.upgrade().is_some(),
            "early drop closed a GPU-registered event"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while service.poll_deferred()? {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(
            event.upgrade().is_none(),
            "completed event failed to retire"
        );
    }
    let mut wake = notifier.wake()?.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !wake.state.ready.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let waker = Waker::from(Arc::new(Notify::default()));
    assert!(
        Pin::new(&mut wake)
            .poll(&mut Context::from_waker(&waker))
            .is_ready(),
        "completion before first poll was lost"
    );
    drop(wake);
    let pending = notifier.wake()?.unwrap();
    let event = Arc::downgrade(pending.event.as_ref().unwrap());
    drop(host);
    drop(pending);
    assert!(
        event.upgrade().is_none(),
        "device retirement retained a wake event"
    );
    Ok(())
}

#[test]
fn smart_erase_completion_wake_validates_owner_epoch_and_optional_capability() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let mut runtime = Notifier::new(host.service())?;
    runtime.epoch = runtime.service.epoch().wrapping_add(1);
    assert!(runtime.wake().is_err(), "stale notifier armed a new epoch");
    runtime.epoch = runtime.service.epoch();
    runtime.fence = None;
    assert!(
        runtime.wake()?.is_none(),
        "optional capability must retain timer polling"
    );
    let runtime = Arc::new(runtime);
    let other = runtime.clone();
    assert!(
        std::thread::spawn(move || other.wake().is_err())
            .join()
            .unwrap(),
        "foreign-thread wake submitted to the immediate context"
    );
    drop(host);
    assert!(runtime.wake().is_err(), "retired device armed another wake");
    Ok(())
}

#[test]
fn gpu_canvas_completion_events_use_the_existing_service_without_image_readback() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let mut canvas = crate::GpuCanvas::new(service.clone())?;
    let before = service.diagnostics().readback_bytes;
    let mut previous_work = service.completion()?;
    if let Some(wake) = canvas.completion_wake()? {
        await_wake(wake);
        assert!(previous_work.poll()?, "frame wake preceded submitted work");
    }
    assert!(canvas.is_ready()?);
    assert_eq!(service.diagnostics().readback_bytes, before);
    drop(host);
    assert!(
        canvas.completion_wake().is_err(),
        "retired renderer armed another wake"
    );
    Ok(())
}
