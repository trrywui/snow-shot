//! Single-pass raster transport with fixed, reusable, reservation-owning buffers.
use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use snow_image_core::*;

const CHUNK_BYTES: usize = 256 * 1024;
const BUFFERS: usize = 3;

struct Chunk {
    bytes: Vec<u8>,
    length: usize,
    _reservation: Reservation,
}
struct State {
    receiver: mpsc::Receiver<Result<Chunk>>,
    recycle: mpsc::SyncSender<Chunk>,
    current: Option<Chunk>,
    offset: usize,
    next_row: u32,
}

pub struct StreamRaster {
    descriptor: ImageDescriptor,
    metadata: SharedMetadata,
    state: Mutex<State>,
    cancellation: CancellationToken,
    alive: Arc<AtomicBool>,
}

pub struct Producer {
    sender: mpsc::SyncSender<Result<Chunk>>,
    recycle: mpsc::Receiver<Chunk>,
    bytes: u64,
    cancellation: CancellationToken,
    alive: Arc<AtomicBool>,
}

pub fn channel(
    descriptor: ImageDescriptor,
    metadata: SharedMetadata,
    budget: &Budget,
    cancellation: CancellationToken,
) -> Result<(StreamRaster, Producer)> {
    descriptor.validate_with_policy(budget)?;
    let bytes = descriptor.required_bytes()?;
    let capacity = bytes.min(CHUNK_BYTES);
    let (sender, receiver) = mpsc::sync_channel(BUFFERS - 1);
    let (recycle_sender, recycle_receiver) = mpsc::sync_channel(BUFFERS);
    let alive = Arc::new(AtomicBool::new(true));
    for _ in 0..BUFFERS {
        let mut reservation = budget.reserve(ResourceKind::Transport, capacity as u64)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| Error::AllocationFailed)?;
        reservation.resize(bytes.capacity() as u64)?;
        bytes.resize(capacity, 0);
        recycle_sender
            .send(Chunk {
                bytes,
                length: 0,
                _reservation: reservation,
            })
            .map_err(|_| Error::Worker("raster buffer channel disconnected".into()))?;
    }
    Ok((
        StreamRaster {
            descriptor,
            metadata,
            state: Mutex::new(State {
                receiver,
                recycle: recycle_sender,
                current: None,
                offset: 0,
                next_row: 0,
            }),
            cancellation: cancellation.clone(),
            alive: alive.clone(),
        },
        Producer {
            sender,
            recycle: recycle_receiver,
            bytes: bytes as u64,
            cancellation,
            alive,
        },
    ))
}

impl Producer {
    /// Dropping the consumer immediately disconnects both bounded channels. A
    /// producer never drains an unneeded raster after a codec rejects it.
    pub fn read_from(self, reader: &mut impl Read) -> Result<()> {
        let result = (|| {
            let mut left = self.bytes;
            while left != 0 {
                self.cancellation.check()?;
                if !self.alive.load(Ordering::Acquire) {
                    return Err(Error::Cancelled);
                }
                let mut chunk = self.recycle.recv().map_err(|_| Error::Cancelled)?;
                chunk.length = left.min(chunk.bytes.len() as u64) as usize;
                reader.read_exact(&mut chunk.bytes[..chunk.length])?;
                left -= chunk.length as u64;
                self.sender.send(Ok(chunk)).map_err(|_| Error::Cancelled)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Transfer the original I/O or reservation error to the codec reader.
            // If that reader already stopped, all queued chunks drop here.
            let _ = self.sender.send(Err(error));
            return Err(Error::Worker("raster input did not complete".into()));
        }
        Ok(())
    }
}

impl Drop for StreamRaster {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

impl StreamRaster {
    pub fn finish(&self) -> Result<()> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.next_row != self.descriptor.height {
            return Err(Error::Codec(
                "encoder did not consume its declared raster".into(),
            ));
        }
        self.cancellation.check()
    }
}

impl State {
    fn transfer(
        &mut self,
        mut length: usize,
        mut destination: Option<&mut [u8]>,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        let mut written = 0;
        while length != 0 {
            cancellation.check()?;
            if self.current.is_none() {
                let chunk = loop {
                    cancellation.check()?;
                    match self.receiver.recv_timeout(Duration::from_millis(10)) {
                        Ok(chunk) => break chunk?,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err(Error::Io(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "raster input ended before its declared size",
                            )));
                        }
                    }
                };
                self.current = Some(chunk);
                self.offset = 0;
            }
            let chunk = self.current.as_ref().expect("current buffer was filled");
            let amount = length.min(chunk.length - self.offset);
            if let Some(destination) = destination.as_mut() {
                destination[written..written + amount]
                    .copy_from_slice(&chunk.bytes[self.offset..self.offset + amount]);
            }
            self.offset += amount;
            written += amount;
            length -= amount;
            if self.offset == chunk.length {
                let chunk = self.current.take().expect("buffer was present");
                // A completed producer need not receive its final buffers back.
                let _ = self.recycle.try_send(chunk);
            }
        }
        Ok(())
    }
}

