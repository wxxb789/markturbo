#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["ruamel.yaml>=0.18,<0.19"]
# ///

import tomllib
import unittest
from pathlib import Path

from ruamel.yaml import YAML


ROOT = Path(__file__).resolve().parents[1]


def load_workflow(path: Path) -> dict:
    yaml = YAML(typ="safe")
    data = yaml.load(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise AssertionError(f"workflow {path} must be a mapping")
    return data


def step_named(steps: list[dict], name: str) -> dict:
    for step in steps:
        if step.get("name") == name:
            return step
    raise AssertionError(f"missing workflow step: {name}")


class ReleaseAutomationContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.config = tomllib.loads((ROOT / "release.toml").read_text(encoding="utf-8"))
        cls.bump = load_workflow(ROOT / ".github" / "workflows" / "version-bump.yml")
        cls.release = load_workflow(ROOT / ".github" / "workflows" / "release.yml")
        cls.pull_request = load_workflow(ROOT / ".github" / "workflows" / "pull-request.yml")

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

    def test_version_bump_uses_pinned_cargo_release_and_defaults_to_patch(self) -> None:
        bump_input = self.bump["on"]["workflow_dispatch"]["inputs"]["bump"]
        self.assertEqual(bump_input["default"], "patch")
        self.assertEqual(bump_input["options"], ["patch", "minor", "major", "alpha", "beta", "rc", "release"])
        self.assertEqual(self.bump["env"]["CARGO_RELEASE_VERSION"], "1.1.5")

        install = step_named(self.bump["jobs"]["bump"]["steps"], "Install cargo-release")
        self.assertIn('cargo install cargo-release --version "$CARGO_RELEASE_VERSION" --locked', install["run"])

        version = step_named(self.bump["jobs"]["bump"]["steps"], "Bump workspace version")
        self.assertEqual(version["env"]["BUMP"], "${{ inputs.bump }}")
        self.assertIn('cargo release version "$BUMP" --workspace --execute --no-confirm', version["run"])

    def test_version_bump_pushes_atomically_and_calls_release(self) -> None:
        commit = step_named(self.bump["jobs"]["bump"]["steps"], "Commit and tag")
        self.assertIn("git push --atomic", commit["run"])
        release_job = self.bump["jobs"]["release"]
        self.assertEqual(release_job["needs"], "bump")
        self.assertEqual(release_job["uses"], "./.github/workflows/release.yml")
        self.assertEqual(release_job["with"]["tag"], "${{ needs.bump.outputs.tag }}")
        self.assertFalse((ROOT / "scripts" / "bump-version.py").exists())

    def test_release_accepts_tags_and_reusable_calls(self) -> None:
        triggers = self.release["on"]
        self.assertEqual(triggers["push"]["tags"], ["v*"])
        self.assertEqual(triggers["workflow_call"]["inputs"]["tag"], {
            "description": "Existing version tag to release",
            "required": True,
            "type": "string",
        })
        self.assertEqual(triggers["workflow_dispatch"]["inputs"]["tag"], {
            "description": "Existing version tag to release",
            "required": True,
            "type": "string",
        })

    def test_release_tag_must_be_reachable_from_the_default_branch(self) -> None:
        contract_steps = self.release["jobs"]["contract"]["steps"]
        checkout = contract_steps[0]
        self.assertEqual(checkout["uses"], "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1")
        self.assertEqual(checkout["with"]["ref"], "${{ env.RELEASE_TAG }}")
        self.assertEqual(checkout["with"]["fetch-depth"], 0)

        ancestry = step_named(contract_steps, "Verify tag belongs to the default branch")
        self.assertEqual(
            ancestry["env"]["DEFAULT_BRANCH"],
            "${{ github.event.repository.default_branch }}",
        )
        self.assertIn(
            'git fetch --no-tags origin "+refs/heads/$DEFAULT_BRANCH:refs/remotes/origin/$DEFAULT_BRANCH"',
            ancestry["run"],
        )
        self.assertIn('git rev-parse --verify "$RELEASE_TAG^{commit}"', ancestry["run"])
        self.assertIn(
            'git merge-base --is-ancestor "$tag_commit" "origin/$DEFAULT_BRANCH"',
            ancestry["run"],
        )

    def test_release_tests_and_packages_every_supported_desktop_platform(self) -> None:
        build = self.release["jobs"]["build"]
        self.assertEqual(
            build["strategy"]["matrix"]["include"],
            [
                {"runner": "ubuntu-latest", "artifact": "linux"},
                {"runner": "macos-latest", "artifact": "macos"},
                {"runner": "windows-latest", "artifact": "windows"},
            ],
        )
        build_steps = build["steps"]
        self.assertEqual(step_named(build_steps, "Lint")["run"], "cargo clippy --workspace --all-targets")
        self.assertEqual(step_named(build_steps, "Test")["run"], "cargo test --release --workspace --locked")
        self.assertEqual(
            step_named(build_steps, "Package")["run"],
            "bash ./scripts/package-release.sh",
        )

    def test_pull_request_validation_preserves_the_release_build_contract(self) -> None:
        self.assertEqual(self.pull_request["on"], {"pull_request": {}})
        self.assertEqual(
            self.pull_request["concurrency"],
            {
                "group": "pull-request-${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}",
                "cancel-in-progress": True,
            },
        )
        self.assertEqual(self.pull_request["permissions"], {"contents": "read"})

        validation = self.pull_request["jobs"]["validate"]
        release_build = self.release["jobs"]["build"]
        self.assertEqual(
            validation["strategy"],
            {
                "fail-fast": False,
                "matrix": {
                    "include": [
                        {"runner": "ubuntu-latest"},
                        {"runner": "macos-latest"},
                        {"runner": "windows-latest"},
                    ],
                },
            },
        )
        self.assertEqual(
            [entry["runner"] for entry in validation["strategy"]["matrix"]["include"]],
            [entry["runner"] for entry in release_build["strategy"]["matrix"]["include"]],
        )
        self.assertEqual(validation["runs-on"], "${{ matrix.runner }}")
        self.assertEqual(validation["defaults"]["run"]["shell"], "bash")
        self.assertNotIn("permissions", validation)

        validation_steps = validation["steps"]
        release_steps = release_build["steps"]
        checkout = validation_steps[0]
        self.assertEqual(checkout["uses"], "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1")
        self.assertEqual(checkout["with"], {"persist-credentials": False})

        setup_uv = validation_steps[1]
        self.assertEqual(setup_uv["uses"], "astral-sh/setup-uv@ae62891fec2bb8e7d6c99fc78c9fec3a63790f8d")
        self.assertEqual(setup_uv["with"], {"enable-cache": False})

        for step_name in ["Install Linux build dependencies", "Install Rust", "Lint", "Test", "Package"]:
            self.assertEqual(
                step_named(validation_steps, step_name),
                step_named(release_steps, step_name),
            )

        self.assertEqual(
            step_named(validation_steps, "Install Rust")["run"],
            "rustup toolchain install stable --profile minimal --no-self-update\n"
            "rustup default stable\n"
            "rustup component add clippy\n",
        )
        self.assertEqual(step_named(validation_steps, "Format")["run"], "cargo fmt --all -- --check")
        self.assertEqual(
            step_named(validation_steps, "Verify release automation")["run"],
            "uv run scripts/test_release_automation.py",
        )
        self.assertEqual(
            step_named(validation_steps, "Test generated app icons")["run"],
            "uv run scripts/test_generate_app_icons.py",
        )
        self.assertEqual(
            step_named(validation_steps, "Test platform packaging")["run"],
            "uv run scripts/test_platform_packaging.py",
        )

    def test_semver_prereleases_become_github_prereleases(self) -> None:
        publish = self.release["jobs"]["publish"]
        release_step = step_named(publish["steps"], "Publish GitHub release")
        self.assertEqual(release_step["env"]["GH_REPO"], "${{ github.repository }}")
        self.assertIn('VERSION="${TAG#v}"', release_step["run"])
        self.assertIn('"$VERSION" == *-*', release_step["run"])
        self.assertIn("--prerelease", release_step["run"])
        self.assertIn("gh release create", release_step["run"])

    def test_existing_draft_release_is_published_after_assets_upload(self) -> None:
        publish = self.release["jobs"]["publish"]
        release_step = step_named(publish["steps"], "Publish GitHub release")
        run = release_step["run"]

        self.assertIn('gh release view "$TAG" --json isDraft --jq .isDraft', run)
        self.assertIn('gh release upload "$TAG" dist/* --clobber', run)
        self.assertIn('gh release edit "$TAG" --draft=false', run)
        self.assertLess(
            run.index('gh release upload "$TAG" dist/* --clobber'),
            run.index('gh release edit "$TAG" --draft=false'),
        )


if __name__ == "__main__":
    unittest.main()
