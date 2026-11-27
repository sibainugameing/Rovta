//! Rendering (ratatui).
use crate::app::*;
use crate::ir::*;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

const BG_BLACK: Color = Color::Rgb(22, 22, 30);
const BG_WHITE: Color = Color::Rgb(30, 30, 42);
const BG_BAR: Color = Color::Rgb(44, 44, 62);
const DIM: Color = Color::Rgb(100, 100, 120);

fn st(fg: Color) -> Style { Style::default().fg(fg) }

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.size();
    let rows = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(5), Constraint::Length(1)]).split(area);
    header(f, app, rows[0]);
    tabs(f, app, rows[1]);
    match app.mode {
        Mode::Tui => match app.view {
            View::Mixer => mixer(f, app, rows[2]),
            v => {
                let cols = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(20), Constraint::Min(20)]).split(rows[2]);
                track_list(f, app, cols[0]);
                if v == View::Roll { roll(f, app, cols[1]) } else { steps(f, app, cols[1]) }
            }
        },
        Mode::Code => code(f, app, rows[2]),
    }
    footer(f, app, rows[3]);
    if let Some(sel) = app.picker { picker(f, sel); }
}

fn picker(f: &mut Frame, sel: usize) {
    let area = f.size();
    let w = 62u16.min(area.width);
    let h = (TRACK_KINDS.len() as u16 + 4).min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height.saturating_sub(h)) / 2, w, h);
    f.render_widget(Clear, r);
    let mut lines = vec![];
    for (i, (name, _, desc)) in TRACK_KINDS.iter().enumerate() {
        let on = i == sel;
        let base = if on { Style::default().fg(Color::Black).bg(Color::Cyan) } else { Style::default() };
        lines.push(Line::from(vec![
            Span::styled(format!(" {} {:<18}", i + 1, name), base.add_modifier(Modifier::BOLD)),
            Span::styled(format!("{desc:<w$}", w = (w as usize).saturating_sub(23)), if on { base } else { st(DIM) }),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(" ↑↓ / 1-6 choose   Enter add   Esc cancel", st(DIM)));
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).border_style(st(Color::Cyan)).title(" New track — choose a type ")), r);
}

fn header(f: &mut Frame, app: &App, a: Rect) {
    let pos = app.play_pos();
    let bpb = app.bar_beats();
    let playing = app.is_playing();
    let (l, r) = app.meter;
    let bar = |v: f32| { let n = ((v.min(1.0)).sqrt() * 8.0).round() as usize; format!("{}{}", "█".repeat(n), "·".repeat(8 - n)) };
    let mut s = vec![
        Span::styled(" Rovta ", Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(" TUI ", if app.mode == Mode::Tui { Style::default().fg(Color::Black).bg(Color::Yellow) } else { st(DIM) }),
        Span::styled(" CODE ", if app.mode == Mode::Code { Style::default().fg(Color::Black).bg(Color::Yellow) } else { st(DIM) }),
        Span::raw(format!("  ♩={}  {}/{}  ", app.song.bpm, app.song.time_sig[0], app.song.time_sig[1])),
        Span::styled(if playing { "▶ " } else { "■ " }, st(if playing { Color::Green } else { Color::Gray })),
        Span::raw(format!("{:02}.{:.1}  ", (pos / bpb) as u32 + 1, pos % bpb + 1.0)),
        Span::styled(format!("L{} R{}", bar(l), bar(r)), st(if l.max(r) > 0.95 { Color::Red } else { Color::Green })),
    ];
    if !app.song.editable { s.push(Span::styled("  (tempo: code)", st(DIM))); }
    f.render_widget(Paragraph::new(Line::from(s)), a);
}

fn tabs(f: &mut Frame, app: &App, a: Rect) {
    let mut s = vec![Span::raw(" ")];
    if app.mode == Mode::Tui {
        for (i, (v, n)) in [(View::Roll, "1 Piano Roll"), (View::Steps, "2 Step Seq"), (View::Mixer, "3 Mixer")].iter().enumerate() {
            let _ = i;
            s.push(Span::styled(format!(" {n} "), if *v == app.view { Style::default().fg(Color::Black).bg(Color::White) } else { st(Color::Gray) }));
            s.push(Span::raw(" "));
        }
    } else {
        for (i, fl) in app.project.files.iter().enumerate() {
            let label = format!(" {}{} ", fl.path, if fl.dirty { "•" } else { "" });
            s.push(Span::styled(label, if i == app.cfile { Style::default().fg(Color::Black).bg(Color::White) } else { st(Color::Gray) }));
            s.push(Span::raw(" "));
        }
    }
    f.render_widget(Paragraph::new(Line::from(s)), a);
}

fn footer(f: &mut Frame, app: &App, a: Rect) {
    if let Some(p) = &app.prompt {
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled(format!(" {}: ", p.label), st(Color::Yellow)), Span::raw(p.buf.clone()), Span::styled("█", st(Color::Gray)),
        ])), a);
        return;
    }
    let style = if app.status_err { Style::default().fg(Color::White).bg(Color::Red) } else { st(Color::Gray) };
    let hint = match app.mode {
        Mode::Tui => "  u:undo  Tab:CODE  ?:help",
        Mode::Code => "  ^S save+run  ^R run  ^G play/stop  ^Space complete  ^O/^T file  Esc→TUI",
    };
    let mut t = format!(" {}", app.status);
    let w = a.width as usize;
    if t.width() + hint.width() < w { t.push_str(&" ".repeat(w - t.width() - hint.width())); t.push_str(hint); }
    f.render_widget(Paragraph::new(Span::styled(t, style)), a);
}

