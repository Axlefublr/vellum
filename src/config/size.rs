use std::collections::BTreeMap;

use super::ToolDefaults;
use crate::tool::Tool;

const DEFAULT_TEXT_MIN_SIZE: f32 = 8.0;
const DEFAULT_TEXT_MAX_SIZE: f32 = 500.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct SizeRange {
    min: f32,
    max: f32,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SizeRangeConfig {
    min: Option<f32>,
    max: Option<f32>,
}

impl SizeRangeConfig {
    pub(super) fn resolve(&self, fallback: &SizeRange) -> SizeRange {
        SizeRange {
            min: self.min.unwrap_or(fallback.min),
            max: self.max.unwrap_or(fallback.max),
        }
    }
}

impl Default for SizeRange {
    fn default() -> Self {
        Self {
            min: 1.0,
            max: 100.0,
        }
    }
}

impl SizeRange {
    pub(super) fn validate(self, name: &str) -> Result<Self, String> {
        if !self.min.is_finite() || self.min <= 0.0 {
            return Err(format!("{name}.min must be greater than 0"));
        }
        if !self.max.is_finite() || self.max < self.min {
            return Err(format!(
                "{name}.max must be greater than or equal to {name}.min"
            ));
        }
        Ok(self)
    }

    pub(crate) fn contains(&self, size: f32) -> bool {
        size.is_finite() && (self.min..=self.max).contains(&size)
    }

    pub(crate) fn clamp(&self, size: f32) -> f32 {
        size.clamp(self.min, self.max)
    }

    pub(crate) fn min(&self) -> f32 {
        self.min
    }

    pub(crate) fn max(&self) -> f32 {
        self.max
    }
}

pub(super) fn resolve_size_ranges(
    tools: &ToolDefaults,
    fallback: &SizeRange,
    global: &SizeRangeConfig,
) -> Result<BTreeMap<Tool, SizeRange>, String> {
    Tool::SIZED
        .into_iter()
        .map(|tool| {
            let name = format!("tools.{}.size_range", tool.name());
            let mut fallback = *fallback;
            if tool == Tool::Text {
                if global.min.is_none() {
                    fallback.min = DEFAULT_TEXT_MIN_SIZE;
                }
                if global.max.is_none() {
                    fallback.max = DEFAULT_TEXT_MAX_SIZE;
                }
            }
            let range = tools
                .get(&tool)
                .and_then(|defaults| defaults.size_range.as_ref())
                .map_or(fallback, |range| range.resolve(&fallback))
                .validate(&name)?;
            Ok((tool, range))
        })
        .collect()
}
