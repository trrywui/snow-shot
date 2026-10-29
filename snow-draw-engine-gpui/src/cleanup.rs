//! Deferred retirement stays on GPUI's owner thread. Immutable pixel resources
//! are Send and can be released by CPU workers independently of the engine.
use crate::CanvasDocument;
use gpui::{App, Global};
use std::{
    collections::VecDeque,
    thread::ThreadId,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct CanvasCleanupDiagnostics {
    pub pending: usize,
    pub completed: u64,
    pub overflow_drops: u64,
    pub last_drop_ms: f64,
    pub maximum_drop_ms: f64,
}
struct Retired {
    document: CanvasDocument,
    owner: ThreadId,
}
#[derive(Default)]
struct RetirementQueue {
    pending: VecDeque<Retired>,
    scheduled: bool,
    stats: CanvasCleanupDiagnostics,
}
impl Global for RetirementQueue {}

/// Cache and pixel payloads are Send; the non-Send document never enters here.
pub(crate) fn retire_send_resources(resources: impl Send + 'static, cx: &App) {
    cx.background_executor()
        .spawn(async move { drop(resources) })
        .detach();
}

pub fn canvas_cleanup_diagnostics(cx: &App) -> CanvasCleanupDiagnostics {
    cx.try_global::<RetirementQueue>()
        .map_or(Default::default(), |q| CanvasCleanupDiagnostics {
            pending: q.pending.len(),
            ..q.stats
        })
}
fn drop_retired(retired: Retired, cx: &mut App, overflow: bool) {
    assert_eq!(
        retired.owner,
        std::thread::current().id(),
        "engine retirement changed its owning thread"
    );
    let start = Instant::now();
    drop(retired.document);
    let q = cx.global_mut::<RetirementQueue>();
    q.stats.completed += 1;
    q.stats.overflow_drops += u64::from(overflow);
    q.stats.last_drop_ms = start.elapsed().as_secs_f64() * 1000.0;
    q.stats.maximum_drop_ms = q.stats.maximum_drop_ms.max(q.stats.last_drop_ms);
}
/// Detaches source assets immediately; defers engine destruction by one frame
/// interval. At most two documents wait; overflow applies owner-thread backpressure.
/// One core drop is indivisible and has no hard time bound.
pub fn retire_canvas_document(mut document: CanvasDocument, cx: &mut App) {
    let resources = document.release_source_resources();
    retire_send_resources(resources, cx);
    if !cx.has_global::<RetirementQueue>() {
        cx.set_global(RetirementQueue::default());
    }
    if cx.global::<RetirementQueue>().pending.len() >= 2 {
        let oldest = cx
            .global_mut::<RetirementQueue>()
            .pending
            .pop_front()
            .unwrap();
        drop_retired(oldest, cx, true);
    }
    cx.global_mut::<RetirementQueue>()
        .pending
        .push_back(Retired {
            document,
            owner: std::thread::current().id(),
        });
    if cx.global::<RetirementQueue>().scheduled {
        return;
    }
    cx.global_mut::<RetirementQueue>().scheduled = true;
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let more = cx.update(|cx| {
                let next = cx.global_mut::<RetirementQueue>().pending.pop_front();
                if let Some(next) = next {
                    drop_retired(next, cx, false);
                }
                let q = cx.global_mut::<RetirementQueue>();
                q.scheduled = !q.pending.is_empty();
                q.scheduled
            });
            if !more {
                break;
            }
        }
    })
    .detach();
}
