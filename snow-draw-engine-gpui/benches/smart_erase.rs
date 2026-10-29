//! Source-grid CPU/GPU reconstruction and eligible donor assembly measurements.
//! Run only in the optimized canvas-perf profile; no display composition is timed.
use anyhow::{Context, Result, ensure};
use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use snow_draw_engine_gpui::{
    CanvasRect, ImageSourceLayer, SmartEraseProfile, SmartEraseService, SmartEraseSolver,
    SourceImage,
    engine::{DisplayFilterType, FilterDisplayItem, FilterRenderSpec},
};
#[cfg(target_os = "windows")]
use snow_draw_engine_gpui::{
    GpuDonorAssembler, GpuDonorInput, GpuSmartErase, GpuSmartEraseInput, GpuSourceMask,
};

struct Case {
    name: &'static str,
    source: SourceImage,
    filter: FilterDisplayItem,
    expected_solver: SmartEraseSolver,
}

fn texture(x: u32, y: u32) -> u8 {
    let mut value = x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663);
    value ^= value >> 13;
    (value % 49) as u8
}

fn case(name: &'static str, size: [u32; 2], mask: [u32; 4]) -> Result<Case> {
    let [left, top, width, height] = mask;
    let source = SourceImage::new(RgbaImage::from_fn(size[0], size[1], |x, y| {
        let n = texture(x, y);
        let pixel = match name {
            "affine_small" | "affine_large_source" | "layered" => [70, 120, 180, 255],
            "periodic" => {
                let n = ((x / 8 + y / 8) % 2 * 24) as u8;
                [140 + n, 160 + n, 180 + n, 255]
            }
            "alpha_gaps" | "layered_alpha" if x % 79 < 3 => [255, 0, 255, 0],
            _ => [90 + n, 120 + n, 60 + n, 255],
        };
        Rgba(
            if name.starts_with("affine")
                && (left..left + width).contains(&x)
                && (top..top + height).contains(&y)
            {
                [240, 5, 9, 255]
            } else {
                pixel
            },
        )
    }))?;
    Ok(Case {
        name,
        source,
        filter: FilterDisplayItem {
            center_x: f64::from(left) + f64::from(width) / 2.0,
            center_y: f64::from(top) + f64::from(height) / 2.0,
            width: f64::from(width),
            height: f64::from(height),
            opacity: 1.0,
            filter: FilterRenderSpec::resolve(DisplayFilterType::SmartErase, 0.5),
            ..Default::default()
        },
        expected_solver: match name {
            "affine_small" | "affine_large_source" | "layered" => SmartEraseSolver::Affine,
            "periodic" => SmartEraseSolver::Periodic,
            _ => SmartEraseSolver::Multiscale,
        },
    })
}

fn cases() -> Result<Vec<Case>> {
    let mut cases = vec![
        case("affine_small", [512, 384], [240, 176, 32, 32])?,
        case("affine_large_source", [1920, 1080], [944, 524, 32, 32])?,
        case("periodic", [256, 192], [96, 64, 64, 48])?,
        case("texture_small", [256, 192], [96, 64, 64, 48])?,
        case("texture_medium", [512, 384], [192, 144, 128, 96])?,
        case("alpha_gaps", [256, 192], [96, 64, 64, 48])?,
    ];
    let mut brush = case("brush", [256, 192], [64, 64, 128, 64])?;
    brush.filter.is_pen_filter = true;
    brush.filter.stroke_width = 18.0;
    brush.filter.points = (0..32)
        .map(|i| [64.0 + f64::from(i) * 4.0, 80.0 + f64::from(i % 5) * 4.0])
        .collect::<Vec<_>>()
        .into();
    cases.push(brush);
    for name in ["layered", "layered_texture", "layered_alpha"] {
        let mut layered = case(name, [768, 512], [352, 232, 64, 48])?;
        let mut layers = vec![ImageSourceLayer::new(layered.source)];
        if name != "layered" {
            layers.push(ImageSourceLayer {
                image: SourceImage::new(RgbaImage::from_fn(385, 257, |x, y| {
                    let n = texture(x + 11, y + 7);
                    Rgba([
                        70 + n,
                        110 + n,
                        150 + n,
                        if name == "layered_alpha" && x % 79 < 3 {
                            0
                        } else {
                            128
                        },
                    ])
                }))?,
                canvas_rect: Some(CanvasRect::new(0.25, 0.5, 767.5, 511.25)?),
                coverage: Some(CanvasRect::new(13.75, 17.25, 731.5, 473.25)?),
                smart_erase_source: true,
            });
        }
        layers.push(ImageSourceLayer {
            image: SourceImage::new(RgbaImage::from_pixel(384, 256, Rgba([2, 250, 5, 255])))?,
            canvas_rect: Some(CanvasRect::new(0.0, 0.0, 768.0, 512.0)?),
            coverage: Some(CanvasRect::new(100.0, 100.0, 500.0, 300.0)?),
            smart_erase_source: false,
        });
        layered.source = SourceImage::from_layers(&layers)?.context("layered source")?;
        cases.push(layered);
    }
    Ok(cases)
}

