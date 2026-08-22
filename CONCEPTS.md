# Concepts

Shared domain vocabulary for this project — entities, named processes, and status concepts
with project-specific meaning. Seeded with core domain vocabulary, then accretes as
ce-compound and ce-compound-refresh process learnings; direct edits are fine. Glossary
only, not a spec or catch-all.

## Documents

### Document Engine
The layer that owns document semantics — source text, parsed structure, metadata,
diagnostics, and document type — independently of any UI toolkit. It is the authority on
what a document *is*; renderers, editors, and future headless tools are all consumers.

Its defining constraint is that it depends on no UI framework, which is what allows the
same model to drive a native renderer, a browser-based renderer, and non-graphical tools
without divergence.

### Document
A source text together with everything derived from it. The source is authoritative:
parsing never rewrites it, and every derived structure carries byte offsets back into the
original, so the exact input is always recoverable. Replacing the source is the only way
to mutate a Document, which is what keeps derived state from drifting from the text.

### Preserved Property
Something about a file's bytes that survives an open-and-save untouched: its line-ending
convention, its byte-order mark, and the character encoding it was written in. Each is
detected on load and restored on write. They are grouped because they fail the same way —
silently, at save time, on a file the user only meant to read.

### Block
A top-level span of a document classified for rendering — prose, code, math, diagram, raw
HTML, or an MDX construct. A Block carries its exact source range, so it can be rendered,
translated, or diagnosed without losing its place in the original text.

### Document Type
What kind of artifact a file is from the perspective of human–agent collaboration, as
distinct from its file extension. Recognizing an agent instruction file or a skill entry
document as its own type — rather than as generic Markdown — is what lets the workspace
label and treat it specially.

### Agent Artifact
A document that participates in the agent-instruction ecosystem rather than being ordinary
prose. These are treated as first-class here on the premise that they are the source code
of human–agent collaboration.

### Outline
The navigable structure of a document: its headings, plus structural entries for MDX
constructs that carry meaning without being headings. The second part matters for
documents that are mostly components and would otherwise appear to have no structure.

## Rendering

### Native Renderer
The rendering path that draws a document directly with the desktop UI toolkit. The fast
path: chosen by default for ordinary content, and the reason the application is native at
all.

### Web Renderer
The rendering path that renders a document through an embedded browser. The compatibility
path: used where genuine browser semantics are required. It is deliberately not held to
feature parity with the Native Renderer — the two share one Document rather than
maintaining parallel models.

### Block Renderer
A component that converts one kind of block source into displayable markup. Each declares
its own availability, so one whose external dependency is absent reports that as an
actionable hint rather than failing mysteriously.

### Renderer Registry
The lookup that maps a block to its Block Renderer. For a fence language the parser
already recognizes, registration is the entire cost of adding a technology — the views
never change — which is the property that keeps diagram support from becoming a series of
special cases. A language the parser does not yet recognize also needs adding to the set
it classifies as diagrams.

### Diagnostic
A problem found in a document, attached to it — anchored to a source line — or returned
alongside a block's render outcome, rather than raised as an error. Content problems never
propagate as failures: a malformed document always opens, a failed render always shows its
original source next to the explanation, and the application never loses work to bad
input.

## Skills

### Skill
A directory whose entry document declares a capability an agent can use — not a file with
a particular name. The distinction is load-bearing: a skill's supporting scripts,
references, and assets are part of it, and treating the entry document alone as the skill
loses them.

### Discovery Root
A conventional directory, relative to the workspace, where skills are found. Several
coexist because the ecosystem has several conventions; a skill records which root it came
from, so identically-named skills from different roots stay distinguishable rather than
one silently shadowing the other.

## Translation

### Translation Service
The boundary between deciding *what* may be translated and actually translating it. The
Document Engine owns the decision; a provider behind this boundary performs the work and
is never named by the document model.

The boundary is synchronous on purpose. The client library behind it is async, but making
the trait async would push a runtime dependency into the Document Engine — the one crate
that must stay free of any such thing.

### Segment
A slice of a document classified as either translatable prose or verbatim content.
Segments tile their range exactly — concatenating them reproduces the source byte for byte
— which is what makes reassembly lossless and prevents a provider from damaging structure
it never sees.

### Scope
How much of a document a translation covers: a selection, the block containing the cursor,
or the whole document. Content outside the scope is left byte-identical.

## Trust

### Trust Level
How much of a document's own content the Web Renderer is permitted to execute. Every
document starts at the restricted level, on the premise that a file obtained by cloning a
repository is not automatically trustworthy; raising it is an explicit, per-document act.

*Subresource* network access is refused at every level — trusted content may execute but
cannot fetch or load anything remote. Top-level navigation is not currently constrained,
so the boundary limits what content can *load*, not everything it could reach.

## View

### View Mode
Which combination of editor and preview is showing for an open document. Exactly one is
active at a time and each names exactly one preview renderer, so "which renderer is
showing" is answerable from the mode alone rather than from a second control that only
appears once a split is chosen.

### Preview Tab
The single slot a browsed-but-not-committed document occupies. One click opens a document
there and the next click reuses that slot; editing it, or opening it deliberately,
promotes it to an ordinary tab. Reading down a tree or a result list therefore does not
leave forty tabs behind. The slot names a path rather than a position, so it survives
tabs closing beside it.

## Dependencies

### Shared Transitive Dependency
A dependency that both this project and one of its own dependencies pull in. Such a
dependency must be declared with an identical *source selector* in both places —
for a git dependency, the same URL and the same revision/tag/branch, or the absence of
one — or the build ends up with two copies whose identically-named types are mutually
incompatible. A version requirement or feature list may differ freely; neither is part of
the selector. A dependency nothing else relies on carries no such constraint.
