# Goal 10 acceptance evidence

**Status:** implementation complete; native/environment and `pending (owner-run)` dispositions below prevent claiming complete public-quality acceptance. Evidence targets **v0.1.4-alpha.5**, revision `46669810a6530b945d4717070258ff111447e73f`. Publication remains draft-only. Candidate A establishes the staged-upgrade precondition only.

## Owner decisions recorded for this acceptance

Owner `wxxb789` supplied the following answers, recorded by the operator on 2026-11-14 in `local://goal10-owner-decisions.md`.

| Decision | Owner answer, verbatim | Date |
|---|---|---|
| Publication | “keep the release draft-only; the owner publishes after reviewing the acceptance evidence.” | 2026-11-14 |
| Acceptance machine | “just use this windows machine, it's devbox, it's windows 11 x64 as well. same thing.” | 2026-11-14 |
| Goal 11 measurement support | “implement the optional minimal local content-free event counters with deliberate export, inspect and disable, so Goal 11 can start measuring at publication.” | 2026-11-14 |

The acceptance-machine answer explicitly relaxes the clean-machine wording: this is a Windows 11 x64 development desktop, not a freshly provisioned machine or VM. The operator had no administrator rights and no usable clean Windows 11 x64 VM. Record that disposition rather than claiming the clean-machine clause was met on its original terms. Publication remains draft-only; no publishing, public-asset replacement or tag mutation is authorized here.

## Binding chain

Candidate A `MarkTurbo-Setup-0.1.4-alpha.4.exe`, **17,883,288 bytes**, installer SHA-256 `4a7f70827657750e7cfc24e128c711b500b45b0c42db0a96381691c110166716` → installed `MarkTurbo.exe`, **61,621,760 bytes**, SHA-256 `15c1c971f309d06e0508501963b97fdaf56e22715436faede9e4a88f184f8324` (**upgrade precondition only**).

Candidate B `MarkTurbo-Setup-0.1.4-alpha.5.exe`, **17,928,075 bytes**, SHA-256 `70672278964d5262d61ec9aee6f13b24a308fc7913fe27d44d86020b3a7637cb` → installed `MarkTurbo.exe`, **61,825,024 bytes**, SHA-256 `cc489fd2248f3b8eab4150c3509e3fe264a9800b11dcbe6d73b5242a8be1056f` → B results below. Downloaded installer/sidecar/manifest and GitHub draft asset 747241383 agree on installer digest/size. The manifest inventories payload paths and byte sizes, not payload hashes; the installed fingerprint was independently measured.

Final native runs used the pristine main checkout detached at B's tag with the existing scripts environment. Matrix receipts report source `46669810a6530b945d4717070258ff111447e73f`, tree `c37fa4306b3e7fa95f4e8e0b7785a8ffe4492e83`, dirty=false. Independent installed-EXE checks before/after matrix and probes agreed. The harness records bindings but does not enforce an expected hash; final evidence is not rebound to A, a local rebuild or the archived Goal 09 binary. Tracked documentation was written only after the native runs finished.

## Completion evidence

