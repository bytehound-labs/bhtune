//! `SimulatorBackend`: an in-process FOPDT (first-order-plus-dead-time) process model,
//! served through the [`Backend`] trait, for fully automated E2E tests (no Windows, no
//! Kepware, no external process) and demo mode.
//!
//! Ported from `Model/ProcessModelOPC.py`, the Python model the legacy app's hidden
//! `OPCClass.Python` test path shells out to (see `AGENTS.md`'s behavior-spec notes). This
//! module splits into three independent pieces:
//!
//! - [`FopdtProcess`]: the process itself -- pure state advanced one tick at a time, with no
//!   `Backend`/async awareness at all.
//! - [`VirtualPid`]: a standalone position-form PID controller, for *closed-loop* validation
//!   (e.g. "do the constants a completed MRFT run just calculated actually control this
//!   process well?") and demos. Not wired into [`SimulatorBackend`] -- the open-loop MRFT
//!   relay test drives the MV itself, so nothing here needs to close the loop automatically.
//! - [`SimulatorBackend`]: the thin [`Backend`] shell wrapping one [`FopdtProcess`].

use std::{collections::VecDeque, sync::Mutex};

use async_trait::async_trait;
use rand::{RngExt, SeedableRng, rngs::StdRng};

use crate::{
    backend::Backend,
    error::{BackendError, BackendResult},
    types::{Quality, TagId, TagNode, TagValue, TagWrite, WriteOutcome},
};

/// Configuration for a [`FopdtProcess`]: the classic three parameters process control
/// literature uses to characterize a first-order-plus-dead-time (FOPDT) process, plus the
/// tick cadence and measurement noise needed to turn the continuous model into a discrete
/// one a [`Backend`] can serve one sample at a time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FopdtConfig {
    /// Process gain (`Kp`): steady-state change in PV per unit change in MV.
    pub gain: f32,
    /// Time constant (`tau`), in seconds: how quickly PV approaches its new steady state
    /// after an MV change, absent dead time. `0.0` means an instantaneous response (PV
    /// reaches its new steady state within a single tick).
    pub time_constant_s: f32,
    /// Dead time (`theta`), in seconds: how long PV takes to begin responding to an MV
    /// change at all. Modeled as a whole number of ticks
    /// (`ceil(dead_time_s / tick_interval_s)`), matching `Model/ProcessModelOPC.py`'s own
    /// `ndelay` calculation. `0.0` disables the delay line entirely.
    pub dead_time_s: f32,
    /// Simulated seconds advanced per [`FopdtProcess::step`] call. Deliberately unrelated to
    /// real wall-clock time -- a caller can call `step` (via [`SimulatorBackend::read`]) as
    /// fast as the CPU allows, and the simulated clock still advances at this rate, which is
    /// what lets an E2E test run an entire tuning cycle in milliseconds instead of the
    /// minutes a real 800ms-tick MRFT test takes.
    pub tick_interval_s: f32,
    /// Amplitude of uniform noise (`+/- noise_amplitude`, in PV engineering units) added to
    /// each computed sample, mirroring `ProcessModelOPC.py`'s `noise_amp`. `0.0` (the
    /// default from [`FopdtConfig::new`]) disables noise entirely and skips drawing from
    /// the RNG altogether, keeping every sample exactly reproducible run to run.
    pub noise_amplitude: f32,
}

impl FopdtConfig {
    /// A process configuration with no measurement noise. Chain
    /// [`FopdtConfig::with_noise_amplitude`] to add some, or set the field directly.
    pub fn new(
        gain: f32,
        time_constant_s: f32,
        dead_time_s: f32,
        tick_interval_s: f32,
    ) -> FopdtConfig {
        FopdtConfig {
            gain,
            time_constant_s,
            dead_time_s,
            tick_interval_s,
            noise_amplitude: 0.0,
        }
    }

    /// Returns `self` with `noise_amplitude` set, for a fluent construction style.
    pub fn with_noise_amplitude(mut self, noise_amplitude: f32) -> FopdtConfig {
        self.noise_amplitude = noise_amplitude;
        self
    }
}