impl RasterSource for StreamRaster {
    fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }
    fn access(&self) -> RasterAccess {
        RasterAccess::SequentialRows
    }
    fn metadata(&self) -> Option<&Metadata> {
        Some(self.metadata.get())
    }
    fn read_rows(&self, first: u32, rows: u32, destination: &mut [u8]) -> Result<()> {
        let row_bytes = self.descriptor.row_bytes()?;
        if first
            .checked_add(rows)
            .is_none_or(|end| end > self.descriptor.height)
            || row_bytes.checked_mul(rows as usize) != Some(destination.len())
        {
            return Err(Error::InvalidInput(
                "invalid streamed raster row range".into(),
            ));
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if first != state.next_row {
            return Err(Error::Unsupported(
                "streamed raster requires consecutive rows".into(),
            ));
        }
        for row in destination.chunks_exact_mut(row_bytes) {
            state.transfer(row_bytes, Some(row), &self.cancellation)?;
            state.transfer(
                self.descriptor.row_stride - row_bytes,
                None,
                &self.cancellation,
            )?;
            state.next_row += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(budget: &Budget, width: u32, height: u32, padding: usize) -> (StreamRaster, Producer) {
        let mut descriptor =
            ImageDescriptor::packed(width, height, PixelFormat::RGBA8, ColorEncoding::Srgb)
                .unwrap();
        descriptor.row_stride += padding;
        channel(
            descriptor,
            SharedMetadata::new(Metadata::default(), budget).unwrap(),
            budget,
            CancellationToken::new(),
        )
        .unwrap()
    }

    #[test]
    fn padded_rows_cross_chunks_and_release_bounded_buffers() {
        let budget = Budget::default();
        let (source, producer) = setup(&budget, 17, 20_000, 3);
        std::thread::scope(|scope| {
            let producer = scope.spawn(|| producer.read_from(&mut std::io::repeat(41)));
            let mut row = [0u8; 68];
            for y in 0..20_000 {
                source.read_rows(y, 1, &mut row).unwrap();
                assert_eq!(row, [41; 68]);
            }
            source.finish().unwrap();
            producer.join().unwrap().unwrap();
        });
        assert_eq!(
            budget.peak_used(ResourceKind::Transport),
            (BUFFERS * CHUNK_BYTES) as u64
        );
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        drop(source);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn truncated_producer_preserves_io_error_and_releases_buffers() {
        let budget = Budget::default();
        let (source, producer) = setup(&budget, 1, 2, 0);
        assert!(producer.read_from(&mut &[1, 2, 3][..]).is_err());
        assert!(
            matches!(source.read_rows(0, 1, &mut [0; 4]), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof)
        );
        drop(source);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn early_codec_rejection_disconnects_producer_without_draining() {
        let budget = Budget::default();
        let (source, producer) = setup(&budget, 4096, 4096, 0);
        drop(source);
        let mut input = std::io::Cursor::new(vec![0u8; 16]);
        assert!(producer.read_from(&mut input).is_err());
        assert_eq!(input.position(), 0);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn incomplete_and_backward_reads_are_rejected() {
        let budget = Budget::default();
        let (source, producer) = setup(&budget, 1, 2, 0);
        producer.read_from(&mut &[1; 8][..]).unwrap();
        source.read_rows(0, 1, &mut [0; 4]).unwrap();
        assert!(source.finish().is_err());
        assert!(source.read_rows(0, 1, &mut [0; 4]).is_err());
        source.read_rows(1, 1, &mut [0; 4]).unwrap();
        source.finish().unwrap();
        drop(source);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn cancellation_wakes_waiting_row_reader() {
        let budget = Budget::default();
        let (source, producer) = setup(&budget, 1, 2, 0);
        let token = source.cancellation.clone();
        std::thread::scope(|scope| {
            let read = scope.spawn(|| source.read_rows(0, 1, &mut [0; 4]));
            token.cancel();
            assert!(matches!(read.join().unwrap(), Err(Error::Cancelled)));
        });
        drop(source);
        drop(producer);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn partial_buffer_admission_failure_rolls_back() {
        let budget = Budget::new(ResourcePolicy {
            total_memory_bytes: 12,
            working_bytes_per_job: 12,
            ..ResourcePolicy::default()
        });
        let descriptor =
            ImageDescriptor::packed(1, 2, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        assert!(
            channel(
                descriptor,
                SharedMetadata::new(Metadata::default(), &budget).unwrap(),
                &budget,
                CancellationToken::new()
            )
            .is_err()
        );
        assert_eq!(budget.total_used(), 0);
    }
}
