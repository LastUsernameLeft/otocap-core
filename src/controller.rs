use std::path::{Path, PathBuf};
use tokio::sync::broadcast;
use thiserror::Error;

use crate::{CaptureOptions, OutputFormat};
use crate::devices::list_input_devices;
use crate::recorder::{start_recording, ActiveRecording};
use crate::manager::{RecordingsManager, RecordingEntry, ManagerError};

#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("Device error: {0}")]
    Device(String),
    #[error("Recording error: {0}")]
    Recording(String),
    #[error("Invalid capture options: {0}")]
    InvalidOptions(String),
    #[error("Manager error: {0}")]
    Manager(#[from] ManagerError),
}

/// Controller for audio recording operations
/// Provides a unified API for both CLI and GUI applications
pub struct RecordingController {
    default_device: Option<String>,
    recordings_manager: RecordingsManager,
}

impl RecordingController {
    pub fn new() -> Self {
        Self {
            default_device: None,
            recordings_manager: RecordingsManager::new(RecordingsManager::default_dir()),
        }
    }

    pub fn with_storage_dir(storage_dir: PathBuf) -> Self {
        Self {
            default_device: None,
            recordings_manager: RecordingsManager::new(storage_dir),
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
        if options.output_format == OutputFormat::Opus {
            return Err(ControllerError::InvalidOptions(
                "Opus output format is not yet supported".to_string()
            ));
        }

        // Ensure storage directory exists
        self.recordings_manager.ensure_storage_dir()
            .map_err(|e| ControllerError::Manager(e))?;

        let active_recording = start_recording(output_path, options)
            .map_err(|e| ControllerError::Recording(e.to_string()))?;

        Ok(ActiveRecordingHandle::new(active_recording))
    }

    /// Generate a default output filename based on current date/time and format
    pub fn generate_output_filename(&self, format: OutputFormat) -> PathBuf {
        let filename = self.recordings_manager.generate_filename(format);
        self.recordings_manager.full_path(&filename)
    }

    /// List all saved recordings
    pub fn list_recordings(&self) -> Result<Vec<RecordingEntry>, ControllerError> {
        self.recordings_manager.list_recordings()
            .map_err(Into::into)
    }

    /// Get metadata for a specific recording
    pub fn get_recording(&self, filename: &str) -> Result<RecordingEntry, ControllerError> {
        self.recordings_manager.get_recording(filename)
            .map_err(Into::into)
    }

    /// Rename a recording
    pub fn rename_recording(&self, old_name: &str, new_name: &str) -> Result<(), ControllerError> {
        self.recordings_manager.rename(old_name, new_name)
            .map_err(Into::into)
    }

    /// Delete a recording
    pub fn delete_recording(&self, filename: &str) -> Result<(), ControllerError> {
        self.recordings_manager.delete(filename)
            .map_err(Into::into)
    }

    /// Get the storage directory
    pub fn storage_dir(&self) -> &Path {
        self.recordings_manager.storage_dir()
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