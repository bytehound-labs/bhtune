# bhtune-driver

The asynchronous `Driver` trait and tag-I/O types used by BHTune, with implementations for
OPC DA, an in-process first-order-plus-dead-time (FOPDT) simulator, and recorded-trace replay.

`OpcDaDriver` communicates through the separate `opcda-bridge` gateway; the application does
not use Windows COM/DCOM directly. `SimulatorDriver` provides a synthetic process for tests
and demonstrations, while `ReplayDriver` validates the driver abstraction against recorded
traces. `ReadOnlyDriver` forwards reads and capabilities while rejecting writes and browse
operations.

API documentation: <https://docs.rs/bhtune-driver>
