//! Application state and input handling.
use crate::engine::{self, Bank, Cmd, Engine};
use crate::ir::*;
use crate::midi;
use crate::project::*;
use crate::script::{self, ScriptError};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode { Tui, Code }
#[derive(Clone, Copy, PartialEq)]
pub enum View { Roll, Steps, Mixer }
pub enum PromptKind { ImportMidi, OpenFile }
pub struct Prompt { pub label: String, pub buf: String, pub kind: PromptKind }
pub struct Snap { pub song: Song, pub files: Vec<(String, Vec<String>)> }
pub struct Completion { pub items: Vec<(String, String)>, pub sel: usize, pub plen: usize }

pub const MIX_PARAMS: [&str; 9] = ["Volume", "Pan", "Wave", "Cutoff", "Drive", "Delay", "Reverb", "Attack", "Release"];
const GRIDS: [f64; 4] = [1.0, 0.5, 0.25, 0.125];

const GLOBAL_API: [(&str, &str); 16] = [
    ("track(", "track(name, opts?) → Track"),
    ("bpm(", "bpm(value)"),
    ("timeSig(", "timeSig(num, den)"),
    ("n(", "n(\"C#4\") → MIDI number"),
    ("chord(", "chord(root, type?) → number[]"),
    ("scale(", "scale(root, type?, octaves?) → number[]"),
    ("console.log(", "console.log(...) — shown in the log panel"),
    ("const ", "keyword"), ("let ", "keyword"), ("function ", "keyword"), ("return ", "keyword"),
    ("import ", "keyword"), ("export ", "keyword"), ("for (", "keyword"), ("interface ", "keyword"), ("type ", "keyword"),
];
const MEMBER_API: [(&str, &str); 6] = [
    ("note(", "note(start, dur, pitch, vel?)"),
    ("notes(", "notes([[start, dur, pitch, vel?], ...])"),
    ("chord(", "chord(start, dur, pitches[], vel?)"),
    ("steps(", "steps(\"x...x...\", pitch, {start?, step?, vel?})"),
    ("auto(", "auto(\"cutoff\", [[beat, value], ...])"),
    ("set(", "set({ vol?, pan?, cutoff?, ... })"),
];

pub struct App {
    pub project: Project,
    pub song: Song,
    pub mode: Mode,
    pub view: View,
    pub sel: usize,
    pub cur_step: f64,
    pub cur_pitch: u8,
    pub grid: f64,
    pub last_dur: f64,
    pub scroll_x: f64,
    pub base_pitch: u8,
    pub step_base: u8,
    pub mix_param: usize,
    pub cfile: usize,
    pub status: String,
    pub status_err: bool,
    pub error: Option<ScriptError>,
    pub logs: Vec<String>,
    pub engine: Option<Engine>,
    pub audio_msg: String,
    pub bank: Bank,
    pub prompt: Option<Prompt>,
    pub comp: Option<Completion>,
    pub code_changed: bool,
    pub armed: Option<&'static str>,
    pub quit: bool,
    pub meter: (f32, f32),
    pub roll_cols: usize,
    pub roll_rows: usize,
    pub ed_w: usize,
    pub ed_h: usize,
    pub undo: Vec<Snap>,
    pub redo: Vec<Snap>,
}

fn r4(x: f64) -> f64 { (x * 10000.0).round() / 10000.0 }
fn is_word(c: char) -> bool { c.is_alphanumeric() || c == '_' || c == '$' }
fn bidx(s: &str, col: usize) -> usize { s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len()) }

impl App {
    pub fn new(dir: &Path) -> Result<App, String> {
        let project = Project::open(dir).map_err(|e| format!("cannot open project: {e}"))?;
        let (engine, audio_msg) = match Engine::start() {
            Ok(e) => { let m = format!("audio: {}", e.device); (Some(e), m) }
            Err(e) => (None, format!("no audio ({e}); editing and export still work")),
        };
        let mut app = App {
            project, song: Song::default(), mode: Mode::Tui, view: View::Roll, sel: 0, cur_step: 0.0, cur_pitch: 60, grid: 0.25, last_dur: 0.25,
            scroll_x: 0.0, base_pitch: 48, step_base: 60, mix_param: 0, cfile: 0, status: String::new(), status_err: false, error: None, logs: vec![],
            engine, audio_msg, bank: Bank::new(), prompt: None, comp: None, code_changed: false, armed: None, quit: false, meter: (0.0, 0.0),
            roll_cols: 64, roll_rows: 24, ed_w: 80, ed_h: 20, undo: vec![], redo: vec![],
        };
        app.run_code();
        let m = app.audio_msg.clone();
        if app.error.is_none() { app.say(&m, false); }
        Ok(app)
    }

