//! Properties that must hold for *every* input sequence.
//!
//! These exist because the whole value of the tool rests on two promises:
//!
//! 1. suppression never outlives a deadline (`suppression_is_bounded`), and
//! 2. an outage nobody asked for escalates instead of being swallowed.
//!
//! Note on (1): suppression is allowed *without* attribution during the grace
//! and settle windows — that is the deliberate hedge. What is forbidden is
//! suppression without a clock attached. A stricter "suppress only when
//! attributed" rule would let the first 9 seconds of a routine recreate through
//! as an alarm, which is the exact problem this tool exists to solve.

use guard_core::{Event, Guard, Input, Output, Reason, State, Timings};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Prior,
    Offline,
    Online,
    Die(i32),
    Tick,
}

fn op_strategy() -> impl Strategy<Value = (Op, u64)> {
    (
        prop_oneof![
            Just(Op::Prior),
            Just(Op::Offline),
            Just(Op::Online),
            (0i32..=255).prop_map(Op::Die),
            Just(Op::Tick),
        ],
        0u64..5_000u64,
    )
}

/// Long enough for every window to expire, plus margin.
const QUIET_MS: u64 = 600_000;

fn settle_to_idle(g: &mut Guard, now: &mut u64) {
    g.step(Input::BridgeOnline, *now);
    *now += 1;
    g.step(Input::Tick, *now + QUIET_MS);
    *now += QUIET_MS;
}

proptest! {
    /// Any active suppression is attached to a deadline, and a confirmed
    /// outage is never suppressed.
    #[test]
    fn suppression_is_always_bounded(ops in prop::collection::vec(op_strategy(), 0..40)) {
        let mut g = Guard::new(Timings::default());
        let mut now = 0u64;
        for (op, dt) in ops {
            now += dt;
            g.step(to_input(&op), now);
            prop_assert!(
                g.suppression_is_bounded(),
                "unbounded suppression in state {}",
                g.state().as_str()
            );
            if g.state() == State::OutageConfirmed {
                prop_assert!(!g.suppression(), "confirmed outage was suppressed");
            }
        }
    }

    /// Silence always releases the suppression. The confirmed-outage state may
    /// legitimately persist until the bridge returns — that is information, not
    /// a stuck alarm.
    #[test]
    fn silence_always_releases_suppression(ops in prop::collection::vec(op_strategy(), 0..40)) {
        let mut g = Guard::new(Timings::default());
        let mut now = 0u64;
        for (op, dt) in ops {
            now += dt;
            g.step(to_input(&op), now);
        }
        g.step(Input::Tick, now + QUIET_MS);
        prop_assert!(!g.suppression());
        prop_assert!(
            matches!(g.state(), State::Idle | State::OutageConfirmed),
            "unexpected state {}",
            g.state().as_str()
        );
    }

    /// An unattributed outage escalates within grace + 1 ms, no matter what
    /// happened before it.
    #[test]
    fn unattributed_outage_always_escalates(prefix in prop::collection::vec(op_strategy(), 0..20)) {
        let mut g = Guard::new(Timings::default());
        let mut now = 0u64;
        for (op, dt) in prefix {
            now += dt;
            g.step(to_input(&op), now);
        }
        // Return to a quiet, online baseline first — a confirmed outage from the
        // prefix would otherwise still be in progress.
        settle_to_idle(&mut g, &mut now);
        g.step(Input::Tick, now + 600_000);
        now += 600_000;

        let mut events = g.step(Input::BridgeOffline, now);
        prop_assert!(g.suppression(), "grace window must suppress briefly");
        prop_assert_eq!(g.state(), State::Suspect);

        events.extend(g.step(Input::Tick, now + Timings::default().grace_ms + 1));
        prop_assert_eq!(g.state(), State::OutageConfirmed);
        prop_assert!(!g.suppression());
        let escalated = events
            .iter()
            .any(|o| matches!(o, Output::Event(Event::OutageConfirmed { .. })));
        prop_assert!(escalated, "no OutageConfirmed event emitted");
    }
}

fn to_input(op: &Op) -> Input {
    match op {
        Op::Prior => Input::PriorSignal {
            reason: Reason::PriorSignal,
            window_ms: 120_000,
        },
        Op::Offline => Input::BridgeOffline,
        Op::Online => Input::BridgeOnline,
        Op::Die(code) => Input::ContainerDie { exit_code: *code },
        Op::Tick => Input::Tick,
    }
}