/// A first-order-plus-dead-time process model, advanced one tick at a time.
///
/// Uses the exact analytical solution for a first-order lag driven by a piecewise-constant
/// (zero-order-hold) input over each tick --
/// `pv_new = pv*decay + (1-decay)*(bias + gain*mv_effective)`,
/// `decay = exp(-tick_interval_s / time_constant_s)` -- rather than numerically integrating
/// an ODE every tick, as `Model/ProcessModelOPC.py`'s `scipy.integrate.odeint` call does.
/// This is not an approximation: it is the closed-form solution of
/// `tau * d(pv)/dt = -(pv - pv0) + gain*(mv - mv0)` for an input held constant over
/// `[t, t+dt]`, which is exactly what "the controller wrote this MV and it stays there until
/// the next write" means physically. Cross-checked numerically (in a disposable scratch
/// script, not part of this repo) against the reference script's own `odeint`-based
/// integration across a range of gain/tau/dt combinations before relying on it -- the two
/// agree to within `odeint`'s own numerical tolerance (~1e-5).
#[derive(Debug)]
pub struct FopdtProcess {
    config: FopdtConfig,
    /// `pv0 - gain*mv0`, fixed at construction from the initial operating point. Folds the
    /// reference script's separate `Bias`/deviation-variable bookkeeping into one constant:
    /// expanding the ODE solution above shows any choice of anchor point (`pv0`, `mv0`)
    /// yields the same absolute-PV trajectory as long as this one value is held fixed, so
    /// there is nothing else worth tracking separately.
    bias: f32,
    /// `exp(-tick_interval_s / time_constant_s)` (or `0.0` for an instant-response
    /// process), precomputed once since it never changes.
    decay: f32,
    pv: f32,
    /// The most recently recorded MV (via [`FopdtProcess::set_mv`]) -- not yet delayed.
    current_mv: f32,
    /// Transport-delay line: holds the last `ceil(dead_time_s / tick_interval_s)` MV values,
    /// oldest first. Seeded with that many copies of the initial MV so the process starts
    /// at rest rather than assuming a fictitious pre-history.
    mv_delay_line: VecDeque<f32>,
    rng: StdRng,
}

