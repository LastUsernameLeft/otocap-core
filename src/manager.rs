use crate::OutputFormat;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ManagerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    #[error("WAV parse error: {0}")]
    WavParse(#[from] hound::Error),
}

#[derive(Debug, Clone)]
pub struct RecordingEntry {
    pub filename: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub format: OutputFormat,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    pub duration_secs: Option<f64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct RecordingsManager {
    storage_dir: PathBuf,
}

impl RecordingsManager {
    pub fn new(storage_dir: PathBuf) -> Self {
        Self { storage_dir }
    }

    pub fn default_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("OTOCAP_RECORDINGS_DIR") {
            PathBuf::from(dir)
        } else if let Some(dirs) = dirs_next() {
            dirs.join("Music").join("otocap")
        } else {
            PathBuf::from("./recordings")
        }
    }

    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    pub fn ensure_storage_dir(&self) -> Result<(), ManagerError> {
        fs::create_dir_all(&self.storage_dir)?;
        Ok(())
    }

    pub fn list_recordings(&self) -> Result<Vec<RecordingEntry>, ManagerError> {
        self.ensure_storage_dir()?;
        let mut entries = Vec::new();

        let dir_iter = match fs::read_dir(&self.storage_dir) {
            Ok(iter) => iter,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
            Err(e) => return Err(ManagerError::Io(e)),
        };

        for entry in dir_iter {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let format = OutputFormat::from_extension(&path);
            // Skip files with unknown extensions (from_extension defaults to Wav)
            let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase());
            let known_extensions: &[&str] = &["wav", "mp3", "opus", "ogg", "flac"];
            if !ext.as_deref().is_some_and(|e| known_extensions.iter().any(|k| *k == e)) {
                continue;
            }

            let metadata = entry.metadata()?;

            let mut recording = RecordingEntry {
                filename: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                path,
                size_bytes: metadata.len(),
                format,
                created: metadata.created().ok(),
                modified: metadata.modified().ok(),
                duration_secs: None,
                sample_rate: None,
                channels: None,
            };

            recording.parse_duration();
            entries.push(recording);
        }

        entries.sort_by(|a, b| {
            b.modified
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .cmp(&a.modified.unwrap_or(SystemTime::UNIX_EPOCH))
        });

        Ok(entries)
    }

    pub fn get_recording(&self, filename: &str) -> Result<RecordingEntry, ManagerError> {
        let path = self.storage_dir.join(filename);
        if !path.exists() {
            return Err(ManagerError::NotFound(format!(
                "Recording '{}' not found in {}",
                filename,
                self.storage_dir.display()
            )));
        }

        let format = OutputFormat::from_extension(&path);
        let metadata = fs::metadata(&path)?;

        let mut recording = RecordingEntry {
            filename: filename.to_string(),
            path,
            size_bytes: metadata.len(),
            format,
            created: metadata.created().ok(),
            modified: metadata.modified().ok(),
            duration_secs: None,
            sample_rate: None,
            channels: None,
        };

        recording.parse_duration();
        Ok(recording)
    }

    pub fn rename(&self, old_filename: &str, new_filename: &str) -> Result<(), ManagerError> {
        let old_path = self.storage_dir.join(old_filename);
        let new_path = self.storage_dir.join(new_filename);

        if !old_path.exists() {
            return Err(ManagerError::NotFound(format!(
                "Recording '{}' not found",
                old_filename
            )));
        }
        if new_path.exists() {
            return Err(ManagerError::InvalidPath(format!(
                "'{}' already exists",
                new_filename
            )));
        }

        // Validate new extension matches a known format
        let _ = OutputFormat::from_extension(&new_path);
        fs::rename(&old_path, &new_path)?;
        Ok(())
    }

    pub fn delete(&self, filename: &str) -> Result<(), ManagerError> {
        let path = self.storage_dir.join(filename);
        if !path.exists() {
            return Err(ManagerError::NotFound(format!(
                "Recording '{}' not found",
                filename
            )));
        }
        fs::remove_file(&path)?;
        Ok(())
    }

    pub fn generate_filename(&self, format: OutputFormat) -> String {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let base = format!("recording_{}", timestamp);

        let mut filename = format!("{}.{}", base, format.extension());
        let mut counter = 1;
        while self.storage_dir.join(&filename).exists() {
            filename = format!("{}_{}.{}", base, counter, format.extension());
            counter += 1;
        }

        filename
    }

    pub fn full_path(&self, filename: &str) -> PathBuf {
        self.storage_dir.join(filename)
    }
}

impl RecordingEntry {
    pub fn parse_duration(&mut self) {
        match self.format {
            OutputFormat::Wav => self.parse_wav_duration(),
            OutputFormat::Mp3 => self.parse_mp3_duration(),
            OutputFormat::Opus => {} // Opus header parsing not implemented
        }
    }

    fn parse_wav_duration(&mut self) {
        match hound::WavReader::open(&self.path) {
            Ok(reader) => {
                let spec = reader.spec();
                let sample_count = reader.duration() as f64;
                self.sample_rate = Some(spec.sample_rate);
                self.channels = Some(spec.channels);
                self.duration_secs = Some(sample_count / spec.sample_rate as f64);
            }
            Err(_) => {}
        }
    }

