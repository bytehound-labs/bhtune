# bhtune-test-support

Unpublished shared test doubles for BHTune. This crate is not a product and is not a
release artifact.

`MockBridgeService` is the one mock gRPC `Bridge` used by `bhtune-driver`, `bhtune-cli`,
and `bhtune-server` tests. It depends only on `opcda-bridge-proto`, so those crates can
depend on it from tests without a cycle. `start_mock_server` binds an ephemeral localhost
port and returns a handle whose `shutdown` method joins the server task.

The `mock-driver` feature is an empty cycle guard. The existing in-process `MockDriver`
doubles stay in the single crate that uses each one. `bhtune-driver` must not enable the
feature.

Consumers declare it as a versionless path dev-dependency. A version would make
`cargo package` rewrite that dependency into a crates.io requirement, and this crate is
not published. Cargo strips the versionless declaration from packaged manifests.
