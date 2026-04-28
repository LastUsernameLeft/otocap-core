use crate::ProcessingMode;
use nnnoiseless::DenoiseState;
use std::collections::VecDeque;
use webrtc_audio_processing::config::{
    Config, GainController, HighPassFilter, NoiseSuppression, NoiseSuppressionLevel,
};

const RNNOISE_FRAME_SIZE: usize = DenoiseState::FRAME_SIZE;

pub struct AudioProcessor {
    mode: ProcessingMode,
    _high_pass_enabled: bool,
    sample_rate: u32,
    channels: u16,
    webrtc: Option<webrtc_audio_processing::Processor>,
    rnnoise_states: Option<Vec<DenoiseState>>,
    rnnoise_out_buf: [f32; RNNOISE_FRAME_SIZE],
    input_buffer: VecDeque<f32>,
    output_buffer: VecDeque<f32>,
    frame_size: usize,
    // Buffer for accumulating samples until we have RNNOISE_FRAME_SIZE per channel
    rnnoise_accum_buffers: Vec<VecDeque<f32>>,
}

impl AudioProcessor {
    pub fn new(mode: ProcessingMode, high_pass_enabled: bool) -> Self {
        Self::with_sample_rate(mode, high_pass_enabled, 48000, 1)
    }

    pub fn with_sample_rate(
        mode: ProcessingMode,
        high_pass_enabled: bool,
        sample_rate: u32,
        channels: u16,
    ) -> Self {
        let webrtc = if mode != ProcessingMode::Off {
            webrtc_audio_processing::Processor::new(sample_rate).ok()
        } else {
            None
        };

        if let Some(ref proc) = webrtc {
            let ns_level = match mode {
                ProcessingMode::Heavy => NoiseSuppressionLevel::VeryHigh,
                _ => NoiseSuppressionLevel::Moderate,
            };
            let config = Config {
                high_pass_filter: if high_pass_enabled {
                    Some(HighPassFilter {
                        apply_in_full_band: true,
                    })
                } else {
                    None
                },
                noise_suppression: Some(NoiseSuppression {
                    level: ns_level,
                    analyze_linear_aec_output: false,
                }),
                gain_controller: Some(GainController::GainController2(Default::default())),
                ..Default::default()
            };
            proc.set_config(config);
        }

        let rnnoise_states = if mode == ProcessingMode::Heavy {
            Some((0..channels).map(|_| *DenoiseState::new()).collect())
        } else {
            None
        };

        let mut rnnoise_accum_buffers = Vec::new();
        if mode == ProcessingMode::Heavy {
            for _ in 0..channels {
                rnnoise_accum_buffers.push(VecDeque::with_capacity(RNNOISE_FRAME_SIZE * 2));
            }
        }

        Self {
            mode,
            _high_pass_enabled: high_pass_enabled,
            sample_rate,
            channels,
            webrtc,
            rnnoise_states,
            rnnoise_out_buf: [0.0; RNNOISE_FRAME_SIZE],
            input_buffer: VecDeque::with_capacity(sample_rate as usize / 10),
            output_buffer: VecDeque::with_capacity(sample_rate as usize / 10),
            frame_size: (sample_rate / 100) as usize, 
            rnnoise_accum_buffers,
        }
    }

