#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "verify_runtime_contract.py"
SPEC = importlib.util.spec_from_file_location("verify_runtime_contract", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class VerifyRuntimeContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.project = Path(__file__).resolve().parents[2]
        cls.paths = [
            cls.project / "config" / "agent-runtime-contract-v1.json",
            cls.project / "docker" / "config" / "settings.json",
            cls.project / "docker" / "config" / "settings-preserved.json",
            cls.project / "docker" / "config" / "QWEN.md",
            cls.project / "docker" / "config" / "system.md",
            cls.project / "docker" / "config" / "deployment-contract.md",
            cls.project / "docker" / "config" / "toolchain-manifest.json",
            cls.project / "docker" / "config" / "run_agent.sh",
            cls.project / "src" / "bin" / "agent_exec.rs",
        ]

    def test_accepts_exact_repository_contract(self) -> None:
        MODULE.verify(self.paths)

    def mutated_contract(self, root: Path, mutate) -> list[Path]:
        value = json.loads(self.paths[0].read_text(encoding="utf-8"))
        mutate(value)
        candidate = root / "contract.json"
        candidate.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
        return [candidate, *self.paths[1:]]

    def test_rejects_settings_hash_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = self.mutated_contract(
                root, lambda value: value["components"].__setitem__("settings_sha256", "0" * 64)
            )
            with self.assertRaisesRegex(MODULE.ContractError, "settings_sha256 drift"):
                MODULE.verify(paths)

    def test_rejects_weaker_thinking_even_if_settings_hash_is_resealed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            settings = json.loads(self.paths[1].read_text(encoding="utf-8"))
            settings["model"]["reasoningEffort"] = "medium"
            settings_path = root / "settings.json"
            settings_path.write_text(json.dumps(settings, indent=2) + "\n", encoding="utf-8")
            contract = json.loads(self.paths[0].read_text(encoding="utf-8"))
            contract["components"]["settings_sha256"] = MODULE.sha256(settings_path.read_bytes())
            contract_path = root / "contract.json"
            contract_path.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
            paths = [contract_path, settings_path, *self.paths[2:]]
            with self.assertRaisesRegex(
                MODULE.ContractError,
                "differ only in preserve_thinking|settings reasoning effort drift",
            ):
                MODULE.verify(paths)

    def test_rejects_a_second_difference_in_preserved_settings(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            settings = json.loads(self.paths[2].read_text(encoding="utf-8"))
            settings["modelProviders"]["openai"][0]["generationConfig"][
                "maxRetries"
            ] = 1
            settings_path = root / "settings-preserved.json"
            settings_path.write_text(
                json.dumps(settings, indent=2) + "\n", encoding="utf-8"
            )
            contract = json.loads(self.paths[0].read_text(encoding="utf-8"))
            contract["components"]["preserved_settings_sha256"] = MODULE.sha256(
                settings_path.read_bytes()
            )
            contract_path = root / "contract.json"
            contract_path.write_text(
                json.dumps(contract, indent=2) + "\n", encoding="utf-8"
            )
            paths = [contract_path, self.paths[1], settings_path, *self.paths[3:]]
            with self.assertRaisesRegex(
                MODULE.ContractError, "differ only in preserve_thinking"
            ):
                MODULE.verify(paths)

    def test_rejects_turn_budget_drift_even_if_settings_are_resealed(self) -> None:
        # The default turn budget is cross-checked in six places. Resealing the
        # settings pair and the contract together still leaves the launcher's
        # own compiled default, which is what the agent actually runs under.
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            contract = json.loads(self.paths[0].read_text(encoding="utf-8"))
            contract["execution"]["max_session_turns"] = 401
            settings_paths = []
            for index, name in ((1, "settings.json"), (2, "settings-preserved.json")):
                settings = json.loads(self.paths[index].read_text(encoding="utf-8"))
                settings["model"]["maxSessionTurns"] = 401
                path = root / name
                path.write_text(json.dumps(settings, indent=2) + "\n", encoding="utf-8")
                settings_paths.append(path)
            contract["components"]["settings_sha256"] = MODULE.sha256(
                settings_paths[0].read_bytes()
            )
            contract["components"]["preserved_settings_sha256"] = MODULE.sha256(
                settings_paths[1].read_bytes()
            )
            contract_path = root / "contract.json"
            contract_path.write_text(
                json.dumps(contract, indent=2) + "\n", encoding="utf-8"
            )
            paths = [contract_path, *settings_paths, *self.paths[3:]]
            with self.assertRaisesRegex(
                MODULE.ContractError, "agent_exec default max session turns drift"
            ):
                MODULE.verify(paths)

    def test_rejects_turn_budget_ceiling_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = self.mutated_contract(
                root,
                lambda value: value["execution"].__setitem__(
                    "max_session_turns_ceiling", 900
                ),
            )
            with self.assertRaisesRegex(
                MODULE.ContractError, "agent_exec max session turns ceiling drift"
            ):
                MODULE.verify(paths)

    def test_rejects_a_default_turn_budget_above_the_ceiling(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = self.mutated_contract(
                root,
                lambda value: value["execution"].__setitem__(
                    "max_session_turns_ceiling", 100
                ),
            )
            with self.assertRaisesRegex(
                MODULE.ContractError, "the default budget must itself be requestable"
            ):
                MODULE.verify(paths)

    def test_rejects_an_unbounded_or_malformed_turn_budget(self) -> None:
        for bad in (0, -1, True, "800", 800.0, None):
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                paths = self.mutated_contract(
                    root,
                    lambda value, bad=bad: value["execution"].__setitem__(
                        "max_session_turns_ceiling", bad
                    ),
                )
                with self.assertRaisesRegex(
                    MODULE.ContractError, "must be an integer of at least 1"
                ):
                    MODULE.verify(paths)

    def test_rejects_a_contract_without_a_turn_budget_ceiling(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = self.mutated_contract(
                root,
                lambda value: value["execution"].pop("max_session_turns_ceiling"),
            )
            # A contract that declares no ceiling cannot be verified against
            # one, so verification fails rather than assuming a bound.
            with self.assertRaises(KeyError):
                MODULE.verify(paths)

    def test_rejects_a_launcher_that_passes_a_fixed_turn_budget(self) -> None:
        # The per-session budget reaches Qwen Code -- and therefore every
        # foreground subagent -- through exactly one flag. A launcher that
        # rebuilt that flag from a constant would silently ignore the accepted
        # budget, so the flag's per-session form is pinned.
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = self.paths[8].read_text(encoding="utf-8")
            resealed = source.replace(
                '.arg(format!("--max-session-turns={max_session_turns}"))',
                '.arg(format!("--max-session-turns={DEFAULT_MAX_SESSION_TURNS}"))',
            )
            self.assertNotEqual(source, resealed)
            source_path = root / "agent_exec.rs"
            source_path.write_text(resealed, encoding="utf-8")
            contract = json.loads(self.paths[0].read_text(encoding="utf-8"))
            contract["components"]["agent_exec_source_sha256"] = MODULE.sha256(
                source_path.read_bytes()
            )
            contract_path = root / "contract.json"
            contract_path.write_text(
                json.dumps(contract, indent=2) + "\n", encoding="utf-8"
            )
            paths = [contract_path, *self.paths[1:8], source_path]
            with self.assertRaisesRegex(
                MODULE.ContractError, "missing canonical fragment"
            ):
                MODULE.verify(paths)

    def test_rejects_extra_native_tool(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = self.mutated_contract(
                root, lambda value: value["native_tools"].append("web_search")
            )
            with self.assertRaisesRegex(MODULE.ContractError, "deployment native tools drift"):
                MODULE.verify(paths)

    def test_rejects_broader_devpts_write_authority(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = self.mutated_contract(
                root,
                lambda value: value["filesystem"].__setitem__(
                    "private_devpts_write_access", "directory-write"
                ),
            )
            with self.assertRaisesRegex(
                MODULE.ContractError, "private devpts Landlock access drift"
            ):
                MODULE.verify(paths)

    def test_rejects_additional_agent_interface(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = self.mutated_contract(
                root, lambda value: value["network"]["interfaces"].append("eth0")
            )
            with self.assertRaisesRegex(MODULE.ContractError, "agent interfaces drift"):
                MODULE.verify(paths)

    def test_rejects_ipv6_route_authority(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = self.mutated_contract(
                root, lambda value: value["network"]["ipv6_routes"].append("default")
            )
            with self.assertRaisesRegex(MODULE.ContractError, "agent IPv6 routes drift"):
                MODULE.verify(paths)

    def test_rejects_symlinked_contract(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            link = Path(temporary) / "contract.json"
            link.symlink_to(self.paths[0])
            with self.assertRaisesRegex(MODULE.ContractError, "non-symlink"):
                MODULE.verify([link, *self.paths[1:]])


if __name__ == "__main__":
    unittest.main()
