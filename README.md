# WIP

# Otocap Core

The core library for the Otocap Audio Recorder. This crate handles the heavy lifting of audio recording, including real-time audio capturing, signal processing, and multi-format encoding.

## Features
- Thread-safe audio processing pipeline.
- Cross-platform audio recording using `cpal`.
- Integrated audio encoding support for multiple formats: WAV, FLAC, MP3, OGG/Opus.
- Sound processing features like high-pass filtering and noise reduction via `nnnoiseless` and `webrtc-audio-processing`.
