//! The output side of a rule: what an invocation expands to.

use alloc::vec::Vec;

use intern_lang::Symbol;
use token_lang::{Span, Token};

use crate::Context;

/// One element of a rule's template — the right-hand side of a `macro_rules!`
/// arm.
///
/// A template is a sequence of elements written out in order when its rule
/// matches. Literal tokens and groups are copied into the output; a
/// [`Var`](Template::Var) is replaced by whatever its metavariable captured; a
/// [`Repeat`](Template::Repeat) writes its body once per captured repetition.
///
/// Literal tokens are where hygiene happens. Every literal token written by an
/// expansion receives a fresh [`Context`] minted for that expansion, so a name
/// the template introduces — a temporary, a loop label — can never be confused
/// with the same name at the call site. Substituted captures keep the context
/// they arrived with; they belong to the caller.
///
/// The enum is `#[non_exhaustive]` so that new kinds of template element can be
/// added in a minor release. Constructing any variant is unaffected; code that
/// matches on a `Template` needs a wildcard arm.
///
/// # Examples
///
/// The template `( $($arg)+* )` — the captured arguments joined by `+`:
///
/// ```
/// use macro_lang::Template;
/// use intern_lang::Interner;
/// use token_lang::{Span, Token};
///
/// let mut names = Interner::new();
/// let template: Vec<Template<char>> = vec![Template::Group {
///     open: Token::new('(', Span::new(30, 31)),
///     close: Token::new(')', Span::new(40, 41)),
///     body: vec![Template::Repeat {
///         body: vec![Template::Var(names.intern("arg"))],
///         separator: Some(Token::new('+', Span::new(36, 37))),
///     }],
/// }];
/// assert_eq!(template.len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Template<K> {
    /// A literal token, written out with a context minted for the expansion.
    Token {
        /// The token to write: its kind and its span in the macro definition.
        token: Token<K>,
        /// The context the token has in the macro definition. For a macro
        /// written directly in source this is [`Context::ROOT`]; for a macro
        /// defined by another macro's expansion it is the context that expansion
        /// gave the token.
        ctx: Context,
    },
    /// A literal group, written out with its delimiters and its expanded body.
    Group {
        /// The opening delimiter.
        open: Token<K>,
        /// The closing delimiter.
        close: Token<K>,
        /// The template expanded between the delimiters.
        body: Vec<Template<K>>,
    },
    /// The tree a metavariable captured, substituted unchanged — `$name`.
    ///
    /// The name must be bound by the rule's pattern. A metavariable captured
    /// inside `n` pattern repetitions must be used inside at least `n` template
    /// repetitions; one used inside more is repeated as a whole.
    Var(Symbol),
    /// The body written once per captured repetition — `$( ... ) sep *`.
    ///
    /// The number of repetitions comes from the metavariables used in the body:
    /// every metavariable repeating at this depth must have captured the same
    /// number of trees, and at least one must be present.
    Repeat {
        /// The template written on each repetition.
        body: Vec<Template<K>>,
        /// A token written between repetitions, such as `,`. Hygiene treats it
        /// as a token defined in the [root](Context::ROOT) context, which is
        /// exact for a macro written in source. A macro whose definition was
        /// itself produced by an expansion should use
        /// [`RepeatSeparated`](Template::RepeatSeparated), which carries the
        /// separator's definition context.
        separator: Option<Token<K>>,
    },
    /// The body written once per captured repetition with `separator` written
    /// between repetitions — `$( ... ) sep *` — where the separator carries its
    /// definition context like a [`Token`](Template::Token) does.
    ///
    /// The separator is a literal token of the template, so it is marked
    /// exactly like one: it receives the context this expansion mints for
    /// `ctx`, the same context every literal token defined in `ctx` receives,
    /// and [`Origin::parent`](crate::Origin::parent) reports `ctx`. A front end
    /// lowering a macro definition that an earlier expansion produced passes
    /// the separator token's own context here, so the separator binds and
    /// resolves like the tokens around it. For `ctx` equal to
    /// [`Context::ROOT`] this behaves exactly like
    /// [`Repeat`](Template::Repeat) with `Some(separator)`.
    ///
    /// Repetition counts follow the same rules as [`Repeat`](Template::Repeat).
    ///
    /// Added in 1.1.0.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Context, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
    /// use intern_lang::Interner;
    /// use token_lang::{Span, Token};
    ///
    /// let mut names = Interner::new();
    /// let x = names.intern("x");
    /// // A context some earlier expansion gave this macro's definition.
    /// let mut expander = Expander::new();
    /// let def = names.intern("def");
    /// let maker = Macro::new(def, vec![Rule {
    ///     pattern: vec![],
    ///     template: vec![Template::token('k', Span::new(0, 1))],
    /// }])?;
    /// let Some(Tree::Token { ctx: def_ctx, .. }) =
    ///     expander.expand(&maker, &[], Span::new(0, 1), Context::ROOT)?.first().cloned()
    /// else {
    ///     return Err("expected a token".into());
    /// };
    ///
    /// // join!($($x:tt)*) => $($x);*  — with `;` defined in `def_ctx`
    /// let join = Macro::new(names.intern("join"), vec![Rule {
    ///     pattern: vec![Pattern::Repeat {
    ///         body: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
    ///         separator: None,
    ///         kleene: Kleene::ZeroOrMore,
    ///     }],
    ///     template: vec![Template::RepeatSeparated {
    ///         body: vec![Template::Var(x)],
    ///         separator: Token::new(';', Span::new(9, 10)),
    ///         ctx: def_ctx,
    ///     }],
    /// }])?;
    ///
    /// let input = [Tree::token('a', Span::new(0, 1)), Tree::token('b', Span::new(1, 2))];
    /// let out = expander.expand(&join, &input, Span::new(0, 2), Context::ROOT)?;
    /// let Some(Tree::Token { ctx: sep_ctx, .. }) = out.get(1) else {
    ///     return Err("expected the separator".into());
    /// };
    /// let origin = expander.origin(*sep_ctx).ok_or("separator not marked")?;
    /// assert_eq!(origin.parent, def_ctx);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    RepeatSeparated {
        /// The template written on each repetition.
        body: Vec<Template<K>>,
        /// The token written between repetitions.
        separator: Token<K>,
        /// The context the separator has in the macro definition.
        ctx: Context,
    },
}

impl<K> Template<K> {
    /// Builds a literal token defined in the [root](Context::ROOT) context —
    /// a token written in a macro definition in source.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Context, Template};
    /// use token_lang::{Span, Token};
    ///
    /// assert_eq!(
    ///     Template::token(';', Span::new(8, 9)),
    ///     Template::Token { token: Token::new(';', Span::new(8, 9)), ctx: Context::ROOT },
    /// );
    /// ```
    #[inline]
    #[must_use]
    pub const fn token(kind: K, span: Span) -> Self {
        Self::Token {
            token: Token::new(kind, span),
            ctx: Context::ROOT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_uses_root_context() {
        assert_eq!(
            Template::token(5u8, Span::new(1, 2)),
            Template::Token {
                token: Token::new(5, Span::new(1, 2)),
                ctx: Context::ROOT,
            }
        );
    }
}
