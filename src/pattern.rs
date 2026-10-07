//! The match side of a rule: what an invocation must look like.

use alloc::vec::Vec;

use intern_lang::Symbol;

/// One element of a rule's pattern — the left-hand side of a `macro_rules!` arm.
///
/// A pattern is a sequence of elements matched against an invocation's token
/// trees from left to right. Literal tokens and groups must appear exactly;
/// a [`Bind`](Pattern::Bind) captures a tree under a name the template can
/// substitute; a [`Repeat`](Pattern::Repeat) matches its body any number of
/// times. The whole pattern must consume the whole invocation.
///
/// Patterns are plain data: a language's front end parses its own macro
/// syntax (say `$x:ident` or `$($e:expr),*`) and lowers it into these
/// elements. [`Macro::new`](crate::Macro::new) validates and compiles them.
///
/// Literal tokens compare by kind alone. Spans never matter to a match, and
/// neither do hygiene contexts: a literal `else` in a pattern matches an `else`
/// token whichever expansion introduced it.
///
/// # Examples
///
/// The pattern `$name:tt = ( $($arg:tt),* )` with `char` tokens:
///
/// ```
/// use macro_lang::{Fragment, Kleene, Pattern};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let pattern: Vec<Pattern<char>> = vec![
///     Pattern::Bind { name: names.intern("name"), fragment: Fragment::Tree },
///     Pattern::Token('='),
///     Pattern::Group {
///         open: '(',
///         close: ')',
///         body: vec![Pattern::Repeat {
///             body: vec![Pattern::Bind { name: names.intern("arg"), fragment: Fragment::Tree }],
///             separator: Some(','),
///             kleene: Kleene::ZeroOrMore,
///         }],
///     },
/// ];
/// assert_eq!(pattern.len(), 3);
/// ```
#[derive(Clone, Debug)]
pub enum Pattern<K> {
    /// Matches a single token whose kind equals this one.
    Token(K),
    /// Matches a delimited group with these delimiter kinds whose contents
    /// match `body` exactly.
    Group {
        /// The kind of the opening delimiter.
        open: K,
        /// The kind of the closing delimiter.
        close: K,
        /// The pattern the group's contents must match, in full.
        body: Vec<Pattern<K>>,
    },
    /// Captures one tree under `name` — a metavariable, `$name:fragment`.
    Bind {
        /// The metavariable name the template refers to. Unique within a rule.
        name: Symbol,
        /// What the captured tree may be.
        fragment: Fragment<K>,
    },
    /// Matches `body` repeatedly — `$( ... ) sep kleene`.
    ///
    /// Every metavariable bound inside the body captures one tree per
    /// repetition, and a template must expand it inside a matching
    /// [`Template::Repeat`](crate::Template::Repeat).
    Repeat {
        /// The pattern matched on each repetition. It must consume at least one
        /// tree; a body that can match nothing is rejected, since it could
        /// repeat forever.
        body: Vec<Pattern<K>>,
        /// A token kind required between repetitions, such as `,`. Not allowed
        /// with [`Kleene::ZeroOrOne`], which never repeats.
        separator: Option<K>,
        /// How many repetitions are allowed.
        kleene: Kleene,
    },
}

/// What a [`Pattern::Bind`] may capture: the fragment specifier.
///
/// macro-lang knows no grammar, so it cannot offer `expr` or `ty` fragments;
/// those need a parser. What it can offer is the structural fragment every
/// macro system has — any single token tree — and a token-level test the
/// language supplies, which covers specifiers like `ident` and `literal`.
///
/// # Examples
///
/// ```
/// use macro_lang::Fragment;
///
/// // Any single tree: a token or a whole delimited group (`$x:tt`).
/// let any: Fragment<char> = Fragment::Tree;
///
/// // A single token whose kind passes a test (`$x:ident`).
/// let ident: Fragment<char> = Fragment::Kind(|c: &char| c.is_alphabetic());
///
/// assert!(matches!(any, Fragment::Tree));
/// assert!(matches!(ident, Fragment::Kind(test) if test(&'x') && !test(&'1')));
/// ```
#[derive(Clone, Copy, Debug)]
pub enum Fragment<K> {
    /// Any single token tree: one token, or one whole delimited group. This is
    /// the `tt` specifier.
    Tree,
    /// A single token (never a group) whose kind satisfies the test — for
    /// example `Fragment::Kind(Kind::is_ident)` for an `ident` specifier.
    Kind(fn(&K) -> bool),
}

/// How many times a [`Pattern::Repeat`] may match: the Kleene operator.
///
/// # Examples
///
/// ```
/// use macro_lang::Kleene;
///
/// assert!(Kleene::ZeroOrMore.allows(0));
/// assert!(!Kleene::OneOrMore.allows(0));
/// assert!(!Kleene::ZeroOrOne.allows(2));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kleene {
    /// Any number of repetitions, including none — `*`.
    ZeroOrMore,
    /// At least one repetition — `+`.
    OneOrMore,
    /// At most one repetition — `?`.
    ZeroOrOne,
}

impl Kleene {
    /// Returns `true` if a repetition may match exactly `count` times.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Kleene;
    ///
    /// assert!(Kleene::OneOrMore.allows(3));
    /// assert!(Kleene::ZeroOrOne.allows(0));
    /// assert!(Kleene::ZeroOrOne.allows(1));
    /// ```
    #[inline]
    #[must_use]
    pub const fn allows(self, count: usize) -> bool {
        match self {
            Self::ZeroOrMore => true,
            Self::OneOrMore => count >= 1,
            Self::ZeroOrOne => count <= 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kleene_bounds() {
        for count in 0..4 {
            assert!(Kleene::ZeroOrMore.allows(count));
            assert_eq!(Kleene::OneOrMore.allows(count), count >= 1);
            assert_eq!(Kleene::ZeroOrOne.allows(count), count <= 1);
        }
    }

    #[test]
    fn test_fragment_kind_calls_the_test() {
        let digit: Fragment<char> = Fragment::Kind(|c: &char| c.is_ascii_digit());
        assert!(matches!(digit, Fragment::Kind(test) if test(&'7') && !test(&'x')));
    }
}
