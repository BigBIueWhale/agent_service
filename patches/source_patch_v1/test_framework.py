"""Failure-semantics tests for the transactional source patch framework."""

from __future__ import annotations

import ast
import os
import runpy
import sys
import tempfile
import unittest
from contextlib import nullcontext
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

from . import framework
from .framework import (
    FileIdentity,
    LandmarkEdit,
    PatchRefusedError,
    PatchSet,
    PatchStage,
    PatchWriteError,
    SourcePatchTransaction,
    require_python_symbols,
    sha256_bytes,
    sha256_text,
)


def _noop(_state) -> None:
    return None


def _review(path: str, before: str | None, after: str | None) -> str:
    old_text = before if before is not None else ""
    new_text = after if after is not None else ""
    old_lines = old_text.count("\n")
    new_lines = new_text.count("\n")
    old = "".join(f"-{line}" for line in old_text.splitlines(keepends=True))
    new = "".join(f"+{line}" for line in new_text.splitlines(keepends=True))
    old_path = f"a/{path}" if before is not None else "/dev/null"
    new_path = f"b/{path}" if after is not None else "/dev/null"
    return (
        f"diff --git a/{path} b/{path}\n"
        f"--- {old_path}\n"
        f"+++ {new_path}\n"
        f"@@ -{1 if old_lines else 0},{old_lines} "
        f"+{1 if new_lines else 0},{new_lines} @@\n"
        f"{old}{new}"
    )


def _stage(
    artifact_root: Path,
    *,
    name: str,
    transformations: tuple[tuple[str, str | None, str | None], ...],
    validate_before=_noop,
    validate_after=_noop,
) -> PatchStage:
    review_path = f"{name}.patch"
    review = "".join(
        _review(path, before, after) for path, before, after in transformations
    )
    (artifact_root / review_path).write_text(review, encoding="utf-8", newline="\n")
    return PatchStage(
        name=name,
        rationale="Synthetic defect used to prove transaction failure semantics.",
        removal_condition="Remove when this framework test is removed.",
        review_patch=review_path,
        review_sha256=sha256_bytes(review.encode("utf-8")),
        files=tuple(
            FileIdentity(
                path=path,
                before_sha256=sha256_text(before) if before is not None else None,
                after_sha256=sha256_text(after) if after is not None else None,
            )
            for path, before, after in transformations
        ),
        edits=tuple(
            LandmarkEdit(
                name=f"{name}:{path}",
                path=path,
                before=before if before is not None else "",
                after=after if after is not None else "",
                review_before=before if before is not None else "",
                review_after=after if after is not None else "",
            )
            for path, before, after in transformations
        ),
        validate_before=validate_before,
        validate_after=validate_after,
    )


def _patchset(
    stages: tuple[PatchStage, ...],
    *,
    final_files: dict[str, str | None],
    final_validator=_noop,
) -> PatchSet:
    return PatchSet(
        name="synthetic-transaction",
        source_revision="synthetic-revision",
        identity_files={"identity.txt": sha256_text("pinned-revision\n")},
        stages=stages,
        final_files=final_files,
        validate_final=final_validator,
    )


class SourcePatchTransactionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="source-patch-test-")
        root = Path(self.temporary.name)
        self.source = root / "source"
        self.artifact = root / "artifact"
        self.source.mkdir()
        self.artifact.mkdir()
        (self.source / "identity.txt").write_text("pinned-revision\n", encoding="utf-8")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_new_nested_sources_are_created_only_after_validation_and_are_idempotent(self) -> None:
        contents = "export const version = 1;\n"
        relative = "packages/core/generated/schema.ts"
        stage = _stage(self.artifact, name="generated-source", transformations=((relative, None, contents),))
        transaction = SourcePatchTransaction(self.source, self.artifact,
            _patchset((stage,), final_files={relative: sha256_text(contents)}))
        plan, result = transaction.plan()
        self.assertFalse((self.source / "packages").exists())
        transaction.commit(plan, result)
        self.assertEqual((self.source / relative).read_text(), contents)
        self.assertEqual(transaction.apply().state, "already-applied")

    def test_nested_source_commit_failure_removes_only_created_directories(self) -> None:
        existing = self.source / "packages"
        existing.mkdir()
        (existing / "sentinel").write_text("preserve\n")
        paths = ("packages/core/generated/a.ts", "packages/core/fixtures/b.ts")
        contents = "export const version = 1;\n"
        stage = _stage(self.artifact, name="nested-source-failure",
            transformations=tuple((path, None, contents) for path in paths))
        transaction = SourcePatchTransaction(self.source, self.artifact,
            _patchset((stage,), final_files={path: sha256_text(contents) for path in paths}))
        real_replace = os.replace
        replaced = []

        def fail_second(source, destination):
            replaced.append(str(destination))
            if len(replaced) == 2:
                raise OSError("second nested replacement failed")
            return real_replace(source, destination)

        with patch.object(framework.os, "replace", fail_second):
            with self.assertRaisesRegex(PatchWriteError, "second nested replacement failed"):
                transaction.apply()
        self.assertEqual(len(replaced), 2)
        self.assertEqual(list(existing.iterdir()), [existing / "sentinel"])
        self.assertEqual((existing / "sentinel").read_text(), "preserve\n")

    def test_new_nested_source_refuses_a_parent_symlink_without_outside_writes(self) -> None:
        outside = Path(self.temporary.name) / "outside"
        outside.mkdir()
        (outside / "sentinel").write_text("preserve\n")
        (self.source / "packages").symlink_to(outside, target_is_directory=True)
        path, contents = "packages/generated/schema.ts", "export const version = 1;\n"
        stage = _stage(self.artifact, name="unsafe-parent", transformations=((path, None, contents),))
        transaction = SourcePatchTransaction(self.source, self.artifact,
            _patchset((stage,), final_files={path: sha256_text(contents)}))
        with self.assertRaisesRegex(PatchWriteError, "path escapes source root"):
            transaction.apply()
        self.assertEqual(list(outside.iterdir()), [outside / "sentinel"])
        self.assertTrue((self.source / "packages").is_symlink())

    def test_apply_is_exact_idempotent_and_preserves_mode(self) -> None:
        before = "def value():\n    return 1\n"
        after = "def value():\n    return 2\n"
        target = self.source / "module.py"
        target.write_text(before, encoding="utf-8")
        target.chmod(0o750)

        def validate_final(state) -> None:
            require_python_symbols(
                state,
                "module.py",
                {"value": ()},
                label="synthetic final contract",
            )

        stage = _stage(
            self.artifact,
            name="change-value",
            transformations=(("module.py", before, after),),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset(
                (stage,),
                final_files={"module.py": sha256_text(after)},
                final_validator=validate_final,
            ),
        )

        first = transaction.apply()
        first_stat = target.stat()
        first_bytes = target.read_bytes()
        second = transaction.apply()
        second_stat = target.stat()

        self.assertEqual(first.state, "applied")
        self.assertEqual(second.state, "already-applied")
        self.assertEqual(second.changed_files, ())
        self.assertEqual(first_bytes, after.encode())
        self.assertEqual(first_stat.st_mode, second_stat.st_mode)
        self.assertEqual(first_stat.st_mtime_ns, second_stat.st_mtime_ns)
        self.assertEqual(first_stat.st_ctime_ns, second_stat.st_ctime_ns)
        self.assertEqual(first_stat.st_ino, second_stat.st_ino)
        self.assertEqual(first_stat.st_mode & 0o777, 0o750)

    def test_new_file_hunk_must_describe_and_create_complete_file(self) -> None:
        after = "def created():\n    return True\n"
        stage = _stage(
            self.artifact,
            name="create-file",
            transformations=(("created.py", None, after),),
        )
        patchset = _patchset((stage,), final_files={"created.py": sha256_text(after)})

        result = SourcePatchTransaction(self.source, self.artifact, patchset).apply()

        self.assertEqual(result.state, "applied")
        self.assertEqual(
            (self.source / "created.py").read_text(encoding="utf-8"), after
        )
        self.assertEqual((self.source / "created.py").stat().st_mode & 0o777, 0o644)

    def test_unknown_source_drift_refuses_without_writes(self) -> None:
        before = "def value():\n    return 1\n"
        after = "def value():\n    return 2\n"
        drift = "def value():\n    return 999\n"
        target = self.source / "module.py"
        target.write_text(drift, encoding="utf-8")
        stage = _stage(
            self.artifact,
            name="change-value",
            transformations=(("module.py", before, after),),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset((stage,), final_files={"module.py": sha256_text(after)}),
        )
        before_stat = target.stat()

        with self.assertRaisesRegex(
            PatchRefusedError, "neither wholly pristine nor wholly final"
        ):
            transaction.apply()

        after_stat = target.stat()
        self.assertEqual(target.read_text(encoding="utf-8"), drift)
        self.assertEqual(before_stat.st_mtime_ns, after_stat.st_mtime_ns)
        self.assertEqual(before_stat.st_ctime_ns, after_stat.st_ctime_ns)
        self.assertFalse(tuple(self.source.rglob("*.qwen-source-patch.*")))

    def test_exact_intermediate_state_refuses_without_finishing_it(self) -> None:
        first = "def value():\n    return 1\n"
        intermediate = "def value():\n    return 2\n"
        final = "def value():\n    return 3\n"
        target = self.source / "module.py"
        target.write_text(intermediate, encoding="utf-8")
        stage_one = _stage(
            self.artifact,
            name="stage-one",
            transformations=(("module.py", first, intermediate),),
        )
        stage_two = _stage(
            self.artifact,
            name="stage-two",
            transformations=(("module.py", intermediate, final),),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset(
                (stage_one, stage_two),
                final_files={"module.py": sha256_text(final)},
            ),
        )
        prior = target.stat()

        with self.assertRaisesRegex(PatchRefusedError, "module.py=intermediate"):
            transaction.apply()

        current = target.stat()
        self.assertEqual(target.read_text(encoding="utf-8"), intermediate)
        self.assertEqual(prior.st_mtime_ns, current.st_mtime_ns)
        self.assertEqual(prior.st_ctime_ns, current.st_ctime_ns)

    def test_review_artifact_drift_refuses_before_source_writes(self) -> None:
        before = "def value():\n    return 1\n"
        after = "def value():\n    return 2\n"
        target = self.source / "module.py"
        target.write_text(before, encoding="utf-8")
        stage = _stage(
            self.artifact,
            name="change-value",
            transformations=(("module.py", before, after),),
        )
        (self.artifact / stage.review_patch).write_text("drift\n", encoding="utf-8")
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset((stage,), final_files={"module.py": sha256_text(after)}),
        )
        prior = target.stat()

        with self.assertRaisesRegex(PatchRefusedError, "review diff SHA-256 drift"):
            transaction.apply()

        current = target.stat()
        self.assertEqual(target.read_text(encoding="utf-8"), before)
        self.assertEqual(prior.st_mtime_ns, current.st_mtime_ns)
        self.assertEqual(prior.st_ctime_ns, current.st_ctime_ns)

    def test_source_change_between_plan_and_commit_is_refused(self) -> None:
        before = "def value():\n    return 1\n"
        after = "def value():\n    return 2\n"
        target = self.source / "module.py"
        target.write_text(before, encoding="utf-8")
        stage = _stage(
            self.artifact,
            name="change-value",
            transformations=(("module.py", before, after),),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset((stage,), final_files={"module.py": sha256_text(after)}),
        )
        planned, result = transaction.plan()
        target.write_text("def value():\n    return 7\n", encoding="utf-8")

        with self.assertRaisesRegex(
            PatchRefusedError, "neither wholly pristine nor wholly final"
        ):
            transaction.commit(planned, result)

        self.assertIn("return 7", target.read_text(encoding="utf-8"))

    def test_second_replace_failure_rolls_back_first_file_and_modes(self) -> None:
        a_before = "def a():\n    return 1\n"
        a_after = "def a():\n    return 2\n"
        b_before = "def b():\n    return 1\n"
        b_after = "def b():\n    return 2\n"
        (self.source / "a.py").write_text(a_before, encoding="utf-8")
        (self.source / "b.py").write_text(b_before, encoding="utf-8")
        (self.source / "a.py").chmod(0o740)
        (self.source / "b.py").chmod(0o640)
        stage = _stage(
            self.artifact,
            name="two-files",
            transformations=(
                ("a.py", a_before, a_after),
                ("b.py", b_before, b_after),
            ),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset(
                (stage,),
                final_files={
                    "a.py": sha256_text(a_after),
                    "b.py": sha256_text(b_after),
                },
            ),
        )
        real_replace = os.replace

        def fail_second_patch_temp(src, dst) -> None:
            if Path(dst).name == "b.py" and ".qwen-source-patch." in Path(src).name:
                raise OSError("injected second replacement failure")
            real_replace(src, dst)

        with patch.object(framework.os, "replace", fail_second_patch_temp):
            with self.assertRaisesRegex(
                PatchWriteError, "caller must discard this disposable tree"
            ):
                transaction.apply()

        self.assertEqual((self.source / "a.py").read_text(), a_before)
        self.assertEqual((self.source / "b.py").read_text(), b_before)
        self.assertEqual((self.source / "a.py").stat().st_mode & 0o777, 0o740)
        self.assertEqual((self.source / "b.py").stat().st_mode & 0o777, 0o640)
        self.assertFalse(tuple(self.source.rglob("*.qwen-source-patch.*")))
        self.assertFalse(tuple(self.source.rglob("*.qwen-rollback.*")))

    def _deletion_transaction(self, before: str) -> SourcePatchTransaction:
        stage = _stage(
            self.artifact,
            name="delete-file",
            transformations=(("removed.py", before, None),),
        )
        return SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset((stage,), final_files={"removed.py": None}),
        )

    def _compile_reviews(self, *stages: str) -> dict[str, object]:
        output = self.artifact / "generated.py"
        compiler = Path(__file__).with_name("compile_review_diff.py")
        arguments = [
            str(compiler),
            "--source",
            str(self.source),
            "--artifact-root",
            str(self.artifact),
            "--output",
            str(output),
            "--source-revision",
            "synthetic-revision",
            "--identity",
            "identity.txt",
        ]
        for stage in stages:
            arguments.extend(("--stage", f"{stage}={stage}.patch"))
        # The maintenance compiler is a script with a sibling framework import.
        # Run that actual entry point while retaining this suite's framework module.
        with patch.dict(sys.modules, {"framework": framework}):
            with patch.object(sys, "argv", arguments):
                runpy.run_path(str(compiler), run_name="__main__")
        tree = ast.parse(output.read_text(encoding="utf-8"))
        return {
            node.targets[0].id: ast.literal_eval(node.value)
            for node in tree.body
            if isinstance(node, ast.Assign)
            and len(node.targets) == 1
            and isinstance(node.targets[0], ast.Name)
        }

    def _write_hunk_review(
        self,
        name: str,
        path: str,
        hunks: tuple[tuple[int, int, str, str], ...],
    ) -> None:
        review = f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n"
        for old_start, new_start, before, after in hunks:
            before_lines = before.count("\n")
            after_lines = after.count("\n")
            review += (
                f"@@ -{old_start},{before_lines} +{new_start},{after_lines} @@\n"
                + "".join(f"-{line}" for line in before.splitlines(keepends=True))
                + "".join(f"+{line}" for line in after.splitlines(keepends=True))
            )
        (self.artifact / f"{name}.patch").write_text(review, encoding="utf-8")

    def _compiled_transaction(
        self, generated: dict[str, object]
    ) -> SourcePatchTransaction:
        stages = tuple(
            PatchStage(
                name=stage["name"],
                rationale="Compile and apply the same exact reviewed operation.",
                removal_condition="Remove with this test.",
                review_patch=stage["review_patch"],
                review_sha256=stage["review_sha256"],
                files=tuple(FileIdentity(**item) for item in stage["files"]),
                edits=tuple(LandmarkEdit(**item) for item in stage["edits"]),
                validate_before=_noop,
                validate_after=_noop,
            )
            for stage in generated["GENERATED_STAGES"]
        )
        return SourcePatchTransaction(
            self.source,
            self.artifact,
            PatchSet(
                name="compiled-deletion-test",
                source_revision=generated["SOURCE_REVISION"],
                identity_files=generated["IDENTITY_FILES"],
                stages=stages,
                final_files=generated["FINAL_FILES"],
                validate_final=_noop,
            ),
        )

    def test_delete_edit_and_create_share_one_exact_idempotent_transaction(
        self,
    ) -> None:
        removed = "# original λ instruction\ndef removed():\n    return 'שלום'\n"
        before = "def value():\n    return 1\n"
        after = "def value():\n    return 2\n"
        created = "def created():\n    return 'new'\n"
        (self.source / "removed.py").write_text(removed, encoding="utf-8")
        (self.source / "removed.py").chmod(0o751)
        (self.source / "changed.py").write_text(before, encoding="utf-8")
        (self.source / "changed.py").chmod(0o640)
        validated = []

        def validate_after(state) -> None:
            self.assertNotIn("removed.py", state)
            self.assertEqual(state["changed.py"], after)
            self.assertEqual(state["created.py"], created)
            validated.append(dict(state))

        stage = _stage(
            self.artifact,
            name="replace-obsolete-module",
            transformations=(
                ("removed.py", removed, None),
                ("changed.py", before, after),
                ("created.py", None, created),
            ),
            validate_after=validate_after,
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset(
                (stage,),
                final_files={
                    "removed.py": None,
                    "changed.py": sha256_text(after),
                    "created.py": sha256_text(created),
                },
                final_validator=validate_after,
            ),
        )
        planned, result = transaction.plan()
        self.assertNotIn("removed.py", planned)
        self.assertEqual((self.source / "removed.py").read_text(), removed)
        self.assertFalse((self.source / "created.py").exists())
        first = transaction.commit(planned, result)
        self.assertEqual(first.state, "applied")
        self.assertEqual(
            first.changed_files, ("changed.py", "created.py", "removed.py")
        )
        self.assertFalse(os.path.lexists(self.source / "removed.py"))
        self.assertEqual((self.source / "changed.py").read_text(), after)
        self.assertEqual((self.source / "created.py").read_text(), created)
        self.assertEqual((self.source / "changed.py").stat().st_mode & 0o777, 0o640)
        self.assertEqual((self.source / "created.py").stat().st_mode & 0o777, 0o644)
        prior = {
            path: (self.source / path).stat() for path in ("changed.py", "created.py")
        }
        with patch.object(framework.os, "replace") as replace:
            with patch.object(Path, "unlink") as unlink:
                second = transaction.apply()
        replace.assert_not_called()
        unlink.assert_not_called()
        self.assertEqual(second.state, "already-applied")
        self.assertEqual(second.changed_files, ())
        self.assertTrue(validated)
        for path, info in prior.items():
            now = (self.source / path).stat()
            self.assertEqual(
                (info.st_mode, info.st_ino, info.st_mtime_ns, info.st_ctime_ns),
                (now.st_mode, now.st_ino, now.st_mtime_ns, now.st_ctime_ns),
            )

    def test_declared_final_absence_is_already_applied_without_placeholder(
        self,
    ) -> None:
        transaction = self._deletion_transaction("def removed():\n    pass\n")
        with patch.object(framework.os, "replace") as replace:
            with patch.object(Path, "unlink") as unlink:
                result = transaction.apply()
        self.assertEqual(result.state, "already-applied")
        self.assertEqual(result.changed_files, ())
        self.assertFalse(os.path.lexists(self.source / "removed.py"))
        replace.assert_not_called()
        unlink.assert_not_called()

    def test_empty_placeholder_is_not_declared_final_absence(self) -> None:
        target = self.source / "removed.py"
        target.write_bytes(b"")
        prior = target.stat()
        transaction = self._deletion_transaction("def removed():\n    pass\n")
        with self.assertRaises(PatchRefusedError):
            transaction.apply()
        self.assertEqual(target.read_bytes(), b"")
        self.assertEqual(target.stat().st_ino, prior.st_ino)
        self.assertEqual(target.stat().st_mtime_ns, prior.st_mtime_ns)

    def test_deletion_refuses_drift_missing_pristine_member_and_wrong_identity(
        self,
    ) -> None:
        original = "def removed():\n    return 1\n"
        companion = "def kept():\n    return 1\n"
        changed = "def kept():\n    return 2\n"
        stage = _stage(
            self.artifact,
            name="delete-and-change",
            transformations=(
                ("removed.py", original, None),
                ("kept.py", companion, changed),
            ),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset(
                (stage,),
                final_files={"removed.py": None, "kept.py": sha256_text(changed)},
            ),
        )
        for case in ("drift", "missing", "identity", "review"):
            with self.subTest(case=case):
                (self.source / "identity.txt").write_text("pinned-revision\n")
                (self.source / "kept.py").write_text(companion)
                target = self.source / "removed.py"
                target.write_text(original)
                if case == "drift":
                    target.write_text(original + "# unexpected retained source\n")
                elif case == "missing":
                    target.unlink()
                elif case == "identity":
                    (self.source / "identity.txt").write_text("different-revision\n")
                elif case == "review":
                    (self.artifact / stage.review_patch).write_text("drift\n")
                before = {
                    p.name: (p.read_bytes(), p.stat()) for p in self.source.iterdir()
                }
                with self.assertRaises(PatchRefusedError):
                    transaction.apply()
                self.assertEqual(set(before), {p.name for p in self.source.iterdir()})
                for name, (data, info) in before.items():
                    path = self.source / name
                    self.assertEqual(path.read_bytes(), data)
                    self.assertEqual(path.stat().st_mtime_ns, info.st_mtime_ns)
                    self.assertEqual(path.stat().st_ctime_ns, info.st_ctime_ns)

    def test_deleted_path_cannot_be_a_symlink_or_directory(self) -> None:
        original = "def removed():\n    pass\n"
        outside = self.artifact / "untouched.py"
        outside.write_text(original)
        target = self.source / "removed.py"
        transaction = self._deletion_transaction(original)
        for case in ("dangling-link", "existing-link", "directory"):
            with self.subTest(case=case):
                if case == "directory":
                    target.mkdir()
                else:
                    target.symlink_to(
                        outside
                        if case == "existing-link"
                        else self.artifact / "missing"
                    )
                try:
                    with self.assertRaises(PatchRefusedError):
                        transaction.apply()
                    self.assertTrue(os.path.lexists(target))
                    self.assertEqual(outside.read_text(), original)
                finally:
                    if case == "directory":
                        target.rmdir()
                    else:
                        target.unlink()

    def test_deletion_landmark_must_cover_the_whole_original_file(self) -> None:
        original = "def first():\n    return 1\ndef second():\n    return 2\n"
        partial = "def first():\n    return 1\n"
        target = self.source / "removed.py"
        target.write_text(original)
        stage = _stage(
            self.artifact,
            name="partial-delete",
            transformations=(("removed.py", partial, None),),
        )
        stage = replace(
            stage,
            files=(FileIdentity("removed.py", sha256_text(original), None),),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset((stage,), final_files={"removed.py": None}),
        )
        with self.assertRaisesRegex(PatchRefusedError, "complete file"):
            transaction.apply()
        self.assertEqual(target.read_text(), original)

    def test_identity_cannot_declare_absence_before_and_after(self) -> None:
        with self.assertRaises(PatchRefusedError):
            FileIdentity("removed.py", None, None)

    def test_immutable_identity_cannot_also_be_a_mutable_stage_file(self) -> None:
        before = "pinned-revision\n"
        after = "changed-revision\n"
        stage = _stage(
            self.artifact,
            name="change-identity",
            transformations=(("identity.txt", before, after),),
        )

        with self.assertRaises(PatchRefusedError):
            _patchset((stage,), final_files={"identity.txt": sha256_text(after)})

        self.assertEqual((self.source / "identity.txt").read_text(), before)

    def test_deletion_plan_refuses_later_source_or_symlink_replacement(self) -> None:
        original = "def removed():\n    pass\n"
        target = self.source / "removed.py"
        transaction = self._deletion_transaction(original)
        for case in ("source", "symlink"):
            with self.subTest(case=case):
                target.write_text(original)
                planned, result = transaction.plan()
                if case == "source":
                    target.write_text(original + "# concurrent change\n")
                else:
                    target.unlink()
                    target.symlink_to(self.artifact / "missing")
                try:
                    with self.assertRaises(PatchRefusedError):
                        transaction.commit(planned, result)
                    self.assertTrue(os.path.lexists(target))
                    if case == "source":
                        self.assertIn("# concurrent change", target.read_text())
                    else:
                        self.assertTrue(target.is_symlink())
                finally:
                    target.unlink()

    def test_failed_later_replace_restores_deleted_and_edited_bytes_modes_and_new_absence(
        self,
    ) -> None:
        removed = "# λ non-ASCII source\ndef a():\n    return 'שלום'\n"
        edited = "def c():\n    return 1\n"
        final = "def c():\n    return 2\n"
        failure = OSError("last replacement refused")
        for deleted_mode in (0o751, 0o000):
            with self.subTest(deleted_mode=oct(deleted_mode)):
                (self.source / "a-removed.py").write_text(removed)
                (self.source / "a-removed.py").chmod(deleted_mode)
                (self.source / "c-edited.py").write_text(edited)
                (self.source / "c-edited.py").chmod(0o640)
                (self.source / "z-last.py").write_text(edited)
                stage = _stage(
                    self.artifact,
                    name="rollback-deletion",
                    transformations=(
                        ("a-removed.py", removed, None),
                        ("b-created.py", None, "def b():\n    pass\n"),
                        ("c-edited.py", edited, final),
                        ("z-last.py", edited, final),
                    ),
                )
                transaction = SourcePatchTransaction(
                    self.source,
                    self.artifact,
                    _patchset(
                        (stage,),
                        final_files={
                            "a-removed.py": None,
                            "b-created.py": sha256_text("def b():\n    pass\n"),
                            "c-edited.py": sha256_text(final),
                            "z-last.py": sha256_text(final),
                        },
                    ),
                )
                real_replace = os.replace
                attempted = []

                def fail_last(src, dst) -> None:
                    if ".qwen-source-patch." in Path(src).name:
                        attempted.append(Path(dst).name)
                        if Path(dst).name == "z-last.py":
                            self.assertFalse((self.source / "a-removed.py").exists())
                            self.assertTrue((self.source / "b-created.py").exists())
                            self.assertEqual(
                                (self.source / "c-edited.py").read_text(), final
                            )
                            raise failure
                    real_replace(src, dst)

                real_read_bytes = Path.read_bytes

                def read_privileged_original(path) -> bytes:
                    if path == self.source / "a-removed.py" and deleted_mode == 0:
                        # Model a privileged build worker's read capability; this
                        # test process cannot read mode 000. All writes, unlink,
                        # rollback bytes and permission changes remain actual FS.
                        return removed.encode()
                    return real_read_bytes(path)

                read_access = (
                    patch.object(Path, "read_bytes", read_privileged_original)
                    if deleted_mode == 0
                    else nullcontext()
                )
                with read_access, patch.object(framework.os, "replace", fail_last):
                    with self.assertRaises(PatchWriteError) as caught:
                        transaction.apply()
                self.assertIs(caught.exception.__cause__, failure)
                self.assertEqual(
                    attempted, ["b-created.py", "c-edited.py", "z-last.py"]
                )
                restored = self.source / "a-removed.py"
                restored_mode = restored.stat().st_mode & 0o777
                restored.chmod(0o400)
                try:
                    self.assertEqual(restored.read_bytes(), removed.encode())
                finally:
                    restored.chmod(restored_mode)
                self.assertEqual(restored_mode, deleted_mode)
                self.assertFalse(os.path.lexists(self.source / "b-created.py"))
                self.assertEqual((self.source / "c-edited.py").read_text(), edited)
                self.assertEqual(
                    (self.source / "c-edited.py").stat().st_mode & 0o777, 0o640
                )
                self.assertEqual((self.source / "z-last.py").read_text(), edited)
                self.assertFalse(tuple(self.source.rglob("*.qwen-source-patch.*")))
                self.assertFalse(tuple(self.source.rglob("*.qwen-rollback.*")))

    def test_failed_unlink_rolls_back_earlier_edit(self) -> None:
        before = "def value():\n    return 1\n"
        after = "def value():\n    return 2\n"
        (self.source / "a-edited.py").write_text(before)
        (self.source / "a-edited.py").chmod(0o750)
        removed = self.source / "z-removed.py"
        removed.write_text(before)
        stage = _stage(
            self.artifact,
            name="unlink-failure",
            transformations=(
                ("a-edited.py", before, after),
                ("z-removed.py", before, None),
            ),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset(
                (stage,),
                final_files={"a-edited.py": sha256_text(after), "z-removed.py": None},
            ),
        )
        failure = OSError("unlink refused")
        real_unlink = Path.unlink

        def refuse_deleted(path, *args, **kwargs) -> None:
            if path == removed:
                raise failure
            real_unlink(path, *args, **kwargs)

        with patch.object(Path, "unlink", refuse_deleted):
            with self.assertRaises(PatchWriteError) as caught:
                transaction.apply()
        self.assertIs(caught.exception.__cause__, failure)
        self.assertEqual((self.source / "a-edited.py").read_text(), before)
        self.assertEqual((self.source / "a-edited.py").stat().st_mode & 0o777, 0o750)
        self.assertEqual(removed.read_text(), before)
        self.assertFalse(tuple(self.source.rglob("*.qwen-source-patch.*")))

    def test_compiler_represents_and_applies_deletion_edit_and_creation(self) -> None:
        removed = "def removed():\n    return 'λ'\n"
        before = "def changed():\n    return 1\n"
        after = "def changed():\n    return 2\n"
        created = "def created():\n    pass\n"
        (self.source / "removed.py").write_text(removed)
        (self.source / "changed.py").write_text(before)
        _stage(
            self.artifact,
            name="compiled",
            transformations=(
                ("removed.py", removed, None),
                ("changed.py", before, after),
                ("created.py", None, created),
            ),
        )
        generated = self._compile_reviews("compiled")
        self.assertEqual(
            generated["FINAL_FILES"],
            {
                "changed.py": sha256_text(after),
                "created.py": sha256_text(created),
                "removed.py": None,
            },
        )
        files = {
            item["path"]: item for item in generated["GENERATED_STAGES"][0]["files"]
        }
        self.assertEqual(files["removed.py"]["before_sha256"], sha256_text(removed))
        self.assertIsNone(files["removed.py"]["after_sha256"])
        self.assertIsNone(files["created.py"]["before_sha256"])
        transaction = self._compiled_transaction(generated)
        self.assertEqual(transaction.apply().state, "applied")
        self.assertFalse(os.path.lexists(self.source / "removed.py"))
        self.assertEqual((self.source / "changed.py").read_text(), after)
        self.assertEqual((self.source / "created.py").read_text(), created)
        self.assertEqual(transaction.apply().state, "already-applied")

    def test_compiler_carries_absence_through_delete_then_recreate_stages(self) -> None:
        before = "def removed():\n    return 1\n"
        recreated = "def replacement():\n    return 2\n"
        target = self.source / "removed.py"
        target.write_text(before)
        _stage(
            self.artifact,
            name="delete",
            transformations=(("removed.py", before, None),),
        )
        _stage(
            self.artifact,
            name="recreate",
            transformations=(("removed.py", None, recreated),),
        )
        generated = self._compile_reviews("delete", "recreate")
        deleted, created = generated["GENERATED_STAGES"]
        self.assertIsNone(deleted["files"][0]["after_sha256"])
        self.assertIsNone(created["files"][0]["before_sha256"])
        self.assertEqual(
            generated["FINAL_FILES"], {"removed.py": sha256_text(recreated)}
        )
        transaction = self._compiled_transaction(generated)
        self.assertEqual(transaction.apply().state, "applied")
        self.assertEqual(target.read_text(), recreated)
        self.assertEqual(transaction.apply().state, "already-applied")

    def test_compiler_refuses_partial_deletion_without_publishing_generated_data(
        self,
    ) -> None:
        original = "def first():\n    return 1\ndef second():\n    return 2\n"
        partial = "def first():\n    return 1\n"
        target = self.source / "removed.py"
        target.write_text(original)
        _stage(
            self.artifact,
            name="partial",
            transformations=(("removed.py", partial, None),),
        )
        with self.assertRaisesRegex(PatchRefusedError, "complete source file"):
            self._compile_reviews("partial")
        self.assertFalse((self.artifact / "generated.py").exists())
        self.assertEqual(target.read_text(), original)

    def test_compiler_accepts_terminal_deletion_when_result_is_source_prefix(
        self,
    ) -> None:
        retained = "def retained():\n    return 'שלום'\n"
        before = retained + "\ndef removed():\n    return 0\n"
        target = self.source / "module.py"
        target.write_text(before, encoding="utf-8")
        _stage(
            self.artifact,
            name="remove-terminal-helper",
            transformations=(("module.py", before, retained),),
        )
        self.assertEqual(before.count(retained), 1)

        generated = self._compile_reviews("remove-terminal-helper")

        self.assertEqual(target.read_text(encoding="utf-8"), before)
        self.assertEqual(generated["FINAL_FILES"], {"module.py": sha256_text(retained)})
        transaction = self._compiled_transaction(generated)
        self.assertEqual(transaction.apply().state, "applied")
        self.assertEqual(target.read_text(encoding="utf-8"), retained)
        self.assertEqual(transaction.apply().state, "already-applied")

    def test_transaction_accepts_terminal_deletion_when_result_is_source_prefix(
        self,
    ) -> None:
        retained = "def retained():\n    return 'λ'\n"
        before = retained + "\ndef removed():\n    return 0\n"
        target = self.source / "module.py"
        target.write_text(before, encoding="utf-8")
        target.chmod(0o750)
        stage = _stage(
            self.artifact,
            name="remove-terminal-helper",
            transformations=(("module.py", before, retained),),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset((stage,), final_files={"module.py": sha256_text(retained)}),
        )
        planned, result = transaction.plan()

        self.assertEqual(planned["module.py"], retained)
        self.assertEqual(target.read_text(encoding="utf-8"), before)
        self.assertEqual(transaction.commit(planned, result).state, "applied")
        self.assertEqual(target.read_text(encoding="utf-8"), retained)
        first = target.stat()
        self.assertEqual(first.st_mode & 0o777, 0o750)
        self.assertEqual(transaction.apply().state, "already-applied")
        second = target.stat()
        self.assertEqual(
            (first.st_ino, first.st_mtime_ns, first.st_ctime_ns),
            (second.st_ino, second.st_mtime_ns, second.st_ctime_ns),
        )

    def test_compiler_accepts_result_landmark_already_elsewhere_in_source(
        self,
    ) -> None:
        before = "value = 2\n# target\nvalue = 1\n"
        after = "value = 2\n# target\nvalue = 2\n"
        target = self.source / "module.py"
        target.write_text(before)
        self._write_hunk_review(
            "same-result",
            "module.py",
            ((3, 3, "value = 1\n", "value = 2\n"),),
        )

        generated = self._compile_reviews("same-result")

        self.assertEqual(generated["FINAL_FILES"], {"module.py": sha256_text(after)})
        self.assertEqual(target.read_text(), before)
        transaction = self._compiled_transaction(generated)
        self.assertEqual(transaction.apply().state, "applied")
        self.assertEqual(target.read_text(), after)
        self.assertEqual(transaction.apply().state, "already-applied")

    def test_compiler_refuses_mislocated_hunk_even_when_before_is_unique(
        self,
    ) -> None:
        before = "# head\n# context\nvalue = 1\n"
        target = self.source / "module.py"
        target.write_text(before)
        for new_start in (0, 1, 99):
            with self.subTest(new_start=new_start):
                self._write_hunk_review(
                    "wrong-line",
                    "module.py",
                    ((3, new_start, "value = 1\n", "value = 2\n"),),
                )
                with self.assertRaises(PatchRefusedError):
                    self._compile_reviews("wrong-line")
                self.assertEqual(target.read_text(), before)
                self.assertFalse((self.artifact / "generated.py").exists())

    def test_compiler_uses_new_line_coordinate_after_earlier_insertions(
        self,
    ) -> None:
        before = "# head\nvalue = 1\n# first\nvalue = 1\n# second\n"
        inserted = "".join(f"# inserted {index}\n" for index in range(6))
        after = inserted + "# head\nvalue = 1\n# first\nvalue = 2\n# second\n"
        target = self.source / "module.py"
        target.write_text(before)
        self._write_hunk_review(
            "shifted-duplicate",
            "module.py",
            (
                (1, 1, "# head\n", inserted + "# head\n"),
                (4, 10, "value = 1\n", "value = 2\n"),
            ),
        )

        generated = self._compile_reviews("shifted-duplicate")

        self.assertEqual(generated["FINAL_FILES"], {"module.py": sha256_text(after)})
        self.assertEqual(target.read_text(), before)
        transaction = self._compiled_transaction(generated)
        self.assertEqual(transaction.apply().state, "applied")
        self.assertEqual(target.read_text(), after)
        self.assertEqual(transaction.apply().state, "already-applied")

    def test_transaction_accepts_result_landmark_already_elsewhere_in_source(
        self,
    ) -> None:
        before = "value = 2\n# target\nvalue = 1\n"
        after = "value = 2\n# target\nvalue = 2\n"
        target = self.source / "module.py"
        target.write_text(before)
        stage = _stage(
            self.artifact,
            name="same-result",
            transformations=(("module.py", "value = 1\n", "value = 2\n"),),
        )
        stage = replace(
            stage,
            files=(FileIdentity("module.py", sha256_text(before), sha256_text(after)),),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset((stage,), final_files={"module.py": sha256_text(after)}),
        )

        self.assertEqual(transaction.apply().state, "applied")
        self.assertEqual(target.read_text(), after)
        self.assertEqual(transaction.apply().state, "already-applied")

    def test_transaction_refuses_ambiguous_before_despite_matching_file_hash(
        self,
    ) -> None:
        before = "value = 1\nvalue = 1\n"
        after = "value = 2\nvalue = 1\n"
        target = self.source / "module.py"
        target.write_text(before)
        prior = target.stat()
        stage = _stage(
            self.artifact,
            name="ambiguous-before",
            transformations=(("module.py", "value = 1\n", "value = 2\n"),),
        )
        stage = replace(
            stage,
            files=(FileIdentity("module.py", sha256_text(before), sha256_text(after)),),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset((stage,), final_files={"module.py": sha256_text(after)}),
        )

        with self.assertRaisesRegex(PatchRefusedError, "expected one before landmark"):
            transaction.apply()

        self.assertEqual(target.read_text(), before)
        self.assertEqual(target.stat().st_mtime_ns, prior.st_mtime_ns)
        self.assertEqual(target.stat().st_ino, prior.st_ino)
        self.assertFalse(tuple(self.source.rglob("*.qwen-source-patch.*")))

    def test_transaction_refuses_wrong_final_hash_when_result_already_occurs(
        self,
    ) -> None:
        before = "value = 2\n# target\nvalue = 1\n"
        wrong_after = "value = 2\n# target\nvalue = 3\n"
        target = self.source / "module.py"
        target.write_text(before)
        prior = target.stat()
        stage = _stage(
            self.artifact,
            name="wrong-final-hash",
            transformations=(("module.py", "value = 1\n", "value = 2\n"),),
        )
        stage = replace(
            stage,
            files=(
                FileIdentity(
                    "module.py", sha256_text(before), sha256_text(wrong_after)
                ),
            ),
        )
        transaction = SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset((stage,), final_files={"module.py": sha256_text(wrong_after)}),
        )

        with self.assertRaisesRegex(PatchRefusedError, "final hash mismatch"):
            transaction.apply()

        self.assertEqual(target.read_text(), before)
        self.assertEqual(target.stat().st_mtime_ns, prior.st_mtime_ns)
        self.assertEqual(target.stat().st_ino, prior.st_ino)
        self.assertFalse(tuple(self.source.rglob("*.qwen-source-patch.*")))


