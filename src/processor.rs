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
    _sample_rate: u32,
    channels: u16,
    webrtc: Option<webrtc_audio_processing::Processor>,
    rnnoise: Option<Box<DenoiseState>>,
    rnnoise_initialized: bool,
    rnnoise_out_buf: [f32; RNNOISE_FRAME_SIZE],
    input_buffer: VecDeque<f32>,
    output_buffer: VecDeque<f32>,
    frame_size: usize,
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

        let rnnoise = if mode == ProcessingMode::Heavy {
            Some(DenoiseState::new())
        } else {
            None
        };

        Self {
            mode,
            _high_pass_enabled: high_pass_enabled,
            _sample_rate: sample_rate,
            channels,
            webrtc,
            rnnoise,
            rnnoise_initialized: false,
            rnnoise_out_buf: [0.0; RNNOISE_FRAME_SIZE],
            input_buffer: VecDeque::with_capacity(sample_rate as usize / 10),
            output_buffer: VecDeque::with_capacity(sample_rate as usize / 10),
            frame_size: (sample_rate / 100) as usize,
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
        let _samples_per_channel = frame.len() / channels;

        for &sample in frame.iter() {
            let val = sample as f32 / i16::MAX as f32;
            self.input_buffer.push_back(val);
        }

        let frame_size = self.frame_size;
        let required_samples = frame_size * channels;

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

            if self.mode == ProcessingMode::Heavy && frame_size == RNNOISE_FRAME_SIZE {
                if let Some(ref mut rnnoise) = self.rnnoise {
                    for ch in 0..channels {
                        rnnoise.process_frame(&mut self.rnnoise_out_buf[..], &channel_frames[ch]);
                        if self.rnnoise_initialized {
                            channel_frames[ch].copy_from_slice(&self.rnnoise_out_buf[..]);
                        } else {
                            self.rnnoise_initialized = true;
                        }
                    }
                }
            }

            for i in 0..frame_size {
                for ch in 0..channels {
                    let sample = channel_frames[ch][i];
                    self.output_buffer.push_back(sample);
                }
            }
        }

        for sample in frame.iter_mut() {
            let val = self.output_buffer.pop_front().unwrap_or(0.0);
            *sample = (val * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
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
}
