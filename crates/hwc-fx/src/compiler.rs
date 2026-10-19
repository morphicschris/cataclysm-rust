//! The effect compiler: a port of `etgEffectCodeLoad` and its parse functions.
//!
//! Effect files (`.ebg`) are C-preprocessed text. The compiler reads them line by line,
//! expands `[expr]` arithmetic, and matches each line against the parse table (block
//! structure, declarations, time tests, labels...), then the function table, then variable
//! assignment. Code goes into three blocks (`startup`, `eachFrame`, `timeIndex`) in the
//! original binary layout (see [`crate::bytecode`]).
//!
//! The original's quirks are kept: case-insensitive symbol lookup (Cataclysm uses `_stricmp`),
//! `x <- -5` assigning `5` (the minus is a `strtok` delimiter), repeated blocks of the same
//! kind concatenating (re-opening a block removes the previous trailing `End`),
//! `modifyTexture` losing its system argument, `call` to an already-defined label jumping to
//! the label's variable offset, and so on. Situations where the original would dereference a
//! null symbol are reported as errors.

use std::collections::HashMap;

use crate::bytecode::{self, evt, function_size, op, rd, wr};
use crate::cfmt::{self, is_number, names_match, scan_float_bits, scan_int, StrTok};
use crate::eval;
use crate::functions::{param_type, Fun, Resolve, FUNCTION_TABLE};

/// `ETG_NewParticleLength`: default number of particle-block slots.
pub const DEFAULT_PARTICLE_BLOCKS: u32 = 8;
/// `EH_DefaultSize`.
pub const HISTORY_DEFAULT: i32 = -1;
/// `ESO_SelfDeleting`.
pub const ESO_SELF_DELETING: u32 = 1;
/// `ESO_SortForward`.
pub const ESO_SORT_FORWARD: u32 = 2;
/// `ESO_WorldRender`.
pub const ESO_WORLD_RENDER: u32 = 4;
/// Line buffer length passed to `fileLineRead` (`STRING_LENGTH`).
const LINE_LENGTH: usize = 256;
/// `REALlyNegative` as stored in history lists.
const HISTORY_EMPTY: f32 = -1.0e21;

/// Parse modes (`EPM_*`).
const MODE_VARIABLES: usize = 3;
const MODE_NOTHING: usize = 64;

/// A compile error: the situations where the original would crash or report a fatal error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError {
    pub file: String,
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.message)
    }
}

impl std::error::Error for CompileError {}

/// An `alternate` decision record (`etgalternate`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    pub kind: u32,
    pub low: u32,
    pub high: u32,
}

/// A loaded effect (`etgeffectstatic`).
#[derive(Clone, Debug, Default)]
pub struct EffectStatic {
    /// File name without directories, as registered.
    pub name: String,
    /// Index of the software-renderer variant, if any (`SoftwareVersion`).
    pub software_version: Option<usize>,
    /// Number of particle-block slots (`0xffffffff` while not loaded).
    pub n_particle_blocks: u32,
    pub variable_size: u32,
    pub rate_size: u32,
    pub effect_size: u32,
    /// `(variable index, initial value)` pairs applied at effect start.
    pub var_init: Vec<(u8, u32)>,
    pub const_data: Vec<u8>,
    pub decisions: Vec<Decision>,
    pub alternate_offsets: Vec<u32>,
    /// Startup, each-frame and time-index code.
    pub code: [Vec<u8>; 3],
    pub n_history: i32,
    pub i_history: i32,
    pub history: Vec<f32>,
    pub special_ops: u32,
}

impl EffectStatic {
    fn blank(name: &str) -> Self {
        Self { name: name.to_string(), n_particle_blocks: 0xffff_ffff, n_history: HISTORY_DEFAULT, ..Default::default() }
    }

    /// Whether the effect has been loaded (registered-only effects have no code).
    pub fn is_loaded(&self) -> bool {
        self.n_particle_blocks != 0xffff_ffff
    }

    /// Dword at `offset` of the constant data (0 past the end).
    pub fn const_u32(&self, offset: u32) -> u32 {
        bytecode::rd_or0(&self.const_data, offset)
    }
}

/// Supplies file contents by game path (e.g. `ETG\\general.script`).
pub trait FileSource {
    fn read_file(&self, path: &str) -> Option<Vec<u8>>;
}

impl FileSource for hwc_formats::gamedata::GameData {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        self.read(path).ok().flatten()
    }
}

/// Files from a directory holding an extracted archive (paths matched ignoring case).
#[derive(Clone, Debug)]
pub struct DirSource {
    pub root: std::path::PathBuf,
}

impl FileSource for DirSource {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        let mut cur = self.root.clone();
        let parts: Vec<&str> = path.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
        for (i, part) in parts.iter().enumerate() {
            let want_dir = i + 1 < parts.len();
            cur = hwc_formats::gamedata::find_ci(&cur, part, want_dir)?;
        }
        std::fs::read(cur).ok()
    }
}

impl FileSource for HashMap<String, Vec<u8>> {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        let key = hwc_formats::big::normalize_name(path);
        self.get(&key).cloned().or_else(|| self.get(path).cloned())
    }
}

/// Effect statics plus the texture and mesh registries their code refers to.
#[derive(Clone, Debug, Default)]
pub struct Library {
    pub statics: Vec<EffectStatic>,
    /// Registered texture paths; a texture handle is an index here (`0xffffffff` = none).
    pub textures: Vec<String>,
    /// Registered mesh paths; a mesh reference is `index + 1` (0 = none).
    pub meshes: Vec<String>,
    /// `etgTimeIndexIndex`: a global in the original, reset only when a code block closes.
    time_index_counter: u32,
    pub warnings: Vec<String>,
}

/// A parsed line's symbol (`etgvarentry`).
#[derive(Clone, Debug)]
struct VarEntry {
    name: String,
    offset: u32,
    rate_offset: u32,
    initial: u32,
    has_initial: bool,
    ty: u8,
    size: u8,
    code_block: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenFn {
    CallbackOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseFn {
    ParseModeRestore,
    CodeBlockEnd,
    ConditionalComplete,
    ElseComplete,
    AlternateComplete,
    TimeBlockComplete,
    AtComplete,
    BetweenComplete,
    CallbackClose,
}

/// A code location (`block`, byte offset), standing in for the original's pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct CodeRef {
    block: usize,
    offset: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct NestEntry {
    open: Option<OpenFn>,
    close: Option<CloseFn>,
    block: usize,
    offset: u32,
    user: Option<CodeRef>,
    /// Saved decision for `alternate` nesting.
    user_decision: Option<(CodeRef, usize)>,
}

struct AltBuild {
    decision: Decision,
    at: CodeRef,
    offsets: Vec<u32>,
}

/// The per-file parse state.
struct Parser<'a> {
    lib: &'a mut Library,
    stat: usize,
    file: String,
    line: usize,
    code: [Vec<u8>; 3],
    off: [u32; 3],
    mode: usize,
    nest: [NestEntry; 10],
    nest_level: usize,
    vars: Vec<VarEntry>,
    const_data: Vec<u8>,
    alts: Vec<AltBuild>,
    total_offsets: u32,
    /// Current `alternate` opcode and its alternate index.
    decision: Option<(CodeRef, usize)>,
    time_index_var: Option<usize>,
    time_index_time: f32,
    if_opcode: Option<CodeRef>,
    /// Names of unresolved `goto`/`call` targets, referenced from the code by index.
    pending_names: Vec<String>,
}

impl Library {
    pub fn new() -> Self {
        Self::default()
    }

