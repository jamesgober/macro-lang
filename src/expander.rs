//! The expander: matching, transcription, the hygiene table, and the budget,
//! in one place.

use alloc::vec::Vec;
use core::fmt;

use token_lang::Span;

use crate::context::Hygiene;
use crate::matcher::{Matcher, Outcome};
use crate::transcribe::{Expansion, Scratch, transcribe};
use crate::{Budget, Context, ExpandError, Limit, Macro, Origin, Tree, Usage};

/// Expands macro invocations and keeps the hygiene record of every expansion.
///
/// An expander owns three things:
///
/// - the **hygiene table** — every [`Context`] it has minted and the expansion
///   that minted it, queried through [`origin`](Expander::origin);
/// - a **[`Budget`]** — limits on nesting depth, on the number of expansions,
///   and on the number of tokens written, so that no macro, however hostile,
///   can make the expander run forever or exhaust memory; and
/// - **pooled scratch buffers** for matching and transcription, reused by every
///   call so that steady-state expansion allocates only the output trees.
///
/// Use one expander for a whole compilation (or one per thread), so that every
/// context in the program comes from one table. The expander does not find
/// invocations or look macros up by name — that is the job of the language's
/// expansion driver, which knows its syntax and scoping rules. The driver
/// finds an invocation, resolves the macro, calls [`expand`](Expander::expand)
/// (or [`expand_at`](Expander::expand_at), when it tracks where each
/// invocation was found), splices the result back in, and repeats on the
/// output until no invocations remain.
///
/// # Examples
///
/// ```
/// use macro_lang::{Context, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
/// use intern_lang::Interner;
/// use token_lang::Span;
///
/// let mut names = Interner::new();
/// let x = names.intern("x");
///
/// // twice!($x:tt) => $x $x
/// let twice = Macro::new(names.intern("twice"), vec![Rule {
///     pattern: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
///     template: vec![Template::Var(x), Template::Var(x)],
/// }])?;
///
/// let mut expander = Expander::new();
/// let input = [Tree::token('a', Span::new(7, 8))];
/// let out = expander.expand(&twice, &input, Span::new(0, 9), Context::ROOT)?;
/// assert_eq!(out, vec![input[0].clone(), input[0].clone()]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Expander<K> {
    hygiene: Hygiene,
    matcher: Matcher<K>,
    scratch: Scratch<K>,
    budget: Budget,
    /// The work charged against the budget since creation or the last
    /// [`reset_usage`](Expander::reset_usage): successful expansions and the
    /// tokens they wrote. Kept apart from the hygiene table, which is never
    /// reset, so a host can start a fresh unit of work without invalidating
    /// any context it has already handed out.
    usage: Usage,
}

impl<K> Expander<K> {
    /// The recursion limit [`Expander::new`] uses: 128 nested expansions, the
    /// same default as rustc's `recursion_limit`. Equal to
    /// [`Budget::DEFAULT`]`.max_depth`.
    pub const DEFAULT_LIMIT: u32 = Budget::DEFAULT.max_depth;

