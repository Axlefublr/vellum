use super::{CIRCLE_KAPPA, freehand};
use crate::render::{Geometry, layout_text};
use peniko::Fill;
use std::borrow::Cow;

pub(super) const HIT_SLOP: f32 = 5.0;

pub type ElementId = u64;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub(super) fn distance_squared(self, other: Self) -> f32 {
        (self.x - other.x).powi(2) + (self.y - other.y).powi(2)
    }

    pub(super) fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub(super) fn midpoint(self, other: Self) -> Self {
        (self + other) * 0.5
    }

    pub(in crate::state) fn segment_distance_squared(self, start: Self, end: Self) -> f32 {
        let delta = end - start;
        let length_squared = delta.distance_squared(Self::default());
        if length_squared <= f32::EPSILON {
            return self.distance_squared(start);
        }
        let offset = self - start;
        let fraction = ((offset.x * delta.x + offset.y * delta.y) / length_squared).clamp(0.0, 1.0);
        self.distance_squared(start + delta * fraction)
    }
}

fn pixel_aligned_coordinate(value: f32, width: f32) -> f32 {
    if !value.is_finite() || !width.is_finite() || width <= 0.0 {
        return value;
    }
    let radius = width * 0.5;
    (value - radius + 0.5).floor() + radius
}

pub(super) fn pixel_aligned_point(point: Point, width: f32) -> Point {
    Point::new(
        pixel_aligned_coordinate(point.x, width),
        pixel_aligned_coordinate(point.y, width),
    )
}

pub(super) fn pixel_aligned_points(points: &[Point], width: f32) -> Cow<'_, [Point]> {
    let Some(&first) = points.first() else {
        return Cow::Borrowed(points);
    };
    let offset = pixel_aligned_point(first, width) - first;
    if offset == Point::default() {
        Cow::Borrowed(points)
    } else {
        Cow::Owned(points.iter().map(|point| *point + offset).collect())
    }
}

fn pixel_aligned_segment(mut start: Point, mut end: Point, width: f32) -> (Point, Point) {
    let delta = end - start;
    let length = delta.length();
    if length <= f32::EPSILON {
        let point = pixel_aligned_point(start, width);
        return (point, point);
    }
    let tolerance = length * 1e-6;
    if delta.y.abs() <= tolerance {
        let y = pixel_aligned_coordinate(start.y, width);
        start.y = y;
        end.y = y;
    } else if delta.x.abs() <= tolerance {
        let x = pixel_aligned_coordinate(start.x, width);
        start.x = x;
        end.x = x;
    }
    (start, end)
}

fn pixel_aligned_rectangle(min: Point, max: Point, width: f32) -> (Point, Point) {
    (
        pixel_aligned_point(min, width),
        pixel_aligned_point(max, width),
    )
}

impl std::ops::Sub for Point {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Add for Point {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::Mul<f32> for Point {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bounds {
    pub min: Point,
    pub max: Point,
}

impl Bounds {
    fn from_points(points: impl IntoIterator<Item = Point>) -> Self {
        let mut points = points.into_iter();
        let Some(first) = points.next() else {
            return Self::default();
        };
        let mut bounds = Self {
            min: first,
            max: first,
        };
        for point in points {
            bounds.min.x = bounds.min.x.min(point.x);
            bounds.min.y = bounds.min.y.min(point.y);
            bounds.max.x = bounds.max.x.max(point.x);
            bounds.max.y = bounds.max.y.max(point.y);
        }
        bounds
    }

    pub(super) fn expanded(self, amount: f32) -> Self {
        Self {
            min: Point::new(self.min.x - amount, self.min.y - amount),
            max: Point::new(self.max.x + amount, self.max.y + amount),
        }
    }