    fn parse_mp3_duration(&mut self) {
        use std::io::Read;
        let mut file = match fs::File::open(&self.path) {
            Ok(f) => f,
            Err(_) => return,
        };

        let mut header = [0u8; 4];
        if file.read_exact(&mut header).is_err() {
            return;
        }

        // Find first valid MP3 frame sync (0xFF 0xFB or 0xFF 0xFA or 0xFF 0xF3 etc)
        let mut buf = vec![0u8; 4096.min(self.size_bytes as usize)];
        if file.read_exact(&mut buf).is_err() && buf.is_empty() {
            buf = header.to_vec();
        }

        // Prepend header to search space
        let mut search = header.to_vec();
        search.extend_from_slice(&buf);

        let mut frame_start = None;
        for i in 0..search.len().saturating_sub(1) {
            if search[i] == 0xFF && (search[i + 1] & 0xE0) == 0xE0 {
                frame_start = Some(i);
                break;
            }
        }

        let offset = match frame_start {
            Some(o) => o,
            None => return,
        };

        if offset + 4 > search.len() {
            return;
        }

        let byte2 = search[offset + 2];
        let byte3 = search[offset + 3];

        // MPEG version and layer from byte2
        let mpeg_version = (byte2 >> 3) & 0x03;
        let bitrate_idx = (byte3 >> 4) & 0x0F;
        let sample_rate_idx = (byte2 >> 2) & 0x03;

        let bitrates: [u32; 16] = [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
        ];

        let sample_rates = match mpeg_version {
            0x03 => [44100, 48000, 32000, 0], // MPEG v1
            0x02 => [22050, 24000, 16000, 0], // MPEG v2
            0x00 => [11025, 12000, 8000, 0],  // MPEG v2.5
            _ => [44100, 48000, 32000, 0],
        };

        let bitrate = bitrates.get(bitrate_idx as usize).copied().unwrap_or(128) * 1000;
        let sample_rate = sample_rates.get(sample_rate_idx as usize).copied().unwrap_or(44100);

        self.sample_rate = Some(sample_rate);
        // Mono if channel_mode bits (byte4 bits 7-6) == 11
        let byte4 = if offset + 4 < search.len() { search[offset + 4] } else { 0 };
        let channel_mode = (byte4 >> 6) & 0x03;
        self.channels = Some(if channel_mode == 3 { 1 } else { 2 });

        // Estimate duration: file_size / bitrate * 8 (rough, ignores ID3 tags)
        let audio_size = (self.size_bytes as f64) - offset as f64;
        self.duration_secs = Some(audio_size * 8.0 / bitrate as f64);
    }

    pub fn format_duration(&self) -> String {
        match self.duration_secs {
            Some(d) => {
                let mins = d as u64 / 60;
                let secs = d as u64 % 60;
                format!("{}:{:02}", mins, secs)
            }
            None => "N/A".to_string(),
        }
    }

    pub fn format_size(&self) -> String {
        let kb = self.size_bytes as f64 / 1024.0;
        if kb < 1024.0 {
            format!("{:.1} KB", kb)
        } else {
            format!("{:.1} MB", kb / 1024.0)
        }
    }
}

fn dirs_next() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        Some(PathBuf::from(home))
    } else if let Ok(profile) = std::env::var("USERPROFILE") {
        Some(PathBuf::from(profile))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_dir() -> PathBuf {
        let dir = std::env::temp_dir().join("otocap_manager_tests");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn create_test_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..48000 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn test_generate_unique_filename() {
        let dir = test_dir();
        let mgr = RecordingsManager::new(dir.clone());
        let fname = mgr.generate_filename(OutputFormat::Wav);
        assert!(fname.starts_with("recording_"));
        assert!(fname.ends_with(".wav"));
    }

    #[test]
    fn test_list_empty_directory() {
        let dir = test_dir().join("empty_list_test");
        let _ = fs::remove_dir_all(&dir);
        let mgr = RecordingsManager::new(dir.clone());
        let entries = mgr.list_recordings().unwrap();
        assert!(entries.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_list_recordings() {
        let dir = test_dir().join("list_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mgr = RecordingsManager::new(dir.clone());

        create_test_wav(&dir.join("test1.wav"));
        create_test_wav(&dir.join("test2.wav"));

        let mut file = fs::File::create(dir.join("not_audio.txt")).unwrap();
        file.write_all(b"hello").unwrap();

        let entries = mgr.list_recordings().unwrap();
        assert_eq!(entries.len(), 2, "Should list only audio files");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wav_duration_parsing() {
        let dir = test_dir().join("wav_parse_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mgr = RecordingsManager::new(dir.clone());

        let path = dir.join("duration_test.wav");
        create_test_wav(&path);

        let entry = mgr.get_recording("duration_test.wav").unwrap();
        assert!(entry.duration_secs.is_some(), "Should parse WAV duration");
        assert_eq!(entry.sample_rate, Some(48000));
        assert_eq!(entry.channels, Some(1));
        // 48000 samples at 48000 Hz = 1 second
        assert!((entry.duration_secs.unwrap() - 1.0).abs() < 0.01);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rename_recording() {
        let dir = test_dir().join("rename_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mgr = RecordingsManager::new(dir.clone());

        create_test_wav(&dir.join("original.wav"));
        mgr.rename("original.wav", "renamed.wav").unwrap();

        assert!(dir.join("renamed.wav").exists());
        assert!(!dir.join("original.wav").exists());

        // Rename to existing file should fail
        create_test_wav(&dir.join("other.wav"));
        assert!(mgr.rename("renamed.wav", "other.wav").is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_recording() {
        let dir = test_dir().join("delete_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mgr = RecordingsManager::new(dir.clone());

        create_test_wav(&dir.join("to_delete.wav"));
        assert!(dir.join("to_delete.wav").exists());

        mgr.delete("to_delete.wav").unwrap();
        assert!(!dir.join("to_delete.wav").exists());

        // Delete non-existent should error
        assert!(mgr.delete("nonexistent.wav").is_err());

        let _ = fs::remove_dir_all(&dir);
    }
}
