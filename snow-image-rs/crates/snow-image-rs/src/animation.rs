//! Lazy animation sessions multiplexed over two serial, supervised worker actors.
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use snow_image_core::*;
use snow_image_protocol::{AnimationDescription, Operation};

use crate::{
    ImageService, Job,
    runtime::{Connection, EncodedInput, Outcome, Payload, ServiceConfig},
};

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn unexpected() -> Error {
    Error::Worker("unexpected animation response".into())
}

/// Timings are returned lazily with frames; opening a session does not decode all frames.
#[derive(Clone, Debug)]
pub struct AnimationInfo {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    /// Zero means infinitely repeating. One means one playback.
    pub loop_count: u32,
}
impl From<&AnimationDescription> for AnimationInfo {
    fn from(value: &AnimationDescription) -> Self {
        Self {
            width: value.width,
            height: value.height,
            frame_count: value.frame_count,
            loop_count: value.loop_count,
        }
    }
}
#[derive(Clone, Debug)]
pub struct AnimationFrame {
    pub image: Image,
    pub index: u32,
    pub duration: Duration,
}

struct SessionHandle {
    id: u64,
    actor: Arc<Actor>,
    info: AnimationInfo,
}
impl Drop for SessionHandle {
    fn drop(&mut self) {
        let mut queue = lock(&self.actor.queue);
        if !queue.stopped {
            // One entry per live session: capacity was admitted before opening it.
            queue.closing.push_back(self.id);
            self.actor.ready.notify_one();
        }
    }
}

/// Clones share playback position. A frame owns its pixels independently of the session.
#[derive(Clone)]
pub struct AnimationSession(Arc<SessionHandle>);
impl AnimationSession {
    pub fn info(&self) -> &AnimationInfo {
        &self.0.info
    }
    pub fn next_frame(&self) -> Job<Option<AnimationFrame>> {
        let id = self.0.id;
        let keepalive = self.0.clone();
        self.0.actor.submit(move |state, cancellation| {
            let _keepalive = keepalive;
            state.advance(id, cancellation)
        })
    }
    pub fn reset(&self) -> Job<()> {
        let id = self.0.id;
        let keepalive = self.0.clone();
        self.0.actor.submit(move |state, cancellation| {
            let _keepalive = keepalive;
            state.reset(id, cancellation)
        })
    }
}

struct LiveSessions {
    count: AtomicUsize,
    maximum: usize,
}
struct SessionPermit(Arc<LiveSessions>);
impl SessionPermit {
    fn acquire(live: &Arc<LiveSessions>) -> Result<Self> {
        live.count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < live.maximum).then_some(count + 1)
            })
            .map_err(|count| Error::ResourceLimit {
                resource: ResourceKind::QueuedWork,
                requested: count as u64 + 1,
                limit: live.maximum as u64,
            })?;
        Ok(Self(live.clone()))
    }
}
impl Drop for SessionPermit {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::AcqRel);
    }
}

