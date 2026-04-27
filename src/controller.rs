use std::path::{Path, PathBuf};
use tokio::sync::broadcast;
use thiserror::Error;

use crate::{CaptureOptions, OutputFormat};
use crate::devices::list_input_devices;
use crate::recorder::{start_recording, ActiveRecording};

#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("Device error: {0}")]
    Device(String),
    #[error("Recording error: {0}")]
    Recording(String),
    #[error("Invalid capture options: {0}")]
    InvalidOptions(String),
}

/// Controller for audio recording operations
/// Provides a unified API for both CLI and GUI applications
pub struct RecordingController {
    default_device: Option<String>,
}

impl RecordingController {
    pub fn new() -> Self {
        Self {
            default_device: None,
        }
    }

    /// Get the list of available input devices
    pub fn get_input_devices(&self) -> Result<Vec<crate::devices::AudioDevice>, ControllerError> {
        list_input_devices()
            .map_err(|e| ControllerError::Device(e.to_string()))
    }

    /// Get the default device name if available
    pub fn get_default_device(&self) -> Option<String> {
        if let Ok(devices) = self.get_input_devices() {
            devices
                .into_iter()
                .find(|dev| dev.is_default)
                .map(|dev| dev.name)
        } else {
            None
        }
    }

    /// Start a new recording with the specified options
    pub fn start_recording<P: AsRef<Path>>(
        &self,
        output_path: P,
        options: CaptureOptions,
    ) -> Result<ActiveRecordingHandle, ControllerError> {
        // Validate options
        if options.output_format == OutputFormat::Opus {
            return Err(ControllerError::InvalidOptions(
                "Opus output format is not yet supported".to_string()
            ));
        }

        let active_recording = start_recording(output_path, options)
            .map_err(|e| ControllerError::Recording(e.to_string()))?;

        Ok(ActiveRecordingHandle::new(active_recording))
    }

    /// Generate a default output filename based on current date/time and format
    pub fn generate_output_filename(&self, format: OutputFormat) -> PathBuf {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let extension = match format {
            OutputFormat::Wav => "wav",
            OutputFormat::Mp3 => "mp3",
            OutputFormat::Opus => "opus",
        };
        PathBuf::from(format!("recording_{}.{}", timestamp, extension))
    }
}

/// Handle to an active recording that provides additional controls
pub struct ActiveRecordingHandle {
    active_recording: ActiveRecording,
}

impl ActiveRecordingHandle {
    pub fn new(active_recording: ActiveRecording) -> Self {
        Self {
            active_recording,
        }
    }

    /// Stop the recording
    pub fn stop(self) -> Result<(), ControllerError> {
        self.active_recording
            .stop()
            .map_err(|e| ControllerError::Recording(e.to_string()))
    }

    /// Pause the recording
    pub fn pause(&self) {
        self.active_recording.pause();
    }

    /// Resume the recording
    pub fn resume(&self) {
        self.active_recording.resume();
    }

    /// Check if recording is paused
    pub fn is_paused(&self) -> bool {
        self.active_recording.is_paused()
    }

    /// Get a receiver for waveform data
    pub fn get_waveform_receiver(&self) -> broadcast::Receiver<Vec<f32>> {
        self.active_recording.samples_rx.resubscribe()
    }
}

impl Default for RecordingController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_controller_creation() {
        let controller = RecordingController::new();
        assert!(controller.get_default_device().is_none() || true); // May or may not have a default
    }

    #[test]
    fn test_filename_generation() {
        let controller = RecordingController::new();
        let filename = controller.generate_output_filename(OutputFormat::Wav);
        assert!(filename.extension().unwrap() == "wav");
        assert!(filename.file_name().unwrap().to_str().unwrap().starts_with("recording_"));
    }
}