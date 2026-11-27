//! TypeScript → (type erasure) → QuickJS-NG → Song IR.
use crate::ir::Song;
use crate::ts::strip_types;
use rquickjs::loader::{Loader, Resolver};
use rquickjs::{Context, Ctx, Error, Function, Module, Runtime};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PRELUDE: &str = include_str!("prelude.js");

#[derive(Debug, Clone)]
pub struct ScriptError {
    pub file: String,
    pub line: Option<usize>,
    pub message: String,
}

pub struct ScriptOutput {
    pub song: Song,
    pub logs: Vec<String>,
}

type Files = Arc<HashMap<String, String>>;

/// Normalise `a/b/../c` style paths.
fn normalize(path: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => { out.pop(); }
            s => out.push(s),
        }
    }
    out.join("/")
}

struct FileResolver(Files);
impl Resolver for FileResolver {
    fn resolve<'js>(&mut self, _ctx: &Ctx<'js>, base: &str, name: &str) -> rquickjs::Result<String> {
        let dir = base.rsplit_once('/').map_or("", |(d, _)| d);
        let joined = if name.starts_with("./") || name.starts_with("../") {
            normalize(&format!("{dir}/{name}"))
        } else {
            normalize(name)
        };
        for cand in [joined.clone(), format!("{joined}.ts"), format!("{joined}.js"), format!("{joined}/index.ts")] {
            if self.0.contains_key(&cand) { return Ok(cand); }
        }
        Err(Error::new_resolving_message(base, name, format!("module not found: {name}")))
    }
}

struct FileLoader(Files);
impl Loader for FileLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<Module<'js, rquickjs::module::Declared>> {
        let src = self.0.get(name).ok_or_else(|| Error::new_loading_message(name, "file not found"))?;
        Module::declare(ctx.clone(), name, strip_types(src))
    }
}

fn parse_stack(stack: &str, files: &HashMap<String, String>) -> (Option<String>, Option<usize>) {
    for line in stack.lines() {
        for (name, _) in files.iter() {
            if let Some(pos) = line.find(name.as_str()) {
                let rest = &line[pos + name.len()..];
                if let Some(r) = rest.strip_prefix(':') {
                    let num: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(l) = num.parse() { return (Some(name.clone()), Some(l)); }
                }
            }
        }
    }
    (None, None)
}

pub fn run_project(files: &HashMap<String, String>, entry: &str) -> Result<ScriptOutput, ScriptError> {
    let err = |m: String| ScriptError { file: entry.to_string(), line: None, message: m };
    let files_arc: Files = Arc::new(files.clone());
    let rt = Runtime::new().map_err(|e| err(e.to_string()))?;
    rt.set_memory_limit(256 * 1024 * 1024);
    let started = Instant::now();
    rt.set_interrupt_handler(Some(Box::new(move || started.elapsed() > Duration::from_secs(3))));
    rt.set_loader(FileResolver(files_arc.clone()), FileLoader(files_arc.clone()));
    let ctx = Context::full(&rt).map_err(|e| err(e.to_string()))?;
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let src = files.get(entry).ok_or_else(|| err(format!("entry file not found: {entry}")))?;
    let js = strip_types(src);

    let result: Result<String, ScriptError> = ctx.with(|ctx| {
        let to_err = |ctx: &Ctx, e: Error| -> ScriptError {
            let caught = match e {
                Error::Exception => Some(ctx.catch()),
                _ => None,
            };
            if let Some(v) = caught {
                if let Some(ex) = v.as_exception() {
                    let msg = ex.message().unwrap_or_default();
                    let stack = ex.stack().unwrap_or_default();
                    let (f, l) = parse_stack(&stack, files);
                    let line = l;
                    return ScriptError { file: f.unwrap_or_else(|| entry.to_string()), line, message: msg };
                }
                return ScriptError { file: entry.to_string(), line: None, message: format!("{:?}", v) };
            }
            ScriptError { file: entry.to_string(), line: None, message: e.to_string() }
        };
        let l2 = logs.clone();
        let logf = Function::new(ctx.clone(), move |s: String| { l2.lock().unwrap().push(s); })
            .map_err(|e| to_err(&ctx, e))?;
        ctx.globals().set("__log", logf).map_err(|e| to_err(&ctx, e))?;
        ctx.eval::<(), _>(PRELUDE).map_err(|e| to_err(&ctx, e))?;
        let promise = Module::evaluate(ctx.clone(), entry, js).map_err(|e| to_err(&ctx, e))?;
        promise.finish::<()>().map_err(|e| to_err(&ctx, e))?;
        ctx.eval::<String, _>("JSON.stringify(globalThis.__song)").map_err(|e| to_err(&ctx, e))
    });
    let json = result?;
    let song: Song = serde_json::from_str(&json).map_err(|e| err(format!("invalid song data: {e}")))?;
    song.validate().map_err(|m| err(m))?;
    let logs = logs.lock().unwrap().clone();
    Ok(ScriptOutput { song, logs })
}