type Task = Box<dyn FnOnce(&mut ActorState) + Send>;
struct Queued {
    task: Task,
    _reservation: Reservation,
}
struct Queue {
    work: VecDeque<Queued>,
    closing: VecDeque<u64>,
    stopped: bool,
}
struct Actor {
    queue: Mutex<Queue>,
    ready: Condvar,
    budget: Budget,
    shutdown: CancellationToken,
    maximum: usize,
}
impl Actor {
    fn submit<T: Send + 'static>(
        &self,
        function: impl FnOnce(&mut ActorState, &CancellationToken) -> Result<T> + Send + 'static,
    ) -> Job<T> {
        let (job, completion) = Job::pair();
        let cancellation = job.cancellation_token();
        let reservation = match self.budget.reserve(ResourceKind::QueuedWork, 1) {
            Ok(reservation) => reservation,
            Err(error) => {
                completion.finish(Err(error));
                return job;
            }
        };
        let mut queue = lock(&self.queue);
        if queue.stopped || self.shutdown.is_cancelled() {
            completion.finish(Err(Error::Cancelled));
            return job;
        }
        if queue.work.len() >= self.maximum {
            completion.finish(Err(Error::ResourceLimit {
                resource: ResourceKind::QueuedWork,
                requested: queue.work.len() as u64 + 1,
                limit: self.maximum as u64,
            }));
            return job;
        }
        let shutdown = self.shutdown.clone();
        queue.work.push_back(Queued {
            _reservation: reservation,
            task: Box::new(move |state| {
                let result = cancellation
                    .check()
                    .and_then(|()| shutdown.check())
                    .and_then(|()| function(state, &cancellation));
                // A failed call can reap the process while leaving its connection
                // owner in the actor. Retire it before delivering completion, even
                // when no later command or session drop will wake this actor.
                state.retire_idle_or_failed_connection();
                completion.finish(result);
            }),
        });
        self.ready.notify_one();
        job
    }
    fn stop(&self) {
        self.shutdown.cancel();
        let mut queue = lock(&self.queue);
        queue.stopped = true;
        self.ready.notify_all();
    }
}

struct PoolInner {
    actors: Vec<Arc<Actor>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    choose: AtomicUsize,
    next_session: AtomicU64,
    live: Arc<LiveSessions>,
}
impl Drop for PoolInner {
    fn drop(&mut self) {
        for actor in &self.actors {
            actor.stop();
        }
        let threads = std::mem::take(&mut *lock(&self.threads));
        let _ = std::thread::Builder::new()
            .name("snow-image-animation-reaper".into())
            .spawn(move || {
                for thread in threads {
                    let _ = thread.join();
                }
            });
    }
}

pub(crate) struct AnimationPool(Arc<PoolInner>);
impl AnimationPool {
    pub(crate) fn new(
        config: Arc<ServiceConfig>,
        budget: Budget,
        shutdown: CancellationToken,
    ) -> Result<Self> {
        let mut actors: Vec<Arc<Actor>> = Vec::new();
        let mut threads = Vec::new();
        for index in 0..2 {
            let maximum = config.policy.max_queued_jobs as usize;
            let actor = Arc::new(Actor {
                queue: Mutex::new(Queue {
                    work: VecDeque::with_capacity(maximum),
                    closing: VecDeque::with_capacity(maximum),
                    stopped: false,
                }),
                ready: Condvar::new(),
                budget: budget.clone(),
                shutdown: shutdown.clone(),
                maximum,
            });
            let actor_thread = actor.clone();
            let state = ActorState {
                config: config.clone(),
                budget: budget.clone(),
                shutdown: shutdown.clone(),
                connection: None,
                sessions: HashMap::new(),
            };
            match std::thread::Builder::new()
                .name(format!("snow-image-animation-{index}"))
                .spawn(move || actor_loop(actor_thread, state))
            {
                Ok(thread) => threads.push(thread),
                Err(error) => {
                    for actor in &actors {
                        Actor::stop(actor);
                    }
                    for thread in threads {
                        let _ = thread.join();
                    }
                    return Err(error.into());
                }
            }
            actors.push(actor);
        }
        Ok(Self(Arc::new(PoolInner {
            actors,
            threads: Mutex::new(threads),
            choose: AtomicUsize::new(0),
            next_session: AtomicU64::new(1),
            live: Arc::new(LiveSessions {
                count: AtomicUsize::new(0),
                maximum: config.policy.max_queued_jobs as usize,
            }),
        })))
    }
    pub(crate) fn shutdown(&self) {
        for actor in &self.0.actors {
            actor.stop();
        }
    }
    fn open(&self, input: EncodedInput, options: DecodeOptions) -> Job<AnimationSession> {
        let permit = match SessionPermit::acquire(&self.0.live) {
            Ok(permit) => permit,
            Err(error) => {
                let (job, completion) = Job::pair();
                completion.finish(Err(error));
                return job;
            }
        };
        let id = self.0.next_session.fetch_add(1, Ordering::Relaxed);
        let actor = self.0.actors
            [self.0.choose.fetch_add(1, Ordering::Relaxed) % self.0.actors.len()]
        .clone();
        let handle_actor = actor.clone();
        actor.submit(move |state, cancellation| {
            let (description, generation) = state.open_remote(&input, &options, cancellation)?;
            let info = AnimationInfo::from(&description);
            state.sessions.insert(
                id,
                LocalSession {
                    input,
                    options,
                    remote: description.session,
                    generation,
                    next_index: 0,
                    canvas: (description.width, description.height),
                    frame_count: description.frame_count,
                    metadata: None,
                    _permit: permit,
                },
            );
            Ok(AnimationSession(Arc::new(SessionHandle {
                id,
                actor: handle_actor,
                info,
            })))
        })
    }
}
impl ImageService {
    pub fn open_animation(
        &self,
        input: EncodedInput,
        options: DecodeOptions,
    ) -> Job<AnimationSession> {
        self.0.animation.open(input, options)
    }
}

