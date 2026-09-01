#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "scripts" / "install-linux.sh"
PACKAGE_SCRIPT = ROOT / "scripts" / "package-release.sh"
DESKTOP_TEMPLATE = (
    ROOT
    / "crates"
    / "mt-app"
    / "resources"
    / "linux"
    / "io.github.wxxb789.markturbo.desktop.in"
)
ICON = ROOT / "crates" / "mt-app" / "resources" / "icons" / "markturbo-512.png"


def bash() -> str:
    if os.name == "nt":
        candidate = Path(r"C:\\Program Files\\Git\\bin\\bash.exe")
        if candidate.exists():
            return str(candidate)
    candidate = shutil.which("bash")
    if candidate:
        return candidate
    raise unittest.SkipTest("bash is required to test release packaging")


def shell_path(path: Path) -> str:
    if os.name != "nt":
        return str(path)
    drive = path.drive.rstrip(":").lower()
    if not drive:
        raise AssertionError(f"expected an absolute Windows path: {path}")
    return f"/{drive}{path.as_posix()[2:]}"


def run_bash(*args: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [bash(), *args],
        check=False,
        capture_output=True,
        text=True,
        env=env,
    )


def write_mv_wrapper(directory: Path, mode: str) -> None:
    real_mv = run_bash("-c", "command -v mv").stdout.strip()
    if not real_mv:
        raise AssertionError("mv is required to test release publication")

    if mode == "fail-desktop":
        trigger = '''
if [[ "$1" == */.markturbo-install.*/io.github.wxxb789.markturbo.desktop && "$2" == */applications/io.github.wxxb789.markturbo.desktop ]]; then
  echo "error: injected mv failure" >&2
  exit 1
fi
'''
    elif mode == "term-after-app":
        trigger = f'''
if [[ "$1" == */.markturbo-install.*/app && "$2" == */markturbo/app ]]; then
  "{real_mv}" "$@"
  printf 'term\n' > "${{MARKTURBO_TEST_MARKER:?}}"
  kill -TERM "$PPID"
  exit 0
fi
'''
    else:
        raise AssertionError(f"unknown mv wrapper mode: {mode}")

    directory.mkdir()
    wrapper = directory / "mv"
    wrapper.write_text(f"#!/usr/bin/env bash\n{trigger}exec \"{real_mv}\" \"$@\"\n", encoding="utf-8")
    wrapper.chmod(0o755)


def run_with_mv_wrapper(
    directory: Path, installer: Path, environment: dict[str, str]
) -> subprocess.CompletedProcess[str]:
    return run_bash(
        "-c",
        'PATH="$1:$PATH"; export PATH; exec bash "$2"',
        "--",
        shell_path(directory),
        shell_path(installer),
        env=environment,
    )


def write_release_binary(path: Path, prefix: str = "") -> None:
    path.write_text(f"#!/usr/bin/env bash\nprintf '{prefix}%s\\n' \"$*\"\n", encoding="utf-8")
    path.chmod(0o755)


def create_linux_release(root: Path, binary_prefix: str = "") -> Path:
    release = root / "release"
    (release / "scripts").mkdir(parents=True)
    (release / "fonts").mkdir()
    (release / "sample").mkdir()
    (release / "docs").mkdir()
    (release / "share" / "applications").mkdir(parents=True)
    (release / "share" / "icons" / "hicolor" / "512x512" / "apps").mkdir(parents=True)

    shutil.copy2(INSTALLER, release / "scripts" / "install-linux.sh")
    shutil.copy2(DESKTOP_TEMPLATE, release / "share" / "applications")
    shutil.copy2(
        ICON,
        release / "share" / "icons" / "hicolor" / "512x512" / "apps" / "io.github.wxxb789.markturbo.png",
    )
    write_release_binary(release / "markturbo", binary_prefix)
    (release / "fonts" / "KaTeX_Main-Regular.ttf").write_text("font", encoding="utf-8")
    (release / "sample" / "README.md").write_text("sample", encoding="utf-8")
    (release / "docs" / "platforms.md").write_text("docs", encoding="utf-8")
    return release


