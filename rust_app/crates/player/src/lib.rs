//! MIDI compilation and guarded playback for the game's 36-key instrument.
use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Note {
    pub start: f64,
    pub duration: f64,
    pub pitch: u8,
}
#[derive(Clone, Debug)]
pub struct Track {
    pub index: usize,
    pub name: String,
    pub notes: Vec<Note>,
}
#[derive(Clone, Debug)]
pub struct Song {
    pub tracks: Vec<Track>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub time: f64,
    pub key: u8,
    pub down: bool,
}
#[derive(Clone, Debug)]
pub struct Timeline {
    pub events: Vec<Event>,
    pub track_name: String,
    pub octave_shift: i16,
    pub folded_notes: usize,
    pub dropped: usize,
    pub conflicts: usize,
    pub accompaniment_notes: usize,
    pub duration: f64,
}

pub fn load_song(path: &Path) -> Result<Song, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("无法读取 MIDI：{e}"))?;
    let midi = Smf::parse(&bytes).map_err(|e| format!("无法解析 MIDI：{e}"))?;
    if midi.header.format == midly::Format::Sequential {
        return Err("不支持各轨独立时间线的 MIDI 2 型文件".into());
    }
    let ticks_per_beat = match midi.header.timing {
        Timing::Metrical(n) if n.as_int() > 0 => n.as_int() as f64,
        _ => return Err("不支持此 MIDI 时间格式".into()),
    };
    let mut tempos = Vec::new();
    for track in &midi.tracks {
        let mut tick = 0_u64;
        for e in track {
            tick += u64::from(e.delta.as_int());
            if let TrackEventKind::Meta(MetaMessage::Tempo(t)) = e.kind {
                tempos.push((tick, t.as_int()));
            }
        }
    }
    tempos.sort_unstable();
    if tempos.first().is_none_or(|change| change.0 > 0) {
        tempos.insert(0, (0, 500_000));
    }
    let (mut previous, mut elapsed, mut tempo) = (0_u64, 0.0, 500_000_u32);
    let mut segments = Vec::with_capacity(tempos.len());
    for (tick, new_tempo) in tempos {
        elapsed += (tick - previous) as f64 * f64::from(tempo) / ticks_per_beat / 1_000_000.0;
        segments.push((tick, elapsed, new_tempo));
        previous = tick;
        tempo = new_tempo;
    }
    let seconds = |tick: u64| -> f64 {
        let i = segments.partition_point(|s| s.0 <= tick).saturating_sub(1);
        let (start, elapsed, tempo) = segments[i];
        elapsed + (tick - start) as f64 * f64::from(tempo) / ticks_per_beat / 1_000_000.0
    };
    let mut tracks = Vec::new();
    for (index, events) in midi.tracks.iter().enumerate() {
        let mut name = format!("音轨 {}", index + 1);
        let mut notes = Vec::new();
        let mut tick = 0_u64;
        let mut active: HashMap<(u8, u8), f64> = HashMap::new();
        for e in events {
            tick += u64::from(e.delta.as_int());
            let now = seconds(tick);
            match e.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(n)) if !n.is_empty() => {
                    name = String::from_utf8_lossy(n).trim().to_string();
                }
                TrackEventKind::Midi { channel, message } if channel.as_int() != 9 => {
                    let (pitch, down) = match message {
                        MidiMessage::NoteOn { key, vel } => (key.as_int(), vel.as_int() > 0),
                        MidiMessage::NoteOff { key, .. } => (key.as_int(), false),
                        _ => continue,
                    };
                    let key = (channel.as_int(), pitch);
                    if let Some(start) = active.remove(&key) {
                        notes.push(Note {
                            start,
                            duration: (now - start).max(0.08),
                            pitch,
                        });
                    }
                    if down {
                        active.insert(key, now);
                    }
                }
                _ => {}
            }
        }
        let end = seconds(tick);
        for ((_, pitch), start) in active {
            notes.push(Note {
                start,
                duration: (end - start).max(0.08),
                pitch,
            });
        }
        notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
        tracks.push(Track { index, name, notes });
    }
    Ok(Song { tracks })
}

