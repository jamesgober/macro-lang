//! # macro_lang
//!
//! Hygienic macro expansion for language implementations: pattern matching over
//! token trees, template substitution, and the hygiene bookkeeping that keeps a
//! macro's names from colliding with its caller's.
//!
//! This is the `macro_rules!` / Scheme `syntax-rules` model, generic over a
//! language's token kind. A [`Macro`] is a list of [`Rule`]s; each rule pairs a
//! [`Pattern`] that an invocation must match with a [`Template`] it expands to.
//! An [`Expander`] runs the expansion: it matches the invocation's [`Tree`]s
//! against each rule in turn, writes out the first matching template, and marks
//! every token the template introduces with a fresh hygiene [`Context`].
//!
//! The crate owns no syntax. A language's front end lexes source into
//! `token_lang` tokens, groups them into [`Tree`]s, lowers its own macro
//! definition syntax into patterns and templates, and drives expansion — finding
//! invocations, resolving macro names, and splicing results back in.
//!
//! ## Hygiene in one paragraph
//!
//! Tokens written in source carry [`Context::ROOT`]. Each expansion mints a new
//! context for the literal tokens its template writes, while the trees it
//! substitutes from the invocation keep the context they arrived with. A name
//! resolver that treats two identifiers as the same binding only when both
//! spelling *and* context match then gets hygiene for free: a temporary the macro
//! introduces cannot capture a same-named variable passed in by the caller, and
//! cannot be captured by one. [`Expander::origin`] reports which expansion minted
//! a context, for definition-site fallback and for diagnostics.
//!
//! ## Example
//!
//! A macro whose template introduces a temporary named `t`, invoked on input
//! that already contains a `t`. After expansion the two `t`s have equal
//! spelling but different contexts, so they can never be confused.
//!
//! ```
//! use macro_lang::{Context, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
//! use intern_lang::Interner;
//! use token_lang::Span;
//!
//! let mut names = Interner::new();
//! let x = names.intern("x");
//!
//! // with_tmp!($x:tt) => t = $x
//! let with_tmp = Macro::new(names.intern("with_tmp"), vec![Rule {
//!     pattern: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
//!     template: vec![
//!         Template::token('t', Span::new(100, 101)),
//!         Template::token('=', Span::new(102, 103)),
//!         Template::Var(x),
//!     ],
//! }])?;
//!
//! // with_tmp!(t) — the caller passes its own `t`.
//! let mut expander = Expander::new();
//! let input = [Tree::token('t', Span::new(9, 10))];
//! let out = expander.expand(&with_tmp, &input, Span::new(0, 11), Context::ROOT)?;
//!
//! let contexts: Vec<Context> = out
//!     .iter()
//!     .filter_map(|tree| match tree {
//!         Tree::Token { token, ctx } if token.kind == 't' => Some(*ctx),
//!         _ => None,
//!     })
//!     .collect();
//!
//! // The macro's `t` was minted a fresh context; the caller's `t` kept the root.
//! assert_eq!(contexts.len(), 2);
//! assert!(!contexts[0].is_root());
//! assert!(contexts[1].is_root());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Matching
//!
//! Patterns are compiled into a nondeterministic automaton and simulated over the
//! input in a single left-to-right pass, merging parses that reach the same
//! point. Matching never backtracks, so its cost grows linearly with the input
//! (and polynomially with the pattern) even for adversarial input; and a rule
//! that matches in more than one way is reported as [`ExpandError::Ambiguous`]
//! instead of silently picking one parse.
//!
//! ## Features
//!
//! - `std` (default) — the standard library. Without it the crate is
//!   `#![no_std]` and needs only `alloc`; expansion uses no operating-system
//!   facilities either way.
//!
//! ## Stability
//!
//! The public surface is frozen and stable as of `1.0.0`: it follows Semantic
//! Versioning, with no breaking changes before `2.0`. The full surface, the
//! matching and hygiene semantics that are part of the contract, and the SemVer
//! promise are catalogued in
//! [`docs/API.md`](https://github.com/jamesgober/macro-lang/blob/main/docs/API.md#stability).

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![deny(
    warnings,
    missing_docs,
    unsafe_op_in_unsafe_fn,
    unused_must_use,
    unused_results,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::dbg_macro,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::undocumented_unsafe_blocks
)]

extern crate alloc;

mod compile;
mod context;
mod definition;
mod error;
mod expander;
mod matcher;
mod pattern;
mod template;
mod transcribe;
mod tree;

pub use context::{Context, Origin};
pub use definition::{Macro, Rule};
pub use error::{ExpandError, MacroError};
pub use expander::Expander;
pub use pattern::{Fragment, Kleene, Pattern};
pub use template::Template;
pub use tree::Tree;

/// Compiles and runs the `rust` code blocks in `README.md` and `docs/API.md` as
/// part of `cargo test`, so the published examples cannot drift from the API.
///
/// Present only while collecting doctests (`#[cfg(doctest)]`); it is not part of
/// the public surface and does not appear in the built library or its docs.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
#[doc = include_str!("../docs/API.md")]
pub struct MarkdownDocTests;