def create_macos_packaging_fixture(root: Path) -> Path:
    script = root / "scripts" / "package-release.sh"
    script.parent.mkdir(parents=True)
    shutil.copy2(PACKAGE_SCRIPT, script)

    (root / "Cargo.toml").write_text('version = "1.2.3"\n', encoding="utf-8")
    (root / "README.md").write_text("readme\n", encoding="utf-8")
    (root / "LICENSE").write_text("license\n", encoding="utf-8")
    (root / "docs").mkdir()
    (root / "docs" / "architecture.md").write_text("architecture\n", encoding="utf-8")
    (root / "docs" / "platforms.md").write_text("platforms\n", encoding="utf-8")
    (root / "sample").mkdir()
    (root / "sample" / "README.md").write_text("sample\n", encoding="utf-8")

    icons = root / "crates" / "mt-app" / "resources" / "icons"
    macos = root / "crates" / "mt-app" / "resources" / "macos"
    icons.mkdir(parents=True)
    macos.mkdir(parents=True)
    (icons / "markturbo.icns").write_text("icon\n", encoding="utf-8")
    (macos / "Info.plist.in").write_text(
        "<plist><dict><string>@SHORT_VERSION@</string><string>@BUNDLE_VERSION@</string></dict></plist>\n",
        encoding="utf-8",
    )

    fonts = root / "fonts" / "katex"
    fonts.mkdir(parents=True)
    for index in range(19):
        (fonts / f"KaTeX_{index}.ttf").write_text("font\n", encoding="utf-8")
    (fonts / "LICENSE.md").write_text("font license\n", encoding="utf-8")

    binary = root / "target" / "release" / "markturbo"
    binary.parent.mkdir(parents=True)
    write_release_binary(binary, "markturbo ")
    return script