fn run(case: Case, count: usize) -> Result<Value> {
    let cancel = Default::default();
    let mut samples = Vec::new();
    let mut cached = Vec::new();
    let mut patch_bytes = 0;
    // One warmup uses the same code and fresh cache as every measured solve.
    for sample in 0..=count {
        let mut service = SmartEraseService::default();
        let mut profile = SmartEraseProfile::default();
        let patch = service
            .solve_profiled(&case.filter, &case.source, &cancel, &mut profile)?
            .context("expected an intersecting patch")?;
        ensure!(
            !profile.cache_hit,
            "solve sample hit the reconstruction cache"
        );
        ensure!(
            profile.solver == Some(case.expected_solver),
            "Expected {:?}, exercised {:?}",
            case.expected_solver,
            profile.solver
        );
        patch_bytes = patch.retained_bytes();
        let mut cache_profile = SmartEraseProfile::default();
        let hit = service
            .solve_profiled(&case.filter, &case.source, &cancel, &mut cache_profile)?
            .context("cached patch")?;
        ensure!(cache_profile.cache_hit && std::sync::Arc::ptr_eq(&patch, &hit));
        if sample != 0 {
            samples.push(profile);
            cached.push(cache_profile.total_ms);
        }
    }
    let mut elapsed: Vec<_> = samples.iter().map(|p| p.total_ms).collect();
    elapsed.sort_by(f64::total_cmp);
    Ok(json!({
        "workload": case.name,
        "source_size": case.source.dimensions(),
        "patch_bytes": patch_bytes,
        "solve_p50_ms": elapsed[elapsed.len() / 2],
        "solve_p95_ms": elapsed[(elapsed.len() * 95).div_ceil(100) - 1],
        "cache_hit_ms": cached,
        "samples": samples,
    }))
}

#[cfg(target_os = "windows")]
fn run_gpu(case: Case, count: usize, runtime: &GpuSmartErase) -> Result<Value> {
    use std::{
        task::Poll,
        time::{Duration, Instant},
    };
    let mut reference = SmartEraseProfile::default();
    SmartEraseService::default().solve_profiled(
        &case.filter,
        &case.source,
        &Default::default(),
        &mut reference,
    )?;
    ensure!(
        reference.solver == Some(SmartEraseSolver::Multiscale),
        "GPU workload did not require patch synthesis"
    );
    let [x, y, w, h] = reference.source_region;
    let mut samples = Vec::new();
    let mut cold = None;
    for sample in 0..=count {
        let start = Instant::now();
        let pixels = case
            .source
            .raster_pixels()
            .context("GPU benchmark CPU donor fixture")?;
        let cropped =
            SourceImage::new(image::imageops::crop_imm(&**pixels, x, y, w, h).to_image())?
                .with_placement(
                    snow_draw_engine_gpui::engine::Point::new(x as f64, y as f64),
                    1.0,
                )?;
        let input = GpuSmartEraseInput::from_filter(&cropped, &case.filter, &Default::default())?
            .with_search_span(case.source.width().max(case.source.height()))?;
        let prepare_ms = start.elapsed().as_secs_f64() * 1000.0;
        let submit = Instant::now();
        let mut job = runtime.start(input, Default::default())?;
        let owner_start_ms = submit.elapsed().as_secs_f64() * 1000.0;
        let mut max_poll_ms = 0.0_f64;
        let deadline = Instant::now() + Duration::from_secs(60);
        let output = loop {
            let poll_start = Instant::now();
            let result = job.poll();
            max_poll_ms = max_poll_ms.max(poll_start.elapsed().as_secs_f64() * 1000.0);
            if let Poll::Ready(result) = result {
                break result?;
            }
            ensure!(Instant::now() < deadline, "GPU solve timed out");
            std::thread::yield_now();
        };
        let result = json!({"input_preparation_ms":prepare_ms,"owner_start_ms":owner_start_ms,
            "max_owner_poll_ms":max_poll_ms,"submit_through_completion_ms":submit.elapsed().as_secs_f64()*1000.0,
            "end_to_end_ms":start.elapsed().as_secs_f64()*1000.0,"gpu":output.profile});
        if sample == 0 {
            cold = Some(result);
        } else {
            samples.push(result);
        }
    }
    let mut times: Vec<_> = samples
        .iter()
        .filter_map(|s| s["end_to_end_ms"].as_f64())
        .collect();
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"workload":case.name,"source_size":case.source.dimensions(),"source_region":reference.source_region,
        "cpu_reference":reference,"cold":cold,"solve_p50_ms":times[times.len()/2],
        "solve_p95_ms":times[(times.len()*95).div_ceil(100)-1],"samples":samples}),
    )
}

