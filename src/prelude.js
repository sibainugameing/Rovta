// Rovta prelude: the music-description API exposed to user scripts.
(function (g) {
  const song = { bpm: 120, timeSig: [4, 4], tracks: [] };
  g.__song = song;
  const fmt = (v) => (typeof v === "string" ? v : (() => { try { return JSON.stringify(v); } catch (e) { return String(v); } })());
  g.console = { log: (...a) => __log(a.map(fmt).join(" ")), warn: (...a) => __log("warn: " + a.map(fmt).join(" ")), error: (...a) => __log("error: " + a.map(fmt).join(" ")) };

  const NAMES = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 };
  g.n = function (x) {
    if (typeof x === "number") return Math.round(x);
    const m = /^([A-Ga-g])([#b]?)(-?\d+)$/.exec(String(x));
    if (!m) throw new Error("invalid note name: " + x);
    let v = NAMES[m[1].toUpperCase()];
    if (m[2] === "#") v++; else if (m[2] === "b") v--;
    return v + (parseInt(m[3], 10) + 1) * 12;
  };
  const CHORDS = { maj: [0, 4, 7], min: [0, 3, 7], dim: [0, 3, 6], aug: [0, 4, 8], sus2: [0, 2, 7], sus4: [0, 5, 7],
    maj7: [0, 4, 7, 11], min7: [0, 3, 7, 10], dom7: [0, 4, 7, 10] };
  const SCALES = { major: [0, 2, 4, 5, 7, 9, 11], minor: [0, 2, 3, 5, 7, 8, 10], pentatonic: [0, 3, 5, 7, 10],
    dorian: [0, 2, 3, 5, 7, 9, 10], blues: [0, 3, 5, 6, 7, 10] };
  g.chord = (root, type = "maj") => { if (!CHORDS[type]) throw new Error("unknown chord type: " + type); return CHORDS[type].map((i) => n(root) + i); };
  g.scale = (root, type = "major", octaves = 1) => {
    const iv = SCALES[type]; if (!iv) throw new Error("unknown scale: " + type);
    const out = []; for (let o = 0; o < octaves; o++) for (const i of iv) out.push(n(root) + i + 12 * o);
    return out;
  };
  g.bpm = (v) => { song.bpm = Number(v); };
  g.timeSig = (num, den) => { song.timeSig = [num, den]; };

  const INSTS = ["analog", "sine", "saw", "square", "tri", "drums", "sampler"];
  const PARAMS = ["vol", "volume", "pan", "cutoff", "drive", "delay", "reverb"];
  class Track {
    constructor(data) { this.data = data; }
    note(start, dur, pitch, vel = 100) {
      const p = n(pitch);
      if (!(p >= 0 && p <= 127)) throw new Error("pitch out of range 0-127: " + pitch);
      if (!(dur > 0)) throw new Error("note duration must be > 0");
      if (!(start >= 0)) throw new Error("note start must be >= 0");
      this.data.notes.push({ start, dur, pitch: p, vel: Math.max(1, Math.min(127, Math.round(vel))) });
      return this;
    }
    notes(list) { for (const x of list) this.note(x[0], x[1], x[2], x.length > 3 ? x[3] : 100); return this; }
    chord(start, dur, pitches, vel = 100) { for (const p of pitches) this.note(start, dur, p, vel); return this; }
    // steps("x...x...x.x.x...", "C2", { start: 0, step: 0.25 })   x = hit, X = accent, . or - = rest
    steps(pattern, pitch, o = {}) {
      const step = o.step ?? 0.25, start = o.start ?? 0, vel = o.vel ?? 100, dur = o.dur ?? step;
      let i = 0;
      for (const c of String(pattern)) {
        if (c === " " || c === "|") continue;
        if (c === "x") this.note(start + i * step, dur, pitch, vel);
        else if (c === "X") this.note(start + i * step, dur, pitch, 127);
        i++;
      }
      return this;
    }
    auto(param, points) {
      if (!PARAMS.includes(param)) throw new Error("cannot automate '" + param + "' (" + PARAMS.join(", ") + ")");
      const pts = points.map((p) => [Number(p[0]), Number(p[1])]).sort((a, b) => a[0] - b[0]);
      this.data.auto.push({ param: param === "volume" ? "vol" : param, points: pts });
      return this;
    }
    set(o) { Object.assign(this.data, o); return this; }
  }
  g.track = function (name, opts = {}) {
    name = String(name);
    if (!name.trim() || /[\r\n]/.test(name)) throw new Error("invalid track name");
    if (song.tracks.some((t) => t.name === name)) throw new Error("duplicate track name: " + name);
    const inst = opts.inst ?? "saw";
    if (!INSTS.includes(inst)) throw new Error("unknown instrument '" + inst + "' (" + INSTS.join(", ") + ")");
    if (inst === "sampler" && !opts.sample) throw new Error("sampler track needs `sample: \"path.wav\"`");
    const data = Object.assign({ name, notes: [], auto: [] }, opts, { inst });
    song.tracks.push(data);
    return new Track(data);
  };
})(globalThis);
