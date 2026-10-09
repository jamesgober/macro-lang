//! Writing out a matched rule's template.
//!
//! The template is a flat op list, so transcription is one loop over it with
//! two explicit stacks — open output groups and open repetitions — rather than a
//! recursive walk. Captures are looked up by repetition path through the
//! matcher's compact arrays: for a metavariable captured inside repetitions
//! `R1 .. Rd`, the iteration indices of the enclosing template repetitions pick
//! one instance at each level, and the final linear index selects the tree.
//!
//! Every token written is charged against the expander's token budget *before*
//! it is written, so an expansion that would exceed the budget stops at the
//! limit instead of first building an arbitrarily large output.

use alloc::vec::Vec;

use crate::compile::{CompiledRule, Op};
use crate::context::Hygiene;
use crate::matcher::{Matcher, OpenGroup};
use crate::{Context, ExpandError, Limit, Tree};

/// One open template repetition.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    /// The current iteration, counting from zero.
    index: usize,
    /// How many iterations this repetition runs.
    count: usize,
    /// The context minted for this repetition's separator, once the first
    /// separator has been written; every later one reuses it instead of
    /// searching the expansion's mark cache again.
    separator: Option<Context>,
}

/// Pooled stacks for transcription, kept in the expander between expansions.
#[derive(Clone, Debug)]
pub(crate) struct Scratch<K> {
    frames: Vec<Frame>,
    groups: Vec<OpenGroup<K>>,
    /// `(definition context, minted context)` pairs for the current expansion.
    marks: Vec<(Context, Context)>,
}

impl<K> Default for Scratch<K> {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            groups: Vec::new(),
            marks: Vec::new(),
        }
    }
}

/// Everything transcription needs about the expansion in progress.
pub(crate) struct Expansion<'a, K> {
    pub(crate) rule: &'a CompiledRule<K>,
    pub(crate) index: usize,
    pub(crate) matcher: &'a Matcher<K>,
    pub(crate) hygiene: &'a mut Hygiene,
    pub(crate) expansion: usize,
    /// How many more tokens the budget allows this expansion to write.
    pub(crate) tokens_left: usize,
    /// The budget's token limit, for the error report.
    pub(crate) tokens_max: usize,
}

/// The token budget remaining to one expansion.
struct Meter {
    left: usize,
    max: usize,
}

impl Meter {
    /// The error for running out, kept out of line so the hot path stays a
    /// subtraction and a branch.
    #[cold]
    #[inline(never)]
    fn exceeded(&self) -> ExpandError {
        ExpandError::Budget(Limit::Tokens { max: self.max })
    }

    /// Takes `n` tokens from the budget, or fails without taking any.
    #[inline]
    fn charge(&mut self, n: usize) -> Result<(), ExpandError> {
        match self.left.checked_sub(n) {
            Some(left) => {
                self.left = left;
                Ok(())
            }
            None => Err(self.exceeded()),
        }
    }
}

