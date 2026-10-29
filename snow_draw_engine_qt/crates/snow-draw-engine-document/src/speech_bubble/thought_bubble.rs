use super::*;

const MAX_TRAIL_CIRCLES: usize = 3;

#[derive(Clone, Copy)]
pub(super) struct Circle {
    pub center: Point<f64>,
    pub radius: f64,
}

/// Hand-shaped cloud puffs in a 100 × 100 box. A broad crown, unequal lobes and
/// small inward scallops give the cloud its silhouette without periodic waves.
/// The four extrema touch the box, and the contour stays star-shaped about its
/// center so resizing, ray-based hit testing and tail clearance share one shape.
pub(super) fn body_segments(body: RectangleData) -> Vec<Segment> {
    let map = |[x, y]: [f64; 2]| {
        Point::new(
            body.width * (x / 100.0 - 0.5),
            body.height * (y / 100.0 - 0.5),
        )
    };
    let mut start = map([47.0, 0.0]);
    [
        [[55.0, 0.0], [61.0, 4.0], [64.0, 11.5]],
        [[72.0, 5.0], [84.0, 10.0], [85.0, 24.5]],
        [[94.0, 22.0], [100.0, 30.0], [98.0, 43.5]],
        [[99.4, 46.0], [100.0, 49.0], [100.0, 52.0]],
        [[100.0, 61.0], [95.0, 68.0], [89.0, 69.0]],
        [[90.0, 80.0], [82.0, 90.0], [73.0, 87.0]],
        [[68.0, 98.0], [57.0, 100.0], [49.0, 92.0]],
        [[45.0, 97.0], [41.0, 100.0], [36.0, 100.0]],
        [[30.0, 100.0], [24.0, 96.0], [22.0, 89.0]],
        [[11.0, 93.0], [3.0, 81.0], [7.0, 69.0]],
        [[2.0, 65.0], [0.0, 60.0], [0.0, 54.0]],
        [[0.0, 45.0], [4.0, 38.0], [10.0, 36.0]],
        [[6.0, 24.0], [15.0, 12.0], [27.0, 16.0]],
        [[30.0, 5.0], [38.0, 0.0], [47.0, 0.0]],
    ]
    .into_iter()
    .map(|[a, b, end]| {
        let end = map(end);
        let segment = cubic(start, map(a), map(b), end);
        start = end;
        segment
    })
    .collect()
}

fn circles(value: &SpeechBubbleData, body: &[Segment], tail: &SpeechBubbleTail) -> Vec<Circle> {
    let distance = tail.tip.x.hypot(tail.tip.y);
    if distance <= f64::EPSILON {
        return Vec::new();
    }
    let axis = Point::new(tail.tip.x / distance, tail.tip.y / distance);
    let boundary = ray_radius(body, axis.y.atan2(axis.x));
    let available = (distance - boundary).max(0.0);
    let stroke = value.body.stroke_width;
    // Compress decorative spacing on short trails before reducing circle size.
    // The stroke clearance remains a hard minimum so outlines never touch.
    let gap = (tail.base_width * 0.22)
        .min(available * 0.25)
        .max(stroke + 1.0);
    if available <= gap + stroke * 0.5 {
        return Vec::new();
    }
    let requested = tail.base_width * 0.5;
    let mut count =
        ((available / (tail.base_width + gap)).ceil() as usize).clamp(1, MAX_TRAIL_CIRCLES);
    let (weights, radius, weight_sum) = loop {
        let weights: Vec<_> = (0..count).map(|i| 0.68_f64.powi(i as i32)).collect();
        // The endpoint is the terminal circle's center, not its outer edge.
        let sum = 2.0 * weights.iter().sum::<f64>() - weights[count - 1];
        let radius = requested.min((available - gap * count as f64).max(0.0) / sum);
        if count == 1 || radius * weights[count - 1] > 0.75 {
            break (weights, radius, sum);
        }
        count -= 1;
    };
    let extra = (available - radius * weight_sum - gap * count as f64).max(0.0);
    let spacing = gap + extra / count as f64;
    let mut cursor = boundary + spacing;
    let mut result = Vec::with_capacity(count);
    for (i, weight) in weights.into_iter().enumerate() {
        let r = radius * weight;
        let center = if i + 1 == count {
            tail.tip
        } else {
            Point::new(axis.x * (cursor + r), axis.y * (cursor + r))
        };
        if let Some(circle) = clear_circle(body, Circle { center, radius: r }, stroke) {
            result.push(circle);
        }
        cursor += 2.0 * r + spacing;
    }
    result
}

fn clear_circle(body: &[Segment], circle: Circle, stroke: f64) -> Option<Circle> {
    if circle.radius <= 0.25 || point_inside_body(body, circle.center) {
        return None;
    }
    // A ray can leave a concave valley close to an adjacent lobe. Test the
    // entire circle, including both outlines, rather than only its center.
    if body
        .iter()
        .any(|segment| segment_near_point(*segment, circle.center, circle.radius + stroke + 0.5, 0))
    {
        return None;
    }
    Some(circle)
}

pub(super) fn base_points(
    value: &SpeechBubbleData,
    body: &[Segment],
    tail: &SpeechBubbleTail,
) -> [Point<f64>; 2] {
    let center = circles(value, body, tail)
        .first()
        .map_or(tail.tip, |circle| circle.center);
    base_points_at(tail, center)
}

fn base_points_at(tail: &SpeechBubbleTail, center: Point<f64>) -> [Point<f64>; 2] {
    let length = tail.tip.x.hypot(tail.tip.y).max(f64::EPSILON);
    let offset = Point::new(
        -tail.tip.y / length * tail.base_width / 2.0,
        tail.tip.x / length * tail.base_width / 2.0,
    );
    [
        Point::new(center.x + offset.x, center.y + offset.y),
        Point::new(center.x - offset.x, center.y - offset.y),
    ]
}

pub(super) fn build_geometry(value: &SpeechBubbleData, key: &GeometryKey) -> BubbleGeometry {
    let body = body_segments(speech_bubble_body_proxy(value));
    let mut outline = body.clone();
    let mut commands =
        Vec::with_capacity(body.len() + 2 + value.tails.len() * MAX_TRAIL_CIRCLES * 6);
    let append = |commands: &mut Vec<PathCommand>, segments: &[Segment]| {
        if let Some(first) = segments.first() {
            commands.push(PathCommand::MoveTo {
                point: [first.start.x, first.start.y],
            });
            commands.extend(segments.iter().map(|segment| segment.command()));
            commands.push(PathCommand::LineTo {
                point: [first.start.x, first.start.y],
            });
        }
    };
    append(&mut commands, &body);
    let mut all_circles = Vec::with_capacity(value.tails.len() * MAX_TRAIL_CIRCLES);
    let mut bases = Vec::with_capacity(value.tails.len());
    for tail in &value.tails {
        let trail = circles(value, &body, tail);
        bases.push(base_points_at(
            tail,
            trail.first().map_or(tail.tip, |circle| circle.center),
        ));
        for circle in trail {
            let segments = arc_segments(
                circle.center,
                circle.radius,
                circle.radius,
                0.0,
                std::f64::consts::TAU,
            );
            append(&mut commands, &segments);
            outline.extend(segments);
            all_circles.push(circle);
        }
    }
    let mut revision = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut revision);
    BubbleGeometry {
        body,
        triangles: Vec::new(),
        tail_bases: bases,
        circles: all_circles,
        outline,
        path: PathGeometry::from_commands(revision.finish(), commands, true),
    }
}
