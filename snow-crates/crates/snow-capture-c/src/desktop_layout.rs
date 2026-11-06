//! Owned geometry cloned from a prepared desktop session, without GPU ownership.
use super::*;

pub struct SnowCaptureDesktopLayoutImpl {
    entries: Vec<MonitorEntry>,
    topology: Option<u64>,
    source: LayoutSource,
}

#[repr(C)]
pub struct SnowCaptureLayoutDiagnostics {
    pub version: u32,
    pub struct_size: u32,
    pub worker_count: usize,
    pub cached_topology_before: u64,
    pub cached_topology_after: u64,
    pub current_topology: u64,
}

/// Read existing state only. This does not enumerate displays, register a callback,
/// refresh workers, or change the calling thread's last capture error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_capture_desktop_session_layout_diagnostics(
    session: *const SnowCaptureDesktopSessionImpl,
    out: *mut SnowCaptureLayoutDiagnostics,
) -> u8 {
    let (Some(session), Some(out)) = (unsafe { session.as_ref() }, unsafe { out.as_mut() }) else {
        return 0;
    };
    if out.version != 1
        || out.struct_size != std::mem::size_of::<SnowCaptureLayoutDiagnostics>() as u32
    {
        return 0;
    }
    out.worker_count = session.workers.len();
    out.cached_topology_before = session.cached_topology_before;
    out.cached_topology_after = session.cached_topology_after;
    out.current_topology = observed_layout_topology();
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_capture_desktop_session_cached_display_id(
    session: *const SnowCaptureDesktopSessionImpl,
    index: usize,
    out: *mut u32,
) -> u8 {
    let (Some(session), Some(out)) = (unsafe { session.as_ref() }, unsafe { out.as_mut() }) else {
        return 0;
    };
    let Some(worker) = session.workers.get(index) else {
        return 0;
    };
    *out = worker.entry.id.macos_display_id().unwrap_or(0);
    1
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SnowCaptureDisplayDescriptor {
    pub version: u32,
    pub struct_size: u32,
    pub stable_id: *const c_char,
    pub name: *const c_char,
    pub display_id: u32,
    pub coordinate_space: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub backing_scale: f64,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub is_primary: u32,
}

fn descriptor(entry: &MonitorEntry) -> SnowCaptureDisplayDescriptor {
    let native = entry.id.desktop_geometry();
    let (x, y, width, height, pixel_width, pixel_height) = native
        .map(|d| (d.x, d.y, d.width, d.height, d.pixel_width, d.pixel_height))
        .unwrap_or((
            f64::from(entry.x),
            f64::from(entry.y),
            f64::from(entry.expected_width),
            f64::from(entry.expected_height),
            entry.expected_width,
            entry.expected_height,
        ));
    SnowCaptureDisplayDescriptor {
        version: 1,
        struct_size: std::mem::size_of::<SnowCaptureDisplayDescriptor>() as u32,
        stable_id: entry.stable_id.as_ptr(),
        name: entry.name.as_ptr(),
        display_id: entry.id.macos_display_id().unwrap_or(0),
        coordinate_space: u32::from(native.is_some()),
        x,
        y,
        width,
        height,
        backing_scale: (f64::from(pixel_width) / width).max(f64::from(pixel_height) / height),
        pixel_width,
        pixel_height,
        is_primary: u32::from(entry.is_primary),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn snow_capture_desktop_session_layout_snapshot(
    session: *mut SnowCaptureDesktopSessionImpl,
    refresh: u8,
) -> *mut SnowCaptureDesktopLayoutImpl {
    let Some(session) = session_mut(session) else {
        return ptr::null_mut();
    };
    match layout_snapshot_with(
        session,
        refresh != 0,
        &current_layout_topology,
        update_monitor_workers,
    ) {
        Ok(layout) => {
            clear_last_error();
            Box::into_raw(Box::new(layout))
        }
        Err(error) => {
            set_last_error(error);
            ptr::null_mut()
        }
    }
}

fn layout_snapshot_with(
    session: &mut SnowCaptureDesktopSessionImpl,
    refresh: bool,
    topology: &impl Fn() -> Result<Option<u64>, String>,
    update: impl FnOnce(&mut SnowCaptureDesktopSessionImpl) -> Result<(), String>,
) -> Result<SnowCaptureDesktopLayoutImpl, String> {
    let source = ensure_layout_current_with(session, refresh, topology, update)?;
    let entries: Vec<_> = session.workers.iter().map(|w| w.entry.clone()).collect();
    if entries.is_empty() {
        return Err("no active displays are available".to_owned());
    }
    Ok(SnowCaptureDesktopLayoutImpl {
        entries,
        topology: session.validated_topology,
        source,
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_capture_desktop_layout_source(
    layout: *const SnowCaptureDesktopLayoutImpl,
) -> u8 {
    unsafe { layout.as_ref() }.map_or(u8::MAX, |layout| layout.source as u8)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_capture_desktop_layout_destroy(
    layout: *mut SnowCaptureDesktopLayoutImpl,
) {
    if !layout.is_null() {
        drop(unsafe { Box::from_raw(layout) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_capture_desktop_layout_count(
    layout: *const SnowCaptureDesktopLayoutImpl,
) -> usize {
    unsafe { layout.as_ref() }.map_or(0, |l| l.entries.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_capture_desktop_layout_display(
    layout: *const SnowCaptureDesktopLayoutImpl,
    index: usize,
    out: *mut SnowCaptureDisplayDescriptor,
) -> u8 {
    let Some(entry) = (unsafe { layout.as_ref() }).and_then(|l| l.entries.get(index)) else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    let value = descriptor(entry);
    if unsafe { (*out).version != 1 || (*out).struct_size != value.struct_size } {
        return 0;
    }
    unsafe {
        out.write(value);
    }
    1
}

fn matches_frames(
    layout: &SnowCaptureDesktopLayoutImpl,
    result: &SnowCaptureScreenshotResultImpl,
) -> bool {
    layout.entries.len() == result.frames.len()
        && layout.entries.iter().all(|expected| {
            result.frames.iter().any(|actual| {
                let d = descriptor(expected);
                expected.id == actual.entry.id
                    && expected.x == actual.entry.x
                    && expected.y == actual.entry.y
                    && d.pixel_width == actual.frame.width()
                    && d.pixel_height == actual.frame.height()
                    && expected.id.desktop_geometry() == actual.entry.id.desktop_geometry()
            })
        })
}

pub(super) fn layout_matches_session(
    layout: &SnowCaptureDesktopLayoutImpl,
    session: &SnowCaptureDesktopSessionImpl,
) -> Result<bool, String> {
    layout_matches_session_with(layout, session, &current_layout_topology)
}

fn layout_matches_session_with(
    layout: &SnowCaptureDesktopLayoutImpl,
    session: &SnowCaptureDesktopSessionImpl,
    current: &impl Fn() -> Result<Option<u64>, String>,
) -> Result<bool, String> {
    let topology = current()?;
    if layout.topology != topology || (topology.is_some() && session.validated_topology != topology)
    {
        return Ok(false);
    }
    let entries: Vec<_> = session.workers.iter().map(|w| w.entry.clone()).collect();
    Ok(same_monitor_layout(&layout.entries, &entries))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_capture_desktop_session_capture_with_layout(
    session: *mut SnowCaptureDesktopSessionImpl,
    request: *const SnowCaptureScreenshotRequest,
    layout: *const SnowCaptureDesktopLayoutImpl,
) -> *mut SnowCaptureScreenshotResultImpl {
    let (Some(session_ref), Some(layout)) =
        (unsafe { session.as_ref() }, unsafe { layout.as_ref() })
    else {
        set_last_error("desktop session or layout is null");
        return ptr::null_mut();
    };
    match layout_matches_session(layout, session_ref) {
        Ok(true) => {}
        Ok(false) => {
            set_last_error("desktop layout changed before capture");
            return ptr::null_mut();
        }
        Err(error) => {
            set_last_error(error);
            return ptr::null_mut();
        }
    }
    let result = unsafe { capture_desktop_session(session, request, Some(layout)) };
    if let Some(frames) = unsafe { result.as_ref() } {
        let validation = layout_matches_session(layout, unsafe { &*session });
        let error = match validation {
            Ok(true) if matches_frames(layout, frames) => None,
            Ok(_) => Some("desktop layout changed during capture".to_owned()),
            Err(error) => Some(error),
        };
        if let Some(error) = error {
            unsafe {
                snow_capture_screenshot_result_destroy(result);
            }
            set_last_error(error);
            return ptr::null_mut();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    fn entry() -> MonitorEntry {
        MonitorEntry {
            id: MonitorId::from_name(1, "test", true),
            stable_id: CString::new("test-id").unwrap(),
            name: CString::new("test").unwrap(),
            x: -20,
            y: 10,
            expected_width: 4,
            expected_height: 2,
            is_primary: true,
        }
    }
    #[test]
    fn layout_diagnostics_read_cached_state_without_refresh_or_error_mutation() {
        let system = CaptureSystem::builder().build().unwrap();
        let (tx, _rx) = mpsc::channel();
        let session = SnowCaptureDesktopSessionImpl {
            system,
            options: CaptureOptions::default(),
            workers: vec![MonitorWorker {
                entry: entry(),
                tx,
                join: None,
            }],
            prepared: true,
            cached_topology_before: 4,
            cached_topology_after: 5,
            validated_topology: None,
        };
        let mut info = SnowCaptureLayoutDiagnostics {
            version: 1,
            struct_size: std::mem::size_of::<SnowCaptureLayoutDiagnostics>() as u32,
            worker_count: 0,
            cached_topology_before: 0,
            cached_topology_after: 0,
            current_topology: 0,
        };
        set_last_error("preserve error");
        assert_eq!(
            unsafe { snow_capture_desktop_session_layout_diagnostics(&session, &mut info) },
            1
        );
        assert_eq!(
            (
                info.worker_count,
                info.cached_topology_before,
                info.cached_topology_after
            ),
            (1, 4, 5)
        );
        let mut id = u32::MAX;
        assert_eq!(
            unsafe { snow_capture_desktop_session_cached_display_id(&session, 0, &mut id) },
            1
        );
        assert_eq!(id, if cfg!(target_os = "macos") { 1 } else { 0 });
        assert_eq!(
            unsafe { snow_capture_desktop_session_cached_display_id(&session, 1, &mut id) },
            0
        );
        info.version = 2;
        assert_eq!(
            unsafe { snow_capture_desktop_session_layout_diagnostics(&session, &mut info) },
            0
        );
        assert_eq!(
            unsafe { CStr::from_ptr(snow_capture_last_error_message()) }
                .to_str()
                .unwrap(),
            "preserve error"
        );
        assert_eq!(session.workers.len(), 1);
        assert!(session.prepared);
        clear_last_error();
    }
    #[test]
    fn snapshot_without_refresh_returns_the_existing_cached_display() {
        let system = CaptureSystem::builder().build().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut cached = entry();
        cached.id = MonitorId::from_parts(0, 42, 42, "display:42", true);
        let mut session = SnowCaptureDesktopSessionImpl {
            system,
            options: CaptureOptions::default(),
            workers: vec![MonitorWorker {
                entry: cached,
                tx,
                join: None,
            }],
            prepared: true,
            cached_topology_before: 7,
            cached_topology_after: 7,
            validated_topology: Some(7),
        };
        let layout = layout_snapshot_with(&mut session, false, &|| Ok(Some(7)), |_| {
            panic!("valid cached layout must not enumerate")
        })
        .unwrap();
        assert_eq!(unsafe { snow_capture_desktop_layout_count(&layout) }, 1);
        let mut output = descriptor(&session.workers[0].entry);
        assert_eq!(
            unsafe { snow_capture_desktop_layout_display(&layout, 0, &mut output) },
            1
        );
        assert_eq!(
            output.display_id,
            if cfg!(target_os = "macos") { 42 } else { 0 }
        );
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert_eq!(
            (
                session.cached_topology_before,
                session.cached_topology_after
            ),
            (7, 7)
        );
        assert_eq!(layout.source, LayoutSource::CacheReuse);
        assert!(layout_matches_session_with(&layout, &session, &|| Ok(Some(7))).unwrap());
        assert!(!layout_matches_session_with(&layout, &session, &|| Ok(Some(8))).unwrap());
    }
    #[test]
    fn changed_topology_refreshes_cached_id_before_snapshot_once() {
        let system = CaptureSystem::builder().build().unwrap();
        let (tx, _rx) = mpsc::channel();
        let mut cached = entry();
        cached.id = MonitorId::from_parts(0, 42, 42, "display:42", true);
        cached.stable_id = CString::new("display:42").unwrap();
        cached.expected_width = 3360;
        cached.expected_height = 2100;
        let mut session = SnowCaptureDesktopSessionImpl {
            system,
            options: CaptureOptions::default(),
            workers: vec![MonitorWorker {
                entry: cached,
                tx,
                join: None,
            }],
            prepared: true,
            cached_topology_before: 7,
            cached_topology_after: 7,
            validated_topology: Some(7),
        };
        let generation = Cell::new(9);
        let updates = Cell::new(0);
        let current = || Ok(Some(generation.get()));
        let layout = layout_snapshot_with(&mut session, false, &current, |session| {
            updates.set(updates.get() + 1);
            session.workers[0].entry.id = MonitorId::from_parts(0, 43, 43, "display:43", true);
            session.workers[0].entry.stable_id = CString::new("display:43").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(updates.get(), 1);
        assert_eq!(layout.source, LayoutSource::TopologyChanged);
        assert!(layout.entries[0].id == MonitorId::from_parts(0, 43, 43, "display:43", true));
        assert_eq!(
            (
                layout.entries[0].expected_width,
                layout.entries[0].expected_height
            ),
            (3360, 2100)
        );
        assert_eq!(session.validated_topology, Some(9));
        assert!(layout_matches_session_with(&layout, &session, &current).unwrap());
        let next = layout_snapshot_with(&mut session, false, &current, |_| {
            panic!("valid cache must be reused")
        })
        .unwrap();
        assert_eq!(next.source, LayoutSource::CacheReuse);
        assert_eq!(updates.get(), 1);
    }
    #[test]
    fn empty_and_explicit_refresh_report_actual_source() {
        let system = CaptureSystem::builder().build().unwrap();
        let mut session = SnowCaptureDesktopSessionImpl {
            system,
            options: CaptureOptions::default(),
            workers: Vec::new(),
            prepared: false,
            cached_topology_before: 0,
            cached_topology_after: 0,
            validated_topology: None,
        };
        let updates = Cell::new(0);
        let (tx, _rx) = mpsc::channel();
        let first = layout_snapshot_with(&mut session, false, &|| Ok(Some(5)), |session| {
            updates.set(updates.get() + 1);
            session.workers.push(MonitorWorker {
                entry: entry(),
                tx,
                join: None,
            });
            Ok(())
        })
        .unwrap();
        assert_eq!(first.source, LayoutSource::EmptyCache);
        let second = layout_snapshot_with(&mut session, true, &|| Ok(Some(5)), |_| {
            updates.set(updates.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(second.source, LayoutSource::RefreshRequested);
        assert_eq!(updates.get(), 2);
    }
    #[test]
    fn failed_or_unstable_refresh_never_validates_cached_workers() {
        let system = CaptureSystem::builder().build().unwrap();
        let (tx, _rx) = mpsc::channel();
        let mut session = SnowCaptureDesktopSessionImpl {
            system,
            options: CaptureOptions::default(),
            workers: vec![MonitorWorker {
                entry: entry(),
                tx,
                join: None,
            }],
            prepared: true,
            cached_topology_before: 7,
            cached_topology_after: 7,
            validated_topology: Some(7),
        };
        let generation = Cell::new(8);
        let current = || Ok(Some(generation.get()));
        let result = layout_snapshot_with(&mut session, false, &current, |_| {
            Err("injected refresh failure".to_owned())
        });
        assert!(result.is_err());
        assert_eq!(session.validated_topology, None);
        let result = layout_snapshot_with(&mut session, false, &current, |_| {
            generation.set(9);
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(session.validated_topology, None);
        assert_eq!(
            (
                session.cached_topology_before,
                session.cached_topology_after
            ),
            (8, 9)
        );
        let result = layout_snapshot_with(&mut session, false, &current, |_| Ok(()));
        assert_eq!(result.unwrap().source, LayoutSource::TopologyChanged);
        assert_eq!(session.validated_topology, Some(9));
        let result = layout_snapshot_with(
            &mut session,
            false,
            &|| Err("display notifications unavailable".to_owned()),
            |_| panic!("unavailable notifications cannot be treated as an unchanged layout"),
        );
        assert!(result.is_err());
        assert_eq!(session.validated_topology, None);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn wake_during_capture_discards_direct_and_fixed_layout_results() {
        use objc2::runtime::AnyObject;

        #[link(name = "AppKit", kind = "framework")]
        unsafe extern "C" {
            static NSWorkspaceDidWakeNotification: *const AnyObject;
        }

        let request = SnowCaptureScreenshotRequest {
            version: SCREENSHOT_REQUEST_VERSION,
            struct_size: SCREENSHOT_REQUEST_SIZE,
            flags: 0,
            reserved0: 0,
            focused_window: 0,
            cancellation_token: ptr::null(),
            reserved: [0; 32],
        };
        for fixed_layout in [false, true] {
            let generation = snow_macos::desktop::display_topology().unwrap();
            let system = CaptureSystem::builder().build().unwrap();
            let (tx, rx) = mpsc::channel();
            let join = thread::spawn(move || {
                let WorkerCommand::Capture(reply, _, _, _) = rx.recv().unwrap() else {
                    panic!("expected one capture request");
                };
                objc2::rc::autoreleasepool(|_| unsafe {
                    let workspace: *mut AnyObject =
                        objc2::msg_send![objc2::class!(NSWorkspace), sharedWorkspace];
                    let center: *mut AnyObject = objc2::msg_send![workspace, notificationCenter];
                    let _: () = objc2::msg_send![
                        center,
                        postNotificationName: NSWorkspaceDidWakeNotification,
                        object: ptr::null::<AnyObject>()
                    ];
                });
                reply
                    .send(Ok(Frame::from_rgba8(4, 2, vec![0; 32]).unwrap()))
                    .unwrap();
            });
            let display = entry();
            let layout = SnowCaptureDesktopLayoutImpl {
                entries: vec![display.clone()],
                topology: Some(generation),
                source: LayoutSource::CacheReuse,
            };
            let mut session = SnowCaptureDesktopSessionImpl {
                system,
                options: CaptureOptions::default(),
                workers: vec![MonitorWorker {
                    entry: display,
                    tx,
                    join: Some(join),
                }],
                prepared: true,
                cached_topology_before: generation,
                cached_topology_after: generation,
                validated_topology: Some(generation),
            };
            let result = if fixed_layout {
                unsafe {
                    snow_capture_desktop_session_capture_with_layout(
                        &mut session,
                        &request,
                        &layout,
                    )
                }
            } else {
                unsafe { snow_capture_desktop_session_capture(&mut session, &request) }
            };
            assert!(result.is_null());
            assert_eq!(
                unsafe { CStr::from_ptr(snow_capture_last_error_message()) }
                    .to_str()
                    .unwrap(),
                "desktop layout changed during capture"
            );
        }
    }
    #[test]
    fn snapshot_owns_strings_and_validates_descriptor_version() {
        let source = entry();
        let snapshot = SnowCaptureDesktopLayoutImpl {
            entries: vec![source.clone()],
            topology: None,
            source: LayoutSource::CacheReuse,
        };
        drop(source);
        let mut output = descriptor(&snapshot.entries[0]);
        output.version = 2;
        assert_eq!(
            unsafe { snow_capture_desktop_layout_display(&snapshot, 0, &mut output) },
            0
        );
        output.version = 1;
        assert_eq!(
            unsafe { snow_capture_desktop_layout_display(&snapshot, 0, &mut output) },
            1
        );
        assert_eq!(
            unsafe { CStr::from_ptr(output.name) }.to_str().unwrap(),
            "test"
        );
        assert_eq!((output.x, output.pixel_width), (-20.0, 4));
        assert_eq!(
            unsafe { snow_capture_desktop_layout_display(&snapshot, 1, &mut output) },
            0
        );
    }
    #[test]
    fn frame_geometry_must_match_snapshot() {
        let expected = entry();
        let mut result = SnowCaptureScreenshotResultImpl {
            frames: vec![SnapshotFrame {
                entry: expected.clone(),
                frame: Arc::new(Frame::from_rgba8(4, 2, vec![0; 32]).unwrap()),
            }],
            focused_window: None,
        };
        let snapshot = SnowCaptureDesktopLayoutImpl {
            entries: vec![expected],
            topology: None,
            source: LayoutSource::CacheReuse,
        };
        assert!(matches_frames(&snapshot, &result));
        result.frames[0].entry.x += 1;
        assert!(!matches_frames(&snapshot, &result));
    }
}
