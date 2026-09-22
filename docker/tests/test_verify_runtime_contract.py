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
                "settings reasoning effort drift",
            ):
                MODULE.verify(paths)

    def test_rejects_retired_generation_fields_even_if_settings_are_resealed(self) -> None:
        # extra_body carries only the reasoning switches. parallel_tool_calls
        # is the client's own constant, which the client refuses to take from
        # extra_body, and the two phase budgets could never bind before the
        # limits that do, so none of them is a setting.
        for field, value in (
            ("parallel_tool_calls", False),
            ("thinking_token_budget", 262144),
            ("final_response_token_budget", 131072),
        ):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                settings = json.loads(self.paths[1].read_text(encoding="utf-8"))
                provider = settings["modelProviders"]["openai"][0]
                provider["generationConfig"]["extra_body"][field] = value
                settings_path = root / "settings.json"
                settings_path.write_text(
                    json.dumps(settings, indent=2) + "\n", encoding="utf-8"
                )
                contract = json.loads(self.paths[0].read_text(encoding="utf-8"))
                contract["components"]["settings_sha256"] = MODULE.sha256(
                    settings_path.read_bytes()
                )
                contract_path = root / "contract.json"
                contract_path.write_text(
                    json.dumps(contract, indent=2) + "\n", encoding="utf-8"
                )
                paths = [contract_path, settings_path, *self.paths[2:]]
                with self.assertRaisesRegex(
                    MODULE.ContractError, "generation extra body drift"
                ):
                    MODULE.verify(paths)

    def test_rejects_retired_settings_even_if_resealed(self) -> None:
        # A setting the client no longer has is refused by name: resealed into the
        # file it would read as a mode that could still be switched on.
        for path, value in (
            (("model", "skipNextSpeakerCheck"), True),
            (("model", "skipNextSpeakerCheck"), False),
        ):
            with self.subTest(path=path, value=value), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                settings = json.loads(self.paths[1].read_text(encoding="utf-8"))
                node = settings
                for key in path[:-1]:
                    node = node.setdefault(key, {})
                node[path[-1]] = value
                settings_path = root / "settings.json"
                settings_path.write_text(json.dumps(settings, indent=2) + "\n", encoding="utf-8")
                contract = json.loads(self.paths[0].read_text(encoding="utf-8"))
                contract["components"]["settings_sha256"] = MODULE.sha256(settings_path.read_bytes())
                contract_path = root / "contract.json"
                contract_path.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
                paths = [contract_path, settings_path, *self.paths[2:]]
                with self.assertRaisesRegex(
                    MODULE.ContractError, f"settings carry {'.'.join(path)}, which the client no longer has"
                ):
                    MODULE.verify(paths)

    def test_rejects_a_wrapper_that_sets_stream_bounds_even_if_resealed(self) -> None:
        # The client derives every stream bound itself; a wrapper that pins one
        # through the environment is a second source, refused by name.
        for line in (
            "export QWEN_STREAM_IDLE_TIMEOUT_MS=240000",
            "export QWEN_STREAM_MAX_LIFETIME_MS=21600000",
        ):
            with self.subTest(line=line), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                source = self.paths[6].read_text(encoding="utf-8")
                resealed = source.replace(
                    "export NO_COLOR=1\n", f"{line}\nexport NO_COLOR=1\n"
                )
                self.assertNotEqual(source, resealed)
                wrapper_path = root / "run_agent.sh"
                wrapper_path.write_text(resealed, encoding="utf-8")
                contract = json.loads(self.paths[0].read_text(encoding="utf-8"))
                contract["components"]["wrapper_sha256"] = MODULE.sha256(
                    wrapper_path.read_bytes()
                )
                contract_path = root / "contract.json"
                contract_path.write_text(
                    json.dumps(contract, indent=2) + "\n", encoding="utf-8"
                )
                paths = [contract_path, *self.paths[1:6], wrapper_path, self.paths[7]]
                with self.assertRaisesRegex(
                    MODULE.ContractError, "agent wrapper must not set stream bounds"
                ):
                    MODULE.verify(paths)

    def test_rejects_turn_budget_drift_even_if_settings_are_resealed(self) -> None:
        # The default turn budget is cross-checked in several places. Resealing
        # the settings file and the contract together still leaves the
        # launcher's own compiled default, which is what the agent actually
        # runs under.
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            contract = json.loads(self.paths[0].read_text(encoding="utf-8"))
            contract["execution"]["max_session_turns"] = 401
            settings = json.loads(self.paths[1].read_text(encoding="utf-8"))
            settings["model"]["maxSessionTurns"] = 401
            settings_path = root / "settings.json"
            settings_path.write_text(
                json.dumps(settings, indent=2) + "\n", encoding="utf-8"
            )
            contract["components"]["settings_sha256"] = MODULE.sha256(
                settings_path.read_bytes()
            )
            contract_path = root / "contract.json"
            contract_path.write_text(
                json.dumps(contract, indent=2) + "\n", encoding="utf-8"
            )
            paths = [contract_path, settings_path, *self.paths[2:]]
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
            source = self.paths[7].read_text(encoding="utf-8")
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
            paths = [contract_path, *self.paths[1:7], source_path]
            with self.assertRaisesRegex(
                MODULE.ContractError, "missing canonical fragment"
            ):
                MODULE.verify(paths)

    def test_rejects_a_contract_that_misstates_what_teardown_keeps_even_if_resealed(
        self,
    ) -> None:
        # Session teardown keeps the final /workspace and /artifacts and
        # discards everything else, /tmp included. A contract that says less,
        # or says it again as "bundled", is refused even when resealed.
        source = self.paths[4].read_text(encoding="utf-8")
        for old, new, refusal in (
            (
                "Its final\n  state is kept",
                "Its initial\n  state is kept",
                "missing canonical fragment",
            ),
            (
                "`/artifacts` starts empty, is kept at session teardown, and is",
                "`/artifacts` starts empty and is",
                "missing canonical fragment",
            ),
            (
                "`/tmp` is discarded at session teardown",
                "`/tmp` is kept at session teardown",
                "missing canonical fragment",
            ),
            (
                "belongs in one of them.\n",
                "belongs in one of them. Scratch is not automatically bundled.\n",
                "deployment contract says 'bundled'",
            ),
        ):
            with self.subTest(new=new), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                resealed = source.replace(old, new)
                self.assertNotEqual(source, resealed)
                deployment_path = root / "deployment-contract.md"
                deployment_path.write_text(resealed, encoding="utf-8")
                contract = json.loads(self.paths[0].read_text(encoding="utf-8"))
                contract["components"]["deployment_contract_sha256"] = MODULE.sha256(
                    deployment_path.read_bytes()
                )
                contract_path = root / "contract.json"
                contract_path.write_text(
                    json.dumps(contract, indent=2) + "\n", encoding="utf-8"
                )
                paths = [contract_path, *self.paths[1:4], deployment_path, *self.paths[5:]]
                with self.assertRaisesRegex(MODULE.ContractError, refusal):
                    MODULE.verify(paths)

    def test_rejects_extra_native_tool(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = self.mutated_contract(
                root, lambda value: value["native_tools"].append("web_search")
            )
            with self.assertRaisesRegex(MODULE.ContractError, "agent_exec strict tools drift"):
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
