//! Text-file project management and TUI ⇄ code synchronisation.
//!
//! Tracks edited from the TUI live in *regions* inside the source:
//!
//! ```text
//! // @tui track:lead
//! track("lead", { inst: "saw" }).notes([ [0, 1, 60, 100] ]);
//! // @end
//! ```
//!
//! The TUI regenerates a region only while its body is byte-for-byte canonical (ignoring
//! whitespace). Once the user writes anything more elaborate there (loops, helpers, ...), the
//! track becomes code-owned: it is still played and shown, but the TUI never rewrites it.
use crate::ir::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct SrcFile {
    pub path: String,
    pub lines: Vec<String>,
    pub dirty: bool,
    pub cur: (usize, usize), // (row, col in chars)
    pub scroll: usize,
    pub hscroll: usize,
    pub undo: Vec<(Vec<String>, (usize, usize))>,
}

impl SrcFile {
    pub fn new(path: &str, text: &str) -> SrcFile {
        let mut lines: Vec<String> = text.replace("\r\n", "\n").replace('\t', "  ").split('\n').map(String::from).collect();
        if lines.len() > 1 && lines.last().map_or(false, |l| l.is_empty()) { lines.pop(); }
        if lines.is_empty() { lines.push(String::new()); }
        SrcFile { path: path.to_string(), lines, dirty: false, cur: (0, 0), scroll: 0, hscroll: 0, undo: vec![] }
    }
    pub fn text(&self) -> String { self.lines.join("\n") + "\n" }
    pub fn clamp(&mut self) {
        self.cur.0 = self.cur.0.min(self.lines.len() - 1);
        self.cur.1 = self.cur.1.min(self.lines[self.cur.0].chars().count());
    }
}

pub struct Project {
    pub dir: PathBuf,
    pub files: Vec<SrcFile>,
}

// ---------- formatting helpers (canonical region text) ----------

fn fnum(x: f64) -> String {
    let r = (x * 10000.0).round() / 10000.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{}", r) }
}
fn q(s: &str) -> String { serde_json::to_string(s).unwrap() }

pub fn gen_track(t: &Track) -> Vec<String> {
    let d = Track::new("", "saw");
    let mut o = vec![format!("inst: {}", q(&t.inst))];
    if let Some(s) = &t.sample { o.push(format!("sample: {}", q(s))); }
    if t.root != d.root { o.push(format!("root: {}", t.root)); }
    if t.vol != d.vol { o.push(format!("vol: {}", fnum(t.vol))); }
    if t.pan != d.pan { o.push(format!("pan: {}", fnum(t.pan))); }
    if t.mute { o.push("mute: true".into()); }
    if t.solo { o.push("solo: true".into()); }
    if t.adsr != d.adsr { o.push(format!("adsr: [{}]", t.adsr.iter().map(|x| fnum(*x)).collect::<Vec<_>>().join(", "))); }
    if t.cutoff != d.cutoff { o.push(format!("cutoff: {}", fnum(t.cutoff))); }
    if t.drive != d.drive { o.push(format!("drive: {}", fnum(t.drive))); }
    if t.delay != d.delay { o.push(format!("delay: {}", fnum(t.delay))); }
    if t.delay_time != d.delay_time { o.push(format!("delayTime: {}", fnum(t.delay_time))); }
    if t.delay_fb != d.delay_fb { o.push(format!("delayFb: {}", fnum(t.delay_fb))); }
    if t.reverb != d.reverb { o.push(format!("reverb: {}", fnum(t.reverb))); }
    let mut out = vec![format!("// @tui track:{}", t.name), format!("track({}, {{ {} }})", q(&t.name), o.join(", "))];
    let mut notes = t.notes.clone();
    notes.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap().then(a.pitch.cmp(&b.pitch)));
    if !notes.is_empty() {
        out.push("  .notes([".into());
        for n in &notes { out.push(format!("    [{}, {}, {}, {}],", fnum(n.start), fnum(n.dur), n.pitch, n.vel)); }
        out.push("  ])".into());
    }
    for a in &t.auto {
        let pts: Vec<String> = a.points.iter().map(|p| format!("[{}, {}]", fnum(p[0]), fnum(p[1]))).collect();
        out.push(format!("  .auto({}, [{}])", q(&a.param), pts.join(", ")));
    }
    let last = out.len() - 1;
    out[last].push(';');
    out.push("// @end".into());
    out
}

