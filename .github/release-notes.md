# MarkTurbo __VERSION__

Channel: **__CHANNEL__**.

<!-- CI replaces this with the complete signing status. For an unsigned
prerelease, it must explicitly warn that the installer is unsigned; it must
never describe an unsigned artifact as signed. Stable publication requires
Authenticode signing. -->
__SIGNING_STATE__

## Download

Platform status: __PLATFORM_STATE__

Download [__ARTIFACT__](https://github.com/wxxb789/markturbo/releases/download/v__VERSION__/__ARTIFACT__),
the [checksum sidecar](https://github.com/wxxb789/markturbo/releases/download/v__VERSION__/__ARTIFACT__.sha256),
and [release.json](https://github.com/wxxb789/markturbo/releases/download/v__VERSION__/release.json)
from this release. The single Windows installer follows the name
`MarkTurbo-Setup-<version>.exe`; `release.json` records its version, channel,
signing state, and payload inventory.

SHA-256 (`__ARTIFACT__`): `__CHECKSUM__`

In PowerShell, compare the displayed hash with the checksum above and the
sidecar before running the installer:

```powershell
Get-FileHash -Algorithm SHA256 '.\__ARTIFACT__'
Get-Content '.\__ARTIFACT__.sha256'
```

Or verify the `sha256sum`-format sidecar from the download directory:

```sh
sha256sum --check '__ARTIFACT__.sha256'
```

Do not install if the hashes differ. Read the signing status above: an explicitly
unsigned prerelease is not a signed stable release, and a self-signed test build
is not trusted by Windows by default.

## Install

Run the verified installer. Installation is per user, without administrator
rights, under `%LOCALAPPDATA%\Programs\MarkTurbo`. Launch **MarkTurbo** from the
Start menu. The installer also registers Shell App Paths and an uninstaller.
Uninstall removes application-owned installation files; user Markdown,
settings, and recovery data live elsewhere and survive uninstall. A running
instance is closed before application-owned files are removed. No untested
upgrade path or compatibility with an older portable distribution is claimed.
This is the first installer-based release.

## First Review

1. Choose **Open Folder** for your workspace or **Open Bundled Sample** on the
   welcome screen. Open an artifact, or use **New** or **Paste** for a rough
   prompt or plan.
2. In **Settings**, configure the provider wire format, model, base URL, and
   credential using secure storage or session-only use. Never put the key in
   your document, screenshots, or settings file.
3. Inspect **Context** and explicitly select any named Effective Agent Context
   sources to include. Only `codex-agents-md-2026-08-29` is a verified profile;
   other harness discoveries are inventory-only.
4. Start **Review**, choose the lens and scope, and inspect the endpoint and
   listed content before choosing **Send to model**. Read the findings and
   clarification questions. Review does not apply changes automatically.
5. Answer the relevant questions and choose **Prepare revision**. Confirm its
   separate disclosure before **Send revision**, inspect and approve or reject
   each proposed change, then choose **Apply approved** and save the result you
   want to keep.

See the [first-Review walkthrough](https://github.com/wxxb789/markturbo/blob/v__VERSION__/README.md#first-review)
and [privacy explanation](https://github.com/wxxb789/markturbo/blob/v__VERSION__/README.md#privacy)
for local operations, endpoint disclosure, credentials, recovery, and logs.

## Screenshots

Goal 09 release-build visual matrix contact sheets:

- [Default viewport](https://github.com/wxxb789/markturbo/blob/v__VERSION__/docs/evidence/goal-09/final4/contact-default.png)
- [Minimum viewport](https://github.com/wxxb789/markturbo/blob/v__VERSION__/docs/evidence/goal-09/final4/contact-minimum.png)
- [Wide viewport](https://github.com/wxxb789/markturbo/blob/v__VERSION__/docs/evidence/goal-09/final4/contact-wide.png)

These are screenshots, not a Review demonstration recording. That recording is
pending.

## Known limitations and acceptance

- Windows 11 x64 is the only public-quality target. Windows 10 x64, macOS, and
  Linux remain compatibility-only, not public-quality platforms.
- Clean-machine acceptance: __ACCEPTANCE_STATE__
- Packaging or CI success alone does not establish clean-machine acceptance.
  The exact installer hash must match the artifact used for acceptance.
- Semantic Review and Translation require a configured model endpoint; local
  editing and inspection remain available without one. Hosted endpoints receive
  only the source content disclosed and confirmed for the operation, plus
  protocol framing.
- PlantUML rendering requires a local PlantUML/Java installation. Mermaid, D2,
  math fonts, and the sample are embedded in release builds.

The [Product Contract](https://github.com/wxxb789/markturbo/blob/v__VERSION__/PRODUCT.md)
defines the product scope; the [release contract](https://github.com/wxxb789/markturbo/blob/v__VERSION__/docs/goals/10-ship-primary-platform-release.md)
defines the acceptance gates. A pending acceptance status is not a claim that
this release passed them.
