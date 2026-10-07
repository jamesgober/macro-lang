//! Macro definitions: rules, and the validated, compiled macro built from them.

use alloc::vec::Vec;
use core::fmt;

use intern_lang::Symbol;

use crate::compile::{CompiledRule, compile_rule};
use crate::{MacroError, Pattern, Template};

/// One arm of a macro: a pattern to match and the template it expands to.
///
/// A rule is plain data. [`Macro::new`] validates it and compiles it into the
/// form the [`Expander`](crate::Expander) runs.
///
/// # Examples
///
/// `($x:tt) => { $x $x }` — duplicate a single tree:
///
/// ```
/// use macro_lang::{Fragment, Pattern, Rule, Template};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let x = names.intern("x");
/// let rule: Rule<char> = Rule {
///     pattern: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
///     template: vec![Template::Var(x), Template::Var(x)],
/// };
/// assert_eq!(rule.template.len(), 2);
/// ```
#[derive(Clone, Debug)]
pub struct Rule<K> {
    /// What an invocation must look like for this rule to apply. It must match
    /// the invocation's trees in full.
    pub pattern: Vec<Pattern<K>>,
    /// What the invocation expands to when the pattern matches.
    pub template: Vec<Template<K>>,
}

/// A validated, compiled macro: a name and an ordered list of rules.
///
/// Building a `Macro` does all the work that does not depend on an invocation:
/// it checks every rule (see [`MacroError`]) and compiles each pattern into a
/// matching automaton and each template into a flat program. A macro is
/// immutable once built and can be expanded any number of times, by any number
/// of [`Expander`](crate::Expander)s.
///
/// When expanded, the rules are tried in order and the first rule whose pattern
/// matches the invocation is used, exactly like the arms of `macro_rules!`.
///
/// # Examples
///
/// A two-rule macro: `()` expands to `0`, anything else is passed through.
///
/// ```
/// use macro_lang::{Fragment, Kleene, Macro, Pattern, Rule, Template};
/// use intern_lang::Interner;
/// use token_lang::Span;
///
/// let mut names = Interner::new();
/// let t = names.intern("t");
/// let mac: Macro<char> = Macro::new(names.intern("or_zero"), vec![
///     Rule { pattern: vec![], template: vec![Template::token('0', Span::new(0, 1))] },
///     Rule {
///         pattern: vec![Pattern::Repeat {
///             body: vec![Pattern::Bind { name: t, fragment: Fragment::Tree }],
///             separator: None,
///             kleene: Kleene::OneOrMore,
///         }],
///         template: vec![Template::Repeat { body: vec![Template::Var(t)], separator: None }],
///     },
/// ])?;
/// assert_eq!(mac.name(), names.intern("or_zero"));
/// # Ok::<(), macro_lang::MacroError>(())
/// ```
#[derive(Clone)]
pub struct Macro<K> {
    name: Symbol,
    rules: Vec<CompiledRule<K>>,
}

