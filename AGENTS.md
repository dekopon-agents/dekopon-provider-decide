# Guidance for coding agents

This repository builds one advisory OpenRouter Decisions Wasm component. Read [README.md](README.md) for its interface, limitations, and validation tolerance. Core's [boundaries](https://github.com/dekopon-agents/dekopon/blob/main/AGENTS.md#boundaries-that-must-survive), [Rust guidelines](https://github.com/dekopon-agents/dekopon/blob/main/AGENTS.md#rust-guidelines), and [comments](https://github.com/dekopon-agents/dekopon/blob/main/AGENTS.md#comments) apply. No releases, grants, bindings, live calls, or version bumps without owner approval.

## Boundaries

- Outputs are advisory script data, never Cedar permissions. The only destination is the owner's `providerSettings.decide.baseUrl` or the OpenRouter default; the broker's grant decides whether egress is allowed.
- Never accept a URL or credential in a question or evidence. Never set `authorization` in the guest; the broker injects it after destination validation. Never echo response bodies or secrets in errors.
- Only native OpenRouter Decisions models are accepted. No direct OpenAI endpoint or chat fallback. A refusal has no choice. Both Luna and Jev support native choice probabilities in their pinned OpenRouter documentation. No production allowlisted profile lacks probability support; the incapable-profile witness uses a `cfg(test)`-only synthetic profile.

## Build and verify

The shared [provider-workflows](https://github.com/dekopon-agents/provider-workflows) job is `ci / validate`. Clone it as a sibling in a bare checkout. Before pushing run:

```console
cargo fmt --all --check
cargo deny --all-features check bans licenses sources advisories
cargo check -p dekopon-decide-provider --locked
cargo clippy -p dekopon-decide-provider --all-targets --locked -- -D warnings
cargo clippy -p dekopon-decide-provider --lib --target wasm32-unknown-unknown --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc -p dekopon-decide-provider --no-deps --locked
../provider-workflows/build.sh
DEKOPON_PROVIDER_COMPONENT="$PWD/decide-provider.wasm" cargo test -p dekopon-decide-provider --locked
```

`build.sh` uses this checkout's target. Built-component tests must read `DEKOPON_PROVIDER_COMPONENT` with `expect`, never skip or silently fall back. Use the published testkit's `Harness` and `Native::requests()`; assert full URI/body, one call, and no guest authorization header for requests. Name native-only tests as such. Match typed errors rather than Display. Use synthetic credentials only. Pin SDK and testkit to the same published exact version, regenerate lock with Cargo. No `CHANGELOG.md` change in a feature PR; commit has one physical `Changelog:` trailer per user-facing entry. PRs never push main.
