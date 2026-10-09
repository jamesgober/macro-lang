//! Lowering a [`Rule`] into the flat programs the matcher and transcriber run.
//!
//! A pattern becomes a Thompson-style NFA program: consuming instructions that
//! each match one input entry, plus epsilon instructions (`Split`, `Jump`) that
//! route between them and bookkeeping instructions (`Enter`, `Iter`) that record
//! repetition structure. A template becomes a flat op list with explicit
//! repetition and group brackets. Both are produced with explicit work stacks,
//! so a definition nested arbitrarily deep cannot overflow the call stack, and
//! every static check is made here so expansion only fails on input-dependent
//! conditions.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use intern_lang::Symbol;
use token_lang::Token;

use crate::{Context, Fragment, Kleene, MacroError, Pattern, Rule, Template};

/// One instruction of a compiled pattern.
#[derive(Clone, Debug)]
pub(crate) enum Inst<K> {
    /// Consume a token of exactly this kind.
    Token(K),
    /// Consume the opening of a group with these delimiter kinds.
    Open { open: K, close: K },
    /// Consume the end of the current group.
    Close,
    /// Consume one tree allowed by `fragment` and capture it into `slot`.
    Bind { slot: usize, fragment: Fragment<K> },
    /// Epsilon: a repetition is starting; open a new instance of `rep`.
    Enter(usize),
    /// Epsilon: one more iteration of the current instance of `rep`.
    Iter(usize),
    /// Epsilon: continue at both targets.
    Split(usize, usize),
    /// Epsilon: continue at the target.
    Jump(usize),
    /// The whole pattern matched, provided the input is exhausted.
    Accept,
}

/// One op of a compiled template.
#[derive(Clone, Debug)]
pub(crate) enum Op<K> {
    /// Write this token, marked for the expansion.
    Token(Token<K>, Context),
    /// Start a group with these delimiters.
    Open(Token<K>, Token<K>),
    /// Finish the innermost open group.
    Close,
    /// Write the tree captured in `slot` at the current repetition indices.
    Var(usize),
    /// Start a repetition. `end` is the index of its matching [`Op::End`];
    /// `locks` is the range of [`CompiledRule::locks`] holding the slots whose
    /// capture counts decide how many times it runs; `separator` is written
    /// between iterations and marked like a literal token defined in its
    /// context.
    Repeat {
        end: usize,
        locks: (usize, usize),
        separator: Option<(Token<K>, Context)>,
    },
    /// Finish one iteration of the repetition that starts at op `start`.
    End { start: usize },
}

/// Static facts about one metavariable slot.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slot {
    /// Start of this slot's repetition chain in [`CompiledRule::chains`].
    chain: usize,
    /// Number of pattern repetitions enclosing the binding.
    pub(crate) depth: usize,
}

/// A rule ready to run: the pattern program, the template ops, and the slot
/// metadata that connects them.
#[derive(Clone, Debug)]
pub(crate) struct CompiledRule<K> {
    pub(crate) program: Vec<Inst<K>>,
    pub(crate) ops: Vec<Op<K>>,
    pub(crate) slots: Vec<Slot>,
    /// Concatenated repetition chains, outermost repetition first.
    chains: Vec<usize>,
    /// Concatenated lockstep slot lists for template repetitions.
    pub(crate) locks: Vec<usize>,
    /// Number of pattern repetitions.
    pub(crate) reps: usize,
    /// The index of the token or group instruction every match must begin
    /// with, if the program reaches one from its start without branching. The
    /// matcher tests the first input entry against it before running.
    pub(crate) lead: Option<usize>,
}

impl<K> CompiledRule<K> {
    /// The repetition chain of `slot`: the pattern repetitions that enclose its
    /// binding, outermost first.
    #[inline]
    pub(crate) fn chain(&self, slot: usize) -> &[usize] {
        self.slots
            .get(slot)
            .and_then(|info| self.chains.get(info.chain..info.chain + info.depth))
            .unwrap_or(&[])
    }
}

/// Validates and compiles one rule. `index` is its position, for error reports.
pub(crate) fn compile_rule<K>(rule: Rule<K>, index: usize) -> Result<CompiledRule<K>, MacroError> {
    let mut compiled = CompiledRule {
        program: Vec::new(),
        ops: Vec::new(),
        slots: Vec::new(),
        chains: Vec::new(),
        locks: Vec::new(),
        reps: 0,
        lead: None,
    };
    let names = compile_pattern(rule.pattern, index, &mut compiled)?;
    compile_template(rule.template, index, &names, &mut compiled)?;
    compiled.lead = lead(&compiled.program);
    Ok(compiled)
}

