# Third-party licensing and scope

The repository-level [`LICENSE`](LICENSE) is The Unlicense
(`SPDX-License-Identifier: Unlicense`). It applies only to original material in
this repository for which the repository author owns the copyright and can make
the public-domain dedication.

It does not relicense third-party material. In particular:

- Qwen Code 0.21.12 at
  `b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38` is an upstream QwenLM
  work under Apache License 2.0. Review patches, semantic-transformation data,
  and generated artifacts containing or modifying Qwen Code source remain
  subject to that upstream license and notices. A copy is in
  [`LICENSES/Apache-2.0.txt`](LICENSES/Apache-2.0.txt).
- The paired Qwen3.8 model, corrected checkpoint, vLLM backend, and their
  patches are not relicensed here; their precise licensing scope is documented
  in the paired `qwen_38_agent_setup` repository.
- Rust crates, Node packages, Ubuntu packages, container base images, Docker
  tooling, compilers, proxy tools, and other bundled dependencies retain their
  respective upstream licenses. Exact version pinning does not change those
  terms.

The Unlicense applies to the separable original service, orchestration, tests,
documentation, and source-transformation framework only where the repository
author owns the relevant rights.

Nothing in this notice grants trademark rights or changes any upstream license.