    fn snap(&self) -> Snap {
        Snap { song: self.song.clone(), files: self.project.files.iter().map(|f| (f.path.clone(), f.lines.clone())).collect() }
    }
    fn push_undo(&mut self, s: Snap) {
        if self.undo.len() >= 200 { self.undo.remove(0); }
        self.undo.push(s);
        self.redo.clear();
    }
    fn restore(&mut self, s: Snap) {
        self.song = s.song;
        for (path, lines) in s.files {
            if let Some(f) = self.project.files.iter_mut().find(|f| f.path == path) {
                if f.lines != lines { f.lines = lines; f.dirty = true; f.clamp(); }
            }
        }
        self.sel = self.sel.min(self.song.tracks.len().saturating_sub(1));
        self.sync_engine();
    }
    fn undo(&mut self) {
        match self.undo.pop() {
            Some(prev) => { let cur = self.snap(); self.redo.push(cur); self.restore(prev); self.say(&format!("Undo ({} left)", self.undo.len()), false); }
            None => self.say("Nothing to undo", false),
        }
    }
    fn redo(&mut self) {
        match self.redo.pop() {
            Some(next) => { let cur = self.snap(); self.undo.push(cur); self.restore(next); self.say("Redo", false); }
            None => self.say("Nothing to redo", false),
        }
    }
    pub fn toggle_play(&mut self) {
        match &self.engine {
            Some(e) => e.send(if e.playing() { Cmd::Stop } else { Cmd::Play }),
            None => { let m = self.audio_msg.clone(); self.say(&m, true) }
        }
    }

    pub fn say(&mut self, m: &str, err: bool) { self.status = m.to_string(); self.status_err = err; }

    pub fn track(&self) -> Option<&Track> { self.song.tracks.get(self.sel) }
    pub fn bar_beats(&self) -> f64 { self.song.beats_per_bar() }

    pub fn sync_engine(&mut self) {
        if let Some(e) = &self.engine { e.send(Cmd::SetSong(Box::new(self.song.clone()), self.bank.clone())); }
    }

    // ---------- code → IR ----------

    pub fn run_code(&mut self) {
        if self.code_changed { self.undo.clear(); self.redo.clear(); }
        let sources = self.project.sources();
        match script::run_project(&sources, "main.ts") {
            Ok(out) => {
                let mut song = out.song;
                self.project.mark_editable(&mut song);
                let mut errs = vec![];
                self.bank = engine::load_bank(&song, &self.project.dir, &mut errs);
                self.logs = out.logs;
                self.logs.extend(errs.into_iter().map(|e| format!("warn: {e}")));
                self.song = song;
                self.sel = self.sel.min(self.song.tracks.len().saturating_sub(1));
                self.error = None;
                self.code_changed = false;
                self.sync_engine();
                let locked = self.song.tracks.iter().filter(|t| !t.editable).count();
                self.say(&format!("Ran OK — {} tracks ({} code-owned), {} BPM", self.song.tracks.len(), locked, self.song.bpm), false);
            }
            Err(e) => {
                let loc = e.line.map_or(String::new(), |l| format!(":{l}"));
                let m = format!("{}{} — {}", e.file, loc, e.message);
                if let (Some(l), Some(fi)) = (e.line, self.project.files.iter().position(|f| f.path == e.file)) {
                    if self.mode == Mode::Code { self.cfile = fi; let f = &mut self.project.files[fi]; f.cur = ((l - 1).min(f.lines.len() - 1), 0); }
                }
                self.error = Some(e);
                self.say(&m, true);
            }
        }
    }

    pub fn save(&mut self) {
        match self.project.save_all() {
            Ok(0) => self.say("Nothing to save", false),
            Ok(n) => self.say(&format!("Saved {n} file(s)"), false),
            Err(e) => self.say(&format!("Save failed: {e}"), true),
        }
    }

    pub fn tick(&mut self) {
        if let Some(e) = &self.engine {
            let (l, r) = e.peaks();
            self.meter = (l.max(self.meter.0 * 0.85), r.max(self.meter.1 * 0.85));
        }
    }

    pub fn play_pos(&self) -> f64 { self.engine.as_ref().map_or(0.0, |e| e.pos()) }
    pub fn is_playing(&self) -> bool { self.engine.as_ref().map_or(false, |e| e.playing()) }

    // ---------- TUI-side edits (each one rewrites the matching `// @tui` region) ----------

