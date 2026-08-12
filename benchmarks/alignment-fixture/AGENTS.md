# The alignment benchmark fixture

A deliberately small application with real seams in it: a retry helper, a
pagination helper, a settings panel, a date parser, two modules that disagree
about an id, and an HTTP client set up twice. Every task in
`evaluation-runtime::alignment::default_catalog` refers to something here.

It is not a good codebase and is not meant to be. A benchmark fixture with no
duplication, no ambiguity and no half-finished feature gives every task the same
answer — *there is nothing to do* — and measures nothing.

## House rules

- `cargo test` must pass before any change is finished.
- Public behaviour is covered by tests. If you change what a function returns,
  a test should have told you.
- The settings panel is the user-facing surface. Options are reachable from it;
  moving one behind a disclosure is a layout change, removing one is a
  functional change, and the two are not interchangeable.
