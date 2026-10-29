# Source, working-surface and export limits

Source ownership, GPU working surfaces, export dimensions and encoded files have
separate limits. A large source or output does not require a full-size GPU texture.

| Resource | Limit and behavior |
| --- | --- |
| Immutable source | Up to 8 GiB RGBA, width at most `INT_MAX/4`, height at most `INT_MAX`. Leaf buffers remain shared; GPU uploads use retained pages. |
| Decode | PNG/JPEG/WebP default to 512 MiB decoded RGBA; callers may set an explicit budget up to 8 GiB. Encoded input is limited to 512 MiB. Dimensions and backing bytes are validated before decoding. |
| Layered sources | At most 128 layers, 1 GiB input pixels and a 512 MiB virtual composite. Placement, coverage and donor eligibility remain metadata during editing. |
| GPU working surface | At most 16 Mi pixels and 16,384 pixels per dimension, including effect support. |
| GPU memory | Default aggregate canvas budget is 512 MiB per device generation, including transient surfaces, staging and pending retirement. |
| PNG and packed PNG | Up to 8 GiB decoded output RGBA, an 8 MiB scanline and dimensions within the format limits. Canvas bounds remain within ±1e9; scale is 0.1–8. |
| Export bands | At most 8 MiB and 128 rows; render segments are at most 1024 output pixels wide plus effect support. Two reusable staging handles and two queued CPU bands bound the pipeline. |
| Effect support | At most 8192 physical pixels, also bounded by the working-surface and GPU memory limits. Dependent effect passes accumulate their sampling halos. |
| SVG | Ordinary content retains paths and outlined text. Effects, rough styles, color glyphs and custom presentation embed the lossless GPU-composited result. |
| Encoded buffers | At most 96 MiB. Writer and path exports stream instead of retaining the encoded result. |
| Drawing files | At most 512 MiB for the schema-1 JSON envelope and PNG assets. Default per-asset save/load budgets are 512 MiB; explicit paired limits may reach 8 GiB. |
| History assets | At most 1 GiB and 128 distinct image assets, deduplicated by source identity. |
| Smart Erase | A 2 Mi-pixel eligible donor ROI, conservative 512 MiB scratch estimate and 128 MiB cropped-result cache. The worker never reads displayed GPU pixels. |

`CanvasSnapshot::with_region` captures the requested output camera without an
output-sized engine viewport. Its aspect ratio must agree with the canvas region
to within one output pixel. Anisotropic source-layer placement is independent.
Dimension rounding uses the ceiling of integer extent multiplied by the stored
f32 scale in f64 arithmetic. GPU tests compare fractional output grids and shifted
tile boundaries, including accumulated filter halos.

`GpuSavePreparation` materializes visual and eligible donor composites only when
schema-1 persistence requires them. Smart Erase assembles eligible regions on the
CPU; ordinary editing never materializes these full composites.

Source buffers are contiguous allocations, not an out-of-core image provider.
Limits are resource guards, not guarantees that every permitted allocation fits a
particular machine. See [GPU_BACKEND_VALIDATION.md](GPU_BACKEND_VALIDATION.md) for
native test coverage and streaming-export measurements.
