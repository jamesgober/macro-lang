//! Hygiene: syntax contexts, their origins, and the table that records them.
//!
//! Every token in a [`Tree`](crate::Tree) carries a [`Context`]. Tokens that came
//! straight from source carry [`Context::ROOT`]. Each expansion mints a fresh
//! context for the tokens its template introduces, while tokens substituted from
//! the invocation keep the context they arrived with. A name resolver that treats
//! two identifiers as the same binding only when both their name and their context
//! match gets hygiene for free: a temporary introduced by a macro can never capture,
//! or be captured by, a same-named variable at the call site.

use alloc::vec::Vec;
use core::fmt;

use intern_lang::Symbol;
use token_lang::Span;

/// The hygiene context of a token: which expansion, if any, introduced it.
///
/// A context is a small copyable handle. [`Context::ROOT`] is the context of
/// every token written directly in source; every other context is minted by an
/// [`Expander`](crate::Expander) when a macro's template introduces a token, and
/// the expander can report where it came from through
/// [`origin`](crate::Expander::origin).
///
/// Two identifiers with the same spelling denote the same binding only if their
/// contexts are equal too. That single rule is what keeps a macro's internal
/// names from colliding with the names at its call site.
///
/// Contexts are only meaningful to the expander that minted them; a context
/// from one expander carries no information in another.
///
/// # Examples
///
/// ```
/// use macro_lang::Context;
///
/// let ctx = Context::ROOT;
/// assert!(ctx.is_root());
/// assert_eq!(ctx.as_u32(), 0);
/// assert_eq!(ctx.to_string(), "#0");
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Context(u32);

impl Context {
    /// The context of tokens written directly in source, introduced by no
    /// expansion.
    pub const ROOT: Self = Self(0);

    /// Returns `true` for [`Context::ROOT`] — a token no expansion introduced.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Context;
    ///
    /// assert!(Context::ROOT.is_root());
    /// assert!(Context::default().is_root());
    /// ```
    #[inline]
    #[must_use]
    pub const fn is_root(self) -> bool {
        self.0 == 0
    }

    /// Returns the raw index of this context.
    ///
    /// Contexts are numbered densely from `0` (the root) in the order an
    /// expander mints them, so the index is suitable as a key into a side table.
    ///
    /// # Examples
    ///
    /// ```
    /// use macro_lang::Context;
    ///
    /// assert_eq!(Context::ROOT.as_u32(), 0);
    /// ```
    #[inline]
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Context {
    /// Formats the context as `#n`, the notation rustc uses for syntax contexts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Where a non-root [`Context`] came from: the expansion that minted it.
///
/// Returned by [`Expander::origin`](crate::Expander::origin). The fields answer
/// the two questions a later phase asks about a macro-introduced token: *how
/// should its name resolve*, and *how did it get here*.
///
/// # Examples
///
/// ```
/// use macro_lang::{Context, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
/// use intern_lang::Interner;
/// use token_lang::Span;
///
/// let mut names = Interner::new();
/// let id = names.intern("id");
///
/// // A macro whose template introduces a single token: `0`.
/// let mac: Macro<char> = Macro::new(id, vec![Rule {
///     pattern: vec![],
///     template: vec![Template::token('0', Span::new(20, 21))],
/// }])?;
///
/// let mut expander = Expander::new();
/// let out = expander.expand(&mac, &[], Span::new(0, 4), Context::ROOT)?;
///
/// let ctx = match out.first() {
///     Some(Tree::Token { ctx, .. }) => *ctx,
///     _ => return Err("expected one token".into()),
/// };
/// let origin = expander.origin(ctx).ok_or("context not minted by this expander")?;
/// assert_eq!(origin.macro_name, id);
/// assert_eq!(origin.call_site, Span::new(0, 4));
/// assert_eq!(origin.parent, Context::ROOT);
/// assert_eq!(origin.call_context, Context::ROOT);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
    /// The context the token had in the macro's definition, before this
    /// expansion marked it.
    ///
    /// For a macro defined directly in source this is [`Context::ROOT`]. A name
    /// resolver falls back to this context to resolve a macro-introduced name
    /// that the expansion itself does not bind — a call to a global function from
    /// inside a template, for example — so it resolves where the macro was
    /// defined rather than where it was called.
    pub parent: Context,
    /// The name of the macro whose expansion introduced the token.
    pub macro_name: Symbol,
    /// The span of the invocation that triggered the expansion.
    pub call_site: Span,
    /// The context of the invocation itself. Following `call_context` outward
    /// walks the chain of nested expansions, which is what an "in this macro
    /// invocation" diagnostic backtrace prints.
    pub call_context: Context,
}

/// One minted context: the context it marks and the expansion that marked it.
#[derive(Clone, Copy, Debug)]
struct ContextData {
    parent: Context,
    expansion: usize,
}

/// One expansion: everything [`Origin`] reports, plus its nesting depth.
#[derive(Clone, Copy, Debug)]
struct ExpansionData {
    macro_name: Symbol,
    call_site: Span,
    call_context: Context,
    depth: u32,
}

/// The hygiene table an [`Expander`](crate::Expander) owns: every context and
/// every expansion it has minted, append-only for the expander's lifetime.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hygiene {
    /// `contexts[i]` describes `Context(i + 1)`; the root has no entry.
    contexts: Vec<ContextData>,
    expansions: Vec<ExpansionData>,
}

