//! Token trees: the input and output of every expansion.

use alloc::vec::Vec;

use token_lang::{Span, Token};

use crate::Context;

/// A token tree: a single token, or a delimited group of trees.
///
/// Macro expansion works on token trees rather than on a flat token stream or a
/// finished syntax tree. Grouping by delimiters is the one piece of structure
/// every language agrees on before parsing, and it is exactly the structure
/// a pattern needs: a group is matched as a unit, and a fragment can capture a
/// whole bracketed expression without knowing the grammar inside it. Expansion
/// runs before parsing; the parser consumes the expanded trees.
///
/// `K` is the language's token kind — the same type a lexer produces for
/// `token_lang::Token<K>`. The crate only ever compares kinds for equality and
/// clones them, so a small `Copy` kind (an `enum` carrying interned
/// [`Symbol`](intern_lang::Symbol)s) is the fast path.
///
/// Every token carries its hygiene [`Context`]. Trees built from source use
/// [`Context::ROOT`], which is what [`Tree::token`] fills in.
///
/// # Examples
///
/// The token trees for `f(a, b)`, using `char` as the token kind:
///
/// ```
/// use macro_lang::Tree;
/// use token_lang::{Span, Token};
///
/// let call = vec![
///     Tree::token('f', Span::new(0, 1)),
///     Tree::Group {
///         open: Token::new('(', Span::new(1, 2)),
///         close: Token::new(')', Span::new(6, 7)),
///         trees: vec![
///             Tree::token('a', Span::new(2, 3)),
///             Tree::token(',', Span::new(3, 4)),
///             Tree::token('b', Span::new(5, 6)),
///         ],
///     },
/// ];
/// assert_eq!(call[1].span(), Span::new(1, 7));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Tree<K> {
    /// A single token and its hygiene context.
    Token {
        /// The token: its kind and source span.
        token: Token<K>,
        /// The hygiene context the token belongs to.
        ctx: Context,
    },
    /// A delimited group: an opening token, the trees inside, and the closing
    /// token.
    Group {
        /// The opening delimiter, such as `(`.
        open: Token<K>,
        /// The closing delimiter, such as `)`.
        close: Token<K>,
        /// The trees between the delimiters, in source order.
        trees: Vec<Tree<K>>,
    },
}

impl<K> Tree<K> {
    /// Builds a single-token tree in the [root](Context::ROOT) context — a token
    /// written directly in source.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::{Context, Tree};
    /// use token_lang::Span;
    ///
    /// let tree = Tree::token('x', Span::new(4, 5));
    /// assert!(matches!(
    ///     tree,
    ///     Tree::Token { token, ctx } if token.kind == 'x' && ctx == Context::ROOT
    /// ));
    /// ```
    #[inline]
    #[must_use]
    pub const fn token(kind: K, span: Span) -> Self {
        Self::Token {
            token: Token::new(kind, span),
            ctx: Context::ROOT,
        }
    }

    /// Returns the source span the tree covers.
    ///
    /// For a token this is the token's span; for a group it runs from the start
    /// of the opening delimiter to the end of the closing one.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Tree;
    /// use token_lang::{Span, Token};
    ///
    /// let group: Tree<char> = Tree::Group {
    ///     open: Token::new('[', Span::new(10, 11)),
    ///     close: Token::new(']', Span::new(14, 15)),
    ///     trees: vec![],
    /// };
    /// assert_eq!(group.span(), Span::new(10, 15));
    /// assert_eq!(Tree::token('x', Span::new(2, 3)).span(), Span::new(2, 3));
    /// ```
    #[inline]
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Token { token, .. } => token.span,
            Self::Group { open, close, .. } => open.span.merge(close.span),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn test_token_uses_root_context() {
        let tree = Tree::token(1u8, Span::new(0, 1));
        assert_eq!(
            tree,
            Tree::Token {
                token: Token::new(1, Span::new(0, 1)),
                ctx: Context::ROOT
            }
        );
    }

    #[test]
    fn test_group_span_covers_both_delimiters() {
        let tree = Tree::Group {
            open: Token::new(0u8, Span::new(3, 4)),
            close: Token::new(1u8, Span::new(9, 10)),
            trees: vec![Tree::token(2u8, Span::new(5, 6))],
        };
        assert_eq!(tree.span(), Span::new(3, 10));
    }
}