impl FopdtProcess {
    /// Builds a process starting at rest: `initial_pv` is assumed to already be the correct
    /// steady-state PV for `initial_mv` (i.e. nothing changes until a [`FopdtProcess::set_mv`]
    /// call moves the MV away from `initial_mv`). `seed` makes the noise sequence
    /// reproducible -- the same config/seed/write sequence always produces the same PV
    /// sequence (on a given platform and `rand` version; see [`rand::rngs::StdRng`]'s own
    /// caveat that its algorithm is not a portability guarantee across those).
    pub fn new(config: FopdtConfig, initial_pv: f32, initial_mv: f32, seed: u64) -> FopdtProcess {
        let bias = initial_pv - config.gain * initial_mv;
        let decay = if config.time_constant_s > 0.0 {
            (-config.tick_interval_s / config.time_constant_s).exp()
        } else {
            0.0
        };
        let delay_ticks = if config.dead_time_s > 0.0 && config.tick_interval_s > 0.0 {
            (config.dead_time_s / config.tick_interval_s).ceil() as usize
        } else {
            0
        };
        FopdtProcess {
            config,
            bias,
            decay,
            pv: initial_pv,
            current_mv: initial_mv,
            mv_delay_line: std::iter::repeat_n(initial_mv, delay_ticks).collect(),
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// The current PV, as of the last [`FopdtProcess::step`] call (or `initial_pv`, if
    /// `step` has never been called).
    pub fn pv(&self) -> f32 {
        self.pv
    }

    /// The most recently recorded MV (see [`FopdtProcess::set_mv`]) -- not delayed; this is
    /// what a real DCS's own MV readback tag would report immediately. Only the PV's
    /// response to it is delayed.
    pub fn mv(&self) -> f32 {
        self.current_mv
    }

    /// Records a new controller output, effective from the next [`FopdtProcess::step`] call
    /// onward (after passing through the dead-time delay line, if configured).
    pub fn set_mv(&mut self, mv: f32) {
        self.current_mv = mv;
    }

    /// Advances the process by one `tick_interval_s`, using whichever MV was most recently
    /// recorded via [`FopdtProcess::set_mv`] -- delayed by `dead_time_s`, if configured -- as
    /// the input, and returns the resulting PV (including noise, if configured).
    pub fn step(&mut self) -> f32 {
        self.mv_delay_line.push_back(self.current_mv);
        let effective_mv = self.mv_delay_line.pop_front().unwrap_or(self.current_mv);

        self.pv = self.pv * self.decay
            + (1.0 - self.decay) * (self.bias + self.config.gain * effective_mv);

        if self.config.noise_amplitude > 0.0 {
            self.pv += self
                .rng
                .random_range(-self.config.noise_amplitude..=self.config.noise_amplitude);
        }

        self.pv
    }
}

/// Configuration for a [`VirtualPid`]: a standard textbook position-form PID controller --
/// proportional and integral on error, derivative on the measurement rather than the error
/// (to avoid "derivative kick" on a setpoint change) -- with output clamping and
/// anti-reset-windup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VirtualPidConfig {
    /// Controller gain (`Kc`).
    pub kc: f32,
    /// Integral time in seconds (`Ti`). `None` disables integral action entirely (a P-only
    /// or PD controller) rather than requiring some sentinel "infinite" value.
    pub ti_s: Option<f32>,
    /// Derivative time in seconds (`Td`). `None` disables derivative action (a P-only or PI
    /// controller).
    pub td_s: Option<f32>,
    pub output_min: f32,
    pub output_max: f32,
    /// The output value corresponding to zero accumulated error -- i.e. the operating point
    /// the controller starts from (matches `op[0]` in `Model/ProcessModelOPC.py`'s reference
    /// controller, which biases every computed output by this same constant).
    pub output_bias: f32,
}

/// A standalone position-form PID controller, for *closed-loop* validation and demos (e.g.
/// "do the constants a completed MRFT run just calculated actually control this process
/// well?"). Deliberately not wired into [`SimulatorBackend`]/[`Backend`] at all: the
/// `Backend` trait models open-loop tag I/O, and during an actual MRFT relay test the engine
/// itself (`bhtune_core::mrft::MrftEngine`) drives the MV -- nothing needs to close the loop
/// automatically for that. This exists for whatever, later, wants to run this process in
/// automatic mode instead (e.g. simulating a completed tune's results before trusting them
/// against a real DCS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VirtualPid {
    config: VirtualPidConfig,
    integral: f32,
    prev_pv: Option<f32>,
}

impl VirtualPid {
    pub fn new(config: VirtualPidConfig) -> VirtualPid {
        VirtualPid {
            config,
            integral: 0.0,
            prev_pv: None,
        }
    }

    /// Computes one control step given `setpoint`/`pv` and the elapsed time `dt` (seconds)
    /// since the previous call, returning the clamped controller output.
    ///
    /// Derivative acts on `pv`, not on the error, so a setpoint change alone never spikes the
    /// output -- mirrors `Model/ProcessModelOPC.py`'s own
    /// `D = -Kc*tauD*(pv[i]-pv[i-1])/dt` (mathematically equivalent to derivative-on-error
    /// only while the setpoint itself is unchanging). The very first call has no previous
    /// `pv` to compare against, so derivative action is skipped for that call only.
    pub fn step(&mut self, setpoint: f32, pv: f32, dt: f32) -> f32 {
        let error = setpoint - pv;

        let integral_gain = self
            .config
            .ti_s
            .filter(|ti| *ti > 0.0)
            .map(|ti| self.config.kc / ti);
        let candidate_integral = self.integral + error * dt;

        let derivative_term = match (self.config.td_s, self.prev_pv) {
            (Some(td), Some(prev_pv)) if td > 0.0 && dt > 0.0 => {
                -self.config.kc * td * (pv - prev_pv) / dt
            }
            _ => 0.0,
        };
        self.prev_pv = Some(pv);

        let proportional_term = self.config.kc * error;
        let integral_term = integral_gain.map_or(0.0, |ki| ki * candidate_integral);
        let raw_output =
            self.config.output_bias + proportional_term + integral_term + derivative_term;
        let clamped_output = raw_output.clamp(self.config.output_min, self.config.output_max);

        // Anti-reset-windup: only commit this tick's integral contribution if doing so
        // didn't require clamping the output. Otherwise the accumulator would keep growing
        // while already saturated, delaying recovery once the error eventually reverses --
        // mirrors the reference script's `ie[i] -= e[i]*delta_t` undo-on-saturation, just
        // phrased as "don't commit" instead of "commit then undo".
        if clamped_output == raw_output {
            self.integral = candidate_integral;
        }

        clamped_output
    }
}