| # | Clause | Result | Evidence |
|---|---|---|---|
| 1 | Exact published artifact hash accepted on the machine | Tested **draft** candidate hash chain above; `pending (owner-run)` final publication/hash confirmation, with owner-relaxed development-machine disposition | B installer/executable chain; not an already-published acceptance claim |
| 2 | Release automation and packaging tests pass from a clean checkout | **PASS**, B CI Check/package/verify/hosted install-smoke; release build checked out exact B revision | Run 43581491321, release/build log checkout `46669810a6530b945d4717070258ff111447e73f` |
| 3 | `cargo fmt --all -- --check` passes | **PASS** within CI Check, followed by successful Clippy/tests/build | CI logged `cargo fmt --all -- --check` |
| 4 | Clippy introduces no new warnings | **No new warnings** per operator change/provenance assessment; two pre-existing `clippy::too_many_arguments` warnings in `agent_artifacts/package/source.rs` pre-date this release and were not introduced by it; not “zero warnings” | CI functions `collect_agent_skill_paths` (line 179), `collect_agent_skill_entry` (line 213); duplicates repeated for lib tests |
| 5 | Release workspace tests pass; publish pass count | **873 passed, 0 failed, 7 ignored**, 30 result lines; **214 tooling tests**, 4 shards | B CI extraction and per-target table below |
| 6 | Goal 04 startup/memory/CPU/window harnesses pass on final executable | B startup/memory/CPU commands **exit 0**, window gate **PASS**, quiet gate **FAIL**; timings are informational, not a full quiet-gated formal-acceptance PASS | B diagnostic table and `b-phase.json` below |
| 7 | Startup does not regress beyond approved Goal 04 guardrail, or release blocked with regression documented | **No numeric guardrail approved in Goal 04; comparison is not possible, not PASS.** No regression figure/threshold is invented | `docs/goal-04-completion.md`, owner-approved inconclusive no-go 2026-09-03 |
| 8 | Download page names verified platform and labels others accurately | **PASS** for rendered B draft copy: Windows 11 x64 only public-quality target; Windows 10 x64/macOS/Linux compatibility-only; unsigned/prerelease/acceptance pending explicit | `gh release view v0.1.4-alpha.5 --json isDraft,body,assets,url`; draft asset digest matches tested installer |
| 9 | Owner approves artifact/screenshots/demonstration/release copy | `pending (owner-run)`; current draft remains unpublished | Owner decision required before publication |

## Clean-machine acceptance items

These items use the owner's development-machine disposition above. Silent installation is explicitly recorded; the interactive wizard was not exercised.

| # | Item | Result | Evidence |
|---|---|---|---|
| 1 | Download and checksum | **PASS**, B installer/sidecar/manifest/draft asset digest and size agree; operator verification exit 0 without repository-version exception | B chain above; `release verify --manifest target/candidates/alpha.5/release.json` |
| 2 | Install through documented user path without developer tools | **PASS**, B installer exit 0 in default per-user directory; all layout/registration assertions passed. Silent installation, not interactive wizard; automation used Python/PowerShell | `/SILENT /SUPPRESSMSGBOXES /NORESTART`, B install facts |
| 3 | Desktop-shell launch and Welcome | **PASS**, B Start-menu shortcut launch reached Welcome through UIA | B UI facts; no foreground/keyboard claim |
| 4 | Bundled sample, native Markdown and Mermaid/D2/math smoke | `pending (owner-run)` renderer smoke: visual fixtures contain a plain plan, not bundled diagrams; blank captures cannot prove rendering | Shipped sample plus owner-observed valid-render outcomes |
| 5 | Unicode create/paste/save/close/reopen byte-correctly | **UIA create/save + B reopen round-trip PASS** by actual Save rewrite and byte equality. Source ValuePattern remains collapsed; actual clipboard Paste/keyboard remains `pending (owner-run)` | A 46-byte save; B documented open → UIA Save, changed mtime, identical 46-byte SHA; preview exposed both expected paragraphs |
| 6 | Controlled endpoint through supported credential path | `pending (owner-run)`; synthetic environment credentials in visual cells do not test secure/session credential controls | Live controlled-endpoint request and confidentiality observation |
| 7 | Review four lenses, Skill supporting file and exact disclosed/transmitted inventory | B plan-only Review loading/result/stale states each **12/12 CAPTURED**; four-lens Review and inventory comparison `pending (owner-run)` because no capture inspects transmitted supporting files | Checked-in rights-cleared corpus plus controlled request inspection; matrix is not overall PASS |
| 8 | Shipped EAC profile, named selection and outbound disclosure | B EAC panel **12/12 CAPTURED**; explicit source selection and endpoint/document/selected-context disclosure `pending (owner-run)`; a panel heading is not request-scope proof | Shipped `codex-agents-md-2026-08-29`; selected-source request inspection |
| 9 | Integrated Review and mixed accepted/rejected revision | B-tag headless `revision_flow`: **2 passed**; installed-artifact mixed-hunk confirmation `pending (owner-run)` | CI; `docs/verification.md:16-26` separates headless consent/apply from native OS seams |
| 10 | First installer release: staged A → B upgrade | **PASS**, measured immediate pre-upgrade document/settings/recovery preserved byte-for-byte; B version/launch/dark preference/three recents preserved | B upgrade facts; baseline drift explicitly recorded below, not hidden |
| 11 | Uninstall preserves user Markdown and leaves no product process | **PASS**, B uninstall exit 0; four owned payloads, shortcut/App Paths/Uninstall entry gone; product processes 0; user Markdown/settings/recovery preserved | B uninstall facts; no real-profile cleanup |

