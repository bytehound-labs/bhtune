# bhtune-runtime

`bhtune-runtime` provides the shared application services used by the `bhtune` command-line
adapter and `bhtune-server` HTTP adapter. It owns configuration resolution, database
bootstrap and seeding, logging, retention, cancellation, driver and OPC helpers, tune
preparation and execution, write-back, history revert, restore, and shared sample exports.
It also provides read-only tune preflight, which reuses request, template, tag, and initial
state validation without creating a run or persisting history.

New PID writes and adapter previews share one controller-target gate: final-unit template
precision applies to active terms, while disable sentinels stay exact. Raw calculations,
pre-write values, readbacks, rollback, history revert, and loop recovery are unrounded.

The runtime source and its direct dependencies are transport-neutral: it does not directly use
`clap`, an HTTP framework, or OpenAPI. The OPC DA gRPC client may bring transport crates
transitively; they are not part of the runtime API. The CLI owns command parsing, terminal
interaction, and process output; the server owns HTTP routes, request/response DTOs, OpenAPI
schemas, and SPA serving. Preflight uses a read-only database connection and a driver wrapper
that rejects writes.
