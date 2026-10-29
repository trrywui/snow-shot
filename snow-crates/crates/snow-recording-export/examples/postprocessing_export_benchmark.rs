//! Portable deterministic deferred-export benchmark. Release with `bench-timing` only.
//! Prepare outside measurement: `prepare DIR WIDTH HEIGHT static|moving|cursor|stationary|hdr`.
//! Measure in a fresh process: `render DIR OUTPUT none|bar|time|trail|combined 0|1 THREADS [SPEED]`.
//! Three seconds at 60 FPS; no capture, audio, webcam, or desktop permissions.
use std::{fs, path::Path, time::Instant};

use anyhow::{Context, Result, bail};
use snow_core::cancellation::CancellationToken;
use snow_recording_effects::keyboard_overlay::KeyEvent;
use snow_recording_export::{
    ExportExecutionMode, ExportFormat, SoftwareH264Priority, StreamingEncoder,
    StreamingEncoderConfig, VideoCodec, deferred::render_bundle_range_with_speed,
    read_deferred_output_settings, write_deferred_output_settings,
};
use snow_recording_model::{
    BundleAssetKind, EffectsConfig, FinalizedTimeline, InputMouseButton, InputStoreWriter,
    IntermediateRecordingProfile, KeyboardOverlayConfig, MouseStore, PlaybackOverlay,
    PlaybackSpeed, RecordedInput, RecordedInputEvent, RecordedMouseClick, RecordingBundleAsset,
    RenderConfig, RenderMetadata, SessionManifest, TrimRange, VideoEncodeConfig,
    VideoEncodingSpeed, read_bundle_render_metadata, write_mouse_records, write_recording_bundle,
    write_render_metadata,
};

const FPS: u32 = 60;
const FRAMES: u64 = 180;
const DURATION_MS: u64 = 3000;

fn style() -> KeyboardOverlayConfig {
    KeyboardOverlayConfig {
        font: None,
        keycap_size: 64,
        background_rgba: [31, 31, 31, 204],
        text_rgba: [255, 255, 255, 217],
        border_rgba: [66, 66, 66, 100],
        labels: Default::default(),
    }
}