struct LocalSession {
    input: EncodedInput,
    options: DecodeOptions,
    remote: u64,
    generation: u64,
    next_index: u32,
    canvas: (u32, u32),
    frame_count: u32,
    metadata: Option<SharedMetadata>,
    _permit: SessionPermit,
}
struct ActorState {
    config: Arc<ServiceConfig>,
    budget: Budget,
    shutdown: CancellationToken,
    connection: Option<Connection>,
    sessions: HashMap<u64, LocalSession>,
}
impl ActorState {
    fn supervise_idle_connection(&mut self) {
        if self
            .connection
            .as_mut()
            .is_some_and(|connection| connection.check_idle().is_err())
        {
            // No request is pending while the actor is idle. Retiring the owner
            // kills and reaps the worker before releasing its reservations;
            // sessions reopen through the existing bounded generation replay.
            // Previously published snapshots are owned entirely by the host.
            self.connection.take();
        }
    }

    fn retire_idle_or_failed_connection(&mut self) {
        if self.sessions.is_empty()
            || self
                .connection
                .as_ref()
                .is_some_and(|connection| !connection.healthy())
        {
            self.connection.take();
        }
    }

    fn connection(&mut self) -> Result<&mut Connection> {
        if self
            .connection
            .as_ref()
            .is_some_and(|connection| !connection.healthy())
        {
            self.connection.take();
        }
        if self.connection.is_none() {
            self.shutdown.check()?;
            self.connection = Some(Connection::spawn(
                self.config.clone(),
                self.budget.clone(),
                self.shutdown.clone(),
                true,
            )?);
        }
        Ok(self.connection.as_mut().expect("connection initialized"))
    }
    fn open_remote(
        &mut self,
        input: &EncodedInput,
        options: &DecodeOptions,
        cancellation: &CancellationToken,
    ) -> Result<(AnimationDescription, u64)> {
        let connection = self.connection()?;
        let generation = connection.generation();
        match connection.call(
            Operation::AnimationOpen(options.clone()),
            Payload::Encoded(input.clone()),
            cancellation,
            None,
        )? {
            Outcome::AnimationOpened(info) => Ok((info, generation)),
            _ => Err(unexpected()),
        }
    }
    /// Replay is capped at 64 frames and checked against a two-second scheduling budget.
    /// Each individual native call also obeys the configured operation timeout.
    fn recover(
        &mut self,
        session: &mut LocalSession,
        cancellation: &CancellationToken,
        reset: bool,
    ) -> Result<()> {
        let generation = self.connection()?.generation();
        if session.generation == generation {
            return Ok(());
        }
        if !reset && session.next_index > 64 {
            return Err(Error::Worker(
                "animation recovery exceeds 64 frames; reset the session explicitly".into(),
            ));
        }
        let (description, generation) =
            self.open_remote(&session.input, &session.options, cancellation)?;
        if (description.width, description.height) != session.canvas
            || description.frame_count != session.frame_count
        {
            self.connection.take();
            return Err(Error::Worker(
                "reopened animation changed canvas or frame count".into(),
            ));
        }
        session.remote = description.session;
        session.generation = generation;
        session.metadata = None;
        let count = if reset { 0 } else { session.next_index };
        let deadline = Instant::now() + Duration::from_secs(2);
        for expected in 0..count {
            if let Err(error) = cancellation.check() {
                self.connection.take();
                return Err(error);
            }
            if Instant::now() >= deadline {
                self.connection.take();
                return Err(Error::Worker(
                    "animation replay time budget exceeded; reset the session explicitly".into(),
                ));
            }
            let outcome = self.connection()?.call(
                Operation::AnimationAdvance {
                    session: session.remote,
                },
                Payload::Empty,
                cancellation,
                session.metadata.clone(),
            );
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.connection.take();
                    return Err(error);
                }
            };
            match outcome {
                Outcome::Frame { image, index, .. }
                    if index == expected
                        && (image.descriptor().width, image.descriptor().height)
                            == session.canvas =>
                {
                    session.metadata = Some(image.shared_metadata())
                }
                _ => {
                    self.connection.take();
                    return Err(unexpected());
                }
            }
        }
        if reset {
            session.next_index = 0;
        }
        Ok(())
    }
    fn advance(
        &mut self,
        id: u64,
        cancellation: &CancellationToken,
    ) -> Result<Option<AnimationFrame>> {
        let mut session = self
            .sessions
            .remove(&id)
            .ok_or_else(|| Error::InvalidInput("animation session is closed".into()))?;
        let result = (|| {
            self.recover(&mut session, cancellation, false)?;
            let outcome = self.connection()?.call(
                Operation::AnimationAdvance {
                    session: session.remote,
                },
                Payload::Empty,
                cancellation,
                session.metadata.clone(),
            )?;
            match outcome {
                Outcome::Frame {
                    image,
                    index,
                    duration_ms,
                } => {
                    if index != session.next_index
                        || index >= session.frame_count
                        || (image.descriptor().width, image.descriptor().height) != session.canvas
                    {
                        self.connection.take();
                        return Err(Error::Worker(
                            "animation frame index or canvas changed unexpectedly".into(),
                        ));
                    }
                    session.next_index = index
                        .checked_add(1)
                        .ok_or_else(|| Error::Worker("animation index overflow".into()))?;
                    session.metadata = Some(image.shared_metadata());
                    Ok(Some(AnimationFrame {
                        image,
                        index,
                        duration: Duration::from_millis(duration_ms),
                    }))
                }
                Outcome::Done if session.next_index == session.frame_count => Ok(None),
                Outcome::Done => {
                    self.connection.take();
                    Err(Error::Worker(
                        "animation ended before its advertised frame count".into(),
                    ))
                }
                _ => Err(unexpected()),
            }
        })();
        self.sessions.insert(id, session);
        result
    }
    fn reset(&mut self, id: u64, cancellation: &CancellationToken) -> Result<()> {
        let mut session = self
            .sessions
            .remove(&id)
            .ok_or_else(|| Error::InvalidInput("animation session is closed".into()))?;
        let result = (|| {
            self.recover(&mut session, cancellation, true)?;
            match self.connection()?.call(
                Operation::AnimationReset {
                    session: session.remote,
                },
                Payload::Empty,
                cancellation,
                session.metadata.clone(),
            )? {
                Outcome::Done => {
                    session.next_index = 0;
                    Ok(())
                }
                _ => Err(unexpected()),
            }
        })();
        self.sessions.insert(id, session);
        result
    }
    fn close(&mut self, id: u64) {
        let Some(session) = self.sessions.remove(&id) else {
            return;
        };
        if let Some(connection) = self.connection.as_mut()
            && connection.healthy()
            && connection.generation() == session.generation
        {
            let _ = connection.call(
                Operation::AnimationClose {
                    session: session.remote,
                },
                Payload::Empty,
                &CancellationToken::new(),
                None,
            );
        }
        self.retire_idle_or_failed_connection();
    }
}

