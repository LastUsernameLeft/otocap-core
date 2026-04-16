use crate::{CaptureOptions, ProcessingMode};
use crate::devices::get_input_device;
use crate::encoder::AudioEncoder;
use crate::processor::AudioProcessor;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecorderError {
    #[error("Device error: {0}")]
    DeviceAuth(String),
    #[error("Stream configuration error: {0}")]
    StreamConfigError(#[from] cpal::BuildStreamError),
    #[error("Stream play error: {0}")]
    StreamPlayError(#[from] cpal::PlayStreamError),
    #[error("Encoder error: {0}")]
    Encoder(#[from] crate::encoder::EncoderError),
    #[error("Device extraction error: {0}")]
    DeviceExtraction(#[from] crate::devices::DeviceError),
    #[error("Unsupported Sample Format: {0}")]
    UnsupportedFormat(String),
}

pub struct ActiveRecording {
    stop_signal: Arc<AtomicBool>,
    stream: cpal::Stream,
    // Keep handle to background thread for joining
    process_handle: Option<thread::JoinHandle<()>>,
}

impl ActiveRecording {
    pub fn stop(mut self) -> Result<(), RecorderError> {
        self.stop_signal.store(true, Ordering::SeqCst);
        self.stream.pause().ok();
        
        // Wait for the processing thread to finish flushing remaining buffer
        if let Some(handle) = self.process_handle.take() {
            let _ = handle.join();
        }
        
        Ok(())
    }
}

pub fn start_recording<P: AsRef<std::path::Path>>(
    output_path: P,
    options: CaptureOptions,
) -> Result<ActiveRecording, RecorderError> {
    let device = get_input_device(options.device_name.as_deref())?;
    
    // We try to stick with 48kHz / 1 channel internally as it pairs nicely with WebRTC
    // but the device config might be different. 
    let config = device.default_input_config().map_err(|e| RecorderError::DeviceAuth(e.to_string()))?;
    
    let sample_format = config.sample_format();
    let stream_config: StreamConfig = config.into();
    
    let channels = stream_config.channels;
    let sample_rate = stream_config.sample_rate.0;

    // We will use real audio sizes later. 480 is the WebRTC sweet spot (10ms @ 48kHz)
    let buffer_capacity = sample_rate as usize * channels as usize * 5; // 5 seconds of buffer
    let (mut producer, mut consumer) = RingBuffer::<i16>::new(buffer_capacity);

    let stop_signal = Arc::new(AtomicBool::new(false));
    let stop_thread = stop_signal.clone();
    
    let mut encoder = AudioEncoder::new(output_path, sample_rate, channels)?;
    let mut processor = AudioProcessor::new(options.processing_mode, options.high_pass_filter);

    let process_handle = thread::spawn(move || {
        let mut local_buffer = vec![0i16; 480 * channels as usize];
        
        loop {
            let stopped = stop_thread.load(Ordering::Relaxed);
            let available = consumer.slots();

            if available >= local_buffer.len() {
                if let Ok(chunk) = consumer.read_chunk(local_buffer.len()) {
                    let mut data = chunk.into_iter().collect::<Vec<i16>>();
                    processor.process_frame(&mut data);
                    
                    if let Err(e) = encoder.write_samples(&data) {
                        eprintln!("Encoding failed: {}", e);
                    }
                }
            } else if stopped {
                // If we've been told to stop and there are no full chunks left, exit.
                break;
            } else {
                thread::sleep(Duration::from_millis(5));
            }
        }
        
        if let Err(e) = encoder.finalize() {
            eprintln!("Failed to finalize encoding: {}", e);
        }
    });

    let err_fn = |err| eprintln!("An error occurred on the audio stream: {}", err);

    let stream = match sample_format {
        SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _: &_| {
                for &sample in data {
                    let _ = producer.push(sample);
                }
            },
            err_fn,
            None,
        )?,
        SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _: &_| {
                for &sample in data {
                    let val = (sample * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                    let _ = producer.push(val);
                }
            },
            err_fn,
            None,
        )?,
        format => return Err(RecorderError::UnsupportedFormat(format.to_string())),
    };

    stream.play()?;

    Ok(ActiveRecording {
        stop_signal,
        stream,
        process_handle: Some(process_handle),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recorder_options_default() {
        let opts = CaptureOptions::default();
        assert_eq!(opts.device_name, None);
        assert_eq!(opts.processing_mode, ProcessingMode::Standard);
        assert_eq!(opts.high_pass_filter, true);
    }
}
