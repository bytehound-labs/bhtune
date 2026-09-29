---
sidebar_position: 5
---

# ADR-0004: Use `f32` for core process and tuning values

**Status:** Accepted

## Context

OPC DA analog values commonly use single-precision representations, and replayable calculations
need one consistent numeric width from sampled values through the tuning math. The
[safety guide](../../guides/safety.md#input-validation) also treats `f32` precision as part of
validating usable values before a live operation.

## Decision

The core MRFT model, process ranges, and tuning calculations use `f32` for process values and
PID tuning numerics. Parsing and validation occur at the boundaries where external values enter
the application.

## Consequences

The engine's precision matches common analog-tag representations without widening and narrowing
values at each calculation step. Range checks, actuation tolerances, and result-validity checks
must account for single-precision limits.