    /// `etgEffectStaticFind`: look up an effect by file name (directories stripped at `\`),
    /// optionally registering a blank entry.
    pub fn find(&mut self, name: &str, register: bool) -> Option<usize> {
        let base = name.rsplit('\\').next().unwrap_or(name);
        if let Some(i) = self.statics.iter().position(|s| s.name.eq_ignore_ascii_case(base)) {
            return Some(i);
        }
        if !register {
            return None;
        }
        self.statics.push(EffectStatic::blank(base));
        Some(self.statics.len() - 1)
    }

    /// Look up a loaded effect by name without registering.
    pub fn get(&self, name: &str) -> Option<usize> {
        let base = name.rsplit('\\').next().unwrap_or(name);
        self.statics.iter().position(|s| s.name.eq_ignore_ascii_case(base))
    }

    /// `trTextureRegister` for effect textures: the handle for a path.
    pub fn texture_register(&mut self, path: &str) -> u32 {
        if let Some(i) = self.textures.iter().position(|t| t.eq_ignore_ascii_case(path)) {
            return i as u32;
        }
        self.textures.push(path.to_string());
        (self.textures.len() - 1) as u32
    }

    /// `etgMeshRegister`: `index + 1` of the mesh path.
    pub fn mesh_register(&mut self, path: &str) -> u32 {
        if let Some(i) = self.meshes.iter().position(|t| t.eq_ignore_ascii_case(path)) {
            return i as u32 + 1;
        }
        self.meshes.push(path.to_string());
        self.meshes.len() as u32
    }

    /// `etgEffectCodeLoad`: load (or return the already-loaded) effect at `path`.
    pub fn load_effect(&mut self, files: &dyn FileSource, path: &str) -> Result<usize, CompileError> {
        let idx = self.find(path, true).expect("registered");
        if self.statics[idx].is_loaded() {
            return Ok(idx);
        }
        let data = files.read_file(path).ok_or_else(|| CompileError {
            file: path.to_string(),
            line: 0,
            message: "file not found".into(),
        })?;
        self.compile_into(idx, path, &data)
    }

    /// Compile effect source text into a (new or registered) static named after `path`.
    pub fn compile_source(&mut self, path: &str, source: &[u8]) -> Result<usize, CompileError> {
        let idx = self.find(path, true).expect("registered");
        if self.statics[idx].is_loaded() {
            return Ok(idx);
        }
        self.compile_into(idx, path, source)
    }

    fn compile_into(&mut self, idx: usize, path: &str, data: &[u8]) -> Result<usize, CompileError> {
        {
            let s = &mut self.statics[idx];
            s.n_particle_blocks = DEFAULT_PARTICLE_BLOCKS;
            s.n_history = HISTORY_DEFAULT;
            s.variable_size = 0;
            s.rate_size = 0;
            s.software_version = None;
            s.const_data.clear();
            s.special_ops = 0;
        }
        let mut p = Parser {
            lib: self,
            stat: idx,
            file: path.to_string(),
            line: 0,
            code: [Vec::new(), Vec::new(), Vec::new()],
            off: [0; 3],
            mode: MODE_NOTHING,
            nest: [NestEntry::default(); 10],
            nest_level: 0,
            vars: Vec::new(),
            const_data: Vec::new(),
            alts: Vec::new(),
            total_offsets: 0,
            decision: None,
            time_index_var: None,
            time_index_time: 0.0,
            if_opcode: None,
            pending_names: Vec::new(),
        };
        let result = p.run(data);
        if result.is_err() {
            // Leave the entry registered but unloaded, as the original's error recovery does.
            p.lib.statics[idx].n_particle_blocks = 0xffff_ffff;
        }
        result.map(|_| idx)
    }
}

/// Reads lines like `fileLineRead` on an archive file: up to `n - 1` bytes, ending at CR
/// (a following LF is consumed); a lone LF stays in the line.
struct LineReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl LineReader<'_> {
    fn read_line(&mut self, n: usize) -> Option<Vec<u8>> {
        if self.pos >= self.data.len() {
            return None;
        }
        let mut out = Vec::new();
        while out.len() < n - 1 && self.pos < self.data.len() {
            let ch = self.data[self.pos];
            self.pos += 1;
            if ch == b'\r' {
                if self.pos < self.data.len() && self.data[self.pos] == b'\n' {
                    self.pos += 1;
                }
                break;
            }
            out.push(ch);
        }
        Some(out)
    }
}

/// `etgColorScan`: three or four comma-separated reals in `0..=1`.
fn color_scan(s: &str) -> u32 {
    let parts: Vec<&str> = s.splitn(5, ',').collect();
    let mut vals = [0.0f32; 4];
    let mut n = 0;
    for (i, p) in parts.iter().take(4).enumerate() {
        match cfmt::scan_float(p) {
            Some(v) => {
                vals[i] = v;
                n += 1;
            }
            None => break,
        }
    }
    if n == 3 {
        vals[3] = 1.0;
    }
    let c = |v: f32| cfmt::col_real_to_ubyte(f64::from(v));
    cfmt::col_rgba(c(vals[0]), c(vals[1]), c(vals[2]), c(vals[3]))
}

/// Splits `name = value` the way `etgParseVariable` does: the name is trimmed of trailing
/// blanks, the value starts at the first non-blank after `=`.
fn parse_variable(params: &str) -> (String, Option<String>) {
    match params.find('=') {
        None => (params.to_string(), None),
        Some(eq) => {
            let name = params[..eq].trim_end_matches([' ', '\t']).to_string();
            let value = params[eq + 1..].trim_start_matches([' ', '\t']).to_string();
            (name, Some(value))
        }
    }
}