    fn edit_track<F: FnOnce(&mut Track)>(&mut self, f: F) -> bool {
        match self.song.tracks.get(self.sel) {
            None => { self.say("No track — press n to create one", true); return false; }
            Some(t) if !t.editable => {
                self.say("Code-defined track: read-only in the TUI (edit it in CODE mode)", true);
                return false;
            }
            _ => {}
        }
        let snap = self.snap();
        let t = &mut self.song.tracks[self.sel];
        let old = t.clone();
        f(t);
        if *t == old { return false; }
        let t = t.clone();
        self.project.write_track(&t);
        self.push_undo(snap);
        self.sync_engine();
        true
    }

    fn edit_song<F: FnOnce(&mut Song)>(&mut self, f: F) {
        if !self.song.editable {
            self.say("Tempo/meter are set by code (no canonical `// @tui song` block)", true);
            return;
        }
        let snap = self.snap();
        let old = (self.song.bpm, self.song.time_sig);
        f(&mut self.song);
        if (self.song.bpm, self.song.time_sig) == old { return; }
        let s = self.song.clone();
        self.project.write_song(&s);
        self.push_undo(snap);
        self.sync_engine();
    }

    fn snap_cursor(&mut self) { self.cur_step = ((self.cur_step / self.grid).round() * self.grid).max(0.0); }

    fn toggle_note(&mut self) {
        let (cs, cp) = (self.cur_step, self.cur_pitch);
        let dur = if self.view == View::Steps { 0.25 } else { self.last_dur };
        self.edit_track(|t| {
            if let Some(i) = t.notes.iter().position(|n| n.pitch == cp && n.start <= cs + 1e-9 && cs < n.start + n.dur - 1e-9) {
                t.notes.remove(i);
            } else {
                t.notes.push(Note { start: cs, dur, pitch: cp, vel: 100 });
            }
        });
    }

    fn tweak_note(&mut self, dur: f64, vel: i32) {
        let (cs, cp, g) = (self.cur_step, self.cur_pitch, self.grid);
        let mut nd = None;
        self.edit_track(|t| {
            if let Some(n) = t.notes.iter_mut().find(|n| n.pitch == cp && n.start <= cs + 1e-9 && cs < n.start + n.dur - 1e-9) {
                n.dur = r4((n.dur + dur * g).max(g.min(0.125)));
                n.vel = (n.vel as i32 + vel).clamp(1, 127) as u8;
                nd = Some(n.dur);
            }
        });
        if let Some(d) = nd { self.last_dur = d; }
    }

    fn new_track(&mut self, drums: bool) {
        let mut i = self.song.tracks.len() + 1;
        let name = loop {
            let n = format!("{} {}", if drums { "drums" } else { "track" }, i);
            if !self.song.tracks.iter().any(|t| t.name == n) { break n; }
            i += 1;
        };
        let snap = self.snap();
        let mut t = Track::new(&name, if drums { "drums" } else { "saw" });
        t.editable = true;
        self.project.write_track(&t);
        self.push_undo(snap);
        self.song.tracks.push(t);
        self.sel = self.song.tracks.len() - 1;
        self.sync_engine();
        self.say(&format!("Added '{name}' (written to main.ts)"), false);
    }

    fn delete_track(&mut self) {
        let Some(t) = self.track() else { return };
        if !t.editable { self.say("Code-defined track: remove it in CODE mode", true); return; }
        let name = t.name.clone();
        if self.armed != Some("delete") {
            self.armed = Some("delete");
            self.say(&format!("Press Ctrl+D again to delete '{name}'"), true);
            return;
        }
        self.armed = None;
        let snap = self.snap();
        self.push_undo(snap);
        self.project.remove_track(&name);
        self.song.tracks.remove(self.sel);
        self.sel = self.sel.min(self.song.tracks.len().saturating_sub(1));
        self.sync_engine();
        self.say(&format!("Deleted '{name}'"), false);
    }

    fn toggle_flag(&mut self, solo: bool) {
        let Some(t) = self.song.tracks.get_mut(self.sel) else { return };
        if t.editable {
            self.edit_track(|t| if solo { t.solo = !t.solo } else { t.mute = !t.mute });
        } else {
            if solo { t.solo = !t.solo } else { t.mute = !t.mute }
            self.sync_engine();
            self.say("Session-only (code-defined track); re-running resets it", false);
        }
    }

