//! Token trees: the input and output of every expansion.

use alloc::vec::Vec;
use core::hash::{Hash, Hasher};
use core::slice;

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
///
/// # Deeply nested trees
///
/// [`Clone`], [`PartialEq`] and [`Hash`] walk a tree with a heap stack, so
/// they work at any nesting depth, and so does everything the
/// [`Expander`](crate::Expander) does with the trees it is given and returns.
///
/// Dropping a tree and formatting it with [`Debug`] still recurse once per
/// level of nesting, as for any recursive Rust type: `Tree` has public fields
/// that callers move out of by value, so it cannot have a custom `Drop`
/// without breaking them. A tree nested hundreds of thousands of groups deep
/// (a few megabytes of `((((...))))` in hostile source) can overflow the stack
/// when it is dropped. A front end that builds trees from untrusted text should
/// either cap group nesting while it builds them, or hand finished trees to
/// [`Tree::dismantle`] instead of dropping them.
#[derive(Debug, Eq)]
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

    /// Drops `trees` without recursion, using a heap stack instead of the call
    /// stack.
    ///
    /// An ordinary drop of a [`Tree`] recurses once per level of group nesting,
    /// so a tree nested hundreds of thousands of levels deep can overflow the
    /// stack. `dismantle` takes the tree apart one group at a time, so it is
    /// safe at any depth. It accepts anything that yields trees: one tree
    /// (`[tree]`), the `Vec` an expansion returns, or an iterator.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Tree;
    /// use token_lang::{Span, Token};
    ///
    /// // A million nested groups: dropping this normally would overflow.
    /// let mut tree = Tree::token('x', Span::new(0, 1));
    /// for _ in 0..1_000_000 {
    ///     tree = Tree::Group {
    ///         open: Token::new('(', Span::new(0, 1)),
    ///         close: Token::new(')', Span::new(0, 1)),
    ///         trees: vec![tree],
    ///     };
    /// }
    /// Tree::dismantle([tree]);
    /// ```
    pub fn dismantle(trees: impl IntoIterator<Item = Self>) {
        let mut stack: Vec<Self> = trees.into_iter().collect();
        while let Some(tree) = stack.pop() {
            // Move the children out before the group itself is dropped, so
            // every drop that happens here is of a tree with no children.
            if let Self::Group { trees, .. } = tree {
                stack.extend(trees);
            }
        }
    }
}

/// One group being copied by [`Tree::clone`]: its delimiters, the originals
/// still to copy, and the copies made so far.
struct CloneFrame<'a, K> {
    open: Token<K>,
    close: Token<K>,
    rest: slice::Iter<'a, Tree<K>>,
    trees: Vec<Tree<K>>,
}

impl<'a, K: Clone> CloneFrame<'a, K> {
    fn new(open: &Token<K>, close: &Token<K>, trees: &'a [Tree<K>]) -> Self {
        Self {
            open: open.clone(),
            close: close.clone(),
            rest: trees.iter(),
            trees: Vec::with_capacity(trees.len()),
        }
    }
}

impl<K: Clone> Clone for Tree<K> {
    /// Copies the tree with a heap stack, so any nesting depth is safe.
    fn clone(&self) -> Self {
        let (open, close, trees) = match self {
            Self::Token { token, ctx } => {
                return Self::Token {
                    token: token.clone(),
                    ctx: *ctx,
                };
            }
            Self::Group { open, close, trees } => (open, close, trees),
        };
        let mut stack = Vec::new();
        let mut frame = CloneFrame::new(open, close, trees);
        loop {
            match frame.rest.next() {
                Some(Self::Token { token, ctx }) => frame.trees.push(Self::Token {
                    token: token.clone(),
                    ctx: *ctx,
                }),
                Some(Self::Group { open, close, trees }) => {
                    let child = CloneFrame::new(open, close, trees);
                    stack.push(core::mem::replace(&mut frame, child));
                }
                None => {
                    let done = Self::Group {
                        open: frame.open,
                        close: frame.close,
                        trees: frame.trees,
                    };
                    match stack.pop() {
                        Some(parent) => {
                            frame = parent;
                            frame.trees.push(done);
                        }
                        None => return done,
                    }
                }
            }
        }
    }
}

impl<K: PartialEq> PartialEq for Tree<K> {
    /// Compares the trees with a heap stack, so any nesting depth is safe.
    /// Trees are equal when they have the same shape and every token, context
    /// and delimiter is equal.
    fn eq(&self, other: &Self) -> bool {
        let mut stack: Vec<(slice::Iter<'_, Self>, slice::Iter<'_, Self>)> = Vec::new();
        let mut pair = (self, other);
        loop {
            match pair {
                (
                    Self::Token { token, ctx },
                    Self::Token {
                        token: other_token,
                        ctx: other_ctx,
                    },
                ) => {
                    if token != other_token || ctx != other_ctx {
                        return false;
                    }
                }
                (
                    Self::Group { open, close, trees },
                    Self::Group {
                        open: other_open,
                        close: other_close,
                        trees: other_trees,
                    },
                ) => {
                    if open != other_open
                        || close != other_close
                        || trees.len() != other_trees.len()
                    {
                        return false;
                    }
                    stack.push((trees.iter(), other_trees.iter()));
                }
                _ => return false,
            }
            // The next pair of children to compare, from the innermost group
            // that still has some.
            loop {
                let Some((left, right)) = stack.last_mut() else {
                    return true;
                };
                if let (Some(a), Some(b)) = (left.next(), right.next()) {
                    pair = (a, b);
                    break;
                }
                let _ = stack.pop();
            }
        }
    }
}

impl<K: Hash> Hash for Tree<K> {
    /// Hashes the tree with a heap stack, so any nesting depth is safe. The
    /// values fed to the hasher are the ones a derived `Hash` would feed it,
    /// in the same order.
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut stack: Vec<slice::Iter<'_, Self>> = Vec::new();
        let mut next = Some(self);
        while let Some(tree) = next {
            match tree {
                Self::Token { token, ctx } => {
                    state.write_isize(0);
                    token.hash(state);
                    ctx.hash(state);
                }
                Self::Group { open, close, trees } => {
                    state.write_isize(1);
                    open.hash(state);
                    close.hash(state);
                    state.write_usize(trees.len());
                    stack.push(trees.iter());
                }
            }
            next = None;
            while let Some(children) = stack.last_mut() {
                if let Some(child) = children.next() {
                    next = Some(child);
                    break;
                }
                let _ = stack.pop();
            }
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