const SCANS: [u8; 36] = [
    0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x1c, 0x10, 0x11, 0x12, 0x13,
    0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
    0x0a, 0x0b, 0x0c, 0x0d,
];
pub fn compile(song: &Song, track: Option<usize>, accompaniment: bool) -> Result<Timeline, String> {
    let tracks: Vec<_> = song.tracks.iter().filter(|t| !t.notes.is_empty()).collect();
    if tracks.is_empty() {
        return Err("MIDI 中没有可演奏的音符".into());
    }
    let named = tracks
        .iter()
        .find(|t| {
            [
                "melody",
                "vocal",
                "sing",
                "voice",
                "lead",
                "solo",
                "主旋律",
                "旋律",
                "人声",
                "唱",
            ]
            .iter()
            .any(|w| t.name.to_lowercase().contains(w))
        })
        .copied();
    let polyphony = |t: &&Track| {
        let mut edges = Vec::with_capacity(t.notes.len() * 2);
        for n in &t.notes {
            edges.push((n.start, 1_i32));
            edges.push((n.start + n.duration, -1));
        }
        edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let (mut current, mut max) = (0, 0);
        for (_, v) in edges {
            current += v;
            max = max.max(current);
        }
        max
    };
    let accomp_track = tracks
        .iter()
        .find(|t| {
            [
                "chord", "accomp", "harmony", "pad", "comp", "guitar", "和弦", "伴奏", "和声",
                "背景",
            ]
            .iter()
            .any(|w| t.name.to_lowercase().contains(w))
        })
        .copied()
        .or_else(|| {
            tracks
                .iter()
                .filter(|t| polyphony(t) >= 3)
                .max_by_key(|t| t.notes.len())
                .copied()
        });
    let auto = || {
        named
            .or_else(|| {
                tracks
                    .iter()
                    .filter(|t| {
                        polyphony(t) <= 2 && accomp_track.is_none_or(|a| a.index != t.index)
                    })
                    .max_by_key(|t| t.notes.len())
                    .copied()
            })
            .unwrap_or_else(|| *tracks.iter().max_by_key(|t| t.notes.len()).unwrap())
    };
    let selected = if let Some(i) = track {
        tracks
            .iter()
            .find(|t| t.index == i)
            .copied()
            .ok_or("选中的音轨没有音符")?
    } else {
        auto()
    };
    let shifts = [0, -12, 12, -24, 24, -36, 36, -48, 48];
    let mut shift = 0;
    let mut max_count = 0;
    for s in shifts {
        let count = selected
            .notes
            .iter()
            .filter(|n| (52..=87).contains(&(i16::from(n.pitch) + s)))
            .count();
        if count > max_count {
            max_count = count;
            shift = s;
        }
    }
    let mut intervals: Vec<(f64, f64, u8, bool)> = Vec::new();
    let mut folded = 0;
    for n in &selected.notes {
        let mut p = i16::from(n.pitch) + shift;
        let old = p;
        while p < 52 {
            p += 12;
        }
        while p > 87 {
            p -= 12;
        }
        if p != old {
            folded += 1;
        }
        intervals.push((
            n.start,
            n.start + n.duration.max(0.04),
            SCANS[(p - 52) as usize],
            true,
        ));
    }
    if accompaniment
        && let Some(t) = tracks.iter().find(|t| {
            t.index != selected.index
                && ["chord", "accomp", "和弦", "伴奏"]
                    .iter()
                    .any(|w| t.name.to_lowercase().contains(w))
        })
    {
        let mut groups: BTreeMap<i64, Vec<&Note>> = BTreeMap::new();
        for n in &t.notes {
            if (52..=87).contains(&(i16::from(n.pitch) + shift)) {
                groups
                    .entry((n.start / 0.08).round_ties_even() as i64)
                    .or_default()
                    .push(n);
            }
        }
        for notes in groups.values_mut() {
            notes.sort_by_key(|n| n.pitch);
            for n in notes.iter().take(2) {
                let key = SCANS[(i16::from(n.pitch) + shift - 52) as usize];
                let end = n.start + n.duration.max(0.04);
                if !intervals
                    .iter()
                    .any(|&(s, e, k, m)| m && k == key && n.start < e && end > s)
                {
                    intervals.push((n.start, end, key, false));
                }
            }
        }
    }
    let mut by_key: BTreeMap<u8, Vec<(f64, f64, bool)>> = BTreeMap::new();
    for (s, e, k, m) in intervals {
        by_key.entry(k).or_default().push((s, e, m));
    }
    let (mut events, mut emitted, mut accomp, mut conflicts) = (Vec::new(), 0, 0, 0);
    for (key, mut times) in by_key {
        times.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for i in 0..times.len() {
            let (start, mut end, melody) = times[i];
            if let Some(next) = times.get(i + 1) {
                if melody && end > next.0 {
                    conflicts += 1;
                }
                end = end.min(next.0);
            }
            if end <= start {
                continue;
            }
            if melody {
                emitted += 1;
            } else {
                accomp += 1;
            }
            events.push(Event {
                time: start,
                key,
                down: true,
            });
            events.push(Event {
                time: end,
                key,
                down: false,
            });
        }
    }
    events.sort_by(|a, b| a.time.total_cmp(&b.time).then(a.down.cmp(&b.down)));
    let duration = events.last().map_or(0.0, |e| e.time);
    Ok(Timeline {
        events,
        track_name: selected.name.clone(),
        octave_shift: shift,
        folded_notes: folded,
        dropped: selected.notes.len() - emitted,
        conflicts,
        accompaniment_notes: accomp,
        duration,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Finished,
    Stopped,
    Error(String),
}
#[derive(Clone, Debug)]
pub struct PlaybackStatus {
    pub state: PlaybackState,
    pub position: f64,
    pub duration: f64,
}

fn reschedule_for_speed(events: &[Event], speed: f64) -> Vec<Event> {
    let mut held: HashMap<u8, (f64, f64)> = HashMap::new();
    let mut last = 0.0_f64;
    events
        .iter()
        .map(|event| {
            let mut time = event.time / speed;
            if !event.down
                && let Some((original_down, scheduled_down)) = held.remove(&event.key)
            {
                let gap = event.time - original_down;
                if gap > 0.0 && gap <= 0.06 {
                    time = time.max(scheduled_down + gap);
                }
            }
            time = time.max(last);
            last = time;
            if event.down {
                held.insert(event.key, (event.time, time));
            }
            Event { time, ..*event }
        })
        .collect()
}

pub struct Player {
    shared: Arc<Mutex<PlaybackStatus>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Player {
    pub fn start(timeline: Timeline, speed: f64) -> Result<Self, String> {
        if timeline.events.is_empty() {
            return Err("时间轴为空".into());
        }
        if !speed.is_finite() || !(0.5..=2.0).contains(&speed) {
            return Err("播放速度应在 0.5 到 2.0 之间".into());
        }
        let hwnd = platform::game_window().ok_or("找不到游戏窗口")?;
        let events = reschedule_for_speed(&timeline.events, speed);
        let duration = events.last().map_or(0.0, |e| e.time);
        let shared = Arc::new(Mutex::new(PlaybackStatus {
            state: PlaybackState::Playing,
            position: 0.0,
            duration,
        }));
        let state = Arc::clone(&shared);
        let worker = thread::spawn(move || {
            let mut guard = Pressed::default();
            if !platform::focus(hwnd) {
                state.lock().unwrap().state = PlaybackState::Error("无法切换到游戏窗口".into());
                return;
            }
            let mut base = Instant::now() + Duration::from_secs(1);
            for ev in &events {
                loop {
                    let current = {
                        let mut s = state.lock().unwrap();
                        if s.state == PlaybackState::Playing
                            && (!platform::is_foreground(hwnd) || platform::modifier_down())
                        {
                            s.state = PlaybackState::Paused;
                        }
                        s.state.clone()
                    };
                    if current == PlaybackState::Stopped {
                        return;
                    }
                    if current == PlaybackState::Paused {
                        guard.release_all();
                        let before = Instant::now();
                        thread::sleep(Duration::from_millis(5));
                        base += before.elapsed();
                        continue;
                    }
                    let target = base + Duration::from_secs_f64(ev.time);
                    let now = Instant::now();
                    if now >= target {
                        let mut s = state.lock().unwrap();
                        if s.state == PlaybackState::Stopped {
                            return;
                        }
                        if s.state != PlaybackState::Playing {
                            continue;
                        }
                        if !platform::is_foreground(hwnd) || platform::modifier_down() {
                            guard.release_all();
                            s.state = PlaybackState::Paused;
                            continue;
                        }
                        if let Err(err) = guard.event(ev.key, ev.down) {
                            s.state = PlaybackState::Error(err);
                            return;
                        }
                        s.position = ev.time;
                        break;
                    }
                    thread::sleep((target - now).min(Duration::from_millis(5)));
                }
            }
            state.lock().unwrap().state = PlaybackState::Finished;
        });
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
    pub fn pause(&self) {
        let mut s = self.shared.lock().unwrap();
        if s.state == PlaybackState::Playing {
            s.state = PlaybackState::Paused;
        }
    }
    pub fn resume(&self) -> Result<(), String> {
        let hwnd = platform::game_window().ok_or("找不到游戏窗口")?;
        if !platform::is_foreground(hwnd) {
            return Err("请先切回游戏窗口".into());
        }
        let mut s = self.shared.lock().unwrap();
        if s.state == PlaybackState::Paused {
            s.state = PlaybackState::Playing;
        }
        Ok(())
    }
    pub fn stop(&self) {
        let mut s = self.shared.lock().unwrap();
        if matches!(s.state, PlaybackState::Playing | PlaybackState::Paused) {
            s.state = PlaybackState::Stopped;
        }
    }
    pub fn status(&self) -> PlaybackStatus {
        self.shared.lock().unwrap().clone()
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Pressed {
    keys: HashSet<u8>,
    send: fn(u8, bool) -> Result<(), String>,
}
impl Default for Pressed {
    fn default() -> Self {
        Self {
            keys: HashSet::new(),
            send: platform::send_key,
        }
    }
}
impl Pressed {
    fn event(&mut self, key: u8, down: bool) -> Result<(), String> {
        if down {
            (self.send)(key, true)?;
            self.keys.insert(key);
        } else if self.keys.contains(&key) {
            (self.send)(key, false)?;
            self.keys.remove(&key);
        }
        Ok(())
    }
    fn release_all(&mut self) {
        for key in self.keys.iter().copied().collect::<Vec<_>>() {
            if (self.send)(key, false).is_ok() {
                self.keys.remove(&key);
            }
        }
    }
}
impl Drop for Pressed {
    fn drop(&mut self) {
        self.release_all();
    }
}
pub fn game_available() -> bool {
    platform::game_window().is_some()
}
pub fn hotkey_down(vk: i32) -> bool {
    platform::key_down(vk)
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;
    #[repr(C)]
    struct MouseInput {
        dx: i32,
        dy: i32,
        data: u32,
        flags: u32,
        time: u32,
        extra: usize,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeyInput {
        vk: u16,
        scan: u16,
        flags: u32,
        time: u32,
        extra: usize,
    }
    #[repr(C)]
    union InputData {
        mouse: std::mem::ManuallyDrop<MouseInput>,
        key: KeyInput,
    }
    #[repr(C)]
    struct Input {
        kind: u32,
        data: InputData,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(Hwnd, isize) -> i32,
            param: isize,
        ) -> i32;
        fn IsWindowVisible(hwnd: Hwnd) -> i32;
        fn GetClassNameW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
        fn GetWindowThreadProcessId(hwnd: Hwnd, process: *mut u32) -> u32;
        fn GetForegroundWindow() -> Hwnd;
        fn SetForegroundWindow(hwnd: Hwnd) -> i32;
        fn GetAsyncKeyState(key: i32) -> i16;
        fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn QueryFullProcessImageNameW(
            process: *mut c_void,
            flags: u32,
            path: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    unsafe extern "system" fn visit(hwnd: Hwnd, param: isize) -> i32 {
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut class = [0u16; 256];
            let mut title = [0u16; 512];
            let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
            let m = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            let title = String::from_utf16_lossy(&title[..m]);
            if class != "UnrealWindow" {
                return 1;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let process = OpenProcess(0x1000, 0, pid);
            let mut path = [0u16; 1024];
            let mut len = path.len() as u32;
            let mut process_name = String::new();
            if !process.is_null() {
                if QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len) != 0 {
                    process_name = String::from_utf16_lossy(&path[..len as usize]);
                }
                CloseHandle(process);
            }
            let named = ["异环", "NTE", "Neverness"]
                .iter()
                .any(|s| title.contains(s))
                || ["HTGame", "NTE", "Neverness"]
                    .iter()
                    .any(|s| process_name.contains(s));
            if named {
                *(param as *mut isize) = hwnd as isize;
                return 0;
            }
            1
        }
    }
    pub fn game_window() -> Option<isize> {
        let mut hwnd = 0isize;
        unsafe {
            EnumWindows(visit, (&mut hwnd as *mut isize) as isize);
        }
        if hwnd == 0 { None } else { Some(hwnd) }
    }
    pub fn focus(hwnd: isize) -> bool {
        unsafe { SetForegroundWindow(hwnd as Hwnd) != 0 || GetForegroundWindow() == hwnd as Hwnd }
    }
    pub fn is_foreground(hwnd: isize) -> bool {
        unsafe { GetForegroundWindow() == hwnd as Hwnd }
    }
    pub fn key_down(vk: i32) -> bool {
        unsafe { GetAsyncKeyState(vk) < 0 }
    }
    pub fn modifier_down() -> bool {
        [0x12, 0x5b, 0x5c].iter().any(|&vk| key_down(vk))
    }
    pub fn send_key(key: u8, down: bool) -> Result<(), String> {
        // Match the legacy engine: the digit row uses virtual keys; other keys use scan codes.
        let digit = match key {
            2..=10 => Some(u16::from(key - 2) + 0x31),
            11 => Some(0x30),
            _ => None,
        };
        let input = Input {
            kind: 1,
            data: InputData {
                key: KeyInput {
                    vk: digit.unwrap_or(0),
                    scan: if digit.is_some() { 0 } else { key as u16 },
                    flags: if digit.is_some() { 0 } else { 0x0008 } | if down { 0 } else { 0x0002 },
                    time: 0,
                    extra: 0,
                },
            },
        };
        if unsafe { SendInput(1, &input, std::mem::size_of::<Input>() as i32) } == 1 {
            Ok(())
        } else {
            Err(format!("按键发送失败：{}", std::io::Error::last_os_error()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static SENT: Mutex<Vec<(u8, bool)>> = Mutex::new(Vec::new());
    fn fake_send(key: u8, down: bool) -> Result<(), String> {
        SENT.lock().unwrap().push((key, down));
        Ok(())
    }
    fn note(start: f64, duration: f64, pitch: u8) -> Note {
        Note {
            start,
            duration,
            pitch,
        }
    }
    #[test]
    fn named_melody_outliers_conflicts_and_accompaniment() {
        let song = Song {
            tracks: vec![
                Track {
                    index: 0,
                    name: "Main Melody".into(),
                    notes: vec![note(0.0, 0.8, 60), note(0.4, 0.3, 60), note(1.0, 0.2, 100)],
                },
                Track {
                    index: 1,
                    name: "Chords".into(),
                    notes: vec![note(0.0, 0.5, 60), note(0.0, 0.5, 64), note(0.0, 0.5, 67)],
                },
            ],
        };
        let t = compile(&song, None, true).unwrap();
        assert_eq!(t.track_name, "Main Melody");
        assert_eq!(t.octave_shift, 0);
        assert_eq!(t.folded_notes, 1);
        assert_eq!(t.conflicts, 1);
        assert_eq!(t.accompaniment_notes, 1);
        let first_up = t
            .events
            .iter()
            .find(|e| e.key == SCANS[8] && !e.down)
            .unwrap();
        assert!((first_up.time - 0.4).abs() < 1e-9);
        assert!(t.events.windows(2).all(|v| v[0].time <= v[1].time));
    }
    #[test]
    fn automatic_track_excludes_polyphonic_accompaniment() {
        let song = Song {
            tracks: vec![
                Track {
                    index: 0,
                    name: "accomp".into(),
                    notes: vec![note(0.0, 1.0, 60), note(0.0, 1.0, 64), note(0.0, 1.0, 67)],
                },
                Track {
                    index: 1,
                    name: "other".into(),
                    notes: vec![note(0.0, 0.2, 72)],
                },
            ],
        };
        assert_eq!(compile(&song, None, false).unwrap().track_name, "other");
        assert!(compile(&song, Some(9), false).is_err());
        assert!(compile(&Song { tracks: vec![] }, None, false).is_err());
    }
    #[test]
    fn speed_change_preserves_short_physical_key_holds() {
        let events = [
            Event {
                time: 0.1,
                key: 1,
                down: true,
            },
            Event {
                time: 0.14,
                key: 1,
                down: false,
            },
        ];
        let out = reschedule_for_speed(&events, 2.0);
        assert!((out[1].time - out[0].time - 0.04).abs() < 1e-9);
    }
    #[test]
    fn held_keys_are_released_on_pause_and_drop() {
        SENT.lock().unwrap().clear();
        {
            let mut held = Pressed {
                keys: HashSet::new(),
                send: fake_send,
            };
            held.event(10, true).unwrap();
            held.release_all();
            // A key-up after pausing must not emit an extra event.
            held.event(10, false).unwrap();
            held.event(11, true).unwrap();
        }
        assert_eq!(
            *SENT.lock().unwrap(),
            vec![(10, true), (10, false), (11, true), (11, false)]
        );
    }
    #[test]
    fn reads_tempo_changes_and_ignores_drums() {
        // Format 1: tempo map changes from 120 to 60 BPM at tick 480; 480 ticks/beat.
        let bytes: [u8; 80] = [
            77, 84, 104, 100, 0, 0, 0, 6, 0, 1, 0, 2, 1, 224, 77, 84, 114, 107, 0, 0, 0, 19, 0,
            255, 81, 3, 7, 161, 32, 131, 96, 255, 81, 3, 15, 66, 64, 0, 255, 47, 0, 77, 84, 114,
            107, 0, 0, 0, 31, 0, 144, 60, 100, 131, 96, 128, 60, 0, 0, 153, 35, 100, 131, 96, 137,
            35, 0, 0, 144, 62, 100, 131, 96, 128, 62, 0, 0, 255, 47, 0,
        ];
        let path = std::env::temp_dir().join(format!("melody-test-{}.mid", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let song = load_song(&path).unwrap();
        let _ = std::fs::remove_file(path);
        assert_eq!(song.tracks[1].notes.len(), 2);
        assert!((song.tracks[1].notes[0].duration - 0.5).abs() < 1e-9);
        assert!((song.tracks[1].notes[1].start - 1.5).abs() < 1e-9);
        assert!((song.tracks[1].notes[1].duration - 1.0).abs() < 1e-9);
    }
    #[test]
    fn original_demo_baseline() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../assets/test_midis/sample_demo.mid");
        let song = load_song(&root).unwrap();
        assert_eq!(song.tracks[0].notes.len(), 4);
        let timeline = compile(&song, None, true).unwrap();
        assert_eq!(timeline.track_name, "Main Melody");
        assert_eq!(timeline.octave_shift, 0);
        assert_eq!(timeline.folded_notes, 0);
        assert_eq!(timeline.dropped, 0);
        assert_eq!(timeline.conflicts, 0);
        assert_eq!(timeline.accompaniment_notes, 0);
        assert_eq!(timeline.events.len(), 8);
        assert_eq!(timeline.events[0].time, 0.0);
    }
}
#[cfg(not(windows))]
mod platform {
    pub fn game_window() -> Option<isize> {
        None
    }
    pub fn focus(_: isize) -> bool {
        false
    }
    pub fn is_foreground(_: isize) -> bool {
        false
    }
    pub fn key_down(_: i32) -> bool {
        false
    }
    pub fn modifier_down() -> bool {
        false
    }
    pub fn send_key(_: u8, _: bool) -> Result<(), String> {
        Err("只支持 Windows".into())
    }
}
