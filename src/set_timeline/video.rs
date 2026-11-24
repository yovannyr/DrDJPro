//! Export one local project interval. Only the audio excerpt, one picture and one encoder
//! output are staged; no directories of per-frame images or full-show intermediate files.
use super::{render_wav, Clip, Project};
use crate::{cancellation::Cancellation, cover_art};
use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use image::{imageops, Rgb, RgbImage, RgbaImage};
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
const W: u32 = 640;
const H: u32 = 360;
const FPS: u64 = 30;

fn label(
    img: &mut RgbImage,
    font: &FontRef<'_>,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    color: [u8; 3],
) {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut x = x;
    let mut previous = None;
    for ch in s.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = previous {
            x += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scaled.scale(), point(x, y));
        if x + scaled.h_advance(id) > W as f32 - 24. {
            break;
        }
        if let Some(shape) = font.outline_glyph(glyph) {
            let bounds = shape.px_bounds();
            shape.draw(|u, v, coverage| {
                let x = bounds.min.x as i32 + u as i32;
                let y = bounds.min.y as i32 + v as i32;
                if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
                    let p = img.get_pixel_mut(x as u32, y as u32);
                    for (channel, fg) in p.0.iter_mut().zip(color) {
                        *channel = (f32::from(*channel) * (1. - coverage)
                            + f32::from(fg) * coverage)
                            .round() as u8;
                    }
                }
            });
        }
        x += scaled.h_advance(id);
        previous = Some(id);
    }
}
fn still(
    clip: Option<&Clip>,
    font: &FontRef<'_>,
    cancel: &Cancellation,
) -> Result<RgbImage, String> {
    let mut image = RgbImage::from_pixel(W, H, Rgb([17, 21, 29]));
    let cover = if let Some(c) = clip {
        let v = cover_art::read_cancellable(&c.path, cancel).ok().flatten();
        cancel.check()?;
        v
    } else {
        None
    };
    if let Some((w, h, raw)) = cover {
        if let Some(rgba) = RgbaImage::from_raw(w, h, raw) {
            let r = image::DynamicImage::ImageRgba8(rgba)
                .resize(240, 238, imageops::FilterType::Triangle)
                .to_rgb8();
            imageops::replace(
                &mut image,
                &r,
                i64::from((W - r.width()) / 2),
                i64::from((252 - r.height()) / 2),
            );
        }
    } else {
        // A neutral record silhouette for missing artwork and silent gaps.
        for y in 26..236 {
            for x in 215..425 {
                let d = (x as i32 - 320).pow(2) + (y as i32 - 131).pow(2);
                if d < 104 * 104 {
                    image.put_pixel(
                        x,
                        y,
                        Rgb(if d < 24 * 24 {
                            [81, 110, 131]
                        } else {
                            [39, 48, 59]
                        }),
                    );
                }
            }
        }
    }
    label(
        &mut image,
        font,
        "Dr. DJ Pro",
        24.,
        286.,
        23.,
        [116, 188, 201],
    );
    label(
        &mut image,
        font,
        clip.map_or("Set arrangement", |c| c.title.as_str()),
        24.,
        327.,
        34.,
        [235, 240, 245],
    );
    Ok(image)
}