/// Writes the template of `ex.rule` into a new tree list. Returns the trees
/// and the number of tokens written.
pub(crate) fn transcribe<K: Clone>(
    ex: Expansion<'_, K>,
    scratch: &mut Scratch<K>,
) -> Result<(Vec<Tree<K>>, usize), ExpandError> {
    let Expansion {
        rule,
        index,
        matcher,
        hygiene,
        expansion,
        tokens_left,
        tokens_max,
    } = ex;
    let mut meter = Meter {
        left: tokens_left,
        max: tokens_max,
    };
    let mismatch = ExpandError::RepetitionMismatch { rule: index };
    scratch.frames.clear();
    scratch.groups.clear();
    scratch.marks.clear();

    let mut out = Vec::new();
    let mut pc = 0;
    while let Some(op) = rule.ops.get(pc) {
        match op {
            Op::Token(token, ctx) => {
                meter.charge(1)?;
                let ctx = hygiene
                    .mark(*ctx, expansion, &mut scratch.marks)
                    .ok_or(ExpandError::ContextOverflow)?;
                current(&mut out, &mut scratch.groups).push(Tree::Token {
                    token: token.clone(),
                    ctx,
                });
            }
            Op::Open(open, close) => {
                // Both delimiters are charged when the group opens.
                meter.charge(2)?;
                scratch
                    .groups
                    .push((open.clone(), close.clone(), Vec::new()));
            }
            Op::Close => {
                if let Some((open, close, trees)) = scratch.groups.pop() {
                    current(&mut out, &mut scratch.groups).push(Tree::Group { open, close, trees });
                }
            }
            Op::Var(slot) => {
                let chain = rule.chain(*slot);
                let instance =
                    locate(matcher, chain, &scratch.frames, chain.len()).ok_or(mismatch)?;
                let at = matcher
                    .leaves
                    .get(*slot)
                    .and_then(|leaves| leaves.get(instance))
                    .ok_or(mismatch)?;
                // Charged before the capture is cloned, so an oversized
                // capture is refused without being copied.
                if !matcher.emit(*at, current(&mut out, &mut scratch.groups), &mut meter.left) {
                    return Err(meter.exceeded());
                }
            }
            Op::Repeat { end, locks, .. } => {
                let count = lockstep(rule, matcher, *locks, &scratch.frames).ok_or(mismatch)?;
                if count == 0 {
                    pc = *end;
                } else {
                    scratch.frames.push(Frame {
                        index: 0,
                        count,
                        separator: None,
                    });
                }
            }
            Op::End { start } => {
                if let Some(frame) = scratch.frames.last_mut() {
                    frame.index += 1;
                    if frame.index < frame.count {
                        if let Some(Op::Repeat {
                            separator: Some((separator, def)),
                            ..
                        }) = rule.ops.get(*start)
                        {
                            // A separator is a literal token of the template:
                            // it is marked from its own definition context,
                            // sharing the minted context of every other
                            // literal defined there.
                            meter.charge(1)?;
                            let ctx = match frame.separator {
                                Some(ctx) => ctx,
                                None => {
                                    let ctx = hygiene
                                        .mark(*def, expansion, &mut scratch.marks)
                                        .ok_or(ExpandError::ContextOverflow)?;
                                    frame.separator = Some(ctx);
                                    ctx
                                }
                            };
                            current(&mut out, &mut scratch.groups).push(Tree::Token {
                                token: separator.clone(),
                                ctx,
                            });
                        }
                        pc = *start;
                    } else {
                        scratch.frames.truncate(scratch.frames.len() - 1);
                    }
                }
            }
        }
        pc += 1;
    }
    Ok((out, tokens_left.saturating_sub(meter.left)))
}

/// The list output is currently being written to: the innermost open group, or
/// the top level.
#[inline]
fn current<'a, K>(
    out: &'a mut Vec<Tree<K>>,
    groups: &'a mut [OpenGroup<K>],
) -> &'a mut Vec<Tree<K>> {
    match groups.last_mut() {
        Some((_, _, trees)) => trees,
        None => out,
    }
}

/// Follows a slot's repetition chain `levels` deep using the iteration indices
/// of the enclosing template repetitions, returning the linear index reached:
/// the instance of the next repetition down, or — at full depth — the index of
/// the captured tree.
#[inline]
fn locate<K>(
    matcher: &Matcher<K>,
    chain: &[usize],
    frames: &[Frame],
    levels: usize,
) -> Option<usize> {
    let mut linear = 0;
    for (rep, frame) in chain.iter().zip(frames).take(levels) {
        linear = matcher.starts.get(*rep)?.get(linear)? + frame.index;
    }
    Some(linear)
}

/// The iteration count of the repetition about to open, taken from every
/// lockstep slot; `None` if they disagree.
fn lockstep<K>(
    rule: &CompiledRule<K>,
    matcher: &Matcher<K>,
    (start, len): (usize, usize),
    frames: &[Frame],
) -> Option<usize> {
    let depth = frames.len();
    let mut agreed: Option<usize> = None;
    for &slot in rule.locks.get(start..start + len)? {
        let chain = rule.chain(slot);
        let instance = locate(matcher, chain, frames, depth)?;
        let count = *matcher.counts.get(*chain.get(depth)?)?.get(instance)?;
        match agreed {
            Some(previous) if previous != count => return None,
            _ => agreed = Some(count),
        }
    }
    agreed
}
