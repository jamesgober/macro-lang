//! Running a compiled pattern against an invocation.
//!
//! The matcher is a Thompson NFA simulation. The invocation is flattened into a
//! sequence of entries (a token, a group opening, a group closing), and every
//! live parse — a *thread* — sits at some pattern instruction and some input
//! position. Positions are processed strictly left to right: the threads at a
//! position are expanded through epsilon instructions, then each consuming
//! instruction either advances its thread to a later position or kills it.
//!
//! Three properties make this robust against hostile input:
//!
//! - **No backtracking.** Threads that reach the same consuming instruction at
//!   the same position are merged, so at most one thread per instruction is
//!   alive per position. Each position therefore costs at most
//!   `O(pattern²)` epsilon steps, and a whole match `O(input × pattern²)` —
//!   linear in the input, never exponential. Real patterns are small and
//!   usually near-deterministic, where a position costs a handful of steps.
//! - **Nothing scales with nesting on the call stack.** Flattening, matching,
//!   and rebuilding captured groups are loops over explicit buffers.
//! - **Ambiguity is detected, not guessed.** Merging two threads means two
//!   different parses reached the same point; if the merged thread goes on to
//!   accept, the rule matched in more than one way and the expansion is
//!   rejected rather than silently picking one.
//!
//! Captures are recorded as a persistent linked list of events shared between
//! threads, so forking a thread is a copy of one index; the winning thread's
//! list is replayed once at the end into compact per-slot arrays.

use alloc::vec::Vec;
use core::mem;

use token_lang::{Span, Token};

use crate::compile::{CompiledRule, Inst};
use crate::{Context, Fragment, Tree};

/// One entry of the flattened invocation.
#[derive(Clone, Debug)]
pub(crate) enum Flat<K> {
    /// A token.
    Leaf { token: Token<K>, ctx: Context },
    /// The opening of a group; `end` is the index of its [`Flat::Close`].
    Open {
        open: Token<K>,
        close: Token<K>,
        end: usize,
    },
    /// The end of a group, with the closing delimiter's span for diagnostics.
    Close { span: Span },
}

/// A group being rebuilt: its delimiters and the trees collected so far.
pub(crate) type OpenGroup<K> = (Token<K>, Token<K>, Vec<Tree<K>>);

/// A live parse: where it is in the pattern, and its capture history.
#[derive(Clone, Copy, Debug)]
struct Thread {
    pc: usize,
    /// Index of the newest event in this thread's history, or [`NO_EVENT`].
    tail: usize,
    /// Set once two different parses have been merged into this thread.
    ambiguous: bool,
}

/// The `tail` of a thread with no events yet.
const NO_EVENT: usize = usize::MAX;

/// One step of capture history.
#[derive(Clone, Copy, Debug)]
enum Event {
    /// `slot` captured the tree starting at flat index `at`.
    Bind { slot: usize, at: usize },
    /// A new instance of repetition `rep` started.
    Enter(usize),
    /// The current instance of repetition `rep` began another iteration.
    Iter(usize),
}

/// How a run of one rule ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Exactly one parse matched; its captures are loaded.
    Match,
    /// More than one parse matched.
    Ambiguous,
    /// Nothing matched; `furthest` is the last position any parse was alive at.
    NoMatch { furthest: usize },
}

