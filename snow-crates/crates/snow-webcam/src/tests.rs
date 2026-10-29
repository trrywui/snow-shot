use super::*;
use std::sync::atomic::AtomicUsize;

fn raw(at: Instant, width: u32, height: u32) -> RawFrame {
    let mut rgba = vec![0; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[(x * 20) as u8, (y * 40) as u8, 99, 255]);
        }
    }
    RawFrame {
        width,
        height,
        stride: width as usize * 4,
        rgba,
        at,
    }
}
fn eventually(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "camera worker did not settle");
        std::thread::yield_now();
    }
}
#[test]
fn crop_is_centered_top_down_unmirrored_and_reuses_its_plan() {
    let mut resize = resize::CropResize::default();
    let image = raw(Instant::now(), 8, 4);
    let first = resize.convert(&image, 4).unwrap();
    assert_eq!(&first[..4], &[40, 0, 99, 255]);
    assert_eq!(&first[12..16], &[100, 0, 99, 255]);
    assert_eq!(&first[48..52], &[40, 120, 99, 255]);
    assert_eq!(resize.convert(&image, 4).unwrap(), first);
    let tall = raw(Instant::now(), 4, 8);
    assert_eq!(&resize.convert(&tall, 4).unwrap()[..4], &[0, 80, 99, 255]);
    let mut invalid = image;
    invalid.stride = 1;
    assert!(resize.convert(&invalid, 4).is_err());
    let packed = raw(Instant::now(), 4, 4);
    let mut padded = RawFrame {
        stride: 24,
        rgba: vec![0; 88],
        ..raw(packed.at, 4, 4)
    };
    for y in 0..4 {
        padded.rgba[y * 24..y * 24 + 16].copy_from_slice(&packed.rgba[y * 16..y * 16 + 16]);
    }
    assert_eq!(
        resize.convert(&padded, 4).unwrap(),
        packed.rgba,
        "the final row need not include trailing native pixel-buffer padding"
    );
}
struct Fake {
    frames: crossbeam_channel::Receiver<Result<RawFrame, String>>,
    live: Arc<AtomicUsize>,
}
impl CameraBackend for Fake {
    fn poll(&mut self, timeout: Duration) -> Result<Option<RawFrame>, String> {
        match self.frames.recv_timeout(timeout) {
            Ok(frame) => frame.map(Some),
            Err(_) => Ok(None),
        }
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::AcqRel);
    }
}
#[test]
fn source_is_lazy_shared_bounded_and_drops_disconnected_and_paused_frames() {
    let (tx, rx) = bounded(1);
    let live = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    let live_factory = live.clone();
    let starts_factory = starts.clone();
    let source = Source::with_backend(CaptureOptions { size: 50, fps: 30 }, move |capacity| {
        assert_eq!(
            capacity,
            CaptureOptions { size: 500, fps: 30 },
            "one native format must cover every editable size and admission rate"
        );
        assert_eq!(
            live_factory.fetch_add(1, Ordering::AcqRel),
            0,
            "never open competing camera sessions"
        );
        starts_factory.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(Fake {
            frames: rx.clone(),
            live: live_factory.clone(),
        }))
    })
    .unwrap();
    assert_eq!(starts.load(Ordering::Acquire), 0);
    source.set_active(true);
    eventually(|| live.load(Ordering::Acquire) == 1);
    let retained = source.clone();
    let at = Instant::now();
    for i in 0..10 {
        let time = at + Duration::from_millis(i * 40);
        tx.send(Ok(raw(time, 8, 4))).unwrap();
        eventually(|| {
            source
                .snapshot(time)
                .frame
                .as_ref()
                .is_some_and(|f| f.at == time)
        });
    }
    assert_eq!(source.0.shared.state.lock().unwrap().frames.len(), 4);
    assert_eq!(starts.load(Ordering::Acquire), 1);
    assert!(
        source.snapshot(at).frame.is_none(),
        "superseded frames are retired"
    );
    assert!(
        source.snapshot(at + Duration::from_secs(4)).frame.is_none(),
        "stalled images cannot linger"
    );
    source.set_active(false);
    assert!(
        source
            .snapshot(at + Duration::from_millis(400))
            .frame
            .is_none()
    );
    eventually(|| live.load(Ordering::Acquire) == 0);
    source.set_active(true);
    eventually(|| starts.load(Ordering::Acquire) == 2);
    assert!(
        source.snapshot(Instant::now()).frame.is_none(),
        "resume needs a fresh sample"
    );
    tx.send(Err("device disconnected".into())).unwrap();
    eventually(|| source.snapshot(Instant::now()).error.is_some());
    assert!(source.snapshot(Instant::now()).frame.is_none());
    eventually(|| live.load(Ordering::Acquire) == 0);
    drop(source);
    assert!(retained.snapshot(Instant::now()).error.is_some());
    drop(retained);
}
#[test]
fn resizing_keeps_the_camera_and_last_frame_until_a_fresh_size_arrives() {
    let (tx, rx) = bounded(1);
    let live = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    let capture_live = live.clone();
    let capture_starts = starts.clone();
    let source = Source::with_backend(CaptureOptions { size: 50, fps: 30 }, move |_| {
        if capture_starts.fetch_add(1, Ordering::AcqRel) != 0 {
            return Err("camera cannot be reopened during an active stream".into());
        }
        capture_live.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(Fake {
            frames: rx.clone(),
            live: capture_live.clone(),
        }))
    })
    .unwrap();
    source.set_active(true);
    let at = Instant::now();
    tx.send(Ok(raw(at, 8, 4))).unwrap();
    eventually(|| source.snapshot(at).frame.is_some());
    let initial = source.snapshot(at).frame.unwrap();
    for size in [500, 75, 300, 50, 220] {
        source.configure(CaptureOptions { size, fps: 30 }).unwrap();
        let retained = source.snapshot(at);
        assert!(
            retained
                .frame
                .as_ref()
                .is_some_and(|f| Arc::ptr_eq(f, &initial)),
            "layout resizing must not clear the displayed camera sample"
        );
        assert!(retained.error.is_none());
    }
    let clock = snow_core::recording_clock::RecordingClock::new(at - Duration::from_millis(1));
    assert!(
        source
            .recording_snapshot(&clock, Duration::from_secs(1))
            .frame
            .is_none(),
        "recording must not encode a retained preview sample with the old dimensions"
    );
    let next = at + Duration::from_millis(40);
    tx.send(Ok(raw(next, 8, 4))).unwrap();
    eventually(|| {
        source
            .snapshot(next)
            .frame
            .is_some_and(|f| f.at == next && f.size == 220)
    });
    source
        .configure(CaptureOptions { size: 220, fps: 15 })
        .unwrap();
    let skipped = at + Duration::from_millis(80);
    let admitted = at + Duration::from_millis(120);
    tx.send(Ok(raw(skipped, 8, 4))).unwrap();
    tx.send(Ok(raw(admitted, 8, 4))).unwrap();
    eventually(|| {
        source
            .snapshot(admitted)
            .frame
            .is_some_and(|f| f.at == admitted)
    });
    assert_eq!(
        source.snapshot(skipped).frame.unwrap().at,
        next,
        "a live cadence change must apply admission without reconnecting"
    );
    assert_eq!(starts.load(Ordering::Acquire), 1);
    assert_eq!(live.load(Ordering::Acquire), 1);
    assert_eq!(initial.size, 50, "retained immutable frames remain valid");
    tx.send(Err("disconnected".into())).unwrap();
    eventually(|| source.snapshot(admitted).error.is_some());
    source
        .configure(CaptureOptions { size: 150, fps: 15 })
        .unwrap();
    assert!(
        source.snapshot(admitted).error.is_some(),
        "resizing cannot clear a device failure"
    );
    assert!(source.snapshot(admitted).frame.is_none());
    source.set_active(false);
    eventually(|| live.load(Ordering::Acquire) == 0);
}