#[cfg(target_os = "windows")]
fn run_donors(case: Case, count: usize, assembler: &GpuDonorAssembler) -> Result<Value> {
    use std::{
        task::Poll,
        time::{Duration, Instant},
    };
    let mut reference = SmartEraseProfile::default();
    SmartEraseService::default().solve_profiled(
        &case.filter,
        &case.source,
        &Default::default(),
        &mut reference,
    )?;
    ensure!(
        reference.solver == Some(case.expected_solver),
        "CPU reference took an unexpected branch"
    );
    let mut samples = Vec::new();
    let mut cold = None;
    for sample in 0..=count {
        let started = Instant::now();
        let input = GpuDonorInput::new(&case.source, reference.source_region, &Default::default())?;
        let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
        let submit = Instant::now();
        let mut job = assembler.start(input, Default::default())?;
        let start_ms = submit.elapsed().as_secs_f64() * 1000.0;
        let mut max_poll_ms = 0.0_f64;
        let deadline = Instant::now() + Duration::from_secs(30);
        let output = loop {
            let poll = Instant::now();
            let result = job.poll();
            max_poll_ms = max_poll_ms.max(poll.elapsed().as_secs_f64() * 1000.0);
            if let Poll::Ready(result) = result {
                break result?;
            }
            ensure!(Instant::now() < deadline, "GPU donor assembly timed out");
            std::thread::yield_now();
        };
        ensure!(
            output.source.raster_pixels().is_none(),
            "donors were materialized on CPU"
        );
        let data = json!({"input_preparation_ms":prepare_ms,"owner_start_ms":start_ms,
            "max_owner_poll_ms":max_poll_ms,"end_to_end_ms":started.elapsed().as_secs_f64()*1000.0,
            "gpu":output.profile});
        if sample == 0 {
            cold = Some(data);
        } else {
            ensure!(
                output.profile.page_upload_bytes == 0,
                "resident donor pages uploaded again"
            );
            samples.push(data);
        }
    }
    let mut elapsed: Vec<_> = samples
        .iter()
        .filter_map(|s| s["end_to_end_ms"].as_f64())
        .collect();
    elapsed.sort_by(f64::total_cmp);
    Ok(
        json!({"workload":case.name,"source_region":reference.source_region,
        "cpu_reference_assembly_ms":reference.stages.donor_assembly_ms,
        "cpu_reference":reference,"cold":cold,"assembly_p50_ms":elapsed[elapsed.len()/2],
        "assembly_p95_ms":elapsed[(elapsed.len()*95).div_ceil(100)-1],"samples":samples}),
    )
}