/// Follows `program` from its start through non-branching epsilon
/// instructions and returns the index of a fixed first token or group.
fn lead<K>(program: &[Inst<K>]) -> Option<usize> {
    let mut pc = 0;
    loop {
        match program.get(pc)? {
            Inst::Enter(_) | Inst::Iter(_) => pc += 1,
            Inst::Token(_) | Inst::Open { .. } => return Some(pc),
            _ => return None,
        }
    }
}

/// Pending work while lowering a pattern.
enum PatternWork<K> {
    Element(Pattern<K>),
    CloseGroup,
    EndRepeat(RepeatFrame<K>),
}

/// What a repetition's closing step needs to know about its opening.
struct RepeatFrame<K> {
    /// Index of the `Iter` instruction each iteration starts at.
    iterate: usize,
    /// Index of the entry `Split` for `*` and `?`, which may skip the body.
    entry: Option<usize>,
    separator: Option<K>,
    kleene: Kleene,
}

/// Lowers a pattern into `out.program`, filling in slot metadata. Returns the
/// name → slot map the template is resolved against.
fn compile_pattern<K>(
    pattern: Vec<Pattern<K>>,
    rule: usize,
    out: &mut CompiledRule<K>,
) -> Result<BTreeMap<Symbol, usize>, MacroError> {
    let mut names = BTreeMap::new();
    // The repetitions enclosing the current position, outermost first.
    let mut chain: Vec<usize> = Vec::new();
    // One flag per open sequence (the root, each group body, each repetition
    // body): does every path through it consume input?
    let mut consumes: Vec<bool> = alloc::vec![false];
    let mut work: Vec<PatternWork<K>> = pattern
        .into_iter()
        .rev()
        .map(PatternWork::Element)
        .collect();

    while let Some(item) = work.pop() {
        match item {
            PatternWork::Element(Pattern::Token(kind)) => {
                set_consumes(&mut consumes);
                out.program.push(Inst::Token(kind));
            }
            PatternWork::Element(Pattern::Group { open, close, body }) => {
                set_consumes(&mut consumes);
                out.program.push(Inst::Open { open, close });
                consumes.push(false);
                work.push(PatternWork::CloseGroup);
                work.extend(body.into_iter().rev().map(PatternWork::Element));
            }
            PatternWork::CloseGroup => {
                // A group consumes its delimiters whatever its body does; the
                // body's own flag is no longer needed.
                consumes.truncate(consumes.len().saturating_sub(1));
                out.program.push(Inst::Close);
            }
            PatternWork::Element(Pattern::Bind { name, fragment }) => {
                let slot = out.slots.len();
                if names.insert(name, slot).is_some() {
                    return Err(MacroError::DuplicateBinding { rule, name });
                }
                out.slots.push(Slot {
                    chain: out.chains.len(),
                    depth: chain.len(),
                });
                out.chains.extend_from_slice(&chain);
                set_consumes(&mut consumes);
                out.program.push(Inst::Bind { slot, fragment });
            }
            PatternWork::Element(Pattern::Repeat {
                body,
                separator,
                kleene,
            }) => {
                if kleene == Kleene::ZeroOrOne && separator.is_some() {
                    return Err(MacroError::OptionalSeparator { rule });
                }
                let rep = out.reps;
                out.reps += 1;
                out.program.push(Inst::Enter(rep));
                let entry = if kleene == Kleene::OneOrMore {
                    None
                } else {
                    // Patched once the exit is known.
                    out.program.push(Inst::Split(0, 0));
                    Some(out.program.len() - 1)
                };
                let iterate = out.program.len();
                out.program.push(Inst::Iter(rep));
                chain.push(rep);
                consumes.push(false);
                work.push(PatternWork::EndRepeat(RepeatFrame {
                    iterate,
                    entry,
                    separator,
                    kleene,
                }));
                work.extend(body.into_iter().rev().map(PatternWork::Element));
            }
            PatternWork::EndRepeat(frame) => {
                // A body that can match nothing would let the repetition loop
                // without advancing; rejecting it here is what keeps the
                // matcher's epsilon graph acyclic.
                if consumes.pop() != Some(true) {
                    return Err(MacroError::EmptyRepetition { rule });
                }
                chain.truncate(chain.len().saturating_sub(1));
                if frame.kleene == Kleene::OneOrMore {
                    set_consumes(&mut consumes);
                }
                close_repeat(&mut out.program, frame);
            }
        }
    }
    out.program.push(Inst::Accept);
    Ok(names)
}

/// Records that the innermost open sequence consumes input.
#[inline]
fn set_consumes(consumes: &mut [bool]) {
    if let Some(flag) = consumes.last_mut() {
        *flag = true;
    }
}

