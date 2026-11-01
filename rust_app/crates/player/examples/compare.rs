//! Diagnostic output for comparing the legacy compiler with the Rust port.
use melody_player::{compile, load_song};
use std::path::Path;

fn main() -> Result<(), String> {
    let path = std::env::args().nth(1).ok_or("missing MIDI path")?;
    let song = load_song(Path::new(&path))?;
    let result = compile(&song, None, true)?;
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        result.track_name,
        result.octave_shift,
        result.folded_notes,
        result.dropped,
        result.conflicts,
        result.accompaniment_notes,
        result.events.len()
    );
    for event in &result.events {
        println!("{:.9}\t{}\t{}", event.time, event.key, event.down as u8);
    }
    Ok(())
}