/// The matcher's state and pooled buffers. One lives in each expander and is
/// reused for every expansion, so steady-state matching does not allocate.
#[derive(Clone, Debug)]
pub(crate) struct Matcher<K> {
    /// The flattened invocation.
    pub(crate) flat: Vec<Flat<K>>,
    /// Whether any token of the invocation carries a non-root context — one an
    /// expansion produced. Set while flattening (a branch-free OR), so the
    /// expander scans the input for its depth only when there is something to
    /// find: source input, the common case, costs nothing.
    pub(crate) marked: bool,
    /// Threads arriving at the position being processed.
    current: Vec<Thread>,
    /// Threads arriving at the following position.
    next: Vec<Thread>,
    /// Threads that skipped a whole group, waiting for their target position,
    /// sorted by descending target.
    deferred: Vec<(usize, Thread)>,
    /// Threads at consuming instructions for the position being processed.
    live: Vec<Thread>,
    /// Work stack for epsilon expansion.
    stack: Vec<Thread>,
    /// The shared capture-history arena: `(previous event, event)`.
    events: Vec<(usize, Event)>,
    /// `seen[pc] == generation` iff instruction `pc` already has a live thread
    /// at the current position, at index `seen_at[pc]` of `live`.
    seen: Vec<u64>,
    seen_at: Vec<usize>,
    generation: u64,
    /// Scratch for replaying a winning history in order.
    replay: Vec<Event>,
    /// Per slot: the flat index of every tree it captured, in order.
    pub(crate) leaves: Vec<Vec<usize>>,
    /// Per repetition: the iteration count of each of its instances, in order.
    pub(crate) counts: Vec<Vec<usize>>,
    /// Per repetition: prefix sums of `counts`, so `starts[r][i]` is the
    /// linear index of the first iteration of instance `i`.
    pub(crate) starts: Vec<Vec<usize>>,
}

impl<K> Default for Matcher<K> {
    fn default() -> Self {
        Self {
            flat: Vec::new(),
            marked: false,
            current: Vec::new(),
            next: Vec::new(),
            deferred: Vec::new(),
            live: Vec::new(),
            stack: Vec::new(),
            events: Vec::new(),
            seen: Vec::new(),
            seen_at: Vec::new(),
            generation: 0,
            replay: Vec::new(),
            leaves: Vec::new(),
            counts: Vec::new(),
            starts: Vec::new(),
        }
    }
}

impl<K: Clone> Matcher<K> {
    /// Flattens `input` into [`Matcher::flat`], iteratively so that deeply
    /// nested input cannot overflow the stack.
    pub(crate) fn load(&mut self, input: &[Tree<K>]) {
        self.flat.clear();
        self.marked = false;
        let mut parents: Vec<(core::slice::Iter<'_, Tree<K>>, usize)> = Vec::new();
        let mut trees = input.iter();
        loop {
            match trees.next() {
                Some(Tree::Token { token, ctx }) => {
                    self.marked |= !ctx.is_root();
                    self.flat.push(Flat::Leaf {
                        token: token.clone(),
                        ctx: *ctx,
                    });
                }
                Some(Tree::Group {
                    open,
                    close,
                    trees: inner,
                }) => {
                    parents.push((mem::replace(&mut trees, inner.iter()), self.flat.len()));
                    self.flat.push(Flat::Open {
                        open: open.clone(),
                        close: close.clone(),
                        end: 0,
                    });
                }
                None => {
                    let Some((outer, at)) = parents.pop() else {
                        break;
                    };
                    let end = self.flat.len();
                    let mut span = Span::empty(0);
                    if let Some(Flat::Open {
                        close, end: slot, ..
                    }) = self.flat.get_mut(at)
                    {
                        *slot = end;
                        span = close.span;
                    }
                    self.flat.push(Flat::Close { span });
                    trees = outer;
                }
            }
        }
    }

    /// Rebuilds the tree starting at flat index `at` and appends it to `out`,
    /// provided its size in tokens — both delimiters of every group counted,
    /// which is exactly the flat entries it spans — is at most `*left`. The
    /// size is taken from `*left`. Returns `false`, writing nothing, if the
    /// tree does not fit, so an oversized capture is never copied.
    ///
    /// The single-token case is inlined into the transcription loop; a group
    /// is rebuilt out of line, iteratively for the same reason as
    /// [`Matcher::load`].
    #[inline]
    pub(crate) fn emit(&self, at: usize, out: &mut Vec<Tree<K>>, left: &mut usize) -> bool {
        match self.flat.get(at) {
            Some(Flat::Leaf { token, ctx }) => {
                let Some(rest) = left.checked_sub(1) else {
                    return false;
                };
                *left = rest;
                out.push(Tree::Token {
                    token: token.clone(),
                    ctx: *ctx,
                });
                true
            }
            Some(Flat::Open { end, .. }) => {
                let Some(rest) = left.checked_sub(end.saturating_sub(at) + 1) else {
                    return false;
                };
                *left = rest;
                self.emit_group(at, *end, out);
                true
            }
            // A capture always starts at a token or a group opening.
            Some(Flat::Close { .. }) | None => true,
        }
    }

    /// Rebuilds the group spanning flat entries `at..=end` into `out`.
    fn emit_group(&self, at: usize, end: usize, out: &mut Vec<Tree<K>>) {
        let mut groups: Vec<OpenGroup<K>> = Vec::new();
        for entry in self.flat.get(at..=end).unwrap_or(&[]) {
            match entry {
                Flat::Leaf { token, ctx } => {
                    if let Some((_, _, trees)) = groups.last_mut() {
                        trees.push(Tree::Token {
                            token: token.clone(),
                            ctx: *ctx,
                        });
                    }
                }
                Flat::Open { open, close, .. } => {
                    groups.push((open.clone(), close.clone(), Vec::new()));
                }
                Flat::Close { .. } => {
                    if let Some((open, close, trees)) = groups.pop() {
                        let tree = Tree::Group { open, close, trees };
                        match groups.last_mut() {
                            Some((_, _, parent)) => parent.push(tree),
                            None => out.push(tree),
                        }
                    }
                }
            }
        }
    }
}

impl<K> Matcher<K> {
    /// The contexts of the invocation's tokens, in order; group delimiters
    /// carry none.
    #[inline]
    pub(crate) fn contexts(&self) -> impl Iterator<Item = Context> + '_ {
        self.flat.iter().filter_map(|entry| match entry {
            Flat::Leaf { ctx, .. } => Some(*ctx),
            Flat::Open { .. } | Flat::Close { .. } => None,
        })
    }

