---
sidebar_position: 6
---

# ADR-0005: Use external schedulers for unattended runs

**Status:** Accepted

## Context

Scheduled or scripted tuning is available through the headless CLI. Operating systems already
provide job schedulers, and unattended execution does not require the web server. The
[CLI quickstart](../../getting-started/cli-quickstart.md) and the
[README automation section](https://github.com/bytehound-labs/bhtune/blob/main/README.md#automation)
describe this invocation model.

## Decision

Scheduling is handled by external tools such as cron or Windows Task Scheduler invoking
`bhtune`. BHTune does not include an in-product scheduler.

## Consequences

The CLI remains independently usable and does not depend on a continuously running
`bhtune-server`. Schedule definition, host-level service management, and scheduler-specific
alerting belong to the external tool; BHTune's safety limits and exit codes describe the run's
outcome.
