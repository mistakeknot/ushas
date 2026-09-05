use bevy_metalfx::MetalFxMode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const PROFILE_VERSION: &str = "claude-lab-standard-v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Native,
    Temporal,
    Spatial,
    Bilinear,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Temporal => "temporal",
            Self::Spatial => "spatial",
            Self::Bilinear => "bilinear",
        }
    }
    pub fn metalfx(self) -> MetalFxMode {
        match self {
            Self::Native | Self::Bilinear => MetalFxMode::Disabled,
            Self::Temporal => MetalFxMode::Temporal,
            Self::Spatial => MetalFxMode::Spatial,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SceneKind {
    #[default]
    Materials,
    Geometry,
    Lighting,
}
impl SceneKind {
    pub const ALL: [Self; 3] = [Self::Materials, Self::Geometry, Self::Lighting];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Materials => "materials",
            Self::Geometry => "geometry",
            Self::Lighting => "lighting",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    #[default]
    Benchmark,
    Stress,
    Capture,
}
impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Benchmark => "benchmark",
            Self::Stress => "stress",
            Self::Capture => "capture",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StressLoad {
    pub claudes: Option<u32>,
    pub lights: Option<u32>,
    pub particles: Option<u32>,
    pub fill: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub action: Action,
    pub mode: Mode,
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub seed: u64,
    pub scene: Option<SceneKind>,
    pub duration: u64,
    pub out: PathBuf,
    pub load: StressLoad,
}
impl Default for RunConfig {
    fn default() -> Self {
        Self {
            action: Action::Benchmark,
            mode: Mode::Native,
            scale: 1.,
            width: 2560,
            height: 1440,
            frames: 1200,
            seed: 21434,
            scene: None,
            duration: 600,
            out: PathBuf::new(),
            load: StressLoad::default(),
        }
    }
}
impl RunConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.out.as_os_str().is_empty() {
            return Err("--out requires a new output directory".into());
        }
        if !(128..=7680).contains(&self.width) || !(128..=4320).contains(&self.height) {
            return Err("output dimensions must be within128..7680 by128..4320".into());
        }
        if !(1..=12000).contains(&self.frames) || !(1..=3600).contains(&self.duration) {
            return Err("frames must be1..12000 and duration1..3600seconds".into());
        }
        if !self.scale.is_finite()
            || ![1.0f32, 2.0 / 3.0, 0.5]
                .iter()
                .any(|s| (self.scale - s).abs() < 0.000001)
        {
            return Err("scale must be1,2/3 or1/2".into());
        }
        if self.mode == Mode::Native && self.scale != 1.0 {
            return Err(
                "native mode requires scale1; use bilinear for a reduced-resolution control".into(),
            );
        }
        if self.load.claudes.is_some_and(|v| !(1..=256).contains(&v))
            || self.load.lights.is_some_and(|v| !(1..=16).contains(&v))
            || self.load.particles.is_some_and(|v| v > 16384)
            || self.load.fill > 8000
        {
            return Err(
                "stress limits:1..256Claudes,1..16lights,0..16384particles,0..8000fill".into(),
            );
        }
        if self.action != Action::Stress && self.load != StressLoad::default() {
            return Err("custom workload controls are available only in stress mode".into());
        }
        Ok(())
    }
    pub fn standard(&self) -> bool {
        self.action != Action::Stress
            && self.width == 2560
            && self.height == 1440
            && self.frames == 1200
            && self.seed == 21434
            && self.scene.is_none()
            && self.load == StressLoad::default()
    }
    pub fn scenes(&self) -> Vec<SceneKind> {
        self.scene
            .map_or_else(|| SceneKind::ALL.to_vec(), |s| vec![s])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> RunConfig {
        RunConfig {
            out: "/tmp/bench-test".into(),
            ..Default::default()
        }
    }
    #[test]
    fn standard_excludes_custom_workload_and_developer_dimensions() {
        let mut c = config();
        assert!(c.standard());
        c.load.fill = 1;
        assert!(!c.standard());
        c.load.fill = 0;
        c.width = 1280;
        assert!(!c.standard());
        c.width = 2560;
        c.frames = 120;
        assert!(!c.standard());
        c.frames = 1200;
        c.action = Action::Stress;
        assert!(!c.standard());
    }
    #[test]
    fn native_cannot_silently_become_bilinear() {
        let mut c = config();
        c.scale = 0.5;
        assert!(c.validate().is_err());
        c.mode = Mode::Bilinear;
        assert!(c.validate().is_ok());
    }
    #[test]
    fn invalid_numbers_and_unbounded_load_fail_before_launch() {
        let mut c = config();
        c.scale = f32::NAN;
        assert!(c.validate().is_err());
        c.scale = 1.;
        c.load.claudes = Some(257);
        assert!(c.validate().is_err());
        c.load.claudes = None;
        c.duration = 0;
        assert!(c.validate().is_err());
        c.duration = 600;
        c.out = PathBuf::new();
        assert!(c.validate().is_err());
    }
    #[test]
    fn only_stress_may_request_custom_burn() {
        let mut c = config();
        c.load.fill = 100;
        assert!(c.validate().is_err());
        c.action = Action::Stress;
        assert!(c.validate().is_ok());
    }
    #[test]
    fn configuration_roundtrip_preserves_exact_scale_and_scene() {
        let mut c = config();
        c.mode = Mode::Temporal;
        c.scale = 2. / 3.;
        c.scene = Some(SceneKind::Geometry);
        let r: RunConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(r.scale.to_bits(), c.scale.to_bits());
        assert_eq!(r.scenes(), vec![SceneKind::Geometry]);
    }
}
