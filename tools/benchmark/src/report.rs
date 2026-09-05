use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SceneResult {
    pub scene: String,
    pub valid: bool,
    pub frames: u32,
    pub elapsed_seconds: f64,
    pub render_fps: Option<f64>,
    pub errors: Vec<String>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct EngineResult {
    pub valid: bool,
    pub stopped: bool,
    pub errors: Vec<String>,
    pub scenes: Vec<SceneResult>,
    pub captures: Vec<Value>,
    pub stress_samples: Vec<Value>,
    pub environment: Value,
}

pub fn emit(event: &str, data: Value) {
    let mut object = data.as_object().cloned().unwrap_or_default();
    object.insert("schema_version".into(), json!(1));
    object.insert("event".into(), json!(event));
    println!("{}", Value::Object(object));
}

pub fn geometric_fps(scenes: &[SceneResult]) -> Option<f64> {
    if scenes.is_empty() {
        return None;
    }
    let mut sum = 0.;
    for scene in scenes {
        let fps = scene.render_fps?;
        if !scene.valid
            || !scene.errors.is_empty()
            || scene.frames == 0
            || !scene.elapsed_seconds.is_finite()
            || scene.elapsed_seconds <= 0.
            || !fps.is_finite()
            || fps <= 0.
        {
            return None;
        }
        let measured = scene.frames as f64 / scene.elapsed_seconds;
        if (fps - measured).abs() > 1e-6 * measured.max(1.) {
            return None;
        }
        sum += fps.ln();
    }
    Some((sum / scenes.len() as f64).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scene(fps: f64) -> SceneResult {
        SceneResult {
            scene: "materials".into(),
            valid: true,
            frames: 1200,
            elapsed_seconds: 1200. / fps,
            render_fps: Some(fps),
            errors: vec![],
        }
    }
    #[test]
    fn geomean_does_not_hide_individual_scenes() {
        let s = vec![scene(60.), scene(120.), scene(240.)];
        assert!((geometric_fps(&s).unwrap() - 120.).abs() < 1e-8);
        assert_eq!(s[0].render_fps, Some(60.));
    }
    #[test]
    fn invalid_or_missing_scene_cannot_be_scored() {
        assert_eq!(geometric_fps(&[]), None);
        let mut s = scene(120.);
        s.valid = false;
        assert_eq!(geometric_fps(&[s]), None);
        let mut s = scene(120.);
        s.render_fps = None;
        assert_eq!(geometric_fps(&[s]), None);
    }
    #[test]
    fn stored_rate_must_match_completed_count_and_elapsed() {
        let mut s = scene(120.);
        s.render_fps = Some(999.);
        assert_eq!(geometric_fps(&[s]), None);
    }
}
