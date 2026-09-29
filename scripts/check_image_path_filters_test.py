import io
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

import check_image_path_filters


def github_workflow(paths: set[str]) -> str:
    entries = "\n".join(f'      - "{path}"' for path in sorted(paths))
    return (
        "name: Docker image\n"
        "on:\n"
        "  push:\n"
        "    paths:\n"
        f"{entries}\n"
        "  pull_request:\n"
        "    paths:\n"
        f"{entries}\n"
    )


def woodpecker_workflow(paths: set[str]) -> str:
    entries = "\n".join(f'      - "{path}"' for path in sorted(paths))
    return f"when:\n  path:\n    include:\n{entries}\nsteps:\n  build:\n"


class CheckImagePathFiltersTests(unittest.TestCase):
    def setUp(self):
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary_directory.name)
        self.script_path = self.root / "scripts" / "check_image_path_filters.py"
        self.script_path.parent.mkdir()
        self.github_path = self.root / ".github" / "workflows" / "docker-publish.yml"
        self.github_path.parent.mkdir(parents=True)
        self.woodpecker_path = self.root / ".woodpecker.yml"

    def tearDown(self):
        self.temporary_directory.cleanup()

    def run_main(self, github_paths: set[str], woodpecker_paths: set[str]):
        self.github_path.write_text(github_workflow(github_paths), encoding="utf-8")
        self.woodpecker_path.write_text(
            woodpecker_workflow(woodpecker_paths), encoding="utf-8"
        )
        output = io.StringIO()
        with patch.object(check_image_path_filters, "__file__", str(self.script_path)):
            with redirect_stdout(output):
                result = check_image_path_filters.main()
        return result, output.getvalue()

    def test_quoted_values_accepts_single_and_double_quotes_and_ignores_other_lines(
        self,
    ):
        lines = [
            '  - ".dockerignore" # generated image input',
            "  - 'Dockerfile'",
            "  - unquoted.yml",
            "    include:",
        ]
        self.assertEqual(
            check_image_path_filters._quoted_values(lines),
            {".dockerignore", "Dockerfile"},
        )

    def test_github_paths_collects_each_paths_filter(self):
        text = github_workflow({".dockerignore", "Dockerfile"})
        text += '    paths-ignore:\n      - "ignored.txt"\n'
        self.assertEqual(
            check_image_path_filters._github_paths(text),
            {".dockerignore", "Dockerfile"},
        )

    def test_woodpecker_paths_reads_the_include_block(self):
        self.assertEqual(
            check_image_path_filters._woodpecker_paths(
                woodpecker_workflow({".dockerignore", "Dockerfile"})
            ),
            {".dockerignore", "Dockerfile"},
        )

    def test_woodpecker_paths_requires_a_filter(self):
        with self.assertRaisesRegex(ValueError, "no path filter"):
            check_image_path_filters._woodpecker_paths("when:\n  event: push\n")

    def test_main_accepts_matching_repository_filters(self):
        result, output = self.run_main(
            check_image_path_filters.EXPECTED_PATHS,
            check_image_path_filters.EXPECTED_PATHS,
        )
        self.assertEqual(result, 0)
        self.assertEqual(output, "Image-producing path filters are aligned.\n")

    def test_main_reports_different_filters(self):
        github_paths = check_image_path_filters.EXPECTED_PATHS - {"Dockerfile"}
        result, output = self.run_main(
            github_paths,
            check_image_path_filters.EXPECTED_PATHS,
        )
        self.assertEqual(result, 1)
        self.assertIn("GitHub Actions image paths differ from the expected set", output)
        self.assertIn("GitHub Actions and Woodpecker image paths differ", output)


if __name__ == "__main__":
    unittest.main()
