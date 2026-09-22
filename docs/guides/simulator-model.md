---
sidebar_position: 4
---

# Simulator model

BHTune's built-in simulator uses one generic **first-order-plus-dead-time (FOPDT)** model.
It is intended to make the tuning workflow reproducible and safe to exercise without a live
DCS, PLC, or OPC DA gateway. The six process categories currently available in the form do not
select six different physical models. They select process-specific MRFT tuning correlations and
defaults while sharing the same simulated process equation.

## Transfer function

The continuous-time transfer function is:

```text
G(s) = K * exp(-theta*s) / (tau*s + 1)
```

where:

| Symbol  | Meaning                                                                        |
| ------- | ------------------------------------------------------------------------------ |
| `K`     | Process gain: the steady-state PV change per unit MV change                    |
| `tau`   | Time constant, in seconds: how quickly the PV responds after the dead time     |
| `theta` | Dead time, in seconds: how long the PV waits before responding to an MV change |

Using an initial operating point, the equivalent differential equation is:

```text
tau * dPV/dt = -(PV - PV0) + K * (MV - MV0)
```

`PV0` and `MV0` are the configured initial process value and manipulated value. The process
starts at this operating point before the MRFT relay changes the MV.

## Discrete simulation

The simulator advances once for each PV read. During one simulated interval, the MV is treated
as constant, so the process uses the exact zero-order-hold solution of the first-order lag:

```text
decay = exp(-delta_t / tau)

PV_next = PV * decay
        + (1 - decay) * (PV0 - K * MV0 + K * MV_delayed)
```

This is a closed-form update, not a numerical ODE approximation. `delta_t` is the configured
simulator poll interval. In Demo mode it is fixed at 200 ms; Full mode uses the global
`[tuning].poll_interval_ms` setting.

Dead time is represented by a queue of delayed MV samples. The queue length is approximately:

```text
ceil(theta / delta_t)
```

The simulator's logical clock advances by `delta_t` on each PV read without sleeping for that
amount of wall-clock time. This allows a complete synthetic tune to run quickly while keeping
the model's timing deterministic.

## Noise and initial conditions

Measurement noise is added after the process update. It is sampled uniformly from:

```text
[-noise_amplitude, +noise_amplitude]
```

A zero noise amplitude disables noise. A fixed RNG seed produces the same noise sequence for
the same simulator configuration and MV-write sequence.

The initial PV is assumed to be the steady-state value for the initial MV. The configured PV
and MV ranges constrain the tuning setup and initial values; they do not turn the simulator into
a separate process model for each process category.

## Relationship to MRFT and PID

During a tune, MRFT drives the simulated MV directly by writing the relay output. The simulator
does not run a PID controller in the background. BHTune also contains a separate `VirtualPid`
helper for closed-loop validation, but that helper is not part of `SimulatorDriver` and does not
affect simulator tune results.

The selected DCS/PLC template remains meaningful in Simulator mode because it formats the
calculated constants using that system's native conventions, such as gain versus proportional
band and the applicable integral or derivative units. It changes result interpretation, not the
FOPDT physics.
