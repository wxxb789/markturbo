# Startup smoke ablation result

The original `trace_has_first_frame` function was extracted from the Python AST
in `.github/workflows/pull-request.yml` and executed against isolated temporary
trace files. No workflow or production source was edited, no process was
launched by this experiment, and no macOS runtime acceptance is claimed.

| Input | Original decision | Without the readiness guard |
| --- | --- | --- |
| Matching schema, nonce, PID and first-frame event | true | true |
| Only process-started event | false | true |
| Foreign nonce | false | true |
| Foreign PID | false | true |
| Foreign schema | false | true |
| Complete JSON without its publication newline | false | true |
| Empty trace | false | true |
| Malformed completed JSON row | RuntimeError | true |

All eight original decisions matched their expected contract. Removing the
guard loses seven distinct rejection cases; build success, architecture/signing
inspection, and headless unit success do not observe these runtime trace inputs.
Decision: retain the staged native first-frame smoke and its identity/publication
checks. Windows native UIA and macOS first-frame launch remain different OS
boundaries. The four Windows CLI adapter rejection/receipt comparisons in
`native-results.md` provide separate CLI smoke evidence, not GUI acceptance.