pub fn gen_song(s: &Song) -> Vec<String> {
    vec![
        "// @tui song".into(),
        format!("bpm({});", fnum(s.bpm)),
        format!("timeSig({}, {});", s.time_sig[0], s.time_sig[1]),
        "// @end".into(),
    ]
}

fn squash(lines: &[String]) -> String { lines.iter().flat_map(|l| l.chars()).filter(|c| !c.is_whitespace()).collect() }

/// Locate a region by marker id, e.g. `track:lead` or `song`. Returns (marker line, `@end` line).
pub fn find_region(lines: &[String], id: &str) -> Option<(usize, usize)> {
    let marker = format!("// @tui {id}");
    let start = lines.iter().position(|l| l.trim() == marker)?;
    let end = (start + 1..lines.len()).find(|&i| lines[i].trim() == "// @end")?;
    Some((start, end))
}

impl Project {
    pub fn open(dir: &Path) -> std::io::Result<Project> {
        fs::create_dir_all(dir)?;
        let mut p = Project { dir: dir.to_path_buf(), files: vec![] };
        if !dir.join("main.ts").exists() { p.write_starter()?; }
        let dts = dir.join("rovta.d.ts");
        if !dts.exists() { fs::write(dts, DTS)?; }
        p.reload_files()?;
        Ok(p)
    }