/// The [`Backend`] implementation for CI E2E tests and demo mode: an in-process
/// [`FopdtProcess`] served through exactly two tags -- a PV tag (reading it advances the
/// simulated clock by one tick) and an MV tag (reading it reports the last-written value
/// without advancing anything; writing it records a new controller output). Any other tag
/// is [`BackendError::InvalidTagValue`]; [`Backend::browse`] is always
/// [`BackendError::Unsupported`], per that method's own documented convention for backends
/// with no real tag tree.
///
/// Uses a plain `std::sync::Mutex`, not `tokio::sync::Mutex` like [`crate::OpcDaBackend`]:
/// every operation here is synchronous, in-memory math with no `.await` point anywhere in
/// the critical section, so there is nothing that could hold the guard across a suspension
/// point -- the concern `tokio::sync::Mutex` exists for -- in the first place.
#[derive(Debug)]
pub struct SimulatorBackend {
    pv_tag: TagId,
    mv_tag: TagId,
    process: Mutex<FopdtProcess>,
}

impl SimulatorBackend {
    /// `pv_tag`/`mv_tag` are the only two tags this backend recognizes -- pick names that
    /// match whatever tag configuration the rest of a test or demo run uses, so the same
    /// tag names work whether the caller is pointed at this backend or a real
    /// [`crate::OpcDaBackend`].
    pub fn new(
        pv_tag: impl Into<TagId>,
        mv_tag: impl Into<TagId>,
        config: FopdtConfig,
        initial_pv: f32,
        initial_mv: f32,
        seed: u64,
    ) -> SimulatorBackend {
        SimulatorBackend {
            pv_tag: pv_tag.into(),
            mv_tag: mv_tag.into(),
            process: Mutex::new(FopdtProcess::new(config, initial_pv, initial_mv, seed)),
        }
    }
}

#[async_trait]
impl Backend for SimulatorBackend {
    async fn read(&self, tags: &[TagId]) -> BackendResult<Vec<TagValue>> {
        let mut process = self.process.lock().unwrap();
        tags.iter()
            .map(|tag| {
                let value = if *tag == self.pv_tag {
                    process.step()
                } else if *tag == self.mv_tag {
                    process.mv()
                } else {
                    return Err(BackendError::InvalidTagValue {
                        tag: tag.clone(),
                        message: "SimulatorBackend only knows its configured PV/MV tags"
                            .to_string(),
                    });
                };
                Ok(TagValue {
                    tag: tag.clone(),
                    value: value.to_string(),
                    quality: Quality::Good,
                    timestamp: None,
                })
            })
            .collect()
    }

    async fn write(&self, tag: &TagId, value: TagWrite) -> BackendResult<WriteOutcome> {
        if *tag != self.mv_tag {
            return Err(BackendError::InvalidTagValue {
                tag: tag.clone(),
                message: "SimulatorBackend only accepts writes to its configured MV tag"
                    .to_string(),
            });
        }
        let mv = match value {
            TagWrite::Float(f) => f,
            TagWrite::Raw(s) => match s.parse::<f32>() {
                Ok(f) => f,
                Err(_) => {
                    return Ok(WriteOutcome::failure(format!(
                        "'{s}' is not a valid numeric MV value"
                    )));
                }
            },
        };
        self.process.lock().unwrap().set_mv(mv);
        Ok(WriteOutcome::success())
    }

