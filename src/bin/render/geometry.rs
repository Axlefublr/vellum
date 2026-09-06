use kurbo::{Affine, BezPath, ParamCurveNearest, Shape, Stroke};
use peniko::Fill;

#[derive(Debug, Clone)]
pub(super) enum DrawCommand {
    Fill {
        path: BezPath,
        fill_rule: Fill,
        color: [f32; 4],
    },
    Stroke {
        path: BezPath,
        stroke: Stroke,
        color: [f32; 4],
    },
}

#[derive(Debug, Clone, Default)]
pub struct Geometry {
    pub(super) commands: Vec<DrawCommand>,
}

pub struct LocalGeometry {
    pub(super) geometry: Geometry,
    pub(super) origin: [f32; 2],
    pub(super) size: [u32; 2],
}

impl LocalGeometry {
    pub fn new(geometry: Geometry, origin: [f32; 2], size: [u32; 2]) -> Self {
        Self {
            geometry,
            origin,
            size,
        }
    }
}

impl Geometry {
    pub fn fill(path: BezPath, fill_rule: Fill, color: [f32; 4]) -> Self {
        Self {
            commands: vec![DrawCommand::Fill {
                path,
                fill_rule,
                color,
            }],
        }
    }

    pub fn stroke(path: BezPath, stroke: Stroke, color: [f32; 4]) -> Self {
        Self {
            commands: vec![DrawCommand::Stroke {
                path,
                stroke,
                color,
            }],
        }
    }

    pub fn push_fill(&mut self, path: BezPath, fill_rule: Fill, color: [f32; 4]) {
        self.commands.push(DrawCommand::Fill {
            path,
            fill_rule,
            color,
        });
    }

    pub fn push_stroke(&mut self, path: BezPath, stroke: Stroke, color: [f32; 4]) {
        self.commands.push(DrawCommand::Stroke {
            path,
            stroke,
            color,
        });
    }

    pub fn append(&mut self, other: Self) {
        self.commands.extend(other.commands);
    }

    pub fn fill_hit_test(&self, point: kurbo::Point, slop: f64) -> bool {
        let slop = slop.max(0.0);
        let slop_squared = slop.powi(2);
        self.commands.iter().any(|command| {
            let DrawCommand::Fill {
                path, fill_rule, ..
            } = command
            else {
                return false;
            };
            // An edge hit avoids scanning the entire outline for its winding.
            if slop_squared > 0.0
                && path.segments().any(|segment| {
                    let bounds = segment.bounding_box().inflate(slop, slop);
                    point.x >= bounds.x0
                        && point.x <= bounds.x1
                        && point.y >= bounds.y0
                        && point.y <= bounds.y1
                        && segment.nearest(point, 0.1).distance_sq <= slop_squared
                })
            {
                return true;
            }
            let winding = path.winding(point);
            match fill_rule {
                Fill::NonZero => winding != 0,
                Fill::EvenOdd => winding % 2 != 0,
            }
        })
    }

    pub fn translated(&self, offset: [f32; 2]) -> Self {
        let transform = Affine::translate((f64::from(offset[0]), f64::from(offset[1])));
        Self {
            commands: self
                .commands
                .iter()
                .map(|command| match command {
                    DrawCommand::Fill {
                        path,
                        fill_rule,
                        color,
                    } => DrawCommand::Fill {
                        path: transform * path,
                        fill_rule: *fill_rule,
                        color: *color,
                    },
                    DrawCommand::Stroke {
                        path,
                        stroke,
                        color,
                    } => DrawCommand::Stroke {
                        path: transform * path,
                        stroke: stroke.clone(),
                        color: *color,
                    },
                })
                .collect(),
        }
    }
}
