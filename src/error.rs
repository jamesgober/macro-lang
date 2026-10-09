//! The two ways macro work fails: a bad definition, or a bad invocation.

use core::fmt;

use intern_lang::Symbol;
use token_lang::Span;

use crate::Limit;

/// Why [`Macro::new`](crate::Macro::new) rejected a macro definition.
///
/// Every check that can be made without an invocation is made when the macro is
/// defined, so a macro that compiles can only fail to expand for reasons that
/// depend on the input. Each variant names the offending rule by its index in
/// the list passed to `Macro::new`; metavariable names are reported as the
/// [`Symbol`] the definition used, which the caller resolves through its own
/// interner.
///
/// The enum is `#[non_exhaustive]`: later releases may add checks.
///
/// # Examples
///
/// ```
/// use macro_lang::{Macro, MacroError, Rule, Template};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let x = names.intern("x");
///
/// // The template uses `$x`, but the pattern never binds it.
/// let err = Macro::<char>::new(names.intern("m"), vec![Rule {
///     pattern: vec![],
///     template: vec![Template::Var(x)],
/// }]).unwrap_err();
/// assert_eq!(err, MacroError::UnboundVariable { rule: 0, name: x });
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MacroError {
    /// The macro has no rules, so no invocation could ever match it. Give it at
    /// least one rule.
    NoRules,
    /// A pattern binds the same metavariable name twice, so a template use of it
    /// would be ambiguous. Rename one of the bindings.
    DuplicateBinding {
        /// Index of the offending rule.
        rule: usize,
        /// The name bound twice.
        name: Symbol,
    },
    /// A template uses a metavariable its rule's pattern never binds. Bind it in
    /// the pattern, or write the token literally.
    UnboundVariable {
        /// Index of the offending rule.
        rule: usize,
        /// The unbound name.
        name: Symbol,
    },
    /// A template uses a repeating metavariable outside enough repetitions to
    /// expand it: the pattern captured it inside `n` repetitions, but the
    /// template uses it inside fewer. Wrap the use in a `Template::Repeat`.
    StillRepeating {
        /// Index of the offending rule.
        rule: usize,
        /// The metavariable used too shallowly.
        name: Symbol,
    },
    /// A template repetition contains no metavariable that repeats at its
    /// depth, so there is no way to know how many times to write it. Use a
    /// repeating metavariable inside it, or remove the repetition.
    NoRepeatingVariable {
        /// Index of the offending rule.
        rule: usize,
    },
    /// A pattern repetition can match without consuming anything, so it could
    /// repeat forever. Give its body at least one token, group, or binding that
    /// must be present.
    EmptyRepetition {
        /// Index of the offending rule.
        rule: usize,
    },
    /// A pattern repetition uses [`Kleene::ZeroOrOne`](crate::Kleene::ZeroOrOne)
    /// together with a separator. An optional element never repeats, so the
    /// separator could never appear. Remove the separator.
    OptionalSeparator {
        /// Index of the offending rule.
        rule: usize,
    },
}

impl fmt::Display for MacroError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRules => f.write_str("macro has no rules"),
            Self::DuplicateBinding { rule, name } => {
                write!(
                    f,
                    "rule {rule}: metavariable {name:?} is bound more than once"
                )
            }
            Self::UnboundVariable { rule, name } => {
                write!(
                    f,
                    "rule {rule}: metavariable {name:?} is not bound by the pattern"
                )
            }
            Self::StillRepeating { rule, name } => write!(
                f,
                "rule {rule}: metavariable {name:?} is still repeating at this depth"
            ),
            Self::NoRepeatingVariable { rule } => write!(
                f,
                "rule {rule}: template repetition contains no metavariable that repeats at its depth"
            ),
            Self::EmptyRepetition { rule } => write!(
                f,
                "rule {rule}: pattern repetition can match without consuming input"
            ),
            Self::OptionalSeparator { rule } => write!(
                f,
                "rule {rule}: an at-most-once repetition cannot have a separator"
            ),
        }
    }
}

impl core::error::Error for MacroError {}