fn prepare(root: &Path, width: u32, height: u32, scene: &str) -> Result<()> {
    let hdr = scene == "hdr";
    let (moving, cursor, stationary) = match scene {
        "static" => (false, false, false),
        "moving" => (true, false, false),
        "cursor" => (false, true, false),
        "stationary" => (false, false, true),
        "hdr" => (false, false, true),
        _ => bail!("source must be static, moving, cursor, stationary, or hdr"),
    };
    if width < 256 || height < 256 || width > 4096 || height > 4096 {
        bail!("fixture dimensions must be in 256..=4096");
    }
    if root.exists() {
        bail!("fixture directory already exists");
    }
    fs::create_dir_all(root)?;
    let video = root.join("source.mp4");
    let config = StreamingEncoderConfig {
        output_path: root.join("unused.mp4"),
        format: ExportFormat::Mp4,
        width,
        height,
        fps: FPS,
        codec: VideoCodec::H264,
        prefer_hardware_h264: false,
        execution_mode: ExportExecutionMode::SoftwareOnly,
        software_h264_priority: SoftwareH264Priority::X264First,
        video: VideoEncodeConfig {
            quality: 80,
            speed: VideoEncodingSpeed::VeryFast,
        },
        encode_threads: 1,
        loop_animated_images: false,
        audio: Vec::new(),
    };
    let mut source_config = config.clone();
    source_config.output_path = video.clone();
    if hdr {
        source_config.codec = VideoCodec::H265;
    }
    let mut builder = StreamingEncoder::builder(source_config).recording_source();
    if hdr {
        builder = builder.hdr10_cpu_input();
    }
    let mut encoder = builder.create()?;
    let mut pixels = vec![0; width as usize * height as usize * 4];
    let mut index_bytes = snow_recording_model::video_index::VIDEO_INDEX_MAGIC.to_vec();
    let count = if moving { FRAMES } else { 1 };
    for index in 0..count {
        for (y, row) in pixels.chunks_exact_mut(width as usize * 4).enumerate() {
            for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
                let tile = ((x / 48 + y / 48) % 2) as u8 * 24;
                let active = moving
                    && ((x as u64 + index * 13) % u64::from(width)) < 180
                    && (200..700).contains(&y);
                let color = if active {
                    [220, 110, 40, 255]
                } else {
                    [tile + 40, tile + 70, tile + 100, 255]
                };
                pixel.copy_from_slice(&color);
            }
        }
        if hdr {
            let format = encoder.input_pixel_format();
            let mut frame = ffmpeg_next::frame::Video::new(format, width, height);
            for plane in 0..frame.planes() {
                let value = if plane == 0 { 600u16 } else { 512u16 };
                let value = if format == ffmpeg_next::format::Pixel::P010LE {
                    value << 6
                } else {
                    value
                };
                for sample in frame.data_mut(plane).chunks_exact_mut(2) {
                    sample.copy_from_slice(&value.to_le_bytes());
                }
            }
            encoder.push_prepared_video_frame_at_pts(index, frame)?;
        } else {
            pixels = encoder.push_owned_rgba_frame_at_pts(index, pixels)?;
        }
        pixels.resize(width as usize * height as usize * 4, 0);
        let at = index * 1000 / u64::from(FPS);
        let next = if index + 1 == count {
            DURATION_MS
        } else {
            (index + 1) * 1000 / u64::from(FPS)
        };
        index_bytes.extend(index.to_le_bytes());
        index_bytes.extend(at.to_le_bytes());
        index_bytes.extend(((next - at) as u32).to_le_bytes());
    }
    encoder.finish_at_duration_ms(DURATION_MS)?;
    let input = root.join("input.bin");
    let mut writer = InputStoreWriter::new(&input)?;
    let mut sequence = 0;
    if cursor {
        writer.push(&RecordedInputEvent {
            timestamp_ms: 0,
            sequence,
            event: RecordedInput::CursorShape(snow_recording_model::CursorShapeRecord {
                shape_id: 1,
                hotspot_x: 0,
                hotspot_y: 0,
                width: 18,
                height: 18,
                mode: snow_recording_model::CursorShapeCompositionMode::AlphaBlend,
                shape_rgba: [240, 220, 20, 255].repeat(18 * 18),
            }),
        })?;
        sequence += 1;
    }
    for index in 0..FRAMES {
        let at = index * 1000 / u64::from(FPS);
        let position = if stationary {
            (100, 100)
        } else {
            (
                (100 + index * 9 % u64::from(width - 200)) as i32,
                (100 + index * 7 % u64::from(height - 200)) as i32,
            )
        };
        let mut events = vec![RecordedInput::Pointer {
            position: Some(position),
            continuity: 0,
            at_ms: at,
        }];
        if cursor {
            events.push(RecordedInput::Cursor(
                snow_recording_model::CursorFrameRecord {
                    timestamp_ms: at,
                    x: 80,
                    y: 80,
                    visible: true,
                    shape_id: Some(1),
                },
            ));
        }
        if index.is_multiple_of(15) {
            let key = KeyEvent {
                at_ms: at,
                key: 65,
                down: true,
                label: "A".into(),
                modifiers: vec![(0x11, "Ctrl".into())],
            };
            events.push(RecordedInput::Click(RecordedMouseClick {
                x: position.0,
                y: position.1,
                button: InputMouseButton::Left,
                down: true,
                key: KeyEvent {
                    key: 0x200,
                    label: "Left click".into(),
                    ..key.clone()
                },
            }));
            events.push(RecordedInput::Key(key.clone()));
            events.push(RecordedInput::Key(KeyEvent { down: false, ..key }));
        }
        for event in events {
            writer.push(&RecordedInputEvent {
                timestamp_ms: at,
                sequence,
                event,
            })?;
            sequence += 1;
        }
    }
    writer.finish()?;
    let metadata = RenderMetadata {
        render: RenderConfig {
            output_width: width,
            output_height: height,
            output_fps: FPS,
            effects: EffectsConfig {
                show_cursor: cursor,
                ..Default::default()
            },
            playback_overlay: PlaybackOverlay::None,
        },
        timeline: FinalizedTimeline::new(DURATION_MS, FPS).map_err(anyhow::Error::msg)?,
        coded_width: width,
        coded_height: height,
    };
    let render = root.join("render.bin");
    let settings = root.join("settings.bin");
    let mouse = root.join("mouse.bin");
    let index = root.join("index.bin");
    write_render_metadata(&render, &metadata)?;
    write_deferred_output_settings(&settings, &config)?;
    write_mouse_records(&mouse, &MouseStore::new())?;
    fs::write(&index, index_bytes)?;
    let manifest = SessionManifest {
        media: snow_recording_model::media::RecordedMedia::new(
            if hdr {
                snow_media::ColorDescription::HDR10
            } else {
                snow_media::ColorDescription::SRGB
            },
            snow_media::CursorMode::Separate,
            Vec::new(),
        ),
        video_codec: if hdr {
            VideoCodec::H265
        } else {
            VideoCodec::H264
        },
        session_id: "postprocessing-benchmark".into(),
        output_dir: root.to_path_buf(),
        keep_temp_files: false,
        fps: FPS,
        intermediate_profile: IntermediateRecordingProfile::EditFast,
        recording_video: config.video,
        width,
        height,
        capture_origin_x: 0,
        capture_origin_y: 0,
        audio_tracks: Vec::new(),
        pause_intervals: Vec::new(),
    };
    let assets: Vec<_> = [
        (BundleAssetKind::VideoIndex, &index),
        (BundleAssetKind::MouseStore, &mouse),
        (BundleAssetKind::InputEvents, &input),
        (BundleAssetKind::RenderMetadata, &render),
        (BundleAssetKind::OutputSettings, &settings),
    ]
    .into_iter()
    .map(|(kind, path)| RecordingBundleAsset {
        kind,
        asset_id: None,
        path,
    })
    .collect();
    write_recording_bundle(&video, &manifest, &assets)?;
    fs::rename(video, root.join("source.snowrec"))?;
    Ok(())
}