fn actor_loop(actor: Arc<Actor>, mut state: ActorState) {
    loop {
        let mut queue = lock(&actor.queue);
        while queue.work.is_empty() && queue.closing.is_empty() && !queue.stopped {
            if state.connection.is_some() {
                let (next_queue, timeout) = actor
                    .ready
                    .wait_timeout(queue, Duration::from_millis(50))
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                queue = next_queue;
                if timeout.timed_out() {
                    // Never hold the submission lock while terminating/reaping
                    // a worker. A queued command will see the new generation.
                    drop(queue);
                    state.supervise_idle_connection();
                    queue = lock(&actor.queue);
                }
            } else {
                queue = actor
                    .ready
                    .wait(queue)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
        if queue.stopped {
            state.connection.take();
            state.sessions.clear();
            let pending = std::mem::take(&mut queue.work);
            drop(queue);
            for Queued { task, _reservation } in pending {
                drop(_reservation);
                task(&mut state);
            }
            break;
        }
        if let Some(id) = queue.closing.pop_front() {
            drop(queue);
            state.close(id);
            continue;
        }
        let work = queue.work.pop_front();
        drop(queue);
        if let Some(Queued { task, _reservation }) = work {
            drop(_reservation);
            task(&mut state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_admission_is_bounded_and_released() {
        let live = Arc::new(LiveSessions {
            count: AtomicUsize::new(0),
            maximum: 1,
        });
        let permit = SessionPermit::acquire(&live).unwrap();
        assert!(SessionPermit::acquire(&live).is_err());
        drop(permit);
        assert!(SessionPermit::acquire(&live).is_ok());
    }
    #[test]
    fn stopped_actor_completes_new_jobs_without_spawning_work() {
        let actor = Actor {
            queue: Mutex::new(Queue {
                work: VecDeque::new(),
                closing: VecDeque::new(),
                stopped: true,
            }),
            ready: Condvar::new(),
            budget: Budget::default(),
            shutdown: CancellationToken::new(),
            maximum: 1,
        };
        assert!(matches!(
            actor.submit::<()>(|_, _| panic!("must not run")).wait(),
            Err(Error::Cancelled)
        ));
        assert_eq!(actor.budget.used(ResourceKind::QueuedWork), 0);
    }
    #[test]
    fn pending_frame_keeps_session_alive_until_cancelled_completion() {
        let budget = Budget::default();
        let shutdown = CancellationToken::new();
        let actor = Arc::new(Actor {
            queue: Mutex::new(Queue {
                work: VecDeque::new(),
                closing: VecDeque::new(),
                stopped: false,
            }),
            ready: Condvar::new(),
            budget: budget.clone(),
            shutdown: shutdown.clone(),
            maximum: 1,
        });
        let session = AnimationSession(Arc::new(SessionHandle {
            id: 7,
            actor: actor.clone(),
            info: AnimationInfo {
                width: 1,
                height: 1,
                frame_count: 1,
                loop_count: 1,
            },
        }));
        let job = session.next_frame();
        job.cancel();
        drop(session);
        assert!(lock(&actor.queue).closing.is_empty());
        let Queued { task, _reservation } = lock(&actor.queue).work.pop_front().unwrap();
        drop(_reservation);
        let mut state = ActorState {
            config: Arc::new(ServiceConfig::new("unused")),
            budget: budget.clone(),
            shutdown,
            connection: None,
            sessions: HashMap::new(),
        };
        task(&mut state);
        assert!(matches!(job.wait(), Err(Error::Cancelled)));
        assert_eq!(lock(&actor.queue).closing.pop_front(), Some(7));
        assert_eq!(budget.used(ResourceKind::QueuedWork), 0);
    }
}