    /// The span to blame when matching got no further than `position`: the
    /// entry there, or an empty span just past the input (falling back to
    /// `call_site` for an empty invocation).
    pub(crate) fn span_at(&self, position: usize, call_site: Span) -> Span {
        match self.flat.get(position) {
            Some(Flat::Leaf { token, .. }) => token.span,
            Some(Flat::Open { open, .. }) => open.span,
            Some(Flat::Close { span }) => *span,
            None => match self.flat.last() {
                Some(Flat::Leaf { token, .. }) => Span::empty(token.span.end().to_u32()),
                Some(Flat::Open { open, .. }) => Span::empty(open.span.end().to_u32()),
                Some(Flat::Close { span }) => Span::empty(span.end().to_u32()),
                None => call_site,
            },
        }
    }

    /// Appends an event to the shared history and returns its index.
    #[inline]
    fn record(&mut self, prev: usize, event: Event) -> usize {
        self.events.push((prev, event));
        self.events.len() - 1
    }

    /// Adds a thread at a consuming instruction to the live set, merging it
    /// with any thread already there.
    #[inline]
    fn admit(&mut self, thread: Thread) {
        let pc = thread.pc;
        if self.seen.get(pc) == Some(&self.generation) {
            let index = self.seen_at.get(pc).copied().unwrap_or(usize::MAX);
            if let Some(existing) = self.live.get_mut(index) {
                existing.ambiguous = true;
            }
            return;
        }
        if let (Some(seen), Some(seen_at)) = (self.seen.get_mut(pc), self.seen_at.get_mut(pc)) {
            *seen = self.generation;
            *seen_at = self.live.len();
        }
        self.live.push(thread);
    }

    /// Follows epsilon instructions from `start`, recording repetition events,
    /// and admits every consuming instruction reached.
    fn expand(&mut self, program: &[Inst<K>], start: Thread) {
        self.stack.push(start);
        while let Some(mut thread) = self.stack.pop() {
            loop {
                match program.get(thread.pc) {
                    Some(Inst::Split(first, second)) => {
                        self.stack.push(Thread {
                            pc: *second,
                            ..thread
                        });
                        thread.pc = *first;
                    }
                    Some(Inst::Jump(target)) => thread.pc = *target,
                    Some(Inst::Enter(rep)) => {
                        thread.tail = self.record(thread.tail, Event::Enter(*rep));
                        thread.pc += 1;
                    }
                    Some(Inst::Iter(rep)) => {
                        thread.tail = self.record(thread.tail, Event::Iter(*rep));
                        thread.pc += 1;
                    }
                    Some(_) => {
                        self.admit(thread);
                        break;
                    }
                    None => break,
                }
            }
        }
    }
}