fn frames(
    p: &Project,
    start: u64,
    end: u64,
    animate: bool,
    cancel: &Cancellation,
    mut out: impl Write,
    sent: &AtomicUsize,
) -> Result<(), String> {
    let font = FontRef::try_from_slice(include_bytes!(
        "../../ui/layout-fonts/LiberationSans-Bold.ttf"
    ))
    .map_err(|e| e.to_string())?;
    let count = ((end - start) * FPS).div_ceil(1000);
    let mut old = None;
    let mut image = still(None, &font, cancel)?;
    for frame in 0..count {
        cancel.check()?;
        let ticks = start * FPS + frame * 1000;
        let active = p
            .clips
            .iter()
            .enumerate()
            .filter(|(_, c)| c.start_ms * FPS <= ticks && ticks < (c.start_ms + c.length_ms) * FPS)
            .max_by_key(|(i, c)| (c.start_ms, *i))
            .map(|(i, _)| i);
        if active != old {
            image = still(active.map(|i| &p.clips[i]), &font, cancel)?;
            old = active;
        }
        let mut bar = 0.;
        if let Some(c) = active.filter(|_| animate).map(|i| &p.clips[i]) {
            if let (Some(anchor), Some(bpm)) = (c.first_beat_ms, c.source_bpm) {
                let elapsed_ms = ticks as f64 / FPS as f64 - c.start_ms as f64;
                let source = c.source_ms as f64 + elapsed_ms * p.bpm / bpm;
                let beat = (source - anchor as f64) * bpm / 60000.;
                let phase = beat.rem_euclid(1.);
                bar = (1.0 - phase * 5.0).clamp(0., 1.);
            }
        }
        let level = (bar * f64::from(W - 48)).round() as u32;
        for y in 339..344 {
            for x in 24..W - 24 {
                image.put_pixel(
                    x,
                    y,
                    if x - 24 < level {
                        Rgb([91, 220, 201])
                    } else {
                        Rgb([32, 40, 48])
                    },
                );
            }
        }
        out.write_all(image.as_raw()).map_err(|e| e.to_string())?;
        sent.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

pub(crate) fn render_video(
    p: &Project,
    path: &Path,
    start: u64,
    end: u64,
    animate: bool,
    cancel: &Cancellation,
) -> Result<(), String> {
    p.interval(start, end)?;
    p.protected_path(path)?;
    if end - start > 600_000 {
        return Err("Choose at most ten minutes of video per export.".into());
    }
    let program = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let encoder=std::env::var_os("PATH").and_then(|value| {
        std::env::split_paths(&value).filter(|p|p.is_absolute()).map(|p|p.join(program)).find(|p|p.is_file())
    }).ok_or("Install FFmpeg with libx264 and AAC, and make it available on PATH before exporting video.")?;
    let parent = path.parent().ok_or("Missing video destination folder.")?;
    let temporary = tempfile::Builder::new()
        .prefix(".drdj-video-")
        .tempdir_in(parent)
        .map_err(|e| e.to_string())?;
    let audio = temporary.path().join("audio.wav");
    render_wav(p, &audio, start, end, cancel)?;
    let file = tempfile::Builder::new()
        .suffix(".mp4")
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    let mut command = Command::new(encoder);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            "640x360",
            "-framerate",
            "30",
            "-i",
            "pipe:0",
            "-i",
        ])
        .arg(&audio)
        .args([
            "-map", "0:v:0", "-map", "1:a:0", "-c:v", "libx264", "-threads", "2", "-pix_fmt",
            "yuv420p", "-preset", "veryfast", "-c:a", "aac", "-t",
        ])
        .arg(format!("{:.3}", (end - start) as f64 / 1000.))
        .args(["-f", "mp4"])
        .arg(file.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot start the local FFmpeg video encoder: {e}"))?;
    let input = child.stdin.take().ok_or("Encoder input unavailable.")?;
    let errors = child
        .stderr
        .take()
        .ok_or("Encoder diagnostics unavailable.")?;
    let sent = AtomicUsize::new(0);
    let outcome = std::thread::scope(|scope| -> Result<(), String> {
        let progress = &sent;
        let producer = scope.spawn(move || frames(p, start, end, animate, cancel, input, progress));
        let messages = scope.spawn(move || {
            let mut errors = errors;
            let mut message = Vec::new();
            let mut chunk = [0; 1024];
            while let Ok(n) = errors.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let keep = n.min(4096 - message.len());
                message.extend_from_slice(&chunk[..keep]);
            }
            String::from_utf8_lossy(&message).into_owned()
        });
        let mut last_count = 0;
        let mut last_progress = Instant::now();
        let status = loop {
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Video export cancelled.".into());
            }
            let count = sent.load(Ordering::Relaxed);
            if count != last_count {
                last_count = count;
                last_progress = Instant::now();
            } else if last_progress.elapsed() >= Duration::from_secs(120) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Video encoder made no progress for two minutes.".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(40)),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e.to_string());
                }
            }
        };
        let message = messages
            .join()
            .map_err(|_| "Encoder diagnostics stopped.")?;
        if !status.success() {
            return Err(format!("Video encoder failed: {message}"));
        }
        producer
            .join()
            .map_err(|_| "Video frame producer stopped.")??;
        Ok(())
    });
    outcome?;
    cancel.check()?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
