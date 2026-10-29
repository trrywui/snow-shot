//! Capture observations and output deadlines have independent ownership.
use std::time::Duration;

use crate::output_schedule::{OutputSchedule, OutputSlot};

pub(super) struct ScheduledFrame<T> {
    pub frame: T,
    pub revision: u64,
    pub slot: OutputSlot,
}

pub(super) struct VideoSchedule<T> {
    output: OutputSchedule,
    // Retain the latest due slot until the first image is emitted.
    initial_slot: Option<OutputSlot>,
    pending: Option<(Duration, u64, T)>,
    current: Option<(u64, T)>,
    revision: u64,
}

impl<T: Clone> VideoSchedule<T> {
    pub fn new(fps: u32) -> Self {
        let mut output = OutputSchedule::new(fps);
        let initial_slot = output.poll(Duration::ZERO);
        Self {
            output,
            initial_slot,
            pending: None,
            current: None,
            revision: 0,
        }
    }

    pub fn wait(&self, elapsed: Duration, limit: Duration) -> Duration {
        self.output.wait(elapsed).min(limit)
    }

    /// Admit an observation and select the latest due output slot together.
    /// Observations must not be later than `elapsed`. A pending image due at
    /// this slot is selected before a newer, future image can supersede it.
    /// Between slots only the newest observation can affect a later output, so
    /// the producer keeps capacity without retaining a queue of native leases.
    /// The first image uses the latest due slot immediately, including when an
    /// empty poll has already consumed that slot's deadline.
    pub fn poll(
        &mut self,
        elapsed: Duration,
        observation: Option<(Duration, T)>,
    ) -> Option<ScheduledFrame<T>> {
        let mut slot = self.output.poll(elapsed);
        if let Some(initial_slot) = self.initial_slot.as_mut() {
            *initial_slot = slot.unwrap_or(*initial_slot);
            slot = Some(*initial_slot);
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|(at, _, _)| slot.is_some_and(|slot| *at <= slot.at))
        {
            let (_, revision, frame) = self.pending.take().expect("pending observation");
            self.current = Some((revision, frame));
        }
        if let Some((at, frame)) = observation {
            debug_assert!(at <= elapsed);
            self.revision = self
                .revision
                .checked_add(1)
                .expect("capture revision overflow");
            if self.initial_slot.is_some() || slot.is_some_and(|slot| at <= slot.at) {
                self.current = Some((self.revision, frame));
            } else {
                self.pending = Some((at, self.revision, frame));
            }
        }
        let slot = slot?;
        let (revision, frame) = self.current.as_ref()?;
        self.initial_slot = None;
        Some(ScheduledFrame {
            frame: frame.clone(),
            revision: *revision,
            slot,
        })
    }