impl<K: PartialEq> Matcher<K> {
    /// Runs one compiled rule against the loaded input. On [`Outcome::Match`]
    /// the captures are ready in `leaves`, `counts`, and `starts`.
    pub(crate) fn run(&mut self, rule: &CompiledRule<K>) -> Outcome {
        let program = rule.program.as_slice();
        let len = self.flat.len();
        if !self.lead_matches(program, rule.lead) {
            return Outcome::NoMatch { furthest: 0 };
        }
        self.events.clear();
        self.deferred.clear();
        if self.seen.len() < program.len() {
            self.seen.resize(program.len(), 0);
            self.seen_at.resize(program.len(), 0);
        }

        // Nearly every step advances one position, so the frontier is a pair of
        // reused vectors; only whole-group skips wait in `deferred`.
        let mut current = mem::take(&mut self.current);
        let mut next = mem::take(&mut self.next);
        current.clear();
        next.clear();
        current.push(Thread {
            pc: 0,
            tail: NO_EVENT,
            ambiguous: false,
        });

        let mut position = 0;
        let mut furthest = 0;
        let mut accepted: Option<Thread> = None;
        loop {
            while let Some(&(target, thread)) = self.deferred.last() {
                if target != position {
                    break;
                }
                current.push(thread);
                self.deferred.truncate(self.deferred.len() - 1);
            }
            if current.is_empty() {
                // Nothing is alive here; resume at the nearest group skip.
                match self.deferred.last() {
                    Some(&(target, _)) => {
                        position = target;
                        continue;
                    }
                    None => break,
                }
            }
            furthest = position;

            self.generation += 1;
            self.live.clear();
            for thread in current.drain(..) {
                self.expand(program, thread);
            }
            for index in 0..self.live.len() {
                let thread = self.live[index];
                if let Some((to, thread)) = self.step(program, thread, position, len, &mut accepted)
                {
                    if to == position + 1 {
                        next.push(thread);
                    } else {
                        // Sorted by descending target, so the nearest skip is
                        // always last.
                        let at = self.deferred.partition_point(|&(t, _)| t > to);
                        self.deferred.insert(at, (to, thread));
                    }
                }
            }
            mem::swap(&mut current, &mut next);
            position += 1;
        }
        self.current = current;
        self.next = next;

        match accepted {
            Some(thread) if thread.ambiguous => Outcome::Ambiguous,
            Some(thread) => {
                self.capture(rule, thread.tail);
                Outcome::Match
            }
            None => Outcome::NoMatch { furthest },
        }
    }

    /// Whether the input can start the way the rule requires. A rule whose
    /// first step is a fixed token or group (`lead`) is rejected here, on the
    /// first entry, without starting the automaton.
    #[inline]
    fn lead_matches(&self, program: &[Inst<K>], lead: Option<usize>) -> bool {
        let Some(pc) = lead else {
            return true;
        };
        match (program.get(pc), self.flat.first()) {
            (Some(Inst::Token(kind)), Some(Flat::Leaf { token, .. })) => token.kind == *kind,
            (
                Some(Inst::Open { open, close }),
                Some(Flat::Open {
                    open: o, close: c, ..
                }),
            ) => o.kind == *open && c.kind == *close,
            _ => false,
        }
    }