    fn adjust(&mut self, dir: f64, big: bool) {
        let p = self.mix_param;
        let st = |a: f64, b: f64| if big { b } else { a };
        self.edit_track(|t| match p {
            0 => t.vol = r4((t.vol + dir * st(0.02, 0.1)).clamp(0.0, 1.5)),
            1 => t.pan = r4((t.pan + dir * st(0.05, 0.25)).clamp(-1.0, 1.0)),
            2 => {
                if let Some(i) = INSTS.iter().position(|x| *x == t.inst) {
                    t.inst = INSTS[(i as i32 + dir as i32).rem_euclid(INSTS.len() as i32) as usize].to_string();
                }
            }
            3 => {
                let c = (t.cutoff * 1.12f64.powf(dir * st(1.0, 4.0))).clamp(40.0, 20000.0);
                t.cutoff = if c > 17000.0 { 20000.0 } else { c.round() };
            }
            4 => t.drive = r4((t.drive + dir * st(0.05, 0.2)).clamp(0.0, 1.0)),
            5 => t.delay = r4((t.delay + dir * st(0.02, 0.1)).clamp(0.0, 1.0)),
            6 => t.reverb = r4((t.reverb + dir * st(0.02, 0.1)).clamp(0.0, 1.0)),
            7 => t.adsr[0] = r4((t.adsr[0] + dir * st(0.005, 0.05)).clamp(0.001, 2.0)),
            _ => t.adsr[3] = r4((t.adsr[3] + dir * st(0.02, 0.2)).clamp(0.002, 4.0)),
        });
    }

    pub fn step_rows(&self) -> Vec<u8> {
        if self.track().map_or(false, |t| t.is_drums()) { DRUM_LANES.iter().map(|d| d.0).collect() }
        else { (0..12).map(|i| self.step_base + 11 - i).collect() }
    }