fn track_list(f: &mut Frame, app: &App, a: Rect) {
    let mut lines = vec![];
    for (i, t) in app.song.tracks.iter().enumerate() {
        let sel = i == app.sel;
        let mut flags = String::new();
        if t.mute { flags.push('M'); }
        if t.solo { flags.push('S'); }
        if !t.editable { flags.push('*'); }
        let name: String = t.name.chars().take(13).collect();
        lines.push(Line::from(vec![
            Span::styled(if sel { "▸" } else { " " }, st(Color::Yellow)),
            Span::styled(format!("{:<13}", name), if sel { Style::default().fg(Color::White).add_modifier(Modifier::BOLD) } else { st(Color::Gray) }),
            Span::styled(format!(" {:<3}", flags), st(Color::Magenta)),
        ]));
    }
    if lines.is_empty() { lines.push(Line::styled(" (no tracks) n: add", st(DIM))); }
    lines.push(Line::raw(""));
    lines.push(Line::styled(" * = code-owned", st(DIM)));
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::RIGHT).border_style(st(DIM)).title(" Tracks ")), a);
}

fn roll(f: &mut Frame, app: &mut App, a: Rect) {
    let kw = 5u16;
    let cols = ((a.width.saturating_sub(kw + 1)) / 2).max(4) as usize;
    let nrows = a.height.saturating_sub(2).max(3) as usize;
    app.roll_cols = cols;
    app.roll_rows = nrows;
    let g = app.grid;
    // follow cursor
    if app.cur_step < app.scroll_x { app.scroll_x = (app.cur_step / app.bar_beats()).floor() * app.bar_beats(); }
    if app.cur_step >= app.scroll_x + cols as f64 * g - g * 2.0 { app.scroll_x = (app.cur_step - (cols as f64 - 4.0) * g).max(0.0); app.scroll_x = (app.scroll_x / g).round() * g; }
    if app.cur_pitch < app.base_pitch { app.base_pitch = app.cur_pitch; }
    if app.cur_pitch as usize >= app.base_pitch as usize + nrows { app.base_pitch = (app.cur_pitch as usize + 1 - nrows) as u8; }
    let base = app.base_pitch as usize;
    let bpb = app.bar_beats();
    let sx = app.scroll_x;
    // paint notes
    let mut grid: Vec<Option<(bool, u8)>> = vec![None; cols * nrows];
    let tr = app.track();
    if let Some(t) = tr {
        for n in &t.notes {
            let p = n.pitch as usize;
            if p < base || p >= base + nrows { continue; }
            let row = nrows - 1 - (p - base);
            let c0 = ((n.start - sx) / g).round() as i64;
            let c1 = (((n.start + n.dur - sx) / g).round() as i64).max(c0 + 1);
            for c in c0.max(0)..c1.min(cols as i64) { grid[row * cols + c as usize] = Some((c == c0, n.vel)); }
        }
    }
    let play = app.play_pos();
    let playing = app.is_playing();
    let mut lines = vec![];
    // ruler
    let mut ruler = vec![Span::raw(" ".repeat(kw as usize))];
    for c in 0..cols {
        let b = sx + c as f64 * g;
        let on_bar = (b % bpb).abs() < 1e-6 || (b % bpb - bpb).abs() < 1e-6;
        let on_beat = (b % 1.0).abs() < 1e-6;
        let s = if on_bar { format!("{:<2}", (b / bpb).round() as i64 + 1) } else if on_beat && g <= 0.5 { "' ".to_string() } else { "  ".to_string() };
        ruler.push(Span::styled(s, st(if on_bar { Color::Yellow } else { DIM })));
    }
    lines.push(Line::from(ruler));
    for r in 0..nrows {
        let p = (base + nrows - 1 - r) as u8;
        let black = is_black(p);
        let label = if app.track().map_or(false, |t| t.is_drums()) {
            DRUM_LANES.iter().find(|d| d.0 == p).map_or(note_name(p), |d| d.1.to_string())
        } else { note_name(p) };
        let mut spans = vec![Span::styled(format!("{:>4} ", label), if p == app.cur_pitch { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { st(if black { DIM } else { Color::Gray }) })];
        for c in 0..cols {
            let b = sx + c as f64 * g;
            let cursor = p == app.cur_pitch && (b - app.cur_step).abs() < g / 2.0;
            let bg = if playing && play >= b && play < b + g { Color::Rgb(70, 30, 30) } else if black { BG_BLACK } else { BG_WHITE };
            let on_bar = (b % bpb).abs() < 1e-6;
            let on_beat = (b % 1.0).abs() < 1e-6;
            let (txt, fg) = match grid[r * cols + c] {
                Some((start, vel)) => {
                    let sh = 90 + vel as u16 * 165 / 127;
                    (if start { "█▌" } else { "██" }, Color::Rgb(40, sh as u8, 200u8.saturating_sub(vel / 2)))
                }
                None => (if on_bar { "│ " } else if on_beat { "· " } else { "  " }, if on_bar { Color::Rgb(120, 120, 150) } else { DIM }),
            };
            let style = if cursor { Style::default().fg(Color::Black).bg(Color::Yellow) } else { Style::default().fg(fg).bg(bg) };
            let txt = if cursor && grid[r * cols + c].is_none() { "▏ " } else { txt };
            spans.push(Span::styled(txt, style));
        }
        lines.push(Line::from(spans));
    }
    let info = tr.map_or("no track".to_string(), |t| format!(
        "{} [{}]  cursor {}  grid 1/{}  len {}", t.name, t.inst, note_name(app.cur_pitch),
        (4.0 / g) as u32, app.last_dur));
    lines.push(Line::styled(format!(" {info}   Enter:note l/L:len v/V:vel g:grid ←→↑↓ move PgUp/PgDn bar Shift+↑↓ octave"), st(DIM)));
    f.render_widget(Paragraph::new(lines), a);
}

fn steps(f: &mut Frame, app: &mut App, a: Rect) {
    let bpb = app.bar_beats();
    let nsteps = (bpb * 4.0).round() as usize;
    let bar = (app.cur_step / bpb).floor();
    let rows = app.step_rows();
    let play = app.play_pos();
    let playing = app.is_playing();
    let drums = app.track().map_or(false, |t| t.is_drums());
    let mut lines = vec![Line::styled(format!(" Bar {}   (1 step = 1/16 note; PgUp/PgDn = prev/next bar)", bar as i64 + 1), st(Color::Yellow)), Line::raw("")];
    let mut head = vec![Span::raw("       ")];
    for s in 0..nsteps { head.push(Span::styled(format!("{:<3}{}", if s % 4 == 0 { (s / 4 + 1).to_string() } else { String::new() }, if s % 4 == 3 { " " } else { "" }), st(DIM))); }
    lines.push(Line::from(head));
    for p in &rows {
        let label = if drums { DRUM_LANES.iter().find(|d| d.0 == *p).map_or(note_name(*p), |d| d.1.to_string()) } else { note_name(*p) };
        let mut sp = vec![Span::styled(format!("{:>5}  ", label), if *p == app.cur_pitch { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { st(Color::Gray) })];
        for s in 0..nsteps {
            let b = bar * bpb + s as f64 * 0.25;
            let on = app.track().map_or(false, |t| t.notes.iter().any(|n| n.pitch == *p && (n.start - b).abs() < 1e-6));
            let cursor = *p == app.cur_pitch && (b - app.cur_step).abs() < 1e-6;
            let head = playing && play >= b && play < b + 0.25;
            let bg = if cursor { Color::Yellow } else if head { Color::Rgb(90, 40, 40) } else if (s / 4) % 2 == 0 { BG_WHITE } else { BG_BAR };
            let fg = if cursor { Color::Black } else if on { Color::Rgb(60, 230, 140) } else { DIM };
            sp.push(Span::styled(if on { "■ " } else { "· " }, Style::default().fg(fg).bg(bg)));
            sp.push(Span::styled(if on { "" } else { "" }, Style::default()));
            sp.push(Span::raw(if s % 4 == 3 { " " } else { "" }));
        }
        lines.push(Line::from(sp));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(if drums { " Enter/x: toggle hit   ←→ step   ↑↓ lane" } else { " Enter/x: toggle   ←→ step   ↑↓ row   Shift+↑↓ octave" }, st(DIM)));
    f.render_widget(Paragraph::new(lines), a);
}

fn mixer(f: &mut Frame, app: &App, a: Rect) {
    let cw = 14usize;
    let mut lines = vec![Line::raw("")];
    let mut head = vec![Span::raw("  Param    ")];
    for (i, t) in app.song.tracks.iter().enumerate() {
        let nm: String = t.name.chars().take(cw - 3).collect();
        let flags = format!("{}{}{}", if t.mute { "M" } else { "" }, if t.solo { "S" } else { "" }, if t.editable { "" } else { "*" });
        head.push(Span::styled(format!("{:<w$}", format!("{nm} {flags}"), w = cw), if i == app.sel { Style::default().fg(Color::Black).bg(Color::Yellow) } else { st(Color::White) }));
    }
    lines.push(Line::from(head));
    let bar = |v: f64, lo: f64, hi: f64| { let n = (((v - lo) / (hi - lo)).clamp(0.0, 1.0) * 6.0).round() as usize; format!("{}{}", "▮".repeat(n), "▯".repeat(6 - n)) };
    for (pi, name) in MIX_PARAMS.iter().enumerate() {
        let mut sp = vec![Span::styled(format!("  {:<8} ", name), if pi == app.mix_param { st(Color::Yellow) } else { st(Color::Gray) })];
        for (ti, t) in app.song.tracks.iter().enumerate() {
            let txt = match pi {
                0 => format!("{} {:.2}", bar(t.vol, 0.0, 1.5), t.vol),
                1 => format!("{} {:+.2}", bar(t.pan, -1.0, 1.0), t.pan),
                2 => format!("{:<w$}", t.inst, w = cw - 2),
                3 => format!("{} {}", bar(t.cutoff.ln(), 40f64.ln(), 20000f64.ln()), if t.cutoff >= 20000.0 { "open".to_string() } else { format!("{}", t.cutoff as u32) }),
                4 => format!("{} {:.2}", bar(t.drive, 0.0, 1.0), t.drive),
                5 => format!("{} {:.2}", bar(t.delay, 0.0, 1.0), t.delay),
                6 => format!("{} {:.2}", bar(t.reverb, 0.0, 1.0), t.reverb),
                7 => format!("{} {:.3}", bar(t.adsr[0], 0.0, 1.0), t.adsr[0]),
                _ => format!("{} {:.2}", bar(t.adsr[3], 0.0, 2.0), t.adsr[3]),
            };
            let sel = ti == app.sel && pi == app.mix_param;
            let style = if sel { Style::default().fg(Color::Black).bg(Color::Cyan) } else if t.editable { st(Color::Green) } else { st(DIM) };
            sp.push(Span::styled(format!("{:<w$}", txt, w = cw), style));
        }
        lines.push(Line::from(sp));
    }
    lines.push(Line::raw(""));
    let auto: Vec<String> = app.song.tracks.iter().filter(|t| !t.auto.is_empty()).map(|t| format!("{}: {}", t.name, t.auto.iter().map(|l| l.param.clone()).collect::<Vec<_>>().join("/"))).collect();
    lines.push(Line::styled(format!("  Automation (code): {}", if auto.is_empty() { "—".into() } else { auto.join("   ") }), st(DIM)));
    lines.push(Line::styled("  ←→ track  ↑↓ param  +/- adjust  PgUp/PgDn coarse  m mute  s solo   (green = TUI-editable, grey = code-owned)", st(DIM)));
    f.render_widget(Paragraph::new(lines), a);
}

// ---------------- code view ----------------

fn highlight(line: &str) -> Vec<(String, Style)> {
    const KW: [&str; 24] = ["const", "let", "var", "function", "return", "if", "else", "for", "while", "of", "in", "import", "from", "export", "default", "new", "type", "interface", "as", "class", "true", "false", "null", "undefined"];
    let mut out = vec![];
    let ch: Vec<char> = line.chars().collect();
    let mut i = 0;
    if line.trim_start().starts_with("// @") {
        return vec![(line.to_string(), Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD))];
    }
    while i < ch.len() {
        let c = ch[i];
        let start = i;
        let style;
        if c == '/' && ch.get(i + 1) == Some(&'/') { i = ch.len(); style = st(Color::Rgb(110, 130, 110)); }
        else if c == '"' || c == '\'' || c == '`' {
            i += 1;
            while i < ch.len() && ch[i] != c { if ch[i] == '\\' { i += 1; } i += 1; }
            i = (i + 1).min(ch.len());
            style = st(Color::Rgb(230, 180, 110));
        } else if c.is_ascii_digit() {
            while i < ch.len() && (ch[i].is_ascii_alphanumeric() || ch[i] == '.') { i += 1; }
            style = st(Color::Rgb(150, 200, 255));
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            while i < ch.len() && (ch[i].is_alphanumeric() || ch[i] == '_' || ch[i] == '$') { i += 1; }
            let w: String = ch[start..i].iter().collect();
            style = if KW.contains(&w.as_str()) { st(Color::Rgb(200, 130, 240)) }
                else if matches!(w.as_str(), "track" | "bpm" | "timeSig" | "n" | "chord" | "scale") && ch.get(i) == Some(&'(') { st(Color::Cyan) }
                else { Style::default() };
        } else { i += 1; style = Style::default().fg(Color::Rgb(170, 170, 190)); }
        out.push((ch[start..i].iter().collect(), style));
    }
    out
}

fn code(f: &mut Frame, app: &mut App, a: Rect) {
    let panel_h = 6u16.min(a.height / 3);
    let parts = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(3), Constraint::Length(panel_h)]).split(a);
    let ed = parts[0];
    let gw = 5usize;
    let w = ed.width as usize;
    let h = ed.height as usize;
    app.ed_w = w.saturating_sub(gw);
    app.ed_h = h;
    let err_line = app.error.as_ref().and_then(|e| e.line.map(|l| (e.file.clone(), l)));
    let comp_info = app.comp.as_ref().map(|c| (c.items.clone(), c.sel));
    let idx = app.cfile.min(app.project.files.len() - 1);
    let file = &mut app.project.files[idx];
    // scroll
    if file.cur.0 < file.scroll { file.scroll = file.cur.0; }
    if file.cur.0 >= file.scroll + h { file.scroll = file.cur.0 + 1 - h; }
    let curline: String = file.lines[file.cur.0].chars().take(file.cur.1).collect();
    let cx = curline.width();
    let tw = w.saturating_sub(gw);
    if cx < file.hscroll { file.hscroll = cx; }
    if cx >= file.hscroll + tw { file.hscroll = cx + 1 - tw; }
    let mut lines = vec![];
    let mut in_region = false;
    for (i, l) in file.lines.iter().enumerate() {
        if l.trim().starts_with("// @tui") { in_region = true; }
        let shade = in_region;
        if l.trim() == "// @end" { in_region = false; }
        if i < file.scroll || i >= file.scroll + h { continue; }
        let is_err = err_line.as_ref().map_or(false, |(fl, ln)| *fl == file.path && *ln == i + 1);
        let mut spans = vec![Span::styled(format!("{:>3}{} ", i + 1, if is_err { "!" } else { " " }), if is_err { Style::default().fg(Color::White).bg(Color::Red) } else { st(DIM) })];
        // horizontal scroll by display columns
        let mut skipped = 0usize;
        for (txt, style) in highlight(l) {
            let mut piece = String::new();
            for c in txt.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if skipped < file.hscroll { skipped += cw; continue; }
                piece.push(c);
            }
            let style = if shade { style.bg(Color::Rgb(26, 30, 40)) } else { style };
            spans.push(Span::styled(piece, style));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), ed);
    let cy = ed.y + (file.cur.0 - file.scroll) as u16;
    let cxs = ed.x + gw as u16 + (cx - file.hscroll) as u16;
    if app.prompt.is_none() { f.set_cursor(cxs.min(ed.x + ed.width.saturating_sub(1)), cy); }
    // completion popup
    if let Some((items, sel)) = comp_info {
        let pw = items.iter().map(|(a, b)| a.width() + b.width() + 4).max().unwrap_or(20).min(60) as u16;
        let ph = items.len() as u16 + 2;
        let px = cxs.min(f.size().width.saturating_sub(pw));
        let py = if cy + 1 + ph <= ed.y + ed.height { cy + 1 } else { cy.saturating_sub(ph) };
        let r = Rect::new(px, py, pw, ph.min(f.size().height.saturating_sub(py)));
        f.render_widget(Clear, r);
        let ls: Vec<Line> = items.iter().enumerate().map(|(i, (a, b))| Line::from(vec![
            Span::styled(format!(" {a:<14}"), if i == sel { Style::default().fg(Color::Black).bg(Color::Cyan) } else { Style::default().fg(Color::White) }),
            Span::styled(format!(" {b}"), if i == sel { Style::default().fg(Color::Black).bg(Color::Cyan) } else { st(DIM) }),
        ])).collect();
        f.render_widget(Paragraph::new(ls).block(Block::default().borders(Borders::ALL).border_style(st(Color::Cyan))), r);
    }
    // problems / log panel
    let mut pl = vec![];
    match &app.error {
        Some(e) => {
            pl.push(Line::styled(format!(" ✗ {}{}", e.file, e.line.map_or(String::new(), |l| format!(":{l}"))), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)));
            for m in e.message.lines().take(3) { pl.push(Line::styled(format!("   {m}"), st(Color::Red))); }
        }
        None => pl.push(Line::styled(" ✓ no errors", st(Color::Green))),
    }
    let room = (panel_h as usize).saturating_sub(pl.len() + 1);
    for l in app.logs.iter().rev().take(room).rev() { pl.push(Line::styled(format!(" > {l}"), st(Color::Gray))); }
    f.render_widget(Paragraph::new(pl).block(Block::default().borders(Borders::TOP).border_style(st(DIM)).title(" Problems / Console ")), parts[1]);
}