    fn collect(dir: &Path, rel: &str, depth: usize, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut ents: Vec<_> = rd.flatten().collect();
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            let name = e.file_name().to_string_lossy().to_string();
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let ft = e.file_type();
            if ft.as_ref().map_or(false, |t| t.is_dir()) {
                if depth < 3 && !matches!(name.as_str(), "target" | "node_modules" | ".git" | "out") && !name.starts_with('.') {
                    Self::collect(&e.path(), &path, depth + 1, out);
                }
            } else if name.ends_with(".ts") && !name.ends_with(".d.ts") {
                out.push(path);
            }
        }
    }

    pub fn reload_files(&mut self) -> std::io::Result<()> {
        let mut names = vec![];
        Self::collect(&self.dir, "", 0, &mut names);
        names.sort_by_key(|n| (n != "main.ts", n.clone()));
        self.files = names.iter().map(|n| Ok(SrcFile::new(n, &fs::read_to_string(self.dir.join(n))?))).collect::<std::io::Result<_>>()?;
        Ok(())
    }

    pub fn save_all(&mut self) -> std::io::Result<usize> {
        let mut n = 0;
        for f in self.files.iter_mut().filter(|f| f.dirty) {
            let p = self.dir.join(&f.path);
            if let Some(parent) = p.parent() { fs::create_dir_all(parent)?; }
            fs::write(p, f.text())?;
            f.dirty = false;
            n += 1;
        }
        Ok(n)
    }

    pub fn any_dirty(&self) -> bool { self.files.iter().any(|f| f.dirty) }

    pub fn sources(&self) -> HashMap<String, String> {
        self.files.iter().map(|f| (f.path.clone(), f.text())).collect()
    }

    pub fn create_file(&mut self, name: &str) -> Result<usize, String> {
        let mut name = name.trim().trim_start_matches("./").to_string();
        if name.is_empty() || name.contains("..") { return Err("invalid file name".into()); }
        if !name.ends_with(".ts") { name.push_str(".ts"); }
        if let Some(i) = self.files.iter().position(|f| f.path == name) { return Ok(i); }
        let mut f = SrcFile::new(&name, "// \n");
        f.dirty = true;
        self.files.push(f);
        Ok(self.files.len() - 1)
    }

    // ---------- region sync ----------

    fn locate(&self, id: &str) -> Option<(usize, (usize, usize))> {
        self.files.iter().enumerate().find_map(|(i, f)| find_region(&f.lines, id).map(|r| (i, r)))
    }

    /// Mark which tracks / song settings are canonical (TUI-editable). Everything else is code-owned.
    pub fn mark_editable(&self, song: &mut Song) {
        for t in song.tracks.iter_mut() {
            let id = format!("track:{}", t.name);
            t.editable = self.locate(&id).map_or(false, |(fi, (s, e))| {
                let body = &self.files[fi].lines[s..=e];
                squash(body) == squash(&gen_track(t))
            });
        }
        song.editable = self.locate("song").map_or(false, |(fi, (s, e))| squash(&self.files[fi].lines[s..=e]) == squash(&gen_song(song)));
    }

    fn put_region(&mut self, id: &str, new: Vec<String>) {
        if let Some((fi, (s, e))) = self.locate(id) {
            let f = &mut self.files[fi];
            f.lines.splice(s..=e, new);
            f.dirty = true;
            f.clamp();
        } else {
            let fi = self.files.iter().position(|f| f.path == "main.ts").unwrap_or(0);
            let f = &mut self.files[fi];
            if f.lines.last().map_or(false, |l| !l.trim().is_empty()) { f.lines.push(String::new()); }
            f.lines.extend(new);
            f.dirty = true;
        }
    }

    pub fn write_track(&mut self, t: &Track) { self.put_region(&format!("track:{}", t.name), gen_track(t)); }
    pub fn write_song(&mut self, s: &Song) { self.put_region("song", gen_song(s)); }

    pub fn remove_track(&mut self, name: &str) {
        if let Some((fi, (s, e))) = self.locate(&format!("track:{name}")) {
            let f = &mut self.files[fi];
            f.lines.drain(s..=e);
            if s < f.lines.len() && s > 0 && f.lines[s].trim().is_empty() && f.lines[s - 1].trim().is_empty() { f.lines.remove(s); }
            if f.lines.is_empty() { f.lines.push(String::new()); }
            f.dirty = true;
            f.clamp();
        }
    }

    fn write_starter(&mut self) -> std::io::Result<()> {
        fs::create_dir_all(self.dir.join("lib"))?;
        fs::write(self.dir.join("lib/patterns.ts"), PATTERNS)?;
        let mut song = Song::default();
        song.bpm = 112.0;
        let mut bass = Track::new("bass", "saw");
        bass.vol = 0.7;
        bass.cutoff = 900.0;
        bass.adsr = [0.005, 0.15, 0.5, 0.1];
        for (bar, root) in [33u8, 29, 36, 31].iter().enumerate() {
            for k in 0..8 {
                bass.notes.push(Note { start: bar as f64 * 4.0 + k as f64 * 0.5, dur: 0.4, pitch: *root, vel: if k % 2 == 0 { 110 } else { 85 } });
            }
        }
        let mut drums = Track::new("drums", "drums");
        drums.vol = 0.85;
        for bar in 0..4 {
            for k in 0..4 {
                let b = bar as f64 * 4.0 + k as f64;
                drums.notes.push(Note { start: b, dur: 0.25, pitch: 36, vel: 120 });
                if k % 2 == 1 { drums.notes.push(Note { start: b, dur: 0.25, pitch: 38, vel: 105 }); }
            }
        }
        let mut lines: Vec<String> = MAIN_HEAD.lines().map(String::from).collect();
        for part in [gen_song(&song), gen_track(&bass), gen_track(&drums)] {
            lines.push(String::new());
            lines.extend(part);
        }
        fs::write(self.dir.join("main.ts"), lines.join("\n") + "\n")
    }
}