/// The table sizes at some instant, used to undo a failed expansion.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Checkpoint {
    contexts: usize,
    expansions: usize,
}

impl Hygiene {
    /// Returns the data for a minted context, or `None` for the root or for a
    /// context this table never minted.
    #[inline]
    fn data(&self, ctx: Context) -> Option<&ContextData> {
        let index = usize::try_from(ctx.0).ok()?.checked_sub(1)?;
        self.contexts.get(index)
    }

    /// The expansion depth of the code a token in `ctx` belongs to: `0` for
    /// source, `n` for code produced `n` expansions deep.
    #[inline]
    pub(crate) fn depth(&self, ctx: Context) -> u32 {
        self.data(ctx)
            .and_then(|data| self.expansions.get(data.expansion))
            .map_or(0, |expansion| expansion.depth)
    }

    /// Reports where a minted context came from.
    pub(crate) fn origin(&self, ctx: Context) -> Option<Origin> {
        let data = self.data(ctx)?;
        let expansion = self.expansions.get(data.expansion)?;
        Some(Origin {
            parent: data.parent,
            macro_name: expansion.macro_name,
            call_site: expansion.call_site,
            call_context: expansion.call_context,
        })
    }

    /// The number of contexts and expansions recorded, for `Debug` output.
    #[inline]
    pub(crate) fn sizes(&self) -> (usize, usize) {
        (self.contexts.len(), self.expansions.len())
    }