    /// Creates an expander with the [default budget](Budget::DEFAULT): a
    /// recursion limit of [128](Self::DEFAULT_LIMIT), `2^20` expansions, and
    /// `2^22` tokens.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Budget, Expander};
    ///
    /// let expander: Expander<char> = Expander::new();
    /// assert_eq!(expander.limit(), Expander::<char>::DEFAULT_LIMIT);
    /// assert_eq!(expander.budget(), Budget::DEFAULT);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::with_budget(Budget::DEFAULT)
    }

    /// Creates an expander that allows invocations nested at most `limit`
    /// expansions deep, with the rest of the [default budget](Budget::DEFAULT).
    ///
    /// An invocation in source runs at depth 1. An invocation runs one level
    /// deeper than the deepest expansion that produced its name token
    /// (`call_context`) or any token of its input, and — through
    /// [`expand_at`](Expander::expand_at) — one level deeper than the code the
    /// driver found it in. A macro that expands into a call of itself without
    /// a base case therefore stops with [`ExpandError::RecursionLimit`] instead
    /// of looping forever. A limit of `0` rejects every expansion.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Expander;
    ///
    /// let expander: Expander<char> = Expander::with_limit(16);
    /// assert_eq!(expander.limit(), 16);
    /// ```
    #[must_use]
    pub fn with_limit(limit: u32) -> Self {
        Self::with_budget(Budget::DEFAULT.with_max_depth(limit))
    }

    /// Creates an expander that enforces `budget`.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Budget, Expander};
    ///
    /// let budget = Budget::DEFAULT.with_max_expansions(10_000).with_max_tokens(100_000);
    /// let expander: Expander<char> = Expander::with_budget(budget);
    /// assert_eq!(expander.budget(), budget);
    /// ```
    #[must_use]
    pub fn with_budget(budget: Budget) -> Self {
        Self {
            hygiene: Hygiene::default(),
            matcher: Matcher::default(),
            scratch: Scratch::default(),
            budget,
            usage: Usage::NONE,
        }
    }

    /// Returns the recursion limit: the budget's
    /// [`max_depth`](Budget::max_depth).
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Expander;
    ///
    /// assert_eq!(Expander::<char>::with_limit(4).limit(), 4);
    /// ```
    #[inline]
    #[must_use]
    pub const fn limit(&self) -> u32 {
        self.budget.max_depth
    }

    /// Returns the budget this expander enforces.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Budget, Expander};
    ///
    /// assert_eq!(Expander::<char>::new().budget(), Budget::DEFAULT);
    /// assert_eq!(Expander::<char>::with_limit(9).budget().max_depth, 9);
    /// ```
    #[inline]
    #[must_use]
    pub const fn budget(&self) -> Budget {
        self.budget
    }

    /// Replaces the budget this expander enforces from now on.
    ///
    /// Work already done stays counted: the expansion and token limits apply
    /// to the [`usage`](Expander::usage) accumulated since the expander was
    /// created or last [reset](Expander::reset_usage), so raising them lets
    /// the current unit of work continue, and lowering them below what it has
    /// already done makes every further expansion fail with
    /// [`ExpandError::Budget`].
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Budget, Context, ExpandError, Expander, Limit, Macro, Rule};
    /// use intern_lang::Interner;
    /// use token_lang::Span;
    ///
    /// let mut names = Interner::new();
    /// let empty = Macro::<char>::new(names.intern("empty"), vec![Rule {
    ///     pattern: vec![],
    ///     template: vec![],
    /// }])?;
    ///
    /// let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(1));
    /// assert!(expander.expand(&empty, &[], Span::new(0, 1), Context::ROOT).is_ok());
    /// assert_eq!(
    ///     expander.expand(&empty, &[], Span::new(0, 1), Context::ROOT),
    ///     Err(ExpandError::Budget(Limit::Expansions { max: 1 })),
    /// );
    ///
    /// // Raising the budget lets the expander continue.
    /// expander.set_budget(Budget::DEFAULT.with_max_expansions(2));
    /// assert!(expander.expand(&empty, &[], Span::new(0, 1), Context::ROOT).is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline]
    pub fn set_budget(&mut self, budget: Budget) {
        self.budget = budget;
    }

    /// Returns the work charged against the budget since the expander was
    /// created or [`reset_usage`](Expander::reset_usage) was last called:
    /// the number of successful expansions and the tokens they wrote.
    ///
    /// Failed expansions are never charged.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Context, Expander, Macro, Rule, Template, Usage};
    /// use intern_lang::Interner;
    /// use token_lang::Span;
    ///
    /// let mut names = Interner::new();
    /// let s = Span::new(0, 1);
    /// // three!() => a b c
    /// let three = Macro::new(names.intern("three"), vec![Rule {
    ///     pattern: vec![],
    ///     template: "abc".chars().map(|c| Template::token(c, s)).collect(),
    /// }])?;
    ///
    /// let mut expander = Expander::new();
    /// assert_eq!(expander.usage(), Usage::NONE);
    /// expander.expand(&three, &[], s, Context::ROOT)?;
    /// expander.expand(&three, &[], s, Context::ROOT)?;
    /// assert_eq!(expander.usage().expansions, 2);
    /// assert_eq!(expander.usage().tokens, 6);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline]
    #[must_use]
    pub const fn usage(&self) -> Usage {
        self.usage
    }

    /// Zeroes the [`usage`](Expander::usage) counters, giving the next unit
    /// of work the full budget again.
    ///
    /// A long-lived host — a language server, a REPL, a watch-mode compiler —
    /// calls this once per unit of work: per document parse, per REPL entry,
    /// per compilation. The budget then bounds each unit, not the host's whole
    /// lifetime. Nothing else changes: the budget itself, every context
    /// already minted, and every [`Origin`] stay exactly as they were, so trees
    /// produced before the reset remain valid. (The hygiene table still only
    /// grows; a host that runs for billions of expansions eventually sees
    /// [`ExpandError::ContextOverflow`] and must start a fresh expander.)
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Budget, Context, ExpandError, Expander, Limit, Macro, Rule, Template, Tree, Usage};
    /// use intern_lang::Interner;
    /// use token_lang::Span;
    ///
    /// let mut names = Interner::new();
    /// let s = Span::new(0, 1);
    /// let one = Macro::new(names.intern("one"), vec![Rule {
    ///     pattern: vec![],
    ///     template: vec![Template::token('k', s)],
    /// }])?;
    ///
    /// // A REPL allowing two expansions per entry.
    /// let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(2));
    /// let mut kept = Vec::new();
    /// for _entry in 0..3 {
    ///     expander.reset_usage();
    ///     kept.extend(expander.expand(&one, &[], s, Context::ROOT)?);
    ///     kept.extend(expander.expand(&one, &[], s, Context::ROOT)?);
    ///     assert_eq!(
    ///         expander.expand(&one, &[], s, Context::ROOT),
    ///         Err(ExpandError::Budget(Limit::Expansions { max: 2 })),
    ///     );
    /// }
    /// assert_eq!(expander.usage().expansions, 2);
    ///
    /// // Contexts minted before a reset still resolve.
    /// let Some(Tree::Token { ctx, .. }) = kept.first() else {
    ///     return Err("expected a token".into());
    /// };
    /// assert!(expander.origin(*ctx).is_some());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline]
    pub fn reset_usage(&mut self) {
        self.usage = Usage::NONE;
    }

    /// Reports which expansion minted `ctx`, or `None` for [`Context::ROOT`] and
    /// for contexts this expander did not mint.
    ///
    /// This is the query a name resolver and a diagnostic renderer need: the
    /// [`Origin`] gives the context to fall back to for definition-site
    /// resolution, the macro that introduced the token, and the invocation that
    /// triggered it.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Context, Expander, Macro, Rule, Template, Tree};
    /// use intern_lang::Interner;
    /// use token_lang::Span;
    ///
    /// let mut names = Interner::new();
    /// let make_tmp = names.intern("make_tmp");
    /// let mac = Macro::new(make_tmp, vec![Rule {
    ///     pattern: vec![],
    ///     template: vec![Template::token('t', Span::new(40, 41))],
    /// }])?;
    ///
    /// let mut expander = Expander::new();
    /// let out = expander.expand(&mac, &[], Span::new(3, 14), Context::ROOT)?;
    /// let ctx = match out.first() {
    ///     Some(Tree::Token { ctx, .. }) => *ctx,
    ///     _ => return Err("expected one token".into()),
    /// };
    ///
    /// let origin = expander.origin(ctx).ok_or("context not minted by this expander")?;
    /// assert_eq!(origin.macro_name, make_tmp);
    /// assert_eq!(origin.call_site, Span::new(3, 14));
    /// assert_eq!(expander.origin(Context::ROOT), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline]
    #[must_use]
    pub fn origin(&self, ctx: Context) -> Option<Origin> {
        self.hygiene.origin(ctx)
    }
}