/// Why [`Expander::expand`](crate::Expander::expand) could not expand an
/// invocation.
///
/// A failed expansion leaves the expander exactly as it was: no contexts are
/// minted and no expansion is recorded. The enum is `#[non_exhaustive]`.
///
/// # Examples
///
/// ```
/// use macro_lang::{Context, ExpandError, Expander, Macro, Pattern, Rule};
/// use intern_lang::Interner;
/// use token_lang::Span;
/// use macro_lang::Tree;
///
/// let mut names = Interner::new();
/// // A macro that accepts exactly `a`.
/// let mac = Macro::new(names.intern("only_a"), vec![Rule {
///     pattern: vec![Pattern::Token('a')],
///     template: vec![],
/// }])?;
///
/// let mut expander = Expander::new();
/// let input = [Tree::token('b', Span::new(9, 10))];
/// let err = expander.expand(&mac, &input, Span::new(0, 11), Context::ROOT).unwrap_err();
/// // The error points at the token no rule could get past.
/// assert_eq!(err, ExpandError::NoMatch { span: Span::new(9, 10) });
/// # Ok::<(), macro_lang::MacroError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExpandError {
    /// No rule matched the invocation.
    ///
    /// `span` locates the furthest point any rule reached before failing: the
    /// token no rule could accept, or an empty span at the end of the input if
    /// every rule wanted more. Report it as "no rules expected this token".
    NoMatch {
        /// Where matching got stuck.
        span: Span,
    },
    /// A rule matched the invocation in more than one way, so the captures are
    /// ambiguous — for example `$($a:tt)* $($b:tt)*` cannot tell where `a`
    /// ends. Rules are tried in order, and an ambiguous rule stops the search
    /// rather than falling through. Make the rule's repetitions unambiguous,
    /// typically with a separator or a literal token between them.
    Ambiguous {
        /// Index of the ambiguous rule.
        rule: usize,
    },
    /// A template repetition uses metavariables that captured different numbers
    /// of repetitions, so they cannot be expanded in lockstep. Match them in the
    /// same pattern repetition, or expand them in separate template
    /// repetitions.
    RepetitionMismatch {
        /// Index of the rule that matched.
        rule: usize,
    },
    /// The invocation is nested deeper than the expander's recursion limit
    /// ([`Budget::max_depth`](crate::Budget::max_depth)) — almost always a
    /// macro that expands to an invocation of itself without a base case. Fix
    /// the macro, or raise the limit with
    /// [`Expander::with_limit`](crate::Expander::with_limit).
    RecursionLimit {
        /// The limit that was exceeded.
        limit: u32,
    },
    /// The expander has used up part of its [`Budget`](crate::Budget): it has
    /// already performed [`max_expansions`](crate::Budget::max_expansions)
    /// expansions, or this one would take the number of tokens written past
    /// [`max_tokens`](crate::Budget::max_tokens). Almost always a macro whose
    /// output grows without bound, such as one that doubles its argument on
    /// every round, or one that expands into several calls of itself. Fix the
    /// macro, or raise the budget with
    /// [`Expander::set_budget`](crate::Expander::set_budget). A long-lived host
    /// should call [`Expander::reset_usage`](crate::Expander::reset_usage) once
    /// per unit of work, or legitimate input eventually exhausts the budget.
    ///
    /// Added in 1.1.0.
    Budget(Limit),
    /// The expander has minted all `u32::MAX` hygiene contexts it can address.
    /// Contexts are never reclaimed, so this takes billions of expansions;
    /// start a fresh [`Expander`](crate::Expander).
    ContextOverflow,
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMatch { span } => write!(f, "no rule matched the invocation (stuck at {span})"),
            Self::Ambiguous { rule } => {
                write!(f, "rule {rule} matches the invocation in more than one way")
            }
            Self::RepetitionMismatch { rule } => write!(
                f,
                "rule {rule}: metavariables repeat a different number of times"
            ),
            Self::RecursionLimit { limit } => {
                write!(f, "macro expansion exceeded the recursion limit of {limit}")
            }
            Self::Budget(limit) => {
                write!(f, "macro expansion exceeded its budget of {limit}")
            }
            Self::ContextOverflow => f.write_str("hygiene context space exhausted"),
        }
    }
}

impl core::error::Error for ExpandError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use alloc::string::ToString;

    use super::*;

    #[test]
    fn test_macro_error_messages_name_the_rule() {
        let name = Symbol::from_u32(3).unwrap();
        let cases = [
            (MacroError::NoRules, "macro has no rules"),
            (
                MacroError::DuplicateBinding { rule: 1, name },
                "rule 1: metavariable Symbol(3) is bound more than once",
            ),
            (
                MacroError::UnboundVariable { rule: 0, name },
                "rule 0: metavariable Symbol(3) is not bound by the pattern",
            ),
            (
                MacroError::StillRepeating { rule: 2, name },
                "rule 2: metavariable Symbol(3) is still repeating at this depth",
            ),
            (
                MacroError::EmptyRepetition { rule: 4 },
                "rule 4: pattern repetition can match without consuming input",
            ),
        ];
        for (err, text) in cases {
            assert_eq!(err.to_string(), text);
        }
        assert!(
            MacroError::NoRepeatingVariable { rule: 0 }
                .to_string()
                .contains("no metavariable")
        );
        assert!(
            MacroError::OptionalSeparator { rule: 0 }
                .to_string()
                .contains("separator")
        );
    }

    #[test]
    fn test_expand_error_messages() {
        assert_eq!(
            ExpandError::NoMatch {
                span: Span::new(2, 5)
            }
            .to_string(),
            "no rule matched the invocation (stuck at 2..5)"
        );
        assert!(
            ExpandError::Ambiguous { rule: 1 }
                .to_string()
                .contains("rule 1")
        );
        assert!(
            ExpandError::RepetitionMismatch { rule: 0 }
                .to_string()
                .contains("repeat")
        );
        assert!(
            ExpandError::RecursionLimit { limit: 8 }
                .to_string()
                .contains('8')
        );
        assert!(
            ExpandError::ContextOverflow
                .to_string()
                .contains("exhausted")
        );
        assert_eq!(
            ExpandError::Budget(Limit::Tokens { max: 64 }).to_string(),
            "macro expansion exceeded its budget of 64 output tokens"
        );
        assert_eq!(
            ExpandError::Budget(Limit::Expansions { max: 3 }).to_string(),
            "macro expansion exceeded its budget of 3 expansions"
        );
    }

    #[test]
    fn test_errors_are_std_errors() {
        fn assert_error<E: core::error::Error>() {}
        assert_error::<MacroError>();
        assert_error::<ExpandError>();
    }
}