    fn export_wav(&mut self) {
        let name = self.project.dir.canonicalize().ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string())).unwrap_or("song".into());
        let path = self.project.dir.join("out").join(format!("{name}.wav"));
        match engine::render_wav(&self.song, self.bank.clone(), &path) {
            Ok(s) => self.say(&format!("Exported {:.1}s WAV → {}", s, path.display()), false),
            Err(e) => self.say(&format!("WAV export failed: {e}"), true),
        }
    }

    fn export_midi(&mut self) {
        let path = self.project.dir.join("out").join("song.mid");
        match midi::export(&self.song, &path) {
            Ok(()) => self.say(&format!("Exported MIDI → {}", path.display()), false),
            Err(e) => self.say(&format!("MIDI export failed: {e}"), true),
        }
    }

    fn import_midi(&mut self, p: &str) {
        let mut path = PathBuf::from(p.trim());
        if path.is_relative() && !path.exists() { path = self.project.dir.join(&path); }
        match midi::import(&path) {
            Ok(imp) => {
                let snap = self.snap();
                self.push_undo(snap);
                let n = imp.tracks.len();
                for mut t in imp.tracks {
                    let base = t.name.clone();
                    let mut k = 2;
                    while self.song.tracks.iter().any(|x| x.name == t.name) { t.name = format!("{base} {k}"); k += 1; }
                    t.editable = true;
                    self.project.write_track(&t);
                    self.song.tracks.push(t);
                }
                if self.song.editable {
                    if let Some(b) = imp.bpm { self.song.bpm = r4(b.round()); }
                    if let Some(s) = imp.time_sig { self.song.time_sig = s; }
                    let s = self.song.clone();
                    self.project.write_song(&s);
                }
                self.sel = self.song.tracks.len() - 1;
                self.sync_engine();
                self.say(&format!("Imported {n} track(s) from {}", path.display()), false);
            }
            Err(e) => self.say(&format!("MIDI import failed: {e}"), true),
        }
    }

    // ---------- keyboard ----------

    pub fn on_key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release { return; }
        let armed = self.armed.take();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(mut p) = self.prompt.take() {
            match k.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    match p.kind {
                        PromptKind::ImportMidi => self.import_midi(&p.buf),
                        PromptKind::OpenFile => match self.project.create_file(&p.buf) {
                            Ok(i) => { self.cfile = i; self.say(&format!("Opened {}", self.project.files[i].path), false); }
                            Err(e) => self.say(&e, true),
                        },
                    }
                }
                KeyCode::Backspace => { p.buf.pop(); self.prompt = Some(p); }
                KeyCode::Char(c) if !ctrl => { p.buf.push(c); self.prompt = Some(p); }
                _ => self.prompt = Some(p),
            }
            return;
        }
        if ctrl && k.code == KeyCode::Char('q') {
            if self.project.any_dirty() && armed != Some("quit") {
                self.armed = Some("quit");
                self.say("Unsaved changes! Press Ctrl+Q again to quit without saving (Ctrl+S saves)", true);
            } else { self.quit = true; }
            return;
        }
        if ctrl && k.code == KeyCode::Char('s') {
            self.save();
            if self.mode == Mode::Code { self.run_code(); }
            return;
        }
        if self.mode == Mode::Code { self.on_key_code(k); } else { self.armed = armed; self.on_key_tui(k); }
    }

    fn leave_code(&mut self) {
        if self.mode == Mode::Code && self.code_changed { self.run_code(); }
    }

    fn on_key_tui(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let bpb = self.bar_beats();
        match k.code {
            KeyCode::Tab => { self.mode = Mode::Code; return; }
            KeyCode::Char('d') if ctrl => { self.delete_track(); return; }
            KeyCode::Char('r') if ctrl => { self.run_code(); return; }
            KeyCode::Char(' ') => { self.toggle_play(); return; }
            KeyCode::Char('u') => { self.undo(); return; }
            KeyCode::Char('z') if ctrl => { self.undo(); return; }
            KeyCode::Char('U') => { self.redo(); return; }
            KeyCode::Char('y') if ctrl => { self.redo(); return; }
            KeyCode::Char('p') => {
                if let Some(e) = &self.engine { e.send(Cmd::Seek(self.cur_step)); e.send(Cmd::Play); }
                return;
            }
            KeyCode::Home | KeyCode::Char('0') => { if let Some(e) = &self.engine { e.send(Cmd::Seek(0.0)); } if self.view != View::Mixer { self.cur_step = 0.0; self.scroll_x = 0.0; } return; }
            KeyCode::Char('1') => { self.view = View::Roll; self.snap_cursor(); return; }
            KeyCode::Char('2') => {
                self.view = View::Steps;
                self.cur_step = (self.cur_step / 0.25).round() * 0.25;
                let rows = self.step_rows();
                if !rows.contains(&self.cur_pitch) { self.cur_pitch = rows[rows.len() / 2]; }
                return;
            }
            KeyCode::Char('3') => { self.view = View::Mixer; return; }
            KeyCode::Char('[') => { self.sel = self.sel.saturating_sub(1); return; }
            KeyCode::Char(']') => { if self.sel + 1 < self.song.tracks.len() { self.sel += 1; } return; }
            KeyCode::Char('n') => { self.new_track(false); return; }
            KeyCode::Char('N') => { self.new_track(true); return; }
            KeyCode::Char('<') => { let b = self.song.bpm - 1.0; self.edit_song(|s| s.bpm = b.max(20.0)); return; }
            KeyCode::Char('>') => { let b = self.song.bpm + 1.0; self.edit_song(|s| s.bpm = b.min(400.0)); return; }
            KeyCode::Char('T') => {
                self.edit_song(|s| {
                    let opts = [[4, 4], [3, 4], [6, 8], [5, 4], [7, 8], [2, 4]];
                    let i = opts.iter().position(|o| *o == s.time_sig).map_or(0, |i| i + 1);
                    s.time_sig = opts[i % opts.len()];
                });
                return;
            }
            KeyCode::Char('e') => { self.export_wav(); return; }
            KeyCode::Char('m') if self.view != View::Mixer => { self.export_midi(); return; }
            KeyCode::Char('i') => { self.prompt = Some(Prompt { label: "Import MIDI file".into(), buf: String::new(), kind: PromptKind::ImportMidi }); return; }
            KeyCode::Char('?') => { self.say("Space play/stop · u undo · U redo · p play from cursor · 1/2/3 views · [ ] track · n/N new · Enter note · l/L len · v/V vel · g grid · < > BPM · e WAV · m MIDI · i import · Tab CODE", false); return; }
            _ => {}
        }
        match self.view {
            View::Roll => match k.code {
                KeyCode::Left => { self.cur_step = (self.cur_step - self.grid).max(0.0); }
                KeyCode::Right => { self.cur_step += self.grid; }
                KeyCode::PageUp => { self.cur_step = (self.cur_step - bpb).max(0.0); }
                KeyCode::PageDown => { self.cur_step += bpb; }
                KeyCode::Up => { self.cur_pitch = (self.cur_pitch + if shift { 12 } else { 1 }).min(127); }
                KeyCode::Down => { self.cur_pitch = self.cur_pitch.saturating_sub(if shift { 12 } else { 1 }); }
                KeyCode::Enter => self.toggle_note(),
                KeyCode::Delete | KeyCode::Backspace => {
                    let (cs, cp) = (self.cur_step, self.cur_pitch);
                    self.edit_track(|t| t.notes.retain(|n| !(n.pitch == cp && n.start <= cs + 1e-9 && cs < n.start + n.dur - 1e-9)));
                }
                KeyCode::Char('l') => self.tweak_note(1.0, 0),
                KeyCode::Char('L') => self.tweak_note(-1.0, 0),
                KeyCode::Char('v') => self.tweak_note(0.0, 8),
                KeyCode::Char('V') => self.tweak_note(0.0, -8),
                KeyCode::Char('g') => { let i = GRIDS.iter().position(|g| *g == self.grid).unwrap_or(0); self.grid = GRIDS[(i + 1) % GRIDS.len()]; self.last_dur = self.grid; self.snap_cursor(); }
                _ => {}
            },
            View::Steps => {
                let rows = self.step_rows();
                let ri = rows.iter().position(|p| *p == self.cur_pitch).unwrap_or(0);
                match k.code {
                    KeyCode::Left => self.cur_step = (self.cur_step - 0.25).max(0.0),
                    KeyCode::Right => self.cur_step += 0.25,
                    KeyCode::PageUp => self.cur_step = (self.cur_step - bpb).max(0.0),
                    KeyCode::PageDown => self.cur_step += bpb,
                    KeyCode::Up => {
                        if shift && !self.track().map_or(false, |t| t.is_drums()) { self.step_base = (self.step_base + 12).min(108); self.cur_pitch = (self.cur_pitch + 12).min(127); }
                        else if ri > 0 { self.cur_pitch = rows[ri - 1]; }
                    }
                    KeyCode::Down => {
                        if shift && !self.track().map_or(false, |t| t.is_drums()) { self.step_base = self.step_base.saturating_sub(12).max(12); self.cur_pitch = self.cur_pitch.saturating_sub(12); }
                        else if ri + 1 < rows.len() { self.cur_pitch = rows[ri + 1]; }
                    }
                    KeyCode::Enter | KeyCode::Char('x') => self.toggle_note(),
                    KeyCode::Char('v') => { self.grid = 0.25; self.tweak_note(0.0, 8) }
                    KeyCode::Char('V') => { self.grid = 0.25; self.tweak_note(0.0, -8) }
                    _ => {}
                }
            }
            View::Mixer => match k.code {
                KeyCode::Left => self.sel = self.sel.saturating_sub(1),
                KeyCode::Right => { if self.sel + 1 < self.song.tracks.len() { self.sel += 1; } }
                KeyCode::Up => self.mix_param = self.mix_param.saturating_sub(1),
                KeyCode::Down => self.mix_param = (self.mix_param + 1).min(MIX_PARAMS.len() - 1),
                KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('l') => self.adjust(1.0, false),
                KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::Char('h') => self.adjust(-1.0, false),
                KeyCode::PageUp => self.adjust(1.0, true),
                KeyCode::PageDown => self.adjust(-1.0, true),
                KeyCode::Char('m') => self.toggle_flag(false),
                KeyCode::Char('s') => self.toggle_flag(true),
                _ => {}
            },
        }
    }

    // ---------- code editor ----------

    pub fn file(&mut self) -> &mut SrcFile { let i = self.cfile.min(self.project.files.len() - 1); &mut self.project.files[i] }

    pub fn on_paste(&mut self, s: String) {
        if let Some(p) = &mut self.prompt { p.buf.push_str(s.trim()); return; }
        if self.mode != Mode::Code { return; }
        let f = self.file();
        snapshot(f);
        for c in s.replace("\r\n", "\n").replace('\t', "  ").chars() {
            if c == '\n' { split_line(f, false); } else { insert_char(f, c); }
        }
        f.dirty = true;
        self.code_changed = true;
        self.undo.clear();
        self.redo.clear();
    }

    fn on_key_code(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let h = self.ed_h.max(4);
        // completion popup navigation
        if let Some(c) = &mut self.comp {
            match k.code {
                KeyCode::Up => { c.sel = c.sel.saturating_sub(1); return; }
                KeyCode::Down => { c.sel = (c.sel + 1).min(c.items.len() - 1); return; }
                KeyCode::Tab | KeyCode::Enter => { self.accept_completion(); return; }
                KeyCode::Esc => { self.comp = None; return; }
                _ => {}
            }
        }
        match k.code {
            KeyCode::Esc => { self.leave_code(); self.mode = Mode::Tui; return; }
            KeyCode::Left if alt => { self.cfile = (self.cfile + self.project.files.len() - 1) % self.project.files.len(); return; }
            KeyCode::Right if alt => { self.cfile = (self.cfile + 1) % self.project.files.len(); return; }
            KeyCode::Char('t') if ctrl => { self.cfile = (self.cfile + self.project.files.len() - 1) % self.project.files.len(); return; }
            KeyCode::Char('r') if ctrl => { self.run_code(); return; }
            KeyCode::Char('g') if ctrl => { self.toggle_play(); return; }
            KeyCode::Char('o') if ctrl => { self.cfile = (self.cfile + 1) % self.project.files.len(); return; }
            KeyCode::Char('p') if ctrl => { self.prompt = Some(Prompt { label: "Open / create file (e.g. lib/drums.ts)".into(), buf: String::new(), kind: PromptKind::OpenFile }); return; }
            KeyCode::Char(' ') if ctrl => { self.refresh_completion(true); return; }
            KeyCode::Char('n') if ctrl => { self.refresh_completion(true); return; }
            _ => {}
        }
        let mut changed = true;
        let f = self.file();
        match k.code {
            KeyCode::Char('z') if ctrl => {
                if let Some((l, c)) = f.undo.pop() { f.lines = l; f.cur = c; f.dirty = true; f.clamp(); }
            }
            KeyCode::Char('k') if ctrl => { snapshot(f); let r = f.cur.0; f.lines.remove(r); if f.lines.is_empty() { f.lines.push(String::new()); } f.clamp(); }
            KeyCode::Char('d') if ctrl => { snapshot(f); let r = f.cur.0; let l = f.lines[r].clone(); f.lines.insert(r + 1, l); f.cur.0 += 1; }
            KeyCode::Char('/') | KeyCode::Char('_') if ctrl => {
                snapshot(f);
                let r = f.cur.0;
                let l = &f.lines[r];
                let ind = l.len() - l.trim_start().len();
                if l.trim_start().starts_with("//") && !l.trim_start().starts_with("// @") {
                    let rest = l[ind..].trim_start_matches("//").trim_start_matches(' ').to_string();
                    f.lines[r] = format!("{}{}", &l[..ind], rest);
                } else if !l.trim_start().starts_with("// @") {
                    f.lines[r] = format!("{}// {}", &l[..ind], &l[ind..]);
                }
            }
            KeyCode::Up => { changed = false; if f.cur.0 > 0 { f.cur.0 -= 1; f.clamp(); } }
            KeyCode::Down => { changed = false; if f.cur.0 + 1 < f.lines.len() { f.cur.0 += 1; f.clamp(); } }
            KeyCode::Left => {
                changed = false;
                if f.cur.1 > 0 { f.cur.1 -= 1; } else if f.cur.0 > 0 { f.cur.0 -= 1; f.cur.1 = f.lines[f.cur.0].chars().count(); }
            }
            KeyCode::Right => {
                changed = false;
                if f.cur.1 < f.lines[f.cur.0].chars().count() { f.cur.1 += 1; } else if f.cur.0 + 1 < f.lines.len() { f.cur.0 += 1; f.cur.1 = 0; }
            }
            KeyCode::Home => {
                changed = false;
                let l = &f.lines[f.cur.0];
                let first = l.chars().take_while(|c| *c == ' ').count();
                f.cur.1 = if f.cur.1 == first { 0 } else { first };
            }
            KeyCode::End => { changed = false; f.cur.1 = f.lines[f.cur.0].chars().count(); }
            KeyCode::PageUp => { changed = false; f.cur.0 = f.cur.0.saturating_sub(h); f.clamp(); }
            KeyCode::PageDown => { changed = false; f.cur.0 = (f.cur.0 + h).min(f.lines.len() - 1); f.clamp(); }
            KeyCode::Enter => { snapshot(f); split_line(f, true); }
            KeyCode::Backspace => { snapshot(f); backspace(f); }
            KeyCode::Delete => { snapshot(f); delete(f); }
            KeyCode::Tab => { snapshot(f); insert_char(f, ' '); insert_char(f, ' '); }
            KeyCode::Char(c) if !ctrl && !alt => {
                snapshot(f);
                if c == '}' || c == ')' || c == ']' {
                    let (r, col) = f.cur;
                    let before: String = f.lines[r].chars().take(col).collect();
                    if !before.is_empty() && before.chars().all(|x| x == ' ') && col >= 2 {
                        let b = bidx(&f.lines[r], col - 2);
                        let e = bidx(&f.lines[r], col);
                        f.lines[r].replace_range(b..e, "");
                        f.cur.1 -= 2;
                    }
                }
                insert_char(f, c);
            }
            _ => { changed = false; }
        }
        if changed { f.dirty = true; self.code_changed = true; self.undo.clear(); self.redo.clear(); if self.comp.is_some() { self.refresh_completion(false); } }
        else if self.comp.is_some() { self.comp = None; }
    }

    fn refresh_completion(&mut self, force: bool) {
        let (row, col) = { let f = self.file(); f.cur };
        let line: Vec<char> = self.file().lines[row].chars().collect();
        let mut s = col;
        while s > 0 && is_word(line[s - 1]) { s -= 1; }
        let prefix: String = line[s..col].iter().collect();
        let member = s > 0 && line[s - 1] == '.';
        if !force && prefix.is_empty() { self.comp = None; return; }
        let lp = prefix.to_lowercase();
        let mut items: Vec<(String, String)> = vec![];
        if member {
            items.extend(MEMBER_API.iter().filter(|(n, _)| n.to_lowercase().starts_with(&lp)).map(|(a, b)| (a.to_string(), b.to_string())));
        } else {
            items.extend(GLOBAL_API.iter().filter(|(n, _)| n.to_lowercase().starts_with(&lp)).map(|(a, b)| (a.to_string(), b.to_string())));
            let mut words: Vec<String> = vec![];
            for l in &self.file().lines {
                let mut w = String::new();
                for c in l.chars().chain(std::iter::once(' ')) {
                    if is_word(c) { w.push(c); } else { if w.len() >= 3 && !w.chars().next().unwrap().is_ascii_digit() { words.push(std::mem::take(&mut w)); } else { w.clear(); } }
                }
            }
            words.sort();
            words.dedup();
            for w in words {
                if w.to_lowercase().starts_with(&lp) && w != prefix && !items.iter().any(|(a, _)| a.trim_end_matches(|c| c == '(' || c == ' ') == w) {
                    items.push((w, "in file".into()));
                }
            }
        }
        items.truncate(12);
        self.comp = if items.is_empty() { None } else { Some(Completion { items, sel: 0, plen: prefix.chars().count() }) };
    }

    fn accept_completion(&mut self) {
        let Some(c) = self.comp.take() else { return };
        let ins = c.items[c.sel].0.clone();
        let f = self.file();
        snapshot(f);
        let (r, col) = f.cur;
        let b0 = bidx(&f.lines[r], col - c.plen);
        let b1 = bidx(&f.lines[r], col);
        f.lines[r].replace_range(b0..b1, &ins);
        f.cur.1 = col - c.plen + ins.chars().count();
        f.dirty = true;
        self.code_changed = true;
    }
}

