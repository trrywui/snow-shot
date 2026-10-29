use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};

use snow_image_core::{CancellationToken, Result};

struct State<T> {
    result: Option<Result<T>>,
    waker: Option<Waker>,
}
struct Shared<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

/// A runtime-independent future. Dropping an unfinished job requests cancellation.
#[must_use = "jobs are cancelled when dropped; await or wait for the result"]
pub struct Job<T> {
    shared: Arc<Shared<T>>,
    cancellation: CancellationToken,
    complete: bool,
}
pub(crate) struct Completion<T>(Arc<Shared<T>>);

impl<T> Job<T> {
    pub(crate) fn pair() -> (Self, Completion<T>) {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                result: None,
                waker: None,
            }),
            ready: Condvar::new(),
        });
        (
            Self {
                shared: shared.clone(),
                cancellation: CancellationToken::new(),
                complete: false,
            },
            Completion(shared),
        )
    }
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    /// Blocks the calling thread. UI callers should await the job instead.
    pub fn wait(mut self) -> Result<T> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.result.is_none() {
            state = self
                .shared
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        self.complete = true;
        state.result.take().expect("completion was checked")
    }
}

impl<T> Future for Job<T> {
    type Output = Result<T>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(result) = state.result.take() {
                Some(result)
            } else {
                if state
                    .waker
                    .as_ref()
                    .is_none_or(|w| !w.will_wake(cx.waker()))
                {
                    state.waker = Some(cx.waker().clone());
                }
                None
            }
        };
        match result {
            Some(result) => {
                self.complete = true;
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}
impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        if !self.complete {
            self.cancellation.cancel();
        }
    }
}
impl<T> Completion<T> {
    pub fn finish(self, result: Result<T>) {
        let waker = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.result = Some(result);
            state.waker.take()
        };
        self.0.ready.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropped_job_cancels_but_completed_wait_does_not() {
        let (job, complete) = Job::<u8>::pair();
        let cancellation = job.cancellation_token();
        complete.finish(Ok(9));
        assert_eq!(job.wait().unwrap(), 9);
        assert!(cancellation.check().is_ok());
        let (job, _complete) = Job::<u8>::pair();
        let cancellation = job.cancellation_token();
        drop(job);
        assert!(cancellation.check().is_err());
    }
    #[test]
    fn wait_observes_cross_thread_completion() {
        let (job, complete) = Job::pair();
        let thread = std::thread::spawn(move || complete.finish(Ok(17)));
        assert_eq!(job.wait().unwrap(), 17);
        thread.join().unwrap();
    }
    #[test]
    fn future_wakes_and_transfers_ownership_exactly_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::Wake;
        struct Counter(AtomicUsize);
        impl Wake for Counter {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let mut context = Context::from_waker(&waker);
        let (mut job, completion) = Job::pair();
        assert!(Pin::new(&mut job).poll(&mut context).is_pending());
        completion.finish(Ok(vec![7, 8, 9]));
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        assert!(
            matches!(Pin::new(&mut job).poll(&mut context), Poll::Ready(Ok(value)) if value == vec![7, 8, 9])
        );
    }
}