## Claims explicitly not made

- No move/repair support is claimed by the installer or the release copy; only
  install, in-place upgrade between tested candidates, and uninstall were
  exercised. No older portable-distribution upgrade compatibility is claimed.
- Uninstall offers no separate data-removal choice and removes only
  application-owned files (`installer/markturbo.iss`); it must not remove or
  modify user Markdown, settings or recovery data — verified in item 11.
- The artifact is an unsigned prerelease; nothing in the acceptance may describe
  it as signed.

## Environment dispositions (measured, not assumed)

1. **Disconnected session.** Operator `query session` reported session 2 `Disc`, console 1 `Conn` without a user. SendInput inserted **0 events**; SetCursorPos failed with **error 5**; GetForegroundWindow returned None; pywinauto click_input/type_keys failed. UIA Invoke and editor ValuePattern worked. A used UIA patterns only; no keyboard/clipboard/Narrator/foreground-pixel pass is inferred (`local://goal10-acceptance-handoff.md`, rehearsal, A facts).
2. **Blank visual captures, local Goal 09 executable—not A/B evidence.** First full matrix: **114/132 CAPTURED, 18 FAILED**, all `CAPTURE_EFFECTIVE_MODE_MISMATCH` (`target/native-acceptance/rehearsal-matrix/matrix.json`). Targeted three-state retry: **13/36 CAPTURED, 23 FAILED**, same reason (`target/native-acceptance/retry-3states/matrix.json`). Second requested full matrix is **incomplete**, stopped by the operator: retained **107 receipts, 91 CAPTURED, 16 FAILED**, no `matrix.json`; the earlier operator sample was approximately 25/41 captured with 16 failed and is a mid-run observation only. Do not invent a completed 132-cell result (`target/native-acceptance/rehearsal-matrix-2/cells/`, count of existing receipt JSON; Main's stop disposition).
3. **Blank-pixel interpretation.** Historical failed welcome/new-unsaved/settings captures were uniform **RGB (10,10,10)**; one 1750×1125 failure image had mean/max 10 while UIA still exposed 16 controls. Historical low-content dark cells could falsely pass the background check on a blank image; dark CAPTURED alone is not rendering proof. Archived final5 captures were real. **B's full run emitted FAIL, 111/132 CAPTURED, 21 FAILED**, all `CAPTURE_EFFECTIVE_MODE_MISMATCH`; every failure PNG was uniform RGB (10,10,10). Welcome failed 6 cells, New 6, Settings 9; all eight other states captured 12/12. **B had zero observed blank-dark CAPTURED cells**—do not convert the historical false-pass finding into a claim about this B run. B wrapper duration **754.161 s**, matrix interval **752.679 s**, summed cell intervals **737.947 s**. Source/exe hashes matched the pristine B tag/installed EXE and remained unchanged. No concrete environment correction existed, so no retry was eligible. The emitted FAIL is retained with an environment blank-capture disposition, not visual acceptance.
4. **`probe shot`: pending.** It reads foreground screen pixels and is unavailable in the disconnected session. PrintWindow cell images are a different capture method and must be assessed for blanks; they are not relabelled as probe-shot evidence.
5. **Quiet gate FAIL.** Against installed A with an isolated profile, the operator ran `probe quiet --samples 60 --interval 1 --wait-seconds 0`: CPU median **19.78% > 5%**, CPU p95 **35.45% > 10%**, disk p95 **14.67% > 10%**. Disk median was not recorded in the supplied baseline and is not invented. Goal 04's owner-approved inconclusive no-go on 2026-09-03 approved **no numeric startup/materiality guardrail**. A/B timings remain informational, not quiet-gated release-regression acceptance (`local://goal10-a-baseline.md`, `docs/goal-04-completion.md`).

6. **Pre-upgrade baseline drift, preserved—not erased.** A-phase recorded settings 646 bytes / two recents. The operator identified a pre-B rehearsal launch of installed A at 10:15:00Z (`logs/markturbo-300.log`, pid 300) rewriting settings to **812 bytes**, SHA-256 `b3d0565ec920aa932f0d1a9c363055a9ca0d568408ab378c17eca1f39aacec9f`, **three recents** (staged document, staged folder, pre-existing root). The first B-install attempt failed the stale 646-byte baseline assertion **before invoking the installer** (exit 1, `B_PHASE_A_SETTINGS_CHANGED`). The measured current snapshot was frozen instead; actual B install preserved all 812 bytes and the recovery aggregate. No entry was dropped and no real-profile artifacts were tidied. The actor/log attribution is operator-provided, not a claim this worker re-read private logs.

7. **Collapsed Source accessibility, alternate honest reopen proof.** Initial B check failed `B_SOURCE_LAYOUT_REOPEN_VALUE_MISMATCH_OR_COLLAPSED` (exit 1): Source editor 0×25, empty ValuePattern even after Edit Invoke. One WindowPattern maximize correction still left 0×25/empty; observed layout names were Edit, Markdown source, More layouts and Preview, with no splitters. The app did expose both expected document paragraphs through preview UIA. Documented open → UIA Save changed mtime from `1794650823985444600` to `1794655246587328100` ns, with exact 46-byte SHA-256 unchanged. Thus the **reader/writer round-trip passed**, not the inaccessible Source-value check; no empty read or no-op save was accepted.

8. **B quiet gate also failed.** CPU median **16.31% > 5%**, p95 **27.65% > 10%**; disk median **0.61%**, p95 **1.90%**, both below limits 2%/10%. Exit **1** after the 60-s window. Plain sampling deliberately omitted `--evidence`, whose formal provenance path requires an active session; this collected a real failed host gate, not formal startup evidence. No quiet threshold was relaxed and no retry-until-green was attempted.

No historical failure or blank image is edited into PASS. Native JSON receipts are content-free, but visual output also retains synthetic workspace Markdown and PNGs; the whole directory is not content-free. Keep actual user text, settings/recovery payloads, credentials and absolute local paths out of retained native evidence.

## Candidate A

### Identity and installation

Source: `target/native-acceptance/goal10-final/a-phase.json`; A CI reference **43577189324**. A is an unsigned prerelease, not final acceptance.

| Observation | Result |
|---|---|
| Installer | `MarkTurbo-Setup-0.1.4-alpha.4.exe`; 17,883,288 bytes; SHA-256 `4a7f70827657750e7cfc24e128c711b500b45b0c42db0a96381691c110166716` |
| Pre-install process count / install exit | **0 / 0**; install script wall time **10.08 s** |
| Installed EXE | 61,621,760 bytes; SHA-256 `15c1c971f309d06e0508501963b97fdaf56e22715436faede9e4a88f184f8324` |
| Payload and metadata | `MarkTurbo.exe`, `nsh/app.json`, `LICENSE`, notices all present; app metadata version `0.1.4-alpha.4` |
| Start-menu shortcut | Present; target matches installed EXE |
| App Paths | Both `(default)` executable and `Path` install directory match; retained values use `%LOCALAPPDATA%/Programs/MarkTurbo` placeholders |
| Uninstall entry | Exactly one; `MarkTurbo 0.1.4-alpha.4`, DisplayVersion `0.1.4-alpha.4`, InstallLocation matches; key `{6BC0E0A7-53F2-4C61-9A59-5E8D7B4C2F10}_is1` |
| Verification | Worker independently matched installer size/hash/manifest; operator's `release verify --ignore-repository-version` exit 0 is attributed, not rerun |

### Staged user state and relaunch

New → editor ValuePattern → Save → native **child** Save As (filename auto_id `1001`, Save auto_id `1`) wrote an exact UTF-8 **46-byte** Unicode document outside the install directory, SHA-256 `1fa7e48a9bce6e788a24e014cf3607a7d15d02f130f3aa08cca4847f35300f91`. No document text/path is retained in the evidence JSON. Settings → Dark persisted **`theme = "dark"`**. The Save UI recorded the document in recents: **1 pre-existing → 2 total**, with staged-document membership true; no extra direct settings write or fabricated recent entry was used.

Both closes used **Quit UIA Invoke**, observed graceful process exit, and used no Stop-Process or WM_CLOSE fallback. PowerShell Start-Process on the Start-menu shortcut reached Welcome; saved file bytes, dark preference and recent membership remained intact. Final process count **0**. UI stage script wall time **30.51 s**. Settings fingerprint: **646 bytes**, SHA-256 `ec1018d5b9ef2aa7d893067de5ecd5364e6fec471d17a47f26bf4e2e020c99fe`.

Exact entry commands were `powershell.exe -NoProfile -ExecutionPolicy Bypass -File target/goal10-work/a-install.ps1` and `uv run --locked --project scripts python target/goal10-work/a-stage.py`. Installation used `/SILENT /SUPPRESSMSGBOXES /NORESTART`, default per-user directory. This is a silent installer run, not interactive-wizard evidence. A-stage proved save and disk persistence; final B's reopen editor check must handle the collapsed Source pane described in the B plan.

### Probe diagnostics against installed A

These are **operator-measured**, isolated-profile, non-gating observations from `local://goal10-a-baseline.md`; they were not rerun by this worker.

| Command suffix (`mt.py probe -- … --exe <installed-A>`) | Observed result |
|---|---|
| `startup --rounds 5 --warmup 1` | Samples **98.6 / 86.1 / 86.2 / 100.4 / 98.6 ms**; median **98.6 ms**, p95 **100.0 ms**; first-visible only |
| `memory --settle 12` | Working set **54.3 MB**, private **13.8 MB**, peak **54.3 MB**, **24 threads**, **14,249 page faults** |
| `cpu --settle 12 --window 5 --samples 6` | Six **0.0%** samples; steady state **0.0%** of one core; working set **54.6 MB** |
| `windows --settle 6 --resize-cycles 1` | **PASS**; alternate **1054×718**, restore **1214×838**, WM_CLOSE exit **0**, stderr/startup-log scans PASS; no WebView data |
| `quiet --samples 60 --interval 1 --wait-seconds 0` | **FAIL**, exact CPU/disk figures above; not a quiet-gated timing baseline |

The earlier local-install rehearsal and 6.390-second local packaged-exe windows PASS are procedure evidence for their named artifact only. They are not added to A/B final-artifact results.

## Adjacent Goal 11 verification

`local://goal11-metrics-verification.md` records operator UIA checks on a locally packaged merged-main runner (**61,863,936 bytes**), not the CI-installed A or final B. Settings rendered local counters; On/Off persisted, disabled launch/quit wrote no `usage.json`, enabled launch/quit recorded one coarse first-launch event, Inspect showed counts, and deliberate native Save-dialog export contained allowed fields only. No content, paths, credentials or precise timestamp appeared in store/export/inspection. Clear was covered by the headless usage-metrics test. This corroborates the approved implementation; B CI separately passed the usage_metrics target, and the adjacent local runner is not silently rebound to B.

## Candidate B CI results

Read using `gh run view 43581491321 --repo wxxb789/markturbo --log` and `--json headSha,headBranch,conclusion,jobs,url`. All five jobs succeeded: bump, release/contract, release/build, release/install-smoke, release/draft-release. Run-level triggering SHA `45d3dc22575232edd773bd4173872e77c79bc195` is the **pre-bump** revision; the release/build/contract/draft jobs explicitly checked out B tag revision `46669810a6530b945d4717070258ff111447e73f`.

The following counts are extracted from CI output, not locally rerun. All failed columns are zero. Interleaved stdout/stderr placed the untitled-recovery “Running” header before translation-flow's result, so target assignment follows the actual test-start/result context rather than blindly taking the last header.

| Target | Passed | Ignored |
|---|---:|---:|
| mt-app lib | 42 | 0 |
| mt-app main | 0 | 0 |
| mt-app conflict | 1 | 0 |
| mt-app hierarchy_layouts | 1 | 0 |
| mt-app hierarchy_welcome | 1 | 0 |
| mt-app language_switch | 1 | 0 |
| mt-app paste_document | 1 | 0 |
| mt-app quit_guard | 1 | 0 |
| mt-app quit_untitled | 1 | 0 |
| mt-app review_stale | 1 | 0 |
| mt-app revision_flow | 2 | 0 |
| mt-app save_twice | 1 | 0 |
| mt-app settings_form_state | 1 | 0 |
| mt-app shell | 1 | 0 |
| mt-app tab_switch | 1 | 0 |
| mt-app theme | 1 | 0 |
| mt-app translation_flow | 1 | 0 |
| mt-app untitled_recovery | 1 | 0 |
| mt-app untitled_save | 1 | 0 |
| mt-app usage_metrics | 1 | 0 |
| mt-app welcome | 1 | 0 |
| mt-core lib | 699 | 3 |
| mt-core goal06 evaluator | 11 | 0 |
| mt-core goal07 evaluator | 18 | 0 |
| mt-core context | 40 | 0 |
| mt-core discovery | 10 | 0 |
| mt-core effective_context_review | 12 | 0 |
| mt-core fixtures | 16 | 0 |
| mt-core model_first_use_cost | 0 | 1 |
| mt-core performance | 5 | 3 |
| **30 result lines, total** | **873** | **7** |

CI logged `cargo fmt --all -- --check`, `cargo clippy --profile release --workspace --all-targets --locked`, `cargo test --profile release --workspace --locked`, and `cargo build --release --locked -p mt-app --bin markturbo --message-format=json-render-diagnostics`. Clippy completed with the two pre-existing warnings described above. Release binary build completed; Check succeeded. No standalone privacy success line is emitted: `checks.py::full` calls `privacy.scan(ROOT, binary)` and fails on findings, so privacy ran as part of successful Check, not an invented separate console PASS. Installer package and both build/draft verify logs name the exact B installer SHA above.

Hosted install-smoke succeeded on Windows Server 2022: downloaded checksum agreement, silent temporary-directory install exit 0, four required payload files, Start-menu shortcut and default App Paths target; silent uninstall exit 0, executable/shortcut absent, product process absent, and an external configuration marker retained. It does not assert UI launch, both App Paths values, Uninstall metadata or all actual user-state preservation; local B covers those separately.

## Candidate B local measurements

Upgrade script: `powershell.exe -NoProfile -ExecutionPolicy Bypass -File target/goal10-work/b-install.ps1`; exit **0**, wall **10.23 s** after the recorded pre-install stale-baseline failure. Installer identity/size, all four payloads, shortcut target, both App Paths values, exactly one Uninstall entry (`MarkTurbo 0.1.4-alpha.5`/DisplayVersion `0.1.4-alpha.5`/symbolic per-user InstallLocation), installed metadata and EXE inventory size all passed. Immediate user document and 812-byte settings were identical; one recovery file's aggregate `5b4f2a5da75d16c3346e3cf42f04926418926ffe5697f61d36898ca8eb067f89` was unchanged. Default profile remains under `%APPDATA%/MarkTurbo`; no settings path under LOCALAPPDATA is asserted.

`uv run --locked --project scripts python target/goal10-work/b-ui.py`: Start-menu Welcome, dark theme, three recents and staged-document membership passed; shell Quit exited gracefully. Source reopen check failed as recorded above and cleaned up through Quit; wall **33.08 s**. `b-reopen.py` then applied the single geometry correction and proved the actual Save round-trip/preview paragraph exposure; wall **13.82 s**, graceful Quit. The known collapsed pane is retained as an environment limitation, not suppressed or called a new repaired product defect.

B matrix: `uv run --locked --project scripts scripts/mt.py visual -- --exe <installed-B> --out target/native-acceptance/goal10-final/b-visual`; exit **1**, **111/132 captured**, **21 failures**, source clean at `46669810a6530b945d4717070258ff111447e73f`, tree `c37fa4306b3e7fa95f4e8e0b7785a8ffe4492e83`, EXE hash unchanged. Offline PNG/receipt analysis confirmed the blank-failure counts above. Provider uses process-local synthetic OpenAI Chat credentials; no owner's endpoint or real credential was used. Receipts carry content-free bindings/geometry/status; synthetic fixture files/PNG text remain in the visual output.

### B installed-artifact diagnostics

Executed serially through `uv run --locked --project scripts python target/goal10-work/b-probes.py` with temporary config/data profiles; installed EXE hash was checked before/after each command. Wrapper wall time **129.36 s**. Commands are `uv run --locked --project scripts scripts/mt.py probe --` followed by:

| Command suffix | Exit | Actual reported results | Command duration |
|---|---:|---|---:|
| `quiet --samples 60 --interval 1 --wait-seconds 0` | 1 | CPU median 16.31%, p95 27.65%; disk median 0.61%, p95 1.90%; **FAIL** | 62.443 s |
| `startup --exe <installed-B> --rounds 5 --warmup 1` | 0 | Samples **101.0 / 93.6 / 101.6 / 89.7 / 91.6 ms**; median **93.6 ms**, p95 **101.5 ms** | 1.491 s |
| `memory --exe <installed-B> --settle 12` | 0 | Working set **54.6 MB**, private **14.2 MB**, peak **54.6 MB**, **14,337 page faults**, **27 threads** | 12.855 s |
| `cpu --exe <installed-B> --settle 12 --window 5 --samples 6` | 0 | Six reported **0.0%** one-core samples, second-half mean **0.0%**, working set **54.6 MB** | 42.814 s |
| `windows --exe <installed-B> --settle 6 --resize-cycles 1` | 0 | **PASS**, one NshWindow plus two system IME helpers, 0 children; alternate **1054×718**, restore **1214×838**; WM_CLOSE exit 0; stderr/startup scans PASS; no WebView data | 8.420 s |

Startup measures only first visible titled window, not first paint/readiness/input. Memory/CPU exits have no numerical release threshold. Window probe requests foreground but does not enforce foreground success. Because quiet failed and no Goal 04 numeric guardrail exists, these numbers cannot be promoted into formal startup-regression acceptance. Probe shot was not run in the disconnected session.

### B uninstall safety

`powershell.exe -NoProfile -ExecutionPolicy Bypass -File target/goal10-work/b-uninstall.ps1` invoked `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`; exit **0**, wall **8.61 s**, measured 11:36:29Z. `MarkTurbo.exe`, `nsh/app.json`, LICENSE and notices were absent; Start-menu shortcut, App Paths and Uninstall entry absent; **0 product processes** remained. User Markdown remained **46 bytes**, SHA-256 `1fa7e48a9bce6e788a24e014cf3607a7d15d02f130f3aa08cca4847f35300f91`. Immediately pre/post-uninstall settings were **842 bytes**, SHA-256 `19adb4e6c1c7f2797968f4ab4b51a8f887b58f753e36834c19cca45cb3cdf265`, unchanged; one recovery file and its aggregate were unchanged. The 842-byte fingerprint is the current post-B-activity state, not the 812-byte immediate-install or 646-byte A-phase snapshot. No extra recent, staged file or real-profile artifact was erased to tidy the run.

The release screenshots remain Goal 09's accepted set plus any later owner-approved fresh captures from a connected session. This B matrix is not presented as artifact-level visual acceptance. Bundled-sample/Mermaid/D2/math appearance remains explicitly owner-run.

## Pending owner items (release-blocking)

- `pending (owner-run)`: keyboard traversal, visible focus and Narrator smoke; UIA patterns do not exercise these disconnected-session seams.
- `pending (owner-run)`: five first-time-user sessions, recorded as `docs/goal-09-acceptance.md` requires; an operator/matrix run is not a user study.
- `pending (owner-run)`: bundled sample and valid native Markdown/Mermaid/D2/math appearance smoke.
- `pending (owner-run)`: live controlled-endpoint credential-path Review for the four lenses, including a Skill supporting file and exact normalized-path/byte-size/inclusion-reason disclosure versus transmitted-files comparison.
- `pending (owner-run)`: explicitly selected named EAC sources and endpoint/document/selected-context disclosure comparison.
- `pending (owner-run)`: installed-artifact mixed accepted/rejected revision and confirmation only approved output applies; same-revision headless proof is separate.
- `pending (owner-run)`: short rights-cleared/synthetic Review demonstration and approval of final artifact, screenshots, demonstration and release copy before publication.
- Recorded B install/upgrade/reopen/CI/uninstall results are bounded observations; the visual FAIL, quiet FAIL and inaccessible Source-value check are retained. Owner approval/publication and the explicit user workflows above remain pending rather than being inferred from CI or diagnostic exits.

## Carried forward

The two operator-approved candidates from `local://goal10-todo-entry.md` are appended under `docs/TODO.md` Deferred after native runs: **retired `mt.py accept goal-02/03/06/07` GPUI adapters**, and **the documented plain cargo-binary startup panic**. The latter was operator-measured: exit **101**, `no reactive runtime: signals are used on the engine thread, while components run`, while the nsh-packaged same-revision runner launches. This worker did not rerun that reported failure. Revalidate each candidate before deleting code/changing the documented entry point; no new acceptance harness was invented.

## Evidence paths

- `target/native-acceptance/goal10-final/a-phase.json` — measured A identity, installation and UI-staged state.
- A CI run: **43577189324**, <https://github.com/wxxb789/markturbo/actions/runs/43577189324>.
- B CI run: **43581491321**, <https://github.com/wxxb789/markturbo/actions/runs/43581491321>; extracted results in `target/native-acceptance/goal10-final/ci-facts.json`.
- `target/native-acceptance/rehearsal-matrix/matrix.json`, `retry-3states/matrix.json`, `rehearsal-matrix-2/cells/*/receipt.json` — local-binary blank-capture attempts, not final B acceptance.
- `target/native-acceptance/goal10-final/b-phase.json` — content-free B installer/executable chain, failed/alternate reopen checks, upgrade/probes/uninstall and pending dispositions.
- `target/native-acceptance/goal10-final/b-visual/matrix.json` and cell receipts/PNGs — emitted FAIL and independently inspected uniform failure images; synthetic fixture workspace files are also retained there.
- `target/goal10-work/b-phase-plan.md` — ordered planned commands, not execution evidence.
- Operator source records: `local://goal10-owner-decisions.md`, `goal10-acceptance-rehearsal.md`, `goal10-visual-blank-captures.md`, `goal10-a-baseline.md`, `goal11-metrics-verification.md`.
