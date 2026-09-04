//! Validation of ordered, same-clock GPU timestamp intervals.
//! This measures elapsed time between boundaries, never a sum of GPU busy time.

#[derive(Debug, PartialEq)]
pub struct Interval {
    pub frame_id: u64,
    pub elapsed_ms: f64,
}

pub fn interval(frame_id: u64, start: u64, end: u64, period_ns: f32) -> Option<Interval> {
    if start == 0 || end == u64::MAX || end <= start || !period_ns.is_finite() || period_ns <= 0.0 {
        return None;
    }
    Some(Interval {
        frame_id,
        elapsed_ms: (end - start) as f64 * f64::from(period_ns) / 1_000_000.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_keeps_encoded_frame_identity_and_converts_nanoseconds() {
        assert_eq!(
            interval(41, 1_000_000, 4_000_000, 1.0),
            Some(Interval {
                frame_id: 41,
                elapsed_ms: 3.0
            }),
        );
    }

    #[test]
    fn overlapping_command_buffers_use_outer_boundaries_not_summed_durations() {
        // Buffers [1, 4] and [2, 5] overlap: the envelope is 4 ms, not 6 ms.
        assert_eq!(
            interval(9, 1_000_000, 5_000_000, 1.0).unwrap().elapsed_ms,
            4.0
        );
    }

    #[test]
    fn invalid_or_unwritten_queries_never_become_samples() {
        for (start, end, period) in [
            (0, 2, 1.0),
            (2, 0, 1.0),
            (2, 2, 1.0),
            (3, 2, 1.0),
            (1, 2, 0.0),
            (1, 2, -1.0),
            (1, 2, f32::NAN),
            (1, 2, f32::INFINITY),
            (1, u64::MAX, 1.0),
        ] {
            assert_eq!(interval(1, start, end, period), None);
        }
    }

    #[test]
    fn subtracts_ticks_before_float_conversion_to_preserve_short_intervals() {
        assert_eq!(
            interval(1, u64::MAX - 10, u64::MAX - 9, 1.0)
                .unwrap()
                .elapsed_ms,
            0.000001
        );
    }
}