    /// Runs one consuming instruction for a live thread. Returns the position
    /// and thread to continue with, or `None` if the thread ends here.
    #[inline]
    fn step(
        &mut self,
        program: &[Inst<K>],
        thread: Thread,
        position: usize,
        len: usize,
        accepted: &mut Option<Thread>,
    ) -> Option<(usize, Thread)> {
        let advance = |thread: Thread, to: usize| {
            Some((
                to,
                Thread {
                    pc: thread.pc + 1,
                    ..thread
                },
            ))
        };
        let entry = self.flat.get(position);
        match (program.get(thread.pc)?, entry) {
            (Inst::Accept, None) if position == len => {
                // The live set holds at most one thread per instruction, so
                // there is at most one accepting thread; merging already folded
                // any competing parse into its `ambiguous` flag.
                *accepted = Some(thread);
                None
            }
            (Inst::Token(kind), Some(Flat::Leaf { token, .. })) if token.kind == *kind => {
                advance(thread, position + 1)
            }
            (
                Inst::Open { open, close },
                Some(Flat::Open {
                    open: input_open,
                    close: input_close,
                    ..
                }),
            ) if input_open.kind == *open && input_close.kind == *close => {
                advance(thread, position + 1)
            }
            (Inst::Close, Some(Flat::Close { .. })) => advance(thread, position + 1),
            (Inst::Bind { slot, fragment }, Some(input)) => {
                let after = match (fragment, input) {
                    (Fragment::Tree, Flat::Leaf { .. }) => position + 1,
                    (Fragment::Tree, Flat::Open { end, .. }) => end + 1,
                    (Fragment::Kind(test), Flat::Leaf { token, .. }) if test(&token.kind) => {
                        position + 1
                    }
                    _ => return None,
                };
                let slot = *slot;
                let tail = self.record(thread.tail, Event::Bind { slot, at: position });
                advance(Thread { tail, ..thread }, after)
            }
            _ => None,
        }
    }
}

impl<K> Matcher<K> {
    /// Replays the winning thread's history into per-slot and per-repetition
    /// arrays.
    fn capture(&mut self, rule: &CompiledRule<K>, tail: usize) {
        reset(&mut self.leaves, rule.slots.len());
        reset(&mut self.counts, rule.reps);
        reset(&mut self.starts, rule.reps);

        self.replay.clear();
        let mut cursor = tail;
        while let Some(&(prev, event)) = self.events.get(cursor) {
            self.replay.push(event);
            cursor = prev;
        }
        for event in self.replay.iter().rev() {
            match *event {
                Event::Bind { slot, at } => {
                    if let Some(leaves) = self.leaves.get_mut(slot) {
                        leaves.push(at);
                    }
                }
                Event::Enter(rep) => {
                    if let Some(counts) = self.counts.get_mut(rep) {
                        counts.push(0);
                    }
                }
                Event::Iter(rep) => {
                    if let Some(count) = self.counts.get_mut(rep).and_then(|c| c.last_mut()) {
                        *count += 1;
                    }
                }
            }
        }
        for (counts, starts) in self.counts.iter().zip(self.starts.iter_mut()) {
            let mut total = 0;
            for &count in counts {
                starts.push(total);
                total += count;
            }
        }
    }
}

