//! A miniature language front end shared by the examples.
//!
//! macro-lang owns no syntax, so a language supplies four things around it:
//!
//! 1. a **lexer** producing `token_lang` tokens — [`lex`];
//! 2. a **tree builder** grouping tokens by delimiters — [`parse_trees`];
//! 3. a **definition front end** lowering its own macro syntax into patterns and
//!    templates — [`define`], which reads `macro_rules!`-style rules such as
//!    `($a:ident, $b:ident) => { $a + $b }`;
//! 4. an **expansion driver** finding invocations and splicing results back —
//!    [`Driver`].
//!
//! Everything here is example code: compact and readable rather than
//! production-grade (lexing errors, for instance, simply panic).

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use intern_lang::{Interner, Symbol};
use macro_lang::{
    Context, ExpandError, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree,
};
use token_lang::{Span, Token};

/// The token kinds of the toy language. Small and `Copy`: identifiers and
/// numbers carry interned symbols, so comparing kinds is an integer compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ident(Symbol),
    Int(Symbol),
    Punct(char),
    Open(char),
    Close(char),
}

impl Kind {
    pub fn is_ident(&self) -> bool {
        matches!(self, Kind::Ident(_))
    }

    pub fn is_literal(&self) -> bool {
        matches!(self, Kind::Int(_))
    }
}

/// Splits `src` into tokens.
pub fn lex(src: &str, names: &mut Interner) -> Vec<Token<Kind>> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        let start = i;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let kind = if c.is_ascii_alphabetic() || c == '_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            Kind::Ident(names.intern(&src[start..i]))
        } else if c.is_ascii_digit() {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            Kind::Int(names.intern(&src[start..i]))
        } else {
            i += 1;
            match c {
                '(' | '[' | '{' => Kind::Open(c),
                ')' | ']' | '}' => Kind::Close(c),
                _ => Kind::Punct(c),
            }
        };
        tokens.push(Token::new(kind, Span::new(start as u32, i as u32)));
    }
    tokens
}

/// Groups a token stream into token trees.
pub fn trees(tokens: Vec<Token<Kind>>) -> Vec<Tree<Kind>> {
    let mut stack: Vec<(Token<Kind>, Vec<Tree<Kind>>)> = Vec::new();
    let mut top: Vec<Tree<Kind>> = Vec::new();
    for token in tokens {
        match token.kind {
            Kind::Open(_) => stack.push((token, std::mem::take(&mut top))),
            Kind::Close(c) => {
                let (open, parent) = stack.pop().expect("unbalanced delimiters");
                assert!(open.kind == Kind::Open(opener(c)), "mismatched delimiters");
                let inner = std::mem::replace(&mut top, parent);
                top.push(Tree::Group {
                    open,
                    close: token,
                    trees: inner,
                });
            }
            _ => top.push(Tree::Token {
                token,
                ctx: Context::ROOT,
            }),
        }
    }
    assert!(stack.is_empty(), "unclosed delimiter");
    top
}

fn opener(close: char) -> char {
    match close {
        ')' => '(',
        ']' => '[',
        _ => '{',
    }
}

/// Lexes and groups `src`.
pub fn parse_trees(src: &str, names: &mut Interner) -> Vec<Tree<Kind>> {
    trees(lex(src, names))
}

/// Keywords of the toy language. They are template tokens like any other and
/// receive contexts too, but a resolver never looks them up, so `render` does
/// not annotate them.
const KEYWORDS: [&str; 2] = ["let", "mut"];

/// One printed token and whether it hugs its neighbours.
struct Piece {
    text: String,
    /// No space before this piece.
    glue_before: bool,
    /// No space after this piece.
    glue_after: bool,
}

