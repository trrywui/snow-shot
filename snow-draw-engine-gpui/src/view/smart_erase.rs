use super::*;
use crate::smart_erase_service::{
    GpuLayeredInput, GpuPatchInput, GpuPatchKey, GpuPatchRequest, SmartEraseJob,
};
use gpui::{AsyncWindowContext, WeakEntity};
use std::{collections::VecDeque, task::Poll, time::Duration};

trait GpuWork {
    type Output;
    fn poll_work(&mut self) -> Poll<Result<Self::Output, CanvasError>>;
    fn commands_ready(&self) -> bool;
}
macro_rules! gpu_work {
    ($kind:ty,$output:ty) => {
        impl GpuWork for $kind {
            type Output = $output;
            fn poll_work(&mut self) -> Poll<Result<Self::Output, CanvasError>> {
                self.poll()
            }
            fn commands_ready(&self) -> bool {
                self.has_pending_submissions()
            }
        }
    };
}
gpu_work!(crate::GpuSmartEraseJob, crate::GpuSmartEraseOutput);
gpu_work!(crate::GpuSourcePrepareJob, crate::GpuSourcePrepared);
gpu_work!(crate::GpuSourceSolveJob, crate::GpuSmartEraseOutput);

async fn finish_gpu<J: GpuWork>(
    this: &WeakEntity<CanvasView>,
    cx: &mut AsyncWindowContext,
    mut job: J,
) -> Result<J::Output, CanvasError> {
    loop {
        let polled = this
            .update_in(cx, |_, _, _| job.poll_work())
            .map_err(|_| CanvasError::Cancelled)?;
        match polled {
            Poll::Ready(result) => return result,
            Poll::Pending => {
                if job.commands_ready() {
                    yield_gpu(cx, true).await;
                } else {
                    let wake = this
                        .update_in(cx, |this, _, _| {
                            this.gpu_smart_erase
                                .as_ref()
                                .expect("GPU runtime")
                                .completion_wake()
                        })
                        .map_err(|_| CanvasError::Cancelled)??;
                    if let Some(wake) = wake {
                        wake.await;
                    } else {
                        yield_gpu(cx, false).await;
                    }
                }
            }
        }
    }
}
async fn yield_gpu(cx: &AsyncWindowContext, commands_ready: bool) {
    if commands_ready {
        cx.foreground_executor().spawn(async {}).await;
    } else {
        cx.background_executor()
            .timer(Duration::from_millis(1))
            .await;
    }
}
fn ensure_runtime(
    this: &mut CanvasView,
    service: &Arc<dyn gpui::GpuService>,
) -> Result<(), CanvasError> {
    if !this
        .gpu_smart_erase
        .as_ref()
        .is_some_and(|runtime| runtime.supports_service(service))
    {
        this.gpu_smart_erase = Some(crate::GpuSmartErase::new(service.clone())?);
    }
    Ok(())
}
async fn solve_request(
    this: &WeakEntity<CanvasView>,
    cx: &mut AsyncWindowContext,
    service: &Arc<dyn gpui::GpuService>,
    input: GpuPatchInput,
    key: &GpuPatchKey,
    cancel: &crate::CanvasCancellation,
) -> Result<crate::GpuSmartEraseOutput, CanvasError> {
    this.update_in(cx, |this, _, _| ensure_runtime(this, service))
        .map_err(|_| CanvasError::Cancelled)??;
    let mut plan = match input {
        GpuPatchInput::Prepared(input) => {
            let job = this
                .update_in(cx, |this, _, _| {
                    this.gpu_smart_erase
                        .as_ref()
                        .expect("GPU runtime")
                        .start(input, cancel.clone())
                })
                .map_err(|_| CanvasError::Cancelled)??;
            return finish_gpu(this, cx, job).await;
        }
        GpuPatchInput::Source(plan) => *plan,
    };
    loop {
        cancel.check()?;
        let GpuLayeredInput {
            donors,
            mask,
            context,
            halo,
        } = plan;
        let mut donors = this
            .update_in(cx, |this, _, _| {
                if !this
                    .gpu_donors
                    .as_ref()
                    .is_some_and(|runtime| runtime.supports_service(service))
                {
                    this.gpu_donors = Some(crate::GpuDonorAssembler::new(service.clone())?);
                }
                this.gpu_donors
                    .as_ref()
                    .expect("GPU donor runtime")
                    .start(donors, cancel.clone())
            })
            .map_err(|_| CanvasError::Cancelled)??;
        let mut completed = None;
        while donors.has_pending_submissions() {
            match this
                .update_in(cx, |_, _, _| donors.poll())
                .map_err(|_| CanvasError::Cancelled)?
            {
                Poll::Ready(result) => {
                    completed = Some(result?.source);
                    break;
                }
                Poll::Pending => {
                    if donors.has_pending_submissions() {
                        yield_gpu(cx, true).await;
                    }
                }
            }
        }
        let source_finished = completed.is_some();
        let prepare = this
            .update_in(cx, |this, _, _| {
                let runtime = this.gpu_smart_erase.as_ref().expect("GPU runtime");
                let mut job = if let Some(source) = completed {
                    runtime.prepare_source(source, mask, cancel.clone())
                } else {
                    runtime.prepare_assembled(&mut donors, mask, cancel.clone())
                }?;
                job.set_donor_context(context)?;
                Ok::<_, CanvasError>(job)
            })
            .map_err(|_| CanvasError::Cancelled)??;
        let prepared = finish_gpu(this, cx, prepare).await?;
        // The downstream completion also covers assembly. Observe it when ready
        // to retire coordinate buffers, without introducing another wait.
        if !source_finished
            && let Poll::Ready(Err(error)) = this
                .update_in(cx, |_, _, _| donors.poll())
                .map_err(|_| CanvasError::Cancelled)?
        {
            return Err(error);
        }
        drop(donors);
        if prepared.masked_pixels != 0
            && context.needs_margin(
                prepared.hole_bounds.map(|v| v as usize),
                (prepared.domain_reach as usize + 16).min(512),
            )
        {
            let filter = key.filter.clone();
            let source = key.fallback_source.clone();
            let worker_cancel = cancel.clone();
            plan = cx
                .background_executor()
                .spawn(async move {
                    GpuLayeredInput::new(
                        &filter,
                        &source,
                        Some((halo * 2).min(512)),
                        &worker_cancel,
                    )
                })
                .await?
                .ok_or(draw::ErrorCode::InvalidState)?;
            continue;
        }
        let prepared = prepared.with_search_span(context.search_span as u32)?;
        let job = this
            .update_in(cx, |this, _, _| {
                this.gpu_smart_erase
                    .as_ref()
                    .expect("GPU runtime")
                    .start_prepared(prepared, cancel.clone())
            })
            .map_err(|_| CanvasError::Cancelled)??;
        return finish_gpu(this, cx, job).await;
    }
}