/// Emits the loop-back of a repetition and patches its entry split.
///
/// The emitted shapes, with `B` the body and `X` the exit:
///
/// ```text
/// *  no separator:  Enter  Split(I, X)  I: Iter B  Split(I, X)          X:
/// *  separator s:   Enter  Split(I, X)  I: Iter B  Split(S, X) S: s Jump(I)  X:
/// +  no separator:  Enter               I: Iter B  Split(I, X)          X:
/// +  separator s:   Enter               I: Iter B  Split(S, X) S: s Jump(I)  X:
/// ?                 Enter  Split(I, X)  I: Iter B                       X:
/// ```
fn close_repeat<K>(program: &mut Vec<Inst<K>>, frame: RepeatFrame<K>) {
    if frame.kleene != Kleene::ZeroOrOne {
        let split = program.len();
        match frame.separator {
            Some(separator) => {
                program.push(Inst::Split(split + 1, split + 3));
                program.push(Inst::Token(separator));
                program.push(Inst::Jump(frame.iterate));
            }
            None => program.push(Inst::Split(frame.iterate, split + 1)),
        }
    }
    let exit = program.len();
    if let Some(entry) = frame.entry {
        if let Some(inst) = program.get_mut(entry) {
            *inst = Inst::Split(frame.iterate, exit);
        }
    }
}

/// Pending work while lowering a template.
enum TemplateWork<K> {
    Element(Template<K>),
    CloseGroup,
    End { start: usize },
}

/// Lowers a template into `out.ops`, resolving metavariable names to slots and
/// collecting each repetition's lockstep slots.
fn compile_template<K>(
    template: Vec<Template<K>>,
    rule: usize,
    names: &BTreeMap<Symbol, usize>,
    out: &mut CompiledRule<K>,
) -> Result<(), MacroError> {
    // One lockstep list per open template repetition, outermost first. A slot
    // is in the list for depth `t` when the pattern captured it inside at least
    // `t` repetitions — it then supplies the iteration count at that depth.
    let mut open: Vec<Vec<usize>> = Vec::new();
    let mut work: Vec<TemplateWork<K>> = template
        .into_iter()
        .rev()
        .map(TemplateWork::Element)
        .collect();

    while let Some(item) = work.pop() {
        match item {
            TemplateWork::Element(Template::Token { token, ctx }) => {
                out.ops.push(Op::Token(token, ctx));
            }
            TemplateWork::Element(Template::Group {
                open: open_token,
                close,
                body,
            }) => {
                out.ops.push(Op::Open(open_token, close));
                work.push(TemplateWork::CloseGroup);
                work.extend(body.into_iter().rev().map(TemplateWork::Element));
            }
            TemplateWork::CloseGroup => out.ops.push(Op::Close),
            TemplateWork::Element(Template::Var(name)) => {
                let slot = *names
                    .get(&name)
                    .ok_or(MacroError::UnboundVariable { rule, name })?;
                let depth = out.slots.get(slot).map_or(0, |info| info.depth);
                if depth > open.len() {
                    return Err(MacroError::StillRepeating { rule, name });
                }
                for locks in open.iter_mut().take(depth) {
                    if !locks.contains(&slot) {
                        locks.push(slot);
                    }
                }
                out.ops.push(Op::Var(slot));
            }
            TemplateWork::Element(Template::Repeat { body, separator }) => {
                // A plain `Repeat` separator is defined in the root context.
                let separator = separator.map(|token| (token, Context::ROOT));
                open_repeat(body, separator, &mut open, &mut work, out);
            }
            TemplateWork::Element(Template::RepeatSeparated {
                body,
                separator,
                ctx,
            }) => open_repeat(body, Some((separator, ctx)), &mut open, &mut work, out),
            TemplateWork::End { start } => {
                let locks = open.pop().unwrap_or_default();
                if locks.is_empty() {
                    return Err(MacroError::NoRepeatingVariable { rule });
                }
                let range = (out.locks.len(), locks.len());
                out.locks.extend_from_slice(&locks);
                let end = out.ops.len();
                out.ops.push(Op::End { start });
                if let Some(Op::Repeat {
                    end: patch_end,
                    locks: patch_locks,
                    ..
                }) = out.ops.get_mut(start)
                {
                    *patch_end = end;
                    *patch_locks = range;
                }
            }
        }
    }
    Ok(())
}

/// Emits the opening of a template repetition (its end and lockstep range are
/// patched when its [`TemplateWork::End`] is reached) and queues its body.
fn open_repeat<K>(
    body: Vec<Template<K>>,
    separator: Option<(Token<K>, Context)>,
    open: &mut Vec<Vec<usize>>,
    work: &mut Vec<TemplateWork<K>>,
    out: &mut CompiledRule<K>,
) {
    let start = out.ops.len();
    out.ops.push(Op::Repeat {
        end: 0,
        locks: (0, 0),
        separator,
    });
    open.push(Vec::new());
    work.push(TemplateWork::End { start });
    work.extend(body.into_iter().rev().map(TemplateWork::Element));
}
