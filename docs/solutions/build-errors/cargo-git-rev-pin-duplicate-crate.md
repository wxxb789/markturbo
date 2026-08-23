---
title: Pinning a shared git dependency by rev duplicates the crate and breaks every trait impl
date: 2026-08-20
category: build-errors
module: workspace-manifest
problem_type: build_error
component: tooling
symptoms:
  - "`no method named w_full found for struct gpui::elements::div::Div` on methods that visibly exist"
  - "`the method when exists ... but its trait bounds were not satisfied` on obviously-satisfied bounds"
  - "`the trait bound gpui::window::ElementId: From<gpui::SharedString> is not satisfied` between two types from the same crate"
  - "One easily-missed `note: there are multiple different versions of crate gpui in the dependency graph`"
root_cause: config_error
resolution_type: config_change
severity: high
framework_version: rustc 1.97.1
related_components: [cargo, gpui, gpui-component]
tags: [cargo, git-dependency, dependency-resolution, rev-pin, duplicate-crate, trait-resolution, gpui]
---

# Pinning a shared git dependency by rev duplicates the crate and breaks every trait impl

## Problem

This workspace depends on `gpui-component` (from `longbridge/gpui-component`) and also
needs `gpui` (from `zed-industries/zed`) directly. `gpui-component` itself depends on
`gpui`, which makes `gpui` a **shared transitive** dependency: both this app and the
component library must end up using the same copy, because the library ships trait
implementations for `gpui` types and this app passes its own `gpui` types into them.

Trying to be careful, `gpui` was pinned to an exact revision — read out of
`gpui-component`'s own `Cargo.lock`. That is the intuitive move, and it is exactly what
broke the build.

## Symptoms

Dozens of errors, all pointing at the wrong thing. Every one reads like a missing API, a
version skew, or a forgotten trait import:

