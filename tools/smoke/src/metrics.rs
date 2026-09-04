//! CPU frame-loop observations, deliberately not GPU or presented FPS.
use serde_json::{json, Value};

/// Inspect the scene area, excluding the UI header and window edges. A flat
/// colored image has channel variation but is still missing scene content.
pub fn image_proof(rgba: &[u8], width: u32, height: u32) -> Value {
    let mut colors = std::collections::BTreeSet::new();
    let (mut low, mut high) = (255u8, 0u8);
    for y in ((height / 5)..(height * 9 / 10)).step_by(2) {
        for x in ((width / 10)..(width * 9 / 10)).step_by(2) {
            let offset = ((y as usize * width as usize) + x as usize) * 4;
            let Some(pixel) = rgba.get(offset..offset + 3) else {
                return json!({"nonuniform":false,"error":"truncated pixels"});
            };
            colors.insert([pixel[0], pixel[1], pixel[2]]);
            let luminance =
                ((u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])) / 3) as u8;
            low = low.min(luminance);
            high = high.max(luminance);
        }
    }
    json!({"nonuniform":colors.len() > 32 && high.saturating_sub(low) > 32,
        "scene_unique_colors":colors.len(),"scene_luminance_min":low,"scene_luminance_max":high})
}

pub fn summarize(samples_ms: &[f64], target_fps: f64) -> Value {
    let mut valid: Vec<_> = samples_ms
        .iter()
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect();
    valid.sort_by(f64::total_cmp);
    if valid.is_empty() {
        return json!({"count":0});
    }
    let mean = valid.iter().sum::<f64>() / valid.len() as f64;
    let percentile = |q: f64| valid[((valid.len() - 1) as f64 * q).round() as usize];
    json!({"count":valid.len(), "mean_ms":mean, "p50_ms":percentile(0.5),
        "p95_ms":percentile(0.95), "p99_ms":percentile(0.99),
        "loop_fps":1000.0/mean, "budget_ms":1000.0/target_fps,
        "budget_miss_fraction":valid.iter().filter(|v| **v > 1000.0/target_fps).count() as f64/valid.len() as f64})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_and_invalid_are_not_zero_cost() {
        assert_eq!(summarize(&[f64::NAN, 0.0, -1.0], 60.0), json!({"count":0}));
    }
    #[test]
    fn counts_misses_without_calling_them_gpu_cost() {
        let s = summarize(&[10.0, 10.0, 20.0, 40.0], 60.0);
        assert_eq!(s["mean_ms"], 20.0);
        assert_eq!(s["budget_miss_fraction"], 0.5);
        assert_eq!(s["loop_fps"], 50.0);
    }

    #[test]
    fn a_flat_colored_frame_is_not_render_proof() {
        let flat = [240u8, 10, 20, 255].repeat(100 * 100);
        assert_eq!(image_proof(&flat, 100, 100)["nonuniform"], false);
        let varied: Vec<_> = (0..10000).flat_map(|n| [(n % 256) as u8; 4]).collect();
        assert_eq!(image_proof(&varied, 100, 100)["nonuniform"], true);
    }
}
