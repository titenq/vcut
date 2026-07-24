use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::Command;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct Config {
    min_segment: f64,
    max_segment: f64,
    #[serde(default = "default_silence_threshold")]
    silence_threshold: f64,
    #[serde(default = "default_min_silence")]
    min_silence: f64,
}

fn default_silence_threshold() -> f64 { -30.0 }
fn default_min_silence() -> f64 { 0.5 }

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SilenceInterval {
    start: f64,
    end: f64,
    duration: f64,
}

fn run_spinner(message: &str, done: Arc<AtomicBool>) {
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut i = 0;
    eprint!("\r");
    while !done.load(Ordering::Relaxed) {
        eprint!("\r{} {}...", frames[i % frames.len()], message);
        std::io::stderr().flush().unwrap();
        i += 1;
        thread::sleep(Duration::from_millis(80));
    }
    eprint!("\r\rm"); // clear spinner line
}

/// Remove SEI NAL units (type 6) that cause the "Late SEI" warning
fn clean_video(input: &str, output: &str) -> Result<()> {
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();
    let handle = thread::spawn(move || run_spinner("Stripping SEI metadata", done_clone));

    let status = Command::new("ffmpeg")
        .args([
            "-i", input,
            "-c:v", "copy",
            "-bsf:v", "filter_units=remove_types=6",
            "-c:a", "copy",
            "-y", output,
        ])
        .status()?;

    done.store(true, Ordering::Relaxed);
    handle.join().unwrap();

    if !status.success() {
        return Err(anyhow!("Failed to strip SEI from video"));
    }
    Ok(())
}

/// Detect silences in audio using ffmpeg's silencedetect filter
fn detect_silences(video: &str, threshold: f64, min_duration: f64) -> Result<Vec<SilenceInterval>> {
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();
    let handle = thread::spawn(move || run_spinner("Detecting silences", done_clone));

    let output = Command::new("ffmpeg")
        .args([
            "-i", video,
            "-af", &format!("silencedetect=noise={}dB:d={}", threshold, min_duration),
            "-f", "null",
            "-"
        ])
        .output()?;

    done.store(true, Ordering::Relaxed);
    handle.join().unwrap();

    let stderr = String::from_utf8(output.stderr)?;
    let mut silences = Vec::new();
    let mut current_start = None;

    for line in stderr.lines() {
        if line.contains("silence_start:") {
            if let Some(start_str) = line.split("silence_start:").nth(1) {
                if let Ok(start) = start_str.trim().parse::<f64>() {
                    current_start = Some(start);
                }
            }
        } else if line.contains("silence_end:") && current_start.is_some() {
            if let Some(end_str) = line.split("silence_end:").nth(1) {
                if let Some(end_part) = end_str.split('|').next() {
                    if let Ok(end) = end_part.trim().parse::<f64>() {
                        let start = current_start.take().unwrap();
                        silences.push(SilenceInterval {
                            start,
                            end,
                            duration: end - start,
                        });
                    }
                }
            }
        }
    }
    Ok(silences)
}

/// Generate automatic cuts based on silences and limits
fn generate_cuts(total_duration: f64, silences: &[SilenceInterval], min_seg: f64, max_seg: f64) -> Vec<(f64, f64)> {
    let mut cuts = Vec::new();
    let mut current_pos = 0.0;

    // Sort silences by time
    let mut silences_sorted = silences.to_vec();
    silences_sorted.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());

    while current_pos < total_duration {
        let target_end = (current_pos + max_seg).min(total_duration);

        // Find the best cut point within the nearest silence in the range [current_pos + min_seg, target_end]
        let mut best_cut = target_end;
        let mut best_score = f64::INFINITY;

        for silence in &silences_sorted {
            if silence.start > target_end { break; }
            // Ideal cut is the middle of the silence, as long as it falls within the allowed range
            let cut_candidate = (silence.start + silence.end) / 2.0;
            if cut_candidate >= current_pos + min_seg && cut_candidate <= target_end {
                let distance_to_target = (cut_candidate - target_end).abs();
                if distance_to_target < best_score {
                    best_score = distance_to_target;
                    best_cut = cut_candidate;
                }
            }
        }

        // If no silence found, use target_end as-is
        if best_cut == target_end && best_score.is_infinite() {
            // Try to find the nearest silence after target_end (to avoid cutting in the middle of speech)
            for silence in &silences_sorted {
                if silence.start >= target_end && silence.start < target_end + 2.0 {
                    best_cut = silence.start; // cut at the start of the silence
                    break;
                }
            }
        }

        // Ensure it doesn't exceed the total duration
        if best_cut > total_duration { best_cut = total_duration; }
        if best_cut - current_pos < min_seg && current_pos > 0.0 {
            // If too short, extend to the minimum
            best_cut = (current_pos + min_seg).min(total_duration);
        }

        cuts.push((current_pos, best_cut));
        current_pos = best_cut;

        // Prevent infinite loop
        if current_pos >= total_duration { break; }
    }

    cuts
}

/// Process video: strip SEI, detect silences, generate cuts and call ffmpeg to cut
fn process_video(video_file: &str, config: &Config) -> Result<()> {
    let base = Path::new(video_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video");
    let ext = Path::new(video_file)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp4");

    // Create a clean copy without SEI NAL units
    let clean_file = format!("{}_clean.{}", base, ext);
    clean_video(video_file, &clean_file)?;

    // Get total video duration
    let duration_output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            &clean_file
        ])
        .output()?;
    let total_duration: f64 = String::from_utf8(duration_output.stdout)?
        .trim()
        .parse()
        .context("Could not get video duration")?;

    let silences = detect_silences(&clean_file, config.silence_threshold, config.min_silence)?;
    println!("Found {} silences.", silences.len());

    println!("Generating automatic cuts...");
    let cuts = generate_cuts(total_duration, &silences, config.min_segment, config.max_segment);
    println!("{} segments will be generated.", cuts.len());

    for (idx, (start, end)) in cuts.iter().enumerate() {
        let out_name = format!("{}_{:03}.{}", base, idx + 1, ext);
        println!("[{}/{}] Cutting {:.2}s - {:.2}s -> {}", idx + 1, cuts.len(), start, end, out_name);

        let status = Command::new("ffmpeg")
            .args([
                "-ss", &start.to_string(),
                "-i", &clean_file,
                "-t", &(end - start).to_string(),
                "-c:v", "libx264", "-preset", "fast",
                "-c:a", "aac",
                "-y", &out_name,
            ])
            .status()?;

        if !status.success() {
            let _ = fs::remove_file(&clean_file);
            return Err(anyhow!("Failed to cut segment {} ({}s - {}s)", idx + 1, start, end));
        }
    }

    // Clean up temporary file
    let _ = fs::remove_file(&clean_file);

    println!("{} segments generated successfully!", cuts.len());
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: vcut <video_file>");
        std::process::exit(1);
    }
    let video_file = &args[1];
    let conteudo = fs::read_to_string("vcut.json")?;
    let config: Config = serde_json::from_str(&conteudo)?;

    process_video(video_file, &config)?;
    Ok(())
}
