//! Optional, bounded camera-track replay on the source recording clock.
use crate::{RecordingExportError, deferred::SequentialVideoSource, error::Result};
use snow_core::cancellation::CancellationToken;
use snow_recording_model::{
    BundleAssetKind, FinalizedTimeline, VideoIndexReader, WebcamStyle, webcam::WebcamMetadata,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

pub(crate) struct WebcamReplay {
    video: SequentialVideoSource,
    metadata: WebcamMetadata,
    bundle: PathBuf,
    index_range: (u64, u64),
    last: Option<u64>,
    cached: Option<Arc<snow_webcam::Frame>>,
    // The decoder must close before Windows removes its temporary source.
    directory: tempfile::TempDir,
}
impl WebcamReplay {
    pub fn open(path: &Path, cancel: &CancellationToken) -> Result<Option<Self>> {
        let footer = snow_recording_model::read_recording_bundle_footer(path)?;
        let Some(metadata) = footer.asset(BundleAssetKind::WebcamMetadata, None) else {
            return Ok(None);
        };
        let video = footer
            .asset(BundleAssetKind::WebcamVideo, None)
            .ok_or_else(|| invalid("webcam video is missing"))?;
        let index = footer
            .asset(BundleAssetKind::WebcamIndex, None)
            .ok_or_else(|| invalid("webcam index is missing"))?;
        if metadata.len > 4 * 1024 * 1024 {
            return Err(invalid("webcam metadata exceeds its limit"));
        }
        let mut input = File::open(path)?;
        input.seek(SeekFrom::Start(metadata.offset))?;
        let metadata =
            snow_recording_model::webcam::read_metadata((&mut input).take(metadata.len))?;
        let directory = tempfile::Builder::new()
            .prefix("snow-webcam-replay-")
            .tempdir()?;
        let video_path = directory.path().join("webcam.mp4");
        let mut output = File::create(&video_path)?;
        input.seek(SeekFrom::Start(video.offset))?;
        let mut input = input.take(video.len);
        let mut buffer = vec![0; 256 * 1024];
        loop {
            crate::clip::check_cancel(cancel)?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
        }
        output.flush()?;
        drop(output);
        let index_range = (index.offset, index.len);
        let video = Self::decoder(path, &video_path, index_range, &metadata, cancel)?;
        Ok(Some(Self {
            video,
            metadata,
            bundle: path.to_path_buf(),
            index_range,
            last: None,
            cached: None,
            directory,
        }))
    }
    fn decoder(
        bundle: &Path,
        video: &Path,
        range: (u64, u64),
        metadata: &WebcamMetadata,
        cancel: &CancellationToken,
    ) -> Result<SequentialVideoSource> {
        let index = VideoIndexReader::from_range(File::open(bundle)?, range.0, range.1)?;
        let timeline =
            FinalizedTimeline::new(metadata.duration_ms, metadata.fps).map_err(invalid)?;
        let side = metadata.style.size;
        let coded = side.div_ceil(2) * 2;
        let mut decoder = SequentialVideoSource::open_indexed(
            video,
            index,
            timeline,
            false,
            (coded, coded),
            (side, side),
            cancel,
        )?;
        decoder.recorded_sdr_colors()?;
        Ok(decoder)
    }
    pub fn style(&self) -> WebcamStyle {
        self.metadata.style
    }
    pub fn frame_at(
        &mut self,
        at_ms: u64,
        cancel: &CancellationToken,
    ) -> Result<Option<Arc<snow_webcam::Frame>>> {
        if !self.metadata.visible_at(at_ms) || at_ms >= self.metadata.duration_ms {
            return Ok(None);
        }
        let slot = at_ms
            .saturating_add(1)
            .saturating_mul(u64::from(self.metadata.fps))
            .saturating_sub(1)
            / 1000;
        if self.last == Some(slot) {
            return Ok(self.cached.clone());
        }
        let rewind = self.last.is_some_and(|last| slot < last);
        if rewind {
            self.video = Self::decoder(
                &self.bundle,
                &self.directory.path().join("webcam.mp4"),
                self.index_range,
                &self.metadata,
                cancel,
            )?;
        }
        if rewind || self.last.is_none() {
            self.video.seek_to(slot, cancel)?;
        }
        let changed = self.video.advance(slot, cancel)?;
        self.last = Some(slot);
        if !changed && self.cached.is_some() {
            return Ok(self.cached.clone());
        }
        let side = self.metadata.style.size;
        let mut pixels = vec![0; (side * side * 4) as usize];
        self.video.copy_rgba(&mut pixels)?;
        self.cached = Some(Arc::new(snow_webcam::Frame {
            revision: slot + 1,
            at: Instant::now(),
            size: side,
            rgba: Arc::new(pixels),
        }));
        Ok(self.cached.clone())
    }
}
fn invalid(value: impl ToString) -> RecordingExportError {
    RecordingExportError::Decode(value.to_string())
}