```
error[E0599]: no method named `w_full` found for struct `gpui::elements::div::Div` in the current scope
error[E0599]: no method named `theme` found for reference `&gpui::Context<'_, DocumentView>`
error[E0599]: no method named `gap_1` found for struct `gpui::elements::div::Div`
error[E0277]: the trait bound `gpui::window::ElementId: std::convert::From<gpui::SharedString>` is not satisfied
error[E0599]: the method `when` exists for struct `gpui_component::button::Button`, but its trait bounds were not satisfied
```

Two tells, both subtle:

- A method **exists** but its trait bounds are "not satisfied", on a type whose bounds
  obviously are satisfied.
- `E0277` names `gpui::SharedString` and `gpui::ElementId` — two types that, read
  literally, come from the same crate and should convert fine.

The only diagnostic naming the real cause was a single `note:` buried in hundreds of
lines:

```
note: there are multiple different versions of crate `gpui` in the dependency graph
```

**That note is the whole diagnosis.** If you skim for `error[...]` lines you will never
see it. Any time a method that visibly exists is reported missing, or a trait bound that
is visibly satisfied is reported unsatisfied, search the raw log for
`multiple different versions` before doing anything else.

## What Didn't Work

Everything the error messages suggested — each dead end costs real time:

- **Hunting for the missing methods.** `w_full`, `gap_1`, and `theme` are all real and
  present. Reading upstream source to confirm they exist only builds confidence that the
  compiler is wrong, which delays the real diagnosis.
- **Adding trait imports.** `E0599` on an extension-trait method usually means a missing
  `use`. Here the traits were already in scope.
- **Toggling feature flags.** Plausible, since `theme` and `w_full` sit near
  optional-feature territory. It does not help: the problem is not *which* features are
  enabled, but which **crate instance** they are enabled on.
- **Diffing the version numbers.** Both copies report `gpui v0.2.2`. The version is
  identical; only the source differs.
- **Tightening the pin further.** The natural escalation — "the pin must not be exact
  enough" — moves in precisely the wrong direction.

## Solution

**Remove the pin.** Declare `gpui` with the same git source selector `gpui-component`
uses: same URL, and critically, no `rev`.

Before:

```toml
gpui = { git = "https://github.com/zed-industries/zed", rev = "e0931d5a9dbf4f781b336fdf448739e74a2ac0b5", features = ["profiler"] }
gpui_platform = { git = "https://github.com/zed-industries/zed", rev = "e0931d5a9dbf4f781b336fdf448739e74a2ac0b5", features = [...] }
```

After — the current manifest at `Cargo.toml:14-26` (lines 27-28 pin two sibling crates the
same way and are shown further down):

```toml
# gpui must be specified EXACTLY as gpui-component specifies it (same URL, no
# rev), or Cargo treats them as distinct sources and the graph ends up with two
# incompatible gpui crates. Cargo.lock is what actually pins the revision.
gpui = { git = "https://github.com/zed-industries/zed", features = ["profiler"] }
gpui_platform = { git = "https://github.com/zed-industries/zed", features = [
    "font-kit", "x11", "wayland", "runtime_shaders",
] }
gpui-component = { git = "https://github.com/longbridge/gpui-component", rev = "b77f352589503a114cdf709507c0738a88e28364", features = [
    "tree-sitter-languages",
] }
```

The upstream side this must match is `.reference/gpui-component/Cargo.toml:43-44` (a local
clone kept for API inspection, gitignored at `.gitignore:5-6`, so it may be absent from a
fresh checkout):

```toml
gpui = { version = "0.2.2", git = "https://github.com/zed-industries/zed", features = ["profiler"] }
gpui_platform = { git = "https://github.com/zed-industries/zed", features = ["font-kit", "x11", "wayland", "runtime_shaders"] }
```

### "Exactly as specified" is narrower than it sounds

What keys a Cargo git source is the **git URL plus the rev/tag/branch selector**. It is
*not* the version requirement and *not* the feature list.

Upstream writes `version = "0.2.2"` alongside the git URL; this repo omits it
(`Cargo.toml:17`). That difference is harmless — both declarations key to the same source.
Two manifests differing only in `version =` or `features = [...]` still unify: features
are unioned across the graph for ordinary same-target dependencies (this workspace sets
`resolver = "2"` at `Cargo.toml:3`, which deliberately does *not* unify build-dependency
and target-gated features — not a factor here), and the version requirement is only a
compatibility check against whatever the source resolves to. **Only the URL and the
rev/tag/branch selector split the graph.**

## Why This Works

Cargo identifies a git dependency by its full source specification, and the presence of a
`rev` is part of that identity. `git = "URL"` and `git = "URL", rev = "abc123"` are **two
different sources**, even when `abc123` is precisely what the plain form would resolve to.
Cargo does not resolve first and compare afterward — it treats them as unrelated origins
from the start and fetches both.

So the graph held two `gpui` crates. The part that makes the errors so misleading:
**`gpui::Div` from copy A is a genuinely different type from `gpui::Div` from copy B**, as
far as the type system is concerned. Same name, same version number, identical source
code — not the same type. Every trait implementation `gpui-component` ships
(`impl Styled for Div`, `impl From<SharedString> for ElementId`, the blanket bounds behind
`.when()`) is written against copy A's types. This app's values were copy B's types. The
impls simply do not apply.

That mismatch has no dedicated diagnostic. `rustc` reports what it locally sees: no such
method on this type, this bound not satisfied. The printed names are identical for both
copies, so the message reads as nonsense.

### The pin was wrong on both axes

`Cargo.lock` is what actually pins the revision, and it pins it for the whole graph at
once — the property the manual pin was trying and failing to buy. This repo's lock
resolves both `gpui` (`Cargo.lock:3010-3012`) and `gpui_platform` (`Cargo.lock:3373-3375`)
to one source:

```
source = "git+https://github.com/zed-industries/zed#314e0902ca079a01659b63a25785131774d66633"
```

**Note the revision: `314e0902…`, not the `e0931d5a…` that was pinned by hand.** The pin
split the source *and* named a revision this graph does not use. It was providing false
precision, not merely a redundant duplicate.

`e0931d5a…` is real — it is what the reference clone's own lock resolves to. But a
revision read out of a *dependency's* lockfile has no authority over *your* graph. Yours
resolves your graph, and it landed elsewhere. Reproducibility was never at risk without
the pin; it was `Cargo.lock`'s job the whole time.

### The general rule, and its asymmetry

Not a gpui problem:

> **A git dependency that is shared transitively — one that both your crate and another of
> your dependencies pull in — must be declared with an identical git source selector
> (same URL, same rev/tag/branch, or the absence of one) everywhere it appears. Let
> `Cargo.lock` pin the revision.**

The asymmetry is easy to over-correct on. Pinning `gpui-component` itself by rev is **fine
and desirable**, and this repo does exactly that (`Cargo.toml:24-28`) for all three of
`gpui-component`, `gpui-component-assets`, and `gpui-wry`.

`gpui-component` and `gpui-wry` have no other consumer in the graph, so there is no second
declaration to disagree with. `gpui-component-assets` is subtler and worth understanding,
because it looks like a counterexample: `gpui-component` *also* depends on it —

```
$ cargo tree -i gpui-component-assets
gpui-component-assets v0.5.1 (…gpui-component?rev=b77f3525…)
├── gpui-component v0.5.2 (…gpui-component?rev=b77f3525…)
│   └── mt-app v0.1.0
└── mt-app v0.1.0
```

— so it is genuinely shared. It stays safe only because upstream declares it as a **path**
dependency inside the same git checkout (`.reference/gpui-component/Cargo.toml:39`:
`gpui-component-assets = { path = "crates/assets", version = "0.5.1" }`). That path
resolves to the same source at the same rev this repo pinned, so the two declarations
cannot disagree. Pin it to a *different* rev than `gpui-component` and the duplicate
returns.

- **Leaf git dependency** (nothing else depends on it) → pin by `rev` freely. You are the
  only voice; you cannot contradict yourself.
- **Shared transitive git dependency** (something else also depends on it) → match the
  other declaration's selector exactly, and do not add a `rev` the other side lacks. When
  the other consumer reaches it by a `path` inside the same checkout, matching that
  checkout's rev is what keeps them unified.

The same logic applies to `path` dependencies and `[patch]` sections — and to any time you
add a *new* direct dependency on something already reachable transitively, which is
exactly the situation that created this bug.

## Prevention

**Detect it at declaration time.** Before adding a direct git dependency, check whether
anything already in the graph depends on it:

```bash
cargo tree -i <crate>
```

Anything other than your own crates means the dependency is shared: open the manifest of
whatever else depends on it and copy its git URL and rev/branch/tag selector verbatim.
Ignore differences in `version =` and `features = [...]`.

**Detect it after the fact.** `cargo tree --duplicates` is the right tool. In this repo it
prints a long list of genuinely duplicated crates (`anstyle`, `bitflags`, `resvg`, several
`windows-*` families) — normal noise in a large graph, none of it a problem. **The signal
is that `gpui` is absent.** Filter to the crate you care about:

```bash
cargo tree --duplicates | grep '^gpui'   # empty output = unified
cargo tree -i gpui --depth 0             # one line = one instance
# gpui v0.2.2 (https://github.com/zed-industries/zed#314e0902)
```

Two lines with the same version and different source hashes is the failure signature.

**The cheapest regression check** reads the lockfile and needs no build — suitable for CI
or a pre-commit hook:

```bash
grep 'source = .*zed-industries/zed' Cargo.lock | sort -u
```

This must return exactly **one** line. Two lines for the same repository URL — one bare,
one with `?rev=` — is the duplicate, visible without compiling anything.

**When you hit the confusing errors, read the log, not the summary:**

```bash
cargo build 2>&1 | grep -i 'multiple different versions'
```

**Guard the manifest with a comment.** The comment at `Cargo.toml:14-16` exists so a
future reader — or an agent tidying dependency declarations — does not "helpfully" re-add
the `rev` for consistency with the pinned `gpui-component` lines directly below it. The
manifest looks inconsistent on purpose; without the comment that reads like an oversight
worth fixing.

## Sibling gotcha: `gpui-component` ships no default features

Same file, same area, different failure mode — worse, because it is silent.

`gpui-component` declares **no** `default` feature: `.reference/gpui-component/crates/ui/Cargo.toml`
opens `[features]` with `decimal`, `inspector`, and `tree-sitter`, and contains zero
`default` keys. Syntax highlighting lives behind `tree-sitter-languages`, which must be
enabled explicitly:

```toml
gpui-component = { git = "...", rev = "...", features = ["tree-sitter-languages"] }
```

Omit it and there is no error, no warning, not even a runtime log line — just an editor
rendering text with no colors, easy to misread as a theme problem or a broken highlighter
config.

Upstream's own example confirms the intent: gpui-component's `crates/story/Cargo.toml`
(a path inside *that* project, not this one) declares
`tree-sitter = ["gpui-component/tree-sitter-languages"]` with `default = ["tree-sitter"]`.
The story crate opts in for itself; consumers must opt in for themselves.

**General form:** when a dependency's behavior degrades silently rather than failing,
check whether the crate declares `default` features at all before assuming your
configuration is wrong. A library with no defaults hands the entire feature-selection
burden to you, and says nothing when you decline something you wanted.

## Verification

The workspace compiles with the unpinned declarations, and 198 tests pass across 7 suites
(per this session's runs). The dependency-graph facts above are verified directly against
`Cargo.toml`, `Cargo.lock`, `.reference/gpui-component/Cargo.toml`, and live `cargo tree`
output.

> **On the revision hashes in this doc:** `314e0902…`, `e0931d5a…`, and `b77f3525…` are
> commits in the **upstream** `zed-industries/zed` and `longbridge/gpui-component`
> repositories, not in this one — a claims-checker run against this repo will not resolve
> them, which is expected. `b77f3525…` appears in `Cargo.toml`, `314e0902…` in
> `Cargo.lock`; `e0931d5a…` appears in neither, and that absence is precisely the point
> being made above.

## Does anything else in this graph have the same shape?

Asked again after the workspace gained `genai`, `reqwest`, `rustls`, `tokio`,
`toml`, `dirs`, `encoding_rs`, `chardetng`, and `tempfile` as direct
dependencies. `cargo tree -d` reports duplicates, but a duplicate is only *this*
bug when the two copies exchange types across an API boundary.

Only one of the new dependencies is duplicated at all: `toml` resolves to both
`0.8.23` and `1.1.4`. It is harmless, and for the reason that matters — the two
never meet. `1.1.4` is reached as a build-dependency; `mt-app` and
gpui-component's `rust-i18n-support` both link `0.8.23`, and no `toml` type
crosses between this crate and any other. `gpui` itself is not duplicated,
which is the invariant this document exists to protect.

The general test, when adding a dependency to a workspace that already shares a
git dependency: a duplicate matters if and only if one copy's types are passed
into code compiled against the other. Version skew in a leaf crate is noise.

## Related Issues

- `docs/platforms.md` — "Notes and caveats" carries a short operational form of this rule
  for someone setting up a build. This doc is the diagnosis-and-prevention version; that
  one is the heads-up.
