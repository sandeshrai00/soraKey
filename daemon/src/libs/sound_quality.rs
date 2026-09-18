use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// Resamples interleaved PCM samples from `from_rate` to `to_rate` using a
/// sinc resampler (rubato). Returns the input unchanged if the rates match.
///
/// Memory: streams chunk-by-chunk with reused scratch buffers and
/// interleaves straight into a pre-sized output — only ~2×1024×channels
/// scratch is ever live, instead of three full-size buffers (≈3× the file).
pub fn resample_interleaved(
    samples: &[f32],
    channels: u16,
    from_rate: u32,
    to_rate: u32,
) -> Result<Vec<f32>, String> {
    if from_rate == to_rate || samples.is_empty() {
        return Ok(samples.to_vec());
    }

    let channels = channels.max(1) as usize;
    let frame_count = samples.len() / channels;

    let params = SincInterpolationParameters {
        sinc_len: 64,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Cubic,
        oversampling_factor: 32,
        window: WindowFunction::BlackmanHarris2,
    };

    let chunk_size = 1024;
    // Max rate ratio the resampler accepts: 5.0 covers every standard
    // up-conversion (11025→48000 is 4.35x, 22050→48000 is 2.17x). The old
    // 2.0 cap made those fail into the silent wrong-rate fallback below
    // (input returned unconverted but tagged with the new rate).
    let mut resampler = SincFixedIn::<f32>::new(
        (to_rate as f64) / (from_rate as f64),
        5.0,
        params,
        chunk_size,
        channels,
    )
    .map_err(|e| format!("Failed to create resampler: {}", e))?;

    // rubato's chunked `process()` already returns output that is correctly
    // time-aligned per call (verified empirically: an impulse at input frame
    // N lands at output frame `N * to_rate/from_rate` with no extra shift).
    // `Resampler::output_delay()` describes filter latency for continuous
    // streaming use and is NOT an offset to skip from this buffered output.
    //
    // What *is* needed: the final chunk is zero-padded up to `chunk_size`
    // frames (rubato requires fixed-size input), so its output must be
    // truncated to the real (non-padded) length - otherwise the padded tail
    // leaks a sinc-smeared artifact into the resampled audio.
    let expected_frame_count =
        (((frame_count as f64) * (to_rate as f64)) / (from_rate as f64)).round() as usize;
    let mut out = Vec::with_capacity(expected_frame_count * channels + chunk_size * channels);
    // Reused input scratch: deinterleaved straight from `samples`, no full copy.
    let mut scratch: Vec<Vec<f32>> = vec![vec![0.0; chunk_size]; channels];
    let mut pos = 0;

    while pos < frame_count {
        let remaining = frame_count - pos;
        let this_chunk = remaining.min(chunk_size);

        for (c, row) in scratch.iter_mut().enumerate() {
            for i in 0..this_chunk {
                row[i] = samples[(pos + i) * channels + c];
            }
            // Zero the tail (leftover from the previous chunk); rubato
            // requires fixed-size input frames.
            row[this_chunk..].fill(0.0);
        }

        match resampler.process(&scratch, None) {
            Ok(resampled_chunks) => {
                let got = resampled_chunks.first().map(|c| c.len()).unwrap_or(0);
                for f in 0..got {
                    for ch in resampled_chunks.iter() {
                        out.push(ch[f]);
                    }
                }
            }
            Err(e) => {
                return Err(format!("Resample chunk failed: {}", e));
            }
        }

        pos += this_chunk;
    }

    // Truncate to the length implied by the real (non-padded) input frame
    // count (truncating samples to a multiple of channels == truncating frames).
    out.truncate(expected_frame_count * channels);

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_upsamples_mono_sine_to_expected_length() {
        let from_rate = 44_100;
        let to_rate = 48_000;
        let duration_secs = 1.0;
        let frame_count = (from_rate as f32 * duration_secs) as usize;

        let samples: Vec<f32> = (0..frame_count)
            .map(|i| {
                let t = (i as f32) / (from_rate as f32);
                (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect();

        let resampled = resample_interleaved(&samples, 1, from_rate, to_rate)
            .expect("test resample must succeed");

        // Length must match the real (non-padded) input frame count scaled
        // by the rate ratio, exactly (within rounding) - not just "close to
        // it within a chunk's worth of slack", which would let a padded,
        // untruncated tail slip through undetected.
        let expected_len =
            ((frame_count as f64) * (to_rate as f64) / (from_rate as f64)).round() as usize;
        let diff = (resampled.len() as i64 - expected_len as i64).unsigned_abs() as usize;
        assert!(
            diff <= 2,
            "resampled length {} too far from expected {} (tail not truncated?)",
            resampled.len(),
            expected_len
        );
    }

    #[test]
    fn resample_does_not_leak_padded_tail_artifact() {
        // Use an input length that is NOT a multiple of the internal chunk
        // size (1024) so the final chunk is zero-padded - this is exactly
        // the case where an untruncated resampler leaks sinc-smeared
        // padding into the output tail.
        let from_rate = 44_100;
        let to_rate = 48_000;
        let frame_count = 1024 * 3 + 200; // partial final chunk

        let samples: Vec<f32> = (0..frame_count)
            .map(|i| {
                let t = (i as f32) / (from_rate as f32);
                (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect();

        let resampled = resample_interleaved(&samples, 1, from_rate, to_rate)
            .expect("test resample must succeed");

        let expected_len =
            ((frame_count as f64) * (to_rate as f64) / (from_rate as f64)).round() as usize;
        let diff = (resampled.len() as i64 - expected_len as i64).unsigned_abs() as usize;
        assert!(
            diff <= 2,
            "resampled length {} too far from expected {} for non-chunk-aligned input (tail not truncated?)",
            resampled.len(),
            expected_len
        );
    }

    #[test]
    fn resample_preserves_impulse_timing() {
        // An impulse at a known frame should land at the equivalent
        // rate-scaled frame in the output, with no extra shift from the
        // sinc filter's internal latency leaking into buffered (non-streaming)
        // output.
        let from_rate = 44_100;
        let to_rate = 48_000;
        let impulse_frame = 1000;
        let frame_count = 4096;

        let mut samples = vec![0.0f32; frame_count];
        samples[impulse_frame] = 1.0;

        let resampled = resample_interleaved(&samples, 1, from_rate, to_rate)
            .expect("test resample must succeed");

        let expected_peak_frame =
            ((impulse_frame as f64) * (to_rate as f64) / (from_rate as f64)).round() as usize;

        let peak_frame = resampled
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.abs().partial_cmp(&b.abs()).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);

        let diff = (peak_frame as i64 - expected_peak_frame as i64).unsigned_abs() as usize;
        assert!(
            diff <= 16,
            "peak at frame {} too far from expected frame {} (filter delay not compensated?)",
            peak_frame,
            expected_peak_frame
        );
    }

    #[test]
    fn resample_skips_when_rates_match() {
        let samples = vec![0.1, 0.2, 0.3, 0.4];
        let resampled = resample_interleaved(&samples, 2, 44_100, 44_100)
            .expect("same-rate resample must succeed");
        assert_eq!(resampled, samples);
    }
}
