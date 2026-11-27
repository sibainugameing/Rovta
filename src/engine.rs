//! Real-time music engine. All audio-rate work happens here (Rust); the UI only sends commands.
use crate::ir::*;
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

const BLOCK: usize = 64;

pub struct Sample { pub data: Vec<f32>, pub sr: f32 }
pub type Bank = HashMap<String, Arc<Sample>>;

pub fn load_bank(song: &Song, dir: &Path, errs: &mut Vec<String>) -> Bank {
    let mut bank = Bank::new();
    for t in &song.tracks {
        let Some(path) = &t.sample else { continue };
        if bank.contains_key(path) { continue; }
        match load_wav(&dir.join(path)) {
            Ok(s) => { bank.insert(path.clone(), Arc::new(s)); }
            Err(e) => errs.push(format!("sample '{path}': {e}")),
        }
    }
    bank
}

fn load_wav(path: &Path) -> Result<Sample, String> {
    let mut r = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().filter_map(Result::ok).map(|x| x as f32 * scale).collect()
        }
    };
    let data = raw.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    Ok(Sample { data, sr: spec.sample_rate as f32 })
}

pub enum Cmd { SetSong(Box<Song>, Bank), Play, Stop, Seek(f64), Loop(bool) }

#[derive(Default)]
pub struct Shared {
    pub pos_beats: AtomicU64,
    pub playing: AtomicBool,
    pub peak_l: AtomicU32,
    pub peak_r: AtomicU32,
}

// ---------------- voices ----------------

#[derive(Clone, Copy, PartialEq)]
enum Stage { Attack, Decay, Sustain, Release, Done }