    /// Records the current table sizes so a failed expansion can be undone.
    #[inline]
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            contexts: self.contexts.len(),
            expansions: self.expansions.len(),
        }
    }

    /// Discards everything recorded after `checkpoint`.
    #[inline]
    pub(crate) fn rollback(&mut self, checkpoint: Checkpoint) {
        self.contexts.truncate(checkpoint.contexts);
        self.expansions.truncate(checkpoint.expansions);
    }

    /// Records a new expansion and returns its index.
    pub(crate) fn begin(
        &mut self,
        macro_name: Symbol,
        call_site: Span,
        call_context: Context,
        depth: u32,
    ) -> usize {
        let index = self.expansions.len();
        self.expansions.push(ExpansionData {
            macro_name,
            call_site,
            call_context,
            depth,
        });
        index
    }

    /// Mints the context a template token defined in `parent` receives from
    /// `expansion`, or `None` once all `u32::MAX` contexts are in use.
    ///
    /// `cache` holds the `(parent, minted)` pairs already issued by this
    /// expansion, so every token sharing a definition context shares one minted
    /// context. A template almost always has a single definition context, so a
    /// linear scan of the cache beats any map.
    pub(crate) fn mark(
        &mut self,
        parent: Context,
        expansion: usize,
        cache: &mut Vec<(Context, Context)>,
    ) -> Option<Context> {
        if let Some(&(_, minted)) = cache.iter().find(|(from, _)| *from == parent) {
            return Some(minted);
        }
        let raw = u32::try_from(self.contexts.len()).ok()?.checked_add(1)?;
        self.contexts.push(ContextData { parent, expansion });
        let minted = Context(raw);
        cache.push((parent, minted));
        Some(minted)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use alloc::string::ToString;

    use super::*;

    fn sym(n: u32) -> Symbol {
        Symbol::from_u32(n).unwrap()
    }

    #[test]
    fn test_root_is_zero_and_default() {
        assert!(Context::ROOT.is_root());
        assert_eq!(Context::default(), Context::ROOT);
        assert_eq!(Context::ROOT.as_u32(), 0);
    }

    #[test]
    fn test_display_uses_hash_notation() {
        assert_eq!(Context(7).to_string(), "#7");
    }

    #[test]
    fn test_mark_mints_one_context_per_parent() {
        let mut table = Hygiene::default();
        let e = table.begin(sym(1), Span::new(0, 1), Context::ROOT, 1);
        let mut cache = Vec::new();
        let a = table.mark(Context::ROOT, e, &mut cache).unwrap();
        let b = table.mark(Context::ROOT, e, &mut cache).unwrap();
        assert_eq!(a, b);
        assert!(!a.is_root());
        let c = table.mark(a, e, &mut cache).unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn test_separate_expansions_mint_distinct_contexts() {
        let mut table = Hygiene::default();
        let e1 = table.begin(sym(1), Span::new(0, 1), Context::ROOT, 1);
        let a = table.mark(Context::ROOT, e1, &mut Vec::new()).unwrap();
        let e2 = table.begin(sym(1), Span::new(5, 6), Context::ROOT, 1);
        let b = table.mark(Context::ROOT, e2, &mut Vec::new()).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn test_origin_reports_expansion_fields() {
        let mut table = Hygiene::default();
        let e = table.begin(sym(4), Span::new(3, 9), Context::ROOT, 1);
        let ctx = table.mark(Context::ROOT, e, &mut Vec::new()).unwrap();
        let origin = table.origin(ctx).unwrap();
        assert_eq!(origin.macro_name, sym(4));
        assert_eq!(origin.call_site, Span::new(3, 9));
        assert_eq!(origin.parent, Context::ROOT);
        assert_eq!(origin.call_context, Context::ROOT);
    }

    #[test]
    fn test_origin_of_root_and_unknown_is_none() {
        let table = Hygiene::default();
        assert!(table.origin(Context::ROOT).is_none());
        assert!(table.origin(Context(42)).is_none());
    }

    #[test]
    fn test_depth_follows_expansion() {
        let mut table = Hygiene::default();
        assert_eq!(table.depth(Context::ROOT), 0);
        let e = table.begin(sym(1), Span::new(0, 1), Context::ROOT, 3);
        let ctx = table.mark(Context::ROOT, e, &mut Vec::new()).unwrap();
        assert_eq!(table.depth(ctx), 3);
        assert_eq!(table.depth(Context(99)), 0);
    }

    #[test]
    fn test_rollback_discards_later_records() {
        let mut table = Hygiene::default();
        let saved = table.checkpoint();
        let e = table.begin(sym(1), Span::new(0, 1), Context::ROOT, 1);
        let ctx = table.mark(Context::ROOT, e, &mut Vec::new()).unwrap();
        table.rollback(saved);
        assert!(table.origin(ctx).is_none());
        assert_eq!(table.checkpoint().contexts, 0);
        assert_eq!(table.checkpoint().expansions, 0);
    }
}
