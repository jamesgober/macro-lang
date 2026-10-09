//! Bounding hostile macros: the recursion limit and the expansion budget.
//!
//! Macro input is untrusted in the same way source text is: a two-line macro
//! can describe unbounded work. This example runs three such macros and shows
//! each one stopped, with no configuration beyond `Expander::new()` unless
//! noted.
//!
//! - `quine!` rebuilds its own invocation from the tokens it captured:
//!   `quine!(quine !)` writes `quine ! (quine !)`, a call identical to the one
//!   it came from. A driver that tracks where it found each invocation stops it
//!   at the recursion limit; a driver that only rescans relies on the budget.
//! - `dup!` doubles its argument every round, reaching `2^n` tokens at depth
//!   `n` — far past any memory long before depth 128. The token budget stops
//!   it.
//! - A custom `Budget` sets tighter limits for an environment that needs them,
//!   such as an editor expanding on every keystroke.
//! - A long-lived host (a language server here) keeps one expander for every
//!   document and calls `reset_usage` before each one, so the budget bounds
//!   each document rather than the host's lifetime — and one hostile document
//!   cannot use up the budget of the documents after it.
//!
//! ```bash
//! cargo run --example budget
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

mod common;

use std::time::Instant;

use common::{Driver, define, parse_trees, render};
use intern_lang::Interner;
use macro_lang::{Budget, ExpandError, Expander};

fn main() {
    let mut names = Interner::new();
    let quine = define("quine", "($a:tt $b:tt) => { $a $b ($a $b) }", &mut names);
    let dup = define("dup", "($($t:tt)*) => { dup!($($t)* $($t)*) }", &mut names);
    let count = define(
        "count",
        "
        () => { 0 } ;
        ($head:tt $($tail:tt)*) => { 1 + count!($($tail)*) }
        ",
        &mut names,
    );

    // 1. A self-rebuilding macro under a depth-tracking driver.
    let mut driver = Driver::new(Expander::new());
    driver.add(quine.clone());
    let src = "quine!(quine !)";
    let started = Instant::now();
    let result = driver.expand_tracked(parse_trees(src, &mut names), 0);
    report(src, "tracked driver", &result.map(|_| ()), started);

    // 2. The same macro under a rescanning driver: depth cannot be seen, so
    //    the default budget ends it.
    let mut driver = Driver::new(Expander::new());
    driver.add(quine);
    let started = Instant::now();
    let result = driver.expand_all(&mut parse_trees(src, &mut names));
    report(src, "rescanning driver", &result, started);

    // 3. A doubling macro: the token budget ends it.
    let mut driver = Driver::new(Expander::new());
    driver.add(dup);
    let src = "dup!(x)";
    let started = Instant::now();
    let result = driver.expand_all(&mut parse_trees(src, &mut names));
    report(src, "default budget", &result, started);

    // 4. A tight custom budget still lets ordinary macros through.
    let budget = Budget::DEFAULT
        .with_max_depth(32)
        .with_max_expansions(1_000)
        .with_max_tokens(10_000);
    let mut driver = Driver::new(Expander::with_budget(budget));
    driver.add(count);
    let src = "count!(a b c d)";
    let mut program = parse_trees(src, &mut names);
    driver
        .expand_all(&mut program)
        .expect("count! fits the budget");
    println!(
        "{src} [tight budget] => {}",
        render(&program, &names, false)
    );

    // 5. A long-lived host: the same expander for every document, its usage
    //    reset per document. The hostile `dup!` document fails on its own
    //    budget; the next document gets a full budget again.
    driver.add(define(
        "dup",
        "($($t:tt)*) => { dup!($($t)* $($t)*) }",
        &mut names,
    ));
    for src in ["count!(a b)", "dup!(x)", "count!(a b c)"] {
        driver.expander.reset_usage();
        let mut program = parse_trees(src, &mut names);
        match driver.expand_all(&mut program) {
            Ok(()) => {
                let usage = driver.expander.usage();
                println!(
                    "{src} [per-document budget] => {} ({} expansions, {} tokens)",
                    render(&program, &names, false),
                    usage.expansions,
                    usage.tokens
                );
            }
            Err(err) => println!("{src} [per-document budget] => error: {err}"),
        }
    }
}

fn report(src: &str, how: &str, result: &Result<(), ExpandError>, started: Instant) {
    let elapsed = started.elapsed();
    match result {
        Ok(()) => println!("{src} [{how}] => expanded (unexpected)"),
        Err(err) => println!("{src} [{how}] => error: {err} (stopped in {elapsed:.1?})"),
    }
}
