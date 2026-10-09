//! Resource budgets: how much work one expander may do before it refuses.
//!
//! Macro expansion is driven by input the compiler does not control, and a
//! two-line macro can describe unbounded work. The recursion limit alone does
//! not stop that: a macro that doubles its argument (`dup!($($t)*) =>
//! dup!($($t)* $($t)*)`) reaches `2^128` tokens long before it reaches depth
//! 128, and a macro that expands into two calls of itself performs `2^128`
//! expansions without ever nesting deeper than 128. A [`Budget`] caps all three
//! quantities, so the total time and memory an expander spends is bounded no
//! matter what the macros say.

use core::fmt;

/// The limits an [`Expander`](crate::Expander) enforces on the work it does.
///
/// | Field | Limits | Default | Exceeding it reports |
/// |---|---|---:|---|
/// | [`max_depth`](Budget::max_depth) | How deeply invocations may nest. | 128 | [`ExpandError::RecursionLimit`](crate::ExpandError::RecursionLimit) |
/// | [`max_expansions`](Budget::max_expansions) | Successful expansions since creation or the last `reset_usage`. | 1 048 576 (`2^20`) | [`ExpandError::Budget`](crate::ExpandError::Budget)`(`[`Limit::Expansions`]`)` |
/// | [`max_tokens`](Budget::max_tokens) | Tokens written since creation or the last `reset_usage`. | 4 194 304 (`2^22`) | [`ExpandError::Budget`](crate::ExpandError::Budget)`(`[`Limit::Tokens`]`)` |
///
/// Every expander has a budget; [`Expander::new`](crate::Expander::new) uses
/// [`Budget::DEFAULT`]. The expansion and token counts are **cumulative**: they
/// accumulate in the expander's [`Usage`] until
/// [`Expander::reset_usage`](crate::Expander::reset_usage) zeroes them. A
/// batch compiler uses one expander per compilation and never resets, so the
/// budget bounds the compilation's total macro output; a long-lived host (a
/// language server, a REPL, a watch-mode compiler) resets once per unit of
/// work — per document parse, per REPL entry, per compilation — so the budget
/// bounds each unit instead of the host's lifetime.
///
/// The defaults are well above what typical programs expand to, and stop a
/// runaway macro quickly: the doubling macro in `examples/budget` stops in
/// about 0.2 s (release build) with a whole-process peak of about 400 MB.
/// Memory grows in proportion to `max_tokens`, so lower it where that is too
/// much; a unit of work that legitimately needs more can raise the budget with
/// [`Expander::set_budget`](crate::Expander::set_budget).
///
/// Tokens are counted as they are written: one per token, two per group (its
/// delimiters), and every token of a substituted capture. An expansion that
/// would push the total past `max_tokens` stops as soon as it does, so even a
/// single enormous expansion allocates no more than the budget allows.
///
/// A failed expansion is not charged: like every other error it leaves the
/// expander exactly as it was.
///
/// The struct is `#[non_exhaustive]` so that further limits can be added in a
/// minor release. Start from [`Budget::DEFAULT`] (or [`Budget::default`]) and
/// adjust it with the `with_*` methods, or assign the public fields.
///
/// # Examples
///
/// ```
/// use macro_lang::{Budget, Expander};
///
/// let budget = Budget::DEFAULT.with_max_depth(32).with_max_tokens(1 << 16);
/// let expander: Expander<char> = Expander::with_budget(budget);
/// assert_eq!(expander.budget().max_depth, 32);
/// assert_eq!(expander.budget().max_tokens, 65_536);
/// assert_eq!(expander.budget().max_expansions, Budget::DEFAULT.max_expansions);
/// assert_eq!(expander.limit(), 32);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Budget {
    /// The deepest an invocation may be nested: an invocation in source runs at
    /// depth 1, one produced by that expansion at depth 2, and so on. Exceeding
    /// it reports [`ExpandError::RecursionLimit`](crate::ExpandError::RecursionLimit).
    /// `0` rejects every expansion. This is the recursion limit that
    /// [`Expander::with_limit`](crate::Expander::with_limit) sets.
    pub max_depth: u32,
    /// The number of successful expansions the expander may perform before
    /// its [`Usage`] is [reset](crate::Expander::reset_usage). `0` rejects
    /// every expansion.
    pub max_expansions: usize,
    /// The number of tokens the expander may write before its [`Usage`] is
    /// [reset](crate::Expander::reset_usage), counting group delimiters and
    /// substituted captures.
    pub max_tokens: usize,
}

impl Budget {
    /// The budget [`Expander::new`](crate::Expander::new) uses: depth 128 (the
    /// same default as rustc's `recursion_limit`), `2^20` expansions, and
    /// `2^22` tokens.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Budget;
    ///
    /// assert_eq!(Budget::DEFAULT.max_depth, 128);
    /// assert_eq!(Budget::DEFAULT.max_expansions, 1 << 20);
    /// assert_eq!(Budget::DEFAULT.max_tokens, 1 << 22);
    /// assert_eq!(Budget::default(), Budget::DEFAULT);
    /// ```
    pub const DEFAULT: Self = Self {
        max_depth: 128,
        max_expansions: 1 << 20,
        max_tokens: 1 << 22,
    };