    async fn browse(&self, _path: &str) -> BackendResult<Vec<TagNode>> {
        Err(BackendError::Unsupported {
            operation: "browse",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- FopdtProcess --------------------------------------------------------------------

    #[test]
    fn step_response_settles_at_gain_times_mv_at_steady_state() {
        let config = FopdtConfig::new(2.0, 5.0, 0.0, 1.0);
        let mut process = FopdtProcess::new(config, 50.0, 25.0, 0);
        process.set_mv(30.0);

        let mut pv = 50.0;
        for _ in 0..200 {
            pv = process.step();
        }

        // bias = 50 - 2*25 = 0; target = bias + gain*mv = 0 + 2*30 = 60.
        assert!((pv - 60.0).abs() < 1e-3, "expected pv near 60.0, got {pv}");
    }

    #[test]
    fn matches_the_multi_tick_closed_form_solution() {
        // A second, independently-coded computation path (a one-shot closed-form formula
        // using `decay.powi(n)`) cross-checked against the per-tick iterative
        // implementation -- not merely restating the same recurrence.
        let config = FopdtConfig::new(1.0, 4.0, 0.0, 1.0);
        let mut process = FopdtProcess::new(config, 10.0, 10.0, 0);
        process.set_mv(20.0);

        let decay: f32 = (-1.0_f32 / 4.0).exp();
        let target = 20.0; // bias(0) + gain(1)*mv(20)
        for n in 1..=6 {
            let pv = process.step();
            let expected = 10.0 * decay.powi(n) + target * (1.0 - decay.powi(n));
            assert!(
                (pv - expected).abs() < 1e-4,
                "tick {n}: pv={pv} expected={expected}"
            );
        }
    }

    #[test]
    fn dead_time_delays_the_response_by_the_configured_number_of_ticks() {
        let config = FopdtConfig::new(1.0, 4.0, 3.0, 1.0); // 3s dead time / 1s ticks = 3 ticks
        let mut process = FopdtProcess::new(config, 10.0, 10.0, 0);
        process.set_mv(20.0);

        // For the first 3 ticks the delay line still reports the initial MV (10.0), so PV
        // must not have moved from steady state at all.
        for tick in 0..3 {
            let pv = process.step();
            assert!(
                (pv - 10.0).abs() < 1e-4,
                "tick {tick}: expected pv to stay at 10.0 during dead time, got {pv}"
            );
        }
        // From the 4th tick onward the new MV has propagated through the delay line and PV
        // must start moving toward the new target.
        let pv_after_delay = process.step();
        assert!(
            pv_after_delay > 10.1,
            "expected pv to start moving after dead time elapsed, got {pv_after_delay}"
        );
    }

    #[test]
    fn zero_dead_time_responds_on_the_very_first_tick() {
        let config = FopdtConfig::new(1.0, 4.0, 0.0, 1.0);
        let mut process = FopdtProcess::new(config, 10.0, 10.0, 0);
        process.set_mv(20.0);
        assert!(process.step() > 10.1);
    }

    #[test]
    fn same_seed_produces_identical_pv_sequences() {
        let config = FopdtConfig::new(1.0, 4.0, 0.0, 1.0).with_noise_amplitude(0.5);
        let mut a = FopdtProcess::new(config, 10.0, 10.0, 42);
        let mut b = FopdtProcess::new(config, 10.0, 10.0, 42);
        a.set_mv(15.0);
        b.set_mv(15.0);
        let seq_a: Vec<f32> = (0..20).map(|_| a.step()).collect();
        let seq_b: Vec<f32> = (0..20).map(|_| b.step()).collect();
        assert_eq!(seq_a, seq_b);
    }

    #[test]
    fn different_seeds_produce_different_pv_sequences() {
        let config = FopdtConfig::new(1.0, 4.0, 0.0, 1.0).with_noise_amplitude(0.5);
        let mut a = FopdtProcess::new(config, 10.0, 10.0, 1);
        let mut b = FopdtProcess::new(config, 10.0, 10.0, 2);
        a.set_mv(15.0);
        b.set_mv(15.0);
        let seq_a: Vec<f32> = (0..20).map(|_| a.step()).collect();
        let seq_b: Vec<f32> = (0..20).map(|_| b.step()).collect();
        assert_ne!(seq_a, seq_b);
    }

    #[test]
    fn noise_never_exceeds_the_configured_amplitude() {
        // gain=0.0 and time_constant_s=0.0 mean the deterministic component of `pv` is
        // exactly 0.0 on every single tick, regardless of history (decay=0.0 discards the
        // previous, noise-carrying pv entirely) -- so each returned sample is purely that
        // tick's noise draw, cleanly isolated for this assertion.
        let config = FopdtConfig::new(0.0, 0.0, 0.0, 1.0).with_noise_amplitude(0.3);
        let mut process = FopdtProcess::new(config, 0.0, 0.0, 7);
        for _ in 0..500 {
            let pv = process.step();
            assert!(pv.abs() <= 0.3, "noise sample {pv} exceeded amplitude 0.3");
        }
    }

    #[test]
    fn mv_reports_the_last_written_value_without_advancing_pv() {
        let config = FopdtConfig::new(1.0, 4.0, 0.0, 1.0);
        let mut process = FopdtProcess::new(config, 10.0, 10.0, 0);
        assert_eq!(process.mv(), 10.0);
        process.set_mv(25.0);
        assert_eq!(process.mv(), 25.0);
        assert_eq!(process.pv(), 10.0); // step() never called -- pv must be untouched
    }

    // --- VirtualPid ------------------------------------------------------------------------

    #[test]
    fn proportional_only_output_matches_kc_times_error_plus_bias() {
        let config = VirtualPidConfig {
            kc: 2.0,
            ti_s: None,
            td_s: None,
            output_min: -1000.0,
            output_max: 1000.0,
            output_bias: 5.0,
        };
        let mut pid = VirtualPid::new(config);
        let output = pid.step(60.0, 50.0, 1.0);
        // error = 10.0, P = 2.0*10.0 = 20.0, output = bias(5) + 20 = 25.0.
        assert!((output - 25.0).abs() < 1e-4);
    }

    #[test]
    fn anti_windup_prevents_the_integral_from_growing_while_saturated() {
        let config = VirtualPidConfig {
            kc: 1.0,
            ti_s: Some(2.0),
            td_s: None,
            output_min: 0.0,
            output_max: 10.0,
            output_bias: 0.0,
        };
        let mut pid = VirtualPid::new(config);

        // A huge, sustained positive error saturates the output high for many ticks -- if
        // the integral term were allowed to keep accumulating while saturated, it would
        // grow far beyond what's needed to reach output_max.
        for _ in 0..100 {
            assert_eq!(pid.step(1000.0, 0.0, 1.0), 10.0);
        }

        // A *small* negative error (a slight overshoot past the setpoint) should
        // immediately pull the output below saturation -- proving the integral accumulator
        // did not keep growing unboundedly during the 100 saturated ticks above (if it had,
        // an error of this modest size couldn't possibly overcome it, and output would stay
        // pinned at 10.0).
        let output = pid.step(-1.0, 0.0, 1.0);
        assert!(
            output < 10.0,
            "expected output to leave saturation, got {output}"
        );
    }

    #[test]
    fn derivative_acts_on_measurement_so_a_setpoint_step_causes_no_kick() {
        let config = VirtualPidConfig {
            kc: 1.0,
            ti_s: None,
            td_s: Some(5.0),
            output_min: -1000.0,
            output_max: 1000.0,
            output_bias: 0.0,
        };
        let mut pid = VirtualPid::new(config);

        // Prime `prev_pv` with an initial call.
        pid.step(50.0, 50.0, 1.0);

        // The setpoint now jumps by 40 (a typical operator setpoint change), but PV itself
        // hasn't moved -- a derivative-on-error implementation would compute a huge
        // spurious derivative kick here (`d(error)/dt` includes the setpoint's own jump);
        // derivative-on-pv must not.
        let output = pid.step(90.0, 50.0, 1.0);
        // error=40, P=1*40=40, I=0 (disabled), D=0 (pv unchanged) => output=40.
        assert!((output - 40.0).abs() < 1e-4);
    }

    #[test]
    fn pid_and_fopdt_process_together_converge_to_the_setpoint() {
        // Gains numerically pre-verified (in a disposable scratch script, not part of this
        // repo) to converge cleanly with no oscillation for this specific process.
        let process_config = FopdtConfig::new(2.0, 5.0, 1.0, 1.0);
        let mut process = FopdtProcess::new(process_config, 20.0, 10.0, 0);

        let pid_config = VirtualPidConfig {
            kc: 0.8,
            ti_s: Some(6.0),
            td_s: None,
            output_min: 0.0,
            output_max: 100.0,
            output_bias: 10.0, // matches the process's initial_mv operating point
        };
        let mut pid = VirtualPid::new(pid_config);

        let setpoint = 45.0;
        let mut pv = process.pv();
        for _ in 0..500 {
            let mv = pid.step(setpoint, pv, 1.0);
            process.set_mv(mv);
            pv = process.step();
        }

        assert!(
            (pv - setpoint).abs() < 0.5,
            "expected convergence near {setpoint}, got {pv}"
        );
    }

    // --- SimulatorBackend ------------------------------------------------------------------

    fn backend() -> SimulatorBackend {
        SimulatorBackend::new(
            "Loop.PV",
            "Loop.MV",
            FopdtConfig::new(1.0, 4.0, 0.0, 1.0),
            50.0,
            50.0,
            0,
        )
    }

    #[tokio::test]
    async fn read_mv_tag_reports_current_mv_without_advancing_pv() {
        let backend = backend();
        let first = backend.read(&["Loop.MV".to_string()]).await.unwrap();
        let second = backend.read(&["Loop.MV".to_string()]).await.unwrap();
        assert_eq!(first[0].value, "50");
        assert_eq!(second[0].value, "50");
        assert_eq!(first[0].quality, Quality::Good);
    }

    #[tokio::test]
    async fn read_pv_tag_advances_the_simulated_process_each_call() {
        let backend = backend();
        backend
            .write(&"Loop.MV".to_string(), TagWrite::Float(80.0))
            .await
            .unwrap();

        let mut values = Vec::new();
        for _ in 0..5 {
            let read = backend.read(&["Loop.PV".to_string()]).await.unwrap();
            values.push(read[0].value.parse::<f32>().unwrap());
        }
        // Each successive read should move further toward the new MV-driven target (80.0),
        // proving each `read` call genuinely advances the simulated clock.
        for pair in values.windows(2) {
            assert!(pair[1] > pair[0], "expected monotonic approach: {values:?}");
        }
    }

    #[tokio::test]
    async fn write_accepts_a_raw_string_that_parses_as_a_number() {
        let backend = backend();
        let outcome = backend
            .write(&"Loop.MV".to_string(), TagWrite::Raw("65.5".to_string()))
            .await
            .unwrap();
        assert!(outcome.success);
        let read = backend.read(&["Loop.MV".to_string()]).await.unwrap();
        assert_eq!(read[0].value, "65.5");
    }

    #[tokio::test]
    async fn write_rejects_a_raw_string_that_does_not_parse_as_a_number() {
        let backend = backend();
        let outcome = backend
            .write(
                &"Loop.MV".to_string(),
                TagWrite::Raw("not-a-number".to_string()),
            )
            .await
            .unwrap();
        assert!(!outcome.success);
        assert!(outcome.error_message.is_some());
    }

    #[tokio::test]
    async fn read_unknown_tag_is_invalid_tag_value_not_a_panic() {
        let backend = backend();
        let err = backend
            .read(&["Nonexistent.Tag".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::InvalidTagValue { .. }));
    }

    #[tokio::test]
    async fn write_unknown_tag_is_invalid_tag_value_not_a_panic() {
        let backend = backend();
        let err = backend
            .write(&"Nonexistent.Tag".to_string(), TagWrite::Float(1.0))
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::InvalidTagValue { .. }));
    }

    #[tokio::test]
    async fn browse_is_unsupported() {
        let backend = backend();
        let err = backend.browse("").await.unwrap_err();
        assert!(matches!(
            err,
            BackendError::Unsupported {
                operation: "browse"
            }
        ));
    }

    /// End-to-end: a real `MrftEngine` (from `bhtune-core`) drives `SimulatorBackend`
    /// through the actual `Backend` trait -- not a hand-rolled process simulation local to
    /// the test, as `bhtune-core`'s own equivalent test necessarily uses (it cannot depend
    /// on `bhtune-backend`, which depends on it) -- and completes with plausible peaks,
    /// troughs, and switch counts. Proves `SimulatorBackend` is actually fit for its
    /// intended purpose: driving synthetic MRFT runs for `core-replay-harness`-style
    /// coverage and future `e2e-simulator` CI tests, entirely without wall-clock sleeps.
    #[tokio::test]
    async fn mrft_engine_completes_a_realistic_relay_test_against_the_simulator_backend() {
        use bhtune_core::{
            Action, ControllerDirection, ControllerType, InitialReadings, LoopConfig, MrftCompat,
            MrftEngine, ProcessType, ResponseLevel, Tick, lookup,
        };
        use chrono::{TimeZone, Utc};

        let pv_tag = "Loop.PV".to_string();
        let mv_tag = "Loop.MV".to_string();

        let initial = InitialReadings {
            pv_ini: 50.0,
            mv_ini: 50.0,
            mv_range_low: 0.0,
            mv_range_high: 100.0,
        };

        // 5 ticks of dead time plus a mild lag: relay feedback auto-tuning fundamentally
        // needs process phase lag to sustain oscillation (a memoryless process makes the
        // relay chatter every tick instead) -- matching the reasoning already established
        // for `bhtune-core`'s own equivalent test.
        let backend = SimulatorBackend::new(
            pv_tag.clone(),
            mv_tag.clone(),
            FopdtConfig::new(1.0, 2.0, 5.0, 1.0),
            initial.pv_ini,
            initial.mv_ini,
            0,
        );

        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 10.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        };
        let tc = lookup(
            config.process_type,
            config.controller_type,
            ResponseLevel::Aggressive,
        );
        let start_time = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let mut engine = MrftEngine::new(
            config,
            ControllerDirection::Reverse,
            tc.beta,
            initial,
            start_time,
            MrftCompat::default(),
        );

        let mut completion = None;
        for i in 1..=500 {
            let read = backend.read(std::slice::from_ref(&pv_tag)).await.unwrap();
            let pv: f32 = read[0].value.parse().unwrap();
            let time = start_time + chrono::Duration::seconds(i);

            for action in engine.step(Tick { time, pv }) {
                match action {
                    Action::WriteMv(mv) => {
                        backend.write(&mv_tag, TagWrite::Float(mv)).await.unwrap();
                    }
                    Action::Complete {
                        peaks,
                        troughs,
                        switch_times,
                        mv_sign_init,
                    } => {
                        completion = Some((peaks, troughs, switch_times, mv_sign_init));
                    }
                }
            }
            if completion.is_some() {
                break;
            }
        }

        let (peaks, troughs, switch_times, mv_sign_init) =
            completion.expect("engine should complete within 500 ticks");

        assert!(!peaks.is_empty(), "expected at least one recorded peak");
        assert!(!troughs.is_empty(), "expected at least one recorded trough");
        assert!(
            switch_times.len() >= 2,
            "expected multiple relay switches, got {}",
            switch_times.len()
        );
        assert!(mv_sign_init == 1 || mv_sign_init == -1);
        for pv in peaks.iter().chain(troughs.iter()) {
            assert!(pv.is_finite());
        }
    }
}