/// Prints trees as source. With `hygiene`, every identifier introduced by an
/// expansion is suffixed with its context, e.g. `tmp#1`.
pub fn render(trees: &[Tree<Kind>], names: &Interner, hygiene: bool) -> String {
    let mut pieces = Vec::new();
    pieces_of(trees, names, hygiene, &mut pieces);
    let mut out = String::new();
    let mut glue_next = true;
    for piece in pieces {
        if !glue_next && !piece.glue_before {
            out.push(' ');
        }
        out.push_str(&piece.text);
        glue_next = piece.glue_after;
    }
    out
}

fn pieces_of(trees: &[Tree<Kind>], names: &Interner, hygiene: bool, out: &mut Vec<Piece>) {
    for tree in trees {
        match tree {
            Tree::Token { token, ctx } => {
                let mut text = match token.kind {
                    Kind::Ident(s) | Kind::Int(s) => names.resolve(s).unwrap_or("?").to_string(),
                    Kind::Punct(c) | Kind::Open(c) | Kind::Close(c) => c.to_string(),
                };
                if hygiene
                    && token.kind.is_ident()
                    && !ctx.is_root()
                    && !KEYWORDS.contains(&text.as_str())
                {
                    text.push_str(&ctx.to_string());
                }
                out.push(Piece {
                    text,
                    glue_before: matches!(token.kind, Kind::Punct('.' | ':' | ',' | ';' | '!')),
                    glue_after: matches!(token.kind, Kind::Punct('.' | ':')),
                });
            }
            Tree::Group { open, close, trees } => {
                let (Kind::Open(o), Kind::Close(c)) = (open.kind, close.kind) else {
                    continue;
                };
                // `f(..)`, `m!(..)`, `a[..]`: a paren or bracket group right
                // after a name is a call or an index, so it stays attached.
                let call = o != '{'
                    && out.last().is_some_and(|last| {
                        last.text == "!" || last.text.starts_with(|ch: char| ch.is_alphanumeric())
                    });
                out.push(Piece {
                    text: o.to_string(),
                    glue_before: call,
                    glue_after: o != '{',
                });
                pieces_of(trees, names, hygiene, out);
                out.push(Piece {
                    text: c.to_string(),
                    glue_before: c != '}',
                    glue_after: false,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// definition front end
// ---------------------------------------------------------------------------

/// Builds a macro from `macro_rules!`-style rule text:
///
/// ```text
/// (pattern) => { template } ;
/// (pattern) => { template }
/// ```
///
/// Patterns use `$name:tt`, `$name:ident`, and `$name:literal` bindings and
/// `$( ... ) sep? op` repetitions with `*`, `+`, or `?`. Templates use `$name`
/// and the same repetition syntax.
pub fn define(name: &str, rules: &str, names: &mut Interner) -> Macro<Kind> {
    let trees = parse_trees(rules, names);
    let mut parsed = Vec::new();
    let mut rest = trees.as_slice();
    while !rest.is_empty() {
        let [
            Tree::Group { trees: pattern, .. },
            arrow1,
            arrow2,
            Tree::Group {
                trees: template, ..
            },
            tail @ ..,
        ] = rest
        else {
            panic!("expected `(pattern) => {{ template }}`");
        };
        assert!(
            is_punct(arrow1, '=') && is_punct(arrow2, '>'),
            "expected `=>`"
        );
        parsed.push(Rule {
            pattern: lower_pattern(pattern, names),
            template: lower_template(template),
        });
        rest = match tail {
            [semi, more @ ..] if is_punct(semi, ';') => more,
            other => other,
        };
    }
    let symbol = names.intern(name);
    Macro::new(symbol, parsed).unwrap_or_else(|err| panic!("invalid macro `{name}`: {err}"))
}

fn is_punct(tree: &Tree<Kind>, c: char) -> bool {
    matches!(tree, Tree::Token { token, .. } if token.kind == Kind::Punct(c))
}

fn kind_of(tree: &Tree<Kind>) -> Option<Kind> {
    match tree {
        Tree::Token { token, .. } => Some(token.kind),
        Tree::Group { .. } => None,
    }
}

fn kleene_of(tree: Option<&Tree<Kind>>) -> Option<Kleene> {
    match tree.and_then(kind_of)? {
        Kind::Punct('*') => Some(Kleene::ZeroOrMore),
        Kind::Punct('+') => Some(Kleene::OneOrMore),
        Kind::Punct('?') => Some(Kleene::ZeroOrOne),
        _ => None,
    }
}

/// After `$( ... )`: reads an optional separator (with its context) and the
/// Kleene operator, returning them and how many trees they took.
fn repetition_suffix(rest: &[Tree<Kind>]) -> (Option<(Token<Kind>, Context)>, Kleene, usize) {
    if let Some(kleene) = kleene_of(rest.first()) {
        if kleene_of(rest.get(1)).is_none() {
            return (None, kleene, 1);
        }
    }
    let Some(Tree::Token { token, ctx }) = rest.first() else {
        panic!("expected a separator or `*`, `+`, `?` after `$( ... )`");
    };
    let kleene = kleene_of(rest.get(1)).expect("expected `*`, `+`, or `?`");
    (Some((*token, *ctx)), kleene, 2)
}

fn lower_pattern(trees: &[Tree<Kind>], names: &mut Interner) -> Vec<Pattern<Kind>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < trees.len() {
        let tree = &trees[i];
        i += 1;
        if is_punct(tree, '$') {
            match &trees[i] {
                Tree::Token { token, .. } => {
                    let Kind::Ident(name) = token.kind else {
                        panic!("expected a metavariable name after `$`")
                    };
                    assert!(
                        is_punct(&trees[i + 1], ':'),
                        "expected `:` in `$name:fragment`"
                    );
                    let Some(Kind::Ident(spec)) = kind_of(&trees[i + 2]) else {
                        panic!("expected a fragment specifier")
                    };
                    let fragment = match names.resolve(spec).unwrap_or("") {
                        "tt" => Fragment::Tree,
                        "ident" => Fragment::Kind(Kind::is_ident),
                        "literal" => Fragment::Kind(Kind::is_literal),
                        other => panic!("unsupported fragment `{other}`"),
                    };
                    out.push(Pattern::Bind { name, fragment });
                    i += 3;
                }
                Tree::Group { trees: body, .. } => {
                    let (separator, kleene, used) = repetition_suffix(&trees[i + 1..]);
                    out.push(Pattern::Repeat {
                        body: lower_pattern(body, names),
                        separator: separator.map(|(token, _)| token.kind),
                        kleene,
                    });
                    i += 1 + used;
                }
            }
        } else {
            match tree {
                Tree::Token { token, .. } => out.push(Pattern::Token(token.kind)),
                Tree::Group { open, close, trees } => out.push(Pattern::Group {
                    open: open.kind,
                    close: close.kind,
                    body: lower_pattern(trees, names),
                }),
            }
        }
    }
    out
}

fn lower_template(trees: &[Tree<Kind>]) -> Vec<Template<Kind>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < trees.len() {
        let tree = &trees[i];
        i += 1;
        if is_punct(tree, '$') {
            match &trees[i] {
                Tree::Token { token, .. } => {
                    let Kind::Ident(name) = token.kind else {
                        panic!("expected a metavariable name after `$`")
                    };
                    out.push(Template::Var(name));
                    i += 1;
                }
                Tree::Group { trees: body, .. } => {
                    let (separator, _kleene, used) = repetition_suffix(&trees[i + 1..]);
                    let body = lower_template(body);
                    // The separator keeps the context it has in the definition,
                    // exactly like a literal token: a macro defined by another
                    // macro's expansion marks it consistently with its other
                    // literals.
                    out.push(match separator {
                        Some((separator, ctx)) => Template::RepeatSeparated {
                            body,
                            separator,
                            ctx,
                        },
                        None => Template::Repeat {
                            body,
                            separator: None,
                        },
                    });
                    i += 1 + used;
                }
            }
        } else {
            match tree {
                Tree::Token { token, ctx } => out.push(Template::Token {
                    token: *token,
                    ctx: *ctx,
                }),
                Tree::Group { open, close, trees } => out.push(Template::Group {
                    open: *open,
                    close: *close,
                    body: lower_template(trees),
                }),
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// expansion driver
// ---------------------------------------------------------------------------

/// Finds `name!( ... )` invocations of known macros and expands them until
/// none remain, including invocations produced by expansions.
pub struct Driver {
    pub expander: Expander<Kind>,
    macros: HashMap<Symbol, Macro<Kind>>,
}

impl Driver {
    pub fn new(expander: Expander<Kind>) -> Self {
        Self {
            expander,
            macros: HashMap::new(),
        }
    }

    pub fn add(&mut self, mac: Macro<Kind>) {
        let _previous = self.macros.insert(mac.name(), mac);
    }

    /// Expands every invocation in `trees`, depth first, to a fixed point.
    pub fn expand_all(&mut self, trees: &mut Vec<Tree<Kind>>) -> Result<(), ExpandError> {
        let mut i = 0;
        while i < trees.len() {
            if let Some((name, ctx, span, args)) = self.invocation(&trees[i..]) {
                let mac = &self.macros[&name];
                // The name token's context tells the expander how deep this
                // invocation sits, which is what bounds runaway recursion.
                let out = self.expander.expand(mac, &args, span, ctx)?;
                let _invocation: Vec<_> = trees.splice(i..i + 3, out).collect();
                // Rescan from the same spot: the output may itself invoke macros.
                continue;
            }
            if let Tree::Group { trees: inner, .. } = &mut trees[i] {
                self.expand_all(inner)?;
            }
            i += 1;
        }
        Ok(())
    }

    /// Expands every invocation in `trees`, which sit in code `depth`
    /// expansions deep, expanding each output fully before splicing it.
    ///
    /// Unlike [`Driver::expand_all`], this driver knows where it found every
    /// invocation, so it passes that depth to `Expander::expand_at`. That
    /// bounds even a macro that rebuilds its own invocation entirely from
    /// tokens it captured, which hygiene alone cannot tell apart from the
    /// original call.
    pub fn expand_tracked(
        &mut self,
        trees: Vec<Tree<Kind>>,
        depth: u32,
    ) -> Result<Vec<Tree<Kind>>, ExpandError> {
        let mut out = Vec::with_capacity(trees.len());
        let mut i = 0;
        while i < trees.len() {
            if let Some((name, ctx, span, args)) = self.invocation(&trees[i..]) {
                let mac = &self.macros[&name];
                let expanded = self.expander.expand_at(mac, &args, span, ctx, depth)?;
                out.extend(self.expand_tracked(expanded, depth + 1)?);
                i += 3;
                continue;
            }
            match &trees[i] {
                Tree::Group {
                    open,
                    close,
                    trees: inner,
                } => {
                    let inner = self.expand_tracked(inner.clone(), depth)?;
                    out.push(Tree::Group {
                        open: *open,
                        close: *close,
                        trees: inner,
                    });
                }
                other => out.push(other.clone()),
            }
            i += 1;
        }
        Ok(out)
    }

    /// Recognizes `name ! ( args )` at the start of `trees`.
    fn invocation(&self, trees: &[Tree<Kind>]) -> Option<(Symbol, Context, Span, Vec<Tree<Kind>>)> {
        let [
            Tree::Token { token, ctx },
            bang,
            Tree::Group {
                trees: args, close, ..
            },
            ..,
        ] = trees
        else {
            return None;
        };
        let Kind::Ident(name) = token.kind else {
            return None;
        };
        if !is_punct(bang, '!') || !self.macros.contains_key(&name) {
            return None;
        }
        Some((name, *ctx, token.span.merge(close.span), args.clone()))
    }
}
