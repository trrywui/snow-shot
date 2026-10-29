//! Session-scoped observation only: the hook never consumes or synthesizes input.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[cfg(windows)]
use crate::keyboard_overlay::modifier;
use crate::keyboard_overlay::{KeyEvent, KeyboardOverlayConfig};
use crossbeam_channel::{Receiver, Sender};

#[derive(Clone)]
pub struct KeyObservation {
    pub at: Instant,
    pub key: u16,
    pub scan: u32,
    pub down: bool,
    pub layout: usize,
    pub pressed: [u8; 256],
    pub alt_gr: bool,
    pub generation: u64,
    #[cfg(test)]
    pub extra_info: usize,
}

pub struct KeyboardInput {
    pub receiver: Receiver<KeyObservation>,
    pub generation: Arc<AtomicU64>,
    _observer: platform::Observer,
}

impl KeyboardInput {
    pub fn start() -> Result<Self, String> {
        let (sender, receiver) = crossbeam_channel::bounded(256);
        let generation = Arc::new(AtomicU64::new(0));
        let observer = platform::Observer::start(sender, Arc::clone(&generation))?;
        Ok(Self {
            receiver,
            generation,
            _observer: observer,
        })
    }
    /// Bench-only: start observation exactly like [`KeyboardInput::start`] but
    /// retain a channel sender so synthetic key observations can be fed to the
    /// recording pipeline without injecting OS keyboard input.
    #[cfg(feature = "bench-synthetic-input")]
    pub fn start_with_synthetic_sender() -> Result<(Self, Sender<KeyObservation>), String> {
        let (sender, receiver) = crossbeam_channel::bounded(256);
        let generation = Arc::new(AtomicU64::new(0));
        let observer = platform::Observer::start(sender.clone(), Arc::clone(&generation))?;
        Ok((
            Self {
                receiver,
                generation,
                _observer: observer,
            },
            sender,
        ))
    }
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        while self.receiver.try_recv().is_ok() {}
    }
}

impl KeyObservation {
    pub fn event(&self, at_ms: u64, config: &KeyboardOverlayConfig) -> KeyEvent {
        let modifiers = MODIFIER_KEYS
            .into_iter()
            .filter(|&key| self.pressed[usize::from(key)] != 0 && !(self.alt_gr && key == 0xa2))
            .map(|key| (key, key_label(key, self, config)))
            .collect();
        KeyEvent {
            at_ms,
            key: self.key,
            down: self.down,
            label: key_label(self.key, self, config),
            modifiers,
        }
    }
}

/// Physical modifiers, ordered by family and then left before right.
pub const MODIFIER_KEYS: [u16; 8] = [0xa2, 0xa3, 0xa4, 0xa5, 0xa0, 0xa1, 0x5b, 0x5c];

pub(crate) fn modifier_label(key: u16, config: &KeyboardOverlayConfig) -> Option<String> {
    let macos = cfg!(target_os = "macos");
    let label = match key {
        0xa0 => "Left Shift",
        0xa1 => "Right Shift",
        0xa2 if macos => "Left Control",
        0xa3 if macos => "Right Control",
        0xa2 => "Left Ctrl",
        0xa3 => "Right Ctrl",
        0xa4 if macos => "Left Option",
        0xa5 if macos => "Right Option",
        0xa4 => "Left Alt",
        0xa5 => "Right Alt",
        0x5b if macos => "Left Command",
        0x5c if macos => "Right Command",
        0x5b => "Left Win",
        0x5c => "Right Win",
        _ => return None,
    };
    Some(
        config
            .labels
            .get(&key)
            .cloned()
            .unwrap_or_else(|| label.into()),
    )
}

/// IOLLEvent.h device-dependent flags preserve each side even when its peer is held.
#[cfg(any(target_os = "macos", test))]
pub fn macos_modifier_sides(flags: u64) -> [bool; 8] {
    [0x1, 0x2000, 0x20, 0x40, 0x2, 0x4, 0x8, 0x10].map(|mask| flags & mask != 0)
}

