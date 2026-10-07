# bhtune

The BHTune command-line application and reusable Rust orchestration library. The package
provides the `bhtune` executable for read-only tune preflight, MRFT tunes, simulation,
template management, run history, exports, and OPC DA diagnostics.

`bhtune check` validates tune inputs and reads the configured OPC tags, values, and quality
without starting an MRFT, writing to the controller, or changing the database. Its
`--write-pid` option assesses read-only readiness only; it does not test controller write
permissions.

PID reviews, table output, and new controller writes use the run's snapshotted template
precision. JSON retains raw result fields and names controller-ready values separately;
previous values, readbacks, rollback, and history revert retain their recorded precision.
The final tune summary uses previews captured during calculation; it performs no database
queries after execution that could replace a confirmed safety exit code.

The OPC DA path uses the separate `opcda-bridge` gateway. The simulator runs in-process and
does not require a DCS/PLC connection.

API documentation: <https://docs.rs/bhtune>
