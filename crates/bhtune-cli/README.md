# bhtune-cli

The BHTune command-line application and reusable Rust orchestration library. The package
provides the `bhtune` executable for running MRFT tunes, simulation, template management,
run history, exports, and OPC DA diagnostics.

The OPC DA path uses the separate `opcda-bridge` gateway. The simulator runs in-process and
does not require a DCS/PLC connection.

API documentation: <https://docs.rs/bhtune-cli>