impl Parser<'_> {
    fn err(&self, message: impl Into<String>) -> CompileError {
        CompileError { file: self.file.clone(), line: self.line, message: message.into() }
    }

    fn warn(&mut self, message: impl Into<String>) {
        let w = format!("{}:{}: {}", self.file, self.line, message.into());
        self.lib.warnings.push(w);
    }

    fn stat(&mut self) -> &mut EffectStatic {
        &mut self.lib.statics[self.stat]
    }

    fn find_var(&self, name: &str) -> Option<usize> {
        self.vars.iter().position(|v| v.name.eq_ignore_ascii_case(name))
    }

    fn need_var(&self, name: &str) -> Result<usize, CompileError> {
        self.find_var(name).ok_or_else(|| self.err(format!("symbol '{name}' not found")))
    }

    /// Value a symbol contributes when used as an operand: labels give their `initial`,
    /// everything else its variable offset.
    fn var_operand(&self, v: usize) -> u32 {
        let e = &self.vars[v];
        if e.ty == evt::CONST_LABEL || e.ty == evt::VAR_LABEL {
            e.initial
        } else {
            e.offset
        }
    }

    fn block(&self) -> Result<usize, CompileError> {
        if self.mode < 3 {
            Ok(self.mode)
        } else {
            Err(self.err("code outside of a code block"))
        }
    }

    fn emit(&mut self, block: usize, bytes: &[u8]) {
        let at = self.off[block] as usize;
        let code = &mut self.code[block];
        if code.len() < at + bytes.len() {
            code.resize(at + bytes.len(), 0);
        }
        code[at..at + bytes.len()].copy_from_slice(bytes);
        self.off[block] += bytes.len() as u32;
    }

    fn emit_words(&mut self, block: usize, words: &[u32]) {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.emit(block, &bytes);
    }

    fn code_wr(&mut self, r: CodeRef, field: u32, v: u32) {
        wr(&mut self.code[r.block], r.offset + field, v);
    }

    fn code_rd(&self, r: CodeRef, field: u32) -> u32 {
        bytecode::rd_or0(&self.code[r.block], r.offset + field)
    }

    fn new_variable(&mut self, name: &str, ty: u8, initial: Option<u32>) {
        let offset = self.lib.statics[self.stat].variable_size;
        self.vars.push(VarEntry {
            name: name.to_string(),
            offset,
            rate_offset: 0xffff_ffff,
            initial: initial.unwrap_or(0),
            has_initial: initial.is_some(),
            ty,
            size: 0,
            code_block: self.mode as u8,
        });
        if ty != evt::LABEL && ty != evt::CONST_LABEL && ty != evt::VAR_LABEL {
            self.stat().variable_size += 4;
        }
    }

    fn nest_set(&mut self, open: Option<OpenFn>, close: CloseFn, block: usize, offset: u32, user: Option<CodeRef>) {
        let lvl = self.nest_level.min(9);
        self.nest[lvl] = NestEntry { open, close: Some(close), block, offset, user, user_decision: None };
    }

    // ---------------------------------------------------------------- line reading

    /// `etgLineRead`: strip `//` comments and surrounding blanks, skip empty lines and expand
    /// `[expressions]`.
    fn read_line(&mut self, reader: &mut LineReader) -> Result<Option<String>, CompileError> {
        loop {
            let Some(raw) = reader.read_line(LINE_LENGTH) else { return Ok(None) };
            self.line += 1;
            let mut s = String::from_utf8_lossy(&raw).into_owned();
            if let Some(p) = s.find("//") {
                s.truncate(p);
            }
            let b = s.as_bytes();
            let start = b.iter().position(|c| *c != b' ' && *c != b'\t').unwrap_or(b.len());
            // Trailing blanks: scanned down to (but excluding) index 0; no trim when every
            // character after the first is blank.
            let mut end = b.len();
            let mut i = b.len();
            while i > 0 {
                let c = if i < b.len() { b[i] } else { 0 };
                if c != b' ' && c != b'\t' && c != b'\n' && c != 0 {
                    end = i + 1;
                    break;
                }
                i -= 1;
            }
            let end = end.min(b.len()).max(start);
            let body = &s[start..end];
            if body.is_empty() {
                continue;
            }
            return self.expand(body).map(Some);
        }
    }

    fn expand(&mut self, line: &str) -> Result<String, CompileError> {
        let mut out = String::new();
        let mut rest = line;
        while let Some(open) = rest.find('[') {
            out.push_str(&rest[..open]);
            let Some(close_rel) = rest[open..].find(']') else {
                return Err(self.err("'[' without ']'"));
            };
            let expr = &rest[open + 1..open + close_rel];
            let mut rng = hwc_math::random::RanStream::DEFAULT;
            let mut random = move || rng.next_u32();
            let (v, e) = eval::evaluate(expr, &mut random);
            if let Some(e) = e {
                self.warn(format!("{e:?} in expression '{expr}'"));
            }
            out.push_str(&eval::num_to_str(v));
            rest = &rest[open + close_rel + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    // ---------------------------------------------------------------- driver

    fn run(&mut self, data: &[u8]) -> Result<(), CompileError> {
        let mut reader = LineReader { data, pos: 0 };
        while let Some(line) = self.read_line(&mut reader)? {
            let mut bytes: Vec<u8> = Vec::new();
            if !self.opcode_scan(&line, &mut bytes)?
                && !self.function_scan(&line, &mut bytes)?
                && !self.var_copy_scan(&line, &mut bytes)?
            {
                self.warn(format!("don't know how to handle '{line}'"));
            }
            if !bytes.is_empty() {
                if self.mode >= 3 {
                    return Err(self.err(format!("opcode outside of a code block: '{line}'")));
                }
                let m = self.mode;
                self.emit(m, &bytes);
            }
        }
        self.finish()
    }

    /// Split `ret <- rest` (`etgOpcodeScan`/`etgFunctionCallScan` prologue). The return name
    /// is the text before the character preceding `<-`, cut at the first blank.
    fn split_return(line: &str) -> (Option<String>, String) {
        match line.find("<-") {
            None => (None, line.to_string()),
            Some(p) => {
                let head = &line[..p.saturating_sub(1)];
                let ret = StrTok::new(head).next_tok(" \t").unwrap_or_default();
                let start = line[p + 2..].trim_start_matches([' ', '\t']).to_string();
                (Some(ret), start)
            }
        }
    }

    /// Isolate `(params)`: returns (text before `(`, text up to the first `)`).
    fn isolate_params(&self, s: &str) -> Result<(String, String), CompileError> {
        let open = s.find('(').ok_or_else(|| self.err(format!("expecting parameters in '{s}'")))?;
        let inner = &s[open + 1..];
        let close = inner.find(')').ok_or_else(|| self.err(format!("no closing parenthesis in '{s}'")))?;
        Ok((s[..open].to_string(), inner[..close].to_string()))
    }

    fn opcode_scan(&mut self, line: &str, dest: &mut Vec<u8>) -> Result<bool, CompileError> {
        let (_, start) = Self::split_return(line);
        let entry = PARSE_TABLE.iter().find(|e| names_match(&start, e.0) > 0);
        let Some(&(name, flags, which)) = entry else { return Ok(false) };
        let len = names_match(&start, name);
        let (opcode_str, params) = if flags & 1 != 0 {
            let (o, p) = self.isolate_params(&start)?;
            (o, Some(p))
        } else if flags & 2 != 0 {
            (start.clone(), Some(start[len..].trim_start_matches([' ', '\t']).to_string()))
        } else {
            (start.clone(), None)
        };
        self.dispatch_parse(which, dest, &opcode_str, params.as_deref().unwrap_or(""))?;
        Ok(true)
    }

    fn function_scan(&mut self, line: &str, dest: &mut Vec<u8>) -> Result<bool, CompileError> {
        let (ret, start) = Self::split_return(line);
        let Some(entry) = FUNCTION_TABLE.iter().find(|e| names_match(&start, e.name) > 0) else {
            return Ok(false);
        };
        let (_, params) = self.isolate_params(&start)?;
        let mut words = vec![op::FUNCTION, entry.fun as u32, 0, 0xffff_ffff, u32::from(entry.pass_this)];
        let mut tok = StrTok::new(&params);
        let mut n = 0usize;
        while let Some(p) = tok.next_tok(", \t") {
            let declared = param_type(entry, n);
            if is_number(&p) {
                let v = if declared == evt::FLOAT { scan_float_bits(&p) } else { scan_int(&p) };
                words.push(u32::from(evt::CONSTANT));
                words.push(v.unwrap_or(0));
            } else {
                let v = self.need_var(&p)?;
                words.push(u32::from(declared));
                words.push(self.var_operand(v));
            }
            n += 1;
        }
        words[2] = n as u32;
        if let Some(r) = ret {
            let v = self.need_var(&r)?;
            words[3] = self.var_operand(v);
        }
        *dest = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        match entry.resolve {
            Resolve::None => {}
            Resolve::DepthWrite => {
                if words.len() > 6 && words[5] == u32::from(evt::CONSTANT) && words[6] == 0 {
                    self.stat().special_ops |= ESO_SORT_FORWARD;
                }
            }
            Resolve::Creation => {
                let b = self.block()?;
                let at = CodeRef { block: b, offset: self.off[b] };
                self.nest_set(Some(OpenFn::CallbackOpen), CloseFn::CallbackClose, b, at.offset, Some(at));
            }
        }
        Ok(true)
    }

    fn var_copy_scan(&mut self, line: &str, dest: &mut Vec<u8>) -> Result<bool, CompileError> {
        if !line.contains("<-") {
            return Ok(false);
        }
        let mut tok = StrTok::new(line);
        let l = tok.next_tok(" \t<-").unwrap_or_default();
        let r = tok.next_tok(" \t<-").ok_or_else(|| self.err("assignment without a value"))?;
        let lv = self.need_var(&l)?;
        let dest_off = self.vars[lv].offset;
        if is_number(&r) {
            let v = if self.vars[lv].ty == evt::FLOAT { scan_float_bits(&r) } else { scan_int(&r) };
            *dest = [op::VARIABLE_ASSIGN, v.unwrap_or(0), dest_off].iter().flat_map(|w| w.to_le_bytes()).collect();
        } else {
            let rv = self.need_var(&r)?;
            let e = &self.vars[rv];
            let words = if e.ty == evt::CONST_LABEL || e.ty == evt::VAR_LABEL {
                [op::VARIABLE_ASSIGN, e.initial, dest_off]
            } else {
                [op::VARIABLE_COPY, e.offset, dest_off]
            };
            *dest = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        }
        Ok(true)
    }

    // ---------------------------------------------------------------- parse functions

    fn dispatch_parse(&mut self, which: P, dest: &mut Vec<u8>, opcode: &str, params: &str) -> Result<(), CompileError> {
        let words: Vec<u32> = match which {
            P::Nop => vec![op::NOP],
            P::If => self.p_conditional(params)?,
            P::Else => self.p_else()?,
            P::Label => {
                self.p_label(params);
                vec![]
            }
            P::EventStart => {
                self.p_event_start(params)?;
                vec![]
            }
            P::Variable => {
                let m = self.mode;
                self.nest_set(None, CloseFn::ParseModeRestore, m, 0, None);
                self.mode = MODE_VARIABLES;
                vec![]
            }
            P::Open => {
                self.nest_push()?;
                vec![]
            }
            P::Close => {
                let off = if self.mode < 3 { self.off[self.mode] } else { 0xffff_ffff };
                self.nest_pop(off)?;
                vec![]
            }
            P::Startup => self.p_block_open(0),
            P::EachFrame => self.p_block_open(1),
            P::TimeIndex => self.p_block_open(2),
            P::ParticleBlocks => {
                let v = scan_int(params).unwrap_or(0);
                self.stat().n_particle_blocks = v;
                vec![]
            }
            P::MaxFrequency => {
                let v = scan_int(params).unwrap_or(0);
                self.stat().n_history = v as i32;
                vec![]
            }
            P::WorldRender => {
                self.stat().special_ops |= ESO_WORLD_RENDER;
                vec![]
            }
            P::Int => {
                self.p_new_number(params, evt::INT);
                vec![]
            }
            P::Float => {
                self.p_new_number(params, evt::FLOAT);
                vec![]
            }
            P::Rgb | P::Rgba => {
                let (name, value) = parse_variable(params);
                let init = value.map(|v| color_scan(&v));
                self.new_variable(&name, if which == P::Rgb { evt::RGB } else { evt::RGBA }, init);
                vec![]
            }
            P::SubTexture => {
                self.p_sub_texture(params);
                vec![]
            }
            P::MorphAnim => {
                self.p_morph_anim(params);
                vec![]
            }
            P::SetTexture => self.p_set_texture(opcode, params)?,
            P::SetMesh => {
                let name = StrTok::new(params).next_tok(", \t").unwrap_or_default();
                let mesh = self.lib.mesh_register(&name);
                vec![op::FUNCTION, Fun::SetMeshdata as u32, 1, 0xffff_ffff, 0, u32::from(evt::CONSTANT), mesh]
            }
            P::At => self.p_at(opcode, params)?,
            P::Between => self.p_between(params)?,
            P::TimeBlock => {
                self.p_time_block(params)?;
                vec![]
            }
            P::Time => self.p_time(params)?,
            P::Goto | P::Call => self.p_branch(which == P::Call, params),
            P::Return => vec![op::RETURN, 0xffff_ffff, 0xffff_ffff],
            P::End => vec![op::END],
            P::Delete => {
                self.stat().special_ops |= ESO_SELF_DELETING;
                vec![op::DELETE]
            }
            P::Yield => vec![op::YIELD],
            P::Alternate => self.p_alternate_start()?,
            P::Alt => self.p_alternate_set(params)?,
            P::Line => vec![],
            P::Spawn => self.p_spawn(params)?,
            P::CreateEffects => self.p_create_effects(params)?,
        };
        *dest = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        Ok(())
    }

    fn p_block_open(&mut self, block: usize) -> Vec<u32> {
        if self.off[block] != 0 {
            self.off[block] -= 4; // drop the previous block's End
        }
        let m = self.mode;
        self.nest_set(None, CloseFn::CodeBlockEnd, m, 0, None);
        self.mode = block;
        vec![]
    }

    fn nest_push(&mut self) -> Result<(), CompileError> {
        if self.nest_level >= 10 {
            return Err(self.err("braces nested too deeply"));
        }
        self.nest_level += 1;
        let e = self.nest[self.nest_level - 1];
        if let Some(OpenFn::CallbackOpen) = e.open {
            self.callback_open(e.block, e.user.unwrap_or_default());
            self.nest[self.nest_level - 1].open = None;
        }
        Ok(())
    }

    fn nest_pop(&mut self, new_offset: u32) -> Result<(), CompileError> {
        if self.nest_level == 0 {
            return Err(self.err("more '}' than '{'"));
        }
        self.nest_level -= 1;
        let e = self.nest[self.nest_level];
        if let Some(f) = e.close {
            self.nest[self.nest_level].close = None;
            match f {
                CloseFn::ParseModeRestore => self.mode = e.block,
                CloseFn::CodeBlockEnd => {
                    let b = self.block()?;
                    self.emit_words(b, &[op::END]);
                    self.mode = e.block;
                    self.lib.time_index_counter = 0;
                }
                CloseFn::ConditionalComplete => {
                    let r = CodeRef { block: e.block, offset: e.offset };
                    self.code_wr(r, 12, new_offset.wrapping_sub(e.offset));
                    self.if_opcode = Some(r);
                }
                CloseFn::ElseComplete => {
                    let Some(ifr) = e.user else { return Err(self.err("'else' without 'if'")) };
                    let ifop = self.code_rd(ifr, 0);
                    if !(42..70).contains(&ifop) {
                        return Err(self.err(format!("'else' statement matches with an invalid 'if' (opcode {ifop})")));
                    }
                    let skip = self.code_rd(ifr, 12);
                    self.code_wr(ifr, 12, skip.wrapping_add(12));
                    self.code_wr(CodeRef { block: e.block, offset: e.offset }, 8, new_offset);
                }
                CloseFn::AlternateComplete => {
                    let Some((at, ai)) = self.decision else { return Err(self.err("unexpected '}' after 'alternate'")) };
                    let table_len = self.code_rd(at, 8);
                    self.code_wr(at, 12, new_offset.wrapping_sub(e.offset).wrapping_add(4));
                    self.total_offsets += table_len;
                    self.alts[ai].decision = Decision { kind: 0, low: 0, high: table_len };
                    let b = self.block()?;
                    self.emit_words(b, &[op::RETURN]);
                    self.decision = e.user_decision;
                }
                CloseFn::TimeBlockComplete => self.time_index_var = None,
                CloseFn::AtComplete => {
                    self.code_wr(CodeRef { block: e.block, offset: e.offset }, 4, new_offset.wrapping_sub(e.offset));
                }
                CloseFn::BetweenComplete => {
                    self.code_wr(CodeRef { block: e.block, offset: e.offset }, 20, new_offset.wrapping_sub(e.offset));
                }
                CloseFn::CallbackClose => {
                    let b = self.block()?;
                    self.emit_words(b, &[op::END]);
                    if let Some(branch) = e.user {
                        self.code_wr(branch, 8, new_offset.wrapping_add(4));
                    }
                }
            }
        }
        Ok(())
    }

    /// `etgCallbackOpen`: insert a call to the creation-callback setup in front of the
    /// `create*` call and a branch over the callback code that follows.
    fn callback_open(&mut self, block: usize, create: CodeRef) {
        let b = create.block;
        let start = create.offset as usize;
        let mut moved = self.code[b][start..(start + function_size(2) as usize).min(self.code[b].len())].to_vec();
        moved.resize(function_size(2) as usize, 0);
        let cur = self.off[b];
        let setup = [
            op::FUNCTION,
            Fun::CreateCallbackSetup as u32,
            1,
            0xffff_ffff,
            1,
            u32::from(evt::CONSTANT),
            cur + function_size(1) + bytecode::SIZE_BRANCH,
        ];
        let mut bytes: Vec<u8> = setup.iter().flat_map(|w| w.to_le_bytes()).collect();
        bytes.extend_from_slice(&moved);
        let branch_at = start + bytes.len();
        bytes.extend([op::GOTO, block as u32, 0xffff_ffff].iter().flat_map(|w| w.to_le_bytes()));
        let code = &mut self.code[b];
        if code.len() < start + bytes.len() {
            code.resize(start + bytes.len(), 0);
        }
        code[start..start + bytes.len()].copy_from_slice(&bytes);
        self.off[b] += function_size(1) + bytecode::SIZE_BRANCH;
        self.nest[self.nest_level - 1].user = Some(CodeRef { block: b, offset: branch_at as u32 });
    }

    fn p_conditional(&mut self, params: &str) -> Result<Vec<u32>, CompileError> {
        let b = self.block()?;
        let mut tok = StrTok::new(params);
        let p0 = tok.next_tok(" \t").ok_or_else(|| self.err("'if' missing left parameter"))?;
        let mut oper = tok.next_tok(" \t").ok_or_else(|| self.err("'if' missing operator"))?;
        let p1 = tok.next_tok(" \t").ok_or_else(|| self.err("'if' missing right parameter"))?;
        let is_var0 = !is_number(&p0);
        let is_var1 = !is_number(&p1);
        let (var_name, other, transpose) = if !is_var0 && is_var1 { (p1, p0, true) } else { (p0, p1, false) };
        let both_vars = is_var0 && is_var1;
        let v0 = self.need_var(&var_name)?;
        let is_float = self.vars[v0].ty == evt::FLOAT;
        let param1 = if both_vars {
            let v1 = self.need_var(&other)?;
            self.vars[v1].offset
        } else if is_float {
            scan_float_bits(&other).unwrap_or(0)
        } else {
            scan_int(&other).unwrap_or(0)
        };
        let sub = match (both_vars, is_float) {
            (true, true) => 3,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 0,
        };
        let mut transpose = transpose;
        let mut opcode = None;
        let mut i = 0;
        while i < COND_TABLE.len() {
            if COND_TABLE[i].0.eq_ignore_ascii_case(&oper) {
                if transpose {
                    transpose = false;
                    oper = COND_TABLE[i].1.to_string();
                    i = 0;
                    continue;
                }
                opcode = Some(COND_TABLE[i].2[sub]);
                break;
            }
            i += 1;
        }
        let opcode = opcode.ok_or_else(|| self.err(format!("unknown comparison '{oper}'")))?;
        let at = self.off[b];
        self.nest_set(None, CloseFn::ConditionalComplete, b, at, None);
        Ok(vec![opcode, self.vars[v0].offset, param1, 0xffff_ffff])
    }

    fn p_else(&mut self) -> Result<Vec<u32>, CompileError> {
        let b = self.block()?;
        let at = self.off[b];
        let ifr = self.if_opcode;
        self.nest_set(None, CloseFn::ElseComplete, b, at, ifr);
        Ok(vec![op::GOTO, b as u32, 0xffff_ffff])
    }

    fn p_label(&mut self, params: &str) {
        let name = StrTok::new(params).next_tok(" \t.,][()").unwrap_or_default();
        let (initial, ty) = match self.mode {
            MODE_NOTHING => (self.const_data.len() as u32, evt::CONST_LABEL),
            MODE_VARIABLES => (self.lib.statics[self.stat].variable_size, evt::VAR_LABEL),
            m => (self.off[m.min(2)], evt::LABEL),
        };
        self.new_variable(&name, ty, None);
        let last = self.vars.len() - 1;
        self.vars[last].initial = initial;
    }

    fn p_event_start(&mut self, params: &str) -> Result<(), CompileError> {
        self.mode = MODE_VARIABLES;
        let mut tok = StrTok::new(params);
        while let Some(t) = tok.next_tok(",") {
            let mut t = t.trim_start_matches([' ', '\t']).to_string();
            while t.ends_with(' ') {
                t.pop();
            }
            let mut sink = Vec::new();
            self.opcode_scan(&t, &mut sink)?;
        }
        self.mode = MODE_NOTHING;
        Ok(())
    }

    fn p_new_number(&mut self, params: &str, ty: u8) {
        if self.mode == MODE_NOTHING {
            let s = StrTok::new(params).next_tok(" =\t").unwrap_or_default();
            let v = if ty == evt::FLOAT { scan_float_bits(&s) } else { scan_int(&s) };
            self.const_data.extend_from_slice(&v.unwrap_or(0).to_le_bytes());
            return;
        }
        let (name, value) = parse_variable(params);
        let init = value.map(|v| if ty == evt::FLOAT { scan_float_bits(&v).unwrap_or(0) } else { scan_int(&v).unwrap_or(0) });
        self.new_variable(&name, ty, init);
    }

    fn p_sub_texture(&mut self, params: &str) {
        let mut tok = StrTok::new(params);
        let name = tok.next_tok(" \t,").unwrap_or_default();
        let xy = tok.next_tok(" \t,").and_then(|s| scan_int(&s)).unwrap_or(0);
        let flags = tok.next_tok(" \t,").and_then(|s| scan_int(&s)).unwrap_or(0);
        let tex = self.lib.texture_register(&name);
        for w in [tex, xy, flags] {
            self.const_data.extend_from_slice(&w.to_le_bytes());
        }
    }

    fn p_morph_anim(&mut self, params: &str) {
        let mut tok = StrTok::new(params);
        let name = tok.next_tok(" \t,").unwrap_or_default();
        let flags = tok.next_tok(" \t,").and_then(|s| scan_int(&s)).unwrap_or(0);
        let mesh = self.lib.mesh_register(&name);
        for w in [mesh, flags] {
            self.const_data.extend_from_slice(&w.to_le_bytes());
        }
    }

    fn p_set_texture(&mut self, opcode: &str, params: &str) -> Result<Vec<u32>, CompileError> {
        let mut tok = StrTok::new(params);
        let mut name = tok.next_tok(", \t").unwrap_or_default();
        let fun = if opcode.eq_ignore_ascii_case("setTexture") {
            Fun::SetTrHandle
        } else {
            // modifyTexture: the system-handle parameter is written and then overwritten when
            // the parameter count is reset below, so the call receives only the texture
            // handle and UVs (as in the original).
            let _ = self.need_var(&name)?;
            name = tok.next_tok(", \t").unwrap_or_default();
            Fun::ModifyTexture
        };
        let tex = self.lib.texture_register(&format!("ETG\\{name}"));
        let mut words = vec![op::FUNCTION, fun as u32, 0, 0xffff_ffff, 0, u32::from(evt::CONSTANT), tex];
        let mut n = 1;
        for _ in 0..4 {
            let v = tok.next_tok(", \t").and_then(|s| scan_int(&s)).unwrap_or(0);
            words.push(u32::from(evt::CONSTANT));
            words.push(v);
            n += 1;
        }
        words[2] = n;
        Ok(words)
    }

    fn p_at(&mut self, opcode: &str, params: &str) -> Result<Vec<u32>, CompileError> {
        let b = self.block()?;
        let mut opc = if names_match(opcode, "at") > 0 {
            op::AT
        } else if names_match(opcode, "before") > 0 {
            op::BEFORE
        } else if names_match(opcode, "after") > 0 {
            op::AFTER
        } else {
            op::EVERY
        };
        let operand = if is_number(params) {
            let mut t = cfmt::scan_float(params).unwrap_or(0.0);
            if opc == op::AT {
                // Round to the middle of a half-universe-update slot (ETG_UpdateRoundOff).
                let round = 1.0 / 32.0f64;
                let diff = f64::from(t) % round;
                if diff <= round / 2.0 {
                    t = ((round / 2.0 - diff) + f64::from(t)) as f32;
                } else {
                    t = (f64::from(t) - (diff - round / 2.0)) as f32;
                }
            }
            t.to_bits()
        } else {
            let v = self.need_var(params)?;
            opc += 1;
            self.vars[v].offset
        };
        let at = self.off[b];
        self.nest_set(None, CloseFn::AtComplete, b, at, None);
        Ok(vec![opc, 0xffff_ffff, operand])
    }

    fn p_between(&mut self, params: &str) -> Result<Vec<u32>, CompileError> {
        let b = self.block()?;
        let mut tok = StrTok::new(params);
        let mut words = vec![op::BETWEEN, 0, 0, 0, 0, 0xffff_ffff];
        for i in 0..2 {
            let p = tok.next_tok(", \t").unwrap_or_default();
            let (time, var) = if is_number(&p) {
                (scan_float_bits(&p).unwrap_or((-1.0f32).to_bits()), 0xffff_ffff)
            } else {
                let v = self.need_var(&p)?;
                ((-1.0f32).to_bits(), self.vars[v].offset)
            };
            words[1 + i] = time;
            words[3 + i] = var;
        }
        let at = self.off[b];
        self.nest_set(None, CloseFn::BetweenComplete, b, at, None);
        Ok(words)
    }

    fn p_time_block(&mut self, params: &str) -> Result<(), CompileError> {
        let v = self.need_var(params)?;
        self.time_index_var = Some(v);
        if self.vars[v].rate_offset == 0xffff_ffff {
            let rs = self.lib.statics[self.stat].rate_size;
            let add = match self.vars[v].ty {
                evt::INT | evt::FLOAT => 4,
                evt::RGB => 12,
                evt::RGBA => 16,
                _ => 0,
            };
            if add != 0 {
                self.vars[v].rate_offset = rs;
                self.stat().rate_size = rs + add;
            }
        }
        let m = self.mode;
        let at = self.off[m.min(2)];
        self.nest_set(None, CloseFn::TimeBlockComplete, m, at, None);
        self.time_index_time = 0.0;
        Ok(())
    }

    fn p_time(&mut self, params: &str) -> Result<Vec<u32>, CompileError> {
        let tv = self.time_index_var.ok_or_else(|| self.err("'time' outside a 'timeBlock'"))?;
        let time = cfmt::scan_float(params).unwrap_or(0.0);
        let comma = params.find(',').ok_or_else(|| self.err("'time' needs a time and a value"))?;
        let value = params[comma..].trim_start_matches(['\t', ' ', ',']).to_string();
        let ty = self.vars[tv].ty;
        let (opcode, end) = if let Some(ev) = self.find_var(&value) {
            let opc = match ty {
                evt::INT => op::EFFECTOR_VI,
                evt::FLOAT => op::EFFECTOR_VF,
                evt::RGB => op::EFFECTOR_VCO,
                _ => op::EFFECTOR_VCA,
            };
            (opc, self.vars[ev].offset)
        } else {
            match ty {
                evt::INT => (op::EFFECTOR_CI, scan_int(&value).unwrap_or(0)),
                evt::FLOAT => (op::EFFECTOR_CF, scan_float_bits(&value).unwrap_or(0)),
                evt::RGB => (op::EFFECTOR_CCO, color_scan(&value)),
                _ => (op::EFFECTOR_CCA, color_scan(&value)),
            }
        };
        let id = self.lib.time_index_counter;
        self.lib.time_index_counter += 1;
        let start = self.time_index_time;
        self.time_index_time = time;
        Ok(vec![
            op::BETWEEN,
            start.to_bits(),
            time.to_bits(),
            0xffff_ffff,
            0xffff_ffff,
            bytecode::SIZE_BETWEEN + bytecode::SIZE_SEFFECTOR,
            opcode,
            end,
            self.vars[tv].offset,
            self.vars[tv].rate_offset,
            id & 0xff,
        ])
    }

    fn p_branch(&mut self, call: bool, params: &str) -> Vec<u32> {
        let opc = if call { op::CALL } else { op::GOTO };
        match self.find_var(params) {
            None => {
                self.pending_names.push(params.to_string());
                vec![opc | op::RESOLVE, 0, (self.pending_names.len() - 1) as u32]
            }
            Some(v) => {
                let e = &self.vars[v];
                if call {
                    // Already-defined label: the original uses the label's variable offset
                    // and pointer size, i.e. block 0 at `variableSize`-at-definition.
                    vec![opc, u32::from(e.size), e.offset]
                } else {
                    vec![opc, u32::from(e.code_block), e.initial]
                }
            }
        }
    }

    fn p_alternate_start(&mut self) -> Result<Vec<u32>, CompileError> {
        let b = self.block()?;
        let old = self.decision;
        let ai = self.alts.len();
        let at = CodeRef { block: b, offset: self.off[b] };
        self.alts.push(AltBuild { decision: Decision::default(), at, offsets: Vec::new() });
        self.decision = Some((at, ai));
        let lvl = self.nest_level.min(9);
        self.nest[lvl] = NestEntry {
            open: None,
            close: Some(CloseFn::AlternateComplete),
            block: b,
            offset: at.offset,
            user: None,
            user_decision: old,
        };
        Ok(vec![op::ALTERNATE, ai as u32, 0, 0xffff_ffff, 0])
    }

    fn p_alternate_set(&mut self, params: &str) -> Result<Vec<u32>, CompileError> {
        let n = scan_int(params).unwrap_or(0) as i32;
        let (at, ai) = self.decision.ok_or_else(|| self.err("'alt' outside an alternate block"))?;
        let table_len = self.code_rd(at, 8);
        let (words, len) = if table_len != 0 { (vec![op::RETURN], 4) } else { (vec![], 0) };
        let cur = self.off[self.block()?];
        let mut tl = table_len;
        for _ in 0..n.max(0) {
            self.alts[ai].offsets.push(cur - at.offset + len);
            tl += 1;
        }
        self.code_wr(at, 8, tl);
        Ok(words)
    }

    fn parse_extra_params(&mut self, tok: &mut StrTok, words: &mut Vec<u32>) -> Result<u32, CompileError> {
        let mut n = 0;
        while let Some(p) = tok.next_tok(", \t") {
            if is_number(&p) {
                let v = if p.contains('.') { scan_float_bits(&p) } else { scan_int(&p) };
                words.push(u32::from(evt::CONSTANT));
                words.push(v.unwrap_or(0));
            } else {
                let v = self.need_var(&p)?;
                words.push(u32::from(self.vars[v].ty));
                words.push(self.var_operand(v));
            }
            n += 1;
        }
        Ok(n)
    }

    fn p_spawn(&mut self, params: &str) -> Result<Vec<u32>, CompileError> {
        let mut tok = StrTok::new(params);
        let name = tok.next_tok(", \t").unwrap_or_default();
        let target = self.lib.find(&name, true).expect("registered") as u32;
        let mut words = vec![op::FUNCTION, Fun::SpawnNewEffect as u32, 0, 0xffff_ffff, 1];
        words.extend([u32::from(evt::CONSTANT), target, u32::from(evt::CONSTANT), 0]);
        let extra = self.parse_extra_params(&mut tok, &mut words)?;
        words[2] = 2 + extra;
        words[8] = extra;
        Ok(words)
    }

    fn p_create_effects(&mut self, params: &str) -> Result<Vec<u32>, CompileError> {
        let mut tok = StrTok::new(params);
        let name = tok.next_tok(", \t").unwrap_or_default();
        let target = self.lib.find(&name, true).expect("registered") as u32;
        let mut words = vec![op::FUNCTION, Fun::CreateEffects as u32, 0, 0xffff_ffff, 1, u32::from(evt::CONSTANT), target];
        for _ in 0..2 {
            let p = tok.next_tok(", \t").ok_or_else(|| self.err("createEffects needs a count and a spread"))?;
            if is_number(&p) {
                words.push(u32::from(evt::CONSTANT));
                words.push(scan_int(&p).unwrap_or(0));
            } else {
                let v = self.need_var(&p)?;
                words.push(u32::from(evt::INT));
                words.push(self.vars[v].offset);
            }
        }
        words.extend([u32::from(evt::CONSTANT), 0]);
        let extra = self.parse_extra_params(&mut tok, &mut words)?;
        words[2] = 4 + extra;
        words[12] = extra;
        Ok(words)
    }

    // ---------------------------------------------------------------- finishing

    fn fix_forward_refs(&mut self, block: usize) -> Result<(), CompileError> {
        let len = self.off[block];
        let mut at = 0u32;
        while at < len {
            let opc = rd(&self.code[block], at);
            let mut clean = opc;
            if opc & op::RESOLVE != 0 {
                let name_idx = rd(&self.code[block], at + 8) as usize;
                let name = self.pending_names.get(name_idx).cloned().unwrap_or_default();
                let v = self.find_var(&name).ok_or_else(|| self.err(format!("unresolved branch destination '{name}'")))?;
                let (initial, cb) = (self.vars[v].initial, u32::from(self.vars[v].code_block));
                clean = opc & !op::RESOLVE;
                let r = CodeRef { block, offset: at };
                self.code_wr(r, 8, initial);
                self.code_wr(r, 4, cb);
                self.code_wr(r, 0, clean);
            }
            let step = if clean == op::FUNCTION {
                function_size(rd(&self.code[block], at + 8))
            } else {
                bytecode::op_length(clean)
            };
            if step == 0 {
                return Err(self.err(format!("bad opcode {clean} in block {block} at {at}")));
            }
            at += step;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), CompileError> {
        for b in 0..3 {
            self.fix_forward_refs(b)?;
        }
        // Combine the alternate offset tables and point each decision at its slice.
        let mut decisions = Vec::new();
        let mut offsets = Vec::new();
        for i in 0..self.alts.len() {
            let start = offsets.len() as u32;
            let at = self.alts[i].at;
            let n = self.code_rd(at, 8) as usize;
            let mut table = self.alts[i].offsets.clone();
            table.resize(n, 0xffff_ffff);
            offsets.extend_from_slice(&table);
            self.code_wr(at, 16, start);
            decisions.push(self.alts[i].decision);
        }
        // Variable initialisation table.
        let mut init = Vec::new();
        let mut var_index: u32 = 0;
        for v in &self.vars {
            if v.has_initial {
                init.push((var_index as u8, v.initial));
            }
            if v.ty != evt::LABEL && v.ty != evt::CONST_LABEL && v.ty != evt::VAR_LABEL {
                var_index += 1;
            }
        }
        let code: [Vec<u8>; 3] = std::array::from_fn(|b| self.code[b][..self.off[b] as usize].to_vec());
        let const_data = std::mem::take(&mut self.const_data);
        let s = self.stat();
        s.decisions = decisions;
        s.alternate_offsets = offsets;
        s.var_init = init;
        s.effect_size = s
            .variable_size
            .wrapping_add(s.variable_size / 4)
            .wrapping_add(0x134)
            .wrapping_add(s.n_particle_blocks.wrapping_mul(4))
            .wrapping_add(s.rate_size);
        s.code = code;
        s.const_data = const_data;
        if s.n_history > 0 {
            s.i_history = 0;
            s.history = vec![HISTORY_EMPTY; s.n_history as usize];
        } else {
            s.history.clear();
        }
        Ok(())
    }
}

/// Parse-table actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum P {
    Nop,
    If,
    Else,
    Label,
    EventStart,
    Variable,
    Open,
    Close,
    Startup,
    EachFrame,
    TimeIndex,
    ParticleBlocks,
    MaxFrequency,
    WorldRender,
    Int,
    Float,
    Rgb,
    Rgba,
    SubTexture,
    MorphAnim,
    SetTexture,
    SetMesh,
    At,
    Between,
    TimeBlock,
    Time,
    Goto,
    Call,
    Return,
    End,
    Delete,
    Yield,
    Alternate,
    Alt,
    Line,
    Spawn,
    CreateEffects,
}

/// `etgParseTable`: name, flags (1 = parameters in parentheses, 2 = rest of line), action.
const PARSE_TABLE: &[(&str, u32, P)] = &[
    ("nop", 0, P::Nop),
    ("if", 1, P::If),
    ("else", 0, P::Else),
    ("label", 1, P::Label),
    ("eventStart", 1, P::EventStart),
    ("variable", 0, P::Variable),
    ("{", 0, P::Open),
    ("}", 0, P::Close),
    ("startup", 0, P::Startup),
    ("eachFrame", 0, P::EachFrame),
    ("timeIndex", 0, P::TimeIndex),
    ("particleBlocks", 1, P::ParticleBlocks),
    ("maxFrequency", 1, P::MaxFrequency),
    ("worldRender", 0, P::WorldRender),
    ("int", 2, P::Int),
    ("float", 2, P::Float),
    ("RGB", 2, P::Rgb),
    ("RGBA", 2, P::Rgba),
    ("subTextureDWORD", 1, P::SubTexture),
    ("morphAnimDWORD", 1, P::MorphAnim),
    ("setTexture", 1, P::SetTexture),
    ("modifyTexture", 1, P::SetTexture),
    ("setMesh", 1, P::SetMesh),
    ("at", 1, P::At),
    ("every", 1, P::At),
    ("before", 1, P::At),
    ("after", 1, P::At),
    ("between", 1, P::Between),
    ("timeBlock", 1, P::TimeBlock),
    ("time", 1, P::Time),
    ("goto", 1, P::Goto),
    ("call", 1, P::Call),
    ("return", 0, P::Return),
    ("end", 1, P::End),
    ("delete", 1, P::Delete),
    ("yield", 1, P::Yield),
    ("alternate", 0, P::Alternate),
    ("alt", 1, P::Alt),
    ("nop", 0, P::Nop),
    ("#line", 2, P::Line),
    ("spawn", 1, P::Spawn),
    ("createEffects", 1, P::CreateEffects),
];

/// `etgConditionalTable`: symbol, transposed symbol, opcodes (VCI, VVI, VCF, VVF).
const COND_TABLE: &[(&str, &str, [u32; 4])] = &[
    ("=", "=", [42, 48, 54, 60]),
    ("==", "=", [42, 48, 54, 60]),
    ("!=", "!=", [43, 49, 55, 61]),
    ("<>", "<>", [43, 49, 55, 61]),
    (">", "<=", [44, 50, 56, 62]),
    (">=", "<", [45, 51, 57, 63]),
    ("<", ">=", [46, 52, 58, 64]),
    ("<=", ">", [47, 53, 59, 65]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{disassemble, Instr};

    fn compile(src: &str) -> (Library, usize) {
        let mut lib = Library::new();
        let src = src.replace('\n', "\r\n");
        let i = lib.compile_source("test.ebg", src.as_bytes()).unwrap();
        (lib, i)
    }

    #[test]
    fn variables_and_blocks() {
        let (lib, i) = compile(
            "eventStart(int lod = 0, float size = 2.5)\nvariable\n{\nint sys\nfloat t = 3;\n}\nstartup\n{\nsetDefaults()\nsys <- createSprites(4, 0)\n}\neachFrame\n{\nat(0.5)\n{\ndelete()\n}\n}\n",
        );
        let s = &lib.statics[i];
        assert_eq!(s.variable_size, 16);
        assert_eq!(s.var_init, vec![(0, 0), (1, 2.5f32.to_bits()), (3, 3.0f32.to_bits())]);
        let start = disassemble(&s.code[0]);
        assert!(matches!(start[0].1, Instr::Call { function, .. } if function == Fun::SetDefaults as u32));
        assert!(matches!(start[1].1, Instr::Call { function, ret: 8, .. } if function == Fun::CreateSprites as u32));
        assert_eq!(start.last().unwrap().1, Instr::End);
        let each = disassemble(&s.code[1]);
        // at(0.5) is rounded to the middle of a 1/32 s slot.
        match &each[0].1 {
            Instr::TimeTest { opcode: 5, skip, operand } => {
                assert_eq!(*skip, 16);
                assert_eq!(f32::from_bits(*operand), 0.515625);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(s.special_ops & ESO_SELF_DELETING, 1);
    }

    #[test]
    fn negative_assignment_loses_sign() {
        let (lib, i) = compile("variable\n{\nfloat v\n}\nstartup\n{\nv <- -200\n}\n");
        let d = disassemble(&lib.statics[i].code[0]);
        assert_eq!(d[0].1, Instr::VarAssign { value: 200f32.to_bits(), dest: 0 });
    }

    #[test]
    fn repeated_blocks_concatenate() {
        let (lib, i) = compile("eachFrame\n{\nnop\n}\neachFrame\n{\nnop\n}\n");
        let d = disassemble(&lib.statics[i].code[1]);
        assert_eq!(d.iter().map(|x| x.1.clone()).collect::<Vec<_>>(), vec![Instr::Nop, Instr::Nop, Instr::End]);
    }

    #[test]
    fn if_else_and_alternate() {
        let (lib, i) = compile(
            "variable\n{\nint a = 1\n}\neachFrame\n{\nif (a == 2)\n{\nnop\n}\nelse\n{\nnop\n}\nalternate\n{\nalt(1)\nnop\nalt(2)\nnop\n}\n}\n",
        );
        let s = &lib.statics[i];
        let d = disassemble(&s.code[1]);
        // if: 16 bytes + nop 4 + else goto 12 -> skip 32
        assert_eq!(d[0].1, Instr::Cond { opcode: 42, p0: 0, p1: 2, skip: 32 });
        assert_eq!(d[2].1, Instr::Branch { opcode: op::GOTO, block: 1, target: 36 });
        assert_eq!(s.decisions, vec![Decision { kind: 0, low: 0, high: 3 }]);
        assert_eq!(s.alternate_offsets, vec![20, 28, 28]);
    }

    #[test]
    fn creation_callback_layout() {
        let (lib, i) = compile("variable\n{\nint s\n}\nstartup\n{\ns <- createSprites(2, 0)\n{\nnop\n}\nnop\n}\n");
        let d = disassemble(&lib.statics[i].code[0]);
        assert!(matches!(&d[0].1, Instr::Call { function, params, .. } if *function == Fun::CreateCallbackSetup as u32 && params[0].1 == 76));
        assert!(matches!(&d[1].1, Instr::Call { function, .. } if *function == Fun::CreateSprites as u32));
        assert_eq!(d[2].1, Instr::Branch { opcode: op::GOTO, block: 0, target: 84 });
        assert_eq!(d[3], (76, Instr::Nop));
        assert_eq!(d[4], (80, Instr::End));
        assert_eq!(d[5], (84, Instr::Nop));
    }
}