impl<K> Macro<K> {
    /// Validates and compiles a macro from its rules.
    ///
    /// # Parameters
    ///
    /// - `name` — the macro's name. It is recorded in the
    ///   [`Origin`](crate::Origin) of every context the macro's expansions mint,
    ///   so diagnostics can say which macro introduced a token.
    /// - `rules` — the arms, in the order they are tried.
    ///
    /// # Errors
    ///
    /// Returns the first problem found, checking rules in order:
    ///
    /// - [`MacroError::NoRules`] if `rules` is empty.
    /// - [`MacroError::DuplicateBinding`] if a pattern binds a name twice.
    /// - [`MacroError::EmptyRepetition`] if a pattern repetition can match
    ///   nothing.
    /// - [`MacroError::OptionalSeparator`] if a `?` repetition has a separator.
    /// - [`MacroError::UnboundVariable`] if a template uses a name the pattern
    ///   does not bind.
    /// - [`MacroError::StillRepeating`] if a template uses a repeating
    ///   metavariable outside enough repetitions.
    /// - [`MacroError::NoRepeatingVariable`] if a template repetition has nothing
    ///   to repeat over.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Fragment, Kleene, Macro, MacroError, Pattern, Rule, Template};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let x = names.intern("x");
    ///
    /// // `$($x:tt)*` captured `x` inside a repetition, so the template cannot
    /// // use it bare.
    /// let err = Macro::<char>::new(names.intern("m"), vec![Rule {
    ///     pattern: vec![Pattern::Repeat {
    ///         body: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
    ///         separator: None,
    ///         kleene: Kleene::ZeroOrMore,
    ///     }],
    ///     template: vec![Template::Var(x)],
    /// }]).unwrap_err();
    /// assert_eq!(err, MacroError::StillRepeating { rule: 0, name: x });
    ///
    /// assert_eq!(Macro::<char>::new(x, vec![]).unwrap_err(), MacroError::NoRules);
    /// ```
    pub fn new(name: Symbol, rules: Vec<Rule<K>>) -> Result<Self, MacroError> {
        if rules.is_empty() {
            return Err(MacroError::NoRules);
        }
        let rules = rules
            .into_iter()
            .enumerate()
            .map(|(index, rule)| compile_rule(rule, index))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { name, rules })
    }

    /// Returns the macro's name.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Macro, Rule};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let empty = names.intern("empty");
    /// let mac: Macro<char> = Macro::new(empty, vec![Rule { pattern: vec![], template: vec![] }])?;
    /// assert_eq!(names.resolve(mac.name()), Some("empty"));
    /// # Ok::<(), macro_lang::MacroError>(())
    /// ```
    #[inline]
    #[must_use]
    pub const fn name(&self) -> Symbol {
        self.name
    }

    /// The compiled rules, in the order they are tried.
    #[inline]
    pub(crate) fn rules(&self) -> &[CompiledRule<K>] {
        &self.rules
    }
}