// ---- text-edit primitives ----

fn snapshot(f: &mut SrcFile) {
    if f.undo.len() > 300 { f.undo.remove(0); }
    f.undo.push((f.lines.clone(), f.cur));
}

fn insert_char(f: &mut SrcFile, c: char) {
    let (r, col) = f.cur;
    let b = bidx(&f.lines[r], col);
    f.lines[r].insert(b, c);
    f.cur.1 += 1;
}

fn split_line(f: &mut SrcFile, indent: bool) {
    let (r, col) = f.cur;
    let b = bidx(&f.lines[r], col);
    let rest = f.lines[r].split_off(b);
    let mut pad = String::new();
    if indent {
        pad = " ".repeat(f.lines[r].chars().take_while(|c| *c == ' ').count());
        if matches!(f.lines[r].trim_end().chars().last(), Some('{') | Some('(') | Some('[')) { pad.push_str("  "); }
    }
    let n = pad.chars().count();
    f.lines.insert(r + 1, format!("{pad}{rest}"));
    f.cur = (r + 1, n);
}

fn backspace(f: &mut SrcFile) {
    let (r, col) = f.cur;
    if col > 0 {
        let (a, b) = (bidx(&f.lines[r], col - 1), bidx(&f.lines[r], col));
        f.lines[r].replace_range(a..b, "");
        f.cur.1 -= 1;
    } else if r > 0 {
        let cur = f.lines.remove(r);
        let n = f.lines[r - 1].chars().count();
        f.lines[r - 1].push_str(&cur);
        f.cur = (r - 1, n);
    }
}

fn delete(f: &mut SrcFile) {
    let (r, col) = f.cur;
    if col < f.lines[r].chars().count() {
        let (a, b) = (bidx(&f.lines[r], col), bidx(&f.lines[r], col + 1));
        f.lines[r].replace_range(a..b, "");
    } else if r + 1 < f.lines.len() {
        let next = f.lines.remove(r + 1);
        f.lines[r].push_str(&next);
    }
}
