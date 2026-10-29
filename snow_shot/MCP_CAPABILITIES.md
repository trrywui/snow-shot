# MCP capability contracts

Edit `mcp-capabilities.source.json` to review capability membership, adapter descriptions,
canvas tool mappings and the explicit Mini exclusions. Full and Mini catalogs are generated
from this single manifest. Do not edit `mcp-capabilities.json`, `mcp-capabilities-mini.json`
or `include/snow_shot/app/mcp/recordingoptioncontracts.h` directly.

The recording `validation` object is the canonical primitive input contract. Its keys also
define the published member list. Qt consumes the generated header for types, integer
bounds and enum choices. The `qt-color` and `output-path` formats select the application's
existing color and output-path checks; those runtime semantics remain owned by Qt.
Recording overrides may be omitted, but supplied JSON null is invalid.

Regenerate after an intentional contract change:

```powershell
python snow_shot/scripts/generate_mcp_capabilities.py
```

Configure rejects stale generated outputs. The `snow_shot_check_mcp_capabilities` target
and `python snow_shot/scripts/generate_mcp_capabilities.py --check` check freshness without
changing files. The existing capability checker still independently compares reviewed
tool/adapter mappings with Qt dispatch and Rust declarations; generation does not replace
those checks with comparisons between generated copies.

Run focused checks after changing these contracts:

```powershell
python snow_shot/tests/mcp_capability_generation_tests.py
python snow_shot/tests/check_mcp_capabilities.py
python snow_shot/tests/check_mcp_capabilities.py --mini
cargo test --manifest-path snow_shot/rust/snow-shot-mcp/Cargo.toml recording_
cargo test --manifest-path snow_shot/rust/snow-shot-mcp/Cargo.toml --features mini recording_
```

The Rust test compares its compiled JSON Schema against the canonical primitives and
exercises actual deserialization of every enum choice, numeric boundary, wrong type and
explicit null. Qt controller tests independently exercise validation through
`snow-shot-screen-recording-controller-tests --automation-only`, including semantic color
and path behavior. Native encoder and device behavior require their separate platform tests.