#[cfg(target_os = "windows")]
fn wait_gpu<T>(
    mut poll: impl FnMut() -> std::task::Poll<Result<T, snow_draw_engine_gpui::CanvasError>>,
) -> Result<(T, f64)> {
    use std::{
        task::Poll,
        time::{Duration, Instant},
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut max = 0.0_f64;
    loop {
        let started = Instant::now();
        let result = poll();
        max = max.max(started.elapsed().as_secs_f64() * 1000.0);
        if let Poll::Ready(result) = result {
            return Ok((result?, max));
        }
        ensure!(Instant::now() < deadline, "GPU pipeline timed out");
        std::thread::yield_now();
    }
}
#[cfg(target_os = "windows")]
fn run_pipeline(
    case: Case,
    count: usize,
    runtime: &GpuSmartErase,
    assembler: &GpuDonorAssembler,
) -> Result<Value> {
    use std::{task::Poll, time::Instant};
    let mut reference = SmartEraseProfile::default();
    SmartEraseService::default().solve_profiled(
        &case.filter,
        &case.source,
        &Default::default(),
        &mut reference,
    )?;
    ensure!(
        reference.solver == Some(case.expected_solver),
        "CPU reference took an unexpected branch"
    );
    ensure!(
        !case.filter.is_pen_filter && case.filter.rotation == 0.0,
        "Pipeline fixture must be an axis-aligned rectangle"
    );
    let [x, y, w, h] = reference.source_region;
    let units = case.source.units_per_pixel();
    let mask: Vec<u8> = (0..w * h)
        .map(|i| {
            let wx = case.source.origin().x + (x as f64 + (i % w) as f64 + 0.5) * units;
            let wy = case.source.origin().y + (y as f64 + (i / w) as f64 + 0.5) * units;
            u8::from(
                wx >= case.filter.center_x - case.filter.width / 2.0
                    && wx < case.filter.center_x + case.filter.width / 2.0
                    && wy >= case.filter.center_y - case.filter.height / 2.0
                    && wy < case.filter.center_y + case.filter.height / 2.0,
            )
        })
        .collect();
    let mut cold = None;
    let mut samples = Vec::new();
    for sample in 0..=count {
        let started = Instant::now();
        let input = GpuDonorInput::new(&case.source, reference.source_region, &Default::default())?;
        let mask = GpuSourceMask::new(&mask, [w, h], &Default::default())?;
        let input_ms = started.elapsed().as_secs_f64() * 1000.0;
        let begin = Instant::now();
        let mut donors = assembler.start(input, Default::default())?;
        let donor_start_ms = begin.elapsed().as_secs_f64() * 1000.0;
        let mut max_poll = 0.0_f64;
        while donors.has_pending_submissions() {
            let tick = Instant::now();
            let result = donors.poll();
            max_poll = max_poll.max(tick.elapsed().as_secs_f64() * 1000.0);
            match result {
                Poll::Ready(result) => {
                    result?;
                    anyhow::bail!("assembly finished before dependency handoff")
                }
                Poll::Pending => {}
            }
        }
        let begin = Instant::now();
        let mut prepare = runtime.prepare_assembled(&mut donors, mask, Default::default())?;
        let prepare_start_ms = begin.elapsed().as_secs_f64() * 1000.0;
        let (prepared, max) = wait_gpu(|| prepare.poll())?;
        max_poll = max_poll.max(max);
        ensure!(
            prepared.masked_pixels as usize == reference.masked_pixels,
            "GPU pipeline mask differs from CPU fixture"
        );
        ensure!(
            prepared.solver() == case.expected_solver,
            "GPU pipeline took an unexpected branch"
        );
        let prepare_profile = prepared.profile.clone();
        let (donors, max) = wait_gpu(|| donors.poll())?;
        max_poll = max_poll.max(max);
        let begin = Instant::now();
        let mut solve = runtime.start_prepared(
            prepared.with_search_span(case.source.width().max(case.source.height()))?,
            Default::default(),
        )?;
        let solve_start_ms = begin.elapsed().as_secs_f64() * 1000.0;
        let (result, max) = wait_gpu(|| solve.poll())?;
        max_poll = max_poll.max(max);
        ensure!(
            result.profile.source_upload_bytes == 0 && result.profile.control_readback_bytes == 0,
            "Resident reconstruction repeated an input transfer or metadata wait"
        );
        let record = json!({"input_preparation_ms":input_ms,"donor_owner_start_ms":donor_start_ms,
            "preparation_owner_start_ms":prepare_start_ms,"solve_owner_start_ms":solve_start_ms,
            "max_owner_poll_ms":max_poll,"end_to_end_ms":started.elapsed().as_secs_f64()*1000.0,
            "assembly":donors.profile,"preparation":prepare_profile,"solve":result.profile});
        if sample == 0 {
            cold = Some(record);
        } else {
            ensure!(
                donors.profile.page_upload_bytes == 0,
                "Resident pages uploaded again"
            );
            samples.push(record);
        }
    }
    let mut times: Vec<_> = samples
        .iter()
        .filter_map(|s| s["end_to_end_ms"].as_f64())
        .collect();
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"workload":case.name,"source_region":reference.source_region,"cpu_reference":reference,"cold":cold,
        "mask_fixture":"Fixed rectangle stencil; stencil construction and worker fast-model classification excluded",
        "pipeline_p50_ms":times[times.len()/2],"pipeline_p95_ms":times[(times.len()*95).div_ceil(100)-1],"samples":samples}),
    )
}