class SourceVectorTransactionTests(unittest.TestCase):
    """Canonical vectors enter Qwen through the authored source transaction.

    Native bindings are now compiler output, qualified by the actual native
    publisher. This suite isolates only the unrelated whole-Qwen semantic
    callback; source-vector byte agreement and transaction refusal stay real.
    """

    vector_names = ("goal-state-v1", "partial-stream-v1")

    @classmethod
    def setUpClass(cls) -> None:
        from . import apply_qwen_code_patchset

        cls.binding_owner = apply_qwen_code_patchset
        root = Path(__file__).resolve().parents[2]
        artifact_paths = tuple(
            f"protocol/test-vectors/{name}.json" for name in cls.vector_names
        )
        cls.artifact_bytes = {
            path: (root / path).read_bytes() for path in artifact_paths
        }
        expected = {
            f"packages/core/src/utils/__fixtures__/{name}.json"
            for name in cls.vector_names
        }
        cls.generated_outputs = {}
        for stage in cls.binding_owner.GENERATED_STAGES:
            for edit in stage["edits"]:
                if edit["path"] in expected:
                    if edit["before"] or edit["path"] in cls.generated_outputs:
                        raise AssertionError(
                            "vector fixture requires the actual complete new-file edit"
                        )
                    contents = edit["after"]
                    if (
                        sha256_text(contents)
                        != cls.binding_owner.FINAL_FILES[edit["path"]]
                    ):
                        raise AssertionError("vector fixture differs from final identity")
                    cls.generated_outputs[edit["path"]] = contents
        if set(cls.generated_outputs) != expected:
            raise AssertionError("canonical source vector fixture is incomplete")

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="source-binding-test-")
        root = Path(self.temporary.name)
        self.source, self.artifact = root / "source", root / "artifact"
        self.source.mkdir()
        self.artifact.mkdir()
        (self.source / "identity.txt").write_text("pinned-revision\n")
        for relative, contents in self.artifact_bytes.items():
            target = self.artifact / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(contents)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _snapshot(self) -> dict[str, tuple[object, ...]]:
        result = {}
        for path in (self.source, *self.source.rglob("*")):
            info = path.lstat()
            result[str(path.relative_to(self.source))] = (
                info.st_mode,
                info.st_ino,
                info.st_mtime_ns,
                path.read_bytes() if path.is_file() else None,
            )
        return result

    def _transaction(
        self, changes: dict[str, str] | None = None
    ) -> SourcePatchTransaction:
        outputs = self.generated_outputs | (changes or {})
        # All outer file hashes and review bytes describe the mutated candidate.
        # A refusal therefore cannot be credited to stale review metadata.
        stage = _stage(
            self.artifact,
            name="canonical-binding-candidate",
            transformations=tuple(
                (path, None, contents) for path, contents in outputs.items()
            ),
        )
        binding_validator = self.binding_owner.build_patchset(
            self.artifact
        ).validate_final
        return SourcePatchTransaction(
            self.source,
            self.artifact,
            _patchset(
                (stage,),
                final_files={
                    path: sha256_text(contents) for path, contents in outputs.items()
                },
                final_validator=binding_validator,
            ),
        )

    def _assert_refused(
        self, reason: str, changes: dict[str, str] | None = None
    ) -> None:
        # An unexpected acceptance must not contaminate a later subtest's source.
        original_source = self.source
        with tempfile.TemporaryDirectory(dir=self.temporary.name) as candidate:
            self.source = Path(candidate)
            (self.source / "identity.txt").write_text("pinned-revision\n")
            try:
                transaction = self._transaction(changes)
                before = self._snapshot()
                with (
                    patch.object(self.binding_owner, "validate_final", return_value=None),
                    patch.object(transaction, "commit", wraps=transaction.commit) as commit,
                ):
                    with self.assertRaisesRegex(PatchRefusedError, reason):
                        transaction.apply()
                    commit.assert_not_called()
                self.assertEqual(self._snapshot(), before)
            finally:
                self.source = original_source

    def test_canonical_vectors_commit_and_are_idempotent(self) -> None:
        transaction = self._transaction()
        with patch.object(self.binding_owner, "validate_final", return_value=None):
            result = transaction.apply()
            self.assertEqual(result.state, "applied")
            self.assertEqual(set(result.changed_files), set(self.generated_outputs))
            for path, contents in self.generated_outputs.items():
                self.assertEqual((self.source / path).read_bytes(), contents.encode())
            before = self._snapshot()
            self.assertEqual(transaction.apply().state, "already-applied")
            self.assertEqual(self._snapshot(), before)

    def test_each_canonical_vector_requires_exact_bytes_before_writes(self) -> None:
        for name in self.vector_names:
            target = self.artifact / f"protocol/test-vectors/{name}.json"
            original = target.read_bytes()
            for label, mutated in (
                ("crlf", original.replace(b"\n", b"\r\n")),
                ("invalid-utf8", original + b"\xff"),
            ):
                with self.subTest(vector=name, mutation=label):
                    self.assertNotEqual(original, mutated)
                    target.write_bytes(mutated)
                    self._assert_refused(f"test vectors drifted: {name}")
            target.write_bytes(original)

    def test_copied_vector_drift_with_coherent_review_refuses_before_writes(self) -> None:
        for name in self.vector_names:
            with self.subTest(vector=name):
                path = f"packages/core/src/utils/__fixtures__/{name}.json"
                self._assert_refused(
                    f"test vectors drifted: {name}",
                    {path: self.generated_outputs[path] + "\n"},
                )



if __name__ == "__main__":
    unittest.main()