impl<K: Clone + PartialEq> Expander<K> {
    /// Expands one invocation of `mac`.
    ///
    /// The rules of `mac` are tried in order against `input`, and the first
    /// rule whose pattern matches all of it is transcribed. Every literal token
    /// the template writes receives a context minted for this expansion; every
    /// captured tree is substituted with its contexts untouched.
    ///
    /// This is [`expand_at`](Expander::expand_at) with a depth of `0`: the
    /// invocation's nesting depth is inferred from hygiene alone (see
    /// [`with_limit`](Expander::with_limit)). That inference cannot see an
    /// invocation rebuilt entirely from tokens captured out of source — a
    /// macro that writes `$name ! ( $args )` from its own input reproduces a
    /// call that is token-for-token identical to the one it came from. A
    /// driver that knows where it found each invocation should call
    /// `expand_at`, which bounds such a macro by the recursion limit; with
    /// `expand`, the [`Budget`]'s expansion and token limits are what stop
    /// it.
    ///
    /// # Parameters
    ///
    /// - `mac` — the macro being invoked.
    /// - `input` — the invocation's argument trees: the contents between the
    ///   invocation's delimiters, without the delimiters themselves.
    /// - `call_site` — the span of the whole invocation, recorded in the
    ///   [`Origin`] of every minted context and used to report a mismatch on an
    ///   empty invocation.
    /// - `call_context` — the context of the invocation itself, normally the
    ///   context of the macro-name token. Pass [`Context::ROOT`] for an
    ///   invocation written in source; pass the token's context for one produced
    ///   by an earlier expansion, so that nesting depth is tracked.
    ///
    /// # Errors
    ///
    /// - [`ExpandError::RecursionLimit`] if the invocation is nested deeper than
    ///   the [limit](Expander::limit).
    /// - [`ExpandError::Budget`] if the expander has already performed
    ///   [`Budget::max_expansions`] expansions, or if this one would take the
    ///   number of tokens written past [`Budget::max_tokens`].
    /// - [`ExpandError::NoMatch`] if no rule matches.
    /// - [`ExpandError::Ambiguous`] if a rule matches in more than one way.
    /// - [`ExpandError::RepetitionMismatch`] if a template repetition's
    ///   metavariables captured different numbers of trees.
    /// - [`ExpandError::ContextOverflow`] if the context space is exhausted.
    ///
    /// On error the expander is left exactly as it was.
    ///
    /// # Examples
    ///
    /// A `vec`-style macro: `list!(a, b, c)` expands to `[a; b; c]`.
    ///
    /// ```
    /// use macro_lang::{Context, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
    /// use intern_lang::Interner;
    /// use token_lang::{Span, Token};
    ///
    /// let mut names = Interner::new();
    /// let e = names.intern("e");
    /// let s = Span::new(0, 0); // spans of the definition, unimportant here
    /// let list = Macro::new(names.intern("list"), vec![Rule {
    ///     pattern: vec![Pattern::Repeat {
    ///         body: vec![Pattern::Bind { name: e, fragment: Fragment::Tree }],
    ///         separator: Some(','),
    ///         kleene: Kleene::ZeroOrMore,
    ///     }],
    ///     template: vec![Template::Group {
    ///         open: Token::new('[', s),
    ///         close: Token::new(']', s),
    ///         body: vec![Template::Repeat {
    ///             body: vec![Template::Var(e)],
    ///             separator: Some(Token::new(';', s)),
    ///         }],
    ///     }],
    /// }])?;
    ///
    /// let input: Vec<Tree<char>> = "a,b,c"
    ///     .char_indices()
    ///     .map(|(i, c)| Tree::token(c, Span::new(i as u32, i as u32 + 1)))
    ///     .collect();
    /// let mut expander = Expander::new();
    /// let out = expander.expand(&list, &input, Span::new(0, 13), Context::ROOT)?;
    ///
    /// let Some(Tree::Group { trees, .. }) = out.first() else {
    ///     return Err("expected one group".into());
    /// };
    /// let kinds: String = trees
    ///     .iter()
    ///     .filter_map(|t| match t { Tree::Token { token, .. } => Some(token.kind), _ => None })
    ///     .collect();
    /// assert_eq!(kinds, "a;b;c");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline]
    pub fn expand(
        &mut self,
        mac: &Macro<K>,
        input: &[Tree<K>],
        call_site: Span,
        call_context: Context,
    ) -> Result<Vec<Tree<K>>, ExpandError> {
        self.expand_at(mac, input, call_site, call_context, 0)
    }