fn main() -> Result<()> {
    ensure!(
        !cfg!(debug_assertions),
        "Use the optimized canvas-perf profile"
    );
    let mut arguments = std::env::args().skip(1);
    let destination = arguments.next().context("output JSON path")?;
    let selected = arguments.next().unwrap_or_else(|| "all".into());
    let count: usize = arguments.next().as_deref().unwrap_or("5").parse()?;
    let backend = arguments.next().unwrap_or_else(|| "cpu".into());
    ensure!(
        matches!(backend.as_str(), "cpu" | "gpu" | "donors" | "pipeline"),
        "Backend must be cpu, gpu, donors or pipeline"
    );
    #[cfg(target_os = "windows")]
    let host = if backend != "cpu" {
        Some(gpui_windows::gpu_extensions::OffscreenGpuHost::new()?)
    } else {
        None
    };
    #[cfg(target_os = "windows")]
    let runtime = host
        .as_ref()
        .filter(|_| backend == "gpu" || backend == "pipeline")
        .map(|host| {
            let mut runtime = GpuSmartErase::new(host.service())?;
            runtime.set_profiling(true);
            Ok::<_, anyhow::Error>(runtime)
        })
        .transpose()?;
    #[cfg(target_os = "windows")]
    let donors = host
        .as_ref()
        .filter(|_| backend == "donors" || backend == "pipeline")
        .map(|host| {
            let mut assembler = GpuDonorAssembler::new(host.service())?;
            assembler.set_profiling(true);
            Ok::<_, anyhow::Error>(assembler)
        })
        .transpose()?;
    ensure!(
        (1..=100).contains(&count),
        "Sample count must be in 1..=100"
    );
    let mut results = Vec::new();
    for case in cases()? {
        if selected != "all" && !case.name.contains(&selected) {
            continue;
        }
        if backend == "gpu"
            && (case.expected_solver != SmartEraseSolver::Multiscale
                || case.source.layers().is_some())
        {
            continue;
        }
        if backend == "donors" && case.source.layers().is_none() {
            continue;
        }
        if backend == "pipeline" && case.filter.is_pen_filter {
            continue;
        }
        let name = case.name;
        let result = if backend == "cpu" {
            run(case, count)
        } else if backend == "gpu" {
            #[cfg(target_os = "windows")]
            {
                run_gpu(
                    case,
                    count,
                    runtime.as_ref().context("GPU runtime missing")?,
                )
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(anyhow::anyhow!("GPU benchmark requires Windows D3D11"))
            }
        } else if backend == "donors" {
            #[cfg(target_os = "windows")]
            {
                run_donors(
                    case,
                    count,
                    donors.as_ref().context("GPU donor assembler missing")?,
                )
            }
        } else {
            #[cfg(target_os = "windows")]
            {
                run_pipeline(
                    case,
                    count,
                    runtime.as_ref().context("GPU runtime missing")?,
                    donors.as_ref().context("GPU donor assembler missing")?,
                )
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(anyhow::anyhow!(
                    "GPU pipeline benchmark requires Windows D3D11"
                ))
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(anyhow::anyhow!(
                    "GPU donor benchmark requires Windows D3D11"
                ))
            }
        };
        eprintln!(
            "{name}: {}",
            if result.is_ok() { "complete" } else { "failed" }
        );
        results.push(match result {
            Ok(value) => value,
            Err(error) => json!({"workload":name,"error":format!("{error:#}")}),
        });
    }
    ensure!(!results.is_empty(), "No workload matched the filter");
    let failed = results.iter().any(|case| case.get("error").is_some());
    #[cfg(target_os = "windows")]
    let adapter = host.as_ref().map(|host| {
        let service = host.service();
        let capabilities = service.capabilities();
        json!({"name":capabilities.adapter_name.as_ref(),
            "vendor_id":capabilities.vendor_id,"device_id":capabilities.device_id})
    });
    #[cfg(not(target_os = "windows"))]
    let adapter = None::<Value>;
    std::fs::write(
        destination,
        serde_json::to_vec_pretty(&json!({
            "profile":"canvas-perf","backend":backend,
            "cpu":std::env::var("PROCESSOR_IDENTIFIER").ok(),
            "adapter":adapter,
            "measurement":match backend.as_str() {
                "donors"=>"Eligible source-grid assembly; one cold run then resident page reuse; input tables, allocation, submission and completion included; no image readback or display composition",
                "gpu"=>"Source-grid reconstruction; one warmup; GPU core includes CPU input preparation and owner submission; no UI timer or display composition",
                "pipeline"=>"Eligible donor assembly, resident domain/model/pyramid preparation and reconstruction; one cold run then warm pages; no CPU fast-model classifier, UI timer or display composition",
                _=>"Source-grid CPU reconstruction; one warmup; fresh result cache per sample; source fixture construction excluded",
            },
            "samples_per_workload":count,
            "cases":results
        }))?,
    )?;
    ensure!(
        !failed,
        "A Smart Erase workload failed; see the output JSON"
    );
    Ok(())
}
