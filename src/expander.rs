//! The expander: matching, transcription, and the hygiene table, in one place.

use alloc::vec::Vec;
use core::fmt;

use token_lang::Span;

use crate::context::Hygiene;
use crate::matcher::{Matcher, Outcome};
use crate::transcribe::{Expansion, Scratch, transcribe};
use crate::{Context, ExpandError, Macro, Origin, Tree};

/// Expands macro invocations and keeps the hygiene record of every expansion.
///
/// An expander owns two things:
///
/// - the **hygiene table** — every [`Context`] it has minted and the expansion
///   that minted it, queried through [`origin`](Expander::origin); and
/// - **pooled scratch buffers** for matching and transcription, reused by every
///   call so that steady-state expansion allocates only the output trees.
///
/// Use one expander for a whole compilation (or one per thread), so that every
/// context in the program comes from one table. The expander does not find
/// invocations or look macros up by name — that is the job of the language's
/// expansion driver, which knows its syntax and scoping rules. The driver
/// finds an invocation, resolves the macro, calls [`expand`](Expander::expand),
/// splices the result back in, and repeats on the output until no invocations
/// remain.
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
    limit: u32,
}

impl<K> Expander<K> {
    /// The recursion limit [`Expander::new`] uses: 128 nested expansions, the
    /// same default as rustc's `recursion_limit`.
    pub const DEFAULT_LIMIT: u32 = 128;

    /// Creates an expander with the [default recursion limit](Self::DEFAULT_LIMIT).
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Expander;
    ///
    /// let expander: Expander<char> = Expander::new();
    /// assert_eq!(expander.limit(), Expander::<char>::DEFAULT_LIMIT);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::with_limit(Self::DEFAULT_LIMIT)
    }

    /// Creates an expander that allows invocations nested at most `limit`
    /// expansions deep.
    ///
    /// Nesting is measured through hygiene: an invocation whose name token was
    /// produced by an expansion `n` levels deep runs at level `n + 1`. A macro
    /// that expands into a call of itself without a base case therefore stops
    /// with [`ExpandError::RecursionLimit`] instead of looping forever. A limit
    /// of `0` rejects every expansion.
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
        Self {
            hygiene: Hygiene::default(),
            matcher: Matcher::default(),
            scratch: Scratch::default(),
            limit,
        }
    }

    /// Returns the recursion limit.
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
        self.limit
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
    pub fn expand(
        &mut self,
        mac: &Macro<K>,
        input: &[Tree<K>],
        call_site: Span,
        call_context: Context,
    ) -> Result<Vec<Tree<K>>, ExpandError> {
        let depth = self.hygiene.depth(call_context).saturating_add(1);
        if depth > self.limit {
            return Err(ExpandError::RecursionLimit { limit: self.limit });
        }

        self.matcher.load(input);
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
                        },
                        &mut self.scratch,
                    );
                    if result.is_err() {
                        self.hygiene.rollback(checkpoint);
                    }
                    return result;
                }
                Outcome::Ambiguous => return Err(ExpandError::Ambiguous { rule: index }),
                Outcome::NoMatch { furthest: reached } => furthest = furthest.max(reached),
            }
        }
        Err(ExpandError::NoMatch {
            span: self.matcher.span_at(furthest, call_site),
        })
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
            .field("limit", &self.limit)
            .field("contexts", &contexts)
            .field("expansions", &expansions)
            .finish()
    }
}