    /// Returns this budget with [`max_depth`](Budget::max_depth) replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Budget;
    ///
    /// assert_eq!(Budget::DEFAULT.with_max_depth(8).max_depth, 8);
    /// ```
    #[inline]
    #[must_use]
    pub const fn with_max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Returns this budget with [`max_expansions`](Budget::max_expansions)
    /// replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Budget;
    ///
    /// assert_eq!(Budget::DEFAULT.with_max_expansions(10).max_expansions, 10);
    /// ```
    #[inline]
    #[must_use]
    pub const fn with_max_expansions(mut self, max_expansions: usize) -> Self {
        self.max_expansions = max_expansions;
        self
    }

    /// Returns this budget with [`max_tokens`](Budget::max_tokens) replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Budget;
    ///
    /// assert_eq!(Budget::DEFAULT.with_max_tokens(1000).max_tokens, 1000);
    /// ```
    #[inline]
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

impl Default for Budget {
    /// Returns [`Budget::DEFAULT`].
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The work an [`Expander`](crate::Expander) has charged against its
/// [`Budget`]: successful expansions and the tokens they wrote, since the
/// expander was created or
/// [`reset_usage`](crate::Expander::reset_usage) was last called.
///
/// Returned by [`Expander::usage`](crate::Expander::usage). Failed expansions
/// are never charged. The struct is `#[non_exhaustive]` so that further
/// counters can be added in a minor release; read the fields directly.
///
/// Added in 1.1.0.
///
/// # Examples
///
/// ```
/// use macro_lang::{Expander, Usage};
///
/// let expander: Expander<char> = Expander::new();
/// let usage = expander.usage();
/// assert_eq!((usage.expansions, usage.tokens), (0, 0));
/// assert_eq!(usage, Usage::NONE);
/// assert_eq!(Usage::default(), Usage::NONE);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Usage {
    /// Successful expansions, compared against
    /// [`Budget::max_expansions`].
    pub expansions: usize,
    /// Tokens written by those expansions, counted as for
    /// [`Budget::max_tokens`].
    pub tokens: usize,
}

impl Usage {
    /// No work done: both counters zero. What a new expander, and one just
    /// [reset](crate::Expander::reset_usage), reports.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Usage;
    ///
    /// assert_eq!(Usage::NONE.expansions, 0);
    /// assert_eq!(Usage::NONE.tokens, 0);
    /// ```
    pub const NONE: Self = Self {
        expansions: 0,
        tokens: 0,
    };
}

/// Which part of a [`Budget`] an expansion ran out of, reported by
/// [`ExpandError::Budget`](crate::ExpandError::Budget).
///
/// Depth is not listed: exceeding [`Budget::max_depth`] keeps reporting
/// [`ExpandError::RecursionLimit`](crate::ExpandError::RecursionLimit), as it
/// did before budgets existed. The enum is `#[non_exhaustive]` so that further
/// limits can be added in a minor release.
///
/// # Examples
///
/// ```
/// use macro_lang::{Budget, Context, ExpandError, Expander, Limit, Macro, Rule, Template};
/// use intern_lang::Interner;
/// use token_lang::Span;
///
/// let mut names = Interner::new();
/// // three!() => a b c
/// let three = Macro::new(names.intern("three"), vec![Rule {
///     pattern: vec![],
///     template: "abc".chars().map(|c| Template::token(c, Span::new(0, 1))).collect(),
/// }])?;
///
/// let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(5));
/// assert!(expander.expand(&three, &[], Span::new(0, 1), Context::ROOT).is_ok());
/// // Three more tokens would make six, one more than the budget allows.
/// assert_eq!(
///     expander.expand(&three, &[], Span::new(0, 1), Context::ROOT),
///     Err(ExpandError::Budget(Limit::Tokens { max: 5 })),
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Limit {
    /// [`Budget::max_expansions`] expansions have already been performed.
    Expansions {
        /// The limit that was reached.
        max: usize,
    },
    /// The expansion would write more than [`Budget::max_tokens`] tokens in
    /// total.
    Tokens {
        /// The limit that would have been exceeded.
        max: usize,
    },
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expansions { max } => write!(f, "{max} expansions"),
            Self::Tokens { max } => write!(f, "{max} output tokens"),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    #[test]
    fn test_default_is_the_documented_budget() {
        assert_eq!(Budget::default(), Budget::DEFAULT);
        assert_eq!(Budget::DEFAULT.max_depth, 128);
        assert_eq!(Budget::DEFAULT.max_expansions, 1_048_576);
        assert_eq!(Budget::DEFAULT.max_tokens, 4_194_304);
    }

    #[test]
    fn test_builders_replace_one_field_each() {
        let budget = Budget::DEFAULT
            .with_max_depth(3)
            .with_max_expansions(4)
            .with_max_tokens(5);
        assert_eq!(
            (budget.max_depth, budget.max_expansions, budget.max_tokens),
            (3, 4, 5)
        );
        assert_eq!(
            Budget::DEFAULT.with_max_tokens(9).max_expansions,
            Budget::DEFAULT.max_expansions
        );
    }

    #[test]
    fn test_limit_display_names_the_quantity() {
        assert_eq!(Limit::Expansions { max: 7 }.to_string(), "7 expansions");
        assert_eq!(Limit::Tokens { max: 9 }.to_string(), "9 output tokens");
    }
}
