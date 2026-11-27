//! Song IR — the single data model shared by the TUI, the script runtime and the audio engine.
use serde::{Deserialize, Serialize};

fn d_vol() -> f64 { 0.8 }
fn d_adsr() -> [f64; 4] { [0.01, 0.1, 0.7, 0.2] }
fn d_cut() -> f64 { 20000.0 }
fn d_dt() -> f64 { 0.75 }
fn d_fb() -> f64 { 0.4 }
fn d_root() -> u8 { 60 }
fn d_inst() -> String { "saw".into() }
fn d_res() -> f64 { 0.25 }
fn d_fenv() -> f64 { 0.5 }
fn d_detune() -> f64 { 10.0 }
fn d_sub() -> f64 { 0.3 }
fn d_bpm() -> f64 { 120.0 }
fn d_sig() -> [u32; 2] { [4, 4] }

pub const INSTS: [&str; 6] = ["analog", "sine", "saw", "square", "tri", "drums"];
pub const DRUM_LANES: [(u8, &str); 6] =
    [(36, "Kick"), (38, "Snare"), (42, "C.Hat"), (46, "O.Hat"), (39, "Clap"), (45, "Tom")];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub start: f64, // beats
    pub dur: f64,   // beats
    pub pitch: u8,
    pub vel: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AutoLane {
    pub param: String,
    pub points: Vec<[f64; 2]>, // [beat, value]
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub name: String,
    #[serde(default = "d_inst")]
    pub inst: String,
    #[serde(default)]
    pub sample: Option<String>,
    #[serde(default = "d_root")]
    pub root: u8,
    #[serde(default = "d_vol")]
    pub vol: f64,
    #[serde(default)]
    pub pan: f64,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default = "d_adsr")]
    pub adsr: [f64; 4],
    #[serde(default = "d_cut")]
    pub cutoff: f64,
    /// analog synth: filter resonance 0..1, filter-envelope amount 0..1, oscillator detune (cents), sub level 0..1
    #[serde(default = "d_res")]
    pub res: f64,
    #[serde(default = "d_fenv")]
    pub fenv: f64,
    #[serde(default = "d_detune")]
    pub detune: f64,
    #[serde(default = "d_sub")]
    pub sub: f64,
    #[serde(default)]
    pub drive: f64,
    #[serde(default)]
    pub delay: f64,
    #[serde(default = "d_dt")]
    pub delay_time: f64,
    #[serde(default = "d_fb")]
    pub delay_fb: f64,
    #[serde(default)]
    pub reverb: f64,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub auto: Vec<AutoLane>,
    /// true when the track lives in a canonical `// @tui` region and can be edited from the TUI.
    #[serde(skip)]
    pub editable: bool,
}

impl Track {
    pub fn new(name: &str, inst: &str) -> Track {
        serde_json::from_value(serde_json::json!({ "name": name, "inst": inst })).unwrap()
    }
    pub fn is_drums(&self) -> bool { self.inst == "drums" }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    #[serde(default = "d_bpm")]
    pub bpm: f64,
    #[serde(default = "d_sig")]
    pub time_sig: [u32; 2],
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(skip)]
    pub editable: bool,
}

impl Default for Song {
    fn default() -> Self { Song { bpm: 120.0, time_sig: [4, 4], tracks: vec![], editable: false } }
}

impl Song {
    pub fn beats_per_bar(&self) -> f64 { self.time_sig[0] as f64 * 4.0 / self.time_sig[1].max(1) as f64 }
    /// Song length in beats, rounded up to whole bars (min. 1 bar).
    pub fn length_beats(&self) -> f64 {
        let end = self
            .tracks
            .iter()
            .flat_map(|t| t.notes.iter().map(|n| n.start + n.dur))
            .fold(0.0f64, f64::max);
        let bpb = self.beats_per_bar();
        ((end / bpb).ceil().max(1.0)) * bpb
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(20.0..=400.0).contains(&self.bpm) { return Err(format!("bpm out of range (20-400): {}", self.bpm)); }
        if self.time_sig[0] == 0 || ![1, 2, 4, 8, 16, 32].contains(&self.time_sig[1]) {
            return Err(format!("invalid time signature {}/{}", self.time_sig[0], self.time_sig[1]));
        }
        Ok(())
    }
}

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
pub fn note_name(p: u8) -> String { format!("{}{}", NAMES[(p % 12) as usize], p as i32 / 12 - 1) }
pub fn is_black(p: u8) -> bool { matches!(p % 12, 1 | 3 | 6 | 8 | 10) }
