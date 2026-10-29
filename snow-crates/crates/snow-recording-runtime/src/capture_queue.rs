//! Bounded capture history shared by direct and deferred recording.
use std::collections::VecDeque;

// One current image, two inbox images, two stream images and one in-flight
// acquisition fit the capture backend's six immutable texture leases.
const INBOX_DEPTH: usize = 2;
pub(crate) const STREAM_DEPTH: usize = 2;

pub(crate) struct CaptureQueue<T> {
    frames: VecDeque<T>,
}

impl<T> Default for CaptureQueue<T> {
    fn default() -> Self {
        Self {
            frames: VecDeque::with_capacity(INBOX_DEPTH),
        }
    }
}

impl<T> CaptureQueue<T> {
    /// Return superseded history so consumers can retain its damage metadata,
    /// while releasing its image before the next capture arrives.
    pub fn push(&mut self, frame: T) -> Option<T> {
        let superseded = (self.frames.len() == INBOX_DEPTH)
            .then(|| self.frames.pop_front().expect("full capture inbox"));
        self.frames.push_back(frame);
        superseded
    }

    pub fn pop_if(&mut self, due: impl FnOnce(&T) -> bool) -> Option<T> {
        if self.frames.front().is_some_and(due) {
            self.frames.pop_front()
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.frames.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snow_core::{event::StreamEvent, stream_queue::StreamQueue};
    use std::sync::{Arc, Weak};

    struct Image(Arc<u64>);
    impl StreamEvent for Image {}

    /// The same immutable-lease ownership as TexturePool, without a GPU.
    struct Images {
        slots: [Weak<u64>; 6],
    }
    impl Images {
        fn new() -> Self {
            Self {
                slots: Default::default(),
            }
        }
        fn acquire(&mut self, value: u64) -> Image {
            let slot = self
                .slots
                .iter_mut()
                .find(|slot| slot.strong_count() == 0)
                .expect("capture consumed an update without capacity to retain it");
            let image = Arc::new(value);
            *slot = Arc::downgrade(&image);
            Image(image)
        }
    }

    #[test]
    fn stalled_output_keeps_capacity_for_the_final_desktop_update() {
        let mut pool = Images::new();
        let stream = StreamQueue::new(STREAM_DEPTH);
        let mut inbox = CaptureQueue::default();
        let mut current = pool.acquire(0);
        // Capture runs ahead of the next output boundary.
        for value in 1..=4 {
            stream.push(pool.acquire(value));
            inbox.push(stream.try_recv().unwrap().0);
        }
        // Stop the consumer while capture fills/replaces its stream queue.
        // The final update must still fit before the desktop becomes static.
        for value in 5..=20 {
            stream.push(pool.acquire(value));
            assert_eq!(*current.0, 0, "held pixels must remain immutable");
        }
        while let Ok((frame, _)) = stream.try_recv() {
            inbox.push(frame);
        }
        while let Some(frame) = inbox.pop_if(|_| true) {
            current = frame;
        }
        assert_eq!(*current.0, 20);
    }

    #[test]
    fn deadline_selection_preserves_future_images_and_clear_releases_leases() {
        let mut inbox = CaptureQueue::default();
        let first = Arc::new(30);
        let future = Arc::new(35);
        inbox.push(first.clone());
        inbox.push(future.clone());
        assert_eq!(*inbox.pop_if(|at| **at <= 33).unwrap(), 30);
        assert!(inbox.pop_if(|at| **at <= 33).is_none());
        assert_eq!(Arc::strong_count(&first), 1);
        assert_eq!(Arc::strong_count(&future), 2);
        inbox.clear();
        assert_eq!(Arc::strong_count(&future), 1);
        // Pause/recovery releases the inbox without changing timestamp policy.
        inbox.push(Arc::new(100));
        assert!(inbox.pop_if(|at| **at <= 99).is_none());
        assert_eq!(*inbox.pop_if(|at| **at <= 100).unwrap(), 100);
    }
}