#[test]
fn invalid_options_and_startup_failures_do_not_open_or_retry_a_camera() {
    assert!(Source::new(CaptureOptions { size: 49, fps: 30 }).is_err());
    assert!(Source::new(CaptureOptions { size: 500, fps: 31 }).is_err());
    let starts = Arc::new(AtomicUsize::new(0));
    let observed = starts.clone();
    let source = Source::with_backend(CaptureOptions { size: 500, fps: 1 }, move |_| {
        observed.fetch_add(1, Ordering::AcqRel);
        Err("access denied".into())
    })
    .unwrap();
    source.set_active(true);
    eventually(|| source.snapshot(Instant::now()).error.is_some());
    source.set_active(true);
    assert_eq!(starts.load(Ordering::Acquire), 1);
    source.set_recording_active(false);
    assert!(source.snapshot(Instant::now()).error.is_some());
    source.set_recording_active(true);
    assert!(
        source.snapshot(Instant::now()).error.is_some(),
        "resume must preserve the failure latch"
    );
    assert_eq!(starts.load(Ordering::Acquire), 1);
    source.retry_failed();
    source.set_active(true);
    eventually(|| starts.load(Ordering::Acquire) == 2);
    eventually(|| source.snapshot(Instant::now()).error.is_some());
}

#[test]
fn recording_clock_rejects_preview_paused_and_future_samples() {
    use snow_core::recording_clock::RecordingClock;
    let source =
        Source::with_backend(CaptureOptions { size: 50, fps: 30 }, |_| unreachable!()).unwrap();
    let origin = Instant::now() - Duration::from_secs(1);
    let clock = RecordingClock::new(origin);
    clock
        .controller()
        .mark_pause(origin + Duration::from_millis(200));
    clock
        .controller()
        .mark_resume(origin + Duration::from_millis(500));
    for (revision, ms) in [(1, -100i64), (2, 100), (3, 300), (4, 600)] {
        let at = if ms < 0 {
            origin - Duration::from_millis(ms.unsigned_abs())
        } else {
            origin + Duration::from_millis(ms as u64)
        };
        source
            .0
            .shared
            .state
            .lock()
            .unwrap()
            .frames
            .push_back(Arc::new(Frame {
                revision,
                at,
                size: 50,
                rgba: Arc::new(vec![255; 10000]),
            }));
    }
    assert!(
        source
            .recording_snapshot(&clock, Duration::ZERO)
            .frame
            .is_none()
    );
    assert_eq!(
        source
            .recording_snapshot(&clock, Duration::from_millis(250))
            .frame
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        source
            .recording_snapshot(&clock, Duration::from_millis(300))
            .frame
            .unwrap()
            .revision,
        4
    );
}

#[test]
fn late_callbacks_and_repeated_teardown_cannot_restore_a_retired_frame() {
    for _ in 0..8 {
        let (tx, rx) = bounded(1);
        let live = Arc::new(AtomicUsize::new(0));
        let capture_live = live.clone();
        let source = Source::with_backend(CaptureOptions { size: 50, fps: 30 }, move |_| {
            capture_live.fetch_add(1, Ordering::AcqRel);
            Ok(Box::new(Fake {
                frames: rx.clone(),
                live: capture_live.clone(),
            }))
        })
        .unwrap();
        source.set_active(true);
        eventually(|| live.load(Ordering::Acquire) == 1);
        source.set_recording_active(false);
        let _ = tx.try_send(Ok(raw(Instant::now(), 8, 4)));
        eventually(|| live.load(Ordering::Acquire) == 0);
        assert!(source.snapshot(Instant::now()).frame.is_none());
        drop(source);
        eventually(|| {
            matches!(
                tx.try_send(Err("late disconnect".into())),
                Err(crossbeam_channel::TrySendError::Disconnected(_))
            )
        });
    }
}