fn key_label(key: u16, observation: &KeyObservation, config: &KeyboardOverlayConfig) -> String {
    if key == 0xa5 && observation.alt_gr {
        return "AltGr".into();
    }
    if let Some(label) = modifier_label(key, config) {
        return label;
    }
    if let Some(label) = config.labels.get(&key) {
        return label.clone();
    }
    #[cfg(target_os = "macos")]
    if let Some(label) = match key {
        0x11 => Some("Control"),
        0x12 => Some("Option"),
        0x10 => Some("Shift"),
        _ => None,
    } {
        return label.into();
    }
    if (0x70..=0x87).contains(&key) {
        return format!("F{}", key - 0x6f);
    }
    if (0x60..=0x69).contains(&key) {
        return format!("Num {}", key - 0x60);
    }
    platform::printable_label(key, observation).unwrap_or_else(|| format!("VK {key:02X}"))
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::thread::JoinHandle;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, GetKeyboardLayout, HKL, MAPVK_VK_TO_CHAR, MapVirtualKeyExW, ToUnicodeEx,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;

    struct Context {
        sender: Sender<KeyObservation>,
        generation: Arc<AtomicU64>,
        seen_generation: u64,
        pressed: [u8; 256],
        left_ctrl_time: Option<u32>,
        alt_gr: bool,
    }
    thread_local! { static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) }; }

    fn pressed_keys() -> [u8; 256] {
        let mut keys = [0; 256];
        for (key, state) in keys.iter_mut().enumerate() {
            *state = u8::from(unsafe { GetAsyncKeyState(key as i32) } < 0) * 0x80;
        }
        keys
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
            if down || up {
                let raw = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
                CONTEXT.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(context) = slot.as_mut() else {
                        return;
                    };
                    let key = raw.vkCode as usize;
                    if key >= 256 {
                        return;
                    }
                    let generation = context.generation.load(Ordering::Acquire);
                    if generation != context.seen_generation {
                        context.pressed = pressed_keys();
                        context.alt_gr = false;
                        context.left_ctrl_time = None;
                        context.seen_generation = generation;
                    }
                    let repeat = down && context.pressed[key] != 0;
                    context.pressed[key] = if down { 0x80 } else { 0 };
                    if key == 0xa2 && down {
                        context.left_ctrl_time = Some(raw.time);
                    }
                    if key == 0xa5 {
                        context.alt_gr = down && context.left_ctrl_time == Some(raw.time);
                    }
                    if repeat {
                        return;
                    }
                    let foreground = unsafe { GetForegroundWindow() };
                    let thread_id = unsafe { GetWindowThreadProcessId(foreground, None) };
                    let layout = unsafe { GetKeyboardLayout(thread_id) };
                    let event = KeyObservation {
                        at: Instant::now(),
                        key: key as u16,
                        scan: raw.scanCode,
                        down,
                        layout: layout.0 as usize,
                        pressed: context.pressed,
                        alt_gr: context.alt_gr,
                        generation,
                        #[cfg(test)]
                        extra_info: raw.dwExtraInfo,
                    };
                    if context.sender.try_send(event).is_err() {
                        // The next event carries a fresh generation and physical modifier snapshot.
                        context.generation.fetch_add(1, Ordering::AcqRel);
                    }
                });
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    pub struct Observer {
        thread_id: u32,
        join: Option<JoinHandle<()>>,
    }

    impl Observer {
        pub fn start(
            sender: Sender<KeyObservation>,
            generation: Arc<AtomicU64>,
        ) -> Result<Self, String> {
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let join = std::thread::Builder::new()
                .name("snow-recording-keyboard-hook".into())
                .spawn(move || {
                    snow_core::qos::apply_current_thread();
                    let thread_id = unsafe { GetCurrentThreadId() };
                    let mut message = MSG::default();
                    unsafe {
                        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
                    }
                    CONTEXT.with(|slot| {
                        *slot.borrow_mut() = Some(Context {
                            sender,
                            generation,
                            seen_generation: 0,
                            pressed: pressed_keys(),
                            left_ctrl_time: None,
                            alt_gr: false,
                        })
                    });
                    let hook =
                        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) };
                    match hook {
                        Ok(hook) => {
                            let _ = ready_tx.send(Ok(thread_id));
                            while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {}
                            let _ = unsafe { UnhookWindowsHookEx(hook) };
                        }
                        Err(error) => {
                            let _ = ready_tx.send(Err(error.to_string()));
                        }
                    }
                    CONTEXT.with(|slot| *slot.borrow_mut() = None);
                })
                .map_err(|error| error.to_string())?;
            match ready_rx.recv() {
                Ok(Ok(thread_id)) => Ok(Self {
                    thread_id,
                    join: Some(join),
                }),
                result => {
                    let _ = join.join();
                    Err(match result {
                        Ok(Err(error)) => error,
                        _ => "keyboard hook initialization failed".into(),
                    })
                }
            }
        }
    }
    impl Drop for Observer {
        fn drop(&mut self) {
            let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    pub fn printable_label(key: u16, event: &KeyObservation) -> Option<String> {
        if modifier(key) {
            return None;
        }
        let layout = HKL(event.layout as *mut _);
        let mapped = unsafe { MapVirtualKeyExW(u32::from(key), MAPVK_VK_TO_CHAR, Some(layout)) }
            & 0x7fff_ffff;
        // Key names stay uppercase. Shortcuts display their base key, not control characters.
        if let Some(ch) = char::from_u32(mapped).filter(|ch| ch.is_alphanumeric()) {
            return Some(ch.to_uppercase().collect());
        }
        let state = [0; 256];
        let mut text = [0u16; 8];
        // Flag 4 prevents changing the application's dead-key composition state (Windows 10+).
        let count = unsafe {
            ToUnicodeEx(
                u32::from(key),
                event.scan,
                &state,
                &mut text,
                4,
                Some(layout),
            )
        };
        let count = count.unsigned_abs() as usize;
        if count > 0 && count <= text.len() {
            let label = String::from_utf16_lossy(&text[..count]);
            if !label.chars().any(char::is_control) && !label.trim().is_empty() {
                return Some(label);
            }
        }
        None
    }

    #[cfg(test)]
    mod native_tests {
        use super::*;

        #[test]
        #[ignore = "briefly focuses an isolated native test window; run explicitly"]
        fn keyboard_observer_receives_shortcut_from_isolated_native_window() {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::Threading::AttachThreadInput;
            use windows::Win32::UI::Input::KeyboardAndMouse::*;
            use windows::core::w;
            const MARKER: usize = 0x534e4f57;
            struct Window {
                hwnd: HWND,
                previous: HWND,
            }
            impl Drop for Window {
                fn drop(&mut self) {
                    unsafe {
                        if GetForegroundWindow() == self.hwnd {
                            let _ = SetForegroundWindow(self.previous);
                        }
                        let _ = DestroyWindow(self.hwnd);
                    }
                }
            }
            // Never release a modifier physically held by the person using the machine.
            for key in [0xa2, 0xa0, 83] {
                assert!(
                    unsafe { GetAsyncKeyState(key) } >= 0,
                    "test keys must initially be up"
                );
            }
            let window = unsafe {
                let previous = GetForegroundWindow();
                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("Snow Shot keyboard recording verification"),
                    WS_OVERLAPPEDWINDOW,
                    30,
                    30,
                    380,
                    120,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let window = Window { hwnd, previous };
                let _ = ShowWindow(hwnd, SW_SHOW);
                let foreground_thread = GetWindowThreadProcessId(previous, None);
                let current_thread = GetCurrentThreadId();
                // Use the same foreground activation approach as the repository's native Qt tests.
                let attached = foreground_thread != 0
                    && foreground_thread != current_thread
                    && AttachThreadInput(current_thread, foreground_thread, true).as_bool();
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
                if attached {
                    let _ = AttachThreadInput(current_thread, foreground_thread, false);
                }
                window
            };
            let input = KeyboardInput::start().unwrap();
            let sequence = [
                (0xa2, false),
                (0xa0, false),
                (83, false),
                (83, true),
                (0xa0, true),
                (0xa2, true),
            ];
            let events: Vec<_> = sequence
                .iter()
                .map(|&(key, up)| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(key),
                            dwFlags: if up {
                                KEYEVENTF_KEYUP
                            } else {
                                KEYBD_EVENT_FLAGS(0)
                            },
                            dwExtraInfo: MARKER,
                            ..Default::default()
                        },
                    },
                })
                .collect();
            unsafe {
                assert_eq!(
                    GetForegroundWindow(),
                    window.hwnd,
                    "do not send input to another application"
                );
                assert_eq!(
                    SendInput(&events, std::mem::size_of::<INPUT>() as i32),
                    events.len() as u32
                );
            }
            let deadline = Instant::now() + std::time::Duration::from_secs(2);
            let mut received = Vec::new();
            while received.len() < 6 && Instant::now() < deadline {
                let mut message = MSG::default();
                unsafe {
                    while PeekMessageW(&mut message, Some(window.hwnd), 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                while let Ok(event) = input.receiver.try_recv() {
                    if event.extra_info == MARKER {
                        received.push(event);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(
                received
                    .iter()
                    .map(|e| (e.key, !e.down))
                    .collect::<Vec<_>>(),
                sequence
            );
            let config = KeyboardOverlayConfig {
                font: None,
                keycap_size: 64,
                background_rgba: [0; 4],
                text_rgba: [0; 4],
                border_rgba: [0; 4],
                labels: [(0x11, "Ctrl".into()), (0x10, "Shift".into())].into(),
            };
            let chord = received[2].event(0, &config);
            assert_eq!(chord.label, "S");
            assert_eq!(
                chord
                    .modifiers
                    .iter()
                    .map(|(_, label)| label.as_str())
                    .collect::<Vec<_>>(),
                ["Left Ctrl", "Left Shift"]
            );
            drop(input);
        }

        #[test]
        fn native_keyboard_observer_has_a_repeatable_joined_lifecycle() {
            for _ in 0..3 {
                let (sender, receiver) = crossbeam_channel::bounded(16);
                let observer = Observer::start(sender, Arc::new(AtomicU64::new(0))).unwrap();
                drop(observer);
                while receiver.try_recv().is_ok() {}
                assert!(matches!(
                    receiver.try_recv(),
                    Err(crossbeam_channel::TryRecvError::Disconnected)
                ));
            }
        }

        #[test]
        fn native_hook_copies_key_edges_suppresses_repeat_and_signals_overflow() {
            let (sender, receiver) = crossbeam_channel::bounded(2);
            let generation = Arc::new(AtomicU64::new(0));
            CONTEXT.with(|slot| {
                *slot.borrow_mut() = Some(Context {
                    sender,
                    generation: Arc::clone(&generation),
                    seen_generation: 0,
                    pressed: [0; 256],
                    left_ctrl_time: None,
                    alt_gr: false,
                })
            });
            let send = |key, message, time| {
                let raw = KBDLLHOOKSTRUCT {
                    vkCode: key,
                    scanCode: 30,
                    time,
                    ..Default::default()
                };
                unsafe {
                    hook_proc(
                        HC_ACTION as i32,
                        WPARAM(message as usize),
                        LPARAM((&raw const raw) as isize),
                    );
                }
            };
            send(65, WM_KEYDOWN, 100);
            send(65, WM_KEYDOWN, 110);
            send(65, WM_KEYUP, 120);
            assert_eq!(receiver.len(), 2);
            send(66, WM_KEYDOWN, 130);
            assert_eq!(generation.load(Ordering::Acquire), 1);
            let down = receiver.try_recv().unwrap();
            let up = receiver.try_recv().unwrap();
            assert!(down.down && !up.down);
            assert_eq!(down.key, 65);
            assert_eq!(down.pressed[65], 128);
            assert_eq!(up.pressed[65], 0);
            CONTEXT.with(|slot| *slot.borrow_mut() = None);
        }
    }
}

#[cfg(target_os = "macos")]
impl KeyObservation {
    /// Shared conversion for live preview and encoded native effects.
    pub fn from_macos(
        event: &snow_macos::input::InputEvent,
        at: Instant,
        generation: u64,
    ) -> Option<Self> {
        if !matches!(event.kind, 10..=12) || event.repeat {
            return None;
        }
        let mut pressed = [0; 256];
        for (key, down) in MODIFIER_KEYS
            .into_iter()
            .zip(macos_modifier_sides(event.modifiers))
        {
            pressed[usize::from(key)] = u8::from(down);
        }
        let (key, down) = if event.kind == 12 {
            let key = match event.key_code {
                54 => 0x5c,
                55 => 0x5b,
                56 => 0xa0,
                60 => 0xa1,
                58 => 0xa4,
                61 => 0xa5,
                59 => 0xa2,
                62 => 0xa3,
                _ => return None,
            };
            (key, pressed[usize::from(key)] != 0)
        } else {
            (0x100 + event.key_code, event.kind == 10)
        };
        Some(Self {
            at,
            key,
            down,
            scan: event.keyboard_type,
            layout: event.modifiers as usize,
            pressed,
            alt_gr: false,
            generation,
            #[cfg(test)]
            extra_info: 0,
        })
    }
}
#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use snow_macos::input::{InputObserver, InputStatus};
    pub struct Observer {
        stop: Sender<()>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Observer {
        pub fn start(
            sender: Sender<KeyObservation>,
            generation: Arc<AtomicU64>,
        ) -> Result<Self, String> {
            let input = InputObserver::start(true, false).map_err(|e| e.to_string())?;
            let (stop, stopped) = crossbeam_channel::bounded(1);
            let worker = std::thread::Builder::new().name("snow-keyboard-adapter".into()).spawn(move || {
                snow_core::qos::apply_current_thread();
                let mut input_generation = input.generation();
                loop {
                    // Poll status as well as events, so secure input clears held keys immediately.
                    let status = input.status();
                    let current = input.generation();
                    if current != input_generation { input_generation = current; generation.fetch_add(1, Ordering::AcqRel); }
                    crossbeam_channel::select_biased! {
                        recv(stopped) -> _ => break,
                        recv(input.events) -> event => {
                            let Ok(event) = event else { break; };
                            if status != InputStatus::Active || event.generation != input_generation { continue; }
                            if let Some(at) = event.instant()
                                && let Some(observation) = KeyObservation::from_macos(&event, at, generation.load(Ordering::Acquire))
                                && sender.try_send(observation).is_err() {
                                generation.fetch_add(1, Ordering::AcqRel);
                            }
                        },
                        default(std::time::Duration::from_millis(20)) => {},
                    }
                }
            }).map_err(|e| e.to_string())?;
            Ok(Self {
                stop,
                worker: Some(worker),
            })
        }
    }
    impl Drop for Observer {
        fn drop(&mut self) {
            let _ = self.stop.try_send(());
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    pub fn printable_label(key: u16, observation: &KeyObservation) -> Option<String> {
        let native = key.checked_sub(0x100)?;
        Some(match native {
            36 => "Return".into(),
            48 => "Tab".into(),
            49 => "Space".into(),
            51 => "Delete".into(),
            53 => "Esc".into(),
            123 => "Left".into(),
            124 => "Right".into(),
            125 => "Down".into(),
            126 => "Up".into(),
            _ => snow_macos::text::keyboard_label(
                native,
                observation.scan,
                observation.layout as u64,
            )
            .unwrap_or_else(|| format!("Key {native}")),
        })
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::*;
    pub struct Observer;
    impl Observer {
        pub fn start(_: Sender<KeyObservation>, _: Arc<AtomicU64>) -> Result<Self, String> {
            Err("keyboard recording is supported only on Windows".into())
        }
    }
    pub fn printable_label(_: u16, _: &KeyObservation) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> KeyboardOverlayConfig {
        KeyboardOverlayConfig {
            font: None,
            keycap_size: 64,
            background_rgba: [0; 4],
            text_rgba: [255; 4],
            border_rgba: [0; 4],
            labels: Default::default(),
        }
    }

    #[test]
    fn physical_modifiers_have_distinct_labels_for_single_keys_and_chords() {
        let config = config();
        let mut observation = KeyObservation {
            at: Instant::now(),
            key: 0,
            scan: 0,
            down: true,
            layout: 0,
            pressed: [0; 256],
            alt_gr: false,
            generation: 0,
            extra_info: 0,
        };
        for pair in MODIFIER_KEYS.chunks_exact(2) {
            let labels = [pair[0], pair[1]].map(|key| modifier_label(key, &config).unwrap());
            assert!(labels[0].starts_with("Left "));
            assert!(labels[1].starts_with("Right "));
            for (index, &key) in pair.iter().enumerate() {
                observation.key = key;
                observation.pressed = [0; 256];
                observation.pressed[usize::from(key)] = 128;
                let event = observation.event(0, &config);
                assert_eq!(event.label, labels[index]);
                assert_eq!(event.modifiers, [(key, labels[index].clone())]);
            }
            observation.pressed[usize::from(pair[0])] = 128;
            assert_eq!(
                observation.event(1, &config).modifiers,
                [(pair[0], labels[0].clone()), (pair[1], labels[1].clone())]
            );
            observation.down = false;
            observation.pressed[usize::from(pair[1])] = 0;
            assert_eq!(
                observation.event(2, &config).modifiers,
                [(pair[0], labels[0].clone())]
            );
            observation.down = true;
        }
    }

    #[test]
    fn macos_device_flags_preserve_each_side_on_peer_release() {
        for (index, mask) in [0x1, 0x2000, 0x20, 0x40, 0x2, 0x4, 0x8, 0x10]
            .into_iter()
            .enumerate()
        {
            let mut expected = [false; 8];
            expected[index] = true;
            assert_eq!(macos_modifier_sides(mask | 0x1e0000), expected);
        }
        assert_eq!(macos_modifier_sides(0x207f), [true; 8]);
        assert_eq!(macos_modifier_sides(0x1e0000), [false; 8]);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_keys_preserve_modifier_state_releases_and_generation() {
        let mut input = snow_macos::input::InputEvent {
            kind: 10,
            x: 0.,
            y: 0.,
            timestamp_ns: 0,
            key_code: 36,
            mouse_button: 0,
            scroll_delta_y: 0.0,
            keyboard_type: 40,
            modifiers: (1 << 20) | (1 << 19) | 0x8 | 0x20,
            repeat: false,
            text: [0; 16],
            text_len: 0,
            generation: 9,
        };
        let config = KeyboardOverlayConfig {
            font: None,
            keycap_size: 64,
            background_rgba: [0; 4],
            text_rgba: [0; 4],
            border_rgba: [0; 4],
            labels: Default::default(),
        };
        let observation = KeyObservation::from_macos(&input, Instant::now(), 7).unwrap();
        assert_eq!(observation.generation, 7);
        let event = observation.event(20, &config);
        assert!(event.down);
        assert_eq!(event.label, "Return");
        assert_eq!(
            event.modifiers,
            [(0xa4, "Left Option".into()), (0x5b, "Left Command".into())]
        );
        input.kind = 11;
        assert!(
            !KeyObservation::from_macos(&input, Instant::now(), 8)
                .unwrap()
                .event(30, &config)
                .down
        );
        input.repeat = true;
        assert!(KeyObservation::from_macos(&input, Instant::now(), 8).is_none());
        input.repeat = false;
        input.kind = 12;
        input.key_code = 55;
        input.modifiers = 0;
        let release = KeyObservation::from_macos(&input, Instant::now(), 8).unwrap();
        assert_eq!(release.key, 0x5b);
        assert!(!release.down);
        assert!(release.event(40, &config).modifiers.is_empty());
        for (left_native, right_native, left, right, left_mask, right_mask, family_mask) in [
            (59, 62, 0xa2, 0xa3, 0x1, 0x2000, 1 << 18),
            (58, 61, 0xa4, 0xa5, 0x20, 0x40, 1 << 19),
            (56, 60, 0xa0, 0xa1, 0x2, 0x4, 1 << 17),
            (55, 54, 0x5b, 0x5c, 0x8, 0x10, 1 << 20),
        ] {
            input.key_code = right_native;
            input.modifiers = family_mask | left_mask | right_mask;
            let both = KeyObservation::from_macos(&input, Instant::now(), 8).unwrap();
            assert_eq!(both.key, right);
            assert!(both.down);
            assert_eq!(both.event(50, &config).modifiers.len(), 2);
            input.key_code = left_native;
            input.modifiers = family_mask | right_mask;
            let release = KeyObservation::from_macos(&input, Instant::now(), 8).unwrap();
            assert_eq!(release.key, left);
            assert!(
                !release.down,
                "releasing one side must not look like a press"
            );
            assert_eq!(release.event(60, &config).modifiers[0].0, right);
            input.key_code = right_native;
            input.modifiers = 0;
            let release = KeyObservation::from_macos(&input, Instant::now(), 8).unwrap();
            assert_eq!(release.key, right);
            assert!(!release.down);
            assert!(release.event(70, &config).modifiers.is_empty());
        }
    }
    #[test]
    fn modifier_sides_remain_distinct_and_altgr_hides_only_synthetic_control() {
        let config = KeyboardOverlayConfig {
            font: None,
            keycap_size: 64,
            background_rgba: [0; 4],
            text_rgba: [0; 4],
            border_rgba: [0; 4],
            labels: [
                (0x11, "Ctrl".into()),
                (0x12, "Alt".into()),
                (0xa5, "Right Alt".into()),
            ]
            .into(),
        };
        let mut event = KeyObservation {
            at: Instant::now(),
            key: 65,
            scan: 0,
            down: true,
            layout: 0,
            pressed: [0; 256],
            alt_gr: true,
            generation: 0,
            extra_info: 0,
        };
        event.pressed[0xa2] = 128;
        event.pressed[0xa5] = 128;
        assert_eq!(event.event(0, &config).modifiers, [(0xa5, "AltGr".into())]);
        event.pressed[0xa3] = 128;
        assert_eq!(
            event.event(0, &config).modifiers,
            [
                (
                    0xa3,
                    if cfg!(target_os = "macos") {
                        "Right Control".into()
                    } else {
                        "Right Ctrl".into()
                    }
                ),
                (0xa5, "AltGr".into())
            ]
        );
        event.pressed[0xa4] = 128;
        assert_eq!(event.event(0, &config).modifiers.len(), 3);
        event.alt_gr = false;
        let modifiers = event.event(0, &config).modifiers;
        assert_eq!(
            modifiers.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            [0xa2, 0xa3, 0xa4, 0xa5]
        );
        assert_eq!(modifiers.last().unwrap().1, "Right Alt");
    }
}