    /// Expands one invocation of `mac` found in code nested `depth` expansions
    /// deep.
    ///
    /// Identical to [`expand`](Expander::expand), except that the driver also
    /// says where it found the invocation: `depth` is `0` for an invocation
    /// written in source, and `d + 1` for one found in the output of a call
    /// made with depth `d`. The invocation then runs at least one level deeper
    /// than `depth` — deeper still if hygiene shows its name token or input was
    /// produced further down — so the [recursion limit](Expander::limit) bounds
    /// every chain of nested expansions the driver performs, whatever the
    /// macros write. That includes a macro that rebuilds its own invocation
    /// from captured tokens, which [`expand`](Expander::expand) alone cannot
    /// tell apart from the original call.
    ///
    /// # Parameters
    ///
    /// As for [`expand`](Expander::expand), plus:
    ///
    /// - `depth` — the nesting depth of the code the invocation was found in.
    ///
    /// # Errors
    ///
    /// As for [`expand`](Expander::expand). On error the expander is left
    /// exactly as it was.
    ///
    /// # Examples
    ///
    /// A driver that expands each output before splicing it threads the depth
    /// through. Here `again!($n:tt $b:tt)` writes back exactly what it was
    /// given, `$n $b ( $n $b )` — a fresh invocation of itself made only of
    /// captured tokens — and `expand_at` still stops it at the limit:
    ///
    /// ```
    /// use macro_lang::{Context, ExpandError, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
    /// use intern_lang::Interner;
    /// use token_lang::{Span, Token};
    ///
    /// let mut names = Interner::new();
    /// let (n, b) = (names.intern("n"), names.intern("b"));
    /// let tt = |name| Pattern::Bind { name, fragment: Fragment::Tree };
    /// let s = Span::new(0, 1);
    /// let again = Macro::new(names.intern("again"), vec![Rule {
    ///     pattern: vec![tt(n), tt(b)],
    ///     template: vec![
    ///         Template::Var(n),
    ///         Template::Var(b),
    ///         Template::Group {
    ///             open: Token::new('(', s),
    ///             close: Token::new(')', s),
    ///             body: vec![Template::Var(n), Template::Var(b)],
    ///         },
    ///     ],
    /// }])?;
    ///
    /// let mut expander = Expander::with_limit(8);
    /// // `a ! ( a ! )`, with `a` naming `again`: the arguments are the trees
    /// // inside the group.
    /// let mut args = vec![Tree::token('a', Span::new(7, 8)), Tree::token('!', Span::new(8, 9))];
    /// let mut depth = 0;
    /// let err = loop {
    ///     match expander.expand_at(&again, &args, s, Context::ROOT, depth) {
    ///         // The output is `again ! ( ... )` again: recurse into it.
    ///         Ok(out) => match out.get(2) {
    ///             Some(Tree::Group { trees, .. }) => {
    ///                 args = trees.clone();
    ///                 depth += 1;
    ///             }
    ///             _ => break None,
    ///         },
    ///         Err(err) => break Some(err),
    ///     }
    /// };
    /// assert_eq!(err, Some(ExpandError::RecursionLimit { limit: 8 }));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn expand_at(
        &mut self,
        mac: &Macro<K>,
        input: &[Tree<K>],
        call_site: Span,
        call_context: Context,
        depth: u32,
    ) -> Result<Vec<Tree<K>>, ExpandError> {
        self.matcher.load(input);
        let depth = self.depth_of(call_context, depth).saturating_add(1);
        if depth > self.budget.max_depth {
            return Err(ExpandError::RecursionLimit {
                limit: self.budget.max_depth,
            });
        }
        if self.usage.expansions >= self.budget.max_expansions {
            return Err(ExpandError::Budget(Limit::Expansions {
                max: self.budget.max_expansions,
            }));
        }

        let mut furthest = 0;
        for (index, rule) in mac.rules().iter().enumerate() {
            match self.matcher.run(rule) {
                Outcome::Match => {
                    let checkpoint = self.hygiene.checkpoint();
                    let expansion = self
                        .hygiene
                        .begin(mac.name(), call_site, call_context, depth);
                    let result = transcribe(
                        Expansion {
                            rule,
                            index,
                            matcher: &self.matcher,
                            hygiene: &mut self.hygiene,
                            expansion,
                            tokens_left: self.budget.max_tokens.saturating_sub(self.usage.tokens),
                            tokens_max: self.budget.max_tokens,
                        },
                        &mut self.scratch,
                    );
                    return match result {
                        Ok((out, written)) => {
                            self.usage.tokens = self.usage.tokens.saturating_add(written);
                            self.usage.expansions = self.usage.expansions.saturating_add(1);
                            Ok(out)
                        }
                        Err(err) => {
                            self.hygiene.rollback(checkpoint);
                            Err(err)
                        }
                    };
                }
                Outcome::Ambiguous => return Err(ExpandError::Ambiguous { rule: index }),
                Outcome::NoMatch { furthest: reached } => furthest = furthest.max(reached),
            }
        }
        Err(ExpandError::NoMatch {
            span: self.matcher.span_at(furthest, call_site),
        })
    }

    /// The depth of the code the loaded invocation belongs to: the deepest of
    /// the driver's `depth`, the expansion that produced the name token, and
    /// the expansions that produced any token of the input.
    ///
    /// Counting the input closes the gap a name token alone leaves: a macro
    /// that rebuilds its own invocation around a captured name still writes
    /// its own literal tokens into the arguments, and those carry its depth.
    fn depth_of(&self, call_context: Context, depth: u32) -> u32 {
        let mut deepest = depth.max(self.hygiene.depth(call_context));
        if self.matcher.marked {
            // Runs of tokens share a context, so look each run up once.
            let mut last = Context::ROOT;
            for ctx in self.matcher.contexts() {
                if ctx != last {
                    deepest = deepest.max(self.hygiene.depth(ctx));
                    last = ctx;
                }
            }
        }
        deepest
    }
}

impl<K> Default for Expander<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K> fmt::Debug for Expander<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (contexts, expansions) = self.hygiene.sizes();
        f.debug_struct("Expander")
            .field("limit", &self.budget.max_depth)
            .field("contexts", &contexts)
            .field("expansions", &expansions)
            .finish()
    }
}