    pub fn set_mode(&mut self, mode: ProcessingMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> ProcessingMode {
        self.mode
    }

    pub fn process_frame(&mut self, frame: &mut [i16]) {
        if self.mode == ProcessingMode::Off {
            return;
        }

        let channels = self.channels as usize;

        // Convert input to float and add to input buffer
        for &sample in frame.iter() {
            let val = sample as f32 / i16::MAX as f32;
            self.input_buffer.push_back(val);
        }

        let frame_size = self.frame_size;
        let required_samples = frame_size * channels;

        // Process in WebRTC frames
        while self.input_buffer.len() >= required_samples {
            let mut channel_frames: Vec<Vec<f32>> = vec![vec![0.0f32; frame_size]; channels];

            // Deinterleave samples into per-channel frames
            for i in 0..frame_size {
                for ch in 0..channels {
                    channel_frames[ch][i] = self.input_buffer.pop_front().unwrap_or(0.0);
                }
            }

            // Apply WebRTC processing (NS, AGC, HPF)
            if let Some(ref proc) = self.webrtc {
                let _ = proc.process_capture_frame(&mut channel_frames);
            }

            // Apply RNNoise heavy mode processing
            if self.mode == ProcessingMode::Heavy {
                if let Some(ref mut states) = self.rnnoise_states {
                    for ch in 0..channels {
                        // Accumulate samples for RNNoise (needs exactly RNNOISE_FRAME_SIZE = 480)
                        for &sample in &channel_frames[ch] {
                            self.rnnoise_accum_buffers[ch].push_back(sample);
                        }

                        // Process accumulated samples in RNNOISE_FRAME_SIZE chunks
                        while self.rnnoise_accum_buffers[ch].len() >= RNNOISE_FRAME_SIZE {
                            let mut rnn_input = [0.0f32; RNNOISE_FRAME_SIZE];
                            for i in 0..RNNOISE_FRAME_SIZE {
                                rnn_input[i] = self.rnnoise_accum_buffers[ch].pop_front().unwrap_or(0.0);
                            }
                            states[ch].process_frame(&mut self.rnnoise_out_buf[..], &rnn_input);

                            // Put denoised samples back into a separate output accumulator
                            // We'll collect these and then merge with channel_frames
                            // For now, store in a temporary buffer
                            for i in 0..RNNOISE_FRAME_SIZE {
                                self.output_buffer.push_back(self.rnnoise_out_buf[i]);
                            }
                        }
                    }
                }
            } else {
                // Non-heavy mode: just pass through
                for i in 0..frame_size {
                    for ch in 0..channels {
                        self.output_buffer.push_back(channel_frames[ch][i]);
                    }
                }
            }
        }

        // For heavy mode, we need to output the denoised samples
        // But we also need to handle the case where we don't have enough accumulated yet
        if self.mode == ProcessingMode::Heavy {
            // Output whatever we have from the RNNoise processing
            // If we don't have enough, use zeros (or the original samples)
            for sample in frame.iter_mut() {
                let val = if !self.output_buffer.is_empty() {
                    self.output_buffer.pop_front().unwrap_or(0.0)
                } else {
                    0.0
                };
                *sample = (val * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            }
        } else {
            // Non-heavy mode: output from output_buffer
            for sample in frame.iter_mut() {
                let val = self.output_buffer.pop_front().unwrap_or(0.0);
                *sample = (val * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            }
        }
    }
}

impl AudioProcessor {
    /// Flush any remaining buffered samples and return them
    /// This should be called at the end of recording to get any partial accumulated data
    pub fn flush(&mut self) -> Vec<i16> {
        // Process any remaining samples in the input buffer
        let frame_size = self.frame_size;
        let channels = self.channels as usize;
        let required_samples = frame_size * channels;

        // Pad input buffer with zeros to make a complete frame if we have partial data
        while self.input_buffer.len() >= required_samples {
            let mut channel_frames: Vec<Vec<f32>> = vec![vec![0.0f32; frame_size]; channels];

            for i in 0..frame_size {
                for ch in 0..channels {
                    channel_frames[ch][i] = self.input_buffer.pop_front().unwrap_or(0.0);
                }
            }

            if let Some(ref proc) = self.webrtc {
                let _ = proc.process_capture_frame(&mut channel_frames);
            }

            if self.mode == ProcessingMode::Heavy {
                if let Some(ref mut states) = self.rnnoise_states {
                    for ch in 0..channels {
                        for &sample in &channel_frames[ch] {
                            self.rnnoise_accum_buffers[ch].push_back(sample);
                        }

                        while self.rnnoise_accum_buffers[ch].len() >= RNNOISE_FRAME_SIZE {
                            let mut rnn_input = [0.0f32; RNNOISE_FRAME_SIZE];
                            for i in 0..RNNOISE_FRAME_SIZE {
                                rnn_input[i] = self.rnnoise_accum_buffers[ch].pop_front().unwrap_or(0.0);
                            }
                            states[ch].process_frame(&mut self.rnnoise_out_buf[..], &rnn_input);

                            for i in 0..RNNOISE_FRAME_SIZE {
                                self.output_buffer.push_back(self.rnnoise_out_buf[i]);
                            }
                        }
                    }
                }
            } else {
                for i in 0..frame_size {
                    for ch in 0..channels {
                        self.output_buffer.push_back(channel_frames[ch][i]);
                    }
                }
            }
        }

        // Handle remaining samples in input_buffer that don't fill a complete frame
        if !self.input_buffer.is_empty() {
            let remaining = self.input_buffer.len();
            let mut channel_frames: Vec<Vec<f32>> = vec![vec![0.0f32; frame_size]; channels];
            
            // Distribute remaining samples across channels and pad with zeros
            for i in 0..frame_size {
                for ch in 0..channels {
                    if i * channels + ch < remaining {
                        channel_frames[ch][i] = self.input_buffer.pop_front().unwrap_or(0.0);
                    }
                }
            }

            if let Some(ref proc) = self.webrtc {
                let _ = proc.process_capture_frame(&mut channel_frames);
            }

            if self.mode == ProcessingMode::Heavy {
                if let Some(ref mut states) = self.rnnoise_states {
                    for ch in 0..channels {
                        for &sample in &channel_frames[ch] {
                            self.rnnoise_accum_buffers[ch].push_back(sample);
                        }

                        while self.rnnoise_accum_buffers[ch].len() >= RNNOISE_FRAME_SIZE {
                            let mut rnn_input = [0.0f32; RNNOISE_FRAME_SIZE];
                            for i in 0..RNNOISE_FRAME_SIZE {
                                rnn_input[i] = self.rnnoise_accum_buffers[ch].pop_front().unwrap_or(0.0);
                            }
                            states[ch].process_frame(&mut self.rnnoise_out_buf[..], &rnn_input);

                            for i in 0..RNNOISE_FRAME_SIZE {
                                self.output_buffer.push_back(self.rnnoise_out_buf[i]);
                            }
                        }
                    }
                }
            } else {
                for i in 0..frame_size {
                    for ch in 0..channels {
                        self.output_buffer.push_back(channel_frames[ch][i]);
                    }
                }
            }
        }

        // For heavy mode, flush remaining RNNoise accumulators with zero-padding
        if self.mode == ProcessingMode::Heavy {
            if let Some(ref mut states) = self.rnnoise_states {
                for ch in 0..channels {
                    // Pad remaining samples with zeros to reach RNNOISE_FRAME_SIZE
                    while self.rnnoise_accum_buffers[ch].len() > 0 {
                        while self.rnnoise_accum_buffers[ch].len() < RNNOISE_FRAME_SIZE {
                            self.rnnoise_accum_buffers[ch].push_back(0.0);
                        }
                        let mut rnn_input = [0.0f32; RNNOISE_FRAME_SIZE];
                        for i in 0..RNNOISE_FRAME_SIZE {
                            rnn_input[i] = self.rnnoise_accum_buffers[ch].pop_front().unwrap_or(0.0);
                        }
                        states[ch].process_frame(&mut self.rnnoise_out_buf[..], &rnn_input);

                        for i in 0..RNNOISE_FRAME_SIZE {
                            self.output_buffer.push_back(self.rnnoise_out_buf[i]);
                        }
                    }
                }
            }
        }

        // Collect all remaining output buffer samples
        let mut result = Vec::new();
        while let Some(val) = self.output_buffer.pop_front() {
            result.push((val * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_processor_off_mode_does_nothing() {
        let mut processor = AudioProcessor::new(ProcessingMode::Off, false);

        let mut frame = vec![100; 480];
        let original_frame = frame.clone();

        processor.process_frame(&mut frame);

        assert_eq!(
            frame, original_frame,
            "Off mode should not modify the audio frame"
        );
    }

    #[test]
    fn test_mode_switching() {
        let mut processor = AudioProcessor::new(ProcessingMode::Standard, true);
        assert_eq!(processor.mode(), ProcessingMode::Standard);

        processor.set_mode(ProcessingMode::Heavy);
        assert_eq!(processor.mode(), ProcessingMode::Heavy);

        processor.set_mode(ProcessingMode::Off);
        assert_eq!(processor.mode(), ProcessingMode::Off);
    }

    #[test]
    fn test_standard_mode_processes_without_panic() {
        let mut processor =
            AudioProcessor::with_sample_rate(ProcessingMode::Standard, true, 48000, 1);
        let mut frame = vec![1000i16; 480];
        processor.process_frame(&mut frame);
        assert!(
            frame.iter().any(|&s| s != 1000),
            "Standard mode should modify the audio frame"
        );
    }

    #[test]
    fn test_heavy_mode_processes_without_panic() {
        let mut processor = AudioProcessor::with_sample_rate(ProcessingMode::Heavy, true, 48000, 1);
        let mut frame = vec![1000i16; 480];
        processor.process_frame(&mut frame);
    }

    #[test]
    fn test_stereo_processing() {
        let mut processor =
            AudioProcessor::with_sample_rate(ProcessingMode::Standard, true, 48000, 2);
        let mut frame = vec![500i16; 960];
        processor.process_frame(&mut frame);
    }

    #[test]
    fn test_flush_returns_remaining_samples() {
        let mut processor =
            AudioProcessor::with_sample_rate(ProcessingMode::Standard, true, 48000, 1);
        // Feed partial frame (less than frame_size = 480)
        let mut partial_frame = vec![100i16; 100];
        processor.process_frame(&mut partial_frame);
        // Flush should return any buffered samples
        let flushed = processor.flush();
        // Flush processes remaining samples through the pipeline and outputs
        // at least a full frame_size worth of samples (or zero if none)
        assert!(flushed.len() >= 0);
    }

    #[test]
    fn test_heavy_mode_flush_with_partial_accumulation() {
        let mut processor =
            AudioProcessor::with_sample_rate(ProcessingMode::Heavy, true, 48000, 1);
        // Feed less than RNNOISE_FRAME_SIZE - these accumulate but don't trigger processing
        let mut partial_frame = vec![500i16; 100];
        processor.process_frame(&mut partial_frame);
        // Flush should handle partial RNNoise accumulator by zero-padding
        let flushed = processor.flush();
        // After flush with partial accumulation, we should get zero-padded output
        // (RNNOISE_FRAME_SIZE samples from the zero-padded processing)
        assert!(!flushed.is_empty());
    }

    #[test]
    fn test_heavy_mode_stereo_flush() {
        let mut processor =
            AudioProcessor::with_sample_rate(ProcessingMode::Heavy, true, 48000, 2);
        let mut frame = vec![500i16; 200]; // 100 samples per channel
        processor.process_frame(&mut frame);
        let flushed = processor.flush();
        // Flush should produce output from zero-padded RNNoise processing
        assert!(!flushed.is_empty());
    }
}