class PlatformPackagingTests(unittest.TestCase):
    def test_linux_installer_installs_a_discoverable_user_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            release = create_linux_release(root)
            data_home = root / "xdg % & $` data"

            sentinel = data_home / "markturbo" / "user-state.txt"
            sentinel.parent.mkdir(parents=True)
            sentinel.write_text("keep me", encoding="utf-8")

            environment = os.environ.copy()
            environment.update(
                {
                    "HOME": shell_path(root / "home"),
                    "XDG_DATA_HOME": shell_path(data_home),
                }
            )
            result = run_bash(shell_path(release / "scripts" / "install-linux.sh"), env=environment)
            self.assertEqual(result.returncode, 0, result.stderr)

            install_root = data_home / "markturbo" / "app"
            binary = install_root / "markturbo"
            desktop_file = data_home / "applications" / "io.github.wxxb789.markturbo.desktop"
            with self.subTest("first install preserves user state"):
                self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep me")
            with self.subTest("installs the release payload under app"):
                self.assertTrue(binary.is_file())
                self.assertTrue((install_root / "fonts" / "KaTeX_Main-Regular.ttf").is_file())
                self.assertTrue((install_root / "sample" / "README.md").is_file())
                self.assertTrue((install_root / "docs" / "platforms.md").is_file())
            self.assertTrue(
                (data_home / "icons" / "hicolor" / "512x512" / "apps" / "io.github.wxxb789.markturbo.png").is_file()
            )

            desktop_entry = desktop_file.read_text(encoding="utf-8")
            with self.subTest("desktop Exec preserves metacharacters"):
                exec_line = next(line for line in desktop_entry.splitlines() if line.startswith("Exec="))
                self.assertTrue(
                    exec_line.endswith('xdg %% & \\$\\` data/markturbo/app/markturbo" %F'), exec_line
                )
            self.assertNotIn("Exec=markturbo %F", desktop_entry)

            launch = run_bash(shell_path(binary), "sample", env=environment)
            self.assertEqual(launch.returncode, 0, launch.stderr)
            self.assertEqual(launch.stdout.strip(), "sample")

            reinstall = run_bash(shell_path(release / "scripts" / "install-linux.sh"), env=environment)
            self.assertEqual(reinstall.returncode, 0, reinstall.stderr)
            with self.subTest("reinstall preserves user state"):
                self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep me")

    def test_linux_installer_rolls_back_an_incomplete_upgrade(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            release = create_linux_release(root, "old:")
            data_home = root / "xdg % & $` data"
            icon = release / "share" / "icons" / "hicolor" / "512x512" / "apps" / "io.github.wxxb789.markturbo.png"
            binary_source = release / "markturbo"

            environment = os.environ.copy()
            environment.update(
                {
                    "HOME": shell_path(root / "home"),
                    "XDG_DATA_HOME": shell_path(data_home),
                }
            )
            first_install = run_bash(shell_path(release / "scripts" / "install-linux.sh"), env=environment)
            self.assertEqual(first_install.returncode, 0, first_install.stderr)

            installer = release / "scripts" / "install-linux.sh"
            install_root = data_home / "markturbo" / "app"
            binary = install_root / "markturbo"
            desktop = data_home / "applications" / "io.github.wxxb789.markturbo.desktop"
            installed_icon = data_home / "icons" / "hicolor" / "512x512" / "apps" / "io.github.wxxb789.markturbo.png"
            previous_binary = binary.read_bytes()
            previous_desktop = desktop.read_bytes()
            previous_icon = installed_icon.read_bytes()

            write_release_binary(binary_source, "new:")
            desktop_template = release / "share" / "applications" / "io.github.wxxb789.markturbo.desktop.in"
            desktop_template.write_bytes(
                desktop_template.read_bytes().replace(b"Name=markturbo", b"Name=markturbo next")
            )
            icon.write_bytes(b"new icon")
            failure_tools = root / "failure-tools"
            write_mv_wrapper(failure_tools, "fail-desktop")
            failed_upgrade = run_with_mv_wrapper(failure_tools, installer, environment)

            with self.subTest("a failed publication rolls back"):
                self.assertNotEqual(failed_upgrade.returncode, 0)
                self.assertIn("injected mv failure", failed_upgrade.stderr)
            with self.subTest("the old app remains installed"):
                self.assertEqual(binary.read_bytes(), previous_binary)
            with self.subTest("the old desktop entry remains installed"):
                self.assertEqual(desktop.read_bytes(), previous_desktop)
            with self.subTest("the old icon remains installed"):
                self.assertEqual(installed_icon.read_bytes(), previous_icon)

            signal_tools = root / "signal-tools"
            write_mv_wrapper(signal_tools, "term-after-app")
            signal_marker = root / "term-marker"
            environment["MARKTURBO_TEST_MARKER"] = shell_path(signal_marker)
            interrupted_upgrade = run_with_mv_wrapper(signal_tools, installer, environment)
            environment.pop("MARKTURBO_TEST_MARKER")
            with self.subTest("an interrupted publication rolls back"):
                self.assertNotEqual(interrupted_upgrade.returncode, 0)
                self.assertEqual(signal_marker.read_text(encoding="utf-8"), "term\n")
            with self.subTest("the old app survives interruption"):
                self.assertEqual(binary.read_bytes(), previous_binary)
            with self.subTest("the old desktop entry survives interruption"):
                self.assertEqual(desktop.read_bytes(), previous_desktop)
            with self.subTest("the old icon survives interruption"):
                self.assertEqual(installed_icon.read_bytes(), previous_icon)

            launch = run_bash(shell_path(binary), "sample", env=environment)
            self.assertEqual(launch.returncode, 0, launch.stderr)
            self.assertEqual(launch.stdout.strip(), "old:sample")

    def test_linux_installer_rejects_an_existing_transaction_lock(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            release = create_linux_release(root, "old:")
            data_home = root / "xdg % & $` data"
            environment = os.environ.copy()
            environment.update(
                {
                    "HOME": shell_path(root / "home"),
                    "XDG_DATA_HOME": shell_path(data_home),
                }
            )
            installer = release / "scripts" / "install-linux.sh"
            first_install = run_bash(shell_path(installer), env=environment)
            self.assertEqual(first_install.returncode, 0, first_install.stderr)

            binary = data_home / "markturbo" / "app" / "markturbo"
            desktop = data_home / "applications" / "io.github.wxxb789.markturbo.desktop"
            icon = data_home / "icons" / "hicolor" / "512x512" / "apps" / "io.github.wxxb789.markturbo.png"
            previous_binary = binary.read_bytes()
            previous_desktop = desktop.read_bytes()
            previous_icon = icon.read_bytes()
            write_release_binary(release / "markturbo", "new:")
            lock = data_home / ".markturbo-install.lock"
            lock.mkdir()

            contended = run_bash(shell_path(installer), env=environment)
            self.assertNotEqual(contended.returncode, 0)
            self.assertIn("another markturbo installation is already running", contended.stderr)
            self.assertTrue(lock.is_dir())
            self.assertEqual(binary.read_bytes(), previous_binary)
            self.assertEqual(desktop.read_bytes(), previous_desktop)
            self.assertEqual(icon.read_bytes(), previous_icon)

    def test_macos_bundle_versions_follow_apple_prerelease_syntax(self) -> None:
        expected = {
            "1.2.3": ("1.2.3", "1.2.3"),
            "1.2.3-alpha.4": ("1.2.3", "1.2.3a4"),
            "1.2.3-beta.5": ("1.2.3", "1.2.3b5"),
            "1.2.3-rc.6": ("1.2.3", "1.2.3fc6"),
        }
        source = shell_path(PACKAGE_SCRIPT)
        for version, fields in expected.items():
            with self.subTest(version=version):
                result = run_bash(
                    "-c",
                    'source "$1"; macos_version_fields "$2"',
                    "--",
                    source,
                    version,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(tuple(result.stdout.strip().split("\t")), fields)

        for version in ("1.2.3-preview.1", "1.2.3-alpha.0", "1.2.3-alpha.256"):
            with self.subTest(version=version):
                invalid = run_bash(
                    "-c",
                    'source "$1"; macos_version_fields "$2"',
                    "--",
                    source,
                    version,
                )
                self.assertNotEqual(invalid.returncode, 0)
                self.assertIn("unsupported macOS bundle version", invalid.stderr)

    def test_macos_bundle_locates_the_sample_beside_the_executable(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            script = create_macos_packaging_fixture(root)
            tools = root / "tools"
            tools.mkdir()
            (tools / "cargo").write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
            (tools / "rustc").write_text(
                "#!/usr/bin/env bash\nprintf 'host: aarch64-apple-darwin\\n'\n",
                encoding="utf-8",
            )
            (tools / "plutil").write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
            (tools / "ln").write_text(
                "#!/usr/bin/env bash\nprintf '%s\\t%s\\t%s\\n' \"$1\" \"$2\" \"$3\" >> \"${MARKTURBO_TEST_LN_LOG:?}\"\n",
                encoding="utf-8",
            )
            for tool in tools.iterdir():
                tool.chmod(0o755)

            link_log = root / "link-log"
            environment = os.environ.copy()
            environment["MARKTURBO_TEST_LN_LOG"] = shell_path(link_log)
            result = run_bash(
                "-c",
                'PATH="$1:$PATH"; export PATH; exec bash "$2"',
                "--",
                shell_path(tools),
                shell_path(script),
                env=environment,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

            release = root / "dist" / "markturbo-1.2.3-aarch64-apple-darwin"
            bundled_sample = release / "markturbo.app" / "Contents" / "Resources" / "sample"
            executable_sample = release / "markturbo.app" / "Contents" / "MacOS" / "sample"
            cli_sample = release / "sample"
            link_calls = {
                tuple(line.split("\t")) for line in link_log.read_text(encoding="utf-8").splitlines()
            }

            with self.subTest("stores one sample copy in the app bundle"):
                self.assertEqual((bundled_sample / "README.md").read_text(encoding="utf-8"), "sample\n")
            with self.subTest("runtime lookup follows a relative link beside the executable"):
                self.assertIn(
                    ("-s", "../Resources/sample", shell_path(executable_sample)),
                    link_calls,
                )
            with self.subTest("CLI and documentation retain a relative root sample path"):
                self.assertIn(
                    ("-s", "markturbo.app/Contents/Resources/sample", shell_path(cli_sample)),
                    link_calls,
                )


if __name__ == "__main__":
    unittest.main()
