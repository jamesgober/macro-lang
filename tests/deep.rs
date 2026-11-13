//! Deeply nested token trees: expansion, cloning, comparison and hashing work
//! at any depth, and `Tree::dismantle` drops a tree of any depth.
//!
//! Every test here runs on the default test-thread stack. Before these trees
//! were walked with heap stacks, each of the million-level cases overflowed it.

#![allow(clippy::unwrap_used)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use intern_lang::Interner;
use macro_lang::{
    Budget, Context, ExpandError, Expander, Fragment, Limit, Macro, Pattern, Rule, Template, Tree,
};
use token_lang::{Span, Token};

const DEEP: usize = 1_000_000;

fn sp(at: u32) -> Span {
    Span::new(at, at + 1)
}

/// `depth` nested groups around one token.
fn nested(depth: usize, leaf: char) -> Tree<char> {
    let mut tree = Tree::token(leaf, sp(0));
    for _ in 0..depth {
        tree = Tree::Group {
            open: Token::new('(', sp(0)),
            close: Token::new(')', sp(1)),
            trees: vec![tree],
        };
    }
    tree
}

/// A macro taking one tree and writing it `copies` times.
fn repeat_arg(names: &mut Interner, copies: usize) -> Macro<char> {
    let x = names.intern("x");
    Macro::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![Pattern::Bind {
                name: x,
                fragment: Fragment::Tree,
            }],
            template: vec![Template::Var(x); copies],
        }],
    )
    .unwrap()
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

#[test]
fn deep_identity_expansion_clone_compare_hash_and_dismantle() {
    let mut names = Interner::new();
    let id = repeat_arg(&mut names, 1);
    let input = vec![nested(DEEP, 'x')];

    let out = Expander::new()
        .expand(&id, &input, sp(9), Context::ROOT)
        .unwrap();
    assert_eq!(out, input);

    let copy = out.clone();
    assert_eq!(copy, out);
    assert_eq!(hash_of(&copy), hash_of(&input));

    let different = vec![nested(DEEP, 'y')];
    assert_ne!(different, input);
    let shallower = vec![nested(DEEP - 1, 'x')];
    assert_ne!(shallower, input);

    Tree::dismantle(input);
    Tree::dismantle(out);
    Tree::dismantle(copy);
    Tree::dismantle(different);
    Tree::dismantle(shallower);
}

#[test]
fn failed_expansion_of_a_deep_capture_does_not_overflow() {
    // Writing the capture twice needs about 4M tokens; the budget allows 3M,
    // so the second copy is refused after the first is already in the output.
    let mut names = Interner::new();
    let twice = repeat_arg(&mut names, 2);
    let input = vec![nested(DEEP, 'x')];
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(3_000_000));

    let err = expander
        .expand(&twice, &input, sp(9), Context::ROOT)
        .unwrap_err();
    assert_eq!(err, ExpandError::Budget(Limit::Tokens { max: 3_000_000 }));
    Tree::dismantle(input);
}

#[test]
fn dismantle_handles_wide_and_mixed_trees() {
    let mut wide = Vec::new();
    for i in 0..10_000 {
        wide.push(nested(i % 50, 'a'));
    }
    wide.push(nested(200_000, 'b'));
    Tree::dismantle(wide);
    Tree::dismantle(std::iter::empty::<Tree<char>>());
    Tree::dismantle([Tree::token('z', sp(3))]);
}

/// The same shape as `Tree`, with the standard derives, to check that the
/// hand-written impls behave exactly like derived ones.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Mirror {
    Token {
        token: Token<char>,
        ctx: Context,
    },
    Group {
        open: Token<char>,
        close: Token<char>,
        trees: Vec<Mirror>,
    },
}

fn mirror(tree: &Tree<char>) -> Mirror {
    match tree {
        Tree::Token { token, ctx } => Mirror::Token {
            token: *token,
            ctx: *ctx,
        },
        Tree::Group { open, close, trees } => Mirror::Group {
            open: *open,
            close: *close,
            trees: trees.iter().map(mirror).collect(),
        },
    }
}

fn samples() -> Vec<Tree<char>> {
    let g = |o: char, c: char, at: u32, trees: Vec<Tree<char>>| Tree::Group {
        open: Token::new(o, sp(at)),
        close: Token::new(c, sp(at + 10)),
        trees,
    };
    vec![
        Tree::token('a', sp(0)),
        Tree::token('b', sp(0)),
        Tree::token('a', sp(1)),
        g('(', ')', 0, vec![]),
        g('[', ']', 0, vec![]),
        g('(', ')', 0, vec![Tree::token('a', sp(1))]),
        g(
            '(',
            ')',
            0,
            vec![Tree::token('a', sp(1)), Tree::token('b', sp(2))],
        ),
        g(
            '(',
            ')',
            0,
            vec![g('(', ')', 1, vec![Tree::token('a', sp(2))])],
        ),
        g(
            '(',
            ')',
            0,
            vec![g('(', ')', 1, vec![]), Tree::token('a', sp(2))],
        ),
        g(
            '(',
            ')',
            0,
            vec![Tree::token('a', sp(2)), g('(', ')', 1, vec![])],
        ),
        g(
            '{',
            '}',
            0,
            vec![
                g(
                    '(',
                    ')',
                    1,
                    vec![Tree::token('a', sp(2)), Tree::token('b', sp(3))],
                ),
                Tree::token(',', sp(4)),
                g(
                    '[',
                    ']',
                    5,
                    vec![g('(', ')', 6, vec![Tree::token('c', sp(7))])],
                ),
            ],
        ),
    ]
}

#[test]
fn hand_written_impls_match_derived_behavior() {
    let trees = samples();
    for a in &trees {
        let ma = mirror(a);
        assert_eq!(mirror(&a.clone()), ma);
        assert_eq!(hash_of(a), hash_of(&ma), "{a:?}");
        for b in &trees {
            assert_eq!(a == b, ma == mirror(b), "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn fields_can_still_be_moved_out_by_value() {
    // `Tree` must stay without a `Drop` impl in 1.x: callers move fields out.
    let tree = nested(3, 'x');
    let Tree::Group { trees, open, close } = tree else {
        panic!("expected a group");
    };
    assert_eq!((open.kind, close.kind), ('(', ')'));
    assert_eq!(trees.len(), 1);
}
