//! Standard MIDI File import / export.
use crate::ir::*;
use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use std::collections::HashMap;
use std::path::Path;

const PPQ: u16 = 480;

pub fn export(song: &Song, path: &Path) -> Result<(), String> {
    let mut tracks: Vec<Vec<TrackEvent>> = vec![];
    let den_pow = (song.time_sig[1].max(1) as f32).log2().round() as u8;
    tracks.push(vec![
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new((60_000_000.0 / song.bpm) as u32))) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TimeSignature(song.time_sig[0] as u8, den_pow, 24, 8)) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) },
    ]);
    let mut melodic = 0u8;
    for t in &song.tracks {
        let ch = if t.is_drums() { 9 } else { let c = melodic % 15; melodic += 1; if c >= 9 { c + 1 } else { c } };
        let mut ev: Vec<(u32, u8, u8, u8)> = vec![]; // (tick, order, key, vel); order 0 = off, 1 = on
        for n in &t.notes {
            let on = (n.start * PPQ as f64).round() as u32;
            let off = on + ((n.dur * PPQ as f64).round() as u32).max(1);
            ev.push((on, 1, n.pitch, n.vel));
            ev.push((off, 0, n.pitch, 0));
        }
        ev.sort();
        let mut out = vec![TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(t.name.as_bytes())) }];
        let mut last = 0;
        for (tick, order, key, vel) in ev {
            let message = if order == 1 { MidiMessage::NoteOn { key: u7::new(key), vel: u7::new(vel) } } else { MidiMessage::NoteOff { key: u7::new(key), vel: u7::new(0) } };
            out.push(TrackEvent { delta: u28::new(tick - last), kind: TrackEventKind::Midi { channel: u4::new(ch), message } });
            last = tick;
        }
        out.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
        tracks.push(out);
    }
    let smf = Smf { header: Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ))), tracks };
    if let Some(p) = path.parent() { std::fs::create_dir_all(p).map_err(|e| e.to_string())?; }
    smf.save(path).map_err(|e| e.to_string())
}

pub struct Imported { pub bpm: Option<f64>, pub time_sig: Option<[u32; 2]>, pub tracks: Vec<Track> }

pub fn import(path: &Path) -> Result<Imported, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let smf = Smf::parse(&bytes).map_err(|e| e.to_string())?;
    let ppq = match smf.header.timing {
        Timing::Metrical(t) => t.as_int() as f64,
        _ => return Err("SMPTE timecode MIDI files are not supported".into()),
    };
    let (mut bpm, mut sig) = (None, None);
    let mut tracks = vec![];
    for (ti, tr) in smf.tracks.iter().enumerate() {
        let mut tick = 0u64;
        let mut name = String::new();
        let mut open: HashMap<(u8, u8), (u64, u8)> = HashMap::new();
        let mut notes = vec![];
        let mut drums = false;
        for e in tr {
            tick += e.delta.as_int() as u64;
            match &e.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(us)) if bpm.is_none() => bpm = Some(60_000_000.0 / us.as_int() as f64),
                TrackEventKind::Meta(MetaMessage::TimeSignature(n, d, _, _)) if sig.is_none() => sig = Some([*n as u32, 1u32 << *d]),
                TrackEventKind::Meta(MetaMessage::TrackName(b)) if name.is_empty() => name = String::from_utf8_lossy(b).trim().to_string(),
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    let (key, vel, on) = match message {
                        MidiMessage::NoteOn { key, vel } => (key.as_int(), vel.as_int(), vel.as_int() > 0),
                        MidiMessage::NoteOff { key, .. } => (key.as_int(), 0, false),
                        _ => continue,
                    };
                    if on {
                        if ch == 9 { drums = true; }
                        open.insert((ch, key), (tick, vel));
                    } else if let Some((t0, v)) = open.remove(&(ch, key)) {
                        notes.push(Note { start: t0 as f64 / ppq, dur: ((tick - t0) as f64 / ppq).max(1.0 / 16.0), pitch: key, vel: v });
                    }
                }
                _ => {}
            }
        }
        if notes.is_empty() { continue; }
        let mut t = Track::new(if name.is_empty() { "midi" } else { &name }, if drums { "drums" } else { "saw" });
        if name.is_empty() { t.name = format!("midi {}", ti); }
        t.notes = notes;
        tracks.push(t);
    }
    if tracks.is_empty() { return Err("no notes found in MIDI file".into()); }
    Ok(Imported { bpm, time_sig: sig, tracks })
}