/// Empties the first `len` lists, growing `lists` if needed and keeping every
/// allocation for reuse.
fn reset(lists: &mut Vec<Vec<usize>>, len: usize) {
    if lists.len() < len {
        lists.resize_with(len, Vec::new);
    }
    for list in lists.iter_mut().take(len) {
        list.clear();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use alloc::vec;

    use intern_lang::Symbol;

    use super::*;
    use crate::compile::compile_rule;
    use crate::{Kleene, Pattern, Rule};

    fn leaf(kind: u8, at: u32) -> Tree<u8> {
        Tree::token(kind, Span::new(at, at + 1))
    }

    fn group(open: u8, close: u8, trees: Vec<Tree<u8>>, at: u32) -> Tree<u8> {
        Tree::Group {
            open: Token::new(open, Span::new(at, at + 1)),
            close: Token::new(close, Span::new(at + 10, at + 11)),
            trees,
        }
    }

    fn bind(n: u32) -> Pattern<u8> {
        Pattern::Bind {
            name: Symbol::from_u32(n).unwrap(),
            fragment: Fragment::Tree,
        }
    }

    fn star(body: Vec<Pattern<u8>>) -> Pattern<u8> {
        Pattern::Repeat {
            body,
            separator: None,
            kleene: Kleene::ZeroOrMore,
        }
    }

    fn rule(pattern: Vec<Pattern<u8>>) -> CompiledRule<u8> {
        compile_rule(
            Rule {
                pattern,
                template: vec![],
            },
            0,
        )
        .unwrap()
    }

    fn run(pattern: Vec<Pattern<u8>>, input: &[Tree<u8>]) -> (Outcome, Matcher<u8>) {
        let mut matcher = Matcher::default();
        matcher.load(input);
        let outcome = matcher.run(&rule(pattern));
        (outcome, matcher)
    }

    #[test]
    fn test_load_flattens_groups_with_end_links() {
        let input = vec![
            leaf(1, 0),
            group(8, 9, vec![leaf(2, 2), group(8, 9, vec![], 3)], 1),
        ];
        let mut matcher = Matcher::default();
        matcher.load(&input);
        assert_eq!(matcher.flat.len(), 6);
        assert!(matches!(matcher.flat[1], Flat::Open { end: 5, .. }));
        assert!(matches!(matcher.flat[3], Flat::Open { end: 4, .. }));
        assert!(matches!(matcher.flat[4], Flat::Close { .. }));
    }

    #[test]
    fn test_emit_charges_tokens_and_delimiters() {
        let input = vec![
            leaf(1, 0),
            group(8, 9, vec![leaf(2, 2), group(8, 9, vec![], 3)], 1),
        ];
        let mut matcher = Matcher::default();
        matcher.load(&input);
        let mut out = Vec::new();
        // The group is five tokens: it does not fit in four.
        let mut left = 4;
        assert!(!matcher.emit(1, &mut out, &mut left));
        assert_eq!((left, out.len()), (4, 0));
        assert!(matcher.emit(0, &mut out, &mut left));
        assert_eq!(left, 3);
        let mut left = 5;
        assert!(matcher.emit(1, &mut out, &mut left));
        assert_eq!(left, 0);
        assert_eq!(out, input);
        // A leaf does not fit in nothing.
        assert!(!matcher.emit(0, &mut out, &mut left));
        // Source tokens carry the root context, which is not collected.
        assert!(!matcher.marked);
    }

    #[test]
    fn test_emit_rebuilds_nested_group() {
        let input = vec![group(
            8,
            9,
            vec![leaf(2, 2), group(8, 9, vec![leaf(3, 4)], 3)],
            1,
        )];
        let mut matcher = Matcher::default();
        matcher.load(&input);
        let mut out = Vec::new();
        let mut left = usize::MAX;
        assert!(matcher.emit(0, &mut out, &mut left));
        assert_eq!(out, input);
    }

    #[test]
    fn test_empty_pattern_matches_only_empty_input() {
        assert_eq!(run(vec![], &[]).0, Outcome::Match);
        assert_eq!(
            run(vec![], &[leaf(1, 0)]).0,
            Outcome::NoMatch { furthest: 0 }
        );
    }

    #[test]
    fn test_literal_sequence_reports_furthest_position() {
        let pattern = vec![Pattern::Token(1), Pattern::Token(2), Pattern::Token(3)];
        let input = [leaf(1, 0), leaf(2, 1), leaf(4, 2)];
        assert_eq!(
            run(pattern.clone(), &input).0,
            Outcome::NoMatch { furthest: 2 }
        );
        let short = [leaf(1, 0), leaf(2, 1)];
        assert_eq!(run(pattern, &short).0, Outcome::NoMatch { furthest: 2 });
    }

    #[test]
    fn test_tree_fragment_consumes_whole_group() {
        let input = [group(8, 9, vec![leaf(1, 2), leaf(2, 3)], 1), leaf(5, 20)];
        let (outcome, matcher) = run(vec![bind(1), Pattern::Token(5)], &input);
        assert_eq!(outcome, Outcome::Match);
        assert_eq!(matcher.leaves[0], vec![0]);
    }

    #[test]
    fn test_kind_fragment_rejects_groups_and_failing_tokens() {
        let even = Pattern::Bind {
            name: Symbol::from_u32(1).unwrap(),
            fragment: Fragment::Kind(|k: &u8| k % 2 == 0),
        };
        assert_eq!(run(vec![even.clone()], &[leaf(4, 0)]).0, Outcome::Match);
        assert!(matches!(
            run(vec![even.clone()], &[leaf(3, 0)]).0,
            Outcome::NoMatch { .. }
        ));
        let grouped = [group(8, 9, vec![], 0)];
        assert!(matches!(
            run(vec![even], &grouped).0,
            Outcome::NoMatch { .. }
        ));
    }

    #[test]
    fn test_group_delimiters_must_match() {
        let pattern = vec![Pattern::Group {
            open: 8,
            close: 9,
            body: vec![bind(1)],
        }];
        let good = [group(8, 9, vec![leaf(1, 1)], 0)];
        assert_eq!(run(pattern.clone(), &good).0, Outcome::Match);
        let wrong = [group(6, 7, vec![leaf(1, 1)], 0)];
        assert!(matches!(
            run(pattern.clone(), &wrong).0,
            Outcome::NoMatch { .. }
        ));
        // The body must consume the whole group.
        let extra = [group(8, 9, vec![leaf(1, 1), leaf(2, 2)], 0)];
        assert!(matches!(run(pattern, &extra).0, Outcome::NoMatch { .. }));
    }

    #[test]
    fn test_nested_repetition_records_counts_and_starts() {
        // $( ( $($x:tt)* ) )*  against  (a b) () (c)
        let outer = star(vec![Pattern::Group {
            open: 8,
            close: 9,
            body: vec![star(vec![bind(1)])],
        }]);
        let input = [
            group(8, 9, vec![leaf(1, 1), leaf(2, 2)], 0),
            group(8, 9, vec![], 20),
            group(8, 9, vec![leaf(3, 41)], 40),
        ];
        let (outcome, matcher) = run(vec![outer], &input);
        assert_eq!(outcome, Outcome::Match);
        // Repetition 0 is the outer one, repetition 1 the inner one.
        assert_eq!(matcher.counts[0], vec![3]);
        assert_eq!(matcher.counts[1], vec![2, 0, 1]);
        assert_eq!(matcher.starts[1], vec![0, 2, 2]);
        assert_eq!(matcher.leaves[0].len(), 3);
    }

    #[test]
    fn test_split_repetitions_are_ambiguous() {
        let input = [leaf(1, 0), leaf(2, 1)];
        let pattern = || vec![star(vec![bind(1)]), star(vec![bind(2)])];
        assert_eq!(run(pattern(), &input).0, Outcome::Ambiguous);
        // With no input there is exactly one parse: both repetitions empty.
        assert_eq!(run(pattern(), &[]).0, Outcome::Match);
    }

    #[test]
    fn test_trailing_literal_after_tree_repetition_is_unambiguous() {
        // $($t:tt)* ;  — only one parse survives to the end of the input.
        let pattern = vec![star(vec![bind(1)]), Pattern::Token(7)];
        let input = [leaf(1, 0), leaf(7, 1), leaf(2, 2), leaf(7, 3)];
        let (outcome, matcher) = run(pattern, &input);
        assert_eq!(outcome, Outcome::Match);
        assert_eq!(matcher.leaves[0], vec![0, 1, 2]);
    }

    #[test]
    fn test_matcher_is_reusable_across_inputs_of_different_sizes() {
        let pattern = || {
            vec![Pattern::Repeat {
                body: vec![bind(1)],
                separator: Some(0),
                kleene: Kleene::OneOrMore,
            }]
        };
        let long: Vec<Tree<u8>> = (0..39u32)
            .map(|i| leaf(if i % 2 == 0 { 5 } else { 0 }, i))
            .collect();
        let mut matcher = Matcher::default();
        matcher.load(&long);
        assert_eq!(matcher.run(&rule(pattern())), Outcome::Match);
        assert_eq!(matcher.leaves[0].len(), 20);

        matcher.load(&[leaf(5, 0)]);
        assert_eq!(matcher.run(&rule(pattern())), Outcome::Match);
        assert_eq!(matcher.leaves[0], vec![0]);

        matcher.load(&[]);
        assert_eq!(
            matcher.run(&rule(pattern())),
            Outcome::NoMatch { furthest: 0 }
        );
    }

    #[test]
    fn test_span_at_points_at_entry_or_end() {
        let mut matcher = Matcher::default();
        matcher.load(&[leaf(1, 4), group(8, 9, vec![], 6)]);
        let call = Span::new(0, 30);
        assert_eq!(matcher.span_at(0, call), Span::new(4, 5));
        assert_eq!(matcher.span_at(1, call), Span::new(6, 7));
        assert_eq!(matcher.span_at(2, call), Span::new(16, 17));
        assert_eq!(matcher.span_at(3, call), Span::empty(17));
        matcher.load(&[]);
        assert_eq!(matcher.span_at(0, call), call);
    }
}
