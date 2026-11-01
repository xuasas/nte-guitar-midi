use std::path::{Path, PathBuf};

/// Locate the existing portable installation without changing it.
pub fn installation_root() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() { candidates.push(dir.to_path_buf()); }
    }
    if let Ok(dir) = std::env::current_dir() { candidates.push(dir); }
    find_installation_root(&candidates)
}

fn find_installation_root(candidates: &[PathBuf]) -> PathBuf {
    for candidate in candidates {
        for dir in candidate.ancestors() {
            if dir.join("vendor").join("sheetsage2").exists() && dir.join("transcriber").exists() {
                return dir.to_path_buf();
            }
        }
    }
    candidates.first().cloned().unwrap_or_else(|| PathBuf::from("."))
}

fn is_midi(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
}

pub fn scan(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut result = Vec::new();
    let sample = root.join("assets/test_midis/sample_demo.mid");
    if sample.is_file() { result.push(sample); }
    let out = root.join("out");
    if !out.exists() { return Ok(result); }
    let mut files = Vec::new();
    let entries = std::fs::read_dir(&out).map_err(|e| format!("无法读取曲库 {}：{e}", out.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() && is_midi(&path) {
            files.push(path);
        } else if path.is_dir() && entry.file_name().to_string_lossy().starts_with("模型对比_") {
            for child in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
                let child = child.map_err(|e| e.to_string())?.path();
                if child.is_file() && is_midi(&child) { files.push(child); }
            }
        }
    }
    files.retain(|p| !matches!(p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase().as_str(),
        "main_melody" | "chords" | "chords_merged" | "melody_vocal" | "melody_instrumental" | "transcription"));
    files.sort(); result.extend(files); Ok(result)
}

/// Import without overwriting any existing music.
pub fn import(source: &Path, root: &Path) -> Result<PathBuf, String> {
    if !source.is_file() || !is_midi(source) { return Err("请选择存在的 MIDI 文件".into()); }
    let dir = root.join("out");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if source.parent().is_some_and(|p| p == dir) { return Ok(source.to_path_buf()); }
    let stem = source.file_stem().ok_or("文件名无效")?.to_string_lossy();
    let ext = source.extension().ok_or("文件扩展名无效")?.to_string_lossy();
    for i in 1..10_000 {
        let name = if i == 1 { format!("{stem}.{ext}") } else { format!("{stem}_{i}.{ext}") };
        let dst = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&dst) {
            Ok(mut f) => {
                let result = std::fs::File::open(source).and_then(|mut src| std::io::copy(&mut src, &mut f));
                if let Err(e) = result { drop(f); let _ = std::fs::remove_file(&dst); return Err(e.to_string()); }
                return Ok(dst);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("无法为曲目分配文件名".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_without_overwriting_and_finds_uppercase_midi() {
        let temp = std::env::temp_dir().join(format!("mingyin-library-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp).unwrap();
        let source = temp.join("测试.MID"); std::fs::write(&source, b"first").unwrap();
        let first = import(&source, &temp).unwrap();
        std::fs::write(&source, b"second").unwrap();
        let second = import(&source, &temp).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first).unwrap(), b"first");
        assert_eq!(std::fs::read(second).unwrap(), b"second");
        assert_eq!(scan(&temp).unwrap().len(), 2);
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn executable_installation_takes_precedence_over_working_directory() {
        let temp = std::env::temp_dir().join(format!("mingyin-roots-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let first = temp.join("executable-install");
        let second = temp.join("working-install");
        for dir in [&first, &second] {
            std::fs::create_dir_all(dir.join("vendor/sheetsage2")).unwrap();
            std::fs::create_dir_all(dir.join("transcriber")).unwrap();
        }
        let nested = first.join("rust_app/target/release");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_installation_root(&[nested, second.clone()]), first);
        assert_eq!(find_installation_root(&[temp.join("unrelated"), second.clone()]), second);
        std::fs::remove_dir_all(temp).unwrap();
    }
}