    pub fn contains(self, point: Point) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub size: f32,
    pub color: [f32; 4],
    pub roundness: f32,
    pub filled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElementKind {
    Freehand {
        points: Vec<Point>,
    },
    Segment {
        points: [Point; 2],
        arrow: bool,
    },
    Triangle {
        vertices: [Point; 3],
    },
    Rectangle {
        min: Point,
        max: Point,
    },
    Ellipse {
        center: Point,
        radii: Point,
    },
    Text {
        origin: Point,
        content: String,
        scale: [f32; 2],
    },
}

impl ElementKind {
    pub(super) fn translated(&self, delta: Point) -> Self {
        let mut translated = self.clone();
        match &mut translated {
            Self::Freehand { points } => {
                points.iter_mut().for_each(|point| *point = *point + delta);
            }
            Self::Segment { points, .. } => {
                points.iter_mut().for_each(|point| *point = *point + delta);
            }
            Self::Triangle { vertices } => vertices
                .iter_mut()
                .for_each(|point| *point = *point + delta),
            Self::Rectangle { min, max } => {
                *min = *min + delta;
                *max = *max + delta;
            }
            Self::Ellipse { center, .. } => *center = *center + delta,
            Self::Text { origin, .. } => *origin = *origin + delta,
        }
        translated
    }
}

#[derive(Debug)]
pub struct Element {
    pub id: ElementId,
    pub kind: ElementKind,
    pub style: Style,
    pub bounds: Bounds,
    pub geometry: Geometry,
}

impl Element {
    pub(super) fn new(id: ElementId, kind: ElementKind, style: Style) -> Self {
        let geometry = geometry(&kind, style);
        Self::with_geometry(id, kind, style, geometry)
    }

    pub(super) fn with_geometry(
        id: ElementId,
        kind: ElementKind,
        style: Style,
        geometry: Geometry,
    ) -> Self {
        let bounds = bounds_for(&kind, style);
        Self {
            id,
            kind,
            style,
            bounds,
            geometry,
        }
    }

    pub(super) fn replace(&mut self, kind: ElementKind, style: Style) -> (ElementKind, Style) {
        let kind = std::mem::replace(&mut self.kind, kind);
        let style = std::mem::replace(&mut self.style, style);
        self.bounds = bounds_for(&self.kind, self.style);
        self.geometry = geometry(&self.kind, self.style);
        (kind, style)
    }

    pub(super) fn hit_test(&self, point: Point) -> bool {
        self.hit_test_with_slop(point, HIT_SLOP, false)
    }

    pub(super) fn erase_hit_test(&self, point: Point, radius: f32) -> bool {
        self.hit_test_with_slop(point, radius, true)
    }

