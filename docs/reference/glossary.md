---
sidebar_position: 1
---

# Glossary

Terms used by BHTune's tuning workflow, OPC DA integration, and run history.

| Term | Definition |
| --- | --- |
| **DA2 / DA3** | OPC Data Access 2 and OPC Data Access 3 browse sources reported by the gateway. The `da2` or `da3` label identifies the source used for namespace browsing; it is not a process type or a BHTune driver. |
| **FOPDT** | First-Order Plus Dead Time, a process model described by gain, time constant, and dead time. BHTune's in-process simulator uses an FOPDT model. See the [simulator model](../guides/simulator-model.md). |
| **Gateway** | The separate `opcda-bridge` service that exposes OPC DA servers to BHTune over a network connection. BHTune connects to the gateway rather than using DCOM directly. See [OPC gateway compatibility](../guides/opc-gateway-compatibility.md). |
| **Hysteresis** | A switching band around the MRFT's PV threshold. It prevents small noisy PV changes near the threshold from causing repeated relay switches. |
| **ItemID** | The exact identifier for an item, or tag, in an OPC DA server. Use the ItemID returned by the server's browse results or a trusted mapping; punctuation in an ItemID does not define its hierarchy. |
| **MRFT** | Modified Relay Feedback Test. BHTune switches the MV by a configured relay amplitude as the PV crosses a threshold, measures the resulting oscillation, and uses it to calculate PID settings. See [MRFT concepts](../guides/mrft-concepts.md). |
| **MV** | Manipulated Variable: the controller output that affects the process, such as a valve position. During a live MRFT, BHTune commands MV relay steps and verifies accepted writes by reading the value back. |
| **Namespace index** | The gateway's optional persistent index of an OPC DA server's browsable items. It enables whole-namespace search; ordinary browsing and direct ItemID entry do not depend on it. |
| **Process type** | The process category selected for a tune: flow, pressure-line, pressure-vessel, level, temperature-mixing, or temperature-heat-exchange. It selects tuning correlations and process defaults; it is separate from the DCS/PLC template. |
| **ProgID** | The registered programmatic identifier of an OPC DA server on the gateway host. `bhtune opc servers` lists registered ProgIDs so you can select the exact server. |
| **PV** | Process Variable: the measured value of the process that the controller regulates. BHTune samples the PV to detect the MRFT oscillation and calculate tuning results. |
| **Quality** | The OPC status attached to a value. BHTune treats exact `Good` as good, exact `Uncertain` according to the configured policy, and every other quality as bad for tuning-critical reads. |
| **Relay amplitude** | The size of each MRFT MV step, expressed as a percentage of the configured MV range. It is an input to the test, not a PID setting. |
| **Restore** | The best-effort return of applicable live loop values to their recorded pre-test targets after a tune. It is distinct from reverting PID constants: `bhtune history revert` restores the values recorded before a PID write, while `bhtune restore-loop` recovers an explicitly eligible orphaned tune. |
| **SP** | Setpoint: the target value the controller is meant to maintain. For an OPC DA tune that starts in the template-defined Auto mode with a configured setpoint tag, BHTune records the original SP for restoration. |
| **Template** | A DCS/PLC definition that maps a base tag to loop tags and describes controller-specific values such as mode codes, PID units, and rounding. Templates are not process types. See [DCS/PLC templates](../dcs-templates.md). |
| **Write-back** | An explicitly requested write of calculated PID constants to their mapped controller tags. BHTune reads the prior values, writes and verifies each constant, and records the result. A simulator has no PID tags to write back to. |
