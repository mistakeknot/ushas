//! CPU frame-loop observations, deliberately not GPU or presented FPS.
use serde_json::{json, Value};

/// Inspect the scene area, excluding the UI header and window edges. A flat
/// colored image has channel variation but is still missing scene content.
pub fn image_proof(rgba: &[u8], width: u32, height: u32) -> Value {
    let pixel_count = (width as usize).checked_mul(height as usize);
    if width == 0 || height == 0 || pixel_count.and_then(|n| n.checked_mul(4)) != Some(rgba.len()) {
        return json!({"nonuniform":false,"error":"invalid dimensions or RGBA length"});
    }
    let pixel_count = pixel_count.unwrap();
    let alpha_zero_pixels = rgba.chunks_exact(4).filter(|pixel| pixel[3] == 0).count();
    let opaque_pixels = rgba.chunks_exact(4).filter(|pixel| pixel[3] == 255).count();
    let all_zero_rgba = rgba.iter().all(|byte| *byte == 0);
    let capture_error = if all_zero_rgba {
        Some("all_zero_rgba_readback")
    } else if alpha_zero_pixels == pixel_count {
        Some("zero_alpha_readback")
    } else {
        None
    };
    let mut colors = std::collections::BTreeSet::new();
    let (mut low, mut high) = (255u8, 0u8);
    for y in ((height / 5)..(height * 9 / 10)).step_by(2) {
        for x in ((width / 10)..(width * 9 / 10)).step_by(2) {
            let offset = ((y as usize * width as usize) + x as usize) * 4;
            let pixel = &rgba[offset..offset + 4];
            // RGB bytes with zero alpha cannot prove visible scene content.
            if pixel[3] == 0 {
                continue;
            }
            colors.insert([pixel[0], pixel[1], pixel[2]]);
            let luminance =
                ((u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])) / 3) as u8;
            low = low.min(luminance);
            high = high.max(luminance);
        }
    }
    json!({"nonuniform":capture_error.is_none() && colors.len() > 32 && high.saturating_sub(low) > 32,
        "alpha_zero_pixels":alpha_zero_pixels,"opaque_fraction":opaque_pixels as f64 / pixel_count as f64,
        "all_zero_rgba":all_zero_rgba,"capture_error":capture_error,
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

    #[test]
    fn zero_alpha_readback_is_invalid_even_with_varied_rgb() {
        let empty = vec![0u8; 100 * 100 * 4];
        let proof = image_proof(&empty, 100, 100);
        assert_eq!(proof["alpha_zero_pixels"], 10000);
        assert_eq!(proof["all_zero_rgba"], true);
        assert_eq!(proof["nonuniform"], false);
        let transparent: Vec<_> = (0..10000)
            .flat_map(|n| [(n % 256) as u8, (n % 251) as u8, (n % 241) as u8, 0])
            .collect();
        assert_eq!(image_proof(&transparent, 100, 100)["nonuniform"], false);
        assert_eq!(image_proof(&transparent, 100, 100)["all_zero_rgba"], false);
    }

    #[test]
    fn validates_full_rgba_length_and_reports_opaque_coverage() {
        let opaque: Vec<_> = (0..10000)
            .flat_map(|n| [(n % 256) as u8, (n % 251) as u8, (n % 241) as u8, 255])
            .collect();
        let proof = image_proof(&opaque, 100, 100);
        assert_eq!(proof["opaque_fraction"], 1.0);
        assert_eq!(proof["nonuniform"], true);
        assert_eq!(
            image_proof(&opaque[..opaque.len() - 1], 100, 100)["nonuniform"],
            false
        );
        assert_eq!(image_proof(&[], 0, 100)["nonuniform"], false);
    }
}