fn render(
    root: &Path,
    output: &Path,
    case: &str,
    hardware: bool,
    threads: u8,
    speed: PlaybackSpeed,
) -> Result<()> {
    let bundle = root.join("source.snowrec");
    let mut config = read_deferred_output_settings(&bundle)?;
    config.output_path = output.to_path_buf();
    config.prefer_hardware_h264 = hardware;
    config.execution_mode = if hardware {
        ExportExecutionMode::HardwarePreferred
    } else {
        ExportExecutionMode::SoftwareOnly
    };
    config.encode_threads = threads;
    let mut metadata = read_bundle_render_metadata(&bundle)?.context("missing render metadata")?;
    metadata.render.playback_overlay = match case {
        "none" | "trail" => PlaybackOverlay::None,
        "bar" | "combined" => PlaybackOverlay::ProgressBar {
            rgba: [22, 119, 255, 255],
        },
        "time" => PlaybackOverlay::PlaybackTime { rgba: [255; 4] },
        _ => bail!("case must be none, bar, time, trail, or combined"),
    };
    if case == "trail" {
        metadata.render.effects.mouse_trail_rgba = [255, 40, 60, 180];
    }
    if case == "combined" {
        metadata.render.effects.show_keyboard = true;
        metadata.render.effects.keyboard = Some(style());
        metadata.render.effects.mouse_trail_rgba = [255, 40, 60, 180];
        metadata.render.effects.mouse_click_rgba = [40, 180, 255, 160];
    }
    let started = Instant::now();
    let result = render_bundle_range_with_speed(
        &bundle,
        config.clone(),
        metadata,
        TrimRange::new(0, DURATION_MS * 1000, DURATION_MS * 1000).map_err(anyhow::Error::msg)?,
        speed,
        &CancellationToken::default(),
        |_, _, _, _| {},
    )?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let input = ffmpeg_next::format::input(output)?;
    let stream = input
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .context("missing output video")?;
    let duration_ms = stream.duration() as f64 * f64::from(stream.time_base()) * 1000.0;
    anyhow::ensure!(
        (duration_ms - DURATION_MS as f64 * 100.0 / f64::from(speed.percent())).abs() <= 1.0,
        "output duration changed: {duration_ms}"
    );
    let decoder = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;
    anyhow::ensure!(
        (decoder.width(), decoder.height()) == (config.width, config.height),
        "output dimensions changed"
    );
    let stages = result.runtime_report.stage_durations_ms;
    println!(
        "width,height,case,hardware_requested,hardware_used,threads,encoder,elapsed_ms,slots_per_second,plan_ms,decode_ms,compose_ms,encode_ms,finalize_ms,output_bytes,duration_ms,speed_percent"
    );
    println!(
        "{},{},{case},{hardware},{},{threads},{},{elapsed_ms:.3},{:.3},{},{},{},{},{},{},{duration_ms:.3},{}",
        config.width,
        config.height,
        result.runtime_report.used_hardware_encode,
        result.runtime_report.video_encoder.unwrap_or_default(),
        speed
            .output_frame_count(DURATION_MS * 1000, FPS)
            .map_err(anyhow::Error::msg)? as f64
            * 1000.0
            / elapsed_ms,
        stages.plan,
        stages.decode,
        stages.compose,
        stages.video_encode,
        stages.finalize,
        fs::metadata(output)?.len(),
        speed.percent()
    );
    Ok(())
}

fn main() -> Result<()> {
    if cfg!(debug_assertions) || !cfg!(feature = "bench-timing") {
        bail!("use the performance preset's Release environment and --features bench-timing");
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("prepare") if args.len() == 5 => prepare(
            Path::new(&args[1]),
            args[2].parse()?,
            args[3].parse()?,
            &args[4],
        ),
        Some("render") if matches!(args.len(), 6 | 7) => {
            let hardware = match args[4].as_str() {
                "0" => false,
                "1" => true,
                _ => bail!("hardware must be 0 or 1"),
            };
            render(
                Path::new(&args[1]),
                Path::new(&args[2]),
                &args[3],
                hardware,
                args[5].parse()?,
                PlaybackSpeed::from_percent(args.get(6).map_or(Ok(100), |value| value.parse())?)
                    .map_err(anyhow::Error::msg)?,
            )
        }
        _ => bail!(
            "usage: prepare DIR WIDTH HEIGHT static|moving|cursor|stationary|hdr; render DIR OUTPUT none|bar|time|trail|combined 0|1 THREADS [SPEED_PERCENT]"
        ),
    }
}