struct Env { stage: Stage, level: f32, a: f32, d: f32, s: f32, r: f32 }
impl Env {
    fn new(adsr: [f64; 4], sr: f32) -> Env {
        let a = 1.0 / (adsr[0].max(0.001) as f32 * sr);
        let s = adsr[2].clamp(0.0, 1.0) as f32;
        let d = (1.0 - s).max(0.0001) / (adsr[1].max(0.001) as f32 * sr);
        Env { stage: Stage::Attack, level: 0.0, a, d, s, r: adsr[3].max(0.002) as f32 * sr }
    }
    fn release(&mut self) {
        if self.stage != Stage::Done && self.stage != Stage::Release {
            self.stage = Stage::Release;
            self.d = self.level / self.r; // reuse `d` as the release step
        }
    }
    #[inline]
    fn tick(&mut self) -> f32 {
        match self.stage {
            Stage::Attack => { self.level += self.a; if self.level >= 1.0 { self.level = 1.0; self.stage = Stage::Decay; } }
            Stage::Decay => { self.level -= self.d; if self.level <= self.s { self.level = self.s; self.stage = Stage::Sustain; } }
            Stage::Sustain => {}
            Stage::Release => { self.level -= self.d; if self.level <= 0.0 { self.level = 0.0; self.stage = Stage::Done; } }
            Stage::Done => {}
        }
        self.level
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Wave { Sine, Saw, Square, Tri }

fn blep(t: f32, dt: f32) -> f32 {
    if t < dt { let x = t / dt; x + x - x * x - 1.0 }
    else if t > 1.0 - dt { let x = (t - 1.0) / dt; x * x + x + x + 1.0 }
    else { 0.0 }
}

struct Syn { wave: Wave, freq: f32, phase: f32, amp: f32, env: Env, gate: i64, delay: usize }
struct Smp { data: Arc<Sample>, pos: f64, rate: f64, amp: f32, env: Env, gate: i64, delay: usize }
struct Drum { kind: u8, t: f32, phase: f32, amp: f32, rng: u32, prev: f32, delay: usize, len: f32 }

enum Voice { Syn(Syn), Smp(Smp), Drum(Drum) }

fn noise(r: &mut u32) -> f32 {
    *r ^= *r << 13; *r ^= *r >> 17; *r ^= *r << 5;
    (*r as f32 / u32::MAX as f32) * 2.0 - 1.0
}

impl Voice {
    fn alive(&self) -> bool {
        match self {
            Voice::Syn(v) => v.env.stage != Stage::Done,
            Voice::Smp(v) => v.env.stage != Stage::Done && (v.pos as usize) < v.data.data.len(),
            Voice::Drum(v) => v.t < v.len,
        }
    }
    fn release(&mut self) {
        match self { Voice::Syn(v) => v.env.release(), Voice::Smp(v) => v.env.release(), Voice::Drum(_) => {} }
    }
    #[inline]
    fn tick(&mut self, sr: f32) -> f32 {
        match self {
            Voice::Syn(v) => {
                if v.delay > 0 { v.delay -= 1; return 0.0; }
                v.gate -= 1;
                if v.gate == 0 { v.env.release(); }
                let dt = v.freq / sr;
                let p = v.phase;
                let s = match v.wave {
                    Wave::Sine => (p * TAU).sin(),
                    Wave::Saw => 2.0 * p - 1.0 - blep(p, dt),
                    Wave::Square => (if p < 0.5 { 1.0 } else { -1.0 }) + blep(p, dt) - blep((p + 0.5) % 1.0, dt),
                    Wave::Tri => 4.0 * (p - 0.5).abs() - 1.0,
                };
                v.phase += dt;
                if v.phase >= 1.0 { v.phase -= 1.0; }
                s * v.amp * v.env.tick()
            }
            Voice::Smp(v) => {
                if v.delay > 0 { v.delay -= 1; return 0.0; }
                v.gate -= 1;
                if v.gate == 0 { v.env.release(); }
                let i = v.pos as usize;
                let f = (v.pos - i as f64) as f32;
                let d = &v.data.data;
                let a = *d.get(i).unwrap_or(&0.0);
                let b = *d.get(i + 1).unwrap_or(&0.0);
                v.pos += v.rate;
                (a + (b - a) * f) * v.amp * v.env.tick()
            }
            Voice::Drum(v) => {
                if v.delay > 0 { v.delay -= 1; return 0.0; }
                let t = v.t;
                v.t += 1.0 / sr;
                let n = noise(&mut v.rng);
                let hp = n - v.prev;
                v.prev = n;
                let sweep = |v: &mut Drum, f0: f32, f1: f32, k: f32| -> f32 {
                    let f = f1 + (f0 - f1) * (-t * k).exp();
                    v.phase += f / sr;
                    (v.phase * TAU).sin()
                };
                let s = match v.kind {
                    35 | 36 => sweep(v, 170.0, 48.0, 28.0) * (-t * 7.0).exp() * 1.2,
                    38 | 40 => (n * 0.7 * (-t * 17.0).exp()) + sweep(v, 210.0, 170.0, 20.0) * (-t * 24.0).exp() * 0.6,
                    42 | 44 => hp * 0.5 * (-t * 70.0).exp(),
                    46 => hp * 0.45 * (-t * 9.0).exp(),
                    39 => { let b = if (t * 90.0).fract() < 0.5 || t > 0.03 { 1.0 } else { 0.4 }; hp * 0.7 * (-t * 16.0).exp() * b }
                    41 | 43 | 45 | 47 | 48 | 50 => {
                        let base = match v.kind { 41 => 80.0, 43 => 100.0, 45 => 120.0, 47 => 150.0, 48 => 175.0, _ => 200.0 };
                        sweep(v, base * 1.6, base, 18.0) * (-t * 8.0).exp()
                    }
                    49 | 57 => hp * 0.4 * (-t * 2.8).exp(),
                    51 | 59 => (hp * 0.25 + sweep(v, 5200.0, 5200.0, 1.0) * 0.1) * (-t * 6.0).exp(),
                    _ => sweep(v, 900.0, 900.0, 1.0) * (-t * 60.0).exp(),
                };
                s * v.amp
            }
        }
    }
}

// ---------------- effects ----------------

struct Comb { buf: Vec<f32>, i: usize, store: f32 }
impl Comb {
    fn new(n: usize) -> Comb { Comb { buf: vec![0.0; n.max(1)], i: 0, store: 0.0 } }
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.buf[self.i];
        self.store = y * 0.75 + self.store * 0.25;
        self.buf[self.i] = x + self.store * 0.82;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}
struct Allpass { buf: Vec<f32>, i: usize }
impl Allpass {
    fn new(n: usize) -> Allpass { Allpass { buf: vec![0.0; n.max(1)], i: 0 } }
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let b = self.buf[self.i];
        let y = -x + b;
        self.buf[self.i] = x + b * 0.5;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}
struct Reverb { l: [Comb; 4], r: [Comb; 4], al: [Allpass; 2], ar: [Allpass; 2] }
impl Reverb {
    fn new(sr: f32) -> Reverb {
        let k = sr / 44100.0;
        let c = |n: f32, o: f32| Comb::new((n * k + o) as usize);
        let a = |n: f32, o: f32| Allpass::new((n * k + o) as usize);
        Reverb {
            l: [c(1116.0, 0.0), c(1188.0, 0.0), c(1277.0, 0.0), c(1356.0, 0.0)],
            r: [c(1116.0, 23.0), c(1188.0, 23.0), c(1277.0, 23.0), c(1356.0, 23.0)],
            al: [a(556.0, 0.0), a(441.0, 0.0)],
            ar: [a(556.0, 23.0), a(441.0, 23.0)],
        }
    }
    #[inline]
    fn tick(&mut self, x: f32) -> (f32, f32) {
        let x = x * 0.03;
        let mut l: f32 = self.l.iter_mut().map(|c| c.tick(x)).sum();
        let mut r: f32 = self.r.iter_mut().map(|c| c.tick(x)).sum();
        for a in self.al.iter_mut() { l = a.tick(l); }
        for a in self.ar.iter_mut() { r = a.tick(r); }
        (l, r)
    }
}

struct TState {
    name: String,
    notes: Vec<Note>,
    voices: Vec<Voice>,
    ic1: f32,
    ic2: f32,
    dl: Vec<f32>,
    dr: Vec<f32>,
    di: usize,
    reverb: Reverb,
    rng: u32,
}

impl TState {
    fn new(t: &Track, sr: f32) -> TState {
        TState {
            name: t.name.clone(), notes: vec![], voices: vec![], ic1: 0.0, ic2: 0.0,
            dl: vec![0.0; (sr * 4.0) as usize], dr: vec![0.0; (sr * 4.0) as usize], di: 0,
            reverb: Reverb::new(sr), rng: 0x1234_5678 ^ (t.name.len() as u32 * 2654435761),
        }
    }
}

fn auto_value(t: &Track, param: &str, beat: f64, base: f64) -> f64 {
    let Some(l) = t.auto.iter().rev().find(|l| l.param == param) else { return base };
    let p = &l.points;
    if p.is_empty() { return base; }
    if beat <= p[0][0] { return p[0][1]; }
    for w in p.windows(2) {
        if beat < w[1][0] {
            let f = (beat - w[0][0]) / (w[1][0] - w[0][0]).max(1e-9);
            return w[0][1] + (w[1][1] - w[0][1]) * f;
        }
    }
    p[p.len() - 1][1]
}

pub struct Renderer {
    sr: f32,
    song: Song,
    bank: Bank,
    states: Vec<TState>,
    pub pos: f64,
    pub playing: bool,
    pub looping: bool,
    rx: Option<Receiver<Cmd>>,
    shared: Option<Arc<Shared>>,
    scratch: Vec<f32>,
}

impl Renderer {
    pub fn new(sr: f32) -> Renderer {
        Renderer { sr, song: Song::default(), bank: Bank::new(), states: vec![], pos: 0.0, playing: false, looping: true, rx: None, shared: None, scratch: vec![] }
    }

    pub fn set_song(&mut self, song: Song, bank: Bank) {
        let same = self.states.len() == song.tracks.len() && self.states.iter().zip(&song.tracks).all(|(s, t)| s.name == t.name);
        if !same { self.states = song.tracks.iter().map(|t| TState::new(t, self.sr)).collect(); }
        for (s, t) in self.states.iter_mut().zip(&song.tracks) {
            let mut n = t.notes.clone();
            n.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
            s.notes = n;
        }
        self.song = song;
        self.bank = bank;
    }

    fn drain(&mut self) {
        let mut cmds = vec![];
        if let Some(rx) = &self.rx { while let Ok(c) = rx.try_recv() { cmds.push(c); } }
        for c in cmds {
            match c {
                Cmd::SetSong(s, b) => self.set_song(*s, b),
                Cmd::Play => self.playing = true,
                Cmd::Stop => { self.playing = false; for s in &mut self.states { for v in &mut s.voices { v.release(); } } }
                Cmd::Seek(b) => { self.pos = b.max(0.0); for s in &mut self.states { s.voices.clear(); } }
                Cmd::Loop(l) => self.looping = l,
            }
        }
    }

    /// Render interleaved stereo.
    pub fn process(&mut self, out: &mut [f32]) {
        self.drain();
        let frames = out.len() / 2;
        let mut f = 0;
        while f < frames {
            let n = (frames - f).min(BLOCK);
            self.block(&mut out[f * 2..(f + n) * 2]);
            f += n;
        }
        if let Some(sh) = &self.shared {
            sh.pos_beats.store(self.pos.to_bits(), Ordering::Relaxed);
            sh.playing.store(self.playing, Ordering::Relaxed);
            let (mut pl, mut pr) = (0.0f32, 0.0f32);
            for fr in out.chunks(2) { pl = pl.max(fr[0].abs()); pr = pr.max(fr[1].abs()); }
            sh.peak_l.store(pl.to_bits(), Ordering::Relaxed);
            sh.peak_r.store(pr.to_bits(), Ordering::Relaxed);
        }
    }

    fn trigger(&mut self, ti: usize, n: &Note, offset: usize) {
        let sr = self.sr;
        let spb = 60.0 / self.song.bpm as f32 * sr;
        let t = &self.song.tracks[ti];
        let amp = n.vel as f32 / 127.0;
        let gate = ((n.dur as f32 * spb) as i64).max(1);
        let st = &mut self.states[ti];
        if st.voices.len() > 64 { st.voices.remove(0); }
        let v = match t.inst.as_str() {
            "drums" => Voice::Drum(Drum { kind: n.pitch, t: 0.0, phase: 0.0, amp: amp * 0.9, rng: st.rng ^ (n.pitch as u32 + 1) * 7919, prev: 0.0, delay: offset,
                len: match n.pitch { 49 | 57 => 1.6, 46 | 51 | 59 => 0.8, 36 | 35 | 41..=50 => 0.5, _ => 0.3 } }),
            "sampler" => match t.sample.as_ref().and_then(|p| self.bank.get(p)) {
                Some(s) => Voice::Smp(Smp { data: s.clone(), pos: 0.0, rate: 2f64.powf((n.pitch as f64 - t.root as f64) / 12.0) * s.sr as f64 / sr as f64,
                    amp: amp * 0.7, env: Env::new(t.adsr, sr), gate, delay: offset }),
                None => return,
            },
            w => Voice::Syn(Syn {
                wave: match w { "sine" => Wave::Sine, "square" => Wave::Square, "tri" => Wave::Tri, _ => Wave::Saw },
                freq: 440.0 * 2f32.powf((n.pitch as f32 - 69.0) / 12.0), phase: 0.0, amp: amp * 0.22,
                env: Env::new(t.adsr, sr), gate, delay: offset,
            }),
        };
        st.voices.push(v);
    }

    fn block(&mut self, out: &mut [f32]) {
        let n = out.len() / 2;
        out.iter_mut().for_each(|s| *s = 0.0);
        let sr = self.sr;
        let spb = 60.0 / self.song.bpm.max(1.0) * sr as f64;
        let b0 = self.pos;
        let blen = n as f64 / spb;
        let len = self.song.length_beats();

        if self.playing {
            // trigger notes starting in [b0, b1), handling loop wrap
            let mut ranges = vec![(b0, b0 + blen, 0.0)];
            if self.looping && b0 + blen >= len { ranges = vec![(b0, len, 0.0), (0.0, b0 + blen - len, len - b0)]; }
            for ti in 0..self.song.tracks.len() {
                let notes = std::mem::take(&mut self.states[ti].notes);
                for &(lo, hi, shift) in &ranges {
                    let first = notes.partition_point(|x| x.start < lo - 1e-9);
                    for nt in notes[first..].iter().take_while(|x| x.start < hi - 1e-9) {
                        let off = (((nt.start - lo) + shift) * spb) as usize;
                        self.trigger(ti, nt, off.min(n - 1));
                    }
                }
                self.states[ti].notes = notes;
            }
        }

        let any_solo = self.song.tracks.iter().any(|t| t.solo);
        let mut tl = std::mem::take(&mut self.scratch);
        tl.clear();
        tl.resize(n * 2, 0.0);
        for ti in 0..self.song.tracks.len() {
            let t = &self.song.tracks[ti];
            let audible = !t.mute && (!any_solo || t.solo);
            let vol = auto_value(t, "vol", b0, t.vol).clamp(0.0, 2.0) as f32;
            let pan = auto_value(t, "pan", b0, t.pan).clamp(-1.0, 1.0) as f32;
            let cutoff = auto_value(t, "cutoff", b0, t.cutoff).clamp(30.0, 20000.0) as f32;
            let drive = auto_value(t, "drive", b0, t.drive).clamp(0.0, 1.0) as f32;
            let dmix = auto_value(t, "delay", b0, t.delay).clamp(0.0, 1.0) as f32;
            let rmix = auto_value(t, "reverb", b0, t.reverb).clamp(0.0, 1.0) as f32;
            let fb = t.delay_fb.clamp(0.0, 0.95) as f32;
            let st = &mut self.states[ti];
            let dlen = st.dl.len();
            let dsamp = ((t.delay_time * spb) as usize).clamp(1, dlen - 1);
            let theta = (pan + 1.0) * PI / 4.0;
            let (gl, gr) = (theta.cos() * vol, theta.sin() * vol);
            let filt = cutoff < 19000.0;
            let g = (PI * cutoff.min(sr * 0.45) / sr).tan();
            let k = 1.0f32;
            let a1 = 1.0 / (1.0 + g * (g + k));
            let (a2, a3) = (g * a1, g * g * a1);
            let gain = 1.0 + drive * 8.0;
            for i in 0..n {
                let mut x = 0.0;
                for v in st.voices.iter_mut() { x += v.tick(sr); }
                if filt {
                    let v3 = x - st.ic2;
                    let v1 = a1 * st.ic1 + a2 * v3;
                    let v2 = st.ic2 + a2 * st.ic1 + a3 * v3;
                    st.ic1 = 2.0 * v1 - st.ic1;
                    st.ic2 = 2.0 * v2 - st.ic2;
                    x = v2;
                }
                if drive > 0.001 { x = (x * gain).tanh() / gain.sqrt(); }
                let (mut l, mut r) = (x * gl, x * gr);
                if dmix > 0.001 {
                    let rp = (st.di + dlen - dsamp) % dlen;
                    let (dl, dr) = (st.dl[rp], st.dr[rp]);
                    st.dl[st.di] = l + dr * fb; // ping-pong
                    st.dr[st.di] = r + dl * fb;
                    st.di = (st.di + 1) % dlen;
                    l += dl * dmix;
                    r += dr * dmix;
                }
                if rmix > 0.001 {
                    let (rl, rr) = st.reverb.tick((l + r) * 0.5);
                    l += rl * rmix * 3.0;
                    r += rr * rmix * 3.0;
                }
                if audible { tl[i * 2] += l; tl[i * 2 + 1] += r; }
            }
            st.voices.retain(|v| v.alive());
        }
        for (o, s) in out.iter_mut().zip(&tl) { *o = (s * 0.9).tanh(); }
        self.scratch = tl;

        if self.playing {
            self.pos += blen;
            if self.looping && self.pos >= len { self.pos -= len; }
        }
    }
}

// ---------------- offline rendering ----------------

pub fn render_wav(song: &Song, bank: Bank, path: &Path) -> Result<f64, String> {
    let sr = 44100u32;
    let mut r = Renderer::new(sr as f32);
    r.set_song(song.clone(), bank);
    r.playing = true;
    r.looping = false;
    let secs = song.length_beats() * 60.0 / song.bpm + 3.0;
    let total = (secs * sr as f64) as usize;
    if let Some(p) = path.parent() { std::fs::create_dir_all(p).map_err(|e| e.to_string())?; }
    let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    let mut buf = vec![0.0f32; 2048];
    let mut done = 0;
    while done < total {
        r.process(&mut buf);
        for s in &buf { w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|e| e.to_string())?; }
        done += buf.len() / 2;
    }
    w.finalize().map_err(|e| e.to_string())?;
    Ok(secs)
}

// ---------------- CPAL output ----------------

pub struct Engine {
    pub tx: Sender<Cmd>,
    pub shared: Arc<Shared>,
    pub device: String,
    _stream: cpal::Stream,
}

fn build<T>(dev: &cpal::Device, cfg: &cpal::StreamConfig, mut r: Renderer) -> Result<cpal::Stream, String>
where T: cpal::SizedSample + cpal::FromSample<f32> {
    use cpal::traits::DeviceTrait;
    let ch = cfg.channels as usize;
    let mut buf: Vec<f32> = vec![];
    dev.build_output_stream(
        cfg,
        move |data: &mut [T], _| {
            let frames = data.len() / ch;
            buf.resize(frames * 2, 0.0);
            r.process(&mut buf);
            for (i, fr) in data.chunks_mut(ch).enumerate() {
                let (l, rr) = (buf[i * 2], buf[i * 2 + 1]);
                for (c, s) in fr.iter_mut().enumerate() {
                    *s = T::from_sample(if ch == 1 { (l + rr) * 0.5 } else { match c { 0 => l, 1 => rr, _ => 0.0 } });
                }
            }
        },
        |_| {},
        None,
    ).map_err(|e| e.to_string())
}

impl Engine {
    pub fn start() -> Result<Engine, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let dev = host.default_output_device().ok_or("no audio output device")?;
        let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        let shared = Arc::new(Shared::default());
        let mut r = Renderer::new(cfg.sample_rate().0 as f32);
        r.rx = Some(rx);
        r.shared = Some(shared.clone());
        let sc: cpal::StreamConfig = cfg.clone().into();
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&dev, &sc, r)?,
            cpal::SampleFormat::I16 => build::<i16>(&dev, &sc, r)?,
            cpal::SampleFormat::U16 => build::<u16>(&dev, &sc, r)?,
            cpal::SampleFormat::I32 => build::<i32>(&dev, &sc, r)?,
            f => return Err(format!("unsupported sample format {f:?}")),
        };
        stream.play().map_err(|e| e.to_string())?;
        Ok(Engine { tx, shared, device: dev.name().unwrap_or_default(), _stream: stream })
    }
    pub fn send(&self, c: Cmd) { let _ = self.tx.send(c); }
    pub fn pos(&self) -> f64 { f64::from_bits(self.shared.pos_beats.load(Ordering::Relaxed)) }
    pub fn playing(&self) -> bool { self.shared.playing.load(Ordering::Relaxed) }
    pub fn peaks(&self) -> (f32, f32) {
        (f32::from_bits(self.shared.peak_l.load(Ordering::Relaxed)), f32::from_bits(self.shared.peak_r.load(Ordering::Relaxed)))
    }
}
