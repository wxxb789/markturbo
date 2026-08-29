# Diagnose Duplicate Git Crates

Document and diagnose a Cargo failure where a direct git dependency is pinned by
`rev` while another dependency declares the same repository without that source
selector. The resulting duplicate crates make trait implementations appear to
be missing even though the APIs exist.

Lead with the recognizable compiler symptom, identify the dependency-tree
evidence that distinguishes the two sources, and state the source-selector fix.
Do not recommend unrelated trait imports or feature flags after the duplicate
source has been established.
