//! Records default Windows output via WASAPI loopback in its own COM thread.
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, Sender}};
use std::thread::{self, JoinHandle};

#[derive(Debug)]
pub enum Event {
    Started { sample_rate: u32, channels: u16 },
    Finished { path: Option<PathBuf> },
    Failed(String),
}

pub struct Recorder {
    pub events: Receiver<Event>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    keep: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Recorder {
    /// Start capture asynchronously. `destination` must not already exist.
    pub fn start(destination: PathBuf) -> Self {
        let (tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let keep = Arc::new(AtomicBool::new(false));
        let (s, p, k) = (stop.clone(), paused.clone(), keep.clone());
        let worker = thread::spawn(move || capture(destination, s, p, k, tx));
        Self { events, stop, paused, keep, worker: Some(worker) }
    }
    pub fn pause(&self) { self.paused.store(true, Ordering::Release); }
    pub fn resume(&self) { self.paused.store(false, Ordering::Release); }
    /// Request stop and return immediately. Poll events for completion.
    pub fn stop(&self, keep: bool) { self.keep.store(keep, Ordering::Release); self.stop.store(true, Ordering::Release); }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wait for capture to stop before process shutdown; this lets the worker
        // close the WAV and remove its temporary file even when the window closes.
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

/// Removes only the file created by this recording, including on early errors.
struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

fn create_temporary(dest: &std::path::Path) -> Result<(TemporaryFile, std::fs::File), String> {
    let parent = dest.parent().ok_or("录音目录无效")?;
    let name = dest.file_name().ok_or("录音文件名无效")?.to_string_lossy();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    for index in 0..100 {
        let path = parent.join(format!(".{name}.{}.{}.{index}.part", std::process::id(), now));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((TemporaryFile(path), file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("无法创建临时录音：{e}")),
        }
    }
    Err("无法创建唯一的临时录音文件".into())
}

fn publish_recording(part: &std::path::Path, dest: &std::path::Path) -> Result<PathBuf, String> {
    let parent = dest.parent().ok_or("录音目录无效")?;
    let stem = dest.file_stem().ok_or("录音文件名无效")?.to_string_lossy();
    let extension = dest.extension().and_then(|e| e.to_str()).unwrap_or("wav");
    for index in 1..10000 {
        let target = if index == 1 { dest.to_path_buf() } else { parent.join(format!("{stem}_{index}.{extension}")) };
        match std::fs::hard_link(part, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("保存录音失败：{e}")),
        }
    }
    Err("无法为录音分配文件名".into())
}

#[cfg(not(windows))]
fn capture(_dest: PathBuf, _stop: Arc<AtomicBool>, _paused: Arc<AtomicBool>, _keep: Arc<AtomicBool>, tx: Sender<Event>) {
    let _ = tx.send(Event::Failed("系统录音仅支持 Windows".into()));
}

#[cfg(windows)]
fn capture(dest: PathBuf, stop: Arc<AtomicBool>, paused: Arc<AtomicBool>, keep: Arc<AtomicBool>, tx: Sender<Event>) {
    let result = capture_inner(&dest, &stop, &paused, &keep, &tx);
    match result { Ok(path) => { let _ = tx.send(Event::Finished { path }); }, Err(e) => { let _ = tx.send(Event::Failed(e)); } }
}

#[cfg(windows)]
fn capture_inner(dest: &std::path::Path, stop: &AtomicBool, paused: &AtomicBool, keep: &AtomicBool, tx: &Sender<Event>) -> Result<Option<PathBuf>, String> {
    use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};
    use windows::Win32::Media::Audio::IAudioCaptureClient;
    let run = || -> Result<Option<PathBuf>, String> {
        let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
        let dev = enumerator.get_default_device(&Direction::Render).map_err(|e| e.to_string())?;
        let mut client = dev.get_iaudioclient().map_err(|e| e.to_string())?;
        let native = client.get_mixformat().map_err(|e| e.to_string())?;
        let rate = native.get_samplespersec(); let channels = native.get_nchannels();
        let fmt = WaveFormat::new(32, 32, &SampleType::Float, rate as usize, channels as usize, None);
        let (period, _) = client.get_device_period().map_err(|e| e.to_string())?;
        client.initialize_client(&fmt, &Direction::Capture, &StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: period * 4 }).map_err(|e| e.to_string())?;
        // Access the native capture client to correctly handle SILENT buffers whose data pointer can be null.
        let capture: IAudioCaptureClient = unsafe { client.as_iaudioclient().GetService() }.map_err(|e| e.to_string())?;
        if let Some(parent) = dest.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        let (part, file) = create_temporary(dest)?;
        let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut wav = hound::WavWriter::new(std::io::BufWriter::new(file), spec).map_err(|e| e.to_string())?;
        let recording = (|| -> Result<(), String> {
            client.start_stream().map_err(|e| e.to_string())?;
            let _ = tx.send(Event::Started { sample_rate: rate, channels });
            while !stop.load(Ordering::Acquire) {
                loop {
                    let available = unsafe { capture.GetNextPacketSize() }.map_err(|e| e.to_string())?;
                    if available == 0 { break; }
                    let mut ptr = std::ptr::null_mut(); let mut frames = 0; let mut flags = 0;
                    unsafe { capture.GetBuffer(&mut ptr, &mut frames, &mut flags, None, None) }.map_err(|e| e.to_string())?;
                    let samples = frames as usize * channels as usize;
                    let write_result = if !paused.load(Ordering::Acquire) {
                        if flags & 2 != 0 || ptr.is_null() {
                            (0..samples).try_for_each(|_| wav.write_sample(0.0f32)).map_err(|e| e.to_string())
                        } else {
                            // The audio packet belongs to WASAPI and remains valid until ReleaseBuffer.
                            let bytes = unsafe { std::slice::from_raw_parts(ptr, samples * 4) };
                            bytes.chunks_exact(4).try_for_each(|b| wav.write_sample(f32::from_le_bytes([b[0],b[1],b[2],b[3]]))).map_err(|e| e.to_string())
                        }
                    } else { Ok(()) };
                    let release = unsafe { capture.ReleaseBuffer(frames) }.map_err(|e| e.to_string());
                    write_result?; release?;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            client.stop_stream().map_err(|e| e.to_string())?; Ok(())
        })();
        let finalized = wav.finalize().map_err(|e| e.to_string());
        if recording.is_err() || finalized.is_err() || !keep.load(Ordering::Acquire) {
            recording?; finalized?; return Ok(None);
        }
        // Atomic hard-link creation does not overwrite an existing recording.
        let actual = publish_recording(&part.0, dest)?;
        Ok(Some(actual))
    };
    wasapi::initialize_mta().ok().map_err(|e| e.to_string())?;
    let result = run();
    wasapi::deinitialize();
    result
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn test_directory() -> PathBuf {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
        let dir = std::env::temp_dir().join(format!("melody-recorder-test-{unique}"));
        std::fs::create_dir(&dir).expect("create test directory"); dir
    }

    #[test]
    fn temporary_file_is_cleaned_and_existing_recording_is_not_replaced() {
        let dir = test_directory();
        let dest = dir.join("音乐.wav");
        std::fs::write(&dest, b"existing").unwrap();
        let (part, mut file) = create_temporary(&dest).unwrap();
        use std::io::Write;
        file.write_all(b"new").unwrap(); drop(file);
        let temp_path = part.0.clone();
        let saved = publish_recording(&temp_path, &dest).unwrap();
        drop(part);
        assert!(!temp_path.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing");
        assert_eq!(std::fs::read(&saved).unwrap(), b"new");
        std::fs::remove_file(dest).unwrap();
        std::fs::remove_file(saved).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    #[ignore = "requires an active Windows output audio device"]
    fn dropping_active_recorder_removes_its_partial_file() {
        let dir = test_directory(); let dest = dir.join("closing.wav");
        let recorder = Recorder::start(dest.clone());
        assert!(matches!(recorder.events.recv_timeout(Duration::from_secs(5)), Ok(Event::Started { .. })));
        drop(recorder);
        assert!(!dest.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir(dir).unwrap();
    }

    // Requires a working default speaker. The test captures briefly but does not play any sound.
    #[test]
    #[ignore = "requires an active Windows output audio device"]
    fn default_device_capture_writes_valid_wav() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
        let path = std::env::temp_dir().join(format!("melody-loopback-{unique}.wav"));
        let recorder = Recorder::start(path.clone());
        match recorder.events.recv_timeout(Duration::from_secs(5)).expect("capture did not start") {
            Event::Started { sample_rate, channels } => { assert!(sample_rate > 0); assert!(channels > 0); }
            other => panic!("unexpected result: {other:?}"),
        }
        recorder.pause(); std::thread::sleep(Duration::from_millis(80)); recorder.resume();
        std::thread::sleep(Duration::from_millis(80)); recorder.stop(true);
        match recorder.events.recv_timeout(Duration::from_secs(5)).expect("capture did not stop") {
            Event::Finished { path: Some(actual) } => assert_eq!(actual, path),
            other => panic!("unexpected result: {other:?}"),
        }
        let wav = hound::WavReader::open(&path).expect("valid WAV");
        assert!(wav.spec().sample_rate > 0);
        drop(wav);
        std::fs::remove_file(path).expect("clean temporary test recording");
    }
}