impl CanvasView {
    pub(super) fn start_gpu_smart_task(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        job: SmartEraseJob,
        service: Arc<dyn gpui::GpuService>,
        generation: u64,
        cancel: crate::CanvasCancellation,
    ) {
        let worker_cancel = cancel.clone();
        let automatic = self.options.smart_erase_execution == crate::SmartEraseExecution::Automatic;
        self.smart_task = Some(cx.spawn_in(window, async move |this, cx| {
            let preparation = cx
                .background_executor()
                .spawn(async move {
                    if automatic {
                        job.prepare_automatic(&worker_cancel)
                    } else {
                        job.prepare_gpu(&worker_cancel)
                    }
                })
                .await;
            let mut warning;
            match preparation {
                Err(error) => {
                    warning = Some(error.to_string());
                }
                Ok(preparation) => {
                    debug_assert_eq!(preparation.revision, generation);
                    warning = preparation.warnings.into_iter().next();
                    let _ = this.update_in(cx, |this, _, cx| {
                        if cancel.check().is_ok() {
                            this.smart_service.seed(&preparation.snapshot);
                            this.rendered_generation = this.generation.wrapping_sub(1);
                            cx.notify();
                        }
                    });
                    for GpuPatchRequest { input, key } in preparation.requests {
                        if cancel.check().is_err() {
                            break;
                        }
                        let result = solve_request(&this, cx, &service, input, &key, &cancel).await;
                        let patch = match result {
                            Ok(output) => Some(key.complete(output)),
                            Err(CanvasError::Cancelled) => break,
                            Err(_) => {
                                // Capability/allocation/device failures retain the same
                                // source-grid CPU fallback; no incomplete GPU result is seeded.
                                let worker_cancel = cancel.clone();
                                match cx
                                    .background_executor()
                                    .spawn(async move {
                                        crate::SmartEraseService::default().solve(
                                            &key.filter,
                                            &key.fallback_source,
                                            &worker_cancel,
                                        )
                                    })
                                    .await
                                {
                                    Ok(patch) => patch,
                                    Err(CanvasError::Cancelled) => break,
                                    Err(error) => {
                                        if warning.is_none() {
                                            warning = Some(error.to_string());
                                        }
                                        None
                                    }
                                }
                            }
                        };
                        if let Some(patch) = patch {
                            let _ = this.update_in(cx, |this, _, cx| {
                                if cancel.check().is_ok() {
                                    this.smart_service.seed(&crate::SmartEraseSnapshot {
                                        entries: VecDeque::from([patch]),
                                    });
                                    this.rendered_generation = this.generation.wrapping_sub(1);
                                    cx.notify();
                                }
                            });
                        }
                    }
                }
            }
            let _ = this.update_in(cx, |this, _, cx| {
                this.smart_task = None;
                if cancel.check().is_ok() && generation == this.generation {
                    this.smart_warning = warning;
                }
                this.rendered_generation = this.generation.wrapping_sub(1);
                cx.notify();
            });
        }));
    }
}
