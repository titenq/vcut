# vcut

Automatic video cutter that splits videos at silence points using FFmpeg.

## Prerequisites

- **Rust** (2024 edition or later) - [Install Rust](https://www.rust-lang.org/tools/install)
- **FFmpeg** - [Install FFmpeg](https://ffmpeg.org/download.html)
- **FFprobe** (bundled with FFmpeg) - [Install FFmpeg](https://ffmpeg.org/download.html)

> **Note:** vcut only works on **Linux**.

## Installation

```bash
git clone git@github.com:titenq/vcut.git
cd vcut
cargo build --release
```

The binary will be at `target/release/vcut`.

## Usage

```bash
vcut <video_file>
```

Example:

```bash
vcut video.mp4
```

vcut reads the `vcut.json` config file from the current directory, detects silence intervals in the video, and splits it into segments, cutting at silence points whenever possible.

Output files are named `<original_name>_001.<ext>`, `<original_name>_002.<ext>`, etc.

## Configuration

Edit `vcut.json` in the project root:

```json
{
  "min_segment": 60,
  "max_segment": 180,
  "silence_threshold": -30,
  "min_silence": 0.5
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `min_segment` | number | *(required)* | Minimum segment duration in seconds |
| `max_segment` | number | *(required)* | Maximum segment duration in seconds |
| `silence_threshold` | number | `-30` | Silence detection threshold in dB |
| `min_silence` | number | `0.5` | Minimum silence duration to be detected (seconds) |

## Supported Formats

Any video format supported by FFmpeg: mp4, mkv, avi, mov, webm, flv, wmv, and more.

## License

MIT