    fn hit_test_with_slop(&self, point: Point, slop: f32, expand_text: bool) -> bool {
        if !self.bounds.expanded(slop).contains(point) {
            return false;
        }
        let tolerance = self.style.size * 0.5 + slop;
        match &self.kind {
            ElementKind::Segment {
                points: [start, end],
                arrow: false,
            } => {
                let (start, end) = pixel_aligned_segment(*start, *end, self.style.size);
                point.segment_distance_squared(start, end) <= tolerance * tolerance
            }
            ElementKind::Freehand { .. }
            | ElementKind::Segment { arrow: true, .. }
            | ElementKind::Triangle { .. } => self
                .geometry
                .fill_hit_test(kurbo_point(point), f64::from(slop)),
            ElementKind::Rectangle { min, max } => {
                let (min, max) = pixel_aligned_rectangle(*min, *max, self.style.size);
                rounded_rectangle_hit(
                    min,
                    max,
                    self.style.roundness,
                    self.style.filled,
                    point,
                    tolerance,
                )
            }
            ElementKind::Ellipse { center, radii } => {
                let local = point - *center;
                (self.style.filled
                    && radii.x > 0.0
                    && radii.y > 0.0
                    && (local.x / radii.x).powi(2) + (local.y / radii.y).powi(2) <= 1.0)
                    || ellipse_distance(local, *radii) <= tolerance
            }
            ElementKind::Text { .. } => {
                if expand_text {
                    self.bounds.expanded(slop).contains(point)
                } else {
                    self.bounds.contains(point)
                }
            }
        }
    }
}

fn ellipse_distance(point: Point, radii: Point) -> f32 {
    let (x, y, a, b) = if radii.x >= radii.y {
        (point.x.abs(), point.y.abs(), radii.x, radii.y)
    } else {
        (point.y.abs(), point.x.abs(), radii.y, radii.x)
    };
    if b <= f32::EPSILON {
        return (x - a).max(0.0).hypot(y);
    }
    let (x, y, a, b) = (f64::from(x), f64::from(y), f64::from(a), f64::from(b));
    let (a2, b2) = (a * a, b * b);
    let (nearest_x, nearest_y) = if y == 0.0 {
        let difference = a2 - b2;
        if a * x < difference {
            let nearest_x = a2 * x / difference;
            (nearest_x, b * (1.0 - (nearest_x / a).powi(2)).sqrt())
        } else {
            (a, 0.0)
        }
    } else if x == 0.0 {
        (0.0, b)
    } else {
        // The nearest point satisfies the ellipse equation with one scalar multiplier.
        // Its residual decreases monotonically above -b², so bisection stays bounded.
        let mut low = b * (y - b);
        let mut high = (a * x + b * y).max(0.0);
        for _ in 0..64 {
            let t = (low + high) * 0.5;
            let residual = (a * x / (t + a2)).powi(2) + (b * y / (t + b2)).powi(2);
            if residual > 1.0 {
                low = t;
            } else {
                high = t;
            }
        }
        let t = (low + high) * 0.5;
        (a2 * x / (t + a2), b2 * y / (t + b2))
    };
    (x - nearest_x).hypot(y - nearest_y) as f32
}

pub(super) fn bounds_for(kind: &ElementKind, style: Style) -> Bounds {
    if let ElementKind::Triangle { vertices } = kind {
        return super::triangle::bounds(vertices, style);
    }
    let width = style.size;
    let bounds = match kind {
        ElementKind::Segment {
            points: [start, end],
            arrow,
        } => {
            let (start, end) = pixel_aligned_segment(*start, *end, width);
            if *arrow {
                Bounds::from_points(
                    [start, end]
                        .into_iter()
                        .chain(arrow_head(start, end, width).vertices),
                )
            } else {
                Bounds::from_points([start, end])
            }
        }
        ElementKind::Freehand { points } => {
            Bounds::from_points(pixel_aligned_points(points, width).iter().copied())
        }
        ElementKind::Triangle { .. } => unreachable!(),
        ElementKind::Rectangle { min, max } => {
            let (min, max) = pixel_aligned_rectangle(*min, *max, width);
            Bounds { min, max }
        }
        ElementKind::Ellipse { center, radii } => Bounds {
            min: Point::new(center.x - radii.x, center.y - radii.y),
            max: Point::new(center.x + radii.x, center.y + radii.y),
        },
        ElementKind::Text {
            origin,
            content,
            scale,
        } => {
            let layout = layout_text(content, style.size);
            text_bounds(
                *origin,
                [layout.full_width(), layout.height()],
                style,
                *scale,
            )
        }
    };
    let radius = width * 0.5;
    let expansion = match kind {
        ElementKind::Text { .. } => 0.0,
        ElementKind::Freehand { .. } => {
            let roundness = style.roundness.clamp(0.0, 1.0);
            radius * (std::f32::consts::SQRT_2 - (std::f32::consts::SQRT_2 - 1.0) * roundness)
        }
        _ => radius,
    };
    bounds.expanded(expansion)
}

pub(super) fn text_bounds(
    origin: Point,
    [width, height]: [f32; 2],
    style: Style,
    scale: [f32; 2],
) -> Bounds {
    let [[min_x, min_y], [max_x, max_y]] = crate::render::text_bounds(
        [origin.x, origin.y],
        [width, height],
        style.size,
        style.filled.then_some(style.roundness),
        scale,
    );
    Bounds {
        min: Point::new(min_x, min_y),
        max: Point::new(max_x, max_y),
    }
}

pub(super) fn geometry(kind: &ElementKind, style: Style) -> Geometry {
    use kurbo::Shape;

    match kind {
        ElementKind::Segment {
            points: [start, end],
            arrow: true,
        } => {
            let (start, end) = pixel_aligned_segment(*start, *end, style.size);
            Geometry::fill(
                arrow_path(arrow_head(start, end, style.size), style.roundness),
                Fill::NonZero,
                style.color,
            )
        }
        ElementKind::Freehand { points } => freehand::geometry(points, style),
        ElementKind::Segment {
            points: [start, end],
            arrow: false,
        } => Geometry::fill(
            line_path(*start, *end, style.size, style.roundness),
            Fill::NonZero,
            style.color,
        ),
        ElementKind::Triangle { vertices } => super::triangle::geometry(vertices, style),
        ElementKind::Rectangle { min, max } => {
            let (min, max) = pixel_aligned_rectangle(*min, *max, style.size);
            rectangle_geometry(min, max, style)
        }
        ElementKind::Ellipse { center, radii } if style.filled => {
            let half = style.size * 0.5;
            let path = kurbo::Ellipse::new(
                (f64::from(center.x), f64::from(center.y)),
                (
                    f64::from((radii.x + half).max(0.0)),
                    f64::from((radii.y + half).max(0.0)),
                ),
                0.0,
            )
            .to_path(0.1);
            Geometry::fill(path, Fill::NonZero, style.color)
        }
        ElementKind::Ellipse { center, radii } => {
            let path = kurbo::Ellipse::new(
                (f64::from(center.x), f64::from(center.y)),
                (f64::from(radii.x), f64::from(radii.y)),
                0.0,
            )
            .to_path(0.1);
            Geometry::stroke(
                path,
                kurbo::Stroke::new(f64::from(style.size))
                    .with_join(kurbo::Join::Miter)
                    .with_caps(kurbo::Cap::Butt)
                    .with_miter_limit(4.0),
                style.color,
            )
        }
        ElementKind::Text { .. } => Geometry::default(),
    }
}

pub(super) fn rendered_segment_endpoints(kind: &ElementKind, style: Style) -> Option<[Point; 2]> {
    let ElementKind::Segment {
        points: [first, last],
        arrow,
    } = kind
    else {
        return None;
    };
    let (start, end) = pixel_aligned_segment(*first, *last, style.size);
    let end = if *arrow {
        arrow_head(start, end, style.size).rendered_tip(style.roundness)
    } else {
        end
    };
    Some([start, end])
}

fn rectangle_radius(min: Point, max: Point, roundness: f32) -> f32 {
    ((max.x - min.x).abs().min((max.y - min.y).abs()) * 0.5) * roundness
}

fn rectangle_geometry(min: Point, max: Point, style: Style) -> Geometry {
    use kurbo::Shape;

    let half = style.size * 0.5;
    let maximum = (max.x - min.x).abs().min((max.y - min.y).abs()) * 0.5;
    let outer_radius = if style.roundness <= f32::EPSILON {
        0.0
    } else {
        half + maximum * style.roundness
    };
    let contours = [
        (
            Point::new(min.x - half, min.y - half),
            Point::new(max.x + half, max.y + half),
            outer_radius,
        ),
        (
            Point::new(min.x + half, min.y + half),
            Point::new(max.x - half, max.y - half),
            (maximum - half).max(0.0) * style.roundness,
        ),
    ];
    let mut path = kurbo::BezPath::new();
    let contour_count = if style.filled { 1 } else { contours.len() };
    for (min, max, radius) in contours.into_iter().take(contour_count) {
        if min.x >= max.x || min.y >= max.y {
            continue;
        }
        if radius <= f32::EPSILON {
            path.extend(
                kurbo::Rect::new(
                    f64::from(min.x),
                    f64::from(min.y),
                    f64::from(max.x),
                    f64::from(max.y),
                )
                .path_elements(0.1),
            );
        } else {
            path.extend(
                kurbo::RoundedRect::new(
                    f64::from(min.x),
                    f64::from(min.y),
                    f64::from(max.x),
                    f64::from(max.y),
                    f64::from(radius),
                )
                .path_elements(0.1),
            );
        }
    }
    Geometry::fill(path, Fill::EvenOdd, style.color)
}

fn rounded_rectangle_hit(
    min: Point,
    max: Point,
    roundness: f32,
    filled: bool,
    point: Point,
    tolerance: f32,
) -> bool {
    let radius = rectangle_radius(min, max, roundness);
    let center = min.midpoint(max);
    let x = (point.x - center.x).abs() - ((max.x - min.x) * 0.5 - radius);
    let y = (point.y - center.y).abs() - ((max.y - min.y) * 0.5 - radius);
    let distance = x.max(0.0).hypot(y.max(0.0)) + x.max(y).min(0.0) - radius;
    if filled {
        distance <= tolerance
    } else {
        distance.abs() <= tolerance
    }
}

pub(super) fn tool_for(kind: &ElementKind) -> super::tool::Tool {
    use super::tool::Tool;

    match kind {
        ElementKind::Freehand { .. } => Tool::Pen,
        ElementKind::Segment { arrow: true, .. } => Tool::Arrow,
        ElementKind::Segment { arrow: false, .. } => Tool::Line,
        ElementKind::Triangle { .. } => Tool::Triangle,
        ElementKind::Rectangle { .. } => Tool::Rectangle,
        ElementKind::Ellipse { .. } => Tool::Ellipse,
        ElementKind::Text { .. } => Tool::Text,
    }
}

#[derive(Clone, Copy)]
struct ArrowHead {
    tail: Point,
    vertices: [Point; 3],
    base: Point,
    normal: Point,
    radius: f32,
    shaft_length: f32,
}

impl ArrowHead {
    fn rendered_tip(self, roundness: f32) -> Point {
        let (before, after) = rounded_polygon_corner(&self.vertices, 0, roundness);
        (before + self.vertices[0] * 2.0 + after) * 0.25
    }
}

fn arrow_head(start: Point, end: Point, width: f32) -> ArrowHead {
    let delta = end - start;
    let length = delta.length();
    if length <= f32::EPSILON {
        return ArrowHead {
            tail: start,
            vertices: [end; 3],
            base: end,
            normal: Point::default(),
            radius: width * 0.5,
            shaft_length: 0.0,
        };
    }
    let direction = Point::new(delta.x / length, delta.y / length);
    let normal = Point::new(-direction.y, direction.x);
    let ideal_size = (width * 5.0).max(16.0);
    let size = ideal_size.min(length);
    let base = Point::new(end.x - direction.x * size, end.y - direction.y * size);
    let half = size * 0.45;
    ArrowHead {
        tail: start,
        vertices: [
            end,
            Point::new(base.x + normal.x * half, base.y + normal.y * half),
            Point::new(base.x - normal.x * half, base.y - normal.y * half),
        ],
        base,
        normal,
        radius: width * 0.5,
        shaft_length: length - size,
    }
}

fn arrow_path(head: ArrowHead, roundness: f32) -> kurbo::BezPath {
    if head.shaft_length <= f32::EPSILON {
        return rounded_polygon_path(&head.vertices, roundness);
    }
    let shaft_offset = head.normal * head.radius;
    rounded_polygon_path(
        &[
            head.tail + shaft_offset,
            head.base + shaft_offset,
            head.vertices[1],
            head.vertices[0],
            head.vertices[2],
            head.base - shaft_offset,
            head.tail - shaft_offset,
        ],
        roundness,
    )
}

fn line_path(start: Point, end: Point, width: f32, roundness: f32) -> kurbo::BezPath {
    let (start, end) = pixel_aligned_segment(start, end, width);
    let delta = end - start;
    let delta_x = f64::from(delta.x);
    let delta_y = f64::from(delta.y);
    let length = delta_x.hypot(delta_y);
    let radius = f64::from(width.max(0.0)) * 0.5;
    let mut path = kurbo::BezPath::new();
    if length <= f64::EPSILON || radius <= f64::EPSILON {
        return path;
    }

    let cap = (radius * f64::from(roundness.clamp(0.0, 1.0))).min(length * 0.5);
    let control_x = cap * CIRCLE_KAPPA;
    let control_y = radius * CIRCLE_KAPPA;
    let end_center = length - cap;

    path.move_to((cap, -radius));
    path.line_to((end_center, -radius));
    path.curve_to(
        (end_center + control_x, -radius),
        (length, -control_y),
        (length, 0.0),
    );
    path.curve_to(
        (length, control_y),
        (end_center + control_x, radius),
        (end_center, radius),
    );
    path.line_to((cap, radius));
    path.curve_to((cap - control_x, radius), (0.0, control_y), (0.0, 0.0));
    path.curve_to(
        (0.0, -control_y),
        (cap - control_x, -radius),
        (cap, -radius),
    );
    path.close_path();
    let transform = if delta.y == 0.0 {
        let direction = f64::from(delta.x.signum());
        kurbo::Affine::new([
            direction,
            0.0,
            0.0,
            direction,
            f64::from(start.x),
            f64::from(start.y),
        ])
    } else if delta.x == 0.0 {
        let direction = f64::from(delta.y.signum());
        kurbo::Affine::new([
            0.0,
            direction,
            -direction,
            0.0,
            f64::from(start.x),
            f64::from(start.y),
        ])
    } else {
        kurbo::Affine::rotate(delta_y.atan2(delta_x))
            .then_translate(kurbo::Vec2::new(f64::from(start.x), f64::from(start.y)))
    };
    path.apply_affine(transform);
    path
}

fn rounded_polygon_path(vertices: &[Point], roundness: f32) -> kurbo::BezPath {
    let mut path = kurbo::BezPath::new();
    if vertices.len() < 3 {
        return path;
    }
    let mut corner = rounded_polygon_corner(vertices, 0, roundness);
    path.move_to(kurbo_point(corner.0));
    for index in 0..vertices.len() {
        let vertex = vertices[index];
        let (before, after) = corner;
        if before == after {
            path.line_to(kurbo_point(vertex));
        } else {
            path.quad_to(kurbo_point(vertex), kurbo_point(after));
        }
        if index + 1 < vertices.len() {
            corner = rounded_polygon_corner(vertices, index + 1, roundness);
            path.line_to(kurbo_point(corner.0));
        }
    }
    path.close_path();
    path
}

fn rounded_polygon_corner(vertices: &[Point], index: usize, roundness: f32) -> (Point, Point) {
    let (before, after) = rounded_corner(
        kurbo_point(vertices[(index + vertices.len() - 1) % vertices.len()]),
        kurbo_point(vertices[index]),
        kurbo_point(vertices[(index + 1) % vertices.len()]),
        f64::from(roundness),
    );
    (
        Point::new(before.x as f32, before.y as f32),
        Point::new(after.x as f32, after.y as f32),
    )
}

pub(super) fn rounded_corner(
    previous: kurbo::Point,
    vertex: kurbo::Point,
    next: kurbo::Point,
    roundness: f64,
) -> (kurbo::Point, kurbo::Point) {
    let roundness = roundness.clamp(0.0, 1.0);
    if roundness <= f64::EPSILON {
        return (vertex, vertex);
    }
    let to_previous = previous - vertex;
    let to_next = next - vertex;
    let previous_length = to_previous.hypot();
    let next_length = to_next.hypot();
    if previous_length <= f64::EPSILON || next_length <= f64::EPSILON {
        return (vertex, vertex);
    }
    let cut = 0.3 * roundness * previous_length.min(next_length);
    (
        vertex + to_previous * (cut / previous_length),
        vertex + to_next * (cut / next_length),
    )
}

fn kurbo_point(point: Point) -> kurbo::Point {
    kurbo::Point::new(f64::from(point.x), f64::from(point.y))
}
