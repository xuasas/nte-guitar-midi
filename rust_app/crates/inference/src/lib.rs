//! Transitional CPU-only bridge to the original SheetSage2 pipeline.
//!
//! The worker is external so the GUI stays responsive; no existing output is
//! overwritten by the underlying `run_song_isolated` pipeline.
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, Sender}};
use std::thread;
use std::time::Duration;

const BRIDGE: &str = include_str!("../bridge.py");

#[derive(Debug, Clone)]
pub struct Environment {
    pub main_python: PathBuf,
    pub model_python: PathBuf,
    pub model_cli: PathBuf,
    pub sheetsage_weights: PathBuf,
    pub mert_weights: PathBuf,
    pub bootstrap: PathBuf,
    pub missing: Vec<String>,
}

impl Environment {
    /// Check required files without loading the models or mutating the environment.
    pub fn inspect(root: &Path) -> Self {
        let default_config = root.join("config.json");
        let config_path = std::env::var("YIHUAN_CONFIG").ok().filter(|s| !s.trim().is_empty())
            .map(|s| configured_path(root, &s)).unwrap_or(default_config);
        let config = read_config(&config_path);
        let local = read_config(&root.join("config.local.json"));
        let setting = |key: &str, var: &str, default: &str| {
            std::env::var(var).ok().filter(|s| !s.is_empty())
                .or_else(|| local.get("sheetsage2").and_then(|v| v.get(key)).and_then(|v| v.as_str()).map(str::to_owned))
                .or_else(|| config.get("sheetsage2").and_then(|v| v.get(key)).and_then(|v| v.as_str()).map(str::to_owned))
                .filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_owned())
        };
        let home = configured_path(root, &setting("home", "YIHUAN_SHEETSAGE2_HOME", "vendor/sheetsage2"));
        let main_python = root.join(".venv").join("Scripts").join("python.exe");
        let preferred_python = configured_path(root, &setting("python", "YIHUAN_SHEETSAGE2_PY", "vendor/sheetsage2/.venv/Scripts/python.exe"));
        let fallbacks = local.get("sheetsage2").and_then(|v| v.get("python_fallbacks"))
            .or_else(|| config.get("sheetsage2").and_then(|v| v.get("python_fallbacks")))
            .and_then(|v| v.as_array());
        let model_python = std::iter::once(preferred_python.clone()).chain(
            fallbacks.into_iter().flatten().filter_map(|v| v.as_str()).map(|p| configured_path(root, p))
        ).find(|p| p.exists()).unwrap_or(preferred_python);
        let model_cli = configured_path(root, &setting("cli", "YIHUAN_SHEETSAGE2_CLI", "vendor/sheetsage2/transcribe_cli.py"));
        let sheetsage_weights = home.join("models").join("SheetSage2").join("model.safetensors");
        let mert_weights = home.join("models").join("MERT-v2-FullSong").join("model.safetensors");
        let bootstrap = root.join("setup").join("bootstrap.ps1");
        let missing = [(&main_python, "主程序 Python 环境"), (&model_python, "模型 Python 环境"),
            (&model_cli, "模型转录程序"), (&sheetsage_weights, "SheetSage2 权重"),
            (&mert_weights, "MERT 权重")].into_iter()
            .filter(|(path, _)| !path.is_file()).map(|(_, label)| label.to_owned()).collect();
        Self { main_python, model_python, model_cli, sheetsage_weights, mert_weights, bootstrap, missing }
    }

    pub fn ready(&self) -> bool { self.missing.is_empty() }
}

