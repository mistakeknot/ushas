//! A single-frame, backpressured video stream. Installed only for video replay.
use crate::config::{RunConfig, SceneKind};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

pub const WIDTH: u32 = 2560;
pub const HEIGHT: u32 = 1440;
pub const FPS: u32 = 60;
pub const FRAME_BYTES: u32 = WIDTH * HEIGHT * 4;
const STALL_TIMEOUT: Duration = Duration::from_secs(120);

struct FramePlan {
    chapters: Vec<SceneKind>,
    next: u32,
    total: u32,
}
impl FramePlan {
    fn new(config: &RunConfig) -> Self {
        let chapters = config.scenes();
        Self {
            total: chapters.len() as u32 * 600,
            chapters,
            next: 0,
        }
    }
    fn header(&self) -> [u8; 24] {
        let mut header = [0; 24];
        header[..8].copy_from_slice(b"USHASV01");
        for (chunk, value) in header[8..]
            .chunks_exact_mut(4)
            .zip([WIDTH, HEIGHT, FPS, self.total])
        {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        header
    }
    fn frame_header(&self, scene: SceneKind, tick: u32) -> Result<[u8; 16], String> {
        if self.next >= self.total
            || self.chapters.get((self.next / 600) as usize) != Some(&scene)
            || tick != self.next % 600 * 2
        {
            return Err(format!(
                "video frame {} has a duplicate, missing or reordered chapter/tick: {} {tick}",
                self.next,
                scene.as_str()
            ));
        }
        let ordinal = SceneKind::ALL
            .iter()
            .position(|s| *s == scene)
            .expect("lab chapter") as u32;
        let mut header = [0; 16];
        for (chunk, value) in
            header
                .chunks_exact_mut(4)
                .zip([self.next, ordinal, tick, FRAME_BYTES])
        {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        Ok(header)
    }
    fn complete(&self) -> bool {
        self.next == self.total
    }
}

/// The kernel pipe is bounded. No frame queue or writer thread exists: a slow
/// encoder pauses this render schedule, retaining its camera and MetalFX history.
pub struct Encoder {
    child: Option<Child>,
    input: Option<ChildStdin>,
    plan: FramePlan,
    output: PathBuf,
    partial: PathBuf,
    lock: PathBuf,
    published: bool,
}
impl Encoder {
    pub fn start(config: &RunConfig) -> Result<Self, String> {
        let program = std::env::var_os("USHAS_VIDEO_ENCODER")
            .map(PathBuf::from)
            .map_or_else(
                || {
                    std::env::current_exe()
                        .map(|p| p.with_file_name("ushas-video-encoder"))
                        .map_err(|e| e.to_string())
                },
                Ok,
            )?;
        Self::start_program(config, &program)
    }
    fn start_program(config: &RunConfig, program: &Path) -> Result<Self, String> {
        let output = config.out.join("video.mp4");
        let partial = config.out.join("video.partial.mp4");
        let lock = config.out.join("video.mp4.encoding-lock");
        if output.exists() || partial.exists() || lock.exists() {
            return Err("video output already exists".into());
        }
        let log = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(config.out.join("encoder.log"))
            .map_err(|e| format!("encoder diagnostics: {e}"))?;
        let mut child = Command::new(program)
            .arg("--out")
            .arg(&output)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(log))
            .spawn()
            .map_err(|e| format!("start video encoder {}: {e}", program.display()))?;
        let input = child.stdin.take();
        let mut encoder = Self {
            child: Some(child),
            input,
            plan: FramePlan::new(config),
            output,
            partial,
            lock,
            published: false,
        };
        let input = encoder
            .input
            .as_mut()
            .ok_or("video encoder has no frame pipe")?;
        nonblocking(input.as_raw_fd())
            .map_err(|e| format!("configure bounded encoder pipe: {e}"))?;
        write_interruptibly(
            input,
            &encoder.plan.header(),
            &crate::control::stop_requested,
        )?;
        Ok(encoder)
    }
    pub fn submit(&mut self, scene: SceneKind, tick: u32, rgba: &[u8]) -> Result<(), String> {
        self.submit_with_cancel(scene, tick, rgba, &crate::control::stop_requested)
    }
    fn submit_with_cancel(
        &mut self,
        scene: SceneKind,
        tick: u32,
        rgba: &[u8],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let result = (|| {
            let header = self.plan.frame_header(scene, tick)?;
            if rgba.len() != FRAME_BYTES as usize || rgba.chunks_exact(4).any(|p| p[3] != 255) {
                return Err("video readback is not tightly packed opaque RGBA8".into());
            }
            let input = self.input.as_mut().ok_or("encoder stream already closed")?;
            write_interruptibly(input, &header, cancelled)?;
            write_interruptibly(input, rgba, cancelled)?;
            self.plan.next += 1;
            if self.plan.next.is_multiple_of(60) || self.plan.complete() {
                crate::report::emit(
                    "progress",
                    json!({"scene":scene.as_str(),"progress":self.plan.next as f64 / self.plan.total as f64,
                    "video_frames":self.plan.next,"video_total_frames":self.plan.total,"message":"Rendering video"}),
                );
            }
            Ok(())
        })();
        if result.is_err() {
            self.abort();
        }
        result
    }
    pub fn finish(&mut self) -> Result<Value, String> {
        let result = (|| {
            if !self.plan.complete() {
                return Err(format!(
                    "incomplete video sequence: {} of {} frames",
                    self.plan.next, self.plan.total
                ));
            }
            self.input.take();
            let started = Instant::now();
            let child = self.child.as_mut().ok_or("video encoder already reaped")?;
            loop {
                if crate::control::stop_requested() {
                    return Err("video export cancelled".into());
                }
                if let Some(status) = child
                    .try_wait()
                    .map_err(|e| format!("wait for video encoder: {e}"))?
                {
                    if !status.success() {
                        return Err(format!(
                            "video encoder failed ({status}); inspect encoder.log"
                        ));
                    }
                    break;
                }
                if started.elapsed() > STALL_TIMEOUT {
                    return Err("video encoder finalization timed out".into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            self.child.take();
            if self.partial.exists()
                || self.lock.exists()
                || std::fs::metadata(&self.output)
                    .map_err(|e| format!("video encoder did not publish output: {e}"))?
                    .len()
                    == 0
            {
                return Err("video encoder left incomplete or empty output".into());
            }
            let hash = crate::report::sha256(&self.output)?;
            self.published = true;
            Ok(
                json!({"path":"video.mp4","width":WIDTH,"height":HEIGHT,"fps":FPS,"simulation_hz":120,
                "frame_count":self.plan.total,"duration_seconds":self.plan.total as f64 / FPS as f64,
                "codec":"h264","bitrate":30000000,"color_space":"rec709","sha256":hash}),
            )
        })();
        if result.is_err() {
            self.abort();
        }
        result
    }
    pub fn abort(&mut self) {
        self.input.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if !self.published {
            let _ = std::fs::remove_file(&self.partial);
            let _ = std::fs::remove_file(&self.output);
            let _ = std::fs::remove_file(&self.lock);
        }
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        self.abort();
    }
}

fn nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fd is a live owned pipe/socket throughout these calls. Existing flags are preserved.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
fn write_interruptibly(
    input: &mut (impl Write + AsRawFd),
    mut bytes: &[u8],
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let mut last_write = Instant::now();
    while !bytes.is_empty() {
        if cancelled() {
            return Err("video export cancelled".into());
        }
        match input.write(bytes) {
            Ok(0) => return Err("video encoder closed its frame pipe".into()),
            Ok(n) => {
                bytes = &bytes[n..];
                last_write = Instant::now();
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if last_write.elapsed() > STALL_TIMEOUT {
                    return Err("video encoder frame admission timed out".into());
                }
                // Wake as soon as the reader admits more bytes. A fixed sleep
                // per pipe-sized chunk would unnecessarily throttle 1440p frames.
                let mut fd = libc::pollfd {
                    fd: input.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: poll borrows one initialized descriptor for at most
                // 50 ms; the input stays owned and open through this call.
                if unsafe { libc::poll(&mut fd, 1, 50) } < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(format!("video encoder pipe readiness failed: {error}"));
                    }
                }
            }
            Err(e) => {
                return Err(format!(
                    "video encoder frame pipe failed: {e}; inspect encoder.log"
                ))
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RunConfig, SceneKind};

    #[test]
    fn stream_cadence_visits_every_chapter_and_uses_frame_index() {
        let mut plan = FramePlan::new(&RunConfig::default());
        assert_eq!(&plan.header()[..8], b"USHASV01");
        assert_eq!(plan.total, 1800);
        for (chapter, scene) in SceneKind::ALL.into_iter().enumerate() {
            for tick in (0..1200).step_by(2) {
                let header = plan.frame_header(scene, tick).unwrap();
                let values: Vec<_> = header
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                assert_eq!(
                    values,
                    [
                        chapter as u32 * 600 + tick / 2,
                        chapter as u32,
                        tick,
                        FRAME_BYTES
                    ]
                );
                plan.next += 1;
            }
        }
        assert!(plan.complete());
        assert!(plan.frame_header(SceneKind::Lighting, 1198).is_err());
    }

    #[test]
    fn single_chapter_keeps_original_ordinal_and_rejects_bad_admission() {
        let mut plan = FramePlan::new(&RunConfig {
            scene: Some(SceneKind::Geometry),
            ..Default::default()
        });
        assert_eq!(plan.total, 600);
        assert!(plan.frame_header(SceneKind::Materials, 0).is_err());
        assert!(plan.frame_header(SceneKind::Geometry, 1).is_err());
        assert!(plan.frame_header(SceneKind::Geometry, 2).is_err());
        assert!(!plan.complete());
        plan.frame_header(SceneKind::Geometry, 0).unwrap();
        // Looking up an admission must not advance replay or accept a duplicate.
        assert_eq!(plan.next, 0);
        plan.next += 1;
        assert!(plan.frame_header(SceneKind::Geometry, 0).is_err());
        assert!(plan.frame_header(SceneKind::Geometry, 2).is_ok());
    }

    fn fake_encoder(script: &str) -> (RunConfig, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!(
            "ushas-video-process-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let program = directory.join("encoder");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        (
            RunConfig {
                out: directory,
                scene: Some(SceneKind::Materials),
                ..Default::default()
            },
            program,
        )
    }

    #[test]
    fn stalled_encoder_has_bounded_admission_and_cancel_reaps_it_and_cleans_files() {
        let (config, program) = fake_encoder("printf incomplete > \"$2\"\nprintf partial > \"${2%/*}/video.partial.mp4\"\nprintf lock > \"$2.encoding-lock\"\nprintf 'retained encoder diagnostic\\n' >&2\nexec /bin/sleep 60");
        let mut encoder = Encoder::start_program(&config, &program).unwrap();
        let pid = encoder.child.as_ref().unwrap().id();
        // Start cancellation only after the fixture is running. An immediate
        // cancellation is also valid, but cannot promise a not-yet-written log.
        let ready = Instant::now();
        while std::fs::metadata(config.out.join("encoder.log"))
            .unwrap()
            .len()
            == 0
        {
            assert!(
                ready.elapsed() < Duration::from_secs(5),
                "fixture failed to start"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let frame = [0, 0, 0, 255].repeat((WIDTH * HEIGHT) as usize);
        let start = Instant::now();
        let error = encoder
            .submit_with_cancel(SceneKind::Materials, 0, &frame, &|| {
                start.elapsed() > Duration::from_millis(40)
            })
            .unwrap_err();
        assert!(error.contains("cancelled"));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(
            encoder.plan.next, 0,
            "a partially written frame never advances cadence"
        );
        assert!(encoder.child.is_none());
        // SAFETY: signal zero only checks this reaped child ID; it sends no signal.
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        for path in [&encoder.output, &encoder.partial, &encoder.lock] {
            assert!(!path.exists());
        }
        assert!(std::fs::read_to_string(config.out.join("encoder.log"))
            .unwrap()
            .contains("retained encoder diagnostic"));
        std::fs::remove_dir_all(config.out).unwrap();
    }

    #[test]
    fn incomplete_sequence_and_encoder_failure_cannot_publish_a_movie() {
        let (config, program) = fake_encoder("exec /bin/sleep 60");
        let mut encoder = Encoder::start_program(&config, &program).unwrap();
        assert!(encoder
            .finish()
            .unwrap_err()
            .contains("incomplete video sequence"));
        assert!(encoder.child.is_none());
        assert!(!encoder.output.exists());
        std::fs::remove_dir_all(config.out).unwrap();

        let (config, program) = fake_encoder("printf 'deliberate failure\\n' >&2\nexit 17");
        if let Ok(mut encoder) = Encoder::start_program(&config, &program) {
            let frame = [0, 0, 0, 255].repeat((WIDTH * HEIGHT) as usize);
            assert!(encoder.submit(SceneKind::Materials, 0, &frame).is_err());
            assert!(encoder.child.is_none());
        }
        assert!(!config.out.join("video.mp4").exists());
        assert!(std::fs::read_to_string(config.out.join("encoder.log"))
            .unwrap()
            .contains("deliberate failure"));
        std::fs::remove_dir_all(config.out).unwrap();
    }

    #[test]
    fn malformed_rgba_and_out_of_order_frames_abort_before_admission() {
        for (tick, pixels) in [
            (0, vec![0; 4]),
            (2, vec![]),
            (0, vec![0; FRAME_BYTES as usize]),
        ] {
            let (config, program) = fake_encoder("exec /bin/sleep 60");
            let mut encoder = Encoder::start_program(&config, &program).unwrap();
            assert!(encoder.submit(SceneKind::Materials, tick, &pixels).is_err());
            assert_eq!(encoder.plan.next, 0);
            assert!(encoder.child.is_none());
            std::fs::remove_dir_all(config.out).unwrap();
        }
    }
}
