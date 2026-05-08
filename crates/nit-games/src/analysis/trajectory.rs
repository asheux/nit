//! Bucketed cooperation-rate trajectories used in random-match charts.

pub(super) struct TrajectoryData {
    pub a_rates: Vec<f64>,
    pub b_rates: Vec<f64>,
    pub starts: Vec<u32>,
    pub ends: Vec<u32>,
}

impl TrajectoryData {
    fn empty() -> Self {
        Self {
            a_rates: Vec::new(),
            b_rates: Vec::new(),
            starts: Vec::new(),
            ends: Vec::new(),
        }
    }
}

/// Bins per-round outcome bytes into `samples` equal-width buckets and
/// reports the per-side cooperation rate within each bucket. Bucket
/// `i` covers rounds `[i*total/samples, (i+1)*total/samples)`.
pub(super) fn build_trajectory(outcomes: &[u8], samples: usize) -> TrajectoryData {
    let total = outcomes.len();
    if total == 0 {
        return TrajectoryData::empty();
    }
    let samples = samples.min(total).max(1);
    let mut a_counts = vec![0u32; samples];
    let mut b_counts = vec![0u32; samples];
    let mut bucket_counts = vec![0u32; samples];
    for (idx, &byte) in outcomes.iter().enumerate() {
        let bucket = idx * samples / total;
        bucket_counts[bucket] += 1;
        match byte {
            b'0' => {
                a_counts[bucket] += 1;
                b_counts[bucket] += 1;
            }
            b'1' => a_counts[bucket] += 1,
            b'2' => b_counts[bucket] += 1,
            _ => {}
        }
    }
    let mut a_rates = Vec::with_capacity(samples);
    let mut b_rates = Vec::with_capacity(samples);
    let mut starts = Vec::with_capacity(samples);
    let mut ends = Vec::with_capacity(samples);
    for bucket in 0..samples {
        let start = (bucket * total / samples) as u32 + 1;
        let end = ((bucket + 1) * total / samples) as u32;
        let window = bucket_counts[bucket].max(1) as f64;
        a_rates.push(a_counts[bucket] as f64 / window);
        b_rates.push(b_counts[bucket] as f64 / window);
        starts.push(start);
        ends.push(end);
    }
    TrajectoryData {
        a_rates,
        b_rates,
        starts,
        ends,
    }
}