fn read_config(path: &Path) -> serde_json::Value {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn configured_path(root: &Path, value: &str) -> PathBuf {
    let value = value.trim();
    let expanded = if value == "~" || value.starts_with("~/") || value.starts_with("~\\") {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        match home { Some(home) => PathBuf::from(home).join(value[1..].trim_start_matches(['/', '\\'])), None => PathBuf::from(value) }
    } else { PathBuf::from(value) };
    if expanded.is_absolute() { expanded } else { root.join(expanded) }
}

#[derive(Debug, Clone)]
pub struct TranscriptionRequest {
    pub root: PathBuf,
    pub audio: PathBuf,
    pub output_dir: PathBuf,
    pub final_name: Option<String>,
    /// "both", "vocal", or "inst".
    pub stems: String,
}

#[derive(Debug, Clone)]
pub enum Event {
    Log(String),
    Progress { stage: String, fraction: Option<f32> },
    Completed { output: Option<PathBuf> },
    Failed(String),
    Cancelled,
}

pub struct Task {
    pub events: Receiver<Event>,
    cancel_requested: Arc<AtomicBool>,
}

impl Task {
    /// Request cancellation of the process and its descendants.
    pub fn cancel(&self) { self.cancel_requested.store(true, Ordering::Release); }
}

/// Launch a transcription. Events are delivered asynchronously to the caller.
pub fn start_transcription(request: TranscriptionRequest) -> Result<Task, String> {
    if !request.audio.is_file() { return Err("找不到音频文件，请重新选择。".into()); }
    if !matches!(request.stems.as_str(), "both" | "vocal" | "inst") {
        return Err("扒谱声部无效。".into());
    }
    let env = Environment::inspect(&request.root);
    if !env.ready() { return Err(format!("模型环境不完整：{}。请运行环境修复。", env.missing.join("、"))); }
    let mut cmd = Command::new(&env.main_python);
    cmd.arg("-u").arg("-c").arg(BRIDGE).arg(&request.root).arg(&request.audio)
        .arg(&request.output_dir).arg(&request.stems)
        .arg(request.final_name.as_deref().unwrap_or(""))
        .current_dir(&request.root)
        .env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1")
        .env("CUDA_VISIBLE_DEVICES", "-1").env("ROFORMER_DEVICE", "cpu");
    spawn_task(cmd, true)
}

/// Run the existing CPU install/repair script on an explicit user action.
pub fn start_install(root: &Path) -> Result<Task, String> {
    let env = Environment::inspect(root);
    if !env.bootstrap.is_file() { return Err("找不到环境修复脚本。".into()); }
    let system_root = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    let powershell = PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut cmd = Command::new(powershell);
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(env.bootstrap).arg("-Force").current_dir(root);
    spawn_task(cmd, false)
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn hide_window(_cmd: &mut Command) {}

#[cfg(windows)]
fn bind_job(child: &Child) -> Result<std::os::windows::io::OwnedHandle, String> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW,
        SetInformationJobObject, JobObjectExtendedLimitInformation,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};
    // SAFETY: all pointers are valid for the call and the returned handle is
    // immediately owned. KILL_ON_JOB_CLOSE prevents child processes surviving
    // either task cancellation or an unexpected exit of the parent app.
    unsafe {
        let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if raw.is_null() { return Err(format!("无法创建任务容器：{}", std::io::Error::last_os_error())); }
        let job = OwnedHandle::from_raw_handle(raw);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(job.as_raw_handle(), JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _, std::mem::size_of_val(&limits) as u32) == 0 {
            return Err(format!("无法配置任务容器：{}", std::io::Error::last_os_error()));
        }
        if AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) == 0 {
            return Err(format!("无法保护后台任务：{}", std::io::Error::last_os_error()));
        }
        Ok(job)
    }
}

#[cfg(not(windows))]
fn bind_job(_child: &Child) -> Result<(), String> { Ok(()) }

fn spawn_task(mut cmd: Command, protocol: bool) -> Result<Task, String> {
    hide_window(&mut cmd);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("无法启动任务：{e}"))?;
    let job = match bind_job(&child) {
        Ok(job) => job,
        Err(error) => { let _ = child.kill(); let _ = child.wait(); return Err(error); }
    };
    let stdout = child.stdout.take().ok_or("无法读取任务输出")?;
    let stderr = child.stderr.take().ok_or("无法读取任务错误信息")?;
    let (events_tx, events) = mpsc::channel();
    let cancel_requested = Arc::new(AtomicBool::new(false));
    let cancel = Arc::clone(&cancel_requested);
    thread::spawn(move || {
        let _job = job;
        supervise(child, stdout, stderr, protocol, events_tx, cancel)
    });
    Ok(Task { events, cancel_requested })
}

enum Line { Stdout(String), Stderr(String) }

fn pump<R: Read + Send + 'static>(reader: R, tx: Sender<Line>, stderr: bool) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buffer).trim_end().to_owned();
                    if !line.is_empty() && tx.send(if stderr { Line::Stderr(line) } else { Line::Stdout(line) }).is_err() { break; }
                }
            }
        }
    })
}

