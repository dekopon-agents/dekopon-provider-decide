# Decide provider 0.1.2 smoke

- Tested prep head `2e7ac13e8ded0cbdd8a40b0457b2193cfc8c0f53`, base `9a862acab993d836e092038c7af5f39e897075de`.
- Completed 2026-11-23T14:27:26Z.
- Rust 1.98.1, wasm-tools 1.259.0, provider-workflows `2033d4c7ef345bdf640204d53a36d4108ffadeba`.
- Cargo.lock resolves Wasmtime 49.0.6, url 2.5.9, and form_urlencoded 1.2.3.
- `decide-provider.wasm` SHA-256 `0e0f4f21376a837fdc63643616001683ae2bf5e9a0d9e8e5eb1056bed80b3665`.

```sh
../provider-workflows/build.sh
sha256sum -c decide-provider.wasm.sha256
DEKOPON_PROVIDER_COMPONENT="$PWD/decide-provider.wasm" cargo test -p dekopon-decide-provider --locked
```

The build and checksum passed. All 13 tests passed (2 unit and 11 integration). The doc-test harness completed with zero tests. The test command used a writable scratch `TMPDIR`.
