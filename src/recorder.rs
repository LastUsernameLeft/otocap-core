use crate::CaptureOptions;
use crate::devices::get_input_device;
use crate::encoder::AudioEncoder;
use crate::processor::AudioProcessor;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use rtrb::RingBuffer;
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
    pause_signal: Arc<AtomicBool>,
    stream: cpal::Stream,
    pub samples_rx: tokio::sync::broadcast::Receiver<Vec<f32>>,
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

    pub fn pause(&self) {
        self.pause_signal.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.pause_signal.store(false, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.pause_signal.load(Ordering::SeqCst)
    }
}

impl Drop for ActiveRecording {
    fn drop(&mut self) {
        // Signal thread to stop
        self.stop_signal.store(true, Ordering::SeqCst);
        
        // Pause stream to stop callbacks
        let _ = self.stream.pause();
        
        // Wait for thread to finish if it hasn't been joined yet
        if let Some(handle) = self.process_handle.take() {
            let _ = handle.join();
        }
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
    let pause_signal = Arc::new(AtomicBool::new(false));
    let pause_thread = pause_signal.clone();
    
    let (samples_tx, samples_rx) = tokio::sync::broadcast::channel::<Vec<f32>>(100);
    
    let mut encoder = AudioEncoder::new(output_path, sample_rate, channels)?;
    let mut processor = AudioProcessor::with_sample_rate(options.processing_mode, options.high_pass_filter, sample_rate, channels);

    let process_handle = thread::spawn(move || {
        let chunk_size = 480 * channels as usize;
        let _local_buffer = vec![0i16; chunk_size];
        
        while !stop_thread.load(Ordering::Relaxed) || !consumer.is_empty() {
            let available = consumer.slots();
            if available >= chunk_size {
                if let Ok(chunk) = consumer.read_chunk(chunk_size) {
                    let mut data = chunk.into_iter().collect::<Vec<i16>>();
                    if !pause_thread.load(Ordering::Relaxed) {
                        processor.process_frame(&mut data);
                        let gui_samples: Vec<f32> = data.iter().step_by(channels as usize * 4).map(|&s| s as f32 / i16::MAX as f32).collect();
                        let _ = samples_tx.send(gui_samples);
                        if let Err(e) = encoder.write_samples(&data) {
                            eprintln!("Encoding failed: {}", e);
                        }
                    }
                }
            } else if stop_thread.load(Ordering::Relaxed) && available > 0 {
                // Drain the remaining samples even if less than a full chunk
                if let Ok(chunk) = consumer.read_chunk(available) {
                    let mut data = chunk.into_iter().collect::<Vec<i16>>();
                    if !pause_thread.load(Ordering::Relaxed) {
                        // We might not be able to process a partial frame with WebRTC/RNNoise correctly
                        // but we can still write it to the encoder (especially WAV)
                        if let Err(e) = encoder.write_samples(&data) {
                            eprintln!("Encoding failed: {}", e);
                        }
                    }
                }
            } else if stop_thread.load(Ordering::Relaxed) && available == 0 {
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
        pause_signal,
        stream,
        samples_rx,
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