fn parse_event(line: &str) -> Option<Event> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let text = |key| value.get(key).and_then(|v| v.as_str()).unwrap_or("").to_owned();
    match value.get("type")?.as_str()? {
        "log" => Some(Event::Log(text("message"))),
        "progress" => Some(Event::Progress { stage: text("stage"), fraction: value.get("fraction").and_then(|v| v.as_f64()).map(|v| v as f32) }),
        "completed" => Some(Event::Completed { output: Some(PathBuf::from(text("output"))) }),
        "error" => Some(Event::Failed(text("message"))),
        _ => None,
    }
}

fn terminate_tree(child: &mut Child) {
    #[cfg(windows)] {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &child.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null());
        hide_window(&mut cmd);
        let _ = cmd.status();
    }
    let _ = child.kill();
}

fn supervise<R: Read + Send + 'static, S: Read + Send + 'static>(mut child: Child, stdout: R, stderr: S, protocol: bool, tx: Sender<Event>, cancel: Arc<AtomicBool>) {
    let (lines_tx, lines_rx) = mpsc::channel();
    let out_thread = pump(stdout, lines_tx.clone(), false);
    let err_thread = pump(stderr, lines_tx.clone(), true);
    drop(lines_tx);
    let mut result = None;
    let mut cancelled = false;
    loop {
        while let Ok(line) = lines_rx.try_recv() { consume_line(line, protocol, &tx, &mut result); }
        if cancel.load(Ordering::Acquire) {
            cancelled = true;
            terminate_tree(&mut child);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = out_thread.join();
                let _ = err_thread.join();
                while let Ok(line) = lines_rx.try_recv() { consume_line(line, protocol, &tx, &mut result); }
                let terminal = if cancelled { Event::Cancelled }
                else if let Some(Event::Failed(error)) = result.as_ref() { Event::Failed(error.clone()) }
                else if !status.success() { Event::Failed(format!("任务退出失败：{status}")) }
                else if protocol { result.unwrap_or_else(|| Event::Failed("识别任务未返回结果。".into())) }
                else { Event::Completed { output: None } };
                let _ = tx.send(terminal);
                break;
            }
            Err(e) => { let _ = tx.send(Event::Failed(format!("无法等待任务：{e}"))); break; }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn consume_line(line: Line, protocol: bool, tx: &Sender<Event>, result: &mut Option<Event>) {
    match line {
        Line::Stderr(line) => { let _ = tx.send(Event::Log(line)); }
        Line::Stdout(line) if protocol => {
            match parse_event(&line) {
                Some(event @ (Event::Completed { .. } | Event::Failed(_))) => *result = Some(event),
                Some(event) => { let _ = tx.send(event); }
                None => { let _ = tx.send(Event::Log(line)); }
            }
        }
        Line::Stdout(line) => { let _ = tx.send(Event::Log(line)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_environment_is_reported_without_installing() {
        let root = std::env::temp_dir().join("melody_inference_missing_environment_test");
        let env = Environment::inspect(&root);
        assert!(!env.ready());
        assert!(env.missing.iter().any(|x| x.contains("SheetSage2")));
    }

    #[test]
    fn structured_events_preserve_chinese_output() {
        match parse_event(r#"{"type":"completed","output":"D:/歌曲/春天.mid"}"#) {
            Some(Event::Completed { output: Some(path) }) => assert!(path.to_string_lossy().contains("春天.mid")),
            _ => panic!("expected completed output"),
        }
        assert!(matches!(parse_event(r#"{"type":"progress","stage":"识别","fraction":null}"#), Some(Event::Progress { fraction: None, .. })));
    }

    #[test]
    fn nonexistent_audio_is_rejected() {
        let request = TranscriptionRequest { root: PathBuf::new(), audio: PathBuf::from("no-such-audio"), output_dir: PathBuf::new(), final_name: None, stems: "both".into() };
        assert!(start_transcription(request).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn cancellation_stops_a_running_process_tree() {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", "ping -n 30 127.0.0.1 >nul"]);
        let task = spawn_task(cmd, false).expect("start test process");
        task.cancel();
        assert!(matches!(task.events.recv_timeout(Duration::from_secs(10)), Ok(Event::Cancelled)));
    }

    #[cfg(windows)]
    #[test]
    fn closing_job_terminates_running_worker() {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", "ping -n 30 127.0.0.1 >nul"]).stdout(Stdio::null()).stderr(Stdio::null());
        hide_window(&mut cmd);
        let mut child = cmd.spawn().expect("spawn worker");
        let job = bind_job(&child).expect("bind worker to job");
        drop(job);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().expect("wait worker").is_some() { break; }
            assert!(std::time::Instant::now() < deadline, "worker survived job close");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