impl<K> fmt::Debug for Macro<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Macro")
            .field("name", &self.name)
            .field("rules", &self.rules.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use alloc::format;
    use alloc::vec;

    use token_lang::{Span, Token};

    use super::*;
    use crate::compile::{Inst, Op};
    use crate::{Fragment, Kleene};

    fn sym(n: u32) -> Symbol {
        Symbol::from_u32(n).unwrap()
    }

    fn bind(n: u32) -> Pattern<u8> {
        Pattern::Bind {
            name: sym(n),
            fragment: Fragment::Tree,
        }
    }

    fn repeat(body: Vec<Pattern<u8>>, separator: Option<u8>, kleene: Kleene) -> Pattern<u8> {
        Pattern::Repeat {
            body,
            separator,
            kleene,
        }
    }

    fn one(
        pattern: Vec<Pattern<u8>>,
        template: Vec<Template<u8>>,
    ) -> Result<Macro<u8>, MacroError> {
        Macro::new(sym(100), vec![Rule { pattern, template }])
    }

    #[test]
    fn test_rejects_no_rules() {
        assert_eq!(
            Macro::<u8>::new(sym(1), vec![]).unwrap_err(),
            MacroError::NoRules
        );
    }

    #[test]
    fn test_rejects_duplicate_binding_in_nested_position() {
        let pattern = vec![bind(1), repeat(vec![bind(1)], None, Kleene::ZeroOrMore)];
        assert_eq!(
            one(pattern, vec![]).unwrap_err(),
            MacroError::DuplicateBinding {
                rule: 0,
                name: sym(1)
            }
        );
    }

    #[test]
    fn test_rejects_unbound_variable() {
        assert_eq!(
            one(vec![], vec![Template::Var(sym(9))]).unwrap_err(),
            MacroError::UnboundVariable {
                rule: 0,
                name: sym(9)
            }
        );
    }

    #[test]
    fn test_rejects_empty_repetition_bodies() {
        let empty = repeat(vec![], None, Kleene::ZeroOrMore);
        assert_eq!(
            one(vec![empty], vec![]).unwrap_err(),
            MacroError::EmptyRepetition { rule: 0 }
        );

        // A body made only of optional parts can still match nothing.
        let nullable = repeat(
            vec![repeat(vec![Pattern::Token(1)], None, Kleene::ZeroOrOne)],
            None,
            Kleene::OneOrMore,
        );
        assert_eq!(
            one(vec![nullable], vec![]).unwrap_err(),
            MacroError::EmptyRepetition { rule: 0 }
        );
    }

    #[test]
    fn test_accepts_body_whose_inner_repetition_requires_input() {
        let inner = repeat(vec![Pattern::Token(1)], None, Kleene::OneOrMore);
        let outer = repeat(vec![inner], Some(2), Kleene::ZeroOrMore);
        assert!(one(vec![outer], vec![]).is_ok());
    }

    #[test]
    fn test_accepts_group_with_empty_body_in_repetition() {
        let group = Pattern::Group {
            open: 1,
            close: 2,
            body: vec![],
        };
        assert!(one(vec![repeat(vec![group], None, Kleene::ZeroOrMore)], vec![]).is_ok());
    }

    #[test]
    fn test_rejects_optional_separator() {
        let pattern = vec![repeat(vec![bind(1)], Some(0), Kleene::ZeroOrOne)];
        assert_eq!(
            one(pattern, vec![]).unwrap_err(),
            MacroError::OptionalSeparator { rule: 0 }
        );
    }

    #[test]
    fn test_rejects_shallow_use_of_repeating_variable() {
        let pattern = vec![repeat(vec![bind(1)], None, Kleene::ZeroOrMore)];
        assert_eq!(
            one(pattern, vec![Template::Var(sym(1))]).unwrap_err(),
            MacroError::StillRepeating {
                rule: 0,
                name: sym(1)
            }
        );
    }

    #[test]
    fn test_rejects_repetition_without_repeating_variable() {
        let template = vec![Template::Repeat {
            body: vec![Template::Var(sym(1))],
            separator: None,
        }];
        assert_eq!(
            one(vec![bind(1)], template).unwrap_err(),
            MacroError::NoRepeatingVariable { rule: 0 }
        );
    }

    #[test]
    fn test_reports_the_failing_rule_index() {
        let rules: Vec<Rule<u8>> = vec![
            Rule {
                pattern: vec![],
                template: vec![],
            },
            Rule {
                pattern: vec![],
                template: vec![Template::Var(sym(5))],
            },
        ];
        assert_eq!(
            Macro::new(sym(1), rules).unwrap_err(),
            MacroError::UnboundVariable {
                rule: 1,
                name: sym(5)
            }
        );
    }

    #[test]
    fn test_star_with_separator_program_shape() {
        let mac = one(
            vec![repeat(vec![Pattern::Token(7)], Some(9), Kleene::ZeroOrMore)],
            vec![],
        )
        .unwrap();
        let program = &mac.rules()[0].program;
        // Enter, Split(I, X), I: Iter, body, Split(S, X), S: sep, Jump(I), X: Accept
        assert!(matches!(program[0], Inst::Enter(0)));
        assert!(matches!(program[1], Inst::Split(2, 7)));
        assert!(matches!(program[2], Inst::Iter(0)));
        assert!(matches!(program[3], Inst::Token(7)));
        assert!(matches!(program[4], Inst::Split(5, 7)));
        assert!(matches!(program[5], Inst::Token(9)));
        assert!(matches!(program[6], Inst::Jump(2)));
        assert!(matches!(program[7], Inst::Accept));
    }

    #[test]
    fn test_template_repeat_is_patched_with_end_and_locks() {
        let pattern = vec![repeat(vec![bind(1), bind(2)], None, Kleene::ZeroOrMore)];
        let template = vec![Template::Repeat {
            body: vec![
                Template::Var(sym(1)),
                Template::Var(sym(2)),
                Template::Var(sym(1)),
            ],
            separator: Some(Token::new(3, Span::new(0, 1))),
        }];
        let mac = one(pattern, template).unwrap();
        let rule = &mac.rules()[0];
        assert!(matches!(
            rule.ops[0],
            Op::Repeat {
                end: 4,
                locks: (0, 2),
                ..
            }
        ));
        assert!(matches!(rule.ops[4], Op::End { start: 0 }));
        assert_eq!(rule.locks, vec![0, 1]);
        assert_eq!(rule.chain(0), &[0]);
    }

    #[test]
    fn test_debug_shows_name_and_rule_count() {
        let mac = one(vec![], vec![]).unwrap();
        assert_eq!(format!("{mac:?}"), "Macro { name: Symbol(100), rules: 1 }");
    }
}
