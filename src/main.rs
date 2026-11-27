mod app;
mod engine;
mod ir;
mod midi;
mod project;
mod script;
mod ts;
mod ui;

use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::path::PathBuf;
use std::time::Duration;

const HELP: &str = "Rovta — code-driven music production in the terminal

USAGE:
  rovta [PROJECT_DIR]                 open the TUI (creates a starter project if empty)
  rovta PROJECT_DIR --render OUT.wav  headless: run the project and export WAV
  rovta PROJECT_DIR --midi OUT.mid    headless: run the project and export MIDI
  rovta PROJECT_DIR --check           headless: run scripts and report errors
";

fn headless(dir: &PathBuf, args: &[String]) -> Result<(), String> {
    let p = project::Project::open(dir).map_err(|e| e.to_string())?;
    let out = script::run_project(&p.sources(), "main.ts").map_err(|e| format!("{}:{} {}", e.file, e.line.unwrap_or(0), e.message))?;
    for l in &out.logs { println!("> {l}"); }
    let song = out.song;
    println!("OK: {} tracks, {} BPM, {} beats", song.tracks.len(), song.bpm, song.length_beats());
    let mut errs = vec![];
    let bank = engine::load_bank(&song, dir, &mut errs);
    for e in errs { eprintln!("warn: {e}"); }
    if let Some(i) = args.iter().position(|a| a == "--render") {
        let path = PathBuf::from(args.get(i + 1).ok_or("--render needs a path")?);
        let s = engine::render_wav(&song, bank, &path)?;
        println!("wrote {} ({s:.1}s)", path.display());
    }
    if let Some(i) = args.iter().position(|a| a == "--midi") {
        let path = PathBuf::from(args.get(i + 1).ok_or("--midi needs a path")?);
        midi::export(&song, &path)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") { print!("{HELP}"); return; }
    let dir = PathBuf::from(args.iter().find(|a| !a.starts_with("--") && !a.ends_with(".wav") && !a.ends_with(".mid")).cloned().unwrap_or(".".into()));
    if args.iter().any(|a| a == "--render" || a == "--midi" || a == "--check") {
        if let Err(e) = headless(&dir, &args) { eprintln!("error: {e}"); std::process::exit(1); }
        return;
    }
    // ALSA/JACK print diagnostics straight to stderr, which would corrupt the TUI: silence fd 2 while running.
    let saved_err = unsafe {
        let saved = libc::dup(2);
        let null = libc::open(b"/dev/null\0".as_ptr() as *const libc::c_char, libc::O_WRONLY);
        if null >= 0 { libc::dup2(null, 2); libc::close(null); }
        saved
    };
    let restore_err = move || unsafe { if saved_err >= 0 { libc::dup2(saved_err, 2); } };
    let mut app = match app::App::new(&dir) {
        Ok(a) => a,
        Err(e) => { restore_err(); eprintln!("error: {e}"); std::process::exit(1); }
    };
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |i| { let _ = restore(); hook(i); }));
    let res = run(&mut app);
    let _ = restore();
    restore_err();
    if let Err(e) = res { eprintln!("error: {e}"); std::process::exit(1); }
}

fn restore() -> std::io::Result<()> {
    disable_raw_mode()?;
    execute!(std::io::stdout(), DisableBracketedPaste, LeaveAlternateScreen)
}

fn run(app: &mut app::App) -> std::io::Result<()> {
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    while !app.quit {
        term.draw(|f| ui::draw(f, app))?;
        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Key(k) => app.on_key(k),
                Event::Paste(s) => app.on_paste(s),
                _ => {}
            }
            while event::poll(Duration::ZERO)? {
                match event::read()? { Event::Key(k) => app.on_key(k), Event::Paste(s) => app.on_paste(s), _ => {} }
            }
        }
        app.tick();
    }
    Ok(())
}
