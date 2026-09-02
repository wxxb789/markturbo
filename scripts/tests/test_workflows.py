import tomllib
import unittest
from pathlib import Path

from ruamel.yaml import YAML


ROOT = Path(__file__).resolve().parents[2]
CHECKOUT = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"
SETUP_UV = "astral-sh/setup-uv@ae62891fec2bb8e7d6c99fc78c9fec3a63790f8d"
RUST_CACHE = "Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6"


def load_workflow(name: str) -> dict:
    yaml = YAML(typ="safe")
    workflow = yaml.load(
        (ROOT / ".github" / "workflows" / name).read_text(encoding="utf-8")
    )
    if not isinstance(workflow, dict):
        raise AssertionError(f"workflow {name} must be a mapping")
    return workflow


def step_named(steps: list[dict], name: str) -> dict:
    for step in steps:
        if step.get("name") == name:
            return step
    raise AssertionError(f"missing workflow step: {name}")


def step_using(steps: list[dict], uses: str) -> dict:
    for step in steps:
        if step.get("uses") == uses:
            return step
    raise AssertionError(f"missing workflow action: {uses}")


class WorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.config = tomllib.loads((ROOT / "release.toml").read_text(encoding="utf-8"))
        cls.pull_request = load_workflow("pull-request.yml")
        cls.release = load_workflow("release.yml")
        cls.bump = load_workflow("version-bump.yml")

    def test_cargo_release_only_owns_workspace_versioning(self) -> None:
        self.assertEqual(
            self.config,
            {
                "shared-version": True,
                "publish": False,
                "tag": False,
                "push": False,
            },
        )

    def test_pr_runs_the_canonical_quality_check_once(self) -> None:
        self.assertEqual(self.pull_request["on"], {"pull_request": {}})
        self.assertEqual(set(self.pull_request["jobs"]), {"quality", "release-tests"})

        quality = self.pull_request["jobs"]["quality"]
        self.assertEqual(quality["runs-on"], "ubuntu-latest")
        steps = quality["steps"]
        checkout = steps[0]
        self.assertEqual(checkout["uses"], CHECKOUT)
        self.assertEqual(
            checkout["with"],
            {
                "persist-credentials": False,
                "ref": "${{ github.event.pull_request.head.sha }}",
                "fetch-depth": 1,
            },
        )
        self.assert_uv_cache(step_using(steps, SETUP_UV))
        self.assert_rust_cache(steps)

        prepare = step_named(steps, "Prepare check range")
        self.assertEqual(
            prepare["env"],
            {
                "BASE_SHA": "${{ github.event.pull_request.base.sha }}",
                "HEAD_SHA": "${{ github.event.pull_request.head.sha }}",
            },
        )
        prepare_run = prepare["run"]
        self.assertIn('git fetch --no-tags --depth=1 origin "$BASE_SHA"', prepare_run)
        self.assertIn('git cat-file -e "$BASE_SHA^{commit}"', prepare_run)
        self.assertIn('test "$(git rev-parse HEAD)" = "$HEAD_SHA"', prepare_run)
        self.assertNotIn("git read-tree", prepare_run)
        check = step_named(steps, "Check")
        self.assertEqual(check["env"], prepare["env"])
        self.assertEqual(
            check["run"],
            "uv run --project scripts scripts/mt.py check ci",
        )
        self.assertEqual(
            [step["name"] for step in steps if "run" in step and step["name"] != "Check"],
            [
                "Install Rust",
                "Install Linux build dependencies",
                "Prepare check range",
            ],
        )

    def test_pr_cross_platform_jobs_only_run_locked_release_tests(self) -> None:
        release_tests = self.pull_request["jobs"]["release-tests"]
        self.assertEqual(
            release_tests["strategy"]["matrix"]["include"],
            [
                {"runner": "macos-latest", "cache": "macos"},
                {"runner": "windows-2022", "cache": "windows"},
            ],
        )
        self.assertEqual(release_tests["runs-on"], "${{ matrix.runner }}")
        steps = release_tests["steps"]
        checkout = steps[0]
        self.assertEqual(
            checkout["with"],
            {
                "persist-credentials": False,
                "ref": "${{ github.event.pull_request.head.sha }}",
                "fetch-depth": 1,
            },
        )
        self.assert_rust_cache(steps)
        self.assertEqual(
            step_named(steps, "Test")["run"],
            "cargo test --locked --release --workspace",
        )
        self.assertEqual(
            [step["name"] for step in steps if "run" in step],
            ["Install Rust", "Test"],
        )
        self.assertFalse(any(step.get("uses") == SETUP_UV for step in steps))

    def test_release_validates_tags_then_publishes_one_windows_executable(self) -> None:
        self.assertEqual(set(self.release["on"]), {"push", "workflow_call", "workflow_dispatch"})
        self.assertEqual(self.release["on"]["push"]["tags"], ["v*"])
        expected_tag_input = {
            "description": "Existing version tag to release",
            "required": True,
            "type": "string",
        }
        self.assertEqual(
            self.release["on"]["workflow_call"]["inputs"]["tag"],
            expected_tag_input,
        )
        self.assertEqual(
            self.release["on"]["workflow_dispatch"]["inputs"]["tag"],
            expected_tag_input,
        )
        self.assertEqual(self.release["env"]["RELEASE_ASSET"], "markturbo-windows-x64.exe")

        contract = self.release["jobs"]["contract"]
        checkout = contract["steps"][0]
        self.assertEqual(checkout["uses"], CHECKOUT)
        self.assertEqual(checkout["with"]["ref"], "${{ env.RELEASE_TAG }}")
        self.assertEqual(checkout["with"]["fetch-depth"], 0)
        ancestry = step_named(contract["steps"], "Verify tag belongs to the default branch")["run"]
        self.assertIn('git merge-base --is-ancestor "$tag_commit" "origin/$DEFAULT_BRANCH"', ancestry)
        self.assertFalse(any(step.get("uses") == SETUP_UV for step in contract["steps"]))
        self.assertEqual(
            [step["name"] for step in contract["steps"] if "run" in step],
            ["Verify tag belongs to the default branch", "Verify tag version"],
        )
        version = step_named(contract["steps"], "Verify tag version")["run"]
        self.assertIn("import tomllib", version)
        self.assertIn('["workspace"]["package"]["version"]', version)
        self.assertNotIn("sed -n", version)

        build = self.release["jobs"]["build"]
        self.assertEqual(build["runs-on"], "windows-2022")
        self.assertEqual(build["defaults"]["run"]["shell"], "pwsh")
        build_steps = build["steps"]
        self.assert_uv_cache(step_using(build_steps, SETUP_UV))
        self.assertEqual(
            step_named(build_steps, "Check")["run"],
            "uv run --project scripts scripts/mt.py check full",
        )
        self.assertFalse(
            any(
                step.get("name") in {"Format", "Lint", "Test", "Build executable"}
                for step in build_steps
            )
        )
        self.assert_rust_cache(build_steps)

        upload = build_steps[-1]
        self.assertEqual(upload["uses"], "actions/upload-artifact@bbbca2ddaa5d8feaa63e36b76fdaad77386f024f")
        self.assertEqual(upload["with"]["path"], "release/${{ env.RELEASE_ASSET }}")
        self.assertEqual(upload["with"]["compression-level"], 0)
        self.assertNotIn("*", upload["with"]["path"])

        publish = self.release["jobs"]["publish"]
        download = publish["steps"][0]
        self.assertEqual(download["with"]["name"], "markturbo-windows-x64")
        publish_run = step_named(publish["steps"], "Publish executable")["run"]
        self.assertEqual(publish_run.count('gh release view "$TAG"'), 1)
        self.assertIn('gh release view "$TAG" --json isDraft,assets', publish_run)
        self.assertIn("jq -r '.isDraft'", publish_run)
        self.assertIn("jq -r '.assets[]?.name'", publish_run)
        self.assertIn('gh release delete-asset "$TAG" "$asset" --yes', publish_run)
        self.assertIn('"$asset" != "$RELEASE_ASSET"', publish_run)
        self.assertIn('gh release upload "$TAG" "$ASSET" --clobber', publish_run)
        self.assertLess(
            publish_run.index('gh release upload "$TAG" "$ASSET" --clobber'),
            publish_run.index('gh release delete-asset "$TAG" "$asset" --yes'),
        )
        self.assertIn('gh release create "$TAG" "$ASSET"', publish_run)
        self.assertNotIn("dist/*", publish_run)
        self.assertNotIn("*.zip", publish_run)
        self.assertNotIn("*.tar.gz", publish_run)

    def test_version_bump_calls_the_reusable_release_workflow(self) -> None:
        bump_input = self.bump["on"]["workflow_dispatch"]["inputs"]["bump"]
        self.assertEqual(bump_input["default"], "patch")
        self.assertEqual(bump_input["options"], ["patch", "minor", "major", "alpha", "beta", "rc", "release"])
        install = step_named(self.bump["jobs"]["bump"]["steps"], "Install cargo-release")["run"]
        self.assertIn('cargo install cargo-release --version "$CARGO_RELEASE_VERSION" --locked', install)
        self.assertEqual(
            step_named(self.bump["jobs"]["bump"]["steps"], "Test tooling")["run"],
            "uv run --project scripts scripts/mt.py check fast",
        )
        self.assert_uv_cache(step_using(self.bump["jobs"]["bump"]["steps"], SETUP_UV))
        self.assert_rust_cache(self.bump["jobs"]["bump"]["steps"])
        self.assertEqual(set(self.bump["jobs"]), {"bump", "release"})
        commit = step_named(self.bump["jobs"]["bump"]["steps"], "Commit and tag")["run"]
        self.assertIn('git push --atomic origin "HEAD:$DEFAULT_BRANCH" "refs/tags/$TAG"', commit)
        release_job = self.bump["jobs"]["release"]
        self.assertEqual(release_job["needs"], "bump")
        self.assertEqual(release_job["uses"], "./.github/workflows/release.yml")
        self.assertEqual(release_job["with"]["tag"], "${{ needs.bump.outputs.tag }}")

    def test_archive_packaging_is_absent_from_automation(self) -> None:
        for path in (
            ROOT / "scripts" / "package-release.sh",
            ROOT / "scripts" / "install-linux.sh",
            ROOT / "scripts" / "test_platform_packaging.py",
            ROOT / "scripts" / "test_release_automation.py",
        ):
            self.assertFalse(path.exists(), path)

    def assert_uv_cache(self, step: dict) -> None:
        self.assertEqual(step["uses"], SETUP_UV)
        self.assertEqual(
            step["with"],
            {
                "enable-cache": True,
                "cache-dependency-glob": "scripts/pyproject.toml\nscripts/uv.lock\n",
            },
        )

    def assert_rust_cache(self, steps: list[dict]) -> None:
        cache = next(step for step in steps if step.get("uses") == RUST_CACHE)
        self.assertIn("shared-key", cache["with"])


if __name__ == "__main__":
    unittest.main()