    /// Pauses, interruptions and configuration changes invalidate image leases,
    /// while the active-time output schedule and generation counter survive.
    pub fn clear(&mut self) {
        self.pending = None;
        self.current = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_capture_is_available_before_the_next_output_deadline() {
        for fps in [1, 30, 60, 240] {
            for at_ms in [10, 2510] {
                for empty_polls in [false, true] {
                    let mut schedule = VideoSchedule::new(fps);
                    let at = Duration::from_millis(at_ms);
                    if empty_polls {
                        for earlier in [Duration::ZERO, at - Duration::from_millis(1)] {
                            assert!(schedule.poll(earlier, None).is_none());
                        }
                    }
                    let first = schedule
                        .poll(at, Some((at, "desktop")))
                        .expect("a short recording must retain its first captured image");
                    assert_eq!(first.frame, "desktop");
                    assert_eq!(first.revision, 1);
                    assert_eq!(first.slot.pts, at_ms * u64::from(fps) / 1000);
                    assert!(schedule.poll(at, None).is_none(), "PTS must not repeat");
                }
            }
        }
    }

    #[test]
    fn captures_after_the_initial_image_still_wait_for_their_output_deadline() {
        let mut schedule = VideoSchedule::new(1);
        let first_at = Duration::from_millis(10);
        let first = schedule.poll(first_at, Some((first_at, 1))).unwrap();
        assert_eq!((first.frame, first.slot.pts), (1, 0));
        let changed_at = Duration::from_millis(20);
        assert!(schedule.poll(changed_at, Some((changed_at, 2))).is_none());
        assert!(schedule.poll(Duration::from_millis(999), None).is_none());
        let changed = schedule.poll(Duration::from_secs(1), None).unwrap();
        assert_eq!(
            (changed.frame, changed.revision, changed.slot.pts),
            (2, 2, 1)
        );
    }

    #[test]
    fn clearing_images_preserves_initial_admission_without_reopening_emitted_slots() {
        let mut schedule = VideoSchedule::new(1);
        assert!(schedule.poll(Duration::ZERO, None).is_none());
        schedule.clear();
        let first_at = Duration::from_millis(10);
        let first = schedule.poll(first_at, Some((first_at, 1))).unwrap();
        assert_eq!((first.frame, first.slot.pts), (1, 0));
        let pending_at = Duration::from_millis(20);
        assert!(schedule.poll(pending_at, Some((pending_at, 2))).is_none());
        schedule.clear();
        let resumed_at = Duration::from_millis(30);
        assert!(schedule.poll(resumed_at, Some((resumed_at, 3))).is_none());
        let resumed = schedule.poll(Duration::from_secs(1), None).unwrap();
        assert_eq!(
            (resumed.frame, resumed.revision, resumed.slot.pts),
            (3, 3, 1)
        );
    }

    #[test]
    fn static_capture_drives_every_output_deadline_without_twenty_millisecond_polling() {
        for fps in [30, 60, 120, 144, 240] {
            let mut schedule = VideoSchedule::new(fps);
            let mut now = Duration::ZERO;
            let mut output = Vec::new();
            let mut observation = Some((Duration::ZERO, "desktop"));
            while now < Duration::from_secs(1) {
                if let Some(frame) = schedule.poll(now, observation.take()) {
                    assert_eq!(frame.frame, "desktop");
                    assert_eq!(frame.revision, 1);
                    output.push(frame.slot.pts);
                }
                let wait = schedule.wait(now, Duration::from_millis(20));
                assert!(!wait.is_zero());
                now += wait;
            }
            assert_eq!(output, (0..u64::from(fps)).collect::<Vec<_>>());
        }
    }

    #[test]
    fn new_captures_wait_for_their_slot_and_held_frames_keep_their_revision() {
        let mut schedule = VideoSchedule::new(60);
        assert_eq!(
            schedule
                .poll(Duration::ZERO, Some((Duration::ZERO, 1)))
                .unwrap()
                .frame,
            1
        );
        assert_eq!(
            schedule
                .poll(
                    Duration::from_millis(21),
                    Some((Duration::from_millis(20), 2)),
                )
                .unwrap()
                .frame,
            1
        );
        let changed = schedule.poll(Duration::from_millis(34), None).unwrap();
        assert_eq!(
            (changed.frame, changed.revision, changed.slot.pts),
            (2, 2, 2)
        );
        assert_eq!(
            schedule
                .poll(Duration::from_millis(51), None)
                .unwrap()
                .revision,
            2
        );
        assert!(schedule.poll(Duration::from_millis(51), None).is_none());
    }

    #[test]
    fn overload_selects_latest_due_image_and_never_replays_missed_slots() {
        let mut schedule = VideoSchedule::new(60);
        schedule.poll(Duration::ZERO, Some((Duration::ZERO, 0)));
        schedule.poll(
            Duration::from_millis(5),
            Some((Duration::from_millis(5), 1)),
        );
        let selected = schedule
            .poll(
                Duration::from_millis(100),
                Some((Duration::from_millis(95), 19)),
            )
            .unwrap();
        assert_eq!((selected.frame, selected.slot.pts), (19, 6));
        assert!(schedule.poll(Duration::from_millis(100), None).is_none());
    }

    #[test]
    fn interruption_discards_old_and_pending_images_without_resetting_pts() {
        let mut schedule = VideoSchedule::new(60);
        schedule
            .poll(Duration::ZERO, Some((Duration::ZERO, 1)))
            .unwrap();
        schedule.poll(
            Duration::from_millis(10),
            Some((Duration::from_millis(10), 2)),
        );
        schedule.clear();
        assert!(schedule.poll(Duration::from_millis(17), None).is_none());
        let resumed = schedule
            .poll(
                Duration::from_millis(34),
                Some((Duration::from_millis(25), 3)),
            )
            .unwrap();
        assert_eq!(
            (resumed.frame, resumed.revision, resumed.slot.pts),
            (3, 3, 2)
        );
    }

    #[test]
    fn due_capture_survives_a_newer_capture_after_the_output_boundary() {
        let mut schedule = VideoSchedule::new(60);
        schedule.poll(Duration::ZERO, Some((Duration::ZERO, 0)));
        assert!(
            schedule
                .poll(
                    Duration::from_millis(10),
                    Some((Duration::from_millis(10), 1)),
                )
                .is_none()
        );
        let first = schedule
            .poll(
                Duration::from_millis(21),
                Some((Duration::from_millis(20), 2)),
            )
            .unwrap();
        assert_eq!((first.frame, first.slot.pts), (1, 1));
        let second = schedule
            .poll(
                Duration::from_millis(34),
                Some((Duration::from_millis(34), 3)),
            )
            .unwrap();
        assert_eq!((second.frame, second.slot.pts), (2, 2));
        let third = schedule.poll(Duration::from_millis(51), None).unwrap();
        assert_eq!((third.frame, third.slot.pts), (3, 3));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn faster_capture_recycles_native_pool_and_preserves_the_final_update() {
        let size = snow_media::geometry::PixelSize::new(16, 16).unwrap();
        let mut producer =
            snow_macos::compositor::Compositor::new(size, snow_media::PixelFormat::Bgra8, 4)
                .unwrap();
        let mut schedule = VideoSchedule::new(1);
        let mut latest_capture = None;
        let mut output = Vec::new();
        for milliseconds in 0..=3000 {
            let now = Duration::from_millis(milliseconds);
            let observation = if milliseconds > 0 && milliseconds < 2000 && milliseconds % 17 == 0 {
                let image = producer.compose(&[], true).unwrap();
                latest_capture = Some(milliseconds);
                Some((now, (milliseconds, image)))
            } else {
                None
            };
            if let Some(frame) = schedule.poll(now, observation) {
                assert_eq!(Some(frame.frame.0), latest_capture);
                output.push((frame.slot.pts, frame.frame.0, frame.revision));
            }
        }
        assert_eq!(output.len(), 4);
        assert_eq!(output[0], (0, 17, 1));
        assert_eq!(output[1].1, 986);
        assert_eq!(output[2].1, 1989);
        assert_eq!(output[2].1, output[3].1);
        assert_eq!(output[2].2, output[3].2);
    }
}
