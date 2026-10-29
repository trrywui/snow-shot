use super::*;

struct Guide {
    color: Vec<Color>,
    variation: Vec<f32>,
}
struct ResultLevel {
    image: Vec<Color>,
    matches: Vec<usize>,
    width: usize,
    height: usize,
}

// RGB is converted to CIE Lab for perceptual guide and patch costs.
fn lab(rgb: Color) -> Color {
    let v = rgb.map(|c| {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    let xyz = [
        (v[0] * 0.4124564 + v[1] * 0.3575761 + v[2] * 0.1804375) / 0.95047,
        v[0] * 0.2126729 + v[1] * 0.7151522 + v[2] * 0.072175,
        (v[0] * 0.0193339 + v[1] * 0.119192 + v[2] * 0.9503041) / 1.08883,
    ];
    let f = xyz.map(|t| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    });
    [
        116.0 * f[1] - 16.0,
        500.0 * (f[0] - f[1]),
        200.0 * (f[1] - f[2]),
    ]
}
fn gaussian(
    values: &[Color],
    level: &Level,
    radius: usize,
    cancel: &Cancellation,
) -> Result<Vec<Color>, CanvasError> {
    let sigma = radius as f32 / 2.0;
    let kernel: Vec<_> = (-(radius as isize)..=radius as isize)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let norm: f32 = kernel.iter().sum();
    let mut temp = vec![[0.0; 3]; values.len()];
    let mut out = temp.clone();
    for y in 0..level.height {
        cancel.check()?;
        for x in 0..level.width {
            let i = y * level.width + x;
            for (k, w) in kernel.iter().enumerate() {
                let xx = (x as isize + k as isize - radius as isize)
                    .clamp(0, level.width as isize - 1) as usize;
                for c in 0..3 {
                    temp[i][c] += values[y * level.width + xx][c] * w / norm;
                }
            }
        }
    }
    for y in 0..level.height {
        cancel.check()?;
        for x in 0..level.width {
            let i = y * level.width + x;
            for (k, w) in kernel.iter().enumerate() {
                let yy = (y as isize + k as isize - radius as isize)
                    .clamp(0, level.height as isize - 1) as usize;
                for c in 0..3 {
                    out[i][c] += temp[yy * level.width + x][c] * w / norm;
                }
            }
        }
    }
    Ok(out)
}
fn guides(level: &Level, colors: &[Color], cancel: &Cancellation) -> Result<Guide, CanvasError> {
    let holes = level.holes();
    let x0 = holes.iter().map(|i| i % level.width).min().unwrap();
    let x1 = holes.iter().map(|i| i % level.width).max().unwrap();
    let y0 = holes.iter().map(|i| i / level.width).min().unwrap();
    let y1 = holes.iter().map(|i| i / level.width).max().unwrap();
    let radius = ((x1 - x0 + 1).min(y1 - y0 + 1) / 12).clamp(4, 16);
    let weights: Vec<_> = (0..colors.len())
        .map(|i| if level.known(i) { [1.0; 3] } else { [0.0; 3] })
        .collect();
    let weights = gaussian(&weights, level, radius, cancel)?;
    let observed: Vec<_> = colors
        .iter()
        .enumerate()
        .map(|(i, c)| if level.known(i) { *c } else { [0.0; 3] })
        .collect();
    let mut mean = gaussian(&observed, level, radius, cancel)?;
    let squared: Vec<_> = observed
        .iter()
        .map(|c| [c.iter().map(|v| v * v).sum::<f32>(); 3])
        .collect();
    let squared = gaussian(&squared, level, radius, cancel)?;
    let mut variation = vec![0.0; colors.len()];
    for i in 0..mean.len() {
        for value in &mut mean[i] {
            *value /= weights[i][0].max(1e-6);
        }
        variation[i] = (squared[i][0] / weights[i][0].max(1e-6)
            - mean[i].iter().map(|c| c * c).sum::<f32>())
        .max(0.0)
        .sqrt();
    }
    let mut guide = Guide {
        color: mean.clone(),
        variation: variation.clone(),
    };
    let mut costs = vec![f32::MAX; colors.len()];
    for (dx, dy) in [(1_isize, 0_isize), (0, 1), (1, 1), (-1, 1)] {
        for y in 0..level.height {
            cancel.check()?;
            for x in 0..level.width {
                if level.at(x as isize - dx, y as isize - dy).is_some() {
                    continue;
                }
                let mut before = None;
                let mut pending = Vec::new();
                let mut xx = x as isize;
                let mut yy = y as isize;
                let flush = |before: Option<usize>,
                             after: Option<usize>,
                             pending: &[usize],
                             guide: &mut Guide,
                             costs: &mut [f32]| {
                    let (Some(a), Some(b)) = (before.or(after), after.or(before)) else {
                        return;
                    };
                    let difference = distance(mean[a], mean[b]);
                    let span = ((a % level.width).abs_diff(b % level.width).pow(2)
                        + (a / level.width).abs_diff(b / level.width).pow(2))
                        as f32;
                    let span = span.sqrt();
                    let cost = difference
                        + 0.02 * span
                        + if before.is_none() || after.is_none() {
                            1000.0
                        } else {
                            0.0
                        };
                    for i in pending {
                        if cost >= costs[*i] {
                            continue;
                        }
                        let d = ((i % level.width).abs_diff(a % level.width).pow(2)
                            + (i / level.width).abs_diff(a / level.width).pow(2))
                            as f32;
                        let t = if span > 0.0 { d.sqrt() / span } else { 0.0 };
                        guide.color[*i] = if difference < 900.0 {
                            mix(mean[a], mean[b], t)
                        } else if t < 0.5 {
                            mean[a]
                        } else {
                            mean[b]
                        };
                        guide.variation[*i] = variation[a] * (1.0 - t) + variation[b] * t;
                        costs[*i] = cost;
                    }
                };
                while let Some(i) = level.at(xx, yy) {
                    if level.known(i) {
                        flush(before, Some(i), &pending, &mut guide, &mut costs);
                        pending.clear();
                        before = Some(i);
                    } else if level.hole[i] {
                        pending.push(i);
                    } else {
                        flush(before, None, &pending, &mut guide, &mut costs);
                        pending.clear();
                        before = None;
                    }
                    xx += dx;
                    yy += dy;
                }
                flush(before, None, &pending, &mut guide, &mut costs);
            }
        }
    }
    Ok(guide)
}

pub(super) fn multiscale(
    level: Level,
    cancel: &Cancellation,
    profiler: &mut Profiler<'_>,
) -> Result<Vec<Color>, CanvasError> {
    let start = profiler.start();
    let mut levels = vec![level];
    // Conservative reduction: no coarse donor is allowed to include a hole or a
    // coverage gap. This is more expensive than resizing the corrupted source.
    while levels.len() < 6 {
        let fine = levels.last().unwrap();
        if fine.width.min(fine.height) < 40 {
            break;
        }
        let coarse = downsample(fine, cancel)?;
        if !coarse.hole.iter().any(|h| *h)
            || erode(&coarse.domain, coarse.width, coarse.height, 3)
                .iter()
                .filter(|d| **d)
                .count()
                < 16
        {
            break;
        }
        levels.push(coarse);
    }
    profiler.finish(Stage::Pyramid, start);
    let count = levels.len();
    let mut previous = None;
    for (n, level) in levels.iter().rev().enumerate() {
        cancel.check()?;
        let passes = if n == 0 {
            5
        } else if n + 1 == count {
            2
        } else {
            3
        };
        previous = Some(fill(level, previous.as_ref(), passes, cancel, profiler)?);
    }
    Ok(previous.unwrap().image)
}
struct Random(u32);
impl Random {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    fn pick(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }
}

fn fill(
    level: &Level,
    previous: Option<&ResultLevel>,
    passes: usize,
    cancel: &Cancellation,
    profiler: &mut Profiler<'_>,
) -> Result<ResultLevel, CanvasError> {
    let start = profiler.start();
    let colors: Vec<_> = level.source.iter().copied().map(lab).collect();
    profiler.finish(Stage::Lab, start);
    let start = profiler.start();
    let guide = guides(level, &colors, cancel)?;
    profiler.finish(Stage::Guides, start);
    let start = profiler.start();
    let holes = level.holes();
    let mut radius = 3;
    let mut donors = erode(&level.domain, level.width, level.height, radius);
    while !donors.iter().any(|d| *d) && radius > 0 {
        radius -= 1;
        donors = erode(&level.domain, level.width, level.height, radius);
    }
    let candidates: Vec<_> = donors
        .iter()
        .enumerate()
        .filter_map(|(i, d)| d.then_some(i))
        .collect();
    if candidates.is_empty() {
        return Err(CanvasError::Unsupported(
            "Smart Erase without complete source patches",
        ));
    }
    let nearest = nearest(level.width, level.height, &donors, cancel)?;
    if let Some(data) = &mut profiler.data {
        data.levels.push(profile::SmartEraseLevelProfile {
            size: [level.width, level.height],
            masked_pixels: holes.len(),
            donor_patches: candidates.len(),
            patch_radius: radius,
            passes,
        });
    }
    profiler.finish(Stage::Donors, start);
    let start = profiler.start();
    let mut random = Random(0x534e4f57);
    let mut output = colors.clone();
    let mut matches = vec![usize::MAX; colors.len()];
    for (n, i) in holes.iter().enumerate() {
        if n % 64 == 0 {
            cancel.check()?;
        }
        let mut best = nearest[*i];
        let mut error = distance(guide.color[*i], guide.color[best])
            + 4.0 * (guide.variation[*i] - guide.variation[best]).powi(2);
        for _ in 0..32 {
            let p = candidates[random.pick(candidates.len())];
            let e = distance(guide.color[*i], guide.color[p])
                + 4.0 * (guide.variation[*i] - guide.variation[p]).powi(2);
            if e < error {
                best = p;
                error = e;
            }
        }
        matches[*i] = best;
        output[*i] = colors[best];
        if let Some(prev) = previous {
            let x = *i % level.width;
            let y = *i / level.width;
            let px = x * prev.width / level.width;
            let py = y * prev.height / level.height;
            let p = prev.matches[py * prev.width + px];
            if p != usize::MAX {
                let sx = p % prev.width * level.width / prev.width;
                let sy = p / prev.width * level.height / prev.height;
                if let Some(j) = level.at(sx as isize, sy as isize)
                    && donors[j]
                {
                    matches[*i] = j;
                }
            }
            if p != usize::MAX {
                output[*i] = lab(prev.image[py * prev.width + px]);
            }
        }
    }
    let mut costs = vec![f32::MAX; colors.len()];
    let mut reuse = vec![0.0; colors.len()];
    let expected = (holes.len() as f32 / candidates.len() as f32).max(1.0);
    // Cropping unused donors must not change spatial penalties or the random
    // search schedule. Both use the original donor-neighborhood extent.
    let scale = level.search_span as f32;
    let mut next = output.clone();
    let mut rgb = level.source.clone();
    profiler.finish(Stage::Initialize, start);
    for pass in 0..passes {
        let start = profiler.start();
        cancel.check()?;
        let synthesized = 0.15 + 0.20 * pass as f32 / (passes - 1).max(1) as f32;
        reuse.fill(0.0);
        for i in &holes {
            reuse[matches[*i]] += 1.0;
        }
        for i in &candidates {
            reuse[*i] = 25.0 * (reuse[*i] / expected).ln_1p();
        }
        let step = if pass % 2 == 0 { 1_isize } else { -1 };
        for n in 0..holes.len() {
            if n % 64 == 0 {
                cancel.check()?;
            }
            let i = holes[if step > 0 { n } else { holes.len() - 1 - n }];
            let x = (i % level.width) as isize;
            let y = (i / level.width) as isize;
            let mut best = matches[i];
            let mut best_cost = f32::MAX;
            let mut try_candidate = |p: usize| {
                if p == usize::MAX || !donors[p] {
                    return;
                }
                let sx = (p % level.width) as isize;
                let sy = (p / level.width) as isize;
                let fixed = 8.0 / (4.0 + guide.variation[i])
                    * distance(guide.color[i], guide.color[p])
                    + 4.0 * (guide.variation[i] - guide.variation[p]).powi(2)
                    + reuse[p]
                    + 8.0 * ((sx - x).pow(2) + (sy - y).pow(2)) as f32 / (scale * scale);
                if fixed > best_cost {
                    return;
                }
                let mut sum = 0.0;
                let mut norm = 0.0;
                for dy in -(radius as isize)..=radius as isize {
                    for dx in -(radius as isize)..=radius as isize {
                        if let Some(t) = level.at(x + dx, y + dy) {
                            if !level.covered[t] {
                                continue;
                            }
                            let s = level.at(sx + dx, sy + dy).unwrap();
                            let weight = if level.hole[t] { synthesized } else { 1.0 };
                            sum += distance(output[t], colors[s]) * weight;
                            norm += weight;
                        }
                    }
                }
                let score = sum / norm.max(1.0) + fixed;
                if score < best_cost {
                    best = p;
                    best_cost = score;
                }
            };
            try_candidate(matches[i]);
            try_candidate(nearest[i]);
            try_candidate(candidates[random.pick(candidates.len())]);
            for (nx, ny, ox, oy) in [(x - step, y, step, 0), (x, y - step, 0, step)] {
                if let Some(n) = level.at(nx, ny) {
                    let p = matches[n];
                    if p != usize::MAX
                        && let Some(j) = level.at(
                            (p % level.width) as isize + ox,
                            (p / level.width) as isize + oy,
                        )
                    {
                        try_candidate(j);
                    }
                }
            }
            // Random windows are centered on the current best at each scale.
            // Borrow candidate selection in a separate scope to retain the best value.
            let center = matches[i];
            let mut window = level.search_span as isize;
            while window > 0 {
                let sx = (center % level.width) as isize
                    + random.pick((window * 2 + 1) as usize) as isize
                    - window;
                let sy = (center / level.width) as isize
                    + random.pick((window * 2 + 1) as usize) as isize
                    - window;
                if let Some(p) = level.at(sx, sy) {
                    try_candidate(p);
                }
                window /= 2;
            }
            matches[i] = best;
            costs[i] = best_cost;
        }
        profiler.finish(Stage::Matching, start);
        let start = profiler.start();
        for (n, i) in holes.iter().enumerate() {
            if n % 64 == 0 {
                cancel.check()?;
            }
            let x = (i % level.width) as isize;
            let y = (i / level.width) as isize;
            let anchor = colors[matches[*i]];
            let tolerance = (guide.variation[*i] * 0.25).clamp(1.5, 10.0);
            let mut sum = [0.0; 3];
            let mut sum_rgb = [0.0; 3];
            let mut total = 0.0;
            for dy in -(radius as isize)..=radius as isize {
                for dx in -(radius as isize)..=radius as isize {
                    if let Some(n) = level.at(x + dx, y + dy) {
                        if !level.hole[n] {
                            continue;
                        }
                        let p = matches[n];
                        if p == usize::MAX {
                            continue;
                        }
                        let j = level
                            .at(
                                (p % level.width) as isize - dx,
                                (p / level.width) as isize - dy,
                            )
                            .unwrap();
                        let weight = 1.0
                            / ((1.0 + costs[n])
                                * (1.0 + distance(guide.color[j], guide.color[*i]) / 100.0)
                                * (1.0 + distance(colors[j], anchor) / (tolerance * tolerance)));
                        for c in 0..3 {
                            sum[c] += colors[j][c] * weight;
                            sum_rgb[c] += level.source[j][c] * weight;
                        }
                        total += weight;
                    }
                }
            }
            if total > 0.0 {
                next[*i] = sum.map(|v| v / total);
                rgb[*i] = sum_rgb.map(|v| v / total);
            } else {
                next[*i] = output[*i];
                rgb[*i] = level.source[matches[*i]];
            }
        }
        std::mem::swap(&mut output, &mut next);
        profiler.finish(Stage::Voting, start);
    }
    Ok(ResultLevel {
        image: rgb,
        matches,
        width: level.width,
        height: level.height,
    })
}

fn downsample(fine: &Level, cancel: &Cancellation) -> Result<Level, CanvasError> {
    let known: Vec<_> = (0..fine.source.len()).map(|i| fine.known(i)).collect();
    let safe = erode(&known, fine.width, fine.height, 2);
    let domain = erode(&fine.domain, fine.width, fine.height, 2);
    let mut coarse = Level::new(fine.width.div_ceil(2), fine.height.div_ceil(2));
    coarse.search_span = fine.search_span.div_ceil(2);
    for y in 0..coarse.height {
        cancel.check()?;
        for x in 0..coarse.width {
            let i = y * coarse.width + x;
            let fx = x * 2;
            let fy = y * 2;
            let mut all_covered = true;
            let mut any_hole = false;
            let mut all_domain = true;
            let mut sum = [0.0; 3];
            let mut total = 0.0;
            for dy in -2_isize..=2 {
                for dx in -2_isize..=2 {
                    if let Some(j) = fine.at(fx as isize + dx, fy as isize + dy) {
                        all_covered &= fine.covered[j];
                        any_hole |= fine.hole[j];
                        all_domain &= domain[j];
                        if fine.known(j) {
                            let weights = [1.0, 4.0, 6.0, 4.0, 1.0];
                            let w = weights[(dx + 2) as usize] * weights[(dy + 2) as usize];
                            for (c, value) in sum.iter_mut().enumerate() {
                                *value += fine.source[j][c] * w;
                            }
                            total += w;
                        }
                    } else {
                        all_covered = false;
                        all_domain = false;
                    }
                }
            }
            coarse.hole[i] = any_hole && all_covered;
            coarse.covered[i] = all_covered;
            coarse.domain[i] = all_domain && safe[fy * fine.width + fx] && !any_hole;
            if total > 0.0 && !any_hole {
                coarse.source[i] = sum.map(|v| v / total);
            }
        }
    }

    Ok(coarse)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reduction_never_promotes_a_transparent_gap_to_coverage() {
        let mut fine = Level::new(161, 165);
        for y in 0..fine.height {
            for x in 0..fine.width {
                let i = y * fine.width + x;
                fine.covered[i] = !(78..84).contains(&x);
                fine.hole[i] = fine.covered[i] && (50..115).contains(&x) && (30..135).contains(&y);
                fine.domain[i] = fine.covered[i] && !fine.hole[i];
                fine.source[i] = if fine.known(i) {
                    [80.0 + (x % 7) as f32, 120.0, 160.0]
                } else {
                    [0.0; 3]
                };
            }
        }
        for _ in 0..3 {
            let coarse = downsample(&fine, &Default::default()).unwrap();
            for y in 0..coarse.height {
                for x in 0..coarse.width {
                    let i = y * coarse.width + x;
                    if coarse.covered[i] {
                        for dy in -2..=2 {
                            for dx in -2..=2 {
                                let j = fine
                                    .at((x * 2) as isize + dx, (y * 2) as isize + dy)
                                    .expect("covered border");
                                assert!(fine.covered[j], "coverage gap entered pyramid");
                            }
                        }
                    }
                    assert!(!coarse.domain[i] || coarse.covered[i] && !coarse.hole[i]);
                }
            }
            fine = coarse;
        }
    }
}