const MAIN_HEAD: &str = r#"// Rovta project. Switch between TUI and CODE with Tab (TUI→CODE) and Esc (CODE→TUI).
// Blocks between `// @tui ...` and `// @end` are managed by the TUI. Anything else is free-form
// TypeScript: loops, functions, imports. Code-defined tracks stay intact and play as usual.
import { arpeggio, hats } from "./lib/patterns";

// Code-defined tracks (shown read-only in the TUI)
const am: number[] = chord("A3", "min");
track("arp", { inst: "tri", vol: 0.4, delay: 0.3, reverb: 0.25 })
  .notes(arpeggio(am, 4, { rate: 0.5 }));
track("hats", { inst: "drums", vol: 0.3 }).notes(hats(4));

// TUI-managed section (piano roll / step sequencer / mixer)"#;

const PATTERNS: &str = r#"// Helper library — shows multi-file composition and TypeScript types.
export type Hit = [start: number, dur: number, pitch: number, vel: number];

export interface ArpOptions {
  rate?: number; // beats per note
  octaves?: number;
}

/** Cycle through chord tones for `bars` bars of 4/4. */
export function arpeggio(chordNotes: number[], bars: number, opts: ArpOptions = {}): Hit[] {
  const rate: number = opts.rate ?? 0.25;
  const octaves: number = opts.octaves ?? 1;
  const pool: number[] = [];
  for (let o = 0; o < octaves; o++) for (const p of chordNotes) pool.push(p + 12 * o);
  const out: Hit[] = [];
  const total = Math.round((bars * 4) / rate);
  for (let i = 0; i < total; i++) out.push([i * rate, rate * 0.9, pool[i % pool.length], 80 + (i % 4 === 0 ? 20 : 0)]);
  return out;
}

/** Off-beat open hats with closed 8ths. */
export function hats(bars: number, swing = 0): Hit[] {
  const out: Hit[] = [];
  for (let i = 0; i < bars * 8; i++) {
    const t = i * 0.5 + (i % 2 ? swing : 0);
    out.push([t, 0.25, i % 2 ? 46 : 42, i % 2 ? 90 : 70]);
  }
  return out;
}
"#;

pub const DTS: &str = r#"// Type declarations for the Rovta scripting API (for tsc / editor support).
type Pitch = number | string; // MIDI number or note name such as "C#4"
type Inst = "sine" | "saw" | "square" | "tri" | "drums" | "sampler";
interface TrackOptions {
  inst?: Inst; sample?: string; root?: number;
  vol?: number; pan?: number; mute?: boolean; solo?: boolean;
  adsr?: [number, number, number, number];
  cutoff?: number; drive?: number; delay?: number; delayTime?: number; delayFb?: number; reverb?: number;
}
type AutoParam = "vol" | "pan" | "cutoff" | "drive" | "delay" | "reverb";
declare class Track {
  note(start: number, dur: number, pitch: Pitch, vel?: number): Track;
  notes(list: Array<[number, number, Pitch] | [number, number, Pitch, number]>): Track;
  chord(start: number, dur: number, pitches: Pitch[], vel?: number): Track;
  steps(pattern: string, pitch: Pitch, opts?: { start?: number; step?: number; vel?: number; dur?: number }): Track;
  auto(param: AutoParam, points: Array<[number, number]>): Track;
  set(opts: TrackOptions): Track;
}
declare function track(name: string, opts?: TrackOptions): Track;
declare function bpm(value: number): void;
declare function timeSig(numerator: number, denominator: number): void;
declare function n(pitch: Pitch): number;
declare function chord(root: Pitch, type?: "maj" | "min" | "dim" | "aug" | "sus2" | "sus4" | "maj7" | "min7" | "dom7"): number[];
declare function scale(root: Pitch, type?: "major" | "minor" | "pentatonic" | "dorian" | "blues", octaves?: number): number[];
"#;
